//! A box that does something to whoever stands in it. Its box is a room's —
//! corners, handles, anchoring — but it is never asked for `rooms`, so it
//! carves nothing and is drawn only as a gizmo.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::PointResolutionError;
use crate::common::ray::PickBox;
use crate::common::trigger::{TriggerEffect, TriggerVolume};
use crate::editor::editable::{Feature, FeatureId, FeatureTrait, PointRef};
use crate::editor::editor_room::EditorRoom;
use crate::editor::mirror::Mirroring;
use crate::get;

pub const TRIGGER_COLOR: Color = Color::srgb(1.0, 0.55, 0.1);

#[derive(Serialize, Deserialize)]
pub struct EditorTrigger {
    volume: EditorRoom,
    effect: TriggerEffect,
}

impl EditorTrigger {
    pub fn from_room(volume: EditorRoom) -> Self {
        Self { volume, effect: TriggerEffect::default() }
    }

    pub fn with_effect(mut self, effect: TriggerEffect) -> Self {
        self.effect = effect;
        self
    }
}

#[typetag::serde(name = "trigger")]
impl FeatureTrait for EditorTrigger {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError> {
        self.volume.get_point(key)
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        let mut changed = false;
        let kill = matches!(self.effect, TriggerEffect::Kill);
        let name = |kill: bool| get!(if kill { "editor.features.trigger.kill" } else { "editor.features.trigger.hurt" });
        egui::ComboBox::from_label(get!("editor.features.trigger.effect"))
            .selected_text(name(kill))
            .show_ui(ui, |ui| {
                if ui.selectable_label(!kill, name(false)).clicked() && kill {
                    self.effect = TriggerEffect::default();
                    changed = true;
                }
                if ui.selectable_label(kill, name(true)).clicked() && !kill {
                    self.effect = TriggerEffect::Kill;
                    changed = true;
                }
            });
        if let TriggerEffect::Hurt { per_second } = &mut self.effect {
            changed |= ui
                .add(
                    egui::DragValue::new(per_second)
                        .speed(1.0)
                        .range(1..=1000)
                        .prefix(format!("{}: ", get!("editor.features.trigger.per_second"))),
                )
                .changed();
        }
        ui.separator();
        changed |= self.volume.corners_ui(ui, features, prior_feature_order, retarget_request);
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.trigger.title")
    }

    fn type_key(&self) -> &'static str { "trigger" }

    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        self.volume.draw_box(gizmos, TRIGGER_COLOR);
    }

    fn entity(&self) -> Option<Entity> {
        self.volume.entity()
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.volume.set_entity(entity);
    }

    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        let (min, max) = self.volume.bounds();
        commands.entity(entity).insert((
            Transform::from_translation((min + max) / 2.0),
            TriggerVolume { min, max, effect: self.effect },
        ));
    }

    fn resolve_references(&mut self, features: &HashMap<FeatureId, Feature>) {
        self.volume.resolve_references(features);
    }

    fn parent_ids(&self) -> Vec<FeatureId> {
        self.volume.parent_ids()
    }

    fn available_point_keys(&self) -> Vec<(String, String)> {
        self.volume.available_point_keys()
    }

    fn reference_points_for_ray(&self, ray: &Ray3d) -> Vec<(String, Vec3)> {
        self.volume.reference_points_for_ray(ray)
    }

    fn drag_handle(&mut self, is_max: bool, axis: u8, new_world_value: f32) -> bool {
        self.volume.drag_handle(is_max, axis, new_world_value)
    }

    fn pick_boxes(&self) -> Vec<PickBox> {
        let (min, max) = self.volume.bounds();
        vec![PickBox { center: (min + max) / 2.0, rotation: Quat::IDENTITY, half_extents: (max - min) / 2.0 }]
    }

    fn drag_handle_bounds(&self) -> Option<(Vec3, Vec3)> {
        Some(self.volume.bounds())
    }

    fn get_point_ref(&self, key: &str) -> Option<&PointRef> {
        self.volume.get_point_ref(key)
    }

    fn get_point_ref_mut(&mut self, key: &str) -> Option<&mut PointRef> {
        self.volume.get_point_ref_mut(key)
    }

    fn own_bases(&self, point_ref_key: &str) -> Vec<(String, Vec3)> {
        self.volume.own_bases(point_ref_key)
    }

    fn rebase_on_own(&mut self, point_ref_key: &str, base: &str) -> bool {
        self.volume.rebase_on_own(point_ref_key, base)
    }

    fn mirror(&mut self, mirroring: &Mirroring) -> bool {
        self.volume.mirror(mirroring)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Breaks if the trigger starts answering `rooms` through its box: it
    /// would carve a hole in every wall it touched.
    #[test]
    fn a_trigger_is_no_room() {
        let room = EditorRoom::from_point_refs(PointRef::absolute(0.0, 0.0, 0.0), PointRef::absolute(1.0, 1.0, 1.0));
        assert!(EditorTrigger::from_room(room).rooms().is_empty());
    }
}
