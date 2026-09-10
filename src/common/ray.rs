use bevy::prelude::*;

/// How far along the ray it first enters the box, and which face it came in
/// through, or `None` if it misses.
///
/// The slab method. A ray starting *inside* the box returns `0.0` rather than
/// the negative distance back to the entry plane it never crossed: the answer
/// wanted everywhere is "where does this ray first touch the volume", and for
/// an origin already in it that is the origin.
///
/// The normal points *back along* the ray — out of the face that was hit — so
/// it is the vector a bounce reflects about. It is the axis of the last slab
/// the ray entered, which is the face it actually crossed; for an origin
/// inside the box that axis is whichever slab it is furthest into, and the
/// answer is arbitrary rather than wrong. Nothing bounces off a wall it is
/// already inside.
pub fn ray_aabb_entry(ray: &Ray3d, min: Vec3, max: Vec3) -> Option<(f32, Vec3)> {
    let inv_dir = Vec3::new(1.0 / ray.direction.x, 1.0 / ray.direction.y, 1.0 / ray.direction.z);

    // Per axis, the pair of plane crossings, nearest first. `f32::min` and
    // `f32::max` are used rather than comparisons because an axis the ray does
    // not move along divides by zero, and the infinities that produces have to
    // fall out of the test rather than through it.
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

/// How far along the ray it first enters the box, or `None` if it misses.
///
/// [`ray_aabb_entry`] without the face, for the callers — hit registration,
/// shortening a shot against a wall — that only want to know how far.
pub fn ray_aabb_distance(ray: &Ray3d, min: Vec3, max: Vec3) -> Option<f32> {
    ray_aabb_entry(ray, min, max).map(|(distance, _)| distance)
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

    /// The face, not just the distance: a bounce reflects about this, and a
    /// normal taken off the wrong axis sends a pipe bomb through the floor
    /// rather than off it.
    #[test]
    fn the_normal_is_the_face_the_ray_came_in_through() {
        let box_min = Vec3::splat(-1.0);
        let box_max = Vec3::splat(1.0);

        let (_, side) = ray_aabb_entry(&ray(Vec3::new(-5.0, 0.0, 0.0), Vec3::X), box_min, box_max).unwrap();
        assert_eq!(side, Vec3::NEG_X, "came in through the -X face");

        let (_, top) = ray_aabb_entry(&ray(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y), box_min, box_max).unwrap();
        assert_eq!(top, Vec3::Y, "came in through the +Y face");
    }

    /// And it faces the ray rather than away from it, whichever way the ray is
    /// going — the sign is the one thing here that can be silently backwards.
    #[test]
    fn the_normal_always_faces_back_along_the_ray() {
        for direction in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z] {
            let origin = -direction * 5.0;
            let (_, normal) =
                ray_aabb_entry(&ray(origin, direction), Vec3::splat(-1.0), Vec3::splat(1.0)).unwrap();
            assert!(normal.dot(direction) < 0.0, "{direction} entered through {normal}");
        }
    }

    /// An axis the ray does not move along divides by zero, and the slab test
    /// has to survive the infinities that produces.
    #[test]
    fn an_axis_aligned_ray_misses_a_box_it_is_beside() {
        assert_eq!(ray_aabb_distance(&ray(Vec3::new(-5.0, 3.0, 0.0), Vec3::X), Vec3::splat(-1.0), Vec3::splat(1.0)), None);
    }
}
