//! Code-defined skeletons: the rig, what it is being asked to do, and how it
//! is drawn.
//!
//! Three layers, and the boundaries between them are the point:
//!
//! - [`rig`] — bones and [`Pose`]s. Proportions live in the skeleton, motion
//!   lives in the pose as rotations, so one animation fits every silhouette.
//! - [`state`] — [`BodyRequests`], which anything can write, and the state
//!   machine that turns them into an [`AnimationState`]. A local player, an
//!   NPC and a body being driven by the network all say *what is happening*
//!   and none of them chooses an animation.
//! - [`gait`] — walking and running, driven by ground covered rather than by
//!   the clock, so a planted foot stays planted at any speed.
//! - [`ik`] — putting the end of a limb where it has to be, and the pipeline
//!   that applies such corrections to whatever a state produced.
//! - [`draw`] — one prism per bone, for anything that wants to see a rig.

pub mod draw;
pub mod gait;
pub mod ik;
pub mod rig;
pub mod state;

pub use draw::{draw_skeleton, SkeletonPalette};
pub use gait::{FootOffset, Gait, GaitShape};
pub use ik::{duck_under, finish_pose, leg_length, plant_feet, solve, Chain, Reach};
pub use rig::{bone, default_humanoid, humanoid, Bone, Pose, PosedBone, Proportions, Side, Skeleton};
pub use state::{
    AnimationClock, AnimationPhase, AnimationState, BodyRequests, ForcedAnimation, PoseInputs,
    SkeletonAnimator,
};
