//! Frames along a path, for a sweep to place its profile at. See
//! `documentation/prop-modeling.md`, "Sweep".
//!
//! A frame is a turn and a scale relative to the path's start: a point of
//! the profile at `p` lands at `point + scale * turn * (p - start)`, so the
//! profile keeps where it was drawn relative to where the path begins, and
//! need not sit on the path itself.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::prop::plane::Plane;
use crate::prop::refs::{Curve3, JOINS};

/// How the profile turns as it travels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orient {
    /// As little as it can while staying square to the path: rotation
    /// minimizing frames, so a straight stretch does not spin it.
    #[default]
    Minimal,
    /// Following the path's curvature, which a straight stretch has none of.
    Frenet,
    /// Not at all: carried along, turned never.
    Fixed,
}

impl Orient {
    pub const ALL: [Orient; 3] = [Orient::Minimal, Orient::Frenet, Orient::Fixed];

    pub fn name(self) -> String {
        match self {
            Orient::Minimal => crate::get!("prop.sweep.minimal"),
            Orient::Frenet => crate::get!("prop.sweep.frenet"),
            Orient::Fixed => crate::get!("prop.sweep.fixed"),
        }
    }
}

/// A path as points, each span tagged with the curve it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct PathPoints {
    pub points: Vec<Vec3>,
    /// One per span: `points.len()` when closed, one fewer otherwise.
    pub spans: Vec<u32>,
    pub closed: bool,
}

impl PathPoints {
    /// Curves already joined end to end, as `PathRef::resolve` gives them.
    pub fn of(curves: &[Curve3]) -> PathPoints {
        let mut points: Vec<Vec3> = Vec::new();
        let mut spans = Vec::new();
        for (index, curve) in curves.iter().enumerate() {
            for (a, b) in curve.segments() {
                if points.is_empty() {
                    points.push(a);
                }
                if b.distance(points[points.len() - 1]) > JOINS {
                    points.push(b);
                    spans.push(index as u32);
                }
            }
        }
        let closed = points.len() > 2 && points[0].distance(points[points.len() - 1]) < JOINS;
        if closed {
            points.pop();
        }
        PathPoints { points, spans, closed }
    }

    /// Turned to start on `plane`: as it is, reversed if only its far end is
    /// on it, or, closed, from its first point on it. `None` if it does not
    /// touch the plane where it could start.
    pub fn from_plane(self, plane: &Plane, within: f32) -> Option<PathPoints> {
        let on = |p: &Vec3| plane.distance(*p).abs() < within;
        if self.points.first().is_some_and(on) {
            return Some(self);
        }
        if self.closed {
            let at = self.points.iter().position(on)?;
            let mut turned = self;
            turned.points.rotate_left(at);
            turned.spans.rotate_left(at);
            return Some(turned);
        }
        self.points.last().is_some_and(on).then(|| self.reversed())
    }

    pub fn reversed(mut self) -> PathPoints {
        self.points.reverse();
        self.spans.reverse();
        if self.closed {
            // The closing span runs from the old first point, now last.
            self.points.rotate_right(1);
            self.spans.rotate_right(1);
        }
        self
    }

    fn span(&self, k: usize) -> Vec3 {
        self.points[(k + 1) % self.points.len()] - self.points[k]
    }

    fn span_count(&self) -> usize {
        if self.closed { self.points.len() } else { self.points.len() - 1 }
    }

    /// Meters from the start to each point; and the whole length, the
    /// closing span included.
    fn distances(&self) -> (Vec<f32>, f32) {
        let mut along = vec![0.0];
        for k in 0..self.points.len() - 1 {
            along.push(along[k] + self.span(k).length());
        }
        let total = along[along.len() - 1] + if self.closed { self.span(self.points.len() - 1).length() } else { 0.0 };
        (along, total)
    }

    /// Square to the path at each point: along the span at an open end, and
    /// halfway between the spans either side elsewhere.
    fn tangents(&self) -> Vec<Vec3> {
        let n = self.points.len();
        let count = self.span_count();
        let dir = |k: usize| self.span(k).normalize_or_zero();
        (0..n)
            .map(|k| match (self.closed, k) {
                (false, 0) => dir(0),
                (false, at) if at == n - 1 => dir(count - 1),
                (_, at) => (dir((at + count - 1) % count) + dir(at % count)).normalize_or(dir(at % count)),
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathProblem {
    /// Fewer than two points, or a span of no length.
    TooShort,
    /// Frenet frames along a straight stretch, which has no curvature to
    /// follow.
    Straight,
}

impl PathProblem {
    pub fn message(self) -> String {
        match self {
            PathProblem::TooShort => crate::get!("prop.problems.path_too_short"),
            PathProblem::Straight => crate::get!("prop.problems.path_straight"),
        }
    }
}

/// A turn and a scale per point of the path, relative to its first.
///
/// `reference` is a direction to start the profile's own "across" from: the
/// sketch plane's `x`. `twist` is degrees about the path over its whole
/// length, `end_scale` the profile's size at the end against its start, both
/// changing evenly with distance. A closed path with [`Orient::Minimal`]
/// spreads whatever turn is left over at the end evenly along it, so it
/// meets itself.
pub fn frames(path: &PathPoints, reference: Vec3, orient: Orient, twist: f32, end_scale: f32) -> Result<Vec<(Quat, f32)>, PathProblem> {
    let n = path.points.len();
    if n < 2 || (0..path.span_count()).any(|k| path.span(k).length() < JOINS) {
        return Err(PathProblem::TooShort);
    }
    let tangents = path.tangents();
    let across = |t: Vec3| {
        let r = reference - t * reference.dot(t);
        r.try_normalize().unwrap_or_else(|| t.any_orthonormal_vector())
    };

    let mut acrosses: Vec<Vec3> = match orient {
        // Turned never, twist included.
        Orient::Fixed => return Ok(scaled(vec![(Quat::IDENTITY, 1.0); n], path, end_scale)),
        Orient::Minimal => {
            let mut r = vec![across(tangents[0])];
            for k in 0..n - 1 {
                r.push(transported(path.points[k], path.points[k + 1], tangents[k], tangents[k + 1], r[k]));
            }
            if path.closed {
                // Carried once more round the closing span, back to the
                // start: how far it ends up turned from where it began.
                let back = transported(path.points[n - 1], path.points[0], tangents[n - 1], tangents[0], r[n - 1]);
                let left = signed_angle(back, r[0], tangents[0]);
                let (along, total) = path.distances();
                for k in 0..n {
                    r[k] = Quat::from_axis_angle(tangents[k], left * along[k] / total) * r[k];
                }
            }
            r
        }
        Orient::Frenet => {
            let mut r = Vec::with_capacity(n);
            for k in 0..n {
                let (before, after) = match (path.closed, k) {
                    (false, 0) => (tangents[0], tangents[1]),
                    (false, at) if at == n - 1 => (tangents[n - 2], tangents[n - 1]),
                    (_, at) => (tangents[(at + n - 1) % n], tangents[(at + 1) % n]),
                };
                let bend = after - before;
                let normal = bend - tangents[k] * bend.dot(tangents[k]);
                r.push(normal.try_normalize().ok_or(PathProblem::Straight)?);
            }
            r
        }
    };

    let (along, total) = path.distances();
    for k in 0..n {
        acrosses[k] = Quat::from_axis_angle(tangents[k], twist.to_radians() * along[k] / total) * acrosses[k];
    }
    let basis = |k: usize| Mat3::from_cols(acrosses[k], tangents[k].cross(acrosses[k]), tangents[k]);
    let start = basis(0);
    let frames = (0..n).map(|k| (Quat::from_mat3(&(basis(k) * start.transpose())), 1.0)).collect();
    Ok(scaled(frames, path, end_scale))
}

fn scaled(mut frames: Vec<(Quat, f32)>, path: &PathPoints, end_scale: f32) -> Vec<(Quat, f32)> {
    let (along, total) = path.distances();
    for (k, frame) in frames.iter_mut().enumerate() {
        frame.1 = 1.0 + (end_scale - 1.0) * along[k] / total.max(f32::EPSILON);
    }
    frames
}

/// `r` carried from `a` (tangent `ta`) to `b` (tangent `tb`) by two
/// reflections: Wang, Jüttler, Zheng and Liu's double reflection method,
/// which minimizes rotation about the path between them.
fn transported(a: Vec3, b: Vec3, ta: Vec3, tb: Vec3, r: Vec3) -> Vec3 {
    let v1 = b - a;
    let c1 = v1.dot(v1);
    if c1 < f32::EPSILON {
        return r;
    }
    let r_l = r - v1 * (2.0 / c1 * v1.dot(r));
    let t_l = ta - v1 * (2.0 / c1 * v1.dot(ta));
    let v2 = tb - t_l;
    let c2 = v2.dot(v2);
    if c2 < f32::EPSILON {
        return r_l;
    }
    (r_l - v2 * (2.0 / c2 * v2.dot(r_l))).normalize_or(r)
}

/// Radians from `from` to `to` about `axis`.
fn signed_angle(from: Vec3, to: Vec3, axis: Vec3) -> f32 {
    from.cross(to).dot(axis).atan2(from.dot(to))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polyline(points: &[[f32; 3]]) -> PathPoints {
        let points: Vec<Vec3> = points.iter().map(|p| Vec3::from_array(*p)).collect();
        PathPoints::of(&[Curve3::Polyline { points, closed: false }])
    }

    #[test]
    fn a_straight_path_turns_nothing_and_frenet_has_nothing_to_follow() {
        let path = polyline(&[[0.0, 0.0, 0.0], [0.0, 0.0, 0.1], [0.0, 0.0, 0.3]]);
        let frames = frames(&path, Vec3::X, Orient::Minimal, 0.0, 1.0).unwrap();
        assert!(frames.iter().all(|(turn, scale)| turn.angle_between(Quat::IDENTITY) < 1e-5 && *scale == 1.0));
        assert_eq!(super::frames(&path, Vec3::X, Orient::Frenet, 0.0, 1.0), Err(PathProblem::Straight));
    }

    #[test]
    fn a_bend_turns_the_profile_with_the_path() {
        let path = polyline(&[[0.0, 0.0, 0.0], [0.0, 0.0, 0.1], [0.1, 0.0, 0.1]]);
        let frames = frames(&path, Vec3::X, Orient::Minimal, 0.0, 1.0).unwrap();
        // The last point's tangent is +X, from +Z at the start.
        assert!((frames[2].0 * Vec3::Z).distance(Vec3::X) < 1e-5);
        assert!((frames[1].0 * Vec3::Z).distance(Vec3::new(1.0, 0.0, 1.0).normalize()) < 1e-5, "square to the bisector at the corner");
    }

    #[test]
    fn twist_and_scale_change_evenly_with_distance() {
        let path = polyline(&[[0.0, 0.0, 0.0], [0.0, 0.0, 0.1], [0.0, 0.0, 0.2]]);
        let frames = frames(&path, Vec3::X, Orient::Minimal, 90.0, 3.0).unwrap();
        assert!((frames[1].1 - 2.0).abs() < 1e-6 && (frames[2].1 - 3.0).abs() < 1e-6);
        assert!((frames[2].0 * Vec3::X).distance(Vec3::Y) < 1e-5, "a quarter turn about +Z by the end");
    }

    /// A closed loop meets itself: what is left over after one lap is
    /// spread along it evenly by distance, so every span, the closing one
    /// included, turns the same amount per meter beyond plain transport.
    #[test]
    fn a_closed_minimal_path_meets_itself() {
        // Not planar, so transport alone leaves a turn after one lap.
        let points: Vec<Vec3> = (0..24)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / 24.0;
                Vec3::new(a.cos() * 0.1, (2.0 * a).sin() * 0.03, a.sin() * 0.1)
            })
            .collect();
        let path = PathPoints::of(&[Curve3::Polyline { points, closed: true }]);
        assert!(path.closed);
        let frames = frames(&path, Vec3::Y, Orient::Minimal, 0.0, 1.0).unwrap();
        let tangents = path.tangents();
        let n = path.points.len();
        let first = (Vec3::Y - tangents[0] * Vec3::Y.dot(tangents[0])).normalize();
        let across: Vec<Vec3> = frames.iter().map(|(turn, _)| *turn * first).collect();
        let per_meter: Vec<f32> = (0..n)
            .map(|k| {
                let next = (k + 1) % n;
                let carried = transported(path.points[k], path.points[next], tangents[k], tangents[next], across[k]);
                signed_angle(carried, across[next], tangents[next]) / path.span(k).length()
            })
            .collect();
        let (low, high) = per_meter.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), v| (l.min(*v), h.max(*v)));
        assert!(high - low < 1e-2, "{per_meter:?}");
    }
}
