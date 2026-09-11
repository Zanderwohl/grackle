//! What a solid is made of, and what colour it is.
//!
//! Kept apart on purpose: a **style** is how light comes off a material, a
//! **tint** is what colour it was painted. That is what lets a red plastic
//! grip and a red painted barrel be obviously different objects.
//!
//! A style is not a texture — four numbers on a `StandardMaterial`, which is
//! the right fidelity for flat-shaded polygons and costs a browser nothing.
//!
//! A [`Surface`] rides on every polygon and survives a boolean cutting it in
//! half, which is what leaves wood on the walls of a hole in a wooden stock.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::get;

/// How light comes off a surface. Adding a fourth is one variant and one arm
/// in [`Style::material`], which is the whole definition.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, EnumIter, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    #[default]
    Metal,
    Plastic,
    Wood,
}

impl Style {
    pub const ALL: [Style; 3] = [Style::Metal, Style::Plastic, Style::Wood];

    pub fn name(self) -> String {
        match self {
            Style::Metal => get!("prop.styles.metal"),
            Style::Plastic => get!("prop.styles.plastic"),
            Style::Wood => get!("prop.styles.wood"),
        }
    }

    /// The numbers that make the style, with the tint already applied.
    ///
    /// Everything that draws a prop comes through here, so a weapon is not a
    /// different object in the editor than it is in a match.
    pub fn material(self, tint: Tint) -> StandardMaterial {
        let base_color = tint.color();
        match self {
            // Metal takes its colour from reflection, so the tint is still the
            // base colour and the metallic term is what reads as steel.
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
        }
    }
}

/// A colour, as three sRGB channels in `0..=1`.
///
/// An array rather than a Bevy [`Color`] because this goes in a file somebody
/// reads, and a `Color` serialises as whichever of its several representations
/// it happened to be holding.
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

/// Everything a polygon knows about how it looks. `Copy`: it is carried by
/// every polygon and copied onto both halves whenever a boolean cuts one.
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
