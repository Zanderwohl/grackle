//! What an [`Explosion`] does to the things around it.
//!
//! One system, so that every explosive in the game agrees about splash. A
//! rocket, a pipe bomb, a barrel and whatever a mapper eventually plants
//! between rounds all write the same message and none of them contains a copy
//! of this loop — which matters less because the loop is hard than because
//! there are four decisions in it (what counts as a body, what a wall does,
//! whether a direct hit also takes splash, what gets thrown) and four copies
//! would answer them four ways.
//!
//! Three rules, and each is a thing a player will notice immediately if it is
//! wrong:
//!
//! - **A wall stops it.** A rocket on the other side of the floor you are
//!   standing on does nothing to you. The check is one ray to each candidate,
//!   the same [`CollisionWorld::ray_distance`] a bullet is shortened by, so
//!   "can it see you" means the same thing for both.
//! - **A direct hit takes the direct number, not that plus splash.** One hit
//!   is one number on the screen.
//! - **The corpses are thrown outwards, not along.** [`Push::Outward`] exists
//!   for this: an explosion at a body's feet has to lift it, and a directed
//!   shove with a wide radius slides the whole room the same way instead.
//!
//! What is deliberately missing is knockback on a body that is still alive —
//! rocket jumping. Not an oversight: [`step_player`](crate::game::player::step_player)
//! writes the horizontal velocity outright from the movement input every step,
//! so an impulse given to a standing player would be gone by the next tick.
//! That is a movement model with momentum, which is its own piece of work; the
//! explosion already carries the number it would need.
//!
//! A splash candidate is anything with [`Hitboxes`] — the same set a bullet
//! can land on. Being shootable and being catchable in a blast are the same
//! property, and a second notion of "is a body" would drift from the first.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::{falloff, Damage, Explosion};
use crate::common::hitbox::Hitboxes;
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::ragdoll::{Push, RagdollShove};

/// How long the debug sphere of a blast is drawn for, in seconds.
///
/// Long enough to see where a rocket went off after it has gone, short enough
/// that a burst of them does not leave the room full of circles.
pub const BLAST_MARK_LIFETIME: f32 = 0.35;

/// A drawing of an explosion that has already happened.
///
/// The explosion itself is instantaneous — it is a message, resolved in the
/// tick it was written — so there would otherwise be nothing to look at. Like
/// [`crate::game::damage::DamageNumber`], it is a world-anchored piece of
/// feedback and not part of the simulation.
#[derive(Component, Debug)]
pub struct BlastMark {
    pub at: Vec3,
    pub radius: f32,
    /// Seconds of life left.
    pub remaining: f32,
}

pub struct ExplosionPlugin;

impl Plugin for ExplosionPlugin {
    fn build(&self, app: &mut App) {
        app
            .add_message::<Explosion>()
            // In `Deal`, because that is exactly what it is: a writer of
            // `Damage`, ordered ahead of the health pool like every other
            // source. It reads `Explosion` in the same tick it was written
            // because the writers are in `Deal` too and messages are readable
            // as soon as they are sent.
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

/// Hurt everything the blast reaches, and throw every corpse it reaches.
pub fn explode(
    world: Res<CollisionWorld>,
    mut blasts: MessageReader<Explosion>,
    bodies: Query<(Entity, &Hitboxes)>,
    mut damage: MessageWriter<Damage>,
    mut shoves: MessageWriter<RagdollShove>,
) {
    for blast in blasts.read() {
        if let Some((target, worth)) = blast.direct {
            damage.write(Damage {
                target,
                source: blast.source,
                amount: worth,
                point: blast.at,
            });
        }

        for (entity, boxes) in &bodies {
            // The one that took it squarely already has its number.
            if blast.direct.is_some_and(|(direct, _)| direct == entity) {
                continue;
            }

            // The chest, not the feet and not the origin: a blast at head
            // height over a body is as close to it as one at its feet.
            let centre = boxes.body.centre;
            let distance = centre.distance(blast.at);
            let reach = falloff(distance, blast.radius);
            if reach <= 0.0 {
                continue;
            }
            if !in_sight(&world, blast.at, centre, distance) {
                continue;
            }

            // Rounded rather than truncated, so the body at the very edge of
            // a blast takes nothing rather than the one just inside it taking
            // nothing too.
            let amount = (blast.damage as f32 * reach).round() as u32;
            if amount == 0 {
                continue;
            }
            damage.write(Damage {
                target: entity,
                source: blast.source,
                amount,
                // Where the blast was, not where the body is: the number
                // belongs to the explosion, and half a dozen of them stacked
                // on one point is what an explosion looks like.
                point: blast.at,
            });
        }

        // One message for the whole blast, whatever it happened to hit. A
        // corpse is not in `bodies` — it has no hitboxes — and this is how it
        // gets thrown all the same: the shove is a fact about the world, and
        // whatever is lying in it answers for itself.
        shoves.write(RagdollShove {
            at: blast.at,
            push: Push::Outward(blast.knockback),
            radius: blast.radius,
        });
    }
}

/// Whether there is a clear line from the blast to a point `distance` away.
///
/// A hair short of the target, because a body standing against a wall has that
/// wall's surface a few centimetres behind its chest and a ray run to the full
/// distance can find it.
fn in_sight(world: &CollisionWorld, from: Vec3, to: Vec3, distance: f32) -> bool {
    const CLEARANCE: f32 = 0.05;

    let Ok(direction) = Dir3::new(to - from) else {
        // Standing exactly on the explosion. Nothing can be between them.
        return true;
    };
    let reach = (distance - CLEARANCE).max(0.0);
    world.ray_distance(&Ray3d::new(from, direction), reach).is_none()
}

fn mark_blasts(mut commands: Commands, mut blasts: MessageReader<Explosion>) {
    for blast in blasts.read() {
        commands.spawn((
            BlastMark {
                at: blast.at,
                radius: blast.radius,
                remaining: BLAST_MARK_LIFETIME,
            },
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

/// The blast as it fades: a sphere the size of the radius that actually did
/// the damage, so a rocket that felt like it should have hurt somebody can be
/// checked rather than argued about.
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
        world
    }

    /// A body standing with its chest at `centre`.
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

    /// The falloff, end to end: nearer is worse, and past the edge is nothing.
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
        let worth = |target| hits.iter().find(|h| h.target == target).unwrap().amount;
        assert_eq!(worth(near), 45, "60 at three quarters reach");
        assert_eq!(worth(far), 15, "60 at a quarter reach");
    }

    /// A direct hit takes the direct number and no splash on top of it — one
    /// hit is one number.
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

    /// A wall between the two is a wall: the thing that stops a bullet stops
    /// a blast.
    #[test]
    fn a_wall_between_them_takes_the_blast() {
        let mut world = a_world();
        // Two rooms side by side, so the wall between them is solid.
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

    /// And one shove for the whole blast, outwards — the corpses it reaches
    /// are not in the body query at all, so this is the only thing that moves
    /// them.
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
