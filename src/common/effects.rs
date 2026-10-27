//! Things you saw happen: tracers, blasts and shots. Nothing depends on one, so a lost
//! effect costs a flash, never a disagreement about who is alive.
//!
//! The visuals read only this queue, filled locally on the authority or off
//! the wire on a client, so the drawing does not know which it is.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::damage::PlayerId;

/// Pure geometry, no entities: nothing to map across the wire, and still
/// drawable after its target is despawned.
#[derive(Message, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    Tracer {
        from: Vec3,
        to: Vec3,
        /// Stopped on a body rather than the world.
        hit: bool,
        /// So the shooter's own machine can draw it from the barrel.
        by: Option<PlayerId>,
    },
    Blast { at: Vec3, radius: f32 },
    /// A weapon went off: every machine starts that body's recoil. The
    /// shooter's own machine has already started it from its own input.
    Fired { by: PlayerId, button: crate::game::weapon::Button },
}

// Registered by `DamagePlugin`, so a weapon that produces effects registers
// nothing.
