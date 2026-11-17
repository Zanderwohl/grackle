//! A round asked for while the bakes it waits on are behind stays in the editor and bakes them in
//! the background, under a window showing how far along they are, rather than entering Play and
//! freezing the frame on them. Without this plugin entering Play still bakes what it must, blocked.
//!
//! On the authority only: a client follows the server's mode, and holding it back here would
//! leave it a round behind with nothing to tell it the round has since ended.

use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::AppMode;
use crate::common::net::NetRole;
use crate::common::round_settings::RoundSettings;
use crate::get;
use crate::tool::bake_job::{BakeKind, BakeProgress, BakeState};
use crate::tool::bake_progress;
use crate::tool::nav_bake::BakeNav;
use crate::tool::room::CalculateRoomGeometry;
use crate::tool::sun_bake::BakeSunlight;

/// What `OnEnter(AppMode::Play)` would otherwise bake while the frame waits. Sightlines are left
/// out: a round never waits for them.
const WAITED_FOR: [BakeKind; 3] = [BakeKind::Rooms, BakeKind::Sunlight, BakeKind::Nav];

/// What a round waits for: the nav graph only while bots may play.
fn waited_for(settings: Option<&RoundSettings>) -> impl Iterator<Item = BakeKind> {
    let bots = RoundSettings::wants_bots(settings);
    WAITED_FOR.into_iter().filter(move |kind| bots || *kind != BakeKind::Nav)
}

const WINDOW_WIDTH: f32 = 360.0;

/// A round being held for its bakes.
#[derive(Resource, Default)]
pub struct RoundPrep {
    /// The frame the hold began. The bakes' statuses are read from the frame after, once they
    /// have heard the request.
    since: Option<u32>,
}

impl RoundPrep {
    pub fn is_waiting(&self) -> bool {
        self.since.is_some()
    }
}

fn ready(progress: &BakeProgress, settings: Option<&RoundSettings>) -> bool {
    waited_for(settings).all(|kind| progress.get(kind).is_none_or(|status| matches!(status.state, BakeState::UpToDate | BakeState::Skipped)))
}

fn asked_to_play(next: &NextState<AppMode>) -> bool {
    matches!(next, NextState::Pending(AppMode::Play) | NextState::PendingIfDifferent(AppMode::Play))
}

/// Before `StateTransition`, so a request made last frame is held before it is answered.
#[allow(clippy::too_many_arguments)]
fn hold_the_round_for_its_bakes(
    mode: Res<State<AppMode>>,
    role: Option<Res<NetRole>>,
    mut next: ResMut<NextState<AppMode>>,
    mut prep: ResMut<RoundPrep>,
    progress: Res<BakeProgress>,
    frame: Res<FrameCount>,
    mut bake_rooms: MessageWriter<CalculateRoomGeometry>,
    mut bake_sunlight: MessageWriter<BakeSunlight>,
    mut bake_nav: MessageWriter<BakeNav>,
    settings: Option<Res<RoundSettings>>,
) {
    let settings = settings.as_deref();
    if *mode.get() == AppMode::Play || !role.is_none_or(|role| role.is_authority()) {
        prep.since = None;
        return;
    }
    match prep.since {
        None if asked_to_play(&next) && !ready(&progress, settings) => {
            next.reset();
            prep.since = Some(frame.0);
            // With auto-bake on this only skips the wait for the map to settle; the sun waits for
            // the rooms either way.
            bake_rooms.write(CalculateRoomGeometry);
            bake_sunlight.write(BakeSunlight);
            if RoundSettings::wants_bots(settings) {
                bake_nav.write(BakeNav);
            }
        }
        Some(since) => {
            // Asked again while waiting: it is the same round.
            if asked_to_play(&next) {
                next.reset();
            }
            if frame.0 > since + 1 && ready(&progress, settings) {
                prep.since = None;
                next.set(AppMode::Play);
            }
        }
        None => {}
    }
}

fn show_the_wait(
    mut contexts: EguiContexts,
    mut prep: ResMut<RoundPrep>,
    progress: Res<BakeProgress>,
    actions: Res<crate::common::keybind::Actions>,
    settings: Option<Res<RoundSettings>>,
) {
    if !prep.is_waiting() {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let mut cancel = actions.just_pressed(crate::common::keybind::Action::Cancel);
    egui::Modal::new(egui::Id::new("round_prep")).show(ctx, |ui| {
        ui.set_width(WINDOW_WIDTH);
        ui.heading(get!("round_prep.title"));
        ui.add_space(4.0);
        for kind in waited_for(settings.as_deref()) {
            let Some(status) = progress.get(kind) else { continue };
            ui.horizontal(|ui| {
                ui.label(bake_progress::kind_name(kind));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(match status.state {
                        BakeState::UpToDate => get!("round_prep.done"),
                        BakeState::Skipped => get!("round_prep.skipped"),
                        BakeState::Baking => format!("{:.0}%", status.fraction() * 100.0),
                        BakeState::Waiting | BakeState::Stale => get!("round_prep.waiting"),
                    });
                });
            });
            let fraction = match status.state {
                BakeState::UpToDate | BakeState::Skipped => 1.0,
                BakeState::Baking => status.fraction(),
                BakeState::Waiting | BakeState::Stale => 0.0,
            };
            bake_progress::bar(ui, fraction, ui.available_width());
            if status.state == BakeState::Baking && !status.doing.is_empty() {
                ui.weak(&status.doing);
            }
        }
        ui.add_space(4.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            cancel |= ui.button(get!("round_prep.cancel")).clicked();
        });
    });
    if cancel {
        prep.since = None;
    }
}

pub struct RoundPrepPlugin;

impl Plugin for RoundPrepPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        app.init_resource::<RoundPrep>()
            .add_systems(PreUpdate, hold_the_round_for_its_bakes)
            .add_systems(EguiPrimaryContextPass, show_the_wait);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::bake_job::BakeStatus;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin));
        app.init_state::<AppMode>();
        app.init_resource::<BakeProgress>();
        app.add_message::<CalculateRoomGeometry>();
        app.add_message::<BakeSunlight>();
        app.add_message::<BakeNav>();
        app.init_resource::<RoundPrep>();
        app.add_systems(PreUpdate, hold_the_round_for_its_bakes);
        app.update();
        app
    }

    fn set(app: &mut App, kind: BakeKind, state: BakeState) {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut()
            .run_system_once(move |mut progress: ResMut<BakeProgress>| crate::tool::bake_job::report(&mut progress, kind, BakeStatus { state, ..default() }))
            .unwrap();
    }

    fn mode(app: &App) -> AppMode {
        *app.world().resource::<State<AppMode>>().get()
    }

    /// Breaks if a round with nothing to bake waits a frame, or one with something behind starts
    /// before it lands: the frame would freeze on it.
    #[test]
    fn a_round_waits_in_the_editor_for_its_bakes() {
        let mut app = app();
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        assert_eq!(mode(&app), AppMode::Play, "a round with nothing to bake was held");

        let mut app = self::app();
        set(&mut app, BakeKind::Nav, BakeState::Stale);
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        assert!(!app.world().resource::<Messages<BakeNav>>().is_empty(), "the nav graph was not asked to bake");
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(mode(&app), AppMode::Editor, "the round started on a stale nav graph");
        assert!(app.world().resource::<RoundPrep>().is_waiting());

        set(&mut app, BakeKind::Nav, BakeState::UpToDate);
        app.update();
        app.update();
        assert_eq!(mode(&app), AppMode::Play, "the bakes landed and the round never started");
        assert!(!app.world().resource::<RoundPrep>().is_waiting());
    }

    /// Breaks if halfbright holds a round for a sunlight bake that will never run.
    #[test]
    fn a_skipped_bake_does_not_hold_the_round() {
        let mut app = app();
        set(&mut app, BakeKind::Sunlight, BakeState::Skipped);
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        assert_eq!(mode(&app), AppMode::Play, "a round was held for a skipped bake");
    }

    /// Breaks if a round without bots waits for, or asks for, a nav graph nothing will read.
    #[test]
    fn a_round_without_bots_does_not_wait_for_the_nav_graph() {
        let mut app = app();
        app.insert_resource(RoundSettings { bots_enabled: false, ..default() });
        set(&mut app, BakeKind::Nav, BakeState::Stale);
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        assert_eq!(mode(&app), AppMode::Play, "a round without bots was held for the nav graph");
        assert!(app.world().resource::<Messages<BakeNav>>().is_empty(), "the nav graph was asked to bake");
    }
}
