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

use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;

use crate::common::app_mode::AppMode;
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::{CameraAxis, Multicam};
use crate::prop::camera::OrbitPivot;
use crate::prop::document::{PropDoc, PropEditor};
use crate::prop::feature::{FeatureOp, PropFeatureId};
use crate::prop::gizmo::HoveredPick;
use crate::prop::reference::{Located, Pickable, Pointer, EDGE_PICK_PX, POINT_PICK_PX};
use crate::prop::region::{arrange, crossing, Arrangement};
use crate::prop::sketch::{PlacedSketch, Sketch, SketchId};
use crate::prop::view::PropBuild;
use crate::tool::tool_helpers::dashed_line;

/// Metres between grid lines, and what an unsnapped point rounds to.
pub const GRID: f32 = 0.005;

/// Pixels within which the cursor lands on a point rather than the grid.
const SNAP_PX: f32 = 10.0;

/// Pixels a press must travel before it is a drag rather than a click.
const DRAG_PX: f32 = 3.0;

/// Grid lines each way past which the grid is not drawn: a sketch the size of
/// a cannon at five millimetres would be thousands of lines.
const MAX_GRID_LINES: i32 = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SketchTool {
    #[default]
    Select,
    Line,
    /// Takes away the piece of a line between the junctions either side of a
    /// click, or every piece a drag passes through.
    Erase,
}

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
    view: ViewChange,
    /// The perspective camera and pivot as they were before the view was
    /// turned onto the sketch.
    saved: Option<(Entity, Transform, Projection, Vec3)>,
}

impl SketchSession {
    pub fn editing(&self) -> Option<PropFeatureId> {
        self.editing
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
    }

    /// Picked again, it is unpicked.
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

/// The sketch feature `id`'s drawing, if it is one.
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
        app.init_resource::<SketchSession>()
            .add_systems(
                Update,
                (keep_the_session, aim_the_view, edit_the_sketch, draw_the_session)
                    .chain()
                    .run_if(in_state(AppMode::Prop)),
            )
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
}

/// Turns the perspective view to look straight down the sketch's plane, with
/// the plane's `y` up, and puts it back on leaving. Waits for the sketch to be
/// evaluated, which a sketch made this frame is not yet.
fn aim_the_view(
    mut session: ResMut<SketchSession>,
    editor: Res<PropEditor>,
    build: Res<PropBuild>,
    mut pivot: ResMut<OrbitPivot>,
    mut cameras: Query<(Entity, &mut Transform, &mut Projection, &Multicam)>,
) {
    match session.view {
        ViewChange::None => {}
        ViewChange::Restore => {
            if let Some((entity, transform, projection, centre)) = session.saved.take()
                && let Ok((_, mut current, mut current_projection, _)) = cameras.get_mut(entity)
            {
                *current = transform;
                *current_projection = projection;
                pivot.0 = centre;
            }
            session.view = ViewChange::None;
        }
        ViewChange::Aim => {
            let Some(placed) = live(&session, &editor, &build) else { return };
            let (centre, radius) = extent(&placed.sketch);
            let centre = placed.plane.lift(centre);
            for (entity, mut transform, projection, multicam) in &mut cameras {
                if multicam.axis != CameraAxis::None {
                    continue;
                }
                if session.saved.is_none() {
                    session.saved = Some((entity, *transform, projection.clone(), pivot.0));
                }
                *transform = Transform::from_translation(centre + *placed.plane.normal * radius * 3.2)
                    .looking_at(centre, *placed.plane.y());
            }
            pivot.0 = centre;
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
    keys: Res<ButtonInput<KeyCode>>,
    egui: Res<EguiWantsInput>,
    cameras: Query<(&Camera, &GlobalTransform), With<Multicam>>,
) {
    let Some(id) = session.editing else { return };

    // Before anything that needs the pointer over a viewport: a drag that
    // ends over a panel still ends.
    let held = mouse.pressed == Some(MouseButton::Left);
    if !held && let Some(press) = session.press.take() {
        let stroke = std::mem::take(&mut session.stroke);
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

    let typing = egui.wants_keyboard_input();
    if !typing && keys.just_pressed(KeyCode::Escape) {
        session.chain = None;
    }
    // Not mid-press, which would strand an eraser stroke or a drag, and not
    // onto the tool already up, since changing tool ends a line chain.
    let chosen = [(KeyCode::KeyL, SketchTool::Line), (KeyCode::KeyK, SketchTool::Erase)]
        .into_iter()
        .find(|(key, _)| keys.just_pressed(*key))
        .map(|(_, tool)| tool);
    if let Some(tool) = chosen
        && !typing
        && session.press.is_none()
        && session.tool != tool
    {
        session.set_tool(tool);
    }
    if !typing && keys.any_just_pressed([KeyCode::Delete, KeyCode::Backspace]) && !session.picks.is_empty() {
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

/// One click of the line tool: lands on `target`, drawing from `chain` if
/// there is one, and returns where the next click draws from. Landing on a
/// point that was already there closes what was being drawn, so the chain
/// ends; clicking the chain's own end again just ends it.
fn draw_to(sketch: &mut Sketch, chain: Option<SketchId>, target: Snapped) -> Option<SketchId> {
    let landed = target.point.unwrap_or_else(|| sketch.add_point(target.at));
    match chain {
        None => Some(landed),
        Some(from) if from == landed => None,
        Some(from) => {
            sketch.add_line(from, landed);
            target.point.is_none().then_some(landed)
        }
    }
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

fn line_under<P: Fn(Vec3) -> Option<Vec2>>(placed: &PlacedSketch, pointer: &Pointer<P>) -> Option<SketchId> {
    placed
        .sketch
        .lines()
        .filter_map(|(id, a, b, _)| {
            let (a, b) = (placed.sketch.point(a)?, placed.sketch.point(b)?);
            Some((id, pointer.to_segment(placed.world(a), placed.world(b))?))
        })
        .filter(|(_, pixels)| *pixels <= EDGE_PICK_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// The pieces the eraser would take: the one under a click, or every one a
/// drag has crossed so far.
fn doomed(session: &SketchSession, sketch: &Sketch) -> Vec<(Vec2, Vec2)> {
    let piece = |line: SketchId, at: Vec2| {
        let (a, b) = sketch.line(line)?;
        let (t0, t1) = sketch.piece_of(line, at)?;
        Some((a.lerp(b, t0), a.lerp(b, t1)))
    };
    let dragging = session.press.is_some_and(|press| press.dragging);
    if !dragging {
        let line = match session.press.map(|press| press.on).unwrap_or(session.hovered) {
            Some(SketchPick::Line(line)) => line,
            _ => return vec![],
        };
        let at = session.stroke.first().copied().or(session.on_plane);
        return at.and_then(|at| piece(line, at)).into_iter().collect();
    }
    let mut pieces = Vec::new();
    for step in session.stroke.windows(2) {
        for (line, ..) in sketch.lines() {
            let Some((a, b)) = sketch.line(line) else { continue };
            if let Some((_, _, at)) = crossing(a, b, step[0], step[1]) {
                pieces.extend(piece(line, at));
            }
        }
    }
    pieces
}

const POINT: Color = Color::srgb(0.92, 0.92, 0.98);
const PICKED: Color = Color::srgb(1.0, 0.8, 0.2);
const HOVERED: Color = Color::srgb(0.5, 0.9, 1.0);
const GRID_COLOUR: Color = Color::srgba(1.0, 1.0, 1.0, 0.06);
const REGION: Color = Color::srgba(0.55, 0.8, 0.95, 0.35);
const DOOMED: Color = Color::srgb(1.0, 0.35, 0.3);

#[derive(Default)]
struct RegionCache {
    from: Option<Sketch>,
    arrangement: Arrangement,
}

/// The grid, every point and line, what is picked and hovered, the line about
/// to be drawn, and each closed region hatched — so a closed shape is visibly
/// closed. From the document rather than the build, so nothing lags a drag.
fn draw_the_session(
    session: Res<SketchSession>,
    editor: Res<PropEditor>,
    build: Res<PropBuild>,
    mut cache: Local<RegionCache>,
    mut gizmos: Gizmos,
) {
    let Some(placed) = live(&session, &editor, &build) else { return };
    let (centre, reach) = extent(&placed.sketch);
    let marker = reach * 0.02;

    // Snapped to the grid's own lines, so the grid does not slide as the
    // sketch grows.
    let half = ((reach * 1.6 / GRID).ceil() as i32).min(MAX_GRID_LINES);
    let middle = (centre / GRID).round() * GRID;
    for i in -half..=half {
        let offset = i as f32 * GRID;
        let span = half as f32 * GRID;
        gizmos.line(
            placed.world(middle + Vec2::new(offset, -span)),
            placed.world(middle + Vec2::new(offset, span)),
            GRID_COLOUR,
        );
        gizmos.line(
            placed.world(middle + Vec2::new(-span, offset)),
            placed.world(middle + Vec2::new(span, offset)),
            GRID_COLOUR,
        );
    }

    if cache.from.as_ref() != Some(&placed.sketch) {
        cache.arrangement = arrange(&placed.sketch);
        cache.from = Some(placed.sketch.clone());
    }
    for region in &cache.arrangement.regions {
        for (a, b) in hatch(&region.outer, &region.holes, reach / 12.0) {
            gizmos.line(placed.world(a), placed.world(b), REGION);
        }
    }

    let colour_of = |pick: SketchPick| {
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
        let colour = match session.chain == Some(point.id) {
            true => HOVERED,
            false => colour_of(pick).unwrap_or(POINT),
        };
        let at = placed.world(Vec2::from_array(point.at));
        gizmos.sphere(Isometry3d::from_translation(at), marker, colour);
    }
    for (id, a, b, construction) in placed.sketch.lines() {
        let (Some(a), Some(b)) = (placed.sketch.point(a), placed.sketch.point(b)) else { continue };
        let colour = colour_of(SketchPick::Line(id)).unwrap_or(POINT);
        let (a, b) = (placed.world(a), placed.world(b));
        match construction {
            true => dashed_line(&mut gizmos, a, b, colour, marker * 2.0, marker),
            false => {
                gizmos.line(a, b, colour);
            }
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

    // Where the next click lands, and the line it would draw. Landing on a
    // point that is there is ringed, since that click closes the shape.
    if session.tool == SketchTool::Line
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

/// Diagonal lines across a region with holes, `spacing` apart: what the
/// region looks like filled, drawn with lines. Even-odd along each line, so
/// holes come out by themselves.
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

    /// Fails if the chain rule changes: a new point continues, an existing
    /// one closes, the chain's own end stops.
    #[test]
    fn the_line_tool_chains_and_closes() {
        let mut sketch = Sketch::default();
        let mut chain = draw_to(&mut sketch, None, snapped(Vec2::ZERO, None));
        let first = chain.unwrap();
        chain = draw_to(&mut sketch, chain, snapped(Vec2::X, None));
        chain = draw_to(&mut sketch, chain, snapped(Vec2::ONE, None));
        assert_eq!(sketch.entities.len(), 2);
        assert!(chain.is_some());

        chain = draw_to(&mut sketch, chain, snapped(Vec2::ZERO, Some(first)));
        assert_eq!(chain, None, "landing on a point that was there closes the shape");
        assert_eq!(sketch.entities.len(), 3);
        assert_eq!(sketch.points.len(), 3, "closing reused the first point");
        assert_eq!(arrange(&sketch).regions.len(), 1);

        let again = draw_to(&mut sketch, None, snapped(Vec2::new(2.0, 0.0), None));
        assert_eq!(draw_to(&mut sketch, again, snapped(Vec2::new(2.0, 0.0), again)), None);
        assert_eq!(sketch.entities.len(), 3, "clicking the chain's end draws nothing");
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
