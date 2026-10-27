//! What is being done to a body that is not damage: humiliation and taunts
//! now, stuns later. Gameplay state with one writer, the authority, and
//! replicated; firing, movement, the held weapon, the view and the animation
//! all read it. See `documentation/animation-layers.md`.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use lightyear::prelude::{LocalTimeline, Tick};

use crate::common::app_mode::AppMode;
use crate::common::assets::Assets as PackAssets;
use crate::common::assets::default_packs;
use crate::common::skeleton::clip::Clip;
use crate::common::skeleton::play::{ClipStack, Layer};
use crate::common::taunt::{TauntId, TauntTurning, TauntWeapon, Taunts};
use crate::game::player::{
    CameraPull, DeathCam, Inputs, LocalPlayer, ViewMode, ViewPreference, CAMERA_PULL_SECONDS,
};

/// Not an enum: a humiliated body may still taunt, as in TF2, and the taunt
/// outranks the humiliation while it lasts.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Reflect, Serialize, Deserialize)]
#[require(ClipStack, TauntClock)]
pub struct Condition {
    /// `H` toggles it until there are rounds to lose.
    pub humiliated: bool,
    /// `G` toggles the first taunt in `taunts.toml` until there is a menu.
    pub taunt: Option<TauntId>,
    /// The authority's tick the taunt began on, so a viewer arriving mid-taunt
    /// starts the clip partway. Lightyear's, since a client's timeline is
    /// synced to the server's.
    pub taunt_started: Tick,
}

/// Seconds into the current taunt, on the authority only.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct TauntClock(pub f32);

impl Condition {
    pub const HUMILIATED: Condition = Condition { humiliated: true, taunt: None, taunt_started: Tick(0) };

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

    /// A taunt's clip is authored standing, so a taunting body stands up as
    /// soon as there is room, crouch held or not.
    pub fn may_crouch(self) -> bool {
        self.taunt.is_none()
    }

    pub fn forces_third_person(self) -> bool {
        self.humiliated || self.taunt.is_some()
    }

    /// How far the body may follow the aim. Free with no taunt, or one whose
    /// entry has not arrived.
    pub fn turning(self, taunts: Option<&Taunts>) -> TauntTurning {
        self.taunt
            .and_then(|id| taunts?.get(id))
            .map_or(TauntTurning::Free, |taunt| taunt.turning)
    }

    /// What the body's own speed is multiplied by. A taunt whose entry has not
    /// arrived leaves it alone, and the server corrects the guess.
    pub fn speed_factor(self, taunts: Option<&Taunts>) -> f32 {
        self.taunt
            .and_then(|id| taunts?.get(id))
            .map_or(1.0, |taunt| taunt.movement.speed_factor())
    }
}

/// Gameplay, so no asset plugins needed.
pub struct ConditionPlugin;

impl Plugin for ConditionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (toggle_conditions, end_taunts)
                .chain()
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
    timeline: Option<Res<LocalTimeline>>,
    mut bodies: Query<(&Inputs, &mut Condition, &mut TauntClock)>,
) {
    for (input, mut condition, mut clock) in &mut bodies {
        if input.humiliate {
            condition.humiliated = !condition.humiliated;
        }
        if input.taunt {
            condition.taunt = match condition.taunt {
                Some(_) => None,
                None => taunts.as_deref().and_then(Taunts::first).map(|taunt| taunt.id),
            };
            condition.taunt_started = timeline.as_deref().map_or_else(Tick::default, LocalTimeline::tick);
            clock.0 = 0.0;
        }
    }
}

/// A taunt whose entry is gone ends at once. Counted on the authority's own
/// clock rather than off `taunt_started`, so it needs no network timeline.
pub fn end_taunts(
    // Inside `FixedUpdate` this is the fixed clock, as `step_player` reads it.
    time: Res<Time>,
    taunts: Option<Res<Taunts>>,
    mut bodies: Query<(&mut Condition, &mut TauntClock)>,
) {
    for (mut condition, mut clock) in &mut bodies {
        let Some(id) = condition.taunt else { continue };
        clock.0 += time.delta_secs();
        let length = match taunts.as_deref().and_then(|taunts| taunts.get(id)) {
            Some(taunt) => taunt.length,
            None => Some(0.0),
        };
        if length.is_some_and(|length| clock.0 >= length) {
            condition.taunt = None;
        }
    }
}

/// The one writer of [`ViewMode`] and [`CameraPull`]. The preference is
/// never overwritten, so it returns when the condition ends. The view is
/// third person until the camera is all the way back in the head.
///
/// A [`DeathCam`] forces third person too, until a living body is ours again.
/// That cuts rather than swings, since the new body is somewhere else
/// entirely.
pub fn decide_the_view(
    mut commands: Commands,
    time: Res<Time>,
    preference: Res<ViewPreference>,
    death_cam: Option<Res<DeathCam>>,
    ours: Option<Single<&Condition, With<LocalPlayer>>>,
    mut pull: ResMut<CameraPull>,
    mut view: ResMut<ViewMode>,
) {
    // A death cam only starts once our body is gone, so any body is a new one.
    let respawned = death_cam.is_some() && ours.is_some();
    if respawned {
        commands.remove_resource::<DeathCam>();
    }
    let dead = death_cam.is_some() && !respawned;
    let forced = dead || ours.is_some_and(|condition| condition.forces_third_person());
    let wanted = match forced {
        true => ViewMode::ThirdPerson,
        false => preference.0,
    };
    let out = match wanted {
        ViewMode::ThirdPerson => 1.0,
        ViewMode::FirstPerson => 0.0,
    };
    let step = match respawned {
        true => 1.0,
        false => time.delta_secs() / CAMERA_PULL_SECONDS,
    };
    let pulled = pull.0 + (out - pull.0).clamp(-step, step);
    if pull.0 != pulled {
        pull.0 = pulled;
    }

    let drawn = match pull.0 > 0.0 {
        true => ViewMode::ThirdPerson,
        false => ViewMode::FirstPerson,
    };
    if *view != drawn {
        *view = drawn;
    }
}

const HUMILIATED_CLIP: &str = "humiliated";

/// From this machine's packs. A missing clip plays nothing; the condition
/// still applies.
#[derive(Resource, Default)]
pub struct ConditionClips {
    humiliated: Option<Handle<Clip>>,
    /// By clip name. A miss is kept too, so it is not read again.
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

/// The table can arrive from the server after startup. Clips are not sent:
/// each machine reads its own pack.
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

/// In the pose chain before `play_the_clips`, so a clip starts and plays in
/// the same frame.
pub(crate) fn perform_conditions(
    time: Res<Time>,
    clips: Option<Res<ConditionClips>>,
    taunts: Option<Res<Taunts>>,
    timeline: Option<Res<LocalTimeline>>,
    mut bodies: Query<(&Condition, &mut ClipStack)>,
) {
    let delta = time.delta_secs();
    let now = timeline.as_deref().map(LocalTimeline::tick);
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
            let arriving = !stack.has_staying(ours);
            if wanted.is_none() && arriving {
                continue;
            }
            if let Some(playing) = stack.keep(ours, wanted.map(|(clip, _)| (layer, clip))) {
                // Lined up with the authority on arrival, then ticked by frame:
                // read straight off the timeline it would step at the tick rate.
                playing.time = match (arriving, layer, now) {
                    (true, Layer::Taunt, Some(now)) => since(condition.taunt_started, now),
                    _ => playing.time + delta,
                };
                if let Some((_, claims)) = wanted {
                    playing.claims = claims;
                }
            }
        }
    }
}

/// Seconds from `started` to `now`, never negative: a client's timeline runs
/// a little ahead of the server's, never behind.
fn since(started: Tick, now: Tick) -> f32 {
    ((now - started).max(0) as f64 / crate::constants::TICK_HZ) as f32
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::skeleton::play::Playing;
    use crate::game::player::PlayerInput;

    /// A taunt's clip is authored standing.
    #[test]
    fn a_taunt_stands_the_body_up_and_humiliation_does_not() {
        let taunting = Condition { taunt: Some(Taunts::shipped().first().unwrap().id), ..default() };
        assert!(!taunting.may_crouch());
        assert!(Condition::HUMILIATED.may_crouch(), "humiliation stopped a crouch, which TF2 allows");
        assert!(Condition::default().may_crouch());
    }

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

    fn a_view_world(preference: ViewMode) -> World {
        let mut world = World::new();
        world.insert_resource(ViewPreference(preference));
        world.init_resource::<ViewMode>();
        world.init_resource::<CameraPull>();
        world.init_resource::<Time>();
        world
    }

    /// One frame of `seconds`, deciding the view.
    fn decide_after(world: &mut World, seconds: f32) {
        world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(seconds));
        world.run_system_once(decide_the_view).unwrap();
    }

    /// Forced, and the preference is still there afterwards.
    #[test]
    fn humiliation_forces_third_person_and_gives_the_view_back() {
        let mut world = a_view_world(ViewMode::FirstPerson);
        let body = world.spawn((LocalPlayer, Condition::HUMILIATED)).id();

        decide_after(&mut world, 1.0);
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::ThirdPerson);
        assert_eq!(world.resource::<ViewPreference>().0, ViewMode::FirstPerson);

        *world.get_mut::<Condition>(body).unwrap() = Condition::default();
        decide_after(&mut world, 1.0);
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::FirstPerson);
    }

    /// The body is drawn whenever the camera is out of the head, including on
    /// the way back in.
    #[test]
    fn the_camera_swings_out_and_the_body_shows_until_it_is_back() {
        let mut world = a_view_world(ViewMode::FirstPerson);
        let body = world.spawn((LocalPlayer, Condition::HUMILIATED)).id();
        let pull = |world: &World| world.resource::<CameraPull>().0;

        decide_after(&mut world, CAMERA_PULL_SECONDS / 2.0);
        assert!((pull(&world) - 0.5).abs() < 1e-4, "{}", pull(&world));
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::ThirdPerson, "the body was not drawn as the camera left");
        decide_after(&mut world, CAMERA_PULL_SECONDS);
        assert_eq!(pull(&world), 1.0);

        *world.get_mut::<Condition>(body).unwrap() = Condition::default();
        decide_after(&mut world, CAMERA_PULL_SECONDS * 0.9);
        assert!(pull(&world) > 0.0);
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::ThirdPerson, "the body vanished with the camera still out");
        decide_after(&mut world, CAMERA_PULL_SECONDS);
        assert_eq!(pull(&world), 0.0);
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::FirstPerson);
    }

    /// Halfway out is between the eye and the third-person spot, eased.
    #[test]
    fn a_half_pulled_camera_is_between_the_two_views() {
        let first = Transform::from_xyz(0.0, 1.6, 0.0);
        let third = Transform::from_xyz(0.0, 1.6, 3.5).with_rotation(Quat::from_rotation_y(0.4));
        assert_eq!(CameraPull(0.0).place(first, third).translation, first.translation);
        assert_eq!(CameraPull(1.0).place(first, third).translation, third.translation);
        let half = CameraPull(0.5).place(first, third);
        assert!((half.translation.z - 1.75).abs() < 1e-4, "{}", half.translation);
        assert!(half.rotation.angle_between(Quat::from_rotation_y(0.2)) < 1e-4);
    }

    /// Dead, the view is out of the head whatever the preference; the next
    /// living body cuts straight back in rather than swinging from the corpse.
    #[test]
    fn the_death_cam_holds_third_person_until_a_living_body_is_ours() {
        let mut world = a_view_world(ViewMode::FirstPerson);
        world.insert_resource(DeathCam { pivot: Vec3::ZERO, whose: None });

        decide_after(&mut world, 1.0);
        assert_eq!(world.resource::<CameraPull>().0, 1.0);
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::ThirdPerson);
        assert!(world.contains_resource::<DeathCam>());

        world.spawn((LocalPlayer, Condition::default()));
        decide_after(&mut world, 0.001);
        assert_eq!(world.resource::<CameraPull>().0, 0.0, "the camera swung in from the corpse");
        assert_eq!(*world.resource::<ViewMode>(), ViewMode::FirstPerson);
        assert!(!world.contains_resource::<DeathCam>());
    }

    /// Only the local body's condition moves our camera.
    #[test]
    fn somebody_elses_humiliation_leaves_our_view_alone() {
        let mut world = a_view_world(ViewMode::FirstPerson);
        world.spawn(Condition::HUMILIATED);
        world.spawn((LocalPlayer, Condition::default()));

        decide_after(&mut world, 1.0);
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
        let both = Condition { humiliated: true, taunt: Some(wave.id), ..default() };
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

    /// Somebody seen mid-taunt is shown where they are in it: the clip starts
    /// as far in as the authority's timeline says, then runs on the frame.
    #[test]
    fn a_taunt_seen_late_starts_partway_through() {
        let taunts = Taunts::shipped();
        let wave = taunts.first().unwrap().clone();
        let mut world = World::new();
        world.init_resource::<Time>();
        let mut timeline = LocalTimeline::default();
        timeline.apply_delta(96);
        world.insert_resource(timeline);
        let mut clips = Assets::<Clip>::default();
        let waving = clips.add(Clip::default());
        world.insert_resource(ConditionClips {
            humiliated: None,
            taunts: BTreeMap::from([(wave.clip.clone(), Some(waving))]),
        });
        world.insert_resource(taunts);
        let body = world
            .spawn(Condition { taunt: Some(wave.id), taunt_started: Tick(64), ..default() })
            .id();

        world.run_system_once(perform_conditions).unwrap();
        let time = |world: &World| world.get::<ClipStack>(body).unwrap().0[0].time;
        let half = (32.0 / crate::constants::TICK_HZ) as f32;
        assert!((time(&world) - half).abs() < 1e-5, "started at {} rather than {half}", time(&world));

        world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(0.1));
        world.run_system_once(perform_conditions).unwrap();
        assert!((time(&world) - half - 0.1).abs() < 1e-5, "it was lined up again rather than ticked");
    }

    /// Every build gets both hands up, at both ends of the bob: a hand that
    /// cannot reach is left hanging, with no error.
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
                for (forearm, hand) in [(bone::FOREARM_R, bone::HAND_R), (bone::FOREARM_L, bone::HAND_L)] {
                    assert_points_on(&rig, &bones, forearm, hand, class);
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
            for time in [0.0, 0.3, 0.6, 0.9, 1.2] {
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
                assert_points_on(&rig, &bones, bone::FOREARM_R, bone::HAND_R, class);
            }
        }
    }

    /// A raised hand points up along its forearm. The solver keeps a tip's
    /// world rotation, so an unkeyed hand stays pointing at the floor.
    fn assert_points_on(
        rig: &crate::common::skeleton::Skeleton,
        bones: &[crate::common::skeleton::rig::PosedBone],
        forearm: &str,
        hand: &str,
        class: crate::common::class::Class,
    ) {
        let along = |name| {
            let bone = &bones[rig.index_of(name).unwrap()];
            (bone.tail - bone.head).normalize()
        };
        assert!(along(hand).y > 0.3, "{class:?}'s {hand} points down: {}", along(hand));
        assert!(
            along(hand).dot(along(forearm)) > 0.9,
            "{class:?}'s {hand} is bent off its forearm",
        );
    }

    /// The taunt's length and its clip's are two numbers for one thing.
    #[test]
    fn a_taunt_that_ends_lasts_as_long_as_its_clip() {
        for taunt in &Taunts::shipped().0 {
            let Some(length) = taunt.length else { continue };
            let clip = crate::common::clip_file::load_named_clip(
                &PackAssets::default(),
                &[std::path::PathBuf::from("assets/default")],
                &taunt.clip,
            )
            .unwrap();
            assert!(!clip.looping, "`{}` ends, but its clip loops", taunt.key);
            assert!((clip.length - length).abs() < 1e-4, "`{}` lasts {length} s and its clip {} s", taunt.key, clip.length);
        }
    }
}
