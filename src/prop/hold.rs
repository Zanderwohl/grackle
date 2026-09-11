//! How a weapon is held: where the hands go on it, and where it is carried.
//!
//! **A hold is two points and an offset, not a pose.** That is the whole of
//! why any weapon can be held while the body does anything: the arms are
//! *solved* to the points rather than posed to match the weapon, so a hold is
//! the same data for every class and at every frame of a walk cycle. See
//! [the plan](../../documentation/weapons-in-hand.md).
//!
//! It lives on the prop because it is a fact about the model's *shape*, and
//! because the prop editor is where a reference figure is already standing to
//! judge it against. Nothing authors one yet — every prop uses the default,
//! which the grip convention makes a sensible one: a prop's origin *is* its
//! grip, so the only guess here is where the support hand goes and how far out
//! the thing is carried.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::class::Class;

use crate::common::rotation::quat_from_euler;
use crate::common::skeleton::ik::{solve, Chain, Reach, LEFT_ARM, RIGHT_ARM};
use crate::common::skeleton::rig::bone;
use crate::common::skeleton::{Pose, Skeleton};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoldSpec {
    /// Where the trigger hand grips, in the weapon's own space.
    ///
    /// The origin, by the convention every prop is authored to — so this is
    /// only here for a model that wants to disagree.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub grip: [f32; 3],
    /// How the trigger hand is turned on the grip, as Euler angles in axis
    /// order — the convention [`crate::common::rotation`] owns.
    ///
    /// **A grip is a frame, not a point.** A hand at the right place with the
    /// wrong rotation holds a weapon by the back of its wrist. This used to be
    /// derived from the reference figure's authored pose, which made the
    /// runtime read its grip orientation out of a prop in the editor; stating
    /// it here is what lets the figure and the game agree by construction.
    #[serde(default = "default_grip_rotation", with = "crate::prop::nice_f32::array")]
    pub grip_rotation: [f32; 3],
    /// Where the support hand grips, when there is one.
    #[serde(default = "default_support", with = "crate::prop::nice_f32::array")]
    pub support: [f32; 3],
    /// How the support hand is turned on it.
    #[serde(default = "default_support_rotation", with = "crate::prop::nice_f32::array")]
    pub support_rotation: [f32; 3],
    /// Whether the off hand comes onto the weapon at all.
    ///
    /// A flag rather than an `Option<[f32; 3]>`, because **TOML has no null**:
    /// an absent field would have to mean one-handed, so a `[hold]` section
    /// that set only the carry would silently lose the support hand. Stated
    /// outright, every field defaults to what it defaults to and a partial
    /// hold behaves like a whole one.
    #[serde(default = "default_two_handed")]
    pub two_handed: bool,
    /// Where the grip is carried relative to the point between the shoulders,
    /// **in fractions of arm length**.
    ///
    /// A fraction rather than metres for the reason a crouch's root offset is
    /// in hip-heights: a Heavy's arms are not a Scout's, and a carry authored
    /// in metres would put one of them out of its own reach.
    #[serde(default = "default_carry", with = "crate::prop::nice_f32::array")]
    pub carry: [f32; 3],
    /// How the weapon is turned as it is carried, before the aim is applied —
    /// what cants a weapon or angles it across the body.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub carry_rotation: [f32; 3],
    /// Classes that hold this thing differently, keyed the way
    /// `loadouts.toml` keys them.
    ///
    /// A Heavy plausibly carries a weapon unlike a Scout, and `carry` being in
    /// arm-length fractions retargets the *size* of a build but not its
    /// habits. Nothing fills this in yet; a prop with no entries is a prop
    /// everybody holds the same way.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub per_class: BTreeMap<String, HoldOverride>,
}

/// Where the weapon hangs in first person.
///
/// **Its own placement, in metres, and not the carry.** A carry is anatomical
/// — it is stated in arm lengths because a Heavy's reach is not a Scout's, and
/// it puts the weapon where a body would actually hold it, which is well below
/// the eye. A viewmodel is a *framing* decision: a composition on a screen that
/// should not shrink because a shorter class is holding it, and which sits far
/// closer to the view axis than any real hold does.
///
/// Reusing the carry put the grip 0.39 m below an eye whose frustum is 0.16 m
/// tall at that distance — the weapon was rendering perfectly, off the bottom
/// of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewmodelSpec {
    /// Where the grip sits relative to the eye, in metres.
    #[serde(default = "default_viewmodel_at", with = "crate::prop::nice_f32::array")]
    pub at: [f32; 3],
    /// How the weapon is turned there, in axis order.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub rotation: [f32; 3],
}

impl Default for ViewmodelSpec {
    fn default() -> Self {
        // Down and to the right of the view axis, and far enough forward to be
        // in front of the near plane — the usual place a shooter puts one.
        Self { at: [0.13, -0.15, -0.30], rotation: [0.0; 3] }
    }
}

fn default_viewmodel_at() -> [f32; 3] {
    ViewmodelSpec::default().at
}

impl ViewmodelSpec {
    pub fn transform(&self) -> Transform {
        Transform::from_translation(Vec3::from_array(self.at))
            .with_rotation(quat_from_euler(Vec3::from_array(self.rotation)))
    }
}

/// What one class does differently.
///
/// **Absence means "not overridden" here, and "use the default" in the base
/// hold.** Opposite readings, and both honest because they are different
/// questions: a base hold with no `support` is a weapon nobody stated a
/// support point for, while an override with no `support` is a class that did
/// not want to move it. That is also why `two_handed` is an `Option` here and
/// a plain flag there — in an override, absence is exactly what `Option`
/// means, and TOML can express it by leaving the field out.
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
    /// Whether this says anything at all. An entry that overrides nothing is
    /// worth dropping rather than writing to the file.
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

impl Default for HoldSpec {
    fn default() -> Self {
        Self {
            grip: [0.0; 3],
            // Taken from the reference figure's authored pose, which is what
            // used to decide this, so the default look survived its deletion.
            grip_rotation: [-0.8054, 0.9568, -1.2790],
            support: [0.0, 0.0, -0.07],
            support_rotation: [-1.3740, 0.3441, -1.2572],
            two_handed: true,
            // Close to the centreline on purpose. A weapon carried out at the
            // shoulder is a weapon the *other* hand cannot reach, which the
            // reference figure only gets away with by twisting its torso —
            // something a body being animated by the state machine does not do.
            carry: [0.05, -0.14, -0.22],
            carry_rotation: [0.0; 3],
            per_class: BTreeMap::new(),
        }
    }
}

impl HoldSpec {
    /// This hold as a given class performs it.
    ///
    /// The base hold with whatever that class overrode laid on top. A class
    /// with no entry gets the base, which is the common case and the reason
    /// this returns a value rather than an `Option`.
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
            // Not inherited: an override describing further overrides would be
            // a class holding a thing as another class holds it.
            per_class: BTreeMap::new(),
        }
    }

    /// This hold with `edited` written into it as `class` sees it.
    ///
    /// **The one place an edit to a hold is applied**, so the panel's boxes
    /// and the viewport's handles cannot write it two different ways. An
    /// override keeps only what actually differs from the base, so it stays a
    /// statement about what a class does differently rather than a copy that
    /// quietly stops tracking.
    pub fn applied(&self, class: Option<Class>, edited: &HoldSpec) -> HoldSpec {
        let key = class
            .map(Class::key)
            .filter(|key| self.per_class.contains_key(key));

        let Some(key) = key else {
            // No override in play: the base takes the edit and keeps whatever
            // overrides it was carrying.
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

    /// Where the grip is carried, in metres, for arms this long.
    pub fn carry(&self, arm_length: f32) -> Vec3 {
        Vec3::from_array(self.carry) * arm_length
    }

    /// The trigger hand's frame on the weapon.
    pub fn grip_frame(&self) -> Transform {
        Transform::from_translation(self.grip())
            .with_rotation(quat_from_euler(Vec3::from_array(self.grip_rotation)))
    }

    /// The support hand's, when there is one.
    pub fn support_frame(&self) -> Option<Transform> {
        self.support().map(|at| {
            Transform::from_translation(at)
                .with_rotation(quat_from_euler(Vec3::from_array(self.support_rotation)))
        })
    }
}

/// Where the weapon sits, in whatever frame `root` is given in.
///
/// **The pivot's position comes from the animated chest and its orientation
/// from the aim.** Following the chest's rotation too would make a crouch's
/// lean tilt the weapon away from where the shot goes; ignoring its position
/// would detach the weapon from a body that is bobbing.
pub fn weapon_transform(
    skeleton: &Skeleton,
    pose: &Pose,
    root: &Transform,
    hold: &HoldSpec,
    aim: Quat,
) -> Option<Transform> {
    let chest = skeleton.index_of(bone::CHEST)?;
    // The chest's tail is the point between the shoulders.
    let pivot = skeleton.posed_bones(pose, root)[chest].tail;
    let proportions = skeleton.proportions();
    let arm = proportions.height * proportions.arm_length;

    Some(
        Transform::from_translation(pivot + aim * hold.carry(arm))
            .with_rotation(aim * quat_from_euler(Vec3::from_array(hold.carry_rotation))),
    )
}

/// Put the hands on a weapon that is already placed.
///
/// **The one description of how a body holds a thing**, so the reference
/// figure in the prop editor and a body in a match cannot disagree — which
/// they did when the figure had an authored pose of its own and the runtime
/// read its grip orientation back out of it.
///
/// Returns how each hand did, so a caller can say so: the trigger hand first.
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
        reached[index] = reach_for(skeleton, pose, root, &chain, placed.translation, elbow_pole(side));
        if reached[index] == Reach::Reached {
            // Last for this hand, because it wants the arm as the solve left
            // it: the hand takes the grip's orientation rather than the one
            // the animation gave it.
            set_world_rotation(skeleton, pose, root, hand, placed.rotation);
        }
    }
    reached
}

/// Which way an elbow points.
///
/// Backwards and a little out and down, which is where a person's elbow goes
/// holding something in front of them. The solver cannot work this out — it is
/// the one thing about a two-bone chain the target does not determine — and
/// getting it wrong is an arm bent the wrong way at the elbow.
fn elbow_pole(side: f32) -> Vec3 {
    Vec3::new(0.35 * side, -0.85, 0.55).normalize()
}

/// Solve a chain onto a target, or leave it exactly as it was.
///
/// [`solve`] writes a straightened limb and *then* reports `Short`, which is
/// right for a leg — a foot pointed at a floor it cannot reach still wants to
/// point at it — and wrong for a hand: an arm stretched at a grip it cannot
/// hold reads as broken, where one still swinging with the walk cycle merely
/// reads as not holding.
fn reach_for(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    chain: &Chain,
    target: Vec3,
    pole: Vec3,
) -> Reach {
    let before = [chain.upper, chain.lower]
        .into_iter()
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

/// Turn a bone so it ends up facing a given way in `root`'s frame.
///
/// A pose stores a joint's rotation relative to its rest and its parent, so a
/// world orientation has to be divided back through both. Read off the posed
/// skeleton rather than walked up the parent chain, because the posed bone
/// already carries the product of everything above it.
fn set_world_rotation(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    name: &'static str,
    wanted: Quat,
) {
    let Some(index) = skeleton.index_of(name) else { return };
    let posed = skeleton.posed_bones(pose, root)[index].rotation;
    let base = posed * pose.joint(name).inverse();
    pose.set(name, base.inverse() * wanted);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::class::Class;
    use crate::common::skeleton::humanoid;

    const EVERY_CLASS: [Class; 11] = [
        Class::Scout, Class::Soldier, Class::Pyro, Class::Demoman,
        Class::Heavy, Class::Engineer, Class::Medic, Class::Sniper,
        Class::Spy, Class::Civilian, Class::Mercenary,
    ];

    /// Absence in an override means "not overridden", which is the opposite
    /// reading from the base hold — where absence means "use the default" —
    /// and the reason the two use different types for the same question.
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

        // A class nobody mentioned, and no class at all, both get the base.
        assert_eq!(base.for_class(Some(Class::Scout)).carry, base.carry);
        assert_eq!(base.for_class(None).carry, base.carry);
    }

    /// An edit lands on the base when no override is in play, and in the
    /// override when one is — writing down only what differs, so the rest goes
    /// on tracking the base.
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

    /// An override describing further overrides would be a class holding a
    /// thing as another class holds it.
    #[test]
    fn an_overridden_hold_carries_no_overrides_of_its_own() {
        let mut base = HoldSpec::default();
        base.per_class.insert(
            Class::key(Class::Heavy),
            HoldOverride { two_handed: Some(false), ..default() },
        );
        assert!(base.for_class(Some(Class::Heavy)).per_class.is_empty());
    }

    /// **Every class has to be able to reach every prop the pack ships.** A
    /// carry is authored in fractions of arm length so that it retargets, and
    /// a build whose hand cannot get to the grip holds nothing while its arm
    /// swings past it — which is silent, and which a per-prop hold makes easy
    /// to author your way into.
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

            for class in EVERY_CLASS {
                let hold = doc.hold.for_class(Some(class));
                let skeleton = humanoid(class.proportions());
                let mut pose = Pose::rest();
                let root = Transform::IDENTITY;

                let chest = skeleton.index_of(bone::CHEST).expect("bone is in the rig");
                let proportions = skeleton.proportions();
                let arm = proportions.height * proportions.arm_length;
                let pivot = skeleton.posed_bones(&pose, &root)[chest].tail;
                let placed = Transform::from_translation(pivot + hold.carry(arm));

                let grip = placed.transform_point(hold.grip());
                assert_eq!(
                    reach_for(&skeleton, &mut pose, &root, &RIGHT_ARM, grip, elbow_pole(1.0)),
                    Reach::Reached,
                    "{class:?} cannot reach the grip of {}",
                    path.display(),
                );

                if let Some(support) = hold.support() {
                    let support = placed.transform_point(support);
                    assert_eq!(
                        reach_for(&skeleton, &mut pose, &root, &LEFT_ARM, support, elbow_pole(-1.0)),
                        Reach::Reached,
                        "{class:?} cannot reach the support grip of {} — either move it \
                         or set `two_handed = false`",
                        path.display(),
                    );
                }
            }
        }
    }

    /// A hand that cannot reach does not stretch for it. `ik::solve` writes a
    /// straightened limb *and then* reports `Short`, which is right for a foot
    /// pointed at a floor and wrong for an arm reaching at a grip it cannot
    /// hold.
    #[test]
    fn an_arm_that_cannot_reach_is_left_where_the_animation_had_it() {
        let skeleton = humanoid(Class::Scout.proportions());
        let mut pose = Pose::rest();
        let before = pose.joint(bone::UPPER_ARM_R);

        let out_of_reach = Vec3::new(0.0, 1.4, -8.0);
        let reach = reach_for(
            &skeleton,
            &mut pose,
            &Transform::IDENTITY,
            &RIGHT_ARM,
            out_of_reach,
            elbow_pole(1.0),
        );

        assert_eq!(reach, Reach::Short);
        assert_eq!(pose.joint(bone::UPPER_ARM_R), before, "the arm stretched for it anyway");
    }

}
