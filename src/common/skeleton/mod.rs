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
//! - [`draw`] — one prism per bone, for anything that wants to see a rig.

pub mod draw;
pub mod rig;
pub mod state;

pub use draw::{draw_skeleton, SkeletonPalette};
pub use rig::{bone, default_humanoid, humanoid, Bone, Pose, PosedBone, Proportions, Side, Skeleton};
pub use state::{AnimationState, BodyRequests, ForcedAnimation, SkeletonAnimator};
