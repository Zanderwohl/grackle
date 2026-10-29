//! Reading and writing a map blueprint: pretty RON of the metadata, the next id, and every feature
//! in order, through the features' own serde (the same serde
//! [`map_sync`](crate::common::map_sync) sends).
//!
//! RON because it writes an `f32` as its shortest decimal and has `None`/`Some`. `typetag` names
//! each feature type, so a new one needs nothing here. No version and no migration chain.

use std::path::{Path, PathBuf};

use bevy::platform::collections::HashMap;
use bevy::prelude::info;
use serde::{Deserialize, Serialize};

use crate::common::mode::GameMode;
use crate::common::movement_rules::MovementRules;
use crate::common::sky::{SkyTime, SunPosition};
use crate::common::palette::Palette;
use crate::constants::{MAP_BACKUP_EXTENSION, MAP_BLUEPRINT_EXTENSION};
use crate::editor::editable::{Feature, FeatureId, FeatureTimeline};
use crate::editor::map_metadata::MapMetadata;

pub struct LoadedBlueprint {
    pub timeline: FeatureTimeline,
    pub metadata: MapMetadata,
}

pub fn map_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.{}", name, MAP_BLUEPRINT_EXTENSION))
}

/// The metadata as written to a blueprint and sent to clients. Not `MapMetadata` itself, which
/// carries the panel's text buffers.
///
/// Fields added after the first are `default`, so a map saved before them still opens.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename = "Metadata")]
pub struct SavedMetadata {
    #[serde(default)]
    display_name: Option<String>,
    authors: Vec<String>,
    /// [`GameMode::prefix`], the same short name a map file is prefixed with.
    game_mode: String,
    #[serde(default)]
    movement: MovementRules,
    /// Always written: the settings cross the wire with postcard, which cannot read a field left
    /// out.
    #[serde(default)]
    palette: Palette,
    #[serde(default)]
    sky: SkyTime,
    #[serde(default)]
    sun: SunPosition,
}

impl SavedMetadata {
    pub fn of(metadata: &MapMetadata) -> Self {
        Self {
            display_name: metadata.display_name.clone(),
            authors: metadata.authors.clone(),
            game_mode: metadata.game_mode.prefix().to_owned(),
            movement: metadata.movement,
            palette: metadata.palette.clone(),
            sky: metadata.sky,
            sun: metadata.sun,
        }
    }

    pub fn into_metadata(self) -> MapMetadata {
        let mut metadata = MapMetadata::default();
        metadata.display_name = self.display_name.filter(|name| !name.trim().is_empty());
        if !self.authors.is_empty() {
            metadata.authors = self.authors;
        }
        metadata.game_mode = GameMode::try_from(self.game_mode.as_str()).unwrap_or(GameMode::Arena);
        metadata.movement = self.movement;
        metadata.palette = self.palette;
        metadata.sky = self.sky;
        metadata.sun = self.sun;
        metadata.normalize_authors_ui_text();
        metadata
    }
}

/// Named `Blueprint` in both directions, since RON writes struct names.
#[derive(Serialize)]
#[serde(rename = "Blueprint")]
struct BlueprintOut<'a> {
    metadata: SavedMetadata,
    next_id: u64,
    features: Vec<&'a Feature>,
}

#[derive(Deserialize)]
#[serde(rename = "Blueprint")]
struct BlueprintIn {
    metadata: SavedMetadata,
    next_id: u64,
    features: Vec<Feature>,
}

pub fn to_text(features: &FeatureTimeline, metadata: &MapMetadata) -> Result<String, String> {
    let blueprint = BlueprintOut {
        metadata: SavedMetadata::of(metadata),
        next_id: features.id_counter(),
        features: features
            .feature_order()
            .iter()
            .filter_map(|id| features.get_feature(id))
            .collect(),
    };
    let config = ron::ser::PrettyConfig::new().struct_names(true);
    ron::ser::to_string_pretty(&blueprint, config).map_err(|e| e.to_string())
}

pub fn parse(text: &str) -> Result<LoadedBlueprint, String> {
    let blueprint: BlueprintIn = ron::from_str(text).map_err(|e| e.to_string())?;

    let order: Vec<FeatureId> = blueprint.features.iter().map(Feature::id).collect();
    let features: HashMap<FeatureId, Feature> =
        blueprint.features.into_iter().map(|feature| (feature.id(), feature)).collect();
    let mut timeline = FeatureTimeline::from_parts(features, order, blueprint.next_id);

    // In order, so each feature resolves against parents already resolved. Resolved positions are
    // not on disk.
    for id in timeline.feature_order().to_vec() {
        if let Some(mut feature) = timeline.features_mut().remove(&id) {
            feature.object_mut().resolve_references(timeline.features_map());
            timeline.features_mut().insert(id, feature);
        }
    }

    Ok(LoadedBlueprint { timeline, metadata: blueprint.metadata.into_metadata() })
}

pub fn save(path: &Path, features: &FeatureTimeline, metadata: &MapMetadata) -> Result<(), String> {
    let text = to_text(features, metadata)?;

    let backup_path =
        path.with_extension(format!("{}.{}", MAP_BLUEPRINT_EXTENSION, MAP_BACKUP_EXTENSION));
    let had_existing = path.exists();
    if had_existing {
        std::fs::copy(path, &backup_path).map_err(|e| format!("Failed to create backup: {e}"))?;
    }

    let result = std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()));

    if result.is_err() && had_existing {
        info!("Save failed, restoring from backup");
        let _ = std::fs::copy(&backup_path, path);
    }
    if had_existing && result.is_ok() {
        let _ = std::fs::remove_file(&backup_path);
    }
    result
}

pub fn load(path: &Path) -> Result<LoadedBlueprint, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Vec3;

    use crate::common::movement_rules::SprintPolicy;
    use crate::common::skeleton::AnimationState;
    use crate::common::team::Team;
    use crate::editor::animation_display::AnimationDisplay;
    use crate::editor::animation_grid::{AnimationGrid, RosterTeam};
    use crate::editor::editable::PointRef;
    use crate::editor::editor_room::EditorRoom;
    use crate::editor::editable::FeatureTrait;
    use crate::editor::footprint_room::FootprintRoom;
    use crate::editor::global_point::GlobalPoint;
    use crate::editor::grackle_point_light::GracklePointLight;
    use crate::editor::mirror::{Mirror, Symmetry};
    use crate::editor::prop::Prop;
    use crate::editor::spawn_point::SpawnPoint;
    use crate::editor::weapon_pickup::WeaponPickup;
    use crate::editor::trigger::EditorTrigger;
    use crate::common::trigger::TriggerEffect;
    use crate::tool::room::Room;

    /// JSON only because `serde_json::Value` is a comparable tree. Compared as values so a field
    /// nobody thought to check still has to survive.
    fn as_values(timeline: &FeatureTimeline) -> Vec<serde_json::Value> {
        timeline
            .feature_order()
            .iter()
            .filter_map(|id| timeline.get_feature(id))
            .map(|feature| serde_json::to_value(feature).unwrap())
            .collect()
    }

    /// One of every feature type, each with a non-default field. A new feature type joins here.
    fn one_of_every_feature() -> FeatureTimeline {
        let mut timeline = FeatureTimeline::default();
        let min = timeline.apply_feature(Box::new(GlobalPoint::new(-2.0, 0.0, -2.0)));
        let max = timeline.apply_feature(Box::new(GlobalPoint::new(2.0, 3.0, 2.0)));
        let surfaces = crate::tool::room::RoomSurfaces {
            walls: Some("concrete".into()),
            ..Default::default()
        };
        let room = timeline.apply_feature(Box::new(EditorRoom::from_points(min, max).with_surfaces(surfaces)));
        timeline.rename(room, "Main hall");
        timeline.apply_feature(Box::new(EditorRoom::from_min_and_size(
            PointRef::reference_with_offset(max, 1.0, 0.0, 1.0),
            Vec3::new(4.0, 3.0, 5.0),
        )));
        let mut light = GracklePointLight::from_point_ref(PointRef::reference_with_offset(min, 0.5, 1.0, 0.5));
        light.intensity = 1234.5;
        timeline.apply_feature(Box::new(light));
        timeline.apply_feature(Box::new(SpawnPoint::new(3.0, 0.0, -4.0).with_yaw(1.25)));
        timeline.apply_feature(Box::new(Prop::new(2.0, 3.0, -1.0).with_rotation(Vec3::new(0.25, -1.5, 0.75))));
        timeline.apply_feature(Box::new(WeaponPickup::new(1.0, 0.0, 5.0).with_weapon("rpg").with_yaw(0.5)));
        timeline.apply_feature(Box::new(
            EditorTrigger::from_room(EditorRoom::from_min_and_size(
                PointRef::reference_with_offset(min, 0.0, -1.0, 0.0),
                Vec3::new(2.0, 0.5, 2.0),
            ))
            .with_effect(TriggerEffect::Hurt { per_second: 15 }),
        ));
        timeline.apply_feature(Box::new(
            EditorTrigger::from_room(EditorRoom::from_point_refs(PointRef::absolute(0.0, -9.0, 0.0), PointRef::absolute(9.0, -8.0, 9.0)))
                .with_effect(TriggerEffect::Kill),
        ));
        timeline.apply_feature(Box::new(
            AnimationDisplay::new(4.0, 0.0, -2.0).with_state(AnimationState::PushingWall).with_yaw(1.25),
        ));
        timeline.apply_feature(Box::new(
            AnimationGrid::new(-3.0, 0.5, 7.0).with_yaw(-0.75).with_teams(RosterTeam::One(Team::Green)),
        ));
        timeline.apply_feature(Box::new(
            Mirror::new(PointRef::reference_with_offset(room, 0.0, -1.5, 0.0))
                .with_symmetry(Symmetry::Rotate180)
                .with_sources(vec![room, min])
                .with_teams(Team::Yellow, Team::Green),
        ));
        timeline.apply_feature(Box::new(
            Mirror::new(PointRef::absolute(1.0, 0.0, 1.0))
                .with_symmetry(Symmetry::Rotate90)
                .with_sources(vec![room])
                .with_four_teams([Team::Red, Team::Blue, Team::Blue, Team::Green]),
        ));

        let mut footprint = FootprintRoom::new(PointRef::reference_with_offset(min, 0.0, 0.5, 0.0), 2.75);
        let a = footprint.add_corner(PointRef::reference_with_offset(max, 1.0, 0.0, 0.0));
        let b = footprint.add_corner(PointRef::absolute(6.0, 0.0, 2.0));
        let c = footprint.add_corner(PointRef::absolute(6.0, 0.0, 6.0));
        for (from, to) in [(a, b), (b, c), (c, a)] {
            footprint.add_line(from, to);
        }
        footprint.resolve_references(timeline.features_map());
        assert!(footprint.name_region_at(bevy::math::Vec2::new(5.0, 3.0)), "the triangle encloses nothing there");
        timeline.apply_feature(Box::new(footprint));
        timeline
    }

    /// A `PointRef` a feature does not list is one whose failure nobody sees.
    #[test]
    fn every_point_ref_is_listed() {
        fn count(value: &serde_json::Value) -> usize {
            match value {
                serde_json::Value::Object(map) => {
                    usize::from(map.contains_key("point_key")) + map.values().map(count).sum::<usize>()
                }
                serde_json::Value::Array(items) => items.iter().map(count).sum(),
                _ => 0,
            }
        }
        let timeline = one_of_every_feature();
        for id in timeline.feature_order() {
            let feature = timeline.get_feature(id).unwrap();
            let value = serde_json::to_value(feature.object()).unwrap();
            assert_eq!(feature.object().point_refs().len(), count(&value), "{}: {value}", feature.type_name());
        }
    }

    #[test]
    fn no_shipped_map_has_a_point_that_does_not_resolve() {
        for name in ["new", "demo"] {
            let path = PathBuf::from(format!("assets/default/blueprints/{name}.{MAP_BLUEPRINT_EXTENSION}"));
            let mut timeline = load(&path).unwrap().timeline;
            timeline.resolve_all();
            for (_, feature) in timeline.active_features() {
                assert!(!feature.is_broken(), "{name}: {}", feature.label());
            }
        }
    }

    /// A wrongly skipped field fails here.
    #[test]
    fn every_feature_type_survives_a_save_and_load() {
        let timeline = one_of_every_feature();

        let mut metadata = MapMetadata::default();
        metadata.display_name = Some("2 Fort".into());
        metadata.authors = vec!["somebody".into(), "somebody, else".into()];
        metadata.game_mode = GameMode::CTF;
        metadata.movement = MovementRules { sprint: SprintPolicy::Enabled, gravity: 9.81 };
        metadata.sky = SkyTime::Evening;
        metadata.sun = SunPosition { altitude: 12.5, azimuth: 270.0 };

        let text = to_text(&timeline, &metadata).unwrap();
        let loaded = parse(&text).unwrap();

        assert_eq!(as_values(&loaded.timeline), as_values(&timeline), "{text}");
        assert_eq!(loaded.timeline.id_counter(), timeline.id_counter());
        assert_eq!(SavedMetadata::of(&loaded.metadata), SavedMetadata::of(&metadata), "{text}");

        let dir = std::env::temp_dir().join("grackle-save-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("round-trip.{MAP_BLUEPRINT_EXTENSION}"));
        save(&path, &timeline, &metadata).unwrap();
        assert_eq!(as_values(&load(&path).unwrap().timeline), as_values(&timeline));
    }

    #[test]
    fn a_map_saved_before_the_newer_settings_opens_with_their_defaults() {
        let text = r#"Blueprint(
            metadata: Metadata(authors: ["somebody"], game_mode: "koth"),
            next_id: 0,
            features: [],
        )"#;
        let metadata = parse(text).unwrap().metadata;
        assert_eq!(metadata.display_name, None);
        assert_eq!(metadata.game_mode, GameMode::KOTH);
        assert_eq!(metadata.movement, MovementRules::default());
        assert_eq!(metadata.sky, SkyTime::Day);
        assert_eq!(metadata.sun, SunPosition::default());
    }

    #[test]
    fn numbers_are_written_the_way_they_were_typed() {
        let mut timeline = FeatureTimeline::default();
        timeline.apply_feature(Box::new(GlobalPoint::new(0.05, 1.1, -0.3)));
        let text = to_text(&timeline, &MapMetadata::default()).unwrap();
        assert!(text.contains("0.05") && !text.contains("0.0500000"), "{text}");
    }

    /// Checks only what a new map and the demo scene must provide: a room, a light, and a spawn with
    /// headroom. Do not pin coordinates here.
    #[test]
    fn a_new_map_can_be_stood_in_lit_and_spawned_into() {
        for name in ["new", "demo"] {
            let path = PathBuf::from(format!("assets/default/blueprints/{name}.{MAP_BLUEPRINT_EXTENSION}"));
            can_be_stood_in_lit_and_spawned_into(&path);
        }
    }

    fn can_be_stood_in_lit_and_spawned_into(path: &Path) {
        use crate::common::class::{body_center_from_feet, CLASS_HALF_EXTENTS};
        use crate::game::collision::CollisionWorld;

        let loaded = load(path).unwrap_or_else(|e| panic!("{path:?} must load: {e}"));

        let rooms: Vec<Room> = loaded
            .timeline
            .active_features()
            .filter_map(|(_, f)| f.object().drag_handle_bounds())
            .map(|(min, max)| Room::new(min, max))
            .collect();
        assert!(!rooms.is_empty(), "{path:?} has no room to stand in");

        let lights = loaded
            .timeline
            .active_features()
            .filter(|(_, f)| f.object().type_key() == "grackle_point_light")
            .count();
        assert!(lights > 0, "{path:?} has no light to see by");

        let spawns: Vec<Vec3> = loaded
            .timeline
            .active_features()
            .filter(|(_, f)| f.object().type_key() == "spawn_point")
            .map(|(_, f)| f.object().get_point("").unwrap())
            .collect();
        assert!(!spawns.is_empty(), "{path:?} has no spawn point");

        let mut world = CollisionWorld::default();
        world.rebuild(&rooms);
        assert!(
            spawns
                .iter()
                .any(|feet| world.fits(body_center_from_feet(*feet), CLASS_HALF_EXTENTS)),
            "no spawn point in {path:?} has room to stand: {spawns:?}"
        );
    }
}
