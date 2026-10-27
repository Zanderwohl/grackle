//! What you see of your own weapon in first person: drawn by its own camera,
//! a child of the play camera, so its near plane and field of view keep the
//! weapon from clipping into walls. Every camera query filters on a marker;
//! see CLAUDE.md, "Editor and game in one process".
//!
//! The weapon is placed by the prop's
//! [`ViewmodelSpec`](crate::prop::hold::ViewmodelSpec), not the body's carry:
//! the carry put the grip 0.39 m under the eye where the frustum is 0.16 m
//! tall, off the bottom of the screen.
//!
//! The arms are solved onto it with the same
//! [`grip_with`](crate::prop::hold::grip_with) a body uses, in eye space from
//! the rest pose, so they neither bob nor crouch with the body.

use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::camera::Hdr;
use bevy::ecs::change_detection::Tick;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::flame::Burning;
use crate::common::skeleton::ik::Reach;
use crate::common::skeleton::rig::bone;
use crate::common::skeleton::{Pose, Skeleton};
use crate::common::team::Team;
use crate::common::weapon::WeaponId;
use crate::game::body_mesh::{body_look, BodyMaterials, BodyMeshCache, BodyTint};
use crate::common::skeleton::clip::Clip;
use crate::common::skeleton::play::{apply_stack, grip_offsets, ClipStack, Frames};
use crate::game::held::{perform, HeldModel, HeldPart, Performance, SharedWeaponClips};
use crate::game::player::{place_camera, LocalPlayer, PlayerCamera, ViewMode, PLAY_CAMERA_ORDER};
use crate::prop::hold::grip_hands;

pub const VIEWMODEL_LAYER: usize = crate::common::render_layers::VIEWMODEL;

/// Drawn over the play camera, under the HUD camera.
pub const VIEWMODEL_CAMERA_ORDER: isize = PLAY_CAMERA_ORDER + 1;

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
    /// Index into [`grip_hands`]'s answer: 0 trigger hand, 1 support.
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
    /// What the arms were last solved from, so a frame that changed none of
    /// it costs a comparison rather than a solve. Cleared whenever the arms are
    /// rebuilt: they spawn hidden, and only placing them shows them.
    arms_placed_for: Option<ArmsKey>,
    /// Where the weapon was drawn this frame, after its clips: what the arms
    /// are solved onto.
    weapon: Option<Transform>,
    /// Where the clips moved each hand's grip, trigger hand first.
    grips: [Vec3; 2],
}

/// Everything [`place_the_viewmodel_arms`] reads, and nothing else.
///
/// The rig and the held model are only ever *inserted* — a respawn, a class
/// change, a weapon switch or a model arriving from the server — so when each
/// last changed stands in for comparing either by value. The frustum is
/// compared bit for bit: an epsilon would be a second opinion about when the
/// weapon has moved.
#[derive(Clone, Copy, PartialEq, Debug)]
struct ArmsKey {
    body: Entity,
    skeleton: Tick,
    held: Tick,
    fov: u32,
    aspect: u32,
    /// The weapon as drawn, so a clip moving it re-solves the arms.
    weapon: [u32; 7],
    /// The grips, so a clip taking a hand off the weapon does too.
    grips: [[u32; 3]; 2],
}

fn bits(transform: &Transform) -> [u32; 7] {
    let (t, r) = (transform.translation, transform.rotation);
    [t.x, t.y, t.z, r.x, r.y, r.z, r.w].map(f32::to_bits)
}

/// Where this machine's shots appear to leave, in world space: the held
/// model's muzzle where the viewmodel draws it. `None` in third person, or
/// holding something with no muzzle. Display only; every shot really leaves
/// the eye.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub struct ApparentMuzzle(pub Option<Vec3>);

pub struct ViewmodelPlugin;

impl Plugin for ViewmodelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApparentMuzzle>()
            // Straight after the view is aimed, so a shot dressed this frame
            // starts from where the barrel is drawn this frame.
            .add_systems(
                RunFixedMainLoop,
                find_the_apparent_muzzle
                    .after(place_camera)
                    .in_set(RunFixedMainLoopSystems::AfterFixedMainLoop)
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), |mut muzzle: ResMut<ApparentMuzzle>| {
                muzzle.0 = None;
            })
            .add_systems(
            Update,
            (
                give_the_view_its_own_camera,
                dress_the_viewmodel,
                dress_the_viewmodel_arms,
                perform_viewmodel_clips,
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
        // Its own performance and stack: first person performs the weapon's
        // own clips, which the body in third person does not.
        commands
            .entity(view)
            .insert((Viewmodel::default(), Performance::default(), ClipStack::default()))
            .with_children(|view| {
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

/// The viewmodel's deploy and kicks, from the held weapon's clips: the same
/// performance a body runs, on a stack of its own.
fn perform_viewmodel_clips(
    time: Res<Time>,
    clips: Res<Assets<Clip>>,
    shared: Option<Res<SharedWeaponClips>>,
    held: Query<&HeldModel, With<LocalPlayer>>,
    mut views: Query<(&mut Performance, &mut ClipStack), With<Viewmodel>>,
) {
    let Ok(held) = held.single() else { return };
    for (mut performance, mut stack) in &mut views {
        perform(held, shared.as_deref(), &mut performance, &mut stack, &clips, time.delta_secs());
    }
}

/// Every frame: the placement is a fraction of the frustum, so it changes with
/// the window's aspect. Then the viewmodel's clips move it, in eye space:
/// `Space::Carry` is the framed weapon and `Space::Eye` the camera.
fn place_the_viewmodel(
    clips: Res<Assets<Clip>>,
    held: Query<(&HeldModel, &Skeleton), With<LocalPlayer>>,
    mut views: Query<(&mut Viewmodel, &ClipStack)>,
    cameras: Query<(&Camera, &Projection, &Children, &ChildOf), With<ViewmodelCamera>>,
    mut parts: Query<&mut Transform, With<ViewmodelPart>>,
) {
    let Ok((held, skeleton)) = held.single() else { return };
    let spec = held.viewmodel();

    for (camera, projection, children, view) in &cameras {
        let Projection::Perspective(perspective) = projection else { continue };
        let Some(size) = camera.logical_viewport_size().filter(|size| size.y > 0.0) else {
            continue;
        };
        let Ok((mut viewmodel, stack)) = views.get_mut(view.parent()) else { continue };

        let framed = spec.transform(perspective.fov, size.x / size.y);
        let placed = played(framed, stack, skeleton, &clips);
        if viewmodel.weapon != Some(placed) {
            viewmodel.weapon = Some(placed);
        }
        let grips = grip_offsets(stack, &clips, skeleton);
        if viewmodel.grips != grips {
            viewmodel.grips = grips;
        }
        for child in children.iter() {
            let Ok(mut transform) = parts.get_mut(child) else { continue };
            // Compared first: this runs every frame, and an unchanged
            // transform marked changed is a propagation for nothing.
            transform.set_if_neq(placed);
        }
    }
}

/// `framed` moved by whatever the stack is playing.
fn played(framed: Transform, stack: &ClipStack, skeleton: &Skeleton, clips: &Assets<Clip>) -> Transform {
    if stack.0.is_empty() {
        return framed;
    }
    let ground = |_: Vec3| 0.0;
    let frames = Frames {
        body: eye_space(skeleton),
        carry: framed,
        weapon: Some(framed),
        eye: Some(Transform::IDENTITY),
        ground: &ground,
        footfalls: None,
    };
    // The pose is thrown away: the arms are solved onto the weapon afterwards.
    let mut pose = Pose::rest();
    let mut moving = framed;
    apply_stack(stack, clips, skeleton, &mut pose, &frames, Some(&mut moving));
    moving
}

/// `muzzle` in the play camera's frame, for a weapon drawn at `weapon` in the
/// viewmodel camera's. The two cameras share a frame but not a field of view,
/// so the point the viewmodel draws is moved to where the play camera sees the
/// same pixel, at the same depth.
pub fn muzzle_in_view(weapon: &Transform, muzzle: Vec3, viewmodel_fov: f32, play_fov: f32) -> Vec3 {
    let drawn = weapon.transform_point(muzzle);
    let widen = (play_fov / 2.0).tan() / (viewmodel_fov / 2.0).tan();
    Vec3::new(drawn.x * widen, drawn.y * widen, drawn.z)
}

/// From where the weapon was drawn, kick and all, so an own tracer leaves the
/// barrel rather than where the barrel rests. That is last frame's drawing:
/// this runs before `Update` places this frame's, and a kick a frame old is
/// well inside a muzzle flash.
pub fn find_the_apparent_muzzle(
    view: Res<ViewMode>,
    held: Query<&HeldModel, With<LocalPlayer>>,
    viewmodels: Query<(&Camera, &Projection), With<ViewmodelCamera>>,
    plays: Query<(&Transform, &Projection, Option<&Viewmodel>), With<PlayerCamera>>,
    mut apparent: ResMut<ApparentMuzzle>,
) {
    let found = || {
        if view.shows_own_body() {
            return None;
        }
        let held = held.single().ok()?;
        let muzzle = held.muzzle()?;
        let (camera, Projection::Perspective(viewmodel)) = viewmodels.single().ok()? else {
            return None;
        };
        let (eye, Projection::Perspective(play), drawn) = plays.single().ok()? else { return None };
        let size = camera.logical_viewport_size().filter(|size| size.y > 0.0)?;
        let weapon = drawn
            .and_then(|drawn| drawn.weapon)
            .unwrap_or_else(|| held.viewmodel().transform(viewmodel.fov, size.x / size.y));
        let seen = muzzle_in_view(&weapon, muzzle, viewmodel.fov, play.fov);
        Some(eye.transform_point(seen))
    };
    apparent.set_if_neq(ApparentMuzzle(found()));
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
        viewmodel.arms_placed_for = None;

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

/// Solve the arms onto the viewmodel's weapon, whenever what they are solved
/// from has changed.
///
/// An arm that did not reach is hidden: `grip_with` leaves it where it was,
/// which in front of an eye is a forearm across the screen. Most weapons have
/// no support grip, so usually only one arm is drawn. Solved only when the rig,
/// held model or frustum changes.
fn place_the_viewmodel_arms(
    bodies: Query<(Entity, Ref<Skeleton>, Ref<HeldModel>), With<LocalPlayer>>,
    cameras: Query<(&Camera, &Projection, &Children, &ChildOf), With<ViewmodelCamera>>,
    mut views: Query<&mut Viewmodel>,
    mut parts: Query<(&ViewmodelArm, &mut Transform, &mut Visibility)>,
) {
    let Ok((body, skeleton, held)) = bodies.single() else { return };

    for (camera, projection, children, view) in &cameras {
        let Projection::Perspective(perspective) = projection else { continue };
        let Some(size) = camera.logical_viewport_size().filter(|size| size.y > 0.0) else {
            continue;
        };
        let aspect = size.x / size.y;

        let Ok(mut viewmodel) = views.get_mut(view.parent()) else { continue };
        let weapon = viewmodel
            .weapon
            .unwrap_or_else(|| held.viewmodel().transform(perspective.fov, aspect));
        let key = ArmsKey {
            body,
            skeleton: skeleton.last_changed(),
            held: held.last_changed(),
            fov: perspective.fov.to_bits(),
            aspect: aspect.to_bits(),
            weapon: bits(&weapon),
            grips: viewmodel.grips.map(|grip| grip.to_array().map(f32::to_bits)),
        };
        if viewmodel.arms_placed_for == Some(key) {
            continue;
        }
        viewmodel.arms_placed_for = Some(key);

        let skeleton: &Skeleton = &skeleton;
        let root = eye_space(skeleton);

        // From rest: a camera that does not bob must not have arms that do.
        let mut pose = Pose::rest();
        let reached = grip_hands(skeleton, &mut pose, &root, held.hold(), &weapon, [true; 2], viewmodel.grips);
        let posed = skeleton.posed_bones(&pose, &root);

        for child in children.iter() {
            let Ok((arm, mut transform, mut visibility)) = parts.get_mut(child) else { continue };
            let wanted = match reached[arm.hand] {
                Reach::Reached => Visibility::Inherited,
                _ => Visibility::Hidden,
            };
            visibility.set_if_neq(wanted);

            let bone = &posed[arm.bone];
            // The mesh is already bone-sized; a scale would skew its normals.
            transform.set_if_neq(Transform {
                translation: bone.head,
                rotation: bone.rotation,
                ..default()
            });
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

    use strum::IntoEnumIterator;
    use crate::common::class::Class;
    use crate::common::skeleton::humanoid;
    use crate::common::skeleton::ik::Reach;
    use crate::prop::document;
    use crate::prop::hold::{grip_with, HoldSpec, ViewmodelSpec};

    use super::*;


    /// An arm that comes up short is left where it was, so a weapon floats in
    /// front of hands elsewhere. A viewmodel is not a hold, so the body tests
    /// do not cover this.
    #[test]
    fn every_class_can_reach_the_viewmodel() {
        let hold = HoldSpec::default();
        let weapon = ViewmodelSpec::default().transform(VIEWMODEL_FOV, 16.0 / 9.0);

        for class in Class::iter() {
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
        for class in Class::iter() {
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
        for class in Class::iter() {
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

    /// Breaks if a shot's apparent start drifts off the barrel as the
    /// play camera's field of view differs from the viewmodel's.
    #[test]
    fn the_apparent_muzzle_is_on_the_pixel_the_barrel_is_drawn_at() {
        let spec = crate::prop::hold::ViewmodelSpec {
            across: [0.6, -0.5],
            depth: 0.4,
            rotation: [0.1, 0.3, 0.0],
        };
        let muzzle = Vec3::new(0.0, 0.05, -0.5);
        let play_fov = std::f32::consts::FRAC_PI_2;

        for aspect in [4.0 / 3.0, 16.0 / 9.0, 32.0 / 9.0] {
            let on_screen = |point: Vec3, fov: f32| {
                let half = (fov / 2.0).tan();
                Vec2::new(point.x / (-point.z * half * aspect), point.y / (-point.z * half))
            };
            let weapon = spec.transform(VIEWMODEL_FOV, aspect);
            let drawn = weapon.transform_point(muzzle);
            let seen = muzzle_in_view(&weapon, muzzle, VIEWMODEL_FOV, play_fov);

            let (want, got) = (on_screen(drawn, VIEWMODEL_FOV), on_screen(seen, play_fov));
            assert!(want.abs_diff_eq(got, 1e-5), "drawn at {want}, seen at {got}");
            assert_eq!(seen.z, drawn.z, "the depth moved");
        }
    }

    /// A kick moves where an own tracer starts: the apparent muzzle follows
    /// the weapon as drawn, not as framed.
    #[test]
    fn the_apparent_muzzle_follows_the_kick() {
        let mut world = World::new();
        world.insert_resource(ViewMode::FirstPerson);
        world.init_resource::<ApparentMuzzle>();
        world.spawn((LocalPlayer, HeldModel::with_muzzle(Vec3::new(0.0, 0.0, -0.5))));
        let mut camera = Camera::default();
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(1600, 900),
            scale_factor: 1.0,
        });
        let projection = |fov| Projection::Perspective(PerspectiveProjection { fov, ..default() });
        world.spawn((ViewmodelCamera, camera, projection(VIEWMODEL_FOV)));
        let framed = ViewmodelSpec::default().transform(VIEWMODEL_FOV, 16.0 / 9.0);
        let view = world
            .spawn((PlayerCamera, Transform::IDENTITY, projection(VIEWMODEL_FOV), Viewmodel {
                weapon: Some(framed),
                ..default()
            }))
            .id();

        let muzzle = |world: &mut World| {
            world.run_system_once(find_the_apparent_muzzle).unwrap();
            world.resource::<ApparentMuzzle>().0.expect("no muzzle found")
        };
        let resting = muzzle(&mut world);
        let kick = Vec3::new(0.0, 0.02, 0.06);
        world.get_mut::<Viewmodel>(view).unwrap().weapon =
            Some(Transform { translation: framed.translation + kick, ..framed });
        let kicked = muzzle(&mut world);
        assert!((kicked - resting).abs_diff_eq(kick, 1e-5), "the muzzle moved {} for a kick of {kick}", kicked - resting);
    }

    /// Drawn over the world and under the UI.
    #[test]
    fn it_sits_between_the_world_and_the_interface() {
        assert!(VIEWMODEL_CAMERA_ORDER > PLAY_CAMERA_ORDER, "the viewmodel draws under the world");
        assert!(
            VIEWMODEL_CAMERA_ORDER < crate::game::hud::HUD_CAMERA_ORDER,
            "the viewmodel draws over the HUD",
        );
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

    /// A viewmodel camera with a play camera above it and the four arm bones
    /// under it, and a body to hold them — what `dress_the_viewmodel_arms`
    /// leaves, without the meshes it needs a renderer for.
    fn a_viewmodel_with_arms(world: &mut World) -> (Entity, Entity, Vec<Entity>) {
        let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Soldier));
        let arms: Vec<(usize, usize)> = ARM_BONES
            .iter()
            .map(|(name, hand)| (skeleton.index_of(name).expect("a rig has arms"), *hand))
            .collect();
        let body = world.spawn((LocalPlayer, skeleton, HeldModel::default())).id();

        let mut camera = Camera::default();
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(1600, 900),
            scale_factor: 1.0,
        });
        world.init_resource::<Assets<Clip>>();
        let view = world.spawn((Viewmodel::default(), ClipStack::default())).id();
        let camera = world
            .spawn((
                ViewmodelCamera,
                camera,
                Projection::Perspective(PerspectiveProjection { fov: VIEWMODEL_FOV, ..default() }),
                ChildOf(view),
            ))
            .id();
        let arms = arms
            .into_iter()
            .map(|(bone, hand)| {
                world
                    .spawn((
                        ViewmodelArm { bone, hand },
                        Transform::IDENTITY,
                        Visibility::Hidden,
                        ChildOf(camera),
                    ))
                    .id()
            })
            .collect();
        (body, camera, arms)
    }

    fn when_placed(world: &World, arms: &[Entity]) -> Vec<Tick> {
        arms.iter()
            .map(|arm| world.entity(*arm).get_ref::<Transform>().unwrap().last_changed())
            .collect()
    }

    /// Fails if the arms are solved again when nothing they read changed, or
    /// not solved again when something did.
    #[test]
    fn the_arms_are_solved_again_only_when_something_they_read_changes() {
        let mut world = World::new();
        let (body, camera, arms) = a_viewmodel_with_arms(&mut world);
        let mut schedule = Schedule::default();
        schedule.add_systems(place_the_viewmodel_arms);

        schedule.run(&mut world);
        let first = when_placed(&world, &arms);
        let trigger_arm = arms[0];
        assert_eq!(
            world.get::<Visibility>(trigger_arm),
            Some(&Visibility::Inherited),
            "the trigger hand was never placed on its grip",
        );

        schedule.run(&mut world);
        assert_eq!(when_placed(&world, &arms), first, "the arms were re-placed with nothing changed");

        // Prove the solve was skipped, not merely a no-op: a hand moved by hand stays moved.
        let solved = *world.get::<Transform>(trigger_arm).unwrap();
        world.get_mut::<Transform>(trigger_arm).unwrap().translation = Vec3::splat(9.0);
        schedule.run(&mut world);
        assert_eq!(
            world.get::<Transform>(trigger_arm).unwrap().translation,
            Vec3::splat(9.0),
            "the arms were solved again with nothing they read changed",
        );
        *world.get_mut::<Transform>(trigger_arm).unwrap() = solved;

        // A wider field of view moves the weapon, and the hands with it.
        if let Projection::Perspective(perspective) =
            &mut *world.get_mut::<Projection>(camera).unwrap()
        {
            perspective.fov = 90.0_f32.to_radians();
        }
        schedule.run(&mut world);
        let at_ninety = *world.get::<Transform>(trigger_arm).unwrap();
        assert_ne!(at_ninety, solved, "a new field of view left the arms where they were");

        // So does a window of another shape.
        world.get_mut::<Camera>(camera).unwrap().computed.target_info =
            Some(bevy::camera::RenderTargetInfo {
                physical_size: UVec2::new(900, 900),
                scale_factor: 1.0,
            });
        schedule.run(&mut world);
        assert_ne!(
            *world.get::<Transform>(trigger_arm).unwrap(),
            at_ninety,
            "a resized window left the arms where they were",
        );

        // And a weapon switch, which is the held model being put back.
        world.entity_mut(body).insert(HeldModel::default());
        world.get_mut::<Transform>(trigger_arm).unwrap().translation = Vec3::splat(9.0);
        schedule.run(&mut world);
        assert_ne!(
            world.get::<Transform>(trigger_arm).unwrap().translation,
            Vec3::splat(9.0),
            "a new held model did not put the hands back on it",
        );
    }

    /// Fails if rebuilt arms for unchanged inputs are left hidden.
    #[test]
    fn rebuilt_arms_are_placed_even_when_nothing_else_changed() {
        let mut world = World::new();
        let (_, _, arms) = a_viewmodel_with_arms(&mut world);
        let mut schedule = Schedule::default();
        schedule.add_systems(place_the_viewmodel_arms);
        schedule.run(&mut world);

        // What `dress_the_viewmodel_arms` does on a rebuild.
        let mut views = world.query::<&mut Viewmodel>();
        for mut viewmodel in views.iter_mut(&mut world) {
            viewmodel.arms_placed_for = None;
        }
        world.entity_mut(arms[0]).insert((Transform::IDENTITY, Visibility::Hidden));

        schedule.run(&mut world);
        assert_eq!(world.get::<Visibility>(arms[0]), Some(&Visibility::Inherited));
        assert_ne!(world.get::<Transform>(arms[0]), Some(&Transform::IDENTITY));
    }

    /// Fails if an unmoved weapon is marked moved by the every-frame placing.
    #[test]
    fn an_unmoved_viewmodel_is_not_marked_moved() {
        let mut world = World::new();
        let (_, camera, _) = a_viewmodel_with_arms(&mut world);
        let part = world.spawn((ViewmodelPart, Transform::IDENTITY, ChildOf(camera))).id();
        let mut schedule = Schedule::default();
        schedule.add_systems(place_the_viewmodel);

        schedule.run(&mut world);
        let first = when_placed(&world, &[part]);
        schedule.run(&mut world);
        assert_eq!(when_placed(&world, &[part]), first, "an unchanged weapon was marked moved");
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

