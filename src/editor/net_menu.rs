use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use bevy_egui::egui::UiKind;

use crate::common::app_mode::AppMode;
use crate::common::net::{parse_endpoint, NetRequest, NetRole};
use crate::constants::DEFAULT_PORT;
use crate::get;

/// Which half of the connect dialog is showing.
///
/// Hosting and joining are the same dialog because they are the same decision
/// — "who is simulating" — and a mapper who picked the wrong one should be one
/// click from the other rather than back out in the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectMode {
    Host,
    Join,
}

/// The little modal that asks for a host and a port.
///
/// A floating `egui::Window` rather than an `egui_dock` tab, deliberately: the
/// panels rule exists so that editor *tools* dock alongside each other, and
/// this is a modal question with an answer, not a surface you work in. It also
/// has to be reachable while a map is open in any layout.
#[derive(Resource)]
pub struct ConnectDialog {
    pub open: bool,
    pub mode: ConnectMode,
    /// What the user is typing. Kept as text rather than a parsed host and
    /// port so that a half-typed address is not repeatedly rejected while it
    /// is still being typed.
    pub endpoint: String,
    /// Filled in on a refused submit, cleared on the next keystroke.
    pub error: Option<String>,
}

impl Default for ConnectDialog {
    fn default() -> Self {
        Self {
            open: false,
            mode: ConnectMode::Join,
            endpoint: format!("localhost:{DEFAULT_PORT}"),
            error: None,
        }
    }
}

impl ConnectDialog {
    fn show(&mut self, mode: ConnectMode) {
        // Not reset if it is already open in this mode: re-picking the menu
        // item should not throw away a half-typed address.
        if !self.open || self.mode != mode {
            self.endpoint = match mode {
                ConnectMode::Host => DEFAULT_PORT.to_string(),
                ConnectMode::Join => format!("localhost:{DEFAULT_PORT}"),
            };
            self.error = None;
        }
        self.mode = mode;
        self.open = true;
    }

    /// Turn what was typed into a request, or into a reason it was refused.
    ///
    /// Split out from the drawing so the grammar of the box is testable
    /// without an egui context.
    pub fn submit(&self) -> Result<NetRequest, String> {
        match self.mode {
            ConnectMode::Host => {
                let text = self.endpoint.trim();
                match text.parse::<u16>() {
                    Ok(0) | Err(_) => Err(get!("net.error.port", "port", text)),
                    Ok(port) => Ok(NetRequest::Host { port }),
                }
            }
            ConnectMode::Join => {
                let (host, port) = parse_endpoint(&self.endpoint)?;
                Ok(NetRequest::Join { host, port })
            }
        }
    }
}

/// Owns the connect dialog and draws it.
///
/// The menu item that opens it lives in the editor's menu bar — see
/// [`network_menu`] — but the window itself is drawn here, so that adding to
/// it does not mean reaching into the panel layout.
pub struct NetMenuPlugin;

impl Plugin for NetMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConnectDialog>()
            .add_systems(
                EguiPrimaryContextPass,
                connect_dialog.run_if(in_state(AppMode::Editor)),
            );
    }
}

/// The `Multiplayer` menu, drawn into the editor's existing menu bar.
///
/// Takes the pieces it needs rather than being a system of its own: two menu
/// bars in one window is two menu bars, and `egui::MenuBar` is built in one
/// pass by whoever owns the top panel.
pub fn network_menu(
    ui: &mut egui::Ui,
    role: &NetRole,
    dialog: &mut ConnectDialog,
    requests: &mut MessageWriter<NetRequest>,
) {
    ui.menu_button(get!("net.menu.title"), |ui| {
        // What we currently are, before what we could become: the answer to
        // "am I hosting?" should not require opening a dialog to find out.
        ui.label(role.describe());
        ui.separator();

        if ui.button(get!("net.menu.host")).clicked() {
            ui.close_kind(UiKind::Menu);
            dialog.show(ConnectMode::Host);
        }
        if ui.button(get!("net.menu.join")).clicked() {
            ui.close_kind(UiKind::Menu);
            dialog.show(ConnectMode::Join);
        }
        ui.separator();
        // Enabled only when there is something to leave, so the menu says
        // whether we are online without being read carefully.
        if ui
            .add_enabled(role.is_online(), egui::Button::new(get!("net.menu.solo")))
            .clicked()
        {
            ui.close_kind(UiKind::Menu);
            requests.write(NetRequest::GoSolo);
        }
    });
}

fn connect_dialog(
    mut contexts: EguiContexts,
    mut dialog: ResMut<ConnectDialog>,
    mut requests: MessageWriter<NetRequest>,
) {
    if !dialog.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };

    let title = match dialog.mode {
        ConnectMode::Host => get!("net.dialog.host_title"),
        ConnectMode::Join => get!("net.dialog.join_title"),
    };

    // `open` drives the window's own close button; `submitted` is set by the
    // buttons inside it. Both are read after the closure, because the closure
    // borrows `dialog` and the request writer would be a second borrow.
    let mut open = true;
    let mut submitted = false;
    let mut cancelled = false;

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(match dialog.mode {
                ConnectMode::Host => get!("net.dialog.port_label"),
                ConnectMode::Join => get!("net.dialog.endpoint_label"),
            });

            // Read out before the field borrows `dialog.endpoint` mutably.
            let hint = match dialog.mode {
                ConnectMode::Host => DEFAULT_PORT.to_string(),
                ConnectMode::Join => format!("localhost:{DEFAULT_PORT}"),
            };
            let field = ui.add(
                egui::TextEdit::singleline(&mut dialog.endpoint)
                    .desired_width(240.0)
                    .hint_text(hint),
            );
            // A stale complaint about an address that has since been corrected
            // reads as the correction having been rejected too.
            if field.changed() {
                dialog.error = None;
            }
            // Return submits, as in every other address box.
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submitted = true;
            }

            if let Some(error) = &dialog.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }

            ui.separator();
            ui.horizontal(|ui| {
                let confirm = match dialog.mode {
                    ConnectMode::Host => get!("net.dialog.host_confirm"),
                    ConnectMode::Join => get!("net.dialog.join_confirm"),
                };
                if ui.button(confirm).clicked() {
                    submitted = true;
                }
                if ui.button(get!("net.dialog.cancel")).clicked() {
                    cancelled = true;
                }
            });

            // No transport behind any of this yet, and the box should say so
            // rather than leaving somebody watching for a connection that was
            // never going to be attempted.
            ui.separator();
            ui.label(get!("net.dialog.unimplemented"));
        });

    if cancelled || !open {
        dialog.open = false;
        dialog.error = None;
        return;
    }
    if !submitted {
        return;
    }

    match dialog.submit() {
        Ok(request) => {
            requests.write(request);
            dialog.open = false;
            dialog.error = None;
        }
        Err(message) => dialog.error = Some(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(mode: ConnectMode, text: &str) -> ConnectDialog {
        ConnectDialog {
            open: true,
            mode,
            endpoint: text.to_owned(),
            error: None,
        }
    }

    #[test]
    fn joining_takes_a_host_and_an_optional_port() {
        assert!(matches!(
            dialog(ConnectMode::Join, "example.net:27016").submit(),
            Ok(NetRequest::Join { host, port }) if host == "example.net" && port == 27016
        ));
        assert!(matches!(
            dialog(ConnectMode::Join, "example.net").submit(),
            Ok(NetRequest::Join { port, .. }) if port == DEFAULT_PORT
        ));
    }

    /// The host box is a port, not an address: typing a hostname into it is a
    /// mistake worth naming rather than binding something surprising.
    #[test]
    fn hosting_takes_only_a_port() {
        assert!(matches!(
            dialog(ConnectMode::Host, "27016").submit(),
            Ok(NetRequest::Host { port: 27016 })
        ));
        assert!(dialog(ConnectMode::Host, "localhost:27016").submit().is_err());
        assert!(dialog(ConnectMode::Host, "0").submit().is_err());
    }

    #[test]
    fn an_empty_box_is_refused() {
        assert!(dialog(ConnectMode::Join, "   ").submit().is_err());
        assert!(dialog(ConnectMode::Host, "").submit().is_err());
    }

    /// Switching modes has to replace the default, or the port box opens
    /// holding `localhost:27100` and refuses it.
    #[test]
    fn switching_mode_reseeds_the_box() {
        let mut dialog = ConnectDialog::default();
        dialog.show(ConnectMode::Join);
        assert!(dialog.submit().is_ok());
        dialog.show(ConnectMode::Host);
        assert!(dialog.submit().is_ok(), "the join default was left in the port box");
    }

    /// But re-opening the mode already showing must not: somebody halfway
    /// through typing an address who brushes the menu should not lose it.
    #[test]
    fn reopening_the_same_mode_keeps_what_was_typed() {
        let mut dialog = ConnectDialog::default();
        dialog.show(ConnectMode::Join);
        dialog.endpoint = "example.net".to_owned();
        dialog.show(ConnectMode::Join);
        assert_eq!(dialog.endpoint, "example.net");
    }
}
