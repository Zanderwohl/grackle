use bevy::app::App;
use bevy::prelude::*;
use crate::editor::editable::{EditorActionId, EditorActions, PointRef};
use crate::editor::global_point::GlobalPoint;
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::tool::room::Room;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

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
        rooms: Query<&Room>,
        mut next_tool: ResMut<NextState<Tools>>,
    ) {
        let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let shift_just_pressed = keys.just_pressed(KeyCode::ShiftLeft) || keys.just_pressed(KeyCode::ShiftRight);

        tool.cursor = compute_cursor(
            &mouse_input, &cameras, tool.last_position,
            tool.snap, tool.snap_granularity, &rooms,
        );

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
                        next_tool.set(Tools::Select);
                    }
                }
            }
            PointToolMode::Picking => {
                if !shift_held {
                    tool.mode = PointToolMode::Normal;
                    tool.hovered_point = None;
                    return;
                }

                tool.hovered_point = mouse_input.world_pos
                    .and_then(|ray| find_hovered_point(&ray, &actions, PICK_RADIUS));

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
                            next_tool.set(Tools::Select);
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
        mut gizmos: Gizmos,
    ) {
        if let Some(cursor) = tool.cursor {
            let color = match tool.mode {
                PointToolMode::RelativeSelected => Color::srgb_u8(80, 140, 255),
                _ => Color::srgb_u8(60, 120, 255),
            };
            gizmos.sphere(Isometry3d::from_translation(cursor), 0.15, color);

            if tool.mode == PointToolMode::RelativeSelected {
                if let Some(base) = tool.reference_resolved {
                    draw_taxicab_path(&mut gizmos, base, cursor);
                }
            }
        }

        if tool.mode == PointToolMode::Picking {
            if let Some(ray) = mouse_input.world_pos {
                draw_picking_gizmos(&mut gizmos, &ray, &actions, &tool.hovered_point);
            }
        }
    }
}
