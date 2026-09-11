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

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::rotation::quat_from_euler;
use crate::common::skeleton::ik::{solve, Chain, Reach, LEFT_ARM, RIGHT_ARM};
use crate::common::skeleton::rig::bone;
use crate::common::skeleton::{Pose, Skeleton};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
        }
    }
}

impl HoldSpec {
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
                let skeleton = humanoid(class.proportions());
                let mut pose = Pose::rest();
                let root = Transform::IDENTITY;

                let chest = skeleton.index_of(bone::CHEST).expect("bone is in the rig");
                let proportions = skeleton.proportions();
                let arm = proportions.height * proportions.arm_length;
                let pivot = skeleton.posed_bones(&pose, &root)[chest].tail;
                let placed = Transform::from_translation(pivot + doc.hold.carry(arm));

                let grip = placed.transform_point(doc.hold.grip());
                assert_eq!(
                    reach_for(&skeleton, &mut pose, &root, &RIGHT_ARM, grip, elbow_pole(1.0)),
                    Reach::Reached,
                    "{class:?} cannot reach the grip of {}",
                    path.display(),
                );

                if let Some(support) = doc.hold.support() {
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
