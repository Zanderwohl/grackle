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

use crate::common::wizard;
use crate::get;
use crate::prop::construct::Making;
use crate::prop::document::{Pick, PropDoc, PropEditor};
use crate::prop::feature::{BodyRef, BooleanOp, End, PropFeature, Evaluated, FeatureOp, PatternKind, Profile, ProfileSource, PropFeatureId, SweepOp};
use crate::prop::path::Orient;
use crate::prop::refs::{AnyRef, Curve3, PathRef, Resolve, JOINS};
use crate::prop::sketch::Sketch;
use crate::prop::sketch_edit::SketchSession;
use crate::prop::slot::{self, plane_through_face, Resolved, Slot, SlotCx, Value};
use crate::prop::surface::Surface;
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Extrude,
    Revolve,
    Sweep,
    Loft,
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
    ToolSpec { tool: Tool::Sweep, name: "prop.features.sweep", icon: "square_along_path", slots: &[slot::REGIONS, slot::PATH], build: along },
    ToolSpec { tool: Tool::Loft, name: "prop.features.loft", icon: "square_joined_to_circle", slots: &[slot::PROFILES], build: loft },
    ToolSpec {
        tool: Tool::Combine(BooleanOp::Union),
        name: "prop.boolean.union",
        icon: "venn_union",
        slots: &[slot::BODIES_2],
        build: |values| combine(BooleanOp::Union, values),
    },
    ToolSpec {
        tool: Tool::Combine(BooleanOp::Subtract),
        name: "prop.boolean.subtract",
        icon: "venn_subtract",
        slots: &[slot::TARGETS, slot::TOOLS],
        build: |values| combine(BooleanOp::Subtract, values),
    },
    ToolSpec {
        tool: Tool::Combine(BooleanOp::Intersect),
        name: "prop.boolean.intersect",
        icon: "venn_intersect",
        slots: &[slot::BODIES_2],
        build: |values| combine(BooleanOp::Intersect, values),
    },
    ToolSpec { tool: Tool::Mirror, name: "prop.features.mirror", icon: "square_mirrored", slots: &[slot::BODIES, slot::PLANE], build: mirror },
    ToolSpec {
        tool: Tool::LinearPattern,
        name: "prop.features.linear_pattern",
        icon: "two_squares",
        slots: &[slot::BODIES, slot::DIRECTION],
        build: row,
    },
    ToolSpec {
        tool: Tool::RadialPattern,
        name: "prop.features.radial_pattern",
        icon: "squares_in_circle",
        slots: &[slot::BODIES, slot::AXIS],
        build: circle,
    },
    ToolSpec { tool: Tool::Plane, name: "prop.features.plane", icon: "ground_square", slots: &[slot::MAKE_PLANE], build: made },
    ToolSpec { tool: Tool::Axis, name: "prop.features.axis", icon: "dashed_line_dots", slots: &[slot::MAKE_AXIS], build: made },
    ToolSpec { tool: Tool::Point, name: "prop.features.point", icon: "dot_crosshair", slots: &[slot::MAKE_POINT], build: made },
    ToolSpec { tool: Tool::Sketch, name: "prop.features.sketch", icon: "pencil", slots: &[slot::PLANE], build: sketch },
];

fn profile_source(value: Resolved) -> Option<ProfileSource> {
    match value {
        Resolved::Regions { sketch, entries } => Some(ProfileSource::Regions { sketch, regions: entries }),
        Resolved::ProfileFace(face) => Some(ProfileSource::Face { face }),
        _ => None,
    }
}

fn extrude(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [profile] = <[_; 1]>::try_from(values).ok()?;
    Some(FeatureOp::Extrude {
        profile: profile_source(profile)?,
        taper: 0.0,
        op: SweepOp::NewBody,
        targets: vec![],
        end: End::Distance { distance: End::DEFAULT_DISTANCE },
        other: None,
        direction: None,
        surface: Surface::default(),
    })
}

fn revolve(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [profile, Resolved::Curve(axis)] = <[_; 2]>::try_from(values).ok()? else { return None };
    Some(FeatureOp::Revolve {
        profile: profile_source(profile)?,
        degrees: 360.0,
        steps: 12,
        symmetric: false,
        op: SweepOp::NewBody,
        targets: vec![],
        axis,
        surface: Surface::default(),
    })
}

fn along(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [profile, Resolved::Path(path)] = <[_; 2]>::try_from(values).ok()? else { return None };
    Some(FeatureOp::Sweep {
        profile: profile_source(profile)?,
        orient: Orient::Minimal,
        twist: 0.0,
        scale: 1.0,
        op: SweepOp::NewBody,
        targets: vec![],
        path,
        surface: Surface::default(),
    })
}

fn loft(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Profiles(profiles)] = <[_; 1]>::try_from(values).ok()? else { return None };
    let profiles = profiles.into_iter().map(|(sketch, region)| Profile { sketch, region, start: None }).collect();
    Some(FeatureOp::Loft { closed: false, op: SweepOp::NewBody, targets: vec![], profiles, surface: Surface::default() })
}

fn combine(op: BooleanOp, values: Vec<Resolved>) -> Option<FeatureOp> {
    Some(match (op, &values[..]) {
        (BooleanOp::Union, [Resolved::Bodies(bodies)]) => FeatureOp::Union { bodies: bodies.clone() },
        (BooleanOp::Intersect, [Resolved::Bodies(bodies)]) => FeatureOp::Intersect { bodies: bodies.clone() },
        (BooleanOp::Subtract, [Resolved::Bodies(targets), Resolved::Bodies(tools)]) => {
            FeatureOp::Subtract { targets: targets.clone(), tools: tools.clone() }
        }
        _ => return None,
    })
}

fn mirror(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Bodies(targets), Resolved::Plane(about)] = <[_; 2]>::try_from(values).ok()? else { return None };
    Some(FeatureOp::Mirror { targets, offset: 0.0, keep_original: true, about })
}

fn row(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Bodies(targets), Resolved::Direction(direction)] = <[_; 2]>::try_from(values).ok()? else { return None };
    Some(FeatureOp::Pattern { targets, count: 3, along: PatternKind::Linear { spacing: 0.05, direction } })
}

fn circle(values: Vec<Resolved>) -> Option<FeatureOp> {
    let [Resolved::Bodies(targets), Resolved::Curve(axis)] = <[_; 2]>::try_from(values).ok()? else { return None };
    Some(FeatureOp::Pattern { targets, count: 6, along: PatternKind::Radial { degrees: 360.0, axis } })
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
pub struct ActiveTool {
    pub task: Option<Task>,
    /// The picks after which the tool was moved on to its next slot; see
    /// [`wizard::move_on`].
    pub moved_on: Vec<Pick>,
}

impl ActiveTool {
    fn set(&mut self, task: Option<Task>) {
        self.task = task;
        self.moved_on.clear();
    }
}

pub type Filled = wizard::Filled<Pick>;

/// A task's slots as wizard steps, against the document and its evaluation.
struct PropSteps<'a> {
    task: Task,
    slots: Vec<Slot>,
    doc: &'a PropDoc,
    evaluated: &'a Evaluated,
}

impl<'a> PropSteps<'a> {
    fn new(task: Task, doc: &'a PropDoc, evaluated: &'a Evaluated) -> Self {
        PropSteps { task, slots: task.slots(doc), doc, evaluated }
    }

    /// What the other slots hold, for a check to compare against: the tool's
    /// other slots, or a re-picked feature's other fields.
    fn others(&self, slot: usize, filled: &Filled) -> Vec<Value> {
        match self.task {
            Task::Make(_) => (0..self.slots.len())
                .filter(|&i| i != slot)
                .filter_map(|i| self.slots[i].kind.value(&filled.has[i], self.evaluated))
                .collect(),
            Task::Repick { feature, field } => self
                .doc
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
}

impl wizard::Steps for PropSteps<'_> {
    type Pick = Pick;

    fn count(&self) -> usize {
        self.slots.len()
    }

    fn answered(&self, step: usize, has: &[Pick]) -> bool {
        self.slots[step].kind.answered(has, self.evaluated)
    }

    fn open(&self, step: usize, has: &[Pick]) -> bool {
        self.slots[step].kind.open(has)
    }

    fn waits(&self, step: usize, has: &[Pick]) -> bool {
        self.slots[step].kind.waits(has)
    }

    fn fits(&self, step: usize, pick: &Pick, filled: &Filled) -> bool {
        // Re-picking may only name what already stands when the feature runs.
        if let Task::Repick { feature, .. } = self.task {
            let Some(at) = self.doc.index_of(feature) else { return false };
            if !pick.features().all(|id| self.doc.index_of(id).is_some_and(|index| index < at)) {
                return false;
            }
        }
        let slot = &self.slots[step];
        if !slot.kind.fits(&filled.has[step], pick, self.evaluated) {
            return false;
        }
        let others = self.others(step, filled);
        let cx = SlotCx { evaluated: self.evaluated, others: &others };
        slot.checks.iter().all(|check| check(&cx, pick))
    }
}

pub fn fill(task: Task, picks: &[Pick], moved_on: &[Pick], doc: &PropDoc, evaluated: &Evaluated) -> Filled {
    wizard::fill(&PropSteps::new(task, doc, evaluated), picks, moved_on)
}

pub fn filled(active: &ActiveTool, picks: &[Pick], doc: &PropDoc, evaluated: &Evaluated) -> Option<Filled> {
    Some(fill(active.task?, picks, &active.moved_on, doc, evaluated))
}

/// Whether Enter or Next would close the slot under way and move on.
pub fn can_move_on(task: Task, filled: &Filled, doc: &PropDoc, evaluated: &Evaluated) -> bool {
    wizard::can_move_on(&PropSteps::new(task, doc, evaluated), filled)
}

pub fn move_on(active: &mut ActiveTool, editor: &PropEditor, evaluated: &Evaluated) {
    let Some(task) = active.task else { return };
    wizard::move_on(&PropSteps::new(task, editor.doc(), evaluated), editor.picks(), &mut active.moved_on);
}

pub fn is_ready(task: Task, filled: &Filled, doc: &PropDoc, evaluated: &Evaluated) -> bool {
    wizard::is_ready(&PropSteps::new(task, doc, evaluated), filled)
}

/// Ready, and nothing more could be added: a last slot taking any number of
/// regions, or a construction another pick would change, waits to be told it
/// is done.
pub fn is_finished(task: Task, filled: &Filled, doc: &PropDoc, evaluated: &Evaluated) -> bool {
    wizard::is_finished(&PropSteps::new(task, doc, evaluated), filled)
}

/// The slot under way, or the open-ended last one.
pub fn prompt(task: Task, filled: &Filled, doc: &PropDoc, evaluated: &Evaluated) -> String {
    let steps = PropSteps::new(task, doc, evaluated);
    match wizard::under_way(&steps, filled) {
        Some(step) => get!(steps.slots[step].prompt),
        None => match steps.slots.last().map(|slot| slot.kind) {
            Some(slot::Kind::Regions) => get!("prop.tool.pick.more_regions"),
            Some(slot::Kind::Profiles) => get!("prop.tool.pick.more_profiles"),
            Some(slot::Kind::Bodies(_)) => get!("prop.tool.pick.more_bodies"),
            _ => get!("prop.tool.pick.more"),
        },
    }
}

/// Whether clicking `candidate` would do anything: unpick it, or fill a slot.
pub fn accepts(task: Task, picks: &[Pick], moved_on: &[Pick], candidate: &Pick, doc: &PropDoc, evaluated: &Evaluated) -> bool {
    wizard::accepts(&PropSteps::new(task, doc, evaluated), picks, moved_on, candidate)
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
        // A list starts from what the field names, so a click adds to it and
        // Escape takes one back; anything else is picked fresh.
        Task::Repick { feature, field } => {
            let preloaded = repicked(editor.doc(), feature, field)
                .filter(|field| matches!(field.slot.kind, slot::Kind::Bodies(_) | slot::Kind::Path))
                .map(|field| field.refs.iter().flat_map(picks_of).collect())
                .unwrap_or_default();
            editor.set_picks(preloaded);
            active.set(Some(task));
        }
        Task::Make(_) => {
            let (filled, moved_on) = wizard::seed(&PropSteps::new(task, editor.doc(), evaluated), editor.picks());
            let kept: Vec<Pick> = editor.picks().iter().filter(|pick| filled.picks().any(|held| held == *pick)).cloned().collect();
            editor.set_picks(kept);
            if is_ready(task, &filled, editor.doc(), evaluated) {
                active.set(None);
                finish(task, &filled, editor, evaluated, session);
                return;
            }
            active.set(Some(task));
            active.moved_on = moved_on;
        }
    }
}

/// The features as they would stand if Enter were pressed now, and the id of
/// the feature it would add, while every slot is answered and the last still
/// takes more. Built on a copy, as [`finish`] builds on the document.
pub fn would_make(active: &ActiveTool, editor: &PropEditor, evaluated: &Evaluated) -> Option<(Vec<PropFeature>, PropFeatureId)> {
    let Some(task @ Task::Make(tool)) = active.task else { return None };
    let doc = editor.doc();
    let filled = fill(task, editor.picks(), &active.moved_on, doc, evaluated);
    if !is_ready(task, &filled, doc, evaluated) || is_finished(task, &filled, doc, evaluated) {
        return None;
    }
    let values: Vec<Value> =
        task.slots(doc).iter().zip(&filled.has).map(|(slot, has)| slot.kind.value(has, evaluated)).collect::<Option<_>>()?;
    let mut draft = doc.clone();
    draft.features.truncate(editor.shown().len());
    let planes: Vec<PropFeatureId> = values.iter().flat_map(Value::faces).map(|face| draft.push(plane_through_face(face))).collect();
    let mut planes = planes.into_iter();
    let resolved = values.into_iter().map(|value| value.resolve(&mut planes, &mut draft)).collect();
    // A datum makes no body, so there would be nothing to draw.
    let op = (tool.spec().build)(resolved).filter(FeatureOp::makes_body)?;
    let id = draft.push(op);
    Some((draft.features, id))
}

/// The picks that name `reference`.
fn picks_of(reference: &AnyRef) -> Vec<Pick> {
    match reference {
        AnyRef::Path(path) => path.curves.iter().map(Pick::of_curve).collect(),
        other => Pick::of_any(other).into_iter().collect(),
    }
}

/// What the view stops at while a re-pick is up: the feature before the one
/// re-picked, so the bodies it eats stand and can be clicked.
pub fn peeked(active: &ActiveTool, doc: &PropDoc) -> Option<PropFeatureId> {
    let Some(Task::Repick { feature, .. }) = active.task else { return None };
    let at = doc.index_of(feature)?;
    at.checked_sub(1).map(|before| doc.features[before].id)
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
    let Some(task) = active.task else { return };
    if let Task::Repick { feature, .. } = task
        && editor.doc().feature(feature).is_none()
    {
        active.set(None);
        return;
    }
    // A marker whose pick was clicked off would close a slot if it came back.
    active.moved_on.retain(|marker| editor.picks().contains(marker));
    let filled = fill(task, editor.picks(), &active.moved_on, editor.doc(), evaluated);
    if confirm && can_move_on(task, &filled, editor.doc(), evaluated) {
        move_on(active, editor, evaluated);
        return;
    }
    let done = match confirm {
        true => is_ready(task, &filled, editor.doc(), evaluated),
        false => is_finished(task, &filled, editor.doc(), evaluated),
    };
    if done {
        active.set(None);
        finish(task, &filled, editor, evaluated, session);
    }
}

/// Puts the tool down, leaving the selection as it is.
pub fn put_down(active: &mut ActiveTool) {
    active.set(None);
}

/// Escape: takes back moving on or the last pick, and with nothing picked
/// puts the tool down as [`cancel`] does.
pub fn back(active: &mut ActiveTool, editor: &mut PropEditor, evaluated: &Evaluated) {
    let Some(filled) = filled(active, editor.picks(), editor.doc(), evaluated) else { return };
    // Picks no slot holds (a feature-list click mid-tool) go first, so the
    // press always takes back something the tool was using.
    let mut picks: Vec<Pick> = editor.picks().iter().filter(|pick| filled.picks().any(|held| held == *pick)).cloned().collect();
    match wizard::back(&mut picks, &mut active.moved_on) {
        wizard::Back::Took(_) | wizard::Back::Reopened => editor.set_picks(picks),
        wizard::Back::Exit => cancel(active, editor),
    }
}

/// Puts the tool down, restoring the selection from before a re-pick. A
/// sweep left joining or cutting nothing, its targets never picked, goes back
/// to making a new body.
pub fn cancel(active: &mut ActiveTool, editor: &mut PropEditor) {
    let task = active.task;
    active.set(None);
    let Some(Task::Repick { feature, .. }) = task else { return };
    let Some(standing) = editor.doc().feature(feature) else { return };
    if standing.op.sweep_with_no_targets() {
        editor.edit(|doc| {
            if let Some(op) = doc.feature_mut(feature).and_then(|f| f.op.sweep_op_mut()) {
                *op = SweepOp::NewBody;
            }
        });
    }
    editor.select(feature);
}

/// After choosing a sweep's operation: a new body forgets its targets, and
/// `true` means it now needs targets picked.
pub fn chose_operation(op: SweepOp, targets: &mut Vec<BodyRef>) -> bool {
    match op {
        SweepOp::NewBody => {
            targets.clear();
            false
        }
        _ => targets.is_empty(),
    }
}

/// Whether `task` has a path slot with a curve in it to carry on from.
pub fn can_chain(task: Task, filled: &Filled, doc: &PropDoc) -> bool {
    task.slots(doc).iter().zip(&filled.has).any(|(slot, has)| slot.kind == slot::Kind::Path && !has.is_empty())
}

/// Carries the path slot's curves on, at both ends, along every curve that
/// joins without a corner: offered seams and edges, and sketch lines and
/// curves. What it finds is picked, so the path is stored as the plain list
/// of curves and no rule is kept to chain differently later.
pub fn chain(task: Task, moved_on: &[Pick], editor: &mut PropEditor, evaluated: &Evaluated) {
    let slots = task.slots(editor.doc());
    let filled = fill(task, editor.picks(), moved_on, editor.doc(), evaluated);
    let Some(has) = slots.iter().zip(&filled.has).find(|(slot, _)| slot.kind == slot::Kind::Path).map(|(_, has)| has.clone())
    else {
        return;
    };
    let scope = evaluated.scope();
    let mut candidates: Vec<(Pick, Curve3)> = Vec::new();
    let mut offer = |pick: Pick| {
        if let Some(curve) = pick.as_curve().and_then(|curve| curve.resolve(&scope)) {
            candidates.push((pick, curve));
        }
    };
    for seam in evaluated.seams().iter().filter(|seam| seam.offered) {
        offer(Pick::Seam(seam.name.clone()));
    }
    for pickable in &evaluated.pickables {
        if matches!(pickable.at, crate::prop::reference::Located::Edge(..)) && !evaluated.replaced(&pickable.reference) {
            offer(Pick::Geom(pickable.reference.clone()));
        }
    }
    for placed in &evaluated.sketches {
        for (entity, _, construction) in placed.sketch.curves() {
            if !construction {
                offer(Pick::SketchCurve { sketch: placed.id, entity });
            }
        }
    }
    let Some(curves) = slot::chained(&has, evaluated) else { return };
    let resolved: Option<Vec<Curve3>> = PathRef { curves }.resolve(&scope);
    let Some(resolved) = resolved else { return };
    let (mut start, mut end) = (resolved[0].clone(), resolved[resolved.len() - 1].clone());
    let mut taken: Vec<Pick> = has.clone();
    loop {
        let found = candidates.iter().find_map(|(pick, curve)| {
            if taken.contains(pick) {
                return None;
            }
            let continues = |at: Vec3, along: Vec3, onto: &Curve3| {
                [onto.clone(), onto.clone().reversed()]
                    .into_iter()
                    .find(|oriented| oriented.start().distance(at) < JOINS && starting_direction(oriented).dot(along) > SMOOTH)
            };
            if let Some(oriented) = continues(end.end(), ending_direction(&end), curve) {
                return Some((pick.clone(), oriented, true));
            }
            let backward = start.clone().reversed();
            continues(backward.end(), ending_direction(&backward), curve).map(|oriented| (pick.clone(), oriented.reversed(), false))
        });
        let Some((pick, oriented, at_end)) = found else { break };
        match at_end {
            true => end = oriented,
            false => start = oriented,
        }
        taken.push(pick.clone());
        editor.toggle(pick);
        if start.start().distance(end.end()) < JOINS {
            break;
        }
    }
}

/// The cosine of the sharpest turn between two curves that still chains.
const SMOOTH: f32 = 0.94;

fn starting_direction(curve: &Curve3) -> Vec3 {
    let segments = curve.segments();
    segments.first().map_or(Vec3::ZERO, |(a, b)| (*b - *a).normalize_or_zero())
}

fn ending_direction(curve: &Curve3) -> Vec3 {
    let segments = curve.segments();
    segments.last().map_or(Vec3::ZERO, |(a, b)| (*b - *a).normalize_or_zero())
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
        slots.iter().zip(&filled.has).map(|(slot, has)| slot.kind.value(has, evaluated)).collect()
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
    use crate::prop::feature::BodyRef;
    use crate::prop::feature::{evaluate, Axis};
    use crate::prop::plane::OriginPlane;
    use crate::prop::profile::Placement;
    use crate::prop::reference::Source;
    use crate::prop::plane::{PlaneDef, PlaneRef};
    use crate::prop::reference::{Element, GeomRef};
    use crate::prop::refs::{CurveRef, DirectionRef};
    use crate::prop::sketch::Sketch;
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
        let picks = [Pick::Axis(Axis::X), Pick::Body(BodyRef::whole(PropFeatureId(1))), Pick::Body(BodyRef::whole(PropFeatureId(0)))];
        let filled = fill(task, &picks, &[], &doc, &evaluated);
        let bodies = vec![Pick::Body(BodyRef::whole(PropFeatureId(1))), Pick::Body(BodyRef::whole(PropFeatureId(0)))];
        assert_eq!(filled.has[0], bodies, "both bodies are targets, in pick order");
        assert!(filled.has[1].is_empty(), "an axis is not a plane");
        assert_eq!(prompt(task, &filled, &doc, &evaluated), get!("prop.tool.pick.plane"));
        assert!(!is_ready(task, &filled, &doc, &evaluated));
    }

    #[test]
    fn a_pick_goes_to_whichever_slot_it_fits_whatever_order_it_came_in() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::RadialPattern);
        let filled = fill(task, &[Pick::Axis(Axis::Y), Pick::Body(BodyRef::whole(PropFeatureId(0)))], &[], &doc, &evaluated);
        assert!(is_finished(task, &filled, &doc, &evaluated));
    }

    #[test]
    fn combine_wants_two_different_bodies() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::Combine(BooleanOp::Union));
        let face = element(0, Element::Face(0));
        assert!(!accepts(task, &[Pick::Body(BodyRef::whole(PropFeatureId(0)))], &[], &face, &doc, &evaluated), "a face of the same body");
        assert!(accepts(task, &[Pick::Body(BodyRef::whole(PropFeatureId(0)))], &[], &Pick::Body(BodyRef::whole(PropFeatureId(1))), &doc, &evaluated));
        assert!(accepts(task, &[Pick::Body(BodyRef::whole(PropFeatureId(0)))], &[], &Pick::Body(BodyRef::whole(PropFeatureId(0))), &doc, &evaluated), "unpicking always works");
    }

    #[test]
    fn a_plane_is_a_plane_or_a_flat_face_and_lines_are_lines() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let sketch = Task::Make(Tool::Sketch);
        assert!(accepts(sketch, &[], &[], &Pick::Plane(PlaneRef::Origin(OriginPlane::Xy)), &doc, &evaluated));
        assert!(accepts(sketch, &[], &[], &element(0, Element::Face(0)), &doc, &evaluated));
        assert!(!accepts(sketch, &[], &[], &element(0, Element::Edge(0)), &doc, &evaluated));
        let pattern = Task::Make(Tool::LinearPattern);
        let picks = [Pick::Body(BodyRef::whole(PropFeatureId(0)))];
        assert!(accepts(pattern, &picks, &[], &element(1, Element::Edge(0)), &doc, &evaluated));
        assert!(accepts(pattern, &picks, &[], &Pick::Axis(Axis::Z), &doc, &evaluated));
        assert!(!accepts(pattern, &picks, &[], &element(1, Element::Point(0)), &doc, &evaluated));
    }

    #[test]
    fn a_plane_tool_finishes_the_moment_its_picks_make_a_plane() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::Plane);
        let corners: Vec<Pick> = (0..3).map(|i| element(0, Element::Point(i))).collect();
        assert!(!is_ready(task, &fill(task, &corners[..2], &[], &doc, &evaluated), &doc, &evaluated));
        assert!(is_finished(task, &fill(task, &corners, &[], &doc, &evaluated), &doc, &evaluated));
    }

    #[test]
    fn a_tool_with_everything_picked_makes_its_feature_at_once() {
        let mut editor = PropEditor::default();
        editor.open(two_cubes(), None);
        let evaluated = editor.doc().evaluate();
        editor.set_picks(vec![Pick::Body(BodyRef::whole(PropFeatureId(0))), Pick::Plane(PlaneRef::Origin(OriginPlane::Yz))]);
        let mut active = ActiveTool::default();
        start(Task::Make(Tool::Mirror), &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        assert_eq!(active.task, None);
        assert!(matches!(
            editor.doc().features.last().map(|f| &f.op),
            Some(FeatureOp::Mirror { targets, about: PlaneRef::Origin(OriginPlane::Yz), .. }) if *targets == vec![BodyRef::whole(PropFeatureId(0))]
        ));
    }

    #[test]
    fn mirroring_about_a_face_makes_the_plane_too_in_one_step() {
        let mut editor = PropEditor::default();
        editor.open(two_cubes(), None);
        let evaluated = editor.doc().evaluate();
        let mut active = ActiveTool::default();
        editor.set_picks(vec![Pick::Body(BodyRef::whole(PropFeatureId(1)))]);
        start(Task::Make(Tool::Mirror), &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        assert_eq!(active.task, Some(Task::Make(Tool::Mirror)), "still wants a plane");
        // A face would join the bodies as cube 0; Enter moves on to the plane.
        advance(&mut active, &mut editor, &evaluated, &mut SketchSession::default(), true);
        editor.toggle(element(0, Element::Face(0)));
        advance(&mut active, &mut editor, &evaluated, &mut SketchSession::default(), false);
        assert_eq!(active.task, None);
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
        assert_eq!(active.task, None);
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
        let edge = fill(task, &[element(0, Element::Edge(0))], &[], &doc, &evaluated);
        assert!(is_ready(task, &edge, &doc, &evaluated), "its middle, if Done");
        assert!(!is_finished(task, &edge, &doc, &evaluated));
        let with_plane = fill(
            task,
            &[element(0, Element::Edge(8)), Pick::Plane(PlaneRef::Origin(OriginPlane::Xy))],
            &[],
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
        assert_eq!(active.task, None);
        let ops: Vec<&FeatureOp> = editor.doc().features.iter().map(|f| &f.op).collect();
        assert_eq!(ops.len(), 5);
        assert!(matches!(ops[4], FeatureOp::Plane { def: PlaneDef::Midplane { .. } }));
        assert!(editor.doc().evaluate().problems.is_empty());
        editor.undo();
        assert_eq!(editor.doc().features.len(), 2, "one undo takes all three");
    }

    /// Fails if a picked face is given a plane feature, or the tool waits for more.
    #[test]
    fn a_picked_face_is_extruded_as_it_stands() {
        let mut editor = PropEditor::default();
        editor.open(two_cubes(), None);
        let evaluated = editor.doc().evaluate();
        editor.set_picks(vec![element(0, Element::Face(0))]);
        let mut active = ActiveTool::default();
        start(Task::Make(Tool::Extrude), &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        assert_eq!(active.task, None);
        let ops: Vec<&FeatureOp> = editor.doc().features.iter().map(|f| &f.op).collect();
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[2], FeatureOp::Extrude { profile: ProfileSource::Face { .. }, .. }));
        let extruded = editor.doc().evaluate();
        assert!(extruded.problems.is_empty(), "{:?}", extruded.problems);
        assert_eq!(extruded.bodies.len(), 3);
    }

    #[test]
    fn a_face_does_not_join_regions_in_one_profile() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::Extrude);
        let face = element(0, Element::Face(0));
        assert!(accepts(task, &[], &[], &face, &doc, &evaluated));
        assert!(!accepts(task, &[face.clone()], &[], &element(1, Element::Face(0)), &doc, &evaluated));
    }

    #[test]
    fn a_row_runs_along_a_planes_normal() {
        let doc = two_cubes();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::LinearPattern);
        let filled = fill(task, &[Pick::Body(BodyRef::whole(PropFeatureId(0))), Pick::Plane(PlaneRef::Origin(OriginPlane::Xz))], &[], &doc, &evaluated);
        assert!(is_finished(task, &filled, &doc, &evaluated));
    }

    /// The check that keeps a new revolve's axis in its sketch's plane runs on
    /// a re-pick too, against the feature's own regions.
    #[test]
    fn a_repicked_axis_still_has_to_lie_in_the_sketch() {
        let mut doc = PropDoc::new("test");
        let sketch = doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xz), sketch: Sketch::default() });
        let revolve = doc.push(FeatureOp::Revolve { op: SweepOp::NewBody, targets: vec![],
            profile: ProfileSource::Regions { sketch: sketch, regions: vec![] },
            degrees: 360.0,
            steps: 12,
            symmetric: false,
            axis: CurveRef::Axis { axis: Axis::X },
            surface: Surface::default(),
        });
        let evaluated = evaluate(&doc.features);
        let task = Task::Repick { feature: revolve, field: 1 };
        assert_eq!(task.name(&doc), get!("prop.inspector.axis"));
        assert!(accepts(task, &[], &[], &Pick::Axis(Axis::Z), &doc, &evaluated));
        assert!(!accepts(task, &[], &[], &Pick::Axis(Axis::Y), &doc, &evaluated), "Y stands off the XZ plane");
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
        assert_eq!(active.task, None);
        assert_eq!(
            editor.doc().feature(plane).map(|f| &f.op),
            Some(&FeatureOp::Plane { def: PlaneDef::Offset { distance: 0.3, from: PlaneRef::Origin(OriginPlane::Xz) } })
        );
    }

    /// Three sketch pieces end to end: a line, a quarter arc tangent to it,
    /// and a line square to the arc's end; and a fourth apart.
    fn bent_path() -> PropDoc {
        let mut sketch = Sketch::default();
        // The arc ends heading +Y; the last line heads +X from there.
        let p: Vec<_> = [[0.0, 0.0], [0.1, 0.0], [0.1, 0.1], [0.2, 0.1], [0.3, 0.1], [0.5, 0.5], [0.6, 0.5]]
            .iter()
            .map(|at| sketch.add_point(Vec2::from_array(*at)))
            .collect();
        sketch.add_line(p[0], p[1]);
        sketch.add_arc(p[2], p[1], p[3], false);
        sketch.add_line(p[3], p[4]);
        sketch.add_line(p[5], p[6]);
        let mut doc = PropDoc::new("test");
        doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch });
        doc
    }

    fn piece(entity: u32) -> Pick {
        Pick::SketchCurve { sketch: PropFeatureId(0), entity: crate::prop::sketch::SketchId(entity) }
    }

    #[test]
    fn a_path_takes_a_curve_joining_either_end() {
        let doc = bent_path();
        let evaluated = evaluate(&doc.features);
        let task = Task::Make(Tool::Sweep);
        // Entities are numbered after the seven points.
        let (line, arc, last, apart) = (piece(7), piece(8), piece(9), piece(10));
        assert!(accepts(task, &[arc.clone()], &[], &line, &doc, &evaluated), "joins the arc's start");
        assert!(accepts(task, &[arc.clone()], &[], &last, &doc, &evaluated), "joins the arc's end");
        assert!(!accepts(task, &[arc], &[], &apart, &doc, &evaluated));
        assert!(!accepts(task, &[line], &[], &last, &doc, &evaluated), "touches neither end");
    }

    #[test]
    fn chain_follows_curves_that_carry_straight_on() {
        let mut editor = PropEditor::default();
        editor.open(bent_path(), None);
        let evaluated = editor.doc().evaluate();
        editor.set_picks(vec![piece(7)]);
        chain(Task::Make(Tool::Sweep), &[], &mut editor, &evaluated);
        // The arc carries on from the line; the last line turns a corner.
        assert_eq!(editor.picks(), &[piece(7), piece(8)]);
    }

    #[test]
    fn a_repick_only_offers_what_stands_before_the_feature() {
        let mut doc = two_cubes();
        let pattern = doc.insert_at(1, FeatureOp::Pattern {
            targets: vec![BodyRef::whole(PropFeatureId(0))],
            count: 2,
            along: PatternKind::Linear { spacing: 0.2, direction: DirectionRef::Along { curve: CurveRef::Axis { axis: Axis::X } } },
        });
        let evaluated = evaluate(&doc.features);
        let task = Task::Repick { feature: pattern, field: 1 };
        assert!(accepts(task, &[], &[], &Pick::Axis(Axis::Y), &doc, &evaluated));
        assert!(!accepts(task, &[], &[], &element(1, Element::Edge(0)), &doc, &evaluated), "cube 1 comes after");
    }

    #[test]
    fn escape_takes_seeded_picks_back_one_at_a_time_then_puts_the_tool_down() {
        let mut editor = PropEditor::default();
        editor.open(two_cubes(), None);
        let evaluated = editor.doc().evaluate();
        let corners: Vec<Pick> = (0..2).map(|i| element(0, Element::Point(i))).collect();
        editor.set_picks(corners.clone());
        let mut active = ActiveTool::default();
        start(Task::Make(Tool::Plane), &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        assert_eq!(active.task, Some(Task::Make(Tool::Plane)), "two corners make no plane yet");
        back(&mut active, &mut editor, &evaluated);
        assert_eq!(editor.picks(), &corners[..1]);
        editor.toggle(Pick::Body(BodyRef::whole(PropFeatureId(1))));
        back(&mut active, &mut editor, &evaluated);
        assert!(editor.picks().is_empty(), "a body no slot holds goes with the corner");
        assert!(active.task.is_some(), "the tool stays up with nothing picked");
        back(&mut active, &mut editor, &evaluated);
        assert_eq!(active.task, None);
    }

    #[test]
    fn escape_out_of_a_repick_selects_the_feature_again() {
        let mut editor = PropEditor::default();
        let mut doc = two_cubes();
        let pattern = doc.insert_at(1, FeatureOp::Pattern {
            targets: vec![BodyRef::whole(PropFeatureId(0))],
            count: 2,
            along: PatternKind::Linear { spacing: 0.2, direction: DirectionRef::Along { curve: CurveRef::Axis { axis: Axis::X } } },
        });
        editor.open(doc, None);
        let evaluated = editor.doc().evaluate();
        editor.select(pattern);
        let mut active = ActiveTool::default();
        let task = Task::Repick { feature: pattern, field: 1 };
        assert_eq!(task.slots(editor.doc()).len(), 1, "the direction is a real field");
        start(task, &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        assert!(editor.picks().is_empty());
        back(&mut active, &mut editor, &evaluated);
        assert_eq!(active.task, None);
        assert_eq!(editor.picks(), &[Pick::Body(BodyRef::whole(pattern))]);
    }

    /// Two squares apart on the XZ plane, and a pick of each region.
    fn two_squares() -> (PropDoc, Vec<Pick>) {
        let mut drawn = Sketch::default();
        for x in [0.1, 0.3] {
            let points: Vec<_> = [[x, 0.0], [x + 0.1, 0.0], [x + 0.1, 0.1], [x, 0.1]]
                .iter()
                .map(|corner| drawn.add_point(Vec2::from_array(*corner)))
                .collect();
            for i in 0..4 {
                drawn.add_line(points[i], points[(i + 1) % 4]);
            }
        }
        let regions = crate::prop::region::arrange(&drawn).regions;
        let mut doc = PropDoc::new("test");
        let sketch = doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xz), sketch: drawn });
        let picks = regions.into_iter().map(|region| Pick::Region { sketch, region: region.name }).collect();
        (doc, picks)
    }

    #[test]
    fn next_closes_the_regions_so_only_the_axis_is_left() {
        let (doc, regions) = two_squares();
        assert_eq!(regions.len(), 2);
        let mut editor = PropEditor::default();
        editor.open(doc, None);
        let evaluated = editor.doc().evaluate();
        let mut active = ActiveTool::default();
        start(Task::Make(Tool::Revolve), &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        editor.toggle(regions[0].clone());
        let task = Task::Make(Tool::Revolve);
        let filled = fill(task, editor.picks(), &active.moved_on, editor.doc(), &evaluated);
        assert!(can_move_on(task, &filled, editor.doc(), &evaluated));
        advance(&mut active, &mut editor, &evaluated, &mut SketchSession::default(), true);
        assert_eq!(active.moved_on, vec![regions[0].clone()], "Enter moved on rather than finishing");
        assert!(!accepts(task, editor.picks(), &active.moved_on, &regions[1], editor.doc(), &evaluated));
        assert!(accepts(task, editor.picks(), &active.moved_on, &Pick::Axis(Axis::Z), editor.doc(), &evaluated));

        back(&mut active, &mut editor, &evaluated);
        assert_eq!(editor.picks(), &regions[..1], "Escape takes back moving on, not the region");
        assert!(accepts(task, editor.picks(), &active.moved_on, &regions[1], editor.doc(), &evaluated));
    }

    #[test]
    fn a_seeded_list_stops_where_a_later_slot_wants_the_same_pick() {
        let (doc, regions) = two_squares();
        let evaluated = evaluate(&doc.features);
        let steps = PropSteps::new(Task::Make(Tool::Revolve), &doc, &evaluated);
        let (filled, moved_on) = wizard::seed(&steps, &regions);
        assert_eq!(filled.has[0], regions, "the axis slot takes no region, so both stay in the first");
        assert!(moved_on.is_empty());
    }

    #[test]
    fn every_tool_fills_the_frame_after_it_starts_as_it_was_seeded() {
        let selection = vec![
            Pick::Body(BodyRef::whole(PropFeatureId(0))),
            Pick::Body(BodyRef::whole(PropFeatureId(1))),
            element(0, Element::Face(0)),
            element(1, Element::Edge(0)),
            Pick::Axis(Axis::X),
            Pick::Plane(PlaneRef::Origin(OriginPlane::Xy)),
        ];
        for spec in TOOLS {
            let task = Task::Make(spec.tool);
            let mut editor = PropEditor::default();
            editor.open(two_cubes(), None);
            let evaluated = editor.doc().evaluate();
            let (seeded, _) = wizard::seed(&PropSteps::new(task, editor.doc(), &evaluated), &selection);
            editor.set_picks(selection.clone());
            let mut active = ActiveTool::default();
            start(task, &mut active, &mut editor, &evaluated, &mut SketchSession::default());
            if active.task.is_none() {
                continue;
            }
            let next_frame = fill(task, editor.picks(), &active.moved_on, editor.doc(), &evaluated);
            assert_eq!(next_frame.has, seeded.has, "{:?} lost its seeded split", spec.tool);
        }
    }

    #[test]
    fn subtract_from_three_selected_bodies_cuts_the_last_two_from_the_first() {
        let mut doc = two_cubes();
        doc.push(cube(Vec3::X * 0.05));
        let mut editor = PropEditor::default();
        editor.open(doc, None);
        let evaluated = editor.doc().evaluate();
        let [a, b, c] = [0, 1, 2].map(|id| Pick::Body(BodyRef::whole(PropFeatureId(id))));
        editor.set_picks(vec![a, b, c]);
        let mut active = ActiveTool::default();
        start(Task::Make(Tool::Combine(BooleanOp::Subtract)), &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        assert_eq!(active.task, None, "every slot was answered");
        let whole = |id| BodyRef::whole(PropFeatureId(id));
        assert_eq!(
            editor.doc().features.last().map(|f| &f.op),
            Some(&FeatureOp::Subtract { targets: vec![whole(0)], tools: vec![whole(1), whole(2)] })
        );
    }

    #[test]
    fn repicking_a_list_starts_from_it_and_shows_the_prop_before_the_feature() {
        let mut doc = two_cubes();
        doc.push(cube(Vec3::X * 0.05));
        let whole = |id| BodyRef::whole(PropFeatureId(id));
        let cut = doc.push(FeatureOp::Subtract { targets: vec![whole(0)], tools: vec![whole(1)] });
        let mut editor = PropEditor::default();
        editor.open(doc, None);
        editor.select(cut);
        let mut active = ActiveTool::default();
        let task = Task::Repick { feature: cut, field: 1 };
        let evaluated = editor.doc().evaluate();
        start(task, &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        assert_eq!(editor.picks(), &[Pick::Body(whole(1))], "the tools it names now");
        assert_eq!(peeked(&active, editor.doc()), Some(PropFeatureId(2)), "the cube before the cut");
        editor.peek_at(peeked(&active, editor.doc()));
        let before = crate::prop::feature::evaluate(editor.shown());
        assert_eq!(before.bodies.len(), 3, "every cube stands, the eaten two included");

        editor.toggle(Pick::Body(whole(2)));
        advance(&mut active, &mut editor, &before, &mut SketchSession::default(), false);
        assert_eq!(active.task, Some(task), "a list waits for Enter");
        advance(&mut active, &mut editor, &before, &mut SketchSession::default(), true);
        assert_eq!(active.task, None);
        let tools = |editor: &PropEditor| match &editor.doc().feature(cut).unwrap().op {
            FeatureOp::Subtract { tools, .. } => tools.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(tools(&editor), vec![whole(1), whole(2)]);
        editor.undo();
        assert_eq!(tools(&editor), vec![whole(1)], "one undo step");
    }

    #[test]
    fn escaping_a_preloaded_repick_takes_its_picks_back_then_keeps_the_field() {
        let mut doc = two_cubes();
        let whole = |id| BodyRef::whole(PropFeatureId(id));
        let union = doc.push(FeatureOp::Union { bodies: vec![whole(0), whole(1)] });
        let mut editor = PropEditor::default();
        editor.open(doc.clone(), None);
        let mut active = ActiveTool::default();
        let evaluated = editor.doc().evaluate();
        start(Task::Repick { feature: union, field: 0 }, &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        for _ in 0..3 {
            back(&mut active, &mut editor, &crate::prop::feature::evaluate(&doc.features[..2]));
        }
        assert_eq!(active.task, None);
        assert_eq!(editor.doc(), &doc, "the field is as it was");
        assert_eq!(editor.picks(), &[Pick::Body(whole(union.0))]);
    }

    #[test]
    fn a_sweep_operation_wants_targets_only_while_it_has_none() {
        let mut targets = vec![];
        assert!(chose_operation(SweepOp::Cut, &mut targets));
        targets.push(BodyRef::whole(PropFeatureId(0)));
        assert!(!chose_operation(SweepOp::Join, &mut targets));
        assert!(!chose_operation(SweepOp::NewBody, &mut targets));
        assert!(targets.is_empty(), "a new body forgets its targets");
    }

    #[test]
    fn escaping_a_targets_pick_puts_the_sweep_back_to_a_new_body() {
        let (mut doc, _) = two_squares();
        let sketch = doc.features[0].id;
        let extrude = doc.push(FeatureOp::Extrude {
            profile: ProfileSource::Regions { sketch, regions: vec![] },
            taper: 0.0,
            op: SweepOp::Cut,
            targets: vec![],
            end: End::Distance { distance: 0.1 },
            other: None,
            direction: None,
            surface: Surface::default(),
        });
        let mut editor = PropEditor::default();
        editor.open(doc, None);
        let mut active = ActiveTool::default();
        let evaluated = editor.doc().evaluate();
        start(Task::Repick { feature: extrude, field: 4 }, &mut active, &mut editor, &evaluated, &mut SketchSession::default());
        back(&mut active, &mut editor, &evaluated);
        assert_eq!(active.task, None);
        assert!(matches!(editor.doc().feature(extrude).unwrap().op, FeatureOp::Extrude { op: SweepOp::NewBody, .. }));
    }

    #[test]
    fn a_tool_waiting_on_enter_shows_what_it_would_make() {
        let mut doc = two_cubes();
        doc.push(cube(Vec3::X * 0.05));
        let mut editor = PropEditor::default();
        editor.open(doc, None);
        let evaluated = editor.doc().evaluate();
        let active = ActiveTool { task: Some(Task::Make(Tool::Combine(BooleanOp::Union))), moved_on: vec![] };
        let body = |id| Pick::Body(BodyRef::whole(PropFeatureId(id)));
        editor.set_picks(vec![body(0)]);
        assert!(would_make(&active, &editor, &evaluated).is_none(), "one body is not a union");
        editor.set_picks(vec![body(0), body(2)]);
        let (features, made) = would_make(&active, &editor, &evaluated).expect("two bodies are");
        let previewed = crate::prop::feature::evaluate(&features);
        let joined: Vec<f32> = previewed.bodies.iter().filter(|(b, _)| b.feature == made).map(|(_, solid)| solid.volume()).collect();
        assert_eq!(joined.len(), 1);
        assert!((joined[0] - 0.15 * 0.1 * 0.1).abs() < 1e-7, "{joined:?}");
        assert_eq!(editor.doc().features.len(), 3, "the document is untouched");
    }
}
