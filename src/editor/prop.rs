use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::rotation::quat_from_euler;
use crate::common::PointResolutionError;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
use crate::get;

/// Edge length of the stand-in box, in metres. Exactly half a metre so mappers can count them.
pub const PROP_SIZE: f32 = 0.5;

/// Marks the entity a [`Prop`] feature drives, so its mesh can be attached without the feature
/// reaching for `Assets`.
#[derive(Component, Debug)]
pub struct PropMarker;

/// The model a [`Prop`] names, empty for the stand-in cube. On the feature's
/// entity, where `crate::game::world_prop` reads it.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub struct PropModel(pub String);

/// A placed piece of map furniture. Its point is the centre of the box, so rotating does not slide
/// it across the floor.
#[derive(Serialize, Deserialize)]
pub struct Prop {
    location: PointRef,
    /// Pitch, yaw and roll in radians, in axis order. See [`crate::common::rotation`].
    rotation: Vec3,
    /// A prop under a pack's `props/`, by file stem. Empty draws the stand-in
    /// cube. Not checked here: a bad name draws nothing, as a weapon's does.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    model: String,
    #[serde(skip)]
    resolved_location: Vec3,
    #[serde(skip)]
    entity: Option<Entity>,
}

#[typetag::serde(name = "prop")]
impl FeatureTrait for Prop {
    fn get_point(&self, _key: &str) -> Result<Vec3, PointResolutionError> {
        Ok(self.resolved_location)
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(get!("editor.features.prop.model"));
            changed |= ui.text_edit_singleline(&mut self.model).changed();
        });
        let label = get!("editor.features.prop.centre");
        changed |= self.location.editor_ui(ui, &label, features, prior_feature_order, retarget_request);

        ui.separator();
        ui.label(get!("editor.features.prop.rotation"));
        // Degrees in the UI, radians stored.
        let rows: [(usize, String); 3] = [
            (1, get!("editor.features.prop.yaw")),
            (0, get!("editor.features.prop.pitch")),
            (2, get!("editor.features.prop.roll")),
        ];
        for (axis, label) in rows {
            let mut degrees = self.rotation[axis].to_degrees();
            if ui
                .add(
                    egui::DragValue::new(&mut degrees)
                        .speed(1.0)
                        .prefix(format!("{label}: "))
                        .suffix("\u{b0}"),
                )
                .changed()
            {
                self.rotation[axis] = degrees.to_radians();
                changed = true;
            }
        }

        if changed {
            if let Ok(v) = self.location.resolve(features) {
                self.resolved_location = v;
            }
        }
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.prop.title")
    }

    fn type_key(&self) -> &'static str { "prop" }

    /// The stub out of the front face tells a yaw of 0 from 180.
    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let rotation = quat_from_euler(self.rotation);
        let teal = Color::srgb_u8(90, 220, 200);

        gizmos.cube(
            Transform::from_translation(self.resolved_location)
                .with_rotation(rotation)
                .with_scale(Vec3::splat(PROP_SIZE)),
            teal,
        );

        let front = rotation * Vec3::NEG_Z;
        gizmos.arrow(
            self.resolved_location,
            self.resolved_location + front * PROP_SIZE,
            teal,
        );

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
            Transform::from_translation(self.resolved_location)
                .with_rotation(quat_from_euler(self.rotation)),
            PropMarker,
            PropModel(self.model.clone()),
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
        vec![("".into(), get!("editor.features.prop.centre"))]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![("".into(), self.resolved_location)]
    }

    /// The placeholder cube [`crate::tool::prop`] puts on the entity.
    fn pick_boxes(&self) -> Vec<crate::common::ray::PickBox> {
        vec![crate::common::ray::PickBox {
            centre: self.resolved_location,
            rotation: quat_from_euler(self.rotation),
            half_extents: Vec3::splat(PROP_SIZE * 0.5),
        }]
    }

    fn rotation_axes(&self) -> Vec<u8> { vec![0, 1, 2] }

    fn euler_angles(&self) -> Vec3 { self.rotation }

    fn set_euler_angle(&mut self, axis: u8, radians: f32) -> bool {
        if axis > 2 {
            return false;
        }
        self.rotation[axis as usize] = radians;
        true
    }

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
        match axis {
            0 => self.resolved_location.x = new_world_value,
            1 => self.resolved_location.y = new_world_value,
            _ => self.resolved_location.z = new_world_value,
        }
        true
    }
}

impl Prop {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self {
            location: PointRef::absolute(x, y, z),
            rotation: Vec3::ZERO,
            model: String::new(),
            resolved_location: Vec3::new(x, y, z),
            entity: None,
        }
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            rotation: Vec3::ZERO,
            model: String::new(),
            resolved_location: Vec3::ZERO,
            entity: None,
        }
    }

    pub fn with_rotation(mut self, rotation: Vec3) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Pitch, yaw and roll in radians, in axis order.
    pub fn rotation(&self) -> Vec3 {
        self.rotation
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// Props are invisible without [`PropMarker`], which only this inserts.
    #[test]
    fn applying_a_prop_marks_and_turns_its_entity() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();

        world
            .run_system_once(move |mut commands: Commands| {
                Prop::new(1.0, 2.0, 3.0)
                    .with_rotation(Vec3::new(0.0, std::f32::consts::FRAC_PI_2, 0.0))
                    .apply_to_entity(&mut commands, entity);
            })
            .unwrap();

        assert!(world.get::<PropMarker>(entity).is_some(), "no mesh will ever be attached");
        let transform = world.get::<Transform>(entity).unwrap();
        assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
        assert!(
            transform
                .rotation
                .abs_diff_eq(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), 1e-5),
            "the prop's yaw never reached the entity"
        );
    }

}
