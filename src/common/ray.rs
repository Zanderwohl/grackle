use bevy::prelude::*;

/// Distance along the ray to where it first enters the box, and the face it
/// came in through, or `None` on a miss. Slab method.
///
/// An origin inside the box returns `0.0`. The normal points back along the
/// ray (what a bounce reflects about); for an origin inside, its axis is
/// arbitrary.
pub fn ray_aabb_entry(ray: &Ray3d, min: Vec3, max: Vec3) -> Option<(f32, Vec3)> {
    let inv_dir = Vec3::new(1.0 / ray.direction.x, 1.0 / ray.direction.y, 1.0 / ray.direction.z);

    // `min`/`max` rather than comparisons so the infinities from an axis the
    // ray does not move along fall out of the test rather than through it.
    let mut entry = f32::NEG_INFINITY;
    let mut exit = f32::INFINITY;
    let mut axis = 0;
    for a in 0..3 {
        let t1 = (min[a] - ray.origin[a]) * inv_dir[a];
        let t2 = (max[a] - ray.origin[a]) * inv_dir[a];
        let near = t1.min(t2);
        if near > entry {
            entry = near;
            axis = a;
        }
        exit = exit.min(t1.max(t2));
    }

    if exit < entry.max(0.0) {
        return None;
    }

    let mut normal = Vec3::ZERO;
    normal[axis] = if ray.direction[axis] > 0.0 { -1.0 } else { 1.0 };
    Some((entry.max(0.0), normal))
}

/// [`ray_aabb_entry`] without the face.
pub fn ray_aabb_distance(ray: &Ray3d, min: Vec3, max: Vec3) -> Option<f32> {
    ray_aabb_entry(ray, min, max).map(|(distance, _)| distance)
}

/// A box turned in the world, for picking a feature by its body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickBox {
    pub center: Vec3,
    pub rotation: Quat,
    pub half_extents: Vec3,
}

impl PickBox {
    /// The movement hull of a body with its feet at `feet`, from the same
    /// constants spawn points are checked with, rather than a pose that changes
    /// every frame.
    pub fn standing_body(feet: Vec3, rotation: Quat) -> Self {
        Self {
            center: crate::common::class::body_center_from_feet(feet),
            rotation,
            half_extents: crate::common::class::CLASS_HALF_EXTENTS,
        }
    }

    /// Takes the ray into the box's frame; a rotation keeps the direction unit
    /// length, so distances are unchanged.
    pub fn ray_distance(&self, ray: &Ray3d) -> Option<f32> {
        let inverse = self.rotation.inverse();
        let local = Ray3d::new(
            inverse * (ray.origin - self.center),
            Dir3::new_unchecked((inverse * *ray.direction).normalize()),
        );
        ray_aabb_distance(&local, -self.half_extents, self.half_extents)
    }

    /// As a transform on a unit cube, for `Gizmos::cube`.
    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.center)
            .with_rotation(self.rotation)
            .with_scale(self.half_extents * 2.0)
    }
}

pub fn ray_intersects_aabb(ray: &Ray3d, min: Vec3, max: Vec3) -> bool {
    ray_aabb_distance(ray, min, max).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ray(origin: Vec3, direction: Vec3) -> Ray3d {
        Ray3d::new(origin, Dir3::new(direction).unwrap())
    }

    #[test]
    fn the_distance_is_to_the_near_face_not_the_center() {
        let hit = ray_aabb_distance(&ray(Vec3::new(-5.0, 0.0, 0.0), Vec3::X), Vec3::splat(-1.0), Vec3::splat(1.0));
        assert!((hit.unwrap() - 4.0).abs() < 1e-5, "{hit:?}");
    }

    #[test]
    fn a_ray_starting_inside_hits_at_zero() {
        let hit = ray_aabb_distance(&ray(Vec3::ZERO, Vec3::X), Vec3::splat(-1.0), Vec3::splat(1.0));
        assert_eq!(hit, Some(0.0));
    }

    #[test]
    fn a_box_behind_the_ray_is_a_miss() {
        assert_eq!(ray_aabb_distance(&ray(Vec3::new(5.0, 0.0, 0.0), Vec3::X), Vec3::splat(-1.0), Vec3::splat(1.0)), None);
    }

    /// A normal off the wrong axis sends a pipe bomb through the floor.
    #[test]
    fn the_normal_is_the_face_the_ray_came_in_through() {
        let box_min = Vec3::splat(-1.0);
        let box_max = Vec3::splat(1.0);

        let (_, side) = ray_aabb_entry(&ray(Vec3::new(-5.0, 0.0, 0.0), Vec3::X), box_min, box_max).unwrap();
        assert_eq!(side, Vec3::NEG_X, "came in through the -X face");

        let (_, top) = ray_aabb_entry(&ray(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y), box_min, box_max).unwrap();
        assert_eq!(top, Vec3::Y, "came in through the +Y face");
    }

    /// The sign is the part that can be silently backwards.
    #[test]
    fn the_normal_always_faces_back_along_the_ray() {
        for direction in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z] {
            let origin = -direction * 5.0;
            let (_, normal) =
                ray_aabb_entry(&ray(origin, direction), Vec3::splat(-1.0), Vec3::splat(1.0)).unwrap();
            assert!(normal.dot(direction) < 0.0, "{direction} entered through {normal}");
        }
    }

    #[test]
    fn a_turned_box_is_hit_where_it_is_turned_to() {
        let long_along_z = PickBox {
            center: Vec3::ZERO,
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            half_extents: Vec3::new(0.1, 0.1, 2.0),
        };
        let hit = long_along_z.ray_distance(&ray(Vec3::new(1.5, 0.0, -5.0), Vec3::Z));
        assert!((hit.unwrap() - 4.9).abs() < 1e-4, "{hit:?}");
        assert_eq!(long_along_z.ray_distance(&ray(Vec3::new(-5.0, 0.0, 1.5), Vec3::X)), None);
    }

    /// A zero direction component divides by zero; the test must survive it.
    #[test]
    fn an_axis_aligned_ray_misses_a_box_it_is_beside() {
        assert_eq!(ray_aabb_distance(&ray(Vec3::new(-5.0, 3.0, 0.0), Vec3::X), Vec3::splat(-1.0), Vec3::splat(1.0)), None);
    }
}
