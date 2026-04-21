use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use crate::common::PointResolutionError;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
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
impl FeatureTrait for GlobalPoint {
    fn get_point(&self, _key: &str) -> Result<Vec3, PointResolutionError> {
        Ok(self.resolved_location)
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        let mut changed = false;
        changed |= self.location.editor_ui(ui, "Location", features, prior_feature_order, retarget_request);
        if changed {
            if let Ok(v) = self.location.resolve(features) {
                self.resolved_location = v;
            }
        }
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.global_point.title")
    }

    fn type_key(&self) -> &'static str { "global_point" }
    
    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        gizmos.sphere(Isometry3d::from_translation(self.resolved_location), 0.2, Color::srgb_u8(255, 60, 60));
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

    fn resolve_references(&mut self, features: &HashMap<FeatureId, Feature>) {
        if let Ok(v) = self.location.resolve(features) {
            self.resolved_location = v;
        }
    }

    fn parent_ids(&self) -> Vec<FeatureId> {
        self.location.referenced_features()
    }

    fn available_point_keys(&self) -> Vec<(String, String)> {
        vec![("".into(), "Point".into())]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![("".into(), self.resolved_location)]
    }

    fn point_ref_slots(&self) -> Vec<&str> { vec!["location"] }

    fn get_point_ref(&self, _key: &str) -> Option<&PointRef> {
        Some(&self.location)
    }

    fn get_point_ref_mut(&mut self, _key: &str) -> Option<&mut PointRef> {
        Some(&mut self.location)
    }

    fn drag_handle(&mut self, _is_max: bool, axis: u8, new_world_value: f32) -> bool {
        let axis_ref = match axis {
            0 => &mut self.location.x,
            1 => &mut self.location.y,
            2 => &mut self.location.z,
            _ => return false,
        };
        let base = self.location.resolved_reference.map(|b| match axis {
            0 => b.x, 1 => b.y, _ => b.z,
        });
        match axis_ref {
            AxisRef::Absolute(v) => *v = new_world_value,
            AxisRef::Relative(offset) => *offset = new_world_value - base.unwrap_or(0.0),
        }
        match axis { 0 => self.resolved_location.x = new_world_value, 1 => self.resolved_location.y = new_world_value, _ => self.resolved_location.z = new_world_value }
        true
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
