//! Tools that make a feature from picks, and re-picking what an existing one
//! is about.
//!
//! Every tool states what it needs as a list of [`Slot`]s. Starting one fills
//! the slots from the selection in pick order; whatever is still missing is
//! asked for ("Pick a plane"), and the viewport offers only picks that fit.
//! The picks a tool is collecting are the selection, so there is one list of
//! what is picked rather than two that can disagree.

use bevy::prelude::*;

use crate::get;
use crate::prop::construct::{Construct, Making};
use crate::prop::document::{Pick, PlaneSource, PropDoc, PropEditor};
use crate::prop::feature::{BooleanOp, End, Evaluated, FeatureOp, PatternKind, PropFeatureId, IN_PLANE};
use crate::prop::plane::{Plane, PlaneDef, PlaneRef};
use crate::prop::reference::{Element, GeomRef};
use crate::prop::refs::{AnyRef, CurveRef, Resolve};
use crate::prop::region::Region;
use crate::prop::sketch::Sketch;
use crate::prop::sketch_edit::{sketch_of_mut, SketchSession};
use crate::prop::surface::Surface;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Extrude,
    Revolve,
    Combine(BooleanOp),
    Mirror,
    LinearPattern,
    RadialPattern,
    Plane,
    Axis,
    Point,
    Sketch,
}

/// A reference on an existing feature, chosen by picking it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    RevolveAxis,
    MirrorPlane,
    PatternDirection,
    PatternAxis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    Make(Tool),
    Repick { feature: PropFeatureId, field: Field },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// A standing body, or a face standing in for one, not already taken by
    /// another slot.
    Body,
    /// One or more regions, all of one sketch.
    Regions,
    /// A straight curve: an edge, a sketch's line, an axis.
    Line,
    /// A straight curve, or a plane's or flat face's normal.
    Direction,
    /// A plane, or a flat face to make one through.
    Plane,
    /// Whatever makes the plane, axis or point; see [`crate::prop::construct`].
    Construct(Construct),
}

pub struct Slot {
    pub want: Want,
    /// Lang key of what is asked for while the slot is empty.
    pub prompt: &'static str,
}

const BODY: Slot = Slot { want: Want::Body, prompt: "prop.tool.pick.body" };
const SECOND_BODY: Slot = Slot { want: Want::Body, prompt: "prop.tool.pick.second_body" };
const REGIONS: Slot = Slot { want: Want::Regions, prompt: "prop.tool.pick.region" };
const AXIS: Slot = Slot { want: Want::Line, prompt: "prop.tool.pick.axis" };
const DIRECTION: Slot = Slot { want: Want::Direction, prompt: "prop.tool.pick.direction" };
const PLANE: Slot = Slot { want: Want::Plane, prompt: "prop.tool.pick.plane" };
const MAKE_PLANE: Slot = Slot { want: Want::Construct(Construct::Plane), prompt: "prop.tool.pick.through" };
const MAKE_AXIS: Slot = Slot { want: Want::Construct(Construct::Axis), prompt: "prop.tool.pick.axis_from" };
const MAKE_POINT: Slot = Slot { want: Want::Construct(Construct::Point), prompt: "prop.tool.pick.point_from" };

impl Task {
    pub fn slots(self) -> &'static [Slot] {
        match self {
            Task::Make(Tool::Extrude) => &[REGIONS],
            Task::Make(Tool::Revolve) => &[REGIONS, AXIS],
            Task::Make(Tool::Combine(_)) => &[BODY, SECOND_BODY],
            Task::Make(Tool::Mirror) => &[BODY, PLANE],
            Task::Make(Tool::LinearPattern) => &[BODY, DIRECTION],
            Task::Make(Tool::RadialPattern) => &[BODY, AXIS],
            Task::Make(Tool::Plane) => &[MAKE_PLANE],
            Task::Make(Tool::Axis) => &[MAKE_AXIS],
            Task::Make(Tool::Point) => &[MAKE_POINT],
            Task::Make(Tool::Sketch) => &[PLANE],
            Task::Repick { field: Field::RevolveAxis | Field::PatternAxis, .. } => &[AXIS],
            Task::Repick { field: Field::MirrorPlane, .. } => &[PLANE],
            Task::Repick { field: Field::PatternDirection, .. } => &[DIRECTION],
        }
    }

    pub fn name(self) -> String {
        match self {
            Task::Make(tool) => tool.name(),
            Task::Repick { field: Field::RevolveAxis | Field::PatternAxis, .. } => get!("prop.inspector.axis"),
            Task::Repick { field: Field::MirrorPlane, .. } => get!("prop.inspector.plane"),
            Task::Repick { field: Field::PatternDirection, .. } => get!("prop.inspector.direction"),
        }
    }
}

impl Tool {
    pub fn name(self) -> String {
        match self {
            Tool::Extrude => get!("prop.features.extrude"),
            Tool::Revolve => get!("prop.features.revolve"),
            Tool::Combine(op) => op.name(),
            Tool::Mirror => get!("prop.features.mirror"),
            Tool::LinearPattern => get!("prop.features.linear_pattern"),
            Tool::RadialPattern => get!("prop.features.radial_pattern"),
            Tool::Plane => get!("prop.features.plane"),
            Tool::Axis => get!("prop.features.axis"),
            Tool::Point => get!("prop.features.point"),
            Tool::Sketch => get!("prop.features.sketch"),
        }
    }

    /// Named for the picture, not the tool: see `documentation/icons.md`.
    pub fn icon(self) -> &'static str {
        match self {
            Tool::Extrude => "square_arrow",
            Tool::Revolve => "axis_arrow_around",
            Tool::Combine(BooleanOp::Union) => "venn_union",
            Tool::Combine(BooleanOp::Subtract) => "venn_subtract",
            Tool::Combine(BooleanOp::Intersect) => "venn_intersect",
            Tool::Mirror => "square_mirrored",
            Tool::LinearPattern => "two_squares",
            Tool::RadialPattern => "squares_in_circle",
            Tool::Plane => "ground_square",
            Tool::Axis => "dashed_line_dots",
            Tool::Point => "dot_crosshair",
            Tool::Sketch => "pencil",
        }
    }
}

#[derive(Resource, Default)]
pub struct ActiveTool(pub Option<Task>);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filled(pub Vec<Vec<Pick>>);

impl Filled {
    pub fn picks(&self) -> impl Iterator<Item = &Pick> {
        self.0.iter().flatten()
    }
}

/// Each pick, in order, into the first open slot it fits. A pick that fits
/// nowhere is dropped, so a selection made for something else does not stop a
/// tool starting.
pub fn fill(task: Task, picks: &[Pick], doc: &PropDoc, evaluated: &Evaluated) -> Filled {
    let slots = task.slots();
    let mut filled = Filled(vec![Vec::new(); slots.len()]);
    for pick in picks {
        let at = (0..slots.len()).find(|&i| {
            is_open(slots[i].want, &filled.0[i], evaluated) && fits(task, i, pick, &filled, doc, evaluated)
        });
        if let Some(at) = at {
            filled.0[at].push(pick.clone());
        }
    }
    filled
}

fn is_open(want: Want, has: &[Pick], _evaluated: &Evaluated) -> bool {
    match want {
        // Whether one more is taken is the construction's question.
        Want::Regions | Want::Construct(_) => true,
        Want::Body | Want::Line | Want::Direction | Want::Plane => has.is_empty(),
    }
}

fn fits(task: Task, slot: usize, pick: &Pick, filled: &Filled, doc: &PropDoc, evaluated: &Evaluated) -> bool {
    // Re-picking may only name what already stands when the feature runs.
    if let Task::Repick { feature, .. } = task {
        let Some(at) = doc.index_of(feature) else { return false };
        if !pick.features().all(|id| doc.index_of(id).is_some_and(|index| index < at)) {
            return false;
        }
    }
    let slots = task.slots();
    match slots[slot].want {
        Want::Body => {
            let Some(body) = pick.body(evaluated) else { return false };
            let taken = slots
                .iter()
                .zip(&filled.0)
                .filter(|(slot, _)| slot.want == Want::Body)
                .flat_map(|(_, has)| has)
                .any(|other| other.body(evaluated) == Some(body));
            !taken
        }
        Want::Regions => {
            let Pick::Region { sketch, region } = pick else { return false };
            let encloses = evaluated.sketch(*sketch).is_some_and(|placed| placed.arrangement.find(region).is_some());
            let same_sketch = filled.0[slot].iter().all(|other| matches!(other, Pick::Region { sketch: s, .. } if s == sketch));
            let axis_lies_in_it = match (evaluated.sketch(*sketch), picked_line(task, filled, evaluated)) {
                (Some(placed), Some(line)) => in_plane(&placed.plane, line),
                _ => true,
            };
            encloses && same_sketch && axis_lies_in_it
        }
        Want::Line => {
            let Some(line) = pick.as_curve().and_then(|curve| straight(&curve, evaluated)) else { return false };
            sweep_plane(task, filled, doc, evaluated).is_none_or(|plane| in_plane(&plane, line))
        }
        Want::Direction => pick.as_direction().is_some_and(|direction| direction.resolve(&evaluated.scope()).is_some()),
        Want::Plane => match pick.as_plane_source() {
            Some(PlaneSource::Plane(on)) => evaluated.datum(on).is_some(),
            Some(PlaneSource::Face(face)) => flat_face(&face, evaluated),
            None => false,
        },
        Want::Construct(construct) => construct.takes(&filled.0[slot], pick, &evaluated.scope()),
    }
}

fn flat_face(reference: &GeomRef, evaluated: &Evaluated) -> bool {
    matches!(reference.element, Element::Face(_))
        && evaluated.locate(reference).is_some_and(|located| located.is_flat())
}

fn straight(curve: &CurveRef, evaluated: &Evaluated) -> Option<(Vec3, Vec3)> {
    curve.resolve(&evaluated.scope())?.line()
}

/// The plane a revolve's axis has to lie in: its sketch's.
fn sweep_plane(task: Task, filled: &Filled, doc: &PropDoc, evaluated: &Evaluated) -> Option<Plane> {
    let sketch = match task {
        Task::Make(Tool::Revolve) => filled.0[0].iter().find_map(|pick| match pick {
            Pick::Region { sketch, .. } => Some(*sketch),
            _ => None,
        }),
        Task::Repick { feature, field: Field::RevolveAxis } => match doc.feature(feature).map(|f| &f.op) {
            Some(FeatureOp::Revolve { sketch, .. }) => Some(*sketch),
            _ => None,
        },
        _ => None,
    }?;
    evaluated.sketch(sketch).map(|placed| placed.plane)
}

fn picked_line(task: Task, filled: &Filled, evaluated: &Evaluated) -> Option<(Vec3, Vec3)> {
    let Task::Make(Tool::Revolve) = task else { return None };
    filled.0[1].first().and_then(Pick::as_curve).and_then(|curve| straight(&curve, evaluated))
}

fn in_plane(plane: &Plane, (a, b): (Vec3, Vec3)) -> bool {
    plane.distance(a).abs() < IN_PLANE && plane.distance(b).abs() < IN_PLANE
}

pub fn is_ready(task: Task, filled: &Filled, evaluated: &Evaluated) -> bool {
    task.slots().iter().zip(&filled.0).all(|(slot, has)| !waiting(slot.want, has, evaluated))
}

fn waiting(want: Want, has: &[Pick], evaluated: &Evaluated) -> bool {
    match want {
        Want::Construct(construct) => construct.making(has, &evaluated.scope()).is_none(),
        _ => has.is_empty(),
    }
}

/// Ready, and nothing more could be added: a last slot taking any number of
/// regions, or a construction another pick would change, waits to be told it
/// is done.
pub fn is_finished(task: Task, filled: &Filled, evaluated: &Evaluated) -> bool {
    let open_ended = match (task.slots().last(), filled.0.last()) {
        (Some(Slot { want: Want::Regions, .. }), _) => true,
        (Some(Slot { want: Want::Construct(construct), .. }), Some(has)) => construct.extendable(has),
        _ => false,
    };
    is_ready(task, filled, evaluated) && !open_ended
}

/// The first slot still wanting a pick, or the open-ended last one.
pub fn prompt(task: Task, filled: &Filled, evaluated: &Evaluated) -> String {
    let slots = task.slots();
    let first = slots.iter().zip(&filled.0).find(|(slot, has)| waiting(slot.want, has, evaluated));
    match first {
        Some((slot, _)) => get!(slot.prompt),
        None if matches!(slots.last(), Some(Slot { want: Want::Regions, .. })) => get!("prop.tool.pick.more_regions"),
        None => get!("prop.tool.pick.more"),
    }
}

/// Whether clicking `candidate` would do anything: unpick it, or fill a slot.
pub fn accepts(task: Task, picks: &[Pick], candidate: &Pick, doc: &PropDoc, evaluated: &Evaluated) -> bool {
    if picks.contains(candidate) {
        return true;
    }
    let mut with = picks.to_vec();
    with.push(candidate.clone());
    fill(task, &with, doc, evaluated).picks().any(|pick| pick == candidate)
}

/// Keeps only what `task` can use of the selection, and finishes at once if
/// that is everything.
pub fn start(
    task: Task,
    active: &mut ActiveTool,
    editor: &mut PropEditor,
    evaluated: &Evaluated,
    session: &mut SketchSession,
) {
    match task {
        // The selection was the feature being edited; what it is about is
        // picked fresh.
        Task::Repick { .. } => editor.clear_selection(),
        Task::Make(_) => {
            let filled = fill(task, editor.picks(), editor.doc(), evaluated);
            editor.set_picks(filled.picks().cloned().collect());
            if is_ready(task, &filled, evaluated) {
                active.0 = None;
                finish(task, &filled, editor, evaluated, session);
                return;
            }
        }
    }
    active.0 = Some(task);
}

/// Finishes the task once its picks are all in; with `confirm`, once it is
/// merely ready.
pub fn advance(
    active: &mut ActiveTool,
    editor: &mut PropEditor,
    evaluated: &Evaluated,
    session: &mut SketchSession,
    confirm: bool,
) {
    let Some(task) = active.0 else { return };
    if let Task::Repick { feature, .. } = task
        && editor.doc().feature(feature).is_none()
    {
        active.0 = None;
        return;
    }
    let filled = fill(task, editor.picks(), editor.doc(), evaluated);
    let done = match confirm {
        true => is_ready(task, &filled, evaluated),
        false => is_finished(task, &filled, evaluated),
    };
    if done {
        active.0 = None;
        finish(task, &filled, editor, evaluated, session);
    }
}

/// Puts the tool down, leaving the selection as it is.
pub fn put_down(active: &mut ActiveTool) {
    active.0 = None;
}

/// Puts the tool down, restoring the selection from before a re-pick.
pub fn cancel(active: &mut ActiveTool, editor: &mut PropEditor) {
    if let Some(Task::Repick { feature, .. }) = active.0.take()
        && editor.doc().feature(feature).is_some()
    {
        editor.select(feature);
    }
}

/// A plane feature through a flat face.
pub fn plane_through_face(face: GeomRef) -> FeatureOp {
    FeatureOp::Plane { def: PlaneDef::Through { through: vec![AnyRef::Face { face }] } }
}

/// Adds `make` on the picked plane, making a plane through a face first, as
/// one undo step.
fn add_on_plane(editor: &mut PropEditor, on: PlaneSource, make: impl FnOnce(PlaneRef) -> FeatureOp) -> PropFeatureId {
    match on {
        PlaneSource::Plane(on) => editor.add(make(on)),
        PlaneSource::Face(face) => editor.add_pair(plane_through_face(face), |plane| make(PlaneRef::Feature(plane))),
    }
}

/// Adds what a construction makes, with a plane feature first for each face
/// standing in for a plane, as one undo step.
pub fn add_making(editor: &mut PropEditor, making: &Making) -> PropFeatureId {
    let faces = making.faces().into_iter().map(plane_through_face).collect();
    editor.add_chain(faces, |planes| making.op(planes))
}

fn finish(task: Task, filled: &Filled, editor: &mut PropEditor, evaluated: &Evaluated, session: &mut SketchSession) {
    let slot = |i: usize| filled.0[i].first();
    let body = |i: usize| slot(i).and_then(|pick| pick.body(evaluated));
    let curve = |i: usize| slot(i).and_then(Pick::as_curve);
    let plane_source = |i: usize| slot(i).and_then(Pick::as_plane_source);
    let made = match task {
        Task::Make(Tool::Extrude) => sweep(editor, evaluated, &filled.0[0], |sketch, regions| FeatureOp::Extrude {
            sketch,
            regions,
            symmetric: false,
            end: End::Distance { distance: 0.05 },
            surface: Surface::default(),
        }),
        Task::Make(Tool::Revolve) => {
            let Some(axis) = curve(1) else { return };
            sweep(editor, evaluated, &filled.0[0], |sketch, regions| FeatureOp::Revolve {
                sketch,
                regions,
                degrees: 360.0,
                steps: 12,
                symmetric: false,
                axis,
                surface: Surface::default(),
            })
        }
        Task::Make(Tool::Combine(op)) => {
            let (Some(target), Some(tool)) = (body(0), body(1)) else { return };
            Some(editor.add(FeatureOp::Boolean { op, target, tool }))
        }
        Task::Make(Tool::Mirror) => {
            let (Some(target), Some(on)) = (body(0), plane_source(1)) else { return };
            Some(add_on_plane(editor, on, |about| FeatureOp::Mirror { target, offset: 0.0, keep_original: true, about }))
        }
        Task::Make(Tool::LinearPattern) => {
            let (Some(target), Some(direction)) = (body(0), slot(1).and_then(Pick::as_direction)) else { return };
            let along = PatternKind::Linear { spacing: 0.05, direction };
            Some(editor.add(FeatureOp::Pattern { target, count: 3, along }))
        }
        Task::Make(Tool::RadialPattern) => {
            let (Some(target), Some(axis)) = (body(0), curve(1)) else { return };
            let along = PatternKind::Radial { degrees: 360.0, axis };
            Some(editor.add(FeatureOp::Pattern { target, count: 6, along }))
        }
        Task::Make(tool @ (Tool::Plane | Tool::Axis | Tool::Point)) => {
            let construct = match tool {
                Tool::Plane => Construct::Plane,
                Tool::Axis => Construct::Axis,
                _ => Construct::Point,
            };
            let Some(making) = construct.making(&filled.0[0], &evaluated.scope()) else { return };
            let id = add_making(editor, &making);
            editor.select(id);
            return;
        }
        Task::Make(Tool::Sketch) => {
            let Some(on) = plane_source(0) else { return };
            let id = add_on_plane(editor, on, |on| FeatureOp::Sketch { on, sketch: Sketch::default() });
            editor.select(id);
            // Straight into it: an empty sketch is only ever made to be drawn in.
            session.enter(id);
            return;
        }
        Task::Repick { feature, field } => {
            repick(editor, feature, field, slot(0));
            editor.select(feature);
            return;
        }
    };
    if let Some(id) = made {
        editor.select(id);
    }
}

/// A sweep of the picked regions, adopted onto their sketch in the same step
/// so undoing the sweep takes them with it.
fn sweep(
    editor: &mut PropEditor,
    evaluated: &Evaluated,
    regions: &[Pick],
    make: impl FnOnce(PropFeatureId, Vec<crate::prop::sketch::SketchId>) -> FeatureOp,
) -> Option<PropFeatureId> {
    let Some(Pick::Region { sketch, .. }) = regions.first() else { return None };
    let sketch = *sketch;
    let placed = evaluated.sketch(sketch)?;
    let found: Vec<Region> = regions
        .iter()
        .map(|pick| match pick {
            Pick::Region { region, .. } => placed.arrangement.find(region).cloned(),
            _ => None,
        })
        .collect::<Option<_>>()?;
    Some(editor.add_built(|doc| {
        let ids = sketch_of_mut(doc, sketch)
            .map(|drawn| found.iter().map(|region| drawn.adopt_region(region)).collect())
            .unwrap_or_default();
        make(sketch, ids)
    }))
}

fn repick(editor: &mut PropEditor, feature: PropFeatureId, field: Field, pick: Option<&Pick>) {
    let Some(pick) = pick else { return };
    match field {
        Field::RevolveAxis | Field::PatternAxis => {
            let Some(curve) = pick.as_curve() else { return };
            editor.edit(|doc| set_curve(doc, feature, curve));
        }
        Field::PatternDirection => {
            let Some(direction) = pick.as_direction() else { return };
            editor.edit(|doc| {
                if let Some(FeatureOp::Pattern { along: PatternKind::Linear { direction: was, .. }, .. }) =
                    doc.feature_mut(feature).map(|f| &mut f.op)
                {
                    *was = direction;
                }
            });
        }
        Field::MirrorPlane => {
            let Some(on) = pick.as_plane_source() else { return };
            editor.edit(|doc| {
                let about = match on {
                    PlaneSource::Plane(on) => on,
                    // Just above the mirror, so it stands where the mirror runs.
                    PlaneSource::Face(face) => {
                        let Some(at) = doc.index_of(feature) else { return };
                        PlaneRef::Feature(doc.insert_at(at, plane_through_face(face)))
                    }
                };
                if let Some(FeatureOp::Mirror { about: was, .. }) = doc.feature_mut(feature).map(|f| &mut f.op) {
                    *was = about;
                }
            });
        }
    }
}

fn set_curve(doc: &mut PropDoc, feature: PropFeatureId, curve: CurveRef) {
    match doc.feature_mut(feature).map(|f| &mut f.op) {
        Some(FeatureOp::Revolve { axis, .. }) | Some(FeatureOp::Pattern { along: PatternKind::Radial { axis, .. }, .. }) => {
            *axis = curve
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::feature::{evaluate, Axis};
    use crate::prop::plane::OriginPlane;
    use crate::prop::profile::Placement;
    use crate::prop::reference::Source;
    use crate::prop::refs::DirectionRef;
    use crate::prop::solid::Shape;

    fn cube(at: Vec3) -> FeatureOp {
        FeatureOp::Primitive {
            shape: Shape::Box { size: [0.1; 3] },
            placement: Placement::at(at.x, at.y, at.z),
            surface: Surface::default(),
        }
    }

    /// Cubes `#0` and `#1`.
    fn two_cubes() -> PropDoc {
        let mut doc = PropDoc::new("test");
        doc.push(cube(Vec3::ZERO));
        doc.push(cube(Vec3::X * 0.3));
        doc
    }

    fn element(feature: u32, element: Element) -> Pick {
        Pick::Geom(GeomRef { element, source: Source { feature: PropFeatureId(feature), copies: vec![] } })
    }

    #[test]
    fn a_tool_takes_what_it_can_from_the_selection_in_order() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::Mirror);
        let picks = [Pick::Axis(Axis::X), Pick::Body(PropFeatureId(1)), Pick::Body(PropFeatureId(0))];
        let filled = fill(task, &picks, &doc, &evaluated);
        assert_eq!(filled.0[0], vec![Pick::Body(PropFeatureId(1))], "the first body that fits is the target");
        assert!(filled.0[1].is_empty(), "a second body is no plane, and an axis is not either");
        assert_eq!(prompt(task, &filled, &evaluated), get!("prop.tool.pick.plane"));
        assert!(!is_ready(task, &filled, &evaluated));
    }

    #[test]
    fn a_pick_goes_to_whichever_slot_it_fits_whatever_order_it_came_in() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::RadialPattern);
        let filled = fill(task, &[Pick::Axis(Axis::Y), Pick::Body(PropFeatureId(0))], &doc, &evaluated);
        assert!(is_finished(task, &filled, &evaluated));
    }

    #[test]
    fn combine_wants_two_different_bodies() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::Combine(BooleanOp::Union));
        let face = element(0, Element::Face(0));
        assert!(!accepts(task, &[Pick::Body(PropFeatureId(0))], &face, &doc, &evaluated), "a face of the same body");
        assert!(accepts(task, &[Pick::Body(PropFeatureId(0))], &Pick::Body(PropFeatureId(1)), &doc, &evaluated));
        assert!(accepts(task, &[Pick::Body(PropFeatureId(0))], &Pick::Body(PropFeatureId(0)), &doc, &evaluated), "unpicking always works");
    }

    #[test]
    fn a_plane_is_a_plane_or_a_flat_face_and_lines_are_lines() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let sketch = Task::Make(Tool::Sketch);
        assert!(accepts(sketch, &[], &Pick::Plane(PlaneRef::Origin(OriginPlane::Xy)), &doc, &evaluated));
        assert!(accepts(sketch, &[], &element(0, Element::Face(0)), &doc, &evaluated));
        assert!(!accepts(sketch, &[], &element(0, Element::Edge(0)), &doc, &evaluated));
        let pattern = Task::Make(Tool::LinearPattern);
        let picks = [Pick::Body(PropFeatureId(0))];
        assert!(accepts(pattern, &picks, &element(1, Element::Edge(0)), &doc, &evaluated));
        assert!(accepts(pattern, &picks, &Pick::Axis(Axis::Z), &doc, &evaluated));
        assert!(!accepts(pattern, &picks, &element(1, Element::Point(0)), &doc, &evaluated));
    }

    #[test]
    fn a_plane_tool_finishes_the_moment_its_picks_make_a_plane() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::Plane);
        let corners: Vec<Pick> = (0..3).map(|i| element(0, Element::Point(i))).collect();
        assert!(!is_ready(task, &fill(task, &corners[..2], &doc, &evaluated), &evaluated));
        assert!(is_finished(task, &fill(task, &corners, &doc, &evaluated), &evaluated));
    }

    #[test]
    fn a_tool_with_everything_picked_makes_its_feature_at_once() {
        let mut editor = PropEditor::default();
        editor.open(two_cubes(), None);
        let evaluated = editor.doc().evaluate();
        editor.set_picks(vec![Pick::Body(PropFeatureId(0)), Pick::Plane(PlaneRef::Origin(OriginPlane::Yz))]);
        let mut active = ActiveTool::default();
        start(Task::Make(Tool::Mirror), &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        assert_eq!(active.0, None);
        assert!(matches!(
            editor.doc().features.last().map(|f| &f.op),
            Some(FeatureOp::Mirror { target: PropFeatureId(0), about: PlaneRef::Origin(OriginPlane::Yz), .. })
        ));
    }

    #[test]
    fn mirroring_about_a_face_makes_the_plane_too_in_one_step() {
        let mut editor = PropEditor::default();
        editor.open(two_cubes(), None);
        let evaluated = editor.doc().evaluate();
        let mut active = ActiveTool::default();
        editor.set_picks(vec![Pick::Body(PropFeatureId(1))]);
        start(Task::Make(Tool::Mirror), &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        assert_eq!(active.0, Some(Task::Make(Tool::Mirror)), "still wants a plane");
        editor.toggle(element(0, Element::Face(0)));
        advance(&mut active, &mut editor, &evaluated, &mut SketchSession::default(), false);
        assert_eq!(active.0, None);
        let ops: Vec<&FeatureOp> = editor.doc().features.iter().map(|f| &f.op).collect();
        assert!(matches!(ops[2], FeatureOp::Plane { .. }));
        assert!(matches!(ops[3], FeatureOp::Mirror { about: PlaneRef::Feature(_), .. }));
        let mirrored = editor.doc().evaluate();
        assert!(mirrored.problems.is_empty(), "{:?}", mirrored.problems);
        editor.undo();
        assert_eq!(editor.doc().features.len(), 2, "one undo takes both");
    }

    #[test]
    fn an_axis_through_two_corners_is_made_at_once() {
        let mut editor = PropEditor::default();
        editor.open(two_cubes(), None);
        let evaluated = editor.doc().evaluate();
        editor.set_picks(vec![element(0, Element::Point(0)), element(1, Element::Point(0))]);
        let mut active = ActiveTool::default();
        start(Task::Make(Tool::Axis), &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        assert_eq!(active.0, None);
        let made = editor.doc().features.last().unwrap().id;
        assert_eq!(editor.picks(), &[Pick::DatumAxis(made)], "picked as an axis, so it can be used as one");
        let evaluated = editor.doc().evaluate();
        assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
        assert_eq!(evaluated.axes.len(), 1);
    }

    #[test]
    fn a_point_on_a_line_waits_in_case_a_plane_follows() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::Point);
        let edge = fill(task, &[element(0, Element::Edge(0))], &doc, &evaluated);
        assert!(is_ready(task, &edge, &evaluated), "its middle, if Done");
        assert!(!is_finished(task, &edge, &evaluated));
        let with_plane = fill(
            task,
            &[element(0, Element::Edge(8)), Pick::Plane(PlaneRef::Origin(OriginPlane::Xy))],
            &doc,
            &evaluated,
        );
        assert!(is_finished(task, &with_plane, &evaluated));
    }

    #[test]
    fn halfway_between_two_faces_makes_their_planes_too_in_one_step() {
        let mut editor = PropEditor::default();
        editor.open(two_cubes(), None);
        let evaluated = editor.doc().evaluate();
        editor.set_picks(vec![element(0, Element::Face(0)), element(0, Element::Face(1))]);
        let mut active = ActiveTool::default();
        start(Task::Make(Tool::Plane), &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        assert_eq!(active.0, None);
        let ops: Vec<&FeatureOp> = editor.doc().features.iter().map(|f| &f.op).collect();
        assert_eq!(ops.len(), 5);
        assert!(matches!(ops[4], FeatureOp::Plane { def: PlaneDef::Midplane { .. } }));
        assert!(editor.doc().evaluate().problems.is_empty());
        editor.undo();
        assert_eq!(editor.doc().features.len(), 2, "one undo takes all three");
    }

    #[test]
    fn a_row_runs_along_a_planes_normal() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::LinearPattern);
        let filled = fill(task, &[Pick::Body(PropFeatureId(0)), Pick::Plane(PlaneRef::Origin(OriginPlane::Xz))], &doc, &evaluated);
        assert!(is_finished(task, &filled, &evaluated));
    }

    #[test]
    fn a_repick_only_offers_what_stands_before_the_feature() {
        let mut doc = two_cubes();
        let pattern = doc.insert_at(1, FeatureOp::Pattern {
            target: PropFeatureId(0),
            count: 2,
            along: PatternKind::Linear { spacing: 0.2, direction: DirectionRef::Along { curve: CurveRef::Axis { axis: Axis::X } } },
        });
        let evaluated = evaluate(&doc.features);
        let task = Task::Repick { feature: pattern, field: Field::PatternDirection };
        assert!(accepts(task, &[], &Pick::Axis(Axis::Y), &doc, &evaluated));
        assert!(!accepts(task, &[], &element(1, Element::Edge(0)), &doc, &evaluated), "cube 1 comes after");
    }
}
