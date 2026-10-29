use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use crate::common::PointResolutionError;
use crate::editor::editable::{Feature, FeatureId, FeatureTrait, PointRef};
use crate::editor::mirror::Mirroring;
use crate::get;

const DEFAULT_INTENSITY: f32 = 10_000.0;
const DEFAULT_RADIUS: f32 = 0.1;
const DEFAULT_RANGE: f32 = 20.0;
/// In lumens. Logarithmic on the slider, since the useful values span orders of magnitude.
const MAX_INTENSITY: f32 = 1.0e7;

#[derive(Serialize, Deserialize)]
pub struct GracklePointLight {
    location: PointRef,
    pub intensity: f32,
    pub radius: f32,
    pub range: f32,
    #[serde(skip)]
    resolved_location: Vec3,
    #[serde(skip)]
    entity: Option<Entity>,
}

#[typetag::serde(name = "grackle_point_light")]
impl FeatureTrait for GracklePointLight {
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

        ui.separator();
        ui.label(get!("editor.features.grackle_point_light.params"));
        changed |= ui.add(egui::Slider::new(&mut self.intensity, 0.0..=MAX_INTENSITY).logarithmic(true).smallest_positive(1.0).text(get!("editor.features.grackle_point_light.intensity"))).changed();
        changed |= ui.add(egui::Slider::new(&mut self.radius, 0.0..=10.0).text(get!("editor.features.grackle_point_light.radius"))).changed();
        changed |= ui.add(egui::Slider::new(&mut self.range, 0.0..=100.0).text(get!("editor.features.grackle_point_light.range"))).changed();

        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.grackle_point_light.title")
    }

    fn tool(&self) -> Option<crate::tool::Tools> { Some(crate::tool::Tools::PointLight) }
    fn type_key(&self) -> &'static str { "grackle_point_light" }

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        gizmos.sphere(Isometry3d::from_translation(self.resolved_location), 0.2, Color::srgb_u8(255, 255, 0));
        self.location.debug_gizmos(self.resolved_location, gizmos);
    }

    fn selected_gizmos(&self, gizmos: &mut Gizmos) {
        gizmos.sphere(Isometry3d::from_translation(self.resolved_location), self.range, Color::srgb_u8(80, 140, 255))
            .resolution(64);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        commands.entity(entity).insert((
            Transform::from_translation(self.resolved_location),
            PointLight {
                intensity: self.intensity,
                radius: self.radius,
                range: self.range,
                ..default()
            },
        ));
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
        vec![("".into(), "Light".into())]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![("".into(), self.resolved_location)]
    }

    fn point_refs(&self) -> Vec<&PointRef> {
        vec![&self.location]
    }

    fn get_point_ref(&self, _key: &str) -> Option<&PointRef> {
        Some(&self.location)
    }

    fn get_point_ref_mut(&mut self, _key: &str) -> Option<&mut PointRef> {
        Some(&mut self.location)
    }

    fn drag_handle(&mut self, _is_max: bool, axis: u8, new_world_value: f32) -> bool {
        if !self.location.drag_axis(axis, new_world_value) {
            return false;
        }
        self.resolved_location[axis as usize] = new_world_value;
        true
    }

    fn mirror(&mut self, mirroring: &Mirroring) -> bool {
        self.resolved_location = mirroring.point(self.resolved_location);
        // Its reference is on the other side; a taxicab path from it would be nonsense.
        self.location.resolved_reference = None;
        true
    }
}

impl GracklePointLight {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self {
            location: PointRef::absolute(x, y, z),
            intensity: DEFAULT_INTENSITY,
            radius: DEFAULT_RADIUS,
            range: DEFAULT_RANGE,
            resolved_location: Vec3::new(x, y, z),
            entity: None,
        }
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            intensity: DEFAULT_INTENSITY,
            radius: DEFAULT_RADIUS,
            range: DEFAULT_RANGE,
            resolved_location: Vec3::ZERO,
            entity: None,
        }
    }
}
