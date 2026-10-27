//! Holding Tab: the HUD goes and a table of everybody, ranked by points, comes
//! up in its place.
//!
//! Whether it is up is worked out again every frame from whether Tab is held
//! *now*, never toggled on a press and undone on a release: a release that
//! goes somewhere else — the pause menu opening over it, another window
//! taking the keyboard — would leave a toggle stuck. Bevy lets go of every key
//! when the window loses focus, so held is always the truth.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
use crate::common::fonts::MonoFont;
use crate::common::swatch::{Ink, Swatch};
use crate::common::team::Team;
use crate::common::ui_shape::{Corner, Path, ShapeMaterial, ShapeStyle};
use crate::game::hud::GameHud;
use crate::game::pause_menu::{not_paused, PauseMenu};
use crate::game::player::LocalPlayer;
use crate::game::score::{ranked_by_points, Standing};
use crate::get;

const TEXT: f32 = 20.0;
/// Logical pixels.
const SWATCH: f32 = 20.0;
const NAME_WIDTH: f32 = 280.0;
const NUMBER_WIDTH: f32 = 96.0;
const ROW_HEIGHT: f32 = 32.0;

#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TabBoard {
    pub up: bool,
}

#[derive(Component)]
struct TabBoardRoot;

pub struct TabBoardPlugin;

impl Plugin for TabBoardPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TabBoard>()
            .init_resource::<MonoFont>()
            .add_systems(
                Update,
                (hold_the_board, spawn_the_board, show_the_board, fill_in_the_board)
                    .chain()
                    .run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), put_the_board_away);
    }
}

/// Not while paused: the menu has the screen, and comes down onto the board
/// again if Tab is still held.
fn hold_the_board(keys: Res<ButtonInput<KeyCode>>, paused: Option<Res<PauseMenu>>, mut board: ResMut<TabBoard>) {
    board.set_if_neq(TabBoard { up: keys.pressed(KeyCode::Tab) && not_paused(paused) });
}

/// Keyed on `Added<LocalPlayer>` like the HUD, so there is a camera to draw
/// it; kept through deaths, so it can be held from the death cam.
fn spawn_the_board(
    mut commands: Commands,
    bodies: Query<(), Added<LocalPlayer>>,
    existing: Query<(), With<TabBoardRoot>>,
) {
    if bodies.is_empty() || !existing.is_empty() {
        return;
    }
    commands.spawn((
        TabBoardRoot,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            display: Display::None,
            ..default()
        },
        Pickable::IGNORE,
        Name::new("Tab scoreboard"),
    ));
}

/// Compared before writing, so a still frame changes nothing.
fn show_the_board(
    board: Res<TabBoard>,
    mut roots: Query<&mut Node, With<TabBoardRoot>>,
    mut hud: Query<&mut Visibility, With<GameHud>>,
) {
    let display = if board.up { Display::Flex } else { Display::None };
    for mut node in &mut roots {
        if node.display != display {
            node.display = display;
        }
    }
    let visibility = if board.up { Visibility::Hidden } else { Visibility::Inherited };
    for mut shown in &mut hud {
        shown.set_if_neq(visibility);
    }
}

/// Rebuilt only while up, on coming up and on any change to the standings.
#[allow(clippy::too_many_arguments)]
fn fill_in_the_board(
    mut commands: Commands,
    board: Res<TabBoard>,
    font: Res<MonoFont>,
    standings: Query<&Standing>,
    changed: Query<(), Changed<Standing>>,
    mut removed: RemovedComponents<Standing>,
    us: Query<&PlayerId, With<LocalPlayer>>,
    roots: Query<Entity, With<TabBoardRoot>>,
    materials: Option<ResMut<Assets<ShapeMaterial>>>,
    mut swatches: Local<HashMap<(Option<Team>, u32), Handle<ShapeMaterial>>>,
) {
    let gone = removed.read().count() > 0;
    let Ok(root) = roots.single() else { return };
    if !board.up || !(board.is_changed() || gone || !changed.is_empty()) {
        return;
    }
    let ours = us.single().ok().copied();
    let mut materials = materials;
    let font = &font.0;

    let mut rows = vec![commands.spawn(header(font)).id()];
    for standing in ranked_by_points(standings.iter()) {
        let swatch = materials.as_deref_mut().map(|materials| {
            swatches
                .entry((standing.team, standing.color.hue.to_bits()))
                .or_insert_with(|| materials.add(squircle(&Swatch::of_body(standing.team, Some(standing.color)))))
                .clone()
        });
        rows.push(row(&mut commands, standing, Some(standing.id) == ours, swatch, font));
    }
    let table = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(16.0)),
                row_gap: Val::Px(2.0),
                border_radius: BorderRadius::all(Val::Px(12.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
            Pickable::IGNORE,
        ))
        .add_children(&rows)
        .id();
    commands.entity(root).despawn_children().add_children(&[table]);
}

fn put_the_board_away(
    mut commands: Commands,
    mut board: ResMut<TabBoard>,
    roots: Query<Entity, With<TabBoardRoot>>,
    mut hud: Query<&mut Visibility, With<GameHud>>,
) {
    board.set_if_neq(TabBoard::default());
    for root in &roots {
        commands.entity(root).despawn();
    }
    for mut shown in &mut hud {
        shown.set_if_neq(Visibility::Inherited);
    }
}

fn squircle(swatch: &Swatch) -> ShapeMaterial {
    let size = Vec2::splat(SWATCH);
    let inset = 1.5;
    let square = [
        Vec2::splat(inset),
        Vec2::new(SWATCH - inset, inset),
        Vec2::splat(SWATCH - inset),
        Vec2::new(inset, SWATCH - inset),
    ];
    let style = ShapeStyle {
        fill: swatch.base,
        highlight: swatch.highlight,
        shadow: swatch.shadow,
        rim: 2.0,
        stroke: Ink::Dark.on(swatch),
        stroke_width: 2.0,
        ..default()
    };
    ShapeMaterial::from_path(&Path::rounded_polygon(&square, Corner::squircle(7.0)), size, style)
}

fn cell(font: &Handle<Font>, words: String, width: f32, right: bool, color: Color) -> impl Bundle {
    (
        Node {
            width: Val::Px(width),
            justify_content: if right { JustifyContent::FlexEnd } else { JustifyContent::FlexStart },
            overflow: Overflow::clip(),
            ..default()
        },
        Pickable::IGNORE,
        children![(
            Text::new(words),
            TextFont { font: font.into(), font_size: bevy::text::FontSize::Px(TEXT), ..default() },
            TextColor(color),
            TextLayout::linebreak(bevy::text::LineBreak::NoWrap),
            Pickable::IGNORE,
        )],
    )
}

fn row_node() -> Node {
    Node {
        height: Val::Px(ROW_HEIGHT),
        align_items: AlignItems::Center,
        column_gap: Val::Px(12.0),
        padding: UiRect::horizontal(Val::Px(8.0)),
        border_radius: BorderRadius::all(Val::Px(6.0)),
        ..default()
    }
}

fn header(font: &Handle<Font>) -> impl Bundle {
    let dim = Color::srgba(1.0, 1.0, 1.0, 0.6);
    let number = |key: &str| cell(font, get!(key), NUMBER_WIDTH, true, dim);
    (
        row_node(),
        Pickable::IGNORE,
        children![
            Node { width: Val::Px(SWATCH), ..default() },
            cell(font, get!("scoreboard.name"), NAME_WIDTH, false, dim),
            number("scoreboard.points"),
            number("scoreboard.kills"),
            number("scoreboard.assists"),
            number("scoreboard.deaths"),
            number("scoreboard.healing"),
        ],
    )
}

fn row(
    commands: &mut Commands,
    standing: &Standing,
    ours: bool,
    swatch: Option<Handle<ShapeMaterial>>,
    font: &Handle<Font>,
) -> Entity {
    let white = Color::WHITE;
    let number = |value: String| cell(font, value, NUMBER_WIDTH, true, white);
    let mut chip = commands.spawn((Node { width: Val::Px(SWATCH), height: Val::Px(SWATCH), ..default() }, Pickable::IGNORE));
    if let Some(swatch) = swatch {
        chip.insert(MaterialNode(swatch));
    }
    let chip = chip.id();
    commands
        .spawn((
            row_node(),
            BackgroundColor(if ours { Color::srgba(1.0, 1.0, 1.0, 0.15) } else { Color::NONE }),
            Pickable::IGNORE,
        ))
        .add_child(chip)
        .with_children(|row| {
            row.spawn(cell(font, standing.display_name(), NAME_WIDTH, false, white));
            row.spawn(number(standing.shown_points().to_string()));
            row.spawn(number(standing.kills.to_string()));
            row.spawn(number(standing.assists.to_string()));
            row.spawn(number(standing.deaths.to_string()));
            row.spawn(number(standing.healing.to_string()));
        })
        .id()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::state::app::StatesPlugin, bevy::input::InputPlugin))
            .init_state::<AppMode>()
            .init_resource::<PauseMenu>()
            .add_plugins(TabBoardPlugin);
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        app
    }

    fn press(app: &mut App, held: bool) {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        if held {
            keys.press(KeyCode::Tab);
        } else {
            keys.release(KeyCode::Tab);
        }
        app.update();
    }

    fn up(app: &App) -> bool {
        app.world().resource::<TabBoard>().up
    }

    #[test]
    fn the_board_is_up_exactly_while_tab_is_held_and_hides_the_hud() {
        let mut app = app();
        let hud = app.world_mut().spawn((GameHud, Visibility::Inherited)).id();

        press(&mut app, true);
        assert!(up(&app));
        assert_eq!(app.world().get::<Visibility>(hud), Some(&Visibility::Hidden));

        press(&mut app, false);
        assert!(!up(&app));
        assert_eq!(app.world().get::<Visibility>(hud), Some(&Visibility::Inherited));
    }

    /// Escape while holding Tab: the menu takes over, and closing it with Tab
    /// let go leaves nothing behind.
    #[test]
    fn pausing_over_the_board_does_not_leave_it_stuck() {
        let mut app = app();
        press(&mut app, true);
        app.world_mut().resource_mut::<PauseMenu>().open = true;
        app.update();
        assert!(!up(&app));

        press(&mut app, false);
        app.world_mut().resource_mut::<PauseMenu>().open = false;
        app.update();
        assert!(!up(&app));
    }

    #[test]
    fn leaving_play_puts_the_hud_back() {
        let mut app = app();
        let hud = app.world_mut().spawn((GameHud, Visibility::Inherited)).id();
        press(&mut app, true);
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Editor);
        app.update();
        assert!(!up(&app));
        assert_eq!(app.world().get::<Visibility>(hud), Some(&Visibility::Inherited));
    }
}
