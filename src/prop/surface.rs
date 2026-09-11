//! What a solid is made of, and what colour it is.
//!
//! Two halves, deliberately kept apart. A **style** is a physical material —
//! how light comes off it — and there are three because that is how many
//! things a Quake-era prop is ever made of. A **tint** is what colour that
//! material was painted. Keeping them separate is what lets a red plastic
//! grip and a red painted barrel be obviously different objects rather than
//! the same red.
//!
//! A style is not a texture. Nothing here loads an image: the whole look is
//! four numbers on a `StandardMaterial`, which is the right level of fidelity
//! for flat-shaded polygons and the only one that costs a browser nothing.
//!
//! A [`Surface`] rides on every polygon and survives being cut in half by a
//! boolean, which is what makes subtracting a hole out of a wooden stock leave
//! wood on the walls of the hole.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::get;

/// How light comes off a surface.
///
/// Three, and adding a fourth is one variant and one match arm — the table in
/// [`Style::material`] is the whole definition, so a style cannot be named in
/// one place and undescribed in another.
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
    /// Everything that draws a prop comes through here, so a style looks the
    /// same in the prop editor, in a mapper's hands and in a match. Let two
    /// callers each pick their own roughness and a weapon is a different
    /// object in the editor than in the game.
    pub fn material(self, tint: Tint) -> StandardMaterial {
        let base_color = tint.color();
        match self {
            // Metal takes its colour from reflection rather than from a base
            // colour, so a tinted metal is a *tinted* metal and not a
            // coloured one; that is why the tint is still the base colour and
            // the metallic term is what makes it read as steel.
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
/// Stored as an array rather than as a Bevy [`Color`] because this goes in a
/// file somebody reads: `tint = [0.2, 0.2, 0.22]` is a diff you can take in at
/// a glance, and a `Color` serialises as whichever of its several
/// representations it happened to be holding.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tint(#[serde(with = "crate::prop::nice_f32::array")] pub [f32; 3]);

impl Default for Tint {
    fn default() -> Self {
        // Gunmetal rather than white: a new solid that already looks like a
        // prop is one fewer thing to set before you can judge the shape.
        Tint([0.42, 0.44, 0.47])
    }
}

impl Tint {
    pub fn color(self) -> Color {
        Color::srgb(self.0[0], self.0[1], self.0[2])
    }

    pub fn from_color(color: Color) -> Self {
        let srgb = color.to_srgba();
        Tint([srgb.red, srgb.green, srgb.blue])
    }
}

/// A style and a tint together: everything a polygon knows about how it looks.
///
/// `Copy`, because it is carried by every polygon and copied onto both halves
/// whenever a boolean cuts one.
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
