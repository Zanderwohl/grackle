//! Datum axes and points: features that make no body, only something to
//! measure from, turn about or pass through, as a plane feature does.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::prop::feature::PropFeatureId;
use crate::prop::plane::{intersection, Plane, PlaneRef};
use crate::prop::refs::{CurveRef, PointRef, Resolve, Scope};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AxisDef {
    /// From `a` toward `b`.
    Through { a: PointRef, b: PointRef },
    /// Where two planes meet.
    Planes { a: PlaneRef, b: PlaneRef },
    /// Along a plane's normal, from a point.
    Normal { plane: PlaneRef, point: PointRef },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PointDef {
    Midpoint { curve: CurveRef },
    /// Where a straight curve, carried on past its ends, meets a plane.
    Intersection { curve: CurveRef, plane: PlaneRef },
}

/// An axis as evaluated, drawn and picked as the segment `a`–`b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DatumAxis {
    pub id: PropFeatureId,
    pub a: Vec3,
    pub b: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DatumPoint {
    pub id: PropFeatureId,
    pub at: Vec3,
}

/// How far past its two points an axis through them is drawn, as a fraction
/// of the distance between them.
const AXIS_MARGIN: f32 = 0.25;

/// Meters; nearer than this, two points are one.
const COINCIDENT: f32 = 1e-6;

impl AxisDef {
    pub fn features(&self) -> Vec<PropFeatureId> {
        match self {
            AxisDef::Through { a, b } => [a.features(), b.features()].concat(),
            AxisDef::Planes { a, b } => a.feature().into_iter().chain(b.feature()).collect(),
            AxisDef::Normal { plane, point } => plane.feature().into_iter().chain(point.features()).collect(),
        }
    }

    /// `None` if the points coincide or the planes are parallel.
    pub fn resolve(&self, id: PropFeatureId, scope: &Scope) -> Option<DatumAxis> {
        let (a, b) = match self {
            AxisDef::Through { a, b } => through(a.resolve(scope)?, b.resolve(scope)?)?,
            AxisDef::Planes { a, b } => where_planes_meet(&scope.plane(*a)?, &scope.plane(*b)?, scope.origin_half)?,
            AxisDef::Normal { plane, point } => normal(&scope.plane(*plane)?, point.resolve(scope)?, scope.origin_half),
        };
        Some(DatumAxis { id, a, b })
    }
}

/// The axis from `a` toward `b`, drawn a little past both.
pub fn through(a: Vec3, b: Vec3) -> Option<(Vec3, Vec3)> {
    if a.distance(b) < COINCIDENT {
        return None;
    }
    let margin = (b - a) * AXIS_MARGIN;
    Some((a - margin, b + margin))
}

/// Centered where the line passes nearest the origin, `half` meters each way.
pub fn where_planes_meet(a: &Plane, b: &Plane, half: f32) -> Option<(Vec3, Vec3)> {
    let (point, along) = intersection(a, b)?;
    let center = point - *along * point.dot(*along);
    Some((center - *along * half, center + *along * half))
}

pub fn normal(plane: &Plane, point: Vec3, half: f32) -> (Vec3, Vec3) {
    (point, point + *plane.normal * half)
}

/// Where the line through `a` and `b` meets `plane`; `None` if it runs along it.
pub fn meet((a, b): (Vec3, Vec3), plane: &Plane) -> Option<Vec3> {
    let across = (b - a).dot(*plane.normal);
    if across.abs() < 1e-9 {
        return None;
    }
    Some(a + (b - a) * (-plane.distance(a) / across))
}

impl PointDef {
    pub fn features(&self) -> Vec<PropFeatureId> {
        match self {
            PointDef::Midpoint { curve } => curve.features(),
            PointDef::Intersection { curve, plane } => curve.features().into_iter().chain(plane.feature()).collect(),
        }
    }

    /// `None` if the curve does not resolve, or runs along the plane.
    pub fn resolve(&self, id: PropFeatureId, scope: &Scope) -> Option<DatumPoint> {
        let at = match self {
            PointDef::Midpoint { curve } => curve.resolve(scope)?.midpoint(),
            PointDef::Intersection { curve, plane } => meet(curve.resolve(scope)?.line()?, &scope.plane(*plane)?)?,
        };
        Some(DatumPoint { id, at })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::feature::Axis;
    use crate::prop::plane::OriginPlane;

    fn empty() -> Scope<'static> {
        Scope::new([], &[], &[], &[], &[], 1.0)
    }

    #[test]
    fn two_origin_planes_meet_in_an_origin_axis() {
        let axis = AxisDef::Planes { a: PlaneRef::Origin(OriginPlane::Xy), b: PlaneRef::Origin(OriginPlane::Xz) }
            .resolve(PropFeatureId(0), &empty())
            .unwrap();
        let along = (axis.b - axis.a).normalize();
        assert!(along.cross(Vec3::X).length() < 1e-5, "{along}");
        assert!((axis.a + axis.b).length() < 1e-5, "centered on the origin");
    }

    #[test]
    fn parallel_planes_make_no_axis() {
        let scope = empty();
        let same = AxisDef::Planes { a: PlaneRef::Origin(OriginPlane::Xy), b: PlaneRef::Origin(OriginPlane::Xy) };
        assert!(same.resolve(PropFeatureId(0), &scope).is_none());
    }

    #[test]
    fn an_axis_meets_a_plane_where_it_crosses_it() {
        let point = PointDef::Intersection { curve: CurveRef::Axis { axis: Axis::Y }, plane: PlaneRef::Origin(OriginPlane::Xz) }
            .resolve(PropFeatureId(0), &empty())
            .unwrap();
        assert!(point.at.length() < 1e-6);
        let along = PointDef::Intersection { curve: CurveRef::Axis { axis: Axis::X }, plane: PlaneRef::Origin(OriginPlane::Xz) };
        assert!(along.resolve(PropFeatureId(0), &empty()).is_none());
    }
}
