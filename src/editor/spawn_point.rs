use bevy::prelude::*;
use bevy::platform::collections::HashMap;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::common::class::{TALLEST_CLASS_EYE_HEIGHT, TALLEST_CLASS_HEIGHT};
use crate::common::PointResolutionError;
use crate::editor::action::FeatureData;
use crate::editor::editable::{AxisRef, Feature, FeatureId, FeatureTrait, PointRef};
use crate::get;

/// Marks the entity a [`SpawnPoint`] feature drives, so the game can find
/// somewhere to stand without reading the editor's timeline — the same
/// arrangement `EditorRoom` has with `Room`.
#[derive(Component, Debug)]
pub struct SpawnPointMarker;

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
        }
    }

    fn apply_snapshot(&mut self, data: &FeatureData) {
        let FeatureData::SpawnPoint { location } = data else { return; };
        self.location = location.clone();
    }

    /// Feet, headroom, eyes.
    ///
    /// The line is the space a body needs and the sphere on it is where that
    /// body would be looking from, so a mapper can see at a glance both
    /// whether the spawn fits and what it would open onto. Drawn while the
    /// feature is selected, and whenever spawn-point gizmos are switched on.
    fn debug_gizmos(&self, gizmos: &mut Gizmos) {
        let feet = self.resolved_location;
        let head = feet + Vec3::Y * TALLEST_CLASS_HEIGHT;
        let eyes = feet + Vec3::Y * TALLEST_CLASS_EYE_HEIGHT;

        let yellow = Color::srgb_u8(255, 214, 0);
        gizmos.sphere(Isometry3d::from_translation(feet), 0.2, yellow);
        gizmos.line(feet, head, yellow);
        gizmos.sphere(Isometry3d::from_translation(eyes), 0.12, yellow);

        self.location.debug_gizmos(feet, gizmos);
    }

    fn entity(&self) -> Option<Entity> {
        self.entity
    }

    fn set_entity(&mut self, entity: Option<Entity>) {
        self.entity = entity;
    }

    fn apply_to_entity(&self, commands: &mut Commands, entity: Entity) {
        commands.entity(entity).insert((
            Transform::from_translation(self.resolved_location),
            SpawnPointMarker,
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
            resolved_location: Vec3::new(x, y, z),
            entity: None,
        }
    }

    pub fn from_point_ref(location: PointRef) -> Self {
        Self {
            location,
            resolved_location: Vec3::ZERO,
            entity: None,
        }
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
