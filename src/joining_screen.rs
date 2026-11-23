//! What [`AppMode::Joining`] draws: a small window over a background picture, naming the server,
//! with a progress bar, the phase it is in and a Disconnect button. The state behind it is
//! [`crate::common::joining`].

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::{AppMode, JoinedFrom};
use crate::common::joining::{progress, JoinBuild};
use crate::common::net::{NetRequest, NetRole};
use crate::common::resource::sync::Fetch;
use crate::get;
use crate::title_background::{BackgroundSet, WantsBackground};

const BACKGROUND: Color = Color::srgb(0.07, 0.07, 0.08);
const WINDOW_WIDTH: f32 = 360.0;

pub struct JoiningScreenPlugin;

impl Plugin for JoiningScreenPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::title_background::TitleBackgroundPlugin>() {
            app.add_plugins(crate::title_background::TitleBackgroundPlugin);
        }
        app.add_systems(Update, show_or_hide)
            .add_systems(EguiPrimaryContextPass, joining_window.run_if(in_state(AppMode::Joining)));
    }
}

#[derive(Component)]
struct JoiningRoot;

/// Under the egui window, on the egui context's camera, as the main menu's root is.
fn show_or_hide(
    mut commands: Commands,
    mode: Res<State<AppMode>>,
    existing: Query<(Entity, Has<UiTargetCamera>), With<JoiningRoot>>,
    camera: Query<Entity, With<bevy_egui::PrimaryEguiContext>>,
) {
    let wanted = *mode.get() == AppMode::Joining;
    let camera = camera.single().ok();
    if wanted && existing.is_empty() {
        let mut root = commands.spawn((
            JoiningRoot,
            WantsBackground(BackgroundSet::Loading),
            // Over the viewport labels, as the main menu is.
            GlobalZIndex(10),
            Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
            BackgroundColor(BACKGROUND),
        ));
        if let Some(camera) = camera {
            root.insert(UiTargetCamera(camera));
        }
        return;
    }
    for (root, targeted) in &existing {
        match (wanted, targeted, camera) {
            (false, _, _) => commands.entity(root).despawn(),
            (true, false, Some(camera)) => {
                commands.entity(root).insert(UiTargetCamera(camera));
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn joining_window(
    mut contexts: EguiContexts,
    fetch: Res<Fetch>,
    build: Res<JoinBuild>,
    role: Res<NetRole>,
    joined: Option<Res<JoinedFrom>>,
    timeline: Option<Res<crate::editor::editable::FeatureTimeline>>,
    bakes: Option<Res<crate::tool::bake_job::BakeProgress>>,
    mut requests: MessageWriter<NetRequest>,
    mut next: ResMut<NextState<AppMode>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let server = match (fetch.server.trim(), &*role) {
        ("", NetRole::Client { host, port }) => format!("{host}:{port}"),
        (name, _) => name.to_owned(),
    };
    let shown = progress(&fetch, &build, timeline.as_deref(), bakes.as_deref());
    let mut disconnect = false;
    egui::Window::new(get!("joining.title", "server", &server))
        .id(egui::Id::new("joining"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .fixed_size([WINDOW_WIDTH, 0.0])
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.viewport_rect().center())
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading(get!("joining.title", "server", &server));
                ui.add_space(8.0);
                ui.add(egui::ProgressBar::new(shown.fraction).show_percentage());
                ui.add(egui::Label::new(&shown.line).truncate());
                ui.add_space(8.0);
                disconnect = ui.button(get!("joining.disconnect")).clicked();
            });
        });
    if disconnect {
        info!("Disconnecting from {server} while joining");
        requests.write(NetRequest::GoSolo);
        next.set(joined.as_deref().copied().unwrap_or_default().after_leaving());
    }
}
