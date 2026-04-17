use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use bevy_egui::egui::Context;
use serde::{Deserialize, Serialize};
use crate::common::PointResolutionError;
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
}

#[typetag::serde(name = "editor_room")]
impl EditorObject for EditorRoom {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError> {
        match key {
            "min" => Ok(self.resolved_min),
            "max" => Ok(self.resolved_max),
            _ => Ok((self.resolved_min + self.resolved_max) / 2.0),
        }
    }

    fn editor_ui(&mut self, ctx: &mut Context) -> bool {
        egui::Window::new(self.type_name()).show(ctx, |ui| {
            ui.label(format!("Min: {}", self.resolved_min));
            ui.label(format!("Max: {}", self.resolved_max));
            let size = self.resolved_max - self.resolved_min;
            ui.label(format!("Size: {}", size));
        });
        false
    }

    fn type_name(&self) -> String {
        get!("editor.actions.room.title")
    }

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let min = self.resolved_min;
        let max = self.resolved_max;
        let color = Color::srgb_u8(200, 200, 200);

        // Bottom face (z = min.z)
        gizmos.line(Vec3::new(min.x, min.y, min.z), Vec3::new(max.x, min.y, min.z), color);
        gizmos.line(Vec3::new(max.x, min.y, min.z), Vec3::new(max.x, max.y, min.z), color);
        gizmos.line(Vec3::new(max.x, max.y, min.z), Vec3::new(min.x, max.y, min.z), color);
        gizmos.line(Vec3::new(min.x, max.y, min.z), Vec3::new(min.x, min.y, min.z), color);

        // Top face (z = max.z)
        gizmos.line(Vec3::new(min.x, min.y, max.z), Vec3::new(max.x, min.y, max.z), color);
        gizmos.line(Vec3::new(max.x, min.y, max.z), Vec3::new(max.x, max.y, max.z), color);
        gizmos.line(Vec3::new(max.x, max.y, max.z), Vec3::new(min.x, max.y, max.z), color);
        gizmos.line(Vec3::new(min.x, max.y, max.z), Vec3::new(min.x, min.y, max.z), color);

        // Vertical edges
        gizmos.line(Vec3::new(min.x, min.y, min.z), Vec3::new(min.x, min.y, max.z), color);
        gizmos.line(Vec3::new(max.x, min.y, min.z), Vec3::new(max.x, min.y, max.z), color);
        gizmos.line(Vec3::new(max.x, max.y, min.z), Vec3::new(max.x, max.y, max.z), color);
        gizmos.line(Vec3::new(min.x, max.y, min.z), Vec3::new(min.x, max.y, max.z), color);
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
}
