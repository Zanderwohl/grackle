//! Things you saw happen, after they already have.
//!
//! An effect is not state and it is not a consequence — it is the flash a
//! player reads to know a thing occurred. Nothing depends on one: a lost
//! effect costs a tracer, never a disagreement about who is alive. That is
//! what lets them travel on an unreliable footing and be written by the
//! authority alone.
//!
//! **The visuals read this and nothing else.** A blast mark used to come off
//! the `Explosion` message directly, which worked exactly as long as the only
//! machine that mattered was the one deciding the damage. Reading one queue
//! that is filled either locally or off the wire is what makes a client's
//! screen show the same things a server's does, without the drawing knowing
//! which it is.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Something that happened somewhere, worth drawing once.
///
/// Pure geometry, deliberately: no entities, so nothing here needs mapping
/// across the wire, and an effect whose target has already been despawned is
/// still perfectly drawable.
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// A hitscan shot, from the muzzle to wherever it stopped.
    Tracer {
        from: Vec3,
        to: Vec3,
        /// Whether it stopped on a body rather than on the world.
        hit: bool,
    },
    /// Something went off.
    Blast { at: Vec3, radius: f32 },
}

// The queue is registered by `DamagePlugin`, beside `Damage` and for the same
// stated reason: it belongs to the module that outlives any one thing writing
// into it. A weapon that starts producing effects registers nothing.
