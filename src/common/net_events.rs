//! Relaying events to clients, as against replicating state.
//!
//! Two hits of thirty and one of sixty leave the same health, so damage
//! records, deaths, effects and shoves travel as messages. On a client they
//! are feedback only, since `DamageSystems` does not run there: a lost or
//! reordered record costs a floating number, never agreement on who is alive.

use bevy::prelude::*;
use lightyear::prelude::*;

use crate::common::damage::{DamageDealt, Died};
use crate::common::effects::Effect;
use crate::common::net::{has_authority, is_remote_client};
use crate::game::ragdoll::RagdollShove;

/// Unordered: two numbers drawn in the wrong order is imperceptible. Reliable:
/// a dropped kill never reaches the kill feed.
pub struct EventChannel;

pub struct NetEventsPlugin;

impl Plugin for NetEventsPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<EventChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ServerToClient);

        // The entity in a record is the sender's, so it has to be mapped.
        app.register_message::<DamageDealt>()
            .add_map_entities()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<Died>()
            .add_map_entities()
            .add_direction(NetworkDirection::ServerToClient);
        // Effects and shoves are pure geometry, so need no mapping and can be
        // drawn after their subject is gone.
        app.register_message::<Effect>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<RagdollShove>()
            .add_direction(NetworkDirection::ServerToClient);

        app.add_systems(
            Update,
            (send_hits, send_deaths, send_effects, send_shoves).run_if(has_authority),
        );
        // `PreUpdate`: shoves are read in `FixedUpdate`, which runs before
        // `Update`, so receiving in `Update` would land a frame late.
        app.add_systems(
            PreUpdate,
            (receive_hits, receive_deaths, receive_effects, receive_shoves)
                .after(MessageSystems::Receive)
                .run_if(is_remote_client),
        );
    }
}

/// Reads the same local queue the floating numbers read, so the damage layer
/// does not know there is a network and a new damage source is relayed
/// without naming itself here.
fn send_hits(
    mut hits: MessageReader<DamageDealt>,
    server: Option<Single<&Server>>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    let Some(server) = server else {
        // A closed game; local readers have their own cursors, so no draining.
        return Ok(());
    };
    for hit in hits.read() {
        sender.send::<_, EventChannel>(hit, &server, &NetworkTarget::All)?;
    }
    Ok(())
}

fn send_deaths(
    mut deaths: MessageReader<Died>,
    server: Option<Single<&Server>>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    let Some(server) = server else { return Ok(()) };
    for death in deaths.read() {
        sender.send::<_, EventChannel>(death, &server, &NetworkTarget::All)?;
    }
    Ok(())
}

/// Arriving records go into the same local queue a server's own would, so
/// nothing downstream asks where a record came from.
fn receive_hits(
    mut receivers: Query<&mut MessageReceiver<DamageDealt>>,
    mut hits: MessageWriter<DamageDealt>,
) {
    for mut receiver in &mut receivers {
        for hit in receiver.receive() {
            hits.write(hit);
        }
    }
}

fn receive_deaths(
    mut receivers: Query<&mut MessageReceiver<Died>>,
    mut deaths: MessageWriter<Died>,
) {
    for mut receiver in &mut receivers {
        for death in receiver.receive() {
            deaths.write(death);
        }
    }
}

fn send_effects(
    mut effects: MessageReader<Effect>,
    server: Option<Single<&Server>>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    let Some(server) = server else { return Ok(()) };
    for effect in effects.read() {
        sender.send::<_, EventChannel>(effect, &server, &NetworkTarget::All)?;
    }
    Ok(())
}

fn receive_effects(
    mut receivers: Query<&mut MessageReceiver<Effect>>,
    mut effects: MessageWriter<Effect>,
) {
    for mut receiver in &mut receivers {
        for effect in receiver.receive() {
            effects.write(effect);
        }
    }
}

fn send_shoves(
    mut shoves: MessageReader<RagdollShove>,
    server: Option<Single<&Server>>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    let Some(server) = server else { return Ok(()) };
    for shove in shoves.read() {
        sender.send::<_, EventChannel>(shove, &server, &NetworkTarget::All)?;
    }
    Ok(())
}

fn receive_shoves(
    mut receivers: Query<&mut MessageReceiver<RagdollShove>>,
    mut shoves: MessageWriter<RagdollShove>,
) {
    for mut receiver in &mut receivers {
        for shove in receiver.receive() {
            shoves.write(shove);
        }
    }
}

/// What crosses is the seed only; each machine's solver runs from it.
pub fn replicate_corpses(
    mut commands: Commands,
    raised: Query<Entity, (With<crate::game::ragdoll::Ragdoll>, Without<Replicate>)>,
) {
    for corpse in &raised {
        commands
            .entity(corpse)
            .insert(Replicate::to_clients(NetworkTarget::All));
    }
}

/// Replicated rather than re-simulated: a client's copy of a remote body lags
/// the server's, so a locally flown rocket would hit a player who was never
/// there.
pub fn replicate_projectiles(
    mut commands: Commands,
    launched: Query<Entity, (With<crate::game::projectile::Projectile>, Without<Replicate>)>,
) {
    for projectile in &launched {
        commands
            .entity(projectile)
            .insert(Replicate::to_clients(NetworkTarget::All));
    }
}

/// The authority owns these bodies because they have health. `CarouselBody`
/// and `OfGrid` are not sent; a viewer needs only the position, build and
/// animation.
pub fn replicate_display_bodies(
    mut commands: Commands,
    standing: Query<Entity, (With<crate::tool::animation_grid::CarouselBody>, Without<Replicate>)>,
) {
    for body in &standing {
        commands
            .entity(body)
            .insert(Replicate::to_clients(NetworkTarget::All));
    }
}
