//! Choosing a team. Everybody enters a match [`Seat::Choosing`], with no
//! body, and is offered the teams they may join; the authority seats them.
//! The picker that asks is [`crate::game::team_picker`].
//!
//! - A team may be joined if, once joined, it outnumbers no other team by
//!   more than [`MOST_AHEAD`].
//! - Random is the team with the fewest players, then the fewest points, then
//!   a coin. A mode without teams offers only Random, as "Free for All".
//! - Changing seat with a body kills it, credited to yourself, so it costs a
//!   point and the respawn timer starts on the new team.
//!
//! Offers are per player: leaving a team is counted before joining one.

use std::collections::HashMap;

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::*;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::damage::{DamageSource, Kill, PlayerId};
use crate::common::game_rules::GameRules;
use crate::common::net::{has_authority, NetRole};
use crate::common::team::{Side, Team};
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::death::Dying;
use crate::game::player::Player;
use crate::game::respawn::{team_name, Identity, OurIdentity, Seat};
use crate::game::score::Standing;

/// Players a team may be ahead of any other by, once joined.
pub const MOST_AHEAD: usize = 2;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamChoice {
    Random,
    Join(Team),
    /// Offered and never open: there is nothing to watch from yet.
    Spectate,
}

/// Written by the picker on every machine; the authority answers its own and
/// a client's goes over the wire.
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChooseTeam {
    pub choice: TeamChoice,
    /// A request from a seat already left is dropped, so a second click
    /// before the answer cannot become a switch that kills you.
    pub from: Seat,
}

/// What this machine's player may choose. On the authority it is worked out
/// here; on a client it is the server's latest offer, sent whenever it changes.
#[derive(Resource, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct TeamOptions {
    pub seat: Seat,
    /// `false` is free for all, where Random is the only way in.
    pub teams: bool,
    /// Every team in play, and whether it may be joined now.
    pub open: Vec<(Team, bool)>,
    pub random: bool,
}

/// One team's numbers, not counting whoever is choosing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TeamTally {
    pub team: Team,
    pub players: usize,
    pub points: i32,
}

/// The sides the map's spawn points are on, in [`Team::ALL`] order; Red and
/// Blue on a map with fewer than two.
pub fn teams_in_play(sides: impl IntoIterator<Item = Option<Team>>) -> Vec<Team> {
    let sides: Vec<Team> = sides.into_iter().flatten().collect();
    let teams: Vec<Team> = Team::ALL.into_iter().filter(|team| sides.contains(team)).collect();
    match teams.len() {
        0 | 1 => vec![Team::Red, Team::Blue],
        _ => teams,
    }
}

/// `others` is everybody else's team and points.
pub fn tally(teams: &[Team], others: impl IntoIterator<Item = (Option<Team>, i32)>) -> Vec<TeamTally> {
    let mut tallies: Vec<TeamTally> =
        teams.iter().map(|&team| TeamTally { team, players: 0, points: 0 }).collect();
    for (team, points) in others {
        if let Some(tally) = tallies.iter_mut().find(|tally| Some(tally.team) == team) {
            tally.players += 1;
            tally.points += points;
        }
    }
    tallies
}

pub fn may_join(team: Team, tallies: &[TeamTally]) -> bool {
    let Some(joined) = tallies.iter().find(|tally| tally.team == team) else { return false };
    let once_joined = joined.players + 1;
    tallies.iter().all(|other| once_joined <= other.players + MOST_AHEAD)
}

pub fn random_team(candidates: &[TeamTally], rng: &mut impl rand::Rng) -> Option<Team> {
    let fewest = candidates.iter().map(|tally| (tally.players, tally.points)).min()?;
    let tied: Vec<Team> = candidates
        .iter()
        .filter(|tally| (tally.players, tally.points) == fewest)
        .map(|tally| tally.team)
        .collect();
    tied.choose(rng).copied()
}

/// Your own team is never open, and Random never lands on it.
pub fn options(seat: Seat, teams: bool, tallies: &[TeamTally]) -> TeamOptions {
    if !teams {
        return TeamOptions { seat, teams, open: Vec::new(), random: seat == Seat::Choosing };
    }
    let current = seat.team();
    let open: Vec<(Team, bool)> = tallies
        .iter()
        .map(|tally| (tally.team, Some(tally.team) != current && may_join(tally.team, tallies)))
        .collect();
    let random = open.iter().any(|(_, open)| *open);
    TeamOptions { seat, teams, open, random }
}

/// Where a choice puts somebody, by the same offer they were shown.
pub fn decide(
    choice: TeamChoice,
    offered: &TeamOptions,
    tallies: &[TeamTally],
    rng: &mut impl rand::Rng,
) -> Result<Seat, &'static str> {
    match choice {
        TeamChoice::Spectate => Err("spectating is not built yet"),
        TeamChoice::Random if !offered.random => Err("there is nowhere else to go"),
        TeamChoice::Random if !offered.teams => Ok(Seat::Playing(None)),
        TeamChoice::Random => {
            let candidates: Vec<TeamTally> = tallies
                .iter()
                .copied()
                .filter(|tally| offered.open.contains(&(tally.team, true)))
                .collect();
            random_team(&candidates, rng).map(|team| Seat::Playing(Some(team))).ok_or("there is nowhere else to go")
        }
        TeamChoice::Join(team) if offered.open.contains(&(team, true)) => Ok(Seat::Playing(Some(team))),
        TeamChoice::Join(_) => Err("that team is not open"),
    }
}

pub struct TeamChoicePlugin;

impl Plugin for TeamChoicePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ChooseTeam>()
            .add_message::<Kill>()
            .init_resource::<TeamOptions>()
            .add_systems(
                Update,
                // Before the bodies, so a first choice is stood up that frame.
                // Ungated by Play so requests outside it are drained, not
                // answered next round.
                (seat_whoever_chose, offer_teams.run_if(in_state(AppMode::Play)).run_if(offers_may_have_changed))
                    .chain()
                    .before(crate::game::net_bodies::give_bodies_to_whoever_needs_one)
                    .run_if(has_authority),
            );
    }
}

/// This machine's player, or a client's link.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Chair {
    Ours,
    Link(Entity),
}

fn seated<'a>(us: Option<&OurIdentity>, links: impl Iterator<Item = (Entity, &'a Identity)>) -> Vec<(Chair, Identity)> {
    links
        .map(|(link, who)| (Chair::Link(link), *who))
        .chain(us.map(|us| (Chair::Ours, us.0)))
        .collect()
}

fn tallies_without(teams: &[Team], here: &[(Chair, Identity)], who: PlayerId, points: &HashMap<PlayerId, i32>) -> Vec<TeamTally> {
    let others = here
        .iter()
        .filter(|(_, other)| other.id != who)
        .map(|(_, other)| (other.team(), points.get(&other.id).copied().unwrap_or(0)));
    tally(teams, others)
}

/// One request at a time, each against the seats the last left, so two joins
/// in one frame cannot both fill the same team.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn seat_whoever_chose(
    mode: Res<State<AppMode>>,
    mut ours: MessageReader<ChooseTeam>,
    mut us: Option<ResMut<OurIdentity>>,
    mut links: Query<(Entity, &mut Identity), With<ClientOf>>,
    mut receivers: Query<(Entity, &mut MessageReceiver<ChooseTeam>), With<ClientOf>>,
    rules: Res<GameRules>,
    spawns: Query<Option<&Side>, With<SpawnPointMarker>>,
    standings: Query<&Standing>,
    bodies: Query<(Entity, &PlayerId, &GlobalTransform), (With<Player>, Without<Dying>)>,
    mut kills: MessageWriter<Kill>,
) {
    let mut requests: Vec<(Chair, ChooseTeam)> = ours.read().map(|request| (Chair::Ours, *request)).collect();
    for (link, mut receiver) in &mut receivers {
        requests.extend(receiver.receive().map(|request| (Chair::Link(link), request)));
    }
    if requests.is_empty() || *mode.get() != AppMode::Play {
        return;
    }

    let teams = teams_in_play(spawns.iter().map(|side| side.map(|side| side.0)));
    let points: HashMap<PlayerId, i32> = standings.iter().map(|standing| (standing.id, standing.points())).collect();
    for (chair, request) in requests {
        let here = seated(us.as_deref(), links.iter());
        let Some((_, who)) = here.iter().find(|(seat, _)| *seat == chair).copied() else { continue };
        if who.seat != request.from {
            debug!("{} asked from a seat they have left; dropped", who.id);
            continue;
        }
        let tallies = tallies_without(&teams, &here, who.id, &points);
        let offered = options(who.seat, rules.teams, &tallies);
        let seat = match decide(request.choice, &offered, &tallies, &mut rand::rng()) {
            Ok(seat) => seat,
            Err(why) => {
                info!("{} cannot have {:?}: {why}", who.id, request.choice);
                continue;
            }
        };
        if seat == who.seat {
            continue;
        }
        info!("{} joins {}", who.id, team_name(seat.team()));
        match chair {
            Chair::Ours => us.as_deref_mut().expect("found above").0.seat = seat,
            Chair::Link(link) => links.get_mut(link).expect("found above").1.seat = seat,
        }
        if let Some((body, _, placed)) = bodies.iter().find(|(_, id, _)| **id == who.id) {
            kills.write(Kill { target: body, source: DamageSource::Player(who.id), point: placed.translation() });
        }
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn offers_may_have_changed(
    us: Option<Res<OurIdentity>>,
    identities: Query<(), Changed<Identity>>,
    standings: Query<(), Changed<Standing>>,
    spawns: Query<(), (With<SpawnPointMarker>, Or<(Added<SpawnPointMarker>, Changed<Side>)>)>,
    rules: Res<GameRules>,
    gone_identities: RemovedComponents<Identity>,
    gone_sides: RemovedComponents<Side>,
    gone_spawns: RemovedComponents<SpawnPointMarker>,
) -> bool {
    us.is_some_and(|us| us.is_changed())
        || !identities.is_empty()
        || !standings.is_empty()
        || !spawns.is_empty()
        || rules.is_changed()
        || !gone_identities.is_empty()
        || !gone_sides.is_empty()
        || !gone_spawns.is_empty()
}

/// On a client's link, written only on change, which is what sends it.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct Offer(pub TeamOptions);

pub(crate) fn offer_teams(
    mut commands: Commands,
    mut ours: ResMut<TeamOptions>,
    us: Option<Res<OurIdentity>>,
    links: Query<(Entity, &Identity, Option<&Offer>), With<ClientOf>>,
    rules: Res<GameRules>,
    spawns: Query<Option<&Side>, With<SpawnPointMarker>>,
    standings: Query<&Standing>,
) {
    let teams = teams_in_play(spawns.iter().map(|side| side.map(|side| side.0)));
    let points: HashMap<PlayerId, i32> = standings.iter().map(|standing| (standing.id, standing.points())).collect();
    let here = seated(us.as_deref(), links.iter().map(|(link, who, _)| (link, who)));
    for (chair, who) in &here {
        let offer = options(who.seat, rules.teams, &tallies_without(&teams, &here, who.id, &points));
        match *chair {
            Chair::Ours => {
                ours.set_if_neq(offer);
            }
            Chair::Link(link) => {
                if links.get(link).is_ok_and(|(_, _, last)| last.is_some_and(|last| last.0 == offer)) {
                    continue;
                }
                commands.entity(link).insert(Offer(offer));
            }
        }
    }
}

/// Reliable and ordered: an offer out of order would show a full team as open.
pub struct TeamChannel;

/// The wire half, beside the transport's other plugins.
pub struct TeamChoiceSyncPlugin;

impl Plugin for TeamChoiceSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<TeamChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.register_message::<TeamOptions>().add_direction(NetworkDirection::ServerToClient);
        app.register_message::<ChooseTeam>().add_direction(NetworkDirection::ClientToServer);
        app.add_systems(Update, (send_our_choice, send_offers, take_the_offer));
    }
}

#[allow(clippy::type_complexity)]
fn send_offers(offers: Query<(Entity, &Offer), (With<ClientOf>, Changed<Offer>)>, mut sender: ServerMultiMessageSender) -> Result {
    for (link, offer) in &offers {
        sender.send_to_entities::<_, TeamChannel>(&offer.0, core::iter::once(link))?;
    }
    Ok(())
}

fn send_our_choice(
    role: Res<NetRole>,
    mut ours: MessageReader<ChooseTeam>,
    mut clients: Query<&mut MessageSender<ChooseTeam>, (With<Client>, With<Connected>)>,
) {
    let requests: Vec<ChooseTeam> = ours.read().copied().collect();
    if role.is_authority() {
        return;
    }
    for mut sender in &mut clients {
        for request in &requests {
            sender.send::<TeamChannel>(*request);
        }
    }
}

fn take_the_offer(mut receivers: Query<&mut MessageReceiver<TeamOptions>, With<Client>>, mut ours: ResMut<TeamOptions>) {
    for mut receiver in &mut receivers {
        if let Some(latest) = receiver.receive().last() {
            ours.set_if_neq(latest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tallies(counts: &[(Team, usize, i32)]) -> Vec<TeamTally> {
        counts.iter().map(|&(team, players, points)| TeamTally { team, players, points }).collect()
    }

    #[test]
    fn a_map_with_fewer_than_two_sides_plays_red_and_blue() {
        assert_eq!(teams_in_play([None, None]), vec![Team::Red, Team::Blue]);
        assert_eq!(teams_in_play([Some(Team::Green)]), vec![Team::Red, Team::Blue]);
        assert_eq!(
            teams_in_play([Some(Team::Green), Some(Team::Red), Some(Team::Yellow), Some(Team::Blue), None]),
            Team::ALL.to_vec(),
            "four ways, in the fixed order"
        );
    }

    #[test]
    fn a_team_may_be_up_by_two_once_joined_and_no_more() {
        let counts = tallies(&[(Team::Red, 1, 0), (Team::Blue, 0, 0)]);
        assert!(may_join(Team::Red, &counts), "two to nothing");
        let counts = tallies(&[(Team::Red, 2, 0), (Team::Blue, 0, 0)]);
        assert!(!may_join(Team::Red, &counts), "three to nothing");
        assert!(may_join(Team::Blue, &counts));
    }

    /// Breaks if leaving your own team is not counted before joining another.
    #[test]
    fn whoever_is_choosing_is_not_counted() {
        let me = Identity { id: PlayerId(1), seat: Seat::Playing(Some(Team::Blue)), color: 0 };
        let red = |id| Identity { id: PlayerId(id), seat: Seat::Playing(Some(Team::Red)), color: 0 };
        let here = [(Chair::Ours, me), (Chair::Link(Entity::PLACEHOLDER), red(2)), (Chair::Link(Entity::PLACEHOLDER), red(3))];
        let counts = tallies_without(&[Team::Red, Team::Blue], &here, me.id, &HashMap::new());
        assert_eq!(counts, tallies(&[(Team::Red, 2, 0), (Team::Blue, 0, 0)]));

        let offer = options(me.seat, true, &counts);
        assert_eq!(offer.open, vec![(Team::Red, false), (Team::Blue, false)], "own team, or three to nothing");
        assert!(!offer.random);
    }

    #[test]
    fn random_is_the_fewest_players_then_the_fewest_points() {
        let mut rng = rand::rng();
        let counts = tallies(&[(Team::Red, 3, 0), (Team::Blue, 2, 9), (Team::Yellow, 2, 4)]);
        for _ in 0..20 {
            assert_eq!(random_team(&counts, &mut rng), Some(Team::Yellow));
        }
    }

    #[test]
    fn random_tosses_a_coin_when_all_else_is_equal() {
        let mut rng = rand::rng();
        let counts = tallies(&[(Team::Red, 1, 2), (Team::Blue, 1, 2), (Team::Green, 0, 0)]);
        let landed: Vec<_> = (0..200).filter_map(|_| random_team(&counts[..2], &mut rng)).collect();
        assert!(landed.contains(&Team::Red) && landed.contains(&Team::Blue), "{landed:?}");
    }

    #[test]
    fn random_never_puts_you_back_on_your_own_team() {
        let counts = tallies(&[(Team::Red, 0, 0), (Team::Blue, 1, 0)]);
        let offer = options(Seat::Playing(Some(Team::Red)), true, &counts);
        let mut rng = rand::rng();
        for _ in 0..20 {
            assert_eq!(decide(TeamChoice::Random, &offer, &counts, &mut rng), Ok(Seat::Playing(Some(Team::Blue))));
        }
    }

    #[test]
    fn free_for_all_is_random_into_no_team_once() {
        let mut rng = rand::rng();
        let first = options(Seat::Choosing, false, &[]);
        assert!(first.open.is_empty());
        assert_eq!(decide(TeamChoice::Random, &first, &[], &mut rng), Ok(Seat::Playing(None)));

        let again = options(Seat::Playing(None), false, &[]);
        assert!(decide(TeamChoice::Random, &again, &[], &mut rng).is_err(), "rejoined the team you are on");
    }

    #[test]
    fn nobody_may_spectate_yet() {
        let counts = tallies(&[(Team::Red, 0, 0), (Team::Blue, 0, 0)]);
        let offer = options(Seat::Choosing, true, &counts);
        assert!(decide(TeamChoice::Spectate, &offer, &counts, &mut rand::rng()).is_err());
    }

    mod in_a_world {
        use bevy::ecs::system::RunSystemOnce;

        use super::super::*;
        use crate::common::damage::Damageable;

        fn world_with_us(seat: Seat) -> World {
            let mut world = World::new();
            world.insert_resource(State::new(AppMode::Play));
            world.init_resource::<GameRules>();
            world.init_resource::<TeamOptions>();
            world.init_resource::<Messages<ChooseTeam>>();
            world.init_resource::<Messages<Kill>>();
            world.insert_resource(OurIdentity(Identity { id: PlayerId(1), seat, color: 0 }));
            world
        }

        fn ask(world: &mut World, choice: TeamChoice, from: Seat) {
            world.write_message(ChooseTeam { choice, from });
            world.run_system_once(seat_whoever_chose).unwrap();
        }

        fn our_seat(world: &World) -> Seat {
            world.resource::<OurIdentity>().0.seat
        }

        #[test]
        fn choosing_seats_us() {
            let mut world = world_with_us(Seat::Choosing);
            ask(&mut world, TeamChoice::Join(Team::Blue), Seat::Choosing);
            assert_eq!(our_seat(&world), Seat::Playing(Some(Team::Blue)));
        }

        /// A request left over from the last round must not seat us in the next.
        #[test]
        fn a_request_made_outside_play_is_dropped() {
            let mut world = world_with_us(Seat::Choosing);
            // One system throughout, so its reader keeps its place as the app's does.
            let seat = world.register_system(seat_whoever_chose);
            world.insert_resource(State::new(AppMode::Editor));
            world.write_message(ChooseTeam { choice: TeamChoice::Join(Team::Blue), from: Seat::Choosing });
            world.run_system(seat).unwrap();
            world.insert_resource(State::new(AppMode::Play));
            world.run_system(seat).unwrap();
            assert_eq!(our_seat(&world), Seat::Choosing);
        }

        /// A double click on Random would otherwise be a switch, and a death.
        #[test]
        fn a_request_from_a_seat_already_left_is_dropped() {
            let mut world = world_with_us(Seat::Playing(Some(Team::Red)));
            ask(&mut world, TeamChoice::Join(Team::Blue), Seat::Choosing);
            assert_eq!(our_seat(&world), Seat::Playing(Some(Team::Red)));
        }

        #[test]
        fn switching_with_a_body_kills_it_at_our_own_hand() {
            let mut world = world_with_us(Seat::Playing(Some(Team::Red)));
            let body = world
                .spawn((Player::default(), PlayerId(1), Damageable::default(), GlobalTransform::default()))
                .id();

            ask(&mut world, TeamChoice::Join(Team::Blue), Seat::Playing(Some(Team::Red)));

            assert_eq!(our_seat(&world), Seat::Playing(Some(Team::Blue)));
            let kills: Vec<Kill> = world.resource_mut::<Messages<Kill>>().drain().collect();
            assert_eq!(kills.len(), 1);
            assert_eq!(kills[0].target, body);
            assert_eq!(kills[0].source, DamageSource::Player(PlayerId(1)), "credited to ourselves, so it costs a point");
        }

        #[test]
        fn the_hosts_offer_is_worked_out_locally() {
            let mut world = world_with_us(Seat::Choosing);
            world.run_system_once(offer_teams).unwrap();
            let offer = world.resource::<TeamOptions>();
            assert_eq!(offer.seat, Seat::Choosing);
            assert_eq!(offer.open, vec![(Team::Red, true), (Team::Blue, true)]);
            assert!(offer.random);
        }
    }
}
