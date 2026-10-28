//! Cutting solids out of one another, with a BSP tree: the `csg.js` algorithm,
//! hand-written because a C library does not compile to wasm.
//!
//! A solid is a set of convex polygons forming a closed surface. Nothing checks
//! either property and both matter:
//!
//! - Convex: [`Plane::split_polygon`] assumes a plane cuts a polygon into
//!   exactly two pieces. A concave one can need three, and the solid grows a
//!   hole.
//! - Closed: "inside" means nothing for a surface with gaps.
//!
//! Normals are per face. A vertex created by a split has no natural normal,
//! and interpolating one makes boolean seams visible.

//!
//! Each polygon carries a `S` it is made of, copied onto every piece a cut
//! leaves: a prop's [`Surface`](crate::prop::surface::Surface), nothing for a
//! map's walls.

use bevy::prelude::*;

/// How close to a plane counts as *on* it. Generous, because a tolerance tight
/// enough for a millimeter leaves a sliver of polygon at every cut.
const EPSILON: f32 = 1e-5;

/// `w` is the distance from the origin along `normal`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub w: f32,
}

/// Which side of a plane something is on. Bits, so the union of a polygon's
/// vertices is the polygon's own classification: `FRONT | BACK == SPANNING`.
const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;
const SPANNING: u8 = 3;

/// A struct rather than four `&mut Vec` arguments, because callers merge two
/// buckets into one.
struct Split<S> {
    coplanar_front: Vec<Polygon<S>>,
    coplanar_back: Vec<Polygon<S>>,
    front: Vec<Polygon<S>>,
    back: Vec<Polygon<S>>,
}

impl<S> Default for Split<S> {
    fn default() -> Self {
        Split { coplanar_front: Vec::new(), coplanar_back: Vec::new(), front: Vec::new(), back: Vec::new() }
    }
}

impl Plane {
    /// `None` if the points are in a line: a zero normal classifies everything
    /// as coplanar and the tree stops dividing space.
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

    /// A coplanar polygon goes to the side its normal faces.
    fn split_polygon<S: Copy>(&self, polygon: &Polygon<S>, into: &mut Split<S>) {
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

                // A split that shaved a corner off exactly on the plane can
                // leave two vertices, which is not a polygon.
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

/// One convex face, wound counter-clockwise seen from outside, as in
/// [`crate::common::mesh`].
#[derive(Clone, Debug)]
pub struct Polygon<S> {
    pub vertices: Vec<Vec3>,
    pub plane: Plane,
    pub surface: S,
}

impl<S: Copy> Polygon<S> {
    pub fn new(vertices: Vec<Vec3>, surface: S) -> Option<Polygon<S>> {
        if vertices.len() < 3 {
            return None;
        }
        let plane = Plane::from_points(vertices[0], vertices[1], vertices[2])?;
        Some(Polygon { vertices, plane, surface })
    }

    /// Keeps the plane: recomputing it could flip a nearly-degenerate sliver.
    fn with_vertices(&self, vertices: Vec<Vec3>) -> Polygon<S> {
        Polygon { vertices, plane: self.plane, surface: self.surface }
    }

    pub fn flip(&mut self) {
        self.vertices.reverse();
        self.plane.flip();
    }

    pub fn translated(mut self, offset: Vec3) -> Polygon<S> {
        self.vertices.iter_mut().for_each(|v| *v += offset);
        self.plane.w += self.plane.normal.dot(offset);
        self
    }
}

#[derive(Clone, Debug)]
struct Node<S> {
    plane: Option<Plane>,
    front: Option<Box<Node<S>>>,
    back: Option<Box<Node<S>>>,
    polygons: Vec<Polygon<S>>,
}

impl<S> Default for Node<S> {
    fn default() -> Self {
        Node { plane: None, front: None, back: None, polygons: Vec::new() }
    }
}

impl<S: Copy> Node<S> {
    fn new(polygons: Vec<Polygon<S>>) -> Node<S> {
        let mut node = Node::default();
        node.build(polygons);
        node
    }

    /// Turn the solid inside out, so `subtract` and `intersect` can reuse the
    /// steps of `union`.
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
    fn clip_polygons(&self, polygons: Vec<Polygon<S>>) -> Vec<Polygon<S>> {
        let Some(plane) = self.plane else {
            return polygons;
        };

        let mut split = Split::default();
        for polygon in &polygons {
            plane.split_polygon(polygon, &mut split);
        }
        // Keeps the shared wall between two unioned boxes from being kept twice.
        split.front.extend(split.coplanar_front);
        split.back.extend(split.coplanar_back);

        let mut kept = match &self.front {
            Some(node) => node.clip_polygons(split.front),
            None => split.front,
        };
        // No back child means the space behind is solid, so what landed there
        // is inside and goes away.
        if let Some(node) = &self.back {
            kept.extend(node.clip_polygons(split.back));
        }
        kept
    }

    /// Remove everything of ours that lies inside `other`.
    fn clip_to(&mut self, other: &Node<S>) {
        self.polygons = other.clip_polygons(std::mem::take(&mut self.polygons));
        if let Some(front) = &mut self.front {
            front.clip_to(other);
        }
        if let Some(back) = &mut self.back {
            back.clip_to(other);
        }
    }

    fn all_polygons(&self) -> Vec<Polygon<S>> {
        let mut all = self.polygons.clone();
        if let Some(front) = &self.front {
            all.extend(front.all_polygons());
        }
        if let Some(back) = &self.back {
            all.extend(back.all_polygons());
        }
        all
    }

    /// Divides space by the first polygon's plane.
    fn build(&mut self, mut polygons: Vec<Polygon<S>>) {
        if polygons.is_empty() {
            return;
        }
        if self.plane.is_none() {
            // Kept outright rather than classified: far enough from the
            // origin, a polygon's own corners are off the plane made from
            // them by more than the tolerance, and one split by its own plane
            // hands each piece the same plane to be split by again, forever.
            let first = polygons.remove(0);
            self.plane = Some(first.plane);
            self.polygons.push(first);
        }
        let plane = self.plane.expect("just set above");

        let mut split = Split::default();
        for polygon in &polygons {
            plane.split_polygon(polygon, &mut split);
        }
        // Both coplanar buckets lie in this node's plane whichever way they
        // face, so they are this node's own polygons.
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

pub fn union<S: Copy>(a: &[Polygon<S>], b: &[Polygon<S>]) -> Vec<Polygon<S>> {
    let mut a = Node::new(a.to_vec());
    let mut b = Node::new(b.to_vec());
    a.clip_to(&b);
    b.clip_to(&a);
    // Removes `b`'s coplanar faces inside the shared region; otherwise two
    // overlapping boxes keep a wall through the middle.
    b.invert();
    b.clip_to(&a);
    b.invert();
    a.build(b.all_polygons());
    a.all_polygons()
}

/// Everything in `a` that is not also in `b`.
pub fn subtract<S: Copy>(a: &[Polygon<S>], b: &[Polygon<S>]) -> Vec<Polygon<S>> {
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

pub fn intersect<S: Copy>(a: &[Polygon<S>], b: &[Polygon<S>]) -> Vec<Polygon<S>> {
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

/// The parts of `polygons` outside the solid `solid`. On its surface, a
/// polygon facing against the solid's face there is inside (two rooms' shared
/// wall), and one facing the same way is outside unless `solid_owns_shared`,
/// so of two coincident faces exactly one survives when each is clipped by
/// the other with the opposite answer.
pub fn clip_outside<S: Copy>(polygons: Vec<Polygon<S>>, solid: &[Polygon<S>], solid_owns_shared: bool) -> Vec<Polygon<S>> {
    let solid = Node::new(solid.to_vec());
    let mut kept = solid.clip_polygons(polygons);
    if solid_owns_shared {
        // Turned round, a coincident face is one facing against the solid,
        // which the clip removes; nothing else changes side.
        kept.iter_mut().for_each(Polygon::flip);
        kept = solid.clip_polygons(kept);
        kept.iter_mut().for_each(Polygon::flip);
    }
    kept
}

/// `polygons` split into the parts inside `solid` and the parts outside. On
/// its surface, a polygon facing against the solid's face there is inside and
/// one facing the same way is outside — so the outside half is exactly
/// [`clip_outside`] with `solid_owns_shared` false.
pub fn partition<S: Copy>(polygons: Vec<Polygon<S>>, solid: &[Polygon<S>]) -> (Vec<Polygon<S>>, Vec<Polygon<S>>) {
    let mut tree = Node::new(solid.to_vec());
    let outside = tree.clip_polygons(polygons.clone());
    tree.invert();
    let inside = tree.clip_polygons(polygons);
    (inside, outside)
}

/// An axis-aligned box's six faces, facing out.
pub fn cuboid<S: Copy>(min: Vec3, max: Vec3, surface: S) -> Vec<Polygon<S>> {
    let corner = |x: bool, y: bool, z: bool| {
        Vec3::new(if x { max.x } else { min.x }, if y { max.y } else { min.y }, if z { max.z } else { min.z })
    };
    let (f, t) = (false, true);
    [
        [corner(f, f, f), corner(f, f, t), corner(f, t, t), corner(f, t, f)],
        [corner(t, f, f), corner(t, t, f), corner(t, t, t), corner(t, f, t)],
        [corner(f, f, f), corner(t, f, f), corner(t, f, t), corner(f, f, t)],
        [corner(f, t, f), corner(f, t, t), corner(t, t, t), corner(t, t, f)],
        [corner(f, f, f), corner(f, t, f), corner(t, t, f), corner(t, f, f)],
        [corner(f, f, t), corner(t, f, t), corner(t, t, t), corner(f, t, t)],
    ]
    .into_iter()
    .filter_map(|face| Polygon::new(face.to_vec(), surface))
    .collect()
}

/// Six times the signed volume enclosed, by the divergence theorem. Only
/// meaningful for a closed surface.
pub fn signed_volume_x6<S>(polygons: &[Polygon<S>]) -> f32 {
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

    fn volume<S>(polygons: &[Polygon<S>]) -> f32 {
        signed_volume_x6(polygons) / 6.0
    }

    /// Centered on `at`.
    fn block(size: Vec3, at: Vec3) -> Vec<Polygon<u8>> {
        cuboid(at - size / 2.0, at + size / 2.0, 0)
    }

    /// If the winding convention inverts, every other test here measures the
    /// wrong thing.
    #[test]
    fn a_box_encloses_its_own_volume() {
        let solid = block(Vec3::ONE, Vec3::ZERO);
        assert!(
            (volume(&solid) - 1.0).abs() < 1e-4,
            "a 1m box came out at {} cubic meters",
            volume(&solid),
        );
    }

    /// Fails if union adds the two volumes instead of counting the overlap once.
    #[test]
    fn a_union_counts_the_overlap_once() {
        let a = block(Vec3::ONE, Vec3::ZERO);
        let b = block(Vec3::ONE, Vec3::new(0.5, 0.0, 0.0));
        let joined = union(&a, &b);
        assert!(
            (volume(&joined) - 1.5).abs() < 1e-3,
            "two half-overlapping 1m boxes came out at {}",
            volume(&joined),
        );
    }

    /// The tool is longer than the box: a tool ending flush with a face is
    /// decided by the tolerance.
    #[test]
    fn subtracting_a_bore_takes_its_volume_away() {
        let solid = block(Vec3::ONE, Vec3::ZERO);
        let bore = block(Vec3::new(0.2, 0.2, 2.0), Vec3::ZERO);
        let drilled = subtract(&solid, &bore);
        assert!(
            (volume(&drilled) - (1.0 - 0.2 * 0.2)).abs() < 1e-3,
            "a 0.2m square bore through a 1m box left {}",
            volume(&drilled),
        );
    }

    /// Repainting the tool belongs in
    /// [`Solid::subtract`](crate::prop::solid::Solid::subtract), not the kernel.
    #[test]
    fn the_kernel_carries_each_faces_own_surface_through_a_cut() {
        let (oak, drill) = (7u8, 0u8);
        let solid = cuboid(Vec3::splat(-0.5), Vec3::splat(0.5), oak);
        let bore = cuboid(Vec3::new(-0.1, -0.1, -1.0), Vec3::new(0.1, 0.1, 1.0), drill);
        let drilled = subtract(&solid, &bore);

        assert!(!drilled.is_empty(), "the subtraction removed everything");
        assert!(
            drilled.iter().any(|polygon| polygon.surface == oak),
            "the block's own faces lost their surface",
        );
        assert!(
            drilled.iter().any(|polygon| polygon.surface == drill),
            "the kernel repainted the tool's faces, which is the layer above's job",
        );
    }

    #[test]
    fn a_cuboids_faces_point_out() {
        let solid = block(Vec3::new(2.0, 3.0, 4.0), Vec3::new(1.0, 1.0, 1.0));
        assert!((volume(&solid) - 24.0).abs() < 1e-4);
        for polygon in &solid {
            let center = polygon.vertices.iter().sum::<Vec3>() / 4.0;
            assert!(polygon.plane.normal.dot(center - Vec3::ONE) > 0.0);
        }
    }

    /// Breaks without `build` keeping its first polygon outright: this far
    /// out a slanted face's corners are off its own plane by more than the
    /// tolerance, and the tree splits it by itself until the stack runs out.
    #[test]
    fn a_slanted_solid_far_from_the_origin_does_not_split_itself_forever() {
        let wedge = |at: Vec3| {
            crate::tool::room::Room::prism(vec![Vec2::new(0.0, 0.0), Vec2::new(10.3, 0.0), Vec2::new(0.0, 10.3)], 0.0, 3.14)
                .translated(at)
                .solid(0u8)
        };
        let far = Vec3::new(143.37, 1.13, 87.91);
        let joined = union(&wedge(far), &wedge(far + Vec3::new(2.0, 0.0, 1.0)));
        assert!(!joined.is_empty());
    }

    #[test]
    fn subtracting_something_disjoint_changes_nothing() {
        let solid = block(Vec3::ONE, Vec3::ZERO);
        let elsewhere = block(Vec3::ONE, Vec3::new(10.0, 0.0, 0.0));
        let result = subtract(&solid, &elsewhere);
        assert!((volume(&result) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn an_intersection_keeps_only_the_overlap() {
        let a = block(Vec3::ONE, Vec3::ZERO);
        let b = block(Vec3::ONE, Vec3::new(0.5, 0.0, 0.0));
        let both = intersect(&a, &b);
        assert!(
            (volume(&both) - 0.5).abs() < 1e-3,
            "the overlap of two half-overlapping 1m boxes came out at {}",
            volume(&both),
        );
    }
}
