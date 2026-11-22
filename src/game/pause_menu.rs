//! The pause menu: `Escape` while playing releases the mouse and offers Resume,
//! Settings, Change Teams, and a way out of the round. Plain Bevy UI, not egui, so it exists
//! in a build with no editor.

use bevy::prelude::*;

use crate::common::app_mode::{AppMode, CameFrom, JoinedFrom};
use crate::common::fonts::DisplayFont;
use crate::common::net::{NetRequest, NetRole};
use crate::common::settings::{OpenSettings, SettingsWindow};
use crate::common::swatch::Swatch;
use crate::game::menu::{menu_button, menu_panel, menu_title, ITEM_SIZE};
use crate::game::plate::plate_text;
use crate::game::plate_button::ButtonActivated;
use crate::game::team_choice::TeamOptions;
use crate::get;

/// Whether the pause menu is showing.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PauseMenu {
    pub open: bool,
}

/// Which menus have the mouse. Each writes only its own flag, so neither
/// rebuilds because the other changed.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MouseHeld {
    pub pause: bool,
    /// Written only by [`crate::game::team_picker`].
    pub team_picker: bool,
    /// Written only by [`crate::game::class_picker`].
    pub class_picker: bool,
    /// The Settings window, which the pause menu opens in Play.
    pub settings: bool,
}

impl MouseHeld {
    pub fn any(&self) -> bool {
        self.pause || self.team_picker || self.class_picker || self.settings
    }
}

/// Set by an overlay that took this frame's Escape, in [`TakeEscape`]; the pause menu opens only
/// when none did. `toggle_pause` clears it.
#[derive(Resource, Debug, Default)]
pub struct EscapeTaken(pub bool);

/// Systems that may take Escape before the pause menu reads it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TakeEscape;

/// Insert to bring up the team picker, which takes it down again.
#[derive(Resource, Debug, Default)]
pub struct ChangeTeamsAsked;

#[derive(Component)]
struct PauseMenuRoot;

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum PauseItem {
    /// Back to [`CameFrom::after_a_round`] on the authority, which ends the round for everybody;
    /// a client disconnects and goes back to [`JoinedFrom`].
    Leave,
    Settings,
    ChangeTeams,
    Resume,
}

const PAUSE_MENU_LAYER: i32 = 20;

/// Run condition. `true` when the resource is missing, so the game layer runs
/// in test apps without this plugin.
pub fn not_paused(held: Option<Res<MouseHeld>>) -> bool {
    held.is_none_or(|held| !held.any())
}

pub struct PauseMenuPlugin;

impl Plugin for PauseMenuPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        app.init_resource::<PauseMenu>()
            .init_resource::<MouseHeld>()
            .init_resource::<EscapeTaken>()
            .init_resource::<DisplayFont>()
            .add_message::<NetRequest>()
            .add_observer(choose)
            .add_systems(
                Update,
                (
                    toggle_pause.after(TakeEscape).run_if(in_state(AppMode::Play)),
                    // Chained so the menu appears the same frame. Ungated so
                    // they run on the frame leaving Play closes the menu.
                    show_or_hide.run_if(resource_changed::<PauseMenu>.or_else(resource_exists_and_changed::<NetRole>)),
                    lend_the_mouse,
                    release_held_keys.run_if(resource_changed::<MouseHeld>),
                    grab_the_cursor,
                )
                    .chain(),
            )
            .add_systems(OnExit(AppMode::Play), close_on_leaving_play);
    }
}

fn toggle_pause(actions: Res<crate::common::keybind::Actions>, mut menu: ResMut<PauseMenu>, mut taken: ResMut<EscapeTaken>) {
    let taken = std::mem::take(&mut taken.0);
    if actions.just_pressed(crate::common::keybind::Action::Pause) && !taken {
        menu.open = !menu.open;
    }
}

fn show_or_hide(
    mut commands: Commands,
    menu: Res<PauseMenu>,
    existing: Query<Entity, With<PauseMenuRoot>>,
    font: Res<DisplayFont>,
    role: Option<Res<NetRole>>,
    offer: Option<Res<TeamOptions>>,
    came_from: Option<Res<CameFrom>>,
) {
    for root in &existing {
        commands.entity(root).despawn();
    }
    if !menu.open {
        return;
    }

    let came_from = came_from.as_deref().copied().unwrap_or_default();
    let leave = match role.as_deref().is_none_or(NetRole::is_authority) {
        false => get!("pause.disconnect"),
        true => get!("pause.return_to", "mode", came_from.after_a_round(role.as_deref()).name()),
    };
    // Without teams, somebody already playing changes rather than joins.
    let change_teams = match offer.as_deref().is_none_or(|offer| offer.teams || offer.seat.is_playing()) {
        true => get!("pause.change_teams"),
        false => get!("pause.join_or_spectate"),
    };
    let items = [
        (PauseItem::Resume, get!("pause.resume")),
        (PauseItem::Settings, get!("pause.settings")),
        (PauseItem::ChangeTeams, change_teams),
        (PauseItem::Leave, leave),
    ];
    let font = &font.0;
    commands
        .spawn((
            PauseMenuRoot,
            // Over the team picker, which may be up underneath.
            GlobalZIndex(PAUSE_MENU_LAYER),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            // Dimmed, not opaque: the match keeps running behind it.
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        ))
        .with_children(|root| {
            root.spawn(menu_panel()).with_children(|menu| {
                menu.spawn(menu_title(font, get!("pause.title")));
                for (item, label) in items {
                    menu.spawn((item, menu_button(Swatch::neutral()))).with_child(plate_text(font, label, ITEM_SIZE));
                }
            });
        });
}

fn choose(
    activated: On<ButtonActivated>,
    items: Query<&PauseItem>,
    mut commands: Commands,
    mut menu: ResMut<PauseMenu>,
    role: Option<Res<NetRole>>,
    came_from: Option<Res<CameFrom>>,
    joined: Option<Res<JoinedFrom>>,
    mut next: ResMut<NextState<AppMode>>,
    mut requests: MessageWriter<NetRequest>,
) {
    let Ok(item) = items.get(activated.entity) else { return };
    match item {
        PauseItem::Resume => menu.open = false,
        PauseItem::Settings => {
            menu.open = false;
            commands.insert_resource(OpenSettings);
        }
        PauseItem::ChangeTeams => {
            menu.open = false;
            commands.insert_resource(ChangeTeamsAsked);
        }
        // Leaving Play closes the menu. A client leaves Play in the same frame,
        // before the role changes, so it is never the authority in a round.
        // A client's role is solo by the time it would be bounced back out of a home not offered
        // networked, since `back_to_the_map_editor_when_networked` runs after the request is applied.
        PauseItem::Leave => {
            let came_from = came_from.as_deref().copied().unwrap_or_default();
            match role.as_deref().is_none_or(NetRole::is_authority) {
                true => next.set(came_from.after_a_round(role.as_deref())),
                false => {
                    requests.write(NetRequest::GoSolo);
                    next.set(joined.as_deref().copied().unwrap_or_default().after_leaving());
                }
            }
        }
    }
}

/// The Settings window counts from the moment it is asked for, since it opens a pass later, and
/// only in Play, so it pauses nothing in the editors.
fn lend_the_mouse(
    menu: Res<PauseMenu>,
    window: Option<Res<SettingsWindow>>,
    asked: Option<Res<OpenSettings>>,
    mode: Res<State<AppMode>>,
    mut held: ResMut<MouseHeld>,
) {
    let settings = *mode.get() == AppMode::Play && (asked.is_some() || window.is_some_and(|window| window.is_open()));
    if held.pause != menu.open || held.settings != settings {
        held.pause = menu.open;
        held.settings = settings;
    }
}

/// `gather_input` stops while a menu is up, so held keys would stay held. Aim is kept.
fn release_held_keys(held: Res<MouseHeld>, latch: Option<ResMut<crate::game::player::InputLatch>>) {
    if held.any()
        && let Some(mut latch) = latch
    {
        latch.0.release_controls();
    }
}

/// The one writer of the cursor while playing: grabbed only while we have a
/// body and no menu is up, so a spectator or somebody waiting to respawn has it free.
fn grab_the_cursor(
    mut commands: Commands,
    held: Res<MouseHeld>,
    ours: Query<(), With<crate::game::player::LocalPlayer>>,
    window: Query<Entity, With<bevy::window::PrimaryWindow>>,
    mode: Res<State<AppMode>>,
    mut grabbed: Local<Option<bool>>,
) {
    if *mode.get() != AppMode::Play {
        *grabbed = None;
        return;
    }
    let wanted = !held.any() && !ours.is_empty();
    if *grabbed != Some(wanted) {
        *grabbed = Some(wanted);
        crate::game::set_cursor_captured(&mut commands, &window, wanted);
    }
}

/// Left open, the overlay would cover the editor with nothing to close it,
/// since `toggle_pause` only runs in Play.
fn close_on_leaving_play(mut menu: ResMut<PauseMenu>) {
    menu.open = false;
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::input::InputPlugin);
        app.init_state::<AppMode>();
        app.add_plugins(PauseMenuPlugin);
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        app
    }

    /// Runs `Update` directly, since `InputPlugin` clears `just_pressed` in
    /// `PreUpdate` of a full `app.update()`.
    fn press_escape(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        crate::common::keybind::resolve_now(app.world_mut());
        app.world_mut().run_schedule(Update);
        app.world_mut().flush();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::Escape);
    }

    fn showing(app: &mut App) -> bool {
        app.world_mut()
            .query_filtered::<Entity, With<PauseMenuRoot>>()
            .iter(app.world())
            .next()
            .is_some()
    }

    #[test]
    fn escape_opens_it_and_escape_closes_it() {
        let mut app = app();
        assert!(!showing(&mut app), "the menu was up before anybody asked");

        press_escape(&mut app);
        assert!(showing(&mut app), "escape did not open the menu");

        press_escape(&mut app);
        assert!(!showing(&mut app), "escape did not close the menu again");
    }

    /// Breaks if pausing clears aim along with the held keys.
    #[test]
    fn pausing_releases_the_controls_but_not_the_view() {
        use crate::game::player::{InputLatch, PlayerInput};

        let mut app = app();
        app.insert_resource(InputLatch(PlayerInput {
            yaw: 1.3,
            pitch: -0.4,
            movement: Vec2::new(0.0, 1.0),
            jump: true,
            sprint: true,
            ..default()
        }));

        press_escape(&mut app);

        let latch = app.world().resource::<InputLatch>().0;
        assert_eq!(latch.yaw, 1.3, "the pause menu turned the player round");
        assert_eq!(latch.pitch, -0.4, "the pause menu changed where they were looking");
        assert_eq!(latch.movement, Vec2::ZERO, "a held key survived the pause");
        assert!(!latch.jump && !latch.sprint, "a held key survived the pause");
    }

    #[test]
    fn leaving_play_takes_the_menu_with_it() {
        let mut app = app();
        press_escape(&mut app);
        assert!(showing(&mut app));

        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Editor);
        app.update();

        assert!(!app.world().resource::<PauseMenu>().open);
        assert!(!showing(&mut app), "the menu outlived the match");
    }

    #[test]
    fn escape_in_the_editor_does_nothing() {
        let mut app = app();
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Editor);
        app.update();

        press_escape(&mut app);
        assert!(!showing(&mut app), "the editor grew a pause menu");
    }

    /// Breaks headless tests' game layer if this returns false.
    #[test]
    fn a_game_with_no_menu_is_not_paused() {
        let mut world = World::new();
        assert!(world.run_system_once(not_paused).unwrap());
    }

    fn activate(app: &mut App, wanted: PauseItem) {
        let item = app
            .world_mut()
            .query::<(Entity, &PauseItem)>()
            .iter(app.world())
            .find(|(_, item)| **item == wanted)
            .map(|(entity, _)| entity)
            .expect("the menu does not offer it");
        app.world_mut().trigger(ButtonActivated { entity: item });
        app.world_mut().flush();
    }

    fn open_menu(role: NetRole) -> App {
        let mut app = app();
        app.insert_resource(role);
        press_escape(&mut app);
        app
    }

    fn labels(app: &mut App) -> Vec<String> {
        app.world_mut().query::<&Text>().iter(app.world()).map(|text| text.0.clone()).collect()
    }

    #[test]
    fn resume_closes_the_menu() {
        let mut app = open_menu(NetRole::Solo);
        activate(&mut app, PauseItem::Resume);
        app.update();
        assert!(!showing(&mut app), "Resume left the menu up");
        assert!(!app.world().resource::<MouseHeld>().any(), "Resume kept the mouse");
    }

    #[test]
    fn settings_closes_the_menu_and_asks_for_the_window() {
        let mut app = open_menu(NetRole::Solo);
        activate(&mut app, PauseItem::Settings);
        assert!(app.world().contains_resource::<OpenSettings>(), "the Settings window was not asked for");
        app.update();
        assert!(!showing(&mut app), "Settings left the menu up");
        assert!(app.world().resource::<MouseHeld>().settings, "the mouse was taken back before the window opened");
    }

    #[test]
    fn change_teams_closes_the_menu_and_asks_for_the_picker() {
        let mut app = open_menu(NetRole::Solo);
        activate(&mut app, PauseItem::ChangeTeams);
        assert!(app.world().contains_resource::<ChangeTeamsAsked>(), "the team picker was not asked for");
        app.update();
        assert!(!showing(&mut app), "Change Teams left the menu up");
    }

    /// Breaks if a mode without teams offers a join to somebody already playing, or a change to
    /// somebody not yet in it.
    #[test]
    fn change_teams_is_named_for_the_mode_and_the_seat() {
        let mut choosing = app();
        choosing.insert_resource(TeamOptions { teams: false, ..default() });
        press_escape(&mut choosing);
        let shown = labels(&mut choosing);
        assert!(shown.contains(&get!("pause.join_or_spectate")) && !shown.contains(&get!("pause.change_teams")), "{shown:?}");

        let mut playing = app();
        playing.insert_resource(TeamOptions { teams: false, seat: crate::game::respawn::Seat::Playing(None), ..default() });
        press_escape(&mut playing);
        let shown = labels(&mut playing);
        assert!(shown.contains(&get!("pause.change_teams")) && !shown.contains(&get!("pause.join_or_spectate")), "{shown:?}");
    }

    /// Breaks if an overlay that took Escape does not keep the menu shut.
    #[test]
    fn escape_taken_by_an_overlay_does_not_open_the_menu() {
        let mut app = app();
        app.insert_resource(EscapeTaken(true));
        press_escape(&mut app);
        assert!(!showing(&mut app), "the menu opened under an overlay that took Escape");
        assert!(!app.world().resource::<EscapeTaken>().0, "the claim outlived its frame");
    }

    /// Breaks if the authority's way out of the menu does not end the round.
    #[test]
    fn the_host_goes_back_to_the_editor() {
        let mut app = open_menu(NetRole::Listen { port: 27100 });
        assert!(labels(&mut app).contains(&get!("pause.return_to", "mode", AppMode::Editor.name())));
        activate(&mut app, PauseItem::Leave);
        app.update();
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::Editor);
        assert!(!showing(&mut app), "the menu outlived the round");
    }

    /// Breaks if a solo round goes anywhere but the mode it was started from, whatever mode the
    /// binary opened onto.
    #[test]
    fn a_solo_game_goes_back_where_it_came_from() {
        let mut app = app();
        app.insert_resource(CameFrom(AppMode::Prop));
        press_escape(&mut app);
        assert!(labels(&mut app).contains(&get!("pause.return_to", "mode", AppMode::Prop.name())));
        activate(&mut app, PauseItem::Leave);
        app.update();
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::Prop);
    }

    /// Breaks if a client's way out does not disconnect and leave Play.
    #[test]
    fn a_client_disconnects() {
        let mut app = open_menu(NetRole::Client { host: "elsewhere".into(), port: 27100 });
        assert!(labels(&mut app).contains(&get!("pause.disconnect")));
        activate(&mut app, PauseItem::Leave);
        let asked = app.world().resource::<Messages<NetRequest>>().iter_current_update_messages().cloned().collect::<Vec<_>>();
        assert!(matches!(asked.as_slice(), [NetRequest::GoSolo]), "{asked:?}");
        app.update();
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::Editor, "a client stayed in a round nobody runs");
    }

    /// Breaks if a client that joined from the main menu is sent to the map editor it waited in.
    #[test]
    fn a_client_goes_back_where_it_joined_from() {
        let mut app = open_menu(NetRole::Client { host: "elsewhere".into(), port: 27100 });
        app.insert_resource(JoinedFrom(AppMode::MainMenu));
        activate(&mut app, PauseItem::Leave);
        app.update();
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::MainMenu);
    }

    /// Breaks if the cursor is grabbed with no body of ours, or left free with one and no menu up.
    #[test]
    fn the_cursor_is_grabbed_only_while_we_have_a_body() {
        use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

        let mut app = app();
        let window = app.world_mut().spawn((Window::default(), PrimaryWindow)).id();
        let grab = |app: &App| app.world().get::<CursorOptions>(window).map(|options| options.grab_mode);

        app.update();
        assert_eq!(grab(&app), Some(CursorGrabMode::None), "grabbed while spectating");

        let body = app.world_mut().spawn(crate::game::player::LocalPlayer).id();
        app.update();
        assert_eq!(grab(&app), Some(CursorGrabMode::Locked), "free with a body to steer");

        press_escape(&mut app);
        app.update();
        assert_eq!(grab(&app), Some(CursorGrabMode::None), "grabbed under the menu");

        press_escape(&mut app);
        app.world_mut().entity_mut(body).despawn();
        app.update();
        assert_eq!(grab(&app), Some(CursorGrabMode::None), "grabbed while waiting to come back");
    }
}
