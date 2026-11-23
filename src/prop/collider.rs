//! What a placed prop collides as: boxes, spheres and capsules, each placed and turned in the
//! prop, and each playing any of the [`ColliderRole`]s. Made by the `Collider` feature, which
//! makes no body. See [the plan](../../documentation/prop-archetypes.md).

use std::collections::BTreeSet;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::game::collision::{Capsule, PropShape};
use crate::get;
use crate::prop::profile::Placement;
use crate::prop::solid::MIN_EXTENT;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ColliderShape {
    /// Centered on the placement.
    Box {
        #[serde(with = "crate::prop::nice_f32::array")]
        size: [f32; 3],
    },
    Sphere {
        #[serde(with = "crate::prop::nice_f32::scalar")]
        radius: f32,
    },
    /// Along the placement's `Y`. `height` is end to end, caps included, and never less than
    /// the two caps.
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

    /// Named for the picture, not the tool: see `documentation/icons.md`.
    pub fn icon(&self) -> &'static str {
        match self {
            ColliderShape::Box { .. } => "cube_dashed",
            ColliderShape::Sphere { .. } => "sphere_dashed",
            ColliderShape::Capsule { .. } => "capsule_dashed",
        }
    }

    /// What the toolbar makes, in its order.
    pub fn defaults() -> [ColliderShape; 3] {
        [
            ColliderShape::Box { size: [0.5; 3] },
            ColliderShape::Sphere { radius: 0.25 },
            ColliderShape::Capsule { radius: 0.2, height: 1.0 },
        ]
    }

    pub fn half_extents(&self) -> Vec3 {
        match *self {
            ColliderShape::Box { size } => Vec3::from_array(size).abs() / 2.0,
            ColliderShape::Sphere { radius } => Vec3::splat(radius.abs()),
            ColliderShape::Capsule { radius, height } => {
                let radius = radius.abs();
                Vec3::new(radius, (height / 2.0).max(radius), radius)
            }
        }
    }

    /// As [`crate::prop::solid::Shape::axis_is_radial`]: a radial axis grows both ways.
    pub fn axis_is_radial(&self, axis: usize) -> bool {
        match self {
            ColliderShape::Box { .. } => false,
            ColliderShape::Sphere { .. } => true,
            ColliderShape::Capsule { .. } => axis != 1,
        }
    }

    pub fn set_half_extent(&mut self, axis: usize, half: f32) {
        let half = half.max(MIN_EXTENT);
        match self {
            ColliderShape::Box { size } => size[axis] = half * 2.0,
            ColliderShape::Sphere { radius } => *radius = half,
            ColliderShape::Capsule { radius, height } => match axis {
                1 => *height = (half * 2.0).max(*radius * 2.0),
                _ => {
                    *radius = half;
                    *height = height.max(half * 2.0);
                }
            },
        }
    }
}

/// What touching a collider does. Any number at once, none included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColliderRole {
    /// Sets off the prop's own effect: a pickup is taken through it.
    Trigger,
    /// Stops bullets, projectiles, blasts and sight, and is what a breakable is shot through.
    Shots,
    /// Bodies walk into it and stand on it.
    Solid,
    /// Sets a body touching it alight.
    Ignites,
    /// Kills a body touching it outright.
    Kills,
}

impl ColliderRole {
    pub const ALL: [ColliderRole; 5] =
        [ColliderRole::Trigger, ColliderRole::Shots, ColliderRole::Solid, ColliderRole::Ignites, ColliderRole::Kills];

    /// Its place in [`ColliderRole::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> String {
        match self {
            ColliderRole::Trigger => get!("prop.collider.roles.trigger"),
            ColliderRole::Shots => get!("prop.collider.roles.shots"),
            ColliderRole::Solid => get!("prop.collider.roles.solid"),
            ColliderRole::Ignites => get!("prop.collider.roles.ignites"),
            ColliderRole::Kills => get!("prop.collider.roles.kills"),
        }
    }

    pub fn defaults(archetype: crate::prop::archetype::Archetype) -> BTreeSet<ColliderRole> {
        match archetype {
            crate::prop::archetype::Archetype::Pickup => BTreeSet::from([ColliderRole::Trigger]),
            _ => BTreeSet::from([ColliderRole::Shots, ColliderRole::Solid]),
        }
    }
}

/// One collider as the prop's evaluation found it.
#[derive(Clone, Debug, PartialEq)]
pub struct Collider {
    pub shape: ColliderShape,
    pub placement: Placement,
    pub roles: BTreeSet<ColliderRole>,
}

impl Collider {
    pub fn has(&self, role: ColliderRole) -> bool {
        self.roles.contains(&role)
    }

    /// Where it stands in the world, for a prop placed at `at`.
    pub fn placed(&self, at: &Transform) -> PropShape {
        let world = at.mul_transform(self.placement.transform());
        let stretch = world.scale.abs();
        match self.shape {
            ColliderShape::Box { size } => {
                PropShape::turned_box(world.translation, Vec3::from_array(size).abs() / 2.0 * stretch, world.rotation)
            }
            ColliderShape::Sphere { radius } => PropShape::sphere(world.translation, radius.abs() * stretch.max_element()),
            ColliderShape::Capsule { radius, height } => {
                let radius = radius.abs();
                let reach = (height / 2.0 - radius).max(0.0);
                PropShape::Capsule(Capsule {
                    a: world.transform_point(Vec3::NEG_Y * reach),
                    b: world.transform_point(Vec3::Y * reach),
                    radius: radius * stretch.max_element(),
                })
            }
        }
    }

    pub fn color(&self) -> Color {
        let drastic = [ColliderRole::Kills, ColliderRole::Ignites, ColliderRole::Solid, ColliderRole::Shots, ColliderRole::Trigger];
        match drastic.into_iter().find(|role| self.has(*role)) {
            Some(ColliderRole::Kills) => Color::srgb(0.95, 0.15, 0.2),
            Some(ColliderRole::Ignites) => Color::srgb(1.0, 0.8, 0.1),
            Some(ColliderRole::Solid) => Color::srgb(1.0, 0.55, 0.1),
            Some(ColliderRole::Shots) => Color::srgb(0.35, 0.6, 1.0),
            Some(ColliderRole::Trigger) => Color::srgb(0.3, 0.9, 0.5),
            None => Color::srgb(0.6, 0.6, 0.6),
        }
    }

    pub fn draw(&self, gizmos: &mut Gizmos, at: &Transform, color: Color) {
        let world = at.mul_transform(self.placement.transform());
        let stretch = world.scale.abs().max_element();
        match self.shape {
            ColliderShape::Box { size } => {
                let scale = Vec3::from_array(size).abs() * world.scale.abs();
                gizmos.cube(Transform { scale, ..world }, color);
            }
            ColliderShape::Sphere { radius } => {
                gizmos.sphere(Isometry3d::new(world.translation, world.rotation), radius.abs() * stretch, color);
            }
            ColliderShape::Capsule { radius, height } => {
                let radius = radius.abs();
                let length = (height - 2.0 * radius).max(0.0);
                let capsule = Capsule3d::new(radius * stretch, length * stretch);
                gizmos.primitive_3d(&capsule, Isometry3d::new(world.translation, world.rotation), color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    /// Lying along the prop's `X` in a prop turned a quarter turn about `Y`, it lies along `Z`.
    #[test]
    fn a_capsule_turns_with_its_placement_and_its_prop() {
        let collider = Collider {
            shape: ColliderShape::Capsule { radius: 0.2, height: 2.0 },
            placement: Placement { origin: [0.0, 0.5, 0.0], rotation: [0.0, 0.0, FRAC_PI_2] },
            roles: BTreeSet::from([ColliderRole::Solid]),
        };
        let prop = Transform::from_xyz(10.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(FRAC_PI_2));
        let placed = collider.placed(&prop);
        let half = Vec3::splat(0.1);
        assert!(placed.overlaps(Vec3::new(10.0, 0.5, 0.9), half), "not along Z");
        assert!(!placed.overlaps(Vec3::new(10.9, 0.5, 0.0), half), "still along X");
    }

    #[test]
    fn a_capsule_never_gets_shorter_than_its_caps() {
        let mut shape = ColliderShape::Capsule { radius: 0.2, height: 1.0 };
        shape.set_half_extent(0, 0.8);
        assert_eq!(shape, ColliderShape::Capsule { radius: 0.8, height: 1.6 });
        shape.set_half_extent(1, 0.1);
        assert_eq!(shape, ColliderShape::Capsule { radius: 0.8, height: 1.6 });
    }
}
