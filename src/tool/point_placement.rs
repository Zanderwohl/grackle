//! The shared state machine behind every point-placing tool: hover to
//! preview, click to commit, or hold shift to pick a reference point and place
//! relative to it. A tool supplies only a [`PlaceablePoint`].

use crate::common::keybind::Actions;
use std::marker::PhantomData;

use bevy::app::App;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::editor::editable::{FeatureTimeline, FeatureTrait, PointRef};
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::tool::point_picker::{Picked, PointPicker};
use crate::tool::room::MapVolumes;
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

#[derive(Resource)]
pub struct PointPlacementTool<F: PlaceablePoint> {
    picker: PointPicker,
    last_position: Vec3,
    cursor: Option<Vec3>,
    snap: bool,
    snap_granularity: f32,
    _feature: PhantomData<F>,
}

impl<F: PlaceablePoint> Default for PointPlacementTool<F> {
    fn default() -> Self {
        Self {
            picker: PointPicker::default(),
            last_position: Vec3::ZERO,
            cursor: None,
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
        tool.picker.reset();
        tool.cursor = None;
    }

    fn interface(
        mut tool: ResMut<Self>,
        cameras: Query<(Entity, &Multicam)>,
        mouse_input: Res<CurrentMouseInput>,
        actions: Res<Actions>,
        mut features: ResMut<FeatureTimeline>,
        rooms: MapVolumes,
        mut next_tool: ResMut<NextState<Tools>>,
    ) {
        let tool = &mut *tool;
        tool.cursor = compute_cursor(&mouse_input, &cameras, tool.last_position, tool.snap, tool.snap_granularity, &rooms);
        let feature = match tool.picker.update(&mouse_input, &actions, &features, tool.cursor, None, Some) {
            Picked::Nothing | Picked::Canceled => return,
            Picked::Absolute(at) => (F::from_position(at), at),
            Picked::Relative(point, at) => (F::from_point_ref(point), at),
        };
        let id = features.apply_feature(feature.0);
        features.select(Some(id));
        tool.last_position = feature.1;
        next_tool.set(Tools::Select);
    }

    fn draw_gizmos(
        tool: Res<Self>,
        features: Res<FeatureTimeline>,
        mouse_input: Res<CurrentMouseInput>,
        mut gizmos: Gizmos,
    ) {
        if let Some(cursor) = tool.cursor {
            let color = if tool.picker.reference().is_some() { F::relative_color() } else { F::normal_color() };
            F::draw_preview(&mut gizmos, cursor, color);
        }
        tool.picker.draw(&mut gizmos, &mouse_input, &features, tool.cursor);
    }
}
