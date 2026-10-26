//! Drawing a footprint room: corners clicked on the floor plane, chained into
//! a loop, closed on the first. See `documentation/footprint-rooms.md`.

use bevy::app::App;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;

use crate::common::app_mode::AppMode;
use crate::editor::editable::{Feature, FeatureId, FeatureTimeline, FeatureTrait, PointRef};
use crate::editor::footprint_room::{FootprintRoom, DEFAULT_HEIGHT};
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::prop::sketch_edit::hatch;
use crate::tool::room::Room;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

const SNAP: f32 = 0.1;

/// How near the first corner, in metres across the floor, a click closes the
/// loop on it rather than adding a corner beside it.
const CLOSE_RADIUS: f32 = 0.3;

/// Hatching across a loop that would close into a room, in metres.
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
    /// Where each corner is across the floor, and what it is measured from.
    corners: Vec<(Vec2, PointRef)>,
}

impl Draft {
    pub fn floor(&self) -> Option<f32> {
        self.floor
    }

    pub fn is_empty(&self) -> bool {
        self.corners.is_empty()
    }

    pub fn push(&mut self, at: Vec3, point: PointRef) {
        self.floor.get_or_insert(at.y);
        self.corners.push((Vec2::new(at.x, at.z), point));
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

    /// The loop as a footprint feature with its one region named. `None`
    /// with fewer than three corners.
    pub fn finish(&self, height: f32, features: &HashMap<FeatureId, Feature>) -> Option<FootprintRoom> {
        let floor = self.floor?;
        if self.corners.len() < 3 {
            return None;
        }
        let first = self.corners[0].0;
        let mut room = FootprintRoom::new(PointRef::absolute(first.x, floor, first.y), height);
        let ids: Vec<_> = self.corners.iter().map(|(_, point)| room.add_corner(point.clone())).collect();
        for i in 0..ids.len() {
            room.add_line(ids[i], ids[(i + 1) % ids.len()]);
        }
        room.resolve_references(features);
        let inside = Room::prism(self.points().collect(), floor, floor + height).floor_point();
        // A loop that crosses itself still names whichever piece that point
        // is in; the rest stays a line the inspector can point at.
        room.name_region_at(Vec2::new(inside.x, inside.z));
        room.resolve_references(features);
        Some(room)
    }
}

#[derive(Resource)]
struct FootprintTool {
    draft: Draft,
    /// Where the next corner would go, on the floor plane.
    cursor: Option<Vec3>,
    /// A feature's point under the pointer while Shift is held.
    hovered_point: Option<(FeatureId, String, Vec3)>,
    /// What the last one was stood up to, for the next.
    height: f32,
}

impl Default for FootprintTool {
    fn default() -> Self {
        Self { draft: Draft::default(), cursor: None, hovered_point: None, height: DEFAULT_HEIGHT }
    }
}

impl FootprintTool {
    fn on_exit(mut tool: ResMut<Self>) {
        tool.draft = Draft::default();
        tool.cursor = None;
        tool.hovered_point = None;
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
        let Some(floor) = self.draft.floor() else {
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
    ) {
        let typing = egui.wants_keyboard_input();
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

        let placed = if shift_held {
            tool.hovered_point = mouse_input
                .world_pos
                .and_then(|ray| find_hovered_point(&ray, &features, PICK_RADIUS));
            match (&tool.hovered_point, mouse_input.released) {
                (Some((feature, key, at)), Some(MouseButton::Left)) => {
                    let mut point = PointRef::reference(*feature);
                    point.point_key = key.clone();
                    Some((*at, point))
                }
                _ => None,
            }
        } else {
            tool.hovered_point = None;
            tool.cursor
                .filter(|_| commit_at_cursor(&mouse_input, &keys, typing))
                .map(|at| (at, PointRef::absolute(at.x, at.y, at.z)))
        };
        let Some((at, point)) = placed else { return };

        if tool.draft.closes_at(Vec2::new(at.x, at.z)) {
            let height = tool.height;
            if let Some(room) = tool.draft.finish(height, features.features_map()) {
                let id = features.apply_feature(Box::new(room));
                features.select(Some(id));
            }
            tool.draft = Draft::default();
            next_tool.set(Tools::Select);
        } else {
            tool.draft.push(at, point);
        }
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
            draw_picking_gizmos(&mut gizmos, &ray, &features, &tool.hovered_point);
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

    fn at(x: f32, z: f32) -> Vec3 {
        Vec3::new(x, 1.5, z)
    }

    fn drawn(points: &[(f32, f32)]) -> Draft {
        let mut draft = Draft::default();
        for (x, z) in points {
            draft.push(at(*x, *z), PointRef::absolute(*x, 1.5, *z));
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
        draft.push(Vec3::new(5.0, 1.5, 1.0), PointRef::reference(anchor));
        draft.push(at(0.0, 4.0), PointRef::absolute(0.0, 1.5, 4.0));
        let room = draft.finish(3.0, timeline.features_map()).unwrap();
        assert_eq!(room.parent_ids(), vec![anchor]);
        assert_eq!(room.rooms().len(), 1, "{:?}", room.problems());
    }

    #[test]
    fn two_corners_are_not_a_room() {
        assert!(drawn(&[(0.0, 0.0), (4.0, 0.0)]).finish(3.0, &HashMap::new()).is_none());
    }
}
