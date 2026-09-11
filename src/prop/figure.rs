//! The body standing behind the prop, for scale.
//!
//! Off by default: it is a ruler, not part of the prop. The question a
//! modeller is asking is never "how long is this" but "does this look right in
//! somebody's hands".
//!
//! **The figure holds the prop, rather than the prop sitting wherever a hand
//! happens to be.** It is placed twice over — its right hand at the origin,
//! and the whole body *turned* so the line between its hands runs down the
//! prop's axis. Facing a fixed direction instead leaves the support hand out
//! beside the weapon rather than on it. Its feet end up well below the grid,
//! which is correct: the grid is a ruler around the prop, not a floor.
//!
//! A rig with a [`Pose`] and deliberately **no `SkeletonAnimator`** — the pose
//! is set once and nothing else may write it, the rule a corpse follows. The
//! ungated `BodyMeshPlugin` then dresses it from the same cached meshes as
//! every other body of that build.

use bevy::prelude::*;

use crate::common::class::Class;
use crate::common::skeleton::rig::bone;
use crate::common::skeleton::{humanoid, Pose, Skeleton};
use crate::game::body_mesh::BodyTint;

/// Which class is standing there, if any.
///
/// A resource rather than a field on the document: it is a fact about how
/// somebody is *looking* at a prop, not about the prop.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScaleFigure(pub Option<Class>);

#[derive(Component)]
pub struct ScaleFigureMarker;

/// Grey-green and unmistakably not the prop. A team colour would be a lie —
/// the figure has no side.
const FIGURE_TINT: Color = Color::srgb(0.34, 0.40, 0.36);

/// The axis a prop is modelled along, and the direction the grip is turned to
/// lie down. The same `-Z` every rig in the game faces.
const FORWARD: Vec3 = Vec3::NEG_Z;

/// Point a bone along a world direction.
///
/// Asks the rig rather than working out each joint's local frame by hand: pose
/// what is decided so far, read where the bone points, write the rotation that
/// closes the gap. A hand-written quaternion is unreadable and tends to be
/// wrong by a mirror on one side of the body.
///
/// **Parents before children.** A bone's frame is its parent's, so aiming an
/// upper arm after its forearm swings the forearm along with it.
fn aim(skeleton: &Skeleton, pose: &mut Pose, name: &'static str, direction: Vec3) {
    let Some(index) = skeleton.index_of(name) else {
        warn!("no bone called {name} to aim");
        return;
    };
    // Cleared first, so what is read back is the bone's rest orientation in
    // its parents' frame rather than wherever a previous call left it.
    pose.set(name, Quat::IDENTITY);
    let current = skeleton.posed_bones(pose, &Transform::IDENTITY)[index].rotation;
    let wanted = (current.inverse() * direction.normalize_or_zero()).normalize_or_zero();
    if wanted == Vec3::ZERO {
        return;
    }
    pose.set(name, Quat::from_rotation_arc(Vec3::Y, wanted));
}

/// Both hands on a weapon held out in front: the right upper arm hangs down
/// and its forearm points forward, and the left arm reaches across so the
/// figure reads as *holding* something rather than pointing at it.
pub fn holding_a_weapon(skeleton: &Skeleton) -> Pose {
    let mut pose = Pose::rest();

    // The torso turns first, and it is what makes the pose *possible* rather
    // than only what makes it look right: this rig's arms are short (about
    // 0.55 m shoulder to fingertip on a 1.85 m body), so with the chest square
    // on the support hand cannot reach. About local `+Y`, which runs up the
    // bone, so this is a twist; negative brings the left shoulder forward.
    pose.set(bone::CHEST, Quat::from_rotation_y(-0.40));

    // Down and a little forward: hanging dead straight reads as a body
    // standing to attention with a rifle taped to its wrist.
    aim(skeleton, &mut pose, bone::UPPER_ARM_R, Vec3::new(-0.30, -0.91, -0.28));
    // Forward and slightly inward: a weapon is held on the centreline.
    aim(skeleton, &mut pose, bone::FOREARM_R, Vec3::new(-0.10, 0.26, -0.96));
    aim(skeleton, &mut pose, bone::HAND_R, Vec3::new(-0.08, 0.20, -0.98));

    // Further across: reaching past the centreline to a foregrip.
    aim(skeleton, &mut pose, bone::UPPER_ARM_L, Vec3::new(0.42, -0.66, -0.62));
    aim(skeleton, &mut pose, bone::FOREARM_L, Vec3::new(0.52, 0.10, -0.85));
    aim(skeleton, &mut pose, bone::HAND_L, Vec3::new(0.42, 0.06, -0.90));

    pose
}

/// Where to stand the figure, and which way to turn it — one decision, so one
/// function:
///
/// - **The right hand goes to the origin**, its *centre* rather than the
///   wrist, since a grip is held in the middle of a palm.
/// - **The body turns so the hands lie along the prop's axis.** Taken from the
///   pose rather than written down, so retuning the pose cannot silently stop
///   the hands lining up.
pub fn place(skeleton: &Skeleton, pose: &Pose) -> Transform {
    let bones = skeleton.posed_bones(pose, &Transform::IDENTITY);
    let centre = |name: &str| {
        skeleton.index_of(name).map(|index| {
            let bone = &bones[index];
            (bone.head + bone.tail) / 2.0
        })
    };

    let Some(grip) = centre(bone::HAND_R) else {
        return Transform::IDENTITY;
    };

    // Flattened onto the ground plane: the hands are at slightly different
    // heights, and tipping the body to match would put its feet in the air.
    let along = centre(bone::HAND_L)
        .map(|support| (support - grip) * Vec3::new(1.0, 0.0, 1.0))
        .filter(|along| along.length_squared() > 1e-6)
        .map(Vec3::normalize);

    let rotation = match along {
        // Both lie in the ground plane, so the arc between them is a turn about
        // `Y` — a heading, which is the only part of a body's orientation
        // anybody chooses.
        Some(along) => Quat::from_rotation_arc(along, FORWARD),
        None => Quat::IDENTITY,
    };

    Transform::from_translation(-(rotation * grip)).with_rotation(rotation)
}

/// Put the chosen figure up and take the old one down.
///
/// Keyed on the choice having changed: re-inserting a `Skeleton` reads as a
/// changed one and throws the body's meshes away — the same trap a feature
/// standing up a body falls into.
pub fn refresh_the_figure(
    mut commands: Commands,
    choice: Res<ScaleFigure>,
    existing: Query<Entity, With<ScaleFigureMarker>>,
    mut last: Local<Option<ScaleFigure>>,
) {
    if *last == Some(*choice) && !existing.is_empty() {
        return;
    }
    *last = Some(*choice);

    for entity in &existing {
        commands.entity(entity).despawn();
    }

    let Some(class) = choice.0 else { return };

    let skeleton = humanoid(class.proportions());
    let pose = holding_a_weapon(&skeleton);
    let placement = place(&skeleton, &pose);

    commands.spawn((
        ScaleFigureMarker,
        crate::prop::view::PropScene,
        placement,
        Visibility::Inherited,
        BodyTint(FIGURE_TINT),
        Name::new("Scale figure"),
        skeleton,
        pose,
    ));
}

/// Take the figure down on the way out, so it is not left standing in a map.
pub fn clear_the_figure(
    mut commands: Commands,
    existing: Query<Entity, With<ScaleFigureMarker>>,
    mut last: Local<Option<ScaleFigure>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    // Forgotten, so re-entering rebuilds rather than deciding nothing changed.
    *last = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig() -> Skeleton {
        humanoid(Class::Soldier.proportions())
    }

    fn direction_of(skeleton: &Skeleton, pose: &Pose, name: &str) -> Vec3 {
        let index = skeleton.index_of(name).expect("bone is in the rig");
        let bone = &skeleton.posed_bones(pose, &Transform::IDENTITY)[index];
        (bone.tail - bone.head).normalize()
    }

    /// A bone asked to point somewhere points there, on either side of a body
    /// whose rest rotations are mirrored.
    #[test]
    fn aiming_a_bone_points_it_where_it_was_asked_to() {
        let skeleton = rig();
        for name in [bone::FOREARM_R, bone::FOREARM_L, bone::THIGH_R] {
            for wanted in [Vec3::NEG_Z, Vec3::X, Vec3::new(0.3, -0.8, -0.5).normalize()] {
                let mut pose = Pose::rest();
                aim(&skeleton, &mut pose, name, wanted);
                let got = direction_of(&skeleton, &pose, name);
                assert!(
                    got.dot(wanted) > 0.999,
                    "{name} aimed at {wanted} ended up pointing {got}",
                );
            }
        }
    }

    /// Measured rather than eyeballed: the elbow is a right angle and the
    /// forearm points the way the body faces.
    #[test]
    fn the_right_elbow_is_bent_a_right_angle_forwards() {
        let skeleton = rig();
        let pose = holding_a_weapon(&skeleton);

        let upper = direction_of(&skeleton, &pose, bone::UPPER_ARM_R);
        let fore = direction_of(&skeleton, &pose, bone::FOREARM_R);

        let elbow = upper.angle_between(fore).to_degrees();
        assert!(
            (elbow - 90.0).abs() < 12.0,
            "the right elbow is bent {elbow:.1}° rather than about 90°",
        );
        assert!(fore.dot(FORWARD) > 0.9, "the right forearm points {fore}, not forwards");
        assert!(upper.y < -0.8, "the right upper arm is not hanging down: {upper}");
    }

    /// The support hand has to be **forward** of the trigger hand: side by
    /// side reads as praying, shoulder-width apart as carrying a tray. The
    /// torso twist buys that reach, so this is what fails if it is removed.
    #[test]
    fn the_support_hand_reaches_forward_along_the_weapon() {
        let skeleton = rig();
        let pose = holding_a_weapon(&skeleton);
        let bones = skeleton.posed_bones(&pose, &Transform::IDENTITY);
        let hand = |name: &str| {
            let index = skeleton.index_of(name).expect("bone is in the rig");
            (bones[index].head + bones[index].tail) / 2.0
        };

        let (left, right) = (hand(bone::HAND_L), hand(bone::HAND_R));
        let along = left - right;
        assert!(
            along.length() > 0.2,
            "the hands are only {:.3} m apart; that is not a two-handed grip",
            along.length(),
        );
        assert!(
            (left.z - right.z) < -0.15,
            "the support hand is {:.3} m forward of the trigger hand, which is not enough",
            right.z - left.z,
        );
        assert!(left.z < -0.1 && right.z < -0.1, "the hands are not out in front");
    }

    /// The contract a modeller relies on: the prop is gripped at the origin
    /// and lies along the axis it was drawn on.
    #[test]
    fn the_figure_grips_the_origin_and_lines_up_with_the_prop() {
        let skeleton = rig();
        let pose = holding_a_weapon(&skeleton);
        let placement = place(&skeleton, &pose);
        let bones = skeleton.posed_bones(&pose, &placement);
        let hand = |name: &str| {
            let index = skeleton.index_of(name).expect("bone is in the rig");
            (bones[index].head + bones[index].tail) / 2.0
        };

        assert!(hand(bone::HAND_R).length() < 1e-4, "the grip is not at the origin");

        let along = (hand(bone::HAND_L) - hand(bone::HAND_R)) * Vec3::new(1.0, 0.0, 1.0);
        assert!(
            along.normalize().dot(FORWARD) > 0.999,
            "the hands run along {} rather than down the prop's axis",
            along.normalize(),
        );
    }

    /// On **every** class: a build is nine numbers and the pose is angles, so
    /// a placement tuned on one could quietly miss on another.
    #[test]
    fn every_class_grips_the_origin() {
        for class in [
            Class::Scout, Class::Soldier, Class::Pyro, Class::Demoman,
            Class::Heavy, Class::Engineer, Class::Medic, Class::Sniper,
            Class::Spy, Class::Civilian, Class::Mercenary,
        ] {
            let skeleton = humanoid(class.proportions());
            let pose = holding_a_weapon(&skeleton);
            let placement = place(&skeleton, &pose);

            let index = skeleton.index_of(bone::HAND_R).expect("bone is in the rig");
            let hand = &skeleton.posed_bones(&pose, &placement)[index];
            let centre = (hand.head + hand.tail) / 2.0;

            assert!(
                centre.length() < 1e-4,
                "{class:?} holds its weapon at {centre} rather than at the origin",
            );
        }
    }

    /// A figure standing at the origin would have its feet where the prop is.
    #[test]
    fn the_figure_stands_below_its_own_hand() {
        let skeleton = rig();
        let pose = holding_a_weapon(&skeleton);
        assert!(
            place(&skeleton, &pose).translation.y < -0.9,
            "the figure's feet are only {:.2} m below its hand",
            -place(&skeleton, &pose).translation.y,
        );
    }
}
