//! Toolbar widgets shared by both editors. Every button names an icon; see
//! [`crate::common::icons`] for where it comes from and what stands in for it.

use bevy_egui::egui::{self, CornerRadius, Popup, RectAlign, Ui};

use crate::common::icons;

pub const BUTTON_HEIGHT: f32 = 28.0;
pub const BUTTON_MIN_WIDTH: f32 = 72.0;
pub const ROW_PADDING: f32 = 6.0;
const GAP: f32 = 6.0;
/// A hairline, so two idle joined buttons still read as two.
const SEAM: f32 = 1.0;
const ARROW_WIDTH: f32 = 16.0;


/// Which sides a button shares with a neighbor; those are squared off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Joins {
    pub left: bool,
    pub right: bool,
}

impl Joins {
    pub const NONE: Self = Self { left: false, right: false };
    pub const LEFT: Self = Self { left: true, right: false };
    pub const RIGHT: Self = Self { left: false, right: true };
    pub const BOTH: Self = Self { left: true, right: true };

    /// The joins of the `index`th of `len` buttons in a [`cluster`].
    pub fn in_run(index: usize, len: usize) -> Self {
        Self { left: index > 0, right: index + 1 < len }
    }

    fn corners(self, rounded: CornerRadius) -> CornerRadius {
        CornerRadius {
            nw: if self.left { 0 } else { rounded.nw },
            sw: if self.left { 0 } else { rounded.sw },
            ne: if self.right { 0 } else { rounded.ne },
            se: if self.right { 0 } else { rounded.se },
        }
    }
}

/// Wraps when narrow. Every widget in it is [`BUTTON_HEIGHT`] tall, combo boxes included.
pub fn row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(ROW_PADDING);
    let inner = ui
        .horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
            ui.spacing_mut().interact_size.y = BUTTON_HEIGHT;
            add(ui)
        })
        .inner;
    ui.add_space(ROW_PADDING);
    inner
}

/// Never wraps partway through. Each button states its own [`Joins`].
pub fn cluster<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = SEAM;
        add(ui)
    })
    .inner
}

fn labeled(ui: &Ui, icon_name: &str, label: String) -> egui::Button<'static> {
    egui::Button::new((icons::atom(ui.ctx(), icon_name), label)).image_tint_follows_text_color(true)
}

pub fn button(ui: &mut Ui, icon: &str, label: impl Into<String>, selected: bool, joins: Joins) -> egui::Response {
    button_enabled(ui, true, icon, label, selected, joins)
}

pub fn button_enabled(
    ui: &mut Ui,
    enabled: bool,
    icon: &str,
    label: impl Into<String>,
    selected: bool,
    joins: Joins,
) -> egui::Response {
    let rounded = ui.visuals().widgets.inactive.corner_radius;
    let button = labeled(ui, icon, label.into())
        .selected(selected)
        .min_size(egui::vec2(BUTTON_MIN_WIDTH, BUTTON_HEIGHT))
        .corner_radius(joins.corners(rounded));
    ui.add_enabled(enabled, button)
}

pub struct Segment {
    pub icon: &'static str,
    pub label: String,
    pub selected: bool,
    pub enabled: bool,
}

impl Segment {
    pub fn new(icon: &'static str, label: impl Into<String>) -> Self {
        Self { icon, label: label.into(), selected: false, enabled: true }
    }

    pub fn selected(self, selected: bool) -> Self {
        Self { selected, ..self }
    }

    pub fn enabled(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }
}

pub fn joined(ui: &mut Ui, segments: impl IntoIterator<Item = Segment>) -> Vec<egui::Response> {
    let segments: Vec<Segment> = segments.into_iter().collect();
    let len = segments.len();
    cluster(ui, |ui| {
        segments
            .into_iter()
            .enumerate()
            .map(|(index, segment)| {
                let joins = Joins::in_run(index, len);
                button_enabled(ui, segment.enabled, segment.icon, segment.label, segment.selected, joins)
            })
            .collect()
    })
}

pub fn clicked(responses: &[egui::Response]) -> Option<usize> {
    responses.iter().position(egui::Response::clicked)
}

/// A family of `len` tools behind one button showing the last used (index 0
/// at first, `active` while one is), with an arrow listing the rest. Returns
/// the index clicked. `entry` gives an icon and a label, and is asked only for
/// what is drawn.
///
/// `id_salt` must be unique across the context: the last-used choice is kept
/// under it rather than under the `Ui`'s id, so moving the toolbar keeps it.
pub fn split_button(
    ui: &mut Ui,
    id_salt: &str,
    len: usize,
    entry: impl Fn(usize) -> (&'static str, String),
    active: Option<usize>,
    joins: Joins,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let id = egui::Id::new(("toolbar_split", id_salt));
    let remembered = ui.data(|data| data.get_temp::<usize>(id));
    let shown = shown(active, remembered, len);
    if active.is_some() && active != remembered {
        ui.data_mut(|data| data.insert_temp(id, shown));
    }

    let mut chosen = None;
    cluster(ui, |ui| {
        let (icon, label) = entry(shown);
        let main = button(ui, icon, label, active.is_some(), Joins { left: joins.left, right: true });
        if main.clicked() {
            chosen = Some(shown);
        }

        let rounded = ui.visuals().widgets.inactive.corner_radius;
        let popup_id = id.with("list");
        let open = Popup::is_id_open(ui.ctx(), popup_id);
        let arrow = ui.add(
            egui::Button::new("")
                .selected(active.is_some() || open)
                .min_size(egui::vec2(ARROW_WIDTH, BUTTON_HEIGHT))
                .corner_radius(Joins { left: true, right: joins.right }.corners(rounded)),
        );
        paint_arrow(ui, &arrow);
        Popup::menu(&arrow)
            .id(popup_id)
            .anchor(main.rect.union(arrow.rect))
            .align(RectAlign::BOTTOM_START)
            .show(|ui| {
                for index in 0..len {
                    let (icon, label) = entry(index);
                    if ui.add(labeled(ui, icon, label).selected(active == Some(index))).clicked() {
                        chosen = Some(index);
                    }
                }
            });
    });

    if let Some(index) = chosen {
        ui.data_mut(|data| data.insert_temp(id, index));
    }
    chosen
}

fn shown(active: Option<usize>, remembered: Option<usize>, len: usize) -> usize {
    active.or(remembered).filter(|&index| index < len).unwrap_or(0)
}

/// Painted, not a glyph, so it does not depend on a pack's fonts.
fn paint_arrow(ui: &Ui, arrow: &egui::Response) {
    let color = ui.style().interact(arrow).fg_stroke.color;
    let center = arrow.rect.center();
    let half = ARROW_WIDTH * 0.2;
    let points = vec![
        center + egui::vec2(-half, -half * 0.5),
        center + egui::vec2(half, -half * 0.5),
        center + egui::vec2(0.0, half * 0.5),
    ];
    ui.painter().add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_joins_only_its_inner_edges() {
        assert_eq!(Joins::in_run(0, 1), Joins::NONE);
        assert_eq!(Joins::in_run(0, 3), Joins::RIGHT);
        assert_eq!(Joins::in_run(1, 3), Joins::BOTH);
        assert_eq!(Joins::in_run(2, 3), Joins::LEFT);
    }

    #[test]
    fn a_shared_edge_is_square_and_the_rest_keep_their_rounding() {
        let corners = Joins::LEFT.corners(CornerRadius::same(4));
        assert_eq!((corners.nw, corners.sw, corners.ne, corners.se), (0, 0, 4, 4));
    }

    #[test]
    fn a_split_button_shows_the_active_one_then_the_last_used_then_the_first() {
        assert_eq!(shown(None, None, 3), 0);
        assert_eq!(shown(None, Some(2), 3), 2);
        assert_eq!(shown(Some(1), Some(2), 3), 1);
        assert_eq!(shown(None, Some(5), 3), 0);
    }

    #[test]
    fn a_split_button_remembers_the_one_that_was_active() {
        let ctx = egui::Context::default();
        let frame = |active: Option<usize>| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                split_button(ui, "test", 3, |index| ("test", index.to_string()), active, Joins::NONE);
            });
            output.textures_delta.clear();
        };
        frame(Some(2));
        frame(None);
        let id = egui::Id::new(("toolbar_split", "test"));
        assert_eq!(ctx.data(|data| data.get_temp::<usize>(id)), Some(2));
    }
}
