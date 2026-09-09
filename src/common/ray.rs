use bevy::prelude::*;

/// How far along the ray it first enters the box, or `None` if it misses.
///
/// The slab method. A ray starting *inside* the box returns `0.0` rather than
/// the negative distance back to the entry plane it never crossed: the answer
/// wanted everywhere is "where does this ray first touch the volume", and for
/// an origin already in it that is the origin.
pub fn ray_aabb_distance(ray: &Ray3d, min: Vec3, max: Vec3) -> Option<f32> {
    let inv_dir = Vec3::new(1.0 / ray.direction.x, 1.0 / ray.direction.y, 1.0 / ray.direction.z);

    let t1 = (min.x - ray.origin.x) * inv_dir.x;
    let t2 = (max.x - ray.origin.x) * inv_dir.x;
    let t3 = (min.y - ray.origin.y) * inv_dir.y;
    let t4 = (max.y - ray.origin.y) * inv_dir.y;
    let t5 = (min.z - ray.origin.z) * inv_dir.z;
    let t6 = (max.z - ray.origin.z) * inv_dir.z;

    let tmin = t1.min(t2).max(t3.min(t4)).max(t5.min(t6));
    let tmax = t1.max(t2).min(t3.max(t4)).min(t5.max(t6));

    (tmax >= tmin.max(0.0)).then(|| tmin.max(0.0))
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
    fn the_distance_is_to_the_near_face_not_the_centre() {
        let hit = ray_aabb_distance(&ray(Vec3::new(-5.0, 0.0, 0.0), Vec3::X), Vec3::splat(-1.0), Vec3::splat(1.0));
        assert!((hit.unwrap() - 4.0).abs() < 1e-5, "{hit:?}");
    }

    /// The one case the bool version could not express: a ray that starts in
    /// the box has not travelled to reach it.
    #[test]
    fn a_ray_starting_inside_hits_at_zero() {
        let hit = ray_aabb_distance(&ray(Vec3::ZERO, Vec3::X), Vec3::splat(-1.0), Vec3::splat(1.0));
        assert_eq!(hit, Some(0.0));
    }

    #[test]
    fn a_box_behind_the_ray_is_a_miss() {
        assert_eq!(ray_aabb_distance(&ray(Vec3::new(5.0, 0.0, 0.0), Vec3::X), Vec3::splat(-1.0), Vec3::splat(1.0)), None);
    }

    /// An axis the ray does not move along divides by zero, and the slab test
    /// has to survive the infinities that produces.
    #[test]
    fn an_axis_aligned_ray_misses_a_box_it_is_beside() {
        assert_eq!(ray_aabb_distance(&ray(Vec3::new(-5.0, 3.0, 0.0), Vec3::X), Vec3::splat(-1.0), Vec3::splat(1.0)), None);
    }
}
