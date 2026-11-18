//! Reads a round file that [`telemetry`](crate::game::telemetry) wrote back into tables for the
//! editor's Rounds tab.
//!
//! Lenient: an unknown event or a line that will not parse is passed over, so a file from an
//! older or newer build still reads, and a file cut short (the process quit mid-round) reads as
//! far as it goes. Every count comes from the events rather than the closing scoreboard, so the
//! tables agree with each other; only healing, which no event carries, is the scoreboard's.

use std::collections::{BTreeMap, HashMap, HashSet};

use bevy::prelude::*;
use serde::Deserialize;

use crate::common::damage::How;
use crate::common::round_settings::BotSkill;
use crate::game::bot_think::Want;
use crate::game::score::{ranked, Standing};
use crate::get;

#[derive(Clone, Debug, PartialEq)]
pub enum Sort {
    Number(f64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: String,
    pub sort: Sort,
    /// A player's color, drawn as a square before the text.
    pub swatch: Option<Color>,
}

impl Cell {
    fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        Cell { sort: Sort::Text(text.to_lowercase()), text, swatch: None }
    }

    fn number(value: f64, text: String) -> Self {
        Cell { text, sort: Sort::Number(value), swatch: None }
    }

    fn count(value: u32) -> Self {
        Cell::number(value as f64, value.to_string())
    }

    /// Sorts below every number.
    fn blank() -> Self {
        Cell { text: "–".to_owned(), sort: Sort::Number(f64::NEG_INFINITY), swatch: None }
    }

    fn decimal(value: f64, places: usize) -> Self {
        Cell::number(value, format!("{value:.places$}"))
    }

    fn percent(share: f64) -> Self {
        Cell::number(share, format!("{:.0}%", share * 100.0))
    }

    fn meters(value: Option<f32>) -> Self {
        value.map_or_else(Cell::blank, |value| Cell::number(value as f64, format!("{value:.1} m")))
    }

    fn seconds(value: Option<f32>) -> Self {
        value.map_or_else(Cell::blank, |value| Cell::number(value as f64, clock(value)))
    }

    fn with_swatch(self, swatch: Option<Color>) -> Self {
        Cell { swatch, ..self }
    }
}

/// Minutes and seconds.
pub fn clock(seconds: f32) -> String {
    let seconds = seconds.max(0.0).round() as u32;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub label: String,
    /// Right-aligned, and sorted largest first on the first click.
    pub numeric: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    /// `columns` are lang keys under `editor.rounds.column`; a leading `#` marks a numeric one.
    fn with_columns(columns: &[&str]) -> Self {
        let columns = columns
            .iter()
            .map(|key| match key.strip_prefix('#') {
                Some(key) => Column { label: get!(format!("editor.rounds.column.{key}")), numeric: true },
                None => Column { label: get!(format!("editor.rounds.column.{key}")), numeric: false },
            })
            .collect();
        Table { columns, rows: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TableKind {
    Players,
    Weapons,
    Duels,
    Bots,
    Kills,
    Pickups,
}

impl TableKind {
    pub const ALL: [TableKind; 6] = [TableKind::Players, TableKind::Weapons, TableKind::Duels, TableKind::Bots, TableKind::Kills, TableKind::Pickups];

    pub fn name(self) -> String {
        match self {
            TableKind::Players => get!("editor.rounds.table.players"),
            TableKind::Weapons => get!("editor.rounds.table.weapons"),
            TableKind::Duels => get!("editor.rounds.table.duels"),
            TableKind::Bots => get!("editor.rounds.table.bots"),
            TableKind::Kills => get!("editor.rounds.table.kills"),
            TableKind::Pickups => get!("editor.rounds.table.pickups"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub map: String,
    pub round: u32,
    /// As written, RFC 3339.
    pub started: String,
    /// Game seconds from the round's start to its last event.
    pub length: f32,
    /// Has its `round_end`.
    pub finished: bool,
    pub people: u32,
    pub bots: u32,
    pub kills: u32,
    pub winner: Option<(String, Option<Color>)>,
    pub bot_skill: Option<BotSkill>,
    pub frag_limit: Option<u32>,
    pub time_limit: Option<f32>,
    /// Lines that did not parse.
    pub skipped: u32,
}

impl Summary {
    /// `2026-11-18 16:38`, or the text as written if it is not RFC 3339.
    pub fn started_text(&self) -> String {
        chrono::DateTime::parse_from_rfc3339(&self.started).map_or_else(|_| self.started.clone(), |at| at.format("%Y-%m-%d %H:%M").to_string())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoundTables {
    pub summary: Summary,
    pub tables: Vec<(TableKind, Table)>,
}

impl RoundTables {
    pub fn table(&self, kind: TableKind) -> Option<&Table> {
        self.tables.iter().find(|(of, _)| *of == kind).map(|(_, table)| table)
    }

    /// A round seen only as its final standings: a client's, which writes no file.
    pub fn of_standings(standings: &[Standing], length: f32) -> Self {
        let mut table = Table::with_columns(&["#place", "player", "#frags", "#kills", "#deaths", "#assists", "#dealt", "#taken", "#healing"]);
        for (place, standing) in standings.iter().enumerate() {
            table.rows.push(vec![
                Cell::count(place as u32 + 1),
                Cell::text(standing.display_name()).with_swatch(Some(standing.swatch())),
                Cell::number(standing.frags as f64, standing.frags.to_string()),
                Cell::count(standing.kills),
                Cell::count(standing.deaths),
                Cell::count(standing.assists),
                Cell::count(standing.damage_dealt),
                Cell::count(standing.damage_taken),
                Cell::count(standing.healing),
            ]);
        }
        let summary = Summary {
            length,
            finished: true,
            people: standings.len() as u32,
            kills: standings.iter().map(|standing| standing.kills).sum(),
            winner: standings.first().map(|standing| (standing.display_name(), Some(standing.swatch()))),
            ..default()
        };
        RoundTables { summary, tables: vec![(TableKind::Players, table)] }
    }
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Line {
    RoundStart {
        round: u32,
        map: String,
        started: String,
        bot_skill: Option<BotSkill>,
        time_limit: Option<f32>,
        frag_limit: Option<u32>,
    },
    Player {
        t: f32,
        id: u64,
        name: String,
        bot: bool,
        temperament: Option<serde_json::Value>,
    },
    Spawn {
        t: f32,
        id: u64,
    },
    Damage {
        t: f32,
        target: Option<u64>,
        by: Option<u64>,
        amount: u32,
        weapon: Option<String>,
    },
    Kill(KillLine),
    Pickup {
        t: f32,
        by: Option<u64>,
        what: String,
    },
    Goal {
        t: f32,
        id: u64,
        retreat: bool,
    },
    Sight {
        t: f32,
        by: u64,
        distance: f32,
    },
    Sample {
        t: f32,
        bodies: Vec<SampleBody>,
    },
    RoundEnd {
        t: f32,
        scoreboard: Vec<Standing>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct KillLine {
    t: f32,
    victim: PartyLine,
    killer: Option<PartyLine>,
    assist: Option<PartyLine>,
    weapon: Option<String>,
    how: How,
    distance: Option<f32>,
    life: Option<f32>,
}

#[derive(Deserialize)]
struct PartyLine {
    id: u64,
    name: String,
    health: u32,
}

#[derive(Deserialize)]
struct SampleBody {
    id: u64,
    weapon: Option<String>,
    want: Option<String>,
}

#[derive(Default)]
struct Person {
    name: String,
    bot: bool,
    reaction: Option<f32>,
    aim_error: Option<f32>,
    /// Game seconds at the first line naming them.
    joined: f32,
    kills: u32,
    deaths: u32,
    /// By their own hand or the map's.
    suicides: u32,
    assists: u32,
    dealt: u32,
    taken: u32,
    lives: Vec<f32>,
    spawned: Option<f32>,
    kill_distances: Vec<f32>,
    pickups: u32,
    weapon_kills: HashMap<String, u32>,
    /// Once-a-second samples alive, and how many of them had each want.
    samples: u32,
    wants: HashMap<String, u32>,
    goals: u32,
    retreats: u32,
    sightings: u32,
    sight_distance: f32,
}

impl Person {
    fn frags(&self) -> i32 {
        self.kills as i32 - self.suicides as i32
    }
}

#[derive(Default)]
struct WeaponTally {
    kills: u32,
    hits: u32,
    damage: u32,
    distances: Vec<f32>,
    /// Samples it was held in, at one a second.
    held: u32,
}

/// `weapon_names` maps a weapon's key to its lang key; a key not in it is shown as written.
pub fn read(text: &str, weapon_names: &HashMap<String, String>) -> RoundTables {
    let mut summary = Summary::default();
    let mut people: BTreeMap<u64, Person> = BTreeMap::new();
    let mut weapons: BTreeMap<String, WeaponTally> = BTreeMap::new();
    let mut duels: HashMap<(u64, u64), u32> = HashMap::new();
    let mut kills: Vec<KillLine> = Vec::new();
    let mut pickups: BTreeMap<String, Vec<(f32, Option<u64>)>> = BTreeMap::new();
    let mut scoreboard: Vec<Standing> = Vec::new();
    let mut last = 0.0f32;

    fn person(people: &mut BTreeMap<u64, Person>, id: u64, t: f32) -> &mut Person {
        people.entry(id).or_insert_with(|| Person { name: format!("Player {id}"), joined: t, ..default() })
    }

    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(line) = serde_json::from_str::<Line>(line) else {
            summary.skipped += 1;
            continue;
        };
        match line {
            Line::RoundStart { round, map, started, bot_skill, time_limit, frag_limit } => {
                summary = Summary { map, round, started, bot_skill, time_limit, frag_limit, skipped: summary.skipped, ..default() };
            }
            Line::Player { t, id, name, bot, temperament } => {
                last = last.max(t);
                let who = person(&mut people, id, t);
                who.name = name;
                who.bot = bot;
                let trait_of = |pointer: &str| temperament.as_ref().and_then(|value| value.pointer(pointer)).and_then(serde_json::Value::as_f64).map(|value| value as f32);
                who.reaction = trait_of("/mind/reaction");
                who.aim_error = trait_of("/mind/aim/error");
            }
            Line::Spawn { t, id } => {
                last = last.max(t);
                person(&mut people, id, t).spawned = Some(t);
            }
            Line::Damage { t, target, by, amount, weapon } => {
                last = last.max(t);
                let Some(target) = target else { continue };
                person(&mut people, target, t).taken += amount;
                if let Some(by) = by.filter(|by| *by != target) {
                    person(&mut people, by, t).dealt += amount;
                    if let Some(weapon) = weapon {
                        let tally = weapons.entry(weapon).or_default();
                        tally.hits += 1;
                        tally.damage += amount;
                    }
                }
            }
            Line::Kill(kill) => {
                last = last.max(kill.t);
                let victim = person(&mut people, kill.victim.id, kill.t);
                victim.deaths += 1;
                victim.spawned = None;
                if let Some(life) = kill.life {
                    victim.lives.push(life);
                }
                match &kill.killer {
                    Some(killer) if killer.id != kill.victim.id => {
                        *duels.entry((killer.id, kill.victim.id)).or_default() += 1;
                        let by = person(&mut people, killer.id, kill.t);
                        by.kills += 1;
                        by.kill_distances.extend(kill.distance);
                        if let Some(weapon) = &kill.weapon {
                            *by.weapon_kills.entry(weapon.clone()).or_default() += 1;
                            let tally = weapons.entry(weapon.clone()).or_default();
                            tally.kills += 1;
                            tally.distances.extend(kill.distance);
                        }
                    }
                    _ => person(&mut people, kill.victim.id, kill.t).suicides += 1,
                }
                if let Some(assist) = &kill.assist
                    && assist.id != kill.victim.id
                    && kill.killer.as_ref().is_none_or(|killer| killer.id != assist.id)
                {
                    person(&mut people, assist.id, kill.t).assists += 1;
                }
                kills.push(kill);
            }
            Line::Pickup { t, by, what } => {
                last = last.max(t);
                if let Some(by) = by {
                    person(&mut people, by, t).pickups += 1;
                }
                pickups.entry(what).or_default().push((t, by));
            }
            Line::Goal { t, id, retreat } => {
                last = last.max(t);
                let who = person(&mut people, id, t);
                who.goals += 1;
                who.retreats += retreat as u32;
            }
            Line::Sight { t, by, distance } => {
                last = last.max(t);
                let who = person(&mut people, by, t);
                who.sightings += 1;
                who.sight_distance += distance;
            }
            Line::Sample { t, bodies } => {
                last = last.max(t);
                for body in bodies {
                    let who = person(&mut people, body.id, t);
                    who.samples += 1;
                    if let Some(want) = body.want {
                        *who.wants.entry(want).or_default() += 1;
                    }
                    if let Some(weapon) = body.weapon {
                        weapons.entry(weapon).or_default().held += 1;
                    }
                }
            }
            Line::RoundEnd { t, scoreboard: board } => {
                last = last.max(t);
                summary.finished = true;
                scoreboard = board;
            }
            Line::Other => {}
        }
    }
    summary.length = last;

    let standing = |id: u64| scoreboard.iter().find(|standing| standing.id.0 == id);
    let swatch = |id: u64| standing(id).map(Standing::swatch);
    let weapon_name = |key: &str| weapon_names.get(key).map_or_else(|| key.to_owned(), |name_key| get!(name_key));
    let name_of = |id: u64, people: &BTreeMap<u64, Person>| people.get(&id).map_or_else(|| format!("Player {id}"), |who| who.name.clone());
    let player_cell = |id: u64, people: &BTreeMap<u64, Person>| Cell::text(name_of(id, people)).with_swatch(swatch(id));

    // As the scoreboard ranks: frags, then fewest deaths, then id.
    let mut order: Vec<u64> = people.keys().copied().collect();
    order.sort_by_key(|id| {
        let who = &people[id];
        (std::cmp::Reverse(who.frags()), who.deaths, *id)
    });

    summary.people = people.values().filter(|who| !who.bot).count() as u32;
    summary.bots = people.values().filter(|who| who.bot).count() as u32;
    summary.kills = kills.len() as u32;
    summary.winner = match scoreboard.is_empty() {
        false => ranked(&scoreboard).first().map(|standing| (standing.display_name(), Some(standing.swatch()))),
        true => order.first().map(|id| (name_of(*id, &people), None)),
    };

    let mut players = Table::with_columns(&[
        "#place", "player", "#frags", "#kills", "#deaths", "#assists", "#suicides", "#kd", "#dealt", "#taken", "#healing", "#dealt_per_minute", "#mean_life",
        "#longest_life", "#kill_distance", "#pickups", "best_weapon",
    ]);
    for (place, id) in order.iter().enumerate() {
        let who = &people[id];
        let minutes = ((last - who.joined) / 60.0).max(1.0 / 60.0);
        let longest = who.lives.iter().copied().chain(who.spawned.map(|spawned| last - spawned)).fold(None, |most: Option<f32>, life| Some(most.map_or(life, |most| most.max(life))));
        let best = who.weapon_kills.iter().max_by_key(|(key, kills)| (**kills, std::cmp::Reverse((*key).clone())));
        players.rows.push(vec![
            Cell::count(place as u32 + 1),
            player_cell(*id, &people),
            Cell::number(who.frags() as f64, who.frags().to_string()),
            Cell::count(who.kills),
            Cell::count(who.deaths),
            Cell::count(who.assists),
            Cell::count(who.suicides),
            Cell::decimal(who.kills as f64 / who.deaths.max(1) as f64, 2),
            Cell::count(who.dealt),
            Cell::count(who.taken),
            standing(*id).map_or_else(Cell::blank, |standing| Cell::count(standing.healing)),
            Cell::decimal(who.dealt as f64 / minutes as f64, 0),
            Cell::seconds(mean(&who.lives)),
            Cell::seconds(longest),
            Cell::meters(mean(&who.kill_distances)),
            Cell::count(who.pickups),
            best.map_or_else(Cell::blank, |(key, kills)| Cell::text(format!("{} ({kills})", weapon_name(key)))),
        ]);
    }

    let held_total: u32 = weapons.values().map(|tally| tally.held).sum();
    let mut weapon_table = Table::with_columns(&[
        "weapon", "#kills", "#kill_share", "#hits", "#damage", "#damage_per_hit", "#kill_distance", "#longest_kill", "#time_held", "#held_share", "#kills_per_minute_held",
    ]);
    for (key, tally) in &weapons {
        let held_minutes = tally.held as f64 / 60.0;
        weapon_table.rows.push(vec![
            Cell::text(weapon_name(key)),
            Cell::count(tally.kills),
            Cell::percent(tally.kills as f64 / summary.kills.max(1) as f64),
            Cell::count(tally.hits),
            Cell::count(tally.damage),
            match tally.hits {
                0 => Cell::blank(),
                hits => Cell::decimal(tally.damage as f64 / hits as f64, 1),
            },
            Cell::meters(mean(&tally.distances)),
            Cell::meters(tally.distances.iter().copied().reduce(f32::max)),
            Cell::seconds(Some(tally.held as f32)),
            Cell::percent(tally.held as f64 / held_total.max(1) as f64),
            match tally.held {
                0 => Cell::blank(),
                _ => Cell::decimal(tally.kills as f64 / held_minutes, 2),
            },
        ]);
    }
    weapon_table.rows.sort_by(|a, b| b[1].sort.compare(&a[1].sort));

    // Killers down the side, victims across the top, both in standings order.
    let mut duel_table = Table { columns: vec![Column { label: get!("editor.rounds.column.killer"), numeric: false }], rows: Vec::new() };
    for (place, _) in order.iter().enumerate() {
        duel_table.columns.push(Column { label: format!("#{}", place + 1), numeric: true });
    }
    duel_table.columns.push(Column { label: get!("editor.rounds.column.kills"), numeric: true });
    for (place, killer) in order.iter().enumerate() {
        let mut row = vec![Cell { text: format!("#{} {}", place + 1, name_of(*killer, &people)), ..player_cell(*killer, &people) }];
        for victim in &order {
            row.push(match duels.get(&(*killer, *victim)) {
                Some(count) => Cell::count(*count),
                None if killer == victim => Cell::blank(),
                None => Cell::number(0.0, String::new()),
            });
        }
        row.push(Cell::count(people[killer].kills));
        duel_table.rows.push(row);
    }

    let mut want_columns: Vec<String> = vec!["bot".to_owned()];
    want_columns.extend(Want::ALL.map(|want| format!("#want_{}", crate::game::telemetry::want_name(want))));
    want_columns.extend(["#goal_changes_per_minute", "#retreats", "#sightings", "#sight_distance", "#reaction", "#aim_error"].map(str::to_owned));
    let mut bot_table = Table::with_columns(&want_columns.iter().map(String::as_str).collect::<Vec<_>>());
    for id in order.iter().filter(|id| people[*id].bot) {
        let who = &people[id];
        let minutes = ((last - who.joined) / 60.0).max(1.0 / 60.0);
        let mut row = vec![player_cell(*id, &people)];
        for want in Want::ALL {
            let count = who.wants.get(&crate::game::telemetry::want_name(want)).copied().unwrap_or(0);
            row.push(Cell::percent(count as f64 / who.samples.max(1) as f64));
        }
        row.push(Cell::decimal(who.goals as f64 / minutes as f64, 1));
        row.push(Cell::count(who.retreats));
        row.push(Cell::count(who.sightings));
        row.push(match who.sightings {
            0 => Cell::blank(),
            sightings => Cell::meters(Some(who.sight_distance / sightings as f32)),
        });
        row.push(who.reaction.map_or_else(Cell::blank, |reaction| Cell::number(reaction as f64, format!("{:.0} ms", reaction * 1000.0))));
        row.push(who.aim_error.map_or_else(Cell::blank, |error| Cell::decimal(error as f64, 3)));
        bot_table.rows.push(row);
    }

    let mut kill_table = Table::with_columns(&["#time", "killer", "weapon", "how", "victim", "assist", "#distance", "#victim_life", "#killer_health"]);
    for kill in &kills {
        let suicide = kill.killer.as_ref().is_none_or(|killer| killer.id == kill.victim.id);
        kill_table.rows.push(vec![
            Cell::seconds(Some(kill.t)),
            match &kill.killer {
                Some(killer) if !suicide => Cell::text(killer.name.clone()).with_swatch(swatch(killer.id)),
                Some(_) => Cell::text(get!("editor.rounds.self")),
                None => Cell::text(get!("editor.rounds.world")),
            },
            kill.weapon.as_deref().map_or_else(Cell::blank, |key| Cell::text(weapon_name(key))),
            Cell::text(how_name(kill.how)),
            Cell::text(kill.victim.name.clone()).with_swatch(swatch(kill.victim.id)),
            kill.assist.as_ref().map_or_else(Cell::blank, |assist| Cell::text(assist.name.clone()).with_swatch(swatch(assist.id))),
            Cell::meters(kill.distance),
            Cell::seconds(kill.life),
            match &kill.killer {
                Some(killer) if !suicide => Cell::count(killer.health),
                _ => Cell::blank(),
            },
        ]);
    }

    let mut pickup_table = Table::with_columns(&["item", "#taken", "#takers", "#first_taken", "#mean_gap", "most_taken_by"]);
    for (what, takes) in &pickups {
        let takers: HashSet<u64> = takes.iter().filter_map(|(_, by)| *by).collect();
        let gaps: Vec<f32> = takes.windows(2).map(|pair| pair[1].0 - pair[0].0).collect();
        let mut by_count: BTreeMap<u64, u32> = BTreeMap::new();
        for by in takes.iter().filter_map(|(_, by)| *by) {
            *by_count.entry(by).or_default() += 1;
        }
        let most = by_count.iter().max_by_key(|(id, count)| (**count, std::cmp::Reverse(**id)));
        pickup_table.rows.push(vec![
            Cell::text(weapon_names.get(what).map_or_else(|| what.clone(), |name_key| get!(name_key))),
            Cell::count(takes.len() as u32),
            Cell::count(takers.len() as u32),
            Cell::seconds(takes.first().map(|(t, _)| *t)),
            Cell::seconds(mean(&gaps)),
            most.map_or_else(Cell::blank, |(id, count)| Cell { text: format!("{} ({count})", name_of(*id, &people)), ..player_cell(*id, &people) }),
        ]);
    }

    RoundTables {
        summary,
        tables: vec![
            (TableKind::Players, players),
            (TableKind::Weapons, weapon_table),
            (TableKind::Duels, duel_table),
            (TableKind::Bots, bot_table),
            (TableKind::Kills, kill_table),
            (TableKind::Pickups, pickup_table),
        ],
    }
}

fn mean(values: &[f32]) -> Option<f32> {
    (!values.is_empty()).then(|| values.iter().sum::<f32>() / values.len() as f32)
}

fn how_name(how: How) -> String {
    match how {
        How::Other => get!("editor.rounds.how.other"),
        How::Hitscan => get!("editor.rounds.how.hitscan"),
        How::Direct => get!("editor.rounds.how.direct"),
        How::Splash => get!("editor.rounds.how.splash"),
        How::Flame => get!("editor.rounds.how.flame"),
        How::Burn => get!("editor.rounds.how.burn"),
        How::Trigger => get!("editor.rounds.how.trigger"),
        How::Kill => get!("editor.rounds.how.kill"),
        How::Exploded => get!("editor.rounds.how.exploded"),
    }
}

impl Sort {
    /// Numbers before text; a NaN never reaches here.
    pub fn compare(&self, other: &Sort) -> std::cmp::Ordering {
        match (self, other) {
            (Sort::Number(a), Sort::Number(b)) => a.total_cmp(b),
            (Sort::Text(a), Sort::Text(b)) => a.cmp(b),
            (Sort::Number(_), Sort::Text(_)) => std::cmp::Ordering::Less,
            (Sort::Text(_), Sort::Number(_)) => std::cmp::Ordering::Greater,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUND: &str = r#"{"event":"round_start","t":0.0,"round":3,"map":"arena","started":"2026-11-18T16:38:54.137051-06:00","slots":8,"fill_with_bots":true,"bot_skill":"normal","time_limit":null,"frag_limit":20}
{"event":"player","t":0.1,"id":1,"name":"Ann","bot":true,"temperament":{"mind":{"reaction":0.25,"aim":{"error":0.1}}}}
{"event":"player","t":0.1,"id":2,"name":"Bo","bot":false,"temperament":null}
{"event":"spawn","t":0.1,"id":1,"at":[0,0,0],"yaw":0}
{"event":"spawn","t":0.1,"id":2,"at":[0,0,0],"yaw":0}
{"event":"sample","t":1.0,"bodies":[{"id":1,"at":[0,0,0],"yaw":0,"health":125,"armor":0,"weapon":"pistol","want":"hunt"},{"id":2,"at":[0,0,0],"yaw":0,"health":125,"armor":0,"weapon":"pistol","want":null}]}
{"event":"damage","t":2.0,"target":2,"by":1,"amount":60,"remaining":65,"weapon":"pistol","how":"Hitscan","reflected":false,"at":[0,0,0]}
{"event":"damage","t":2.5,"target":2,"by":1,"amount":65,"remaining":0,"weapon":"pistol","how":"Hitscan","reflected":false,"at":[0,0,0]}
{"event":"kill","t":2.5,"victim":{"id":2,"name":"Bo","health":65,"armor":0,"at":[0,0,0],"wants":null},"killer":{"id":1,"name":"Ann","health":125,"armor":0,"at":[4,0,0],"wants":null},"assist":null,"weapon":"pistol","how":"Hitscan","reflected":false,"distance":4.0,"life":2.4,"damage":[]}
{"event":"something_from_a_newer_build","t":3.0}
not json at all
{"event":"kill","t":5.0,"victim":{"id":1,"name":"Ann","health":10,"armor":0,"at":[0,0,0],"wants":null},"killer":null,"assist":{"id":2,"name":"Bo","health":125,"armor":0,"at":[0,0,0],"wants":null},"weapon":null,"how":"Trigger","reflected":false,"distance":null,"life":4.9,"damage":[]}
{"event":"pickup","t":6.0,"by":2,"what":"health_small","at":[0,0,0]}
{"event":"round_end","t":10.0,"scoreboard":[]}
"#;

    fn row<'a>(table: &'a Table, name: &str) -> &'a [Cell] {
        table.rows.iter().find(|row| row.iter().any(|cell| cell.text == name)).expect("no such row")
    }

    fn column(table: &Table, key: &str) -> usize {
        let label = get!(format!("editor.rounds.column.{key}"));
        table.columns.iter().position(|column| column.label == label).expect("no such column")
    }

    /// Breaks if a kill, a death to the map, an assist or damage is counted against the wrong
    /// person, or a line the reader does not know stops it.
    #[test]
    fn a_round_file_reads_into_tables_that_agree() {
        let tables = read(ROUND, &HashMap::new());
        let summary = &tables.summary;
        assert_eq!((summary.round, summary.map.as_str(), summary.kills, summary.people, summary.bots), (3, "arena", 2, 1, 1));
        assert!(summary.finished);
        assert_eq!(summary.length, 10.0);
        assert_eq!(summary.skipped, 1, "the unknown event is passed over quietly and the garbage counted");

        let players = tables.table(TableKind::Players).unwrap();
        let ann = row(players, "Ann");
        assert_eq!(ann[column(players, "kills")].text, "1");
        assert_eq!(ann[column(players, "suicides")].text, "1", "a death to the map costs a frag");
        assert_eq!(ann[column(players, "frags")].text, "0");
        assert_eq!(ann[column(players, "dealt")].text, "125");
        let bo = row(players, "Bo");
        assert_eq!(bo[column(players, "assists")].text, "1");
        assert_eq!(bo[column(players, "taken")].text, "125");
        assert_eq!(bo[column(players, "pickups")].text, "1");

        let weapons = tables.table(TableKind::Weapons).unwrap();
        let pistol = row(weapons, "pistol");
        assert_eq!(pistol[column(weapons, "hits")].text, "2");
        assert_eq!(pistol[column(weapons, "kill_share")].text, "50%");

        let duels = tables.table(TableKind::Duels).unwrap();
        let total: f64 = duels.rows.iter().flat_map(|row| &row[1..row.len() - 1]).map(|cell| match cell.sort {
            Sort::Number(value) if value.is_finite() => value,
            _ => 0.0,
        }).sum();
        assert_eq!(total, 1.0, "a death to the map is nobody's duel");

        let bots = tables.table(TableKind::Bots).unwrap();
        assert_eq!(bots.rows.len(), 1);
        assert_eq!(row(bots, "Ann")[column(bots, "want_hunt")].text, "100%");
        assert_eq!(row(bots, "Ann")[column(bots, "reaction")].text, "250 ms");

        assert_eq!(tables.table(TableKind::Kills).unwrap().rows.len(), 2);
    }

    /// Breaks if a file cut off mid-round stops reading, or reads as finished.
    #[test]
    fn a_file_cut_short_reads_as_far_as_it_goes() {
        let cut = &ROUND[..ROUND.find("{\"event\":\"pickup\"").unwrap()];
        let tables = read(cut, &HashMap::new());
        assert!(!tables.summary.finished);
        assert_eq!(tables.summary.kills, 2);
        assert_eq!(tables.summary.length, 5.0);
    }
}
