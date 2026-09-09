//! A skeleton stood on the map holding one animation state.
//!
//! For looking at animations while building them: place one, pick a state, and
//! it stands there doing that state forever, in the editor and in play. It is
//! a mapper-facing feature rather than a debug overlay because the thing it is
//! useful for — is this animation readable at the size and distance a player
//! will see it — is a question about a room, and can only be answered in one.
//!
//! Deliberately thin. The feature stores a point, a facing and a state; the
//! body itself is an ECS entity like any other, animated by the same systems
//! that animate a player. That is why picking a state here is
//! [`ForcedAnimation`] and not a second animation path: a display that had its
//! own way of posing a skeleton would be a display that agreed with the game
//! right up until it mattered.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use crate::common::skeleton::{
    default_humanoid, AnimationState, ForcedAnimation, Pose, SkeletonAnimator,
};
use crate::common::PointResolutionError;
use crate::editor::action::FeatureData;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
use crate::common::damage::Damageable;
use crate::common::hitbox::Hitboxes;
use crate::get;

/// Marks the entity an [`AnimationDisplay`] drives, so the body can be found
/// without reading the timeline — the same arrangement `SpawnPoint` has with
/// `SpawnPointMarker`.
///
/// Requires [`Hitboxes`] and [`Damageable`]: a display exists to be looked at
/// closely, and where a body can be hit while it holds a state is one of the
/// things worth looking at. Health comes with them because a body you can hit
/// and cannot hurt is a harness that only tests half the answer.
#[derive(Component, Debug)]
#[require(Hitboxes, Damageable)]
pub struct AnimationDisplayMarker;

/// A body on the map, holding one state.
#[derive(Serialize, Deserialize)]
pub struct AnimationDisplay {
    /// The **feet**, as with a spawn point: the end that is lined up against a
    /// floor, and the end a skeleton is placed by.
    location: PointRef,
    /// Which way it faces, in radians about world Y.
    ///
    /// Only yaw, for the same reason a spawn point has only yaw: a body
    /// standing on its side is not a thing to want, and an animation is judged
    /// from the front or the side, which is a heading.
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

        // Degrees in the box, radians in the field, as everywhere else.
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

        // Every state, listed from the state machine itself rather than
        // written out here: a state that existed but could not be selected
        // would be one nobody could look at.
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

    fn snapshot(&self) -> FeatureData {
        FeatureData::AnimationDisplay {
            location: self.location.clone(),
            yaw: self.yaw,
            state: self.state,
        }
    }

    fn apply_snapshot(&mut self, data: &FeatureData) {
        let FeatureData::AnimationDisplay { location, yaw, state } = data else { return; };
        self.location = location.clone();
        self.yaw = *yaw;
        self.state = *state;
    }

    /// The point and the facing only.
    ///
    /// The body itself is drawn by the skeleton systems, off the entity this
    /// feature drives — the same way a prop's box is a mesh rather than a
    /// gizmo. So this draws what the *feature* is, which is a placed point,
    /// and stays visible against the body standing on it.
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

    /// A body, exactly like the game's: a rig, a pose, and the state machine.
    ///
    /// [`ForcedAnimation`] is the only difference between this and a player —
    /// it is pinned to the chosen state instead of being told what is
    /// happening to it.
    fn apply_to_entity(&self, commands: &mut Commands, entity: Entity) {
        commands
            .entity(entity)
            .insert((
                Transform::from_translation(self.resolved_location)
                    .with_rotation(Quat::from_rotation_y(self.yaw)),
                AnimationDisplayMarker,
                // The state is the one thing here an edit is allowed to
                // change, and the reason this runs again at all.
                ForcedAnimation(self.state),
            ))
            // The body once, not once per edit: this runs on every change to
            // the feature, a re-inserted rig reads as a changed rig, and the
            // mesh would be rebuilt on every frame of a drag.
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

    fn point_ref_slots(&self) -> Vec<&str> { vec!["location"] }

    /// Yaw and the state, both as scalars, because scalars are what the save
    /// format carries. The state's number is [`AnimationState::index`], which
    /// is stable across builds by contract.
    fn scalar_fields(&self) -> Vec<(&str, f32)> {
        vec![("yaw", self.yaw), ("state", self.state.index() as f32)]
    }

    fn set_scalar_field(&mut self, key: &str, value: f32) {
        match key {
            "yaw" => self.yaw = value,
            "state" => self.state = AnimationState::from_index(value.max(0.0) as u32),
            _ => {}
        }
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

    /// The display is a body like any other, and the chosen state has to reach
    /// it as [`ForcedAnimation`]. Break the link and the display silently
    /// stands there idle whatever a mapper picked.
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

    /// Yaw and the state go to disk as two scalars and have to come back as
    /// the same two. The state especially: it is an enum squeezed through an
    /// `f32`, which is exactly the sort of thing that rounds to Idle unnoticed.
    #[test]
    fn yaw_and_state_round_trip_through_the_scalar_fields() {
        let display = AnimationDisplay::new(0.0, 0.0, 0.0)
            .with_state(AnimationState::PushingWall)
            .with_yaw(-1.25);

        let mut loaded = AnimationDisplay::new(0.0, 0.0, 0.0);
        for (key, value) in display.scalar_fields() {
            loaded.set_scalar_field(key, value);
        }

        assert_eq!(loaded.state(), AnimationState::PushingWall);
        assert_eq!(loaded.yaw(), -1.25);
    }
}
