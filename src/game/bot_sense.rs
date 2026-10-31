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
use crate::common::player_color::PlayerColor;
use crate::common::swatch::Swatch;
use crate::common::team::{hostile, Team};
use crate::game::bot::{BotOf, ShowBots};
use crate::game::bot_fight::Aim;
use crate::game::bot_think::Drives;
use crate::game::collision::CollisionWorld;
use crate::game::damage::DamageSystems;
use crate::game::death::Dying;
use crate::game::hitbox::update_hitboxes;
use crate::game::nav::NavGraph;
use crate::game::player::{PhysicsBody, Player};
use crate::tool::nav_bake::NavBake;

/// Radians either side of the look: a 90° view.
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
/// Meters along the direction a hit came from that a bot guesses its attacker is, unless a wall
/// is nearer. A hit gives a direction, not a distance.
const HURT_GUESS: f32 = 10.0;
/// Seconds between noticing something and acting on it, the range a temperament rolls in.
const REACTION: std::ops::Range<f32> = 0.2..0.4;

/// Rolled per seat and kept across lives, like [`RouteCosts`](crate::game::bot::RouteCosts).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct BotMind {
    /// Seconds between noticing an enemy and acting on it; acting at once reads as cheating.
    pub reaction: f32,
    pub drives: Drives,
    pub aim: Aim,
}

impl Default for BotMind {
    fn default() -> Self {
        BotMind { reaction: 0.3, drives: Drives::default(), aim: Aim::default() }
    }
}

impl BotMind {
    pub fn temperament() -> Self {
        BotMind { reaction: rand::random_range(REACTION), drives: Drives::temperament(), aim: Aim::temperament() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sense {
    Seen,
    /// Hit by them, which gives only the direction the damage came from.
    Hurt,
}

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
    let turned = crate::common::rotation::wrap_angle(across - yaw);
    let up = to.y.atan2(to.xz().length());
    turned.abs() <= HALF_VIEW && (up - pitch).abs() <= HALF_VIEW_VERTICAL
}

/// In view and with nothing solid between, by the ray test a shot uses.
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
            // Seen, it already knows better than a direction.
            if senses.known.iter().any(|known| known.who == from && known.in_sight) {
                continue;
            }
            let Ok(toward) = Dir3::new(hit.from.unwrap_or(hit.point) - eye) else { continue };
            let reach = world.ray_distance(&Ray3d::new(eye, toward), HURT_GUESS).unwrap_or(HURT_GUESS);
            senses.notice(from, attacker, eye + *toward * reach, Sense::Hurt, now);
        }
    }
}

/// Meters out from the eye the view is drawn.
const VIEW_DRAWN: f32 = 2.0;
/// Segments along each arc of the view's far edge.
const VIEW_ARC_STEPS: usize = 8;
/// Meters of line, then of gap, in a dashed line.
const DASH: (f32, f32) = (0.3, 0.2);
/// Least alpha an enemy about to be forgotten is drawn with, so it does not vanish before it is.
const FADED: f32 = 0.15;

/// In [`in_view`]'s convention.
pub fn look(yaw: f32, pitch: f32) -> Vec3 {
    let pitch = pitch.clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
    Vec3::new(-yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos())
}

fn dashed(gizmos: &mut Gizmos, from: Vec3, to: Vec3, color: Color) {
    let length = from.distance(to);
    let along = (to - from).normalize_or_zero();
    let mut at = 0.0;
    while at < length {
        gizmos.line(from + along * at, from + along * (at + DASH.0).min(length), color);
        at += DASH.0 + DASH.1;
    }
}

/// Each bot's view as [`in_view`] tests it, and a line to each enemy it knows of: solid while
/// seen, dashed when only hurt by them, fading as it forgets.
fn draw_bot_senses(
    show: Res<ShowBots>,
    time: Res<Time>,
    mut gizmos: Gizmos,
    bots: Query<(&PhysicsBody, &Stance, &Player, &BotSenses, Option<&Team>, Option<&PlayerColor>), With<BotOf>>,
) {
    if !show.0 {
        return;
    }
    let now = time.elapsed_secs();
    for (body, stance, player, senses, team, color) in &bots {
        let color = Swatch::of_body(team.copied(), color.copied()).base;
        let eye = body.current + Vec3::Y * player.duck.eye_from_center(*stance);
        let at = |yaw: f32, pitch: f32| eye + look(player.yaw + yaw, player.pitch + pitch) * VIEW_DRAWN;
        let (across, up) = (HALF_VIEW, HALF_VIEW_VERTICAL);
        for (yaw, pitch) in [(-across, -up), (across, -up), (across, up), (-across, up)] {
            gizmos.line(eye, at(yaw, pitch), color);
        }
        let steps = (0..=VIEW_ARC_STEPS).map(|step| step as f32 / VIEW_ARC_STEPS as f32 * 2.0 - 1.0);
        for edge in [-up, up] {
            gizmos.linestrip(steps.clone().map(|t| at(t * across, edge)), color);
        }
        for edge in [-across, across] {
            gizmos.linestrip(steps.clone().map(|t| at(edge, t * up)), color);
        }

        for known in &senses.known {
            let fade = if known.in_sight { 1.0 } else { (1.0 - (now - known.last) / REMEMBERS).max(FADED) };
            let shade = color.with_alpha(fade);
            match known.how {
                Sense::Seen => gizmos.line(eye, known.at, shade),
                Sense::Hurt => dashed(&mut gizmos, eye, known.at, shade),
            }
        }
    }
}

pub struct BotSensePlugin;

impl Plugin for BotSensePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Effect>()
            .add_message::<DamageDealt>()
            .init_resource::<ShowBots>()
            .add_systems(
                FixedUpdate,
                sense
                    .after(DamageSystems::Apply)
                    .after(update_hitboxes)
                    .run_if(has_authority)
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(Update, draw_bot_senses.run_if(in_state(AppMode::Play)));
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

    /// Breaks if the drawn view and the tested one disagree: just inside each corner the debug
    /// view draws is in view, and just outside is not.
    #[test]
    fn the_view_drawn_is_the_view_tested() {
        let eye = Vec3::new(1.0, 1.6, -2.0);
        for (yaw, pitch) in [(0.0, 0.0), (1.2, 0.2), (-2.5, -0.3)] {
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let corner = |k: f32| eye + look(yaw + sx * HALF_VIEW * k, pitch + sy * HALF_VIEW_VERTICAL * k) * 5.0;
                assert!(in_view(eye, yaw, pitch, corner(0.98)), "inside the corner at {yaw}, {pitch}");
                assert!(!in_view(eye, yaw, pitch, corner(1.02)), "outside the corner at {yaw}, {pitch}");
            }
        }
    }

    /// Breaks if a bot follows somebody it has lost sight of: seen in the open, then moved
    /// behind a pillar, they are remembered where they were last seen.
    #[test]
    fn a_lost_enemy_is_remembered_where_it_was_last_seen() {
        let mut world = a_world(&[open()]);
        let mut collision = CollisionWorld::default();
        collision.rebuild_volumes(&[
            crate::common::world_surface::Volume::air(open()),
            crate::common::world_surface::Volume::solid(Room::new(Vec3::new(-2.0, 0.0, -6.0), Vec3::new(2.0, 6.0, -5.0))),
        ]);
        world.insert_resource(collision);
        let bot = a_bot(&mut world, Vec3::ZERO, 0.0);
        let enemy = a_body(&mut world, Vec3::new(5.0, 0.0, -10.0), 0.0, 2);
        world.run_system_once(sense).unwrap();
        let first = world.get::<PhysicsBody>(enemy).unwrap().current;
        assert_eq!(seen(&world, bot), vec![PlayerId(2)]);

        let hidden = crate::common::class::body_center_from_feet(Vec3::new(0.0, 0.0, -10.0));
        world.entity_mut(enemy).insert((
            PhysicsBody::at(hidden),
            Hitboxes { body: Box3 { center: hidden, half_extents: Stance::Standing.half_extents() }, head: Some(Box3 { center: hidden + Vec3::Y * 0.7, half_extents: Vec3::splat(0.15) }) },
        ));
        world.run_system_once(sense).unwrap();
        let known = world.get::<BotSenses>(bot).unwrap().known[0];
        assert!(!known.in_sight, "the bot saw through the pillar");
        assert_eq!(known.at, first, "the bot knows where its enemy went");
    }

    /// Breaks if being hit tells a bot where its attacker is rather than which way: shot from far
    /// behind, it knows the direction and guesses the distance.
    #[test]
    fn a_hit_gives_a_direction_and_not_a_place() {
        let mut world = a_world(&[open()]);
        let bot = a_bot(&mut world, Vec3::ZERO, 0.0);
        let shooter = a_body(&mut world, Vec3::new(0.0, 0.0, 18.0), 0.0, 2);
        let muzzle = world.get::<PhysicsBody>(shooter).unwrap().current;
        world.write_message(DamageDealt {
            target: bot,
            source: DamageSource::Player(PlayerId(2)),
            amount: 10,
            remaining: 90,
            point: Vec3::new(0.0, 1.0, 0.0),
            from: Some(muzzle),
        });
        world.run_system_once(sense).unwrap();
        let known = world.get::<BotSenses>(bot).unwrap().known[0];
        assert_eq!(known.how, Sense::Hurt);
        let eye = world.get::<PhysicsBody>(bot).unwrap().current + Vec3::Y * Player::default().duck.eye_from_center(Stance::Standing);
        assert!((known.at - eye).normalize().dot((muzzle - eye).normalize()) > 0.999, "it does not know the direction");
        assert!((known.at.distance(eye) - HURT_GUESS).abs() < 1e-3, "it knows the distance");
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
        let mind = BotMind { reaction: 0.3, ..default() };
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
