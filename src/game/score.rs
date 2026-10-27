//! Frags and deaths, the frag limit, and the scoreboard. See "Scoring" in
//! [documentation/arena.md](../../documentation/arena.md).
//!
//! A score belongs to the player, not the body, so it lives on a
//! [`Standing`] entity of its own: one per identity, kept in step with who is
//! here by [`keep_standings`] on the authority and replicated to everybody.
//! Cleared on entering Play rather than leaving it, so the last round's
//! standings are still there in the editor.

use std::cmp::Reverse;

use bevy::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::damage::{DamageSource, Died, PlayerId};
use crate::common::fonts::DisplayFont;
use crate::common::game_rules::GameRules;
use crate::common::net::{has_authority, NetRole};
use crate::common::player_color::PlayerColor;
use crate::common::player_settings::{settings_of, ClientSettings, PlayerSettings};
use crate::common::team::Team;
use crate::game::body_mesh::body_look;
use crate::game::damage::DamageSystems;
use crate::game::player::LocalPlayer;
use crate::game::respawn::{HueStart, Identity, OurIdentity};
use crate::get;

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Standing {
    pub id: PlayerId,
    /// As the player set it, already cleaned; empty is unnamed.
    pub name: String,
    pub team: Option<Team>,
    pub color: PlayerColor,
    /// Negative is possible: killing yourself costs one.
    pub frags: i32,
    pub deaths: u32,
}

impl Standing {
    fn of(who: &Identity, name: &str, start: &HueStart) -> Self {
        Self { id: who.id, name: name.to_owned(), team: who.team, color: who.color(start), frags: 0, deaths: 0 }
    }

    /// What a row says: the name, or "Player {id}" for somebody unnamed.
    pub fn display_name(&self) -> String {
        match self.name.is_empty() {
            true => get!("scoreboard.player", "id", self.id.0),
            false => self.name.clone(),
        }
    }

    /// The same rule a body is drawn by, so a row matches its body.
    pub fn swatch(&self) -> Color {
        body_look(self.team.as_ref(), Some(&self.color), None, None).color
    }

    /// What one death does to the standings.
    fn count(standings: &mut [Mut<Standing>], died: &Died) {
        let Some(victim) = died.victim_id else { return };
        // A body with no standing (a grid's dummy) is nobody's frag.
        let Some(index) = standings.iter().position(|standing| standing.id == victim) else { return };
        standings[index].deaths += 1;
        match died.killer {
            DamageSource::Player(killer) if killer != victim => {
                if let Some(killer) = standings.iter_mut().find(|standing| standing.id == killer) {
                    killer.frags += 1;
                }
            }
            // Yourself, or the map.
            _ => standings[index].frags -= 1,
        }
    }
}

/// Most frags first, then fewest deaths, then by id, so every machine draws
/// the same order.
pub fn ranked<'a>(standings: impl IntoIterator<Item = &'a Standing>) -> Vec<&'a Standing> {
    let mut ranked: Vec<&Standing> = standings.into_iter().collect();
    ranked.sort_by_key(|standing| (Reverse(standing.frags), standing.deaths, standing.id));
    ranked
}

pub struct ScorePlugin;

impl Plugin for ScorePlugin {
    fn build(&self, app: &mut App) {
        // Also `DamagePlugin`'s; here too for an app without weapons.
        app.add_message::<Died>().add_systems(
            FixedUpdate,
            record_frags
                .after(crate::game::death::announce_the_dead)
                .before(crate::game::death::reap_the_dead)
                .in_set(DamageSystems::Resolve),
        )
        .add_systems(
            Update,
            (keep_standings, end_the_round_at_the_frag_limit)
                .chain()
                .run_if(in_state(AppMode::Play))
                .run_if(has_authority),
        )
        .add_systems(OnEnter(AppMode::Play), clear_standings.run_if(has_authority))
        .add_systems(
            Update,
            (spawn_the_scoreboard, fill_in_the_scoreboard).chain().run_if(in_state(AppMode::Play)),
        )
        .add_systems(OnExit(AppMode::Play), despawn_the_scoreboard);
    }
}

fn clear_standings(mut commands: Commands, standings: Query<Entity, With<Standing>>) {
    for standing in &standings {
        commands.entity(standing).despawn();
    }
}

/// One standing per identity, keyed on absence like the bodies are: a player
/// with none gets one, a standing with nobody behind it goes.
fn keep_standings(
    mut commands: Commands,
    role: Option<Res<NetRole>>,
    us: Option<Res<OurIdentity>>,
    our_settings: Option<Res<PlayerSettings>>,
    links: Query<(&Identity, Option<&ClientSettings>)>,
    start: Option<Res<HueStart>>,
    mut standings: Query<(Entity, &mut Standing)>,
) {
    // Made in the same command flush as the first identity.
    let Some(start) = start else { return };
    let unset = PlayerSettings::default();
    let here: Vec<(Identity, &str)> = links
        .iter()
        .map(|(who, link)| (*who, settings_of(false, link, None, &unset).name.as_str()))
        .chain(us.map(|us| (us.0, settings_of(true, None, our_settings.as_deref(), &unset).name.as_str())))
        .collect();

    for (entity, mut standing) in &mut standings {
        match here.iter().find(|(who, _)| who.id == standing.id) {
            None => commands.entity(entity).despawn(),
            Some((who, name)) => {
                let color = who.color(&start);
                // Compared first: a written `Standing` is a packet.
                if standing.team != who.team || standing.color != color || standing.name != *name {
                    standing.team = who.team;
                    standing.color = color;
                    standing.name = (*name).to_owned();
                }
            }
        }
    }

    let hosting = matches!(role.as_deref(), Some(NetRole::Listen { .. }));
    for (who, name) in &here {
        if standings.iter().any(|(_, standing)| standing.id == who.id) {
            continue;
        }
        let mut standing = commands.spawn((Standing::of(who, name, &start), Name::new("Standing")));
        if hosting {
            standing.insert(Replicate::to_clients(NetworkTarget::All));
        }
    }
}

fn record_frags(mut died: MessageReader<Died>, mut standings: Query<&mut Standing>) {
    let mut standings: Vec<Mut<Standing>> = standings.iter_mut().collect();
    for died in died.read() {
        Standing::count(&mut standings, died);
    }
}

/// Back to the editor, through the transition F5 uses, so `match_state`
/// carries everybody with it.
fn end_the_round_at_the_frag_limit(
    rules: Res<GameRules>,
    standings: Query<&Standing>,
    mut next: ResMut<NextState<AppMode>>,
) {
    let Some(limit) = rules.frag_limit else { return };
    let Some(winner) = standings.iter().find(|standing| standing.frags >= limit as i32) else { return };
    if matches!(*next, NextState::Pending(_)) {
        return;
    }
    info!("{} reached {limit} frags; the round is over", winner.id);
    next.set(AppMode::Editor);
}

const ROW_TEXT: f32 = 20.0;
const SWATCH: f32 = 14.0;
const MARGIN: f32 = 16.0;

#[derive(Component)]
struct Scoreboard;

/// Keyed on `Added<LocalPlayer>` like the HUD, because a client's body
/// arrives from the server.
fn spawn_the_scoreboard(
    mut commands: Commands,
    bodies: Query<Entity, Added<LocalPlayer>>,
    existing: Query<(), With<Scoreboard>>,
) {
    if bodies.is_empty() || !existing.is_empty() {
        return;
    }
    commands.spawn((
        Scoreboard,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(MARGIN),
            top: Val::Px(MARGIN),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            padding: UiRect::all(Val::Px(6.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
        Pickable::IGNORE,
        Name::new("Scoreboard"),
    ));
}

/// Rebuilt only when a standing changes, comes or goes, or when whose row is
/// ours does.
#[allow(clippy::too_many_arguments)]
fn fill_in_the_scoreboard(
    mut commands: Commands,
    rules: Res<GameRules>,
    font: Res<DisplayFont>,
    standings: Query<&Standing>,
    changed: Query<(), Changed<Standing>>,
    mut removed: RemovedComponents<Standing>,
    us: Query<&PlayerId, With<LocalPlayer>>,
    mut boards: Query<(Entity, &mut Node, Option<&Children>), With<Scoreboard>>,
    mut drawn_for: Local<Option<PlayerId>>,
) {
    let Ok((board, mut node, rows)) = boards.single_mut() else { return };

    let display = if rules.frag_limit.is_some() { Display::Flex } else { Display::None };
    if node.display != display {
        node.display = display;
    }

    let ours = us.single().ok().copied();
    let fresh = rows.is_none_or(|rows| rows.is_empty()) && !standings.is_empty();
    let stale = removed.read().count() > 0 || !changed.is_empty() || *drawn_for != ours;
    if !fresh && !stale {
        return;
    }
    *drawn_for = ours;

    if let Some(rows) = rows {
        for row in rows {
            commands.entity(*row).despawn();
        }
    }
    commands.entity(board).with_children(|board| {
        for standing in ranked(standings.iter()) {
            board.spawn(row(standing, Some(standing.id) == ours, &font.0));
        }
    });
}

fn row(standing: &Standing, ours: bool, font: &Handle<Font>) -> impl Bundle {
    let text = |value: String| {
        (
            Text::new(value),
            TextFont { font: font.into(), font_size: bevy::text::FontSize::Px(ROW_TEXT), ..default() },
            TextColor(Color::WHITE),
            Pickable::IGNORE,
        )
    };
    (
        Node {
            column_gap: Val::Px(8.0),
            align_items: AlignItems::Center,
            padding: UiRect::horizontal(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(if ours { Color::srgba(1.0, 1.0, 1.0, 0.18) } else { Color::NONE }),
        Pickable::IGNORE,
        children![
            (
                Node { width: Val::Px(SWATCH), height: Val::Px(SWATCH), ..default() },
                BackgroundColor(standing.swatch()),
                Pickable::IGNORE,
            ),
            (
                Node { min_width: Val::Px(120.0), ..default() },
                Pickable::IGNORE,
                children![text(standing.display_name())],
            ),
            text(standing.frags.to_string()),
        ],
    )
}

fn despawn_the_scoreboard(mut commands: Commands, boards: Query<Entity, With<Scoreboard>>) {
    for board in &boards {
        commands.entity(board).despawn();
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn standing(id: u64, frags: i32, deaths: u32) -> Standing {
        Standing { id: PlayerId(id), name: String::new(), team: None, color: PlayerColor { hue: 0.0 }, frags, deaths }
    }

    fn died(victim: u64, killer: DamageSource) -> Died {
        Died {
            victim: Entity::PLACEHOLDER,
            victim_name: None,
            victim_id: Some(PlayerId(victim)),
            killer,
            assist: None,
            at: Vec3::ZERO,
        }
    }

    fn after(deaths: &[Died]) -> Vec<Standing> {
        let mut world = World::new();
        world.init_resource::<Messages<Died>>();
        for id in 0..3 {
            world.spawn(standing(id, 0, 0));
        }
        for death in deaths {
            world.write_message(death.clone());
        }
        world.run_system_once(record_frags).unwrap();
        let mut all: Vec<Standing> = world.query::<&Standing>().iter(&world).cloned().collect();
        all.sort_by_key(|standing| standing.id);
        all
    }

    #[test]
    fn a_kill_is_a_frag_to_the_killer_and_a_death_to_the_victim() {
        let all = after(&[died(1, DamageSource::Player(PlayerId(0)))]);
        assert_eq!((all[0].frags, all[0].deaths), (1, 0));
        assert_eq!((all[1].frags, all[1].deaths), (0, 1));
    }

    #[test]
    fn killing_yourself_or_dying_to_the_map_costs_a_frag() {
        let all = after(&[died(2, DamageSource::Player(PlayerId(2))), died(2, DamageSource::World)]);
        assert_eq!((all[2].frags, all[2].deaths), (-2, 2));
    }

    #[test]
    fn a_body_with_no_standing_is_nobodys_frag() {
        let all = after(&[died(40, DamageSource::Player(PlayerId(0)))]);
        assert!(all.iter().all(|standing| standing.frags == 0 && standing.deaths == 0));
    }

    #[test]
    fn the_board_ranks_by_frags_then_fewest_deaths_then_id() {
        let all = [standing(3, 2, 5), standing(1, 4, 1), standing(2, 2, 1), standing(0, 2, 1)];
        let order: Vec<u64> = ranked(&all).iter().map(|standing| standing.id.0).collect();
        assert_eq!(order, vec![1, 0, 2, 3]);
    }

    #[test]
    fn every_player_gets_one_standing_and_a_leaver_loses_theirs() {
        let mut world = World::new();
        world.insert_resource(HueStart(0.0));
        let who = |id: u64, color: usize| Identity { id: PlayerId(id), team: None, color };
        world.insert_resource(OurIdentity(who(0, 0)));
        let client = world.spawn(who(1, 1)).id();

        for _ in 0..2 {
            world.run_system_once(keep_standings).unwrap();
            world.flush();
        }
        let ids = |world: &mut World| {
            let mut ids: Vec<u64> = world.query::<&Standing>().iter(world).map(|standing| standing.id.0).collect();
            ids.sort();
            ids
        };
        assert_eq!(ids(&mut world), vec![0, 1]);

        world.despawn(client);
        world.run_system_once(keep_standings).unwrap();
        world.flush();
        assert_eq!(ids(&mut world), vec![0]);
    }

    #[test]
    fn reaching_the_frag_limit_ends_the_round() {
        let mut world = World::new();
        world.init_resource::<NextState<AppMode>>();
        world.insert_resource(GameRules(crate::common::game_rules::ModeRules { frag_limit: Some(3), ..default() }));
        let leader = world.spawn(standing(0, 2, 0)).id();

        world.run_system_once(end_the_round_at_the_frag_limit).unwrap();
        assert!(matches!(world.resource::<NextState<AppMode>>(), NextState::Unchanged));

        world.get_mut::<Standing>(leader).unwrap().frags = 3;
        world.run_system_once(end_the_round_at_the_frag_limit).unwrap();
        assert!(matches!(world.resource::<NextState<AppMode>>(), NextState::Pending(AppMode::Editor)));
    }
}
