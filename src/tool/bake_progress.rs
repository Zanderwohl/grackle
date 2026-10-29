//! Drawing [`BakeProgress`]: which bake of all of them is running, and how far it and the whole
//! lot have got. A step is one bake, in [`BakeKind`] order.

use bevy_egui::egui;

use crate::get;
use crate::tool::bake_job::{BakeKind, BakeProgress, BakeState, BakeStatus};

/// The bake running, and where it stands among all of them.
#[derive(Clone, Debug, PartialEq)]
pub struct Step<'a> {
    /// 1-based.
    pub step: usize,
    pub steps: usize,
    pub kind: BakeKind,
    pub status: &'a BakeStatus,
}

impl<'a> Step<'a> {
    /// `None` when nothing is baking. Statuses come in [`BakeKind`] order.
    pub fn of(statuses: impl IntoIterator<Item = (BakeKind, &'a BakeStatus)>) -> Option<Self> {
        let statuses: Vec<_> = statuses.into_iter().collect();
        let steps = statuses.len();
        statuses
            .into_iter()
            .enumerate()
            .find(|(_, (_, status))| status.state == BakeState::Baking)
            .map(|(index, (kind, status))| Step { step: index + 1, steps, kind, status })
    }

    pub fn current(progress: &'a BakeProgress) -> Option<Self> {
        Self::of(progress.iter())
    }

    /// From 0 to 1, counting every bake before this one as done.
    pub fn overall(&self) -> f32 {
        ((self.step - 1) as f32 + self.status.fraction()) / self.steps as f32
    }

    pub fn within(&self) -> f32 {
        self.status.fraction()
    }

    pub fn line(&self) -> String {
        get!(
            "bakes.progress.status",
            "step", self.step,
            "steps", self.steps,
            "completed", self.status.completed,
            "scheduled", self.status.scheduled,
            "doing", self.status.doing
        )
        .trim_end()
        .to_owned()
    }
}

pub fn kind_name(kind: BakeKind) -> String {
    match kind {
        BakeKind::Rooms => get!("bakes.kind.rooms"),
        BakeKind::Sunlight => get!("bakes.kind.sunlight"),
    }
}

/// `width` is the bar's, in points; `None` fills what is left.
pub fn bar(ui: &mut egui::Ui, fraction: f32, width: Option<f32>) -> egui::Response {
    let bar = egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage();
    match width {
        Some(width) => ui.add(bar.desired_width(width)),
        None => ui.add(bar),
    }
}

/// For the Bakes panel: what is baking, the status line, and both bars.
pub fn section(ui: &mut egui::Ui, progress: &BakeProgress) {
    let Some(step) = Step::current(progress) else { return };
    ui.strong(kind_name(step.kind));
    ui.label(step.line());
    egui::Grid::new("bake_progress").num_columns(2).show(ui, |ui| {
        ui.label(get!("bakes.progress.step"));
        bar(ui, step.within(), None);
        ui.end_row();
        ui.label(get!("bakes.progress.overall"));
        bar(ui, step.overall(), None);
        ui.end_row();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: BakeState, completed: usize, scheduled: usize, doing: &str) -> BakeStatus {
        BakeStatus { state, auto: true, completed, scheduled, doing: doing.to_owned() }
    }

    #[test]
    fn nothing_baking_is_no_step() {
        let rooms = status(BakeState::UpToDate, 4, 4, "");
        let sun = status(BakeState::Waiting, 0, 0, "");
        assert_eq!(Step::of([(BakeKind::Rooms, &rooms), (BakeKind::Sunlight, &sun)]), None);
        assert_eq!(Step::of([]), None);
    }

    #[test]
    fn the_step_is_the_baking_ones_place_among_all_of_them() {
        let rooms = status(BakeState::UpToDate, 4, 4, "");
        let sun = status(BakeState::Baking, 3, 12, "Hall");
        let step = Step::of([(BakeKind::Rooms, &rooms), (BakeKind::Sunlight, &sun)]).unwrap();
        assert_eq!((step.step, step.steps, step.kind), (2, 2, BakeKind::Sunlight));
        assert_eq!(step.line(), "Step 2/2: 3/12 Hall");
    }

    #[test]
    fn an_idle_item_leaves_no_trailing_space() {
        let rooms = status(BakeState::Baking, 0, 5, "");
        let step = Step::of([(BakeKind::Rooms, &rooms)]).unwrap();
        assert_eq!(step.line(), "Step 1/1: 0/5");
    }

    #[test]
    fn overall_counts_earlier_bakes_as_done() {
        let rooms = status(BakeState::UpToDate, 4, 4, "");
        let sun = status(BakeState::Baking, 1, 4, "Hall");
        let step = Step::of([(BakeKind::Rooms, &rooms), (BakeKind::Sunlight, &sun)]).unwrap();
        assert_eq!(step.within(), 0.25);
        assert_eq!(step.overall(), (1.0 + 0.25) / 2.0);
    }

    #[test]
    fn the_first_of_two_is_at_most_half_overall() {
        let rooms = status(BakeState::Baking, 3, 4, "Hall");
        let sun = status(BakeState::Waiting, 0, 0, "");
        let step = Step::of([(BakeKind::Rooms, &rooms), (BakeKind::Sunlight, &sun)]).unwrap();
        assert_eq!(step.step, 1);
        assert_eq!(step.overall(), 0.75 / 2.0);
    }
}
