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
    pub fn of(statuses: &[(BakeKind, &'a BakeStatus)]) -> Option<Self> {
        Self::find(statuses.iter().copied(), statuses.len())
    }

    pub fn current(progress: &'a BakeProgress) -> Option<Self> {
        Self::find(progress.iter(), progress.iter().count())
    }

    fn find(statuses: impl Iterator<Item = (BakeKind, &'a BakeStatus)>, steps: usize) -> Option<Self> {
        statuses
            .enumerate()
            .find(|(_, (_, status))| status.state == BakeState::Baking)
            .map(|(index, (kind, status))| Step { step: index + 1, steps, kind, status })
    }

    /// From 0 to 1, counting every bake before this one as done.
    pub fn overall(&self) -> f32 {
        ((self.step - 1) as f32 + self.status.fraction()) / self.steps as f32
    }

    pub fn within(&self) -> f32 {
        self.status.fraction()
    }

    pub fn line(&self) -> String {
        let status = self.status;
        if status.doing.is_empty() {
            get!(
                "bakes.progress.status_idle",
                "step", self.step,
                "steps", self.steps,
                "completed", status.completed,
                "scheduled", status.scheduled
            )
        } else {
            get!(
                "bakes.progress.status",
                "step", self.step,
                "steps", self.steps,
                "completed", status.completed,
                "scheduled", status.scheduled,
                "doing", status.doing
            )
        }
    }
}

pub fn kind_name(kind: BakeKind) -> String {
    match kind {
        BakeKind::Rooms => get!("bakes.kind.rooms"),
        BakeKind::Sunlight => get!("bakes.kind.sunlight"),
    }
}

pub fn bar(ui: &mut egui::Ui, fraction: f32, width: f32) -> egui::Response {
    ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage().desired_width(width))
}

/// For the Bakes panel: what is baking, the status line, and both bars. Nothing when idle.
pub fn section(ui: &mut egui::Ui, progress: &BakeProgress) {
    let Some(step) = Step::current(progress) else { return };
    ui.separator();
    ui.strong(kind_name(step.kind));
    ui.label(step.line());
    // Rows rather than a `Grid`: a grid sizes its columns from last frame, and a bar filling
    // the column feeds its own width back into that.
    for (label, fraction) in [(get!("bakes.progress.step"), step.within()), (get!("bakes.progress.overall"), step.overall())] {
        ui.horizontal(|ui| {
            ui.label(label);
            bar(ui, fraction, ui.available_width());
        });
    }
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
        assert_eq!(Step::of(&[(BakeKind::Rooms, &rooms), (BakeKind::Sunlight, &sun)]), None);
        assert_eq!(Step::of(&[]), None);
    }

    #[test]
    fn the_step_is_the_baking_ones_place_among_all_of_them() {
        let rooms = status(BakeState::UpToDate, 4, 4, "");
        let sun = status(BakeState::Baking, 3, 12, "Hall");
        let step = Step::of(&[(BakeKind::Rooms, &rooms), (BakeKind::Sunlight, &sun)]).unwrap();
        assert_eq!((step.step, step.steps, step.kind), (2, 2, BakeKind::Sunlight));
        assert_eq!(step.line(), "Step 2/2: 3/12 Hall");
    }

    #[test]
    fn an_unnamed_item_leaves_no_trailing_space() {
        let rooms = status(BakeState::Baking, 0, 5, "");
        let step = Step::of(&[(BakeKind::Rooms, &rooms)]).unwrap();
        assert_eq!(step.line(), "Step 1/1: 0/5");
    }

    #[test]
    fn overall_counts_earlier_bakes_as_done() {
        let rooms = status(BakeState::UpToDate, 4, 4, "");
        let sun = status(BakeState::Baking, 1, 4, "Hall");
        let step = Step::of(&[(BakeKind::Rooms, &rooms), (BakeKind::Sunlight, &sun)]).unwrap();
        assert_eq!(step.within(), 0.25);
        assert_eq!(step.overall(), (1.0 + 0.25) / 2.0);
    }

    #[test]
    fn the_first_of_two_is_at_most_half_overall() {
        let rooms = status(BakeState::Baking, 3, 4, "Hall");
        let sun = status(BakeState::Waiting, 0, 0, "");
        let step = Step::of(&[(BakeKind::Rooms, &rooms), (BakeKind::Sunlight, &sun)]).unwrap();
        assert_eq!(step.step, 1);
        assert_eq!(step.overall(), 0.75 / 2.0);
    }
}
