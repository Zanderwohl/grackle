//! Drawing a rig as gizmos: one rectangular prism per bone.
//!
//! Shared rather than living with the game's bodies, because the editor draws
//! skeletons too — a spawn point shows the body that will stand on it, and an
//! Animation Display shows one holding a state. All three want the same rig
//! drawn the same way, and a second copy of this would be a second answer to
//! "how tall is that, really".

use bevy::prelude::*;

use crate::common::skeleton::rig::{Pose, Side, Skeleton};

/// The colours a skeleton is drawn in.
///
/// Two of them so far: [`SkeletonPalette::SIDES`] for a body being looked at
/// on its own, where telling front from back matters, and
/// [`SkeletonPalette::flat`] for a rig drawn as part of another gizmo, where
/// it has to stay recognisable as that gizmo.
#[derive(Clone, Copy, Debug)]
pub struct SkeletonPalette {
    pub left: Color,
    pub right: Color,
    pub centre: Color,
    pub joint: Color,
}

impl SkeletonPalette {
    /// Left and right in different colours. An A-pose is nearly symmetrical,
    /// so without this there is no telling from a screenshot whether a body is
    /// facing you or away.
    pub const SIDES: SkeletonPalette = SkeletonPalette {
        left: Color::srgb(0.47, 0.75, 1.0),
        right: Color::srgb(1.0, 0.59, 0.35),
        centre: Color::srgb(0.9, 0.9, 0.9),
        joint: Color::srgb(1.0, 0.94, 0.47),
    };

    /// One colour throughout, for a body drawn inside another feature's gizmo.
    pub fn flat(colour: Color) -> SkeletonPalette {
        SkeletonPalette { left: colour, right: colour, centre: colour, joint: colour }
    }

    fn for_bone(&self, name: &str) -> Color {
        match Side::of(name) {
            Side::Left => self.left,
            Side::Right => self.right,
            Side::Centre => self.centre,
        }
    }
}

/// Draw `skeleton` in `pose`, with its feet at `root`.
pub fn draw_skeleton(
    gizmos: &mut Gizmos,
    skeleton: &Skeleton,
    pose: &Pose,
    root: &Transform,
    palette: &SkeletonPalette,
) {
    for bone in skeleton.posed_bones(pose, root) {
        gizmos.cube(bone.prism(), palette.for_bone(bone.name));
        // The head is the joint the bone turns about, which is what makes a
        // bent limb readable — without the dots a chain of boxes reads as one
        // shape.
        gizmos.sphere(
            Isometry3d::from_translation(bone.head),
            bone.thickness.min_element() * 0.35,
            palette.joint,
        );
    }
}
