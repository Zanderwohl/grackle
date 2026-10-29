//! Drawing a footprint room: corners clicked on the floor plane, chained into
//! a loop, closed on the first. With a footprint selected, the drawing is
//! added to it instead: corners snap onto its own, and a chain may end on one.
//! See `documentation/footprint-rooms.md`.

use bevy::app::App;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;

use crate::common::app_mode::AppMode;
use crate::editor::editable::{EditEvent, Feature, FeatureId, FeatureTimeline, PointRef};
use crate::editor::footprint_room::{ChainCorner, FootprintRoom, DEFAULT_HEIGHT};
use crate::prop::sketch::SketchId;
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::prop::sketch_edit::hatch;
use crate::tool::room::Room;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

const SNAP: f32 = 0.1;

/// How near a corner, in meters across the floor, a click lands on it — the
/// first, to close the loop, or one of the footprint's own — rather than
/// beside it.
const CLOSE_RADIUS: f32 = 0.3;

/// Hatching across a loop that would close into a room, in meters.
const HATCH_SPACING: f32 = 0.5;

pub struct FootprintPlugin;

impl Plugin for FootprintPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FootprintTool>()
            .add_systems(
                Update,
                (FootprintTool::interface, FootprintTool::draw_gizmos)
                    .chain()
                    .run_if(in_state(Tools::Footprint))
                    .run_if(in_state(AppMode::Editor)),
            )
            .add_systems(OnExit(Tools::Footprint), FootprintTool::on_exit);
    }
}

/// The loop being drawn. Plain data, so what a click does is testable without
/// a mouse.
#[derive(Default, Clone)]
pub struct Draft {
    /// Set by the first corner; every later one is on its plane.
    floor: Option<f32>,
    /// Where each corner is across the floor, and what it is.
    corners: Vec<(Vec2, ChainCorner)>,
}

impl Draft {
    pub fn floor(&self) -> Option<f32> {
        self.floor
    }

    pub fn is_empty(&self) -> bool {
        self.corners.is_empty()
    }

    pub fn push(&mut self, at: Vec3, corner: ChainCorner) {
        self.floor.get_or_insert(at.y);
        self.corners.push((Vec2::new(at.x, at.z), corner));
    }

    /// Whether adding the footprint's own corner `id` would add nothing: it is
    /// already the last.
    fn ends_on(&self, id: SketchId) -> bool {
        matches!(self.corners.last(), Some((_, ChainCorner::Existing(last))) if *last == id)
    }

    fn chain(&self) -> Vec<ChainCorner> {
        self.corners.iter().map(|(_, corner)| corner.clone()).collect()
    }

    /// Takes the last corner back; the floor goes with the first.
    pub fn pop(&mut self) {
        self.corners.pop();
        if self.corners.is_empty() {
            self.floor = None;
        }
    }

    /// Whether a corner put at `at` would close the loop instead.
    pub fn closes_at(&self, at: Vec2) -> bool {
        self.corners.len() >= 3 && self.corners[0].0.distance(at) <= CLOSE_RADIUS
    }

    pub fn points(&self) -> impl Iterator<Item = Vec2> + '_ {
        self.corners.iter().map(|(at, _)| *at)
    }

    /// The loop as a new footprint, its regions named by the same rule as a
    /// chain added to an existing one. `None` with fewer than three corners.
    pub fn finish(&self, height: f32, features: &HashMap<FeatureId, Feature>) -> Option<FootprintRoom> {
        let floor = self.floor?;
        if self.corners.len() < 3 {
            return None;
        }
        let first = self.corners[0].0;
        let mut room = FootprintRoom::new(PointRef::absolute(first.x, floor, first.y), height);
        room.add_chain(&self.chain(), true, features);
        Some(room)
    }
}

#[derive(Resource)]
struct FootprintTool {
    draft: Draft,
    /// The footprint being drawn onto, if one was selected.
    target: Option<FeatureId>,
    /// Its floor and corners as they stood when the drawing started.
    target_floor: Option<f32>,
    target_corners: Vec<(SketchId, Vec3)>,
    /// Where the next corner would go, on the floor plane.
    cursor: Option<Vec3>,
    /// A feature's point under the pointer while Shift is held.
    hovered_point: Option<(FeatureId, String, Vec3)>,
    /// What the last one was stood up to, for the next.
    height: f32,
}

impl Default for FootprintTool {
    fn default() -> Self {
        Self {
            draft: Draft::default(),
            target: None,
            target_floor: None,
            target_corners: Vec::new(),
            cursor: None,
            hovered_point: None,
            height: DEFAULT_HEIGHT,
        }
    }
}

impl FootprintTool {
    fn on_exit(mut tool: ResMut<Self>) {
        tool.draft = Draft::default();
        tool.target = None;
        tool.cursor = None;
        tool.hovered_point = None;
    }

    /// Drawing onto a footprint, only what stands before it: a corner on it,
    /// after it, or on a Mirror's copy of it would be a cycle.
    fn pickable(&self, features: &FeatureTimeline) -> Vec<FeatureId> {
        let active = features.active_features().map(|(id, _)| id);
        match self.target {
            Some(target) => active.take_while(|id| *id != target).collect(),
            None => active.collect(),
        }
    }

    /// Between drawings, whatever footprint is selected is what the next one
    /// goes onto.
    fn aim_at_selection(&mut self, features: &FeatureTimeline) {
        if !self.draft.is_empty() {
            return;
        }
        let footprint = features
            .selected_feature()
            .and_then(|id| features.get_feature(&id).and_then(|f| f.object().footprint()).map(|f| (id, f)));
        self.target = footprint.map(|(id, _)| id);
        self.target_floor = footprint.map(|(_, f)| f.floor_height());
        self.target_corners = footprint.map(|(_, f)| f.corner_points().collect()).unwrap_or_default();
    }

    /// The footprint's own corner near `at`, if there is one.
    fn target_corner_near(&self, at: Vec2) -> Option<(SketchId, Vec3)> {
        self.target_corners.iter().copied().find(|(_, p)| Vec2::new(p.x, p.z).distance(at) <= CLOSE_RADIUS)
    }

    /// Where the pointer is on the floor plane. Before the first corner there
    /// is no plane yet, so it is wherever a click would land on the map's
    /// walls, as for the Room tool. A side view looking along the plane keeps
    /// the axis it cannot see from the last corner.
    fn floor_cursor(
        &self,
        mouse_input: &CurrentMouseInput,
        cameras: &Query<(Entity, &Multicam)>,
        rooms: &Query<&Room>,
    ) -> Option<Vec3> {
        let Some(floor) = self.draft.floor().or(self.target_floor) else {
            let start = Vec3::ZERO;
            return compute_cursor(mouse_input, cameras, start, true, SNAP, rooms);
        };
        let ray = mouse_input.world_pos?;
        let direction = Vec3::from(ray.direction);
        let last = self.draft.points().last().unwrap_or(Vec2::ZERO);
        let on_plane = if direction.y.abs() > 0.05 {
            let t = (floor - ray.origin.y) / direction.y;
            if t < 0.0 {
                return None;
            }
            ray.origin + direction * t
        } else if direction.x.abs() > direction.z.abs() {
            Vec3::new(last.x, floor, ray.origin.z)
        } else {
            Vec3::new(ray.origin.x, floor, last.y)
        };
        let snapped = snap_vec3(on_plane, SNAP);
        Some(Vec3::new(snapped.x, floor, snapped.z))
    }

    fn interface(
        mut tool: ResMut<Self>,
        cameras: Query<(Entity, &Multicam)>,
        mouse_input: Res<CurrentMouseInput>,
        keys: Res<ButtonInput<KeyCode>>,
        mut features: ResMut<FeatureTimeline>,
        rooms: Query<&Room>,
        mut next_tool: ResMut<NextState<Tools>>,
        egui: Res<EguiWantsInput>,
        mut commands: Commands,
        mut edit_events: MessageWriter<EditEvent>,
    ) {
        let typing = egui.wants_keyboard_input();
        tool.aim_at_selection(&features);
        if !typing && keys.just_pressed(KeyCode::Escape) {
            tool.draft = Draft::default();
            return;
        }
        if !typing && keys.just_pressed(KeyCode::Backspace) {
            tool.draft.pop();
            return;
        }

        tool.cursor = tool.floor_cursor(&mouse_input, &cameras, &rooms);
        let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

        // With Shift, a feature's point under the pointer; with none there,
        // Shift changes nothing and the click goes on the grid.
        let pickable = tool.pickable(&features);
        tool.hovered_point = mouse_input
            .world_pos
            .filter(|_| shift_held)
            .and_then(|ray| find_hovered_point_filtered(&ray, &features, PICK_RADIUS, &pickable));
        let placed = match tool.hovered_point.clone() {
            Some((feature, key, at)) => (mouse_input.released == Some(MouseButton::Left)).then(|| {
                let mut point = PointRef::reference(feature);
                point.point_key = key;
                (at, ChainCorner::New(point))
            }),
            None => {
                let own = tool.cursor.and_then(|at| tool.target_corner_near(Vec2::new(at.x, at.z)));
                if let Some((_, at)) = own {
                    tool.cursor = Some(at);
                }
                tool.cursor.filter(|_| commit_at_cursor(&mouse_input, &keys, typing)).map(|at| match own {
                    Some((id, at)) => (at, ChainCorner::Existing(id)),
                    None => (at, ChainCorner::New(PointRef::absolute(at.x, at.y, at.z))),
                })
            }
        };
        let Some((at, corner)) = placed else { return };

        let closes = tool.draft.closes_at(Vec2::new(at.x, at.z));
        // Onto a corner the footprint already has, a chain is joined to it and
        // done: a line across a room, or a wing out and back.
        let joins = match (&corner, tool.target) {
            (ChainCorner::Existing(id), Some(_)) => !tool.draft.is_empty() && !closes && !tool.draft.ends_on(*id),
            _ => false,
        };
        if !closes && !joins {
            if !matches!(&corner, ChainCorner::Existing(id) if tool.draft.ends_on(*id)) {
                tool.draft.push(at, corner);
            }
            return;
        }
        if joins {
            tool.draft.push(at, corner);
        }

        match tool.target {
            Some(target) => {
                if let Some(mut feature) = features.features_mut().remove(&target) {
                    if let Some(footprint) = feature.object_mut().footprint_mut() {
                        footprint.add_chain(&tool.draft.chain(), closes, features.features_map());
                    }
                    if let Some(entity) = feature.object().entity() {
                        feature.object_mut().apply_to_entity(&mut commands, entity);
                        edit_events.write(EditEvent { editor_id: target._id(), feature_id: target, entity });
                    }
                    features.features_mut().insert(target, feature);
                }
            }
            None => {
                if let Some(room) = tool.draft.finish(tool.height, features.features_map()) {
                    let id = features.apply_feature(Box::new(room));
                    features.select(Some(id));
                }
            }
        }
        tool.draft = Draft::default();
        next_tool.set(Tools::Select);
    }

    fn draw_gizmos(
        tool: Res<FootprintTool>,
        features: Res<FeatureTimeline>,
        mouse_input: Res<CurrentMouseInput>,
        mut gizmos: Gizmos,
    ) {
        let drawn = Color::srgb_u8(200, 200, 120);
        let pending = Color::srgb_u8(60, 120, 255);
        let closing = Color::srgb_u8(100, 255, 100);

        if let Some(ray) = mouse_input.world_pos.filter(|_| tool.hovered_point.is_some()) {
            draw_picking_gizmos_filtered(&mut gizmos, &ray, &features, &tool.hovered_point, &tool.pickable(&features));
        }
        for (_, at) in &tool.target_corners {
            gizmos.sphere(Isometry3d::from_translation(*at), 0.08, drawn);
        }

        let Some(floor) = tool.draft.floor() else {
            if let Some(cursor) = tool.cursor {
                gizmos.sphere(Isometry3d::from_translation(cursor), 0.15, pending);
            }
            return;
        };
        let at = |p: Vec2| Vec3::new(p.x, floor, p.y);
        let points: Vec<Vec2> = tool.draft.points().collect();
        for pair in points.windows(2) {
            gizmos.line(at(pair[0]), at(pair[1]), drawn);
        }

        let next = tool.hovered_point.as_ref().map(|(_, _, p)| *p).or(tool.cursor);
        let Some(next) = next else { return };
        let next = Vec2::new(next.x, next.z);
        let closes = tool.draft.closes_at(next);
        if let Some(last) = points.last() {
            dashed_line(&mut gizmos, at(*last), at(if closes { points[0] } else { next }), pending, 0.15, 0.1);
        }
        gizmos.sphere(Isometry3d::from_translation(at(points[0])), 0.15, if closes { closing } else { drawn });
        gizmos.sphere(Isometry3d::from_translation(at(next)), 0.12, pending);

        // What would be a room if the loop closed here.
        let mut outline = points.clone();
        if !closes {
            outline.push(next);
        }
        if outline.len() >= 3 {
            if let Some(last) = outline.last() {
                dashed_line(&mut gizmos, at(*last), at(outline[0]), pending, 0.15, 0.1);
            }
            for (a, b) in hatch(&outline, &[], HATCH_SPACING) {
                gizmos.line(at(a), at(b), if closes { closing } else { pending.with_alpha(0.4) });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::editable::FeatureTrait;

    fn at(x: f32, z: f32) -> Vec3 {
        Vec3::new(x, 1.5, z)
    }

    fn drawn(points: &[(f32, f32)]) -> Draft {
        let mut draft = Draft::default();
        for (x, z) in points {
            draft.push(at(*x, *z), ChainCorner::New(PointRef::absolute(*x, 1.5, *z)));
        }
        draft
    }

    #[test]
    fn the_first_corner_sets_the_floor() {
        let draft = drawn(&[(0.0, 0.0), (4.0, 0.0)]);
        assert_eq!(draft.floor(), Some(1.5));
    }

    #[test]
    fn a_click_by_the_first_corner_closes_only_once_there_is_a_shape() {
        let two = drawn(&[(0.0, 0.0), (4.0, 0.0)]);
        assert!(!two.closes_at(Vec2::new(0.1, 0.1)), "two corners closed into nothing");
        let three = drawn(&[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0)]);
        assert!(three.closes_at(Vec2::new(0.1, 0.1)));
        assert!(!three.closes_at(Vec2::new(1.0, 0.0)));
    }

    #[test]
    fn taking_back_the_last_corner_takes_the_floor_with_the_first() {
        let mut draft = drawn(&[(0.0, 0.0), (4.0, 0.0)]);
        draft.pop();
        assert_eq!(draft.points().count(), 1);
        draft.pop();
        assert!(draft.is_empty());
        assert_eq!(draft.floor(), None);
    }

    #[test]
    fn a_closed_loop_is_a_footprint_with_its_room_named() {
        let draft = drawn(&[(0.0, 0.0), (6.0, 0.0), (6.0, 2.0), (2.0, 2.0), (2.0, 6.0), (0.0, 6.0)]);
        let room = draft.finish(2.5, &HashMap::new()).expect("no feature");
        assert_eq!(room.rooms().len(), 1, "{:?}", room.problems());
        let made = &room.rooms()[0];
        assert!((made.volume() - 20.0 * 2.5).abs() < 1e-4);
        assert_eq!((made.floor, made.ceiling), (1.5, 4.0));
        assert!(room.problems().is_empty(), "{:?}", room.problems());
    }

    #[test]
    fn a_loop_drawn_the_other_way_round_is_the_same_room() {
        let draft = drawn(&[(0.0, 6.0), (2.0, 6.0), (2.0, 2.0), (6.0, 2.0), (6.0, 0.0), (0.0, 0.0)]);
        let room = draft.finish(2.5, &HashMap::new()).unwrap();
        assert!((room.rooms()[0].volume() - 20.0 * 2.5).abs() < 1e-4);
    }

    #[test]
    fn a_corner_put_on_a_features_point_is_measured_from_it() {
        use crate::editor::global_point::GlobalPoint;
        let mut timeline = FeatureTimeline::default();
        let anchor = timeline.apply_feature(Box::new(GlobalPoint::new(5.0, 1.5, 1.0)));
        let mut draft = drawn(&[(0.0, 0.0), (4.0, 0.0)]);
        draft.push(Vec3::new(5.0, 1.5, 1.0), ChainCorner::New(PointRef::reference(anchor)));
        draft.push(at(0.0, 4.0), ChainCorner::New(PointRef::absolute(0.0, 1.5, 4.0)));
        let room = draft.finish(3.0, timeline.features_map()).unwrap();
        assert_eq!(room.parent_ids(), vec![anchor]);
        assert_eq!(room.rooms().len(), 1, "{:?}", room.problems());
    }

    #[test]
    fn two_corners_are_not_a_room() {
        assert!(drawn(&[(0.0, 0.0), (4.0, 0.0)]).finish(3.0, &HashMap::new()).is_none());
    }

    /// A corner on anything at or after the footprint, a Mirror copying it included, is a cycle.
    #[test]
    fn drawing_onto_a_footprint_offers_only_what_stands_before_it() {
        use crate::editor::global_point::GlobalPoint;
        use crate::editor::mirror::Mirror;

        let mut timeline = FeatureTimeline::default();
        let before = timeline.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
        let footprint = timeline.apply_feature(Box::new(FootprintRoom::new(PointRef::absolute(0.0, 0.0, 0.0), 3.0)));
        timeline.apply_feature(Box::new(Mirror::new(PointRef::absolute(0.0, 0.0, 0.0)).with_sources(vec![footprint])));

        let mut tool = FootprintTool::default();
        assert_eq!(tool.pickable(&timeline).len(), 3, "a new footprint goes in after everything");
        tool.target = Some(footprint);
        assert_eq!(tool.pickable(&timeline), vec![before]);
    }
}
