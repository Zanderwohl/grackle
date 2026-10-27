use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::class::{TALLEST_CLASS_EYE_HEIGHT, TALLEST_CLASS_HEIGHT};
use crate::common::skeleton::{default_humanoid, Pose};
use crate::game::body_mesh::BodyTint;
use crate::common::PointResolutionError;
use crate::editor::editable::{Feature, FeatureId, FeatureTrait, PointRef};
use crate::editor::mirror::Mirroring;
use crate::get;

/// Marks the entity a [`SpawnPoint`] feature drives, so the game finds spawns without reading the
/// timeline.
#[derive(Component, Debug)]
pub struct SpawnPointMarker;

/// Nothing else on the map is this green, so the preview cannot be mistaken for a player.
pub const SPAWN_BODY_COLOR: Color = Color::srgb(0.13, 0.95, 0.35);

/// Somewhere a player can be put at the start of a round. The point is the feet; everything above
/// comes from the class metrics, so a spawn stays valid when they change.
#[derive(Serialize, Deserialize)]
pub struct SpawnPoint {
    location: PointRef,
    /// Radians about world Y. Only yaw, because a body stands upright.
    yaw: f32,
    #[serde(skip)]
    resolved_location: Vec3,
    #[serde(skip)]
    entity: Option<Entity>,
}

#[typetag::serde(name = "spawn_point")]
impl FeatureTrait for SpawnPoint {
    fn get_point(&self, key: &str) -> Result<Vec3, PointResolutionError> {
        match key {
            "eyes" => Ok(self.resolved_location + Vec3::Y * TALLEST_CLASS_EYE_HEIGHT),
            _ => Ok(self.resolved_location),
        }
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, features: &HashMap<FeatureId, Feature>, prior_feature_order: &[FeatureId], retarget_request: &mut Option<String>) -> bool {
        let mut changed = false;
        let label = get!("editor.features.spawn_point.feet");
        changed |= self.location.editor_ui(ui, &label, features, prior_feature_order, retarget_request);
        ui.label(get!(
            "editor.features.spawn_point.clearance",
            "height",
            format!("{TALLEST_CLASS_HEIGHT:.1}")
        ));

        // Degrees in the UI, radians stored.
        let mut degrees = self.yaw.to_degrees();
        if ui
            .add(
                egui::DragValue::new(&mut degrees)
                    .speed(1.0)
                    .prefix(format!("{}: ", get!("editor.features.spawn_point.facing")))
                    .suffix("\u{b0}"),
            )
            .changed()
        {
            self.yaw = degrees.to_radians();
            changed = true;
        }

        if changed {
            if let Ok(v) = self.location.resolve(features) {
                self.resolved_location = v;
            }
        }
        changed
    }

    fn type_name(&self) -> String {
        get!("editor.features.spawn_point.title")
    }

    fn type_key(&self) -> &'static str { "spawn_point" }

    /// The feet, the eye and the facing. The body is a rig on the entity and drawn separately.
    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let feet = self.resolved_location;
        let eyes = feet + Vec3::Y * TALLEST_CLASS_EYE_HEIGHT;

        let yellow = Color::srgb_u8(255, 214, 0);
        gizmos.sphere(Isometry3d::from_translation(feet), 0.2, yellow);
        gizmos.sphere(Isometry3d::from_translation(eyes), 0.12, yellow);

        let facing = Quat::from_rotation_y(self.yaw) * Vec3::NEG_Z;
        gizmos.arrow(eyes, eyes + facing, yellow);

        self.location.debug_gizmos(feet, gizmos);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    /// The rest-pose rig the game spawns, so a mapper sees the space a body really takes. No animator,
    /// so it holds the pose; no hitboxes, so it cannot be shot.
    fn apply_to_entity(&mut self, commands: &mut Commands, entity: Entity) {
        commands
            .entity(entity)
            .insert((
                Transform::from_translation(self.resolved_location)
                    .with_rotation(Quat::from_rotation_y(self.yaw)),
                SpawnPointMarker,
            ))
            // `insert_if_new`: this runs on every edit, and a re-inserted rig rebuilds the mesh every frame
            // of a drag.
            .insert_if_new((
                default_humanoid().clone(),
                Pose::rest(),
                BodyTint(SPAWN_BODY_COLOR),
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
            ("".into(), get!("editor.features.spawn_point.feet")),
            ("eyes".into(), get!("editor.features.spawn_point.eyes")),
        ]
    }

    fn reference_points_for_ray(&self, _ray: &Ray3d) -> Vec<(String, Vec3)> {
        vec![
            ("".into(), self.resolved_location),
            ("eyes".into(), self.resolved_location + Vec3::Y * TALLEST_CLASS_EYE_HEIGHT),
        ]
    }

    /// The preview body, which is the hull the spawn is checked for.
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
        self.yaw = mirroring.yaw(self.yaw);
        self.location.resolved_reference = None;
        true
    }
}

impl SpawnPoint {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self {
            location: PointRef::absolute(x, y, z),
            yaw: 0.0,
            resolved_location: Vec3::new(x, y, z),
            entity: None,
        }
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            yaw: 0.0,
            resolved_location: Vec3::ZERO,
            entity: None,
        }
    }

    /// Radians about world Y.
    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn with_yaw(mut self, yaw: f32) -> Self {
        self.yaw = yaw;
        self
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// Without [`SpawnPointMarker`] every map silently has no spawns and uses the fallback.
    #[test]
    fn applying_a_spawn_point_marks_its_entity_for_the_game() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();

        world
            .run_system_once(move |mut commands: Commands| {
                SpawnPoint::new(1.0, 2.0, 3.0).apply_to_entity(&mut commands, entity);
            })
            .unwrap();

        assert!(
            world.get::<SpawnPointMarker>(entity).is_some(),
            "the game would never find this spawn point"
        );
        assert_eq!(
            world.get::<Transform>(entity).unwrap().translation,
            Vec3::new(1.0, 2.0, 3.0),
            "the marked entity is not where the spawn point is"
        );
    }

    #[test]
    fn a_spawn_point_stands_a_green_body_that_cannot_be_shot() {
        use crate::common::hitbox::Hitboxes;
        use crate::common::skeleton::Skeleton;

        let mut world = World::new();
        let entity = world.spawn_empty().id();

        world
            .run_system_once(move |mut commands: Commands| {
                SpawnPoint::new(1.0, 2.0, 3.0).apply_to_entity(&mut commands, entity);
            })
            .unwrap();

        assert!(world.get::<Skeleton>(entity).is_some(), "nothing is standing there");
        assert_eq!(
            world.get::<BodyTint>(entity).map(|tint| tint.0),
            Some(SPAWN_BODY_COLOR),
            "the preview body is not the preview color",
        );
        assert!(
            world.get::<Hitboxes>(entity).is_none(),
            "a spawn point's preview is not a thing to shoot at",
        );
    }

    /// A re-applied feature must not re-insert the `Skeleton`, or the mesh rebuilds every frame of a
    /// drag.
    #[test]
    fn dragging_a_spawn_point_does_not_rebuild_its_body() {
        use crate::common::skeleton::Skeleton;

        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let apply = move |mut commands: Commands| {
            SpawnPoint::new(1.0, 2.0, 3.0).apply_to_entity(&mut commands, entity);
        };

        world.run_system_once(apply).unwrap();
        world.clear_trackers();
        world.run_system_once(apply).unwrap();

        let rig = world.entity(entity).get_ref::<Skeleton>().expect("a body");
        assert!(!rig.is_changed(), "the rig was replaced by an edit that did not touch it");
    }

    /// `available_point_keys` advertises the eyes, so they must resolve there and not fall back to the
    /// feet.
    #[test]
    fn the_eyes_reference_point_is_above_the_feet() {
        let spawn = SpawnPoint::new(0.0, 4.0, 0.0);
        assert_eq!(spawn.get_point("").unwrap(), Vec3::new(0.0, 4.0, 0.0));
        assert_eq!(
            spawn.get_point("eyes").unwrap(),
            Vec3::new(0.0, 4.0 + TALLEST_CLASS_EYE_HEIGHT, 0.0)
        );
    }
}
