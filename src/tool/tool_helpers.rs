use bevy::prelude::*;
use crate::common::ray::ray_intersects_aabb;
use crate::common::world_surface;
use crate::editor::editable::{FeatureId, FeatureTimeline};
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::{CameraAxis, Multicam};
use crate::tool::room::Room;
use crate::tool::show::GizmoVisibility;

pub const PICK_RADIUS: f32 = 0.1;

/// Where a relatively placed point lands this frame: at `cursor` on a click
/// or `Enter`, on the reference itself on `Escape` or a right click (not the
/// end of a camera turn), or not yet.
pub fn relative_commit(
    mouse: &CurrentMouseInput,
    keys: &ButtonInput<KeyCode>,
    typing: bool,
    cursor: Option<Vec3>,
    reference: Vec3,
) -> Option<Vec3> {
    if (!typing && keys.just_pressed(KeyCode::Escape)) || (mouse.in_camera.is_some() && mouse.clicked(MouseButton::Right)) {
        Some(reference)
    } else if commit_at_cursor(mouse, keys, typing) {
        cursor
    } else {
        None
    }
}

/// The camera is being turned or panned. Shift is its speed boost then, so a
/// Shift held through one is not a placement tool's Shift.
pub fn camera_dragging(mouse: &CurrentMouseInput) -> bool {
    matches!(mouse.pressed, Some(MouseButton::Right | MouseButton::Middle))
}

/// A click, or `Enter` while no text field has the keyboard.
pub fn commit_at_cursor(mouse: &CurrentMouseInput, keys: &ButtonInput<KeyCode>, typing: bool) -> bool {
    let enter = keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter);
    mouse.released == Some(MouseButton::Left) || (!typing && enter)
}
const SELECT_POINT_RADIUS: f32 = 0.3;

/// The `s` for which `axis_origin + s * axis_dir` is the point on that line
/// closest to the ray. `None` when the ray is nearly parallel to the axis.
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

/// The signed angle of `point` about `center` on the plane spanned by
/// `cos_dir` and `sin_dir` (unit, mutually perpendicular), right-handed about
/// the normal those two imply.
pub fn angle_on_plane(point: Vec3, center: Vec3, cos_dir: Vec3, sin_dir: Vec3) -> f32 {
    let v = point - center;
    v.dot(sin_dir).atan2(v.dot(cos_dir))
}

/// Snap each axis to the nearest multiple of `granularity` (ties round half away from zero).
pub fn snap_vec3(v: Vec3, granularity: f32) -> Vec3 {
    if granularity <= 0.0 {
        return v;
    }
    let inv = 1.0 / granularity;
    Vec3::new(
        (v.x * inv).round() * granularity,
        (v.y * inv).round() * granularity,
        (v.z * inv).round() * granularity,
    )
}

/// A world-space cursor position from the current mouse input.
///
/// Orthographic views take the constrained component from `last_position`.
/// The perspective view uses where the ray first meets solid wall from inside
/// a room.
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
    let (_, multicam) = cameras.get(camera_entity).ok()?;

    let cursor = match multicam.axis {
        CameraAxis::None => {
            return cursor_from_room_faces(&world_pos, rooms, snap, snap_granularity);
        }
        CameraAxis::X => Vec3::new(last_position.x, world_pos.origin.y, world_pos.origin.z),
        CameraAxis::Y => Vec3::new(world_pos.origin.x, last_position.y, world_pos.origin.z),
        CameraAxis::Z => Vec3::new(world_pos.origin.x, world_pos.origin.y, last_position.z),
    };

    Some(if snap { snap_vec3(cursor, snap_granularity) } else { cursor })
}

/// Where the ray first meets solid wall from inside any room, snapped, so a
/// cursor goes through a doorway.
fn cursor_from_room_faces(
    ray: &Ray3d,
    rooms: &Query<&Room>,
    snap: bool,
    snap_granularity: f32,
) -> Option<Vec3> {
    let rooms: Vec<Room> = rooms.iter().cloned().collect();
    nearest_wall(&rooms, ray)
        .map(|(_, pos)| if snap { snap_vec3(pos, snap_granularity) } else { pos })
}

/// The nearest solid wall `ray` meets from inside any of `rooms`, each room
/// carved by all the others.
fn nearest_wall(rooms: &[Room], ray: &Ray3d) -> Option<(f32, Vec3)> {
    (0..rooms.len())
        .filter_map(|index| room_wall_hit(rooms, index, ray))
        .min_by(|(a, _), (b, _)| a.total_cmp(b))
}

/// Where `ray` meets `rooms[index]`'s share of the surface from inside it. A
/// room's share lies on its box, so a box the ray misses is not carved at all.
fn room_wall_hit(rooms: &[Room], index: usize, ray: &Ray3d) -> Option<(f32, Vec3)> {
    let (min, max) = rooms[index].bounds();
    if !ray_intersects_aabb(ray, min, max) {
        return None;
    }
    world_surface::ray_from_inside(&world_surface::room_share(rooms, index), ray)
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

/// The nearest visible feature hit by a ray, as its id and the hit position.
/// Rooms are tested against solid wall seen from inside, so a room never hides
/// what stands in it; everything else by pick boxes and by proximity to its
/// point.
pub fn find_nearest_feature_hit(
    ray: &Ray3d,
    features: &FeatureTimeline,
    visibility: &GizmoVisibility,
) -> Option<(FeatureId, Vec3)> {
    let dir = Vec3::from(ray.direction);
    let mut best_t = f32::MAX;
    let mut best: Option<(FeatureId, Vec3)> = None;

    // Hidden rooms too: they still carve openings into their neighbours. A
    // Mirror's copies are unpickable, but they carve.
    let (owners, rooms): (Vec<Option<FeatureId>>, Vec<Room>) = features
        .active_features()
        .flat_map(|(id, feature)| {
            let own = feature.object().rooms().into_iter().map(move |room| (Some(id), room));
            let copies = feature.object().standing_copies().into_iter().flat_map(|copy| copy.rooms()).map(|room| (None, room));
            own.chain(copies).collect::<Vec<_>>()
        })
        .unzip();

    for (feature_id, feature) in features.active_features() {
        let key = feature.object().type_key();
        let visible = match key {
            "global_point" => visibility.points,
            "editor_room" | "footprint_room" => visibility.rooms,
            "grackle_point_light" => visibility.point_lights,
            "spawn_point" => visibility.spawn_points,
            "prop" => visibility.props,
            "animation_display" => visibility.animation_displays,
            "animation_grid" => visibility.animation_grids,
            "mirror" => visibility.mirrors,
            _ => false,
        };
        if !visible { continue; }

        match key {
            "editor_room" | "footprint_room" => {
                let own = owners.iter().enumerate().filter(|(_, owner)| **owner == Some(feature_id));
                for (index, _) in own {
                    if let Some((t, hit)) = room_wall_hit(&rooms, index, ray) {
                        if t < best_t {
                            best_t = t;
                            best = Some((feature_id, hit));
                        }
                    }
                }
            }
            "global_point" | "grackle_point_light" | "spawn_point" | "prop" | "animation_display" | "animation_grid" | "mirror" => {
                // Both: a grid's point is between bodies, inside none.
                for pick_box in feature.object().pick_boxes() {
                    if let Some(t) = pick_box.ray_distance(ray) {
                        if t < best_t {
                            best_t = t;
                            best = Some((feature_id, ray.origin + dir * t));
                        }
                    }
                }
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
