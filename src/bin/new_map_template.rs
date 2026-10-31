//! Writes `assets/default/blueprints/new.gmb`, the map you get on opening the
//! editor or from File → New, `demo.gmb`, File → Demo Scene, and `arena.gmb`,
//! a free-for-all map for bots to fight over. Generated
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
