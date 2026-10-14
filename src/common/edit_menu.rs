//! The menu bar's **Edit** pulldown, shared by every authoring mode, just to
//! the right of **File**.
//!
//! Here for the reason [`file_menu`] is: the same entries in the same order
//! with the same names in every mode. Undo had been a pair of buttons in the
//! prop editor's tree, a history panel in the map editor, and a chord only in
//! the animation editor — three answers to "where is undo".
//!
//! **What the entries mean is the mode's business; whether each can be done
//! right now is too.** Unlike the File menu, every entry here can be
//! legitimately impossible at a given moment — nothing to undo, nothing to
//! redo, nothing saved to go back to — so each mode passes an
//! [`EditAvailability`] and an unavailable entry is greyed out rather than
//! offered and ignored. That is a statement about the document's state, which
//! is what a greyed entry is for.
//!
//! [`file_menu`]: crate::common::file_menu

use bevy_egui::egui;
use bevy_egui::egui::UiKind;

use crate::get;

/// What the Edit menu was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    Undo,
    Redo,
    /// Back to what is on disk. **One undo step wherever the document's
    /// history allows it**, so a revert clicked by mistake is a `Ctrl+Z` away
    /// rather than a cliff; see each mode's handler for where it cannot.
    Revert,
}

/// Every action, in the order the menu offers them. Revert below a separator:
/// undo and redo walk the history, revert jumps out of it.
pub const ALL: &[EditAction] = &[EditAction::Undo, EditAction::Redo, EditAction::Revert];

/// Which entries can be done right now, as the mode in front sees it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditAvailability {
    pub undo: bool,
    pub redo: bool,
    /// There is a saved file to go back to. A document never written has none.
    pub revert: bool,
}

impl EditAvailability {
    pub fn allows(self, action: EditAction) -> bool {
        match action {
            EditAction::Undo => self.undo,
            EditAction::Redo => self.redo,
            EditAction::Revert => self.revert,
        }
    }
}

impl EditAction {
    /// The lang key this entry's label is under.
    pub fn name_key(self) -> &'static str {
        match self {
            EditAction::Undo => "edit.menu.undo",
            EditAction::Redo => "edit.menu.redo",
            EditAction::Revert => "edit.menu.revert",
        }
    }

    /// The chord that does the same thing, for the hint down the right of the
    /// entry. Text, as [`FileAction::chord`] is, and guarded against drifting
    /// from the keyboard the same way.
    ///
    /// Revert has none on purpose: it throws edits away, and a chord is a
    /// thing you hit by accident on the way to something else.
    ///
    /// [`FileAction::chord`]: crate::common::file_menu::FileAction::chord
    pub fn chord(self) -> Option<&'static str> {
        match self {
            EditAction::Undo => Some("Ctrl+Z"),
            EditAction::Redo => Some("Ctrl+Shift+Z"),
            EditAction::Revert => None,
        }
    }
}

/// Draw the pulldown, recording what was picked.
pub fn edit_menu(ui: &mut egui::Ui, available: EditAvailability, request: &mut Option<EditAction>) {
    ui.menu_button(get!("edit.menu.title"), |ui| {
        for action in ALL {
            if *action == EditAction::Revert {
                ui.separator();
            }
            let mut button = egui::Button::new(get!(action.name_key()));
            if let Some(chord) = action.chord() {
                button = button.shortcut_text(chord);
            }
            if ui.add_enabled(available.allows(*action), button).clicked() {
                ui.close_kind(UiKind::Menu);
                *request = Some(*action);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::shortcuts::chords;
    use bevy::prelude::{ButtonInput, KeyCode};

    /// Every entry has a name in the shipped lang file — read from the file,
    /// as the File menu's test does, because the language in force is global
    /// state other tests move around.
    #[test]
    fn every_entry_has_a_name_to_show() {
        let Ok(text) = std::fs::read_to_string("assets/default/lang/en-US.toml") else {
            return;
        };
        let table: toml::Value = toml::from_str(&text).expect("en-US should be valid TOML");

        let mut keys: Vec<&str> = ALL.iter().map(|action| action.name_key()).collect();
        keys.push("edit.menu.title");
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

    /// A chord hint naming a chord nothing listens for is a promise the
    /// keyboard does not keep.
    #[test]
    fn the_menu_only_claims_chords_that_exist() {
        let hit = |held: &[KeyCode], key: KeyCode| {
            let mut keys = ButtonInput::<KeyCode>::default();
            for code in held {
                keys.press(*code);
            }
            keys.press(key);
            chords(&keys, false)
        };
        for action in ALL {
            let Some(chord) = action.chord() else { continue };
            let pressed = match chord {
                "Ctrl+Z" => hit(&[KeyCode::ControlLeft], KeyCode::KeyZ),
                "Ctrl+Shift+Z" => hit(&[KeyCode::ControlLeft, KeyCode::ShiftLeft], KeyCode::KeyZ),
                other => panic!("{action:?} claims '{other}', which this test cannot press"),
            };
            let answered = match action {
                EditAction::Undo => pressed.undo && !pressed.redo,
                EditAction::Redo => pressed.redo,
                EditAction::Revert => false,
            };
            assert!(answered, "{action:?} claims '{chord}', which asks for something else");
        }
    }

    /// Greyed means greyed: availability is read per entry, not as a whole.
    #[test]
    fn each_entry_reads_its_own_availability() {
        let only_redo = EditAvailability { redo: true, ..Default::default() };
        assert!(!only_redo.allows(EditAction::Undo));
        assert!(only_redo.allows(EditAction::Redo));
        assert!(!only_redo.allows(EditAction::Revert));
    }
}
