use bevy::prelude::*;

pub fn ray_intersects_aabb(ray: &Ray3d, min: Vec3, max: Vec3) -> bool {
    let inv_dir = Vec3::new(1.0 / ray.direction.x, 1.0 / ray.direction.y, 1.0 / ray.direction.z);

    let t1 = (min.x - ray.origin.x) * inv_dir.x;
    let t2 = (max.x - ray.origin.x) * inv_dir.x;
    let t3 = (min.y - ray.origin.y) * inv_dir.y;
    let t4 = (max.y - ray.origin.y) * inv_dir.y;
    let t5 = (min.z - ray.origin.z) * inv_dir.z;
    let t6 = (max.z - ray.origin.z) * inv_dir.z;

    let tmin = t1.min(t2).max(t3.min(t4)).max(t5.min(t6));
    let tmax = t1.max(t2).min(t3.max(t4)).min(t5.max(t6));

    tmax >= tmin.max(0.0)
}
