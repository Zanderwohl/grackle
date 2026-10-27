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
use crate::common::skeleton::rig::bone;
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

    /// Placed in a map rather than carried; these have colliders.
    pub fn is_placed(self) -> bool {
        matches!(self, Archetype::Obstacle | Archetype::Breakable | Archetype::Pickup)
    }

    /// Whether its colliders block movement. A pickup's are only a trigger.
    pub fn blocks(self) -> bool {
        matches!(self, Archetype::Obstacle | Archetype::Breakable)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ColliderShape {
    Box {
        #[serde(with = "crate::prop::nice_f32::array")]
        size: [f32; 3],
    },
    Sphere {
        #[serde(with = "crate::prop::nice_f32::scalar")]
        radius: f32,
    },
    /// Upright. `height` is end to end, caps included.
    Capsule {
        #[serde(with = "crate::prop::nice_f32::scalar")]
        radius: f32,
        #[serde(with = "crate::prop::nice_f32::scalar")]
        height: f32,
    },
}

impl ColliderShape {
    pub fn name(&self) -> String {
        match self {
            ColliderShape::Box { .. } => get!("prop.collider.box"),
            ColliderShape::Sphere { .. } => get!("prop.collider.sphere"),
            ColliderShape::Capsule { .. } => get!("prop.collider.capsule"),
        }
    }

    pub fn defaults() -> [ColliderShape; 3] {
        [
            ColliderShape::Box { size: [0.5; 3] },
            ColliderShape::Sphere { radius: 0.25 },
            ColliderShape::Capsule { radius: 0.2, height: 1.0 },
        ]
    }
}

/// Axis-aligned in the prop's own frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collider {
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub origin: [f32; 3],
    pub shape: ColliderShape,
}

impl Collider {
    /// The shape as a core box plus a radius: the one form the collision world
    /// answers for.
    pub fn rounded(&self) -> RoundedBox {
        let centre = Vec3::from_array(self.origin);
        match self.shape {
            ColliderShape::Box { size } => RoundedBox {
                centre,
                core: Vec3::from_array(size).abs() * 0.5,
                radius: 0.0,
            },
            ColliderShape::Sphere { radius } => {
                RoundedBox { centre, core: Vec3::ZERO, radius: radius.max(0.0) }
            }
            ColliderShape::Capsule { radius, height } => {
                let radius = radius.max(0.0);
                RoundedBox {
                    centre,
                    core: Vec3::new(0.0, (height * 0.5 - radius).max(0.0), 0.0),
                    radius,
                }
            }
        }
    }
}

/// A box grown by a radius in every direction (a Minkowski sum). A box has
/// radius zero, a sphere a zero core, an upright capsule a core that is only
/// tall.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RoundedBox {
    pub centre: Vec3,
    /// Half extents of the core.
    pub core: Vec3,
    pub radius: f32,
}

impl RoundedBox {
    /// Placed in the world. Turned, the core becomes the bounding box of its
    /// turned self: exact for quarter turns, generous otherwise.
    pub fn placed(&self, at: &Transform) -> RoundedBox {
        let rotation = Mat3::from_quat(at.rotation);
        let abs = Mat3::from_cols(rotation.x_axis.abs(), rotation.y_axis.abs(), rotation.z_axis.abs());
        let scale = at.scale.abs();
        RoundedBox {
            centre: at.transform_point(self.centre),
            core: abs * (self.core * scale),
            radius: self.radius * scale.max_element(),
        }
    }

    pub fn half_extents(&self) -> Vec3 {
        self.core + Vec3::splat(self.radius)
    }

    /// How far the box `(centre, half)` is from the core on each axis, zero
    /// where they overlap.
    fn gaps(&self, centre: Vec3, half: Vec3) -> Vec3 {
        ((centre - self.centre).abs() - half - self.core).max(Vec3::ZERO)
    }

    /// Strict, like the wall test: touching is not overlapping.
    pub fn overlaps(&self, centre: Vec3, half: Vec3) -> bool {
        let gaps = self.gaps(centre, half);
        if self.radius <= 0.0 {
            return (0..3).all(|axis| {
                (centre[axis] - self.centre[axis]).abs() < half[axis] + self.core[axis]
            });
        }
        gaps.length_squared() < self.radius * self.radius
    }

    /// Where along `axis` this shape stands, as a slab `(min, max)`, for a box
    /// sitting where `centre` puts it on the other two axes. `None` when the
    /// box would pass it by. What depenetration escapes along, axis by axis.
    pub fn slab_along(&self, axis: usize, centre: Vec3, half: Vec3) -> Option<(f32, f32)> {
        let mut gaps = self.gaps(centre, half);
        gaps[axis] = 0.0;
        let reach = if self.radius <= 0.0 {
            // Must overlap outright on the other axes, as a wall slab must.
            let clear = (0..3).filter(|other| *other != axis).any(|other| {
                (centre[other] - self.centre[other]).abs() >= half[other] + self.core[other]
            });
            if clear {
                return None;
            }
            0.0
        } else {
            let left = self.radius * self.radius - gaps.length_squared();
            if left <= 0.0 {
                return None;
            }
            left.sqrt()
        };
        let extent = self.core[axis] + reach;
        Some((self.centre[axis] - extent, self.centre[axis] + extent))
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
    /// Specced, not yet built: there is no armour pool.
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
    let head = skeleton.index_of(bone::HEAD)?;
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
    use super::*;
    use crate::common::skeleton::default_humanoid;

    fn unit_box_at(centre: Vec3) -> RoundedBox {
        RoundedBox { centre, core: Vec3::splat(0.5), radius: 0.0 }
    }

    #[test]
    fn a_box_overlaps_only_what_it_actually_reaches() {
        let shape = unit_box_at(Vec3::ZERO);
        assert!(shape.overlaps(Vec3::new(0.9, 0.0, 0.0), Vec3::splat(0.5)));
        assert!(!shape.overlaps(Vec3::new(1.0, 0.0, 0.0), Vec3::splat(0.5)), "touching is not inside");
    }

    /// A box diagonally off a sphere's centre is further away than either
    /// axis alone says; treating the sphere as its bounding box would catch it.
    #[test]
    fn a_sphere_is_round_at_the_corners() {
        let sphere = RoundedBox { centre: Vec3::ZERO, core: Vec3::ZERO, radius: 1.0 };
        let half = Vec3::splat(0.1);
        assert!(sphere.overlaps(Vec3::new(1.0, 0.0, 0.0), half));
        assert!(!sphere.overlaps(Vec3::new(0.9, 0.9, 0.0), half), "the corner of the bounding box counted");
    }

    #[test]
    fn a_capsule_is_tall_and_round() {
        let capsule = Collider {
            origin: [0.0; 3],
            shape: ColliderShape::Capsule { radius: 0.25, height: 2.0 },
        }
        .rounded();
        assert_eq!(capsule.half_extents(), Vec3::new(0.25, 1.0, 0.25));
    }

    /// The slab narrows the further off-centre the box passes, which is what
    /// lets a body slide round a pillar rather than stopping square against it.
    #[test]
    fn a_rounded_slab_narrows_off_centre() {
        let sphere = RoundedBox { centre: Vec3::ZERO, core: Vec3::ZERO, radius: 1.0 };
        let half = Vec3::ZERO;
        let (_, head_on) = sphere.slab_along(0, Vec3::ZERO, half).unwrap();
        let (_, glancing) = sphere.slab_along(0, Vec3::new(0.0, 0.0, 0.8), half).unwrap();
        assert!((head_on - 1.0).abs() < 1e-6);
        assert!((glancing - 0.6).abs() < 1e-5, "{glancing}");
        assert!(sphere.slab_along(0, Vec3::new(0.0, 0.0, 1.1), half).is_none());
    }

    #[test]
    fn a_quarter_turn_swaps_a_boxs_sides() {
        let long = RoundedBox { centre: Vec3::X, core: Vec3::new(2.0, 0.5, 0.25), radius: 0.0 };
        let placed = long.placed(&Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)));
        assert!(placed.core.abs_diff_eq(Vec3::new(0.25, 0.5, 2.0), 1e-5), "{:?}", placed.core);
        assert!(placed.centre.abs_diff_eq(Vec3::NEG_Z, 1e-5), "{:?}", placed.centre);
    }

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
