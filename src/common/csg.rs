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
        match self.side_of(polygon) {
            Side::CoplanarFront => into.coplanar_front.push(polygon.clone()),
            Side::CoplanarBack => into.coplanar_back.push(polygon.clone()),
            Side::Front => into.front.push(polygon.clone()),
            Side::Back => into.back.push(polygon.clone()),
            Side::Spanning(front, back) => {
                into.front.extend(front);
                into.back.extend(back);
            }
        }
    }

    fn side_of<S: Copy>(&self, polygon: &Polygon<S>) -> Side<S> {
        let mut polygon_type = COPLANAR;
        // Most polygons have few corners; a fixed buffer keeps this off the heap.
        let mut types = [0u8; 16];
        let small = polygon.vertices.len() <= types.len();
        let mut spilled = Vec::new();
        for (i, vertex) in polygon.vertices.iter().enumerate() {
            let classification = self.classify(*vertex);
            polygon_type |= classification;
            match small {
                true => types[i] = classification,
                false => spilled.push(classification),
            }
        }
        let types: &[u8] = if small { &types[..polygon.vertices.len()] } else { &spilled };

        match polygon_type {
            COPLANAR if self.normal.dot(polygon.plane.normal) > 0.0 => Side::CoplanarFront,
            COPLANAR => Side::CoplanarBack,
            FRONT => Side::Front,
            BACK => Side::Back,
            _ => {
                let count = polygon.vertices.len();
                let mut front_vertices = Vec::with_capacity(count + 1);
                let mut back_vertices = Vec::with_capacity(count + 1);
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
                let front = (front_vertices.len() >= 3).then(|| polygon.with_vertices(front_vertices));
                let back = (back_vertices.len() >= 3).then(|| polygon.with_vertices(back_vertices));
                Side::Spanning(front, back)
            }
        }
    }
}

enum Side<S> {
    CoplanarFront,
    CoplanarBack,
    Front,
    Back,
    Spanning(Option<Polygon<S>>, Option<Polygon<S>>),
}

/// What clipping left of one polygon. `Whole` keeps the polygon as it came
/// rather than as the pieces the tree's planes cut it into, so a face that
/// nothing removes stays one face however many planes cross it.
enum Clipped<S> {
    Whole,
    Gone,
    Pieces(Vec<Polygon<S>>),
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
        let mut kept = Vec::with_capacity(polygons.len());
        for polygon in polygons {
            match self.clip_one(&polygon) {
                Clipped::Whole => kept.push(polygon),
                Clipped::Gone => {}
                Clipped::Pieces(pieces) => kept.extend(pieces),
            }
        }
        kept
    }

    fn clip_one(&self, polygon: &Polygon<S>) -> Clipped<S> {
        let Some(plane) = self.plane else {
            return Clipped::Whole;
        };
        // A coplanar polygon goes with the side its normal faces, which keeps
        // the shared wall between two unioned boxes from being kept twice. No
        // back child means the space behind is solid, so what lands there is
        // inside and goes away.
        let front = |piece: &Polygon<S>| match &self.front {
            Some(node) => node.clip_one(piece),
            None => Clipped::Whole,
        };
        let back = |piece: &Polygon<S>| match &self.back {
            Some(node) => node.clip_one(piece),
            None => Clipped::Gone,
        };
        match plane.side_of(polygon) {
            Side::Front | Side::CoplanarFront => front(polygon),
            Side::Back | Side::CoplanarBack => back(polygon),
            Side::Spanning(front_piece, back_piece) => {
                let in_front = front_piece.as_ref().map(front);
                let behind = back_piece.as_ref().map(back);
                if let (Some(Clipped::Whole), Some(Clipped::Whole)) = (&in_front, &behind) {
                    return Clipped::Whole;
                }
                let mut pieces = Vec::new();
                for (piece, clipped) in [(front_piece, in_front), (back_piece, behind)] {
                    match (piece, clipped) {
                        (Some(piece), Some(Clipped::Whole)) => pieces.push(piece),
                        (_, Some(Clipped::Pieces(more))) => pieces.extend(more),
                        _ => {}
                    }
                }
                match pieces.is_empty() {
                    true => Clipped::Gone,
                    false => Clipped::Pieces(pieces),
                }
            }
        }
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

/// Corners of the box round `polygons`, widened by the tolerance so a face
/// lying on another solid's box still counts as touching it.
fn bounds<S>(polygons: &[Polygon<S>]) -> Option<(Vec3, Vec3)> {
    let mut points = polygons.iter().flat_map(|polygon| &polygon.vertices);
    let first = *points.next()?;
    let (min, max) = points.fold((first, first), |(min, max), p| (min.min(*p), max.max(*p)));
    Some((min - Vec3::splat(EPSILON * 10.0), max + Vec3::splat(EPSILON * 10.0)))
}

fn overlaps((a_min, a_max): (Vec3, Vec3), (b_min, b_max): (Vec3, Vec3)) -> bool {
    a_min.cmple(b_max).all() && b_min.cmple(a_max).all()
}

/// `polygons` clipped by `tree`, the solid inside `around`. A polygon whose
/// box misses `around` is wholly outside the solid, so it is kept, or for an
/// inverted tree, whose inside is everything outside the solid, dropped,
/// without walking the tree.
fn clip<S: Copy>(tree: &Node<S>, around: (Vec3, Vec3), inverted: bool, polygons: Vec<Polygon<S>>) -> Vec<Polygon<S>> {
    let mut kept = Vec::with_capacity(polygons.len());
    for polygon in polygons {
        let near = bounds(std::slice::from_ref(&polygon)).is_some_and(|box_| overlaps(box_, around));
        if !near {
            if !inverted {
                kept.push(polygon);
            }
            continue;
        }
        match tree.clip_one(&polygon) {
            Clipped::Whole => kept.push(polygon),
            Clipped::Gone => {}
            Clipped::Pieces(pieces) => kept.extend(pieces),
        }
    }
    kept
}

fn flipped<S: Copy>(mut polygons: Vec<Polygon<S>>) -> Vec<Polygon<S>> {
    polygons.iter_mut().for_each(Polygon::flip);
    polygons
}

fn inverted<S: Copy>(mut tree: Node<S>) -> Node<S> {
    tree.invert();
    tree
}

// The booleans are csg.js's, with the polygons kept apart from the trees:
// a tree's planes classify, and what is returned is the operands' own
// polygons clipped, never the pieces building a tree cut them into. Clipping
// reads only a tree's planes, so the order of steps is csg.js's exactly.

pub fn union<S: Copy>(a: &[Polygon<S>], b: &[Polygon<S>]) -> Vec<Polygon<S>> {
    let (Some(a_box), Some(b_box)) = (bounds(a), bounds(b)) else {
        return a.iter().chain(b).cloned().collect();
    };
    if !overlaps(a_box, b_box) {
        return a.iter().chain(b).cloned().collect();
    }
    let a_tree = Node::new(a.to_vec());
    let b_tree = Node::new(b.to_vec());
    let mut kept = clip(&b_tree, b_box, false, a.to_vec());
    let b_left = clip(&a_tree, a_box, false, b.to_vec());
    // Removes `b`'s coplanar faces inside the shared region; otherwise two
    // overlapping boxes keep a wall through the middle.
    kept.extend(flipped(clip(&a_tree, a_box, false, flipped(b_left))));
    kept
}

/// Everything in `a` that is not also in `b`.
pub fn subtract<S: Copy>(a: &[Polygon<S>], b: &[Polygon<S>]) -> Vec<Polygon<S>> {
    let (Some(a_box), Some(b_box)) = (bounds(a), bounds(b)) else { return a.to_vec() };
    if !overlaps(a_box, b_box) {
        return a.to_vec();
    }
    let a_tree = inverted(Node::new(a.to_vec()));
    let b_tree = Node::new(b.to_vec());
    let a_left = clip(&b_tree, b_box, false, flipped(a.to_vec()));
    let b_left = clip(&a_tree, a_box, true, b.to_vec());
    let b_left = flipped(clip(&a_tree, a_box, true, flipped(b_left)));
    let mut kept = flipped(a_left);
    kept.extend(flipped(b_left));
    kept
}

pub fn intersect<S: Copy>(a: &[Polygon<S>], b: &[Polygon<S>]) -> Vec<Polygon<S>> {
    let (Some(a_box), Some(b_box)) = (bounds(a), bounds(b)) else { return Vec::new() };
    if !overlaps(a_box, b_box) {
        return Vec::new();
    }
    let a_tree = inverted(Node::new(a.to_vec()));
    let b_tree = inverted(Node::new(b.to_vec()));
    let b_left = flipped(clip(&a_tree, a_box, true, b.to_vec()));
    let a_left = clip(&b_tree, b_box, true, flipped(a.to_vec()));
    let b_left = clip(&a_tree, a_box, true, b_left);
    let mut kept = flipped(a_left);
    kept.extend(flipped(b_left));
    kept
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
