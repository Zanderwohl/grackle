//! Drawing a footprint room: corners clicked on the floor plane, chained into
//! a loop, closed on the first, picked by [`crate::tool::map_wizard`]. Every drawing is a footprint of its own, of
//! air under `Tools::Footprint` and solid under `Tools::FootprintBlock`.
//! See `documentation/footprint-rooms.md`.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::editor::editable::{Feature, FeatureId, FeatureTimeline, FeatureTrait, PointRef};
use crate::editor::footprint_room::{ChainCorner, FootprintRoom, DEFAULT_HEIGHT, LINE_COLOR};
use crate::editor::input::CurrentMouseInput;
use crate::prop::sketch_edit::hatch;
use crate::tool::map_wizard::MapPick;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

const SNAP: f32 = 0.1;

/// How near the first corner, in meters across the floor, a click closes the
/// loop rather than landing beside it.
const CLOSE_RADIUS: f32 = 0.3;

/// Hatching across a loop that would close into a room, in meters.
const HATCH_SPACING: f32 = 0.5;

fn made_by(tool: &Tools, room: FootprintRoom) -> Box<dyn FeatureTrait> {
    crate::editor::transmogrify::footprint_as(tool, room)
}

/// The loop being drawn. Plain data, so what a click does is testable without
/// a mouse.
#[derive(Default, Clone)]
pub struct Draft {
    /// Set by the first corner; every later one is on its plane.
    floor: Option<f32>,
    /// Where each corner is across the floor, and what it is.
    corners: Vec<(Vec2, PointRef)>,
}

impl Draft {
    pub fn floor(&self) -> Option<f32> {
        self.floor
    }

    pub fn is_empty(&self) -> bool {
        self.corners.is_empty()
    }

    pub fn push(&mut self, at: Vec3, corner: PointRef) {
        self.floor.get_or_insert(at.y);
        self.corners.push((Vec2::new(at.x, at.z), corner));
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

    /// The loop as a new footprint with its region named. `None` with fewer
    /// than three corners.
    pub fn finish(&self, height: f32, features: &HashMap<FeatureId, Feature>) -> Option<FootprintRoom> {
        let floor = self.floor?;
        if self.corners.len() < 3 {
            return None;
        }
        let first = self.corners[0].0;
        let mut room = FootprintRoom::new(PointRef::absolute(first.x, floor, first.y), height);
        let chain: Vec<_> = self.corners.iter().map(|(_, corner)| ChainCorner::New(corner.clone())).collect();
        room.add_chain(&chain, true, features);
        Some(room)
    }
}

/// Where the pointer is on the floor plane at height `floor`. A side view
/// looking along the plane keeps the axis it cannot see from `last`, the last
/// corner.
pub fn floor_cursor(floor: f32, last: Vec3, mouse_input: &CurrentMouseInput) -> Option<Vec3> {
    let ray = mouse_input.world_pos?;
    let direction = Vec3::from(ray.direction);
    let on_plane = if direction.y.abs() > 0.05 {
        let t = (floor - ray.origin.y) / direction.y;
        if t < 0.0 {
            return None;
        }
        ray.origin + direction * t
    } else if direction.x.abs() > direction.z.abs() {
        Vec3::new(last.x, floor, ray.origin.z)
    } else {
        Vec3::new(ray.origin.x, floor, last.z)
    };
    let snapped = snap_vec3(on_plane, SNAP);
    Some(Vec3::new(snapped.x, floor, snapped.z))
}

/// Whether a corner put at `at` would close the loop through `points` instead.
pub fn closes(points: &[Vec3], at: Vec3) -> bool {
    points.len() >= 3 && points[0].xz().distance(at.xz()) <= CLOSE_RADIUS
}

pub fn build(tool: &Tools, steps: &[Vec<MapPick>], features: &FeatureTimeline) -> Option<Box<dyn FeatureTrait>> {
    let mut draft = Draft::default();
    for pick in steps.first()? {
        match pick {
            MapPick::Point { at, absolute: true, .. } => draft.push(*at, PointRef::absolute(at.x, at.y, at.z)),
            MapPick::Point { point, at, .. } => draft.push(*at, point.clone()),
            _ => {}
        }
    }
    Some(made_by(tool, draft.finish(DEFAULT_HEIGHT, features.features_map())?))
}

/// The corners so far, the next going to `next`, and the room the loop would
/// make if it closed there.
pub fn preview(gizmos: &mut Gizmos, points: &[Vec3], next: Vec3) {
    let drawn = LINE_COLOR;
    let pending = Color::srgb_u8(60, 120, 255);
    let closing = Color::srgb_u8(100, 255, 100);
    let Some(first) = points.first() else { return };
    let floor = first.y;
    let at = |p: Vec2| Vec3::new(p.x, floor, p.y);
    let points: Vec<Vec2> = points.iter().map(|p| p.xz()).collect();
    for pair in points.windows(2) {
        gizmos.line(at(pair[0]), at(pair[1]), drawn);
    }
    let next = next.xz();
    let closes = points.len() >= 3 && points[0].distance(next) <= CLOSE_RADIUS;
    if let Some(last) = points.last() {
        dashed_line(gizmos, at(*last), at(if closes { points[0] } else { next }), pending, 0.15, 0.1);
    }
    gizmos.sphere(Isometry3d::from_translation(at(points[0])), 0.15, if closes { closing } else { drawn });
    let mut outline = points.clone();
    if !closes {
        outline.push(next);
    }
    if outline.len() >= 3 {
        if let Some(last) = outline.last() {
            dashed_line(gizmos, at(*last), at(outline[0]), pending, 0.15, 0.1);
        }
        for (a, b) in hatch(&outline, &[], HATCH_SPACING) {
            gizmos.line(at(a), at(b), if closes { closing } else { pending.with_alpha(0.4) });
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
    fn a_loop_closed_under_footprint_block_is_solid() {
        use crate::common::world_surface::Fill;
        let square = || drawn(&[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]).finish(3.0, &HashMap::new()).unwrap();
        let block = made_by(&Tools::FootprintBlock, square());
        assert_eq!(block.type_key(), "footprint_block");
        assert!(block.rooms().is_empty());
        assert!(!block.volumes().is_empty() && block.volumes().iter().all(|volume| volume.fill == Fill::Solid));
        let room = made_by(&Tools::Footprint, square());
        assert_eq!(room.type_key(), "footprint_room");
        assert_eq!(room.rooms().len(), 1);
    }

    #[test]
    fn two_corners_are_not_a_room() {
        assert!(drawn(&[(0.0, 0.0), (4.0, 0.0)]).finish(3.0, &HashMap::new()).is_none());
    }
}
