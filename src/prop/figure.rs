//! The body standing behind the prop, for scale.
//!
//! A weapon is modelled in metres and nothing in an empty viewport says how
//! big a metre is. The grid helps; a person helps more, because the question
//! being asked is never "how long is this" but "does this look right in
//! somebody's hands".
//!
//! **Off by default.** It is a ruler, not part of the prop, and a modeller who
//! wants the silhouette on its own should get the silhouette on its own.
//!
//! **The figure holds the prop, rather than the prop being wherever a hand
//! happens to be.** A prop is authored around its own origin, running along
//! the axis it is drawn on — so the figure is placed twice over: its right
//! hand is put at the origin, and it is *turned* so the line between its two
//! hands runs down the prop's axis. Facing a fixed direction instead would
//! leave the support hand out beside a weapon rather than on it, which is
//! wrong in exactly the way that makes a reference figure useless. The body
//! ends up with its feet well below the grid, which is correct: the grid is a
//! ruler around the prop, not a floor the figure stands on.
//!
//! It is a rig with a [`Pose`] and deliberately **no `SkeletonAnimator`**: the
//! pose is set once and nothing else may write it, the same rule a corpse
//! follows. `BodyMeshPlugin` is ungated, so the ordinary drawing dresses it
//! from the same cached meshes every other body of that build uses — a
//! reference figure is not a second kind of body.

use bevy::prelude::*;

use crate::common::class::Class;
use crate::common::skeleton::rig::bone;
use crate::common::skeleton::{humanoid, Pose, Skeleton};
use crate::game::body_mesh::BodyTint;

/// Which class is standing there, if any.
///
/// A resource rather than a field on the document: the figure is a fact about
/// how somebody is *looking* at a prop, like which camera they are in, and
/// writing it into the file would make a reviewer's diff show that somebody
/// turned a ruler on.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScaleFigure(pub Option<Class>);

/// Marks the figure, so the one that is up can be found and replaced.
#[derive(Component)]
pub struct ScaleFigureMarker;

/// Grey-green and unmistakably not the prop.
///
/// A team colour would be a lie — the figure has no side — and the default
/// body colour would make it read as a body in a match rather than as a
/// measuring stick.
const FIGURE_TINT: Color = Color::srgb(0.34, 0.40, 0.36);

/// The axis a prop is modelled along, and so the direction the figure's grip
/// is turned to lie down. The same `-Z` every rig in the game faces, which is
/// what makes a weapon lined up against these hands lined up the way it will
/// be held.
const FORWARD: Vec3 = Vec3::NEG_Z;

/// Point a bone along a world direction, by writing the joint rotation that
/// gets it there.
///
/// The alternative is working out each joint's local frame by hand and
/// writing down a quaternion, which is both unreadable and the kind of thing
/// that is wrong by a mirror on one side of the body. This asks the rig
/// instead: pose everything decided so far, read where the bone currently
/// points, and write the rotation that closes the gap.
///
/// **Parents before children.** A bone's frame is its parent's, so aiming an
/// upper arm after its forearm would swing the forearm along with it and
/// leave it pointing somewhere nobody asked for.
fn aim(skeleton: &Skeleton, pose: &mut Pose, name: &'static str, direction: Vec3) {
    let Some(index) = skeleton.index_of(name) else {
        warn!("no bone called {name} to aim");
        return;
    };
    // Cleared first so what is read back is the bone's *rest* orientation in
    // the frame its parents have put it in, rather than wherever a previous
    // call to this function left it.
    pose.set(name, Quat::IDENTITY);
    let current = skeleton.posed_bones(pose, &Transform::IDENTITY)[index].rotation;
    let wanted = (current.inverse() * direction.normalize_or_zero()).normalize_or_zero();
    if wanted == Vec3::ZERO {
        return;
    }
    pose.set(name, Quat::from_rotation_arc(Vec3::Y, wanted));
}

/// Both hands on a weapon held out in front.
///
/// The right arm is the one that matters: the upper arm hangs down and the
/// forearm points forward, which is the right angle at the elbow the pose is
/// named for. The left arm reaches across to where the right hand is, so the
/// figure reads as *holding* something rather than pointing at it — which is
/// the only reason to have a figure at all.
pub fn holding_a_weapon(skeleton: &Skeleton) -> Pose {
    let mut pose = Pose::rest();

    // The torso turns before either arm moves, and it is what makes the pose
    // possible rather than only what makes it look right. This rig's arms are
    // short — about 0.55 m from shoulder to fingertip on a 1.85 m body — so
    // with the chest square on, the support hand cannot reach a weapon held
    // anywhere near the other hand. Turning the left shoulder forward is what
    // a person actually does with a two-handed weapon, and it buys most of the
    // reach back. About local `+Y`, which runs up the bone, so this is a twist
    // rather than a lean; negative brings the left shoulder round to the front.
    pose.set(bone::CHEST, Quat::from_rotation_y(-0.40));

    // Down, and a little forward — an upper arm hanging dead straight reads as
    // a body standing to attention with a rifle taped to its wrist.
    aim(skeleton, &mut pose, bone::UPPER_ARM_R, Vec3::new(-0.30, -0.91, -0.28));
    // Forward, and very slightly inward: a weapon is held on the centreline,
    // not out at shoulder width.
    aim(skeleton, &mut pose, bone::FOREARM_R, Vec3::new(-0.10, 0.26, -0.96));
    aim(skeleton, &mut pose, bone::HAND_R, Vec3::new(-0.08, 0.20, -0.98));

    // The support hand comes further across, because it is reaching past the
    // centreline to a foregrip rather than meeting in the middle.
    aim(skeleton, &mut pose, bone::UPPER_ARM_L, Vec3::new(0.42, -0.66, -0.62));
    aim(skeleton, &mut pose, bone::FOREARM_L, Vec3::new(0.52, 0.10, -0.85));
    aim(skeleton, &mut pose, bone::HAND_L, Vec3::new(0.42, 0.06, -0.90));

    pose
}

/// Where to stand the figure, and which way to turn it.
///
/// Two things at once, because they are one decision:
///
/// - **The right hand goes to the origin.** Its *centre* rather than the
///   wrist: a grip is held in the middle of a palm, so measuring from the
///   wrist would leave every weapon sitting an inch too far forward in the
///   hand.
/// - **The body turns so the hands lie along the prop's axis.** A person
///   holding a two-handed weapon stands at an angle to it; a figure facing a
///   fixed direction would put the support hand out beside the weapon instead
///   of on it. Taking the angle from the pose rather than writing one down
///   means retuning the pose cannot silently stop the hands lining up.
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
    // heights and tipping the whole body to make up the difference would put
    // its feet in the air.
    let along = centre(bone::HAND_L)
        .map(|support| (support - grip) * Vec3::new(1.0, 0.0, 1.0))
        .filter(|along| along.length_squared() > 1e-6)
        .map(Vec3::normalize);

    let rotation = match along {
        // Both vectors lie in the ground plane, so the arc between them is a
        // turn about `Y` — a heading, which is the only part of a body's
        // orientation that is anybody's to choose.
        Some(along) => Quat::from_rotation_arc(along, FORWARD),
        None => Quat::IDENTITY,
    };

    Transform::from_translation(-(rotation * grip)).with_rotation(rotation)
}

/// Put the chosen figure up, take the old one down, and do nothing at all when
/// the choice has not moved.
///
/// Keyed on the resource having changed rather than on comparing the figure in
/// the world with the one asked for: rebuilding a rig re-inserts a `Skeleton`,
/// which reads as a changed one and throws the body's meshes away — the same
/// trap a feature standing up a body falls into.
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

/// Take the figure down on the way out of the mode, so it cannot be left
/// standing in the middle of somebody's map.
pub fn clear_the_figure(
    mut commands: Commands,
    existing: Query<Entity, With<ScaleFigureMarker>>,
    mut last: Local<Option<ScaleFigure>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    // Forgotten rather than remembered, so re-entering the mode builds it
    // again instead of deciding nothing has changed.
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

    /// `aim` is the whole reason the pose is readable, so it is worth pinning
    /// on its own: a bone asked to point somewhere points there, on either
    /// side of a body whose rest rotations are mirrored.
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

    /// The pose the user asked for, measured rather than eyeballed: the elbow
    /// is a right angle and the forearm points the way the body faces.
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

    /// The support hand has to be **forward** of the trigger hand, not merely
    /// somewhere else: hands side by side read as praying, and hands a
    /// shoulder-width apart across the body read as carrying a tray. The
    /// torso twist is what buys the reach, so this is also what fails if
    /// somebody takes it out for looking unnecessary.
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

    /// Both halves of the placement, which is the contract a modeller relies
    /// on: the prop is gripped at the origin and lies along the axis it was
    /// drawn on. A figure turned to face a fixed direction instead would put
    /// the support hand out beside the weapon rather than on it.
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

    /// On **every** class, because a build is nine numbers and the pose is
    /// angles: a Heavy's arms are not a Scout's, and a placement worked out
    /// for one that quietly missed on another would be a prop that sits in the
    /// air for half the roster.
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
    /// The offset is what puts the body below its own hand, and it is a real
    /// distance rather than a rounding error.
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
