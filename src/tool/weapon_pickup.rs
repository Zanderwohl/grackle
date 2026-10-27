//! Places [`WeaponPickup`] features, and hands their panel the catalog.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::AppMode;
use crate::common::weapon::WeaponCatalog;
use crate::editor::editable::{FeatureTrait, PointRef};
use crate::editor::weapon_pickup::{self, WeaponChoices, WeaponPickup};
use crate::get;
use crate::tool::point_placement::{add_point_placement_tool, PlaceablePoint};
use crate::tool::Tools;

pub struct WeaponPickupPlugin;

impl Plugin for WeaponPickupPlugin {
    fn build(&self, app: &mut App) {
        add_point_placement_tool::<WeaponPickup>(app);
        app.add_systems(
            EguiPrimaryContextPass,
            offer_the_catalog.run_if(in_state(AppMode::Editor)),
        );
    }
}

/// Every frame, so a catalog that arrives from a server or a change of
/// language reaches an open panel. Unordered against the panels: the data
/// outlives the frame, so at worst a panel lists last frame's.
fn offer_the_catalog(mut contexts: EguiContexts, catalog: Res<WeaponCatalog>) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let mut choices: Vec<(String, String)> =
        catalog.weapons.iter().map(|weapon| (weapon.key.clone(), get!(weapon.name_key))).collect();
    choices.sort_by(|a, b| a.1.cmp(&b.1));
    ctx.data_mut(|data| data.insert_temp(WeaponChoices::id(), WeaponChoices(choices)));
}

impl PlaceablePoint for WeaponPickup {
    const TOOL: Tools = Tools::WeaponPickup;

    fn from_position(position: Vec3) -> Box<dyn FeatureTrait> {
        Box::new(WeaponPickup::new(position.x, position.y, position.z))
    }

    fn from_point_ref(point_ref: PointRef) -> Box<dyn FeatureTrait> {
        Box::new(WeaponPickup::from_point_ref(point_ref))
    }

    fn normal_color() -> Color {
        Color::srgb_u8(240, 170, 60)
    }

    fn relative_color() -> Color {
        Color::srgb_u8(250, 210, 140)
    }

    fn draw_preview(gizmos: &mut Gizmos, cursor: Vec3, color: Color) {
        let center = WeaponPickup::box_center_over(cursor);
        gizmos.cube(Transform::from_translation(center).with_scale(weapon_pickup::BOX), color);
        gizmos.line(cursor, center - Vec3::Y * (weapon_pickup::BOX.y * 0.5), color);
    }
}
