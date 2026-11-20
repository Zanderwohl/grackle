//! The main menu's server browser: an egui window listing the servers LAN discovery has found
//! ([`crate::common::lan_discovery`]), sortable by any column and filtered by the controls under it.
//! Only LAN servers exist so far, so it has no tabs.

use std::cmp::Ordering;
use std::net::SocketAddr;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use egui_extras::{Column, TableBuilder};
use strum::IntoEnumIterator;

use crate::common::app_mode::AppMode;
use crate::common::keybind::{self, Action, Actions};
use crate::common::lan_discovery::{FoundServer, ServerList};
use crate::common::mode::GameMode;
use crate::common::net::NetRequest;
use crate::get;

const WINDOW_ID: &str = "server_browser";
/// Where the largest filter section's size is kept between frames.
const SECTION_SIZE_ID: &str = "server_browser_section_size";
const ROW_HEIGHT: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Motd,
    Map,
    Mode,
    Players,
    Ping,
}

impl SortBy {
    const ALL: [SortBy; 6] = [SortBy::Name, SortBy::Motd, SortBy::Map, SortBy::Mode, SortBy::Players, SortBy::Ping];

    fn heading(self) -> String {
        match self {
            SortBy::Name => get!("server_browser.column.name"),
            SortBy::Motd => get!("server_browser.column.motd"),
            SortBy::Map => get!("server_browser.column.map"),
            SortBy::Mode => get!("server_browser.column.mode"),
            SortBy::Players => get!("server_browser.column.players"),
            SortBy::Ping => get!("server_browser.column.ping"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BotsFilter {
    #[default]
    Any,
    No,
    Yes,
}

impl BotsFilter {
    const ALL: [BotsFilter; 3] = [BotsFilter::Any, BotsFilter::No, BotsFilter::Yes];

    fn name(self) -> String {
        match self {
            BotsFilter::Any => get!("server_browser.any"),
            BotsFilter::No => get!("server_browser.no"),
            BotsFilter::Yes => get!("server_browser.yes"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Filters {
    /// Whitespace-separated terms, each of which must appear in the name, MOTD, map or mode.
    pub search: String,
    pub mode: Option<GameMode>,
    /// Matched against the map name alone.
    pub map: String,
    pub bots: BotsFilter,
    pub max_ping_ms: Option<u32>,
    pub has_players: bool,
    pub not_full: bool,
}

/// A server as its row shows it, with lowercase copies to search and sort by, built once per
/// frame rather than per comparison.
pub struct Row<'a> {
    pub server: &'a FoundServer,
    name: String,
    motd: String,
    map: String,
    mode: String,
    /// Name, MOTD, map and mode, lowercase.
    keys: [String; 4],
}

impl<'a> Row<'a> {
    pub fn of(server: &'a FoundServer) -> Self {
        let reply = &server.reply;
        let name = if reply.name.is_empty() { get!("server_browser.unnamed") } else { reply.name.clone() };
        let map = if reply.map.is_empty() { get!("server_browser.untitled") } else { reply.map.clone() };
        let mode = mode_name(&reply.mode);
        let keys = [&name, &reply.motd, &map, &mode].map(|text| text.to_lowercase());
        Row { server, name, motd: reply.motd.clone(), map, mode, keys }
    }

    fn key(&self, by: SortBy) -> &str {
        match by {
            SortBy::Motd => &self.keys[1],
            SortBy::Map => &self.keys[2],
            SortBy::Mode => &self.keys[3],
            _ => &self.keys[0],
        }
    }
}

/// The mode in this machine's language, else the key the server sent.
fn mode_name(key: &str) -> String {
    GameMode::try_from(key).map(|mode| mode.to_string()).unwrap_or_else(|_| key.to_owned())
}

impl Filters {
    pub fn admits(&self, row: &Row) -> bool {
        let reply = &row.server.reply;
        let searched = self.search.split_whitespace().map(str::to_lowercase).all(|term| row.keys.iter().any(|key| key.contains(&term)));
        searched
            && self.mode.is_none_or(|mode| GameMode::try_from(reply.mode.as_str()) == Ok(mode))
            && row.key(SortBy::Map).contains(&self.map.trim().to_lowercase())
            && match self.bots {
                BotsFilter::Any => true,
                BotsFilter::No => !reply.bots_enabled,
                BotsFilter::Yes => reply.bots_enabled,
            }
            && self.max_ping_ms.is_none_or(|most| row.server.ping_ms <= most as f32)
            && (!self.has_players || reply.people > 0)
            && (!self.not_full || reply.people < reply.slots)
    }
}

/// Ties are broken by name.
pub fn sort(rows: &mut [Row], by: SortBy, ascending: bool) {
    rows.sort_by(|a, b| {
        let order = match by {
            SortBy::Players => (a.server.reply.people, a.server.reply.slots).cmp(&(b.server.reply.people, b.server.reply.slots)),
            SortBy::Ping => a.server.ping_ms.partial_cmp(&b.server.ping_ms).unwrap_or(Ordering::Equal),
            _ => a.key(by).cmp(b.key(by)),
        };
        let order = if ascending { order } else { order.reverse() };
        order.then_with(|| a.key(SortBy::Name).cmp(b.key(SortBy::Name)))
    });
}

#[derive(Resource, Debug, Clone)]
pub struct ServerBrowser {
    pub open: bool,
    pub sort: SortBy,
    pub ascending: bool,
    pub filters: Filters,
    pub selected: Option<SocketAddr>,
}

impl Default for ServerBrowser {
    fn default() -> Self {
        ServerBrowser { open: false, sort: SortBy::Name, ascending: true, filters: Filters::default(), selected: None }
    }
}

impl ServerBrowser {
    pub fn sort_by(&mut self, by: SortBy) {
        if self.sort == by {
            self.ascending = !self.ascending;
        } else {
            self.sort = by;
            self.ascending = true;
        }
    }
}

pub struct ServerBrowserPlugin;

impl Plugin for ServerBrowserPlugin {
    fn build(&self, app: &mut App) {
        keybind::ensure(app);
        app.init_resource::<ServerBrowser>()
            .add_systems(Update, (close_on_escape.run_if(in_state(AppMode::MainMenu)), search_while_open).chain())
            .add_systems(OnExit(AppMode::MainMenu), |mut browser: ResMut<ServerBrowser>| browser.open = false)
            .add_systems(EguiPrimaryContextPass, browser_window.run_if(in_state(AppMode::MainMenu)));
    }
}

fn close_on_escape(actions: Res<Actions>, mut browser: ResMut<ServerBrowser>) {
    if browser.open && actions.just_pressed(Action::Cancel) {
        browser.open = false;
    }
}

fn search_while_open(browser: Res<ServerBrowser>, list: Option<ResMut<ServerList>>) {
    if let Some(mut list) = list
        && list.searching != browser.open
    {
        list.searching = browser.open;
    }
}

fn browser_window(mut contexts: EguiContexts, mut browser: ResMut<ServerBrowser>, list: Option<Res<ServerList>>, mut requests: MessageWriter<NetRequest>) {
    if !browser.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let servers = list.as_deref().map_or(&[][..], |list| &list.servers);
    if let Some(address) = show_the_window(ctx, &mut browser, servers).join {
        requests.write(NetRequest::Join { host: address.ip().to_string(), port: address.port() });
        browser.open = false;
    }
}

/// What one frame of the window came to.
struct Drawn {
    /// A server chosen to join.
    join: Option<SocketAddr>,
    /// Each filter section's rect, margin included.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the window's test"))]
    sections: Vec<egui::Rect>,
}

/// Draws the window, closing it if its X was clicked.
fn show_the_window(ctx: &egui::Context, browser: &mut ServerBrowser, servers: &[FoundServer]) -> Drawn {
    let mut open = true;
    let mut join = None;
    let mut sections = Vec::new();
    egui::Window::new(get!("server_browser.title"))
        .id(egui::Id::new(WINDOW_ID))
        .collapsible(false)
        .resizable(true)
        .default_size([820.0, 520.0])
        // Where it opens; egui remembers wherever it is dragged after that.
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.viewport_rect().center())
        .open(&mut open)
        .show(ctx, |ui| {
            // The controls are laid out first and the table takes exactly what is left, so the
            // contents never ask for more height than the window has, which would grow it.
            egui::Panel::bottom("server_browser_controls").resizable(false).show_separator_line(false).show(ui, |ui| {
                ui.add_space(4.0);
                sections = filter_controls(ui, &mut browser.filters);
                ui.add_space(4.0);
                let selected = browser.selected.and_then(|address| servers.iter().find(|server| server.address == address));
                let joinable = selected.is_some_and(FoundServer::joinable);
                if ui.add_enabled(joinable, egui::Button::new(get!("server_browser.join"))).clicked() {
                    join = selected.map(|server| server.address);
                }
                ui.add_space(4.0);
            });
            egui::CentralPanel::default().show(ui, |ui| {
                if let Some(chosen) = server_table(ui, browser, servers) {
                    join = Some(chosen);
                }
            });
        });
    if !open {
        browser.open = false;
    }
    Drawn { join, sections }
}

/// Where each column starts, in logical pixels; a column dragged afterward keeps its own width.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ColumnWidths {
    /// Players and Ping: the Players header, the wider of their two.
    narrow: f32,
    /// Map and Game Mode: the Game Mode header and two characters.
    wide: f32,
    name: f32,
    motd: f32,
    /// Six characters, which no column starts or is dragged narrower than.
    least: f32,
}

impl ColumnWidths {
    fn measured(ui: &egui::Ui) -> Self {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let text = |text: &str| ui.painter().layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE).size().x;
        // A header carries its sort arrow and the button's padding on each side.
        let header = |by: SortBy| text(&format!("{}{SORTED_UP}", by.heading())) + 2.0 * ui.spacing().button_padding.x;
        Self::split(ui.available_width(), ui.spacing().item_spacing.x, header(SortBy::Players), header(SortBy::Mode), text("0"))
    }

    /// `available` is the table's width and `gap` what the table leaves between two columns.
    fn split(available: f32, gap: f32, players_header: f32, mode_header: f32, char_width: f32) -> Self {
        let narrow = players_header;
        let wide = mode_header + 2.0 * char_width;
        let least = 6.0 * char_width;
        let left = available - 2.0 * narrow - 2.0 * wide - 5.0 * gap;
        ColumnWidths { narrow, wide, name: (left / 3.0).max(least), motd: (left * 2.0 / 3.0).max(least), least }
    }
}

const SORTED_UP: &str = " ⏶";
const SORTED_DOWN: &str = " ⏷";

/// Returns a server double-clicked to join.
fn server_table(ui: &mut egui::Ui, browser: &mut ServerBrowser, servers: &[FoundServer]) -> Option<SocketAddr> {
    // On a window's first frame egui measures its contents, and a table would store its clipped
    // columns at their contents' widths then, keeping them over the widths given here.
    if ui.is_sizing_pass() {
        ui.allocate_space(ui.available_size());
        return None;
    }
    let height = (ui.available_height() - ROW_HEIGHT - 2.0 * ui.spacing().item_spacing.y).max(ROW_HEIGHT);
    let mut shown: Vec<Row> = servers.iter().map(Row::of).filter(|row| browser.filters.admits(row)).collect();
    sort(&mut shown, browser.sort, browser.ascending);
    let mut join = None;
    let mut clicked_header = None;
    let widths = ColumnWidths::measured(ui);
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .auto_shrink(false)
        .min_scrolled_height(0.0)
        .max_scroll_height(height)
        .column(Column::initial(widths.name).at_least(widths.least).clip(true))
        .column(Column::initial(widths.motd).at_least(widths.least).clip(true))
        .column(Column::initial(widths.wide).at_least(widths.least).clip(true))
        .column(Column::initial(widths.wide).at_least(widths.least).clip(true))
        .column(Column::initial(widths.narrow).at_least(widths.least))
        .column(Column::initial(widths.narrow).at_least(widths.least))
        .header(ROW_HEIGHT, |mut header| {
            for by in SortBy::ALL {
                header.col(|ui| {
                    let arrow = match (browser.sort == by, browser.ascending) {
                        (false, _) => "",
                        (true, true) => SORTED_UP,
                        (true, false) => SORTED_DOWN,
                    };
                    let label = egui::RichText::new(format!("{}{arrow}", by.heading())).strong();
                    if ui.add(egui::Button::new(label).frame(false)).clicked() {
                        clicked_header = Some(by);
                    }
                });
            }
        })
        .body(|mut body| {
            if shown.is_empty() {
                let message = if servers.is_empty() { get!("server_browser.querying") } else { get!("server_browser.no_match") };
                body.row(ROW_HEIGHT, |mut row| {
                    row.col(|ui| {
                        ui.add(egui::Label::new(message).wrap_mode(egui::TextWrapMode::Extend));
                    });
                });
                return;
            }
            for row in shown {
                let server = row.server;
                let reply = &server.reply;
                body.row(ROW_HEIGHT, |mut table_row| {
                    table_row.set_selected(browser.selected == Some(server.address));
                    let cells = [
                        row.name,
                        row.motd,
                        row.map,
                        row.mode,
                        format!("{}/{}", reply.people, reply.slots),
                        get!("server_browser.ping_ms", "ms", server.ping_ms.round() as u32),
                    ];
                    for cell in cells {
                        table_row.col(|ui| {
                            let label = if server.joinable() { egui::RichText::new(cell) } else { egui::RichText::new(cell).weak() };
                            ui.add(egui::Label::new(label).truncate().selectable(false));
                        });
                    }
                    let response = table_row.response();
                    let response = match server.joinable() {
                        true => response,
                        false => response.on_hover_text(get!("server_browser.incompatible", "build", &reply.build)),
                    };
                    if response.clicked() {
                        browser.selected = Some(server.address);
                    }
                    if response.double_clicked() && server.joinable() {
                        join = Some(server.address);
                    }
                });
            }
        });
    if let Some(by) = clicked_header {
        browser.sort_by(by);
    }
    join
}

/// Returns each section's rect.
fn filter_controls(ui: &mut egui::Ui, filters: &mut Filters) -> Vec<egui::Rect> {
    ui.horizontal(|ui| {
        ui.label(get!("server_browser.search"));
        ui.add(egui::TextEdit::singleline(&mut filters.search).desired_width(f32::INFINITY));
    });
    // Sections wrap onto the next row when the window is too narrow. Each is drawn at the largest
    // one's size from last frame, so they line up whatever their contents or language.
    let size_id = egui::Id::new(SECTION_SIZE_ID);
    let size = ui.data(|data| data.get_temp::<egui::Vec2>(size_id)).unwrap_or(egui::Vec2::ZERO);
    let mut largest = egui::Vec2::ZERO;
    let mut rects = Vec::new();
    ui.horizontal_wrapped(|ui| {
        let mut add_section = |ui: &mut egui::Ui, title: String, add_contents: &mut dyn FnMut(&mut egui::Ui)| {
            let (rect, natural) = section(ui, title, size, add_contents);
            largest = largest.max(natural);
            rects.push(rect);
        };
        add_section(ui, get!("server_browser.section.game"), &mut |ui| {
            let mode_text = |mode: Option<GameMode>| mode.map_or_else(|| get!("server_browser.any"), |mode| mode.to_string());
            egui::ComboBox::from_label(get!("server_browser.mode")).selected_text(mode_text(filters.mode)).show_ui(ui, |ui| {
                ui.selectable_value(&mut filters.mode, None, mode_text(None));
                for mode in GameMode::iter() {
                    ui.selectable_value(&mut filters.mode, Some(mode), mode_text(Some(mode)));
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut filters.map).desired_width(120.0));
                ui.label(get!("server_browser.map"));
            });
        });
        add_section(ui, get!("server_browser.section.players"), &mut |ui| {
            ui.checkbox(&mut filters.has_players, get!("server_browser.has_players"));
            ui.checkbox(&mut filters.not_full, get!("server_browser.not_full"));
            egui::ComboBox::from_label(get!("server_browser.bots")).selected_text(filters.bots.name()).show_ui(ui, |ui| {
                for bots in BotsFilter::ALL {
                    ui.selectable_value(&mut filters.bots, bots, bots.name());
                }
            });
        });
        add_section(ui, get!("server_browser.section.other"), &mut |ui| {
            ui.horizontal(|ui| {
                let mut limited = filters.max_ping_ms.is_some();
                if ui.checkbox(&mut limited, get!("server_browser.max_ping")).changed() {
                    filters.max_ping_ms = limited.then_some(DEFAULT_MAX_PING_MS);
                }
                if let Some(most) = filters.max_ping_ms.as_mut() {
                    ui.add(egui::DragValue::new(most).range(1..=9999).suffix(get!("server_browser.ms_suffix")));
                }
            });
        });
    });
    if largest != size {
        ui.data_mut(|data| data.insert_temp(size_id, largest));
        ui.ctx().request_repaint();
    }
    rects
}

/// Draws a section at least `size` inside its margin. Returns its rect, margin included, and the
/// size its contents asked for.
fn section(ui: &mut egui::Ui, title: String, size: egui::Vec2, add_contents: &mut dyn FnMut(&mut egui::Ui)) -> (egui::Rect, egui::Vec2) {
    let mut natural = egui::Vec2::ZERO;
    // A group's margin, without its border.
    let frame = egui::Frame::new().inner_margin(6).show(ui, |ui| {
        ui.set_min_size(size);
        natural = ui
            .vertical(|ui| {
                ui.strong(title);
                add_contents(ui);
            })
            .response
            .rect
            .size();
    });
    (frame.response.rect, natural)
}

/// What ticking Max ping starts at.
const DEFAULT_MAX_PING_MS: u32 = 100;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::lan_discovery::Reply;
    use crate::constants::NETCODE_PROTOCOL_ID;

    fn server(name: &str, map: &str, mode: &str, people: u16, slots: u16, ping_ms: f32) -> FoundServer {
        FoundServer {
            address: format!("192.168.1.{}:27100", 10 + name.len()).parse().unwrap(),
            reply: Reply {
                nonce: 0,
                netcode_protocol: NETCODE_PROTOCOL_ID,
                build: "0.1.0".into(),
                port: 27100,
                name: name.into(),
                motd: format!("{name} says hello"),
                map: map.into(),
                mode: mode.into(),
                people,
                slots,
                bots_enabled: people == 0,
            },
            ping_ms,
            pings: [ping_ms].into(),
            last_seen: 0.0,
        }
    }

    fn admitted<'a>(filters: &Filters, servers: &'a [FoundServer]) -> Vec<&'a str> {
        servers.iter().filter(|server| filters.admits(&Row::of(server))).map(|server| server.reply.name.as_str()).collect()
    }

    fn servers() -> Vec<FoundServer> {
        vec![
            server("Upstairs", "Foundry", "arena", 3, 8, 12.0),
            server("Basement", "Cloister", "ctf", 0, 8, 40.0),
            server("Attic", "Waterworks", "arena", 8, 8, 150.0),
        ]
    }

    /// Breaks if the search is not split into terms that must all match, is case-sensitive, or
    /// misses the MOTD or the mode's name.
    #[test]
    fn every_search_term_must_match_some_field() {
        let servers = servers();
        let search = |text: &str| admitted(&Filters { search: text.into(), ..default() }, &servers);
        assert_eq!(search(""), ["Upstairs", "Basement", "Attic"]);
        assert_eq!(search("FOUNDRY"), ["Upstairs"]);
        assert_eq!(search("hello attic"), ["Attic"], "terms were not each matched");
        assert_eq!(search("cloister upstairs"), Vec::<&str>::new(), "terms matched different servers");
        assert_eq!(search(&GameMode::CTF.to_string().to_lowercase()), ["Basement"], "the mode's name was not searched");
    }

    /// Breaks if any filter lets through what it should keep out, or the map filter reads another field.
    #[test]
    fn each_filter_narrows_the_list() {
        let servers = servers();
        let with = |filters: Filters| admitted(&filters, &servers);
        assert_eq!(with(Filters { mode: Some(GameMode::Arena), ..default() }), ["Upstairs", "Attic"]);
        assert_eq!(with(Filters { map: "water".into(), ..default() }), ["Attic"]);
        assert_eq!(with(Filters { map: "Upstairs".into(), ..default() }), Vec::<&str>::new(), "the map filter read the name");
        assert_eq!(with(Filters { bots: BotsFilter::Yes, ..default() }), ["Basement"]);
        assert_eq!(with(Filters { bots: BotsFilter::No, ..default() }), ["Upstairs", "Attic"]);
        assert_eq!(with(Filters { max_ping_ms: Some(40), ..default() }), ["Upstairs", "Basement"]);
        assert_eq!(with(Filters { has_players: true, ..default() }), ["Upstairs", "Attic"]);
        assert_eq!(with(Filters { not_full: true, ..default() }), ["Upstairs", "Basement"]);
    }

    /// Breaks if the window grows frame after frame, which it does when its contents ask for more
    /// height than it has, or if the filter sections are not all one size.
    #[test]
    fn the_window_keeps_its_height_and_its_sections_match() {
        let ctx = egui::Context::default();
        let mut browser = ServerBrowser { open: true, ..default() };
        let servers = servers();
        let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..default() };
        let mut heights = Vec::new();
        let mut sections = Vec::new();
        for _ in 0..120 {
            let mut output = ctx.run_ui(input(), |ui| {
                sections = show_the_window(ui.ctx(), &mut browser, &servers).sections;
            });
            output.textures_delta.clear();
            heights.push(ctx.memory(|memory| memory.area_rect(egui::Id::new(WINDOW_ID))).expect("the window was not drawn").height());
        }
        let (settled, last) = (heights[10], heights[heights.len() - 1]);
        assert_eq!(sections.len(), 3);
        for rect in &sections {
            assert!((rect.size() - sections[0].size()).length() < 0.5, "the sections differ in size: {:?}", sections.iter().map(|rect| rect.size()).collect::<Vec<_>>());
        }
        assert!((last - settled).abs() < 0.5, "the window grew from {settled} to {last}");
        assert!(last < 700.0, "the window is {last} tall against a default of 520");
    }

    /// Breaks if Name and MOTD do not split what the fixed columns leave a third to two thirds, or
    /// either starts under six characters.
    #[test]
    fn name_and_motd_share_what_the_other_columns_leave() {
        let widths = ColumnWidths::split(1000.0, 10.0, 60.0, 90.0, 8.0);
        assert_eq!((widths.narrow, widths.wide, widths.least), (60.0, 106.0, 48.0));
        let left = 1000.0 - 2.0 * 60.0 - 2.0 * 106.0 - 5.0 * 10.0;
        assert!((widths.name - left / 3.0).abs() < 0.01 && (widths.motd - left * 2.0 / 3.0).abs() < 0.01, "{widths:?}");
        let cramped = ColumnWidths::split(400.0, 10.0, 60.0, 90.0, 8.0);
        assert_eq!((cramped.name, cramped.motd), (48.0, 48.0), "a cramped table started a column under six characters");
    }

    /// Breaks if a column sorts by the wrong thing, or a second click on it does not turn it round.
    #[test]
    fn the_headers_sort_and_turn_the_order_round() {
        let servers = servers();
        let names = |by: SortBy, ascending: bool| {
            let mut rows: Vec<Row> = servers.iter().map(Row::of).collect();
            sort(&mut rows, by, ascending);
            rows.iter().map(|row| row.server.reply.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(names(SortBy::Name, true), ["Attic", "Basement", "Upstairs"]);
        assert_eq!(names(SortBy::Map, true), ["Basement", "Upstairs", "Attic"]);
        assert_eq!(names(SortBy::Players, false), ["Attic", "Upstairs", "Basement"]);
        assert_eq!(names(SortBy::Ping, true), ["Upstairs", "Basement", "Attic"]);

        let mut browser = ServerBrowser::default();
        assert_eq!((browser.sort, browser.ascending), (SortBy::Name, true), "the list does not open sorted by name");
        browser.sort_by(SortBy::Name);
        assert_eq!((browser.sort, browser.ascending), (SortBy::Name, false));
        browser.sort_by(SortBy::Map);
        assert_eq!((browser.sort, browser.ascending), (SortBy::Map, true));
    }
}
