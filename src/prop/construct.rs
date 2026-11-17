//! What a selection makes when it is used to construct a plane, an axis or a
//! point: the picks are sorted by what they stand for, and the shape of the
//! list decides the construction.
//!
//! | Making | From |
//! | --- | --- |
//! | Plane | three points, a line and a point, two lines or one flat face (through them); a plane (offset); two planes (halfway); a plane and a line along it (at an angle) |
//! | Axis | two points; two planes (where they meet); a plane and a point (normal through the point) |
//! | Point | a line (its middle); a line and a plane (where they meet) |
//!
//! A flat face stands in for a plane, and is given a plane feature of its own
//! when the construction is made.

use bevy::prelude::*;

use crate::prop::datum::{self, AxisDef, PointDef};
use crate::prop::document::{Pick, PlaneSource};
use crate::prop::feature::{FeatureOp, PropFeatureId};
use crate::prop::plane::{self, plane_through_located, preview_corners, Datum, Plane, PlaneDef, PlaneRef};
use crate::prop::reference::{GeomRef, Located};
use crate::prop::refs::{flat_face, AnyRef, CurveRef, PointRef, Resolve, Scope};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Construct {
    Plane,
    Axis,
    Point,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Making {
    PlaneThrough(Vec<AnyRef>),
    Offset(PlaneSource),
    Angle(PlaneSource, CurveRef),
    Midplane(PlaneSource, PlaneSource),
    AxisThrough(PointRef, PointRef),
    AxisWhereMeet(PlaneSource, PlaneSource),
    AxisNormal(PlaneSource, PointRef),
    Midpoint(CurveRef),
    PointWhereMeet(CurveRef, PlaneSource),
}

/// Where a making would be, to preview and to know it is real.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Preview {
    Plane { plane: Plane, corners: [Vec3; 4] },
    Axis { a: Vec3, b: Vec3 },
    Point(Vec3),
}

/// Default offset of a new offset plane; meters.
const NEW_OFFSET: f32 = 0.02;
/// Default angle of a new angled plane; degrees.
const NEW_ANGLE: f32 = 45.0;

/// How a pick sorts, for counting.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Point,
    Curve,
    /// A plane feature or origin plane.
    Plane,
    /// A face: a plane to some makings, geometry to a plane through it.
    Face,
    Other,
}

fn sort(pick: &Pick) -> Sort {
    if pick.as_point().is_some() {
        Sort::Point
    } else if pick.as_curve().is_some() {
        Sort::Curve
    } else if matches!(pick, Pick::Plane(_)) {
        Sort::Plane
    } else if pick.as_face().is_some() {
        Sort::Face
    } else {
        Sort::Other
    }
}

fn is_plane(sort: Sort) -> bool {
    matches!(sort, Sort::Plane | Sort::Face)
}

impl Construct {
    /// What the picks make, if they make something that resolves.
    pub fn making(self, picks: &[Pick], scope: &Scope) -> Option<Making> {
        let making = self.shape(picks)?;
        making.preview(scope)?;
        Some(making)
    }

    /// By the sorts of the picks alone; [`Construct::making`] checks it is real.
    fn shape(self, picks: &[Pick]) -> Option<Making> {
        let sorts: Vec<Sort> = picks.iter().map(sort).collect();
        let plane = |i: usize| picks[i].as_plane_source();
        let point = |i: usize| picks[i].as_point();
        let curve = |i: usize| picks[i].as_curve();
        match self {
            Construct::Plane => {
                let geometry = !sorts.is_empty() && sorts.iter().all(|s| matches!(s, Sort::Point | Sort::Curve | Sort::Face));
                // Geometry first, so one face is a plane through it.
                if geometry && through_shape(&sorts) {
                    return Some(Making::PlaneThrough(picks.iter().filter_map(Pick::as_located).collect()));
                }
                match sorts[..] {
                    [Sort::Plane] => Some(Making::Offset(plane(0)?)),
                    [a, b] if is_plane(a) && is_plane(b) => Some(Making::Midplane(plane(0)?, plane(1)?)),
                    [a, Sort::Curve] if is_plane(a) => Some(Making::Angle(plane(0)?, curve(1)?)),
                    [Sort::Curve, b] if is_plane(b) => Some(Making::Angle(plane(1)?, curve(0)?)),
                    _ => None,
                }
            }
            Construct::Axis => match sorts[..] {
                [Sort::Point, Sort::Point] => Some(Making::AxisThrough(point(0)?, point(1)?)),
                [a, b] if is_plane(a) && is_plane(b) => Some(Making::AxisWhereMeet(plane(0)?, plane(1)?)),
                [a, Sort::Point] if is_plane(a) => Some(Making::AxisNormal(plane(0)?, point(1)?)),
                [Sort::Point, b] if is_plane(b) => Some(Making::AxisNormal(plane(1)?, point(0)?)),
                _ => None,
            },
            Construct::Point => match sorts[..] {
                [Sort::Curve] => Some(Making::Midpoint(curve(0)?)),
                [Sort::Curve, b] if is_plane(b) => Some(Making::PointWhereMeet(curve(0)?, plane(1)?)),
                [a, Sort::Curve] if is_plane(a) => Some(Making::PointWhereMeet(curve(1)?, plane(0)?)),
                _ => None,
            },
        }
    }

    /// Whether a slot holding `has` takes `pick`: when the two make something,
    /// or are on their way to it. Never trades something made for something
    /// not yet made.
    pub fn takes(self, has: &[Pick], pick: &Pick, scope: &Scope) -> bool {
        let with: Vec<Pick> = has.iter().chain([pick]).cloned().collect();
        if self.making(&with, scope).is_some() {
            return true;
        }
        self.making(has, scope).is_none() && self.shape(&with).is_none() && self.viable(&with)
    }

    /// Whether `picks` makes something, or could once more are added.
    pub fn viable(self, picks: &[Pick]) -> bool {
        if self.shape(picks).is_some() {
            return true;
        }
        let sorts: Vec<Sort> = picks.iter().map(sort).collect();
        let count = |wanted: Sort| sorts.iter().filter(|s| **s == wanted).count();
        let (points, curves, planes) = (count(Sort::Point), count(Sort::Curve), sorts.iter().filter(|s| is_plane(**s)).count());
        if sorts.contains(&Sort::Other) {
            return false;
        }
        match self {
            // Short of three points, a line and a point, or two lines.
            Construct::Plane => planes == 0 && points + 2 * curves < 4 && curves <= 2,
            Construct::Axis => curves == 0 && sorts.len() == 1,
            Construct::Point => points == 0 && sorts.len() == 1 && planes + curves == 1,
        }
    }

    /// Whether what `picks` makes could become something else with another
    /// pick, so a tool waits to be told it is done.
    pub fn extendable(self, picks: &[Pick]) -> bool {
        let sorts: Vec<Sort> = picks.iter().map(sort).collect();
        match self {
            Construct::Plane => sorts[..] == [Sort::Plane],
            Construct::Axis => false,
            Construct::Point => sorts[..] == [Sort::Curve],
        }
    }
}

/// Points, lines and faces in one of the shapes a plane goes through.
fn through_shape(sorts: &[Sort]) -> bool {
    let count = |wanted: Sort| sorts.iter().filter(|s| **s == wanted).count();
    match count(Sort::Face) {
        0 => matches!((count(Sort::Curve), count(Sort::Point)), (0, 3) | (1, 1) | (2, 0)),
        1 => sorts.len() == 1,
        _ => false,
    }
}

/// A plane source as a datum; a face as the plane through it.
fn datum_of(source: &PlaneSource, scope: &Scope) -> Option<Datum> {
    match source {
        PlaneSource::Plane(on) => scope.datum(*on),
        PlaneSource::Face(face) => Datum::through(PropFeatureId(u32::MAX), &[flat_face(face, scope)?]),
    }
}

impl Making {
    pub fn preview(&self, scope: &Scope) -> Option<Preview> {
        let plane = |(plane, corners): (Plane, [Vec3; 4])| Preview::Plane { plane, corners };
        let line = |curve: &CurveRef| curve.resolve(scope).and_then(|curve| curve.line());
        Some(match self {
            Making::PlaneThrough(through) => {
                let located: Vec<Located> = through.iter().map(|r| r.locate(scope)).collect::<Option<_>>()?;
                let found = plane_through_located(&located)?;
                let points: Vec<Vec3> = located.iter().flat_map(Located::points).collect();
                plane((found, preview_corners(&found, &points)))
            }
            Making::Offset(from) => plane(plane::offset(&datum_of(from, scope)?, NEW_OFFSET)),
            Making::Angle(from, about) => plane(plane::angled(&datum_of(from, scope)?, line(about)?, NEW_ANGLE)?),
            Making::Midplane(a, b) => plane(plane::halfway(&datum_of(a, scope)?, &datum_of(b, scope)?)?),
            Making::AxisThrough(a, b) => {
                let (a, b) = datum::through(a.resolve(scope)?, b.resolve(scope)?)?;
                Preview::Axis { a, b }
            }
            Making::AxisWhereMeet(a, b) => {
                let (a, b) = (datum_of(a, scope)?.plane, datum_of(b, scope)?.plane);
                let (a, b) = datum::where_planes_meet(&a, &b, scope.origin_half)?;
                Preview::Axis { a, b }
            }
            Making::AxisNormal(on, point) => {
                let (a, b) = datum::normal(&datum_of(on, scope)?.plane, point.resolve(scope)?, scope.origin_half);
                Preview::Axis { a, b }
            }
            Making::Midpoint(curve) => Preview::Point(curve.resolve(scope)?.midpoint()),
            Making::PointWhereMeet(curve, on) => Preview::Point(datum::meet(line(curve)?, &datum_of(on, scope)?.plane)?),
        })
    }

    /// The faces standing in for planes, each needing a plane feature first.
    pub fn faces(&self) -> Vec<GeomRef> {
        let face = |source: &PlaneSource| match source {
            PlaneSource::Face(face) => Some(face.clone()),
            PlaneSource::Plane(_) => None,
        };
        match self {
            Making::Offset(a) | Making::Angle(a, _) | Making::AxisNormal(a, _) | Making::PointWhereMeet(_, a) => {
                face(a).into_iter().collect()
            }
            Making::Midplane(a, b) | Making::AxisWhereMeet(a, b) => face(a).into_iter().chain(face(b)).collect(),
            Making::PlaneThrough(_) | Making::AxisThrough(..) | Making::Midpoint(_) => vec![],
        }
    }

    /// The feature, with each of [`Making::faces`] given, in order, the plane
    /// made through it.
    pub fn op(&self, face_planes: &[PropFeatureId]) -> FeatureOp {
        let mut planes = face_planes.iter();
        let mut on = |source: &PlaneSource| match source {
            PlaneSource::Plane(on) => *on,
            // `faces` lists them in the order they are asked for here.
            PlaneSource::Face(_) => PlaneRef::Feature(*planes.next().expect("a plane per face")),
        };
        match self {
            Making::PlaneThrough(through) => FeatureOp::Plane { def: PlaneDef::Through { through: through.clone() } },
            Making::Offset(from) => FeatureOp::Plane { def: PlaneDef::Offset { distance: NEW_OFFSET, from: on(from) } },
            Making::Angle(from, about) => {
                FeatureOp::Plane { def: PlaneDef::Angle { degrees: NEW_ANGLE, from: on(from), about: about.clone() } }
            }
            Making::Midplane(a, b) => {
                let a = on(a);
                FeatureOp::Plane { def: PlaneDef::Midplane { a, b: on(b) } }
            }
            Making::AxisThrough(a, b) => FeatureOp::Axis { def: AxisDef::Through { a: a.clone(), b: b.clone() } },
            Making::AxisWhereMeet(a, b) => {
                let a = on(a);
                FeatureOp::Axis { def: AxisDef::Planes { a, b: on(b) } }
            }
            Making::AxisNormal(plane, point) => FeatureOp::Axis { def: AxisDef::Normal { plane: on(plane), point: point.clone() } },
            Making::Midpoint(curve) => FeatureOp::Point { def: PointDef::Midpoint { curve: curve.clone() } },
            Making::PointWhereMeet(curve, plane) => {
                FeatureOp::Point { def: PointDef::Intersection { curve: curve.clone(), plane: on(plane) } }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::feature::Axis;
    use crate::prop::plane::OriginPlane;

    fn scope() -> Scope<'static> {
        Scope::new([], &[], &[], &[], &[], 1.0)
    }

    #[test]
    fn a_lone_plane_offsets_and_waits_for_more() {
        let picks = [Pick::Plane(PlaneRef::Origin(OriginPlane::Xy))];
        assert!(matches!(Construct::Plane.making(&picks, &scope()), Some(Making::Offset(_))));
        assert!(Construct::Plane.extendable(&picks));
    }

    #[test]
    fn two_planes_make_a_midplane_or_an_axis() {
        let picks = [Pick::Plane(PlaneRef::Origin(OriginPlane::Xy)), Pick::Plane(PlaneRef::Origin(OriginPlane::Xz))];
        let Some(Preview::Plane { plane, .. }) = Construct::Plane.making(&picks, &scope()).and_then(|m| m.preview(&scope()))
        else {
            panic!("no midplane");
        };
        let expected = (Vec3::Z + Vec3::Y).normalize();
        assert!(plane.normal.dot(expected) > 0.999, "{:?}", plane.normal);
        assert!(matches!(Construct::Axis.making(&picks, &scope()), Some(Making::AxisWhereMeet(..))));
    }

    #[test]
    fn an_axis_alone_is_on_its_way_to_a_plane_and_makes_a_midpoint() {
        let picks = [Pick::Axis(Axis::X)];
        assert!(Construct::Plane.viable(&picks));
        assert!(Construct::Plane.making(&picks, &scope()).is_none());
        assert!(matches!(Construct::Point.making(&picks, &scope()), Some(Making::Midpoint(_))));
    }

    #[test]
    fn two_crossing_axes_make_a_plane_through_them() {
        let picks = [Pick::Axis(Axis::X), Pick::Axis(Axis::Y)];
        let Some(Preview::Plane { plane, .. }) = Construct::Plane.making(&picks, &scope()).and_then(|m| m.preview(&scope()))
        else {
            panic!("no plane");
        };
        assert!(plane.normal.dot(Vec3::Z).abs() > 0.999);
    }
}
