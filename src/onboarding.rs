//! The first screen for somebody with no name saved: a name and a field of view, then on to the
//! binary's home mode (the map editor or the main menu) with the demo scene asked for. Started as the
//! initial [`AppMode`] rather than switched to, so the home mode's `OnEnter` never runs before it.

use bevy::prelude::*;
use bevy_egui::egui::{self, Id, LayerId, Ui, UiBuilder};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};

use crate::common::app_mode::{AppMode, Home};
use crate::common::file_menu::{FileAction, DEMO_SCENE};
use crate::common::player_settings::{self, PlayerSettings, NAME_LIMIT};
use crate::editor::panels::CurrentFilePath;
use crate::get;

const WIDTH: f32 = 320.0;

/// Add after `PlayerSettingsPlugin`: its saved name decides whether the process starts here.
pub struct OnboardingPlugin {
    /// The command line chose where to start, so the process starts in the map editor, where the
    /// start flags take it from.
    pub skip: bool,
    /// Where a bare launch starts, and where Continue goes; inserted as [`Home`] for everybody else.
    pub home: AppMode,
}

impl Plugin for OnboardingPlugin {
    fn build(&self, app: &mut App) {
        let unnamed = app.world().get_resource::<PlayerSettings>().is_some_and(needs_onboarding);
        if !self.skip {
            match unnamed {
                true => app.insert_state(AppMode::Onboarding),
                false if self.home != AppMode::default() => app.insert_state(self.home),
                false => app,
            };
        }
        app.insert_resource(Home(self.home))
            .init_resource::<Draft>()
            .add_systems(Update, follow_the_settings.run_if(in_state(AppMode::Onboarding)))
            .add_systems(EguiPrimaryContextPass, screen.run_if(in_state(AppMode::Onboarding)));
    }
}

pub fn needs_onboarding(settings: &PlayerSettings) -> bool {
    is_blank(&settings.name)
}

/// True for a name that [`PlayerSettings::cleaned`] would leave empty.
fn is_blank(name: &str) -> bool {
    !name.chars().any(|c| !c.is_whitespace() && !c.is_control())
}

/// Continue writes only these two fields, so changes applied in the Settings window meanwhile stay.
#[derive(Resource, Default)]
struct Draft {
    name: String,
    field_of_view: u32,
}

impl Draft {
    fn of(settings: &PlayerSettings) -> Self {
        Self { name: settings.name.clone(), field_of_view: settings.field_of_view }
    }
}

/// A system's first run sees every resource as changed, so this also seeds the draft on entry.
fn follow_the_settings(mut draft: ResMut<Draft>, settings: Option<Res<PlayerSettings>>) {
    if let Some(settings) = settings.filter(|settings| settings.is_changed()) {
        *draft = Draft::of(&settings);
    }
}

fn finish(draft: &Draft, home: AppMode, settings: Option<&mut PlayerSettings>, current_file: &mut CurrentFilePath, next: &mut NextState<AppMode>) {
    if let Some(settings) = settings {
        let chosen = PlayerSettings { name: draft.name.clone(), field_of_view: draft.field_of_view, ..settings.clone() };
        *settings = chosen.cleaned();
    }
    current_file.request(FileAction::Demo(DEMO_SCENE));
    next.set(home);
}

fn screen(
    mut contexts: EguiContexts,
    mut draft: ResMut<Draft>,
    home: Res<Home>,
    settings: Option<ResMut<PlayerSettings>>,
    mut current_file: ResMut<CurrentFilePath>,
    mut next: ResMut<NextState<AppMode>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();
    let mut window_ui = Ui::new(
        ctx.clone(),
        Id::new("grackle_onboarding_window"),
        UiBuilder::new().layer_id(LayerId::background()).max_rect(ctx.viewport_rect()),
    );

    let mut go = false;
    egui::CentralPanel::default().show(&mut window_ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.25).max(24.0));
            ui.label(egui::RichText::new(get!("onboarding.title")).size(72.0).strong());
            ui.add_space(40.0);

            ui.label(get!("onboarding.name"));
            let name = ui.add(
                egui::TextEdit::singleline(&mut draft.name).char_limit(NAME_LIMIT).desired_width(WIDTH),
            );
            let named = !is_blank(&draft.name);
            go |= named && name.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            ui.add_space(16.0);

            ui.label(get!("onboarding.field_of_view"));
            // `vertical_centered` puts a slider's track-and-number pair at the left; a fixed-width
            // row is centered.
            let row = egui::vec2(WIDTH, ui.spacing().interact_size.y);
            ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let number = ui.spacing().interact_size.x + ui.spacing().item_spacing.x;
                ui.spacing_mut().slider_width = WIDTH - number;
                ui.add(egui::Slider::new(&mut draft.field_of_view, player_settings::FIELD_OF_VIEW_RANGE).suffix("\u{b0}"));
            });
            ui.add_space(32.0);

            let button = egui::Button::new(get!("onboarding.continue")).min_size(egui::vec2(WIDTH * 0.5, 32.0));
            go |= ui.add_enabled(named, button).clicked();
        });
    });

    if go {
        finish(&draft, home.0, settings.map(ResMut::into_inner), &mut current_file, &mut next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_or_whitespace_name_needs_onboarding() {
        assert!(needs_onboarding(&PlayerSettings::default()));
        assert!(needs_onboarding(&PlayerSettings { name: "  \t".into(), ..default() }));
        assert!(!needs_onboarding(&PlayerSettings { name: "Ada".into(), ..default() }));
    }

    fn app(settings: PlayerSettings, skip: bool) -> App {
        app_with_home(settings, skip, AppMode::Editor)
    }

    fn app_with_home(settings: PlayerSettings, skip: bool, home: AppMode) -> App {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin).insert_resource(settings);
        app.add_plugins(OnboardingPlugin { skip, home });
        app.init_state::<AppMode>();
        app.update();
        app
    }

    #[test]
    fn continue_keeps_every_other_setting_and_opens_the_demo() {
        let mut settings = PlayerSettings { auto_reload: false, ..default() };
        let draft = Draft { name: "  Ada ".into(), field_of_view: 200 };
        let (mut current_file, mut next) = (CurrentFilePath::default(), NextState::<AppMode>::default());
        finish(&draft, AppMode::Editor, Some(&mut settings), &mut current_file, &mut next);
        assert_eq!(settings.name, "Ada");
        assert_eq!(settings.field_of_view, *player_settings::FIELD_OF_VIEW_RANGE.end());
        assert!(!settings.auto_reload, "a setting the screen does not ask for was overwritten");
        assert_eq!(current_file.requested(), Some(FileAction::Demo(DEMO_SCENE)));
        assert!(matches!(next, NextState::Pending(AppMode::Editor)));
    }

    #[test]
    fn the_screen_shows_a_name_applied_from_the_settings_window() {
        let mut app = app(PlayerSettings::default(), false);
        app.world_mut().resource_mut::<PlayerSettings>().name = "Ada".into();
        app.update();
        assert_eq!(app.world().resource::<Draft>().name, "Ada");
    }

    #[test]
    fn only_somebody_unnamed_starts_in_onboarding() {
        let mode = |app: &App| *app.world().resource::<State<AppMode>>().get();
        assert_eq!(mode(&app(PlayerSettings::default(), false)), AppMode::Onboarding);
        assert_eq!(mode(&app(PlayerSettings { name: "Ada".into(), ..default() }, false)), AppMode::Editor);
        assert_eq!(mode(&app(PlayerSettings::default(), true)), AppMode::Editor);
    }

    /// Fails if the game binary starts anywhere but its main menu on a bare launch, or keeps a
    /// new player from naming themself first.
    #[test]
    fn a_bare_launch_of_the_game_starts_in_the_main_menu() {
        let mode = |app: &App| *app.world().resource::<State<AppMode>>().get();
        let named = PlayerSettings { name: "Ada".into(), ..default() };
        assert_eq!(mode(&app_with_home(named.clone(), false, AppMode::MainMenu)), AppMode::MainMenu);
        assert_eq!(mode(&app_with_home(PlayerSettings::default(), false, AppMode::MainMenu)), AppMode::Onboarding);
        assert_eq!(mode(&app_with_home(named, true, AppMode::MainMenu)), AppMode::Editor);

        let mut next = NextState::<AppMode>::default();
        let draft = Draft { name: "Ada".into(), field_of_view: 90 };
        finish(&draft, AppMode::MainMenu, None, &mut CurrentFilePath::default(), &mut next);
        assert!(matches!(next, NextState::Pending(AppMode::MainMenu)));
    }
}
