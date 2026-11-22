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
use crate::common::skeleton::ik::{self, reach_or_leave, Reach};
use crate::common::skeleton::play::Grips;
use crate::common::skeleton::{Pose, Skeleton};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoldSpec {
    /// In the weapon's own space.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub grip: [f32; 3],
    /// How the trigger hand is turned on the grip, as Euler angles in axis
    /// order ([`crate::common::rotation`]).
    #[serde(default = "default_grip_rotation", with = "crate::prop::nice_f32::array")]
    pub grip_rotation: [f32; 3],
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
    /// Applied before the aim.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub carry_rotation: [f32; 3],
    /// Classes that hold this thing differently, keyed by `Class::key`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub per_class: BTreeMap<String, HoldOverride>,
}

/// Where the weapon hangs in first person. Not the carry, which sits well below
/// the eye. Stated across the frustum rather than in meters, so it stays in
/// place at any FOV or aspect.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewmodelSpec {
    /// Where the grip sits across the view, as a fraction of the frustum's
    /// half-extent at [`depth`](Self::depth). `[1.0, -1.0]` is the bottom-right
    /// corner.
    #[serde(default = "default_across", with = "crate::prop::nice_f32::array")]
    pub across: [f32; 2],
    /// How far in front of the eye, in meters; this decides apparent size.
    #[serde(default = "default_depth", with = "crate::prop::nice_f32::scalar")]
    pub depth: f32,
    /// Euler angles in axis order.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub rotation: [f32; 3],
    /// With the sights up, such as a scope's eyepiece at the eye. `None` stays put.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sighted: Option<Framing>,
}

/// Where a weapon sits in the camera's frame, as [`ViewmodelSpec`] states it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Framing {
    #[serde(with = "crate::prop::nice_f32::array")]
    pub across: [f32; 2],
    #[serde(with = "crate::prop::nice_f32::scalar")]
    pub depth: f32,
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub rotation: [f32; 3],
}

impl Default for ViewmodelSpec {
    fn default() -> Self {
        Self { across: [0.35, -0.71], depth: 0.30, rotation: [0.0; 3], sighted: None }
    }
}

fn default_across() -> [f32; 2] {
    ViewmodelSpec::default().across
}

fn default_depth() -> f32 {
    ViewmodelSpec::default().depth
}

impl Framing {
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

impl ViewmodelSpec {
    /// With the sights down.
    pub fn framing(&self) -> Framing {
        Framing { across: self.across, depth: self.depth, rotation: self.rotation }
    }

    pub fn set_framing(&mut self, Framing { across, depth, rotation }: Framing) {
        (self.across, self.depth, self.rotation) = (across, depth, rotation);
    }

    /// With the sights down.
    pub fn transform(&self, vertical_fov: f32, aspect: f32) -> Transform {
        self.framing().transform(vertical_fov, aspect)
    }

    /// The prop turned into the camera's frame.
    pub fn rotation(&self) -> Quat {
        quat_from_euler(Vec3::from_array(self.rotation))
    }

    pub fn sighted_framing(&self) -> Framing {
        self.sighted.unwrap_or_else(|| self.framing())
    }

    /// `raised` is 0 to 1.
    pub fn transform_raised(&self, vertical_fov: f32, aspect: f32, raised: f32) -> Transform {
        let down = self.transform(vertical_fov, aspect);
        if raised <= 0.0 || self.sighted.is_none() {
            return down;
        }
        let up = self.sighted_framing().transform(vertical_fov, aspect);
        if raised >= 1.0 {
            return up;
        }
        Transform::from_translation(down.translation.lerp(up.translation, raised))
            .with_rotation(down.rotation.slerp(up.rotation, raised))
    }
}

/// Where shots appear to leave the weapon, in its own space. Drawing only: the
/// shot itself always leaves the eye.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MuzzleSpec {
    /// At rest, in the prop's own space.
    #[serde(with = "crate::prop::nice_f32::array")]
    pub at: [f32; 3],
    /// The prop bone the muzzle rides on, by name; absent is the prop itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash: Option<MuzzleFlashSpec>,
}

/// See [`crate::game::muzzle_flash`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MuzzleFlashSpec {
    /// A flag rather than dropping the section, so turning it off and on keeps
    /// the sizes.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Meters from the muzzle to the tips of the crown.
    #[serde(with = "crate::prop::nice_f32::scalar")]
    pub length: f32,
    /// Meters from the barrel's axis to the tips of the crown.
    #[serde(with = "crate::prop::nice_f32::scalar")]
    pub radius: f32,
    /// Meters, the ring at the muzzle.
    #[serde(with = "crate::prop::nice_f32::scalar")]
    pub base: f32,
    #[serde(with = "crate::prop::nice_f32::scalar")]
    pub seconds: f32,
}

impl Default for MuzzleFlashSpec {
    fn default() -> Self {
        Self { enabled: true, length: 0.3, radius: 0.14, base: 0.025, seconds: 0.1 }
    }
}

fn default_true() -> bool {
    true
}

impl MuzzleSpec {
    pub fn point(&self) -> Vec3 {
        Vec3::from_array(self.at)
    }

    pub fn flash(&self) -> Option<MuzzleFlashSpec> {
        self.flash.filter(|flash| flash.enabled)
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
            // Close to the centerline: out at the shoulder, the support hand
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

    /// This hold with `edited` written into it as `class` sees it. An override
    /// keeps only what differs, so the rest goes on tracking the base.
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

    /// Meters, for arms this long.
    pub fn carry(&self, arm_length: f32) -> Vec3 {
        Vec3::from_array(self.carry) * arm_length
    }

    pub fn grip_frame(&self) -> Transform {
        Transform::from_translation(self.grip())
            .with_rotation(quat_from_euler(Vec3::from_array(self.grip_rotation)))
    }

    pub fn support_frame(&self) -> Option<Transform> {
        self.two_handed.then(|| self.loose_support_frame())
    }

    /// The support grip whether or not the weapon is held with it.
    pub fn loose_support_frame(&self) -> Transform {
        Transform::from_translation(Vec3::from_array(self.support))
            .with_rotation(quat_from_euler(Vec3::from_array(self.support_rotation)))
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
    if let Some((bone, offset)) = skeleton.anatomy().mount {
        let pivot = skeleton.posed_bone(pose, root, bone).tail;
        return Some(
            Transform::from_translation(pivot + aim * offset)
                .with_rotation(aim * quat_from_euler(Vec3::from_array(hold.carry_rotation))),
        );
    }
    let carry = skeleton.anatomy().carry?;
    let pivot = skeleton.posed_bone(pose, root, carry).tail;
    let arm = skeleton.anatomy().arm;

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
    let carry = skeleton.anatomy().carry?;
    let pivot = skeleton.posed_bone(pose, root, carry).tail;
    Some(Transform::from_translation(pivot).with_rotation(aim))
}

/// Put the hands on a weapon that is already placed. Returns how each hand
/// did, trigger hand first.
pub fn grip_with(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    hold: &HoldSpec,
    weapon: &Transform,
) -> [Reach; 2] {
    grip_hands(skeleton, pose, root, hold, weapon, [true; 2], Grips::default())
}

/// [`grip_with`], solving only the hands in `which`, trigger hand first. A
/// hand left out is untouched and reports `Impossible`. Each grip is moved by
/// its `grips` entry, in the weapon's axes, keeping the hand's rotation:
/// what a clip's `Space::Grip` tracks ask for. A one-handed weapon's free hand
/// goes to the stored support point while a track keys it.
pub fn grip_hands(
    skeleton: &Skeleton,
    pose: &mut Pose,
    root: &Transform,
    hold: &HoldSpec,
    weapon: &Transform,
    which: [bool; 2],
    grips: Grips,
) -> [Reach; 2] {
    let mut reached = [Reach::Impossible; 2];
    let loose = hold.support_frame().is_none();
    let support = hold.support_frame().or_else(|| (grips.taken[1] > 0.0).then(|| hold.loose_support_frame()));
    let frames = [Some(hold.grip_frame()), support];
    // A fading clip moves a free hand by its whole key; the caller blends the arm instead.
    let mut offsets = grips.offsets;
    if loose && grips.taken[1] > 0.0 {
        offsets[1] /= grips.taken[1];
    }

    for (index, frame) in frames.into_iter().enumerate() {
        let Some(limb) = skeleton.anatomy().hand(index).copied() else { continue };
        let Some(mut frame) = frame.filter(|_| which[index]) else { continue };
        frame.translation += offsets[index];
        let placed = weapon.mul_transform(frame);
        reached[index] = reach_or_leave(skeleton, pose, root, &limb.chain, placed.translation, root.rotation * limb.pole);
        if let (Reach::Reached, Some(hand)) = (reached[index], limb.chain.tip) {
            // After the solve: a world rotation depends on where the arm ended up.
            ik::set_world_rotation(skeleton, pose, root, hand, placed.rotation);
        }
    }
    reached
}




#[cfg(test)]
mod tests {
    /// Fails if a raise misses either end, or a framing with no sighted pose moves.
    #[test]
    fn a_raise_runs_from_the_lowered_framing_to_the_sighted_one() {
        use super::{Framing, ViewmodelSpec};
        let (fov, aspect) = (1.2, 16.0 / 9.0);
        let sighted = Framing { across: [0.0, -0.2], depth: 0.5, rotation: [0.0, 0.1, 0.0] };
        let spec = ViewmodelSpec { sighted: Some(sighted), ..ViewmodelSpec::default() };
        assert_eq!(spec.transform_raised(fov, aspect, 0.0), spec.transform(fov, aspect));
        let up = spec.transform_raised(fov, aspect, 1.0);
        let wanted = spec.sighted_framing().transform(fov, aspect);
        assert_eq!(up, wanted);
        let plain = ViewmodelSpec::default();
        assert_eq!(plain.transform_raised(fov, aspect, 1.0), plain.transform(fov, aspect));
    }

    /// A body with no hands holds its weapon on its mount, turned by the aim, whatever the hold's
    /// carry says; nothing grips.
    #[test]
    fn a_body_without_hands_wears_its_weapon_on_its_mount() {
        use crate::common::assets::{default_packs, Assets};
        use crate::common::skeleton::rig_file::{build_rig, MountEntry};
        let mut file = crate::common::class_file::load_rig(&Assets::default(), &default_packs(), "raptor").unwrap();
        file.hands.clear();
        file.mount = Some(MountEntry { bone: "spine".into(), offset: [0.0, 0.14, -0.1] });
        let rig = build_rig(&file, &crate::common::skeleton::rig::Build { rig: Some("raptor".into()), ..default() }).unwrap();
        let (bone, offset) = rig.anatomy().mount.expect("the rig has a mount");
        let root = Transform::IDENTITY;
        let aim = Quat::from_rotation_y(0.4) * Quat::from_rotation_x(0.2);
        let hold = HoldSpec { carry: [0.3, -0.2, -0.5], ..HoldSpec::default() };
        let placed = weapon_transform(&rig, &Pose::rest(), &root, &hold, aim).unwrap();
        let mount = rig.posed_bone(&Pose::rest(), &root, bone).tail + aim * offset;
        assert!(placed.translation.distance(mount) < 1e-5, "the weapon is {} m off the mount", placed.translation.distance(mount));

        let mut pose = Pose::rest();
        assert_eq!(grip_with(&rig, &mut pose, &root, &hold, &placed), [Reach::Impossible; 2]);
    }

    /// Fails if the raptor's arms cannot reach a shipped weapon's grips where its hold carries
    /// it, level or aimed up or down: in game that is an arm swinging past the weapon.
    #[test]
    fn the_raptor_grips_every_shipped_weapon() {
        let raptor = crate::common::class::ClassBuilds::shipped().rig_for(Class::Raptor);
        let root = Transform::IDENTITY;
        let Ok(entries) = std::fs::read_dir("assets/default/props") else { return };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().and_then(|e| e.to_str()) != Some("gpp") {
                continue;
            }
            let doc = crate::prop::document::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
            if doc.archetype != crate::prop::archetype::Archetype::Held {
                continue;
            }
            let hold = doc.hold.for_class(Some(Class::Raptor));
            for pitch in [-0.8f32, 0.0, 0.8] {
                let aim = Quat::from_rotation_x(pitch);
                let placed = weapon_transform(&raptor, &Pose::rest(), &root, &hold, aim).unwrap();
                let mut pose = Pose::rest();
                let reached = grip_with(&raptor, &mut pose, &root, &hold, &placed);
                let wanted = if hold.two_handed { [Reach::Reached; 2] } else { [Reach::Reached, Reach::Impossible] };
                assert_eq!(reached, wanted, "the raptor's hands at pitch {pitch} on {}", path.display());
            }
        }
    }
    use crate::common::skeleton::rig::bone;
    use crate::common::skeleton::ik::{LEFT_ARM, RIGHT_ARM};
    use super::*;
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
            // A hat or a crate has a hold nobody uses.
            if doc.archetype != crate::prop::archetype::Archetype::Held {
                continue;
            }

            for class in crate::common::class::ClassBuilds::shipped().humanoids() {
                let hold = doc.hold.for_class(Some(class));
                let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(class));
                let mut pose = Pose::rest();
                let root = Transform::IDENTITY;

                let chest = skeleton.index_of(bone::CHEST).expect("bone is in the rig");
                let arm = skeleton.anatomy().arm;
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
