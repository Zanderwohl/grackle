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
use serde::{Deserialize, Serialize};

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
pub const ELBOW_BEND: f32 = 1.1;

/// How far the chest leans into the run, in radians.
const RUN_LEAN: f32 = 0.12;

/// How much of a sidestep's cycle a foot spends down.
const SIDESTEP_STANCE: f32 = 0.45;

/// How far a foot steps sideways, in leg-lengths.
///
/// Small, for two reasons that pull the same way. A foot reaching straight out
/// to the side has the same limit a foot reaching forward does, and it starts
/// from a hip that is only a hand's width off the body's centre line — so it
/// runs out of room sooner than a stride does.
const SIDESTEP_HALF_STRIDE: f32 = 0.22;

/// How far the body tips into a sidestep, in radians.
const SIDESTEP_LEAN: f32 = 0.12;

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
    /// How far a foot swings from under the hip going forwards, in
    /// leg-lengths.
    pub half_stride: f32,
    /// And going sideways, which is always less.
    ///
    /// A leg reaching straight out to the side has the same limit as one
    /// reaching forward, but it starts from a hip only a hand's width off the
    /// centre line and has to leave room for its partner — so a sideways step
    /// is a short one whatever the body is doing.
    pub half_stride_across: f32,
}

impl GaitShape {
    /// How far a foot swings when the body is going `travel`, in leg-lengths.
    ///
    /// The two limits as the axes of an ellipse, rather than one or the other:
    /// a diagonal is neither a stride nor a sidestep and taking either number
    /// whole would make it reach too far one way or not far enough the other.
    pub fn half_stride_for(&self, travel: Vec2) -> f32 {
        let travel = travel.normalize_or_zero();
        if travel == Vec2::ZERO {
            return self.half_stride;
        }

        let across = travel.x / self.half_stride_across;
        let along = travel.y / self.half_stride;
        1.0 / (across * across + along * along).sqrt()
    }

    /// How far the body travels in one full cycle of two steps, in
    /// leg-lengths, going `travel`.
    ///
    /// Falls out of the stride and the stance rather than being chosen. A
    /// planted foot travels `2 × half_stride` backwards through the body while
    /// it is down, and it can only stand still doing that if the body covers
    /// exactly as much ground in the same time — which is `stance` of the
    /// cycle. It is why the stride has to be the one for the direction the
    /// body is actually going: a step of one length paid for at another is a
    /// foot that slides.
    pub fn stride_per_cycle(&self, travel: Vec2) -> f32 {
        2.0 * self.half_stride_for(travel) / self.stance
    }
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
            half_stride_across: SIDESTEP_HALF_STRIDE,
        }
    }

    /// A duck-walk: short steps, both feet down most of the time, and no
    /// speed at which it turns into anything else.
    pub const CROUCHED: GaitShape = GaitShape {
        stance: CROUCH_STANCE,
        half_stride: CROUCH_HALF_STRIDE,
        half_stride_across: SIDESTEP_HALF_STRIDE * 0.9,
    };

    /// Stepping sideways: out with the leading foot, closed up with the other.
    pub const SIDESTEP: GaitShape = GaitShape {
        stance: SIDESTEP_STANCE,
        half_stride: SIDESTEP_HALF_STRIDE,
        half_stride_across: SIDESTEP_HALF_STRIDE,
    };

    /// The same, ducked: shorter still, and both feet down more of the time.
    pub const CROUCHED_SIDESTEP: GaitShape = GaitShape {
        stance: CROUCH_STANCE,
        half_stride: SIDESTEP_HALF_STRIDE * 0.8,
        half_stride_across: SIDESTEP_HALF_STRIDE * 0.8,
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
    /// Where the body is going, in its own frame and in leg-lengths per
    /// second: `x` towards its right, `y` forwards.
    ///
    /// A vector rather than a speed, because a body moving diagonally is doing
    /// both things at once and the feet have to as well. A sign could only
    /// ever say forwards or sideways.
    velocity: Vec2,
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
        self.velocity.length()
    }

    /// Which way it is going, in its own frame: `x` right, `y` forwards.
    ///
    /// Zero when it is not going anywhere, which the gait reads as feet that
    /// stay where they rest.
    pub fn travel(&self) -> Vec2 {
        self.velocity.normalize_or_zero()
    }

    /// The cycle this body's speed puts it in.
    pub fn shape(&self) -> GaitShape {
        GaitShape::for_speed(self.speed())
    }

    /// Advance by the ground covered this step: `moved` metres in the body's
    /// own frame, over `dt` seconds, on legs `leg` metres long.
    ///
    /// Distance rather than time, and the body's own leg length rather than a
    /// fixed number of metres, so two builds running side by side stay in step
    /// as a fraction of their own stride.
    pub fn advance(&mut self, moved: Vec2, leg: f32, dt: f32) {
        if leg <= 0.0 || dt <= 0.0 {
            return;
        }

        let measured = moved / dt / leg;
        self.velocity += (measured - self.velocity) * (dt / SPEED_SMOOTHING).min(1.0);

        let stride = self.shape().stride_per_cycle(self.travel()) * leg;
        self.phase = (self.phase + moved.length() / stride).rem_euclid(1.0);
    }

    /// Put a body back at the start of its stride, standing still.
    ///
    /// For a body that has stopped: setting off again mid-swing would start
    /// the walk on a foot that is already in the air.
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.velocity = Vec2::ZERO;
    }
}

/// How fast a body with no movement to measure should look like it is going,
/// in leg-lengths per second.
///
/// For bodies that are being shown rather than played — an animation display
/// forced into a run covers no ground, and a cycle driven by ground covered
/// would stand perfectly still. A component rather than a constant because a
/// grid shows the same state at several speeds on purpose: the upright gait
/// changes shape as it speeds up, so a walk and a run are two rows of it.
#[derive(Component, Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct DisplaySpeed(pub f32);

impl Default for DisplaySpeed {
    fn default() -> Self {
        DisplaySpeed(6.0)
    }
}

/// Where one foot is, relative to where it rests, in leg-lengths.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FootOffset {
    /// Along the body's facing. Positive is in front of the hip.
    pub ahead: f32,
    /// Across the body. Positive is towards the character's right.
    pub across: f32,
    /// Off the floor.
    pub lift: f32,
}

/// The left and right foot offsets at a point in the cycle.
///
/// The two feet are half a cycle apart, which is the whole of what makes a
/// walk a walk. `direction` is +1 running forwards and -1 backwards; the cycle
/// itself runs the same way either way, since it is driven by distance covered
/// rather than by which way that distance was.
pub fn foot_offsets(phase: f32, travel: Vec2, shape: GaitShape) -> [FootOffset; 2] {
    [
        foot_offset(phase, travel, shape, -1.0),
        foot_offset(phase + 0.5, travel, shape, 1.0),
    ]
}

/// One foot, `side` being which side of the body it is on: -1 left, +1 right.
fn foot_offset(phase: f32, travel: Vec2, shape: GaitShape, side: f32) -> FootOffset {
    let stance = shape.stance;
    let half_stride = shape.half_stride_for(travel);
    let travel = travel.normalize_or_zero();
    let phase = phase.rem_euclid(1.0);

    // How far this foot is from the middle of its own travel, from in front to
    // behind while it is planted and back round while it is not. The rate on
    // the ground is the whole of the no-slide property, and it does not care
    // which direction the travel is in.
    let (sweep, lift) = if phase < stance {
        (1.0 - 2.0 * (phase / stance), 0.0)
    } else {
        let travelled = (phase - stance) / (1.0 - stance);
        (
            2.0 * travelled - 1.0,
            FOOT_LIFT * (std::f32::consts::PI * travelled).sin(),
        )
    };

    // The step goes the way the body is going: a diagonal steps forward and
    // sideways at once, in the proportions it is actually travelling. Both
    // components cancel the body's own movement along their own axis while the
    // foot is down, which is what keeps a diagonal from sliding.
    FootOffset {
        ahead: travel.y * half_stride * sweep,
        // Each foot keeps to its own side, offset outwards by as much of its
        // reach as the step is sideways: a leg swinging across its partner
        // would swing through it, and a body running straight has no such
        // problem to solve.
        across: travel.x * half_stride * sweep + side * half_stride * travel.x.abs(),
        lift,
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

    /// Stepping sideways, upright.
    pub const SIDESTEP: GaitStyle = GaitStyle {
        shape: GaitShape::SIDESTEP,
        sink: GAIT_CROUCH,
        // No fold: a body stepping sideways does not lean over its own feet,
        // it tips towards where it is going, which `gait_pose` adds.
        lean: 0.0,
        swing: 0.25,
    };

    /// And ducked.
    pub const CROUCHED_SIDESTEP: GaitStyle = GaitStyle {
        shape: GaitShape::CROUCHED_SIDESTEP,
        sink: CROUCH_DEPTH,
        lean: CROUCH_LEAN,
        swing: 0.15,
    };
}

/// Bring the arms in from the rest pose and hang them at the sides, swinging.
///
/// Shared by every posture that is not the rest pose, which is all of them: an
/// A-pose is a shape for building a rig, and a body that stood in one while
/// idling and came out of it to run would change shape for no reason anybody
/// could see. `swing` is the fore-and-aft turn for the left and right arms, in
/// radians, and `elbow` is how far the elbows are held bent.
///
/// Tuck first, then swing: the axis an arm swings about is not the same one
/// before and after it has come in. The signs are mirrored because the two
/// shoulders are, so the same sign on both would tuck one arm in and throw the
/// other out.
pub fn arms_at_sides(pose: &mut Pose, swing: [f32; 2], elbow: f32) {
    for (arm, forearm, tuck, swing) in [
        (bone::UPPER_ARM_L, bone::FOREARM_L, -ARM_TUCK, swing[0]),
        (bone::UPPER_ARM_R, bone::FOREARM_R, ARM_TUCK, swing[1]),
    ] {
        pose.set(arm, Quat::from_rotation_z(tuck) * Quat::from_rotation_x(swing));
        pose.set(forearm, Quat::from_rotation_x(elbow));
    }
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

    // Held in front of a folded body rather than hanging past it.
    arms_at_sides(&mut pose, [0.35, 0.35], ELBOW_BEND);

    pose
}

/// Everything above the knees: the sink into the stride, the dip on each step,
/// the arms.
///
/// The legs are deliberately absent. They are decided by where the feet have
/// to be, which is [`foot_offsets`] and the solver — a gait that also stated
/// its own knee angles would be stating the same thing twice, in units that
/// disagree between builds.
pub fn gait_pose(inputs: &PoseInputs, travel: Vec2, style: GaitStyle) -> Pose {
    let GaitStyle { shape, sink, lean, swing: arm_swing } = style;
    let phase = inputs.stride.rem_euclid(1.0);
    let cycle = std::f32::consts::TAU * phase;

    let mut pose = Pose::rest();
    // Two dips per cycle, one under each step, on top of the constant sink.
    pose.root_offset = Vec3::NEG_Y * (sink + GAIT_BOB * (1.0 - (2.0 * cycle).cos()) * 0.5);

    // Leaning into it on both axes at once, in the proportions it is
    // travelling: forwards over its own stride, and tipped towards where it is
    // stepping sideways. A body walking backwards leans back — but a crouched
    // one is folded either way, so only the upright part of the lean turns
    // around with it.
    let folded = lean > RUN_LEAN;
    let leaning = lean * if folded { 1.0 } else { travel.y };
    pose.set(
        bone::CHEST,
        Quat::from_rotation_z(travel.x * SIDESTEP_LEAN) * Quat::from_rotation_x(leaning),
    );
    pose.set(bone::NECK, Quat::from_rotation_x(-leaning * 0.7));

    // Opposite the leg on the same side, which is what stops a run looking
    // like a march.
    let [left, right] = foot_offsets(phase, travel, shape);
    let swing_of = |offset: FootOffset| {
        // Opposite whichever way the leg on that side is travelling, which for
        // a diagonal is a bit of both.
        let along = offset.ahead * travel.y.signum() + offset.across * travel.x.signum();
        -along / shape.half_stride_for(travel) * arm_swing
    };
    arms_at_sides(&mut pose, [swing_of(left), swing_of(right)], ELBOW_BEND);

    pose
}

/// Which way a state's feet travel, in the body's own frame.
///
/// A coarse answer, and only used for bodies with no movement to measure —
/// a display standing on the spot. A body that is actually going somewhere
/// takes its direction from where it went, which is the only way a diagonal
/// can be one: the states are labels, and there is no `RunForwardAndLeft`.
pub fn direction_of(state: AnimationState) -> Vec2 {
    match state {
        AnimationState::RunBackward | AnimationState::CrouchWalkBackward => Vec2::NEG_Y,
        AnimationState::StrafeLeft | AnimationState::CrouchStrafeLeft => Vec2::NEG_X,
        AnimationState::StrafeRight | AnimationState::CrouchStrafeRight => Vec2::X,
        _ => Vec2::Y,
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

    /// Every shape a body is actually built in, plus the two sample builds
    /// that exist to be extremes.
    ///
    /// The constraints below are the reason the roster's numbers are safe to
    /// change: a stride that cannot be reached or a crouch that cannot fold
    /// fails here rather than in the game.
    fn every_build() -> Vec<Proportions> {
        use crate::common::class::Class;
        use strum::IntoEnumIterator;

        Class::iter()
            .map(|class| class.proportions())
            .chain([Proportions::STOCKY, Proportions::LANKY])
            .collect()
    }

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
        let travel = Vec2::Y;
        let mut phase = 0.0;
        let mut travelled = 0.0;

        let world_position = |phase: f32, travelled: f32| {
            let [left, _] = foot_offsets(phase, Vec2::Y, shape);
            (travelled + left.ahead * leg, left.lift)
        };

        let (mut expected, _) = world_position(phase, travelled);
        for _ in 0..40 {
            let step = 0.01;
            travelled += step;
            phase += step / (shape.stride_per_cycle(travel) * leg);

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
                let travel = Vec2::Y;
                let mut phase = 0.0;
                let mut travelled = 0.0;
                let mut was: Option<Vec3> = None;

                for _ in 0..24 {
                    // A fortieth of the cycle at a time, forwards along -Z.
                    let step = shape.stride_per_cycle(travel) * leg / 40.0;
                    travelled += step;
                    phase += step / (shape.stride_per_cycle(travel) * leg);

                    let root = Transform::from_xyz(0.0, 0.0, -travelled);
                    let inputs = PoseInputs { seconds: 0.0, stride: phase, speed, travel: Vec2::Y };
                    let pose =
                        finish_pose(&skeleton, AnimationState::RunForward, &inputs, &root);
                    let ankle = skeleton
                        .posed_bones(&pose, &root)
                        .into_iter()
                        .find(|posed| posed.name == bone::SHIN_L)
                        .unwrap()
                        .tail;

                    let planted = foot_offsets(phase, Vec2::Y, shape)[0].lift == 0.0;
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
    /// actually get to, on every build — and in every direction.
    ///
    /// The directions matter as much as the builds. A stride checked only
    /// along the axes passes happily while a diagonal reaches past what the
    /// leg can do, and a foot that cannot get to its target lands short and
    /// slides, which is the one thing the whole arrangement exists to prevent.
    #[test]
    fn no_step_is_out_of_reach() {
        let corner = Vec2::new(1.0, 1.0).normalize();
        let directions = [
            Vec2::Y,
            Vec2::NEG_Y,
            Vec2::X,
            Vec2::NEG_X,
            corner,
            Vec2::new(-corner.x, corner.y),
            Vec2::new(corner.x, -corner.y),
            Vec2::new(-corner.x, -corner.y),
        ];

        for proportions in every_build() {
            let skeleton = humanoid(proportions);
            let root = Transform::IDENTITY;

            for speed_step in 0..4 {
                let speed = speed_step as f32 * 3.0;
                for step in 0..16 {
                    for travel in directions {
                        let inputs = PoseInputs {
                            seconds: 0.0,
                            stride: step as f32 / 16.0,
                            speed,
                            travel,
                        };
                        for state in [
                            AnimationState::RunForward,
                            AnimationState::StrafeRight,
                            AnimationState::CrouchWalk,
                            AnimationState::CrouchStrafeLeft,
                        ] {
                            let mut pose = state.pose(&inputs);
                            let reached = plant_feet(
                                &skeleton,
                                &mut pose,
                                &root,
                                state.foot_offsets(&inputs),
                            );
                            assert_eq!(
                                reached,
                                [Reach::Reached; 2],
                                "in {state:?} going {travel} at stride {:.2} and speed {speed} \
                                 on {proportions:?} the legs cannot make the step",
                                inputs.stride
                            );
                        }
                    }
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
            let [left, right] = foot_offsets(step as f32 / 128.0, Vec2::Y, shape);
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
            let [left, right] = foot_offsets(step as f32 / 128.0, Vec2::Y, shape);
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
            .filter(|step| foot_offsets(*step as f32 / 128.0, Vec2::Y, shape)[0].lift == 0.0)
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
            gait.advance(Vec2::Y * RUNNING * leg * dt, leg, dt);
        }
        assert!(
            (gait.speed() - RUNNING).abs() < 0.1,
            "measured {} rather than {RUNNING}",
            gait.speed()
        );
        assert!(gait.shape().has_flight(), "running fast is not a run");

        // And a body that stops covering ground is not running any more.
        for _ in 0..64 {
            gait.advance(Vec2::ZERO, leg, dt);
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
        gait.advance(Vec2::Y * 0.4, 0.98, dt);
        let stopped = gait.phase();

        for _ in 0..64 {
            gait.advance(Vec2::ZERO, 0.98, dt);
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
            small.advance(Vec2::Y * RUNNING * short * dt, short, dt);
            big.advance(Vec2::Y * RUNNING * tall * dt, tall, dt);
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

        for proportions in every_build() {
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
                        ..default()
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
            let [left, right] = foot_offsets(step as f32 / 64.0, Vec2::Y, GaitShape::CROUCHED);
            assert!(left.lift == 0.0 || right.lift == 0.0);
        }
    }

    /// A sidestep does not slide either — the same cancellation, along the
    /// axis the body is actually travelling.
    ///
    /// Both ways round, because a state that stepped the wrong way would still
    /// hold its feet perfectly while going the other one. That is exactly what
    /// a missing arm in [`direction_of`] looks like: the animation is right
    /// half the time.
    #[test]
    fn a_planted_foot_does_not_slide_sideways() {
        for (state, towards) in [
            (AnimationState::StrafeRight, 1.0),
            (AnimationState::StrafeLeft, -1.0),
            (AnimationState::CrouchStrafeRight, 1.0),
            (AnimationState::CrouchStrafeLeft, -1.0),
        ] {
            let skeleton = humanoid(Proportions::DEFAULT);
            let leg = leg_length(&skeleton);
            let shape = if state.ducks() {
                GaitShape::CROUCHED_SIDESTEP
            } else {
                GaitShape::SIDESTEP
            };
            let travel = Vec2::new(towards, 0.0);
            let mut phase = 0.0;
            let mut travelled = 0.0;
            let mut was: Option<Vec3> = None;

            for _ in 0..24 {
                let step = shape.stride_per_cycle(travel) * leg / 40.0;
                travelled += step;
                phase += step / (shape.stride_per_cycle(travel) * leg);

                // +X is the character's own right, for a body facing -Z.
                let root = Transform::from_xyz(towards * travelled, 0.0, 0.0);
                let inputs = PoseInputs {
                    seconds: 0.0,
                    stride: phase,
                    speed: 2.0,
                    travel: Vec2::new(towards, 0.0),
                };
                let pose = finish_pose(&skeleton, state, &inputs, &root);
                let ankle = skeleton
                    .posed_bones(&pose, &root)
                    .into_iter()
                    .find(|posed| posed.name == bone::SHIN_L)
                    .unwrap()
                    .tail;

                let planted = foot_offsets(phase, Vec2::new(towards, 0.0), shape)[0].lift == 0.0;
                if let (true, Some(was)) = (planted, was) {
                    assert!(
                        (ankle - was).length() < 1e-3,
                        "in {state:?} the planted foot slid {:.5} m",
                        (ankle - was).length()
                    );
                }
                was = planted.then_some(ankle);
            }
        }
    }

    /// Every state that travels has to say which way, or it animates as
    /// whichever direction happens to be the default.
    #[test]
    fn every_gait_state_knows_which_way_it_is_going() {
        for (state, direction) in [
            (AnimationState::RunForward, Vec2::Y),
            (AnimationState::RunBackward, Vec2::NEG_Y),
            (AnimationState::StrafeLeft, Vec2::NEG_X),
            (AnimationState::StrafeRight, Vec2::X),
            (AnimationState::CrouchWalk, Vec2::Y),
            (AnimationState::CrouchWalkBackward, Vec2::NEG_Y),
            (AnimationState::CrouchStrafeLeft, Vec2::NEG_X),
            (AnimationState::CrouchStrafeRight, Vec2::X),
        ] {
            assert_eq!(direction_of(state), direction, "{state:?} goes the wrong way");
        }
    }

    /// And the body tips the way it is going, rather than always the same way.
    #[test]
    fn a_sidestep_leans_towards_where_it_is_going() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::IDENTITY;
        let inputs = PoseInputs { seconds: 0.0, stride: 0.2, speed: 2.0, ..default() };

        let crown = |state: AnimationState| {
            skeleton
                .posed_bones(&state.pose(&inputs), &root)
                .into_iter()
                .find(|posed| posed.name == bone::HEAD)
                .unwrap()
                .tail
                .x
        };

        let right = crown(AnimationState::StrafeRight);
        let left = crown(AnimationState::StrafeLeft);
        assert!(right > 0.0, "stepping right leans to {right}");
        assert!(left < 0.0, "stepping left leans to {left}");
        assert!((right + left).abs() < 1e-5, "the two leans do not mirror");
    }

    /// A diagonal steps diagonally: both feet travel forwards and sideways at
    /// once, in the proportions the body is actually going, rather than the
    /// body running straight while it slides across the floor.
    #[test]
    fn a_diagonal_steps_both_ways_at_once() {
        let shape = GaitShape::for_speed(RUNNING);
        let diagonal = Vec2::new(1.0, 1.0).normalize();

        let mut stepped_ahead = false;
        let mut stepped_across = false;
        for step in 0..32 {
            let [left, _] = foot_offsets(step as f32 / 32.0, diagonal, shape);
            stepped_ahead |= left.ahead.abs() > 0.05;
            stepped_across |= left.across.abs() > 0.05;

            // In the proportions it is travelling, not one and then the other.
            if left.ahead.abs() > 1e-4 {
                let sideways = (left.across - left.ahead).abs();
                assert!(sideways < 0.35, "the step is {sideways} out of proportion");
            }
        }
        assert!(stepped_ahead, "a diagonal never steps forwards");
        assert!(stepped_across, "a diagonal never steps sideways");

        // And running straight is still straight: the sideways offset that
        // keeps a sidestep's feet apart must not leak into a run.
        for step in 0..32 {
            let [left, right] = foot_offsets(step as f32 / 32.0, Vec2::Y, shape);
            assert_eq!(left.across, 0.0);
            assert_eq!(right.across, 0.0);
        }
    }

    /// And it does not slide while doing it. The cancellation is per axis, so
    /// a diagonal is the case that catches one axis being handled and the
    /// other forgotten.
    #[test]
    fn a_planted_foot_does_not_slide_on_a_diagonal() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let leg = leg_length(&skeleton);
        let shape = GaitShape::for_speed(RUNNING);
        let travel = Vec2::new(1.0, 1.0).normalize();
        // The body faces -Z, so its own forward and right are these in world.
        let world = Vec3::new(travel.x, 0.0, -travel.y);

        let mut phase = 0.0;
        let mut travelled = 0.0;
        let mut was: Option<Vec3> = None;

        for _ in 0..24 {
            let step = shape.stride_per_cycle(travel) * leg / 40.0;
            travelled += step;
            phase += step / (shape.stride_per_cycle(travel) * leg);

            let root = Transform::from_translation(world * travelled);
            let inputs = PoseInputs {
                seconds: 0.0,
                stride: phase,
                speed: RUNNING,
                travel,
            };
            let pose = finish_pose(&skeleton, AnimationState::RunForward, &inputs, &root);
            let ankle = skeleton
                .posed_bones(&pose, &root)
                .into_iter()
                .find(|posed| posed.name == bone::SHIN_L)
                .unwrap()
                .tail;

            let planted = foot_offsets(phase, travel, shape)[0].lift == 0.0;
            if let (true, Some(was)) = (planted, was) {
                assert!(
                    (ankle - was).length() < 1e-3,
                    "the planted foot slid {:.5} m on the diagonal",
                    (ankle - was).length()
                );
            }
            was = planted.then_some(ankle);
        }
    }

    /// The feet keep to their own sides. A leg that swung across its partner
    /// would swing through it, which is the one thing a sidestep must not do.
    #[test]
    fn the_feet_never_cross_in_a_sidestep() {
        for shape in [GaitShape::SIDESTEP, GaitShape::CROUCHED_SIDESTEP] {
            for direction in [Vec2::NEG_X, Vec2::X] {
                for step in 0..128 {
                    let [left, right] = foot_offsets(step as f32 / 128.0, direction, shape);
                    assert!(
                        left.across < right.across,
                        "at {:.2} the left foot is at {} and the right at {}",
                        step as f32 / 128.0,
                        left.across,
                        right.across
                    );
                }
            }
        }
    }

    /// Stepping left is stepping right in a mirror, and neither is the walk
    /// turned sideways: the feet go across the body, not through it.
    ///
    /// The mirror is half a cycle along, because which foot leads is set by
    /// where in the cycle the body is rather than by which way it is going. A
    /// body that reversed direction mid-shuffle would close up with the foot
    /// it had just stepped out with, which is what people do.
    #[test]
    fn a_sidestep_mirrors_and_goes_sideways() {
        let shape = GaitShape::SIDESTEP;
        for step in 0..32 {
            let phase = step as f32 / 32.0;
            let [left, right] = foot_offsets(phase, Vec2::NEG_X, shape);
            let [mirror_left, mirror_right] = foot_offsets(phase + 0.5, Vec2::X, shape);

            assert_eq!(left.ahead, 0.0, "a sidestep is stepping through the body");
            assert!(left.across != 0.0 || right.across != 0.0);
            assert!(
                (left.across + mirror_right.across).abs() < 1e-6,
                "the left foot at {} does not mirror the right at {}",
                left.across,
                mirror_right.across
            );
            assert!((right.across + mirror_left.across).abs() < 1e-6);
        }
    }

    /// Backwards is the same cycle with the feet travelling the other way —
    /// not the cycle running in reverse, which would be a film played
    /// backwards rather than a body walking backwards.
    #[test]
    fn running_backwards_reverses_the_feet_and_not_the_cycle() {
        let shape = GaitShape::for_speed(RUNNING);
        let phase = 0.3;
        let [forwards, _] = foot_offsets(phase, Vec2::Y, shape);
        let [backwards, _] = foot_offsets(phase, Vec2::NEG_Y, shape);

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
        let inputs = PoseInputs {
            seconds: 0.0,
            stride: 0.15,
            speed: RUNNING,
            travel: Vec2::Y,
        };
        let running = gait_pose(&inputs, Vec2::Y, GaitStyle::upright(RUNNING));

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
                        travel: Vec2::Y,
                    };
                    hand(&gait_pose(&inputs, Vec2::Y, GaitStyle::upright(RUNNING)), name).x
                })
                .sum()
        };
        let lopsided = across_the_cycle(bone::HAND_L) + across_the_cycle(bone::HAND_R);
        assert!(lopsided.abs() < 1e-4, "the arms are lopsided by {lopsided} over a cycle");
    }
}

