//! The keyboard chords both editors answer to.
//!
//! One definition, because the map editor and the prop editor are two surfaces
//! for one habit: `Ctrl+S` has to mean the same thing in both, and a second
//! copy of "is Shift down" is a second copy that drifts.
//!
//! **Nothing fires while egui wants the keyboard.** A feature being renamed, a
//! map's author list being typed into — `Ctrl+Z` there means undo the *text*,
//! and stealing it to undo the document is the kind of thing that loses work
//! rather than the kind that looks wrong.
//!
//! `typing` is passed in rather than read from `EguiWantsInput` here, so the
//! chords can be tested without an egui context and this module does not have
//! to know there is one.

use bevy::prelude::*;

/// What the keyboard asked for this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EditorChords {
    /// `Ctrl+S`. **Means save-as on a document that has never been saved** —
    /// see each editor's handler; a save with nowhere to save to is a question,
    /// not an error and not a silent nothing.
    pub save: bool,
    /// `Ctrl+Shift+S`.
    pub save_as: bool,
    /// `Ctrl+Z`.
    pub undo: bool,
    /// `Ctrl+Shift+Z`, or `Ctrl+Y`.
    pub redo: bool,
}

/// Read the chords, or nothing at all if egui is taking the keystrokes.
///
/// Both `Super` and `Control` count as the modifier, so the same binding works
/// for somebody who learnt it on either platform. `Ctrl+Y` is redo as well,
/// which is the Windows habit and costs nothing to keep.
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

    EditorChords {
        save: modifier && !shift && keys.just_pressed(KeyCode::KeyS),
        save_as: modifier && shift && keys.just_pressed(KeyCode::KeyS),
        undo: modifier && !shift && keys.just_pressed(KeyCode::KeyZ),
        redo: (modifier && shift && keys.just_pressed(KeyCode::KeyZ))
            || (ctrl && keys.just_pressed(KeyCode::KeyY)),
    }
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

    /// The modifier is not optional. Typing `s` into a viewport must not save.
    #[test]
    fn a_bare_letter_is_not_a_chord() {
        let bare = pressed(&[], KeyCode::KeyS, false);
        assert_eq!(bare, EditorChords::default());
        assert_eq!(pressed(&[], KeyCode::KeyZ, false), EditorChords::default());
    }

    /// The one that loses work if it is wrong: `Ctrl+Z` while renaming a
    /// feature means undo the *text*, not throw away the document's last edit.
    #[test]
    fn nothing_fires_while_egui_has_the_keyboard() {
        for key in [KeyCode::KeyS, KeyCode::KeyZ, KeyCode::KeyY] {
            assert_eq!(
                pressed(&[KeyCode::ControlLeft, KeyCode::ShiftLeft], key, true),
                EditorChords::default(),
                "{key:?} fired while somebody was typing",
            );
        }
    }
}
