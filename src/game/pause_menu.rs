//! The pause menu: `Escape` while playing, and the mouse comes back.
//!
//! Deliberately plain Bevy UI rather than an `egui` window. The editor's
//! panels are egui because they are tools; this is part of the game, has to
//! exist in a build with no editor in it, and has to work on a browser tab
//! where the egui layer is one more thing to have gone wrong.
//!
//! **Releasing the mouse is the point of it.** A grabbed cursor is not
//! something a player can get out of by clicking somewhere else, so a window
//! with no way to hand the pointer back is a window you have to kill. That is
//! also why pausing switches off aim and movement gathering rather than only
//! showing something: a released cursor that still spun the view would be
//! worse than a captured one.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::get;

/// Whether the pause menu is showing.
///
/// A resource rather than a `SubState` because nothing hangs off it but a few
/// run conditions, and a state would pull `OnEnter`/`OnExit` transitions into
/// something that is one boolean.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PauseMenu {
    pub open: bool,
}

/// Marks the menu's root node, so closing it is a despawn of one entity.
#[derive(Component)]
struct PauseMenuRoot;

/// Run condition: the player is playing rather than looking at the menu.
///
/// What everything that reads the mouse or the keyboard for the body is gated
/// on. Note it answers `true` when the resource is missing, so the game layer
/// still runs in a test app that has not added this plugin.
pub fn not_paused(menu: Option<Res<PauseMenu>>) -> bool {
    menu.is_none_or(|menu| !menu.open)
}

pub struct PauseMenuPlugin;

impl Plugin for PauseMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PauseMenu>()
            .add_systems(
                Update,
                (
                    toggle_pause.run_if(in_state(AppMode::Play)),
                    // Chained, so a press and what it does to the screen land
                    // on the same frame. Unordered, this runs first as often
                    // as not and the menu appears a frame late — which is
                    // exactly long enough to look like a dropped keypress.
                    //
                    // Not itself gated on `AppMode`: it has to run on the
                    // frame the menu is closed by *leaving* Play, and by then
                    // the gate is already false.
                    show_or_hide.run_if(resource_changed::<PauseMenu>),
                )
                    .chain(),
            )
            .add_systems(OnExit(AppMode::Play), close_on_leaving_play);
    }
}

/// `Escape` both ways.
fn toggle_pause(keys: Res<ButtonInput<KeyCode>>, mut menu: ResMut<PauseMenu>) {
    if keys.just_pressed(KeyCode::Escape) {
        menu.open = !menu.open;
    }
}

/// Put the menu up or take it down, and hand the mouse over or take it back.
///
/// One system for both halves so the cursor and the menu cannot disagree —
/// split across an `OnEnter`/`OnExit` pair they eventually would, and a menu
/// showing with the cursor still grabbed is a menu nobody can click.
fn show_or_hide(
    mut commands: Commands,
    menu: Res<PauseMenu>,
    existing: Query<Entity, With<PauseMenuRoot>>,
    window: Query<Entity, With<bevy::window::PrimaryWindow>>,
    mode: Res<State<AppMode>>,
    // Optional so the menu can be tested without the rest of the game layer.
    latch: Option<ResMut<crate::game::player::InputLatch>>,
) {
    for root in &existing {
        commands.entity(root).despawn();
    }

    if menu.open {
        // A key held as the menu goes up would otherwise stay held in the
        // latch, and the body would walk into a wall for as long as the menu
        // was open — `gather_input` is not running to correct it. Aim is
        // deliberately kept: it is where the player put the view, not
        // something a key is holding down.
        if let Some(mut latch) = latch {
            latch.0.release_controls();
        }

        commands.spawn((
            PauseMenuRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            // Dimmed rather than opaque: the match is still running behind it,
            // and a player who cannot see what is happening to them while the
            // menu is up will not open it.
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
            children![(
                Text::new(get!("pause.title")),
                TextFont { font_size: bevy::text::FontSize::Px(42.0), ..default() },
                TextColor(Color::WHITE),
            )],
        ));
    }

    // Captured exactly when the menu is not showing. Stated as a fact about
    // the menu rather than toggled, so the two cannot drift apart.
    //
    // Only while playing, and this is not a tidiness guard: change detection
    // counts a resource's insertion as a change, so without it the first frame
    // of the process would grab the cursor inside the editor.
    if *mode.get() == AppMode::Play {
        crate::game::set_cursor_captured(&mut commands, &window, !menu.open);
    }
}

/// The round ended, or somebody went back to the editor.
///
/// The menu belongs to a match. Left open it would be a black overlay across
/// the editor with no key bound to close it, since `toggle_pause` only runs
/// while playing.
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

    /// A tap, driven through `Update` rather than through `app.update()`.
    ///
    /// `InputPlugin` clears `just_pressed` in `PreUpdate`, so a press made and
    /// then handed to a full frame is a press that was already forgotten by
    /// the time `toggle_pause` runs.
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

    /// Opening the menu lets go of the controls and keeps the view.
    ///
    /// The bug this pins was reported as "I press escape and lose my
    /// orientation": clearing the whole latch took `yaw` and `pitch` with the
    /// held keys, and the body snapped round to face north.
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

    /// The menu belongs to a match. Left up it would be a black overlay across
    /// the editor with no key bound to take it down, because `toggle_pause`
    /// only runs while playing.
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

    /// Escape while editing belongs to the tools, not to a menu that has no
    /// match to pause.
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

    /// The run condition has to answer sensibly for an app that never added
    /// this plugin, or the game layer stops stepping in every headless test.
    #[test]
    fn a_game_with_no_menu_is_not_paused() {
        let mut world = World::new();
        assert!(world.run_system_once(not_paused).unwrap());
    }

}
