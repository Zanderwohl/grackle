//! A moving box against a [`RoundedBox`].
//!
//! The box grown by a rounded box is another rounded box with a bigger core,
//! so the sweep is a point against that: three slabs (the core grown along
//! one axis each), twelve edge cylinders and eight corner spheres. The union
//! is entered where the first of them is. Cylinders are tested on their sides
//! only, because a cap lies inside the corner sphere at its end.

use bevy::prelude::*;

use super::hull::{enter, Hit};
use crate::prop::archetype::RoundedBox;

pub fn sweep(shape: &RoundedBox, centre: Vec3, half: Vec3, delta: Vec3) -> Option<Hit> {
    let core = shape.core + half;
    let r = shape.radius;
    let from = centre - shape.centre;

    let slab = |grow: Vec3| {
        let extent = core + grow;
        let planes = (0..3).flat_map(move |axis| {
            let mut n = Vec3::ZERO;
            n[axis] = 1.0;
            [(n, extent[axis]), (-n, extent[axis])]
        });
        enter(planes, from, delta).map(|hit| Hit { plane: hit.plane + hit.normal.dot(shape.centre), ..hit })
    };
    if r <= 0.0 {
        return slab(Vec3::ZERO);
    }
    if distance_to_core(from, core) < r {
        return None;
    }

    let mut best: Option<f32> = None;
    let mut consider = |t: Option<f32>| {
        if let Some(t) = t.filter(|t| (0.0..1.0).contains(t)) {
            if best.is_none_or(|b| t < b) {
                best = Some(t);
            }
        }
    };

    for axis in 0..3 {
        let mut grow = Vec3::ZERO;
        grow[axis] = r;
        consider(slab(grow).map(|hit| hit.t));
    }
    for along in 0..3 {
        let (i, j) = ((along + 1) % 3, (along + 2) % 3);
        for (si, sj) in [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)] {
            let at = Vec2::new(from[i] - si * core[i], from[j] - sj * core[j]);
            let moving = Vec2::new(delta[i], delta[j]);
            consider(first_root(moving.length_squared(), at.dot(moving), at.length_squared() - r * r).filter(|t| {
                (from[along] + delta[along] * t).abs() <= core[along]
            }));
        }
    }
    for corner in 0..8 {
        let sign = Vec3::new(
            if corner & 1 == 0 { -1.0 } else { 1.0 },
            if corner & 2 == 0 { -1.0 } else { 1.0 },
            if corner & 4 == 0 { -1.0 } else { 1.0 },
        );
        let at = from - sign * core;
        consider(first_root(delta.length_squared(), at.dot(delta), at.length_squared() - r * r));
    }

    let t = best?;
    let reached = from + delta * t;
    let normal = (reached - reached.clamp(-core, core))
        .try_normalize()
        .unwrap_or_else(|| (-delta).normalize_or_zero());
    Some(Hit { t, normal, plane: normal.dot(centre + delta * t) })
}

fn distance_to_core(point: Vec3, core: Vec3) -> f32 {
    (point.abs() - core).max(Vec3::ZERO).length()
}

/// The entering root of `a t² + 2 b t + c = 0`, for a point starting on or
/// outside the surface (`c ≥ 0`). A graze (one root) is not an entry.
fn first_root(a: f32, b: f32, c: f32) -> Option<f32> {
    if a == 0.0 || c < 0.0 {
        return None;
    }
    let disc = b * b - a * c;
    if disc <= 0.0 {
        return None;
    }
    Some((-b - disc.sqrt()) / a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_coming_in_diagonally_meets_a_sphere_at_its_corner() {
        let sphere = RoundedBox { centre: Vec3::ZERO, core: Vec3::ZERO, radius: 1.0 };
        let half = Vec3::splat(0.5);
        let start = Vec3::new(5.0, 5.0, 0.0);
        let hit = sweep(&sphere, start, half, Vec3::new(-10.0, -10.0, 0.0)).expect("passed through");

        let reached = start + Vec3::new(-10.0, -10.0, 0.0) * hit.t;
        let expected = 0.5 + 1.0 / 2f32.sqrt();
        assert!((reached.x - expected).abs() < 1e-4, "{reached}");
        assert!(hit.normal.abs_diff_eq(Vec3::new(1.0, 1.0, 0.0).normalize(), 1e-4), "{}", hit.normal);
    }

    #[test]
    fn a_box_meets_a_capsule_on_its_side_at_its_radius() {
        let pillar = RoundedBox { centre: Vec3::ZERO, core: Vec3::new(0.0, 2.0, 0.0), radius: 0.5 };
        let hit = sweep(&pillar, Vec3::new(-5.0, 0.0, 0.0), Vec3::splat(0.25), Vec3::X * 10.0).unwrap();
        assert!((-5.0 + 10.0 * hit.t + 0.75).abs() < 1e-4);
        assert!(hit.normal.abs_diff_eq(Vec3::NEG_X, 1e-5));
    }

    #[test]
    fn a_box_passing_a_sphere_by_misses_it() {
        let sphere = RoundedBox { centre: Vec3::ZERO, core: Vec3::ZERO, radius: 1.0 };
        // Inside the grown bounding box's corner, outside the rounding.
        assert_eq!(sweep(&sphere, Vec3::new(-5.0, 1.3, 1.3), Vec3::splat(0.5), Vec3::X * 10.0), None);
    }
}
