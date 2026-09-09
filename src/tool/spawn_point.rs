//! Places [`SpawnPoint`] features.
//!
//! Deliberately the point tool with a different feature at the end of it:
//! placing a spawn *is* placing a point, and a mapper who has learnt one has
//! learnt the other — relative placement and shift-picking included. The
//! shared half lives in [`crate::tool::point_placement`].

use bevy::app::App;
use bevy::prelude::*;
use crate::common::app_mode::AppMode;
use crate::common::class::{TALLEST_CLASS_EYE_HEIGHT, TALLEST_CLASS_HEIGHT};
use crate::editor::editable::{FeatureTrait, PointRef};
use crate::editor::spawn_point::{SpawnPoint, SpawnPointMarker};
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

pub struct SpawnPointPlugin;

impl Plugin for SpawnPointPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<SpawnPoint>(app);
        // Ungated: it is the system that has to notice the mode changed.
        app.add_systems(Update, hide_spawn_bodies_while_playing);
    }
}

/// A spawn point's body is drawn in the editor and not while playing: the one
/// place it would certainly be in the way is where a player materialises,
/// which is inside it.
///
/// Hidden rather than despawned, so F5 back brings it straight back.
fn hide_spawn_bodies_while_playing(
    mode: Res<State<AppMode>>,
    mut bodies: Query<&mut Visibility, With<SpawnPointMarker>>,
) {
    let wanted = match mode.get() {
        AppMode::Editor => Visibility::Inherited,
        AppMode::Play => Visibility::Hidden,
    };
    for mut visibility in &mut bodies {
        // Assigned only on a change, so this does not dirty every spawn point
        // on the map every frame.
        if *visibility != wanted {
            *visibility = wanted;
        }
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
