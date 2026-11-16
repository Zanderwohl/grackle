//! The team picker: up on entering a match until a seat is taken, and on `.`
//! after. Plain Bevy UI, like the pause menu. What it offers and what a choice
//! does is [`crate::game::team_choice`]; this only asks.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::fonts::DisplayFont;
use crate::common::swatch::Swatch;
use crate::common::team::Side;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::menu::{menu_button, menu_item_node, menu_panel, menu_title, ITEM_SIZE};
use crate::game::pause_menu::{ChangeTeamsAsked, EscapeTaken, MouseHeld, PauseMenu, TakeEscape};
use crate::game::plate::{plate_text, FollowsTheLean, Plate};
use crate::game::plate_button::ButtonActivated;
use crate::game::player::{fallback_spawn, DeathCam, InputLatch, LocalPlayer, OurLastBody};
use crate::game::spectate::{first_camera_for, Cameras};
use crate::game::respawn::{placed_spawns, Seat};
use crate::game::team_choice::{ChooseTeam, TeamChoice, TeamOptions};
use crate::get;

/// Whether the player asked for the picker with `.`. It is up regardless while
/// they have no seat; [`MouseHeld::team_picker`] is whether it is up.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TeamPicker {
    pub wanted: bool,
    /// The picker stays up until this seat is left, so a refusal leaves it
    /// showing the new offer.
    pub asked_from: Option<Seat>,
}

impl TeamPicker {
    /// With no seat the picker bars the way into the match, asked for or not.
    pub fn bars_the_way(offer: &TeamOptions) -> bool {
        offer.seat == Seat::Choosing
    }

    pub fn is_up(&self, offer: &TeamOptions) -> bool {
        self.wanted || Self::bars_the_way(offer)
    }

    /// Escape takes it down: up only because it was asked for.
    pub fn closable(&self, offer: &TeamOptions) -> bool {
        self.is_up(offer) && !Self::bars_the_way(offer)
    }
}

/// Under the pause menu.
const PICKER_LAYER: i32 = 10;
/// Shares of a full team's OKLCH lightness and chroma.
const CLOSED_LIGHTNESS: f32 = 0.6;
const CLOSED_CHROMA: f32 = 0.3;
/// Radians, looking down at the spawn point while choosing.
pub(crate) const OVERLOOK_PITCH: f32 = -0.25;

#[derive(Component)]
struct PickerRoot;

#[derive(Component, Clone, Copy)]
struct PickerItem(TeamChoice);

pub struct TeamPickerPlugin;

impl Plugin for TeamPickerPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        app.init_resource::<TeamPicker>()
            .init_resource::<EscapeTaken>()
            .add_observer(choose)
            .init_resource::<MouseHeld>()
            .init_resource::<DisplayFont>()
            .init_resource::<TeamOptions>()
            .add_message::<ChooseTeam>()
            .add_systems(
                Update,
                (
                    (escape_closes_the_picker.in_set(TakeEscape), toggle_the_picker, decide_the_picker, overlook_while_choosing)
                        .chain()
                        .run_if(in_state(AppMode::Play)),
                    // Ungated, so leaving Play takes it down.
                    show_or_hide.run_if(resource_changed::<MouseHeld>.or_else(resource_changed::<TeamOptions>)),
                )
                    .chain(),
            )
            .add_systems(OnExit(AppMode::Play), put_it_down);
    }
}

/// Not while the pause menu is open over it, which Escape closes first.
fn escape_closes_the_picker(
    actions: Res<crate::common::keybind::Actions>,
    menu: Option<Res<PauseMenu>>,
    offer: Res<TeamOptions>,
    mut picker: ResMut<TeamPicker>,
    mut taken: ResMut<EscapeTaken>,
) {
    let menu_open = menu.is_some_and(|menu| menu.open);
    if actions.just_pressed(crate::common::keybind::Action::Pause) && !menu_open && picker.closable(&offer) {
        *picker = TeamPicker::default();
        taken.0 = true;
    }
}

fn toggle_the_picker(
    mut commands: Commands,
    actions: Res<crate::common::keybind::Actions>,
    asked: Option<Res<ChangeTeamsAsked>>,
    mut picker: ResMut<TeamPicker>,
) {
    if asked.is_some() {
        commands.remove_resource::<ChangeTeamsAsked>();
        *picker = TeamPicker { wanted: true, asked_from: None };
    } else if actions.just_pressed(crate::common::keybind::Action::TeamPicker) {
        picker.wanted = !picker.wanted;
        picker.asked_from = None;
    }
}

/// Up while there is no seat, so it bars the way into the match.
fn decide_the_picker(mut picker: ResMut<TeamPicker>, offer: Res<TeamOptions>, mut held: ResMut<MouseHeld>) {
    if picker.asked_from.is_some_and(|from| from != offer.seat) {
        *picker = TeamPicker::default();
    }
    let up = picker.is_up(&offer);
    if held.team_picker != up {
        held.team_picker = up;
    }
}

fn put_it_down(mut picker: ResMut<TeamPicker>, mut held: ResMut<MouseHeld>) {
    *picker = TeamPicker::default();
    held.team_picker = false;
}

/// With no body and no death to watch, the view orbits a spawn point: a
/// [`DeathCam`] of nobody's, so the next body takes it down. Not while
/// [`OurLastBody`] is held, as a death cam is about to start, nor while our
/// seat may use a spectator camera, which `look_through` puts the view in.
/// A spectator's view is [`crate::game::spectate`]'s.
#[allow(clippy::too_many_arguments)]
fn overlook_while_choosing(
    mut commands: Commands,
    offer: Res<TeamOptions>,
    ours: Query<(), With<LocalPlayer>>,
    death_cam: Option<Res<DeathCam>>,
    last: Option<Res<OurLastBody>>,
    spawns: Query<(&Transform, Option<&Side>), With<SpawnPointMarker>>,
    cameras: Cameras,
    collision: Res<CollisionWorld>,
    mut latch: ResMut<InputLatch>,
) {
    let dead = death_cam.as_deref().is_some_and(|cam| cam.whose.is_some());
    if !ours.is_empty() || dead || last.is_some() || offer.seat == Seat::Spectating {
        return;
    }
    if first_camera_for(&cameras, offer.seat).is_some() {
        if death_cam.is_some() {
            commands.remove_resource::<DeathCam>();
        }
        return;
    }
    if death_cam.is_some() {
        return;
    }
    // A client's map can arrive after the round starts.
    let spawn = match placed_spawns(&spawns).into_iter().next() {
        Some(spawn) => spawn,
        None if collision.volumes().is_empty() => return,
        None => fallback_spawn(&collision),
    };
    commands.insert_resource(overlook(spawn.feet));
    latch.0.yaw = spawn.yaw;
    latch.0.pitch = OVERLOOK_PITCH;
}

/// Orbiting a spawn point at a standing eye over its `feet`.
pub(crate) fn overlook(feet: Vec3) -> DeathCam {
    DeathCam { pivot: feet + Vec3::Y * Stance::Standing.eye_height(), whose: None }
}

fn rows(offer: &TeamOptions) -> Vec<(TeamChoice, String, Swatch, bool)> {
    let mut rows = Vec::new();
    match offer.teams {
        true => {
            rows.push((TeamChoice::Random, get!("team.picker.random"), Swatch::neutral(), offer.random));
            for &(team, open) in &offer.open {
                rows.push((TeamChoice::Join(team), team.name(), Swatch::team(team), open));
            }
        }
        false => rows.push((TeamChoice::Random, get!("team.picker.free_for_all"), Swatch::neutral(), offer.random)),
    }
    rows.push((TeamChoice::Spectate, get!("team.picker.spectate"), Swatch::neutral(), offer.spectate));
    rows
}

/// Rebuilt only when a row would change, so an unrelated offer does not
/// reset a hover.
fn show_or_hide(
    mut commands: Commands,
    held: Res<MouseHeld>,
    offer: Res<TeamOptions>,
    existing: Query<Entity, With<PickerRoot>>,
    font: Res<DisplayFont>,
    mut shown: Local<Option<(bool, TeamOptions)>>,
) {
    // The seat is read when a row is clicked, not drawn.
    let wanted = (held.team_picker, TeamOptions { seat: Seat::Choosing, ..offer.clone() });
    if shown.as_ref() == Some(&wanted) {
        return;
    }
    *shown = Some(wanted);
    for root in &existing {
        commands.entity(root).despawn();
    }
    if !held.team_picker {
        return;
    }

    let font = &font.0;
    commands
        .spawn((
            PickerRoot,
            GlobalZIndex(PICKER_LAYER),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
        ))
        .with_children(|root| {
            root.spawn(menu_panel()).with_children(|menu| {
                menu.spawn(menu_title(font, get!("team.picker.title")));
                for (choice, label, swatch, open) in rows(&offer) {
                    let mut row = match open {
                        true => menu.spawn((PickerItem(choice), menu_button(swatch))),
                        // A plain plate, so it neither lights nor answers a click.
                        false => menu.spawn((PickerItem(choice), Plate::new(swatch.muted(CLOSED_LIGHTNESS, CLOSED_CHROMA)), FollowsTheLean, menu_item_node())),
                    };
                    row.with_child(plate_text(font, label, ITEM_SIZE));
                }
            });
        });
}

fn choose(
    activated: On<ButtonActivated>,
    items: Query<&PickerItem>,
    offer: Res<TeamOptions>,
    mut picker: ResMut<TeamPicker>,
    mut requests: MessageWriter<ChooseTeam>,
) {
    let Ok(&PickerItem(choice)) = items.get(activated.entity) else { return };
    requests.write(ChooseTeam { choice, from: offer.seat });
    picker.asked_from = Some(offer.seat);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::team::Team;

    #[test]
    fn free_for_all_offers_one_way_in_and_spectating() {
        let offer = TeamOptions { seat: Seat::Choosing, teams: false, open: Vec::new(), random: true, spectate: true };
        let shown: Vec<_> = rows(&offer).into_iter().map(|(choice, _, _, open)| (choice, open)).collect();
        assert_eq!(shown, vec![(TeamChoice::Random, true), (TeamChoice::Spectate, true)]);
    }

    #[test]
    fn teams_are_offered_between_random_and_spectating() {
        let offer = TeamOptions {
            seat: Seat::Playing(Some(Team::Red)),
            teams: true,
            open: vec![(Team::Red, false), (Team::Blue, true)],
            random: true,
            spectate: false,
        };
        let shown: Vec<_> = rows(&offer).into_iter().map(|(choice, _, _, open)| (choice, open)).collect();
        assert_eq!(
            shown,
            vec![
                (TeamChoice::Random, true),
                (TeamChoice::Join(Team::Red), false),
                (TeamChoice::Join(Team::Blue), true),
                (TeamChoice::Spectate, false),
            ]
        );
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::input::InputPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        app.init_state::<AppMode>();
        app.init_resource::<InputLatch>();
        app.init_resource::<CollisionWorld>();
        app.add_plugins(TeamPickerPlugin);
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        app
    }

    fn up(app: &App) -> bool {
        app.world().resource::<MouseHeld>().team_picker
    }

    #[test]
    fn it_bars_the_way_in_until_a_seat_is_taken() {
        let mut app = app();
        assert!(up(&app), "a player with no seat walked straight into the match");

        app.world_mut().resource_mut::<TeamOptions>().seat = Seat::Playing(Some(Team::Red));
        app.update();
        assert!(!up(&app));
    }

    #[test]
    fn period_brings_it_back_and_leaving_play_puts_it_down() {
        let mut app = app();
        app.world_mut().resource_mut::<TeamOptions>().seat = Seat::Playing(None);
        app.update();

        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::Period);
        crate::common::keybind::resolve_now(app.world_mut());
        app.world_mut().run_schedule(Update);
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().release(KeyCode::Period);
        assert!(up(&app), "`.` did not open the picker");

        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Editor);
        app.update();
        assert!(!up(&app), "the picker outlived the match");
    }

    /// Breaks if a refusal closes the picker with nothing to show for it.
    #[test]
    fn a_choice_closes_the_picker_only_once_the_seat_changes() {
        let mut app = app();
        app.world_mut().resource_mut::<TeamOptions>().seat = Seat::Playing(Some(Team::Red));
        app.insert_resource(TeamPicker { wanted: true, asked_from: Some(Seat::Playing(Some(Team::Red))) });
        app.update();
        assert!(up(&app), "closed before the server answered");

        app.world_mut().resource_mut::<TeamOptions>().seat = Seat::Playing(Some(Team::Blue));
        app.update();
        assert!(!up(&app), "still up after the switch landed");
    }

    /// Breaks if a full team can be clicked, or an open one does not ask for its seat.
    #[test]
    fn open_rows_are_buttons_that_ask_and_full_teams_are_not() {
        use crate::game::plate_button::PlateButton;

        let mut app = app();
        app.insert_resource(TeamOptions {
            seat: Seat::Choosing,
            teams: true,
            open: vec![(Team::Red, false), (Team::Blue, true)],
            random: true,
            spectate: true,
        });
        app.update();

        let rows: Vec<(Entity, TeamChoice, bool)> = app
            .world_mut()
            .query::<(Entity, &PickerItem, Has<PlateButton>)>()
            .iter(app.world())
            .map(|(entity, item, button)| (entity, item.0, button))
            .collect();
        let button = |choice| rows.iter().find(|row| row.1 == choice).map(|row| row.2);
        assert_eq!(button(TeamChoice::Join(Team::Red)), Some(false), "a full team is a button");
        assert_eq!(button(TeamChoice::Join(Team::Blue)), Some(true));
        assert_eq!(button(TeamChoice::Spectate), Some(true));

        let blue = rows.iter().find(|row| row.1 == TeamChoice::Join(Team::Blue)).unwrap().0;
        app.world_mut().trigger(ButtonActivated { entity: blue });
        app.world_mut().flush();
        let asked: Vec<_> = app.world().resource::<Messages<ChooseTeam>>().iter_current_update_messages().map(|ask| ask.choice).collect();
        assert_eq!(asked, vec![TeamChoice::Join(Team::Blue)]);
        assert_eq!(app.world().resource::<TeamPicker>().asked_from, Some(Seat::Choosing));
    }

    fn press_escape(app: &mut App) {
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::Escape);
        crate::common::keybind::resolve_now(app.world_mut());
        app.world_mut().run_schedule(Update);
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().release(KeyCode::Escape);
    }

    /// Breaks if Escape leaves up a picker the player asked for, or lets the pause menu open too.
    #[test]
    fn escape_closes_a_picker_that_was_asked_for() {
        let mut app = app();
        app.world_mut().resource_mut::<TeamOptions>().seat = Seat::Playing(None);
        app.insert_resource(TeamPicker { wanted: true, asked_from: None });
        press_escape(&mut app);
        assert!(!app.world().resource::<TeamPicker>().wanted, "the picker stayed up");
        assert!(app.world().resource::<EscapeTaken>().0, "the pause menu would open as well");
    }

    /// Breaks if Escape is taken while the picker bars the way in, leaving no way to the menu.
    #[test]
    fn escape_is_left_for_the_menu_while_the_picker_bars_the_way() {
        let mut app = app();
        app.insert_resource(TeamPicker { wanted: true, asked_from: None });
        press_escape(&mut app);
        assert!(!app.world().resource::<EscapeTaken>().0, "no way to the menu while choosing");
    }

    /// Breaks if the pause menu's Change Teams does not bring the picker up.
    #[test]
    fn change_teams_from_the_pause_menu_brings_it_up() {
        let mut app = app();
        app.world_mut().resource_mut::<TeamOptions>().seat = Seat::Playing(None);
        app.insert_resource(ChangeTeamsAsked);
        app.update();
        assert!(up(&app), "the picker did not come up");
        assert!(!app.world().contains_resource::<ChangeTeamsAsked>());
    }
}
