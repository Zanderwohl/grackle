//! Places a [`Mirror`] at its center. Its sources and teams are picked in its panel.

use bevy::prelude::*;
use crate::editor::editable::{FeatureTrait, PointRef};
use crate::editor::mirror::Mirror;
use crate::tool::point_placement::PlaceablePoint;
use crate::tool::Tools;

impl PlaceablePoint for Mirror {
    const TOOL: Tools = Tools::Mirror;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait> {
        Box::new(Mirror::new(PointRef::absolute(position.x, position.y, position.z)))
    }

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait> {
        Box::new(Mirror::new(point_ref))
    }

    fn normal_color() -> Color {
        Color::srgb_u8(230, 90, 230)
    }

    fn relative_color() -> Color {
        Color::srgb_u8(240, 160, 240)
    }

    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color) {
        gizmos.sphere(Isometry3d::from_translation(cursor), 0.2, color);
        gizmos.line(cursor, cursor + Vec3::Y * 3.0, color);
    }
}
