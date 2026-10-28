//! Dragging one item of an ordered feature list, where an item must stay below
//! everything it depends on and above everything that depends on it. Shared by
//! the map's Features panel and the prop editor's feature list.

use std::ops::RangeInclusive;

use bevy_egui::egui;

/// Where the item at `at` may be moved to, in a list of `len`. `depends_on`
/// and `dependants` are the current indices of its direct relations; indirect
/// ones are already beyond a direct one. Always contiguous, since moving one
/// item leaves the others' order alone.
///
/// Indices count the list with the item taken out, which is the index it will
/// end up at.
pub fn legal_range(
    len: usize,
    at: usize,
    depends_on: impl IntoIterator<Item = usize>,
    dependants: impl IntoIterator<Item = usize>,
) -> RangeInclusive<usize> {
    let without = |i: usize| if i < at { i } else { i - 1 };

    let low = depends_on.into_iter().map(|index| without(index) + 1).max().unwrap_or(0);
    let high = dependants
        .into_iter()
        .map(without)
        .fold(len.saturating_sub(1), usize::min);

    // An out-of-order list would give an inverted range, and `clamp` panics on
    // one.
    low..=high.max(low)
}

/// Whether the drag in progress has been called off with Escape or the right button. Remembered in
/// egui's memory under `list` until the drag ends, because egui goes on reporting the drag until the
/// left button comes up, and the release must not drop it.
pub fn cancelled(ui: &egui::Ui, list: egui::Id, dragging: bool) -> bool {
    let key = list.with("reorder_cancelled");
    if !dragging {
        ui.data_mut(|data| data.remove::<bool>(key));
        return false;
    }
    let now = ui.input(|input| {
        input.key_pressed(egui::Key::Escape) || input.pointer.button_pressed(egui::PointerButton::Secondary)
    });
    let cancelled = now || ui.data(|data| data.get_temp::<bool>(key).unwrap_or(false));
    if cancelled {
        ui.data_mut(|data| data.insert_temp(key, true));
    }
    cancelled
}

/// Draws where a dragged row would land and marks the rows that pin it, and
/// returns that landing index. `rects` is every row in order, the dragged one
/// included.
pub fn show_the_drop<Id: PartialEq>(
    ui: &egui::Ui,
    rects: &[(Id, egui::Rect)],
    dragged: &Id,
    pins: &[Id],
    legal: &RangeInclusive<usize>,
) -> usize {
    let others: Vec<egui::Rect> = rects
        .iter()
        .filter(|(id, _)| id != dragged)
        .map(|(_, rect)| *rect)
        .collect();
    if others.is_empty() {
        return 0;
    }

    let pointer = ui
        .ctx()
        .pointer_interact_pos()
        .map(|pointer| pointer.y)
        .unwrap_or(f32::NEG_INFINITY);
    // Midpoints rather than edges, so the line flips to the next gap when the
    // pointer is more than half way into a row.
    let wanted = others.iter().take_while(|rect| rect.center().y < pointer).count();
    let target = wanted.clamp(*legal.start(), *legal.end());

    let painter = ui.painter();
    for (id, rect) in rects {
        if pins.contains(id) {
            // A bar rather than an outline, so it does not clash with the
            // selection highlight.
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
                0.0,
                ui.visuals().warn_fg_color,
            );
        }
    }

    let line = match others.get(target) {
        Some(rect) => rect.top(),
        // Past the last row, where an item with no dependants may go.
        None => others.last().expect("checked non-empty above").bottom(),
    };
    painter.hline(
        others[0].x_range(),
        line,
        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
    );

    target
}

/// How the rollback bar was handled this frame.
#[derive(Clone, Copy, Default)]
pub struct BarDrag {
    pub dragging: bool,
    pub dropped: bool,
}

/// The rollback bar, under the row just drawn: faint at the bottom of the list, where it changes
/// nothing, and bold elsewhere so a partial view is not mistaken for the whole. Its id is fixed rather
/// than taken from where it is laid out, which is what lets a drag carry it past more than one row.
pub fn rollback_bar(ui: &mut egui::Ui, list: egui::Id, rolled_back: bool) -> BarDrag {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::hover());
    let bar = ui
        .interact(rect, list.with("rollback_bar"), egui::Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeVertical);
    let lit = rolled_back || bar.hovered() || bar.dragged();
    let stroke = match lit {
        true => egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
        false => egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
    };
    ui.painter().hline(rect.x_range(), rect.center().y, stroke);
    BarDrag { dragging: bar.dragged(), dropped: bar.drag_stopped() }
}

/// While the bar is dragged, draws where it would land: under the row nearest above the pointer, and
/// never above the first, since an empty view shows nothing. Returns that row on release, and nothing
/// if Escape or the right button called the drag off. Applied once rather than live, because each step
/// of a roll back rebuilds what it crosses.
///
/// `rows` span each row and anything drawn under it before the next.
pub fn drop_the_bar<Id: Copy>(ui: &egui::Ui, list: egui::Id, rows: &[(Id, egui::Rect)], drag: BarDrag) -> Option<Id> {
    let active = drag.dragging || drag.dropped;
    if cancelled(ui, list.with("rollback_bar"), active) || !active {
        return None;
    }
    let pointer = ui.ctx().pointer_interact_pos()?.y;
    let above = rows.iter().take_while(|(_, rect)| rect.center().y < pointer).count();
    let at = above.max(1) - 1;
    let (anchor, rect) = rows.get(at)?;

    let line = match rows.get(at + 1) {
        Some((_, next)) => (rect.bottom() + next.top()) / 2.0,
        None => rect.bottom() + ui.spacing().item_spacing.y,
    };
    ui.painter().hline(
        rect.x_range(),
        line,
        egui::Stroke::new(3.0, ui.visuals().selection.stroke.color),
    );

    drag.dropped.then_some(*anchor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_range_stops_short_of_both_relations() {
        // Five items; the one at 2 depends on 0 and is depended on by 4.
        assert_eq!(legal_range(5, 2, [0], [4]), 1..=3);
        assert_eq!(legal_range(5, 2, [], []), 0..=4);
        assert_eq!(legal_range(5, 2, [1], [3]), 2..=2, "pinned on both sides, it can only stay");
    }
}
