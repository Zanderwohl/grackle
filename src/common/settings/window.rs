//! The Settings window, opened from every authoring mode's Edit menu or the
//! [`Action::Settings`] chord.
//!
//! It edits a draft. Nothing goes to disk or into force until Apply. Close with
//! an unchanged draft closes the window; with a changed one it asks first.
//! Pages are listed down the left third, the chosen page fills the rest.
//!
//! A floating window rather than a dock tab: it is a dialog every mode opens,
//! and the dock belongs to the map editor. While it is open no action fires
//! ([`Quiet`]), so the key being bound, or one pressed over the window, does
//! nothing else.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy_egui::egui::{self, Id, Ui};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};

use super::Settings;
use crate::common::app_mode::{authoring, AppMode};
use crate::common::icons;
use crate::common::keybind::sheet::Sheet;
use crate::common::keybind::{self, Action, Actions, Held, Keymap, Quiet, ResolveActions};
use crate::common::lang::{self, Language};
use crate::get;

const WINDOW_ID: &str = "settings_window";

/// Its smallest size, and its size on a first open. egui remembers a resize.
const MIN_SIZE: egui::Vec2 = egui::vec2(520.0, 560.0);

/// The space on each side of a line between two panels.
const GAP: i8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Page {
    General,
    Shortcuts,
    Developer,
}

impl Page {
    const ALL: [Page; 3] = [Page::General, Page::Shortcuts, Page::Developer];

    fn name(self) -> String {
        match self {
            Page::General => get!("settings.page.general"),
            Page::Shortcuts => get!("settings.page.shortcuts"),
            Page::Developer => get!("settings.page.developer"),
        }
    }
}

/// The window and its draft. A resource rather than egui memory: the draft
/// is a half-made decision, and a frame that does not draw the window must not
/// lose it.
#[derive(Resource)]
pub struct SettingsWindow {
    open: bool,
    draft: Settings,
    keys_draft: Keymap,
    /// Whether the unsaved-changes question shows.
    asking: bool,
    /// A failed save, kept until the user has seen it.
    problem: Option<String>,
    /// Kept between opens.
    page: Page,
    sheet: Sheet,
    /// Read on opening, because reading parses every lang file.
    languages: Vec<Language>,
    /// Every icon name in every pack, read on opening.
    icons: Vec<String>,
}

impl Default for SettingsWindow {
    fn default() -> Self {
        Self {
            open: false,
            draft: Settings::default(),
            keys_draft: Keymap::default(),
            asking: false,
            problem: None,
            page: Page::General,
            sheet: Sheet::default(),
            languages: Vec::new(),
            icons: Vec::new(),
        }
    }
}

impl SettingsWindow {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens it from what is in force, or raises it if it is open: a draft
    /// abandoned by an earlier Close does not come back.
    fn open(&mut self, ctx: &egui::Context, settings: &Settings, keymap: &Keymap) {
        if self.open {
            ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, Id::new(WINDOW_ID)));
            return;
        }
        self.draft = settings.clone();
        self.keys_draft = keymap.clone();
        self.sheet.stop();
        self.asking = false;
        self.problem = None;
        self.languages = lang::available_languages(&lang::default_packs());
        self.icons = icon_names();
        self.open = true;
    }

    fn has_unsaved_changes(&self, settings: &Settings, keymap: &Keymap) -> bool {
        self.draft != *settings || self.keys_draft != *keymap
    }

    /// Saves the draft and puts it in force. A failed save keeps the draft so
    /// Apply can be pressed again; the two files save independently.
    fn apply(&mut self, settings: &mut Settings, keymap: &mut Keymap) {
        self.problem = None;
        if self.draft != *settings {
            match self.draft.save() {
                Ok(()) => {
                    if self.draft.language != settings.language {
                        let code = self.draft.language.as_deref().unwrap_or(lang::FALLBACK_LANG);
                        lang::change_lang_or_fallback(code, &lang::default_packs());
                    }
                    *settings = self.draft.clone();
                }
                Err(problem) => {
                    warn!("{problem}");
                    self.problem = Some(problem);
                }
            }
        }
        if self.keys_draft != *keymap {
            match self.keys_draft.save() {
                Ok(()) => *keymap = self.keys_draft.clone(),
                Err(problem) => {
                    warn!("{problem}");
                    self.problem = Some(problem);
                }
            }
        }
    }

    /// Closes without asking, dropping the draft.
    fn dismiss(&mut self) {
        if self.open {
            info!("Leaving the editors closed the Settings window; changes not applied were dropped");
        }
        self.sheet.stop();
        self.asking = false;
        self.open = false;
    }

    fn close(&mut self, settings: &Settings, keymap: &Keymap) {
        if self.has_unsaved_changes(settings, keymap) {
            self.asking = true;
            return;
        }
        self.sheet.stop();
        self.open = false;
    }

    fn show(&mut self, ctx: &egui::Context, settings: &mut Settings, keymap: &mut Keymap, held: Held) {
        if !self.open {
            return;
        }
        let mut close_requested = false;
        let unsaved = self.has_unsaved_changes(settings, keymap);

        egui::Window::new(get!("settings.window_title"))
            .id(Id::new(WINDOW_ID))
            .collapsible(false)
            .resizable(true)
            .min_size(MIN_SIZE)
            .default_size(MIN_SIZE)
            // No `.open()`: the window's X goes the way of Close, so it asks too.
            .show(ctx, |ui| {
                // The footer first, so the pages get only the space above it.
                egui::Panel::bottom("settings_footer")
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin { top: GAP, ..Default::default() }))
                    .show(ui, |ui| {
                        if let Some(problem) = &self.problem {
                            ui.colored_label(ui.visuals().error_fg_color, problem);
                            ui.add_space(5.0);
                        }
                        ui.horizontal(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(get!("settings.close")).clicked() {
                                    close_requested = true;
                                }
                                if ui.add_enabled(unsaved, egui::Button::new(get!("settings.apply"))).clicked() {
                                    self.apply(settings, keymap);
                                }
                            });
                        });
                    });

                egui::Panel::left("settings_pages")
                    .resizable(false)
                    .exact_size(ui.available_width() / 3.0)
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin { right: GAP, ..Default::default() }))
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                            for page in Page::ALL {
                                if ui.selectable_value(&mut self.page, page, page.name()).changed() {
                                    self.sheet.stop();
                                }
                            }
                        });
                    });

                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: GAP, ..Default::default() }))
                    .show(ui, |ui| {
                        // One scroll offset per page.
                        egui::ScrollArea::vertical().id_salt(self.page).auto_shrink(false).show(ui, |ui| {
                            match self.page {
                                Page::General => self.general(ui),
                                Page::Shortcuts => self.sheet.show(ui, &mut self.keys_draft, held),
                                Page::Developer => self.developer(ui),
                            }
                        });
                    });
            });

        if close_requested {
            self.close(settings, keymap);
        }
        self.ask_about_unsaved(ctx, settings, keymap);
    }

    fn general(&mut self, ui: &mut Ui) {
        ui.heading(get!("settings.general.interface"));
        ui.add_space(5.0);
        egui::Grid::new("settings_interface").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label(get!("settings.general.language"));
            let current = &mut self.draft.language;
            let code = current.as_deref().unwrap_or(lang::FALLBACK_LANG);
            let shown = self.languages.iter().find(|l| l.code == code).map_or(code.to_owned(), |l| l.name.clone());
            egui::ComboBox::from_id_salt("language").selected_text(shown).width(180.0).show_ui(ui, |ui| {
                for language in &self.languages {
                    let chosen = match language.code == lang::FALLBACK_LANG {
                        true => None,
                        false => Some(language.code.clone()),
                    };
                    ui.selectable_value(current, chosen, &language.name);
                }
            });
            ui.end_row();
        });
    }

    fn developer(&mut self, ui: &mut Ui) {
        ui.heading(get!("settings.developer.icons"));
        ui.add_space(5.0);
        icon_grid(ui, &self.icons);
    }

    /// A modal, so the draft it is about cannot be edited while it shows.
    fn ask_about_unsaved(&mut self, ctx: &egui::Context, settings: &mut Settings, keymap: &mut Keymap) {
        if !self.asking {
            return;
        }
        egui::Modal::new(Id::new("settings_unsaved")).show(ctx, |ui| {
            ui.set_width(280.0);
            ui.heading(get!("settings.unsaved.heading"));
            ui.add_space(10.0);
            ui.label(get!("settings.unsaved.body"));
            ui.add_space(15.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(get!("settings.unsaved.discard")).clicked() {
                        self.asking = false;
                        self.sheet.stop();
                        self.open = false;
                    }
                    if ui.button(get!("settings.unsaved.save")).clicked() {
                        self.apply(settings, keymap);
                        self.asking = false;
                        // A failed save keeps the window open with the problem showing.
                        self.open = self.problem.is_some();
                    }
                });
            });
        });
    }
}

/// The width of one cell of the icon grid, and the size of the icon in it.
const ICON_CELL: f32 = 84.0;
const ICON_SIZE: f32 = 32.0;

/// Every icon, each with its name under it. A name too long for its cell is
/// cut short; egui shows the whole name on hover.
fn icon_grid(ui: &mut Ui, names: &[String]) {
    let colour = ui.visuals().text_color();
    // Every cell exactly its full size, aligned to the top: a wrapped row
    // centers each item on the row's height so far, so a taller cell would
    // push the next one down.
    let label = ui.text_style_height(&egui::TextStyle::Small);
    let cell = egui::vec2(ICON_CELL, ICON_SIZE + ui.spacing().item_spacing.y + label);
    let row = egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true);
    ui.with_layout(row, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 12.0);
        for name in names {
            ui.allocate_ui_with_layout(cell, egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.set_min_size(cell);
                match icons::image(ui.ctx(), name) {
                    Some(image) => {
                        ui.add(image.fit_to_exact_size(egui::Vec2::splat(ICON_SIZE)).tint(colour));
                    }
                    None => {
                        ui.add_sized(egui::Vec2::splat(ICON_SIZE), egui::Label::new(icons::PLACEHOLDER));
                    }
                }
                ui.add(egui::Label::new(egui::RichText::new(name).small()).truncate());
            });
        }
    });
}

/// Every `icons/*.svg` stem in every pack, sorted, each once. Reads the
/// directories, so native only; a browser build would read a manifest.
fn icon_names() -> Vec<String> {
    let mut names: Vec<String> = lang::default_packs()
        .iter()
        .filter_map(|pack| std::fs::read_dir(pack.join("icons")).ok())
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
        .filter_map(|path| path.file_stem()?.to_str().map(str::to_owned))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn request_id() -> Id {
    Id::new("grackle_open_settings")
}

/// Asks for the window, from a menu that has only a `Ui`.
pub fn request_settings(ctx: &egui::Context) {
    ctx.data_mut(|data| data.insert_temp(request_id(), true));
}

fn take_request(ctx: &egui::Context) -> bool {
    ctx.data_mut(|data| data.remove_temp::<bool>(request_id())).unwrap_or(false)
}

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        keybind::ensure(app);
        app.init_resource::<Settings>()
            .init_resource::<SettingsWindow>()
            .add_systems(PreUpdate, capture_binding.after(InputSystems).before(ResolveActions))
            .add_systems(Update, open_from_the_keyboard.run_if(authoring))
            .add_systems(EguiPrimaryContextPass, draw_settings);
    }
}

fn capture_binding(
    mut window: ResMut<SettingsWindow>,
    keys: Res<ButtonInput<KeyCode>>,
    mut messages: MessageReader<KeyboardInput>,
) {
    let window = &mut *window;
    window.sheet.capture_from(&mut window.keys_draft, &keys, &mut messages);
}

#[derive(Resource, Default)]
struct OpenRequested;

fn open_from_the_keyboard(mut commands: Commands, actions: Res<Actions>) {
    if actions.just_pressed(Action::Settings) {
        commands.insert_resource(OpenRequested);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_settings(
    mut commands: Commands,
    mut contexts: EguiContexts,
    mut window: ResMut<SettingsWindow>,
    mut settings: ResMut<Settings>,
    mut keymap: ResMut<Keymap>,
    mut quiet: ResMut<Quiet>,
    keys: Res<ButtonInput<KeyCode>>,
    requested: Option<Res<OpenRequested>>,
    mode: Option<Res<State<AppMode>>>,
    mut published: Local<bool>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        // No window can be drawn, so nothing may stay quiet on its account.
        quiet.set_if_neq(Quiet(false));
        return;
    };
    if keymap.is_changed() || !*published {
        keybind::publish(ctx, &keymap);
        *published = true;
    }
    let authoring = mode.is_none_or(|mode| mode.get().is_authoring());
    if take_request(ctx) || requested.is_some() {
        commands.remove_resource::<OpenRequested>();
        if authoring {
            window.open(ctx, &settings, &keymap);
        }
    }
    // Play grabs the cursor, so a window left open there could not be closed
    // and would keep every action quiet.
    if !authoring {
        window.dismiss();
    }
    if window.is_open() {
        // Compared rather than written through, so drawing is not an edit.
        let before = keymap.clone();
        window.show(ctx, &mut settings, keymap.bypass_change_detection(), Held::of(&keys));
        if *keymap != before {
            keymap.set_changed();
        }
    }
    quiet.set_if_neq(Quiet(window.is_open()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_egui::egui::{Pos2, RawInput, Rect};

    /// Draws the window headless and returns each text it painted, with its rect.
    fn frame(ctx: &egui::Context, window: &mut SettingsWindow) -> Vec<(String, Rect)> {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 900.0))),
            ..Default::default()
        };
        let mut texts = Vec::new();
        let (mut settings, mut keymap) = (Settings::default(), Keymap::default());
        // Inside the frame, because `Context::run` drains the paint lists.
        let mut output = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            window.show(&ctx, &mut settings, &mut keymap, Held::default());
            ctx.graphics(|graphics| {
                let Some(list) = graphics.get(egui::LayerId::new(egui::Order::Middle, Id::new(WINDOW_ID))) else { return };
                for entry in list.all_entries() {
                    if let egui::Shape::Text(text) = &entry.shape {
                        texts.push((text.galley.text().to_owned(), entry.shape.visual_bounding_rect()));
                    }
                }
            });
        });
        output.textures_delta.clear();
        texts
    }

    fn find<'a>(texts: &'a [(String, Rect)], wanted: &str) -> Option<&'a Rect> {
        texts.iter().find(|(text, _)| text == wanted).map(|(_, rect)| rect)
    }

    /// Fails if a page shows another's settings, sits under the page list, or
    /// pushes Apply and Close off the window.
    #[test]
    fn each_page_shows_its_own_settings_beside_the_list_and_above_the_footer() {
        let ctx = egui::Context::default();
        let mut window = SettingsWindow::default();
        window.open(&ctx, &Settings::default(), &Keymap::default());

        for (page, mine, other) in [
            (Page::General, get!("settings.general.language"), get!("settings.developer.icons")),
            (Page::Shortcuts, get!("keybind.sheet.heading"), get!("settings.general.language")),
            (Page::Developer, get!("settings.developer.icons"), get!("keybind.sheet.heading")),
        ] {
            window.page = page;
            let mut texts = Vec::new();
            for _ in 0..5 {
                texts = frame(&ctx, &mut window);
            }
            for name in Page::ALL {
                assert!(find(&texts, &name.name()).is_some(), "{} is missing from the list", name.name());
            }
            let heading = find(&texts, &mine).unwrap_or_else(|| panic!("{page:?} does not show {mine}"));
            assert!(find(&texts, &other).is_none(), "{page:?} also shows {other}");
            let list = find(&texts, &page.name()).unwrap();
            assert!(heading.left() > list.right(), "{mine} is not right of the page list");

            let apply = find(&texts, &get!("settings.apply")).expect("Apply is missing");
            let close = find(&texts, &get!("settings.close")).expect("Close is missing");
            for button in [apply, close] {
                assert!(button.top() > heading.bottom(), "a button is not below the page");
                assert!(button.bottom() < 900.0, "a button is off the bottom of the screen");
            }
            assert!((apply.top() - close.top()).abs() < 1.0, "Apply and Close are not on one row");
        }
    }

    /// Fails if the Shortcuts page leaves out an action, so it cannot be rebound.
    #[test]
    fn the_shortcuts_page_lists_every_action() {
        let ctx = egui::Context::default();
        let mut window = SettingsWindow::default();
        window.open(&ctx, &Settings::default(), &Keymap::default());
        window.page = Page::Shortcuts;
        let mut texts = Vec::new();
        for _ in 0..5 {
            texts = frame(&ctx, &mut window);
        }
        // Only those scrolled into view are painted, so check the first context's.
        for &action in Action::ALL.iter().filter(|a| a.context() == keybind::Context::Everywhere) {
            assert!(find(&texts, &action.name()).is_some(), "{action:?} is not listed");
        }
    }

    #[test]
    fn the_developer_page_lists_the_shipped_icons() {
        let names = icon_names();
        assert!(names.iter().any(|name| name == icons::FALLBACK), "{names:?}");
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]), "not sorted and unique");
    }
}
