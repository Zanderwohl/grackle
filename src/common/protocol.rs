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

        // What the step writes, and therefore what a client is told. Not
        // predicted: the server simulates and the client draws, so there is
        // exactly one writer for each of these anywhere in the system.
        // Prediction belongs on top of that, added once, not spread through
        // every system that touches a body.
        app.component::<PhysicsBody>().replicate();
        app.component::<Player>().replicate();
        app.component::<Stance>().replicate();

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
