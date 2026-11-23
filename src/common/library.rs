//! The Library: every file of one kind in every pack (`assets/*/{folder}`), as a grid of previews
//! in a modal that blocks the mode behind it, opened from the File menu. Each mode with one draws
//! it from a system of its own ([`Library::show`]), handing over how its kind is previewed; what
//! is chosen comes back to the mode as a [`FileAction`] ([`Library::take`]).
//!
//! [`NewDialog`] is the question its New button asks — a name, a pack, and for some kinds a little
//! more — and the weapon and lang editors ask it from their own listings too.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy_egui::egui;
use strum::IntoEnumIterator;

use crate::common::assets::{ASSET_ROOT, USER_PACK};
use crate::common::file_menu::FileAction;
use crate::common::mode::GameMode;
use crate::common::preview_grid::{self, Face, Tile};
use crate::common::recent_files::DocumentKind;
use crate::get;
use crate::prop::archetype::Archetype;

const DEFAULT_PACK: &str = "assets/default";
/// Points, the face of a tile in the Library; a picker's are far smaller.
const TILE: f32 = 160.0;
/// Points between the Library and each edge of the window.
const MARGIN: f32 = 100.0;

impl DocumentKind {
    pub fn folder(self) -> &'static str {
        match self {
            DocumentKind::Map => crate::constants::BLUEPRINTS_DIR,
            DocumentKind::Prop => crate::prop::document::PROPS_DIR,
            DocumentKind::Animation => crate::common::clip_file::CLIPS_DIR,
            DocumentKind::Material => crate::material::MATERIALS_DIR,
            DocumentKind::Weapon => crate::common::weapon_file::WEAPONS_DIR,
            DocumentKind::Lang => crate::lang_editor::LANG_SUBDIR,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            DocumentKind::Map => crate::constants::MAP_BLUEPRINT_EXTENSION,
            DocumentKind::Prop => crate::prop::document::PROP_EXTENSION,
            DocumentKind::Animation => crate::common::clip_file::CLIP_EXTENSION,
            DocumentKind::Material => crate::material::FILE_EXTENSION,
            DocumentKind::Weapon => crate::common::weapon_file::WEAPON_EXTENSION,
            DocumentKind::Lang => crate::lang_editor::FILE_EXTENSION,
        }
    }

    /// Which kinds the File menu offers a Library for. The animation and lang editors list their
    /// files beside what they edit instead.
    pub fn has_library(self) -> bool {
        matches!(self, DocumentKind::Map | DocumentKind::Prop | DocumentKind::Material | DocumentKind::Weapon)
    }

    /// Lower case, for the middle of a sentence.
    pub fn noun_key(self) -> &'static str {
        match self {
            DocumentKind::Map => "library.kind.map",
            DocumentKind::Prop => "library.kind.prop",
            DocumentKind::Animation => "library.kind.animation",
            DocumentKind::Material => "library.kind.material",
            DocumentKind::Weapon => "library.kind.weapon",
            DocumentKind::Lang => "library.kind.lang",
        }
    }

    pub fn library_key(self) -> &'static str {
        match self {
            DocumentKind::Map => "library.title.map",
            DocumentKind::Prop => "library.title.prop",
            DocumentKind::Animation => "library.title.animation",
            DocumentKind::Material => "library.title.material",
            DocumentKind::Weapon => "library.title.weapon",
            DocumentKind::Lang => "library.title.lang",
        }
    }

    /// A Library tile's face, so a picture is not stretched: a map's is a 3:2 postcard.
    fn tile_size(self) -> egui::Vec2 {
        match self {
            DocumentKind::Map => egui::vec2(TILE * 1.5, TILE),
            _ => egui::Vec2::splat(TILE),
        }
    }

    pub fn path_in(self, pack: &Path, name: &str) -> PathBuf {
        pack.join(self.folder()).join(format!("{name}.{}", self.extension()))
    }

    /// Before asking whether it is taken.
    fn name_problem(self, name: &str) -> Option<String> {
        if name.is_empty() {
            return Some(get!("library.new.no_name"));
        }
        let fine = match self {
            DocumentKind::Weapon => crate::common::weapon_file::is_weapon_key(name),
            DocumentKind::Lang => crate::lang_editor::doc::valid_segment(name),
            _ => valid_file_name(name),
        };
        match (fine, self) {
            (true, _) => None,
            (false, DocumentKind::Weapon) => Some(get!("weapon_editor.key.malformed")),
            (false, DocumentKind::Lang) => Some(get!("lang_editor.error.bad_code")),
            (false, _) => Some(get!("library.new.bad_name")),
        }
    }
}

fn valid_file_name(name: &str) -> bool {
    !name.starts_with(' ') && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ' '))
}

/// Every pack on disk, as the New dialog offers them: the user pack, the default pack, then any
/// other folder under `assets/` by name.
pub fn packs() -> Vec<PathBuf> {
    let mut packs = vec![PathBuf::from(USER_PACK), PathBuf::from(DEFAULT_PACK)];
    let mut others: Vec<PathBuf> = crate::common::lang::default_packs()
        .into_iter()
        .filter(|pack| !packs.contains(pack))
        .collect();
    others.sort();
    packs.extend(others);
    packs
}

pub fn pack_name(pack: &Path) -> String {
    if pack == Path::new(USER_PACK) {
        get!("library.pack.user")
    } else if pack == Path::new(DEFAULT_PACK) {
        get!("library.pack.default")
    } else {
        pack.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    pub pack: PathBuf,
    /// The file's stem.
    pub name: String,
    pub path: PathBuf,
}

/// Authoring side only: read from disk, as a pack offers no listing of its own.
pub fn scan(kind: DocumentKind) -> Vec<LibraryEntry> {
    let mut packs = packs();
    // A kept pack is opened through its maps, so they are listed though the pack is not searched.
    if kind == DocumentKind::Map {
        packs.extend(crate::common::pack_info::kept_packs());
    }
    let mut entries = Vec::new();
    for pack in &packs {
        let Ok(files) = std::fs::read_dir(pack.join(kind.folder())) else { continue };
        for path in files.filter_map(Result::ok).map(|entry| entry.path()) {
            if path.is_file()
                && path.extension().and_then(|e| e.to_str()) == Some(kind.extension())
                && let Some(name) = path.file_stem().and_then(|stem| stem.to_str())
            {
                entries.push(LibraryEntry { pack: pack.clone(), name: name.to_owned(), path: path.clone() });
            }
        }
    }
    let order = |pack: &Path| packs.iter().position(|of| of == pack);
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(order(&a.pack).cmp(&order(&b.pack))));
    entries
}

/// What a kind is made with beyond its name and pack. Each can be changed afterward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewExtra {
    None,
    Prop(Archetype),
    Map(GameMode),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewItem {
    pub kind: DocumentKind,
    pub pack: PathBuf,
    pub name: String,
    pub path: PathBuf,
    pub extra: NewExtra,
}

impl NewItem {
    /// A new pack has no folder yet.
    pub fn make_folder(&self) -> Result<(), String> {
        let folder = self.pack.join(self.kind.folder());
        let new_pack = !self.pack.is_dir();
        std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        if new_pack {
            crate::common::lang::rescan_packs();
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
enum PackChoice {
    Existing(PathBuf),
    New,
}

pub enum NewOutcome {
    Asking,
    Canceled,
    Create(NewItem),
}

#[derive(Debug, Clone)]
pub struct NewDialog {
    kind: DocumentKind,
    name: String,
    pack: PackChoice,
    new_pack: String,
    extra: NewExtra,
    focused: bool,
}

impl NewDialog {
    pub fn new(kind: DocumentKind) -> Self {
        let extra = match kind {
            DocumentKind::Prop => NewExtra::Prop(Archetype::default()),
            DocumentKind::Map => NewExtra::Map(GameMode::Arena),
            _ => NewExtra::None,
        };
        Self { kind, name: String::new(), pack: PackChoice::Existing(PathBuf::from(USER_PACK)), new_pack: String::new(), extra, focused: false }
    }

    fn chosen_pack(&self) -> PathBuf {
        match &self.pack {
            PackChoice::Existing(pack) => pack.clone(),
            PackChoice::New => Path::new(ASSET_ROOT).join(self.new_pack.trim()),
        }
    }

    fn pack_problem(&self) -> Option<String> {
        let PackChoice::New = self.pack else { return None };
        let name = self.new_pack.trim();
        if name.is_empty() {
            return Some(get!("library.new.no_pack_name"));
        }
        if !valid_file_name(name) {
            return Some(get!("library.new.bad_name"));
        }
        let taken = std::fs::read_dir(ASSET_ROOT)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|entry| entry.path().is_dir() && entry.file_name().to_string_lossy().eq_ignore_ascii_case(name));
        taken.then(|| get!("library.new.pack_exists", "pack", name))
    }

    fn problem(&self, elsewhere: &dyn Fn(&str) -> Option<String>) -> Option<String> {
        let name = self.name.trim();
        if let Some(problem) = self.kind.name_problem(name).or_else(|| self.pack_problem()) {
            return Some(problem);
        }
        let pack = self.chosen_pack();
        if self.kind.path_in(&pack, name).exists() {
            return Some(get!(
                "library.new.exists",
                "type",
                get!(self.kind.noun_key()),
                "name",
                name,
                "pack",
                pack_name(&pack)
            ));
        }
        elsewhere(name)
    }

    /// `elsewhere` adds a kind's own rule about names taken, such as a weapon key in another pack.
    pub fn show(&mut self, ctx: &egui::Context, elsewhere: &dyn Fn(&str) -> Option<String>) -> NewOutcome {
        let mut outcome = NewOutcome::Asking;
        let problem = self.problem(elsewhere);
        let modal = egui::Modal::new(egui::Id::new(("library_new", self.kind.trash_key()))).show(ctx, |ui| {
            ui.set_min_width(340.0);
            ui.heading(get!("library.new.heading", "type", get!(self.kind.noun_key())));
            egui::Grid::new("library_new_grid").num_columns(2).show(ui, |ui| {
                ui.label(get!("library.new.name"));
                let field = ui.text_edit_singleline(&mut self.name);
                if !self.focused {
                    field.request_focus();
                    self.focused = true;
                }
                ui.end_row();

                ui.label(get!("library.new.pack"));
                let shown = match &self.pack {
                    PackChoice::Existing(pack) => pack_name(pack),
                    PackChoice::New => get!("library.pack.new"),
                };
                egui::ComboBox::from_id_salt("library_new_pack").selected_text(shown).show_ui(ui, |ui| {
                    for pack in packs() {
                        let name = pack_name(&pack);
                        ui.selectable_value(&mut self.pack, PackChoice::Existing(pack), name);
                    }
                    ui.selectable_value(&mut self.pack, PackChoice::New, get!("library.pack.new"));
                });
                ui.end_row();
                if self.pack == PackChoice::New {
                    ui.label(get!("library.new.pack_name"));
                    ui.text_edit_singleline(&mut self.new_pack);
                    ui.end_row();
                }

                match &mut self.extra {
                    NewExtra::None => {}
                    NewExtra::Prop(archetype) => {
                        ui.label(get!("library.new.archetype"));
                        egui::ComboBox::from_id_salt("library_new_archetype").selected_text(archetype.name()).show_ui(ui, |ui| {
                            for each in Archetype::ALL {
                                ui.selectable_value(archetype, each, each.name());
                            }
                        });
                        ui.end_row();
                    }
                    NewExtra::Map(mode) => {
                        ui.label(get!("library.new.game_mode"));
                        egui::ComboBox::from_id_salt("library_new_mode").selected_text(mode.to_string()).show_ui(ui, |ui| {
                            for each in GameMode::iter() {
                                ui.selectable_value(mode, each, each.to_string());
                            }
                        });
                        ui.end_row();
                    }
                }
            });
            // Only once there is a name, so a fresh dialog shows no error.
            if let Some(problem) = problem.as_ref().filter(|_| !self.name.trim().is_empty()) {
                ui.colored_label(ui.visuals().error_fg_color, problem);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let create = ui.add_enabled(problem.is_none(), egui::Button::new(get!("library.new.create"))).clicked();
                let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
                if (create || enter) && problem.is_none() {
                    let pack = self.chosen_pack();
                    let name = self.name.trim().to_owned();
                    outcome = NewOutcome::Create(NewItem {
                        kind: self.kind,
                        path: self.kind.path_in(&pack, &name),
                        pack,
                        name,
                        extra: self.extra,
                    });
                }
                if ui.button(get!("library.new.cancel")).clicked() {
                    outcome = NewOutcome::Canceled;
                }
            });
        });
        if matches!(outcome, NewOutcome::Asking) && modal.should_close() {
            outcome = NewOutcome::Canceled;
        }
        outcome
    }
}

struct Showing {
    kind: DocumentKind,
    entries: Vec<LibraryEntry>,
    search: String,
    new: Option<NewDialog>,
}

#[derive(Resource, Default)]
pub struct Library {
    showing: Option<Showing>,
    chosen: Option<(DocumentKind, FileAction)>,
    /// Bumped each time one opens, so a preview cache knows to read again.
    pub opened: u64,
}

impl Library {
    /// Reads the packs again, so a file saved since is there.
    pub fn open(&mut self, kind: DocumentKind) {
        self.showing = Some(Showing { kind, entries: scan(kind), search: String::new(), new: None });
        self.opened += 1;
    }

    pub fn close(&mut self) {
        self.showing = None;
    }

    pub fn showing(&self) -> Option<DocumentKind> {
        self.showing.as_ref().map(|showing| showing.kind)
    }

    pub fn entries(&self) -> &[LibraryEntry] {
        self.showing.as_ref().map_or(&[], |showing| &showing.entries)
    }

    /// What was chosen from `kind`'s Library: an `OpenPath` or a `Create`.
    pub fn take(&mut self, kind: DocumentKind) -> Option<FileAction> {
        match &self.chosen {
            Some((of, _)) if *of == kind => self.chosen.take().map(|(_, action)| action),
            _ => None,
        }
    }

    /// `preview` is an entry's picture, if its kind has one.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        preview: &mut dyn FnMut(&LibraryEntry) -> Option<egui::TextureId>,
        elsewhere: &dyn Fn(&str) -> Option<String>,
    ) {
        let Some(showing) = self.showing.as_mut() else { return };
        let kind = showing.kind;
        let mut chosen = None;
        let mut close = false;
        let viewport = ctx.viewport_rect();
        let modal = egui::Modal::new(egui::Id::new(("library", kind.trash_key()))).show(ctx, |ui| {
            // The frame's own margin is inside the area the modal is given, so it comes off too.
            let frame = ui.style().spacing.window_margin.sum();
            let width = (viewport.width() - 2.0 * MARGIN - frame.x).max(TILE);
            let height = (viewport.height() - 2.0 * MARGIN - frame.y).max(TILE);
            ui.set_width(width);
            ui.set_height(height);
            ui.horizontal(|ui| {
                ui.heading(get!(kind.library_key()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.button(get!("library.close")).clicked();
                    if ui.button(get!("library.new.button")).clicked() {
                        showing.new = Some(NewDialog::new(kind));
                    }
                });
            });
            ui.add(egui::TextEdit::singleline(&mut showing.search).hint_text(get!("editor.picker.search")).desired_width(f32::INFINITY));
            ui.separator();
            let needle = showing.search.trim().to_lowercase();
            let shown: Vec<&LibraryEntry> = showing.entries.iter().filter(|entry| entry.name.to_lowercase().contains(&needle)).collect();
            if shown.is_empty() {
                ui.weak(get!("library.empty"));
            }
            let spacing = ui.spacing().item_spacing.x;
            let tile_size = kind.tile_size();
            let columns = ((width + spacing) / (tile_size.x + spacing)).floor().max(1.0) as usize;
            egui::ScrollArea::vertical().max_height(ui.available_height()).auto_shrink([false, false]).show(ui, |ui| {
                for row in shown.chunks(columns) {
                    ui.horizontal(|ui| {
                        for entry in row {
                            let pack = pack_name(&entry.pack);
                            let face = preview(entry).map_or(Face::NoPreview, Face::Picture);
                            let tooltip = entry.path.display().to_string();
                            let tile = Tile { label: &entry.name, face, badge: Some(&pack), tooltip: &tooltip, selected: false };
                            if preview_grid::tile(ui, tile_size, true, &tile).clicked() {
                                chosen = Some(FileAction::OpenPath(entry.path.clone()));
                            }
                        }
                    });
                }
            });
        });
        if let Some(dialog) = showing.new.as_mut() {
            match dialog.show(ctx, elsewhere) {
                NewOutcome::Asking => {}
                NewOutcome::Canceled => showing.new = None,
                NewOutcome::Create(item) => chosen = Some(FileAction::Create(item)),
            }
        } else if modal.should_close() {
            close = true;
        }
        if let Some(action) = chosen {
            self.chosen = Some((kind, action));
            close = true;
        }
        if close {
            self.showing = None;
        }
    }
}

pub fn showing(kind: DocumentKind) -> impl FnMut(Res<Library>) -> bool + Clone {
    move |library: Res<Library>| library.showing() == Some(kind)
}

/// A Library left open when its mode is left would come back over the next.
fn close_on_leaving_the_mode(mut library: ResMut<Library>) {
    library.close();
}

/// Draws no Library itself: each mode's plugin adds the system that does.
pub struct LibraryPlugin;

impl Plugin for LibraryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Library>()
            .add_systems(Update, close_on_leaving_the_mode.run_if(state_changed::<crate::common::app_mode::AppMode>));
    }
}

/// For plugins that stand alone in tests.
pub fn ensure(app: &mut App) {
    if !app.is_plugin_added::<LibraryPlugin>() {
        app.add_plugins(LibraryPlugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_pack_is_offered_first_and_the_default_second() {
        let packs = packs();
        assert_eq!(packs[0], Path::new(USER_PACK));
        assert_eq!(packs[1], Path::new(DEFAULT_PACK));
    }

    /// Fails if a shipped kind's folder or extension is wrong, which would list nothing.
    #[test]
    fn every_kind_with_shipped_files_finds_them() {
        if !Path::new(DEFAULT_PACK).is_dir() {
            return;
        }
        for kind in crate::common::file_menu::ALL_KINDS {
            let found = scan(*kind);
            assert!(found.iter().any(|entry| entry.pack == Path::new(DEFAULT_PACK)), "{kind:?} lists nothing shipped");
            for entry in found {
                assert_eq!(entry.path, kind.path_in(&entry.pack, &entry.name), "{kind:?}");
            }
        }
    }

    #[test]
    fn a_name_taken_in_the_same_pack_is_refused_by_name() {
        if !Path::new(DEFAULT_PACK).is_dir() {
            return;
        }
        let mut dialog = NewDialog::new(DocumentKind::Prop);
        dialog.pack = PackChoice::Existing(PathBuf::from(DEFAULT_PACK));
        dialog.name = "rocket_launcher".into();
        let problem = dialog.problem(&|_| None).expect("a shipped prop's name was offered");
        assert!(problem.contains("rocket_launcher"), "{problem}");
        dialog.name = "no_such_prop_anywhere".into();
        assert_eq!(dialog.problem(&|_| None), None);
        dialog.pack = PackChoice::New;
        dialog.new_pack = "default".into();
        assert!(dialog.problem(&|_| None).is_some(), "a new pack named as an old one was offered");
    }

    /// Fails if drawing the Library or its New dialog panics, or either closes on its own.
    #[test]
    fn the_library_and_its_new_dialog_draw() {
        let ctx = egui::Context::default();
        let mut library = Library::default();
        library.open(DocumentKind::Prop);
        library.showing.as_mut().unwrap().new = Some(NewDialog::new(DocumentKind::Prop));
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| library.show(ui.ctx(), &mut |_| None, &|_| None));
            output.textures_delta.clear();
        }
        assert_eq!(library.showing(), Some(DocumentKind::Prop));
        assert!(library.showing.as_ref().unwrap().new.is_some(), "the New dialog closed itself");
        assert_eq!(library.take(DocumentKind::Prop), None);
    }

    #[test]
    fn a_weapon_is_named_by_its_key_rule() {
        assert!(DocumentKind::Weapon.name_problem("Has Spaces").is_some());
        assert!(DocumentKind::Weapon.name_problem("scattergun").is_none());
        assert!(DocumentKind::Map.name_problem("My Map").is_none());
        assert!(DocumentKind::Map.name_problem("../escape").is_some());
    }
}
