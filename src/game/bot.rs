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
use crate::common::weapon::WeaponId;
use crate::common::player_color::PlayerColor;
use crate::common::swatch::Swatch;
use crate::common::team::Team;
use crate::game::nav::{CellId, Link, LinkKind, NavGraph};
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

/// A bot's place in the match, alive or not. Holds the
/// [`Identity`](crate::game::respawn::Identity) once it has been given one,
/// so a respawn is the same bot on the same team. Despawning it retires the bot.
#[derive(Component, Default, Debug)]
pub struct BotSeat;

/// On a bot's body: the seat it is driven for.
#[derive(Component, Clone, Copy, Debug)]
pub struct BotOf(pub Entity);

/// What a bot wants this tick, in the world's terms rather than the keyboard's.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct BotIntent {
    /// Where to stand, as feet. Walked to along the nav graph.
    pub goal: Option<Vec3>,
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
    /// How wrong the bot can be about a stretch of floor, 0 to 1. At 1 a stretch may read as safe
    /// however dangerous it is, or as twice as risky, and dear even where nothing threatens.
    pub misjudgment: f32,
    /// Which stretches it is wrong about, and which way. Rolled again each life.
    pub beliefs: u64,
}

impl Default for RouteCosts {
    fn default() -> Self {
        RouteCosts { exposure: 0.5, danger: 4.0, hurt: 2.0, misjudgment: 0.0, beliefs: 0 }
    }
}

/// The range each weight of a [`RouteCosts::temperament`] is scaled by: some bots barely mind.
const TEMPERAMENT: std::ops::Range<f32> = 0.1..1.5;
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
        (risk * (1.0 + wrong) + link.seconds * PHANTOM_RISK * wrong.max(0.0)).max(0.0)
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
    planned: Option<(Vec3, u64)>,
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

/// Plans `route` to `goal` from `feet` if what it was planned for has changed.
fn plan(route: &mut BotRoute, graph: &NavGraph, generation: u64, feet: Vec3, goal: Option<Vec3>, costs: &RouteCosts) {
    let Some(goal) = goal else {
        route.forget();
        return;
    };
    if route.planned == Some((goal, generation)) {
        return;
    }
    route.forget();
    route.planned = Some((goal, generation));
    let (Some(from), Some(to)) = (graph.cell_at(feet), graph.cell_at(goal)) else { return };
    route.from = Some(from);
    route.links = graph.path(from, to, |cell, link| costs.extra(graph, cell, link)).unwrap_or_default();
    route.closest = f32::INFINITY;
}

/// One tick of following `route`: the movement and facing it calls for, and whether to jump.
/// A drop or a jump is replayed from the input the bake found it with, once the bot is on the
/// cell it starts from.
fn follow(route: &mut BotRoute, graph: &NavGraph, feet: Vec3, dt: f32) -> Option<PlayerInput> {
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
    if matches!(link.kind, LinkKind::Drop | LinkKind::Jump) && link.leap.is_some() && reached(feet, start) {
        route.leaping = Some(0);
        return follow(route, graph, feet, dt);
    }
    if reached(feet, target) {
        route.next += 1;
        route.closest = f32::INFINITY;
        return follow(route, graph, feet, dt);
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
    Some(PlayerInput { movement: Vec2::Y, yaw, ..default() })
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
    mut bots: Query<(&PhysicsBody, &Stance, &Player, &BotIntent, Option<&RouteCosts>, &mut BotRoute, &mut Inputs), With<BotOf>>,
) {
    let dt = time.delta_secs();
    for (body, stance, player, intent, costs, mut route, mut inputs) in &mut bots {
        let feet = feet_of(body, *stance);
        let walking = nav.as_deref().and_then(|nav| {
            plan(&mut route, nav.graph(), nav.generation(), feet, intent.goal, &costs.copied().unwrap_or_default());
            follow(&mut route, nav.graph(), feet, dt)
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
        input.sprint |= intent.sprint;
        input.crouch = intent.crouch && !leaping;
        input.attack = intent.fire;
        input.attack_held = intent.fire;
        input.alt_attack = intent.alt_fire;
        input.alt_attack_held = intent.alt_fire;
        input.reload = intent.reload;
        input.select = intent.select;
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

/// Whether each bot's route is drawn over the floor. `F8` toggles it; off by default, a debug view.
#[derive(Resource, Default, Debug)]
pub struct ShowBotRoutes(pub bool);

fn toggle_bot_routes(keys: Res<ButtonInput<KeyCode>>, mut show: ResMut<ShowBotRoutes>) {
    if keys.just_pressed(KeyCode::F8) {
        show.0 = !show.0;
    }
}

/// Meters above the floor a route is drawn, so it does not fight the floor for depth.
const ROUTE_LIFT: f32 = 0.1;

/// From each bot's feet through the cells it has yet to reach, to a ring at its goal, in the
/// bot's own color. Routes live on the authority, so a client draws none.
fn draw_bot_routes(
    show: Res<ShowBotRoutes>,
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
        if let Some(goal) = intent.goal {
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
pub(crate) fn fill_empty_slots(
    mut commands: Commands,
    settings: Res<RoundSettings>,
    clients: Query<(), (With<ClientOf>, With<Connected>)>,
    seats: Query<(Entity, Has<FillsASlot>), With<BotSeat>>,
    bodies: Query<(Entity, &BotOf)>,
) {
    // The host is always playing: the authority is a solo game or a listen server.
    let people = 1 + clients.iter().count();
    let by_hand = seats.iter().filter(|(_, fills)| !fills).count();
    let mut fillers: Vec<Entity> = seats.iter().filter(|(_, fills)| *fills).map(|(seat, _)| seat).collect();
    let wanted = settings.bots_to_fill(people, by_hand);
    fillers.sort();
    for _ in fillers.len()..wanted {
        commands.spawn((BotSeat, FillsASlot, Name::new("Bot seat")));
    }
    for seat in fillers.iter().skip(wanted) {
        commands.entity(*seat).despawn();
        for (body, _) in bodies.iter().filter(|(_, of)| of.0 == *seat) {
            commands.entity(body).despawn();
        }
    }
}

/// A placeholder until there is a brain: wanders from one random cell to another.
#[derive(Component, Default, Debug)]
pub struct Roam;

fn roam(nav: Option<Res<NavBake>>, mut bots: Query<(&BotRoute, &mut BotIntent), With<Roam>>) {
    let Some(nav) = nav else { return };
    let cells = nav.graph().cells();
    if cells.is_empty() {
        return;
    }
    for (route, mut intent) in &mut bots {
        if intent.goal.is_none() || route.is_done() {
            intent.goal = Some(cells[rand::random_range(0..cells.len())].feet);
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
            .init_resource::<ShowBotRoutes>()
            .add_systems(Update, (toggle_bot_routes, draw_bot_routes).chain().run_if(in_state(AppMode::Play)))
            .add_systems(FixedPreUpdate, steer_bots.run_if(has_authority).run_if(in_state(AppMode::Play)))
            .add_systems(FixedUpdate, retire_bots.before(step_player).run_if(has_authority))
            .add_systems(Update, (seat_bots_from_the_keyboard, fill_empty_slots, roam).chain().run_if(has_authority).run_if(in_state(AppMode::Play)));
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
            plan(&mut route, &graph, 0, feet, Some(goal), &RouteCosts::default());
            let input = follow(&mut route, &graph, feet, dt).unwrap_or_default();
            step(&mut mover, Vec3::ZERO, &input, &Condition::default(), None, &world, &rules, dt);
        }
        panic!("the bot never reached the ledge; it is at {}", feet_of(&mover.body, mover.stance));
    }
}
