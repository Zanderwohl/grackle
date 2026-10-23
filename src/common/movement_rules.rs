//! How bodies move on a given map: the map's settings, read by the step.
//!
//! The map's metadata is what is saved, edited and sent; this resource is a
//! copy of its movement section, so the simulation reads one small value
//! rather than reaching into the editor.

use std::fmt::Display;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::get;

/// Metres per second squared, downwards.
pub const DEFAULT_GRAVITY: f32 = 20.0;

/// What holding the sprint key does.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, EnumIter, Serialize, Deserialize)]
pub enum SprintPolicy {
    /// Always at sprint speed; the key does nothing.
    #[default]
    Always,
    /// Walk, and sprint while the key is held.
    Enabled,
    /// Always at walking speed; the key does nothing.
    Disabled,
}

impl SprintPolicy {
    pub fn sprints(self, held: bool) -> bool {
        match self {
            SprintPolicy::Always => true,
            SprintPolicy::Enabled => held,
            SprintPolicy::Disabled => false,
        }
    }
}

impl Display for SprintPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SprintPolicy::Always => write!(f, "{}", get!("movement.sprint.always")),
            SprintPolicy::Enabled => write!(f, "{}", get!("movement.sprint.enabled")),
            SprintPolicy::Disabled => write!(f, "{}", get!("movement.sprint.disabled")),
        }
    }
}

#[derive(Resource, Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Movement", default)]
pub struct MovementRules {
    pub sprint: SprintPolicy,
    /// Metres per second squared, downwards.
    pub gravity: f32,
}

impl Default for MovementRules {
    fn default() -> Self {
        Self {
            sprint: SprintPolicy::default(),
            gravity: DEFAULT_GRAVITY,
        }
    }
}

impl MovementRules {
    /// Vertical acceleration, signed: negative is down.
    pub fn fall(&self) -> f32 {
        -self.gravity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_enabled_sprint_listens_to_the_key() {
        for held in [false, true] {
            assert!(SprintPolicy::Always.sprints(held));
            assert!(!SprintPolicy::Disabled.sprints(held));
            assert_eq!(SprintPolicy::Enabled.sprints(held), held);
        }
    }
}
