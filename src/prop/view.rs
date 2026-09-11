//! Showing a prop in the same viewports the map is edited in.
//!
//! Reusing the editor's four cameras is the whole reason this can be a mode
//! rather than a second program. Three things happen at the boundary, all
//! reversible:
//!
//! - **Everything that is not the prop is hidden, not unloaded.** A prop is
//!   centimetres across and a map is tens of metres, so leaving the map in
//!   puts the prop inside a wall. Hiding is a `Visibility`, so the open
//!   blueprint is untouched.
//!
//!   **The rule asks what is *not* the prop scene** rather than naming the
//!   kinds of thing that might be in the way. Naming them was the first
//!   attempt and was wrong within one map: the animation grid's bodies stand
//!   loose in the world (`OfGrid` replaced `ChildOf` so they could be
//!   replicated), so hiding feature entities left sixty people standing around
//!   the weapon. Corpses, projectiles and tracers are loose the same way.
//! - **The cameras are framed on the prop and put back afterwards** — see
//!   [`super::camera`].
//! - **The prop gets its own light.** The map's lights are feature entities
//!   and went out with everything else.
//!
//! The prop is rebuilt on [`PropEditor::generation`] rather than on anything
//! being `Changed`: evaluating is a boolean kernel doing real work, and doing
//! it again because egui reported a hover would be felt.

use core::time::Duration;

use bevy::platform::collections::HashMap;
use bevy::platform::time::Instant;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::prop::baked::{parts, SurfaceMaterials};
use crate::prop::figure::{clear_the_figure, pose_the_figure, refresh_the_figure, ScaleFigure};
use crate::prop::document::PropEditor;
use crate::prop::feature::Evaluated;

/// Standing the prop up. A set rather than named systems, for the reason
/// `BakeSystems` is one: the camera frames itself on the prop's bounds from
/// another module, and must not have to name what builds them.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropSceneSystems {
    Build,
}

/// The root the prop's meshes hang off. One entity, cleared and refilled.
#[derive(Component)]
pub struct PropStage;

/// On every root entity the prop editor owns, and the whole of what keeps it
/// visible. Forgetting one is obvious: it is invisible immediately.
#[derive(Component)]
pub struct PropScene;

/// The prop editor's own light, so it can be taken away again on the way out.
#[derive(Component)]
struct PropStageLight;

/// What was hidden on the way in, and what it was before.
///
/// Put back rather than blanket-set to `Inherited`: plenty here is hidden on
/// purpose — a spawn point's preview during a round, the body you are looking
/// out of — and an exit that made everything visible would break them.
#[derive(Resource, Default)]
struct HiddenForModelling(HashMap<Entity, Visibility>);

/// The last evaluation, kept so the panels can show the problem list and the
/// triangle count without running the kernel a second time.
#[derive(Resource, Default)]
pub struct PropBuild {
    pub evaluated: Evaluated,
    /// The generation this was built from, so the rebuild knows it is stale.
    built: Option<u64>,
    /// How long the last evaluation took, which is what decides whether the
    /// next one waits.
    cost: Duration,
    /// The generation a deferred rebuild is waiting on, and since when.
    waiting: Option<(u64, Duration)>,
}

impl PropBuild {
    /// Whether the geometry on screen is behind the document. The panel says
    /// so: a viewport that has stopped following a drag looks exactly like one
    /// that has stopped working.
    pub fn settling(&self) -> bool {
        self.waiting.is_some()
    }
}

/// How long an evaluation may take before the next one is made to wait. Half a
/// frame at 60 Hz; under it, rebuilding per frame costs nothing anybody sees.
const REBUILD_BUDGET: Duration = Duration::from_millis(8);

/// How quiet the document has to go before an expensive prop is rebuilt.
///
/// Short enough to feel like a pause at the end of a drag rather than a delay,
/// long enough that a drag does not sneak a rebuild in between two frames of
/// mouse movement.
const REBUILD_DEBOUNCE: Duration = Duration::from_millis(120);

pub struct PropViewPlugin;

impl Plugin for PropViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PropBuild>()
            .init_resource::<SurfaceMaterials>()
            .init_resource::<HiddenForModelling>()
            .init_resource::<ScaleFigure>()
            // Chained, and the order is the point: framing reads the bounds
            // of the prop, so the prop has to have been built first. On the
            // way in the last build belongs to a previous visit or to no
            // visit at all, and framing on stale bounds puts the cameras
            // somewhere plausible and wrong.
            .add_systems(
                OnEnter(AppMode::Prop),
                (light_the_stage, refresh_the_figure, pose_the_figure, set_the_world_aside, rebuild_the_prop)
                    .chain()
                    .in_set(PropSceneSystems::Build),
            )
            .add_systems(
                OnExit(AppMode::Prop),
                (clear_the_stage, clear_the_figure, put_the_world_back),
            )
            .add_systems(
                Update,
                (
                    refresh_the_figure,
                    // After the rig exists and before anything draws it: the
                    // figure's whole pose comes from the document's hold.
                    pose_the_figure,
                    // Every frame rather than only on the way in: a map edit
                    // can land mid-session — `sync_entities` is deliberately
                    // ungated — and a room that appeared while somebody was
                    // modelling would appear around them. So would a corpse
                    // replicated from a server that is still playing.
                    set_the_world_aside,
                    rebuild_the_prop,
                    draw_the_bench,
                )
                    .chain()
                    .in_set(PropSceneSystems::Build)
                    .run_if(in_state(AppMode::Prop)),
            );
    }
}

/// Hide every root entity that is not part of the prop scene.
///
/// **Roots only**, because `Visibility` inherits: hiding a body hides its bone
/// meshes. Walking children too would fight the systems that legitimately hide
/// one part of a visible thing.
///
/// **`Without<Node>`**, because Bevy UI carries `Visibility` too and the
/// viewport labels are UI — chrome rather than scene.
fn set_the_world_aside(
    mut hidden: ResMut<HiddenForModelling>,
    mut entities: Query<
        (Entity, &mut Visibility),
        (Without<ChildOf>, Without<Node>, Without<PropScene>),
    >,
) {
    for (entity, mut visibility) in &mut entities {
        // Already invisible, and not by us: leave it, and do not record it, so
        // the way out does not turn it on.
        if *visibility == Visibility::Hidden {
            continue;
        }
        hidden.0.entry(entity).or_insert(*visibility);
        *visibility = Visibility::Hidden;
    }
}

/// Put back exactly what was there, for everything that still exists.
fn put_the_world_back(
    mut hidden: ResMut<HiddenForModelling>,
    mut entities: Query<&mut Visibility>,
) {
    for (entity, was) in hidden.0.drain() {
        // A despawn while somebody was modelling is ordinary: a round can be
        // running on a server the whole time.
        if let Ok(mut visibility) = entities.get_mut(entity) {
            *visibility = was;
        }
    }
}

fn light_the_stage(mut commands: Commands) {
    // Two, because a key light alone leaves the underside of a receiver as a
    // silhouette — exactly the part somebody is trying to look at.
    commands.spawn((
        PropStageLight,
        PropScene,
        DirectionalLight { illuminance: 6_000.0, shadow_maps_enabled: false, ..default() },
        Transform::from_xyz(2.0, 4.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        PropStageLight,
        PropScene,
        DirectionalLight { illuminance: 2_500.0, shadow_maps_enabled: false, ..default() },
        Transform::from_xyz(-3.0, -1.0, -2.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn clear_the_stage(
    mut commands: Commands,
    stage: Query<Entity, With<PropStage>>,
    lights: Query<Entity, With<PropStageLight>>,
    mut build: ResMut<PropBuild>,
) {
    for entity in stage.iter().chain(lights.iter()) {
        commands.entity(entity).despawn();
    }
    // The stage it would otherwise consider current has just been despawned.
    build.built = None;
}

/// Evaluate the document and put the result in the world, if it has moved on.
fn rebuild_the_prop(
    mut commands: Commands,
    editor: Res<PropEditor>,
    time: Res<Time>,
    mut build: ResMut<PropBuild>,
    mut surfaces: ResMut<SurfaceMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    stage: Query<Entity, With<PropStage>>,
) {
    let generation = editor.generation();
    if build.built == Some(generation) && !stage.is_empty() {
        build.waiting = None;
        return;
    }

    // **An expensive prop settles; a cheap one keeps up**, decided by the last
    // evaluation's own cost so there is nothing to tune per prop. Waiting
    // unconditionally would make a three-box prop feel sticky for no reason.
    //
    // The gizmos draw from the *document*, not from this, so a handle keeps
    // following the pointer while the geometry catches up — which is what
    // makes the wait read as settling rather than as lag.
    if build.cost > REBUILD_BUDGET && !stage.is_empty() {
        let now = time.elapsed();
        match build.waiting {
            // Still moving: start the clock again from this change.
            Some((waiting_for, _)) if waiting_for != generation => {
                build.waiting = Some((generation, now));
                return;
            }
            Some((_, since)) if now.saturating_sub(since) < REBUILD_DEBOUNCE => return,
            None => {
                build.waiting = Some((generation, now));
                return;
            }
            _ => {}
        }
    }
    build.waiting = None;

    build.built = Some(generation);
    // Bevy's `Instant`, not the standard library's, which panics on wasm.
    let started = Instant::now();
    build.evaluated = editor.doc().evaluate();
    build.cost = started.elapsed();

    for entity in &stage {
        commands.entity(entity).despawn();
    }

    let mut root = commands.spawn((
        PropStage,
        PropScene,
        Transform::IDENTITY,
        Visibility::Inherited,
        Name::new("Prop"),
    ));
    root.with_children(|parent| {
        for (surface, mesh) in parts(&build.evaluated) {
            let material = surfaces.get(surface, &mut materials);
            parent.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material)));
        }
    });
}

/// The origin, the axes and a ruled floor. Ten-centimetre cells over a metre:
/// a prop has no room around it to judge size against, so the grid is the
/// ruler.
fn draw_the_bench(mut gizmos: Gizmos, editor: Res<PropEditor>, build: Res<PropBuild>) {
    gizmos.grid(
        Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        UVec2::splat(10),
        Vec2::splat(0.1),
        Color::srgba(1.0, 1.0, 1.0, 0.08),
    );
    gizmos.line(Vec3::ZERO, Vec3::X * 0.25, Color::srgb(0.9, 0.3, 0.3));
    gizmos.line(Vec3::ZERO, Vec3::Y * 0.25, Color::srgb(0.3, 0.9, 0.3));
    gizmos.line(Vec3::ZERO, Vec3::Z * 0.25, Color::srgb(0.3, 0.5, 0.9));

    // A feature eaten by a later boolean has no body to outline, and drawing
    // its ingredients would show geometry that is not in the prop.
    let Some(selected) = editor.selected else { return };
    let Some((_, solid)) = build.evaluated.bodies.iter().find(|(id, _)| *id == selected) else {
        return;
    };
    let Some((min, max)) = solid.bounds() else { return };
    gizmos.cube(
        Transform::from_translation((min + max) / 2.0).with_scale(max - min),
        Color::srgb(1.0, 0.8, 0.2),
    );
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// The rule in one picture. Each of these five entities is a case that got
    /// this wrong at some point, or would have:
    ///
    /// - a **loose body**, like the animation grid's — not a feature entity,
    ///   which is what the first version of this missed
    /// - a **child**, which inherits and must not be touched separately
    /// - the **prop scene** itself, which is the entire point of the mode
    /// - a **UI node**, because Bevy UI carries `Visibility` too and the
    ///   viewport labels are UI
    /// - something **already hidden**, which must still be hidden afterwards
    #[test]
    fn everything_but_the_prop_is_hidden_and_then_put_back_as_it_was() {
        let mut world = World::new();
        world.init_resource::<HiddenForModelling>();

        let loose = world.spawn(Visibility::Inherited).id();
        let parent = world.spawn(Visibility::Visible).id();
        let child = world.spawn((Visibility::Inherited, ChildOf(parent))).id();
        let prop = world.spawn((Visibility::Inherited, PropScene)).id();
        let label = world.spawn((Visibility::Inherited, Node::default())).id();
        let already_hidden = world.spawn(Visibility::Hidden).id();

        world.run_system_once(set_the_world_aside).unwrap();

        let seen = |world: &World, entity: Entity| *world.get::<Visibility>(entity).unwrap();
        assert_eq!(seen(&world, loose), Visibility::Hidden, "a loose body stayed in the viewport");
        assert_eq!(seen(&world, parent), Visibility::Hidden);
        assert_eq!(
            seen(&world, child),
            Visibility::Inherited,
            "a child was hidden in its own right; it should follow its parent",
        );
        assert_eq!(seen(&world, prop), Visibility::Inherited, "the prop hid itself");
        assert_eq!(seen(&world, label), Visibility::Inherited, "the viewport labels went out");

        world.run_system_once(put_the_world_back).unwrap();

        // Back to what each *was*, not blanket-visible: `parent` was
        // explicitly `Visible` and `already_hidden` was hidden by somebody
        // else, and an exit that levelled both would be an exit that broke
        // whatever set them.
        assert_eq!(seen(&world, loose), Visibility::Inherited);
        assert_eq!(seen(&world, parent), Visibility::Visible);
        assert_eq!(
            seen(&world, already_hidden),
            Visibility::Hidden,
            "something hidden before the mode opened was turned on by leaving it",
        );
    }

    /// Entities appear while somebody is modelling — a map edit lands, a
    /// server replicates a corpse — so the sweep runs every frame, and running
    /// it twice must not lose what the first pass recorded.
    #[test]
    fn a_second_pass_does_not_forget_what_the_first_one_saved() {
        let mut world = World::new();
        world.init_resource::<HiddenForModelling>();
        let was_visible = world.spawn(Visibility::Visible).id();

        world.run_system_once(set_the_world_aside).unwrap();
        let latecomer = world.spawn(Visibility::Inherited).id();
        world.run_system_once(set_the_world_aside).unwrap();

        assert_eq!(*world.get::<Visibility>(latecomer).unwrap(), Visibility::Hidden);

        world.run_system_once(put_the_world_back).unwrap();
        assert_eq!(
            *world.get::<Visibility>(was_visible).unwrap(),
            Visibility::Visible,
            "the second pass recorded it as hidden and put back the wrong answer",
        );
        assert_eq!(*world.get::<Visibility>(latecomer).unwrap(), Visibility::Inherited);
    }
}
