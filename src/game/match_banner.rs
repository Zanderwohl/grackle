//! The pre-game's words: the map's name a third of the way down and the countdown two thirds of
//! the way down, both centered on those lines. "Start!" stays up for [`START_SHOWN_FOR`] into the
//! match. Between rounds, a line just above the bottom of the window safe zone says the next round
//! is coming. Plain Bevy UI, like the HUD, since it is part of the game.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::fonts::DisplayFont;
use crate::common::map_sync::ServerMapFile;
use crate::common::match_phase::{phase_of, Countdown, MatchPhase};
use crate::editor::map_metadata::MapMetadata;
use crate::editor::panels::CurrentFilePath;
use crate::game::hud::WINDOW_MARGIN;
use crate::game::postcards::KeptClear;
use crate::get;

/// Real seconds "Start!" stays up after the match goes live.
pub const START_SHOWN_FOR: f32 = 1.0;

#[derive(Component)]
struct Banner;

#[derive(Component)]
struct MapTitle;

#[derive(Component)]
struct CountdownText;

#[derive(Component)]
struct BetweenRoundsText;

/// Logical pixels between the bottom of the window safe zone and the line's text.
const ABOVE_THE_SAFE_ZONE: f32 = 8.0;

pub struct MatchBannerPlugin;

impl Plugin for MatchBannerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DisplayFont>()
            .add_systems(Update, (spawn_the_banner, fill_in_the_banner).chain().run_if(in_state(AppMode::Play)))
            .add_systems(OnExit(AppMode::Play), despawn_the_banner);
    }
}

/// A full-width line of no height, `top` down the window, whose text is centered on it.
fn line(top: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        top: Val::Percent(top),
        width: Val::Percent(100.0),
        height: Val::Px(0.0),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    }
}

fn words(font: &Handle<Font>, size: f32, text: String) -> impl Bundle {
    (
        Text::new(text),
        TextFont { font: font.into(), font_size: bevy::text::FontSize::Vh(size), ..default() },
        TextColor(Color::WHITE),
        TextShadow::default(),
        Pickable::IGNORE,
    )
}

fn spawn_the_banner(mut commands: Commands, existing: Query<(), With<Banner>>, font: Res<DisplayFont>) {
    if !existing.is_empty() {
        return;
    }
    commands.spawn((
        Banner,
        Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        // A full-screen node would otherwise swallow the pause menu's clicks.
        Pickable::IGNORE,
        Visibility::Hidden,
        Name::new("Match banner"),
        children![
            (line(100.0 / 3.0), Pickable::IGNORE, children![(MapTitle, KeptClear, words(&font.0, 8.0, String::new()))]),
            (line(200.0 / 3.0), Pickable::IGNORE, children![(CountdownText, KeptClear, words(&font.0, 10.0, String::new()))]),
            (
                Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(WINDOW_MARGIN + ABOVE_THE_SAFE_ZONE),
                    width: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                Pickable::IGNORE,
                children![(BetweenRoundsText, KeptClear, words(&font.0, 4.0, get!("match.between_rounds")))],
            ),
        ],
    ));
}

pub fn countdown_words(left: Option<f32>) -> String {
    match left {
        None => get!("match.waiting"),
        Some(left) if left < 0.5 => get!("match.start"),
        Some(left) => format!("{}", left.round() as u32),
    }
}

/// The map's own name, else its file's: the server's, on a client.
fn map_name(metadata: Option<&MapMetadata>, server_file: Option<&ServerMapFile>, file: Option<&CurrentFilePath>) -> String {
    metadata
        .and_then(|metadata| metadata.display_name.clone())
        .filter(|name| !name.is_empty())
        .or_else(|| server_file?.0.clone())
        .or_else(|| file?.path.as_ref()?.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn fill_in_the_banner(
    time: Res<Time<Real>>,
    phase: Option<Res<State<MatchPhase>>>,
    countdown: Res<Countdown>,
    metadata: Option<Res<MapMetadata>>,
    (file, server_file): (Option<Res<CurrentFilePath>>, Option<Res<ServerMapFile>>),
    mut live_since: Local<Option<f32>>,
    mut banner: Query<&mut Visibility, With<Banner>>,
    mut title: Query<(&mut Text, &mut Visibility), (With<MapTitle>, Without<CountdownText>, Without<Banner>)>,
    mut count: Query<(&mut Text, &mut Visibility), (With<CountdownText>, Without<MapTitle>, Without<Banner>)>,
    mut between: Query<&mut Visibility, (With<BetweenRoundsText>, Without<MapTitle>, Without<CountdownText>, Without<Banner>)>,
) {
    let now = time.elapsed_secs();
    let phase = phase_of(&phase);
    match phase {
        Some(MatchPhase::Live) => {
            live_since.get_or_insert(now);
        }
        _ => *live_since = None,
    }
    let starting = live_since.is_some_and(|since| now - since < START_SHOWN_FOR);
    let between_rounds = phase == Some(MatchPhase::BetweenRounds);
    let titled = phase == Some(MatchPhase::PreGame) || (phase == Some(MatchPhase::Live) && starting);
    let counted = match phase {
        Some(MatchPhase::PreGame) => countdown.of(MatchPhase::PreGame).map(countdown_words),
        Some(MatchPhase::Live) if starting => Some(get!("match.start")),
        _ => None,
    };
    let shown = |yes: bool| if yes { Visibility::Inherited } else { Visibility::Hidden };
    for mut visibility in &mut banner {
        visibility.set_if_neq(shown(titled || between_rounds));
    }
    for mut visibility in &mut between {
        visibility.set_if_neq(shown(between_rounds));
    }
    let name = titled.then(|| map_name(metadata.as_deref(), server_file.as_deref(), file.as_deref()));
    for (text, visibility) in &mut title {
        write_line(text, visibility, name.as_deref());
    }
    for (text, visibility) in &mut count {
        write_line(text, visibility, counted.as_deref());
    }
}

/// Hidden with no words; compared before writing, so a still frame changes nothing.
fn write_line(mut text: Mut<Text>, mut visibility: Mut<Visibility>, words: Option<&str>) {
    visibility.set_if_neq(if words.is_some() { Visibility::Inherited } else { Visibility::Hidden });
    if let Some(words) = words.filter(|words| text.0 != *words) {
        text.0 = words.to_owned();
    }
}

fn despawn_the_banner(mut commands: Commands, banners: Query<Entity, With<Banner>>) {
    for banner in &banners {
        commands.entity(banner).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_countdown_rounds_and_says_start_below_half_a_second() {
        assert_eq!(countdown_words(Some(9.6)), "10");
        assert_eq!(countdown_words(Some(1.49)), "1");
        assert_eq!(countdown_words(Some(0.5)), "1");
        assert_eq!(countdown_words(Some(0.49)), get!("match.start"));
        assert_eq!(countdown_words(Some(0.0)), get!("match.start"));
        assert_eq!(countdown_words(None), get!("match.waiting"));
    }
}
