//! Closed 2D loops in a sketch plane, which [`super::solid`] extrudes along
//! the plane's normal or revolves about its vertical.
//!
//! A circle is an n-gon; there is no curve type, deliberately — the look is
//! faceted and the side count is the triangle budget.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::rotation::quat_from_euler;

pub const MIN_SIDES: u32 = 3;

/// The most steps a revolve may take: a low-poly game's ceiling, and the
/// triangle budget a turned part can cost.
pub const MAX_REVOLVE_STEPS: u32 = 24;

/// How many steps a revolve asked for `requested` takes.
pub fn revolve_steps(requested: u32) -> u32 {
    requested.clamp(MIN_SIDES, MAX_REVOLVE_STEPS)
}

/// How many sides a request for `requested` gets: raised to [`MIN_SIDES`]
/// rather than refused, so a file asking for fewer still builds.
pub fn sides(requested: u32) -> u32 {
    requested.max(MIN_SIDES)
}

/// A sketch plane. Its normal is local `+Z`, the direction an extrusion
/// travels; a revolve spins about local `+Y`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// In the prop's own space.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub origin: [f32; 3],
    /// Euler angles in radians, in axis order, as [`crate::common::rotation`]
    /// defines them.
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

    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.origin()).with_rotation(self.rotation())
    }
}

/// A closed loop, in sketch coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Profile {
    /// Centred on the sketch origin, `half` each way.
    Rect {
        #[serde(with = "crate::prop::nice_f32::array")]
        half: [f32; 2],
    },
    /// `radius` is to the corners, not the flats.
    Ngon {
        sides: u32,
        #[serde(with = "crate::prop::nice_f32::scalar")]
        radius: f32,
    },
    /// Either winding; see [`Profile::wound_ccw`].
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
    pub fn loop_of(points: &[Vec2]) -> Profile {
        Profile::Points { points: points.iter().map(|p| p.to_array()).collect() }
    }

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
                let sides = self::sides(*sides);
                (0..sides)
                    .map(|i| {
                        // A quarter turn in, so an even-sided n-gon rests on a
                        // flat rather than balancing on a corner.
                        let angle = std::f32::consts::FRAC_PI_2
                            + std::f32::consts::TAU * i as f32 / sides as f32;
                        Vec2::new(radius * angle.cos(), radius * angle.sin())
                    })
                    .collect()
            }
            Profile::Points { points } => points.iter().map(|p| Vec2::from_array(*p)).collect(),
        }
    }

    /// Twice the signed area, positive when counter-clockwise.
    pub fn signed_area_x2(&self) -> f32 {
        let points = self.points();
        let mut total = 0.0;
        for i in 0..points.len() {
            let j = (i + 1) % points.len();
            total += points[i].x * points[j].y - points[j].x * points[i].y;
        }
        total
    }

    pub fn wound_ccw(&self) -> Vec<Vec2> {
        let mut points = self.points();
        if self.signed_area_x2() < 0.0 {
            points.reverse();
        }
        points
    }
}

/// Ear clipping. The kernel needs convex polygons, and a fan is wrong for any
/// profile with a notch. On a self-intersecting loop it returns what it has
/// rather than panicking mid-edit.
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

    // Each pass must remove an ear, so a pass that removes none never will.
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

    #[test]
    fn the_built_in_profiles_are_wound_counter_clockwise() {
        assert!(Profile::Rect { half: [1.0, 2.0] }.signed_area_x2() > 0.0);
        assert!(Profile::Ngon { sides: 8, radius: 1.0 }.signed_area_x2() > 0.0);
    }

    #[test]
    fn an_ngon_measures_its_radius_to_a_corner() {
        let points = Profile::Ngon { sides: 6, radius: 2.0 }.points();
        assert_eq!(points.len(), 6);
        for point in points {
            assert!((point.length() - 2.0).abs() < 1e-5, "{point} is not 2m out");
        }
    }

    /// Clamped rather than refused, so a drag box can pass through 2.
    #[test]
    fn an_ngon_cannot_be_given_fewer_than_three_sides() {
        assert_eq!(Profile::Ngon { sides: 1, radius: 1.0 }.points().len(), 3);
    }

    /// Fails if a notched cap is not covered exactly once.
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
