//! Writes `assets/default/blueprints/new.gmb`, the map you get on opening the
//! editor or from File → New, and `demo.gmb`, File → Demo Scene. Generated
//! here rather than saved from the editor so their contents are reviewable as
//! code. Run from the repo root:
//!
//! ```text
//! cargo run --bin new_map_template
//! ```
//!
//! What either must provide is asserted by
//! `a_new_map_can_be_stood_in_lit_and_spawned_into` in
//! [`grackle::editor::save`], which deliberately pins no coordinates.

use std::collections::BTreeMap;
use std::f32::consts::{FRAC_PI_2, PI};
use std::path::PathBuf;

use bevy::prelude::Vec3;
use grackle::constants::MAP_BLUEPRINT_EXTENSION;
use grackle::editor::animation_grid::{self, AnimationGrid};
use grackle::editor::editable::{FeatureId, FeatureTimeline, PointRef};
use grackle::editor::editor_room::EditorRoom;
use grackle::tool::room::RoomSurfaces;
use grackle::editor::global_point::GlobalPoint;
use grackle::editor::grackle_point_light::GracklePointLight;
use grackle::editor::map_metadata::MapMetadata;
use grackle::editor::prop::Prop;
use grackle::editor::save;
use grackle::common::palette::{Palette, RolePaint};
use grackle::common::team::Team;
use grackle::editor::mirror::{Mirror, Symmetry};
use grackle::editor::spawn_point::SpawnPoint;
use grackle::editor::weapon_pickup::WeaponPickup;

/// Clear floor between the grid and the walls: enough to back away and see a
/// whole row.
const ROOM_MARGIN_M: f32 = 3.0;

const ROOM_HEIGHT_M: f32 = 6.0;

const SPAWN_DISTANCE_M: f32 = 2.5;

const LIGHT_HEIGHT_M: f32 = 4.5;

/// How far each pack sits from both walls of its corner.
const PACK_FROM_WALLS_M: f32 = 1.5;

/// Weapon pickups and how far along the wall behind the spawn each lies, in meters.
const WEAPONS_BEHIND_THE_SPAWN: [(&str, f32); 4] =
    [("rocket_launcher", -6.0), ("flamethrower", -2.0), ("rpg", 2.0), ("pill_launcher", 6.0)];

/// A team's spawn nook, butted against a side wall so the shared face opens it into the room.
const NOOK_M: Vec3 = Vec3::new(2.0, 3.0, 2.0);

/// The role a nook's walls name; the palette paints it in each side's color.
const SPAWN_WALLS: &str = "spawn_walls";

fn main() {
    write("new", &new_map(), &MapMetadata::default());
    let (demo, metadata) = demo_scene();
    write("demo", &demo, &metadata);
}

/// A point at the origin, a room off it, a spawn on its floor and a light under its ceiling.
fn new_map() -> FeatureTimeline {
    let mut timeline = FeatureTimeline::default();
    let origin = timeline.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
    let room = timeline.apply_feature(Box::new(EditorRoom::from_point_refs(
        PointRef::reference(origin),
        PointRef::absolute(16.0, 4.0, 16.0),
    )));
    let spawn = timeline.apply_feature(Box::new(SpawnPoint::from_point_ref(on_the_floor(room, Vec3::ZERO))));
    let mut under_the_ceiling = PointRef::reference_with_offset(room, 0.0, -0.5, 0.0);
    under_the_ceiling.point_key = "top_plane_center".to_owned();
    let mut light = GracklePointLight::from_point_ref(under_the_ceiling);
    light.intensity = 1_000_000.0;
    let light = timeline.apply_feature(Box::new(light));
    for (id, name) in [(origin, "Map Origin"), (room, "Starter Room"), (spawn, "Spawn Point"), (light, "Starter Light")] {
        timeline.rename(id, name);
    }
    timeline
}

fn demo_scene() -> (FeatureTimeline, MapMetadata) {
    let mut timeline = FeatureTimeline::default();

    // The room is sized to hold the grid, since the roster and its states grow.
    let (grid_min, grid_max) = animation_grid::footprint();
    // The grid's point is the middle of its front row.
    let grid_offset = Vec3::new(0.0, 0.0, -(grid_min.z + grid_max.z) * 0.5);
    let spawn_offset = grid_offset + Vec3::new(0.0, 0.0, grid_min.z - SPAWN_DISTANCE_M);

    // Symmetric about the origin, so the floor center everything is anchored
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
        // Whole meters, so the corners read as typed; rounding up only adds margin.
        .ceil();
    let room_min = Vec3::new(-half.x, 0.0, -half.z);
    let room_max = Vec3::new(half.x, ROOM_HEIGHT_M, half.z);

    // Two points and a room between them, so a corner is a point other
    // features can hang off.
    let min = timeline.apply_feature(Box::new(GlobalPoint::new(room_min.x, room_min.y, room_min.z)));
    let max = timeline.apply_feature(Box::new(GlobalPoint::new(room_max.x, room_max.y, room_max.z)));
    let surfaces = RoomSurfaces {
        floor: Some("tread_plate".to_owned()),
        walls: Some("board_formed_concrete".to_owned()),
        ceiling: Some("plaster".to_owned()),
    };
    let room = timeline.apply_feature(Box::new(EditorRoom::from_points(min, max).with_surfaces(surfaces)));

    // Anchored to the floor center so resizing the room takes the contents along.
    let mut light = GracklePointLight::from_point_ref(on_the_floor(room, Vec3::Y * LIGHT_HEIGHT_M));
    light.intensity = 250_000.0;
    light.radius = 3.1;
    // Reaches the far corner of whatever size the room came out.
    light.range = (room_max - room_min).length();
    timeline.apply_feature(Box::new(light));

    // Facing +Z, toward the grid.
    timeline.apply_feature(Box::new(
        SpawnPoint::from_point_ref(on_the_floor(room, spawn_offset))
            .with_yaw(PI),
    ));

    // Yaw zero faces -Z, so the roster looks back at the spawn.
    timeline.apply_feature(Box::new(AnimationGrid::from_point_ref(on_the_floor(
        room,
        grid_offset,
    ))));

    // The far corners as the spawn faces them, each anchored to its own floor
    // corner so resizing the room keeps it in the corner. The spawn looks along
    // +Z, so its left is +X. The corner names are `CuboidPoint`'s, which do not
    // read as left and right from here: the values are what matter.
    let packs = [
        // (+X, floor, +Z)
        ("health_pack", "back_bottom_left_corner", Vec3::new(-PACK_FROM_WALLS_M, 0.0, -PACK_FROM_WALLS_M)),
        // (-X, floor, +Z)
        ("ammo_pack", "front_bottom_left_corner", Vec3::new(PACK_FROM_WALLS_M, 0.0, -PACK_FROM_WALLS_M)),
        // (+X, floor, -Z), behind the spawn
        ("armor_pack", "back_bottom_right_corner", Vec3::new(-PACK_FROM_WALLS_M, 0.0, PACK_FROM_WALLS_M)),
    ];
    for (model, corner, offset) in packs {
        let mut at = PointRef::reference_with_offset(room, offset.x, offset.y, offset.z);
        at.point_key = corner.to_owned();
        timeline.apply_feature(Box::new(Prop::from_point_ref(at).with_model(model)));
    }

    // What the arena loadout does not start with, in a row along the wall behind the spawn,
    // anchored to that wall's floor edge. Lying along the wall, so the row reads as a rack.
    for (weapon, x) in WEAPONS_BEHIND_THE_SPAWN {
        let mut at = PointRef::reference_with_offset(room, x, 0.0, PACK_FROM_WALLS_M);
        at.point_key = "bottom_right_edge_center".to_owned();
        timeline.apply_feature(Box::new(WeaponPickup::from_point_ref(at).with_weapon(weapon).with_yaw(FRAC_PI_2)));
    }

    // Red's nook on the +X wall, as the spawn above faces its left, and Blue's made from it by the
    // Mirror. Anchored to the middle of that wall's floor edge, so it follows the wall when the
    // room is resized.
    let mut nook_corner = PointRef::reference_with_offset(room, 0.0, 0.0, -NOOK_M.z / 2.0);
    nook_corner.point_key = "back_bottom_edge_center".to_owned();
    let nook_surfaces = RoomSurfaces {
        floor: Some("tread_plate".to_owned()),
        walls: Some(SPAWN_WALLS.to_owned()),
        ceiling: Some("plaster".to_owned()),
    };
    let nook = timeline.apply_feature(Box::new(
        EditorRoom::from_min_and_size(nook_corner, NOOK_M).with_surfaces(nook_surfaces),
    ));
    // Yaw a quarter turn faces -X: out of the nook and into the room.
    let nook_spawn = timeline.apply_feature(Box::new(
        SpawnPoint::from_point_ref(on_the_floor(nook, Vec3::ZERO)).with_yaw(FRAC_PI_2),
    ));
    // Its own light: the room's is across the map and the side wall shades the nook from it.
    let mut nook_light = GracklePointLight::from_point_ref(on_the_floor(nook, Vec3::Y * (NOOK_M.y - 0.4)));
    nook_light.intensity = 40_000.0;
    nook_light.radius = 0.3;
    nook_light.range = 8.0;
    let nook_light = timeline.apply_feature(Box::new(nook_light));
    timeline.apply_feature(Box::new(
        Mirror::new(on_the_floor(room, Vec3::ZERO))
            .with_symmetry(Symmetry::MirrorX)
            .with_sources(vec![nook, nook_spawn, nook_light])
            .with_teams(Team::Red, Team::Blue),
    ));

    let mut metadata = MapMetadata::default();
    metadata.palette = Palette {
        roles: BTreeMap::from([(
            SPAWN_WALLS.to_owned(),
            RolePaint {
                sides: BTreeMap::from([
                    (Team::Red, "plaster_red".to_owned()),
                    (Team::Blue, "plaster_blue".to_owned()),
                    (Team::Yellow, "plaster_yellow".to_owned()),
                    (Team::Green, "plaster_green".to_owned()),
                ]),
                unsided: Some("plaster".to_owned()),
            },
        )]),
    };

    (timeline, metadata)
}

fn write(name: &str, timeline: &FeatureTimeline, metadata: &MapMetadata) {
    let path = PathBuf::from(format!(
        "assets/default/blueprints/{name}.{MAP_BLUEPRINT_EXTENSION}"
    ));
    match save::save(&path, timeline, metadata) {
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
