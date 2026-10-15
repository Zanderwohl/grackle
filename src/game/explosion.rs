//! What an [`Explosion`] does to the things around it. Every explosive writes
//! the same message so they all agree about splash.
//!
//! A wall stops a blast (the same [`CollisionWorld::ray_distance`] a bullet
//! uses). A direct hit takes the direct number instead of splash. Corpses are
//! thrown [`Push::Outward`]. A splash candidate is anything with [`Hitboxes`].
//!
//! No knockback on living bodies: `step_player` overwrites horizontal
//! velocity every step, so an impulse would be gone next tick.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::effects::Effect;
use crate::common::damage::{falloff, Damage, Explosion};
use crate::common::hitbox::Hitboxes;
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::ragdoll::{Push, RagdollShove};

/// How long the debug sphere of a blast is drawn for, in seconds.
pub const BLAST_MARK_LIFETIME: f32 = 0.35;

/// A drawing of an explosion that has already happened. Feedback only.
#[derive(Component, Debug)]
pub struct BlastMark {
    pub at: Vec3,
    pub radius: f32,
    /// Seconds.
    pub remaining: f32,
}

pub struct ExplosionPlugin;

impl Plugin for ExplosionPlugin {
    fn build(&self, app: &mut App) {
        app
            .add_message::<Explosion>()
            .add_systems(FixedUpdate, explode.in_set(DamageSystems::Deal))
            .add_systems(Update, (mark_blasts, fade_blast_marks))
            .add_systems(
                PostUpdate,
                draw_blast_marks.run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), clear_blast_marks)
        ;
    }
}

pub fn explode(
    world: Res<CollisionWorld>,
    mut blasts: MessageReader<Explosion>,
    bodies: Query<(Entity, &Hitboxes)>,
    mut damage: MessageWriter<Damage>,
    mut shoves: MessageWriter<RagdollShove>,
    mut effects: MessageWriter<Effect>,
) {
    for blast in blasts.read() {
        effects.write(Effect::Blast { at: blast.at, radius: blast.radius });

        if let Some((target, worth)) = blast.direct {
            damage.write(Damage {
                target,
                source: blast.source,
                amount: worth,
                point: blast.at,
            });
        }

        for (entity, boxes) in &bodies {
            if blast.direct.is_some_and(|(direct, _)| direct == entity) {
                continue;
            }

            let centre = boxes.body.centre;
            let distance = centre.distance(blast.at);
            let reach = falloff(distance, blast.radius);
            if reach <= 0.0 {
                continue;
            }
            if !in_sight(&world, blast.at, centre, distance) {
                continue;
            }

            // Rounded, not truncated, so only the very edge rounds to zero.
            let amount = (blast.damage as f32 * reach).round() as u32;
            if amount == 0 {
                continue;
            }
            damage.write(Damage {
                target: entity,
                source: blast.source,
                amount,
                // Over the body: at the blast point every victim's number
                // would be drawn on top of the others.
                point: centre,
            });
        }

        // Corpses have no hitboxes, so this is the only thing that moves them.
        shoves.write(RagdollShove {
            at: blast.at,
            push: Push::Outward(blast.knockback),
            radius: blast.radius,
        });
    }
}

/// Metres short of its target a line of sight stops, because a body against a
/// wall has the wall a few centimetres behind its chest.
const SIGHT_CLEARANCE: f32 = 0.05;

/// Whether there is a clear line from the blast to a point `distance` away.
fn in_sight(world: &CollisionWorld, from: Vec3, to: Vec3, distance: f32) -> bool {
    let Ok(direction) = Dir3::new(to - from) else {
        return true;
    };
    let reach = (distance - SIGHT_CLEARANCE).max(0.0);
    world.ray_distance(&Ray3d::new(from, direction), reach).is_none()
}

/// Reads [`Effect`] rather than `Explosion`: `Explosion` exists only on the
/// authority, `Effect` is relayed to clients.
fn mark_blasts(mut commands: Commands, mut effects: MessageReader<Effect>) {
    for effect in effects.read() {
        let Effect::Blast { at, radius } = *effect else { continue };
        commands.spawn((
            BlastMark { at, radius, remaining: BLAST_MARK_LIFETIME },
            Name::new("Blast mark"),
        ));
    }
}

fn fade_blast_marks(
    mut commands: Commands,
    time: Res<Time>,
    mut marks: Query<(Entity, &mut BlastMark)>,
) {
    for (entity, mut mark) in &mut marks {
        mark.remaining -= time.delta_secs();
        if mark.remaining <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// A sphere the size of the damage radius.
fn draw_blast_marks(mut gizmos: Gizmos, marks: Query<&BlastMark>) {
    for mark in &marks {
        let alpha = (mark.remaining / BLAST_MARK_LIFETIME).clamp(0.0, 1.0);
        gizmos.sphere(
            Isometry3d::from_translation(mark.at),
            mark.radius,
            Color::srgba(1.0, 0.55, 0.1, alpha),
        );
    }
}

fn clear_blast_marks(mut commands: Commands, marks: Query<Entity, With<BlastMark>>) {
    for mark in &marks {
        commands.entity(mark).despawn();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::damage::{DamageSource, PlayerId};
    use crate::common::hitbox::Box3;
    use crate::tool::room::Room;

    const SHOOTER: DamageSource = DamageSource::Player(PlayerId(3));

    fn a_world() -> World {
        let mut world = World::new();
        world.init_resource::<CollisionWorld>();
        world.init_resource::<Messages<Explosion>>();
        world.init_resource::<Messages<Damage>>();
        world.init_resource::<Messages<RagdollShove>>();
        world.init_resource::<Messages<Effect>>();
        world
    }

    fn body(world: &mut World, centre: Vec3) -> Entity {
        world
            .spawn(Hitboxes {
                body: Box3 { centre, half_extents: Vec3::new(0.4, 0.9, 0.4) },
                head: Box3 { centre: centre + Vec3::Y * 0.8, half_extents: Vec3::splat(0.15) },
            })
            .id()
    }

    fn blast(at: Vec3, direct: Option<(Entity, u32)>) -> Explosion {
        Explosion { at, radius: 4.0, damage: 60, knockback: 10.0, source: SHOOTER, direct }
    }

    fn damage_written(world: &mut World) -> Vec<Damage> {
        let messages = world.resource::<Messages<Damage>>();
        let mut cursor = messages.get_cursor();
        cursor.read(messages).copied().collect()
    }

    #[test]
    fn splash_falls_off_with_distance_and_stops_at_the_radius() {
        let mut world = a_world();
        let near = body(&mut world, Vec3::new(1.0, 0.0, 0.0));
        let far = body(&mut world, Vec3::new(3.0, 0.0, 0.0));
        let _outside = body(&mut world, Vec3::new(9.0, 0.0, 0.0));
        world.write_message(blast(Vec3::ZERO, None));

        world.run_system_once(explode).unwrap();

        let hits = damage_written(&mut world);
        assert_eq!(hits.len(), 2, "the body outside the radius was caught: {hits:?}");
        let hit_on = |target| *hits.iter().find(|h| h.target == target).unwrap();
        assert_eq!(hit_on(near).amount, 45, "60 at three quarters reach");
        assert_eq!(hit_on(far).amount, 15, "60 at a quarter reach");
    }

    /// Breaks if splash numbers go at the blast point, where they overlap.
    #[test]
    fn a_splash_number_is_anchored_over_the_body_it_hurt() {
        let mut world = a_world();
        let left = body(&mut world, Vec3::new(-1.0, 0.0, 0.0));
        let right = body(&mut world, Vec3::new(1.0, 0.0, 0.0));
        world.write_message(blast(Vec3::ZERO, None));

        world.run_system_once(explode).unwrap();

        let hits = damage_written(&mut world);
        let point_on = |target| hits.iter().find(|h| h.target == target).unwrap().point;
        assert_eq!(point_on(left), Vec3::new(-1.0, 0.0, 0.0));
        assert_eq!(point_on(right), Vec3::new(1.0, 0.0, 0.0));
        assert_ne!(point_on(left), point_on(right), "two hits landed on one point");
    }

    /// A direct hit's number goes where the projectile struck, not the chest.
    #[test]
    fn a_direct_hit_is_anchored_where_it_struck() {
        let mut world = a_world();
        let target = body(&mut world, Vec3::new(0.0, 1.0, 0.0));
        let struck = Vec3::new(0.1, 1.6, 0.3);
        world.write_message(blast(struck, Some((target, 90))));

        world.run_system_once(explode).unwrap();

        let hits = damage_written(&mut world);
        assert_eq!(hits.iter().find(|h| h.target == target).unwrap().point, struck);
    }

    /// Breaks if a direct hit also takes splash.
    #[test]
    fn a_direct_hit_takes_its_own_number_instead_of_the_splash() {
        let mut world = a_world();
        let hit = body(&mut world, Vec3::new(0.2, 0.0, 0.0));
        let bystander = body(&mut world, Vec3::new(2.0, 0.0, 0.0));
        world.write_message(blast(Vec3::ZERO, Some((hit, 90))));

        world.run_system_once(explode).unwrap();

        let hits = damage_written(&mut world);
        assert_eq!(hits.iter().filter(|h| h.target == hit).count(), 1, "{hits:?}");
        assert_eq!(hits.iter().find(|h| h.target == hit).unwrap().amount, 90);
        assert!(hits.iter().any(|h| h.target == bystander), "the bystander was missed");
    }

    #[test]
    fn a_wall_between_them_takes_the_blast() {
        let mut world = a_world();
        // Two rooms side by side, with a solid wall between them.
        world.resource_mut::<CollisionWorld>().rebuild(&[
            Room::new(Vec3::new(-4.0, 0.0, -4.0), Vec3::new(-0.5, 4.0, 4.0)),
            Room::new(Vec3::new(0.5, 0.0, -4.0), Vec3::new(4.0, 4.0, 4.0)),
        ]);
        let sheltered = body(&mut world, Vec3::new(1.5, 2.0, 0.0));
        world.write_message(blast(Vec3::new(-1.5, 2.0, 0.0), None));

        world.run_system_once(explode).unwrap();

        let hits = damage_written(&mut world);
        assert!(
            !hits.iter().any(|h| h.target == sheltered),
            "the blast went through the wall: {hits:?}"
        );
    }

    /// Breaks if corpses stop being thrown: they are not in the body query.
    #[test]
    fn a_blast_shoves_outwards_once() {
        let mut world = a_world();
        world.write_message(blast(Vec3::new(1.0, 2.0, 3.0), None));

        world.run_system_once(explode).unwrap();

        let messages = world.resource::<Messages<RagdollShove>>();
        let mut cursor = messages.get_cursor();
        let shoves: Vec<RagdollShove> = cursor.read(messages).copied().collect();
        assert_eq!(shoves.len(), 1);
        assert_eq!(shoves[0].at, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(shoves[0].push, Push::Outward(10.0));
        assert_eq!(shoves[0].radius, 4.0);
    }
}
