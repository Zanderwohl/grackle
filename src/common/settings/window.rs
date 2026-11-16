//! The Settings window, opened from every authoring mode's Edit menu, the
//! [`Action::Settings`] chord, or the pause menu in Play. It edits a draft: nothing is saved until Apply,
//! and Close with changes asks first.
//!
//! A floating window, because every mode opens it and the dock belongs to the
//! map editor. While it is open no action fires ([`Quiet`]).

use bevy::input::keyboard::KeyboardInput;
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy_egui::egui::{self, Id, Ui};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};

use super::Settings;
use crate::common::app_mode::{authoring, AppMode};
use crate::common::credits;
use crate::common::fonts::{self, PackFonts};
use crate::common::music::MusicLibrary;
use crate::common::pack_fonts::PackFont;
use crate::common::player_settings::{self, PlayerSettings, NAME_LIMIT};
use crate::common::icons;
use crate::common::keybind::sheet::Sheet;
use crate::common::keybind::{self, Action, Actions, Held, Keymap, Quiet, ResolveActions};
use crate::common::lang::{self, Language};
use crate::get;

const WINDOW_ID: &str = "settings_window";

/// Also its size on a first open; egui remembers a resize.
const MIN_SIZE: egui::Vec2 = egui::vec2(520.0, 560.0);

/// The space on each side of a line between two panels.
const GAP: i8 = 8;

const FONT_NAME_SIZE: f32 = 20.0;
const SAMPLE_SIZE: f32 = 28.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Page {
    General,
    Graphics,
    Sound,
    Shortcuts,
    Fonts,
    Developer,
    Attribution,
}

impl Page {
    const ALL: [Page; 7] = [Page::General, Page::Graphics, Page::Sound, Page::Shortcuts, Page::Fonts, Page::Developer, Page::Attribution];

    fn name(self) -> String {
        match self {
            Page::General => get!("settings.page.general"),
            Page::Graphics => get!("settings.page.graphics"),
            Page::Sound => get!("settings.page.sound"),
            Page::Shortcuts => get!("settings.page.shortcuts"),
            Page::Fonts => get!("settings.page.fonts"),
            Page::Developer => get!("settings.page.developer"),
            Page::Attribution => get!("settings.page.attribution"),
        }
    }
}

/// A resource rather than egui memory, so a frame that does not draw the
/// window cannot lose the draft.
#[derive(Resource)]
pub struct SettingsWindow {
    open: bool,
    draft: Settings,
    keys_draft: Keymap,
    /// `None` where the app has no `PlayerSettingsPlugin`, and the page
    /// shows no player settings.
    player_draft: Option<PlayerSettings>,
    asking: bool,
    problem: Option<String>,
    page: Page,
    sheet: Sheet,
    /// Read on opening, because reading parses every lang file.
    languages: Vec<Language>,
    icons: Vec<String>,
    /// What the Fonts page draws in each font; kept while the process runs.
    sample: Option<String>,
    /// Apply wrote the keymap this frame.
    keys_applied: bool,
    /// Apply wrote the player settings this frame, so they are saved and sent.
    player_applied: bool,
    escaped: bool,
    /// A text field had the keyboard last frame, so an Escape now only takes its focus.
    typing: bool,
}

/// What the drafts are copies of; written only by Apply.
pub struct InForce<'a> {
    pub settings: &'a mut Settings,
    pub keymap: &'a mut Keymap,
    pub player: Option<&'a mut PlayerSettings>,
}

impl Default for SettingsWindow {
    fn default() -> Self {
        Self {
            open: false,
            draft: Settings::default(),
            keys_draft: Keymap::default(),
            player_draft: None,
            asking: false,
            problem: None,
            page: Page::General,
            sheet: Sheet::default(),
            languages: Vec::new(),
            icons: Vec::new(),
            sample: None,
            keys_applied: false,
            player_applied: false,
            escaped: false,
            typing: false,
        }
    }
}

impl SettingsWindow {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Starts from what is in force, so a draft abandoned by Close does not
    /// come back.
    fn open(&mut self, ctx: &egui::Context, now: &InForce) {
        if self.open {
            ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, Id::new(WINDOW_ID)));
            return;
        }
        self.draft = now.settings.clone();
        self.keys_draft = now.keymap.clone();
        self.player_draft = now.player.as_deref().cloned();
        self.sheet.stop();
        self.asking = false;
        self.problem = None;
        self.languages = lang::available_languages(&lang::default_packs());
        self.icons = icon_names();
        self.open = true;
    }

    fn has_unsaved_changes(&self, now: &InForce) -> bool {
        self.draft != *now.settings || self.keys_draft != *now.keymap || self.player_draft.as_ref() != now.player.as_deref()
    }

    /// A failed save keeps the draft, so Apply can be pressed again.
    /// `PlayerSettingsPlugin` saves the player settings; they are cleaned here
    /// because the host's own name is shown as it stands.
    fn apply(&mut self, now: &mut InForce) {
        self.problem = None;
        let InForce { settings, keymap, player } = now;
        if let (Some(draft), Some(player)) = (&mut self.player_draft, player) {
            *draft = draft.clone().cleaned();
            if *draft != **player {
                **player = draft.clone();
                self.player_applied = true;
            }
        }
        if self.draft != **settings {
            match self.draft.save() {
                Ok(()) => {
                    if self.draft.language != settings.language {
                        let code = self.draft.language.as_deref().unwrap_or(lang::FALLBACK_LANG);
                        lang::change_lang_or_fallback(code, &lang::default_packs());
                    }
                    **settings = self.draft.clone();
                }
                Err(problem) => {
                    warn!("{problem}");
                    self.problem = Some(problem);
                }
            }
        }
        if self.keys_draft != **keymap {
            match self.keys_draft.save() {
                Ok(()) => {
                    **keymap = self.keys_draft.clone();
                    self.keys_applied = true;
                }
                Err(problem) => {
                    warn!("{problem}");
                    self.problem = Some(problem);
                }
            }
        }
    }

    fn dismiss(&mut self) {
        if self.open {
            info!("Starting a round closed the Settings window; changes not applied were dropped");
        }
        self.sheet.stop();
        self.asking = false;
        self.open = false;
    }

    fn escape(&mut self, now: &InForce) {
        match self.asking {
            true => self.asking = false,
            false => self.close(now),
        }
    }

    fn close(&mut self, now: &InForce) {
        if self.has_unsaved_changes(now) {
            self.asking = true;
            return;
        }
        self.sheet.stop();
        self.open = false;
    }

    fn show(&mut self, ctx: &egui::Context, now: &mut InForce, held: Held, packs: Credited) {
        if !self.open {
            return;
        }
        let mut close_requested = false;
        let unsaved = self.has_unsaved_changes(now);

        egui::Window::new(get!("settings.window_title"))
            .id(Id::new(WINDOW_ID))
            .collapsible(false)
            .resizable(true)
            .min_size(MIN_SIZE)
            .default_size(MIN_SIZE)
            // No `.open()`, so the window's X asks about changes as Close does.
            .show(ctx, |ui| {
                // First, so the pages get only the space above it.
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
                                    self.apply(now);
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
                        if self.page == Page::Fonts {
                            // Its own scroll area, so the sample stays in view.
                            let sample = self.sample.get_or_insert_with(|| get!("settings.fonts.sample"));
                            fonts_page(ui, sample, packs.fonts);
                            return;
                        }
                        egui::ScrollArea::vertical().id_salt(self.page).auto_shrink(false).show(ui, |ui| {
                            match self.page {
                                Page::General => self.general(ui),
                                Page::Graphics => self.graphics(ui),
                                Page::Sound => self.sound(ui),
                                Page::Shortcuts => self.sheet.show(ui, &mut self.keys_draft, held),
                                Page::Developer => self.developer(ui),
                                Page::Attribution => credits::credits(ui, packs.music, packs.fonts),
                                Page::Fonts => {}
                            }
                        });
                    });
            });

        if close_requested {
            self.close(now);
        }
        self.ask_about_unsaved(ctx, now);
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

        let Some(player) = &mut self.player_draft else { return };
        ui.add_space(GAP.into());
        ui.heading(get!("settings.general.player"));
        ui.add_space(5.0);
        egui::Grid::new("settings_player").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label(get!("settings.general.name"));
            ui.add(egui::TextEdit::singleline(&mut player.name).char_limit(NAME_LIMIT));
            ui.end_row();

            ui.label(get!("settings.general.field_of_view"));
            ui.add(egui::Slider::new(&mut player.field_of_view, player_settings::FIELD_OF_VIEW_RANGE).suffix("\u{b0}"));
            ui.end_row();
        });
        ui.checkbox(&mut player.auto_reload, get!("settings.general.auto_reload"));
    }

    fn graphics(&mut self, ui: &mut Ui) {
        ui.heading(get!("settings.graphics.debris"));
        ui.add_space(5.0);
        let Some(player) = &mut self.player_draft else { return };
        let debris = &mut player.debris;
        egui::Grid::new("settings_debris").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            for (label, value) in [
                (get!("settings.graphics.bullet_holes"), &mut debris.bullet_holes),
                (get!("settings.graphics.blood"), &mut debris.blood),
                (get!("settings.graphics.scorch"), &mut debris.scorch),
            ] {
                ui.label(label);
                ui.add(egui::Slider::new(value, player_settings::DEBRIS_RANGE));
                ui.end_row();
            }
        });
    }

    fn sound(&mut self, ui: &mut Ui) {
        ui.heading(get!("settings.sound.volume"));
        ui.add_space(5.0);
        let Some(player) = &mut self.player_draft else { return };
        let sound = &mut player.sound;
        egui::Grid::new("settings_volume").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            for (label, value) in [
                (get!("settings.sound.master"), &mut sound.master),
                (get!("settings.sound.effects"), &mut sound.effects),
                (get!("settings.sound.music"), &mut sound.music),
            ] {
                ui.label(label);
                ui.add(egui::Slider::new(value, player_settings::VOLUME_RANGE).suffix("%"));
                ui.end_row();
            }
        });
        ui.checkbox(&mut sound.mute_music_on_lan, get!("settings.sound.mute_music_on_lan"));
    }

    fn developer(&mut self, ui: &mut Ui) {
        ui.heading(get!("settings.developer.icons"));
        ui.add_space(5.0);
        icon_grid(ui, &self.icons);
    }

    /// A modal, so the draft cannot be edited while it shows.
    fn ask_about_unsaved(&mut self, ctx: &egui::Context, now: &mut InForce) {
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
                        self.apply(now);
                        self.asking = false;
                        // A failed save keeps the window open with the problem showing.
                        self.open = self.problem.is_some();
                    }
                });
            });
        });
    }
}

#[derive(Clone, Copy)]
struct Credited<'a> {
    music: &'a MusicLibrary,
    fonts: &'a PackFonts,
}

fn fonts_page(ui: &mut Ui, sample: &mut String, fonts: &PackFonts) {
    ui.add(egui::TextEdit::singleline(sample).desired_width(f32::INFINITY));
    ui.add_space(GAP.into());
    let lower = sample.to_lowercase();
    let upper = sample.to_uppercase();
    egui::ScrollArea::vertical().id_salt(Page::Fonts).auto_shrink(false).show(ui, |ui| {
        for font in &fonts.fonts {
            ui.push_id(&font.folder, |ui| font_sample(ui, font, &lower, &upper));
            ui.separator();
        }
    });
}

fn font_sample(ui: &mut Ui, font: &PackFont, lower: &str, upper: &str) {
    let family = fonts::family(ui.ctx(), font);
    let in_font = |text: &str, size: f32| match &family {
        Some(family) => egui::RichText::new(text).font(egui::FontId::new(size, family.clone())),
        None => egui::RichText::new(text).size(size),
    };
    let name = &font.attribution.name;
    ui.horizontal(|ui| {
        ui.label(name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(in_font(name, FONT_NAME_SIZE)));
    });
    ui.label(in_font(lower, SAMPLE_SIZE));
    ui.label(in_font(upper, SAMPLE_SIZE));
}

const ICON_CELL: f32 = 84.0;
const ICON_SIZE: f32 = 32.0;

/// A name too long for its cell is cut short; egui shows it whole on hover.
fn icon_grid(ui: &mut Ui, names: &[String]) {
    let color = ui.visuals().text_color();
    // Every cell its full size and aligned to the top: a wrapped row centers
    // each item on the row's height so far, so a taller cell pushes the next
    // one down.
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
                        ui.add(image.fit_to_exact_size(egui::Vec2::splat(ICON_SIZE)).tint(color));
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

/// Reads the directories, so native only; a browser build would read a
/// manifest.
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

/// For a menu that has only a `Ui`.
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
            .init_resource::<MusicLibrary>()
            .init_resource::<PackFonts>()
            .add_systems(PreUpdate, capture_binding.after(InputSystems).before(ResolveActions))
            .add_systems(Update, open_from_the_keyboard.run_if(authoring))
            .add_systems(EguiPrimaryContextPass, draw_settings);
    }
}

/// Also notes an Escape for [`draw_settings`], read here because a binding being waited for
/// takes it in this system, and that press must not close the window too.
fn capture_binding(
    mut window: ResMut<SettingsWindow>,
    keys: Res<ButtonInput<KeyCode>>,
    keymap: Res<Keymap>,
    mut messages: MessageReader<KeyboardInput>,
) {
    let window = &mut *window;
    let waiting = window.sheet.is_capturing();
    window.sheet.capture_from(&mut window.keys_draft, &keys, &mut messages);
    // `Actions` is quiet while the window is open, so the table is read here.
    window.escaped = window.open
        && !waiting
        && !window.typing
        && Actions::read(&keymap, Some(&keys), None, false).just_pressed(Action::Cancel);
}

/// Insert to open the Settings window on the next egui pass, from code with no `egui::Context`.
#[derive(Resource, Default)]
pub struct OpenSettings;

fn open_from_the_keyboard(mut commands: Commands, actions: Res<Actions>) {
    if actions.just_pressed(Action::Settings) {
        commands.insert_resource(OpenSettings);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_settings(
    mut commands: Commands,
    mut contexts: EguiContexts,
    mut window: ResMut<SettingsWindow>,
    mut settings: ResMut<Settings>,
    mut keymap: ResMut<Keymap>,
    mut player: Option<ResMut<PlayerSettings>>,
    mut quiet: ResMut<Quiet>,
    keys: Res<ButtonInput<KeyCode>>,
    requested: Option<Res<OpenSettings>>,
    mode: Option<Res<State<AppMode>>>,
    (music, pack_fonts): (Res<MusicLibrary>, Res<PackFonts>),
    (mut published, mut last_mode): (Local<bool>, Local<Option<AppMode>>),
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        // No window can be drawn, so nothing may stay quiet for it.
        quiet.set_if_neq(Quiet(false));
        return;
    };
    if keymap.is_changed() || !*published {
        keybind::publish(ctx, &keymap);
        *published = true;
    }
    // Starting a round grabs the cursor, so a window carried into Play could not be closed and
    // would keep every action quiet. Opened from the pause menu in Play, the menu lends the mouse.
    let current = mode.map(|mode| *mode.get());
    if current == Some(AppMode::Play) && *last_mode != current {
        window.dismiss();
    }
    *last_mode = current;
    // Written through only by Apply, so drawing is not an edit.
    let mut now = InForce {
        settings: settings.bypass_change_detection(),
        keymap: keymap.bypass_change_detection(),
        player: player.as_mut().map(|player| player.bypass_change_detection()),
    };
    if take_request(ctx) || requested.is_some() {
        commands.remove_resource::<OpenSettings>();
        window.open(ctx, &now);
    }
    if std::mem::take(&mut window.escaped) {
        window.escape(&now);
    }
    if window.is_open() {
        let packs = Credited { music: &music, fonts: &pack_fonts };
        window.show(ctx, &mut now, Held::of(&keys), packs);
        if std::mem::take(&mut window.keys_applied) {
            keymap.set_changed();
        }
        if std::mem::take(&mut window.player_applied) {
            if let Some(player) = &mut player {
                player.set_changed();
            }
        }
    }
    window.typing = ctx.text_edit_focused();
    quiet.set_if_neq(Quiet(window.is_open()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_egui::egui::{Pos2, RawInput, Rect};

    /// Each text painted, with its rect.
    fn frame(ctx: &egui::Context, window: &mut SettingsWindow) -> Vec<(String, Rect)> {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 900.0))),
            ..Default::default()
        };
        let mut texts = Vec::new();
        let (mut settings, mut keymap, mut player) = (Settings::default(), Keymap::default(), PlayerSettings::default());
        // Inside the frame, because `Context::run` drains the paint lists.
        let mut output = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            let (music, fonts) = (MusicLibrary::default(), PackFonts::default());
            let mut now = InForce { settings: &mut settings, keymap: &mut keymap, player: Some(&mut player) };
            window.show(&ctx, &mut now, Held::default(), Credited { music: &music, fonts: &fonts });
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
        let (mut settings, mut keymap, mut player) = (Settings::default(), Keymap::default(), PlayerSettings::default());
        window.open(&ctx, &InForce { settings: &mut settings, keymap: &mut keymap, player: Some(&mut player) });

        for (page, mine, other) in [
            (Page::General, get!("settings.general.language"), get!("settings.developer.icons")),
            (Page::Graphics, get!("settings.graphics.debris"), get!("settings.general.language")),
            (Page::Sound, get!("settings.sound.volume"), get!("settings.graphics.debris")),
            (Page::Shortcuts, get!("keybind.sheet.heading"), get!("settings.general.language")),
            (Page::Developer, get!("settings.developer.icons"), get!("keybind.sheet.heading")),
            (Page::Attribution, get!("credits.music"), get!("settings.developer.icons")),
            (Page::Fonts, get!("settings.fonts.sample"), get!("settings.developer.icons")),
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
        let (mut settings, mut keymap, mut player) = (Settings::default(), Keymap::default(), PlayerSettings::default());
        window.open(&ctx, &InForce { settings: &mut settings, keymap: &mut keymap, player: Some(&mut player) });
        window.page = Page::Shortcuts;
        let mut texts = Vec::new();
        for _ in 0..5 {
            texts = frame(&ctx, &mut window);
        }
        // Only those scrolled into view are painted, so check the first context's.
        for &action in Action::ALL.iter().filter(|a| a.context() == keybind::Context::Everywhere && !a.is_debug()) {
            assert!(find(&texts, &action.name()).is_some(), "{action:?} is not listed");
        }
    }

    /// Fails if a player setting takes effect before Apply, or survives a
    /// Close that discarded it.
    #[test]
    fn player_settings_wait_for_apply_and_close_discards_them() {
        let ctx = egui::Context::default();
        let (mut settings, mut keymap, mut player) = (Settings::default(), Keymap::default(), PlayerSettings::default());
        let mut window = SettingsWindow::default();
        let mut now = InForce { settings: &mut settings, keymap: &mut keymap, player: Some(&mut player) };
        window.open(&ctx, &now);
        let draft = window.player_draft.as_mut().unwrap();
        draft.name = "Ada".into();
        draft.debris.blood = 7;
        assert_eq!(now.player.as_deref(), Some(&PlayerSettings::default()), "the draft reached the settings in force");

        window.close(&now);
        assert!(window.asking, "Close with changes did not ask");
        window.asking = false;
        window.open = false;
        window.open(&ctx, &now);
        assert_eq!(window.player_draft, Some(PlayerSettings::default()), "a discarded draft came back");

        window.player_draft.as_mut().unwrap().name = "  Ada  ".into();
        window.apply(&mut now);
        assert_eq!(now.player.as_deref().unwrap().name, "Ada", "the name was not cleaned");
        assert!(std::mem::take(&mut window.player_applied));
        assert!(!window.has_unsaved_changes(&now));
    }

    fn escape_world() -> World {
        let mut world = World::new();
        world.init_resource::<Messages<KeyboardInput>>();
        world.init_resource::<Keymap>();
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::Escape);
        world.insert_resource(keys);
        world.insert_resource(SettingsWindow { open: true, ..default() });
        world
    }

    fn escaped(world: &mut World) -> bool {
        world.run_system_cached(capture_binding).unwrap();
        world.resource::<SettingsWindow>().escaped
    }

    /// Breaks if Escape stops closing the open window, or is noted while it is shut.
    #[test]
    fn escape_is_noted_only_while_the_window_is_open() {
        let mut world = escape_world();
        assert!(escaped(&mut world), "Escape would not close the window");
        world.resource_mut::<SettingsWindow>().open = false;
        assert!(!escaped(&mut world), "noted with the window shut");
    }

    /// Breaks if the press that stops waiting for a binding also closes the window.
    #[test]
    fn escape_that_stops_a_binding_does_not_close_the_window() {
        let mut world = escape_world();
        world.resource_mut::<SettingsWindow>().sheet.begin(Action::TeamPicker, 0);
        assert!(!escaped(&mut world));
    }

    /// Breaks if Escape in a text field closes the window rather than leaving the field.
    #[test]
    fn escape_in_a_text_field_does_not_close_the_window() {
        let mut world = escape_world();
        world.resource_mut::<SettingsWindow>().typing = true;
        assert!(!escaped(&mut world));
    }

    /// Breaks if Escape discards changes, keeps a clean window open, or leaves the question up.
    #[test]
    fn escape_closes_asks_or_drops_the_question() {
        let (mut settings, mut keymap) = (Settings::default(), Keymap::default());
        let mut window = SettingsWindow { open: true, draft: settings.clone(), keys_draft: keymap.clone(), ..default() };
        let now = InForce { settings: &mut settings, keymap: &mut keymap, player: None };
        window.escape(&now);
        assert!(!window.open, "a window with nothing to apply stayed open");

        window.open = true;
        window.draft.language = Some("de-DE".into());
        window.escape(&now);
        assert!(window.open && window.asking, "unapplied changes were dropped without asking");
        window.escape(&now);
        assert!(window.open && !window.asking, "Escape on the question did not keep the window");
    }

    #[test]
    fn the_developer_page_lists_the_shipped_icons() {
        let names = icon_names();
        assert!(names.iter().any(|name| name == icons::FALLBACK), "{names:?}");
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]), "not sorted and unique");
    }
}
