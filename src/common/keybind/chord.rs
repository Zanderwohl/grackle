//! One key or mouse button with its modifiers.
//!
//! Keys are Bevy's `KeyCode`s, which name a position on the keyboard rather
//! than the letter printed on it, so WASD stays in place on any layout.
//! `command` is Ctrl or Super, whichever is held.

use bevy::prelude::*;

use crate::get;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Input {
    Key(KeyCode),
    Mouse(MouseButton),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Chord {
    pub input: Input,
    /// Ctrl or Super.
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Held {
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Held {
    pub fn of(keys: &ButtonInput<KeyCode>) -> Self {
        keys.get_pressed().filter_map(|key| modifier_of(*key)).fold(Self::default(), |mut held, modifier| {
            match modifier {
                Modifier::Command => held.command = true,
                Modifier::Shift => held.shift = true,
                Modifier::Alt => held.alt = true,
            }
            held
        })
    }

    pub fn count(self) -> usize {
        self.command as usize + self.shift as usize + self.alt as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Modifier {
    Command,
    Shift,
    Alt,
}

pub fn modifier_of(key: KeyCode) -> Option<Modifier> {
    match key {
        KeyCode::ControlLeft | KeyCode::ControlRight | KeyCode::SuperLeft | KeyCode::SuperRight => Some(Modifier::Command),
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Some(Modifier::Shift),
        KeyCode::AltLeft | KeyCode::AltRight => Some(Modifier::Alt),
        _ => None,
    }
}

impl Chord {
    pub const fn key(key: KeyCode) -> Self {
        Self { input: Input::Key(key), command: false, shift: false, alt: false }
    }

    pub const fn mouse(button: MouseButton) -> Self {
        Self { input: Input::Mouse(button), command: false, shift: false, alt: false }
    }

    pub const fn command(key: KeyCode) -> Self {
        Self { command: true, ..Self::key(key) }
    }

    pub const fn with_shift(self) -> Self {
        Self { shift: true, ..self }
    }

    pub const fn with_alt(self) -> Self {
        Self { alt: true, ..self }
    }

    pub fn modifiers(self) -> Held {
        Held { command: self.command, shift: self.shift, alt: self.alt }
    }

    /// Takes out this chord's own key: a bare Shift binding is pressed with
    /// Shift held, which is not an extra modifier.
    fn others(self, held: Held) -> Held {
        let Input::Key(key) = self.input else { return held };
        match modifier_of(key) {
            Some(Modifier::Command) => Held { command: false, ..held },
            Some(Modifier::Shift) => Held { shift: false, ..held },
            Some(Modifier::Alt) => Held { alt: false, ..held },
            None => held,
        }
    }

    /// `exact` asks for these modifiers and no others; otherwise these and
    /// possibly more.
    pub fn modifiers_match(self, held: Held, exact: bool) -> bool {
        let held = self.others(held);
        let wanted = self.modifiers();
        match exact {
            true => held == wanted,
            false => (!wanted.command || held.command) && (!wanted.shift || held.shift) && (!wanted.alt || held.alt),
        }
    }

    /// Escape and Enter belong to [`super::Action::FIXED`], with any modifiers.
    pub fn is_reserved(self) -> bool {
        matches!(self.input, Input::Key(KeyCode::Escape | KeyCode::Enter | KeyCode::NumpadEnter))
    }

    /// `Ctrl` stands for Super too.
    pub fn label(self) -> String {
        let mut parts = Vec::with_capacity(4);
        if self.command {
            parts.push(get!("keybind.modifier.command"));
        }
        if self.alt {
            parts.push(get!("keybind.modifier.alt"));
        }
        if self.shift {
            parts.push(get!("keybind.modifier.shift"));
        }
        parts.push(input_label(self.input));
        parts.join("+")
    }

    /// For example `Ctrl+Shift+`.
    pub fn held_prefix(held: Held) -> String {
        let probe = Chord { command: held.command, shift: held.shift, alt: held.alt, ..Chord::key(KeyCode::KeyA) };
        let label = probe.label();
        label.strip_suffix(&input_label(probe.input)).unwrap_or_default().to_owned()
    }

    /// For `shortcuts.toml`, untranslated, unlike [`Chord::label`].
    pub fn to_key(self) -> String {
        let mut text = String::new();
        for (held, name) in [(self.command, "command"), (self.alt, "alt"), (self.shift, "shift")] {
            if held {
                text.push_str(name);
                text.push('+');
            }
        }
        text.push_str(input_name(self.input));
        text
    }

    pub fn from_key(text: &str) -> Option<Self> {
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let input = input_from_name(parts.pop()?)?;
        let mut chord = Chord { input, command: false, shift: false, alt: false };
        for part in parts {
            match part {
                "command" => chord.command = true,
                "alt" => chord.alt = true,
                "shift" => chord.shift = true,
                _ => return None,
            }
        }
        Some(chord)
    }
}

/// Every bindable key, by its name in `shortcuts.toml`. A key off this list
/// (media keys, IME keys) is refused when captured.
const KEYS: &[(KeyCode, &str)] = &[
    (KeyCode::KeyA, "KeyA"), (KeyCode::KeyB, "KeyB"), (KeyCode::KeyC, "KeyC"), (KeyCode::KeyD, "KeyD"),
    (KeyCode::KeyE, "KeyE"), (KeyCode::KeyF, "KeyF"), (KeyCode::KeyG, "KeyG"), (KeyCode::KeyH, "KeyH"),
    (KeyCode::KeyI, "KeyI"), (KeyCode::KeyJ, "KeyJ"), (KeyCode::KeyK, "KeyK"), (KeyCode::KeyL, "KeyL"),
    (KeyCode::KeyM, "KeyM"), (KeyCode::KeyN, "KeyN"), (KeyCode::KeyO, "KeyO"), (KeyCode::KeyP, "KeyP"),
    (KeyCode::KeyQ, "KeyQ"), (KeyCode::KeyR, "KeyR"), (KeyCode::KeyS, "KeyS"), (KeyCode::KeyT, "KeyT"),
    (KeyCode::KeyU, "KeyU"), (KeyCode::KeyV, "KeyV"), (KeyCode::KeyW, "KeyW"), (KeyCode::KeyX, "KeyX"),
    (KeyCode::KeyY, "KeyY"), (KeyCode::KeyZ, "KeyZ"),
    (KeyCode::Digit0, "Digit0"), (KeyCode::Digit1, "Digit1"), (KeyCode::Digit2, "Digit2"), (KeyCode::Digit3, "Digit3"),
    (KeyCode::Digit4, "Digit4"), (KeyCode::Digit5, "Digit5"), (KeyCode::Digit6, "Digit6"), (KeyCode::Digit7, "Digit7"),
    (KeyCode::Digit8, "Digit8"), (KeyCode::Digit9, "Digit9"),
    (KeyCode::F1, "F1"), (KeyCode::F2, "F2"), (KeyCode::F3, "F3"), (KeyCode::F4, "F4"), (KeyCode::F5, "F5"),
    (KeyCode::F6, "F6"), (KeyCode::F7, "F7"), (KeyCode::F8, "F8"), (KeyCode::F9, "F9"), (KeyCode::F10, "F10"),
    (KeyCode::F11, "F11"), (KeyCode::F12, "F12"),
    (KeyCode::Numpad0, "Numpad0"), (KeyCode::Numpad1, "Numpad1"), (KeyCode::Numpad2, "Numpad2"),
    (KeyCode::Numpad3, "Numpad3"), (KeyCode::Numpad4, "Numpad4"), (KeyCode::Numpad5, "Numpad5"),
    (KeyCode::Numpad6, "Numpad6"), (KeyCode::Numpad7, "Numpad7"), (KeyCode::Numpad8, "Numpad8"),
    (KeyCode::Numpad9, "Numpad9"), (KeyCode::NumpadAdd, "NumpadAdd"), (KeyCode::NumpadSubtract, "NumpadSubtract"),
    (KeyCode::NumpadMultiply, "NumpadMultiply"), (KeyCode::NumpadDivide, "NumpadDivide"),
    (KeyCode::NumpadDecimal, "NumpadDecimal"), (KeyCode::NumpadEnter, "NumpadEnter"),
    (KeyCode::Space, "Space"), (KeyCode::Enter, "Enter"), (KeyCode::Escape, "Escape"), (KeyCode::Tab, "Tab"),
    (KeyCode::Backspace, "Backspace"), (KeyCode::Delete, "Delete"), (KeyCode::Insert, "Insert"),
    (KeyCode::Home, "Home"), (KeyCode::End, "End"), (KeyCode::PageUp, "PageUp"), (KeyCode::PageDown, "PageDown"),
    (KeyCode::ArrowUp, "ArrowUp"), (KeyCode::ArrowDown, "ArrowDown"), (KeyCode::ArrowLeft, "ArrowLeft"),
    (KeyCode::ArrowRight, "ArrowRight"), (KeyCode::CapsLock, "CapsLock"),
    (KeyCode::ShiftLeft, "ShiftLeft"), (KeyCode::ShiftRight, "ShiftRight"),
    (KeyCode::ControlLeft, "ControlLeft"), (KeyCode::ControlRight, "ControlRight"),
    (KeyCode::AltLeft, "AltLeft"), (KeyCode::AltRight, "AltRight"),
    (KeyCode::SuperLeft, "SuperLeft"), (KeyCode::SuperRight, "SuperRight"),
    (KeyCode::Backquote, "Backquote"), (KeyCode::Minus, "Minus"), (KeyCode::Equal, "Equal"),
    (KeyCode::BracketLeft, "BracketLeft"), (KeyCode::BracketRight, "BracketRight"), (KeyCode::Backslash, "Backslash"),
    (KeyCode::Semicolon, "Semicolon"), (KeyCode::Quote, "Quote"), (KeyCode::Comma, "Comma"),
    (KeyCode::Period, "Period"), (KeyCode::Slash, "Slash"),
];

const BUTTONS: &[(MouseButton, &str)] = &[
    (MouseButton::Left, "MouseLeft"),
    (MouseButton::Right, "MouseRight"),
    (MouseButton::Middle, "MouseMiddle"),
    (MouseButton::Back, "MouseBack"),
    (MouseButton::Forward, "MouseForward"),
];

pub fn is_bindable(input: Input) -> bool {
    match input {
        Input::Key(key) => KEYS.iter().any(|(k, _)| *k == key),
        Input::Mouse(button) => BUTTONS.iter().any(|(b, _)| *b == button),
    }
}

fn input_name(input: Input) -> &'static str {
    match input {
        Input::Key(key) => KEYS.iter().find(|(k, _)| *k == key).map_or("Unidentified", |(_, name)| name),
        Input::Mouse(button) => BUTTONS.iter().find(|(b, _)| *b == button).map_or("Unidentified", |(_, name)| name),
    }
}

fn input_from_name(name: &str) -> Option<Input> {
    KEYS.iter()
        .find(|(_, n)| *n == name)
        .map(|(key, _)| Input::Key(*key))
        .or_else(|| BUTTONS.iter().find(|(_, n)| *n == name).map(|(button, _)| Input::Mouse(*button)))
}

pub fn input_label(input: Input) -> String {
    let name = input_name(input);
    if let Some(letter) = name.strip_prefix("Key") {
        return letter.to_owned();
    }
    if let Some(digit) = name.strip_prefix("Digit") {
        return digit.to_owned();
    }
    if let Some(digit) = name.strip_prefix("Numpad").filter(|rest| rest.len() == 1) {
        return get!("keybind.key.numpad", "key", digit);
    }
    if name.len() <= 3 && name.starts_with('F') {
        return name.to_owned();
    }
    let symbol = match input {
        Input::Key(KeyCode::Backquote) => Some("`"),
        Input::Key(KeyCode::Minus) => Some("-"),
        Input::Key(KeyCode::Equal) => Some("="),
        Input::Key(KeyCode::BracketLeft) => Some("["),
        Input::Key(KeyCode::BracketRight) => Some("]"),
        Input::Key(KeyCode::Backslash) => Some("\\"),
        Input::Key(KeyCode::Semicolon) => Some(";"),
        Input::Key(KeyCode::Quote) => Some("'"),
        Input::Key(KeyCode::Comma) => Some(","),
        Input::Key(KeyCode::Period) => Some("."),
        Input::Key(KeyCode::Slash) => Some("/"),
        _ => None,
    };
    if let Some(symbol) = symbol {
        return symbol.to_owned();
    }
    crate::common::lang::get_parsed(&format!("keybind.key.{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_reads_back_what_it_wrote() {
        for chord in [
            Chord::command(KeyCode::KeyN),
            Chord::command(KeyCode::KeyN).with_shift().with_alt(),
            Chord::key(KeyCode::Backspace),
            Chord::key(KeyCode::ShiftLeft),
            Chord::mouse(MouseButton::Right),
            Chord::command(KeyCode::Comma),
        ] {
            assert_eq!(Chord::from_key(&chord.to_key()), Some(chord), "{} did not read back", chord.to_key());
        }
    }

    #[test]
    fn nonsense_is_not_a_chord() {
        for text in ["", "command+", "hyper+KeyN", "command+Nope", "KeyN+command", "N"] {
            assert_eq!(Chord::from_key(text), None, "{text:?} parsed");
        }
    }

    #[test]
    fn every_listed_key_has_a_unique_name() {
        let mut names: Vec<&str> = KEYS.iter().map(|(_, n)| *n).chain(BUTTONS.iter().map(|(_, n)| *n)).collect();
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count);
    }

    /// Fails if Shift+B fires a command bound to B, or B fails while sprinting.
    #[test]
    fn exact_and_loose_matching() {
        let b = Chord::key(KeyCode::KeyB);
        let shift = Held { shift: true, ..default() };
        assert!(b.modifiers_match(Held::default(), true));
        assert!(!b.modifiers_match(shift, true));
        assert!(b.modifiers_match(shift, false));
        assert!(!b.with_shift().modifiers_match(Held::default(), false));
    }

    /// Fails if a bare Shift binding counts its own key as an extra modifier.
    #[test]
    fn a_modifier_key_is_not_its_own_modifier() {
        let sprint = Chord::key(KeyCode::ShiftLeft);
        assert!(sprint.modifiers_match(Held { shift: true, ..default() }, true));
    }

    /// Fails if a label shows a key code or a missing lang key. de-DE falls
    /// back to en-US for these, so another test's language does not matter.
    #[test]
    fn labels_read_as_the_keys_do() {
        assert_eq!(Chord::command(KeyCode::KeyS).with_shift().label(), "Ctrl+Shift+S");
        assert_eq!(Chord::command(KeyCode::KeyN).with_alt().label(), "Ctrl+Alt+N");
        assert_eq!(Chord::command(KeyCode::Comma).label(), "Ctrl+,");
        assert_eq!(Chord::key(KeyCode::Digit1).label(), "1");
        assert_eq!(Chord::key(KeyCode::F5).label(), "F5");
        assert_eq!(Chord::key(KeyCode::Numpad0).label(), "Num 0");
        assert_eq!(Chord::key(KeyCode::Space).label(), "Space");
        assert_eq!(Chord::key(KeyCode::ShiftLeft).label(), "Left Shift");
        assert_eq!(Chord::mouse(MouseButton::Left).label(), "Left Mouse");
        assert_eq!(Chord::held_prefix(Held { command: true, shift: true, alt: false }), "Ctrl+Shift+");
        for input in KEYS.iter().map(|(key, _)| Input::Key(*key)).chain(BUTTONS.iter().map(|(b, _)| Input::Mouse(*b))) {
            let label = input_label(input);
            assert!(!label.starts_with('<'), "{input:?} shows {label}");
        }
    }

    #[test]
    fn held_reads_each_modifier_on_either_side() {
        for (key, want) in [
            (KeyCode::SuperRight, Held { command: true, ..default() }),
            (KeyCode::ControlLeft, Held { command: true, ..default() }),
            (KeyCode::ShiftRight, Held { shift: true, ..default() }),
            (KeyCode::AltLeft, Held { alt: true, ..default() }),
            (KeyCode::KeyA, Held::default()),
        ] {
            let mut keys = ButtonInput::<KeyCode>::default();
            keys.press(key);
            assert_eq!(Held::of(&keys), want, "{key:?}");
        }
    }

    #[test]
    fn escape_and_enter_are_reserved_with_any_modifier() {
        assert!(Chord::key(KeyCode::Escape).is_reserved());
        assert!(Chord::command(KeyCode::Enter).is_reserved());
        assert!(Chord::key(KeyCode::NumpadEnter).is_reserved());
        assert!(!Chord::key(KeyCode::Space).is_reserved());
    }
}
