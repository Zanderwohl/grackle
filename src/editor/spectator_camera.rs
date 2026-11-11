//! A place a spectator may look out of: fixed, at its own field of view, for
//! the seats it admits. Stepped through by [`crate::game::spectate`] after
//! every living body, by [`SpectatorCamera::order`] and then feature id.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::rotation::{euler_from_quat, quat_from_euler};
use crate::common::team::Team;
use crate::common::PointResolutionError;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
use crate::editor::mirror::Mirroring;
use crate::game::respawn::Seat;
use crate::get;

pub const DEFAULT_ORDER: u32 = 1000;
/// Degrees, vertical.
pub const DEFAULT_FOV: f32 = 90.0;
/// Degrees, vertical.
pub const FOV_RANGE: std::ops::RangeInclusive<f32> = 10.0..=150.0;

const COLOR: Color = Color::srgb(0.75, 0.45, 0.95);
/// Meters: how far out the gizmo's frustum is drawn.
const FRUSTUM_LENGTH: f32 = 0.8;
const FRUSTUM_ASPECT: f32 = 16.0 / 9.0;
/// Half-extents of the body box, in the camera's own frame.
const BODY: Vec3 = Vec3::new(0.12, 0.12, 0.2);

/// Which seats may look through a camera. Choosing and spectating are
/// unassigned; playing with no team is free for all.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct Viewers {
    pub unassigned: bool,
    pub free_for_all: bool,
    pub red: bool,
    pub blue: bool,
    pub yellow: bool,
    pub green: bool,
}

impl Default for Viewers {
    fn default() -> Self {
        Self { unassigned: true, free_for_all: true, red: true, blue: true, yellow: true, green: true }
    }
}

impl Viewers {
    pub fn admits(&self, seat: Seat) -> bool {
        match seat {
            Seat::Choosing | Seat::Spectating => self.unassigned,
            Seat::Playing(None) => self.free_for_all,
            Seat::Playing(Some(team)) => *self.team(team),
        }
    }

    fn team(&self, team: Team) -> &bool {
        match team {
            Team::Red => &self.red,
            Team::Blue => &self.blue,
            Team::Yellow => &self.yellow,
            Team::Green => &self.green,
        }
    }

    fn rows(&mut self) -> [(&mut bool, String); 6] {
        [
            (&mut self.unassigned, get!("editor.features.spectator_camera.unassigned")),
            (&mut self.free_for_all, get!("editor.features.spectator_camera.free_for_all")),
            (&mut self.red, Team::Red.name()),
            (&mut self.blue, Team::Blue.name()),
            (&mut self.yellow, Team::Yellow.name()),
            (&mut self.green, Team::Green.name()),
        ]
    }
}

/// On every machine, since cameras are map content: a client lists the same ones.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct SpectatorCameraSpec {
    /// Radians, vertical.
    pub fov: f32,
    pub order: u32,
    pub viewers: Viewers,
}

#[derive(Serialize, Deserialize)]
pub struct SpectatorCamera {
    location: PointRef,
    /// Pitch, yaw and roll in radians, in axis order. See [`crate::common::rotation`].
    rotation: Vec3,
    /// Degrees, vertical.
    #[serde(default = "default_fov")]
    fov: f32,
    /// Lower comes first among the cameras; ties go by feature id.
    #[serde(default = "default_order")]
    order: u32,
    #[serde(default)]
    viewers: Viewers,
    #[serde(skip)]
    resolved_location: Vec3,
    #[serde(skip)]
    entity: Option<Entity>,
}

fn default_fov() -> f32 {
    DEFAULT_FOV
}

fn default_order() -> u32 {
    DEFAULT_ORDER
}

#[typetag::serde(name = "spectator_camera")]
impl FeatureTrait for SpectatorCamera {
    fn get_point(&self, _key: &str) -> Result<Vec3, PointResolutionError> {
        Ok(self.resolved_location)
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        let label = get!("editor.features.spectator_camera.lens");
        let mut changed = self.location.editor_ui(ui, &label, features, prior_feature_order, retarget_request);

        ui.separator();
        ui.label(get!("editor.features.spectator_camera.rotation"));
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
        changed |= ui
            .add(
                egui::DragValue::new(&mut self.fov)
                    .speed(0.5)
                    .range(FOV_RANGE)
                    .prefix(format!("{}: ", get!("editor.features.spectator_camera.fov")))
                    .suffix("\u{b0}"),
            )
            .changed();
        changed |= ui
            .add(egui::DragValue::new(&mut self.order).speed(1.0).prefix(format!("{}: ", get!("editor.features.spectator_camera.order"))))
            .changed();

        ui.separator();
        ui.label(get!("editor.features.spectator_camera.viewers"));
        for (admitted, label) in self.viewers.rows() {
            changed |= ui.checkbox(admitted, label).changed();
        }

        if changed && let Ok(v) = self.location.resolve(features) {
            self.resolved_location = v;
        }
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.spectator_camera.title")
    }

    fn tool(&self) -> Option<crate::tool::Tools> {
        Some(crate::tool::Tools::SpectatorCamera)
    }

    fn type_key(&self) -> &'static str {
        "spectator_camera"
    }

    /// The frustum looks out of `-Z`; the tick on its top edge shows roll.
    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let rotation = quat_from_euler(self.rotation);
        let at = self.resolved_location;
        gizmos.cube(Transform::from_translation(at + rotation * Vec3::Z * BODY.z).with_rotation(rotation).with_scale(BODY * 2.0), COLOR);

        let corners = frustum_corners(self.fov_radians(), FRUSTUM_LENGTH).map(|corner| at + rotation * corner);
        for (index, corner) in corners.iter().enumerate() {
            gizmos.line(at, *corner, COLOR);
            gizmos.line(*corner, corners[(index + 1) % 4], COLOR);
        }
        let top = (corners[0] + corners[1]) * 0.5;
        gizmos.line(top, top + rotation * Vec3::Y * (FRUSTUM_LENGTH * 0.15), COLOR);

        self.location.debug_gizmos(self.resolved_location, gizmos);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        commands.entity(entity).insert((
            Transform::from_translation(self.resolved_location).with_rotation(quat_from_euler(self.rotation)),
            SpectatorCameraSpec { fov: self.fov_radians(), order: self.order, viewers: self.viewers },
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
        vec![("".into(), get!("editor.features.spectator_camera.lens"))]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![("".into(), self.resolved_location)]
    }

    fn pick_boxes(&self) -> Vec<crate::common::ray::PickBox> {
        let rotation = quat_from_euler(self.rotation);
        vec![crate::common::ray::PickBox {
            center: self.resolved_location + rotation * Vec3::Z * BODY.z,
            rotation,
            half_extents: BODY,
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

    /// Turned, never reflected, as a prop is. Viewers are not swapped between teams.
    fn mirror(&mut self, mirroring: &Mirroring) -> bool {
        self.resolved_location = mirroring.point(self.resolved_location);
        self.rotation = euler_from_quat(mirroring.rotation(quat_from_euler(self.rotation)));
        self.location.resolved_reference = None;
        true
    }
}

/// The far rectangle of a frustum `length` out of `-Z`, clockwise from top
/// left seen from behind.
fn frustum_corners(fov: f32, length: f32) -> [Vec3; 4] {
    let up = length * (fov * 0.5).tan();
    let right = up * FRUSTUM_ASPECT;
    [
        Vec3::new(-right, up, -length),
        Vec3::new(right, up, -length),
        Vec3::new(right, -up, -length),
        Vec3::new(-right, -up, -length),
    ]
}

impl SpectatorCamera {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self::from_point_ref(PointRef::absolute(x, y, z)).at(Vec3::new(x, y, z))
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            rotation: Vec3::ZERO,
            fov: DEFAULT_FOV,
            order: DEFAULT_ORDER,
            viewers: Viewers::default(),
            resolved_location: Vec3::ZERO,
            entity: None,
        }
    }

    /// Clamped here too: a blueprint is text, and a width of 0 or past 180 is
    /// a degenerate projection.
    fn fov_radians(&self) -> f32 {
        self.fov.clamp(*FOV_RANGE.start(), *FOV_RANGE.end()).to_radians()
    }

    fn at(mut self, resolved: Vec3) -> Self {
        self.resolved_location = resolved;
        self
    }

    pub fn with_rotation(mut self, rotation: Vec3) -> Self {
        self.rotation = rotation;
        self
    }

    /// Degrees.
    pub fn with_fov(mut self, fov: f32) -> Self {
        self.fov = fov;
        self
    }

    pub fn with_order(mut self, order: u32) -> Self {
        self.order = order;
        self
    }

    pub fn with_viewers(mut self, viewers: Viewers) -> Self {
        self.viewers = viewers;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_seat_is_admitted_by_default_and_each_box_shuts_out_its_own() {
        let seats = [
            Seat::Choosing,
            Seat::Spectating,
            Seat::Playing(None),
            Seat::Playing(Some(Team::Red)),
            Seat::Playing(Some(Team::Blue)),
            Seat::Playing(Some(Team::Yellow)),
            Seat::Playing(Some(Team::Green)),
        ];
        assert!(seats.iter().all(|seat| Viewers::default().admits(*seat)));

        let only_red = Viewers { red: false, ..default() };
        let shut: Vec<Seat> = seats.into_iter().filter(|seat| !only_red.admits(*seat)).collect();
        assert_eq!(shut, vec![Seat::Playing(Some(Team::Red))]);

        let no_one_unassigned = Viewers { unassigned: false, ..default() };
        assert!(!no_one_unassigned.admits(Seat::Choosing) && !no_one_unassigned.admits(Seat::Spectating));
        assert!(no_one_unassigned.admits(Seat::Playing(None)), "free for all is its own box");
    }

    #[test]
    fn a_width_out_of_range_reaches_the_view_clamped() {
        for (fov, wanted) in [(0.0, *FOV_RANGE.start()), (500.0, *FOV_RANGE.end())] {
            let got = SpectatorCamera::new(0.0, 0.0, 0.0).with_fov(fov).fov_radians();
            assert!((got - wanted.to_radians()).abs() < 1e-6, "{fov} came out as {got}");
        }
    }

    /// A camera from before a field existed still reads, at that field's default.
    #[test]
    fn a_camera_with_only_a_place_reads_at_the_defaults() {
        let text = r#"(location: (reference: None, point_key: "", x: Absolute(1.0), y: Absolute(2.0), z: Absolute(3.0)), rotation: (0.0, 0.0, 0.0))"#;
        let camera: SpectatorCamera = ron::from_str(text).unwrap();
        assert_eq!((camera.fov, camera.order, camera.viewers), (DEFAULT_FOV, DEFAULT_ORDER, Viewers::default()));
    }
}
