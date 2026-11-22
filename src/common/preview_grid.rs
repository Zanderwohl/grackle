//! A tile of a grid of previews, shared by the pickers ([`crate::common::picker`]) and the Library
//! ([`crate::common::library`]): a picture, or a stand-in where a kind has none yet.

use bevy_egui::egui;

use crate::get;

const PLACEHOLDER_GRAY: egui::Color32 = egui::Color32::from_gray(90);

#[derive(Clone, Copy)]
pub enum Face<'a> {
    Picture(egui::TextureId),
    /// White on gray, for a choice that says what it is in words.
    Text(&'a str),
    /// "[No preview]" over the name, black on white.
    NoPreview,
}

pub struct Tile<'a> {
    /// The caption, and the second line of [`Face::NoPreview`].
    pub label: &'a str,
    pub face: Face<'a>,
    pub badge: Option<&'a str>,
    pub tooltip: &'a str,
    pub selected: bool,
}

/// `size` is the face's; a caption is drawn above it. The response is the face's.
pub fn tile(ui: &mut egui::Ui, size: egui::Vec2, captioned: bool, tile: &Tile) -> egui::Response {
    if !captioned {
        return face(ui, size, tile);
    }
    ui.vertical(|ui| {
        ui.set_width(size.x);
        ui.add(egui::Label::new(tile.label).truncate());
        face(ui, size, tile)
    })
    .inner
}

fn face(ui: &mut egui::Ui, size: egui::Vec2, tile: &Tile) -> egui::Response {
    let inside = size - ui.spacing().button_padding * 2.0;
    let button = match tile.face {
        Face::Picture(texture) => egui::Button::image(egui::Image::new((texture, inside))),
        Face::Text(_) | Face::NoPreview => egui::Button::new(""),
    };
    let response = ui.add_sized(size, button.selected(tile.selected));
    let rect = egui::Rect::from_center_size(response.rect.center(), inside);
    match tile.face {
        Face::Picture(_) => {}
        Face::Text(text) => {
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 2.0, PLACEHOLDER_GRAY);
            let galley = painter.layout(text.to_owned(), font(inside), egui::Color32::WHITE, rect.width() - 4.0);
            painter.galley(rect.center() - galley.size() / 2.0, galley, egui::Color32::WHITE);
        }
        Face::NoPreview => no_preview(&ui.painter_at(rect), rect, tile.label),
    }
    if let Some(badge) = tile.badge {
        let painter = ui.painter_at(response.rect);
        let text = painter.layout_no_wrap(badge.to_owned(), egui::FontId::proportional(10.0), egui::Color32::WHITE);
        let rect = egui::Rect::from_min_size(
            response.rect.left_bottom() - egui::vec2(0.0, text.size().y + 2.0),
            text.size() + egui::vec2(4.0, 2.0),
        );
        painter.rect_filled(rect, 2.0, egui::Color32::from_black_alpha(180));
        painter.galley(rect.min + egui::vec2(2.0, 1.0), text, egui::Color32::WHITE);
    }
    response.on_hover_text(tile.tooltip)
}

/// Smaller in a small tile, so a picker's 64-point tile still fits a line or two.
fn font(inside: egui::Vec2) -> egui::FontId {
    egui::FontId::proportional((inside.min_elem() / 6.0).clamp(8.0, 16.0))
}

/// The stand-in for a kind with no preview yet.
pub fn no_preview(painter: &egui::Painter, rect: egui::Rect, name: &str) {
    painter.rect_filled(rect, 2.0, egui::Color32::WHITE);
    let width = rect.width() - 4.0;
    let mut job = egui::text::LayoutJob::default();
    let format = egui::TextFormat { font_id: font(rect.size()), color: egui::Color32::BLACK, ..Default::default() };
    job.append(&format!("{}\n{name}", get!("editor.picker.no_preview")), 0.0, format);
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 4, break_anywhere: true, ..Default::default() };
    job.halign = egui::Align::Center;
    let galley = painter.layout_job(job);
    painter.galley(egui::pos2(rect.center().x, rect.center().y - galley.size().y / 2.0), galley, egui::Color32::BLACK);
}
