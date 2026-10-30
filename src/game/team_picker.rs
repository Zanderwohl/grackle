//! The team picker: up on entering a match until a seat is taken, and on `.`
//! after. Plain Bevy UI, like the pause menu. What it offers and what a choice
//! does is [`crate::game::team_choice`]; this only asks.

use bevy::picking::events::PointerClick;
use bevy::picking::hover::Hovered;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::class::Stance;
use crate::common::fonts::DisplayFont;
use crate::common::swatch::Swatch;
use crate::common::team::Side;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::pause_menu::MouseHeld;
use crate::game::player::{fallback_spawn, DeathCam, InputLatch, LocalPlayer, OurLastBody};
use crate::game::respawn::{placed_spawns, Seat};
use crate::game::team_choice::{ChooseTeam, TeamChoice, TeamOptions};
use crate::get;

/// Whether the player asked for the picker with `.`. It is up regardless while
/// they have no seat; [`MouseHeld::team_picker`] is whether it is up.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TeamPicker {
    pub wanted: bool,
    /// The seat a choice was asked from. The picker stays up until that seat
    /// is left, so a refusal leaves it showing the new offer rather than
    /// closing on nothing.
    pub asked_from: Option<Seat>,
}

const ROW_WIDTH: f32 = 420.0;
const ROW_HEIGHT: f32 = 56.0;
const ROW_GAP: f32 = 10.0;
/// How far a hovered row moves left, in pixels. The row's hover zone is this
/// much wider than its face, so a face that has moved stays hovered.
const SLIDE: f32 = 20.0;
const SLIDE_SECONDS: f32 = 0.1;
/// Under the pause menu.
const PICKER_LAYER: i32 = 10;
const CLOSED_ALPHA: f32 = 0.35;
/// Radians, looking down at the spawn point while choosing.
const OVERLOOK_PITCH: f32 = -0.25;

#[derive(Component)]
struct PickerRoot;

/// The hover zone. Its one child is the [`Face`].
#[derive(Component, Clone, Copy)]
struct PickerRow {
    open: bool,
}

/// Pixels moved left so far.
#[derive(Component, Default)]
struct Face(f32);

pub struct TeamPickerPlugin;

impl Plugin for TeamPickerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TeamPicker>()
            .init_resource::<MouseHeld>()
            .init_resource::<DisplayFont>()
            .init_resource::<TeamOptions>()
            .add_message::<ChooseTeam>()
            .add_systems(
                Update,
                (
                    (toggle_the_picker, decide_the_picker, overlook_while_choosing).chain().run_if(in_state(AppMode::Play)),
                    // Ungated, so leaving Play takes it down.
                    show_or_hide.run_if(resource_changed::<MouseHeld>.or_else(resource_changed::<TeamOptions>)),
                    slide_the_rows.run_if(in_state(AppMode::Play)),
                )
                    .chain(),
            )
            .add_systems(OnExit(AppMode::Play), put_it_down);
    }
}

fn toggle_the_picker(keys: Res<ButtonInput<KeyCode>>, mut picker: ResMut<TeamPicker>) {
    if keys.just_pressed(KeyCode::Period) {
        picker.wanted = !picker.wanted;
        picker.asked_from = None;
    }
}

/// Up while there is no seat, so it bars the way into the match.
fn decide_the_picker(mut picker: ResMut<TeamPicker>, offer: Res<TeamOptions>, mut held: ResMut<MouseHeld>) {
    if picker.asked_from.is_some_and(|from| from != offer.seat) {
        *picker = TeamPicker::default();
    }
    let up = picker.wanted || offer.seat == Seat::Choosing;
    if held.team_picker != up {
        held.team_picker = up;
    }
}

fn put_it_down(mut picker: ResMut<TeamPicker>, mut held: ResMut<MouseHeld>) {
    *picker = TeamPicker::default();
    held.team_picker = false;
}

/// With no body and no death to watch, the view looks out from behind a spawn
/// point. A [`DeathCam`] with nobody's corpse, so the next body takes it down
/// as it does after a death. Not while [`OurLastBody`] is held: that is a
/// death cam about to start.
fn overlook_while_choosing(
    mut commands: Commands,
    ours: Query<(), With<LocalPlayer>>,
    death_cam: Option<Res<DeathCam>>,
    last: Option<Res<OurLastBody>>,
    spawns: Query<(&Transform, Option<&Side>), With<SpawnPointMarker>>,
    collision: Res<CollisionWorld>,
    mut latch: ResMut<InputLatch>,
) {
    if !ours.is_empty() || death_cam.is_some() || last.is_some() {
        return;
    }
    // A client's map can arrive after the round starts.
    let spawn = match placed_spawns(&spawns).into_iter().next() {
        Some(spawn) => spawn,
        None if collision.volumes().is_empty() => return,
        None => fallback_spawn(&collision),
    };
    commands.insert_resource(DeathCam { pivot: spawn.feet + Vec3::Y * Stance::Standing.eye_height(), whose: None });
    latch.0.yaw = spawn.yaw;
    latch.0.pitch = OVERLOOK_PITCH;
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
    rows.push((TeamChoice::Spectate, get!("team.picker.spectate"), Swatch::neutral(), false));
    rows
}

/// Rebuilt whole only when a row would change, so a hover in progress is not
/// reset by an offer that leaves every row as it was.
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

    let text_font = TextFont { font: (&font.0).into(), font_size: bevy::text::FontSize::Px(30.0), ..default() };
    commands
        .spawn((
            PickerRoot,
            GlobalZIndex(PICKER_LAYER),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(ROW_GAP),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
        ))
        .with_children(|root| {
            for (choice, label, swatch, open) in rows(&offer) {
                let alpha = if open { 1.0 } else { CLOSED_ALPHA };
                root.spawn((
                    PickerRow { open },
                    Hovered::default(),
                    Node { width: Val::Px(ROW_WIDTH + SLIDE), height: Val::Px(ROW_HEIGHT), ..default() },
                ))
                .observe(move |click: On<PointerClick>, offer: Res<TeamOptions>, mut picker: ResMut<TeamPicker>, mut requests: MessageWriter<ChooseTeam>| {
                    if open && click.button == PointerButton::Primary {
                        requests.write(ChooseTeam { choice, from: offer.seat });
                        picker.asked_from = Some(offer.seat);
                    }
                })
                .with_children(|row| {
                    row.spawn((
                        Face::default(),
                        Node {
                            position_type: PositionType::Relative,
                            left: Val::Px(SLIDE),
                            width: Val::Px(ROW_WIDTH),
                            height: Val::Percent(100.0),
                            align_items: AlignItems::Center,
                            padding: UiRect::horizontal(Val::Px(20.0)),
                            ..default()
                        },
                        BackgroundColor(swatch.base.with_alpha(alpha)),
                        children![(Text::new(label), text_font.clone(), TextColor(swatch.text().with_alpha(alpha)))],
                    ));
                });
            }
        });
}

fn slide_the_rows(
    time: Res<Time>,
    rows: Query<(&PickerRow, &Hovered, &Children)>,
    mut faces: Query<(&mut Face, &mut Node)>,
) {
    let step = SLIDE / SLIDE_SECONDS * time.delta_secs();
    for (row, hovered, children) in &rows {
        let target = if row.open && hovered.get() { SLIDE } else { 0.0 };
        for &child in children {
            let Ok((mut face, mut node)) = faces.get_mut(child) else { continue };
            let moved = face.0 + (target - face.0).clamp(-step, step);
            if face.0 != moved {
                face.0 = moved;
                node.left = Val::Px(SLIDE - moved);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::team::Team;

    #[test]
    fn free_for_all_offers_one_way_in_and_spectating() {
        let offer = TeamOptions { seat: Seat::Choosing, teams: false, open: Vec::new(), random: true };
        let shown: Vec<_> = rows(&offer).into_iter().map(|(choice, _, _, open)| (choice, open)).collect();
        assert_eq!(shown, vec![(TeamChoice::Random, true), (TeamChoice::Spectate, false)]);
    }

    #[test]
    fn teams_are_offered_between_random_and_spectating() {
        let offer = TeamOptions {
            seat: Seat::Playing(Some(Team::Red)),
            teams: true,
            open: vec![(Team::Red, false), (Team::Blue, true)],
            random: true,
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
}
