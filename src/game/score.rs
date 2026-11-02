//! What each player did this round, the frag limit, and the scoreboard. See "Scoring" in
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
use crate::common::damage::{DamageDealt, DamageSource, Died, Healed, PlayerId};
use crate::common::fonts::DisplayFont;
use crate::common::game_rules::GameRules;
use crate::common::net::{has_authority, NetRole};
use crate::common::player_color::PlayerColor;
use crate::common::round_settings::RoundSettings;
use crate::common::player_settings::{settings_of, ClientSettings, PlayerSettings};
use crate::common::team::Team;
use crate::game::body_mesh::body_look;
use crate::game::damage::DamageSystems;
use crate::game::player::{Player, Viewed};
use crate::game::respawn::{HueStart, Identity, OurIdentity};
use crate::game::rounds::RoundHistory;
use crate::get;

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Standing {
    pub id: PlayerId,
    /// As the player set it, already cleaned; empty is unnamed.
    pub name: String,
    pub team: Option<Team>,
    pub color: PlayerColor,
    /// What the frag limit counts. Negative is possible: killing yourself
    /// costs one.
    pub frags: i32,
    /// Other people killed; unlike `frags`, a suicide takes none away.
    pub kills: u32,
    pub assists: u32,
    pub deaths: u32,
    /// To other players, armor included; not to yourself or to a crate.
    pub damage_dealt: u32,
    /// From anything, yourself included.
    pub damage_taken: u32,
    /// Given, to yourself or anybody else.
    pub healing: u32,
}

/// What a round's points are made of. Every machine works them out from the
/// replicated counts, so they are never sent.
pub const POINTS_PER_KILL: i32 = 2;
pub const POINTS_PER_ASSIST: i32 = 1;
pub const POINTS_PER_DEATH: i32 = -1;
pub const HEALING_PER_POINT: u32 = 150;
pub const DAMAGE_PER_POINT: u32 = 300;

impl Standing {
    fn of(who: &Identity, name: &str, start: &HueStart) -> Self {
        Self {
            id: who.id,
            name: name.to_owned(),
            team: who.team(),
            color: who.color(start),
            frags: 0,
            kills: 0,
            assists: 0,
            deaths: 0,
            damage_dealt: 0,
            damage_taken: 0,
            healing: 0,
        }
    }

    /// What players are ranked by. Partial hundreds of damage and healing
    /// count for nothing. Negative for somebody who dies more than they score.
    pub fn points(&self) -> i32 {
        POINTS_PER_KILL * self.kills as i32
            + POINTS_PER_ASSIST * self.assists as i32
            + POINTS_PER_DEATH * self.deaths as i32
            + (self.healing / HEALING_PER_POINT) as i32
            + (self.damage_dealt / DAMAGE_PER_POINT) as i32
    }

    /// What a scoreboard shows: never below zero, though the ranking still
    /// tells a −3 from a −1.
    pub fn shown_points(&self) -> u32 {
        self.points().max(0) as u32
    }

    /// The frags the HUD shows, floored at zero as `shown_points` is.
    pub fn shown_frags(&self) -> u32 {
        self.frags.max(0) as u32
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
                    killer.kills += 1;
                }
            }
            // Yourself, or the map.
            _ => standings[index].frags -= 1,
        }
        if let Some(DamageSource::Player(helper)) = died.assist
            && helper != victim
            && died.killer != DamageSource::Player(helper)
            && let Some(helper) = standings.iter_mut().find(|standing| standing.id == helper)
        {
            helper.assists += 1;
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

/// Most points first, then most kills, then fewest deaths, then by id.
pub fn ranked_by_points<'a>(standings: impl IntoIterator<Item = &'a Standing>) -> Vec<&'a Standing> {
    let mut ranked: Vec<&Standing> = standings.into_iter().collect();
    ranked.sort_by_key(|standing| (Reverse(standing.points()), Reverse(standing.kills), standing.deaths, standing.id));
    ranked
}

pub struct ScorePlugin;

impl Plugin for ScorePlugin {
    fn build(&self, app: &mut App) {
        // Also `DamagePlugin`'s; here too for an app without weapons.
        app.add_message::<Died>()
            .add_message::<DamageDealt>()
            .add_message::<Healed>()
            .add_systems(
                FixedUpdate,
                // Before the reaping, while a hit's target still says who it was.
                (record_damage, record_frags)
                    .after(crate::game::death::announce_the_dead)
                    .before(crate::game::death::reap_the_dead)
                    .in_set(DamageSystems::Resolve),
            )
            .add_systems(Update, record_healing.run_if(in_state(AppMode::Play)).run_if(has_authority))
        .add_systems(
            Update,
            (keep_standings, end_the_round_at_the_frag_limit)
                .chain()
                .run_if(in_state(AppMode::Play))
                .run_if(has_authority),
        )
        .add_systems(OnEnter(AppMode::Play), clear_standings.run_if(has_authority))
        .add_systems(Update, end_the_round_at_the_time_limit.run_if(in_state(AppMode::Play)).run_if(has_authority))
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
    // Nobody choosing a team or watching is in the round.
    let here: Vec<(Identity, &str)> = links
        .iter()
        .map(|(who, link)| (*who, settings_of(false, link, None, &unset).name.as_str()))
        .chain(us.map(|us| (us.0, settings_of(true, None, our_settings.as_deref(), &unset).name.as_str())))
        .filter(|(who, _)| who.seat.is_playing())
        .collect();

    for (entity, mut standing) in &mut standings {
        match here.iter().find(|(who, _)| who.id == standing.id) {
            None => commands.entity(entity).despawn(),
            Some((who, name)) => {
                let color = who.color(&start);
                // Compared first: a written `Standing` is a packet.
                if standing.team != who.team() || standing.color != color || standing.name != *name {
                    standing.team = who.team();
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

fn record_damage(
    mut dealt: MessageReader<DamageDealt>,
    ids: Query<&PlayerId>,
    mut standings: Query<&mut Standing>,
) {
    for hit in dealt.read() {
        if hit.amount == 0 {
            continue;
        }
        let Ok(&victim) = ids.get(hit.target) else { continue };
        for mut standing in &mut standings {
            if standing.id == victim {
                standing.damage_taken += hit.amount;
            } else if hit.source == DamageSource::Player(standing.id) {
                standing.damage_dealt += hit.amount;
            }
        }
    }
}

fn record_healing(mut healed: MessageReader<Healed>, mut standings: Query<&mut Standing>) {
    for heal in healed.read() {
        if let Some(mut standing) = standings.iter_mut().find(|standing| standing.id == heal.healer) {
            standing.healing += heal.amount;
        }
    }
}

/// Back to the editor, through the transition F5 uses, so `match_state`
/// carries everybody with it.
fn end_the_round_at_the_frag_limit(
    rules: Res<GameRules>,
    standings: Query<&Standing>,
    history: Option<ResMut<RoundHistory>>,
    mut next: ResMut<NextState<AppMode>>,
) {
    let Some(limit) = rules.frag_limit else { return };
    let Some(winner) = standings.iter().find(|standing| standing.frags >= limit as i32) else { return };
    if matches!(*next, NextState::Pending(_)) {
        return;
    }
    info!("{} reached {limit} frags; the round is over", winner.id);
    if let Some(mut history) = history {
        history.run_out();
    }
    next.set(AppMode::Editor);
}

/// On the clock [`RoundHistory`] records a round's length by, so a limit of five minutes is a
/// round the Rounds tab says lasted five.
fn end_the_round_at_the_time_limit(
    settings: Option<Res<RoundSettings>>,
    history: Option<ResMut<RoundHistory>>,
    time: Res<Time<Real>>,
    mut next: ResMut<NextState<AppMode>>,
) {
    let Some(limit) = settings.and_then(|settings| settings.time_limit) else { return };
    let Some(mut history) = history else { return };
    let Some(elapsed) = history.elapsed(time.elapsed_secs()) else { return };
    if elapsed < limit || matches!(*next, NextState::Pending(_)) {
        return;
    }
    info!("{limit} seconds are up; the round is over");
    history.run_out();
    next.set(AppMode::Editor);
}

const ROW_TEXT: f32 = 20.0;
const SWATCH: f32 = 14.0;
const MARGIN: f32 = 16.0;

#[derive(Component)]
struct Scoreboard;

/// Up with the round, like the HUD, so a spectator has one too.
fn spawn_the_scoreboard(mut commands: Commands, existing: Query<(), With<Scoreboard>>) {
    if !existing.is_empty() {
        return;
    }
    commands.spawn((
        Scoreboard,
        crate::game::hud::GameHud,
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
/// lit does: ours, or the body a spectator watches.
#[allow(clippy::too_many_arguments)]
fn fill_in_the_scoreboard(
    mut commands: Commands,
    rules: Res<GameRules>,
    font: Res<DisplayFont>,
    standings: Query<&Standing>,
    changed: Query<(), Changed<Standing>>,
    mut removed: RemovedComponents<Standing>,
    us: Query<&PlayerId, (Viewed, With<Player>)>,
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
            text(standing.shown_frags().to_string()),
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
        Standing {
            id: PlayerId(id),
            name: String::new(),
            team: None,
            color: PlayerColor { hue: 0.0 },
            frags,
            kills: 0,
            assists: 0,
            deaths,
            damage_dealt: 0,
            damage_taken: 0,
            healing: 0,
        }
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
        assert_eq!(all[2].shown_frags(), 0);
    }

    #[test]
    fn a_kill_with_help_is_an_assist_and_a_suicide_is_no_kill() {
        let mut death = died(2, DamageSource::Player(PlayerId(0)));
        death.assist = Some(DamageSource::Player(PlayerId(1)));
        let all = after(&[death, died(0, DamageSource::Player(PlayerId(0)))]);
        assert_eq!((all[0].kills, all[0].frags, all[0].assists), (1, 0, 0));
        assert_eq!((all[1].kills, all[1].assists), (0, 1));
    }

    #[test]
    fn damage_counts_given_to_others_and_taken_from_anybody() {
        let mut world = World::new();
        world.init_resource::<Messages<DamageDealt>>();
        for id in 0..2 {
            world.spawn(standing(id, 0, 0));
        }
        let body = |world: &mut World, id: u64| world.spawn(PlayerId(id)).id();
        let (zero, one) = (body(&mut world, 0), body(&mut world, 1));
        let crate_ = world.spawn_empty().id();
        let hit = |target, source, amount| DamageDealt { target, source, amount, remaining: 0, point: Vec3::ZERO, from: None, cause: Default::default() };
        world.write_message(hit(one, DamageSource::Player(PlayerId(0)), 40));
        world.write_message(hit(zero, DamageSource::Player(PlayerId(0)), 15));
        world.write_message(hit(crate_, DamageSource::Player(PlayerId(0)), 99));
        world.write_message(hit(one, DamageSource::World, 5));
        world.run_system_once(record_damage).unwrap();

        let mut all: Vec<Standing> = world.query::<&Standing>().iter(&world).cloned().collect();
        all.sort_by_key(|standing| standing.id);
        assert_eq!((all[0].damage_dealt, all[0].damage_taken), (40, 15));
        assert_eq!((all[1].damage_dealt, all[1].damage_taken), (0, 45));
    }

    #[test]
    fn healing_is_credited_to_the_healer() {
        let mut world = World::new();
        world.init_resource::<Messages<Healed>>();
        world.spawn(standing(0, 0, 0));
        world.write_message(Healed { target: Entity::PLACEHOLDER, healer: PlayerId(0), amount: 75 });
        world.run_system_once(record_healing).unwrap();
        assert_eq!(world.query::<&Standing>().single(&world).unwrap().healing, 75);
    }

    #[test]
    fn points_add_up_and_partial_hundreds_count_for_nothing() {
        let mut someone = standing(0, 0, 2);
        someone.kills = 3;
        someone.assists = 1;
        someone.healing = 299;
        someone.damage_dealt = 650;
        // 6 for kills, 1 assist, 1 for healing, 2 for damage, -2 for deaths.
        assert_eq!(someone.points(), 8);

        let unlucky = standing(1, 0, 5);
        assert_eq!((unlucky.points(), unlucky.shown_points()), (-5, 0));
    }

    #[test]
    fn the_tab_board_ranks_by_points_then_kills() {
        let mut a = standing(0, 0, 0);
        a.kills = 1;
        let mut b = standing(1, 0, 0);
        b.assists = 2;
        let mut c = standing(2, 0, 3);
        c.kills = 3;
        let all = [a, b, c];
        let order: Vec<u64> = ranked_by_points(&all).iter().map(|standing| standing.id.0).collect();
        assert_eq!(order, vec![2, 0, 1]);
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
        let who = |id: u64, color: usize| Identity { id: PlayerId(id), seat: crate::game::respawn::Seat::Playing(None), color };
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

    #[test]
    fn running_out_of_time_ends_the_round() {
        let mut world = World::new();
        world.init_resource::<NextState<AppMode>>();
        world.insert_resource(RoundSettings { time_limit: Some(60.0), ..default() });
        world.insert_resource(Time::<Real>::default());
        world.insert_resource(RoundHistory::started_at(0.0));

        world.resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs(59));
        world.run_system_once(end_the_round_at_the_time_limit).unwrap();
        assert!(matches!(world.resource::<NextState<AppMode>>(), NextState::Unchanged));

        world.resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs(1));
        world.run_system_once(end_the_round_at_the_time_limit).unwrap();
        assert!(matches!(world.resource::<NextState<AppMode>>(), NextState::Pending(AppMode::Editor)));
    }
}
