//! The map's surface: the boundary of its air, faces pointing into it. What is
//! drawn, what is collided with and what a pick lands on are all this, so an
//! opening you can see through is one you can walk and shoot through. See
//! `documentation/collision.md` and `documentation/blocks.md`.
//!
//! The map is a sequence of [`Volume`]s, rooms of air and blocks of solid,
//! and a point is air when the last volume containing it is a room. Each
//! volume's share is its own boundary cut where the air does not change across
//! it, so one can be rebuilt without the rest. Where two volumes' faces
//! coincide, the later one owns the face.

use std::cmp::Ordering;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::common::csg::{self, Polygon};
use crate::common::lightmap::Atlas;
use crate::tool::room::{Room, RoomSlot, MATERIAL_TILE_METERS};

/// A piece of the map's surface. Walls are made of nothing yet.
pub type Face = Polygon<()>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    Air,
    Solid,
}

/// A prism of air or of solid, at its place in the map's order.
#[derive(Clone, Debug, PartialEq)]
pub struct Volume {
    pub shape: Room,
    pub fill: Fill,
}

impl Volume {
    pub fn air(shape: Room) -> Self {
        Self { shape, fill: Fill::Air }
    }

    pub fn solid(shape: Room) -> Self {
        Self { shape, fill: Fill::Solid }
    }
}

/// Whether `point` is air: the last volume containing it is a room. Outside
/// every volume is solid.
pub fn is_air(volumes: &[Volume], point: Vec3) -> bool {
    volumes.iter().rev().find(|volume| volume.shape.contains(point)).is_some_and(|volume| volume.fill == Fill::Air)
}

/// The share of the surface that belongs to `volumes[index]`, facing into the
/// air: the parts of its boundary on no later volume, where the last earlier
/// writer outside it has the other fill.
///
/// Cut in coordinates relative to the volume's own corner, so the kernel's
/// tolerance is measured against the size of a neighborhood rather than the
/// distance from the origin.
pub fn share(volumes: &[Volume], index: usize) -> Vec<Face> {
    let volume = &volumes[index];
    let origin = volume.shape.bounds().0;
    let local = |other: &Volume| other.shape.translated(-origin).solid(());
    let mut faces = volume.shape.translated(-origin).solid(());

    // A later volume is the last writer on and inside its own boundary.
    for later in volumes[index + 1..].iter().filter(|other| touching(&volume.shape, &other.shape)) {
        faces = csg::clip_outside(faces, &local(later), true);
        if faces.is_empty() {
            return faces;
        }
    }

    // What lies just outside each piece, from void onwards in order.
    let (mut outside_air, mut outside_solid) = (Vec::new(), faces);
    for earlier in volumes[..index].iter().filter(|other| touching(&volume.shape, &other.shape)) {
        let solid = local(earlier);
        let (air_in, air_out) = csg::partition(std::mem::take(&mut outside_air), &solid);
        let (solid_in, solid_out) = csg::partition(std::mem::take(&mut outside_solid), &solid);
        (outside_air, outside_solid) = (air_out, solid_out);
        match earlier.fill {
            Fill::Air => outside_air.extend(air_in.into_iter().chain(solid_in)),
            Fill::Solid => outside_solid.extend(air_in.into_iter().chain(solid_in)),
        }
    }

    let (kept, faces_out_of_it) = match volume.fill {
        Fill::Air => (outside_solid, false),
        Fill::Solid => (outside_air, true),
    };
    kept.into_iter()
        .map(|mut face| {
            if !faces_out_of_it {
                face.flip();
            }
            face.translated(origin)
        })
        .collect()
}

/// Every volume's share, in order.
pub fn of_volumes(volumes: &[Volume]) -> Vec<Face> {
    (0..volumes.len()).flat_map(|index| share(volumes, index)).collect()
}

/// Rooms alone, in an order of their boxes so that which room keeps a face two
/// share does not depend on the order they are handed in: the lower box, then
/// the lower index, is later and so keeps it.
fn in_box_order(rooms: &[Room]) -> (Vec<Volume>, Vec<usize>) {
    let key = |room: &Room| {
        let (min, max) = room.bounds();
        [min.x, min.y, min.z, max.x, max.y, max.z]
    };
    let mut order: Vec<usize> = (0..rooms.len()).collect();
    order.sort_by(|&a, &b| {
        let by_box = key(&rooms[a])
            .iter()
            .zip(key(&rooms[b]))
            .map(|(x, y)| x.total_cmp(&y))
            .find(|order| *order != Ordering::Equal)
            .unwrap_or(Ordering::Equal);
        by_box.then(a.cmp(&b)).reverse()
    });
    let volumes = order.iter().map(|&index| Volume::air(rooms[index].clone())).collect();
    (volumes, order)
}

/// The share of `rooms[index]` in a map of rooms alone.
pub fn room_share(rooms: &[Room], index: usize) -> Vec<Face> {
    let (volumes, order) = in_box_order(rooms);
    let at = order.iter().position(|&i| i == index).expect("every room is in the order");
    share(&volumes, at)
}

/// Every room's share, in a map of rooms alone.
pub fn of_rooms(rooms: &[Room]) -> Vec<Face> {
    of_volumes(&in_box_order(rooms).0)
}

/// Overlapping or meeting, faces and edges included: a room meeting this one
/// along a wall opens it.
fn touching(a: &Room, b: &Room) -> bool {
    let ((a_min, a_max), (b_min, b_max)) = (a.bounds(), b.bounds());
    (0..3).all(|axis| a_min[axis] <= b_max[axis] && b_min[axis] <= a_max[axis])
}

/// Flat-shaded triangles, normals into the room, with mikktspace tangents for a
/// material's normal map. UVs are world-space in material tiles
/// ([`MATERIAL_TILE_METERS`]) across the face's two widest axes, so a material
/// runs on across neighboring walls instead of stretching with the room, and
/// `v` runs down a wall, as image rows do. `UV_1` is `lightmap_uv` of a face's index in `faces` and
/// a vertex.
pub fn mesh(faces: &[Face], lightmap_uv: impl Fn(usize, Vec3) -> Vec2) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut lightmap_uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for (index, face) in faces.iter().enumerate() {
        let normal = face.plane.normal;
        let flat = normal.abs();
        let uv = |p: Vec3| -> [f32; 2] {
            let (u, v) = if flat.x >= flat.y && flat.x >= flat.z {
                (p.z, -p.y)
            } else if flat.y >= flat.z {
                (p.x, p.z)
            } else {
                (p.x, -p.y)
            };
            [u / MATERIAL_TILE_METERS, v / MATERIAL_TILE_METERS]
        };
        let base = positions.len() as u32;
        for vertex in &face.vertices {
            positions.push(vertex.to_array());
            normals.push(normal.to_array());
            uvs.push(uv(*vertex));
            lightmap_uvs.push(lightmap_uv(index, *vertex).to_array());
        }
        for i in 1..face.vertices.len() as u32 - 1 {
            indices.extend_from_slice(&[base, base + i, base + i + 1]);
        }
    }

    let empty = indices.is_empty();
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, lightmap_uvs)
        .with_inserted_indices(Indices::U32(indices));
    if !empty {
        // Planar convex faces with distinct UVs always have tangents.
        mesh.generate_tangents().expect("a room's faces have tangents");
    }
    mesh
}

/// [`mesh`] once per [`RoomSlot`], each slot sorted by its faces' inward
/// normals, so each can take its own material. A slot with no faces left —
/// every one opened up by a neighbor — is left out. `UV_1` is into one
/// [`Atlas`] of all of `faces`, which every slot shares.
pub fn slot_meshes(faces: &[Face]) -> Vec<(RoomSlot, Mesh)> {
    let atlas = Atlas::of(faces);
    RoomSlot::ALL
        .into_iter()
        .filter_map(|slot| {
            let mine: Vec<usize> =
                (0..faces.len()).filter(|&index| RoomSlot::of(faces[index].plane.normal) == slot).collect();
            let shown: Vec<Face> = mine.iter().map(|&index| faces[index].clone()).collect();
            (!mine.is_empty()).then(|| (slot, mesh(&shown, |index, point| atlas.uv(mine[index], point))))
        })
        .collect()
}

/// Where `ray` first meets a face from the side it faces — from inside, for a
/// room's share — and how far along. Edges count as face, so a pick on the
/// seam between two pieces of one wall does not fall through.
pub fn ray_from_inside<'a>(faces: impl IntoIterator<Item = &'a Face>, ray: &Ray3d) -> Option<(f32, Vec3)> {
    let direction = Vec3::from(ray.direction);
    let mut best: Option<(f32, Vec3)> = None;
    for face in faces {
        if let Some(t) = meets(face, ray.origin, direction)
            && best.is_none_or(|(best_t, _)| t < best_t)
        {
            best = Some((t, ray.origin + direction * t));
        }
    }
    best
}

/// How far along `direction` from `origin` the ray meets `face` from the side it faces. Edges
/// count as face.
pub fn meets(face: &Face, origin: Vec3, direction: Vec3) -> Option<f32> {
    let normal = face.plane.normal;
    let closing = normal.dot(direction);
    if closing >= 0.0 {
        return None;
    }
    let t = (face.plane.w - normal.dot(origin)) / closing;
    (t >= 0.0 && contains(face, origin + direction * t)).then_some(t)
}

/// For a point already in the face's plane. Convex and wound
/// counter-clockwise about the normal, so inside is left of every edge.
fn contains(face: &Face, point: Vec3) -> bool {
    const ON_THE_EDGE: f32 = 1.0e-5;
    let count = face.vertices.len();
    (0..count).all(|i| {
        let (a, b) = (face.vertices[i], face.vertices[(i + 1) % count]);
        (b - a).cross(point - a).dot(face.plane.normal) >= -ON_THE_EDGE * (b - a).length()
    })
}

/// The area of the faces facing along `normal`, for tests that measure what a
/// cut left rather than counting the pieces it left it in.
#[cfg(test)]
fn area_facing(faces: &[Face], normal: Vec3) -> f32 {
    faces
        .iter()
        .filter(|face| face.plane.normal.abs_diff_eq(normal, 1e-5))
        .map(|face| {
            let a = face.vertices[0];
            face.vertices[1..].windows(2).map(|w| (w[0] - a).cross(w[1] - a).length() / 2.0).sum::<f32>()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_count(mesh: &Mesh) -> usize {
        match mesh.indices() {
            Some(Indices::U32(v)) => v.len() / 3,
            _ => 0,
        }
    }

    #[test]
    fn a_rooms_slots_are_sorted_by_which_way_they_face() {
        let rooms = [Room::new(Vec3::ZERO, Vec3::new(10.0, 3.0, 10.0))];
        let slots = slot_meshes(&room_share(&rooms, 0));
        let tris: Vec<(RoomSlot, usize)> = slots.iter().map(|(slot, mesh)| (*slot, triangle_count(mesh))).collect();
        assert_eq!(tris, vec![(RoomSlot::Floor, 2), (RoomSlot::Walls, 8), (RoomSlot::Ceiling, 2)]);
    }

    /// A slot with nothing left in it is not baked: an empty mesh is an entity drawing nothing.
    #[test]
    fn a_slot_opened_up_entirely_is_left_out() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0)),
            Room::new(Vec3::new(0.0, 3.0, 0.0), Vec3::new(4.0, 6.0, 4.0)),
        ];
        let slots: Vec<RoomSlot> = slot_meshes(&room_share(&rooms, 0)).into_iter().map(|(slot, _)| slot).collect();
        assert_eq!(slots, vec![RoomSlot::Floor, RoomSlot::Walls]);
    }

    /// Breaks if texture coordinates go back to per-face fractions: a material then stretches
    /// with the room and jumps where two rooms' walls meet.
    #[test]
    fn texture_coordinates_are_world_space_so_walls_line_up() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 6.0)),
            Room::new(Vec3::new(4.0, 0.0, 0.0), Vec3::new(9.0, 3.0, 6.0)),
        ];
        let point = Vec3::new(4.0, 3.0, 6.0);
        // On the back walls, which face -Z and so map x across and y down.
        let uv_at = |index: usize| {
            let (_, walls) = slot_meshes(&room_share(&rooms, index))
                .into_iter()
                .find(|(slot, _)| *slot == RoomSlot::Walls)
                .unwrap();
            let positions = walls.attribute(Mesh::ATTRIBUTE_POSITION).unwrap().as_float3().unwrap().to_vec();
            let normals = walls.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap().as_float3().unwrap().to_vec();
            let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) = walls.attribute(Mesh::ATTRIBUTE_UV_0)
            else {
                panic!("no texture coordinates")
            };
            let at = (0..positions.len())
                .position(|i| Vec3::from(positions[i]).abs_diff_eq(point, 1e-4) && Vec3::from(normals[i]) == Vec3::NEG_Z)
                .expect("the shared corner is a vertex of both rooms' back walls");
            uvs[at]
        };
        assert_eq!(uv_at(0), uv_at(1));
        assert_eq!(uv_at(0), [point.x / MATERIAL_TILE_METERS, -point.y / MATERIAL_TILE_METERS]);
    }

    fn total_area(faces: &[Face]) -> f32 {
        [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z]
            .into_iter()
            .map(|n| area_facing(faces, n))
            .sum()
    }

    fn ray(origin: Vec3, direction: Vec3) -> Ray3d {
        Ray3d::new(origin, Dir3::new(direction).unwrap())
    }

    #[test]
    fn a_lone_room_is_six_walls_facing_in() {
        let rooms = [Room::new(Vec3::ZERO, Vec3::new(10.0, 3.0, 10.0))];
        let share = room_share(&rooms, 0);
        assert!((total_area(&share) - (2.0 * 100.0 + 4.0 * 30.0)).abs() < 1e-3);
        assert!((area_facing(&share, Vec3::Y) - 100.0).abs() < 1e-3, "the floor does not face up into the room");
    }

    #[test]
    fn a_shared_full_wall_is_open_from_both_sides() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::new(10.0, 3.0, 10.0)),
            Room::new(Vec3::new(10.0, 0.0, 0.0), Vec3::new(20.0, 3.0, 10.0)),
        ];
        for index in 0..2 {
            let share = room_share(&rooms, index);
            assert!(area_facing(&share, Vec3::X) + area_facing(&share, Vec3::NEG_X) < 30.0 + 1e-3, "room {index} kept the shared wall");
            assert!((total_area(&share) - (200.0 + 90.0)).abs() < 1e-3, "room {index} lost more than the shared wall");
        }
    }

    #[test]
    fn a_partial_wall_leaves_a_frame_round_the_opening() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::splat(10.0)),
            Room::new(Vec3::new(10.0, 2.0, 2.0), Vec3::new(20.0, 8.0, 8.0)),
        ];
        let share = room_share(&rooms, 0);
        assert!((area_facing(&share, Vec3::NEG_X) - (100.0 - 36.0)).abs() < 1e-3);
        assert!((area_facing(&share, Vec3::X) - 100.0).abs() < 1e-3, "the far wall was touched");
    }

    #[test]
    fn a_room_crossing_a_wall_cuts_a_hole_and_loses_its_inside_part() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::splat(10.0)),
            Room::new(Vec3::new(5.0, 3.0, 3.0), Vec3::new(15.0, 7.0, 7.0)),
        ];
        let a = room_share(&rooms, 0);
        assert!((area_facing(&a, Vec3::NEG_X) - (100.0 - 16.0)).abs() < 1e-3);
        let b = room_share(&rooms, 1);
        assert!(area_facing(&b, Vec3::X) < 1e-3, "the end of B inside A is still there");
        // Only the half of B outside A keeps its sides: four 5 × 4 strips.
        assert!((area_facing(&b, Vec3::Y) + area_facing(&b, Vec3::NEG_Y) - 40.0).abs() < 1e-3);
    }

    /// A change from the rectangle bake, which kept all six faces of an
    /// engulfed room: a union has no inner surface.
    #[test]
    fn a_room_wholly_inside_another_adds_nothing() {
        let rooms = [Room::new(Vec3::ZERO, Vec3::splat(10.0)), Room::new(Vec3::splat(3.0), Vec3::splat(7.0))];
        assert!(room_share(&rooms, 1).is_empty());
        assert!((total_area(&room_share(&rooms, 0)) - 600.0).abs() < 1e-3);
    }

    /// Two overlapping rooms on one floor: the floor is kept once, by one of
    /// them, and not by neither.
    #[test]
    fn a_floor_two_rooms_share_is_kept_exactly_once() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::new(10.0, 3.0, 10.0)),
            Room::new(Vec3::new(5.0, 0.0, 5.0), Vec3::new(15.0, 3.0, 15.0)),
        ];
        let floor = area_facing(&of_rooms(&rooms), Vec3::Y);
        assert!((floor - (100.0 + 100.0 - 25.0)).abs() < 1e-3, "floor area {floor}");
    }

    #[test]
    fn two_identical_rooms_are_one_room() {
        let room = Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0));
        let surface = of_rooms(&[room.clone(), room]);
        assert!((total_area(&surface) - (32.0 + 48.0)).abs() < 1e-3);
    }

    fn hall() -> Room {
        Room::new(Vec3::ZERO, Vec3::new(10.0, 3.0, 10.0))
    }

    /// The air the surface encloses. Faces point into it, so the signed volume
    /// is negative; a surface with a gap in it measures nothing sensible.
    fn air_volume(faces: &[Face]) -> f32 {
        -csg::signed_volume_x6(faces) / 6.0
    }

    #[test]
    fn a_block_on_the_floor_has_no_floor_under_it_and_no_bottom() {
        let volumes = [Volume::air(hall()), Volume::solid(Room::new(Vec3::new(4.0, 0.0, 4.0), Vec3::new(6.0, 1.0, 6.0)))];
        let room = share(&volumes, 0);
        assert!((area_facing(&room, Vec3::Y) - 96.0).abs() < 1e-3, "floor {}", area_facing(&room, Vec3::Y));
        let block = share(&volumes, 1);
        assert!((area_facing(&block, Vec3::Y) - 4.0).abs() < 1e-3, "the top does not face up out of the block");
        assert!(area_facing(&block, Vec3::NEG_Y) < 1e-3, "the block kept its bottom");
        assert!((total_area(&block) - 12.0).abs() < 1e-3);
        assert!((air_volume(&of_volumes(&volumes)) - 296.0).abs() < 1e-2);
    }

    #[test]
    fn a_block_through_a_wall_is_sealed_at_the_wall() {
        let volumes = [Volume::air(hall()), Volume::solid(Room::new(Vec3::new(8.0, 0.0, 4.0), Vec3::new(12.0, 3.0, 6.0)))];
        let surface = of_volumes(&volumes);
        assert!(surface.iter().flat_map(|face| &face.vertices).all(|v| v.x <= 10.0 + 1e-4), "a face outside the hall");
        assert!((area_facing(&share(&volumes, 1), Vec3::NEG_X) - 6.0).abs() < 1e-3);
        assert!((air_volume(&surface) - (300.0 - 12.0)).abs() < 1e-2);
        assert!(!is_air(&volumes, Vec3::new(9.0, 1.0, 5.0)));
        assert!(is_air(&volumes, Vec3::new(7.0, 1.0, 5.0)));
    }

    #[test]
    fn a_room_after_a_block_tunnels_through_it() {
        let wall = Room::new(Vec3::new(0.0, 0.0, 4.0), Vec3::new(10.0, 3.0, 6.0));
        let door = Room::new(Vec3::new(2.0, 0.0, 3.0), Vec3::new(3.0, 2.0, 7.0));
        let walled = [Volume::air(hall()), Volume::solid(wall.clone())];
        let (t, _) = ray_from_inside(&of_volumes(&walled), &ray(Vec3::new(2.5, 1.0, 1.0), Vec3::Z)).unwrap();
        assert!((t - 3.0).abs() < 1e-4, "the wall is not there: {t}");

        let volumes = [Volume::air(hall()), Volume::solid(wall), Volume::air(door)];
        let surface = of_volumes(&volumes);
        let (t, _) = ray_from_inside(&surface, &ray(Vec3::new(2.5, 1.0, 1.0), Vec3::Z)).unwrap();
        assert!((t - 9.0).abs() < 1e-4, "the doorway is shut: {t}");
        let (t, _) = ray_from_inside(&surface, &ray(Vec3::new(5.0, 1.0, 1.0), Vec3::Z)).unwrap();
        assert!((t - 3.0).abs() < 1e-4, "the wall beside the doorway is gone: {t}");
        assert!((air_volume(&surface) - (300.0 - 60.0 + 4.0)).abs() < 1e-2);
        assert!(is_air(&volumes, Vec3::new(2.5, 1.0, 5.0)));
        assert!(!is_air(&volumes, Vec3::new(2.5, 2.5, 5.0)), "air above the doorway");
    }

    #[test]
    fn a_block_before_every_room_is_nothing() {
        let block = Volume::solid(Room::new(Vec3::splat(2.0), Vec3::splat(5.0)));
        let volumes = [block, Volume::air(hall())];
        assert!(share(&volumes, 0).is_empty());
        assert!((total_area(&share(&volumes, 1)) - 320.0).abs() < 1e-3);
    }

    #[test]
    fn a_block_filling_its_room_leaves_no_surface() {
        let room = Room::new(Vec3::ZERO, Vec3::splat(4.0));
        assert!(of_volumes(&[Volume::air(room.clone()), Volume::solid(room)]).is_empty());
        let bigger = Room::new(Vec3::splat(-1.0), Vec3::splat(5.0));
        assert!(of_volumes(&[Volume::air(Room::new(Vec3::ZERO, Vec3::splat(4.0))), Volume::solid(bigger)]).is_empty());
    }

    /// Three of the block's faces coincide with the hall's, all facing the
    /// same way: neither keeps them.
    #[test]
    fn a_block_flush_in_a_corner_shows_two_faces() {
        let volumes = [Volume::air(hall()), Volume::solid(Room::new(Vec3::new(8.0, 0.0, 8.0), Vec3::new(10.0, 3.0, 10.0)))];
        assert!((total_area(&share(&volumes, 1)) - 12.0).abs() < 1e-3);
        let surface = of_volumes(&volumes);
        assert!((total_area(&surface) - (96.0 + 96.0 + 108.0 + 12.0)).abs() < 1e-3, "area {}", total_area(&surface));
        assert!((air_volume(&surface) - 288.0).abs() < 1e-2);
    }

    /// `f32` spacing is about 1.5e-5 m here; the kernel's tolerance is 1e-5.
    #[test]
    fn a_shared_wall_far_from_the_origin_still_opens() {
        let rooms = [
            Room::new(Vec3::new(117.3, 0.0, 93.7), Vec3::new(123.9, 3.1, 101.3)),
            Room::new(Vec3::new(123.9, 0.0, 95.1), Vec3::new(131.7, 3.1, 99.9)),
        ];
        let a = room_share(&rooms, 0);
        let open = (99.9 - 95.1) * 3.1;
        assert!((area_facing(&a, Vec3::NEG_X) - (7.6 * 3.1 - open)).abs() < 1e-2);
    }

    fn area(faces: &[Face]) -> f32 {
        faces
            .iter()
            .map(|face| {
                let a = face.vertices[0];
                face.vertices[1..].windows(2).map(|w| (w[0] - a).cross(w[1] - a).length() / 2.0).sum::<f32>()
            })
            .sum()
    }

    /// Two triangles making a square, `size` across and `height` high, split
    /// along its diagonal, with the corner at `at`.
    fn a_square_cut_corner_to_corner(at: Vec3, size: f32, height: f32) -> [Room; 2] {
        let p = |x: f32, z: f32| Vec2::new(at.x + x, at.z + z);
        [
            Room::prism(vec![p(0.0, 0.0), p(size, 0.0), p(0.0, size)], at.y, at.y + height),
            Room::prism(vec![p(size, 0.0), p(size, size), p(0.0, size)], at.y, at.y + height),
        ]
    }

    #[test]
    fn a_diagonal_wall_two_rooms_share_is_open() {
        let rooms = a_square_cut_corner_to_corner(Vec3::ZERO, 10.0, 3.0);
        let surface = of_rooms(&rooms);
        assert!((area(&surface) - (200.0 + 120.0)).abs() < 1e-2, "area {}", area(&surface));
        let diagonal = surface.iter().filter(|face| face.plane.normal.x.abs() > 0.1 && face.plane.normal.z.abs() > 0.1).count();
        assert_eq!(diagonal, 0, "the shared diagonal wall was kept");
    }

    /// The tolerance on a wall that is not on an axis plane, where the two
    /// rooms' planes come from different corners and round differently.
    #[test]
    fn a_diagonal_wall_far_from_the_origin_still_opens() {
        let rooms = a_square_cut_corner_to_corner(Vec3::new(143.37, 1.13, 87.91), 10.3, 3.14);
        let surface = of_rooms(&rooms);
        let expected = 2.0 * 10.3 * 10.3 + 4.0 * 10.3 * 3.14;
        assert!((area(&surface) - expected).abs() < 1e-2, "area {} of {expected}", area(&surface));
    }

    #[test]
    fn a_notched_room_is_walled_round_its_notch() {
        let l = Room::prism(
            vec![Vec2::new(0.0, 0.0), Vec2::new(3.0, 0.0), Vec2::new(3.0, 1.0), Vec2::new(1.0, 1.0), Vec2::new(1.0, 3.0), Vec2::new(0.0, 3.0)],
            0.0,
            2.0,
        );
        let surface = room_share(&[l], 0);
        assert!((area(&surface) - (2.0 * 5.0 + 12.0 * 2.0)).abs() < 1e-3);
        assert!((area_facing(&surface, Vec3::Y) - 5.0).abs() < 1e-3, "the floor does not face up");
    }

    #[test]
    fn a_pillar_is_walled_and_its_floor_is_not_there() {
        let square = |lo: f32, hi: f32| vec![Vec2::new(lo, lo), Vec2::new(hi, lo), Vec2::new(hi, hi), Vec2::new(lo, hi)];
        let room = Room::with_holes(square(0.0, 4.0), vec![square(1.5, 2.5)], 0.0, 3.0);
        let surface = room_share(&[room], 0);
        assert!((area(&surface) - (15.0 + 15.0 + 48.0 + 12.0)).abs() < 1e-3, "area {}", area(&surface));
        let (t, _) = ray_from_inside(&surface, &ray(Vec3::new(0.5, 1.0, 2.0), Vec3::X)).unwrap();
        assert!((t - 1.0).abs() < 1e-4, "a shot across the room passed through the pillar: {t}");
    }

    #[test]
    fn a_pick_from_outside_passes_the_near_wall_and_lands_on_the_far_one() {
        let rooms = [Room::new(Vec3::ZERO, Vec3::splat(10.0))];
        let (t, hit) = ray_from_inside(&room_share(&rooms, 0), &ray(Vec3::new(-5.0, 5.0, 5.0), Vec3::X)).unwrap();
        assert!((t - 15.0).abs() < 1e-4, "{t}");
        assert!((hit.x - 10.0).abs() < 1e-4, "{hit}");
    }

    #[test]
    fn a_pick_goes_through_an_opening_and_not_through_the_wall_beside_it() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::splat(10.0)),
            Room::new(Vec3::new(10.0, 0.0, 2.0), Vec3::new(20.0, 4.0, 4.0)),
        ];
        let surface = of_rooms(&rooms);
        let (t, _) = ray_from_inside(&surface, &ray(Vec3::new(5.0, 2.0, 3.0), Vec3::X)).unwrap();
        assert!((t - 15.0).abs() < 1e-4, "B's far wall, got {t}");
        let (t, _) = ray_from_inside(&surface, &ray(Vec3::new(5.0, 7.0, 7.0), Vec3::X)).unwrap();
        assert!((t - 5.0).abs() < 1e-4, "A's wall beside the doorway, got {t}");
    }

    /// Breaks if the winding and the normals disagree, which culls every wall
    /// seen from inside the room.
    #[test]
    fn every_triangle_is_wound_to_face_its_normal() {
        let rooms = [
            Room::new(Vec3::ZERO, Vec3::splat(10.0)),
            Room::new(Vec3::new(10.0, 2.0, 2.0), Vec3::new(20.0, 8.0, 8.0)),
        ];
        let mesh = mesh(&room_share(&rooms, 0), |_, _| Vec2::ZERO);
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else { panic!() };
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(normals)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL) else { panic!() };
        let Some(Indices::U32(indices)) = mesh.indices() else { panic!() };
        for triangle in indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(positions[triangle[i] as usize]));
            let wound = (b - a).cross(c - a);
            assert!(wound.dot(Vec3::from(normals[triangle[0] as usize])) > 0.0, "{a} {b} {c}");
        }
    }

    #[test]
    fn the_mesh_has_a_triangle_pair_per_quad() {
        let rooms = [Room::new(Vec3::ZERO, Vec3::new(10.0, 3.0, 10.0))];
        let mesh = mesh(&room_share(&rooms, 0), |_, _| Vec2::ZERO);
        let Some(Indices::U32(indices)) = mesh.indices() else { panic!() };
        assert_eq!(indices.len() / 3, 12);
    }
}
