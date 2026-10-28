use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use crate::common::PointResolutionError;
use crate::common::cuboid::CuboidPoint;
use crate::common::ray::ray_intersects_aabb;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, OwnBase, PointRef};
use crate::editor::mirror::Mirroring;
use crate::material::picker::material_button;
use crate::tool::room::{Room, RoomSlot, RoomSurfaces};
use crate::get;

/// What `max`'s relative axes are measured from.
#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum MaxBase {
    /// Whatever `max.reference` names, or nothing.
    #[default]
    Reference,
    /// This room's own min, so the far corner is a size. A room's min cannot
    /// be named by its own `PointRef`: that would make the room its own parent.
    OwnMin,
}

impl MaxBase {
    fn is_reference(&self) -> bool {
        *self == MaxBase::Reference
    }
}

#[derive(Serialize, Deserialize)]
pub struct EditorRoom {
    min: PointRef,
    max: PointRef,
    /// Beside `max` rather than replacing it, so maps saved before it read unchanged.
    #[serde(default, skip_serializing_if = "MaxBase::is_reference")]
    max_base: MaxBase,
    #[serde(default, skip_serializing_if = "RoomSurfaces::is_empty")]
    surfaces: RoomSurfaces,
    #[serde(skip)]
    resolved_min: Vec3,
    #[serde(skip)]
    resolved_max: Vec3,
    #[serde(skip)]
    entity: Option<Entity>,
}

impl EditorRoom {
    pub fn from_points(min_feature: FeatureId, max_feature: FeatureId) -> Self {
        Self {
            min: PointRef::reference(min_feature),
            max: PointRef::reference(max_feature),
            max_base: MaxBase::Reference,
            surfaces: RoomSurfaces::default(),
            resolved_min: Vec3::ZERO,
            resolved_max: Vec3::ZERO,
            entity: None,
        }
    }

    pub fn from_point_refs(min: PointRef, max: PointRef) -> Self {
        Self {
            min,
            max,
            max_base: MaxBase::Reference,
            surfaces: RoomSurfaces::default(),
            resolved_min: Vec3::ZERO,
            resolved_max: Vec3::ZERO,
            entity: None,
        }
    }

    /// A room whose far corner is `size` from `min`, and follows it.
    pub fn from_min_and_size(min: PointRef, size: Vec3) -> Self {
        let mut max = PointRef::absolute(0.0, 0.0, 0.0);
        max.x = AxisRef::Relative(size.x);
        max.y = AxisRef::Relative(size.y);
        max.z = AxisRef::Relative(size.z);
        Self {
            min,
            max,
            max_base: MaxBase::OwnMin,
            surfaces: RoomSurfaces::default(),
            resolved_min: Vec3::ZERO,
            resolved_max: Vec3::ZERO,
            entity: None,
        }
    }

    pub fn with_surfaces(mut self, surfaces: RoomSurfaces) -> Self {
        self.surfaces = surfaces;
        self
    }

    /// Min first, since an `OwnMin` max is measured from it. A max that has
    /// been given a reference (the retarget tool writes one straight into the
    /// `PointRef`) is measured from that instead.
    fn resolve_corners(&mut self, features: &HashMap<FeatureId, Feature>) {
        if let Ok(v) = self.min.resolve(features) {
            self.resolved_min = v;
        }
        if self.max.reference.is_some() {
            self.max_base = MaxBase::Reference;
        }
        match self.max_base {
            MaxBase::Reference => {
                if let Ok(v) = self.max.resolve(features) {
                    self.resolved_max = v;
                }
            }
            MaxBase::OwnMin => {
                let base = self.resolved_min;
                self.max.resolved_reference = Some(base);
                self.resolved_max = Vec3::new(
                    self.max.x.resolve_with_base(Some(base.x)).unwrap_or(base.x),
                    self.max.y.resolve_with_base(Some(base.y)).unwrap_or(base.y),
                    self.max.z.resolve_with_base(Some(base.z)).unwrap_or(base.z),
                );
            }
        }
    }
}

#[typetag::serde(name = "editor_room")]
impl FeatureTrait for EditorRoom {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError> {
        let min = self.resolved_min.min(self.resolved_max);
        let max = self.resolved_min.max(self.resolved_max);
        match key {
            "min" => Ok(min),
            "max" => Ok(max),
            "" => Ok((min + max) / 2.0),
            other => {
                let cp = CuboidPoint::try_from(other)?;
                Ok(cp.resolve_in_bounds(min, max))
            }
        }
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        let mut changed = false;
        let size = self.resolved_max - self.resolved_min;
        ui.label(format!("Size: {}", size));
        ui.separator();
        if self.min.editor_ui(ui, "Min", features, prior_feature_order, retarget_request) {
            // Before the max's widgets, which offer the min as a base.
            self.resolve_corners(features);
            changed = true;
        }
        ui.separator();
        let own_label = get!("editor.features.room.own_min");
        let mut from_min = self.max_base == MaxBase::OwnMin;
        changed |= self.max.editor_ui_with_own(ui, "Max", features, prior_feature_order, retarget_request, Some(OwnBase {
            label: &own_label,
            point: self.resolved_min,
            active: &mut from_min,
        }));
        self.max_base = if from_min { MaxBase::OwnMin } else { MaxBase::Reference };
        if changed {
            self.resolve_corners(features);
        }
        ui.separator();
        ui.label(get!("editor.features.room.materials"));
        egui::Grid::new("room_materials").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            for slot in RoomSlot::ALL {
                ui.label(get!(slot.name_key()));
                changed |= material_button(ui, self.surfaces.slot_mut(slot), true);
                ui.end_row();
            }
        });
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.room.title")
    }

    fn type_key(&self) -> &'static str { "editor_room" }

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let min = self.resolved_min.min(self.resolved_max);
        let max = self.resolved_min.max(self.resolved_max);
        let color = Color::srgb_u8(200, 200, 200);

        gizmos.line(Vec3::new(min.x, min.y, min.z), Vec3::new(max.x, min.y, min.z), color);
        gizmos.line(Vec3::new(max.x, min.y, min.z), Vec3::new(max.x, max.y, min.z), color);
        gizmos.line(Vec3::new(max.x, max.y, min.z), Vec3::new(min.x, max.y, min.z), color);
        gizmos.line(Vec3::new(min.x, max.y, min.z), Vec3::new(min.x, min.y, min.z), color);

        gizmos.line(Vec3::new(min.x, min.y, max.z), Vec3::new(max.x, min.y, max.z), color);
        gizmos.line(Vec3::new(max.x, min.y, max.z), Vec3::new(max.x, max.y, max.z), color);
        gizmos.line(Vec3::new(max.x, max.y, max.z), Vec3::new(min.x, max.y, max.z), color);
        gizmos.line(Vec3::new(min.x, max.y, max.z), Vec3::new(min.x, min.y, max.z), color);

        gizmos.line(Vec3::new(min.x, min.y, min.z), Vec3::new(min.x, min.y, max.z), color);
        gizmos.line(Vec3::new(max.x, min.y, min.z), Vec3::new(max.x, min.y, max.z), color);
        gizmos.line(Vec3::new(max.x, max.y, min.z), Vec3::new(max.x, max.y, max.z), color);
        gizmos.line(Vec3::new(min.x, max.y, min.z), Vec3::new(min.x, max.y, max.z), color);

        self.min.debug_gizmos(self.resolved_min, gizmos);
        self.max.debug_gizmos(self.resolved_max, gizmos);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        let min = self.resolved_min.min(self.resolved_max);
        let max = self.resolved_min.max(self.resolved_max);
        let center = (min + max) / 2.0;
        commands.entity(entity).insert((
            Transform::from_translation(center),
            Room::new(min, max),
            self.surfaces.clone(),
        ));
    }

    fn resolve_references(&mut self, features: &HashMap<FeatureId, Feature>) {
        self.resolve_corners(features);
    }

    fn parent_ids(&self) -> Vec<FeatureId> {
        let mut ids = self.min.referenced_features();
        for id in self.max.referenced_features() {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }

    fn available_point_keys(&self) -> Vec<(String, String)> {
        vec![
            ("".into(), "Center".into()),
            ("min".into(), "Min".into()),
            ("max".into(), "Max".into()),
            ("centroid".into(), "Centroid".into()),
            ("top_plane_center".into(), "Top".into()),
            ("bottom_plane_center".into(), "Bottom".into()),
            ("front_plane_center".into(), "Front".into()),
            ("back_plane_center".into(), "Back".into()),
            ("left_plane_center".into(), "Left".into()),
            ("right_plane_center".into(), "Right".into()),
            ("front_bottom_left_corner".into(), "Front Bottom Left".into()),
            ("front_bottom_right_corner".into(), "Front Bottom Right".into()),
            ("front_top_left_corner".into(), "Front Top Left".into()),
            ("front_top_right_corner".into(), "Front Top Right".into()),
            ("back_bottom_left_corner".into(), "Back Bottom Left".into()),
            ("back_bottom_right_corner".into(), "Back Bottom Right".into()),
            ("back_top_left_corner".into(), "Back Top Left".into()),
            ("back_top_right_corner".into(), "Back Top Right".into()),
            ("front_top_edge_center".into(), "Front Top Edge".into()),
            ("front_bottom_edge_center".into(), "Front Bottom Edge".into()),
            ("front_left_edge_center".into(), "Front Left Edge".into()),
            ("front_right_edge_center".into(), "Front Right Edge".into()),
            ("back_top_edge_center".into(), "Back Top Edge".into()),
            ("back_bottom_edge_center".into(), "Back Bottom Edge".into()),
            ("back_left_edge_center".into(), "Back Left Edge".into()),
            ("back_right_edge_center".into(), "Back Right Edge".into()),
            ("bottom_left_edge_center".into(), "Bottom Left Edge".into()),
            ("bottom_right_edge_center".into(), "Bottom Right Edge".into()),
            ("top_left_edge_center".into(), "Top Left Edge".into()),
            ("top_right_edge_center".into(), "Top Right Edge".into()),
        ]
    }

    fn get_point_ref(&self, key: &str) -> Option<&PointRef> {
        match key {
            "Min" | "min" => Some(&self.min),
            "Max" | "max" => Some(&self.max),
            _ => None,
        }
    }

    fn get_point_ref_mut(&mut self, key: &str) -> Option<&mut PointRef> {
        match key {
            "Min" | "min" => Some(&mut self.min),
            "Max" | "max" => Some(&mut self.max),
            _ => None,
        }
    }

    /// `is_max` names a face of the sorted bounds, not a corner: a room drawn right to left has
    /// Point 0 on its max side, so the corner written is whichever lies on that face.
    fn drag_handle(&mut self, is_max: bool, axis: u8, new_world_value: f32) -> bool {
        if axis > 2 {
            return false;
        }
        let i = axis as usize;
        let corners_swapped = self.resolved_min[i] > self.resolved_max[i];
        let is_max = is_max != corners_swapped;
        let point_ref = if is_max { &mut self.max } else { &mut self.min };
        let axis_ref = match axis {
            0 => &mut point_ref.x,
            1 => &mut point_ref.y,
            _ => &mut point_ref.z,
        };
        let base = point_ref.resolved_reference.map(|b| b[i]);
        match axis_ref {
            AxisRef::Absolute(v) => *v = new_world_value,
            AxisRef::Relative(offset) => {
                *offset = new_world_value - base.unwrap_or(0.0);
            }
        }
        if is_max {
            self.resolved_max[i] = new_world_value;
        } else {
            // A face handle resizes. A max measured from the min would
            // otherwise be carried along, moving the room instead.
            if self.max_base == MaxBase::OwnMin {
                let moved_by = new_world_value - self.resolved_min[i];
                let max_axis = match axis { 0 => &mut self.max.x, 1 => &mut self.max.y, _ => &mut self.max.z };
                if let AxisRef::Relative(offset) = max_axis {
                    *offset -= moved_by;
                }
                if let Some(base) = &mut self.max.resolved_reference {
                    base[i] = new_world_value;
                }
            }
            self.resolved_min[i] = new_world_value;
        }
        true
    }

    fn own_bases(&self, point_ref_key: &str) -> Vec<(String, Vec3)> {
        match point_ref_key {
            "Max" | "max" => vec![("min".into(), self.resolved_min)],
            _ => Vec::new(),
        }
    }

    fn rebase_on_own(&mut self, point_ref_key: &str, base: &str) -> bool {
        if !matches!(point_ref_key, "Max" | "max") || base != "min" {
            return false;
        }
        let offset = self.resolved_max - self.resolved_min;
        self.max.reference = None;
        self.max.point_key.clear();
        self.max.x = AxisRef::Relative(offset.x);
        self.max.y = AxisRef::Relative(offset.y);
        self.max.z = AxisRef::Relative(offset.z);
        self.max.resolved_reference = Some(self.resolved_min);
        self.max_base = MaxBase::OwnMin;
        true
    }

    fn rooms(&self) -> Vec<Room> {
        vec![Room::new(self.resolved_min, self.resolved_max)]
    }

    fn drag_handle_bounds(&self) -> Option<(Vec3, Vec3)> {
        Some((self.resolved_min.min(self.resolved_max), self.resolved_min.max(self.resolved_max)))
    }

    fn reference_points_for_ray(&self, ray: &Ray3d) -> Vec<(String, Vec3)> {
        let padding = Vec3::splat(0.25);
        let min = self.resolved_min.min(self.resolved_max) - padding;
        let max = self.resolved_min.max(self.resolved_max) + padding;
        if !ray_intersects_aabb(ray, min, max) {
            return vec![];
        }
        self.available_point_keys().into_iter().filter_map(|(key, _)| {
            self.get_point(&key).ok().map(|v| (key, v))
        }).collect()
    }

    fn mirror(&mut self, mirroring: &Mirroring) -> bool {
        (self.resolved_min, self.resolved_max) = mirroring.bounds(self.resolved_min, self.resolved_max);
        self.min.resolved_reference = None;
        self.max.resolved_reference = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(min: Vec3, max: Vec3) -> EditorRoom {
        let mut room = EditorRoom::from_point_refs(
            PointRef::absolute(min.x, min.y, min.z),
            PointRef::absolute(max.x, max.y, max.z),
        );
        room.resolved_min = min;
        room.resolved_max = max;
        room
    }

    fn drawn_from(min: Vec3, size: Vec3) -> EditorRoom {
        let mut room = EditorRoom::from_min_and_size(PointRef::absolute(min.x, min.y, min.z), size);
        room.resolve_corners(&HashMap::new());
        room
    }

    #[test]
    fn a_far_corner_drawn_from_the_min_follows_it() {
        let mut room = drawn_from(Vec3::ZERO, Vec3::new(4.0, 3.0, 5.0));
        assert_eq!(room.resolved_max, Vec3::new(4.0, 3.0, 5.0));

        room.min = PointRef::absolute(10.0, 1.0, 0.0);
        room.resolve_corners(&HashMap::new());
        assert_eq!(room.resolved_max, Vec3::new(14.0, 4.0, 5.0));
    }

    /// Breaks if a min face's handle moves the room: its far corner is
    /// measured from the min, so it has to be held where it was.
    #[test]
    fn dragging_a_min_face_resizes_a_room_drawn_from_its_min() {
        let mut room = drawn_from(Vec3::ZERO, Vec3::new(4.0, 3.0, 5.0));
        assert!(room.drag_handle(false, 0, -2.0));
        room.resolve_corners(&HashMap::new());
        assert_eq!(room.drag_handle_bounds(), Some((Vec3::new(-2.0, 0.0, 0.0), Vec3::new(4.0, 3.0, 5.0))));
    }

    #[test]
    fn rebasing_onto_the_min_keeps_the_far_corner_where_it_is() {
        let mut room = room(Vec3::new(1.0, 0.0, 1.0), Vec3::new(5.0, 3.0, 4.0));
        assert!(room.rebase_on_own("Max", "min"));
        room.resolve_corners(&HashMap::new());
        assert_eq!(room.max_base, MaxBase::OwnMin);
        assert_eq!(room.resolved_max, Vec3::new(5.0, 3.0, 4.0));
    }

    /// The retarget tool writes a reference straight into the `PointRef`;
    /// from then on that is what the far corner is measured from.
    #[test]
    fn a_far_corner_given_a_reference_stops_following_the_min() {
        let mut room = drawn_from(Vec3::ZERO, Vec3::ONE);
        room.max.reference = Some(FeatureId::from_raw(99));
        room.resolve_corners(&HashMap::new());
        assert_eq!(room.max_base, MaxBase::Reference);
    }

    /// Breaks if every room starts writing the field: maps would churn on
    /// their next save for nothing.
    #[test]
    fn a_room_not_drawn_from_its_min_writes_no_base() {
        let text = ron::to_string(&room(Vec3::ZERO, Vec3::ONE)).unwrap();
        assert!(!text.contains("max_base"), "{text}");
        let text = ron::to_string(&drawn_from(Vec3::ZERO, Vec3::ONE)).unwrap();
        assert!(text.contains("max_base"), "{text}");
    }

    #[test]
    fn a_handle_moves_its_own_face_whichever_corner_is_point_zero() {
        for (p0, p1) in [
            (Vec3::new(-2.0, 0.0, -3.0), Vec3::new(2.0, 4.0, 3.0)),
            (Vec3::new(2.0, 4.0, 3.0), Vec3::new(-2.0, 0.0, -3.0)),
        ] {
            let mut room = room(p0, p1);
            assert!(room.drag_handle(false, 2, -5.0));
            assert!(room.drag_handle(true, 0, 6.0));
            let (min, max) = room.drag_handle_bounds().unwrap();
            assert_eq!(min, Vec3::new(-2.0, 0.0, -5.0), "from {p0} to {p1}");
            assert_eq!(max, Vec3::new(6.0, 4.0, 3.0), "from {p0} to {p1}");
        }
    }
}
