//! A mark the mapper puts on the map's surface: grime, a sign, a stripe. Stands up a
//! [`MapDecalSpec`] on every machine, which [`crate::game::decal`] cuts from the walls.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::decal::Projection;
use crate::common::rotation::{euler_from_quat, quat_from_euler};
use crate::common::PointResolutionError;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
use crate::editor::mirror::Mirroring;
use crate::get;

const COLOR: Color = Color::srgb(0.95, 0.75, 0.3);
/// Meters, each side.
pub const SIZE_RANGE: std::ops::RangeInclusive<f32> = 0.05..=20.0;
pub const DEPTH_RANGE: std::ops::RangeInclusive<f32> = 0.01..=5.0;
pub const DEFAULT_PICTURE: &str = "scorch_1.png";

#[derive(Component, Clone, Debug, PartialEq)]
pub struct MapDecalSpec {
    pub projection: Projection,
    /// A file under a pack's `decals/`.
    pub picture: String,
}

#[derive(Serialize, Deserialize)]
pub struct EditorDecal {
    location: PointRef,
    /// Pitch, yaw and roll in radians, in axis order. Unturned, the picture faces `+Z` and is
    /// pressed onto what is behind it along `-Z`.
    rotation: Vec3,
    /// Meters, width and height.
    size: Vec2,
    /// Meters, the whole depth of the box the picture is pressed through.
    depth: f32,
    picture: String,
    #[serde(skip)]
    resolved_location: Vec3,
    #[serde(skip)]
    entity: Option<Entity>,
}

#[typetag::serde(name = "decal")]
impl FeatureTrait for EditorDecal {
    fn resources_named(&self) -> Vec<crate::common::resource::Named> {
        vec![crate::common::resource::Named::Decal(self.picture.clone())]
    }

    fn get_point(&self, _key: &str) -> Result<Vec3, PointResolutionError> {
        Ok(self.resolved_location)
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        let label = get!("editor.features.decal.center");
        let mut changed = self.location.editor_ui(ui, &label, features, prior_feature_order, retarget_request);

        ui.separator();
        ui.label(get!("editor.features.decal.rotation"));
        let rows: [(usize, String); 3] = [
            (1, get!("editor.features.prop.yaw")),
            (0, get!("editor.features.prop.pitch")),
            (2, get!("editor.features.prop.roll")),
        ];
        for (axis, label) in rows {
            let mut degrees = self.rotation[axis].to_degrees();
            if ui.add(egui::DragValue::new(&mut degrees).speed(1.0).prefix(format!("{label}: ")).suffix("\u{b0}")).changed() {
                self.rotation[axis] = degrees.to_radians();
                changed = true;
            }
        }

        ui.separator();
        let meters = |ui: &mut egui::Ui, value: &mut f32, label: String, range: std::ops::RangeInclusive<f32>| {
            ui.add(egui::DragValue::new(value).speed(0.01).range(range).prefix(format!("{label}: ")).suffix(" m")).changed()
        };
        changed |= meters(ui, &mut self.size.x, get!("editor.features.decal.width"), SIZE_RANGE);
        changed |= meters(ui, &mut self.size.y, get!("editor.features.decal.height"), SIZE_RANGE);
        changed |= meters(ui, &mut self.depth, get!("editor.features.decal.depth"), DEPTH_RANGE);

        ui.separator();
        egui::ComboBox::from_label(get!("editor.features.decal.picture")).selected_text(&self.picture).show_ui(ui, |ui| {
            for name in pictures_on_disk() {
                if ui.selectable_label(self.picture == name, &name).clicked() && self.picture != name {
                    self.picture = name;
                    changed = true;
                }
            }
        });

        if changed && let Ok(v) = self.location.resolve(features) {
            self.resolved_location = v;
        }
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.decal.title")
    }

    fn tool(&self) -> Option<crate::tool::Tools> {
        Some(crate::tool::Tools::Decal)
    }

    fn type_key(&self) -> &'static str {
        "decal"
    }

    /// The picture's outline, a tick toward its top, and an arrow the way it is pressed.
    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let projection = self.projection();
        let (right, up, out) = projection.frame();
        let (w, h) = (right * projection.size.x * 0.5, up * projection.size.y * 0.5);
        let c = projection.center;
        let corners = [c - w + h, c + w + h, c + w - h, c - w - h];
        for i in 0..4 {
            gizmos.line(corners[i], corners[(i + 1) % 4], COLOR);
        }
        gizmos.line(c + h, c + h * 1.2, COLOR);
        gizmos.arrow(c + out * projection.depth * 0.5, c - out * projection.depth * 0.5, COLOR);
        self.location.debug_gizmos(self.resolved_location, gizmos);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        commands.entity(entity).insert(MapDecalSpec { projection: self.projection(), picture: self.picture.clone() });
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
        vec![("".into(), get!("editor.features.decal.center"))]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![("".into(), self.resolved_location)]
    }

    fn pick_boxes(&self) -> Vec<crate::common::ray::PickBox> {
        let rotation = quat_from_euler(self.rotation);
        vec![crate::common::ray::PickBox {
            center: self.resolved_location,
            rotation,
            half_extents: Vec3::new(self.size.x * 0.5, self.size.y * 0.5, 0.02),
        }]
    }

    fn rotation_axes(&self) -> Vec<u8> {
        vec![0, 1, 2]
    }

    fn euler_angles(&self) -> Vec3 {
        self.rotation
    }

    fn set_euler_angle(&mut self, axis: u8, radians: f32) -> bool {
        if axis > 2 {
            return false;
        }
        self.rotation[axis as usize] = radians;
        true
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
        let axis_ref = match axis {
            0 => &mut self.location.x,
            1 => &mut self.location.y,
            2 => &mut self.location.z,
            _ => return false,
        };
        let base = self.location.resolved_reference.map(|b| match axis {
            0 => b.x,
            1 => b.y,
            _ => b.z,
        });
        match axis_ref {
            AxisRef::Absolute(v) => *v = new_world_value,
            AxisRef::Relative(offset) => *offset = new_world_value - base.unwrap_or(0.0),
        }
        self.resolved_location[axis as usize] = new_world_value;
        true
    }

    /// Turned, never reflected, so a mirrored sign still reads the right way round.
    fn mirror(&mut self, mirroring: &Mirroring) -> bool {
        self.resolved_location = mirroring.point(self.resolved_location);
        self.rotation = euler_from_quat(mirroring.rotation(quat_from_euler(self.rotation)));
        self.location.resolved_reference = None;
        true
    }
}

impl EditorDecal {
    /// On the floor: pressed straight down.
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        let mut decal = Self::from_point_ref(PointRef::absolute(x, y, z));
        decal.resolved_location = Vec3::new(x, y, z);
        decal
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            rotation: Vec3::new(-std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            size: Vec2::ONE,
            depth: 0.5,
            picture: DEFAULT_PICTURE.into(),
            resolved_location: Vec3::ZERO,
            entity: None,
        }
    }

    pub fn with_rotation(mut self, rotation: Vec3) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_picture(mut self, picture: &str) -> Self {
        self.picture = picture.into();
        self
    }

    /// Clamped here too: a blueprint is text.
    fn projection(&self) -> Projection {
        let rotation = quat_from_euler(self.rotation);
        Projection {
            center: self.resolved_location,
            normal: rotation * Vec3::Z,
            up: rotation * Vec3::Y,
            size: self.size.clamp(Vec2::splat(*SIZE_RANGE.start()), Vec2::splat(*SIZE_RANGE.end())),
            depth: self.depth.clamp(*DEPTH_RANGE.start(), *DEPTH_RANGE.end()),
        }
    }
}

/// Every PNG under every pack's `decals/`, for the picker. Authoring only: nothing that plays reads
/// a directory.
fn pictures_on_disk() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    #[cfg(not(target_arch = "wasm32"))]
    for pack in crate::common::lang::default_packs() {
        let Ok(entries) = std::fs::read_dir(pack.join("decals")) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".png") && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_decal_presses_onto_the_floor_below_it() {
        let projection = EditorDecal::new(1.0, 2.0, 3.0).projection();
        assert!(projection.normal.abs_diff_eq(Vec3::Y, 1e-6), "{}", projection.normal);
        assert_eq!(projection.center, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn the_default_picture_ships() {
        assert!(pictures_on_disk().iter().any(|name| name == DEFAULT_PICTURE));
    }
}
