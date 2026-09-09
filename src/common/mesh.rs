//! Building meshes out of quads, by hand.
//!
//! A `Cuboid` bakes a closed surface of its own, so two of them meeting at a
//! knee overlap without sharing an edge, and there is no seam to sew. Keeping
//! the vertices here means a limb can hand its end ring to the next part.
//!
//! Two ideas carry the module:
//!
//! - **A ring** is a closed loop of points across a shape — its cross-section
//!   somewhere along its length. Every surface is a tube run between two
//!   rings, so a rounder shape is a longer ring rather than a second shape.
//! - **Winding is counter-clockwise seen from outside**, which is what wgpu's
//!   front face and `StandardMaterial`'s back-face culling agree on. Get it
//!   backwards and the part is not missing, which would be obvious — it is
//!   inside out, which is not.
//!
//! Each face carries its own vertices and one flat normal, which is what
//! welding will change: averaging normals across an edge means finding the two
//! faces that share a ring, and the rings are what survives here.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

/// A triangle list under construction.
///
/// Positions are in whatever frame the caller is working in; nothing here
/// knows about world space, so the same builder makes a bone's mesh in bone
/// space and a prop's in its own.
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

    /// One quad, wound counter-clockwise **seen from outside**, with its own
    /// flat normal.
    ///
    /// UVs are the unit square in corner order, which is a stand-in until
    /// something authors skins.
    pub fn quad(&mut self, corners: [Vec3; 4]) {
        self.quad_uv(corners, [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y]);
    }

    /// One triangle, wound counter-clockwise seen from outside.
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

    /// A quad with the texture coordinates spelled out.
    pub fn quad_uv(&mut self, corners: [Vec3; 4], uvs: [Vec2; 4]) {
        // From the first corner: a quad built from a ring is planar by
        // construction, so one triangle's normal is the quad's.
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

    /// The wall between two rings of the same length.
    ///
    /// `head` and `tail` are the same loop at two places along a shape, listed
    /// in the same order and going the same way round. Which is which decides
    /// which way the wall faces: rings wound clockwise seen from `head`
    /// looking towards `tail` give a wall that faces outwards.
    ///
    /// Panics on a length mismatch, because the alternative is a shape with a
    /// hole in it and no error to go with it.
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

    /// Close a ring off with a fan from its centre.
    ///
    /// `reverse` flips the winding, which the two ends of a shape need in
    /// opposite senses: one ring order caps correctly at the head and inside
    /// out at the tail.
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

    /// A closed solid from a stack of rings: walls between each neighbouring
    /// pair, and a cap on each end. Rings run head to tail, all the same
    /// length.
    pub fn hull(&mut self, rings: &[Vec<Vec3>]) {
        let Some(first) = rings.first() else { return };
        for pair in rings.windows(2) {
            self.tube(&pair[0], &pair[1]);
        }
        self.cap(first, false);
        self.cap(rings.last().expect("checked non-empty above"), true);
    }

    /// Hand the result to Bevy.
    ///
    /// `RenderAssetUsages::default()` keeps the data in main memory as well as
    /// on the GPU, for the kilobytes it costs: a weld pass, a decal or an
    /// exporter would otherwise have to rebuild the mesh to read it.
    pub fn build(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// A rectangular ring in the plane `y = y`, centred on `offset`.
///
/// Wound so a stack built smallest-`y`-first faces outwards under
/// [`MeshBuilder::hull`], which is the convention a bone uses: local `+Y` runs
/// head to tail, so a ring's `y` is how far along it sits.
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

/// A rectangular ring with its corners cut off, in the plane `y = y`.
///
/// Eight points rather than four, wound the way [`rect_ring`] is, so the two
/// are interchangeable within a shape whose every ring agrees. `chamfer` is
/// the fraction of each half-extent a corner takes: at zero this degenerates
/// into [`rect_ring`] with four coincident pairs, so callers pick between the
/// two rather than passing zero; at one it is a diamond.
///
/// Eight quads instead of four, and most of the difference between a limb that
/// reads as a box and one that reads as an arm.
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

    /// A cube built the way a bone is, checked face by face.
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

    /// The rule the whole module rests on, and the one whose failure looks
    /// like a rendering bug rather than a maths one: every face points away
    /// from the middle of the solid.
    #[test]
    fn every_face_of_a_hull_faces_outwards() {
        let mesh = cube();
        let positions = attribute(&mesh, Mesh::ATTRIBUTE_POSITION.id);
        let normals = attribute(&mesh, Mesh::ATTRIBUTE_NORMAL.id);
        let centre = Vec3::new(0.0, 0.5, 0.0);

        for (position, normal) in positions.iter().zip(&normals) {
            let position = Vec3::from_array(*position);
            let normal = Vec3::from_array(*normal);
            // Vertices sit on the corners, so "outwards" is measured from the
            // solid's middle to the vertex; a fan's centre vertex is the one
            // exception and it lies on its own face's plane.
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

    /// Six faces, and no more geometry than six faces: a hull that quietly
    /// emitted its walls twice would look identical and cost double.
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

    /// Rings are wound one way; a shape whose taper reversed part way along
    /// would turn itself inside out at that ring rather than pinching.
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
            // The caps are the ends, not the taper, and they point straight
            // along the axis; the fan's centre vertex has no sideways
            // direction to be checked against either.
            if outward.xz().length() < 1e-4 || normal.xz().length() < 1e-4 {
                continue;
            }
            // Only the horizontal part: a tapered wall leans, so its normal
            // has a `y` and comparing the full vectors would fail on a
            // perfectly good cone.
            assert!(
                normal.xz().dot(outward.xz()) > 0.0,
                "a tapered face points inwards at {position}",
            );
        }
    }
}
