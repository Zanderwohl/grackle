//! The body standing behind the prop, for scale.
//!
//! It holds the prop through [`crate::prop::hold`], the same code the game
//! uses.
//!
//! The prop stays at its own origin, where its feature gizmos are drawn; the
//! body is placed by inverting where the hold says the weapon would be, so its
//! feet end up well below the grid.
//!
//! A rig with a [`Pose`] and no `SkeletonAnimator`: the pose is written here
//! and nothing else may touch it.

use bevy::prelude::*;

use crate::common::class::Class;
use crate::common::skeleton::ik::Reach;
use crate::common::skeleton::{humanoid, Pose, Skeleton};
use crate::game::body_mesh::BodyTint;
use crate::prop::camera::ViewmodelPreview;
use crate::prop::document::PropEditor;
use crate::common::skeleton::play::{apply_stack, ClipStack, Frames};
use crate::prop::archetype::{hat_transform, Archetype};
use crate::prop::hold::{carry_frame, grip_with, weapon_transform};

/// Which class is standing there, if any. View state, not part of the document.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScaleFigure(pub Option<Class>);

#[derive(Component)]
pub struct ScaleFigureMarker;

/// Whether each hand made it onto the weapon, trigger hand first. Read where
/// the handles are drawn, so an unreachable grip shows on screen.
#[derive(Component, Clone, Copy, Debug)]
pub struct FigureReach(pub [Reach; 2]);

/// What the reference figure is performing. A resource because the figure is
/// respawned when the class changes. Absent outside the animation editor.
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
/// In the animation editor the weapon moves and the body stands still: through
/// the modelling surface's inversion, a clip that lowers the weapon would tip
/// the body over instead.
#[derive(Resource, Debug, Clone, Copy)]
pub struct FigureWeapon(pub Transform);

impl Default for FigureWeapon {
    fn default() -> Self {
        FigureWeapon(Transform::IDENTITY)
    }
}

/// How the animation editor stages the figure. Outside that mode it reads as
/// its default (see [`stage_in_force`]).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FigureStage {
    pub holding: bool,
    pub anchor: Anchor,
}

impl Default for FigureStage {
    fn default() -> Self {
        FigureStage { holding: true, anchor: Anchor::Weapon }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Anchor {
    /// The grip sits at the origin, as on the modelling surface. A body clip's
    /// bob then shows up as the feet bobbing.
    #[default]
    Weapon,
    /// The feet stand at the origin and the weapon is placed off the animated
    /// chest, as in a match.
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

/// Metres to the side of a placed prop the figure stands, clear of a crate.
const PLACED_PROP_CLEARANCE: f32 = 1.0;

/// Not a team colour: the figure has no side.
const FIGURE_TINT: Color = Color::srgb(0.34, 0.40, 0.36);

/// Keyed on the choice or the builds changing: re-inserting a `Skeleton`
/// reads as changed and rebuilds the body's meshes.
pub fn refresh_the_figure(
    mut commands: Commands,
    choice: Res<ScaleFigure>,
    builds: Res<crate::common::class::ClassBuilds>,
    existing: Query<Entity, With<ScaleFigureMarker>>,
    mut last: Local<Option<ScaleFigure>>,
) {
    if *last == Some(*choice) && !existing.is_empty() && !builds.is_changed() {
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
        // Both written by `pose_the_figure`.
        Transform::IDENTITY,
        Pose::rest(),
        Visibility::Inherited,
        BodyTint(FIGURE_TINT),
        Name::new("Scale figure"),
        FigureReach([Reach::Impossible; 2]),
        humanoid(builds.of(class)),
    ));
}

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
    let hold = editor.doc().hold.for_class(choice.0);
    let stage = stage_in_force(stage.as_deref(), mode.as_deref());
    for (skeleton, mut pose, mut transform, mut reach, mut visibility) in &mut figures {
        // Hidden during the viewmodel preview: the eye is inside this head.
        let wanted = match preview.0 {
            true => Visibility::Hidden,
            false => Visibility::Inherited,
        };
        if *visibility != wanted {
            *visibility = wanted;
        }

        // A hat stays at the origin and the figure stands under it. Inverted
        // with the hat's scale, so a larger hat shows as a smaller figure.
        if editor.doc().archetype == Archetype::Hat {
            let fit = editor.doc().hat.for_class(choice.0);
            let Some(hat) = hat_transform(skeleton, &Pose::rest(), &Transform::IDENTITY, &fit)
            else {
                continue;
            };
            *pose = Pose::rest();
            *transform = Transform::from_matrix(hat.to_matrix().inverse());
            continue;
        }

        // A placed prop stands on the floor at its origin, so the figure stands
        // on the same floor beside it.
        if editor.doc().archetype.is_placed() {
            *pose = Pose::rest();
            *transform = Transform::from_translation(Vec3::X * PLACED_PROP_CLEARANCE);
            continue;
        }

        let mut standing = Pose::rest();

        let Some(local) =
            weapon_transform(skeleton, &standing, &Transform::IDENTITY, &hold, Quat::IDENTITY)
        else {
            continue;
        };
        // Placed from the hold before any clip runs; placed from the animated
        // weapon, the inversion tips the body over.
        let rotation = local.rotation.inverse();
        let root = match stage.anchor {
            Anchor::Weapon => Transform::from_translation(rotation * -local.translation)
                .with_rotation(rotation),
            Anchor::Floor => Transform::IDENTITY,
        };

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
                weapon: stage.holding.then_some(placed),
                eye: None,
                ground: &ground,
                // Same answer `ik::animation_pose` gives in a match.
                footfalls: corrections.as_deref().and_then(|c| c.0).and_then(|(state, inputs)| {
                    state
                        .stance(&inputs)
                        .map(|stance| crate::common::skeleton::gait::footfalls(inputs.stride, stance))
                }),
            };
            let weapon = stage.holding.then_some(&mut placed);
            apply_stack(&performance.0, clips, skeleton, &mut standing, &frames, weapon);
        }
        // The game's body-clip corrections, before the weapon is placed from
        // the pose they leave.
        if let Some((state, inputs)) = corrections.as_deref().and_then(|c| c.0) {
            crate::common::skeleton::ik::correct_for_state(
                skeleton,
                &mut standing,
                state,
                &inputs,
                &root,
            );
        }

        // On the floor the weapon follows the animated chest, as in the game,
        // with the weapon clip's change from rest carried on top.
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

        if stage.holding {
            reach.0 = grip_with(skeleton, &mut standing, &root, &hold, &placed);
        }

        *pose = standing;
        *transform = root;
    }
}

pub fn clear_the_figure(
    mut commands: Commands,
    existing: Query<Entity, With<ScaleFigureMarker>>,
    mut last: Local<Option<ScaleFigure>>,
) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }
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
                humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Mercenary)),
            ))
            .id();
        world.run_system_once(pose_the_figure).unwrap();
        (world, figure)
    }

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

    #[test]
    fn empty_hands_are_not_put_on_a_grip() {
        use crate::common::app_mode::AppMode;
        let (world, figure) = staged(
            FigureStage { holding: false, anchor: Anchor::Weapon },
            AppMode::Animation,
        );
        assert!(hand_to_grip(&world, figure) > 0.05, "an empty hand went to the grip");
    }

    fn hand_to_grip(world: &World, figure: Entity) -> f32 {
        let skeleton = world.get::<Skeleton>(figure).unwrap();
        let pose = world.get::<Pose>(figure).unwrap();
        let root = world.get::<Transform>(figure).unwrap();
        let index = skeleton.index_of(bone::HAND_R).expect("bone is in the rig");
        let hand = skeleton.posed_bones(pose, root)[index].head;
        (hand - HoldSpec::default().grip()).length()
    }

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

    /// Fails if a body clip is previewed without the game's corrections: the
    /// idle's settle then lifts the feet off the floor.
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

        let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(Class::Mercenary));
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

    /// Fails if any class's hand misses the grip at the prop's origin, through
    /// the two calls `pose_the_figure` makes.
    #[test]
    fn every_class_stands_holding_the_prop_where_it_is() {
        let hold = HoldSpec::default();
        for class in [
            Class::Scout, Class::Soldier, Class::Pyro, Class::Demoman,
            Class::Heavy, Class::Engineer, Class::Medic, Class::Sniper,
            Class::Spy, Class::Civilian, Class::Mercenary,
        ] {
            let skeleton = humanoid(crate::common::class::ClassBuilds::shipped().of(class));
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

            assert!(
                root.translation.y < -0.9,
                "{class:?} stands {} below its own hand",
                -root.translation.y,
            );
        }
    }
}

