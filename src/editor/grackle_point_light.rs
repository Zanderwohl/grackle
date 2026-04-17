use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use crate::common::PointResolutionError;
use crate::editor::editable::{EditorAction, EditorActionId, EditorObject, PointRef};
use crate::get;

const DEFAULT_INTENSITY: f32 = 10_000.0;
const DEFAULT_RADIUS: f32 = 0.1;
const DEFAULT_RANGE: f32 = 20.0;

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
impl EditorObject for GracklePointLight {
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

        ui.separator();
        ui.label(get!("editor.actions.grackle_point_light.params"));
        changed |= ui.add(egui::Slider::new(&mut self.intensity, 0.0..=10000.0).text(get!("editor.actions.grackle_point_light.intensity"))).changed();
        changed |= ui.add(egui::Slider::new(&mut self.radius, 0.0..=10.0).text(get!("editor.actions.grackle_point_light.radius"))).changed();
        changed |= ui.add(egui::Slider::new(&mut self.range, 0.0..=100.0).text(get!("editor.actions.grackle_point_light.range"))).changed();

        changed
    }

    fn type_name(&self) -> String {
        get!("editor.actions.grackle_point_light.title")
    }

    fn type_key(&self) -> &'static str { "grackle_point_light" }

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        gizmos.sphere(Isometry3d::from_translation(self.resolved_location), 0.2, Color::srgb_u8(255, 200, 50));
        self.location.debug_gizmos(self.resolved_location, gizmos);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    fn apply_to_entity(&self, commands: &mut Commands, entity: Entity) {
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

    fn resolve_references(&mut self, actions: &HashMap<EditorActionId, EditorAction>) {
        if let Ok(v) = self.location.resolve(actions) {
            self.resolved_location = v;
        }
    }

    fn parent_ids(&self) -> Vec<EditorActionId> {
        self.location.referenced_actions()
    }

    fn available_point_keys(&self) -> Vec<(String, String)> {
        vec![("".into(), "Light".into())]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![("".into(), self.resolved_location)]
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
