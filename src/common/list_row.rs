//! A row of a listing (the animation editor's clips, the weapon editor's weapons, the lang
//! editor's files): as wide as the list whatever it names, clicked to pick, right-clicked for its
//! menu, and renamed in place.
//!
//! A rename is begun with [`begin_rename`], usually from the row's own menu, and kept in egui's
//! memory until [`renamable_row`] reports it committed or put down: Enter or leaving the field
//! commits, Escape puts it back.

use bevy_egui::egui;

use crate::get;

/// Square, before the text.
pub struct Leading {
    pub picture: Option<egui::TextureId>,
    pub size: f32,
}

/// Picked, hovered or plain; the text truncated rather than widening the list.
pub fn row(ui: &mut egui::Ui, selected: bool, text: impl Into<egui::WidgetText>, leading: Option<Leading>) -> egui::Response {
    let padding = ui.spacing().button_padding;
    let width = ui.available_width();
    let lead = leading.as_ref().map_or(0.0, |leading| leading.size + ui.spacing().item_spacing.x);
    let galley = text.into().into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        width - lead - padding.x * 2.0,
        egui::TextStyle::Button,
    );
    let height = galley.size().y.max(leading.as_ref().map_or(0.0, |leading| leading.size)) + padding.y * 2.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let fill = if selected {
            Some(visuals.selection.bg_fill)
        } else if response.hovered() || response.has_focus() {
            Some(visuals.widgets.hovered.weak_bg_fill)
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(rect, visuals.widgets.hovered.corner_radius, fill);
        }
        let mut x = rect.left() + padding.x;
        if let Some(leading) = leading {
            let square = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - leading.size / 2.0), egui::Vec2::splat(leading.size));
            if let Some(picture) = leading.picture {
                egui::Image::new((picture, square.size())).paint_at(ui, square);
            }
            x += lead;
        }
        let color = if selected { visuals.selection.stroke.color } else { visuals.text_color() };
        ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, color);
    }
    response
}

#[derive(Clone)]
struct Renaming {
    row: egui::Id,
    text: String,
    /// Focus is asked for once, on the first frame the field is drawn.
    focused: bool,
}

fn renaming_id() -> egui::Id {
    egui::Id::new("list_row_renaming")
}

/// One rename in the whole interface at a time: beginning another puts the first down.
pub fn begin_rename(ctx: &egui::Context, row: egui::Id, name: &str) {
    ctx.data_mut(|data| data.insert_temp(renaming_id(), Renaming { row, text: name.to_owned(), focused: false }));
}

pub fn any_rename(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<Renaming>(renaming_id())).is_some()
}

pub fn is_renaming(ctx: &egui::Context, row: egui::Id) -> bool {
    ctx.data(|data| data.get_temp::<Renaming>(renaming_id())).is_some_and(|renaming| renaming.row == row)
}

pub enum RowEvent {
    /// The row as drawn, with no rename open on it.
    Row(egui::Response),
    /// Still being typed.
    Renaming,
    /// What was typed, which `problem` found nothing wrong with.
    Renamed(String),
    /// Put back with Escape, or left with a problem.
    PutBack,
}

/// [`row`], or the field renaming it while a rename is open on `id`. `problem` says what is wrong
/// with a name typed: shown under the field, and Enter does nothing until it is fixed.
pub fn renamable_row(
    ui: &mut egui::Ui,
    id: egui::Id,
    selected: bool,
    text: impl Into<egui::WidgetText>,
    leading: Option<Leading>,
    problem: impl Fn(&str) -> Option<String>,
) -> RowEvent {
    let Some(mut renaming) = ui.data(|data| data.get_temp::<Renaming>(renaming_id())).filter(|renaming| renaming.row == id) else {
        return RowEvent::Row(row(ui, selected, text, leading));
    };
    let field = ui.add(egui::TextEdit::singleline(&mut renaming.text).desired_width(f32::INFINITY));
    if !renaming.focused {
        field.request_focus();
        renaming.focused = true;
    }
    let wrong = problem(renaming.text.trim());
    if let Some(wrong) = &wrong {
        ui.colored_label(ui.visuals().error_fg_color, wrong);
    }
    let (escape, enter) = ui.input(|input| (input.key_pressed(egui::Key::Escape), input.key_pressed(egui::Key::Enter)));
    let event = if escape {
        RowEvent::PutBack
    } else if field.lost_focus() {
        match (wrong, enter) {
            (None, _) => RowEvent::Renamed(renaming.text.trim().to_owned()),
            // Enter on a bad name keeps the field up to fix it.
            (Some(_), true) => {
                renaming.focused = false;
                RowEvent::Renaming
            }
            (Some(_), false) => RowEvent::PutBack,
        }
    } else {
        RowEvent::Renaming
    };
    ui.data_mut(|data| match event {
        RowEvent::Renaming => {
            data.insert_temp(renaming_id(), renaming);
        }
        _ => data.remove::<Renaming>(renaming_id()),
    });
    event
}

/// Asks before deleting `name`. `Some(true)` once confirmed, `Some(false)` once declined.
/// `more` is drawn under the question, for what else would be affected.
pub fn confirm_delete(ctx: &egui::Context, id: egui::Id, name: &str, more: impl FnOnce(&mut egui::Ui)) -> Option<bool> {
    let mut answer = None;
    let modal = egui::Modal::new(id).show(ctx, |ui| {
        ui.set_min_width(300.0);
        ui.heading(get!("library.delete.heading"));
        ui.label(get!("library.delete.body", "name", name));
        more(ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(get!("library.delete.confirm")).clicked() || ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                answer = Some(true);
            }
            if ui.button(get!("library.delete.cancel")).clicked() {
                answer = Some(false);
            }
        });
    });
    if answer.is_none() && modal.should_close() {
        answer = Some(false);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    /// One frame of a row named "walk", with the events given.
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, problem: fn(&str) -> Option<String>) -> RowEvent {
        let mut event = None;
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            event = Some(renamable_row(ui, egui::Id::new("row"), false, "walk", None, problem));
        });
        // Nothing renders here, so the font atlas's upload is dropped on purpose.
        output.textures_delta.clear();
        event.unwrap()
    }

    fn fine(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn enter_commits_what_was_typed_and_escape_puts_it_back() {
        let ctx = egui::Context::default();
        assert!(matches!(frame(&ctx, vec![], fine), RowEvent::Row(_)), "a row with no rename open is a row");

        begin_rename(&ctx, egui::Id::new("row"), "walk");
        assert!(matches!(frame(&ctx, vec![], fine), RowEvent::Renaming));
        assert!(matches!(frame(&ctx, vec![egui::Event::Text("_fast".into())], fine), RowEvent::Renaming));
        match frame(&ctx, vec![key(egui::Key::Enter)], fine) {
            RowEvent::Renamed(name) => assert_eq!(name, "walk_fast"),
            _ => panic!("Enter did not commit"),
        }
        assert!(!any_rename(&ctx), "a committed rename stayed open");

        begin_rename(&ctx, egui::Id::new("row"), "walk");
        frame(&ctx, vec![], fine);
        frame(&ctx, vec![egui::Event::Text("x".into())], fine);
        assert!(matches!(frame(&ctx, vec![key(egui::Key::Escape)], fine), RowEvent::PutBack));
        assert!(!any_rename(&ctx));
    }

    #[test]
    fn enter_on_a_name_with_a_problem_keeps_the_field_up() {
        let ctx = egui::Context::default();
        let refuse = |_: &str| Some("no".to_owned());
        begin_rename(&ctx, egui::Id::new("row"), "walk");
        frame(&ctx, vec![], refuse);
        assert!(matches!(frame(&ctx, vec![key(egui::Key::Enter)], refuse), RowEvent::Renaming));
        assert!(any_rename(&ctx), "a refused name closed the field");
    }
}
