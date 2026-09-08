//! Skeletons built in code, not imported.
//!
//! Nothing here loads a file. A rig is a list of bones with rest positions
//! derived from a handful of [`Proportions`] numbers, and a [`Pose`] is a set
//! of joint rotations *relative to that rest pose*. The split is the point:
//!
//! - **Proportions live in the skeleton.** Bone lengths, widths and the width
//!   of the shoulders are all read from one struct, so a heavy and a scout are
//!   the same call with different numbers rather than two authored models.
//! - **Motion lives in the pose, as rotations only.** A rotation says "the
//!   elbow is bent this far", which is true whatever the forearm's length is.
//!   Store a *position* instead and the same animation puts one class's hand
//!   at its hip and another's through its own knee.
//!
//! So an animation authored once applies to every silhouette, which is what
//! TF2-style classes need, and it is also what makes procedural motion and IK
//! tractable: a solver only ever writes joint rotations, and the same solver
//! output is valid for every body it is run on.
//!
//! A pose names the joints it moves and says nothing about the rest, so
//! layering (a walk cycle underneath, a look-at on the neck, an IK pass on one
//! leg) is composition rather than a merge of two full skeletons.
//!
//! Everything in here is plain maths over `bevy_math` types: no assets, no
//! file system, no threads. It builds for wasm as-is.

use std::sync::OnceLock;

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::common::class::TALLEST_CLASS_HEIGHT;

/// The bone names a humanoid rig publishes.
///
/// Constants rather than string literals at each call site, because a
/// misspelled name in a [`Pose`] is silently a joint that never moves — there
/// is nothing to fail, the bone just stays at rest.
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
///
/// Only used to colour the gizmos so far, but a mirror for animation — play a
/// one-sided clip on the other arm — wants the same question answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Centre,
}

impl Side {
    pub fn of(bone_name: &str) -> Side {
        if bone_name.ends_with(".l") {
            Side::Left
        } else if bone_name.ends_with(".r") {
            Side::Right
        } else {
            Side::Centre
        }
    }
}

/// The numbers a body's shape is built from, all as fractions of its height.
///
/// Fractions rather than metres so that changing `height` alone gives a
/// proportional body, and changing one fraction changes exactly one thing:
/// a class with stubby legs is `hip_height` down and `torso_length` up, not a
/// re-authored skeleton.
///
/// The rig is derived from these, so anything not listed — where the knee
/// sits, how long the neck is — is computed rather than stored, and cannot
/// contradict them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Proportions {
    /// Ground to the crown of the head, in metres. Everything else is a
    /// fraction of this.
    pub height: f32,
    /// Ground to the hip joint.
    pub hip_height: f32,
    /// Hip joint to shoulder joint.
    pub torso_length: f32,
    /// Shoulder joint to wrist. Split between upper arm, forearm and hand.
    pub arm_length: f32,
    /// The head bone itself — jaw to crown.
    pub head_length: f32,
    /// Half the distance between the two shoulder joints.
    pub shoulder_half_width: f32,
    /// Half the distance between the two hip joints.
    pub hip_half_width: f32,
    /// Multiplier on every limb and torso cross-section. Girth is the whole
    /// difference between two silhouettes that stand the same height.
    pub girth: f32,
}

impl Proportions {
    /// The body the game currently moves: [`TALLEST_CLASS_HEIGHT`] tall, and
    /// average in every other respect.
    ///
    /// Height comes from the class metrics rather than a number typed here, so
    /// the drawn body and the box that collides stay the same size. See
    /// [`crate::common::class`] for why that has one home.
    pub const DEFAULT: Proportions = Proportions {
        height: TALLEST_CLASS_HEIGHT,
        hip_height: 0.53,
        torso_length: 0.28,
        arm_length: 0.31,
        head_length: 0.13,
        shoulder_half_width: 0.115,
        hip_half_width: 0.055,
        girth: 1.0,
    };

    /// Short legs, long arms, twice the girth — the silhouette test. Not a
    /// class; classes do not exist yet.
    pub const STOCKY: Proportions = Proportions {
        height: TALLEST_CLASS_HEIGHT,
        hip_height: 0.46,
        torso_length: 0.32,
        arm_length: 0.33,
        head_length: 0.145,
        shoulder_half_width: 0.155,
        hip_half_width: 0.075,
        girth: 1.6,
    };

    /// The other end of it: small, long-legged, thin.
    pub const LANKY: Proportions = Proportions {
        height: TALLEST_CLASS_HEIGHT * 0.85,
        hip_height: 0.57,
        torso_length: 0.26,
        arm_length: 0.31,
        head_length: 0.12,
        shoulder_half_width: 0.10,
        hip_half_width: 0.048,
        girth: 0.75,
    };

    /// The hip joint's height in metres — the length a pose's root offset is
    /// measured in. See [`Pose::root_offset`].
    pub fn hip_metres(&self) -> f32 {
        self.height * self.hip_height
    }
}

impl Default for Proportions {
    fn default() -> Self {
        Proportions::DEFAULT
    }
}

/// One bone, stored relative to its parent.
///
/// **Local `+Y` runs down the bone**, from `head` to tail, which is why a
/// rotation applied to a bone is about a joint and not about some arbitrary
/// origin. The tail is at local `(0, length, 0)` and is where children attach
/// unless they say otherwise.
#[derive(Clone, Debug)]
pub struct Bone {
    pub name: &'static str,
    /// Index into [`Skeleton::bones`]. Always less than this bone's own index:
    /// the list is in dependency order so forward kinematics is one pass.
    pub parent: Option<usize>,
    /// Where this bone's head sits in the parent's frame.
    pub head: Vec3,
    /// Rest orientation in the parent's frame. A pose's rotation for this bone
    /// is applied *after* this, so an empty pose is the rest pose.
    pub rest: Quat,
    pub length: f32,
    /// Full width of the drawn prism on the bone's local X and Z.
    pub thickness: Vec2,
}

/// A rig: bones in dependency order, plus the numbers they were built from.
#[derive(Component, Clone, Debug)]
pub struct Skeleton {
    bones: Vec<Bone>,
    proportions: Proportions,
}

impl Skeleton {
    /// The numbers this rig was built from. Anything that needs a body's
    /// height — how tall a display stands, where its head is — asks the rig
    /// rather than reaching for a constant of its own.
    pub fn proportions(&self) -> Proportions {
        self.proportions
    }

    /// Forward kinematics: every bone placed in world space.
    ///
    /// `root` is where the skeleton's origin sits, and the origin is **the
    /// feet**, standing on the ground plane — the same end a `SpawnPoint`
    /// marks, so a body can be put on a spawn without converting anything.
    ///
    /// One pass, because bones are stored parents-first.
    pub fn posed_bones(&self, pose: &Pose, root: &Transform) -> Vec<PosedBone> {
        let root = {
            let mut root = *root;
            // The offset is in hip-heights, so a crouch authored on one body
            // is the same crouch on a taller one. Applied in the root's own
            // frame so it turns with the body.
            root.translation += root.rotation * (pose.root_offset * self.proportions.hip_metres());
            root
        };

        let mut globals: Vec<Transform> = Vec::with_capacity(self.bones.len());
        let mut posed: Vec<PosedBone> = Vec::with_capacity(self.bones.len());

        for bone in &self.bones {
            let local = Transform {
                translation: bone.head,
                rotation: bone.rest * pose.joint(bone.name),
                scale: Vec3::ONE,
            };
            let global = match bone.parent {
                Some(parent) => globals[parent].mul_transform(local),
                None => root.mul_transform(local),
            };

            posed.push(PosedBone {
                name: bone.name,
                head: global.translation,
                tail: global.translation + global.rotation * (Vec3::Y * bone.length),
                rotation: global.rotation,
                length: bone.length,
                thickness: bone.thickness,
            });
            globals.push(global);
        }

        posed
    }
}

/// How high the ankle sits, as a fraction of height.
///
/// Not a [`Proportions`] field because it is the same on every build: an ankle
/// is at the bottom of a leg wherever that leg starts.
const ANKLE_HEIGHT: f32 = 0.04;

/// How long a leg is, as a fraction of the hip's height off the ground.
///
/// The one number an animation needs to know about the rig it is playing on,
/// and the reason it can be a constant: a leg spans hip to ankle, so its length
/// is fixed relative to the hip once [`ANKLE_HEIGHT`] is. That is what lets a
/// pose lower the hips by a third of a leg — planting the feet by bending the
/// knees exactly enough — without knowing whose legs they are.
///
/// Exact for the default build's hip height and within a fraction of a percent
/// for the others, which is a tenth of a millimetre at the floor. See
/// `feet_stay_planted_through_the_whole_idle_cycle`.
pub const LEG_SPAN_PER_HIP_HEIGHT: f32 =
    1.0 - ANKLE_HEIGHT / Proportions::DEFAULT.hip_height;

/// The one humanoid rig, built once.
///
/// Classes do not exist yet, so every body on the map is the same shape and
/// rebuilding it per frame per gizmo would be pure waste. When classes arrive
/// this becomes a lookup rather than a constant.
pub fn default_humanoid() -> &'static Skeleton {
    static RIG: OnceLock<Skeleton> = OnceLock::new();
    RIG.get_or_init(|| humanoid(Proportions::DEFAULT))
}

/// A bone after forward kinematics, in world space.
#[derive(Clone, Copy, Debug)]
pub struct PosedBone {
    pub name: &'static str,
    pub head: Vec3,
    pub tail: Vec3,
    /// The bone's orientation, with `+Y` running head to tail. Kept as well as
    /// the two endpoints because the roll about the bone is real information —
    /// a forearm can twist without either end moving.
    pub rotation: Quat,
    pub length: f32,
    pub thickness: Vec2,
}

impl PosedBone {
    /// Where the middle of the drawn prism goes, and how it is turned.
    pub fn prism(&self) -> Transform {
        Transform {
            translation: self.head + self.rotation * (Vec3::Y * self.length * 0.5),
            rotation: self.rotation,
            scale: Vec3::new(self.thickness.x, self.length, self.thickness.y),
        }
    }
}

/// What the joints are doing, as rotations away from rest.
///
/// Sparse on purpose: a pose names only the joints it has an opinion about.
/// That is what lets a walk cycle, a look-at and an IK pass be three separate
/// things applied to the same body rather than one monolithic pose that has to
/// state a value for every bone. It is also why a pose carries no lengths and
/// no positions, and so is valid on any skeleton that has the bones it names —
/// retargeting between two builds is not a conversion step, it is just
/// applying the same pose to the other rig.
#[derive(Component, Clone, Debug, Default)]
pub struct Pose {
    /// Where the whole body sits relative to its normal standing position,
    /// **in hip-heights** rather than metres.
    ///
    /// The one thing a pose says that is not a rotation, and it has to be
    /// there: a crouch or a jump-tuck lowers the root. Measured in hip-heights
    /// so "drop by a third of a leg" means the same thing on every build,
    /// which is exactly what it would not mean in metres.
    pub root_offset: Vec3,
    joints: HashMap<&'static str, Quat>,
}

impl Pose {
    /// The rest pose — every joint at whatever the skeleton says.
    pub fn rest() -> Pose {
        Pose::default()
    }

    /// The rotation for a bone, or identity if this pose says nothing about it.
    pub fn joint(&self, name: &str) -> Quat {
        self.joints.get(name).copied().unwrap_or(Quat::IDENTITY)
    }

    pub fn set(&mut self, name: &'static str, rotation: Quat) {
        self.joints.insert(name, rotation);
    }

    pub fn with(mut self, name: &'static str, rotation: Quat) -> Pose {
        self.set(name, rotation);
        self
    }

    /// The bones this pose has an opinion about.
    pub fn posed_joints(&self) -> impl Iterator<Item = (&'static str, Quat)> + '_ {
        self.joints.iter().map(|(name, rotation)| (*name, *rotation))
    }
}

/// Build a humanoid in an A-pose from a set of proportions.
///
/// The A-pose — arms out and down at about 45° — rather than a T-pose, because
/// it is closer to the middle of a shoulder's range: every arm animation is
/// then a smaller rotation away from rest, and the shoulder deformation a T
/// bakes in never has to be undone.
///
/// The body stands with its feet on `y = 0` at the origin, facing `-Z`, which
/// is the direction [`crate::game::player`] treats as forward. `+X` is the
/// character's own right.
pub fn humanoid(proportions: Proportions) -> Skeleton {
    let p = proportions;
    let h = p.height;
    let girth = p.girth;

    // Every landmark in rest-space metres, so the layout below reads as a
    // description of a body rather than a chain of offsets.
    let hip_y = h * p.hip_height;
    let shoulder_y = hip_y + h * p.torso_length;
    let head_base_y = h - h * p.head_length;
    // The spine is two bones so the chest can lean without the hips going with
    // it. The break is low, where a real spine actually bends.
    let spine_mid_y = hip_y + (shoulder_y - hip_y) * 0.45;

    let shoulder_x = h * p.shoulder_half_width;
    let hip_x = h * p.hip_half_width;

    // Down and out at 45°, both arms.
    let arm_dir = Vec3::new(std::f32::consts::FRAC_1_SQRT_2, -std::f32::consts::FRAC_1_SQRT_2, 0.0);
    let arm = h * p.arm_length;
    let (upper_arm, forearm, hand) = (arm * 0.45, arm * 0.40, arm * 0.28);

    // Legs stand very slightly apart, so the knee has a bend direction that is
    // not exactly degenerate when an IK solver goes looking for one.
    let leg_dir = Vec3::new(0.03, -1.0, 0.0).normalize();
    let leg = (hip_y - h * ANKLE_HEIGHT) / -leg_dir.y;
    let (thigh, shin) = (leg * 0.5, leg * 0.5);
    let foot_length = h * 0.14;

    let mut builder = Builder::default();

    // Spine, bottom up. The pelvis is the root: it is the bone the whole body
    // hangs off, and the one a root offset moves.
    builder.bone(bone::PELVIS, None, Vec3::new(0.0, hip_y, 0.0), Vec3::new(0.0, spine_mid_y, 0.0), Vec2::new(0.20, 0.14) * girth * h);
    builder.bone(bone::CHEST, Some(bone::PELVIS), Vec3::new(0.0, spine_mid_y, 0.0), Vec3::new(0.0, shoulder_y, 0.0), Vec2::new(0.24, 0.15) * girth * h);
    builder.bone(bone::NECK, Some(bone::CHEST), Vec3::new(0.0, shoulder_y, 0.0), Vec3::new(0.0, head_base_y, 0.0), Vec2::new(0.07, 0.07) * girth * h);
    // The head keeps its own size: a heavier build is not a bigger skull, and
    // a head that grew with girth would read as a different character rather
    // than the same one heavier.
    builder.bone(bone::HEAD, Some(bone::NECK), Vec3::new(0.0, head_base_y, 0.0), Vec3::new(0.0, h, 0.0), Vec2::new(0.13, 0.15) * h);

    // Both limbs, mirrored. `sign` is +1 on the character's right.
    for (sign, names) in [
        (-1.0, LimbNames::LEFT),
        (1.0, LimbNames::RIGHT),
    ] {
        let mirror = Vec3::new(sign, 1.0, 1.0);

        // The clavicle runs from the base of the neck out to the shoulder
        // joint, so a shrug is a rotation rather than a moved arm.
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
        // Straight off the pelvis rather than through a stub: the hip joint is
        // an offset from the pelvis, not a bone of its own, and the builder
        // takes a head position rather than assuming the parent's tail.
        builder.bone(names.thigh, Some(bone::PELVIS), hip, knee, Vec2::new(0.11, 0.11) * girth * h);
        builder.bone(names.shin, Some(names.thigh), knee, ankle, Vec2::new(0.09, 0.09) * girth * h);
        builder.bone(names.foot, Some(names.shin), ankle, toe, Vec2::new(0.09, 0.06) * girth * h);
    }

    Skeleton { bones: builder.bones, proportions }
}

/// The one place a limb's four bone names are listed per side.
///
/// Names are `&'static str`, so they cannot be built by appending `.l` at
/// runtime; a table beats writing the limb out twice.
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

/// Turns a body described in world-space landmarks into parent-relative bones.
///
/// Authoring a rig as "the elbow is here, the wrist is there" is the readable
/// way to write one down; storing it that way would be the wrong thing
/// entirely, because a chain of absolute positions does not bend. This does
/// the conversion once, at build time.
#[derive(Default)]
struct Builder {
    bones: Vec<Bone>,
    /// Each bone's rest orientation and head position in world rest space —
    /// what the next bone needs to express itself relative to its parent.
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

/// The orientation that points a bone's local `+Y` down its own length.
///
/// Written out rather than using `Quat::from_rotation_arc`, which picks an
/// arbitrary axis when the bone points straight down — every leg — and so
/// would roll the left and right legs differently for no reason. Here the
/// bone's local `+Z` is world forward wherever that is meaningful, so two
/// mirrored bones come out mirrored.
fn bone_rotation(along: Vec3) -> Quat {
    let y = along.normalize_or_zero();
    if y == Vec3::ZERO {
        return Quat::IDENTITY;
    }
    // A bone lying along the reference axis has no forward to speak of; feet
    // are the case that matters, and up is the right answer for them.
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

    /// The rig has to stand on the floor and be as tall as it says it is: the
    /// spawn-point clearance check and the drawn body are both measured from
    /// the feet, and a skeleton that floats or sinks makes them disagree.
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

    /// An A-pose, not a T-pose: the hands are outside the shoulders *and*
    /// below them. A rig that came out in a T would silently change what every
    /// arm animation means, since a pose is measured from rest.
    #[test]
    fn the_rest_pose_is_an_a_pose() {
        let bones = rest(Proportions::DEFAULT);
        let shoulder = bones[bone::UPPER_ARM_R].head;
        let hand = bones[bone::HAND_R].tail;

        assert!(hand.x > shoulder.x, "the arm is not out: {} vs {}", hand.x, shoulder.x);
        assert!(hand.y < shoulder.y, "the arm is not down: {} vs {}", hand.y, shoulder.y);
        // Roughly 45°, which is what makes it an A rather than a shrug or a T.
        let drop = shoulder.y - hand.y;
        let out = hand.x - shoulder.x;
        assert!((drop / out - 1.0).abs() < 0.2, "the arm is at the wrong angle: out {out}, down {drop}");
    }

    /// Left and right have to be mirror images, or every animation is subtly
    /// one-sided and mirroring a clip stops being possible.
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

    /// Bones are stored parent-relative, so a chain has to actually join up
    /// after forward kinematics. If it does not, the rig comes apart the first
    /// time anything is rotated.
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

    /// The whole reason poses are rotations: one clip, several silhouettes.
    ///
    /// The same pose on two very different builds has to give the same *joint
    /// angles* — the elbow is bent the same amount — while the hand naturally
    /// lands somewhere else, because the arm is a different length. Store
    /// positions instead and this is the test that breaks.
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

        // And the two bodies are genuinely different, so the agreement above
        // is not just two identical skeletons.
        let stocky_hand = stocky[bone::HAND_R].tail;
        let lanky_hand = lanky[bone::HAND_R].tail;
        assert!(
            (stocky_hand - lanky_hand).length() > 0.1,
            "the two builds put the hand in the same place: {stocky_hand} and {lanky_hand}"
        );
    }

    /// A root offset is in hip-heights, so the same crouch takes proportionally
    /// as much off every build rather than dropping a short one through the
    /// floor.
    #[test]
    fn a_root_offset_scales_with_the_body() {
        let mut pose = Pose::rest();
        pose.root_offset = Vec3::new(0.0, -0.25, 0.0);

        for proportions in [Proportions::DEFAULT, Proportions::LANKY] {
            let skeleton = humanoid(proportions);
            let standing = posed(&skeleton, &Pose::rest())[bone::HEAD].tail.y;
            let crouched = posed(&skeleton, &pose)[bone::HEAD].tail.y;
            let dropped = standing - crouched;
            let expected = proportions.hip_metres() * 0.25;
            assert!((dropped - expected).abs() < 1e-4, "dropped {dropped}, expected {expected}");
        }
    }

    /// A pose says nothing about the bones it does not name, which is what
    /// makes layering possible — an IK pass on one leg must not straighten the
    /// other one.
    #[test]
    fn a_pose_leaves_unnamed_joints_alone() {
        let skeleton = humanoid(Proportions::DEFAULT);
        let before = posed(&skeleton, &Pose::rest());
        let after = posed(&skeleton, &Pose::rest().with(bone::THIGH_R, Quat::from_rotation_x(0.7)));

        assert!((before[bone::FOOT_L].tail - after[bone::FOOT_L].tail).length() < 1e-6);
        assert!((before[bone::FOOT_R].tail - after[bone::FOOT_R].tail).length() > 0.05);
    }

    /// The skeleton is placed by its root transform, feet first — the same end
    /// a spawn point marks, so standing a body on one is not a conversion.
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

        // Turned a quarter turn: what was on the character's right is now
        // where -Z was.
        let hand = placed[bone::HAND_R].tail - feet;
        let unturned = bones[bone::HAND_R].tail;
        assert!((hand - Quat::from_rotation_y(yaw) * unturned).length() < 1e-4);
    }
}
