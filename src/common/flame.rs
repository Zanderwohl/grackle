//! Fire: what it is worth, how wide it goes, and how long you burn for
//! afterwards.
//!
//! The third damage source, and the first one whose interesting half happens
//! *after* the hit. A bullet and a rocket are events; fire is a state a body
//! is left in, which is why the numbers here come in two groups — what a puff
//! of flame does when it lands, and what being alight costs you per second for
//! the next few of them.
//!
//! Values and arithmetic only, like [`crate::common::projectile`]: the cone is
//! a function of `(axis, spread, count)` and the burn is a function of
//! `(state, dt)`, so both can be re-run against a rewound world once there is
//! a server to disagree with. In particular the cone carries **no randomness**.
//! A spread drawn from an RNG is the one thing in a shot that cannot be
//! reconciled, and a fixed pattern is also the one you can learn to aim.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::damage::DamageSource;

/// A flame weapon.
///
/// Split the way the fire is: the first four numbers are the puff, the last
/// three are what it leaves behind.
///
/// **How often it puffs is not here.** That was `interval`, and it was a
/// cadence wearing a flame spec's coat — it now lives on
/// [`crate::common::weapon::Mounted`] beside every other button's rate of
/// fire. Leaving both would have been two writers for one number, and the
/// symptom is a data file that is quietly ignored.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct FlameSpec {
    /// How far it reaches, in metres. Short, and deliberately the thing that
    /// makes a flamethrower a flamethrower.
    pub range: f32,
    /// Half-angle of the cone, in radians.
    pub spread: f32,
    /// How many rays one puff is traced as.
    ///
    /// A sampling rate, not a damage multiplier: a body caught by four of them
    /// takes one puff's worth, because how finely the cone is sampled is an
    /// implementation detail and must not be worth anything.
    pub rays: u32,
    /// What one puff takes off a body it catches.
    pub damage: u32,
    /// How long a body burns for after being caught, in seconds.
    pub afterburn_duration: f32,
    /// What each tick of afterburn takes off.
    pub afterburn_damage: u32,
    /// Seconds between afterburn ticks.
    pub afterburn_interval: f32,
}

impl FlameSpec {
    /// The one flame weapon there is.
    ///
    /// Ten puffs a second, so the direct damage is small and arrives as a
    /// stream; the afterburn is twelve ticks of four over six seconds, which
    /// is worth more than standing in the fire was. That is the character of
    /// the thing — it punishes leaving as much as staying.
    pub const FLAMETHROWER: Self = Self {
        range: 6.0,
        spread: 0.28,
        rays: 5,
        damage: 7,
        afterburn_duration: 6.0,
        afterburn_damage: 4,
        afterburn_interval: 0.5,
    };
}

/// A body that is on fire, and who set it alight.
///
/// The "last burned by" tag as well as the timer: afterburn has to be credited
/// to somebody, and the last person to light you is the answer everywhere —
/// a kill feed, a scoreboard, and whatever revenge system turns up later. It
/// lives on the burning body rather than in a list held by the flamethrower,
/// so a shooter who leaves the match does not take the fire with them.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Burning {
    /// Who lit it *most recently*. Overwritten on every re-light.
    pub source: DamageSource,
    /// Seconds of fire left.
    pub remaining: f32,
    /// What each tick takes off.
    pub damage: u32,
    /// Seconds between ticks.
    pub interval: f32,
    /// Seconds until the next tick.
    pub until_next: f32,
}

impl Burning {
    /// Catch fire.
    pub fn lit(spec: &FlameSpec, source: DamageSource) -> Self {
        Self {
            source,
            remaining: spec.afterburn_duration,
            damage: spec.afterburn_damage,
            interval: spec.afterburn_interval,
            until_next: spec.afterburn_interval,
        }
    }

    /// Caught again, while already alight.
    ///
    /// The timer is **reset, not extended and not maxed**: a body caught by a
    /// short-burning weapon while a long burn is running comes out of it with
    /// the short one. That is the rule because the alternative is worse in
    /// both directions — a `max` makes the last hit sometimes do nothing, and
    /// an extension makes standing in a flame for five seconds set you alight
    /// for thirty.
    ///
    /// What is deliberately *not* touched is [`Burning::until_next`]. A
    /// flamethrower re-lights ten times a second, so a re-light that pushed
    /// the next tick out would be a burn that never ticks at all for as long
    /// as somebody keeps burning you — which is exactly backwards.
    pub fn relight(&mut self, spec: &FlameSpec, source: DamageSource) {
        self.source = source;
        self.remaining = spec.afterburn_duration;
        self.damage = spec.afterburn_damage;
        self.interval = spec.afterburn_interval;
    }

    /// Advance by `dt`, and say how many ticks fell due.
    ///
    /// Several, in principle: a long step or a short interval can cover more
    /// than one, and dropping the extras would make afterburn quietly cheaper
    /// on a slow machine. An interval of zero yields no ticks rather than
    /// looping forever.
    pub fn tick(&mut self, dt: f32) -> u32 {
        self.remaining -= dt;
        if self.interval <= 0.0 {
            return 0;
        }

        self.until_next -= dt;
        let mut due = 0;
        while self.until_next <= 0.0 {
            due += 1;
            self.until_next += self.interval;
        }
        due
    }

    /// Whether the fire has burned out.
    pub fn is_out(&self) -> bool {
        self.remaining <= 0.0
    }
}

/// How far apart two rays of a cone are placed, in radians of turn.
///
/// The golden angle. Any fixed turn would be deterministic, which is the
/// requirement; this one is the turn that does not line the samples up into
/// spokes, so five rays cover five different parts of the cone rather than
/// three of them landing on one radius.
const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// `count` directions spread evenly through a cone of half-angle `spread`
/// about `axis`.
///
/// **The first is always the axis itself**, so a weapon traced with one ray is
/// a weapon that shoots where it is pointed, and the middle of the cone is
/// never a gap. The rest spiral outwards with the angle from the axis going as
/// the square root of the index, which is what spreads them evenly over the
/// cone's *area* rather than bunching them at the centre.
pub fn cone_directions(axis: Dir3, spread: f32, count: u32) -> Vec<Dir3> {
    if count == 0 {
        return Vec::new();
    }

    // Deterministic, and that is the whole requirement: two machines tracing
    // the same shot have to pick the same rays.
    let (right, up) = axis.any_orthonormal_pair();

    (0..count)
        .map(|i| {
            let t = if count == 1 { 0.0 } else { i as f32 / (count - 1) as f32 };
            let from_axis = spread * t.sqrt();
            let turn = i as f32 * GOLDEN_ANGLE;

            let offset = right * turn.cos() + up * turn.sin();
            let direction = *axis * from_axis.cos() + offset * from_axis.sin();
            // A unit vector built from an orthonormal basis is a unit vector;
            // the fallback is unreachable arithmetic rather than a case.
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

    /// The middle of the cone is never a gap: whatever it is sampled with,
    /// the weapon shoots where it is pointed.
    #[test]
    fn the_first_ray_is_the_one_you_aimed() {
        for count in 1..12 {
            let rays = cone_directions(Dir3::NEG_Z, 0.3, count);
            assert_eq!(rays.len(), count as usize);
            assert!((rays[0].as_vec3() - Vec3::NEG_Z).length() < 1e-5, "{count} rays: {:?}", rays[0]);
        }
    }

    /// Nothing leaves the cone. A ray outside it is a weapon with a longer
    /// reach than it draws.
    #[test]
    fn every_ray_stays_inside_the_cone() {
        let spread = 0.3;
        for ray in cone_directions(Dir3::NEG_Z, spread, 9) {
            let angle = ray.angle_between(Vec3::NEG_Z);
            assert!(angle <= spread + 1e-5, "a ray left the cone at {angle} rad");
        }
    }

    /// And they are not all the axis, which is the other way this could
    /// trivially "pass".
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

    /// No RNG anywhere: the same shot traces the same rays, which is what a
    /// server has to be able to re-run.
    #[test]
    fn the_same_shot_picks_the_same_rays_every_time() {
        let axis = Dir3::new(Vec3::new(0.3, -0.2, -1.0)).unwrap();
        assert_eq!(cone_directions(axis, 0.25, 7), cone_directions(axis, 0.25, 7));
    }

    /// Afterburn ticks on its interval and stops when the fire is out.
    #[test]
    fn a_burn_ticks_on_its_interval_and_then_goes_out() {
        let mut burn = Burning::lit(&FlameSpec::FLAMETHROWER, ME);
        let dt = 1.0 / 64.0;

        let mut ticks = 0;
        // Well past the six seconds, to check it does not keep ticking.
        for _ in 0..(64 * 8) {
            if burn.is_out() {
                break;
            }
            ticks += burn.tick(dt);
        }

        let expected = (FlameSpec::FLAMETHROWER.afterburn_duration
            / FlameSpec::FLAMETHROWER.afterburn_interval) as u32;
        assert_eq!(ticks, expected, "twelve ticks of four over six seconds");
    }

    /// Re-lighting resets the timer rather than taking whichever is longer —
    /// a short burn on top of a long one is a short burn.
    #[test]
    fn relighting_resets_the_timer_even_when_it_shortens_it() {
        let long = FlameSpec { afterburn_duration: 20.0, ..FlameSpec::FLAMETHROWER };
        let short = FlameSpec { afterburn_duration: 2.0, ..FlameSpec::FLAMETHROWER };

        let mut burn = Burning::lit(&long, ME);
        burn.relight(&short, YOU);

        assert_eq!(burn.remaining, 2.0, "the longer burn won");
        assert_eq!(burn.source, YOU, "the credit stayed with whoever lit it first");
    }

    /// The subtle one: a flamethrower re-lights ten times a second, and a
    /// re-light that also reset the tick clock would be a burn that never
    /// ticks while somebody keeps burning you.
    #[test]
    fn being_burned_continuously_still_ticks() {
        let spec = FlameSpec::FLAMETHROWER;
        let mut burn = Burning::lit(&spec, ME);
        let dt = 1.0 / 64.0;

        let mut ticks = 0;
        // Two seconds of being held in the fire, re-lit every puff.
        for step in 0..(64 * 2) {
            ticks += burn.tick(dt);
            if step % 6 == 0 {
                burn.relight(&spec, ME);
            }
        }

        assert!(ticks >= 3, "held in the fire for two seconds and burned {ticks} times");
    }

    /// A degenerate interval is a burn that does not tick, not a hang.
    #[test]
    fn an_interval_of_zero_does_not_loop_forever() {
        let mut burn = Burning::lit(
            &FlameSpec { afterburn_interval: 0.0, ..FlameSpec::FLAMETHROWER },
            ME,
        );
        assert_eq!(burn.tick(1.0), 0);
        assert_eq!(burn.remaining, FlameSpec::FLAMETHROWER.afterburn_duration - 1.0);
    }
}
