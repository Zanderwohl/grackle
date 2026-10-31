//! The pause menu: `Escape` while playing releases the mouse. Plain Bevy UI,
//! not egui, so it exists in a build with no editor.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::fonts::DisplayFont;
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
}

impl MouseHeld {
    pub fn any(&self) -> bool {
        self.pause || self.team_picker
    }
}

#[derive(Component)]
struct PauseMenuRoot;

const PAUSE_MENU_LAYER: i32 = 20;

/// Run condition. `true` when the resource is missing, so the game layer runs
/// in test apps without this plugin.
pub fn not_paused(held: Option<Res<MouseHeld>>) -> bool {
    held.is_none_or(|held| !held.any())
}

pub struct PauseMenuPlugin;

impl Plugin for PauseMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PauseMenu>()
            .init_resource::<MouseHeld>()
            .init_resource::<DisplayFont>()
            .add_systems(
                Update,
                (
                    toggle_pause.run_if(in_state(AppMode::Play)),
                    // Chained so the menu appears the same frame. Ungated so
                    // they run on the frame leaving Play closes the menu.
                    show_or_hide.run_if(resource_changed::<PauseMenu>),
                    lend_the_mouse,
                    capture_or_release.run_if(resource_changed::<MouseHeld>),
                )
                    .chain(),
            )
            .add_systems(OnExit(AppMode::Play), close_on_leaving_play);
    }
}

fn toggle_pause(keys: Res<ButtonInput<KeyCode>>, mut menu: ResMut<PauseMenu>) {
    if keys.just_pressed(KeyCode::Escape) {
        menu.open = !menu.open;
    }
}

fn show_or_hide(
    mut commands: Commands,
    menu: Res<PauseMenu>,
    existing: Query<Entity, With<PauseMenuRoot>>,
    font: Res<DisplayFont>,
) {
    for root in &existing {
        commands.entity(root).despawn();
    }

    if menu.open {
        commands.spawn((
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
            children![(
                Text::new(get!("pause.title")),
                TextFont {
                    font: (&font.0).into(),
                    font_size: bevy::text::FontSize::Px(42.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            )],
        ));
    }
}

fn lend_the_mouse(menu: Res<PauseMenu>, mut held: ResMut<MouseHeld>) {
    if held.pause != menu.open {
        held.pause = menu.open;
    }
}

/// The one writer of the cursor while playing, so it cannot disagree with
/// whichever menu is up.
fn capture_or_release(
    mut commands: Commands,
    held: Res<MouseHeld>,
    window: Query<Entity, With<bevy::window::PrimaryWindow>>,
    mode: Res<State<AppMode>>,
    latch: Option<ResMut<crate::game::player::InputLatch>>,
) {
    // `gather_input` stops while a menu is up, so held keys would stay held.
    // Aim is kept.
    if held.any()
        && let Some(mut latch) = latch
    {
        latch.0.release_controls();
    }

    // Only in Play: the resource's insertion counts as a change, and would
    // grab the cursor in the editor on the first frame.
    if *mode.get() == AppMode::Play {
        crate::game::set_cursor_captured(&mut commands, &window, !held.any());
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

}
