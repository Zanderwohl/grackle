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
//! it again because egui reported a hover would be felt. And the generation is
//! only the cue to *look*: what decides is whether the features shown differ
//! from the ones last built, because most of a document — the hold, the
//! viewmodel, the clips — is not geometry, and dragging a grip must not re-run
//! the kernel every frame.

use core::time::Duration;

use bevy::platform::collections::HashMap;
use bevy::platform::time::Instant;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::prop::baked::{parts, SurfaceMaterials};
use crate::prop::figure::{clear_the_figure, pose_the_figure, refresh_the_figure, ScaleFigure};
use crate::prop::document::PropEditor;
use crate::prop::feature::{Evaluated, PropFeature};

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
    /// The generation last looked at, so an unchanged document costs nothing
    /// to check.
    built: Option<u64>,
    /// The features `evaluated` was built from — **the** key, not the
    /// generation. Every write to the document bumps the generation, and most
    /// of a document is not geometry: a counter bumped only for geometry
    /// would go stale the day something forgot to bump it, whereas the
    /// evaluator's own input cannot. A prop is a few kilobytes, so the
    /// comparison is cheap beside the kernel it saves.
    input: Option<Vec<PropFeature>>,
    /// How long the last evaluation took, which is what decides whether the
    /// next one waits.
    cost: Duration,
    /// The features a deferred rebuild is waiting on, and since when. Keyed on
    /// the features for the same reason: a hold dragged while an expensive
    /// prop settles must not keep restarting the clock.
    waiting: Option<(Vec<PropFeature>, Duration)>,
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
            .init_resource::<crate::prop::figure::FigureStage>()
            // Chained, and the order is the point: framing reads the bounds
            // of the prop, so the prop has to have been built first. On the
            // way in the last build belongs to a previous visit or to no
            // visit at all, and framing on stale bounds puts the cameras
            // somewhere plausible and wrong.
            // Registered for **both** prop modes rather than for `Prop` alone.
            // They are one scene asked two questions, so swapping between them
            // runs an exit and an entry — both in the same `StateTransition`,
            // before anything is drawn, so there is no flicker and the world
            // is set aside again before a frame of it could be seen. A rebuild
            // per swap is real work, but a mode swap is not a frame.
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
                    // After the rebuild, which respawns the stage at the
                    // origin: a weapon mid-deploy must not snap home for one
                    // frame every time the geometry changes.
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

/// Put the prop where a performance says it is being held.
///
/// Identity on the modelling surface — the prop is pinned to its own origin,
/// which is where its feature gizmos are drawn — and moved only while
/// something is animating it. Its own system rather than part of the rebuild,
/// because the mesh is rebuilt on `PropEditor::generation` and a weapon that
/// only moved when somebody edited the geometry would be a weapon that does
/// not move.
///
/// And hidden when the figure is staged holding nothing: a prop floating at
/// the origin beside a body with empty hands would be a prop nobody is
/// holding, which is not a thing the game ever draws.
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
    build.input = None;
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
    // Down to the rollback bar, and everything downstream — the meshes, the
    // picking, the framing, which bodies the toolbar offers — reads this, so
    // rolled back is one consistent picture of the prop as it stood there.
    let shown = editor.shown();

    // The document moved and the geometry did not: a grip, a viewmodel, a
    // clip, a name. The stage on screen is already right.
    if !stage.is_empty() && build.input.as_deref() == Some(shown) {
        build.built = Some(generation);
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
    // The primary bright and the rest of the selection dimmer, matching the
    // tree's two highlights.
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

/// The ghost's edges, and what they were drawn for.
#[derive(Default)]
struct GhostCache {
    /// The generation and selection last looked at — the cue to compare.
    key: Option<(u64, crate::prop::feature::PropFeatureId)>,
    /// The selection and the features down to it that the edges came from,
    /// which is what decides — the same rule as [`PropBuild`]'s, so dragging a
    /// grip does not re-evaluate the list up to the selected boolean.
    input: Option<(crate::prop::feature::PropFeatureId, Vec<PropFeature>)>,
    edges: Vec<(Vec3, Vec3)>,
}

/// What a selected boolean, mirror or repaint was given to work on, drawn as
/// faint wireframe where it stood before the feature ate it.
///
/// A subtract that removes nothing visible is the hardest thing in the editor
/// to debug from the result alone: the tool might be in the wrong place, the
/// wrong size, or the wrong body. Showing it answers all three at once.
///
/// Re-evaluates the list *up to* the feature, so it is cached on the
/// selection and the features down to it, and recomputed only when one of
/// them moves — not every frame something is merely selected, nor every frame
/// of a drag that is not geometry.
fn ghost_the_operands(
    mut gizmos: Gizmos,
    editor: Res<PropEditor>,
    mut cache: Local<GhostCache>,
) {
    let Some(selected) = editor.selected else {
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

    /// Most of a document is not geometry. Dragging a grip writes the document
    /// every frame, and re-running the kernel and re-uploading every mesh for
    /// it is the cost the generation was meant to save — so a hold edit must
    /// leave the stage alone while a feature edit, an undo and the rollback
    /// bar still rebuild it.
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
