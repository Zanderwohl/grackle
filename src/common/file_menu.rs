//! The menu bar's **File** pulldown, shared by every authoring mode.
//!
//! Here rather than in any one editor's panel module for the reason
//! [`mode_menu`] is: the same five entries have to be in the same order with
//! the same names in every mode, and three copies is three that drift. They
//! already had — the map editor offered *New, Save, Save As, Load, Quit* in
//! hardcoded English, the prop editor *New, Open, Save, Save As* through the
//! lang layer, and the animation editor a bare **Save** button and no File menu
//! at all. Which is three answers to "where is Save As", and the one you get
//! depends on which thing you happen to be editing.
//!
//! **What the entries mean is the mode's business; that they are there is
//! not.** This produces a [`FileAction`] and nothing else — it has no idea what
//! a blueprint, a prop or a clip set is, and each editor answers all five for
//! whatever it is editing. That is the whole of the consistency on offer: *Save*
//! always saves the thing in front of you.
//!
//! [`mode_menu`]: crate::common::app_mode::mode_menu

use bevy_egui::egui;
use bevy_egui::egui::UiKind;

use crate::common::shortcuts::EditorChords;
use crate::get;

/// What the File menu was asked for.
///
/// Every mode answers all five, which is what makes the menu worth sharing.
/// An entry a mode could not answer would have to be greyed out, and a menu
/// that is a different shape in each mode is the thing this replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAction {
    New,
    Save,
    SaveAs,
    Open,
    Quit,
}

/// Every action, in the order the menu offers them.
///
/// Hand-ordered rather than derived: *New* and *Open* are how you get a
/// document, *Save* and *Save As* are what you do to one, and *Quit* is on its
/// own below a separator because it is the one that cannot be undone.
pub const ALL: &[FileAction] = &[
    FileAction::New,
    FileAction::Save,
    FileAction::SaveAs,
    FileAction::Open,
    FileAction::Quit,
];

impl FileAction {
    /// The lang key this entry's label is under.
    pub fn name_key(self) -> &'static str {
        match self {
            FileAction::New => "file.menu.new",
            FileAction::Save => "file.menu.save",
            FileAction::SaveAs => "file.menu.save_as",
            FileAction::Open => "file.menu.open",
            FileAction::Quit => "file.menu.quit",
        }
    }

    /// Which action the keyboard asked for, if it asked for one.
    ///
    /// **The one translation from a chord to a file action**, so that `Ctrl+S`
    /// in the three editors cannot mean three slightly different things — they
    /// each hand what comes back to the same handler the menu feeds, which is
    /// the whole arrangement in one line per editor.
    ///
    /// *Open* and *Quit* have no chord. `Ctrl+O` is free and could have one;
    /// `Ctrl+Q` is a quit you hit by accident on the way to something else,
    /// and there is nowhere to ask about unsaved work yet.
    pub fn from_chords(chords: EditorChords) -> Option<FileAction> {
        if chords.new {
            Some(FileAction::New)
        } else if chords.save_as {
            // Before the plain save: `Ctrl+Shift+S` sets only `save_as`, but
            // reading them the other way round is how a save-as becomes a
            // silent overwrite if that ever stops being true.
            Some(FileAction::SaveAs)
        } else if chords.save {
            Some(FileAction::Save)
        } else {
            None
        }
    }

    /// The chord that does the same thing, for the hint down the right of the
    /// entry — or nothing, for the two that have none.
    ///
    /// Taken from [`shortcuts`] in spirit rather than in fact: that module
    /// reads the keyboard and this one draws a label, so what is written here
    /// is text. The one risk is the two going out of step, which
    /// `the_menu_only_claims_chords_that_exist` is what guards.
    ///
    /// [`shortcuts`]: crate::common::shortcuts
    pub fn chord(self) -> Option<&'static str> {
        match self {
            FileAction::New => Some("Ctrl+N"),
            FileAction::Save => Some("Ctrl+S"),
            FileAction::SaveAs => Some("Ctrl+Shift+S"),
            FileAction::Open | FileAction::Quit => None,
        }
    }
}

/// Draw the pulldown, recording what was picked.
///
/// `request` rather than a return value, so a caller that also draws the Mode
/// menu collects both into locals and answers them after the borrow on its
/// document has been dropped — which is how both editors' menu bars are
/// already written.
pub fn file_menu(ui: &mut egui::Ui, request: &mut Option<FileAction>) {
    ui.menu_button(get!("file.menu.title"), |ui| {
        for action in ALL {
            // Quit is the only one that throws work away without asking, so it
            // gets the gap. Nobody reaches for *Save As* and hits *Quit*.
            if *action == FileAction::Quit {
                ui.separator();
            }
            let mut button = egui::Button::new(get!(action.name_key()));
            if let Some(chord) = action.chord() {
                button = button.shortcut_text(chord);
            }
            if ui.add(button).clicked() {
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

    /// Every action is offered exactly once. A menu missing one is a mode you
    /// cannot save from; a menu offering one twice is two entries that look
    /// identical and one of them is a typo.
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

    /// Every entry has a name in the shipped lang file.
    ///
    /// A missing key renders as `<file.menu.open>` on screen rather than
    /// panicking, which is the right behaviour and means nothing fails — the
    /// menu just has a word in angle brackets in it. Read out of the file
    /// rather than through `get!`, because the language in force is global
    /// state other tests move around.
    #[test]
    fn every_entry_has_a_name_to_show() {
        let Ok(text) = std::fs::read_to_string("assets/default/lang/en-US.toml") else {
            // Run from somewhere other than the repo root.
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

    /// A chord hint that names a chord nothing listens for is worse than no
    /// hint: it is a promise the keyboard does not keep.
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
                "Ctrl+N" => hit(&[KeyCode::ControlLeft], KeyCode::KeyN),
                "Ctrl+S" => hit(&[KeyCode::ControlLeft], KeyCode::KeyS),
                "Ctrl+Shift+S" => hit(&[KeyCode::ControlLeft, KeyCode::ShiftLeft], KeyCode::KeyS),
                other => panic!("{action:?} claims '{other}', which this test cannot press"),
            };
            // Not merely "a chord fired" but "*this* action came back", which
            // is what every editor acts on. `Ctrl+Shift+S` reaching `Save`
            // would be a save-as that silently overwrote.
            assert_eq!(
                FileAction::from_chords(pressed),
                Some(*action),
                "{action:?} claims '{chord}', which asks for something else",
            );
        }
    }

    /// An idle keyboard asks for nothing. Said out loud because every editor
    /// calls its file handler with whatever this returns, and a `Some` here
    /// would be a save a frame, forever.
    #[test]
    fn nothing_held_asks_for_nothing() {
        assert_eq!(FileAction::from_chords(EditorChords::default()), None);
    }
}
