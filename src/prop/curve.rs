//! A sketch's curves in its own plane: what each entity draws, evaluated and
//! cut into straight pieces for the arrangement, which only knows segments.
//! See `documentation/prop-modeling.md`, section 4 of its decisions ("Curves in
//! sketches").
//!
//! Faceting is by chord tolerance, the same on every machine. The count
//! changes as a curve is dragged, which renames nothing: a swept face is
//! named by the entity, never by a facet.

use bevy::prelude::*;

/// Meters a facet may stand off its curve at most.
pub const CHORD_TOLERANCE: f32 = 2e-4;
/// Pieces per whole turn, at least and at most: a small circle still reads
/// round, and a large one costs a low-poly prop no more than this.
const MIN_PER_TURN: f32 = 8.0;
const MAX_PER_TURN: f32 = 48.0;
/// Pieces a free curve is cut into, at most.
const MAX_PIECES: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Curve2 {
    Line { a: Vec2, b: Vec2 },
    /// From `start` round `center` by `sweep` radians, positive
    /// counter-clockwise. `end` is where it stops, given rather than derived
    /// so the arrangement meets the sketch's own end point exactly.
    Arc { center: Vec2, start: Vec2, end: Vec2, sweep: f32 },
    Circle { center: Vec2, radius: f32 },
    /// Through its first and last points, pulled toward the rest.
    Bezier { points: Vec<Vec2> },
    /// A rational B-spline: `knots` has `points + degree + 1` entries,
    /// non-decreasing.
    Nurbs { degree: usize, points: Vec<Vec2>, weights: Vec<f32>, knots: Vec<f32> },
}

impl Curve2 {
    pub fn closed(&self) -> bool {
        matches!(self, Curve2::Circle { .. })
    }

    /// At `t` from 0 to 1 along it.
    pub fn point(&self, t: f32) -> Vec2 {
        match self {
            Curve2::Line { a, b } => a.lerp(*b, t),
            Curve2::Arc { center, start, sweep, .. } => {
                let from = *start - *center;
                Vec2::from_angle(sweep * t).rotate(from) + *center
            }
            Curve2::Circle { center, radius } => *center + Vec2::from_angle(std::f32::consts::TAU * t) * *radius,
            Curve2::Bezier { points } => de_casteljau(points, t),
            Curve2::Nurbs { degree, points, weights, knots } => de_boor(*degree, points, weights, knots, t),
        }
    }

    /// Points along it, ends included, no two facets farther than
    /// [`CHORD_TOLERANCE`] off it. A closed curve's last point is its first
    /// again. An open curve's ends are its own exactly.
    pub fn facets(&self) -> Vec<Vec2> {
        let pieces = match self {
            Curve2::Line { .. } => 1,
            Curve2::Arc { center, start, sweep, .. } => turn_pieces(start.distance(*center), sweep.abs()),
            Curve2::Circle { radius, .. } => turn_pieces(*radius, std::f32::consts::TAU),
            Curve2::Bezier { points } => bezier_pieces(points),
            Curve2::Nurbs { points, knots, degree, .. } => nurbs_pieces(points, knots, *degree),
        };
        let mut out: Vec<Vec2> = (0..=pieces).map(|i| self.point(i as f32 / pieces as f32)).collect();
        match self {
            Curve2::Arc { start, end, .. } => {
                out[0] = *start;
                out[pieces] = *end;
            }
            Curve2::Line { a, b } => out = vec![*a, *b],
            Curve2::Circle { .. } => out[pieces] = out[0],
            Curve2::Bezier { points } | Curve2::Nurbs { points, .. } => {
                if let (Some(first), Some(last)) = (points.first(), points.last()) {
                    out[0] = *first;
                    out[pieces] = *last;
                }
            }
        }
        out
    }
}

/// Pieces for `angle` radians of a circle of `radius`, each within the chord
/// tolerance.
fn turn_pieces(radius: f32, angle: f32) -> usize {
    let per_piece = match radius > CHORD_TOLERANCE {
        true => 2.0 * (1.0 - CHORD_TOLERANCE / radius).acos(),
        false => std::f32::consts::TAU / MIN_PER_TURN,
    };
    let per_turn = (std::f32::consts::TAU / per_piece).clamp(MIN_PER_TURN, MAX_PER_TURN);
    ((angle / std::f32::consts::TAU * per_turn).ceil() as usize).max(1)
}

/// A degree-`d` Bézier strays from a chord by at most `d(d-1)/8` of its
/// largest second difference times the step squared.
fn bezier_pieces(points: &[Vec2]) -> usize {
    let degree = points.len().saturating_sub(1) as f32;
    let bend = points.windows(3).map(|w| (w[0] - 2.0 * w[1] + w[2]).length()).fold(0.0, f32::max);
    let pieces = (degree * (degree - 1.0) * bend / (8.0 * CHORD_TOLERANCE)).sqrt().ceil();
    (pieces as usize).clamp(1, MAX_PIECES)
}

/// As for a Bézier per knot span, which is one between distinct knots.
fn nurbs_pieces(points: &[Vec2], knots: &[f32], degree: usize) -> usize {
    let spans = knots.windows(2).filter(|w| w[1] > w[0]).count().max(1);
    let d = degree as f32;
    let bend = points.windows(3).map(|w| (w[0] - 2.0 * w[1] + w[2]).length()).fold(0.0, f32::max);
    let per_span = (d * (d - 1.0).max(1.0) * bend / (8.0 * CHORD_TOLERANCE)).sqrt().ceil().max(1.0) as usize;
    (spans * per_span).clamp(spans, MAX_PIECES)
}

fn de_casteljau(points: &[Vec2], t: f32) -> Vec2 {
    let mut level = points.to_vec();
    while level.len() > 1 {
        level = level.windows(2).map(|w| w[0].lerp(w[1], t)).collect();
    }
    level.first().copied().unwrap_or_default()
}

/// `t` from 0 to 1 is taken across the knots' usable range.
fn de_boor(degree: usize, points: &[Vec2], weights: &[f32], knots: &[f32], t: f32) -> Vec2 {
    let n = points.len();
    if n == 0 || knots.len() != n + degree + 1 || weights.len() != n {
        return points.first().copied().unwrap_or_default();
    }
    let (low, high) = (knots[degree], knots[n]);
    let u = low + (high - low) * t.clamp(0.0, 1.0);
    // The span holding u; the last span holds its own upper end.
    let mut k = degree;
    while k + 1 < n && u >= knots[k + 1] {
        k += 1;
    }
    let mut d: Vec<Vec3> = (0..=degree)
        .map(|j| {
            let i = k - degree + j;
            (points[i] * weights[i]).extend(weights[i])
        })
        .collect();
    for r in 1..=degree {
        for j in (r..=degree).rev() {
            let i = k - degree + j;
            let span = knots[i + degree + 1 - r] - knots[i];
            let alpha = if span.abs() < f32::EPSILON { 0.0 } else { (u - knots[i]) / span };
            d[j] = d[j - 1].lerp(d[j], alpha);
        }
    }
    let h = d[degree];
    match h.z.abs() > f32::EPSILON {
        true => h.truncate() / h.z,
        false => h.truncate(),
    }
}

/// Knots for a curve through its first and last points, evenly spaced
/// between.
pub fn clamped_knots(points: usize, degree: usize) -> Vec<f32> {
    let inner = points.saturating_sub(degree + 1);
    let mut knots = vec![0.0; degree + 1];
    knots.extend((1..=inner).map(|i| i as f32 / (inner + 1) as f32));
    knots.extend(std::iter::repeat_n(1.0, degree + 1));
    knots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arc_stays_within_the_chord_tolerance_and_meets_its_ends() {
        let center = Vec2::ZERO;
        let start = Vec2::new(0.05, 0.0);
        let end = Vec2::new(0.0, 0.05);
        let arc = Curve2::Arc { center, start, end, sweep: std::f32::consts::FRAC_PI_2 };
        let facets = arc.facets();
        assert_eq!(facets[0], start);
        assert_eq!(facets[facets.len() - 1], end);
        for pair in facets.windows(2) {
            let middle = (pair[0] + pair[1]) / 2.0;
            assert!(0.05 - middle.length() <= CHORD_TOLERANCE * 1.01, "{}", 0.05 - middle.length());
        }
    }

    #[test]
    fn a_circle_closes_on_itself() {
        let facets = Curve2::Circle { center: Vec2::ONE, radius: 0.02 }.facets();
        assert_eq!(facets[0], facets[facets.len() - 1]);
        assert!(facets.len() > MIN_PER_TURN as usize);
    }

    #[test]
    fn a_nurbs_with_unit_weights_and_clamped_knots_is_its_bezier() {
        let points = vec![Vec2::ZERO, Vec2::new(0.02, 0.04), Vec2::new(0.06, 0.04), Vec2::new(0.08, 0.0)];
        let bezier = Curve2::Bezier { points: points.clone() };
        let nurbs = Curve2::Nurbs { degree: 3, weights: vec![1.0; 4], knots: clamped_knots(4, 3), points };
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            assert!(bezier.point(t).distance(nurbs.point(t)) < 1e-6, "at {t}");
        }
    }

    /// A quarter circle is exactly a rational quadratic with the middle
    /// weight the cosine of half its angle.
    #[test]
    fn a_weighted_nurbs_draws_a_true_arc() {
        let points = vec![Vec2::X, Vec2::ONE, Vec2::Y];
        let weights = vec![1.0, std::f32::consts::FRAC_1_SQRT_2, 1.0];
        let quarter = Curve2::Nurbs { degree: 2, points, weights, knots: clamped_knots(3, 2) };
        for i in 0..=8 {
            let p = quarter.point(i as f32 / 8.0);
            assert!((p.length() - 1.0).abs() < 1e-5, "{p}");
        }
    }
}
