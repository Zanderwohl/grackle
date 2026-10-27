//! A colour per player, for telling bodies apart where teams do not. See
//! "Body colour" in [documentation/arena.md](../../documentation/arena.md).
//!
//! Decided once on the authority and replicated, never derived: two machines
//! agreeing on join order is a coincidence.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Fixed so only the hue says who somebody is; about as light and as muted as
/// [`Team::colour`](crate::common::team::Team::colour), so every player reads
/// as well under map lighting as a team does.
const LIGHTNESS: f32 = 0.62;
/// About the most every hue holds inside sRGB at this lightness; cyan runs
/// out first.
const CHROMA: f32 = 0.10;

/// Hues handed out before the wheel starts filling in between them.
const FIRST_ROUND: usize = 12;

/// A hue in degrees on the OKLCH wheel.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerColour {
    pub hue: f32,
}

impl PlayerColour {
    /// `start` in degrees, chosen per match; `index` as from [`lowest_free`].
    pub fn nth(start: f32, index: usize) -> Self {
        Self { hue: (start + hue_offset(index)).rem_euclid(360.0) }
    }

    pub fn colour(self) -> Color {
        Color::oklch(LIGHTNESS, CHROMA, self.hue)
    }
}

/// 30° apart for the first twelve, then the 15° points between them, then
/// 7.5°, each round halving the step, so a hue once handed out never moves.
pub fn hue_offset(index: usize) -> f32 {
    let step = 360.0 / FIRST_ROUND as f32;
    if index < FIRST_ROUND {
        return index as f32 * step;
    }
    let (mut first, mut count, mut half) = (FIRST_ROUND, FIRST_ROUND, step / 2.0);
    while index >= first + count {
        first += count;
        count *= 2;
        half /= 2.0;
    }
    (2 * (index - first) + 1) as f32 * half
}

/// So somebody leaving frees their hue for the next to join.
pub fn lowest_free(taken: impl IntoIterator<Item = usize>) -> usize {
    let taken: Vec<usize> = taken.into_iter().collect();
    (0..).find(|index| !taken.contains(index)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_twelve_are_thirty_degrees_apart_and_the_next_fill_in() {
        let offsets: Vec<f32> = (0..24).map(hue_offset).collect();
        assert_eq!(&offsets[..3], &[0.0, 30.0, 60.0]);
        assert_eq!(offsets[11], 330.0);
        assert_eq!(&offsets[12..14], &[15.0, 45.0]);
        assert_eq!(offsets[23], 345.0);
        assert_eq!(hue_offset(24), 7.5);
        assert_eq!(hue_offset(47), 352.5);
    }

    #[test]
    fn no_two_of_the_first_hundred_share_a_hue() {
        let mut hues: Vec<f32> = (0..100).map(hue_offset).collect();
        hues.sort_by(f32::total_cmp);
        hues.dedup();
        assert_eq!(hues.len(), 100);
        assert!(hues.iter().all(|hue| (0.0..360.0).contains(hue)));
    }

    #[test]
    fn a_freed_hue_is_handed_out_again() {
        assert_eq!(lowest_free([]), 0);
        assert_eq!(lowest_free([0, 1, 3]), 2);
        assert_eq!(lowest_free([1, 0]), 2);
    }

    /// Out of gamut is clipped per channel on the way to the screen, which
    /// shifts the hue and can make two players look alike.
    #[test]
    fn every_hue_is_in_the_srgb_gamut() {
        for index in 0..48 {
            let colour = PlayerColour::nth(0.0, index);
            let rgb = Srgba::from(colour.colour());
            for channel in [rgb.red, rgb.green, rgb.blue] {
                assert!((-1.0e-3..=1.0 + 1.0e-3).contains(&channel), "{colour:?} is {rgb:?}");
            }
        }
    }
}
