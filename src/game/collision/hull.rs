//! Convex hulls, and a moving box against one.
//!
//! A box swept against a hull is a point swept against the hull grown by the
//! box (their Minkowski sum). The sum's faces are among the hull's faces, the
//! box's axes, and each hull edge crossed with each axis; every candidate is
//! stored as a support plane, which is exact because a candidate that is not a
//! face of the sum is only redundant. Without the edge crossings (the bevels)
//! a box catches on the air past a sharp corner.

use bevy::prelude::*;

/// Normals closer than this are one plane.
const SAME_NORMAL: f32 = 1.0e-6;

/// A convex solid, or a flat one: a triangle is a hull with no thickness.
#[derive(Clone, Debug, PartialEq)]
pub struct Hull {
    /// Outward unit normals, each with how far along it the hull reaches.
    /// The axes come right after the faces, in `+X -X +Y -Y +Z -Z` order, so
    /// a cuboid's escapes are tried in the order they always were.
    planes: Vec<(Vec3, f32)>,
    min: Vec3,
    max: Vec3,
}

/// Where a sweep first touched something: a fraction of the move, the
/// surface's outward normal there, and `normal · center` at the touch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f32,
    pub normal: Vec3,
    pub plane: f32,
}

/// `center`, moved onto `n · x = plane` along `n`. On an axis it is written
/// as the plane's own number rather than as a step from `center`, which keeps
/// a box against a box wall on exactly the bits the per-axis code reached;
/// the ragdoll pins those.
pub fn onto_plane(center: Vec3, n: Vec3, plane: f32) -> Vec3 {
    let mut moved = center;
    match (0..3).find(|axis| n[*axis].abs() == 1.0) {
        Some(axis) => moved[axis] = n[axis] * plane,
        None => moved += n * (plane - n.dot(center)),
    }
    moved
}

const AXES: [Vec3; 6] = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];

impl Hull {
    /// `faces` are outward normals and `edges` directions, either sense; the
    /// caller states them because deriving them from a point cloud is a convex
    /// hull algorithm this does not need yet.
    pub fn new(vertices: &[Vec3], faces: &[Vec3], edges: &[Vec3]) -> Hull {
        let mut normals: Vec<Vec3> = Vec::new();
        let mut add = |n: Vec3| {
            let Some(n) = n.try_normalize() else { return };
            if !normals.iter().any(|m| m.abs_diff_eq(n, SAME_NORMAL)) {
                normals.push(n);
            }
        };
        faces.iter().copied().for_each(&mut add);
        AXES.into_iter().for_each(&mut add);
        for edge in edges {
            for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                let crossed = edge.cross(axis);
                if crossed.length_squared() > SAME_NORMAL {
                    add(crossed);
                    add(-crossed);
                }
            }
        }

        let reach = |n: Vec3| vertices.iter().map(|v| v.dot(n)).fold(f32::NEG_INFINITY, f32::max);
        let planes = normals.into_iter().map(|n| (n, reach(n))).collect();
        let min = vertices.iter().copied().fold(Vec3::INFINITY, Vec3::min);
        let max = vertices.iter().copied().fold(Vec3::NEG_INFINITY, Vec3::max);
        Hull { planes, min, max }
    }

    pub fn cuboid(min: Vec3, max: Vec3) -> Hull {
        let corners: Vec<Vec3> = (0..8)
            .map(|i| Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            ))
            .collect();
        Hull::new(&corners, &AXES, &[])
    }

    /// A convex face given depth behind it: `normal` is the side it is hit
    /// from, and the hull reaches `depth` the other way. The face's vertices
    /// go counter-clockwise about `normal`.
    pub fn slab(face: &[Vec3], normal: Vec3, depth: f32) -> Hull {
        let behind = -normal * depth;
        let vertices: Vec<Vec3> = face.iter().copied().chain(face.iter().map(|v| *v + behind)).collect();
        let edges: Vec<Vec3> = (0..face.len()).map(|i| face[(i + 1) % face.len()] - face[i]).collect();
        let mut faces = vec![normal, -normal];
        faces.extend(edges.iter().map(|edge| edge.cross(normal)));
        let mut directions = edges;
        directions.push(normal);
        Hull::new(&vertices, &faces, &directions)
    }

    /// Two-sided: it can be hit, and escaped, from either face.
    pub fn triangle(a: Vec3, b: Vec3, c: Vec3) -> Hull {
        let n = (b - a).cross(c - a);
        Hull::new(&[a, b, c], &[n, -n], &[b - a, c - b, a - c])
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        (self.min, self.max)
    }

    /// The planes of this hull grown by a box of half extents `half`.
    fn grown(&self, half: Vec3) -> impl Iterator<Item = (Vec3, f32)> + '_ {
        self.planes.iter().map(move |&(n, reach)| (n, reach + half.dot(n.abs())))
    }

    /// The first touch of a box moving by `delta`, if before the end of the
    /// move. A box already inside is not hit (that is
    /// [`Hull::escapes`]'s job); one touching and moving in is hit at zero.
    pub fn sweep(&self, center: Vec3, half: Vec3, delta: Vec3) -> Option<Hit> {
        enter(self.grown(half), center, delta)
    }

    /// Strict: touching is not overlapping.
    pub fn overlaps(&self, center: Vec3, half: Vec3) -> bool {
        self.grown(half).all(|(n, w)| n.dot(center) - w < 0.0)
    }

    /// Distance to where the ray enters, with the entry face's normal. An
    /// origin inside is a hit at zero, as [`crate::common::ray::ray_aabb_entry`]
    /// has it.
    pub fn ray(&self, ray: &Ray3d, range: f32) -> Option<(f32, Vec3)> {
        let direction = Vec3::from(ray.direction);
        let mut entry = f32::NEG_INFINITY;
        let mut exit = f32::INFINITY;
        let mut normal = -direction;
        for &(n, w) in &self.planes {
            let distance = n.dot(ray.origin) - w;
            let closing = n.dot(direction);
            if closing == 0.0 {
                if distance > 0.0 {
                    return None;
                }
                continue;
            }
            let t = -distance / closing;
            if closing < 0.0 {
                if t > entry {
                    entry = t;
                    normal = n;
                }
            } else {
                exit = exit.min(t);
            }
        }
        if exit < entry.max(0.0) || entry > range {
            return None;
        }
        Some((entry.max(0.0), normal))
    }

    /// Where a box overlapping this hull could go to be just clear of it, one
    /// candidate per plane, `skin` beyond it.
    pub fn escapes(&self, center: Vec3, half: Vec3, skin: f32) -> impl Iterator<Item = Vec3> + '_ {
        self.grown(half).map(move |(n, w)| onto_plane(center, n, w + skin))
    }
}

/// A point moving by `delta` against the solid `n · x < w` for every plane.
/// Strictly inside is no hit; parallel to a plane while on or outside it is
/// none either, so sliding along a wall's face does not catch on it.
pub(super) fn enter(planes: impl Iterator<Item = (Vec3, f32)>, from: Vec3, delta: Vec3) -> Option<Hit> {
    let mut entry = f32::NEG_INFINITY;
    let mut exit = f32::INFINITY;
    let mut normal = Vec3::ZERO;
    let mut plane = 0.0;
    let mut outside = false;
    for (n, w) in planes {
        let distance = n.dot(from) - w;
        outside |= distance >= 0.0;
        let closing = n.dot(delta);
        if closing == 0.0 {
            if distance >= 0.0 {
                return None;
            }
            continue;
        }
        let t = -distance / closing;
        if closing < 0.0 {
            if t > entry {
                entry = t;
                normal = n;
                plane = w;
            }
        } else {
            exit = exit.min(t);
        }
    }
    (outside && (0.0..1.0).contains(&entry) && entry < exit).then_some(Hit { t: entry, normal, plane })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALF: Vec3 = Vec3::splat(0.5);

    /// A wall turned 45° about Y, its inner face through the origin facing
    /// `-X -Z`, a meter thick.
    fn turned_wall() -> Hull {
        let out = Vec3::new(-1.0, 0.0, -1.0).normalize();
        let along = Vec3::new(1.0, 0.0, -1.0).normalize();
        let mut vertices = Vec::new();
        for s in [-5.0, 5.0] {
            for d in [0.0, -1.0] {
                for y in [-5.0, 5.0] {
                    vertices.push(along * s + out * d + Vec3::Y * y);
                }
            }
        }
        Hull::new(&vertices, &[out, -out, along, -along, Vec3::Y, Vec3::NEG_Y], &[along, out, Vec3::Y])
    }

    #[test]
    fn a_cuboid_has_six_planes_in_axis_order() {
        let hull = Hull::cuboid(Vec3::ZERO, Vec3::ONE);
        let normals: Vec<Vec3> = hull.planes.iter().map(|(n, _)| *n).collect();
        assert_eq!(normals, AXES);
    }

    #[test]
    fn a_box_moving_into_a_turned_wall_stops_with_its_corner_on_it() {
        let hull = turned_wall();
        let start = Vec3::new(-3.0, 0.0, -3.0);
        let hit = hull.sweep(start, HALF, Vec3::new(6.0, 0.0, 6.0)).expect("passed through");

        let reached = start + Vec3::new(6.0, 0.0, 6.0) * hit.t;
        // The leading corner is at reached + (0.5, _, 0.5), on the plane x + z = 0.
        assert!((reached.x + 0.5 + reached.z + 0.5).abs() < 1e-4, "{reached}");
        assert!(hit.normal.abs_diff_eq(Vec3::new(-1.0, 0.0, -1.0).normalize(), 1e-5), "{}", hit.normal);
    }

    #[test]
    fn a_box_sliding_parallel_to_a_turned_wall_is_not_stopped() {
        let hull = turned_wall();
        let along = Vec3::new(1.0, 0.0, -1.0);
        assert_eq!(hull.sweep(Vec3::new(-3.0, 0.0, -3.0), HALF, along * 2.0), None);
    }

    #[test]
    fn a_box_already_inside_is_not_swept_but_can_escape_along_the_wall_normal() {
        let hull = turned_wall();
        let inside = Vec3::new(0.1, 0.0, 0.1);
        assert!(hull.overlaps(inside, HALF));
        assert_eq!(hull.sweep(inside, HALF, Vec3::X), None);

        let nearest = hull
            .escapes(inside, HALF, 0.001)
            .min_by(|a, b| (*a - inside).length().total_cmp(&(*b - inside).length()))
            .unwrap();
        assert!(!hull.overlaps(nearest, HALF));
        assert!((nearest - inside).normalize().abs_diff_eq(Vec3::new(-1.0, 0.0, -1.0).normalize(), 1e-4));
    }

    #[test]
    fn a_slab_is_its_face_with_depth_behind_it() {
        // The floor of a room, facing up into it.
        let face = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 2.0), Vec3::new(2.0, 0.0, 2.0), Vec3::new(2.0, 0.0, 0.0)];
        let slab = Hull::slab(&face, Vec3::Y, 0.5);
        assert_eq!(slab.bounds(), (Vec3::new(0.0, -0.5, 0.0), Vec3::new(2.0, 0.0, 2.0)));
        assert_eq!(slab.planes.len(), 6, "a rectangle's slab is a box: {:?}", slab.planes);
        assert!(slab.overlaps(Vec3::new(1.0, 0.0, 1.0), Vec3::splat(0.1)));
        assert!(!slab.overlaps(Vec3::new(3.0, 0.0, 1.0), Vec3::splat(0.1)));
    }

    #[test]
    fn a_triangle_is_hit_from_either_side() {
        let floor = Hull::triangle(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 0.0, -2.0), Vec3::new(0.0, 0.0, 2.0));

        let from_above = floor.sweep(Vec3::new(0.0, 3.0, 0.0), HALF, Vec3::NEG_Y * 6.0).unwrap();
        assert!((3.0 - 6.0 * from_above.t - 0.5).abs() < 1e-5);
        assert!(from_above.normal.abs_diff_eq(Vec3::Y, 1e-6));

        let from_below = floor.sweep(Vec3::new(0.0, -3.0, 0.0), HALF, Vec3::Y * 6.0).unwrap();
        assert!(from_below.normal.abs_diff_eq(Vec3::NEG_Y, 1e-6));
    }

    /// Breaks without the bevel planes: the box overlaps the triangle's
    /// bounding box beside its slanted edge, but not the triangle.
    #[test]
    fn a_box_beside_a_triangles_slanted_edge_misses_it() {
        let floor = Hull::triangle(Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 4.0));
        let beside = Vec3::new(3.0, 0.0, 3.0);
        assert!(!floor.overlaps(beside, HALF));
        assert_eq!(floor.sweep(beside + Vec3::Y * 2.0, HALF, Vec3::NEG_Y * 4.0), None);
        assert!(floor.overlaps(Vec3::new(1.0, 0.0, 1.0), HALF));
    }

    #[test]
    fn a_ray_enters_a_triangle_where_it_crosses_it() {
        let wall = Hull::triangle(Vec3::new(3.0, -1.0, -1.0), Vec3::new(3.0, 1.0, -1.0), Vec3::new(3.0, 0.0, 2.0));
        let (distance, normal) = wall.ray(&Ray3d::new(Vec3::ZERO, Dir3::X), 10.0).unwrap();
        assert!((distance - 3.0).abs() < 1e-5);
        assert!(normal.abs_diff_eq(Vec3::NEG_X, 1e-6));
        assert_eq!(wall.ray(&Ray3d::new(Vec3::new(0.0, 0.0, 3.0), Dir3::X), 10.0), None);
    }
}
