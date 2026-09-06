//! The one definition of what a feature's three stored angles mean.
//!
//! Features store orientation as three Euler angles rather than a quaternion,
//! because that is what a mapper types into a box and what a diff of a
//! blueprint has to stay readable as. A quaternion round-trips badly through
//! that: the same orientation has several representations, and typing 90 into
//! one box would move the numbers in the other two.
//!
//! Everything that turns those three numbers into an orientation goes through
//! here — the entity the game moves, the gizmo the editor draws, and the ring
//! a drag rotates around. Let the convention drift between them and a prop
//! sits one way in the editor and another in the map.

use bevy::prelude::*;

/// Yaw first, then pitch, then roll.
///
/// Yaw around world Y is the axis a mapper reaches for constantly — it is the
/// only one a spawn point even has — so it goes first, where it stays a plain
/// heading no matter what the other two are doing.
pub const EULER_ORDER: EulerRot = EulerRot::YXZ;

/// The angles in *axis* order: `x` is pitch (about X), `y` is yaw (about Y),
/// `z` is roll (about Z). Indexing them by `0/1/2` the way the drag handles
/// index position is the point; the application order is [`EULER_ORDER`] and
/// is not the same thing.
pub fn quat_from_euler(angles: Vec3) -> Quat {
    Quat::from_euler(EULER_ORDER, angles.y, angles.x, angles.z)
}

/// The world-space axis that a change to Euler component `axis` actually
/// rotates the object around, given the other two angles.
///
/// This is what makes a rotation ring honest. Because the angles are applied
/// yaw-pitch-roll, yaw always turns about world Y, but pitch turns about an X
/// that yaw has already swung, and roll about a Z that both have. Draw a ring
/// on the world axis instead and dragging it would move the object somewhere
/// other than under the cursor as soon as another angle was non-zero.
///
/// Returns [`Vec3::Y`] for an axis index that is not 0, 1 or 2.
pub fn euler_axis_world(angles: Vec3, axis: u8) -> Vec3 {
    match axis {
        0 => Quat::from_rotation_y(angles.y) * Vec3::X,
        2 => quat_from_euler(Vec3::new(angles.x, angles.y, 0.0)) * Vec3::Z,
        _ => Vec3::Y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The claim [`euler_axis_world`] makes: adding `d` to one component is
    /// the same as turning by `d` about the axis it names. Break it and a
    /// rotation ring drifts out from under the cursor.
    #[test]
    fn each_ring_axis_is_the_axis_its_component_turns_about() {
        let base = Vec3::new(0.4, -1.1, 0.7);
        let d = 0.3;

        for axis in 0u8..3 {
            let mut moved = base;
            moved[axis as usize] += d;

            let expected =
                Quat::from_axis_angle(euler_axis_world(base, axis), d) * quat_from_euler(base);
            // `abs_diff_eq` rather than `angle_between`, which goes NaN on
            // two quaternions that agree to the last bit.
            assert!(
                quat_from_euler(moved).abs_diff_eq(expected, 1e-4),
                "axis {axis} does not turn about the axis the ring is drawn on"
            );
        }
    }

    /// Yaw has to stay a plain heading about world Y whatever else is set,
    /// because that is the only angle a spawn point has.
    #[test]
    fn yaw_is_always_about_world_y() {
        let angles = Vec3::new(0.9, 0.0, -0.6);
        assert_eq!(euler_axis_world(angles, 1), Vec3::Y);
        assert!(quat_from_euler(Vec3::new(0.0, 1.2, 0.0))
            .abs_diff_eq(Quat::from_rotation_y(1.2), 1e-6));
    }
}
