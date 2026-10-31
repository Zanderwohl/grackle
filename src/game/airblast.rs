//! A gust from the muzzle ([`AirblastSpec`]): it shoves bodies it may hurt along the aim, turns
//! hostile projectiles back as the airblaster's, and puts out burning teammates — whichever its
//! spec asks for. Nothing is hurt, so `apply_damage` is never asked and each part checks
//! [`Allegiances`] itself. A body or projectile is caught when it is ahead of the eye, within the
//! range, inside the cone widened by its own size, and in sight. Its cone is drawn with the
//! blasts' marks, in [`crate::game::explosion`].

use bevy::prelude::*;

use crate::common::class::Stance;
use crate::common::damage::{DamageSource, PlayerId};
use crate::common::effects::Effect;
use crate::common::flame::Burning;
use crate::common::hitbox::Hitboxes;
use crate::common::team::Allegiances;
use crate::common::weapon::{AirblastSpec, WeaponAction};
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::hitscan::{aim, eye};
use crate::game::player::{Impulse, PhysicsBody, Player};
use crate::game::projectile::Projectile;
use crate::game::ragdoll::{Push, RagdollShove};
use crate::game::weapon::WeaponActionFired;

/// Meters a body's middle may be outside the cone and still be caught: about half a body.
const BODY_ALLOWANCE: f32 = 0.4;
/// Meters on top of a projectile's own radius.
const PROJECTILE_ALLOWANCE: f32 = 0.3;

pub struct AirblastPlugin;

impl Plugin for AirblastPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            airblast.in_set(DamageSystems::Deal).before(crate::game::projectile::step_projectiles).after(crate::game::hitbox::update_hitboxes),
        );
    }
}

/// Whether `point` is in a cone from `from` along `toward`, `allowance` meters wider than
/// `spread`, out to `range`.
pub fn in_cone(from: Vec3, toward: Dir3, range: f32, spread: f32, point: Vec3, allowance: f32) -> bool {
    let offset = point - from;
    let along = offset.dot(*toward);
    if along <= 0.0 || along > range + allowance {
        return false;
    }
    let across = (offset - *toward * along).length();
    across <= along * spread.tan() + allowance
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn airblast(
    mut commands: Commands,
    world: Res<CollisionWorld>,
    mut fired: MessageReader<WeaponActionFired>,
    shooters: Query<(&PhysicsBody, &Player, &Stance, Option<&PlayerId>)>,
    mut bodies: Query<(Entity, &Hitboxes, Option<&mut Impulse>, Has<Burning>)>,
    mut projectiles: Query<(&mut Projectile, &PhysicsBody)>,
    allegiances: Allegiances,
    mut shoves: MessageWriter<RagdollShove>,
    mut effects: MessageWriter<Effect>,
) {
    for shot in fired.read() {
        let WeaponAction::Airblast(spec) = shot.action else { continue };
        let Ok((body, player, stance, id)) = shooters.get(shot.shooter) else { continue };
        let source = id.map_or(DamageSource::World, |id| DamageSource::Player(*id));
        let from = eye(body.current, *stance, player.duck);
        let toward = aim(player);
        effects.write(Effect::Gust { from, toward: *toward, range: spec.range, spread: spec.spread, by: id.copied() });

        let caught = |point: Vec3, allowance: f32| in_cone(from, toward, spec.range, spec.spread, point, allowance) && world.segment_clear(from, point);

        for (entity, boxes, impulse, burning) in &mut bodies {
            if entity == shot.shooter || !caught(boxes.body.center, BODY_ALLOWANCE) {
                continue;
            }
            if allegiances.may_hurt(source, entity) {
                if let Some(mut impulse) = impulse {
                    impulse.0 += push(&spec, toward);
                }
            } else if spec.extinguish && burning {
                commands.entity(entity).remove::<Burning>();
            }
        }

        if spec.reflect {
            for (mut projectile, at) in &mut projectiles {
                let theirs = projectile.shooter != Some(shot.shooter) && allegiances.may_hurt(projectile.source(), shot.shooter);
                if theirs && caught(at.current, projectile.spec.radius + PROJECTILE_ALLOWANCE) {
                    reflect(&mut projectile, at.current, toward, spec.reflect_speed, id.copied(), shot.shooter);
                }
            }
        }

        // Corpses have no hitboxes; the middle of the cone is shoved as one.
        if spec.push > 0.0 {
            shoves.write(RagdollShove {
                at: from + *toward * spec.range * 0.5,
                push: Push::Along(push(&spec, toward)),
                radius: spec.range * 0.5,
            });
        }
    }
}

/// The push a gust gives a body, in meters per second.
pub fn push(spec: &AirblastSpec, toward: Dir3) -> Vec3 {
    *toward * spec.push + Vec3::Y * spec.lift
}

/// Turns `projectile`, now `at`, along `toward` at `speed` times its speed, as `owner`'s from
/// `shooter`. Its fuse and bounces start again, or a grenade turned late goes off on the
/// airblaster.
pub fn reflect(projectile: &mut Projectile, at: Vec3, toward: Dir3, speed: f32, owner: Option<PlayerId>, shooter: Entity) {
    projectile.velocity = *toward * projectile.velocity.length() * speed;
    projectile.owner = owner;
    projectile.shooter = Some(shooter);
    projectile.launched = at;
    projectile.age = 0.0;
    projectile.bounces = 0;
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::class::body_center_from_feet;
    use crate::common::flame::FlameSpec;
    use crate::common::hitbox::Box3;
    use crate::common::projectile::ProjectileSpec;
    use crate::common::team::Team;
    use crate::game::projectile::fired;

    const SPEC: AirblastSpec = AirblastSpec { range: 5.0, spread: 0.5, push: 9.0, lift: 4.0, reflect: true, reflect_speed: 1.0, extinguish: true };

    fn a_world() -> World {
        let mut world = World::new();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<Messages<WeaponActionFired>>();
        world.init_resource::<Messages<RagdollShove>>();
        world.init_resource::<Messages<Effect>>();
        world
    }

    /// At the origin, looking down -Z, on Red.
    fn a_shooter(world: &mut World) -> Entity {
        let center = body_center_from_feet(Vec3::ZERO);
        world.spawn((Player::default(), PlayerId(1), Team::Red, Stance::Standing, PhysicsBody { previous: center, current: center })).id()
    }

    /// `away` meters down -Z and `across` to the side, at eye height.
    fn a_body(world: &mut World, away: f32, across: f32, team: Team) -> Entity {
        let center = Vec3::new(across, crate::common::class::TALLEST_CLASS_EYE_HEIGHT, -away);
        let boxes = Hitboxes { body: Box3 { center, half_extents: Vec3::new(0.4, 0.9, 0.4) }, head: None };
        world.spawn((boxes, Impulse::default(), PlayerId(2 + away as u64), team)).id()
    }

    fn gust(world: &mut World, shooter: Entity) {
        world.write_message(WeaponActionFired { shooter, button: crate::game::weapon::Button::Secondary, action: WeaponAction::Airblast(SPEC) });
        world.run_system_once(airblast).unwrap();
        world.flush();
    }

    /// Breaks if the cone stops widening with distance, reaches behind the eye, or past its range.
    #[test]
    fn the_cone_is_ahead_and_widens() {
        let (from, toward) = (Vec3::ZERO, Dir3::NEG_Z);
        assert!(in_cone(from, toward, 5.0, 0.5, Vec3::new(0.0, 0.0, -3.0), 0.0));
        assert!(in_cone(from, toward, 5.0, 0.5, Vec3::new(1.5, 0.0, -3.0), 0.0), "inside tan(0.5) * 3 = 1.64");
        assert!(!in_cone(from, toward, 5.0, 0.5, Vec3::new(1.5, 0.0, -1.0), 0.0), "too wide that close");
        assert!(!in_cone(from, toward, 5.0, 0.5, Vec3::new(0.0, 0.0, 1.0), 0.0), "behind");
        assert!(!in_cone(from, toward, 5.0, 0.5, Vec3::new(0.0, 0.0, -5.5), 0.0), "past the range");
    }

    /// Breaks if a gust stops pushing an enemy along the aim, or starts pushing a teammate or
    /// somebody outside it.
    #[test]
    fn a_gust_pushes_enemies_in_the_cone_and_nobody_else() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        let enemy = a_body(&mut world, 3.0, 0.0, Team::Blue);
        let ally = a_body(&mut world, 2.0, 0.0, Team::Red);
        let aside = a_body(&mut world, 4.0, 4.0, Team::Blue);
        gust(&mut world, shooter);
        let pushed = |entity| world.get::<Impulse>(entity).unwrap().0;
        assert!((pushed(enemy) - Vec3::new(0.0, 4.0, -9.0)).length() < 1e-4, "{}", pushed(enemy));
        assert_eq!(pushed(ally), Vec3::ZERO);
        assert_eq!(pushed(aside), Vec3::ZERO);
    }

    /// Breaks if a teammate stays alight in the cone, or an enemy is put out.
    #[test]
    fn a_gust_puts_out_teammates_only() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        let lit = Burning::lit(&FlameSpec::FLAMETHROWER, DamageSource::World);
        let ally = a_body(&mut world, 2.0, 0.0, Team::Red);
        let enemy = a_body(&mut world, 3.0, 0.0, Team::Blue);
        world.entity_mut(ally).insert(lit);
        world.entity_mut(enemy).insert(lit);
        gust(&mut world, shooter);
        assert!(world.get::<Burning>(ally).is_none(), "the teammate still burns");
        assert!(world.get::<Burning>(enemy).is_some(), "put the enemy out");
    }

    /// Breaks if an enemy's rocket in the cone is not turned back as ours, or a teammate's is.
    #[test]
    fn a_gust_turns_an_enemy_rocket_back_as_ours() {
        let mut world = a_world();
        let shooter = a_shooter(&mut world);
        let enemy = a_body(&mut world, 30.0, 0.0, Team::Blue);
        let ally = a_body(&mut world, 31.0, 0.0, Team::Red);
        let rocket = |world: &mut World, by: Entity, at: Vec3| {
            let id = *world.get::<PlayerId>(by).unwrap();
            world.spawn((fired(ProjectileSpec::ROCKET, at, Dir3::Z, Some(id), Some(by)), PhysicsBody { previous: at, current: at })).id()
        };
        let theirs = rocket(&mut world, enemy, Vec3::new(0.0, 1.6, -3.0));
        let friendly = rocket(&mut world, ally, Vec3::new(0.2, 1.6, -3.5));
        gust(&mut world, shooter);

        let theirs = world.get::<Projectile>(theirs).unwrap();
        assert_eq!(theirs.owner, Some(PlayerId(1)));
        assert_eq!(theirs.shooter, Some(shooter));
        assert_eq!((theirs.age, theirs.launched), (0.0, Vec3::new(0.0, 1.6, -3.0)), "the fuse runs on from the enemy's shot");
        assert!((theirs.velocity - Vec3::NEG_Z * ProjectileSpec::ROCKET.initial_speed).length() < 1e-3, "{}", theirs.velocity);
        assert_eq!(world.get::<Projectile>(friendly).unwrap().shooter, Some(ally), "reflected a teammate's rocket");
    }
}
