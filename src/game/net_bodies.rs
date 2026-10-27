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
use crate::common::player_color::lowest_free;
use crate::common::net::NetRole;
use crate::common::team::Side;
use crate::common::weapon::WeaponCatalog;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::player::{spawn_player, LocalPlayer, Player, Simulated, Spawn};
use crate::game::respawn::{choose_spawn, next_team, placed_spawns, team_name, HueStart, Identity, OurIdentity};
use crate::tool::room::Room;

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
                        .run_if(in_state(AppMode::Play)),
                    claim_our_own_body,
                ),
            )
            .add_systems(OnEnter(AppMode::Play), forget_respawn_timers);
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
/// entry runs out.
fn give_bodies_to_whoever_needs_one(
    mut commands: Commands,
    role: Option<Res<NetRole>>,
    links: Query<(Entity, &RemoteId), (With<ClientOf>, With<Connected>)>,
    identities: Query<&Identity>,
    owners: Query<&ControlledBy, With<Player>>,
    ours: Query<(), (With<Player>, With<LocalPlayer>)>,
    us: Option<Res<OurIdentity>>,
    collision: Res<CollisionWorld>,
    spawns: Query<(&Transform, Option<&Side>), With<SpawnPointMarker>>,
    rooms: Query<&Room>,
    mut ids: ResMut<NextPlayerId>,
    hue_start: Option<Res<HueStart>>,
    rules: Res<GameRules>,
    catalog: Res<WeaponCatalog>,
    timers: Option<Res<RespawnTimers>>,
) {
    let waiting = |who: Identity| timers.as_deref().is_some_and(|timers| timers.waiting(who.id));
    if !role.as_deref().is_none_or(NetRole::is_authority) {
        return;
    }
    let hosting = matches!(role.as_deref(), Some(NetRole::Listen { .. }));

    let placed = placed_spawns(&spawns);
    let rooms: Vec<Room> = rooms.iter().cloned().collect();

    // Decides the next player's team and color. Counted from existing
    // identities, so somebody leaving frees their side and hue up.
    let mut here = identities.iter().count() + usize::from(us.is_some());
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
        let who = Identity {
            id: ids.allocate(),
            team: next_team(here, rules.teams),
            color: lowest_free(hues.iter().copied()),
        };
        here += 1;
        hues.push(who.color);
        who
    };

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
        if !waiting(who) {
            let spawn = choose_spawn(&placed, who.team, &collision, &rooms);
            info!("Standing our own body up as {} on {}", who.id, team_name(who.team));
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
        if waiting(who) {
            continue;
        }
        let spawn = choose_spawn(&placed, who.team, &collision, &rooms);
        info!("Standing a client's body up as {} on {}", who.id, team_name(who.team));
        spawn_body_for(&mut commands, spawn, (who, start), (link, peer.0), &rules, &catalog);
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

    #[test]
    fn whoever_has_no_body_gets_one() {
        let mut world = World::new();
        a_map(&mut world);

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

    /// Dead, nobody is owed a body until the timer runs out.
    #[test]
    fn the_dead_wait_out_their_respawn_timer() {
        let mut world = World::new();
        a_map(&mut world);
        let us = Identity { id: PlayerId(7), team: Some(Team::Red), color: 0 };
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
        let marker = world
            .query_filtered::<Entity, With<SpawnPointMarker>>()
            .single(&world)
            .unwrap();
        world.despawn(marker);

        a_turn(&mut world);

        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 1);
    }
}
