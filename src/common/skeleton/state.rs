//! What a body is doing, and who gets to say so.
//!
//! The rule this module exists to enforce: **nothing outside picks an
//! animation.** A local player, an NPC, a body being driven by the network and
//! a preview standing in the editor all have completely different ideas of
//! where their information comes from, and if each of them chose a clip you
//! would have four copies of the same state machine drifting apart — the walk
//! that only breaks for remote players is exactly that bug.
//!
//! So there are two components and one direction of flow:
//!
//! - [`BodyRequests`] is a description of the situation: running forward,
//!   airborne, a wall in front. Anything may write it, and several systems
//!   can each own a field of it — input writes what is being asked for, the
//!   physics step writes what the world did about it. It contains no
//!   animation names at all.
//! - [`SkeletonAnimator`] is the state machine. It reads requests, decides
//!   which [`AnimationState`] follows from them, and is the only thing that
//!   writes a [`Pose`]. Transitions, blending and dwell times are its problem
//!   and nobody else's.
//!
//! [`ForcedAnimation`] is the deliberate exception: it pins a body to one
//! state regardless of requests, which is what the editor's Animation Display
//! feature needs — there is no situation to describe, a mapper is asking to
//! see a particular state.
//!
//! There are no animation clips yet, so every state currently poses as the
//! A-pose. The machine is still worth having now: it is the seam everything
//! else is written against, and the transitions are testable before there is
//! anything to look at.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use crate::common::skeleton::rig::Pose;
use crate::get;

/// How long a state has to hold before another one can take over.
///
/// Without it, a body pressed against a wall alternates between running and
/// pushing at whatever rate collision happens to resolve, and the animation
/// flickers. A tenth of a second is below the threshold where a real
/// transition feels delayed and above the rate anything chatters at.
const MIN_DWELL: f32 = 0.1;

/// What is happening to a body, in terms anything can observe.
///
/// Deliberately not "which animation": these are facts about the world and
/// the body's intent, so an NPC's planner and a network packet can fill in the
/// same struct that keyboard input does.
///
/// Each field is *assigned* by whoever owns it, every frame, rather than
/// accumulated — a field nobody writes stays false, and a writer that stops
/// having an opinion clears its own field. That is why there is no reset
/// system: a stale `wall_ahead` that nothing ever cleared would be a body
/// pushing at thin air forever.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct BodyRequests {
    /// Being asked to move forwards — a held W, an NPC's path, a replayed
    /// input. Asked for, not achieved: a body walking into a wall is still
    /// running forward, and the difference is [`BodyRequests::wall_ahead`].
    pub running_forward: bool,
    /// Being asked to move backwards.
    pub running_backward: bool,
    /// Being asked to move sideways, either way.
    pub strafing: bool,
    /// Movement is being stopped by something solid in the direction asked
    /// for. Written by whatever ran the movement, because it is the only thing
    /// that knows.
    pub wall_ahead: bool,
    /// Not standing on anything.
    pub airborne: bool,
}

impl BodyRequests {
    /// Being asked to move at all, in any direction.
    pub fn moving(&self) -> bool {
        self.running_forward || self.running_backward || self.strafing
    }
}

/// One animation state. There are no clips behind them yet.
///
/// The order of these is on disk — the editor's Animation Display feature
/// stores the state as [`AnimationState::index`] — so **add new states at the
/// end** and do not reorder. A shuffled list would silently repoint every
/// saved display at a different animation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, EnumIter)]
pub enum AnimationState {
    #[default]
    Idle,
    RunForward,
    RunBackward,
    Strafe,
    /// Asking to move forwards into something solid. Its own state rather than
    /// a variant of running, because a body whose legs keep cycling against a
    /// wall is the single most obvious animation bug there is.
    PushingWall,
    Airborne,
}

impl AnimationState {
    /// Stable index for the save format. See the type's note about ordering.
    pub fn index(&self) -> u32 {
        match self {
            AnimationState::Idle => 0,
            AnimationState::RunForward => 1,
            AnimationState::RunBackward => 2,
            AnimationState::Strafe => 3,
            AnimationState::PushingWall => 4,
            AnimationState::Airborne => 5,
        }
    }

    /// The state an index names, or [`AnimationState::Idle`] for one this
    /// build does not know.
    ///
    /// A map saved by a later build naming a state this one has never heard of
    /// is a display that stands there idle, not a refusal to open the map.
    pub fn from_index(index: u32) -> AnimationState {
        match index {
            1 => AnimationState::RunForward,
            2 => AnimationState::RunBackward,
            3 => AnimationState::Strafe,
            4 => AnimationState::PushingWall,
            5 => AnimationState::Airborne,
            _ => AnimationState::Idle,
        }
    }

    pub fn name(&self) -> String {
        match self {
            AnimationState::Idle => get!("animation.states.idle"),
            AnimationState::RunForward => get!("animation.states.run_forward"),
            AnimationState::RunBackward => get!("animation.states.run_backward"),
            AnimationState::Strafe => get!("animation.states.strafe"),
            AnimationState::PushingWall => get!("animation.states.pushing_wall"),
            AnimationState::Airborne => get!("animation.states.airborne"),
        }
    }

    /// The state that follows from a situation.
    ///
    /// Priority order, most overriding first: being in the air beats anything
    /// the legs were asked to do, and running into a wall beats running.
    pub fn from_requests(requests: &BodyRequests) -> AnimationState {
        if requests.airborne {
            AnimationState::Airborne
        } else if requests.running_forward && requests.wall_ahead {
            AnimationState::PushingWall
        } else if requests.running_forward {
            AnimationState::RunForward
        } else if requests.running_backward {
            AnimationState::RunBackward
        } else if requests.strafing {
            AnimationState::Strafe
        } else {
            AnimationState::Idle
        }
    }

    /// Transitions that must not wait for [`MIN_DWELL`].
    ///
    /// Leaving the ground and landing are the two the eye catches: a jump that
    /// starts a tenth of a second late reads as the body being dragged
    /// upwards, where a walk that starts late reads as nothing at all.
    fn is_urgent(&self) -> bool {
        matches!(self, AnimationState::Airborne)
    }

    /// The pose this state is in, `seconds` into it.
    ///
    /// Every state is the A-pose today, because there are no clips. When there
    /// are, this is where a clip is sampled — and the sample is a [`Pose`], so
    /// the same clip serves every set of proportions.
    pub fn pose(&self, _seconds: f32) -> Pose {
        Pose::rest()
    }
}

/// The state machine, and the only thing that writes a [`Pose`].
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct SkeletonAnimator {
    state: AnimationState,
    /// How long the current state has been running, which is what a clip is
    /// sampled at and what [`MIN_DWELL`] is measured against.
    elapsed: f32,
}

impl SkeletonAnimator {
    pub fn state(&self) -> AnimationState {
        self.state
    }

    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// Advance by `dt`, moving to the state `requests` calls for if this one
    /// has had its turn.
    pub fn advance(&mut self, requests: &BodyRequests, dt: f32) {
        self.step_towards(AnimationState::from_requests(requests), dt);
    }

    /// Advance by `dt` in a state chosen for us. See [`ForcedAnimation`].
    pub fn force(&mut self, state: AnimationState, dt: f32) {
        self.step_towards(state, dt);
    }

    fn step_towards(&mut self, wanted: AnimationState, dt: f32) {
        self.elapsed += dt;
        if wanted == self.state {
            return;
        }
        // Either the state has had its dwell, or the new one is too important
        // to wait — leaving the state matters as much as arriving, so a jump
        // out of a hundredth-of-a-second idle still plays.
        if self.elapsed >= MIN_DWELL || wanted.is_urgent() || self.state.is_urgent() {
            self.state = wanted;
            self.elapsed = 0.0;
        }
    }

    pub fn pose(&self) -> Pose {
        self.state.pose(self.elapsed)
    }
}

/// Pin a body to one state and ignore its requests.
///
/// For showing an animation rather than driving a character: the editor's
/// Animation Display feature, and anything else that wants a body to stand
/// there doing one named thing.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ForcedAnimation(pub AnimationState);

#[cfg(test)]
mod tests {
    use super::*;

    /// The example the whole arrangement is for: two independent facts, one
    /// written by input and one by collision, and neither writer knows what
    /// animation comes out.
    #[test]
    fn running_at_a_wall_is_not_running() {
        let mut requests = BodyRequests::default();
        requests.running_forward = true;
        assert_eq!(AnimationState::from_requests(&requests), AnimationState::RunForward);

        requests.wall_ahead = true;
        assert_eq!(AnimationState::from_requests(&requests), AnimationState::PushingWall);

        // A wall in front of a body that is not going anywhere is just a wall.
        requests.running_forward = false;
        assert_eq!(AnimationState::from_requests(&requests), AnimationState::Idle);
    }

    /// Being in the air beats whatever the legs were asked for.
    #[test]
    fn airborne_overrides_everything_else() {
        let requests = BodyRequests {
            running_forward: true,
            wall_ahead: true,
            airborne: true,
            ..default()
        };
        assert_eq!(AnimationState::from_requests(&requests), AnimationState::Airborne);
    }

    /// A body scraping along a wall flickers `wall_ahead` at the rate
    /// collision resolves it. The machine has to sit on that rather than
    /// passing it through to the animation.
    #[test]
    fn a_flickering_request_does_not_flicker_the_state() {
        let mut animator = SkeletonAnimator::default();
        let mut requests = BodyRequests { running_forward: true, ..default() };
        animator.advance(&requests, 0.5);
        assert_eq!(animator.state(), AnimationState::RunForward);

        // One frame's worth of wall, gone again next frame.
        requests.wall_ahead = true;
        animator.advance(&requests, 1.0 / 64.0);
        requests.wall_ahead = false;
        animator.advance(&requests, 1.0 / 64.0);

        assert_eq!(animator.state(), AnimationState::RunForward, "the wall touch changed the animation");
    }

    /// Held long enough, though, it has to actually change.
    #[test]
    fn a_request_that_persists_wins() {
        let mut animator = SkeletonAnimator::default();
        let requests = BodyRequests { running_forward: true, wall_ahead: true, ..default() };
        for _ in 0..16 {
            animator.advance(&requests, 1.0 / 64.0);
        }
        assert_eq!(animator.state(), AnimationState::PushingWall);
    }

    /// Leaving the ground and landing are exempt from the dwell: a jump that
    /// starts late looks like the body is being dragged upwards.
    #[test]
    fn jumping_and_landing_are_immediate() {
        let mut animator = SkeletonAnimator::default();
        let running = BodyRequests { running_forward: true, ..default() };
        animator.advance(&running, 0.5);

        let jumping = BodyRequests { running_forward: true, airborne: true, ..default() };
        animator.advance(&jumping, 1.0 / 64.0);
        assert_eq!(animator.state(), AnimationState::Airborne, "the jump waited for the dwell");

        // And landing immediately after, without having dwelt in the air.
        animator.advance(&running, 1.0 / 64.0);
        assert_eq!(animator.state(), AnimationState::RunForward, "the landing waited for the dwell");
    }

    /// The elapsed time is what a clip will be sampled at, so it has to
    /// restart with the state and keep running while the state holds.
    #[test]
    fn elapsed_time_restarts_with_the_state() {
        let mut animator = SkeletonAnimator::default();
        let idle = BodyRequests::default();
        animator.advance(&idle, 0.5);
        assert!((animator.elapsed() - 0.5).abs() < 1e-6);

        animator.advance(&BodyRequests { running_forward: true, ..default() }, 0.25);
        assert_eq!(animator.state(), AnimationState::RunForward);
        assert_eq!(animator.elapsed(), 0.0, "the new state started part-way through");
    }

    /// A forced state ignores requests entirely — that is the whole point of
    /// it — but still ages, so a clip plays.
    #[test]
    fn a_forced_state_ignores_requests() {
        let mut animator = SkeletonAnimator::default();
        animator.force(AnimationState::Airborne, 0.5);
        animator.force(AnimationState::Airborne, 0.5);
        assert_eq!(animator.state(), AnimationState::Airborne);
        assert!((animator.elapsed() - 0.5).abs() < 1e-6);
    }

    /// The index is on disk. Every state has to survive the trip, and an index
    /// from a newer build has to land somewhere rather than failing the load.
    #[test]
    fn every_state_round_trips_through_its_index() {
        use strum::IntoEnumIterator;

        for state in AnimationState::iter() {
            assert_eq!(AnimationState::from_index(state.index()), state);
        }
        assert_eq!(AnimationState::from_index(9999), AnimationState::Idle);
    }
}
