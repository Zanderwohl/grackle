//! Showing a prop in the same viewports the map is edited in.
//!
//! The prop editor reuses the editor's four cameras rather than standing up
//! its own, which is the whole reason it can exist as a mode rather than as a
//! second program: a mapper already knows how to fly the freecam and read the
//! three orthographic views, and a modelling tool that made them relearn it
//! would be a modelling tool nobody opens.
//!
//! Three things have to happen at the boundary, and all three are reversible:
//!
//! - **Everything that is not the prop is hidden, not unloaded.** A prop is
//!   centimetres across and a map is tens of metres; leaving the map in would
//!   put the prop inside a wall. Hiding is a `Visibility`, so coming back is
//!   instant and nothing about the open blueprint is touched — which matters,
//!   because opening the prop editor mid-map must not cost the map.
//!
//!   **The rule asks what is *not* the prop scene**, rather than naming the
//!   kinds of thing that might be in the way. Naming them was the first
//!   attempt and it was wrong within one map: the animation grid's bodies
//!   stand in the world rather than inside the grid feature — `OfGrid`
//!   replaced `ChildOf` so they could be replicated — so a rule that hid
//!   feature entities left sixty people standing around the weapon. Corpses,
//!   projectiles, tracers and emitters are all loose in the same way, and each
//!   would have had to be remembered. Asking about absence has no such list to
//!   forget, and a new kind of loose entity is hidden the day it is written.
//! - **The cameras are framed on the prop and put back afterwards.** Their
//!   transforms and projections are saved on the way in. Framing without
//!   saving would leave somebody's carefully placed viewpoint pointing at a
//!   room's corner after a detour into a weapon file.
//! - **The prop gets its own light.** The map's lights are feature entities,
//!   so they went out with everything else, and a metal surface with nothing
//!   to reflect is a black shape.
//!
//! The prop itself is rebuilt from the document whenever it changes, keyed on
//! [`PropEditor::generation`] rather than on any component being `Changed`:
//! evaluating a feature list is a boolean kernel doing real work, and doing it
//! again because egui reported a hover would be felt.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::prop::figure::{clear_the_figure, refresh_the_figure, ScaleFigure};
use crate::prop::document::PropEditor;
use crate::prop::feature::Evaluated;
use crate::prop::surface::Surface;

/// Standing the prop up, so anything that needs to read the built prop can be
/// ordered after it.
///
/// A set rather than a list of names, for the reason `BakeSystems` is one: the
/// camera lives in its own module and frames itself on the prop's bounds, so
/// it needs the prop built first and must not have to name the systems that
/// build it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropSceneSystems {
    Build,
}

/// The root the prop's meshes hang off. One entity, cleared and refilled.
#[derive(Component)]
pub struct PropStage;

/// **On every root entity that belongs to the prop editor**, and the whole of
/// what keeps it visible: the stage, the lights, the scale figure.
///
/// Anything spawned into this mode that a modeller is meant to see needs one,
/// and the failure of forgetting is the obvious one — it is invisible
/// immediately, rather than a map entity that is quietly still there.
#[derive(Component)]
pub struct PropScene;

/// The prop editor's own light, so it can be taken away again on the way out.
#[derive(Component)]
struct PropStageLight;

/// What was hidden on the way in, and what it was before.
///
/// Put back rather than blanket-set to `Inherited` on the way out: plenty of
/// things in this world are hidden on purpose — a spawn point's preview during
/// a round, a body you are looking out of — and an exit that made everything
/// visible would be an exit that broke them. Something already hidden when the
/// mode opened is never recorded, so it is still hidden afterwards.
#[derive(Resource, Default)]
struct HiddenForModelling(HashMap<Entity, Visibility>);

/// The last evaluation, kept so the panels can show the problem list and the
/// triangle count without running the kernel a second time.
#[derive(Resource, Default)]
pub struct PropBuild {
    pub evaluated: Evaluated,
    /// The generation this was built from, so the rebuild knows it is stale.
    built: Option<u64>,
}

/// Materials are cached per surface for the life of the process.
///
/// A drag on a radius rebuilds the prop every frame; minting a material per
/// rebuild would fill `Assets<StandardMaterial>` with a few thousand
/// identical greys over an afternoon. Meshes are *not* cached, and do not need
/// to be: they hang off the entities, so despawning the stage's children drops
/// the last handle and the asset with it.
#[derive(Resource, Default)]
struct SurfaceMaterials(Vec<(Surface, Handle<StandardMaterial>)>);

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
                (light_the_stage, refresh_the_figure, set_the_world_aside, rebuild_the_prop)
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
/// **Roots only**, because `Visibility` inherits: hiding a body hides its
/// bone meshes, and hiding a room hides its baked geometry. Walking children
/// as well would be the same work several times over and would fight the
/// systems that legitimately hide one part of a visible thing.
///
/// **`Without<Node>`**, because Bevy UI carries `Visibility` too and the
/// viewport labels the four cameras are named by are UI. They are chrome
/// rather than scene, and a modelling mode with unlabelled viewports is worse
/// than one with them.
fn set_the_world_aside(
    mut hidden: ResMut<HiddenForModelling>,
    mut entities: Query<
        (Entity, &mut Visibility),
        (Without<ChildOf>, Without<Node>, Without<PropScene>),
    >,
) {
    for (entity, mut visibility) in &mut entities {
        if *visibility == Visibility::Hidden {
            // Already invisible, and not by us: leave it alone and — crucially
            // — do not record it, so the way out does not turn it on.
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
        // A despawn while somebody was modelling is ordinary — a round can be
        // running on a server the whole time — so a missing entity is not a
        // problem, it is one fewer thing to restore.
        if let Ok(mut visibility) = entities.get_mut(entity) {
            *visibility = was;
        }
    }
}

fn light_the_stage(mut commands: Commands) {
    // Two lights and no shadows. A key light alone leaves the underside of a
    // receiver as a silhouette, which is exactly the part somebody modelling
    // it is trying to look at; shadows would be the map's problem rather than
    // the prop's.
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
    // The next entry has to rebuild: the stage it would otherwise consider
    // current has just been despawned.
    build.built = None;
}

/// Evaluate the document and put the result in the world, if it has moved on.
fn rebuild_the_prop(
    mut commands: Commands,
    editor: Res<PropEditor>,
    mut build: ResMut<PropBuild>,
    mut materials: ResMut<SurfaceMaterials>,
    mut material_assets: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    stage: Query<Entity, With<PropStage>>,
) {
    if build.built == Some(editor.generation()) && !stage.is_empty() {
        return;
    }
    build.built = Some(editor.generation());
    build.evaluated = editor.doc().evaluate();

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
        for (_, solid) in &build.evaluated.bodies {
            for (surface, mesh) in solid.meshes() {
                let material = match materials.0.iter().find(|(known, _)| *known == surface) {
                    Some((_, handle)) => handle.clone(),
                    None => {
                        let handle = material_assets.add(surface.material());
                        materials.0.push((surface, handle.clone()));
                        handle
                    }
                };
                parent.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material)));
            }
        }
    });
}

/// The origin, the axes and a ruled floor.
///
/// A prop has no room around it to judge size against, so the grid *is* the
/// ruler: ten-centimetre cells over a metre, which is the range everything a
/// player holds lives in.
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

    // The selected feature's *body*, if it still has one. A feature that has
    // been eaten by a later boolean has no body to outline, and drawing its
    // ingredients would be showing geometry that is not in the prop.
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
