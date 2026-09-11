//! A closed surface, how to sweep one out of a profile, and how to hand it to
//! Bevy.
//!
//! This sits between [`super::profile`] (2D shapes) and [`super::csg`] (which
//! cuts closed surfaces out of one another) and owns the one rule both of them
//! depend on: **a solid's faces point outwards**.
//!
//! Rather than deriving the winding of every sweep by hand and getting one of
//! them backwards, a solid is built however is convenient and then *measured*:
//! [`Solid::oriented_outward`] computes the enclosed volume and flips the
//! whole thing if it came out negative. An inside-out solid is the failure
//! that does not announce itself — it renders as a hole in the world from one
//! side and correctly from the other, and a boolean against it keeps exactly
//! the wrong half — so it is worth a dot product per face to make it
//! unrepresentable.
//!
//! Meshes come out **flat shaded and grouped by surface**. One `Mesh3d` takes
//! one material, so a prop with a wooden stock and a steel barrel is two
//! entities; grouping here rather than at draw time means a solid that is all
//! one material is one entity, which is the common case.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::prop::csg::{self, Polygon};
use crate::prop::profile::{triangulate, Profile, MIN_SIDES};
use crate::prop::surface::Surface;

/// A bag of convex polygons that is meant to be closed. See [`super::csg`] for
/// why both of those words matter.
#[derive(Clone, Debug, Default)]
pub struct Solid {
    polygons: Vec<Polygon>,
}

impl Solid {
    pub fn from_polygons(polygons: Vec<Polygon>) -> Solid {
        Solid { polygons }.oriented_outward()
    }

    pub fn polygons(&self) -> &[Polygon] {
        &self.polygons
    }

    pub fn is_empty(&self) -> bool {
        self.polygons.is_empty()
    }

    /// How many triangles this would draw as — the number a mapper is actually
    /// spending when they turn a cylinder's side count up.
    pub fn triangle_count(&self) -> usize {
        self.polygons
            .iter()
            .map(|polygon| polygon.vertices.len().saturating_sub(2))
            .sum()
    }

    /// Flip the whole solid if it turned out to be enclosing negative volume.
    ///
    /// Cheaper than reasoning about the winding of every sweep, and unlike
    /// reasoning it cannot be subtly wrong for one case out of five. A solid
    /// measuring zero is left alone: it is either empty or not closed, and
    /// flipping it would be guessing.
    pub fn oriented_outward(mut self) -> Solid {
        if csg::signed_volume_x6(&self.polygons) < 0.0 {
            for polygon in &mut self.polygons {
                polygon.flip();
            }
        }
        self
    }

    /// The volume enclosed, in cubic metres.
    pub fn volume(&self) -> f32 {
        csg::signed_volume_x6(&self.polygons) / 6.0
    }

    pub fn transformed(mut self, transform: Transform) -> Solid {
        for polygon in &mut self.polygons {
            for vertex in &mut polygon.vertices {
                *vertex = transform.transform_point(*vertex);
            }
        }
        self.rebuild_planes();
        // A negative scale turns a solid inside out, and a mirror is exactly
        // that. Measuring afterwards means neither the caller nor this
        // function has to remember which transforms do it.
        self.oriented_outward()
    }

    pub fn translated(self, offset: Vec3) -> Solid {
        self.transformed(Transform::from_translation(offset))
    }

    /// The solid reflected in a plane through `origin` with normal `normal`.
    pub fn mirrored(self, origin: Vec3, normal: Vec3) -> Solid {
        let normal = normal.normalize_or_zero();
        if normal == Vec3::ZERO {
            return self;
        }
        let mut mirrored = self;
        for polygon in &mut mirrored.polygons {
            for vertex in &mut polygon.vertices {
                let offset = *vertex - origin;
                *vertex = origin + offset - 2.0 * offset.dot(normal) * normal;
            }
        }
        mirrored.rebuild_planes();
        mirrored.oriented_outward()
    }

    /// Repaint every face.
    pub fn painted(mut self, surface: Surface) -> Solid {
        for polygon in &mut self.polygons {
            polygon.surface = surface;
        }
        self
    }

    /// Recompute each face's plane from its (moved) corners.
    ///
    /// A face whose corners have become collinear is dropped rather than kept
    /// with a stale plane: the kernel classifies points against planes, and a
    /// plane that no longer describes its own polygon is how a boolean starts
    /// keeping the wrong side.
    fn rebuild_planes(&mut self) {
        self.polygons.retain_mut(|polygon| {
            match csg::Plane::from_points(polygon.vertices[0], polygon.vertices[1], polygon.vertices[2]) {
                Some(plane) => {
                    polygon.plane = plane;
                    true
                }
                None => false,
            }
        });
    }

    pub fn union(&self, other: &Solid) -> Solid {
        Solid { polygons: csg::union(&self.polygons, &other.polygons) }
    }

    /// Cut `other` out, leaving the walls of the hole made of whatever this
    /// solid is mostly made of.
    ///
    /// The kernel has no opinion about materials: a face it keeps from the
    /// tool keeps the tool's surface, which is the arithmetically honest
    /// answer and the wrong one for a modelling tool. Drilling a bore through
    /// a steel barrel should leave steel inside the bore, not whatever colour
    /// the drill happened to be — and nobody sets a drill's material, because
    /// a drill is not part of the finished prop.
    ///
    /// So the rule lives here rather than in [`super::csg`]: repaint the tool
    /// before cutting with it, and every face the cut produces comes out
    /// right. The same applies to an intersection, whose new faces are also
    /// the tool's.
    pub fn subtract(&self, other: &Solid) -> Solid {
        let tool = other.clone().painted(self.dominant_surface());
        Solid { polygons: csg::subtract(&self.polygons, &tool.polygons) }
    }

    pub fn intersect(&self, other: &Solid) -> Solid {
        let tool = other.clone().painted(self.dominant_surface());
        Solid { polygons: csg::intersect(&self.polygons, &tool.polygons) }
    }

    /// The surface covering the most of this solid.
    ///
    /// By **area**, not by face count: a barrel is mostly its long sides and a
    /// handful of little end facets, and counting faces would let the ends
    /// decide what the barrel is made of.
    pub fn dominant_surface(&self) -> Surface {
        let mut totals: Vec<(Surface, f32)> = Vec::new();
        for polygon in &self.polygons {
            let a = polygon.vertices[0];
            let area: f32 = polygon.vertices[1..]
                .windows(2)
                .map(|w| (w[0] - a).cross(w[1] - a).length() / 2.0)
                .sum();
            match totals.iter_mut().find(|(surface, _)| *surface == polygon.surface) {
                Some(entry) => entry.1 += area,
                None => totals.push((polygon.surface, area)),
            }
        }
        totals
            .into_iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(surface, _)| surface)
            .unwrap_or_default()
    }

    /// The box the solid sits in, or `None` if there is nothing in it.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for polygon in &self.polygons {
            for vertex in &polygon.vertices {
                min = min.min(*vertex);
                max = max.max(*vertex);
            }
        }
        (min.x <= max.x).then_some((min, max))
    }

    /// Push a profile along its sketch's normal.
    ///
    /// `depth` runs along the sketch's local `+Z`. `midplane` is CAD's
    /// symmetric extrude — half each way instead of all of it forwards — which
    /// is what almost every part of a weapon wants, because a barrel is
    /// centred on its axis rather than starting at it.
    ///
    /// Caps are **triangulated, not fanned**: the kernel takes convex polygons
    /// only, and a profile with a notch in it is exactly the case a fan gets
    /// wrong without saying so.
    pub fn extrude(
        profile: &Profile,
        depth: f32,
        midplane: bool,
        surface: Surface,
    ) -> Solid {
        let points = profile.wound_ccw();
        if points.len() < 3 || depth.abs() < f32::EPSILON {
            return Solid::default();
        }

        let (near, far) = if midplane {
            (-depth / 2.0, depth / 2.0)
        } else {
            (0.0, depth)
        };
        let at = |p: Vec2, z: f32| Vec3::new(p.x, p.y, z);

        let mut polygons = Vec::new();
        for [a, b, c] in triangulate(&points) {
            polygons.extend(Polygon::new(
                vec![at(points[a], far), at(points[b], far), at(points[c], far)],
                surface,
            ));
            polygons.extend(Polygon::new(
                vec![at(points[c], near), at(points[b], near), at(points[a], near)],
                surface,
            ));
        }
        for i in 0..points.len() {
            let j = (i + 1) % points.len();
            // Planar by construction: both edges run along the same extrusion
            // direction, so this stays one quad rather than two triangles.
            polygons.extend(Polygon::new(
                vec![
                    at(points[i], near),
                    at(points[j], near),
                    at(points[j], far),
                    at(points[i], far),
                ],
                surface,
            ));
        }

        Solid::from_polygons(polygons)
    }

    /// Spin a profile about the sketch's local `+Y`.
    ///
    /// `segments` is the facet count, and it is the whole of what makes this a
    /// Quake-era shape rather than a smooth one. A full turn wraps and needs
    /// no caps; anything less is capped with the profile at each end, so a
    /// half-revolve is a solid half rather than an open shell.
    ///
    /// Side faces are **triangles**. A quad between two steps of a revolve is
    /// only planar when the profile edge it came from is parallel to the axis
    /// or square to it, and a nearly-planar quad handed to the kernel is a
    /// polygon whose own corners are on both sides of its plane.
    pub fn revolve(
        profile: &Profile,
        degrees: f32,
        segments: u32,
        surface: Surface,
    ) -> Solid {
        let points = profile.wound_ccw();
        let segments = segments.max(MIN_SIDES);
        if points.len() < 3 {
            return Solid::default();
        }

        let full = degrees.abs() >= 360.0 - 1e-3;
        let sweep = if full { std::f32::consts::TAU } else { degrees.to_radians() };
        let steps = segments as usize;

        let ring = |k: usize| -> Vec<Vec3> {
            let angle = sweep * k as f32 / steps as f32;
            let (sin, cos) = angle.sin_cos();
            points
                .iter()
                .map(|p| Vec3::new(p.x * cos, p.y, -p.x * sin))
                .collect()
        };

        let rings: Vec<Vec<Vec3>> = (0..=steps).map(ring).collect();
        let mut polygons = Vec::new();
        for k in 0..steps {
            // The last ring of a full turn is the first ring again, to within
            // rounding; using index 0 rather than the computed one is what
            // keeps the seam welded instead of leaving a hairline gap.
            let (head, tail) = if full && k + 1 == steps {
                (&rings[k], &rings[0])
            } else {
                (&rings[k], &rings[k + 1])
            };
            for i in 0..points.len() {
                let j = (i + 1) % points.len();
                polygons.extend(Polygon::new(vec![head[i], head[j], tail[j]], surface));
                polygons.extend(Polygon::new(vec![head[i], tail[j], tail[i]], surface));
            }
        }

        if !full {
            for [a, b, c] in triangulate(&points) {
                let first = &rings[0];
                let last = &rings[steps];
                polygons.extend(Polygon::new(vec![first[a], first[b], first[c]], surface));
                polygons.extend(Polygon::new(vec![last[c], last[b], last[a]], surface));
            }
        }

        Solid::from_polygons(polygons)
    }

    /// A box centred on the origin.
    pub fn cuboid(size: Vec3, surface: Surface) -> Solid {
        Solid::extrude(
            &Profile::Rect { half: [size.x / 2.0, size.y / 2.0] },
            size.z,
            true,
            surface,
        )
    }

    /// The meshes this solid draws as, one per distinct surface.
    ///
    /// Flat shaded: each face's own plane normal on each of its vertices,
    /// which is both the look and the only honest answer after a boolean has
    /// cut a face in half. UVs are a planar projection in metres, so nothing
    /// has to author them and a future texture has something to sit on.
    pub fn meshes(&self) -> Vec<(Surface, Mesh)> {
        let mut groups: Vec<(Surface, MeshParts)> = Vec::new();
        for polygon in &self.polygons {
            let group = match groups.iter_mut().find(|(surface, _)| *surface == polygon.surface) {
                Some((_, parts)) => parts,
                None => {
                    groups.push((polygon.surface, MeshParts::default()));
                    &mut groups.last_mut().expect("just pushed").1
                }
            };
            group.push(polygon);
        }
        groups.into_iter().map(|(surface, parts)| (surface, parts.build())).collect()
    }
}

/// One mesh under construction: a fan per polygon, with the polygon's own
/// normal on every vertex.
#[derive(Default)]
struct MeshParts {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl MeshParts {
    fn push(&mut self, polygon: &Polygon) {
        let normal = polygon.plane.normal;
        // Project onto whichever pair of world axes the face is most square
        // to. Picking the dominant axis rather than a fixed one is what stops
        // a wall's texture from being smeared into a stripe.
        let (u_axis, v_axis) = {
            let a = normal.abs();
            if a.x >= a.y && a.x >= a.z {
                (Vec3::Z, Vec3::Y)
            } else if a.y >= a.z {
                (Vec3::X, Vec3::Z)
            } else {
                (Vec3::X, Vec3::Y)
            }
        };

        let base = self.positions.len() as u32;
        for vertex in &polygon.vertices {
            self.positions.push(vertex.to_array());
            self.normals.push(normal.to_array());
            self.uvs.push([vertex.dot(u_axis), vertex.dot(v_axis)]);
        }
        // A fan is correct here and only here: the kernel's polygons are
        // convex, which is the invariant the whole module is built on.
        for i in 1..polygon.vertices.len() as u32 - 1 {
            self.indices.extend_from_slice(&[base, base + i, base + i + 1]);
        }
    }

    fn build(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// A 3D primitive, as a handful of numbers.
///
/// Every one of these is a [`Profile`] swept, and they are named variants
/// rather than left as "draw a profile and extrude it" because a mapper
/// reaching for a cylinder should get a cylinder. What they are *not* is a
/// second geometry path: [`Shape::solid`] goes through the same two sweeps
/// everything else does, so a bug in a cap is one bug rather than six.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Shape {
    /// Centred on the origin.
    Box {
        #[serde(with = "crate::prop::nice_f32::array")]
        size: [f32; 3],
    },
    /// The cylinder: an n-gon pushed along its own axis, standing on `Y`.
    Prism {
        sides: u32,
        #[serde(with = "crate::prop::nice_f32::scalar")]
        radius: f32,
        #[serde(with = "crate::prop::nice_f32::scalar")]
        height: f32,
    },
    /// An n-sided cone, standing on `Y`, apex up.
    Cone {
        sides: u32,
        #[serde(with = "crate::prop::nice_f32::scalar")]
        radius: f32,
        #[serde(with = "crate::prop::nice_f32::scalar")]
        height: f32,
    },
    /// A ball of `sides` around and `rings` over the top. Coarse on purpose —
    /// the default is what a rivet wants, not what a planet does.
    Sphere {
        sides: u32,
        rings: u32,
        #[serde(with = "crate::prop::nice_f32::scalar")]
        radius: f32,
    },
    /// A right triangle pushed sideways: a ramp, a foregrip, a stock's comb.
    Wedge {
        #[serde(with = "crate::prop::nice_f32::array")]
        size: [f32; 3],
    },
}

impl Default for Shape {
    fn default() -> Self {
        Shape::Box { size: [0.2, 0.2, 0.2] }
    }
}

impl Shape {
    pub fn name(&self) -> String {
        match self {
            Shape::Box { .. } => crate::get!("prop.shapes.box"),
            Shape::Prism { .. } => crate::get!("prop.shapes.prism"),
            Shape::Cone { .. } => crate::get!("prop.shapes.cone"),
            Shape::Sphere { .. } => crate::get!("prop.shapes.sphere"),
            Shape::Wedge { .. } => crate::get!("prop.shapes.wedge"),
        }
    }

    /// The solid, in its own space, centred on the origin.
    pub fn solid(&self, surface: Surface) -> Solid {
        match *self {
            Shape::Box { size } => Solid::cuboid(Vec3::from_array(size), surface),
            // Extruded along the sketch normal and then stood up, rather than
            // sketched in the ground plane: one sweep, turned, instead of a
            // second way of writing the same one.
            Shape::Prism { sides, radius, height } => {
                Solid::extrude(&Profile::Ngon { sides, radius }, height, true, surface)
                    .transformed(Transform::from_rotation(Quat::from_rotation_x(
                        -std::f32::consts::FRAC_PI_2,
                    )))
            }
            Shape::Cone { sides, radius, height } => Solid::revolve(
                &Profile::Points {
                    points: vec![
                        [0.0, -height / 2.0],
                        [radius, -height / 2.0],
                        [0.0, height / 2.0],
                    ],
                },
                360.0,
                sides,
                surface,
            ),
            Shape::Sphere { sides, rings, radius } => {
                // Half a disc, spun. The rings are the profile's own point
                // count, so a sphere's two resolutions are the two numbers
                // that actually cost triangles.
                let rings = rings.max(2);
                let points: Vec<[f32; 2]> = (0..=rings)
                    .map(|i| {
                        let angle = std::f32::consts::PI * i as f32 / rings as f32
                            - std::f32::consts::FRAC_PI_2;
                        [radius * angle.cos(), radius * angle.sin()]
                    })
                    .collect();
                Solid::revolve(&Profile::Points { points }, 360.0, sides, surface)
            }
            Shape::Wedge { size } => Solid::extrude(
                &Profile::Points {
                    points: vec![
                        [-size[0] / 2.0, -size[1] / 2.0],
                        [size[0] / 2.0, -size[1] / 2.0],
                        [-size[0] / 2.0, size[1] / 2.0],
                    ],
                },
                size[2],
                true,
                surface,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant the kernel rests on, asserted about the thing that
    /// produces the kernel's input. A sweep wound the wrong way measures
    /// negative, and the orientation pass is what is being checked here.
    #[test]
    fn every_primitive_encloses_positive_volume() {
        let shapes = [
            Shape::Box { size: [0.3, 0.4, 0.5] },
            Shape::Prism { sides: 8, radius: 0.2, height: 0.6 },
            Shape::Cone { sides: 6, radius: 0.2, height: 0.5 },
            Shape::Sphere { sides: 8, rings: 5, radius: 0.25 },
            Shape::Wedge { size: [0.3, 0.3, 0.2] },
        ];
        for shape in shapes {
            let solid = shape.solid(Surface::default());
            assert!(
                solid.volume() > 0.0,
                "{shape:?} came out inside out, at {} cubic metres",
                solid.volume(),
            );
        }
    }

    /// A prism's volume is arithmetic, so the sweep can be checked rather than
    /// eyeballed: a regular n-gon of corner radius r has area n·r²·sin(2π/n)/2.
    #[test]
    fn a_prism_holds_what_its_cross_section_says_it_should() {
        let (sides, radius, height) = (12u32, 0.2f32, 0.6f32);
        let solid = Shape::Prism { sides, radius, height }.solid(Surface::default());
        let area = sides as f32 * radius * radius
            * (std::f32::consts::TAU / sides as f32).sin()
            / 2.0;
        assert!(
            (solid.volume() - area * height).abs() < 1e-4,
            "a {sides}-gon prism measured {} rather than {}",
            solid.volume(),
            area * height,
        );
    }

    /// A sphere is a revolve with caps that must *not* appear, and a seam that
    /// must close. Both failures leave the surface open, and an open surface
    /// measures the wrong volume rather than erroring.
    #[test]
    fn a_fine_sphere_approaches_the_real_thing() {
        let solid = Shape::Sphere { sides: 48, rings: 24, radius: 1.0 }.solid(Surface::default());
        let ideal = 4.0 / 3.0 * std::f32::consts::PI;
        // A faceted sphere is inscribed, so it is always a little short; the
        // bound is what a 48×24 approximation is worth rather than a tolerance.
        assert!(
            solid.volume() > ideal * 0.99 && solid.volume() < ideal,
            "a 48x24 sphere measured {} against {ideal}",
            solid.volume(),
        );
    }

    /// A mirror is a negative scale, which turns a surface inside out. Solids
    /// measure themselves afterwards, so the caller never has to know that.
    #[test]
    fn mirroring_does_not_turn_a_solid_inside_out() {
        let solid = Shape::Wedge { size: [0.3, 0.3, 0.2] }.solid(Surface::default());
        let mirrored = solid.clone().mirrored(Vec3::ZERO, Vec3::X);
        assert!((mirrored.volume() - solid.volume()).abs() < 1e-5);
        assert!(mirrored.volume() > 0.0, "the mirrored copy is inside out");
    }

    /// Drilling a bore through a steel barrel leaves steel inside the bore.
    /// The kernel does not do this on its own — it keeps the tool's faces
    /// wearing the tool's surface — so this is the test for the repaint in
    /// [`Solid::subtract`], and the failure is a prop with bright default
    /// grey down every hole in it.
    #[test]
    fn the_walls_of_a_cut_are_made_of_what_was_cut() {
        use crate::prop::surface::{Style, Tint};
        let oak = Surface::new(Style::Wood, Tint([0.5, 0.3, 0.1]));
        let block = Shape::Box { size: [1.0, 1.0, 1.0] }.solid(oak);
        let bore = Shape::Box { size: [0.2, 0.2, 2.0] }.solid(Surface::default());

        let drilled = block.subtract(&bore);
        assert!(!drilled.is_empty(), "the subtraction removed everything");
        assert!(
            drilled.polygons().iter().all(|polygon| polygon.surface == oak),
            "a face came back wearing the drill's surface",
        );
        assert_eq!(drilled.meshes().len(), 1, "the drill left a second material behind");
    }

    /// A union is the other way round, and deliberately so: joining a wooden
    /// stock to a steel receiver is two materials meeting, not one material
    /// swallowing the other.
    #[test]
    fn a_union_keeps_both_materials() {
        use crate::prop::surface::{Style, Tint};
        let oak = Surface::new(Style::Wood, Tint([0.5, 0.3, 0.1]));
        let steel = Surface::new(Style::Metal, Tint::default());
        let joined = Shape::Box { size: [1.0, 1.0, 1.0] }.solid(steel).union(
            &Shape::Box { size: [1.0, 1.0, 1.0] }.solid(oak).translated(Vec3::new(0.5, 0.0, 0.0)),
        );
        assert_eq!(joined.meshes().len(), 2);
    }

    /// Two materials means two meshes, and one material means one. Getting
    /// this wrong costs an entity per face rather than producing anything
    /// visibly broken.
    #[test]
    fn meshes_are_grouped_by_surface() {
        use crate::prop::surface::{Style, Tint};
        let steel = Surface::new(Style::Metal, Tint::default());
        let oak = Surface::new(Style::Wood, Tint([0.5, 0.3, 0.1]));

        let one = Shape::Box { size: [1.0, 1.0, 1.0] }.solid(steel);
        assert_eq!(one.meshes().len(), 1);

        let two = one.union(&Shape::Box { size: [1.0, 1.0, 1.0] }.solid(oak)
            .translated(Vec3::new(2.0, 0.0, 0.0)));
        assert_eq!(two.meshes().len(), 2);
    }
}
