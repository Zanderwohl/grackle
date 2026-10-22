//! What a projectile is, as numbers, and the arithmetic of moving one.
//!
//! One type covers rockets, pipe bombs and anything else thrown rather than
//! traced: they differ only in a curve, a bounce and a fuse, and two systems
//! would mean two copies of the swept move and the splash.
//!
//! No `World` here: the step is a function of `(state, spec, dt)`, so client
//! and server reach the same position. Gravity is passed in because it is a
//! map's setting, [`crate::common::movement_rules::MovementRules`].

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Everything that distinguishes one thrown weapon from another.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct ProjectileSpec {
    /// Metres per second.
    pub initial_speed: f32,
    /// Fraction of gravity felt: `0.0` flies flat, `1.0` arcs.
    pub gravity_factor: f32,
    /// Fraction of speed shed per second.
    pub air_resistance: f32,
    /// Radians per second. Cosmetic; nothing tests against a projectile's rotation.
    pub spin_rate: f32,
    /// Walls it may bounce off before the next one is its last. `0` makes the
    /// first contact the last.
    pub max_bounces: u32,
    /// Fraction of speed that survives a bounce.
    pub restitution: f32,
    /// Seconds of game time.
    pub max_lifetime: f32,
    /// Whether running out of bounces detonates it rather than stopping it.
    pub explodes_at_last_bounce: bool,
    /// Whether the fuse running out detonates it.
    pub explodes_at_lifetime_end: bool,
    /// Metres. Splash and shove fall off to zero here
    /// ([`crate::common::damage::falloff`]).
    pub explosion_radius: f32,
    /// Taken by the body hit directly, instead of splash rather than on top of it.
    pub damage_direct_hit: u32,
    /// Splash at the centre; others in the radius take a share.
    pub damage_splash: u32,
    /// Speed given to a corpse at the centre, in metres per second; a speed, not
    /// an impulse (see [`crate::game::ragdoll::RagdollShove`]).
    pub knockback: f32,
    /// Metres. How big it is drawn and how far off a wall it stops.
    pub radius: f32,
}

/// Test fixtures; the game reads its projectiles from `weapons.toml`.
#[cfg(test)]
impl ProjectileSpec {
    pub const ROCKET: Self = Self {
        initial_speed: 30.0,
        gravity_factor: 0.0,
        air_resistance: 0.0,
        spin_rate: 0.0,
        max_bounces: 0,
        restitution: 0.0,
        max_lifetime: 8.0,
        explodes_at_last_bounce: true,
        explodes_at_lifetime_end: false,
        explosion_radius: 3.0,
        damage_direct_hit: 90,
        damage_splash: 60,
        knockback: 9.0,
        radius: 0.12,
    };

    pub const RPG: Self = Self {
        gravity_factor: 1.0,
        initial_speed: 34.0,
        ..Self::ROCKET
    };

    pub const PIPE_BOMB: Self = Self {
        initial_speed: 24.0,
        gravity_factor: 1.0,
        air_resistance: 0.1,
        spin_rate: 12.0,
        max_bounces: 4,
        restitution: 0.45,
        max_lifetime: 2.3,
        explodes_at_last_bounce: true,
        explodes_at_lifetime_end: true,
        explosion_radius: 3.4,
        damage_direct_hit: 100,
        damage_splash: 60,
        knockback: 10.0,
        radius: 0.14,
    };
}

/// One step of flight, ignoring collisions. Returns `(velocity, displacement)`;
/// the caller must sweep the displacement against the world.
///
/// Euler at the fixed rate, for reproducibility rather than accuracy. Drag is
/// clamped so a large coefficient stops the projectile rather than reversing it.
pub fn fly(velocity: Vec3, spec: &ProjectileSpec, gravity: f32, dt: f32) -> (Vec3, Vec3) {
    let mut moved = velocity + Vec3::Y * (gravity * spec.gravity_factor * dt);
    moved *= (1.0 - spec.air_resistance * dt).clamp(0.0, 1.0);
    (moved, moved * dt)
}

/// Mirrors `velocity` about the face `normal` and scales by `restitution`.
/// No friction along the surface.
pub fn bounce(velocity: Vec3, normal: Vec3, restitution: f32) -> Vec3 {
    (velocity - 2.0 * velocity.dot(normal) * normal) * restitution
}

/// Why a projectile's flight ended, so detonation is decided from the spec in
/// one place ([`ProjectileSpec::detonates_on`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// Hit a body. Always detonates.
    DirectHit,
    /// Hit a wall with no bounces left.
    LastBounce,
    Fuse,
}

impl ProjectileSpec {
    pub fn detonates_on(&self, ending: Ending) -> bool {
        match ending {
            Ending::DirectHit => true,
            Ending::LastBounce => self.explodes_at_last_bounce,
            Ending::Fuse => self.explodes_at_lifetime_end,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAVITY: f32 = -20.0;
    const DT: f32 = 1.0 / 64.0;

    #[test]
    fn a_rocket_does_not_fall_and_does_not_slow_down() {
        let mut velocity = Vec3::NEG_Z * ProjectileSpec::ROCKET.initial_speed;
        for _ in 0..64 {
            let (next, _) = fly(velocity, &ProjectileSpec::ROCKET, GRAVITY, DT);
            velocity = next;
        }
        assert!(velocity.y.abs() < 1e-5, "the rocket fell: {velocity}");
        assert!(
            (velocity.length() - ProjectileSpec::ROCKET.initial_speed).abs() < 1e-3,
            "the rocket slowed down: {velocity}"
        );
    }

    #[test]
    fn a_pipe_bomb_falls_and_slows_down() {
        let launched = Vec3::NEG_Z * ProjectileSpec::PIPE_BOMB.initial_speed;
        let mut velocity = launched;
        for _ in 0..64 {
            let (next, _) = fly(velocity, &ProjectileSpec::PIPE_BOMB, GRAVITY, DT);
            velocity = next;
        }
        assert!(velocity.y < -15.0, "the pipe did not fall: {velocity}");
        assert!(velocity.z > launched.z, "the pipe did not slow down: {velocity}");
    }

    /// Drag never turns a projectile around, however large the coefficient or step.
    #[test]
    fn drag_cannot_reverse_a_projectile() {
        let spec = ProjectileSpec { air_resistance: 1000.0, ..ProjectileSpec::PIPE_BOMB };
        let (velocity, moved) = fly(Vec3::NEG_Z * 30.0, &spec, 0.0, 1.0);
        assert_eq!(velocity, Vec3::ZERO);
        assert_eq!(moved, Vec3::ZERO);
    }

    #[test]
    fn a_bounce_mirrors_the_velocity_about_the_face() {
        let bounced = bounce(Vec3::new(0.0, -10.0, 0.0), Vec3::Y, 0.5);
        assert!((bounced - Vec3::new(0.0, 5.0, 0.0)).length() < 1e-5, "{bounced}");
    }

    #[test]
    fn a_glancing_bounce_keeps_its_sideways_motion() {
        let bounced = bounce(Vec3::new(6.0, -6.0, 0.0), Vec3::Y, 1.0);
        assert!((bounced - Vec3::new(6.0, 6.0, 0.0)).length() < 1e-5, "{bounced}");
    }

    #[test]
    fn each_weapon_goes_off_when_its_own_kind_should() {
        let rocket = ProjectileSpec::ROCKET;
        assert!(rocket.detonates_on(Ending::DirectHit));
        assert!(rocket.detonates_on(Ending::LastBounce), "a rocket must go off on the wall it hits");
        assert!(!rocket.detonates_on(Ending::Fuse), "a rocket has no fuse");

        let pipe = ProjectileSpec::PIPE_BOMB;
        assert!(pipe.detonates_on(Ending::DirectHit));
        assert!(pipe.detonates_on(Ending::Fuse), "a pipe bomb is a fuse");
    }

    #[test]
    fn the_rpg_differs_from_the_rocket_only_in_falling_and_speed() {
        let flattened = ProjectileSpec {
            gravity_factor: ProjectileSpec::ROCKET.gravity_factor,
            initial_speed: ProjectileSpec::ROCKET.initial_speed,
            ..ProjectileSpec::RPG
        };
        assert_eq!(flattened, ProjectileSpec::ROCKET);
    }
}
