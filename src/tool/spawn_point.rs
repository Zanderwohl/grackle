//! Places [`SpawnPoint`] features.
//!
//! Deliberately the point tool with a different feature at the end of it:
//! placing a spawn *is* placing a point, and a mapper who has learnt one has
//! learnt the other — relative placement and shift-picking included. The
//! shared half lives in [`crate::tool::point_placement`].

use bevy::app::App;
use bevy::prelude::*;
use crate::common::class::{TALLEST_CLASS_EYE_HEIGHT, TALLEST_CLASS_HEIGHT};
use crate::editor::editable::{FeatureTrait, PointRef};
use crate::editor::spawn_point::SpawnPoint;
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

pub struct SpawnPointPlugin;

impl Plugin for SpawnPointPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<SpawnPoint>(app);
    }
}

impl PlaceablePoint for SpawnPoint {
    const TOOL: Tools = Tools::SpawnPoint;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait> {
        Box::new(SpawnPoint::new(position.x, position.y, position.z))
    }

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait> {
        Box::new(SpawnPoint::from_point_ref(point_ref))
    }

    fn normal_color() -> Color {
        Color::srgb_u8(255, 214, 0)
    }

    fn relative_color() -> Color {
        Color::srgb_u8(255, 233, 120)
    }

    /// Preview the body that would stand here, so headroom is visible before
    /// the spawn point is committed rather than after.
    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color) {
        gizmos.sphere(Isometry3d::from_translation(cursor), 0.15, color);
        gizmos.line(cursor, cursor + Vec3::Y * TALLEST_CLASS_HEIGHT, color);
        gizmos.sphere(
            Isometry3d::from_translation(cursor + Vec3::Y * TALLEST_CLASS_EYE_HEIGHT),
            0.12,
            color,
        );
    }
}
