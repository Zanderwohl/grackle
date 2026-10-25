//! Who gets a body. The server spawns every body, the host's included; a
//! client's arrives replicated and the client predicts only its own. See
//! "Who gets a body, and when it goes away" in CLAUDE.md.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::NextPlayerId;
use crate::common::net::NetRole;
use crate::common::team::Side;
use crate::common::weapon::WeaponCatalogue;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::player::{spawn_player, LocalPlayer, Player, Simulated, Spawn};
use crate::game::respawn::{choose_spawn, next_team, placed_spawns, Identity, OurIdentity};
use crate::tool::room::Room;

/// Spawns bodies for connected clients and marks our own when it arrives.
pub struct NetBodiesPlugin;

impl Plugin for NetBodiesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                give_bodies_to_whoever_needs_one.run_if(in_state(AppMode::Play)),
                claim_our_own_body,
            ),
        );
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
    who: Identity,
    link: Entity,
    peer: PeerId,
    catalogue: &WeaponCatalogue,
) {
    let body = spawn_player(commands, spawn, who.id, who.team, catalogue);
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
/// queue or timer. Respawning is therefore immediate; delays are a gamemode
/// question.
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
    catalogue: Res<WeaponCatalogue>,
) {
    if !role.as_deref().is_none_or(NetRole::is_authority) {
        return;
    }
    let hosting = matches!(role.as_deref(), Some(NetRole::Listen { .. }));

    let placed = placed_spawns(&spawns);
    let rooms: Vec<Room> = rooms.iter().cloned().collect();

    // Decides the next player's team. Counted from existing identities, so
    // somebody leaving frees their side up.
    let mut here = identities.iter().count() + usize::from(us.is_some());

    if ours.is_empty() {
        // The identity outlives the body, so a respawn is the same player on
        // the same team. `enter_play` drops it for a new match.
        let who = match us {
            Some(us) => us.0,
            None => {
                let who = Identity { id: ids.allocate(), team: next_team(here) };
                commands.insert_resource(OurIdentity(who));
                here += 1;
                who
            }
        };
        let spawn = choose_spawn(&placed, who.team, &collision, &rooms);
        info!("Standing our own body up as {} on {}", who.id, who.team.name());
        let body = spawn_player(&mut commands, spawn, who.id, who.team, &catalogue);
        commands.entity(body).insert(LocalPlayer);
        if hosting {
            commands.entity(body).insert(Replicate::to_clients(NetworkTarget::All));
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
                let who = Identity { id: ids.allocate(), team: next_team(here) };
                commands.entity(link).insert(who);
                here += 1;
                who
            }
        };
        let spawn = choose_spawn(&placed, who.team, &collision, &rooms);
        info!("Standing a client's body up as {} on {}", who.id, who.team.name());
        spawn_body_for(&mut commands, spawn, who, link, peer.0, &catalogue);
    }
}

/// Recognise the body the server says is ours, by `Controlled` (the receiver
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
    use crate::common::team::Team;

    fn a_map(world: &mut World) {
        let room = Room::new(Vec3::new(-8.0, 0.0, -8.0), Vec3::new(8.0, 6.0, 8.0));
        let mut collision = CollisionWorld::default();
        collision.rebuild(std::slice::from_ref(&room));
        world.insert_resource(collision);
        world.init_resource::<NextPlayerId>();
        // The real catalogue, so test bodies are armed as in a round.
        world.insert_resource(crate::common::weapon_file::default_catalogue());
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
