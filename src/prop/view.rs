//! Showing a prop in the editor's four viewports. On the way in, everything
//! that is not [`PropScene`] is hidden (not unloaded), the cameras are framed
//! (see [`super::camera`]) and the prop gets its own lights; all of it is
//! undone on the way out. See "Modelling a prop" in `CLAUDE.md`.
//!
//! The hiding rule names what to keep rather than what to hide, because
//! loose entities (grid bodies, corpses, projectiles) are not feature
//! entities. A root on no layer the viewports draw is kept too: it is not in
//! the way, and hiding it would blank whatever camera does draw it.
//!
//! Rebuilt on [`PropEditor::generation`] rather than `Changed`, since
//! evaluation is expensive and egui marks changes on hover. The generation is
//! only the cue to look: it rebuilds only if the shown features differ.

use core::time::Duration;

use bevy::camera::visibility::RenderLayers;
use bevy::platform::collections::HashMap;
use bevy::platform::time::Instant;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::editor::multicam::viewport_layers;
use crate::prop::baked::{parts, SurfaceMaterials};
use crate::prop::figure::{clear_the_figure, pose_the_figure, refresh_the_figure, ScaleFigure};
use crate::prop::document::{Pick, PropEditor};
use crate::prop::gizmo::HoveredPick;
use crate::prop::plane::draw_square;
use crate::tool::tool_helpers::dashed_line;
use crate::prop::reference::Located;
use crate::prop::feature::{Evaluated, PropFeature};

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
    /// The features `evaluated` was built from: the real key, since most of a
    /// document is not geometry and a geometry-only counter could go stale.
    input: Option<Vec<PropFeature>>,
    /// Decides whether the next rebuild is debounced.
    cost: Duration,
    /// The features a deferred rebuild waits on, and since when, so a hold
    /// dragged while a prop settles does not restart the clock.
    waiting: Option<(Vec<PropFeature>, Duration)>,
}

impl PropBuild {
    /// Whether the geometry on screen is behind the document.
    pub fn settling(&self) -> bool {
        self.waiting.is_some()
    }

    /// The editor generation `evaluated` stands for; a cue that it changed.
    pub fn built(&self) -> Option<u64> {
        self.built
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
        (Entity, &mut Visibility, Option<&RenderLayers>),
        (Without<ChildOf>, Without<Node>, Without<PropScene>),
    >,
) {
    let viewports = viewport_layers();
    for (entity, mut visibility, layers) in &mut entities {
        if layers.is_some_and(|layers| !layers.intersects(&viewports)) {
            continue;
        }
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
    build.input = None;
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
    // Down to the rollback bar; everything downstream reads this.
    let shown = editor.shown();

    // The document moved and the geometry did not: the stage is already right.
    if !stage.is_empty() && build.input.as_deref() == Some(shown) {
        build.built = Some(generation);
        build.waiting = None;
        return;
    }

    // Debounce only props whose last evaluation was expensive. Gizmos draw
    // from the document, so handles keep following the pointer meanwhile.
    if build.cost > REBUILD_BUDGET && !stage.is_empty() {
        let now = time.elapsed();
        let since = match &build.waiting {
            Some((waiting_for, since)) if waiting_for.as_slice() == shown => Some(*since),
            _ => None,
        };
        match since {
            Some(since) if now.saturating_sub(since) >= REBUILD_DEBOUNCE => {}
            Some(_) => return,
            // Still moving, or only now started: the clock runs from here.
            None => {
                build.waiting = Some((shown.to_vec(), now));
                return;
            }
        }
    }
    build.waiting = None;

    build.built = Some(generation);
    // Bevy's `Instant`, not the standard library's, which panics on wasm.
    let started = Instant::now();
    build.evaluated = crate::prop::feature::evaluate(shown);
    build.cost = started.elapsed();
    build.input = Some(shown.to_vec());

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
fn draw_the_bench(
    mut gizmos: Gizmos,
    editor: Res<PropEditor>,
    build: Res<PropBuild>,
    hovered: Res<HoveredPick>,
) {
    gizmos.grid(
        Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        UVec2::splat(10),
        Vec2::splat(0.1),
        Color::srgba(1.0, 1.0, 1.0, 0.08),
    );
    gizmos.line(Vec3::ZERO, Vec3::X * 0.25, Color::srgb(0.9, 0.3, 0.3));
    gizmos.line(Vec3::ZERO, Vec3::Y * 0.25, Color::srgb(0.3, 0.9, 0.3));
    gizmos.line(Vec3::ZERO, Vec3::Z * 0.25, Color::srgb(0.3, 0.5, 0.9));

    let primary = editor.primary();
    let marker = build.evaluated.bounds().map_or(MIN_MARKER, |(min, max)| (max - min).length() * MARKER_SIZE).max(MIN_MARKER);
    let picked_colour = |pick: &Pick| match Some(pick) == primary {
        true => PICKED,
        false => PICKED.with_alpha(0.5),
    };
    for pick in editor.picks() {
        match pick {
            // An eaten feature has no body and is not outlined.
            Pick::Body(picked) => {
                let Some((_, solid)) = build.evaluated.bodies.iter().find(|(id, _)| id == picked) else {
                    continue;
                };
                let Some((min, max)) = solid.bounds() else { continue };
                gizmos.cube(Transform::from_translation((min + max) / 2.0).with_scale(max - min), picked_colour(pick));
            }
            Pick::Geom(reference) => {
                if let Some(at) = build.evaluated.locate(reference) {
                    draw_located(&mut gizmos, at, marker, picked_colour(pick));
                }
            }
            // Drawn with every other plane, below.
            Pick::Plane(_) => {}
        }
    }
    if let Some(Pick::Geom(reference)) = &hovered.0 {
        if let Some(at) = build.evaluated.locate(reference) {
            draw_located(&mut gizmos, at, marker * 0.8, HOVERED);
        }
    }
    let picked_features = editor.picked_features(&build.evaluated);
    for placed in &build.evaluated.sketches {
        let colour = match picked_features.contains(&placed.id) {
            true => PICKED,
            false => SKETCH,
        };
        for (a, b, construction) in placed.world_lines() {
            match construction {
                true => dashed_line(&mut gizmos, a, b, colour, marker * 2.0, marker),
                false => {
                    gizmos.line(a, b, colour);
                }
            }
        }
    }
    for datum in &build.evaluated.planes {
        let pick = Pick::Plane(datum.id);
        let colour = if editor.picks().contains(&pick) {
            picked_colour(&pick)
        } else if hovered.0.as_ref() == Some(&pick) {
            HOVERED
        } else if datum.id.feature().is_none() {
            // Always there, so quieter than planes somebody made.
            PLANE.with_alpha(0.35)
        } else {
            PLANE
        };
        draw_square(&mut gizmos, &datum.corners, colour);
    }
}

const PICKED: Color = Color::srgb(1.0, 0.8, 0.2);
const HOVERED: Color = Color::srgb(0.5, 0.9, 1.0);
const PLANE: Color = Color::srgb(0.55, 0.55, 0.55);
const SKETCH: Color = Color::srgb(0.92, 0.92, 0.98);

/// A corner's marker as a fraction of the prop's size. Not camera-relative,
/// for the same reason a feature's handles are not: four viewports.
const MARKER_SIZE: f32 = 0.012;
/// Metres.
const MIN_MARKER: f32 = 0.002;

fn draw_located(gizmos: &mut Gizmos, at: Located, size: f32, colour: Color) {
    match at {
        Located::Face { points, .. } => {
            for i in 0..points.len() {
                gizmos.line(points[i], points[(i + 1) % points.len()], colour);
            }
            let centre = points.iter().sum::<Vec3>() / points.len() as f32;
            gizmos.sphere(Isometry3d::from_translation(centre), size * 0.5, colour);
        }
        Located::Point(p) => {
            gizmos.sphere(Isometry3d::from_translation(p), size, colour);
        }
        Located::Edge(a, b) => {
            gizmos.line(a, b, colour);
            gizmos.sphere(Isometry3d::from_translation((a + b) / 2.0), size * 0.5, colour);
        }
    }
}

#[derive(Default)]
struct GhostCache {
    /// The generation and selection last looked at: the cue to compare.
    key: Option<(u64, crate::prop::feature::PropFeatureId)>,
    /// What the edges were drawn from, which decides, as in [`PropBuild`].
    input: Option<(crate::prop::feature::PropFeatureId, Vec<PropFeature>)>,
    edges: Vec<(Vec3, Vec3)>,
}

/// Wireframe of the selected feature's operands as they stood before it ran.
/// Cached on the selection and the features up to it, since it re-evaluates
/// the list up to that feature.
fn ghost_the_operands(
    mut gizmos: Gizmos,
    editor: Res<PropEditor>,
    build: Res<PropBuild>,
    mut cache: Local<GhostCache>,
) {
    let Some(selected) = editor.primary_feature(&build.evaluated) else {
        cache.key = None;
        cache.input = None;
        cache.edges.clear();
        return;
    };
    let key = (editor.generation(), selected);
    if cache.key != Some(key) {
        cache.key = Some(key);
        let features = editor.shown();
        let at = features.iter().position(|feature| feature.id == selected);
        let upto = at.map_or(&[][..], |at| &features[..=at]);
        let unchanged = cache
            .input
            .as_ref()
            .is_some_and(|(was, built_from)| *was == selected && built_from.as_slice() == upto);
        if !unchanged {
            cache.input = Some((selected, upto.to_vec()));
            cache.edges.clear();
        }
        if let Some(at) = at.filter(|_| !unchanged) {
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
        let elsewhere = world
            .spawn((Visibility::Inherited, RenderLayers::layer(crate::common::render_layers::THUMBNAIL)))
            .id();

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
        assert_eq!(seen(&world, elsewhere), Visibility::Inherited, "hid what no viewport draws");

        world.run_system_once(put_the_world_back).unwrap();

        assert_eq!(seen(&world, loose), Visibility::Inherited);
        assert_eq!(seen(&world, parent), Visibility::Visible);
        assert_eq!(
            seen(&world, already_hidden),
            Visibility::Hidden,
            "something hidden before the mode opened was turned on by leaving it",
        );
    }

    /// Fails if a hold edit rebuilds the stage, or if a feature edit, undo or
    /// the rollback bar does not.
    #[test]
    fn only_the_geometry_rebuilds_the_stage() {
        use crate::prop::feature::FeatureOp;

        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.init_resource::<PropBuild>();
        world.init_resource::<SurfaceMaterials>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<Assets<Mesh>>();
        let mut editor = PropEditor::default();
        let first = editor.edit(|doc| doc.push(FeatureOp::default()));
        world.insert_resource(editor);

        let build = |world: &mut World| {
            // Cheap by fiat, so the debounce is not what is being measured.
            world.resource_mut::<PropBuild>().cost = Duration::ZERO;
            world.run_system_once(rebuild_the_prop).unwrap();
            let mut stage = world.query_filtered::<Entity, With<PropStage>>();
            let stages: Vec<Entity> = stage.iter(world).collect();
            assert_eq!(stages.len(), 1, "one stage, always");
            stages[0]
        };
        let built = build(&mut world);
        assert!(!world.resource::<PropBuild>().evaluated.bodies.is_empty());

        let generation = world.resource::<PropEditor>().generation();
        {
            let mut editor = world.resource_mut::<PropEditor>();
            editor.begin_gesture();
            editor.doc_mut().hold.grip[0] += 0.1;
            editor.doc_mut().viewmodel = Default::default();
            editor.end_gesture();
        }
        assert_ne!(world.resource::<PropEditor>().generation(), generation);
        assert_eq!(build(&mut world), built, "moving the grip rebuilt the prop");

        world.resource_mut::<PropEditor>().edit(|doc| doc.push(FeatureOp::default()));
        let after_edit = build(&mut world);
        assert_ne!(after_edit, built, "a new feature did not rebuild the prop");

        world.resource_mut::<PropEditor>().roll_back_to(Some(first));
        let rolled_back = build(&mut world);
        assert_ne!(rolled_back, after_edit, "moving the rollback bar did not rebuild");

        world.resource_mut::<PropEditor>().roll_back_to(None);
        let whole = build(&mut world);
        assert_ne!(whole, rolled_back, "taking the bar away did not rebuild");

        world.resource_mut::<PropEditor>().undo();
        let undone = build(&mut world);
        assert_ne!(undone, whole, "undo did not rebuild");
        assert_eq!(world.resource::<PropBuild>().input.as_deref().map(<[_]>::len), Some(1));
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
