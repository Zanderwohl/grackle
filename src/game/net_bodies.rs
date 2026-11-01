//! Who gets a body. The server spawns every body, the host's included; a
//! client's arrives replicated and the client predicts only its own. See
//! "Who gets a body, and when it goes away" in CLAUDE.md.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::*;

use crate::common::app_mode::AppMode;
use std::collections::HashMap;

use crate::common::damage::{Died, NextPlayerId, PlayerId};
use crate::common::game_rules::{GameRules, ModeRules};
use crate::common::round_settings::RoundSettings;
use crate::common::player_color::lowest_free;
use crate::common::player_settings::{settings_of, ClientSettings, PlayerSettings};
use crate::common::net::NetRole;
use crate::common::team::{Side, Team};
use crate::common::weapon::WeaponCatalog;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::player::{spawn_player, LocalPlayer, Player, Simulated, Spawn};
use crate::game::bot::{BotIntent, BotOf, BotRoute, BotSeat, RouteCosts};
use crate::game::bot_sense::{BotMind, BotSenses};
use crate::game::bot_fight::BotAim;
use crate::game::bot_think::BotThought;
use crate::game::weapon::AutoReload;
use crate::game::respawn::{choose_spawn, placed_spawns, team_name, HueStart, Identity, OurIdentity, Seat};
use crate::game::score::Standing;
use crate::game::team_choice::{seat_for_a_bot, teams_in_play};

/// Spawns bodies for connected clients and marks our own when it arrives.
pub struct NetBodiesPlugin;

impl Plugin for NetBodiesPlugin {
    fn build(&self, app: &mut App) {
        // Also `DamagePlugin`'s; here too because the timers read it.
        app.add_message::<Died>()
            .init_resource::<RespawnTimers>()
            .add_systems(
                Update,
                (
                    (start_respawn_timers, give_bodies_to_whoever_needs_one)
                        .chain()
                        // A filler steps aside before somebody joining is given a body.
                        .after(crate::game::bot::fill_empty_slots)
                        .run_if(in_state(AppMode::Play)),
                    claim_our_own_body,
                    apply_player_settings.run_if(in_state(AppMode::Play)),
                ),
            )
            .add_systems(OnEnter(AppMode::Play), (forget_respawn_timers, forget_client_identities));
    }
}

/// A placeholder until gamemodes have numbers of their own.
pub const RESPAWN_SECONDS: f32 = 3.0;

/// Seconds left before each dead player is owed a body again.
#[derive(Resource, Default, Debug)]
pub struct RespawnTimers(pub HashMap<PlayerId, f32>);

impl RespawnTimers {
    fn waiting(&self, id: PlayerId) -> bool {
        self.0.contains_key(&id)
    }
}

/// Chained before `give_bodies_to_whoever_needs_one` and not through
/// `Commands`: a slow frame runs the death and the reap before either runs.
fn start_respawn_timers(
    time: Res<Time>,
    role: Option<Res<NetRole>>,
    mut deaths: MessageReader<Died>,
    mut timers: ResMut<RespawnTimers>,
) {
    // Drained either way; a client's are relayed copies.
    let deaths: Vec<PlayerId> = deaths.read().filter_map(|death| death.victim_id).collect();
    if !role.as_deref().is_none_or(NetRole::is_authority) {
        return;
    }
    let dt = time.delta_secs();
    timers.0.retain(|_, left| {
        *left -= dt;
        *left > 0.0
    });
    for id in deaths {
        timers.0.insert(id, RESPAWN_SECONDS);
    }
}

fn forget_respawn_timers(mut timers: ResMut<RespawnTimers>) {
    timers.0.clear();
}

/// A new match is a new player for everybody, as `enter_play` makes it for the
/// host by dropping `OurIdentity`: the next body a connection is given comes
/// with a fresh id, team and color. Only identities are on link entities, so
/// there is nothing else to tell apart.
fn forget_client_identities(mut commands: Commands, links: Query<Entity, With<Identity>>) {
    for link in &links {
        commands.entity(link).remove::<Identity>();
    }
}

/// Stand a body up for one client and start replicating it.
///
/// `PredictionTarget` replicates the `Predicted` marker to the owner only, on
/// the same entity rather than a second copy. No `InterpolationTarget`: that
/// would make a second entity, with no interpolation functions registered;
/// `interpolate_bodies` smooths the others instead.
fn spawn_body_for(
    commands: &mut Commands,
    spawn: Spawn,
    (who, start): (Identity, HueStart),
    (link, peer): (Entity, PeerId),
    rules: &ModeRules,
    catalog: &WeaponCatalog,
) {
    let body = spawn_player(commands, spawn, &who, who.color(&start), rules, catalog);
    commands.entity(body).insert((
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::Single(peer)),
        // Despawns the body when the connection goes.
        ControlledBy { owner: link, lifetime: Lifetime::default() },
    ));
}

/// Give a body to everybody who should have one and does not.
///
/// Keyed on absence, so joining, round start and death are one case with no
/// queue. A player who died is not owed one until their [`RespawnTimers`]
/// entry runs out, and one still choosing a team is owed none at all.
pub(crate) fn give_bodies_to_whoever_needs_one(
    mut commands: Commands,
    role: Option<Res<NetRole>>,
    links: Query<(Entity, &RemoteId), (With<ClientOf>, With<Connected>)>,
    identities: Query<&Identity>,
    owners: Query<&ControlledBy, With<Player>>,
    ours: Query<(), (With<Player>, With<LocalPlayer>)>,
    us: Option<Res<OurIdentity>>,
    collision: Res<CollisionWorld>,
    spawns: Query<(&Transform, Option<&Side>), With<SpawnPointMarker>>,
    mut ids: ResMut<NextPlayerId>,
    hue_start: Option<Res<HueStart>>,
    rules: Res<GameRules>,
    catalog: Res<WeaponCatalog>,
    timers: Option<Res<RespawnTimers>>,
    (seats, driven, round): (Query<(Entity, Option<&RouteCosts>, Option<&BotMind>), With<BotSeat>>, Query<&BotOf, With<Player>>, Option<Res<RoundSettings>>),
    standings: Query<&Standing>,
) {
    let waiting = |who: Identity| timers.as_deref().is_some_and(|timers| timers.waiting(who.id));
    if !role.as_deref().is_none_or(NetRole::is_authority) {
        return;
    }
    let hosting = matches!(role.as_deref(), Some(NetRole::Listen { .. }));

    let placed = placed_spawns(&spawns);

    // Decides the next player's color. Counted from existing identities, so
    // somebody leaving frees their hue up. A team is theirs to choose.
    let mut hues: Vec<usize> =
        identities.iter().chain(us.as_deref().map(|us| &us.0)).map(|who| who.color).collect();
    let start = match hue_start {
        Some(start) => *start,
        None => {
            let start = HueStart::random();
            commands.insert_resource(start);
            start
        }
    };
    let mut meet = |ids: &mut NextPlayerId| {
        let who = Identity { id: ids.allocate(), seat: Seat::Choosing, color: lowest_free(hues.iter().copied()) };
        hues.push(who.color);
        who
    };

    let our_identity = us.as_deref().map(|us| us.0);

    if ours.is_empty() {
        // The identity outlives the body, so a respawn is the same player on
        // the same team. `enter_play` drops it for a new match.
        let who = match us {
            Some(us) => us.0,
            None => {
                let who = meet(&mut ids);
                commands.insert_resource(OurIdentity(who));
                who
            }
        };
        if !waiting(who) && who.seat.is_playing() {
            let spawn = choose_spawn(&placed, who.team(), &collision);
            info!("Standing our own body up as {} on {}", who.id, team_name(who.team()));
            let body = spawn_player(&mut commands, spawn, &who, who.color(&start), &rules, &catalog);
            commands.entity(body).insert(LocalPlayer);
            if hosting {
                commands.entity(body).insert(Replicate::to_clients(NetworkTarget::All));
            }
        }
    }

    for (link, peer) in &links {
        if owners.iter().any(|owned| owned.owner == link) {
            continue;
        }
        // On the link entity, which lives exactly as long as the connection.
        let who = match identities.get(link) {
            Ok(who) => *who,
            Err(_) => {
                let who = meet(&mut ids);
                commands.entity(link).insert(who);
                who
            }
        };
        if waiting(who) || !who.seat.is_playing() {
            continue;
        }
        let spawn = choose_spawn(&placed, who.team(), &collision);
        info!("Standing a client's body up as {} on {}", who.id, team_name(who.team()));
        spawn_body_for(&mut commands, spawn, (who, start), (link, peer.0), &rules, &catalog);
    }

    // Everybody seated, the points and the teams in play, worked out only once a bot needs a seat.
    let mut seating: Option<(Vec<Identity>, std::collections::HashMap<PlayerId, i32>, Vec<Team>)> = None;
    for (seat, temperament, mind) in &seats {
        if driven.iter().any(|of| of.0 == seat) {
            continue;
        }
        // On the seat, like the identity, so a bot keeps its temperament across lives.
        let costs = temperament.copied().unwrap_or_else(|| {
            let rolled = RouteCosts::temperament();
            commands.entity(seat).insert(rolled);
            rolled
        });
        let mind = mind.copied().unwrap_or_else(|| {
            let rolled = BotMind::temperament(round.as_deref().map(|round| round.bot_skill).unwrap_or_default());
            commands.entity(seat).insert(rolled);
            rolled
        });
        // Wrong about different places each life.
        let costs = RouteCosts { beliefs: rand::random(), ..costs };
        // On the seat, as a client's is on its link. A bot has no picker: it takes the seat
        // Random would give a person.
        let mut who = identities.get(seat).copied().unwrap_or_else(|_| meet(&mut ids));
        if who.seat == Seat::Choosing {
            let (seated, points, teams) = seating.get_or_insert_with(|| {
                let seated = identities.iter().copied().chain(our_identity).collect();
                let points = standings.iter().map(|standing| (standing.id, standing.points())).collect();
                (seated, points, teams_in_play(placed.iter().map(|spawn| spawn.side)))
            });
            let others = seated.iter().copied().filter(|other| other.id != who.id);
            if let Some(taken) = seat_for_a_bot(teams, others, points, !rules.teams) {
                who.seat = taken;
            }
            // Counted at once, so two bots seated in one pass do not both take the side the
            // other has not joined yet.
            seated.retain(|other| other.id != who.id);
            seated.push(who);
            commands.entity(seat).insert(who);
        }
        // Refused a seat, a bot waits choosing, as a person would, rather than playing for nobody.
        if waiting(who) || who.seat == Seat::Choosing {
            continue;
        }
        let spawn = choose_spawn(&placed, who.team(), &collision);
        info!("Standing a bot's body up as {} on {}", who.id, team_name(who.team()));
        let body = spawn_player(&mut commands, spawn, &who, who.color(&start), &rules, &catalog);
        commands.entity(body).insert((BotOf(seat), BotIntent::default(), BotRoute::default(), costs, mind, BotSenses::default(), BotThought::default(), BotAim::default(), Name::new("Bot")));
        if hosting {
            commands.entity(body).insert(Replicate::to_clients(NetworkTarget::All));
        }
    }
}

/// Copy each player's settings onto their body, on the authority, which is
/// where they are acted on. Compared first, so a still body is not a changed
/// one.
#[allow(clippy::type_complexity)]
fn apply_player_settings(
    role: Option<Res<NetRole>>,
    ours: Option<Res<PlayerSettings>>,
    links: Query<&ClientSettings>,
    mut bodies: Query<(&mut AutoReload, Has<LocalPlayer>, Option<&ControlledBy>), With<Player>>,
) {
    if !role.as_deref().is_none_or(NetRole::is_authority) {
        return;
    }
    let unset = PlayerSettings::default();
    for (mut auto, local, owner) in &mut bodies {
        let link = owner.and_then(|owner| links.get(owner.owner).ok());
        let wanted = AutoReload(settings_of(local, link, ours.as_deref(), &unset).auto_reload);
        if *auto != wanted {
            *auto = wanted;
        }
    }
}

/// Recognize the body the server says is ours, by `Controlled` (the receiver
/// half of `ControlledBy`). The server predicts a body only to its
/// controller, so `LocalPlayer`, `InputMarker` and `Simulated` belong on the
/// same entity.
fn claim_our_own_body(
    mut commands: Commands,
    ours: Query<Entity, (Added<Controlled>, With<Player>, Without<LocalPlayer>)>,
) {
    for body in &ours {
        info!("The server has given us a body");
        commands.entity(body).insert((
            LocalPlayer,
            Simulated,
            InputMarker::<crate::game::player::PlayerInput>::default(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::damage::{Damageable, PlayerId};
    use crate::common::player_color::PlayerColor;
    use crate::common::team::Team;
    use crate::tool::room::Room;

    fn a_map(world: &mut World) {
        let room = Room::new(Vec3::new(-8.0, 0.0, -8.0), Vec3::new(8.0, 6.0, 8.0));
        let mut collision = CollisionWorld::default();
        collision.rebuild(std::slice::from_ref(&room));
        world.insert_resource(collision);
        world.init_resource::<NextPlayerId>();
        // The real catalog, so test bodies are armed as in a round.
        world.insert_resource(crate::common::weapon_file::default_catalog());
        world.init_resource::<GameRules>();
        world.spawn(room);
        world.spawn((Transform::from_xyz(2.0, 0.0, -3.0), SpawnPointMarker));
    }

    /// One pass of the rule. No `NetRole` means a solo game.
    fn a_turn(world: &mut World) {
        world.run_system_once(give_bodies_to_whoever_needs_one).unwrap();
        world.flush();
    }

    /// Meets us, then takes a seat, as the picker would.
    fn sit_down(world: &mut World, seat: Seat) {
        a_turn(world);
        world.resource_mut::<OurIdentity>().0.seat = seat;
    }

    /// The host's is dropped by `enter_play`; this is the same for a client.
    #[test]
    fn a_new_match_forgets_every_clients_identity() {
        let mut world = World::new();
        let who = Identity { id: PlayerId(4), seat: Seat::Playing(None), color: 2 };
        let link = world.spawn(who).id();

        world.run_system_once(forget_client_identities).unwrap();
        world.flush();
        assert!(world.get::<Identity>(link).is_none(), "the client is the same player next match");
    }

    #[test]
    fn nobody_choosing_a_team_is_given_a_body() {
        let mut world = World::new();
        a_map(&mut world);

        for _ in 0..3 {
            a_turn(&mut world);
        }
        assert_eq!(world.resource::<OurIdentity>().0.seat, Seat::Choosing);
        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 0);
    }

    #[test]
    fn whoever_has_no_body_gets_one() {
        let mut world = World::new();
        a_map(&mut world);
        sit_down(&mut world, Seat::Playing(Some(Team::Red)));

        a_turn(&mut world);

        let (placed, team) = world
            .query_filtered::<(&Transform, &Team), (With<Player>, With<PlayerId>)>()
            .single(&world)
            .unwrap();
        assert_eq!(*team, Team::Red, "the first player was put on nobody's side");
        assert!(
            (placed.translation.x - 2.0).abs() < 0.01
                && (placed.translation.z + 3.0).abs() < 0.01,
            "not put on the spawn point: {}",
            placed.translation
        );
    }

    /// Breaks if a bot is served differently from a person: a seat gets a body it drives, the
    /// identity stays on the seat across a death, and the seat going takes the body with it.
    #[test]
    fn a_bot_seat_is_served_a_body_like_a_client() {
        use crate::game::bot::{BotOf, BotSeat};
        let mut world = World::new();
        a_map(&mut world);
        // The host has chosen Red, so the bot, which has no picker, takes the other side.
        // Seated after, or the bot is seated beside nobody and tosses a coin.
        sit_down(&mut world, Seat::Playing(Some(Team::Red)));
        let seat = world.spawn(BotSeat).id();
        a_turn(&mut world);
        let bots = |world: &mut World| world.query_filtered::<(Entity, &BotOf, &PlayerId), (With<Player>, With<Simulated>)>().iter(world).map(|(body, of, id)| (body, of.0, *id)).collect::<Vec<_>>();
        let first = bots(&mut world);
        assert_eq!(first.len(), 1, "the seat was not given exactly one body");
        assert_eq!(first[0].1, seat);
        let who = *world.get::<Identity>(seat).expect("the identity is not on the seat");
        assert_eq!(first[0].2, who.id);
        assert_eq!(who.seat, Seat::Playing(Some(Team::Blue)), "the bot did not take the open side");
        assert_eq!(world.query_filtered::<(), (With<Player>, With<LocalPlayer>)>().iter(&world).count(), 1, "the bot took the host's place");

        world.despawn(first[0].0);
        a_turn(&mut world);
        let second = bots(&mut world);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].2, who.id, "a bot came back as somebody else");
        let temperament = world.get::<crate::game::bot::RouteCosts>(seat).copied();
        assert!(temperament.is_some(), "the seat holds no temperament");
        let reborn = world.get::<crate::game::bot::RouteCosts>(second[0].0).copied().unwrap();
        assert_eq!(crate::game::bot::RouteCosts { beliefs: 0, ..reborn }, crate::game::bot::RouteCosts { beliefs: 0, ..temperament.unwrap() }, "a bot came back with a different temperament");
    }

    /// Dead, nobody is owed a body until the timer runs out.
    #[test]
    fn the_dead_wait_out_their_respawn_timer() {
        let mut world = World::new();
        a_map(&mut world);
        let us = Identity { id: PlayerId(7), seat: Seat::Playing(Some(Team::Red)), color: 0 };
        world.insert_resource(OurIdentity(us));
        world.init_resource::<RespawnTimers>();
        world.resource_mut::<RespawnTimers>().0.insert(us.id, RESPAWN_SECONDS);

        a_turn(&mut world);
        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 0);

        world.resource_mut::<RespawnTimers>().0.clear();
        a_turn(&mut world);
        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 1);
    }

    /// Breaks if dying allocates a new id or team.
    #[test]
    fn a_new_life_keeps_the_same_identity() {
        let mut world = World::new();
        a_map(&mut world);
        sit_down(&mut world, Seat::Playing(Some(Team::Red)));

        let mut lives = Vec::new();
        for _ in 0..3 {
            a_turn(&mut world);
            let body = world
                .query_filtered::<Entity, With<Player>>()
                .single(&world)
                .unwrap();
            lives.push((*world.get::<PlayerId>(body).unwrap(), *world.get::<Team>(body).unwrap()));
            world.despawn(body);
        }

        assert_eq!(lives[0], lives[1], "a life cost the player their identity");
        assert_eq!(lives[1], lives[2], "a life cost the player their identity");
    }

    /// The rule runs every frame, so this breaking fills the map with bodies.
    #[test]
    fn a_living_body_is_not_joined_by_another() {
        let mut world = World::new();
        a_map(&mut world);
        sit_down(&mut world, Seat::Playing(Some(Team::Red)));

        for _ in 0..4 {
            a_turn(&mut world);
        }

        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 1);
    }

    /// Breaks if a body at zero health, not yet reaped, gets a second body
    /// beside it.
    #[test]
    fn a_body_at_zero_health_is_still_a_body() {
        let mut world = World::new();
        a_map(&mut world);
        sit_down(&mut world, Seat::Playing(Some(Team::Red)));
        a_turn(&mut world);

        let body = world.query_filtered::<Entity, With<Player>>().single(&world).unwrap();
        world.get_mut::<Damageable>(body).unwrap().apply(1000);
        a_turn(&mut world);

        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 1);
    }

    #[test]
    fn without_teams_a_body_has_no_side_but_a_color_and_the_modes_loadout() {
        let mut world = World::new();
        a_map(&mut world);
        let wrench = crate::common::weapon::WeaponId::of("medi_wrench");
        world.insert_resource(GameRules(ModeRules { teams: false, loadout: Some(vec![wrench]), ..default() }));
        sit_down(&mut world, Seat::Playing(None));

        a_turn(&mut world);

        let body = world.query_filtered::<Entity, With<Player>>().single(&world).unwrap();
        assert!(world.get::<Team>(body).is_none(), "put on a side in a mode without teams");
        let start = *world.resource::<HueStart>();
        assert_eq!(world.get::<PlayerColor>(body), Some(&PlayerColor::nth(start.0, 0)));
        assert_eq!(world.get::<crate::common::weapon::Equipped>(body).unwrap().held(), Some(wrench));
    }

    #[test]
    fn a_new_life_keeps_its_color() {
        let mut world = World::new();
        a_map(&mut world);
        sit_down(&mut world, Seat::Playing(Some(Team::Red)));

        a_turn(&mut world);
        let body = world.query_filtered::<Entity, With<Player>>().single(&world).unwrap();
        let first = *world.get::<PlayerColor>(body).unwrap();
        world.despawn(body);
        a_turn(&mut world);

        let body = world.query_filtered::<Entity, With<Player>>().single(&world).unwrap();
        assert_eq!(*world.get::<PlayerColor>(body).unwrap(), first);
    }

    #[test]
    fn a_map_without_a_spawn_point_still_puts_you_somewhere() {
        let mut world = World::new();
        a_map(&mut world);
        sit_down(&mut world, Seat::Playing(Some(Team::Red)));
        let marker = world
            .query_filtered::<Entity, With<SpawnPointMarker>>()
            .single(&world)
            .unwrap();
        world.despawn(marker);

        a_turn(&mut world);

        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 1);
    }
}
