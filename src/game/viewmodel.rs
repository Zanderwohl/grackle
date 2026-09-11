//! What you see of your own weapon from inside your own head.
//!
//! A viewmodel is not the third-person weapon seen from closer up. It is drawn
//! by **its own camera**, and that is the whole reason this module exists: a
//! weapon a metre from the eye is a weapon that clips through every wall it is
//! pushed against unless something else draws it, with its own near plane and
//! its own field of view.
//!
//! ## Cameras are told apart by markers, never by shape
//!
//! The window has six cameras in it now — four editor viewports, the play
//! camera, this one — and a query that says `&Camera` gets whichever ones
//! happen to match. Every camera here carries a zero-sized marker
//! ([`Multicam`](crate::editor::multicam::Multicam),
//! [`PlayerCamera`](crate::game::player::PlayerCamera), [`ViewmodelCamera`])
//! and **every query filters on one**. The failure mode is not a compile
//! error: it is `place_camera` moving two cameras, or damage numbers
//! projected through the wrong one.
//!
//! This camera is a **child of the play camera**, which is what keeps the two
//! from ever disagreeing about where you are looking — and means
//! `despawn_the_view` takes it down without knowing it exists.
//!
//! ## What it draws
//!
//! Its own copy of the held weapon's meshes, on [`VIEWMODEL_LAYER`], placed
//! from the same [`HoldSpec`] the body uses but against the camera rather than
//! against a chest. The aim needs no term of its own: the camera is already
//! pointed where the player is looking, so in its frame the weapon is simply
//! held still.
//!
//! The arms are not here yet — that is the rest of stage 5 in
//! [the plan](../../documentation/weapons-in-hand.md).

use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::skeleton::Skeleton;
use crate::common::weapon::WeaponId;
use crate::game::held::{HeldModel, HeldPart};
use crate::game::player::{LocalPlayer, PlayerCamera, ViewMode};
use crate::prop::hold::HoldSpec;

/// The layer only the viewmodel camera draws.
///
/// Layer 0 is everything else and 31 is the editor's shape painter, so this is
/// a free one rather than an arbitrary one.
pub const VIEWMODEL_LAYER: usize = 1;

/// Drawn over the play camera, under the 2D UI camera at `isize::MAX`.
const VIEWMODEL_CAMERA_ORDER: isize = 101;

/// Close enough that a weapon held at arm's length never crosses it.
const VIEWMODEL_NEAR: f32 = 0.005;

/// Narrower than the play camera's, which is deliberately wide. A weapon drawn
/// at 90° across the eye is a weapon bent round the edges of the screen.
const VIEWMODEL_FOV: f32 = 70.0 * std::f32::consts::PI / 180.0;

/// Where the shoulder sits relative to the eye.
///
/// A carry is stated relative to the point between the shoulders, and in first
/// person there is no chest to measure from — the camera *is* the head. One
/// number rather than reaching into the rig for a bone the viewmodel does not
/// otherwise need.
const SHOULDER_BELOW_EYE: Vec3 = Vec3::new(0.0, -0.22, 0.0);

/// The second camera, and the reason a query for `&Camera` is never enough.
#[derive(Component)]
pub struct ViewmodelCamera;

/// One drawn part of the weapon in your own hands.
#[derive(Component)]
pub struct ViewmodelPart;

/// What the viewmodel is currently showing, so it can tell when to rebuild.
#[derive(Component, Default)]
pub struct Viewmodel {
    parts: Vec<Entity>,
    /// The weapon the parts were built for, and the cache generation they came
    /// from — the same two questions `HeldModel` asks, for the same reason.
    showing: Option<(Option<WeaponId>, u64)>,
}

pub struct ViewmodelPlugin;

impl Plugin for ViewmodelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (give_the_view_its_own_camera, dress_the_viewmodel, show_it_only_in_first_person)
                .chain()
                .run_if(in_state(AppMode::Play)),
        );
    }
}

/// Hang a second camera off the play camera, once.
fn give_the_view_its_own_camera(
    mut commands: Commands,
    views: Query<Entity, (With<PlayerCamera>, Without<Viewmodel>)>,
) {
    for view in &views {
        commands.entity(view).insert(Viewmodel::default()).with_children(|view| {
            view.spawn((
                ViewmodelCamera,
                Camera3d::default(),
                Camera {
                    order: VIEWMODEL_CAMERA_ORDER,
                    // Nothing, or it would paint over the world it is drawn on
                    // top of.
                    clear_color: ClearColorConfig::None,
                    ..default()
                },
                Tonemapping::TonyMcMapface,
                Projection::Perspective(PerspectiveProjection {
                    fov: VIEWMODEL_FOV,
                    near: VIEWMODEL_NEAR,
                    ..default()
                }),
                RenderLayers::layer(VIEWMODEL_LAYER),
                // At the play camera exactly, so the two can never disagree
                // about where the player is looking.
                Transform::IDENTITY,
                Name::new("Viewmodel view"),
            ));
        });
    }
}

/// Give the viewmodel its own copy of whatever the body is holding.
///
/// A second copy rather than the body's, because the two are in different
/// places: the body's weapon is in the body's hand and this one is in front of
/// the eye. Keyed on what is showing against what is held, the same way
/// [`HeldModel`] is keyed — the model may still be in flight.
fn dress_the_viewmodel(
    mut commands: Commands,
    held: Query<(&HeldModel, &Skeleton), With<LocalPlayer>>,
    // Copied off the body's own parts rather than looked up in the cache
    // again: whatever the body is drawing is by definition the right meshes,
    // and a second lookup is a second chance to disagree.
    drawn: Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>), With<HeldPart>>,
    mut views: Query<(Entity, &mut Viewmodel)>,
    cameras: Query<Entity, With<ViewmodelCamera>>,
) {
    let Ok((held, skeleton)) = held.single() else { return };
    let Ok(camera) = cameras.single() else { return };

    for (_, mut viewmodel) in &mut views {
        let wanted = Some(held.showing());
        if viewmodel.showing == wanted {
            continue;
        }
        viewmodel.showing = wanted;

        for part in viewmodel.parts.drain(..) {
            commands.entity(part).despawn();
        }

        let hold = held.hold();
        let proportions = skeleton.proportions();
        let placed = weapon_in_view(hold, proportions.height * proportions.arm_length);

        let mut parts = Vec::new();
        commands.entity(camera).with_children(|camera| {
            for part in held.parts() {
                let Ok((mesh, material)) = drawn.get(*part) else { continue };
                parts.push(
                    camera
                        .spawn((
                            ViewmodelPart,
                            mesh.clone(),
                            material.clone(),
                            RenderLayers::layer(VIEWMODEL_LAYER),
                            placed,
                        ))
                        .id(),
                );
            }
        });
        viewmodel.parts = parts;
    }
}

/// Where the weapon sits in the camera's own frame.
///
/// The same carry the body uses, measured from the eye rather than from a
/// chest. No aim term: the camera is already pointed where the player is
/// looking, so in its frame the weapon is held still.
fn weapon_in_view(hold: &HoldSpec, arm_length: f32) -> Transform {
    Transform::from_translation(SHOULDER_BELOW_EYE + hold.carry(arm_length)).with_rotation(
        crate::common::rotation::quat_from_euler(Vec3::from_array(hold.carry_rotation)),
    )
}

/// A viewmodel is what you see *instead of* your own body.
fn show_it_only_in_first_person(
    view: Res<ViewMode>,
    mut cameras: Query<&mut Camera, With<ViewmodelCamera>>,
) {
    let wanted = !view.shows_own_body();
    for mut camera in &mut cameras {
        // Assigned only on a change, so this does not dirty the camera every
        // frame.
        if camera.is_active != wanted {
            camera.is_active = wanted;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// **The two cameras are told apart by markers, and nothing else.**
    ///
    /// The failure this guards is not a compile error. `place_camera` writes
    /// `Query<&mut Transform, With<PlayerCamera>>` and damage numbers project
    /// through `Query<(&Camera, &GlobalTransform), With<PlayerCamera>>` — a
    /// viewmodel camera that also answered to `PlayerCamera` would be moved
    /// twice and projected through once by mistake, and both read as the view
    /// being subtly wrong rather than as anything being broken.
    #[test]
    fn the_viewmodel_camera_is_not_the_players() {
        let mut world = World::new();
        let view = world.spawn(PlayerCamera).id();

        world.run_system_once(give_the_view_its_own_camera).unwrap();

        let mut cameras = world.query_filtered::<Entity, With<ViewmodelCamera>>();
        let made: Vec<Entity> = cameras.iter(&world).collect();
        assert_eq!(made.len(), 1, "expected exactly one viewmodel camera");

        let viewmodel = made[0];
        assert!(
            world.get::<PlayerCamera>(viewmodel).is_none(),
            "the viewmodel camera answers to `PlayerCamera` and will be driven as the view",
        );
        // A child, so it cannot drift from the view and `despawn_the_view`
        // takes it down without knowing it exists.
        assert_eq!(world.get::<ChildOf>(viewmodel).map(|of| of.parent()), Some(view));
    }

    /// Once, not once per frame — the play camera keeps its `Viewmodel` as the
    /// record that it already has one.
    #[test]
    fn a_view_is_only_given_one_camera() {
        let mut world = World::new();
        world.spawn(PlayerCamera);

        for _ in 0..3 {
            world.run_system_once(give_the_view_its_own_camera).unwrap();
        }

        let mut cameras = world.query_filtered::<Entity, With<ViewmodelCamera>>();
        assert_eq!(cameras.iter(&world).count(), 1, "a camera per frame");
    }

    /// Drawn over the world and under the UI, on a layer nothing else uses.
    #[test]
    fn it_sits_between_the_world_and_the_interface() {
        assert!(VIEWMODEL_CAMERA_ORDER > 100, "the viewmodel draws under the world");
        assert!(VIEWMODEL_CAMERA_ORDER < isize::MAX, "the viewmodel draws over the interface");
        // 0 is everything else and 31 is the editor's shape painter.
        assert_ne!(VIEWMODEL_LAYER, 0);
        assert_ne!(VIEWMODEL_LAYER, 31);
    }

    /// In front of the eye and below it, or the weapon is behind your head.
    #[test]
    fn the_weapon_hangs_in_front_of_and_below_the_eye() {
        let placed = weapon_in_view(&HoldSpec::default(), 0.55);
        assert!(placed.translation.z < -0.05, "the weapon is not in front: {}", placed.translation);
        assert!(placed.translation.y < -0.1, "the weapon is not below the eye");
    }
}
