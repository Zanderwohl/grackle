//! Skeletons as ECS bodies: one entity carries the rig, the situation it is
//! in, and the state machine between them.
//!
//! A body is any entity with a [`Skeleton`] and a [`Pose`]. Add
//! [`SkeletonAnimator`] and it animates itself; add [`BodyRequests`] and
//! whatever knows what is happening — input here, an NPC's planner or the
//! network later — describes the situation without choosing an animation. Add
//! [`ForcedAnimation`] instead and it holds one state, which is what the
//! editor's Animation Display does.
//!
//! Everything is on one entity so that a writer need not walk to a child.
//!
//! Only the two player systems are gated on [`AppMode`]: the editor animates
//! skeletons too. Drawing lives here rather than in `GamePlugin` because
//! `Gizmos` needs renderer resources the headless tests lack.

use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::common::app_mode::{showing_the_world, AppMode};
use crate::common::class::Stance;
use crate::common::damage::PlayerId;
use crate::common::class::{Class, ClassBuilds};
use crate::common::skeleton::ik::finish_pose;
use crate::common::skeleton::play::{ClipStack, Layer, Regions};
use crate::common::skeleton::state::{AnimationState, ClipLibrary};
use crate::common::skeleton::rig::bone;
use crate::common::skeleton::{
    draw_skeleton, animator_pose, humanoid, leg_length, AnimationClock, AnimationPhase, BodyRequests,
    direction_of, DisplaySpeed, ForcedAnimation, Gait, Pose, PoseInputs, Skeleton,
    SkeletonAnimator,
    SkeletonPalette,
};
use crate::game::body_mesh::BodyMeshPlugin;
use crate::game::condition::Condition;
use crate::game::held::HeldWeaponPlugin;
use crate::game::viewmodel::ViewmodelPlugin;
use crate::game::hitbox::HitboxPlugin;
use crate::game::weapon::WeaponPlugin;
use crate::game::player::{
    step_player, Inputs, LocalPlayer, PhysicsBody, Player, ViewMode, PLAYER_HALF,
};

/// Where a skeleton's feet sit relative to the entity carrying it.
///
/// A rig is placed by its feet, but a body's origin is the centre of its
/// collision box. A component rather than a child entity holds the difference.
#[derive(Component, Clone, Copy, Debug)]
pub struct SkeletonRoot(pub Vec3);

/// Whether the bones are drawn on top of the mesh. `F6` toggles it.
///
/// Off by default: the mesh is meant to cover the bones.
#[derive(Resource, Debug, Default)]
pub struct ShowBones(pub bool);

pub struct SkeletonPlugin;

impl Plugin for SkeletonPlugin {
    fn build(&self, app: &mut App) {
        app
            // Need the same renderer resources this plugin does.
            .add_plugins((HitboxPlugin, WeaponPlugin))
            // Ungated: the editor shows bodies, and bodies in an animation
            // grid hold weapons and wear hats too.
            .add_plugins((BodyMeshPlugin, HeldWeaponPlugin, crate::game::hat::HatPlugin, ViewmodelPlugin))
            .add_plugins((crate::game::recoil::RecoilPlugin, crate::game::reloading::ReloadingPlugin))
            .add_plugins(GameTimePlugin)
            .init_resource::<ShowBones>()
            .init_resource::<ClassBuilds>()
            .add_systems(Update, toggle_bones)
            // Here rather than in `ConditionPlugin`: the clips are assets, and
            // the game's tests run without the asset plugins.
            .add_systems(Startup, crate::game::condition::load_condition_clips)
            .add_systems(Update, crate::game::condition::load_taunt_clips)
            .init_resource::<crate::common::skeleton::play::LayerBlends>()
            .add_systems(Startup, crate::common::layer_file::load_blends_at_startup)
            .add_systems(Update, (
                describe_player_bodies
                    .run_if(in_state(AppMode::Play))
                    .run_if(crate::common::net::has_authority),
                // Not gated on `AppMode::Play`: a replicated body can arrive
                // before this process has finished entering the round, and a
                // body that misses its rig stays invisible.
                rebuild_rigs_for_new_builds,
                dress_new_players,
                dress_bodies_from_elsewhere,
                equip_new_bodies,
                phase_bodies_by_id,
                // After the writers, so a body animates on this frame's
                // situation.
                advance_animators,
                look_with_the_head,
                // Placing, clip playback and gripping are three systems because
                // a `Space::Weapon` clip track is read against a placed weapon,
                // and the arms are solved onto wherever the clip left it.
                //
                // A clip's leg track wins over `plant_feet` because planting
                // runs inside `finish_pose`, already done by here; `Free` axes
                // read the planted tip. If body animations move into
                // `finish_pose`, a clip would run before planting and the plant
                // would need suppressing for legs a clip owns.
                crate::game::held::place_the_weapon,
                // In the chain so starting and playing a clip are never a
                // frame apart.
                crate::game::held::perform_weapon_clips,
                crate::game::condition::perform_conditions,
                crate::game::held::fade_the_layers,
                crate::game::held::play_the_clips,
                crate::game::held::grip_the_weapon,
            ).chain())
            // After propagation, so a rig is drawn where its body is now,
            // after `interpolate_bodies`. Gated because a gizmo is not an
            // entity, so the prop editor's hiding cannot reach it.
            .add_systems(
                PostUpdate,
                draw_skeletons
                    .after(TransformSystems::Propagate)
                    .run_if(showing_the_world),
            )
        ;
    }
}

/// Game time: the clocks a match runs on, and the switch that stops them.
///
/// Not run in the editor: a room is easier to line up against a body holding
/// still. `advance_animators` is not frozen, so a display still blends into a
/// state picked in the panel; with the clock frozen that gives a held frame.
pub struct GameTimePlugin;

impl Plugin for GameTimePlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<AnimationClock>()
            // On the tick: every viewer of a body has to agree on the clock.
            .add_systems(FixedUpdate, (
                // Not on replayed ticks: nothing rolls the clock or a stride
                // back, so a replay would advance them twice.
                (
                    advance_animation_clock,
                    // After the step, since it advances on the ground the step covered.
                    advance_gaits.after(step_player),
                ).run_if(not(lightyear::prelude::is_in_rollback)),
                follow_stance.after(step_player),
            ).run_if(in_state(AppMode::Play)))
        ;
    }
}

fn toggle_bones(keys: Res<ButtonInput<KeyCode>>, mut show: ResMut<ShowBones>) {
    if keys.just_pressed(KeyCode::F6) {
        show.0 = !show.0;
    }
}

/// One fixed step of the shared clock.
///
/// `Time<Fixed>`'s delta inside `FixedUpdate` is exactly one tick, whatever
/// the frame rate.
pub fn advance_animation_clock(time: Res<Time<Fixed>>, mut clock: ResMut<AnimationClock>) {
    clock.advance(time.delta_secs());
}

/// Gives every rig a [`Gait`], so no spawner can forget it.
///
/// Not [`Hitboxes`](crate::common::hitbox::Hitboxes): those come from
/// `#[require]` on the markers of a real body ([`Player`], `CarouselBody`,
/// `AnimationDisplayMarker`), so a spawn point's preview has none.
fn equip_new_bodies(
    mut commands: Commands,
    bodies: Query<Entity, (With<Skeleton>, Without<Gait>)>,
) {
    for body in &bodies {
        commands.entity(body).insert_if_new(Gait::default());
    }
}

/// Keep the rig's feet on the bottom of the hull as the hull changes height.
///
/// The offset changes with stance because ducking halves the box. On the tick
/// beside the step, so the crouch and its drawing land together.
pub fn follow_stance(mut bodies: Query<(&Stance, &mut SkeletonRoot), Changed<Stance>>) {
    for (stance, mut root) in &mut bodies {
        root.0 = Vec3::NEG_Y * stance.half_extents().y;
    }
}

/// Advance every body's stride by the ground it covered this tick.
///
/// Off the step's displacement, so the cycle is a function of the simulation
/// and not of frame rate.
pub fn advance_gaits(
    time: Res<Time<Fixed>>,
    mut bodies: Query<(
        &Skeleton,
        &SkeletonAnimator,
        &GlobalTransform,
        Option<&PhysicsBody>,
        Option<&DisplaySpeed>,
        &mut Gait,
    )>,
) {
    let dt = time.delta_secs();
    for (skeleton, animator, global, physics, display, mut gait) in &mut bodies {
        // Reset on stopping, so setting off does not begin mid-swing.
        if !animator.state().uses_gait() {
            gait.reset();
            continue;
        }

        let leg = leg_length(skeleton);
        let moved = match physics {
            // Horizontal ground covered, in the body's own frame: falling is
            // not walking.
            Some(body) => {
                let world = (body.current - body.previous).xz();
                let facing = global.rotation();
                let right = (facing * Vec3::X).xz();
                let forward = (facing * Vec3::NEG_Z).xz();
                Vec2::new(world.dot(right), world.dot(forward))
            }
            // A body shown rather than played: the ground its state says it
            // would cover.
            None => {
                direction_of(animator.state()) * display.copied().unwrap_or_default().0 * leg * dt
            }
        };
        gait.advance(moved, leg, dt);
    }
}

/// Where a body's feet are. Shared by animating, drawing and hit boxing so
/// they cannot disagree.
pub fn skeleton_root(global: &GlobalTransform, offset: Option<&SkeletonRoot>) -> Transform {
    let mut root = global.compute_transform();
    if let Some(SkeletonRoot(offset)) = offset {
        root.translation += root.rotation * *offset;
    }
    root
}

/// How far along an axis of `PlayerInput::movement` (-1 to 1) counts as asking
/// to move that way.
const MOVING: f32 = 0.5;

/// Describe what a body we are simulating is doing.
///
/// Authority only: `BodyRequests` is replicated, and a client has no `Inputs`
/// for remote bodies, so describing them would flatten them to standing.
///
/// What the body may do, not only what was pressed: a taunt that stops the
/// body is not running, and described as running it freezes mid-stride, since
/// no ground is covered for the stride to advance on.
fn describe_player_bodies(
    taunts: Option<Res<crate::common::taunt::Taunts>>,
    mut players: Query<(&Player, &Stance, &Inputs, &Condition, &mut BodyRequests)>,
) {
    for (player, stance, input, condition, mut requests) in &mut players {
        let movement = match condition.speed_factor(taunts.as_deref()) > 0.0 {
            true => input.movement,
            false => Vec2::ZERO,
        };
        let mut described = BodyRequests {
            running_forward: movement.y > MOVING,
            running_backward: movement.y < -MOVING,
            strafing_left: movement.x < -MOVING,
            strafing_right: movement.x > MOVING,
            airborne: !player.on_ground,
            // The stance, not the key: a body that cannot stand up under a
            // vent is still crouched.
            crouching: *stance == Stance::Crouched,
            wall_ahead: false,
        };
        // The step is the only thing that knows a wall was hit.
        described.wall_ahead = described.moving() && (player.blocked.x || player.blocked.z);
        // Built whole and compared: `BodyRequests` is replicated.
        requests.set_if_neq(described);
    }
}

/// Give every body a skeleton built from its class.
///
/// Keyed on the absence of a rig, not on `Added<Class>`: `Class` is
/// `replicate_once` and can arrive frames after the body (measured three in
/// eight late for corpses), and an undressed body is invisible rather than an
/// error.
///
/// Must not decide a corpse's build: `CorpseBuild` is read off the rig.
fn dress_new_players(
    mut commands: Commands,
    builds: Res<ClassBuilds>,
    players: Query<(Entity, &Class), (With<Player>, Without<Skeleton>)>,
) {
    for (player, class) in &players {
        // `insert_if_new`: a replicated body may already carry the server's
        // `Stance`.
        commands.entity(player).insert_if_new((
            humanoid(builds.of(*class)),
            Pose::rest(),
            SkeletonAnimator::default(),
            BodyRequests::default(),
            Stance::default(),
            // Replaced by `phase_bodies_by_id` once a `PlayerId` arrives,
            // which on a replicated body is a separate message.
            AnimationPhase::default(),
            SkeletonRoot(Vec3::NEG_Y * PLAYER_HALF.y),
        ));
    }
}

/// How much of a body's pitch each joint takes.
///
/// Split because a head alone at eighty degrees reads as a broken neck. The
/// shares sum to one.
const NECK_SHARE: f32 = 0.4;
const HEAD_SHARE: f32 = 1.0 - NECK_SHARE;

/// How far the head will follow the look direction, up and down.
///
/// The head stops at these; the aim does not. Further down than up.
const HEAD_PITCH_UP: f32 = 45.0 * std::f32::consts::PI / 180.0;
const HEAD_PITCH_DOWN: f32 = -70.0 * std::f32::consts::PI / 180.0;

/// How far the head actually turns for a given look direction.
///
/// The one place `Player.pitch` is narrowed, so the aim stays the truth.
fn head_pitch(look: f32) -> f32 {
    look.clamp(HEAD_PITCH_DOWN, HEAD_PITCH_UP)
}

/// Tilt the head and neck to where the body is looking.
///
/// A spine bone's positive local `X` rotation is down, and pitch is positive
/// up, hence the negation. Composes onto the pose `advance_animators` left.
/// Runs on every `Player`, since `pitch` is replicated; yaw is
/// [`face_bodies`](crate::game::player::face_bodies)'s.
fn look_with_the_head(mut bodies: Query<(&Player, &mut Pose)>) {
    for (player, mut pose) in &mut bodies {
        if player.pitch == 0.0 && pose.joint(bone::NECK) == Quat::IDENTITY {
            // Skipping keeps a still body from dirtying its pose every frame.
            continue;
        }
        look(&mut pose, player.pitch);
    }
}

/// Bend the neck and head towards `pitch`, onto whatever the pose has them
/// doing. Shared with the hitboxes, which must put the head where it is drawn.
pub(crate) fn look(pose: &mut Pose, pitch: f32) {
    let pitch = head_pitch(pitch);
    for (joint, share) in [(bone::NECK, NECK_SHARE), (bone::HEAD, HEAD_SHARE)] {
        let animated = pose.joint(joint);
        pose.set(joint, animated * Quat::from_rotation_x(-pitch * share));
    }
}

/// Dress a body that arrived from somewhere else.
///
/// A non-player body arrives with only a `Class`, a `ForcedAnimation` and a
/// position; what grid it belongs to stays on the authority. `insert_if_new`,
/// so a body the authority stood up keeps its own rig.
///
/// `Without<Player>` keeps this disjoint from [`dress_new_players`]: this one
/// inserts no `SkeletonRoot`, so a player dressed here floats half a metre up.
fn dress_bodies_from_elsewhere(
    mut commands: Commands,
    builds: Res<ClassBuilds>,
    bodies: Query<(Entity, &Class), (Added<Class>, Without<Skeleton>, Without<Player>)>,
) {
    for (body, class) in &bodies {
        commands.entity(body).insert_if_new((
            humanoid(builds.of(*class)),
            Pose::rest(),
            SkeletonAnimator::default(),
            BodyRequests::default(),
            AnimationPhase::default(),
            // `CarouselBody` would require these, but a viewer is never sent
            // one.
            crate::common::hitbox::Hitboxes::default(),
            // `Player` requires these; a non-player arrives without them.
            Transform::default(),
            Visibility::default(),
        ));
    }
}

/// A client rigs its bodies from its own pack until the server's builds
/// arrive, and they may arrive after the bodies. Corpses keep the build they
/// died with.
fn rebuild_rigs_for_new_builds(
    mut commands: Commands,
    builds: Res<ClassBuilds>,
    bodies: Query<(Entity, &Class, &Skeleton), Without<crate::game::ragdoll::Ragdoll>>,
) {
    if !builds.is_changed() {
        return;
    }
    for (body, class, skeleton) in &bodies {
        let wanted = builds.of(*class);
        if skeleton.proportions() != wanted {
            commands.entity(body).insert(humanoid(wanted));
        }
    }
}

/// Put each body at its own point in the animation cycle, from its network id.
///
/// Bodies bouncing in step read as one machine. `PlayerId` is replicated, so
/// every viewer agrees; an `Entity` index or an RNG would not.
fn phase_bodies_by_id(
    mut commands: Commands,
    // Either arrival, because they arrive in either order: a replicated body's
    // id comes in its own message, and its rig waits on its `Class`, which
    // does too. Keyed on the id alone, an id that beat the rig was spent on a
    // body with nothing to phase and never looked at again.
    bodies: Query<(Entity, &PlayerId), (With<Skeleton>, Or<(Added<PlayerId>, Added<Skeleton>)>)>,
) {
    for (body, id) in &bodies {
        commands.entity(body).insert(AnimationPhase::from_id(id.0));
    }
}

/// Run every body's state machine and write the pose it produces.
///
/// The only writer of an animated body's [`Pose`]. A look-at or IK pass runs
/// after this and edits the pose it left.
fn advance_animators(
    time: Res<Time>,
    clock: Res<AnimationClock>,
    // Absent: every state is a Rust function.
    library: Option<Res<ClipLibrary>>,
    mut bodies: Query<(
        &Skeleton,
        &mut SkeletonAnimator,
        &mut Pose,
        &GlobalTransform,
        Option<&SkeletonRoot>,
        Option<&BodyRequests>,
        Option<&ForcedAnimation>,
        Option<&AnimationPhase>,
        Option<&Gait>,
        Option<&ClipStack>,
    )>,
) {
    let dt = time.delta_secs();
    let clips = library.as_deref();
    for (skeleton, mut animator, mut pose, global, offset, requests, forced, phase, gait, stack) in
        &mut bodies
    {
        match forced {
            // Being shown rather than driven: requests, if any, are ignored.
            Some(ForcedAnimation(state)) => animator.force(*state, dt),
            None => animator.advance(&requests.copied().unwrap_or_default(), dt),
        }
        // The shared clock, not time-in-state, so all viewers see the same
        // point in the cycle.
        //
        // The root is last frame's, as propagation has not run. Harmless: the
        // corrections resolve to joint angles, so only facing matters.
        let gait = gait.copied().unwrap_or_default();
        let inputs = PoseInputs {
            seconds: clock.seconds() + phase.copied().unwrap_or_default().0,
            stride: gait.phase(),
            speed: gait.speed(),
            travel: gait.travel(),
        };
        let root = skeleton_root(global, offset);
        *pose = animator_pose(skeleton, &animator, &inputs, &root, clips);

        if let Some(stack) = stack {
            let still = AnimationState::from_requests(&BodyRequests {
                crouching: requests.is_some_and(|requests| requests.crouching),
                airborne: requests.is_some_and(|requests| requests.airborne),
                ..default()
            });
            stand_where_claimed(skeleton, &mut pose, stack, || {
                finish_pose(skeleton, still, &inputs, &root, clips)
            });
        }
    }
}

/// What a layer above the weapon claims is its own, so the animator's motion
/// there does not show through what the layer leaves unkeyed: a taunt that
/// claims the legs and keys none of them stands, rather than walking on the
/// spot beneath it. "Standing" is the state the body would be in with no
/// movement asked for, so a crouched taunt stays crouched. Blended by how
/// much the layer covers, so it fades with the layer.
///
/// Not the weapon layer's claims: the grip owns the arms there, and an arm
/// under a deploy still swings when the weapon has no grip for it.
pub(crate) fn stand_where_claimed(
    skeleton: &Skeleton,
    pose: &mut Pose,
    stack: &ClipStack,
    still: impl FnOnce() -> Pose,
) {
    let regions = [Regions::LEGS, Regions::TORSO, Regions::HEAD, Regions::ARM_L, Regions::ARM_R];
    let cover = regions.map(|region| stack.cover(region, Layer::Weapon));
    if cover.iter().all(|cover| *cover <= 0.0) {
        return;
    }
    let still = still();
    for bone in skeleton.bones() {
        let region = Regions::of_bone(bone.name);
        let Some(index) = regions.iter().position(|claimed| *claimed == region) else { continue };
        if cover[index] > 0.0 {
            pose.set(bone.name, pose.joint(bone.name).slerp(still.joint(bone.name), cover[index]));
        }
    }
    let legs = cover[0];
    if legs > 0.0 {
        pose.root_offset = pose.root_offset.lerp(still.root_offset, legs);
    }
}

/// Skips the body the camera is inside.
pub fn draw_skeletons(
    show: Res<ShowBones>,
    mut gizmos: Gizmos,
    view: Res<ViewMode>,
    bodies: Query<(&Skeleton, &Pose, &GlobalTransform, Option<&SkeletonRoot>, Option<&LocalPlayer>)>,
) {
    if !show.0 {
        return;
    }

    for (skeleton, pose, global, offset, own_body) in &bodies {
        // `LocalPlayer`, not `Player`: replicated bodies carry `Player` too.
        if own_body.is_some() && !view.shows_own_body() {
            continue;
        }
        let root = skeleton_root(global, offset);
        draw_skeleton(&mut gizmos, skeleton, pose, &root, &SkeletonPalette::SIDES);
    }
}

#[cfg(test)]
mod tests {
    use lightyear::prelude::input::native::ActionState;

    use crate::game::player::PlayerInput;

    use super::*;
    use crate::common::hitbox::Hitboxes;
    use crate::common::skeleton::rig::{humanoid, Proportions};
    use bevy::ecs::system::RunSystemOnce;

    /// One frame of animation. An animator handed a `dt` of zero never leaves
    /// its state, so every test below would pass for the wrong reason.
    fn tick(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.world_mut().run_system_once(advance_animators).unwrap();
    }

    /// One pass of both dressing systems, commands applied.
    fn dress(world: &mut World) {
        if !world.contains_resource::<ClassBuilds>() {
            world.insert_resource(ClassBuilds::shipped());
        }
        world.run_system_once(dress_new_players).unwrap();
        world.run_system_once(dress_bodies_from_elsewhere).unwrap();
        world.flush();
    }

    /// A body is built from its own class. Every player is a Mercenary, which
    /// is the default build, so a hardcoded default would also pass today.
    #[test]
    fn a_body_is_built_from_its_class() {
        let mut world = World::new();
        let body = world.spawn((Player::default(), Class::Heavy)).id();

        dress(&mut world);

        let rig = world.get::<Skeleton>(body).expect("a body with a class went undressed");
        assert_eq!(
            rig.proportions(), ClassBuilds::shipped().of(Class::Heavy),
            "the body was built from something other than its class"
        );
    }

    /// A body whose `Class` arrives late is dressed when it arrives, not never.
    #[test]
    fn a_body_whose_class_is_late_is_dressed_when_it_arrives() {
        let mut world = World::new();
        let body = world.spawn(Player::default()).id();

        dress(&mut world);
        assert!(
            world.get::<Skeleton>(body).is_none(),
            "a body with no class was dressed as something"
        );

        world.entity_mut(body).insert(Class::Sniper);
        dress(&mut world);

        let rig = world.get::<Skeleton>(body).expect("the class arrived and nothing dressed it");
        assert_eq!(rig.proportions(), ClassBuilds::shipped().of(Class::Sniper));
    }

    /// Fails if a body whose id arrived before its rig is never phased.
    #[test]
    fn a_body_whose_id_beat_its_rig_is_still_phased() {
        let mut world = World::new();
        world.insert_resource(ClassBuilds::default());
        let mut schedule = Schedule::default();
        schedule.add_systems((dress_new_players, phase_bodies_by_id).chain());

        // An id whose phase is not the zero a fresh rig is dressed with.
        let id = (0..).find(|&id| AnimationPhase::from_id(id).0 != 0.0).unwrap();
        let body = world.spawn((Player::default(), PlayerId(id))).id();
        schedule.run(&mut world);
        assert!(world.get::<Skeleton>(body).is_none(), "a body with no class was dressed");

        world.entity_mut(body).insert(Class::Mercenary);
        schedule.run(&mut world);

        assert_eq!(
            world.get::<AnimationPhase>(body).copied(),
            Some(AnimationPhase::from_id(id)),
            "the id came before the rig and the body was never phased"
        );
    }

    /// Fails if a body rigged before the server's builds arrived keeps the
    /// build it was given first.
    #[test]
    fn new_builds_rebuild_the_rigs_they_change() {
        let mut world = World::new();
        world.insert_resource(ClassBuilds::default());
        let body = world.spawn((Player::default(), Class::Heavy)).id();
        dress(&mut world);
        assert_eq!(world.get::<Skeleton>(body).unwrap().proportions(), Proportions::DEFAULT);

        world.insert_resource(ClassBuilds::shipped());
        world.run_system_once(rebuild_rigs_for_new_builds).unwrap();
        world.flush();

        assert_eq!(
            world.get::<Skeleton>(body).unwrap().proportions(),
            ClassBuilds::shipped().of(Class::Heavy),
        );
    }

    /// A player's body is dressed by `dress_new_players`, which gives it a
    /// `SkeletonRoot`; the other system would leave it floating.
    #[test]
    fn a_players_body_is_dressed_as_a_player() {
        let mut world = World::new();
        let player = world.spawn((Player::default(), Class::Mercenary)).id();
        let display = world.spawn(Class::Mercenary).id();

        dress(&mut world);

        assert!(
            world.get::<SkeletonRoot>(player).is_some(),
            "a player's rig has no root, so it is drawn at the wrong height"
        );
        assert!(
            world.get::<Skeleton>(display).is_some(),
            "a body that is not a player went undressed"
        );
        assert!(
            world.get::<SkeletonRoot>(display).is_none(),
            "a display body was given a player's root offset"
        );
    }

    /// Real bodies are boxed; a spawn point's preview is not.
    #[test]
    fn only_real_bodies_are_boxed() {
        use crate::editor::animation_display::AnimationDisplayMarker;
        use crate::editor::spawn_point::SpawnPointMarker;
        use crate::tool::animation_grid::CarouselBody;

        let mut world = World::new();
        let rig = || (humanoid(Proportions::DEFAULT), Pose::rest(), Transform::IDENTITY);

        let real = [
            world.spawn((CarouselBody(0), rig())).id(),
            world.spawn((AnimationDisplayMarker, rig())).id(),
            world.spawn((Player::default(), rig())).id(),
        ];
        let preview = world.spawn((SpawnPointMarker, rig())).id();

        world.run_system_once(equip_new_bodies).unwrap();

        for body in real {
            assert!(world.get::<Hitboxes>(body).is_some(), "a real body cannot be hit");
            assert!(world.get::<Gait>(body).is_some(), "a body with no stride");
        }
        assert!(
            world.get::<Hitboxes>(preview).is_none(),
            "a spawn point's preview was boxed as though it were in the game",
        );
        assert!(world.get::<Gait>(preview).is_some(), "the preview is not a body at all");
    }

    /// The editor holds a frame: the clock does not advance outside Play.
    #[test]
    fn game_time_does_not_run_in_the_editor() {
        use bevy::state::app::StatesPlugin;

        let mut app = App::new();
        app.add_plugins((StatesPlugin, bevy::time::TimePlugin));
        app.init_state::<AppMode>();
        app.add_plugins(GameTimePlugin);
        app.update();

        for _ in 0..8 {
            app.world_mut().run_schedule(FixedUpdate);
        }
        assert_eq!(app.world().resource::<AnimationClock>().ticks(), 0, "the editor was animating");

        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        for _ in 0..8 {
            app.world_mut().run_schedule(FixedUpdate);
        }
        assert_eq!(app.world().resource::<AnimationClock>().ticks(), 8, "playing did not start the clock");
    }

    /// A body's animation follows from `BodyRequests`, whoever wrote them.
    #[test]
    fn a_body_animates_from_whatever_wrote_its_requests() {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin);
        app.init_resource::<AnimationClock>();
        let body = app
            .world_mut()
            .spawn((
                humanoid(Proportions::DEFAULT),
                Pose::rest(),
                SkeletonAnimator::default(),
                BodyRequests { running_forward: true, ..default() },
                Transform::IDENTITY,
            ))
            .id();

        // Long enough that the dwell is not what is being tested.
        for _ in 0..8 {
            tick(&mut app);
        }

        let animator = app.world().get::<SkeletonAnimator>(body).unwrap();
        assert_eq!(animator.state(), AnimationState::RunForward);
    }

    /// A forced body ignores its requests.
    #[test]
    fn a_forced_body_ignores_its_requests() {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin);
        app.init_resource::<AnimationClock>();
        let body = app
            .world_mut()
            .spawn((
                humanoid(Proportions::DEFAULT),
                Pose::rest(),
                SkeletonAnimator::default(),
                BodyRequests { running_forward: true, ..default() },
                ForcedAnimation(AnimationState::Airborne),
                Transform::IDENTITY,
            ))
            .id();

        for _ in 0..8 {
            tick(&mut app);
        }

        assert_eq!(
            app.world().get::<SkeletonAnimator>(body).unwrap().state(),
            AnimationState::Airborne
        );
    }

    /// Which way the head's face points, in world space.
    ///
    /// The head bone's rest rotation is not identity: its local `-Z` points
    /// behind the body, so the face is `rotation * Z`.
    fn face_of(pose: &Pose) -> Vec3 {
        let skeleton = humanoid(Proportions::DEFAULT);
        let bones = skeleton.posed_bones(pose, &Transform::IDENTITY);
        let head = bones
            .iter()
            .find(|bone| bone.name == bone::HEAD)
            .expect("no head on the rig");
        head.rotation * Vec3::Z
    }

    fn face_at(pitch: f32) -> Vec3 {
        let mut app = App::new();
        let body = app
            .world_mut()
            .spawn((Player { pitch, ..default() }, Pose::rest()))
            .id();
        app.world_mut().run_system_once(look_with_the_head).unwrap();
        face_of(app.world().get::<Pose>(body).unwrap())
    }

    /// The convention every other assertion here rests on: a body at rest
    /// faces the way it walks, `-Z`. Fails if the head's rest rotation is
    /// re-authored, instead of quietly inverting everyone's aim.
    #[test]
    fn a_body_at_rest_faces_the_way_it_walks() {
        let face = face_of(&Pose::rest());
        assert!(
            face.abs_diff_eq(Vec3::NEG_Z, 1e-4),
            "the rig no longer faces -Z at rest; it faces {face}"
        );
    }

    /// Looking up points the face up, and looking down points it down.
    /// Looking up points the face up. Backwards, heads tilt away from where
    /// their owner is looking.
    #[test]
    fn pitching_up_points_the_face_up() {
        let level = face_at(0.0);
        let up = face_at(0.8);
        let down = face_at(-0.8);

        assert!(level.y.abs() < 1e-3, "a level body is not looking level: {level}");
        assert!(up.y > 0.5, "looking up did not point the face up: {up}");
        assert!(down.y < -0.5, "looking down did not point the face down: {down}");
        assert!(up.z < 0.0 && down.z < 0.0, "the head turned to face backwards");
    }

    /// The head stops at its limits, further down than up, while the aim goes on.
    #[test]
    fn the_head_stops_short_of_the_aim() {
        let up = face_at(1.5);
        let down = face_at(-1.5);

        assert!(
            (up.y - HEAD_PITCH_UP.sin()).abs() < 1e-3,
            "the head reached {} looking up rather than stopping at {}",
            up.y.asin().to_degrees(),
            HEAD_PITCH_UP.to_degrees(),
        );
        assert!(
            (down.y - HEAD_PITCH_DOWN.sin()).abs() < 1e-3,
            "the head reached {} looking down rather than stopping at {}",
            down.y.asin().to_degrees(),
            HEAD_PITCH_DOWN.to_degrees(),
        );
        assert!(
            down.y.abs() > up.y.abs(),
            "a body should be able to look further down than up",
        );

        // Within the limits it still tracks exactly.
        let modest = face_at(0.3);
        assert!((modest.y - 0.3_f32.sin()).abs() < 1e-3, "the head lagged its own aim");
    }

    /// The head takes the larger share and the two sum to the whole pitch.
    #[test]
    fn the_neck_and_the_head_share_the_whole_pitch() {
        let mut app = App::new();
        let pitch = 0.7;
        let body = app
            .world_mut()
            .spawn((Player { pitch, ..default() }, Pose::rest()))
            .id();
        app.world_mut().run_system_once(look_with_the_head).unwrap();

        let pose = app.world().get::<Pose>(body).unwrap();
        // Negated on the way in, so read back negated.
        let angle_of = |joint: &str| -pose.joint(joint).to_euler(EulerRot::XYZ).0;
        let neck = angle_of(bone::NECK);
        let head = angle_of(bone::HEAD);

        assert!(head > neck, "the neck bent further than the head");
        assert!(
            (neck + head - pitch).abs() < 1e-4,
            "the shares came to {} rather than {pitch}",
            neck + head
        );
    }

    /// Input says forward and the step says blocked: the body is described as
    /// pushing a wall.
    #[test]
    fn a_player_pressed_against_a_wall_is_described_as_pushing() {
        let mut app = App::new();
        let player = app
            .world_mut()
            .spawn((
                Player { on_ground: true, blocked: BVec3::new(false, false, true), ..default() },
                ActionState(PlayerInput { movement: Vec2::new(0.0, 1.0), ..default() }),
                Stance::Standing,
                BodyRequests::default(),
            ))
            .id();

        app.world_mut().run_system_once(describe_player_bodies).unwrap();

        let requests = app.world().get::<BodyRequests>(player).unwrap();
        assert!(requests.running_forward, "input never reached the body");
        assert!(requests.wall_ahead, "the wall the step hit never reached the body");
        assert_eq!(AnimationState::from_requests(requests), AnimationState::PushingWall);

        // Described again with nothing new to say, it says nothing:
        // `BodyRequests` is replicated, and a write is a resend.
        let described = app.world().entity(player).get_ref::<BodyRequests>().unwrap().last_changed();
        app.world_mut().run_system_once(describe_player_bodies).unwrap();
        let again = app.world().entity(player).get_ref::<BodyRequests>().unwrap().last_changed();
        assert_eq!(again, described, "an unchanged description was written anyway");
    }

    /// A taunt claiming the legs at full weight stands them; one fading in
    /// stands them partway; the weapon layer's claim stands nothing.
    #[test]
    fn a_claim_above_the_weapon_stands_what_it_claims() {
        use crate::common::skeleton::play::Playing;
        use crate::common::skeleton::rig::bone;

        let rig = crate::common::skeleton::default_humanoid().clone();
        let striding = {
            let mut pose = Pose::rest();
            pose.set(bone::THIGH_L, Quat::from_rotation_x(0.8));
            pose.set(bone::UPPER_ARM_R, Quat::from_rotation_x(0.5));
            pose
        };
        let clip = Handle::<crate::common::skeleton::clip::Clip>::default();
        let claiming = |layer, weight| {
            ClipStack(vec![Playing {
                weight,
                claims: Regions::LEGS,
                ..Playing::new(layer, clip.clone(), 0.0)
            }])
        };
        let thigh_after = |stack: ClipStack| {
            let mut pose = striding.clone();
            stand_where_claimed(&rig, &mut pose, &stack, Pose::rest);
            (pose.joint(bone::THIGH_L).angle_between(Quat::IDENTITY), pose.joint(bone::UPPER_ARM_R))
        };

        let (thigh, arm) = thigh_after(claiming(Layer::Taunt, 1.0));
        assert!(thigh < 1e-5, "the claimed leg still strides: {thigh}");
        assert_eq!(arm, striding.joint(bone::UPPER_ARM_R), "an unclaimed arm was stood");

        let (thigh, _) = thigh_after(claiming(Layer::Taunt, 0.5));
        assert!((thigh - 0.4).abs() < 1e-3, "half a fade should stand it halfway, not {thigh}");

        let (thigh, _) = thigh_after(claiming(Layer::Weapon, 1.0));
        assert!((thigh - 0.8).abs() < 1e-5, "the weapon layer stood a leg");
    }

    /// Pressing forward while a taunt holds you still is not running:
    /// described as running, the legs freeze mid-stride, since no ground is
    /// covered for the stride to advance on.
    #[test]
    fn a_body_a_taunt_holds_still_is_not_described_as_running() {
        use crate::common::taunt::{TauntId, Taunts};

        let mut world = World::new();
        world.insert_resource(Taunts::shipped());
        let pressing_forward =
            ActionState(PlayerInput { movement: Vec2::Y, ..default() });
        let free = world
            .spawn((Player::default(), Stance::Standing, pressing_forward.clone(), BodyRequests::default()))
            .id();
        let waving = world
            .spawn((
                Player::default(),
                Stance::Standing,
                pressing_forward,
                Condition { taunt: Some(TauntId::of("wave")), ..default() },
                BodyRequests::default(),
            ))
            .id();

        world.run_system_once(describe_player_bodies).unwrap();
        assert!(world.get::<BodyRequests>(free).unwrap().running_forward);
        assert!(!world.get::<BodyRequests>(waving).unwrap().moving(), "the waving body was described as running");
    }
}
