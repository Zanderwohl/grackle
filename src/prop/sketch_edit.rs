//! Editing a sketch: the third workspace, beside Model and Fit. See
//! `documentation/sketches.md`.
//!
//! [`SketchSession::editing`] is the one answer to "is a sketch being edited";
//! the panels, the model's picking and handles, and the systems here all ask
//! it. While it is set, a left click belongs to the sketch: the model is shown
//! for snapping to and cannot be picked.
//!
//! Every edit goes through [`PropEditor`], so the prop's undo covers the
//! sketch: a placed point or a drawn line is one [`PropEditor::edit`], and a
//! dragged point one gesture.

use crate::common::keybind::{Action, Actions};
use crate::get;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::AppMode;
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::{CameraAxis, Multicam};
use crate::prop::camera::OrbitPivot;
use crate::prop::document::{PropDoc, PropEditor};
use crate::prop::feature::{FeatureOp, PropFeatureId};
use crate::prop::gizmo::HoveredPick;
use crate::prop::reference::{Located, Pickable, Pointer, EDGE_PICK_PX, POINT_PICK_PX};
use crate::prop::region::{arrange, crossing, Arrangement};
use crate::prop::curve::{clamped_knots, Curve2};
use crate::prop::sketch::{PlacedSketch, Sketch, SketchId};
use crate::prop::view::PropBuild;
use crate::tool::tool_helpers::dashed_line;

/// Meters between grid lines, and what an unsnapped point rounds to.
pub const GRID: f32 = 0.005;

/// Meters the grid is drawn behind the plane, away from the viewer.
const GRID_BEHIND: f32 = 0.0001;

/// Pixels within which the cursor lands on a point rather than the grid.
const SNAP_PX: f32 = 10.0;

/// Pixels a press must travel before it is a drag rather than a click.
const DRAG_PX: f32 = 3.0;

/// Grid lines each way past which the grid is not drawn: a sketch the size of
/// a cannon at five millimeters would be thousands of lines.
const MAX_GRID_LINES: i32 = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SketchTool {
    #[default]
    Select,
    Line,
    /// Center, start, end; from start the way the pointer went round, up to
    /// a whole turn.
    Arc,
    /// Center, then any point on the rim.
    Circle,
    /// Control points, finished by clicking the last again, `Enter` or Done.
    Bezier,
    /// As [`SketchTool::Bezier`], cubic where it has the points for it.
    Nurbs,
    /// Takes away the piece of a line between the junctions either side of a
    /// click, or every piece a drag passes through; a curve goes whole.
    Erase,
}

impl SketchTool {
    /// Whether it collects points until told it is done.
    pub fn open_ended(self) -> bool {
        matches!(self, SketchTool::Bezier | SketchTool::Nurbs)
    }
}

/// `Line` is any entity: a line or a curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SketchPick {
    Point(SketchId),
    Line(SketchId),
}

impl SketchPick {
    pub fn id(self) -> SketchId {
        match self {
            SketchPick::Point(id) | SketchPick::Line(id) => id,
        }
    }
}

/// Where a click would land: a position in the sketch's plane, and the
/// existing point it is on, if any.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapped {
    pub at: Vec2,
    pub point: Option<SketchId>,
}

#[derive(Clone, Copy, Debug)]
struct Press {
    on: Option<SketchPick>,
    /// Window pixels, to tell a click from a drag.
    from: Vec2,
    dragging: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ViewChange {
    #[default]
    None,
    Aim,
    Restore,
}

#[derive(Resource, Default)]
pub struct SketchSession {
    editing: Option<PropFeatureId>,
    tool: SketchTool,
    /// The point the line tool draws from next.
    chain: Option<SketchId>,
    /// The points a curve tool has been given so far.
    pending: Vec<SketchId>,
    /// Radians the arc tool's pointer has gone round its center since the
    /// start was placed, positive counter-clockwise, within a whole turn
    /// either way; and the direction it was last seen in.
    arc_sweep: f32,
    arc_seen: Option<Vec2>,
    /// The pointer was off the plane or over the center since `arc_seen`, so
    /// how it went round in between is not known.
    arc_lost: bool,
    /// Oldest first, like the model's picks.
    pub picks: Vec<SketchPick>,
    hovered: Option<SketchPick>,
    cursor: Option<Snapped>,
    /// Where the pointer is on the plane, unsnapped: an eraser takes the
    /// piece the pointer is over, not the one nearest a grid point.
    on_plane: Option<Vec2>,
    press: Option<Press>,
    /// The eraser's path since the press, in the plane.
    stroke: Vec<Vec2>,
    /// The pieces the stroke has crossed so far, and how many of its steps
    /// have been looked at: the drawing does not change during a stroke, so
    /// only the newest step needs testing.
    doomed: Vec<(Vec2, Vec2)>,
    stroke_scanned: usize,
    /// Cached for the panels, which ask every frame.
    regions: Option<(Sketch, Arrangement)>,
    view: ViewChange,
    /// The perspective camera and pivot as they were before the view was
    /// turned onto the sketch.
    saved: Option<(Entity, Transform, Projection, Vec3)>,
    turn: Option<Turn>,
}

/// The perspective view on its way to facing the sketch.
#[derive(Clone, Copy, Debug)]
struct Turn {
    camera: Entity,
    from: Viewpoint,
    to: Viewpoint,
    elapsed: f32,
    seconds: f32,
}

/// A camera as the pivot it orbits, its rotation, and where it stands in its
/// own frame from the pivot. Blending these keeps the pivot on screen through
/// a turn, where blending positions would cut the corner.
#[derive(Clone, Copy, Debug)]
struct Viewpoint {
    pivot: Vec3,
    rotation: Quat,
    offset: Vec3,
}

impl Viewpoint {
    fn of(transform: &Transform, pivot: Vec3) -> Viewpoint {
        Viewpoint { pivot, rotation: transform.rotation, offset: transform.rotation.inverse() * (transform.translation - pivot) }
    }

    /// `t` from 0 to 1, already eased.
    fn toward(self, to: Viewpoint, t: f32) -> (Transform, Vec3) {
        let pivot = self.pivot.lerp(to.pivot, t);
        let rotation = self.rotation.slerp(to.rotation, t);
        let offset = self.offset.lerp(to.offset, t);
        (Transform::from_translation(pivot + rotation * offset).with_rotation(rotation), pivot)
    }
}

/// Radians a turn onto the sketch covers per second, on average.
const TURN_RATE: f32 = std::f32::consts::FRAC_PI_2;

/// Constant acceleration to halfway and constant deceleration after: a
/// second-order polynomial each half.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match t < 0.5 {
        true => 2.0 * t * t,
        false => 1.0 - 2.0 * (1.0 - t) * (1.0 - t),
    }
}

impl SketchSession {
    pub fn editing(&self) -> Option<PropFeatureId> {
        self.editing
    }

    pub fn arrangement(&mut self, sketch: &Sketch) -> &Arrangement {
        if self.regions.as_ref().is_none_or(|(from, _)| from != sketch) {
            self.regions = Some((sketch.clone(), arrange(sketch)));
        }
        &self.regions.as_ref().expect("just filled").1
    }

    pub fn tool(&self) -> SketchTool {
        self.tool
    }

    /// From another sketch too, keeping the view to go back to from before the
    /// first.
    pub fn enter(&mut self, sketch: PropFeatureId) {
        if self.editing == Some(sketch) {
            return;
        }
        *self = SketchSession { editing: Some(sketch), view: ViewChange::Aim, saved: self.saved.take(), ..default() };
    }

    pub fn leave(&mut self) {
        if self.editing.is_some() {
            *self = SketchSession { view: ViewChange::Restore, saved: self.saved.take(), ..default() };
        }
    }

    /// Ends a chain in progress: a line from it to a point picked with a
    /// different tool would be a surprise.
    pub fn set_tool(&mut self, tool: SketchTool) {
        self.tool = tool;
        self.chain = None;
        self.pending.clear();
    }

    /// Whether an open-ended curve tool has enough points to be finished.
    pub fn can_finish_curve(&self) -> bool {
        self.tool.open_ended() && self.pending.len() >= 2
    }

    /// Makes the curve the pending points describe, as one undo step.
    pub fn finish_curve(&mut self, editor: &mut PropEditor) {
        let (Some(id), true) = (self.editing, self.can_finish_curve()) else { return };
        let points = std::mem::take(&mut self.pending);
        let tool = self.tool;
        editor.edit(|doc| {
            let Some(sketch) = sketch_of_mut(doc, id) else { return };
            match tool {
                SketchTool::Bezier => sketch.add_bezier(points),
                _ => {
                    let degree = (points.len() as u32 - 1).min(3);
                    sketch.add_nurbs(points, degree)
                }
            };
        });
    }

    pub fn toggle(&mut self, pick: SketchPick) {
        match self.picks.iter().position(|picked| *picked == pick) {
            Some(at) => {
                self.picks.remove(at);
            }
            None => self.picks.push(pick),
        }
    }
}

pub fn not_sketching(session: Res<SketchSession>) -> bool {
    session.editing.is_none()
}

pub fn sketch_of(doc: &PropDoc, id: PropFeatureId) -> Option<&Sketch> {
    match &doc.feature(id)?.op {
        FeatureOp::Sketch { sketch, .. } => Some(sketch),
        _ => None,
    }
}

pub fn sketch_of_mut(doc: &mut PropDoc, id: PropFeatureId) -> Option<&mut Sketch> {
    match &mut doc.feature_mut(id)?.op {
        FeatureOp::Sketch { sketch, .. } => Some(sketch),
        _ => None,
    }
}

/// The sketch being edited, placed where it was evaluated but drawn as the
/// document has it now: the build is debounced, and a dragged point that waited
/// on it would lag the pointer.
fn live(session: &SketchSession, editor: &PropEditor, build: &PropBuild) -> Option<PlacedSketch> {
    let id = session.editing?;
    let placed = build.evaluated.sketches.iter().find(|placed| placed.id == id)?;
    let sketch = sketch_of(editor.doc(), id)?.clone();
    Some(PlacedSketch { sketch, ..placed.clone() })
}

pub struct SketchEditPlugin;

impl Plugin for SketchEditPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        app.init_resource::<SketchSession>()
            .add_systems(
                Update,
                (keep_the_session, aim_the_view, edit_the_sketch, draw_the_session)
                    .chain()
                    .run_if(in_state(AppMode::Prop)),
            )
            .add_systems(EguiPrimaryContextPass, label_the_length.run_if(in_state(AppMode::Prop)))
            // The camera's own restore puts the map's views back; the sketch's
            // saved view is of a mode that is ending.
            .add_systems(OnExit(AppMode::Prop), |mut session: ResMut<SketchSession>, mut editor: ResMut<PropEditor>| {
                *session = SketchSession::default();
                editor.peek_at(None);
            });
    }
}

/// Show the prop as it stood at the open sketch, leave a sketch that has gone
/// — deleted, or undone away — and forget picks and a chain naming points
/// that have.
fn keep_the_session(
    mut session: ResMut<SketchSession>,
    mut editor: ResMut<PropEditor>,
    mut hovered: ResMut<HoveredPick>,
) {
    // Compared first: a `ResMut` written every frame reads as changed.
    if editor.peek() != session.editing {
        editor.peek_at(session.editing);
    }
    let Some(id) = session.editing else { return };
    if hovered.0.is_some() {
        hovered.0 = None;
    }
    let Some(sketch) = sketch_of(editor.doc(), id) else {
        session.leave();
        return;
    };
    let exists = |pick: &SketchPick| match pick {
        SketchPick::Point(id) => sketch.point(*id).is_some(),
        SketchPick::Line(id) => sketch.entities.iter().any(|entity| entity.id() == *id),
    };
    if !session.picks.iter().all(exists) {
        session.picks.retain(exists);
    }
    if session.chain.is_some_and(|chain| sketch.point(chain).is_none()) {
        session.chain = None;
    }
    if session.pending.iter().any(|point| sketch.point(*point).is_none()) {
        session.pending.clear();
    }
}

/// Turns the perspective view to look straight down the sketch's plane, with
/// the plane's `y` up, and puts it back on leaving. Waits for the sketch to be
/// evaluated, which a sketch made this frame is not yet.
fn aim_the_view(
    mut session: ResMut<SketchSession>,
    editor: Res<PropEditor>,
    build: Res<PropBuild>,
    time: Res<Time>,
    mouse: Res<CurrentMouseInput>,
    mut scrolls: MessageReader<bevy::input::mouse::MouseWheel>,
    mut pivot: ResMut<OrbitPivot>,
    mut cameras: Query<(Entity, &mut Transform, &mut Projection, &Multicam)>,
) {
    // Orbiting, panning or zooming takes the view back from a turn.
    let navigating = matches!(mouse.pressed, Some(MouseButton::Right | MouseButton::Middle)) | (scrolls.read().count() > 0);
    if navigating {
        session.turn = None;
    }
    if let Some(mut turn) = session.turn {
        turn.elapsed += time.delta_secs();
        let done = turn.elapsed >= turn.seconds;
        if let Ok((_, mut transform, ..)) = cameras.get_mut(turn.camera) {
            let (now, center) = turn.from.toward(turn.to, ease(turn.elapsed / turn.seconds));
            *transform = now;
            pivot.0 = center;
        }
        session.turn = (!done).then_some(turn);
    }
    match session.view {
        ViewChange::None => {}
        ViewChange::Restore => {
            if let Some((entity, transform, projection, center)) = session.saved.take()
                && let Ok((_, mut current, mut current_projection, _)) = cameras.get_mut(entity)
            {
                *current = transform;
                *current_projection = projection;
                pivot.0 = center;
            }
            session.view = ViewChange::None;
        }
        ViewChange::Aim => {
            let Some(placed) = live(&session, &editor, &build) else { return };
            let (center, radius) = extent(&placed.sketch);
            let center = placed.plane.lift(center);
            let facing = Transform::from_translation(center + *placed.plane.normal * radius * 3.2)
                .looking_at(center, *placed.plane.y());
            for (entity, transform, projection, multicam) in &mut cameras {
                if multicam.axis != CameraAxis::None {
                    continue;
                }
                if session.saved.is_none() {
                    session.saved = Some((entity, *transform, projection.clone(), pivot.0));
                }
                let (from, to) = (Viewpoint::of(&transform, pivot.0), Viewpoint::of(&facing, center));
                let seconds = from.rotation.angle_between(to.rotation) / TURN_RATE;
                session.turn = Some(Turn { camera: entity, from, to, elapsed: 0.0, seconds: seconds.max(f32::EPSILON) });
            }
            session.view = ViewChange::None;
        }
    }
}

/// The middle of a sketch's points and how far they reach from it, in its
/// plane; an empty sketch is a weapon-sized patch round the plane's origin.
fn extent(sketch: &Sketch) -> (Vec2, f32) {
    let points: Vec<Vec2> = sketch.points.iter().map(|point| Vec2::from_array(point.at)).collect();
    let Some((min, max)) = points
        .iter()
        .copied()
        .map(|p| (p, p))
        .reduce(|(min, max), (p, _)| (min.min(p), max.max(p)))
    else {
        return (Vec2::ZERO, 0.05);
    };
    ((min + max) / 2.0, ((max - min).length() / 2.0).max(0.05))
}

#[allow(clippy::too_many_arguments)]
fn edit_the_sketch(
    mut session: ResMut<SketchSession>,
    mut editor: ResMut<PropEditor>,
    build: Res<PropBuild>,
    mouse: Res<CurrentMouseInput>,
    actions: Res<Actions>,
    cameras: Query<(&Camera, &GlobalTransform), With<Multicam>>,
) {
    let Some(id) = session.editing else { return };

    // Before anything that needs the pointer over a viewport: a drag that
    // ends over a panel still ends.
    let held = mouse.pressed == Some(MouseButton::Left);
    if !held && let Some(press) = session.press.take() {
        let stroke = std::mem::take(&mut session.stroke);
        session.doomed.clear();
        session.stroke_scanned = 0;
        match (session.tool, press.dragging, press.on) {
            (SketchTool::Erase, true, _) => {
                editor.edit(|doc| sketch_of_mut(doc, id).map(|sketch| sketch.erase_along(&stroke)));
            }
            (SketchTool::Erase, false, Some(SketchPick::Line(line))) => {
                if let Some(&at) = stroke.first() {
                    editor.edit(|doc| sketch_of_mut(doc, id).map(|sketch| sketch.erase_piece(line, at)));
                }
            }
            (SketchTool::Erase, false, _) => {}
            (_, true, _) => editor.end_gesture(),
            (_, false, Some(pick)) => session.toggle(pick),
            (_, false, None) => session.picks.clear(),
        }
    }

    if actions.just_pressed(Action::Cancel) {
        session.chain = None;
        session.pending.clear();
    }
    if actions.just_pressed(Action::Confirm) {
        session.finish_curve(&mut editor);
    }
    if actions.just_pressed(Action::ResetView) {
        session.view = ViewChange::Aim;
    }
    // Not mid-press, which would strand an eraser stroke or a drag. Onto the
    // tool already up it drops the line in progress, as the button does.
    let chosen = [(Action::SketchLine, SketchTool::Line), (Action::SketchErase, SketchTool::Erase)]
        .into_iter()
        .find(|(action, _)| actions.just_pressed(*action))
        .map(|(_, tool)| tool);
    if let Some(tool) = chosen
        && session.press.is_none()
    {
        session.set_tool(tool);
    }
    if actions.just_pressed(Action::Delete) && !session.picks.is_empty() {
        let ids: Vec<SketchId> = session.picks.iter().map(|pick| pick.id()).collect();
        editor.edit(|doc| sketch_of_mut(doc, id).map(|sketch| sketch.remove(&ids)));
        session.picks.clear();
        session.chain = None;
    }

    let Some(placed) = live(&session, &editor, &build) else {
        session.cursor = None;
        session.on_plane = None;
        session.hovered = None;
        return;
    };
    let (Some(ray), Some((camera, at))) =
        (mouse.world_pos, mouse.in_camera.and_then(|camera| cameras.get(camera).ok()))
    else {
        session.cursor = None;
        session.on_plane = None;
        session.hovered = None;
        return;
    };
    let pointer = Pointer { ray, face: None, project: |p: Vec3| camera.world_to_viewport(at, p).ok() };

    let dragged = match session.press {
        Some(Press { on: Some(SketchPick::Point(point)), dragging: true, .. }) => Some(point),
        _ => None,
    };
    let cursor = placed
        .plane
        .hit(ray)
        .map(|hit| snap(placed.plane.flatten(hit), &placed, &pointer, &build.evaluated.pickables, dragged));
    session.cursor = cursor;
    wind_the_arc(&mut session, &placed.sketch);
    session.on_plane = placed.plane.hit(ray).map(|hit| placed.plane.flatten(hit));
    session.hovered = match (session.tool, session.press) {
        (SketchTool::Select, None) => under(&placed, &pointer),
        (SketchTool::Erase, None) => line_under(&placed, &pointer).map(SketchPick::Line),
        _ => None,
    };

    let clicked = mouse.just_pressed && held;
    match session.tool {
        SketchTool::Line => {
            if let (true, Some(target)) = (clicked, cursor) {
                let chain = session.chain;
                session.chain = editor
                    .edit(|doc| sketch_of_mut(doc, id).map(|sketch| draw_to(sketch, chain, target)))
                    .flatten();
            }
        }
        SketchTool::Arc | SketchTool::Circle | SketchTool::Bezier | SketchTool::Nurbs => {
            if let (true, Some(target)) = (clicked, cursor) {
                // Clicking the last point again finishes an open-ended curve.
                if session.tool.open_ended() && target.point.is_some() && target.point == session.pending.last().copied() {
                    session.finish_curve(&mut editor);
                    return;
                }
                let (tool, mut pending, sweep) = (session.tool, std::mem::take(&mut session.pending), session.arc_sweep);
                editor.edit(|doc| {
                    let Some(sketch) = sketch_of_mut(doc, id) else { return };
                    if let (SketchTool::Circle, Some(&center)) = (tool, pending.first()) {
                        let radius = sketch.point(center).map_or(0.0, |c| c.distance(target.at));
                        if radius > 0.0 {
                            sketch.add_circle(center, radius);
                        }
                        pending.clear();
                        return;
                    }
                    if let (SketchTool::Arc, &[center, start]) = (tool, &pending[..]) {
                        place_arc(sketch, center, start, target.point, sweep);
                        pending.clear();
                        return;
                    }
                    pending.push(target.point.unwrap_or_else(|| sketch.add_point(target.at)));
                });
                session.pending = pending;
            }
        }
        SketchTool::Erase => {
            if clicked && let (Some(from), Some(at)) = (mouse.global_pos, session.on_plane) {
                session.press = Some(Press { on: session.hovered, from, dragging: false });
                session.stroke = vec![at];
            }
            let Some(mut press) = session.press else { return };
            if !press.dragging && mouse.global_pos.is_some_and(|now| now.distance(press.from) > DRAG_PX) {
                press.dragging = true;
                session.press = Some(press);
            }
            if press.dragging
                && let Some(at) = session.on_plane
                && session.stroke.last() != Some(&at)
            {
                session.stroke.push(at);
            }
            while session.stroke_scanned + 1 < session.stroke.len() {
                let step = session.stroke_scanned;
                let (from, to) = (session.stroke[step], session.stroke[step + 1]);
                let crossed = crossed_pieces(&placed.sketch, from, to);
                session.doomed.extend(crossed);
                session.stroke_scanned += 1;
            }
        }
        SketchTool::Select => {
            if clicked && let Some(from) = mouse.global_pos {
                session.press = Some(Press { on: session.hovered, from, dragging: false });
            }
            let Some(mut press) = session.press else { return };
            let Some(SketchPick::Point(point)) = press.on else { return };
            if !press.dragging && mouse.global_pos.is_some_and(|now| now.distance(press.from) > DRAG_PX) {
                press.dragging = true;
                session.press = Some(press);
                editor.begin_gesture();
            }
            if press.dragging
                && let Some(target) = cursor
                && placed.sketch.point(point) != Some(target.at)
            {
                // Only on a real move, or a pointer held still dirties the
                // document every frame.
                if let Some(sketch) = sketch_of_mut(editor.doc_mut(), id) {
                    sketch.move_point(point, target.at);
                }
            }
        }
    }
}

/// Returns where the next click draws from: where this one landed, unless the
/// line ended on an existing point or enclosed a new region, which ends the
/// shape. Clicking the chain's own end draws nothing.
fn draw_to(sketch: &mut Sketch, chain: Option<SketchId>, target: Snapped) -> Option<SketchId> {
    let landed = target.point.unwrap_or_else(|| sketch.add_point(target.at));
    let Some(from) = chain.filter(|from| *from != landed) else { return Some(landed) };
    let before = arrange(sketch).regions.len();
    sketch.add_line(from, landed);
    (target.point.is_none() && arrange(sketch).regions.len() <= before).then_some(landed)
}

/// The nearest sketch point within [`SNAP_PX`], else the nearest model corner
/// — its position copied, seen straight down the plane — else the grid.
/// `skip` is a point being dragged, which must not snap to itself.
fn snap<P: Fn(Vec3) -> Option<Vec2>>(
    on_plane: Vec2,
    placed: &PlacedSketch,
    pointer: &Pointer<P>,
    pickables: &[Pickable],
    skip: Option<SketchId>,
) -> Snapped {
    let nearest_point = placed
        .sketch
        .points
        .iter()
        .filter(|point| Some(point.id) != skip)
        .filter_map(|point| {
            let at = Vec2::from_array(point.at);
            Some((point.id, at, pointer.to_point(placed.world(at))?))
        })
        .filter(|(_, _, pixels)| *pixels <= SNAP_PX)
        .min_by(|a, b| a.2.total_cmp(&b.2));
    if let Some((id, at, _)) = nearest_point {
        return Snapped { at, point: Some(id) };
    }
    let nearest_corner = pickables
        .iter()
        .filter_map(|pickable| match pickable.at {
            Located::Point(p) => Some((p, pointer.to_point(p)?)),
            _ => None,
        })
        .filter(|(_, pixels)| *pixels <= SNAP_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((corner, _)) = nearest_corner {
        return Snapped { at: placed.plane.flatten(corner), point: None };
    }
    Snapped { at: (on_plane / GRID).round() * GRID, point: None }
}

/// What a click with the select tool would pick: a point, else a line.
fn under<P: Fn(Vec3) -> Option<Vec2>>(placed: &PlacedSketch, pointer: &Pointer<P>) -> Option<SketchPick> {
    let point = placed
        .sketch
        .points
        .iter()
        .filter_map(|point| Some((point.id, pointer.to_point(placed.world(Vec2::from_array(point.at)))?)))
        .filter(|(_, pixels)| *pixels <= POINT_PICK_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| SketchPick::Point(id));
    point.or_else(|| line_under(placed, pointer).map(SketchPick::Line))
}

/// A line or curve, by its nearest piece.
fn line_under<P: Fn(Vec3) -> Option<Vec2>>(placed: &PlacedSketch, pointer: &Pointer<P>) -> Option<SketchId> {
    placed
        .sketch
        .curves()
        .filter_map(|(id, curve, _)| {
            let facets = curve.facets();
            let nearest = facets
                .windows(2)
                .filter_map(|f| pointer.to_segment(placed.world(f[0]), placed.world(f[1])))
                .min_by(f32::total_cmp)?;
            Some((id, nearest))
        })
        .filter(|(_, pixels)| *pixels <= EDGE_PICK_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// The pieces the eraser would take: the one under a click, or every one a
/// drag has crossed so far.
fn doomed(session: &SketchSession, sketch: &Sketch) -> Vec<(Vec2, Vec2)> {
    if session.press.is_some_and(|press| press.dragging) {
        return session.doomed.clone();
    }
    let line = match session.press.map(|press| press.on).unwrap_or(session.hovered) {
        Some(SketchPick::Line(line)) => line,
        _ => return vec![],
    };
    let at = session.stroke.first().copied().or(session.on_plane);
    at.map(|at| piece_at(sketch, line, at)).unwrap_or_default()
}

/// The piece of a line between junctions, or a whole curve, as segments.
fn piece_at(sketch: &Sketch, entity: SketchId, at: Vec2) -> Vec<(Vec2, Vec2)> {
    if let Some((a, b)) = sketch.line(entity) {
        return sketch.piece_of(entity, at).map(|(t0, t1)| vec![(a.lerp(b, t0), a.lerp(b, t1))]).unwrap_or_default();
    }
    let facets = sketch.curve(entity).map(|curve| curve.facets()).unwrap_or_default();
    facets.windows(2).map(|f| (f[0], f[1])).collect()
}

fn crossed_pieces(sketch: &Sketch, from: Vec2, to: Vec2) -> Vec<(Vec2, Vec2)> {
    sketch
        .curves()
        .filter_map(|(entity, curve, _)| {
            let facets = curve.facets();
            let at = facets.windows(2).find_map(|f| crossing(f[0], f[1], from, to).map(|(_, _, at)| at))?;
            Some(piece_at(sketch, entity, at))
        })
        .flatten()
        .collect()
}

const POINT: Color = Color::srgb(0.92, 0.92, 0.98);
const PICKED: Color = Color::srgb(1.0, 0.8, 0.2);
const HOVERED: Color = Color::srgb(0.5, 0.9, 1.0);
const GRID_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.06);
const REGION: Color = Color::srgba(0.55, 0.8, 0.95, 0.35);
const DOOMED: Color = Color::srgb(1.0, 0.35, 0.3);

#[derive(Default)]
struct RegionCache {
    from: Option<Sketch>,
    /// In the plane, so a moved plane needs no re-hatching.
    hatched: Vec<(Vec2, Vec2)>,
}

/// Regions are hatched so a closed shape is visibly closed. Drawn from the
/// document rather than the build, so nothing lags a drag.
fn draw_the_session(
    session: Res<SketchSession>,
    editor: Res<PropEditor>,
    build: Res<PropBuild>,
    mouse: Res<CurrentMouseInput>,
    cameras: Query<(Entity, &GlobalTransform, &Multicam)>,
    mut cache: Local<RegionCache>,
    mut gizmos: Gizmos,
) {
    let Some(placed) = live(&session, &editor, &build) else { return };
    let (center, reach) = extent(&placed.sketch);
    let marker = reach * 0.02;

    // Gizmos in one plane depth-fight, so the grid sits just behind the plane
    // as seen from the view under the pointer, or else the perspective view.
    let viewer = cameras
        .iter()
        .find(|(entity, ..)| Some(*entity) == mouse.in_camera)
        .or_else(|| cameras.iter().find(|(.., multicam)| multicam.axis == CameraAxis::None))
        .map(|(_, at, _)| at.translation());
    let facing = viewer.map_or(1.0, |eye| (eye - placed.world(center)).dot(*placed.plane.normal).signum());
    let behind = -*placed.plane.normal * facing * GRID_BEHIND;

    // Snapped to the grid's own lines, so the grid does not slide as the
    // sketch grows.
    let half = ((reach * 1.6 / GRID).ceil() as i32).min(MAX_GRID_LINES);
    let middle = (center / GRID).round() * GRID;
    for i in -half..=half {
        let offset = i as f32 * GRID;
        let span = half as f32 * GRID;
        gizmos.line(
            placed.world(middle + Vec2::new(offset, -span)) + behind,
            placed.world(middle + Vec2::new(offset, span)) + behind,
            GRID_COLOR,
        );
        gizmos.line(
            placed.world(middle + Vec2::new(-span, offset)) + behind,
            placed.world(middle + Vec2::new(span, offset)) + behind,
            GRID_COLOR,
        );
    }

    // `reach` comes from the sketch, so the sketch is the whole key.
    if cache.from.as_ref() != Some(&placed.sketch) {
        let arrangement = arrange(&placed.sketch);
        cache.hatched = arrangement
            .regions
            .iter()
            .flat_map(|region| hatch(&region.outer, &region.holes, reach / 12.0))
            .collect();
        cache.from = Some(placed.sketch.clone());
    }
    for (a, b) in &cache.hatched {
        gizmos.line(placed.world(*a), placed.world(*b), REGION);
    }

    let color_of = |pick: SketchPick| {
        if session.picks.contains(&pick) {
            Some(PICKED)
        } else if session.hovered == Some(pick) {
            Some(HOVERED)
        } else {
            None
        }
    };
    for point in &placed.sketch.points {
        let pick = SketchPick::Point(point.id);
        let color = match session.chain == Some(point.id) {
            true => HOVERED,
            false => color_of(pick).unwrap_or(POINT),
        };
        let at = placed.world(Vec2::from_array(point.at));
        gizmos.sphere(Isometry3d::from_translation(at), marker, color);
    }
    for (id, curve, construction) in placed.sketch.curves() {
        let color = color_of(SketchPick::Line(id)).unwrap_or(POINT);
        for facet in curve.facets().windows(2) {
            let (a, b) = (placed.world(facet[0]), placed.world(facet[1]));
            match construction {
                true => dashed_line(&mut gizmos, a, b, color, marker * 2.0, marker),
                false => {
                    gizmos.line(a, b, color);
                }
            }
        }
    }
    // What a curve tool would place with the pointer as its next point.
    if !session.pending.is_empty() {
        let mut points: Vec<Vec2> = session.pending.iter().filter_map(|point| placed.sketch.point(*point)).collect();
        points.extend(session.cursor.map(|cursor| cursor.at));
        let solid = |gizmos: &mut Gizmos, curve: &Curve2| {
            for facet in curve.facets().windows(2) {
                gizmos.line(placed.world(facet[0]), placed.world(facet[1]), HOVERED);
            }
        };
        match (session.tool, &points[..]) {
            (SketchTool::Circle, [center, rim]) => solid(&mut gizmos, &Curve2::Circle { center: *center, radius: center.distance(*rim) }),
            // The radius as a dashed circle until the start is down, then the
            // arc it would make.
            (SketchTool::Arc, [center, rim]) => {
                let circle = Curve2::Circle { center: *center, radius: center.distance(*rim) };
                for facet in circle.facets().windows(2) {
                    dashed_line(&mut gizmos, placed.world(facet[0]), placed.world(facet[1]), HOVERED, marker, marker);
                }
            }
            (SketchTool::Arc, [center, start, _]) if session.arc_sweep != 0.0 => {
                solid(&mut gizmos, &arc_by(*center, *start, session.arc_sweep));
            }
            (SketchTool::Bezier, points) if points.len() >= 2 => {
                hull(&mut gizmos, &placed, points, marker);
                solid(&mut gizmos, &Curve2::Bezier { points: points.to_vec() });
            }
            (SketchTool::Nurbs, points) if points.len() >= 2 => {
                hull(&mut gizmos, &placed, points, marker);
                let degree = (points.len() - 1).min(3);
                let nurbs = Curve2::Nurbs {
                    degree,
                    points: points.to_vec(),
                    weights: vec![1.0; points.len()],
                    knots: clamped_knots(points.len(), degree),
                };
                solid(&mut gizmos, &nurbs);
            }
            _ => {}
        }
    }

    if session.tool == SketchTool::Erase {
        for (a, b) in doomed(&session, &placed.sketch) {
            gizmos.line(placed.world(a), placed.world(b), DOOMED);
        }
        for step in session.stroke.windows(2) {
            dashed_line(&mut gizmos, placed.world(step[0]), placed.world(step[1]), DOOMED, marker, marker);
        }
    }

    // Ringed when the click would join an existing point.
    if !matches!(session.tool, SketchTool::Select | SketchTool::Erase)
        && let Some(cursor) = session.cursor
    {
        let at = placed.world(cursor.at);
        gizmos.sphere(Isometry3d::from_translation(at), marker * 1.3, HOVERED);
        if cursor.point.is_some() {
            let facing = Quat::from_rotation_arc(Vec3::Z, *placed.plane.normal);
            gizmos.circle(Isometry3d::new(at, facing), marker * 3.0, HOVERED);
        }
        if let Some(from) = session.chain.and_then(|chain| placed.sketch.point(chain)) {
            gizmos.line(placed.world(from), at, HOVERED);
        }
    }
}

/// Logical pixels between the cursor and its length label, clear of the snap ring.
const LABEL_GAP: f32 = 16.0;

fn measured_from(session: &SketchSession, sketch: &Sketch) -> Option<(SketchId, Vec2)> {
    let from = match session.tool {
        SketchTool::Line => session.chain?,
        SketchTool::Circle | SketchTool::Arc => match session.pending[..] {
            [center] => center,
            _ => return None,
        },
        SketchTool::Bezier | SketchTool::Nurbs => *session.pending.last()?,
        SketchTool::Select | SketchTool::Erase => return None,
    };
    Some((from, sketch.point(from)?))
}

fn distance_text(meters: f32, in_meters: bool, signed: bool) -> String {
    let (value, key, places) = match in_meters {
        false => (meters * 100.0, "prop.sketch.centimeters", 1),
        true => (meters, "prop.sketch.meters", 3),
    };
    let mut number = format!("{value:.places$}");
    if number.contains('.') {
        number = number.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if number == "-0" {
        number = "0".to_string();
    }
    if signed && number != "0" && !number.starts_with('-') {
        number.insert(0, '+');
    }
    get!(key, "value", number)
}

/// Legs along the sketch's own `x` and `y`, all in one unit, chosen from the
/// rounded length.
pub fn measure_text(delta: Vec2) -> String {
    let length = delta.length();
    let in_meters = (length * 1000.0).round() >= 1000.0;
    get!(
        "prop.sketch.measure",
        "length",
        distance_text(length, in_meters, false),
        "across",
        distance_text(delta.x, in_meters, true),
        "up",
        distance_text(delta.y, in_meters, true)
    )
}

/// Sides are tried facing away from `toward`, where the measured line comes
/// from, so the label does not cover the line. A label that fits nowhere is
/// left partly outside `bounds` rather than over the cursor.
pub fn place_label(cursor: Vec2, toward: Vec2, size: Vec2, bounds: Rect) -> Rect {
    let half = size / 2.0;
    let mut sides = [Vec2::X, Vec2::NEG_X, Vec2::NEG_Y, Vec2::Y];
    let away = -toward.normalize_or_zero();
    sides.sort_by(|a, b| b.dot(away).total_cmp(&a.dot(away)));
    let push_in = |min: Vec2| min.max(bounds.min).min((bounds.max - size).max(bounds.min));
    let rect_on = |side: Vec2| {
        let min = cursor + side * (half + Vec2::splat(LABEL_GAP)) - half;
        let slid = push_in(min);
        // Across the side only, so it keeps its gap from the cursor.
        let min = if side.x == 0.0 { Vec2::new(slid.x, min.y) } else { Vec2::new(min.x, slid.y) };
        Rect::from_corners(min, min + size)
    };
    let inside = |rect: Rect| {
        let overlap = rect.intersect(bounds);
        if overlap.is_empty() { 0.0 } else { overlap.size().x * overlap.size().y }
    };
    let whole = size.x * size.y;
    if let Some(rect) = sides.iter().map(|side| rect_on(*side)).find(|rect| inside(*rect) >= whole - 1e-3) {
        return rect;
    }
    sides.iter().map(|side| rect_on(*side)).max_by(|a, b| inside(*a).total_cmp(&inside(*b))).unwrap()
}

fn label_the_length(
    session: Res<SketchSession>,
    editor: Res<PropEditor>,
    build: Res<PropBuild>,
    mouse: Res<CurrentMouseInput>,
    cameras: Query<(&Camera, &GlobalTransform), With<Multicam>>,
    mut contexts: EguiContexts,
) {
    let (Some(cursor), Some(id)) = (session.cursor, session.editing) else { return };
    // Not `live`, which clones the sketch.
    let Some(sketch) = sketch_of(editor.doc(), id) else { return };
    let Some(placed) = build.evaluated.sketches.iter().find(|placed| placed.id == id) else { return };
    let Some((anchor, from)) = measured_from(&session, sketch) else { return };
    if cursor.point == Some(anchor) {
        return;
    }
    let Some((camera, view)) = mouse.in_camera.and_then(|camera| cameras.get(camera).ok()) else { return };
    let (Ok(at), Ok(start)) = (
        camera.world_to_viewport(view, placed.world(cursor.at)),
        camera.world_to_viewport(view, placed.world(from)),
    ) else {
        return;
    };
    let Some(bounds) = camera.logical_viewport_rect() else { return };
    let Ok(ctx) = contexts.ctx_mut() else { return };
    // Bevy's logical pixels are egui points times its zoom.
    let zoom = ctx.zoom_factor();
    let (at, start) = (at / zoom, start / zoom);
    let bounds = Rect::from_corners(bounds.min / zoom, bounds.max / zoom);

    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("sketch_length")));
    let galley = painter.layout_no_wrap(
        measure_text(cursor.at - from),
        egui::FontId::proportional(13.0),
        {
            let [r, g, b, _] = POINT.to_srgba().to_u8_array();
            egui::Color32::from_rgb(r, g, b)
        },
    );
    let padding = Vec2::new(6.0, 3.0);
    let size = Vec2::new(galley.size().x, galley.size().y) + padding * 2.0;
    let rect = place_label(at, start - at, size, bounds);
    let rect = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.min.y), egui::pos2(rect.max.x, rect.max.y));
    painter.rect_filled(rect, 3.0, egui::Color32::from_black_alpha(190));
    painter.galley(rect.min + egui::vec2(padding.x, padding.y), galley, egui::Color32::PLACEHOLDER);
}

/// A curve tool's control points so far, joined by dashes.
fn hull(gizmos: &mut Gizmos, placed: &PlacedSketch, points: &[Vec2], marker: f32) {
    for step in points.windows(2) {
        dashed_line(gizmos, placed.world(step[0]), placed.world(step[1]), HOVERED, marker, marker);
    }
}

/// A sweep this close to a whole turn closes the arc on its start.
const WHOLE_TURN: f32 = std::f32::consts::TAU - 1e-4;

/// Follows the pointer round the arc's center while the start is down, so an
/// arc can go past a half turn; anything else forgets it.
fn wind_the_arc(session: &mut SketchSession, sketch: &Sketch) {
    let center_and_start = match (session.tool, &session.pending[..]) {
        (SketchTool::Arc, [center, start]) => sketch.point(*center).zip(sketch.point(*start)),
        _ => None,
    };
    let Some((center, start)) = center_and_start else {
        (session.arc_sweep, session.arc_seen, session.arc_lost) = (0.0, None, false);
        return;
    };
    let seen = session.arc_seen.unwrap_or(start - center);
    // Near the center a pixel is a large turn, so the direction is not
    // trusted there.
    let near = start.distance(center) * NEAR_CENTER;
    let Some(now) = session.cursor.map(|cursor| cursor.at - center).filter(|now| now.length() > near) else {
        session.arc_lost = true;
        return;
    };
    session.arc_sweep = wound(session.arc_sweep, seen, now, session.arc_lost);
    (session.arc_seen, session.arc_lost) = (Some(now), false);
}

/// Of the start's distance from the center, how near the pointer may come
/// before its direction stops counting.
const NEAR_CENTER: f32 = 0.15;
/// A turn this close to a half turn could have gone either way.
const AMBIGUOUS: f32 = 0.9 * std::f32::consts::PI;

/// `sweep` carried on by the turn from `seen` to `now`, held within a whole
/// turn either side. The shorter way, unless that is a guess — nearly a half
/// turn, or past a quarter turn since the pointer was `lost` — in which case
/// the way the arc is already going.
pub fn wound(sweep: f32, seen: Vec2, now: Vec2, lost: bool) -> f32 {
    let mut turn = seen.angle_to(now);
    let guess = turn.abs() > AMBIGUOUS || (lost && turn.abs() > std::f32::consts::FRAC_PI_2);
    if guess && sweep != 0.0 && turn.signum() != sweep.signum() {
        turn += std::f32::consts::TAU * sweep.signum();
    }
    (sweep + turn).clamp(-std::f32::consts::TAU, std::f32::consts::TAU)
}

/// Below this an arc is too short to place: radians.
const LEAST_SWEEP: f32 = 1e-2;

/// The arc round `center` from `start` by `sweep`, ending on `snapped` if the
/// click landed on another point. A whole turn closes on its own start, as a
/// circle does; a snap onto the start short of that is ignored, since it
/// would read as a whole turn.
fn place_arc(sketch: &mut Sketch, center: SketchId, start: SketchId, snapped: Option<SketchId>, sweep: f32) {
    if sweep.abs() < LEAST_SWEEP {
        return;
    }
    let end = match sweep.abs() >= WHOLE_TURN {
        true => start,
        false => match snapped.filter(|point| *point != start) {
            Some(point) => point,
            None => {
                let (Some(c), Some(s)) = (sketch.point(center), sketch.point(start)) else { return };
                let Curve2::Arc { end, .. } = arc_by(c, s, sweep) else { return };
                sketch.add_point(end)
            }
        },
    };
    sketch.add_arc(center, start, end, sweep < 0.0);
}

/// The arc round `center` from `start` by `sweep` radians, positive
/// counter-clockwise.
pub fn arc_by(center: Vec2, start: Vec2, sweep: f32) -> Curve2 {
    let end = center + Vec2::from_angle(sweep).rotate(start - center);
    Curve2::Arc { center, start, end, sweep }
}

/// Diagonal lines `spacing` apart. Even-odd along each line, so holes come
/// out by themselves.
pub fn hatch(outer: &[Vec2], holes: &[Vec<Vec2>], spacing: f32) -> Vec<(Vec2, Vec2)> {
    // In coordinates turned an eighth, where the hatching runs along `t`.
    let turn = |p: Vec2| Vec2::new(p.x + p.y, p.x - p.y) / std::f32::consts::SQRT_2;
    let back = |s: f32, t: f32| Vec2::new(s + t, s - t) / std::f32::consts::SQRT_2;
    let loops: Vec<Vec<Vec2>> =
        std::iter::once(outer).chain(holes.iter().map(Vec::as_slice)).map(|l| l.iter().map(|p| turn(*p)).collect()).collect();
    let (low, high) = loops
        .iter()
        .flatten()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), p| (low.min(p.x), high.max(p.x)));
    if spacing <= 0.0 || !low.is_finite() {
        return vec![];
    }
    let mut lines = vec![];
    let mut s = (low / spacing).floor() * spacing + spacing / 2.0;
    while s < high {
        let mut crossings: Vec<f32> = loops
            .iter()
            .flat_map(|l| (0..l.len()).map(move |i| (l[i], l[(i + 1) % l.len()])))
            .filter(|(a, b)| (a.x > s) != (b.x > s))
            .map(|(a, b)| a.y + (s - a.x) / (b.x - a.x) * (b.y - a.y))
            .collect();
        crossings.sort_by(f32::total_cmp);
        for pair in crossings.chunks_exact(2) {
            lines.push((back(s, pair[0]), back(s, pair[1])));
        }
        s += spacing;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapped(at: Vec2, point: Option<SketchId>) -> Snapped {
        Snapped { at, point }
    }

    #[test]
    fn the_measure_reads_its_length_then_its_legs() {
        assert_eq!(measure_text(Vec2::new(0.3, -0.4)), "50 cm (+30 cm, -40 cm)");
        assert_eq!(measure_text(Vec2::new(0.005, 0.0)), "0.5 cm (+0.5 cm, 0 cm)");
        assert_eq!(measure_text(Vec2::new(1.2, 0.0)), "1.2 m (+1.2 m, 0 m)");
        assert_eq!(measure_text(Vec2::new(1.12, 0.5)), "1.227 m (+1.12 m, +0.5 m)");
        assert_eq!(measure_text(Vec2::new(0.99996, 0.0)), "1 m (+1 m, 0 m)");
    }

    #[test]
    fn each_tool_measures_from_its_own_point() {
        let mut sketch = Sketch::default();
        let (a, b) = (sketch.add_point(Vec2::ZERO), sketch.add_point(Vec2::X));
        let from = |tool, chain, pending: Vec<SketchId>| {
            let session = SketchSession { tool, chain, pending, ..default() };
            measured_from(&session, &sketch).map(|(id, _)| id)
        };
        assert_eq!(from(SketchTool::Line, Some(b), vec![]), Some(b));
        assert_eq!(from(SketchTool::Line, None, vec![]), None);
        assert_eq!(from(SketchTool::Circle, None, vec![a]), Some(a));
        assert_eq!(from(SketchTool::Arc, None, vec![a]), Some(a));
        assert_eq!(from(SketchTool::Arc, None, vec![a, b]), None, "an arc's start is down: it winds, not measures");
        assert_eq!(from(SketchTool::Bezier, None, vec![a, b]), Some(b));
        assert_eq!(from(SketchTool::Nurbs, None, vec![]), None);
        assert_eq!(from(SketchTool::Select, Some(b), vec![a]), None);
    }

    #[test]
    fn the_label_stays_off_the_line_and_on_screen() {
        let bounds = Rect::new(0.0, 0.0, 800.0, 600.0);
        let size = Vec2::new(120.0, 20.0);
        // Drawn from the left: the label goes right.
        let rect = place_label(Vec2::new(400.0, 300.0), Vec2::NEG_X, size, bounds);
        assert!(rect.min.x > 400.0);
        // At the right edge: above or below, not back over the line.
        let rect = place_label(Vec2::new(790.0, 300.0), Vec2::NEG_X, size, bounds);
        assert!(bounds.contains(rect.min) && bounds.contains(rect.max));
        assert!(rect.max.y < 300.0 || rect.min.y > 300.0);
        // Wider than the viewport: slid to its edge, but kept off the cursor.
        let cursor = Vec2::new(5.0, 5.0);
        let rect = place_label(cursor, Vec2::ONE, Vec2::new(900.0, 20.0), bounds);
        assert_eq!(rect.min.x, 0.0);
        assert!(!rect.contains(cursor));
    }

    #[test]
    fn a_turn_eases_in_and_out_and_lands_on_its_target() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(0.5), 0.5);
        assert_eq!(ease(1.0), 1.0);
        // Slow at both ends: the first and last tenth cover 2% each.
        assert!((ease(0.1) - 0.02).abs() < 1e-6);
        assert!((1.0 - ease(0.9) - 0.02).abs() < 1e-6);
        let pivot = Vec3::new(0.1, 0.0, 0.0);
        let start = Transform::from_xyz(1.0, 1.0, 1.0).looking_at(Vec3::ZERO, Vec3::Y);
        let end = Transform::from_xyz(0.1, 0.0, 0.5).looking_at(pivot, Vec3::Y);
        let (from, to) = (Viewpoint::of(&start, Vec3::ZERO), Viewpoint::of(&end, pivot));
        let (at_start, _) = from.toward(to, 0.0);
        let (at_end, center) = from.toward(to, 1.0);
        assert!(at_start.translation.distance(start.translation) < 1e-5);
        assert!(at_end.translation.distance(end.translation) < 1e-5);
        assert!(at_end.rotation.angle_between(end.rotation) < 1e-4);
        assert_eq!(center, pivot);
    }

    /// Going round in steps of a sixth of a turn reaches 300 degrees, and
    /// going on stops at a whole turn.
    #[test]
    fn an_arc_winds_past_a_half_turn_and_stops_at_a_whole_one() {
        let step = std::f32::consts::TAU / 6.0;
        let mut sweep = 0.0;
        let mut seen = Vec2::X;
        for k in 1..=5 {
            let now = Vec2::from_angle(step * k as f32);
            sweep = wound(sweep, seen, now, false);
            seen = now;
        }
        assert!((sweep - 300f32.to_radians()).abs() < 1e-4, "{}", sweep.to_degrees());
        for k in 6..=9 {
            let now = Vec2::from_angle(step * k as f32);
            sweep = wound(sweep, seen, now, false);
            seen = now;
        }
        assert_eq!(sweep, std::f32::consts::TAU);
        // Coming back unwinds from the cap.
        sweep = wound(sweep, seen, Vec2::from_angle(step * 8.0), false);
        assert!(sweep < std::f32::consts::TAU);
    }

    /// Through the center the direction flips by a half turn; the arc keeps
    /// going the way it was, and so it does after the pointer was away.
    #[test]
    fn an_ambiguous_turn_keeps_the_arcs_direction() {
        let sweep = 100f32.to_radians();
        let across = wound(sweep, Vec2::from_angle(sweep), Vec2::from_angle(sweep + std::f32::consts::PI), false);
        assert!((across - (sweep + std::f32::consts::PI)).abs() < 1e-4, "{}", across.to_degrees());
        let short = 30f32.to_radians();
        let back = wound(short, Vec2::from_angle(short), Vec2::from_angle(short - 2.0), true);
        assert!((back - (short + std::f32::consts::TAU - 2.0)).abs() < 1e-4, "{}", back.to_degrees());
        // A little behind where it left is taken as a step back.
        let nudge = wound(short, Vec2::from_angle(short), Vec2::from_angle(short - 0.1), true);
        assert!((nudge - (short - 0.1)).abs() < 1e-4, "{}", nudge.to_degrees());
    }

    /// The session winds as the pointer moves and the click places that
    /// sweep: a whole turn closes on the start, a snap onto the start short of
    /// one is ignored, and a click without moving places nothing.
    #[test]
    fn the_arc_tool_places_what_it_wound() {
        let mut sketch = Sketch::default();
        let center = sketch.add_point(Vec2::ZERO);
        let start = sketch.add_point(Vec2::new(0.02, 0.0));
        let mut session = SketchSession { tool: SketchTool::Arc, pending: vec![center, start], ..default() };
        for degrees in (30..=270).step_by(30) {
            session.cursor = Some(snapped(Vec2::from_angle((degrees as f32).to_radians()) * 0.02, None));
            wind_the_arc(&mut session, &sketch);
        }
        assert!((session.arc_sweep - 270f32.to_radians()).abs() < 1e-4);
        // Across the center: ignored while near it.
        session.cursor = Some(snapped(Vec2::ZERO, None));
        wind_the_arc(&mut session, &sketch);
        assert!(session.arc_lost);

        place_arc(&mut sketch, center, start, None, 270f32.to_radians());
        place_arc(&mut sketch, center, start, Some(start), std::f32::consts::TAU);
        place_arc(&mut sketch, center, start, Some(start), 10f32.to_radians());
        place_arc(&mut sketch, center, start, None, 1e-4);
        let sweeps: Vec<f32> = sketch.curves().into_iter().filter_map(|(_, curve, _)| match curve {
            Curve2::Arc { sweep, .. } => Some(sweep.to_degrees()),
            _ => None,
        }).collect();
        assert_eq!(sweeps.len(), 3, "{sweeps:?}");
        for (got, want) in sweeps.iter().zip([270.0, 360.0, 10.0]) {
            assert!((got - want).abs() < 0.01, "{sweeps:?}");
        }
    }

    /// The placed arc is the one previewed: 300 degrees clockwise stays
    /// 300 degrees, and a whole turn closes on its start.
    #[test]
    fn a_placed_arc_keeps_its_wound_sweep() {
        let mut sketch = Sketch::default();
        let center = sketch.add_point(Vec2::ZERO);
        let start = sketch.add_point(Vec2::new(0.02, 0.0));
        let Curve2::Arc { end, .. } = arc_by(Vec2::ZERO, Vec2::new(0.02, 0.0), -300f32.to_radians()) else { panic!() };
        let end = sketch.add_point(end);
        let arc = sketch.add_arc(center, start, end, true);
        let Some(Curve2::Arc { sweep, .. }) = sketch.curve(arc) else { panic!() };
        assert!((sweep + 300f32.to_radians()).abs() < 1e-4, "{}", sweep.to_degrees());

        let whole = sketch.add_arc(center, start, start, false);
        let Some(Curve2::Arc { sweep, .. }) = sketch.curve(whole) else { panic!() };
        assert!((sweep - std::f32::consts::TAU).abs() < 1e-4, "{}", sweep.to_degrees());

        let mut ring = Sketch::default();
        let (center, start) = (ring.add_point(Vec2::ZERO), ring.add_point(Vec2::new(0.02, 0.0)));
        ring.add_arc(center, start, start, false);
        assert_eq!(arrange(&ring).regions.len(), 1, "a whole turn encloses a region as a circle does");
    }

    /// Fails if the chain rule changes: every click continues from where it
    /// landed until a line ends on an existing point or encloses something.
    #[test]
    fn the_line_tool_chains_until_it_closes_a_shape() {
        let mut sketch = Sketch::default();
        let first = draw_to(&mut sketch, None, snapped(Vec2::ZERO, None)).unwrap();
        let mut chain = draw_to(&mut sketch, Some(first), snapped(Vec2::X, None));
        chain = draw_to(&mut sketch, chain, snapped(Vec2::ONE, None));
        assert_eq!(sketch.entities.len(), 2);
        let corner = chain.unwrap();
        assert_eq!(draw_to(&mut sketch, chain, snapped(Vec2::ONE, chain)), Some(corner), "clicking the end draws nothing");
        assert_eq!(sketch.entities.len(), 2);

        // Onto a point that encloses nothing new: the shape still ends there.
        let spur = draw_to(&mut sketch, chain, snapped(Vec2::new(2.0, 1.0), None));
        let back = draw_to(&mut sketch, spur, snapped(Vec2::ONE, Some(corner)));
        assert_eq!(back, None, "joining an existing point ends the chain");
        assert_eq!(sketch.entities.len(), 4);

        // Starting on an existing point is a start, not an end.
        let restarted = draw_to(&mut sketch, None, snapped(Vec2::ONE, Some(corner)));
        assert_eq!(restarted, Some(corner));
        chain = draw_to(&mut sketch, restarted, snapped(Vec2::ZERO, Some(first)));
        assert_eq!(chain, None, "closing the triangle ends the chain");
        assert_eq!(sketch.points.len(), 4, "closing reused the first point");
        assert_eq!(arrange(&sketch).regions.len(), 1);
    }

    /// Fails if a curve tool's points are left loose, or its curve is more
    /// than one undo step.
    #[test]
    fn finishing_a_curve_tool_makes_its_curve_in_one_step() {
        use crate::prop::plane::{OriginPlane, PlaneRef};
        let mut doc = PropDoc::new("test");
        let mut sketch = Sketch::default();
        let points: Vec<SketchId> = (0..4).map(|i| sketch.add_point(Vec2::new(i as f32 * 0.01, (i % 2) as f32 * 0.01))).collect();
        let id = doc.push(FeatureOp::Sketch { on: PlaneRef::Origin(OriginPlane::Xy), sketch });
        let mut editor = PropEditor::default();
        editor.open(doc, None);
        let mut session = SketchSession::default();
        session.enter(id);
        session.set_tool(SketchTool::Nurbs);
        session.pending = points.clone();
        assert!(session.can_finish_curve());
        session.finish_curve(&mut editor);
        let made = &sketch_of(editor.doc(), id).unwrap().entities;
        assert!(matches!(&made[..], [crate::prop::sketch::SketchEntity::Nurbs { degree: 3, points: p, .. }] if *p == points));
        assert!(session.pending.is_empty());
        editor.undo();
        assert!(sketch_of(editor.doc(), id).unwrap().entities.is_empty());
    }

    #[test]
    fn hatching_stays_inside_and_skips_the_hole() {
        let outer = vec![Vec2::ZERO, Vec2::new(4.0, 0.0), Vec2::new(4.0, 4.0), Vec2::new(0.0, 4.0)];
        let hole = vec![Vec2::ONE, Vec2::new(3.0, 1.0), Vec2::new(3.0, 3.0), Vec2::new(1.0, 3.0)];
        let lines = hatch(&outer, std::slice::from_ref(&hole), 0.25);
        assert!(!lines.is_empty());
        let inside = |p: Vec2| (-1e-4..=4.0001).contains(&p.x) && (-1e-4..=4.0001).contains(&p.y);
        for (a, b) in &lines {
            assert!(inside(*a) && inside(*b), "{a} {b}");
            let middle = (*a + *b) / 2.0;
            assert!(!(middle.x > 1.0 && middle.x < 3.0 && middle.y > 1.0 && middle.y < 3.0), "{middle} is in the hole");
        }
    }

    #[test]
    fn removing_a_point_takes_its_lines() {
        let mut sketch = Sketch::default();
        let a = sketch.add_point(Vec2::ZERO);
        let b = sketch.add_point(Vec2::X);
        let c = sketch.add_point(Vec2::Y);
        sketch.add_line(a, b);
        let keep = sketch.add_line(b, c);
        sketch.remove(&[a]);
        assert_eq!(sketch.points.len(), 2);
        assert_eq!(sketch.entities.iter().map(|e| e.id()).collect::<Vec<_>>(), vec![keep]);
    }
}
