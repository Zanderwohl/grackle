//! Naming a prop's corners and edges so something later in the list can refer
//! to them.
//!
//! Booleans re-split the kernel's polygons, so nothing in a [`Solid`] is stable
//! between evaluations. A reference names the generator instead ([`Outline`],
//! from the same sweep as the solid) and every mirror or pattern copy it went
//! through. A corner a boolean cut away still resolves; an edge a boolean made
//! has no name and cannot be picked.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::prop::feature::PropFeatureId;
use crate::prop::solid::Solid;

/// One duplication a piece of geometry went through. The original is never
/// named, so inserting a pattern does not rename what it copies; a mirror's
/// reflection is copy 1 whether or not the original is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CopyOf {
    pub by: PropFeatureId,
    pub index: u32,
}

/// The generator, and its copies in the order they were made.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Source {
    pub feature: PropFeatureId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copies: Vec<CopyOf>,
}

impl Source {
    pub fn features(&self) -> impl Iterator<Item = PropFeatureId> + '_ {
        std::iter::once(self.feature).chain(self.copies.iter().map(|copy| copy.by))
    }
}

/// An index into the generator's [`Outline`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Element {
    Point(u32),
    Edge(u32),
    Face(u32),
}

/// `element` first: TOML needs every scalar before any sub-table.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GeomRef {
    pub element: Element,
    pub source: Source,
}

/// Where a reference is, in the prop's own space.
#[derive(Clone, Debug, PartialEq)]
pub enum Located {
    Point(Vec3),
    Edge(Vec3, Vec3),
    /// Every facet's corners, and the area-weighted normal out of the body.
    /// A face may be curved — a revolved segment is one face — so
    /// [`Self::is_flat`] checks rather than trusts.
    Face { facets: Vec<Vec<Vec3>>, normal: Vec3 },
}

impl Located {
    pub fn points(&self) -> Vec<Vec3> {
        match self {
            Located::Point(p) => vec![*p],
            Located::Edge(a, b) => vec![*a, *b],
            Located::Face { facets, .. } => facets.concat(),
        }
    }
}

/// One flat piece of a face. `outward` is stored rather than derived from the
/// winding, because the winding is only fixed afterward by
/// [`Solid::oriented_outward`]; it is zero for a facet with no area.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineFacet {
    pub points: Vec<u32>,
    pub outward: Vec3,
}

/// A face as a person would point at it: every facet one segment of a
/// profile swept along one piece of path.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineFace {
    pub facets: Vec<OutlineFacet>,
}

/// A generator's corners, the edges between them, and its faces, numbered by
/// [`crate::prop::skin`] from the profile *as authored*, not as wound for the
/// kernel, so dragging a point across the loop does not renumber everything.
/// Degenerate entries — a revolve's corners on the axis, the zero-length edges
/// between them — are kept so the numbering holds; they are filtered when
/// picking.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    pub points: Vec<Vec3>,
    pub edges: Vec<[u32; 2]>,
    pub faces: Vec<OutlineFace>,
}

impl Outline {
    /// `other`'s elements after this one's, renumbered.
    pub fn append(&mut self, other: Outline) {
        let offset = self.points.len() as u32;
        self.points.extend(other.points);
        self.edges.extend(other.edges.into_iter().map(|[a, b]| [a + offset, b + offset]));
        self.faces.extend(other.faces.into_iter().map(|face| OutlineFace {
            facets: face
                .facets
                .into_iter()
                .map(|facet| OutlineFacet { points: facet.points.into_iter().map(|p| p + offset).collect(), outward: facet.outward })
                .collect(),
        }));
    }

    pub fn transformed(mut self, transform: Transform) -> Outline {
        for point in &mut self.points {
            *point = transform.transform_point(*point);
        }
        for facet in self.faces.iter_mut().flat_map(|face| &mut face.facets) {
            facet.outward = transform.rotation * facet.outward;
        }
        self
    }

    /// Must match [`Solid::mirrored`].
    pub fn mirrored(mut self, origin: Vec3, normal: Vec3) -> Outline {
        let normal = normal.normalize_or_zero();
        for point in &mut self.points {
            let offset = *point - origin;
            *point = origin + offset - 2.0 * offset.dot(normal) * normal;
        }
        // A reflection keeps out as out.
        for facet in self.faces.iter_mut().flat_map(|face| &mut face.facets) {
            facet.outward -= 2.0 * facet.outward.dot(normal) * normal;
        }
        self
    }

    pub fn locate(&self, element: Element) -> Option<Located> {
        match element {
            Element::Point(i) => self.points.get(i as usize).copied().map(Located::Point),
            Element::Edge(i) => {
                let [a, b] = *self.edges.get(i as usize)?;
                Some(Located::Edge(self.points[a as usize], self.points[b as usize]))
            }
            Element::Face(i) => {
                let face = self.faces.get(i as usize)?;
                let facets: Vec<Vec<Vec3>> = face
                    .facets
                    .iter()
                    .map(|facet| facet.points.iter().map(|&p| self.points[p as usize]).collect())
                    .collect();
                // Each facet's area-weighted normal, turned out of the body.
                let sum = face.facets.iter().zip(&facets).fold(Vec3::ZERO, |sum, (facet, points)| {
                    let normal = newell(points);
                    match normal.dot(facet.outward) < 0.0 {
                        true => sum - normal,
                        false => sum + normal,
                    }
                });
                let normal = sum
                    .try_normalize()
                    .unwrap_or_else(|| face.facets.iter().map(|facet| facet.outward).sum::<Vec3>().normalize_or_zero());
                Some(Located::Face { facets, normal })
            }
        }
    }
}

/// A loop's area-weighted normal: exact for a flat loop, the best fit for one
/// that is nearly flat, and zero for one with no area. Twice the area long.
pub fn newell(points: &[Vec3]) -> Vec3 {
    (0..points.len()).fold(Vec3::ZERO, |sum, i| {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        sum + Vec3::new(
            (a.y - b.y) * (a.z + b.z),
            (a.z - b.z) * (a.x + b.x),
            (a.x - b.x) * (a.y + b.y),
        )
    })
}

impl Located {
    /// To within [`ON_SURFACE`].
    pub fn is_flat(&self) -> bool {
        match self {
            Located::Face { facets, normal } => {
                // A curved face's normals can cancel to nothing, which is no
                // plane however close its points are to one.
                let Some(first) = facets.iter().flatten().next() else { return false };
                normal.length() > 0.5 && facets.iter().flatten().all(|p| (*p - *first).dot(*normal).abs() < ON_SURFACE)
            }
            Located::Point(_) | Located::Edge(..) => true,
        }
    }
}

/// In the prop's space.
#[derive(Clone, Debug, PartialEq)]
pub struct Wire {
    pub source: Source,
    pub outline: Outline,
}

impl Wire {
    pub fn new(feature: PropFeatureId, outline: Outline) -> Wire {
        Wire { source: Source { feature, copies: vec![] }, outline }
    }

    /// The same outline moved, named as copy `index` made by `by`.
    pub fn copied(&self, by: PropFeatureId, index: u32, outline: Outline) -> Wire {
        let mut source = self.source.clone();
        source.copies.push(CopyOf { by, index });
        Wire { source, outline }
    }
}

/// A named corner or edge that is on its body's surface, so can be seen and
/// clicked.
#[derive(Clone, Debug, PartialEq)]
pub struct Pickable {
    pub body: PropFeatureId,
    pub reference: GeomRef,
    pub at: Located,
}

/// Meters. Well above the kernel's rounding, well below anything modeled.
const ON_SURFACE: f32 = 1e-4;

/// An edge shorter than this has collapsed onto a point; meters.
const DEGENERATE_EDGE: f32 = 1e-5;

/// Every named element of `wires` on `solid`'s surface. A cut-away corner is
/// not offered: a dot floating beside a bore reads as a bug. An edge needs its
/// middle on the surface too, or a long tool's sides poke out of its hole.
pub fn pickables(body: PropFeatureId, solid: &Solid, wires: &[Wire]) -> Vec<Pickable> {
    let surface = SurfaceIndex::of(solid);
    let mut found = Vec::new();
    for wire in wires {
        let named = |element| GeomRef { element, source: wire.source.clone() };
        for (i, &point) in wire.outline.points.iter().enumerate() {
            if surface.contains(point) {
                found.push(Pickable { body, reference: named(Element::Point(i as u32)), at: Located::Point(point) });
            }
        }
        for (i, &[a, b]) in wire.outline.edges.iter().enumerate() {
            let (a, b) = (wire.outline.points[a as usize], wire.outline.points[b as usize]);
            if a.distance(b) < DEGENERATE_EDGE {
                continue;
            }
            if [a, (a + b) / 2.0, b].into_iter().all(|p| surface.contains(p)) {
                found.push(Pickable { body, reference: named(Element::Edge(i as u32)), at: Located::Edge(a, b) });
            }
        }
    }
    found
}

/// A solid's polygons filed by their boxes, so asking whether a point is on
/// the surface tests the polygons near it.
struct SurfaceIndex<'a> {
    solid: &'a Solid,
    grid: crate::prop::box_grid::BoxGrid,
}

impl<'a> SurfaceIndex<'a> {
    fn of(solid: &'a Solid) -> Self {
        let reach = Vec3::splat(ON_SURFACE);
        let boxes: Vec<(Vec3, Vec3)> = solid
            .polygons()
            .iter()
            .map(|polygon| {
                let (min, max) = polygon.vertices.iter().fold((Vec3::MAX, Vec3::MIN), |(min, max), p| (min.min(*p), max.max(*p)));
                (min - reach, max + reach)
            })
            .collect();
        SurfaceIndex { solid, grid: crate::prop::box_grid::BoxGrid::new(&boxes) }
    }

    fn contains(&self, point: Vec3) -> bool {
        let polygons = self.solid.polygons();
        self.grid.near_point(point).any(|i| on_polygon(&polygons[i], point))
    }
}

#[cfg(test)]
fn on_surface(solid: &Solid, point: Vec3) -> bool {
    solid.polygons().iter().any(|polygon| on_polygon(polygon, point))
}

fn on_polygon(polygon: &crate::prop::solid::Polygon, point: Vec3) -> bool {
    let normal = polygon.plane.normal;
    if (normal.dot(point) - polygon.plane.w).abs() > ON_SURFACE {
        return false;
    }
    // Inside a convex polygon is the same side of every edge. Either
    // side, so the test does not lean on the winding.
    let v = &polygon.vertices;
    let sides = (0..v.len()).map(|i| {
        let (a, b) = (v[i], v[(i + 1) % v.len()]);
        (b - a).cross(point - a).dot(normal) / (b - a).length().max(f32::EPSILON)
    });
    let (mut ahead, mut behind) = (true, true);
    for side in sides {
        ahead &= side >= -ON_SURFACE;
        behind &= side <= ON_SURFACE;
    }
    ahead || behind
}

/// Pixels. Generous, because a corner has no area to hit.
pub const POINT_PICK_PX: f32 = 10.0;
/// Pixels. Tighter than a point's, so a click near a corner takes the corner.
pub const EDGE_PICK_PX: f32 = 6.0;

/// Distances on screen from the pointer, for picking things too thin to hit
/// with a ray. Engine-free so picking can be tested without a camera.
pub struct Pointer<P> {
    pub ray: Ray3d,
    /// How far down the ray the nearest face is, so nothing round the back of
    /// a body is picked through it.
    pub face: Option<f32>,
    /// World to viewport pixels.
    pub project: P,
}

impl<P: Fn(Vec3) -> Option<Vec2>> Pointer<P> {
    fn depth(&self, p: Vec3) -> f32 {
        (p - self.ray.origin).dot(*self.ray.direction)
    }

    /// Pixels from the pointer to `p`, if it is in front and not hidden.
    /// Measured at `p`'s own depth: any point down the ray lands on the
    /// pointer, perspective or orthographic.
    pub fn to_point(&self, p: Vec3) -> Option<f32> {
        let depth = self.depth(p);
        // Something on the face that was hit is at the same depth, give or
        // take the kernel's rounding and the slant of the face across a few
        // pixels.
        let hidden = self.face.is_some_and(|face| depth > face + 0.002 + face * 0.01);
        if depth <= 0.0 || hidden {
            return None;
        }
        let on_screen = (self.project)(p)?;
        let pointer = (self.project)(self.ray.origin + *self.ray.direction * depth)?;
        Some(on_screen.distance(pointer))
    }

    pub fn to_segment(&self, a: Vec3, b: Vec3) -> Option<f32> {
        self.to_point(closest_on_segment(self.ray.origin, *self.ray.direction, a, b))
    }
}

pub fn element_under<'a, P: Fn(Vec3) -> Option<Vec2>>(
    pickables: &'a [Pickable],
    pointer: &Pointer<P>,
) -> Option<&'a Pickable> {
    corner_under(pickables, pointer).or_else(|| edge_under(pickables, pointer))
}

pub fn corner_under<'a, P: Fn(Vec3) -> Option<Vec2>>(
    pickables: &'a [Pickable],
    pointer: &Pointer<P>,
) -> Option<&'a Pickable> {
    nearest(pickables, POINT_PICK_PX, |at| match at {
        Located::Point(p) => pointer.to_point(*p),
        Located::Edge(..) | Located::Face { .. } => None,
    })
}

pub fn edge_under<'a, P: Fn(Vec3) -> Option<Vec2>>(
    pickables: &'a [Pickable],
    pointer: &Pointer<P>,
) -> Option<&'a Pickable> {
    nearest(pickables, EDGE_PICK_PX, |at| match at {
        Located::Edge(a, b) => pointer.to_segment(*a, *b),
        Located::Point(_) | Located::Face { .. } => None,
    })
}

fn nearest(pickables: &[Pickable], within: f32, distance: impl Fn(&Located) -> Option<f32>) -> Option<&Pickable> {
    pickables
        .iter()
        .filter_map(|pickable| Some((pickable, distance(&pickable.at)?)))
        .filter(|(_, pixels)| *pixels <= within)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(pickable, _)| pickable)
}

pub fn segment_under<T: Clone, P: Fn(Vec3) -> Option<Vec2>>(
    segments: impl IntoIterator<Item = (T, Vec3, Vec3)>,
    pointer: &Pointer<P>,
) -> Option<T> {
    segments
        .into_iter()
        .filter_map(|(what, a, b)| Some((what, pointer.to_segment(a, b)?)))
        .filter(|(_, pixels)| *pixels <= EDGE_PICK_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(what, _)| what)
}

pub fn point_under<T: Clone, P: Fn(Vec3) -> Option<Vec2>>(
    points: impl IntoIterator<Item = (T, Vec3)>,
    pointer: &Pointer<P>,
) -> Option<T> {
    points
        .into_iter()
        .filter_map(|(what, p)| Some((what, pointer.to_point(p)?)))
        .filter(|(_, pixels)| *pixels <= POINT_PICK_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(what, _)| what)
}

/// The point of segment `a`–`b` nearest the line through `origin` along
/// `direction`.
fn closest_on_segment(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let along = b - a;
    let w = a - origin;
    let (dd, da, aa) = (direction.dot(direction), direction.dot(along), along.dot(along));
    let (dw, aw) = (direction.dot(w), along.dot(w));
    let denominator = dd * aa - da * da;
    // Parallel to the ray: every point is as near as every other.
    let t = if denominator.abs() < 1e-12 { 0.0 } else { (da * dw - dd * aw) / denominator };
    a + along * t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::feature::BodyRef;
    use crate::prop::feature::{evaluate, FeatureOp, PropFeature};
    use crate::prop::profile::Placement;
    use crate::prop::solid::Shape;
    use crate::prop::surface::Surface;

    fn primitive(id: u32, shape: Shape, at: Vec3) -> PropFeature {
        PropFeature::new(
            PropFeatureId(id),
            FeatureOp::Primitive { shape, placement: Placement::at(at.x, at.y, at.z), surface: Surface::default() },
        )
    }

    fn cube(id: u32, size: f32, at: Vec3) -> PropFeature {
        primitive(id, Shape::Box { size: [size; 3] }, at)
    }

    /// Fails if a generator's outline drifts from the solid it describes.
    #[test]
    fn every_primitives_outline_lies_on_its_surface() {
        let shapes = [
            Shape::Box { size: [0.3, 0.4, 0.5] },
            Shape::Prism { sides: 8, radius: 0.2, height: 0.6 },
            Shape::Cone { sides: 6, radius: 0.2, height: 0.5 },
            Shape::Sphere { sides: 8, rings: 5, radius: 0.25 },
            Shape::Wedge { size: [0.3, 0.3, 0.2] },
        ];
        for shape in shapes {
            let solid = shape.solid(Surface::default());
            let outline = shape.outline();
            assert!(!outline.points.is_empty(), "{shape:?}");
            for point in &outline.points {
                assert!(on_surface(&solid, *point), "{shape:?}: {point} is off the surface");
            }
        }
    }

    #[test]
    fn a_box_offers_eight_corners_and_twelve_edges() {
        let evaluated = evaluate(&[cube(0, 0.2, Vec3::ZERO)]);
        let points = evaluated.pickables.iter().filter(|p| matches!(p.at, Located::Point(_))).count();
        let edges = evaluated.pickables.iter().filter(|p| matches!(p.at, Located::Edge(..))).count();
        assert_eq!((points, edges), (8, 12));
    }

    /// Fails if a reference names the boolean's polygons rather than the
    /// generator: lengthening the tool must not rename the target's corners.
    #[test]
    fn a_boolean_keeps_the_names_its_operands_gave() {
        let corner = GeomRef { element: Element::Point(0), source: Source { feature: PropFeatureId(0), copies: vec![] } };
        let build = |tool_size: f32| {
            let drilled = PropFeature::new(
                PropFeatureId(2),
                FeatureOp::Subtract { targets: vec![BodyRef::whole(PropFeatureId(0))], tools: vec![BodyRef::whole(PropFeatureId(1))] },
            );
            evaluate(&[cube(0, 0.2, Vec3::ZERO), cube(1, tool_size, Vec3::splat(0.1)), drilled])
        };
        let small = build(0.05);
        let large = build(0.08);
        assert_eq!(small.locate(&corner), large.locate(&corner));
        assert_eq!(small.locate(&corner), Some(Located::Point(Vec3::splat(-0.1))));
    }

    /// The tool's corner at the target's corner is cut away; it still
    /// resolves, but is not offered.
    #[test]
    fn a_corner_cut_away_resolves_but_is_not_offered() {
        let drilled = PropFeature::new(
            PropFeatureId(2),
            FeatureOp::Subtract { targets: vec![BodyRef::whole(PropFeatureId(0))], tools: vec![BodyRef::whole(PropFeatureId(1))] },
        );
        let evaluated = evaluate(&[cube(0, 0.2, Vec3::ZERO), cube(1, 0.1, Vec3::splat(0.1)), drilled]);
        // The target's +X+Y+Z corner, inside the tool.
        let outline = Shape::Box { size: [0.2; 3] }.outline();
        let index = outline.points.iter().position(|p| *p == Vec3::splat(0.1)).unwrap() as u32;
        let cut = GeomRef { element: Element::Point(index), source: Source { feature: PropFeatureId(0), copies: vec![] } };
        assert_eq!(evaluated.locate(&cut), Some(Located::Point(Vec3::splat(0.1))));
        assert!(!evaluated.pickables.iter().any(|p| p.reference == cut));
    }

    /// Fails if a copy shares its original's name, or if keeping the original
    /// renames the reflection.
    #[test]
    fn a_mirror_names_its_reflection_whether_or_not_the_original_stays() {
        let mirror = |keep_original| {
            PropFeature::new(
                PropFeatureId(1),
                FeatureOp::Mirror {
                    targets: vec![BodyRef::whole(PropFeatureId(0))],
                    about: crate::prop::plane::PlaneRef::Origin(crate::prop::plane::OriginPlane::Yz),
                    offset: 0.0,
                    keep_original,
                },
            )
        };
        let reflected = GeomRef {
            element: Element::Point(0),
            source: Source { feature: PropFeatureId(0), copies: vec![CopyOf { by: PropFeatureId(1), index: 1 }] },
        };
        let original = GeomRef { element: Element::Point(0), source: Source { feature: PropFeatureId(0), copies: vec![] } };
        let block = cube(0, 0.1, Vec3::new(0.3, 0.0, 0.0));

        let kept = evaluate(&[block.clone(), mirror(true)]);
        let alone = evaluate(&[block, mirror(false)]);
        assert_eq!(kept.locate(&reflected), alone.locate(&reflected));
        assert_eq!(kept.locate(&reflected), Some(Located::Point(Vec3::new(-0.25, -0.05, -0.05))));
        assert_eq!(kept.locate(&original), Some(Located::Point(Vec3::new(0.25, -0.05, -0.05))));
        assert_eq!(alone.locate(&original), None);
    }

    /// Straight-on orthographic projection, a pixel a millimeter.
    fn looking_down_z(p: Vec3) -> Option<Vec2> {
        Some(p.truncate() * 1000.0)
    }

    #[test]
    fn a_corner_beats_the_edges_that_meet_at_it() {
        let evaluated = evaluate(&[cube(0, 0.2, Vec3::ZERO)]);
        let ray = Ray3d::new(Vec3::new(0.097, 0.098, 1.0), Dir3::NEG_Z);
        let pointer = Pointer { ray, face: evaluated.face_distance(ray), project: looking_down_z };
        let picked = element_under(&evaluated.pickables, &pointer).unwrap();
        assert_eq!(picked.at, Located::Point(Vec3::splat(0.1)));
    }

    #[test]
    fn away_from_a_corner_an_edge_is_picked() {
        let evaluated = evaluate(&[cube(0, 0.2, Vec3::ZERO)]);
        let ray = Ray3d::new(Vec3::new(0.0, 0.097, 1.0), Dir3::NEG_Z);
        let pointer = Pointer { ray, face: evaluated.face_distance(ray), project: looking_down_z };
        let picked = element_under(&evaluated.pickables, &pointer).unwrap();
        let Located::Edge(a, b) = picked.at else { panic!("{picked:?}") };
        assert_eq!((a.y, b.y, a.z, b.z), (0.1, 0.1, 0.1, 0.1), "the near top edge, not the far one");
    }

    #[test]
    fn nothing_is_picked_in_the_middle_of_a_face() {
        let evaluated = evaluate(&[cube(0, 0.2, Vec3::ZERO)]);
        let ray = Ray3d::new(Vec3::new(0.0, 0.0, 1.0), Dir3::NEG_Z);
        let pointer = Pointer { ray, face: evaluated.face_distance(ray), project: looking_down_z };
        assert!(element_under(&evaluated.pickables, &pointer).is_none());
    }

    /// Fails if a face's normal comes from a winding nothing promises.
    #[test]
    fn every_flat_face_points_out_of_its_body() {
        use crate::prop::skin::{extrusion, revolution, skin, TaggedLoop};
        use crate::prop::surface::Surface;
        let points = |p: &[[f32; 2]]| vec![TaggedLoop::numbered(p.iter().map(|q| Vec2::from_array(*q)).collect())];
        let swept = |run| skin(&run, Surface::default()).unwrap().outline;
        let clockwise = [[0.0, 0.0], [0.0, 0.1], [0.1, 0.1], [0.1, 0.0]];
        let outlines = [
            Shape::Box { size: [0.3, 0.4, 0.5] }.outline(),
            Shape::Prism { sides: 6, radius: 0.2, height: 0.6 }.outline(),
            Shape::Wedge { size: [0.3, 0.3, 0.2] }.outline(),
            swept(extrusion(points(&clockwise), -0.2, false)),
            // Sectors of a cylinder, which are convex up to half a turn.
            swept(revolution(points(&[[0.0, -0.1], [0.1, -0.1], [0.1, 0.1], [0.0, 0.1]]), 90.0, 6)),
            swept(revolution(points(&[[0.0, -0.1], [0.0, 0.1], [-0.1, 0.1], [-0.1, -0.1]]), -120.0, 6)),
            Shape::Box { size: [0.3, 0.4, 0.5] }.outline().mirrored(Vec3::new(0.1, 0.0, 0.0), Vec3::X),
        ];
        for (n, outline) in outlines.iter().enumerate() {
            let middle = outline.points.iter().sum::<Vec3>() / outline.points.len() as f32;
            let mut flat = 0;
            for i in 0..outline.faces.len() as u32 {
                let face = outline.locate(Element::Face(i)).unwrap();
                let Located::Face { normal, .. } = &face else { unreachable!() };
                // A revolve's side on its own axis has no area to face
                // anywhere, and its side round the axis is curved.
                if !face.is_flat() {
                    continue;
                }
                flat += 1;
                let points = face.points();
                let center = points.iter().sum::<Vec3>() / points.len() as f32;
                assert!(normal.dot(center - middle) > 0.0, "outline {n}, face {i} points inwards");
            }
            assert!(flat >= 4, "outline {n} has only {flat} flat faces");
        }
    }

    #[test]
    fn a_click_on_a_box_lands_on_the_face_it_hit() {
        let evaluated = evaluate(&[cube(0, 0.2, Vec3::ZERO)]);
        let (body, face) = evaluated.surface_under(Ray3d::new(Vec3::new(0.03, 0.02, 1.0), Dir3::NEG_Z)).unwrap();
        assert_eq!(body.feature, PropFeatureId(0));
        let Some(Located::Face { normal, .. }) = evaluated.locate(&face.expect("a face")) else { panic!() };
        assert!(normal.distance(Vec3::Z) < 1e-6, "{normal}");
    }

    /// A sphere's band between two rings is one face, curved, and a click on
    /// any of its facets picks it.
    #[test]
    fn a_click_on_a_sphere_picks_the_band_it_hit() {
        let ball = primitive(0, Shape::Sphere { sides: 8, rings: 5, radius: 0.2 }, Vec3::ZERO);
        let evaluated = evaluate(&[ball]);
        let (body, face) = evaluated.surface_under(Ray3d::new(Vec3::new(0.05, 0.07, 1.0), Dir3::NEG_Z)).unwrap();
        assert_eq!(body.feature, PropFeatureId(0));
        let Some(Located::Face { facets, .. }) = evaluated.locate(&face.expect("a face")) else { panic!() };
        assert_eq!(facets.len(), 8, "one facet per step round the axis");
    }

    /// Fails if a hole's wall cannot be picked: it faces the opposite way
    /// from the tool face that made it.
    #[test]
    fn a_holes_wall_is_the_tools_face() {
        let drilled = PropFeature::new(
            PropFeatureId(2),
            FeatureOp::Subtract { targets: vec![BodyRef::whole(PropFeatureId(0))], tools: vec![BodyRef::whole(PropFeatureId(1))] },
        );
        let bore = primitive(1, Shape::Box { size: [0.1, 0.1, 0.4] }, Vec3::ZERO);
        let evaluated = evaluate(&[cube(0, 0.2, Vec3::ZERO), bore, drilled]);
        // Along the bore, just inside its -X wall, aimed at it.
        let ray = Ray3d::new(Vec3::new(0.0, 0.0, 0.0), Dir3::NEG_X);
        let (body, face) = evaluated.surface_under(ray).unwrap();
        assert_eq!(body.feature, PropFeatureId(2));
        assert_eq!(face.expect("a face").source.feature, PropFeatureId(1));
    }

    /// Fails if `element` stops coming first, which TOML refuses.
    #[test]
    fn a_reference_round_trips_through_toml() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Holder {
            at: GeomRef,
        }
        let held = Holder {
            at: GeomRef {
                element: Element::Edge(7),
                source: Source { feature: PropFeatureId(3), copies: vec![CopyOf { by: PropFeatureId(5), index: 2 }] },
            },
        };
        let text = toml::to_string_pretty(&held).unwrap();
        assert_eq!(toml::from_str::<Holder>(&text).unwrap(), held, "{text}");
    }
}
