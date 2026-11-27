//! The plate at the top middle of the [`SafeZone`]: "{winner} wins!" in the winner's color through
//! the post-game, else "Spectating {name}" in the watched body's color while a spectator looks out
//! of a body (not out of a spectator camera). While watching a body, the window's edges are also
//! darkened by a soft vignette.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::PlayerId;
use crate::common::fonts::DisplayFont;
use crate::common::game_rules::GameRules;
use crate::common::match_phase::{phase_of, MatchPhase};
use crate::common::player_color::PlayerColor;
use crate::common::swatch::Swatch;
use crate::common::team::Team;
use crate::game::chat::{Speaker, Winner};
use crate::game::death::Dying;
use crate::game::hud::{SafeZone, MARGIN_Y};
use crate::game::match_cycle::{winners, Winners};
use crate::game::plate::{plate, plate_text, Plate};
use crate::game::player::{ThroughCamera, Watched};
use crate::game::postcards::KeptClear;
use crate::game::score::Standing;
use crate::get;

const TEXT: f32 = 26.0;

/// How far in from each window edge the vignette darkens, in percent of the window's shorter side,
/// so all four strips are one width.
const VIGNETTE_WIDTH: f32 = 10.0;
/// Black's alpha at the very edge.
const VIGNETTE_EDGE: f32 = 1.0;
/// How far below the top of the safe zone the plate's top is, in plate heights.
const DROP: f32 = 1.5;

pub struct TopBannerPlugin;

impl Plugin for TopBannerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DisplayFont>()
            .add_systems(
                Update,
                (put_up_the_banner, (write_the_banner, show_the_vignette)).chain().run_if(in_state(AppMode::Play)),
            )
            .add_systems(OnExit(AppMode::Play), take_down_the_vignette);
    }
}

/// Goes with the HUD: it is the safe zone's child.
#[derive(Component)]
struct TopBanner;

#[derive(Component)]
struct BannerText;

/// A root of its own, not in the HUD, so the Tab scoreboard does not hide it.
#[derive(Component)]
struct Vignette;

#[derive(Clone, Debug, PartialEq)]
pub struct Words {
    pub text: String,
    pub swatch: Swatch,
}

/// One entry per winning team, or per winner on no team.
pub fn winning_sides(won: &[PlayerId], standings: &[&Standing], teams: bool) -> Vec<Winner> {
    let mut sides: Vec<Winner> = Vec::new();
    for &id in won {
        let standing = standings.iter().find(|standing| standing.id == id);
        match standing.and_then(|standing| standing.team).filter(|_| teams) {
            Some(team) if sides.contains(&Winner::Team(team)) => {}
            Some(team) => sides.push(Winner::Team(team)),
            None => sides.push(Winner::Player(match standing {
                Some(standing) => standing.speaker(),
                None => Speaker { id, name: String::new(), team: None, color: None },
            })),
        }
    }
    sides
}

/// "{winner} wins!" in its color, or "{a} and {b} tie!" in neither's.
pub fn win_words(won: &[PlayerId], standings: &[&Standing], teams: bool) -> Option<Words> {
    let sides = winning_sides(won, standings, teams);
    let text = Winner::words(&sides)?;
    let swatch = match sides.as_slice() {
        [side] => side.swatch(),
        _ => Swatch::neutral(),
    };
    Some(Words { text, swatch })
}

fn put_up_the_banner(
    mut commands: Commands,
    font: Res<DisplayFont>,
    zone: Option<Single<Entity, With<SafeZone>>>,
    banners: Query<(), With<TopBanner>>,
    vignettes: Query<(), With<Vignette>>,
) {
    if let Some(zone) = zone
        && banners.is_empty()
    {
        commands.entity(*zone).with_child((
            TopBanner,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(MARGIN_Y),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            Visibility::Hidden,
            Pickable::IGNORE,
            Name::new("Top banner"),
            children![(KeptClear, plate(Swatch::neutral()), children![(BannerText, plate_text(&font.0, String::new(), TEXT))])],
        ));
    }
    if vignettes.is_empty() {
        commands.spawn(vignette());
    }
}

/// What the banner says this frame: the post-game's winner over whoever is watched.
#[allow(clippy::type_complexity)]
fn banner_words(
    phase: Option<MatchPhase>,
    won: &Winners,
    rules: &GameRules,
    standings: &[&Standing],
    watched: Option<(&PlayerId, Option<&Team>, Option<&PlayerColor>)>,
    through_camera: bool,
) -> Option<Words> {
    if phase == Some(MatchPhase::PostGame) {
        // A client is told the authority's pick with the phase; empty until it arrives.
        let picked = if won.0.is_empty() { winners(standings.iter().copied(), rules.teams) } else { won.0.clone() };
        if let Some(words) = win_words(&picked, standings, rules.teams) {
            return Some(words);
        }
    }
    let (&id, team, color) = watched.filter(|_| !through_camera)?;
    let name = standings
        .iter()
        .find(|standing| standing.id == id)
        .map_or_else(|| get!("scoreboard.player", "id", id.0), |standing| standing.display_name());
    Some(Words { text: get!("match.spectating", "name", name), swatch: Swatch::of_body(team.copied(), color.copied()) })
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn write_the_banner(
    phase: Option<Res<State<MatchPhase>>>,
    won: Option<Res<Winners>>,
    rules: Option<Res<GameRules>>,
    standings: Query<&Standing>,
    watched: Option<Single<(&PlayerId, Option<&Team>, Option<&PlayerColor>), (With<Watched>, Without<Dying>)>>,
    through_camera: Option<Res<ThroughCamera>>,
    mut plate: Query<(&mut Plate, &ComputedNode)>,
    mut text: Query<(&mut Text, &ChildOf), With<BannerText>>,
    mut banner: Query<(&mut Visibility, &mut Node), With<TopBanner>>,
) {
    let standings: Vec<&Standing> = standings.iter().collect();
    let words = banner_words(
        phase_of(&phase),
        won.as_deref().unwrap_or(&Winners::default()),
        rules.as_deref().unwrap_or(&GameRules::default()),
        &standings,
        watched.as_deref().map(|(id, team, color)| (*id, *team, *color)),
        through_camera.is_some(),
    );
    let mut ready = false;
    let mut height = None;
    if let Some(words) = &words {
        for (mut text, parent) in &mut text {
            let Ok((mut plate, computed)) = plate.get_mut(parent.parent()) else { continue };
            if plate.swatch != words.swatch {
                plate.swatch = words.swatch;
            }
            // New words are laid out later this frame, so the panel would be the old width.
            let rewritten = text.0 != words.text;
            if rewritten {
                words.text.clone_into(&mut text.0);
            }
            ready = !rewritten && computed.size() != Vec2::ZERO;
            height = Some(computed.size().y * computed.inverse_scale_factor());
        }
    }
    for (mut visibility, mut node) in &mut banner {
        visibility.set_if_neq(if ready { Visibility::Inherited } else { Visibility::Hidden });
        // Clear of the kill feed's first line, which on a narrow window is beside it.
        if let Some(height) = height {
            let top = Val::Px((MARGIN_Y + DROP * height).round());
            if node.top != top {
                node.top = top;
            }
        }
    }
}

/// Black fading in from each window edge. The corners, under two strips, are darker.
fn vignette() -> impl Bundle {
    let fade = |angle: f32| {
        let stops = vec![
            ColorStop::new(Color::srgba(0.0, 0.0, 0.0, VIGNETTE_EDGE), Val::Percent(0.0)),
            ColorStop::new(Color::srgba(0.0, 0.0, 0.0, VIGNETTE_EDGE * 0.3), Val::Percent(50.0)),
            ColorStop::new(Color::srgba(0.0, 0.0, 0.0, 0.0), Val::Percent(100.0)),
        ];
        BackgroundGradient::from(LinearGradient::new(angle, stops))
    };
    let strip = |node: Node, angle: f32| (Node { position_type: PositionType::Absolute, ..node }, fade(angle), Pickable::IGNORE);
    let across = |top: Option<f32>, bottom: Option<f32>| Node {
        left: Val::Px(0.0),
        right: Val::Px(0.0),
        top: top.map_or(Val::Auto, Val::Px),
        bottom: bottom.map_or(Val::Auto, Val::Px),
        height: Val::VMin(VIGNETTE_WIDTH),
        ..default()
    };
    let down = |left: Option<f32>, right: Option<f32>| Node {
        top: Val::Px(0.0),
        bottom: Val::Px(0.0),
        left: left.map_or(Val::Auto, Val::Px),
        right: right.map_or(Val::Auto, Val::Px),
        width: Val::VMin(VIGNETTE_WIDTH),
        ..default()
    };
    (
        Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        Vignette,
        // Under the HUD and everything else.
        GlobalZIndex(-1),
        Visibility::Hidden,
        Pickable::IGNORE,
        Name::new("Spectating vignette"),
        children![
            strip(across(Some(0.0), None), LinearGradient::TO_BOTTOM),
            strip(across(None, Some(0.0)), LinearGradient::TO_TOP),
            strip(down(Some(0.0), None), LinearGradient::TO_RIGHT),
            strip(down(None, Some(0.0)), LinearGradient::TO_LEFT),
        ],
    )
}

fn show_the_vignette(
    watched: Query<(), (With<Watched>, Without<Dying>)>,
    through_camera: Option<Res<ThroughCamera>>,
    mut vignettes: Query<&mut Visibility, With<Vignette>>,
) {
    let watching = !watched.is_empty() && through_camera.is_none();
    for mut visibility in &mut vignettes {
        visibility.set_if_neq(if watching { Visibility::Inherited } else { Visibility::Hidden });
    }
}

fn take_down_the_vignette(mut commands: Commands, vignettes: Query<Entity, With<Vignette>>) {
    for vignette in &vignettes {
        commands.entity(vignette).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing(id: u64, name: &str, team: Option<Team>, frags: i32) -> Standing {
        Standing {
            id: PlayerId(id),
            name: name.into(),
            team,
            color: PlayerColor { hue: 0.0 },
            frags,
            kills: 0,
            assists: 0,
            deaths: 0,
            damage_dealt: 0,
            damage_taken: 0,
            healing: 0,
            streak: 0,
            kill_times: Vec::new(),
        }
    }

    /// Breaks if a free-for-all winner is not named in their own color, or a team win names a
    /// player rather than the team.
    #[test]
    fn the_winner_is_a_player_alone_and_a_team_with_teams() {
        let alone = standing(1, "Grackle", None, 5);
        let other = standing(2, "Crow", None, 2);
        let words = win_words(&[PlayerId(1)], &[&alone, &other], false).unwrap();
        assert_eq!(words.text, get!("match.wins", "winner", "Grackle"));
        assert_eq!(words.swatch, alone.hud_swatch());

        let red = [standing(1, "Grackle", Some(Team::Red), 3), standing(2, "Crow", Some(Team::Red), 2)];
        let blue = standing(3, "Jay", Some(Team::Blue), 4);
        let everybody = [&red[0], &red[1], &blue];
        let words = win_words(&[PlayerId(1), PlayerId(2)], &everybody, true).unwrap();
        assert_eq!(words.text, get!("match.wins", "winner", Team::Red.name()));
        assert_eq!(words.swatch, Swatch::team(Team::Red));
    }

    /// Breaks if a tie is not said as one, in neither side's color.
    #[test]
    fn a_tie_names_everybody_who_tied() {
        let a = standing(1, "Grackle", None, 3);
        let b = standing(2, "", None, 3);
        let words = win_words(&[PlayerId(1), PlayerId(2)], &[&a, &b], false).unwrap();
        let names = [a.display_name(), b.display_name()].join(&get!("match.and"));
        assert_eq!(words.text, get!("match.tie", "winners", names));
        assert_eq!(words.swatch, Swatch::neutral());
        assert_eq!(win_words(&[], &[&a, &b], false), None);
    }

    /// Breaks if the banner names a watched body while looking through a spectator camera, or the
    /// post-game's winner does not take the place of whoever is watched.
    #[test]
    fn the_banner_says_spectating_only_out_of_a_body_and_the_winner_first() {
        let a = standing(1, "Grackle", None, 3);
        let b = standing(2, "Crow", None, 1);
        let standings = [&a, &b];
        let rules = GameRules::default();
        let watched = Some((&PlayerId(2), None, None));
        let live = banner_words(Some(MatchPhase::Live), &Winners::default(), &rules, &standings, watched, false).unwrap();
        assert_eq!(live.text, get!("match.spectating", "name", "Crow"));
        assert_eq!(banner_words(Some(MatchPhase::Live), &Winners::default(), &rules, &standings, watched, true), None);
        let post = banner_words(Some(MatchPhase::PostGame), &Winners::default(), &rules, &standings, watched, false).unwrap();
        assert_eq!(post.text, get!("match.wins", "winner", "Grackle"));
    }
}

