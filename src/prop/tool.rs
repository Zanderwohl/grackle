//! Tools that make a feature from picks, and re-picking what an existing one
//! is about.
//!
//! A tool is a row in [`TOOLS`]: its name, its icon, the [`Slot`]s it wants
//! filled and a function that builds the feature from what they come to. A
//! re-pick takes its one slot from the feature's [`Field`](crate::prop::slot::Field).
//! Starting either fills the slots from the selection in pick order; whatever
//! is still missing is asked for ("Pick a plane"), and the viewport offers
//! only picks that fit. The picks a tool is collecting are the selection, so
//! there is one list of what is picked rather than two that can disagree.

use crate::get;
use crate::prop::construct::Making;
use crate::prop::document::{Pick, PropDoc, PropEditor};
use crate::prop::feature::{BooleanOp, End, Evaluated, FeatureOp, PatternKind, PropFeatureId};
use crate::prop::sketch::Sketch;
use crate::prop::sketch_edit::SketchSession;
use crate::prop::slot::{self, plane_through_face, Resolved, Slot, SlotCx, Value};
use crate::prop::surface::Surface;
use bevy::prelude::*;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    Make(Tool),
    /// Field `field` of [`FeatureOp::fields`].
    Repick { feature: PropFeatureId, field: usize },
}

pub struct ToolSpec {
    pub tool: Tool,
    /// Lang key.
    pub name: &'static str,
    /// Named for the picture, not the tool: see `documentation/icons.md`.
    pub icon: &'static str,
    pub slots: &'static [Slot],
    /// From one resolved value per slot, in slot order.
    pub build: fn(Vec<Resolved>) -> Option<FeatureOp>,
}

pub const TOOLS: &[ToolSpec] = &[
    ToolSpec { tool: Tool::Extrude, name: "prop.features.extrude", icon: "square_arrow", slots: &[slot::REGIONS], build: extrude },
    ToolSpec {
        tool: Tool::Revolve,
        name: "prop.features.revolve",
        icon: "axis_arrow_around",
        slots: &[slot::REGIONS, slot::AXIS],
        build: revolve,
    },
    ToolSpec {
        tool: Tool::Combine(BooleanOp::Union),
        name: "prop.boolean.union",
        icon: "venn_union",
        slots: &[slot::BODY, slot::SECOND_BODY],
        build: |values| combine(BooleanOp::Union, values),
    },
    ToolSpec {
        tool: Tool::Combine(BooleanOp::Subtract),
        name: "prop.boolean.subtract",
        icon: "venn_subtract",
        slots: &[slot::BODY, slot::SECOND_BODY],
        build: |values| combine(BooleanOp::Subtract, values),
    },
    ToolSpec {
        tool: Tool::Combine(BooleanOp::Intersect),
        name: "prop.boolean.intersect",
        icon: "venn_intersect",
        slots: &[slot::BODY, slot::SECOND_BODY],
        build: |values| combine(BooleanOp::Intersect, values),
    },
    ToolSpec { tool: Tool::Mirror, name: "prop.features.mirror", icon: "square_mirrored", slots: &[slot::BODY, slot::PLANE], build: mirror },
    ToolSpec {
        tool: Tool::LinearPattern,
        name: "prop.features.linear_pattern",
        icon: "two_squares",
        slots: &[slot::BODY, slot::DIRECTION],
        build: row,
    },
    ToolSpec {
        tool: Tool::RadialPattern,
        name: "prop.features.radial_pattern",
        icon: "squares_in_circle",
        slots: &[slot::BODY, slot::AXIS],
        build: circle,
    },
    ToolSpec { tool: Tool::Plane, name: "prop.features.plane", icon: "ground_square", slots: &[slot::MAKE_PLANE], build: made },
    ToolSpec { tool: Tool::Axis, name: "prop.features.axis", icon: "dashed_line_dots", slots: &[slot::MAKE_AXIS], build: made },
    ToolSpec { tool: Tool::Point, name: "prop.features.point", icon: "dot_crosshair", slots: &[slot::MAKE_POINT], build: made },
    ToolSpec { tool: Tool::Sketch, name: "prop.features.sketch", icon: "pencil", slots: &[slot::PLANE], build: sketch },
];

fn extrude(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Regions { sketch, entries }] = <[_; 1]>::try_from(values).ok()? else { return None };
    let end = End::Distance { distance: 0.05 };
    Some(FeatureOp::Extrude { sketch, regions: entries, symmetric: false, end, surface: Surface::default() })
}

fn revolve(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Regions { sketch, entries }, Resolved::Curve(axis)] = <[_; 2]>::try_from(values).ok()? else { return None };
    Some(FeatureOp::Revolve {
        sketch,
        regions: entries,
        degrees: 360.0,
        steps: 12,
        symmetric: false,
        axis,
        surface: Surface::default(),
    })
}

fn combine(op: BooleanOp, values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Body(target), Resolved::Body(tool)] = <[_; 2]>::try_from(values).ok()? else { return None };
    Some(FeatureOp::Boolean { op, target, tool })
}

fn mirror(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Body(target), Resolved::Plane(about)] = <[_; 2]>::try_from(values).ok()? else { return None };
    Some(FeatureOp::Mirror { target, offset: 0.0, keep_original: true, about })
}

fn row(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Body(target), Resolved::Direction(direction)] = <[_; 2]>::try_from(values).ok()? else { return None };
    Some(FeatureOp::Pattern { target, count: 3, along: PatternKind::Linear { spacing: 0.05, direction } })
}

fn circle(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Body(target), Resolved::Curve(axis)] = <[_; 2]>::try_from(values).ok()? else { return None };
    Some(FeatureOp::Pattern { target, count: 6, along: PatternKind::Radial { degrees: 360.0, axis } })
}

fn made(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Op(op)] = <[_; 1]>::try_from(values).ok()? else { return None };
    Some(op)
}

fn sketch(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Plane(on)] = <[_; 1]>::try_from(values).ok()? else { return None };
    Some(FeatureOp::Sketch { on, sketch: Sketch::default() })
}

impl Tool {
    pub fn spec(self) -> &'static ToolSpec {
        TOOLS.iter().find(|spec| spec.tool == self).expect("every tool has a row in TOOLS")
    }

    pub fn name(self) -> String {
        get!(self.spec().name)
    }

    pub fn icon(self) -> &'static str {
        self.spec().icon
    }
}

impl Task {
    pub fn slots(self, doc: &PropDoc) -> Vec<Slot> {
        match self {
            Task::Make(tool) => tool.spec().slots.to_vec(),
            Task::Repick { feature, field } => repicked(doc, feature, field).map(|f| f.slot).into_iter().collect(),
        }
    }

    pub fn name(self, doc: &PropDoc) -> String {
        match self {
            Task::Make(tool) => tool.name(),
            Task::Repick { feature, field } => repicked(doc, feature, field).map(|f| get!(f.label)).unwrap_or_default(),
        }
    }
}

fn repicked(doc: &PropDoc, feature: PropFeatureId, field: usize) -> Option<slot::Field> {
    doc.feature(feature)?.op.fields().into_iter().nth(field)
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
    let slots = task.slots(doc);
    let mut filled = Filled(vec![Vec::new(); slots.len()]);
    for pick in picks {
        let at = (0..slots.len()).find(|&i| {
            slots[i].kind.open(&filled.0[i]) && fits(task, &slots, i, pick, &filled, doc, evaluated)
        });
        if let Some(at) = at {
            filled.0[at].push(pick.clone());
        }
    }
    filled
}

/// What the other slots hold, for a check to compare against: the tool's
/// other slots, or a re-picked feature's other fields.
fn others(task: Task, slots: &[Slot], slot: usize, filled: &Filled, doc: &PropDoc, evaluated: &Evaluated) -> Vec<Value> {
    match task {
        Task::Make(_) => (0..slots.len())
            .filter(|&i| i != slot)
            .filter_map(|i| slots[i].kind.value(&filled.0[i], evaluated))
            .collect(),
        Task::Repick { feature, field } => doc
            .feature(feature)
            .map(|f| f.op.fields())
            .unwrap_or_default()
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != field)
            .filter_map(|(_, f)| Value::of_field(f))
            .collect(),
    }
}

fn fits(task: Task, slots: &[Slot], slot: usize, pick: &Pick, filled: &Filled, doc: &PropDoc, evaluated: &Evaluated) -> bool {
    // Re-picking may only name what already stands when the feature runs.
    if let Task::Repick { feature, .. } = task {
        let Some(at) = doc.index_of(feature) else { return false };
        if !pick.features().all(|id| doc.index_of(id).is_some_and(|index| index < at)) {
            return false;
        }
    }
    if !slots[slot].kind.fits(&filled.0[slot], pick, evaluated) {
        return false;
    }
    let others = others(task, slots, slot, filled, doc, evaluated);
    let cx = SlotCx { evaluated, others: &others };
    slots[slot].checks.iter().all(|check| check(&cx, pick))
}

pub fn is_ready(task: Task, filled: &Filled, doc: &PropDoc, evaluated: &Evaluated) -> bool {
    task.slots(doc).iter().zip(&filled.0).all(|(slot, has)| !slot.kind.waiting(has, evaluated))
}

/// Ready, and nothing more could be added: a last slot taking any number of
/// regions, or a construction another pick would change, waits to be told it
/// is done.
pub fn is_finished(task: Task, filled: &Filled, doc: &PropDoc, evaluated: &Evaluated) -> bool {
    let slots = task.slots(doc);
    let open_ended = slots.last().zip(filled.0.last()).is_some_and(|(slot, has)| slot.kind.open_ended(has));
    is_ready(task, filled, doc, evaluated) && !open_ended
}

/// The first slot still wanting a pick, or the open-ended last one.
pub fn prompt(task: Task, filled: &Filled, doc: &PropDoc, evaluated: &Evaluated) -> String {
    let slots = task.slots(doc);
    let first = slots.iter().zip(&filled.0).find(|(slot, has)| slot.kind.waiting(has, evaluated));
    match first {
        Some((slot, _)) => get!(slot.prompt),
        None if matches!(slots.last(), Some(Slot { kind: slot::Kind::Regions, .. })) => get!("prop.tool.pick.more_regions"),
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
            if is_ready(task, &filled, editor.doc(), evaluated) {
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
        true => is_ready(task, &filled, editor.doc(), evaluated),
        false => is_finished(task, &filled, editor.doc(), evaluated),
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

/// Adds what a construction makes, with a plane feature first for each face
/// standing in for a plane, as one undo step.
pub fn add_making(editor: &mut PropEditor, making: &Making) -> PropFeatureId {
    let faces = making.faces().into_iter().map(plane_through_face).collect();
    editor.add_chain(faces, |_, planes| Some(making.op(planes))).expect("always built")
}

fn finish(task: Task, filled: &Filled, editor: &mut PropEditor, evaluated: &Evaluated, session: &mut SketchSession) {
    let slots = task.slots(editor.doc());
    let Some(values): Option<Vec<Value>> =
        slots.iter().zip(&filled.0).map(|(slot, has)| slot.kind.value(has, evaluated)).collect()
    else {
        return;
    };
    // A plane feature for each face standing in for a plane, ahead of what
    // it is made for, all one undo step.
    let faces: Vec<FeatureOp> = values.iter().flat_map(Value::faces).map(plane_through_face).collect();
    match task {
        Task::Make(tool) => {
            let made = editor.add_chain(faces, |doc, planes| {
                let mut planes = planes.iter().copied();
                let resolved = values.into_iter().map(|value| value.resolve(&mut planes, doc)).collect();
                (tool.spec().build)(resolved)
            });
            let Some(id) = made else { return };
            editor.select(id);
            // Straight into it: an empty sketch is only ever made to be drawn in.
            if tool == Tool::Sketch {
                session.enter(id);
            }
        }
        Task::Repick { feature, field } => {
            let Some(value) = values.into_iter().next() else { return };
            editor.edit(|doc| {
                // Just above the feature, so they stand where it runs.
                let Some(at) = doc.index_of(feature) else { return };
                let planes: Vec<PropFeatureId> =
                    faces.into_iter().enumerate().map(|(i, op)| doc.insert_at(at + i, op)).collect();
                let resolved = value.resolve(&mut planes.into_iter(), doc);
                if let Some(op) = doc.feature_mut(feature).map(|f| &mut f.op) {
                    op.set_field(field, resolved);
                }
            });
            editor.select(feature);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::feature::{evaluate, Axis};
    use crate::prop::plane::OriginPlane;
    use crate::prop::profile::Placement;
    use crate::prop::reference::Source;
    use crate::prop::plane::{PlaneDef, PlaneRef};
    use crate::prop::reference::{Element, GeomRef};
    use crate::prop::refs::{CurveRef, DirectionRef};
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
        assert_eq!(prompt(task, &filled, &doc, &evaluated), get!("prop.tool.pick.plane"));
        assert!(!is_ready(task, &filled, &doc, &evaluated));
    }

    #[test]
    fn a_pick_goes_to_whichever_slot_it_fits_whatever_order_it_came_in() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::RadialPattern);
        let filled = fill(task, &[Pick::Axis(Axis::Y), Pick::Body(PropFeatureId(0))], &doc, &evaluated);
        assert!(is_finished(task, &filled, &doc, &evaluated));
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
        assert!(!is_ready(task, &fill(task, &corners[..2], &doc, &evaluated), &doc, &evaluated));
        assert!(is_finished(task, &fill(task, &corners, &doc, &evaluated), &doc, &evaluated));
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
        assert!(is_ready(task, &edge, &doc, &evaluated), "its middle, if Done");
        assert!(!is_finished(task, &edge, &doc, &evaluated));
        let with_plane = fill(
            task,
            &[element(0, Element::Edge(8)), Pick::Plane(PlaneRef::Origin(OriginPlane::Xy))],
            &doc,
            &evaluated,
        );
        assert!(is_finished(task, &with_plane, &doc, &evaluated));
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
        assert!(is_finished(task, &filled, &doc, &evaluated));
    }

    /// The check that keeps a new revolve's axis in its sketch's plane runs on
    /// a re-pick too, against the feature's own regions.
    #[test]
    fn a_repicked_axis_still_has_to_lie_in_the_sketch() {
        let mut doc = PropDoc::new("test");
        let sketch = doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xz), sketch: Sketch::default() });
        let revolve = doc.push(FeatureOp::Revolve {
            sketch,
            regions: vec![],
            degrees: 360.0,
            steps: 12,
            symmetric: false,
            axis: CurveRef::Axis { axis: Axis::X },
            surface: Surface::default(),
        });
        let evaluated = evaluate(&doc.features);
        let task = Task::Repick { feature: revolve, field: 1 };
        assert_eq!(task.name(&doc), get!("prop.inspector.axis"));
        assert!(accepts(task, &[], &Pick::Axis(Axis::Z), &doc, &evaluated));
        assert!(!accepts(task, &[], &Pick::Axis(Axis::Y), &doc, &evaluated), "Y stands off the XZ plane");
    }

    #[test]
    fn repicking_what_an_offset_plane_is_from_keeps_its_distance() {
        let mut editor = PropEditor::default();
        let mut doc = two_cubes();
        let plane = doc.push(FeatureOp::Plane {
            def: PlaneDef::Offset { distance: 0.3, from: PlaneRef::Origin(OriginPlane::Xy) },
        });
        editor.open(doc, None);
        let evaluated = editor.doc().evaluate();
        let mut active = ActiveTool::default();
        let mut session = SketchSession::default();
        start(Task::Repick { feature: plane, field: 0 }, &mut active, &mut editor, &evaluated, &mut session);
        editor.toggle(Pick::Plane(PlaneRef::Origin(OriginPlane::Xz)));
        advance(&mut active, &mut editor, &evaluated, &mut session, true);
        assert_eq!(active.0, None);
        assert_eq!(
            editor.doc().feature(plane).map(|f| &f.op),
            Some(&FeatureOp::Plane { def: PlaneDef::Offset { distance: 0.3, from: PlaneRef::Origin(OriginPlane::Xz) } })
        );
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
        let task = Task::Repick { feature: pattern, field: 1 };
        assert!(accepts(task, &[], &Pick::Axis(Axis::Y), &doc, &evaluated));
        assert!(!accepts(task, &[], &element(1, Element::Edge(0)), &doc, &evaluated), "cube 1 comes after");
    }
}
