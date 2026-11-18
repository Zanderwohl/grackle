//! Bakes a blueprint's nav graph and fields without a window and prints the map's statistics, as
//! the Bakes panel shows them, with a top-down chart of each field. For tuning a map from a
//! terminal. Run from the repo root:
//!
//! ```text
//! cargo run --bin map_stats -- assets/default/blueprints/arena.gmb
//! ```
//!
//! Mirrors are not resolved here, so a map built from one reports only its originals.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::prelude::{IVec2, Vec3};
use grackle::common::game_rules::Modes;
use grackle::common::trigger::{TriggerEffect, TriggerVolume};
use grackle::common::weapon::{WeaponCatalog, WeaponId};
use grackle::common::weapon_file;
use grackle::editor::save;
use grackle::game::collision::CollisionWorld;
use grackle::game::nav::{self, CellId, NavGraph};
use grackle::game::nav_derived::{self, RANGE_BANDS};
use grackle::game::nav_field::{self, Field, Pickup};
use grackle::game::nav_rating::{MapStatistics, Rating};

/// Meters of floor per character of a chart.
const CHART_METERS: f32 = 1.0;

fn main() {
    let path = PathBuf::from(std::env::args().nth(1).expect("usage: map_stats PATH.gmb"));
    let loaded = save::load(&path).unwrap_or_else(|error| panic!("{path:?}: {error}"));
    let mut timeline = loaded.timeline;
    timeline.resolve_all();
    let rules = loaded.metadata.movement.clone();

    let mut volumes = Vec::new();
    let mut stairs = Vec::new();
    let mut triggers = Vec::new();
    let mut spawns = Vec::new();
    let mut pickups = Vec::new();
    for (_, feature) in timeline.active_features() {
        let object = feature.object();
        volumes.extend(object.volumes());
        stairs.extend(object.stairs());
        stairs.extend(object.standing_copies().iter().filter_map(|copy| copy.object.stairs()));
        let value = serde_json::to_value(object).unwrap_or_default();
        let inner = value.as_object().and_then(|o| o.values().next()).cloned().unwrap_or_default();
        match object.type_key() {
            "spawn_point" => spawns.push(object.get_point("").unwrap()),
            "trigger" => {
                let (min, max) = object.drag_handle_bounds().unwrap();
                let effect = match inner.get("effect") {
                    Some(serde_json::Value::String(kill)) if kill == "Kill" => TriggerEffect::Kill,
                    Some(effect) => serde_json::from_value(effect.clone()).unwrap_or_default(),
                    None => TriggerEffect::default(),
                };
                triggers.push(TriggerVolume { min, max, effect });
            }
            "weapon_pickup" => {
                let label = inner.get("weapon").and_then(|w| w.as_str()).unwrap_or("weapon").to_owned();
                let id = WeaponId::of(&label);
                pickups.push(Pickup::weapon(label, object.get_point("").unwrap()).giving(id));
            }
            "prop" => {
                let label = inner.get("model").and_then(|w| w.as_str()).unwrap_or("prop").to_owned();
                pickups.push(Pickup::prop(label, object.get_point("").unwrap()));
            }
            _ => {}
        }
    }

    let mut world = CollisionWorld::default();
    let mut faces = world.rebuild_volumes(&volumes);
    grackle::common::stairs::add_to_world(&mut world, &mut faces, &stairs);
    let mut graph = nav::graph(&world, &faces, &triggers, &rules, &spawns);
    graph.set_fields(nav_field::bake(&world, &graph, &triggers, &rules, pickups.clone(), spawns.clone()));

    let catalog = weapon_file::default_catalog();
    let starting = nav_derived::starting_weapons(&catalog, &Modes::shipped().of(loaded.metadata.game_mode));
    report(&graph, &spawns, &pickups);
    first_weapons(&graph, &catalog, &starting);
}

fn report(graph: &NavGraph, spawns: &[Vec3], pickups: &[Pickup]) {
    let statistics = MapStatistics::of(graph);
    println!("floor {:.0} m², rise {:.1} m, {} cells", statistics.floor, statistics.rise, graph.cells().len());
    for (rating, share) in Rating::ALL.iter().zip(&statistics.shares) {
        println!("  {:<10} {:>5.1}%", format!("{rating:?}"), share.unwrap_or(0.0) * 100.0);
    }
    for field in [Field::Exposure, Field::Sight, Field::Traffic, Field::Cover, Field::Knockback, Field::RangeMedian, Field::Escape] {
        let all: Vec<f32> = (0..graph.cells().len() as CellId).filter_map(|id| graph.field(id, field)).collect();
        let mut sorted: Vec<f32> = all.iter().copied().filter(|value| value.is_finite()).collect();
        let mean = sorted.iter().sum::<f32>() / sorted.len().max(1) as f32;
        sorted.sort_by(f32::total_cmp);
        let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
        let unbounded = all.len() - sorted.len();
        let none = if unbounded > 0 { format!("  none {:.1}%", unbounded as f32 / all.len() as f32 * 100.0) } else { String::new() };
        println!("  {:<11} mean {:>7.2}  median {:>7.2}  max {:>7.2}{none}", format!("{field:?}"), mean, median, sorted.last().copied().unwrap_or(0.0));
    }

    println!("\nengagement range, floor by median band");
    let lows = std::iter::once(0.0).chain(RANGE_BANDS);
    let bands: Vec<String> = lows.zip(RANGE_BANDS.map(Some).into_iter().chain([None])).map(|(low, high)| match high {
        Some(high) => format!("{low:.0}-{high:.0} m"),
        None => format!("{low:.0}+ m"),
    }).collect();
    for (band, share) in bands.iter().zip(statistics.range_bands.unwrap_or_default()) {
        println!("  {band:<10} {:>5.1}%", share * 100.0);
    }
    for feet in spawns {
        let at = graph.cell_at(*feet);
        let exposure = at.and_then(|id| graph.field(id, Field::Exposure)).unwrap_or(f32::NAN);
        println!("  spawn {:>6.1} {:>5.1} {:>6.1}  exposure {:>6.1}{}", feet.x, feet.y, feet.z, exposure, if at.is_none() { "  OFF THE GRAPH" } else { "" });
    }
    for pickup in pickups {
        let at = graph.cell_at(pickup.feet);
        let exposure = at.and_then(|id| graph.field(id, Field::Exposure)).unwrap_or(f32::NAN);
        println!("  {:<16} {:>6.1} {:>5.1} {:>6.1}  exposure {:>6.1}{}", pickup.label, pickup.feet.x, pickup.feet.y, pickup.feet.z, exposure, if at.is_none() { "  OFF THE GRAPH" } else { "" });
    }

    chart(graph, "height of the top floor, m (0-9, a+ above)", |graph, id| {
        let y = graph.cells()[id as usize].feet.y;
        Some(if y < 0.0 { '_' } else { char::from_digit((y.round() as u32).min(35), 36).unwrap() })
    });
    chart(graph, "exposure, m² (0 <25, 1 <50, ... 9 >=400, # >=600)", |graph, id| {
        let exposure = graph.field(id, Field::Exposure)?;
        Some(if exposure >= 600.0 { '#' } else { char::from_digit(((exposure / 400.0 * 9.0) as u32).min(9), 10).unwrap() })
    });
    chart(graph, "traffic share (0-9 tenths)", |graph, id| {
        let traffic = graph.field(id, Field::Traffic)?;
        Some(char::from_digit(((traffic * 10.0) as u32).min(9), 10).unwrap())
    });
    chart(graph, "median engagement range, m (0 <5, 1 <10, ... 9 >=45; . sees nothing)", |graph, id| {
        let range = graph.field(id, Field::RangeMedian)?;
        Some(if range <= 0.0 { '.' } else { char::from_digit(((range / 5.0) as u32).min(9), 10).unwrap() })
    });
    chart(graph, "escape out of sight, in half seconds (0 <0.5 s, 1 <1 s, ...; # none within reach)", |graph, id| {
        let escape = graph.field(id, Field::Escape)?;
        Some(if escape.is_finite() { char::from_digit(((escape / 0.5) as u32).min(9), 10).unwrap() } else { '#' })
    });
    chart(graph, "rating: c cramped, o open, G gauntlet, b busy, v vantage, A ambush, X death trap, ! precarious", |graph, id| {
        let holds = |rating: Rating| rating.holds(graph, id).unwrap_or(false);
        Some(if holds(Rating::Precarious) {
            '!'
        } else if holds(Rating::DeathTrap) {
            'X'
        } else if holds(Rating::Gauntlet) {
            'G'
        } else if holds(Rating::Ambush) {
            'A'
        } else if holds(Rating::Vantage) {
            'v'
        } else if holds(Rating::Busy) {
            'b'
        } else if holds(Rating::Open) {
            'o'
        } else if holds(Rating::Cramped) {
            'c'
        } else {
            '.'
        })
    });
}

/// One character per [`CHART_METERS`] square, from the highest cell in it; north (-Z) up.
fn chart(graph: &NavGraph, title: &str, glyph: impl Fn(&NavGraph, CellId) -> Option<char>) {
    let mut top: HashMap<IVec2, CellId> = HashMap::new();
    for (id, cell) in graph.cells().iter().enumerate() {
        let square = IVec2::new((cell.feet.x / CHART_METERS).floor() as i32, (cell.feet.z / CHART_METERS).floor() as i32);
        let entry = top.entry(square).or_insert(id as CellId);
        if graph.cells()[*entry as usize].feet.y < cell.feet.y {
            *entry = id as CellId;
        }
    }
    let Some(min) = top.keys().copied().reduce(IVec2::min) else { return };
    let max = top.keys().copied().reduce(IVec2::max).unwrap();
    println!("\n{title}   x {}..{}, z {}..{}", min.x, max.x, min.y, max.y);
    for z in min.y..=max.y {
        let row: String = (min.x..=max.x).map(|x| top.get(&IVec2::new(x, z)).and_then(|id| glyph(graph, *id)).unwrap_or(' ')).collect();
        println!("{z:>4} {row}");
    }
}

/// Per spawn, the nearest weapon it does not start with, and how exposed the walk there is.
fn first_weapons(graph: &NavGraph, catalog: &WeaponCatalog, starting: &[WeaponId]) {
    let names: Vec<String> = starting.iter().map(|id| catalog.get(*id).map_or_else(|| format!("{id:?}"), |weapon| weapon.key.clone())).collect();
    println!("\nfirst weapon per spawn, starting with {}: seconds, then exposure along the way, mean / most m²", names.join(", "));
    let rows = nav_derived::first_weapons(graph, starting);
    for row in &rows {
        let feet = row.spawn;
        match row.pickup.and_then(|pickup| graph.fields().pickups.get(pickup)) {
            Some(pickup) => println!(
                "  spawn {:>6.1} {:>5.1} {:>6.1}  {:<16} {:>5.1} s  {:>6.1} / {:>6.1}",
                feet.x, feet.y, feet.z, pickup.label, row.seconds, row.mean_exposure, row.most_exposure
            ),
            None => println!("  spawn {:>6.1} {:>5.1} {:>6.1}  none", feet.x, feet.y, feet.z),
        }
    }
    let reached: Vec<_> = rows.iter().filter(|row| row.pickup.is_some()).collect();
    if !reached.is_empty() {
        let mean = |value: fn(&&nav_derived::FirstWeapon) -> f32| reached.iter().map(value).sum::<f32>() / reached.len() as f32;
        println!("  mean {:>32.1} s  {:>6.1} / {:>6.1}", mean(|row| row.seconds), mean(|row| row.mean_exposure), mean(|row| row.most_exposure));
    }
}
