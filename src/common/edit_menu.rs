//! The menu bar's Edit pulldown, shared by every authoring mode (as
//! [`file_menu`] is). Each mode passes an [`EditAvailability`], and an entry
//! that cannot be done right now is grayed out rather than ignored.
//!
//! [`file_menu`]: crate::common::file_menu

use bevy_egui::egui;
use bevy_egui::egui::UiKind;

use crate::common::shortcuts::Chord;
use crate::get;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    Undo,
    Redo,
    /// Back to what is on disk. One undo step where the mode's history allows
    /// it; see each mode's handler.
    Revert,
}

/// Revert sits below a separator: undo and redo walk the history, revert
/// jumps out of it.
pub const ALL: &[EditAction] = &[EditAction::Undo, EditAction::Redo, EditAction::Revert];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditAvailability {
    pub undo: bool,
    pub redo: bool,
    /// A document never written has nothing to revert to.
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
    pub fn name_key(self) -> &'static str {
        match self {
            EditAction::Undo => "edit.menu.undo",
            EditAction::Redo => "edit.menu.redo",
            EditAction::Revert => "edit.menu.revert",
        }
    }

    /// Revert has no chord: it throws edits away and a chord is easy to hit
    /// by accident.
    pub fn chord(self) -> Option<Chord> {
        match self {
            EditAction::Undo => Some(Chord::UNDO),
            EditAction::Redo => Some(Chord::REDO),
            EditAction::Revert => None,
        }
    }
}

pub fn edit_menu(ui: &mut egui::Ui, available: EditAvailability, request: &mut Option<EditAction>) {
    ui.menu_button(get!("edit.menu.title"), |ui| {
        for action in ALL {
            if *action == EditAction::Revert {
                ui.separator();
            }
            let mut button = egui::Button::new(get!(action.name_key()));
            if let Some(chord) = action.chord() {
                button = button.shortcut_text(chord.hint());
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
    use crate::common::shortcuts::press;

    /// Read from the file because the language in force is global state other
    /// tests move.
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

    #[test]
    fn each_entrys_chord_asks_for_that_entry() {
        for action in ALL {
            let Some(chord) = action.chord() else { continue };
            let pressed = press(chord);
            let answered = match action {
                EditAction::Undo => pressed.undo && !pressed.redo,
                EditAction::Redo => pressed.redo && !pressed.undo,
                EditAction::Revert => false,
            };
            assert!(answered, "{action:?}");
        }
    }

    #[test]
    fn each_entry_reads_its_own_availability() {
        let only_redo = EditAvailability { redo: true, ..Default::default() };
        assert!(!only_redo.allows(EditAction::Undo));
        assert!(only_redo.allows(EditAction::Redo));
        assert!(!only_redo.allows(EditAction::Revert));
    }
}
