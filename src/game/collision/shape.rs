//! A placed prop's collider, turned however it was placed: a box as a [`Hull`], or a
//! [`Capsule`], which is a sphere when its ends meet.
//!
//! A box moving against a capsule is a point moving against the capsule's segment grown by the
//! box (`core`) and then by the radius. The core's planes pushed out by the radius reach further
//! than that shape at its edges and corners, so an entry through them counts only where it lands
//! within the radius of the core. Any other entry is through an edge cylinder or a corner sphere.

use bevy::prelude::*;

use super::hull::{enter, onto_plane, Hit, Hull};

/// Meters within which a point on a shape's surface counts as inside it, for a ray leaving it.
const ON_SURFACE: f32 = 1.0e-4;

#[derive(Clone, Debug, PartialEq)]
pub enum PropShape {
    Hull(Hull),
    Capsule(Capsule),
}

/// Every point within `radius` of the segment `a..b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capsule {
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f32,
}

impl PropShape {
    /// A box of half extents `half`, turned by `rotation` about its center.
    pub fn turned_box(center: Vec3, half: Vec3, rotation: Quat) -> PropShape {
        let axes = [rotation * Vec3::X, rotation * Vec3::Y, rotation * Vec3::Z];
        let half = half.abs();
        let corners: Vec<Vec3> = (0..8)
            .map(|i| {
                let sign = |bit: usize| if i & bit == 0 { -1.0 } else { 1.0 };
                center + axes[0] * half.x * sign(1) + axes[1] * half.y * sign(2) + axes[2] * half.z * sign(4)
            })
            .collect();
        let faces = [axes[0], -axes[0], axes[1], -axes[1], axes[2], -axes[2]];
        PropShape::Hull(Hull::new(&corners, &faces, &axes))
    }

    pub fn sphere(center: Vec3, radius: f32) -> PropShape {
        PropShape::Capsule(Capsule { a: center, b: center, radius: radius.max(0.0) })
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        match self {
            PropShape::Hull(hull) => hull.bounds(),
            PropShape::Capsule(capsule) => {
                let r = Vec3::splat(capsule.radius);
                (capsule.a.min(capsule.b) - r, capsule.a.max(capsule.b) + r)
            }
        }
    }

    /// A box already inside is not hit; one touching and moving in is hit at zero.
    pub fn sweep(&self, center: Vec3, half: Vec3, delta: Vec3) -> Option<Hit> {
        match self {
            PropShape::Hull(hull) => hull.sweep(center, half, delta),
            PropShape::Capsule(capsule) => capsule.sweep(center, half, delta),
        }
    }

    /// Strict: touching is not overlapping.
    pub fn overlaps(&self, center: Vec3, half: Vec3) -> bool {
        match self {
            PropShape::Hull(hull) => hull.overlaps(center, half),
            PropShape::Capsule(capsule) => capsule.distance_squared(center, half).0 < capsule.radius * capsule.radius,
        }
    }

    /// Where a box overlapping this shape could go to be just clear of it, `skin` beyond it.
    pub fn escapes(&self, center: Vec3, half: Vec3, skin: f32) -> Vec<Vec3> {
        match self {
            PropShape::Hull(hull) => hull.escapes(center, half, skin).collect(),
            PropShape::Capsule(capsule) => capsule.escapes(center, half, skin),
        }
    }

    /// A ray starting inside or on the shape passes out of it, so a blast on a crate's face is not
    /// stopped by the crate.
    pub fn ray(&self, ray: &Ray3d, range: f32) -> Option<(f32, Vec3)> {
        if self.overlaps(ray.origin, Vec3::splat(ON_SURFACE)) {
            return None;
        }
        match self {
            PropShape::Hull(hull) => hull.ray(ray, range),
            PropShape::Capsule(capsule) => {
                let hit = capsule.sweep(ray.origin, Vec3::ZERO, *ray.direction * range)?;
                Some((hit.t * range, hit.normal))
            }
        }
    }
}

impl Capsule {
    /// The squared distance from the box at `center` to the segment, and the parameter (0 to 1)
    /// of the nearest point. Each axis adds `max(|x| - h, 0)²`, so the sum is convex and one
    /// quadratic between the parameters where an axis reaches a face of the box.
    fn distance_squared(&self, center: Vec3, half: Vec3) -> (f32, f32) {
        let p = self.a - center;
        let d = self.b - self.a;
        let mut breaks = vec![0.0, 1.0];
        for axis in 0..3 {
            if d[axis] != 0.0 {
                for face in [half[axis], -half[axis]] {
                    let s = (face - p[axis]) / d[axis];
                    if s > 0.0 && s < 1.0 {
                        breaks.push(s);
                    }
                }
            }
        }
        breaks.sort_by(f32::total_cmp);

        let at = |s: f32| -> f32 {
            (0..3).map(|axis| ((p[axis] + s * d[axis]).abs() - half[axis]).max(0.0).powi(2)).sum()
        };
        let mut best = (at(0.0), 0.0);
        for span in breaks.windows(2) {
            let (low, high) = (span[0], span[1]);
            let middle = (low + high) * 0.5;
            let (mut slope, mut weight) = (0.0, 0.0);
            for axis in 0..3 {
                let x = p[axis] + middle * d[axis];
                let offset = match x {
                    x if x > half[axis] => p[axis] - half[axis],
                    x if x < -half[axis] => p[axis] + half[axis],
                    _ => continue,
                };
                slope += offset * d[axis];
                weight += d[axis] * d[axis];
            }
            let s = match weight > 0.0 {
                true => (-slope / weight).clamp(low, high),
                false => low,
            };
            for s in [s, high] {
                let value = at(s);
                if value < best.0 {
                    best = (value, s);
                }
            }
        }
        best
    }

    /// The point of `core(half)` nearest `point`.
    fn nearest_on_core(&self, point: Vec3, half: Vec3) -> Vec3 {
        let (_, s) = self.distance_squared(point, half);
        let on_segment = self.a.lerp(self.b, s);
        on_segment + (point - on_segment).clamp(-half, half)
    }

    fn core(&self, half: Vec3) -> Hull {
        let corners: Vec<Vec3> = [self.a, self.b]
            .into_iter()
            .flat_map(|end| (0..8).map(move |i| end + corner_sign(i) * half))
            .collect();
        Hull::new(&corners, &[], &[self.b - self.a])
    }

    fn sweep(&self, center: Vec3, half: Vec3, delta: Vec3) -> Option<Hit> {
        let r = self.radius;
        if self.distance_squared(center, half).0 < r * r {
            return None;
        }
        let core = self.core(half);
        let landed = |t: f32| {
            let reached = center + delta * t;
            let distance = self.distance_squared(reached, half).0.sqrt();
            distance <= r + ON_SURFACE * (1.0 + r)
        };
        if let Some(hit) = enter(core.planes().iter().map(|&(n, w)| (n, w + r)), center, delta)
            && landed(hit.t)
        {
            return Some(hit);
        }
        if r <= 0.0 {
            return None;
        }

        let mut best: Option<f32> = None;
        let mut consider = |t: Option<f32>| {
            if let Some(t) = t.filter(|t| (0.0..1.0).contains(t))
                && best.is_none_or(|b| t < b)
            {
                best = Some(t);
            }
        };
        let ends: &[Vec3] = if self.a == self.b { &[self.a] } else { &[self.a, self.b] };
        for &end in ends {
            for corner in 0..8 {
                let at = center - (end + corner_sign(corner) * half);
                consider(first_root(delta.length_squared(), at.dot(delta), at.length_squared() - r * r));
                // The box's edges at this end, each once: from the corners with the bit clear.
                for axis in 0..3 {
                    if corner & (1 << axis) == 0 {
                        let mut edge = Vec3::ZERO;
                        edge[axis] = half[axis] * 2.0;
                        consider(cylinder(center, end + corner_sign(corner) * half, edge, delta, r));
                    }
                }
            }
        }
        if self.a != self.b {
            for corner in 0..8 {
                consider(cylinder(center, self.a + corner_sign(corner) * half, self.b - self.a, delta, r));
            }
        }

        let t = best?;
        let reached = center + delta * t;
        let normal = (reached - self.nearest_on_core(reached, half))
            .try_normalize()
            .unwrap_or_else(|| (-delta).normalize_or_zero());
        Some(Hit { t, normal, plane: normal.dot(reached) })
    }

    fn escapes(&self, center: Vec3, half: Vec3, skin: f32) -> Vec<Vec3> {
        let r = self.radius;
        let mut escapes = Vec::new();
        let away = center - self.nearest_on_core(center, half);
        // Along the distance's gradient, which grows the distance by at least the step.
        if let Some(direction) = away.try_normalize() {
            escapes.push(center + direction * (r + skin - away.length()));
        }
        escapes.extend(self.core(half).planes().iter().map(|&(n, w)| onto_plane(center, n, w + r + skin)));
        escapes
    }
}

fn corner_sign(corner: usize) -> Vec3 {
    let sign = |bit: usize| if corner & bit == 0 { -1.0 } else { 1.0 };
    Vec3::new(sign(1), sign(2), sign(4))
}

/// A point at `from` moving by `delta` against the cylinder of radius `r` round the segment
/// `start..start + edge`, entered through its side.
fn cylinder(from: Vec3, start: Vec3, edge: Vec3, delta: Vec3, r: f32) -> Option<f32> {
    let length = edge.length();
    let along = edge.try_normalize()?;
    let at = from - start;
    let across = at - along * at.dot(along);
    let moving = delta - along * delta.dot(along);
    let t = first_root(moving.length_squared(), across.dot(moving), across.length_squared() - r * r)?;
    let reached = (at + delta * t).dot(along);
    (0.0..=length).contains(&reached).then_some(t)
}

/// The entering root of `a t² + 2 b t + c = 0`, for a point starting on or outside the surface
/// (`c ≥ 0`). A graze (one root) is not an entry.
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
    use std::f32::consts::FRAC_PI_4;

    const HALF: Vec3 = Vec3::splat(0.5);

    #[test]
    fn a_box_coming_in_diagonally_meets_a_sphere_at_its_corner() {
        let sphere = PropShape::sphere(Vec3::ZERO, 1.0);
        let start = Vec3::new(5.0, 5.0, 0.0);
        let delta = Vec3::new(-10.0, -10.0, 0.0);
        let hit = sphere.sweep(start, HALF, delta).expect("passed through");

        let reached = start + delta * hit.t;
        let expected = 0.5 + 1.0 / 2f32.sqrt();
        assert!((reached.x - expected).abs() < 1e-4, "{reached}");
        assert!(hit.normal.abs_diff_eq(Vec3::new(1.0, 1.0, 0.0).normalize(), 1e-4), "{}", hit.normal);
    }

    #[test]
    fn a_box_passing_a_sphere_by_misses_it() {
        let sphere = PropShape::sphere(Vec3::ZERO, 1.0);
        // Inside the grown bounding box's corner, outside the rounding.
        assert_eq!(sphere.sweep(Vec3::new(-5.0, 1.3, 1.3), HALF, Vec3::X * 10.0), None);
        assert!(!sphere.overlaps(Vec3::new(0.0, 1.3, 1.3), HALF));
    }

    #[test]
    fn a_box_meets_an_upright_capsule_on_its_side_at_its_radius() {
        let pillar = PropShape::Capsule(Capsule { a: Vec3::NEG_Y * 2.0, b: Vec3::Y * 2.0, radius: 0.5 });
        let hit = pillar.sweep(Vec3::new(-5.0, 0.0, 0.0), Vec3::splat(0.25), Vec3::X * 10.0).unwrap();
        assert!((-5.0 + 10.0 * hit.t + 0.75).abs() < 1e-4);
        assert!(hit.normal.abs_diff_eq(Vec3::NEG_X, 1e-5));
    }

    /// A capsule lying along a diagonal: a box falling onto its middle lands on its top, and one
    /// falling beside the diagonal, inside its bounding box, falls past.
    #[test]
    fn a_turned_capsule_is_round_along_its_own_length() {
        let lying = PropShape::Capsule(Capsule { a: Vec3::new(-3.0, 0.0, -3.0), b: Vec3::new(3.0, 0.0, 3.0), radius: 0.5 });
        let hit = lying.sweep(Vec3::new(1.0, 5.0, 1.0), Vec3::splat(0.25), Vec3::NEG_Y * 10.0).expect("fell through");
        assert!((5.0 - 10.0 * hit.t - 0.75).abs() < 1e-4, "{}", hit.t);
        assert!(hit.normal.abs_diff_eq(Vec3::Y, 1e-4), "{}", hit.normal);

        assert_eq!(lying.sweep(Vec3::new(2.0, 5.0, -2.0), Vec3::splat(0.25), Vec3::NEG_Y * 10.0), None);
    }

    /// A box falling past the end of a capsule lands on the rounded cap, lower than the square
    /// corner of the grown planes would have caught it.
    #[test]
    fn a_capsules_end_is_rounded() {
        let rod = PropShape::Capsule(Capsule { a: Vec3::ZERO, b: Vec3::X * 2.0, radius: 1.0 });
        let start = Vec3::new(3.0, 5.0, 0.0);
        let hit = rod.sweep(start, Vec3::splat(0.25), Vec3::NEG_Y * 10.0).expect("fell past the cap");
        // The box's corner (2.75, y - 0.25) meets the cap round (2, 0) at radius 1.
        let expected = 0.25 + (1.0f32 - 0.75 * 0.75).sqrt();
        assert!((5.0 - 10.0 * hit.t - expected).abs() < 1e-4, "{}", 5.0 - 10.0 * hit.t);
    }

    #[test]
    fn a_turned_box_stops_a_box_at_its_slanted_face() {
        let turned = PropShape::turned_box(Vec3::ZERO, Vec3::splat(1.0), Quat::from_rotation_y(FRAC_PI_4));
        let hit = turned.sweep(Vec3::new(-5.0, 0.0, 0.0), HALF, Vec3::X * 10.0).unwrap();
        let reached = -5.0 + 10.0 * hit.t;
        // The turned box's corner reaches √2 along X; the moving box's face meets it.
        assert!((reached + 2f32.sqrt() + 0.5).abs() < 1e-4, "{reached}");
    }

    #[test]
    fn a_body_inside_a_capsule_escapes_the_shortest_way() {
        let pillar = PropShape::Capsule(Capsule { a: Vec3::ZERO, b: Vec3::Y * 2.0, radius: 0.5 });
        let inside = Vec3::new(0.6, 1.0, 0.0);
        assert!(pillar.overlaps(inside, Vec3::splat(0.25)));
        let nearest = pillar
            .escapes(inside, Vec3::splat(0.25), 0.001)
            .into_iter()
            .min_by(|a, b| (*a - inside).length().total_cmp(&(*b - inside).length()))
            .unwrap();
        assert!(!pillar.overlaps(nearest, Vec3::splat(0.25)));
        assert!((nearest.x - 0.751).abs() < 1e-4, "{nearest}");
    }

    #[test]
    fn a_ray_enters_a_capsule_where_it_crosses_it_and_leaves_one_it_starts_in() {
        let ball = PropShape::sphere(Vec3::X * 5.0, 1.0);
        let (distance, normal) = ball.ray(&Ray3d::new(Vec3::ZERO, Dir3::X), 10.0).unwrap();
        assert!((distance - 4.0).abs() < 1e-4);
        assert!(normal.abs_diff_eq(Vec3::NEG_X, 1e-4));
        assert_eq!(ball.ray(&Ray3d::new(Vec3::X * 5.0, Dir3::X), 10.0), None, "a ray from inside was stopped");
        assert_eq!(ball.ray(&Ray3d::new(Vec3::X * 4.0, Dir3::NEG_X), 10.0), None, "a ray from its face was stopped");
    }

    #[test]
    fn the_distance_to_a_segment_is_its_nearest_sample() {
        let capsule = Capsule { a: Vec3::new(-1.0, 0.3, 2.0), b: Vec3::new(2.5, -1.2, -0.7), radius: 0.0 };
        let half = Vec3::new(0.4, 0.9, 0.2);
        for center in [Vec3::ZERO, Vec3::new(1.0, 2.0, 0.0), Vec3::new(-3.0, 0.0, 1.0), Vec3::new(0.5, -0.4, 0.5)] {
            let (exact, _) = capsule.distance_squared(center, half);
            let sampled = (0..=10_000)
                .map(|i| {
                    let point = capsule.a.lerp(capsule.b, i as f32 / 10_000.0) - center;
                    (point.abs() - half).max(Vec3::ZERO).length_squared()
                })
                .fold(f32::INFINITY, f32::min);
            assert!((exact - sampled).abs() < 1e-3, "{center}: {exact} against {sampled}");
            assert!(exact <= sampled + 1e-6);
        }
    }
}
