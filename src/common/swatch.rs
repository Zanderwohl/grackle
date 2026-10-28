//! Interface colors, as against a map's paint in [`crate::common::palette`]:
//! one base per team, and its highlights and shadows derived in OKLCH, where
//! lightness is perceptual and so a "lighter" red and a "lighter" yellow look
//! equally lighter.
//!
//! These are not [`Team::color`], which is a body's base under map lighting
//! and deliberately muted. Flat panels under no light want more chroma.
//!
//! Highlights warm toward yellow and shadows cool toward blue, the way paint
//! and light do, rather than simply mixing with white and black, which grays a
//! color out.

use bevy::prelude::*;

use crate::common::player_color::PlayerColor;
use crate::common::team::Team;

/// Where highlights lean, in OKLCH degrees: a warm yellow.
const WARM_HUE: f32 = 95.0;
/// Where shadows lean: a cool blue-violet.
const COOL_HUE: f32 = 275.0;
/// [`Swatch::player`]'s, between the four teams'. Chroma is asked for high
/// and given back by [`in_gamut`] wherever a hue cannot hold it.
const PLAYER_LIGHTNESS: f32 = 0.62;
const PLAYER_CHROMA: f32 = 0.17;
/// An outline's lightness in [`Ink::Dark`], whatever the base's.
const DARK_INK_LIGHTNESS: f32 = 0.24;

/// One color and the tones around it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Swatch {
    pub base: Color,
    /// For rims and glints.
    pub highlight: Color,
    /// For the side away from the light.
    pub shadow: Color,
    /// Darker still: drop shadows and text on light bases.
    pub deep: Color,
}

impl Swatch {
    pub fn from_oklch(lightness: f32, chroma: f32, hue: f32) -> Self {
        let base = Oklcha::lch(lightness, chroma, hue);
        Self {
            base: in_gamut(base),
            highlight: in_gamut(Oklcha::lch(
                (lightness + 0.13).min(0.97),
                chroma * 0.85,
                lean(hue, WARM_HUE, 0.2),
            )),
            shadow: in_gamut(Oklcha::lch(
                (lightness - 0.16).max(0.05),
                chroma * 1.05,
                lean(hue, COOL_HUE, 0.15),
            )),
            deep: in_gamut(Oklcha::lch(
                (lightness - 0.38).max(0.04),
                chroma * 0.7,
                lean(hue, COOL_HUE, 0.25),
            )),
        }
    }

    pub fn team(team: Team) -> Self {
        match team {
            Team::Red => Self::from_oklch(0.60, 0.20, 27.0),
            Team::Blue => Self::from_oklch(0.56, 0.15, 255.0),
            Team::Yellow => Self::from_oklch(0.84, 0.16, 88.0),
            Team::Green => Self::from_oklch(0.64, 0.16, 148.0),
        }
    }

    /// Before a body has a team: a cream that reads against any map.
    pub fn neutral() -> Self {
        Self::from_oklch(0.88, 0.035, 85.0)
    }

    pub fn of(team: Option<Team>) -> Self {
        team.map_or_else(Self::neutral, Self::team)
    }

    /// A player's own hue, for a mode with no teams: as light as a team's and
    /// as saturated as the hue allows.
    pub fn player(color: PlayerColor) -> Self {
        Self::from_oklch(PLAYER_LIGHTNESS, PLAYER_CHROMA, color.hue)
    }

    /// What a body's HUD is painted in: its team, else its own color, else
    /// neutral, the order [`body_look`](crate::game::body_mesh) draws it in.
    pub fn of_body(team: Option<Team>, color: Option<PlayerColor>) -> Self {
        match (team, color) {
            (Some(team), _) => Self::team(team),
            (None, Some(color)) => Self::player(color),
            (None, None) => Self::neutral(),
        }
    }

    /// Whichever of white and [`Ink::Black`] stands out on the base.
    pub fn text(&self) -> Color {
        if Oklcha::from(self.base).lightness > 0.72 { Ink::Black.on(self) } else { Ink::White.on(self) }
    }
}

/// An outline color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    White,
    /// Not pure black: a deep warm brown-black, which is what print uses.
    Black,
    /// The base's own hue taken nearly to black, so an outline belongs to its
    /// panel. At a fixed lightness, unlike [`Swatch::deep`], so a light cream
    /// gets as firm an edge as a dark blue.
    Dark,
    Highlight,
}

impl Ink {
    pub fn on(self, swatch: &Swatch) -> Color {
        match self {
            Ink::White => Color::oklch(0.98, 0.01, WARM_HUE),
            Ink::Black => Color::oklch(0.17, 0.015, 60.0),
            Ink::Dark => {
                let base = Oklcha::from(swatch.base);
                in_gamut(Oklcha::lch(DARK_INK_LIGHTNESS, base.chroma * 0.8, lean(base.hue, COOL_HUE, 0.15)))
            }
            Ink::Highlight => swatch.highlight,
        }
    }
}

/// Turn `from` a fraction of the short way round toward `to`, in degrees.
fn lean(from: f32, to: f32, amount: f32) -> f32 {
    let delta = (to - from + 540.0).rem_euclid(360.0) - 180.0;
    (from + delta * amount).rem_euclid(360.0)
}

/// Lose chroma until the color fits in sRGB, keeping lightness and hue.
/// Clamping channels instead would shift the hue of a saturated yellow.
fn in_gamut(color: Oklcha) -> Color {
    let fits = |c: Oklcha| {
        let rgb = Srgba::from(c);
        [rgb.red, rgb.green, rgb.blue].iter().all(|v| (-1e-4..=1.0 + 1e-4).contains(v))
    };
    if fits(color) {
        return color.into();
    }
    let (mut low, mut high) = (0.0, color.chroma);
    for _ in 0..20 {
        let mid = (low + high) * 0.5;
        if fits(color.with_chroma(mid)) { low = mid } else { high = mid }
    }
    color.with_chroma(low).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lightness(c: Color) -> f32 {
        Oklcha::from(c).lightness
    }

    #[test]
    fn a_dark_outline_is_darker_than_any_panel_and_keeps_its_hue() {
        for swatch in Team::ALL.map(Swatch::team).into_iter().chain([Swatch::neutral()]) {
            let ink = Ink::Dark.on(&swatch);
            assert!(lightness(ink) < lightness(swatch.shadow), "{swatch:?}");
            let (ink, base) = (Oklcha::from(ink), Oklcha::from(swatch.base));
            let turned = (ink.hue - base.hue + 540.0).rem_euclid(360.0) - 180.0;
            assert!(turned.abs() < 40.0, "{swatch:?} turned {turned} degrees");
        }
    }

    #[test]
    fn a_player_color_makes_a_whole_swatch_and_a_team_outranks_it() {
        for index in 0..24 {
            let color = PlayerColor::nth(0.0, index);
            let swatch = Swatch::player(color);
            assert!(lightness(swatch.highlight) > lightness(swatch.base), "{color:?}");
            assert!(lightness(swatch.base) > lightness(swatch.shadow), "{color:?}");
            assert_eq!(Swatch::of_body(None, Some(color)), swatch);
            assert_eq!(Swatch::of_body(Some(Team::Blue), Some(color)), Swatch::team(Team::Blue));
        }
        assert_eq!(Swatch::of_body(None, None), Swatch::neutral());
    }

    #[test]
    fn every_swatch_is_ordered_light_to_dark() {
        for swatch in Team::ALL.map(Swatch::team).into_iter().chain([Swatch::neutral()]) {
            assert!(lightness(swatch.highlight) > lightness(swatch.base));
            assert!(lightness(swatch.base) > lightness(swatch.shadow));
            assert!(lightness(swatch.shadow) > lightness(swatch.deep));
        }
    }

    #[test]
    fn every_tone_is_displayable() {
        for swatch in Team::ALL.map(Swatch::team) {
            for c in [swatch.base, swatch.highlight, swatch.shadow, swatch.deep] {
                let rgb = c.to_srgba();
                assert!([rgb.red, rgb.green, rgb.blue].iter().all(|v| (-1e-3..=1.001).contains(v)), "{c:?}");
            }
        }
    }

    #[test]
    fn leaning_takes_the_short_way_round() {
        assert!((lean(350.0, 10.0, 0.5) - 0.0).abs() < 1e-4);
        assert!((lean(10.0, 350.0, 0.5) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn yellow_takes_dark_text_and_blue_light() {
        assert_eq!(Swatch::team(Team::Yellow).text(), Ink::Black.on(&Swatch::team(Team::Yellow)));
        assert_eq!(Swatch::team(Team::Blue).text(), Ink::White.on(&Swatch::team(Team::Blue)));
    }
}
