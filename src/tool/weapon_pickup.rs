//! Places [`WeaponPickup`] features, and hands their panel the catalog.

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

use crate::common::app_mode::AppMode;
use crate::common::weapon::WeaponCatalog;
use crate::editor::editable::{FeatureTrait, PointRef};
use crate::common::picker::Choice;
use crate::editor::weapon_pickup::{self, WeaponPickup, WEAPONS};
use crate::prop::picker::PropPreviews;
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

/// Again whenever the catalog changes or a picture arrives, since the button
/// names its weapon whether or not it is open; models are only pictured while
/// it is.
fn offer_the_catalog(
    mut previews: PropPreviews,
    catalog: Res<WeaponCatalog>,
    mut offered: Local<Option<usize>>,
) {
    let Some(ctx) = previews.ctx() else { return };
    if WEAPONS.take_open(ctx) {
        previews.want_pictures(catalog.weapons.iter().filter_map(|weapon| weapon.model.as_deref()));
    }
    if *offered == Some(previews.pictures()) && !catalog.is_changed() {
        return;
    }
    *offered = Some(previews.pictures());
    let mut choices: Vec<Choice> = catalog
        .weapons
        .iter()
        .map(|weapon| Choice {
            label: get!(weapon.name_key),
            preview: weapon.model.as_deref().and_then(|model| previews.picture(model)),
            ..Choice::named(weapon.key.clone())
        })
        .collect();
    choices.sort_by(|a, b| a.label.cmp(&b.label));
    let Some(ctx) = previews.ctx() else { return };
    WEAPONS.publish(ctx, choices);
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
