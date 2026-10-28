//! An invisible box that does something to whoever stands in it. Stood up on
//! every machine by the `Trigger` map feature, and acted on by the authority
//! in [`crate::game::trigger`].

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum TriggerEffect {
    Hurt { per_second: u32 },
    /// Empties the health pool outright rather than dealing damage, so armor
    /// and teams do not enter into it.
    Kill,
}

impl Default for TriggerEffect {
    fn default() -> Self {
        TriggerEffect::Hurt { per_second: 10 }
    }
}

#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct TriggerVolume {
    pub min: Vec3,
    pub max: Vec3,
    pub effect: TriggerEffect,
}

impl TriggerVolume {
    /// Strict, like the pickups': touching is not overlapping.
    pub fn overlaps(&self, center: Vec3, half: Vec3) -> bool {
        (0..3).all(|axis| center[axis] + half[axis] > self.min[axis] && center[axis] - half[axis] < self.max[axis])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_beside_the_volume_is_not_in_it() {
        let volume = TriggerVolume { min: Vec3::ZERO, max: Vec3::ONE, effect: TriggerEffect::Kill };
        let half = Vec3::splat(0.5);
        assert!(volume.overlaps(Vec3::new(1.4, 0.5, 0.5), half));
        assert!(!volume.overlaps(Vec3::new(1.5, 0.5, 0.5), half), "touching");
        assert!(!volume.overlaps(Vec3::new(0.5, -0.6, 0.5), half));
    }
}
