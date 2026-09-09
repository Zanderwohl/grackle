use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::class::{TALLEST_CLASS_EYE_HEIGHT, TALLEST_CLASS_HEIGHT};
use crate::common::skeleton::{default_humanoid, Pose};
use crate::game::body_mesh::BodyTint;
use crate::common::PointResolutionError;
use crate::editor::action::FeatureData;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
use crate::get;

/// Marks the entity a [`SpawnPoint`] feature drives, so the game can find
/// somewhere to stand without reading the editor's timeline — the same
/// arrangement `EditorRoom` has with `Room`.
#[derive(Component, Debug)]
pub struct SpawnPointMarker;

/// The green a spawn point's body is painted.
///
/// Bright, and nothing else on the map is: this is a drawing of the space a
/// body takes up, and it has to be impossible to mistake for somebody standing
/// there. The same green whether the spawn is selected or not.
pub const SPAWN_BODY_COLOUR: Color = Color::srgb(0.13, 0.95, 0.35);

/// Somewhere a player can be put at the start of a round.
///
/// The point is the **feet**: it is placed against a floor, which is the thing
/// a mapper can actually see and line up. Everything about a body above it —
/// the head clearance it needs, where its camera ends up — comes from the
/// class metrics rather than being stored here, so a spawn point stays valid
/// when those change.
#[derive(Serialize, Deserialize)]
pub struct SpawnPoint {
    location: PointRef,
    /// Which way the body faces, in radians about world Y.
    ///
    /// Only yaw, because a body stands upright: a pitched or rolled spawn
    /// point could only ever describe a spawn nobody can be put in.
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

        // Degrees in the box, radians in the field: nobody lines a spawn up
        // against a wall in radians, and everything downstream is trigonometry.
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

    fn snapshot(&self) -> FeatureData {
        FeatureData::SpawnPoint {
            location: self.location.clone(),
            yaw: self.yaw,
        }
    }

    fn apply_snapshot(&mut self, data: &FeatureData) {
        let FeatureData::SpawnPoint { location, yaw } = data else { return; };
        self.location = location.clone();
        self.yaw = *yaw;
    }

    /// The feet, the eye and the facing — the spawn point rather than the
    /// body, which is a rig on this feature's entity and drawn whatever this
    /// does.
    ///
    /// Drawn while the feature is selected, and whenever spawn-point gizmos
    /// are switched on.
    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let feet = self.resolved_location;
        let eyes = feet + Vec3::Y * TALLEST_CLASS_EYE_HEIGHT;

        let yellow = Color::srgb_u8(255, 214, 0);
        gizmos.sphere(Isometry3d::from_translation(feet), 0.2, yellow);
        gizmos.sphere(Isometry3d::from_translation(eyes), 0.12, yellow);

        // Which way the body looks, drawn from the eyes because that is where
        // the view actually starts.
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

    /// The body that will stand here.
    ///
    /// The same rig the game puts on what it spawns, in its rest pose, so what
    /// stands here is what turns up on F5: the space a body needs is a shape
    /// rather than a height, and an arm through a doorframe is the sort of
    /// thing a mapper can only see if it is drawn.
    ///
    /// A rig with no animator holds the pose it was given, so this stands
    /// still without a system to make it. That and the absent hitboxes are the
    /// difference between this body and an animation display's.
    fn apply_to_entity(&self, commands: &mut Commands, entity: Entity) {
        commands
            .entity(entity)
            .insert((
                Transform::from_translation(self.resolved_location)
                    .with_rotation(Quat::from_rotation_y(self.yaw)),
                SpawnPointMarker,
            ))
            // The body once, not once per edit. This runs on every change to
            // the feature, and a re-inserted rig reads as a changed rig, so
            // `insert` would rebuild the mesh on every frame of a drag.
            .insert_if_new((
                default_humanoid().clone(),
                Pose::rest(),
                BodyTint(SPAWN_BODY_COLOUR),
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

    fn point_ref_slots(&self) -> Vec<&str> { vec!["location"] }

    fn scalar_fields(&self) -> Vec<(&str, f32)> {
        vec![("yaw", self.yaw)]
    }

    fn set_scalar_field(&mut self, key: &str, value: f32) {
        if key == "yaw" {
            self.yaw = value;
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

    /// The heading a body put here faces, in radians about world Y.
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

    /// The game finds spawn points by [`SpawnPointMarker`], which only ever
    /// gets inserted here. Break this link and nothing fails loudly: every map
    /// simply appears to have no spawn points and quietly uses the fallback.
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

    /// The body a mapper lines up against a floor: a real rig, in the green
    /// that marks it a preview, with no hitboxes.
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
            Some(SPAWN_BODY_COLOUR),
            "the preview body is not the preview colour",
        );
        assert!(
            world.get::<Hitboxes>(entity).is_none(),
            "a spawn point's preview is not a thing to shoot at",
        );
    }

    /// Applying the feature again — which happens on every frame of a drag —
    /// must leave the rig alone. A re-inserted `Skeleton` reads as a changed
    /// one, and the mesh would be rebuilt for every frame the point moved.
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

    /// The gizmo's upper sphere is the eye height, and `available_point_keys`
    /// advertises it as a reference — so it has to resolve to that, not to the
    /// feet it would fall back to.
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
