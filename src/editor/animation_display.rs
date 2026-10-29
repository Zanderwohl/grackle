//! A skeleton stood on the map holding one animation state, for judging an animation at the size
//! and distance a player sees it.
//!
//! The body is an ordinary entity animated by the same systems as a player; the state is applied
//! through [`ForcedAnimation`] rather than a second animation path, so the display cannot disagree
//! with the game.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use crate::common::skeleton::{
    default_humanoid, AnimationState, ForcedAnimation, Pose, SkeletonAnimator,
};
use crate::common::PointResolutionError;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
use crate::common::damage::Damageable;
use crate::common::hitbox::Hitboxes;
use crate::get;

/// Marks the entity an [`AnimationDisplay`] drives. Hitboxes and health are required so hits can be
/// tested against a held state.
#[derive(Component, Debug)]
#[require(Hitboxes, Damageable)]
pub struct AnimationDisplayMarker;

#[derive(Serialize, Deserialize)]
pub struct AnimationDisplay {
    /// The feet, as with a spawn point.
    location: PointRef,
    /// Radians about world Y.
    yaw: f32,
    state: AnimationState,
    #[serde(skip)]
    resolved_location: Vec3,
    #[serde(skip)]
    entity: Option<Entity>,
}

#[typetag::serde(name = "animation_display")]
impl FeatureTrait for AnimationDisplay {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError> {
        match key {
            "head" => Ok(self.resolved_location + Vec3::Y * default_humanoid().proportions().height),
            _ => Ok(self.resolved_location),
        }
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        let mut changed = false;
        let label = get!("editor.features.animation_display.feet");
        changed |= self.location.editor_ui(ui, &label, features, prior_feature_order, retarget_request);

        let mut degrees = self.yaw.to_degrees();
        if ui
            .add(
                egui::DragValue::new(&mut degrees)
                    .speed(1.0)
                    .prefix(format!("{}: ", get!("editor.features.animation_display.facing")))
                    .suffix("\u{b0}"),
            )
            .changed()
        {
            self.yaw = degrees.to_radians();
            changed = true;
        }

        // Listed from the state machine, so no state is unselectable.
        egui::ComboBox::from_label(get!("editor.features.animation_display.state"))
            .selected_text(self.state.name())
            .show_ui(ui, |ui| {
                for state in AnimationState::iter() {
                    if ui.selectable_label(self.state == state, state.name()).clicked() {
                        if self.state != state {
                            self.state = state;
                            changed = true;
                        }
                    }
                }
            });

        if changed {
            if let Ok(v) = self.location.resolve(features) {
                self.resolved_location = v;
            }
        }
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.animation_display.title")
    }

    fn type_key(&self) -> &'static str { "animation_display" }

    /// The point and facing only; the body is drawn by the skeleton systems.
    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let feet = self.resolved_location;
        let violet = Color::srgb_u8(190, 140, 255);

        gizmos.sphere(Isometry3d::from_translation(feet), 0.2, violet);
        let facing = Quat::from_rotation_y(self.yaw) * Vec3::NEG_Z;
        gizmos.arrow(feet, feet + facing, violet);

        self.location.debug_gizmos(feet, gizmos);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    /// A body like the game's; [`ForcedAnimation`] pins it to the chosen state.
    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        commands
            .entity(entity)
            .insert((
                Transform::from_translation(self.resolved_location)
                    .with_rotation(Quat::from_rotation_y(self.yaw)),
                AnimationDisplayMarker,
                ForcedAnimation(self.state),
            ))
            // `insert_if_new`: this runs on every edit, and a re-inserted rig rebuilds the mesh every frame
            // of a drag.
            .insert_if_new((
                default_humanoid().clone(),
                Pose::rest(),
                SkeletonAnimator::default(),
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
        vec![
            ("".into(), get!("editor.features.animation_display.feet")),
            ("head".into(), get!("editor.features.animation_display.head")),
        ]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![
            ("".into(), self.resolved_location),
            ("head".into(), self.resolved_location + Vec3::Y * default_humanoid().proportions().height),
        ]
    }

    fn pick_boxes(&self) -> Vec<crate::common::ray::PickBox> {
        vec![crate::common::ray::PickBox::standing_body(self.resolved_location, Quat::from_rotation_y(self.yaw))]
    }

    fn rotation_axes(&self) -> Vec<u8> { vec![1] }

    fn euler_angles(&self) -> Vec3 { Vec3::new(0.0, self.yaw, 0.0) }

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

impl AnimationDisplay {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self {
            location: PointRef::absolute(x, y, z),
            yaw: 0.0,
            state: AnimationState::default(),
            resolved_location: Vec3::new(x, y, z),
            entity: None,
        }
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            yaw: 0.0,
            state: AnimationState::default(),
            resolved_location: Vec3::ZERO,
            entity: None,
        }
    }

    pub fn with_state(mut self, state: AnimationState) -> Self {
        self.state = state;
        self
    }

    pub fn with_yaw(mut self, yaw: f32) -> Self {
        self.yaw = yaw;
        self
    }

    pub fn state(&self) -> AnimationState {
        self.state
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// Without the link the display stands idle whatever state was picked.
    #[test]
    fn applying_a_display_makes_a_body_pinned_to_its_state() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();

        world
            .run_system_once(move |mut commands: Commands| {
                AnimationDisplay::new(1.0, 0.0, 3.0)
                    .with_state(AnimationState::RunForward)
                    .with_yaw(std::f32::consts::FRAC_PI_2)
                    .apply_to_entity(&mut commands, entity);
            })
            .unwrap();

        assert!(world.get::<AnimationDisplayMarker>(entity).is_some());
        assert!(world.get::<crate::common::skeleton::Skeleton>(entity).is_some(), "nothing to draw");
        assert!(world.get::<SkeletonAnimator>(entity).is_some(), "nothing to animate it");
        let forced = world.get::<ForcedAnimation>(entity).expect("the chosen state never arrived");
        assert_eq!(forced.0, AnimationState::RunForward);

        let transform = world.get::<Transform>(entity).unwrap();
        assert_eq!(transform.translation, Vec3::new(1.0, 0.0, 3.0));
        assert!(transform.rotation.abs_diff_eq(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), 1e-5));
    }

}
