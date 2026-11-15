//! The document commands every authoring mode answers to, read from
//! [`Actions`] so they mean the same in every editor.

use crate::common::keybind::{Action, Actions};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EditorChords {
    /// Discards the open document without asking, as the menu's New does;
    /// there is nowhere to ask yet.
    pub new: bool,
    /// Means save-as on a document that has never been saved.
    pub save: bool,
    pub save_as: bool,
    pub undo: bool,
    pub redo: bool,
}

pub fn chords(actions: &Actions) -> EditorChords {
    EditorChords {
        new: actions.just_pressed(Action::New),
        save: actions.just_pressed(Action::Save),
        save_as: actions.just_pressed(Action::SaveAs),
        undo: actions.just_pressed(Action::Undo),
        redo: actions.just_pressed(Action::Redo),
    }
}

/// For the menus' tests.
#[cfg(test)]
pub fn press(action: Action) -> EditorChords {
    let mut actions = Actions::default();
    actions.press(action);
    chords(&actions)
}
