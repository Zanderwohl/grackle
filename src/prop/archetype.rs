//! What a prop *is* to the game: held, worn, or placed in a map as an obstacle,
//! a breakable or a pickup. See [the plan](../../documentation/prop-archetypes.md).
//!
//! Every section is kept whatever the archetype, so switching archetype and
//! back loses nothing.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::class::Class;
use crate::common::rotation::quat_from_euler;
use crate::common::skeleton::{Pose, Skeleton};
use crate::get;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Archetype {
    #[default]
    Held,
    Hat,
    Obstacle,
    Breakable,
    Pickup,
}

impl Archetype {
    pub const ALL: [Archetype; 5] = [
        Archetype::Held,
        Archetype::Hat,
        Archetype::Obstacle,
        Archetype::Breakable,
        Archetype::Pickup,
    ];

    pub fn name(self) -> String {
        match self {
            Archetype::Held => get!("prop.archetype.held"),
            Archetype::Hat => get!("prop.archetype.hat"),
            Archetype::Obstacle => get!("prop.archetype.obstacle"),
            Archetype::Breakable => get!("prop.archetype.breakable"),
            Archetype::Pickup => get!("prop.archetype.pickup"),
        }
    }

    /// Placed in a map rather than carried.
    pub fn is_placed(self) -> bool {
        matches!(self, Archetype::Obstacle | Archetype::Breakable | Archetype::Pickup)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BreakableSpec {
    pub health: u32,
}

impl Default for BreakableSpec {
    fn default() -> Self {
        Self { health: 100 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickupKind {
    #[default]
    Health,
    Ammo,
    /// Specced, not yet built: there is no armor pool.
    Armor,
    /// Specced, not yet built: there is no capture-the-flag mode.
    Flag,
}

impl PickupKind {
    pub const ALL: [PickupKind; 4] =
        [PickupKind::Health, PickupKind::Ammo, PickupKind::Armor, PickupKind::Flag];

    pub fn name(self) -> String {
        match self {
            PickupKind::Health => get!("prop.pickup.health"),
            PickupKind::Ammo => get!("prop.pickup.ammo"),
            PickupKind::Armor => get!("prop.pickup.armor"),
            PickupKind::Flag => get!("prop.pickup.flag"),
        }
    }
}

/// How a pickup stands in the world. Only the drawing differs: where it is
/// placed, and what it is taken through, are the same either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickupStyle {
    /// Sits on the floor, still.
    #[default]
    Quake,
    /// Hovers a little above the floor, turning.
    Tf2,
}

impl PickupStyle {
    pub const ALL: [PickupStyle; 2] = [PickupStyle::Quake, PickupStyle::Tf2];

    pub fn name(self) -> String {
        match self {
            PickupStyle::Quake => get!("prop.pickup.styles.quake"),
            PickupStyle::Tf2 => get!("prop.pickup.styles.tf2"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PickupSpec {
    pub kind: PickupKind,
    #[serde(default)]
    pub style: PickupStyle,
    /// A fraction of the taker's maximum, so one pack means the same to every
    /// class. `1.0` is a full refill.
    #[serde(with = "crate::prop::nice_f32::scalar")]
    pub amount: f32,
    /// Seconds of game time before a taken pickup is back.
    #[serde(with = "crate::prop::nice_f32::scalar")]
    pub respawn: f32,
}

impl Default for PickupSpec {
    fn default() -> Self {
        Self { kind: PickupKind::Health, style: PickupStyle::Quake, amount: 0.5, respawn: 10.0 }
    }
}

/// How a hat sits on a head. Offset from the crown in the body's frame, `-Z`
/// forward and `+Y` up, so it reads the same whatever the head bone's rest
/// rotation is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HatSpec {
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub offset: [f32; 3],
    /// Euler angles in axis order.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub rotation: [f32; 3],
    /// Keyed by `Class::key`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub per_class: BTreeMap<String, HatOverride>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HatOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f32>,
}

/// One class's resolved fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HatFit {
    pub offset: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: f32,
}

impl Default for HatFit {
    fn default() -> Self {
        Self { offset: [0.0; 3], rotation: [0.0; 3], scale: 1.0 }
    }
}

impl HatFit {
    pub fn transform(&self) -> Transform {
        Transform::from_translation(Vec3::from_array(self.offset))
            .with_rotation(quat_from_euler(Vec3::from_array(self.rotation)))
            .with_scale(Vec3::splat(self.scale))
    }
}

impl HatSpec {
    pub fn for_class(&self, class: Option<Class>) -> HatFit {
        let base = HatFit { offset: self.offset, rotation: self.rotation, scale: 1.0 };
        let Some(over) = class.and_then(|class| self.per_class.get(&Class::key(class))) else {
            return base;
        };
        HatFit {
            offset: over.offset.unwrap_or(base.offset),
            rotation: over.rotation.unwrap_or(base.rotation),
            scale: over.scale.unwrap_or(base.scale),
        }
    }

    /// `edited` written back as `class` sees it; an override keeps only what
    /// differs from the base.
    pub fn applied(&self, class: Option<Class>, edited: &HatFit) -> HatSpec {
        let key = class.map(Class::key).filter(|key| self.per_class.contains_key(key));
        let mut applied = self.clone();
        let Some(key) = key else {
            applied.offset = edited.offset;
            applied.rotation = edited.rotation;
            return applied;
        };
        let differs = |a: [f32; 3], b: [f32; 3]| (a != b).then_some(a);
        applied.per_class.insert(
            key,
            HatOverride {
                offset: differs(edited.offset, self.offset),
                rotation: differs(edited.rotation, self.rotation),
                scale: (edited.scale != 1.0).then_some(edited.scale),
            },
        );
        applied
    }
}

/// The crown, turned with the head: the head bone's tail, and the body's
/// facing carried along by however far the head has turned from rest. In
/// whatever frame `root` is given in.
pub fn crown_frame(skeleton: &Skeleton, pose: &Pose, root: &Transform) -> Option<Transform> {
    let head = skeleton.anatomy().head?;
    let posed = skeleton.posed_bone(pose, root, head);
    let rest = skeleton.posed_bone(&Pose::rest(), root, head);
    let turned = posed.rotation * rest.rotation.inverse();
    Some(Transform::from_translation(posed.tail).with_rotation(turned * root.rotation))
}

/// Where a hat sits, in `root`'s frame.
pub fn hat_transform(
    skeleton: &Skeleton,
    pose: &Pose,
    root: &Transform,
    fit: &HatFit,
) -> Option<Transform> {
    crown_frame(skeleton, pose, root).map(|crown| crown.mul_transform(fit.transform()))
}

#[cfg(test)]
mod tests {
    use crate::common::skeleton::rig::bone;
    use super::*;
    use crate::common::skeleton::default_humanoid;

    /// The hat's frame is the body's, not the head bone's: a bare hat at rest
    /// faces the way the body walks.
    #[test]
    fn a_hat_at_rest_faces_forward_on_top_of_the_head() {
        let skeleton = default_humanoid();
        let root = Transform::IDENTITY;
        let crown = crown_frame(&skeleton, &Pose::rest(), &root).unwrap();
        assert!(crown.rotation.abs_diff_eq(Quat::IDENTITY, 1e-5), "{:?}", crown.rotation);

        let head = skeleton.index_of(bone::HEAD).unwrap();
        let top = skeleton.posed_bone(&Pose::rest(), &root, head).tail;
        assert!((crown.translation - top).length() < 1e-5);
        assert!(crown.translation.y > 1.5, "the crown is not at head height");
    }

    #[test]
    fn a_hat_turns_with_the_body() {
        let skeleton = default_humanoid();
        let root = Transform::from_rotation(Quat::from_rotation_y(1.0));
        let crown = crown_frame(&skeleton, &Pose::rest(), &root).unwrap();
        assert!(crown.rotation.abs_diff_eq(root.rotation, 1e-5));
    }

    #[test]
    fn a_class_override_keeps_only_what_differs() {
        let mut spec = HatSpec { offset: [0.0, 0.01, 0.0], ..default() };
        spec.per_class.insert(Class::key(Class::Heavy), HatOverride::default());
        let mut fit = spec.for_class(Some(Class::Heavy));
        fit.scale = 1.2;
        let applied = spec.applied(Some(Class::Heavy), &fit);
        let over = &applied.per_class[&Class::key(Class::Heavy)];
        assert_eq!(over.scale, Some(1.2));
        assert_eq!(over.offset, None, "an unchanged offset was pinned");
        assert_eq!(applied.for_class(Some(Class::Scout)).scale, 1.0);
    }
}
