//! What is being done to a body that is not damage: humiliation and taunts
//! now, stuns later. Gameplay state with one writer, the authority, and
//! replicated; firing, movement, the held weapon, the view and the animation
//! all read it. See `documentation/animation-layers.md`.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::app_mode::AppMode;
use crate::common::assets::Assets as PackAssets;
use crate::common::assets::default_packs;
use crate::common::skeleton::clip::Clip;
use crate::common::skeleton::play::{ClipStack, Layer};
use crate::common::taunt::{TauntId, TauntWeapon, Taunts};
use crate::game::player::{Inputs, LocalPlayer, ViewMode, ViewPreference};

/// Not an enum: a humiliated body may still taunt, as in TF2, and the taunt
/// outranks the humiliation while it lasts.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Reflect, Serialize, Deserialize)]
#[require(ClipStack)]
pub struct Condition {
    /// Hands up, weapon gone, no shooting. `H` toggles it until there are
    /// rounds to lose.
    pub humiliated: bool,
    /// `G` toggles the first taunt in `taunts.toml` until there is a way to
    /// choose one.
    pub taunt: Option<TauntId>,
}

impl Condition {
    pub const HUMILIATED: Condition = Condition { humiliated: true, taunt: None };

    pub fn may_fire(self) -> bool {
        !self.humiliated && self.taunt.is_none()
    }

    /// A taunt decides for itself; one whose entry has not arrived hides it,
    /// since a weapon flickering in and out is worse than one missing.
    pub fn shows_weapon(self, taunts: Option<&Taunts>) -> bool {
        match self.taunt {
            Some(id) => taunts
                .and_then(|taunts| taunts.get(id))
                .is_some_and(|taunt| taunt.weapon == TauntWeapon::Shown),
            None => !self.humiliated,
        }
    }

    pub fn forces_third_person(self) -> bool {
        self.humiliated || self.taunt.is_some()
    }

    /// What the body's own speed is multiplied by. A taunt whose entry has not
    /// arrived leaves it alone, and the server corrects the guess.
    pub fn speed_factor(self, taunts: Option<&Taunts>) -> f32 {
        self.taunt
            .and_then(|id| taunts?.get(id))
            .map_or(1.0, |taunt| taunt.movement.speed_factor())
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

/// Reads the edges and never takes them: `write_client_inputs` already did.
pub fn toggle_conditions(
    taunts: Option<Res<Taunts>>,
    mut bodies: Query<(&Inputs, &mut Condition)>,
) {
    for (input, mut condition) in &mut bodies {
        if input.humiliate {
            condition.humiliated = !condition.humiliated;
        }
        if input.taunt {
            condition.taunt = match condition.taunt {
                Some(_) => None,
                None => taunts.as_deref().and_then(Taunts::first).map(|taunt| taunt.id),
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

const HUMILIATED_CLIP: &str = "humiliated";

/// Each condition's clip, read from this machine's packs. A condition whose
/// file is missing plays nothing and still does everything else it does.
#[derive(Resource, Default)]
pub struct ConditionClips {
    humiliated: Option<Handle<Clip>>,
    /// By clip name. A miss is remembered too, so it is not read again every
    /// time the table changes.
    taunts: BTreeMap<String, Option<Handle<Clip>>>,
}

pub fn load_condition_clips(
    mut commands: Commands,
    packs: Res<PackAssets>,
    mut clips: ResMut<Assets<Clip>>,
) {
    let humiliated = crate::common::clip_file::load_named_clip(&packs, &default_packs(), HUMILIATED_CLIP)
        .map(|clip| clips.add(clip));
    commands.insert_resource(ConditionClips { humiliated, taunts: BTreeMap::new() });
}

/// The taunt table can arrive from the server after startup, naming clips
/// nobody has read yet. Clips themselves are not sent: each machine draws
/// from its own pack, as with body animations.
pub fn load_taunt_clips(
    taunts: Option<Res<Taunts>>,
    packs: Res<PackAssets>,
    mut clips: ResMut<Assets<Clip>>,
    mut loaded: ResMut<ConditionClips>,
) {
    let Some(taunts) = taunts.filter(|taunts| taunts.is_changed()) else { return };
    for taunt in &taunts.0 {
        if loaded.taunts.contains_key(&taunt.clip) {
            continue;
        }
        let clip = crate::common::clip_file::load_named_clip(&packs, &default_packs(), &taunt.clip);
        if clip.is_none() {
            warn!("The `{}` taunt's clip `{}` is not in any pack", taunt.key, taunt.clip);
        }
        loaded.taunts.insert(taunt.clip.clone(), clip.map(|clip| clips.add(clip)));
    }
}

/// Keep each body's condition clips on its stack, and tick them. Runs in the
/// pose chain before `play_the_clips`, so starting one and playing it are
/// never a frame apart.
pub(crate) fn perform_conditions(
    time: Res<Time>,
    clips: Option<Res<ConditionClips>>,
    taunts: Option<Res<Taunts>>,
    mut bodies: Query<(&Condition, &mut ClipStack)>,
) {
    let delta = time.delta_secs();
    let clips = clips.as_deref();
    for (condition, mut stack) in &mut bodies {
        let humiliation = condition
            .humiliated
            .then(|| clips?.humiliated.as_ref())
            .flatten()
            .map(|clip| (clip, Layer::Humiliation.claims()));
        let taunt = condition
            .taunt
            .and_then(|id| taunts.as_deref()?.get(id))
            .and_then(|taunt| Some((clips?.taunts.get(&taunt.clip)?.as_ref()?, taunt.claims)));

        for (layer, wanted) in [(Layer::Humiliation, humiliation), (Layer::Taunt, taunt)] {
            let ours = |playing: Layer| playing == layer;
            if wanted.is_none() && !stack.has_staying(ours) {
                continue;
            }
            if let Some(playing) = stack.keep(ours, wanted.map(|(clip, _)| (layer, clip))) {
                playing.time += delta;
                if let Some((_, claims)) = wanted {
                    playing.claims = claims;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::skeleton::play::Playing;
    use crate::game::player::PlayerInput;

    #[test]
    fn h_humiliates_and_h_again_lets_go() {
        let mut world = World::new();
        let body = world.spawn((Condition::default(), Inputs::default())).id();

        let press = |world: &mut World, pressed: bool| {
            world.get_mut::<Inputs>(body).unwrap().0 = PlayerInput { humiliate: pressed, ..default() };
            world.run_system_once(toggle_conditions).unwrap();
            *world.get::<Condition>(body).unwrap()
        };

        assert_eq!(press(&mut world, true), Condition::HUMILIATED);
        assert_eq!(press(&mut world, false), Condition::HUMILIATED, "a tick with no press toggled");
        assert_eq!(press(&mut world, true), Condition::default());
    }

    /// Forced, and the preference is still there afterwards.
    #[test]
    fn humiliation_forces_third_person_and_gives_the_view_back() {
        let mut world = World::new();
        world.insert_resource(ViewPreference(ViewMode::FirstPerson));
        world.init_resource::<ViewMode>();
        let body = world.spawn((LocalPlayer, Condition::HUMILIATED)).id();

        world.run_system_once(decide_the_view).unwrap();
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::ThirdPerson);
        assert_eq!(world.resource::<ViewPreference>().0, ViewMode::FirstPerson);

        *world.get_mut::<Condition>(body).unwrap() = Condition::default();
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
        world.spawn(Condition::HUMILIATED);
        world.spawn((LocalPlayer, Condition::default()));

        world.run_system_once(decide_the_view).unwrap();
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::FirstPerson);
    }

    /// The clip goes on with the condition and is sent out with it, and a
    /// weapon's clip beside it is left alone.
    #[test]
    fn the_condition_clip_follows_the_condition() {
        let mut world = World::new();
        world.init_resource::<Time>();
        let mut clips = Assets::<Clip>::default();
        let humiliated = clips.add(Clip::default());
        let deploy = clips.add(Clip::default());
        world.insert_resource(ConditionClips { humiliated: Some(humiliated.clone()), taunts: BTreeMap::new() });
        let body = world
            .spawn((
                Condition::HUMILIATED,
                ClipStack(vec![Playing::new(Layer::Weapon, deploy, 0.0)]),
            ))
            .id();

        world.run_system_once(perform_conditions).unwrap();
        let stack = world.get::<ClipStack>(body).unwrap();
        assert_eq!(stack.0.len(), 2);
        assert!(stack.0.iter().any(|playing| playing.layer == Layer::Humiliation && !playing.leaving));

        *world.get_mut::<Condition>(body).unwrap() = Condition::default();
        world.run_system_once(perform_conditions).unwrap();
        let stack = world.get::<ClipStack>(body).unwrap();
        let leaving = |layer| stack.0.iter().find(|playing| playing.layer == layer).unwrap().leaving;
        assert!(leaving(Layer::Humiliation), "the humiliation clip outlived humiliation");
        assert!(!leaving(Layer::Weapon), "the weapon's clip was sent out too");
    }

    /// A humiliated body may still taunt, as in TF2: both clips play, the
    /// taunt on top with the claims its entry states.
    #[test]
    fn a_humiliated_body_can_still_taunt() {
        let taunts = Taunts::shipped();
        let wave = taunts.first().unwrap().clone();
        let mut world = World::new();
        world.init_resource::<Time>();
        let mut clips = Assets::<Clip>::default();
        let humiliated = clips.add(Clip::default());
        let waving = clips.add(Clip::default());
        world.insert_resource(ConditionClips {
            humiliated: Some(humiliated),
            taunts: BTreeMap::from([(wave.clip.clone(), Some(waving))]),
        });
        world.insert_resource(taunts.clone());
        let both = Condition { humiliated: true, taunt: Some(wave.id) };
        let body = world.spawn(both).id();

        world.run_system_once(perform_conditions).unwrap();
        let stack = world.get::<ClipStack>(body).unwrap();
        let on = |layer| stack.0.iter().find(|playing| playing.layer == layer && !playing.leaving);
        assert!(on(Layer::Humiliation).is_some(), "the taunt pushed humiliation off");
        assert_eq!(on(Layer::Taunt).expect("no taunt playing").claims, wave.claims);

        assert!(!both.may_fire());
        assert!(both.forces_third_person());
        assert!(!both.shows_weapon(Some(&taunts)), "the wave says the weapon is hidden");
        assert_eq!(both.speed_factor(Some(&taunts)), 0.0);
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

    /// Every build can get its right hand up by its head and sweep it from
    /// one side of the wave to the other.
    #[test]
    fn every_class_can_wave() {
        use strum::IntoEnumIterator;
        use crate::common::class::{Class, ClassBuilds};
        use crate::common::skeleton::ik::Reach;
        use crate::common::skeleton::play::{apply, Frames};
        use crate::common::skeleton::rig::bone;
        use crate::common::skeleton::{humanoid, Pose};

        let taunts = Taunts::shipped();
        let wave = taunts.first().expect("the default pack ships no taunt");
        let clip = crate::common::clip_file::load_named_clip(
            &PackAssets::default(),
            &[std::path::PathBuf::from("assets/default")],
            &wave.clip,
        )
        .expect("the first taunt's clip is not in the default pack");
        let ground = |_: Vec3| 0.0;

        for class in Class::iter() {
            let rig = humanoid(ClassBuilds::shipped().of(class));
            for time in [0.0, 0.2, 0.4] {
                let mut pose = Pose::rest();
                let root = Transform::IDENTITY;
                let carry = crate::prop::hold::carry_frame(&rig, &pose, &root, Quat::IDENTITY).unwrap();
                let reaches = apply(&clip, time, &rig, &mut pose, &Frames::standing(root, carry, &ground), None);
                assert_eq!(reaches, vec![Reach::Reached], "{class:?} cannot wave at {time} s");

                let bones = rig.posed_bones(&pose, &root);
                let head = &bones[rig.index_of(bone::HEAD).unwrap()];
                let wrist = bones[rig.index_of(bone::FOREARM_R).unwrap()].tail;
                // Halfway up the head: the arm is up, whatever the build's
                // neck and shoulders make of exactly how far.
                let eyes = (head.head.y + head.tail.y) / 2.0;
                assert!(wrist.y > eyes, "{class:?}'s wrist is below its eyes at {time} s");
            }
        }
    }
}
