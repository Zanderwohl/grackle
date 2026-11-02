//! What a bot wants. Each want is scored from its own state and what its [`BotSenses`] report,
//! the highest wins, and the one it already has gets a commitment bonus a rival must beat, so it
//! does not flip between two at a threshold. The one writer of [`BotIntent::goal`]. See "Think" in
//! documentation/bot-brain.md.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::round_settings::{Better, BotSkill};
use crate::common::damage::{Armor, Damageable, PlayerId};
use crate::common::movement_rules::MovementRules;
use crate::common::net::has_authority;
use crate::common::weapon::{Ammo, Equipped, WeaponCatalog, WeaponId};
use crate::game::bot::{BotIntent, BotOf, BotRoute, Goal};
use crate::game::bot_sense::{sense, BotMind, BotSenses, Sense, REMEMBERS, REMEMBERS_NOISES};
use crate::game::nav::{CellId, NavGraph};
use crate::game::nav_rating::Rating;
use crate::game::player::{running_speed, PhysicsBody};
use crate::get;
use crate::game::world_prop::{GivesWeapon, Pickup, WorldProp};
use crate::prop::archetype::PickupKind;
use crate::prop::baked::PropCache;
use crate::tool::nav_bake::NavBake;

/// Seconds between thoughts when nothing happens.
const THINK_SECONDS: f32 = 0.25;
/// Seconds away at which a pickup, a hiding place or an ambush is worth half as much.
const TRAVEL_HALF: f32 = 5.0;
/// Added to the want a bot already has, before its temperament's stubbornness.
const COMMITMENT: f32 = 0.15;
/// Seconds a goal that could not be reached, or an ambush already held, is left alone.
const UNREACHABLE_REST: f32 = 3.0;
/// Seconds a bot whose route came back lost waits before thinking again. Every lost route is a
/// search of the whole graph, and a bot stranded where nothing leads out would search every tick.
const LOST_RETRY: f32 = 1.0;
/// Meters on a side of the squares cells are filed in, to find the ones near a bot.
const NEAR_SQUARE: f32 = 4.0;
const AMBUSH_REST: f32 = 30.0;
/// Seconds after being hurt that a bot still feels under fire.
const UNDER_FIRE: f32 = 2.0;
/// Meters around a bot that it looks for somewhere less seen, and the square meters of exposure
/// less than where it stands that make one worth going to.
const COVER_RADIUS: f32 = 10.0;
const COVER_GAIN: f32 = 20.0;
/// Meters within which two places are the same.
const SAME_PLACE: f32 = 1.0;
/// A heard noise is worth this much of a seen enemy to go and look at.
const NOISE_WORTH: f32 = 0.5;
const ROAM_SCORE: f32 = 0.1;
/// Random cells tried for somewhere to roam that is not one just given up on.
const ROAM_ROLLS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Want {
    Health,
    Armor,
    /// A weapon not carried, or ammo for one that is.
    Arms,
    Hunt,
    Cover,
    Ambush,
    Roam,
}

impl Want {
    pub const ALL: [Want; 7] = [Want::Health, Want::Armor, Want::Arms, Want::Hunt, Want::Cover, Want::Ambush, Want::Roam];

    pub fn name(self) -> String {
        match self {
            Want::Health => get!("bot.want.health"),
            Want::Armor => get!("bot.want.armor"),
            Want::Arms => get!("bot.want.arms"),
            Want::Hunt => get!("bot.want.hunt"),
            Want::Cover => get!("bot.want.cover"),
            Want::Ambush => get!("bot.want.ambush"),
            Want::Roam => get!("bot.want.roam"),
        }
    }
}

/// How much each want matters to a bot, rolled per seat inside its [`BotMind`].
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Drives {
    pub health: f32,
    pub armor: f32,
    pub arms: f32,
    pub hunt: f32,
    pub cover: f32,
    pub ambush: f32,
    pub roam: f32,
    /// Times [`COMMITMENT`].
    pub stubbornness: f32,
    /// Seconds an ambush is held with nobody coming.
    pub patience: f32,
}

impl Default for Drives {
    fn default() -> Self {
        Drives { health: 1.0, armor: 0.6, arms: 0.7, hunt: 0.8, cover: 0.9, ambush: 0.3, roam: 1.0, stubbornness: 1.0, patience: 10.0 }
    }
}

/// Each drive scaled by this, rolled once per drive.
const DRIVE_SCALE: std::ops::Range<f32> = 0.5..1.5;
const PATIENCE: std::ops::Range<f32> = 4.0..20.0;

impl Drives {
    pub fn temperament(skill: BotSkill) -> Self {
        let base = Drives::default();
        let roll = |drive: f32| drive * rand::random_range(DRIVE_SCALE);
        let lean = |drive: f32, better| drive * rand::random_range(skill.within(DRIVE_SCALE, better));
        Drives {
            health: roll(base.health),
            armor: roll(base.armor),
            arms: roll(base.arms),
            hunt: roll(base.hunt),
            cover: lean(base.cover, Better::Higher),
            ambush: roll(base.ambush),
            roam: lean(base.roam, Better::Lower),
            stubbornness: roll(base.stubbornness),
            patience: rand::random_range(skill.within(PATIENCE, Better::Higher)),
        }
    }
}

/// A want with somewhere to go for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub want: Want,
    pub goal: Goal,
    pub score: f32,
}

/// What a pickup would do for this bot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Offer {
    Health,
    Armor,
    Ammo,
    /// A weapon, and how much of its ammo is missing if it is already carried.
    Weapon { carried: bool, missing: f32 },
}

/// Everything a bot's choice is made from, as fractions where it can be.
#[derive(Clone, Debug, Default)]
pub struct Situation {
    pub health: f32,
    pub armor: f32,
    /// Ammo missing across what is carried, `0.0` full.
    pub ammo_missing: f32,
    /// Pickups in stock, by their place in the bake, with the seconds to reach each.
    pub pickups: Vec<(u16, Offer, f32)>,
    /// Where enemies are believed to stand, as feet, and how fresh that is, `1.0` this tick.
    pub enemies: Vec<(Vec3, f32)>,
    /// Hurt lately, or an enemy in sight now.
    pub threatened: bool,
    /// The nearby place much less seen than here, as feet.
    pub cover: Option<Vec3>,
    /// An ambush place, as feet, the seconds to it, and how much of its hold is left.
    pub ambush: Option<(Vec3, f32, f32)>,
    pub roam: Option<Vec3>,
}

fn nearness(seconds: f32) -> f32 {
    1.0 / (1.0 + seconds.max(0.0) / TRAVEL_HALF)
}

pub fn candidates(situation: &Situation, drives: &Drives) -> Vec<Candidate> {
    let s = situation;
    let mut out = Vec::new();
    for &(index, offer, seconds) in &s.pickups {
        let (want, need) = match offer {
            Offer::Health => (Want::Health, (1.0 - s.health) * drives.health),
            // Armor once health is seen to, not instead of it.
            Offer::Armor => (Want::Armor, (1.0 - s.armor) * s.health * drives.armor),
            Offer::Ammo => (Want::Arms, s.ammo_missing * drives.arms),
            Offer::Weapon { carried: false, .. } => (Want::Arms, drives.arms),
            Offer::Weapon { carried: true, missing } => (Want::Arms, missing * drives.arms),
        };
        if need > 0.0 {
            out.push(Candidate { want, goal: Goal::Pickup(index), score: need * nearness(seconds) });
        }
    }
    // A bot hunts as boldly as it is healthy and armed.
    let boldness = s.health * (1.0 - 0.5 * s.ammo_missing) * drives.hunt;
    for &(at, freshness) in &s.enemies {
        out.push(Candidate { want: Want::Hunt, goal: Goal::Place(at), score: boldness * freshness });
    }
    if let (Some(at), true) = (s.cover, s.threatened) {
        out.push(Candidate { want: Want::Cover, goal: Goal::Place(at), score: (1.0 - s.health) * drives.cover });
    }
    if let Some((at, seconds, left)) = s.ambush {
        out.push(Candidate { want: Want::Ambush, goal: Goal::Place(at), score: s.health * left * nearness(seconds) * drives.ambush });
    }
    if let Some(at) = s.roam {
        out.push(Candidate { want: Want::Roam, goal: Goal::Place(at), score: ROAM_SCORE * drives.roam });
    }
    out
}

fn same_goal(a: Goal, b: Goal) -> bool {
    match (a, b) {
        (Goal::Pickup(a), Goal::Pickup(b)) => a == b,
        (Goal::Place(a), Goal::Place(b)) => a.distance(b) <= SAME_PLACE,
        _ => false,
    }
}

/// Every candidate, best first, the current want and goal counted with its bonus. A hunt keeps
/// its bonus as the enemy moves, since the place it goes to moves with them.
pub fn rank(candidates: &[Candidate], current: Option<(Want, Goal)>, drives: &Drives) -> Vec<Candidate> {
    let bonus = COMMITMENT * drives.stubbornness;
    let mut ranked: Vec<Candidate> = candidates
        .iter()
        .map(|candidate| {
            let kept = current.is_some_and(|(want, goal)| want == candidate.want && (want == Want::Hunt || same_goal(goal, candidate.goal)));
            Candidate { score: candidate.score + if kept { bonus } else { 0.0 }, ..*candidate }
        })
        .collect();
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score));
    ranked
}

/// The best of `ranked` and the best of a different want.
pub fn choose(ranked: &[Candidate]) -> (Option<Candidate>, Option<Candidate>) {
    let best = ranked.first().copied();
    let runner_up = ranked.iter().find(|other| best.is_some_and(|best| other.want != best.want)).copied();
    (best, runner_up)
}

/// A bot's train of thought: what it wants now and what it was weighed against, for `F8`.
#[derive(Component, Clone, Debug, Default)]
pub struct BotThought {
    pub chosen: Option<Candidate>,
    pub runner_up: Option<Candidate>,
    /// The best score each want had, with the commitment bonus, for the wants that had one.
    pub pulls: Vec<(Want, f32)>,
    /// Seconds until it thinks again with nothing having happened.
    next: f32,
    /// What it last thought from, so news makes it think at once.
    health: u32,
    noticed: Vec<PlayerId>,
    /// Goals it gave up on, and the game time it may want each again.
    rested: Vec<(Goal, f32)>,
    /// The ambush held, and for how many seconds.
    held: Option<(Goal, f32)>,
    roam: Option<Vec3>,
    /// The route was done last tick, so an arrival is news once rather than for as long as it lasts.
    was_done: bool,
}

impl BotThought {
    fn rest(&mut self, goal: Goal, until: f32) {
        self.rested.retain(|(rested, _)| !same_goal(*rested, goal));
        self.rested.push((goal, until));
    }

    fn resting(&self, goal: Goal) -> bool {
        self.rested.iter().any(|(rested, _)| same_goal(*rested, goal))
    }
}

/// A point dropped onto the floor under it: a body's center, or an eye's height.
fn floor_under(graph: &NavGraph, at: Vec3) -> Option<CellId> {
    [Stance::Standing.half_extents().y, 1.6, 0.0, 2.5].into_iter().find_map(|drop| graph.cell_at(at - Vec3::Y * drop))
}

type Standing<'w, 's, 'a> = Query<'w, 's, (&'a WorldProp, &'a Pickup, Option<&'a GivesWeapon>)>;

/// What a pickup the bake knows of is, if it is in stock, matched by place to what stands in the
/// world.
fn live_pickup(baked: &crate::game::nav_field::Pickup, standing: &Standing, cache: Option<&PropCache>) -> Option<Live> {
    let (prop, pickup, gives) =
        standing.iter().find(|(prop, ..)| (prop.at - baked.feet).xz().length() <= SAME_PLACE && (prop.at.y - baked.feet.y).abs() <= 2.0)?;
    if pickup.taken {
        return None;
    }
    Some(match gives {
        Some(gives) => Live::Weapon(gives.weapon),
        None => Live::Kind(cache?.get(&prop.model)?.pickup.kind),
    })
}

#[derive(Clone, Copy, Debug)]
enum Live {
    Kind(PickupKind),
    Weapon(WeaponId),
}

/// Ammo missing from one weapon, as a fraction of a full magazine and reserve.
fn missing(weapon: WeaponId, ammo: &Ammo, catalog: &WeaponCatalog) -> f32 {
    let Some(spec) = catalog.get(weapon) else { return 0.0 };
    let full = spec.magazine.clip.unwrap_or(0) + spec.magazine.reserve;
    if full == 0 {
        return 0.0;
    }
    let have = ammo.at(weapon).map_or(0, |state| state.clip + state.reserve);
    1.0 - (have as f32 / full as f32).min(1.0)
}

/// What `think` keeps about the graph until it changes: the cells the ambush rating holds, and
/// every cell filed by the square it stands in.
#[derive(Default)]
pub struct MapIndex {
    generation: Option<u64>,
    ambushes: Vec<CellId>,
    squares: std::collections::HashMap<IVec2, Vec<CellId>>,
}

fn square_of(at: Vec3) -> IVec2 {
    (at.xz() / NEAR_SQUARE).floor().as_ivec2()
}

impl MapIndex {
    fn keep(&mut self, graph: &NavGraph, generation: u64) {
        if self.generation == Some(generation) {
            return;
        }
        let cells = 0..graph.cells().len() as CellId;
        self.ambushes = cells.clone().filter(|cell| Rating::Ambush.holds(graph, *cell) == Some(true)).collect();
        self.squares.clear();
        for cell in cells {
            self.squares.entry(square_of(graph.cells()[cell as usize].feet)).or_default().push(cell);
        }
        self.generation = Some(generation);
    }

    /// The cells within `radius` of `at`, across the floor.
    fn near<'a>(&'a self, graph: &'a NavGraph, at: Vec3, radius: f32) -> impl Iterator<Item = CellId> + 'a {
        let (lo, hi) = (square_of(at - Vec3::splat(radius)), square_of(at + Vec3::splat(radius)));
        (lo.x..=hi.x)
            .flat_map(move |x| (lo.y..=hi.y).map(move |y| IVec2::new(x, y)))
            .filter_map(|square| self.squares.get(&square))
            .flatten()
            .copied()
            .filter(move |cell| graph.cells()[*cell as usize].feet.distance(at) <= radius)
    }
}

#[allow(clippy::type_complexity)]
pub fn think(
    time: Res<Time>,
    nav: Option<Res<NavBake>>,
    catalog: Option<Res<WeaponCatalog>>,
    cache: Option<Res<PropCache>>,
    rules: Option<Res<MovementRules>>,
    standing: Standing,
    mut index: Local<MapIndex>,
    mut bots: Query<
        (
            (&PhysicsBody, &Stance, &BotSenses, &BotMind, &BotRoute, Option<&PlayerId>),
            (Option<&Damageable>, Option<&Armor>, Option<&Equipped>, Option<&Ammo>),
            (&mut BotIntent, &mut BotThought),
        ),
        With<BotOf>,
    >,
) {
    let Some(nav) = nav else { return };
    let graph = nav.graph();
    if graph.cells().is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    index.keep(graph, nav.generation());
    let baked = &graph.fields().pickups;
    let cache = cache.as_deref();
    // Matched only once some bot thinks this tick.
    let mut live: Option<Vec<Option<Live>>> = None;
    let speed = running_speed(&rules.as_deref().cloned().unwrap_or_default());

    for ((body, stance, senses, mind, route, id), (health, armor, equipped, ammo), (mut intent, mut thought)) in &mut bots {
        let thought = thought.as_mut();
        thought.rested.retain(|(_, until)| *until > now);
        let current = thought.chosen.map(|chosen| (chosen.want, chosen.goal));
        let arrived = route.is_done() && !route.is_lost();
        if let Some((want, goal)) = current {
            if route.is_lost() {
                thought.rest(goal, now + UNREACHABLE_REST);
            } else if arrived && want == Want::Ambush {
                let held = match thought.held {
                    Some((held, seconds)) if same_goal(held, goal) => seconds + dt,
                    _ => dt,
                };
                thought.held = Some((goal, held));
                if held >= mind.drives.patience {
                    thought.rest(goal, now + AMBUSH_REST);
                }
            } else if arrived && (want == Want::Roam || (want == Want::Hunt && !senses.known.iter().any(|known| known.in_sight))) {
                // Looked there and found nothing: not again until the memory is gone. A hunt
                // standing over somebody it can see has found them.
                thought.rest(goal, now + REMEMBERS);
            }
        }

        let hp = health.map_or(u32::MAX, Damageable::health);
        let noticed: Vec<PlayerId> = senses.noticed(now, mind).map(|known| known.who).collect();
        let taken = matches!(intent.goal, Some(Goal::Pickup(pickup)) if baked.get(pickup as usize).and_then(|baked| live_pickup(baked, &standing, cache)).is_none());
        let done_now = route.is_done() && !thought.was_done;
        thought.was_done = route.is_done();
        let news = hp < thought.health || noticed.iter().any(|who| !thought.noticed.contains(who)) || done_now || taken;
        thought.health = hp;
        thought.noticed = noticed;
        thought.next -= dt;
        if !news && thought.next > 0.0 {
            continue;
        }
        thought.next = if route.is_lost() {
            LOST_RETRY
        } else {
            // Spread across ticks so a room of bots does not all think at once.
            THINK_SECONDS * (0.75 + 0.5 * id.map_or(0.5, |id| (id.0 % 7) as f32 / 6.0))
        };
        let live = live.get_or_insert_with(|| baked.iter().map(|baked| live_pickup(baked, &standing, cache)).collect());

        let feet = body.current - Vec3::Y * stance.half_extents().y;
        let here = graph.cell_at(feet);
        let fields = graph.fields();
        let seconds_to = |index: u16| here.and_then(|cell| fields.distances.get(index as usize)?.get(cell as usize).copied()).unwrap_or(f32::INFINITY);

        let carried = equipped.map_or(&[][..], Equipped::carried);
        let catalog = catalog.as_deref();
        let ammo_missing = match (ammo, catalog) {
            (Some(ammo), Some(catalog)) if !carried.is_empty() => {
                carried.iter().map(|weapon| missing(*weapon, ammo, catalog)).sum::<f32>() / carried.len() as f32
            }
            _ => 0.0,
        };

        let pickups = live
            .iter()
            .enumerate()
            .filter_map(|(index, live)| {
                let offer = match (*live)? {
                    Live::Kind(PickupKind::Health) => Offer::Health,
                    Live::Kind(PickupKind::Armor) => Offer::Armor,
                    Live::Kind(PickupKind::Ammo) => Offer::Ammo,
                    Live::Kind(PickupKind::Flag) => return None,
                    Live::Weapon(weapon) => Offer::Weapon {
                        carried: carried.contains(&weapon),
                        missing: match (ammo, catalog) {
                            (Some(ammo), Some(catalog)) => missing(weapon, ammo, catalog),
                            _ => 0.0,
                        },
                    },
                };
                let index = index as u16;
                let seconds = seconds_to(index);
                (seconds.is_finite() && !thought.resting(Goal::Pickup(index))).then_some((index, offer, seconds))
            })
            .collect();

        let place_of = |at: Vec3| floor_under(graph, at).map(|cell| graph.cells()[cell as usize].feet);
        let mut enemies: Vec<(Vec3, f32)> = senses
            .noticed(now, mind)
            .filter_map(|known| Some((place_of(known.at)?, if known.in_sight { 1.0 } else { 1.0 - (now - known.last) / REMEMBERS })))
            .chain(senses.heard(now, mind).filter_map(|noise| Some((place_of(noise.at)?, NOISE_WORTH * (1.0 - (now - noise.when) / REMEMBERS_NOISES)))))
            .filter(|(at, _)| !thought.resting(Goal::Place(*at)))
            .collect();
        enemies.retain(|(_, freshness)| *freshness > 0.0);

        let threatened = senses.known.iter().any(|known| known.in_sight || (known.how == Sense::Hurt && now - known.last <= UNDER_FIRE));
        let exposure = |cell: CellId| graph.field(cell, crate::game::nav_field::Field::Exposure);
        let cover = here.and_then(|here| {
            let mine = exposure(here)?;
            let (best, least) = index
                .near(graph, feet, COVER_RADIUS)
                .filter(|cell| !thought.resting(Goal::Place(graph.cells()[*cell as usize].feet)))
                .filter_map(|cell| Some((cell, exposure(cell)?)))
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            (least + COVER_GAIN <= mine).then(|| graph.cells()[best as usize].feet)
        });

        if current.is_none_or(|(want, _)| want != Want::Ambush) {
            thought.held = None;
        }
        let ambush = index
            .ambushes
            .iter()
            .map(|cell| graph.cells()[*cell as usize].feet)
            .filter(|at| !thought.resting(Goal::Place(*at)))
            .min_by(|a, b| a.distance(feet).total_cmp(&b.distance(feet)))
            .map(|at| {
                let left = match thought.held {
                    Some((held, seconds)) if same_goal(held, Goal::Place(at)) => 1.0 - seconds / mind.drives.patience,
                    _ => 1.0,
                };
                (at, at.distance(feet) / speed, left.max(0.0))
            });

        let roam_done = thought.roam.is_none_or(|at| thought.resting(Goal::Place(at)));
        if roam_done {
            let cells = graph.cells();
            let roll = || cells[rand::random_range(0..cells.len())].feet;
            thought.roam = (0..ROAM_ROLLS).map(|_| roll()).find(|at| !thought.resting(Goal::Place(*at)));
        }

        let situation = Situation {
            health: health.map_or(1.0, |health| health.health() as f32 / health.max().max(1) as f32),
            armor: armor.map_or(1.0, |armor| if armor.max() == 0 { 1.0 } else { armor.points() as f32 / armor.max() as f32 }),
            ammo_missing,
            pickups,
            enemies,
            threatened,
            cover,
            ambush,
            roam: thought.roam,
        };
        let ranked = rank(&candidates(&situation, &mind.drives), current, &mind.drives);
        (thought.chosen, thought.runner_up) = choose(&ranked);
        thought.pulls = Want::ALL.iter().filter_map(|want| Some((*want, ranked.iter().find(|candidate| candidate.want == *want)?.score))).collect();
        let goal = thought.chosen.map(|chosen| chosen.goal);
        // A hunt's place moves a little with every sighting; replanning for each would be a search
        // a tick.
        let moved = match (intent.goal, goal) {
            (Some(old), Some(new)) => !same_goal(old, new),
            (old, new) => old != new,
        };
        if moved {
            intent.goal = goal;
        }
    }
}

pub struct BotThinkPlugin;

impl Plugin for BotThinkPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, think.after(sense).run_if(has_authority).run_if(in_state(AppMode::Play)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calm() -> Situation {
        Situation { health: 1.0, armor: 1.0, roam: Some(Vec3::new(9.0, 0.0, 9.0)), ..default() }
    }

    fn picked(situation: &Situation, current: Option<(Want, Goal)>) -> Candidate {
        choose(&rank(&candidates(situation, &Drives::default()), current, &Drives::default())).0.unwrap()
    }

    /// Breaks if a hurt bot does not go for health, or goes for the far pack over the near one.
    #[test]
    fn a_hurt_bot_goes_for_the_nearer_health() {
        let situation = Situation { health: 0.3, pickups: vec![(0, Offer::Health, 20.0), (1, Offer::Health, 3.0)], ..calm() };
        let chosen = picked(&situation, None);
        assert_eq!((chosen.want, chosen.goal), (Want::Health, Goal::Pickup(1)));
    }

    /// Breaks if the commitment bonus does not hold: a pack barely nearer than the one it is
    /// going for does not turn it round, and one much nearer does.
    #[test]
    fn it_does_not_turn_back_for_a_little_better() {
        let going = Some((Want::Health, Goal::Pickup(0)));
        let barely = Situation { health: 0.5, pickups: vec![(0, Offer::Health, 6.0), (1, Offer::Health, 5.5)], ..calm() };
        assert_eq!(picked(&barely, going).goal, Goal::Pickup(0));
        let much = Situation { health: 0.5, pickups: vec![(0, Offer::Health, 30.0), (1, Offer::Health, 1.0)], ..calm() };
        assert_eq!(picked(&much, going).goal, Goal::Pickup(1));
    }

    /// Breaks if a healthy bot that knows of an enemy does not go after it.
    #[test]
    fn a_bot_that_sees_an_enemy_hunts_it() {
        let at = Vec3::new(4.0, 0.0, -10.0);
        let situation = Situation { enemies: vec![(at, 1.0)], ..calm() };
        let chosen = picked(&situation, None);
        assert_eq!((chosen.want, chosen.goal), (Want::Hunt, Goal::Place(at)));
    }

    /// Breaks if a badly hurt bot under fire stands and fights rather than getting out of sight.
    #[test]
    fn a_badly_hurt_bot_under_fire_takes_cover() {
        let situation = Situation {
            health: 0.2,
            enemies: vec![(Vec3::new(4.0, 0.0, -10.0), 1.0)],
            threatened: true,
            cover: Some(Vec3::new(-3.0, 0.0, 2.0)),
            ..calm()
        };
        assert_eq!(picked(&situation, None).want, Want::Cover);
    }

    /// Breaks if a bot with nothing to want stands still.
    #[test]
    fn with_nothing_wanted_it_roams() {
        assert_eq!(picked(&calm(), None).want, Want::Roam);
    }

    /// Breaks if an ambush held past its patience keeps the bot there.
    #[test]
    fn an_ambush_wears_out() {
        let fresh = Situation { ambush: Some((Vec3::new(2.0, 0.0, 2.0), 1.0, 1.0)), ..calm() };
        assert_eq!(picked(&fresh, None).want, Want::Ambush);
        let worn = Situation { ambush: Some((Vec3::new(2.0, 0.0, 2.0), 0.0, 0.0)), ..calm() };
        assert_eq!(picked(&worn, None).want, Want::Roam);
    }

    /// Breaks if a goal with no way to it is kept, so a stranded bot searches the whole graph
    /// every tick for it: it is rested and another chosen.
    #[test]
    fn a_goal_with_no_way_to_it_is_put_down() {
        use bevy::ecs::system::RunSystemOnce;
        use crate::common::movement_rules::MovementRules;
        use crate::common::world_surface::Volume;
        use crate::game::collision::CollisionWorld;
        use crate::game::nav;
        use crate::tool::room::Room;
        let volumes = [Volume::air(Room::new(Vec3::ZERO, Vec3::new(4.0, 3.0, 4.0))), Volume::air(Room::new(Vec3::new(20.0, 0.0, 0.0), Vec3::new(24.0, 3.0, 4.0)))];
        let mut collision = CollisionWorld::default();
        let faces = collision.rebuild_volumes(&volumes);
        let graph = nav::graph(&collision, &faces, &[], &MovementRules::default(), &[]);
        let sealed = Goal::Place(graph.cells()[graph.cell_at(Vec3::new(22.25, 0.0, 2.25)).unwrap() as usize].feet);
        let feet = Vec3::new(1.25, 0.0, 1.25);

        let mut world = World::new();
        world.insert_resource(NavBake::holding(graph.clone()));
        world.init_resource::<Time>();
        let mut route = BotRoute::default();
        crate::game::bot::plan_for_test(&mut route, &graph, feet, sealed);
        assert!(route.is_lost());
        let thought = BotThought { chosen: Some(Candidate { want: Want::Roam, goal: sealed, score: ROAM_SCORE }), roam: Some(match sealed { Goal::Place(at) => at, _ => unreachable!() }), ..default() };
        let bot = world
            .spawn((
                PhysicsBody::at(crate::common::class::body_center_from_feet(feet)),
                Stance::Standing,
                BotSenses::default(),
                BotMind::default(),
                route,
                PlayerId(1),
                (BotIntent { goal: Some(sealed), ..default() }, thought, BotOf(Entity::PLACEHOLDER)),
            ))
            .id();
        world.run_system_once(think).unwrap();
        assert_ne!(world.get::<BotIntent>(bot).unwrap().goal, Some(sealed), "a goal with no way to it was kept");
        let thought = world.get::<BotThought>(bot).unwrap();
        assert!(thought.resting(sealed), "it would be chosen again at once");
        assert!(thought.next >= LOST_RETRY, "a stranded bot would search the graph again within the second");
    }

    /// Breaks if filing cells by square loses any near a bot, or offers one out of reach: it finds
    /// exactly what a scan of every cell does, across square edges and at negative coordinates.
    #[test]
    fn the_cells_near_a_bot_are_the_ones_a_scan_finds() {
        use crate::common::movement_rules::MovementRules;
        use crate::common::world_surface::Volume;
        use crate::game::collision::CollisionWorld;
        use crate::game::nav;
        use crate::tool::room::Room;
        let mut collision = CollisionWorld::default();
        let faces = collision.rebuild_volumes(&[Volume::air(Room::new(Vec3::new(-14.0, 0.0, -9.0), Vec3::new(13.0, 3.0, 11.0)))]);
        let graph = nav::graph(&collision, &faces, &[], &MovementRules::default(), &[]);
        let mut index = MapIndex::default();
        index.keep(&graph, 0);
        for at in [Vec3::ZERO, Vec3::new(-7.9, 0.0, 3.99), Vec3::new(12.0, 0.0, -8.0)] {
            let mut near: Vec<CellId> = index.near(&graph, at, COVER_RADIUS).collect();
            near.sort();
            let scanned: Vec<CellId> = (0..graph.cells().len() as CellId).filter(|cell| graph.cells()[*cell as usize].feet.distance(at) <= COVER_RADIUS).collect();
            assert!(!scanned.is_empty());
            assert_eq!(near, scanned, "around {at}");
        }
    }

    /// Breaks if the runner-up is the same want as the winner, which says nothing about the choice.
    #[test]
    fn the_runner_up_is_a_different_want() {
        let situation = Situation { health: 0.4, pickups: vec![(0, Offer::Health, 2.0), (1, Offer::Health, 3.0)], ..calm() };
        let (best, runner_up) = choose(&rank(&candidates(&situation, &Drives::default()), None, &Drives::default()));
        assert_eq!(best.unwrap().want, Want::Health);
        assert_eq!(runner_up.unwrap().want, Want::Roam);
    }
}
