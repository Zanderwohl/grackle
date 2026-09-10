//! What actually goes over the wire, and which end is believed about it.
//!
//! Shared by both ends by construction: there is one binary, so a client and a
//! server register the same components in the same order because they run the
//! same code. Lightyear hashes the registries and refuses a connection whose
//! hashes differ, which turns "these two builds disagree" into a refused
//! handshake rather than two processes quietly reading each other's packets
//! wrong.
//!
//! Must be added **after** the client and server plugin groups and **before**
//! any link entity is spawned — see [`NetTransportPlugin`].
//!
//! [`NetTransportPlugin`]: crate::common::net_transport::NetTransportPlugin

use bevy::ecs::entity::{EntityMapper, MapEntities};
use bevy::prelude::*;
use lightyear::prelude::input::native::InputPlugin;
use lightyear::prelude::*;

use crate::common::class::Stance;
use crate::common::damage::{Damageable, PlayerId};
use crate::common::skeleton::BodyRequests;
use crate::game::player::{PhysicsBody, Player, PlayerInput};

/// Registers everything the two ends have to agree about.
pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // The input is not replicated like a component: it travels the other
        // way, client to server, in its own redundant queue that carries the
        // last several ticks so that one lost packet is not one lost step.
        app.add_plugins(InputPlugin::<PlayerInput>::default());

        // What the step writes, and therefore both what a client is told and
        // what has to be rewound and replayed when the server disagrees.
        //
        // All three, because a rollback replays the step and a step that
        // started from the wrong state diverges again immediately:
        // `PhysicsBody` is the position, `Player` carries the velocity and the
        // on-ground flag, and `Stance` decides how tall the hull being swept
        // is.
        app.component::<PhysicsBody>().replicate().predict();
        app.component::<Player>().replicate().predict();
        app.component::<Stance>().replicate().predict();

        // How a body is animated, in nine bools.
        //
        // **Poses are never sent.** A pose is twenty-odd quaternions per body
        // per tick, and it is the output of a state machine that is
        // deterministic and already present on every machine. What crosses is
        // the input to that machine — what the body is being asked to do and
        // what the world is doing to it — and each process runs the same
        // `AnimationState` over it. `Gait` is not sent either: it is advanced
        // from ground covered, and the ground covered is `PhysicsBody`, which
        // is.
        app.component::<BodyRequests>().replicate();

        // Health, because a corpse is something every client has to see.
        //
        // Not predicted: a client guessing that its shot killed somebody and
        // being wrong is a body that falls over and stands back up. Damage is
        // the server's to decide and this is the client being told.
        app.component::<Damageable>().replicate();

        // What a body is built like, and what it is doing. Between them, plus
        // the `PhysicsBody` above, they are everything a viewer needs to draw
        // a body somebody else is driving — including one standing in an
        // animation grid, which is why none of them mentions grids.
        app.component::<crate::common::class::Class>().replicate_once();
        app.component::<crate::common::skeleton::ForcedAnimation>().replicate();
        app.component::<crate::common::skeleton::gait::DisplaySpeed>().replicate();

        // That a body has died. Its one job on the wire is to hide the body
        // for the tick it outlives itself by — a death is always a despawn,
        // and without this the body stands upright inside its own corpse until
        // the despawn arrives.
        app.component::<crate::game::death::Dying>().replicate_once();

        // A corpse, and only its **seed**. `Ragdoll` carries two points per
        // bone at the moment of death, and from there every machine runs the
        // same solver over the same rig: nothing can touch a corpse, nothing
        // collides with one, and it takes no input, so the simulation is a
        // function of the seed alone. `replicate_once` is what keeps that
        // affordable — twenty bones of physics per corpse once, not per tick.
        //
        // A corpse is a prop, so it is replicated the way a prop is. The
        // alternative — every machine raising its own from the replicated fact
        // that a body's health hit zero — cannot be made to work, because that
        // fact and the despawn that follows arrive together and the body is
        // gone by the time anything can copy its pose.
        app.component::<crate::game::ragdoll::Ragdoll>().replicate_once();

        // A projectile in flight, so everybody can see it coming. Replicated
        // rather than re-simulated: see `replicate_projectiles`.
        app.component::<crate::game::projectile::Projectile>().replicate();

        // Sent once, on the insert that first carries it. An id is a fact
        // about who a body belongs to, not a value that changes per tick, and
        // re-sending it every update would be bandwidth spent restating it.
        // It is also what an animation phase is derived from, so that two
        // clients put the same body at the same point in its cycle.
        app.component::<PlayerId>().replicate_once();
    }
}

/// Required by the input layer; there is nothing to map.
///
/// Inputs may name entities — "I am shooting at that one" — and those ids are
/// the sender's, so they have to be translated on arrival. Nothing in
/// [`PlayerInput`] is an entity: a weapon slot is a number and aim is an
/// angle, deliberately, so that what a client asserts is what it did rather
/// than what it hit.
impl MapEntities for PlayerInput {
    fn map_entities<M: EntityMapper>(&mut self, _mapper: &mut M) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The protocol has to build without a window, a renderer or a socket.
    /// It is also the cheapest possible check that every registered component
    /// still satisfies the bounds replication and prediction put on it — those
    /// are trait bounds, so this failing is a compile error rather than a
    /// failed assertion, which is the point.
    #[test]
    fn the_protocol_registers() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        app.add_plugins(lightyear::prelude::client::ClientPlugins {
            tick_duration: std::time::Duration::from_secs_f64(1.0 / crate::constants::TICK_HZ),
        });
        app.add_plugins(ProtocolPlugin);
        app.finish();
    }
}
