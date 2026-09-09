//! Writes `assets/default/blueprints/new.gmb`, the map you get on opening the
//! editor.
//!
//! The template is a SQLite database, so it is a binary that shows up in a
//! diff as "changed" and nothing more. Building it from a program rather than
//! by saving over it from the editor is what makes it reviewable: what a new
//! map contains is this file, and changing it is a code change with a reason
//! attached.
//!
//! Run from the repo root, since the path is relative:
//!
//! ```text
//! cargo run --bin new_map_template
//! ```
//!
//! Most of what is here is taste and will change. The things a new map has to
//! *provide* — somewhere to stand, a light, a spawn point with headroom — are
//! asserted by `a_new_map_can_be_stood_in_lit_and_spawned_into` in
//! [`grackle::editor::save`], which deliberately does not pin any coordinates.

use std::path::PathBuf;

use bevy::prelude::Vec3;
use grackle::constants::MAP_BLUEPRINT_EXTENSION;
use grackle::editor::animation_grid::AnimationGrid;
use grackle::editor::editable::{FeatureId, FeatureTimeline, PointRef};
use grackle::editor::editor_room::EditorRoom;
use grackle::editor::global_point::GlobalPoint;
use grackle::editor::grackle_point_light::GracklePointLight;
use grackle::editor::map_metadata::MapMetadata;
use grackle::editor::save;
use grackle::editor::spawn_point::SpawnPoint;

/// The room's two corners.
///
/// Big enough to hold the animation grid with a couple of metres to spare on
/// every side — eighteen metres of it across and twelve deep — and tall enough
/// that the ceiling is not the first thing you look at. Everything else in the
/// template hangs off the room, so changing these two lines moves the lot.
const ROOM_MIN: Vec3 = Vec3::new(-12.0, 0.0, -12.0);
const ROOM_MAX: Vec3 = Vec3::new(12.0, 6.0, 12.0);

/// Where the grid's front row stands, from the middle of the floor.
///
/// Pushed back so the rows behind it stay inside the room: they recede along
/// +Z from here.
const GRID_OFFSET: Vec3 = Vec3::new(0.0, 0.0, -4.0);

/// Where a player starts, from the middle of the floor.
///
/// In front of the grid, far enough back to see a whole row at once.
const SPAWN_OFFSET: Vec3 = Vec3::new(0.0, 0.0, -9.0);

/// How high the light hangs above the floor.
const LIGHT_HEIGHT: f32 = 4.5;

fn main() {
    let mut timeline = FeatureTimeline::default();

    // The room is two points and a box between them, rather than a box with
    // coordinates in it: dragging a corner is then editing a point that other
    // things can hang off, which is the whole shape of the feature model.
    let min = timeline.apply_feature(Box::new(GlobalPoint::new(ROOM_MIN.x, ROOM_MIN.y, ROOM_MIN.z)));
    let max = timeline.apply_feature(Box::new(GlobalPoint::new(ROOM_MAX.x, ROOM_MAX.y, ROOM_MAX.z)));
    let room = timeline.apply_feature(Box::new(EditorRoom::from_points(min, max)));

    // Everything below is anchored to the middle of the room's floor, so
    // resizing the room takes the contents with it instead of leaving a spawn
    // point outside the wall it used to be next to.
    let mut light = GracklePointLight::from_point_ref(on_the_floor(room, Vec3::Y * LIGHT_HEIGHT));
    light.intensity = 250_000.0;
    light.radius = 3.1;
    light.range = 40.0;
    timeline.apply_feature(Box::new(light));

    // Facing +Z, which is where the grid is: a new map opens looking at the
    // thing it was made to show.
    timeline.apply_feature(Box::new(
        SpawnPoint::from_point_ref(on_the_floor(room, SPAWN_OFFSET))
            .with_yaw(std::f32::consts::PI),
    ));

    // Yaw zero faces -Z, so the roster looks back at the spawn.
    timeline.apply_feature(Box::new(AnimationGrid::from_point_ref(on_the_floor(
        room,
        GRID_OFFSET,
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

/// A point `offset` from the middle of the room's floor.
fn on_the_floor(room: FeatureId, offset: Vec3) -> PointRef {
    let mut point = PointRef::reference_with_offset(room, offset.x, offset.y, offset.z);
    point.point_key = "bottom_plane_center".to_owned();
    point
}
