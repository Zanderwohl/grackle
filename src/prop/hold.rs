//! How a weapon is held: where the hands go on it, and where it is carried.
//!
//! **A hold is two points and an offset, not a pose.** That is the whole of
//! why any weapon can be held while the body does anything: the arms are
//! *solved* to the points rather than posed to match the weapon, so a hold is
//! the same data for every class and at every frame of a walk cycle. See
//! [the plan](../../documentation/weapons-in-hand.md).
//!
//! It lives on the prop because it is a fact about the model's *shape*, and
//! because the prop editor is where a reference figure is already standing to
//! judge it against. Nothing authors one yet — every prop uses the default,
//! which the grip convention makes a sensible one: a prop's origin *is* its
//! grip, so the only guess here is where the support hand goes and how far out
//! the thing is carried.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoldSpec {
    /// Where the trigger hand grips, in the weapon's own space.
    ///
    /// The origin, by the convention every prop is authored to — so this is
    /// only here for a model that wants to disagree.
    #[serde(default, with = "crate::prop::nice_f32::array")]
    pub grip: [f32; 3],
    /// Where the support hand grips, when there is one.
    #[serde(default = "default_support", with = "crate::prop::nice_f32::array")]
    pub support: [f32; 3],
    /// Whether the off hand comes onto the weapon at all.
    ///
    /// A flag rather than an `Option<[f32; 3]>`, because **TOML has no null**:
    /// an absent field would have to mean one-handed, so a `[hold]` section
    /// that set only the carry would silently lose the support hand. Stated
    /// outright, every field defaults to what it defaults to and a partial
    /// hold behaves like a whole one.
    #[serde(default = "default_two_handed")]
    pub two_handed: bool,
    /// Where the grip is carried relative to the point between the shoulders,
    /// **in fractions of arm length**.
    ///
    /// A fraction rather than metres for the reason a crouch's root offset is
    /// in hip-heights: a Heavy's arms are not a Scout's, and a carry authored
    /// in metres would put one of them out of its own reach.
    #[serde(default = "default_carry", with = "crate::prop::nice_f32::array")]
    pub carry: [f32; 3],
}

fn default_support() -> [f32; 3] {
    HoldSpec::default().support
}

fn default_two_handed() -> bool {
    HoldSpec::default().two_handed
}

fn default_carry() -> [f32; 3] {
    HoldSpec::default().carry
}

impl Default for HoldSpec {
    fn default() -> Self {
        Self {
            grip: [0.0; 3],
            support: [0.0, 0.0, -0.07],
            two_handed: true,
            // Close to the centreline on purpose. A weapon carried out at the
            // shoulder is a weapon the *other* hand cannot reach, which the
            // reference figure only gets away with by twisting its torso —
            // something a body being animated by the state machine does not do.
            carry: [0.05, -0.14, -0.22],
        }
    }
}

impl HoldSpec {
    pub fn grip(&self) -> Vec3 {
        Vec3::from_array(self.grip)
    }

    pub fn support(&self) -> Option<Vec3> {
        self.two_handed.then(|| Vec3::from_array(self.support))
    }

    /// Where the grip is carried, in metres, for arms this long.
    pub fn carry(&self, arm_length: f32) -> Vec3 {
        Vec3::from_array(self.carry) * arm_length
    }
}
