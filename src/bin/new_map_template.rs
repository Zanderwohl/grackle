//! Writes `assets/default/blueprints/new.gmb`, the map you get on opening the
//! editor. Generated here rather than saved from the editor so its contents
//! are reviewable as code. Run from the repo root:
//!
//! ```text
//! cargo run --bin new_map_template
//! ```
//!
//! What a new map must provide is asserted by
//! `a_new_map_can_be_stood_in_lit_and_spawned_into` in
//! [`grackle::editor::save`], which deliberately pins no coordinates.

use std::path::PathBuf;

use bevy::prelude::Vec3;
use grackle::constants::MAP_BLUEPRINT_EXTENSION;
use grackle::editor::animation_grid::{self, AnimationGrid};
use grackle::editor::editable::{FeatureId, FeatureTimeline, PointRef};
use grackle::editor::editor_room::EditorRoom;
use grackle::editor::global_point::GlobalPoint;
use grackle::editor::grackle_point_light::GracklePointLight;
use grackle::editor::map_metadata::MapMetadata;
use grackle::editor::save;
use grackle::editor::spawn_point::SpawnPoint;

/// Clear floor between the grid and the walls: enough to back away and see a
/// whole row.
const ROOM_MARGIN_M: f32 = 3.0;

const ROOM_HEIGHT_M: f32 = 6.0;

const SPAWN_DISTANCE_M: f32 = 2.5;

const LIGHT_HEIGHT_M: f32 = 4.5;

fn main() {
    let mut timeline = FeatureTimeline::default();

    // The room is sized to hold the grid, since the roster and its states grow.
    let (grid_min, grid_max) = animation_grid::footprint();
    // The grid's point is the middle of its front row.
    let grid_offset = Vec3::new(0.0, 0.0, -(grid_min.z + grid_max.z) * 0.5);
    let spawn_offset = grid_offset + Vec3::new(0.0, 0.0, grid_min.z - SPAWN_DISTANCE_M);

    // Symmetric about the origin, so the floor centre everything is anchored
    // to is where these offsets were measured from.
    let margin = Vec3::new(ROOM_MARGIN_M, 0.0, ROOM_MARGIN_M);
    let corners = [
        grid_offset + grid_min - margin,
        grid_offset + grid_max + margin,
        spawn_offset - margin,
    ];
    let half = corners
        .iter()
        .fold(Vec3::ZERO, |half, corner| half.max(corner.abs()))
        // Whole metres, so the corners read as typed; rounding up only adds margin.
        .ceil();
    let room_min = Vec3::new(-half.x, 0.0, -half.z);
    let room_max = Vec3::new(half.x, ROOM_HEIGHT_M, half.z);

    // Two points and a room between them, so a corner is a point other
    // features can hang off.
    let min = timeline.apply_feature(Box::new(GlobalPoint::new(room_min.x, room_min.y, room_min.z)));
    let max = timeline.apply_feature(Box::new(GlobalPoint::new(room_max.x, room_max.y, room_max.z)));
    let room = timeline.apply_feature(Box::new(EditorRoom::from_points(min, max)));

    // Anchored to the floor centre so resizing the room takes the contents along.
    let mut light = GracklePointLight::from_point_ref(on_the_floor(room, Vec3::Y * LIGHT_HEIGHT_M));
    light.intensity = 250_000.0;
    light.radius = 3.1;
    // Reaches the far corner of whatever size the room came out.
    light.range = (room_max - room_min).length();
    timeline.apply_feature(Box::new(light));

    // Facing +Z, towards the grid.
    timeline.apply_feature(Box::new(
        SpawnPoint::from_point_ref(on_the_floor(room, spawn_offset))
            .with_yaw(std::f32::consts::PI),
    ));

    // Yaw zero faces -Z, so the roster looks back at the spawn.
    timeline.apply_feature(Box::new(AnimationGrid::from_point_ref(on_the_floor(
        room,
        grid_offset,
    ))));

    let path = PathBuf::from(format!(
        "assets/default/blueprints/new.{MAP_BLUEPRINT_EXTENSION}"
    ));
    match save::save(&path, &timeline, &MapMetadata::default()) {
        Ok(()) => println!("Wrote {}", path.display()),
        Err(error) => {
            eprintln!("Could not write {}: {error}", path.display());
            std::process::exit(1);
        }
    }
}

fn on_the_floor(room: FeatureId, offset: Vec3) -> PointRef {
    let mut point = PointRef::reference_with_offset(room, offset.x, offset.y, offset.z);
    point.point_key = "bottom_plane_center".to_owned();
    point
}
