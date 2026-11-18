//! The Rounds tab: the newest round files under [`ROUNDS_DIR`], read on the IO pool and listed
//! newest first, and the selected round as sortable tables
//! ([`round_file`](crate::game::round_file)). A client writes no files, so it also lists the
//! rounds it saw this session, as their final standings.
//!
//! Files are read again when a round ends and on Refresh; one whose size and modified time have
//! not changed is not. Native only: on wasm the list holds only this session's rounds.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::tasks::futures::check_ready;
use bevy::tasks::{IoTaskPool, Task};
use bevy_egui::egui;

use crate::common::app_mode::AppMode;
use crate::common::net::NetRole;
use crate::common::weapon::WeaponCatalog;
use crate::game::round_file::{RoundTables, Table, TableKind};
use crate::game::rounds::RoundHistory;
use crate::get;

/// Only the newest are read; older files stay on disk.
pub const LISTED_ROUNDS: usize = 100;

/// Modified time and length, which say whether a file must be read again.
type Stamp = (SystemTime, u64);

pub struct ArchivedRound {
    pub path: PathBuf,
    stamp: Stamp,
    pub read: Result<RoundTables, String>,
}

struct Scanned {
    /// Newest first.
    present: Vec<(PathBuf, Stamp)>,
    fresh: Vec<ArchivedRound>,
}

#[derive(Clone, Debug, PartialEq)]
enum Selected {
    File(PathBuf),
    /// [`Round::number`](crate::game::rounds::Round::number).
    Session(u32),
}

#[derive(Resource)]
pub struct RoundArchive {
    /// Newest first.
    rounds: Vec<ArchivedRound>,
    scan: Option<Task<Scanned>>,
    stale: bool,
    selected: Option<Selected>,
    table: TableKind,
    /// Per table: the column sorted by, and whether largest first.
    sort: HashMap<TableKind, (usize, bool)>,
}

impl Default for RoundArchive {
    fn default() -> Self {
        RoundArchive { rounds: Vec::new(), scan: None, stale: true, selected: None, table: TableKind::Players, sort: HashMap::new() }
    }
}

pub struct RoundArchivePlugin;

impl Plugin for RoundArchivePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoundArchive>()
            .add_systems(OnExit(AppMode::Play), |mut archive: ResMut<RoundArchive>| archive.stale = true)
            .add_systems(Update, read_round_files.run_if(in_state(AppMode::Editor)));
    }
}

/// A frame after leaving Play at the earliest, so telemetry has closed the round's file.
fn read_round_files(mut archive: ResMut<RoundArchive>, catalog: Option<Res<WeaponCatalog>>) {
    if let Some(task) = archive.scan.as_mut() {
        let Some(scanned) = check_ready(task) else { return };
        archive.scan = None;
        let mut old: HashMap<PathBuf, ArchivedRound> = archive.rounds.drain(..).map(|round| (round.path.clone(), round)).collect();
        let mut fresh: HashMap<PathBuf, ArchivedRound> = scanned.fresh.into_iter().map(|round| (round.path.clone(), round)).collect();
        archive.rounds = scanned.present.into_iter().filter_map(|(path, _)| fresh.remove(&path).or_else(|| old.remove(&path))).collect();
        return;
    }
    if !archive.stale {
        return;
    }
    archive.stale = false;
    let known: HashMap<PathBuf, Stamp> = archive.rounds.iter().map(|round| (round.path.clone(), round.stamp)).collect();
    let names: HashMap<String, String> =
        catalog.iter().flat_map(|catalog| catalog.weapons.iter()).map(|weapon| (weapon.key.clone(), weapon.name_key.clone())).collect();
    archive.scan = Some(IoTaskPool::get().spawn(async move { scan(&known, &names) }));
}

#[cfg(not(target_arch = "wasm32"))]
fn scan(known: &HashMap<PathBuf, Stamp>, names: &HashMap<String, String>) -> Scanned {
    use std::fs;
    let mut present: Vec<(PathBuf, Stamp)> = Vec::new();
    for map in fs::read_dir(crate::game::telemetry::ROUNDS_DIR).into_iter().flatten().flatten() {
        for file in fs::read_dir(map.path()).into_iter().flatten().flatten() {
            let path = file.path();
            if path.extension().is_none_or(|extension| extension != "jsonl") {
                continue;
            }
            let Ok(metadata) = file.metadata() else { continue };
            present.push((path, (metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH), metadata.len())));
        }
    }
    present.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| b.0.cmp(&a.0)));
    present.truncate(LISTED_ROUNDS);
    let fresh = present
        .iter()
        .filter(|(path, stamp)| known.get(path) != Some(stamp))
        .map(|(path, stamp)| ArchivedRound {
            path: path.clone(),
            stamp: *stamp,
            read: fs::read_to_string(path).map(|text| crate::game::round_file::read(&text, names)).map_err(|error| error.to_string()),
        })
        .collect();
    Scanned { present, fresh }
}

#[cfg(target_arch = "wasm32")]
fn scan(_: &HashMap<PathBuf, Stamp>, _: &HashMap<String, String>) -> Scanned {
    Scanned { present: Vec::new(), fresh: Vec::new() }
}

#[derive(SystemParam)]
pub struct RoundsTab<'w> {
    history: Res<'w, RoundHistory>,
    archive: ResMut<'w, RoundArchive>,
    role: Option<Res<'w, NetRole>>,
}

impl RoundsTab<'_> {
    pub fn view(&mut self) -> RoundsView<'_> {
        let client = self.role.as_deref().is_some_and(|role| !role.is_authority());
        RoundsView { history: &self.history, archive: self.archive.as_mut(), client }
    }
}

pub struct RoundsView<'a> {
    history: &'a RoundHistory,
    archive: &'a mut RoundArchive,
    client: bool,
}

const LIST_WIDTH: f32 = 240.0;

impl RoundsView<'_> {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        // This session's rounds are built each frame: a client's last one still settles.
        let session: Vec<(u32, RoundTables)> = match self.client {
            true => self.history.rounds().map(|round| (round.number, RoundTables::of_standings(&round.standings, round.length))).collect(),
            false => Vec::new(),
        };
        let archive = &mut *self.archive;
        if archive.selected.is_none() {
            archive.selected = session
                .first()
                .map(|(number, _)| Selected::Session(*number))
                .or_else(|| archive.rounds.first().map(|round| Selected::File(round.path.clone())));
        }

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(LIST_WIDTH, ui.available_height()), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(LIST_WIDTH);
                list_ui(ui, archive, &session);
            });
            ui.separator();
            ui.vertical(|ui| {
                let shown = match &archive.selected {
                    Some(Selected::Session(number)) => session.iter().find(|(of, _)| of == number).map(|(_, tables)| Ok(tables)),
                    Some(Selected::File(path)) => archive.rounds.iter().find(|round| &round.path == path).map(|round| match &round.read {
                        Ok(tables) => Ok(tables),
                        Err(error) => Err(get!("editor.rounds.unreadable", "path", round.path.display(), "error", error)),
                    }),
                    None => None,
                };
                match shown {
                    Some(Ok(tables)) => round_ui(ui, tables, &mut archive.table, &mut archive.sort),
                    Some(Err(error)) => {
                        ui.label(error);
                    }
                    None => {
                        ui.label(get!("editor.rounds.none"));
                    }
                }
            });
        });
    }
}

fn list_ui(ui: &mut egui::Ui, archive: &mut RoundArchive, session: &[(u32, RoundTables)]) {
    ui.horizontal(|ui| {
        if ui.add_enabled(archive.scan.is_none(), egui::Button::new(get!("editor.rounds.refresh"))).clicked() {
            archive.stale = true;
        }
        if archive.scan.is_some() {
            ui.spinner();
        }
    });
    egui::ScrollArea::vertical().id_salt("rounds_list").auto_shrink([false, false]).show(ui, |ui| {
        if !session.is_empty() {
            ui.strong(get!("editor.rounds.this_session"));
            for (number, tables) in session {
                let text = format!("{}\n{}", get!("editor.rounds.session_round", "number", number), detail_line(tables));
                let selected = archive.selected == Some(Selected::Session(*number));
                if ui.selectable_label(selected, text).clicked() {
                    archive.selected = Some(Selected::Session(*number));
                }
            }
            ui.strong(get!("editor.rounds.recorded"));
        }
        if archive.rounds.is_empty() {
            ui.label(match archive.scan.is_some() {
                true => get!("editor.rounds.reading"),
                false => get!("editor.rounds.none"),
            });
        }
        for round in &archive.rounds {
            let text = match &round.read {
                Ok(tables) => {
                    let summary = &tables.summary;
                    let mut first = format!("{} · {}", summary.map, summary.started_text());
                    if !summary.finished {
                        first.push_str(&format!(" ({})", get!("editor.rounds.unfinished")));
                    }
                    format!("{first}\n{}", detail_line(tables))
                }
                Err(_) => round.path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            };
            let selected = archive.selected.as_ref().is_some_and(|selected| *selected == Selected::File(round.path.clone()));
            if ui.selectable_label(selected, text).clicked() {
                archive.selected = Some(Selected::File(round.path.clone()));
            }
        }
    });
}

/// Length, kills and winner.
fn detail_line(tables: &RoundTables) -> String {
    let summary = &tables.summary;
    let mut line = format!("{} · {}", crate::game::round_file::clock(summary.length), get!("editor.rounds.kills", "kills", summary.kills));
    if let Some((name, _)) = &summary.winner {
        line.push_str(&format!(" · {name}"));
    }
    line
}

fn round_ui(ui: &mut egui::Ui, tables: &RoundTables, shown: &mut TableKind, sort: &mut HashMap<TableKind, (usize, bool)>) {
    let summary = &tables.summary;
    ui.horizontal_wrapped(|ui| {
        if let Some((name, color)) = &summary.winner {
            if let Some(color) = color {
                swatch(ui, *color);
            }
            ui.strong(get!("editor.rounds.winner", "name", name));
            ui.separator();
        }
        ui.label(get!("editor.rounds.length", "length", crate::game::round_file::clock(summary.length)));
        ui.separator();
        ui.label(get!("editor.rounds.players", "people", summary.people, "bots", summary.bots));
        ui.separator();
        ui.label(get!("editor.rounds.kills", "kills", summary.kills));
        if let Some(skill) = summary.bot_skill.filter(|_| summary.bots > 0) {
            ui.separator();
            ui.label(get!("editor.rounds.skill", "skill", skill.name()));
        }
        if let Some(limit) = summary.frag_limit {
            ui.separator();
            ui.label(get!("editor.rounds.frag_limit", "limit", limit));
        }
        if let Some(limit) = summary.time_limit {
            ui.separator();
            ui.label(get!("editor.rounds.time_limit", "limit", crate::game::round_file::clock(limit)));
        }
        if summary.skipped > 0 {
            ui.separator();
            ui.label(get!("editor.rounds.skipped", "count", summary.skipped));
        }
    });
    if tables.table(*shown).is_none() {
        *shown = TableKind::Players;
    }
    ui.horizontal(|ui| {
        for kind in TableKind::ALL.into_iter().filter(|kind| tables.table(*kind).is_some()) {
            ui.selectable_value(shown, kind, kind.name());
        }
    });
    ui.separator();
    if let Some(table) = tables.table(*shown) {
        let order = sort.get(shown).copied();
        let order = table_ui(ui, *shown, table, order);
        match order {
            Some(order) => sort.insert(*shown, order),
            None => sort.remove(shown),
        };
    }
}

/// Clicking a header sorts by it, numbers largest first; again, the other way. Returns the order.
fn table_ui(ui: &mut egui::Ui, kind: TableKind, table: &Table, sort: Option<(usize, bool)>) -> Option<(usize, bool)> {
    let mut sort = sort.filter(|(column, _)| *column < table.columns.len());
    let mut rows: Vec<usize> = (0..table.rows.len()).collect();
    if let Some((column, descending)) = sort {
        rows.sort_by(|a, b| {
            let order = table.rows[*a][column].sort.compare(&table.rows[*b][column].sort);
            if descending { order.reverse() } else { order }
        });
    }
    egui::ScrollArea::both().id_salt(("rounds_table", kind)).auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new(("rounds_grid", kind)).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
            for (index, column) in table.columns.iter().enumerate() {
                let arrow = match sort {
                    Some((sorted, true)) if sorted == index => " ▼",
                    Some((sorted, false)) if sorted == index => " ▲",
                    _ => "",
                };
                let header = egui::RichText::new(format!("{}{arrow}", column.label)).strong();
                if ui.add(egui::Button::new(header).frame(false)).clicked() {
                    sort = Some(match sort {
                        Some((sorted, descending)) if sorted == index => (index, !descending),
                        _ => (index, column.numeric),
                    });
                }
            }
            ui.end_row();
            for row in rows {
                for cell in &table.rows[row] {
                    ui.horizontal(|ui| {
                        if let Some(color) = cell.swatch {
                            swatch(ui, color);
                        }
                        ui.label(&cell.text);
                    });
                }
                ui.end_row();
            }
        });
    });
    sort
}

fn swatch(ui: &mut egui::Ui, color: Color) {
    let color = Srgba::from(color);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    ui.painter().rect_filled(rect, 2.0, egui::Color32::from_rgb(channel(color.red), channel(color.green), channel(color.blue)));
}
