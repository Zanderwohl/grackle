//! Walking and running: where the feet go, and when.
//!
//! The cycle is driven by **distance travelled**, not by the clock. That one
//! decision is what stops feet sliding: during the half of the cycle a foot is
//! planted, it moves backwards through the body's own frame at exactly the
//! rate the body moves forwards, so it stands still in the world. Tie the
//! cycle to time instead and the two rates agree only at one speed.
//!
//! It also gives a few things away for free. A body pushing at a wall covers
//! no distance, so its cycle does not advance and its legs stop moving without
//! anything having to notice. A body that speeds up takes the same strides
//! more often rather than the same strides faster.
//!
//! Two units, each matching what it scales with:
//!
//! - **Foot offsets are in leg-lengths**, because a leg is what has to reach
//!   them. A long-legged build takes longer steps in metres and the identical
//!   step as a fraction of itself.
//! - **Root offsets are in hip-heights**, which is what [`Pose::root_offset`]
//!   is measured in.
//!
//! Nothing here is a clip and nothing here is keyframed. A gait is a function
//! of one number — where in the stride you are — which is exactly the sort of
//! motion that should not be authored by hand.

use bevy::prelude::*;

use crate::common::skeleton::rig::{bone, Pose};
use crate::common::skeleton::state::{AnimationState, PoseInputs};

/// How far a foot swings from under the hip, in leg-lengths.
///
/// Bounded by geometry rather than taste: a leg reaching forward has to still
/// touch the floor, and the further forward it goes the more the hips have to
/// drop for it to get there. See [`GAIT_CROUCH`] — the pair are chosen
/// together, and `no_step_is_out_of_reach` is what says they still fit.
const HALF_STRIDE: f32 = 0.4;

/// How far the body travels in one full cycle of two steps, in leg-lengths.
///
/// Falls out of the stride rather than being chosen: a planted foot travels
/// `2 × HALF_STRIDE` backwards through the body over half a cycle, and it can
/// only stand still while doing that if the body covers exactly as much
/// ground.
pub const STRIDE_PER_CYCLE: f32 = 4.0 * HALF_STRIDE;

/// How high a foot lifts at the top of its swing, in leg-lengths.
const FOOT_LIFT: f32 = 0.18;

/// How far the hips drop while moving, in hip-heights.
///
/// A body standing still has straight legs, and a straight leg cannot reach
/// anywhere but the spot directly under its hip. So a gait has to sink into
/// the knees before it can have a stride at all — which is what real ones do,
/// and why this is a constant of the gait rather than an expressive flourish.
const GAIT_CROUCH: f32 = 0.115;

/// How much further the hips dip at the middle of each step, in hip-heights.
const GAIT_BOB: f32 = 0.03;

/// How far the arms swing, in radians.
const ARM_SWING: f32 = 0.55;

/// How far the chest leans into the run, in radians.
const RUN_LEAN: f32 = 0.12;

/// Where in a stride a body is: one full cycle of two steps, wrapping at 1.
///
/// Simulated state rather than something each viewer works out for itself. It
/// is accumulated from distance actually covered, so a server owns it and
/// replicates it the way it replicates a position — a client that started
/// watching halfway through cannot recover it by looking at a clock.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct Gait {
    phase: f32,
}

impl Gait {
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Advance by `distance` metres of ground covered, on a body whose legs
    /// are `leg` metres long.
    ///
    /// Distance rather than time, and the body's own leg length rather than a
    /// fixed number of metres, so two builds running side by side stay in
    /// step as a fraction of their own stride.
    pub fn advance(&mut self, distance: f32, leg: f32) {
        if leg <= 0.0 {
            return;
        }
        self.phase = (self.phase + distance / (STRIDE_PER_CYCLE * leg)).rem_euclid(1.0);
    }

    /// Put a body back at the start of its stride.
    ///
    /// For a body that has stopped: standing still and then setting off again
    /// mid-swing would start the walk on a foot that is already in the air.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }
}

/// Where one foot is, relative to where it rests, in leg-lengths.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FootOffset {
    /// Along the body's facing. Positive is in front of the hip.
    pub ahead: f32,
    /// Off the floor.
    pub lift: f32,
}

/// The left and right foot offsets at a point in the cycle.
///
/// The two feet are half a cycle apart, which is the whole of what makes a
/// walk a walk. `direction` is +1 running forwards and -1 backwards; the cycle
/// itself runs the same way either way, since it is driven by distance covered
/// rather than by which way that distance was.
pub fn foot_offsets(phase: f32, direction: f32) -> [FootOffset; 2] {
    [
        foot_offset(phase, direction),
        foot_offset(phase + 0.5, direction),
    ]
}

fn foot_offset(phase: f32, direction: f32) -> FootOffset {
    let phase = phase.rem_euclid(1.0);

    if phase < 0.5 {
        // Planted. Travels from in front of the hip to behind it, at exactly
        // the rate the body travels forwards — so in the world it does not
        // move at all.
        let travelled = phase / 0.5;
        FootOffset {
            ahead: direction * HALF_STRIDE * (1.0 - 2.0 * travelled),
            lift: 0.0,
        }
    } else {
        // Swinging back to the front, over the top.
        let travelled = (phase - 0.5) / 0.5;
        FootOffset {
            ahead: direction * HALF_STRIDE * (2.0 * travelled - 1.0),
            lift: FOOT_LIFT * (std::f32::consts::PI * travelled).sin(),
        }
    }
}

/// Everything above the knees: the sink into the stride, the dip on each step,
/// the arms.
///
/// The legs are deliberately absent. They are decided by where the feet have
/// to be, which is [`foot_offsets`] and the solver — a gait that also stated
/// its own knee angles would be stating the same thing twice, in units that
/// disagree between builds.
pub fn gait_pose(inputs: &PoseInputs, direction: f32) -> Pose {
    let phase = inputs.stride.rem_euclid(1.0);
    let cycle = std::f32::consts::TAU * phase;

    let mut pose = Pose::rest();
    // Two dips per cycle, one under each step, on top of the constant sink.
    pose.root_offset = Vec3::NEG_Y * (GAIT_CROUCH + GAIT_BOB * (1.0 - (2.0 * cycle).cos()) * 0.5);

    pose.set(bone::CHEST, Quat::from_rotation_x(RUN_LEAN * direction));
    pose.set(bone::NECK, Quat::from_rotation_x(-RUN_LEAN * direction * 0.7));

    // Opposite the leg on the same side, which is what stops a run looking
    // like a march.
    let [left, right] = foot_offsets(phase, direction);
    pose.set(
        bone::UPPER_ARM_L,
        Quat::from_rotation_x(-left.ahead / HALF_STRIDE * ARM_SWING),
    );
    pose.set(
        bone::UPPER_ARM_R,
        Quat::from_rotation_x(-right.ahead / HALF_STRIDE * ARM_SWING),
    );

    pose
}

/// Which way a state's feet travel: forwards, backwards, or not at all.
///
/// Strafing runs the forward cycle. Sideways steps are a different gait rather
/// than this one mirrored, and guessing at one would be worse than a body that
/// visibly runs while it slides sideways.
pub fn direction_of(state: AnimationState) -> f32 {
    match state {
        AnimationState::RunBackward => -1.0,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The claim the whole design rests on: while a foot is planted, the body
    /// moves and the foot does not.
    ///
    /// Walked here one small step at a time, exactly as the fixed step would:
    /// advance the body, advance the cycle by the distance covered, and ask
    /// where the foot is in the world.
    #[test]
    fn a_planted_foot_does_not_slide() {
        let leg = 0.98;
        let mut gait = Gait::default();
        let mut travelled = 0.0;

        let world_position = |gait: &Gait, travelled: f32| {
            let [left, _] = foot_offsets(gait.phase(), 1.0);
            (travelled + left.ahead * leg, left.lift)
        };

        // Start just after the foot plants, and stop before it lifts.
        let (mut expected, _) = world_position(&gait, travelled);
        for _ in 0..40 {
            let step = 0.01;
            travelled += step;
            gait.advance(step, leg);

            let (position, lift) = world_position(&gait, travelled);
            if lift > 0.0 {
                break;
            }
            assert!(
                (position - expected).abs() < 1e-5,
                "the planted foot slid {:.6} m",
                position - expected
            );
            expected = position;
        }
    }

    /// The stride and the sink into the knees are chosen together: a leg
    /// reaching forward still has to touch the floor, and a straight one
    /// cannot. Every step of the cycle has to be somewhere the leg can
    /// actually get to, on every build.
    #[test]
    fn no_step_is_out_of_reach() {
        use crate::common::skeleton::ik::{plant_feet, Reach};
        use crate::common::skeleton::rig::{humanoid, Proportions};

        for proportions in [Proportions::DEFAULT, Proportions::STOCKY, Proportions::LANKY] {
            let skeleton = humanoid(proportions);
            let root = Transform::IDENTITY;

            for step in 0..48 {
                let inputs = PoseInputs { seconds: 0.0, stride: step as f32 / 48.0 };
                let state = AnimationState::RunForward;
                let mut pose = state.pose(&inputs);

                let reached = plant_feet(&skeleton, &mut pose, &root, state.foot_offsets(&inputs));
                assert_eq!(
                    reached,
                    [Reach::Reached; 2],
                    "at stride {:.2} on {proportions:?} the legs cannot make the step",
                    inputs.stride
                );
            }
        }
    }

    /// The same claim as `a_planted_foot_does_not_slide`, but end to end: a
    /// body walking across the world, through the gait, the solver and forward
    /// kinematics, with the foot's actual position measured each step.
    ///
    /// This is the one that would catch a units mix-up between the stride and
    /// the leg it is measured in — the offsets can cancel perfectly and still
    /// put the foot somewhere else.
    #[test]
    fn a_planted_foot_holds_still_through_the_whole_pipeline() {
        use crate::common::skeleton::ik::{finish_pose, leg_length};
        use crate::common::skeleton::rig::{bone, humanoid, Proportions};

        for proportions in [Proportions::DEFAULT, Proportions::LANKY] {
            let skeleton = humanoid(proportions);
            let leg = leg_length(&skeleton);
            let mut gait = Gait::default();
            let mut travelled = 0.0;
            let mut was: Option<Vec3> = None;

            for _ in 0..24 {
                // A tenth of the cycle at a time, forwards along -Z.
                let step = STRIDE_PER_CYCLE * leg / 40.0;
                travelled += step;
                gait.advance(step, leg);

                let root = Transform::from_xyz(0.0, 0.0, -travelled);
                let inputs = PoseInputs { seconds: 0.0, stride: gait.phase() };
                let pose = finish_pose(&skeleton, AnimationState::RunForward, &inputs, &root);
                let ankle = skeleton
                    .posed_bones(&pose, &root)
                    .into_iter()
                    .find(|posed| posed.name == bone::SHIN_L)
                    .unwrap()
                    .tail;

                let planted = foot_offsets(gait.phase(), 1.0)[0].lift == 0.0;
                if let (true, Some(was)) = (planted, was) {
                    assert!(
                        (ankle - was).length() < 1e-3,
                        "the planted foot slid {:.5} m on {proportions:?}",
                        (ankle - was).length()
                    );
                }
                was = planted.then_some(ankle);
            }

            assert!(travelled > 0.0);
        }
    }

    /// A body that covers no ground does not move its legs. This is what makes
    /// walking into a wall look like walking into a wall, and nothing had to
    /// arrange it.
    #[test]
    fn a_body_going_nowhere_does_not_take_steps() {
        let mut gait = Gait::default();
        gait.advance(0.4, 0.98);
        let stopped = gait.phase();

        for _ in 0..64 {
            gait.advance(0.0, 0.98);
        }

        assert_eq!(gait.phase(), stopped);
    }

    /// Two builds running side by side are in step as a fraction of their own
    /// strides: the long-legged one covers more ground per cycle.
    #[test]
    fn stride_scales_with_the_legs_that_take_it() {
        let (short, tall) = (0.7, 1.2);
        let mut small = Gait::default();
        let mut big = Gait::default();

        small.advance(STRIDE_PER_CYCLE * short, short);
        big.advance(STRIDE_PER_CYCLE * tall, tall);

        // Each has taken exactly one cycle, having covered different distances.
        assert!(small.phase().abs() < 1e-5, "{}", small.phase());
        assert!(big.phase().abs() < 1e-5, "{}", big.phase());
    }

    /// One foot down while the other swings, all the way round. Both feet off
    /// the floor at once is a jump, and neither ever leaving it is a shuffle.
    ///
    /// The claim is about them never being airborne together, not about one
    /// always being airborne: at the two instants a foot lifts and lands, it
    /// is at zero height like the other one, and that is a walk changing feet
    /// rather than a gap in the gait.
    #[test]
    fn the_feet_take_turns() {
        for step in 0..64 {
            let phase = step as f32 / 64.0;
            let [left, right] = foot_offsets(phase, 1.0);

            assert!(
                left.lift == 0.0 || right.lift == 0.0,
                "at {phase:.2} both feet are off the floor: {} and {}",
                left.lift,
                right.lift
            );
        }

        // A quarter of the way into each half, the swinging foot is plainly up
        // and the planted one is plainly down.
        let [left, right] = foot_offsets(0.25, 1.0);
        assert_eq!(left.lift, 0.0);
        assert!(right.lift > 0.0, "the right foot never leaves the floor");

        let [left, right] = foot_offsets(0.75, 1.0);
        assert!(left.lift > 0.0, "the left foot never leaves the floor");
        assert_eq!(right.lift, 0.0);
    }

    /// Backwards is the same cycle with the feet travelling the other way —
    /// not the cycle running in reverse, which would be a film played
    /// backwards rather than a body walking backwards.
    #[test]
    fn running_backwards_reverses_the_feet_and_not_the_cycle() {
        let phase = 0.3;
        let [forwards, _] = foot_offsets(phase, 1.0);
        let [backwards, _] = foot_offsets(phase, -1.0);

        assert!((forwards.ahead + backwards.ahead).abs() < 1e-6);
        assert_eq!(forwards.lift, backwards.lift);
    }
}
