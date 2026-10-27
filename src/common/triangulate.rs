//! Ear clipping, for the caps of anything swept from a 2D loop: a prop's
//! extrude and a room's floor and ceiling.

use bevy::prelude::*;

/// Ear clipping. The kernel needs convex polygons, and a fan is wrong for any
/// loop with a notch. On a self-intersecting loop it returns what it has
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
                warn!("a loop could not be triangulated; it probably crosses itself");
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
