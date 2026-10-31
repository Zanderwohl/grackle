//! The one definition of what a feature's three stored angles mean.
//!
//! Euler angles rather than a quaternion because that is what a mapper types
//! and what a blueprint diff stays readable as. The entity, the gizmo and the
//! drag ring all convert through here, so a prop cannot sit one way in the
//! editor and another in the map.

use bevy::prelude::*;

/// Yaw first, so it stays a plain heading about world Y whatever the other two
/// are; it is the only angle a spawn point has.
pub const EULER_ORDER: EulerRot = EulerRot::YXZ;

/// `angles` is in axis order (`x` pitch, `y` yaw, `z` roll), not application
/// order ([`EULER_ORDER`]).
pub fn quat_from_euler(angles: Vec3) -> Quat {
    Quat::from_euler(EULER_ORDER, angles.y, angles.x, angles.z)
}

/// The inverse of [`quat_from_euler`], in axis order. `Quat::to_euler` returns
/// application order; transposing them makes a body lean when it meant to turn.
pub fn euler_from_quat(turn: Quat) -> Vec3 {
    let (y, x, z) = turn.to_euler(EULER_ORDER);
    Vec3::new(x, y, z)
}

/// The world axis a change to Euler component `axis` rotates about, given the
/// other two. Pitch turns about an X that yaw has swung, roll about a Z both
/// have; a ring drawn on the world axis would drift from under the cursor.
///
/// Returns [`Vec3::Y`] for an index that is not 0, 1 or 2.
pub fn euler_axis_world(angles: Vec3, axis: u8) -> Vec3 {
    match axis {
        0 => Quat::from_rotation_y(angles.y) * Vec3::X,
        2 => quat_from_euler(Vec3::new(angles.x, angles.y, 0.0)) * Vec3::Z,
        _ => Vec3::Y,
    }
}

/// An orientation split into where `axis` points and how far it is spun about
/// itself. The spin is what remains after the shortest swing onto the aim,
/// measured about `axis` in the object's frame. Exact inverse of
/// [`from_aim_and_spin`].
pub fn aim_and_spin(turn: Quat, axis: Vec3) -> (Vec3, f32) {
    let axis = axis.normalize();
    let aim = (turn * axis).normalize();
    let swing = Quat::from_rotation_arc(axis, aim);
    let twist = swing.inverse() * turn;
    // The twist leaves `axis` fixed, so its vector part is parallel to `axis`.
    let spin = 2.0 * twist.xyz().dot(axis).atan2(twist.w);
    (aim, wrap_angle(spin))
}

/// Inverse of [`aim_and_spin`].
pub fn from_aim_and_spin(axis: Vec3, aim: Vec3, spin: f32) -> Quat {
    let axis = axis.normalize();
    Quat::from_rotation_arc(axis, aim.normalize()) * Quat::from_axis_angle(axis, spin)
}

/// Into `(-π, π]`, so a spin reads as the short way round.
pub fn wrap_angle(angle: f32) -> f32 {
    let wrapped = (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    if wrapped <= -std::f32::consts::PI { wrapped + std::f32::consts::TAU } else { wrapped }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Includes turns that point the axis straight backwards, where the
    /// shortest swing is a choice.
    #[test]
    fn aim_and_spin_round_trip() {
        let turns = [
            Quat::IDENTITY,
            quat_from_euler(Vec3::new(0.4, -1.1, 0.7)),
            quat_from_euler(Vec3::new(-std::f32::consts::FRAC_PI_2, 0.0, 0.0)),
            Quat::from_rotation_x(std::f32::consts::PI),
            Quat::from_rotation_z(std::f32::consts::PI) * Quat::from_rotation_y(0.3),
        ];
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            for turn in turns {
                let (aim, spin) = aim_and_spin(turn, axis);
                let back = from_aim_and_spin(axis, aim, spin);
                assert!(
                    back.abs_diff_eq(turn, 1e-4) || back.abs_diff_eq(-turn, 1e-4),
                    "{turn:?} about {axis:?} came back as {back:?}",
                );
            }
        }
    }

    #[test]
    fn spin_leaves_the_aim_alone() {
        let aim = Vec3::new(0.3, -0.2, 1.0).normalize();
        for spin in [0.0, 0.5, -2.0, 3.0] {
            let turn = from_aim_and_spin(Vec3::Y, aim, spin);
            let (back, measured) = aim_and_spin(turn, Vec3::Y);
            assert!(back.abs_diff_eq(aim, 1e-5));
            assert!((measured - spin).abs() < 1e-4, "{spin} read back as {measured}");
        }
    }

    /// Adding `d` to one component must equal turning by `d` about the axis it
    /// names, or a rotation ring drifts from under the cursor.
    #[test]
    fn each_ring_axis_is_the_axis_its_component_turns_about() {
        let base = Vec3::new(0.4, -1.1, 0.7);
        let d = 0.3;

        for axis in 0u8..3 {
            let mut moved = base;
            moved[axis as usize] += d;

            let expected =
                Quat::from_axis_angle(euler_axis_world(base, axis), d) * quat_from_euler(base);
            // `angle_between` goes NaN on quaternions that agree exactly.
            assert!(
                quat_from_euler(moved).abs_diff_eq(expected, 1e-4),
                "axis {axis} does not turn about the axis the ring is drawn on"
            );
        }
    }

    #[test]
    fn yaw_is_always_about_world_y() {
        let angles = Vec3::new(0.9, 0.0, -0.6);
        assert_eq!(euler_axis_world(angles, 1), Vec3::Y);
        assert!(quat_from_euler(Vec3::new(0.0, 1.2, 0.0))
            .abs_diff_eq(Quat::from_rotation_y(1.2), 1e-6));
    }
}

#[cfg(test)]
mod euler_tests {
    use super::*;

    /// A transposed pair round-trips any single-axis rotation and silently
    /// swaps two axes on everything else.
    #[test]
    fn a_rotation_survives_being_written_as_three_angles() {
        for angles in [
            Vec3::ZERO,
            Vec3::new(0.4, 0.0, 0.0),
            Vec3::new(0.0, 0.7, 0.0),
            Vec3::new(0.0, 0.0, -0.3),
            Vec3::new(0.25, -0.6, 0.45),
            Vec3::new(-1.1, 0.2, 0.9),
        ] {
            let turn = quat_from_euler(angles);
            let back = quat_from_euler(euler_from_quat(turn));
            assert!(
                turn.abs_diff_eq(back, 1e-5),
                "{angles:?} did not survive: {turn:?} became {back:?}",
            );
        }
    }
}
