//! [`AppMode::Starting`]: the first screen of a bare launch. A title picture, the wordmark and a
//! progress window cover the first frames, which compile pipelines and bake the startup map and
//! would otherwise leave a still window that looks hung. It hands over to [`AfterStartup`] once
//! loading is done and the wordmark has played ([`SHORTEST_SECONDS`]), or after
//! [`GIVE_UP_SECONDS`].
//!
//! The warm-ups wait behind a shut [`WarmUpGate`] until the picture and letters have been drawn
//! for [`FRAMES_BEFORE_WARMING_UP`] frames, since each pipeline compiled in a frame holds that
//! frame back from the window.
//!
//! The letters are PNGs in [`WORDMARK_DIR`], pre-rendered from Nabla because Bevy's text does not
//! paint COLRv1 color fonts. The picture is a [`BackgroundSet::Startup`] pick, which the main menu
//! shows again on its first showing.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::AppMode;
use crate::common::keybind::{self, Action, Actions};
use crate::common::joining::{bake_fraction, settled};
use crate::game::warm_up::{WarmUp, WarmUpGate};
use crate::get;
use crate::title_background::{BackgroundSet, TitleBackground, WantsBackground};
use crate::tool::bake_job::{BakeKind, BakeProgress, BakeStatus};

const BACKGROUND: Color = Color::srgb(0.07, 0.07, 0.08);
/// Shipped with the binary at this path, not looked up across packs.
const WORDMARK_DIR: &str = "default/images/wordmark";

struct Letter {
    file: &'static str,
    /// Logical pixels from the wordmark's top left; the PNGs are drawn at twice this.
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

/// "pikaia.space" in Nabla at 100 pt (133⅓ px) as Chrome lays it out, each letter cropped to its
/// painted pixels.
const LETTERS: [Letter; 12] = [
    Letter { file: "p", x: 0.0, y: 20.0, width: 58.0, height: 118.0 },
    Letter { file: "i", x: 69.33, y: 1.5, width: 58.0, height: 119.0 },
    Letter { file: "k", x: 138.66, y: 0.0, width: 58.0, height: 121.0 },
    Letter { file: "a", x: 207.99, y: 20.0, width: 58.0, height: 101.0 },
    Letter { file: "i", x: 277.33, y: 1.5, width: 58.0, height: 119.0 },
    Letter { file: "a", x: 346.66, y: 20.0, width: 58.0, height: 101.0 },
    Letter { file: "period", x: 415.99, y: 90.5, width: 31.0, height: 31.0 },
    Letter { file: "s", x: 457.59, y: 20.0, width: 58.0, height: 101.0 },
    Letter { file: "p", x: 526.92, y: 20.0, width: 58.0, height: 118.0 },
    Letter { file: "a", x: 596.25, y: 20.0, width: 58.0, height: 101.0 },
    Letter { file: "c", x: 665.58, y: 20.0, width: 58.0, height: 101.0 },
    Letter { file: "e", x: 734.91, y: 20.0, width: 58.0, height: 101.0 },
];
const WORDMARK_SIZE: Vec2 = Vec2::new(792.91, 138.0);
const LETTER_FALL_SECONDS: f32 = 0.5;
const LETTER_SPACING_SECONDS: f32 = 0.2;
const WORDMARK_SECONDS: f32 = (LETTERS.len() - 1) as f32 * LETTER_SPACING_SECONDS + LETTER_FALL_SECONDS;
/// On the wordmark's clock, so the screen outlasts the drop by a second however the frames went.
const SHORTEST_SECONDS: f32 = WORDMARK_SECONDS + 1.0;
/// The most the wordmark's clock moves in a frame, so a frame held back by a pipeline compile
/// slows the drop rather than skipping it.
const WORDMARK_MOST_PER_FRAME: f32 = 1.0 / 20.0;
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
        keybind::ensure(app);
        app.init_resource::<StartupSteps>()
            .add_systems(
                Update,
                (
                    show_or_hide,
                    drop_the_letters.after(watch_the_steps),
                    (watch_the_steps, finish_starting).chain().run_if(in_state(AppMode::Starting)),
                    open_the_warm_ups.after(watch_the_steps),
                ),
            )
            .add_systems(EguiPrimaryContextPass, startup_window.run_if(in_state(AppMode::Starting)));
    }
}

#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct StartupSteps {
    /// The most warm-up entities seen at once, so the share done is measured against it.
    pub warm_ups_total: usize,
    pub warm_ups_left: usize,
    pub lens_warming: bool,
    /// The picture is on screen and every letter of the wordmark has loaded.
    pub picture_ready: bool,
    pub rooms: Option<BakeStatus>,
    pub light: Option<BakeStatus>,
    /// Frames in a row quicker than [`QUICK_FRAME_SECONDS`].
    pub quick_frames: u32,
    /// Real seconds since the screen went up.
    pub seconds: f32,
    /// Seconds of the wordmark's drop played, from when the picture is up.
    pub wordmark_seconds: f32,
    /// [`Action::SkipIntro`] was pressed; the screen ends once loading is done.
    pub skipped: bool,
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
        self.loaded() && (self.skipped || self.wordmark_seconds >= SHORTEST_SECONDS)
    }

    pub fn loaded(&self) -> bool {
        self.warmed_up()
            && self.picture_ready
            && settled(self.rooms.as_ref())
            && settled(self.light.as_ref())
            && self.quick_frames >= QUICK_FRAMES
    }

    pub fn progress(&self) -> StartupProgress {
        if self.loaded() {
            return StartupProgress { fraction: 1.0, line: get!("startup.done") };
        }
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

/// An index into [`LETTERS`].
#[derive(Component)]
struct WordmarkLetter {
    index: usize,
    image: Handle<Image>,
}

/// easeOutBounce: 0 at 0 and 1 at 1, never past 1, landing with three bounces.
pub fn ease_out_bounce(t: f32) -> f32 {
    const N: f32 = 7.5625;
    const D: f32 = 2.75;
    let t = t.clamp(0.0, 1.0);
    if t < 1.0 / D {
        N * t * t
    } else if t < 2.0 / D {
        let t = t - 1.5 / D;
        N * t * t + 0.75
    } else if t < 2.5 / D {
        let t = t - 2.25 / D;
        N * t * t + 0.9375
    } else {
        let t = t - 2.625 / D;
        N * t * t + 0.984375
    }
}

/// easeOutElastic: 0 at 0 and 1 at 1, swinging past 1 and back as it settles.
pub fn ease_out_elastic(t: f32) -> f32 {
    const C4: f32 = 2.0 * std::f32::consts::PI / 3.0;
    match t {
        t if t <= 0.0 => 0.0,
        t if t >= 1.0 => 1.0,
        t => 2f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * C4).sin() + 1.0,
    }
}

const LETTER_EASE: fn(f32) -> f32 = ease_out_elastic;

/// How far letter `index` has fallen, 0 to 1, `seconds` into the drop.
fn fallen(index: usize, seconds: f32) -> f32 {
    LETTER_EASE((seconds - index as f32 * LETTER_SPACING_SECONDS) / LETTER_FALL_SECONDS)
}

/// Under the egui window, on the egui context's camera, as the joining screen's root is.
fn show_or_hide(
    mut commands: Commands,
    mode: Res<State<AppMode>>,
    existing: Query<(Entity, Has<UiTargetCamera>), With<StartupRoot>>,
    camera: Query<Entity, With<bevy_egui::PrimaryEguiContext>>,
    asset_server: Option<Res<AssetServer>>,
) {
    let wanted = *mode.get() == AppMode::Starting;
    let camera = camera.single().ok();
    if wanted && existing.is_empty() {
        let mut root = commands.spawn((
            StartupRoot,
            WantsBackground(BackgroundSet::Startup),
            GlobalZIndex(10),
            Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
            BackgroundColor(BACKGROUND),
        ));
        // In order, so a later letter is drawn over an earlier one, as the font lays them.
        root.with_children(|root| {
            for (index, letter) in LETTERS.iter().enumerate() {
                let image = asset_server
                    .as_ref()
                    .map_or_else(Handle::default, |assets| assets.load(format!("{WORDMARK_DIR}/{}.png", letter.file)));
                root.spawn((
                    WordmarkLetter { index, image: image.clone() },
                    ImageNode::new(image),
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(-letter.height),
                        width: Val::Px(letter.width),
                        height: Val::Px(letter.height),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
            }
        });
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

/// Each letter at its own x, from just above the window to its place in the centered wordmark.
fn drop_the_letters(
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    ui_scale: Option<Res<UiScale>>,
    steps: Res<StartupSteps>,
    mut letters: Query<(&WordmarkLetter, &mut Node)>,
) {
    let Ok(window) = window.single() else { return };
    // `Val::Px` is in logical pixels divided by `UiScale`.
    let window = Vec2::new(window.width(), window.height()) / ui_scale.map_or(1.0, |scale| scale.0);
    let corner = (window - WORDMARK_SIZE) / 2.0;
    for (letter, mut node) in &mut letters {
        let place = &LETTERS[letter.index];
        let top = -place.height + (corner.y + place.y + place.height) * fallen(letter.index, steps.wordmark_seconds);
        let wanted = (Val::Px(corner.x + place.x), Val::Px(top));
        if (node.left, node.top) != wanted {
            (node.left, node.top) = wanted;
        }
    }
}

/// A letter that will not load is passed over rather than holding the window hidden.
fn letters_loaded(asset_server: Option<&AssetServer>, letters: &Query<&WordmarkLetter>) -> bool {
    let Some(asset_server) = asset_server else { return true };
    letters.iter().all(|letter| {
        letter.image == Handle::default()
            || asset_server.is_loaded_with_dependencies(&letter.image)
            || asset_server.load_state(&letter.image).is_failed()
    })
}

#[allow(clippy::too_many_arguments)]
fn watch_the_steps(
    mut steps: ResMut<StartupSteps>,
    time: Res<Time<Real>>,
    warm_ups: Query<(), With<WarmUp>>,
    lens: Option<Res<crate::game::forward_camera::LensWarmUp>>,
    roots: Query<(), With<StartupRoot>>,
    pictures: Query<(&Visibility, &ImageNode), With<TitleBackground>>,
    letters: Query<&WordmarkLetter>,
    asset_server: Option<Res<AssetServer>>,
    actions: Res<Actions>,
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
    // ask for one, and one that will not load is passed over, as a letter is.
    let failed = |node: &ImageNode| asset_server.as_ref().is_some_and(|assets| assets.load_state(&node.image).is_failed());
    let picture_up = pictures.iter().any(|(visibility, node)| *visibility != Visibility::Hidden || failed(node));
    steps.picture_ready =
        letters_loaded(asset_server.as_deref(), &letters) && (picture_up || (pictures.is_empty() && *frames_up > 2));
    if steps.picture_ready {
        steps.wordmark_seconds += delta.min(WORDMARK_MOST_PER_FRAME);
    }
    if actions.just_pressed(Action::SkipIntro) {
        steps.skipped = true;
    }
    steps.rooms = bakes.as_deref().and_then(|bakes| bakes.get(BakeKind::Rooms)).cloned();
    steps.light = bakes.as_deref().and_then(|bakes| bakes.get(BakeKind::Sunlight)).cloned();
    steps.quick_frames = match delta < QUICK_FRAME_SECONDS {
        true => steps.quick_frames.saturating_add(1),
        false => 0,
    };
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
            wordmark_seconds: SHORTEST_SECONDS,
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

    /// Breaks if loading done ends the screen before the wordmark has played and a second more,
    /// or if a click before then does not end it.
    #[test]
    fn the_wordmark_plays_out_unless_a_click_skips_it() {
        let playing = StartupSteps { wordmark_seconds: SHORTEST_SECONDS - 0.1, ..finished() };
        assert!(playing.loaded() && !playing.done());
        assert_eq!(playing.progress().line, get!("startup.done"));
        assert!(StartupSteps { skipped: true, ..playing.clone() }.done());
        let loading = StartupSteps { skipped: true, rooms: status(BakeState::Baking), ..playing };
        assert!(!loading.done(), "a click skipped loading as well as the wordmark");
        assert!((WORDMARK_SECONDS - 2.7).abs() < 1e-5);
    }

    #[test]
    fn a_letter_falls_from_above_and_lands_after_its_turn() {
        assert_eq!(ease_out_bounce(0.0), 0.0);
        assert!((ease_out_bounce(1.0) - 1.0).abs() < 1e-6);
        for step in 0..=100 {
            let eased = ease_out_bounce(step as f32 / 100.0);
            assert!((0.0..=1.0 + 1e-6).contains(&eased), "{eased}");
        }
        assert_eq!(ease_out_elastic(0.0), 0.0);
        assert_eq!(ease_out_elastic(1.0), 1.0);
        assert!((0..100).any(|step| ease_out_elastic(step as f32 / 100.0) > 1.0), "elastic never swung past");
        assert_eq!(fallen(3, 3.0 * LETTER_SPACING_SECONDS), 0.0, "a letter started before its turn");
        assert!((fallen(3, 3.0 * LETTER_SPACING_SECONDS + LETTER_FALL_SECONDS) - 1.0).abs() < 1e-6);
        let last = LETTERS.len() - 1;
        assert!((fallen(last, WORDMARK_SECONDS) - 1.0).abs() < 1e-6, "the last letter is still falling");
    }

    /// With no warm-ups, bakes or pictures in the app, only settling and the wordmark are left.
    #[test]
    fn startup_hands_over_to_where_it_was_told() {
        let mut app = app(AppMode::Onboarding);
        for _ in 0..QUICK_FRAMES + 4 {
            app.update();
        }
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::Starting, "it left before the wordmark played");
        app.world_mut().resource_mut::<StartupSteps>().wordmark_seconds = SHORTEST_SECONDS;
        app.update();
        app.update();
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::Onboarding);
        app.update();
        let mut roots = app.world_mut().query_filtered::<(), With<StartupRoot>>();
        assert_eq!(roots.iter(app.world()).count(), 0, "the screen outlived its state");
    }

    #[test]
    fn a_click_skips_to_the_menu_once_loaded() {
        let mut app = app(AppMode::MainMenu);
        app.add_plugins(bevy::input::InputPlugin);
        for _ in 0..QUICK_FRAMES + 4 {
            app.update();
        }
        app.world_mut().write_message(bevy::input::mouse::MouseButtonInput {
            button: MouseButton::Right,
            state: bevy::input::ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        app.update();
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::MainMenu);
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


