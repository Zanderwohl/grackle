//! Putting the end of a limb where it has to be.
//!
//! Joint angles retarget across builds; contacts do not, since different leg
//! lengths at the same angles put the feet in different places. So a contact
//! is a target and the angles are solved for. The solver is a general two-bone
//! chain, used by gaits and by clip contacts alike.
//!
//! [`AnimationState::plants_feet`] decides which corrections a body gets;
//! [`finish_pose`] applies them.

use bevy::prelude::*;

use crate::common::class::CROUCH_HEIGHT;
use crate::common::skeleton::anatomy::PerFoot;
use crate::common::skeleton::gait::FootOffset;
use crate::common::skeleton::interpolation::Interpolation;
use crate::common::skeleton::rig::{bone, Pose, Skeleton};
use crate::common::skeleton::clip::{Clip, StateAnimation};
use crate::common::skeleton::play::{apply, Frames, Regions};
use crate::common::skeleton::state::{AnimationState, ClipLibrary, PoseInputs, SkeletonAnimator};

/// How far from straight and from folded a limb is kept, since both leave the
/// bend plane undefined. Small enough that full stretch still counts as reached.
const STRAIGHT_MARGIN: f32 = 1e-5;

/// `upper`, any `middle` bones, and `lower`, root first, and optionally a `tip` that keeps its
/// world orientation through the solve, so a sole stays flat while the knee bends. Two bones are
/// solved exactly; more by [`fabrik`].
#[derive(Clone, Copy, Debug)]
pub struct Chain {
    pub upper: &'static str,
    /// Interned (`rig::intern_list`), so a chain stays `Copy`.
    pub middle: &'static [&'static str],
    pub lower: &'static str,
    pub tip: Option<&'static str>,
    /// Bit `i` set: the joint at the head of the chain's bone `i` (root first, so from 1) bends
    /// away from the pole rather than toward it, as a digitigrade hock does under a forward knee.
    pub away: u32,
}

impl Chain {
    /// Three bones, the knee bending toward the pole and the hock away: solved exactly by
    /// [`digitigrade`] rather than by [`fabrik`].
    pub fn is_digitigrade(&self) -> bool {
        self.middle.len() == 1 && self.away == 1 << 2
    }

    /// Root first, without the tip.
    pub fn bones(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.upper).chain(self.middle.iter().copied()).chain(std::iter::once(self.lower))
    }
}

pub const LEFT_LEG: Chain = Chain {
    upper: bone::THIGH_L,
    middle: &[],
    lower: bone::SHIN_L,
    tip: Some(bone::FOOT_L),
    away: 0,
};

pub const RIGHT_LEG: Chain = Chain {
    upper: bone::THIGH_R,
    middle: &[],
    lower: bone::SHIN_R,
    tip: Some(bone::FOOT_R),
    away: 0,
};

pub const LEFT_ARM: Chain = Chain {
    upper: bone::UPPER_ARM_L,
    middle: &[],
    lower: bone::FOREARM_L,
    tip: Some(bone::HAND_L),
    away: 0,
};

pub const RIGHT_ARM: Chain = Chain {
    upper: bone::UPPER_ARM_R,
    middle: &[],
    lower: bone::FOREARM_R,
    tip: Some(bone::HAND_R),
    away: 0,
};

/// What happened when a chain was asked to reach somewhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reach {
    /// The tip is on the target.
    Reached,
    /// Out of reach: the limb is pointed straight at it, not stretched.
    Short,
    /// The chain is not in this skeleton, or the target is on its root joint.
    /// Nothing was written.
    #[default]
    Impossible,
}

/// Bend `chain` so its end lands on `target`, with the joint pointing along
/// `pole`. Writes only the chain's joints. `target` and `pole` are world space.
pub fn solve(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    chain: &Chain,
    target: Vec3,
    pole: Vec3,
) -> Reach {
    if chain.is_digitigrade() {
        return digitigrade(skeleton, pose, root, chain, target, pole);
    }
    if !chain.middle.is_empty() {
        return fabrik(skeleton, pose, root, chain, target, pole);
    }
    let bones = skeleton.bones();
    let (Some(upper), Some(lower)) = (skeleton.index_of(chain.upper), skeleton.index_of(chain.lower))
    else {
        return Reach::Impossible;
    };

    // Relative to the posed chain: the pose may have moved the hip or shoulder.
    // Only the bones read are placed, and the tip before its joints are rewritten.
    let joint = skeleton.posed_bone(pose, root, upper).head;
    let (first, second) = (bones[upper].length, bones[lower].length);
    let parent = match bones[upper].parent {
        Some(parent) => skeleton.posed_bone(pose, root, parent).rotation,
        None => root.rotation,
    };
    let tip = chain.tip.and_then(|tip| {
        let index = skeleton.index_of(tip)?;
        Some((tip, index, skeleton.posed_bone(pose, root, index).rotation))
    });

    let to_target = target - joint;
    let distance = to_target.length();
    if distance < 1e-5 {
        return Reach::Impossible;
    }
    let direction = to_target / distance;

    // See `STRAIGHT_MARGIN`.
    let reach = first + second;
    let usable = distance.clamp((first - second).abs() + STRAIGHT_MARGIN, reach - STRAIGHT_MARGIN);

    // Law of cosines: the first bone's angle off the line to the target.
    let cosine = ((first * first + usable * usable - second * second) / (2.0 * first * usable))
        .clamp(-1.0, 1.0);
    let bend = cosine.acos();

    let axis = direction.cross(pole);
    let axis = if axis.length_squared() < 1e-8 {
        // Pole along the limb: any plane, as long as it is stable.
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

    // Roll from the hinge, not the pole: a bone near-parallel to the pole (a
    // shin in a deep crouch) flips its frame half a turn. See `aim`.
    let upper_rotation = aim(upper_direction, axis);
    let lower_rotation = aim(lower_direction, axis);

    // World orientation to rest-relative, parent-frame rotation.
    pose.set(chain.upper, (parent * bones[upper].rest).inverse() * upper_rotation);
    pose.set(chain.lower, (upper_rotation * bones[lower].rest).inverse() * lower_rotation);

    if let Some((tip, index, was)) = tip {
        pose.set(tip, (lower_rotation * bones[index].rest).inverse() * was);
    }

    if distance <= reach + STRAIGHT_MARGIN {
        Reach::Reached
    } else {
        Reach::Short
    }
}

/// Hold each of the rig's feet at its rest position plus its offset, so the knees take whatever
/// the animation does above the hips. `offsets` is in `Anatomy::feet` order.
pub fn plant_feet(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    offsets: &[FootOffset],
) -> PerFoot<Reach> {
    let standing = Pose::rest();
    let forward = root.rotation * Vec3::NEG_Z;
    let across = root.rotation * Vec3::X;
    let anatomy = skeleton.anatomy();
    anatomy
        .feet
        .iter()
        .zip(offsets)
        .map(|(foot, offset)| {
            let Some(limb) = anatomy.limbs.get(foot.limb) else { return Reach::Impossible };
            let chain = &limb.chain;
            let Some(resting) = skeleton
                .index_of(chain.lower)
                .map(|lower| skeleton.posed_bone(&standing, root, lower).tail)
            else {
                return Reach::Impossible;
            };
            // Offsets are in leg-lengths.
            let leg = chain_length(skeleton, chain);
            let target = resting
                + (forward * offset.ahead + across * offset.across + Vec3::Y * offset.lift) * leg;
            solve(skeleton, pose, root, chain, target, root.rotation * limb.pole)
        })
        .collect()
}

/// [`solve`], but a limb that does not reach is left as it was. [`solve`]
/// writes a straightened limb before reporting [`Reach::Short`], which reads as
/// broken on an arm at a grip it cannot hold.
pub fn reach_or_leave(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    chain: &Chain,
    target: Vec3,
    pole: Vec3,
) -> Reach {
    let before = chain
        .bones()
        .chain(chain.tip)
        .map(|joint| (joint, pose.joint(joint)))
        .collect::<Vec<_>>();

    let reach = solve(skeleton, pose, root, chain, target, pole);
    if reach != Reach::Reached {
        for (joint, rotation) in before {
            pose.set(joint, rotation);
        }
    }
    reach
}

/// Turn a bone to a world orientation, whatever its parents are doing.
pub fn set_world_rotation(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    name: &'static str,
    wanted: Quat,
) {
    let Some(index) = skeleton.index_of(name) else { return };
    let posed = skeleton.posed_bone(pose, root, index).rotation;
    let base = posed * pose.joint(name).inverse();
    pose.set(name, base.inverse() * wanted);
}

/// Which way an elbow points: back, a little out and down. `side` is +1 on the
/// right. In the body's frame; rotate it by the root before solving, or a
/// turned body poles its elbows against a fixed direction.
pub fn elbow_pole(side: f32) -> Vec3 {
    Vec3::new(0.35 * side, -0.85, 0.55).normalize()
}

/// Fully extended length, excluding the tip.
pub fn chain_length(skeleton: &Skeleton, chain: &Chain) -> f32 {
    let bones = skeleton.bones();
    chain.bones().filter_map(|name| skeleton.index_of(name)).map(|index| bones[index].length).sum()
}

/// The unit a gait's strides are measured in.
pub fn leg_length(skeleton: &Skeleton) -> f32 {
    skeleton.anatomy().leg
}

/// Sit a body exactly under `ceiling` (meters above its feet), never above
/// rest. The crouch hull is one size for every build, so the duck depth is
/// solved per build rather than authored.
///
/// Raises as well as lowers, which cancels a step bob so the head stays level.
/// Exact in one pass since the root offset is rigid; [`plant_feet`] fixes the
/// knees afterward.
pub fn duck_under(skeleton: &Skeleton, pose: &mut Pose, root: &Transform, ceiling: f32) {
    let mut highest = f32::MIN;
    skeleton.each_posed_bone(pose, root, |posed| {
        highest = f32::max(highest, posed.head.y.max(posed.tail.y));
    });

    let slack = (highest - root.translation.y) - ceiling;
    if slack.abs() < 1e-5 {
        return;
    }

    // Clamped at rest, so a small body is not stretched up to the ceiling.
    let hips = pose.root_offset.y - slack / skeleton.anatomy().hip;
    pose.root_offset.y = hips.min(0.0);
}

/// The pose during a state transition. Blends the finished poses, since a
/// crouch is only a crouch after `duck_under`. `crouch` is as [`ducked_pose`].
pub fn animator_pose(
    skeleton: &Skeleton,
    animator: &SkeletonAnimator,
    inputs: &PoseInputs,
    root: &Transform,
    clips: Option<&ClipLibrary>,
    crouch: Option<f32>,
) -> Pose {
    let arriving = ducked_pose(skeleton, animator.state(), inputs, root, clips, crouch);
    if animator.blend() >= 1.0 || animator.previous() == animator.state() {
        return arriving;
    }

    let leaving = ducked_pose(skeleton, animator.previous(), inputs, root, clips, crouch);
    Pose::lerp(&leaving, &arriving, Interpolation::SmoothStep.ease(animator.blend()))
}

/// `state` done `crouch` of the way from standing to crouched, 0 to 1. `None` takes the state as
/// it is, for a body with no duck of its own, whose state alone says whether it is crouched.
pub fn ducked_pose(
    skeleton: &Skeleton,
    state: AnimationState,
    inputs: &PoseInputs,
    root: &Transform,
    clips: Option<&ClipLibrary>,
    crouch: Option<f32>,
) -> Pose {
    let finish = |state| finish_pose(skeleton, state, inputs, root, clips);
    let crouch = match crouch {
        None => return finish(state),
        Some(crouch) if crouch <= 0.0 => return finish(state.upright()),
        Some(crouch) if crouch >= 1.0 => return finish(state.ducked()),
        Some(crouch) => crouch,
    };
    let (upright, ducked) = (state.upright(), state.ducked());
    let (standing, crouched) = (finish(upright), finish(ducked));
    let mut pose = Pose::lerp(&standing, &crouched, crouch);
    // A squat, then a hunch: the hips go down with the duck and the body above them leans in
    // late, so the head drops rather than swinging forward on its way down.
    let lean = crouch * crouch;
    for (index, name) in skeleton.bones().iter().map(|bone| bone.name).enumerate() {
        if !matches!(skeleton.anatomy().region_of(index), Regions::LEGS | Regions::NONE) {
            pose.set(name, standing.joint(name).slerp(crouched.joint(name), lean));
        }
    }
    // Blended knees do not follow blended hips down, and the feet sink through the floor.
    if upright.plants_feet() && ducked.plants_feet() {
        let feet = |state: AnimationState| state.foot_offsets(inputs, skeleton.anatomy());
        let offsets: PerFoot<FootOffset> = feet(upright)
            .iter()
            .zip(feet(ducked).iter())
            .map(|(from, to)| FootOffset {
                ahead: from.ahead.lerp(to.ahead, crouch),
                across: from.across.lerp(to.across, crouch),
                lift: from.lift.lerp(to.lift, crouch),
            })
            .collect();
        plant_by(skeleton, &mut pose, root, &offsets, inputs.feet_free);
    }
    pose
}

/// A state's pose plus its corrections. Shared by drawing and hitboxes, which
/// must not disagree.
pub fn finish_pose(
    skeleton: &Skeleton,
    state: AnimationState,
    inputs: &PoseInputs,
    root: &Transform,
    clips: Option<&ClipLibrary>,
) -> Pose {
    // A state without a clip falls back to its function; `None` is the
    // all-function baseline the comparison tests measure against.
    let mut pose = clips
        .and_then(|library| library.get_for(skeleton.kind(), state))
        .map(|animation| animation_pose(animation, state, inputs, skeleton, root))
        .unwrap_or_else(|| fallback_pose(skeleton, state, inputs));
    correct_for_state(skeleton, &mut pose, state, inputs, root);
    pose
}

/// A state with no clip: the humanoid's pose function, which names the humanoid's bones, or rest
/// for any other rig.
fn fallback_pose(skeleton: &Skeleton, state: AnimationState, inputs: &PoseInputs) -> Pose {
    match skeleton.kind() {
        super::rig::HUMANOID => state.pose(inputs),
        _ => Pose::rest(),
    }
}

/// The corrections a state calls for. Public so the animation editor previews
/// a clip as it is played rather than as authored.
pub fn correct_for_state(
    skeleton: &Skeleton,
    pose: &mut Pose,
    state: AnimationState,
    inputs: &PoseInputs,
    root: &Transform,
) {
    // Duck first; planting puts the knees back under the lowered hips.
    if state.ducks() {
        duck_under(skeleton, pose, root, CROUCH_HEIGHT);
    }
    if state.plants_feet() {
        plant_by(skeleton, pose, root, &state.foot_offsets(inputs, skeleton.anatomy()), inputs.feet_free);
    }
}

/// [`plant_feet`], blended back toward the unplanted legs by `free`.
fn plant_by(skeleton: &Skeleton, pose: &mut Pose, root: &Transform, offsets: &[FootOffset], free: f32) {
    if free >= 1.0 {
        return;
    }
    let loose = (free > 0.0).then(|| pose.clone());
    plant_feet(skeleton, pose, root, offsets);
    let Some(loose) = loose else { return };
    let anatomy = skeleton.anatomy();
    for foot in &anatomy.feet {
        let Some(limb) = anatomy.limbs.get(foot.limb) else { continue };
        for name in limb.chain.bones().chain(limb.chain.tip) {
            pose.set(name, pose.joint(name).slerp(loose.joint(name), free));
        }
    }
}

/// A state's animation before corrections. A blend lerps the two variants
/// either side of the heading joint by joint, which is why neighboring
/// variants must drive the same joints. Public so the animation editor plays a
/// blend the same way.
pub fn animation_pose(
    animation: &StateAnimation,
    state: AnimationState,
    inputs: &PoseInputs,
    skeleton: &Skeleton,
    root: &Transform,
) -> Pose {
    let Some((first, second)) = animation.at_heading(state.heading(inputs)) else {
        return fallback_pose(skeleton, state, inputs);
    };
    let footfalls = state
        .stance(inputs)
        .and_then(|stance| super::gait::footfalls(inputs.stride, stance, skeleton.anatomy()));
    let pose = clip_pose(first, inputs, footfalls, skeleton, root);
    match second {
        Some((other, t)) => Pose::lerp(&pose, &clip_pose(other, inputs, footfalls, skeleton, root), t),
        None => pose,
    }
}

/// One clip on a body. A body clip has no weapon or eye frame; the ground is
/// flat at the root's height.
fn clip_pose(
    clip: &Clip,
    inputs: &PoseInputs,
    footfalls: Option<PerFoot<f32>>,
    skeleton: &Skeleton,
    root: &Transform,
) -> Pose {
    let mut pose = Pose::rest();
    let floor = root.translation.y;
    let ground = move |_: Vec3| floor;
    let frames = Frames {
        body: *root,
        carry: *root,
        weapon: None,
        eye: None,
        ground: &ground,
        footfalls,
    };
    apply(clip, clip.time_from(inputs), skeleton, &mut pose, &frames, None);
    pose
}

/// A thigh, shin and metatarsus onto `target`, the knee toward `pole` and the hock away from it.
///
/// The metatarsus keeps its rest angle to the thigh, as a bird's or a dog's roughly does, so the
/// two add up to one rigid bone from the hip and the leg is [`solve`]'s two-bone problem: exact,
/// and the same every tick. Out of reach is that virtual leg straightened, not the real one.
fn digitigrade(skeleton: &Skeleton, pose: &mut Pose, root: &Transform, chain: &Chain, target: Vec3, pole: Vec3) -> Reach {
    let bones = skeleton.bones();
    let (Some(thigh), Some(shin), Some(meta)) = (
        skeleton.index_of(chain.upper),
        chain.middle.first().and_then(|name| skeleton.index_of(name)),
        skeleton.index_of(chain.lower),
    ) else {
        return Reach::Impossible;
    };
    let hip = skeleton.posed_bone(pose, root, thigh).head;
    let parent = match bones[thigh].parent {
        Some(parent) => skeleton.posed_bone(pose, root, parent).rotation,
        None => root.rotation,
    };
    let tip = chain.tip.and_then(|tip| {
        let index = skeleton.index_of(tip)?;
        Some((tip, index, skeleton.posed_bone(pose, root, index).rotation))
    });

    let to_target = target - hip;
    let distance = to_target.length();
    if distance < 1e-5 {
        return Reach::Impossible;
    }
    let direction = to_target / distance;
    let axis = direction.cross(pole).try_normalize().unwrap_or_else(|| direction.any_orthonormal_vector());

    // The metatarsus's rest direction in the thigh's frame, as a turn about the thigh's `+X`
    // (the hinge axis; positive toward `+Z`).
    let (upper, middle, lower) = (bones[thigh].length, bones[shin].length, bones[meta].length);
    let rest = bones[shin].rest * bones[meta].rest * Vec3::Y;
    let hock = rest.z.atan2(rest.y);
    // Thigh plus metatarsus as one bone, `upper + lower·e^(i·hock)` in the bend plane.
    let (along, across) = (upper + lower * hock.cos(), lower * hock.sin());
    let first = along.hypot(across);
    let offset = across.atan2(along);

    let reach = first + middle;
    let usable = distance.clamp((first - middle).abs() + STRAIGHT_MARGIN, reach - STRAIGHT_MARGIN);
    let cosine = ((first * first + usable * usable - middle * middle) / (2.0 * first * usable)).clamp(-1.0, 1.0);
    let virtual_bone = Quat::from_axis_angle(axis, cosine.acos()) * direction;
    let elbow = hip + virtual_bone * first;
    let end = hip + direction * usable;

    let thigh_direction = Quat::from_axis_angle(axis, -offset) * virtual_bone;
    let shin_direction = (end - elbow).normalize_or_zero();
    let meta_direction = Quat::from_axis_angle(axis, hock) * thigh_direction;
    if shin_direction == Vec3::ZERO {
        return Reach::Impossible;
    }

    let thigh_rotation = aim(thigh_direction, axis);
    let shin_rotation = aim(shin_direction, axis);
    let meta_rotation = aim(meta_direction, axis);
    pose.set(chain.upper, (parent * bones[thigh].rest).inverse() * thigh_rotation);
    pose.set(bones[shin].name, (thigh_rotation * bones[shin].rest).inverse() * shin_rotation);
    pose.set(chain.lower, (shin_rotation * bones[meta].rest).inverse() * meta_rotation);
    if let Some((tip, index, was)) = tip {
        pose.set(tip, (meta_rotation * bones[index].rest).inverse() * was);
    }

    if distance <= reach + STRAIGHT_MARGIN {
        Reach::Reached
    } else {
        Reach::Short
    }
}

/// Rounds of [`fabrik`]: positions, then rotations through each joint's limit, then positions
/// again from where the limits left the joints. Fixed, so a hitbox rebuilt on the tick is the
/// same on every machine.
const FABRIK_ROUNDS: usize = 4;
/// Back-and-forth passes over the positions in each round.
const FABRIK_PASSES: usize = 8;
/// How close the tip must come to count as reached, meters.
const FABRIK_REACHED: f32 = 1e-3;

/// A chain of three or more bones onto `target`, each inner joint bending toward `pole` (world
/// space), or away from it where [`Chain::away`] says. FABRIK:
/// joint positions are pulled to the target and back to the root, then each bone is turned to
/// its new direction as far as its [`JointLimit`](super::joints::JointLimit) allows.
fn fabrik(skeleton: &Skeleton, pose: &mut Pose, root: &Transform, chain: &Chain, target: Vec3, pole: Vec3) -> Reach {
    let bones = skeleton.bones();
    let Some(indices) = chain.bones().map(|name| skeleton.index_of(name)).collect::<Option<Vec<_>>>() else {
        return Reach::Impossible;
    };
    let tip = chain.tip.and_then(|tip| {
        let index = skeleton.index_of(tip)?;
        Some((tip, index, skeleton.posed_bone(pose, root, index).rotation))
    });
    let lengths: Vec<f32> = indices.iter().map(|index| bones[*index].length).collect();
    let pole = pole.normalize_or_zero();

    let joints = |pose: &Pose| -> Vec<Vec3> {
        let mut points: Vec<Vec3> = indices.iter().map(|index| skeleton.posed_bone(pose, root, *index).head).collect();
        if let Some(last) = indices.last() {
            points.push(skeleton.posed_bone(pose, root, *last).tail);
        }
        points
    };
    let base = joints(pose)[0];
    if (target - base).length() < 1e-5 {
        return Reach::Impossible;
    }
    // One roll reference for the whole chain, square to the plane it bends in. Per bone, `bone ×
    // pole` vanishes for a bone running along the pole and flips its frame, and a zigzag has bones
    // on both sides of it.
    let hinge = (target - base)
        .cross(pole)
        .try_normalize()
        .unwrap_or_else(|| (target - base).normalize().any_orthonormal_vector());

    for round in 0..FABRIK_ROUNDS {
        let mut points = joints(pose);
        if round == 0 {
            // Each inner joint bowed its own way, so the chain settles bending as the rig says
            // rather than whichever way it started.
            let span = lengths.iter().sum::<f32>() * 0.05;
            let count = points.len();
            for (at, point) in points.iter_mut().enumerate().take(count - 1).skip(1) {
                let side = if chain.away & (1 << at) != 0 { -1.0 } else { 1.0 };
                *point += pole * side * span;
            }
        }
        for _ in 0..FABRIK_PASSES {
            let last = points.len() - 1;
            points[last] = target;
            for at in (0..last).rev() {
                let toward = (points[at] - points[at + 1]).normalize_or(Vec3::Y);
                points[at] = points[at + 1] + toward * lengths[at];
            }
            points[0] = base;
            for at in 0..last {
                let toward = (points[at + 1] - points[at]).normalize_or(Vec3::Y);
                points[at + 1] = points[at] + toward * lengths[at];
            }
        }

        // Root first, each from where the bones before it actually ended up.
        for (at, index) in indices.iter().enumerate() {
            let bone = &bones[*index];
            let head = skeleton.posed_bone(pose, root, *index).head;
            let parent = match bone.parent {
                Some(parent) => skeleton.posed_bone(pose, root, parent).rotation,
                None => root.rotation,
            };
            let frame = parent * bone.rest;
            let wanted = (points[at + 1] - head).normalize_or(frame * Vec3::Y);
            let allowed = frame * bone.limit.nearest(frame.inverse() * wanted);
            let rotation = aim(allowed, hinge);
            pose.set(bone.name, frame.inverse() * rotation);
        }
    }

    if let Some((tip, index, was)) = tip {
        let lower = skeleton.posed_bone(pose, root, *indices.last().unwrap_or(&index)).rotation;
        pose.set(tip, (lower * bones[index].rest).inverse() * was);
    }
    let reached = joints(pose).last().copied().unwrap_or(base);
    match reached.distance(target) <= FABRIK_REACHED {
        true => Reach::Reached,
        false => Reach::Short,
    }
}

/// Points local `+Y` along `along` with local `+X` along `hinge`. The roll
/// reference must stay square to the bone; a joint's hinge does, while a
/// near-parallel reference flips the frame half a turn.
fn aim(along: Vec3, hinge: Vec3) -> Quat {
    let y = along.normalize_or_zero();
    if y == Vec3::ZERO {
        return Quat::IDENTITY;
    }

    let flattened = hinge - y * hinge.dot(y);
    let x = if flattened.length_squared() < 1e-8 {
        y.any_orthonormal_vector()
    } else {
        flattened.normalize()
    };

    Quat::from_mat3(&Mat3::from_cols(x, y, x.cross(y)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::skeleton::rig::{humanoid, Proportions};

    /// Breaks if a duck blends knees without replanting them, so the feet sink through the floor
    /// halfway down, or the body leans in as early as the hips drop and the head swings forward.
    #[test]
    fn a_duck_squats_with_the_feet_on_the_floor_and_hunches_late() {
        use super::super::rig::bone;
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::IDENTITY;
        let inputs = PoseInputs::default();
        let at = |crouch: f32, name: &str| {
            let pose = ducked_pose(&skeleton, AnimationState::Idle, &inputs, &root, None, Some(crouch));
            skeleton.posed_bones(&pose, &root)[skeleton.index_of(name).unwrap()]
        };
        let ankle = at(0.0, bone::FOOT_L).head.y;
        for step in 1..10 {
            let crouch = step as f32 / 10.0;
            let sunk = ankle - at(crouch, bone::FOOT_L).head.y;
            assert!(sunk.abs() < 0.01, "{crouch} of the way down the ankle is {sunk} m off the floor");
        }
        let (head, halfway, down) = (at(0.0, bone::HEAD).tail, at(0.5, bone::HEAD).tail, at(1.0, bone::HEAD).tail);
        let forward = |head: Vec3| -head.z;
        assert!(forward(halfway) < forward(down) * 0.35, "halfway down the head is {} of the way forward", forward(halfway) / forward(down));
        assert!(halfway.y < head.y - (head.y - down.y) * 0.35, "halfway down the head has hardly dropped");
    }

    fn tip_of(skeleton: &Skeleton, pose: &Pose, root: &Transform, name: &str) -> Vec3 {
        skeleton
            .posed_bones(pose, root)
            .into_iter()
            .find(|posed| posed.name == name)
            .expect("no such bone")
            .tail
    }

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
        let bones = skeleton.bones();
        let bone_length: f32 = [bone::THIGH_L, bone::SHIN_L]
            .iter()
            .map(|name| bones[skeleton.index_of(name).unwrap()].length)
            .sum();
        assert!((leg - bone_length).abs() < 1e-3, "the leg is {leg} long, not {bone_length}");
    }

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
                plant_feet(&skeleton, &mut pose, &root, &[FootOffset::default(); 2]);

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

    /// A roll reference that can go parallel to a bone flips it half a turn
    /// between samples; this samples the states that bring limbs nearest that.
    #[test]
    fn a_solved_limb_never_lurches() {
        use crate::common::class::Class;
        use crate::common::skeleton::rig::{humanoid, Proportions};

        let corner = Vec2::new(1.0, 1.0).normalize();
        for proportions in [Proportions::DEFAULT, crate::common::class::ClassBuilds::shipped().of(Class::Heavy)] {
            let skeleton = humanoid(proportions);
            let root = Transform::IDENTITY;

            for (state, speed, travel) in [
                (AnimationState::CrouchWalk, 4.5, Vec2::Y),
                (AnimationState::CrouchWalkBackward, 4.5, Vec2::NEG_Y),
                (AnimationState::CrouchStrafeLeft, 4.5, Vec2::NEG_X),
                (AnimationState::RunForward, 2.0, Vec2::Y),
                (AnimationState::RunForward, 7.5, Vec2::Y),
                (AnimationState::RunForward, 4.5, corner),
                (AnimationState::StrafeRight, 4.5, Vec2::X),
            ] {
                let mut previous: Option<Pose> = None;
                for step in 0..400 {
                    let inputs = PoseInputs {
                        seconds: 0.0,
                        stride: step as f32 / 400.0,
                        speed,
                        travel,
                        ..Default::default()
                    };
                    let pose = finish_pose(&skeleton, state, &inputs, &root, None);

                    if let Some(before) = &previous {
                        for bone in skeleton.bones() {
                            let turned =
                                before.joint(bone.name).angle_between(pose.joint(bone.name));
                            assert!(
                                turned < 0.2,
                                "{} turns {turned:.2} rad in a four-hundredth of a {state:?} \
                                 cycle on {proportions:?}",
                                bone.name
                            );
                        }
                    }
                    previous = Some(pose);
                }
            }
        }
    }

    /// An airborne body with planted feet does the splits.
    #[test]
    fn only_states_that_ask_for_it_get_their_feet_planted() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let root = Transform::IDENTITY;

        assert!(AnimationState::Idle.plants_feet());
        assert!(!AnimationState::Airborne.plants_feet());

        let inputs = PoseInputs { seconds: 1.0, ..default() };
        let finished = finish_pose(&skeleton, AnimationState::Airborne, &inputs, &root, None);
        let untouched = AnimationState::Airborne.pose(&inputs);
        assert_eq!(finished.joint(bone::THIGH_L), untouched.joint(bone::THIGH_L));
        assert_eq!(finished.joint(bone::SHIN_L), untouched.joint(bone::SHIN_L));

        let idling = finish_pose(&skeleton, AnimationState::Idle, &inputs, &root, None);
        assert_ne!(idling.joint(bone::THIGH_L), Quat::IDENTITY, "the idle was never corrected");
    }
}
