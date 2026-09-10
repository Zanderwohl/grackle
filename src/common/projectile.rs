//! What a projectile *is*, as numbers, and the arithmetic of moving one.
//!
//! One type covers the rocket launcher, the pipe bomb launcher and whatever
//! else is thrown rather than traced, because the three things that actually
//! differ between them are a curve, a bounce and a fuse. A grenade is a rocket
//! that falls, spins, bounces four times and goes off on a timer; a rocket is a
//! grenade that flies straight and goes off on contact. Writing them as two
//! systems would mean two copies of the swept move, the splash and the
//! provenance, and the second copy is where the bugs live.
//!
//! Deliberately a plain value with no `World` in it, for the same reason
//! [`crate::common::hitscan::trace`] is: the step is a function of
//! `(state, spec, dt)`, so a client and a server fed the same values put the
//! projectile in the same place. A spec that read the clock or an RNG could
//! not be reconciled.
//!
//! **Gravity is a parameter, not a constant here.** The number lives with the
//! player's step in [`crate::game::player::GRAVITY`], and a second opinion
//! about how hard things fall is exactly the kind of drift that leaves a rocket
//! arcing differently from the man who fired it.

use bevy::prelude::*;

/// Everything that distinguishes one thrown weapon from another.
///
/// A `const` per weapon rather than a trait per weapon: adding the flare gun
/// should be adding thirteen numbers, not a type with thirteen methods that
/// return them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectileSpec {
    /// How fast it leaves the muzzle, in metres per second.
    pub initial_speed: f32,
    /// How much of gravity it feels. `0.0` for a rocket that flies flat, `1.0`
    /// for anything that arcs.
    pub gravity_factor: f32,
    /// Drag, as the fraction of its speed it sheds per second. `0.0` for a
    /// rocket, which is under power the whole way.
    pub air_resistance: f32,
    /// How fast it tumbles, in radians per second. Cosmetic — nothing tests
    /// against a projectile's rotation — but it is the difference between a
    /// rocket that points where it is going and a pipe that end-over-ends.
    pub spin_rate: f32,
    /// How many walls it may bounce off before the next one is its last.
    /// `0` for a rocket, so its first contact is already its last.
    pub max_bounces: u32,
    /// How much speed survives a bounce, as a fraction. Meaningless at
    /// `max_bounces == 0`, and set to zero there rather than left to look like
    /// a rocket has an opinion about being bouncy.
    pub restitution: f32,
    /// How long it may live, in seconds of game time.
    pub max_lifetime: f32,
    /// Whether running out of bounces detonates it, rather than simply
    /// stopping it. True for a rocket — its "last bounce" is the wall it hits.
    pub explodes_at_last_bounce: bool,
    /// Whether the fuse running out detonates it. True for a pipe bomb, false
    /// for a rocket that has somehow flown for its whole lifetime without
    /// touching anything.
    pub explodes_at_lifetime_end: bool,
    /// How far the explosion reaches, in metres. Both the splash damage and
    /// the shove fall off to nothing here — see
    /// [`crate::common::damage::falloff`].
    pub explosion_radius: f32,
    /// What hitting a body squarely is worth. Not stacked with the splash:
    /// the body that was hit takes this instead, the way it reads to a player.
    pub damage_direct_hit: u32,
    /// What standing at the centre of the explosion is worth. Everyone else
    /// in the radius takes a share of it.
    pub damage_splash: u32,
    /// How hard the explosion throws a corpse at its centre, in metres per
    /// second. A speed and not an impulse — see
    /// [`crate::game::ragdoll::RagdollShove`] for why a corpse is deliberately
    /// not a momentum simulation.
    pub knockback: f32,
    /// How big it is drawn, and how far off a wall it stops, in metres.
    pub radius: f32,
}

impl ProjectileSpec {
    /// A TF2 rocket: flat, fast, and gone the moment it touches anything.
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

    /// The TF2C-style RPG: the same rocket, but it falls.
    ///
    /// Here to prove the point of the spec — an arcing rocket is one number
    /// away from a flat one, and neither is a second projectile type.
    pub const RPG: Self = Self {
        gravity_factor: 1.0,
        initial_speed: 34.0,
        ..Self::ROCKET
    };

    /// A pipe bomb: arcs, tumbles, bounces, and goes off on its fuse.
    pub const PIPE_BOMB: Self = Self {
        initial_speed: 24.0,
        gravity_factor: 1.0,
        air_resistance: 0.1,
        spin_rate: 12.0,
        max_bounces: 4,
        restitution: 0.45,
        max_lifetime: 2.3,
        // A pipe that has run out of bounces has run out of the thing that
        // makes it a pipe; it goes off rather than lying there inert.
        explodes_at_last_bounce: true,
        explodes_at_lifetime_end: true,
        explosion_radius: 3.4,
        damage_direct_hit: 100,
        damage_splash: 60,
        knockback: 10.0,
        radius: 0.14,
    };
}

/// One step of flight, ignoring anything it might run into.
///
/// Gravity first, then drag, then the move — Euler, at the fixed rate the rest
/// of the physics runs at, which is what makes it reproducible rather than
/// accurate. `air_resistance` is applied as a fraction of speed per second and
/// clamped so that a large coefficient stops the projectile rather than
/// reversing it.
///
/// Returns the velocity after the step and the displacement over it. The two
/// are separate because the caller has to *sweep* the displacement against the
/// world before it may be believed.
pub fn fly(velocity: Vec3, spec: &ProjectileSpec, gravity: f32, dt: f32) -> (Vec3, Vec3) {
    let mut moved = velocity + Vec3::Y * (gravity * spec.gravity_factor * dt);
    moved *= (1.0 - spec.air_resistance * dt).clamp(0.0, 1.0);
    (moved, moved * dt)
}

/// What a velocity becomes when it meets a wall whose face is `normal`.
///
/// Mirrored about the face and scaled by `restitution`, which is the whole of
/// the bounce model. No friction along the surface: a pipe that skids has a
/// tangential component this leaves alone, and a real friction term is a
/// number to add here rather than a rewrite.
pub fn bounce(velocity: Vec3, normal: Vec3, restitution: f32) -> Vec3 {
    (velocity - 2.0 * velocity.dot(normal) * normal) * restitution
}

/// Why a projectile's flight ended.
///
/// Carried out of the step so that the decision — detonate, or simply go away
/// — is made from the spec in one place rather than at each of the three
/// points a projectile can die.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// It hit a body. Always a detonation, for every spec: a pipe that landed
    /// squarely on somebody and then dropped to the floor inert would be a
    /// direct hit that did nothing.
    DirectHit,
    /// It hit a wall with no bounces left.
    LastBounce,
    /// The fuse ran out.
    Fuse,
}

impl ProjectileSpec {
    /// Whether an ending is an explosion or just an end.
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

    /// A rocket flies flat: no gravity, no drag, the same speed a second later
    /// as it left with.
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

    /// A pipe bomb does both, which is the whole reason the two share a type.
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

    /// Drag slows a projectile down; it never turns one around, however
    /// absurd the coefficient or however long the step.
    #[test]
    fn drag_cannot_reverse_a_projectile() {
        let spec = ProjectileSpec { air_resistance: 1000.0, ..ProjectileSpec::PIPE_BOMB };
        let (velocity, moved) = fly(Vec3::NEG_Z * 30.0, &spec, 0.0, 1.0);
        assert_eq!(velocity, Vec3::ZERO);
        assert_eq!(moved, Vec3::ZERO);
    }

    /// Straight down onto a floor comes straight back up, at a fraction of
    /// the speed.
    #[test]
    fn a_bounce_mirrors_the_velocity_about_the_face() {
        let bounced = bounce(Vec3::new(0.0, -10.0, 0.0), Vec3::Y, 0.5);
        assert!((bounced - Vec3::new(0.0, 5.0, 0.0)).length() < 1e-5, "{bounced}");
    }

    /// And a glancing one keeps the part of its motion that was along the
    /// wall rather than into it.
    #[test]
    fn a_glancing_bounce_keeps_its_sideways_motion() {
        let bounced = bounce(Vec3::new(6.0, -6.0, 0.0), Vec3::Y, 1.0);
        assert!((bounced - Vec3::new(6.0, 6.0, 0.0)).length() < 1e-5, "{bounced}");
    }

    /// The three endings, per weapon. This table *is* the difference between
    /// a rocket and a pipe bomb, so it is worth stating rather than trusting.
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

    /// The RPG is a rocket that falls and nothing else — the point of having
    /// the spec at all.
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
