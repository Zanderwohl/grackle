//! Drawings that say what a number means where the number alone does not: damage over distance,
//! the curve bots weigh a weapon by and the one the range's damage numbers should match, and a
//! pellet pattern against a body at two distances.

use bevy_egui::egui;

use crate::common::class::{CLASS_RADIUS, TALLEST_CLASS_HEIGHT};
use crate::common::weapon::{distance_scale, DistanceScale, PelletGrid, Unit, WeaponAction};
use crate::get;

use super::form::unit_key;

const HEIGHT: f32 = 90.0;
const SAMPLES: usize = 64;

/// The damage and the two lines an action scales it by, and how far it reaches.
fn curve_of(action: &WeaponAction) -> Option<(f32, Option<DistanceScale>, Option<DistanceScale>, f32)> {
    let (damage, ramp, fall, reach) = match action {
        WeaponAction::Hitscan(spec) => (spec.damage as f32, spec.ramp_up, spec.falloff, spec.range_m),
        WeaponAction::Flame(spec) => (spec.damage as f32, spec.ramp_up, spec.falloff, spec.range_m),
        // A blast is scaled by the shooter's distance to it, which has no reach of its own.
        WeaponAction::Projectile(spec) => (spec.damage as f32, spec.ramp_up, spec.falloff, f32::INFINITY),
        _ => return None,
    };
    if ramp.is_none() && fall.is_none() {
        return None;
    }
    // Past the last line's end it is flat, so show half as far again and no further than it reaches.
    let ends = ramp.iter().chain(fall.iter()).map(|line| line.far.max(line.near)).fold(0.0, f32::max);
    Some((damage, ramp, fall, (ends * 1.5).max(1.0).min(reach)))
}

pub fn damage_over_distance(ui: &mut egui::Ui, action: &WeaponAction) {
    let Some((damage, ramp, fall, extent)) = curve_of(action) else { return };
    let meters = get!(unit_key(Unit::Meters));
    let at = |distance: f32| damage * distance_scale(ramp, fall, distance);
    let points: Vec<(f32, f32)> =
        (0..=SAMPLES).map(|i| extent * i as f32 / SAMPLES as f32).map(|d| (d, at(d))).collect();
    let most = points.iter().map(|(_, y)| *y).fold(1.0, f32::max) * 1.1;

    let (response, painter) = ui.allocate_painter(egui::vec2(ui.available_width(), HEIGHT), egui::Sense::hover());
    let rect = response.rect;
    let visuals = ui.visuals();
    painter.rect_stroke(rect, 2.0, visuals.widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    let to_screen = |(d, y): (f32, f32)| {
        egui::pos2(rect.left() + rect.width() * d / extent, rect.bottom() - rect.height() * y / most)
    };
    painter.add(egui::Shape::line(
        points.iter().copied().map(to_screen).collect(),
        egui::Stroke::new(2.0, visuals.selection.bg_fill),
    ));
    let font = egui::FontId::monospace(11.0);
    let color = visuals.weak_text_color();
    painter.text(rect.left_top() + egui::vec2(4.0, 2.0), egui::Align2::LEFT_TOP, format!("{:.0}", at(0.0)), font.clone(), color);
    painter.text(
        rect.right_top() + egui::vec2(-4.0, 2.0),
        egui::Align2::RIGHT_TOP,
        format!("{:.0}", at(extent)),
        font.clone(),
        color,
    );
    painter.text(rect.left_bottom() + egui::vec2(4.0, -2.0), egui::Align2::LEFT_BOTTOM, format!("0{meters}"), font.clone(), color);
    painter.text(
        rect.right_bottom() + egui::vec2(-4.0, -2.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("{extent:.0}{meters}"),
        font,
        color,
    );
    response.on_hover_ui_at_pointer(|ui| {
        if let Some(pointer) = ui.ctx().pointer_hover_pos() {
            let d = ((pointer.x - rect.left()) / rect.width() * extent).clamp(0.0, extent);
            ui.label(format!("{d:.1}{meters}: {:.1}", at(d)));
        }
    });
}

/// Boxes this many meters across, so a body and the pattern are to scale.
const WINDOW: f32 = 2.5;

pub fn pellet_pattern(ui: &mut egui::Ui, grid: &PelletGrid) {
    let meters = get!(unit_key(Unit::Meters));
    let side = (ui.available_width() / 2.0 - ui.spacing().item_spacing.x).min(HEIGHT * 1.2);
    ui.horizontal(|ui| {
        for distance in [10.0, 30.0] {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            let visuals = ui.visuals();
            painter.rect_stroke(rect, 2.0, visuals.widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
            let scale = side / WINDOW;
            // The body's box, centered on the aim, as tall as the tallest class.
            let body = egui::Rect::from_center_size(
                rect.center(),
                egui::vec2(CLASS_RADIUS * 2.0 * scale, TALLEST_CLASS_HEIGHT * scale),
            );
            painter.rect_filled(body, 2.0, visuals.faint_bg_color.gamma_multiply(3.0));
            for offset in grid.offsets() {
                let at = rect.center() + egui::vec2(offset.x, -offset.y) * distance * scale;
                painter.circle_filled(at, 2.5, visuals.selection.bg_fill);
            }
            painter.text(
                rect.left_top() + egui::vec2(4.0, 2.0),
                egui::Align2::LEFT_TOP,
                format!("{distance:.0}{meters}"),
                egui::FontId::monospace(11.0),
                visuals.weak_text_color(),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon::HitscanSpec;

    #[test]
    fn a_flat_weapon_draws_no_curve() {
        assert!(curve_of(&WeaponAction::Hitscan(HitscanSpec::default())).is_none());
    }

    /// Fails if the graph stops short of a line's far end, which hides where it goes flat.
    #[test]
    fn the_curve_runs_past_the_last_line() {
        let spec = HitscanSpec {
            falloff: Some(DistanceScale { near: 5.0, far: 10.0, scale: 0.5 }),
            ..HitscanSpec::default()
        };
        let (_, _, _, extent) = curve_of(&WeaponAction::Hitscan(spec)).expect("a curve");
        assert!(extent >= 10.0 && extent <= spec.range_m, "{extent}");
    }
}
