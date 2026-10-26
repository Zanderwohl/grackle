//! The map's surface: the boundary of the union of its rooms, faces pointing
//! into the air. What is drawn, what is collided with and what a pick lands on
//! are all this, so an opening you can see through is one you can walk and
//! shoot through. See `documentation/collision.md`.
//!
//! Each room's share is its own walls with whatever other rooms open up
//! clipped away, so one room can be rebuilt without the rest. Two rooms
//! sharing a wall lose it; a room crossing another's wall cuts a hole in it; a
//! room wholly inside another adds nothing. Where two rooms' faces coincide
//! facing the same way — floors at one height, overlapping — exactly one
//! keeps it, by an order on the rooms that does not depend on which is being
//! rebuilt.

use std::cmp::Ordering;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::common::csg::{self, Polygon};
use crate::tool::room::Room;

/// A piece of the map's surface. Walls are made of nothing yet.
pub type Face = Polygon<()>;

/// The share of the surface that belongs to `rooms[index]`, facing into it.
pub fn room_share(rooms: &[Room], index: usize) -> Vec<Face> {
    let room = &rooms[index];
    let (min, max) = bounds(room);
    let mut faces = csg::cuboid(min, max, ());

    for (other_index, other) in rooms.iter().enumerate() {
        if other_index == index || !touching(room, other) {
            continue;
        }
        let (other_min, other_max) = bounds(other);
        let owns_shared = precedes(other, other_index, room, index);
        faces = csg::clip_outside(faces, &csg::cuboid(other_min, other_max, ()), owns_shared);
        if faces.is_empty() {
            break;
        }
    }

    faces.iter_mut().for_each(Polygon::flip);
    faces
}

/// Every room's share, in room order.
pub fn of_rooms(rooms: &[Room]) -> Vec<Face> {
    (0..rooms.len()).flat_map(|index| room_share(rooms, index)).collect()
}

fn bounds(room: &Room) -> (Vec3, Vec3) {
    (room.min.min(room.max), room.min.max(room.max))
}

/// Overlapping or meeting, faces and edges included: a room meeting this one
/// along a wall opens it.
fn touching(a: &Room, b: &Room) -> bool {
    let ((a_min, a_max), (b_min, b_max)) = (bounds(a), bounds(b));
    (0..3).all(|axis| a_min[axis] <= b_max[axis] && b_min[axis] <= a_max[axis])
}

/// Which of two rooms keeps a face they share facing the same way: the lower
/// box, then the lower index for two boxes that are the same.
fn precedes(a: &Room, a_index: usize, b: &Room, b_index: usize) -> bool {
    let key = |room: &Room| {
        let (min, max) = bounds(room);
        [min.x, min.y, min.z, max.x, max.y, max.z]
    };
    let by_box = key(a)
        .iter()
        .zip(key(b))
        .map(|(x, y)| x.total_cmp(&y))
        .find(|order| *order != Ordering::Equal)
        .unwrap_or(Ordering::Equal);
    by_box.then(a_index.cmp(&b_index)) == Ordering::Less
}

/// Flat-shaded triangles, normals into the room. UVs are metres across the
/// face's two widest axes; nothing is textured yet.
pub fn mesh(faces: &[Face]) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for face in faces {
        let normal = face.plane.normal;
        let flat = normal.abs();
        let (u, v) = if flat.x >= flat.y && flat.x >= flat.z {
            (2, 1)
        } else if flat.y >= flat.z {
            (0, 2)
        } else {
            (0, 1)
        };
        let base = positions.len() as u32;
        for vertex in &face.vertices {
            positions.push(vertex.to_array());
            normals.push(normal.to_array());
            uvs.push([vertex[u], vertex[v]]);
        }
        for i in 1..face.vertices.len() as u32 - 1 {
            indices.extend_from_slice(&[base, base + i, base + i + 1]);
        }
    }

    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
}

/// Where `ray` first meets a face from the side it faces — from inside, for a
/// room's share — and how far along. Edges count as face, so a pick on the
/// seam between two pieces of one wall does not fall through.
pub fn ray_from_inside<'a>(faces: impl IntoIterator<Item = &'a Face>, ray: &Ray3d) -> Option<(f32, Vec3)> {
    let direction = Vec3::from(ray.direction);
    let mut best: Option<(f32, Vec3)> = None;
    for face in faces {
        let normal = face.plane.normal;
        let closing = normal.dot(direction);
        if closing >= 0.0 {
            continue;
        }
        let t = (face.plane.w - normal.dot(ray.origin)) / closing;
        if t < 0.0 || best.is_some_and(|(best_t, _)| t >= best_t) {
            continue;
        }
        let hit = ray.origin + direction * t;
        if contains(face, hit) {
            best = Some((t, hit));
        }
    }
    best
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
        let mesh = mesh(&room_share(&rooms, 0));
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
        let mesh = mesh(&room_share(&rooms, 0));
        let Some(Indices::U32(indices)) = mesh.indices() else { panic!() };
        assert_eq!(indices.len() / 3, 12);
    }
}
