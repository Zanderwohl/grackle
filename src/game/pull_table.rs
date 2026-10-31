//! While `F8` shows the bots and a spectator watches one, every want and how hard it pulls, top
//! right of the [`WindowSafeZone`]: a debug view for tuning [`Drives`](crate::game::bot_think::Drives).

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::fonts::DisplayFont;
use crate::game::bot::ShowBots;
use crate::game::bot_think::{BotThought, Want};
use crate::game::hud::WindowSafeZone;
use crate::game::player::Watched;
use crate::get;

const TEXT: f32 = 16.0;
/// Alpha of a row that is not the chosen want.
const DIM: f32 = 0.6;

#[derive(Component)]
struct PullTable;

/// The `row`th want in alphabetical order: its name, or its pull.
#[derive(Component, Clone, Copy)]
struct PullCell {
    row: usize,
    score: bool,
}

pub struct PullTablePlugin;

impl Plugin for PullTablePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShowBots>().add_systems(Update, show_the_pulls.run_if(in_state(AppMode::Play)));
    }
}

/// Goes when the HUD does: it is the window safe zone's child.
fn pull_table(font: &Handle<Font>) -> impl Bundle {
    (
        PullTable,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(0.0),
            right: Val::Px(0.0),
            display: Display::Grid,
            grid_template_columns: vec![GridTrack::auto(), GridTrack::auto()],
            column_gap: Val::Px(14.0),
            row_gap: Val::Px(2.0),
            padding: UiRect::all(Val::Px(8.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        Visibility::Hidden,
        Pickable::IGNORE,
        Name::new("Pull table"),
        Children::spawn(SpawnIter(cells(font).into_iter())),
    )
}

fn cells(font: &Handle<Font>) -> Vec<impl Bundle> {
    (0..Want::ALL.len())
        .flat_map(|row| [false, true].map(|score| (row, score)))
        .map(|(row, score)| {
            let justify = if score { JustifySelf::End } else { JustifySelf::Start };
            (
                PullCell { row, score },
                Text::new(String::new()),
                TextFont { font: font.clone().into(), font_size: bevy::text::FontSize::Px(TEXT), ..default() },
                TextColor(Color::WHITE),
                Pickable::IGNORE,
                Node { justify_self: justify, ..default() },
            )
        })
        .collect()
}

/// Each want's name and pull, alphabetical by name, the chosen one undimmed. A want with nothing
/// to go for has no pull.
pub fn rows(thought: &BotThought) -> Vec<(String, String, bool)> {
    let mut rows: Vec<(String, String, bool)> = Want::ALL
        .iter()
        .map(|want| {
            let pull = thought.pulls.iter().find(|(pulled, _)| pulled == want).map_or_else(|| get!("bot.pull.none"), |(_, score)| format!("{score:.2}"));
            (want.name(), pull, thought.chosen.is_some_and(|chosen| chosen.want == *want))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

#[allow(clippy::type_complexity)]
fn show_the_pulls(
    mut commands: Commands,
    font: Res<DisplayFont>,
    show: Res<ShowBots>,
    zone: Option<Single<Entity, With<WindowSafeZone>>>,
    watched: Option<Single<&BotThought, With<Watched>>>,
    mut tables: Query<&mut Visibility, With<PullTable>>,
    mut cells: Query<(&PullCell, &mut Text, &mut TextColor)>,
) {
    let Some(zone) = zone else { return };
    let Some(mut visibility) = tables.iter_mut().next() else {
        commands.entity(*zone).with_child(pull_table(&font.0));
        return;
    };
    let Some(thought) = watched.filter(|_| show.0) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    visibility.set_if_neq(Visibility::Inherited);
    let rows = rows(&thought);
    for (cell, mut text, mut ink) in &mut cells {
        let Some((name, pull, chosen)) = rows.get(cell.row) else { continue };
        let words = if cell.score { pull } else { name };
        if text.0 != *words {
            text.0 = words.clone();
        }
        ink.set_if_neq(TextColor(Color::srgba(1.0, 1.0, 1.0, if *chosen { 1.0 } else { DIM })));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::bot::Goal;
    use crate::game::bot_think::Candidate;

    /// Breaks if the table drops a want with nothing to go for, leaves alphabetical order, or
    /// marks the wrong row as chosen.
    #[test]
    fn every_want_is_listed_alphabetically_and_the_chosen_one_is_marked() {
        let mut thought = BotThought::default();
        let health = Candidate { want: Want::Health, goal: Goal::Pickup(0), score: 0.62 };
        thought.chosen = Some(health);
        thought.pulls = vec![(Want::Health, 0.62), (Want::Roam, 0.1)];
        let rows = rows(&thought);
        assert_eq!(rows.len(), Want::ALL.len());
        assert!(rows.windows(2).all(|pair| pair[0].0 <= pair[1].0), "{rows:?}");
        assert!(rows.contains(&(Want::Health.name(), "0.62".to_owned(), true)));
        assert!(rows.contains(&(Want::Roam.name(), "0.10".to_owned(), false)));
        assert!(rows.contains(&(Want::Hunt.name(), get!("bot.pull.none"), false)));
    }
}
