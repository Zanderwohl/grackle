//! Bots: bodies driven by the authority rather than a keyboard. See documentation/bots.md.
//!
//! A bot is a [`BotSeat`], an entity that holds an [`Identity`](crate::game::respawn::Identity) the way a client's link does and
//! is served a body by `give_bodies_to_whoever_needs_one` under the same rule. The body is an
//! ordinary player body plus [`BotOf`] and a [`BotIntent`]: whatever decides what the bot wants
//! writes the intent, and [`steer_bots`] turns it into the body's `Inputs` each tick, the component
//! a client's keyboard fills. The step, the weapons, the animation and the network cannot tell a
//! bot from a person.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::net::has_authority;
use crate::common::round_settings::RoundSettings;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::Connected;
use crate::common::damage::{through_armor, Armor, Damageable};
use crate::common::game_rules::GameRules;
use crate::common::weapon::{Ammo, Equipped, WeaponId};
use crate::game::weapon::Trigger;
use crate::common::player_color::PlayerColor;
use crate::common::swatch::Swatch;
use crate::common::team::Team;
use crate::game::respawn::{Identity, OurIdentity};
use std::collections::HashMap;
use crate::game::nav::{CellId, Leap, Link, LinkKind, NavGraph};
use crate::game::nav_field::Field;
use crate::game::player::{step_player, Inputs, PhysicsBody, Player, PlayerInput};
use crate::tool::nav_bake::NavBake;

/// Meters across the floor within which a cell counts as reached.
const ARRIVED: f32 = 0.35;
/// Meters up or down within which a cell counts as reached, so a drop is not reached from above.
const ARRIVED_HEIGHT: f32 = 0.6;
/// Seconds without closing on the next cell after which the route is planned again.
const STUCK_SECONDS: f32 = 1.5;
/// Meters closer to the next cell that count as progress.
const PROGRESS: f32 = 0.1;
/// Meters ahead within which a crouch on the route is ducked for. A duck takes a fifth of a second
/// before the box shrinks, so ducking only on arrival walks the bot into the lintel first.
const CROUCH_AHEAD: f32 = 1.5;

/// A bot's place in the match, alive or not. Holds the
/// [`Identity`](crate::game::respawn::Identity) once it has been given one,
/// so a respawn is the same bot on the same team. Despawning it retires the bot.
#[derive(Component, Default, Debug)]
pub struct BotSeat;

/// On a bot's body: the seat it is driven for.
#[derive(Component, Clone, Copy, Debug)]
pub struct BotOf(pub Entity);

/// Where a bot is going.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Goal {
    /// To stand here, as feet.
    Place(Vec3),
    /// To take the pickup at this place in the baked pickups, standing or in flight.
    Pickup(u16),
}

/// What a bot wants this tick, in the world's terms rather than the keyboard's.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct BotIntent {
    /// Walked to along the nav graph.
    pub goal: Option<Goal>,
    /// Where to look. With nothing, the bot looks the way it is going.
    pub look_at: Option<Vec3>,
    pub sprint: bool,
    pub crouch: bool,
    pub fire: bool,
    pub alt_fire: bool,
    pub reload: bool,
    pub select: Option<WeaponId>,
}

/// How a bot weighs a route beyond the time it takes. Each link costs its seconds, and on top its
/// seconds again for every weighted unit of how exposed, how dangerous and how hurtful the cell it
/// enters is: time spent there is what is paid for, so risk is priced rather than forbidden, and a
/// dangerous way that saves enough time is still taken. Skill and temperament are these weights,
/// not a different pathfinder. A field not baked yet costs nothing.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct RouteCosts {
    /// Per 100 m² of exposure: at 1, a cell seen from 100 m² of floor costs its time twice over.
    pub exposure: f32,
    /// Per share of knockback headings that throw a body somewhere it dies.
    pub danger: f32,
    /// Per 10 damage a second from a Hurt trigger.
    pub hurt: f32,
    /// Per 10 damage a rocket jump does its jumper.
    pub self_damage: f32,
    /// The chance, each time it plans, that the bot forgets to ask whether a rocket jump would
    /// kill it.
    pub shortsightedness: f32,
    /// How wrong the bot can be about a stretch of floor, 0 to 1. At 1 a stretch may read as safe
    /// however dangerous it is, or as twice as risky, and dear even where nothing threatens.
    pub misjudgment: f32,
    /// Which stretches it is wrong about, and which way. Rolled again each life.
    pub beliefs: u64,
}

impl Default for RouteCosts {
    fn default() -> Self {
        RouteCosts { exposure: 0.5, danger: 4.0, hurt: 2.0, self_damage: 1.0, shortsightedness: 0.0, misjudgment: 0.0, beliefs: 0 }
    }
}

/// The range each weight of a [`RouteCosts::temperament`] is scaled by: some bots barely mind.
const TEMPERAMENT: std::ops::Range<f32> = 0.1..1.5;
/// The most shortsightedness a temperament rolls.
const MOST_SHORTSIGHTEDNESS: f32 = 0.1;
/// The most misjudgment a temperament rolls.
const MOST_MISJUDGMENT: f32 = 0.8;
/// Meters across a stretch of floor misjudged as one, so a mistake covers a corridor or a ledge
/// rather than averaging away cell by cell along a route.
const MISJUDGED_STRETCH: f32 = 4.0;
/// At full misjudgment, how much of a safe stretch's own time it may be thought to cost on top.
const PHANTOM_RISK: f32 = 1.0;

impl RouteCosts {
    /// The defaults, each scaled at random within [`TEMPERAMENT`], so bots are not all equally
    /// careful and none is perfectly so. Rolled once per seat and kept across its lives.
    pub fn temperament() -> Self {
        let base = RouteCosts::default();
        let roll = |weight: f32| weight * rand::random_range(TEMPERAMENT);
        RouteCosts {
            exposure: roll(base.exposure),
            danger: roll(base.danger),
            hurt: roll(base.hurt),
            self_damage: roll(base.self_damage),
            shortsightedness: rand::random_range(0.0..MOST_SHORTSIGHTEDNESS),
            misjudgment: rand::random_range(0.0..MOST_MISJUDGMENT),
            beliefs: rand::random(),
        }
    }

    /// Seconds on top of `link`'s own for entering `cell` by it, as this bot sees it.
    pub fn extra(&self, graph: &NavGraph, cell: CellId, link: &Link) -> f32 {
        let field = |field| graph.field(cell, field).unwrap_or(0.0);
        let hurt = graph.cells()[cell as usize].hurt as f32;
        let risk = link.seconds * (self.exposure * field(Field::Exposure) / 100.0 + self.danger * field(Field::Knockback) + self.hurt * hurt / 10.0);
        let wrong = self.misjudgment * self.belief(graph.cells()[cell as usize].feet);
        let hurt_self = link.leap.map_or(0.0, |leap| leap.damage as f32 / 10.0 * self.self_damage);
        (risk * (1.0 + wrong) + link.seconds * PHANTOM_RISK * wrong.max(0.0)).max(0.0) + hurt_self
    }

    /// How wrong this bot is about the stretch of floor at `feet`, -1 to 1: the same for every cell
    /// in a [`MISJUDGED_STRETCH`] square at one height.
    fn belief(&self, feet: Vec3) -> f32 {
        let stretch = (feet / MISJUDGED_STRETCH).floor().as_ivec3();
        // SplitMix64 over the beliefs and the stretch, so a mistake is the same every replan.
        let mut hash = self.beliefs ^ (stretch.x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (stretch.y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f) ^ (stretch.z as u64).wrapping_mul(0x1656_67b1_9e37_79f9);
        hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        hash ^= hash >> 31;
        (hash >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
    }
}

/// How a bot is getting to its goal: the route planned and how far along it is. [`steer_bots`]'s
/// own, replanned whenever the goal, the graph or the bot's progress says it no longer holds.
#[derive(Component, Clone, Debug, Default)]
pub struct BotRoute {
    /// The goal the route was planned to, and the graph it was planned on.
    planned: Option<(Goal, u64)>,
    /// The cell the route starts from.
    from: Option<CellId>,
    links: Vec<Link>,
    /// The next link to take.
    next: usize,
    /// Ticks into the leap being replayed, if one is.
    leaping: Option<u32>,
    /// Nearest the next cell has been, and the seconds since it got nearer.
    closest: f32,
    stalled: f32,
    /// Seconds waiting at a rocket jump's start for the launcher.
    waited: f32,
    /// This plan forgot to ask whether a rocket jump would kill the bot.
    careless: bool,
}

impl BotRoute {
    /// The links still to take.
    pub fn ahead(&self) -> &[Link] {
        &self.links[self.next.min(self.links.len())..]
    }

    /// Planned, and nothing left to take: arrived, or the goal cannot be reached.
    pub fn is_done(&self) -> bool {
        self.planned.is_some() && self.ahead().is_empty()
    }

    /// Planned, and no way there was found.
    pub fn is_lost(&self) -> bool {
        self.planned.is_some() && self.links.is_empty()
    }

    fn forget(&mut self) {
        *self = BotRoute::default();
    }
}

/// Where a body's feet are, from the center of its box.
fn feet_of(body: &PhysicsBody, stance: Stance) -> Vec3 {
    body.current - Vec3::Y * stance.half_extents().y
}

fn reached(feet: Vec3, cell: Vec3) -> bool {
    (feet - cell).xz().length() <= ARRIVED && (feet.y - cell.y).abs() <= ARRIVED_HEIGHT
}

/// Yaw and pitch that look from `eye` at `target`, in the step's convention: yaw about world Y
/// with `-Z` at zero, pitch positive up.
pub fn aim_at(eye: Vec3, target: Vec3) -> (f32, f32) {
    let to = target - eye;
    let yaw = (-to.x).atan2(-to.z);
    let pitch = to.y.atan2(to.xz().length());
    (yaw, pitch)
}

/// `direction` across the floor, in the frame of a body facing `yaw`: `x` right, `y` forward.
fn movement_toward(direction: Vec2, yaw: f32) -> Vec2 {
    let forward = Vec2::new(-yaw.sin(), -yaw.cos());
    let right = Vec2::new(yaw.cos(), -yaw.sin());
    Vec2::new(direction.dot(right), direction.dot(forward)).clamp_length_max(1.0)
}

/// What a bot has to hand for the route: how fast it is going, what it holds, whether that would
/// fire now, and which weapons it could rocket jump on.
#[derive(Clone, Copy, Debug, Default)]
pub struct Gear<'a> {
    /// Meters per second across the floor.
    pub speed: f32,
    pub held: Option<WeaponId>,
    /// The held weapon is drawn, off its cooldown and loaded.
    pub ready: bool,
    /// Standing all the way up, as a baked rocket jump starts.
    pub upright: bool,
    pub health: u32,
    pub armor: u32,
    /// The mode's scale on hurting oneself.
    pub self_damage: f32,
    /// Carried with a round to fire.
    pub loaded: &'a [WeaponId],
}

impl Gear<'_> {
    /// A rocket jump needs its launcher loaded, and unless `careless`, a body it would not kill.
    fn can_take(&self, link: &Link, careless: bool) -> bool {
        let Some(leap) = link.leap.filter(|leap| leap.rocket.is_some()) else { return true };
        leap.rocket.is_some_and(|rocket| self.loaded.contains(&rocket)) && (careless || !self.would_die(&leap))
    }

    fn would_die(&self, leap: &Leap) -> bool {
        let hurt = (leap.damage as f32 * self.self_damage).round() as u32;
        through_armor(hurt, self.armor) >= self.health
    }
}

/// Plans `route` to `goal` from `feet` if what it was planned for has changed. A rocket jump the
/// bot has nothing to fire for is a link it cannot take.
fn plan(route: &mut BotRoute, graph: &NavGraph, generation: u64, feet: Vec3, goal: Option<Goal>, costs: &RouteCosts, gear: &Gear) {
    let Some(goal) = goal else {
        route.forget();
        return;
    };
    if route.planned == Some((goal, generation)) {
        return;
    }
    route.forget();
    route.planned = Some((goal, generation));
    route.careless = rand::random::<f32>() < costs.shortsightedness;
    let Some(from) = graph.cell_at(feet) else { return };
    let careless = route.careless;
    let extra = |cell, link: &Link| if gear.can_take(link, careless) { costs.extra(graph, cell, link) } else { f32::INFINITY };
    let path = match goal {
        Goal::Place(place) => graph.cell_at(place).and_then(|to| graph.path(from, to, extra)),
        Goal::Pickup(pickup) => graph.path_to_pickup(from, pickup, extra),
    };
    route.from = Some(from);
    route.links = path.unwrap_or_default();
    route.closest = f32::INFINITY;
}

#[cfg(test)]
pub(crate) fn plan_for_test(route: &mut BotRoute, graph: &NavGraph, feet: Vec3, goal: Goal) {
    plan(route, graph, 0, feet, Some(goal), &RouteCosts::default(), &Gear::default());
}

/// Meters per second under which a bot counts as standing still, to set off a rocket jump from.
const STILL: f32 = 0.5;
/// Meters from a rocket jump's start within which a bot crouch-walks the rest of the way, and
/// within which it counts as there. A run-up of a few ticks from a stride past the start goes off
/// the edge before it jumps.
const ROCKET_APPROACH: f32 = 1.0;
const ROCKET_ARRIVED: f32 = 0.1;
/// Seconds a bot waits at a rocket jump's start for its launcher before planning again.
const MOST_WAIT: f32 = 3.0;
/// Links ahead within which a bot draws the launcher a rocket jump needs, so it has deployed.
const DRAW_AHEAD: usize = 4;

/// One tick of following `route`: the movement and facing it calls for, and whether to jump.
/// A drop or a jump is replayed from the input the bake found it with, once the bot is on the
/// cell it starts from.
fn follow(route: &mut BotRoute, graph: &NavGraph, feet: Vec3, gear: &Gear, dt: f32) -> Option<PlayerInput> {
    let cells = graph.cells();
    let link = *route.ahead().first()?;
    let start = match route.next.checked_sub(1) {
        Some(previous) => cells[route.links[previous].to as usize].feet,
        None => route.from.map_or(feet, |from| cells[from as usize].feet),
    };

    if let (Some(tick), Some(leap)) = (route.leaping, link.leap) {
        if tick >= leap.ticks {
            route.leaping = None;
            route.next += 1;
            route.closest = f32::INFINITY;
            // Landed short or long: the next link may not start here, so plan from where it is.
            if !reached(feet, cells[link.to as usize].feet) {
                route.planned = None;
            }
            return None;
        }
        route.leaping = Some(tick + 1);
        return Some(leap.input(tick));
    }

    let target = cells[link.to as usize].feet;
    let rocket = link.leap.and_then(|leap| leap.rocket);
    let from_start = (start - feet).xz().length();
    let level = (start.y - feet.y).abs() <= ARRIVED_HEIGHT;
    if rocket.is_some() && level && from_start > ROCKET_ARRIVED && from_start < ROCKET_APPROACH {
        let select = rocket.filter(|rocket| gear.held != Some(*rocket));
        return Some(PlayerInput { movement: Vec2::Y, yaw: aim_at(feet, start).0, crouch: true, select, ..default() });
    }
    let at_start = if rocket.is_some() { level && from_start <= ROCKET_ARRIVED } else { reached(feet, start) };
    if let Some(leap) = link.leap.filter(|_| at_start) {
        // Hurt since it was planned: a bot that remembered to ask asks again.
        if !gear.can_take(&link, route.careless) {
            route.planned = None;
            return None;
        }
        // A rocket jump starts still and upright, with the launcher drawn, so it goes as baked.
        let set = leap.rocket.is_none_or(|rocket| gear.held == Some(rocket) && gear.ready && gear.upright && gear.speed < STILL);
        if set {
            route.waited = 0.0;
            route.leaping = Some(0);
            return follow(route, graph, feet, gear, dt);
        }
        route.waited += dt;
        if route.waited > MOST_WAIT {
            route.planned = None;
        }
        return Some(PlayerInput { yaw: leap.yaw, select: leap.rocket.filter(|rocket| gear.held != Some(*rocket)), ..default() });
    }
    if reached(feet, target) {
        route.next += 1;
        route.closest = f32::INFINITY;
        return follow(route, graph, feet, gear, dt);
    }

    // A leap is set off from its start; anything else is walked straight at.
    let toward = if link.leap.is_some() { start } else { target };
    let away = (toward - feet).xz();
    let distance = away.length();
    if distance < route.closest - PROGRESS {
        (route.closest, route.stalled) = (distance, 0.0);
    } else {
        route.stalled += dt;
        if route.stalled > STUCK_SECONDS {
            route.planned = None;
        }
    }
    let yaw = aim_at(feet, toward).0;
    let low = |link: &Link| link.kind == LinkKind::Crouch || cells[link.to as usize].crouched;
    let crouch = route.ahead().iter().take_while(|link| (cells[link.to as usize].feet - feet).length() <= CROUCH_AHEAD).any(low)
        || graph.cell_at(feet).is_some_and(|here| cells[here as usize].crouched);
    let launcher = route.ahead().iter().take(DRAW_AHEAD).find_map(|link| link.leap.and_then(|leap| leap.rocket));
    let select = launcher.filter(|rocket| gear.held != Some(*rocket));
    Some(PlayerInput { movement: Vec2::Y, yaw, crouch, select, ..default() })
}

/// The cell a bot's route is heading for next, if it has one.
pub fn heading_for(route: &BotRoute) -> Option<CellId> {
    route.ahead().first().map(|link| link.to)
}

/// Writes every bot body's `Inputs` from its intent and route, before the step reads them.
#[allow(clippy::type_complexity)]
pub fn steer_bots(
    time: Res<Time>,
    nav: Option<Res<NavBake>>,
    rules: Option<Res<GameRules>>,
    mut bots: Query<
        (
            (&PhysicsBody, &Stance, &Player, &BotIntent, Option<&RouteCosts>),
            (&mut BotRoute, &mut Inputs, Option<(&Equipped, &Ammo, &Trigger)>, Option<&Damageable>, Option<&Armor>),
        ),
        With<BotOf>,
    >,
) {
    let dt = time.delta_secs();
    for ((body, stance, player, intent, costs), (mut route, mut inputs, arms, health, armor)) in &mut bots {
        let feet = feet_of(body, *stance);
        let loaded: Vec<WeaponId> = arms.map_or_else(Vec::new, |(equipped, ammo, _)| {
            equipped.carried().iter().copied().filter(|weapon| ammo.at(*weapon).is_some_and(|state| state.clip + state.reserve > 0)).collect()
        });
        let gear = Gear {
            speed: player.velocity.xz().length(),
            held: arms.and_then(|(equipped, ..)| equipped.held()),
            ready: arms.is_some_and(|(equipped, ammo, trigger)| {
                trigger.primary_ready() && equipped.held().and_then(|held| ammo.at(held)).is_some_and(|state| state.clip > 0 && state.reloading <= 0.0)
            }),
            upright: *stance == Stance::Standing && player.duck.amount == 0.0,
            health: health.map_or(u32::MAX, Damageable::health),
            armor: armor.map_or(0, Armor::points),
            self_damage: rules.as_deref().map_or(1.0, |rules| rules.0.self_damage),
            loaded: &loaded,
        };
        let walking = nav.as_deref().and_then(|nav| {
            plan(&mut route, nav.graph(), nav.generation(), feet, intent.goal, &costs.copied().unwrap_or_default(), &gear);
            follow(&mut route, nav.graph(), feet, &gear, dt)
        });
        let mut input = walking.unwrap_or(PlayerInput { yaw: player.yaw, pitch: player.pitch, ..default() });
        let leaping = route.leaping.is_some();
        // A leap's own facing is part of what makes it land, so looking elsewhere waits for it.
        if let Some(target) = intent.look_at.filter(|_| !leaping) {
            let eye = body.current + Vec3::Y * player.duck.eye_from_center(*stance);
            let (yaw, pitch) = aim_at(eye, target);
            let world = Vec2::new(-input.yaw.sin(), -input.yaw.cos()) * input.movement.length();
            input.movement = movement_toward(world, yaw);
            (input.yaw, input.pitch) = (yaw, pitch);
        }
        // A leap's own buttons are part of what makes it land, a rocket jump's crouch and shot
        // among them, so the intent's wait for it.
        if !leaping {
            input.sprint |= intent.sprint;
            input.crouch |= intent.crouch;
            input.attack |= intent.fire;
            input.attack_held |= intent.fire;
            input.alt_attack = intent.alt_fire;
            input.alt_attack_held = intent.alt_fire;
            input.reload = intent.reload;
            // The route's draw first: a rocket jump waits at its start for that launcher.
            input.select = input.select.or(intent.select);
        }
        inputs.0 = input;
    }
}

/// Bodies whose seat is gone go with it.
fn retire_bots(mut commands: Commands, bodies: Query<(Entity, &BotOf)>, seats: Query<(), With<BotSeat>>) {
    for (body, seat) in &bodies {
        if !seats.contains(seat.0) {
            commands.entity(body).despawn();
        }
    }
}

/// Whether each bot's route and senses are drawn. `F8` toggles it; off by default, a debug view.
#[derive(Resource, Default, Debug)]
pub struct ShowBots(pub bool);

fn toggle_bot_routes(keys: Res<ButtonInput<KeyCode>>, mut show: ResMut<ShowBots>) {
    if keys.just_pressed(KeyCode::F8) {
        show.0 = !show.0;
    }
}

/// Meters above the floor a route is drawn, so it does not fight the floor for depth.
const ROUTE_LIFT: f32 = 0.1;

/// From each bot's feet through the cells it has yet to reach, to a ring at its goal, in the
/// bot's own color. Routes live on the authority, so a client draws none.
fn draw_bot_routes(
    show: Res<ShowBots>,
    nav: Option<Res<NavBake>>,
    mut gizmos: Gizmos,
    bots: Query<(&PhysicsBody, &Stance, &BotRoute, &BotIntent, Option<&Team>, Option<&PlayerColor>), With<BotOf>>,
) {
    let (true, Some(nav)) = (show.0, nav) else { return };
    let cells = nav.graph().cells();
    for (body, stance, route, intent, team, color) in &bots {
        let color = Swatch::of_body(team.copied(), color.copied()).base;
        let lift = Vec3::Y * ROUTE_LIFT;
        let points = std::iter::once(feet_of(body, *stance)).chain(route.ahead().iter().map(|link| cells[link.to as usize].feet)).map(|feet| feet + lift);
        gizmos.linestrip(points, color);
        let goal = intent.goal.and_then(|goal| match goal {
            Goal::Place(place) => Some(place),
            Goal::Pickup(pickup) => nav.graph().fields().pickups.get(pickup as usize).map(|pickup| pickup.reach.center - lift),
        });
        if let Some(goal) = goal {
            gizmos.circle(Isometry3d::new(goal + lift, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)), ARRIVED, color);
        }
    }
}

/// A seat [`fill_empty_slots`] stood up, as against one seated by hand: only these come and go
/// with the slots.
#[derive(Component, Default, Debug)]
pub struct FillsASlot;

/// Keeps the slots nobody has taken filled with bots, or empty of them, as people come and go and
/// the settings change. Bots seated by hand count against the slots and are left alone.
///
/// A filler never keeps anybody out: the server's cap counts people only, and somebody joining
/// takes a filler's seat and body away in the same frame, before
/// `give_bodies_to_whoever_needs_one` stands theirs up.
#[allow(clippy::type_complexity)]
pub(crate) fn fill_empty_slots(
    mut commands: Commands,
    settings: Res<RoundSettings>,
    clients: Query<(), (With<ClientOf>, With<Connected>)>,
    seats: Query<(Entity, Has<FillsASlot>, Option<&Identity>), With<BotSeat>>,
    identities: Query<&Identity>,
    us: Option<Res<OurIdentity>>,
    bodies: Query<(Entity, &BotOf)>,
) {
    // The host is always playing: the authority is a solo game or a listen server.
    let people = 1 + clients.iter().count();
    let by_hand = seats.iter().filter(|(_, fills, _)| !fills).count();
    let mut fillers: Vec<(Entity, Option<Team>)> = seats.iter().filter(|(_, fills, _)| *fills).map(|(seat, _, who)| (seat, who.and_then(Identity::team))).collect();
    let wanted = settings.bots_to_fill(people, by_hand);
    for _ in fillers.len()..wanted {
        commands.spawn((BotSeat, FillsASlot, Name::new("Bot seat")));
    }
    // From the team furthest ahead, so the seats the balance rule filled stay balanced; a filler
    // not yet on a team first, and the newest among equals.
    let mut players: HashMap<Team, usize> = HashMap::new();
    for team in identities.iter().chain(us.as_deref().map(|us| &us.0)).filter_map(Identity::team) {
        *players.entry(team).or_default() += 1;
    }
    let mut retired = Vec::new();
    while fillers.len() > wanted {
        let ahead = |team: Option<Team>| team.map_or(usize::MAX, |team| players.get(&team).copied().unwrap_or(0));
        let index = (0..fillers.len()).max_by_key(|index| (ahead(fillers[*index].1), fillers[*index].0)).expect("more than wanted");
        let (seat, team) = fillers.swap_remove(index);
        if let Some(team) = team {
            *players.entry(team).or_default() -= 1;
        }
        retired.push(seat);
    }
    for seat in &retired {
        commands.entity(*seat).despawn();
        for (body, _) in bodies.iter().filter(|(_, of)| of.0 == *seat) {
            commands.entity(body).despawn();
        }
    }
}

/// A debug key until bots can be added from a menu: `B` seats a bot, `Shift+B` retires the newest.
fn seat_bots_from_the_keyboard(mut commands: Commands, keys: Res<ButtonInput<KeyCode>>, seats: Query<Entity, With<BotSeat>>) {
    if !keys.just_pressed(KeyCode::KeyB) {
        return;
    }
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
        if let Some(newest) = seats.iter().max() {
            commands.entity(newest).despawn();
        }
    } else {
        commands.spawn((BotSeat, Name::new("Bot seat")));
    }
}

pub struct BotPlugin;

impl Plugin for BotPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoundSettings>()
            .init_resource::<ShowBots>()
            .add_systems(Update, (toggle_bot_routes, draw_bot_routes).chain().run_if(in_state(AppMode::Play)))
            .add_systems(FixedPreUpdate, steer_bots.run_if(has_authority).run_if(in_state(AppMode::Play)))
            .add_systems(FixedUpdate, retire_bots.before(step_player).run_if(has_authority))
            .add_systems(Update, (seat_bots_from_the_keyboard, fill_empty_slots).chain().run_if(has_authority).run_if(in_state(AppMode::Play)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::movement_rules::MovementRules;
    use crate::common::world_surface::Volume;
    use crate::game::collision::CollisionWorld;
    use crate::game::nav;
    use crate::game::player::{step, Mover};
    use crate::game::condition::Condition;
    use crate::tool::room::Room;

    /// Breaks if aim and movement disagree about which way is forward: facing a point and walking
    /// forward must close on it.
    #[test]
    fn walking_forward_while_aimed_at_a_point_closes_on_it() {
        for target in [Vec3::new(5.0, 0.0, 0.0), Vec3::new(-3.0, 0.0, 4.0), Vec3::new(0.0, 0.0, -6.0)] {
            let (yaw, _) = aim_at(Vec3::ZERO, target);
            let forward = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
            assert!(forward.dot(target.normalize()) > 0.999, "aimed at {target}, facing {forward}");
            assert!((movement_toward(target.xz().normalize(), yaw) - Vec2::Y).length() < 1e-4);
        }
    }

    /// Breaks if an intent does not reach the body's `Inputs`, or a body outlives its seat.
    #[test]
    fn an_intent_is_written_into_the_inputs_and_a_seat_takes_its_body_with_it() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<Time>();
        let seat = world.spawn(BotSeat).id();
        let target = Vec3::new(4.0, 1.8, -4.0);
        let body = world
            .spawn((
                BotOf(seat),
                Player::default(),
                PhysicsBody::at(Vec3::Y),
                Stance::Standing,
                BotIntent { look_at: Some(target), fire: true, ..default() },
                BotRoute::default(),
                Inputs::default(),
            ))
            .id();
        world.run_system_once(steer_bots).unwrap();
        let input = world.get::<Inputs>(body).unwrap().0;
        let eye = Vec3::Y + Vec3::Y * Player::default().duck.eye_from_center(Stance::Standing);
        assert!((input.yaw - aim_at(eye, target).0).abs() < 1e-5, "the bot does not face what it looks at");
        assert!(input.attack_held, "the bot's trigger is not held");
        assert_eq!(input.movement, Vec2::ZERO, "a bot with no goal walks");

        world.despawn(seat);
        world.run_system_once(retire_bots).unwrap();
        world.flush();
        assert!(world.get_entity(body).is_err(), "the body outlived its seat");
    }

    /// Breaks if the slots are not kept full of bots, a bot seated by hand is not counted, or
    /// turning filling off leaves the fillers in.
    #[test]
    fn empty_slots_fill_with_bots_and_empty_again() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(RoundSettings { slots: 4, fill_with_bots: true });
        let run = |world: &mut World| {
            world.run_system_once(fill_empty_slots).unwrap();
            world.flush();
        };
        let count = |world: &mut World| (world.query_filtered::<(), (With<BotSeat>, With<FillsASlot>)>().iter(world).count(), world.query_filtered::<(), With<BotSeat>>().iter(world).count());

        run(&mut world);
        assert_eq!(count(&mut world), (3, 3), "the host and three bots fill four slots");
        run(&mut world);
        assert_eq!(count(&mut world), (3, 3), "filling again seated more bots");

        world.spawn(BotSeat);
        run(&mut world);
        assert_eq!(count(&mut world), (2, 3), "a bot seated by hand was not counted against the slots");

        // Somebody joining takes a filler's place at once, body and all.
        let filler = world.query_filtered::<Entity, With<FillsASlot>>().iter(&world).max().unwrap();
        let body = world.spawn(BotOf(filler)).id();
        world.spawn((ClientOf, lightyear::prelude::RemoteId(lightyear::prelude::PeerId::Entity(1)), Connected));
        run(&mut world);
        assert_eq!(count(&mut world), (1, 2), "a filler kept its slot when somebody joined");
        assert!(world.get_entity(body).is_err(), "the filler's body outlived its seat by a frame");

        world.resource_mut::<RoundSettings>().fill_with_bots = false;
        run(&mut world);
        assert_eq!(count(&mut world), (0, 1), "the fillers stayed with filling off, or the hand-seated bot went");
    }

    /// Breaks if a route ignores what its cells are like. Along a 40 m lip over a kill floor, plain
    /// time keeps to the lip; weighing danger walks in past the push's reach, about 7 m, and back.
    #[test]
    fn weighing_danger_keeps_a_route_off_the_lip_of_a_pit() {
        use crate::common::trigger::{TriggerEffect, TriggerVolume};
        use crate::game::nav_field;
        let volumes = [
            Volume::air(Room::new(Vec3::new(0.0, 0.0, -4.0), Vec3::new(40.0, 4.0, 8.0))),
            Volume::air(Room::new(Vec3::new(0.0, -6.0, 8.0), Vec3::new(40.0, 4.0, 12.0))),
        ];
        let kill = TriggerVolume { min: Vec3::new(0.0, -6.0, 8.0), max: Vec3::new(40.0, -5.0, 12.0), effect: TriggerEffect::Kill };
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(&volumes);
        let rules = MovementRules::default();
        let mut graph = nav::graph(&world, &faces, &[kill], &rules, &[]);
        graph.set_fields(nav_field::bake(&world, &graph, &[kill], &rules, Vec::new(), Vec::new()));

        let (from, to) = (graph.cell_at(Vec3::new(0.75, 0.0, 7.25)).unwrap(), graph.cell_at(Vec3::new(39.25, 0.0, 7.25)).unwrap());
        let inmost = |costs: RouteCosts| {
            let path = graph.path(from, to, |cell, link| costs.extra(&graph, cell, link)).unwrap();
            path.iter().map(|link| graph.cells()[link.to as usize].feet.z).fold(f32::INFINITY, f32::min)
        };
        let careless = RouteCosts { exposure: 0.0, danger: 0.0, hurt: 0.0, ..default() };
        assert!(inmost(careless) > 6.5, "with no weights the route still left the lip, to {}", inmost(careless));
        assert!(inmost(RouteCosts::default()) < 1.0, "weighing danger, the route came no further in than {}", inmost(RouteCosts::default()));

        // Misjudging, some bots take the lip for safe and some take the safe ground for dangerous.
        let routes: Vec<f32> = (0..24).map(|beliefs| inmost(RouteCosts { misjudgment: 1.0, beliefs, ..default() })).collect();
        assert!(routes.iter().any(|z| *z > 3.0), "no misjudging bot kept near the lip: {routes:?}");
        assert!(routes.iter().any(|z| *z < 1.0), "every misjudging bot kept near the lip: {routes:?}");
    }

    /// Breaks if a mistake is not the same across a stretch and between plans, or a bot that
    /// misjudges nothing is wrong anyway.
    #[test]
    fn a_misjudgment_holds_across_a_stretch_and_only_with_misjudgment() {
        let wrong = RouteCosts { misjudgment: 1.0, beliefs: 7, ..default() };
        let at = Vec3::new(1.0, 0.0, 1.0);
        assert_eq!(wrong.belief(at), wrong.belief(at + Vec3::new(2.5, 0.0, 2.5)), "one stretch, two beliefs");
        let beliefs: Vec<f32> = (0..64).map(|x| wrong.belief(Vec3::new(x as f32 * MISJUDGED_STRETCH, 0.0, 0.0))).collect();
        assert!(beliefs.iter().all(|belief| (-1.0..=1.0).contains(belief)));
        assert!(beliefs.iter().any(|belief| *belief > 0.5) && beliefs.iter().any(|belief| *belief < -0.5), "beliefs do not spread both ways: {beliefs:?}");

        let volumes = [Volume::air(Room::new(Vec3::ZERO, Vec3::new(8.0, 3.0, 8.0)))];
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(&volumes);
        let graph = nav::graph(&world, &faces, &[], &MovementRules::default(), &[]);
        let sound = RouteCosts { beliefs: 7, ..default() };
        for cell in 0..graph.cells().len() as CellId {
            for link in graph.links(cell) {
                assert_eq!(sound.extra(&graph, link.to, link), RouteCosts::default().extra(&graph, link.to, link), "a bot with no misjudgment was wrong");
                assert!(wrong.extra(&graph, link.to, link) >= 0.0);
            }
        }
    }

    /// Breaks if a bot cannot take a pickup only a rocket jump reaches: holding a loaded launcher,
    /// it stops at the jump's start, waits for the launcher, and is thrown onto the platform by
    /// its own blast, through the real step and explosion rules. Without one, it has no route.
    #[test]
    fn a_bot_rocket_jumps_to_a_pickup_on_an_island() {
        use crate::common::projectile::ProjectileSpec;
        use crate::common::trigger::{TriggerEffect, TriggerVolume};
        use crate::game::nav::{Launcher, Leaping};
        use crate::game::nav_field::{self, Pickup};
        let volumes = [
            Volume::air(Room::new(Vec3::new(-10.0, -10.0, -10.0), Vec3::new(10.0, 6.0, 10.0))),
            Volume::solid(Room::new(Vec3::new(-10.0, -10.0, -10.0), Vec3::new(10.0, 0.0, -7.7))),
            Volume::solid(Room::new(Vec3::new(-2.0, -10.0, -2.0), Vec3::new(2.0, 0.0, 2.0))),
        ];
        let kill = TriggerVolume { min: Vec3::new(-10.0, -10.0, -10.0), max: Vec3::new(10.0, -9.0, 10.0), effect: TriggerEffect::Kill };
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(&volumes);
        let rules = MovementRules::default();
        let launcher = Launcher { weapon: WeaponId::of("rocket_launcher"), rocket: ProjectileSpec::ROCKET };
        let prize = Pickup::weapon("bfg".into(), Vec3::new(0.25, 0.0, 0.25));
        let start = Vec3::new(0.25, 0.0, -9.25);
        let leaping = Leaping { triggers: &[kill], reaches: &[prize.reach], launcher: Some(launcher) };
        let mut graph = nav::graph_leaping(&world, &faces, &leaping, &rules, &[start]);
        graph.set_fields(nav_field::bake(&world, &graph, &[kill], &rules, vec![prize.clone()], Vec::new()));

        let unarmed = Gear::default();
        let mut route = BotRoute::default();
        plan(&mut route, &graph, 0, start, Some(Goal::Pickup(0)), &RouteCosts::default(), &unarmed);
        assert!(route.ahead().is_empty(), "a bot with no launcher was routed by a rocket jump");

        let loaded = [launcher.weapon];
        // Hurt enough that the jump would kill: careful, no route; shortsighted every time, the jump.
        let hurt = Gear { health: 20, self_damage: 1.0, loaded: &loaded, ..default() };
        let mut route = BotRoute::default();
        plan(&mut route, &graph, 0, start, Some(Goal::Pickup(0)), &RouteCosts::default(), &hurt);
        assert!(route.ahead().is_empty(), "a bot the jump would kill was routed by it");
        let mut route = BotRoute::default();
        plan(&mut route, &graph, 0, start, Some(Goal::Pickup(0)), &RouteCosts { shortsightedness: 1.0, ..default() }, &hurt);
        assert!(!route.ahead().is_empty(), "a bot that forgot to look did not take the jump");

        let mut mover = Mover { player: Player { ground: Some(Vec3::Y), ..default() }, body: PhysicsBody::at(crate::common::class::body_center_from_feet(start)), stance: Stance::Standing };
        let mut route = BotRoute::default();
        let mut held = None;
        // The shot, as `pull_trigger`, `fire_projectiles`, `step_projectiles` and `explode` take
        // it, after the move it was fired on; that agrees with a round is
        // `a_baked_rocket_jump_lands_in_a_round_where_the_bake_said`.
        let mut shot: Option<nav::Shot> = None;
        let mut kick = Vec3::ZERO;
        let dt = nav::TICK;
        for _ in 0..(12.0 / dt) as u32 {
            let feet = feet_of(&mover.body, mover.stance);
            if prize.reach.touches(mover.body.current, mover.stance.half_extents()) {
                return;
            }
            let upright = mover.stance == Stance::Standing && mover.player.duck.amount == 0.0;
            let gear = Gear { speed: mover.player.velocity.xz().length(), held, ready: held.is_some(), upright, health: 150, armor: 0, self_damage: 1.0, loaded: &loaded };
            plan(&mut route, &graph, 0, feet, Some(Goal::Pickup(0)), &RouteCosts::default(), &gear);
            let input = follow(&mut route, &graph, feet, &gear, dt).unwrap_or_default();
            held = input.select.or(held);
            step(&mut mover, std::mem::take(&mut kick), &input, &Condition::default(), None, &world, &rules, dt);
            if input.attack && held == Some(launcher.weapon) {
                shot = Some(nav::Shot::fire(launcher, &mover));
            }
            if let Some(blast) = shot.as_mut().and_then(|shot| shot.tick(&world, rules.fall(), &mover)) {
                kick = blast.unwrap_or_default().1;
                shot = None;
            }
        }
        panic!("the bot never took the pickup; it is at {}", feet_of(&mover.body, mover.stance));
    }

    /// Breaks if a brain's weapon choice beats the route's: at a rocket jump's start the bot asks
    /// for the launcher whatever the intent selects, or it waits there for a launcher it never holds.
    #[test]
    fn a_rocket_jumps_launcher_beats_the_intents_weapon() {
        use bevy::ecs::system::RunSystemOnce;
        use crate::common::projectile::ProjectileSpec;
        use crate::game::nav::{Launcher, Leaping};
        let volumes = [
            Volume::air(Room::new(Vec3::new(-10.0, -10.0, -10.0), Vec3::new(10.0, 6.0, 10.0))),
            Volume::solid(Room::new(Vec3::new(-10.0, -10.0, -10.0), Vec3::new(10.0, 0.0, -7.7))),
            Volume::solid(Room::new(Vec3::new(-2.0, -10.0, -2.0), Vec3::new(2.0, 0.0, 2.0))),
        ];
        let mut world_collision = CollisionWorld::default();
        let faces = world_collision.rebuild_volumes(&volumes);
        let launcher = Launcher { weapon: WeaponId::of("rocket_launcher"), rocket: ProjectileSpec::ROCKET };
        let leaping = Leaping { launcher: Some(launcher), ..default() };
        let graph = nav::graph_leaping(&world_collision, &faces, &leaping, &MovementRules::default(), &[Vec3::new(0.25, 0.0, -9.25)]);
        let island = graph.cells().iter().find(|cell| cell.feet.xz().length() < 1.0).unwrap().feet;
        // Where the bot's own route sets off by rocket, so the launcher is the next thing it needs.
        let loaded = [launcher.weapon];
        let gear = Gear { health: 150, self_damage: 1.0, loaded: &loaded, ..default() };
        let mut route = BotRoute::default();
        plan(&mut route, &graph, 0, Vec3::new(0.25, 0.0, -9.25), Some(Goal::Place(island)), &RouteCosts::default(), &gear);
        let jump = route.links.iter().position(|link| link.kind == LinkKind::RocketJump).expect("the route to the island has no rocket jump");
        let start = match jump.checked_sub(1) {
            Some(before) => graph.cells()[route.links[before].to as usize].feet,
            None => graph.cells()[route.from.unwrap() as usize].feet,
        };

        let mut world = World::new();
        world.insert_resource(NavBake::holding(graph));
        world.init_resource::<Time>();
        let other = WeaponId::of("shotgun");
        let mut equipped = Equipped::carrying([launcher.weapon, other]);
        equipped.select(other);
        let mut ammo = Ammo::default();
        ammo.at_mut(launcher.weapon).clip = 4;
        let bot = world
            .spawn((
                BotOf(Entity::PLACEHOLDER),
                Player { ground: Some(Vec3::Y), ..default() },
                PhysicsBody::at(crate::common::class::body_center_from_feet(start)),
                Stance::Standing,
                BotIntent { goal: Some(Goal::Place(island)), select: Some(other), ..default() },
                BotRoute::default(),
                Inputs::default(),
                (equipped, ammo, Trigger::default()),
            ))
            .id();
        world.run_system_once(steer_bots).unwrap();
        assert_eq!(world.get::<Inputs>(bot).unwrap().0.select, Some(launcher.weapon), "the intent's weapon beat the rocket jump's launcher");
    }

    /// Breaks if a bot walks into a vent standing: it must duck ahead of the crouch links and come
    /// out the far side, driving `step` with nothing but what [`follow`] says.
    #[test]
    fn a_bot_crouches_through_a_vent() {
        let volumes = [
            Volume::air(Room::new(Vec3::ZERO, Vec3::new(6.0, 3.0, 4.0))),
            Volume::air(Room::new(Vec3::new(6.0, 0.0, 1.0), Vec3::new(12.0, 1.7, 3.0))),
            Volume::air(Room::new(Vec3::new(12.0, 0.0, 0.0), Vec3::new(18.0, 3.0, 4.0))),
        ];
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(&volumes);
        let rules = MovementRules::default();
        let start = Vec3::new(2.25, 0.0, 2.25);
        let graph = nav::graph(&world, &faces, &[], &rules, &[start]);
        let goal = Vec3::new(16.25, 0.0, 2.25);
        let arrived = graph.cells()[graph.cell_at(goal).unwrap() as usize].feet;

        let mut mover = Mover { player: Player { ground: Some(Vec3::Y), ..default() }, body: PhysicsBody::at(crate::common::class::body_center_from_feet(start)), stance: Stance::Standing };
        let mut route = BotRoute::default();
        let mut crouched = false;
        let dt = nav::TICK;
        for _ in 0..(20.0 / dt) as usize {
            let feet = feet_of(&mover.body, mover.stance);
            if reached(feet, arrived) {
                assert!(crouched, "the bot came through the vent without ever crouching");
                return;
            }
            plan(&mut route, &graph, 0, feet, Some(Goal::Place(goal)), &RouteCosts::default(), &Gear::default());
            let input = follow(&mut route, &graph, feet, &Gear::default(), dt).unwrap_or_default();
            crouched |= mover.stance == Stance::Crouched;
            step(&mut mover, Vec3::ZERO, &input, &Condition::default(), None, &world, &rules, dt);
        }
        panic!("the bot never came out of the vent; it is at {}", feet_of(&mover.body, mover.stance));
    }

    /// Breaks if a filler is retired by age rather than by team: with Red a player ahead, the
    /// filler that goes when somebody joins is a Red one, however old.
    #[test]
    fn a_joiner_retires_a_filler_from_the_team_furthest_ahead() {
        use crate::common::damage::PlayerId;
        use crate::game::respawn::Seat;
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(RoundSettings { slots: 4, fill_with_bots: true });
        let on = |id, team| Identity { id: PlayerId(id), seat: Seat::Playing(Some(team)), color: 0 };
        world.insert_resource(OurIdentity(on(1, Team::Blue)));
        world.spawn((BotSeat, FillsASlot, on(2, Team::Red)));
        world.spawn((BotSeat, FillsASlot, on(3, Team::Red)));
        let blue = world.spawn((BotSeat, FillsASlot, on(4, Team::Blue))).id();
        world.spawn((ClientOf, lightyear::prelude::RemoteId(lightyear::prelude::PeerId::Entity(1)), Connected));
        world.run_system_once(fill_empty_slots).unwrap();
        world.flush();
        let reds = world.query_filtered::<&Identity, With<BotSeat>>().iter(&world).filter(|who| who.team() == Some(Team::Red)).count();
        assert_eq!(reds, 1, "a Blue filler went while Red was ahead");
        assert!(world.get_entity(blue).is_ok());
    }

    /// Breaks if a bot cannot follow a route the graph gives it: across a room, up a ledge by its
    /// jump, it must arrive by driving `step` with nothing but what [`follow`] says.
    #[test]
    fn a_bot_follows_its_route_up_a_ledge_by_its_jump() {
        let volumes = [Volume::air(Room::new(Vec3::ZERO, Vec3::new(10.0, 4.0, 4.0))), Volume::air(Room::new(Vec3::new(10.0, 1.0, 0.0), Vec3::new(16.0, 5.0, 4.0)))];
        let mut world = CollisionWorld::default();
        let faces = world.rebuild_volumes(&volumes);
        let rules = MovementRules::default();
        let start = Vec3::new(2.25, 0.0, 2.25);
        let graph = nav::graph(&world, &faces, &[], &rules, &[start]);
        let goal = Vec3::new(14.25, 1.0, 2.25);
        assert!(graph.cell_at(goal).is_some(), "no cell on the ledge");

        let up = graph.path(graph.cell_at(start).unwrap(), graph.cell_at(goal).unwrap(), |_, _| 0.0).unwrap();
        assert!(up.iter().any(|link| link.kind == LinkKind::Jump), "the route up the ledge has no jump in it");

        let mut mover = Mover { player: Player { ground: Some(Vec3::Y), ..default() }, body: PhysicsBody::at(crate::common::class::body_center_from_feet(start)), stance: Stance::Standing };
        let mut route = BotRoute::default();
        let dt = nav::TICK;
        for _ in 0..(10.0 / dt) as usize {
            let feet = feet_of(&mover.body, mover.stance);
            if reached(feet, graph.cells()[graph.cell_at(goal).unwrap() as usize].feet) {
                return;
            }
            plan(&mut route, &graph, 0, feet, Some(Goal::Place(goal)), &RouteCosts::default(), &Gear::default());
            let input = follow(&mut route, &graph, feet, &Gear::default(), dt).unwrap_or_default();
            step(&mut mover, Vec3::ZERO, &input, &Condition::default(), None, &world, &rules, dt);
        }
        panic!("the bot never reached the ledge; it is at {}", feet_of(&mover.body, mover.stance));
    }
}
