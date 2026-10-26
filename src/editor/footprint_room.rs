//! Rooms drawn as a floor plan: a sketch whose corners are `PointRef`s,
//! arranged into regions the way a prop's sketch is, each named region stood
//! up between two heights. See `documentation/footprint-rooms.md`.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::ray::ray_intersects_aabb;
use crate::common::PointResolutionError;
use crate::editor::editable::{Feature, FeatureId, FeatureTrait, PointRef};
use crate::editor::mirror::Mirroring;
use crate::get;
use crate::prop::region::{arrange, Arrangement, Note};
use crate::prop::sketch::{region_label, Sketch, SketchEntity, SketchId, SketchPoint, SketchRegion};
use crate::tool::room::Room;

/// A height a new footprint stands to before anybody sets one.
pub const DEFAULT_HEIGHT: f32 = 3.0;

#[derive(Clone, Serialize, Deserialize)]
pub struct Corner {
    pub id: SketchId,
    /// Read for `x` and `z`; its `y` is the floor's business.
    pub at: PointRef,
}

/// A sketch line, as the map writes it. Not [`SketchEntity`]: that is
/// internally tagged, which RON writes with its struct name and then cannot
/// read back.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub id: SketchId,
    pub a: SketchId,
    pub b: SketchId,
    /// Drawn and snapped to, never part of a region.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub construction: bool,
}

/// Why part of a footprint stands nothing up.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// A region entry it names that no longer resolves: a bounding line gone.
    Lost(SketchId),
    Note(Note),
}

#[derive(Serialize, Deserialize)]
pub struct FootprintRoom {
    /// Read for `y`.
    floor: PointRef,
    /// Metres from the floor up to the ceiling.
    height: f32,
    /// Shared by corners, lines and region entries, and never reused, so a
    /// region naming a line cannot be handed another.
    next_id: u32,
    corners: Vec<Corner>,
    lines: Vec<Line>,
    /// Hints in world `x`/`z`.
    regions: Vec<SketchRegion>,
    /// Region entries stood up as rooms.
    rooms: Vec<SketchId>,
    #[serde(skip)]
    resolved_floor: f32,
    #[serde(skip)]
    resolved_corners: Vec<(SketchId, Vec2)>,
    #[serde(skip)]
    resolved: Vec<Room>,
    #[serde(skip)]
    problems: Vec<Problem>,
    #[serde(skip)]
    entity: Option<Entity>,
    #[serde(skip)]
    room_entities: Vec<Entity>,
}

impl FootprintRoom {
    pub fn new(floor: PointRef, height: f32) -> Self {
        Self {
            floor,
            height,
            next_id: 0,
            corners: Vec::new(),
            lines: Vec::new(),
            regions: Vec::new(),
            rooms: Vec::new(),
            resolved_floor: 0.0,
            resolved_corners: Vec::new(),
            resolved: Vec::new(),
            problems: Vec::new(),
            entity: None,
            room_entities: Vec::new(),
        }
    }

    fn take_id(&mut self) -> SketchId {
        let id = SketchId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn add_corner(&mut self, at: PointRef) -> SketchId {
        let id = self.take_id();
        self.corners.push(Corner { id, at });
        id
    }

    pub fn add_line(&mut self, a: SketchId, b: SketchId) -> SketchId {
        let id = self.take_id();
        self.lines.push(Line { id, a, b, construction: false });
        id
    }

    /// Stand up the region under `at` (world `x`/`z`) as a room. Needs the
    /// corners resolved; `false` when nothing is enclosed there.
    pub fn name_region_at(&mut self, at: Vec2) -> bool {
        let (mut sketch, origin) = self.local_sketch();
        let arrangement = arrange(&sketch);
        let Some(region) = arrangement.region_at(at - origin) else { return false };
        let id = sketch.adopt_region(region);
        self.absorb(sketch, origin);
        if !self.rooms.contains(&id) {
            self.rooms.push(id);
        }
        true
    }

    pub fn rooms(&self) -> &[Room] {
        &self.resolved
    }

    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The resolved corners as a prop sketch, relative to the first corner.
    /// The region code's tolerance is absolute and chosen for a prop; the
    /// tests far from the origin pass without this, but it costs nothing and
    /// keeps the numbers the arrangement sees the size of the plan.
    fn local_sketch(&self) -> (Sketch, Vec2) {
        let origin = self.resolved_corners.first().map_or(Vec2::ZERO, |(_, at)| *at);
        let sketch = Sketch {
            next_id: self.next_id,
            points: self
                .resolved_corners
                .iter()
                .map(|(id, at)| SketchPoint { id: *id, at: (*at - origin).to_array() })
                .collect(),
            entities: self
                .lines
                .iter()
                .map(|line| SketchEntity::Line { id: line.id, a: line.a, b: line.b, construction: line.construction })
                .collect(),
            regions: self
                .regions
                .iter()
                .map(|region| SketchRegion { hint: (Vec2::from_array(region.hint) - origin).to_array(), ..region.clone() })
                .collect(),
        };
        (sketch, origin)
    }

    /// Take back what a sketch edit changed: lines, region entries and the id
    /// counter. Points are not taken back; corners are `PointRef`s.
    fn absorb(&mut self, sketch: Sketch, origin: Vec2) {
        self.next_id = sketch.next_id;
        self.lines = sketch
            .entities
            .into_iter()
            .map(|entity| match entity {
                SketchEntity::Line { id, a, b, construction } => Line { id, a, b, construction },
            })
            .collect();
        self.regions = sketch
            .regions
            .into_iter()
            .map(|region| SketchRegion { hint: (Vec2::from_array(region.hint) + origin).to_array(), ..region })
            .collect();
    }

    fn resolve_rooms(&mut self) {
        let (sketch, origin) = self.local_sketch();
        let arrangement: Arrangement = arrange(&sketch);
        self.problems = arrangement.notes.iter().cloned().map(Problem::Note).collect();
        let (floor, ceiling) = (self.resolved_floor, self.resolved_floor + self.height);
        let shifted = |points: &[Vec2]| points.iter().map(|p| *p + origin).collect::<Vec<_>>();
        self.resolved = self
            .rooms
            .iter()
            .filter_map(|id| {
                let found = sketch
                    .region(*id)
                    .and_then(|entry| arrangement.resolve(&entry.name(), Vec2::from_array(entry.hint)));
                if found.is_none() {
                    self.problems.push(Problem::Lost(*id));
                }
                let region = found?;
                Some(Room::with_holes(shifted(&region.outer), region.holes.iter().map(|h| shifted(h)).collect(), floor, ceiling))
            })
            .collect();
    }

    fn corner_at(&self, id: SketchId) -> Option<Vec2> {
        self.resolved_corners.iter().find(|(corner, _)| *corner == id).map(|(_, at)| *at)
    }

    fn corner_label(id: SketchId) -> String {
        get!("editor.features.footprint_room.corner", "id", id)
    }

    fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let (low, high) = self
            .resolved_corners
            .iter()
            .fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), (_, p)| (lo.min(*p), hi.max(*p)));
        (low.x <= high.x).then(|| {
            (
                Vec3::new(low.x, self.resolved_floor, low.y),
                Vec3::new(high.x, self.resolved_floor + self.height, high.y),
            )
        })
    }

    fn problem_text(&self, problem: &Problem) -> String {
        let (sketch, _) = self.local_sketch();
        match problem {
            Problem::Lost(id) => get!("editor.features.footprint_room.lost", "region", region_label(&sketch, *id)),
            Problem::Note(Note::Open(line)) => get!("editor.features.footprint_room.open", "line", line),
            Problem::Note(Note::ZeroLength(line)) => get!("editor.features.footprint_room.zero_length", "line", line),
            Problem::Note(Note::Coincident(a, b)) => get!("editor.features.footprint_room.coincident", "a", a, "b", b),
            Problem::Note(Note::MissingPoint(line)) => get!("editor.features.footprint_room.missing_point", "line", line),
        }
    }
}

#[typetag::serde(name = "footprint_room")]
impl FeatureTrait for FootprintRoom {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError> {
        if key.is_empty() || key == "floor" {
            return self
                .resolved
                .first()
                .map(Room::floor_point)
                .ok_or(PointResolutionError::NoSuchPoint);
        }
        let (corner, up) = match key.strip_suffix(".ceiling") {
            Some(corner) => (corner, self.height),
            None => (key, 0.0),
        };
        let id = corner
            .strip_prefix("corner#")
            .and_then(|id| id.parse().ok())
            .ok_or(PointResolutionError::NoSuchPoint)?;
        let at = self.corner_at(SketchId(id)).ok_or(PointResolutionError::NoSuchPoint)?;
        Ok(Vec3::new(at.x, self.resolved_floor + up, at.y))
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        let mut changed = false;
        for problem in &self.problems {
            ui.colored_label(egui::Color32::ORANGE, self.problem_text(problem));
        }
        ui.horizontal(|ui| {
            ui.label(get!("editor.features.footprint_room.height"));
            changed |= ui.add(egui::DragValue::new(&mut self.height).speed(0.05).range(0.1..=f32::MAX)).changed();
        });
        ui.separator();
        changed |= self.floor.editor_ui(ui, &get!("editor.features.footprint_room.floor"), features, prior_feature_order, retarget_request);
        for corner in &mut self.corners {
            ui.separator();
            changed |= corner.at.editor_ui(ui, &Self::corner_label(corner.id), features, prior_feature_order, retarget_request);
        }
        if changed {
            self.resolve_references(features);
        }
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.footprint_room.title")
    }

    fn type_key(&self) -> &'static str {
        "footprint_room"
    }

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let at = |p: Vec2| Vec3::new(p.x, self.resolved_floor, p.y);
        for line in &self.lines {
            if let (Some(a), Some(b)) = (self.corner_at(line.a), self.corner_at(line.b)) {
                gizmos.line(at(a), at(b), Color::srgb_u8(200, 200, 120));
            }
        }
        for room in &self.resolved {
            for (a, b) in room.edges() {
                gizmos.line(a, b, Color::srgb_u8(100, 100, 100));
            }
        }
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
        self.room_entities.clear();
    }

    /// One child per room, re-applied rather than respawned: a respawned room
    /// is a rebake of it every frame of a drag.
    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        commands.entity(entity).insert_if_new((Transform::IDENTITY, Visibility::Inherited));
        for surplus in self.room_entities.drain(self.resolved.len().min(self.room_entities.len())..) {
            commands.entity(surplus).despawn();
        }
        for (index, room) in self.resolved.iter().enumerate() {
            match self.room_entities.get(index) {
                Some(child) => {
                    commands.entity(*child).insert(room.clone());
                }
                None => {
                    let child = commands.spawn((room.clone(), ChildOf(entity))).id();
                    self.room_entities.push(child);
                }
            }
        }
    }

    fn resolve_references(&mut self, features: &HashMap<FeatureId, Feature>) {
        if let Ok(floor) = self.floor.resolve(features) {
            self.resolved_floor = floor.y;
        }
        // A corner that does not resolve is left out, and the lines naming it
        // say so as missing points.
        self.resolved_corners = self
            .corners
            .iter_mut()
            .filter_map(|corner| corner.at.resolve(features).ok().map(|at| (corner.id, Vec2::new(at.x, at.z))))
            .collect();
        self.resolve_rooms();
    }

    fn parent_ids(&self) -> Vec<FeatureId> {
        let mut ids = self.floor.referenced_features();
        for corner in &self.corners {
            for id in corner.at.referenced_features() {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    fn available_point_keys(&self) -> Vec<(String, String)> {
        let mut keys = vec![("floor".to_string(), get!("editor.features.footprint_room.floor"))];
        for corner in &self.corners {
            let label = Self::corner_label(corner.id);
            keys.push((format!("corner#{}", corner.id.0), label.clone()));
            keys.push((format!("corner#{}.ceiling", corner.id.0), get!("editor.features.footprint_room.ceiling_of", "corner", label)));
        }
        keys
    }

    fn reference_points_for_ray(&self, ray: &Ray3d) -> Vec<(String, Vec3)> {
        let Some((min, max)) = self.bounds() else { return vec![] };
        let padding = Vec3::splat(0.25);
        if !ray_intersects_aabb(ray, min - padding, max + padding) {
            return vec![];
        }
        self.available_point_keys()
            .into_iter()
            .filter_map(|(key, _)| self.get_point(&key).ok().map(|v| (key, v)))
            .collect()
    }

    fn get_point_ref(&self, key: &str) -> Option<&PointRef> {
        if key == "floor" || key == get!("editor.features.footprint_room.floor") {
            return Some(&self.floor);
        }
        self.corners
            .iter()
            .find(|corner| key == format!("corner#{}", corner.id.0) || key == Self::corner_label(corner.id))
            .map(|corner| &corner.at)
    }

    fn get_point_ref_mut(&mut self, key: &str) -> Option<&mut PointRef> {
        if key == "floor" || key == get!("editor.features.footprint_room.floor") {
            return Some(&mut self.floor);
        }
        self.corners
            .iter_mut()
            .find(|corner| key == format!("corner#{}", corner.id.0) || key == Self::corner_label(corner.id))
            .map(|corner| &mut corner.at)
    }

    fn rooms(&self) -> Vec<Room> {
        self.resolved.clone()
    }

    fn mirror(&mut self, mirroring: &Mirroring) -> bool {
        let floor = self.resolved_floor;
        let across = |p: Vec2| {
            let moved = mirroring.point(Vec3::new(p.x, floor, p.y));
            Vec2::new(moved.x, moved.z)
        };
        let loop_across = |points: &[Vec2]| points.iter().map(|p| across(*p)).collect::<Vec<_>>();
        self.resolved = self
            .resolved
            .iter()
            .map(|room| Room::with_holes(loop_across(room.footprint()), room.holes().iter().map(|h| loop_across(h)).collect(), room.floor, room.ceiling))
            .collect();
        for (_, at) in &mut self.resolved_corners {
            *at = across(*at);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corner(x: f32, z: f32) -> PointRef {
        PointRef::absolute(x, 0.0, z)
    }

    /// A closed loop through `points`, each a new corner.
    fn draw_loop(room: &mut FootprintRoom, points: &[(f32, f32)]) -> Vec<SketchId> {
        let ids: Vec<SketchId> = points.iter().map(|(x, z)| room.add_corner(corner(*x, *z))).collect();
        for i in 0..ids.len() {
            room.add_line(ids[i], ids[(i + 1) % ids.len()]);
        }
        ids
    }

    fn resolved(mut room: FootprintRoom) -> FootprintRoom {
        room.resolve_references(&HashMap::new());
        room
    }

    #[test]
    fn an_l_drawn_and_named_is_one_room_of_its_area() {
        let mut room = FootprintRoom::new(PointRef::absolute(0.0, 1.0, 0.0), 2.5);
        draw_loop(&mut room, &[(0.0, 0.0), (3.0, 0.0), (3.0, 1.0), (1.0, 1.0), (1.0, 3.0), (0.0, 3.0)]);
        let mut room = resolved(room);
        assert!(room.name_region_at(Vec2::new(0.5, 0.5)));
        let room = resolved(room);
        assert_eq!(room.rooms().len(), 1);
        assert!((room.rooms()[0].volume() - 5.0 * 2.5).abs() < 1e-4);
        assert_eq!((room.rooms()[0].floor, room.rooms()[0].ceiling), (1.0, 3.5));
        assert!(room.problems().is_empty(), "{:?}", room.problems());
    }

    #[test]
    fn a_loop_drawn_inside_is_a_pillar() {
        let mut room = FootprintRoom::new(PointRef::absolute(0.0, 0.0, 0.0), 3.0);
        draw_loop(&mut room, &[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]);
        draw_loop(&mut room, &[(1.5, 1.5), (2.5, 1.5), (2.5, 2.5), (1.5, 2.5)]);
        let mut room = resolved(room);
        assert!(room.name_region_at(Vec2::new(0.5, 0.5)));
        let room = resolved(room);
        assert_eq!(room.rooms()[0].holes().len(), 1);
        assert!((room.rooms()[0].volume() - 45.0).abs() < 1e-3);
    }

    /// Two squares overlapping make three regions; naming all three makes
    /// three rooms that the union joins back into one space.
    #[test]
    fn crossing_loops_split_into_regions() {
        let mut room = FootprintRoom::new(PointRef::absolute(0.0, 0.0, 0.0), 3.0);
        draw_loop(&mut room, &[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]);
        draw_loop(&mut room, &[(2.0, 2.0), (6.0, 2.0), (6.0, 6.0), (2.0, 6.0)]);
        let mut room = resolved(room);
        for at in [Vec2::new(1.0, 1.0), Vec2::new(3.0, 3.0), Vec2::new(5.0, 5.0)] {
            assert!(room.name_region_at(at), "nothing enclosed at {at}");
        }
        let room = resolved(room);
        assert_eq!(room.rooms().len(), 3);
        let total: f32 = room.rooms().iter().map(Room::volume).sum();
        assert!((total - (16.0 + 16.0 - 4.0) * 3.0).abs() < 1e-3);
    }

    #[test]
    fn a_region_whose_line_is_deleted_is_a_problem_and_no_room() {
        let mut room = FootprintRoom::new(PointRef::absolute(0.0, 0.0, 0.0), 3.0);
        draw_loop(&mut room, &[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]);
        let mut room = resolved(room);
        room.name_region_at(Vec2::new(1.0, 1.0));
        room.lines.pop();
        let room = resolved(room);
        assert!(room.rooms().is_empty());
        assert!(room.problems().iter().any(|p| matches!(p, Problem::Lost(_))), "{:?}", room.problems());
    }

    #[test]
    fn a_footprint_far_from_the_origin_still_makes_its_room() {
        let mut room = FootprintRoom::new(PointRef::absolute(0.0, 0.0, 0.0), 3.0);
        let (x, z) = (143.37, 87.91);
        draw_loop(&mut room, &[(x, z), (x + 7.3, z), (x + 7.3, z + 2.1), (x + 1.9, z + 2.1), (x + 1.9, z + 5.7), (x, z + 5.7)]);
        let mut room = resolved(room);
        assert!(room.name_region_at(Vec2::new(x + 0.5, z + 0.5)));
        let room = resolved(room);
        let expected = (7.3 * 2.1 + 1.9 * (5.7 - 2.1)) * 3.0;
        assert!((room.rooms()[0].volume() - expected).abs() < 1e-2, "{} of {expected}", room.rooms()[0].volume());
    }

    #[test]
    fn crossing_slanted_loops_far_from_the_origin_still_split() {
        let mut room = FootprintRoom::new(PointRef::absolute(0.0, 0.0, 0.0), 3.0);
        let centre = Vec2::new(143.37, 87.91);
        let square = |at: Vec2, turn: f32| -> Vec<(f32, f32)> {
            (0..4)
                .map(|i| {
                    let angle = turn + i as f32 * std::f32::consts::FRAC_PI_2;
                    let p = at + Vec2::new(angle.cos(), angle.sin()) * 2.83;
                    (p.x, p.y)
                })
                .collect()
        };
        draw_loop(&mut room, &square(centre, 0.3));
        draw_loop(&mut room, &square(centre + Vec2::new(1.7, 1.1), 0.9));
        let mut room = resolved(room);
        for at in [centre - Vec2::new(1.0, 0.8), centre + Vec2::new(0.9, 0.6), centre + Vec2::new(2.9, 2.3)] {
            assert!(room.name_region_at(at), "nothing enclosed at {at}");
        }
        let room = resolved(room);
        assert_eq!(room.rooms().len(), 3, "{:?}", room.problems());
        let total: f32 = room.rooms().iter().map(Room::volume).sum();
        let one = 2.0 * 2.83f32 * 2.83 * 3.0;
        assert!(total > one && total < 2.0 * one, "{total}");
    }

    #[test]
    fn a_corners_points_are_published_on_the_floor_and_ceiling() {
        let mut room = FootprintRoom::new(PointRef::absolute(0.0, 2.0, 0.0), 3.0);
        let ids = draw_loop(&mut room, &[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0)]);
        let room = resolved(room);
        let key = format!("corner#{}", ids[1].0);
        assert_eq!(room.get_point(&key).unwrap(), Vec3::new(4.0, 2.0, 0.0));
        assert_eq!(room.get_point(&format!("{key}.ceiling")).unwrap(), Vec3::new(4.0, 5.0, 0.0));
    }

    #[test]
    fn a_mirrored_footprint_is_the_same_room_reflected() {
        let mut room = FootprintRoom::new(PointRef::absolute(0.0, 0.0, 0.0), 3.0);
        draw_loop(&mut room, &[(1.0, 0.0), (4.0, 0.0), (1.0, 3.0)]);
        let mut room = resolved(room);
        room.name_region_at(Vec2::new(1.5, 0.5));
        let mut room = resolved(room);
        let mirror = crate::editor::mirror::Mirror::new(PointRef::absolute(0.0, 0.0, 0.0));
        assert!(room.mirror(&mirror.mirroring()));
        let mirrored = &room.rooms()[0];
        assert!((mirrored.volume() - 4.5 * 3.0).abs() < 1e-4, "inside out after mirroring");
        assert!(mirrored.bounds().1.x <= 0.0 || mirrored.bounds().0.z < 0.0, "not reflected: {:?}", mirrored.bounds());
    }

    /// The point a corner names, moved, moves the wall.
    #[test]
    fn a_corner_measured_from_another_feature_follows_it() {
        use crate::editor::editable::FeatureTimeline;
        use crate::editor::global_point::GlobalPoint;

        let mut timeline = FeatureTimeline::default();
        let anchor = timeline.apply_feature(Box::new(GlobalPoint::new(4.0, 0.0, 0.0)));
        let mut room = FootprintRoom::new(PointRef::absolute(0.0, 0.0, 0.0), 3.0);
        let a = room.add_corner(corner(0.0, 0.0));
        let b = room.add_corner(PointRef::reference(anchor));
        let c = room.add_corner(corner(0.0, 4.0));
        for (from, to) in [(a, b), (b, c), (c, a)] {
            room.add_line(from, to);
        }
        room.resolve_references(timeline.features_map());
        assert!(room.name_region_at(Vec2::new(0.5, 0.5)));
        assert_eq!(room.parent_ids(), vec![anchor]);
        room.resolve_references(timeline.features_map());
        assert!((room.rooms()[0].bounds().1.x - 4.0).abs() < 1e-5);

        let mut point = timeline.features_mut().remove(&anchor).unwrap();
        point.object_mut().drag_handle(false, 0, 7.0);
        point.object_mut().resolve_references(timeline.features_map());
        timeline.features_mut().insert(anchor, point);
        room.resolve_references(timeline.features_map());
        assert!((room.rooms()[0].bounds().1.x - 7.0).abs() < 1e-5, "the wall stayed: {:?}", room.rooms()[0].bounds());
        assert!(room.problems().is_empty(), "the region was lost when its corner moved: {:?}", room.problems());
    }

    /// From the map's side: the rooms stand as entities the bake and
    /// collision read, and a pick lands on the notch's wall, not across it.
    #[test]
    fn a_footprint_in_a_map_is_rooms_collision_and_picking_agree_on() {
        use crate::editor::editable::FeatureTimeline;
        use crate::game::collision::CollisionWorld;
        use crate::tool::show::GizmoVisibility;
        use crate::tool::tool_helpers::find_nearest_feature_hit;

        let mut room = FootprintRoom::new(PointRef::absolute(0.0, 0.0, 0.0), 3.0);
        draw_loop(&mut room, &[(0.0, 0.0), (6.0, 0.0), (6.0, 2.0), (2.0, 2.0), (2.0, 6.0), (0.0, 6.0)]);
        let mut room = resolved(room);
        room.name_region_at(Vec2::new(1.0, 1.0));
        let mut timeline = FeatureTimeline::default();
        let id = timeline.apply_feature(Box::new(room));

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(timeline);
        app.add_systems(Update, FeatureTimeline::sync_entities);
        app.update();
        app.update();

        let world = app.world_mut();
        let rooms: Vec<Room> = world.query::<&Room>().iter(world).cloned().collect();
        assert_eq!(rooms.len(), 1, "the footprint stood up no room entity");

        let mut collision = CollisionWorld::default();
        collision.rebuild(&rooms);
        // Across the notch, from one arm of the L to the other.
        let across = Ray3d::new(Vec3::new(4.0, 1.0, 1.0), Dir3::Z);
        let wall = collision.ray_distance(&across, 10.0).expect("no wall across the notch");
        assert!((wall - 1.0).abs() < 1e-3, "{wall}");

        let timeline = app.world().resource::<FeatureTimeline>();
        let (picked, hit) = find_nearest_feature_hit(&across, timeline, &GizmoVisibility::default()).expect("nothing picked");
        assert_eq!(picked, id);
        assert!((hit.z - 2.0).abs() < 1e-3, "picked through the notch at {hit}");
    }
}
