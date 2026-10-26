//! A body jolting when it is hurt: drawing, from the `DamageDealt` records
//! every machine already has (local on the authority, relayed elsewhere). See
//! `documentation/animation-layers.md`.
//!
//! The flinch is on [`Layer::Flinch`], which claims nothing: its tracks
//! compose onto whatever won, so it shows over a run, a taunt or humiliation
//! alike and hides none of them. Which way the body is pushed picks one of
//! four clips, in the body's own frame.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::assets::{default_packs, Assets as PackAssets};
use crate::common::damage::{DamageDealt, DamageSource, PlayerId};
use crate::common::skeleton::clip::Clip;
use crate::common::skeleton::play::{ClipStack, Layer};

/// Which way a hit pushes a body, in the body's own frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Push {
    /// Away from its front: shot from in front.
    Back,
    Forward,
    /// Towards its own left (`-X`).
    Left,
    Right,
}

impl Push {
    const ALL: [Push; 4] = [Push::Back, Push::Forward, Push::Left, Push::Right];

    fn clip_name(self) -> &'static str {
        match self {
            Push::Back => "flinch_back",
            Push::Forward => "flinch_forward",
            Push::Left => "flinch_left",
            Push::Right => "flinch_right",
        }
    }

    /// From the direction the hit travelled, in the body's frame (`-Z` is
    /// where it faces). Along its strongest axis; no direction at all is
    /// taken as a hit from in front.
    pub fn of(travel: Vec3) -> Push {
        let flat = Vec2::new(travel.x, travel.z);
        if flat.length_squared() < 1e-8 {
            return Push::Back;
        }
        match flat.x.abs() > flat.y.abs() {
            true if flat.x > 0.0 => Push::Right,
            true => Push::Left,
            false if flat.y > 0.0 => Push::Back,
            false => Push::Forward,
        }
    }
}

/// The four clips, from the packs. A missing one is a flinch that direction
/// does not have.
#[derive(Resource, Default)]
pub struct FlinchClips([Option<Handle<Clip>>; 4]);

impl FlinchClips {
    fn of(&self, push: Push) -> Option<&Handle<Clip>> {
        self.0[Push::ALL.iter().position(|each| *each == push).unwrap_or(0)].as_ref()
    }
}

pub struct FlinchPlugin;

impl Plugin for FlinchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FlinchClips>()
            .add_systems(Startup, load_flinch_clips)
            .add_systems(
                Update,
                (start_flinches, end_flinches)
                    .chain()
                    .before(crate::game::held::fade_the_layers)
                    .run_if(in_state(AppMode::Play)),
            );
    }
}

fn load_flinch_clips(packs: Res<PackAssets>, mut clips: ResMut<Assets<Clip>>, mut loaded: ResMut<FlinchClips>) {
    for (index, push) in Push::ALL.into_iter().enumerate() {
        loaded.0[index] = crate::common::clip_file::load_named_clip(&packs, &default_packs(), push.clip_name())
            .map(|clip| clips.add(clip));
    }
}

fn is_a_flinch(layer: Layer) -> bool {
    layer == Layer::Flinch
}

/// Every hit starts its body's flinch again from the top, in the direction
/// the hit travelled: from the attacker's body where there is one to look at,
/// otherwise away from where it landed (your own rocket, the map, an attacker
/// already dead).
fn start_flinches(
    mut hits: MessageReader<DamageDealt>,
    clips: Res<FlinchClips>,
    attackers: Query<(Entity, &PlayerId, &GlobalTransform)>,
    mut bodies: Query<(&GlobalTransform, &mut ClipStack)>,
) {
    for hit in hits.read() {
        let Ok((placed, mut stack)) = bodies.get_mut(hit.target) else { continue };
        let at = placed.translation();
        let from = match hit.source {
            DamageSource::Player(id) => attackers
                .iter()
                .find(|(entity, attacker, _)| **attacker == id && *entity != hit.target)
                .map(|(_, _, attacker)| attacker.translation()),
            DamageSource::World => None,
        }
        .unwrap_or(hit.point);
        let travel = placed.rotation().inverse() * (at - from);

        let Some(clip) = clips.of(Push::of(travel)) else { continue };
        if let Some(playing) = stack.keep(is_a_flinch, Some((Layer::Flinch, clip))) {
            playing.time = 0.0;
        }
    }
}

/// A flinch is a one-shot: ticked here, and sent out when its clip ends.
fn end_flinches(time: Res<Time>, clips: Res<Assets<Clip>>, mut stacks: Query<&mut ClipStack>) {
    for mut stack in &mut stacks {
        if !stack.has_staying(is_a_flinch) {
            continue;
        }
        let mut over = false;
        for playing in stack.0.iter_mut().filter(|playing| is_a_flinch(playing.layer) && !playing.leaving) {
            playing.time += time.delta_secs();
            over |= clips.get(&playing.clip).is_none_or(|clip| playing.time > clip.length);
        }
        if over {
            stack.keep(is_a_flinch, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::common::skeleton::play::{apply, Frames};
    use crate::common::skeleton::rig::bone;
    use crate::common::skeleton::{humanoid, Pose};

    /// A hit's direction of travel, in the body's frame, picks the push.
    #[test]
    fn a_hit_pushes_the_way_it_travelled() {
        assert_eq!(Push::of(Vec3::Z), Push::Back, "shot from in front");
        assert_eq!(Push::of(Vec3::NEG_Z), Push::Forward, "shot from behind");
        assert_eq!(Push::of(Vec3::X), Push::Right);
        assert_eq!(Push::of(Vec3::NEG_X), Push::Left);
        assert_eq!(Push::of(Vec3::Y * 3.0), Push::Back, "no direction is taken as from in front");
    }

    /// Each shipped flinch moves the head the way its name says, and lets go
    /// by the end. Measured, since a sign on a roll is easy to get backwards.
    #[test]
    fn every_flinch_moves_the_head_the_way_it_is_named() {
        let rig = humanoid(crate::common::class::ClassBuilds::shipped().of(crate::common::class::Class::Soldier));
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let head = |pose: &Pose| {
            rig.posed_bones(pose, &Transform::IDENTITY)[rig.index_of(bone::HEAD).unwrap()].tail
        };
        let rest = head(&Pose::rest());
        let packs = [std::path::PathBuf::from("assets/default")];

        for (push, wanted) in [
            (Push::Back, Vec3::Z),
            (Push::Forward, Vec3::NEG_Z),
            (Push::Left, Vec3::NEG_X),
            (Push::Right, Vec3::X),
        ] {
            let clip = crate::common::clip_file::load_named_clip(&Default::default(), &packs, push.clip_name())
                .unwrap_or_else(|| panic!("no {}", push.clip_name()));
            let at = |time| {
                let mut pose = Pose::rest();
                apply(&clip, time, &rig, &mut pose, &frames, None);
                head(&pose)
            };
            let moved = (at(0.06) - rest).with_y(0.0);
            assert!(moved.dot(wanted) > 0.05, "{push:?} moved the head {moved}");
            assert!((at(clip.length) - rest).length() < 1e-4, "{push:?} did not let go");
        }
    }

    /// A hit starts its target's flinch, a second hit starts it again from
    /// the top, and it is sent out once its clip has run.
    #[test]
    fn a_hit_starts_a_flinch_and_it_runs_out() {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<Messages<DamageDealt>>();
        let mut assets = Assets::<Clip>::default();
        let clip = assets.add(Clip { length: 0.4, ..default() });
        world.insert_resource(assets);
        world.insert_resource(FlinchClips([Some(clip.clone()), Some(clip.clone()), Some(clip.clone()), Some(clip)]));
        let body = world.spawn((GlobalTransform::IDENTITY, ClipStack::default())).id();
        let hit = |world: &mut World| {
            world.write_message(DamageDealt {
                target: body,
                source: DamageSource::World,
                amount: 10,
                remaining: 90,
                point: Vec3::new(0.0, 1.0, -1.0),
            });
            world.run_system_once(start_flinches).unwrap();
        };
        let flinch = |world: &World| {
            world.get::<ClipStack>(body).unwrap().0.iter().find(|playing| playing.layer == Layer::Flinch).cloned()
        };
        let pass = |world: &mut World, seconds: f32| {
            world.resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(seconds));
            world.run_system_once(end_flinches).unwrap();
        };

        hit(&mut world);
        assert!(flinch(&world).is_some_and(|playing| !playing.leaving), "the hit started no flinch");
        pass(&mut world, 0.3);
        hit(&mut world);
        assert_eq!(flinch(&world).unwrap().time, 0.0, "a second hit did not start it again");
        pass(&mut world, 0.5);
        assert!(flinch(&world).unwrap().leaving, "the flinch outlived its clip");
    }

    /// A body nobody steers — an animation grid's — flinches too: it has an
    /// animator, so a stack, and the stack is played without a `Player`.
    #[test]
    fn a_grid_body_flinches() {
        use crate::common::skeleton::SkeletonAnimator;

        let mut world = World::new();
        world.init_resource::<Messages<DamageDealt>>();
        let packs = [std::path::PathBuf::from("assets/default")];
        let mut assets = Assets::<Clip>::default();
        let back = assets.add(
            crate::common::clip_file::load_named_clip(&Default::default(), &packs, "flinch_back").unwrap(),
        );
        world.insert_resource(assets);
        world.insert_resource(FlinchClips([Some(back), None, None, None]));
        let body = world
            .spawn((
                humanoid(crate::common::class::ClassBuilds::shipped().of(crate::common::class::Class::Heavy)),
                Pose::rest(),
                SkeletonAnimator::default(),
                GlobalTransform::IDENTITY,
            ))
            .id();
        assert!(world.get::<ClipStack>(body).is_some(), "an animated body came without a stack");

        world.write_message(DamageDealt {
            target: body,
            source: DamageSource::World,
            amount: 10,
            remaining: 90,
            point: Vec3::new(0.0, 1.0, -1.0),
        });
        world.run_system_once(start_flinches).unwrap();
        world.get_mut::<ClipStack>(body).unwrap().0[0].weight = 1.0;
        world.get_mut::<ClipStack>(body).unwrap().0[0].time = 0.06;
        world.run_system_once(crate::game::held::play_the_clips).unwrap();

        let chest = world.get::<Pose>(body).unwrap().joint(bone::CHEST);
        assert!(chest.angle_between(Quat::IDENTITY) > 0.1, "the grid body did not flinch");
    }
}

