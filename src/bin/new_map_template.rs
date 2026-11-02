//! Writes `assets/default/blueprints/new.gmb`, the map you get on opening the
//! editor or from File → New, `demo.gmb`, File → Demo Scene, and `arena.gmb` and
//! `cloister.gmb`, free-for-all maps for bots to fight over. Generated
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

use bevy::prelude::{Vec2, Vec3};
use grackle::constants::MAP_BLUEPRINT_EXTENSION;
use grackle::common::sky::SKYBOX;
use grackle::editor::animation_grid::{self, AnimationGrid};
use grackle::editor::block::EditorBlock;
use grackle::editor::trigger::EditorTrigger;
use grackle::common::trigger::TriggerEffect;
use grackle::material::pit::PIT;
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
const WEAPONS_BEHIND_THE_SPAWN: [(&str, f32); 6] = [
    ("shotgun", -10.0),
    ("rocket_launcher", -6.0),
    ("flamethrower", -2.0),
    ("rpg", 2.0),
    ("pill_launcher", 6.0),
    ("pistol", 10.0),
];

/// A team's spawn nook, butted against a side wall so the shared face opens it into the room.
const NOOK_M: Vec3 = Vec3::new(2.0, 3.0, 2.0);

/// The role a nook's walls name; the palette paints it in each side's color.
const SPAWN_WALLS: &str = "spawn_walls";

fn main() {
    write("new", &new_map(), &MapMetadata::default());
    let (demo, metadata) = demo_scene();
    write("demo", &demo, &metadata);
    let (arena, metadata) = arena();
    write("arena", &arena, &metadata);
    let (cloister, metadata) = cloister();
    write("cloister", &cloister, &metadata);
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

/// Half the arena hall's width and depth.
const HALL_M: f32 = 20.0;
const HALL_HEIGHT_M: f32 = 12.0;
/// Under `STEP_HEIGHT`, so a flight is walked rather than jumped.
const STEP_RISE_M: f32 = 0.4;
/// Wider than a body, or the nav bake finds nowhere on a tread to stand.
const STEP_RUN_M: f32 = 1.0;
/// The middle platform's top: two jumps up, or the stairs.
const PLATFORM_M: f32 = 2.4;
/// A ledge along each side wall, and how far out from the wall it runs.
const LEDGE_M: f32 = 3.2;
const LEDGE_DEPTH_M: f32 = 4.0;
const LEDGE_HALF_LENGTH_M: f32 = 10.0;
/// The corridor behind each end wall, and the doorway left open at each end of it.
const CORRIDOR_DEPTH_M: f32 = 4.0;
const CORRIDOR_HALF_LENGTH_M: f32 = 14.0;
const CORRIDOR_HEIGHT_M: f32 = 4.0;
const DOORWAY_M: f32 = 4.0;
/// Over a ledge's eye, so the pinwheel blocks the ledges' view across the hall.
const PINWHEEL_M: f32 = 6.0;
const PIT_DEPTH_M: f32 = 5.0;
/// How far a spawn alcove or corner nook runs back from the hall.
const ALCOVE_DEPTH_M: f32 = 3.0;

/// A square hall open to the sky, with a stepped platform in the middle ringed by a pinwheel of
/// tall walls, a ledge along each side wall, a corridor behind each end wall, a kill pit on each
/// side, pillars and a wall across each lane of the ring, and spawns tucked out of the hall's sight.
/// Point-symmetric about the origin, so no spawn is better placed than its opposite. Free for all,
/// so nothing has a side.
fn arena() -> (FeatureTimeline, MapMetadata) {
    let mut timeline = FeatureTimeline::default();
    let origin = timeline.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
    timeline.rename(origin, "Map Origin");
    let at = |x: f32, y: f32, z: f32| PointRef::reference_with_offset(origin, x, y, z);

    let hall = RoomSurfaces {
        floor: Some("concrete".to_owned()),
        walls: Some("board_formed_concrete".to_owned()),
        ceiling: Some(SKYBOX.to_owned()),
    };
    let corridor = RoomSurfaces {
        floor: Some("tread_plate".to_owned()),
        walls: Some("plaster".to_owned()),
        ceiling: Some("plaster".to_owned()),
    };
    let volume = |timeline: &mut FeatureTimeline, name: &str, min: Vec3, max: Vec3, surfaces: Option<&RoomSurfaces>| {
        let room = EditorRoom::from_min_and_size(at(min.x, min.y, min.z), max - min);
        let id = match surfaces {
            Some(surfaces) => timeline.apply_feature(Box::new(room.with_surfaces(surfaces.clone()))),
            None => timeline.apply_feature(Box::new(EditorBlock::from_room(room))),
        };
        timeline.rename(id, name);
        id
    };

    volume(&mut timeline, "Hall", Vec3::new(-HALL_M, 0.0, -HALL_M), Vec3::new(HALL_M, HALL_HEIGHT_M, HALL_M), Some(&hall));
    for (end, sign) in [("North", -1.0_f32), ("South", 1.0)] {
        let (near, far) = (sign * HALL_M, sign * (HALL_M + CORRIDOR_DEPTH_M));
        volume(
            &mut timeline,
            &format!("{end} Corridor"),
            Vec3::new(-CORRIDOR_HALF_LENGTH_M, 0.0, near.min(far)),
            Vec3::new(CORRIDOR_HALF_LENGTH_M, CORRIDOR_HEIGHT_M, near.max(far)),
            Some(&corridor),
        );
        // Closes the corridor off from the hall but for a doorway at each end.
        let wall = CORRIDOR_HALF_LENGTH_M - DOORWAY_M;
        volume(
            &mut timeline,
            &format!("{end} Corridor Wall"),
            Vec3::new(-wall, 0.0, near - 0.6),
            Vec3::new(wall, CORRIDOR_HEIGHT_M, near + 0.6),
            None,
        );
    }

    volume(&mut timeline, "Platform", Vec3::new(-4.0, 0.0, -4.0), Vec3::new(4.0, PLATFORM_M, 4.0), None);
    let flight = (PLATFORM_M / STEP_RISE_M).round() as usize;
    for (side, sign) in [("North", -1.0_f32), ("South", 1.0)] {
        for step in 1..flight {
            let run = 4.0 + STEP_RUN_M * (flight - step) as f32;
            let (near, far) = (sign * 4.0, sign * run);
            volume(
                &mut timeline,
                &format!("Platform {side} Step {step}"),
                Vec3::new(-1.5, 0.0, near.min(far)),
                Vec3::new(1.5, STEP_RISE_M * step as f32, near.max(far)),
                None,
            );
        }
    }
    // A box to jump onto on each open side of the platform, and from it onto the platform.
    for (side, sign) in [("East", 1.0_f32), ("West", -1.0)] {
        let (near, far) = (sign * 4.0, sign * 5.5);
        volume(&mut timeline, &format!("Platform {side} Crate"), Vec3::new(near.min(far), 0.0, -1.0), Vec3::new(near.max(far), 1.2, 1.0), None);
    }

    // East's stairs climb at its south end and West's at its north, the half turn of each other.
    for (side, sign) in [("East", 1.0_f32), ("West", -1.0)] {
        let (wall, edge) = (sign * HALL_M, sign * (HALL_M - LEDGE_DEPTH_M));
        let (x_min, x_max) = (wall.min(edge), wall.max(edge));
        volume(&mut timeline, &format!("{side} Ledge"), Vec3::new(x_min, 0.0, -LEDGE_HALF_LENGTH_M), Vec3::new(x_max, LEDGE_M, LEDGE_HALF_LENGTH_M), None);
        let steps = (LEDGE_M / STEP_RISE_M).round() as usize;
        for step in 1..steps {
            let run = LEDGE_HALF_LENGTH_M + STEP_RUN_M * (steps - step) as f32;
            let (near, far) = (sign * LEDGE_HALF_LENGTH_M, sign * run);
            volume(
                &mut timeline,
                &format!("{side} Ledge Step {step}"),
                Vec3::new(x_min, 0.0, near.min(far)),
                Vec3::new(x_max, STEP_RISE_M * step as f32, near.max(far)),
                None,
            );
        }
    }

    for (x, z) in [(10.0, 10.0), (-10.0, 10.0), (10.0, -10.0), (-10.0, -10.0)] {
        volume(&mut timeline, &format!("Pillar {x:+} {z:+}"), Vec3::new(x - 1.25, 0.0, z - 1.25), Vec3::new(x + 1.25, HALL_HEIGHT_M, z + 1.25), None);
    }
    // Across the middle of each side of the ring the pinwheel leaves, so none is one long lane; a
    // gap is left at the outer end to get round.
    for (name, min, max) in [
        ("East Lane Wall", Vec3::new(9.5, 0.0, -0.3), Vec3::new(13.0, PINWHEEL_M, 0.3)),
        ("West Lane Wall", Vec3::new(-13.0, 0.0, -0.3), Vec3::new(-9.5, PINWHEEL_M, 0.3)),
        ("North Lane Wall", Vec3::new(-0.3, 0.0, -13.0), Vec3::new(0.3, PINWHEEL_M, -9.5)),
        ("South Lane Wall", Vec3::new(-0.3, 0.0, 9.5), Vec3::new(0.3, PINWHEEL_M, 13.0)),
    ] {
        volume(&mut timeline, name, min, max, None);
    }

    // A pinwheel round the platform, too tall to see over from a ledge, so the hall is a ring of
    // lanes rather than one sightline; each wall is the last turned a quarter about the middle.
    let mut wall = (Vec2::new(7.0, -9.0), Vec2::new(7.6, -2.0));
    for quarter in 0..4 {
        let (min, max) = (wall.0.min(wall.1), wall.0.max(wall.1));
        volume(&mut timeline, &format!("Pinwheel Wall {}", quarter + 1), Vec3::new(min.x, 0.0, min.y), Vec3::new(max.x, PINWHEEL_M, max.y), None);
        wall = (wall.0.perp(), wall.1.perp());
    }

    // Beside each low wall, where a rocket off the ledge stairs or the platform can put a body.
    let pit_surfaces = RoomSurfaces {
        floor: Some(PIT.to_owned()),
        walls: Some("board_formed_concrete".to_owned()),
        ceiling: None,
    };
    for (side, sign) in [("East", 1.0_f32), ("West", -1.0)] {
        let (a, b) = (sign * Vec2::new(10.0, 3.0), sign * Vec2::new(14.0, 7.0));
        let (min, max) = (a.min(b), a.max(b));
        volume(&mut timeline, &format!("{side} Pit"), Vec3::new(min.x, -PIT_DEPTH_M, min.y), Vec3::new(max.x, 0.0, max.y), Some(&pit_surfaces));
        let floor = EditorRoom::from_min_and_size(at(min.x, -PIT_DEPTH_M, min.y), Vec3::new(max.x - min.x, 1.0, max.y - min.y));
        let kill = timeline.apply_feature(Box::new(EditorTrigger::from_room(floor).with_effect(TriggerEffect::Kill)));
        timeline.rename(kill, &format!("{side} Pit Floor"));
    }

    // Spawns stand out of the hall's sight: an alcove off each ledge, a nook in each corner, and
    // the far end of each corridor. Each faces the middle; `yaw` zero faces -Z.
    let nook = RoomSurfaces {
        floor: Some("tread_plate".to_owned()),
        walls: Some("plaster".to_owned()),
        ceiling: Some("plaster".to_owned()),
    };
    let mut spawns = Vec::new();
    let mut lights = Vec::new();
    for (side, sign) in [("East", 1.0_f32), ("West", -1.0)] {
        let (a, b) = (sign * HALL_M, sign * (HALL_M + ALCOVE_DEPTH_M));
        volume(&mut timeline, &format!("{side} Ledge Alcove"), Vec3::new(a.min(b), LEDGE_M, -2.0), Vec3::new(a.max(b), LEDGE_M + 3.0, 2.0), Some(&nook));
        let feet = Vec3::new(sign * (HALL_M + ALCOVE_DEPTH_M / 2.0), LEDGE_M, 0.0);
        spawns.push(feet);
        lights.push((feet + Vec3::Y * 2.6, 30_000.0, 8.0));
    }
    for (x, z) in [(1.0_f32, 1.0_f32), (-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0)] {
        let (a, b) = (Vec2::new(x * (HALL_M - 4.0), z * HALL_M), Vec2::new(x * HALL_M, z * (HALL_M + ALCOVE_DEPTH_M)));
        let (min, max) = (a.min(b), a.max(b));
        volume(&mut timeline, &format!("Corner Nook {x:+} {z:+}"), Vec3::new(min.x, 0.0, min.y), Vec3::new(max.x, 3.0, max.y), Some(&nook));
        let feet = Vec3::new(x * (HALL_M - 2.0), 0.0, z * (HALL_M + ALCOVE_DEPTH_M / 2.0));
        spawns.push(feet);
        lights.push((feet + Vec3::Y * 2.6, 30_000.0, 8.0));
    }
    for sign in [1.0_f32, -1.0] {
        spawns.push(Vec3::new(sign * 9.0, 0.0, sign * (HALL_M + CORRIDOR_DEPTH_M / 2.0)));
    }
    for (index, feet) in spawns.into_iter().enumerate() {
        let spawn = timeline.apply_feature(Box::new(SpawnPoint::from_point_ref(at(feet.x, feet.y, feet.z)).with_yaw(feet.x.atan2(feet.z))));
        timeline.rename(spawn, &format!("Spawn {}", index + 1));
    }

    let corridor_middle = HALL_M + CORRIDOR_DEPTH_M / 2.0;
    let packs = [
        ("health_pack", Vec3::new(0.0, PLATFORM_M, 0.0)),
        ("health_pack", Vec3::new(-11.0, 0.0, -corridor_middle)),
        ("health_pack", Vec3::new(11.0, 0.0, corridor_middle)),
        ("ammo_pack", Vec3::new(11.0, 0.0, -corridor_middle)),
        ("ammo_pack", Vec3::new(-11.0, 0.0, corridor_middle)),
        ("armor_pack", Vec3::new(HALL_M - LEDGE_DEPTH_M / 2.0, LEDGE_M, -9.0)),
        ("armor_pack", Vec3::new(-(HALL_M - LEDGE_DEPTH_M / 2.0), LEDGE_M, 9.0)),
        ("health_pack", Vec3::new(13.0, 0.0, -1.0)),
        ("health_pack", Vec3::new(-13.0, 0.0, 1.0)),
    ];
    for (model, feet) in packs {
        timeline.apply_feature(Box::new(Prop::from_point_ref(at(feet.x, feet.y, feet.z)).with_model(model)));
    }

    // In pairs a half turn apart, so neither half of the map is better armed. The rocket launchers
    // are down the corridors rather than on the platform, so the best perch and the best splash
    // are two trips.
    let weapons = [
        ("rocket_launcher", Vec3::new(0.0, 0.0, corridor_middle), FRAC_PI_2),
        ("rocket_launcher", Vec3::new(0.0, 0.0, -corridor_middle), FRAC_PI_2),
        ("flamethrower", Vec3::new(-6.0, 0.0, corridor_middle), 0.0),
        ("flamethrower", Vec3::new(6.0, 0.0, -corridor_middle), 0.0),
        ("rpg", Vec3::new(5.5, 0.0, -6.0), 0.0),
        ("rpg", Vec3::new(-5.5, 0.0, 6.0), 0.0),
        ("shotgun", Vec3::new(0.0, 0.0, -14.5), FRAC_PI_2),
        ("shotgun", Vec3::new(0.0, 0.0, 14.5), FRAC_PI_2),
        ("pill_launcher", Vec3::new(HALL_M - LEDGE_DEPTH_M / 2.0, LEDGE_M, 9.0), 0.0),
        ("pill_launcher", Vec3::new(-(HALL_M - LEDGE_DEPTH_M / 2.0), LEDGE_M, -9.0), 0.0),
    ];
    for (weapon, feet, yaw) in weapons {
        timeline.apply_feature(Box::new(WeaponPickup::from_point_ref(at(feet.x, feet.y, feet.z)).with_weapon(weapon).with_yaw(yaw)));
    }

    // The sky lights the hall; these light the platform's shade, the corridors and the spawns.
    lights.extend([
        (Vec3::new(0.0, 8.0, 0.0), 600_000.0, 30.0),
        (Vec3::new(14.0, 2.6, 0.0), 60_000.0, 12.0),
        (Vec3::new(-14.0, 2.6, 0.0), 60_000.0, 12.0),
        (Vec3::new(-7.0, 3.5, -corridor_middle), 50_000.0, 12.0),
        (Vec3::new(7.0, 3.5, -corridor_middle), 50_000.0, 12.0),
        (Vec3::new(-7.0, 3.5, corridor_middle), 50_000.0, 12.0),
        (Vec3::new(7.0, 3.5, corridor_middle), 50_000.0, 12.0),
    ]);
    for (point, intensity, range) in lights {
        let mut light = GracklePointLight::from_point_ref(at(point.x, point.y, point.z));
        light.intensity = intensity;
        light.range = range;
        timeline.apply_feature(Box::new(light));
    }

    let mut metadata = MapMetadata::default();
    metadata.display_name = Some("Foundry".to_owned());
    metadata.authors = vec!["Claude".to_owned()];
    (timeline, metadata)
}

/// Half the courtyard's length (east to west) and width.
const COURT_X_M: f32 = 12.0;
const COURT_Z_M: f32 = 10.0;
/// The end halls run from the courtyard's wall to here.
const HALL_END_M: f32 = 25.0;
const INDOOR_HEIGHT_M: f32 = 3.6;
/// The gallery's floor over each hall, and its ceiling.
const GALLERY_M: f32 = 4.0;
const GALLERY_TOP_M: f32 = 7.6;
/// The tunnel under the courtyard: its floor and its ceiling.
const TUNNEL_M: f32 = -4.4;
const TUNNEL_TOP_M: f32 = -1.6;
const TUNNEL_HALF_WIDTH_M: f32 = 1.25;
const BASIN_DEPTH_M: f32 = 1.2;
/// Over a crouched eye and under a standing one, so a half wall hides a body that ducks.
const HALF_WALL_M: f32 = 1.3;

/// A free-for-all map of several kinds of place, for bots: an open courtyard with a sunken basin
/// and a bridge high across it, a two-story hall at each end with a gallery looking down through
/// windows, a colonnade down each side, a tunnel under the courtyard, and spawn closets out of
/// sight. Every piece is placed with its half turn about the origin, so no spawn is better placed
/// than its opposite.
fn cloister() -> (FeatureTimeline, MapMetadata) {
    let mut timeline = FeatureTimeline::default();
    let origin = timeline.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
    timeline.rename(origin, "Map Origin");
    let at = |x: f32, y: f32, z: f32| PointRef::reference_with_offset(origin, x, y, z);

    let paint = |floor: &str, walls: &str, ceiling: &str| RoomSurfaces {
        floor: Some(floor.to_owned()),
        walls: Some(walls.to_owned()),
        ceiling: Some(ceiling.to_owned()),
    };
    let court = paint("concrete", "board_formed_concrete", SKYBOX);
    let indoors = paint("tread_plate", "plaster", "plaster");
    let gallery = paint("steel_plate", "plaster", "plaster");
    let tunnel = paint("concrete", "board_formed_concrete", "board_formed_concrete");

    // A box, and with `pair` its half turn too, named with a suffix when there are two.
    let place = |timeline: &mut FeatureTimeline, name: &str, min: Vec3, max: Vec3, surfaces: Option<&RoomSurfaces>, pair: bool| {
        let turned = (Vec3::new(-max.x, min.y, -max.z), Vec3::new(-min.x, max.y, -min.z));
        let boxes = if pair { vec![(format!("{name} East"), min, max), (format!("{name} West"), turned.0, turned.1)] } else { vec![(name.to_owned(), min, max)] };
        for (name, min, max) in boxes {
            let room = EditorRoom::from_min_and_size(at(min.x, min.y, min.z), max - min);
            let id = match surfaces {
                Some(surfaces) => timeline.apply_feature(Box::new(room.with_surfaces(surfaces.clone()))),
                None => timeline.apply_feature(Box::new(EditorBlock::from_room(room))),
            };
            timeline.rename(id, &name);
        }
    };
    let v = Vec3::new;

    place(&mut timeline, "Courtyard", v(-COURT_X_M, 0.0, -COURT_Z_M), v(COURT_X_M, 14.0, COURT_Z_M), Some(&court), false);
    place(&mut timeline, "Basin", v(-5.0, -BASIN_DEPTH_M, -4.0), v(5.0, 0.0, 4.0), Some(&court), false);
    // Into the basin from its east and west ends; its long sides are a jump.
    place(&mut timeline, "Basin Upper Step", v(4.0, -BASIN_DEPTH_M, -4.0), v(5.0, -0.4, 4.0), None, true);
    place(&mut timeline, "Basin Lower Step", v(3.0, -BASIN_DEPTH_M, -4.0), v(4.0, -0.8, 4.0), None, true);
    place(&mut timeline, "Colonnade", v(-16.0, 0.0, COURT_Z_M), v(16.0, INDOOR_HEIGHT_M, COURT_Z_M + 4.0), Some(&indoors), true);
    // Walled off from the courtyard but for three arches, so the colonnade is a lane of its own.
    place(&mut timeline, "Colonnade Wall", v(-COURT_X_M, 0.0, COURT_Z_M - 0.5), v(COURT_X_M, INDOOR_HEIGHT_M, COURT_Z_M + 0.5), None, true);
    for x in [-8.0_f32, 0.0, 8.0] {
        place(&mut timeline, &format!("Colonnade Arch {x:+}"), v(x - 1.25, 0.0, COURT_Z_M - 0.5), v(x + 1.25, 2.8, COURT_Z_M + 0.5), Some(&indoors), true);
    }
    // A closet off each end of each colonnade: the north-east and south-west are dead ends and hold
    // spawns; the other two are on the way round the hall and hold a pill launcher.
    place(&mut timeline, "Armory Corner", v(16.0, 0.0, 11.0), v(20.0, 3.0, 14.0), Some(&indoors), true);
    place(&mut timeline, "Spawn Corner", v(16.0, 0.0, -14.0), v(20.0, 3.0, -11.0), Some(&indoors), true);

    place(&mut timeline, "Hall", v(13.0, 0.0, -COURT_Z_M), v(HALL_END_M, INDOOR_HEIGHT_M, COURT_Z_M), Some(&indoors), true);
    place(&mut timeline, "Gallery", v(13.0, GALLERY_M, -COURT_Z_M), v(HALL_END_M, GALLERY_TOP_M, COURT_Z_M), Some(&gallery), true);
    for (name, z) in [("North", -4.5_f32), ("South", 4.5)] {
        place(&mut timeline, &format!("Hall {name} Doorway"), v(COURT_X_M, 0.0, z - 1.5), v(13.0, 3.0, z + 1.5), Some(&indoors), true);
    }
    // Windows with a sill a crouch hides behind, and a door out onto the balcony between them.
    for (name, z) in [("North", -6.0_f32), ("South", 6.0)] {
        place(&mut timeline, &format!("Gallery {name} Window"), v(COURT_X_M, GALLERY_M + HALF_WALL_M, z - 2.0), v(13.0, GALLERY_M + 3.0, z + 2.0), Some(&gallery), true);
    }
    place(&mut timeline, "Balcony Door", v(COURT_X_M, GALLERY_M, -1.5), v(13.0, GALLERY_M + 2.4, 1.5), Some(&gallery), true);
    place(&mut timeline, "Balcony", v(9.5, 0.0, -2.5), v(COURT_X_M, GALLERY_M, 2.5), None, true);
    place(&mut timeline, "Bridge", v(-9.5, GALLERY_M - 0.4, -1.25), v(9.5, GALLERY_M, 1.25), None, false);
    place(&mut timeline, "Gallery Pillar", v(18.0, GALLERY_M, -3.0), v(20.0, GALLERY_TOP_M, 3.0), None, true);
    for (name, z) in [("North", -4.5_f32), ("South", 4.5)] {
        place(&mut timeline, &format!("Hall {name} Pillar"), v(17.5, 0.0, z - 0.5), v(18.5, INDOOR_HEIGHT_M, z + 0.5), None, true);
    }

    // Up to the gallery along the hall's north wall, climbing west.
    let flight = (GALLERY_M / STEP_RISE_M).round() as usize;
    let foot = HALL_END_M - 1.0;
    place(&mut timeline, "Gallery Stairwell", v(foot - (flight - 1) as f32, 0.0, -COURT_Z_M), v(foot, GALLERY_TOP_M, -COURT_Z_M + 2.5), Some(&indoors), true);
    for step in 1..flight {
        let x = foot - step as f32;
        place(&mut timeline, &format!("Gallery Step {step}"), v(x, 0.0, -COURT_Z_M), v(x + STEP_RUN_M, STEP_RISE_M * step as f32, -COURT_Z_M + 2.5), None, true);
    }

    // Down to the tunnel from the middle of the hall, descending west, railed on both sides.
    place(&mut timeline, "Tunnel", v(-13.0, TUNNEL_M, -TUNNEL_HALF_WIDTH_M), v(13.0, TUNNEL_TOP_M, TUNNEL_HALF_WIDTH_M), Some(&tunnel), false);
    let flight = (-TUNNEL_M / STEP_RISE_M).round() as usize;
    let top = 22.0;
    place(&mut timeline, "Tunnel Stairwell", v(13.0, TUNNEL_M, -TUNNEL_HALF_WIDTH_M), v(top, INDOOR_HEIGHT_M, TUNNEL_HALF_WIDTH_M), Some(&tunnel), true);
    for step in 1..flight {
        let x = top - step as f32;
        place(&mut timeline, &format!("Tunnel Step {step}"), v(x, TUNNEL_M, -TUNNEL_HALF_WIDTH_M), v(x + STEP_RUN_M, -STEP_RISE_M * step as f32, TUNNEL_HALF_WIDTH_M), None, true);
    }
    for (name, z) in [("North", -TUNNEL_HALF_WIDTH_M - 0.35), ("South", TUNNEL_HALF_WIDTH_M)] {
        place(&mut timeline, &format!("Tunnel Rail {name}"), v(13.0, 0.0, z), v(top, 1.0, z + 0.35), None, true);
    }
    place(&mut timeline, "Tunnel Alcove", v(4.0, TUNNEL_M, TUNNEL_HALF_WIDTH_M), v(7.0, TUNNEL_TOP_M, TUNNEL_HALF_WIDTH_M + 3.0), Some(&tunnel), true);

    // A way round the hall: its closet runs on into a passage to the colonnade's corner closet.
    place(&mut timeline, "Hall Closet", v(HALL_END_M, 0.0, 5.0), v(HALL_END_M + 3.0, 3.0, 11.0), Some(&indoors), true);
    place(&mut timeline, "Back Passage", v(20.0, 0.0, 11.0), v(HALL_END_M + 3.0, 3.0, 14.0), Some(&indoors), true);
    place(&mut timeline, "Hall Closet Screen", v(HALL_END_M, 0.0, 7.0), v(HALL_END_M + 0.5, 3.0, COURT_Z_M), None, true);
    place(&mut timeline, "Gallery Closet", v(HALL_END_M, GALLERY_M, -2.0), v(HALL_END_M + 3.0, GALLERY_M + 3.0, 2.0), Some(&gallery), true);

    // Cover in the courtyard: half walls to duck behind and crates to stand behind.
    place(&mut timeline, "Court Half Wall", v(5.5, 0.0, 5.5), v(9.0, HALF_WALL_M, 6.0), None, true);
    place(&mut timeline, "Court Low Wall", v(-1.75, 0.0, 6.75), v(1.75, HALF_WALL_M, 7.25), None, true);
    place(&mut timeline, "Court Crates", v(6.5, 0.0, -7.0), v(8.0, 3.0, -5.5), None, true);
    // Before each hall doorway, so stepping out of one is not stepping in front of the whole courtyard.
    for (name, z) in [("North", -4.5_f32), ("South", 4.5)] {
        place(&mut timeline, &format!("Doorway {name} Screen"), v(9.5, 0.0, z - 1.25), v(10.2, 3.0, z + 1.25), None, true);
    }
    // A meter a jump from the courtyard up onto the balcony, beside the bridge.
    for (step, x) in [(1, 5.0_f32), (2, 6.5), (3, 8.0)] {
        place(&mut timeline, &format!("Balcony Crate {step}"), v(x, 0.0, 1.6), v(x + 1.5, step as f32, 3.1), None, true);
    }

    let half_turn = |p: Vec3| Vec3::new(-p.x, p.y, -p.z);
    let spawns = [v(18.5, 0.0, -12.5), v(26.75, 0.0, 10.0), v(26.5, GALLERY_M, 0.0), v(5.5, TUNNEL_M, 3.5)];
    for (index, feet) in spawns.iter().flat_map(|feet| [*feet, half_turn(*feet)]).enumerate() {
        let spawn = timeline.apply_feature(Box::new(SpawnPoint::from_point_ref(at(feet.x, feet.y, feet.z)).with_yaw(feet.x.atan2(feet.z))));
        timeline.rename(spawn, &format!("Spawn {}", index + 1));
    }

    let packs = [
        ("armor_pack", v(0.0, -BASIN_DEPTH_M, 0.0), false),
        ("health_pack", v(0.0, TUNNEL_M, 0.0), false),
        ("health_pack", v(14.5, 0.0, 12.0), true),
        ("health_pack", v(22.0, GALLERY_M, -4.0), true),
        ("ammo_pack", v(17.0, 0.0, 12.5), true),
    ];
    for (model, feet, pair) in packs {
        for feet in if pair { vec![feet, half_turn(feet)] } else { vec![feet] } {
            timeline.apply_feature(Box::new(Prop::from_point_ref(at(feet.x, feet.y, feet.z)).with_model(model)));
        }
    }
    let weapons = [
        ("laser_rifle", v(19.5, GALLERY_M, 6.0), 0.0),
        ("rocket_launcher", v(8.0, TUNNEL_M, 0.0), FRAC_PI_2),
        ("shotgun", v(0.0, 0.0, 12.0), FRAC_PI_2),
        ("flamethrower", v(23.0, 0.0, 12.5), FRAC_PI_2),
        ("rpg", v(10.75, GALLERY_M, 0.0), 0.0),
        ("pill_launcher", v(19.0, 0.0, 12.5), FRAC_PI_2),
    ];
    for (weapon, feet, yaw) in weapons {
        for feet in [feet, half_turn(feet)] {
            timeline.apply_feature(Box::new(WeaponPickup::from_point_ref(at(feet.x, feet.y, feet.z)).with_weapon(weapon).with_yaw(yaw)));
        }
    }

    // The sky lights the courtyard; these light what is roofed.
    let lights = [
        (v(19.0, 3.1, -6.0), 60_000.0, 12.0),
        (v(19.0, 3.1, 6.0), 60_000.0, 12.0),
        (v(16.0, 7.1, 0.0), 60_000.0, 12.0),
        (v(22.0, 7.1, -6.0), 50_000.0, 10.0),
        (v(22.0, 7.1, 6.0), 50_000.0, 10.0),
        (v(8.0, 3.1, 12.0), 40_000.0, 10.0),
        (v(-2.0, 3.1, 12.0), 40_000.0, 10.0),
        (v(18.0, 2.6, 12.5), 25_000.0, 6.0),
        (v(18.0, 2.6, -12.5), 25_000.0, 6.0),
        (v(26.5, 2.6, 8.0), 25_000.0, 6.0),
        (v(23.0, 2.6, 12.5), 25_000.0, 6.0),
        (v(26.5, GALLERY_M + 2.6, 0.0), 25_000.0, 6.0),
        (v(5.0, TUNNEL_TOP_M - 0.3, 0.0), 30_000.0, 9.0),
        (v(11.0, TUNNEL_TOP_M - 0.3, 0.0), 30_000.0, 9.0),
        (v(5.5, TUNNEL_TOP_M - 0.3, 3.5), 15_000.0, 5.0),
    ];
    for (point, intensity, range) in lights {
        for point in [point, half_turn(point)] {
            let mut light = GracklePointLight::from_point_ref(at(point.x, point.y, point.z));
            light.intensity = intensity;
            light.range = range;
            timeline.apply_feature(Box::new(light));
        }
    }

    let mut metadata = MapMetadata::default();
    metadata.display_name = Some("Cloister".to_owned());
    metadata.authors = vec!["Claude".to_owned()];
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
