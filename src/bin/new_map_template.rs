//! Writes `assets/default/blueprints/new.gmb`, the map you get on opening the
//! editor or from File → New, `demo.gmb`, File → Demo Maps → Demo Scene, and `arena.gmb`,
//! `cloister.gmb` and `waterworks.gmb`, free-for-all maps for bots to fight over, and
//! `showcase.gmb`, every role and material on walls. Generated
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
use grackle::editor::editable::{FeatureId, FeatureTimeline, FeatureTrait, PointRef};
use grackle::editor::editor_room::EditorRoom;
use grackle::editor::footprint_room::FootprintRoom;
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
use grackle::editor::spectator_camera::SpectatorCamera;
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
    let (waterworks, metadata) = waterworks();
    write("waterworks", &waterworks, &metadata);
    let (showcase, metadata) = showcase();
    write("showcase", &showcase, &metadata);
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
    // A high corner, looking down at the middle of the floor; the room is 16 by 4 by 16.
    let mut corner = PointRef::reference_with_offset(room, 7.0, -0.5, 7.0);
    corner.point_key = "top_plane_center".to_owned();
    let overlook = timeline.apply_feature(Box::new(
        SpectatorCamera::from_point_ref(corner).with_rotation(looking(Vec3::new(15.0, 3.5, 15.0), Vec3::new(8.0, 0.0, 8.0))),
    ));
    for (id, name) in [(origin, "Map Origin"), (room, "Starter Room"), (spawn, "Spawn Point"), (light, "Starter Light"), (overlook, "Overlook Camera")] {
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

    // High behind the spawn, looking over the grid.
    let behind = Vec3::new(-(half.x - 0.5), ROOM_HEIGHT_M - 0.5, -(half.z - 0.5));
    let mut up_there = PointRef::reference_with_offset(room, behind.x, -0.5, behind.z);
    up_there.point_key = "top_plane_center".to_owned();
    let overview = timeline.apply_feature(Box::new(
        SpectatorCamera::from_point_ref(up_there).with_rotation(looking(behind, Vec3::ZERO)),
    ));
    timeline.rename(overview, "Overview Camera");

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
    // At the back of the nook, looking out into the room. Local to Red, so the Mirror's copy is
    // Blue's.
    let nook_eye = Vec3::new(0.7, 2.5, 0.0);
    let nook_floor = Vec3::new(half.x + NOOK_M.x / 2.0, 0.0, 0.0);
    let nook_camera = timeline.apply_feature(Box::new(
        SpectatorCamera::from_point_ref(on_the_floor(nook, nook_eye)).with_rotation(looking(nook_floor + nook_eye, Vec3::Y)),
    ));
    timeline.rename(nook_camera, "Nook Camera");
    timeline.apply_feature(Box::new(
        Mirror::new(on_the_floor(room, Vec3::ZERO))
            .with_symmetry(Symmetry::MirrorX)
            .with_sources(vec![nook, nook_spawn, nook_light, nook_camera])
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

    // High over the middle of each wall, looking at the platform: down the axes, which clear the
    // lane walls and miss the pinwheel and the pillars.
    for (name, from) in [
        ("North", Vec3::new(0.0, 10.0, -(HALL_M - 1.0))),
        ("South", Vec3::new(0.0, 10.0, HALL_M - 1.0)),
        ("East", Vec3::new(HALL_M - 1.0, 10.0, 0.0)),
        ("West", Vec3::new(-(HALL_M - 1.0), 10.0, 0.0)),
    ] {
        let camera = timeline.apply_feature(Box::new(
            SpectatorCamera::from_point_ref(at(from.x, from.y, from.z)).with_rotation(looking(from, Vec3::new(0.0, PLATFORM_M, 0.0))),
        ));
        timeline.rename(camera, &format!("{name} Camera"));
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
        ("sniper_rifle", v(19.5, GALLERY_M, 6.0), 0.0),
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

    // A corner of the courtyard looking down into the basin, and a corner of the hall looking
    // across it to the doorways, each with its half turn.
    let cameras = [
        ("Courtyard", v(11.0, 10.0, -9.0), v(0.0, -BASIN_DEPTH_M, 0.0)),
        ("Hall", v(24.5, 3.1, 9.5), v(14.0, 0.5, -5.0)),
    ];
    for (name, from, to) in cameras {
        for (end, from, to) in [("East", from, to), ("West", half_turn(from), half_turn(to))] {
            let camera = timeline.apply_feature(Box::new(SpectatorCamera::from_point_ref(at(from.x, from.y, from.z)).with_rotation(looking(from, to))));
            timeline.rename(camera, &format!("{name} Camera {end}"));
        }
    }

    let mut metadata = MapMetadata::default();
    metadata.display_name = Some("Cloister".to_owned());
    metadata.authors = vec!["Claude".to_owned()];
    (timeline, metadata)
}

/// Places a map's features off one origin, by name.
struct Builder {
    timeline: FeatureTimeline,
    origin: FeatureId,
}

impl Builder {
    fn new() -> Self {
        let mut timeline = FeatureTimeline::default();
        let origin = timeline.apply_feature(Box::new(GlobalPoint::new(0.0, 0.0, 0.0)));
        timeline.rename(origin, "Map Origin");
        Self { timeline, origin }
    }

    fn at(&self, p: Vec3) -> PointRef {
        PointRef::reference_with_offset(self.origin, p.x, p.y, p.z)
    }

    fn room(&mut self, name: &str, min: Vec3, max: Vec3, surfaces: &RoomSurfaces) -> FeatureId {
        let room = EditorRoom::from_min_and_size(self.at(min), max - min).with_surfaces(surfaces.clone());
        let id = self.timeline.apply_feature(Box::new(room));
        self.timeline.rename(id, name);
        id
    }

    fn block(&mut self, name: &str, min: Vec3, max: Vec3) -> FeatureId {
        let room = EditorRoom::from_min_and_size(self.at(min), max - min);
        let id = self.timeline.apply_feature(Box::new(EditorBlock::from_room(room)));
        self.timeline.rename(id, name);
        id
    }

    /// A prism over `corners`, a closed loop in world `x`/`z`, with `inside` in its one region. Its
    /// origin is its first corner, and the corners are measured from it, so it moves as one.
    fn footprint(&mut self, name: &str, floor: f32, height: f32, corners: &[Vec2], inside: Vec2, surfaces: &RoomSurfaces) -> FeatureId {
        let first = corners[0];
        let mut room = FootprintRoom::new(self.at(Vec3::new(first.x, floor, first.y)), height).with_surfaces(surfaces.clone());
        let ids: Vec<_> = corners.iter().map(|c| room.add_local_corner(*c - first)).collect();
        for (i, a) in ids.iter().enumerate() {
            room.add_line(*a, ids[(i + 1) % ids.len()]);
        }
        room.resolve_references(self.timeline.features_map());
        assert!(room.name_region_at(inside), "{name} encloses nothing at {inside}");
        let id = self.timeline.apply_feature(Box::new(room));
        self.timeline.rename(id, name);
        id
    }

    /// A flight of `count` one-meter treads, the first over `first` (min and max in `x`/`z`), each
    /// next one a meter along `along`, the `k`th (from 1) standing from `floor` to `top + rise * k`.
    #[allow(clippy::too_many_arguments)]
    fn flight(&mut self, name: &str, first: (Vec2, Vec2), along: Vec2, count: usize, floor: f32, top: f32, rise: f32) {
        for k in 1..=count {
            let shift = along * (k - 1) as f32;
            let (a, b) = (first.0 + shift, first.1 + shift);
            let height = top + rise * k as f32;
            self.block(&format!("{name} Step {k}"), Vec3::new(a.x, floor, a.y), Vec3::new(b.x, height, b.y));
        }
    }

    fn spawn(&mut self, name: &str, feet: Vec3, facing: Vec3) {
        self.spawn_feature(name, feet, facing);
    }

    fn spawn_feature(&mut self, name: &str, feet: Vec3, facing: Vec3) -> FeatureId {
        let toward = facing - feet;
        let spawn = SpawnPoint::from_point_ref(self.at(feet)).with_yaw((-toward.x).atan2(-toward.z));
        let id = self.timeline.apply_feature(Box::new(spawn));
        self.timeline.rename(id, name);
        id
    }

    fn pack(&mut self, model: &str, feet: Vec3) {
        self.timeline.apply_feature(Box::new(Prop::from_point_ref(self.at(feet)).with_model(model)));
    }

    fn weapon(&mut self, weapon: &str, feet: Vec3, yaw: f32) {
        self.timeline.apply_feature(Box::new(WeaponPickup::from_point_ref(self.at(feet)).with_weapon(weapon).with_yaw(yaw)));
    }

    fn light(&mut self, point: Vec3, intensity: f32, range: f32) {
        self.light_feature(point, intensity, range);
    }

    fn light_feature(&mut self, point: Vec3, intensity: f32, range: f32) -> FeatureId {
        let mut light = GracklePointLight::from_point_ref(self.at(point));
        light.intensity = intensity;
        light.range = range;
        self.timeline.apply_feature(Box::new(light))
    }

    fn camera(&mut self, name: &str, from: Vec3, to: Vec3) {
        let id = self.timeline.apply_feature(Box::new(SpectatorCamera::from_point_ref(self.at(from)).with_rotation(looking(from, to))));
        self.timeline.rename(id, &format!("{name} Camera"));
    }
}

/// The ground; the plaza, garden, yard, market and warehouse stand on it.
const GROUND_M: f32 = 0.0;
/// The lower level: the canal, the cistern and the tunnels between them.
const LOW_M: f32 = -4.5;
/// Treads down from the ground to the lower level.
const DOWN_FLIGHT: usize = 11;
const TUNNEL_HEIGHT_M: f32 = 2.6;
const MEZZANINE_M: f32 = 4.0;
const TOWER_M: f32 = 4.8;

/// A free-for-all map half again as big as the arena or the cloister, of many kinds of place and
/// no symmetry: an open plaza round a water tower, a sunken canal that bends twice, a warehouse
/// with a mezzanine, a hedged garden, a pillared cistern under the plaza, a market street walled
/// into a slalom, a yard with a tower, and narrow tunnels and dog-legs between them. Spawns stand
/// round a corner in nooks off quiet rooms, away from every pickup. Every gap a body walks
/// through is at least 1.5 m: bots wedged in narrower ones and stood there for the rest of a round.
fn waterworks() -> (FeatureTimeline, MapMetadata) {
    let mut b = Builder::new();
    let v = Vec3::new;
    let w = Vec2::new;
    let paint = |floor: &str, walls: &str, ceiling: &str| RoomSurfaces {
        floor: Some(floor.to_owned()),
        walls: Some(walls.to_owned()),
        ceiling: Some(ceiling.to_owned()),
    };
    let outdoors = paint("concrete", "board_formed_concrete", SKYBOX);
    let garden = paint("grid_green", "board_formed_concrete", SKYBOX);
    let indoors = paint("tread_plate", "plaster", "plaster");
    let works = paint("steel_plate", "board_formed_concrete", "board_formed_concrete");
    let below = paint("concrete", "board_formed_concrete", "board_formed_concrete");
    let nook = paint("tread_plate", "plaster_blue", "plaster");

    // The plaza: an irregular octagon open to the sky, a water tower on a dais in the middle, a
    // stairwell down to the cistern cut into its floor.
    let plaza = [w(-12.0, -10.0), w(10.0, -10.0), w(16.0, -4.0), w(16.0, 8.0), w(10.0, 14.0), w(-10.0, 14.0), w(-16.0, 8.0), w(-16.0, -4.0)];
    b.footprint("Plaza", GROUND_M, 14.0, &plaza, w(-8.0, 6.0), &outdoors);
    b.block("Water Tower", v(-2.0, 0.0, -3.5), v(2.0, 10.0, 0.5));
    b.block("Pump House", v(6.5, 0.0, -7.5), v(10.5, 4.0, -4.5));
    for (x, z) in [(-12.0_f32, 1.0_f32), (-12.0, 5.0), (-2.0, 11.0), (-9.0, -6.0)] {
        b.block(&format!("Plaza Pillar {x:+} {z:+}"), v(x - 0.6, 0.0, z - 0.6), v(x + 0.6, 6.0, z + 0.6));
    }
    b.block("Dais", v(-4.0, 0.0, -6.0), v(4.0, 0.8, 2.0));
    b.block("Dais North Step", v(-1.5, 0.0, -7.0), v(1.5, 0.4, -6.0));
    b.block("Dais South Step", v(-1.5, 0.0, 2.0), v(1.5, 0.4, 3.0));
    b.block("Plaza Low Wall East", v(11.0, 0.0, 0.0), v(14.0, 1.3, 0.4));
    b.block("Arcade Screen", v(-13.2, 0.0, -3.0), v(-12.6, 3.0, 1.5));
    b.block("Plaza Crates", v(-7.0, 0.0, 8.0), v(-5.0, 2.4, 10.0));
    b.room("Cistern Stairwell", v(6.0, LOW_M, 2.0), v(8.5, GROUND_M, 13.0), &below);
    b.flight("Cistern Stairwell", (w(6.0, 12.0), w(8.5, 13.0)), w(0.0, -1.0), DOWN_FLIGHT, LOW_M, GROUND_M, -0.4);
    b.block("Stairwell Rail West", v(5.6, 0.0, 1.6), v(6.0, 1.0, 12.0));
    b.block("Stairwell Rail East", v(8.5, 0.0, 1.6), v(8.9, 1.0, 12.0));
    b.block("Stairwell Rail North", v(6.0, 0.0, 1.6), v(8.5, 1.0, 2.0));

    // The cistern under the plaza: low, pillared, quiet.
    b.room("Cistern", v(-6.0, LOW_M, -8.0), v(10.0, LOW_M + 3.5, 10.0), &below);
    for x in [-1.0_f32, 4.0] {
        for z in [-4.0_f32, 2.0, 7.0] {
            b.block(&format!("Cistern Pillar {x:+} {z:+}"), v(x - 0.6, LOW_M, z - 0.6), v(x + 0.6, LOW_M + 3.5, z + 0.6));
        }
    }

    // Up from the cistern into the garden: a tunnel north, then a stairwell cut into the garden's
    // floor, climbing north.
    b.room("Garden Tunnel", v(0.0, LOW_M, -20.0), v(2.5, LOW_M + TUNNEL_HEIGHT_M, -8.0), &below);
    b.room("Garden Stairwell", v(0.0, LOW_M, -31.0), v(2.5, GROUND_M, -20.0), &below);
    b.flight("Garden Stairwell", (w(0.0, -21.0), w(2.5, -20.0)), w(0.0, -1.0), DOWN_FLIGHT, LOW_M, LOW_M, 0.4);
    b.block("Garden Rail West", v(-0.4, 0.0, -30.0), v(0.0, 1.0, -19.6));
    b.block("Garden Rail East", v(2.5, 0.0, -30.0), v(2.9, 1.0, -19.6));
    b.block("Garden Rail South", v(0.0, 0.0, -20.0), v(2.5, 1.0, -19.6));

    // East from the cistern to the canal, with a jog.
    b.room("Canal Tunnel West", v(9.5, LOW_M, 6.0), v(18.0, LOW_M + TUNNEL_HEIGHT_M, 8.5), &below);
    b.room("Canal Tunnel Jog", v(15.5, LOW_M, 2.0), v(18.0, LOW_M + TUNNEL_HEIGHT_M, 8.5), &below);
    b.room("Canal Tunnel Nook Door", v(12.0, LOW_M, 8.5), v(14.5, LOW_M + TUNNEL_HEIGHT_M, 10.5), &nook);
    b.room("Canal Tunnel Nook", v(12.0, LOW_M, 10.5), v(17.0, LOW_M + TUNNEL_HEIGHT_M, 13.5), &nook);
    b.room("Canal Tunnel East", v(15.5, LOW_M, 2.0), v(26.0, LOW_M + TUNNEL_HEIGHT_M, 4.5), &below);

    // The canal: sunken, open to the sky, an S of three reaches.
    let canal = [w(22.0, -38.0), w(29.0, -38.0), w(29.0, -14.0), w(32.0, -14.0), w(32.0, 12.0), w(29.0, 12.0), w(29.0, 34.0), w(22.0, 34.0), w(22.0, 10.0), w(26.0, 10.0), w(26.0, -12.0), w(22.0, -12.0)];
    b.footprint("Canal", LOW_M, 13.5, &canal, w(28.0, 0.0), &outdoors);
    b.block("North Sluice", v(22.0, LOW_M, -32.6), v(26.5, GROUND_M, -32.0));
    b.block("South Sluice", v(25.0, LOW_M, 14.0), v(29.0, GROUND_M, 14.6));
    b.block("Upper Middle Sluice", v(26.0, LOW_M, -6.0), v(30.0, LOW_M + 3.0, -5.4));
    b.block("Middle Sluice", v(28.0, LOW_M, 6.0), v(32.0, LOW_M + 3.0, 6.6));
    b.room("Canal North Nook Door", v(21.5, LOW_M, -35.0), v(22.0, LOW_M + TUNNEL_HEIGHT_M, -33.0), &nook);
    b.room("Canal North Nook", v(17.0, LOW_M, -38.0), v(21.5, LOW_M + TUNNEL_HEIGHT_M, -33.0), &nook);

    // A spur off the canal's south reach to a spawn.
    b.room("Spawn Tunnel", v(29.0, LOW_M, 21.0), v(34.5, LOW_M + TUNNEL_HEIGHT_M, 23.5), &below);
    b.room("Tunnel Nook Door", v(32.0, LOW_M, 23.5), v(34.5, LOW_M + TUNNEL_HEIGHT_M, 25.5), &nook);
    b.room("Tunnel Nook", v(32.0, LOW_M, 25.5), v(37.0, LOW_M + TUNNEL_HEIGHT_M, 28.5), &nook);

    // The garden: open to the sky, hedged into lanes, a gate to the plaza, steps down to the canal.
    b.room("Garden", v(-4.0, 0.0, -38.0), v(16.0, 10.0, -18.0), &garden);
    // A dog-leg, so the plaza and the garden do not see down it into each other.
    b.room("Garden Gate South", v(3.0, 0.0, -14.0), v(7.0, 4.0, -9.5), &indoors);
    b.room("Garden Gate Cross", v(-3.0, 0.0, -16.5), v(7.0, 4.0, -14.0), &indoors);
    b.room("Garden Gate North", v(-3.0, 0.0, -18.5), v(0.0, 4.0, -16.5), &indoors);
    for (name, min, max) in [
        ("Hedge North West", v(-4.0, 0.0, -33.0), v(-1.0, 2.6, -32.2)),
        ("Hedge North", v(3.5, 0.0, -33.0), v(8.0, 2.6, -32.2)),
        ("Hedge Middle", v(5.0, 0.0, -32.0), v(5.8, 2.6, -24.0)),
        ("Hedge East", v(9.0, 0.0, -27.0), v(14.0, 2.6, -26.2)),
        ("Hedge North East", v(10.0, 0.0, -38.0), v(10.8, 2.6, -31.0)),
        ("Hedge Gate Screen", v(3.5, 0.0, -21.2), v(8.5, 2.6, -20.4)),
        ("Hedge South East", v(12.0, 0.0, -23.0), v(12.8, 2.6, -18.0)),
    ] {
        b.block(name, min, max);
    }
    b.room("Canal Steps Door", v(16.0, 0.0, -30.0), v(22.0, 3.0, -27.5), &indoors);
    b.block("Canal Steps Landing", v(22.0, LOW_M, -30.0), v(24.5, 0.0, -27.5));
    b.flight("Canal Steps", (w(22.0, -27.5), w(24.5, -26.5)), w(0.0, 1.0), DOWN_FLIGHT, LOW_M, GROUND_M, -0.4);

    // The warehouse: tall and roofed, a mezzanine round two walls, crates and a shelf on its floor.
    b.room("Warehouse", v(-38.0, 0.0, -32.0), v(-20.0, 9.0, -14.0), &indoors);
    b.block("Mezzanine North", v(-38.0, 0.0, -32.0), v(-20.0, MEZZANINE_M, -28.0));
    b.block("Mezzanine West", v(-38.0, 0.0, -28.0), v(-34.0, MEZZANINE_M, -18.0));
    b.flight("Mezzanine", (w(-25.0, -28.0), w(-24.0, -25.5)), w(-1.0, 0.0), 9, 0.0, 0.0, 0.4);
    // Hides the west mezzanine, and the office door off it, from the floor and the north mezzanine.
    b.block("Mezzanine Landing", v(-34.0, 0.0, -28.0), v(-33.0, MEZZANINE_M, -25.5));
    b.block("Mezzanine Wall", v(-34.4, MEZZANINE_M, -26.0), v(-34.0, MEZZANINE_M + 2.4, -19.5));
    b.block("Mezzanine Partition", v(-38.0, MEZZANINE_M, -26.5), v(-35.5, 9.0, -26.0));
    b.block("Warehouse Shelf", v(-31.0, 0.0, -24.0), v(-30.0, 5.0, -17.0));
    b.block("Warehouse Crates North", v(-26.0, 0.0, -24.0), v(-24.0, 2.0, -22.0));
    b.block("Warehouse Crates South", v(-27.0, 0.0, -19.0), v(-24.5, 2.4, -16.5));
    b.room("Office Door", v(-40.0, MEZZANINE_M, -22.0), v(-38.0, MEZZANINE_M + 3.0, -20.0), &nook);
    b.room("Office", v(-44.0, MEZZANINE_M, -26.0), v(-40.0, MEZZANINE_M + 3.0, -20.0), &nook);
    b.room("Garden Lane West", v(-20.0, 0.0, -28.0), v(-11.0, 3.0, -25.0), &indoors);
    b.room("Lane Nook Door", v(-18.0, 0.0, -30.0), v(-15.5, 3.0, -28.0), &nook);
    b.room("Lane Nook", v(-18.0, 0.0, -33.0), v(-11.0, 3.0, -30.0), &nook);
    b.room("Garden Lane East", v(-14.0, 0.0, -25.0), v(-4.0, 3.0, -22.5), &indoors);

    // From the warehouse round a corner into the plaza.
    b.room("Arcade North", v(-26.0, 0.0, -14.0), v(-23.0, 3.5, 1.0), &indoors);
    b.room("Arcade South", v(-26.0, 0.0, -2.0), v(-15.5, 3.5, 1.0), &indoors);
    b.room("Market Link", v(-21.0, 0.0, 1.0), v(-18.5, 3.5, 10.0), &indoors);

    // From the warehouse's far corner down to the market, a jog with a spawn at its end.
    b.room("Loading Passage North", v(-37.0, 0.0, -14.0), v(-34.5, 3.0, -2.0), &indoors);
    b.room("Loading Nook Door", v(-34.5, 0.0, -10.0), v(-32.5, 3.0, -7.5), &nook);
    b.room("Loading Nook", v(-32.5, 0.0, -12.0), v(-28.5, 3.0, -7.5), &nook);
    b.room("Loading Passage Jog", v(-40.0, 0.0, -4.0), v(-30.0, 3.0, -1.5), &indoors);
    b.room("Loading Passage South", v(-40.0, 0.0, -1.5), v(-37.5, 3.0, 10.0), &indoors);

    // The market: a street open to the sky, walled across twice into a slalom, stalls along it.
    b.room("Market", v(-40.0, 0.0, 10.0), v(-18.0, 10.0, 18.0), &outdoors);
    b.room("Market Gate", v(-19.0, 0.0, 9.0), v(-12.0, 3.5, 13.0), &indoors);
    b.block("Market Wall West", v(-32.0, 0.0, 13.0), v(-31.4, 4.0, 18.0));
    b.block("Market Wall East", v(-23.0, 0.0, 10.0), v(-22.4, 4.0, 14.5));
    b.block("Market Stall West", v(-38.0, 0.0, 15.5), v(-35.5, 2.8, 18.0));
    b.block("Market Stall Middle", v(-29.0, 0.0, 10.0), v(-26.5, 2.8, 12.0));
    b.room("Market Nook Door", v(-40.0, 0.0, 18.0), v(-37.5, 3.0, 20.5), &nook);
    b.room("Market Nook", v(-40.0, 0.0, 20.5), v(-34.0, 3.0, 24.0), &nook);

    // The yard: open to the sky off the plaza's south-east, a tower in it, a door down to the canal.
    b.room("Yard", v(8.0, 0.0, 16.0), v(21.5, 12.0, 30.0), &outdoors);
    b.room("Yard Gate", v(9.0, 0.0, 12.0), v(13.0, 3.5, 16.5), &indoors);
    b.block("Yard Screen", v(8.0, 0.0, 18.5), v(12.3, 6.0, 19.1));
    b.block("Tower", v(15.0, 0.0, 22.0), v(21.5, TOWER_M, 30.0));
    b.block("Tower Parapet", v(15.0, TOWER_M, 22.0), v(21.5, TOWER_M + 1.1, 22.4));
    b.flight("Tower", (w(12.5, 18.0), w(15.0, 19.0)), w(0.0, 1.0), 12, 0.0, 0.0, 0.4);
    b.room("Canal Yard Door", v(21.5, 0.0, 17.0), v(22.0, 3.0, 19.5), &indoors);
    b.block("Canal Yard Landing", v(22.0, LOW_M, 17.0), v(24.5, 0.0, 19.5));
    b.flight("Canal Yard Steps", (w(22.0, 19.5), w(24.5, 20.5)), w(0.0, 1.0), DOWN_FLIGHT, LOW_M, GROUND_M, -0.4);

    // Spawns, each round a corner from its door.
    b.spawn("Spawn Office", v(-42.0, MEZZANINE_M, -24.5), v(-39.0, MEZZANINE_M, -21.0));
    b.spawn("Spawn Loading Nook", v(-29.5, 0.0, -11.0), v(-33.5, 0.0, -8.75));
    b.spawn("Spawn Canal North", v(18.5, LOW_M, -36.5), v(21.75, LOW_M, -34.0));
    b.spawn("Spawn Lane", v(-12.5, 0.0, -31.5), v(-16.75, 0.0, -29.0));
    b.spawn("Spawn Tunnel", v(36.0, LOW_M, 27.0), v(33.25, LOW_M, 24.5));
    b.spawn("Spawn Market", v(-35.5, 0.0, 22.5), v(-38.75, 0.0, 19.25));
    b.spawn("Spawn Canal Tunnel", v(16.0, LOW_M, 12.5), v(13.25, LOW_M, 9.5));
    b.spawn("Spawn Loading", v(-31.5, 0.0, -2.75), v(-35.75, 0.0, -2.75));

    b.pack("armor_pack", v(0.0, 0.8, -4.5));
    for feet in [v(-30.0, MEZZANINE_M, -30.0), v(10.0, 0.0, 28.0), v(-2.5, 0.0, -36.0), v(7.0, LOW_M, -6.0), v(-21.0, 0.0, 16.0)] {
        b.pack("health_pack", feet);
    }
    for (weapon, feet) in [
        ("rocket_launcher", v(-28.0, 0.0, -20.0)),
        ("rocket_launcher", v(26.5, LOW_M, -22.0)),
        ("flamethrower", v(-24.5, 0.0, -0.5)),
        ("flamethrower", v(1.25, LOW_M, -14.0)),
        ("pill_launcher", v(-28.0, 0.0, 16.5)),
        ("pill_launcher", v(2.0, 0.0, -15.25)),
        ("shotgun", v(7.5, 0.0, -29.0)),
        ("shotgun", v(10.5, 0.0, 22.0)),
        ("rpg", v(-4.0, LOW_M, -4.0)),
        ("sniper_rifle", v(19.0, TOWER_M, 27.0)),
    ] {
        b.weapon(weapon, feet, 0.0);
    }

    // The sky lights the plaza, canal, garden, market and yard; these light what is roofed.
    for (point, intensity, range) in [
        (v(-29.0, 7.5, -24.0), 120_000.0, 16.0),
        (v(-24.0, 7.5, -17.0), 80_000.0, 14.0),
        (v(-36.0, 7.0, -22.0), 60_000.0, 12.0),
        (v(-42.0, 6.6, -23.0), 20_000.0, 6.0),
        (v(-24.5, 3.1, -7.0), 40_000.0, 10.0),
        (v(-20.0, 3.1, -0.5), 40_000.0, 10.0),
        (v(-35.75, 2.6, -8.0), 30_000.0, 9.0),
        (v(-35.0, 2.6, -2.75), 30_000.0, 9.0),
        (v(-38.75, 2.6, 4.0), 30_000.0, 9.0),
        (v(-19.75, 3.1, 5.5), 30_000.0, 9.0),
        (v(-37.0, 2.6, 22.25), 20_000.0, 6.0),
        (v(-15.0, 2.6, -25.0), 30_000.0, 9.0),
        (v(10.0, 3.0, 14.25), 30_000.0, 9.0),
        (v(-15.5, 3.0, 11.0), 30_000.0, 9.0),
        (v(2.0, 3.4, -15.25), 30_000.0, 9.0),
        (v(-30.5, 2.6, -9.75), 20_000.0, 6.0),
        (v(31.0, LOW_M + 2.2, 22.25), 20_000.0, 7.0),
        (v(-14.5, 2.6, -31.5), 20_000.0, 6.0),
        (v(-3.0, LOW_M + 3.0, 0.0), 60_000.0, 12.0),
        (v(5.0, LOW_M + 3.0, 5.0), 60_000.0, 12.0),
        (v(-3.0, LOW_M + 3.0, -6.0), 40_000.0, 10.0),
        (v(15.0, LOW_M + 2.2, 12.0), 20_000.0, 6.0),
        (v(1.25, LOW_M + 2.2, -14.0), 30_000.0, 9.0),
        (v(14.0, LOW_M + 2.2, 7.25), 30_000.0, 9.0),
        (v(21.0, LOW_M + 2.2, 3.25), 30_000.0, 9.0),
        (v(19.25, LOW_M + 2.2, -35.5), 20_000.0, 6.0),
        (v(34.5, LOW_M + 2.2, 27.0), 20_000.0, 6.0),
    ] {
        b.light(point, intensity, range);
    }

    b.camera("Plaza", v(-14.0, 9.0, 6.0), v(0.0, 0.8, -2.0));
    b.camera("Canal", v(30.0, 6.0, 10.0), v(28.0, LOW_M, -10.0));
    b.camera("Warehouse", v(-22.0, 7.5, -16.0), v(-32.0, 2.0, -28.0));

    let mut metadata = MapMetadata::default();
    metadata.display_name = Some("Waterworks".to_owned());
    metadata.authors = vec!["Claude".to_owned()];
    (b.timeline, metadata)
}

/// Half the showcase's hub, which is open to the sky.
const HUB_M: f32 = 12.0;
/// A showcase bay: as wide as it is deep, opening its whole side onto the arm's corridor.
const BAY_M: f32 = 6.0;
const BAY_HEIGHT_M: f32 = 4.5;
const SHOWCASE_CORRIDOR_HALF_WIDTH_M: f32 = 3.0;
/// A gallery alcove, its open side on the gallery hall.
const ALCOVE_WIDTH_M: f32 = 5.0;
const ALCOVE_GAP_M: f32 = 1.0;
const ALCOVE_DEPTH_SHOWCASE_M: f32 = 4.0;
/// The gallery hall, north to south.
const GALLERY_NEAR_M: f32 = 16.0;
const GALLERY_FAR_M: f32 = 22.0;

/// Every role and every material, each on walls where it can be seen.
///
/// Four arms off a hub open to the sky, Red's built and a quarter-turn Mirror making Blue's,
/// Yellow's and Green's from it. Each arm is a corridor with one bay per role in the palette,
/// built-in roles included, the role on the bay's walls, so the same bay in each arm shows that
/// role in each team's paint. Off the hub's southeast corner a gallery Mirror leaves alone holds
/// an alcove per material no role names, floored and walled in it.
///
/// Both lists are read when the map is written: the roles from the palette, the materials from the
/// default pack's folder. Run this again after adding either.
fn showcase() -> (FeatureTimeline, MapMetadata) {
    use grackle::material::{FILE_EXTENSION, MATERIALS_DIR};

    let mut b = Builder::new();
    let v = Vec3::new;
    let paint = |floor: &str, walls: &str, ceiling: &str| RoomSurfaces {
        floor: Some(floor.to_owned()),
        walls: Some(walls.to_owned()),
        ceiling: Some(ceiling.to_owned()),
    };

    let palette = Palette {
        roles: BTreeMap::from([(
            "team_plaster".to_owned(),
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
    let roles: Vec<String> = palette.every_role().map(|(name, _)| name.clone()).collect();
    let role_paint: std::collections::BTreeSet<String> =
        palette.every_role().flat_map(|(_, paint)| paint.sides.values().chain(&paint.unsided).cloned()).collect();
    let mut loose: Vec<String> = std::fs::read_dir(PathBuf::from("assets/default").join(MATERIALS_DIR))
        .expect("run from the repo root")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some(FILE_EXTENSION))
        .filter_map(|path| path.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
        .filter(|name| !role_paint.contains(name))
        .collect();
    loose.sort();

    b.room("Hub", v(-HUB_M, 0.0, -HUB_M), v(HUB_M, 8.0, HUB_M), &paint("concrete", "board_formed_concrete", SKYBOX));
    b.light(v(0.0, 6.0, 0.0), 400_000.0, 30.0);
    b.spawn("Hub Spawn", v(0.0, 0.0, 4.0), v(0.0, 0.0, -10.0));
    b.camera("Hub", v(-HUB_M + 1.0, 6.5, HUB_M - 1.0), v(0.0, 0.0, -4.0));

    // Red's arm runs north (-Z). A bay every 8 m down the corridor, alternating east and west.
    let mut sources = Vec::new();
    let pairs = roles.len().div_ceil(2) as f32;
    let corridor_end = -HUB_M - 2.0 - pairs * (BAY_M + 2.0);
    let w = SHOWCASE_CORRIDOR_HALF_WIDTH_M;
    sources.push(b.room("Arm", v(-w, 0.0, corridor_end), v(w, 5.0, -HUB_M), &paint("tread_plate", "concrete", "plaster")));
    for (i, role) in roles.iter().enumerate() {
        let near = -HUB_M - 2.0 - (i / 2) as f32 * (BAY_M + 2.0);
        let (x0, x1) = if i % 2 == 0 { (w, w + BAY_M) } else { (-w - BAY_M, -w) };
        sources.push(b.room(&format!("Bay: {role}"), v(x0, 0.0, near - BAY_M), v(x1, BAY_HEIGHT_M, near), &paint("concrete", role, "plaster")));
        sources.push(b.light_feature(v((x0 + x1) / 2.0, BAY_HEIGHT_M - 0.6, near - BAY_M / 2.0), 60_000.0, 10.0));
    }
    for k in 0..pairs as usize {
        let z = -HUB_M - 2.0 - k as f32 * (BAY_M + 2.0) - BAY_M / 2.0;
        sources.push(b.light_feature(v(0.0, 4.4, z), 30_000.0, 9.0));
    }
    sources.push(b.spawn_feature("Arm Spawn", v(0.0, 0.0, corridor_end + 2.0), v(0.0, 0.0, 0.0)));
    let mirror = b.timeline.apply_feature(Box::new(
        Mirror::new(b.at(Vec3::ZERO))
            .with_symmetry(Symmetry::Rotate90)
            .with_sources(sources)
            .with_four_teams([Team::Red, Team::Blue, Team::Yellow, Team::Green]),
    ));
    b.timeline.rename(mirror, "Four Arms");

    // The gallery: a hall east from the hub's south side, an alcove per material either side.
    // Clear of the south arm's bays, which end 3 m short of its door at x = 9.
    let door = (10.0, 12.0);
    b.room("Gallery Door", v(door.0, 0.0, HUB_M), v(door.1, 3.0, GALLERY_NEAR_M), &paint("concrete", "concrete", "plaster"));
    let first = door.1 + 2.0;
    let per_side = loose.len().div_ceil(2) as f32;
    let hall_end = first + per_side * (ALCOVE_WIDTH_M + ALCOVE_GAP_M) + 1.0;
    b.room("Gallery", v(door.0, 0.0, GALLERY_NEAR_M), v(hall_end, 5.0, GALLERY_FAR_M), &RoomSurfaces::default());
    for (j, material) in loose.iter().enumerate() {
        let x0 = first + (j / 2) as f32 * (ALCOVE_WIDTH_M + ALCOVE_GAP_M);
        let (z0, z1) = if j % 2 == 0 {
            (GALLERY_FAR_M, GALLERY_FAR_M + ALCOVE_DEPTH_SHOWCASE_M)
        } else {
            (GALLERY_NEAR_M - ALCOVE_DEPTH_SHOWCASE_M, GALLERY_NEAR_M)
        };
        b.room(&format!("Alcove: {material}"), v(x0, 0.0, z0), v(x0 + ALCOVE_WIDTH_M, 4.0, z1), &paint(material, material, "plaster"));
        b.light(v(x0 + ALCOVE_WIDTH_M / 2.0, 3.4, (z0 + z1) / 2.0), 40_000.0, 8.0);
    }
    for k in 0..per_side as usize {
        b.light(v(first + k as f32 * (ALCOVE_WIDTH_M + ALCOVE_GAP_M) + ALCOVE_WIDTH_M / 2.0, 4.4, (GALLERY_NEAR_M + GALLERY_FAR_M) / 2.0), 30_000.0, 9.0);
    }
    let middle = (GALLERY_NEAR_M + GALLERY_FAR_M) / 2.0;
    b.camera("Gallery", v(hall_end - 1.0, 4.0, middle), v(door.0, 0.0, middle));

    let mut metadata = MapMetadata::default();
    metadata.display_name = Some("Material Showcase".to_owned());
    metadata.authors = vec!["Claude".to_owned()];
    metadata.palette = palette;
    (b.timeline, metadata)
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

/// Pitch and yaw, in a spectator camera's axis order, from `from` toward `to`. Yaw zero looks
/// down -Z.
fn looking(from: Vec3, to: Vec3) -> Vec3 {
    let toward = (to - from).normalize();
    Vec3::new(toward.y.asin(), (-toward.x).atan2(-toward.z), 0.0)
}

fn on_the_floor(room: FeatureId, offset: Vec3) -> PointRef {
    let mut point = PointRef::reference_with_offset(room, offset.x, offset.y, offset.z);
    point.point_key = "bottom_plane_center".to_owned();
    point
}
