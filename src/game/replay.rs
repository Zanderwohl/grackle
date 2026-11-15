//! Drawing the match late, for the kill cam: every world mesh's `GlobalTransform` is recorded as it
//! is drawn and an older one put in its place for the render, then the live one is put back in
//! `First`, so nothing that simulates ever reads a replayed value. See "Record and replay" in
//! [documentation/kill-cam.md](../../documentation/kill-cam.md).

use std::collections::VecDeque;

use bevy::camera::visibility::{RenderLayers, VisibilitySystems};
use bevy::ecs::entity::EntityHashMap;
use bevy::light::NotShadowCaster;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::common::air_lit::LitMaterial;
use crate::common::render_layers;

/// The replay is running while this exists; nothing is recorded without it.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct Replay {
    /// Seconds of `Time` (virtual): the moment drawn this frame.
    pub view: f64,
    /// Time has stopped at `view`, so no more frames are needed.
    pub frozen: bool,
}

#[derive(Resource, Default)]
pub struct ReplayHistory {
    tracks: EntityHashMap<Track>,
    last_recorded: Option<f64>,
    /// The live value of every `GlobalTransform` written this frame, put back in `First`.
    overridden: Vec<(Entity, GlobalTransform)>,
}

impl ReplayHistory {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.overridden.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    at: f64,
    placed: GlobalTransform,
    shown: bool,
}

struct Track {
    /// Oldest first.
    frames: VecDeque<Frame>,
    /// When it appeared, if after the replay began: before then it is drawn out of sight.
    born: Option<f64>,
    look: Option<Look>,
    ghost: Option<Entity>,
}

/// What a ghost needs to draw a mesh that has been despawned.
#[derive(Clone)]
struct Look {
    mesh: Handle<Mesh>,
    material: GhostMaterial,
    layers: Option<RenderLayers>,
    tag: Option<MeshTag>,
    no_shadow: bool,
}

#[derive(Clone)]
enum GhostMaterial {
    Standard(Handle<StandardMaterial>),
    Lit(Handle<LitMaterial>),
}

/// Stands in for a despawned mesh while the view still has frames of it.
#[derive(Component)]
pub struct ReplayGhost;

/// Far below any map, where frustum culling drops it.
fn out_of_sight() -> GlobalTransform {
    GlobalTransform::from_translation(Vec3::new(0.0, -100_000.0, 0.0))
}

fn lerp(a: &GlobalTransform, b: &GlobalTransform, t: f32) -> GlobalTransform {
    let (sa, ra, ta) = a.to_scale_rotation_translation();
    let (sb, rb, tb) = b.to_scale_rotation_translation();
    GlobalTransform::from(Transform { translation: ta.lerp(tb, t), rotation: ra.slerp(rb, t), scale: sa.lerp(sb, t) })
}

impl Track {
    /// Where it is drawn at `view`: `None` before it was born.
    fn sample(&self, view: f64) -> Option<(GlobalTransform, bool)> {
        if self.born.is_some_and(|born| view < born) {
            return None;
        }
        let after = self.frames.partition_point(|frame| frame.at <= view);
        let frame = match after {
            0 => return self.frames.front().map(|frame| (frame.placed, frame.shown)),
            after => self.frames[after - 1],
        };
        let Some(next) = self.frames.get(after) else { return Some((frame.placed, frame.shown)) };
        let t = ((view - frame.at) / (next.at - frame.at)).clamp(0.0, 1.0) as f32;
        Some((lerp(&frame.placed, &next.placed, t), frame.shown))
    }

    /// Frames before the view are never sampled again, except the last of them.
    fn prune(&mut self, view: f64) {
        while self.frames.len() > 1 && self.frames[1].at <= view {
            self.frames.pop_front();
        }
    }

    fn last_at(&self) -> f64 {
        self.frames.back().map_or(f64::NEG_INFINITY, |frame| frame.at)
    }
}

fn on_the_world(layers: Option<&RenderLayers>) -> bool {
    layers.is_none_or(|layers| layers.intersects(&RenderLayers::layer(render_layers::WORLD)))
}

pub struct ReplayPlugin;

impl Plugin for ReplayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ReplayHistory>()
            .add_systems(First, put_back_the_live_transforms)
            .add_systems(
                PostUpdate,
                (record, play)
                    .chain()
                    .after(TransformSystems::Propagate)
                    .after(VisibilitySystems::VisibilityPropagate)
                    .before(VisibilitySystems::CheckVisibility),
            );
    }
}

/// Runs whether or not a replay is: a system skipped by a run condition would see every change
/// since it last ran as new on the first frame of the next one.
#[allow(clippy::type_complexity)]
fn record(
    mut commands: Commands,
    time: Res<Time>,
    replay: Option<Res<Replay>>,
    mut history: ResMut<ReplayHistory>,
    meshes: Query<
        (
            Entity,
            Ref<GlobalTransform>,
            Option<&InheritedVisibility>,
            Ref<Mesh3d>,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&MeshMaterial3d<LitMaterial>>,
            Option<&RenderLayers>,
            Option<&MeshTag>,
            Has<NotShadowCaster>,
        ),
        Without<ReplayGhost>,
    >,
) {
    let Some(replay) = replay else {
        if !history.tracks.is_empty() || history.last_recorded.is_some() {
            for ghost in history.tracks.values().filter_map(|track| track.ghost) {
                commands.entity(ghost).try_despawn();
            }
            history.tracks.clear();
            history.last_recorded = None;
        }
        return;
    };
    let now = time.elapsed_secs_f64();
    let first = history.last_recorded.is_none();
    let previous = history.last_recorded.unwrap_or(now);
    for (entity, placed, visibility, mesh, standard, lit, layers, tag, no_shadow) in &meshes {
        if !on_the_world(layers) {
            continue;
        }
        let frame = Frame { at: now, placed: *placed, shown: visibility.is_none_or(|visibility| visibility.get()) };
        match history.tracks.get_mut(&entity) {
            Some(track) => {
                let moved = placed.is_changed() || track.frames.back().is_some_and(|last| last.shown != frame.shown);
                if replay.frozen || !moved {
                    continue;
                }
                // Held still since its last frame, so the tween starts from where it stood.
                if let Some(&last) = track.frames.back()
                    && last.at < previous
                {
                    track.frames.push_back(Frame { at: previous, ..last });
                }
                track.frames.push_back(frame);
            }
            None => {
                let material = match (standard, lit) {
                    (Some(standard), _) => Some(GhostMaterial::Standard(standard.0.clone())),
                    (None, Some(lit)) => Some(GhostMaterial::Lit(lit.0.clone())),
                    (None, None) => None,
                };
                let look = material.map(|material| Look {
                    mesh: mesh.0.clone(),
                    material,
                    layers: layers.cloned(),
                    tag: tag.cloned(),
                    no_shadow,
                });
                // On the first frame everything already stood there. A mesh may be put on an entity
                // a frame after its transform, as a replicated one is dressed.
                let born = (!first && (placed.is_added() || mesh.is_added())).then_some(now);
                history.tracks.insert(entity, Track { frames: VecDeque::from([frame]), born, look, ghost: None });
            }
        }
    }
    history.last_recorded = Some(now);
}

#[allow(clippy::type_complexity)]
fn play(
    mut commands: Commands,
    replay: Option<Res<Replay>>,
    mut history: ResMut<ReplayHistory>,
    mut live: Query<&mut GlobalTransform, Without<ReplayGhost>>,
    mut ghosts: Query<(&mut Transform, &mut GlobalTransform), With<ReplayGhost>>,
) {
    let Some(replay) = replay else { return };
    let view = replay.view;
    let ReplayHistory { tracks, overridden, .. } = &mut *history;
    tracks.retain(|entity, track| {
        let target = track.sample(view).filter(|(_, shown)| *shown).map_or_else(out_of_sight, |(placed, _)| placed);
        track.prune(view);
        if let Ok(mut placed) = live.get_mut(*entity) {
            if *placed != target {
                overridden.push((*entity, *placed));
                *placed = target;
            }
            return true;
        }
        let Some(look) = track.look.as_ref().filter(|_| track.last_at() > view) else {
            if let Some(ghost) = track.ghost {
                commands.entity(ghost).try_despawn();
            }
            return false;
        };
        match track.ghost.and_then(|ghost| ghosts.get_mut(ghost).ok()) {
            Some((mut transform, mut placed)) => {
                *transform = target.compute_transform();
                *placed = target;
            }
            None => {
                let mut ghost = commands.spawn((ReplayGhost, Mesh3d(look.mesh.clone()), target.compute_transform(), target, Name::new("Replay ghost")));
                match &look.material {
                    GhostMaterial::Standard(material) => ghost.insert(MeshMaterial3d(material.clone())),
                    GhostMaterial::Lit(material) => ghost.insert(MeshMaterial3d(material.clone())),
                };
                if let Some(layers) = &look.layers {
                    ghost.insert(layers.clone());
                }
                if let Some(tag) = look.tag.clone() {
                    ghost.insert(tag);
                }
                if look.no_shadow {
                    ghost.insert(NotShadowCaster);
                }
                track.ghost = Some(ghost.id());
            }
        }
        true
    });
}

fn put_back_the_live_transforms(mut history: ResMut<ReplayHistory>, mut live: Query<&mut GlobalTransform, Without<ReplayGhost>>) {
    for (entity, placed) in history.overridden.drain(..) {
        if let Ok(mut current) = live.get_mut(entity) {
            *current = placed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32) -> GlobalTransform {
        GlobalTransform::from_translation(Vec3::X * x)
    }

    fn track(born: Option<f64>, frames: &[(f64, f32)]) -> Track {
        Track {
            frames: frames.iter().map(|&(at, x)| Frame { at, placed: super::tests::at(x), shown: true }).collect(),
            born,
            look: None,
            ghost: None,
        }
    }

    /// Breaks if a sample between frames is not tweened, or a mesh born later is drawn early.
    #[test]
    fn a_track_is_tweened_between_frames_and_absent_before_it_was_born() {
        let moving = track(None, &[(1.0, 0.0), (2.0, 10.0)]);
        assert_eq!(moving.sample(0.5).unwrap().0, at(0.0), "before the first frame it stands where it was");
        assert!((moving.sample(1.25).unwrap().0.translation().x - 2.5).abs() < 1e-4);
        assert_eq!(moving.sample(9.0).unwrap().0, at(10.0));
        let born = track(Some(1.5), &[(1.5, 3.0)]);
        assert!(born.sample(1.0).is_none(), "drawn before it existed");
        assert_eq!(born.sample(1.5).unwrap().0, at(3.0));
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin, ReplayPlugin));
        app
    }

    #[derive(Resource)]
    struct LiveWas(f32);

    /// Breaks if a replayed transform is still in place when anything simulates, or if history is
    /// kept while no replay runs.
    #[test]
    fn the_live_transform_is_back_before_anything_simulates() {
        let mut app = app();
        app.insert_resource(LiveWas(0.0));
        app.add_systems(PreUpdate, |placed: Query<&GlobalTransform, With<Mesh3d>>, was: Res<LiveWas>| {
            for placed in &placed {
                assert_eq!(placed.translation().x, was.0, "a replayed transform reached PreUpdate");
            }
        });
        let body = app.world_mut().spawn((Mesh3d(Handle::default()), Transform::default())).id();
        app.update();
        assert!(app.world().resource::<ReplayHistory>().is_empty(), "recorded with no replay");

        // The view stays before every frame, so the body is drawn where it stood when the replay began.
        app.insert_resource(Replay { view: 0.0, frozen: false });
        for frame in 1..6 {
            app.world_mut().get_mut::<Transform>(body).unwrap().translation.x = frame as f32;
            app.update();
            app.world_mut().resource_mut::<LiveWas>().0 = frame as f32;
            let drawn = app.world().get::<GlobalTransform>(body).unwrap().translation().x;
            assert_eq!(drawn, 1.0, "frame {frame}: drawn at {drawn}");
        }
        app.world_mut().remove_resource::<Replay>();
        app.update();
        assert!(app.world().resource::<ReplayHistory>().is_empty(), "history kept after the replay");
        assert_eq!(app.world().get::<GlobalTransform>(body).unwrap().translation().x, 5.0);
    }
}
