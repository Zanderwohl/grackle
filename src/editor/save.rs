//! Reading and writing a map blueprint.
//!
//! **A blueprint is flat text: the feature list, written by the features'
//! own serde.** Pretty RON of the map's metadata, the next id to hand out, and
//! every feature in order. It used to be a SQLite database holding the
//! undo history beside the features, with a migration chain and a per-type
//! table registration in seven places; history now lives in memory (as the
//! prop editor's does), and with it went the reason for a database.
//!
//! RON rather than the TOML a prop is written in, for reasons that are about
//! the features rather than about taste: it writes an `f32` as its shortest
//! decimal, so a file says `0.05` and not `0.05000000074505806`; it has
//! `None` and `Some(..)`, so an `Option` field needs nothing special; and it
//! reads as Rust, struct names and all, which is what the features are. It is
//! also what Bevy writes scenes in. The features go through the same serde
//! [`map_sync`](crate::common::map_sync) puts on the wire, so a feature that
//! travels to a client is a feature that saves.
//!
//! **Adding a feature type needs nothing here.** `typetag` names it in the
//! file, so a new `FeatureTrait` implementor saves and loads the day it is
//! written — `every_feature_type_survives_a_save_and_load` is what fails if
//! one does not.
//!
//! There is no version and no migration chain: no blueprint written before
//! this format is read, deliberately.

use std::path::{Path, PathBuf};

use bevy::platform::collections::HashMap;
use bevy::prelude::info;
use serde::{Deserialize, Serialize};

use crate::common::mode::GameMode;
use crate::constants::{MAP_BACKUP_EXTENSION, MAP_BLUEPRINT_EXTENSION};
use crate::editor::editable::{Feature, FeatureId, FeatureTimeline};
use crate::editor::map_metadata::MapMetadata;

/// Result of loading a blueprint file (features + map metadata).
pub struct LoadedBlueprint {
    pub timeline: FeatureTimeline,
    pub metadata: MapMetadata,
}

pub fn map_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.{}", name, MAP_BLUEPRINT_EXTENSION))
}

/// What a map says about itself, as it goes on disk. Its own type rather than
/// `MapMetadata` derived, because that carries the metadata panel's text
/// buffers, which are not facts about the map.
#[derive(Serialize, Deserialize)]
#[serde(rename = "Metadata")]
struct MetadataOnDisk {
    authors: Vec<String>,
    /// [`GameMode::prefix`], the same short name a map file is prefixed with.
    game_mode: String,
}

/// The file's shape. Named `Blueprint` in the file for both directions, since
/// RON writes struct names and the two halves are one format.
#[derive(Serialize)]
#[serde(rename = "Blueprint")]
struct BlueprintOut<'a> {
    metadata: MetadataOnDisk,
    next_id: u64,
    features: Vec<&'a Feature>,
}

#[derive(Deserialize)]
#[serde(rename = "Blueprint")]
struct BlueprintIn {
    metadata: MetadataOnDisk,
    next_id: u64,
    features: Vec<Feature>,
}

/// The blueprint's text, as [`save`] writes it.
pub fn to_text(features: &FeatureTimeline, metadata: &MapMetadata) -> Result<String, String> {
    let blueprint = BlueprintOut {
        metadata: MetadataOnDisk {
            authors: metadata.authors.clone(),
            game_mode: metadata.game_mode.prefix().to_owned(),
        },
        next_id: features.id_counter(),
        features: features
            .feature_order()
            .iter()
            .filter_map(|id| features.get_feature(id))
            .collect(),
    };
    // Indented, one field a line, and with struct names — the point of a text
    // format is a diff that says which number on which feature moved.
    let config = ron::ser::PrettyConfig::new().struct_names(true);
    ron::ser::to_string_pretty(&blueprint, config).map_err(|e| e.to_string())
}

/// Read a blueprint's text back into a map.
pub fn parse(text: &str) -> Result<LoadedBlueprint, String> {
    let blueprint: BlueprintIn = ron::from_str(text).map_err(|e| e.to_string())?;

    let order: Vec<FeatureId> = blueprint.features.iter().map(Feature::id).collect();
    let features: HashMap<FeatureId, Feature> =
        blueprint.features.into_iter().map(|feature| (feature.id(), feature)).collect();
    let mut timeline = FeatureTimeline::from_parts(features, order, blueprint.next_id);

    // In order, so every feature resolves against parents already resolved.
    // A `PointRef`'s resolved position is not on disk — it is a fact about the
    // features it points at, which have only just arrived.
    for id in timeline.feature_order().to_vec() {
        if let Some(mut feature) = timeline.features_mut().remove(&id) {
            feature.object_mut().resolve_references(timeline.features_map());
            timeline.features_mut().insert(id, feature);
        }
    }

    let mut metadata = MapMetadata::default();
    if !blueprint.metadata.authors.is_empty() {
        metadata.authors = blueprint.metadata.authors;
    }
    metadata.game_mode =
        GameMode::try_from(blueprint.metadata.game_mode.as_str()).unwrap_or(GameMode::Arena);
    metadata.normalize_authors_ui_text();

    Ok(LoadedBlueprint { timeline, metadata })
}

/// Write a blueprint, through a backup that is put back if the write fails.
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

    use crate::common::skeleton::AnimationState;
    use crate::common::team::Team;
    use crate::editor::animation_display::AnimationDisplay;
    use crate::editor::animation_grid::{AnimationGrid, RosterTeam};
    use crate::editor::editable::PointRef;
    use crate::editor::editor_room::EditorRoom;
    use crate::editor::global_point::GlobalPoint;
    use crate::editor::grackle_point_light::GracklePointLight;
    use crate::editor::prop::Prop;
    use crate::editor::spawn_point::SpawnPoint;
    use crate::tool::room::Room;

    /// Each feature as a JSON value, in order — what a round trip has to give
    /// back. JSON here only because `serde_json::Value` is a ready-made
    /// comparable tree; the file itself is RON.
    /// Compared as values rather than through each type's accessors, so a
    /// field nobody thought to check still has to survive.
    fn as_values(timeline: &FeatureTimeline) -> Vec<serde_json::Value> {
        timeline
            .feature_order()
            .iter()
            .filter_map(|id| timeline.get_feature(id))
            .map(|feature| serde_json::to_value(feature).unwrap())
            .collect()
    }

    /// One of every feature type, each with the field most likely to come back
    /// as a default set to something that is not one.
    ///
    /// **This is the test a new feature type joins.** There is no registration
    /// to forget any more — `typetag` names the type in the file — but a field
    /// that does not round-trip (a `#[serde(skip)]` on something that should
    /// be saved, say) still loses data quietly, and this is where it shows.
    #[test]
    fn every_feature_type_survives_a_save_and_load() {
        let mut timeline = FeatureTimeline::default();
        let min = timeline.apply_feature(Box::new(GlobalPoint::new(-2.0, 0.0, -2.0)));
        let max = timeline.apply_feature(Box::new(GlobalPoint::new(2.0, 3.0, 2.0)));
        timeline.apply_feature(Box::new(EditorRoom::from_points(min, max)));
        let mut light = GracklePointLight::from_point_ref(PointRef::reference_with_offset(min, 0.5, 1.0, 0.5));
        light.intensity = 1234.5;
        timeline.apply_feature(Box::new(light));
        timeline.apply_feature(Box::new(SpawnPoint::new(3.0, 0.0, -4.0).with_yaw(1.25)));
        timeline.apply_feature(Box::new(Prop::new(2.0, 3.0, -1.0).with_rotation(Vec3::new(0.25, -1.5, 0.75))));
        timeline.apply_feature(Box::new(
            AnimationDisplay::new(4.0, 0.0, -2.0).with_state(AnimationState::PushingWall).with_yaw(1.25),
        ));
        timeline.apply_feature(Box::new(
            AnimationGrid::new(-3.0, 0.5, 7.0).with_yaw(-0.75).with_teams(RosterTeam::One(Team::Green)),
        ));

        let mut metadata = MapMetadata::default();
        metadata.authors = vec!["somebody".into(), "somebody, else".into()];
        metadata.game_mode = GameMode::CTF;

        let text = to_text(&timeline, &metadata).unwrap();
        let loaded = parse(&text).unwrap();

        assert_eq!(as_values(&loaded.timeline), as_values(&timeline), "{text}");
        assert_eq!(loaded.timeline.id_counter(), timeline.id_counter());
        assert_eq!(loaded.metadata.authors, metadata.authors);
        assert_eq!(loaded.metadata.game_mode, GameMode::CTF);

        // And through the disk, since that is the path the editor takes.
        let dir = std::env::temp_dir().join("grackle-save-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("round-trip.{MAP_BLUEPRINT_EXTENSION}"));
        save(&path, &timeline, &metadata).unwrap();
        assert_eq!(as_values(&load(&path).unwrap().timeline), as_values(&timeline));
    }

    /// Floats are written as typed, which is half the reason the format is
    /// text: a diff of a moved point should show the number that moved.
    #[test]
    fn numbers_are_written_the_way_they_were_typed() {
        let mut timeline = FeatureTimeline::default();
        timeline.apply_feature(Box::new(GlobalPoint::new(0.05, 1.1, -0.3)));
        let text = to_text(&timeline, &MapMetadata::default()).unwrap();
        assert!(text.contains("0.05") && !text.contains("0.0500000"), "{text}");
    }

    /// What a new map has to give you, and no more than that.
    ///
    /// Most of what is in the template is taste and will change. Pinning the
    /// spawn point's coordinates would just be a test that has to be edited
    /// every time someone moves it. These are the things whose absence would
    /// actually break something: no room and there is nothing to stand in, no
    /// light and you cannot see it, no spawn with headroom and pressing F5
    /// falls back to guessing.
    #[test]
    fn a_new_map_can_be_stood_in_lit_and_spawned_into() {
        use crate::common::class::{body_centre_from_feet, CLASS_HALF_EXTENTS};
        use crate::game::collision::CollisionWorld;

        let path = PathBuf::from(format!(
            "assets/default/blueprints/new.{}",
            MAP_BLUEPRINT_EXTENSION
        ));
        let loaded = load(&path).expect("the shipped template must load");

        let rooms: Vec<Room> = loaded
            .timeline
            .active_features()
            .filter_map(|(_, f)| f.object().drag_handle_bounds())
            .map(|(min, max)| Room::new(min, max))
            .collect();
        assert!(!rooms.is_empty(), "the template has no room to stand in");

        let lights = loaded
            .timeline
            .active_features()
            .filter(|(_, f)| f.object().type_key() == "grackle_point_light")
            .count();
        assert!(lights > 0, "the template has no light to see by");

        let spawns: Vec<Vec3> = loaded
            .timeline
            .active_features()
            .filter(|(_, f)| f.object().type_key() == "spawn_point")
            .map(|(_, f)| f.object().get_point("").unwrap())
            .collect();
        assert!(!spawns.is_empty(), "the template has no spawn point");

        let mut world = CollisionWorld::default();
        world.rebuild(&rooms);
        assert!(
            spawns
                .iter()
                .any(|feet| world.fits(body_centre_from_feet(*feet), CLASS_HALF_EXTENTS)),
            "no spawn point in the template has room to stand: {spawns:?}"
        );
    }
}
