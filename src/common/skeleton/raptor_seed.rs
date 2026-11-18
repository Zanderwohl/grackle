//! The raptor's body clips, as poses to sample: a generator, as `new_map_template` is for
//! blueprints, and with its rig file the only place that names the raptor's bones.
//!
//! Turns are stated about the body's axes and carried into each bone's rest frame
//! ([`Turn::onto`]), so one sign means one thing on every bone: `yaw` positive to the left,
//! `pitch` positive lifting whatever lies ahead of the bone's head, `roll` positive lifting the
//! right side. A tail points backwards, so it is turned by `swing` (positive, tip to the right)
//! and `lift` (positive, tip up).
//!
//! The motions are the humanoid's ([`super::gait::gait_pose`]) where a raptor has the part,
//! and the tail counterbalances the legs.

use std::f32::consts::TAU;

use bevy::prelude::*;

use super::gait::GaitShape;
use super::ik;
use super::rig::{Pose, Skeleton};
use super::state::{AnimationState, PoseInputs, AIRBORNE_PERIOD};

const PELVIS: &str = "pelvis";
const SPINE: &str = "spine";
const NECK: &str = "neck";
const HEAD: &str = "head";
const TAIL: [&str; 3] = ["tail.1", "tail.2", "tail.3"];
/// Each arm's upper bone and forearm.
const ARMS: [(&str, &str); 2] = [("upper_arm.l", "forearm.l"), ("upper_arm.r", "forearm.r")];
/// Each leg's limb, its foot, and where in a pedal it starts.
const LEGS: [(&str, &str, f32); 2] = [("left_leg", "foot.l", 0.0), ("right_leg", "foot.r", 0.5)];

/// Seconds per breath standing or crouched; the tail sways twice in one.
pub const IDLE_LENGTH: f32 = 4.0;

/// How far the hips rise and fall over a step, peak to peak, in hip-heights, walking and running.
const WALK_BOB: f32 = 0.05;
const RUN_BOB: f32 = 0.08;
/// Share of a step at which a walk's hips are lowest: just after the foot lands.
const WALK_LOWEST: f32 = 0.1;
/// How far the hips sit down to step, in hip-heights.
const GAIT_SINK: f32 = 0.04;
/// Radians.
const WALK_PELVIS_TURN: f32 = 0.08;
const RUN_PELVIS_TURN: f32 = 0.12;
const WALK_PELVIS_DROP: f32 = 0.06;
const RUN_PELVIS_DROP: f32 = 0.04;
/// Hip-heights.
const WALK_SWAY: f32 = 0.03;
const RUN_SWAY: f32 = 0.012;
/// How far the body tips forward into a walk and a run, radians, and the share the pelvis takes.
const WALK_LEAN: f32 = 0.05;
const RUN_LEAN: f32 = 0.18;
const PELVIS_LEAN: f32 = 0.6;
/// How far further it tips as each landing is taken, radians.
const WALK_LEAN_PULSE: f32 = 0.03;
const RUN_LEAN_PULSE: f32 = 0.05;
/// Share of the pelvis's turn the spine turns back past straight.
const COUNTER_TURN: f32 = 0.5;
/// Share of the pelvis's drop the spine takes back.
const LEVELING: f32 = 0.7;

/// How far each tail bone swings against the legs, radians, root first. Growing down the tail,
/// and each a little behind the one before, so it whips rather than swinging as a rod.
const TAIL_SWING: [f32; 3] = [0.1, 0.14, 0.2];
/// How far each tail bone lags the one before, in shares of a cycle.
const TAIL_LAG: f32 = 0.06;
/// How far the tail droops as each landing is taken, radians, root first.
const TAIL_DROOP: [f32; 3] = [0.03, 0.05, 0.07];
/// How far the tail is carried up while moving, radians, root first: level against the lean.
const TAIL_CARRIED: [f32; 3] = [0.05, 0.05, 0.05];

/// Standing: a slow breath, and the tail swaying behind.
const BREATH_SINK: f32 = 0.025;
const BREATH_LEAN: f32 = 0.04;
const IDLE_TAIL_SWING: [f32; 3] = [0.07, 0.12, 0.18];
const IDLE_TAIL_LIFT: [f32; 3] = [0.15, 0.1, 0.05];

/// Crouched: the front raised and the neck reaching forward and down to stalk, the head kept
/// level, the tail low. How far the hips drop is `duck_under`'s, to the rig's `crouched_crown`.
const CROUCH_SPINE: f32 = 0.45;
const CROUCH_NECK: f32 = 0.3;
/// How far the neck reaches forward and down from the shoulders, radians: further at a run, and
/// much further crouched, stalking. The head turns back by as much, so it still looks ahead.
const RUN_NECK_DROP: f32 = 15.0 * std::f32::consts::PI / 180.0;
const CROUCH_NECK_DROP: f32 = 85.0 * std::f32::consts::PI / 180.0;
const CROUCH_TAIL_LIFT: [f32; 3] = [-0.15, 0.0, 0.05];

/// The taunt: its file name under the raptor's clips, and its length in seconds, the humanoid
/// wave's.
pub const TAUNT_WAG: &str = "taunt_wag";
pub const TAUNT_LENGTH: f32 = super::humanoid_seed::WAVE_LENGTH;
/// Seconds the tail takes to curl up, and to come back down at the end.
const TAUNT_RAISE: f32 = 0.25;
/// How far each tail bone curls up, radians, root first: together past straight up at the tip.
const TAUNT_TAIL_LIFT: [f32; 3] = [0.75, 0.6, 0.45];
/// Wags while it is up, how far each bone swings, radians, and how far each lags the one before.
const TAUNT_WAGS: f32 = 4.0;
const TAUNT_TAIL_SWING: [f32; 3] = [0.1, 0.3, 0.4];
const TAUNT_TAIL_LAG: f32 = 0.6;
/// How far the front and the head lift to look up, radians.
const TAUNT_SPINE: f32 = 0.1;
const TAUNT_NECK: f32 = 0.25;
const TAUNT_HEAD: f32 = 0.2;

/// The lunge: its file name under the raptor's clips, and its length in seconds, the humanoid
/// shadow-box's.
pub const TAUNT_LUNGE: &str = "taunt_lunge";
pub const LUNGE_LENGTH: f32 = super::humanoid_seed::SHADOWBOX_LENGTH;
/// The hips, in hip-heights: down and back into the crouch, breathing in it, drawn further back
/// and down, thrown forward and up over the feet, sinking as the weight lands, and back.
const LUNGE_ROOT_Y: &[(f32, f32)] = &[(0.0, 0.0), (0.36, -0.23), (0.46, -0.21), (0.62, -0.225), (0.84, -0.215), (0.98, -0.27), (1.14, -0.08), (1.26, -0.11), (1.45, -0.13), (1.85, -0.02), (2.2, 0.0)];
/// Backwards positive.
const LUNGE_ROOT_Z: &[(f32, f32)] = &[(0.0, 0.0), (0.36, 0.06), (0.84, 0.06), (0.98, 0.12), (1.14, -0.27), (1.26, -0.25), (1.45, -0.23), (1.9, -0.01), (2.2, 0.0)];
/// Radians, positive lifting the front: the shoulders drop to bring the body level in the crouch,
/// and the hips tip forward into the lunge.
const LUNGE_PELVIS: &[(f32, f32)] = &[(0.0, 0.0), (0.36, -0.05), (0.84, -0.05), (0.98, -0.02), (1.14, -0.2), (1.45, -0.15), (1.9, -0.01), (2.2, 0.0)];
const LUNGE_SPINE: &[(f32, f32)] = &[(0.0, 0.0), (0.36, -0.2), (0.84, -0.2), (0.98, -0.12), (1.12, -0.18), (1.3, -0.18), (1.9, 0.0), (2.2, 0.0)];
/// The neck drawn up and back to coil, further as it draws back, and thrown out level to strike.
const LUNGE_NECK: &[(f32, f32)] = &[(0.0, 0.0), (0.36, 0.35), (0.84, 0.35), (0.98, 0.5), (1.12, -0.55), (1.26, -0.48), (1.5, -0.42), (1.95, 0.0), (2.2, 0.0)];
/// The head tipped down to watch from the coil, thrown back to open the jaws as it strikes,
/// snapped down, and again.
const LUNGE_HEAD: &[(f32, f32)] = &[(0.0, 0.0), (0.36, -0.5), (0.84, -0.5), (0.98, -0.55), (1.08, 0.6), (1.14, -0.2), (1.22, 0.0), (1.3, 0.35), (1.35, -0.15), (1.48, 0.05), (1.95, 0.0), (2.2, 0.0)];
/// A shake of the head once it has bitten, radians to the left, the neck taking `LUNGE_NECK_SHAKE`
/// of it.
const LUNGE_SHAKE: &[(f32, f32)] = &[(0.0, 0.0), (1.48, 0.0), (1.54, 0.14), (1.62, -0.12), (1.7, 0.06), (1.8, 0.0), (2.2, 0.0)];
const LUNGE_NECK_SHAKE: f32 = 0.4;
/// The claws: each arm swung forward and each forearm up to reach, radians.
const LUNGE_ARM: &[(f32, f32)] = &[(0.0, 0.0), (0.4, 0.2), (0.84, 0.2), (0.98, 0.05), (1.12, 1.0), (1.2, 1.05), (1.45, 0.7), (1.9, 0.0), (2.2, 0.0)];
const LUNGE_FOREARM: &[(f32, f32)] = &[(0.0, 0.0), (0.4, 0.3), (0.84, 0.3), (1.0, 0.1), (1.1, 0.2), (1.16, 0.7), (1.4, 0.4), (1.9, 0.0), (2.2, 0.0)];
/// The tail lifted over the idle's carriage to balance the body: held level behind the crouch,
/// dropped as the body draws back, flung up as the front goes down. Each bone lags the one before by `LUNGE_TAIL_LAG`
/// seconds, so it whips.
const LUNGE_TAIL: &[(f32, f32)] = &[(0.0, 0.0), (0.36, -0.05), (0.84, -0.05), (0.98, -0.1), (1.16, 0.25), (1.35, 0.18), (1.8, 0.02), (2.2, 0.0)];
const LUNGE_TAIL_LAG: f32 = 0.05;
/// A lash from side to side while it waits, and once more after the bite: how strongly, how far
/// each bone swings, radians, how often, and how far each lags the one before, radians of a lash.
const LUNGE_LASH: &[(f32, f32)] = &[(0.0, 0.0), (0.3, 0.0), (0.45, 1.0), (0.84, 1.0), (0.98, 0.0), (1.4, 0.0), (1.55, 0.5), (1.75, 0.0), (2.2, 0.0)];
const LUNGE_LASH_SWING: [f32; 3] = [0.06, 0.12, 0.18];
const LUNGE_LASH_HZ: f32 = 2.0;
const LUNGE_LASH_LAG: f32 = 0.8;

/// A pedal in the air: the ankle drawn round a loop above where it rests, meters on the reference
/// rig, raised by `lift`, `reach` either way along the body and `rise` up and down.
#[derive(Clone, Copy)]
struct Pedal {
    lift: f32,
    reach: f32,
    rise: f32,
}

const AIRBORNE_PEDAL: Pedal = Pedal { lift: 0.22, reach: 0.12, rise: 0.07 };
/// Tucked for the short box; `tuck_into_the_box` takes up whatever is left.
const CROUCH_AIRBORNE_PEDAL: Pedal = Pedal { lift: 0.36, reach: 0.07, rise: 0.04 };
/// How far the toes hang in the air, radians.
const TOES_HANG: f32 = 0.5;
const AIRBORNE_SPINE: f32 = 0.1;
const AIRBORNE_TAIL_LIFT: [f32; 3] = [0.2, 0.1, 0.05];
const AIRBORNE_TAIL_SWING: [f32; 3] = [0.06, 0.09, 0.13];

/// A turn about the body's axes, radians; see the module docs for the signs.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Turn {
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) roll: f32,
}

impl Turn {
    fn quat(self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch) * Quat::from_rotation_z(self.roll)
    }

    fn add(self, other: Turn) -> Turn {
        Turn { yaw: self.yaw + other.yaw, pitch: self.pitch + other.pitch, roll: self.roll + other.roll }
    }

    /// A tail bone's swing and lift as a turn: it points backwards, so a yaw to the left swings
    /// its tip right and a pitch up lowers it.
    fn of_tail(swing: f32, lift: f32) -> Turn {
        Turn { yaw: swing, pitch: -lift, roll: 0.0 }
    }

    /// Set on `bone` as a joint: the turn about the body's axes, carried into the bone's rest
    /// frame, so it turns the bone the same way whatever its parents are doing.
    pub(super) fn onto(self, skeleton: &Skeleton, pose: &mut Pose, bone: &'static str) {
        let Some(index) = skeleton.index_of(bone) else { return };
        let frame = skeleton.posed_bone(&Pose::rest(), &Transform::IDENTITY, index).rotation;
        pose.set(bone, frame.inverse() * self.quat() * frame);
    }
}

/// Everything above the legs, as turns and a root offset.
#[derive(Clone, Copy, Debug, Default)]
struct Posture {
    root: Vec3,
    pelvis: Turn,
    spine: Turn,
    neck: Turn,
    head: Turn,
    tail: [Turn; 3],
}

impl Posture {
    fn add(self, other: Posture) -> Posture {
        Posture {
            root: self.root + other.root,
            pelvis: self.pelvis.add(other.pelvis),
            spine: self.spine.add(other.spine),
            neck: self.neck.add(other.neck),
            head: self.head.add(other.head),
            tail: [0, 1, 2].map(|at| self.tail[at].add(other.tail[at])),
        }
    }

    fn pose(self, skeleton: &Skeleton) -> Pose {
        let mut pose = Pose::rest();
        pose.root_offset = self.root;
        self.pelvis.onto(skeleton, &mut pose, PELVIS);
        self.spine.onto(skeleton, &mut pose, SPINE);
        self.neck.onto(skeleton, &mut pose, NECK);
        self.head.onto(skeleton, &mut pose, HEAD);
        for (bone, turn) in TAIL.into_iter().zip(self.tail) {
            turn.onto(skeleton, &mut pose, bone);
        }
        pose
    }
}

/// The pose a raptor's `state` is in, before corrections. Rest for a state it has no clip for.
pub fn pose(state: AnimationState, inputs: &PoseInputs, skeleton: &Skeleton) -> Pose {
    match state {
        AnimationState::Idle => idle(inputs.seconds).pose(skeleton),
        AnimationState::Crouch => idle(inputs.seconds).add(crouched()).pose(skeleton),
        AnimationState::Airborne => airborne(inputs.seconds, Posture::default(), AIRBORNE_PEDAL, skeleton),
        AnimationState::CrouchAirborne => airborne(inputs.seconds, crouched(), CROUCH_AIRBORNE_PEDAL, skeleton),
        state if state.uses_gait() => gait(state, inputs).pose(skeleton),
        _ => Pose::rest(),
    }
}

/// A clip played by name.
pub fn named(name: &str, inputs: &PoseInputs, skeleton: &Skeleton) -> Pose {
    match name {
        TAUNT_WAG => taunt_wag(inputs.seconds).pose(skeleton),
        TAUNT_LUNGE => taunt_lunge(inputs.seconds, skeleton).0,
        _ => Pose::rest(),
    }
}

/// The raptor's wave: the tail curls up to point at the sky and waggles, the head lifts to
/// look up, and it all settles back by the end.
fn taunt_wag(seconds: f32) -> Posture {
    let smooth = |t: f32| {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let up = smooth(seconds / TAUNT_RAISE) * (1.0 - smooth((seconds - (TAUNT_LENGTH - TAUNT_RAISE)) / TAUNT_RAISE));
    let wagging = TAU * TAUNT_WAGS * (seconds - TAUNT_RAISE * 0.5) / (TAUNT_LENGTH - TAUNT_RAISE);
    Posture {
        spine: Turn { pitch: TAUNT_SPINE * up, ..default() },
        neck: Turn { pitch: TAUNT_NECK * up, ..default() },
        head: Turn { pitch: TAUNT_HEAD * up, ..default() },
        tail: [0, 1, 2].map(|at| {
            let swing = TAUNT_TAIL_SWING[at] * up * (wagging - TAUNT_TAIL_LAG * at as f32).sin();
            Turn::of_tail(swing, TAUNT_TAIL_LIFT[at] * up)
        }),
        ..default()
    }
}

/// Crouches into a ready stance with the neck coiled and the tail lashing, draws back, and lunges
/// over its planted feet with the claws out, snapping twice and shaking its head before it
/// settles. Returns each leg's reach so a test can see the feet stayed down.
fn taunt_lunge(seconds: f32, skeleton: &Skeleton) -> (Pose, [ik::Reach; 2]) {
    use super::seed::keyed;
    let at = |keys| keyed(keys, seconds);
    let shake = at(LUNGE_SHAKE);
    let lash = at(LUNGE_LASH);
    let posture = Posture {
        root: Vec3::new(0.0, at(LUNGE_ROOT_Y), at(LUNGE_ROOT_Z)),
        pelvis: Turn { pitch: at(LUNGE_PELVIS), ..default() },
        spine: Turn { pitch: at(LUNGE_SPINE), ..default() },
        neck: Turn { pitch: at(LUNGE_NECK), yaw: shake * LUNGE_NECK_SHAKE, ..default() },
        head: Turn { pitch: at(LUNGE_HEAD), yaw: shake * (1.0 - LUNGE_NECK_SHAKE), ..default() },
        tail: [0, 1, 2].map(|bone| {
            let behind = bone as f32;
            let lift = keyed(LUNGE_TAIL, seconds - LUNGE_TAIL_LAG * behind);
            let swing = lash * LUNGE_LASH_SWING[bone] * (TAU * LUNGE_LASH_HZ * seconds - LUNGE_LASH_LAG * behind).sin();
            Turn::of_tail(swing, IDLE_TAIL_LIFT[bone] + lift)
        }),
    };
    let mut pose = posture.pose(skeleton);
    for (upper, forearm) in ARMS {
        Turn { pitch: at(LUNGE_ARM), ..default() }.onto(skeleton, &mut pose, upper);
        Turn { pitch: at(LUNGE_FOREARM), ..default() }.onto(skeleton, &mut pose, forearm);
    }
    let reaches = plant_legs(skeleton, &mut pose);
    (pose, reaches)
}

/// Each foot back where it rests and flat on the floor, under however the body has moved.
fn plant_legs(skeleton: &Skeleton, pose: &mut Pose) -> [ik::Reach; 2] {
    let root = Transform::IDENTITY;
    let rest = Pose::rest();
    LEGS.map(|(limb, foot, _)| {
        let Some(limb) = skeleton.anatomy().limb(limb) else { return ik::Reach::Impossible };
        let Some(lower) = skeleton.index_of(limb.chain.lower) else { return ik::Reach::Impossible };
        let ankle = skeleton.posed_bone(&rest, &root, lower).tail;
        let reach = ik::solve(skeleton, pose, &root, &limb.chain, ankle, limb.pole);
        if let Some(index) = skeleton.index_of(foot) {
            let flat = skeleton.posed_bone(&rest, &root, index).rotation;
            ik::set_world_rotation(skeleton, pose, &root, foot, flat);
        }
        reach
    })
}

/// A slow breath: the hips settle and the body tips into it, the neck taking the tip back, and
/// the tail sways twice.
fn idle(seconds: f32) -> Posture {
    let breath = TAU * seconds / IDLE_LENGTH;
    let settle = (1.0 - breath.cos()) * 0.5;
    let lean = settle * BREATH_LEAN;
    Posture {
        root: Vec3::NEG_Y * settle * BREATH_SINK,
        spine: Turn { pitch: -lean, ..default() },
        neck: Turn { pitch: lean, ..default() },
        tail: [0, 1, 2].map(|at| {
            let sway = (2.0 * breath - TAU * TAIL_LAG * 2.0 * at as f32).sin();
            Turn::of_tail(IDLE_TAIL_SWING[at] * sway, IDLE_TAIL_LIFT[at] - settle * TAIL_DROOP[at])
        }),
        ..default()
    }
}

/// What crouching adds.
fn crouched() -> Posture {
    Posture {
        spine: Turn { pitch: CROUCH_SPINE, ..default() },
        neck: Turn { pitch: CROUCH_NECK - CROUCH_NECK_DROP, ..default() },
        head: Turn { pitch: -(CROUCH_SPINE + CROUCH_NECK - CROUCH_NECK_DROP), ..default() },
        tail: CROUCH_TAIL_LIFT.map(|lift| Turn::of_tail(0.0, lift)),
        ..default()
    }
}

/// Walking or running, upright or crouched: the humanoid's hips, the spine turning against them,
/// the neck holding the head steady, and the tail swinging against the legs.
///
/// Stride phase 0 is the left foot landing, ½ the right.
fn gait(state: AnimationState, inputs: &PoseInputs) -> Posture {
    let ducked = state.ducks();
    let shape = if ducked { GaitShape::CROUCHED } else { GaitShape::for_speed(inputs.speed) };
    let heft = if ducked { 0.5 } else { 1.0 };
    let run = shape.run_share();
    let travel = state.travel(inputs);
    let ahead = if travel.y < -1e-3 { -1.0 } else { 1.0 };
    let phase = inputs.stride.rem_euclid(1.0);
    let cycle = TAU * phase;
    let step = (2.0 * phase).rem_euclid(1.0);
    let lowest = WALK_LOWEST.lerp(shape.stance, run);
    let bearing = TAU * (phase - shape.stance * 0.5);
    let landing = (TAU * (step - lowest)).cos();

    let bob = WALK_BOB.lerp(RUN_BOB, run) * 0.5 * heft;
    let rise = bob * ((TAU * lowest).cos() - landing);
    let sway = -WALK_SWAY.lerp(RUN_SWAY, run) * heft * bearing.cos();

    // The forward leg's hip goes forward and the swinging leg's hip drops.
    let turn = -WALK_PELVIS_TURN.lerp(RUN_PELVIS_TURN, run) * heft * ahead * cycle.cos();
    let drop = -WALK_PELVIS_DROP.lerp(RUN_PELVIS_DROP, run) * heft * bearing.cos();
    let lean = WALK_LEAN.lerp(RUN_LEAN, run) * travel.y + WALK_LEAN_PULSE.lerp(RUN_LEAN_PULSE, run) * heft * landing;
    let pelvis = Turn { yaw: turn, pitch: -lean * PELVIS_LEAN, roll: drop };
    let spine = Turn { yaw: -turn * (1.0 + COUNTER_TURN), pitch: -lean * (1.0 - PELVIS_LEAN), roll: -drop * LEVELING };
    let reaching = RUN_NECK_DROP * run;
    let neck = Turn { yaw: -(pelvis.yaw + spine.yaw), pitch: lean - reaching, roll: -(pelvis.roll + spine.roll) };
    let head = Turn { pitch: reaching, ..default() };

    // A leg swinging forward turns the body after it; the tail swings the other way across the
    // back, fastest as the left leg is halfway through its swing.
    let left_swinging = (1.0 + shape.stance) * 0.5;
    let tail = [0, 1, 2].map(|at| {
        let behind = TAIL_LAG * at as f32;
        let swing = TAIL_SWING[at] * heft * ahead * (TAU * (phase - left_swinging - behind)).sin();
        let droop = TAIL_DROOP[at] * heft * (TAU * (step - lowest - behind)).cos();
        // The pelvis's lean lifts the tail with it; carried level against that.
        let carried = TAIL_CARRIED[at] * run + if at == 0 { -lean * PELVIS_LEAN } else { 0.0 };
        Turn::of_tail(swing, carried - droop)
    });

    let mut posture = Posture { root: Vec3::new(sway, -GAIT_SINK + rise, 0.0), pelvis, spine, neck, head, tail };
    if ducked {
        posture = posture.add(crouched());
    }
    posture
}

/// Off the ground: each leg pedals a loop, half a turn behind the other, the toes hanging, and
/// the tail lifted and swinging against them.
fn airborne(seconds: f32, beneath: Posture, pedal: Pedal, skeleton: &Skeleton) -> Pose {
    let turn = TAU * seconds / AIRBORNE_PERIOD;
    let mut posture = beneath.add(Posture {
        spine: Turn { pitch: AIRBORNE_SPINE, ..default() },
        neck: Turn { pitch: -AIRBORNE_SPINE, ..default() },
        tail: [0, 1, 2].map(|at| {
            let swing = AIRBORNE_TAIL_SWING[at] * (turn - TAU * TAIL_LAG * at as f32).sin();
            Turn::of_tail(swing, AIRBORNE_TAIL_LIFT[at])
        }),
        ..default()
    });
    posture.root = beneath.root;
    let mut pose = posture.pose(skeleton);
    pedal_legs(skeleton, &mut pose, turn, pedal);
    pose
}

/// Where each ankle is on its pedal at `turn`, in the body's frame on the reference rig.
fn pedal_targets(skeleton: &Skeleton, turn: f32, pedal: Pedal) -> Vec<(&'static str, &'static str, Vec3)> {
    let anatomy = skeleton.anatomy();
    LEGS.iter()
        .filter_map(|(limb, foot, behind)| {
            let chain = anatomy.limb(limb)?.chain;
            let lower = skeleton.index_of(chain.lower)?;
            let resting = skeleton.posed_bone(&Pose::rest(), &Transform::IDENTITY, lower).tail;
            let at = turn + TAU * behind;
            // Forward along the top of the loop and back along the bottom.
            let offset = Vec3::new(0.0, pedal.lift + pedal.rise * at.sin(), pedal.reach * at.cos());
            Some((*limb, *foot, resting + offset))
        })
        .collect()
}

fn pedal_legs(skeleton: &Skeleton, pose: &mut Pose, turn: f32, pedal: Pedal) {
    let root = Transform::IDENTITY;
    for (limb, foot, target) in pedal_targets(skeleton, turn, pedal) {
        let Some(limb) = skeleton.anatomy().limb(limb) else { continue };
        ik::solve(skeleton, pose, &root, &limb.chain, target, limb.pole);
        let Some(index) = skeleton.index_of(foot) else { continue };
        let resting = skeleton.posed_bone(&Pose::rest(), &root, index).rotation;
        ik::set_world_rotation(skeleton, pose, &root, foot, Quat::from_rotation_x(-TOES_HANG) * resting);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::skeleton::ik::Reach;

    fn raptor() -> Skeleton {
        crate::common::class::ClassBuilds::shipped().rig_for(crate::common::class::Class::Raptor)
    }

    /// Every bone this names is the rig's: a renamed bone would be a track that does nothing.
    #[test]
    fn every_bone_named_here_is_the_raptors() {
        let rig = raptor();
        let arms = ARMS.iter().flat_map(|(upper, forearm)| [*upper, *forearm]);
        for name in [PELVIS, SPINE, NECK, HEAD].into_iter().chain(TAIL).chain(LEGS.iter().map(|(_, foot, _)| *foot)).chain(arms) {
            assert!(rig.index_of(name).is_some(), "the raptor has no {name}");
        }
        for (limb, _, _) in LEGS {
            assert!(rig.anatomy().limb(limb).is_some(), "the raptor has no {limb}");
        }
    }

    /// A pedal the leg cannot reach straightens it toward the loop and the foot stops drawing one.
    #[test]
    fn every_pedal_is_within_the_legs_reach() {
        let rig = raptor();
        for pedal in [AIRBORNE_PEDAL, CROUCH_AIRBORNE_PEDAL] {
            for step in 0..32 {
                let turn = TAU * step as f32 / 32.0;
                for (limb, _, target) in pedal_targets(&rig, turn, pedal) {
                    let chain = rig.anatomy().limb(limb).unwrap();
                    let mut pose = Pose::rest();
                    let reach = ik::solve(&rig, &mut pose, &Transform::IDENTITY, &chain.chain, target, chain.pole);
                    assert_eq!(reach, Reach::Reached, "{limb} at {turn:.2} cannot reach {target}");
                }
            }
        }
    }

    /// The feet stay where they rest through the whole lunge: a leg short of its ankle is
    /// pointed at it and the foot slides or lifts.
    #[test]
    fn the_lunge_keeps_both_feet_planted() {
        let rig = raptor();
        let root = Transform::IDENTITY;
        let ankle = |pose: &Pose, limb: &str| {
            let lower = rig.anatomy().limb(limb).unwrap().chain.lower;
            rig.posed_bone(pose, &root, rig.index_of(lower).unwrap()).tail
        };
        for step in 0..=440 {
            let seconds = LUNGE_LENGTH * step as f32 / 440.0;
            let (pose, reaches) = taunt_lunge(seconds, &rig);
            assert!(reaches.iter().all(|reach| *reach == Reach::Reached), "at {seconds:.3} s: {reaches:?}");
            for (limb, _, _) in LEGS {
                let apart = ankle(&pose, limb).distance(ankle(&Pose::rest(), limb));
                assert!(apart < 1e-3, "at {seconds:.3} s the {limb} ankle is {apart:.3} m from where it rests");
            }
        }
    }

    /// The head goes out ahead of where it rests at the bite, which is what makes it a lunge.
    #[test]
    fn the_lunge_throws_the_head_forward() {
        let rig = raptor();
        let head = |seconds: f32| {
            let (pose, _) = taunt_lunge(seconds, &rig);
            rig.posed_bone(&pose, &Transform::IDENTITY, rig.index_of(HEAD).unwrap()).tail
        };
        let reached = head(0.0).z - head(1.14).z;
        assert!(reached > 0.4, "the head reaches only {reached:.2} m forward");
    }

    /// The tail counterbalances the legs: it swings, and to the right while the left leg swings
    /// forward.
    #[test]
    fn the_tail_swings_against_the_legs() {
        let rig = raptor();
        let tip = |stride: f32| {
            let inputs = PoseInputs { stride, speed: super::super::gait::RUN_SPEED, travel: Vec2::Y, ..default() };
            let pose = pose(AnimationState::RunForward, &inputs, &rig);
            rig.posed_bone(&pose, &Transform::IDENTITY, rig.index_of("tail.3").unwrap()).tail.x
        };
        let shape = GaitShape::for_speed(super::super::gait::RUN_SPEED);
        let left_swinging = (1.0 + shape.stance) * 0.5;
        let d = 0.01;
        assert!(tip(left_swinging + d) > tip(left_swinging - d), "the tail moves left as the left leg swings forward");
        let reach = (0..32).map(|step| tip(step as f32 / 32.0)).fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
        assert!(reach.1 - reach.0 > 0.08, "the tail tip swings only {:.3} m", reach.1 - reach.0);
    }

    /// The raptor's clips, baked as the shipped ones are, as a match holds them.
    fn library() -> crate::common::skeleton::state::ClipLibrary {
        let mut library = crate::common::skeleton::state::ClipLibrary::default();
        for seed in super::super::seed::SEEDS.iter().filter(|seed| seed.rig == "raptor" && seed.name.is_none()) {
            library.insert_for("raptor", seed.state, super::super::seed::bake(*seed));
        }
        library
    }

    fn extent(rig: &Skeleton, pose: &Pose) -> (f32, f32) {
        let (mut low, mut high) = (f32::MAX, f32::MIN);
        rig.each_posed_bone(pose, &Transform::IDENTITY, |posed| {
            low = low.min(posed.head.y.min(posed.tail.y));
            high = high.max(posed.head.y.max(posed.tail.y));
        });
        (low, high)
    }

    /// Crouched, the raptor fits the short box with its feet on the floor and its hips well
    /// down; crouched in the air it stays inside the box's top and bottom.
    #[test]
    fn a_crouched_raptor_fits_its_box_and_sits_down_into_it() {
        use crate::common::class::CROUCH_HEIGHT;
        use crate::common::skeleton::ik::finish_pose;
        let rig = raptor();
        let library = library();
        let root = Transform::IDENTITY;
        let hips = |pose: &Pose| rig.posed_bone(pose, &root, rig.index_of(PELVIS).unwrap()).head.y;
        let standing = finish_pose(&rig, AnimationState::Idle, &PoseInputs::default(), &root, Some(&library));
        for (state, clearance) in [
            (AnimationState::Crouch, None),
            (AnimationState::CrouchWalk, None),
            (AnimationState::CrouchAirborne, Some(0.05)),
        ] {
            for step in 0..16 {
                let at = step as f32 / 16.0;
                let inputs = PoseInputs { seconds: at * IDLE_LENGTH, stride: at, speed: 1.0, travel: Vec2::Y, ..default() };
                let pose = finish_pose(&rig, state, &inputs, &root, Some(&library));
                let (low, high) = extent(&rig, &pose);
                assert!(high <= CROUCH_HEIGHT + 1e-3, "{state:?} at {at:.2} reaches {high:.3} m");
                match clearance {
                    Some(clearance) => assert!(low >= clearance, "{state:?} at {at:.2} hangs to {low:.3} m"),
                    None => assert!(low.abs() < 0.02, "{state:?} at {at:.2}: the lowest point is at {low:.3} m, off the floor"),
                }
                if state == AnimationState::Crouch {
                    let dropped = hips(&standing) - hips(&pose);
                    assert!(dropped > 0.15, "crouching drops the hips only {dropped:.3} m");
                }
            }
        }
    }
}
