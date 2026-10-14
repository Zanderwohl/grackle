//! The frame every authoring mode's docked side panels are drawn in.
//!
//! An `egui::Panel` pads its contents by default, which around a `DockArea`
//! is a gutter between the tab bar and the panel's edge — the tab body already
//! pads what is inside it. Shared so the three editors cannot drift apart.

use bevy_egui::egui;

/// A side or top panel's own frame, minus its inner margin.
pub fn dock_panel_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).inner_margin(0)
}
