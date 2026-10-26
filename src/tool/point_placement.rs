//! The shared state machine behind every point-placing tool: hover to
//! preview, click to commit, or hold shift to pick a reference point and place
//! relative to it. A tool supplies only a [`PlaceablePoint`].

use std::marker::PhantomData;

use bevy::app::App;
use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;

use crate::common::app_mode::AppMode;
use crate::editor::editable::{FeatureId, FeatureTimeline, FeatureTrait, PointRef};
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::tool::room::Room;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

const DEFAULT_SNAP_GRANULARITY: f32 = 0.1;

pub trait PlaceablePoint: Send + Sync + 'static {
    const TOOL: Tools;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait>;

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait>;

    fn normal_color() -> Color;

    fn relative_color() -> Color;

    /// Draw the shape that would land at `cursor`, at its real size.
    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color);
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum PointPlacementMode {
    Normal,
    Picking,
    RelativeSelected,
}

#[derive(Resource)]
pub struct PointPlacementTool<F: PlaceablePoint> {
    mode: PointPlacementMode,
    last_position: Vec3,
    cursor: Option<Vec3>,
    reference_feature: Option<FeatureId>,
    reference_key: String,
    reference_resolved: Option<Vec3>,
    hovered_point: Option<(FeatureId, String, Vec3)>,
    /// The Shift held for picking was used to fly; releasing it keeps the reference.
    flew: bool,
    snap: bool,
    snap_granularity: f32,
    _feature: PhantomData<F>,
}

impl<F: PlaceablePoint> Default for PointPlacementTool<F> {
    fn default() -> Self {
        Self {
            mode: PointPlacementMode::Normal,
            last_position: Vec3::ZERO,
            cursor: None,
            reference_feature: None,
            reference_key: String::new(),
            reference_resolved: None,
            hovered_point: None,
            flew: false,
            snap: true,
            snap_granularity: DEFAULT_SNAP_GRANULARITY,
            _feature: PhantomData,
        }
    }
}

/// Wire up a point-placing tool from its own `Plugin::build`.
pub fn add_point_placement_tool<F: PlaceablePoint>(app: &mut App) {
    app
        .init_resource::<PointPlacementTool<F>>()
        .add_systems(Update, (
            PointPlacementTool::<F>::interface,
            PointPlacementTool::<F>::draw_gizmos,
        ).chain().run_if(in_state(F::TOOL)).run_if(in_state(AppMode::Editor)))
        .add_systems(OnExit(F::TOOL), PointPlacementTool::<F>::on_exit)
    ;
}

impl<F: PlaceablePoint> PointPlacementTool<F> {
    fn on_exit(mut tool: ResMut<Self>) {
        tool.mode = PointPlacementMode::Normal;
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
        egui: Res<EguiWantsInput>,
    ) {
        let shift_held = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let shift_just_pressed = keys.just_pressed(KeyCode::ShiftLeft) || keys.just_pressed(KeyCode::ShiftRight);

        tool.cursor = compute_cursor(
            &mouse_input, &cameras, tool.last_position,
            tool.snap, tool.snap_granularity, &rooms,
        );

        match tool.mode {
            PointPlacementMode::Normal => {
                if shift_held {
                    tool.mode = PointPlacementMode::Picking;
                    tool.hovered_point = None;
                    tool.flew = false;
                } else if let Some(cursor) = tool.cursor {
                    if commit_at_cursor(&mouse_input, &keys, egui.wants_keyboard_input()) {
                        let id = features.apply_feature(F::from_position(cursor));
                        features.select(Some(id));
                        tool.last_position = cursor;
                        next_tool.set(Tools::Select);
                    }
                }
            }
            PointPlacementMode::Picking => {
                tool.flew |= camera_dragging(&mouse_input);
                if !shift_held {
                    tool.hovered_point = None;
                    if tool.flew && tool.reference_feature.is_some() {
                        tool.mode = PointPlacementMode::RelativeSelected;
                    } else {
                        tool.mode = PointPlacementMode::Normal;
                        tool.reference_feature = None;
                        tool.reference_key.clear();
                        tool.reference_resolved = None;
                    }
                    return;
                }

                tool.hovered_point = mouse_input.world_pos
                    .and_then(|ray| find_hovered_point(&ray, &features, PICK_RADIUS));

                if mouse_input.released == Some(MouseButton::Left) {
                    if let Some((feature_id, key, resolved)) = tool.hovered_point.take() {
                        tool.reference_feature = Some(feature_id);
                        tool.reference_key = key;
                        tool.reference_resolved = Some(resolved);
                        tool.mode = PointPlacementMode::RelativeSelected;
                    }
                }
            }
            PointPlacementMode::RelativeSelected => {
                // Kept until Shift comes up, in case it was held to fly faster.
                if shift_just_pressed {
                    tool.mode = PointPlacementMode::Picking;
                    tool.hovered_point = None;
                    tool.flew = false;
                    return;
                }

                let (Some(ref_feature), Some(ref_resolved)) = (tool.reference_feature, tool.reference_resolved) else {
                    return;
                };
                let typing = egui.wants_keyboard_input();
                if let Some(at) = relative_commit(&mouse_input, &keys, typing, tool.cursor, ref_resolved) {
                    let d = at - ref_resolved;
                    let mut pr = PointRef::reference_with_offset(ref_feature, d.x, d.y, d.z);
                    if !tool.reference_key.is_empty() {
                        pr.point_key = tool.reference_key.clone();
                    }
                    let id = features.apply_feature(F::from_point_ref(pr));
                    features.select(Some(id));
                    tool.last_position = at;
                    next_tool.set(Tools::Select);
                }
            }
        }
    }

    fn draw_gizmos(
        tool: Res<Self>,
        features: Res<FeatureTimeline>,
        mouse_input: Res<CurrentMouseInput>,
        mut gizmos: Gizmos,
    ) {
        if let Some(cursor) = tool.cursor {
            let color = match tool.mode {
                PointPlacementMode::RelativeSelected => F::relative_color(),
                _ => F::normal_color(),
            };
            F::draw_preview(&mut gizmos, cursor, color);

            if tool.mode == PointPlacementMode::RelativeSelected {
                if let Some(base) = tool.reference_resolved {
                    draw_taxicab_path(&mut gizmos, base, cursor);
                }
            }
        }

        if tool.mode == PointPlacementMode::Picking {
            if let Some(ray) = mouse_input.world_pos {
                draw_picking_gizmos(&mut gizmos, &ray, &features, &tool.hovered_point);
            }
        }
    }
}
