//! Fire: the puff, and the afterburn it leaves.
//!
//! Values and pure functions only, so both can be re-run against a rewound
//! world. The cone has no randomness: an RNG spread cannot be reconciled, and
//! a fixed pattern can be learned.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::damage::DamageSource;
use crate::common::weapon::{distance_scale, DistanceScale};

/// Rate of fire is on [`crate::common::weapon::Mounted`], not here.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct FlameSpec {
    #[serde(rename = "range")]
    pub range_m: f32,
    /// Half-angle, radians.
    pub spread: f32,
    /// A sampling rate, not a multiplier: a body hit by several rays takes one
    /// puff.
    pub rays: u32,
    pub damage: u32,
    #[serde(rename = "afterburn_duration")]
    pub afterburn_duration_s: f32,
    pub afterburn_damage: u32,
    /// Between afterburn ticks.
    #[serde(rename = "afterburn_interval")]
    pub afterburn_interval_s: f32,
    /// On a puff's damage, by meters from the eye to the hit. Not on the afterburn it leaves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ramp_up: Option<DistanceScale>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub falloff: Option<DistanceScale>,
}

impl FlameSpec {
    /// The afterburn (twelve ticks of four) is worth more than the direct
    /// damage, so leaving the fire is punished as much as staying.
    pub const FLAMETHROWER: Self = Self {
        range_m: 6.0,
        spread: 0.28,
        rays: 5,
        damage: 7,
        afterburn_duration_s: 6.0,
        afterburn_damage: 4,
        afterburn_interval_s: 0.5,
        ramp_up: None,
        falloff: None,
    };

    /// What a puff `distance` meters from the eye is worth, times `damage`.
    pub fn scale_at(&self, distance: f32) -> f32 {
        distance_scale(self.ramp_up, self.falloff, distance)
    }
}

/// On the burning body rather than the shooter, so the fire outlives a
/// shooter who leaves.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Burning {
    /// Whoever lit it most recently; afterburn is credited to them.
    pub source: DamageSource,
    pub remaining_s: f32,
    pub damage: u32,
    pub interval_s: f32,
    pub until_next_s: f32,
}

impl Burning {
    pub fn lit(spec: &FlameSpec, source: DamageSource) -> Self {
        Self {
            source,
            remaining_s: spec.afterburn_duration_s,
            damage: spec.afterburn_damage,
            interval_s: spec.afterburn_interval_s,
            until_next_s: spec.afterburn_interval_s,
        }
    }

    /// Resets the timer rather than extending or maxing it: `max` makes the
    /// last hit sometimes do nothing, and extending makes five seconds in the
    /// flame burn for thirty.
    ///
    /// Leaves [`Burning::until_next`] alone: re-lit ten times a second, a reset
    /// tick clock would never tick.
    pub fn relight(&mut self, spec: &FlameSpec, source: DamageSource) {
        self.source = source;
        self.remaining_s = spec.afterburn_duration_s;
        self.damage = spec.afterburn_damage;
        self.interval_s = spec.afterburn_interval_s;
    }

    /// Returns how many ticks fell due, possibly several, so a slow frame does
    /// not make afterburn cheaper. A zero interval yields none rather than
    /// looping forever.
    pub fn tick(&mut self, dt: f32) -> u32 {
        self.remaining_s -= dt;
        if self.interval_s <= 0.0 {
            return 0;
        }

        self.until_next_s -= dt;
        let mut due = 0;
        while self.until_next_s <= 0.0 {
            due += 1;
            self.until_next_s += self.interval_s;
        }
        due
    }

    pub fn is_out(&self) -> bool {
        self.remaining_s <= 0.0
    }
}

/// Radians of turn between successive rays. The golden angle never lines
/// samples up into spokes.
const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// `count` directions through a cone of half-angle `spread` about `axis`.
///
/// The first is always the axis, so the middle is never a gap. The angle from
/// the axis goes as the square root of the index, which spreads rays evenly by
/// area rather than bunching them at the center.
pub fn cone_directions(axis: Dir3, spread: f32, count: u32) -> Vec<Dir3> {
    if count == 0 {
        return Vec::new();
    }

    let (right, up) = axis.any_orthonormal_pair();

    (0..count)
        .map(|i| {
            let t = if count == 1 { 0.0 } else { i as f32 / (count - 1) as f32 };
            let from_axis = spread * t.sqrt();
            let turn = i as f32 * GOLDEN_ANGLE;

            let offset = right * turn.cos() + up * turn.sin();
            let direction = *axis * from_axis.cos() + offset * from_axis.sin();
            // Unit by construction; the fallback is unreachable.
            Dir3::new(direction).unwrap_or(axis)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::damage::PlayerId;

    const ME: DamageSource = DamageSource::Player(PlayerId(1));
    const YOU: DamageSource = DamageSource::Player(PlayerId(2));

    #[test]
    fn the_first_ray_is_the_one_you_aimed() {
        for count in 1..12 {
            let rays = cone_directions(Dir3::NEG_Z, 0.3, count);
            assert_eq!(rays.len(), count as usize);
            assert!((rays[0].as_vec3() - Vec3::NEG_Z).length() < 1e-5, "{count} rays: {:?}", rays[0]);
        }
    }

    #[test]
    fn every_ray_stays_inside_the_cone() {
        let spread = 0.3;
        for ray in cone_directions(Dir3::NEG_Z, spread, 9) {
            let angle = ray.angle_between(Vec3::NEG_Z);
            assert!(angle <= spread + 1e-5, "a ray left the cone at {angle} rad");
        }
    }

    #[test]
    fn the_rays_actually_spread_out() {
        let spread = 0.3;
        let rays = cone_directions(Dir3::NEG_Z, spread, 5);
        let widest = rays
            .iter()
            .map(|ray| ray.angle_between(Vec3::NEG_Z))
            .fold(0.0, f32::max);
        assert!(widest > spread * 0.9, "the widest ray was only {widest} rad from the axis");
    }

    #[test]
    fn the_same_shot_picks_the_same_rays_every_time() {
        let axis = Dir3::new(Vec3::new(0.3, -0.2, -1.0)).unwrap();
        assert_eq!(cone_directions(axis, 0.25, 7), cone_directions(axis, 0.25, 7));
    }

    #[test]
    fn a_burn_ticks_on_its_interval_and_then_goes_out() {
        let mut burn = Burning::lit(&FlameSpec::FLAMETHROWER, ME);
        let dt = 1.0 / 64.0;

        let mut ticks = 0;
        // Past the burn's end, to check it stops.
        for _ in 0..(64 * 8) {
            if burn.is_out() {
                break;
            }
            ticks += burn.tick(dt);
        }

        let expected = (FlameSpec::FLAMETHROWER.afterburn_duration_s
            / FlameSpec::FLAMETHROWER.afterburn_interval_s) as u32;
        assert_eq!(ticks, expected, "twelve ticks of four over six seconds");
    }

    #[test]
    fn relighting_resets_the_timer_even_when_it_shortens_it() {
        let long = FlameSpec { afterburn_duration_s: 20.0, ..FlameSpec::FLAMETHROWER };
        let short = FlameSpec { afterburn_duration_s: 2.0, ..FlameSpec::FLAMETHROWER };

        let mut burn = Burning::lit(&long, ME);
        burn.relight(&short, YOU);

        assert_eq!(burn.remaining_s, 2.0, "the longer burn won");
        assert_eq!(burn.source, YOU, "the credit stayed with whoever lit it first");
    }

    /// Fails if re-lighting resets the tick clock, which stops a continuous
    /// burn from ever ticking.
    #[test]
    fn being_burned_continuously_still_ticks() {
        let spec = FlameSpec::FLAMETHROWER;
        let mut burn = Burning::lit(&spec, ME);
        let dt = 1.0 / 64.0;

        let mut ticks = 0;
        for step in 0..(64 * 2) {
            ticks += burn.tick(dt);
            if step % 6 == 0 {
                burn.relight(&spec, ME);
            }
        }

        assert!(ticks >= 3, "held in the fire for two seconds and burned {ticks} times");
    }

    #[test]
    fn an_interval_of_zero_does_not_loop_forever() {
        let mut burn = Burning::lit(
            &FlameSpec { afterburn_interval_s: 0.0, ..FlameSpec::FLAMETHROWER },
            ME,
        );
        assert_eq!(burn.tick(1.0), 0);
        assert_eq!(burn.remaining_s, FlameSpec::FLAMETHROWER.afterburn_duration_s - 1.0);
    }
}
