//! What you see of your own weapon in first person: drawn by its own camera,
//! a child of the play camera, so its near plane and field of view keep the
//! weapon from clipping into walls. Every camera query filters on a marker;
//! see CLAUDE.md, "Editor and game in one process".
//!
//! The weapon is placed by the prop's [`ViewmodelSpec`], not the body's carry:
//! the carry put the grip 0.39 m under the eye where the frustum is 0.16 m
//! tall, off the bottom of the screen.
//!
//! The arms are solved onto it with the same
//! [`grip_with`](crate::prop::hold::grip_with) a body uses, in eye space from
//! the rest pose, so they neither bob nor crouch with the body.

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

/// The layer only the viewmodel camera draws. 0 is everything else and 31 is
/// the editor's shape painter.
pub const VIEWMODEL_LAYER: usize = 1;

/// Drawn over the play camera, under the 2D UI camera at `isize::MAX`.
const VIEWMODEL_CAMERA_ORDER: isize = 101;

/// Close enough that a weapon at arm's length never crosses it. `pub` so the
/// prop editor's viewmodel preview matches it.
pub const VIEWMODEL_NEAR: f32 = 0.005;

/// Narrower than the play camera's: at 90° a weapon across the eye bends round
/// the screen edges.
pub const VIEWMODEL_FOV: f32 = 70.0 * std::f32::consts::PI / 180.0;

#[derive(Component)]
pub struct ViewmodelCamera;

#[derive(Component)]
pub struct ViewmodelPart;

#[derive(Component, Clone, Copy)]
pub struct ViewmodelArm {
    /// Index into [`Skeleton::posed_bones`].
    bone: usize,
    /// Index into [`grip_with`]'s answer: 0 trigger hand, 1 support.
    hand: usize,
}

/// Forearms and hands only: an upper arm runs back to a shoulder beside the
/// camera and at a 5 mm near plane reads as a slab in the screen corner.
const ARM_BONES: [(&str, usize); 4] = [
    (bone::FOREARM_R, 0),
    (bone::HAND_R, 0),
    (bone::FOREARM_L, 1),
    (bone::HAND_L, 1),
];

/// Fraction of the way up the head bone the eye sits. Taken off the rig rather
/// than the movement hull, whose 1.8 m is the same for every class.
const EYE_UP_THE_HEAD: f32 = 0.55;

/// What the viewmodel is showing, so it can tell when to rebuild.
#[derive(Component, Default)]
pub struct Viewmodel {
    parts: Vec<Entity>,
    /// Weapon and cache generation, as [`HeldModel::showing`].
    showing: Option<(Option<WeaponId>, u64)>,
    arms: Vec<Entity>,
    /// The body the arms were built for. A respawn, class change or a client
    /// being handed a body all show up as a different entity.
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
                    // Or it would paint over the world under it.
                    clear_color: ClearColorConfig::None,
                    ..default()
                },
                // Must match the play camera's: stacked cameras with different
                // HDR settings render to different textures, and the top one
                // composites as nothing, with no error.
                Hdr,
                Tonemapping::TonyMcMapface,
                Projection::Perspective(PerspectiveProjection {
                    fov: VIEWMODEL_FOV,
                    near: VIEWMODEL_NEAR,
                    ..default()
                }),
                RenderLayers::layer(VIEWMODEL_LAYER),
                Transform::IDENTITY,
                Name::new("Viewmodel view"),
            ));
        });
    }
}

/// A second copy of the held weapon, in front of the eye rather than in the
/// body's hand. Compared against `HeldModel::showing`, since the model may
/// still be in flight.
fn dress_the_viewmodel(
    mut commands: Commands,
    held: Query<&HeldModel, With<LocalPlayer>>,
    // Copied off the body's own parts rather than a second cache lookup that
    // could disagree.
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
                            // Written by `place_the_viewmodel`, which needs the
                            // camera's shape.
                            Transform::IDENTITY,
                        ))
                        .id(),
                );
            }
        });
        viewmodel.parts = parts;
    }
}

/// Every frame: the placement is a fraction of the frustum, so it changes with
/// the window's aspect.
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

/// The body's own meshes and material, so your hands are your team's colour
/// and alight when you are.
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

        // No body: no arms, rather than somebody else's.
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
                            // Written by `place_the_viewmodel_arms`, which needs
                            // the camera's shape.
                            Transform::IDENTITY,
                            // Hidden until it reaches, so a rest-posed arm is
                            // never drawn for a frame.
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
/// An arm that did not reach is hidden: `grip_with` leaves it where it was,
/// which in front of an eye is a forearm across the screen. Most weapons have
/// no support grip, so usually only one arm is drawn.
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

        // From rest: a camera that does not bob must not have arms that do.
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
            // The mesh is already bone-sized; a scale would skew its normals.
            *transform = Transform { translation: bone.head, rotation: bone.rotation, ..default() };
        }
    }
}

/// Where the rig's root goes for its eye to sit at the camera. No rotation:
/// the rig and a camera both face `-Z`.
fn eye_space(skeleton: &Skeleton) -> Transform {
    let bones = skeleton.posed_bones(&Pose::rest(), &Transform::IDENTITY);
    let eye = skeleton
        .index_of(bone::HEAD)
        .map(|head| bones[head].head.lerp(bones[head].tail, EYE_UP_THE_HEAD))
        .unwrap_or_default();
    Transform::from_translation(-eye)
}

fn show_it_only_in_first_person(
    view: Res<ViewMode>,
    mut cameras: Query<&mut Camera, With<ViewmodelCamera>>,
) {
    let wanted = !view.shows_own_body();
    for mut camera in &mut cameras {
        // Assigned only on change, so the camera is not dirtied every frame.
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

    /// An arm that comes up short is left where it was, so a weapon floats in
    /// front of hands elsewhere. A viewmodel is not a hold, so the body tests
    /// do not cover this.
    #[test]
    fn every_class_can_reach_the_viewmodel() {
        let hold = HoldSpec::default();
        let weapon = ViewmodelSpec::default().transform(VIEWMODEL_FOV, 16.0 / 9.0);

        for class in EVERY_CLASS {
            let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(class));
            let mut pose = Pose::rest();
            let reached = grip_with(&skeleton, &mut pose, &eye_space(&skeleton), &hold, &weapon);
            assert_eq!(
                reached[0],
                Reach::Reached,
                "{class:?} cannot get its trigger hand to its own viewmodel",
            );
        }
    }

    /// The eye is the rig's own, with the shoulders below it.
    #[test]
    fn the_eye_is_the_rigs_own_and_the_shoulders_hang_below_it() {
        for class in EVERY_CLASS {
            let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(class));
            let root = eye_space(&skeleton);
            let bones = skeleton.posed_bones(&Pose::rest(), &root);

            let head = skeleton.index_of(bone::HEAD).expect("a rig has a head");
            let eye = bones[head].head.lerp(bones[head].tail, EYE_UP_THE_HEAD);
            assert!(eye.length() < 1e-5, "{class:?}'s eye is at {eye}, not at the camera");

            let shoulder = skeleton.index_of(bone::UPPER_ARM_R).expect("a rig has an arm");
            let at = bones[shoulder].head;
            assert!(at.y < 0.0, "{class:?}'s shoulder is above its own eye");
            // The rig faces `-Z` with `+Y` up, so its right is `+X`.
            assert!(at.x > 0.0, "{class:?}'s right shoulder is not on its right");
        }
    }

    /// A support hand with no grip must report not reaching, or its forearm is
    /// drawn across the screen.
    #[test]
    fn a_hand_with_no_grip_to_hold_does_not_reach() {
        let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Soldier));
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

    /// The shipped launcher's tuned hold and viewmodel, which nothing else here
    /// exercises. Asserts the support hand matches what the file states.
    #[test]
    fn the_shipped_launcher_is_held_in_first_person() {
        let doc = document::parse(
            &std::fs::read_to_string("assets/default/props/rocket_launcher.gpp")
                .expect("the default pack ships a rocket launcher"),
        )
        .expect("the shipped launcher parses");

        let weapon = doc.viewmodel.transform(VIEWMODEL_FOV, 16.0 / 9.0);
        for class in EVERY_CLASS {
            let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(class));
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

    /// Different builds have different shoulders in view.
    #[test]
    fn a_bigger_build_has_its_own_arms() {
        let reach = |class: Class| {
            let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(class));
            let bones = skeleton.posed_bones(&Pose::rest(), &eye_space(&skeleton));
            bones[skeleton.index_of(bone::UPPER_ARM_R).unwrap()].head
        };
        assert!(
            (reach(Class::Heavy) - reach(Class::Scout)).length() > 0.01,
            "every class has the same shoulders",
        );
    }

    /// A viewmodel camera that also answered to `PlayerCamera` would be moved
    /// by `place_camera` and projected through by damage numbers.
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
        // A child, so it follows the view and `despawn_the_view` removes it.
        assert_eq!(world.get::<ChildOf>(viewmodel).map(|of| of.parent()), Some(view));
    }

    /// The `Viewmodel` component on the view records that it already has one.
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
        assert_ne!(VIEWMODEL_LAYER, 0);
        assert_ne!(VIEWMODEL_LAYER, 31);
    }

    /// The body's carry put the grip off the bottom of the screen; the default
    /// viewmodel must be inside the frustum.
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

    /// A fixed view-space offset drifts towards the middle as the view widens,
    /// showing the weapon's cut end. Placement must be a fraction of the frustum.
    #[test]
    fn a_viewmodel_stays_where_it_was_put_however_the_view_changes() {
        let spec = ViewmodelSpec::default();

        let screen_place = |fov: f32, aspect: f32| {
            let placed = spec.transform(fov, aspect);
            let half_height = spec.depth * (fov / 2.0).tan();
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

