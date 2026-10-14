//! The keyboard chords both editors answer to, defined once.
//!
//! Nothing fires while egui wants the keyboard: `Ctrl+Z` in a text field means
//! undo the text, and stealing it loses work. `typing` is passed in so this is
//! testable without an egui context.
//!
//! `Super` and `Control` both count as the modifier. `Alt` does not; it
//! composes characters.

use bevy::prelude::*;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EditorChords {
    /// `Ctrl+N`. Discards the open document without asking, as the menu's New
    /// does; there is nowhere to ask yet.
    pub new: bool,
    /// `Ctrl+S`. Means save-as on a document that has never been saved.
    pub save: bool,
    /// `Ctrl+Shift+S`.
    pub save_as: bool,
    /// `Ctrl+Z`.
    pub undo: bool,
    /// `Ctrl+Shift+Z`, or `Ctrl+Y`.
    pub redo: bool,
}

/// A letter with the modifier held, and Shift or not. The menus' hint text is
/// generated from the same value the keyboard is read by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub shift: bool,
    pub key: KeyCode,
}

impl Chord {
    pub const NEW: Chord = Chord { shift: false, key: KeyCode::KeyN };
    pub const SAVE: Chord = Chord { shift: false, key: KeyCode::KeyS };
    pub const SAVE_AS: Chord = Chord { shift: true, key: KeyCode::KeyS };
    pub const UNDO: Chord = Chord { shift: false, key: KeyCode::KeyZ };
    pub const REDO: Chord = Chord { shift: true, key: KeyCode::KeyZ };

    fn pressed(self, keys: &ButtonInput<KeyCode>, modifier: bool, shift: bool) -> bool {
        modifier && shift == self.shift && keys.just_pressed(self.key)
    }

    /// Menu hint text, e.g. `Ctrl+Shift+S`. Says `Ctrl` on macOS too, where
    /// `Super` also counts.
    pub fn hint(self) -> String {
        let name = format!("{:?}", self.key);
        let letter = name.strip_prefix("Key").unwrap_or(&name);
        match self.shift {
            true => format!("Ctrl+Shift+{letter}"),
            false => format!("Ctrl+{letter}"),
        }
    }
}

/// All false while `typing`. `Ctrl+Y` is also redo, the Windows habit.
pub fn chords(keys: &ButtonInput<KeyCode>, typing: bool) -> EditorChords {
    if typing {
        return EditorChords::default();
    }

    let modifier = keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight)
        || keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    let pressed = |chord: Chord| chord.pressed(keys, modifier, shift);
    EditorChords {
        new: pressed(Chord::NEW),
        save: pressed(Chord::SAVE),
        save_as: pressed(Chord::SAVE_AS),
        undo: pressed(Chord::UNDO),
        redo: pressed(Chord::REDO) || (ctrl && keys.just_pressed(KeyCode::KeyY)),
    }
}

/// What `chord` reads as with Control held. For the menus' tests.
#[cfg(test)]
pub fn press(chord: Chord) -> EditorChords {
    let mut keys = ButtonInput::<KeyCode>::default();
    keys.press(KeyCode::ControlLeft);
    if chord.shift {
        keys.press(KeyCode::ShiftLeft);
    }
    keys.press(chord.key);
    chords(&keys, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pressed(held: &[KeyCode], hit: KeyCode, typing: bool) -> EditorChords {
        let mut keys = ButtonInput::<KeyCode>::default();
        for key in held {
            keys.press(*key);
        }
        keys.clear_just_pressed(hit);
        keys.press(hit);
        chords(&keys, typing)
    }

    #[test]
    fn shift_is_what_separates_save_from_save_as() {
        let save = pressed(&[KeyCode::ControlLeft], KeyCode::KeyS, false);
        assert!(save.save && !save.save_as);

        let save_as = pressed(&[KeyCode::ControlLeft, KeyCode::ShiftLeft], KeyCode::KeyS, false);
        assert!(save_as.save_as && !save_as.save, "shift+S still read as a plain save");
    }

    #[test]
    fn undo_and_redo_are_the_usual_three_ways() {
        assert!(pressed(&[KeyCode::SuperLeft], KeyCode::KeyZ, false).undo);
        assert!(pressed(&[KeyCode::SuperLeft, KeyCode::ShiftLeft], KeyCode::KeyZ, false).redo);
        assert!(pressed(&[KeyCode::ControlLeft], KeyCode::KeyY, false).redo);
        assert!(!pressed(&[KeyCode::SuperLeft, KeyCode::ShiftLeft], KeyCode::KeyZ, false).undo);
    }

    /// Typing `s` into a viewport must not save.
    #[test]
    fn a_bare_letter_is_not_a_chord() {
        let bare = pressed(&[], KeyCode::KeyS, false);
        assert_eq!(bare, EditorChords::default());
        assert_eq!(pressed(&[], KeyCode::KeyZ, false), EditorChords::default());
        assert_eq!(pressed(&[], KeyCode::KeyN, false), EditorChords::default());
    }

    /// `Ctrl+Shift+N` must not make a New out of somebody reaching for
    /// something else.
    #[test]
    fn new_is_the_unshifted_n() {
        assert!(pressed(&[KeyCode::ControlLeft], KeyCode::KeyN, false).new);
        assert!(pressed(&[KeyCode::SuperLeft], KeyCode::KeyN, false).new);
        assert!(!pressed(&[KeyCode::ControlLeft, KeyCode::ShiftLeft], KeyCode::KeyN, false).new);
    }

    #[test]
    fn a_hint_names_the_modifier_shift_and_letter() {
        assert_eq!(Chord::SAVE.hint(), "Ctrl+S");
        assert_eq!(Chord::SAVE_AS.hint(), "Ctrl+Shift+S");
    }

    /// `Ctrl+Z` while renaming a feature means undo the text.
    #[test]
    fn nothing_fires_while_egui_has_the_keyboard() {
        for key in [KeyCode::KeyS, KeyCode::KeyZ, KeyCode::KeyY, KeyCode::KeyN] {
            assert_eq!(
                pressed(&[KeyCode::ControlLeft, KeyCode::ShiftLeft], key, true),
                EditorChords::default(),
                "{key:?} fired while somebody was typing",
            );
        }
    }
}
