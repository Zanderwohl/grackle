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
use grackle::editor::animation_grid::{self, AnimationGrid};
use grackle::editor::editable::{FeatureId, FeatureTimeline, PointRef};
use grackle::editor::editor_room::EditorRoom;
use grackle::editor::global_point::GlobalPoint;
use grackle::editor::grackle_point_light::GracklePointLight;
use grackle::editor::map_metadata::MapMetadata;
use grackle::editor::save;
use grackle::editor::spawn_point::SpawnPoint;

/// How much clear floor there is between the grid and the walls.
///
/// Enough to walk round it and to back far enough away to see a row at once.
const ROOM_MARGIN: f32 = 3.0;

/// How high the room is.
///
/// Not derived from anything: it is head-room, and the ceiling is not what
/// anybody is here to look at.
const ROOM_HEIGHT: f32 = 6.0;

/// How far in front of the grid a player starts.
const SPAWN_DISTANCE: f32 = 2.5;

/// How high the light hangs above the floor.
const LIGHT_HEIGHT: f32 = 4.5;

fn main() {
    let mut timeline = FeatureTimeline::default();

    // The room is sized to hold the grid rather than the grid squeezed into a
    // room: the roster and the states it is shown in both grow, and a template
    // whose back rows had quietly ended up outside the wall is exactly what
    // happened when this was two typed-in corners.
    let (grid_min, grid_max) = animation_grid::footprint();
    // The grid's point is the middle of its *front row*, so centring it in the
    // room means offsetting by however much of it is behind that.
    let grid_offset = Vec3::new(0.0, 0.0, -(grid_min.z + grid_max.z) * 0.5);
    let spawn_offset = grid_offset + Vec3::new(0.0, 0.0, grid_min.z - SPAWN_DISTANCE);

    // Symmetric about the origin, so that the middle of the room's floor — the
    // point everything below is anchored to — is where these offsets were
    // measured from. A room sized to hug its contents would put its own centre
    // somewhere else and shift the lot by the difference.
    let margin = Vec3::new(ROOM_MARGIN, 0.0, ROOM_MARGIN);
    let corners = [
        grid_offset + grid_min - margin,
        grid_offset + grid_max + margin,
        spawn_offset - margin,
    ];
    let half = corners
        .iter()
        .fold(Vec3::ZERO, |half, corner| half.max(corner.abs()));
    let room_min = Vec3::new(-half.x, 0.0, -half.z);
    let room_max = Vec3::new(half.x, ROOM_HEIGHT, half.z);

    // The room is two points and a box between them, rather than a box with
    // coordinates in it: dragging a corner is then editing a point that other
    // things can hang off, which is the whole shape of the feature model.
    let min = timeline.apply_feature(Box::new(GlobalPoint::new(room_min.x, room_min.y, room_min.z)));
    let max = timeline.apply_feature(Box::new(GlobalPoint::new(room_max.x, room_max.y, room_max.z)));
    let room = timeline.apply_feature(Box::new(EditorRoom::from_points(min, max)));

    // Everything below is anchored to the middle of the room's floor, so
    // resizing the room takes the contents with it instead of leaving a spawn
    // point outside the wall it used to be next to.
    let mut light = GracklePointLight::from_point_ref(on_the_floor(room, Vec3::Y * LIGHT_HEIGHT));
    light.intensity = 250_000.0;
    light.radius = 3.1;
    // Far enough to reach the far corner of whatever the room came out as.
    light.range = (room_max - room_min).length();
    timeline.apply_feature(Box::new(light));

    // Facing +Z, which is where the grid is: a new map opens looking at the
    // thing it was made to show.
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

/// A point `offset` from the middle of the room's floor.
fn on_the_floor(room: FeatureId, offset: Vec3) -> PointRef {
    let mut point = PointRef::reference_with_offset(room, offset.x, offset.y, offset.z);
    point.point_key = "bottom_plane_center".to_owned();
    point
}
