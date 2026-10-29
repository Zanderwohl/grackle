//! A weapon lying in the map for anybody to take. See "Weapon pickups" in
//! [documentation/arena.md](../../documentation/arena.md).
//!
//! The map names the weapon, not a model: the weapon already names its own,
//! so a pickup cannot disagree with `weapons.toml` about what a rifle looks
//! like. What stands in the world is [`crate::game::world_prop`]'s.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::picker::{picker_button, PickerKind};
use crate::common::weapon::WeaponId;
use crate::common::PointResolutionError;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
use crate::editor::mirror::Mirroring;
use crate::game::world_prop::WEAPON_HOVER;
use crate::get;
use crate::prop::archetype::PickupStyle;

/// The box drawn and picked in the editor, in meters: about a rifle.
pub const BOX: Vec3 = Vec3::new(0.25, 0.25, 0.8);

const DEFAULT_RESPAWN: f32 = 15.0;
const DEFAULT_AMMO: f32 = 0.5;

/// The catalog's weapons, by name, pictured by their models. Published by
/// [`crate::tool::weapon_pickup`], because a feature's panel has no world to
/// read the catalog from.
pub const WEAPONS: PickerKind = PickerKind::new("weapon");

/// On the feature's entity, where `crate::game::world_prop` reads it.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct WeaponPickupSpec {
    pub weapon: WeaponId,
    /// Fraction of the weapon's reserve given to somebody who already has it.
    pub ammo: f32,
    /// Seconds of game time before a taken one is back.
    pub respawn: f32,
    pub style: PickupStyle,
}

#[derive(Serialize, Deserialize)]
pub struct WeaponPickup {
    /// Where it stands on the floor.
    location: PointRef,
    /// Radians.
    #[serde(default)]
    yaw: f32,
    /// As `weapons.toml` names it. One nobody defines gives nothing and draws
    /// nothing.
    #[serde(default)]
    weapon: String,
    #[serde(default = "default_respawn")]
    respawn: f32,
    #[serde(default = "default_ammo")]
    ammo: f32,
    #[serde(default = "tf2")]
    style: PickupStyle,
    #[serde(skip)]
    resolved_location: Vec3,
    #[serde(skip)]
    entity: Option<Entity>,
}

fn default_respawn() -> f32 {
    DEFAULT_RESPAWN
}

fn default_ammo() -> f32 {
    DEFAULT_AMMO
}

fn tf2() -> PickupStyle {
    PickupStyle::Tf2
}

#[typetag::serde(name = "weapon_pickup")]
impl FeatureTrait for WeaponPickup {
    fn get_point(&self, _key: &str) -> Result<Vec3, PointResolutionError> {
        Ok(self.resolved_location)
    }

    fn editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        features: &HashMap<FeatureId, Feature>,
        prior_feature_order: &[FeatureId],
        retarget_request: &mut Option<String>,
    ) -> bool {
        let mut changed = false;

        ui.horizontal(|ui| {
            ui.label(get!("editor.features.weapon_pickup.weapon"));
            if let Some(Some(pick)) = picker_button(ui, WEAPONS, Some(&self.weapon), None) {
                changed |= pick != self.weapon;
                self.weapon = pick;
            }
        });

        let label = get!("editor.features.weapon_pickup.feet");
        changed |= self.location.editor_ui(ui, &label, features, prior_feature_order, retarget_request);

        let mut degrees = self.yaw.to_degrees();
        if ui
            .add(
                egui::DragValue::new(&mut degrees)
                    .speed(1.0)
                    .prefix(format!("{}: ", get!("editor.features.weapon_pickup.facing")))
                    .suffix("\u{b0}"),
            )
            .changed()
        {
            self.yaw = degrees.to_radians();
            changed = true;
        }

        changed |= ui
            .add(
                egui::DragValue::new(&mut self.respawn)
                    .speed(0.5)
                    .range(0.0..=600.0)
                    .prefix(format!("{}: ", get!("editor.features.weapon_pickup.respawn")))
                    .suffix(" s"),
            )
            .changed();

        let mut percent = self.ammo * 100.0;
        if ui
            .add(
                egui::DragValue::new(&mut percent)
                    .speed(1.0)
                    .range(0.0..=100.0)
                    .prefix(format!("{}: ", get!("editor.features.weapon_pickup.ammo")))
                    .suffix("%"),
            )
            .changed()
        {
            self.ammo = percent / 100.0;
            changed = true;
        }

        egui::ComboBox::from_label(get!("editor.features.weapon_pickup.style"))
            .selected_text(self.style.name())
            .show_ui(ui, |ui| {
                for style in PickupStyle::ALL {
                    if ui.selectable_label(self.style == style, style.name()).clicked() && self.style != style {
                        self.style = style;
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
        get!("editor.features.weapon_pickup.title")
    }

    fn type_key(&self) -> &'static str {
        "weapon_pickup"
    }

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let orange = Color::srgb_u8(240, 170, 60);
        let rotation = Quat::from_rotation_y(self.yaw);
        let center = self.box_center();
        gizmos.cube(Transform::from_translation(center).with_rotation(rotation).with_scale(BOX), orange);
        gizmos.line(self.resolved_location, center - Vec3::Y * (BOX.y * 0.5), orange);
        let front = rotation * Vec3::NEG_Z;
        gizmos.arrow(center, center + front * BOX.z, orange);
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
            Transform::from_translation(self.resolved_location).with_rotation(Quat::from_rotation_y(self.yaw)),
            self.spec(),
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
        vec![("".into(), get!("editor.features.weapon_pickup.feet"))]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![("".into(), self.resolved_location)]
    }

    fn pick_boxes(&self) -> Vec<crate::common::ray::PickBox> {
        vec![crate::common::ray::PickBox {
            center: self.box_center(),
            rotation: Quat::from_rotation_y(self.yaw),
            half_extents: BOX * 0.5,
        }]
    }

    fn rotation_axes(&self) -> Vec<u8> {
        vec![1]
    }

    fn euler_angles(&self) -> Vec3 {
        Vec3::new(0.0, self.yaw, 0.0)
    }

    fn set_euler_angle(&mut self, axis: u8, radians: f32) -> bool {
        if axis != 1 {
            return false;
        }
        self.yaw = radians;
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
        match axis {
            0 => self.resolved_location.x = new_world_value,
            1 => self.resolved_location.y = new_world_value,
            _ => self.resolved_location.z = new_world_value,
        }
        true
    }

    fn mirror(&mut self, mirroring: &Mirroring) -> bool {
        self.resolved_location = mirroring.point(self.resolved_location);
        self.yaw = mirroring.yaw(self.yaw);
        self.location.resolved_reference = None;
        true
    }
}

impl WeaponPickup {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self::from_point_ref(PointRef::absolute(x, y, z)).at(Vec3::new(x, y, z))
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            yaw: 0.0,
            weapon: String::new(),
            respawn: DEFAULT_RESPAWN,
            ammo: DEFAULT_AMMO,
            style: PickupStyle::Tf2,
            resolved_location: Vec3::ZERO,
            entity: None,
        }
    }

    fn at(mut self, resolved: Vec3) -> Self {
        self.resolved_location = resolved;
        self
    }

    pub fn with_weapon(mut self, weapon: impl Into<String>) -> Self {
        self.weapon = weapon.into();
        self
    }

    pub fn with_yaw(mut self, yaw: f32) -> Self {
        self.yaw = yaw;
        self
    }

    pub fn spec(&self) -> WeaponPickupSpec {
        WeaponPickupSpec {
            weapon: WeaponId::of(&self.weapon),
            ammo: self.ammo.clamp(0.0, 1.0),
            respawn: self.respawn.max(0.0),
            style: self.style,
        }
    }

    /// Hovering over its point, where the weapon is drawn.
    pub fn box_center_over(feet: Vec3) -> Vec3 {
        feet + Vec3::Y * (WEAPON_HOVER + BOX.y * 0.5)
    }

    fn box_center(&self) -> Vec3 {
        Self::box_center_over(self.resolved_location)
    }
}
