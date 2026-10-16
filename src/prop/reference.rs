//! Naming a prop's corners and edges so something later in the list can refer
//! to them.
//!
//! The kernel's output is convex polygon soup, split wherever a boolean cut
//! it, so nothing in a [`Solid`] is stable from one evaluation to the next. A
//! reference instead names where the geometry *came from*: the generator that
//! made it ([`Outline`], built from the same sweep the solid was), and every
//! mirror or pattern copy it went through on the way. A boolean cannot break
//! one, since a reference never looks at a boolean's result; a corner that a
//! boolean cut away still resolves, to where it would have been.
//!
//! The price is that an edge a boolean *made*, where a bore leaves a face, has
//! no name and cannot be picked.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::prop::feature::PropFeatureId;
use crate::prop::profile::{sides, Profile};
use crate::prop::solid::{is_full_turn, Solid};

/// One duplication a piece of geometry went through. The original is never
/// named, so inserting a pattern does not rename what it copies; a mirror's
/// reflection is copy 1 whether or not the original is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CopyOf {
    pub by: PropFeatureId,
    pub index: u32,
}

/// The generator, and its copies in the order they were made.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Source {
    pub feature: PropFeatureId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copies: Vec<CopyOf>,
}

impl Source {
    /// Every feature the name depends on.
    pub fn features(&self) -> impl Iterator<Item = PropFeatureId> + '_ {
        std::iter::once(self.feature).chain(self.copies.iter().map(|copy| copy.by))
    }
}

/// An index into the generator's [`Outline`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Element {
    Point(u32),
    Edge(u32),
}

/// `element` first: TOML needs every scalar before any sub-table.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GeomRef {
    pub element: Element,
    pub source: Source,
}

/// Where a reference is, in the prop's own space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Located {
    Point(Vec3),
    Edge(Vec3, Vec3),
}

/// A generator's corners and the edges between them.
///
/// Numbered by formula from the profile *as authored*, not as wound for the
/// kernel, so dragging a point across the loop does not renumber everything.
/// Degenerate entries — a revolve's corners on the axis, the zero-length edges
/// between them — are kept so the formula holds; they are filtered when
/// picking.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    pub points: Vec<Vec3>,
    pub edges: Vec<[u32; 2]>,
}

impl Outline {
    /// Matches [`Solid::extrude`]: points `0..n` on the near cap, `n..2n` on
    /// the far; edges round the near cap, round the far, then the sides.
    pub fn extrude(profile: &Profile, depth: f32, midplane: bool) -> Outline {
        let loop_ = profile.points();
        if loop_.len() < 3 || depth.abs() < f32::EPSILON {
            return Outline::default();
        }
        let (near, far) = if midplane { (-depth / 2.0, depth / 2.0) } else { (0.0, depth) };
        let n = loop_.len() as u32;
        let points = [near, far]
            .into_iter()
            .flat_map(|z| loop_.iter().map(move |p| Vec3::new(p.x, p.y, z)))
            .collect();
        let mut edges = Vec::with_capacity(3 * n as usize);
        for cap in 0..2 {
            for i in 0..n {
                edges.push([cap * n + i, cap * n + (i + 1) % n]);
            }
        }
        for i in 0..n {
            edges.push([i, n + i]);
        }
        Outline { points, edges }
    }

    /// Matches [`Solid::revolve`]: ring `k` holds points `k*n..(k+1)*n`.
    /// Edges round every ring first, then along the sweep. A side face's
    /// diagonal is a triangulation detail rather than an edge, so is left out.
    pub fn revolve(profile: &Profile, degrees: f32, segments: u32) -> Outline {
        let loop_ = profile.points();
        if loop_.len() < 3 {
            return Outline::default();
        }
        let full = is_full_turn(degrees);
        let sweep = if full { std::f32::consts::TAU } else { degrees.to_radians() };
        let steps = sides(segments);
        let rings = if full { steps } else { steps + 1 };
        let n = loop_.len() as u32;

        let points = (0..rings)
            .flat_map(|k| {
                let (sin, cos) = (sweep * k as f32 / steps as f32).sin_cos();
                loop_.iter().map(move |p| Vec3::new(p.x * cos, p.y, -p.x * sin))
            })
            .collect();
        let mut edges = Vec::with_capacity(((rings + steps) * n) as usize);
        for k in 0..rings {
            for i in 0..n {
                edges.push([k * n + i, k * n + (i + 1) % n]);
            }
        }
        for k in 0..steps {
            let next = (k + 1) % rings;
            for i in 0..n {
                edges.push([k * n + i, next * n + i]);
            }
        }
        Outline { points, edges }
    }

    pub fn transformed(mut self, transform: Transform) -> Outline {
        for point in &mut self.points {
            *point = transform.transform_point(*point);
        }
        self
    }

    /// Reflected in the plane through `origin` with normal `normal`, as
    /// [`Solid::mirrored`] does.
    pub fn mirrored(mut self, origin: Vec3, normal: Vec3) -> Outline {
        let normal = normal.normalize_or_zero();
        for point in &mut self.points {
            let offset = *point - origin;
            *point = origin + offset - 2.0 * offset.dot(normal) * normal;
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
        }
    }
}

/// One generator's outline, in the prop's space, under its name.
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

/// Metres. Well above the kernel's rounding, well below anything modelled.
const ON_SURFACE: f32 = 1e-4;

/// An edge shorter than this has collapsed onto a point; metres.
const DEGENERATE_EDGE: f32 = 1e-5;

/// Every named element of `wires` that lies on `solid`'s surface. A cut-away
/// corner still resolves but is not offered: a dot floating in the air beside
/// a bore reads as a bug. An edge is offered when both ends and its middle
/// are on the surface, which keeps a long tool's sides from poking out of the
/// hole it made.
pub fn pickables(body: PropFeatureId, solid: &Solid, wires: &[Wire]) -> Vec<Pickable> {
    let mut found = Vec::new();
    for wire in wires {
        let named = |element| GeomRef { element, source: wire.source.clone() };
        for (i, &point) in wire.outline.points.iter().enumerate() {
            if on_surface(solid, point) {
                found.push(Pickable { body, reference: named(Element::Point(i as u32)), at: Located::Point(point) });
            }
        }
        for (i, &[a, b]) in wire.outline.edges.iter().enumerate() {
            let (a, b) = (wire.outline.points[a as usize], wire.outline.points[b as usize]);
            if a.distance(b) < DEGENERATE_EDGE {
                continue;
            }
            if [a, (a + b) / 2.0, b].into_iter().all(|p| on_surface(solid, p)) {
                found.push(Pickable { body, reference: named(Element::Edge(i as u32)), at: Located::Edge(a, b) });
            }
        }
    }
    found
}

fn on_surface(solid: &Solid, point: Vec3) -> bool {
    solid.polygons().iter().any(|polygon| {
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
    })
}

/// Pixels. Generous, because a corner has no area to hit.
pub const POINT_PICK_PX: f32 = 10.0;
/// Pixels. Tighter than a point's, so a click near a corner takes the corner.
pub const EDGE_PICK_PX: f32 = 6.0;

/// The element under the pointer: the nearest corner within
/// [`POINT_PICK_PX`], or failing that the nearest edge within
/// [`EDGE_PICK_PX`]. `project` is world to viewport pixels; `face` is how far
/// down the ray the nearest face is, so a corner round the back of a body is
/// not picked through it.
///
/// Engine-free so it can be tested without a camera.
pub fn element_under<'a>(
    pickables: &'a [Pickable],
    ray: Ray3d,
    face: Option<f32>,
    project: impl Fn(Vec3) -> Option<Vec2>,
) -> Option<&'a Pickable> {
    let origin = ray.origin;
    let direction = *ray.direction;
    let depth = |p: Vec3| (p - origin).dot(direction);
    // Something on the face that was hit is at the same depth, give or take
    // the kernel's rounding and the slant of the face across a few pixels.
    let visible = |p: Vec3| face.is_none_or(|face| depth(p) <= face + 0.002 + face * 0.01);
    // In pixels, at the candidate's own depth: any point down the ray lands
    // on the pointer, perspective or orthographic.
    let pixels = |p: Vec3| {
        let on_screen = project(p)?;
        let pointer = project(origin + direction * depth(p))?;
        Some(on_screen.distance(pointer))
    };

    let nearest = |within: f32, wanted: fn(&Located) -> Option<Vec3>, at: &dyn Fn(Vec3, &Located) -> Vec3| {
        pickables
            .iter()
            .filter_map(|pickable| {
                let anchor = wanted(&pickable.at)?;
                let p = at(anchor, &pickable.at);
                if depth(p) <= 0.0 || !visible(p) {
                    return None;
                }
                let distance = pixels(p)?;
                (distance <= within).then_some((pickable, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(pickable, _)| pickable)
    };

    let point = |located: &Located| match located {
        Located::Point(p) => Some(*p),
        Located::Edge(..) => None,
    };
    let edge = |located: &Located| match located {
        Located::Edge(a, _) => Some(*a),
        Located::Point(_) => None,
    };
    nearest(POINT_PICK_PX, point, &|p, _| p).or_else(|| {
        nearest(EDGE_PICK_PX, edge, &|_, located| match *located {
            Located::Edge(a, b) => closest_on_segment(origin, direction, a, b),
            Located::Point(p) => p,
        })
    })
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
    use crate::prop::feature::{evaluate, BooleanOp, FeatureOp, PropFeature};
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
                FeatureOp::Boolean { op: BooleanOp::Subtract, target: PropFeatureId(0), tool: PropFeatureId(1) },
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
            FeatureOp::Boolean { op: BooleanOp::Subtract, target: PropFeatureId(0), tool: PropFeatureId(1) },
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
                    target: PropFeatureId(0),
                    axis: crate::prop::feature::Axis::X,
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

    /// Straight-on orthographic projection, a pixel a millimetre.
    fn looking_down_z(p: Vec3) -> Option<Vec2> {
        Some(p.truncate() * 1000.0)
    }

    #[test]
    fn a_corner_beats_the_edges_that_meet_at_it() {
        let evaluated = evaluate(&[cube(0, 0.2, Vec3::ZERO)]);
        let ray = Ray3d::new(Vec3::new(0.097, 0.098, 1.0), Dir3::NEG_Z);
        let face = evaluated.face_distance(ray);
        let picked = element_under(&evaluated.pickables, ray, face, looking_down_z).unwrap();
        assert_eq!(picked.at, Located::Point(Vec3::splat(0.1)));
    }

    #[test]
    fn away_from_a_corner_an_edge_is_picked() {
        let evaluated = evaluate(&[cube(0, 0.2, Vec3::ZERO)]);
        let ray = Ray3d::new(Vec3::new(0.0, 0.097, 1.0), Dir3::NEG_Z);
        let face = evaluated.face_distance(ray);
        let picked = element_under(&evaluated.pickables, ray, face, looking_down_z).unwrap();
        let Located::Edge(a, b) = picked.at else { panic!("{picked:?}") };
        assert_eq!((a.y, b.y, a.z, b.z), (0.1, 0.1, 0.1, 0.1), "the near top edge, not the far one");
    }

    #[test]
    fn nothing_is_picked_in_the_middle_of_a_face() {
        let evaluated = evaluate(&[cube(0, 0.2, Vec3::ZERO)]);
        let ray = Ray3d::new(Vec3::new(0.0, 0.0, 1.0), Dir3::NEG_Z);
        let face = evaluated.face_distance(ray);
        assert!(element_under(&evaluated.pickables, ray, face, looking_down_z).is_none());
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
