//! Drawing a rig as gizmos: one rectangular prism per bone. Shared by the game
//! and the editor's spawn points and animation displays.

use bevy::prelude::*;

use crate::common::skeleton::rig::{Pose, Side, Skeleton};

/// The colors a skeleton is drawn in.
#[derive(Clone, Copy, Debug)]
pub struct SkeletonPalette {
    pub left: Color,
    pub right: Color,
    pub center: Color,
    pub joint: Color,
}

impl SkeletonPalette {
    /// Left and right differ, because an A-pose is nearly symmetrical and
    /// otherwise front and back look the same.
    pub const SIDES: SkeletonPalette = SkeletonPalette {
        left: Color::srgb(0.47, 0.75, 1.0),
        right: Color::srgb(1.0, 0.59, 0.35),
        center: Color::srgb(0.9, 0.9, 0.9),
        joint: Color::srgb(1.0, 0.94, 0.47),
    };

    /// One color throughout, for a body drawn inside another feature's gizmo.
    pub fn flat(color: Color) -> SkeletonPalette {
        SkeletonPalette { left: color, right: color, center: color, joint: color }
    }

    fn for_bone(&self, name: &str) -> Color {
        match Side::of(name) {
            Side::Left => self.left,
            Side::Right => self.right,
            Side::Center => self.center,
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
        // Without joint dots a chain of boxes reads as one shape.
        gizmos.sphere(
            Isometry3d::from_translation(bone.head),
            bone.thickness.min_element() * 0.35,
            palette.joint,
        );
    }
}
