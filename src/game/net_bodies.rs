//! Who gets a body, and which process is believed about where it is.
//!
//! The server spawns every body, including the host's own. A client never
//! spawns one: its body arrives replicated, marked as predicted, and the
//! client steps its own copy forward from its own inputs so that moving feels
//! immediate — then is corrected whenever the server disagrees.
//!
//! Three kinds of body end up in a client's world and they are not
//! interchangeable:
//!
//! | Body | What it is | Who steps it |
//! | --- | --- | --- |
//! | Predicted | our own, run ahead of the server | us, and replayed on rollback |
//! | Interpolated | somebody else's, drawn slightly in the past | nobody — Lightyear blends it |
//! | Confirmed | the server's last word, kept for comparison | nobody |
//!
//! Stepping an interpolated body is the mistake this module exists to prevent:
//! it does not error, it just makes everybody else's body twitch between where
//! the simulation put it and where the server said it was.

use bevy::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::damage::NextPlayerId;
use crate::common::net::NetRole;
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
/// One entity, replicated to everybody, and `ControlledBy` says whose it is.
///
/// `PredictionTarget` goes to its owner alone, because only the person holding
/// the controls has inputs to run ahead with. It is a replication target for
/// the `Predicted` *marker*, so the owner ends up with one entity carrying it
/// rather than a predicted copy beside a confirmed one — there is no ghost of
/// yourself to hide.
///
/// Still no `InterpolationTarget`: that one really would make a second entity,
/// written by interpolation functions none of which are registered.
/// `interpolate_bodies` smooths everybody else between the last two positions
/// the server sent.
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
        // Ties the body's life to the connection's: somebody who disconnects
        // does not leave a body standing in the map for the rest of the round.
        ControlledBy { owner: link, lifetime: Lifetime::default() },
    ));
}

/// Give a body to everybody who should have one and does not.
///
/// **One rule instead of four.** This used to be a system for a client
/// arriving mid-round, another for clients already connected when the round
/// started, a third to start replicating the host's body, and a spawn inside
/// `enter_play` for the host itself. Each one answered "when does somebody get
/// a body" for its own case, and respawning would have been a fifth.
///
/// Asking about *absence* answers all of them at once. A player with no body
/// gets one, whether they never had one, joined a minute ago, or died two
/// seconds ago. There is no queue to keep, no timer keyed to an entity that
/// has already been despawned, and nothing to get wrong when a case nobody
/// thought of turns up.
///
/// **Respawning is immediate**, which is a placeholder and not a decision.
/// When a body comes back — on a wave, after a delay, at your team's end of
/// the map — is a gamemode question, and this layer deliberately answers none
/// of those. What it guarantees is only that being alive is the resting state.
fn give_bodies_to_whoever_needs_one(
    mut commands: Commands,
    role: Option<Res<NetRole>>,
    links: Query<(Entity, &RemoteId), (With<ClientOf>, With<Connected>)>,
    identities: Query<&Identity>,
    owners: Query<&ControlledBy, With<Player>>,
    ours: Query<(), (With<Player>, With<LocalPlayer>)>,
    us: Option<Res<OurIdentity>>,
    collision: Res<CollisionWorld>,
    spawns: Query<&Transform, With<SpawnPointMarker>>,
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

    // How many people are already in the match, which is what decides the side
    // the next one is put on. Counted from the identities that exist rather
    // than from a running total, so somebody leaving frees their side up.
    let mut here = identities.iter().count() + usize::from(us.is_some());

    // Whoever is sitting at this machine, if anybody is. A dedicated server
    // would have nobody here and the loop below would be the whole of it.
    if ours.is_empty() {
        // **The identity outlives the body.** Made once and kept for the rest
        // of the match, so the body that comes back after a death is the same
        // player: a fresh id per life is a scoreboard showing one player per
        // life, and a fresh team is being put on the other side for dying.
        // `enter_play` drops it, which is what makes a new match a new player.
        let who = match us {
            Some(us) => us.0,
            None => {
                let who = Identity { id: ids.allocate(), team: next_team(here) };
                commands.insert_resource(OurIdentity(who));
                here += 1;
                who
            }
        };
        let spawn = choose_spawn(&placed, &collision, &rooms);
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
        // On the link rather than in a roster of our own, because the link is
        // already exactly the right lifetime: it appears when somebody
        // connects and goes when they leave, so an identity cannot outlive the
        // person it belongs to or be inherited by whoever reconnects next.
        let who = match identities.get(link) {
            Ok(who) => *who,
            Err(_) => {
                let who = Identity { id: ids.allocate(), team: next_team(here) };
                commands.entity(link).insert(who);
                here += 1;
                who
            }
        };
        let spawn = choose_spawn(&placed, &collision, &rooms);
        info!("Standing a client's body up as {} on {}", who.id, who.team.name());
        spawn_body_for(&mut commands, spawn, who, link, peer.0, &catalogue);
    }
}

/// Recognise the body the server says is ours.
///
/// `Controlled` is the receiver-side half of the `ControlledBy` the server put
/// on it, so it arrives on exactly one body and only on the machine that
/// drives it. That makes it the natural answer to "which of these is mine",
/// and it does not require a second copy of the body to exist the way asking
/// `Predicted` did.
///
/// Marking it `LocalPlayer` is what aims the view at it; `InputMarker` is what
/// makes Lightyear send this machine's inputs for it; `Simulated` is what makes
/// `step_player` run it ahead of the server.
///
/// All three land together because they are the same body by construction: the
/// server predicts a body only to the client that controls it, so "ours",
/// "what we drive" and "what we predict" cannot come apart.
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

    /// A room with a floor, and a spawn point standing in it.
    fn a_map(world: &mut World) {
        let room = Room::new(Vec3::new(-8.0, 0.0, -8.0), Vec3::new(8.0, 6.0, 8.0));
        let mut collision = CollisionWorld::default();
        collision.rebuild(std::slice::from_ref(&room));
        world.insert_resource(collision);
        world.init_resource::<NextPlayerId>();
        // A body is handed its weapons at spawn, so the rule needs a
        // catalogue to hand them out of. The real one, so that a body stood up
        // in a test is armed the way a body stood up in a round is.
        world.insert_resource(crate::common::weapon_file::default_catalogue());
        world.spawn(room);
        world.spawn((Transform::from_xyz(2.0, 0.0, -3.0), SpawnPointMarker));
    }

    /// One pass of the rule, commands applied. No `NetRole`, which is a closed
    /// game — its own authority, and the case every other role is a variation
    /// on.
    fn a_turn(world: &mut World) {
        world.run_system_once(give_bodies_to_whoever_needs_one).unwrap();
        world.flush();
    }

    /// The whole feature: somebody with no body gets one, at a spawn point,
    /// as somebody in particular.
    #[test]
    fn whoever_has_no_body_gets_one() {
        let mut world = World::new();
        a_map(&mut world);

        a_turn(&mut world);

        let (placed, id, team) = world
            .query_filtered::<(&Transform, &PlayerId, &Team), With<Player>>()
            .single(&world)
            .unwrap();
        assert_eq!(*team, Team::Red, "the first player was put on nobody's side");
        assert!(id.0 > 0 || id.0 == 0, "a body with no identity: {id}");
        assert!(
            (placed.translation.x - 2.0).abs() < 0.01
                && (placed.translation.z + 3.0).abs() < 0.01,
            "not put on the spawn point: {}",
            placed.translation
        );
    }

    /// **The one that matters for a scoreboard**: a life is not a new player.
    ///
    /// `F5` allocates a fresh identity on purpose — `enter_play` drops the
    /// resource — but dying must not, or one player who died four times is
    /// five players on the scoreboard and possibly on four different sides.
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
            // Die.
            world.despawn(body);
        }

        assert_eq!(lives[0], lives[1], "a life cost the player their identity");
        assert_eq!(lives[1], lives[2], "a life cost the player their identity");
    }

    /// And it does not stand a second body up beside a living one. The rule
    /// runs every frame, so getting this wrong fills the map with bodies
    /// rather than leaving it short of one.
    #[test]
    fn a_living_body_is_not_joined_by_another() {
        let mut world = World::new();
        a_map(&mut world);

        for _ in 0..4 {
            a_turn(&mut world);
        }

        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 1);
    }

    /// A body still on its feet is left alone even at zero health. The
    /// reaping is what decides a body is gone, and standing another up beside
    /// something about to be reaped would leave two of you for a tick.
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

    /// A map with no spawn point at all still puts you somewhere, rather than
    /// leaving a match with nobody in it and no error to say why.
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
