//! Walking and running: where the feet go, and when.
//!
//! The cycle is driven by **distance travelled**, not by the clock. That one
//! decision is what stops feet sliding: while a foot is planted it moves
//! backwards through the body's own frame at exactly the rate the body moves
//! forwards, so it stands still in the world. Tie the cycle to time instead
//! and the two rates agree only at one speed.
//!
//! It also gives a few things away for free. A body pushing at a wall covers
//! no distance, so its cycle does not advance and its legs stop moving without
//! anything having to notice.
//!
//! Walking and running are the same cycle with a different **stance
//! fraction** — how much of it a foot spends on the ground. Above a half the
//! two stances overlap, so there is always a foot down and sometimes two: a
//! walk. Below a half they separate and leave a moment in the air: a run. It
//! is read off how fast the body is actually going, never off a key, so a body
//! slowed by a hill or shoved by a lift is animated as what it is doing rather
//! than as what it was asked to do.
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

/// How far a foot swings from under the hip while walking, in leg-lengths.
const WALK_HALF_STRIDE: f32 = 0.42;

/// How far it swings while running.
///
/// Bounded by geometry rather than taste: a leg reaching forward has to still
/// touch the floor, and the further forward it goes the more the hips have to
/// drop for it to get there. See [`GAIT_CROUCH`] — the pair are chosen
/// together, and `no_step_is_out_of_reach` is what says they still fit. This
/// sits about six per cent inside what the legs can actually do.
const RUN_HALF_STRIDE: f32 = 0.48;

/// How much of the cycle a foot spends on the ground while walking.
///
/// Over half, which is the whole definition of a walk: the two stances overlap
/// by what is left over, so there is always a foot down and, for that overlap,
/// two.
const WALK_STANCE: f32 = 0.62;

/// And while running: under half, so the stances separate and leave a moment
/// with both feet off the floor.
///
/// It is also the cadence control. A shorter stance means a longer stride for
/// the same step, so the same speed is covered in fewer, longer cycles instead
/// of a sprint on the spot.
const RUN_STANCE: f32 = 0.32;

/// At or below this, in leg-lengths per second, a body is walking.
const WALK_SPEED: f32 = 2.8;

/// At or above this it is running. Between the two the gait is somewhere in
/// between, rather than switching over.
const RUN_SPEED: f32 = 4.5;

/// How quickly measured speed is allowed to change the gait, in seconds.
///
/// A body's speed over one tick is a noisy thing — a step into a wall, a stair
/// edge — and the gait's shape should not flinch at either. Short enough that
/// setting off and pulling up still read as immediate.
const SPEED_SMOOTHING: f32 = 0.15;

/// How high a foot lifts at the top of its swing, in leg-lengths.
const FOOT_LIFT: f32 = 0.18;

/// How far the hips drop while moving, in hip-heights.
///
/// A body standing still has straight legs, and a straight leg cannot reach
/// anywhere but the spot directly under its hip. So a gait has to sink into
/// the knees before it can have a stride at all — which is what real ones do,
/// and why this is a constant of the gait rather than an expressive flourish.
const GAIT_CROUCH: f32 = 0.13;

/// How much further the hips dip at the middle of each step, in hip-heights.
const GAIT_BOB: f32 = 0.03;

/// How far the arms swing fore and aft, in radians.
const ARM_SWING: f32 = 0.6;

/// How far the arms come in from the rest pose, in radians.
///
/// Rest is an A-pose, which is a shape for building a rig and not a shape
/// anybody runs in. This brings the arms down to the sides — about a third of
/// a turn in from where they hang at rest, which leaves them a few degrees off
/// vertical rather than out on the diagonal.
const ARM_TUCK: f32 = 0.6;

/// How far the elbows are held bent while running, in radians.
const ELBOW_BEND: f32 = 1.1;

/// How far the chest leans into the run, in radians.
const RUN_LEAN: f32 = 0.12;

/// The shape of a cycle at one speed: how long a foot is down, and how far it
/// swings.
///
/// Interpolated rather than switched. Walking and running are genuinely
/// different gaits, but a hard change of stance fraction moves every foot at
/// once — the pose either side of the threshold is not the same pose, so the
/// body would visibly snap at whatever speed the line was drawn at. Sliding
/// between them means the moment a body stops having a foot down is a
/// consequence of its speed rather than a decision anybody made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GaitShape {
    /// How much of the cycle each foot is on the ground.
    pub stance: f32,
    /// How far a foot swings from under the hip, in leg-lengths.
    pub half_stride: f32,
}

impl GaitShape {
    /// The shape a body moving at `speed` leg-lengths per second runs in.
    pub fn for_speed(speed: f32) -> GaitShape {
        // Smoothed at both ends, so neither the start of the transition nor
        // the end of it is a corner.
        let t = ((speed - WALK_SPEED) / (RUN_SPEED - WALK_SPEED)).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);

        GaitShape {
            stance: WALK_STANCE + (RUN_STANCE - WALK_STANCE) * t,
            half_stride: WALK_HALF_STRIDE + (RUN_HALF_STRIDE - WALK_HALF_STRIDE) * t,
        }
    }

    /// How far the body travels in one full cycle of two steps, in
    /// leg-lengths.
    ///
    /// Falls out of the stride and the stance rather than being chosen. A
    /// planted foot travels `2 × half_stride` backwards through the body while
    /// it is down, and it can only stand still doing that if the body covers
    /// exactly as much ground in the same time — which is `stance` of the
    /// cycle.
    pub fn stride_per_cycle(&self) -> f32 {
        2.0 * self.half_stride / self.stance
    }

    /// A duck-walk: short steps, both feet down most of the time, and no
    /// speed at which it turns into anything else.
    pub const CROUCHED: GaitShape = GaitShape {
        stance: CROUCH_STANCE,
        half_stride: CROUCH_HALF_STRIDE,
    };

    /// Whether this gait leaves the ground: a run does, a walk does not.
    ///
    /// Two stances half a cycle apart overlap exactly when they take up more
    /// than the whole of it between them.
    pub fn has_flight(&self) -> bool {
        self.stance < 0.5
    }
}

/// How much of the cycle a crouched foot spends down.
///
/// Well over half: a duck-walk always has a foot on the floor, and usually
/// two. Nobody runs while crouched.
const CROUCH_STANCE: f32 = 0.68;

/// How far a crouched foot swings, in leg-lengths.
///
/// Short. The legs are already folded up, and the shuffle is what makes
/// crouching cost something to move in.
const CROUCH_HALF_STRIDE: f32 = 0.26;

/// How far the hips drop to crouch, in hip-heights.
///
/// Deep enough that the drawn body fits inside the box a crouched body
/// collides with — `a_crouched_body_fits_under_its_own_ceiling` is what holds
/// the two together, since a body drawn standing taller than the gap it just
/// ducked through would be worse than not drawing it at all.
pub const CROUCH_DEPTH: f32 = 0.70;

/// How far the chest folds forward while crouched, in radians.
pub const CROUCH_LEAN: f32 = 1.02;

/// The shape of a cycle at one speed: how long a foot is down, and how far it
/// swings.
///
/// Simulated state rather than something each viewer works out for itself. It
/// is accumulated from distance actually covered, so a server owns it and
/// replicates it the way it replicates a position — a client that started
/// watching halfway through cannot recover it by looking at a clock.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct Gait {
    phase: f32,
    speed: f32,
}

impl Gait {
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// How fast the body is going, in leg-lengths per second.
    ///
    /// Measured from the ground it covered rather than taken from whatever
    /// asked it to move, and in leg-lengths so that "walking pace" means the
    /// same thing on every build. It is measured here rather than written by
    /// input because this is where the distance is already known, and two
    /// definitions of how fast a body is going would eventually disagree.
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// The cycle this body's speed puts it in.
    pub fn shape(&self) -> GaitShape {
        GaitShape::for_speed(self.speed)
    }

    /// Advance by `distance` metres of ground covered over `dt` seconds, on a
    /// body whose legs are `leg` metres long.
    ///
    /// Distance rather than time, and the body's own leg length rather than a
    /// fixed number of metres, so two builds running side by side stay in step
    /// as a fraction of their own stride.
    pub fn advance(&mut self, distance: f32, leg: f32, dt: f32) {
        if leg <= 0.0 || dt <= 0.0 {
            return;
        }

        let measured = distance / dt / leg;
        self.speed += (measured - self.speed) * (dt / SPEED_SMOOTHING).min(1.0);

        let stride = self.shape().stride_per_cycle() * leg;
        self.phase = (self.phase + distance / stride).rem_euclid(1.0);
    }

    /// Put a body back at the start of its stride, standing still.
    ///
    /// For a body that has stopped: setting off again mid-swing would start
    /// the walk on a foot that is already in the air.
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.speed = 0.0;
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
pub fn foot_offsets(phase: f32, direction: f32, shape: GaitShape) -> [FootOffset; 2] {
    [
        foot_offset(phase, direction, shape),
        foot_offset(phase + 0.5, direction, shape),
    ]
}

fn foot_offset(phase: f32, direction: f32, shape: GaitShape) -> FootOffset {
    let GaitShape { stance, half_stride } = shape;
    let phase = phase.rem_euclid(1.0);

    if phase < stance {
        // Planted. Travels from in front of the hip to behind it, at exactly
        // the rate the body travels forwards — so in the world it does not
        // move at all.
        let travelled = phase / stance;
        FootOffset {
            ahead: direction * half_stride * (1.0 - 2.0 * travelled),
            lift: 0.0,
        }
    } else {
        // Swinging back to the front, over the top. It has longer to do it in
        // than it had on the ground, which is what a run looks like.
        let travelled = (phase - stance) / (1.0 - stance);
        FootOffset {
            ahead: direction * half_stride * (2.0 * travelled - 1.0),
            lift: FOOT_LIFT * (std::f32::consts::PI * travelled).sin(),
        }
    }
}

/// A gait's posture: how the body carries itself, as opposed to where its feet
/// go.
///
/// Bundled rather than passed one argument at a time because they go together
/// — a crouched walk is a deeper sink, a bigger fold forward, a shorter arm
/// swing and a shorter step, and picking three of the four would just look
/// wrong.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GaitStyle {
    pub shape: GaitShape,
    /// How far the hips sit below where they rest, in hip-heights.
    pub sink: f32,
    /// How far the chest leans into it, in radians.
    pub lean: f32,
    /// How far the arms swing, in radians.
    pub swing: f32,
}

impl GaitStyle {
    /// Walking or running upright, at whatever speed the body is going.
    pub fn upright(speed: f32) -> GaitStyle {
        GaitStyle {
            shape: GaitShape::for_speed(speed),
            sink: GAIT_CROUCH,
            lean: RUN_LEAN,
            swing: ARM_SWING,
        }
    }

    /// Shuffling along ducked.
    pub const CROUCHED: GaitStyle = GaitStyle {
        shape: GaitShape::CROUCHED,
        sink: CROUCH_DEPTH,
        lean: CROUCH_LEAN,
        // Small: the arms are in front of a folded body, not swinging past it.
        swing: 0.2,
    };
}

/// Standing still, ducked.
///
/// The posture a crouched walk is built on, without the stride — so the two
/// cannot drift apart, and a body that stops moving while crouched settles
/// into the same shape it was walking in.
pub fn crouch_posture() -> Pose {
    let mut pose = Pose::rest();
    pose.root_offset = Vec3::NEG_Y * CROUCH_DEPTH;
    pose.set(bone::CHEST, Quat::from_rotation_x(CROUCH_LEAN));
    // The head comes back up to look ahead, rather than at the floor the chest
    // is folded towards.
    pose.set(bone::NECK, Quat::from_rotation_x(-CROUCH_LEAN * 0.6));

    for (arm, forearm, tuck) in [
        (bone::UPPER_ARM_L, bone::FOREARM_L, -ARM_TUCK),
        (bone::UPPER_ARM_R, bone::FOREARM_R, ARM_TUCK),
    ] {
        pose.set(arm, Quat::from_rotation_z(tuck) * Quat::from_rotation_x(0.35));
        pose.set(forearm, Quat::from_rotation_x(ELBOW_BEND));
    }

    pose
}

/// Everything above the knees: the sink into the stride, the dip on each step,
/// the arms.
///
/// The legs are deliberately absent. They are decided by where the feet have
/// to be, which is [`foot_offsets`] and the solver — a gait that also stated
/// its own knee angles would be stating the same thing twice, in units that
/// disagree between builds.
pub fn gait_pose(inputs: &PoseInputs, direction: f32, style: GaitStyle) -> Pose {
    let GaitStyle { shape, sink, lean, swing: arm_swing } = style;
    let phase = inputs.stride.rem_euclid(1.0);
    let cycle = std::f32::consts::TAU * phase;

    let mut pose = Pose::rest();
    // Two dips per cycle, one under each step, on top of the constant sink.
    pose.root_offset = Vec3::NEG_Y * (sink + GAIT_BOB * (1.0 - (2.0 * cycle).cos()) * 0.5);

    // A body walking backwards leans back, not forward — but a crouched one is
    // folded either way, so only the upright part of the lean turns around.
    let leaning = lean * if lean > RUN_LEAN { 1.0 } else { direction };
    pose.set(bone::CHEST, Quat::from_rotation_x(leaning));
    pose.set(bone::NECK, Quat::from_rotation_x(-leaning * 0.7));

    // In to the sides first, then swinging fore and aft about the shoulder it
    // now hangs from — the order matters, because the axis an arm swings about
    // is not the same one before and after it has come in.
    //
    // Opposite the leg on the same side, which is what stops a run looking
    // like a march. Mirrored signs: the two shoulders are mirror images, so
    // the same sign on both would tuck one arm in and throw the other out.
    let [left, right] = foot_offsets(phase, direction, shape);
    for (arm, forearm, tuck, offset) in [
        (bone::UPPER_ARM_L, bone::FOREARM_L, -ARM_TUCK, left),
        (bone::UPPER_ARM_R, bone::FOREARM_R, ARM_TUCK, right),
    ] {
        let swing = -offset.ahead / shape.half_stride * arm_swing;
        pose.set(
            arm,
            Quat::from_rotation_z(tuck) * Quat::from_rotation_x(swing),
        );
        pose.set(forearm, Quat::from_rotation_x(ELBOW_BEND));
    }

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
    use crate::common::skeleton::ik::{finish_pose, leg_length, plant_feet, Reach};
    use crate::common::skeleton::rig::{bone, humanoid, Proportions};

    /// A brisk walk and a hard run, in leg-lengths per second.
    const WALKING: f32 = 2.5;
    const RUNNING: f32 = 7.0;

    /// The claim the whole design rests on: while a foot is planted, the body
    /// moves and the foot does not.
    ///
    /// Walked here one small step at a time, exactly as the fixed step would:
    /// advance the body, advance the cycle by the distance covered, and ask
    /// where the foot is in the world.
    #[test]
    fn a_planted_foot_does_not_slide() {
        let leg = 0.98;
        let shape = GaitShape::for_speed(RUNNING);
        let mut phase = 0.0;
        let mut travelled = 0.0;

        let world_position = |phase: f32, travelled: f32| {
            let [left, _] = foot_offsets(phase, 1.0, shape);
            (travelled + left.ahead * leg, left.lift)
        };

        let (mut expected, _) = world_position(phase, travelled);
        for _ in 0..40 {
            let step = 0.01;
            travelled += step;
            phase += step / (shape.stride_per_cycle() * leg);

            let (position, lift) = world_position(phase, travelled);
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

    /// The same claim, but end to end: a body walking across the world,
    /// through the gait, the solver and forward kinematics, with the foot's
    /// actual position measured each step.
    ///
    /// This is the one that would catch a units mix-up between the stride and
    /// the leg it is measured in — the offsets can cancel perfectly and still
    /// put the foot somewhere else.
    #[test]
    fn a_planted_foot_holds_still_through_the_whole_pipeline() {
        for proportions in [Proportions::DEFAULT, Proportions::LANKY] {
            for speed in [WALKING, RUNNING] {
                let skeleton = humanoid(proportions);
                let leg = leg_length(&skeleton);
                let shape = GaitShape::for_speed(speed);
                let mut phase = 0.0;
                let mut travelled = 0.0;
                let mut was: Option<Vec3> = None;

                for _ in 0..24 {
                    // A fortieth of the cycle at a time, forwards along -Z.
                    let step = shape.stride_per_cycle() * leg / 40.0;
                    travelled += step;
                    phase += step / (shape.stride_per_cycle() * leg);

                    let root = Transform::from_xyz(0.0, 0.0, -travelled);
                    let inputs = PoseInputs { seconds: 0.0, stride: phase, speed };
                    let pose =
                        finish_pose(&skeleton, AnimationState::RunForward, &inputs, &root);
                    let ankle = skeleton
                        .posed_bones(&pose, &root)
                        .into_iter()
                        .find(|posed| posed.name == bone::SHIN_L)
                        .unwrap()
                        .tail;

                    let planted = foot_offsets(phase, 1.0, shape)[0].lift == 0.0;
                    if let (true, Some(was)) = (planted, was) {
                        assert!(
                            (ankle - was).length() < 1e-3,
                            "the planted foot slid {:.5} m at {speed} on {proportions:?}",
                            (ankle - was).length()
                        );
                    }
                    was = planted.then_some(ankle);
                }
            }
        }
    }

    /// The stride and the sink into the knees are chosen together: a leg
    /// reaching forward still has to touch the floor, and a straight one
    /// cannot. Every step of every gait has to be somewhere the leg can
    /// actually get to, on every build.
    #[test]
    fn no_step_is_out_of_reach() {
        for proportions in [Proportions::DEFAULT, Proportions::STOCKY, Proportions::LANKY] {
            let skeleton = humanoid(proportions);
            let root = Transform::IDENTITY;

            for speed_step in 0..=8 {
                let speed = speed_step as f32;
                for step in 0..48 {
                    let inputs = PoseInputs {
                        seconds: 0.0,
                        stride: step as f32 / 48.0,
                        speed,
                    };
                    let state = AnimationState::RunForward;
                    let mut pose = state.pose(&inputs);

                    let reached =
                        plant_feet(&skeleton, &mut pose, &root, state.foot_offsets(&inputs));
                    assert_eq!(
                        reached,
                        [Reach::Reached; 2],
                        "at stride {:.2} and speed {speed} on {proportions:?} the legs cannot \
                         make the step",
                        inputs.stride
                    );
                }
            }
        }
    }

    /// A walk always has a foot down, and for part of the cycle has two. That
    /// is the definition, not a consequence — it is what a stance fraction
    /// over a half means.
    #[test]
    fn a_walk_always_has_a_foot_on_the_ground() {
        let shape = GaitShape::for_speed(WALKING);
        assert!(!shape.has_flight(), "a walk left the ground");

        let mut both_down = 0;
        for step in 0..128 {
            let [left, right] = foot_offsets(step as f32 / 128.0, 1.0, shape);
            assert!(
                left.lift == 0.0 || right.lift == 0.0,
                "at {:.2} a walking body has both feet in the air",
                step as f32 / 128.0
            );
            if left.lift == 0.0 && right.lift == 0.0 {
                both_down += 1;
            }
        }

        assert!(both_down > 0, "a walk with no double support is a run in disguise");
    }

    /// A run takes one foot at a time, with a moment where it has neither.
    #[test]
    fn a_run_leaves_the_ground() {
        let shape = GaitShape::for_speed(RUNNING);
        assert!(shape.has_flight());

        let mut airborne = 0;
        for step in 0..128 {
            let [left, right] = foot_offsets(step as f32 / 128.0, 1.0, shape);
            assert!(
                left.lift > 0.0 || right.lift > 0.0,
                "at {:.2} a running body has both feet planted, which is a walk",
                step as f32 / 128.0
            );
            if left.lift > 0.0 && right.lift > 0.0 {
                airborne += 1;
            }
        }

        assert!(airborne > 0, "the run has no flight phase");

        // Each foot is down for its share of the cycle and no more.
        let planted = (0..128)
            .filter(|step| foot_offsets(*step as f32 / 128.0, 1.0, shape)[0].lift == 0.0)
            .count() as f32
            / 128.0;
        assert!(
            (planted - shape.stance).abs() < 0.02,
            "a foot is down for {planted:.2} of the cycle, not {}",
            shape.stance
        );
    }

    /// Which of the two you get is read off speed and nothing else — and the
    /// change between them is gradual, because a stance fraction that jumped
    /// would move every foot at once and snap the body at whatever speed the
    /// line was drawn at.
    #[test]
    fn the_gait_turns_into_a_run_as_the_body_speeds_up() {
        assert!(!GaitShape::for_speed(0.5).has_flight(), "a stroll left the ground");
        assert!(GaitShape::for_speed(9.0).has_flight(), "a sprint never left the ground");

        let mut previous = GaitShape::for_speed(0.0);
        let mut biggest = 0.0_f32;
        for step in 1..=180 {
            let shape = GaitShape::for_speed(step as f32 / 20.0);
            biggest = biggest.max((shape.stance - previous.stance).abs());
            // Faster is never a longer stance: a body does not dawdle its way
            // into a sprint.
            assert!(shape.stance <= previous.stance + 1e-6);
            previous = shape;
        }

        assert!(biggest < 0.02, "the gait jumps by {biggest} between neighbouring speeds");
    }

    /// Speed is measured from ground covered, not taken from whatever asked
    /// the body to move. A body dragged along is moving; a body pushing at a
    /// wall is not.
    #[test]
    fn speed_is_measured_from_the_ground_covered() {
        let leg = 0.98;
        let dt = 1.0 / 64.0;
        let mut gait = Gait::default();

        // Settled after a moment of running at a constant rate.
        for _ in 0..64 {
            gait.advance(RUNNING * leg * dt, leg, dt);
        }
        assert!(
            (gait.speed() - RUNNING).abs() < 0.1,
            "measured {} rather than {RUNNING}",
            gait.speed()
        );
        assert!(gait.shape().has_flight(), "running fast is not a run");

        // And a body that stops covering ground is not running any more.
        for _ in 0..64 {
            gait.advance(0.0, leg, dt);
        }
        assert!(gait.speed() < 0.5, "a stopped body is still moving at {}", gait.speed());
        assert!(!gait.shape().has_flight());
    }

    /// A body that covers no ground does not move its legs. This is what makes
    /// walking into a wall look like walking into a wall, and nothing had to
    /// arrange it.
    #[test]
    fn a_body_going_nowhere_does_not_take_steps() {
        let dt = 1.0 / 64.0;
        let mut gait = Gait::default();
        gait.advance(0.4, 0.98, dt);
        let stopped = gait.phase();

        for _ in 0..64 {
            gait.advance(0.0, 0.98, dt);
        }

        assert_eq!(gait.phase(), stopped);
    }

    /// Two builds running at the same speed *for them* stay in step: the
    /// long-legged one covers more ground doing it.
    #[test]
    fn stride_scales_with_the_legs_that_take_it() {
        let (short, tall) = (0.7, 1.2);
        let dt = 1.0 / 64.0;
        let mut small = Gait::default();
        let mut big = Gait::default();

        for _ in 0..96 {
            small.advance(RUNNING * short * dt, short, dt);
            big.advance(RUNNING * tall * dt, tall, dt);
        }

        assert!(
            (small.phase() - big.phase()).abs() < 1e-4,
            "{} against {}",
            small.phase(),
            big.phase()
        );
        assert!(small.phase() > 0.0);
    }

    /// The drawn body has to fit inside the box it ducks through gaps with.
    ///
    /// These are two different descriptions of the same crouch — one for
    /// collision, one for the eye — and nothing in the type system keeps them
    /// together. A body drawn standing a head taller than the gap it just
    /// walked through would be worse than not drawing it at all.
    #[test]
    fn a_crouched_body_fits_under_its_own_ceiling() {
        use crate::common::class::CROUCH_HEIGHT;

        for proportions in [Proportions::DEFAULT, Proportions::STOCKY, Proportions::LANKY] {
            let skeleton = humanoid(proportions);
            let root = Transform::IDENTITY;

            for (state, strides) in [
                (AnimationState::Crouch, 1),
                (AnimationState::CrouchWalk, 16),
            ] {
                for step in 0..strides {
                    let inputs = PoseInputs {
                        seconds: step as f32 * 0.3,
                        stride: step as f32 / strides as f32,
                        speed: 1.0,
                    };
                    let pose = finish_pose(&skeleton, state, &inputs, &root);

                    let highest = skeleton
                        .posed_bones(&pose, &root)
                        .iter()
                        .map(|posed| posed.head.y.max(posed.tail.y))
                        .fold(0.0_f32, f32::max);
                    // Exactly under it, not merely below it: a body folded to
                    // the floor would pass a one-sided check and look
                    // ridiculous doing it, and one folded barely at all would
                    // clip through the gap it is ducking into.
                    assert!(
                        (highest - CROUCH_HEIGHT).abs() < 1e-3,
                        "{state:?} on {proportions:?} stands {highest:.2} m tall against a \
                         {CROUCH_HEIGHT} m ceiling"
                    );
                }
            }
        }
    }

    /// Nobody runs while crouched: the duck-walk keeps a foot down whatever
    /// speed the body is somehow going.
    #[test]
    fn a_duck_walk_never_leaves_the_ground() {
        assert!(!GaitShape::CROUCHED.has_flight());
        assert!(GaitShape::CROUCHED.stance > 0.5);

        for step in 0..64 {
            let [left, right] = foot_offsets(step as f32 / 64.0, 1.0, GaitShape::CROUCHED);
            assert!(left.lift == 0.0 || right.lift == 0.0);
        }
    }

    /// Backwards is the same cycle with the feet travelling the other way —
    /// not the cycle running in reverse, which would be a film played
    /// backwards rather than a body walking backwards.
    #[test]
    fn running_backwards_reverses_the_feet_and_not_the_cycle() {
        let shape = GaitShape::for_speed(RUNNING);
        let phase = 0.3;
        let [forwards, _] = foot_offsets(phase, 1.0, shape);
        let [backwards, _] = foot_offsets(phase, -1.0, shape);

        assert!((forwards.ahead + backwards.ahead).abs() < 1e-6);
        assert_eq!(forwards.lift, backwards.lift);
    }

    /// A rest pose is a shape for building a rig, not one for running in. The
    /// arms come in to the sides, and stay mirrored doing it.
    #[test]
    fn the_arms_come_in_from_the_a_pose() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::IDENTITY;
        let hand = |pose: &Pose, name: &str| {
            skeleton
                .posed_bones(pose, &root)
                .into_iter()
                .find(|posed| posed.name == name)
                .unwrap()
                .tail
        };

        let resting = Pose::rest();
        let inputs = PoseInputs { seconds: 0.0, stride: 0.15, speed: RUNNING };
        let running = gait_pose(&inputs, 1.0, GaitStyle::upright(RUNNING));

        for name in [bone::HAND_L, bone::HAND_R] {
            assert!(
                hand(&running, name).x.abs() < hand(&resting, name).x.abs(),
                "{name} is no closer to the body than it is at rest"
            );
        }

        // The two arms do the same thing half a cycle apart, so at any one
        // moment they are not mirror images — one is forward while the other
        // is back. Over a whole cycle they have to even out, and that is the
        // symmetry worth asserting.
        let across_the_cycle = |name: &str| -> f32 {
            (0..32)
                .map(|step| {
                    let inputs = PoseInputs {
                        seconds: 0.0,
                        stride: step as f32 / 32.0,
                        speed: RUNNING,
                    };
                    hand(&gait_pose(&inputs, 1.0, GaitStyle::upright(RUNNING)), name).x
                })
                .sum()
        };
        let lopsided = across_the_cycle(bone::HAND_L) + across_the_cycle(bone::HAND_R);
        assert!(lopsided.abs() < 1e-4, "the arms are lopsided by {lopsided} over a cycle");
    }
}

