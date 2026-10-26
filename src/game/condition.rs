//! What is being done to a body that is not damage: humiliation now, taunts
//! and stuns later. Gameplay state with one writer, the authority, and
//! replicated; firing, the held weapon, the view and the animation all read
//! it. See `documentation/animation-layers.md`.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::assets::Assets as PackAssets;
use crate::common::assets::default_packs;
use crate::common::skeleton::clip::Clip;
use crate::common::skeleton::play::{ClipStack, Layer, Playing};
use crate::game::player::{Inputs, LocalPlayer, ViewMode, ViewPreference};

#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Reflect, Serialize, Deserialize)]
#[require(ClipStack)]
pub enum Condition {
    #[default]
    Normal,
    /// Hands up, weapon gone, no shooting. `H` toggles it until there are
    /// rounds to lose.
    Humiliated,
}

impl Condition {
    pub fn may_fire(self) -> bool {
        matches!(self, Condition::Normal)
    }

    pub fn shows_weapon(self) -> bool {
        matches!(self, Condition::Normal)
    }

    pub fn forces_third_person(self) -> bool {
        !matches!(self, Condition::Normal)
    }

    /// The layer its clip plays on, and the clip's file under `animations/`.
    fn performance(self) -> Option<(Layer, &'static str)> {
        match self {
            Condition::Normal => None,
            Condition::Humiliated => Some((Layer::Humiliation, "humiliated")),
        }
    }
}

/// The toggles and the view: gameplay, so no asset plugins needed.
pub struct ConditionPlugin;

impl Plugin for ConditionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            toggle_conditions
                .before(crate::game::weapon::pull_trigger)
                .run_if(in_state(AppMode::Play))
                .run_if(crate::common::net::has_authority),
        )
        .add_systems(
            Update,
            decide_the_view
                .after(crate::game::player::toggle_view)
                .run_if(in_state(AppMode::Play)),
        );
    }
}

/// Reads the edge and never takes it: `write_client_inputs` already did.
pub fn toggle_conditions(mut bodies: Query<(&Inputs, &mut Condition)>) {
    for (input, mut condition) in &mut bodies {
        if input.humiliate {
            *condition = match *condition {
                Condition::Humiliated => Condition::Normal,
                Condition::Normal => Condition::Humiliated,
            };
        }
    }
}

/// The one writer of [`ViewMode`]: the player's preference, unless our body's
/// condition forces third person. The preference is never overwritten, so
/// it is still there when the condition ends.
pub fn decide_the_view(
    preference: Res<ViewPreference>,
    ours: Option<Single<&Condition, With<LocalPlayer>>>,
    mut view: ResMut<ViewMode>,
) {
    let wanted = match ours.is_some_and(|condition| condition.forces_third_person()) {
        true => ViewMode::ThirdPerson,
        false => preference.0,
    };
    if *view != wanted {
        *view = wanted;
    }
}

/// Each condition's clip, read from the packs at startup. A condition whose
/// file is missing plays nothing and still does everything else it does.
#[derive(Resource, Default)]
pub struct ConditionClips {
    humiliated: Option<Handle<Clip>>,
}

impl ConditionClips {
    fn of(&self, condition: Condition) -> Option<&Handle<Clip>> {
        match condition {
            Condition::Normal => None,
            Condition::Humiliated => self.humiliated.as_ref(),
        }
    }
}

pub fn load_condition_clips(
    mut commands: Commands,
    packs: Res<PackAssets>,
    mut clips: ResMut<Assets<Clip>>,
) {
    let mut load = |condition: Condition| {
        let (_, name) = condition.performance()?;
        let clip = crate::common::clip_file::load_named_clip(&packs, &default_packs(), name)?;
        Some(clips.add(clip))
    };
    let humiliated = load(Condition::Humiliated);
    commands.insert_resource(ConditionClips { humiliated });
}

/// Keep each body's condition clip on its stack, and tick it. Runs in the
/// pose chain before `play_the_clips`, so starting one and playing it are
/// never a frame apart.
pub(crate) fn perform_conditions(
    time: Res<Time>,
    clips: Option<Res<ConditionClips>>,
    mut bodies: Query<(&Condition, &mut ClipStack)>,
) {
    let delta = time.delta_secs();
    for (condition, mut stack) in &mut bodies {
        let wanted = condition
            .performance()
            .and_then(|(layer, _)| Some((layer, clips.as_deref()?.of(*condition)?)));
        let playing = stack.0.iter().position(|playing| is_a_condition(playing.layer));

        match (wanted, playing) {
            (Some((layer, handle)), Some(index))
                if stack.0[index].layer == layer && stack.0[index].clip == *handle =>
            {
                stack.0[index].time += delta;
            }
            (Some((layer, handle)), _) => {
                stack.0.retain(|playing| !is_a_condition(playing.layer));
                stack.0.push(Playing::new(layer, handle.clone(), 0.0));
            }
            (None, Some(_)) => stack.0.retain(|playing| !is_a_condition(playing.layer)),
            (None, None) => {}
        }
    }
}

fn is_a_condition(layer: Layer) -> bool {
    matches!(layer, Layer::Humiliation)
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::game::player::PlayerInput;

    #[test]
    fn h_humiliates_and_h_again_lets_go() {
        let mut world = World::new();
        let body = world.spawn((Condition::Normal, Inputs::default())).id();

        let press = |world: &mut World, pressed: bool| {
            world.get_mut::<Inputs>(body).unwrap().0 = PlayerInput { humiliate: pressed, ..default() };
            world.run_system_once(toggle_conditions).unwrap();
            *world.get::<Condition>(body).unwrap()
        };

        assert_eq!(press(&mut world, true), Condition::Humiliated);
        assert_eq!(press(&mut world, false), Condition::Humiliated, "a tick with no press toggled");
        assert_eq!(press(&mut world, true), Condition::Normal);
    }

    /// Forced, and the preference is still there afterwards.
    #[test]
    fn humiliation_forces_third_person_and_gives_the_view_back() {
        let mut world = World::new();
        world.insert_resource(ViewPreference(ViewMode::FirstPerson));
        world.init_resource::<ViewMode>();
        let body = world.spawn((LocalPlayer, Condition::Humiliated)).id();

        world.run_system_once(decide_the_view).unwrap();
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::ThirdPerson);
        assert_eq!(world.resource::<ViewPreference>().0, ViewMode::FirstPerson);

        *world.get_mut::<Condition>(body).unwrap() = Condition::Normal;
        world.run_system_once(decide_the_view).unwrap();
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::FirstPerson);
    }

    /// Only the local body forces anything: somebody else being humiliated
    /// is not a reason to move our camera.
    #[test]
    fn somebody_elses_humiliation_leaves_our_view_alone() {
        let mut world = World::new();
        world.insert_resource(ViewPreference(ViewMode::FirstPerson));
        world.init_resource::<ViewMode>();
        world.spawn(Condition::Humiliated);
        world.spawn((LocalPlayer, Condition::Normal));

        world.run_system_once(decide_the_view).unwrap();
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::FirstPerson);
    }

    /// The clip goes on with the condition and comes off with it, and a
    /// weapon's clip beside it is left alone.
    #[test]
    fn the_condition_clip_follows_the_condition() {
        let mut world = World::new();
        world.init_resource::<Time>();
        let mut clips = Assets::<Clip>::default();
        let humiliated = clips.add(Clip::default());
        let deploy = clips.add(Clip::default());
        world.insert_resource(ConditionClips { humiliated: Some(humiliated.clone()) });
        let body = world
            .spawn((
                Condition::Humiliated,
                ClipStack(vec![Playing::new(Layer::Weapon, deploy, 0.0)]),
            ))
            .id();

        world.run_system_once(perform_conditions).unwrap();
        let stack = world.get::<ClipStack>(body).unwrap();
        assert_eq!(stack.0.len(), 2);
        assert_eq!(stack.owner(crate::common::skeleton::play::Regions::ARM_R), Some(Layer::Humiliation));

        *world.get_mut::<Condition>(body).unwrap() = Condition::Normal;
        world.run_system_once(perform_conditions).unwrap();
        let stack = world.get::<ClipStack>(body).unwrap();
        assert_eq!(stack.0.len(), 1, "the humiliation clip outlived humiliation");
        assert_eq!(stack.0[0].layer, Layer::Weapon, "the weapon's clip was taken off too");
    }

    /// Every build gets both hands up from the shipped clip, at both ends of
    /// its bob: a hand that cannot reach is left hanging, which reads as a
    /// broken pose rather than an error.
    #[test]
    fn every_class_can_put_its_hands_up() {
        use strum::IntoEnumIterator;
        use crate::common::class::{Class, ClassBuilds};
        use crate::common::skeleton::ik::Reach;
        use crate::common::skeleton::play::{apply, Frames};
        use crate::common::skeleton::rig::bone;
        use crate::common::skeleton::{humanoid, Pose};

        let clip = crate::common::clip_file::load_named_clip(
            &PackAssets::default(),
            &[std::path::PathBuf::from("assets/default")],
            "humiliated",
        )
        .expect("the default pack should ship a humiliation clip");
        let ground = |_: Vec3| 0.0;

        for class in Class::iter() {
            let rig = humanoid(ClassBuilds::shipped().of(class));
            for time in [0.0, 0.6] {
                let mut pose = Pose::rest();
                let root = Transform::IDENTITY;
                let carry = crate::prop::hold::carry_frame(&rig, &pose, &root, Quat::IDENTITY).unwrap();
                let reaches = apply(&clip, time, &rig, &mut pose, &Frames::standing(root, carry, &ground), None);
                assert_eq!(reaches, vec![Reach::Reached; 2], "{class:?} cannot get its hands up at {time} s");

                let bones = rig.posed_bones(&pose, &root);
                let y = |name| bones[rig.index_of(name).unwrap()].tail.y;
                for hand in [bone::HAND_R, bone::HAND_L] {
                    assert!(y(hand) > y(bone::NECK), "{class:?}'s {hand} is not up by its head");
                }
            }
        }
    }
}

