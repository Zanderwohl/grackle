//! The pistol's taunt: spun on the trigger finger, caught, raised barrel-up before the mouth and
//! blown across. A generator, as [`super::humanoid_seed`] is; the clip file is the source of truth
//! once written.
//!
//! The weapon is keyed and the grip solves the trigger hand onto it, except while it spins, when
//! `Controller::Hold` lets go and the hand is keyed. The support hand goes to the hip, so the
//! taunt claims the left arm and not the right.
//!
//! Points are meters in [`Space::Shoulders`] (`+X` right, `+Y` up, `-Z` forward), since the
//! pistol is one size on every build. Angles are radians; the weapon's `x` lifts the barrel and
//! its `y` turns it left.

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;

use super::clip::{Axis, ChainRef, Channel, Clip, Clock, Combine, Controller, Key, Space, Track, Unit};
use super::seed::{fit, keyed};
use super::Interpolation;
use crate::common::rotation::quat_from_euler;

pub const TAUNT_TWIRL: &str = "taunt_twirl";
pub const TWIRL_LENGTH: f32 = 4.0;

/// `props/pistol.gpp` in its own frame: the guard's opening, the hold's grip and the muzzle.
pub const GUARD: Vec3 = Vec3::new(0.0, 0.16, -0.107);
pub const GRIP: Vec3 = Vec3::new(0.021, 0.07, -0.001);
pub const MUZZLE: Vec3 = Vec3::new(0.0, 0.212, -0.195);

/// The hold's carry at a level aim for a 0.56 m arm. It only shapes the curve next to the first and
/// last keys, which are each build's own hold; a longer arm's carry is out of the shortest
/// builds' reach.
const CARRY: Vec3 = Vec3::new(0.0168, -0.0896, -0.28);

/// The guard while it spins, in reach of every humanoid build. It spins about the body's `X`.
const SPIN_AT: Vec3 = Vec3::new(0.18, -0.21, -0.32);
/// Turns spun; seconds' share spent reaching full speed; the share of full speed left at the catch.
const TURNS: f32 = 3.0;
const SPIN_RAMP: f32 = 0.08;
const SPIN_END_SPEED: f32 = 0.25;
/// Meters: the radius of the circle the hand pumps with the spin.
const PUMP: f32 = 0.012;

/// Seconds: the grip letting go, the spin, and the catch.
const RELEASE: (f32, f32) = (0.36, 0.50);
const SPIN: (f32, f32) = (0.47, 1.57);
const CATCH: (f32, f32) = (1.52, 1.60);
/// While the guard rather than the grip carries the pistol. Both ends fall where it is level and
/// still, so switching does not move it.
const ON_THE_FINGER: (f32, f32) = (0.33, 1.8);

/// Radians: the flick's lift and the catch's rock past level.
const FLICK: f32 = 0.3;
const CATCH_ROCK: f32 = -0.15;

/// The muzzle for the blow: level with the lips, right of the middle so the camera behind the head
/// sees it. On the reference build the face is about 0.15 m ahead of the origin.
const BLOW_MUZZLE: Vec3 = Vec3::new(0.05, 0.12, -0.252);
/// Upright and in profile, the grip right of the barrel: held with the slide toward the face, the
/// grip is 0.14 m further out, past the shortest builds' reach.
const UPRIGHT: f32 = PI / 2.0 + 0.12;
const UPRIGHT_YAW: f32 = -PI / 2.0;
/// Radians the barrel tips toward the lips as the breath goes out.
const BLOW_TIP: f32 = 0.04;

/// The trigger hand's direction while the pistol spins, and meters from the wrist to the guard.
/// The wrist sits behind the guard and only a little below it, so the pistol swings clear of it.
const SPIN_HAND: Vec3 = Vec3::new(-0.15, 0.3, -1.0);
const FINGER: f32 = 0.12;

/// The support hand on the hip: wrist, where the hand points, and the elbow's pole.
const HIP_WRIST: Vec3 = Vec3::new(-0.2, -0.38, 0.03);
const HIP_HAND: Vec3 = Vec3::new(-0.13, -0.5, -0.05);
const HIP_ELBOW: [f32; 3] = [-1.0, -0.2, 0.6];
/// The support wrist at the hold on the reference build.
const SUPPORT_WRIST: Vec3 = Vec3::new(-0.02, -0.12, -0.26);
const SPIN_ELBOW: [f32; 3] = [1.0, -1.0, 0.2];

/// Beats, (seconds, share).
const LEFT_TO_HIP: &[(f32, f32)] = &[(0.0, 0.0), (0.12, 0.0), (0.48, 1.0), (3.45, 1.0), (3.9, 0.0), (4.0, 0.0)];
/// 1 at the spinning place. Back to 0 while the pistol is up, so the lowering goes to the hold.
const TO_SPIN: &[(f32, f32)] = &[(0.0, 0.0), (0.04, 0.0), (0.32, 1.0), (2.37, 1.0), (2.6, 0.0), (4.0, 0.0)];
/// The raise to the blow, the barrel turning up a little ahead of the lift.
const RAISE: &[(f32, f32)] = &[(0.0, 0.0), (1.84, 0.0), (2.36, 1.0), (3.42, 1.0), (3.98, 0.0), (4.0, 0.0)];
const TURN_UP: &[(f32, f32)] = &[(0.0, 0.0), (1.8, 0.0), (2.26, 1.0), (3.4, 1.0), (3.9, 0.0), (4.0, 0.0)];
const BREATH_IN: &[(f32, f32)] = &[(0.0, 0.0), (2.34, 0.0), (2.56, 1.0), (2.66, 1.0), (2.86, 0.0), (4.0, 0.0)];
const BLOW: &[(f32, f32)] = &[(0.0, 0.0), (2.62, 0.0), (2.95, 1.0), (3.12, 1.0), (3.36, 0.0), (4.0, 0.0)];
/// Meters the raise arcs above a straight line.
const RAISE_ARC: f32 = 0.06;

const WATCH: &[(f32, f32)] = &[(0.0, 0.0), (0.18, 0.0), (0.5, 1.0), (1.66, 1.0), (2.18, 0.15), (3.3, 0.15), (3.7, 0.0), (4.0, 0.0)];
const PLEASED: &[(f32, f32)] = &[(0.0, 0.0), (3.1, 0.0), (3.34, 1.0), (3.62, 1.0), (3.95, 0.0), (4.0, 0.0)];

/// Radians, `(pitch forward, yaw left, roll)`, at full share of their beat.
const CHEST_WATCH: Vec3 = Vec3::new(0.08, -0.12, 0.0);
const NECK_WATCH: Vec3 = Vec3::new(0.18, -0.08, 0.0);
const HEAD_WATCH: Vec3 = Vec3::new(0.26, -0.12, 0.0);
const CHEST_IN: Vec3 = Vec3::new(-0.05, 0.0, 0.0);
const NECK_IN: Vec3 = Vec3::new(-0.03, 0.0, 0.0);
const HEAD_IN: Vec3 = Vec3::new(-0.1, 0.0, 0.0);
const CHEST_BLOW: Vec3 = Vec3::new(0.07, 0.0, 0.0);
const NECK_BLOW: Vec3 = Vec3::new(0.1, 0.0, 0.0);
const HEAD_BLOW: Vec3 = Vec3::new(0.04, 0.0, 0.0);
const HEAD_PLEASED: Vec3 = Vec3::new(-0.06, 0.06, 0.1);

/// How finely the curves are sampled, and how far a fitted key may stray from them.
const SAMPLES_PER_SECOND: f32 = 960.0;
const POINT_TOLERANCE: f32 = 0.0005;
const ANGLE_TOLERANCE: f32 = 0.1 * PI / 180.0;
const WEIGHT_TOLERANCE: f32 = 0.005;

/// 0 to 1.
fn spun(seconds: f32) -> f32 {
    let along = ((seconds - SPIN.0) / (SPIN.1 - SPIN.0)).clamp(0.0, 1.0);
    let speed = |u: f32| (u / SPIN_RAMP).min(1.0) * (1.0 - (1.0 - SPIN_END_SPEED) * u);
    let steps = 400;
    let integral = |to: f32| (0..steps).map(|step| speed((step as f32 + 0.5) / steps as f32 * to)).sum::<f32>() * to / steps as f32;
    integral(along) / integral(1.0)
}

fn weapon_angles(seconds: f32) -> Vec3 {
    let flick = keyed(&[(0.0, 0.0), (0.34, 0.0), (SPIN.0, FLICK)], seconds);
    let spin = (TURNS * TAU + FLICK) * spun(seconds);
    let rock = keyed(&[(0.0, 0.0), (SPIN.1, 0.0), (1.64, CATCH_ROCK), (1.8, 0.0), (4.0, 0.0)], seconds);
    let up = keyed(TURN_UP, seconds);
    let tip = BLOW_TIP * keyed(BLOW, seconds);
    // Counted down, so the spin ends level.
    let pitch = TURNS * TAU + flick - spin + rock + (UPRIGHT + tip) * up;
    Vec3::new(pitch, UPRIGHT_YAW * up, 0.0)
}

fn weapon_turn(seconds: f32) -> Quat {
    quat_from_euler(weapon_angles(seconds))
}

fn held_grip(seconds: f32) -> Vec3 {
    let at_carry = CARRY + GRIP;
    let spinning = SPIN_AT - GUARD + GRIP;
    let caught = at_carry.lerp(spinning, keyed(TO_SPIN, seconds));

    let upright = quat_from_euler(Vec3::new(UPRIGHT, UPRIGHT_YAW, 0.0));
    let blowing = BLOW_MUZZLE - upright * MUZZLE + upright * GRIP;
    let raise = keyed(RAISE, seconds);
    let arc = Vec3::Y * RAISE_ARC * (PI * raise).sin();
    let blow = Vec3::new(0.0, 0.004, 0.0) * keyed(BLOW, seconds);
    caught.lerp(blowing, raise) + arc + blow
}

fn spinning_guard(seconds: f32) -> Vec3 {
    let pump = keyed(&[(0.0, 0.0), (SPIN.0, 0.0), (0.6, 1.0), (1.4, 1.0), (SPIN.1, 0.0), (4.0, 0.0)], seconds);
    let angle = (TURNS * TAU + FLICK) * spun(seconds);
    SPIN_AT + PUMP * pump * Vec3::new(0.0, angle.cos() - 1.0, -angle.sin())
}

fn weapon_origin(seconds: f32) -> Vec3 {
    let turn = weapon_turn(seconds);
    let spinning = (ON_THE_FINGER.0..ON_THE_FINGER.1).contains(&seconds);
    match spinning {
        true => spinning_guard(seconds) - turn * GUARD,
        false => held_grip(seconds) - turn * GRIP,
    }
}

/// The wrist and where the hand points, for while it is off the grip.
fn trigger_hand(seconds: f32) -> (Vec3, Vec3) {
    let guard = weapon_origin(seconds) + weapon_turn(seconds) * GUARD;
    let pointing = SPIN_HAND.normalize();
    (guard - pointing * FINGER, guard + pointing * 0.15)
}

fn hold_weight(seconds: f32) -> f32 {
    keyed(&[(0.0, 1.0), (RELEASE.0, 1.0), (RELEASE.1, 0.0), (CATCH.0, 0.0), (CATCH.1, 1.0), (4.0, 1.0)], seconds)
}

fn support_hand(seconds: f32) -> (Vec3, Vec3) {
    let share = keyed(LEFT_TO_HIP, seconds);
    let wrist = SUPPORT_WRIST.lerp(HIP_WRIST, share);
    let resting = SUPPORT_WRIST + Vec3::new(0.05, 0.0, -0.15);
    (wrist, resting.lerp(HIP_HAND, share))
}

fn spine(seconds: f32, watch: Vec3, breath_in: Vec3, blow: Vec3, pleased: Vec3) -> Vec3 {
    watch * keyed(WATCH, seconds)
        + breath_in * keyed(BREATH_IN, seconds)
        + blow * keyed(BLOW, seconds)
        + pleased * keyed(PLEASED, seconds)
}

pub fn clip() -> Clip {
    let count = (TWIRL_LENGTH * SAMPLES_PER_SECOND) as usize;
    let times: Vec<f32> = (0..=count).map(|step| TWIRL_LENGTH * step as f32 / count as f32).collect();
    let sampled = |value: &dyn Fn(f32) -> Vec3| -> Vec<Vec3> { times.iter().map(|at| value(*at)).collect() };
    let fitted = |values: Vec<Vec3>, tolerance: f32| -> Vec<Key> {
        let apart = |at: Vec3, index: usize| (at - values[index]).abs().max_element();
        fit(&times, &values, TWIRL_LENGTH, false, tolerance, apart)
            .into_iter()
            .map(|index| Key::all(times[index], values[index].to_array(), Interpolation::Spline))
            .collect()
    };
    let track = |space: Space, channel: Channel| Track {
        combine: Combine::Replace,
        space,
        unit: Unit::Meters,
        enabled: true,
        timing: Default::default(),
        channel,
    };
    let rotation = |bone: &str, watch, breath_in, blow, pleased| {
        let keys = fitted(sampled(&|at| spine(at, watch, breath_in, blow, pleased)), ANGLE_TOLERANCE);
        track(Space::Body, Channel::Rotation { bone: bone.into(), keys })
    };

    // The ends are each build's own hold, whatever its carry.
    let mut placed = fitted(sampled(&weapon_origin), POINT_TOLERANCE);
    for end in [0, placed.len() - 1] {
        placed[end].to = [Axis::Value(0.0); 3];
        placed[end].space = Some(Space::Weapon);
    }
    let weight = fitted(sampled(&|at| Vec3::new(hold_weight(at), 0.0, 0.0)), WEIGHT_TOLERANCE)
        .into_iter()
        .map(|key| Key { to: [key.to[0], Axis::Free, Axis::Free], ..key })
        .collect();

    Clip {
        name_key: String::new(),
        length: TWIRL_LENGTH,
        clock: Clock::Seconds,
        looping: false,
        blend_in: None,
        blend_out: None,
        note: "Written by `pistol_twirl::clip`: the support hand goes to the hip as the pistol comes \
               down by the right of the chest; a flick lifts the barrel and the grip lets go, and it \
               spins three times on the trigger finger, barrel down and back, slowing; the palm \
               closes on it with a rock; it comes up through an arc, turning upright, its barrel \
               before the lips; a breath in, a breath out across it with the head leaning in, a \
               pleased tilt, and back to the hold. The pistol is keyed and the trigger hand only \
               where the spin frees it, so retune it there."
            .into(),
        // The spine first: turning the chest after the arms are solved moves them off their targets.
        tracks: vec![
            rotation(super::rig::bone::CHEST, CHEST_WATCH, CHEST_IN, CHEST_BLOW, Vec3::ZERO),
            rotation(super::rig::bone::NECK, NECK_WATCH, NECK_IN, NECK_BLOW, Vec3::ZERO),
            rotation(super::rig::bone::HEAD, HEAD_WATCH, HEAD_IN, HEAD_BLOW, HEAD_PLEASED),
            track(Space::Shoulders, Channel::Effector {
                chain: ChainRef::RIGHT_ARM,
                pole: Some(SPIN_ELBOW),
                keys: fitted(sampled(&|at| trigger_hand(at).0), POINT_TOLERANCE),
            }),
            track(Space::Shoulders, Channel::LookAt {
                bone: super::rig::bone::HAND_R.into(),
                keys: fitted(sampled(&|at| trigger_hand(at).1), POINT_TOLERANCE),
            }),
            track(Space::Shoulders, Channel::Effector {
                chain: ChainRef::LEFT_ARM,
                pole: Some(HIP_ELBOW),
                keys: fitted(sampled(&|at| support_hand(at).0), POINT_TOLERANCE),
            }),
            track(Space::Shoulders, Channel::LookAt {
                bone: super::rig::bone::HAND_L.into(),
                keys: fitted(sampled(&|at| support_hand(at).1), POINT_TOLERANCE),
            }),
            track(Space::Shoulders, Channel::Weapon { keys: placed }),
            track(Space::Shoulders, Channel::WeaponRotation { keys: fitted(sampled(&weapon_angles), ANGLE_TOLERANCE) }),
            track(Space::Body, Channel::Weight { controller: Controller::Hold, keys: weight }),
        ],
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::class::{Class, ClassBuilds};
    use crate::common::skeleton::humanoid;
    use crate::common::skeleton::play::{sample_key, ClipStack, Layer, Playing};
    use crate::common::skeleton::rig::{bone, Pose, PosedBone, Skeleton};
    use crate::game::held::{grip_the_weapon, place_the_weapon, play_the_clips, HeldModel, WeaponInHand};
    use crate::game::player::Player;
    use crate::prop::document::PropDoc;
    use crate::prop::feature::FeatureOp;
    use crate::prop::hold::HoldSpec;

    fn pistol() -> PropDoc {
        let packs = crate::common::assets::default_packs();
        crate::prop::document::load(&crate::common::assets::Assets::default(), &packs[0], "pistol")
            .expect("the default pack ships no pistol")
    }

    fn guard_of(pistol: &PropDoc) -> Vec3 {
        pistol
            .features
            .iter()
            .find(|feature| feature.name == "Guard opening")
            .and_then(|feature| match &feature.op {
                FeatureOp::Primitive { placement, .. } => Some(Vec3::from_array(placement.origin)),
                _ => None,
            })
            .expect("the pistol has no guard opening")
    }

    struct Stage {
        clip: Clip,
        claims: Vec<String>,
        hold: HoldSpec,
        muzzle: Vec3,
        guard: Vec3,
    }

    impl Stage {
        /// The shipped clip, which may be hand-edited, and the generator's.
        fn both() -> [Stage; 2] {
            let text = std::fs::read_to_string(format!("assets/default/animations/{TAUNT_TWIRL}.gpa")).unwrap();
            let shipped = super::super::clip::StateAnimation::from_toml(&text).unwrap().clips()[0].clone();
            [Stage::with(shipped), Stage::with(clip())]
        }

        fn with(clip: Clip) -> Stage {
            let pistol = pistol();
            let taunt = crate::common::taunt::Taunts::shipped()
                .0
                .into_iter()
                .find(|taunt| Some(&taunt.key) == pistol.taunt.as_ref())
                .expect("the pistol names no taunt in taunts.toml");
            assert_eq!(taunt.clip, TAUNT_TWIRL);
            let muzzle = pistol.muzzle.expect("the pistol has no muzzle").point();
            Stage { clip, claims: taunt.claims, guard: guard_of(&pistol), hold: pistol.hold, muzzle }
        }

        fn keys(&self, wanted: impl Fn(&Channel) -> bool) -> &[Key] {
            self.clip
                .tracks
                .iter()
                .find(|track| wanted(&track.channel))
                .map(|track| match &track.channel {
                    Channel::Weight { keys, .. } | Channel::Effector { keys, .. } => keys.as_slice(),
                    _ => &[],
                })
                .unwrap_or(&[])
        }

        fn holding(&self, seconds: f32) -> f32 {
            let keys = self.keys(|channel| matches!(channel, Channel::Weight { controller: Controller::Hold, .. }));
            sample_key(keys, seconds, self.clip.length, false).and_then(|axes| axes[0].value()).unwrap_or(1.0)
        }

        /// In [`Space::Shoulders`].
        fn support_target(&self, seconds: f32) -> Vec3 {
            let keys = self.keys(|channel| matches!(channel, Channel::Effector { chain, .. } if *chain == ChainRef::LEFT_ARM));
            let axes = sample_key(keys, seconds, self.clip.length, false).expect("the support hand is not keyed");
            Vec3::from_array(axes.map(|axis| axis.value().expect("a support hand axis is free")))
        }

        /// Through the game's own systems. `None` is the hold with no clip.
        fn posed(&self, class: Class, seconds: Option<f32>) -> Posed {
            let skeleton = humanoid(ClassBuilds::shipped().of(class));
            let claims = skeleton.anatomy().claims(self.claims.iter().map(String::as_str));
            let mut world = World::new();
            let mut clips = bevy::asset::Assets::<Clip>::default();
            let handle = clips.add(self.clip.clone());
            world.insert_resource(clips);
            let stack = seconds.map(|seconds| Playing { claims, ..Playing::new(Layer::Taunt, handle, seconds) });
            let body = world
                .spawn((
                    skeleton.clone(),
                    Pose::rest(),
                    Player::default(),
                    HeldModel::with_hold(self.hold.for_class(Some(class))),
                    WeaponInHand::default(),
                    ClipStack(stack.into_iter().collect()),
                ))
                .id();
            world.run_system_once(place_the_weapon).unwrap();
            world.run_system_once(play_the_clips).unwrap();
            world.run_system_once(grip_the_weapon).unwrap();
            Posed { skeleton, pose: world.get::<Pose>(body).unwrap().clone(), weapon: world.get::<WeaponInHand>(body).unwrap().0 }
        }
    }

    struct Posed {
        skeleton: Skeleton,
        pose: Pose,
        weapon: Transform,
    }

    impl Posed {
        fn bone(&self, name: &str) -> PosedBone {
            self.skeleton.posed_bone(&self.pose, &Transform::IDENTITY, self.skeleton.index_of(name).unwrap())
        }

        /// The origin of [`Space::Shoulders`], which is taken before the clip leans the chest.
        fn shoulders(&self) -> Vec3 {
            let chest = self.skeleton.index_of(bone::CHEST).unwrap();
            self.skeleton.posed_bone(&Pose::rest(), &Transform::IDENTITY, chest).tail
        }

        /// Meters: to the head's axis, less half its depth.
        fn to_face(&self, muzzle: Vec3) -> f32 {
            let head = self.bone(bone::HEAD);
            let along = (muzzle - head.head).dot(head.tail - head.head) / (head.tail - head.head).length_squared();
            let axis = head.head.lerp(head.tail, along.clamp(0.0, 1.0));
            let depth = self.skeleton.bones()[self.skeleton.index_of(bone::HEAD).unwrap()].thickness.y;
            muzzle.distance(axis) - depth / 2.0
        }
    }

    fn every_humanoid() -> Vec<Class> {
        ClassBuilds::shipped().humanoids().collect()
    }

    fn frames() -> impl Iterator<Item = f32> {
        let count = (TWIRL_LENGTH * 60.0).round() as usize;
        (0..=count).map(move |frame| TWIRL_LENGTH * frame as f32 / count as f32)
    }

    /// A retuned pistol would otherwise spin and be blown across at the old points.
    #[test]
    fn the_generator_measures_the_pistol_its_file_describes() {
        let pistol = pistol();
        assert_eq!(GRIP, Vec3::from_array(pistol.hold.grip), "the pistol's grip moved; update `GRIP`");
        assert_eq!(Some(MUZZLE), pistol.muzzle.map(|muzzle| muzzle.point()), "the pistol's muzzle moved; update `MUZZLE`");
        assert_eq!(GUARD, guard_of(&pistol), "the pistol's guard moved; update `GUARD`");
    }

    /// The blends in and out start and end at each build's hold.
    #[test]
    fn it_starts_and_ends_on_the_hold() {
        for stage in Stage::both() {
            for class in every_humanoid() {
                let held = stage.posed(class, None).weapon;
                for at in [0.0, TWIRL_LENGTH] {
                    let posed = stage.posed(class, Some(at)).weapon;
                    assert!(posed.translation.distance(held.translation) < 0.001, "{class:?} at {at}: {posed:?} is not the hold {held:?}");
                    assert!(posed.rotation.angle_between(held.rotation) < 0.01, "{class:?} at {at}: turned off the hold");
                }
            }
        }
    }

    /// While the grip is off, the guard moves no more than the hand's pump.
    #[test]
    fn the_pistol_spins_about_the_trigger_finger() {
        for stage in Stage::both() {
            for class in every_humanoid() {
                let mut first = None;
                for at in frames().filter(|at| stage.holding(*at) <= 0.001) {
                    let posed = stage.posed(class, Some(at));
                    let guard = posed.weapon.transform_point(stage.guard) - posed.shoulders();
                    let first = *first.get_or_insert(guard);
                    assert!(guard.distance(first) < 0.03, "{class:?} at {at:.2}: the guard is {guard}, off the finger at {first}");
                }
                assert!(first.is_some(), "the grip never lets go, so nothing spins");
            }
        }
    }

    /// A hand short of the grip leaves the pistol floating beside it.
    #[test]
    fn every_build_keeps_its_hand_on_the_grip_while_the_hold_has_it() {
        for stage in Stage::both() {
            for class in every_humanoid() {
                for at in frames().filter(|at| stage.holding(*at) >= 0.999) {
                    let posed = stage.posed(class, Some(at));
                    let grip = posed.weapon.transform_point(Vec3::from_array(stage.hold.for_class(Some(class)).grip));
                    let gap = posed.bone(bone::HAND_R).head.distance(grip);
                    assert!(gap < 0.005, "{class:?} at {at:.2}: the wrist is {gap:.3} m off the grip");
                }
            }
        }
    }

    /// A jump between frames is a grip or hip out of reach.
    #[test]
    fn neither_hand_jumps() {
        for stage in Stage::both() {
            for class in every_humanoid() {
                let mut was: Option<[Vec3; 2]> = None;
                for at in frames() {
                    let posed = stage.posed(class, Some(at));
                    let hands = [bone::HAND_R, bone::HAND_L].map(|hand| posed.bone(hand).head - posed.shoulders());
                    if let Some(was) = was {
                        for (hand, (now, then)) in hands.iter().zip(was).enumerate() {
                            let speed = now.distance(then) * 60.0;
                            assert!(speed < 4.0, "{class:?} at {at:.2}: hand {hand} moved at {speed:.1} m/s");
                        }
                    }
                    was = Some(hands);
                }
            }
        }
    }

    #[test]
    fn the_muzzle_comes_up_before_the_lips() {
        for stage in Stage::both() {
            for class in every_humanoid() {
                for at in frames().filter(|at| (2.4..3.35).contains(at)) {
                    let posed = stage.posed(class, Some(at));
                    let barrel = posed.weapon.rotation * Vec3::NEG_Z;
                    assert!(barrel.angle_between(Vec3::Y) < 0.3, "{class:?} at {at:.2}: the barrel points {barrel}");
                    let muzzle = posed.weapon.transform_point(stage.muzzle);
                    let face = posed.to_face(muzzle);
                    assert!((0.035..0.15).contains(&face), "{class:?} at {at:.2}: the muzzle is {face:.3} m from the face");
                    let head = posed.bone(bone::HEAD);
                    assert!(muzzle.z < head.head.z, "{class:?} at {at:.2}: the muzzle is not in front");
                    let above = muzzle.y - head.head.y;
                    assert!((-0.04..0.08).contains(&above), "{class:?} at {at:.2}: the muzzle is {above:.3} m above the jaw");
                }
            }
        }
    }

    #[test]
    fn the_support_hand_reaches_where_the_clip_puts_it() {
        for stage in Stage::both() {
            for class in every_humanoid() {
                for at in frames().filter(|at| (0.6..3.4).contains(at)) {
                    let posed = stage.posed(class, Some(at));
                    let wrist = posed.bone(bone::HAND_L).head - posed.shoulders();
                    let target = stage.support_target(at);
                    assert!(wrist.distance(target) < 0.01, "{class:?} at {at:.2}: the support hand is at {wrist}, short of {target}");
                }
            }
        }
    }
}
