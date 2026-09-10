//! Telling clients what happened, as against what is.
//!
//! Replication carries **state**: a body's health is a component, so a client
//! is told the number and can draw a health bar from it. It cannot carry
//! **events** — a hit landed here, for this much, by this person — because an
//! event is not a value anything holds afterwards. Two hits of thirty in one
//! tick and one hit of sixty leave a body at exactly the same health, and a
//! client watching only the number would draw one number instead of two, in
//! the wrong place, credited to nobody.
//!
//! So the records travel as messages. They are **feedback only** on the
//! receiving end: nothing a client does with a `DamageDealt` changes a health
//! pool, because `DamageSystems` does not run there. Arriving late, arriving
//! out of order, or not arriving at all costs a floating number, never a
//! disagreement about who is alive.

use bevy::prelude::*;
use lightyear::prelude::*;

use crate::common::damage::{DamageDealt, Died};
use crate::common::effects::Effect;
use crate::common::net::{has_authority, is_remote_client, NetRole};
use crate::game::ragdoll::RagdollShove;

/// What happened, as against what is.
///
/// Unordered: each record stands alone, and two hits landing in the wrong
/// order draw two numbers in the wrong order, which nobody can perceive.
/// Reliable, because a dropped kill is a kill feed that never mentions it.
pub struct EventChannel;

/// Relays the damage layer's records to clients.
pub struct NetEventsPlugin;

impl Plugin for NetEventsPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<EventChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::ServerToClient);

        // `add_map_entities`, because the entity in a record is the sender's.
        // Without it a damage number would be anchored to whatever entity
        // happened to share that index on the receiving machine.
        app.register_message::<DamageDealt>()
            .add_map_entities()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<Died>()
            .add_map_entities()
            .add_direction(NetworkDirection::ServerToClient);
        // No `add_map_entities`: an effect is pure geometry, which is what
        // lets it be drawn even when whatever it happened to has already gone.
        app.register_message::<Effect>()
            .add_direction(NetworkDirection::ServerToClient);
        // A shove is geometry too — a point, a direction and a radius — and
        // needs no mapping for the same reason. It is relayed because a
        // corpse's simulation is otherwise identical on every machine: the
        // seed crosses, the solver is the same, and the only thing that would
        // differ is that nobody but the server ever saw the rocket hit it.
        app.register_message::<RagdollShove>()
            .add_direction(NetworkDirection::ServerToClient);

        // Sending in `Update`, after the tick that produced the records.
        app.add_systems(
            Update,
            (send_hits, send_deaths, send_effects, send_shoves).run_if(has_authority),
        );
        // **Receiving in `PreUpdate`**, before anything acts on what arrived —
        // and that is not tidiness. A shove is read from `FixedUpdate`, which
        // runs before `Update`, so a record taken in `Update` is not read
        // until the next frame's fixed loop and lands a frame late on a corpse
        // that has already begun to fall.
        app.add_systems(
            PreUpdate,
            (receive_hits, receive_deaths, receive_effects, receive_shoves)
                .after(MessageSystems::Receive)
                .run_if(is_remote_client),
        );
    }
}

/// Pass on every hit the damage layer resolved this frame.
///
/// Reads the same local message the floating numbers read, rather than being
/// written by `apply_damage` itself. That keeps the damage layer unaware there
/// is a network at all: a new source of damage joins `DamageSystems::Deal` and
/// is relayed without naming itself here, which is the same reason the sets
/// exist in the first place.
fn send_hits(
    mut hits: MessageReader<DamageDealt>,
    server: Option<Single<&Server>>,
    mut sender: ServerMultiMessageSender,
) -> Result {
    let Some(server) = server else {
        // A closed game. Draining is not needed — the local readers have their
        // own cursors — so there is simply nothing to do.
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

/// Put arriving hits into the same local queue a server's own would be in.
///
/// Everything downstream — the floating numbers, and a scoreboard when there
/// is one — then reads one queue and never asks where a record came from.
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

/// Pass on every effect the authority produced this frame.
///
/// The same shape as the damage records and for the same reason: what happened
/// is not a value anything holds afterwards. A blast is over by the time the
/// health it took is replicated, and a tracer never was a value at all.
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

/// Pass on every push a corpse was given.
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

/// Show everybody the corpse the authority just made.
///
/// A corpse is a prop that used to be a person: nothing can touch one, nothing
/// collides with one, and it takes no input — so it goes on the wire the way
/// any other prop in the world does. What crosses is the seed only, and the
/// solver on each machine takes it from there.
pub fn replicate_corpses(
    mut commands: Commands,
    role: Res<NetRole>,
    raised: Query<Entity, (Added<crate::game::ragdoll::Ragdoll>, Without<Replicate>)>,
) {
    if !matches!(*role, NetRole::Listen { .. }) {
        return;
    }
    for corpse in &raised {
        commands
            .entity(corpse)
            .insert(Replicate::to_clients(NetworkTarget::All));
    }
}

/// Start replicating anything that goes bang, so everyone can see it coming.
///
/// A projectile is **replicated, not re-simulated**. Each client could step
/// the same spec from the same origin and get the same arc — the map is the
/// same and the maths is deterministic — right up until it meets a body, and
/// a client's copy of a remote body is always a little behind the server's. A
/// locally simulated rocket would detonate against a player who, on the
/// server, was never there: a puff of smoke and no damage, which reads as the
/// game losing a hit that plainly landed.
///
/// So the server flies it and everybody watches. `PhysicsBody` carries the
/// position and `interpolate_bodies` smooths it, exactly as for a body.
pub fn replicate_projectiles(
    mut commands: Commands,
    role: Res<NetRole>,
    launched: Query<Entity, (Added<crate::game::projectile::Projectile>, Without<Replicate>)>,
) {
    if !matches!(*role, NetRole::Listen { .. }) {
        return;
    }
    for projectile in &launched {
        commands
            .entity(projectile)
            .insert(Replicate::to_clients(NetworkTarget::All));
    }
}

/// Show everybody the bodies standing in an animation grid.
///
/// The authority stands them up — they have health and hitboxes, so somebody
/// has to be believed about whether each is alive — and everybody else is
/// handed the result. What crosses is a position, a build and an animation;
/// `CarouselBody` and `OfGrid` stay here, because a viewer drawing a body does
/// not need to know it is part of a roster.
pub fn replicate_display_bodies(
    mut commands: Commands,
    role: Res<NetRole>,
    standing: Query<Entity, (Added<crate::tool::animation_grid::CarouselBody>, Without<Replicate>)>,
) {
    if !matches!(*role, NetRole::Listen { .. }) {
        return;
    }
    for body in &standing {
        commands
            .entity(body)
            .insert(Replicate::to_clients(NetworkTarget::All));
    }
}
