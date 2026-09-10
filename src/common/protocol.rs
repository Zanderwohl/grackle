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
use crate::common::damage::PlayerId;
use crate::game::player::{PhysicsBody, Player, PlayerInput};

/// Registers everything the two ends have to agree about.
pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // The input is not replicated like a component: it travels the other
        // way, client to server, in its own redundant queue that carries the
        // last several ticks so that one lost packet is not one lost step.
        app.add_plugins(InputPlugin::<PlayerInput>::default());

        // Predicted, all three, because all three are what the step writes
        // and therefore what has to be rewound and replayed when the server
        // disagrees. `PhysicsBody` is the position; `Player` carries the
        // velocity and the on-ground flag, without which a replayed step
        // starts from the wrong state and diverges again immediately; and
        // `Stance` decides how big the hull being swept is.
        app.component::<PhysicsBody>().replicate().predict();
        app.component::<Player>().replicate().predict();
        app.component::<Stance>().replicate().predict();

        // Sent once, on the insert that first carries it. An id is a fact
        // about who a body belongs to, not a value that changes per tick, and
        // re-sending it every update would be bandwidth spent restating it.
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
