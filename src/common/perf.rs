//! The F3 performance overlay.
//!
//! Deliberately hand-rolled on top of `bevy_egui` rather than pulled from a
//! crate: the one that used to serve this (`iyes_perf_ui`) is unmaintained
//! upstream and was pinned to a git branch per Bevy release, which is a wasm
//! blocker and a recurring cost at every engine bump. Everything shown here
//! comes out of `FrameTimeDiagnosticsPlugin` and the fixed clock, so there is
//! nothing to port next time.

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
    keyboard: Res<ButtonInput<KeyCode>>,
    state: Res<State<DebugState>>,
    mut next_state: ResMut<NextState<DebugState>>,
) {
    if keyboard.just_pressed(KeyCode::F3) {
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
) {
    let ctx = contexts.ctx_mut();
    if ctx.is_err() { warn!("{}", ctx.unwrap_err()); return; }
    let ctx = ctx.unwrap();

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

                    // The fixed clock is the one physics runs on, so its rate
                    // and how far behind it is are what actually matter when a
                    // step starts costing more than its budget.
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

#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
enum DebugState {
    #[default]
    Off,
    AllPerf,
}
