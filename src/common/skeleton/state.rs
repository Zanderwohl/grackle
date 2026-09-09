//! What a body is doing, and who gets to say so.
//!
//! The rule this module exists to enforce: **nothing outside picks an
//! animation.** A local player, an NPC, a body being driven by the network and
//! a preview standing in the editor all have completely different ideas of
//! where their information comes from, and if each of them chose a clip you
//! would have four copies of the same state machine drifting apart — the walk
//! that only breaks for remote players is exactly that bug.
//!
//! So there are two components and one direction of flow:
//!
//! - [`BodyRequests`] is a description of the situation: running forward,
//!   airborne, a wall in front. Anything may write it, and several systems
//!   can each own a field of it — input writes what is being asked for, the
//!   physics step writes what the world did about it. It contains no
//!   animation names at all.
//! - [`SkeletonAnimator`] is the state machine. It reads requests, decides
//!   which [`AnimationState`] follows from them, and is the only thing that
//!   writes a [`Pose`]. Transitions, blending and dwell times are its problem
//!   and nobody else's.
//!
//! [`ForcedAnimation`] is the deliberate exception: it pins a body to one
//! state regardless of requests, which is what the editor's Animation Display
//! feature needs — there is no situation to describe, a mapper is asking to
//! see a particular state.
//!
//! There are no animation clips yet, so every state currently poses as the
//! A-pose. The machine is still worth having now: it is the seam everything
//! else is written against, and the transitions are testable before there is
//! anything to look at.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::common::skeleton::gait::{
    direction_of, foot_offsets as gait_foot_offsets, gait_pose, FootOffset, GaitShape,
};
use crate::common::skeleton::rig::{bone, Pose};
use crate::get;

/// Everything a state needs to know to pose a body.
///
/// Two clocks, deliberately, because animations are not all functions of the
/// same thing. An idle is a function of time; a run is a function of how far
/// you have gone, which is what stops its feet sliding when you speed up. Both
/// are quantities every viewer of a body can agree on — see [`AnimationClock`]
/// and [`crate::common::skeleton::gait::Gait`] — and a state says which it
/// uses simply by reading it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PoseInputs {
    /// The shared clock, plus this body's own [`AnimationPhase`].
    pub seconds: f32,
    /// Where in its stride the body is, wrapping at 1.
    pub stride: f32,
    /// How fast it is going, in leg-lengths per second.
    ///
    /// Measured from ground covered rather than asked for, so a body dragged
    /// along by something else animates as moving and a body pushing at a wall
    /// does not. It is what decides whether a gait is a walk or a run.
    pub speed: f32,
}

/// The clock every animation is sampled against.
///
/// Not each viewer's own elapsed time, and not the animator's time-in-state:
/// two people watching the same body on the same tick have to see it in the
/// same part of its cycle, or a spectator and a player disagree about where a
/// head is. So the clock is a shared quantity — advanced by whole fixed steps,
/// which is what makes it a number a server can state and a client can be
/// corrected to.
///
/// Sampling on tick boundaries rather than at frame rate is deliberate for the
/// same reason. An idle cycle is seconds long; 64 Hz is far finer than
/// anything the eye can catch in one, and it is the rate everything else that
/// has to agree already runs at.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct AnimationClock {
    ticks: u64,
    seconds: f32,
}

impl AnimationClock {
    /// Seconds since the clock started.
    pub fn seconds(&self) -> f32 {
        self.seconds
    }

    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// One fixed step. `dt` is the fixed timestep, so this is exact rather
    /// than accumulated frame time.
    pub fn advance(&mut self, dt: f32) {
        self.ticks += 1;
        // From the tick count rather than by adding `dt` each time: a float
        // added sixty-four times a second drifts, and two machines that had
        // been running for different lengths of time would drift apart.
        self.seconds = self.ticks as f32 * dt;
    }
}

/// Where in its cycle a body is, relative to the shared clock.
///
/// Bodies that all bounced on the same frame would read as one machine rather
/// than ten people, so each is offset. The offset is derived from an
/// identifier everyone agrees on rather than rolled locally: two viewers of
/// the same body must land on the same phase, which is exactly what an RNG
/// would not give them.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct AnimationPhase(pub f32);

impl AnimationPhase {
    /// A phase from a stable identifier.
    ///
    /// The identifier has to be one every machine computes the same way — a
    /// cell's index in a grid, and a body's network id once there is a
    /// network. Not an `Entity`, whose index is local to one `World` and would
    /// put a body in a different part of its cycle on every machine.
    pub fn from_id(id: u64) -> AnimationPhase {
        // SplitMix64's finaliser: integer-only, so every machine agrees, and
        // it scatters consecutive ids rather than leaving neighbours in step.
        let mut z = id.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        // Into [0, 1) with a division that is exact in binary, then out to a
        // spread of seconds wider than any cycle.
        AnimationPhase(((z >> 40) as f32 / (1u32 << 24) as f32) * PHASE_SPREAD)
    }
}

/// How far apart in time two bodies can be put.
///
/// Longer than the longest cycle, so a phase can land anywhere in one.
const PHASE_SPREAD: f32 = 10.0;

/// How long a state has to hold before another one can take over.
///
/// Without it, a body pressed against a wall alternates between running and
/// pushing at whatever rate collision happens to resolve, and the animation
/// flickers. A tenth of a second is below the threshold where a real
/// transition feels delayed and above the rate anything chatters at.
const MIN_DWELL: f32 = 0.1;

/// What is happening to a body, in terms anything can observe.
///
/// Deliberately not "which animation": these are facts about the world and
/// the body's intent, so an NPC's planner and a network packet can fill in the
/// same struct that keyboard input does.
///
/// Each field is *assigned* by whoever owns it, every frame, rather than
/// accumulated — a field nobody writes stays false, and a writer that stops
/// having an opinion clears its own field. That is why there is no reset
/// system: a stale `wall_ahead` that nothing ever cleared would be a body
/// pushing at thin air forever.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct BodyRequests {
    /// Being asked to move forwards — a held W, an NPC's path, a replayed
    /// input. Asked for, not achieved: a body walking into a wall is still
    /// running forward, and the difference is [`BodyRequests::wall_ahead`].
    pub running_forward: bool,
    /// Being asked to move backwards.
    pub running_backward: bool,
    /// Being asked to move sideways, either way.
    pub strafing: bool,
    /// Movement is being stopped by something solid in the direction asked
    /// for. Written by whatever ran the movement, because it is the only thing
    /// that knows.
    pub wall_ahead: bool,
    /// Not standing on anything.
    pub airborne: bool,
}

impl BodyRequests {
    /// Being asked to move at all, in any direction.
    pub fn moving(&self) -> bool {
        self.running_forward || self.running_backward || self.strafing
    }
}

/// One animation state. There are no clips behind them yet.
///
/// The order of these is on disk — the editor's Animation Display feature
/// stores the state as [`AnimationState::index`] — so **add new states at the
/// end** and do not reorder. A shuffled list would silently repoint every
/// saved display at a different animation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, EnumIter)]
pub enum AnimationState {
    #[default]
    Idle,
    RunForward,
    RunBackward,
    Strafe,
    /// Asking to move forwards into something solid. Its own state rather than
    /// a variant of running, because a body whose legs keep cycling against a
    /// wall is the single most obvious animation bug there is.
    PushingWall,
    Airborne,
}

impl AnimationState {
    /// Stable index for the save format. See the type's note about ordering.
    pub fn index(&self) -> u32 {
        match self {
            AnimationState::Idle => 0,
            AnimationState::RunForward => 1,
            AnimationState::RunBackward => 2,
            AnimationState::Strafe => 3,
            AnimationState::PushingWall => 4,
            AnimationState::Airborne => 5,
        }
    }

    /// The state an index names, or [`AnimationState::Idle`] for one this
    /// build does not know.
    ///
    /// A map saved by a later build naming a state this one has never heard of
    /// is a display that stands there idle, not a refusal to open the map.
    pub fn from_index(index: u32) -> AnimationState {
        match index {
            1 => AnimationState::RunForward,
            2 => AnimationState::RunBackward,
            3 => AnimationState::Strafe,
            4 => AnimationState::PushingWall,
            5 => AnimationState::Airborne,
            _ => AnimationState::Idle,
        }
    }

    pub fn name(&self) -> String {
        match self {
            AnimationState::Idle => get!("animation.states.idle"),
            AnimationState::RunForward => get!("animation.states.run_forward"),
            AnimationState::RunBackward => get!("animation.states.run_backward"),
            AnimationState::Strafe => get!("animation.states.strafe"),
            AnimationState::PushingWall => get!("animation.states.pushing_wall"),
            AnimationState::Airborne => get!("animation.states.airborne"),
        }
    }

    /// The state that follows from a situation.
    ///
    /// Priority order, most overriding first: being in the air beats anything
    /// the legs were asked to do, and running into a wall beats running.
    pub fn from_requests(requests: &BodyRequests) -> AnimationState {
        if requests.airborne {
            AnimationState::Airborne
        } else if requests.running_forward && requests.wall_ahead {
            AnimationState::PushingWall
        } else if requests.running_forward {
            AnimationState::RunForward
        } else if requests.running_backward {
            AnimationState::RunBackward
        } else if requests.strafing {
            AnimationState::Strafe
        } else {
            AnimationState::Idle
        }
    }

    /// Whether this state's feet are pinned to the ground it is standing on.
    ///
    /// A correction the state asks for rather than something every body gets:
    /// see [`crate::common::skeleton::ik::finish_pose`]. Airborne is the case
    /// that makes the distinction real — feet welded to a floor a body has
    /// left would be a body doing the splits on the way up.
    pub fn plants_feet(&self) -> bool {
        !matches!(self, AnimationState::Airborne)
    }

    /// Transitions that must not wait for [`MIN_DWELL`].
    ///
    /// Leaving the ground and landing are the two the eye catches: a jump that
    /// starts a tenth of a second late reads as the body being dragged
    /// upwards, where a walk that starts late reads as nothing at all.
    fn is_urgent(&self) -> bool {
        matches!(self, AnimationState::Airborne)
    }

    /// The pose this state is in at `seconds` on the shared clock.
    ///
    /// `seconds` is [`AnimationClock`] plus the body's [`AnimationPhase`], not
    /// time-in-state: a cycle that restarted on every transition would hitch
    /// each time a body brushed a wall, and two viewers would only agree if
    /// they had also agreed on the exact tick the state changed.
    ///
    /// The moving states are a gait, the idle is a settle, and being in the
    /// air is still the A-pose — falling is the next thing worth writing, and
    /// standing in for it with a guess would be worse than the placeholder
    /// being obvious.
    ///
    /// Both are functions rather than sampled keys, because both genuinely are
    /// functions. The first state that needs a clip is one nobody can derive:
    /// a taunt.
    pub fn pose(&self, inputs: &PoseInputs) -> Pose {
        match self {
            AnimationState::Idle => idle_pose(inputs.seconds),
            state if state.uses_gait() => gait_pose(inputs, direction_of(*state)),
            _ => Pose::rest(),
        }
    }

    /// Whether this state walks, in the sense of taking steps.
    ///
    /// `PushingWall` does, which is the interesting one: its cycle is driven
    /// by ground covered, so a body shoving at a wall holds whatever step it
    /// stopped on instead of running on the spot.
    pub fn uses_gait(&self) -> bool {
        matches!(
            self,
            AnimationState::RunForward
                | AnimationState::RunBackward
                | AnimationState::Strafe
                | AnimationState::PushingWall
        )
    }

    /// Where this state wants the feet, relative to where they rest.
    ///
    /// Empty offsets mean "where a body standing still has them", which is
    /// what an idle wants. A clip will answer this from its contact spans.
    pub fn foot_offsets(&self, inputs: &PoseInputs) -> [FootOffset; 2] {
        if self.uses_gait() {
            gait_foot_offsets(
                inputs.stride,
                direction_of(*self),
                GaitShape::for_speed(inputs.speed),
            )
        } else {
            [FootOffset::default(); 2]
        }
    }
}

/// The state machine, and the only thing that writes a [`Pose`].
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct SkeletonAnimator {
    state: AnimationState,
    /// How long the current state has been running, which is what a clip is
    /// sampled at and what [`MIN_DWELL`] is measured against.
    elapsed: f32,
}

impl SkeletonAnimator {
    pub fn state(&self) -> AnimationState {
        self.state
    }

    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// Advance by `dt`, moving to the state `requests` calls for if this one
    /// has had its turn.
    pub fn advance(&mut self, requests: &BodyRequests, dt: f32) {
        self.step_towards(AnimationState::from_requests(requests), dt);
    }

    /// Advance by `dt` in a state chosen for us. See [`ForcedAnimation`].
    pub fn force(&mut self, state: AnimationState, dt: f32) {
        self.step_towards(state, dt);
    }

    fn step_towards(&mut self, wanted: AnimationState, dt: f32) {
        self.elapsed += dt;
        if wanted == self.state {
            return;
        }
        // Either the state has had its dwell, or the new one is too important
        // to wait — leaving the state matters as much as arriving, so a jump
        // out of a hundredth-of-a-second idle still plays.
        if self.elapsed >= MIN_DWELL || wanted.is_urgent() || self.state.is_urgent() {
            self.state = wanted;
            self.elapsed = 0.0;
        }
    }

    /// The pose this body holds, before any corrections.
    pub fn pose_at(&self, inputs: &PoseInputs) -> Pose {
        self.state.pose(inputs)
    }
}

/// Pin a body to one state and ignore its requests.
///
/// For showing an animation rather than driving a character: the editor's
/// Animation Display feature, and anything else that wants a body to stand
/// there doing one named thing.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ForcedAnimation(pub AnimationState);

/// How long one idle cycle takes, in seconds.
const IDLE_PERIOD: f32 = 3.4;

/// How far the hips drop at the bottom of the cycle, in hip-heights.
///
/// In hip-heights rather than metres so the same idle reads the same on every
/// build — a two-centimetre drop is a settle on a tall body and a squat on a
/// short one.
const IDLE_BOB: f32 = 0.022;

/// How far the chest leans forward at the bottom, in radians.
const IDLE_LEAN: f32 = 0.045;

/// How far the arms swing, in radians.
const IDLE_ARM_SWING: f32 = 0.05;

/// Standing still: a slow settle into the knees and back up.
///
/// Only the hips and the spine. The legs are not mentioned at all — the feet
/// are held by the planting pass in [`crate::common::skeleton::ik`], which is
/// the same pass a run cycle and a taunt's contact spans will use. This
/// function used to work the knee angles out itself, exactly and only for the
/// case of a hip directly above a planted foot; the solver does the general
/// thing, so the special case is gone.
fn idle_pose(seconds: f32) -> Pose {
    // Down from rest and back, never up: rest already has the legs straight,
    // so there is nowhere above it to go without leaving the floor.
    let cycle = std::f32::consts::TAU * seconds / IDLE_PERIOD;
    let settle = (1.0 - cycle.cos()) * 0.5;

    let mut pose = Pose::rest();
    pose.root_offset = Vec3::NEG_Y * (settle * IDLE_BOB);

    // A settle that only moved vertically would read as an elevator. The chest
    // leans into it and the arms trail a quarter cycle behind, which is what
    // makes it look like weight rather than translation.
    pose.set(bone::CHEST, Quat::from_rotation_x(settle * IDLE_LEAN));
    pose.set(bone::NECK, Quat::from_rotation_x(-settle * IDLE_LEAN * 0.6));

    let trail = (cycle - std::f32::consts::FRAC_PI_2).sin();
    pose.set(bone::UPPER_ARM_L, Quat::from_rotation_z(-trail * IDLE_ARM_SWING));
    pose.set(bone::UPPER_ARM_R, Quat::from_rotation_z(trail * IDLE_ARM_SWING));

    pose
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::skeleton::rig::{humanoid, Proportions};

    /// The idle's own pose, before corrections.
    fn idle_at(seconds: f32) -> Pose {
        AnimationState::Idle.pose(&PoseInputs { seconds, ..default() })
    }

    /// Where a named bone's head and tail end up, for a body standing at the
    /// origin.
    fn bone_at(proportions: Proportions, pose: &Pose, name: &str) -> (Vec3, Vec3) {
        let skeleton = humanoid(proportions);
        let posed = skeleton
            .posed_bones(pose, &Transform::IDENTITY)
            .into_iter()
            .find(|bone| bone.name == name)
            .expect("no such bone");
        (posed.head, posed.tail)
    }

    /// The bounce lowers the hips and the legs hang off them, so on its own
    /// the idle is a body sinking through the floor. What holds the feet is
    /// the planting pass the state asks for — this is the whole pipeline, and
    /// the thing a player would actually see.
    ///
    /// Across all three builds, because a foot that stayed put on one and
    /// slid on another is exactly what a pose of rotations would do.
    #[test]
    fn feet_stay_planted_through_the_whole_idle_cycle() {
        use crate::common::skeleton::ik::finish_pose;

        for proportions in [Proportions::DEFAULT, Proportions::STOCKY, Proportions::LANKY] {
            let skeleton = humanoid(proportions);
            let (_, resting_toe) = bone_at(proportions, &Pose::rest(), bone::FOOT_L);

            for step in 0..40 {
                let seconds = step as f32 * IDLE_PERIOD / 40.0;
                let pose = finish_pose(
                    &skeleton,
                    AnimationState::Idle,
                    &PoseInputs { seconds, ..default() },
                    &Transform::IDENTITY,
                );
                let (ankle, toe) = bone_at(proportions, &pose, bone::FOOT_L);

                assert!(
                    (toe - resting_toe).length() < 1e-4,
                    "at {seconds:.2}s the foot has moved {:.5} m",
                    (toe - resting_toe).length()
                );
                assert!(ankle.y > 0.0, "the ankle went through the floor at {seconds:.2}s");
            }
        }
    }

    /// And it does actually move — a planted-feet test passes perfectly on an
    /// animation that does nothing at all.
    #[test]
    fn the_idle_settles_and_comes_back_up() {
        let standing = bone_at(Proportions::DEFAULT, &Pose::rest(), bone::HEAD).1.y;
        let bottom = bone_at(Proportions::DEFAULT, &idle_at(IDLE_PERIOD * 0.5), bone::HEAD).1.y;
        let back_up = bone_at(Proportions::DEFAULT, &idle_at(IDLE_PERIOD), bone::HEAD).1.y;

        assert!(standing - bottom > 0.015, "the head only dropped {} m", standing - bottom);
        assert!((standing - back_up).abs() < 1e-4, "the cycle does not return to where it started");
    }

    /// A one-sided idle would be a body leaning slightly for ever, and would
    /// show up the moment it was played next to nine others.
    #[test]
    fn the_idle_is_symmetric() {
        let pose = idle_at(0.9);
        let (_, left) = bone_at(Proportions::DEFAULT, &pose, bone::HAND_L);
        let (_, right) = bone_at(Proportions::DEFAULT, &pose, bone::HAND_R);

        assert!(
            (left - Vec3::new(-right.x, right.y, right.z)).length() < 1e-5,
            "the hands are at {left} and {right}"
        );
    }

    /// The point of a shared clock: the same tick gives the same pose, on any
    /// machine, whatever either of them was doing beforehand.
    #[test]
    fn the_same_moment_gives_the_same_pose() {
        let one = idle_at(12.25);
        let other = idle_at(12.25);
        assert_eq!(one.root_offset, other.root_offset);
        assert_eq!(one.joint(bone::THIGH_L), other.joint(bone::THIGH_L));
    }

    /// The clock counts ticks and derives seconds from the count, so a machine
    /// that has been running for an hour is at the same time as one that has
    /// been running for a minute if they are on the same tick.
    #[test]
    fn the_clock_advances_by_whole_ticks() {
        let dt = 1.0 / 64.0;
        let mut clock = AnimationClock::default();
        for _ in 0..640 {
            clock.advance(dt);
        }

        assert_eq!(clock.ticks(), 640);
        assert!((clock.seconds() - 10.0).abs() < 1e-4, "ten seconds is {}", clock.seconds());
    }

    /// A phase has to be a function of the identifier and nothing else — two
    /// viewers deriving it separately must land on the same number — and
    /// consecutive ids must not come out next to each other, or a grid bounces
    /// as a wave.
    #[test]
    fn a_phase_is_derived_from_its_id_alone() {
        assert_eq!(AnimationPhase::from_id(7), AnimationPhase::from_id(7));

        let phases: Vec<f32> = (0..60).map(|id| AnimationPhase::from_id(id).0).collect();
        assert!(phases.iter().all(|phase| (0.0..PHASE_SPREAD).contains(phase)));

        let mut sorted = phases.clone();
        sorted.sort_by(f32::total_cmp);
        sorted.dedup();
        assert_eq!(sorted.len(), phases.len(), "two bodies were given the same phase");

        // Neighbours land far apart rather than a step apart.
        let neighbours = phases.windows(2).filter(|pair| (pair[1] - pair[0]).abs() < 0.1).count();
        assert!(neighbours < 6, "{neighbours} of 59 neighbouring ids are nearly in step");
    }

    /// The example the whole arrangement is for: two independent facts, one
    /// written by input and one by collision, and neither writer knows what
    /// animation comes out.
    #[test]
    fn running_at_a_wall_is_not_running() {
        let mut requests = BodyRequests::default();
        requests.running_forward = true;
        assert_eq!(AnimationState::from_requests(&requests), AnimationState::RunForward);

        requests.wall_ahead = true;
        assert_eq!(AnimationState::from_requests(&requests), AnimationState::PushingWall);

        // A wall in front of a body that is not going anywhere is just a wall.
        requests.running_forward = false;
        assert_eq!(AnimationState::from_requests(&requests), AnimationState::Idle);
    }

    /// Being in the air beats whatever the legs were asked for.
    #[test]
    fn airborne_overrides_everything_else() {
        let requests = BodyRequests {
            running_forward: true,
            wall_ahead: true,
            airborne: true,
            ..default()
        };
        assert_eq!(AnimationState::from_requests(&requests), AnimationState::Airborne);
    }

    /// A body scraping along a wall flickers `wall_ahead` at the rate
    /// collision resolves it. The machine has to sit on that rather than
    /// passing it through to the animation.
    #[test]
    fn a_flickering_request_does_not_flicker_the_state() {
        let mut animator = SkeletonAnimator::default();
        let mut requests = BodyRequests { running_forward: true, ..default() };
        animator.advance(&requests, 0.5);
        assert_eq!(animator.state(), AnimationState::RunForward);

        // One frame's worth of wall, gone again next frame.
        requests.wall_ahead = true;
        animator.advance(&requests, 1.0 / 64.0);
        requests.wall_ahead = false;
        animator.advance(&requests, 1.0 / 64.0);

        assert_eq!(animator.state(), AnimationState::RunForward, "the wall touch changed the animation");
    }

    /// Held long enough, though, it has to actually change.
    #[test]
    fn a_request_that_persists_wins() {
        let mut animator = SkeletonAnimator::default();
        let requests = BodyRequests { running_forward: true, wall_ahead: true, ..default() };
        for _ in 0..16 {
            animator.advance(&requests, 1.0 / 64.0);
        }
        assert_eq!(animator.state(), AnimationState::PushingWall);
    }

    /// Leaving the ground and landing are exempt from the dwell: a jump that
    /// starts late looks like the body is being dragged upwards.
    #[test]
    fn jumping_and_landing_are_immediate() {
        let mut animator = SkeletonAnimator::default();
        let running = BodyRequests { running_forward: true, ..default() };
        animator.advance(&running, 0.5);

        let jumping = BodyRequests { running_forward: true, airborne: true, ..default() };
        animator.advance(&jumping, 1.0 / 64.0);
        assert_eq!(animator.state(), AnimationState::Airborne, "the jump waited for the dwell");

        // And landing immediately after, without having dwelt in the air.
        animator.advance(&running, 1.0 / 64.0);
        assert_eq!(animator.state(), AnimationState::RunForward, "the landing waited for the dwell");
    }

    /// The elapsed time is what a clip will be sampled at, so it has to
    /// restart with the state and keep running while the state holds.
    #[test]
    fn elapsed_time_restarts_with_the_state() {
        let mut animator = SkeletonAnimator::default();
        let idle = BodyRequests::default();
        animator.advance(&idle, 0.5);
        assert!((animator.elapsed() - 0.5).abs() < 1e-6);

        animator.advance(&BodyRequests { running_forward: true, ..default() }, 0.25);
        assert_eq!(animator.state(), AnimationState::RunForward);
        assert_eq!(animator.elapsed(), 0.0, "the new state started part-way through");
    }

    /// A forced state ignores requests entirely — that is the whole point of
    /// it — but still ages, so a clip plays.
    #[test]
    fn a_forced_state_ignores_requests() {
        let mut animator = SkeletonAnimator::default();
        animator.force(AnimationState::Airborne, 0.5);
        animator.force(AnimationState::Airborne, 0.5);
        assert_eq!(animator.state(), AnimationState::Airborne);
        assert!((animator.elapsed() - 0.5).abs() < 1e-6);
    }

    /// The index is on disk. Every state has to survive the trip, and an index
    /// from a newer build has to land somewhere rather than failing the load.
    #[test]
    fn every_state_round_trips_through_its_index() {
        use strum::IntoEnumIterator;

        for state in AnimationState::iter() {
            assert_eq!(AnimationState::from_index(state.index()), state);
        }
        assert_eq!(AnimationState::from_index(9999), AnimationState::Idle);
    }
}
