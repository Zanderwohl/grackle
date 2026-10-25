//! Places [`SpawnPoint`] features. The point tool with a different feature at
//! the end; the shared half is [`crate::tool::point_placement`].

use bevy::app::App;
use bevy::prelude::*;
use crate::common::app_mode::AppMode;
use crate::common::team::Side;
use crate::common::class::{TALLEST_CLASS_EYE_HEIGHT, TALLEST_CLASS_HEIGHT};
use crate::editor::editable::{FeatureTrait, PointRef};
use crate::editor::spawn_point::{SpawnPoint, SpawnPointMarker};
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

pub struct SpawnPointPlugin;

impl Plugin for SpawnPointPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<SpawnPoint>(app);
        // Ungated: it has to notice the mode changing.
        app.add_systems(Update, hide_spawn_bodies_while_playing)
            .add_systems(Update, draw_spawn_sides.run_if(in_state(AppMode::Editor)));
    }
}

/// A ring at the feet in the side's colour, drawn always rather than only when selected, so which half
/// of a mirrored map is whose can be read at a glance. A ring and not a tint: the preview stays green,
/// the one colour no player is.
fn draw_spawn_sides(
    spawns: Query<(&GlobalTransform, &Side), With<SpawnPointMarker>>,
    mut gizmos: Gizmos,
) {
    for (placed, side) in &spawns {
        let ground = Isometry3d::new(placed.translation(), Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
        gizmos.circle(ground, 0.6, side.0.colour());
    }
}

/// Hidden in Play, where a player spawns inside it.
fn hide_spawn_bodies_while_playing(
    mode: Res<State<AppMode>>,
    mut bodies: Query<&mut Visibility, With<SpawnPointMarker>>,
) {
    let wanted = match mode.get().is_authoring() {
        true => Visibility::Inherited,
        false => Visibility::Hidden,
    };
    for mut visibility in &mut bodies {
        // Only on a change, so it does not mark every spawn changed each frame.
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
