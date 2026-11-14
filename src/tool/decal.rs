//! Places [`EditorDecal`] features: the point tool, pressing a picture onto the floor below.

use bevy::prelude::*;

use crate::editor::decal::EditorDecal;
use crate::editor::editable::{FeatureTrait, PointRef};
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

pub struct DecalToolPlugin;

impl Plugin for DecalToolPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<EditorDecal>(app);
    }
}

impl PlaceablePoint for EditorDecal {
    const TOOL: Tools = Tools::Decal;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait> {
        Box::new(EditorDecal::new(position.x, position.y, position.z))
    }

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait> {
        Box::new(EditorDecal::from_point_ref(point_ref))
    }

    fn normal_color() -> Color {
        Color::srgb(0.95, 0.75, 0.3)
    }

    fn relative_color() -> Color {
        Color::srgb(1.0, 0.9, 0.65)
    }

    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color) {
        gizmos.rect(Isometry3d::new(cursor, Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)), Vec2::ONE, color);
        gizmos.arrow(cursor + Vec3::Y * 0.25, cursor - Vec3::Y * 0.25, color);
    }
}
