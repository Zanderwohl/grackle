use bevy::app::App;
use bevy::prelude::*;
use crate::editor::editable::{EditorActionId, EditorActions, PointRef};
use crate::editor::grackle_point_light::GracklePointLight;
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::{CameraAxis, Multicam};
use crate::tool::Tools;

const PICK_RADIUS: f32 = 0.1;
const DEFAULT_SNAP_GRANULARITY: f32 = 0.1;

#[derive(PartialEq, Eq, Clone, Copy)]
enum PointLightToolMode {
    Normal,
    Picking,
    RelativeSelected,
}

#[derive(Resource)]
struct PointLightTool {
    mode: PointLightToolMode,
    last_position: Vec3,
    cursor: Option<Vec3>,
    reference_action: Option<EditorActionId>,
    reference_key: String,
    reference_resolved: Option<Vec3>,
    hovered_point: Option<(EditorActionId, String, Vec3)>,
    snap: bool,
    snap_granularity: f32,
}

impl Default for PointLightTool {
    fn default() -> Self {
        Self {
            mode: PointLightToolMode::Normal,
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

pub struct PointLightPlugin;

impl Plugin for PointLightPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<PointLightTool>()
            .add_systems(Update, (
                PointLightTool::interface,
                PointLightTool::draw_gizmos,
            ).chain().run_if(in_state(Tools::PointLight)))
            .add_systems(OnExit(Tools::PointLight), PointLightTool::on_exit)
        ;
    }
}

impl PointLightTool {
    fn on_exit(mut tool: ResMut<Self>) {
        tool.mode = PointLightToolMode::Normal;
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

        match tool.mode {
            PointLightToolMode::Normal => {
                if shift_held {
                    tool.mode = PointLightToolMode::Picking;
                    tool.hovered_point = None;
                } else if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        let light = GracklePointLight::new(cursor.x, cursor.y, cursor.z);
                        let id = actions.take_action(Box::new(light));
                        actions.select(Some(id));
                        tool.last_position = cursor;
                    }
                }
            }
            PointLightToolMode::Picking => {
                if !shift_held {
                    tool.mode = PointLightToolMode::Normal;
                    tool.hovered_point = None;
                    return;
                }

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
                        tool.mode = PointLightToolMode::RelativeSelected;
                    }
                }
            }
            PointLightToolMode::RelativeSelected => {
                if shift_just_pressed {
                    tool.mode = PointLightToolMode::Normal;
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
                            let light = GracklePointLight::from_point_ref(pr);
                            let id = actions.take_action(Box::new(light));
                            actions.select(Some(id));
                            tool.last_position = cursor;
                        }
                    }
                }
            }
        }
    }

    fn draw_gizmos(
        tool: Res<PointLightTool>,
        actions: Res<EditorActions>,
        mouse_input: Res<CurrentMouseInput>,
        mut gizmos: Gizmos,
    ) {
        if let Some(cursor) = tool.cursor {
            let color = match tool.mode {
                PointLightToolMode::RelativeSelected => Color::srgb_u8(0, 220, 220),
                _ => Color::srgb_u8(255, 200, 50),
            };
            gizmos.sphere(Isometry3d::from_translation(cursor), 0.15, color);

            if tool.mode == PointLightToolMode::RelativeSelected {
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

        if tool.mode == PointLightToolMode::Picking {
            if let Some(ray) = mouse_input.world_pos {
                let dim_color = Color::srgba(0.5, 0.5, 0.5, 0.4);
                let highlight_color = Color::srgb_u8(180, 240, 255);

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
