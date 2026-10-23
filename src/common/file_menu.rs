//! The menu bar's File pulldown, shared by every authoring mode so the entries
//! cannot drift between them (as with [`mode_menu`]).
//!
//! This produces a [`FileAction`] and nothing else; each editor answers all
//! five for whatever it is editing.
//!
//! [`mode_menu`]: crate::common::app_mode::mode_menu

use std::path::{Path, PathBuf};

use bevy_egui::egui;
use bevy_egui::egui::UiKind;

use crate::common::shortcuts::{Chord, EditorChords};
use crate::get;

/// What the File menu was asked for. Every mode answers all five.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAction {
    New,
    Save,
    SaveAs,
    Open,
    Quit,
}

/// Menu order. Quit sits apart because it is the one that cannot be undone.
pub const ALL: &[FileAction] = &[
    FileAction::New,
    FileAction::Save,
    FileAction::SaveAs,
    FileAction::Open,
    FileAction::Quit,
];

impl FileAction {
    pub fn name_key(self) -> &'static str {
        match self {
            FileAction::New => "file.menu.new",
            FileAction::Save => "file.menu.save",
            FileAction::SaveAs => "file.menu.save_as",
            FileAction::Open => "file.menu.open",
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

    pub fn chord(self) -> Option<Chord> {
        match self {
            FileAction::New => Some(Chord::NEW),
            FileAction::Save => Some(Chord::SAVE),
            FileAction::SaveAs => Some(Chord::SAVE_AS),
            FileAction::Open | FileAction::Quit => None,
        }
    }
}

/// `request` rather than a return value, so a caller drawing several menus
/// can answer them after the borrow on its document is dropped.
pub fn file_menu(ui: &mut egui::Ui, request: &mut Option<FileAction>) {
    ui.menu_button(get!("file.menu.title"), |ui| {
        for action in ALL {
            // Quit throws work away without asking, so it gets the gap.
            if *action == FileAction::Quit {
                ui.separator();
            }
            let mut button = egui::Button::new(get!(action.name_key()));
            if let Some(chord) = action.chord() {
                button = button.shortcut_text(chord.hint());
            }
            if ui.add(button).clicked() {
                ui.close_kind(UiKind::Menu);
                *request = Some(*action);
            }
        }
    });
}

/// Where a file dialog opens: beside the file being edited, or else the
/// highest-priority pack's `subdir`, so a fresh document is saved into the pack
/// rather than wherever the OS last looked. Absolute, because a native dialog
/// handed a relative path can ignore it. `None` when neither exists.
pub fn dialog_directory(current: Option<&Path>, subdir: &str) -> Option<PathBuf> {
    let beside = current.and_then(Path::parent).filter(|dir| !dir.as_os_str().is_empty());
    let pack = || crate::common::assets::default_packs().pop().map(|pack| pack.join(subdir));
    let directory = beside.map(Path::to_path_buf).or_else(pack)?;
    std::path::absolute(&directory).ok().filter(|dir| dir.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::shortcuts::press;

    #[test]
    fn the_menu_offers_each_action_once() {
        for action in ALL {
            assert_eq!(
                ALL.iter().filter(|other| *other == action).count(),
                1,
                "{action:?} is offered more than once",
            );
        }
        assert_eq!(ALL.len(), 5, "an action was added without being offered");
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

        let mut keys: Vec<&str> = ALL.iter().map(|action| action.name_key()).collect();
        keys.push("file.menu.title");
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
        for action in ALL {
            let Some(chord) = action.chord() else { continue };
            assert_eq!(FileAction::from_chords(press(chord)), Some(*action), "{action:?}");
        }
    }

    #[test]
    fn a_dialog_opens_in_the_pack_until_there_is_a_file() {
        // Not run from the repo root.
        if !Path::new("assets/default/props").is_dir() {
            return;
        }
        let fresh = dialog_directory(None, "props").expect("the pack has a props folder");
        assert!(fresh.is_absolute(), "{fresh:?}");
        assert!(fresh.ends_with("assets/default/props"), "{fresh:?}");

        let beside = dialog_directory(Some(Path::new("assets/default/blueprints/new.gmb")), "props");
        assert!(beside.unwrap().ends_with("assets/default/blueprints"), "did not open beside the file");

        assert_eq!(dialog_directory(None, "no-such-folder"), None);
    }

    /// A `Some` here would be a save every frame.
    #[test]
    fn nothing_held_asks_for_nothing() {
        assert_eq!(FileAction::from_chords(EditorChords::default()), None);
    }
}
