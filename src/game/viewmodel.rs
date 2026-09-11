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
//! ## The arms
//!
//! **The weapon is placed and the hands follow it**, which is the same rule
//! `hold_the_weapon` runs on the body and the same code:
//! [`grip_with`](crate::prop::hold::grip_with) solves both arms onto the grip
//! frames the prop already carries. So a hold authored in the prop editor is
//! the hold in first person too, and there is no second set of numbers to
//! author or to let drift.
//!
//! What differs is the frame it is solved in. A body's arms are solved where
//! the body is; these are solved in **eye space** — the rig stood so its own
//! eye is at the camera — from the **rest pose**, never the animated one. The
//! arms are welded to the view: they must not bob with the walk cycle or drop
//! half a metre when you crouch, because the camera does neither.

use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::camera::Hdr;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::flame::Burning;
use crate::common::skeleton::ik::Reach;
use crate::common::skeleton::rig::bone;
use crate::common::skeleton::{Pose, Skeleton};
use crate::common::team::Team;
use crate::common::weapon::WeaponId;
use crate::game::body_mesh::{body_look, BodyMaterials, BodyMeshCache, BodyTint};
use crate::game::held::{HeldModel, HeldPart};
use crate::game::player::{LocalPlayer, PlayerCamera, ViewMode};
use crate::prop::hold::grip_with;

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

/// One drawn bone of the arms holding it.
#[derive(Component, Clone, Copy)]
pub struct ViewmodelArm {
    /// An index into the list [`Skeleton::posed_bones`] returns, as
    /// [`BoneMesh`](crate::game::body_mesh::BoneMesh) is for a body.
    bone: usize,
    /// Which hand this bone belongs to, indexing [`grip_with`]'s answer: the
    /// trigger hand first, then the support.
    hand: usize,
}

/// Which bones the viewmodel draws, and which hand each belongs to.
///
/// **Forearms and hands, not whole arms.** An upper arm runs back to a
/// shoulder that is beside the camera rather than in front of it, and at a
/// near plane of five millimetres it reads as a slab of shoulder hanging in
/// the corner of the screen rather than as an arm. A sleeve entering frame at
/// the elbow is what a person actually sees down their own sights.
const ARM_BONES: [(&str, usize); 4] = [
    (bone::FOREARM_R, 0),
    (bone::HAND_R, 0),
    (bone::FOREARM_L, 1),
    (bone::HAND_L, 1),
];

/// Where up the head bone an eye sits.
///
/// The rig has no eye in it, and the movement hull's is no use here: it is a
/// flat 1.8 m for every class, so a Scout's arms would hang from a Heavy's
/// shoulders. Taken off the rig instead, so the arms in front of you are the
/// arms of the body you are playing.
const EYE_UP_THE_HEAD: f32 = 0.55;

/// What the viewmodel is currently showing, so it can tell when to rebuild.
#[derive(Component, Default)]
pub struct Viewmodel {
    parts: Vec<Entity>,
    /// The weapon the parts were built for, and the cache generation they came
    /// from — the same two questions `HeldModel` asks, for the same reason.
    showing: Option<(Option<WeaponId>, u64)>,
    arms: Vec<Entity>,
    /// Which body the arms were built for. A respawn, a class change and a
    /// client being handed a body are all a *different entity*, which is one
    /// question rather than three.
    armed_for: Option<Entity>,
}

pub struct ViewmodelPlugin;

impl Plugin for ViewmodelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                give_the_view_its_own_camera,
                dress_the_viewmodel,
                dress_the_viewmodel_arms,
                place_the_viewmodel,
                place_the_viewmodel_arms,
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

/// Give the viewmodel a pair of arms, from the body's own build.
///
/// **The same meshes and the same material the body is drawn with.** A
/// first-person forearm is that forearm seen from closer up, so a second set
/// of handles would be a second silhouette to keep in step — and taking the
/// material too means your own hands are your own team's colour, and alight
/// when you are.
fn dress_the_viewmodel_arms(
    mut commands: Commands,
    mut cache: ResMut<BodyMeshCache>,
    mut palette: ResMut<BodyMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    bodies: Query<
        (Entity, &Skeleton, Option<&Team>, Option<&BodyTint>, Option<&Burning>),
        With<LocalPlayer>,
    >,
    mut views: Query<&mut Viewmodel>,
    cameras: Query<Entity, With<ViewmodelCamera>>,
) {
    let Ok(camera) = cameras.single() else { return };
    let body = bodies.single().ok();

    for mut viewmodel in &mut views {
        if viewmodel.armed_for == body.map(|(entity, ..)| entity) {
            continue;
        }
        viewmodel.armed_for = body.map(|(entity, ..)| entity);

        for arm in viewmodel.arms.drain(..) {
            commands.entity(arm).despawn();
        }

        // Nobody's body to take a build from: no arms, rather than somebody
        // else's.
        let Some((_, skeleton, team, tint, burning)) = body else { continue };

        let material = palette.get(body_look(team, tint, burning), &mut materials);
        let handles = cache.handles(skeleton, &mut meshes).clone();

        let mut arms = Vec::new();
        commands.entity(camera).with_children(|camera| {
            for (name, hand) in ARM_BONES {
                let Some(bone) = skeleton.index_of(name) else { continue };
                let Some(handle) = handles.get(bone) else { continue };
                arms.push(
                    camera
                        .spawn((
                            ViewmodelArm { bone, hand },
                            Mesh3d(handle.clone()),
                            MeshMaterial3d(material.clone()),
                            RenderLayers::layer(VIEWMODEL_LAYER),
                            // Written by `place_the_viewmodel_arms` before
                            // anything is drawn; where a bone goes depends on
                            // the shape of the camera, which this does not
                            // have.
                            Transform::IDENTITY,
                            // Hidden until something says it reached, so a
                            // rest-posed arm is never drawn for a frame.
                            Visibility::Hidden,
                        ))
                        .id(),
                );
            }
        });
        viewmodel.arms = arms;
    }
}

/// Solve the arms onto the viewmodel's weapon, every frame.
///
/// The whole of the work is [`grip_with`]'s, which is what puts a body's hands
/// on a weapon — so the grip frames authored in the prop editor are the ones
/// used here, and first person cannot show a hold the third-person body does
/// not perform.
///
/// **An arm that did not reach is not drawn.** `grip_with` leaves a hand that
/// cannot get to its grip exactly where it was, which on a body still swinging
/// with the walk cycle reads as not holding — and in front of an eye reads as
/// a forearm lying across the screen at the camera's own depth. A weapon with
/// no support grip, which is most of them, has no left arm at all until there
/// is an idle pose for one to be in. That is stage 6.
fn place_the_viewmodel_arms(
    bodies: Query<(&Skeleton, &HeldModel), With<LocalPlayer>>,
    cameras: Query<(&Camera, &Projection, &Children), With<ViewmodelCamera>>,
    mut parts: Query<(&ViewmodelArm, &mut Transform, &mut Visibility)>,
) {
    let Ok((skeleton, held)) = bodies.single() else { return };

    for (camera, projection, children) in &cameras {
        let Projection::Perspective(perspective) = projection else { continue };
        let Some(size) = camera.logical_viewport_size().filter(|size| size.y > 0.0) else {
            continue;
        };

        let root = eye_space(skeleton);
        let weapon = held.viewmodel().transform(perspective.fov, size.x / size.y);

        // From rest rather than from the body's pose: these arms are welded to
        // the view, and a camera that does not bob must not have arms that do.
        let mut pose = Pose::rest();
        let reached = grip_with(skeleton, &mut pose, &root, held.hold(), &weapon);
        let posed = skeleton.posed_bones(&pose, &root);

        for child in children.iter() {
            let Ok((arm, mut transform, mut visibility)) = parts.get_mut(child) else { continue };
            let wanted = match reached[arm.hand] {
                Reach::Reached => Visibility::Inherited,
                _ => Visibility::Hidden,
            };
            if *visibility != wanted {
                *visibility = wanted;
            }

            let bone = &posed[arm.bone];
            // The mesh is already the bone's size; a scale here would be a
            // second opinion about that and would skew its normals.
            *transform = Transform { translation: bone.head, rotation: bone.rotation, ..default() };
        }
    }
}

/// Where the rig's root goes for its own eye to sit at the camera.
///
/// No rotation: the rig faces `-Z` and so does a camera, so standing the body
/// at the eye is the whole of it.
fn eye_space(skeleton: &Skeleton) -> Transform {
    let bones = skeleton.posed_bones(&Pose::rest(), &Transform::IDENTITY);
    let eye = skeleton
        .index_of(bone::HEAD)
        .map(|head| bones[head].head.lerp(bones[head].tail, EYE_UP_THE_HEAD))
        .unwrap_or_default();
    Transform::from_translation(-eye)
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

    use crate::common::class::Class;
    use crate::common::skeleton::humanoid;
    use crate::common::skeleton::ik::Reach;
    use crate::prop::document;
    use crate::prop::hold::{HoldSpec, ViewmodelSpec};

    use super::*;

    const EVERY_CLASS: [Class; 11] = [
        Class::Scout, Class::Soldier, Class::Pyro, Class::Demoman,
        Class::Heavy, Class::Engineer, Class::Medic, Class::Sniper,
        Class::Spy, Class::Civilian, Class::Mercenary,
    ];

    /// **The arms have to get to the weapon they are drawn holding**, on every
    /// build — and a viewmodel is not a hold, so the body reaching a grip
    /// proves nothing about this. An arm that comes up short is put back where
    /// it was by `reach_for`, so the failure is a weapon floating in front of
    /// two hands that are somewhere else entirely.
    #[test]
    fn every_class_can_reach_the_viewmodel() {
        let hold = HoldSpec::default();
        let weapon = ViewmodelSpec::default().transform(VIEWMODEL_FOV, 16.0 / 9.0);

        for class in EVERY_CLASS {
            let skeleton = humanoid(class.proportions());
            let mut pose = Pose::rest();
            let reached = grip_with(&skeleton, &mut pose, &eye_space(&skeleton), &hold, &weapon);
            assert_eq!(
                reached[0],
                Reach::Reached,
                "{class:?} cannot get its trigger hand to its own viewmodel",
            );
        }
    }

    /// The rig's own eye, not the movement hull's — that one is a flat 1.8 m
    /// for every class, so a Scout's arms would hang off a Heavy's shoulders.
    #[test]
    fn the_eye_is_the_rigs_own_and_the_shoulders_hang_below_it() {
        for class in EVERY_CLASS {
            let skeleton = humanoid(class.proportions());
            let root = eye_space(&skeleton);
            let bones = skeleton.posed_bones(&Pose::rest(), &root);

            let head = skeleton.index_of(bone::HEAD).expect("a rig has a head");
            let eye = bones[head].head.lerp(bones[head].tail, EYE_UP_THE_HEAD);
            assert!(eye.length() < 1e-5, "{class:?}'s eye is at {eye}, not at the camera");

            let shoulder = skeleton.index_of(bone::UPPER_ARM_R).expect("a rig has an arm");
            let at = bones[shoulder].head;
            assert!(at.y < 0.0, "{class:?}'s shoulder is above its own eye");
            // The rig faces `-Z` with `+Y` up, so its right hand is `+X`.
            assert!(at.x > 0.0, "{class:?}'s right shoulder is not on its right");
        }
    }

    /// **A hand that is not on the weapon is not drawn.** `grip_with` leaves
    /// an arm it could not solve exactly where it was, which is right on a
    /// body — it swings with the walk cycle and reads as not holding — and in
    /// front of an eye is a forearm lying across the screen at the camera's
    /// own depth. Most weapons state no support grip, so this is the ordinary
    /// case rather than the edge one.
    #[test]
    fn a_hand_with_no_grip_to_hold_does_not_reach() {
        let skeleton = humanoid(Class::Soldier.proportions());
        let weapon = ViewmodelSpec::default().transform(VIEWMODEL_FOV, 16.0 / 9.0);
        let mut pose = Pose::rest();

        let reached =
            grip_with(&skeleton, &mut pose, &eye_space(&skeleton), &HoldSpec::default(), &weapon);
        assert_eq!(reached[0], Reach::Reached, "the trigger hand has a grip and should be on it");
        assert_ne!(
            reached[1],
            Reach::Reached,
            "a support hand with nothing to hold reported holding it",
        );
        assert_eq!(
            ARM_BONES.iter().filter(|(_, hand)| *hand == 1).count(),
            2,
            "the support hand's bones are no longer the ones this hides",
        );
    }

    /// The shipped prop, with the hold and the viewmodel somebody actually
    /// tuned rather than the defaults — which is the combination a player
    /// sees, and the one nothing else in this file exercises.
    ///
    /// The launcher is `two_handed = false`, so it is held by one hand and one
    /// arm is drawn. That is the file's decision and this asserts it is
    /// *carried out*, not that it is right.
    #[test]
    fn the_shipped_launcher_is_held_in_first_person() {
        let doc = document::parse(
            &std::fs::read_to_string("assets/default/props/rocket_launcher.gpp")
                .expect("the default pack ships a rocket launcher"),
        )
        .expect("the shipped launcher parses");

        let weapon = doc.viewmodel.transform(VIEWMODEL_FOV, 16.0 / 9.0);
        for class in EVERY_CLASS {
            let skeleton = humanoid(class.proportions());
            let hold = doc.hold.for_class(Some(class));
            let mut pose = Pose::rest();
            let reached = grip_with(&skeleton, &mut pose, &eye_space(&skeleton), &hold, &weapon);
            assert_eq!(
                reached[0],
                Reach::Reached,
                "{class:?} cannot get its trigger hand to the launcher in view",
            );
            assert_eq!(
                reached[1] == Reach::Reached,
                hold.support_frame().is_some(),
                "{class:?}'s support hand disagrees with what the file states",
            );
        }
    }

    /// A build that differs gives shoulders that differ, which is the whole
    /// reason the eye comes off the rig.
    #[test]
    fn a_bigger_build_has_its_own_arms() {
        let reach = |class: Class| {
            let skeleton = humanoid(class.proportions());
            let bones = skeleton.posed_bones(&Pose::rest(), &eye_space(&skeleton));
            bones[skeleton.index_of(bone::UPPER_ARM_R).unwrap()].head
        };
        assert!(
            (reach(Class::Heavy) - reach(Class::Scout)).length() > 0.01,
            "every class has the same shoulders",
        );
    }

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

