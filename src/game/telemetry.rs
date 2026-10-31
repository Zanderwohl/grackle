//! What each round did, kept for reading afterward. Two records, both on the authority:
//!
//! - **The log** gets a RON line per kill — victim, killer and assist with their health and what
//!   each wanted, the weapon, how it killed and who did what damage — and at the round's end a
//!   [`RoundRecord`]: every bot's temperament and the scoreboard.
//! - **`cache/rounds/{map}/{started}.jsonl`** gets one JSON [`Event`] per line, game seconds since
//!   the round started first in each: spawns, every hit, kills, pickups, what bots go for and
//!   who they first see, and every body's place once a second. Derived and disposable, like the
//!   rest of `cache/`. Native only.
//!
//! Nothing here is read back by the game: it only watches.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::Serialize;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::damage::{Armor, DamageDealt, DamageSource, Damageable, Died, How, PlayerId};
use crate::common::game_rules::GameRules;
use crate::common::net::has_authority;
use crate::common::round_settings::RoundSettings;
use crate::common::weapon::{Equipped, WeaponCatalog, WeaponId};
use crate::game::bot::{BotIntent, BotOf, BotSeat, Goal, RouteCosts};
use crate::game::bot_sense::{BotMind, BotSenses, Sense};
use crate::game::bot_think::{BotThought, Want};
use crate::game::player::{PhysicsBody, Player};
use crate::game::respawn::Identity;
use crate::game::score::{ranked, Standing};
use crate::game::world_prop::{PickupTaken, PickupWhat};
use crate::tool::nav_bake::NavBake;

/// Game seconds between samples of where everybody is.
const SAMPLE_EVERY: f32 = 1.0;
/// A bot seeing somebody it last saw longer ago than this has found them again.
const SIGHT_GAP: f32 = 3.0;

/// What a bot is made of: rolled per seat and kept across lives.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Temperament {
    pub route: RouteCosts,
    pub mind: BotMind,
}

/// One per round, at its end, in the log.
#[derive(Serialize, Debug)]
pub struct RoundRecord {
    pub round: u32,
    pub map: String,
    pub seconds: f32,
    pub temperaments: Vec<(u64, String, Temperament)>,
    /// Winner first.
    pub scoreboard: Vec<Standing>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Party {
    pub id: u64,
    pub name: String,
    /// Before the hit that killed, for the victim.
    pub health: u32,
    pub armor: u32,
    /// Feet.
    pub at: [f32; 3],
    /// A bot's: what it was going for, and every want's pull.
    pub wants: Option<Wants>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Wants {
    pub chosen: Option<String>,
    pub pulls: Vec<(String, f32)>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Share {
    /// `None` is the map.
    pub by: Option<u64>,
    pub amount: u32,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct KillRecord {
    pub t: f32,
    pub victim: Party,
    /// `None` is the map; the victim's own id is a suicide.
    pub killer: Option<Party>,
    pub assist: Option<Party>,
    pub weapon: Option<String>,
    pub how: How,
    pub reflected: bool,
    /// Meters from the killer's feet to the victim's.
    pub distance: Option<f32>,
    /// Seconds since the victim last spawned.
    pub life: Option<f32>,
    /// Everything that hurt the victim this life, by whom.
    pub damage: Vec<Share>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Sampled {
    pub id: u64,
    pub at: [f32; 3],
    pub yaw: f32,
    pub health: u32,
    pub armor: u32,
    pub weapon: Option<String>,
    pub want: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    RoundStart { t: f32, round: u32, map: String, started: String, slots: u32, fill_with_bots: bool, time_limit: Option<f32>, frag_limit: Option<u32> },
    /// The first time somebody is seen in the round.
    Player { t: f32, id: u64, name: String, bot: bool, temperament: Option<Temperament> },
    Spawn { t: f32, id: u64, at: [f32; 3], yaw: f32 },
    Damage { t: f32, target: Option<u64>, by: Option<u64>, amount: u32, remaining: u32, weapon: Option<String>, how: How, reflected: bool, at: [f32; 3] },
    Kill(KillRecord),
    Pickup { t: f32, by: Option<u64>, what: String, at: [f32; 3] },
    /// A bot going for something else. A retreat is a turn to health or cover with an enemy in
    /// sight.
    Goal { t: f32, id: u64, want: String, score: f32, runner_up: Option<(String, f32)>, at: Option<[f32; 3]>, health: u32, enemies_in_sight: u32, retreat: bool },
    /// A bot seeing somebody it had not seen for [`SIGHT_GAP`].
    Sight { t: f32, by: u64, of: u64, distance: f32 },
    Sample { t: f32, bodies: Vec<Sampled> },
    RoundEnd { t: f32, scoreboard: Vec<Standing> },
}

/// What was last seen of somebody, kept so a body reaped before `Update` still has a record.
#[derive(Clone, Debug)]
struct Last {
    entity: Entity,
    party: Party,
    spawned: f32,
    damage: Vec<Share>,
}

#[derive(Resource, Default)]
struct Round {
    number: u32,
    map: String,
    /// Game seconds at its start.
    started: f32,
    next_sample: f32,
    people: HashMap<PlayerId, Last>,
    announced: Vec<PlayerId>,
    wanted: HashMap<PlayerId, Want>,
    sightings: HashMap<(PlayerId, PlayerId), f32>,
    /// The hit that emptied each body, from `note_damage` to `note_kills`.
    fatal: HashMap<Entity, crate::common::damage::Cause>,
    #[cfg(not(target_arch = "wasm32"))]
    file: Option<std::io::BufWriter<std::fs::File>>,
    #[cfg(test)]
    written: Vec<Event>,
}

impl Round {
    fn write(&mut self, event: &Event) {
        #[cfg(test)]
        self.written.push(event.clone());
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(file) = self.file.as_mut() {
            use std::io::Write;
            let line = serde_json::to_string(event).expect("an event is plain data");
            if let Err(error) = writeln!(file, "{line}") {
                warn!("Stopped writing this round's telemetry: {error}");
                self.file = None;
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = event;
    }

    fn flush(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(file) = self.file.as_mut() {
            use std::io::Write;
            let _ = file.flush();
        }
    }
}

/// Inserted by the `editor` binary: rounds are written to `cache/rounds/` only where somebody
/// asked, so the game's tests, which play rounds, leave no files behind.
#[derive(Resource, Default)]
pub struct RecordRounds;

pub struct TelemetryPlugin;

impl Plugin for TelemetryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Round>()
            .add_message::<Died>()
            .add_message::<DamageDealt>()
            .add_message::<PickupTaken>()
            .add_systems(OnEnter(AppMode::Play), open_the_round.run_if(has_authority))
            .add_systems(
                Update,
                (watch_bodies, note_damage, note_kills, note_pickups, note_thoughts, note_sightings, sample)
                    .chain()
                    .run_if(in_state(AppMode::Play))
                    .run_if(has_authority),
            )
            .add_systems(OnExit(AppMode::Play), close_the_round.run_if(has_authority));
    }
}

fn point(at: Vec3) -> [f32; 3] {
    let round = |value: f32| (value * 100.0).round() / 100.0;
    [round(at.x), round(at.y), round(at.z)]
}

fn weapon_key(catalog: Option<&WeaponCatalog>, weapon: Option<WeaponId>) -> Option<String> {
    let weapon = weapon?;
    Some(catalog.and_then(|catalog| catalog.get(weapon)).map_or_else(|| format!("{weapon:?}"), |spec| spec.key.clone()))
}

fn want_name(want: Want) -> String {
    format!("{want:?}").to_lowercase()
}

fn wants_of(thought: &BotThought) -> Wants {
    Wants { chosen: thought.chosen.map(|chosen| want_name(chosen.want)), pulls: thought.pulls.iter().map(|(want, pull)| (want_name(*want), (pull * 1000.0).round() / 1000.0)).collect() }
}

/// The map's name as a folder: its display name, else its file's, made safe for a path.
fn map_name(metadata: Option<&crate::editor::map_metadata::MapMetadata>, file: Option<&crate::editor::panels::CurrentFilePath>) -> String {
    let named = metadata.and_then(|metadata| metadata.display_name.clone());
    let filed = file.and_then(|file| file.path.as_ref()).and_then(|path| path.file_stem()).map(|stem| stem.to_string_lossy().into_owned());
    let name = named.or(filed).unwrap_or_else(|| "untitled".to_owned());
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '_' }).collect();
    if safe.is_empty() { "untitled".to_owned() } else { safe }
}

#[allow(clippy::too_many_arguments)]
fn open_the_round(
    mut round: ResMut<Round>,
    time: Res<Time>,
    settings: Option<Res<RoundSettings>>,
    rules: Option<Res<GameRules>>,
    metadata: Option<Res<crate::editor::map_metadata::MapMetadata>>,
    file: Option<Res<crate::editor::panels::CurrentFilePath>>,
    recording: Option<Res<RecordRounds>>,
) {
    let number = round.number + 1;
    let map = map_name(metadata.as_deref(), file.as_deref());
    let now = chrono::Local::now();
    *round = Round { number, map: map.clone(), started: time.elapsed_secs(), ..default() };
    #[cfg(not(target_arch = "wasm32"))]
    if recording.is_some() {
        let directory = std::path::Path::new("cache/rounds").join(&map);
        let path = directory.join(format!("{}.jsonl", now.format("%Y-%m-%d_%H-%M-%S")));
        match std::fs::create_dir_all(&directory).and_then(|_| std::fs::File::create(&path)) {
            Ok(opened) => {
                info!("Recording this round to {}", path.display());
                round.file = Some(std::io::BufWriter::new(opened));
            }
            Err(error) => warn!("Not recording this round to {}: {error}", path.display()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = recording;
    let settings = settings.as_deref().copied().unwrap_or_default();
    round.write(&Event::RoundStart {
        t: 0.0,
        round: number,
        map,
        started: now.to_rfc3339(),
        slots: settings.slots,
        fill_with_bots: settings.fill_with_bots,
        time_limit: settings.time_limit,
        frag_limit: rules.and_then(|rules| rules.frag_limit),
    });
}

type Bodies<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static PlayerId,
        &'static PhysicsBody,
        &'static Stance,
        &'static Player,
        Option<&'static Damageable>,
        Option<&'static Armor>,
        Option<&'static Equipped>,
        Option<&'static BotThought>,
        Option<(&'static RouteCosts, &'static BotMind)>,
    ),
>;

/// Keeps [`Last`] current, and notes who is new to the round and who has just spawned.
fn watch_bodies(
    mut round: ResMut<Round>,
    time: Res<Time>,
    bodies: Bodies,
    fresh: Query<(), Added<Player>>,
    standings: Query<&Standing>,
) {
    let t = time.elapsed_secs() - round.started;
    for (entity, id, body, stance, player, health, armor, _, thought, temperament) in &bodies {
        let name = standings.iter().find(|standing| standing.id == *id).map_or_else(|| format!("Player {}", id.0), Standing::display_name);
        let party = Party {
            id: id.0,
            name: name.clone(),
            health: health.map_or(0, Damageable::health),
            armor: armor.map_or(0, Armor::points),
            at: point(crate::game::bot::feet_of(body, *stance)),
            wants: thought.map(wants_of),
        };
        if !round.announced.contains(id) {
            round.announced.push(*id);
            let temperament = temperament.map(|(route, mind)| Temperament { route: *route, mind: *mind });
            round.write(&Event::Player { t, id: id.0, name, bot: temperament.is_some(), temperament });
        }
        if fresh.contains(entity) {
            round.write(&Event::Spawn { t, id: id.0, at: party.at, yaw: player.yaw });
            round.people.insert(*id, Last { entity, party, spawned: t, damage: Vec::new() });
            continue;
        }
        match round.people.get_mut(id) {
            Some(last) => (last.entity, last.party) = (entity, party),
            None => {
                round.people.insert(*id, Last { entity, party, spawned: t, damage: Vec::new() });
            }
        }
    }
}

fn id_of(round: &Round, entity: Entity) -> Option<PlayerId> {
    round.people.iter().find(|(_, last)| last.entity == entity).map(|(id, _)| *id)
}

fn note_damage(mut round: ResMut<Round>, time: Res<Time>, mut dealt: MessageReader<DamageDealt>, catalog: Option<Res<WeaponCatalog>>) {
    let t = time.elapsed_secs() - round.started;
    for hit in dealt.read() {
        if hit.amount == 0 {
            continue;
        }
        let target = id_of(&round, hit.target);
        let by = match hit.source {
            DamageSource::Player(id) => Some(id.0),
            DamageSource::World => None,
        };
        if let Some(last) = target.and_then(|target| round.people.get_mut(&target)) {
            match last.damage.iter_mut().find(|share| share.by == by) {
                Some(share) => share.amount += hit.amount,
                None => last.damage.push(Share { by, amount: hit.amount }),
            }
        }
        let event = Event::Damage {
            t,
            target: target.map(|target| target.0),
            by,
            amount: hit.amount,
            remaining: hit.remaining,
            weapon: weapon_key(catalog.as_deref(), hit.cause.weapon),
            how: hit.cause.how,
            reflected: hit.cause.reflected,
            at: point(hit.point),
        };
        round.write(&event);
        if hit.remaining == 0 {
            round.fatal.insert(hit.target, hit.cause);
        }
    }
}

fn note_kills(mut round: ResMut<Round>, time: Res<Time>, mut died: MessageReader<Died>, catalog: Option<Res<WeaponCatalog>>) {
    let t = time.elapsed_secs() - round.started;
    for death in died.read() {
        let Some(victim_id) = death.victim_id else { continue };
        let Some(victim) = round.people.get_mut(&victim_id) else { continue };
        // Feet, as last seen, like everybody else's; `Died::at` is the body's center.
        let party = victim.party.clone();
        let life = Some(t - victim.spawned);
        let damage = std::mem::take(&mut victim.damage);
        let party_of = |round: &Round, source: Option<DamageSource>| match source? {
            DamageSource::Player(id) => round.people.get(&id).map(|last| last.party.clone()),
            DamageSource::World => None,
        };
        let killer = party_of(&round, Some(death.killer));
        let assist = party_of(&round, death.assist);
        let cause = round.fatal.remove(&death.victim).unwrap_or_default();
        let distance = killer.as_ref().filter(|killer| killer.id != victim_id.0).map(|killer| {
            let (a, b) = (Vec3::from_array(killer.at), Vec3::from_array(party.at));
            (a.distance(b) * 100.0).round() / 100.0
        });
        let record = KillRecord { t, victim: party, killer, assist, weapon: weapon_key(catalog.as_deref(), cause.weapon), how: cause.how, reflected: cause.reflected, distance, life, damage };
        info!("kill {}", ron::to_string(&record).expect("a kill is plain data"));
        round.write(&Event::Kill(record));
    }
}

fn note_pickups(mut round: ResMut<Round>, time: Res<Time>, mut taken: MessageReader<PickupTaken>, catalog: Option<Res<WeaponCatalog>>) {
    let t = time.elapsed_secs() - round.started;
    for pickup in taken.read() {
        let what = match &pickup.what {
            PickupWhat::Prop(model) => model.clone(),
            PickupWhat::Weapon(weapon) => weapon_key(catalog.as_deref(), Some(*weapon)).unwrap_or_default(),
        };
        round.write(&Event::Pickup { t, by: pickup.by.map(|by| by.0), what, at: point(pickup.at) });
    }
}

/// A bot going for a different want is an event; its score changing is not.
fn note_thoughts(
    mut round: ResMut<Round>,
    time: Res<Time>,
    nav: Option<Res<NavBake>>,
    bots: Query<(&PlayerId, &BotThought, &BotSenses, Option<&Damageable>), With<BotOf>>,
) {
    let t = time.elapsed_secs() - round.started;
    for (id, thought, senses, health) in &bots {
        let Some(chosen) = thought.chosen else { continue };
        if round.wanted.get(id) == Some(&chosen.want) {
            continue;
        }
        round.wanted.insert(*id, chosen.want);
        let at = match chosen.goal {
            Goal::Place(place) => Some(point(place)),
            Goal::Pickup(index) => nav.as_deref().and_then(|nav| nav.graph().fields().pickups.get(index as usize)).map(|pickup| point(pickup.feet)),
        };
        let enemies_in_sight = senses.known.iter().filter(|known| known.in_sight && known.how == Sense::Seen).count() as u32;
        let retreat = enemies_in_sight > 0 && matches!(chosen.want, Want::Health | Want::Cover);
        round.write(&Event::Goal {
            t,
            id: id.0,
            want: want_name(chosen.want),
            score: (chosen.score * 1000.0).round() / 1000.0,
            runner_up: thought.runner_up.map(|other| (want_name(other.want), (other.score * 1000.0).round() / 1000.0)),
            at,
            health: health.map_or(0, Damageable::health),
            enemies_in_sight,
            retreat,
        });
    }
}

fn note_sightings(mut round: ResMut<Round>, time: Res<Time>, bots: Query<(&PlayerId, &PhysicsBody, &BotSenses), With<BotIntent>>) {
    let t = time.elapsed_secs() - round.started;
    for (id, body, senses) in &bots {
        for known in senses.known.iter().filter(|known| known.in_sight && known.how == Sense::Seen) {
            let key = (*id, known.who);
            let found = round.sightings.get(&key).is_none_or(|last| t - *last > SIGHT_GAP);
            round.sightings.insert(key, t);
            if found {
                let distance = (body.current.distance(known.at) * 100.0).round() / 100.0;
                round.write(&Event::Sight { t, by: id.0, of: known.who.0, distance });
            }
        }
    }
}

fn sample(mut round: ResMut<Round>, time: Res<Time>, bodies: Bodies, catalog: Option<Res<WeaponCatalog>>) {
    let t = time.elapsed_secs() - round.started;
    if t < round.next_sample {
        return;
    }
    round.next_sample = t + SAMPLE_EVERY;
    let mut sampled: Vec<Sampled> = bodies
        .iter()
        .map(|(_, id, body, stance, player, health, armor, equipped, thought, _)| Sampled {
            id: id.0,
            at: point(crate::game::bot::feet_of(body, *stance)),
            yaw: (player.yaw * 1000.0).round() / 1000.0,
            health: health.map_or(0, Damageable::health),
            armor: armor.map_or(0, Armor::points),
            weapon: weapon_key(catalog.as_deref(), equipped.and_then(Equipped::held)),
            want: thought.and_then(|thought| thought.chosen).map(|chosen| want_name(chosen.want)),
        })
        .collect();
    sampled.sort_by_key(|body| body.id);
    round.write(&Event::Sample { t, bodies: sampled });
    round.flush();
}

/// Bots by their seats, which outlive their bodies, so a bot dead at the end still has its
/// temperament recorded.
fn close_the_round(
    mut round: ResMut<Round>,
    time: Res<Time>,
    standings: Query<&Standing>,
    seats: Query<(&Identity, &RouteCosts, &BotMind), With<BotSeat>>,
) {
    let t = time.elapsed_secs() - round.started;
    let scoreboard: Vec<Standing> = ranked(standings.iter()).into_iter().cloned().collect();
    round.write(&Event::RoundEnd { t, scoreboard: scoreboard.clone() });
    round.flush();
    #[cfg(not(target_arch = "wasm32"))]
    {
        round.file = None;
    }
    let name = |id: PlayerId| scoreboard.iter().find(|standing| standing.id == id).map_or_else(|| format!("Player {}", id.0), Standing::display_name);
    let mut temperaments: Vec<(u64, String, Temperament)> = seats.iter().map(|(who, route, mind)| (who.id.0, name(who.id), Temperament { route: *route, mind: *mind })).collect();
    temperaments.sort_by_key(|(id, ..)| *id);
    let record = RoundRecord { round: round.number, map: round.map.clone(), seconds: (t * 10.0).round() / 10.0, temperaments, scoreboard };
    let pretty = ron::ser::PrettyConfig::new().depth_limit(4);
    info!("round {}", ron::ser::to_string_pretty(&record, pretty).expect("a round is plain data"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::damage::Cause;

    /// Breaks if a kill loses its weapon or kind, what each attacker did, or the victim's place.
    #[test]
    fn a_kill_is_recorded_with_its_cause_and_who_did_what() {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .init_resource::<Round>()
            .add_message::<DamageDealt>()
            .add_message::<Died>()
            .add_systems(Update, (watch_bodies, note_damage, note_kills).chain());
        let body = |app: &mut App, id: u64, at: Vec3| {
            app.world_mut().spawn((PlayerId(id), Player::default(), PhysicsBody::at(at), Stance::Standing, Damageable::default())).id()
        };
        let victim = body(&mut app, 1, Vec3::new(0.0, 1.0, 0.0));
        body(&mut app, 2, Vec3::new(10.0, 1.0, 0.0));
        app.update();

        let rocket = Some(crate::common::weapon::WeaponId(7));
        for (by, amount, remaining) in [(3, 40, 85), (2, 85, 0)] {
            app.world_mut().write_message(DamageDealt {
                target: victim,
                source: DamageSource::Player(PlayerId(by)),
                amount,
                remaining,
                point: Vec3::ZERO,
                from: None,
                cause: Cause::of(rocket, How::Splash),
            });
        }
        app.world_mut().write_message(Died {
            victim,
            victim_name: None,
            victim_id: Some(PlayerId(1)),
            killer: DamageSource::Player(PlayerId(2)),
            assist: Some(DamageSource::Player(PlayerId(3))),
            at: Vec3::new(0.0, 1.0, 0.0),
        });
        app.update();

        let round = app.world().resource::<Round>();
        let kill = round.written.iter().find_map(|event| match event { Event::Kill(kill) => Some(kill.clone()), _ => None }).expect("no kill recorded");
        assert_eq!(kill.victim.id, 1);
        assert_eq!(kill.killer.as_ref().map(|killer| killer.id), Some(2));
        assert_eq!((kill.weapon.as_deref(), kill.how), (Some("WeaponId(7)"), How::Splash), "the catalog-less key falls back to the id");
        assert_eq!(kill.damage, vec![Share { by: Some(3), amount: 40 }, Share { by: Some(2), amount: 85 }]);
        assert_eq!(kill.distance, Some(10.0));
        assert!(round.written.iter().filter(|event| matches!(event, Event::Damage { .. })).count() == 2);
    }
}
