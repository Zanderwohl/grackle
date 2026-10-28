use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use bevy_egui::egui::UiKind;

use crate::common::app_mode::AppMode;
use crate::common::net::{parse_endpoint, NetRequest, NetRole};
use crate::common::net_transport::NetStatus;
use crate::constants::DEFAULT_PORT;
use crate::get;

/// Which half of the connect dialog is showing. One dialog, so a wrong pick is one click from the
/// other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectMode {
    Host,
    Join,
}

/// A floating window rather than a dock tab: it is a modal question, not a work surface, and must
/// be reachable in any layout.
#[derive(Resource)]
pub struct ConnectDialog {
    pub open: bool,
    pub mode: ConnectMode,
    /// Text rather than a parsed address, so a half-typed one is not rejected while typing.
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
        // Not reset when already open in this mode, so a half-typed address survives.
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

    /// Separate from drawing so it is testable without egui.
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

/// Draws the connect dialog; the menu item that opens it is [`network_menu`].
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

/// The `Multiplayer` menu, drawn into the editor's existing menu bar, which is built in one pass by
/// whoever owns the top panel.
pub fn network_menu(
    ui: &mut egui::Ui,
    role: &NetRole,
    status: &NetStatus,
    dialog: &mut ConnectDialog,
    requests: &mut MessageWriter<NetRequest>,
) {
    ui.menu_button(get!("net.menu.title"), |ui| {
        // The role is what was asked for; the status is what the socket is doing. A host that never
        // answered shows as `Client` and `Connecting`.
        ui.label(role.describe());
        ui.label(status.describe());
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

    // Read after the closure, because the closure borrows `dialog`.
    let mut open = true;
    let mut submitted = false;
    let mut canceled = false;

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
            if field.changed() {
                dialog.error = None;
            }
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
                    canceled = true;
                }
            });
        });

    if canceled || !open {
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

    /// Switching modes must replace the default, or the port box opens holding `localhost:27100` and
    /// refuses it.
    #[test]
    fn switching_mode_reseeds_the_box() {
        let mut dialog = ConnectDialog::default();
        dialog.show(ConnectMode::Join);
        assert!(dialog.submit().is_ok());
        dialog.show(ConnectMode::Host);
        assert!(dialog.submit().is_ok(), "the join default was left in the port box");
    }

    #[test]
    fn reopening_the_same_mode_keeps_what_was_typed() {
        let mut dialog = ConnectDialog::default();
        dialog.show(ConnectMode::Join);
        dialog.endpoint = "example.net".to_owned();
        dialog.show(ConnectMode::Join);
        assert_eq!(dialog.endpoint, "example.net");
    }
}
