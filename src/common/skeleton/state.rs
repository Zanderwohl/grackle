//! What a body is doing, and who gets to say so.
//!
//! Nothing outside this module picks an animation: a local player, an NPC, a
//! networked body and an editor preview each choosing a clip would be four
//! copies of one state machine drifting apart.
//!
//! - [`BodyRequests`] describes the situation (running forward, airborne, a
//!   wall ahead) and names no animation. Several systems each own a field.
//! - [`SkeletonAnimator`] reads requests, picks an [`AnimationState`], and is
//!   the only writer of a [`Pose`]. Transitions, blending and dwell live there.
//!
//! [`ForcedAnimation`] pins a body to one state regardless of requests, for
//! the editor's Animation Display feature.

use std::collections::HashMap;
use std::sync::Arc;

use super::clip::StateAnimation;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::common::skeleton::gait::{
    arms_at_sides, crouch_posture, direction_of, foot_offsets as gait_foot_offsets, gait_pose,
    FootOffset, GaitStyle,
};
use crate::common::skeleton::rig::{bone, Pose};
use crate::get;

/// Everything a state needs to know to pose a body.
///
/// Two clocks: an idle is a function of time, a run of distance covered, which
/// keeps its feet from sliding as speed changes. Every viewer agrees on both.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PoseInputs {
    /// The shared clock, plus this body's own [`AnimationPhase`].
    pub seconds: f32,
    /// Where in its stride the body is, wrapping at 1.
    pub stride: f32,
    /// How fast it is going, in leg-lengths per second. Measured from ground
    /// covered rather than requested, so a body pushing at a wall does not
    /// animate as moving.
    pub speed: f32,
    /// Which way it is going, in its own frame: `x` right, `y` forwards.
    pub travel: Vec2,
}

/// The clock every animation is sampled against.
///
/// Shared rather than per-viewer or time-in-state, so two viewers see a body
/// at the same point in its cycle. Advanced by whole fixed steps so a server
/// can state it and a client be corrected to it.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct AnimationClock {
    ticks: u64,
    seconds: f32,
}

impl AnimationClock {
    pub fn seconds(&self) -> f32 {
        self.seconds
    }

    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// One fixed step; `dt` is the fixed timestep.
    pub fn advance(&mut self, dt: f32) {
        self.ticks += 1;
        // From the tick count rather than summed, so machines that have run for
        // different lengths of time do not drift apart.
        self.seconds = self.ticks as f32 * dt;
    }
}

/// Where in its cycle a body is, relative to the shared clock.
///
/// Keeps bodies from bouncing in unison. Derived from an id rather than rolled
/// locally, so every viewer lands on the same phase.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct AnimationPhase(pub f32);

impl AnimationPhase {
    /// From an id every machine computes the same way (a grid cell index, a
    /// network id), never an `Entity`, whose index is local to one `World`.
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

/// Seconds. Longer than the longest cycle, so a phase can land anywhere in one.
const PHASE_SPREAD: f32 = 10.0;

/// How long a state change blends over, in seconds. Short enough to read as a
/// change rather than a dissolve.
const BLEND_TIME: f32 = 0.15;

/// Seconds a state holds before another can take over. Stops a body against a
/// wall flickering between running and pushing at the collision rate.
const MIN_DWELL: f32 = 0.1;

/// What is happening to a body, as observable facts rather than animation
/// names, so input, an NPC planner and the network can fill the same struct.
///
/// Each field is assigned by its owner every frame, not accumulated, which is
/// why there is no reset system.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub struct BodyRequests {
    /// Asked for, not achieved: a body walking into a wall is still running
    /// forward, and the difference is [`BodyRequests::wall_ahead`].
    pub running_forward: bool,
    pub running_backward: bool,
    /// Towards the character's own left or right.
    pub strafing_left: bool,
    pub strafing_right: bool,
    /// Movement is stopped by something solid. Written by whatever ran the
    /// movement.
    pub wall_ahead: bool,
    /// Not standing on anything.
    pub airborne: bool,
    /// What the body is, not what was asked: a body under a vent stays crouched
    /// after the key comes up.
    pub crouching: bool,
}

impl BodyRequests {
    pub fn moving(&self) -> bool {
        self.running_forward || self.running_backward || self.strafing_left || self.strafing_right
    }
}

/// One animation state.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter, Reflect,
)]
pub enum AnimationState {
    #[default]
    Idle,
    RunForward,
    RunBackward,
    StrafeLeft,
    /// Its own state so the legs do not keep cycling against a wall.
    PushingWall,
    Airborne,
    /// Ducked and still.
    Crouch,
    /// Ducked and moving: a short-stepped shuffle that always keeps a foot
    /// down.
    CrouchWalk,
    /// Its own state because a pose cannot tell which way the feet travel.
    CrouchWalkBackward,
    StrafeRight,
    CrouchStrafeLeft,
    CrouchStrafeRight,
    /// The crouch posture with airborne legs, since the body is flying
    /// through the short hull.
    CrouchAirborne,
}

impl AnimationState {
    pub fn name(&self) -> String {
        match self {
            AnimationState::Idle => get!("animation.states.idle"),
            AnimationState::RunForward => get!("animation.states.run_forward"),
            AnimationState::RunBackward => get!("animation.states.run_backward"),
            AnimationState::StrafeLeft => get!("animation.states.strafe_left"),
            AnimationState::StrafeRight => get!("animation.states.strafe_right"),
            AnimationState::CrouchStrafeLeft => get!("animation.states.crouch_strafe_left"),
            AnimationState::CrouchStrafeRight => get!("animation.states.crouch_strafe_right"),
            AnimationState::CrouchAirborne => get!("animation.states.crouch_airborne"),
            AnimationState::PushingWall => get!("animation.states.pushing_wall"),
            AnimationState::Airborne => get!("animation.states.airborne"),
            AnimationState::Crouch => get!("animation.states.crouch"),
            AnimationState::CrouchWalk => get!("animation.states.crouch_walk"),
            AnimationState::CrouchWalkBackward => get!("animation.states.crouch_walk_backward"),
        }
    }

    /// Priority order: airborne beats everything, crouching beats what the legs
    /// were asked to do, and a wall beats running.
    pub fn from_requests(requests: &BodyRequests) -> AnimationState {
        if requests.airborne {
            if requests.crouching {
                AnimationState::CrouchAirborne
            } else {
                AnimationState::Airborne
            }
        } else if requests.crouching {
            if requests.wall_ahead || !requests.moving() {
                AnimationState::Crouch
            } else if requests.running_forward {
                AnimationState::CrouchWalk
            } else if requests.running_backward {
                AnimationState::CrouchWalkBackward
            } else if requests.strafing_left {
                AnimationState::CrouchStrafeLeft
            } else {
                AnimationState::CrouchStrafeRight
            }
        } else if requests.running_forward && requests.wall_ahead {
            AnimationState::PushingWall
        } else if requests.running_forward {
            AnimationState::RunForward
        } else if requests.running_backward {
            AnimationState::RunBackward
        } else if requests.strafing_left {
            AnimationState::StrafeLeft
        } else if requests.strafing_right {
            AnimationState::StrafeRight
        } else {
            AnimationState::Idle
        }
    }

    /// Whether this state has to fit inside the crouched hull. How far a body
    /// folds is worked out per build by
    /// [`crate::common::skeleton::ik::duck_under`].
    pub fn ducks(&self) -> bool {
        matches!(
            self,
            AnimationState::Crouch
                | AnimationState::CrouchWalk
                | AnimationState::CrouchWalkBackward
                | AnimationState::CrouchStrafeLeft
                | AnimationState::CrouchStrafeRight
                | AnimationState::CrouchAirborne
        )
    }

    /// Whether this state's feet are pinned to the ground; see
    /// [`crate::common::skeleton::ik::finish_pose`]. False in the air, where
    /// pinned feet would do the splits.
    pub fn plants_feet(&self) -> bool {
        !matches!(
            self,
            AnimationState::Airborne | AnimationState::CrouchAirborne
        )
    }

    /// Transitions that must not wait for [`MIN_DWELL`]: a jump that starts late
    /// reads as the body being dragged upwards.
    fn is_urgent(&self) -> bool {
        matches!(self, AnimationState::Airborne)
    }

    /// The file name a clip for this state is stored under. Written out rather
    /// than derived so a rename cannot orphan shipped clips. Names come from the
    /// enum so the library loads by name without listing a directory.
    pub fn clip_name(self) -> &'static str {
        match self {
            AnimationState::Idle => "idle",
            AnimationState::RunForward => "run_forward",
            AnimationState::RunBackward => "run_backward",
            AnimationState::StrafeLeft => "strafe_left",
            AnimationState::StrafeRight => "strafe_right",
            AnimationState::PushingWall => "pushing_wall",
            AnimationState::Airborne => "airborne",
            AnimationState::Crouch => "crouch",
            AnimationState::CrouchWalk => "crouch_walk",
            AnimationState::CrouchWalkBackward => "crouch_walk_backward",
            AnimationState::CrouchStrafeLeft => "crouch_strafe_left",
            AnimationState::CrouchStrafeRight => "crouch_strafe_right",
            AnimationState::CrouchAirborne => "crouch_airborne",
        }
    }

    /// The pose this state is in, before corrections.
    ///
    /// `inputs.seconds` is the shared clock plus phase, not time-in-state: a
    /// cycle restarted on every transition would hitch whenever a body brushed
    /// a wall, and viewers would have to agree on the tick of every change.
    pub fn pose(&self, inputs: &PoseInputs) -> Pose {
        match self {
            AnimationState::Idle => idle_pose(inputs.seconds),
            AnimationState::Crouch => crouch_posture(),
            AnimationState::Airborne => airborne_pose(),
            AnimationState::CrouchAirborne => {
                // Nothing is planted, so the tuck stands; `duck_under` then
                // folds it under the short hull.
                let mut pose = crouch_posture();
                airborne_legs(&mut pose);
                pose
            }
            state if state.uses_gait() => {
                gait_pose(inputs, state.travel(inputs), state.gait_style(inputs))
            }
            _ => Pose::rest(),
        }
    }

    /// Whether this state takes steps. `PushingWall` does: its cycle runs on
    /// ground covered, so a body shoving a wall holds its step.
    pub fn uses_gait(&self) -> bool {
        matches!(
            self,
            AnimationState::RunForward
                | AnimationState::RunBackward
                | AnimationState::StrafeLeft
                | AnimationState::StrafeRight
                | AnimationState::CrouchStrafeLeft
                | AnimationState::CrouchStrafeRight
                | AnimationState::PushingWall
                | AnimationState::CrouchWalk
                | AnimationState::CrouchWalkBackward
        )
    }

    /// Measured travel where there is any, else the state's own direction: a
    /// display standing still has nothing to measure.
    fn travel(&self, inputs: &PoseInputs) -> Vec2 {
        if inputs.travel == Vec2::ZERO {
            direction_of(*self)
        } else {
            inputs.travel.normalize_or_zero()
        }
    }

    /// A crouched walk is its own gait, not the upright one played lower.
    fn gait_style(&self, inputs: &PoseInputs) -> GaitStyle {
        match self {
            AnimationState::CrouchWalk | AnimationState::CrouchWalkBackward => GaitStyle::CROUCHED,
            AnimationState::StrafeLeft | AnimationState::StrafeRight => GaitStyle::SIDESTEP,
            AnimationState::CrouchStrafeLeft | AnimationState::CrouchStrafeRight => {
                GaitStyle::CROUCHED_SIDESTEP
            }
            _ => GaitStyle::upright(inputs.speed),
        }
    }

    /// How much of a stride each foot is down for, if this state takes steps.
    pub fn stance(&self, inputs: &PoseInputs) -> Option<f32> {
        self.uses_gait().then(|| self.gait_style(inputs).shape.stance)
    }

    /// Which way this body is going relative to the way its state faces — the
    /// axis a [`StateAnimation::Blend`] is blended along.
    pub fn heading(&self, inputs: &PoseInputs) -> f32 {
        super::gait::heading_between(direction_of(*self), self.travel(inputs))
    }

    pub fn foot_offsets(&self, inputs: &PoseInputs) -> [FootOffset; 2] {
        if self.uses_gait() {
            gait_foot_offsets(inputs.stride, self.travel(inputs), self.gait_style(inputs).shape)
        } else {
            [FootOffset::default(); 2]
        }
    }
}

/// Body animations authored as data, per state. A state without a clip uses
/// its built-in function, so states migrate one at a time. Empty by default.
#[derive(Resource, Debug, Default, Clone)]
pub struct ClipLibrary {
    by_state: HashMap<AnimationState, Arc<StateAnimation>>,
}

impl ClipLibrary {
    pub fn get(&self, state: AnimationState) -> Option<&Arc<StateAnimation>> {
        self.by_state.get(&state)
    }

    pub fn insert(&mut self, state: AnimationState, animation: impl Into<StateAnimation>) {
        self.by_state.insert(state, Arc::new(animation.into()));
    }

    /// Put a state back on its built-in pose, for a clip deleted, reverted, or
    /// missing from a newly loaded pack.
    pub fn remove(&mut self, state: AnimationState) {
        self.by_state.remove(&state);
    }

    pub fn is_empty(&self) -> bool {
        self.by_state.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_state.len()
    }

    /// Which states are being played from data rather than from code.
    pub fn states(&self) -> impl Iterator<Item = AnimationState> + '_ {
        self.by_state.keys().copied()
    }
}

/// The state machine, and the only thing that writes a [`Pose`].
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct SkeletonAnimator {
    state: AnimationState,
    /// The state being blended out of; `blend` runs 0 to 1 over [`BLEND_TIME`].
    previous: AnimationState,
    blend: f32,
    /// Time in the current state; what [`MIN_DWELL`] is measured against.
    elapsed: f32,
}

impl SkeletonAnimator {
    pub fn state(&self) -> AnimationState {
        self.state
    }

    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// The state being blended out of. [`Self::blend`] is 1 once settled.
    pub fn previous(&self) -> AnimationState {
        self.previous
    }

    pub fn blend(&self) -> f32 {
        self.blend
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
        self.blend = (self.blend + dt / BLEND_TIME).min(1.0);
        if wanted == self.state {
            return;
        }
        // Leaving an urgent state matters as much as entering one, so a jump
        // out of a very short idle still plays.
        if self.elapsed >= MIN_DWELL || wanted.is_urgent() || self.state.is_urgent() {
            // An interrupted blend restarts from the current state, losing
            // the pose it was part way to; not visible at this blend length.
            self.previous = self.state;
            self.blend = 0.0;
            self.state = wanted;
            self.elapsed = 0.0;
        }
    }

    /// The pose this body holds, before any corrections.
    pub fn pose_at(&self, inputs: &PoseInputs) -> Pose {
        self.state.pose(inputs)
    }
}

/// Pin a body to one state and ignore its requests, for showing an animation
/// rather than driving a character.
#[derive(
    Component, Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize,
)]
pub struct ForcedAnimation(pub AnimationState);

/// How long one idle cycle takes, in seconds.
pub const IDLE_PERIOD: f32 = 3.4;

/// How far the hips drop at the bottom of the cycle, in hip-heights so it
/// reads the same on every build.
const IDLE_BOB: f32 = 0.022;

/// How far the chest leans forward at the bottom, in radians.
const IDLE_LEAN: f32 = 0.045;

/// How far the arms swing, in radians.
const IDLE_ARM_SWING: f32 = 0.05;

/// Radians.
const IDLE_ELBOW_BEND: f32 = 0.3;

/// Left and right arm forward and up, in radians. The arm opposite the
/// leading leg carries higher, as when running.
const AIRBORNE_ARM_SWING: [f32; 2] = [0.5, 0.9];
const AIRBORNE_ELBOW_BEND: f32 = 1.3;

/// Hip raise and knee bend in the air, in radians. One leg leads knee-up and
/// the other trails heel-tucked; legs together read as standing to attention.
/// The knee bends further than the hip lifts, which tucks the heels up.
const AIRBORNE_LEADING: (f32, f32) = (0.55, -1.25);
const AIRBORNE_TRAILING: (f32, f32) = (0.10, -1.05);

/// Shared by the airborne and crouch-airborne poses.
fn airborne_legs(pose: &mut Pose) {
    for ((thigh, shin), (hip, knee)) in [
        ((bone::THIGH_L, bone::SHIN_L), AIRBORNE_LEADING),
        ((bone::THIGH_R, bone::SHIN_R), AIRBORNE_TRAILING),
    ] {
        pose.set(thigh, Quat::from_rotation_x(hip));
        pose.set(shin, Quat::from_rotation_x(knee));
    }
}

/// Off the ground. The legs are written here rather than solved, because
/// nothing is planted ([`AnimationState::plants_feet`]).
fn airborne_pose() -> Pose {
    let mut pose = Pose::rest();
    airborne_legs(&mut pose);

    arms_at_sides(&mut pose, AIRBORNE_ARM_SWING, AIRBORNE_ELBOW_BEND);

    pose
}

/// Standing still: a slow settle into the knees and back up.
///
/// Only the hips and spine; the feet are held by the planting pass in
/// [`crate::common::skeleton::ik`].
fn idle_pose(seconds: f32) -> Pose {
    // Down from rest and back, never up: rest already has the legs straight.
    let cycle = std::f32::consts::TAU * seconds / IDLE_PERIOD;
    let settle = (1.0 - cycle.cos()) * 0.5;

    let mut pose = Pose::rest();
    pose.root_offset = Vec3::NEG_Y * (settle * IDLE_BOB);

    // The chest leans and the arms trail a quarter cycle behind, so it reads
    // as weight rather than translation.
    pose.set(bone::CHEST, Quat::from_rotation_x(settle * IDLE_LEAN));
    pose.set(bone::NECK, Quat::from_rotation_x(-settle * IDLE_LEAN * 0.6));

    // Both arms sway together; opposed swings read as walking.
    let trail = (cycle - std::f32::consts::FRAC_PI_2).sin() * IDLE_ARM_SWING;
    arms_at_sides(&mut pose, [trail, trail], IDLE_ELBOW_BEND);

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

    /// The idle lowers the hips; the planting pass must hold the feet still, on
    /// every build.
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
                    None,
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

    /// A planted-feet test passes on an animation that does nothing.
    #[test]
    fn the_idle_settles_and_comes_back_up() {
        let standing = bone_at(Proportions::DEFAULT, &Pose::rest(), bone::HEAD).1.y;
        let bottom = bone_at(Proportions::DEFAULT, &idle_at(IDLE_PERIOD * 0.5), bone::HEAD).1.y;
        let back_up = bone_at(Proportions::DEFAULT, &idle_at(IDLE_PERIOD), bone::HEAD).1.y;

        assert!(standing - bottom > 0.015, "the head only dropped {} m", standing - bottom);
        assert!((standing - back_up).abs() < 1e-4, "the cycle does not return to where it started");
    }

    /// The idle hangs its arms at its sides, not in the A-pose.
    #[test]
    fn the_idle_stands_with_its_arms_at_its_sides() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let hand = |pose: &Pose| {
            skeleton
                .posed_bones(pose, &Transform::IDENTITY)
                .into_iter()
                .find(|posed| posed.name == bone::HAND_R)
                .unwrap()
                .tail
        };

        let resting = hand(&Pose::rest()).x;
        for seconds in [0.0, 0.7, 1.6, 2.9] {
            let idling = hand(&idle_at(seconds)).x;
            assert!(
                idling < resting * 0.6,
                "at {seconds}s the hand is {idling:.2} out, against {resting:.2} at rest"
            );
        }
    }

    /// Legs together in the air read as standing to attention.
    #[test]
    fn the_airborne_pose_lifts_both_feet_and_splits_them() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::IDENTITY;
        let ankle = |pose: &Pose, name: &str| {
            skeleton
                .posed_bones(pose, &root)
                .into_iter()
                .find(|posed| posed.name == name)
                .unwrap()
                .tail
        };

        let flying = AnimationState::Airborne.pose(&PoseInputs::default());
        for name in [bone::SHIN_L, bone::SHIN_R] {
            let lift = ankle(&flying, name).y - ankle(&Pose::rest(), name).y;
            assert!(lift > 0.08, "{name} is only {lift:.2} m off where it stands");
        }

        // One in front of the other, by enough to see.
        let split = ankle(&flying, bone::SHIN_L).z - ankle(&flying, bone::SHIN_R).z;
        assert!(split.abs() > 0.1, "the feet are {split:.2} m apart front to back");
    }

    /// A one-sided idle leans, visibly so beside other bodies.
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

    /// The same moment gives the same pose on any machine.
    #[test]
    fn the_same_moment_gives_the_same_pose() {
        let one = idle_at(12.25);
        let other = idle_at(12.25);
        assert_eq!(one.root_offset, other.root_offset);
        assert_eq!(one.joint(bone::THIGH_L), other.joint(bone::THIGH_L));
    }

    /// Seconds come from the tick count, so equal ticks give equal time however
    /// long each machine has run.
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

    /// A phase depends only on its id, and consecutive ids are scattered so a
    /// grid does not bounce as a wave.
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

    /// Input and collision each write one fact; neither knows the animation.
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

    /// Ducking outranks what the legs were asked to do.
    #[test]
    fn a_ducked_body_crouches_whether_or_not_it_is_moving() {
        let still = BodyRequests { crouching: true, ..default() };
        assert_eq!(AnimationState::from_requests(&still), AnimationState::Crouch);

        let moving = BodyRequests { crouching: true, running_forward: true, ..default() };
        assert_eq!(AnimationState::from_requests(&moving), AnimationState::CrouchWalk);

        // A crouch-jump is both: folded, with the legs of a jump.
        let jumping = BodyRequests { crouching: true, airborne: true, ..default() };
        assert_eq!(
            AnimationState::from_requests(&jumping),
            AnimationState::CrouchAirborne
        );
        assert!(AnimationState::CrouchAirborne.ducks(), "a crouch-jump does not fold");
        assert!(
            !AnimationState::CrouchAirborne.plants_feet(),
            "a crouch-jump has its feet on a floor it has left"
        );
    }

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

    /// `wall_ahead` flickers at the collision rate along a wall; the state must
    /// not.
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

    /// Elapsed time restarts with the state and runs while it holds.
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

    /// A state change blends rather than cutting, and the blend finishes.
    #[test]
    fn a_state_change_blends_rather_than_snapping() {
        let mut animator = SkeletonAnimator::default();
        let crouching = BodyRequests { crouching: true, ..default() };
        animator.advance(&BodyRequests::default(), 0.5);
        assert_eq!(animator.blend(), 1.0, "a settled body is still blending");

        animator.advance(&crouching, 1.0 / 64.0);
        assert_eq!(animator.state(), AnimationState::Crouch);
        assert_eq!(animator.previous(), AnimationState::Idle);
        assert!(animator.blend() < 0.5, "the change was most of the way done immediately");

        for _ in 0..16 {
            animator.advance(&crouching, 1.0 / 64.0);
        }
        assert_eq!(animator.blend(), 1.0);
    }

    /// Sideways is four states: two sides, each standing or ducked.
    #[test]
    fn stepping_sideways_has_a_side_and_a_stance() {
        let left = BodyRequests { strafing_left: true, ..default() };
        let right = BodyRequests { strafing_right: true, ..default() };
        assert_eq!(AnimationState::from_requests(&left), AnimationState::StrafeLeft);
        assert_eq!(AnimationState::from_requests(&right), AnimationState::StrafeRight);

        let ducked = BodyRequests { crouching: true, ..left };
        assert_eq!(
            AnimationState::from_requests(&ducked),
            AnimationState::CrouchStrafeLeft
        );

        // Forwards plus sideways animates as forwards.
        let diagonal = BodyRequests { running_forward: true, ..right };
        assert_eq!(AnimationState::from_requests(&diagonal), AnimationState::RunForward);
    }

    /// Without its own state, a body shuffling backwards steps forwards.
    #[test]
    fn a_ducked_body_walking_backwards_has_its_own_state() {
        let back = BodyRequests { crouching: true, running_backward: true, ..default() };
        assert_eq!(
            AnimationState::from_requests(&back),
            AnimationState::CrouchWalkBackward
        );

        use crate::common::skeleton::gait::direction_of;
        assert_eq!(direction_of(AnimationState::CrouchWalkBackward), Vec2::NEG_Y);
        assert_eq!(direction_of(AnimationState::CrouchWalk), Vec2::Y);
    }

    /// A forced state ignores requests but still ages.
    #[test]
    fn a_forced_state_ignores_requests() {
        let mut animator = SkeletonAnimator::default();
        animator.force(AnimationState::Airborne, 0.5);
        animator.force(AnimationState::Airborne, 0.5);
        assert_eq!(animator.state(), AnimationState::Airborne);
        assert!((animator.elapsed() - 0.5).abs() < 1e-6);
    }
}

