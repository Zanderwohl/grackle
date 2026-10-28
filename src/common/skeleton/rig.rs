//! Skeletons built in code, not imported.
//!
//! A rig is bones with rest positions derived from [`Proportions`]; a [`Pose`]
//! is joint rotations relative to rest. Motion is stored as rotations only, so
//! one animation or IK result is valid on every build; a stored position would
//! put one class's hand through another's knee.
//!
//! A pose names only the joints it moves, so layering is composition.
//!
//! Plain math over `bevy_math`: builds for wasm as-is.

use std::sync::OnceLock;

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::class::TALLEST_CLASS_HEIGHT;

/// The bone names a humanoid rig publishes. A misspelled name in a [`Pose`]
/// silently never moves.
pub mod bone {
    pub const PELVIS: &str = "pelvis";
    pub const CHEST: &str = "chest";
    pub const NECK: &str = "neck";
    pub const HEAD: &str = "head";

    pub const CLAVICLE_L: &str = "clavicle.l";
    pub const UPPER_ARM_L: &str = "upper_arm.l";
    pub const FOREARM_L: &str = "forearm.l";
    pub const HAND_L: &str = "hand.l";
    pub const THIGH_L: &str = "thigh.l";
    pub const SHIN_L: &str = "shin.l";
    pub const FOOT_L: &str = "foot.l";

    pub const CLAVICLE_R: &str = "clavicle.r";
    pub const UPPER_ARM_R: &str = "upper_arm.r";
    pub const FOREARM_R: &str = "forearm.r";
    pub const HAND_R: &str = "hand.r";
    pub const THIGH_R: &str = "thigh.r";
    pub const SHIN_R: &str = "shin.r";
    pub const FOOT_R: &str = "foot.r";
}

/// Which side of the body a bone is on, read off its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Center,
}

impl Side {
    pub fn of(bone_name: &str) -> Side {
        if bone_name.ends_with(".l") {
            Side::Left
        } else if bone_name.ends_with(".r") {
            Side::Right
        } else {
            Side::Center
        }
    }
}

/// The numbers a body's shape is built from, as fractions of `height` so that
/// changing `height` alone scales the body.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proportions {
    /// Ground to crown, in meters.
    pub height: f32,
    /// Ground to the hip joint.
    pub hip_height: f32,
    /// Hip joint to shoulder joint.
    pub torso_length: f32,
    /// Shoulder joint to wrist. Split between upper arm, forearm and hand.
    pub arm_length: f32,
    /// Jaw to crown.
    pub head_length: f32,
    /// Half the distance between the shoulder joints. A floor: a wide chest
    /// pushes them further out; see [`humanoid`].
    pub shoulder_half_width: f32,
    /// Half the distance between the two hip joints.
    pub hip_half_width: f32,
    /// Multiplier on every limb and torso cross-section.
    pub girth: f32,
    /// Extra width and depth around the middle, on top of `girth`: in full at
    /// the hips and a quarter at the chest, so girth alone need not tell fat
    /// from burly.
    pub belly: f32,
}

impl Proportions {
    /// Height comes from the class metrics so the drawn body and the collision
    /// box agree; see [`crate::common::class`].
    pub const DEFAULT: Proportions = Proportions {
        height: TALLEST_CLASS_HEIGHT,
        hip_height: 0.53,
        torso_length: 0.28,
        arm_length: 0.31,
        head_length: 0.13,
        shoulder_half_width: 0.115,
        hip_half_width: 0.055,
        girth: 1.0,
        belly: 1.0,
    };

    /// Short legs, long arms, heavy girth. A test build, not a class.
    pub const STOCKY: Proportions = Proportions {
        height: TALLEST_CLASS_HEIGHT,
        hip_height: 0.46,
        torso_length: 0.32,
        arm_length: 0.33,
        head_length: 0.145,
        shoulder_half_width: 0.155,
        hip_half_width: 0.075,
        girth: 1.6,
        belly: 1.0,
    };

    /// Small, long-legged, thin. A test build, not a class.
    pub const LANKY: Proportions = Proportions {
        height: TALLEST_CLASS_HEIGHT * 0.85,
        hip_height: 0.57,
        torso_length: 0.26,
        arm_length: 0.31,
        head_length: 0.12,
        shoulder_half_width: 0.10,
        hip_half_width: 0.048,
        girth: 0.75,
        belly: 1.0,
    };

    /// The hip joint's height in meters: the unit of [`Pose::root_offset`].
    pub fn hip_meters(&self) -> f32 {
        self.height * self.hip_height
    }

    /// `arm_length` in meters: the unit of `HoldSpec::carry` and
    /// `Unit::ArmLengths`.
    pub fn arm_meters(&self) -> f32 {
        self.height * self.arm_length
    }
}

impl Default for Proportions {
    fn default() -> Self {
        Proportions::DEFAULT
    }
}

/// One bone, stored relative to its parent. Local `+Y` runs from `head` to
/// tail, which is at local `(0, length, 0)`.
#[derive(Clone, Debug)]
pub struct Bone {
    pub name: &'static str,
    /// Index into [`Skeleton::bones`], always less than this bone's own, so
    /// forward kinematics is one pass.
    pub parent: Option<usize>,
    /// In the parent's frame.
    pub head: Vec3,
    /// Rest orientation in the parent's frame. A pose's rotation is applied
    /// after this, so an empty pose is rest.
    pub rest: Quat,
    pub length: f32,
    /// Full width on the bone's local X and Z.
    pub thickness: Vec2,
}

/// A rig: bones in dependency order, plus the numbers they were built from.
#[derive(Component, Clone, Debug)]
pub struct Skeleton {
    bones: Vec<Bone>,
    proportions: Proportions,
}

impl Skeleton {
    /// Parents first.
    pub fn bones(&self) -> &[Bone] {
        &self.bones
    }

    /// Also the index into [`Skeleton::posed_bones`].
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|bone| bone.name == name)
    }

    pub fn proportions(&self) -> Proportions {
        self.proportions
    }

    /// Forward kinematics into world space. `root` places the feet, the same
    /// end a `SpawnPoint` marks. For one or two bones use
    /// [`Skeleton::posed_bone`]; to visit them without a list,
    /// [`Skeleton::each_posed_bone`].
    pub fn posed_bones(&self, pose: &Pose, root: &Transform) -> Vec<PosedBone> {
        let mut posed = Vec::with_capacity(self.bones.len());
        self.each_posed_bone(pose, root, |bone| posed.push(bone));
        posed
    }

    /// Every bone placed, handed over one at a time, parents first — the same
    /// values [`Skeleton::posed_bones`] collects, without the list.
    ///
    /// The parents' frames are kept on the stack for any rig that fits, which
    /// every rig built here does. This is what `posed_bones` is written on,
    /// so the two cannot disagree.
    pub fn each_posed_bone(&self, pose: &Pose, root: &Transform, mut visit: impl FnMut(PosedBone)) {
        let root = self.offset_root(pose, root);
        let mut globals = Scratch::<Transform, BONES_ON_THE_STACK>::new(self.bones.len(), Transform::IDENTITY);

        for (index, bone) in self.bones.iter().enumerate() {
            let parent = match bone.parent {
                Some(parent) => globals[parent],
                None => root,
            };
            let global = parent.mul_transform(self.local(pose, bone));
            visit(Self::place(bone, &global));
            globals[index] = global;
        }
    }

    /// One bone placed, exactly as it comes out of [`Skeleton::posed_bones`]
    /// at the same index.
    ///
    /// Walks that bone's own ancestors and nothing else, so reading a hand or
    /// a head costs four or five transforms rather than the whole rig. The
    /// products are taken root-first, in the same order the full pass takes
    /// them, which is what makes the answer identical rather than merely
    /// close: floating-point multiplication does not reassociate.
    ///
    /// Panics on an index the rig does not have, as indexing the list would.
    pub fn posed_bone(&self, pose: &Pose, root: &Transform, index: usize) -> PosedBone {
        let root = self.offset_root(pose, root);
        Self::place(&self.bones[index], &self.global(pose, &root, index))
    }

    /// A bone's world frame, recursing to the root first so the products are
    /// taken in the order [`Skeleton::each_posed_bone`] takes them. Bounded
    /// by the rig's depth, which is a handful.
    fn global(&self, pose: &Pose, root: &Transform, index: usize) -> Transform {
        let bone = &self.bones[index];
        let parent = match bone.parent {
            Some(parent) => self.global(pose, root, parent),
            None => *root,
        };
        parent.mul_transform(self.local(pose, bone))
    }

    /// The root the bones hang from, moved by the pose's root offset.
    fn offset_root(&self, pose: &Pose, root: &Transform) -> Transform {
        let mut root = *root;
        // The offset is in hip-heights, so a crouch authored on one body is
        // the same crouch on a taller one. Applied in the root's own frame so
        // it turns with the body.
        root.translation += root.rotation * (pose.root_offset * self.proportions.hip_meters());
        root
    }

    fn local(&self, pose: &Pose, bone: &Bone) -> Transform {
        Transform {
            translation: bone.head,
            rotation: bone.rest * pose.joint(bone.name),
            scale: Vec3::ONE,
        }
    }

    fn place(bone: &Bone, global: &Transform) -> PosedBone {
        PosedBone {
            name: bone.name,
            head: global.translation,
            tail: global.translation + global.rotation * (Vec3::Y * bone.length),
            rotation: global.rotation,
            length: bone.length,
            thickness: bone.thickness,
        }
    }
}

/// Ankle height as a fraction of height, the same on every build.
const ANKLE_HEIGHT: f32 = 0.04;

/// [`Proportions::DEFAULT`]'s rig, built once.
pub fn default_humanoid() -> &'static Skeleton {
    static RIG: OnceLock<Skeleton> = OnceLock::new();
    RIG.get_or_init(|| humanoid(Proportions::DEFAULT))
}

/// Bones a per-body working list keeps on the stack. Every rig built here
/// fits; one that does not spills to the heap rather than failing.
pub(crate) const BONES_ON_THE_STACK: usize = 32;

/// A working list as long as a rig, on the stack for any rig that fits and on
/// the heap for one that does not.
///
/// Forward kinematics wants each parent's frame, and the ragdoll solver wants
/// a few lists per tick — where each point started, which are touching a
/// wall, each bone's frame. As `Vec`s those were allocations per body per
/// frame, and a rocket into a crowd is dozens of bodies at once. One type for
/// all of them, so the allowance and the fallback are said once.
pub(crate) struct Scratch<T, const N: usize> {
    inline: [T; N],
    spilled: Vec<T>,
    len: usize,
}

impl<T: Copy, const N: usize> Scratch<T, N> {
    pub(crate) fn new(len: usize, fill: T) -> Self {
        Scratch {
            inline: [fill; N],
            // `Vec::new` does not allocate, so a rig that fits pays nothing
            // for the fallback.
            spilled: if len > N { vec![fill; len] } else { Vec::new() },
            len,
        }
    }
}

impl<T, const N: usize> std::ops::Deref for Scratch<T, N> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        if self.len <= N { &self.inline[..self.len] } else { &self.spilled }
    }
}

impl<T, const N: usize> std::ops::DerefMut for Scratch<T, N> {
    fn deref_mut(&mut self) -> &mut [T] {
        if self.len <= N { &mut self.inline[..self.len] } else { &mut self.spilled }
    }
}

/// A bone after forward kinematics, in world space.
#[derive(Clone, Copy, Debug)]
pub struct PosedBone {
    pub name: &'static str,
    pub head: Vec3,
    pub tail: Vec3,
    /// `+Y` head to tail. Carries the roll the endpoints cannot.
    pub rotation: Quat,
    pub length: f32,
    pub thickness: Vec2,
}

impl PosedBone {
    pub fn prism(&self) -> Transform {
        Transform {
            translation: self.head + self.rotation * (Vec3::Y * self.length * 0.5),
            rotation: self.rotation,
            scale: Vec3::new(self.thickness.x, self.length, self.thickness.y),
        }
    }
}

/// Joint rotations away from rest. Sparse: a pose names only the joints it
/// moves, and is valid on any skeleton with those bones.
#[derive(Component, Clone, Debug, Default)]
pub struct Pose {
    /// The body's offset from standing, in hip-heights so it means the same on
    /// every build.
    pub root_offset: Vec3,
    joints: HashMap<&'static str, Quat>,
}

impl Pose {
    pub fn rest() -> Pose {
        Pose::default()
    }

    /// Identity for a bone this pose does not name.
    pub fn joint(&self, name: &str) -> Quat {
        self.joints.get(name).copied().unwrap_or(Quat::IDENTITY)
    }

    pub fn set(&mut self, name: &'static str, rotation: Quat) {
        self.joints.insert(name, rotation);
    }

    /// Back to [`Pose::rest`], keeping the joints' storage for reuse.
    pub fn reset_to_rest(&mut self) {
        self.root_offset = Vec3::ZERO;
        self.joints.clear();
    }

    /// A joint only one side names blends toward rest. Slerp takes the short
    /// way, since a long way round bends a knee backwards in transit.
    pub fn lerp(from: &Pose, to: &Pose, t: f32) -> Pose {
        let t = t.clamp(0.0, 1.0);
        let mut blended = Pose {
            root_offset: from.root_offset.lerp(to.root_offset, t),
            joints: HashMap::with_capacity(from.joints.len().max(to.joints.len())),
        };

        // Each joint once: a joint both sides name is blended on the first
        // pass and skipped on the second, rather than slerped twice.
        for name in from.joints.keys() {
            blended
                .joints
                .insert(*name, from.joint(name).slerp(to.joint(name), t));
        }
        for name in to.joints.keys().filter(|name| !from.joints.contains_key(*name)) {
            blended
                .joints
                .insert(*name, Quat::IDENTITY.slerp(to.joint(name), t));
        }

        blended
    }

    pub fn with(mut self, name: &'static str, rotation: Quat) -> Pose {
        self.set(name, rotation);
        self
    }

    pub fn posed_joints(&self) -> impl Iterator<Item = (&'static str, Quat)> + '_ {
        self.joints.iter().map(|(name, rotation)| (*name, *rotation))
    }
}

/// Build a humanoid in an A-pose (arms 45° down), which is nearer the middle
/// of a shoulder's range than a T.
///
/// Feet on `y = 0` at the origin, facing `-Z` (forward for
/// [`crate::game::player`]); `+X` is the character's right.
pub fn humanoid(proportions: Proportions) -> Skeleton {
    let p = proportions;
    let h = p.height;
    let girth = p.girth;
    let belly = girth * p.belly;
    let chest_belly = girth * (1.0 + (p.belly - 1.0) * 0.25);

    // Landmarks in rest-space meters.
    let hip_y = h * p.hip_height;
    let shoulder_y = hip_y + h * p.torso_length;
    let head_base_y = h - h * p.head_length;
    let spine_mid_y = hip_y + (shoulder_y - hip_y) * 0.45;

    let chest_thickness = Vec2::new(0.24, 0.15) * chest_belly * h;

    // The chest scales with girth and belly but `shoulder_half_width` does not,
    // so on a wide build the shoulder is pushed to the edge of the ribcage
    // rather than starting inside it. The clavicle absorbs the difference.
    let shoulder_x = (h * p.shoulder_half_width).max(chest_thickness.x * 0.5);
    let hip_x = h * p.hip_half_width;

    let arm_dir = Vec3::new(std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2, 0.0);
    let arm = h * p.arm_length;
    let (upper_arm, forearm, hand) = (arm * 0.45, arm * 0.40, arm * 0.28);

    // Slightly splayed so IK's knee bend direction is not degenerate.
    let leg_dir = Vec3::new(0.03, -1.0, 0.0).normalize();
    let leg = (hip_y - h * ANKLE_HEIGHT) / -leg_dir.y;
    let (thigh, shin) = (leg * 0.5, leg * 0.5);
    let foot_length = h * 0.14;

    let mut builder = Builder::default();

    builder.bone(bone::PELVIS, None, Vec3::new(0.0, hip_y, 0.0), Vec3::new(0.0, spine_mid_y, 0.0), Vec2::new(0.20, 0.14) * belly * h);
    builder.bone(bone::CHEST, Some(bone::PELVIS), Vec3::new(0.0, spine_mid_y, 0.0), Vec3::new(0.0, shoulder_y, 0.0), chest_thickness);
    builder.bone(bone::NECK, Some(bone::CHEST), Vec3::new(0.0, shoulder_y, 0.0), Vec3::new(0.0, head_base_y, 0.0), Vec2::new(0.07, 0.07) * girth * h);
    // The head does not scale with girth.
    builder.bone(bone::HEAD, Some(bone::NECK), Vec3::new(0.0, head_base_y, 0.0), Vec3::new(0.0, h, 0.0), Vec2::new(0.13, 0.15) * h);

    // `sign` is +1 on the character's right.
    for (sign, names) in [
        (-1.0, LimbNames::LEFT),
        (1.0, LimbNames::RIGHT),
    ] {
        let mirror = Vec3::new(sign, 1.0, 1.0);

        let shoulder = Vec3::new(shoulder_x, shoulder_y - h * 0.01, 0.0) * mirror;
        builder.bone(names.clavicle, Some(bone::CHEST), Vec3::new(0.0, shoulder_y, 0.0), shoulder, Vec2::new(0.05, 0.05) * girth * h);

        let elbow = shoulder + arm_dir * mirror * upper_arm;
        let wrist = elbow + arm_dir * mirror * forearm;
        let finger = wrist + arm_dir * mirror * hand;
        builder.bone(names.upper_arm, Some(names.clavicle), shoulder, elbow, Vec2::new(0.075, 0.075) * girth * h);
        builder.bone(names.forearm, Some(names.upper_arm), elbow, wrist, Vec2::new(0.065, 0.065) * girth * h);
        builder.bone(names.hand, Some(names.forearm), wrist, finger, Vec2::new(0.045, 0.08) * girth * h);

        let hip = Vec3::new(hip_x, hip_y, 0.0) * mirror;
        let knee = hip + leg_dir * mirror * thigh;
        let ankle = knee + leg_dir * mirror * shin;
        let toe = ankle + Vec3::new(0.0, 0.0, -foot_length);
        builder.bone(names.thigh, Some(bone::PELVIS), hip, knee, Vec2::new(0.11, 0.11) * girth * h);
        builder.bone(names.shin, Some(names.thigh), knee, ankle, Vec2::new(0.09, 0.09) * girth * h);
        builder.bone(names.foot, Some(names.shin), ankle, toe, Vec2::new(0.09, 0.06) * girth * h);
    }

    Skeleton { bones: builder.bones, proportions }
}

/// A limb's bone names per side. Names are `&'static str`, so they cannot be
/// built by appending `.l` at runtime.
struct LimbNames {
    clavicle: &'static str,
    upper_arm: &'static str,
    forearm: &'static str,
    hand: &'static str,
    thigh: &'static str,
    shin: &'static str,
    foot: &'static str,
}

impl LimbNames {
    const LEFT: LimbNames = LimbNames {
        clavicle: bone::CLAVICLE_L,
        upper_arm: bone::UPPER_ARM_L,
        forearm: bone::FOREARM_L,
        hand: bone::HAND_L,
        thigh: bone::THIGH_L,
        shin: bone::SHIN_L,
        foot: bone::FOOT_L,
    };

    const RIGHT: LimbNames = LimbNames {
        clavicle: bone::CLAVICLE_R,
        upper_arm: bone::UPPER_ARM_R,
        forearm: bone::FOREARM_R,
        hand: bone::HAND_R,
        thigh: bone::THIGH_R,
        shin: bone::SHIN_R,
        foot: bone::FOOT_R,
    };
}

/// Turns world-space landmarks into parent-relative bones.
#[derive(Default)]
struct Builder {
    bones: Vec<Bone>,
    /// Each bone's rest orientation and head position in world rest space.
    rest: Vec<(Quat, Vec3)>,
}

impl Builder {
    fn bone(&mut self, name: &'static str, parent: Option<&'static str>, head: Vec3, tail: Vec3, thickness: Vec2) {
        let parent = parent.map(|parent| {
            self.bones
                .iter()
                .position(|bone| bone.name == parent)
                .unwrap_or_else(|| panic!("bone {name} is attached to {parent}, which has not been added yet"))
        });

        let along = tail - head;
        let length = along.length();
        let rest_global = bone_rotation(along);

        let (local_head, local_rest) = match parent {
            Some(parent) => {
                let (parent_rotation, parent_head) = self.rest[parent];
                let inverse = parent_rotation.inverse();
                (inverse * (head - parent_head), inverse * rest_global)
            }
            None => (head, rest_global),
        };

        self.bones.push(Bone { name, parent, head: local_head, rest: local_rest, length, thickness });
        self.rest.push((rest_global, head));
    }
}

/// The orientation that points a bone's local `+Y` down its own length, with
/// local `+Z` at world forward where meaningful so mirrored bones come out
/// mirrored. `Quat::from_rotation_arc` picks an arbitrary roll for a bone
/// pointing straight down, which every leg does.
fn bone_rotation(along: Vec3) -> Quat {
    let y = along.normalize_or_zero();
    if y == Vec3::ZERO {
        return Quat::IDENTITY;
    }
    // Feet lie along forward, so they take up as reference instead.
    let reference = if y.z.abs() > 0.9 { Vec3::Y } else { Vec3::NEG_Z };
    let z = (reference - y * reference.dot(y)).normalize();
    let x = y.cross(z);
    Quat::from_mat3(&Mat3::from_cols(x, y, z))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posed(skeleton: &Skeleton, pose: &Pose) -> HashMap<&'static str, PosedBone> {
        skeleton
            .posed_bones(pose, &Transform::IDENTITY)
            .into_iter()
            .map(|bone| (bone.name, bone))
            .collect()
    }

    fn rest(proportions: Proportions) -> HashMap<&'static str, PosedBone> {
        let skeleton = humanoid(proportions);
        posed(&skeleton, &Pose::rest())
    }

    /// A floating or sunken rig disagrees with the spawn clearance check.
    #[test]
    fn a_rest_pose_stands_on_the_ground_at_its_stated_height() {
        for proportions in [Proportions::DEFAULT, Proportions::STOCKY, Proportions::LANKY] {
            let bones = rest(proportions);
            let crown = bones[bone::HEAD].tail.y;
            assert!(
                (crown - proportions.height).abs() < 1e-4,
                "crown at {crown}, height is {}",
                proportions.height
            );

            let lowest = bones.values().map(|bone| bone.head.y.min(bone.tail.y)).fold(f32::MAX, f32::min);
            assert!(lowest >= 0.0, "a bone is underground at y = {lowest}");
            assert!(lowest < proportions.height * 0.06, "the body is floating: lowest bone at y = {lowest}");
        }
    }

    /// Wide classes must not start their arms inside the chest, and slim ones
    /// must keep their stated shoulders.
    #[test]
    fn no_build_starts_its_arms_inside_its_own_chest() {
        use crate::common::class::Class;
        use strum::IntoEnumIterator;

        for class in Class::iter() {
            let proportions = crate::common::class::ClassBuilds::shipped().of(class);
            let skeleton = humanoid(proportions);
            let bones = posed(&skeleton, &Pose::rest());
            let chest_half = bones[bone::CHEST].thickness.x * 0.5;
            let shoulder = bones[bone::UPPER_ARM_R].head.x;

            assert!(
                shoulder >= chest_half - 1e-4,
                "the {:?}'s shoulder is {shoulder:.3} m out and its chest {chest_half:.3} m",
                class,
            );
            assert!(
                shoulder >= proportions.height * proportions.shoulder_half_width - 1e-4,
                "the {:?}'s shoulders were pulled in",
                class,
            );
        }
    }

    /// A T-pose rest would silently change every arm animation.
    #[test]
    fn the_rest_pose_is_an_a_pose() {
        let bones = rest(Proportions::DEFAULT);
        let shoulder = bones[bone::UPPER_ARM_R].head;
        let hand = bones[bone::HAND_R].tail;

        assert!(hand.x > shoulder.x, "the arm is not out: {} vs {}", hand.x, shoulder.x);
        assert!(hand.y < shoulder.y, "the arm is not down: {} vs {}", hand.y, shoulder.y);
        let drop = shoulder.y - hand.y;
        let out = hand.x - shoulder.x;
        assert!((drop / out - 1.0).abs() < 0.2, "the arm is at the wrong angle: out {out}, down {drop}");
    }

    /// Otherwise every animation is subtly one-sided.
    #[test]
    fn the_two_sides_mirror() {
        let bones = rest(Proportions::DEFAULT);
        for (left, right) in [
            (bone::HAND_L, bone::HAND_R),
            (bone::FOOT_L, bone::FOOT_R),
            (bone::UPPER_ARM_L, bone::UPPER_ARM_R),
        ] {
            let l = bones[left].tail;
            let r = bones[right].tail;
            assert!(
                (l - Vec3::new(-r.x, r.y, r.z)).length() < 1e-5,
                "{left} at {l} does not mirror {right} at {r}"
            );
        }
    }

    /// Otherwise the rig comes apart when rotated.
    #[test]
    fn a_chain_joins_up_when_posed() {
        let skeleton = humanoid(Proportions::STOCKY);
        let pose = Pose::rest()
            .with(bone::UPPER_ARM_R, Quat::from_rotation_x(-0.9))
            .with(bone::FOREARM_R, Quat::from_rotation_x(-1.2))
            .with(bone::CHEST, Quat::from_rotation_z(0.3));
        let bones = posed(&skeleton, &pose);

        for (parent, child) in [
            (bone::UPPER_ARM_R, bone::FOREARM_R),
            (bone::FOREARM_R, bone::HAND_R),
            (bone::THIGH_L, bone::SHIN_L),
            (bone::SHIN_L, bone::FOOT_L),
        ] {
            let joint = bones[parent].tail;
            let head = bones[child].head;
            assert!((joint - head).length() < 1e-5, "{child} came off {parent}: {head} vs {joint}");
        }
    }

    /// One pose must give the same joint angles on different builds; storing
    /// positions breaks this.
    #[test]
    fn one_pose_reads_the_same_on_bodies_of_different_proportions() {
        let pose = Pose::rest()
            .with(bone::UPPER_ARM_R, Quat::from_rotation_x(-1.1))
            .with(bone::FOREARM_R, Quat::from_rotation_x(-1.4));

        let angle_between = |bones: &HashMap<&'static str, PosedBone>| {
            let upper = (bones[bone::UPPER_ARM_R].tail - bones[bone::UPPER_ARM_R].head).normalize();
            let fore = (bones[bone::FOREARM_R].tail - bones[bone::FOREARM_R].head).normalize();
            upper.dot(fore).clamp(-1.0, 1.0).acos()
        };

        let stocky = posed(&humanoid(Proportions::STOCKY), &pose);
        let lanky = posed(&humanoid(Proportions::LANKY), &pose);

        let (a, b) = (angle_between(&stocky), angle_between(&lanky));
        assert!((a - b).abs() < 1e-4, "the same pose bent the elbow differently: {a} vs {b}");

        // The builds must actually differ.
        let stocky_hand = stocky[bone::HAND_R].tail;
        let lanky_hand = lanky[bone::HAND_R].tail;
        assert!(
            (stocky_hand - lanky_hand).length() > 0.1,
            "the two builds put the hand in the same place: {stocky_hand} and {lanky_hand}"
        );
    }

    /// In meters a crouch would drop a short build through the floor.
    #[test]
    fn a_root_offset_scales_with_the_body() {
        let mut pose = Pose::rest();
        pose.root_offset = Vec3::new(0.0, -0.25, 0.0);

        for proportions in [Proportions::DEFAULT, Proportions::LANKY] {
            let skeleton = humanoid(proportions);
            let standing = posed(&skeleton, &Pose::rest())[bone::HEAD].tail.y;
            let crouched = posed(&skeleton, &pose)[bone::HEAD].tail.y;
            let dropped = standing - crouched;
            let expected = proportions.hip_meters() * 0.25;
            assert!((dropped - expected).abs() < 1e-4, "dropped {dropped}, expected {expected}");
        }
    }

    /// An IK pass on one leg must not straighten the other.
    #[test]
    fn a_pose_leaves_unnamed_joints_alone() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let before = posed(&skeleton, &Pose::rest());
        let after = posed(&skeleton, &Pose::rest().with(bone::THIGH_R, Quat::from_rotation_x(0.7)));

        assert!((before[bone::FOOT_L].tail - after[bone::FOOT_L].tail).length() < 1e-6);
        assert!((before[bone::FOOT_R].tail - after[bone::FOOT_R].tail).length() > 0.05);
    }

    /// The root transform places the feet, as a spawn point marks them.
    #[test]
    fn the_root_transform_places_the_feet() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let feet = Vec3::new(3.0, 2.0, -4.0);
        let yaw = std::f32::consts::FRAC_PI_2;
        let root = Transform::from_translation(feet).with_rotation(Quat::from_rotation_y(yaw));

        let bones = posed(&skeleton, &Pose::rest());
        let placed: HashMap<_, _> = skeleton
            .posed_bones(&Pose::rest(), &root)
            .into_iter()
            .map(|bone| (bone.name, bone))
            .collect();

        let crown = placed[bone::HEAD].tail;
        assert!((crown - (feet + Vec3::Y * Proportions::DEFAULT.height)).length() < 1e-4, "crown at {crown}");

        let hand = placed[bone::HAND_R].tail - feet;
        let unturned = bones[bone::HAND_R].tail;
        assert!((hand - Quat::from_rotation_y(yaw) * unturned).length() < 1e-4);
    }

    /// Reading one bone is the full pass's answer for that bone, to the bit.
    /// Hitboxes, IK and the hold all read single bones now, so "close" would
    /// be a body whose head box or planted foot depends on which function
    /// asked. Every bone, every build, through poses that bend every joint,
    /// with roots that move, turn, scale and carry a root offset.
    #[test]
    fn one_bone_is_placed_exactly_as_the_whole_rig_places_it() {
        let bent = |seed: f32| {
            let mut pose = Pose::rest();
            for (n, bone) in humanoid(Proportions::DEFAULT).bones().iter().enumerate() {
                let k = seed + n as f32 * 0.37;
                pose.set(bone.name, Quat::from_euler(EulerRot::YXZ, k.sin() * 0.6, (k * 1.3).cos() * 0.8, (k * 0.7).sin() * 0.4));
            }
            pose.root_offset = Vec3::new(0.05 * seed.sin(), -0.3 * seed.cos().abs(), 0.1);
            pose
        };
        let poses = [Pose::rest(), bent(0.5), bent(1.9), bent(-3.2)];
        let roots = [
            Transform::IDENTITY,
            Transform::from_xyz(3.0, -1.5, 7.25).with_rotation(Quat::from_rotation_y(2.1)),
            Transform::from_xyz(-0.4, 0.2, 1.0)
                .with_rotation(Quat::from_euler(EulerRot::XYZ, 0.3, -1.2, 0.8))
                .with_scale(Vec3::new(1.1, 0.9, 1.3)),
        ];

        for proportions in [Proportions::DEFAULT, Proportions::STOCKY, Proportions::LANKY] {
            let skeleton = humanoid(proportions);
            for pose in &poses {
                for root in &roots {
                    let all = skeleton.posed_bones(pose, root);
                    let mut visited = 0;
                    skeleton.each_posed_bone(pose, root, |posed| {
                        assert_eq!(posed.tail, all[visited].tail);
                        visited += 1;
                    });
                    assert_eq!(visited, all.len());

                    for (index, whole) in all.iter().enumerate() {
                        let one = skeleton.posed_bone(pose, root, index);
                        assert_eq!(one.name, whole.name);
                        assert_eq!(one.head, whole.head, "{} head", whole.name);
                        assert_eq!(one.tail, whole.tail, "{} tail", whole.name);
                        assert_eq!(one.rotation, whole.rotation, "{} rotation", whole.name);
                    }
                }
            }
        }
    }
}
