//! Getting back up.
//!
//! Death used to be the end of the session: `reap_the_dead` despawned the
//! body, the camera went with it as a child, and what was left was a map with
//! nobody in it and no way back except `F5`. This is the smallest thing that
//! is not that — **you come back instantly, on the same tick, at a spawn point
//! chosen the same way the first one was.**
//!
//! Instant is a placeholder and is meant to look like one. A respawn *timer*
//! is a gamemode's number, along with wave respawns, spawn protection and
//! whether you come back at all; none of those can be answered here without
//! answering them for every gamemode at once. What is not a placeholder is the
//! shape:
//!
//! - **Your identity outlives your body.** [`LocalPlayer`] holds the
//!   [`PlayerId`] and the team for the whole match, and a new body is given
//!   the *same* id. That is the opposite of what `F5` does, and deliberately:
//!   a new match is a new player, a new life is not. A scoreboard that handed
//!   out a fresh id per death would show one player per life.
//! - **The choice of where is shared with the first spawn**, in
//!   [`choose_spawn`]. Two copies of "pick a usable spawn point, fall back to
//!   the largest room" is one copy that quietly stops matching the other, and
//!   the failure is a body that respawns inside a wall on maps the first spawn
//!   handles fine.
//!
//! It runs **on the tick, immediately after the reaping**, rather than in
//! `Update`. A frame with no player is a frame with no camera, which is a
//! black screen and a stream of Bevy warnings; landing in the same fixed step
//! means there is never a moment when the world has nobody in it.

use bevy::prelude::*;
use rand::seq::IndexedRandom;

use crate::common::damage::PlayerId;
use crate::common::team::Team;
use crate::editor::spawn_point::SpawnPointMarker;
use crate::game::collision::CollisionWorld;
use crate::game::player::{fallback_spawn, spawn_player, usable_spawns, Player, Spawn};
use crate::tool::room::Room;

/// Who you are for the length of a match, independent of whatever body you
/// are currently wearing.
///
/// A resource rather than a component for exactly the reason this module
/// exists: components live on entities, and the entity is the thing that keeps
/// being destroyed. Written once when a match starts and read every time a
/// body has to be stood up.
///
/// One player, because there is one. When there are more this becomes the
/// local half of a roster and the rest of it comes off the wire — but the
/// distinction it draws, between the player and the body, is the one that
/// survives that.
#[derive(Resource, Clone, Copy, Debug)]
pub struct LocalPlayer {
    pub id: PlayerId,
    pub team: Team,
}

/// Where to put a body, given what the map offers.
///
/// Shared by the first spawn of a match and every one after it. Uniformly at
/// random for now: per-team spawns, and not dropping somebody on top of
/// somebody else, are gamemode questions this is deliberately not trying to
/// answer yet — and both of them get harder, not easier, if the two spawn
/// paths have drifted apart by the time they are asked.
///
/// Warns about spawn points it had to skip, and falls back to the largest room
/// rather than refusing to put anybody anywhere. A map with no usable spawn is
/// a map you can still walk around in.
pub fn choose_spawn(placed: &[Spawn], collision: &CollisionWorld, rooms: &[Room]) -> Spawn {
    let usable = usable_spawns(placed, collision);
    if usable.len() < placed.len() {
        warn!(
            "{} of {} spawn point(s) have too little headroom to stand in",
            placed.len() - usable.len(),
            placed.len()
        );
    }

    match usable.choose(&mut rand::rng()) {
        Some(spawn) => *spawn,
        None => {
            warn!("No usable spawn point on this map; falling back to the largest room");
            fallback_spawn(rooms)
        }
    }
}

/// Every spawn point on the map, as somewhere a body could stand.
///
/// The feature writes its facing into the transform's rotation, so the marked
/// entity carries both halves and neither has to be looked up twice.
pub fn placed_spawns(spawns: &Query<&Transform, With<SpawnPointMarker>>) -> Vec<Spawn> {
    spawns
        .iter()
        .map(|placed| Spawn {
            feet: placed.translation,
            yaw: placed.rotation.to_euler(EulerRot::YXZ).0,
        })
        .collect()
}

/// Put a body back on the map whenever there is not one.
///
/// Stated as an invariant — *while you are playing, there is a body* — rather
/// than as a reaction to a [`Died`](crate::common::damage::Died) message, and
/// that is the more honest of the two while there is one player: it is right
/// whatever removed the body, including a way of dying nobody has written yet.
/// The day there are several players this becomes "a player with no body",
/// which is the same rule with a roster behind it.
///
/// The corpse is left where it fell. `raise_ragdolls` stands one up before the
/// reaping, so what you see on coming back is the body you were in a moment
/// ago, lying there — which is the whole of the death feedback for now.
pub fn respawn_players(
    mut commands: Commands,
    local: Option<Res<LocalPlayer>>,
    collision: Res<CollisionWorld>,
    rooms: Query<&Room>,
    spawns: Query<&Transform, With<SpawnPointMarker>>,
    players: Query<(), With<Player>>,
) {
    if !players.is_empty() {
        return;
    }
    // No match is running: nothing has entered Play, or it is on its way out.
    // Standing a body up now would be standing one up in the editor.
    let Some(local) = local else { return };

    let rooms: Vec<Room> = rooms.iter().cloned().collect();
    let spawn = choose_spawn(&placed_spawns(&spawns), &collision, &rooms);
    spawn_player(&mut commands, spawn, local.id, local.team);
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::damage::Damageable;

    /// A room with a floor, and a spawn point standing in it.
    fn a_map(world: &mut World) {
        let room = Room::new(Vec3::new(-8.0, 0.0, -8.0), Vec3::new(8.0, 6.0, 8.0));
        let mut collision = CollisionWorld::default();
        collision.rebuild(std::slice::from_ref(&room));
        world.insert_resource(collision);
        world.spawn(room);
        world.spawn((Transform::from_xyz(2.0, 0.0, -3.0), SpawnPointMarker));
    }

    fn a_match(world: &mut World) {
        a_map(world);
        world.insert_resource(LocalPlayer { id: PlayerId(7), team: Team::Blue });
    }

    /// The whole feature: a body that is gone comes back, at a spawn point.
    #[test]
    fn a_map_with_nobody_on_it_gets_somebody() {
        let mut world = World::new();
        a_match(&mut world);

        world.run_system_once(respawn_players).unwrap();

        let (transform, id, team) = world
            .query::<(&Transform, &PlayerId, &Team)>()
            .single(&world)
            .unwrap();
        assert_eq!(*id, PlayerId(7), "came back as somebody else");
        assert_eq!(*team, Team::Blue, "came back on the wrong side");
        assert!(
            (transform.translation.x - 2.0).abs() < 0.01
                && (transform.translation.z + 3.0).abs() < 0.01,
            "not put on the spawn point: {}",
            transform.translation
        );
    }

    /// **The one that matters for a scoreboard**: a life is not a new player.
    /// `F5` allocates a fresh id on purpose; dying must not, or one player who
    /// died four times is five players.
    #[test]
    fn a_new_life_keeps_the_same_identity() {
        let mut world = World::new();
        a_match(&mut world);

        let mut ids = Vec::new();
        for _ in 0..3 {
            world.run_system_once(respawn_players).unwrap();
            let body = world.query_filtered::<Entity, With<Player>>().single(&world).unwrap();
            ids.push(*world.get::<PlayerId>(body).unwrap());
            // Die.
            world.despawn(body);
        }

        assert_eq!(ids, vec![PlayerId(7); 3], "a life cost the player their identity");
    }

    /// And it does not stand up a second body beside a living one. The system
    /// runs every tick, so getting this wrong is a map that fills with bodies
    /// rather than one that is short of one.
    #[test]
    fn a_living_body_is_not_joined_by_another() {
        let mut world = World::new();
        a_match(&mut world);
        world.run_system_once(respawn_players).unwrap();

        for _ in 0..3 {
            world.run_system_once(respawn_players).unwrap();
        }

        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 1);
    }

    /// A body still on its feet is left alone even at zero health — the
    /// reaping is what decides a body is gone, and respawning something that
    /// is about to be reaped would leave two of you for a tick.
    #[test]
    fn a_body_at_zero_health_is_still_a_body() {
        let mut world = World::new();
        a_match(&mut world);
        world.run_system_once(respawn_players).unwrap();

        let body = world.query_filtered::<Entity, With<Player>>().single(&world).unwrap();
        world.get_mut::<Damageable>(body).unwrap().apply(1000);
        world.run_system_once(respawn_players).unwrap();

        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 1);
    }

    /// Nothing is standing anybody up outside a match. The system is gated on
    /// `AppMode::Play` as well, but the resource is what makes it safe on its
    /// own — the gate is a schedule detail and this is the invariant.
    #[test]
    fn nobody_is_spawned_when_no_match_is_running() {
        let mut world = World::new();
        a_map(&mut world);

        world.run_system_once(respawn_players).unwrap();

        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 0);
    }

    /// **The ordering claim, end to end**: a body killed by a shot is dead,
    /// reaped, and back on its feet inside one fixed step.
    ///
    /// Worth assembling the plugins for, because nothing in the type system
    /// holds it together and the failure is not an error — it is a frame with
    /// no camera, or, if the despawn has not been applied by the time this
    /// runs, a respawn that never happens because the corpse still counts as a
    /// player.
    #[test]
    fn a_killed_body_is_back_on_its_feet_in_the_same_tick() {
        use crate::common::app_mode::AppMode;
        use crate::common::damage::{Damage, DamageSource};
        use crate::common::skeleton::AnimationClock;
        use crate::game::damage::{DamagePlugin, DamageSystems};
        use crate::game::death::reap_the_dead;

        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        app.init_state::<AppMode>();
        app.init_resource::<AnimationClock>();
        app.add_plugins(DamagePlugin);
        app.add_systems(
            FixedUpdate,
            respawn_players.after(reap_the_dead).in_set(DamageSystems::Resolve),
        );
        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Play);
        app.update();

        a_match(app.world_mut());
        let spawn = Spawn { feet: Vec3::new(2.0, 0.0, -3.0), yaw: 0.0 };
        app.world_mut().commands().queue(move |world: &mut World| {
            let mut commands = world.commands();
            spawn_player(&mut commands, spawn, PlayerId(7), Team::Blue);
        });
        app.update();

        let died = app
            .world_mut()
            .query_filtered::<Entity, With<Player>>()
            .single(app.world())
            .unwrap();
        app.world_mut().write_message(Damage {
            target: died,
            source: DamageSource::World,
            amount: 10_000,
            point: Vec3::ZERO,
        });

        // One whole fixed step, driven by hand: what is under test is what
        // happens inside one, not how the fixed loop decides to take it.
        app.world_mut().run_schedule(FixedUpdate);

        let (body, id) = app
            .world_mut()
            .query_filtered::<(Entity, &PlayerId), With<Player>>()
            .single(app.world())
            .unwrap();
        assert_ne!(body, died, "the corpse was left standing as the player");
        assert_eq!(*id, PlayerId(7));
        assert!(
            app.world().get::<crate::common::damage::Damageable>(body).unwrap().is_alive(),
            "came back dead"
        );
    }

    /// A map with no spawn point at all still puts you somewhere, rather than
    /// leaving a match with nobody in it and no error to say why.
    #[test]
    fn a_map_without_a_spawn_point_still_puts_you_somewhere() {
        let mut world = World::new();
        a_match(&mut world);
        let marker = world
            .query_filtered::<Entity, With<SpawnPointMarker>>()
            .single(&world)
            .unwrap();
        world.despawn(marker);

        world.run_system_once(respawn_players).unwrap();

        assert_eq!(world.query_filtered::<(), With<Player>>().iter(&world).count(), 1);
    }
}
