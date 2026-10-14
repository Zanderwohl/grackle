//! Showing a prop in the editor's four viewports. On the way in, everything
//! that is not [`PropScene`] is hidden (not unloaded), the cameras are framed
//! (see [`super::camera`]) and the prop gets its own lights; all of it is
//! undone on the way out. See "Modelling a prop" in `CLAUDE.md`.
//!
//! The hiding rule names what to keep rather than what to hide, because
//! loose entities (grid bodies, corpses, projectiles) are not feature
//! entities.
//!
//! Rebuilt on [`PropEditor::generation`] rather than `Changed`, since
//! evaluation is expensive and egui marks changes on hover.

use core::time::Duration;

use bevy::platform::collections::HashMap;
use bevy::platform::time::Instant;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::prop::baked::{parts, SurfaceMaterials};
use crate::prop::figure::{clear_the_figure, pose_the_figure, refresh_the_figure, ScaleFigure};
use crate::prop::document::PropEditor;
use crate::prop::feature::Evaluated;

/// A set so the camera can order after the build without naming its systems.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropSceneSystems {
    Build,
}

#[derive(Component)]
pub struct PropStage;

/// On every root entity the prop editor owns; anything without it is hidden.
#[derive(Component)]
pub struct PropScene;

#[derive(Component)]
struct PropStageLight;

/// What was hidden and its previous value, restored exactly rather than set
/// to `Inherited`, since some things are hidden on purpose.
#[derive(Resource, Default)]
struct HiddenForModelling(HashMap<Entity, Visibility>);

/// The last evaluation, reused by the panels.
#[derive(Resource, Default)]
pub struct PropBuild {
    pub evaluated: Evaluated,
    built: Option<u64>,
    /// Decides whether the next rebuild is debounced.
    cost: Duration,
    /// The generation a deferred rebuild is waiting on, and since when.
    waiting: Option<(u64, Duration)>,
}

impl PropBuild {
    /// Whether the geometry on screen is behind the document.
    pub fn settling(&self) -> bool {
        self.waiting.is_some()
    }
}

/// Half a frame at 60 Hz. A cheaper evaluation rebuilds every frame.
const REBUILD_BUDGET: Duration = Duration::from_millis(8);

/// How long the document must be unchanged before an expensive prop is
/// rebuilt.
const REBUILD_DEBOUNCE: Duration = Duration::from_millis(120);

pub struct PropViewPlugin;

impl Plugin for PropViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PropBuild>()
            .init_resource::<SurfaceMaterials>()
            .init_resource::<HiddenForModelling>()
            .init_resource::<ScaleFigure>()
            .init_resource::<crate::prop::figure::FigureStage>()
            // Chained: framing reads the prop's bounds, which are stale from
            // a previous visit until the rebuild. Registered for both prop
            // modes; swapping runs exit and entry in one `StateTransition`, so
            // nothing flickers.
            .add_systems(
                OnEnter(AppMode::Prop),
                (light_the_stage, refresh_the_figure, pose_the_figure, set_the_world_aside, rebuild_the_prop)
                    .chain()
                    .in_set(PropSceneSystems::Build),
            )
            .add_systems(
                OnEnter(AppMode::Animation),
                (light_the_stage, refresh_the_figure, pose_the_figure, set_the_world_aside, rebuild_the_prop)
                    .chain()
                    .in_set(PropSceneSystems::Build),
            )
            .add_systems(
                OnExit(AppMode::Prop),
                (clear_the_stage, clear_the_figure, put_the_world_back),
            )
            .add_systems(
                OnExit(AppMode::Animation),
                (clear_the_stage, clear_the_figure, put_the_world_back),
            )
            .add_systems(
                Update,
                (
                    refresh_the_figure,
                    pose_the_figure,
                    // Every frame: map edits and replication land mid-session.
                    set_the_world_aside,
                    rebuild_the_prop,
                    // After the rebuild, which respawns the stage at the
                    // origin, so an animated prop does not snap home a frame.
                    carry_the_prop,
                    draw_the_bench,
                    ghost_the_operands,
                )
                    .chain()
                    .in_set(PropSceneSystems::Build)
                    .run_if(crate::common::app_mode::showing_a_prop),
            );
    }
}

/// Moves the prop where an animation holds it (identity while modelling), and
/// hides it when the staged figure is holding nothing. Separate from the
/// rebuild, which only runs on edits.
pub fn carry_the_prop(
    held: Res<crate::prop::figure::FigureWeapon>,
    figure: Option<Res<crate::prop::figure::FigureStage>>,
    mode: Option<Res<State<AppMode>>>,
    mut stage: Query<(&mut Transform, &mut Visibility), With<PropStage>>,
) {
    let staged = crate::prop::figure::stage_in_force(figure.as_deref(), mode.as_deref());
    let wanted = match staged.holding {
        true => Visibility::Inherited,
        false => Visibility::Hidden,
    };
    for (mut transform, mut visibility) in &mut stage {
        if *transform != held.0 {
            *transform = held.0;
        }
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

/// Roots only, because `Visibility` inherits and other systems hide
/// individual children. `Without<Node>` keeps the viewport labels (UI).
fn set_the_world_aside(
    mut hidden: ResMut<HiddenForModelling>,
    mut entities: Query<
        (Entity, &mut Visibility),
        (Without<ChildOf>, Without<Node>, Without<PropScene>),
    >,
) {
    for (entity, mut visibility) in &mut entities {
        // Hidden by someone else: not recorded, so leaving keeps it hidden.
        if *visibility == Visibility::Hidden {
            continue;
        }
        hidden.0.entry(entity).or_insert(*visibility);
        *visibility = Visibility::Hidden;
    }
}

fn put_the_world_back(
    mut hidden: ResMut<HiddenForModelling>,
    mut entities: Query<&mut Visibility>,
) {
    for (entity, was) in hidden.0.drain() {
        // May have been despawned meanwhile.
        if let Ok(mut visibility) = entities.get_mut(entity) {
            *visibility = was;
        }
    }
}

fn light_the_stage(mut commands: Commands) {
    // A fill light too, or the underside is a silhouette.
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
    build.built = None;
}

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

    // Debounce only props whose last evaluation was expensive. Gizmos draw
    // from the document, so handles keep following the pointer meanwhile.
    if build.cost > REBUILD_BUDGET && !stage.is_empty() {
        let now = time.elapsed();
        match build.waiting {
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
    // Down to the rollback bar; everything downstream reads this.
    build.evaluated = crate::prop::feature::evaluate(editor.shown());
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

/// Origin, axes, and a 1 m floor grid of 10 cm cells for judging size.
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

    // An eaten feature has no body and is not outlined.
    for picked in editor.selection() {
        let Some((_, solid)) = build.evaluated.bodies.iter().find(|(id, _)| *id == picked) else {
            continue;
        };
        let Some((min, max)) = solid.bounds() else { continue };
        let colour = match editor.selected == Some(picked) {
            true => Color::srgb(1.0, 0.8, 0.2),
            false => Color::srgba(1.0, 0.8, 0.2, 0.35),
        };
        gizmos.cube(Transform::from_translation((min + max) / 2.0).with_scale(max - min), colour);
    }
}

#[derive(Default)]
struct GhostCache {
    key: Option<(u64, crate::prop::feature::PropFeatureId)>,
    edges: Vec<(Vec3, Vec3)>,
}

/// Wireframe of the selected feature's operands as they stood before it ran.
/// Cached on generation and selection, since it re-evaluates the list up to
/// that feature.
fn ghost_the_operands(
    mut gizmos: Gizmos,
    editor: Res<PropEditor>,
    mut cache: Local<GhostCache>,
) {
    let Some(selected) = editor.selected else {
        cache.key = None;
        cache.edges.clear();
        return;
    };
    let key = (editor.generation(), selected);
    if cache.key != Some(key) {
        cache.key = Some(key);
        cache.edges.clear();
        let features = editor.shown();
        if let Some(at) = features.iter().position(|feature| feature.id == selected) {
            let operands = features[at].op.consumes();
            if !operands.is_empty() {
                let before = crate::prop::feature::evaluate(&features[..at]);
                for (_, solid) in before.bodies.iter().filter(|(id, _)| operands.contains(id)) {
                    for polygon in solid.polygons() {
                        let v = &polygon.vertices;
                        for i in 0..v.len() {
                            cache.edges.push((v[i], v[(i + 1) % v.len()]));
                        }
                    }
                }
            }
        }
    }
    for (a, b) in &cache.edges {
        gizmos.line(*a, *b, Color::srgba(0.5, 0.8, 1.0, 0.25));
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// Covers a loose root, a child, the prop scene, a UI node and something
    /// already hidden; fails if any is hidden or restored wrongly.
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

        assert_eq!(seen(&world, loose), Visibility::Inherited);
        assert_eq!(seen(&world, parent), Visibility::Visible);
        assert_eq!(
            seen(&world, already_hidden),
            Visibility::Hidden,
            "something hidden before the mode opened was turned on by leaving it",
        );
    }

    /// Fails if a second sweep overwrites what the first recorded.
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
