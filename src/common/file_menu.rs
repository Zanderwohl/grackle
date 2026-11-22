//! The menu bar's File pulldown, shared by every authoring mode so the entries
//! cannot drift between them (as with [`mode_menu`]).
//!
//! This produces a [`FileAction`] and nothing else; each editor answers every
//! one for whatever it is editing.
//!
//! [`mode_menu`]: crate::common::app_mode::mode_menu

use std::path::{Path, PathBuf};

use bevy_egui::egui;
use bevy_egui::egui::UiKind;

use crate::common::keybind::{self, Action};
pub use crate::common::recent_files::DocumentKind;
use crate::common::shortcuts::EditorChords;
use crate::get;

/// What the File menu was asked for. Every mode answers each of them for whatever it edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileAction {
    New,
    Open,
    Save,
    SaveAs,
    OpenRecent(PathBuf),
    /// A shipped file, opened untitled so a save asks where rather than writing over it.
    Demo(Demo),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Demo {
    /// From the repo root.
    pub path: &'static str,
    pub name_key: &'static str,
}

pub const DEMO_SCENE: Demo = Demo { path: "assets/default/blueprints/demo.gmb", name_key: "file.menu.demo_scene" };
pub const SHOWCASE: Demo = Demo { path: "assets/default/blueprints/showcase.gmb", name_key: "file.menu.showcase" };

const MAP_DEMOS: &[Demo] = &[DEMO_SCENE, SHOWCASE];
const PROP_DEMOS: &[Demo] = &[
    Demo { path: "assets/default/props/rocket_launcher.gpp", name_key: "file.menu.rocket_launcher" },
    Demo { path: "assets/default/props/health_pack.gpp", name_key: "file.menu.health_pack" },
    Demo { path: "assets/default/props/top_hat.gpp", name_key: "file.menu.top_hat" },
];

impl DocumentKind {
    pub fn demos(self) -> &'static [Demo] {
        match self {
            DocumentKind::Map => MAP_DEMOS,
            DocumentKind::Prop => PROP_DEMOS,
            DocumentKind::Animation | DocumentKind::Material | DocumentKind::Weapon | DocumentKind::Lang => &[],
        }
    }

    pub fn demos_key(self) -> &'static str {
        match self {
            DocumentKind::Map => "file.menu.demo_maps",
            DocumentKind::Prop => "file.menu.demo_props",
            DocumentKind::Animation => "file.menu.demo_animations",
            DocumentKind::Material => "file.menu.demo_materials",
            DocumentKind::Weapon => "file.menu.demo_weapons",
            DocumentKind::Lang => "file.menu.demo_langs",
        }
    }
}

pub const ALL_KINDS: &[DocumentKind] = &[
    DocumentKind::Map,
    DocumentKind::Prop,
    DocumentKind::Animation,
    DocumentKind::Material,
    DocumentKind::Weapon,
    DocumentKind::Lang,
];

enum Entry {
    Action(FileAction),
    Recent,
    Demos,
    Gap,
}

/// The menu, top to bottom. Quit sits apart because it throws work away without asking.
const MENU: &[Entry] = &[
    Entry::Action(FileAction::New),
    Entry::Action(FileAction::Open),
    Entry::Action(FileAction::Save),
    Entry::Action(FileAction::SaveAs),
    Entry::Gap,
    Entry::Recent,
    Entry::Demos,
    Entry::Gap,
    Entry::Action(FileAction::Quit),
];

impl FileAction {
    pub fn name_key(&self) -> &'static str {
        match self {
            FileAction::New => "file.menu.new",
            FileAction::Open => "file.menu.open",
            FileAction::Save => "file.menu.save",
            FileAction::SaveAs => "file.menu.save_as",
            FileAction::OpenRecent(_) => "file.menu.recent",
            FileAction::Demo(demo) => demo.name_key,
            FileAction::Quit => "file.menu.quit",
        }
    }

    /// The one translation from a chord to a file action, so `Ctrl+S` means the
    /// same in every editor.
    ///
    /// Open and Quit have no chord: `Ctrl+Q` is too easy to hit by accident and
    /// there is nowhere yet to ask about unsaved work.
    pub fn from_chords(chords: EditorChords) -> Option<FileAction> {
        if chords.new {
            Some(FileAction::New)
        } else if chords.save_as {
            // Before plain save, so a save-as can never become a silent overwrite.
            Some(FileAction::SaveAs)
        } else if chords.save {
            Some(FileAction::Save)
        } else {
            None
        }
    }

    pub fn chord(&self) -> Option<Action> {
        match self {
            FileAction::New => Some(Action::New),
            FileAction::Save => Some(Action::Save),
            FileAction::SaveAs => Some(Action::SaveAs),
            FileAction::Open | FileAction::OpenRecent(_) | FileAction::Demo(_) | FileAction::Quit => None,
        }
    }

    pub fn opens(&self) -> Option<PathBuf> {
        match self {
            FileAction::OpenRecent(path) => Some(path.clone()),
            FileAction::Demo(demo) => Some(PathBuf::from(demo.path)),
            _ => None,
        }
    }
}

/// `request` rather than a return value, so a caller drawing several menus can answer them after
/// the borrow on its document is dropped. `recent` is what [`RecentFiles`] holds for `kind`;
/// entries whose file is gone are left out.
///
/// [`RecentFiles`]: crate::common::recent_files::RecentFiles
pub fn file_menu(ui: &mut egui::Ui, kind: DocumentKind, recent: &[PathBuf], request: &mut Option<FileAction>) {
    ui.menu_button(get!("file.menu.title"), |ui| {
        for line in MENU {
            match line {
                Entry::Action(action) => entry(ui, action.clone(), request),
                Entry::Gap => {
                    ui.separator();
                }
                Entry::Recent => {
                    let recent: Vec<&PathBuf> = recent.iter().filter(|path| path.is_file()).collect();
                    ui.add_enabled_ui(!recent.is_empty(), |ui| {
                        ui.menu_button(get!("file.menu.recent"), |ui| {
                            for path in recent {
                                let name =
                                    path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                                let button = ui.button(name).on_hover_text(path.display().to_string());
                                if button.clicked() {
                                    ui.close_kind(UiKind::Menu);
                                    *request = Some(FileAction::OpenRecent(path.clone()));
                                }
                            }
                        });
                    });
                }
                Entry::Demos => {
                    let demos = kind.demos();
                    ui.add_enabled_ui(!demos.is_empty(), |ui| {
                        ui.menu_button(get!(kind.demos_key()), |ui| {
                            for demo in demos {
                                entry(ui, FileAction::Demo(*demo), request);
                            }
                        });
                    });
                }
            }
        }
    });
}

fn entry(ui: &mut egui::Ui, action: FileAction, request: &mut Option<FileAction>) {
    let mut button = egui::Button::new(get!(action.name_key()));
    if let Some(hint) = action.chord().and_then(|chord| keybind::hint(ui.ctx(), chord)) {
        button = button.shortcut_text(hint);
    }
    if ui.add(button).clicked() {
        ui.close_kind(UiKind::Menu);
        *request = Some(action);
    }
}

/// Holds a file action that would replace unsaved edits until the player says to go ahead. Each
/// mode keeps one and passes its File requests through [`DiscardQuestion::guard`].
#[derive(Debug, Default, Clone)]
pub struct DiscardQuestion {
    held: Option<FileAction>,
}

impl DiscardQuestion {
    /// `action` as it is, unless it opens a file over unsaved edits: that is held for
    /// [`DiscardQuestion::show`].
    pub fn guard(&mut self, action: Option<FileAction>, dirty: bool) -> Option<FileAction> {
        match action {
            Some(action) if dirty && action.opens().is_some() => {
                self.held = Some(action);
                None
            }
            other => other,
        }
    }

    /// Draws the question while an action is held, and gives the action back once the player
    /// discards.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<FileAction> {
        let path = self.held.as_ref()?.opens()?;
        let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        let (mut discard, mut cancel) = (false, false);
        egui::Window::new(get!("file.discard.heading"))
            .id(egui::Id::new("file_discard_question"))
            .collapsible(false)
            .resizable(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.viewport_rect().center())
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.label(get!("file.discard.body", "file", name));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    discard = ui.button(get!("file.discard.discard")).clicked();
                    cancel = ui.button(get!("file.discard.cancel")).clicked();
                });
            });
        if cancel {
            self.held = None;
        }
        discard.then(|| self.held.take()).flatten()
    }
}

/// Where a file dialog opens: beside the file being edited, or else the user
/// pack's `subdir`, so new work lands in the user pack rather than wherever
/// the OS last looked. Absolute, because a native dialog handed a relative path
/// can ignore it. `None` when neither can be found or made.
pub fn dialog_directory(current: Option<&Path>, subdir: &str) -> Option<PathBuf> {
    let beside = current.and_then(Path::parent).filter(|dir| !dir.as_os_str().is_empty());
    let directory = match beside {
        Some(beside) => beside.to_path_buf(),
        None => {
            let fresh = Path::new(crate::common::assets::USER_PACK).join(subdir);
            // Made at startup too; again here in case it was deleted since.
            std::fs::create_dir_all(&fresh).ok()?;
            fresh
        }
    };
    std::path::absolute(&directory).ok().filter(|dir| dir.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::shortcuts::press;

    /// A new variant stops this compiling; add it to `every` below too.
    fn listed(action: &FileAction) {
        match action {
            FileAction::New
            | FileAction::Open
            | FileAction::Save
            | FileAction::SaveAs
            | FileAction::OpenRecent(_)
            | FileAction::Demo(_)
            | FileAction::Quit => {}
        }
    }

    /// Fails if an action is answered by every mode but drawn nowhere in the menu, or twice.
    #[test]
    fn the_menu_offers_every_action_once() {
        let every = [
            FileAction::New,
            FileAction::Open,
            FileAction::Save,
            FileAction::SaveAs,
            FileAction::OpenRecent(PathBuf::new()),
            FileAction::Demo(DEMO_SCENE),
            FileAction::Quit,
        ];
        for action in &every {
            listed(action);
            let drawn = MENU
                .iter()
                .filter(|line| match (action, line) {
                    (FileAction::OpenRecent(_), Entry::Recent) | (FileAction::Demo(_), Entry::Demos) => true,
                    (
                        FileAction::New | FileAction::Open | FileAction::Save | FileAction::SaveAs | FileAction::Quit,
                        Entry::Action(entry),
                    ) => entry == action,
                    _ => false,
                })
                .count();
            assert_eq!(drawn, 1, "{action:?} is drawn {drawn} times");
        }
    }

    /// Fails if a demo names a file that is not shipped, which would open nothing.
    #[test]
    fn every_demo_is_shipped() {
        if !Path::new("assets/default").is_dir() {
            return;
        }
        for kind in ALL_KINDS {
            for demo in kind.demos() {
                assert!(Path::new(demo.path).is_file(), "{kind:?} offers {} and it is not there", demo.path);
            }
        }
    }

    /// A missing key renders as `<key>` rather than failing. Read from the file
    /// rather than `get!`, because the language in force is global state other
    /// tests move.
    #[test]
    fn every_entry_has_a_name_to_show() {
        let Ok(text) = std::fs::read_to_string("assets/default/lang/en-US.toml") else {
            return;
        };
        let table: toml::Value = toml::from_str(&text).expect("en-US should be valid TOML");

        let mut keys: Vec<&str> = MENU
            .iter()
            .filter_map(|line| match line {
                Entry::Action(action) => Some(action.name_key()),
                _ => None,
            })
            .collect();
        keys.extend(["file.menu.title", "file.menu.recent", "file.menu.quit"]);
        for kind in ALL_KINDS {
            keys.push(kind.demos_key());
            keys.extend(kind.demos().iter().map(|demo| demo.name_key));
        }
        for key in keys {
            let mut at = &table;
            for part in key.split('.') {
                at = at
                    .get(part)
                    .unwrap_or_else(|| panic!("en-US has no `{key}`, so the menu shows the key"));
            }
            assert!(at.as_str().is_some(), "`{key}` is not a string");
        }
    }

    /// `Ctrl+Shift+S` reaching `Save` would be a save-as that silently
    /// overwrote.
    #[test]
    fn each_entrys_chord_asks_for_that_entry() {
        for line in MENU {
            let Entry::Action(action) = line else { continue };
            let Some(chord) = action.chord() else { continue };
            assert_eq!(FileAction::from_chords(press(chord)), Some(action.clone()), "{action:?}");
        }
    }

    #[test]
    fn a_dialog_opens_in_the_user_pack_until_there_is_a_file() {
        // Not run from the repo root.
        if !Path::new("assets/default").is_dir() {
            return;
        }
        let fresh = dialog_directory(None, "props").expect("the user pack's props folder");
        assert!(fresh.is_absolute(), "{fresh:?}");
        assert!(fresh.ends_with("assets/user/props"), "{fresh:?}");

        let beside = dialog_directory(Some(Path::new("assets/default/blueprints/new.gmb")), "props");
        assert!(beside.unwrap().ends_with("assets/default/blueprints"), "did not open beside the file");
    }

    /// Fails if opening over unsaved edits goes ahead unasked, or anything else is held.
    #[test]
    fn only_an_open_over_unsaved_edits_is_held() {
        let mut question = DiscardQuestion::default();
        let recent = FileAction::OpenRecent(PathBuf::from("a.gpp"));
        assert_eq!(question.guard(Some(recent.clone()), false), Some(recent.clone()));
        assert_eq!(question.guard(Some(FileAction::Save), true), Some(FileAction::Save));
        assert_eq!(question.held, None);
        assert_eq!(question.guard(Some(recent.clone()), true), None);
        assert_eq!(question.guard(Some(FileAction::Demo(DEMO_SCENE)), true), None);
        assert_eq!(question.held, Some(FileAction::Demo(DEMO_SCENE)));
    }

    /// A `Some` here would be a save every frame.
    #[test]
    fn nothing_held_asks_for_nothing() {
        assert_eq!(FileAction::from_chords(EditorChords::default()), None);
    }
}
