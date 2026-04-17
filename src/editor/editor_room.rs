use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use bevy_egui::egui::Context;
use serde::{Deserialize, Serialize};
use crate::common::PointResolutionError;
use crate::common::cuboid::CuboidPoint;
use crate::common::ray::ray_intersects_aabb;
use crate::editor::editable::{EditorAction, EditorActionId, EditorObject, PointRef};
use crate::tool::room::Room;
use crate::get;

#[derive(Serialize, Deserialize)]
pub struct EditorRoom {
    min: PointRef,
    max: PointRef,
    #[serde(skip)]
    resolved_min: Vec3,
    #[serde(skip)]
    resolved_max: Vec3,
    #[serde(skip)]
    entity: Option<Entity>,
}

impl EditorRoom {
    pub fn from_points(min_action: EditorActionId, max_action: EditorActionId) -> Self {
        Self {
            min: PointRef::reference(min_action),
            max: PointRef::reference(max_action),
            resolved_min: Vec3::ZERO,
            resolved_max: Vec3::ZERO,
            entity: None,
        }
    }

    pub fn from_point_refs(min: PointRef, max: PointRef) -> Self {
        Self {
            min,
            max,
            resolved_min: Vec3::ZERO,
            resolved_max: Vec3::ZERO,
            entity: None,
        }
    }
}

#[typetag::serde(name = "editor_room")]
impl EditorObject for EditorRoom {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError> {
        match key {
            "min" => Ok(self.resolved_min),
            "max" => Ok(self.resolved_max),
            "" => Ok((self.resolved_min + self.resolved_max) / 2.0),
            other => {
                let cp = CuboidPoint::try_from(other)?;
                Ok(cp.resolve_in_bounds(self.resolved_min, self.resolved_max))
            }
        }
    }

    fn editor_ui(&mut self, ctx: &mut Context, actions: &HashMap<EditorActionId, EditorAction>, prior_action_order: &[EditorActionId]) -> bool {
        let mut changed = false;
        egui::Window::new(self.type_name()).show(ctx, |ui| {
            let size = self.resolved_max - self.resolved_min;
            ui.label(format!("Size: {}", size));
            ui.separator();
            changed |= self.min.editor_ui(ui, "Min", actions, prior_action_order);
            ui.separator();
            changed |= self.max.editor_ui(ui, "Max", actions, prior_action_order);
        });
        if changed {
            if let Ok(v) = self.min.resolve(actions) {
                self.resolved_min = v;
            }
            if let Ok(v) = self.max.resolve(actions) {
                self.resolved_max = v;
            }
        }
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.actions.room.title")
    }

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let min = self.resolved_min;
        let max = self.resolved_max;
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

    fn apply_to_entity(&self, commands: &mut Commands, entity: Entity) {
        let center = (self.resolved_min + self.resolved_max) / 2.0;
        commands.entity(entity).insert((
            Transform::from_translation(center),
            Room::new(self.resolved_min, self.resolved_max),
        ));
    }

    fn resolve_references(&mut self, actions: &HashMap<EditorActionId, EditorAction>) {
        if let Ok(v) = self.min.resolve(actions) {
            self.resolved_min = v;
        }
        if let Ok(v) = self.max.resolve(actions) {
            self.resolved_max = v;
        }
    }

    fn parent_ids(&self) -> Vec<EditorActionId> {
        let mut ids = self.min.referenced_actions();
        for id in self.max.referenced_actions() {
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
}
