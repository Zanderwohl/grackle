//! The toolbar widgets both editors are built from, so a tool looks and sizes
//! the same whichever editor it is in.
//!
//! Every button carries an icon slot. There are no icons yet, so each draws
//! [`PLACEHOLDER_ICON`]; the room it takes is the point.

use bevy_egui::egui::{self, CornerRadius, Popup, RectAlign, Ui};

pub const BUTTON_HEIGHT: f32 = 28.0;
pub const BUTTON_MIN_WIDTH: f32 = 72.0;
pub const ROW_PADDING: f32 = 6.0;
/// Between unrelated controls in a row.
const GAP: f32 = 6.0;
/// Between buttons that share an edge: a hairline, so two idle buttons still
/// read as two.
const SEAM: f32 = 1.0;
const ARROW_WIDTH: f32 = 16.0;

pub const PLACEHOLDER_ICON: &str = "□";
const ARROW: &str = "⏷";

/// Which of a button's sides it shares with a neighbor. A shared side is
/// squared off; the rest keep the theme's rounding.
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

/// A row of toolbar controls that wraps when the panel is narrow. Every
/// widget in it is [`BUTTON_HEIGHT`] tall, combo boxes included.
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

/// Buttons that share edges. Each states its own [`Joins`] — usually
/// [`Joins::in_run`] — and the cluster never wraps partway through.
pub fn cluster<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = SEAM;
        add(ui)
    })
    .inner
}

/// A tool: lit while `selected`.
pub fn button(ui: &mut Ui, label: impl Into<String>, selected: bool, joins: Joins) -> egui::Response {
    button_enabled(ui, true, label, selected, joins)
}

pub fn button_enabled(
    ui: &mut Ui,
    enabled: bool,
    label: impl Into<String>,
    selected: bool,
    joins: Joins,
) -> egui::Response {
    let rounded = ui.visuals().widgets.inactive.corner_radius;
    ui.add_enabled(
        enabled,
        egui::Button::new((PLACEHOLDER_ICON, label.into()))
            .selected(selected)
            .min_size(egui::vec2(BUTTON_MIN_WIDTH, BUTTON_HEIGHT))
            .corner_radius(joins.corners(rounded)),
    )
}

/// One button standing for a family of tools, with a narrow arrow beside it
/// listing the rest. The button is the last one used — first `labels[0]`, and
/// whichever is `active` while one is — and picking from the list makes that
/// one the button. Returns the index clicked, from either half.
///
/// `id_salt` must be unique in the `Ui`: the last-used choice is kept under it.
pub fn split_button(
    ui: &mut Ui,
    id_salt: &str,
    labels: &[String],
    active: Option<usize>,
    joins: Joins,
) -> Option<usize> {
    if labels.is_empty() {
        return None;
    }
    let id = ui.make_persistent_id(id_salt);
    let remembered = ui.data(|data| data.get_temp::<usize>(id));
    let shown = active.or(remembered).filter(|&index| index < labels.len()).unwrap_or(0);
    if active.is_some() && active != remembered {
        ui.data_mut(|data| data.insert_temp(id, shown));
    }

    let mut chosen = None;
    cluster(ui, |ui| {
        let main = button(ui, labels[shown].clone(), active.is_some(), Joins { left: joins.left, right: true });
        if main.clicked() {
            chosen = Some(shown);
        }

        let rounded = ui.visuals().widgets.inactive.corner_radius;
        let popup_id = id.with("list");
        let open = Popup::is_id_open(ui.ctx(), popup_id);
        let arrow = ui.add(
            egui::Button::new(ARROW)
                .selected(active.is_some() || open)
                .min_size(egui::vec2(ARROW_WIDTH, BUTTON_HEIGHT))
                .corner_radius(Joins { left: true, right: joins.right }.corners(rounded)),
        );
        Popup::menu(&arrow)
            .id(popup_id)
            .anchor(main.rect.union(arrow.rect))
            .align(RectAlign::BOTTOM_START)
            .show(|ui| {
                for (index, label) in labels.iter().enumerate() {
                    if ui
                        .add(egui::Button::new((PLACEHOLDER_ICON, label.as_str())).selected(active == Some(index)))
                        .clicked()
                    {
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
}
