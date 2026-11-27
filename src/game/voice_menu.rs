//! The voice menu: `Z` ([`Action::Voice`]) puts it up above the chat, listing each
//! [`VoiceCommand`] under the weapon bucket's key that sends it ([`Action::BUCKETS`], `1` to
//! `9`), as the taunt menu does: it holds no mouse, so it reads Play's keys, never the Menu
//! context's choices. `0` ([`Action::PlayDismiss`]), `Q` ([`Action::PlayCancel`]) and `Z` again
//! put it down. The chat key puts
//! it down and the chat opens from the same press; the taunt key puts it down and the taunt menu
//! opens. Up, it owns the digits, so a press does not also choose a weapon.
//!
//! It holds no mouse, so the body moves and shoots while it is up. A choice is a [`SayVoice`],
//! which the authority words into the chat ([`crate::game::chat`]).

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::AppMode;
use crate::common::keybind::{Action, Actions, Keymap};
use crate::game::chat::{Audience, SayVoice, VoiceCommand};
use crate::game::chat_window::chat_zone;
use crate::game::pause_menu::MouseHeld;
use crate::get;

/// Between the menu and the chat under it, in logical pixels.
const GAP: f32 = 8.0;
const PADDING: f32 = 6.0;
const TEXT_SIZE: f32 = 15.0;

pub struct VoiceMenuPlugin;

impl Plugin for VoiceMenuPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        app.init_resource::<VoiceMenu>()
            .init_resource::<MouseHeld>()
            .add_message::<SayVoice>()
            .add_systems(OnExit(AppMode::Play), close_the_menu);
    }
}

/// Draws it; added in `launch.rs`, since it draws with egui.
pub struct VoiceMenuWindowPlugin;

impl Plugin for VoiceMenuWindowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VoiceMenu>()
            .add_systems(EguiPrimaryContextPass, show_the_voice_menu.run_if(in_state(AppMode::Play)));
    }
}

#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceMenu {
    open: bool,
    /// The menu took a key this frame, so nothing after it may read the same press.
    answered: bool,
}

impl VoiceMenu {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the keys pressed this frame are the menu's: while it is up, and on the frame one
    /// of them took it down.
    pub fn holds_the_keys(&self) -> bool {
        self.open || self.answered
    }
}

/// One frame's keys, as [`answer`] reads them.
#[derive(Default, Clone, Copy)]
struct Keys {
    voice: bool,
    chat: bool,
    taunt: bool,
    cancel: bool,
    dismiss: bool,
    /// Index into [`Action::BUCKETS`].
    choice: Option<usize>,
}

/// What a frame's keys do to the menu. Pure, so the rules are testable without a world.
fn answer(open: bool, keys: Keys) -> (VoiceMenu, Option<VoiceCommand>) {
    let shut = |answered| VoiceMenu { open: false, answered };
    if !open {
        return match keys.voice {
            true => (VoiceMenu { open: true, answered: true }, None),
            false => (shut(false), None),
        };
    }
    // Left for the chat and the taunt menu to answer.
    if keys.chat || keys.taunt {
        return (shut(false), None);
    }
    if keys.voice || keys.cancel || keys.dismiss {
        return (shut(true), None);
    }
    match keys.choice {
        Some(choice) => match VoiceCommand::ALL.get(choice) {
            Some(command) => (shut(true), Some(*command)),
            None => (VoiceMenu { open: true, answered: true }, None),
        },
        None => (VoiceMenu { open: true, answered: false }, None),
    }
}

/// Before the taunt menu and `choose_weapons`, so the menu has the keys first. Down while any
/// menu holds the mouse, so the chat opening puts it down.
pub fn answer_the_voice_keys(
    actions: Res<Actions>,
    held: Option<Res<MouseHeld>>,
    mut menu: ResMut<VoiceMenu>,
    mut said: MessageWriter<SayVoice>,
) {
    if held.is_some_and(|held| held.any()) {
        menu.set_if_neq(VoiceMenu::default());
        return;
    }
    let keys = Keys {
        voice: actions.just_pressed(Action::Voice),
        chat: actions.just_pressed(Action::Chat),
        taunt: actions.just_pressed(Action::Taunt),
        cancel: actions.just_pressed(Action::PlayCancel),
        dismiss: actions.just_pressed(Action::PlayDismiss),
        choice: Action::BUCKETS.iter().position(|action| actions.just_pressed(*action)),
    };
    let (next, command) = answer(menu.open, keys);
    if let Some(command) = command {
        said.write(SayVoice { audience: Audience::All, command });
    }
    menu.set_if_neq(next);
}

fn close_the_menu(mut menu: ResMut<VoiceMenu>) {
    menu.set_if_neq(VoiceMenu::default());
}

/// The first key bound to `action`, or nothing.
fn bind(keymap: Option<&Keymap>, action: Action) -> Option<String> {
    keymap.and_then(|keymap| keymap.chords(action).first()).map(|chord| chord.label())
}

/// As wide as the chat, its bottom just above the chat's top, as tall as its lines.
fn show_the_voice_menu(mut contexts: EguiContexts, menu: Res<VoiceMenu>, keymap: Option<Res<Keymap>>) {
    if !menu.is_open() {
        return;
    }
    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(e) => {
            warn!("voice_menu::show_the_voice_menu: no egui context: {e}");
            return;
        }
    };
    let screen = ctx.content_rect();
    let zone = chat_zone(screen.width(), screen.height());
    let bottom_left = egui::pos2(screen.min.x + zone.min.x, screen.min.y + zone.min.y - GAP);
    let keymap = keymap.as_deref();
    let font = egui::FontId::proportional(TEXT_SIZE);
    egui::Area::new(egui::Id::new("voice_menu"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::LEFT_BOTTOM)
        .fixed_pos(bottom_left)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(170))
                .corner_radius(4.0)
                .inner_margin(PADDING)
                .show(ui, |ui| {
                    ui.set_width(zone.width() - 2.0 * PADDING);
                    ui.label(egui::RichText::new(get!("voice.title")).font(font.clone()).strong().color(egui::Color32::WHITE));
                    for (command, action) in VoiceCommand::ALL.into_iter().zip(Action::BUCKETS) {
                        let text = match bind(keymap, action) {
                            Some(key) => get!("voice.line", "key", key, "line", command.summary()),
                            None => command.summary(),
                        };
                        ui.label(egui::RichText::new(text).font(font.clone()).color(egui::Color32::WHITE));
                    }
                    // An exception to the no-instructions rule: the menu holds the digits, so it
                    // says how to close it without choosing.
                    if let Some(cancel) = bind(keymap, Action::PlayCancel) {
                        ui.label(egui::RichText::new(get!("voice.cancel", "bind", cancel)).font(font.clone()).color(egui::Color32::GRAY));
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_voice_key_puts_the_menu_up_and_down() {
        let (menu, said) = answer(false, Keys { voice: true, ..default() });
        assert!(menu.is_open() && menu.holds_the_keys() && said.is_none());
        let (menu, said) = answer(true, Keys::default());
        assert!(menu.is_open() && said.is_none(), "an idle frame put it down");
        let (menu, said) = answer(true, Keys { voice: true, ..default() });
        assert!(!menu.is_open() && menu.holds_the_keys() && said.is_none());
    }

    #[test]
    fn a_digit_sends_the_line_under_it() {
        for (index, command) in VoiceCommand::ALL.into_iter().enumerate() {
            let (menu, said) = answer(true, Keys { choice: Some(index), ..default() });
            assert_eq!(said, Some(command));
            assert!(!menu.is_open() && menu.holds_the_keys(), "the digit would also choose a weapon");
        }
    }

    #[test]
    fn zero_and_cancel_put_it_down_sending_nothing() {
        for keys in [Keys { dismiss: true, ..default() }, Keys { cancel: true, ..default() }] {
            let (menu, said) = answer(true, keys);
            assert!(!menu.is_open() && menu.holds_the_keys() && said.is_none());
        }
    }

    /// Fails if the chat or taunt key is swallowed, so the chat or taunt menu would not open.
    #[test]
    fn the_chat_and_taunt_keys_put_it_down_and_pass_through() {
        for keys in [Keys { chat: true, ..default() }, Keys { taunt: true, ..default() }] {
            let (menu, said) = answer(true, keys);
            assert!(!menu.holds_the_keys() && said.is_none());
        }
    }

    #[test]
    fn digits_are_the_weapons_while_the_menu_is_down() {
        let (menu, said) = answer(false, Keys { choice: Some(0), cancel: true, ..default() });
        assert!(!menu.holds_the_keys() && said.is_none());
    }
}
