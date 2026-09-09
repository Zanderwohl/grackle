//! Putting the end of a limb where it has to be, rather than where the joint
//! angles happened to leave it.
//!
//! Rotations retarget across builds for free; contacts do not. Two bodies with
//! different leg lengths given the same joint angles put their feet in
//! different places, and one of them is sliding along the floor. So a foot's
//! position is stated as a *target* and the angles are solved for, which is
//! the only formulation that means the same thing on every silhouette.
//!
//! That applies to authored motion as much as procedural: a taunt that plants
//! a foot and pivots on it needs the same pinning a run cycle does, from the
//! same solver, driven by contact spans in the clip instead of by a gait. The
//! solver is deliberately general — a two-bone chain and a target — rather
//! than a leg-shaped special case, because it has both jobs to do.
//!
//! Which corrections a body gets is the state machine's business:
//! [`AnimationState::plants_feet`] decides, [`finish_pose`] applies. A body in
//! the air is the case that makes the point — its feet belong wherever the
//! animation put them, not welded to a floor it is nowhere near.

use bevy::prelude::*;

use crate::common::class::CROUCH_HEIGHT;
use crate::common::skeleton::gait::FootOffset;
use crate::common::skeleton::rig::{bone, Pose, Skeleton};
use crate::common::skeleton::state::{AnimationState, PoseInputs};

/// How far from straight, and from folded, a limb is kept.
///
/// Both extremes leave the plane the limb bends in undefined. This is well
/// below what anything measures a landing to, so a target at the very end of a
/// limb's reach is still reported — and drawn — as reached.
const STRAIGHT_MARGIN: f32 = 1e-5;

/// Two bones and, optionally, what hangs off the end of them.
///
/// The `tip` keeps the world orientation it had before the solve. That is what
/// keeps a sole flat on the floor while the knee above it bends: without it, a
/// foot points wherever its shin ended up, which is the tell-tale of IK
/// applied without thinking about the joint below.
#[derive(Clone, Copy, Debug)]
pub struct Chain {
    pub upper: &'static str,
    pub lower: &'static str,
    pub tip: Option<&'static str>,
}

pub const LEFT_LEG: Chain = Chain {
    upper: bone::THIGH_L,
    lower: bone::SHIN_L,
    tip: Some(bone::FOOT_L),
};

pub const RIGHT_LEG: Chain = Chain {
    upper: bone::THIGH_R,
    lower: bone::SHIN_R,
    tip: Some(bone::FOOT_R),
};

pub const LEFT_ARM: Chain = Chain {
    upper: bone::UPPER_ARM_L,
    lower: bone::FOREARM_L,
    tip: Some(bone::HAND_L),
};

pub const RIGHT_ARM: Chain = Chain {
    upper: bone::UPPER_ARM_R,
    lower: bone::FOREARM_R,
    tip: Some(bone::HAND_R),
};

/// What happened when a chain was asked to reach somewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// The tip is on the target.
    Reached,
    /// The target is further away than the limb is long, so it is pointed at
    /// rather than reached. Worth telling a caller about: a leg that cannot
    /// reach the ground is a body that should be falling, not one that should
    /// stretch.
    Short,
    /// The chain is not in this skeleton, or the target is on top of the
    /// joint it hangs from. Nothing was written.
    Impossible,
}

/// Bend `chain` so its end lands on `target`, with the joint pointing along
/// `pole`.
///
/// Writes the two (or three) joints into `pose` and leaves everything else
/// alone, so it composes with whatever produced the pose — a function, a clip,
/// or an earlier correction.
///
/// `root` is the body's transform, feet on the floor, and `target` and `pole`
/// are in world space alongside it.
pub fn solve(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    chain: &Chain,
    target: Vec3,
    pole: Vec3,
) -> Reach {
    let bones = skeleton.bones();
    let (Some(upper), Some(lower)) = (skeleton.index_of(chain.upper), skeleton.index_of(chain.lower))
    else {
        return Reach::Impossible;
    };

    // The chain as it stands, which is what the solve is relative to: the
    // shoulder or hip may itself have been moved by the pose this is fixing up.
    let posed = skeleton.posed_bones(pose, root);
    let joint = posed[upper].head;
    let (first, second) = (bones[upper].length, bones[lower].length);
    let parent = match bones[upper].parent {
        Some(parent) => posed[parent].rotation,
        None => root.rotation,
    };

    let to_target = target - joint;
    let distance = to_target.length();
    if distance < 1e-5 {
        return Reach::Impossible;
    }
    let direction = to_target / distance;

    // Clamped just inside the limb's range: exactly straight and exactly
    // folded are both places where the plane the limb bends in stops being
    // defined, and a hair inside them is indistinguishable to look at. The
    // hair is smaller than anything a caller checks a landing against, so a
    // target at full stretch still counts as reached.
    let reach = first + second;
    let usable = distance.clamp((first - second).abs() + STRAIGHT_MARGIN, reach - STRAIGHT_MARGIN);

    // The angle between the limb's first bone and the line to the target, from
    // the triangle the two bones and that line make.
    let cosine = ((first * first + usable * usable - second * second) / (2.0 * first * usable))
        .clamp(-1.0, 1.0);
    let bend = cosine.acos();

    // The limb bends in the plane containing the target and the pole, and
    // towards the pole — that is what stops knees from choosing a side.
    let axis = direction.cross(pole);
    let axis = if axis.length_squared() < 1e-8 {
        // The pole is along the limb, so it says nothing about the plane. Any
        // plane will do; keeping it stable frame to frame is what matters.
        direction.any_orthonormal_vector()
    } else {
        axis.normalize()
    };

    let upper_direction = Quat::from_axis_angle(axis, bend) * direction;
    let elbow = joint + upper_direction * first;
    let end = joint + direction * usable;
    let lower_direction = (end - elbow).normalize_or_zero();
    if lower_direction == Vec3::ZERO {
        return Reach::Impossible;
    }

    let upper_rotation = aim(upper_direction, pole);
    let lower_rotation = aim(lower_direction, pole);

    // Back from world orientations to what a pose stores: the turn each joint
    // makes away from its rest, in its parent's frame.
    pose.set(chain.upper, (parent * bones[upper].rest).inverse() * upper_rotation);
    pose.set(chain.lower, (upper_rotation * bones[lower].rest).inverse() * lower_rotation);

    if let Some(tip) = chain.tip {
        if let Some(index) = skeleton.index_of(tip) {
            let was = posed[index].rotation;
            pose.set(tip, (lower_rotation * bones[index].rest).inverse() * was);
        }
    }

    if distance <= reach + STRAIGHT_MARGIN {
        Reach::Reached
    } else {
        Reach::Short
    }
}

/// Hold both feet where a body standing still would have them.
///
/// The targets come from the rest pose at the same place on the floor, so
/// anything the animation does above the hips — a settle, a lean, a crouch —
/// happens over feet that stay put, and the knees take the difference.
///
/// Translation-invariant by construction: targets and joints are both measured
/// at `root`, so the whole thing depends on the body's facing and not on where
/// it is standing.
///
/// Standing still is the only source of targets there is today. A gait will
/// supply its own — a foot planted where it was put down, until the stride
/// lifts it — and a clip's contact spans will supply theirs. Those replace the
/// targets, not the solve underneath.
pub fn plant_feet(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    offsets: [FootOffset; 2],
) -> [Reach; 2] {
    let mut reached = [Reach::Impossible; 2];
    let standing = skeleton.posed_bones(&Pose::rest(), root);
    // Knees bend forwards. The one thing the solver cannot work out for
    // itself, and the one thing a body is never in two minds about.
    let forward = root.rotation * Vec3::NEG_Z;

    for (index, (chain, offset)) in [LEFT_LEG, RIGHT_LEG].iter().zip(offsets).enumerate() {
        let Some(resting) = standing
            .iter()
            .find(|posed| posed.name == chain.lower)
            .map(|posed| posed.tail)
        else {
            continue;
        };

        // In leg-lengths, so a long-legged build takes a longer step in metres
        // and the same step as a fraction of itself.
        let leg = chain_length(skeleton, chain);
        let target = resting + (forward * offset.ahead + Vec3::Y * offset.lift) * leg;
        reached[index] = solve(skeleton, pose, root, chain, target, forward);
    }

    // Handed back rather than swallowed: a foot that cannot reach where the
    // gait wants it is a stride the body is not built for, and eventually it
    // is also how a leg finds out the ground is further away than it is long.
    reached
}

/// How long a chain is, end to end, fully extended.
pub fn chain_length(skeleton: &Skeleton, chain: &Chain) -> f32 {
    let bones = skeleton.bones();
    [chain.upper, chain.lower]
        .iter()
        .filter_map(|name| skeleton.index_of(name))
        .map(|index| bones[index].length)
        .sum()
}

/// How long this body's legs are, which is the unit a gait's strides are
/// measured in.
pub fn leg_length(skeleton: &Skeleton) -> f32 {
    chain_length(skeleton, &LEFT_LEG)
}

/// Sit a body exactly under `ceiling`, measured from its feet: low enough to
/// fit, and no lower.
///
/// A crouched body has to fit inside the box it ducks through gaps with, and
/// that box is one size for everybody while the bodies are not. A build with a
/// long back and short legs cannot fold into it as easily as a lanky one, and
/// a small build would be folding for no reason. So how far to duck is not a
/// number that can be written down once — it is a constraint, like a planted
/// foot, and it is solved the same way.
///
/// Both directions on purpose. Cancelling the slack is what keeps a crouched
/// body's head still: a step bob that pushed the crown up would push it into
/// the vent the body is ducking through, and holding the head level while the
/// legs do the work is what crouching along a low gap actually looks like.
///
/// Exact in one pass, because the root offset translates the whole body
/// rigidly. The knees are put back underneath afterwards by [`plant_feet`].
pub fn duck_under(skeleton: &Skeleton, pose: &mut Pose, root: &Transform, ceiling: f32) {
    let highest = skeleton
        .posed_bones(pose, root)
        .iter()
        .map(|posed| posed.head.y.max(posed.tail.y))
        .fold(f32::MIN, f32::max);

    let slack = (highest - root.translation.y) - ceiling;
    if slack.abs() < 1e-5 {
        return;
    }

    // In hip-heights, which is what a root offset is measured in. Never above
    // rest: a body small enough to walk under the gap stands up straight
    // rather than being stretched to reach the ceiling.
    let hips = pose.root_offset.y - slack / skeleton.proportions().hip_metres();
    pose.root_offset.y = hips.min(0.0);
}

/// The pose a body is actually in: its state's own pose, plus the corrections
/// that state calls for.
///
/// One function because there are two callers that must not disagree — the one
/// that draws a body and the one that works out where it can be hit. A hitbox
/// computed from an uncorrected pose would sit somewhere the body visibly is
/// not.
pub fn finish_pose(
    skeleton: &Skeleton,
    state: AnimationState,
    inputs: &PoseInputs,
    root: &Transform,
) -> Pose {
    let mut pose = state.pose(inputs);
    // Ducking first: it lowers the hips, and planting is what puts the knees
    // back underneath wherever they end up.
    if state.ducks() {
        duck_under(skeleton, &mut pose, root, CROUCH_HEIGHT);
    }
    if state.plants_feet() {
        plant_feet(skeleton, &mut pose, root, state.foot_offsets(inputs));
    }
    pose
}

/// The orientation that points a bone's local `+Y` along `along`, with its
/// local `+Z` as near `reference` as it can be.
///
/// The reference is what fixes the roll. Handing it the pole means a knee and
/// the foot below it face the same way the limb bends, which is the difference
/// between a leg and a leg with a twisted shin.
fn aim(along: Vec3, reference: Vec3) -> Quat {
    let y = along.normalize_or_zero();
    if y == Vec3::ZERO {
        return Quat::IDENTITY;
    }

    let flattened = reference - y * reference.dot(y);
    let z = if flattened.length_squared() < 1e-8 {
        y.any_orthonormal_vector()
    } else {
        flattened.normalize()
    };

    Quat::from_mat3(&Mat3::from_cols(y.cross(z), y, z))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::skeleton::rig::{humanoid, Proportions};

    fn tip_of(skeleton: &Skeleton, pose: &Pose, root: &Transform, name: &str) -> Vec3 {
        skeleton
            .posed_bones(pose, root)
            .into_iter()
            .find(|posed| posed.name == name)
            .expect("no such bone")
            .tail
    }

    /// The whole contract in one line: ask for a place, get the joints that
    /// put the limb's end there.
    #[test]
    fn the_end_of_a_chain_lands_on_its_target() {
        for proportions in [Proportions::DEFAULT, Proportions::STOCKY, Proportions::LANKY] {
            let skeleton = humanoid(proportions);
            let root = Transform::from_xyz(1.0, 0.0, -2.0);
            let standing = tip_of(&skeleton, &Pose::rest(), &root, bone::SHIN_L);

            for target in [
                standing + Vec3::new(0.0, 0.1, -0.3),
                standing + Vec3::new(-0.15, 0.25, 0.2),
                standing + Vec3::new(0.05, 0.4, 0.0),
                standing,
            ] {
                let mut pose = Pose::rest();
                let reach = solve(&skeleton, &mut pose, &root, &LEFT_LEG, target, root.rotation * Vec3::NEG_Z);

                assert_eq!(reach, Reach::Reached);
                let landed = tip_of(&skeleton, &pose, &root, bone::SHIN_L);
                assert!(
                    (landed - target).length() < 1e-4,
                    "asked for {target}, got {landed} on {proportions:?}"
                );
            }
        }
    }

    /// Arms are legs with a different name — the solver has to be general,
    /// because a clip's contact spans will want hands as well as feet.
    #[test]
    fn an_arm_reaches_too() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::IDENTITY;
        let target = Vec3::new(0.35, 1.4, -0.45);

        let mut pose = Pose::rest();
        let reach = solve(&skeleton, &mut pose, &root, &RIGHT_ARM, target, Vec3::NEG_Z);

        assert_eq!(reach, Reach::Reached);
        assert!((tip_of(&skeleton, &pose, &root, bone::FOREARM_R) - target).length() < 1e-4);
    }

    /// A target further away than the limb is long points the limb at it
    /// rather than stretching, and says so.
    #[test]
    fn an_unreachable_target_is_reported_rather_than_stretched_to() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::IDENTITY;
        let hip = skeleton
            .posed_bones(&Pose::rest(), &root)
            .into_iter()
            .find(|posed| posed.name == bone::THIGH_L)
            .unwrap()
            .head;
        let target = hip + Vec3::new(0.0, 0.0, -5.0);

        let mut pose = Pose::rest();
        assert_eq!(
            solve(&skeleton, &mut pose, &root, &LEFT_LEG, target, Vec3::NEG_Z),
            Reach::Short
        );

        let landed = tip_of(&skeleton, &pose, &root, bone::SHIN_L);
        let leg = (landed - hip).length();
        assert!(leg.is_finite(), "the solve produced {landed}");
        assert!((landed - hip).normalize().dot(Vec3::NEG_Z) > 0.99, "the leg is not pointing at the target");
        // Straight, not stretched: still exactly as long as the two bones.
        let bones = skeleton.bones();
        let bone_length: f32 = [bone::THIGH_L, bone::SHIN_L]
            .iter()
            .map(|name| bones[skeleton.index_of(name).unwrap()].length)
            .sum();
        assert!((leg - bone_length).abs() < 1e-3, "the leg is {leg} long, not {bone_length}");
    }

    /// Knees forward, always. Reverse the pole and the same target is reached
    /// with the joint on the other side, which is what makes the parameter
    /// worth having.
    #[test]
    fn the_joint_bends_towards_the_pole() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::IDENTITY;
        let target = tip_of(&skeleton, &Pose::rest(), &root, bone::SHIN_L) + Vec3::Y * 0.3;

        let knee = |pole: Vec3| {
            let mut pose = Pose::rest();
            solve(&skeleton, &mut pose, &root, &LEFT_LEG, target, pole);
            skeleton
                .posed_bones(&pose, &root)
                .into_iter()
                .find(|posed| posed.name == bone::SHIN_L)
                .unwrap()
                .head
        };

        assert!(knee(Vec3::NEG_Z).z < knee(Vec3::Z).z, "the pole did not choose the side");
    }

    /// The sole stays flat while the knee above it bends. Without the tip
    /// step, a planted foot points wherever its shin ended up.
    #[test]
    fn the_foot_keeps_its_orientation() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::IDENTITY;
        let facing = |pose: &Pose| {
            let posed = skeleton
                .posed_bones(pose, &root)
                .into_iter()
                .find(|posed| posed.name == bone::FOOT_L)
                .unwrap();
            posed.tail - posed.head
        };

        let mut pose = Pose::rest();
        let before = facing(&pose);
        let target = tip_of(&skeleton, &Pose::rest(), &root, bone::SHIN_L) + Vec3::Y * 0.25;
        solve(&skeleton, &mut pose, &root, &LEFT_LEG, target, Vec3::NEG_Z);

        assert!(
            (facing(&pose) - before).length() < 1e-4,
            "the foot turned from {before} to {}",
            facing(&pose)
        );
    }

    /// What the idle used to do with hand-worked arithmetic, now a
    /// consequence of the solver: the hips move and the feet do not.
    #[test]
    fn planting_holds_the_feet_while_the_hips_drop() {
        for proportions in [Proportions::DEFAULT, Proportions::STOCKY, Proportions::LANKY] {
            let skeleton = humanoid(proportions);
            let root = Transform::from_xyz(-3.0, 1.0, 4.0)
                .with_rotation(Quat::from_rotation_y(0.9));
            let standing: Vec<Vec3> = [bone::FOOT_L, bone::FOOT_R]
                .iter()
                .map(|name| tip_of(&skeleton, &Pose::rest(), &root, name))
                .collect();

            for drop in [0.0, 0.01, 0.05, 0.12] {
                let mut pose = Pose::rest();
                pose.root_offset = Vec3::NEG_Y * drop;
                plant_feet(&skeleton, &mut pose, &root, [FootOffset::default(); 2]);

                for (name, was) in [bone::FOOT_L, bone::FOOT_R].iter().zip(&standing) {
                    let now = tip_of(&skeleton, &pose, &root, name);
                    assert!(
                        (now - *was).length() < 1e-4,
                        "{name} moved {:.5} m when the hips dropped {drop} on {proportions:?}",
                        (now - *was).length()
                    );
                }
            }
        }
    }

    /// Planting is a property of the state, not of every body. A body in the
    /// air whose feet were welded to the floor would be a body doing the
    /// splits on the way up.
    #[test]
    fn only_states_that_ask_for_it_get_their_feet_planted() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::IDENTITY;

        assert!(AnimationState::Idle.plants_feet());
        assert!(!AnimationState::Airborne.plants_feet());

        // And the pipeline honours it: an airborne body's legs come out of the
        // pipeline exactly as its own pose left them, tuck and all.
        let inputs = PoseInputs { seconds: 1.0, ..default() };
        let finished = finish_pose(&skeleton, AnimationState::Airborne, &inputs, &root);
        let untouched = AnimationState::Airborne.pose(&inputs);
        assert_eq!(finished.joint(bone::THIGH_L), untouched.joint(bone::THIGH_L));
        assert_eq!(finished.joint(bone::SHIN_L), untouched.joint(bone::SHIN_L));

        let idling = finish_pose(&skeleton, AnimationState::Idle, &inputs, &root);
        assert_ne!(idling.joint(bone::THIGH_L), Quat::IDENTITY, "the idle was never corrected");
    }
}
