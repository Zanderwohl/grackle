//! Runs matches back to back. The authority is the one writer of [`MatchPhase`]; a client follows
//! the server's [`ServerPhase`]. See [documentation/match-cycle.md](../../documentation/match-cycle.md).

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::{Damageable, PlayerId};
use crate::common::game_rules::GameRules;
use crate::common::match_phase::{phase_of, shows_the_cameras, Countdown, MatchNumber, MatchPhase, ServerPhase, ShotClock};
use crate::common::net::{has_authority, is_remote_client, NetRole};
use crate::common::round_settings::RoundSettings;
use crate::game::bot::BotSeat;
use crate::game::chat::{ChatEntry, RoundEvent};
use crate::game::condition::Condition;
use crate::game::death::Dying;
use crate::game::net_bodies::RespawnTimers;
use crate::game::player::Player;
use crate::game::respawn::{Identity, OurIdentity};
use crate::game::score::Standing;
use crate::game::top_banner::winning_sides;

/// How long the post-game waits after the last loser is gone.
pub const LAST_FRAG_GRACE: f32 = 2.0;

/// Marks an app that runs the cycle, so the round's clock moves onto [`MatchPhase::Live`].
#[derive(Resource, Default)]
pub struct MatchCycle;

/// Who won the match just ended; only they come back in the post-game. Picked by the authority and
/// stated to clients with the phase.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Winners(pub Vec<PlayerId>);

/// The authority's clocks for the phase under way.
#[derive(Resource, Clone, Copy, Debug, Default)]
struct Cycle {
    /// Seconds of countdown left.
    left: f32,
    /// The countdown last ran with no person playing.
    counting_for_bots: bool,
    /// The match went live with no person playing.
    bot_only: bool,
    post_elapsed: f32,
    losers_gone_for: f32,
}

/// Whether a player is owed a body in `phase`; with no cycle, always.
pub fn owed_a_body(phase: Option<MatchPhase>, winners: Option<&Winners>, id: PlayerId) -> bool {
    match phase {
        None | Some(MatchPhase::Live) => true,
        Some(MatchPhase::PreGame | MatchPhase::BetweenRounds) => false,
        Some(MatchPhase::PostGame) => winners.is_some_and(|winners| winners.0.contains(&id)),
    }
}

/// The most frags win; with teams, every player on the team with the most frags. Ties all win.
/// A player on no team in a team mode is judged alone.
pub fn winners<'a>(standings: impl IntoIterator<Item = &'a Standing>, teams: bool) -> Vec<PlayerId> {
    let standings: Vec<&Standing> = standings.into_iter().collect();
    let score = |standing: &Standing| -> i32 {
        match (teams, standing.team) {
            (true, Some(team)) => standings.iter().filter(|other| other.team == Some(team)).map(|other| other.frags).sum(),
            _ => standing.frags,
        }
    };
    let Some(best) = standings.iter().map(|standing| score(standing)).max() else { return Vec::new() };
    let mut won: Vec<PlayerId> = standings.iter().filter(|standing| score(standing) == best).map(|standing| standing.id).collect();
    won.sort();
    won
}

fn bots_may_play_alone(settings: &RoundSettings, bots: &Query<(), With<BotSeat>>) -> bool {
    settings.bot_only_matches && settings.fill_with_bots && !bots.is_empty()
}

/// Humans whose seat is [`Seat::Playing`]: choosing or spectating does not count, nor do bots.
pub fn people_playing(us: Option<&OurIdentity>, links: &Query<&Identity, (With<ClientOf>, With<Connected>)>) -> usize {
    let playing = |who: &Identity| who.seat.is_playing();
    us.map(|us| &us.0).into_iter().chain(links.iter()).filter(|who| playing(who)).count()
}

pub struct MatchCyclePlugin;

impl Plugin for MatchCyclePlugin {
    fn build(&self, app: &mut App) {
        // After `GamePlugin`, which owns `AppMode`.
        app.add_sub_state::<MatchPhase>()
            .init_resource::<MatchCycle>()
            .init_resource::<MatchNumber>()
            .init_resource::<Countdown>()
            .init_resource::<ServerPhase>()
            .init_resource::<ShotClock>()
            .init_resource::<Winners>()
            .init_resource::<Cycle>()
            .add_message::<ChatEntry>()
            .init_resource::<RoundSettings>()
            .init_resource::<RespawnTimers>()
            .add_systems(
                OnEnter(MatchPhase::PreGame),
                (
                    crate::game::rounds::stop_settling,
                    start_a_new_match,
                    crate::game::reset::reset_for_play,
                    crate::game::score::clear_standings.run_if(has_authority),
                    crate::game::kill_feed::forget_the_feed,
                    crate::game::kill_cam::end_the_kill_cam,
                )
                    .chain(),
            )
            .add_systems(OnEnter(MatchPhase::Live), crate::game::rounds::start_the_clock)
            .add_systems(OnExit(MatchPhase::Live), crate::game::rounds::record_the_round)
            .add_systems(OnEnter(MatchPhase::PostGame), pick_the_winners.run_if(has_authority))
            .add_systems(OnEnter(MatchPhase::BetweenRounds), (clear_the_field, crate::game::kill_cam::end_the_kill_cam).chain())
            .add_systems(
                Update,
                (
                    run_the_countdown.run_if(in_state(MatchPhase::PreGame)),
                    watch_who_is_playing,
                    (humiliate_the_losers, end_the_post_game).chain().run_if(in_state(MatchPhase::PostGame)),
                    run_the_time_between_rounds.run_if(in_state(MatchPhase::BetweenRounds)),
                )
                    .run_if(in_state(AppMode::Play))
                    .run_if(has_authority),
            )
            .add_systems(Update, follow_the_server_phase.run_if(in_state(AppMode::Play)).run_if(is_remote_client))
            .add_systems(Update, advance_the_shot_clock.run_if(|phase: Option<Res<State<MatchPhase>>>| shows_the_cameras(phase_of(&phase))))
            .add_systems(OnExit(AppMode::Play), forget_the_server_phase);
    }
}

/// Everything a match owns goes, on entering the pre-game, whether from Play starting or a match
/// ending. Identities stay: a new match is not a new player.
#[allow(clippy::too_many_arguments)]
fn start_a_new_match(
    mut commands: Commands,
    role: Option<Res<NetRole>>,
    settings: Res<RoundSettings>,
    mut number: ResMut<MatchNumber>,
    mut cycle: ResMut<Cycle>,
    mut countdown: ResMut<Countdown>,
    mut clock: ResMut<ShotClock>,
    mut winners: ResMut<Winners>,
    mut timers: ResMut<RespawnTimers>,
    bodies: Query<Entity, With<Player>>,
    mut chat: MessageWriter<ChatEntry>,
) {
    clock.0 = 0.0;
    commands.remove_resource::<crate::game::player::DeathCam>();
    commands.remove_resource::<crate::game::player::OurLastBody>();
    if !role.as_deref().is_none_or(NetRole::is_authority) {
        return;
    }
    number.0 += 1;
    info!("Match {} is in its pre-game", number.0);
    *cycle = Cycle { left: settings.pre_game_length, ..default() };
    *countdown = Countdown::default();
    winners.0.clear();
    timers.0.clear();
    // Replicated, so a client's go with these despawns.
    for body in &bodies {
        commands.entity(body).despawn();
    }
}

/// Counts down while somebody is playing, or while bots may play alone; holds at the full length
/// otherwise. A person arriving while it counts for bots alone starts it again.
#[allow(clippy::too_many_arguments)]
fn run_the_countdown(
    time: Res<Time>,
    settings: Res<RoundSettings>,
    us: Option<Res<OurIdentity>>,
    links: Query<&Identity, (With<ClientOf>, With<Connected>)>,
    bots: Query<(), With<BotSeat>>,
    mut cycle: ResMut<Cycle>,
    mut countdown: ResMut<Countdown>,
    mut next: ResMut<NextState<MatchPhase>>,
) {
    let people = people_playing(us.as_deref(), &links);
    let ready = people > 0 || bots_may_play_alone(&settings, &bots);
    if !ready {
        cycle.left = settings.pre_game_length;
        cycle.counting_for_bots = false;
        countdown.set_if_neq(Countdown { phase: Some(MatchPhase::PreGame), left: None });
        return;
    }
    if people > 0 && cycle.counting_for_bots {
        info!("Somebody is playing; the countdown starts again");
        cycle.left = settings.pre_game_length;
    }
    cycle.counting_for_bots = people == 0;
    cycle.left -= time.delta_secs();
    countdown.set_if_neq(Countdown { phase: Some(MatchPhase::PreGame), left: Some(cycle.left.max(0.0)) });
    if cycle.left <= 0.0 && matches!(*next, NextState::Unchanged) {
        cycle.bot_only = people == 0;
        info!("The match is live{}", if cycle.bot_only { ", with bots only" } else { "" });
        next.set(MatchPhase::Live);
    }
}

/// A person joining a match of bots alone gets a new match rather than the middle of that one;
/// a match everybody has left waits for players, unless bots may play alone.
fn watch_who_is_playing(
    phase: Option<Res<State<MatchPhase>>>,
    settings: Res<RoundSettings>,
    us: Option<Res<OurIdentity>>,
    links: Query<&Identity, (With<ClientOf>, With<Connected>)>,
    bots: Query<(), With<BotSeat>>,
    cycle: Res<Cycle>,
    history: Option<ResMut<crate::game::rounds::RoundHistory>>,
    mut next: ResMut<NextState<MatchPhase>>,
) {
    let Some(phase) = phase_of(&phase) else { return };
    if phase == MatchPhase::PreGame || matches!(*next, NextState::Pending(_)) {
        return;
    }
    let people = people_playing(us.as_deref(), &links);
    let restart = if cycle.bot_only && people > 0 {
        info!("A person joined a match of bots only; starting a new match");
        true
    } else if phase == MatchPhase::Live && !cycle.bot_only && people == 0 && !bots_may_play_alone(&settings, &bots) {
        info!("Nobody is playing; waiting for players");
        true
    } else {
        false
    };
    if restart {
        // Not a round: it did not finish.
        if let Some(mut history) = history {
            history.abandon();
        }
        next.set(MatchPhase::PreGame);
    }
}

/// A mode with no scoreboard has no winners, so it skips the post-game rather than humiliating
/// everybody.
fn pick_the_winners(
    rules: Res<GameRules>,
    standings: Query<&Standing>,
    mut won: ResMut<Winners>,
    mut cycle: ResMut<Cycle>,
    mut next: ResMut<NextState<MatchPhase>>,
    mut chat: MessageWriter<ChatEntry>,
) {
    if standings.is_empty() {
        won.0.clear();
        next.set(MatchPhase::BetweenRounds);
        return;
    }
    won.0 = winners(standings.iter(), rules.teams);
    let standings: Vec<&Standing> = standings.iter().collect();
    let sides = winning_sides(&won.0, &standings, rules.teams);
    if !sides.is_empty() {
        chat.write(ChatEntry::Round(RoundEvent::Won { sides }));
    }
    cycle.post_elapsed = 0.0;
    cycle.losers_gone_for = 0.0;
    info!("Humiliation: {:?} won", won.0);
}

/// Every frame, so a loser's `H` cannot let go of it and a winner's new body is boosted too.
/// Nothing clears the boost: the bodies go when the post-game ends.
fn humiliate_the_losers(winners: Res<Winners>, mut bodies: Query<(&PlayerId, &mut Condition), With<Player>>) {
    for (id, mut condition) in &mut bodies {
        if winners.0.contains(id) {
            if !condition.crit_boosted {
                condition.crit_boosted = true;
            }
        } else if !condition.humiliated {
            condition.humiliated = true;
        }
    }
}

fn end_the_post_game(
    time: Res<Time>,
    settings: Res<RoundSettings>,
    winners: Res<Winners>,
    bodies: Query<(&PlayerId, &Damageable), (With<Player>, Without<Dying>)>,
    mut cycle: ResMut<Cycle>,
    mut next: ResMut<NextState<MatchPhase>>,
) {
    let dt = time.delta_secs();
    cycle.post_elapsed += dt;
    let a_loser_stands = bodies.iter().any(|(id, health)| !winners.0.contains(id) && health.health() > 0);
    cycle.losers_gone_for = if a_loser_stands { 0.0 } else { cycle.losers_gone_for + dt };
    let done = cycle.losers_gone_for >= LAST_FRAG_GRACE || cycle.post_elapsed >= settings.post_game_length;
    if done && matches!(*next, NextState::Unchanged) {
        next.set(MatchPhase::BetweenRounds);
    }
}

/// Every body goes and the scoreboard stays, on entering the time between rounds.
#[allow(clippy::too_many_arguments)]
fn clear_the_field(
    mut commands: Commands,
    role: Option<Res<NetRole>>,
    settings: Res<RoundSettings>,
    mut cycle: ResMut<Cycle>,
    mut countdown: ResMut<Countdown>,
    mut clock: ResMut<ShotClock>,
    mut timers: ResMut<RespawnTimers>,
    bodies: Query<Entity, With<Player>>,
    mut chat: MessageWriter<ChatEntry>,
) {
    clock.0 = 0.0;
    commands.remove_resource::<crate::game::player::DeathCam>();
    commands.remove_resource::<crate::game::player::OurLastBody>();
    if !role.as_deref().is_none_or(NetRole::is_authority) {
        return;
    }
    info!("Between rounds");
    cycle.left = settings.between_rounds_length;
    let seconds = cycle.left.ceil() as u32;
    if seconds > 0 {
        chat.write(ChatEntry::Round(RoundEvent::NextRoundIn { seconds }));
    }
    *countdown = Countdown { phase: Some(MatchPhase::BetweenRounds), left: Some(cycle.left) };
    timers.0.clear();
    for body in &bodies {
        commands.entity(body).despawn();
    }
}

/// Not shown, but sent like the pre-game's.
fn run_the_time_between_rounds(
    time: Res<Time>,
    mut cycle: ResMut<Cycle>,
    mut countdown: ResMut<Countdown>,
    mut next: ResMut<NextState<MatchPhase>>,
) {
    cycle.left -= time.delta_secs();
    countdown.set_if_neq(Countdown { phase: Some(MatchPhase::BetweenRounds), left: Some(cycle.left.max(0.0)) });
    if cycle.left <= 0.0 && matches!(*next, NextState::Unchanged) {
        next.set(MatchPhase::PreGame);
    }
}

/// A different phase is followed; the same pre-game under a new number is a restart.
fn follow_the_server_phase(
    server: Res<ServerPhase>,
    phase: Res<State<MatchPhase>>,
    mut next: ResMut<NextState<MatchPhase>>,
    mut number: ResMut<MatchNumber>,
    mut winners: ResMut<Winners>,
) {
    let Some(stated) = server.0.as_ref() else { return };
    if winners.0 != stated.winners {
        winners.0.clone_from(&stated.winners);
    }
    let restart = stated.phase == MatchPhase::PreGame && number.0 != stated.number;
    if *phase.get() != stated.phase || restart {
        info!("Server says the match is now {:?}", stated.phase);
        next.set(stated.phase);
    }
    number.set_if_neq(MatchNumber(stated.number));
}

/// What a client heard last Play would otherwise be followed on entering the next.
fn forget_the_server_phase(mut server: ResMut<ServerPhase>, mut countdown: ResMut<Countdown>) {
    server.0 = None;
    *countdown = Countdown::default();
}

fn advance_the_shot_clock(time: Res<Time<Real>>, mut clock: ResMut<ShotClock>) {
    clock.0 += time.delta_secs();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::chat::Winner;
    use crate::get;
    use crate::common::player_color::PlayerColor;
    use crate::common::team::Team;
    use crate::game::respawn::Seat;

    fn standing(id: u64, team: Option<Team>, frags: i32) -> Standing {
        Standing {
            id: PlayerId(id),
            name: String::new(),
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

    fn app(settings: RoundSettings) -> App {
        let mut app = App::new();
        app.add_plugins((bevy::time::TimePlugin, bevy::state::app::StatesPlugin))
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_millis(100)))
            .init_state::<AppMode>()
            .insert_resource(settings)
            .init_resource::<GameRules>()
            .init_resource::<crate::common::skeleton::AnimationClock>()
            .init_resource::<crate::game::player::InputLatch>()
            .init_resource::<crate::game::kill_feed::KillFeed>()
            .init_resource::<crate::game::rounds::RoundHistory>()
            .add_plugins(MatchCyclePlugin);
        app.update();
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        app
    }

    fn phase(app: &App) -> MatchPhase {
        *app.world().resource::<State<MatchPhase>>().get()
    }

    fn run(app: &mut App, seconds: f32) {
        for _ in 0..(seconds * 10.0).ceil() as usize {
            app.update();
        }
    }

    fn person(app: &mut App, seat: Seat) {
        app.world_mut().insert_resource(OurIdentity(Identity { id: PlayerId(1), seat, color: 0, class: None }));
    }

    /// Breaks if the countdown runs for nobody, or bots play alone when they may not.
    #[test]
    fn the_pre_game_waits_for_a_person_unless_bots_may_play_alone() {
        let settings = RoundSettings { pre_game_length: 1.0, fill_with_bots: true, ..default() };
        let mut app = app(settings);
        app.world_mut().spawn(BotSeat);
        person(&mut app, Seat::Spectating);
        run(&mut app, 3.0);
        assert_eq!(phase(&app), MatchPhase::PreGame, "bots played alone");
        assert_eq!(app.world().resource::<Countdown>().left, None);

        app.world_mut().resource_mut::<RoundSettings>().bot_only_matches = true;
        run(&mut app, 2.0);
        assert_eq!(phase(&app), MatchPhase::Live, "bots may play alone, but did not");
    }

    /// Breaks if a person is dropped into a match of bots alone rather than given a new one.
    #[test]
    fn a_person_joining_a_match_of_bots_starts_a_new_match() {
        let settings = RoundSettings { pre_game_length: 1.0, fill_with_bots: true, bot_only_matches: true, ..default() };
        let mut app = app(settings);
        app.world_mut().spawn(BotSeat);
        run(&mut app, 2.0);
        assert_eq!(phase(&app), MatchPhase::Live);
        let number = app.world().resource::<MatchNumber>().0;

        person(&mut app, Seat::Playing(None));
        run(&mut app, 0.3);
        assert_eq!(phase(&app), MatchPhase::PreGame, "the person was dropped into the bots' match");
        assert_eq!(app.world().resource::<MatchNumber>().0, number + 1, "not a new match");
        run(&mut app, 2.0);
        assert_eq!(phase(&app), MatchPhase::Live, "the person's match did not start");
    }

    /// Breaks if the post-game outlasts its losers by more than the grace, keeps humiliating
    /// past its length, or stops boosting the winners' shots to crits.
    #[test]
    fn the_post_game_ends_after_the_last_loser_or_its_length() {
        let settings = RoundSettings { pre_game_length: 0.0, post_game_length: 10.0, ..default() };
        let mut app = app(settings);
        person(&mut app, Seat::Playing(None));
        run(&mut app, 0.5);
        assert_eq!(phase(&app), MatchPhase::Live);
        app.world_mut().spawn(standing(1, None, 5));
        app.world_mut().resource_mut::<NextState<MatchPhase>>().set(MatchPhase::PostGame);
        app.update();
        assert_eq!(app.world().resource::<Winners>().0, vec![PlayerId(1)]);
        let loser = app.world_mut().spawn((Player::default(), PlayerId(2), Condition::default(), Damageable::with_health(100))).id();
        let winner = app.world_mut().spawn((Player::default(), PlayerId(1), Condition::default(), Damageable::with_health(100))).id();
        run(&mut app, 3.0);
        let lost = *app.world().get::<Condition>(loser).unwrap();
        assert!(lost.humiliated && !lost.crit_boosted, "the loser was not humiliated, or was boosted");
        let won = *app.world().get::<Condition>(winner).unwrap();
        assert!(won.crit_boosted && !won.humiliated, "the winner was not crit boosted, or was humiliated");
        app.world_mut().despawn(winner);
        assert_eq!(phase(&app), MatchPhase::PostGame, "ended with a loser standing");

        app.world_mut().despawn(loser);
        run(&mut app, LAST_FRAG_GRACE + 0.3);
        assert_ne!(phase(&app), MatchPhase::PostGame, "outlasted the last loser");
    }

    /// Breaks if the post-game skips the time between rounds, a body or the scoreboard is wrong
    /// through it, or it does not end in a new match.
    #[test]
    fn the_scoreboard_stands_between_rounds_without_bodies_then_a_new_match_starts() {
        let settings = RoundSettings { pre_game_length: 0.0, post_game_length: 0.5, between_rounds_length: 2.0, ..default() };
        let mut app = app(settings);
        person(&mut app, Seat::Playing(None));
        run(&mut app, 0.5);
        assert_eq!(phase(&app), MatchPhase::Live);
        let number = app.world().resource::<MatchNumber>().0;
        let board = app.world_mut().spawn(standing(1, None, 5)).id();
        let winner = app.world_mut().spawn((Player::default(), PlayerId(1), Condition::default(), Damageable::with_health(100))).id();
        app.world_mut().resource_mut::<NextState<MatchPhase>>().set(MatchPhase::PostGame);
        run(&mut app, 1.0);

        assert_eq!(phase(&app), MatchPhase::BetweenRounds, "the post-game did not lead between rounds");
        assert!(app.world().get_entity(winner).is_err(), "a body stood between rounds");
        assert!(app.world().get_entity(board).is_ok(), "the scoreboard went before the next match");
        assert!(app.world().resource::<Countdown>().left.is_some(), "nothing to send between rounds");

        app.world_mut().resource_mut::<RoundSettings>().pre_game_length = 10.0;
        run(&mut app, 2.0);
        assert_eq!(phase(&app), MatchPhase::PreGame, "the time between rounds did not end");
        assert_eq!(app.world().resource::<MatchNumber>().0, number + 1, "not a new match");
    }

    #[derive(Resource, Default)]
    struct Heard(Vec<RoundEvent>);

    fn listen(mut entries: MessageReader<ChatEntry>, mut heard: ResMut<Heard>) {
        heard.0.extend(entries.read().filter_map(|entry| match entry {
            ChatEntry::Round(event) => Some(event.clone()),
            _ => None,
        }));
    }

    /// Breaks if the chat is not told who won as the post-game starts, or how long the wait is
    /// as it ends.
    #[test]
    fn the_chat_hears_who_won_then_how_long_until_the_next_round() {
        let settings = RoundSettings { pre_game_length: 0.0, post_game_length: 0.5, between_rounds_length: 7.5, ..default() };
        let mut app = app(settings);
        app.init_resource::<Heard>().add_systems(Update, listen);
        person(&mut app, Seat::Playing(None));
        run(&mut app, 0.5);
        let mut named = standing(1, None, 5);
        named.name = "Grackle".into();
        app.world_mut().spawn(named.clone());
        app.world_mut().spawn(standing(2, None, 1));
        app.world_mut().resource_mut::<NextState<MatchPhase>>().set(MatchPhase::PostGame);
        run(&mut app, 1.0);
        assert_eq!(phase(&app), MatchPhase::BetweenRounds);
        let heard = &app.world().resource::<Heard>().0;
        let won = RoundEvent::Won { sides: vec![Winner::Player(named.speaker())] };
        assert_eq!(heard.iter().filter(|event| **event == won).count(), 1, "{heard:?}");
        let last = heard.iter().rposition(|event| matches!(event, RoundEvent::NextRoundIn { .. }));
        assert_eq!(last.map(|at| &heard[at]), Some(&RoundEvent::NextRoundIn { seconds: 8 }), "{heard:?}");
        assert!(heard.iter().position(|event| *event == won) < last, "the wait came before the win");
        assert_eq!(won.words(), get!("match.wins", "winner", "Grackle"));
    }

    #[test]
    fn the_most_frags_win_and_ties_share() {
        let standings = [standing(1, None, 4), standing(2, None, 9), standing(3, None, 9)];
        assert_eq!(winners(&standings, false), vec![PlayerId(2), PlayerId(3)]);
        assert!(winners(&[], false).is_empty());
    }

    /// Breaks if a team mode crowns the best player rather than the best team.
    #[test]
    fn with_teams_the_whole_best_team_wins() {
        let standings = [standing(1, Some(Team::Red), 9), standing(2, Some(Team::Blue), 5), standing(3, Some(Team::Blue), 5)];
        assert_eq!(winners(&standings, true), vec![PlayerId(2), PlayerId(3)]);
    }

    #[test]
    fn only_winners_come_back_in_the_post_game_and_nobody_in_the_pre_game() {
        let won = Winners(vec![PlayerId(1)]);
        assert!(owed_a_body(None, None, PlayerId(2)));
        assert!(owed_a_body(Some(MatchPhase::Live), None, PlayerId(2)));
        assert!(!owed_a_body(Some(MatchPhase::PreGame), Some(&won), PlayerId(1)));
        assert!(owed_a_body(Some(MatchPhase::PostGame), Some(&won), PlayerId(1)));
        assert!(!owed_a_body(Some(MatchPhase::PostGame), Some(&won), PlayerId(2)));
        assert!(!owed_a_body(Some(MatchPhase::BetweenRounds), Some(&won), PlayerId(1)));
    }
}
