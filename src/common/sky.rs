//! The map's sky: what a slot painted [`SKYBOX`] draws through to, set per map as a time of
//! day. A gradient by elevation, mixed in Oklab so a sunset's orange runs into its blue without
//! passing through gray. The GPU half is [`crate::material::sky`], which mixes the same stops the
//! same way.
//!
//! [`publish_sky`] is the only writer of the [`SkyTime`] resource; nothing reads `MapMetadata` for
//! it.

use std::fmt::Display;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::editor::map_metadata::MapMetadata;
use crate::get;

/// The material name a slot gives to be drawn as sky. Shadows a pack material of the same name.
pub const SKYBOX: &str = "skybox";

/// Stops per gradient, fixed so the shader's uniform is one shape.
pub const STOPS: usize = 5;

#[derive(Resource, Copy, Clone, Debug, Default, PartialEq, Eq, EnumIter, Serialize, Deserialize)]
pub enum SkyTime {
    HighNoon,
    #[default]
    Day,
    Evening,
    Night,
}

impl SkyTime {
    /// Elevation — the `y` of a unit direction, -1 straight down to 1 straight up — and the sRGB
    /// color there, ascending. Below the horizon is haze rather than ground: it is only seen
    /// looking down through a wall.
    pub fn stops(self) -> [(f32, Srgba); STOPS] {
        let hex = |s| Srgba::hex(s).expect("a sky stop is a valid hex color");
        match self {
            SkyTime::HighNoon => [
                (-1.0, hex("6f8296")),
                (0.0, hex("d6ecf8")),
                (0.08, hex("b4daf6")),
                (0.35, hex("6faeea")),
                (1.0, hex("1f5fcf")),
            ],
            SkyTime::Day => [
                (-1.0, hex("7a8a9c")),
                (0.0, hex("e6eff2")),
                (0.1, hex("c4dcec")),
                (0.4, hex("8cbce6")),
                (1.0, hex("3f7fd2")),
            ],
            SkyTime::Evening => [
                (-1.0, hex("2a2438")),
                (0.0, hex("ffb36a")),
                (0.1, hex("f0806a")),
                (0.35, hex("8e5f9e")),
                (1.0, hex("232e6a")),
            ],
            SkyTime::Night => [
                (-1.0, hex("030409")),
                (0.0, hex("1d2a4a")),
                (0.12, hex("121d3a")),
                (0.4, hex("0a1228")),
                (1.0, hex("02040c")),
            ],
        }
    }

    /// Each stop in Oklab, with its elevation: what the shader is handed.
    pub fn oklab_stops(self) -> [(f32, Oklaba); STOPS] {
        self.stops().map(|(elevation, color)| (elevation, color.into()))
    }

    /// The color looking in a direction whose `y` is `elevation`. The CPU twin of the shader, for
    /// previews and tests.
    pub fn color_at(self, elevation: f32) -> LinearRgba {
        let stops = self.oklab_stops();
        let mut color = stops[0].1;
        for pair in stops.windows(2) {
            let ((lo, from), (hi, to)) = (pair[0], pair[1]);
            if elevation > lo {
                color = from.mix(&to, ((elevation - lo) / (hi - lo).max(1e-5)).clamp(0.0, 1.0));
            }
        }
        color.into()
    }
}

impl Display for SkyTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkyTime::HighNoon => write!(f, "{}", get!("sky.high_noon")),
            SkyTime::Day => write!(f, "{}", get!("sky.day")),
            SkyTime::Evening => write!(f, "{}", get!("sky.evening")),
            SkyTime::Night => write!(f, "{}", get!("sky.night")),
        }
    }
}

/// Where the sun is, set by the mapper and not derived from the [`SkyTime`]: which way light falls
/// on a map is theirs to choose.
#[derive(Resource, Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SunPosition {
    /// Degrees above the horizon.
    pub altitude: f32,
    /// Degrees clockwise from north (`-Z`) seen from above, so 90 is east (`+X`).
    pub azimuth: f32,
}

impl Default for SunPosition {
    fn default() -> Self {
        Self { altitude: 50.0, azimuth: 135.0 }
    }
}

impl SunPosition {
    /// Unit, from the ground toward the sun.
    pub fn toward(self) -> Vec3 {
        let (altitude, azimuth) = (self.altitude.to_radians(), self.azimuth.to_radians());
        Vec3::new(azimuth.sin() * altitude.cos(), altitude.sin(), -azimuth.cos() * altitude.cos())
    }
}

/// Ungated, as [`publish_palette`](crate::common::palette::publish_palette) is.
pub fn publish_sky(metadata: Res<MapMetadata>, mut sky: ResMut<SkyTime>, mut sun: ResMut<SunPosition>) {
    if metadata.is_changed() {
        sky.set_if_neq(metadata.sky);
        sun.set_if_neq(metadata.sun);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    /// The shader takes the last segment a direction is above, which is only the right one if
    /// the stops ascend, and it clamps rather than extrapolates, so they have to span every
    /// direction.
    #[test]
    fn every_sky_covers_every_direction_in_order() {
        for sky in SkyTime::iter() {
            let stops = sky.stops();
            assert_eq!(stops[0].0, -1.0, "{sky:?}");
            assert_eq!(stops[STOPS - 1].0, 1.0, "{sky:?}");
            assert!(stops.windows(2).all(|pair| pair[0].0 < pair[1].0), "{sky:?} has stops out of order");
        }
    }

    #[test]
    fn a_sky_is_its_authored_color_at_each_stop() {
        for sky in SkyTime::iter() {
            for (elevation, authored) in sky.stops() {
                let drawn = Srgba::from(sky.color_at(elevation));
                let close = (drawn.red - authored.red).abs() < 1e-3
                    && (drawn.green - authored.green).abs() < 1e-3
                    && (drawn.blue - authored.blue).abs() < 1e-3;
                assert!(close, "{sky:?} at {elevation} is {drawn:?}, authored {authored:?}");
            }
        }
    }

    #[test]
    fn the_sun_is_where_its_angles_say() {
        let east = SunPosition { altitude: 0.0, azimuth: 90.0 }.toward();
        assert!(east.abs_diff_eq(Vec3::X, 1e-6), "{east}");
        let north = SunPosition { altitude: 0.0, azimuth: 0.0 }.toward();
        assert!(north.abs_diff_eq(Vec3::NEG_Z, 1e-6), "{north}");
        let overhead = SunPosition { altitude: 90.0, azimuth: 200.0 }.toward();
        assert!(overhead.abs_diff_eq(Vec3::Y, 1e-6), "{overhead}");
    }

    /// Mixed in sRGB, salmon into purple sags through a duller brown; in Oklab the midpoint keeps
    /// its chroma.
    #[test]
    fn a_sunset_does_not_go_muddy_between_stops() {
        let [_, _, (lo, from), (hi, to), _] = SkyTime::Evening.stops();
        let chroma = |c: Srgba| Oklcha::from(c).chroma;
        let middle = chroma(SkyTime::Evening.color_at((lo + hi) / 2.0).into());
        assert!(middle >= chroma(from).min(chroma(to)) * 0.9, "chroma {middle} between {from:?} and {to:?}");
    }
}
