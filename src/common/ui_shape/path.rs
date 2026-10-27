//! Closed outlines built from lines and Bézier curves, flattened to a polygon
//! for [`super::ShapeMaterial`] to draw.
//!
//! Coordinates are logical pixels in the shape's own box, `+Y` down, the way a
//! UI node is laid out. Winding does not matter: the shader measures distance.

use bevy::prelude::*;

/// What one piece of a [`Path`] does after the point before it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Segment {
    Line(Vec2),
    Quadratic(Vec2, Vec2),
    Cubic(Vec2, Vec2, Vec2),
}

/// A closed outline: the last point joins back to the first.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    start: Vec2,
    segments: Vec<Segment>,
}

/// How many straight pieces a curve is flattened into. A corner is a quarter
/// turn at most, and at HUD sizes eight steps are invisible as steps.
pub const STEPS_PER_CURVE: usize = 8;

impl Path {
    pub fn starting_at(point: Vec2) -> Self {
        Self { start: point, segments: Vec::new() }
    }

    pub fn line_to(mut self, to: Vec2) -> Self {
        self.segments.push(Segment::Line(to));
        self
    }

    pub fn quadratic_to(mut self, control: Vec2, to: Vec2) -> Self {
        self.segments.push(Segment::Quadratic(control, to));
        self
    }

    pub fn cubic_to(mut self, first: Vec2, second: Vec2, to: Vec2) -> Self {
        self.segments.push(Segment::Cubic(first, second, to));
        self
    }

    /// A polygon whose every corner is replaced by a [`Corner`]. A radius
    /// longer than half either neighbouring edge is cut to it, so two corners
    /// never overlap on a short edge.
    pub fn rounded_polygon(points: &[Vec2], corner: Corner) -> Self {
        Self::rounded_polygon_each(points, &vec![corner; points.len()])
    }

    /// As [`Path::rounded_polygon`], with a corner per point.
    pub fn rounded_polygon_each(points: &[Vec2], corners: &[Corner]) -> Self {
        assert_eq!(points.len(), corners.len(), "one corner per point");
        let n = points.len();
        assert!(n >= 3, "a polygon needs three points");

        let mut segments = Vec::new();
        let mut start = None;
        for i in 0..n {
            let previous = points[(i + n - 1) % n];
            let at = points[i];
            let next = points[(i + 1) % n];
            let radius = corners[i]
                .radius
                .min(at.distance(previous) * 0.5)
                .min(at.distance(next) * 0.5);
            let into = at + (previous - at).normalize_or_zero() * radius;
            let out = at + (next - at).normalize_or_zero() * radius;

            match start {
                None => start = Some(into),
                Some(_) => segments.push(Segment::Line(into)),
            }
            if radius > 0.0 {
                segments.extend(squircle_corner(into, at, out, corners[i].exponent));
            }
        }
        let start = start.expect("n >= 3");
        Self { start, segments }
    }

    /// The outline as points, each curve cut into [`STEPS_PER_CURVE`] lines.
    /// The closing point is not repeated.
    pub fn flatten(&self) -> Vec<Vec2> {
        let mut points = vec![self.start];
        let mut from = self.start;
        for segment in &self.segments {
            match *segment {
                Segment::Line(to) => {
                    points.push(to);
                    from = to;
                }
                Segment::Quadratic(control, to) => {
                    let (a, b, c) = (from, control, to);
                    for step in 1..=STEPS_PER_CURVE {
                        let t = step as f32 / STEPS_PER_CURVE as f32;
                        let u = 1.0 - t;
                        points.push(a * u * u + b * 2.0 * u * t + c * t * t);
                    }
                    from = to;
                }
                Segment::Cubic(first, second, to) => {
                    let (a, b, c, d) = (from, first, second, to);
                    for step in 1..=STEPS_PER_CURVE {
                        let t = step as f32 / STEPS_PER_CURVE as f32;
                        let u = 1.0 - t;
                        points.push(
                            a * u * u * u + b * 3.0 * u * u * t + c * 3.0 * u * t * t + d * t * t * t,
                        );
                    }
                    from = to;
                }
            }
        }
        if points.len() > 1 && points.last().unwrap().distance(points[0]) < 1e-4 {
            points.pop();
        }
        points.dedup_by(|a, b| a.distance(*b) < 1e-4);
        points
    }
}

/// A rounded corner: how far back along each edge it starts, and how square
/// it is. Exponent 2 is an elliptical arc; 4 is the classic squircle; higher
/// creeps towards the sharp corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corner {
    pub radius: f32,
    pub exponent: f32,
}

impl Corner {
    pub const SHARP: Corner = Corner { radius: 0.0, exponent: 4.0 };

    pub fn squircle(radius: f32) -> Self {
        Self { radius, exponent: 4.0 }
    }

    pub fn round(radius: f32) -> Self {
        Self { radius, exponent: 2.0 }
    }
}

/// A quarter superellipse from `into` to `out`, bulging towards `corner`, in
/// the affine frame the three points make, so it works at any angle and not
/// only on right angles.
fn squircle_corner(into: Vec2, corner: Vec2, out: Vec2, exponent: f32) -> Vec<Segment> {
    // The centre is the parallelogram's fourth point: `into` and `out` are
    // its two axes' ends.
    let centre = into + out - corner;
    let along_into = into - centre;
    let along_out = out - centre;
    let power = 2.0 / exponent.max(1.0);
    (1..=STEPS_PER_CURVE)
        .map(|step| {
            let theta = step as f32 / STEPS_PER_CURVE as f32 * std::f32::consts::FRAC_PI_2;
            let (s, c) = theta.sin_cos();
            Segment::Line(centre + along_into * c.max(0.0).powf(power) + along_out * s.max(0.0).powf(power))
        })
        .collect()
}

/// A closed parametric curve, sampled at `samples` evenly spaced `t` in
/// `[0, 1)`. `curve(0)` and `curve(1)` should be the same point.
pub fn parametric(samples: usize, curve: impl Fn(f32) -> Vec2) -> Vec<Vec2> {
    (0..samples).map(|i| curve(i as f32 / samples as f32)).collect()
}

/// A whole superellipse: `|x/a|^n + |y/b|^n = 1`.
pub fn superellipse(centre: Vec2, half_size: Vec2, exponent: f32, samples: usize) -> Vec<Vec2> {
    let power = 2.0 / exponent.max(1.0);
    parametric(samples, |t| {
        let (s, c) = (t * std::f32::consts::TAU).sin_cos();
        centre + half_size * Vec2::new(c.signum() * c.abs().powf(power), s.signum() * s.abs().powf(power))
    })
}

/// Shoelace area, unsigned.
pub fn area(points: &[Vec2]) -> f32 {
    let n = points.len();
    (0..n).map(|i| points[i].perp_dot(points[(i + 1) % n])).sum::<f32>().abs() * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(side: f32) -> Vec<Vec2> {
        vec![Vec2::ZERO, Vec2::new(side, 0.0), Vec2::splat(side), Vec2::new(0.0, side)]
    }

    #[test]
    fn a_sharp_polygon_flattens_to_its_own_corners() {
        let points = Path::rounded_polygon(&square(10.0), Corner::SHARP).flatten();
        assert_eq!(points.len(), 4);
        assert!((area(&points) - 100.0).abs() < 1e-3);
    }

    /// A squircle cuts less off a corner than a circle and more than nothing.
    #[test]
    fn a_squircle_corner_sits_between_a_circle_and_a_point() {
        let side = 100.0;
        let radius = 20.0;
        let lost_to = |corner: Corner| {
            side * side - area(&Path::rounded_polygon(&square(side), corner).flatten())
        };
        let circle = lost_to(Corner::round(radius));
        let squircle = lost_to(Corner::squircle(radius));
        // Four quarter-circle notches: r^2 (1 - pi/4) each.
        let expected_circle = 4.0 * radius * radius * (1.0 - std::f32::consts::FRAC_PI_4);
        assert!((circle - expected_circle).abs() < expected_circle * 0.05, "{circle} vs {expected_circle}");
        assert!(squircle > 0.0 && squircle < circle, "{squircle} not inside (0, {circle})");
    }

    /// The corner meets the edges where the radius says it does, so a
    /// straight edge is still straight up to there.
    #[test]
    fn a_corner_starts_and_ends_on_its_edges() {
        let points = Path::rounded_polygon(&square(100.0), Corner::squircle(20.0)).flatten();
        assert!(points.contains(&Vec2::new(0.0, 20.0)) || points.iter().any(|p| p.distance(Vec2::new(0.0, 20.0)) < 1e-3));
        assert!(points.iter().any(|p| p.distance(Vec2::new(20.0, 0.0)) < 1e-3));
    }

    #[test]
    fn a_radius_too_big_for_its_edge_is_cut_down() {
        let thin = [Vec2::ZERO, Vec2::new(100.0, 0.0), Vec2::new(100.0, 10.0), Vec2::new(0.0, 10.0)];
        let points = Path::rounded_polygon(&thin, Corner::squircle(50.0)).flatten();
        assert!(points.iter().all(|p| p.y >= -1e-3 && p.y <= 10.0 + 1e-3));
    }

    /// Corners on a trapezoid, where no angle is square.
    #[test]
    fn a_slanted_corner_stays_inside_its_polygon() {
        let trapezoid = [Vec2::new(20.0, 0.0), Vec2::new(100.0, 0.0), Vec2::new(120.0, 50.0), Vec2::new(0.0, 50.0)];
        let sharp = area(&trapezoid);
        let rounded = area(&Path::rounded_polygon(&trapezoid, Corner::squircle(12.0)).flatten());
        assert!(rounded < sharp && rounded > sharp * 0.9);
    }

    #[test]
    fn curves_are_flattened() {
        let points = Path::starting_at(Vec2::ZERO)
            .cubic_to(Vec2::new(0.0, -10.0), Vec2::new(10.0, -10.0), Vec2::new(10.0, 0.0))
            .quadratic_to(Vec2::new(5.0, 10.0), Vec2::ZERO)
            .flatten();
        // Both curves' last points close onto the start and are dropped once.
        assert_eq!(points.len(), 2 * STEPS_PER_CURVE);
    }

    #[test]
    fn a_superellipse_of_exponent_two_is_an_ellipse() {
        let points = superellipse(Vec2::ZERO, Vec2::new(20.0, 10.0), 2.0, 256);
        let ellipse = std::f32::consts::PI * 20.0 * 10.0;
        assert!((area(&points) - ellipse).abs() < ellipse * 0.01);
    }
}
