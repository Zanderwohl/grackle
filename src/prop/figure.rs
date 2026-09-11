//! The body standing behind the prop, for scale.
//!
//! Off by default: it is a ruler, not part of the prop. The question a
//! modeller is asking is never "how long is this" but "does this look right in
//! somebody's hands".
//!
//! **It holds the prop by the same code the game does.** It used to have an
//! authored pose of its own and the runtime read its grip orientation back out
//! of that, so the editor showed a hold the game did not perform. Both now go
//! through [`crate::prop::hold`], which is what makes authoring one mean
//! anything.
//!
//! **The prop does not move; the figure does.** A prop is authored around its
//! own origin and its feature gizmos are drawn there, so the body is placed by
//! inverting where the hold says the weapon would be. Its feet end up well
//! below the grid, which is correct: the grid is a ruler around the prop, not
//! a floor.
//!
//! A rig with a [`Pose`] and deliberately **no `SkeletonAnimator`** — the pose
//! is written here and nothing else may touch it, the rule a corpse follows.

use bevy::prelude::*;

use crate::common::class::Class;
use crate::common::skeleton::{humanoid, Pose, Skeleton};
use crate::game::body_mesh::BodyTint;
use crate::prop::document::PropEditor;
use crate::prop::hold::{grip_with, weapon_transform};

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

    commands.spawn((
        ScaleFigureMarker,
        crate::prop::view::PropScene,
        // Both written by `pose_the_figure` on the next pass, from the hold the
        // document is carrying.
        Transform::IDENTITY,
        Pose::rest(),
        Visibility::Inherited,
        BodyTint(FIGURE_TINT),
        Name::new("Scale figure"),
        humanoid(class.proportions()),
    ));
}

/// Stand the figure so it is holding the prop, by **the same code the game
/// uses**.
///
/// This is the whole point of the figure now. It used to have an authored pose
/// of its own — a handful of `aim` calls and a torso twist — and the runtime
/// read its grip orientation back out of that, so the editor was showing a
/// hold the game did not perform. Now both go through
/// [`grip_with`](crate::prop::hold::grip_with) and what you drag is what
/// ships.
///
/// **The prop does not move; the figure does.** A prop is authored around its
/// own origin and its feature gizmos are drawn there, so the body is placed by
/// inverting where the hold says the weapon would be — which is also what
/// makes dragging the figure the natural way to author a carry.
pub fn pose_the_figure(
    editor: Res<PropEditor>,
    mut figures: Query<(&Skeleton, &mut Pose, &mut Transform), With<ScaleFigureMarker>>,
) {
    let hold = editor.doc().hold;
    for (skeleton, mut pose, mut transform) in &mut figures {
        let mut standing = Pose::rest();

        // Where the weapon would be if the body stood at the origin; the
        // figure goes wherever puts that at the prop's own origin instead.
        let Some(local) =
            weapon_transform(skeleton, &standing, &Transform::IDENTITY, &hold, Quat::IDENTITY)
        else {
            continue;
        };
        let rotation = local.rotation.inverse();
        let root = Transform::from_translation(rotation * -local.translation)
            .with_rotation(rotation);

        // The weapon is at the prop's origin, which is where its geometry is.
        grip_with(skeleton, &mut standing, &root, &hold, &Transform::IDENTITY);

        *pose = standing;
        *transform = root;
    }
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
    use crate::common::skeleton::rig::bone;
    use crate::prop::hold::HoldSpec;

    /// The figure stands holding the prop at the prop's own origin, on every
    /// build — which is the whole contract, since a prop's geometry is
    /// authored there and the figure is what moves.
    ///
    /// Measured through the same two calls `pose_the_figure` makes, so a
    /// change to either shows up here rather than as a body standing beside
    /// its weapon.
    #[test]
    fn every_class_stands_holding_the_prop_where_it_is() {
        let hold = HoldSpec::default();
        for class in [
            Class::Scout, Class::Soldier, Class::Pyro, Class::Demoman,
            Class::Heavy, Class::Engineer, Class::Medic, Class::Sniper,
            Class::Spy, Class::Civilian, Class::Mercenary,
        ] {
            let skeleton = humanoid(class.proportions());
            let mut pose = Pose::rest();

            let local = weapon_transform(
                &skeleton, &pose, &Transform::IDENTITY, &hold, Quat::IDENTITY,
            )
            .expect("a humanoid has a chest");
            let rotation = local.rotation.inverse();
            let root = Transform::from_translation(rotation * -local.translation)
                .with_rotation(rotation);

            grip_with(&skeleton, &mut pose, &root, &hold, &Transform::IDENTITY);

            let index = skeleton.index_of(bone::HAND_R).expect("bone is in the rig");
            let hand = skeleton.posed_bones(&pose, &root)[index].head;
            assert!(
                (hand - hold.grip()).length() < 0.02,
                "{class:?} stands with its hand at {hand} while the grip is at {}",
                hold.grip(),
            );

            // And below the weapon, because a prop sits at chest height on a
            // body whose feet are on the floor.
            assert!(
                root.translation.y < -0.9,
                "{class:?} stands {} below its own hand",
                -root.translation.y,
            );
        }
    }
}
