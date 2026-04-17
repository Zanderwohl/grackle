use bevy::app::App;
use bevy::prelude::*;
use crate::editor::editable::{EditorActionId, EditorActions, PointRef};
use crate::editor::global_point::GlobalPoint;
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::{CameraAxis, Multicam};
use crate::tool::Tools;

const PICK_RADIUS: f32 = 0.1;
const DEFAULT_SNAP_GRANULARITY: f32 = 0.1;

#[derive(PartialEq, Eq, Clone, Copy)]
enum PointToolMode {
    Normal,
    Picking,
    RelativeSelected,
}

#[derive(Resource)]
struct PointTool {
    mode: PointToolMode,
    last_position: Vec3,
    cursor: Option<Vec3>,
    reference_action: Option<EditorActionId>,
    reference_key: String,
    reference_resolved: Option<Vec3>,
    hovered_point: Option<(EditorActionId, String, Vec3)>,
    snap: bool,
    snap_granularity: f32,
}

impl Default for PointTool {
    fn default() -> Self {
        Self {
            mode: PointToolMode::Normal,
            last_position: Vec3::ZERO,
            cursor: None,
            reference_action: None,
            reference_key: String::new(),
            reference_resolved: None,
            hovered_point: None,
            snap: true,
            snap_granularity: DEFAULT_SNAP_GRANULARITY,
        }
    }
}

pub struct PointPlugin;

impl Plugin for PointPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<PointTool>()
            .add_systems(Update, (
                PointTool::interface,
                PointTool::draw_gizmos,
            ).chain().run_if(in_state(Tools::Point)))
            .add_systems(OnExit(Tools::Point), PointTool::on_exit)
        ;
    }
}

impl PointTool {
    fn on_exit(mut tool: ResMut<Self>) {
        tool.mode = PointToolMode::Normal;
        tool.cursor = None;
        tool.hovered_point = None;
        tool.reference_action = None;
        tool.reference_key.clear();
        tool.reference_resolved = None;
    }

    fn interface(
        mut tool: ResMut<Self>,
        cameras: Query<(Entity, &Multicam)>,
        mouse_input: Res<CurrentMouseInput>,
        keys: Res<ButtonInput<KeyCode>>,
        mut actions: ResMut<EditorActions>,
    ) {
        let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let shift_just_pressed = keys.just_pressed(KeyCode::ShiftLeft) || keys.just_pressed(KeyCode::ShiftRight);

        // Cursor computation (same pattern as RoomTool)
        tool.cursor = None;
        if let Some(camera_entity) = mouse_input.in_camera {
            if let Some(world_pos) = mouse_input.world_pos {
                for (entity, multicam) in &cameras {
                    if camera_entity == entity && multicam.axis != CameraAxis::None {
                        let origin = world_pos.origin;
                        let cursor = match multicam.axis {
                            CameraAxis::None => unreachable!(),
                            CameraAxis::X => Vec3::new(tool.last_position.x, origin.y, origin.z),
                            CameraAxis::Y => Vec3::new(origin.x, tool.last_position.y, origin.z),
                            CameraAxis::Z => Vec3::new(origin.x, origin.y, tool.last_position.z),
                        };
                        let cursor = if tool.snap {
                            let g = tool.snap_granularity;
                            Vec3::new(
                                f32::ceil(cursor.x / g) * g,
                                f32::ceil(cursor.y / g) * g,
                                f32::ceil(cursor.z / g) * g,
                            )
                        } else {
                            cursor
                        };
                        tool.cursor = Some(cursor);
                    }
                }
            }
        }

        // Mode FSM
        match tool.mode {
            PointToolMode::Normal => {
                if shift_held {
                    tool.mode = PointToolMode::Picking;
                    tool.hovered_point = None;
                } else if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        let point = GlobalPoint::new(cursor.x, cursor.y, cursor.z);
                        let id = actions.take_action(Box::new(point));
                        actions.select(Some(id));
                        tool.last_position = cursor;
                    }
                }
            }
            PointToolMode::Picking => {
                if !shift_held {
                    tool.mode = PointToolMode::Normal;
                    tool.hovered_point = None;
                    return;
                }

                // Find closest reference point to mouse ray
                tool.hovered_point = None;
                if let Some(ray) = mouse_input.world_pos {
                    let mut best_dist = PICK_RADIUS;
                    let mut best: Option<(EditorActionId, String, Vec3)> = None;

                    for (action_id, action) in actions.active_actions() {
                        let points = action.object().reference_points_for_ray(&ray);
                        for (key, pos) in points {
                            let dist = ray_point_distance(&ray, pos);
                            if dist < best_dist {
                                best_dist = dist;
                                best = Some((action_id, key, pos));
                            }
                        }
                    }
                    tool.hovered_point = best;
                }

                if mouse_input.released == Some(MouseButton::Left) {
                    if let Some((action_id, key, resolved)) = tool.hovered_point.take() {
                        tool.reference_action = Some(action_id);
                        tool.reference_key = key;
                        tool.reference_resolved = Some(resolved);
                        tool.mode = PointToolMode::RelativeSelected;
                    }
                }
            }
            PointToolMode::RelativeSelected => {
                if shift_just_pressed {
                    tool.mode = PointToolMode::Normal;
                    tool.reference_action = None;
                    tool.reference_key.clear();
                    tool.reference_resolved = None;
                    return;
                }

                if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        if let (Some(ref_action), Some(ref_resolved)) = (tool.reference_action, tool.reference_resolved) {
                            let d = cursor - ref_resolved;
                            let mut pr = PointRef::reference_with_offset(ref_action, d.x, d.y, d.z);
                            if !tool.reference_key.is_empty() {
                                pr.point_key = tool.reference_key.clone();
                            }
                            let point = GlobalPoint::from_point_ref(pr);
                            let id = actions.take_action(Box::new(point));
                            actions.select(Some(id));
                            tool.last_position = cursor;
                        }
                    }
                }
            }
        }
    }

    fn draw_gizmos(
        tool: Res<PointTool>,
        actions: Res<EditorActions>,
        mouse_input: Res<CurrentMouseInput>,
        keys: Res<ButtonInput<KeyCode>>,
        mut gizmos: Gizmos,
    ) {
        // Preview sphere at cursor
        if let Some(cursor) = tool.cursor {
            let color = match tool.mode {
                PointToolMode::RelativeSelected => Color::srgb_u8(80, 140, 255),
                _ => Color::srgb_u8(60, 120, 255),
            };
            gizmos.sphere(Isometry3d::from_translation(cursor), 0.15, color);

            // In RelativeSelected mode, draw dashed taxicab path from reference to cursor
            if tool.mode == PointToolMode::RelativeSelected {
                if let Some(base) = tool.reference_resolved {
                    const DASH: f32 = 0.15;
                    const GAP: f32 = 0.1;

                    let d = cursor - base;
                    let segments: [(f32, Vec3, Color); 3] = [
                        (d.x, Vec3::X, Color::srgb_u8(255, 80, 80)),
                        (d.z, Vec3::Z, Color::srgb_u8(80, 80, 255)),
                        (d.y, Vec3::Y, Color::srgb_u8(80, 255, 80)),
                    ];

                    let mut pos = base;
                    for (offset, unit, seg_color) in segments {
                        if offset.abs() < f32::EPSILON { continue; }
                        let next = pos + unit * offset;
                        dashed_line(&mut gizmos, pos, next, seg_color, DASH, GAP);
                        pos = next;
                    }
                }
            }
        }

        // In Picking mode, draw all reference point candidates
        if tool.mode == PointToolMode::Picking {
            if let Some(ray) = mouse_input.world_pos {
                let dim_color = Color::srgb_u8(200, 200, 200);
                let highlight_color = Color::srgb_u8(0, 230, 0);

                for (action_id, action) in actions.active_actions() {
                    let points = action.object().reference_points_for_ray(&ray);
                    for (key, pos) in &points {
                        let is_hovered = tool.hovered_point.as_ref()
                            .is_some_and(|(hid, hkey, _)| *hid == action_id && hkey == key);
                        let color = if is_hovered { highlight_color } else { dim_color };
                        gizmos.sphere(Isometry3d::from_translation(*pos), 0.1, color);
                    }
                }
            }
        }
    }
}

fn ray_point_distance(ray: &Ray3d, point: Vec3) -> f32 {
    let to_point = point - ray.origin;
    let dir = Vec3::from(ray.direction);
    let cross = to_point.cross(dir);
    cross.length() / dir.length()
}

fn dashed_line(gizmos: &mut Gizmos, start: Vec3, end: Vec3, color: Color, dash: f32, gap: f32) {
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
