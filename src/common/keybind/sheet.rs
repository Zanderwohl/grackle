//! The Shortcuts page of the Settings window.
//!
//! A click on a chord waits, and the next press replaces it. Keys come from
//! Bevy's input, not egui's. A mouse button is bound only when pressed on the
//! waiting prompt; pressed elsewhere it stops the wait, so a click on Close
//! closes. Escape stops the wait; Enter is refused. A modifier pressed and
//! released with no other modifier held is bound alone (Shift to sprint).
//! The page edits the window's draft.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy_egui::egui::{self, RichText, Ui};

use super::chord::{is_bindable, modifier_of};
use super::{context_name, Action, Chord, Context, Held, Keymap};
use crate::get;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    Reserved,
    Unbindable,
}

#[derive(Default, Debug)]
pub struct Sheet {
    /// A slot past the end adds a chord.
    capturing: Option<(Action, usize)>,
    /// Bound alone if released with nothing else pressed.
    lone_modifier: Option<KeyCode>,
    refused: Option<Refusal>,
}

impl Sheet {
    pub fn is_capturing(&self) -> bool {
        self.capturing.is_some()
    }

    pub fn stop(&mut self) {
        self.capturing = None;
        self.lone_modifier = None;
    }

    pub(crate) fn begin(&mut self, action: Action, slot: usize) {
        self.capturing = Some((action, slot));
        self.lone_modifier = None;
        self.refused = None;
    }

    pub fn capture(
        &mut self,
        keymap: &mut Keymap,
        held: Held,
        presses: impl IntoIterator<Item = (KeyCode, ButtonState)>,
        clicked: impl IntoIterator<Item = MouseButton>,
    ) {
        let Some((action, slot)) = self.capturing else { return };
        let mut caught = None;

        for (key, state) in presses {
            match (state, modifier_of(key)) {
                // With another modifier already down it is part of a chord.
                (ButtonState::Pressed, Some(_)) => {
                    self.lone_modifier = Chord::key(key).modifiers_match(held, true).then_some(key);
                }
                (ButtonState::Released, Some(_)) if self.lone_modifier == Some(key) => {
                    caught = Some(Chord::key(key));
                    break;
                }
                (ButtonState::Pressed, None) => {
                    caught = Some(Chord { command: held.command, shift: held.shift, alt: held.alt, ..Chord::key(key) });
                    break;
                }
                _ => {}
            }
        }
        if caught.is_none()
            && let Some(button) = clicked.into_iter().next()
        {
            caught = Some(Chord { command: held.command, shift: held.shift, alt: held.alt, ..Chord::mouse(button) });
        }

        let Some(chord) = caught else { return };
        self.stop();
        if chord == Chord::key(KeyCode::Escape) {
            return;
        }
        if chord.is_reserved() {
            self.refused = Some(Refusal::Reserved);
            return;
        }
        if !is_bindable(chord.input) {
            self.refused = Some(Refusal::Unbindable);
            return;
        }
        keymap.bind(action, slot, chord);
    }

    /// The mouse half is in [`Sheet::waiting`], which knows where the prompt is.
    pub fn capture_from(
        &mut self,
        keymap: &mut Keymap,
        keys: &ButtonInput<KeyCode>,
        messages: &mut MessageReader<KeyboardInput>,
    ) {
        let presses: Vec<_> = messages.read().filter(|m| !m.repeat).map(|m| (m.key_code, m.state)).collect();
        if !self.is_capturing() {
            return;
        }
        self.capture(keymap, Held::of(keys), presses, []);
    }

    pub fn show(&mut self, ui: &mut Ui, keymap: &mut Keymap, held: Held) {
        ui.horizontal(|ui| {
            ui.heading(get!("keybind.sheet.heading"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let changed = keymap != Keymap::shipped();
                if ui.add_enabled(changed, egui::Button::new(get!("keybind.sheet.reset"))).clicked() {
                    *keymap = Keymap::shipped().clone();
                    self.stop();
                }
            });
        });
        ui.add_space(5.0);

        match self.refused {
            Some(Refusal::Reserved) => {
                ui.colored_label(ui.visuals().error_fg_color, get!("keybind.sheet.reserved"));
            }
            Some(Refusal::Unbindable) => {
                ui.colored_label(ui.visuals().error_fg_color, get!("keybind.sheet.unbindable"));
            }
            None => {}
        }

        for context in Context::ALL {
            let actions: Vec<Action> =
                Action::ALL.iter().copied().filter(|a| a.context() == context && !a.is_debug()).collect();
            self.section(ui, keymap, held, &context_name(context), ("settings_shortcuts", context), &actions);
        }
        let debug: Vec<Action> = Action::ALL.iter().copied().filter(|a| a.is_debug()).collect();
        self.section(ui, keymap, held, &get!("keybind.sheet.debug"), ("settings_shortcuts", "debug"), &debug);
    }

    fn section(
        &mut self,
        ui: &mut Ui,
        keymap: &mut Keymap,
        held: Held,
        heading: &str,
        id: impl std::hash::Hash + std::fmt::Debug,
        actions: &[Action],
    ) {
        if actions.is_empty() {
            return;
        }
        ui.add_space(10.0);
        ui.label(RichText::new(heading).strong());
        ui.add_space(4.0);
        egui::Grid::new(id).num_columns(2).spacing([10.0, 6.0]).striped(true).show(ui, |ui| {
            for &action in actions {
                ui.label(action.name());
                if action.is_fixed() {
                    let labels: Vec<String> = keymap.chords(action).iter().map(|c| c.label()).collect();
                    ui.label(RichText::new(labels.join(", ")).monospace().weak());
                } else {
                    ui.horizontal_wrapped(|ui| self.chords(ui, keymap, action, held));
                }
                ui.end_row();
            }
        });
    }

    fn chords(&mut self, ui: &mut Ui, keymap: &mut Keymap, action: Action, held: Held) {
        let count = keymap.chords(action).len();
        let mut remove = None;

        for slot in 0..count {
            let chord = keymap.chords(action)[slot];
            if self.capturing == Some((action, slot)) {
                self.waiting(ui, keymap, held);
                continue;
            }
            let clashes = keymap.clashes(action, chord);
            let mut text = RichText::new(chord.label()).monospace();
            if !clashes.is_empty() {
                text = text.color(ui.visuals().error_fg_color);
            }
            let (response, remove_response) = joined(ui, text);
            let mut response = response.on_hover_text(get!("keybind.sheet.change_hover"));
            if !clashes.is_empty() {
                let names: Vec<String> = clashes.iter().map(|a| a.name()).collect();
                response = response.on_hover_text(get!("keybind.sheet.clash", "actions", names.join(", ")));
            }
            if response.clicked() {
                // So the key being captured cannot also click it.
                response.surrender_focus();
                self.begin(action, slot);
            }
            if remove_response.on_hover_text(get!("keybind.sheet.remove_hover")).clicked() {
                remove = Some(slot);
            }
        }

        if self.capturing == Some((action, count)) {
            self.waiting(ui, keymap, held);
        } else {
            let add = ui.small_button("+").on_hover_text(get!("keybind.sheet.add_hover"));
            if add.clicked() {
                add.surrender_focus();
                self.begin(action, count);
            }
        }

        if let Some(slot) = remove {
            keymap.unbind(action, slot);
            self.stop();
        }
    }

    fn waiting(&mut self, ui: &mut Ui, keymap: &mut Keymap, held: Held) {
        let prefix = Chord::held_prefix(held);
        let prompt = match prefix.is_empty() {
            true => get!("keybind.sheet.press"),
            false => get!("keybind.sheet.press_held", "held", prefix),
        };
        let text = RichText::new(prompt).monospace().color(ui.visuals().selection.stroke.color);
        let response = ui.add(egui::Button::new(text));
        let pressed = ui.input(|input| {
            [
                (egui::PointerButton::Primary, MouseButton::Left),
                (egui::PointerButton::Secondary, MouseButton::Right),
                (egui::PointerButton::Middle, MouseButton::Middle),
                (egui::PointerButton::Extra1, MouseButton::Back),
                (egui::PointerButton::Extra2, MouseButton::Forward),
            ]
            .into_iter()
            .find(|(button, _)| input.pointer.button_pressed(*button))
            .map(|(_, mouse)| mouse)
        });
        match pressed {
            Some(button) if response.contains_pointer() => self.capture(keymap, held, [], [button]),
            Some(_) => self.stop(),
            None => {}
        }
    }
}

/// Joined along one seam so they read as one control.
fn joined(ui: &mut Ui, chord: RichText) -> (egui::Response, egui::Response) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let round = ui.visuals().widgets.inactive.corner_radius;
        let left = egui::CornerRadius { nw: round.nw, sw: round.sw, ne: 0, se: 0 };
        let right = egui::CornerRadius { nw: 0, sw: 0, ne: round.ne, se: round.se };
        let chord = ui.add(egui::Button::new(chord).corner_radius(left));
        let remove =
            ui.add(egui::Button::new("×").corner_radius(right).min_size(egui::vec2(0.0, chord.rect.height())));
        (chord, remove)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: KeyCode) -> (KeyCode, ButtonState) {
        (key, ButtonState::Pressed)
    }

    fn release(key: KeyCode) -> (KeyCode, ButtonState) {
        (key, ButtonState::Released)
    }

    fn waiting(action: Action, slot: usize) -> (Sheet, Keymap) {
        let mut sheet = Sheet::default();
        sheet.begin(action, slot);
        (sheet, Keymap::default())
    }

    #[test]
    fn the_next_key_replaces_the_chord_with_its_modifiers() {
        let (mut sheet, mut keymap) = waiting(Action::Save, 0);
        let held = Held { command: true, ..default() };
        sheet.capture(&mut keymap, held, [press(KeyCode::ControlLeft), press(KeyCode::KeyG)], []);
        assert!(!sheet.is_capturing());
        assert_eq!(keymap.chords(Action::Save), &[Chord::command(KeyCode::KeyG)]);
    }

    /// Fails if Shift cannot be bound alone, or Ctrl+S binds Ctrl.
    #[test]
    fn a_modifier_let_go_alone_is_bound_alone() {
        let (mut sheet, mut keymap) = waiting(Action::Sprint, 0);
        sheet.capture(&mut keymap, Held { shift: true, ..default() }, [press(KeyCode::ShiftLeft)], []);
        assert!(sheet.is_capturing(), "a held modifier ended the wait");
        sheet.capture(&mut keymap, Held::default(), [release(KeyCode::ShiftLeft)], []);
        assert_eq!(keymap.chords(Action::Sprint)[0], Chord::key(KeyCode::ShiftLeft));
    }

    /// Fails if Shift pressed and let go while Ctrl is held binds bare Shift.
    #[test]
    fn a_modifier_with_another_held_is_not_bound_alone() {
        let (mut sheet, mut keymap) = waiting(Action::Sprint, 0);
        let both = Held { command: true, shift: true, ..default() };
        sheet.capture(&mut keymap, both, [press(KeyCode::ShiftLeft)], []);
        sheet.capture(&mut keymap, Held { command: true, ..default() }, [release(KeyCode::ShiftLeft)], []);
        assert!(sheet.is_capturing(), "a modifier chord was bound as one key");
        sheet.capture(&mut keymap, both, [press(KeyCode::KeyS)], []);
        assert_eq!(keymap.chords(Action::Sprint)[0], Chord::command(KeyCode::KeyS).with_shift());
    }

    #[test]
    fn a_mouse_button_can_be_bound() {
        let (mut sheet, mut keymap) = waiting(Action::Jump, 1);
        sheet.capture(&mut keymap, Held::default(), [], [MouseButton::Middle]);
        assert_eq!(keymap.chords(Action::Jump), &[Chord::key(KeyCode::Space), Chord::mouse(MouseButton::Middle)]);
    }

    /// Fails if Escape binds itself instead of backing out.
    #[test]
    fn escape_stops_waiting_and_changes_nothing() {
        let (mut sheet, mut keymap) = waiting(Action::Jump, 0);
        sheet.capture(&mut keymap, Held::default(), [press(KeyCode::Escape)], []);
        assert!(!sheet.is_capturing());
        assert_eq!(sheet.refused, None);
        assert_eq!(keymap, Keymap::default());
    }

    #[test]
    fn enter_is_refused_and_says_so() {
        let (mut sheet, mut keymap) = waiting(Action::Jump, 0);
        sheet.capture(&mut keymap, Held { shift: true, ..default() }, [press(KeyCode::Enter)], []);
        assert_eq!(sheet.refused, Some(Refusal::Reserved));
        assert_eq!(keymap, Keymap::default());
    }

    #[test]
    fn a_key_off_the_list_is_refused() {
        let (mut sheet, mut keymap) = waiting(Action::Jump, 0);
        sheet.capture(&mut keymap, Held::default(), [press(KeyCode::MediaPlayPause)], []);
        assert_eq!(sheet.refused, Some(Refusal::Unbindable));
        assert_eq!(keymap, Keymap::default());
    }

    #[test]
    fn a_frame_with_nothing_pressed_keeps_waiting() {
        let (mut sheet, mut keymap) = waiting(Action::Jump, 0);
        sheet.capture(&mut keymap, Held::default(), [release(KeyCode::KeyX)], []);
        assert!(sheet.is_capturing());
    }
}
