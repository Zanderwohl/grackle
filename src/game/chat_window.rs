//! The chat on screen, in egui: closed, the latest lines float over the bottom left of the window
//! and fade; open, the round's whole log scrolls above a text field, and the mouse is free.
//!
//! It sits left of the HUD's safe zone where that strip is wide enough ([`chat_zone`]), else above
//! the HUD's readouts. The chat key (`Y`) opens it, and closes it again when the field does
//! not have the keyboard, and `Escape`, `Enter` (which sends), a click outside it or the window
//! losing focus close it from anywhere.
//!
//! Added in `launch.rs`, since it draws with egui.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowFocused};
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::AppMode;
use crate::common::keybind::{Action, Actions};
use crate::common::swatch::Swatch;
use crate::game::chat::{cleaned, Audience, ChatEntry, ChatLog, SayChat, CHAT_LIMIT};
use crate::game::hud::{SAFE_ZONE_ASPECT, WINDOW_MARGIN};
use crate::game::pause_menu::{EscapeTaken, MouseHeld, PauseMenu, TakeEscape};
use crate::get;

/// The strip left of the safe zone is used only if at least this wide, in logical pixels.
const LEFT_STRIP_MIN_WIDTH: f32 = 320.0;
/// Between the chat and the safe zone, in logical pixels.
const GAP: f32 = 8.0;
/// Above the HUD's readouts (the portrait is the tallest), when the chat is placed over them.
const HUD_CLEARANCE: f32 = 150.0;
const OVER_HUD_MAX_WIDTH: f32 = 420.0;
const HEIGHT_SHARE: f32 = 0.28;
const MIN_HEIGHT: f32 = 120.0;
const MAX_HEIGHT: f32 = 320.0;

const PADDING: f32 = 6.0;
const TEXT_SIZE: f32 = 15.0;

/// Seconds a line stays in the closed log, the last [`FADE_SECONDS`] of them fading.
const SHOWN_SECONDS: f32 = 12.0;
const FADE_SECONDS: f32 = 2.0;
const CLOSED_LINES: usize = 8;

#[derive(Resource, Default, Debug)]
pub struct ChatWindow {
    pub open: bool,
    draft: String,
    /// Asked for after the field is first drawn, so the key that opened the window is not typed.
    focus_next: bool,
    /// The click that closed the window holds the mouse until it is let go, so it does not fire.
    until_released: bool,
}

impl ChatWindow {
    fn open(&mut self) {
        self.open = true;
        self.focus_next = true;
    }

    fn close(&mut self) {
        self.open = false;
        self.focus_next = false;
    }
}

/// `width` and `height` in logical pixels.
pub fn chat_zone(width: f32, height: f32) -> Rect {
    let tall = (height * HEIGHT_SHARE).clamp(MIN_HEIGHT, MAX_HEIGHT);
    let safe_width = (height * SAFE_ZONE_ASPECT).min(width);
    let safe_left = (width - safe_width) / 2.0;
    let strip = safe_left - GAP - WINDOW_MARGIN;
    if strip >= LEFT_STRIP_MIN_WIDTH {
        let bottom = height - WINDOW_MARGIN;
        return Rect::new(WINDOW_MARGIN, bottom - tall, WINDOW_MARGIN + strip, bottom);
    }
    let wide = (width - 2.0 * WINDOW_MARGIN).min(OVER_HUD_MAX_WIDTH).max(0.0);
    let bottom = (height - WINDOW_MARGIN - HUD_CLEARANCE).max(WINDOW_MARGIN + tall);
    Rect::new(WINDOW_MARGIN, bottom - tall, WINDOW_MARGIN + wide, bottom)
}

pub struct ChatWindowPlugin;

impl Plugin for ChatWindowPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        app.init_resource::<ChatWindow>()
            .init_resource::<MouseHeld>()
            .init_resource::<EscapeTaken>()
            .init_resource::<PauseMenu>()
            .add_message::<SayChat>()
            .add_message::<WindowFocused>()
            .add_systems(
                Update,
                (toggle_the_chat.in_set(TakeEscape), close_on_blur, lend_the_mouse)
                    .chain()
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(EguiPrimaryContextPass, show_the_chat.run_if(in_state(AppMode::Play)))
            .add_systems(OnExit(AppMode::Play), close_on_leaving_play);
    }
}

/// Opened only while no other menu is up. While the field has the keyboard, no action fires, so
/// the chat key and `Escape` reach only egui, and [`show_the_chat`] answers them.
fn toggle_the_chat(
    actions: Res<Actions>,
    menu: Res<PauseMenu>,
    held: Res<MouseHeld>,
    mut chat: ResMut<ChatWindow>,
    mut taken: ResMut<EscapeTaken>,
) {
    match chat.open {
        true if actions.just_pressed(Action::Pause) => {
            chat.close();
            taken.0 = true;
        }
        true if actions.just_pressed(Action::Chat) => chat.close(),
        true => {}
        false if actions.just_pressed(Action::Chat) && !menu.open && !held.settings => chat.open(),
        false => {}
    }
}

/// `pause_on_blur` passes over a blur while the chat holds the mouse, so the pause menu goes up
/// here instead, or the cursor would be grabbed again on coming back.
fn close_on_blur(
    mut focus: MessageReader<WindowFocused>,
    window: Query<(), With<PrimaryWindow>>,
    mut chat: ResMut<ChatWindow>,
    mut menu: ResMut<PauseMenu>,
) {
    let blurred = focus.read().any(|event| !event.focused && window.contains(event.window));
    if blurred && chat.open {
        chat.close();
        menu.open = true;
    }
}

fn lend_the_mouse(actions: Res<Actions>, mut chat: ResMut<ChatWindow>, mut held: ResMut<MouseHeld>) {
    if chat.until_released && !actions.pressed(Action::Fire) && !actions.pressed(Action::AltFire) {
        chat.until_released = false;
    }
    let holding = chat.open || chat.until_released;
    if held.chat != holding {
        held.chat = holding;
    }
}

fn close_on_leaving_play(mut chat: ResMut<ChatWindow>) {
    *chat = ChatWindow::default();
}

/// Brighter than Yellow team's color, so an announcement is not read as a Yellow player's name.
const ROUND_EVENT_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 221, 64);

fn color32(color: Color) -> egui::Color32 {
    let [r, g, b, _] = color.to_srgba().to_u8_array();
    egui::Color32::from_rgb(r, g, b)
}

fn rect(rect: Rect) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.min.y), egui::pos2(rect.max.x, rect.max.y))
}

/// `alpha` from 0 (clear) to 1 (opaque).
fn line(entry: &ChatEntry, alpha: f32, width: f32) -> egui::text::LayoutJob {
    let font = egui::FontId::proportional(TEXT_SIZE);
    let format = |color: egui::Color32| egui::TextFormat { font_id: font.clone(), color: color.gamma_multiply(alpha), ..default() };
    let white = egui::Color32::WHITE;
    let mut job = egui::text::LayoutJob::default();
    match entry {
        ChatEntry::Said { from, audience, text } => {
            match audience {
                Audience::All => {}
            }
            let swatch: Swatch = from.swatch();
            job.append(&get!("chat.said", "name", from.display_name()), 0.0, format(color32(swatch.base)));
            job.append(text, 0.0, format(white));
        }
        ChatEntry::Voice { from, audience, command, rand } => {
            match audience {
                Audience::All => {}
            }
            job.append(&get!("chat.said", "name", from.display_name()), 0.0, format(color32(from.swatch().base)));
            job.append(&command.said(*rand), 0.0, format(white));
        }
        ChatEntry::TooFast { .. } => job.append(&get!("chat.too_fast"), 0.0, format(egui::Color32::LIGHT_GRAY)),
        ChatEntry::Round(event) => job.append(&event.words(), 0.0, format(ROUND_EVENT_COLOR)),
        // Never kept in the log.
        ChatEntry::Clear => {}
    }
    job.wrap.max_width = width;
    job
}

fn show_the_chat(
    mut contexts: EguiContexts,
    log: Res<ChatLog>,
    time: Res<Time<Real>>,
    mut chat: ResMut<ChatWindow>,
    mut said: MessageWriter<SayChat>,
) {
    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(e) => {
            warn!("chat_window::show_the_chat: no egui context: {e}");
            return;
        }
    };
    let screen = ctx.content_rect();
    let zone = rect(chat_zone(screen.width(), screen.height())).translate(screen.min.to_vec2());
    match chat.open {
        false => float_the_latest(ctx, &log, time.elapsed_secs(), zone),
        true => {
            if let Some(text) = show_the_window(ctx, &log, &mut chat, zone) {
                said.write(SayChat { audience: Audience::All, text });
            }
        }
    }
}

/// Newest at the bottom of `zone`. Takes no input.
fn float_the_latest(ctx: &egui::Context, log: &ChatLog, now: f32, zone: egui::Rect) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("chat_log")));
    let mut bottom = zone.max.y;
    for logged in log.lines.iter().rev().take(CLOSED_LINES) {
        let left = SHOWN_SECONDS - (now - logged.at);
        if left <= 0.0 {
            break;
        }
        let alpha = (left / FADE_SECONDS).clamp(0.0, 1.0);
        let galley = ctx.fonts_mut(|fonts| fonts.layout_job(line(&logged.entry, alpha, zone.width())));
        let top = bottom - galley.size().y;
        if top < zone.min.y {
            break;
        }
        let at = egui::pos2(zone.min.x, top);
        let shadow = egui::Color32::from_black_alpha((220.0 * alpha) as u8);
        painter.galley_with_override_text_color(at + egui::vec2(1.0, 1.0), galley.clone(), shadow);
        painter.galley(at, galley, egui::Color32::WHITE);
        bottom = top;
    }
}

/// Returns what was sent.
fn show_the_window(ctx: &egui::Context, log: &ChatLog, chat: &mut ChatWindow, zone: egui::Rect) -> Option<String> {
    let mut sent = None;
    let mut close = false;
    let inner = zone.shrink(PADDING);
    let area = egui::Area::new(egui::Id::new("chat_window")).order(egui::Order::Foreground).fixed_pos(zone.min).show(ctx, |ui| {
        egui::Frame::new()
            .fill(egui::Color32::from_black_alpha(170))
            .corner_radius(4.0)
            .inner_margin(PADDING)
            .show(ui, |ui| {
                ui.set_width(inner.width());
                ui.set_height(inner.height());
                let field_height = ui.spacing().interact_size.y;
                let log_height = (inner.height() - field_height - ui.spacing().item_spacing.y).max(0.0);
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    .max_height(log_height)
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        // A refusal floats and fades with the latest lines but is not part of
                        // the round's log.
                        for logged in log.lines.iter().filter(|logged| !matches!(logged.entry, ChatEntry::TooFast { .. })) {
                            ui.label(line(&logged.entry, 1.0, width));
                        }
                    });
                let field = ui.add(
                    egui::TextEdit::singleline(&mut chat.draft)
                        .desired_width(f32::INFINITY)
                        .char_limit(CHAT_LIMIT)
                        .font(egui::FontId::proportional(TEXT_SIZE)),
                );
                if std::mem::take(&mut chat.focus_next) {
                    field.request_focus();
                }
                let (enter, escape) = ui.input(|input| (input.key_pressed(egui::Key::Enter), input.key_pressed(egui::Key::Escape)));
                if escape {
                    chat.draft.clear();
                    close = true;
                } else if enter && field.lost_focus() {
                    sent = cleaned(&chat.draft);
                    chat.draft.clear();
                    close = true;
                }
            });
    });
    let clicked_outside = ctx.input(|input| {
        input.pointer.any_pressed() && input.pointer.interact_pos().is_some_and(|at| !area.response.rect.contains(at))
    });
    if clicked_outside {
        chat.until_released = true;
        close = true;
    }
    if close {
        chat.close();
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails if a narrow window puts the chat in a strip it does not fit, or a wide one puts it
    /// over the HUD when the strip beside the safe zone has room.
    #[test]
    fn the_chat_goes_beside_the_safe_zone_only_where_there_is_room() {
        let wide = chat_zone(2560.0, 1080.0);
        assert_eq!(wide.min.x, WINDOW_MARGIN);
        assert_eq!(wide.max.y, 1080.0 - WINDOW_MARGIN, "the strip's chat sits on the window safe zone's bottom");
        let safe_left = (2560.0 - 1080.0 * SAFE_ZONE_ASPECT) / 2.0;
        assert!(wide.max.x <= safe_left, "it reached into the safe zone");
        assert!(wide.width() >= LEFT_STRIP_MIN_WIDTH);

        for (width, height) in [(1920.0, 1080.0), (385.0, 719.0), (800.0, 600.0)] {
            let narrow = chat_zone(width, height);
            assert_eq!(narrow.min.x, WINDOW_MARGIN, "{width}x{height}");
            assert!(narrow.max.x <= width - WINDOW_MARGIN, "{width}x{height} runs off the right");
            assert!(narrow.max.y <= height - WINDOW_MARGIN - HUD_CLEARANCE, "{width}x{height} covers the HUD");
            assert!(narrow.min.y >= 0.0, "{width}x{height} runs off the top");
        }
    }
}
