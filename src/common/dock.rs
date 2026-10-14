//! The frame every authoring mode's docked side panels are drawn in: no inner
//! margin, since the dock's tab body already pads its contents.

use bevy_egui::egui;

pub fn dock_panel_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).inner_margin(0)
}
