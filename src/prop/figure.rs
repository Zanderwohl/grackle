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
use crate::common::skeleton::ik::Reach;
use crate::common::skeleton::{humanoid, Pose, Skeleton};
use crate::game::body_mesh::BodyTint;
use crate::prop::camera::ViewmodelPreview;
use crate::prop::document::PropEditor;
use crate::common::skeleton::play::{apply_stack, ClipStack, Frames};
use crate::prop::hold::{carry_frame, grip_with, weapon_transform};

/// Which class is standing there, if any.
///
/// A resource rather than a field on the document: it is a fact about how
/// somebody is *looking* at a prop, not about the prop.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScaleFigure(pub Option<Class>);

#[derive(Component)]
pub struct ScaleFigureMarker;

/// Whether each hand made it onto the weapon, trigger hand first.
///
/// Written where the arms are solved and read where the handles are drawn, so
/// a grip nobody can reach says so on screen. Without it an unreachable grip
/// is a figure standing slightly beside its weapon for a reason you have to
/// guess at — and the class dropdown is right there, so it is a reason you
/// would have to guess at eleven times.
#[derive(Component, Clone, Copy, Debug)]
pub struct FigureReach(pub [Reach; 2]);

/// What the reference figure is performing, if anything.
///
/// A resource rather than a component on the figure because the figure is
/// respawned whenever the class on show changes, and a performance that went
/// away with it would stop every time somebody compared two builds. Absent
/// entirely — the animation editor is not open — means the figure stands in
/// its hold, which is what the modelling surface wants.
#[derive(Resource, Default)]
pub struct FigurePerformance(pub ClipStack);

/// Which body state the figure's performance is the clip of, and the inputs it
/// is at — so the corrections the game makes after a body clip are made here
/// too. `None` for a weapon clip, or for nothing playing.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct FigureCorrections(
    pub Option<(crate::common::skeleton::state::AnimationState, crate::common::skeleton::state::PoseInputs)>,
);

/// Where the prop is being held, once a performance has moved it.
///
/// **In the animation editor the weapon moves and the body stands still**,
/// which is the opposite of the modelling surface and deliberately so. There,
/// a prop is pinned to its own origin because that is where its feature gizmos
/// are drawn and a carry is authored by dragging the body around it. Here
/// nothing is being dragged, and a clip that lowers a weapon would tilt the
/// *body* through the same inversion — so you would watch a figure lie down
/// rather than watch a launcher come up, which is the one thing the mode
/// exists to show.
///
/// Identity when nothing is performing, so the modelling surface is unchanged.
#[derive(Resource, Debug, Clone, Copy)]
pub struct FigureWeapon(pub Transform);

impl Default for FigureWeapon {
    fn default() -> Self {
        FigureWeapon(Transform::IDENTITY)
    }
}

/// How the animation editor stages the figure: whether it holds the prop, and
/// what stays still while a clip plays.
///
/// **Animation-editor only.** The modelling surface always stands the figure
/// holding the prop at the prop's own origin — its feature gizmos are drawn
/// there and a carry is authored by dragging the body around it — so outside
/// that mode this is read as its default whatever it holds. Kept across
/// visits rather than reset, because it is a way of looking that somebody
/// chose.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FigureStage {
    /// Off to see a body's own poses with nothing in its hands — what the
    /// game draws for a body holding no weapon.
    pub holding: bool,
    pub anchor: Anchor,
}

impl Default for FigureStage {
    fn default() -> Self {
        FigureStage { holding: true, anchor: Anchor::Weapon }
    }
}

/// What stays put while the figure performs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Anchor {
    /// The weapon's grip sits at the origin and the body hangs off it, as on
    /// the modelling surface. Right for a weapon clip, read against the hold;
    /// wrong for a body clip, whose bob then shows up as feet bobbing.
    #[default]
    Weapon,
    /// The feet stand at the origin, on the grid, and the weapon goes where
    /// the game would put it — off the *animated* chest. What a body clip
    /// looks like in a match.
    Floor,
}

impl Anchor {
    pub const ALL: [Anchor; 2] = [Anchor::Weapon, Anchor::Floor];

    pub fn name_key(self) -> &'static str {
        match self {
            Anchor::Weapon => "anim.anchor.weapon",
            Anchor::Floor => "anim.anchor.floor",
        }
    }
}

/// The staging in force: the chosen one in the animation editor, the default
/// everywhere else.
pub fn stage_in_force(
    stage: Option<&FigureStage>,
    mode: Option<&State<crate::common::app_mode::AppMode>>,
) -> FigureStage {
    let animating = mode.is_some_and(|mode| *mode.get() == crate::common::app_mode::AppMode::Animation);
    match (animating, stage) {
        (true, Some(stage)) => *stage,
        _ => FigureStage::default(),
    }
}

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
        FigureReach([Reach::Impossible; 2]),
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
    choice: Res<ScaleFigure>,
    preview: Res<ViewmodelPreview>,
    performance: Option<Res<FigurePerformance>>,
    clips: Option<Res<Assets<crate::common::skeleton::clip::Clip>>>,
    mut held: Option<ResMut<FigureWeapon>>,
    stage: Option<Res<FigureStage>>,
    corrections: Option<Res<FigureCorrections>>,
    mode: Option<Res<State<crate::common::app_mode::AppMode>>>,
    mut figures: Query<
        (&Skeleton, &mut Pose, &mut Transform, &mut FigureReach, &mut Visibility),
        With<ScaleFigureMarker>,
    >,
) {
    // As the class on show performs it, so an override is authored against the
    // body that will actually hold the thing.
    let hold = editor.doc().hold.for_class(choice.0);
    let stage = stage_in_force(stage.as_deref(), mode.as_deref());
    for (skeleton, mut pose, mut transform, mut reach, mut visibility) in &mut figures {
        // Out of the way while the perspective view is standing in for an eye,
        // because that eye is inside this body's head.
        let wanted = match preview.0 {
            true => Visibility::Hidden,
            false => Visibility::Inherited,
        };
        if *visibility != wanted {
            *visibility = wanted;
        }

        let mut standing = Pose::rest();

        // Where the weapon would be if the body stood at the origin; the
        // figure goes wherever puts that at the prop's own origin instead.
        let Some(local) =
            weapon_transform(skeleton, &standing, &Transform::IDENTITY, &hold, Quat::IDENTITY)
        else {
            continue;
        };
        // **The body is placed from the hold alone**, before any clip runs.
        // That is what keeps it standing still while a performance moves the
        // weapon: place it from the *animated* weapon instead and the same
        // inversion tips the body over, so a deploy reads as a figure lying
        // down rather than as a launcher coming up.
        let rotation = local.rotation.inverse();
        let root = match stage.anchor {
            Anchor::Weapon => Transform::from_translation(rotation * -local.translation)
                .with_rotation(rotation),
            // Feet at the origin, facing the way a body faces.
            Anchor::Floor => Transform::IDENTITY,
        };

        // With no performance the weapon sits at the prop's own origin, which
        // is where its geometry is — the modelling surface, unchanged. On the
        // floor it is wherever the hold puts it on a body standing there.
        let mut placed = match stage.anchor {
            Anchor::Weapon => Transform::IDENTITY,
            Anchor::Floor => local,
        };
        let at_rest = placed;
        if let (Some(performance), Some(clips)) = (performance.as_deref(), clips.as_deref()) {
            let ground = move |_: Vec3| root.translation.y;
            let frames = Frames {
                body: root,
                carry: carry_frame(skeleton, &standing, &root, Quat::IDENTITY)
                    .unwrap_or(root),
                // Nothing in the hands is nothing to measure from, and a
                // weapon track with nothing to move moves nothing.
                weapon: stage.holding.then_some(placed),
                eye: None,
                ground: &ground,
                // Where the feet are in a gait at the preview's inputs, so a
                // foot-timed track is seen at the speed it is being previewed
                // at — the same answer `ik::animation_pose` gives in a match.
                footfalls: corrections.as_deref().and_then(|c| c.0).and_then(|(state, inputs)| {
                    state
                        .stance(&inputs)
                        .map(|stance| crate::common::skeleton::gait::footfalls(inputs.stride, stance))
                }),
            };
            let weapon = stage.holding.then_some(&mut placed);
            apply_stack(&performance.0, clips, skeleton, &mut standing, &frames, weapon);
        }
        // Then what the game does to a body clip before anything else sees
        // it: duck, plant the feet. In that order and at this point, because
        // the weapon is placed from the pose these leave.
        if let Some((state, inputs)) = corrections.as_deref().and_then(|c| c.0) {
            crate::common::skeleton::ik::correct_for_state(
                skeleton,
                &mut standing,
                state,
                &inputs,
                &root,
            );
        }

        // **On the floor the weapon follows the animated chest**, the order
        // the game runs in: the body animates, the weapon is placed from the
        // pose it left, then a weapon clip moves it. Whatever the clip did to
        // the weapon is carried across as a change from where it started, so
        // a body clip's bob lifts the weapon and a weapon clip's recoil still
        // lands.
        if stage.anchor == Anchor::Floor && stage.holding {
            if let Some(animated) =
                weapon_transform(skeleton, &standing, &root, &hold, Quat::IDENTITY)
            {
                let moved = placed.compute_affine() * at_rest.compute_affine().inverse();
                placed = Transform::from_matrix(Mat4::from(moved * animated.compute_affine()));
            }
        }

        if let Some(held) = held.as_deref_mut() {
            if held.0 != placed {
                held.0 = placed;
            }
        }

        // Onto wherever the clip left it, which is the same order the game
        // runs in: place the weapon, then put the hands on it. Empty hands are
        // left where the pose put them, which is what a body holding nothing
        // looks like in a match.
        if stage.holding {
            reach.0 = grip_with(skeleton, &mut standing, &root, &hold, &placed);
        }

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

    fn staged(stage: FigureStage, mode: crate::common::app_mode::AppMode) -> (World, Entity) {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(PropEditor::default());
        world.insert_resource(ScaleFigure(Some(Class::Mercenary)));
        world.insert_resource(ViewmodelPreview::default());
        world.insert_resource(FigureWeapon::default());
        world.insert_resource(stage);
        world.insert_resource(State::new(mode));
        world.init_resource::<Assets<crate::common::skeleton::clip::Clip>>();
        let figure = world
            .spawn((
                ScaleFigureMarker,
                Transform::IDENTITY,
                Pose::rest(),
                Visibility::Inherited,
                FigureReach([Reach::Impossible; 2]),
                humanoid(Class::Mercenary.proportions()),
            ))
            .id();
        world.run_system_once(pose_the_figure).unwrap();
        (world, figure)
    }

    /// On the floor the feet are at the origin and the weapon is wherever the
    /// hold puts it — at chest height, not at the origin.
    #[test]
    fn a_floor_anchored_figure_stands_on_the_origin() {
        use crate::common::app_mode::AppMode;
        let (world, figure) = staged(
            FigureStage { holding: true, anchor: Anchor::Floor },
            AppMode::Animation,
        );
        assert_eq!(*world.get::<Transform>(figure).unwrap(), Transform::IDENTITY);
        let weapon = world.resource::<FigureWeapon>().0;
        assert!(weapon.translation.y > 0.9, "the weapon is at {}", weapon.translation);
    }

    /// Holding nothing leaves the hands to the pose — they are not solved onto
    /// a grip that is not there.
    #[test]
    fn empty_hands_are_not_put_on_a_grip() {
        use crate::common::app_mode::AppMode;
        let (world, figure) = staged(
            FigureStage { holding: false, anchor: Anchor::Weapon },
            AppMode::Animation,
        );
        assert!(hand_to_grip(&world, figure) > 0.05, "an empty hand went to the grip");
    }

    /// How far the trigger hand is from the grip, with the weapon at the origin.
    fn hand_to_grip(world: &World, figure: Entity) -> f32 {
        let skeleton = world.get::<Skeleton>(figure).unwrap();
        let pose = world.get::<Pose>(figure).unwrap();
        let root = world.get::<Transform>(figure).unwrap();
        let index = skeleton.index_of(bone::HAND_R).expect("bone is in the rig");
        let hand = skeleton.posed_bones(pose, root)[index].head;
        (hand - HoldSpec::default().grip()).length()
    }

    /// The modelling surface ignores the animation editor's staging.
    #[test]
    fn the_prop_editor_always_stands_holding_at_the_origin() {
        use crate::common::app_mode::AppMode;
        let (world, figure) = staged(
            FigureStage { holding: false, anchor: Anchor::Floor },
            AppMode::Prop,
        );
        assert_ne!(*world.get::<Transform>(figure).unwrap(), Transform::IDENTITY);
        assert!(hand_to_grip(&world, figure) < 0.02, "the hands were not solved");
    }

    /// **A body clip is previewed with the game's corrections.** The idle's
    /// settle lowers the hips; in a match the knees take it and the feet stay
    /// on the floor. Previewed without them the whole body sank and rose, feet
    /// and all — which read as the feet floating.
    #[test]
    fn a_previewed_idle_keeps_its_feet_on_the_floor() {
        use crate::common::app_mode::AppMode;
        use crate::common::skeleton::play::Playing;
        use crate::common::skeleton::state::{AnimationState, PoseInputs};
        use bevy::ecs::system::RunSystemOnce;

        let text = std::fs::read_to_string("assets/default/animations/idle.gpa")
            .expect("the default pack ships an idle");
        let clip: crate::common::skeleton::clip::Clip = toml::from_str(&text).unwrap();
        // The deepest point of the settle.
        let time = 2.0;

        let foot_height = |corrected: bool| {
            let (mut world, figure) = staged(
                FigureStage { holding: false, anchor: Anchor::Floor },
                AppMode::Animation,
            );
            let clip = world
                .resource_mut::<Assets<crate::common::skeleton::clip::Clip>>()
                .add(clip.clone());
            world.insert_resource(FigurePerformance(ClipStack(vec![Playing {
                clip,
                time,
                weight: 1.0,
            }])));
            world.insert_resource(FigureCorrections(corrected.then_some((
                AnimationState::Idle,
                PoseInputs { seconds: time, ..Default::default() },
            ))));
            world.run_system_once(pose_the_figure).unwrap();
            let skeleton = world.get::<Skeleton>(figure).unwrap();
            let pose = world.get::<Pose>(figure).unwrap();
            let root = world.get::<Transform>(figure).unwrap();
            let index = skeleton.index_of(bone::FOOT_L).expect("bone is in the rig");
            skeleton.posed_bones(pose, root)[index].head.y
        };

        let skeleton = humanoid(Class::Mercenary.proportions());
        let index = skeleton.index_of(bone::FOOT_L).unwrap();
        let resting = skeleton.posed_bones(&Pose::rest(), &Transform::IDENTITY)[index].head.y;

        assert!(
            (foot_height(false) - resting).abs() > 0.01,
            "the uncorrected clip no longer moves the feet — this test has lost its point",
        );
        assert!(
            (foot_height(true) - resting).abs() < 0.005,
            "a planted foot left the floor: {} against {resting}",
            foot_height(true),
        );
    }

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

