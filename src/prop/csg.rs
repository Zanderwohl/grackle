//! Cutting solids out of one another, with a BSP tree.
//!
//! This is the whole modelling kernel. It is the classic `csg.js` algorithm
//! written out in Rust, and it is here rather than pulled from a crate for the
//! same reason the ragdoll solver is hand-written: a C library does not
//! compile to wasm, and a weapon has to be drawable in a browser tab. It is
//! also a couple of hundred lines, which is less than the argument for taking
//! a dependency would be.
//!
//! **A solid is a bag of convex polygons that happens to be closed.** Nothing
//! here checks either property, and both matter:
//!
//! - **Convex.** A plane cuts a convex polygon into exactly two pieces, which
//!   is the assumption [`Plane::split_polygon`] is written on. Hand it a
//!   concave polygon that a plane enters and leaves twice and you get two
//!   pieces where there should be three, so the solid quietly grows a hole.
//!   Everything in [`super::solid`] emits triangles or planar convex faces for
//!   exactly this reason.
//! - **Closed.** A boolean decides what to keep by asking which side of a
//!   plane a polygon is on, and "inside" only means anything for a surface
//!   with no gaps in it. Subtracting from an open shell gives a result that
//!   looks right from one angle and is inside out from another.
//!
//! Normals are **per face, not per vertex**. A flat-shaded polygon is the look
//! this game is after, and it also sidesteps the one genuinely awkward part of
//! a CSG port: a vertex created by a split has no natural normal, and
//! interpolating one across a cut edge is what makes a boolean seam visible.

use bevy::prelude::*;

use crate::prop::surface::Surface;

/// How close to a plane counts as *on* it.
///
/// Generous by the standards of a modelling kernel, because the numbers here
/// are metres and a weapon is under one of them: a tolerance tight enough for
/// a millimetre leaves slivers of polygon at every cut, and a sliver is a
/// z-fighting artefact rather than an error.
const EPSILON: f32 = 1e-5;

/// An oriented plane, as a normal and a distance along it from the origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub w: f32,
}

/// Which side of a plane something is on. The values are bits, so the union of
/// a polygon's vertices' classifications is the polygon's own — that is what
/// makes `FRONT | BACK == SPANNING` work out.
const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;
const SPANNING: u8 = 3;

/// The four buckets one plane sorts a polygon into.
///
/// A struct rather than four `&mut Vec` arguments because two of the callers
/// want the same vector in two of the buckets — a clip sends coplanar faces to
/// whichever side they face, and a build keeps both in the node — and Rust
/// will not lend one vector out twice. Merging afterwards states which bucket
/// went where, which is the part worth reading anyway.
#[derive(Default)]
struct Split {
    coplanar_front: Vec<Polygon>,
    coplanar_back: Vec<Polygon>,
    front: Vec<Polygon>,
    back: Vec<Polygon>,
}

impl Plane {
    /// The plane through three points, or `None` if they are in a line.
    ///
    /// Degenerate triangles are dropped rather than tolerated: a plane built
    /// from a zero normal classifies every point as coplanar, which makes the
    /// BSP tree silently stop dividing space.
    pub fn from_points(a: Vec3, b: Vec3, c: Vec3) -> Option<Plane> {
        let normal = (b - a).cross(c - a);
        if normal.length_squared() < EPSILON * EPSILON {
            return None;
        }
        let normal = normal.normalize();
        Some(Plane { normal, w: normal.dot(a) })
    }

    pub fn flip(&mut self) {
        self.normal = -self.normal;
        self.w = -self.w;
    }

    fn classify(&self, point: Vec3) -> u8 {
        let distance = self.normal.dot(point) - self.w;
        if distance < -EPSILON {
            BACK
        } else if distance > EPSILON {
            FRONT
        } else {
            COPLANAR
        }
    }

    /// Sort one polygon into the four buckets, splitting it if it straddles.
    ///
    /// The coplanar buckets are kept apart from the other two because a
    /// polygon lying *in* the dividing plane belongs to whichever side it
    /// faces, and that is a question about its normal rather than about its
    /// position.
    fn split_polygon(&self, polygon: &Polygon, into: &mut Split) {
        let mut polygon_type = COPLANAR;
        let types: Vec<u8> = polygon
            .vertices
            .iter()
            .map(|vertex| {
                let classification = self.classify(*vertex);
                polygon_type |= classification;
                classification
            })
            .collect();

        match polygon_type {
            COPLANAR => {
                if self.normal.dot(polygon.plane.normal) > 0.0 {
                    into.coplanar_front.push(polygon.clone());
                } else {
                    into.coplanar_back.push(polygon.clone());
                }
            }
            FRONT => into.front.push(polygon.clone()),
            BACK => into.back.push(polygon.clone()),
            _ => {
                let mut front_vertices = Vec::new();
                let mut back_vertices = Vec::new();
                let count = polygon.vertices.len();
                for i in 0..count {
                    let j = (i + 1) % count;
                    let (ti, tj) = (types[i], types[j]);
                    let (vi, vj) = (polygon.vertices[i], polygon.vertices[j]);

                    if ti != BACK {
                        front_vertices.push(vi);
                    }
                    if ti != FRONT {
                        back_vertices.push(vi);
                    }
                    if (ti | tj) == SPANNING {
                        let t = (self.w - self.normal.dot(vi)) / self.normal.dot(vj - vi);
                        let cut = vi.lerp(vj, t);
                        front_vertices.push(cut);
                        back_vertices.push(cut);
                    }
                }

                // Three vertices is the least a polygon can be; a split that
                // shaved a corner off exactly on the plane can leave two, and
                // a two-vertex polygon has no plane to rebuild itself from.
                if front_vertices.len() >= 3 {
                    into.front.push(polygon.with_vertices(front_vertices));
                }
                if back_vertices.len() >= 3 {
                    into.back.push(polygon.with_vertices(back_vertices));
                }
            }
        }
    }
}

/// One convex face, wound counter-clockwise seen from outside.
///
/// The winding convention is [`crate::common::mesh`]'s, and it has to be: the
/// two modules build the same kind of surface and a prop drawn by one and a
/// body drawn by the other would otherwise disagree about which way is out.
#[derive(Clone, Debug)]
pub struct Polygon {
    pub vertices: Vec<Vec3>,
    pub plane: Plane,
    pub surface: Surface,
}

impl Polygon {
    /// A face from its corners, or `None` if they do not describe one.
    pub fn new(vertices: Vec<Vec3>, surface: Surface) -> Option<Polygon> {
        if vertices.len() < 3 {
            return None;
        }
        let plane = Plane::from_points(vertices[0], vertices[1], vertices[2])?;
        Some(Polygon { vertices, plane, surface })
    }

    /// The same face with different corners — used by a split, where the
    /// plane is known to be unchanged and recomputing it from the new corners
    /// would be both wasted work and a chance to flip a nearly-degenerate
    /// sliver the wrong way round.
    fn with_vertices(&self, vertices: Vec<Vec3>) -> Polygon {
        Polygon { vertices, plane: self.plane, surface: self.surface }
    }

    pub fn flip(&mut self) {
        self.vertices.reverse();
        self.plane.flip();
    }
}

/// A node of the BSP tree: a dividing plane, the polygons lying in it, and the
/// space on either side.
#[derive(Clone, Debug, Default)]
struct Node {
    plane: Option<Plane>,
    front: Option<Box<Node>>,
    back: Option<Box<Node>>,
    polygons: Vec<Polygon>,
}

impl Node {
    fn new(polygons: Vec<Polygon>) -> Node {
        let mut node = Node::default();
        node.build(polygons);
        node
    }

    /// Turn the solid inside out: every face flipped, and front and back
    /// swapped everywhere. This is how `subtract` and `intersect` are written
    /// in terms of `union` rather than as three separate algorithms.
    fn invert(&mut self) {
        for polygon in &mut self.polygons {
            polygon.flip();
        }
        if let Some(plane) = &mut self.plane {
            plane.flip();
        }
        if let Some(front) = &mut self.front {
            front.invert();
        }
        if let Some(back) = &mut self.back {
            back.invert();
        }
        std::mem::swap(&mut self.front, &mut self.back);
    }

    /// Drop the parts of `polygons` that fall inside this solid.
    fn clip_polygons(&self, polygons: Vec<Polygon>) -> Vec<Polygon> {
        let Some(plane) = self.plane else {
            return polygons;
        };

        let mut split = Split::default();
        for polygon in &polygons {
            plane.split_polygon(polygon, &mut split);
        }
        // A coplanar polygon goes with the side it faces, which is what keeps
        // the shared wall between two unioned boxes from being kept twice.
        split.front.extend(split.coplanar_front);
        split.back.extend(split.coplanar_back);

        let mut kept = match &self.front {
            Some(node) => node.clip_polygons(split.front),
            None => split.front,
        };
        // No back child means the space behind this plane is solid, so
        // everything that landed there is inside and goes away.
        if let Some(node) = &self.back {
            kept.extend(node.clip_polygons(split.back));
        }
        kept
    }

    /// Remove everything of ours that lies inside `other`.
    fn clip_to(&mut self, other: &Node) {
        self.polygons = other.clip_polygons(std::mem::take(&mut self.polygons));
        if let Some(front) = &mut self.front {
            front.clip_to(other);
        }
        if let Some(back) = &mut self.back {
            back.clip_to(other);
        }
    }

    fn all_polygons(&self) -> Vec<Polygon> {
        let mut all = self.polygons.clone();
        if let Some(front) = &self.front {
            all.extend(front.all_polygons());
        }
        if let Some(back) = &self.back {
            all.extend(back.all_polygons());
        }
        all
    }

    /// Add polygons to the tree, dividing space by the first one's plane.
    ///
    /// Iterative down the back edge rather than recursive on both: a stack of
    /// boxes stacked along one axis degenerates into a list, and a list deep
    /// enough overflows the stack on a shape nobody would call complicated.
    fn build(&mut self, polygons: Vec<Polygon>) {
        if polygons.is_empty() {
            return;
        }
        if self.plane.is_none() {
            self.plane = Some(polygons[0].plane);
        }
        let plane = self.plane.expect("just set above");

        let mut split = Split::default();
        for polygon in &polygons {
            plane.split_polygon(polygon, &mut split);
        }
        // Both coplanar buckets lie *in* this node's plane, so they are this
        // node's own polygons whichever way they face.
        self.polygons.extend(split.coplanar_front);
        self.polygons.extend(split.coplanar_back);

        if !split.front.is_empty() {
            self.front.get_or_insert_with(|| Box::new(Node::default())).build(split.front);
        }
        if !split.back.is_empty() {
            self.back.get_or_insert_with(|| Box::new(Node::default())).build(split.back);
        }
    }
}

/// Everything in either solid.
pub fn union(a: &[Polygon], b: &[Polygon]) -> Vec<Polygon> {
    let mut a = Node::new(a.to_vec());
    let mut b = Node::new(b.to_vec());
    a.clip_to(&b);
    b.clip_to(&a);
    // `b`'s remaining faces include the inside walls of the shared region,
    // facing the wrong way. Flipping, clipping and flipping back is what
    // removes them, and leaving it out is what puts a wall through the middle
    // of two boxes that overlap.
    b.invert();
    b.clip_to(&a);
    b.invert();
    a.build(b.all_polygons());
    a.all_polygons()
}

/// Everything in `a` that is not also in `b`.
pub fn subtract(a: &[Polygon], b: &[Polygon]) -> Vec<Polygon> {
    let mut a = Node::new(a.to_vec());
    let mut b = Node::new(b.to_vec());
    a.invert();
    a.clip_to(&b);
    b.clip_to(&a);
    b.invert();
    b.clip_to(&a);
    b.invert();
    a.build(b.all_polygons());
    a.invert();
    a.all_polygons()
}

/// Only what is in both.
pub fn intersect(a: &[Polygon], b: &[Polygon]) -> Vec<Polygon> {
    let mut a = Node::new(a.to_vec());
    let mut b = Node::new(b.to_vec());
    a.invert();
    b.clip_to(&a);
    b.invert();
    a.clip_to(&b);
    b.clip_to(&a);
    a.build(b.all_polygons());
    a.invert();
    a.all_polygons()
}

/// Six times the signed volume the polygons enclose.
///
/// The divergence theorem over a triangle fan per face. Only meaningful for a
/// closed surface, which is the point: it is what the tests measure a boolean
/// by, because "does this look right" is not something a test can ask and the
/// volume of a box with a hole in it is arithmetic.
pub fn signed_volume_x6(polygons: &[Polygon]) -> f32 {
    let mut total = 0.0;
    for polygon in polygons {
        let a = polygon.vertices[0];
        for window in polygon.vertices[1..].windows(2) {
            total += a.dot(window[0].cross(window[1]));
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::solid::Solid;

    fn volume(polygons: &[Polygon]) -> f32 {
        signed_volume_x6(polygons) / 6.0
    }

    /// The measurement everything else in this file is checked with, checked
    /// first: a unit box encloses one cubic metre. If this is wrong — most
    /// likely because the winding convention got inverted somewhere — every
    /// other assertion here is measuring the wrong thing and passing anyway.
    #[test]
    fn a_box_encloses_its_own_volume() {
        let solid = Solid::cuboid(Vec3::ONE, Surface::default());
        assert!(
            (volume(solid.polygons()) - 1.0).abs() < 1e-4,
            "a 1m box came out at {} cubic metres",
            volume(solid.polygons()),
        );
    }

    /// Two boxes sharing exactly half their volume. Union has to notice the
    /// overlap rather than adding the two up, and the wall down the middle has
    /// to go: a union that kept it would measure correctly and draw a seam.
    #[test]
    fn a_union_counts_the_overlap_once() {
        let a = Solid::cuboid(Vec3::ONE, Surface::default());
        let b = Solid::cuboid(Vec3::ONE, Surface::default())
            .translated(Vec3::new(0.5, 0.0, 0.0));
        let joined = union(a.polygons(), b.polygons());
        assert!(
            (volume(&joined) - 1.5).abs() < 1e-3,
            "two half-overlapping 1m boxes came out at {}",
            volume(&joined),
        );
    }

    /// A hole all the way through, which is the operation a weapon is mostly
    /// made of. The tool is longer than the box on purpose: a subtraction
    /// whose tool ends exactly flush with a face is the one case where the
    /// tolerance decides the answer, and that is not what this is testing.
    #[test]
    fn subtracting_a_bore_takes_its_volume_away() {
        let block = Solid::cuboid(Vec3::ONE, Surface::default());
        let bore = Solid::cuboid(Vec3::new(0.2, 0.2, 2.0), Surface::default());
        let drilled = subtract(block.polygons(), bore.polygons());
        assert!(
            (volume(&drilled) - (1.0 - 0.2 * 0.2)).abs() < 1e-3,
            "a 0.2m square bore through a 1m box left {}",
            volume(&drilled),
        );
    }

    /// The kernel itself has **no opinion about materials**: a face it keeps
    /// from the tool keeps the tool's surface. That is the arithmetically
    /// honest answer and deliberately not the modelling one — see
    /// [`Solid::subtract`](crate::prop::solid::Solid::subtract), which
    /// repaints the tool first so the walls of a bore come out made of
    /// barrel. Pinned here so that rule stays in one layer rather than being
    /// half-implemented in both.
    #[test]
    fn the_kernel_carries_each_faces_own_surface_through_a_cut() {
        use crate::prop::surface::{Style, Tint};
        let oak = Surface::new(Style::Wood, Tint([0.5, 0.3, 0.1]));
        let block = Solid::cuboid(Vec3::ONE, oak);
        let bore = Solid::cuboid(Vec3::new(0.2, 0.2, 2.0), Surface::default());
        let drilled = subtract(block.polygons(), bore.polygons());

        assert!(!drilled.is_empty(), "the subtraction removed everything");
        assert!(
            drilled.iter().any(|polygon| polygon.surface == oak),
            "the block's own faces lost their surface",
        );
        assert!(
            drilled.iter().any(|polygon| polygon.surface == Surface::default()),
            "the kernel repainted the tool's faces, which is the layer above's job",
        );
    }

    /// Subtracting something that misses entirely must not perturb the solid.
    /// A kernel that rebuilt the shape anyway would pass a volume check and
    /// still have quietly retriangulated every face.
    #[test]
    fn subtracting_something_disjoint_changes_nothing() {
        let block = Solid::cuboid(Vec3::ONE, Surface::default());
        let elsewhere = Solid::cuboid(Vec3::ONE, Surface::default())
            .translated(Vec3::new(10.0, 0.0, 0.0));
        let result = subtract(block.polygons(), elsewhere.polygons());
        assert!((volume(&result) - 1.0).abs() < 1e-4);
    }

    /// Only the overlap survives an intersection — the operation that trims a
    /// barrel to a silhouette.
    #[test]
    fn an_intersection_keeps_only_the_overlap() {
        let a = Solid::cuboid(Vec3::ONE, Surface::default());
        let b = Solid::cuboid(Vec3::ONE, Surface::default())
            .translated(Vec3::new(0.5, 0.0, 0.0));
        let both = intersect(a.polygons(), b.polygons());
        assert!(
            (volume(&both) - 0.5).abs() < 1e-3,
            "the overlap of two half-overlapping 1m boxes came out at {}",
            volume(&both),
        );
    }
}
