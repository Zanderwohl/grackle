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

/// The three angles, in axis order, that [`quat_from_euler`] would turn back
/// into `turn`.
///
/// The exact inverse, and here rather than written out at each call site
/// because the *order* is the trap: `Quat::to_euler` hands them back in
/// application order — yaw, pitch, roll — and these are indexed by axis. One
/// transposition and a body leans when it meant to turn.
pub fn euler_from_quat(turn: Quat) -> Vec3 {
    let (y, x, z) = turn.to_euler(EULER_ORDER);
    Vec3::new(x, y, z)
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

/// An orientation split into **where an axis points** and **how far it is
/// spun about itself** — the two questions somebody lining up a barrel is
/// actually asking, where three Euler angles are an answer to neither.
///
/// `axis` is the object's own axis (a prism's length, say). The aim is where
/// `turn` sends it; the spin is what is left once the shortest swing onto that
/// aim is taken out, measured about `axis` in the object's own frame. Exact:
/// [`from_aim_and_spin`] puts back the rotation this took apart.
pub fn aim_and_spin(turn: Quat, axis: Vec3) -> (Vec3, f32) {
    let axis = axis.normalize();
    let aim = (turn * axis).normalize();
    let swing = Quat::from_rotation_arc(axis, aim);
    let twist = swing.inverse() * turn;
    // A rotation that leaves `axis` where it was is a rotation about it, so the
    // twist's vector part is parallel to `axis` and its sign says which way.
    let spin = 2.0 * twist.xyz().dot(axis).atan2(twist.w);
    (aim, wrap_angle(spin))
}

/// The orientation that points `axis` along `aim`, spun `spin` radians about
/// itself. The inverse of [`aim_and_spin`].
pub fn from_aim_and_spin(axis: Vec3, aim: Vec3, spin: f32) -> Quat {
    let axis = axis.normalize();
    Quat::from_rotation_arc(axis, aim.normalize()) * Quat::from_axis_angle(axis, spin)
}

/// Into `(-π, π]`, so a spin reads as the short way round.
fn wrap_angle(angle: f32) -> f32 {
    let wrapped = (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    if wrapped <= -std::f32::consts::PI { wrapped + std::f32::consts::TAU } else { wrapped }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Taking an orientation apart into aim and spin and putting it back is
    /// the identity — whatever the axis, including turns that point it
    /// straight backwards, where the shortest swing is a choice.
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

    /// Spinning a thing about its own length does not move where it points.
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

#[cfg(test)]
mod euler_tests {
    use super::*;

    /// The two conversions are each other's inverse.
    ///
    /// Worth pinning because the failure is quiet and asymmetric: `to_euler`
    /// returns yaw-pitch-roll and these are indexed by axis, so a transposed
    /// pair round-trips *some* rotations perfectly — anything with only one
    /// non-zero angle — and silently swaps two axes on everything else. A
    /// migration that wrote poses out through this would put a lean where a
    /// turn belongs, on a subset of bones.
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
