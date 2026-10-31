//! The last [`KEPT_ROUNDS`] rounds' final standings, newest first, for the
//! editor's Rounds tab.
//!
//! On every machine, taken from the replicated [`Standing`]s as Play ends, and
//! kept in step with them until the next round starts, which is when they are
//! cleared. A client is told the round is over on a different channel from
//! the standings, so the final frag can arrive after it has left Play. Not
//! saved and not sent; a client that joins late has only the rounds it saw.

use std::collections::VecDeque;

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::game::score::{ranked, Standing};

pub const KEPT_ROUNDS: usize = 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Round {
    /// Counted from one, over every round this process has recorded.
    pub number: u32,
    /// Wall-clock seconds, so a paused menu still counts.
    pub length: f32,
    /// Winner first.
    pub standings: Vec<Standing>,
}

impl Round {
    /// Minutes and seconds, as the Rounds tab and the log show it.
    pub fn length_text(&self) -> String {
        let seconds = self.length.round() as u32;
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

#[derive(Resource, Debug, Default)]
pub struct RoundHistory {
    rounds: VecDeque<Round>,
    recorded: u32,
    /// Real seconds when the round under way started.
    started: Option<f32>,
    /// The newest round still follows the standings: from its end until the
    /// next round starts.
    settling: bool,
    /// Every round that has ended here, with standings or without.
    ended: u32,
}

impl RoundHistory {
    /// Real seconds since the round under way started, `now` being `Time<Real>`'s; what both a
    /// round's recorded length and its time limit are measured on.
    pub fn elapsed(&self, now: f32) -> Option<f32> {
        self.started.map(|started| now - started)
    }

    #[cfg(test)]
    pub(crate) fn started_at(started: f32) -> Self {
        RoundHistory { started: Some(started), ..default() }
    }

    /// Newest first.
    pub fn rounds(&self) -> impl Iterator<Item = &Round> {
        self.rounds.iter()
    }

    fn record(&mut self, standings: Vec<Standing>, length: f32) {
        self.recorded += 1;
        self.rounds.push_front(Round { number: self.recorded, length, standings });
        self.rounds.truncate(KEPT_ROUNDS);
        self.settling = true;
    }
}

/// `--rounds N`: the process exits once N rounds have ended here.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct QuitAfterRounds(pub Option<u32>);

pub struct RoundHistoryPlugin;

impl Plugin for RoundHistoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoundHistory>()
            .add_systems(OnEnter(AppMode::Play), start_the_clock)
            .add_systems(OnExit(AppMode::Play), record_the_round)
            .init_resource::<QuitAfterRounds>()
            .add_systems(Update, (settle_the_last_round, quit_after_enough_rounds).run_if(not(in_state(AppMode::Play))));
    }
}

fn start_the_clock(time: Res<Time<Real>>, mut history: ResMut<RoundHistory>) {
    history.started = Some(time.elapsed_secs());
    history.settling = false;
}

/// Only on a change, and only to the standings: a round's length is when it
/// ended here.
fn settle_the_last_round(
    mut history: ResMut<RoundHistory>,
    standings: Query<&Standing>,
    changed: Query<(), Changed<Standing>>,
) {
    if !history.settling || changed.is_empty() {
        return;
    }
    let settled: Vec<Standing> = ranked(standings.iter()).into_iter().cloned().collect();
    if let Some(last) = history.rounds.front_mut() {
        last.standings = settled;
    }
}

/// A mode with no scoreboard has no standings, and leaves no round.
fn record_the_round(time: Res<Time<Real>>, mut history: ResMut<RoundHistory>, standings: Query<&Standing>) {
    let length = history.elapsed(time.elapsed_secs()).unwrap_or(0.0);
    history.started = None;
    history.ended += 1;
    if standings.is_empty() {
        return;
    }
    history.record(ranked(standings.iter()).into_iter().cloned().collect(), length);
    if let Some(round) = history.rounds.front() {
        info!("{}", report(round));
    }
}

/// The round's standings as a table for the log, which is where an unattended `--rounds` run's
/// result is read from. On a client the final frag can still be in flight.
pub fn report(round: &Round) -> String {
    let mut report = format!("Round {} over after {}", round.number, round.length_text());
    report.push_str("\n  #  name                 frags  kills  deaths  assists  dealt  taken");
    for (place, standing) in round.standings.iter().enumerate() {
        report.push_str(&format!(
            "\n  {:<2} {:<20} {:>5}  {:>5}  {:>6}  {:>7}  {:>5}  {:>5}",
            place + 1,
            standing.display_name(),
            standing.frags,
            standing.kills,
            standing.deaths,
            standing.assists,
            standing.damage_dealt,
            standing.damage_taken,
        ));
    }
    report
}

/// A frame after the round is recorded, so it has reached the log.
fn quit_after_enough_rounds(quit: Res<QuitAfterRounds>, history: Res<RoundHistory>, mut exit: MessageWriter<AppExit>) {
    if quit.0.is_some_and(|rounds| history.ended >= rounds) {
        info!("{} rounds played, as asked; quitting", history.ended);
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::damage::PlayerId;
    use crate::common::player_color::PlayerColor;

    fn standing(id: u64, frags: i32) -> Standing {
        Standing {
            id: PlayerId(id),
            name: String::new(),
            team: None,
            color: PlayerColor { hue: 0.0 },
            frags,
            kills: 0,
            assists: 0,
            deaths: 0,
            damage_dealt: 0,
            damage_taken: 0,
            healing: 0,
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::time::TimePlugin, bevy::state::app::StatesPlugin))
            .init_state::<AppMode>()
            .add_plugins(RoundHistoryPlugin);
        app.update();
        app
    }

    fn play_a_round(app: &mut App, standings: &[Standing]) {
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        let spawned: Vec<Entity> = standings.iter().map(|standing| app.world_mut().spawn(standing.clone()).id()).collect();
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Editor);
        app.update();
        for entity in spawned {
            app.world_mut().despawn(entity);
        }
    }

    #[test]
    fn a_round_is_kept_ranked_and_newest_first() {
        let mut app = app();
        play_a_round(&mut app, &[standing(1, 3), standing(2, 7)]);
        play_a_round(&mut app, &[standing(1, 5)]);

        let history = app.world().resource::<RoundHistory>();
        let rounds: Vec<&Round> = history.rounds().collect();
        assert_eq!(rounds.len(), 2);
        assert_eq!(rounds[0].number, 2);
        let winners: Vec<PlayerId> = rounds[1].standings.iter().map(|standing| standing.id).collect();
        assert_eq!(winners, vec![PlayerId(2), PlayerId(1)]);
    }

    /// The final frag replicates after the client has already left Play.
    #[test]
    fn a_frag_that_arrives_after_the_round_ends_still_counts() {
        let mut app = app();
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        let winner = app.world_mut().spawn(standing(1, 19)).id();
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Editor);
        app.update();

        app.world_mut().get_mut::<Standing>(winner).unwrap().frags = 20;
        app.update();
        let history = app.world().resource::<RoundHistory>();
        assert_eq!(history.rounds().next().unwrap().standings[0].frags, 20);

        // Not once the next round is under way.
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();
        app.world_mut().get_mut::<Standing>(winner).unwrap().frags = 0;
        app.update();
        let history = app.world().resource::<RoundHistory>();
        assert_eq!(history.rounds().next().unwrap().standings[0].frags, 20);
    }

    #[test]
    fn only_the_last_twenty_are_kept_and_a_round_with_no_scoreboard_is_none() {
        let mut app = app();
        play_a_round(&mut app, &[]);
        for _ in 0..KEPT_ROUNDS + 5 {
            play_a_round(&mut app, &[standing(1, 1)]);
        }

        let history = app.world().resource::<RoundHistory>();
        assert_eq!(history.rounds().count(), KEPT_ROUNDS);
        assert_eq!(history.rounds().next().unwrap().number, KEPT_ROUNDS as u32 + 5);
    }
}
