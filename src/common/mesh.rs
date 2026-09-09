//! Building meshes out of quads, by hand.
//!
//! Bevy's primitives ([`bevy::math::primitives::Cuboid`] and friends) each
//! bake their own self-contained mesh, and that is exactly what a character
//! built out of parts cannot use: two boxes meeting at a knee are two closed
//! surfaces that happen to overlap, and there is no seam to sew because
//! neither one has an edge the other shares. Rolling our own keeps the
//! vertices in our hands, so a limb can later hand its end ring to the next
//! part instead of capping it off.
//!
//! Two ideas carry the whole module:
//!
//! - **A ring** is a closed loop of points around a shape — the cross-section
//!   at some point along it. Every surface here is built by running a tube
//!   between two rings, so making a shape rounder is giving its rings more
//!   points, not writing a second shape.
//! - **Winding is outward-facing everywhere.** A quad is listed
//!   counter-clockwise seen from outside the solid, which is what wgpu's
//!   default front face and `StandardMaterial`'s back-face culling agree on.
//!   Get it backwards and the part is not missing, which would be obvious —
//!   it is inside out, which is not.
//!
//! Vertices are **not** shared between faces: each quad carries its own four,
//! with one flat normal. That is a deliberate first cut — it gives crisp
//! low-poly facets and it makes every face independent. Welding is the
//! follow-up, and the ring-shaped API above is what it will be written
//! against: averaging normals across an edge means finding the two faces that
//! share a ring, and rings are the thing that survives here.
//!
//! Plain maths over `bevy_math` types plus one `Mesh`. No assets, no file
//! system: it builds for wasm as-is.

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

    /// How many vertices have been emitted so far.
    ///
    /// Only really useful for asserting on a shape's cost in a test, which is
    /// the one thing about a generated mesh that is easy to get wrong by an
    /// order of magnitude without noticing.
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    /// One quad, wound counter-clockwise **seen from outside**, with its own
    /// flat normal.
    ///
    /// UVs are the unit square in the order given, which is a stand-in: the
    /// bodies are untextured, and a real layout is a job for whatever authors
    /// skins.
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
        // From the first corner, because a quad built from a ring is planar by
        // construction and a degenerate first triangle would mean a
        // degenerate quad.
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
    /// in the same order and going the same way round. Which one is which
    /// decides which way the wall faces: with the rings wound so that they run
    /// clockwise seen from `head` looking towards `tail`, the wall faces
    /// outwards.
    ///
    /// Panics on a length mismatch. That is a mistake in a shape's own
    /// definition — a taper that dropped a corner — and quietly skirting it
    /// would produce a hole rather than an error.
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
    /// `reverse` flips the winding, which is what the two ends of a shape
    /// need: a ring wound to face outwards as a wall caps correctly at one end
    /// and inside out at the other.
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
    /// pair, and a cap on each end.
    ///
    /// The one call most shapes actually want. Rings run from the head end to
    /// the tail end, all the same length.
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
    /// on the GPU. A body mesh is small and something will want to read it
    /// back — a decal, a weld pass, an exporter — and rebuilding it to answer
    /// that would be worse than the handful of kilobytes.
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
/// Wound so that a stack of these built head-first — smaller `y` first — comes
/// out facing outwards under [`MeshBuilder::hull`]. That is the convention a
/// bone uses: local `+Y` runs down the bone, so a ring's `y` is how far along
/// it sits.
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
/// Eight points rather than four, wound the same way [`rect_ring`] is, so the
/// two are interchangeable within one shape as long as every ring of that
/// shape agrees. `chamfer` is the fraction of each half-extent the corner
/// takes: zero is [`rect_ring`] with four coincident pairs — which is why the
/// caller picks between them rather than passing zero — and one is a diamond.
///
/// This is most of the difference between a limb that reads as a box and one
/// that reads as an arm, and it costs eight quads instead of four.
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
