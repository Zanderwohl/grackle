//! The plane a selection describes, if it describes one, and the plane
//! features made from them.
//!
//! Accepted selections are three corners, an edge and a corner, two edges, one
//! flat face, or one plane feature. The frame is decided by pick order alone —
//! never by bounds or the longest edge — so nudging one corner cannot spin
//! everything drawn on the plane through a right angle: the origin is the
//! first point, `x` runs towards the second, and the normal follows the
//! right-hand rule. An edge, when there is one, supplies the first two points,
//! so `x` runs along it. A face's normal is the way out of its body, so
//! something sketched on it extrudes outwards.
//!
//! Three origin planes are always present, so an empty prop has somewhere to
//! draw.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::get;
use crate::prop::document::{Pick, PropEditor};
use crate::prop::feature::{Evaluated, PropFeatureId};
use crate::prop::reference::{Located, Pointer, EDGE_PICK_PX};
use crate::prop::view::PropBuild;
use crate::tool::tool_helpers::dashed_line;

/// Metres. Points nearer than this to the line or plane they are tested
/// against are on it; the same order as picking's surface tolerance.
const ON_PLANE: f32 = 1e-4;

/// How far off the line the furthest point must be, as a fraction of how far
/// the selection reaches from its origin: roughly the sine of the smallest
/// angle accepted. Below it a plane exists but tilts wildly when one corner
/// moves a fraction of a millimetre, and anything drawn on it would too.
const MIN_SPREAD: f32 = 0.01;

/// The drawn square's margin past what it was made from, as a fraction of its
/// extent.
const PREVIEW_MARGIN: f32 = 0.25;

/// A frame, not the kernel's `normal · p = w`: something drawn on a plane
/// needs an origin and an in-plane direction as well as a normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub origin: Vec3,
    pub x: Dir3,
    pub normal: Dir3,
}

impl Plane {
    pub fn y(&self) -> Dir3 {
        Dir3::new_unchecked(self.normal.cross(*self.x))
    }

    pub fn distance(&self, point: Vec3) -> f32 {
        (point - self.origin).dot(*self.normal)
    }

    /// From the plane's own `x`/`y` to the prop's space.
    pub fn lift(&self, at: Vec2) -> Vec3 {
        self.origin + *self.x * at.x + *self.y() * at.y
    }

    /// Where `point` lands on the plane, seen straight down its normal, in the
    /// plane's own `x`/`y`.
    pub fn flatten(&self, point: Vec3) -> Vec2 {
        let offset = point - self.origin;
        Vec2::new(offset.dot(*self.x), offset.dot(*self.y()))
    }

    /// Where a ray meets the plane, from either side; `None` if it runs along
    /// it or the plane is behind it.
    pub fn hit(&self, ray: Ray3d) -> Option<Vec3> {
        let facing = ray.direction.dot(*self.normal);
        if facing.abs() < 1e-6 {
            return None;
        }
        let t = (self.origin - ray.origin).dot(*self.normal) / facing;
        (t > 0.0).then(|| ray.get_point(t))
    }
}

/// One of the three planes through the prop's origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginPlane {
    Xy,
    Yz,
    Xz,
}

impl OriginPlane {
    pub const ALL: [OriginPlane; 3] = [OriginPlane::Xy, OriginPlane::Yz, OriginPlane::Xz];

    /// Each frame reads the right way up when looked at down its normal from
    /// the front, the side and above, so a sketch's `x` is screen-right in
    /// the view it is drawn in. The `XZ` normal is `+Y` so a sketch on the
    /// floor extrudes up.
    pub fn plane(self) -> Plane {
        let (x, normal) = match self {
            OriginPlane::Xy => (Dir3::X, Dir3::Z),
            OriginPlane::Yz => (Dir3::NEG_Z, Dir3::X),
            OriginPlane::Xz => (Dir3::X, Dir3::Y),
        };
        Plane { origin: Vec3::ZERO, x, normal }
    }

    pub fn name(self) -> String {
        match self {
            OriginPlane::Xy => get!("prop.planes.xy"),
            OriginPlane::Yz => get!("prop.planes.yz"),
            OriginPlane::Xz => get!("prop.planes.xz"),
        }
    }
}

/// A plane something can be drawn on: an origin plane, or a plane feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaneRef {
    Origin(OriginPlane),
    Feature(PropFeatureId),
}

impl PlaneRef {
    pub fn feature(self) -> Option<PropFeatureId> {
        match self {
            PlaneRef::Origin(_) => None,
            PlaneRef::Feature(id) => Some(id),
        }
    }
}

/// Metres. An origin plane's half-width on an empty prop.
const MIN_ORIGIN_HALF: f32 = 0.05;

/// A plane as evaluated: the plane, and the square it is drawn and picked as.
#[derive(Clone, Debug, PartialEq)]
pub struct Datum {
    pub id: PlaneRef,
    pub plane: Plane,
    pub corners: [Vec3; 4],
}

impl Datum {
    /// From what a plane feature names, already located; `None` if it is no
    /// plane.
    pub fn through(id: PropFeatureId, located: &[Located]) -> Option<Datum> {
        let plane = plane_through_located(located)?;
        let corners = preview_corners(&plane, &all_points(located));
        Some(Datum { id: PlaneRef::Feature(id), plane, corners })
    }

    /// An origin plane, drawn as a square `half` each way of the origin:
    /// sized to the prop rather than fixed, since a prop may be a pistol
    /// grip or a cannon.
    pub fn origin(which: OriginPlane, half: f32) -> Datum {
        let plane = which.plane();
        let half = half.max(MIN_ORIGIN_HALF);
        let corner = |u: f32, v: f32| *plane.x * u * half + *plane.y() * v * half;
        let corners = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
        Datum { id: PlaneRef::Origin(which), plane, corners }
    }
}

/// The plane through what is picked, or `None` if the picks do not make one:
/// the wrong kinds or counts, a body among them, something that does not
/// resolve, a face that is not flat, or geometry that is coincident, collinear
/// or skew.
pub fn plane_through(picks: &[Pick], evaluated: &Evaluated) -> Option<Plane> {
    picked_plane(picks, evaluated).map(|(plane, _)| plane)
}

/// [`plane_through`], with the square to draw it as.
fn picked_plane(picks: &[Pick], evaluated: &Evaluated) -> Option<(Plane, [Vec3; 4])> {
    if let [Pick::Plane(id)] = picks {
        return evaluated.datum(*id).map(|datum| (datum.plane, datum.corners));
    }
    let located = locate_all(picks, evaluated)?;
    let plane = plane_through_located(&located)?;
    Some((plane, preview_corners(&plane, &all_points(&located))))
}

/// Every pick's position, or `None` if any is not geometry or does not
/// resolve.
fn locate_all(picks: &[Pick], evaluated: &Evaluated) -> Option<Vec<Located>> {
    picks
        .iter()
        .map(|pick| match pick {
            Pick::Body(_) | Pick::Plane(_) => None,
            Pick::Geom(reference) => evaluated.locate(reference),
        })
        .collect()
}

fn all_points(located: &[Located]) -> Vec<Vec3> {
    located.iter().flat_map(Located::points).collect()
}

/// [`plane_through`] once the picks are positions.
pub fn plane_through_located(located: &[Located]) -> Option<Plane> {
    match located {
        [face @ Located::Face { points, normal }] => face.is_flat().then(|| face_frame(points, *normal)).flatten(),
        _ => frame(&framing_points(located)?),
    }
}

/// The points that frame a plane, edges' ends first, if the picks are one of
/// the accepted shapes.
fn framing_points(located: &[Located]) -> Option<Vec<Vec3>> {
    let mut edges = vec![];
    let mut corners = vec![];
    for at in located {
        match *at {
            Located::Edge(a, b) => edges.extend([a, b]),
            Located::Point(p) => corners.push(p),
            Located::Face { .. } => return None,
        }
    }
    let shape = (edges.len() / 2, corners.len());
    matches!(shape, (0, 3) | (1, 1) | (2, 0)).then(|| [edges, corners].concat())
}

/// Origin at `points[0]`, `x` towards `points[1]`, normal from the point
/// furthest off that line — furthest, so two nearly parallel edges are judged
/// by their far ends rather than by whichever point happened to come third.
/// Then every point has to lie on the result.
fn frame(points: &[Vec3]) -> Option<Plane> {
    let origin = points[0];
    let along = points[1] - origin;
    if along.length() < ON_PLANE {
        return None;
    }
    let x = Dir3::new(along).ok()?;
    let (off_line, distance) = points[2..]
        .iter()
        .map(|p| {
            let across = x.cross(*p - origin);
            (across, across.length())
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    let reach = points.iter().map(|p| p.distance(origin)).fold(0.0, f32::max);
    if distance < ON_PLANE.max(reach * MIN_SPREAD) {
        return None;
    }
    let plane = Plane { origin, x, normal: Dir3::new(off_line).ok()? };
    points.iter().all(|p| plane.distance(*p).abs() < ON_PLANE).then_some(plane)
}

/// Origin at the face's first corner and `x` along its first edge that has
/// any length — a revolve's side can begin with two corners on the axis.
fn face_frame(points: &[Vec3], normal: Vec3) -> Option<Plane> {
    let normal = Dir3::new(normal).ok()?;
    let origin = points[0];
    let x = points[1..].iter().find_map(|p| {
        let along = (*p - origin).reject_from_normalized(*normal);
        (along.length() >= ON_PLANE).then(|| Dir3::new(along).ok()).flatten()
    })?;
    Some(Plane { origin, x, normal })
}

/// A square on `plane` around `points`, in its own `x` and `y`, the selection's
/// larger extent across plus a margin. Square rather than fitted, so an edge
/// and a corner close to it still preview as a plane rather than a sliver.
pub fn preview_corners(plane: &Plane, points: &[Vec3]) -> [Vec3; 4] {
    let (x, y) = (*plane.x, *plane.y());
    let flat = |p: &Vec3| {
        let offset = *p - plane.origin;
        Vec2::new(offset.dot(x), offset.dot(y))
    };
    let (min, max) = points
        .iter()
        .map(flat)
        .fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(min, max), p| (min.min(p), max.max(p)));
    let centre = (min + max) / 2.0;
    let half = (max - min).max_element() / 2.0 * (1.0 + PREVIEW_MARGIN);
    let at = |u: f32, v: f32| plane.origin + x * (centre.x + u * half) + y * (centre.y + v * half);
    [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)]
}

/// The plane the current selection describes. Recomputed only when the picks
/// or the evaluated prop change, never because a frame went by.
#[derive(Resource, Default)]
pub struct PickedPlane {
    pub plane: Option<Plane>,
    /// Where the preview is drawn, found alongside `plane`.
    corners: Option<[Vec3; 4]>,
    /// The picks and build generation `plane` was found from.
    from: Option<(Vec<Pick>, Option<u64>)>,
    /// Set by the panels while something that would use the plane is hovered.
    pub previewed: bool,
}

pub struct PropPlanePlugin;

impl Plugin for PropPlanePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PickedPlane>().add_systems(
            Update,
            (find_the_picked_plane, preview_the_picked_plane).chain().run_if(in_state(AppMode::Prop)),
        );
    }
}

fn find_the_picked_plane(editor: Res<PropEditor>, build: Res<PropBuild>, mut picked: ResMut<PickedPlane>) {
    let unchanged = picked
        .from
        .as_ref()
        .is_some_and(|(picks, built)| picks.as_slice() == editor.picks() && *built == build.built());
    if unchanged {
        return;
    }
    let found = picked_plane(editor.picks(), &build.evaluated);
    picked.plane = found.map(|(plane, _)| plane);
    picked.corners = found.map(|(_, corners)| corners);
    picked.from = Some((editor.picks().to_vec(), build.built()));
}

const PREVIEW_COLOUR: Color = Color::srgb(0.55, 0.8, 0.95);

fn preview_the_picked_plane(picked: Res<PickedPlane>, mut gizmos: Gizmos) {
    let (true, Some(corners)) = (picked.previewed, picked.corners) else { return };
    draw_square(&mut gizmos, &corners, PREVIEW_COLOUR);
}

/// The plane feature whose drawn square has an edge within
/// [`EDGE_PICK_PX`] of the pointer. By its border rather than its inside, so a
/// plane through a body does not stop clicks reaching the body.
pub fn plane_under<P: Fn(Vec3) -> Option<Vec2>>(planes: &[Datum], pointer: &Pointer<P>) -> Option<PlaneRef> {
    planes
        .iter()
        .filter_map(|datum| {
            (0..4)
                .filter_map(|i| pointer.to_segment(datum.corners[i], datum.corners[(i + 1) % 4]))
                .min_by(f32::total_cmp)
                .map(|pixels| (datum.id, pixels))
        })
        .filter(|(_, pixels)| *pixels <= EDGE_PICK_PX)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// Dashes scale with the square, so a small prop is not drawn solid.
pub fn draw_square(gizmos: &mut Gizmos, corners: &[Vec3; 4], colour: Color) {
    let side = corners[0].distance(corners[1]);
    for i in 0..4 {
        dashed_line(gizmos, corners[i], corners[(i + 1) % 4], colour, side / 24.0, side / 48.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prop::feature::{evaluate, FeatureOp, PropFeature, PropFeatureId};
    use crate::prop::profile::Placement;
    use crate::prop::reference::{Element, GeomRef, Source};
    use crate::prop::solid::Shape;
    use crate::prop::surface::Surface;

    fn point(x: f32, y: f32, z: f32) -> Located {
        Located::Point(Vec3::new(x, y, z))
    }

    fn edge(a: [f32; 3], b: [f32; 3]) -> Located {
        Located::Edge(Vec3::from_array(a), Vec3::from_array(b))
    }

    fn assert_frame(plane: Option<Plane>, origin: Vec3, x: Vec3, normal: Vec3) {
        let plane = plane.expect("a plane");
        assert!(plane.origin.distance(origin) < 1e-6, "{plane:?}");
        assert!(plane.x.distance(x) < 1e-6, "{plane:?}");
        assert!(plane.normal.distance(normal) < 1e-6, "{plane:?}");
    }

    #[test]
    fn three_corners_make_a_plane_framed_by_their_order() {
        let picked = [point(0.0, 0.0, 0.0), point(2.0, 0.0, 0.0), point(0.0, 3.0, 0.0)];
        assert_frame(plane_through_located(&picked), Vec3::ZERO, Vec3::X, Vec3::Z);

        let reversed = [picked[0].clone(), picked[2].clone(), picked[1].clone()];
        assert_frame(plane_through_located(&reversed), Vec3::ZERO, Vec3::Y, Vec3::NEG_Z);
    }

    #[test]
    fn corners_in_a_line_or_on_top_of_each_other_make_none() {
        let in_a_line = [point(0.0, 0.0, 0.0), point(1.0, 0.0, 0.0), point(5.0, 0.0, 0.0)];
        assert_eq!(plane_through_located(&in_a_line), None);
        let doubled = [point(0.0, 0.0, 0.0), point(0.0, 0.0, 0.0), point(0.0, 1.0, 0.0)];
        assert_eq!(plane_through_located(&doubled), None);
    }

    /// The edge sets `x` whichever was picked first.
    #[test]
    fn an_edge_and_a_corner_off_it_make_a_plane_along_the_edge() {
        let along_x = edge([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        let above = point(0.5, 0.0, 2.0);
        assert_frame(plane_through_located(&[along_x.clone(), above.clone()]), Vec3::ZERO, Vec3::X, Vec3::NEG_Y);
        assert_frame(plane_through_located(&[above.clone(), along_x.clone()]), Vec3::ZERO, Vec3::X, Vec3::NEG_Y);

        let on_its_line = point(3.0, 0.0, 0.0);
        assert_eq!(plane_through_located(&[along_x.clone(), on_its_line.clone()]), None);
    }

    #[test]
    fn crossing_and_parallel_edges_make_a_plane_and_skew_ones_do_not() {
        let base = edge([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        let crossing = edge([0.5, -1.0, 0.0], [0.5, 1.0, 0.0]);
        assert_frame(plane_through_located(&[base.clone(), crossing.clone()]), Vec3::ZERO, Vec3::X, Vec3::Z);

        let parallel = edge([0.0, 2.0, 0.0], [1.0, 2.0, 0.0]);
        assert_frame(plane_through_located(&[base.clone(), parallel.clone()]), Vec3::ZERO, Vec3::X, Vec3::Z);

        let skew = edge([0.5, -1.0, 1.0], [0.5, 1.0, 1.0]);
        assert_eq!(plane_through_located(&[base.clone(), skew.clone()]), None);

        let collinear = edge([2.0, 0.0, 0.0], [3.0, 0.0, 0.0]);
        assert_eq!(plane_through_located(&[base.clone(), collinear.clone()]), None);
    }

    /// Fails if the threshold is a distance alone: 0.2 mm off a 30 cm line is
    /// a plane whose tilt a nudge would swing.
    #[test]
    fn corners_nearly_in_a_line_make_none() {
        let barrel = [point(0.0, 0.0, 0.0), point(0.3, 0.0, 0.0)];
        let barely = [barrel[0].clone(), barrel[1].clone(), point(0.15, 0.0002, 0.0)];
        assert_eq!(plane_through_located(&barely), None);
        let clearly = [barrel[0].clone(), barrel[1].clone(), point(0.15, 0.01, 0.0)];
        assert!(plane_through_located(&clearly).is_some());

        let long = edge([0.0, 0.0, 0.0], [0.3, 0.0, 0.0]);
        let hairline = edge([0.0, 0.001, 0.0], [0.3, 0.001, 0.0]);
        assert_eq!(plane_through_located(&[long.clone(), hairline.clone()]), None);
    }

    #[test]
    fn the_preview_is_a_square_on_the_plane_around_the_selection() {
        let picked = [point(0.0, 0.0, 0.0), point(2.0, 0.0, 0.0), point(0.0, 3.0, 0.0)];
        let plane = plane_through_located(&picked).unwrap();
        let points = framing_points(&picked).unwrap();
        let corners = preview_corners(&plane, &points);
        for corner in corners {
            assert!(plane.distance(corner).abs() < 1e-6, "{corner} is off the plane");
        }
        let sides: Vec<f32> = (0..4).map(|i| corners[i].distance(corners[(i + 1) % 4])).collect();
        for side in &sides {
            assert!((side - 3.0 * (1.0 + PREVIEW_MARGIN)).abs() < 1e-5, "{sides:?}");
        }
        let (min, max) = (corners[0], corners[2]);
        for p in points {
            assert!(p.x > min.x && p.x < max.x && p.y > min.y && p.y < max.y, "{p} is outside");
        }
    }

    #[test]
    fn a_flat_face_alone_makes_a_plane_facing_out() {
        let evaluated = evaluate(&[cube_feature()]);
        // Face 1 of an extrude is its far cap: the cube's +Z side.
        let plane = plane_through(&[face(1)], &evaluated).expect("a plane");
        assert!(plane.normal.distance(Vec3::Z) < 1e-6, "{plane:?}");
        assert!((plane.distance(Vec3::ZERO) + 0.1).abs() < 1e-6);
        assert_eq!(plane_through(&[face(1), face(0)], &evaluated), None, "one face, not two");
    }

    /// Fails if a plane feature is resolved against the end of the list: a
    /// mirror below it that takes the original away must not move it.
    #[test]
    fn a_plane_feature_stands_where_its_references_were_when_it_ran() {
        let plane = PropFeature::new(PropFeatureId(1), FeatureOp::Plane { through: vec![geom_ref(Element::Face(1))] });
        let mirror = PropFeature::new(
            PropFeatureId(2),
            FeatureOp::Mirror {
                target: PropFeatureId(0),
                axis: crate::prop::feature::Axis::Z,
                offset: 1.0,
                keep_original: false,
            },
        );
        let evaluated = evaluate(&[cube_feature(), plane.clone(), mirror]);
        assert!(evaluated.problems.is_empty(), "{:?}", evaluated.problems);
        let datum = evaluated.datum(PlaneRef::Feature(PropFeatureId(1))).expect("a plane");
        assert!(datum.plane.normal.distance(Vec3::Z) < 1e-6);
        assert!((datum.plane.distance(Vec3::ZERO) + 0.1).abs() < 1e-6);
        assert_eq!(plane_through(&[Pick::Plane(PlaneRef::Feature(PropFeatureId(1)))], &evaluated), Some(datum.plane));
        assert!(evaluated.bodies.len() == 1, "a plane is no body");

        let before_its_cube = evaluate(&[plane, cube_feature()]);
        assert!(before_its_cube.datum(PlaneRef::Feature(PropFeatureId(1))).is_none());
        assert_eq!(before_its_cube.problems.len(), 1);
    }

    /// Fails if a plane can be dragged above what it was made through, or the
    /// generator deleted without being told about it.
    #[test]
    fn a_plane_is_pinned_below_what_it_names() {
        use crate::prop::document::PropDoc;
        let mut doc = PropDoc::new("test");
        let cube = doc.push(cube_feature().op);
        let plane = doc.push(FeatureOp::Plane {
            through: vec![GeomRef { element: Element::Face(1), source: Source { feature: cube, copies: vec![] } }],
        });
        assert_eq!(doc.dependants(cube), vec![plane]);
        assert!(!doc.move_to(plane, 0));
        assert_eq!(doc.index_of(plane), Some(1));
    }

    #[test]
    fn a_plane_feature_survives_the_file() {
        use crate::prop::document::{parse, to_text, PropDoc};
        let mut doc = PropDoc::new("test");
        let cube = doc.push(cube_feature().op);
        doc.push(FeatureOp::Plane {
            through: vec![
                GeomRef { element: Element::Edge(4), source: Source { feature: cube, copies: vec![] } },
                GeomRef { element: Element::Point(2), source: Source { feature: cube, copies: vec![] } },
            ],
        });
        let text = to_text(&doc).unwrap();
        assert_eq!(parse(&text).unwrap(), doc, "{text}");
    }

    #[test]
    fn origin_planes_are_right_handed_and_always_there() {
        for which in OriginPlane::ALL {
            let plane = which.plane();
            assert!(plane.x.dot(*plane.normal).abs() < 1e-6);
            assert!(plane.x.cross(*plane.y()).distance(*plane.normal) < 1e-6, "{which:?}");
        }
        assert_eq!(OriginPlane::Xz.plane().normal, Dir3::Y, "a floor sketch extrudes up");

        let empty = evaluate(&[]);
        for which in OriginPlane::ALL {
            let datum = empty.datum(PlaneRef::Origin(which)).expect("an origin plane");
            assert!(datum.corners.iter().all(|c| datum.plane.distance(*c).abs() < 1e-6));
            assert_eq!(plane_through(&[Pick::Plane(PlaneRef::Origin(which))], &empty), Some(datum.plane));
        }
    }

    /// Fails if an origin plane is a fixed size: it should frame the prop.
    #[test]
    fn origin_planes_grow_with_the_prop() {
        let small = evaluate(&[]).datum(PlaneRef::Origin(OriginPlane::Xy)).unwrap().corners[2];
        let big = PropFeature::new(
            PropFeatureId(0),
            FeatureOp::Primitive {
                shape: Shape::Box { size: [2.0, 0.2, 0.2] },
                placement: Placement::default(),
                surface: Surface::default(),
            },
        );
        let large = evaluate(&[big]).datum(PlaneRef::Origin(OriginPlane::Xy)).unwrap().corners[2];
        assert!(large.x > 1.0 && large.x > small.x * 10.0, "{small} vs {large}");
    }

    fn cube_feature() -> PropFeature {
        PropFeature::new(
            PropFeatureId(0),
            FeatureOp::Primitive {
                shape: Shape::Box { size: [0.2; 3] },
                placement: Placement::default(),
                surface: Surface::default(),
            },
        )
    }

    fn geom_ref(element: Element) -> GeomRef {
        GeomRef { element, source: Source { feature: PropFeatureId(0), copies: vec![] } }
    }

    fn face(index: u32) -> Pick {
        Pick::Geom(geom_ref(Element::Face(index)))
    }

    #[test]
    fn other_combinations_make_none() {
        let corner = point(0.0, 0.0, 0.0);
        let along_x = edge([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        assert_eq!(plane_through_located(&[]), None);
        assert_eq!(plane_through_located(&[along_x.clone()]), None);
        assert_eq!(plane_through_located(&[corner.clone(), point(1.0, 0.0, 0.0).clone()]), None);
        let four = [corner, point(1.0, 0.0, 0.0), point(0.0, 1.0, 0.0), point(1.0, 1.0, 0.0)];
        assert_eq!(plane_through_located(&four), None, "three corners, not any number");
        let crossing = edge([0.5, -1.0, 0.0], [0.5, 1.0, 0.0]);
        assert_eq!(plane_through_located(&[along_x.clone(), crossing.clone(), point(0.0, 1.0, 0.0).clone()]), None);
    }

    #[test]
    fn picks_are_resolved_against_the_prop_and_a_body_spoils_them() {
        let cube = PropFeature::new(
            PropFeatureId(0),
            FeatureOp::Primitive {
                shape: Shape::Box { size: [0.2; 3] },
                placement: Placement::default(),
                surface: Surface::default(),
            },
        );
        let evaluated = evaluate(&[cube]);
        let corner = |index| {
            Pick::Geom(GeomRef {
                element: Element::Point(index),
                source: Source { feature: PropFeatureId(0), copies: vec![] },
            })
        };
        // Three corners of the near cap: the cube's -Z face.
        let face = [corner(0), corner(1), corner(2)];
        let plane = plane_through(&face, &evaluated).expect("a plane");
        assert!(plane.normal.dot(Vec3::Z).abs() > 1.0 - 1e-6);
        assert!((plane.distance(Vec3::ZERO).abs() - 0.1).abs() < 1e-6);

        let mut spoiled = face.to_vec();
        spoiled[2] = Pick::Body(PropFeatureId(0));
        assert_eq!(plane_through(&spoiled, &evaluated), None);

        let mut dangling = face.to_vec();
        dangling[2] = corner(99);
        assert_eq!(plane_through(&dangling, &evaluated), None);
    }
}
