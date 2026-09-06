use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::rotation::quat_from_euler;
use crate::common::PointResolutionError;
use crate::editor::action::FeatureData;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
use crate::get;

/// The edge length of the stand-in box, in metres.
///
/// Props have no models yet, and a box that is exactly half a metre on a side
/// is a ruler as much as a placeholder: a mapper can count them along a wall
/// and know how big the room is before any art exists.
pub const PROP_SIZE: f32 = 0.5;

/// Marks the entity a [`Prop`] feature drives, so the mesh that stands in for
/// its model can be attached without the feature reaching for `Assets` — the
/// same arrangement `SpawnPoint` has with `SpawnPointMarker`.
#[derive(Component, Debug)]
pub struct PropMarker;

/// A placed piece of map furniture.
///
/// Its point is the **centre** of the box, not a corner and not the feet: a
/// prop is a thing you rotate, and rotating about anything but the centre
/// would slide it across the floor as it turned.
///
/// All three angles are exposed, unlike a spawn point's yaw — a crate on its
/// side is a legitimate thing to want, and a body standing sideways is not.
#[derive(Serialize, Deserialize)]
pub struct Prop {
    location: PointRef,
    /// Pitch, yaw and roll in radians, in axis order. See
    /// [`crate::common::rotation`] for how the three combine.
    rotation: Vec3,
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
        let label = get!("editor.features.prop.centre");
        changed |= self.location.editor_ui(ui, &label, features, prior_feature_order, retarget_request);

        ui.separator();
        ui.label(get!("editor.features.prop.rotation"));
        // Degrees in the boxes, radians in the field: a prop is lined up
        // against a wall in degrees, and everything downstream is trigonometry.
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

    fn snapshot(&self) -> FeatureData {
        FeatureData::Prop {
            location: self.location.clone(),
            rotation: self.rotation,
        }
    }

    fn apply_snapshot(&mut self, data: &FeatureData) {
        let FeatureData::Prop { location, rotation } = data else { return; };
        self.location = location.clone();
        self.rotation = *rotation;
    }

    /// The box, turned the way the prop is turned, with a stub out of its
    /// front face — without that there is no telling a yaw of 0 from one of
    /// 180 on a shape this symmetrical.
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

    fn point_ref_slots(&self) -> Vec<&str> { vec!["location"] }

    fn scalar_fields(&self) -> Vec<(&str, f32)> {
        vec![
            ("pitch", self.rotation.x),
            ("yaw", self.rotation.y),
            ("roll", self.rotation.z),
        ]
    }

    fn set_scalar_field(&mut self, key: &str, value: f32) {
        match key {
            "pitch" => self.rotation.x = value,
            "yaw" => self.rotation.y = value,
            "roll" => self.rotation.z = value,
            _ => {}
        }
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
            resolved_location: Vec3::new(x, y, z),
            entity: None,
        }
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            rotation: Vec3::ZERO,
            resolved_location: Vec3::ZERO,
            entity: None,
        }
    }

    pub fn with_rotation(mut self, rotation: Vec3) -> Self {
        self.rotation = rotation;
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

    /// The stand-in mesh is hung off [`PropMarker`], which only ever gets
    /// inserted here. Break the link and props are simply invisible — nothing
    /// fails, the map just quietly loses its furniture.
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

    /// Rotation goes to disk as three scalars, and has to come back as the
    /// same three. A silent mismatch here loses every prop's facing on load.
    #[test]
    fn rotation_round_trips_through_the_scalar_fields() {
        let prop = Prop::new(0.0, 0.0, 0.0).with_rotation(Vec3::new(0.1, 0.2, 0.3));
        let mut loaded = Prop::new(0.0, 0.0, 0.0);
        for (key, value) in prop.scalar_fields() {
            loaded.set_scalar_field(key, value);
        }
        assert_eq!(loaded.rotation(), Vec3::new(0.1, 0.2, 0.3));
    }
}
