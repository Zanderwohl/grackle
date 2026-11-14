//! How a value gets from one keyframe to the next.
//!
//! A closed set of named curves rather than bezier handles: the timeline is
//! Bevy UI, which has no painter to drag tangents in, and a named curve reads
//! in a hand-edited file.
//!
//! The curve belongs to the key being left, not the one being arrived at.
//!
//! Every curve but [`Interpolation::Spline`] is shaped within its own segment, so it either
//! comes to rest at each key or changes speed there. A spline runs through the keys at an
//! unbroken speed, reading the keys either side of its segment; see [`through`].

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// The shape of the ramp between two keys.
///
/// A unit enum so it is a bare string in TOML and survives postcard; see
/// [`clip`](super::clip).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    /// Hold the outgoing value until the next key, then jump.
    Step,
    #[default]
    Linear,
    QuadraticIn,
    QuadraticOut,
    QuadraticInOut,
    CubicIn,
    CubicOut,
    CubicInOut,
    /// `3t² − 2t³`.
    SmoothStep,
    /// Perlin's `6t⁵ − 15t⁴ + 10t³`: zero second derivative at both ends, so a
    /// loop seam does not kink.
    SmootherStep,
    /// Through the keys without a change of speed at any: a Catmull-Rom spline.
    Spline,
}

impl Interpolation {
    /// Every curve, in the order the picker offers them.
    pub const ALL: &'static [Interpolation] = &[
        Interpolation::Step,
        Interpolation::Linear,
        Interpolation::Spline,
        Interpolation::SmoothStep,
        Interpolation::SmootherStep,
        Interpolation::QuadraticIn,
        Interpolation::QuadraticOut,
        Interpolation::QuadraticInOut,
        Interpolation::CubicIn,
        Interpolation::CubicOut,
        Interpolation::CubicInOut,
    ];

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
            Interpolation::Spline => "anim.ease.spline",
        }
    }

    /// Progress along the segment for time fraction `t`, clamped to 0..=1
    /// rather than extrapolated: a cubic run past its end has no bound.
    pub fn ease(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            // Jumps on arrival; rounding would jump half a segment early.
            Interpolation::Step => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            // Alone, without its neighbors, a spline is a straight line.
            Interpolation::Linear | Interpolation::Spline => t,
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

    /// The short way round; see [`Pose::lerp`].
    ///
    /// [`Pose::lerp`]: crate::common::skeleton::Pose::lerp
    pub fn quat(self, from: Quat, to: Quat, t: f32) -> Quat {
        from.slerp(to, self.ease(t))
    }
}

/// A spline's value `t` of the way from `p[1]` to `p[2]`, with `p[0]` before and `p[3]` after.
/// `gaps` are the times between them, in any unit: before, the segment, after. A missing
/// neighbor is the end itself, with a gap of zero.
///
/// The slope at each end is the slope across its two neighbors, so a key spaced unevenly from
/// its neighbors is still passed through at one speed.
pub fn through(p: [f32; 4], gaps: [f32; 3], t: f32) -> f32 {
    let [before, span, after] = gaps;
    if span <= 0.0 {
        return p[1];
    }
    let slope = |a: f32, b: f32, gap: f32| (b - a) / gap * span;
    let leaving = slope(p[0], p[2], before + span);
    let arriving = slope(p[1], p[3], span + after);
    let t = t.clamp(0.0, 1.0);
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * p[1] + (t3 - 2.0 * t2 + t) * leaving + (-2.0 * t3 + 3.0 * t2) * p[2] + (t3 - t2) * arriving
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A curve that misses its ends makes a joint twitch on arrival at a key.
    #[test]
    fn every_curve_joins_its_own_ends() {
        for ease in Interpolation::ALL {
            assert_eq!(ease.ease(0.0), 0.0, "{ease:?} did not start at its start");
            assert_eq!(ease.ease(1.0), 1.0, "{ease:?} did not end at its end");
        }
    }

    /// A non-monotonic curve reads as a bone shaking.
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

    /// Extrapolating past the ends puts a limb somewhere impossible for a frame.
    #[test]
    fn a_curve_clamps_rather_than_extrapolating() {
        for ease in Interpolation::ALL {
            assert_eq!(ease.ease(-4.0), 0.0, "{ease:?} extrapolated backwards");
            assert_eq!(ease.ease(4.0), 1.0, "{ease:?} extrapolated forwards");
        }
    }

    /// A step that jumps mid-segment is early on every key.
    #[test]
    fn a_step_holds_until_it_arrives() {
        for step in 0..100 {
            assert_eq!(Interpolation::Step.ease(step as f32 / 100.0), 0.0);
        }
        assert_eq!(Interpolation::Step.ease(1.0), 1.0);
    }
}
