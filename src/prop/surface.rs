//! A style (how light comes off a surface) and a tint (its color), kept
//! apart so red plastic and red-painted metal differ. A style is a few
//! `StandardMaterial` numbers, not a texture — except
//! [`Style::ForwardCamera`], which in the viewmodel shows what is ahead; see
//! [`crate::game::forward_camera`].

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::get;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, EnumIter, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    #[default]
    Metal,
    Plastic,
    Wood,
    /// Glass everywhere but the viewmodel, where it is the forward camera's
    /// view, tinted.
    ForwardCamera,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Metal, Style::Plastic, Style::Wood, Style::ForwardCamera];

    pub fn name(self) -> String {
        match self {
            Style::Metal => get!("prop.styles.metal"),
            Style::Plastic => get!("prop.styles.plastic"),
            Style::Wood => get!("prop.styles.wood"),
            Style::ForwardCamera => get!("prop.styles.forward_camera"),
        }
    }

    pub fn material(self, tint: Tint) -> StandardMaterial {
        let base_color = tint.color();
        match self {
            Style::Metal => StandardMaterial {
                base_color,
                metallic: 1.0,
                perceptual_roughness: 0.35,
                ..default()
            },
            Style::Plastic => StandardMaterial {
                base_color,
                metallic: 0.0,
                perceptual_roughness: 0.55,
                reflectance: 0.35,
                ..default()
            },
            Style::Wood => StandardMaterial {
                base_color,
                metallic: 0.0,
                perceptual_roughness: 0.85,
                reflectance: 0.15,
                ..default()
            },
            Style::ForwardCamera => StandardMaterial {
                base_color,
                metallic: 0.0,
                perceptual_roughness: 0.05,
                reflectance: 0.6,
                ..default()
            },
        }
    }
}

/// sRGB channels in `0..=1`. Not a Bevy [`Color`], which serializes as
/// whichever representation it happens to hold.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tint(#[serde(with = "crate::prop::nice_f32::array")] pub [f32; 3]);

impl Default for Tint {
    fn default() -> Self {
        // Gunmetal rather than white, so a new solid already looks like a prop.
        Tint([0.42, 0.44, 0.47])
    }
}

impl Tint {
    pub fn color(self) -> Color {
        Color::srgb(self.0[0], self.0[1], self.0[2])
    }
}

/// Carried by every polygon and copied onto both halves of a cut.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    pub style: Style,
    pub tint: Tint,
}

impl Surface {
    pub fn new(style: Style, tint: Tint) -> Self {
        Self { style, tint }
    }

    pub fn material(self) -> StandardMaterial {
        self.style.material(self.tint)
    }
}
