//! The plane a selection describes, if it describes one.
//!
//! Accepted selections are three corners, an edge and a corner, or two edges.
//! The frame is decided by pick order alone — never by bounds or the longest
//! edge — so nudging one corner cannot spin everything drawn on the plane
//! through a right angle: the origin is the first point, `x` runs towards the
//! second, and the normal follows the right-hand rule. An edge, when there is
//! one, supplies the first two points, so `x` runs along it.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::prop::document::{Pick, PropEditor};
use crate::prop::feature::Evaluated;
use crate::prop::reference::Located;
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

/// The preview's margin past the selection, as a fraction of its extent.
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
}

/// The plane through what is picked, or `None` if the picks do not make one:
/// the wrong kinds or counts, a body among them, something that does not
/// resolve, or geometry that is coincident, collinear or skew.
pub fn plane_through(picks: &[Pick], evaluated: &Evaluated) -> Option<Plane> {
    plane_through_located(&locate_all(picks, evaluated)?)
}

/// Every pick's position, or `None` if any is a body or does not resolve.
fn locate_all(picks: &[Pick], evaluated: &Evaluated) -> Option<Vec<Located>> {
    picks
        .iter()
        .map(|pick| match pick {
            Pick::Body(_) => None,
            Pick::Geom(reference) => evaluated.locate(reference),
        })
        .collect()
}

/// [`plane_through`] once the picks are positions.
pub fn plane_through_located(located: &[Located]) -> Option<Plane> {
    frame(&framing_points(located)?)
}

/// The points that frame a plane, edges' ends first, if the picks are one of
/// the accepted shapes.
fn framing_points(located: &[Located]) -> Option<Vec<Vec3>> {
    let edges = located.iter().filter_map(|at| match *at {
        Located::Edge(a, b) => Some([a, b]),
        Located::Point(_) => None,
    });
    let corners = located.iter().filter_map(|at| match *at {
        Located::Point(p) => Some(p),
        Located::Edge(..) => None,
    });
    let points: Vec<Vec3> = edges.clone().flatten().chain(corners.clone()).collect();
    let shape = (edges.count(), corners.count());
    matches!(shape, (0, 3) | (1, 1) | (2, 0)).then_some(points)
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
    let points = locate_all(editor.picks(), &build.evaluated).and_then(|located| framing_points(&located));
    picked.plane = points.as_deref().and_then(frame);
    picked.corners = picked.plane.zip(points).map(|(plane, points)| preview_corners(&plane, &points));
    picked.from = Some((editor.picks().to_vec(), build.built()));
}

const PREVIEW_COLOUR: Color = Color::srgb(0.55, 0.8, 0.95);

fn preview_the_picked_plane(picked: Res<PickedPlane>, mut gizmos: Gizmos) {
    let (true, Some(corners)) = (picked.previewed, picked.corners) else { return };
    // Dashes scale with the square, so a small prop is not drawn solid.
    let side = corners[0].distance(corners[1]);
    for i in 0..4 {
        dashed_line(&mut gizmos, corners[i], corners[(i + 1) % 4], PREVIEW_COLOUR, side / 24.0, side / 48.0);
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

        let reversed = [picked[0], picked[2], picked[1]];
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
        assert_frame(plane_through_located(&[along_x, above]), Vec3::ZERO, Vec3::X, Vec3::NEG_Y);
        assert_frame(plane_through_located(&[above, along_x]), Vec3::ZERO, Vec3::X, Vec3::NEG_Y);

        let on_its_line = point(3.0, 0.0, 0.0);
        assert_eq!(plane_through_located(&[along_x, on_its_line]), None);
    }

    #[test]
    fn crossing_and_parallel_edges_make_a_plane_and_skew_ones_do_not() {
        let base = edge([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        let crossing = edge([0.5, -1.0, 0.0], [0.5, 1.0, 0.0]);
        assert_frame(plane_through_located(&[base, crossing]), Vec3::ZERO, Vec3::X, Vec3::Z);

        let parallel = edge([0.0, 2.0, 0.0], [1.0, 2.0, 0.0]);
        assert_frame(plane_through_located(&[base, parallel]), Vec3::ZERO, Vec3::X, Vec3::Z);

        let skew = edge([0.5, -1.0, 1.0], [0.5, 1.0, 1.0]);
        assert_eq!(plane_through_located(&[base, skew]), None);

        let collinear = edge([2.0, 0.0, 0.0], [3.0, 0.0, 0.0]);
        assert_eq!(plane_through_located(&[base, collinear]), None);
    }

    /// Fails if the threshold is a distance alone: 0.2 mm off a 30 cm line is
    /// a plane whose tilt a nudge would swing.
    #[test]
    fn corners_nearly_in_a_line_make_none() {
        let barrel = [point(0.0, 0.0, 0.0), point(0.3, 0.0, 0.0)];
        let barely = [barrel[0], barrel[1], point(0.15, 0.0002, 0.0)];
        assert_eq!(plane_through_located(&barely), None);
        let clearly = [barrel[0], barrel[1], point(0.15, 0.01, 0.0)];
        assert!(plane_through_located(&clearly).is_some());

        let long = edge([0.0, 0.0, 0.0], [0.3, 0.0, 0.0]);
        let hairline = edge([0.0, 0.001, 0.0], [0.3, 0.001, 0.0]);
        assert_eq!(plane_through_located(&[long, hairline]), None);
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
    fn other_combinations_make_none() {
        let corner = point(0.0, 0.0, 0.0);
        let along_x = edge([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        assert_eq!(plane_through_located(&[]), None);
        assert_eq!(plane_through_located(&[along_x]), None);
        assert_eq!(plane_through_located(&[corner, point(1.0, 0.0, 0.0)]), None);
        let four = [corner, point(1.0, 0.0, 0.0), point(0.0, 1.0, 0.0), point(1.0, 1.0, 0.0)];
        assert_eq!(plane_through_located(&four), None, "three corners, not any number");
        let crossing = edge([0.5, -1.0, 0.0], [0.5, 1.0, 0.0]);
        assert_eq!(plane_through_located(&[along_x, crossing, point(0.0, 1.0, 0.0)]), None);
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
