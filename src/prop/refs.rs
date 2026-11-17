//! References by what they are geometrically — a point, a curve, a path, a
//! plane, a direction, a face — rather than by where they came from. Any
//! source of a kind may stand wherever that kind is wanted: a corner, a sketch
//! point or a datum point are all a [`PointRef`]. See
//! `documentation/prop-modeling.md`.
//!
//! Every reference resolves against a [`Scope`]: what stands at one point in
//! the feature list. The evaluator builds one per feature, and
//! [`Evaluated::scope`](crate::prop::feature::Evaluated::scope) one for the
//! finished prop.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::get;
use crate::prop::datum::{DatumAxis, DatumPoint};
use crate::prop::feature::{Axis, PropFeatureId};
use crate::prop::plane::{Datum, Plane, PlaneRef};
use crate::prop::reference::{Element, GeomRef, Located, Wire};
use crate::prop::sketch::{PlacedSketch, SketchId};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PointRef {
    Origin,
    /// A named corner of a body.
    Corner { corner: GeomRef },
    Sketch { sketch: PropFeatureId, point: SketchId },
    Datum { feature: PropFeatureId },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CurveRef {
    /// Through the origin.
    Axis { axis: Axis },
    /// A named edge of a body.
    Edge { edge: GeomRef },
    Sketch { sketch: PropFeatureId, entity: SketchId },
    Datum { feature: PropFeatureId },
}

/// Curves joined end to end, in order. Each may run either way; resolving
/// turns each to meet the last.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PathRef {
    pub curves: Vec<CurveRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DirectionRef {
    /// Along a straight curve, first end to second.
    Along { curve: CurveRef },
    /// A plane's normal.
    Normal { plane: PlaneRef },
    /// A flat face's outward normal.
    Face { face: GeomRef },
}

/// One of any kind, for what takes a mix: a plane through a point and a line.
/// Tagged `ref` so the kind's own `kind` tag can share its table.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "ref", rename_all = "snake_case")]
pub enum AnyRef {
    Point(PointRef),
    Curve(CurveRef),
    Path(PathRef),
    Plane(PlaneRef),
    Direction(DirectionRef),
    Face { face: GeomRef },
    Body { body: PropFeatureId },
    /// Entries in a sketch's region list.
    Regions { sketch: PropFeatureId, entries: Vec<SketchId> },
}

impl PointRef {
    pub fn features(&self) -> Vec<PropFeatureId> {
        match self {
            PointRef::Origin => vec![],
            PointRef::Corner { corner } => corner.source.features().collect(),
            PointRef::Sketch { sketch, .. } => vec![*sketch],
            PointRef::Datum { feature } => vec![*feature],
        }
    }
}

impl CurveRef {
    pub fn features(&self) -> Vec<PropFeatureId> {
        match self {
            CurveRef::Axis { .. } => vec![],
            CurveRef::Edge { edge } => edge.source.features().collect(),
            CurveRef::Sketch { sketch, .. } => vec![*sketch],
            CurveRef::Datum { feature } => vec![*feature],
        }
    }
}

impl PathRef {
    pub fn features(&self) -> Vec<PropFeatureId> {
        self.curves.iter().flat_map(CurveRef::features).collect()
    }
}

impl DirectionRef {
    pub fn features(&self) -> Vec<PropFeatureId> {
        match self {
            DirectionRef::Along { curve } => curve.features(),
            DirectionRef::Normal { plane } => plane.feature().into_iter().collect(),
            DirectionRef::Face { face } => face.source.features().collect(),
        }
    }

    /// The curve it runs along, if it runs along one.
    pub fn curve(&self) -> Option<&CurveRef> {
        match self {
            DirectionRef::Along { curve } => Some(curve),
            DirectionRef::Normal { .. } | DirectionRef::Face { .. } => None,
        }
    }
}

impl AnyRef {
    /// A named corner, edge or face as the kind it is.
    pub fn of_geom(reference: GeomRef) -> AnyRef {
        match reference.element {
            Element::Point(_) => AnyRef::Point(PointRef::Corner { corner: reference }),
            Element::Edge(_) => AnyRef::Curve(CurveRef::Edge { edge: reference }),
            Element::Face(_) => AnyRef::Face { face: reference },
        }
    }

    pub fn features(&self) -> Vec<PropFeatureId> {
        match self {
            AnyRef::Point(point) => point.features(),
            AnyRef::Curve(curve) => curve.features(),
            AnyRef::Path(path) => path.features(),
            AnyRef::Plane(plane) => plane.feature().into_iter().collect(),
            AnyRef::Direction(direction) => direction.features(),
            AnyRef::Face { face } => face.source.features().collect(),
            AnyRef::Body { body } => vec![*body],
            AnyRef::Regions { sketch, .. } => vec![*sketch],
        }
    }
}

/// A resolved curve. Only lines until sketches have arcs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve3 {
    Line { a: Vec3, b: Vec3 },
}

impl Curve3 {
    pub fn start(&self) -> Vec3 {
        match *self {
            Curve3::Line { a, .. } => a,
        }
    }

    pub fn end(&self) -> Vec3 {
        match *self {
            Curve3::Line { b, .. } => b,
        }
    }

    pub fn reversed(self) -> Curve3 {
        match self {
            Curve3::Line { a, b } => Curve3::Line { a: b, b: a },
        }
    }

    /// The two ends, if it is straight.
    pub fn line(&self) -> Option<(Vec3, Vec3)> {
        match *self {
            Curve3::Line { a, b } => Some((a, b)),
        }
    }

    pub fn midpoint(&self) -> Vec3 {
        match *self {
            Curve3::Line { a, b } => (a + b) / 2.0,
        }
    }
}

/// What everything resolves against: the bodies' outlines, planes, datums
/// and sketches standing at one point in the list.
pub struct Scope<'a> {
    wires: Vec<&'a Wire>,
    pub planes: &'a [Datum],
    pub sketches: &'a [PlacedSketch],
    pub axes: &'a [DatumAxis],
    pub points: &'a [DatumPoint],
    /// Meters each way of the origin an origin plane is drawn, which sizes
    /// what is made from one.
    pub origin_half: f32,
}

impl<'a> Scope<'a> {
    pub fn new(
        wires: impl IntoIterator<Item = &'a Wire>,
        planes: &'a [Datum],
        sketches: &'a [PlacedSketch],
        axes: &'a [DatumAxis],
        points: &'a [DatumPoint],
        origin_half: f32,
    ) -> Scope<'a> {
        Scope { wires: wires.into_iter().collect(), planes, sketches, axes, points, origin_half }
    }

    /// Where a named element is now, whether or not it can be seen.
    pub fn locate(&self, reference: &GeomRef) -> Option<Located> {
        self.wires
            .iter()
            .find(|wire| wire.source == reference.source)
            .and_then(|wire| wire.outline.locate(reference.element))
    }

    pub fn sketch(&self, id: PropFeatureId) -> Option<&'a PlacedSketch> {
        self.sketches.iter().find(|placed| placed.id == id)
    }

    /// An origin plane, sized to the prop, or a plane feature that has run.
    pub fn datum(&self, on: PlaneRef) -> Option<Datum> {
        match on {
            PlaneRef::Origin(which) => Some(Datum::origin(which, self.origin_half)),
            PlaneRef::Feature(_) => self.planes.iter().find(|datum| datum.id == on).cloned(),
        }
    }

    pub fn plane(&self, on: PlaneRef) -> Option<Plane> {
        match on {
            PlaneRef::Origin(which) => Some(which.plane()),
            PlaneRef::Feature(_) => self.planes.iter().find(|datum| datum.id == on).map(|datum| datum.plane),
        }
    }
}

pub fn axis_name(axis: Axis) -> String {
    get!("prop.lines.axis", "axis", axis.name())
}

pub trait Resolve {
    type Out;
    fn resolve(&self, scope: &Scope) -> Option<Self::Out>;
}

/// Meters. Shorter has no direction worth trusting.
const DEGENERATE: f32 = 1e-6;

/// Meters two curve ends may be apart and still join.
pub const JOINS: f32 = 1e-4;

impl Resolve for PointRef {
    type Out = Vec3;
    fn resolve(&self, scope: &Scope) -> Option<Vec3> {
        match self {
            PointRef::Origin => Some(Vec3::ZERO),
            PointRef::Corner { corner } => match scope.locate(corner)? {
                Located::Point(p) => Some(p),
                Located::Edge(..) | Located::Face { .. } => None,
            },
            PointRef::Sketch { sketch, point } => {
                let placed = scope.sketch(*sketch)?;
                Some(placed.world(placed.sketch.point(*point)?))
            }
            PointRef::Datum { feature } => scope.points.iter().find(|p| p.id == *feature).map(|p| p.at),
        }
    }
}

impl Resolve for CurveRef {
    type Out = Curve3;
    /// An origin axis is the unit segment from the origin.
    fn resolve(&self, scope: &Scope) -> Option<Curve3> {
        let (a, b) = match self {
            CurveRef::Axis { axis } => (Vec3::ZERO, axis.normal()),
            CurveRef::Edge { edge } => match scope.locate(edge)? {
                Located::Edge(a, b) => (a, b),
                Located::Point(_) | Located::Face { .. } => return None,
            },
            CurveRef::Sketch { sketch, entity } => {
                let placed = scope.sketch(*sketch)?;
                let (a, b) = placed.sketch.line(*entity)?;
                (placed.world(a), placed.world(b))
            }
            CurveRef::Datum { feature } => {
                let axis = scope.axes.iter().find(|axis| axis.id == *feature)?;
                (axis.a, axis.b)
            }
        };
        (a.distance(b) > DEGENERATE).then_some(Curve3::Line { a, b })
    }
}

impl Resolve for PathRef {
    type Out = Vec<Curve3>;
    /// Each curve turned to start where the last ended. `None` if any does
    /// not resolve or does not join, or there are none.
    fn resolve(&self, scope: &Scope) -> Option<Vec<Curve3>> {
        let curves: Vec<Curve3> = self.curves.iter().map(|curve| curve.resolve(scope)).collect::<Option<_>>()?;
        let mut joined: Vec<Curve3> = Vec::with_capacity(curves.len());
        for curve in &curves {
            let next = match joined.last() {
                None => match curves.get(1) {
                    // The first runs toward whichever end the second touches.
                    Some(second) if touches(curve.start(), second) && !touches(curve.end(), second) => curve.reversed(),
                    _ => *curve,
                },
                Some(last) if last.end().distance(curve.start()) < JOINS => *curve,
                Some(last) if last.end().distance(curve.end()) < JOINS => curve.reversed(),
                Some(_) => return None,
            };
            joined.push(next);
        }
        (!joined.is_empty()).then_some(joined)
    }
}

fn touches(point: Vec3, curve: &Curve3) -> bool {
    point.distance(curve.start()) < JOINS || point.distance(curve.end()) < JOINS
}

impl Resolve for DirectionRef {
    type Out = Dir3;
    fn resolve(&self, scope: &Scope) -> Option<Dir3> {
        match self {
            DirectionRef::Along { curve } => {
                let (a, b) = curve.resolve(scope)?.line()?;
                Dir3::new(b - a).ok()
            }
            DirectionRef::Normal { plane } => scope.plane(*plane).map(|plane| plane.normal),
            DirectionRef::Face { face } => match scope.locate(face)? {
                face @ Located::Face { normal, .. } if face.is_flat() => Dir3::new(normal).ok(),
                _ => None,
            },
        }
    }
}

/// A face resolves to where it is, only if it is flat.
pub fn flat_face(face: &GeomRef, scope: &Scope) -> Option<Located> {
    if !matches!(face.element, Element::Face(_)) {
        return None;
    }
    scope.locate(face).filter(Located::is_flat)
}

impl AnyRef {
    /// Where it is, as a plane through it would see it: a point, a straight
    /// curve's ends, or a flat face. Paths, planes and directions have no such
    /// place.
    pub fn locate(&self, scope: &Scope) -> Option<Located> {
        match self {
            AnyRef::Point(point) => point.resolve(scope).map(Located::Point),
            AnyRef::Curve(curve) => curve.resolve(scope)?.line().map(|(a, b)| Located::Edge(a, b)),
            AnyRef::Face { face } => flat_face(face, scope),
            AnyRef::Path(_) | AnyRef::Plane(_) | AnyRef::Direction(_) | AnyRef::Body { .. } | AnyRef::Regions { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::plane::OriginPlane;
    use crate::prop::reference::{Outline, Source};

    fn square_wire() -> Wire {
        // Corners of a unit square at z = 0, edges round it.
        let outline = Outline {
            points: vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y],
            edges: vec![[0, 1], [1, 2], [2, 3], [3, 0]],
            faces: vec![],
        };
        Wire::new(PropFeatureId(0), outline)
    }

    fn edge(i: u32) -> CurveRef {
        CurveRef::Edge { edge: GeomRef { element: Element::Edge(i), source: Source { feature: PropFeatureId(0), copies: vec![] } } }
    }

    #[test]
    fn a_path_turns_each_curve_to_meet_the_last() {
        let wire = square_wire();
        let scope = Scope::new([&wire], &[], &[], &[], &[], 1.0);
        // Edge 0 runs 0→1; edge 3 runs 3→0, so the path from 1 along 0 then
        // 3 has to turn the first round.
        let path = PathRef { curves: vec![edge(0), edge(3)] };
        let joined = path.resolve(&scope).unwrap();
        assert_eq!(joined[0].start(), Vec3::X);
        assert_eq!(joined[0].end(), Vec3::ZERO);
        assert_eq!(joined[1].end(), Vec3::Y);
    }

    #[test]
    fn a_path_with_a_gap_does_not_resolve() {
        let wire = square_wire();
        let scope = Scope::new([&wire], &[], &[], &[], &[], 1.0);
        let path = PathRef { curves: vec![edge(0), edge(2)] };
        assert!(path.resolve(&scope).is_none());
    }

    #[test]
    fn a_direction_along_an_origin_axis_is_that_axis() {
        let scope = Scope::new([], &[], &[], &[], &[], 1.0);
        let along = DirectionRef::Along { curve: CurveRef::Axis { axis: Axis::Y } };
        assert_eq!(along.resolve(&scope), Some(Dir3::Y));
        let normal = DirectionRef::Normal { plane: PlaneRef::Origin(OriginPlane::Xz) };
        assert_eq!(normal.resolve(&scope), Some(Dir3::Y));
    }

    #[test]
    fn every_kind_round_trips_through_toml() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Holder {
            refs: Vec<AnyRef>,
        }
        let corner = GeomRef { element: Element::Point(2), source: Source { feature: PropFeatureId(4), copies: vec![] } };
        let holder = Holder {
            refs: vec![
                AnyRef::Point(PointRef::Origin),
                AnyRef::Point(PointRef::Corner { corner: corner.clone() }),
                AnyRef::Point(PointRef::Sketch { sketch: PropFeatureId(1), point: SketchId(3) }),
                AnyRef::Point(PointRef::Datum { feature: PropFeatureId(7) }),
                AnyRef::Curve(CurveRef::Axis { axis: Axis::Z }),
                AnyRef::Curve(CurveRef::Sketch { sketch: PropFeatureId(1), entity: SketchId(5) }),
                AnyRef::Curve(CurveRef::Datum { feature: PropFeatureId(8) }),
                AnyRef::Path(PathRef { curves: vec![CurveRef::Axis { axis: Axis::X }, edge(1)] }),
                AnyRef::Plane(PlaneRef::Origin(OriginPlane::Xy)),
                AnyRef::Plane(PlaneRef::Feature(PropFeatureId(2))),
                AnyRef::Direction(DirectionRef::Normal { plane: PlaneRef::Feature(PropFeatureId(2)) }),
                AnyRef::Direction(DirectionRef::Along { curve: edge(2) }),
                AnyRef::Face { face: corner },
                AnyRef::Body { body: PropFeatureId(9) },
                AnyRef::Regions { sketch: PropFeatureId(1), entries: vec![SketchId(2), SketchId(4)] },
            ],
        };
        let text = toml::to_string(&holder).unwrap();
        assert_eq!(toml::from_str::<Holder>(&text).unwrap(), holder, "{text}");
    }
}
