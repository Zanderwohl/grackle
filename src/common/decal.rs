//! A decal's geometry: a box projected onto the map's surface, cut out of the faces it covers.
//! Everything here takes faces, never a room's box. See `documentation/decals.md`.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::lightmap::Atlas;
use crate::common::sky::SKYBOX;
use crate::common::world_surface::{self, Face};
use crate::tool::room::RoomSlot;

/// Meters a piece stands off its wall, on top of its material's depth bias.
const LIFT: f32 = 0.001;
/// A face turned further than this from the projection takes nothing: a mark wrapped round a
/// corner onto a wall it is edge-on to smears into a stripe.
const MIN_FACING: f32 = 0.35;
/// Facing from which a piece is drawn at full strength, fading to nothing at [`MIN_FACING`].
const FULL_FACING: f32 = 0.7;

/// Where a decal lands: a box `size` across and `depth` deep, centered on `center`, projecting
/// into the wall along `-normal`. Texture `u` runs along `up × normal`, `v` down `up`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    pub center: Vec3,
    /// Out of the wall, unit length.
    pub normal: Vec3,
    /// Toward the top of the picture. Need not be perpendicular to `normal`.
    pub up: Vec3,
    /// Meters, width and height.
    pub size: Vec2,
    /// Meters, all of it: half in front of `center` and half behind.
    pub depth: f32,
}

impl Projection {
    /// Square, on a wall at `at` facing `normal`, picture upright: world up on a wall, and on a
    /// floor or ceiling `facing`, which is usually the way the shot or the viewer went.
    pub fn on(at: Vec3, normal: Vec3, size: f32, depth: f32, facing: Vec3) -> Self {
        let up = if normal.y.abs() < 0.9 { Vec3::Y } else { facing.reject_from(normal).try_normalize().unwrap_or(Vec3::Z) };
        Self { center: at, normal, up, size: Vec2::splat(size), depth }
    }

    /// Right, up and out, orthonormal.
    pub fn frame(&self) -> (Vec3, Vec3, Vec3) {
        let normal = self.normal.normalize();
        let right = self.up.cross(normal).try_normalize().unwrap_or_else(|| normal.any_orthonormal_vector());
        (right, normal.cross(right), normal)
    }

    /// The axis-aligned box around the projection.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let (right, up, out) = self.frame();
        let half = right.abs() * self.size.x * 0.5 + up.abs() * self.size.y * 0.5 + out.abs() * self.depth * 0.5;
        (self.center - half, self.center + half)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Piece {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Into `atlas`, the room's lightmap layout.
    pub lightmap_uvs: Vec<[f32; 2]>,
    /// White, faded by how squarely the face meets the projection.
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl Piece {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn mesh(&self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, self.lightmap_uvs.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors.clone())
            .with_inserted_indices(Indices::U32(self.indices.clone()))
    }
}

/// `material` as the palette resolves it, not a role. Sky is drawn at infinite distance, so a mark
/// on it would hang in the air.
pub fn takes_decals(material: Option<&str>) -> bool {
    material != Some(SKYBOX)
}

/// `faces` cut to `projection`, leaving out the faces of any slot `takes` refuses. `atlas` must
/// be `Atlas::of(faces)`, the layout the room's lightmap is in; every cut vertex lies on its face,
/// so its chart gives `UV_1` exactly.
pub fn cut(projection: &Projection, faces: &[Face], atlas: &Atlas, takes: impl Fn(RoomSlot) -> bool) -> Piece {
    let (right, up, out) = projection.frame();
    let half = Vec3::new(projection.size.x, projection.size.y, projection.depth) * 0.5;
    let planes = [(right, half.x), (-right, half.x), (up, half.y), (-up, half.y), (out, half.z), (-out, half.z)];
    let mut piece = Piece::default();

    for (index, face) in faces.iter().enumerate() {
        let facing = face.plane.normal.dot(out);
        if facing < MIN_FACING || !takes(RoomSlot::of(face.plane.normal)) {
            continue;
        }
        let mut polygon = face.vertices.clone();
        for (axis, limit) in planes {
            polygon = clip(&polygon, |p| limit - (p - projection.center).dot(axis));
            if polygon.len() < 3 {
                break;
            }
        }
        if polygon.len() < 3 {
            continue;
        }
        let strength = ((facing - MIN_FACING) / (FULL_FACING - MIN_FACING)).clamp(0.0, 1.0);
        let base = piece.positions.len() as u32;
        for p in &polygon {
            let local = *p - projection.center;
            piece.positions.push((*p + face.plane.normal * LIFT).to_array());
            piece.normals.push(face.plane.normal.to_array());
            piece.uvs.push([local.dot(right) / projection.size.x + 0.5, 0.5 - local.dot(up) / projection.size.y]);
            piece.lightmap_uvs.push(atlas.uv(index, *p).to_array());
            piece.colors.push([1.0, 1.0, 1.0, strength]);
        }
        for i in 1..polygon.len() as u32 - 1 {
            piece.indices.extend_from_slice(&[base, base + i, base + i + 1]);
        }
    }
    piece
}

/// The part of a convex polygon where `inside` is not negative (Sutherland–Hodgman).
fn clip(polygon: &[Vec3], inside: impl Fn(Vec3) -> f32) -> Vec<Vec3> {
    let mut kept = Vec::with_capacity(polygon.len() + 2);
    for (i, &a) in polygon.iter().enumerate() {
        let b = polygon[(i + 1) % polygon.len()];
        let (da, db) = (inside(a), inside(b));
        if da >= 0.0 {
            kept.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            kept.push(a + (b - a) * (da / (da - db)));
        }
    }
    kept
}

/// The first face `faces` puts in the way of `range` meters from `origin` along unit
/// `direction`, from the side it faces: where, and the face's normal.
pub fn hit<'a>(faces: impl IntoIterator<Item = &'a Face>, origin: Vec3, direction: Vec3, range: f32) -> Option<(Vec3, Vec3)> {
    hit_tagged(faces.into_iter().map(|face| (face, ())), origin, direction, range).map(|(at, normal, ())| (at, normal))
}

/// [`hit`], returning the tag of the face hit.
pub fn hit_tagged<'a, T>(
    faces: impl IntoIterator<Item = (&'a Face, T)>,
    origin: Vec3,
    direction: Vec3,
    range: f32,
) -> Option<(Vec3, Vec3, T)> {
    faces
        .into_iter()
        .filter_map(|(face, tag)| {
            world_surface::meets(face, origin, direction).filter(|t| *t <= range).map(|t| (t, face.plane.normal, tag))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(t, normal, tag)| (origin + direction * t, normal, tag))
}

/// Whether two axis-aligned boxes overlap, touching included.
pub fn overlaps(a: (Vec3, Vec3), b: (Vec3, Vec3)) -> bool {
    a.0.cmple(b.1).all() && b.0.cmple(a.1).all()
}

pub fn bounds_of(faces: &[Face]) -> Option<(Vec3, Vec3)> {
    let mut vertices = faces.iter().flat_map(|face| face.vertices.iter().copied());
    let first = vertices.next()?;
    Some(vertices.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::room::Room;

    fn room_faces() -> Vec<Face> {
        world_surface::of_rooms(&[Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))])
    }

    fn area(piece: &Piece) -> f32 {
        piece
            .indices
            .chunks(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(piece.positions[i as usize]));
                (b - a).cross(c - a).length() * 0.5
            })
            .sum()
    }

    #[test]
    fn a_mark_in_the_middle_of_a_wall_is_its_full_size() {
        let faces = room_faces();
        let projection = Projection::on(Vec3::new(2.0, 1.5, 0.0), Vec3::Z, 0.5, 0.2, Vec3::NEG_Z);
        let piece = cut(&projection, &faces, &Atlas::of(&faces), |_| true);
        assert!((area(&piece) - 0.25).abs() < 1e-4, "{}", area(&piece));
        for uv in &piece.uvs {
            assert!(uv.iter().all(|c| (-1e-4..=1.0001).contains(c)), "{uv:?}");
        }
    }

    #[test]
    fn a_mark_in_a_corner_takes_only_the_faces_it_faces() {
        let faces = room_faces();
        // On the floor, against the wall at z = 0: half of it would hang in the wall.
        let projection = Projection::on(Vec3::new(2.0, 0.0, 0.0), Vec3::Y, 1.0, 0.4, Vec3::NEG_Z);
        let piece = cut(&projection, &faces, &Atlas::of(&faces), |_| true);
        assert!((area(&piece) - 0.5).abs() < 1e-4, "{}", area(&piece));
        assert!(piece.normals.iter().all(|n| *n == [0.0, 1.0, 0.0]));
    }

    #[test]
    fn a_mark_in_a_corner_leaves_out_a_slot_that_refuses_it() {
        let faces = room_faces();
        // Pressed in at 45°, so both the floor and the wall face it well enough to take a piece.
        let projection = Projection::on(Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 1.0).normalize(), 1.0, 0.4, Vec3::NEG_Z);
        let both = cut(&projection, &faces, &Atlas::of(&faces), |_| true);
        assert!(both.normals.contains(&[0.0, 1.0, 0.0]) && both.normals.contains(&[0.0, 0.0, 1.0]));
        let refused = cut(&projection, &faces, &Atlas::of(&faces), |slot| slot != RoomSlot::Floor);
        assert!(!refused.is_empty() && refused.normals.iter().all(|n| *n == [0.0, 0.0, 1.0]), "{:?}", refused.normals);
        assert!(area(&refused) < area(&both));
        assert!(!takes_decals(Some(SKYBOX)));
        assert!(takes_decals(Some("concrete")) && takes_decals(None));
    }

    #[test]
    fn the_picture_is_upright_and_not_mirrored_seen_from_the_room() {
        // The wall at z = 0 faces +Z; seen from inside, +X is to the right.
        let projection = Projection::on(Vec3::new(2.0, 1.5, 0.0), Vec3::Z, 1.0, 0.2, Vec3::NEG_Z);
        let (right, up, _) = projection.frame();
        assert!(right.abs_diff_eq(Vec3::X, 1e-6) && up.abs_diff_eq(Vec3::Y, 1e-6));
    }

    #[test]
    fn a_ray_finds_the_wall_it_meets_and_not_the_one_behind_it() {
        let faces = room_faces();
        let (at, normal) = hit(&faces, Vec3::new(2.0, 1.5, 2.0), Vec3::NEG_Z, 5.0).unwrap();
        assert!(at.abs_diff_eq(Vec3::new(2.0, 1.5, 0.0), 1e-5));
        assert_eq!(normal, Vec3::Z);
        assert!(hit(&faces, Vec3::new(2.0, 1.5, 2.0), Vec3::NEG_Z, 1.5).is_none());
    }
}
