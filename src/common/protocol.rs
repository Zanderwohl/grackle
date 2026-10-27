//! What goes over the wire, and which end is believed about it.
//!
//! One binary, so both ends register the same components in the same order;
//! Lightyear hashes the registries and refuses a handshake when they differ.
//!
//! Must be added after the client and server plugin groups and before any
//! link entity is spawned — see [`NetTransportPlugin`].
//!
//! [`NetTransportPlugin`]: crate::common::net_transport::NetTransportPlugin

use bevy::ecs::entity::{EntityMapper, MapEntities};
use bevy::prelude::*;
use lightyear::prelude::input::native::InputPlugin;
use lightyear::prelude::*;

use crate::common::class::Stance;
use crate::common::damage::{Armor, Damageable, PlayerId};
use crate::common::skeleton::BodyRequests;
use crate::game::player::{PhysicsBody, Player, PlayerInput};

pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // Client to server, in a redundant queue of the last several ticks so
        // one lost packet is not one lost step.
        app.add_plugins(InputPlugin::<PlayerInput>::default());

        // Everything the step reads back, so a rollback replays from the right
        // state: position, velocity and on-ground flag, and hull height.
        app.component::<PhysicsBody>().replicate().predict();
        app.component::<Player>().replicate().predict();
        app.component::<Stance>().replicate().predict();

        // Poses are never sent; each process runs `AnimationState` over these
        // nine bools. `Gait` is derived from `PhysicsBody`.
        app.component::<BodyRequests>().replicate();

        // Not predicted: a wrongly guessed kill is a body that falls over and
        // stands back up.
        app.component::<Damageable>().replicate();
        app.component::<Armor>().replicate();
        app.component::<crate::game::score::Standing>().replicate();

        // With `PhysicsBody`, everything a viewer needs to draw a body somebody
        // else drives, grid bodies included.
        app.component::<crate::common::class::Class>().replicate_once();
        // Once: a new side is a new life, which is a new body.
        app.component::<crate::common::team::Team>().replicate_once();
        app.component::<crate::common::player_color::PlayerColor>().replicate_once();
        app.component::<crate::common::skeleton::ForcedAnimation>().replicate();
        app.component::<crate::common::skeleton::gait::DisplaySpeed>().replicate();

        // Hides the body for the tick it outlives itself by, so it does not
        // stand inside its own corpse until the despawn arrives.
        app.component::<crate::game::death::Dying>().replicate_once();

        // A corpse's seed only; every machine runs the same solver from it.
        // Raising corpses locally from a death cannot work: the death and the
        // despawn arrive together, so the pose is gone before it can be copied.
        app.component::<crate::game::ragdoll::Ragdoll>().replicate_once();
        // The build is read off the rig, not a `Class`, which not every body has.
        app.component::<crate::game::ragdoll::CorpseBuild>().replicate_once();
        app.component::<crate::game::ragdoll::CorpseOf>().replicate_once();

        // Replicated rather than re-simulated: see `replicate_projectiles`.
        app.component::<crate::game::projectile::Projectile>().replicate();

        // One component rather than two: replication does not promise to
        // deliver two components in one packet. Not predicted, so there is
        // one writer.
        app.component::<crate::common::weapon::Equipped>().replicate();

        // Not predicted, though a client's own body reads it: one writer.
        app.component::<crate::game::condition::Condition>().replicate();

        // Separate from `Equipped` because it changes every shot, and
        // replication is change-detected per component.
        app.component::<crate::common::weapon::Ammo>().replicate();

        // Placed props: the model and where, and whether a pickup is there to
        // take. A breakable's health is its `Damageable`, above.
        app.component::<crate::game::world_prop::WorldProp>().replicate();
        app.component::<crate::game::world_prop::Pickup>().replicate();

        // Once. Animation phase is derived from it, so every client puts a body
        // at the same point in its cycle.
        app.component::<PlayerId>().replicate_once();
    }
}

/// Nothing in [`PlayerInput`] is an entity, deliberately: a client asserts
/// what it did (slot, aim), not what it hit.
impl MapEntities for PlayerInput {
    fn map_entities<M: EntityMapper>(&mut self, _mapper: &mut M) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fails to compile if a registered component stops satisfying the
    /// replication or prediction bounds, and fails at runtime if a
    /// registration needs a window, renderer or socket.
    #[test]
    fn the_protocol_registers() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.add_plugins(bevy::time::TimePlugin);
        // A model's clips are assets, and `PropSyncPlugin` registers them.
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(lightyear::prelude::client::ClientPlugins {
            tick_duration: crate::constants::one_tick(),
        });
        app.add_plugins(ProtocolPlugin);
        app.add_plugins(crate::common::pack_sync::PackSyncPlugin);
        app.add_plugins(crate::prop::sync::PropSyncPlugin);
        app.finish();
    }
}
