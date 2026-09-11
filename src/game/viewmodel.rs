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
//! Its own copy of the held weapon's meshes, on [`VIEWMODEL_LAYER`], placed by
//! the prop's [`ViewmodelSpec`] — **not** by the carry the body uses. A carry
//! is anatomical, stated in arm lengths and putting the weapon where a body
//! would really hold it, which is well below the eye; a viewmodel is a framing
//! decision on a screen. Reusing the carry put the grip 0.39 m under an eye
//! whose frustum is 0.16 m tall at that distance, so the weapon rendered
//! perfectly, off the bottom of the screen.
//!
//! The aim needs no term of its own: the camera is already pointed where the
//! player is looking, so in its frame the weapon is simply held still.
//!
//! The arms are not here yet — that is the rest of stage 5 in
//! [the plan](../../documentation/weapons-in-hand.md).

use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::camera::Hdr;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::weapon::WeaponId;
use crate::game::held::{HeldModel, HeldPart};
use crate::game::player::{LocalPlayer, PlayerCamera, ViewMode};

/// The layer only the viewmodel camera draws.
///
/// Layer 0 is everything else and 31 is the editor's shape painter, so this is
/// a free one rather than an arbitrary one.
pub const VIEWMODEL_LAYER: usize = 1;

/// Drawn over the play camera, under the 2D UI camera at `isize::MAX`.
const VIEWMODEL_CAMERA_ORDER: isize = 101;

/// Close enough that a weapon held at arm's length never crosses it.
///
/// `pub` because the prop editor borrows it to preview a viewmodel: a preview
/// at the editor's own near plane and field of view would be showing the right
/// place at the wrong size.
pub const VIEWMODEL_NEAR: f32 = 0.005;

/// Narrower than the play camera's, which is deliberately wide. A weapon drawn
/// at 90° across the eye is a weapon bent round the edges of the screen.
pub const VIEWMODEL_FOV: f32 = 70.0 * std::f32::consts::PI / 180.0;

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
            (
                give_the_view_its_own_camera,
                dress_the_viewmodel,
                place_the_viewmodel,
                show_it_only_in_first_person,
            )
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
                // **Matched to the play camera's, not chosen.** Two cameras
                // stacked on one window target with different HDR settings
                // render to different textures, and the one on top composites
                // as nothing — no error, no warning, an empty overlay.
                Hdr,
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
    held: Query<&HeldModel, With<LocalPlayer>>,
    // Copied off the body's own parts rather than looked up in the cache
    // again: whatever the body is drawing is by definition the right meshes,
    // and a second lookup is a second chance to disagree.
    drawn: Query<(&Mesh3d, &MeshMaterial3d<StandardMaterial>), With<HeldPart>>,
    mut views: Query<(Entity, &mut Viewmodel)>,
    cameras: Query<Entity, With<ViewmodelCamera>>,
) {
    let Ok(held) = held.single() else { return };
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
                            // Written by `place_the_viewmodel` before anything
                            // is drawn: where it goes depends on the shape of
                            // the camera, which this does not have.
                            Transform::IDENTITY,
                        ))
                        .id(),
                );
            }
        });
        viewmodel.parts = parts;
    }
}

/// Put the weapon where the viewmodel says, for the camera it is drawn by.
///
/// **Every frame, not once when it is dressed.** The placement is a fraction
/// of the frustum, so it depends on the camera's aspect — and a window the
/// player drags wider would otherwise leave the weapon where the old shape put
/// it, drifting in from the edge it was meant to hang off.
fn place_the_viewmodel(
    held: Query<&HeldModel, With<LocalPlayer>>,
    cameras: Query<(&Camera, &Projection, &Children), With<ViewmodelCamera>>,
    mut parts: Query<&mut Transform, With<ViewmodelPart>>,
) {
    let Ok(held) = held.single() else { return };
    let spec = held.viewmodel();

    for (camera, projection, children) in &cameras {
        let Projection::Perspective(perspective) = projection else { continue };
        let Some(size) = camera.logical_viewport_size().filter(|size| size.y > 0.0) else {
            continue;
        };

        let placed = spec.transform(perspective.fov, size.x / size.y);
        for child in children.iter() {
            let Ok(mut transform) = parts.get_mut(child) else { continue };
            *transform = placed;
        }
    }
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

    use crate::prop::hold::ViewmodelSpec;

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

    /// **A viewmodel has to be inside the frustum it is drawn by**, which the
    /// body's carry is not: it is anatomical, and puts the grip well below the
    /// eye. Reusing it rendered the weapon perfectly, off the bottom of the
    /// screen — visible in the ECS, `ViewVisibility` true, and nowhere on it.
    #[test]
    fn the_default_viewmodel_is_somewhere_the_camera_can_see() {
        let placed = ViewmodelSpec::default().transform(VIEWMODEL_FOV, 16.0 / 9.0);
        let depth = -placed.translation.z;
        assert!(depth > VIEWMODEL_NEAR, "the weapon is behind the near plane");

        let half_height = depth * (VIEWMODEL_FOV / 2.0).tan();
        assert!(
            placed.translation.y.abs() < half_height,
            "the grip sits {:.3} m off the view axis where the frustum is {half_height:.3} m tall",
            placed.translation.y,
        );
    }

    /// **The same numbers mean the same place on screen at any field of view
    /// and any window shape.** A weapon pinned at a fixed offset in view space
    /// drifts towards the middle as the view widens, and a weapon that drifts
    /// inwards shows the cut end it is meant to be hanging off the edge of.
    #[test]
    fn a_viewmodel_stays_where_it_was_put_however_the_view_changes() {
        let spec = ViewmodelSpec::default();

        let screen_place = |fov: f32, aspect: f32| {
            let placed = spec.transform(fov, aspect);
            let half_height = spec.depth * (fov / 2.0).tan();
            // Where it lands as a fraction of the screen, which is what a
            // player actually sees.
            Vec2::new(
                placed.translation.x / (half_height * aspect),
                placed.translation.y / half_height,
            )
        };

        let reference = screen_place(VIEWMODEL_FOV, 16.0 / 9.0);
        for (fov, aspect) in [
            (VIEWMODEL_FOV, 4.0 / 3.0),
            (VIEWMODEL_FOV, 21.0 / 9.0),
            (110.0_f32.to_radians(), 16.0 / 9.0),
            (60.0_f32.to_radians(), 1.0),
        ] {
            let at = screen_place(fov, aspect);
            assert!(
                at.abs_diff_eq(reference, 1e-5),
                "at {:.0}° and {aspect:.2}:1 the weapon sits at {at} rather than {reference}",
                fov.to_degrees(),
            );
        }
    }
}
