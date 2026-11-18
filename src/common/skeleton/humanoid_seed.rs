//! The humanoid's clips played by name, as poses to sample, as [`super::raptor_seed`] is the
//! raptor's: a generator, and the clip file is the source of truth once baked.
//!
//! Solved on the reference build and baked as joint rotations, which fit every humanoid build: the
//! root offset is in hip-heights, and a leg bent through the same angles drops its hip by the same
//! share of its height, so planted feet stay within a millimeter of the floor.
//!
//! Turns are [`Turn`]s about the body's axes: `yaw` positive to the left, `pitch` positive tipping
//! an upright bone back.

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;

use super::ik::{self, Chain, Reach};
use super::raptor_seed::Turn;
use super::rig::{bone, default_humanoid, Pose, Skeleton};
use super::seed::keyed;
use super::state::PoseInputs;

/// The shadow-boxing taunt's file name, and its length in seconds. The raptor's lunge is as long,
/// since a taunt's length is read off its humanoid clip.
pub const TAUNT_SHADOWBOX: &str = "taunt_shadowbox";
pub const SHADOWBOX_LENGTH: f32 = 2.2;

/// The wave's file name, and its length in seconds. The raptor's wag is as long.
pub const TAUNT_WAVE: &str = "taunt_wave";
pub const WAVE_LENGTH: f32 = 2.0;

/// 0 with the hand hanging, 1 with it up by the head: it swings up past where it settles and back,
/// and comes down after the last sweep.
const WAVE_RAISE: &[(f32, f32)] = &[(0.0, 0.0), (0.08, 0.0), (0.42, 1.08), (0.58, 1.0), (1.45, 1.0), (1.88, 0.0), (2.0, 0.0)];
/// How wide the sweeps are, 0 to 1: they open while the arm comes up and close before it comes down.
const WAVING: &[(f32, f32)] = &[(0.0, 0.0), (0.25, 0.0), (0.55, 1.0), (1.3, 1.0), (1.6, 0.0), (2.0, 0.0)];
/// Sweeps per second, and the second the first one passes the middle going out.
const WAVE_RATE: f32 = 2.6;
const WAVE_START: f32 = 0.3;

/// Where the wrist sweeps between, from between the shoulders in arm-lengths in the chest's axes:
/// in by the head and out past the shoulder. It dips at either end, as a forearm turning at the
/// elbow does.
const WAVE_IN: Vec3 = Vec3::new(0.3, 0.72, -0.1);
const WAVE_OUT: Vec3 = Vec3::new(0.68, 0.72, -0.1);
const WAVE_DIP: f32 = 0.06;
/// Arm-lengths out to the side the hand swings as it comes up, so it rises past the hip rather
/// than through it.
const WAVE_RAISE_SWING: f32 = 0.25;
/// Which way the waving elbow points, in the chest's axes: back while it hangs, out and down while
/// it waves.
const HANGING_ELBOW: Vec3 = Vec3::new(0.3, -0.2, 1.0);
const WAVE_ELBOW: Vec3 = Vec3::new(1.0, -0.3, 0.3);
/// Radians the hand bends back against the way it is sweeping, so it trails the forearm.
const WAVE_FLAP: f32 = 0.45;

/// The body leans away from the raised arm, then sways with the sweeps a little behind the hand:
/// the hips sliding out under the chest one way while the chest leans the other. Radians, and the
/// hips' slide in hip-heights.
const LEAN_AWAY: f32 = 0.07;
const CHEST_SWAY: f32 = 0.16;
const PELVIS_SWAY: f32 = 0.06;
const HIP_SLIDE: f32 = 0.035;
/// Radians the sway runs behind the hand, and the head's tilt behind that.
const SWAY_LAG: f32 = 0.5;
const HEAD_LAG: f32 = 1.1;
/// The head keeps this share of the chest's lean and tilts on its own after the sway. Radians.
const HEAD_KEEPS: f32 = 0.35;
const HEAD_TILT: f32 = 0.12;
/// The waving shoulder rises as the arm comes up. Radians.
const WAVE_SHRUG: f32 = 0.12;
/// The knees give a little as the arm comes up and at each end of a sweep, in hip-heights.
const WAVE_SETTLE: f32 = 0.025;
const WAVE_BOB: f32 = 0.015;
/// How far the hanging hand swings after the sway, in arm-lengths.
const LOOSE_SWING: f32 = 0.08;

/// The hips, in hip-heights: a dip to push off, up through the hop, and further down than the
/// stance as the landing is taken, then the same back out.
const HOP_ROOT: &[(f32, f32)] = &[
    (0.0, 0.0),
    (0.12, -0.06),
    (0.25, 0.05),
    (0.37, -0.1),
    (0.5, -0.06),
    (1.56, -0.06),
    (1.67, -0.09),
    (1.8, 0.04),
    (1.92, -0.07),
    (2.06, -0.01),
    (2.2, 0.0),
];
/// Meters each foot leaves the floor in a hop. Each lands as the hips reach their lowest.
const HOP_FEET: &[(f32, f32)] = &[
    (0.0, 0.0),
    (0.13, 0.0),
    (0.24, 0.08),
    (0.36, 0.0),
    (1.68, 0.0),
    (1.79, 0.08),
    (1.91, 0.0),
    (2.2, 0.0),
];
/// 0 standing square, 1 in the stance. Changes only while the feet are off the floor.
const STANCE: &[(f32, f32)] = &[(0.0, 0.0), (0.14, 0.0), (0.34, 1.0), (1.69, 1.0), (1.89, 0.0), (2.2, 0.0)];
/// 0 hands down, 1 up in guard: up a little ahead of the hop, down a little behind it.
const GUARD: &[(f32, f32)] = &[(0.0, 0.0), (0.06, 0.0), (0.3, 1.0), (1.62, 1.0), (1.9, 0.0), (2.2, 0.0)];
/// How far each fist is out, 0 at guard and 1 at full reach: left, right, left, left. Out in
/// 80–100 ms and back slower; the second left is pulled only halfway back before the third.
const LEFT_PUNCHES: &[(f32, f32)] = &[
    (0.0, 0.0),
    (0.52, 0.0),
    (0.6, 1.0),
    (0.63, 1.0),
    (0.75, 0.0),
    (1.04, 0.0),
    (1.12, 1.0),
    (1.14, 1.0),
    (1.21, 0.45),
    (1.28, 1.0),
    (1.32, 1.0),
    (1.48, 0.0),
    (2.2, 0.0),
];
const RIGHT_PUNCHES: &[(f32, f32)] = &[(0.0, 0.0), (0.76, 0.0), (0.86, 1.0), (0.9, 1.0), (1.04, 0.0), (2.2, 0.0)];

/// Where each foot goes in the stance, from where it rests, in leg-lengths (`+X` right, `-Z`
/// forward): left foot leading, right foot back. And how far each turns its toes right, radians.
const LEAD_FOOT: Vec3 = Vec3::new(0.03, 0.0, -0.25);
const REAR_FOOT: Vec3 = Vec3::new(0.07, 0.0, 0.21);
const LEAD_TOES: f32 = -0.35;
const REAR_TOES: f32 = -0.8;
/// Meters the rear heel is off the floor in the stance, and further on the cross as the foot
/// pivots, the toes kept down. Radians the pivot turns it.
const REAR_HEEL: f32 = 0.025;
const CROSS_HEEL: f32 = 0.045;
const CROSS_PIVOT: f32 = -0.25;

/// The stance turns the body side-on to the right, the hips more than the chest, and leans the
/// chest forward with the head kept level. Radians.
const PELVIS_BLADE: f32 = -0.4;
const CHEST_BLADE: f32 = -0.1;
const CHEST_LEAN: f32 = -0.15;
/// A left turns the chest further right, putting the shoulder behind it; the cross turns the hips
/// and then the chest back left. Radians.
const JAB_CHEST: f32 = -0.35;
const JAB_PELVIS: f32 = -0.08;
const CROSS_PELVIS: f32 = 0.35;
const CROSS_CHEST: f32 = 0.55;
/// How far the chest leans forward into each punch, radians.
const PUNCH_LEAN: f32 = -0.1;
/// The punching shoulder drives forward and rises toward the chin while the other draws back,
/// each clavicle turned about the body's axes. Radians.
const SHOULDER_IN: f32 = 0.4;
const SHOULDER_LIFT: f32 = 0.15;
const SHOULDER_BACK: f32 = 0.15;
/// The weight goes forward into each punch and down, in hip-heights.
const JAB_FORWARD: f32 = 0.04;
const CROSS_FORWARD: f32 = 0.09;
const PUNCH_DIP: f32 = 0.025;
/// Share of the chest's turn the neck and head take back, so the eyes stay on the way the body faces.
const NECK_SHARE: f32 = 0.4;

/// Where each fist sits in guard, from between the shoulders in arm-lengths, in the chest's own
/// axes (`+X` right, `-Z` forward): the left out in front, the right by the chin.
const LEAD_GUARD: Vec3 = Vec3::new(-0.22, 0.18, -0.45);
const REAR_GUARD: Vec3 = Vec3::new(0.12, 0.22, -0.28);
/// Where each wrist hangs with the hands down, from its own shoulder in arm-lengths, `+X` outward.
const HANGING: Vec3 = Vec3::new(0.15, -0.78, -0.08);
/// How far forward of the straight line between down and guard a hand swings, in arm-lengths, so
/// it passes in front of the belly rather than through it.
const RAISE_SWING: f32 = 0.2;
/// How far a punch reaches from its shoulder, in arm-lengths: just short of straight, since shoulder
/// to wrist is 0.85 on every build. Aimed straight ahead of the body at the height of the chin.
const PUNCH_REACH: f32 = 0.82;
const CHIN_HEIGHT: f32 = 0.15;
/// Meters ahead of the shoulder the punches converge on.
const PUNCH_FOCUS: f32 = 1.8;
/// Which way the elbows point, in the chest's axes, `+X` outward: down in guard, swung out as the
/// fist turns over at full reach.
/// Which way a fist points in guard, in the chest's axes: forward and up off a forearm held
/// upright, so the hand reads as closed. It straightens along the forearm as the punch goes out.
const FIST: Vec3 = Vec3::new(0.0, 0.6, -1.0);
const GUARD_ELBOW: Vec3 = Vec3::new(0.5, -1.0, 0.1);
const PUNCH_ELBOW: Vec3 = Vec3::new(1.0, -0.3, 0.0);

/// A clip played by name, solved on the reference build whatever `skeleton` is given, since that
/// is the build the shipped clip was baked on.
pub fn named(name: &str, inputs: &PoseInputs) -> Pose {
    match name {
        TAUNT_SHADOWBOX => shadowbox(inputs.seconds, default_humanoid()).0,
        TAUNT_WAVE => wave(inputs.seconds, default_humanoid()).0,
        _ => Pose::rest(),
    }
}

/// One side of the body as the shadow-box moves it.
struct Side {
    sign: f32,
    leg: Chain,
    arm: Chain,
    foot: &'static str,
    hand: &'static str,
    upper_arm: &'static str,
    stance: Vec3,
    toes: f32,
    guard: Vec3,
    punches: &'static [(f32, f32)],
}

const LEFT: Side = Side {
    sign: -1.0,
    leg: ik::LEFT_LEG,
    arm: ik::LEFT_ARM,
    foot: bone::FOOT_L,
    hand: bone::HAND_L,
    upper_arm: bone::UPPER_ARM_L,
    stance: LEAD_FOOT,
    toes: LEAD_TOES,
    guard: LEAD_GUARD,
    punches: LEFT_PUNCHES,
};

const RIGHT: Side = Side {
    sign: 1.0,
    leg: ik::RIGHT_LEG,
    arm: ik::RIGHT_ARM,
    foot: bone::FOOT_R,
    hand: bone::HAND_R,
    upper_arm: bone::UPPER_ARM_R,
    stance: REAR_FOOT,
    toes: REAR_TOES,
    guard: REAR_GUARD,
    punches: RIGHT_PUNCHES,
};

/// Hops into a boxer's stance with the hands coming up, throws a left, a right and two quick
/// lefts, each with the body turned in behind it, and hops back out. Returns each limb's reach,
/// legs then arms, so a test can see nothing was left short.
fn shadowbox(seconds: f32, skeleton: &Skeleton) -> (Pose, [Reach; 4]) {
    let at = |keys| keyed(keys, seconds);
    let (stance, guard, feet_up) = (at(STANCE), at(GUARD), at(HOP_FEET));
    let (left, right) = (at(LEFT_PUNCHES), at(RIGHT_PUNCHES));
    let anatomy = skeleton.anatomy();
    let root = Transform::IDENTITY;
    let rest = Pose::rest();
    let rest_of = |name: &str| skeleton.posed_bone(&rest, &root, skeleton.index_of(name).unwrap());

    let mut pose = Pose::rest();
    pose.root_offset = Vec3::new(
        0.0,
        at(HOP_ROOT) - PUNCH_DIP * (left + right),
        -(JAB_FORWARD * left + CROSS_FORWARD * right),
    );
    let pelvis = stance * PELVIS_BLADE + JAB_PELVIS * left + CROSS_PELVIS * right;
    let chest = stance * CHEST_BLADE + JAB_CHEST * left + CROSS_CHEST * right;
    let lean = stance * CHEST_LEAN + PUNCH_LEAN * (left + right);
    let turned = pelvis + chest;
    Turn { yaw: pelvis, ..default() }.onto(skeleton, &mut pose, bone::PELVIS);
    Turn { yaw: chest, pitch: lean, ..default() }.onto(skeleton, &mut pose, bone::CHEST);
    Turn { yaw: -turned * NECK_SHARE, pitch: -lean * NECK_SHARE, ..default() }.onto(skeleton, &mut pose, bone::NECK);
    Turn { yaw: -turned * (1.0 - NECK_SHARE), pitch: -lean * (1.0 - NECK_SHARE), ..default() }
        .onto(skeleton, &mut pose, bone::HEAD);

    // A clavicle's tip runs out to its side, so a turn to the left takes the right shoulder
    // forward and the left one back: signed by side.
    for (clavicle, sign, out, other) in [(bone::CLAVICLE_L, -1.0, left, right), (bone::CLAVICLE_R, 1.0, right, left)] {
        Turn { yaw: sign * (SHOULDER_IN * out - SHOULDER_BACK * other), roll: sign * SHOULDER_LIFT * out, ..default() }
            .onto(skeleton, &mut pose, clavicle);
    }

    let mut reaches = [Reach::Impossible; 4];
    for (index, side) in [LEFT, RIGHT].iter().enumerate() {
        let rear = side.sign > 0.0;
        let heel = match rear {
            true => stance * REAR_HEEL + CROSS_HEEL * right,
            false => 0.0,
        };
        let toes = stance * side.toes + if rear { CROSS_PIVOT * right } else { 0.0 };
        let foot = rest_of(side.foot);
        let ankle = rest_of(side.leg.lower).tail + side.stance * stance * anatomy.leg + Vec3::Y * (feet_up + heel);
        let facing = Quat::from_rotation_y(toes);
        let knee = facing * Vec3::new(0.2 * side.sign, 0.0, -1.0).normalize();
        reaches[index] = ik::solve(skeleton, &mut pose, &root, &side.leg, ankle, knee);
        // Pitched toes down by as much as the heel rose, so the toes stay where they were.
        let tipped = Quat::from_rotation_x(-(heel / skeleton.bones()[skeleton.index_of(side.foot).unwrap()].length).asin());
        ik::set_world_rotation(skeleton, &mut pose, &root, side.foot, facing * tipped * foot.rotation);
    }

    // The chest's turn from rest, as the arms see it once the body has turned.
    let chest_bone = skeleton.index_of(bone::CHEST).unwrap();
    let posed_chest = skeleton.posed_bone(&pose, &root, chest_bone);
    let torso = posed_chest.rotation * rest_of(bone::CHEST).rotation.inverse();
    let arm = anatomy.arm;
    for (index, side) in [LEFT, RIGHT].iter().enumerate() {
        let outward = Vec3::new(side.sign, 1.0, 1.0);
        let shoulder = skeleton.posed_bone(&pose, &root, skeleton.index_of(side.upper_arm).unwrap()).head;
        let hanging = shoulder + torso * (HANGING * outward) * arm;
        let raised = posed_chest.tail + torso * side.guard * arm;
        let swing = torso * Vec3::NEG_Z * RAISE_SWING * arm * (PI * guard).sin();
        let held = hanging.lerp(raised, guard) + swing;
        let ahead = Vec3::new(0.0, posed_chest.tail.y + CHIN_HEIGHT * arm, shoulder.z - PUNCH_FOCUS);
        let punched = shoulder + (ahead - shoulder).normalize() * PUNCH_REACH * arm;
        let out = keyed(side.punches, seconds);
        let wrist = held.lerp(punched, out);
        let elbow = torso * (GUARD_ELBOW.lerp(PUNCH_ELBOW, out) * outward).normalize();
        reaches[2 + index] = ik::solve(skeleton, &mut pose, &root, &side.arm, wrist, elbow);
        let forearm = skeleton.posed_bone(&pose, &root, skeleton.index_of(side.arm.lower).unwrap()).rotation;
        let along = forearm * Vec3::Y;
        let pointing = along.lerp((torso * FIST).normalize(), guard * (1.0 - out)).normalize();
        // The hand rests straight along the forearm, so its rest frame is the forearm's.
        ik::set_world_rotation(skeleton, &mut pose, &root, side.hand, Quat::from_rotation_arc(along, pointing) * forearm);
    }
    (pose, reaches)
}

/// Raises the right hand up beside the head with the body leaning away from it, sweeps it out and
/// in three times with the hips and chest swaying behind it and the head behind them, and lowers
/// it. Returns each limb's reach, legs then arms.
fn wave(seconds: f32, skeleton: &Skeleton) -> (Pose, [Reach; 4]) {
    let raise = keyed(WAVE_RAISE, seconds);
    let waving = keyed(WAVING, seconds);
    let phase = TAU * WAVE_RATE * (seconds - WAVE_START);
    let sweep = waving * phase.sin();
    let sway = waving * (phase - SWAY_LAG).sin();
    let anatomy = skeleton.anatomy();
    let root = Transform::IDENTITY;
    let rest = Pose::rest();
    let rest_of = |name: &str| skeleton.posed_bone(&rest, &root, skeleton.index_of(name).unwrap());

    let mut pose = Pose::rest();
    let settle = WAVE_SETTLE * raise.clamp(0.0, 1.0) + WAVE_BOB * sweep * sweep;
    pose.root_offset = Vec3::new(-HIP_SLIDE * sway, -settle, 0.0);
    // Rolls about the body's axes, positive leaning left: each bone's is what it adds to its parent's.
    let pelvis = -PELVIS_SWAY * sway;
    let chest = LEAN_AWAY * raise - CHEST_SWAY * sway;
    let head = HEAD_KEEPS * chest + HEAD_TILT * waving * (phase - HEAD_LAG).sin();
    Turn { roll: pelvis, ..default() }.onto(skeleton, &mut pose, bone::PELVIS);
    Turn { roll: chest - pelvis, ..default() }.onto(skeleton, &mut pose, bone::CHEST);
    Turn { roll: (head - chest) * NECK_SHARE, ..default() }.onto(skeleton, &mut pose, bone::NECK);
    Turn { roll: (head - chest) * (1.0 - NECK_SHARE), ..default() }.onto(skeleton, &mut pose, bone::HEAD);
    Turn { roll: WAVE_SHRUG * raise, ..default() }.onto(skeleton, &mut pose, bone::CLAVICLE_R);

    let mut reaches = [Reach::Impossible; 4];
    for (index, side) in [LEFT, RIGHT].iter().enumerate() {
        let foot = rest_of(side.foot);
        let ankle = rest_of(side.leg.lower).tail;
        let knee = Vec3::new(0.2 * side.sign, 0.0, -1.0).normalize();
        reaches[index] = ik::solve(skeleton, &mut pose, &root, &side.leg, ankle, knee);
        ik::set_world_rotation(skeleton, &mut pose, &root, side.foot, foot.rotation);
    }

    let chest_bone = skeleton.index_of(bone::CHEST).unwrap();
    let posed_chest = skeleton.posed_bone(&pose, &root, chest_bone);
    let torso = posed_chest.rotation * rest_of(bone::CHEST).rotation.inverse();
    let arm = anatomy.arm;
    let shoulder_of = |pose: &Pose, side: &Side| skeleton.posed_bone(pose, &root, skeleton.index_of(side.upper_arm).unwrap()).head;
    let hand_along_forearm = |pose: &mut Pose, side: &Side, bend: Quat| {
        let forearm = skeleton.posed_bone(pose, &root, skeleton.index_of(side.arm.lower).unwrap()).rotation;
        ik::set_world_rotation(skeleton, pose, &root, side.hand, bend * forearm);
    };

    // The left hand hangs under its shoulder, swinging after the sway.
    let shoulder = shoulder_of(&pose, &LEFT);
    let swing = Vec3::X * LOOSE_SWING * waving * (phase - HEAD_LAG).sin() * arm;
    let wrist = shoulder + HANGING * Vec3::new(-1.0, 1.0, 1.0) * arm + swing;
    let elbow = torso * (HANGING_ELBOW * Vec3::new(-1.0, 1.0, 1.0)).normalize();
    reaches[2] = ik::solve(skeleton, &mut pose, &root, &LEFT.arm, wrist, elbow);
    hand_along_forearm(&mut pose, &LEFT, Quat::IDENTITY);

    let shoulder = shoulder_of(&pose, &RIGHT);
    let hanging = shoulder + torso * HANGING * arm;
    let middle = (WAVE_IN + WAVE_OUT) * 0.5;
    let half = (WAVE_OUT - WAVE_IN) * 0.5;
    let up = posed_chest.tail + torso * (middle + half * sweep - Vec3::Y * WAVE_DIP * sweep * sweep) * arm;
    let out = torso * Vec3::X * WAVE_RAISE_SWING * arm * (PI * raise.clamp(0.0, 1.0)).sin();
    let wrist = hanging.lerp(up, raise) + out;
    let elbow = torso * HANGING_ELBOW.lerp(WAVE_ELBOW, raise.clamp(0.0, 1.0)).normalize();
    reaches[3] = ik::solve(skeleton, &mut pose, &root, &RIGHT.arm, wrist, elbow);
    // The sweep's speed is its phase's cosine; the fingers bend toward where the hand has been.
    let flap = WAVE_FLAP * waving * phase.cos();
    hand_along_forearm(&mut pose, &RIGHT, Quat::from_axis_angle(torso * Vec3::Z, flap));
    (pose, reaches)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A limb short of its target is pointed at it rather than bent onto it: a foot off the floor
    /// or a punch that falls short.
    #[test]
    fn every_limb_reaches_through_the_shadowbox() {
        let rig = default_humanoid();
        for step in 0..=440 {
            let seconds = SHADOWBOX_LENGTH * step as f32 / 440.0;
            let (_, reaches) = shadowbox(seconds, rig);
            assert!(reaches.iter().all(|reach| *reach == Reach::Reached), "at {seconds:.3} s: {reaches:?}");
        }
    }

    /// The feet stay on the floor except in the hops, and the fists go out in front of the face.
    #[test]
    fn the_feet_stay_down_and_the_punches_land_ahead() {
        let rig = default_humanoid();
        let root = Transform::IDENTITY;
        let posed = |pose: &Pose, name: &str| rig.posed_bone(pose, &root, rig.index_of(name).unwrap());
        let rest_ankle = posed(&Pose::rest(), bone::SHIN_L).tail.y;
        for seconds in [0.0, 0.5, 0.62, 0.88, 1.13, 1.3, 1.6, 2.2] {
            let (pose, _) = shadowbox(seconds, rig);
            let ankle = posed(&pose, bone::SHIN_L).tail.y;
            assert!((ankle - rest_ankle).abs() < 1e-3, "at {seconds} s the lead ankle is {ankle:.3} m up, not {rest_ankle:.3}");
        }
        for (seconds, forearm) in [(0.615, bone::FOREARM_L), (0.88, bone::FOREARM_R), (1.3, bone::FOREARM_L)] {
            let (pose, _) = shadowbox(seconds, rig);
            let wrist = posed(&pose, forearm).tail;
            let chest = posed(&pose, bone::CHEST).tail;
            assert!(wrist.z < chest.z - 0.4, "at {seconds} s the {forearm} is only {:.2} m ahead", chest.z - wrist.z);
            assert!(wrist.x.abs() < 0.2, "at {seconds} s the {forearm} lands {:.2} m off the middle", wrist.x);
        }
    }

    /// It starts and ends standing, so the taunt's blend in and out is from and to the idle. Knees
    /// and wrists by where they are: the solver may roll a straight limb about its own length.
    #[test]
    fn the_shadowbox_starts_and_ends_standing_square() {
        let rig = default_humanoid();
        let root = Transform::IDENTITY;
        let tail = |pose: &Pose, name: &str| rig.posed_bone(pose, &root, rig.index_of(name).unwrap()).tail;
        for seconds in [0.0, SHADOWBOX_LENGTH] {
            let (pose, _) = shadowbox(seconds, rig);
            assert!(pose.root_offset.length() < 1e-4, "at {seconds} s the hips are off by {:?}", pose.root_offset);
            for name in [bone::PELVIS, bone::CHEST] {
                let turned = pose.joint(name).angle_between(Quat::IDENTITY);
                assert!(turned < 1e-3, "at {seconds} s {name} is turned {:.1}°", turned.to_degrees());
            }
            for name in [bone::THIGH_L, bone::THIGH_R, bone::SHIN_L, bone::SHIN_R] {
                let apart = tail(&pose, name).distance(tail(&Pose::rest(), name));
                assert!(apart < 0.01, "at {seconds} s the {name} ends {apart:.3} m from where it rests");
            }
        }
    }

    #[test]
    fn every_limb_reaches_through_the_wave() {
        let rig = default_humanoid();
        for step in 0..=400 {
            let seconds = WAVE_LENGTH * step as f32 / 400.0;
            let (_, reaches) = wave(seconds, rig);
            assert!(reaches.iter().all(|reach| *reach == Reach::Reached), "at {seconds:.3} s: {reaches:?}");
        }
    }

    /// The feet stay planted while the body sways over them, and the hand is up by the head.
    #[test]
    fn the_wave_sways_over_planted_feet() {
        let rig = default_humanoid();
        let root = Transform::IDENTITY;
        let posed = |pose: &Pose, name: &str| rig.posed_bone(pose, &root, rig.index_of(name).unwrap());
        let mut leans = Vec::new();
        for step in 0..=40 {
            let seconds = WAVE_LENGTH * step as f32 / 40.0;
            let (pose, _) = wave(seconds, rig);
            for shin in [bone::SHIN_L, bone::SHIN_R] {
                let apart = posed(&pose, shin).tail.distance(posed(&Pose::rest(), shin).tail);
                assert!(apart < 1e-3, "at {seconds} s the {shin}'s ankle slid {apart:.4} m");
            }
            let chest = posed(&pose, bone::CHEST);
            leans.push((chest.rotation * Vec3::Y).x);
        }
        let (least, most) = leans.iter().fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
        assert!(least < -0.05 && most > 0.05, "the chest only leaned between {least:.3} and {most:.3}");
        let (pose, _) = wave(0.9, rig);
        let wrist = posed(&pose, bone::FOREARM_R).tail;
        let crown = posed(&pose, bone::HEAD).head;
        assert!(wrist.y > crown.y - 0.15, "the waving wrist is {:.2} m below the neck", crown.y - wrist.y);
    }

    /// It starts and ends standing and still, so the taunt blends in from the idle rather than
    /// snapping the arm up.
    #[test]
    fn the_wave_starts_and_ends_standing_still() {
        let rig = default_humanoid();
        let root = Transform::IDENTITY;
        let tail = |pose: &Pose, name: &str| rig.posed_bone(pose, &root, rig.index_of(name).unwrap()).tail;
        for seconds in [0.0, WAVE_LENGTH] {
            let (pose, _) = wave(seconds, rig);
            let (soon, _) = wave(if seconds == 0.0 { 0.05 } else { WAVE_LENGTH - 0.05 }, rig);
            assert!(pose.root_offset.length() < 1e-4, "at {seconds} s the hips are off by {:?}", pose.root_offset);
            for name in [bone::PELVIS, bone::CHEST] {
                let turned = pose.joint(name).angle_between(Quat::IDENTITY);
                assert!(turned < 1e-3, "at {seconds} s {name} is turned {:.1}°", turned.to_degrees());
            }
            let moved = tail(&pose, bone::FOREARM_R).distance(tail(&soon, bone::FOREARM_R));
            assert!(moved < 0.01, "the wrist moves {moved:.3} m in the 50 ms at {seconds} s");
        }
    }
}
