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
use grackle::common::movement_rules::MovementRules;
use grackle::common::trigger::{TriggerEffect, TriggerVolume};
use grackle::editor::save;
use grackle::game::collision::CollisionWorld;
use grackle::game::nav::{self, CellId, NavGraph};
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
    let mut triggers = Vec::new();
    let mut spawns = Vec::new();
    let mut pickups = Vec::new();
    for (_, feature) in timeline.active_features() {
        let object = feature.object();
        volumes.extend(object.volumes());
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
                pickups.push(Pickup::weapon(label, object.get_point("").unwrap()));
            }
            "prop" => {
                let label = inner.get("model").and_then(|w| w.as_str()).unwrap_or("prop").to_owned();
                pickups.push(Pickup::prop(label, object.get_point("").unwrap()));
            }
            _ => {}
        }
    }

    let mut world = CollisionWorld::default();
    let faces = world.rebuild_volumes(&volumes);
    let mut graph = nav::graph(&world, &faces, &triggers, &rules, &spawns);
    graph.set_fields(nav_field::bake(&world, &graph, &triggers, &rules, pickups.clone(), spawns.clone()));

    report(&graph, &spawns, &pickups, &rules);
}

fn report(graph: &NavGraph, spawns: &[Vec3], pickups: &[Pickup], _rules: &MovementRules) {
    let statistics = MapStatistics::of(graph);
    println!("floor {:.0} m², rise {:.1} m, {} cells", statistics.floor, statistics.rise, graph.cells().len());
    for (rating, share) in Rating::ALL.iter().zip(&statistics.shares) {
        println!("  {:<10} {:>5.1}%", format!("{rating:?}"), share.unwrap_or(0.0) * 100.0);
    }
    for field in [Field::Exposure, Field::Sight, Field::Traffic, Field::Cover, Field::Knockback] {
        let values: Vec<f32> = (0..graph.cells().len() as CellId).filter_map(|id| graph.field(id, field)).collect();
        let mean = values.iter().sum::<f32>() / values.len().max(1) as f32;
        let mut sorted = values.clone();
        sorted.sort_by(f32::total_cmp);
        let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
        println!("  {:<10} mean {:>7.2}  median {:>7.2}  max {:>7.2}", format!("{field:?}"), mean, median, sorted.last().copied().unwrap_or(0.0));
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
    chart(graph, "rating: c cramped, o open, G gauntlet, b busy, v vantage, A ambush, ! precarious", |graph, id| {
        let holds = |rating: Rating| rating.holds(graph, id).unwrap_or(false);
        Some(if holds(Rating::Precarious) {
            '!'
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
