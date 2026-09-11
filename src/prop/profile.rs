//! The 2D shapes a solid is swept from, and where the paper they are drawn on
//! sits.
//!
//! A profile is a **closed loop in a sketch plane**, and everything volumetric
//! in [`super::solid`] is one of these either pushed along the plane's normal
//! or spun about its vertical. Keeping the 2D shape as its own value is what
//! makes "extrude this" and "revolve this" the same authoring gesture with a
//! different verb, rather than two unrelated features.
//!
//! **A circle is an n-gon.** There is no curve type and there is not going to
//! be one: the game's look is faceted, and a cylinder with a side count a
//! mapper picked is both the right silhouette and a triangle budget stated out
//! loud. [`Profile::Ngon`] is the cylinder, the hexagonal bolt and the
//! octagonal muzzle, and which of those it is is one number.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::rotation::quat_from_euler;

/// The fewest sides anything round is allowed. Two gives a line and three a
/// triangle; refusing below three means no caller has to check.
pub const MIN_SIDES: u32 = 3;

/// Where a sketch is, and which way up it is.
///
/// The sketch's own X and Y are the plane's; its **normal is local `+Z`**, and
/// that is the direction an extrusion travels. A revolve spins about local
/// `+Y` — the sketch's vertical — which is what makes a profile drawn to the
/// right of the origin sweep into a shape around it.
///
/// Angles are the three Euler components in axis order, radians, the same
/// convention every map feature stores and [`crate::common::rotation`] owns.
/// A prop that turned by one rule in the prop editor and another in a mapper's
/// hands would be a prop nobody could line up against anything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// Where the sketch origin sits, in the prop's own space.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub origin: [f32; 3],
    /// Pitch, yaw and roll in radians, in axis order.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub rotation: [f32; 3],
}

impl Placement {
    pub fn at(x: f32, y: f32, z: f32) -> Self {
        Self { origin: [x, y, z], rotation: [0.0; 3] }
    }

    pub fn origin(&self) -> Vec3 {
        Vec3::from_array(self.origin)
    }

    pub fn rotation(&self) -> Quat {
        quat_from_euler(Vec3::from_array(self.rotation))
    }

    /// A point in the sketch, moved into the prop's space.
    pub fn point(&self, local: Vec3) -> Vec3 {
        self.origin() + self.rotation() * local
    }

    /// The transform a solid built in this placement's local space wants.
    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.origin()).with_rotation(self.rotation())
    }
}

/// A closed loop, in sketch coordinates.
///
/// The three variants are not three shapes so much as three ways of saying one:
/// a rectangle and an n-gon are both a [`Profile::Points`] somebody would have
/// had to type out, kept as their own variants so that changing a radius stays
/// one number rather than a list to re-enter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Profile {
    /// Centred on the sketch origin, `half` each way.
    Rect {
        #[serde(with = "crate::prop::nice_f32::array")]
        half: [f32; 2],
    },
    /// A regular polygon, `radius` to its **corners** rather than to its flats.
    ///
    /// To the corners because that is the number that decides whether it fits
    /// through a hole; a mapper sizing a barrel against a receiver is placing
    /// the widest part.
    Ngon {
        sides: u32,
        #[serde(with = "crate::prop::nice_f32::scalar")]
        radius: f32,
    },
    /// Whatever was typed in, wound counter-clockwise.
    Points {
        #[serde(with = "crate::prop::nice_f32::pairs")]
        points: Vec<[f32; 2]>,
    },
}

impl Default for Profile {
    fn default() -> Self {
        Profile::Rect { half: [0.1, 0.1] }
    }
}

impl Profile {
    /// The loop, counter-clockwise, with no repeated last point.
    pub fn points(&self) -> Vec<Vec2> {
        match self {
            Profile::Rect { half } => {
                let (x, y) = (half[0], half[1]);
                vec![
                    Vec2::new(-x, -y),
                    Vec2::new(x, -y),
                    Vec2::new(x, y),
                    Vec2::new(-x, y),
                ]
            }
            Profile::Ngon { sides, radius } => {
                let sides = (*sides).max(MIN_SIDES);
                (0..sides)
                    .map(|i| {
                        // Started at a quarter turn so an even-sided n-gon
                        // rests on a flat rather than balancing on a corner —
                        // a hexagonal bolt head and a square post both want
                        // their bottom face parallel to the ground.
                        let angle = std::f32::consts::FRAC_PI_2
                            + std::f32::consts::TAU * i as f32 / sides as f32;
                        Vec2::new(radius * angle.cos(), radius * angle.sin())
                    })
                    .collect()
            }
            Profile::Points { points } => points.iter().map(|p| Vec2::from_array(*p)).collect(),
        }
    }

    /// Twice the signed area the loop encloses; positive when it is wound
    /// counter-clockwise.
    ///
    /// The sign is what everything downstream orients itself by, so a
    /// hand-typed profile entered the wrong way round comes out solid rather
    /// than inside out.
    pub fn signed_area_x2(&self) -> f32 {
        let points = self.points();
        let mut total = 0.0;
        for i in 0..points.len() {
            let j = (i + 1) % points.len();
            total += points[i].x * points[j].y - points[j].x * points[i].y;
        }
        total
    }

    /// The loop, guaranteed counter-clockwise.
    pub fn wound_ccw(&self) -> Vec<Vec2> {
        let mut points = self.points();
        if self.signed_area_x2() < 0.0 {
            points.reverse();
        }
        points
    }

    pub fn name(&self) -> String {
        match self {
            Profile::Rect { .. } => crate::get!("prop.profiles.rect"),
            Profile::Ngon { .. } => crate::get!("prop.profiles.ngon"),
            Profile::Points { .. } => crate::get!("prop.profiles.points"),
        }
    }
}

/// Cut a loop into triangles, by ear clipping.
///
/// The CSG kernel only accepts convex polygons, so a cap cannot simply be the
/// profile: a fan from the centroid would be right for a rectangle and wrong
/// for anything with a notch in it, and the failure is a solid with
/// overlapping faces on one end rather than an error.
///
/// Returns indices into `points`, three at a time. Gives up and returns what
/// it has if the loop self-intersects, because a half-capped solid is
/// something you can look at and diagnose, and a panic in the middle of
/// somebody's edit is not.
pub fn triangulate(points: &[Vec2]) -> Vec<[usize; 3]> {
    if points.len() < 3 {
        return vec![];
    }

    let cross = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a);
    let inside = |a: Vec2, b: Vec2, c: Vec2, p: Vec2| {
        cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
    };

    let mut remaining: Vec<usize> = (0..points.len()).collect();
    let mut triangles = Vec::with_capacity(points.len().saturating_sub(2));

    // Each pass round the loop must remove at least one ear, so a pass that
    // removes none is a loop no amount of further looking will fix.
    let mut stalled = 0;
    while remaining.len() > 3 {
        let count = remaining.len();
        let mut clipped = false;
        for offset in 0..count {
            let (ia, ib, ic) = (
                remaining[offset],
                remaining[(offset + 1) % count],
                remaining[(offset + 2) % count],
            );
            let (a, b, c) = (points[ia], points[ib], points[ic]);
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            if remaining
                .iter()
                .filter(|i| **i != ia && **i != ib && **i != ic)
                .any(|i| inside(a, b, c, points[*i]))
            {
                continue;
            }
            triangles.push([ia, ib, ic]);
            remaining.remove((offset + 1) % count);
            clipped = true;
            break;
        }
        if !clipped {
            stalled += 1;
            if stalled > 1 {
                warn!("a profile could not be triangulated; it probably crosses itself");
                return triangles;
            }
        }
    }

    triangles.push([remaining[0], remaining[1], remaining[2]]);
    triangles
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything downstream reads the winding as "which way is out", so a
    /// profile that came back clockwise would build every solid inside out.
    #[test]
    fn the_built_in_profiles_are_wound_counter_clockwise() {
        assert!(Profile::Rect { half: [1.0, 2.0] }.signed_area_x2() > 0.0);
        assert!(Profile::Ngon { sides: 8, radius: 1.0 }.signed_area_x2() > 0.0);
    }

    /// A radius is to the corners, so a square n-gon is the square that
    /// *contains* the circle's diameter on its diagonal, not on its flats.
    #[test]
    fn an_ngon_measures_its_radius_to_a_corner() {
        let points = Profile::Ngon { sides: 6, radius: 2.0 }.points();
        assert_eq!(points.len(), 6);
        for point in points {
            assert!((point.length() - 2.0).abs() < 1e-5, "{point} is not 2m out");
        }
    }

    /// Fewer than three sides is not a shape. Clamping rather than erroring
    /// because the number comes off a drag box, and a box you cannot drag
    /// through 2 on the way to 3 is a box that fights you.
    #[test]
    fn an_ngon_cannot_be_given_fewer_than_three_sides() {
        assert_eq!(Profile::Ngon { sides: 1, radius: 1.0 }.points().len(), 3);
    }

    /// The property the kernel's convexity rule actually depends on: a cap is
    /// triangles, and the triangles cover the profile exactly once.
    #[test]
    fn triangulating_a_notched_profile_covers_its_area_once() {
        // An L: convex-hull area 4, actual area 3.
        let points = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        let triangles = triangulate(&points);
        assert_eq!(triangles.len(), points.len() - 2, "an ear clip leaves n-2 triangles");

        let area: f32 = triangles
            .iter()
            .map(|[a, b, c]| {
                (points[*b] - points[*a]).perp_dot(points[*c] - points[*a]) / 2.0
            })
            .sum();
        assert!(
            (area - 3.0).abs() < 1e-5,
            "the L's 3 square metres triangulated to {area}",
        );
    }
}
