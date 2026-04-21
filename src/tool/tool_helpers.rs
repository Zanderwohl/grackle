use bevy::prelude::*;
use crate::editor::editable::{FeatureId, FeatureTimeline};
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::{CameraAxis, Multicam};
use crate::tool::room::Room;
use crate::tool::show::GizmoVisibility;

pub const PICK_RADIUS: f32 = 0.1;
const SELECT_POINT_RADIUS: f32 = 0.3;

/// Given a camera ray and an infinite line through `axis_origin` in direction
/// `axis_dir`, returns the parameter `s` such that `axis_origin + s * axis_dir`
/// is the closest point on that line to the ray.
/// Returns `None` when the ray is nearly parallel to the axis.
pub fn closest_param_on_axis(ray: Ray3d, axis_origin: Vec3, axis_dir: Vec3) -> Option<f32> {
    let d1 = *ray.direction;
    let d2 = axis_dir;
    let w = ray.origin - axis_origin;
    let a = d1.dot(d1);
    let b = d1.dot(d2);
    let c = d2.dot(d2);
    let d = d1.dot(w);
    let e = d2.dot(w);
    let denom = a * c - b * b;
    if denom.abs() < 1e-6 {
        return None;
    }
    Some((a * e - b * d) / denom)
}

pub fn snap_vec3(v: Vec3, granularity: f32) -> Vec3 {
    Vec3::new(
        f32::ceil(v.x / granularity) * granularity,
        f32::ceil(v.y / granularity) * granularity,
        f32::ceil(v.z / granularity) * granularity,
    )
}

/// Compute a world-space cursor position from the current mouse input.
///
/// For orthographic cameras, projects the ray origin onto the plane defined by the
/// camera axis, using `last_position` for the constrained component.
/// For the perspective camera (`CameraAxis::None`), performs analytical ray-vs-face
/// intersection against all `Room` AABBs and returns the closest hit point whose
/// inward-facing normal faces the camera.
pub fn compute_cursor(
    mouse_input: &CurrentMouseInput,
    cameras: &Query<(Entity, &Multicam)>,
    last_position: Vec3,
    snap: bool,
    snap_granularity: f32,
    rooms: &Query<&Room>,
) -> Option<Vec3> {
    let camera_entity = mouse_input.in_camera?;
    let world_pos = mouse_input.world_pos?;

    for (entity, multicam) in cameras {
        if camera_entity != entity {
            continue;
        }

        let cursor = match multicam.axis {
            CameraAxis::None => {
                return cursor_from_room_faces(&world_pos, rooms, snap, snap_granularity);
            }
            CameraAxis::X => Vec3::new(last_position.x, world_pos.origin.y, world_pos.origin.z),
            CameraAxis::Y => Vec3::new(world_pos.origin.x, last_position.y, world_pos.origin.z),
            CameraAxis::Z => Vec3::new(world_pos.origin.x, world_pos.origin.y, last_position.z),
        };

        return Some(if snap { snap_vec3(cursor, snap_granularity) } else { cursor });
    }

    None
}

/// For each room, test the ray against all 6 axis-aligned faces. Accept only hits
/// whose inward-facing normal points toward the camera (dot(normal, ray_dir) < 0),
/// meaning the face is visible. Return the closest such hit, snapped.
fn cursor_from_room_faces(
    ray: &Ray3d,
    rooms: &Query<&Room>,
    snap: bool,
    snap_granularity: f32,
) -> Option<Vec3> {
    let dir = Vec3::from(ray.direction);
    let mut best_t = f32::MAX;
    let mut best_pos: Option<Vec3> = None;

    for room in rooms.iter() {
        // 6 faces: (fixed axis, fixed value, inward normal)
        let faces: [(usize, f32, Vec3); 6] = [
            (0, room.min.x, Vec3::X),      // -X face, inward normal +X
            (0, room.max.x, Vec3::NEG_X),   // +X face, inward normal -X
            (1, room.min.y, Vec3::Y),        // -Y face (floor), inward normal +Y
            (1, room.max.y, Vec3::NEG_Y),    // +Y face (ceiling), inward normal -Y
            (2, room.min.z, Vec3::Z),        // -Z face, inward normal +Z
            (2, room.max.z, Vec3::NEG_Z),    // +Z face, inward normal -Z
        ];

        for (axis, plane_val, normal) in faces {
            // Must face the camera
            if normal.dot(dir) >= 0.0 {
                continue;
            }

            let dir_component = match axis {
                0 => dir.x,
                1 => dir.y,
                _ => dir.z,
            };

            // Ray parallel to face
            if dir_component.abs() < 1e-8 {
                continue;
            }

            let origin_component = match axis {
                0 => ray.origin.x,
                1 => ray.origin.y,
                _ => ray.origin.z,
            };

            let t = (plane_val - origin_component) / dir_component;
            if t < 0.0 || t >= best_t {
                continue;
            }

            let hit = ray.origin + dir * t;

            // Check the hit point is within the face's 2D bounds
            let in_bounds = match axis {
                0 => hit.y >= room.min.y && hit.y <= room.max.y && hit.z >= room.min.z && hit.z <= room.max.z,
                1 => hit.x >= room.min.x && hit.x <= room.max.x && hit.z >= room.min.z && hit.z <= room.max.z,
                _ => hit.x >= room.min.x && hit.x <= room.max.x && hit.y >= room.min.y && hit.y <= room.max.y,
            };

            if in_bounds {
                best_t = t;
                best_pos = Some(hit);
            }
        }
    }

    best_pos.map(|pos| if snap { snap_vec3(pos, snap_granularity) } else { pos })
}

/// Find the closest reference point to the mouse ray within `pick_radius`.
pub fn find_hovered_point(
    ray: &Ray3d,
    features: &FeatureTimeline,
    pick_radius: f32,
) -> Option<(FeatureId, String, Vec3)> {
    let mut best_dist = pick_radius;
    let mut best: Option<(FeatureId, String, Vec3)> = None;

    for (feature_id, feature) in features.active_features() {
        let points = feature.object().reference_points_for_ray(ray);
        for (key, pos) in points {
            let dist = ray_point_distance(ray, pos);
            if dist < best_dist {
                best_dist = dist;
                best = Some((feature_id, key, pos));
            }
        }
    }

    best
}

/// Draw dim gray spheres for all reference point candidates, with the hovered one highlighted green.
pub fn draw_picking_gizmos(
    gizmos: &mut Gizmos,
    ray: &Ray3d,
    features: &FeatureTimeline,
    hovered: &Option<(FeatureId, String, Vec3)>,
) {
    let dim_color = Color::srgb_u8(200, 200, 200);
    let highlight_color = Color::srgb_u8(0, 230, 0);

    for (feature_id, feature) in features.active_features() {
        let points = feature.object().reference_points_for_ray(ray);
        for (key, pos) in &points {
            let is_hovered = hovered.as_ref()
                .is_some_and(|(hid, hkey, _)| *hid == feature_id && hkey == key);
            let color = if is_hovered { highlight_color } else { dim_color };
            gizmos.sphere(Isometry3d::from_translation(*pos), 0.1, color);
        }
    }
}

/// Like `find_hovered_point` but only considers features whose IDs are in `allowed`.
pub fn find_hovered_point_filtered(
    ray: &Ray3d,
    features: &FeatureTimeline,
    pick_radius: f32,
    allowed: &[FeatureId],
) -> Option<(FeatureId, String, Vec3)> {
    let mut best_dist = pick_radius;
    let mut best: Option<(FeatureId, String, Vec3)> = None;

    for (feature_id, feature) in features.active_features() {
        if !allowed.contains(&feature_id) { continue; }
        let points = feature.object().reference_points_for_ray(ray);
        for (key, pos) in points {
            let dist = ray_point_distance(ray, pos);
            if dist < best_dist {
                best_dist = dist;
                best = Some((feature_id, key, pos));
            }
        }
    }

    best
}

/// Like `draw_picking_gizmos` but only draws for features whose IDs are in `allowed`.
pub fn draw_picking_gizmos_filtered(
    gizmos: &mut Gizmos,
    ray: &Ray3d,
    features: &FeatureTimeline,
    hovered: &Option<(FeatureId, String, Vec3)>,
    allowed: &[FeatureId],
) {
    let dim_color = Color::srgb_u8(200, 200, 200);
    let highlight_color = Color::srgb_u8(0, 230, 0);

    for (feature_id, feature) in features.active_features() {
        if !allowed.contains(&feature_id) { continue; }
        let points = feature.object().reference_points_for_ray(ray);
        for (key, pos) in &points {
            let is_hovered = hovered.as_ref()
                .is_some_and(|(hid, hkey, _)| *hid == feature_id && hkey == key);
            let color = if is_hovered { highlight_color } else { dim_color };
            gizmos.sphere(Isometry3d::from_translation(*pos), 0.1, color);
        }
    }
}

/// Draw a dashed taxicab path (X -> Z -> Y) from `base` to `target`.
pub fn draw_taxicab_path(gizmos: &mut Gizmos, base: Vec3, target: Vec3) {
    const DASH: f32 = 0.15;
    const GAP: f32 = 0.1;

    let d = target - base;
    let segments: [(f32, Vec3, Color); 3] = [
        (d.x, Vec3::X, Color::srgb_u8(255, 80, 80)),
        (d.z, Vec3::Z, Color::srgb_u8(80, 80, 255)),
        (d.y, Vec3::Y, Color::srgb_u8(80, 255, 80)),
    ];

    let mut pos = base;
    for (offset, unit, seg_color) in segments {
        if offset.abs() < f32::EPSILON { continue; }
        let next = pos + unit * offset;
        dashed_line(gizmos, pos, next, seg_color, DASH, GAP);
        pos = next;
    }
}

pub fn ray_point_distance(ray: &Ray3d, point: Vec3) -> f32 {
    let to_point = point - ray.origin;
    let dir = Vec3::from(ray.direction);
    let cross = to_point.cross(dir);
    cross.length() / dir.length()
}

pub fn dashed_line(gizmos: &mut Gizmos, start: Vec3, end: Vec3, color: Color, dash: f32, gap: f32) {
    let dir = end - start;
    let len = dir.length();
    if len < 0.001 { return; }
    let norm = dir / len;
    let mut t = 0.0;
    while t < len {
        let dash_end = (t + dash).min(len);
        gizmos.line(start + norm * t, start + norm * dash_end, color);
        t = dash_end + gap;
    }
}

pub fn bounds_gizmo(gizmos: &mut Gizmos, min: Vec3, max: Vec3, color: Color) {
    gizmos.line(Vec3::new(min.x, min.y, min.z), Vec3::new(max.x, min.y, min.z), color);
    gizmos.line(Vec3::new(max.x, min.y, min.z), Vec3::new(max.x, max.y, min.z), color);
    gizmos.line(Vec3::new(max.x, max.y, min.z), Vec3::new(min.x, max.y, min.z), color);
    gizmos.line(Vec3::new(min.x, max.y, min.z), Vec3::new(min.x, min.y, min.z), color);

    gizmos.line(Vec3::new(min.x, min.y, max.z), Vec3::new(max.x, min.y, max.z), color);
    gizmos.line(Vec3::new(max.x, min.y, max.z), Vec3::new(max.x, max.y, max.z), color);
    gizmos.line(Vec3::new(max.x, max.y, max.z), Vec3::new(min.x, max.y, max.z), color);
    gizmos.line(Vec3::new(min.x, max.y, max.z), Vec3::new(min.x, min.y, max.z), color);

    gizmos.line(Vec3::new(min.x, min.y, min.z), Vec3::new(min.x, min.y, max.z), color);
    gizmos.line(Vec3::new(max.x, min.y, min.z), Vec3::new(max.x, min.y, max.z), color);
    gizmos.line(Vec3::new(max.x, max.y, min.z), Vec3::new(max.x, max.y, max.z), color);
    gizmos.line(Vec3::new(min.x, max.y, min.z), Vec3::new(min.x, max.y, max.z), color);
}

/// Find the nearest visible editor feature hit by a ray.
/// For rooms, tests against all 6 AABB faces (visible ones only).
/// For points/lights, tests ray proximity within SELECT_POINT_RADIUS.
/// Returns the feature ID and hit position of the closest hit across all types.
pub fn find_nearest_feature_hit(
    ray: &Ray3d,
    features: &FeatureTimeline,
    visibility: &GizmoVisibility,
) -> Option<(FeatureId, Vec3)> {
    let dir = Vec3::from(ray.direction);
    let mut best_t = f32::MAX;
    let mut best: Option<(FeatureId, Vec3)> = None;

    for (feature_id, feature) in features.active_features() {
        let key = feature.object().type_key();
        let visible = match key {
            "global_point" => visibility.points,
            "editor_room" => visibility.rooms,
            "grackle_point_light" => visibility.point_lights,
            _ => false,
        };
        if !visible { continue; }

        match key {
            "editor_room" => {
                if let Some((min, max)) = feature.object().drag_handle_bounds() {
                    let faces: [(usize, f32, Vec3); 6] = [
                        (0, min.x, Vec3::X),
                        (0, max.x, Vec3::NEG_X),
                        (1, min.y, Vec3::Y),
                        (1, max.y, Vec3::NEG_Y),
                        (2, min.z, Vec3::Z),
                        (2, max.z, Vec3::NEG_Z),
                    ];

                    for (axis, plane_val, normal) in faces {
                        if normal.dot(dir) >= 0.0 { continue; }

                        let dir_c = match axis { 0 => dir.x, 1 => dir.y, _ => dir.z };
                        if dir_c.abs() < 1e-8 { continue; }

                        let origin_c = match axis { 0 => ray.origin.x, 1 => ray.origin.y, _ => ray.origin.z };
                        let t = (plane_val - origin_c) / dir_c;
                        if t < 0.0 || t >= best_t { continue; }

                        let hit = ray.origin + dir * t;
                        let in_bounds = match axis {
                            0 => hit.y >= min.y && hit.y <= max.y && hit.z >= min.z && hit.z <= max.z,
                            1 => hit.x >= min.x && hit.x <= max.x && hit.z >= min.z && hit.z <= max.z,
                            _ => hit.x >= min.x && hit.x <= max.x && hit.y >= min.y && hit.y <= max.y,
                        };

                        if in_bounds {
                            best_t = t;
                            best = Some((feature_id, hit));
                        }
                    }
                }
            }
            "global_point" | "grackle_point_light" => {
                if let Ok(pos) = feature.object().get_point("") {
                    let dist = ray_point_distance(ray, pos);
                    if dist < SELECT_POINT_RADIUS {
                        let t = (pos - ray.origin).dot(dir);
                        if t > 0.0 && t < best_t {
                            best_t = t;
                            best = Some((feature_id, pos));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    best
}
