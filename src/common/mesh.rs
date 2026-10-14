//! Building meshes out of quads, by hand.
//!
//! Two `Cuboid`s meeting at a knee are two closed surfaces with no shared edge
//! to sew; keeping the vertices here lets a limb hand its end ring to the next
//! part.
//!
//! - A ring is a closed loop of points across a shape. Every surface is a tube
//!   between two rings.
//! - Winding is counter-clockwise seen from outside (wgpu's front face).
//!   Backwards, a part renders inside out rather than missing.
//!
//! Each face has its own vertices and one flat normal.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

/// A triangle list under construction, in whatever frame the caller uses.
#[derive(Clone, Debug, Default)]
pub struct MeshBuilder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl MeshBuilder {
    pub fn new() -> MeshBuilder {
        MeshBuilder::default()
    }

    /// One quad, wound counter-clockwise seen from outside, with its own flat
    /// normal. UVs are a placeholder unit square.
    pub fn quad(&mut self, corners: [Vec3; 4]) {
        self.quad_uv(corners, [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y]);
    }

    pub fn tri(&mut self, corners: [Vec3; 3]) {
        let normal = (corners[1] - corners[0])
            .cross(corners[2] - corners[0])
            .normalize_or_zero();

        let base = self.positions.len() as u32;
        for (corner, uv) in corners.into_iter().zip([Vec2::ZERO, Vec2::X, Vec2::ONE]) {
            self.positions.push(corner.to_array());
            self.normals.push(normal.to_array());
            self.uvs.push(uv.to_array());
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    pub fn quad_uv(&mut self, corners: [Vec3; 4], uvs: [Vec2; 4]) {
        // A quad built from a ring is planar, so one triangle's normal is the quad's.
        let normal = (corners[1] - corners[0])
            .cross(corners[2] - corners[0])
            .normalize_or_zero();

        let base = self.positions.len() as u32;
        for (corner, uv) in corners.into_iter().zip(uvs) {
            self.positions.push(corner.to_array());
            self.normals.push(normal.to_array());
            self.uvs.push(uv.to_array());
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// The wall between two rings listed in the same order. Rings wound clockwise
    /// seen from `head` towards `tail` give an outward-facing wall.
    ///
    /// Panics on a length mismatch rather than leaving a silent hole.
    pub fn tube(&mut self, head: &[Vec3], tail: &[Vec3]) {
        assert_eq!(
            head.len(),
            tail.len(),
            "a tube joins two rings of the same length; got {} and {}",
            head.len(),
            tail.len()
        );
        let n = head.len();
        for i in 0..n {
            let j = (i + 1) % n;
            let (u0, u1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
            self.quad_uv(
                [head[i], tail[i], tail[j], head[j]],
                [
                    Vec2::new(u0, 0.0),
                    Vec2::new(u0, 1.0),
                    Vec2::new(u1, 1.0),
                    Vec2::new(u1, 0.0),
                ],
            );
        }
    }

    /// Close a ring off with a fan from its centre. `reverse` flips the winding;
    /// the two ends of a shape need opposite senses.
    pub fn cap(&mut self, ring: &[Vec3], reverse: bool) {
        if ring.len() < 3 {
            return;
        }
        let centre = ring.iter().copied().sum::<Vec3>() / ring.len() as f32;
        let n = ring.len();
        for i in 0..n {
            let j = (i + 1) % n;
            let (a, b) = if reverse { (ring[j], ring[i]) } else { (ring[i], ring[j]) };
            self.tri([centre, a, b]);
        }
    }

    /// A closed solid from rings running head to tail, all the same length.
    pub fn hull(&mut self, rings: &[Vec<Vec3>]) {
        let Some(first) = rings.first() else { return };
        for pair in rings.windows(2) {
            self.tube(&pair[0], &pair[1]);
        }
        self.cap(first, false);
        self.cap(rings.last().expect("checked non-empty above"), true);
    }

    /// `RenderAssetUsages::default()` keeps the data in main memory too, so a weld
    /// pass or exporter can read it without rebuilding the mesh.
    pub fn build(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// A rectangular ring in the plane `y = y`, centred on `offset`. Wound so a
/// stack built smallest-`y`-first faces outwards under [`MeshBuilder::hull`]
/// (a bone's local `+Y` runs head to tail).
pub fn rect_ring(y: f32, half: Vec2, offset: Vec2) -> Vec<Vec3> {
    [
        Vec2::new(-half.x, -half.y),
        Vec2::new(half.x, -half.y),
        Vec2::new(half.x, half.y),
        Vec2::new(-half.x, half.y),
    ]
    .into_iter()
    .map(|corner| {
        let corner = corner + offset;
        Vec3::new(corner.x, y, corner.y)
    })
    .collect()
}

/// A rectangular ring with its corners cut off, wound like [`rect_ring`] so the
/// two can share a shape. `chamfer` is the fraction of each half-extent a
/// corner takes: zero gives four coincident pairs (use [`rect_ring`] instead),
/// one gives a diamond.
pub fn rounded_ring(y: f32, half: Vec2, offset: Vec2, chamfer: f32) -> Vec<Vec3> {
    let cut = half * chamfer.clamp(0.0, 1.0);
    [
        Vec2::new(-half.x + cut.x, -half.y),
        Vec2::new(half.x - cut.x, -half.y),
        Vec2::new(half.x, -half.y + cut.y),
        Vec2::new(half.x, half.y - cut.y),
        Vec2::new(half.x - cut.x, half.y),
        Vec2::new(-half.x + cut.x, half.y),
        Vec2::new(-half.x, half.y - cut.y),
        Vec2::new(-half.x, -half.y + cut.y),
    ]
    .into_iter()
    .map(|corner| {
        let corner = corner + offset;
        Vec3::new(corner.x, y, corner.y)
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Mesh {
        let mut builder = MeshBuilder::new();
        builder.hull(&[
            rect_ring(0.0, Vec2::splat(0.5), Vec2::ZERO),
            rect_ring(1.0, Vec2::splat(0.5), Vec2::ZERO),
        ]);
        builder.build()
    }

    fn attribute(mesh: &Mesh, id: bevy::mesh::MeshVertexAttributeId) -> Vec<[f32; 3]> {
        match mesh.attribute(id).expect("attribute is there") {
            bevy::mesh::VertexAttributeValues::Float32x3(values) => values.clone(),
            other => panic!("wrong attribute format: {other:?}"),
        }
    }

    /// A face pointing inwards renders as a hole from one side.
    #[test]
    fn every_face_of_a_hull_faces_outwards() {
        let mesh = cube();
        let positions = attribute(&mesh, Mesh::ATTRIBUTE_POSITION.id);
        let normals = attribute(&mesh, Mesh::ATTRIBUTE_NORMAL.id);
        let centre = Vec3::new(0.0, 0.5, 0.0);

        for (position, normal) in positions.iter().zip(&normals) {
            let position = Vec3::from_array(*position);
            let normal = Vec3::from_array(*normal);
            // A fan's centre vertex lies on its own face's plane; skip it.
            let outward = position - centre;
            if outward.length() < 1e-4 {
                continue;
            }
            assert!(
                normal.dot(outward) > 0.0,
                "the face at {position} points inwards: normal {normal}",
            );
        }
    }

    /// A hull that emitted its walls twice would look identical and cost double.
    #[test]
    fn a_two_ring_hull_is_a_box() {
        let mesh = cube();
        let normals = attribute(&mesh, Mesh::ATTRIBUTE_NORMAL.id);

        for axis in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z] {
            assert!(
                normals
                    .iter()
                    .any(|normal| Vec3::from_array(*normal).dot(axis) > 0.99),
                "no face points along {axis}",
            );
        }

            // Four walls of four vertices, and two caps of four triangles.
        assert_eq!(mesh.count_vertices(), 4 * 4 + 2 * 4 * 3);
    }

    /// A taper must not turn the shape inside out at an interior ring.
    #[test]
    fn a_taper_keeps_facing_outwards() {
        let mut builder = MeshBuilder::new();
        builder.hull(&[
            rect_ring(0.0, Vec2::splat(0.5), Vec2::ZERO),
            rect_ring(0.5, Vec2::splat(0.3), Vec2::ZERO),
            rect_ring(1.0, Vec2::splat(0.1), Vec2::ZERO),
        ]);
        let mesh = builder.build();
        let positions = attribute(&mesh, Mesh::ATTRIBUTE_POSITION.id);
        let normals = attribute(&mesh, Mesh::ATTRIBUTE_NORMAL.id);

        for (position, normal) in positions.iter().zip(&normals) {
            let position = Vec3::from_array(*position);
            let outward = position - Vec3::new(0.0, 0.5, 0.0);
            let normal = Vec3::from_array(*normal);
            // Skip the caps and fan centres, which have no sideways direction.
            if outward.xz().length() < 1e-4 || normal.xz().length() < 1e-4 {
                continue;
            }
            // Horizontal part only: a tapered wall's normal has a `y`.
            assert!(
                normal.xz().dot(outward.xz()) > 0.0,
                "a tapered face points inwards at {position}",
            );
        }
    }
}
