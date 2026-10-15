//! Code-defined skeletons: the rig, what it is asked to do, and how it is drawn.
//!
//! - [`rig`] — bones and [`Pose`]s. Proportions live in the skeleton, motion in
//!   the pose as rotations, so one animation fits every build.
//! - [`state`] — [`BodyRequests`], which anything can write, and the state
//!   machine that turns them into an [`AnimationState`]. No writer chooses an
//!   animation.
//! - [`gait`] — walking and running, driven by ground covered rather than the
//!   clock, so a planted foot stays planted at any speed.
//! - [`ik`] — putting the end of a limb where it has to be, and the correction
//!   pipeline applied after a state.
//! - [`joints`] — how far each joint bends, for a solver handed positions.
//! - [`clip`] — an animation as data: keys hold a target rather than a value,
//!   so one clip fits every build.
//! - [`play`] — sampling a clip and composing it onto a pose.
//! - [`interpolation`] — the named curves a key eases along.
//! - [`seed`] — how the shipped clips were sampled from the pose functions they
//!   replace.
//! - [`draw`] — one prism per bone, as gizmos.
//! - [`mesh`] — the solid a body is drawn as.

pub mod clip;
pub mod draw;
pub mod gait;
pub mod ik;
pub mod interpolation;
pub mod joints;
pub mod mesh;
pub mod play;
pub mod rig;
pub mod seed;
pub mod state;

pub use clip::{
    Axis, ChainRef, Channel, Clip, ClipSet, Clock, Combine, Key, Space, Track, Unit,
};
pub use draw::{draw_skeleton, SkeletonPalette};
pub use interpolation::Interpolation;
pub use play::{apply, apply_stack, ClipAssetsPlugin, ClipStack, Frames, Playing};
pub use mesh::{body_meshes, bone_mesh, Profile, Ring};
pub use joints::{limit_of, Hinge, JointLimit};
pub use gait::{direction_of, DisplaySpeed, FootOffset, Gait, GaitShape};
pub use ik::{animator_pose, duck_under, finish_pose, leg_length, plant_feet, solve, Chain, Reach};
pub use rig::{bone, default_humanoid, humanoid, Bone, Pose, PosedBone, Proportions, Side, Skeleton};
pub use state::{
    AnimationClock, AnimationPhase, AnimationState, BodyRequests, ForcedAnimation, PoseInputs,
    SkeletonAnimator,
};
