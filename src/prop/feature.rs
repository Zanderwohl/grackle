//! The feature list a prop is, and how it evaluates. Like the map's
//! [`FeatureTimeline`], but a feature is an enum rather than a `typetag` trait
//! object, because a boolean has to know what it is subtracting. See CLAUDE.md,
//! "Modelling a prop".
//!
//! A feature that consumes bodies produces one under its own id; what is left
//! at the end is drawn. A feature naming a body that is not there is skipped
//! with a note rather than stopping evaluation, which would blank the viewport
//! whenever a feature was deleted.
//!
//! [`FeatureTimeline`]: crate::editor::editable::FeatureTimeline

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::get;
use crate::prop::profile::Placement;
use crate::prop::solid::Polygon;
use crate::prop::line::LineRef;
use crate::prop::plane::{Datum, OriginPlane, PlaneRef, MIN_ORIGIN_HALF};
use crate::prop::plane::Plane;
use crate::prop::profile::revolve_steps;
use crate::prop::region::{arrange, axis_clear, Region};
use crate::prop::sketch::SketchId;
use crate::prop::sketch::{PlacedSketch, Sketch};
use crate::prop::reference::{face_at, pickables, GeomRef, Located, Outline, Pickable, Wire};
use crate::prop::solid::{is_full_turn, Shape, Solid};
use crate::prop::surface::Surface;

/// Never reused, so a reference survives reordering and deletion.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct PropFeatureId(pub u32);

impl std::fmt::Display for PropFeatureId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOp {
    #[default]
    Union,
    Subtract,
    Intersect,
}

impl BooleanOp {
    pub const ALL: [BooleanOp; 3] = [BooleanOp::Union, BooleanOp::Subtract, BooleanOp::Intersect];

    pub fn name(self) -> String {
        match self {
            BooleanOp::Union => get!("prop.boolean.union"),
            BooleanOp::Subtract => get!("prop.boolean.subtract"),
            BooleanOp::Intersect => get!("prop.boolean.intersect"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    #[default]
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    pub fn normal(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Axis::X => "X",
            Axis::Y => "Y",
            Axis::Z => "Z",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "feature", rename_all = "snake_case")]
pub enum FeatureOp {
    Primitive { shape: Shape, placement: Placement, surface: Surface },
    /// Regions of a sketch pushed along its plane's normal, joined into one
    /// body. Scalars first: TOML needs every scalar before any sub-table.
    Extrude {
        sketch: PropFeatureId,
        /// Entries in the sketch's own region list.
        regions: Vec<SketchId>,
        /// Half each way.
        #[serde(default)]
        symmetric: bool,
        end: End,
        surface: Surface,
    },
    /// Regions of a sketch turned about a line lying in the sketch's plane,
    /// in straight steps. Scalars first, as for [`FeatureOp::Extrude`].
    Revolve {
        sketch: PropFeatureId,
        /// As for [`FeatureOp::Extrude`].
        regions: Vec<SketchId>,
        #[serde(with = "crate::prop::nice_f32::scalar")]
        degrees: f32,
        /// Clamped by [`revolve_steps`].
        steps: u32,
        /// Half each way of the sketch's plane.
        #[serde(default)]
        symmetric: bool,
        axis: LineRef,
        surface: Surface,
    },
    Boolean { op: BooleanOp, target: PropFeatureId, tool: PropFeatureId },
    /// About a plane, as it stood when this runs.
    Mirror {
        target: PropFeatureId,
        /// Metres along the plane's normal.
        #[serde(default, with = "crate::prop::nice_f32::scalar")]
        offset: f32,
        /// Union with the original rather than replacing it.
        #[serde(default)]
        keep_original: bool,
        about: PlaneRef,
    },
    /// Its own feature because what gets repainted is usually a boolean's
    /// result, which no earlier feature has a surface for.
    Paint { target: PropFeatureId, surface: Surface },
    /// Copies of a body joined into one. The original is the first copy.
    Pattern {
        target: PropFeatureId,
        count: u32,
        along: PatternKind,
    },
    /// Not geometry: something to sketch on and measure from. Through what
    /// [`crate::prop::plane`] accepts, as it stood when this runs.
    Plane { through: Vec<GeomRef> },
    /// A drawing on a plane, which makes no body; see [`crate::prop::sketch`].
    Sketch { on: PlaneRef, sketch: Sketch },
}

/// An enum so that going up to a face or through everything can be added
/// without changing the file.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum End {
    /// Metres along the plane's normal; negative goes the other way.
    Distance {
        #[serde(with = "crate::prop::nice_f32::scalar")]
        distance: f32,
    },
}

impl End {
    pub fn distance(&self) -> f32 {
        match *self {
            End::Distance { distance } => distance,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PatternKind {
    /// Each copy `spacing` metres further along `direction` than the last;
    /// negative goes back down it.
    Linear {
        #[serde(with = "crate::prop::nice_f32::scalar")]
        spacing: f32,
        direction: LineRef,
    },
    /// About `axis`. A whole turn spaces copies evenly; less puts the first
    /// and last at the ends of `degrees`.
    Radial {
        #[serde(with = "crate::prop::nice_f32::scalar")]
        degrees: f32,
        axis: LineRef,
    },
}

impl PatternKind {
    pub fn line(&self) -> &LineRef {
        match self {
            PatternKind::Linear { direction, .. } => direction,
            PatternKind::Radial { axis, .. } => axis,
        }
    }

    pub fn line_mut(&mut self) -> &mut LineRef {
        match self {
            PatternKind::Linear { direction, .. } => direction,
            PatternKind::Radial { axis, .. } => axis,
        }
    }

    /// Copy `index` of `count`, with [`Self::line`] resolved to `(a, b)`.
    pub fn copy(&self, (a, b): (Vec3, Vec3), index: u32, count: u32) -> Transform {
        let along = (b - a).normalize_or_zero();
        match *self {
            PatternKind::Linear { spacing, .. } => Transform::from_translation(along * spacing * index as f32),
            PatternKind::Radial { degrees, .. } => {
                // A whole turn would put the last copy on top of the first.
                let gaps = match is_full_turn(degrees) {
                    true => count,
                    false => count.saturating_sub(1).max(1),
                };
                let angle = (degrees / gaps as f32).to_radians() * index as f32;
                let turn = Quat::from_axis_angle(along, angle);
                Transform::from_translation(a - turn * a).with_rotation(turn)
            }
        }
    }
}

impl Default for FeatureOp {
    fn default() -> Self {
        FeatureOp::Primitive {
            shape: Shape::default(),
            placement: Placement::default(),
            surface: Surface::default(),
        }
    }
}

impl FeatureOp {
    pub fn type_name(&self) -> String {
        match self {
            FeatureOp::Primitive { shape, .. } => shape.name(),
            FeatureOp::Extrude { .. } => get!("prop.features.extrude"),
            FeatureOp::Revolve { .. } => get!("prop.features.revolve"),
            FeatureOp::Boolean { op, .. } => op.name(),
            FeatureOp::Mirror { .. } => get!("prop.features.mirror"),
            FeatureOp::Paint { .. } => get!("prop.features.paint"),
            FeatureOp::Pattern { .. } => get!("prop.features.pattern"),
            FeatureOp::Plane { .. } => get!("prop.features.plane"),
            FeatureOp::Sketch { .. } => get!("prop.features.sketch"),
        }
    }

    /// The single answer the evaluator, the delete check and the panel's
    /// dropdowns all use.
    pub fn consumes(&self) -> Vec<PropFeatureId> {
        match self {
            FeatureOp::Primitive { .. }
            | FeatureOp::Extrude { .. }
            | FeatureOp::Revolve { .. }
            | FeatureOp::Plane { .. }
            | FeatureOp::Sketch { .. } => vec![],
            FeatureOp::Boolean { target, tool, .. } => vec![*target, *tool],
            FeatureOp::Mirror { target, .. }
            | FeatureOp::Paint { target, .. }
            | FeatureOp::Pattern { target, .. } => vec![*target],
        }
    }

    /// Features this one names without consuming: what a plane was made
    /// through, the plane or line something is about, and every copy that
    /// geometry went through.
    pub fn references(&self) -> Vec<PropFeatureId> {
        let mut named: Vec<PropFeatureId> = match self {
            FeatureOp::Plane { through } => through.iter().flat_map(|r| r.source.features()).collect(),
            FeatureOp::Sketch { on, .. } => on.feature().into_iter().collect(),
            FeatureOp::Extrude { sketch, .. } => vec![*sketch],
            FeatureOp::Revolve { sketch, axis, .. } => std::iter::once(*sketch).chain(axis.features()).collect(),
            FeatureOp::Mirror { about, .. } => about.feature().into_iter().collect(),
            FeatureOp::Pattern { along, .. } => along.line().features(),
            _ => vec![],
        };
        named.sort();
        named.dedup();
        named
    }

    pub fn depends_on(&self) -> Vec<PropFeatureId> {
        let mut all = self.consumes();
        all.extend(self.references());
        all.sort();
        all.dedup();
        all
    }

    pub fn placement_mut(&mut self) -> Option<&mut Placement> {
        match self {
            FeatureOp::Primitive { placement, .. } => Some(placement),
            FeatureOp::Extrude { .. }
            | FeatureOp::Revolve { .. }
            | FeatureOp::Boolean { .. }
            | FeatureOp::Mirror { .. }
            | FeatureOp::Paint { .. }
            | FeatureOp::Pattern { .. }
            | FeatureOp::Plane { .. }
            | FeatureOp::Sketch { .. } => None,
        }
    }

    /// The local axis "points along" turns. Must match how each primitive is
    /// built: prism and cone on `+Y`. Shapes with no length of their own take
    /// `+Z`.
    pub fn main_axis(&self) -> Vec3 {
        match self {
            FeatureOp::Primitive { shape: Shape::Prism { .. } | Shape::Cone { .. }, .. } => Vec3::Y,
            _ => Vec3::Z,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropFeature {
    pub id: PropFeatureId,
    /// User-typed content, not a lang key.
    pub name: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// Not `#[serde(flatten)]`: TOML needs every scalar before any sub-table,
    /// which a flattened enum with nested tables cannot promise.
    pub op: FeatureOp,
}

fn enabled_by_default() -> bool {
    true
}

impl PropFeature {
    pub fn new(id: PropFeatureId, op: FeatureOp) -> PropFeature {
        PropFeature { id, name: op.type_name(), enabled: true, op }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub feature: PropFeatureId,
    pub message: String,
}

impl Problem {
    fn missing_body(feature: PropFeatureId) -> Self {
        Self { feature, message: get!("prop.problems.missing_body") }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Evaluated {
    /// The bodies left standing, in the order they were made.
    pub bodies: Vec<(PropFeatureId, Solid)>,
    pub problems: Vec<Problem>,
    /// Which bodies were live just before each feature ran. Recorded so the
    /// panel does not reimplement the consuming rule.
    pub live_before: Vec<Vec<PropFeatureId>>,
    /// Beside `bodies`, in the same order: the outlines each was made from.
    pub wires: Vec<(PropFeatureId, Vec<Wire>)>,
    /// The named corners and edges that can be seen; see [`pickables`].
    pub pickables: Vec<Pickable>,
    /// The origin planes, then plane features in list order.
    pub planes: Vec<Datum>,
    /// Sketches, in list order, placed where their planes were.
    pub sketches: Vec<PlacedSketch>,
    /// Metres each way of the origin the origin planes and axes are drawn.
    pub origin_half: f32,
}

impl Evaluated {
    pub fn triangle_count(&self) -> usize {
        self.bodies.iter().map(|(_, solid)| solid.triangle_count()).sum()
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        self.bounds_of(|_| true)
    }

    /// A feature already eaten has no body, so contributes nothing.
    pub fn bounds_of_bodies(&self, ids: &[PropFeatureId]) -> Option<(Vec3, Vec3)> {
        self.bounds_of(|id| ids.contains(&id))
    }

    fn bounds_of(&self, wanted: impl Fn(PropFeatureId) -> bool) -> Option<(Vec3, Vec3)> {
        self.bodies
            .iter()
            .filter(|(id, _)| wanted(*id))
            .filter_map(|(_, solid)| solid.bounds())
            .reduce(|(min_a, max_a), (min_b, max_b)| (min_a.min(min_b), max_a.max(max_b)))
    }

    /// Tested against faces, not boxes, so a click down a bore reaches what is
    /// behind. Faces are convex, so a fan triangulates them exactly.
    pub fn body_under(&self, origin: Vec3, direction: Vec3) -> Option<PropFeatureId> {
        self.face_under(origin, direction).map(|(id, _, _)| id)
    }

    pub fn face_distance(&self, ray: Ray3d) -> Option<f32> {
        self.face_under(ray.origin, *ray.direction).map(|(_, _, distance)| distance)
    }

    fn face_under(&self, origin: Vec3, direction: Vec3) -> Option<(PropFeatureId, &Polygon, f32)> {
        self.bodies
            .iter()
            .flat_map(|(id, solid)| solid.polygons().iter().map(move |polygon| (*id, polygon)))
            .filter_map(|(id, polygon)| {
                let v = &polygon.vertices;
                (1..v.len().saturating_sub(1))
                    .filter_map(|i| ray_triangle(origin, direction, v[0], v[i], v[i + 1]))
                    .min_by(f32::total_cmp)
                    .map(|distance| (id, polygon, distance))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
    }

    /// What a click on a body's surface lands on: the body, and the named flat
    /// face there if one matches. One that a boolean made has none.
    pub fn surface_under(&self, ray: Ray3d) -> Option<(PropFeatureId, Option<GeomRef>)> {
        let (body, polygon, distance) = self.face_under(ray.origin, *ray.direction)?;
        let wires = self.wires.iter().find(|(id, _)| *id == body).map(|(_, wires)| wires.as_slice())?;
        Some((body, face_at(wires, ray.get_point(distance), polygon.plane.normal)))
    }

    /// The sketch region under the pointer, nearest first, unless a face is
    /// in front of it. A region on a face wins over that face: picking it is
    /// the reason it was drawn there.
    pub fn region_under(&self, ray: Ray3d) -> Option<(PropFeatureId, &Region)> {
        let face = self.face_distance(ray);
        self.sketches
            .iter()
            .filter_map(|placed| {
                let hit = placed.plane.hit(ray)?;
                let distance = hit.distance(ray.origin);
                if face.is_some_and(|face| distance > face + 0.002 + face * 0.01) {
                    return None;
                }
                let region = placed.arrangement.region_at(placed.plane.flatten(hit))?;
                Some((distance, placed.id, region))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id, region)| (id, region))
    }

    pub fn sketch(&self, id: PropFeatureId) -> Option<&PlacedSketch> {
        self.sketches.iter().find(|placed| placed.id == id)
    }

    pub fn datum(&self, id: PlaneRef) -> Option<&Datum> {
        self.planes.iter().find(|datum| datum.id == id)
    }

    /// Where a reference is now, whether or not it can be seen.
    pub fn locate(&self, reference: &GeomRef) -> Option<Located> {
        self.wires
            .iter()
            .flat_map(|(_, wires)| wires)
            .find(|wire| wire.source == reference.source)
            .and_then(|wire| wire.outline.locate(reference.element))
    }

    /// A line as it is now, end to end as it is drawn: an origin axis runs
    /// across the origin planes.
    pub fn locate_line(&self, line: &LineRef) -> Option<(Vec3, Vec3)> {
        match line {
            LineRef::Origin { axis } => Some((-axis.normal() * self.origin_half, axis.normal() * self.origin_half)),
            _ => line.resolve(
                |edge| match self.locate(edge)? {
                    Located::Edge(a, b) => Some((a, b)),
                    Located::Point(_) | Located::Face { .. } => None,
                },
                &self.sketches,
            ),
        }
    }

    pub fn body_of(&self, reference: &GeomRef) -> Option<PropFeatureId> {
        self.wires
            .iter()
            .find(|(_, wires)| wires.iter().any(|wire| wire.source == reference.source))
            .map(|(id, _)| *id)
    }
}

/// Möller–Trumbore, from either side: an inside-out face can still be seen
/// and clicked.
fn ray_triangle(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let ab = b - a;
    let ac = c - a;
    let p = direction.cross(ac);
    let det = ab.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let t_vec = origin - a;
    let u = t_vec.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = t_vec.cross(ab);
    let v = direction.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = ac.dot(q) * inv;
    (t > 0.0).then_some(t)
}

/// Where a revolve turns its regions: local `+Y` along the axis from its first
/// point, local `+X` across it towards the regions — so every radius
/// [`Solid::revolve`] sees is positive — and local `Z` completing the frame.
struct AxisFrame {
    transform: Transform,
    origin: Vec2,
    along: Vec2,
    across: Vec2,
}

impl AxisFrame {
    /// `None` if the axis has no length or crosses any of the regions.
    fn new(plane: &Plane, a: Vec2, b: Vec2, regions: &[&Region]) -> Option<AxisFrame> {
        if !regions.iter().all(|region| axis_clear(region, a, b)) {
            return None;
        }
        let along = (b - a).try_normalize()?;
        // Either perpendicular in the plane; the one the regions are on.
        let mut across = Vec2::new(along.y, -along.x);
        let lean: f32 = regions.iter().flat_map(|region| &region.outer).map(|p| (*p - a).dot(across)).sum();
        if lean < 0.0 {
            across = -across;
        }
        let lift = |v: Vec2| *plane.x * v.x + *plane.y() * v.y;
        let (x, y) = (lift(across), lift(along));
        let basis = Mat3::from_cols(x, y, x.cross(y));
        let transform = Transform::from_translation(plane.lift(a)).with_rotation(Quat::from_mat3(&basis));
        Some(AxisFrame { transform, origin: a, along, across })
    }

    /// Sketch-plane points as (radius, height) about the axis.
    fn local(&self, points: &[Vec2]) -> Vec<Vec2> {
        points
            .iter()
            .map(|p| Vec2::new((*p - self.origin).dot(self.across), (*p - self.origin).dot(self.along)))
            .collect()
    }
}

/// A body mid-evaluation, with the outlines of everything that went into it.
struct Live {
    id: PropFeatureId,
    solid: Solid,
    wires: Vec<Wire>,
}

/// Where a reference is among the bodies live at this point in the list.
fn locate_in(live: &[Live], reference: &GeomRef) -> Option<Located> {
    live.iter()
        .flat_map(|body| &body.wires)
        .find(|wire| wire.source == reference.source)
        .and_then(|wire| wire.outline.locate(reference.element))
}

fn edge_of(live: &[Live], reference: &GeomRef) -> Option<(Vec3, Vec3)> {
    match locate_in(live, reference)? {
        Located::Edge(a, b) => Some((a, b)),
        Located::Point(_) | Located::Face { .. } => None,
    }
}

/// An origin plane, or a plane feature that has already run.
fn plane_of(on: PlaneRef, planes: &[Datum]) -> Option<Plane> {
    match on {
        PlaneRef::Origin(which) => Some(which.plane()),
        PlaneRef::Feature(_) => planes.iter().find(|datum| datum.id == on).map(|datum| datum.plane),
    }
}

/// Metres a revolve's axis may sit off its sketch's plane: the kernel's
/// rounding, not a modelling tolerance.
pub(crate) const IN_PLANE: f32 = 1e-4;

/// Every region a sweep names, as the sketch now stands, or a Problem naming
/// the first that is not there.
fn resolve_regions(
    feature: PropFeatureId,
    placed: &PlacedSketch,
    regions: &[SketchId],
    problems: &mut Vec<Problem>,
) -> Option<Vec<Region>> {
    if regions.is_empty() {
        problems.push(Problem { feature, message: get!("prop.problems.no_regions") });
        return None;
    }
    let mut found = Vec::with_capacity(regions.len());
    for &id in regions {
        match placed.resolve(id) {
            Some(region) => found.push(region),
            None => {
                let region = placed.region_label(id);
                problems.push(Problem { feature, message: get!("prop.problems.missing_region", "region", region) });
                return None;
            }
        }
    }
    Some(found)
}

/// A pure function of the list, so it runs without a `World`.
pub fn evaluate(features: &[PropFeature]) -> Evaluated {
    let mut live: Vec<Live> = Vec::new();
    let mut problems = Vec::new();
    let mut planes = Vec::new();
    let mut sketches = Vec::new();
    let mut live_before = Vec::with_capacity(features.len());

    for feature in features {
        live_before.push(live.iter().map(|body| body.id).collect());

        if !feature.enabled {
            continue;
        }

        // Find before taking: a boolean that took one operand and failed to
        // find the other would delete a body it never used.
        let find = |id: PropFeatureId, live: &[Live]| live.iter().position(|body| body.id == id);
        let made = |solid: Solid, outline: Outline| Live {
            id: feature.id,
            solid,
            wires: vec![Wire::new(feature.id, outline)],
        };

        match &feature.op {
            FeatureOp::Primitive { shape, placement, surface } => {
                let solid = shape.solid(*surface).transformed(placement.transform());
                live.push(made(solid, shape.outline().transformed(placement.transform())));
            }
            FeatureOp::Extrude { sketch, symmetric, end, surface, regions } => {
                let Some(placed) = sketches.iter().find(|placed: &&PlacedSketch| placed.id == *sketch) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.missing_sketch") });
                    continue;
                };
                let Some(found) = resolve_regions(feature.id, placed, regions, &mut problems) else { continue };
                let (frame, depth) = (placed.plane.transform(), end.distance());
                let mut outline = Outline::default();
                let mut solid: Option<Solid> = None;
                for region in &found {
                    let piece = Solid::extrude_region(&region.outer, &region.holes, depth, *symmetric, *surface)
                        .transformed(frame);
                    // Unioned rather than gathered: regions sharing a line
                    // must become one closed surface.
                    solid = Some(match solid {
                        Some(joined) => joined.union(&piece),
                        None => piece,
                    });
                    outline.append(Outline::extrude_region(&region.outer, &region.holes, depth, *symmetric));
                }
                let solid = solid.unwrap_or_default();
                if solid.is_empty() {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.empty_sweep") });
                    continue;
                }
                live.push(made(solid, outline.transformed(frame)));
            }
            FeatureOp::Revolve { sketch, axis, degrees, steps, symmetric, surface, regions } => {
                let Some(placed) = sketches.iter().find(|placed: &&PlacedSketch| placed.id == *sketch) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.missing_sketch") });
                    continue;
                };
                let Some(found) = resolve_regions(feature.id, placed, regions, &mut problems) else { continue };
                let Some((a, b)) = axis.resolve(|edge| edge_of(&live, edge), &sketches) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.missing_axis") });
                    continue;
                };
                if [a, b].iter().any(|p| placed.plane.distance(*p).abs() > IN_PLANE) {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.axis_off_plane") });
                    continue;
                }
                let (a, b) = (placed.plane.flatten(a), placed.plane.flatten(b));
                let Some(frame) = AxisFrame::new(&placed.plane, a, b, &found.iter().collect::<Vec<_>>()) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.axis_crosses") });
                    continue;
                };
                let steps = revolve_steps(*steps);
                let spin = match *symmetric {
                    true => Transform::from_rotation(Quat::from_rotation_y(-degrees.to_radians() / 2.0)),
                    false => Transform::IDENTITY,
                };
                let mut outline = Outline::default();
                let mut solid: Option<Solid> = None;
                for region in &found {
                    let outer = frame.local(&region.outer);
                    let holes: Vec<Vec<Vec2>> = region.holes.iter().map(|hole| frame.local(hole)).collect();
                    let piece = Solid::revolve_region(&outer, &holes, *degrees, steps, *surface)
                        .transformed(spin)
                        .transformed(frame.transform);
                    solid = Some(match solid {
                        Some(joined) => joined.union(&piece),
                        None => piece,
                    });
                    outline.append(Outline::revolve_region(&outer, &holes, *degrees, steps).transformed(spin));
                }
                let solid = solid.unwrap_or_default();
                if solid.is_empty() {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.empty_sweep") });
                    continue;
                }
                live.push(made(solid, outline.transformed(frame.transform)));
            }
            FeatureOp::Boolean { op, target, tool } => {
                // Otherwise reported as a missing body.
                if target == tool {
                    problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.self_boolean"),
                    });
                    continue;
                }
                let (Some(at_target), Some(at_tool)) = (find(*target, &live), find(*tool, &live))
                else {
                    problems.push(Problem::missing_body(feature.id));
                    continue;
                };
                // Higher index first, or removing one shifts the other.
                let (first, second) = (at_target.max(at_tool), at_target.min(at_tool));
                let (a, b) = if at_target < at_tool {
                    let b = live.remove(first);
                    let a = live.remove(second);
                    (a, b)
                } else {
                    let a = live.remove(first);
                    let b = live.remove(second);
                    (a, b)
                };
                let result = match op {
                    BooleanOp::Union => a.solid.union(&b.solid),
                    BooleanOp::Subtract => a.solid.subtract(&b.solid),
                    BooleanOp::Intersect => a.solid.intersect(&b.solid),
                };
                if result.is_empty() {
                    problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.nothing_left"),
                    });
                    continue;
                }
                let wires = a.wires.into_iter().chain(b.wires).collect();
                live.push(Live { id: feature.id, solid: result, wires });
            }
            FeatureOp::Mirror { target, about, offset, keep_original } => {
                let Some(plane) = plane_of(*about, &planes) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.no_mirror_plane") });
                    continue;
                };
                let Some(body) = find(*target, &live).map(|at| live.remove(at)) else {
                    problems.push(Problem::missing_body(feature.id));
                    continue;
                };
                let normal = *plane.normal;
                let origin = plane.origin + normal * *offset;
                let reflected = body.solid.clone().mirrored(origin, normal);
                let reflections = body
                    .wires
                    .iter()
                    .map(|wire| wire.copied(feature.id, 1, wire.outline.clone().mirrored(origin, normal)));
                let (solid, wires) = if *keep_original {
                    (body.solid.union(&reflected), body.wires.iter().cloned().chain(reflections).collect())
                } else {
                    (reflected, reflections.collect())
                };
                live.push(Live { id: feature.id, solid, wires });
            }
            FeatureOp::Paint { target, surface } => {
                let Some(body) = find(*target, &live).map(|at| live.remove(at)) else {
                    problems.push(Problem::missing_body(feature.id));
                    continue;
                };
                live.push(Live { id: feature.id, solid: body.solid.painted(*surface), wires: body.wires });
            }
            FeatureOp::Plane { through } => {
                let located: Option<Vec<Located>> = through.iter().map(|reference| locate_in(&live, reference)).collect();
                match located.and_then(|located| Datum::through(feature.id, &located)) {
                    Some(datum) => planes.push(datum),
                    None => problems.push(Problem { feature: feature.id, message: get!("prop.problems.no_plane") }),
                }
            }
            FeatureOp::Sketch { on, sketch } => {
                match plane_of(*on, &planes) {
                    Some(plane) => sketches.push(PlacedSketch {
                        id: feature.id,
                        plane,
                        sketch: sketch.clone(),
                        arrangement: arrange(sketch),
                    }),
                    None => problems.push(Problem {
                        feature: feature.id,
                        message: get!("prop.problems.no_sketch_plane"),
                    }),
                }
            }
            FeatureOp::Pattern { target, count, along } => {
                let Some(line) = along.line().resolve(|edge| edge_of(&live, edge), &sketches) else {
                    problems.push(Problem { feature: feature.id, message: get!("prop.problems.missing_line") });
                    continue;
                };
                let Some(body) = find(*target, &live).map(|at| live.remove(at)) else {
                    problems.push(Problem::missing_body(feature.id));
                    continue;
                };
                let count = (*count).max(1);
                // Unioned, not concatenated: overlapping copies must become one
                // closed surface or a later boolean keeps the wrong half.
                let solid = (1..count).fold(body.solid.clone(), |joined, index| {
                    joined.union(&body.solid.clone().transformed(along.copy(line, index, count)))
                });
                let mut wires = body.wires.clone();
                for index in 1..count {
                    let moved = along.copy(line, index, count);
                    wires.extend(
                        body.wires
                            .iter()
                            .map(|wire| wire.copied(feature.id, index, wire.outline.clone().transformed(moved))),
                    );
                }
                live.push(Live { id: feature.id, solid, wires });
            }
        }
    }

    let pickables = live.iter().flat_map(|body| pickables(body.id, &body.solid, &body.wires)).collect();
    // Framing whatever is standing: the furthest any body reaches from the
    // origin, plus the same margin a plane feature's square has.
    let reach = live
        .iter()
        .filter_map(|body| body.solid.bounds())
        .map(|(min, max)| min.abs().max(max.abs()).max_element())
        .fold(0.0, f32::max);
    let origin_half = (reach * 1.25).max(MIN_ORIGIN_HALF);
    planes.splice(0..0, OriginPlane::ALL.map(|which| Datum::origin(which, origin_half)));
    let (bodies, wires) = live.into_iter().map(|body| ((body.id, body.solid), (body.id, body.wires))).unzip();
    Evaluated { bodies, wires, pickables, problems, live_before, planes, sketches, origin_half }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if copies land on top of each other, including the last of a
    /// whole radial turn.
    #[test]
    fn a_pattern_makes_that_many_separate_copies() {
        let cube = primitive(0, Shape::Box { size: [0.1, 0.1, 0.1] }, Vec3::new(0.3, 0.0, 0.0));
        let one = evaluate(std::slice::from_ref(&cube)).bodies[0].1.volume();
        let pattern = |count: u32, along: PatternKind| {
            let evaluated = evaluate(&[
                cube.clone(),
                PropFeature::new(PropFeatureId(1), FeatureOp::Pattern { target: PropFeatureId(0), count, along }),
            ]);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            assert_eq!(evaluated.bodies.len(), 1, "the copies are one body");
            evaluated.bodies[0].1.volume()
        };

        let z = LineRef::Origin { axis: Axis::Z };
        let row = pattern(3, PatternKind::Linear { spacing: 0.2, direction: z.clone() });
        assert!((row - 3.0 * one).abs() < 1e-6, "{row} vs {}", 3.0 * one);
        let round = pattern(4, PatternKind::Radial { degrees: 360.0, axis: LineRef::Origin { axis: Axis::Y } });
        assert!((round - 4.0 * one).abs() < 1e-6, "{round} vs {}", 4.0 * one);
        let alone = pattern(1, PatternKind::Linear { spacing: 0.2, direction: z });
        assert!((alone - one).abs() < 1e-9);
    }

    /// Fails if picking uses boxes: the ray must pass down the bore.
    #[test]
    fn a_ray_picks_the_nearest_face_not_the_nearest_box() {
        let cube = |id: u32, z: f32| primitive(id, Shape::Box { size: [0.1, 0.1, 0.1] }, Vec3::new(0.0, 0.0, z));
        let near = cube(0, 0.0);
        let far = cube(1, -0.5);
        let evaluated = evaluate(&[near.clone(), far.clone()]);
        let from_front = evaluated.body_under(Vec3::new(0.0, 0.0, 1.0), Vec3::NEG_Z);
        assert_eq!(from_front, Some(PropFeatureId(0)));
        let beside = evaluated.body_under(Vec3::new(1.0, 0.0, 1.0), Vec3::NEG_Z);
        assert_eq!(beside, None);

        let bore = primitive(2, Shape::Box { size: [0.04, 0.04, 0.3] }, Vec3::ZERO);
        let drilled = PropFeature::new(
            PropFeatureId(3),
            FeatureOp::Boolean { op: BooleanOp::Subtract, target: PropFeatureId(0), tool: PropFeatureId(2) },
        );
        let evaluated = evaluate(&[near, far, bore, drilled]);
        assert_eq!(
            evaluated.body_under(Vec3::new(0.0, 0.0, 1.0), Vec3::NEG_Z),
            Some(PropFeatureId(1)),
            "the ray goes down the hole",
        );
        assert_eq!(
            evaluated.body_under(Vec3::new(0.04, 0.0, 1.0), Vec3::NEG_Z),
            Some(PropFeatureId(3)),
            "beside the hole is the drilled body, under its own id",
        );
    }

    fn primitive(id: u32, shape: Shape, at: Vec3) -> PropFeature {
        PropFeature::new(
            PropFeatureId(id),
            FeatureOp::Primitive {
                shape,
                placement: Placement::at(at.x, at.y, at.z),
                surface: Surface::default(),
            },
        )
    }

    fn unit_box(id: u32, at: Vec3) -> PropFeature {
        primitive(id, Shape::Box { size: [1.0, 1.0, 1.0] }, at)
    }

    /// Fails as a barrel drawn with its own bore inside it.
    #[test]
    fn a_boolean_consumes_both_of_its_operands() {
        let features = vec![
            unit_box(0, Vec3::ZERO),
            unit_box(1, Vec3::new(0.5, 0.0, 0.0)),
            PropFeature::new(
                PropFeatureId(2),
                FeatureOp::Boolean {
                    op: BooleanOp::Subtract,
                    target: PropFeatureId(0),
                    tool: PropFeatureId(1),
                },
            ),
        ];
        let result = evaluate(&features);
        assert!(result.problems.is_empty(), "{:?}", result.problems);
        assert_eq!(result.bodies.len(), 1, "the operands were left lying around");
        assert_eq!(result.bodies[0].0, PropFeatureId(2), "the result took the wrong id");
        assert!((result.bodies[0].1.volume() - 0.5).abs() < 1e-3);
    }

    /// Fails if a dangling reference blanks the viewport or eats its target.
    #[test]
    fn a_dangling_reference_is_a_note_rather_than_a_blank_prop() {
        let features = vec![
            unit_box(0, Vec3::ZERO),
            PropFeature::new(
                PropFeatureId(2),
                FeatureOp::Boolean {
                    op: BooleanOp::Subtract,
                    target: PropFeatureId(0),
                    tool: PropFeatureId(9),
                },
            ),
        ];
        let result = evaluate(&features);
        assert_eq!(result.problems.len(), 1);
        assert_eq!(result.problems[0].feature, PropFeatureId(2));
        assert_eq!(result.bodies.len(), 1, "the surviving body went missing too");
        assert_eq!(result.bodies[0].0, PropFeatureId(0), "the target was eaten by a failed boolean");
    }

    /// A feature that fails must put back whatever it took.
    #[test]
    fn a_boolean_pointed_at_itself_leaves_the_body_alone() {
        let features = vec![
            unit_box(0, Vec3::ZERO),
            PropFeature::new(
                PropFeatureId(1),
                FeatureOp::Boolean {
                    op: BooleanOp::Union,
                    target: PropFeatureId(0),
                    tool: PropFeatureId(0),
                },
            ),
        ];
        let result = evaluate(&features);
        assert_eq!(result.problems.len(), 1);
        assert_eq!(result.bodies.len(), 1);
        assert_eq!(result.bodies[0].0, PropFeatureId(0));
    }

    #[test]
    fn disabling_a_feature_removes_its_body() {
        let mut features = vec![unit_box(0, Vec3::ZERO), unit_box(1, Vec3::new(3.0, 0.0, 0.0))];
        features[0].enabled = false;
        let result = evaluate(&features);
        assert_eq!(result.bodies.len(), 1);
        assert_eq!(result.bodies[0].0, PropFeatureId(1));
    }

    #[test]
    fn a_kept_mirror_doubles_the_body() {
        let features = vec![
            unit_box(0, Vec3::new(1.0, 0.0, 0.0)),
            PropFeature::new(
                PropFeatureId(1),
                FeatureOp::Mirror {
                    target: PropFeatureId(0),
                    about: crate::prop::plane::PlaneRef::Origin(crate::prop::plane::OriginPlane::Yz),
                    offset: 0.0,
                    keep_original: true,
                },
            ),
        ];
        let result = evaluate(&features);
        assert!(result.problems.is_empty(), "{:?}", result.problems);
        assert_eq!(result.bodies.len(), 1);
        assert!((result.bodies[0].1.volume() - 2.0).abs() < 1e-3);
    }

    /// Fails as the panel offering bodies that are already eaten.
    #[test]
    fn the_live_set_is_recorded_as_each_feature_sees_it() {
        let features = vec![
            unit_box(0, Vec3::ZERO),
            unit_box(1, Vec3::new(0.5, 0.0, 0.0)),
            PropFeature::new(
                PropFeatureId(2),
                FeatureOp::Boolean {
                    op: BooleanOp::Subtract,
                    target: PropFeatureId(0),
                    tool: PropFeatureId(1),
                },
            ),
            unit_box(3, Vec3::new(9.0, 0.0, 0.0)),
        ];
        let result = evaluate(&features);
        assert_eq!(result.live_before[0], vec![]);
        assert_eq!(result.live_before[1], vec![PropFeatureId(0)]);
        assert_eq!(result.live_before[2], vec![PropFeatureId(0), PropFeatureId(1)]);
        assert_eq!(result.live_before[3], vec![PropFeatureId(2)]);
    }

    mod extrude {
        use super::super::*;
        use crate::prop::document::{parse, to_text, PropDoc};
        use crate::prop::plane::OriginPlane;
        use crate::prop::region::{arrange, Region, RegionRef};
        use crate::prop::sketch::{Sketch, SketchId};

        fn polygon(sketch: &mut Sketch, corners: &[[f32; 2]]) -> Vec<SketchId> {
            let points: Vec<SketchId> = corners.iter().map(|c| sketch.add_point(Vec2::from_array(*c))).collect();
            (0..points.len()).map(|i| sketch.add_line(points[i], points[(i + 1) % points.len()])).collect()
        }

        fn square(sketch: &mut Sketch, min: f32, max: f32) -> Vec<SketchId> {
            polygon(sketch, &[[min, min], [max, min], [max, max], [min, max]])
        }

        /// A sketch on `on`, then an extrude of the regions `pick` chooses.
        fn swept(
            on: OriginPlane,
            sketch: Sketch,
            distance: f32,
            symmetric: bool,
            pick: impl Fn(&[Region]) -> Vec<RegionRef>,
        ) -> (Vec<PropFeature>, Evaluated) {
            let mut sketch = sketch;
            let picked = pick(&arrange(&sketch).regions);
            let regions = adopt(&mut sketch, picked);
            let features = vec![
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(on), sketch }),
                PropFeature::new(
                    PropFeatureId(1),
                    FeatureOp::Extrude {
                        sketch: PropFeatureId(0),
                        symmetric,
                        end: End::Distance { distance },
                        surface: Surface::default(),
                        regions,
                    },
                ),
            ];
            let evaluated = evaluate(&features);
            (features, evaluated)
        }

        pub(super) fn adopt(sketch: &mut Sketch, names: Vec<RegionRef>) -> Vec<SketchId> {
            let arrangement = arrange(sketch);
            names.iter().map(|name| sketch.adopt_region(arrangement.find(name).unwrap())).collect()
        }

        pub(super) fn every(regions: &[Region]) -> Vec<RegionRef> {
            regions.iter().map(|region| region.name.clone()).collect()
        }

        #[test]
        fn a_square_extrudes_to_a_box_along_the_planes_normal() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (_, evaluated) = swept(OriginPlane::Xy, sketch.clone(), 0.1, false, every);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let (_, solid) = &evaluated.bodies[0];
            assert!((solid.volume() - 0.04 * 0.04 * 0.1).abs() < 1e-8, "{}", solid.volume());
            let (min, max) = solid.bounds().unwrap();
            assert!(min.z.abs() < 1e-6 && (max.z - 0.1).abs() < 1e-6, "{min} {max}");

            let (_, symmetric) = swept(OriginPlane::Xy, sketch.clone(), 0.1, true, every);
            let (min, max) = symmetric.bodies[0].1.bounds().unwrap();
            assert!((min.z + 0.05).abs() < 1e-6 && (max.z - 0.05).abs() < 1e-6, "{min} {max}");

            let (_, up) = swept(OriginPlane::Xz, sketch, 0.1, false, every);
            let (min, max) = up.bodies[0].1.bounds().unwrap();
            assert!(min.y.abs() < 1e-6 && (max.y - 0.1).abs() < 1e-6, "a floor sketch goes up: {min} {max}");
        }

        fn sketch_in(features: &mut [PropFeature]) -> &mut Sketch {
            match &mut features[0].op {
                FeatureOp::Sketch { sketch, .. } => sketch,
                _ => unreachable!(),
            }
        }

        /// Fails if drawing across a swept region changes, or loses, the body.
        #[test]
        fn a_line_drawn_across_a_swept_region_leaves_the_body_alone() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (mut features, before) = swept(OriginPlane::Xy, sketch, 0.1, false, every);
            let drawn = sketch_in(&mut features);
            let (a, b) = (drawn.add_point(Vec2::new(0.02, -0.01)), drawn.add_point(Vec2::new(0.02, 0.05)));
            drawn.add_line(a, b);

            let after = evaluate(&features);
            assert!(after.problems.is_empty(), "{:?}", after.problems);
            let (was, is) = (before.bodies[0].1.volume(), after.bodies[0].1.volume());
            assert!((was - is).abs() < 1e-9, "{was} vs {is}");
        }

        /// Fails if a merge is guessed at rather than reported by name.
        #[test]
        fn a_region_merged_away_is_a_problem_naming_its_label() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (a, b) = (sketch.add_point(Vec2::new(0.02, 0.0)), sketch.add_point(Vec2::new(0.02, 0.04)));
            let divider = sketch.add_line(a, b);
            let left = |regions: &[Region]| {
                vec![regions.iter().find(|r| r.contains(Vec2::new(0.01, 0.02))).unwrap().name.clone()]
            };
            let (mut features, _) = swept(OriginPlane::Xy, sketch, 0.1, false, left);
            let drawn = sketch_in(&mut features);
            drawn.regions[0].label = "Grip".into();
            drawn.remove(&[divider]);

            let after = evaluate(&features);
            assert!(after.bodies.is_empty());
            assert_eq!(after.problems.len(), 1);
            assert!(after.problems[0].message.contains("Grip"), "{}", after.problems[0].message);
        }

        /// Fails if two sweeps of one region get an entry each, so that
        /// re-picking it for one leaves the other behind.
        #[test]
        fn sweeps_of_one_region_share_its_entry() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let region = arrange(&sketch).regions[0].clone();
            let first = sketch.adopt_region(&region);
            assert_eq!(sketch.adopt_region(&region), first);
            assert_eq!(sketch.regions.len(), 1);
        }

        #[test]
        fn removing_a_sweep_forgets_its_region_unless_it_was_labelled() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            square(&mut sketch, 0.1, 0.14);
            let regions = arrange(&sketch).regions;
            let mut doc = PropDoc::new("test");
            let named = sketch.adopt_region(&regions[0]);
            sketch.adopt_region(&regions[1]);
            sketch.regions[0].label = "Kept".into();
            let on = doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch });
            let ids = match &doc.features[0].op {
                FeatureOp::Sketch { sketch, .. } => sketch.regions.iter().map(|r| r.id).collect(),
                _ => unreachable!(),
            };
            let sweep = doc.push(FeatureOp::Extrude {
                sketch: on,
                regions: ids,
                symmetric: false,
                end: End::Distance { distance: 0.1 },
                surface: Surface::default(),
            });

            doc.remove(sweep);
            let FeatureOp::Sketch { sketch, .. } = &doc.features[0].op else { unreachable!() };
            assert_eq!(sketch.regions.iter().map(|r| r.id).collect::<Vec<_>>(), vec![named]);
        }

        #[test]
        fn a_hole_is_left_out() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            square(&mut sketch, 0.01, 0.02);
            let biggest = |regions: &[Region]| {
                let ring = regions.iter().max_by(|a, b| a.area().total_cmp(&b.area())).unwrap();
                vec![ring.name.clone()]
            };
            let (_, evaluated) = swept(OriginPlane::Xy, sketch, 0.1, false, biggest);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let volume = evaluated.bodies[0].1.volume();
            let expected = (0.04 * 0.04 - 0.01 * 0.01) * 0.1;
            assert!((volume - expected).abs() < 1e-8, "{volume} vs {expected}");
        }

        /// Fails if regions sharing a line are gathered rather than joined.
        #[test]
        fn regions_side_by_side_become_one_body() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.02);
            square(&mut sketch, 0.01, 0.03);
            let (_, evaluated) = swept(OriginPlane::Xy, sketch, 0.05, false, every);
            assert_eq!(evaluated.bodies.len(), 1);
            let area = 2.0 * 0.02 * 0.02 - 0.01 * 0.01;
            assert!((evaluated.bodies[0].1.volume() - area * 0.05).abs() < 1e-8);
        }

        /// The extruded body is named like any other: its corners can be
        /// picked, and its far cap faces out along the plane's normal.
        #[test]
        fn an_extruded_body_has_named_corners_and_faces() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (_, evaluated) = swept(OriginPlane::Xy, sketch, 0.1, false, every);
            let corners = evaluated.pickables.iter().filter(|p| matches!(p.at, Located::Point(_))).count();
            assert_eq!(corners, 8);
            let far_cap = GeomRef {
                element: crate::prop::reference::Element::Face(1),
                source: crate::prop::reference::Source { feature: PropFeatureId(1), copies: vec![] },
            };
            let Some(Located::Face { normal, points }) = evaluated.locate(&far_cap) else { panic!() };
            assert!(normal.distance(Vec3::Z) < 1e-6);
            assert!(points.iter().all(|p| (p.z - 0.1).abs() < 1e-6));
        }

        #[test]
        fn a_region_is_picked_where_it_is_drawn() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (_, evaluated) = swept(OriginPlane::Xy, sketch, -0.1, false, every);
            // Down onto the sketch, which sits on the extrusion's top cap.
            let (id, region) = evaluated.region_under(Ray3d::new(Vec3::new(0.02, 0.02, 1.0), Dir3::NEG_Z)).unwrap();
            assert_eq!(id, PropFeatureId(0));
            assert!((region.area() - 0.0016).abs() < 1e-8);
            // From underneath, the body is in the way.
            assert!(evaluated.region_under(Ray3d::new(Vec3::new(0.02, 0.02, -1.0), Dir3::Z)).is_none());
        }

        #[test]
        fn an_extrude_survives_the_file() {
            let mut sketch = Sketch::default();
            square(&mut sketch, 0.0, 0.04);
            let (features, _) = swept(OriginPlane::Xy, sketch, 0.1, true, every);
            let mut doc = PropDoc::new("test");
            for feature in features {
                doc.push(feature.op);
            }
            let text = to_text(&doc).unwrap();
            assert_eq!(parse(&text).unwrap(), doc, "{text}");
        }
    }

    mod revolve {
        use super::super::*;
        use super::extrude::{adopt, every};
        use crate::prop::document::{parse, to_text, PropDoc};
        use crate::prop::plane::OriginPlane;
        use crate::prop::region::arrange;
        use crate::prop::sketch::{Sketch, SketchId};

        /// A rectangle `r0..r1` across and `0..h` up, and a construction axis
        /// along the plane's `y` at `axis_x`.
        fn ring(r0: f32, r1: f32, h: f32, axis_x: f32) -> (Sketch, SketchId) {
            let mut sketch = Sketch::default();
            let corners = [[r0, 0.0], [r1, 0.0], [r1, h], [r0, h]];
            let points: Vec<SketchId> = corners.iter().map(|c| sketch.add_point(Vec2::from_array(*c))).collect();
            for i in 0..4 {
                sketch.add_line(points[i], points[(i + 1) % 4]);
            }
            let (a, b) = (sketch.add_point(Vec2::new(axis_x, -0.1)), sketch.add_point(Vec2::new(axis_x, 0.1)));
            let axis = sketch.add_line(a, b);
            sketch.set_construction(axis, true);
            (sketch, axis)
        }

        fn turned(on: OriginPlane, mut sketch: Sketch, axis: SketchId, degrees: f32, steps: u32, symmetric: bool) -> Evaluated {
            let names = every(&arrange(&sketch).regions);
            let regions = adopt(&mut sketch, names);
            evaluate(&[
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(on), sketch }),
                PropFeature::new(
                    PropFeatureId(1),
                    FeatureOp::Revolve {
                        sketch: PropFeatureId(0),
                        axis: LineRef::Sketch { sketch: PropFeatureId(0), line: axis },
                        degrees,
                        steps,
                        symmetric,
                        surface: Surface::default(),
                        regions,
                    },
                ),
            ])
        }

        /// The area of a regular `n`-gon with corners `r` from its middle.
        fn ngon(n: u32, r: f32) -> f32 {
            n as f32 / 2.0 * r * r * (std::f32::consts::TAU / n as f32).sin()
        }

        #[test]
        fn a_rectangle_off_the_axis_turns_into_a_faceted_ring() {
            let (sketch, axis) = ring(0.02, 0.05, 0.1, 0.0);
            let evaluated = turned(OriginPlane::Xy, sketch, axis, 360.0, 12, false);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let volume = evaluated.bodies[0].1.volume();
            let expected = 0.1 * (ngon(12, 0.05) - ngon(12, 0.02));
            assert!((volume - expected).abs() < 1e-7, "{volume} vs {expected}");
            // Turned about the plane's y, which on XY is world Y.
            let (min, max) = evaluated.bodies[0].1.bounds().unwrap();
            assert!(min.y.abs() < 1e-6 && (max.y - 0.1).abs() < 1e-6, "{min} {max}");
        }

        /// Fails if the step count is not clamped to the low-poly range.
        #[test]
        fn steps_are_kept_between_three_and_twenty_four() {
            let volume = |steps| {
                let (sketch, axis) = ring(0.0, 0.05, 0.1, 0.0);
                turned(OriginPlane::Xy, sketch, axis, 360.0, steps, false).bodies[0].1.volume()
            };
            assert!((volume(100) - volume(24)).abs() < 1e-9);
            assert!((volume(1) - volume(3)).abs() < 1e-9);
            assert!((volume(24) - 0.1 * ngon(24, 0.05)).abs() < 1e-7);
        }

        #[test]
        fn a_part_turn_is_that_part_and_symmetric_straddles_the_plane() {
            let (sketch, axis) = ring(0.02, 0.05, 0.1, 0.0);
            let full = turned(OriginPlane::Xy, sketch.clone(), axis, 360.0, 12, false).bodies[0].1.volume();
            let quarter = turned(OriginPlane::Xy, sketch.clone(), axis, 90.0, 3, false);
            assert!((quarter.bodies[0].1.volume() * 4.0 - full).abs() < 1e-7);

            let both_ways = turned(OriginPlane::Xy, sketch, axis, 90.0, 3, true);
            let (min, max) = both_ways.bodies[0].1.bounds().unwrap();
            assert!((min.z + max.z).abs() < 1e-6, "even about the sketch's plane: {min} {max}");
        }

        /// Fails if the frame is fixed to one side: a region left of its axis
        /// turns as well as one to the right.
        #[test]
        fn a_region_either_side_of_its_axis_turns() {
            let (sketch, axis) = ring(-0.05, -0.02, 0.1, 0.0);
            let evaluated = turned(OriginPlane::Xy, sketch, axis, 360.0, 12, false);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            let expected = 0.1 * (ngon(12, 0.05) - ngon(12, 0.02));
            assert!((evaluated.bodies[0].1.volume() - expected).abs() < 1e-7);
        }

        #[test]
        fn an_axis_through_the_region_or_missing_is_a_problem() {
            let (sketch, axis) = ring(0.02, 0.05, 0.1, 0.03);
            let crossed = turned(OriginPlane::Xy, sketch.clone(), axis, 360.0, 12, false);
            assert!(crossed.bodies.is_empty());
            assert_eq!(crossed.problems.len(), 1);

            let missing = turned(OriginPlane::Xy, sketch, SketchId(999), 360.0, 12, false);
            assert!(missing.bodies.is_empty());
            assert_eq!(missing.problems.len(), 1);
        }

        /// Fails if a line that is not the sketch's own cannot be turned
        /// about, or one off the sketch's plane quietly turns something.
        #[test]
        fn any_line_in_the_sketchs_plane_is_an_axis() {
            let (mut sketch, _) = ring(0.02, 0.05, 0.1, 0.0);
            let names = every(&arrange(&sketch).regions);
            let regions = adopt(&mut sketch, names);
            let about = |axis: Axis| {
                evaluate(&[
                    PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch: sketch.clone() }),
                    PropFeature::new(
                        PropFeatureId(1),
                        FeatureOp::Revolve {
                            sketch: PropFeatureId(0),
                            regions: regions.clone(),
                            degrees: 360.0,
                            steps: 12,
                            symmetric: false,
                            axis: LineRef::Origin { axis },
                            surface: Surface::default(),
                        },
                    ),
                ])
            };
            let turned = about(Axis::Y);
            assert!(turned.problems.is_empty(), "{:?}", turned.problems);
            let expected = 0.1 * (ngon(12, 0.05) - ngon(12, 0.02));
            assert!((turned.bodies[0].1.volume() - expected).abs() < 1e-7);

            let off = about(Axis::Z);
            assert!(off.bodies.is_empty());
            assert_eq!(off.problems[0].message, get!("prop.problems.axis_off_plane"));
        }

        /// A part turn of a region with a hole: the cutter's caps are a step
        /// beyond the body's, and the volume still comes out exact.
        #[test]
        fn a_hole_is_left_out_of_a_part_turn() {
            let (mut sketch, axis) = ring(0.02, 0.06, 0.1, 0.0);
            let corners = [[0.03, 0.03], [0.05, 0.03], [0.05, 0.07], [0.03, 0.07]];
            let points: Vec<SketchId> = corners.iter().map(|c| sketch.add_point(Vec2::from_array(*c))).collect();
            for i in 0..4 {
                sketch.add_line(points[i], points[(i + 1) % 4]);
            }
            let outer = arrange(&sketch).regions.into_iter().max_by(|a, b| a.area().total_cmp(&b.area())).unwrap();
            let outer = sketch.adopt_region(&outer);
            let evaluated = evaluate(&[
                PropFeature::new(PropFeatureId(0), FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch }),
                PropFeature::new(
                    PropFeatureId(1),
                    FeatureOp::Revolve {
                        sketch: PropFeatureId(0),
                        axis: LineRef::Sketch { sketch: PropFeatureId(0), line: axis },
                        degrees: 120.0,
                        steps: 4,
                        symmetric: false,
                        surface: Surface::default(),
                        regions: vec![outer],
                    },
                ),
            ]);
            assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
            // A wedge of a 12-gon ring is a third of the whole ring; 4 steps
            // over 120 degrees is 12 per turn.
            let ring = |r0: f32, r1: f32, h: f32| h * (ngon(12, r1) - ngon(12, r0)) / 3.0;
            let expected = ring(0.02, 0.06, 0.1) - ring(0.03, 0.05, 0.04);
            let volume = evaluated.bodies[0].1.volume();
            assert!((volume - expected).abs() < 1e-7, "{volume} vs {expected}");
        }

        #[test]
        fn a_revolve_survives_the_file() {
            let (mut sketch, axis) = ring(0.02, 0.05, 0.1, 0.0);
            let names = every(&arrange(&sketch).regions);
            let regions = adopt(&mut sketch, names);
            let mut doc = PropDoc::new("test");
            let on = doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xz), sketch });
            doc.push(FeatureOp::Revolve {
                sketch: on,
                axis: LineRef::Sketch { sketch: on, line: axis },
                degrees: 270.0,
                steps: 9,
                symmetric: true,
                surface: Surface::default(),
                regions,
            });
            let text = to_text(&doc).unwrap();
            assert_eq!(parse(&text).unwrap(), doc, "{text}");
        }
    }
}