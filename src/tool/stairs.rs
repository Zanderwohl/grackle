//! Places [`EditorStairs`] by its four points, each picked by a [`PointPicker`]. Escape outside a
//! relative pick drops what was picked.

use bevy::app::App;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::keybind::Actions;
use crate::common::stairs::{StairsShape, StairsStyle};
use crate::editor::editable::{FeatureTimeline, PointRef};
use crate::editor::input::CurrentMouseInput;
use crate::editor::multicam::Multicam;
use crate::editor::stairs::{draw_collider, EditorStairs, STAIRS_COLOR};
use crate::tool::point_picker::{Picked, PointPicker};
use crate::tool::room::MapVolumes;
use crate::tool::tool_helpers::*;
use crate::tool::Tools;

const DEFAULT_SNAP_GRANULARITY: f32 = 0.1;

/// Meters. The height point is kept at least this far above the footprint while picking.
pub const LEAST_HEIGHT: f32 = 0.1;

/// Meters. A run or width shorter than this is refused.
const SHORTEST: f32 = 1.0e-3;

const CURSOR_COLOR: Color = Color::srgb_u8(60, 120, 255);
const PICKED_COLOR: Color = Color::srgb_u8(100, 255, 100);
const REFUSED_COLOR: Color = Color::srgb_u8(240, 70, 50);

pub struct StairsToolPlugin;

impl Plugin for StairsToolPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        app.init_resource::<StairsTool>()
            .add_systems(
                Update,
                (StairsTool::interface, StairsTool::draw_gizmos).chain().run_if(in_state(Tools::Stairs)).run_if(in_state(AppMode::Editor)),
            )
            .add_systems(OnExit(Tools::Stairs), StairsTool::on_exit);
    }
}

#[derive(Resource)]
struct StairsTool {
    picker: PointPicker,
    /// Start, end, width, in that order; the fourth pick makes the feature.
    picked: Vec<(PointRef, Vec3)>,
    cursor: Option<Vec3>,
    /// The depth an orthographic view puts the cursor at.
    last_position: Vec3,
    snap: bool,
    snap_granularity: f32,
}

impl Default for StairsTool {
    fn default() -> Self {
        Self {
            picker: PointPicker::default(),
            picked: Vec::new(),
            cursor: None,
            last_position: Vec3::ZERO,
            snap: true,
            snap_granularity: DEFAULT_SNAP_GRANULARITY,
        }
    }
}

/// The footprint plane's normal through start, end and the width point, upward where it has an
/// upward side. `None` while the three are in a line.
fn footprint_up(start: Vec3, end: Vec3, width: Vec3) -> Option<Vec3> {
    let along = end - start;
    let run = along.length();
    if run < SHORTEST {
        return None;
    }
    let normal = (along / run).cross(width - start);
    if normal.length() < SHORTEST {
        return None;
    }
    let normal = normal.normalize();
    Some(if normal.y < 0.0 { -normal } else { normal })
}

/// `point`, moved along `up` until it is at least [`LEAST_HEIGHT`] above the plane through `start`.
fn lift(point: Vec3, start: Vec3, up: Vec3) -> Vec3 {
    let height = (point - start).dot(up);
    if height < LEAST_HEIGHT { point + up * (LEAST_HEIGHT - height) } else { point }
}

impl StairsTool {
    fn on_exit(mut tool: ResMut<Self>) {
        let last_position = tool.last_position;
        *tool = StairsTool { last_position, ..default() };
    }


    /// Where a pick at `cursor` would land: the height point lifted off the footprint. `None`
    /// for an end on the start or a width point in line with the run.
    fn placed(&self, cursor: Vec3) -> Option<Vec3> {
        match self.picked.as_slice() {
            [(_, start)] => ((cursor - *start).length() >= SHORTEST).then_some(cursor),
            [(_, start), (_, end)] => footprint_up(*start, *end, cursor).map(|_| cursor),
            [(_, start), (_, end), (_, width)] => Some(footprint_up(*start, *end, *width).map_or(cursor, |up| lift(cursor, *start, up))),
            _ => Some(cursor),
        }
    }

    /// Takes the pick; the fourth makes the feature and hands back to Select.
    fn commit(&mut self, point: PointRef, at: Vec3, features: &mut FeatureTimeline, next_tool: &mut NextState<Tools>) {
        self.last_position = at;
        self.picked.push((point, at));
        let [(start, _), (end, _), (width, _), (height, _)] = self.picked.as_slice() else { return };
        let stairs = EditorStairs::new(start.clone(), end.clone(), width.clone(), height.clone());
        self.picked.clear();
        let id = features.apply_feature(Box::new(stairs));
        features.select(Some(id));
        next_tool.set(Tools::Select);
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
        let suggestion = tool.picked.last().map_or(tool.last_position, |(_, at)| *at);
        tool.cursor = compute_cursor(&mouse_input, &cameras, suggestion, tool.snap, tool.snap_granularity, &rooms);
        let snap = tool.snap.then_some(tool.snap_granularity);
        let mut picker = std::mem::take(&mut tool.picker);
        let picked = picker.update(&mouse_input, &actions, &features, tool.cursor, snap, |cursor| tool.placed(cursor));
        tool.picker = picker;
        match picked {
            Picked::Nothing => {}
            Picked::Canceled => tool.picked.clear(),
            Picked::Absolute(at) => tool.commit(PointRef::absolute(at.x, at.y, at.z), at, &mut features, &mut next_tool),
            Picked::Relative(point, at) => tool.commit(point, at, &mut features, &mut next_tool),
        }
    }

    fn draw_gizmos(tool: Res<Self>, features: Res<FeatureTimeline>, mouse_input: Res<CurrentMouseInput>, mut gizmos: Gizmos) {
        for (_, at) in &tool.picked {
            gizmos.sphere(Isometry3d::from_translation(*at), 0.15, PICKED_COLOR);
        }
        let placed = tool.cursor.map(|cursor| (cursor, tool.placed(cursor)));
        match placed {
            Some((cursor, None)) => {
                gizmos.sphere(Isometry3d::from_translation(cursor), 0.15, REFUSED_COLOR);
            }
            Some((_, Some(at))) => {
                gizmos.sphere(Isometry3d::from_translation(at), 0.15, CURSOR_COLOR);
                match tool.picked.as_slice() {
                    [(_, start)] => {
                        gizmos.line(*start, at, STAIRS_COLOR);
                    }
                    [(_, start), (_, end)] => {
                        gizmos.line(*start, *end, STAIRS_COLOR);
                        if let Some(along) = (*end - *start).try_normalize() {
                            let foot = *start + along * (at - *start).dot(along);
                            dashed_line(&mut gizmos, foot, at, STAIRS_COLOR, 0.15, 0.1);
                        }
                    }
                    [(_, start), (_, end), (_, width)] => {
                        let shape = StairsShape { start: *start, end: *end, width: *width, height: at, style: StairsStyle::default() };
                        draw_collider(&mut gizmos, &shape, STAIRS_COLOR);
                    }
                    _ => {}
                }
            }
            None => {}
        }
        let shown = placed.map(|(cursor, at)| at.unwrap_or(cursor));
        tool.picker.draw(&mut gizmos, &mouse_input, &features, shown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_height_point_is_kept_off_the_footprint() {
        let (start, end, width) = (Vec3::ZERO, Vec3::X * 4.0, Vec3::new(0.0, 0.0, -2.0));
        let up = footprint_up(start, end, width).unwrap();
        assert!(up.abs_diff_eq(Vec3::Y, 1e-5), "{up}");
        assert!(lift(Vec3::new(2.0, -1.0, 0.0), start, up).abs_diff_eq(Vec3::new(2.0, LEAST_HEIGHT, 0.0), 1e-5));
        assert_eq!(lift(Vec3::new(2.0, 3.0, 0.0), start, up), Vec3::new(2.0, 3.0, 0.0));
    }

    #[test]
    fn a_width_point_in_line_with_the_run_is_refused() {
        let mut tool = StairsTool::default();
        tool.picked = vec![(PointRef::absolute(0.0, 0.0, 0.0), Vec3::ZERO), (PointRef::absolute(4.0, 0.0, 0.0), Vec3::X * 4.0)];
        assert!(tool.placed(Vec3::X * 2.0).is_none());
        assert!(tool.placed(Vec3::new(2.0, 0.0, 1.0)).is_some());
        tool.picked.truncate(1);
        assert!(tool.placed(Vec3::ZERO).is_none());
    }
}
