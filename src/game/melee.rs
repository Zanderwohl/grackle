//! A melee swing: the nearest body it may hurt within reach of the eye takes the hit on the tick
//! it is fired, and a teammate in the way is swung through. What a swing looks like is the held
//! prop's own `recoil` and `fire` clips, started by [`crate::game::recoil`].

use bevy::prelude::*;

use crate::common::class::Stance;
use crate::common::damage::{Cause, Damage, DamageSource, How, KillShove, PlayerId, CRIT_MULTIPLIER};
use crate::common::hitbox::Hitboxes;
use crate::common::hitscan::{trace, Hit};
use crate::common::team::Allegiances;
use crate::common::weapon::{MeleeSpec, WeaponAction};
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::hitbox::update_hitboxes;
use crate::game::hitscan::eye;
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

/// Across the swing and up and down it, as fractions of [`MeleeSpec::width`].
const SPREAD: [Vec2; 5] = [
    Vec2::ZERO,
    Vec2::new(-0.5, 0.0),
    Vec2::new(0.5, 0.0),
    Vec2::new(0.0, 0.5),
    Vec2::new(0.0, -0.5),
];

/// The nearest hit of any ray in [`SPREAD`], short of any wall, on a body `hurts` allows.
fn reach(
    origin: Vec3,
    looking: Quat,
    spec: &MeleeSpec,
    world: &CollisionWorld,
    targets: &Query<(Entity, &Hitboxes)>,
    hurts: impl Fn(Entity) -> bool,
) -> Option<(Hit<Entity>, Dir3)> {
    let range = spec.range.max(0.0);
    let across = spec.width / range.max(1e-3);
    let hostile: Vec<(Entity, Hitboxes)> =
        targets.iter().filter(|(entity, _)| hurts(*entity)).map(|(entity, boxes)| (entity, *boxes)).collect();
    SPREAD
        .iter()
        .filter_map(|offset| {
            let direction = Dir3::new(looking * Vec3::new(offset.x * across, offset.y * across, -1.0)).ok()?;
            let ray = Ray3d::new(origin, direction);
            let range = world.ray_distance(&ray, range).unwrap_or(range);
            trace(&ray, range, hostile.iter().copied()).map(|hit| (hit, direction))
        })
        .min_by(|(a, _), (b, _)| a.distance.total_cmp(&b.distance))
}

/// One hit at most per [`WeaponActionFired`] carrying a melee action.
pub fn swing(
    mut fired: MessageReader<WeaponActionFired>,
    world: Res<CollisionWorld>,
    shooters: Query<(&PhysicsBody, &Player, &Stance, Option<&PlayerId>)>,
    targets: Query<(Entity, &Hitboxes)>,
    allegiances: Allegiances,
    mut damage: MessageWriter<Damage>,
    mut shoves: MessageWriter<RagdollShove>,
) {
    for shot in fired.read() {
        let WeaponAction::Melee(spec) = shot.action else { continue };
        let Ok((body, player, stance, id)) = shooters.get(shot.shooter) else { continue };
        let origin = eye(body.current, *stance, player.duck, player.eye);
        let looking = Quat::from_rotation_y(player.yaw) * Quat::from_rotation_x(player.pitch);
        let source = id.map_or(DamageSource::World, |id| DamageSource::Player(*id));
        let hurts = |target: Entity| target != shot.shooter && allegiances.may_hurt(source, target);
        let Some((hit, direction)) = reach(origin, looking, &spec, &world, &targets, hurts) else { continue };

        let worth = match shot.critical {
            true => spec.damage as f32 * CRIT_MULTIPLIER,
            false => spec.damage as f32,
        };
        damage.write(Damage {
            target: hit.target,
            source,
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
    use crate::common::team::Team;

    const SIGN: MeleeSpec = MeleeSpec {
        range: 2.0,
        damage: 35,
        width: 0.6,
        shove_per_damage: 0.1,
        kill_shove: Some(crate::common::weapon::KillShoveSpec { speed: 9.0, lift: 3.0, radius_m: 1.2 }),
    };

    /// A shooter at the origin facing -Z, and bodies `away` meters ahead and `aside` across, at
    /// eye height.
    fn swing_among(team: Option<Team>, bodies: &[(f32, f32, Option<Team>)]) -> (Vec<Damage>, Vec<Entity>) {
        let mut world = World::new();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<Messages<WeaponActionFired>>();
        world.init_resource::<Messages<Damage>>();
        world.init_resource::<Messages<RagdollShove>>();
        let center = body_center_from_feet(Vec3::ZERO);
        let mut shooter = world.spawn((
            Player::default(),
            PlayerId(1),
            Stance::Standing,
            PhysicsBody { previous: center, current: center },
            Hitboxes::default(),
        ));
        if let Some(team) = team {
            shooter.insert(team);
        }
        let shooter = shooter.id();
        let targets = bodies
            .iter()
            .map(|&(away, aside, team)| {
                let mut body = world.spawn(Hitboxes {
                    body: Box3 {
                        center: Vec3::new(aside, TALLEST_CLASS_EYE_HEIGHT, -away),
                        half_extents: Vec3::new(0.1, 0.9, 0.1),
                    },
                    head: None,
                });
                if let Some(team) = team {
                    body.insert(team);
                }
                body.id()
            })
            .collect();
        world.write_message(WeaponActionFired {
            shooter,
            button: crate::game::weapon::Button::Primary,
            action: WeaponAction::Melee(SIGN),
            weapon: None,
            critical: false,
        });
        world.run_system_once(swing).unwrap();
        let messages = world.resource::<Messages<Damage>>();
        (messages.get_cursor().read(messages).copied().collect(), targets)
    }

    fn swing_at(away: f32, aside: f32) -> Vec<Damage> {
        swing_among(None, &[(away, aside, None)]).0
    }

    /// Breaks if a teammate on the crosshair stops a swing an enemy beside it is in reach of.
    #[test]
    fn a_swing_passes_through_a_teammate() {
        let (hits, bodies) = swing_among(Some(Team::Red), &[(1.0, 0.0, Some(Team::Red)), (1.8, 0.25, Some(Team::Blue))]);
        assert_eq!(hits.iter().map(|hit| hit.target).collect::<Vec<_>>(), vec![bodies[1]]);
    }

    /// Breaks if a far body on the crosshair is hit before a nearer one off to the side.
    #[test]
    fn a_swing_hits_the_nearest_body() {
        let (hits, bodies) = swing_among(None, &[(1.9, 0.0, None), (0.8, -0.12, None)]);
        assert_eq!(hits.iter().map(|hit| hit.target).collect::<Vec<_>>(), vec![bodies[1]]);
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
