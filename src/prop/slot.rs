//! What a tool or a feature's field wants picked: a [`Kind`], whether it
//! takes one pick or many, and a list of [`Check`]s each pick must pass.
//!
//! A check sees the values of the other slots — the tool's other picks while
//! a feature is being made, the feature's other fields while one is re-picked
//! — so "the axis lies in the regions' plane" is one function serving both.

use bevy::prelude::*;

use crate::prop::construct::{Construct, Making};
use crate::prop::document::{Pick, PlaneSource, PropDoc};
use crate::prop::feature::{Evaluated, FeatureOp, PropFeatureId, IN_PLANE};
use crate::prop::plane::{Plane, PlaneDef, PlaneRef};
use crate::prop::reference::{Element, GeomRef};
use std::collections::VecDeque;

use crate::prop::refs::{AnyRef, Curve3, CurveRef, DirectionRef, PathRef, PointRef, Resolve, JOINS};
use crate::prop::region::Region;
use crate::prop::sketch::SketchId;
use crate::prop::sketch_edit::sketch_of_mut;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A standing body, or a face standing in for one.
    Body,
    /// One or more regions, all of one sketch.
    Regions,
    /// Two or more regions, each of a sketch of its own: a loft's profiles.
    Profiles,
    /// A corner, a sketch point, a datum point or a junction.
    Point,
    /// A straight curve: an edge, a sketch's line, an axis.
    Curve,
    /// A straight curve, or a plane's or flat face's normal.
    Direction,
    /// A plane, or a flat face to make one through.
    Plane,
    /// Curves joined end to end, each joining either end of those before.
    Path,
    /// Whatever makes the plane, axis or point; see [`crate::prop::construct`].
    Construct(Construct),
}

/// Whether `pick` may go in a slot, given what the other slots hold.
pub type Check = fn(&SlotCx, &Pick) -> bool;

#[derive(Clone, Copy)]
pub struct Slot {
    pub kind: Kind,
    /// Lang key of what is asked for while the slot is empty.
    pub prompt: &'static str,
    pub checks: &'static [Check],
}

pub struct SlotCx<'a> {
    pub evaluated: &'a Evaluated,
    /// Every other slot's value, where it has one.
    pub others: &'a [Value],
}

pub const BODY: Slot = Slot { kind: Kind::Body, prompt: "prop.tool.pick.body", checks: &[another_body] };
pub const SECOND_BODY: Slot = Slot { kind: Kind::Body, prompt: "prop.tool.pick.second_body", checks: &[another_body] };
pub const REGIONS: Slot = Slot { kind: Kind::Regions, prompt: "prop.tool.pick.region", checks: &[around_the_axis] };
pub const AXIS: Slot = Slot { kind: Kind::Curve, prompt: "prop.tool.pick.axis", checks: &[in_the_regions_plane] };
pub const DIRECTION: Slot = Slot { kind: Kind::Direction, prompt: "prop.tool.pick.direction", checks: &[] };
pub const PLANE: Slot = Slot { kind: Kind::Plane, prompt: "prop.tool.pick.plane", checks: &[] };
pub const PROFILES: Slot = Slot { kind: Kind::Profiles, prompt: "prop.tool.pick.profile", checks: &[] };
pub const POINT: Slot = Slot { kind: Kind::Point, prompt: "prop.tool.pick.point", checks: &[] };
pub const PATH: Slot = Slot { kind: Kind::Path, prompt: "prop.tool.pick.path", checks: &[] };
pub const MAKE_PLANE: Slot = Slot { kind: Kind::Construct(Construct::Plane), prompt: "prop.tool.pick.through", checks: &[] };
pub const MAKE_AXIS: Slot = Slot { kind: Kind::Construct(Construct::Axis), prompt: "prop.tool.pick.axis_from", checks: &[] };
pub const MAKE_POINT: Slot = Slot { kind: Kind::Construct(Construct::Point), prompt: "prop.tool.pick.point_from", checks: &[] };

/// What a slot's picks come to, before any face is given its plane feature.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Body(PropFeatureId),
    /// `regions` is empty when read off a feature, where only the sketch is
    /// needed.
    Regions { sketch: PropFeatureId, regions: Vec<Region> },
    /// One region per sketch, in pick order.
    Profiles(Vec<(PropFeatureId, Region)>),
    Point(PointRef),
    Curve(CurveRef),
    Direction(DirectionRef),
    Plane(PlaneSource),
    /// In order along the path.
    Path(PathRef),
    Making(Making),
}

/// A [`Value`] made concrete: faces given plane features, regions adopted
/// onto their sketch.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolved {
    Body(PropFeatureId),
    Regions { sketch: PropFeatureId, entries: Vec<SketchId> },
    Profiles(Vec<(PropFeatureId, SketchId)>),
    Point(PointRef),
    Curve(CurveRef),
    Direction(DirectionRef),
    Plane(PlaneRef),
    Path(PathRef),
    /// What a construction made, whole.
    Op(FeatureOp),
}

/// A reference a feature holds, for re-picking, labeling and dependencies.
pub struct Field {
    /// Lang key.
    pub label: &'static str,
    pub slot: Slot,
    /// Eaten when the feature runs: a body operand.
    pub consumed: bool,
    /// Offered as a Pick row in the Inspector. Body operands are chosen from
    /// a list, since one already eaten cannot be clicked, and regions have
    /// rows of their own.
    pub picked: bool,
    pub refs: Vec<AnyRef>,
}

impl Kind {
    /// Whether another pick may go in, before its fit is asked.
    pub fn open(self, has: &[Pick]) -> bool {
        match self {
            // Whether one more is taken is the construction's question.
            Kind::Regions | Kind::Profiles | Kind::Path | Kind::Construct(_) => true,
            Kind::Body | Kind::Point | Kind::Curve | Kind::Direction | Kind::Plane => has.is_empty(),
        }
    }

    /// Whether `pick` is this kind of thing, and resolves.
    pub fn fits(self, has: &[Pick], pick: &Pick, evaluated: &Evaluated) -> bool {
        match self {
            Kind::Body => pick.body(evaluated).is_some(),
            Kind::Regions => {
                let Pick::Region { sketch, region } = pick else { return false };
                let encloses = evaluated.sketch(*sketch).is_some_and(|placed| placed.arrangement.find(region).is_some());
                let same_sketch = has.iter().all(|other| matches!(other, Pick::Region { sketch: s, .. } if s == sketch));
                encloses && same_sketch
            }
            Kind::Profiles => {
                let Pick::Region { sketch, region } = pick else { return false };
                let encloses = evaluated.sketch(*sketch).is_some_and(|placed| placed.arrangement.find(region).is_some());
                let new_sketch = !has.iter().any(|other| matches!(other, Pick::Region { sketch: s, .. } if s == sketch));
                encloses && new_sketch
            }
            Kind::Point => pick.as_point().is_some_and(|point| point.resolve(&evaluated.scope()).is_some()),
            Kind::Curve => pick.as_curve().is_some_and(|curve| straight(&curve, evaluated).is_some()),
            Kind::Direction => pick.as_direction().is_some_and(|direction| direction.resolve(&evaluated.scope()).is_some()),
            Kind::Plane => match pick.as_plane_source() {
                Some(PlaneSource::Plane(on)) => evaluated.datum(on).is_some(),
                Some(PlaneSource::Face(face)) => flat_face(&face, evaluated),
                None => false,
            },
            Kind::Path => {
                let with: Vec<Pick> = has.iter().chain([pick]).cloned().collect();
                !has.contains(pick) && chained(&with, evaluated).is_some()
            }
            Kind::Construct(construct) => construct.takes(has, pick, &evaluated.scope()),
        }
    }

    /// Short of what it needs.
    pub fn waiting(self, has: &[Pick], evaluated: &Evaluated) -> bool {
        match self {
            Kind::Construct(construct) => construct.making(has, &evaluated.scope()).is_none(),
            Kind::Profiles => has.len() < 2,
            _ => has.is_empty(),
        }
    }

    /// Could take another pick that changes what it comes to, so a tool waits
    /// to be told it is done.
    pub fn open_ended(self, has: &[Pick]) -> bool {
        match self {
            Kind::Regions | Kind::Profiles | Kind::Path => true,
            Kind::Construct(construct) => construct.extendable(has),
            _ => false,
        }
    }

    pub fn value(self, has: &[Pick], evaluated: &Evaluated) -> Option<Value> {
        let first = has.first()?;
        Some(match self {
            Kind::Body => Value::Body(first.body(evaluated)?),
            Kind::Regions => {
                let Pick::Region { sketch, .. } = first else { return None };
                let placed = evaluated.sketch(*sketch)?;
                let regions = has
                    .iter()
                    .map(|pick| match pick {
                        Pick::Region { region, .. } => placed.arrangement.find(region).cloned(),
                        _ => None,
                    })
                    .collect::<Option<_>>()?;
                Value::Regions { sketch: *sketch, regions }
            }
            Kind::Profiles => Value::Profiles(
                has.iter()
                    .map(|pick| match pick {
                        Pick::Region { sketch, region } => {
                            Some((*sketch, evaluated.sketch(*sketch)?.arrangement.find(region)?.clone()))
                        }
                        _ => None,
                    })
                    .collect::<Option<_>>()?,
            ),
            Kind::Point => Value::Point(first.as_point()?),
            Kind::Curve => Value::Curve(first.as_curve()?),
            Kind::Direction => Value::Direction(first.as_direction()?),
            Kind::Plane => Value::Plane(first.as_plane_source()?),
            Kind::Path => Value::Path(PathRef { curves: chained(has, evaluated)? }),
            Kind::Construct(construct) => Value::Making(construct.making(has, &evaluated.scope())?),
        })
    }
}

/// A plane feature through a flat face.
pub fn plane_through_face(face: GeomRef) -> FeatureOp {
    FeatureOp::Plane { def: PlaneDef::Through { through: vec![AnyRef::Face { face }] } }
}

impl Value {
    /// The faces standing in for planes, each needing a plane feature first.
    pub fn faces(&self) -> Vec<GeomRef> {
        match self {
            Value::Plane(PlaneSource::Face(face)) => vec![face.clone()],
            Value::Making(making) => making.faces(),
            _ => vec![],
        }
    }

    /// Takes as many of `planes` as [`Value::faces`] listed, in order, and
    /// adopts regions onto their sketch in `doc`.
    pub fn resolve(self, planes: &mut impl Iterator<Item = PropFeatureId>, doc: &mut PropDoc) -> Resolved {
        match self {
            Value::Body(id) => Resolved::Body(id),
            Value::Regions { sketch, regions } => {
                let entries = sketch_of_mut(doc, sketch)
                    .map(|drawn| regions.iter().map(|region| drawn.adopt_region(region)).collect())
                    .unwrap_or_default();
                Resolved::Regions { sketch, entries }
            }
            Value::Profiles(profiles) => Resolved::Profiles(
                profiles
                    .into_iter()
                    .filter_map(|(sketch, region)| Some((sketch, sketch_of_mut(doc, sketch)?.adopt_region(&region))))
                    .collect(),
            ),
            Value::Point(point) => Resolved::Point(point),
            Value::Curve(curve) => Resolved::Curve(curve),
            Value::Direction(direction) => Resolved::Direction(direction),
            Value::Plane(PlaneSource::Plane(on)) => Resolved::Plane(on),
            Value::Path(path) => Resolved::Path(path),
            Value::Plane(PlaneSource::Face(_)) => Resolved::Plane(PlaneRef::Feature(planes.next().expect("a plane per face"))),
            Value::Making(making) => {
                let ids: Vec<PropFeatureId> = planes.take(making.faces().len()).collect();
                Resolved::Op(making.op(&ids))
            }
        }
    }

    /// What a field holds, as a value for checks to compare against. A
    /// construction's field has none.
    pub fn of_field(field: &Field) -> Option<Value> {
        Some(match (field.slot.kind, field.refs.first()?) {
            (Kind::Body, AnyRef::Body { body }) => Value::Body(*body),
            (Kind::Regions, AnyRef::Regions { sketch, .. }) => Value::Regions { sketch: *sketch, regions: vec![] },
            (Kind::Point, AnyRef::Point(point)) => Value::Point(point.clone()),
            (Kind::Curve, AnyRef::Curve(curve)) => Value::Curve(curve.clone()),
            (Kind::Direction, AnyRef::Direction(direction)) => Value::Direction(direction.clone()),
            (Kind::Plane, AnyRef::Plane(on)) => Value::Plane(PlaneSource::Plane(*on)),
            (Kind::Path, AnyRef::Path(path)) => Value::Path(path.clone()),
            _ => return None,
        })
    }
}

/// The picked curves in order along one path: each added after the last if
/// it joins its end, before the first if it joins its start. `None` if one
/// does not resolve, joins neither end, or the path has closed already.
pub fn chained(picks: &[Pick], evaluated: &Evaluated) -> Option<Vec<CurveRef>> {
    let scope = evaluated.scope();
    let mut chain: VecDeque<(CurveRef, Curve3)> = VecDeque::new();
    for pick in picks {
        let reference = pick.as_curve()?;
        let curve = reference.resolve(&scope)?;
        let Some((first, last)) = chain.front().zip(chain.back()) else {
            chain.push_back((reference, curve));
            continue;
        };
        let (start, end) = (first.1.start(), last.1.end());
        if chain.len() > 1 && start.distance(end) < JOINS {
            return None;
        }
        if curve.start().distance(end) < JOINS {
            chain.push_back((reference, curve));
        } else if curve.end().distance(end) < JOINS {
            chain.push_back((reference, curve.reversed()));
        } else if curve.end().distance(start) < JOINS {
            chain.push_front((reference, curve));
        } else if curve.start().distance(start) < JOINS {
            chain.push_front((reference, curve.reversed()));
        } else {
            return None;
        }
    }
    (!chain.is_empty()).then(|| chain.into_iter().map(|(reference, _)| reference).collect())
}

fn straight(curve: &CurveRef, evaluated: &Evaluated) -> Option<(Vec3, Vec3)> {
    curve.resolve(&evaluated.scope())?.line()
}

fn flat_face(reference: &GeomRef, evaluated: &Evaluated) -> bool {
    matches!(reference.element, Element::Face(_))
        && evaluated.locate(reference).is_some_and(|located| located.is_flat())
}

fn in_plane(plane: &Plane, (a, b): (Vec3, Vec3)) -> bool {
    plane.distance(a).abs() < IN_PLANE && plane.distance(b).abs() < IN_PLANE
}

/// A body no other slot already holds: a boolean of a body with itself
/// would be broken on arrival.
pub fn another_body(cx: &SlotCx, pick: &Pick) -> bool {
    let body = pick.body(cx.evaluated);
    !cx.others.iter().any(|other| matches!(other, Value::Body(id) if Some(*id) == body))
}

/// A curve lying in the plane of the regions another slot holds.
pub fn in_the_regions_plane(cx: &SlotCx, pick: &Pick) -> bool {
    let Some(line) = pick.as_curve().and_then(|curve| straight(&curve, cx.evaluated)) else { return true };
    cx.others.iter().all(|other| match other {
        Value::Regions { sketch, .. } => cx.evaluated.sketch(*sketch).is_none_or(|placed| in_plane(&placed.plane, line)),
        _ => true,
    })
}

/// Regions of a sketch whose plane holds the curve another slot holds.
pub fn around_the_axis(cx: &SlotCx, pick: &Pick) -> bool {
    let Pick::Region { sketch, .. } = pick else { return true };
    let Some(placed) = cx.evaluated.sketch(*sketch) else { return true };
    cx.others.iter().all(|other| match other {
        Value::Curve(curve) => straight(curve, cx.evaluated).is_none_or(|line| in_plane(&placed.plane, line)),
        _ => true,
    })
}
