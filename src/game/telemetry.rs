//! What each round did, kept for reading afterward. Two records, both on the authority:
//!
//! - **`cache/rounds/{pack}/{map}/{started}.jsonl`** ([`rounds_folder`]) gets one JSON [`Event`] per line, game seconds since
//!   the round started first in each: spawns, every hit, kills, pickups, what bots go for and
//!   who they first see, and every body's place once a second. Derived and disposable, like the
//!   rest of `cache/`. Native only.
//! - **The log** gets the same events as `GAME_EVENT {json}` lines ([`log_event`]), the frequent
//!   ones at debug, plus a few only the log has: a [`RoundRecord`] at the round's end, team and
//!   class choices, and a client's view of each death.
//!
//! A round is one match's live phase, so matches played back to back are a file each; without
//! the match cycle (the game's tests) it is the whole of Play.
//!
//! Nothing here is read back by the game: it only watches.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::Serialize;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::match_phase::MatchPhase;
use crate::common::damage::{Armor, DamageDealt, DamageSource, Damageable, Died, How, PlayerId};
use crate::common::net::has_authority;
use crate::common::round_settings::{BotSkill, RoundSettings};
use crate::common::team::Team;
use crate::common::weapon::{Equipped, WeaponCatalog, WeaponId};
use crate::game::bot::{BotIntent, BotOf, BotSeat, Goal, RouteCosts};
use crate::game::bot_sense::{BotMind, BotSenses, Sense};
use crate::game::bot_think::{BotThought, Want};
use crate::game::player::{PhysicsBody, Player};
use crate::game::respawn::Identity;
use crate::game::score::{ranked, Standing};
use crate::game::sights::Sights;
use crate::game::world_prop::{PickupTaken, PickupWhat};
use crate::tool::nav_bake::NavBake;
use crate::get;

/// Where round files go, a folder per map.
/// Under [`cache_path::CACHE_ROOT`](crate::common::cache_path::CACHE_ROOT).
pub const ROUNDS_KIND: &str = "rounds";

/// `cache/rounds/{pack}/{map}`, for the map saved at `file`.
pub fn rounds_folder(file: Option<&std::path::Path>) -> std::path::PathBuf {
    crate::common::cache_path::document_folder(ROUNDS_KIND, file)
}

/// `cache/rounds`, holding a folder per pack.
pub fn rounds_root() -> std::path::PathBuf {
    std::path::Path::new(crate::common::cache_path::CACHE_ROOT).join(ROUNDS_KIND)
}
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
#[derive(Serialize, Clone, Debug, PartialEq)]
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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub headshot: bool,
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
    /// Looking through a scope.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub sighted: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    RoundStart { t: f32, round: u32, map: String, started: String, slots: u32, fill_with_bots: bool, bot_skill: BotSkill, time_limit: Option<f32>, frag_limit: Option<u32> },
    /// The first time somebody is seen in the round, and again whenever a bot's temperament is
    /// rolled again.
    Player { t: f32, id: u64, name: String, bot: bool, temperament: Option<Temperament> },
    Spawn { t: f32, id: u64, at: [f32; 3], yaw: f32 },
    /// The Round tab's bot skill changed mid-round.
    Skill { t: f32, bot_skill: BotSkill },
    Damage {
        t: f32,
        target: Option<u64>,
        by: Option<u64>,
        amount: u32,
        remaining: u32,
        weapon: Option<String>,
        how: How,
        reflected: bool,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        headshot: bool,
        at: [f32; 3],
    },
    Kill(KillRecord),
    Pickup { t: f32, by: Option<u64>, what: String, at: [f32; 3] },
    /// A bot going for something else. A retreat is a turn to health or cover with an enemy in
    /// sight.
    Goal { t: f32, id: u64, want: String, score: f32, runner_up: Option<(String, f32)>, at: Option<[f32; 3]>, health: u32, enemies_in_sight: u32, retreat: bool },
    /// A bot seeing somebody it had not seen for [`SIGHT_GAP`].
    Sight { t: f32, by: u64, of: u64, distance: f32 },
    Sample { t: f32, bodies: Vec<Sampled> },
    RoundEnd { t: f32, scoreboard: Vec<Standing> },
    /// Logged only, after the round's file is closed: every bot's temperament and the scoreboard.
    RoundRecord(RoundRecord),
    /// Logged only, by the authority. With no team, `team` is `None`.
    Seat { id: u64, seat: SeatKind, team: Option<Team> },
    /// Logged only, by the authority: a class chosen, for the next body.
    Class { id: u64, class: String },
    /// Logged only, by a client, which keeps no round: a death as it was told of it. A `killer`
    /// of `None` is the map.
    Death { victim: Option<u64>, victim_name: Option<String>, killer: Option<u64>, assist: Option<u64>, at: [f32; 3] },
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SeatKind {
    Playing,
    Spectating,
}

/// The prefix of every game event's line in the log, to grep for.
pub const GAME_EVENT: &str = "GAME_EVENT";
/// The log target of every game event, for `RUST_LOG`.
pub const EVENT_TARGET: &str = "grackle::event";

impl Event {
    /// Many a second at the busiest: every hit, every bot's change of mind, every sighting and
    /// the once-a-second sample.
    fn frequent(&self) -> bool {
        matches!(self, Event::Damage { .. } | Event::Goal { .. } | Event::Sight { .. } | Event::Sample { .. })
    }
}

/// Writes `GAME_EVENT {json}` to the log, the JSON being what a round's `.jsonl` file holds.
pub fn log_event(event: &Event) {
    let line = || serde_json::to_string(event).expect("an event is plain data");
    if event.frequent() {
        if bevy::log::tracing::enabled!(target: EVENT_TARGET, bevy::log::Level::DEBUG) {
            debug!(target: EVENT_TARGET, "{GAME_EVENT} {}", line());
        }
    } else {
        info!(target: EVENT_TARGET, "{GAME_EVENT} {}", line());
    }
}

/// `None` is the map.
pub fn source_id(source: DamageSource) -> Option<u64> {
    match source {
        DamageSource::Player(id) => Some(id.0),
        DamageSource::World => None,
    }
}

/// What was last seen of somebody, kept so a body reaped before `Update` still has a record.
/// Plain data, refreshed every frame; a [`Party`]'s strings are built only when one is written.
#[derive(Clone, Debug)]
struct Last {
    entity: Entity,
    /// The standing's name as set, or `None` with no standing.
    named: Option<String>,
    health: u32,
    armor: u32,
    at: [f32; 3],
    /// A bot's chosen want and every want's pull.
    thought: Option<(Option<Want>, Vec<(Want, f32)>)>,
    spawned: f32,
    damage: Vec<Share>,
}

impl Last {
    fn new(entity: Entity, spawned: f32) -> Self {
        Self { entity, named: None, health: 0, armor: 0, at: [0.0; 3], thought: None, spawned, damage: Vec::new() }
    }

    fn party(&self, id: PlayerId) -> Party {
        Party {
            id: id.0,
            name: name_of(id, self.named.as_deref()),
            health: self.health,
            armor: self.armor,
            at: self.at,
            wants: self.thought.as_ref().map(|(chosen, pulls)| Wants {
                chosen: chosen.map(want_name),
                pulls: pulls.iter().map(|(want, pull)| (want_name(*want), (pull * 1000.0).round() / 1000.0)).collect(),
            }),
        }
    }
}

/// [`Standing::display_name`], or "Player {id}" for somebody with no standing.
fn name_of(id: PlayerId, named: Option<&str>) -> String {
    match named {
        None => format!("Player {}", id.0),
        Some("") => get!("scoreboard.player", "id", id.0),
        Some(name) => name.to_owned(),
    }
}

#[derive(Resource, Default)]
struct Round {
    /// Between opening and closing. Outside it the systems still read every message, so one
    /// from a post-game cannot wait unread for the next match, and write nothing.
    open: bool,
    number: u32,
    map: String,
    /// Game seconds at its start.
    started: f32,
    next_sample: f32,
    people: HashMap<PlayerId, Last>,
    /// The temperament last written for each body seen, `None` for a person.
    announced: HashMap<PlayerId, Option<Temperament>>,
    bot_skill: BotSkill,
    wanted: HashMap<PlayerId, Want>,
    sightings: HashMap<(PlayerId, PlayerId), f32>,
    #[cfg(not(target_arch = "wasm32"))]
    file: Option<std::io::BufWriter<std::fs::File>>,
    #[cfg(test)]
    written: Vec<Event>,
}

impl Round {
    fn write(&mut self, event: &Event) {
        if !self.open {
            return;
        }
        log_event(event);
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

/// Inserted by [`crate::launch::run`]: rounds are written to `cache/rounds/` only where somebody
/// asked, so the game's tests, which play rounds, leave no files behind.
#[derive(Resource, Default)]
pub struct RecordRounds;

pub struct TelemetryPlugin;

impl Plugin for TelemetryPlugin {
    fn build(&self, app: &mut App) {
        // With the match cycle a round is one match's live phase, so back-to-back matches are a
        // file each. Leaving Play mid-match leaves `Live` too, the phase being Play's sub-state.
        let no_cycle = not(resource_exists::<crate::game::match_cycle::MatchCycle>);
        app.init_resource::<Round>()
            .add_message::<Died>()
            .add_message::<DamageDealt>()
            .add_message::<PickupTaken>()
            .add_systems(OnEnter(AppMode::Play), open_the_round.run_if(has_authority).run_if(no_cycle.clone()))
            .add_systems(
                Update,
                (note_the_skill, watch_bodies, note_damage, note_kills, note_pickups, note_thoughts, note_sightings, sample)
                    .chain()
                    .run_if(in_state(AppMode::Play))
                    .run_if(has_authority),
            )
            .add_systems(OnExit(AppMode::Play), close_the_round.run_if(has_authority).run_if(no_cycle))
            .add_systems(OnEnter(MatchPhase::Live), open_the_round.run_if(has_authority))
            .add_systems(OnExit(MatchPhase::Live), close_the_round.run_if(has_authority));
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

/// The variant's name in lower case.
pub(crate) fn want_name(want: Want) -> String {
    match want {
        Want::Health => "health",
        Want::Armor => "armor",
        Want::Arms => "arms",
        Want::Hunt => "hunt",
        Want::Cover => "cover",
        Want::Ambush => "ambush",
        Want::Camp => "camp",
        Want::Roam => "roam",
    }
    .to_owned()
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
    metadata: Option<Res<crate::editor::map_metadata::MapMetadata>>,
    file: Option<Res<crate::editor::panels::CurrentFilePath>>,
    recording: Option<Res<RecordRounds>>,
) {
    let number = round.number + 1;
    let map = map_name(metadata.as_deref(), file.as_deref());
    let now = chrono::Local::now();
    let settings = settings.as_deref().copied().unwrap_or_default();
    *round = Round { open: true, number, map: map.clone(), started: time.elapsed_secs(), bot_skill: settings.bot_skill, ..default() };
    #[cfg(not(target_arch = "wasm32"))]
    if recording.is_some() {
        let directory = rounds_folder(file.as_deref().and_then(|file| file.path.as_deref()));
        let stamp = now.format("%Y-%m-%d_%H-%M-%S");
        // Never over another round's file: two servers, or a round under a second long.
        let opened = std::fs::create_dir_all(&directory).and_then(|_| {
            let mut tries = (1..).map(|part| match part {
                1 => directory.join(format!("{stamp}.jsonl")),
                part => directory.join(format!("{stamp}.{part}.jsonl")),
            });
            loop {
                let path = tries.next().expect("endless");
                match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                    Ok(file) => break Ok((path, file)),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => break Err(error),
                }
            }
        });
        match opened {
            Ok((path, opened)) => {
                info!("Recording this round to {}", path.display());
                round.file = Some(std::io::BufWriter::new(opened));
            }
            Err(error) => warn!("Not recording this round to {}: {error}", directory.display()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = recording;
    round.write(&Event::RoundStart {
        t: 0.0,
        round: number,
        map,
        started: now.to_rfc3339(),
        slots: settings.slots,
        fill_with_bots: settings.fill_with_bots,
        bot_skill: settings.bot_skill,
        time_limit: settings.time_limit,
        frag_limit: settings.frag_limit,
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
        Option<&'static Sights>,
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
    for (entity, id, body, stance, player, health, armor, _, thought, temperament, _) in &bodies {
        let named = standings.iter().find(|standing| standing.id == *id).map(|standing| standing.name.as_str());
        let at = point(crate::game::bot::feet_of(body, *stance));
        let temperament = temperament.map(|(route, mind)| Temperament { route: *route, mind: *mind });
        // Beliefs are rolled again each life, which is no new temperament.
        let rolled = temperament.map(|temperament| Temperament { route: RouteCosts { beliefs: 0, ..temperament.route }, ..temperament });
        if round.announced.get(id) != Some(&rolled) {
            round.announced.insert(*id, rolled);
            round.write(&Event::Player { t, id: id.0, name: name_of(*id, named), bot: temperament.is_some(), temperament });
        }
        let spawned = fresh.contains(entity);
        if spawned {
            round.write(&Event::Spawn { t, id: id.0, at, yaw: player.yaw });
        }
        let last = round.people.entry(*id).or_insert_with(|| Last::new(entity, t));
        if spawned {
            last.spawned = t;
            last.damage.clear();
        }
        last.entity = entity;
        if last.named.as_deref() != named {
            last.named = named.map(str::to_owned);
        }
        last.health = health.map_or(0, Damageable::health);
        last.armor = armor.map_or(0, Armor::points);
        last.at = at;
        match (thought, &mut last.thought) {
            (None, kept) => *kept = None,
            (Some(thought), Some((chosen, pulls))) => {
                *chosen = thought.chosen.map(|chosen| chosen.want);
                pulls.clear();
                pulls.extend_from_slice(&thought.pulls);
            }
            (Some(thought), kept) => *kept = Some((thought.chosen.map(|chosen| chosen.want), thought.pulls.clone())),
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
            headshot: hit.cause.headshot,
            at: point(hit.point),
        };
        round.write(&event);
    }
}

fn note_kills(mut round: ResMut<Round>, time: Res<Time>, mut died: MessageReader<Died>, catalog: Option<Res<WeaponCatalog>>) {
    let t = time.elapsed_secs() - round.started;
    for death in died.read() {
        let Some(victim_id) = death.victim_id else { continue };
        let Some(victim) = round.people.get_mut(&victim_id) else { continue };
        // Feet, as last seen, like everybody else's; `Died::at` is the body's center.
        let party = victim.party(victim_id);
        let life = Some(t - victim.spawned);
        let damage = std::mem::take(&mut victim.damage);
        let party_of = |round: &Round, source: Option<DamageSource>| match source? {
            DamageSource::Player(id) => round.people.get(&id).map(|last| last.party(id)),
            DamageSource::World => None,
        };
        let killer = party_of(&round, Some(death.killer));
        let assist = party_of(&round, death.assist);
        let cause = death.cause;
        let distance = killer.as_ref().filter(|killer| killer.id != victim_id.0).map(|killer| {
            let (a, b) = (Vec3::from_array(killer.at), Vec3::from_array(party.at));
            (a.distance(b) * 100.0).round() / 100.0
        });
        let record = KillRecord { t, victim: party, killer, assist, weapon: weapon_key(catalog.as_deref(), cause.weapon), how: cause.how, reflected: cause.reflected, headshot: cause.headshot, distance, life, damage };
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
            let last = round.sightings.entry((*id, known.who)).or_insert(f32::NEG_INFINITY);
            let found = t - *last > SIGHT_GAP;
            *last = t;
            if found {
                let distance = (body.current.distance(known.at) * 100.0).round() / 100.0;
                round.write(&Event::Sight { t, by: id.0, of: known.who.0, distance });
            }
        }
    }
}

fn note_the_skill(mut round: ResMut<Round>, time: Res<Time>, settings: Option<Res<RoundSettings>>) {
    let Some(skill) = settings.map(|settings| settings.bot_skill) else { return };
    if skill != round.bot_skill {
        round.bot_skill = skill;
        let t = time.elapsed_secs() - round.started;
        round.write(&Event::Skill { t, bot_skill: skill });
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
        .map(|(_, id, body, stance, player, health, armor, equipped, thought, _, sights)| Sampled {
            id: id.0,
            at: point(crate::game::bot::feet_of(body, *stance)),
            yaw: (player.yaw * 1000.0).round() / 1000.0,
            health: health.map_or(0, Damageable::health),
            armor: armor.map_or(0, Armor::points),
            weapon: weapon_key(catalog.as_deref(), equipped.and_then(Equipped::held)),
            want: thought.and_then(|thought| thought.chosen).map(|chosen| want_name(chosen.want)),
            sighted: sights.is_some_and(Sights::sighted),
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
    if !round.open {
        return;
    }
    let t = time.elapsed_secs() - round.started;
    let scoreboard: Vec<Standing> = ranked(standings.iter()).into_iter().cloned().collect();
    round.write(&Event::RoundEnd { t, scoreboard: scoreboard.clone() });
    round.flush();
    round.open = false;
    #[cfg(not(target_arch = "wasm32"))]
    {
        round.file = None;
    }
    let name = |id: PlayerId| scoreboard.iter().find(|standing| standing.id == id).map_or_else(|| format!("Player {}", id.0), Standing::display_name);
    let mut temperaments: Vec<(u64, String, Temperament)> = seats.iter().map(|(who, route, mind)| (who.id.0, name(who.id), Temperament { route: *route, mind: *mind })).collect();
    temperaments.sort_by_key(|(id, ..)| *id);
    let record = RoundRecord { round: round.number, map: round.map.clone(), seconds: (t * 10.0).round() / 10.0, temperaments, scoreboard };
    log_event(&Event::RoundRecord(record));
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
            .insert_resource(Round { open: true, ..default() })
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
            cause: Cause::of(rocket, How::Splash),
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
    /// Breaks if matches played back to back in one stay in Play share a file, or a kill in the
    /// post-game is written into the match it followed.
    #[test]
    fn each_match_is_a_round_of_its_own() {
        let mut app = App::new();
        app.add_plugins((bevy::time::TimePlugin, bevy::state::app::StatesPlugin))
            .init_state::<AppMode>()
            .add_sub_state::<MatchPhase>()
            .init_resource::<crate::game::match_cycle::MatchCycle>()
            .add_plugins(TelemetryPlugin);
        app.update();
        let phase = |app: &mut App, phase: MatchPhase| {
            app.world_mut().resource_mut::<NextState<MatchPhase>>().set(phase);
            app.update();
        };
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        let victim = app.world_mut().spawn((PlayerId(1), Player::default(), PhysicsBody::at(Vec3::Y), Stance::Standing, Damageable::default())).id();
        for _ in 0..2 {
            phase(&mut app, MatchPhase::Live);
            phase(&mut app, MatchPhase::PostGame);
            app.world_mut().write_message(Died {
                victim,
                victim_name: None,
                victim_id: Some(PlayerId(1)),
                killer: DamageSource::World,
                assist: None,
                at: Vec3::Y,
                cause: Cause::of(None, How::Kill),
            });
            app.update();
            // With no pre-game or time between rounds, the next match starts at once.
            phase(&mut app, MatchPhase::PreGame);
        }
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Editor);
        app.update();

        let round = app.world().resource::<Round>();
        assert_eq!(round.number, 2, "the second match did not start a round of its own");
        // What `written` holds is the second round's.
        let count = |of: fn(&Event) -> bool| round.written.iter().filter(|event| of(event)).count();
        assert_eq!(count(|event| matches!(event, Event::RoundEnd { .. })), 1, "leaving Play after the post-game closed the round again");
        assert_eq!(count(|event| matches!(event, Event::Kill(_))), 0, "a post-game kill was written");
    }

    /// Breaks if a bot's temperament rolled again mid-round is not recorded, or a new life's
    /// beliefs read as a new temperament.
    #[test]
    fn a_temperament_rolled_again_is_recorded_again() {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin).insert_resource(Round { open: true, ..default() }).add_systems(Update, watch_bodies);
        let bot = app.world_mut().spawn((PlayerId(1), Player::default(), PhysicsBody::at(Vec3::Y), Stance::Standing, RouteCosts::default(), BotMind::default())).id();
        app.update();
        app.world_mut().entity_mut(bot).insert(RouteCosts { beliefs: 7, ..default() });
        app.update();
        app.world_mut().entity_mut(bot).insert(BotMind { reaction: 0.25, ..default() });
        app.update();

        let players = app.world().resource::<Round>().written.iter().filter(|event| matches!(event, Event::Player { .. })).count();
        assert_eq!(players, 2);
    }
}
