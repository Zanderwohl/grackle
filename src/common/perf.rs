//! The F3 performance overlay.
//!
//! Hand-rolled on `bevy_egui` rather than a crate that would need a fork per
//! Bevy release; reads only `FrameTimeDiagnosticsPlugin` and the fixed clock.

use bevy::app::{App, Plugin, Update};
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::time::{Fixed, Real};
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::get;

pub struct PerfPlugin;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        app
            .add_plugins(FrameTimeDiagnosticsPlugin::new(256))
            .init_state::<DebugState>()
            .add_systems(Update, toggle_perf)
            .add_systems(
                EguiPrimaryContextPass,
                perf_ui.run_if(in_state(DebugState::AllPerf)),
            )
        ;
    }
}

fn toggle_perf(
    actions: Res<crate::common::keybind::Actions>,
    state: Res<State<DebugState>>,
    mut next_state: ResMut<NextState<DebugState>>,
) {
    if actions.just_pressed(crate::common::keybind::Action::PerfOverlay) {
        match state.get() {
            DebugState::Off => {
                next_state.set(DebugState::AllPerf);
            },
            DebugState::AllPerf => {
                next_state.set(DebugState::Off);
            },
        }
    }
}

fn perf_ui(
    mut contexts: EguiContexts,
    diagnostics: Res<DiagnosticsStore>,
    fixed: Res<Time<Fixed>>,
    real: Res<Time<Real>>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut lang_rates: Local<LangRates>,
) {
    let ctx = contexts.ctx_mut();
    if ctx.is_err() { warn!("perf::perf_ui: no egui context: {}", ctx.unwrap_err()); return; }
    let ctx = ctx.unwrap();

    let lang = crate::common::lang::cache_stats();
    lang_rates.update(lang, real.elapsed_secs_f64());

    let smoothed = |key: &bevy::diagnostic::DiagnosticPath| {
        diagnostics.get(key).and_then(|d| d.smoothed())
    };

    egui::Window::new(get!("debug.perf.title"))
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::RIGHT_TOP, [-8.0, 8.0])
        .show(ctx, |ui| {
            egui::Grid::new("debug_perf_grid")
                .num_columns(2)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    let mut row = |label: String, value: String| {
                        ui.label(label);
                        ui.monospace(value);
                        ui.end_row();
                    };

                    row(
                        get!("debug.perf.fps"),
                        match smoothed(&FrameTimeDiagnosticsPlugin::FPS) {
                            Some(fps) => format!("{fps:.1}"),
                            None => "—".to_owned(),
                        },
                    );
                    row(
                        get!("debug.perf.frame_time"),
                        match smoothed(&FrameTimeDiagnosticsPlugin::FRAME_TIME) {
                            Some(ms) => format!("{ms:.2} ms"),
                            None => "—".to_owned(),
                        },
                    );
                    row(
                        get!("debug.perf.frame_count"),
                        match diagnostics
                            .get(&FrameTimeDiagnosticsPlugin::FRAME_COUNT)
                            .and_then(|d| d.value())
                        {
                            Some(frames) => format!("{frames:.0}"),
                            None => "—".to_owned(),
                        },
                    );

                    // Physics runs on the fixed clock, so its rate and lag are
                    // what matter when a step overruns its budget.
                    row(
                        get!("debug.perf.fixed_rate"),
                        format!("{:.0} Hz", 1.0 / fixed.timestep().as_secs_f64()),
                    );
                    row(
                        get!("debug.perf.fixed_overstep"),
                        format!("{:.1} ms", fixed.overstep().as_secs_f64() * 1000.0),
                    );

                    row(
                        get!("debug.perf.uptime"),
                        format!("{:.0} s", real.elapsed_secs_f64()),
                    );

                    row(get!("debug.perf.lang_cache"), format!("{} / {}", lang.entries, lang.capacity));
                    row(
                        get!("debug.perf.lang_hit_rate"),
                        match lang_rates.hit_rate {
                            Some(rate) => format!("{:.2}%", rate * 100.0),
                            None => "—".to_owned(),
                        },
                    );
                    row(
                        get!("debug.perf.lang_churn"),
                        get!("debug.perf.per_second",
                            "misses", format!("{:.0}", lang_rates.misses),
                            "rotations", format!("{:.1}", lang_rates.rotations)),
                    );

                    if let Ok(window) = window.single() {
                        row(
                            get!("debug.perf.resolution"),
                            format!("{} x {}", window.physical_width(), window.physical_height()),
                        );
                        row(
                            get!("debug.perf.scale_factor"),
                            format!("{:.2}", window.scale_factor()),
                        );
                    }
                });
        });
}

/// The string cache's hit rate, misses and rotations over the last whole second.
#[derive(Default)]
struct LangRates {
    since: Option<(f64, crate::common::lang::CacheStats)>,
    last_seen: f64,
    hit_rate: Option<f64>,
    misses: f64,
    rotations: f64,
}

impl LangRates {
    /// Longer than any frame, so a gap this long means the overlay was closed.
    const GAP_SECONDS: f64 = 0.5;

    fn update(&mut self, now: crate::common::lang::CacheStats, seconds: f64) {
        let reopened = seconds - self.last_seen > Self::GAP_SECONDS;
        self.last_seen = seconds;
        let Some((then, before)) = self.since.filter(|_| !reopened) else {
            *self = Self { since: Some((seconds, now)), last_seen: seconds, ..default() };
            return;
        };
        let elapsed = seconds - then;
        if elapsed >= 1.0 {
            let hits = now.hits - before.hits;
            let misses = now.misses - before.misses;
            self.hit_rate = (hits + misses > 0).then(|| hits as f64 / (hits + misses) as f64);
            self.misses = misses as f64 / elapsed;
            self.rotations = (now.rotations - before.rotations) as f64 / elapsed;
            self.since = Some((seconds, now));
        }
    }
}

#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
enum DebugState {
    #[default]
    Off,
    AllPerf,
}
