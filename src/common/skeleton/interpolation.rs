//! How a value gets from one keyframe to the next.
//!
//! A closed set of named curves rather than per-key bezier handles, and that
//! is a decision rather than a stage on the way to handles. Handles are worth
//! having when you can *draw* them — a curve editor with two tangents you drag
//! — and the timeline is Bevy UI, which has no painter. A dropdown of named
//! curves is a control that can actually be built, and a file that says
//! `ease = "cubic_in_out"` is one somebody can read and edit by hand, which
//! four extra floats per key would not be.
//!
//! **The curve belongs to the key you are leaving.** Stated once, here,
//! because the other reading — that it belongs to the key you are arriving at
//! — is equally defensible, and two call sites that disagree is a segment
//! whose curve inverts halfway along it.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// The shape of the ramp between two keys.
///
/// A plain unit enum with no payload, so it is a bare string in TOML and
/// survives postcard — the same reason `BooleanOp` is written this way. See
/// the note on the wire in [`clip`](super::clip).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    /// Hold the outgoing value until the next key, then jump.
    ///
    /// Not a degenerate case to be tidied away: a muzzle flash that is on for
    /// two frames and off after them is a step, and easing it would be a flash
    /// that fades, which is not what a flash does.
    Step,
    #[default]
    Linear,
    QuadraticIn,
    QuadraticOut,
    QuadraticInOut,
    CubicIn,
    CubicOut,
    CubicInOut,
    /// `3t² − 2t³`. Gentle at both ends, and the one most poses want.
    SmoothStep,
    /// Perlin's `6t⁵ − 15t⁴ + 10t³` — flat at both ends in the second
    /// derivative too, so a loop seam does not show as a kink.
    SmootherStep,
}

impl Interpolation {
    /// Every curve, in the order the picker offers them.
    ///
    /// Hand-ordered rather than derived, so related curves sit together: the
    /// enum's own order is the order they were added, and adding a fourth
    /// cubic should not put it beside `Step`.
    pub const ALL: &'static [Interpolation] = &[
        Interpolation::Step,
        Interpolation::Linear,
        Interpolation::SmoothStep,
        Interpolation::SmootherStep,
        Interpolation::QuadraticIn,
        Interpolation::QuadraticOut,
        Interpolation::QuadraticInOut,
        Interpolation::CubicIn,
        Interpolation::CubicOut,
        Interpolation::CubicInOut,
    ];

    /// The lang key this curve's name is under.
    pub fn name_key(self) -> &'static str {
        match self {
            Interpolation::Step => "anim.ease.step",
            Interpolation::Linear => "anim.ease.linear",
            Interpolation::QuadraticIn => "anim.ease.quadratic_in",
            Interpolation::QuadraticOut => "anim.ease.quadratic_out",
            Interpolation::QuadraticInOut => "anim.ease.quadratic_in_out",
            Interpolation::CubicIn => "anim.ease.cubic_in",
            Interpolation::CubicOut => "anim.ease.cubic_out",
            Interpolation::CubicInOut => "anim.ease.cubic_in_out",
            Interpolation::SmoothStep => "anim.ease.smooth_step",
            Interpolation::SmootherStep => "anim.ease.smoother_step",
        }
    }

    /// Where along the segment the value has actually got to, given how far
    /// along it in time we are.
    ///
    /// Clamped rather than extrapolated. Every caller has already decided that
    /// `t` is inside this segment, so a value outside it is a bug upstream,
    /// and extrapolating a cubic past its end is how a bone ends up somewhere
    /// no key ever asked for.
    pub fn ease(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            // Not `t`, and not `t.round()`: the outgoing value holds for the
            // whole segment and the jump happens *on arrival*, which is why
            // the boundary is `>= 1.0` and not `> 0.5`. Rounding would jump
            // half a segment early on every key in the clip.
            Interpolation::Step => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Interpolation::Linear => t,
            Interpolation::QuadraticIn => t * t,
            Interpolation::QuadraticOut => t * (2.0 - t),
            Interpolation::QuadraticInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    let u = t - 1.0;
                    1.0 - 2.0 * u * u
                }
            }
            Interpolation::CubicIn => t * t * t,
            Interpolation::CubicOut => {
                let u = t - 1.0;
                1.0 + u * u * u
            }
            Interpolation::CubicInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    let u = 2.0 * t - 2.0;
                    1.0 + u * u * u * 0.5
                }
            }
            Interpolation::SmoothStep => t * t * (3.0 - 2.0 * t),
            Interpolation::SmootherStep => t * t * t * (t * (t * 6.0 - 15.0) + 10.0),
        }
    }

    pub fn f32(self, from: f32, to: f32, t: f32) -> f32 {
        from + (to - from) * self.ease(t)
    }

    pub fn vec3(self, from: Vec3, to: Vec3, t: f32) -> Vec3 {
        from.lerp(to, self.ease(t))
    }

    /// Rotations take the short way round, for the reason [`Pose::lerp`]
    /// documents: two poses of one body are never more than a half-turn apart
    /// at a joint, and a knee that took the long way bends backwards on the
    /// way there.
    ///
    /// [`Pose::lerp`]: crate::common::skeleton::Pose::lerp
    pub fn quat(self, from: Quat, to: Quat, t: f32) -> Quat {
        from.slerp(to, self.ease(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every curve starts where it started and ends where it ends. A curve
    /// that did not would move a value the key either side of it never asked
    /// for, and the tell is a joint that twitches at the moment it arrives.
    #[test]
    fn every_curve_joins_its_own_ends() {
        for ease in Interpolation::ALL {
            assert_eq!(ease.ease(0.0), 0.0, "{ease:?} did not start at its start");
            assert_eq!(ease.ease(1.0), 1.0, "{ease:?} did not end at its end");
        }
    }

    /// A curve that went backwards partway would run the segment in reverse
    /// for a while — which reads as a bone shaking rather than as an easing
    /// choice. `Step` is the deliberate exception: it is flat and then jumps.
    #[test]
    fn no_curve_goes_backwards() {
        for ease in Interpolation::ALL {
            let mut previous = 0.0;
            for step in 0..=100 {
                let value = ease.ease(step as f32 / 100.0);
                assert!(
                    value >= previous - 1e-6,
                    "{ease:?} went backwards at {step}: {previous} then {value}"
                );
                previous = value;
            }
        }
    }

    /// Past the ends a curve clamps rather than extrapolating. A cubic run
    /// past its own end climbs without bound, and the symptom is one frame
    /// where a limb is somewhere impossible.
    #[test]
    fn a_curve_clamps_rather_than_extrapolating() {
        for ease in Interpolation::ALL {
            assert_eq!(ease.ease(-4.0), 0.0, "{ease:?} extrapolated backwards");
            assert_eq!(ease.ease(4.0), 1.0, "{ease:?} extrapolated forwards");
        }
    }

    /// `Step` holds the outgoing value for the whole segment. Written down
    /// because the obvious implementations — `t`, or rounding — are both a
    /// jump in the *middle*, which is half a frame early on every segment.
    #[test]
    fn a_step_holds_until_it_arrives() {
        for step in 0..100 {
            assert_eq!(Interpolation::Step.ease(step as f32 / 100.0), 0.0);
        }
        assert_eq!(Interpolation::Step.ease(1.0), 1.0);
    }
}
