//! Places [`SpectatorCamera`] features: the point tool with a camera at the end.

use bevy::prelude::*;

use crate::editor::editable::{FeatureTrait, PointRef};
use crate::editor::spectator_camera::SpectatorCamera;
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

pub struct SpectatorCameraPlugin;

impl Plugin for SpectatorCameraPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<SpectatorCamera>(app);
    }
}

impl PlaceablePoint for SpectatorCamera {
    const TOOL: Tools = Tools::SpectatorCamera;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait> {
        Box::new(SpectatorCamera::new(position.x, position.y, position.z))
    }

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait> {
        Box::new(SpectatorCamera::from_point_ref(point_ref))
    }

    fn normal_color() -> Color {
        Color::srgb(0.75, 0.45, 0.95)
    }

    fn relative_color() -> Color {
        Color::srgb(0.88, 0.75, 1.0)
    }

    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color) {
        gizmos.cube(Transform::from_translation(cursor).with_scale(Vec3::splat(0.25)), color);
        gizmos.arrow(cursor, cursor + Vec3::NEG_Z * 0.6, color);
    }
}
