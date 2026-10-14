//! Things you saw happen: tracers and blasts. Nothing depends on one, so a lost
//! effect costs a flash, never a disagreement about who is alive.
//!
//! The visuals read only this queue, filled locally on the authority or off
//! the wire on a client, so the drawing does not know which it is.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Pure geometry, no entities: nothing to map across the wire, and still
/// drawable after its target is despawned.
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    Tracer {
        from: Vec3,
        to: Vec3,
            /// Stopped on a body rather than the world.
        hit: bool,
    },
    Blast { at: Vec3, radius: f32 },
}

// Registered by `DamagePlugin`, so a weapon that produces effects registers
// nothing.
