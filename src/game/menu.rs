//! The plate menus stand on and their rows: the pause menu and the team picker.

use bevy::prelude::*;

use crate::common::swatch::Swatch;
use crate::game::hud::{ZONE_DROP, ZONE_STROKE};
use crate::game::plate::{plate_text, FollowsTheLean, Plate};
use crate::game::plate_button::{plate_button_node, PlateButton};

pub const TITLE_SIZE: f32 = 42.0;
pub const ITEM_SIZE: f32 = 30.0;
const ITEM_GAP: f32 = 12.0;
const ITEM_MIN_WIDTH: f32 = 320.0;
/// OKLCH lightness the menu's panel is below the buttons' cream.
const MENU_DARKEN: f32 = -0.3;
/// Logical pixels each side, past the drop. The slant takes about 22 px of it at mid-height.
const MENU_SIDE: f32 = 44.0;

/// The dark plate a menu's title and buttons stand on.
pub fn menu_panel() -> impl Bundle {
    (
        Plate::new(Swatch::neutral().lightened(MENU_DARKEN)),
        Node {
            padding: UiRect {
                left: Val::Px(ZONE_STROKE + MENU_SIDE),
                right: Val::Px(ZONE_STROKE + ZONE_DROP.x + MENU_SIDE),
                top: Val::Px(ZONE_STROKE + 16.0),
                bottom: Val::Px(ZONE_STROKE + ZONE_DROP.y + 28.0),
            },
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Stretch,
            row_gap: Val::Px(ITEM_GAP),
            ..default()
        },
    )
}

pub fn menu_title(font: &Handle<Font>, words: String) -> impl Bundle {
    (plate_text(font, words, TITLE_SIZE), FollowsTheLean, Node { align_self: AlignSelf::Center, ..default() })
}

/// One row of a [`menu_panel`]; give it a [`plate_text`] child at [`ITEM_SIZE`].
pub fn menu_button(swatch: Swatch) -> impl Bundle {
    (PlateButton::new(swatch), Plate::new(swatch), Pickable::default(), FollowsTheLean, menu_item_node())
}

/// [`menu_button`]'s node, for a row that is a plain [`Plate`] rather than a button.
pub fn menu_item_node() -> Node {
    Node { min_width: Val::Px(ITEM_MIN_WIDTH), ..plate_button_node() }
}
