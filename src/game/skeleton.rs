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
//! Everything is on one entity on purpose. A writer that had to walk to a
//! child to say "this body is airborne" is a writer three different subsystems
//! would each get subtly wrong.
//!
//! Nothing here is gated on [`AppMode`] except the two systems that are about
//! *players*: the editor draws and animates skeletons too, and a rig that
//! froze on F5 would be a preview that only worked in one mode. Drawing lives
//! in this plugin rather than `GamePlugin` because `Gizmos` needs renderer
//! resources the game's headless tests do not have.

use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::skeleton::{
    draw_skeleton, animator_pose, humanoid, leg_length, AnimationClock, AnimationPhase, BodyRequests,
    direction_of, DisplaySpeed, ForcedAnimation, Gait, Pose, PoseInputs, Proportions, Skeleton,
    SkeletonAnimator,
    SkeletonPalette,
};
use crate::game::body_mesh::BodyMeshPlugin;
use crate::game::hitbox::HitboxPlugin;
use crate::game::hitscan::HitscanPlugin;
use crate::game::player::{
    step_player, PhysicsBody, Player, PlayerInput, ViewMode, PLAYER_HALF,
};

/// Where a skeleton's feet sit relative to the entity carrying it.
///
/// A rig is placed by its feet — the end a spawn point marks and a mapper
/// lines up against a floor — but the body the game moves has its origin at
/// the centre of its collision box. Rather than a child entity to hold the
/// difference, the difference is a component: one entity, one place to look
/// for everything about a body.
#[derive(Component, Clone, Copy, Debug)]
pub struct SkeletonRoot(pub Vec3);

/// Whether the bones are drawn on top of the mesh. `F6` toggles it.
///
/// Off by default: it is a debug view of what is inside a body, and a limb
/// that bends the wrong way is a bone problem the mesh is built to hide.
#[derive(Resource, Debug, Default)]
pub struct ShowBones(pub bool);

pub struct SkeletonPlugin;

impl Plugin for SkeletonPlugin {
    fn build(&self, app: &mut App) {
        app
            // The boxes a body can be hit on are part of what a body is, and
            // they need the same renderer resources this plugin already does.
            // The debug laser next to the boxes it tests against: it has no
            // meaning without them, and it is how you check they are where
            // they look like they are.
            .add_plugins((HitboxPlugin, HitscanPlugin))
            // Beside the hitboxes rather than in `GamePlugin` for the same
            // reason: the editor shows bodies too, and one with geometry only
            // in Play would be a preview of something else.
            .add_plugins(BodyMeshPlugin)
            .add_plugins(GameTimePlugin)
            .init_resource::<ShowBones>()
            .add_systems(Update, toggle_bones)
            .add_systems(Update, (
                describe_player_bodies.run_if(in_state(AppMode::Play)),
                dress_new_players.run_if(in_state(AppMode::Play)),
                equip_new_bodies,
                // After the writers, so a body animates on the situation it is
                // in this frame rather than last frame's.
                advance_animators,
            ).chain())
            // After propagation, so a rig is drawn where its body is now.
            // `interpolate_bodies` has already run by here, so a player's
            // skeleton follows the smoothed position, not the 64 Hz one.
            .add_systems(PostUpdate, draw_skeletons.after(TransformSystems::Propagate))
        ;
    }
}

/// Game time: the clocks a match runs on, and the switch that stops them.
///
/// **None of it runs in the editor.** Everything here is a cycle — the shared
/// clock an animation is sampled against, the stride a gait is at, the stance
/// a step decided — and a room is far easier to line up against a body that is
/// holding still than one that is breathing.
///
/// What this does *not* freeze is `advance_animators`, which still blends a
/// display into whatever state the panel has just been set to. That is not
/// game time; that is the editor answering an edit, and a display that stopped
/// responding to the dropdown above it would be a worse editor rather than a
/// stiller one. The clock being frozen is what makes the answer a held frame
/// instead of a cycle.
///
/// Its own plugin so that "does game time run right now" is one gate in one
/// place rather than a `run_if` on each system, and so a test can ask the
/// question without standing up a renderer.
pub struct GameTimePlugin;

impl Plugin for GameTimePlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<AnimationClock>()
            // Before anything reads it, and on the tick: the clock is the one
            // quantity every viewer of a body has to agree on.
            .add_systems(FixedUpdate, (
                advance_animation_clock,
                // After the step, because it advances on the ground that step
                // actually covered.
                advance_gaits.after(step_player),
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
/// `Time<Fixed>`'s delta inside `FixedUpdate` is the fixed timestep itself, so
/// this advances by exactly one tick however the frame rate is behaving —
/// which is the whole reason the clock is here rather than in `Update`.
pub fn advance_animation_clock(time: Res<Time<Fixed>>, mut clock: ResMut<AnimationClock>) {
    clock.advance(time.delta_secs());
}

/// Anything with a rig gets the [`Gait`] every body has, in one place rather
/// than a line in each of the things that spawn a body: a body whose spawner
/// forgot it would stand still while running.
///
/// The gait and nothing else. Every body has a stride, but not every body is
/// one somebody can shoot — a rig standing on a spawn point is a drawing of
/// what fits there — so [`Hitboxes`](crate::common::hitbox::Hitboxes) come
/// from `#[require(Hitboxes)]` on the markers that mean a real body:
/// [`Player`], `CarouselBody`, `AnimationDisplayMarker`.
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
/// A body's transform is the centre of its box, and ducking makes that box
/// half as tall — so the offset from the centre down to the feet is not a
/// constant. In `FixedUpdate` beside the step that changes the stance, so a
/// crouch and the body drawn crouching happen on the same tick rather than a
/// frame apart.
pub fn follow_stance(mut bodies: Query<(&Stance, &mut SkeletonRoot), Changed<Stance>>) {
    for (stance, mut root) in &mut bodies {
        root.0 = Vec3::NEG_Y * stance.half_extents().y;
    }
}

/// Advance every body's stride by the ground it covered this tick.
///
/// In `FixedUpdate` and off the step's own displacement, so the cycle is a
/// function of the simulation rather than of how fast anyone is drawing. It is
/// the same reasoning as the hitboxes: a server owns this number and
/// replicates it, the way it replicates a position.
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
        // Back to the start when a body stops, so setting off again does not
        // begin mid-swing on a foot that is already in the air.
        if !animator.state().uses_gait() {
            gait.reset();
            continue;
        }

        let leg = leg_length(skeleton);
        let moved = match physics {
            // Only the ground covered counts, and in the body's own frame —
            // stepping forwards and stepping sideways are different steps, and
            // a diagonal is both. Falling is not walking, and a body shoved
            // sideways by a lift has not taken a step.
            Some(body) => {
                let world = (body.current - body.previous).xz();
                let facing = global.rotation();
                let right = (facing * Vec3::X).xz();
                let forward = (facing * Vec3::NEG_Z).xz();
                Vec2::new(world.dot(right), world.dot(forward))
            }
            // Nothing moving to measure: a body being shown rather than
            // played, covering ground it is not actually covering, in the
            // direction its state says it would be going.
            None => {
                direction_of(animator.state()) * display.copied().unwrap_or_default().0 * leg * dt
            }
        };
        gait.advance(moved, leg, dt);
    }
}

/// Where a body's feet are, from the entity carrying the rig.
///
/// Three systems ask — the one that animates, the one that draws and the one
/// that works out where a body can be hit — and a rig drawn at one place and
/// boxed at another would be worse than either.
pub fn skeleton_root(global: &GlobalTransform, offset: Option<&SkeletonRoot>) -> Transform {
    let mut root = global.compute_transform();
    if let Some(SkeletonRoot(offset)) = offset {
        root.translation += root.rotation * *offset;
    }
    root
}

/// Describe what the local player's body is doing.
///
/// The one writer of [`BodyRequests`] that exists today. A remote player's
/// requests will come off the wire and an NPC's from its planner; all three
/// write the same component and none of them names an animation, which is the
/// entire point — the state machine is written once, in
/// [`crate::common::skeleton::state`].
///
/// Fields are assigned rather than accumulated: this system owns all of them,
/// every frame, so nothing goes stale.
fn describe_player_bodies(
    input: Res<PlayerInput>,
    mut players: Query<(&Player, &Stance, &mut BodyRequests)>,
) {
    for (player, stance, mut requests) in &mut players {
        // `PlayerInput` is the local player's, so this is only correct while
        // there is one body. A second local player, or a remote one, gets its
        // own writer rather than a second reading of this resource.
        requests.running_forward = input.movement.y > 0.5;
        requests.running_backward = input.movement.y < -0.5;
        requests.strafing_left = input.movement.x < -0.5;
        requests.strafing_right = input.movement.x > 0.5;
        requests.airborne = !player.on_ground;
        // What the body *is*, not what the key says: a body that cannot stand
        // up under a vent is still crouched, and should still look it.
        requests.crouching = *stance == Stance::Crouched;
        // Asked to move and stopped on a ground axis. The step is the only
        // thing that knows a wall was hit, so it records it and this reads it.
        requests.wall_ahead = requests.moving() && (player.blocked.x || player.blocked.z);
    }
}

/// Give every newly spawned body a skeleton.
///
/// Keyed off `Added<Player>` rather than done in `enter_play`, because the
/// body is spawned by a command there and does not exist until that schedule's
/// commands are applied. This also covers any other way a body comes to be.
fn dress_new_players(mut commands: Commands, players: Query<Entity, Added<Player>>) {
    for player in &players {
        commands.entity(player).insert((
            humanoid(Proportions::DEFAULT),
            Pose::rest(),
            SkeletonAnimator::default(),
            BodyRequests::default(),
            Stance::default(),
            // Phase zero until bodies have identities everyone agrees on. A
            // networked body takes its phase from its network id, which is
            // what makes two clients put it in the same part of its cycle;
            // an `Entity`'s index would not, being local to one `World`.
            AnimationPhase::default(),
            SkeletonRoot(Vec3::NEG_Y * PLAYER_HALF.y),
        ));
    }
}

/// Run every body's state machine and write the pose it produces.
///
/// The only place a [`Pose`] is written on an animated body. A system that
/// wanted to bend one bone — a look-at, an IK pass — belongs after this and
/// edits the pose it left, rather than competing with it.
fn advance_animators(
    time: Res<Time>,
    clock: Res<AnimationClock>,
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
    )>,
) {
    let dt = time.delta_secs();
    for (skeleton, mut animator, mut pose, global, offset, requests, forced, phase, gait) in
        &mut bodies
    {
        match forced {
            // Being shown rather than driven: requests, if any, are ignored.
            Some(ForcedAnimation(state)) => animator.force(*state, dt),
            // A body with nothing describing it is a body standing still,
            // which is the right answer for one nothing is driving.
            None => animator.advance(&requests.copied().unwrap_or_default(), dt),
        }
        // Sampled from the shared clock rather than from time-in-state, so
        // two people watching this body on the same tick see it in the same
        // part of its cycle whatever either of them was doing a moment ago.
        //
        // The root here is last frame's, since propagation has not run yet.
        // Harmless: the corrections are stated in world space but resolve to
        // joint angles, so only the body's facing matters and not where it is
        // standing.
        let gait = gait.copied().unwrap_or_default();
        let inputs = PoseInputs {
            seconds: clock.seconds() + phase.copied().unwrap_or_default().0,
            stride: gait.phase(),
            speed: gait.speed(),
            travel: gait.travel(),
        };
        *pose = animator_pose(skeleton, &animator, &inputs, &skeleton_root(global, offset));
    }
}

/// Every body on the map, in whatever pose it is holding, once `F6` asks for
/// them.
///
/// Skips the one the camera is inside: from in there its own rig is a set of
/// bones across the lens. `F` swaps to third person and it appears.
pub fn draw_skeletons(
    show: Res<ShowBones>,
    mut gizmos: Gizmos,
    view: Res<ViewMode>,
    bodies: Query<(&Skeleton, &Pose, &GlobalTransform, Option<&SkeletonRoot>, Option<&Player>)>,
) {
    if !show.0 {
        return;
    }

    for (skeleton, pose, global, offset, own_body) in &bodies {
        // `Player` is the body this machine is looking out of. A remote body
        // will not carry it, so this hides one rig rather than everybody's.
        if own_body.is_some() && !view.shows_own_body() {
            continue;
        }
        let mut root = global.compute_transform();
        if let Some(SkeletonRoot(offset)) = offset {
            root.translation += root.rotation * *offset;
        }
        draw_skeleton(&mut gizmos, skeleton, pose, &root, &SkeletonPalette::SIDES);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::hitbox::Hitboxes;
    use crate::common::skeleton::AnimationState;
    use bevy::ecs::system::RunSystemOnce;

    /// One frame of animation. `Time` does not advance on its own in a bare
    /// app, and an animator handed a `dt` of zero never leaves its state —
    /// which would make every test below pass for the wrong reason.
    fn tick(app: &mut App) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.world_mut().run_system_once(advance_animators).unwrap();
    }

    /// A real body carries a marker that requires hitboxes; a spawn point's
    /// preview carries none, so it comes out unboxed on the same rig.
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
        // A body in every other respect: it is only hit volumes it goes
        // without.
        assert!(world.get::<Gait>(preview).is_some(), "the preview is not a body at all");
    }

    /// The thing the whole gate is for: a body in the editor is holding a
    /// frame, not playing a cycle. Only the clock is checked, because the
    /// clock is what every animation is sampled against — freeze it and the
    /// pose is frozen with it.
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

    /// The seam the whole arrangement rests on: a body's animation follows
    /// from a component anything can write. Written here by hand, the way a
    /// network packet or an NPC would.
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

    /// A forced body ignores its requests, which is what makes the editor's
    /// display show what a mapper asked for rather than what it is doing.
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

    /// Walking into a wall is the two-writer case: input says forward, the
    /// step says blocked, and only the state machine puts them together.
    #[test]
    fn a_player_pressed_against_a_wall_is_described_as_pushing() {
        let mut app = App::new();
        app.insert_resource(PlayerInput {
            movement: Vec2::new(0.0, 1.0),
            ..default()
        });
        let player = app
            .world_mut()
            .spawn((
                Player { on_ground: true, blocked: BVec3::new(false, false, true), ..default() },
                Stance::Standing,
                BodyRequests::default(),
            ))
            .id();

        app.world_mut().run_system_once(describe_player_bodies).unwrap();

        let requests = app.world().get::<BodyRequests>(player).unwrap();
        assert!(requests.running_forward, "input never reached the body");
        assert!(requests.wall_ahead, "the wall the step hit never reached the body");
        assert_eq!(AnimationState::from_requests(requests), AnimationState::PushingWall);
    }
}
