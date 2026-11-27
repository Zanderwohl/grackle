//! A melee swing: the nearest body within reach of the eye takes the hit on the
//! tick it is fired. What a swing looks like is the held prop's own `recoil`
//! and `fire` clips, started by [`crate::game::recoil`] like any shot's kick.

use bevy::prelude::*;

use crate::common::class::Stance;
use crate::common::damage::{Cause, Damage, DamageSource, How, KillShove, PlayerId, CRIT_MULTIPLIER};
use crate::common::hitbox::Hitboxes;
use crate::common::hitscan::Hit;
use crate::common::weapon::{MeleeSpec, WeaponAction};
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::hitbox::update_hitboxes;
use crate::game::hitscan::{eye, trace_past};
use crate::game::player::{step_player, PhysicsBody, Player};
use crate::game::ragdoll::{Push, RagdollShove};
use crate::game::weapon::WeaponActionFired;

pub struct MeleePlugin;

impl Plugin for MeleePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            swing.after(update_hitboxes).after(step_player).in_set(DamageSystems::Deal),
        );
    }
}

/// Across the swing and up and down it, as fractions of [`MeleeSpec::width`]:
/// the middle first, so a body on the crosshair is the one hit.
const SPREAD: [Vec2; 5] = [
    Vec2::ZERO,
    Vec2::new(-0.5, 0.0),
    Vec2::new(0.5, 0.0),
    Vec2::new(0.0, 0.5),
    Vec2::new(0.0, -0.5),
];

/// The first ray in [`SPREAD`] that reaches a body, short of any wall.
fn reach(
    origin: Vec3,
    looking: Quat,
    spec: &MeleeSpec,
    shooter: Entity,
    world: &CollisionWorld,
    targets: &Query<(Entity, &Hitboxes)>,
) -> Option<(Hit<Entity>, Dir3)> {
    let range = spec.range.max(0.0);
    let across = spec.width / range.max(1e-3);
    SPREAD.iter().find_map(|offset| {
        let direction = Dir3::new(looking * Vec3::new(offset.x * across, offset.y * across, -1.0)).ok()?;
        let ray = Ray3d::new(origin, direction);
        let range = world.ray_distance(&ray, range).unwrap_or(range);
        trace_past(&ray, range, shooter, targets).map(|hit| (hit, direction))
    })
}

/// One hit at most per [`WeaponActionFired`] carrying a melee action.
pub fn swing(
    mut fired: MessageReader<WeaponActionFired>,
    world: Res<CollisionWorld>,
    shooters: Query<(&PhysicsBody, &Player, &Stance, Option<&PlayerId>)>,
    targets: Query<(Entity, &Hitboxes)>,
    mut damage: MessageWriter<Damage>,
    mut shoves: MessageWriter<RagdollShove>,
) {
    for shot in fired.read() {
        let WeaponAction::Melee(spec) = shot.action else { continue };
        let Ok((body, player, stance, id)) = shooters.get(shot.shooter) else { continue };
        let origin = eye(body.current, *stance, player.duck, player.eye);
        let looking = Quat::from_rotation_y(player.yaw) * Quat::from_rotation_x(player.pitch);
        let Some((hit, direction)) = reach(origin, looking, &spec, shot.shooter, &world, &targets) else { continue };

        let worth = match shot.critical {
            true => spec.damage as f32 * CRIT_MULTIPLIER,
            false => spec.damage as f32,
        };
        damage.write(Damage {
            target: hit.target,
            source: id.map_or(DamageSource::World, |id| DamageSource::Player(*id)),
            amount: worth.round() as u32,
            point: hit.point,
            from: Some(origin),
            // Level, away from the swinger, so a body killed looking down is not driven into the floor.
            kill_shove: spec.kill_shove.map(|shove| KillShove {
                velocity: Quat::from_rotation_y(player.yaw) * Vec3::NEG_Z * shove.speed + Vec3::Y * shove.lift,
                radius: shove.radius_m,
            }),
            cause: Cause { critical: shot.critical, ..Cause::of(shot.weapon, How::Melee) },
        });
        shoves.write(RagdollShove {
            at: hit.point,
            push: Push::Along(*direction * (worth * spec.shove_per_damage)),
            radius: 0.4,
        });
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::class::{body_center_from_feet, TALLEST_CLASS_EYE_HEIGHT};
    use crate::common::hitbox::Box3;

    const SIGN: MeleeSpec = MeleeSpec {
        range: 2.0,
        damage: 35,
        width: 0.6,
        shove_per_damage: 0.1,
        kill_shove: Some(crate::common::weapon::KillShoveSpec { speed: 9.0, lift: 3.0, radius_m: 1.2 }),
    };

    /// A shooter at the origin facing -Z, and a body `away` meters ahead and
    /// `aside` meters to its right, at eye height.
    fn swing_at(away: f32, aside: f32) -> Vec<Damage> {
        let mut world = World::new();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<Messages<WeaponActionFired>>();
        world.init_resource::<Messages<Damage>>();
        world.init_resource::<Messages<RagdollShove>>();
        let center = body_center_from_feet(Vec3::ZERO);
        let shooter = world
            .spawn((
                Player::default(),
                PlayerId(1),
                Stance::Standing,
                PhysicsBody { previous: center, current: center },
                Hitboxes::default(),
            ))
            .id();
        world.spawn(Hitboxes {
            body: Box3 {
                center: Vec3::new(aside, TALLEST_CLASS_EYE_HEIGHT, -away),
                half_extents: Vec3::new(0.1, 0.9, 0.1),
            },
            head: None,
        });
        world.write_message(WeaponActionFired {
            shooter,
            button: crate::game::weapon::Button::Primary,
            action: WeaponAction::Melee(SIGN),
            weapon: None,
            critical: false,
        });
        world.run_system_once(swing).unwrap();
        let messages = world.resource::<Messages<Damage>>();
        messages.get_cursor().read(messages).copied().collect()
    }

    /// Breaks if a swing reaches past its range, or misses a body in front of it.
    #[test]
    fn a_swing_hits_within_reach_and_not_beyond() {
        let hits = swing_at(1.5, 0.0);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].amount, 35);
        assert_eq!(hits[0].cause.how, How::Melee);
        let shove = hits[0].kill_shove.expect("no kill shove");
        assert!(shove.velocity.z < -8.0 && shove.velocity.y > 0.0, "not thrown back from the swing: {shove:?}");
        assert!(swing_at(2.5, 0.0).is_empty(), "hit past its range");
    }

    /// Breaks if the swing is one ray down the crosshair, so a body a little
    /// to one side is missed.
    #[test]
    fn a_swing_is_wider_than_the_crosshair() {
        assert_eq!(swing_at(1.8, 0.25).len(), 1);
    }
}
