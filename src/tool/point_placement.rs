//! The shared state machine behind every point-placing tool.
//!
//! Placing a point, a light, a spawn or a prop is one interaction: hover to
//! see where the thing would land, click to commit, or hold shift to pick an
//! existing reference point first and place relative to it. The tools differ
//! only in which feature comes out at the end and what the preview looks like,
//! so that is all a tool has to supply — see [`PlaceablePoint`].
//!
//! Each tool keeps its own `Plugin`, its own [`Tools`] variant and its own
//! lang key; what it does not keep is a fifth copy of the state machine.

use std::marker::PhantomData;

use bevy::app::App;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::editor::editable::{FeatureId, FeatureTimeline, FeatureTrait, PointRef};
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::tool::room::Room;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

const DEFAULT_SNAP_GRANULARITY: f32 = 0.1;

/// What a tool has to provide to be a point-placing tool: which feature to
/// build (absolutely, and relative to another feature's point), which [`Tools`]
/// state it runs in, and what the cursor preview looks like.
pub trait PlaceablePoint: Send + Sync + 'static {
    /// The tool state this tool's systems are gated on.
    const TOOL: Tools;

    /// Build the feature at an absolute world position.
    fn from_position(position: Vec3) -> Box<dyn FeatureTrait>;

    /// Build the feature anchored to another feature's point.
    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait>;

    /// Preview colour while placing absolutely.
    fn normal_color() -> Color;

    /// Preview colour while placing relative to a picked reference point.
    fn relative_color() -> Color;

    /// Draw the shape that would land at `cursor`, so its size and footprint
    /// are visible before it is committed rather than after.
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
            snap: true,
            snap_granularity: DEFAULT_SNAP_GRANULARITY,
            _feature: PhantomData,
        }
    }
}

/// Wire up a point-placing tool: its resource, its two systems gated on
/// `F::TOOL`, and the reset on leaving the tool.
///
/// Call this from the tool's own `Plugin::build`; anything else the tool needs
/// (a mesh-attaching system, extra resources) stays with the tool.
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
                } else if let Some(cursor) = tool.cursor {
                    if mouse_input.released == Some(MouseButton::Left) {
                        let id = features.apply_feature(F::from_position(cursor));
                        features.select(Some(id));
                        tool.last_position = cursor;
                        next_tool.set(Tools::Select);
                    }
                }
            }
            PointPlacementMode::Picking => {
                if !shift_held {
                    tool.mode = PointPlacementMode::Normal;
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
                        tool.mode = PointPlacementMode::RelativeSelected;
                    }
                }
            }
            PointPlacementMode::RelativeSelected => {
                if shift_just_pressed {
                    tool.mode = PointPlacementMode::Normal;
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
                            let id = features.apply_feature(F::from_point_ref(pr));
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
