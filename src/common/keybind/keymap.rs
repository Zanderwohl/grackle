//! Which chords fire which actions, stored in `assets/user/shortcuts.toml`,
//! one list of chords per action:
//!
//! ```toml
//! [bindings]
//! save = ["command+KeyS"]
//! delete = ["Delete", "Backspace"]
//! taunt = []
//! ```
//!
//! An action the file does not name gets its default, so a new action reaches
//! existing files. An empty list stays unbound.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::{Action, Chord};

#[derive(Resource, Clone, PartialEq, Debug)]
pub struct Keymap {
    bindings: BTreeMap<Action, Vec<Chord>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::shipped().clone()
    }
}

static SHIPPED: std::sync::LazyLock<Keymap> = std::sync::LazyLock::new(|| Keymap {
    bindings: Action::ALL.iter().map(|&action| (action, defaults(action))).collect(),
});

fn defaults(action: Action) -> Vec<Chord> {
    use KeyCode as K;
    let key = Chord::key;
    let command = Chord::command;
    match action {
        Action::ToggleMode => vec![key(K::F5)],
        Action::Settings => vec![command(K::Comma)],
        Action::PerfOverlay => vec![key(K::F3)],
        Action::ShowHitboxes => vec![key(K::F4)],
        Action::ShowBones => vec![key(K::F6)],

        Action::New => vec![command(K::KeyN)],
        Action::Save => vec![command(K::KeyS)],
        Action::SaveAs => vec![command(K::KeyS).with_shift()],
        Action::Undo => vec![command(K::KeyZ)],
        Action::Redo => vec![command(K::KeyZ).with_shift(), command(K::KeyY)],
        // The key labeled Delete on a Mac sends Backspace.
        Action::Delete => vec![key(K::Delete), key(K::Backspace)],
        Action::Modify => vec![key(K::ShiftLeft), key(K::ShiftRight)],
        Action::Alternate => vec![key(K::AltLeft), key(K::AltRight)],
        Action::Cancel => vec![key(K::Escape)],
        Action::Confirm => vec![key(K::Enter), key(K::NumpadEnter)],

        Action::FlyForward => vec![key(K::KeyW)],
        Action::FlyBack => vec![key(K::KeyS)],
        Action::FlyLeft => vec![key(K::KeyA)],
        Action::FlyRight => vec![key(K::KeyD)],
        Action::FlyUp => vec![key(K::KeyE)],
        Action::FlyDown => vec![key(K::KeyQ)],

        Action::FrameProp => vec![key(K::KeyF)],
        Action::ResetView => vec![key(K::Numpad0)],
        Action::SketchLine => vec![key(K::KeyL)],
        Action::SketchErase => vec![key(K::KeyK)],

        Action::PlayClip => vec![key(K::Space)],
        Action::SetKey => vec![key(K::KeyK)],

        Action::MoveForward => vec![key(K::KeyW)],
        Action::MoveBack => vec![key(K::KeyS)],
        Action::MoveLeft => vec![key(K::KeyA)],
        Action::MoveRight => vec![key(K::KeyD)],
        Action::Jump => vec![key(K::Space)],
        Action::Crouch => vec![key(K::ControlLeft), key(K::ControlRight)],
        Action::Sprint => vec![key(K::ShiftLeft), key(K::ShiftRight)],
        Action::Fire => vec![Chord::mouse(MouseButton::Left)],
        Action::AltFire => vec![Chord::mouse(MouseButton::Right)],
        Action::Reload => vec![key(K::KeyR)],
        Action::Bucket1 => vec![key(K::Digit1)],
        Action::Bucket2 => vec![key(K::Digit2)],
        Action::Bucket3 => vec![key(K::Digit3)],
        Action::Bucket4 => vec![key(K::Digit4)],
        Action::Bucket5 => vec![key(K::Digit5)],
        Action::Bucket6 => vec![key(K::Digit6)],
        Action::Bucket7 => vec![key(K::Digit7)],
        Action::Bucket8 => vec![key(K::Digit8)],
        Action::Bucket9 => vec![key(K::Digit9)],
        Action::ToggleView => vec![key(K::KeyF)],
        Action::Scoreboard => vec![key(K::Tab)],
        Action::TeamPicker => vec![key(K::Period)],
        Action::Taunt => vec![key(K::KeyG)],
        Action::Spray => vec![key(K::KeyT)],
        Action::Pause => vec![key(K::Escape)],
        Action::Humiliate => vec![key(K::KeyH)],
        Action::Suicide => vec![key(K::KeyO)],
        Action::SeatBot => vec![key(K::KeyB)],
        Action::RetireBot => vec![key(K::KeyB).with_shift()],
        Action::NavOverlay => vec![key(K::F7)],
        Action::BotRoutes => vec![key(K::F8)],
        Action::SafeZone => vec![key(K::F9)],
    }
}

/// Plain strings, so a bad entry costs that entry and not the whole file.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct KeymapFile {
    bindings: BTreeMap<String, Vec<String>>,
}

pub fn path() -> PathBuf {
    std::path::Path::new(crate::common::assets::USER_PACK).join("shortcuts.toml")
}

impl Keymap {
    /// A bad file or entry is logged and dropped, never a reason not to start.
    pub fn load() -> Self {
        let path = path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                warn!("Could not read {}: {e}; the default shortcuts stand", path.display());
                return Self::default();
            }
        };
        match Self::parse(&text) {
            Ok((keymap, problems)) => {
                for problem in problems {
                    warn!("{}: {problem}", path.display());
                }
                keymap
            }
            Err(e) => {
                warn!("{} will not read, so the default shortcuts stand: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Returns the failure, so the Settings window does not claim a save it did
    /// not make.
    pub fn save(&self) -> Result<(), String> {
        let path = path();
        let text = toml::to_string_pretty(&self.to_file()).map_err(|e| format!("Could not write the shortcuts: {e}"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("Could not write {}: {e}", path.display()))
    }

    fn parse(text: &str) -> Result<(Self, Vec<String>), toml::de::Error> {
        let file: KeymapFile = toml::from_str(text)?;
        let mut problems = Vec::new();
        Ok((Self::from_file(file, &mut problems), problems))
    }

    fn from_file(file: KeymapFile, problems: &mut Vec<String>) -> Self {
        let mut keymap = Self::default();
        for (name, chords) in file.bindings {
            let Some(action) = Action::from_key(&name) else {
                problems.push(format!("unknown action {name:?}; ignored"));
                continue;
            };
            let mut read = Vec::new();
            for text in chords {
                match Chord::from_key(&text) {
                    Some(chord) if chord.is_reserved() => {
                        problems.push(format!("{text:?} for {name} uses Escape or Enter, which are fixed; ignored"))
                    }
                    Some(chord) if !read.contains(&chord) => read.push(chord),
                    Some(_) => {}
                    None => problems.push(format!("unknown chord {text:?} for {name}; ignored")),
                }
            }
            keymap.bindings.insert(action, read);
        }
        keymap
    }

    /// Fixed actions are left out.
    fn to_file(&self) -> KeymapFile {
        let bindings = self
            .bindings
            .iter()
            .filter(|(action, _)| !action.is_fixed())
            .map(|(action, chords)| (action.to_key().to_owned(), chords.iter().map(|chord| chord.to_key()).collect()))
            .collect();
        KeymapFile { bindings }
    }

    /// Built once.
    pub fn shipped() -> &'static Keymap {
        &SHIPPED
    }

    pub fn chords(&self, action: Action) -> &[Chord] {
        self.bindings.get(&action).map(Vec::as_slice).unwrap_or_default()
    }

    /// The other actions, in overlapping contexts, that `chord` also fires.
    pub fn clashes(&self, action: Action, chord: Chord) -> Vec<Action> {
        Action::ALL
            .iter()
            .copied()
            .filter(|other| *other != action && other.context().overlaps(action.context()))
            .filter(|other| self.chords(*other).contains(&chord))
            .collect()
    }

    /// A `slot` past the end adds a chord. Refuses a fixed action, Escape,
    /// Enter and a duplicate.
    pub fn bind(&mut self, action: Action, slot: usize, chord: Chord) -> bool {
        if chord.is_reserved() || action.is_fixed() {
            return false;
        }
        let chords = self.bindings.entry(action).or_default();
        if chords.contains(&chord) {
            return false;
        }
        match chords.get_mut(slot) {
            Some(existing) => *existing = chord,
            None => chords.push(chord),
        }
        true
    }

    pub fn unbind(&mut self, action: Action, slot: usize) {
        if action.is_fixed() {
            return;
        }
        if let Some(chords) = self.bindings.get_mut(&action)
            && slot < chords.len()
        {
            chords.remove(slot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> (Keymap, Vec<String>) {
        Keymap::parse(text).expect("the fixture does not parse")
    }

    /// Fails if a fresh install has two actions on one chord where both are live.
    #[test]
    fn no_two_defaults_clash() {
        let keymap = Keymap::default();
        for &action in Action::ALL {
            for &chord in keymap.chords(action) {
                let clashes = keymap.clashes(action, chord);
                assert!(clashes.is_empty(), "{action:?} shares {} with {clashes:?}", chord.to_key());
            }
        }
    }

    #[test]
    fn the_file_reads_back_what_it_wrote() {
        let mut keymap = Keymap::default();
        keymap.unbind(Action::Taunt, 0);
        keymap.bind(Action::Save, 1, Chord::key(KeyCode::F2));
        let text = toml::to_string_pretty(&keymap.to_file()).unwrap();
        let (back, problems) = read(&text);
        assert_eq!(back, keymap);
        assert!(problems.is_empty(), "{problems:?}");
    }

    /// Fails if an unbound action gets its default back on the next launch.
    #[test]
    fn an_empty_list_stays_unbound_and_a_missing_one_is_the_default() {
        let (keymap, _) = read("[bindings]\ntaunt = []\n");
        assert!(keymap.chords(Action::Taunt).is_empty());
        assert_eq!(keymap.chords(Action::Save), Keymap::default().chords(Action::Save));
    }

    /// Fails if one bad entry costs more than itself, or a file can bind Escape.
    #[test]
    fn a_bad_entry_is_dropped_and_the_rest_applies() {
        let (keymap, problems) =
            read("[bindings]\nsave = [\"command+KeyS\", \"command+Nope\", \"Escape\"]\npause = [\"Space\"]\nfly = [\"KeyF\"]\n");
        assert_eq!(keymap.chords(Action::Save), &[Chord::command(KeyCode::KeyS)]);
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert_eq!(keymap.chords(Action::Pause), &[Chord::key(KeyCode::Escape)]);
    }

    #[test]
    fn binding_refuses_escape_enter_fixed_actions_and_duplicates() {
        let mut keymap = Keymap::default();
        assert!(!keymap.bind(Action::Jump, 0, Chord::key(KeyCode::Escape)));
        assert!(!keymap.bind(Action::Jump, 1, Chord::command(KeyCode::Enter)));
        assert!(!keymap.bind(Action::Jump, 1, Chord::key(KeyCode::Space)), "a duplicate was added");
        assert!(!keymap.bind(Action::Pause, 0, Chord::key(KeyCode::F1)), "a fixed action was bound");
        assert!(keymap.bind(Action::Jump, 1, Chord::mouse(MouseButton::Right)));
        assert_eq!(keymap.chords(Action::Jump).len(), 2);
    }

    /// Fails if `F` in Play clashes with `F` in the prop editor.
    #[test]
    fn a_clash_is_only_where_both_are_live() {
        let mut keymap = Keymap::default();
        assert!(keymap.clashes(Action::ToggleView, Chord::key(KeyCode::KeyF)).is_empty());
        keymap.bind(Action::Taunt, 0, Chord::key(KeyCode::KeyF));
        assert_eq!(keymap.clashes(Action::Taunt, Chord::key(KeyCode::KeyF)), vec![Action::ToggleView]);
    }
}
