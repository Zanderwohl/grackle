//! What a bot notices. A bot never reads where an enemy is: the think and fight layers read
//! [`BotSenses`], and this is the only thing that writes it. See "Senses" in
//! documentation/bot-brain.md.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::damage::{DamageDealt, DamageSource, PlayerId};
use crate::common::effects::Effect;
use crate::common::hitbox::Hitboxes;
use crate::common::net::has_authority;
use crate::common::team::{hostile, Team};
use crate::game::bot::BotOf;
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::death::Dying;
use crate::game::hitbox::update_hitboxes;
use crate::game::nav::NavGraph;
use crate::game::player::{PhysicsBody, Player};
use crate::tool::nav_bake::NavBake;

/// Radians either side of where a bot looks that it sees across: a 90° view, so what is behind
/// it, it notices only by hearing or being hit.
pub const HALF_VIEW: f32 = std::f32::consts::FRAC_PI_4;
/// Radians above and below its look: wider, since a ledge overhead is somewhere a person glances.
pub const HALF_VIEW_VERTICAL: f32 = std::f32::consts::FRAC_PI_3;
/// Meters.
pub const SIGHT_RANGE: f32 = 60.0;
/// Meters within which a shot and a blast are heard.
pub const HEARS_SHOTS: f32 = 30.0;
pub const HEARS_BLASTS: f32 = 40.0;
/// Seconds an enemy is remembered unseen, and a noise at all.
pub const REMEMBERS: f32 = 4.0;
pub const REMEMBERS_NOISES: f32 = 3.0;
/// Seconds between noticing something and acting on it, the range a temperament rolls in.
const REACTION: std::ops::Range<f32> = 0.2..0.4;

/// A bot's temperament for the brain, rolled per seat and kept across lives, beside its
/// [`RouteCosts`](crate::game::bot::RouteCosts).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct BotMind {
    /// Seconds between noticing an enemy and acting on it. A bot that fires the frame somebody
    /// rounds a corner reads as a cheat whatever its aim.
    pub reaction: f32,
}

impl Default for BotMind {
    fn default() -> Self {
        BotMind { reaction: 0.3 }
    }
}

impl BotMind {
    pub fn temperament() -> Self {
        BotMind { reaction: rand::random_range(REACTION) }
    }
}

/// How an enemy was last noticed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sense {
    Seen,
    /// Hit by them: where the damage came from, which for a blast is the blast.
    Hurt,
}

/// An enemy the bot knows of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Known {
    pub who: PlayerId,
    pub body: Entity,
    /// Where it was when last noticed: its center if seen.
    pub at: Vec3,
    pub how: Sense,
    /// Seconds of game time.
    pub last: f32,
    /// When it was first noticed since it was last forgotten, which the reaction runs from.
    pub since: f32,
    /// Seen this tick.
    pub in_sight: bool,
}

/// Something heard: where, roughly, and when, but not who.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Noise {
    pub at: Vec3,
    pub when: f32,
}

#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct BotSenses {
    pub known: Vec<Known>,
    pub noises: Vec<Noise>,
}

impl BotSenses {
    /// The enemies known long enough to act on.
    pub fn noticed(&self, now: f32, mind: &BotMind) -> impl Iterator<Item = &Known> {
        let reaction = mind.reaction;
        self.known.iter().filter(move |known| now - known.since >= reaction)
    }

    /// Noises heard long enough ago to act on.
    pub fn heard(&self, now: f32, mind: &BotMind) -> impl Iterator<Item = &Noise> {
        let reaction = mind.reaction;
        self.noises.iter().filter(move |noise| now - noise.when >= reaction)
    }

    fn notice(&mut self, who: PlayerId, body: Entity, at: Vec3, how: Sense, now: f32) {
        match self.known.iter_mut().find(|known| known.who == who) {
            Some(known) => {
                (known.body, known.at, known.how, known.last) = (body, at, how, now);
                known.in_sight |= how == Sense::Seen;
            }
            None => self.known.push(Known { who, body, at, how, last: now, since: now, in_sight: how == Sense::Seen }),
        }
    }

    fn forget(&mut self, now: f32) {
        self.known.retain(|known| now - known.last <= REMEMBERS);
        self.noises.retain(|noise| now - noise.when <= REMEMBERS_NOISES);
        for known in &mut self.known {
            known.in_sight = false;
        }
    }
}

/// Whether `target` is inside the view of an eye at `eye` looking along `yaw` and `pitch`, in the
/// step's convention: yaw about world Y with `-Z` at zero, pitch positive up.
pub fn in_view(eye: Vec3, yaw: f32, pitch: f32, target: Vec3) -> bool {
    let to = target - eye;
    if to.length() > SIGHT_RANGE {
        return false;
    }
    let across = (-to.x).atan2(-to.z);
    let turned = (across - yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    let up = to.y.atan2(to.xz().length());
    turned.abs() <= HALF_VIEW && (up - pitch).abs() <= HALF_VIEW_VERTICAL
}

/// Whether an eye sees any of `points`: in view, and with nothing solid between, by the test a
/// shot is.
pub fn sees(world: &CollisionWorld, eye: Vec3, yaw: f32, pitch: f32, points: &[Vec3]) -> bool {
    points.iter().any(|point| in_view(eye, yaw, pitch, *point) && world.segment_clear(eye, *point))
}

/// The baked sightlines' quick no, before any ray is cast.
fn could_see(graph: Option<&NavGraph>, eye_feet: Vec3, body_feet: Vec3) -> bool {
    let Some(graph) = graph.filter(|graph| graph.fields().has_sight()) else { return true };
    match (graph.cell_at(eye_feet), graph.cell_at(body_feet)) {
        (Some(eye), Some(body)) => graph.fields().sightlines.could_see(eye, body),
        _ => true,
    }
}

/// What every bot sees, hears and feels this tick. After the damage is applied, so this tick's
/// shots, blasts and hits are among what it notices, and after the boxes, so a head is where
/// this tick's step put it.
#[allow(clippy::type_complexity)]
pub fn sense(
    time: Res<Time>,
    world: Res<CollisionWorld>,
    nav: Option<Res<NavBake>>,
    mut effects: MessageReader<Effect>,
    mut dealt: MessageReader<DamageDealt>,
    mut bots: Query<(Entity, &PhysicsBody, &Stance, &Player, Option<&Team>, Option<&PlayerId>, &mut BotSenses), With<BotOf>>,
    bodies: Query<(Entity, &PlayerId, &PhysicsBody, &Stance, &Hitboxes, Option<&Team>), (With<Player>, Without<Dying>)>,
) {
    let now = time.elapsed_secs();
    let graph = nav.as_deref().map(NavBake::graph);
    let heard: Vec<(Vec3, f32, Option<PlayerId>)> = effects
        .read()
        .filter_map(|effect| match *effect {
            Effect::Tracer { from, by, .. } => Some((from, HEARS_SHOTS, by)),
            Effect::Blast { at, .. } => Some((at, HEARS_BLASTS, None)),
            Effect::Fired { .. } => None,
        })
        .collect();
    let hits: Vec<DamageDealt> = dealt.read().copied().collect();

    for (bot, body, stance, player, team, id, mut senses) in &mut bots {
        senses.forget(now);
        let center = body.current;
        let feet = center - Vec3::Y * stance.half_extents().y;
        let eye = center + Vec3::Y * player.duck.eye_from_center(*stance);

        for (other, who, their, their_stance, boxes, their_team) in &bodies {
            if other == bot || !hostile(team.copied(), their_team.copied()) {
                continue;
            }
            let their_feet = their.current - Vec3::Y * their_stance.half_extents().y;
            if !could_see(graph, feet, their_feet) {
                continue;
            }
            let head = boxes.head.map_or(their.current + Vec3::Y * their_stance.half_extents().y * 0.8, |head| head.center);
            if sees(&world, eye, player.yaw, player.pitch, &[head, boxes.body.center]) {
                senses.notice(*who, other, their.current, Sense::Seen, now);
            }
        }

        for &(at, earshot, by) in &heard {
            if by.is_some() && by == id.copied() {
                continue;
            }
            if at.distance(center) <= earshot {
                senses.noises.push(Noise { at, when: now });
            }
        }

        for hit in hits.iter().filter(|hit| hit.target == bot) {
            let DamageSource::Player(from) = hit.source else { continue };
            if Some(from) == id.copied() {
                continue;
            }
            let Some((attacker, ..)) = bodies.iter().find(|(_, who, ..)| **who == from) else { continue };
            senses.notice(from, attacker, hit.from.unwrap_or(hit.point), Sense::Hurt, now);
        }
    }
}

pub struct BotSensePlugin;

impl Plugin for BotSensePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Effect>().add_message::<DamageDealt>().add_systems(
            FixedUpdate,
            sense
                .after(DamageSystems::Apply)
                .after(update_hitboxes)
                .run_if(has_authority)
                .run_if(in_state(AppMode::Play)),
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::hitbox::Box3;
    use crate::game::collision::CollisionWorld;
    use crate::tool::room::Room;

    fn a_world(rooms: &[Room]) -> World {
        let mut world = World::new();
        let mut collision = CollisionWorld::default();
        collision.rebuild(rooms);
        world.insert_resource(collision);
        world.init_resource::<Time>();
        world.init_resource::<Messages<Effect>>();
        world.init_resource::<Messages<DamageDealt>>();
        world
    }

    fn a_body(world: &mut World, at: Vec3, yaw: f32, who: u64) -> Entity {
        let center = crate::common::class::body_center_from_feet(at);
        world
            .spawn((
                Player { yaw, ..default() },
                PhysicsBody::at(center),
                Stance::Standing,
                PlayerId(who),
                Hitboxes { body: Box3 { center, half_extents: Stance::Standing.half_extents() }, head: Some(Box3 { center: center + Vec3::Y * 0.7, half_extents: Vec3::splat(0.15) }) },
            ))
            .id()
    }

    fn a_bot(world: &mut World, at: Vec3, yaw: f32) -> Entity {
        let bot = a_body(world, at, yaw, 1);
        world.entity_mut(bot).insert((BotOf(Entity::PLACEHOLDER), BotSenses::default()));
        bot
    }

    fn seen(world: &World, bot: Entity) -> Vec<PlayerId> {
        world.get::<BotSenses>(bot).unwrap().known.iter().filter(|known| known.in_sight).map(|known| known.who).collect()
    }

    fn open() -> Room {
        Room::new(Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 6.0, 20.0))
    }

    /// Breaks if a bot sees behind itself, or past the edge of its view: looking north, a body
    /// ahead is seen, one behind and one far to the side are not.
    #[test]
    fn a_bot_sees_ahead_and_not_behind_or_beside() {
        let mut world = a_world(&[open()]);
        let bot = a_bot(&mut world, Vec3::ZERO, 0.0);
        a_body(&mut world, Vec3::new(0.0, 0.0, -10.0), 0.0, 2);
        a_body(&mut world, Vec3::new(0.0, 0.0, 10.0), 0.0, 3);
        a_body(&mut world, Vec3::new(10.0, 0.0, -2.0), 0.0, 4);
        world.run_system_once(sense).unwrap();
        assert_eq!(seen(&world, bot), vec![PlayerId(2)]);
    }

    /// Breaks if sight goes through walls: a body straight ahead behind a pillar is not seen.
    #[test]
    fn a_bot_does_not_see_through_a_wall() {
        let mut world = a_world(&[open()]);
        let mut collision = CollisionWorld::default();
        collision.rebuild_volumes(&[
            crate::common::world_surface::Volume::air(open()),
            crate::common::world_surface::Volume::solid(Room::new(Vec3::new(-2.0, 0.0, -6.0), Vec3::new(2.0, 6.0, -5.0))),
        ]);
        world.insert_resource(collision);
        let bot = a_bot(&mut world, Vec3::ZERO, 0.0);
        a_body(&mut world, Vec3::new(0.0, 0.0, -10.0), 0.0, 2);
        world.run_system_once(sense).unwrap();
        assert!(seen(&world, bot).is_empty(), "the bot saw through the pillar");
    }

    /// Breaks if a bot is deaf behind itself: a shot behind it is heard where it was fired.
    #[test]
    fn a_shot_behind_a_bot_is_heard() {
        let mut world = a_world(&[open()]);
        let bot = a_bot(&mut world, Vec3::ZERO, 0.0);
        let from = Vec3::new(0.0, 1.5, 12.0);
        world.write_message(Effect::Tracer { from, to: Vec3::new(5.0, 1.5, 0.0), hit: false, by: Some(PlayerId(2)) });
        world.run_system_once(sense).unwrap();
        let senses = world.get::<BotSenses>(bot).unwrap();
        assert_eq!(senses.noises.len(), 1);
        assert_eq!(senses.noises[0].at, from);
        assert!(senses.known.is_empty(), "a noise told the bot who made it");
    }

    /// Breaks if a teammate is treated as an enemy.
    #[test]
    fn a_teammate_in_view_is_not_an_enemy() {
        let mut world = a_world(&[open()]);
        let bot = a_bot(&mut world, Vec3::ZERO, 0.0);
        world.entity_mut(bot).insert(Team::Red);
        let friend = a_body(&mut world, Vec3::new(0.0, 0.0, -10.0), 0.0, 2);
        world.entity_mut(friend).insert(Team::Red);
        world.run_system_once(sense).unwrap();
        assert!(seen(&world, bot).is_empty());
    }

    /// Breaks if a bot acts the instant it notices, or never forgets: an enemy is noticed only
    /// after the reaction time, and gone from memory a few seconds after it was last seen.
    #[test]
    fn noticing_waits_for_the_reaction_and_memory_fades() {
        let mind = BotMind { reaction: 0.3 };
        let mut senses = BotSenses::default();
        senses.notice(PlayerId(2), Entity::PLACEHOLDER, Vec3::ZERO, Sense::Seen, 10.0);
        assert_eq!(senses.noticed(10.1, &mind).count(), 0, "noticed before the reaction time");
        assert_eq!(senses.noticed(10.3, &mind).count(), 1);
        senses.forget(10.0 + REMEMBERS - 0.1);
        assert_eq!(senses.known.len(), 1, "forgot too soon");
        senses.forget(10.0 + REMEMBERS + 0.1);
        assert!(senses.known.is_empty(), "never forgot");
    }
}
