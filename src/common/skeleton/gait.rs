//! Walking and running: where the feet go, and when.
//!
//! The cycle is driven by distance travelled, not by the clock: a planted foot
//! moves back through the body's frame at the rate the body moves forward, so
//! it stands still in the world at every speed. A body pushing at a wall
//! covers no distance, so its legs stop.
//!
//! Walking and running are one cycle with a different stance fraction (how
//! much of it a foot is on the ground): above a half there is always a foot
//! down, below a half there is a moment in the air. It is read off measured
//! speed, never off input.
//!
//! Foot offsets are in leg-lengths, since a leg has to reach them; root
//! offsets are in hip-heights, as [`Pose::root_offset`] is.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::skeleton::rig::{bone, Pose};
use crate::common::skeleton::state::{AnimationState, PoseInputs};

/// How far a foot swings from under the hip while walking, in leg-lengths.
const WALK_HALF_STRIDE: f32 = 0.42;

/// How far it swings while running, in leg-lengths.
///
/// Chosen with [`GAIT_CROUCH`]: a leg reaching further forward needs the hips
/// lower to touch the floor. About six per cent inside the legs' reach;
/// `no_step_is_out_of_reach` checks it.
const RUN_HALF_STRIDE: f32 = 0.48;

/// How much of the cycle a foot spends on the ground while walking. Over half,
/// so the stances overlap and there is always a foot down.
const WALK_STANCE: f32 = 0.62;

/// And while running: under half, so both feet leave the floor for a moment.
/// A shorter stance also means longer strides, so speed is covered in fewer
/// cycles rather than by a sprint on the spot.
const RUN_STANCE: f32 = 0.32;

/// At or below this, in leg-lengths per second, a body is walking.
const WALK_SPEED: f32 = 2.8;

/// At or above this it is running; between the two the shape is interpolated.
const RUN_SPEED: f32 = 4.5;

/// Time constant, in seconds, for measured speed to change the gait. One
/// tick's speed is noisy at walls and stair edges.
const SPEED_SMOOTHING: f32 = 0.15;

/// How high a foot lifts at the top of its swing, in leg-lengths.
const FOOT_LIFT: f32 = 0.18;

/// How far the hips drop while moving, in hip-heights. A straight leg reaches
/// only the spot under its hip, so a stride needs bent knees.
const GAIT_CROUCH: f32 = 0.13;

/// How much further the hips dip at the middle of each step, in hip-heights.
const GAIT_BOB: f32 = 0.03;

/// How far the arms swing fore and aft, in radians.
const ARM_SWING: f32 = 0.6;

/// How far the arms come in from the A-pose rest, in radians, leaving them a
/// few degrees off vertical.
const ARM_TUCK: f32 = 0.6;

/// How far the elbows are held bent while running, in radians.
pub const ELBOW_BEND: f32 = 1.1;

/// How far the chest leans into the run, in radians.
const RUN_LEAN: f32 = 0.12;

/// How much of a sidestep's cycle a foot spends down.
const SIDESTEP_STANCE: f32 = 0.45;

/// How far a foot steps sideways, in leg-lengths. Small because the hip is
/// only a hand's width off the centre line, so a sideways reach runs out
/// sooner than a stride.
const SIDESTEP_HALF_STRIDE: f32 = 0.22;

/// How far the body tips into a sidestep, in radians.
const SIDESTEP_LEAN: f32 = 0.12;

/// The shape of a cycle at one speed: how long a foot is down, and how far it
/// swings.
///
/// Interpolated between walk and run rather than switched, because a jump in
/// stance fraction moves every foot at once and the body snaps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GaitShape {
    /// How much of the cycle each foot is on the ground.
    pub stance: f32,
    /// How far a foot swings from under the hip going forwards, in
    /// leg-lengths.
    pub half_stride: f32,
    /// Going sideways, in leg-lengths; always less, since the hip starts near
    /// the centre line and has to leave room for the other leg.
    pub half_stride_across: f32,
}

impl GaitShape {
    /// How far a foot swings going `travel`, in leg-lengths: the forward and
    /// sideways limits as the axes of an ellipse, so a diagonal reaches neither
    /// too far nor too short.
    pub fn half_stride_for(&self, travel: Vec2) -> f32 {
        let travel = travel.normalize_or_zero();
        if travel == Vec2::ZERO {
            return self.half_stride;
        }

        let across = travel.x / self.half_stride_across;
        let along = travel.y / self.half_stride;
        1.0 / (across * across + along * along).sqrt()
    }

    /// Ground covered per cycle of two steps, in leg-lengths, going `travel`.
    ///
    /// A planted foot sweeps `2 × half_stride` back while down for `stance` of
    /// the cycle, and the body must cover the same to keep it still. The stride
    /// must be the one for the actual direction, or the foot slides.
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

    /// Whether this gait leaves the ground: two stances half a cycle apart
    /// separate when each is under half.
    pub fn has_flight(&self) -> bool {
        self.stance < 0.5
    }
}

/// How much of the cycle a crouched foot spends down. Well over half: a
/// duck-walk always has a foot on the floor.
const CROUCH_STANCE: f32 = 0.68;

/// How far a crouched foot swings, in leg-lengths.
const CROUCH_HALF_STRIDE: f32 = 0.26;

/// How far the hips drop to crouch, in hip-heights. Deep enough for the drawn
/// body to fit the crouched collision box
/// (`a_crouched_body_fits_under_its_own_ceiling`).
pub const CROUCH_DEPTH: f32 = 0.70;

/// How far the chest folds forward while crouched, in radians.
pub const CROUCH_LEAN: f32 = 1.02;

/// A body's stride phase and measured velocity, accumulated from ground
/// covered. Not replicated: each machine advances it from the replicated
/// `PhysicsBody`.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct Gait {
    phase: f32,
    /// In the body's own frame, in leg-lengths per second: `x` right, `y`
    /// forwards. A vector so a diagonal drives both axes of the step.
    velocity: Vec2,
}

impl Gait {
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// How fast the body is going, in leg-lengths per second, measured from
    /// ground covered rather than from input.
    pub fn speed(&self) -> f32 {
        self.velocity.length()
    }

    /// Which way it is going, in its own frame: `x` right, `y` forwards. Zero
    /// when stopped.
    pub fn travel(&self) -> Vec2 {
        self.velocity.normalize_or_zero()
    }

    pub fn shape(&self) -> GaitShape {
        GaitShape::for_speed(self.speed())
    }

    /// Advance by the ground covered this step: `moved` metres in the body's own
    /// frame, over `dt` seconds, on legs `leg` metres long. Scaling by leg length
    /// keeps different builds in step as a fraction of their own stride.
    pub fn advance(&mut self, moved: Vec2, leg: f32, dt: f32) {
        if leg <= 0.0 || dt <= 0.0 {
            return;
        }

        let measured = moved / dt / leg;
        self.velocity += (measured - self.velocity) * (dt / SPEED_SMOOTHING).min(1.0);

        let stride = self.shape().stride_per_cycle(self.travel()) * leg;
        self.phase = (self.phase + moved.length() / stride).rem_euclid(1.0);
    }

    /// Back to the start of the stride, standing still, so setting off again
    /// does not begin on a foot already in the air.
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.velocity = Vec2::ZERO;
    }
}

/// Apparent speed, in leg-lengths per second, for a body with no movement to
/// measure, such as an animation display forced into a run. A component so a
/// grid can show one state at several speeds.
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

/// The left and right foot offsets at a point in the cycle, half a cycle
/// apart. Travelling backwards reverses the feet, not the cycle.
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

    // Distance from the middle of this foot's travel: front to back while
    // planted, back round while not. The planted rate is the no-slide property.
    let (sweep, lift) = if phase < stance {
        (1.0 - 2.0 * (phase / stance), 0.0)
    } else {
        let travelled = (phase - stance) / (1.0 - stance);
        (
            2.0 * travelled - 1.0,
            FOOT_LIFT * (std::f32::consts::PI * travelled).sin(),
        )
    };

    // The step follows the travel direction; each component cancels the body's
    // movement along its axis while the foot is down.
    FootOffset {
        ahead: travel.y * half_stride * sweep,
        // Each foot is pushed outwards by the sideways share of its reach, so a
        // sidestep's legs do not swing through each other.
        across: travel.x * half_stride * sweep + side * half_stride * travel.x.abs(),
        lift,
    }
}

/// A gait's posture, as opposed to where its feet go. Bundled because the
/// parts only look right together.
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
        // No fold; `gait_pose` tips the body towards where it is going.
        lean: 0.0,
        swing: 0.25,
    };

    /// Stepping sideways, ducked.
    pub const CROUCHED_SIDESTEP: GaitStyle = GaitStyle {
        shape: GaitShape::CROUCHED_SIDESTEP,
        sink: CROUCH_DEPTH,
        lean: CROUCH_LEAN,
        swing: 0.15,
    };
}

/// Bring the arms in from the A-pose rest and hang them at the sides,
/// swinging. Used by every posture, so a body does not change shape between
/// idling and running.
///
/// `swing` is the fore-and-aft turn for the left and right arms and `elbow`
/// the elbow bend, in radians. Tuck before swing, since the swing axis moves
/// as the arm comes in; the tuck sign is mirrored because the shoulders are.
pub fn arms_at_sides(pose: &mut Pose, swing: [f32; 2], elbow: f32) {
    for (arm, forearm, tuck, swing) in [
        (bone::UPPER_ARM_L, bone::FOREARM_L, -ARM_TUCK, swing[0]),
        (bone::UPPER_ARM_R, bone::FOREARM_R, ARM_TUCK, swing[1]),
    ] {
        pose.set(arm, Quat::from_rotation_z(tuck) * Quat::from_rotation_x(swing));
        pose.set(forearm, Quat::from_rotation_x(elbow));
    }
}

/// Standing still, ducked: the crouched walk's posture without the stride, so
/// stopping mid-crouch settles into the same shape.
pub fn crouch_posture() -> Pose {
    let mut pose = Pose::rest();
    pose.root_offset = Vec3::NEG_Y * CROUCH_DEPTH;
    pose.set(bone::CHEST, Quat::from_rotation_x(CROUCH_LEAN));
    // The head comes back up to look ahead rather than at the floor.
    pose.set(bone::NECK, Quat::from_rotation_x(-CROUCH_LEAN * 0.6));

    // Held in front of a folded body rather than hanging past it.
    arms_at_sides(&mut pose, [0.35, 0.35], ELBOW_BEND);

    pose
}

/// Everything above the knees: the sink, the dip on each step, the arms.
///
/// No legs: those follow from [`foot_offsets`] and the solver, and knee
/// angles stated here too would disagree between builds.
pub fn gait_pose(inputs: &PoseInputs, travel: Vec2, style: GaitStyle) -> Pose {
    let GaitStyle { shape, sink, lean, swing: arm_swing } = style;
    let phase = inputs.stride.rem_euclid(1.0);
    let cycle = std::f32::consts::TAU * phase;

    let mut pose = Pose::rest();
    // Two dips per cycle, one under each step, on top of the constant sink.
    pose.root_offset = Vec3::NEG_Y * (sink + GAIT_BOB * (1.0 - (2.0 * cycle).cos()) * 0.5);

    // Lean forwards over the stride and tip towards a sideways step. A
    // crouched body is folded either way, so only an upright lean reverses
    // when walking backwards.
    let folded = lean > RUN_LEAN;
    let leaning = lean * if folded { 1.0 } else { travel.y };
    pose.set(
        bone::CHEST,
        Quat::from_rotation_z(travel.x * SIDESTEP_LEAN) * Quat::from_rotation_x(leaning),
    );
    pose.set(bone::NECK, Quat::from_rotation_x(-leaning * 0.7));

    // Arms swing opposite the leg on the same side.
    let [left, right] = foot_offsets(phase, travel, shape);
    let swing_of = |offset: FootOffset| {
        // Opposite the leg's travel along each axis, so a diagonal mixes both.
        let along = offset.ahead * travel.y.signum() + offset.across * travel.x.signum();
        -along / shape.half_stride_for(travel) * arm_swing
    };
    arms_at_sides(&mut pose, [swing_of(left), swing_of(right)], ELBOW_BEND);

    pose
}

/// The signed angle from `facing` to `travel`, counter-clockwise positive,
/// both in the body's own frame. No travel is no heading at all.
pub fn heading_between(facing: Vec2, travel: Vec2) -> f32 {
    if travel == Vec2::ZERO || facing == Vec2::ZERO {
        return 0.0;
    }
    let (facing, travel) = (facing.normalize(), travel.normalize());
    let cross = facing.x * travel.y - facing.y * travel.x;
    cross.atan2(facing.dot(travel))
}

/// Where a foot is in its own cycle, normalised: 0–½ on the ground, ½–1 in
/// the air, whatever the stance fraction.
///
/// `phase` is the foot's own phase — the stride for the left foot, the stride
/// plus a half for the right, which is how [`foot_offsets`] offsets them.
pub fn footfall(phase: f32, stance: f32) -> f32 {
    let phase = phase.rem_euclid(1.0);
    let stance = stance.clamp(1e-3, 1.0 - 1e-3);
    if phase < stance {
        0.5 * phase / stance
    } else {
        0.5 + 0.5 * (phase - stance) / (1.0 - stance)
    }
}

/// Both feet's [`footfall`]s for a body at `stride` in a gait of this stance,
/// left first.
pub fn footfalls(stride: f32, stance: f32) -> [f32; 2] {
    [footfall(stride, stance), footfall(stride + 0.5, stance)]
}

/// Which way a state's feet travel, in the body's own frame. Only for bodies
/// with no movement to measure; a moving body uses where it went, which is
/// how a diagonal is possible.
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

    /// Every class's build plus the two extreme sample builds.
    fn every_build() -> Vec<Proportions> {
        use crate::common::class::Class;
        use strum::IntoEnumIterator;

        Class::iter()
            .map(|class| class.proportions())
            .chain([Proportions::STOCKY, Proportions::LANKY])
            .collect()
    }

    /// While a foot is planted the body moves and the foot does not, stepped as
    /// the fixed step would.
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

    /// End to end through the solver and forward kinematics; catches a units
    /// mix-up between stride and leg length that the offsets alone would hide.
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
                        finish_pose(&skeleton, AnimationState::RunForward, &inputs, &root, None);
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

    /// Every step of every gait must be reachable on every build in every
    /// direction; an unreachable foot lands short and slides. A stride checked
    /// only along the axes can overreach on a diagonal.
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

    /// A walk always has a foot down, and sometimes two.
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

    /// Walk turns to run with speed alone, gradually; a jump in stance snaps
    /// every foot at once.
    #[test]
    fn the_gait_turns_into_a_run_as_the_body_speeds_up() {
        assert!(!GaitShape::for_speed(0.5).has_flight(), "a stroll left the ground");
        assert!(GaitShape::for_speed(9.0).has_flight(), "a sprint never left the ground");

        let mut previous = GaitShape::for_speed(0.0);
        let mut biggest = 0.0_f32;
        for step in 1..=180 {
            let shape = GaitShape::for_speed(step as f32 / 20.0);
            biggest = biggest.max((shape.stance - previous.stance).abs());
            // Faster never lengthens the stance.
            assert!(shape.stance <= previous.stance + 1e-6);
            previous = shape;
        }

        assert!(biggest < 0.02, "the gait jumps by {biggest} between neighbouring speeds");
    }

    /// Speed comes from ground covered, so a body that stops stops running.
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

    /// No ground covered, no steps: walking into a wall stops the legs.
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

    /// The drawn crouch must match the collision crouch; nothing in the types
    /// ties them together.
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
                    let pose = finish_pose(&skeleton, state, &inputs, &root, None);

                    let highest = skeleton
                        .posed_bones(&pose, &root)
                        .iter()
                        .map(|posed| posed.head.y.max(posed.tail.y))
                        .fold(0.0_f32, f32::max);
                    // Exactly at it: a body folded to the floor or barely folded would
                    // each pass a one-sided check.
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

    /// Sidesteps hold their feet too, both ways round: a wrong arm in
    /// [`direction_of`] would still hold the feet while stepping the wrong way.
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
                let pose = finish_pose(&skeleton, state, &inputs, &root, None);
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

    /// The body tips the way it is going.
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

    /// A diagonal steps forwards and sideways in proportion, rather than running
    /// straight while sliding sideways.
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

    /// The no-slide cancellation is per axis; a diagonal catches one axis being
    /// missed.
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
            let pose = finish_pose(&skeleton, AnimationState::RunForward, &inputs, &root, None);
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

    /// A leg swinging across its partner would swing through it.
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

    /// Stepping left mirrors stepping right half a cycle along, since which foot
    /// leads is set by cycle position, not direction. The feet go across the
    /// body, not through it.
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

    /// Backwards reverses the feet, not the cycle; a reversed cycle looks like a
    /// film played backwards.
    #[test]
    fn running_backwards_reverses_the_feet_and_not_the_cycle() {
        let shape = GaitShape::for_speed(RUNNING);
        let phase = 0.3;
        let [forwards, _] = foot_offsets(phase, Vec2::Y, shape);
        let [backwards, _] = foot_offsets(phase, Vec2::NEG_Y, shape);

        assert!((forwards.ahead + backwards.ahead).abs() < 1e-6);
        assert_eq!(forwards.lift, backwards.lift);
    }

    /// The arms come in from the A-pose and stay symmetric.
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

        // The arms are half a cycle apart, so they mirror only summed over a
        // cycle.
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

