//! [`AppMode::Starting`]: the first screen of a bare launch, a title picture under a progress
//! window in the bottom right, up while the first frames compile pipelines and bake the startup
//! map. Without it those frames are a still window that reads as a hung process. It hands over to
//! [`AfterStartup`] once every step is done and frames have come quickly for a while, or after
//! [`GIVE_UP_SECONDS`] whatever is left.
//!
//! The warm-ups wait behind a shut [`WarmUpGate`] until the picture has been drawn for
//! [`FRAMES_BEFORE_WARMING_UP`] frames: each pipeline they compile holds its frame back from the
//! window, and before the picture is up that is a blank window that does not answer.
//!
//! The window opens hidden (`launch.rs`) and is shown once the picture has been drawn
//! ([`show_the_window`]), or at once by a launch that does not start here.
//!
//! Over the picture, centered, is the title in Nabla (`fonts/Nabla_Regular/`). It is a COLRv1
//! color font, which Bevy's text does not paint, so it is drawn from its plain outlines in one
//! color. The window waits for the font as it does for the picture.
//!
//! The picture is a [`BackgroundSet::Startup`] pick, which the main menu shows again on its first
//! showing, so the one screen leads into the other.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::AppMode;
use crate::common::joining::{bake_fraction, settled};
use crate::game::warm_up::{WarmUp, WarmUpGate};
use crate::get;
use crate::title_background::{BackgroundSet, TitleBackground, WantsBackground};
use crate::tool::bake_job::{BakeKind, BakeProgress, BakeStatus};

const BACKGROUND: Color = Color::srgb(0.07, 0.07, 0.08);
/// The folder under a pack's `fonts/` the title is set in; the default font without it.
const TITLE_FONT_FOLDER: &str = "Nabla_Regular";
/// 100 pt at 96 logical px to the inch, since Bevy sizes text in logical pixels.
const TITLE_SIZE_PX: f32 = 100.0 * 96.0 / 72.0;
const WINDOW_WIDTH: f32 = 320.0;
/// From the window's edges, as the window safe zone is.
const MARGIN: f32 = 20.0;

/// A frame quicker than this counts toward settling: one that compiles a pipeline on macOS takes
/// far longer, and a debug build's ordinary frame is well under it.
const QUICK_FRAME_SECONDS: f32 = 0.1;
const QUICK_FRAMES: u32 = 10;
pub const GIVE_UP_SECONDS: f32 = 20.0;
/// Frames the picture is drawn before the warm-ups start, so it reaches the window first.
const FRAMES_BEFORE_WARMING_UP: u32 = 2;
/// Frames the picture is drawn before the window is shown, so it never shows empty.
const FRAMES_BEFORE_SHOWING: u32 = 1;
/// The window is shown by now whatever the picture is doing.
const SHOW_BY_SECONDS: f32 = 3.0;

/// Where [`AppMode::Starting`] goes once it is done. Inserted with that state by
/// [`crate::onboarding::OnboardingPlugin`].
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AfterStartup(pub AppMode);

pub struct StartupScreenPlugin;

impl Plugin for StartupScreenPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<crate::title_background::TitleBackgroundPlugin>() {
            app.add_plugins(crate::title_background::TitleBackgroundPlugin);
        }
        // Shut before `Startup`, which spawns the warm-up cameras, and only for a launch that
        // starts here: a start flag goes straight to a mode that draws the world.
        if app.world().contains_resource::<AfterStartup>() {
            app.insert_resource(WarmUpGate { open: false });
        }
        app.init_resource::<StartupSteps>()
            .add_systems(
                Update,
                (
                    show_or_hide,
                    show_the_title.after(show_or_hide).before(watch_the_steps),
                    (watch_the_steps, finish_starting).chain().run_if(in_state(AppMode::Starting)),
                    open_the_warm_ups.after(watch_the_steps),
                    show_the_window.after(watch_the_steps),
                ),
            )
            .add_systems(EguiPrimaryContextPass, startup_window.run_if(in_state(AppMode::Starting)));
    }
}

/// What startup has left, measured each frame by [`watch_the_steps`].
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct StartupSteps {
    /// The most warm-up entities seen at once, so the share done is measured against it.
    pub warm_ups_total: usize,
    pub warm_ups_left: usize,
    pub lens_warming: bool,
    /// The picture and the title are both on screen.
    pub picture_ready: bool,
    pub rooms: Option<BakeStatus>,
    pub light: Option<BakeStatus>,
    /// Frames in a row quicker than [`QUICK_FRAME_SECONDS`].
    pub quick_frames: u32,
    /// Real seconds since the screen went up.
    pub seconds: f32,
}

const WARM_UP_WEIGHT: f32 = 4.0;
const PICTURE_WEIGHT: f32 = 1.0;
const ROOMS_WEIGHT: f32 = 2.0;
const LIGHT_WEIGHT: f32 = 2.0;
const SETTLE_WEIGHT: f32 = 1.0;

#[derive(Clone, Debug, PartialEq)]
pub struct StartupProgress {
    pub fraction: f32,
    pub line: String,
}

impl StartupSteps {
    fn warmed_up(&self) -> bool {
        self.warm_ups_left == 0 && !self.lens_warming
    }

    fn warm_up_fraction(&self) -> f32 {
        // The lens counts as one more entity, so a lens left alone is not shown as done.
        let total = self.warm_ups_total + 1;
        let left = self.warm_ups_left + usize::from(self.lens_warming);
        1.0 - left as f32 / total as f32
    }

    pub fn done(&self) -> bool {
        self.warmed_up()
            && self.picture_ready
            && settled(self.rooms.as_ref())
            && settled(self.light.as_ref())
            && self.quick_frames >= QUICK_FRAMES
    }

    pub fn progress(&self) -> StartupProgress {
        let settle = (self.quick_frames as f32 / QUICK_FRAMES as f32).min(1.0);
        let done = self.warm_up_fraction() * WARM_UP_WEIGHT
            + f32::from(u8::from(self.picture_ready)) * PICTURE_WEIGHT
            + bake_fraction(self.rooms.as_ref()) * ROOMS_WEIGHT
            + bake_fraction(self.light.as_ref()) * LIGHT_WEIGHT
            + settle * SETTLE_WEIGHT;
        let total = WARM_UP_WEIGHT + PICTURE_WEIGHT + ROOMS_WEIGHT + LIGHT_WEIGHT + SETTLE_WEIGHT;
        let line = match () {
            _ if !self.warmed_up() => get!("startup.compiling"),
            _ if !self.picture_ready => get!("startup.background"),
            _ if !settled(self.rooms.as_ref()) => get!("startup.rooms"),
            _ if !settled(self.light.as_ref()) => get!("startup.light"),
            _ => get!("startup.settling"),
        };
        StartupProgress { fraction: (done / total).clamp(0.0, 1.0), line }
    }
}

#[derive(Component)]
struct StartupRoot;

/// Hidden until its font has loaded, so it never shows in the fallback face first.
#[derive(Component)]
struct StartupTitle(Handle<Font>);

/// Under the egui window, on the egui context's camera, as the joining screen's root is.
fn show_or_hide(
    mut commands: Commands,
    mode: Res<State<AppMode>>,
    existing: Query<(Entity, Has<UiTargetCamera>), With<StartupRoot>>,
    camera: Query<Entity, With<bevy_egui::PrimaryEguiContext>>,
    title_font: TitleFont,
) {
    let wanted = *mode.get() == AppMode::Starting;
    let camera = camera.single().ok();
    if wanted && existing.is_empty() {
        let font = title_font.load();
        let mut root = commands.spawn((
            StartupRoot,
            WantsBackground(BackgroundSet::Startup),
            GlobalZIndex(10),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(BACKGROUND),
            children![(
                StartupTitle(font.clone()),
                Text::new(get!("startup.studio")),
                TextFont { font: font.into(), font_size: bevy::text::FontSize::Px(TITLE_SIZE_PX), ..default() },
                TextColor(Color::WHITE),
                TextShadow::default(),
                Pickable::IGNORE,
                Visibility::Hidden,
            )],
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

#[derive(bevy::ecs::system::SystemParam)]
struct TitleFont<'w> {
    fonts: Option<Res<'w, crate::common::fonts::PackFonts>>,
    packs: Option<Res<'w, crate::common::assets::Assets>>,
    asset_server: Option<Res<'w, AssetServer>>,
}

impl TitleFont<'_> {
    fn load(&self) -> Handle<Font> {
        let (Some(fonts), Some(packs), Some(asset_server)) = (&self.fonts, &self.packs, &self.asset_server) else {
            return Handle::default();
        };
        let Some(font) = fonts.fonts.iter().find(|font| font.folder == TITLE_FONT_FOLDER) else {
            warn!("No pack has the font {TITLE_FONT_FOLDER}; the startup title is in the default font");
            return Handle::default();
        };
        match packs.asset_path(&font.face_pack, &font.face_path()) {
            Some(path) => asset_server.load(path),
            None => Handle::default(),
        }
    }
}

/// A font that will not load is swapped for the default rather than leaving the title hidden.
fn show_the_title(
    asset_server: Option<Res<AssetServer>>,
    mut titles: Query<(&mut StartupTitle, &mut TextFont, &mut Visibility)>,
) {
    for (mut title, mut font, mut visibility) in &mut titles {
        if *visibility != Visibility::Hidden {
            continue;
        }
        let loaded = match &asset_server {
            Some(asset_server) if title.0 != Handle::default() => {
                if asset_server.load_state(&title.0).is_failed() {
                    warn!("The startup title's font did not load; using the default font");
                    title.0 = Handle::default();
                    font.font = Handle::<Font>::default().into();
                    true
                } else {
                    asset_server.is_loaded_with_dependencies(&title.0)
                }
            }
            _ => true,
        };
        if loaded {
            *visibility = Visibility::Inherited;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn watch_the_steps(
    mut steps: ResMut<StartupSteps>,
    time: Res<Time<Real>>,
    warm_ups: Query<(), With<WarmUp>>,
    lens: Option<Res<crate::game::forward_camera::LensWarmUp>>,
    roots: Query<(), With<StartupRoot>>,
    pictures: Query<&Visibility, With<TitleBackground>>,
    titles: Query<&Visibility, With<StartupTitle>>,
    bakes: Option<Res<BakeProgress>>,
    mut frames_up: Local<u32>,
) {
    let delta = time.delta_secs();
    steps.seconds += delta;
    steps.warm_ups_left = warm_ups.iter().count();
    steps.warm_ups_total = steps.warm_ups_total.max(steps.warm_ups_left);
    steps.lens_warming = lens.is_some();
    if !roots.is_empty() {
        *frames_up += 1;
    }
    // With no picture in any pack there is nothing to wait for once the root has had a frame to
    // ask for one.
    let title_shown = titles.iter().all(|visibility| *visibility != Visibility::Hidden);
    steps.picture_ready = title_shown
        && (pictures.iter().any(|visibility| *visibility != Visibility::Hidden) || (pictures.is_empty() && *frames_up > 2));
    steps.rooms = bakes.as_deref().and_then(|bakes| bakes.get(BakeKind::Rooms)).cloned();
    steps.light = bakes.as_deref().and_then(|bakes| bakes.get(BakeKind::Sunlight)).cloned();
    steps.quick_frames = match delta < QUICK_FRAME_SECONDS {
        true => steps.quick_frames.saturating_add(1),
        false => 0,
    };
}

/// Every frame until shown, since a window hidden by mistake is a process with nothing on screen.
fn show_the_window(
    mut windows: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    steps: Res<StartupSteps>,
    mode: Res<State<AppMode>>,
    mut frames_shown: Local<u32>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let Ok(mut window) = windows.single_mut() else { return };
    if steps.picture_ready {
        *frames_shown += 1;
    }
    if *frames_shown > FRAMES_BEFORE_SHOWING || steps.seconds >= SHOW_BY_SECONDS || *mode.get() != AppMode::Starting {
        window.visible = true;
        *done = true;
        info!("Showing the window {:.2} s into startup", steps.seconds);
    }
}

fn open_the_warm_ups(
    gate: Option<ResMut<WarmUpGate>>,
    steps: Res<StartupSteps>,
    mode: Res<State<AppMode>>,
    mut frames_shown: Local<u32>,
) {
    let Some(mut gate) = gate.filter(|gate| !gate.open) else { return };
    if steps.picture_ready {
        *frames_shown += 1;
    }
    if *frames_shown > FRAMES_BEFORE_WARMING_UP || *mode.get() != AppMode::Starting {
        gate.open = true;
    }
}

fn finish_starting(steps: Res<StartupSteps>, after: Option<Res<AfterStartup>>, mut next: ResMut<NextState<AppMode>>) {
    let after = after.map_or(AppMode::MainMenu, |after| after.0);
    if steps.done() {
        info!("Started in {:.1} s; on to {}", steps.seconds, after.name());
    } else if steps.seconds >= GIVE_UP_SECONDS {
        warn!("Startup still waiting after {GIVE_UP_SECONDS} s ({steps:?}); going on to {} anyway", after.name());
    } else {
        return;
    }
    next.set(after);
}

fn startup_window(mut contexts: EguiContexts, steps: Res<StartupSteps>) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let shown = steps.progress();
    egui::Window::new(get!("startup.title"))
        .id(egui::Id::new("startup"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .fixed_size([WINDOW_WIDTH, 0.0])
        .anchor(egui::Align2::RIGHT_BOTTOM, [-MARGIN, -MARGIN])
        .show(ctx, |ui| {
            ui.heading(get!("startup.title"));
            ui.add_space(8.0);
            ui.add(egui::ProgressBar::new(shown.fraction).show_percentage());
            ui.add(egui::Label::new(&shown.line).truncate());
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::bake_job::BakeState;

    fn status(state: BakeState) -> Option<BakeStatus> {
        Some(BakeStatus { state, ..default() })
    }

    fn finished() -> StartupSteps {
        StartupSteps {
            warm_ups_total: 4,
            picture_ready: true,
            rooms: status(BakeState::UpToDate),
            light: status(BakeState::Skipped),
            quick_frames: QUICK_FRAMES,
            ..default()
        }
    }

    /// Breaks if startup can end with a step still running, or never ends once all are done.
    #[test]
    fn startup_ends_only_once_every_step_is_done() {
        let steps = finished();
        assert!(steps.done());
        assert!((steps.progress().fraction - 1.0).abs() < 1e-6);
        for unfinished in [
            StartupSteps { warm_ups_left: 1, ..finished() },
            StartupSteps { lens_warming: true, ..finished() },
            StartupSteps { picture_ready: false, ..finished() },
            StartupSteps { rooms: status(BakeState::Baking), ..finished() },
            StartupSteps { light: status(BakeState::Waiting), ..finished() },
            StartupSteps { quick_frames: QUICK_FRAMES - 1, ..finished() },
        ] {
            assert!(!unfinished.done(), "{unfinished:?}");
            assert!(unfinished.progress().fraction < 1.0, "{unfinished:?}");
        }
    }

    fn app(after: AppMode) -> App {
        let mut app = App::new();
        app.add_plugins((bevy::state::app::StatesPlugin, bevy::time::TimePlugin))
            .insert_state(AppMode::Starting)
            .insert_resource(AfterStartup(after))
            .add_plugins(StartupScreenPlugin);
        app
    }

    /// With no warm-ups, bakes or pictures in the app, only settling is left, so it hands over.
    #[test]
    fn startup_hands_over_to_where_it_was_told() {
        let mut app = app(AppMode::Onboarding);
        for _ in 0..QUICK_FRAMES + 4 {
            app.update();
        }
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::Onboarding);
        app.update();
        let mut roots = app.world_mut().query_filtered::<(), With<StartupRoot>>();
        assert_eq!(roots.iter(app.world()).count(), 0, "the screen outlived its state");
    }

    /// Breaks if the warm-ups draw before the picture is up, or never start.
    #[test]
    fn the_warm_ups_wait_for_the_picture() {
        let mut app = app(AppMode::MainMenu);
        app.update();
        assert!(!app.world().resource::<WarmUpGate>().open, "the warm-ups started with nothing on screen");
        for _ in 0..FRAMES_BEFORE_WARMING_UP + 4 {
            app.update();
        }
        assert!(app.world().resource::<WarmUpGate>().open);
    }

    #[test]
    fn a_launch_that_does_not_start_here_warms_up_at_once() {
        let mut app = App::new();
        app.add_plugins((bevy::state::app::StatesPlugin, bevy::time::TimePlugin))
            .insert_state(AppMode::Editor)
            .add_plugins(StartupScreenPlugin);
        app.update();
        assert!(crate::game::warm_up::WarmUpGate::default().open);
        assert!(app.world().get_resource::<WarmUpGate>().is_none_or(|gate| gate.open));
    }

    #[test]
    fn startup_gives_up_on_a_step_that_never_finishes() {
        let mut app = app(AppMode::MainMenu);
        app.world_mut().spawn(WarmUp);
        app.update();
        app.world_mut().resource_mut::<StartupSteps>().seconds = GIVE_UP_SECONDS;
        app.update();
        app.update();
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::MainMenu);
    }
}

