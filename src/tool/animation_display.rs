//! Places [`AnimationDisplay`] features.
//!
//! The point tool with a different feature at the end of it, the same way the
//! spawn point and prop tools are: placing a display *is* placing a point, so
//! relative placement and shift-picking work here without a mapper learning
//! anything new. Which state it holds is chosen afterwards, in the feature
//! panel, and which way it faces from the rotation rings in
//! [`crate::tool::rotate_drag`].

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::skeleton::{default_humanoid, draw_skeleton, Pose, SkeletonPalette};
use crate::editor::animation_display::AnimationDisplay;
use crate::editor::editable::{FeatureId, FeatureTimeline, PointRef};
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::tool::room::Room;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

const DEFAULT_SNAP_GRANULARITY: f32 = 0.1;

#[derive(PartialEq, Eq, Clone, Copy)]
enum AnimationDisplayToolMode {
    Normal,
    Picking,
    RelativeSelected,
}

#[derive(Resource)]
struct AnimationDisplayTool {
    mode: AnimationDisplayToolMode,
    last_position: Vec3,
    cursor: Option<Vec3>,
    reference_feature: Option<FeatureId>,
    reference_key: String,
    reference_resolved: Option<Vec3>,
    hovered_point: Option<(FeatureId, String, Vec3)>,
    snap: bool,
    snap_granularity: f32,
}

impl Default for AnimationDisplayTool {
    fn default() -> Self {
        Self {
            mode: AnimationDisplayToolMode::Normal,
            last_position: Vec3::ZERO,
            cursor: None,
            reference_feature: None,
            reference_key: String::new(),
            reference_resolved: None,
            hovered_point: None,
            snap: true,
            snap_granularity: DEFAULT_SNAP_GRANULARITY,
        }
    }
}

pub struct AnimationDisplayPlugin;

impl Plugin for AnimationDisplayPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<AnimationDisplayTool>()
            .add_systems(Update, (
                AnimationDisplayTool::interface,
                AnimationDisplayTool::draw_gizmos,
            ).chain().run_if(in_state(Tools::AnimationDisplay)).run_if(in_state(AppMode::Editor)))
            .add_systems(OnExit(Tools::AnimationDisplay), AnimationDisplayTool::on_exit)
        ;
    }
}

impl AnimationDisplayTool {
    fn on_exit(mut tool: ResMut<Self>) {
        tool.mode = AnimationDisplayToolMode::Normal;
        tool.cursor = None;
        tool.hovered_point = None;
        tool.reference_feature = None;
        tool.reference_key.clear();
        tool.reference_resolved = None;
    }

    fn interface(
        mut tool: ResMut<Self>,
        cameras: Query<(Entity, &Multicam)>,
        mouse_input: Res<CurrentMouseInput>,
        keys: Res<ButtonInput<KeyCode>>,
        mut features: ResMut<FeatureTimeline>,
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
            AnimationDisplayToolMode::Normal => {
                if shift_held {
                    tool.mode = AnimationDisplayToolMode::Picking;
                    tool.hovered_point = None;
                } else if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        let display = AnimationDisplay::new(cursor.x, cursor.y, cursor.z);
                        let id = features.apply_feature(Box::new(display));
                        features.select(Some(id));
                        tool.last_position = cursor;
                        next_tool.set(Tools::Select);
                    }
                }
            }
            AnimationDisplayToolMode::Picking => {
                if !shift_held {
                    tool.mode = AnimationDisplayToolMode::Normal;
                    tool.hovered_point = None;
                    return;
                }

                tool.hovered_point = mouse_input.world_pos
                    .and_then(|ray| find_hovered_point(&ray, &features, PICK_RADIUS));

                if mouse_input.released == Some(MouseButton::Left) {
                    if let Some((feature_id, key, resolved)) = tool.hovered_point.take() {
                        tool.reference_feature = Some(feature_id);
                        tool.reference_key = key;
                        tool.reference_resolved = Some(resolved);
                        tool.mode = AnimationDisplayToolMode::RelativeSelected;
                    }
                }
            }
            AnimationDisplayToolMode::RelativeSelected => {
                if shift_just_pressed {
                    tool.mode = AnimationDisplayToolMode::Normal;
                    tool.reference_feature = None;
                    tool.reference_key.clear();
                    tool.reference_resolved = None;
                    return;
                }

                if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        if let (Some(ref_feature), Some(ref_resolved)) = (tool.reference_feature, tool.reference_resolved) {
                            let d = cursor - ref_resolved;
                            let mut pr = PointRef::reference_with_offset(ref_feature, d.x, d.y, d.z);
                            if !tool.reference_key.is_empty() {
                                pr.point_key = tool.reference_key.clone();
                            }
                            let display = AnimationDisplay::from_point_ref(pr);
                            let id = features.apply_feature(Box::new(display));
                            features.select(Some(id));
                            tool.last_position = cursor;
                            next_tool.set(Tools::Select);
                        }
                    }
                }
            }
        }
    }

    fn draw_gizmos(
        tool: Res<AnimationDisplayTool>,
        features: Res<FeatureTimeline>,
        mouse_input: Res<CurrentMouseInput>,
        mut gizmos: Gizmos,
    ) {
        if let Some(cursor) = tool.cursor {
            let colour = match tool.mode {
                AnimationDisplayToolMode::RelativeSelected => Color::srgb_u8(220, 190, 255),
                _ => Color::srgb_u8(190, 140, 255),
            };
            // The body that would land here, at the size it would land at.
            // A display is placed to be looked at from somewhere, so its
            // footprint and height matter before it is committed, not after.
            draw_skeleton(
                &mut gizmos,
                default_humanoid(),
                &Pose::rest(),
                &Transform::from_translation(cursor),
                &SkeletonPalette::flat(colour),
            );

            if tool.mode == AnimationDisplayToolMode::RelativeSelected {
                if let Some(base) = tool.reference_resolved {
                    draw_taxicab_path(&mut gizmos, base, cursor);
                }
            }
        }

        if tool.mode == AnimationDisplayToolMode::Picking {
            if let Some(ray) = mouse_input.world_pos {
                draw_picking_gizmos(&mut gizmos, &ray, &features, &tool.hovered_point);
            }
        }
    }
}
