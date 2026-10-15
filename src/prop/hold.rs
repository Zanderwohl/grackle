//! How a weapon is held: where the hands go on it, and where it is carried.
//!
//! A hold is grip frames and a carry offset, not a pose: the arms are solved
//! onto the grips, so one hold serves every class and every frame. See
//! [the plan](../../documentation/weapons-in-hand.md).
//!
//! It lives on the prop because it is a fact about the model's shape. By
//! convention a prop's origin is its grip.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::class::Class;

use crate::common::rotation::quat_from_euler;
use crate::common::skeleton::ik::{self, reach_or_leave, Reach, LEFT_ARM, RIGHT_ARM};
use crate::common::skeleton::rig::bone;
use crate::common::skeleton::{Pose, Skeleton};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoldSpec {
    /// Where the trigger hand grips, in the weapon's own space. Normally the origin.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub grip: [f32; 3],
    /// How the trigger hand is turned on the grip, as Euler angles in axis
    /// order ([`crate::common::rotation`]).
    #[serde(default = "default_grip_rotation", with = "crate::prop::nice_f32::array")]
    pub grip_rotation: [f32; 3],
    /// Where the support hand grips, when there is one.
    #[serde(default = "default_support", with = "crate::prop::nice_f32::array")]
    pub support: [f32; 3],
    #[serde(default = "default_support_rotation", with = "crate::prop::nice_f32::array")]
    pub support_rotation: [f32; 3],
    /// A flag rather than an `Option` on `support`: TOML has no null, so absence
    /// would have to mean one-handed and a partial `[hold]` would lose the
    /// support hand.
    #[serde(default = "default_two_handed")]
    pub two_handed: bool,
    /// Where the grip is carried relative to the point between the shoulders,
    /// in fractions of arm length, so it retargets across classes.
    #[serde(default = "default_carry", with = "crate::prop::nice_f32::array")]
    pub carry: [f32; 3],
    /// How the weapon is turned as it is carried, before the aim is applied.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub carry_rotation: [f32; 3],
    /// Classes that hold this thing differently, keyed by `Class::key`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub per_class: BTreeMap<String, HoldOverride>,
}

/// Where the weapon hangs in first person. Not the carry, which sits well below
/// the eye. Stated across the frustum rather than in metres, so it stays in
/// place at any FOV or aspect.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewmodelSpec {
    /// Where the grip sits across the view, as a fraction of the frustum's
    /// half-extent at [`depth`](Self::depth). `[1.0, -1.0]` is the bottom-right
    /// corner.
    #[serde(default = "default_across", with = "crate::prop::nice_f32::array")]
    pub across: [f32; 2],
    /// How far in front of the eye, in metres; this decides apparent size.
    #[serde(default = "default_depth", with = "crate::prop::nice_f32::scalar")]
    pub depth: f32,
    /// Euler angles in axis order.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub rotation: [f32; 3],
}

impl Default for ViewmodelSpec {
    fn default() -> Self {
        Self { across: [0.35, -0.71], depth: 0.30, rotation: [0.0; 3] }
    }
}

fn default_across() -> [f32; 2] {
    ViewmodelSpec::default().across
}

fn default_depth() -> f32 {
    ViewmodelSpec::default().depth
}

impl ViewmodelSpec {
    /// Where the weapon sits in the camera's own frame.
    pub fn transform(&self, vertical_fov: f32, aspect: f32) -> Transform {
        let half_height = self.depth * (vertical_fov / 2.0).tan();
        Transform::from_translation(Vec3::new(
            self.across[0] * half_height * aspect,
            self.across[1] * half_height,
            -self.depth,
        ))
        .with_rotation(quat_from_euler(Vec3::from_array(self.rotation)))
    }
}

/// What one class does differently. Absence means "not overridden" here, but
/// "use the default" in [`HoldSpec`], hence `two_handed` is an `Option` here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HoldOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grip: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grip_rotation: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_rotation: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub two_handed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry_rotation: Option<[f32; 3]>,
}

impl HoldOverride {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

fn default_support() -> [f32; 3] {
    HoldSpec::default().support
}

fn default_grip_rotation() -> [f32; 3] {
    HoldSpec::default().grip_rotation
}

fn default_support_rotation() -> [f32; 3] {
    HoldSpec::default().support_rotation
}

fn default_two_handed() -> bool {
    HoldSpec::default().two_handed
}

fn default_carry() -> [f32; 3] {
    HoldSpec::default().carry
}

/// Euler radians, from the reference figure's former authored pose.
const DEFAULT_GRIP_ROTATION: [f32; 3] = [-0.8054, 0.9568, -1.2790];
const DEFAULT_SUPPORT_ROTATION: [f32; 3] = [-1.3740, 0.3441, -1.2572];

impl Default for HoldSpec {
    fn default() -> Self {
        Self {
            grip: [0.0; 3],
            grip_rotation: DEFAULT_GRIP_ROTATION,
            support: [0.0, 0.0, -0.07],
            support_rotation: DEFAULT_SUPPORT_ROTATION,
            two_handed: true,
            // Close to the centreline: out at the shoulder, the support hand
            // cannot reach without a torso twist the state machine never does.
            carry: [0.05, -0.14, -0.22],
            carry_rotation: [0.0; 3],
            per_class: BTreeMap::new(),
        }
    }
}

impl HoldSpec {
    pub fn for_class(&self, class: Option<Class>) -> HoldSpec {
        let Some(over) = class.and_then(|class| self.per_class.get(&Class::key(class))) else {
            return self.clone();
        };
        HoldSpec {
            grip: over.grip.unwrap_or(self.grip),
            grip_rotation: over.grip_rotation.unwrap_or(self.grip_rotation),
            support: over.support.unwrap_or(self.support),
            support_rotation: over.support_rotation.unwrap_or(self.support_rotation),
            two_handed: over.two_handed.unwrap_or(self.two_handed),
            carry: over.carry.unwrap_or(self.carry),
            carry_rotation: over.carry_rotation.unwrap_or(self.carry_rotation),
            per_class: BTreeMap::new(),
        }
    }

    /// This hold with `edited` written into it as `class` sees it. The panel and
    /// the viewport handles both go through here. An override keeps only what
    /// differs, so the rest goes on tracking the base.
    pub fn applied(&self, class: Option<Class>, edited: &HoldSpec) -> HoldSpec {
        let key = class
            .map(Class::key)
            .filter(|key| self.per_class.contains_key(key));

        let Some(key) = key else {
            let mut base = edited.clone();
            base.per_class = self.per_class.clone();
            return base;
        };

        let differs = |a: [f32; 3], b: [f32; 3]| (a != b).then_some(a);
        let mut applied = self.clone();
        applied.per_class.insert(
            key,
            HoldOverride {
                grip: differs(edited.grip, self.grip),
                grip_rotation: differs(edited.grip_rotation, self.grip_rotation),
                support: differs(edited.support, self.support),
                support_rotation: differs(edited.support_rotation, self.support_rotation),
                two_handed: (edited.two_handed != self.two_handed).then_some(edited.two_handed),
                carry: differs(edited.carry, self.carry),
                carry_rotation: differs(edited.carry_rotation, self.carry_rotation),
            },
        );
        applied
    }

    pub fn grip(&self) -> Vec3 {
        Vec3::from_array(self.grip)
    }

    pub fn support(&self) -> Option<Vec3> {
        self.two_handed.then(|| Vec3::from_array(self.support))
    }

    /// Metres, for arms this long.
    pub fn carry(&self, arm_length: f32) -> Vec3 {
        Vec3::from_array(self.carry) * arm_length
    }

    pub fn grip_frame(&self) -> Transform {
        Transform::from_translation(self.grip())
            .with_rotation(quat_from_euler(Vec3::from_array(self.grip_rotation)))
    }

    pub fn support_frame(&self) -> Option<Transform> {
        self.support().map(|at| {
            Transform::from_translation(at)
                .with_rotation(quat_from_euler(Vec3::from_array(self.support_rotation)))
        })
    }
}

/// Where the weapon sits, in whatever frame `root` is given in. Position from
/// the animated chest, orientation from the aim: following the chest's rotation
/// would tilt a crouching weapon off where the shot goes.
pub fn weapon_transform(
    skeleton: &Skeleton,
    pose: &Pose,
    root: &Transform,
    hold: &HoldSpec,
    aim: Quat,
) -> Option<Transform> {
    let chest = skeleton.index_of(bone::CHEST)?;
    // The chest's tail is the point between the shoulders.
    let pivot = skeleton.posed_bone(pose, root, chest).tail;
    let arm = skeleton.proportions().arm_metres();

    Some(
        Transform::from_translation(pivot + aim * hold.carry(arm))
            .with_rotation(aim * quat_from_euler(Vec3::from_array(hold.carry_rotation))),
    )
}

/// The frame a weapon hangs from: the point between the shoulders, turned by
/// the aim. [`weapon_transform`] without the carry offset, which a clip's
/// `Space::Carry` would otherwise count twice.
pub fn carry_frame(
    skeleton: &Skeleton,
    pose: &Pose,
    root: &Transform,
    aim: Quat,
) -> Option<Transform> {
    let chest = skeleton.index_of(bone::CHEST)?;
    let pivot = skeleton.posed_bone(pose, root, chest).tail;
    Some(Transform::from_translation(pivot).with_rotation(aim))
}

/// Put the hands on a weapon that is already placed. The reference figure and
/// a body in a match both use this. Returns how each hand did, trigger hand first.
pub fn grip_with(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    hold: &HoldSpec,
    weapon: &Transform,
) -> [Reach; 2] {
    let mut reached = [Reach::Impossible; 2];
    let hands = [
        (RIGHT_ARM, bone::HAND_R, 1.0, Some(hold.grip_frame())),
        (LEFT_ARM, bone::HAND_L, -1.0, hold.support_frame()),
    ];

    for (index, (chain, hand, side, frame)) in hands.into_iter().enumerate() {
        let Some(frame) = frame else { continue };
        let placed = weapon.mul_transform(frame);
        reached[index] = reach_or_leave(skeleton, pose, root, &chain, placed.translation, root.rotation * ik::elbow_pole(side));
        if reached[index] == Reach::Reached {
            // After the solve: a world rotation depends on where the arm ended up.
            ik::set_world_rotation(skeleton, pose, root, hand, placed.rotation);
        }
    }
    reached
}




#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;
    use crate::common::class::Class;
    use crate::common::skeleton::humanoid;


    #[test]
    fn a_class_inherits_everything_it_does_not_override() {
        let mut base = HoldSpec::default();
        base.per_class.insert(
            Class::key(Class::Heavy),
            HoldOverride { carry: Some([1.0, 2.0, 3.0]), ..default() },
        );

        let heavy = base.for_class(Some(Class::Heavy));
        assert_eq!(heavy.carry, [1.0, 2.0, 3.0], "the override did not take");
        assert_eq!(heavy.grip, base.grip, "the grip was not inherited");
        assert_eq!(heavy.two_handed, base.two_handed, "two-handedness was not inherited");

        assert_eq!(base.for_class(Some(Class::Scout)).carry, base.carry);
        assert_eq!(base.for_class(None).carry, base.carry);
    }

    #[test]
    fn an_edit_goes_wherever_the_class_is_being_edited() {
        let base = HoldSpec::default();

        let mut edited = base.clone();
        edited.carry = [9.0, 9.0, 9.0];
        let to_base = base.applied(Some(Class::Heavy), &edited);
        assert_eq!(to_base.carry, [9.0, 9.0, 9.0], "an edit with no override missed the base");
        assert!(to_base.per_class.is_empty());

        let mut with_override = base.clone();
        with_override
            .per_class
            .insert(Class::key(Class::Heavy), HoldOverride::default());
        let to_class = with_override.applied(Some(Class::Heavy), &edited);
        assert_eq!(to_class.carry, base.carry, "an override leaked into the base");
        let over = &to_class.per_class[&Class::key(Class::Heavy)];
        assert_eq!(over.carry, Some([9.0, 9.0, 9.0]));
        assert_eq!(over.grip, None, "an unchanged field was written down anyway");
        assert_eq!(to_class.for_class(Some(Class::Heavy)).carry, [9.0, 9.0, 9.0]);
    }

    #[test]
    fn an_overridden_hold_carries_no_overrides_of_its_own() {
        let mut base = HoldSpec::default();
        base.per_class.insert(
            Class::key(Class::Heavy),
            HoldOverride { two_handed: Some(false), ..default() },
        );
        assert!(base.for_class(Some(Class::Heavy)).per_class.is_empty());
    }

    /// Fails if some class cannot reach a shipped prop's grips, which in game
    /// is silent: the arm just swings past.
    #[test]
    fn every_class_can_reach_every_shipped_props_grip() {
        let directory = std::path::Path::new("assets/default/props");
        let Ok(entries) = std::fs::read_dir(directory) else { return };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("gpp") {
                continue;
            }
            let doc = crate::prop::document::parse(&std::fs::read_to_string(&path).unwrap())
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));

            for class in Class::iter() {
                let hold = doc.hold.for_class(Some(class));
                let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(class));
                let mut pose = Pose::rest();
                let root = Transform::IDENTITY;

                let chest = skeleton.index_of(bone::CHEST).expect("bone is in the rig");
                let arm = skeleton.proportions().arm_metres();
                let pivot = skeleton.posed_bones(&pose, &root)[chest].tail;
                let placed = Transform::from_translation(pivot + hold.carry(arm));

                let grip = placed.transform_point(hold.grip());
                assert_eq!(
                    reach_or_leave(&skeleton, &mut pose, &root, &RIGHT_ARM, grip, ik::elbow_pole(1.0)),
                    Reach::Reached,
                    "{class:?} cannot reach the grip of {}",
                    path.display(),
                );

                if let Some(support) = hold.support() {
                    let support = placed.transform_point(support);
                    assert_eq!(
                        reach_or_leave(&skeleton, &mut pose, &root, &LEFT_ARM, support, ik::elbow_pole(-1.0)),
                        Reach::Reached,
                        "{class:?} cannot reach the support grip of {} — either move it \
                         or set `two_handed = false`",
                        path.display(),
                    );
                }
            }
        }
    }

    /// `ik::solve` straightens a limb before reporting `Short`; an arm must be
    /// put back instead.
    #[test]
    fn an_arm_that_cannot_reach_is_left_where_the_animation_had_it() {
        let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Scout));
        let mut pose = Pose::rest();
        let before = pose.joint(bone::UPPER_ARM_R);

        let out_of_reach = Vec3::new(0.0, 1.4, -8.0);
        let reach = reach_or_leave(
            &skeleton,
            &mut pose,
            &Transform::IDENTITY,
            &RIGHT_ARM,
            out_of_reach,
            ik::elbow_pole(1.0),
        );

        assert_eq!(reach, Reach::Short);
        assert_eq!(pose.joint(bone::UPPER_ARM_R), before, "the arm stretched for it anyway");
    }

    /// Fails if elbows are poled in world space: the prop editor turns its
    /// reference figure, and it would then hold a weapon differently from a
    /// body in a match.
    #[test]
    fn a_turned_body_grips_a_turned_weapon_the_same_way() {
        let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Soldier));
        let hold = HoldSpec::default();
        let grip = |turn: Quat| {
            let root = Transform::from_rotation(turn);
            let weapon = root.mul_transform(
                weapon_transform(&skeleton, &Pose::rest(), &Transform::IDENTITY, &hold, Quat::IDENTITY)
                    .expect("the rig has a chest"),
            );
            let mut pose = Pose::rest();
            let reached = grip_with(&skeleton, &mut pose, &root, &hold, &weapon);
            (reached, pose)
        };

        let (straight, at_rest) = grip(Quat::IDENTITY);
        let (turned, after) = grip(Quat::from_euler(EulerRot::YXZ, 1.1, 0.4, -0.3));
        assert_eq!(straight, turned);
        for name in [bone::UPPER_ARM_R, bone::FOREARM_R, bone::UPPER_ARM_L, bone::FOREARM_L] {
            let angle = at_rest.joint(name).angle_between(after.joint(name));
            assert!(angle < 1e-3, "{name} bends {angle} rad differently once the body is turned");
        }
    }
}
