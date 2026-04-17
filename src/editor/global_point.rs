use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use crate::common::PointResolutionError;
use crate::editor::editable::{EditorAction, EditorActionId, EditorObject, PointRef};
use crate::get;

#[derive(Serialize, Deserialize)]
pub struct GlobalPoint {
    location: PointRef,
    #[serde(skip)]
    resolved_location: Vec3,
    #[serde(skip)]
    entity: Option<Entity>,
}

#[typetag::serde(name = "global_point")]
impl EditorObject for GlobalPoint {
    fn get_point(&self, _key: &str) -> Result<Vec3, PointResolutionError> {
        Ok(self.resolved_location)
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, actions: &HashMap<EditorActionId, EditorAction>, prior_action_order: &[EditorActionId]) -> bool {
        let mut changed = false;
        changed |= self.location.editor_ui(ui, "Location", actions, prior_action_order);
        if changed {
            if let Ok(v) = self.location.resolve(actions) {
                self.resolved_location = v;
            }
        }
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.actions.global_point.title")
    }

    fn type_key(&self) -> &'static str { "global_point" }
    
    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        gizmos.sphere(Isometry3d::from_translation(self.resolved_location), 0.2, Color::srgb_u8(0, 255, 0));
        self.location.debug_gizmos(self.resolved_location, gizmos);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    fn apply_to_entity(&self, commands: &mut Commands, entity: Entity) {
        commands.entity(entity).insert(Transform::from_translation(self.resolved_location));
    }

    fn resolve_references(&mut self, actions: &HashMap<EditorActionId, EditorAction>) {
        if let Ok(v) = self.location.resolve(actions) {
            self.resolved_location = v;
        }
    }

    fn parent_ids(&self) -> Vec<EditorActionId> {
        self.location.referenced_actions()
    }

    fn available_point_keys(&self) -> Vec<(String, String)> {
        vec![("".into(), "Point".into())]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![("".into(), self.resolved_location)]
    }
}

impl GlobalPoint {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self {
            location: PointRef::absolute(x, y, z),
            resolved_location: Vec3::new(x, y, z),
            entity: None,
        }
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            resolved_location: Vec3::ZERO,
            entity: None,
        }
    }
}
