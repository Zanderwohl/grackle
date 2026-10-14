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
//! - [`joints`] — how far each joint bends, for the one caller that does not
//!   already know: a solver handed positions rather than an animation.
//! - [`clip`] — an animation **as data**: keys that hold a *target* rather
//!   than a value, so one clip fits every build and a foot can be keyed to
//!   the floor rather than to a height.
//! - [`play`] — sampling a clip and composing it onto a pose. The pure half
//!   and the half that needs to know where the floor is, kept apart.
//! - [`interpolation`] — the named curves a key eases along.
//! - [`seed`] — how the shipped body clips were sampled out of the pose
//!   functions they replace. The generator, not the files: once seeded, a clip
//!   is edited and the functions are only where its reasoning is written down.
//! - [`draw`] — one prism per bone, for anything that wants to see a rig.
//! - [`mesh`] — the solid a body is actually drawn as, built from the same
//!   proportions the bones are.

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
pub use play::{apply, apply_stack, ClipStack, Frames, Playing};
pub use mesh::{body_meshes, bone_mesh, Profile, Ring};
pub use joints::{limit_of, Hinge, JointLimit};
pub use gait::{direction_of, DisplaySpeed, FootOffset, Gait, GaitShape};
pub use ik::{animator_pose, duck_under, finish_pose, leg_length, plant_feet, solve, Chain, Reach};
pub use rig::{bone, default_humanoid, humanoid, Bone, Pose, PosedBone, Proportions, Side, Skeleton};
pub use state::{
    AnimationClock, AnimationPhase, AnimationState, BodyRequests, ForcedAnimation, PoseInputs,
    SkeletonAnimator,
};
