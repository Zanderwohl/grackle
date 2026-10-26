//! Turning a [`Clip`] into motion.
//!
//! - [`sample_at`] is pure and stateless (no cursor), so a backwards scrub, a
//!   body in a match and a rollback replay all make the same call.
//! - [`apply`] turns a sample into a pose. It is handed the clip's spaces in
//!   [`Frames`] rather than looking them up, so a clip samples in a unit test
//!   with no `World`.
//!
//! Every sample returns an `Option`, and `None` (or [`Axis::Free`] on one axis)
//! leaves the layer beneath alone, as absence does in a [`Pose`].

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::clip::{Axis, ChainRef, Channel, Clip, Combine, Key, Space, Unit};
use super::ik::{self, Chain, Reach};
use super::interpolation::Interpolation;
use super::rig::{Pose, Skeleton};
use crate::common::rotation::quat_from_euler;

/// The value a keyed channel holds at `time`, or `None` with no keys. `time`
/// must already be through [`Clip::wrap`].
///
/// Outside the keys a one-shot holds its end values, so a finished reload does
/// not snap back to the hold. A loop blends last to first across the seam,
/// over `(length - last.at) + first.at`. The outgoing key's curve governs each
/// segment, the seam's included.
pub fn sample_at<K, T: Copy>(
    keys: &[K],
    time: f32,
    length: f32,
    looping: bool,
    at: impl Fn(&K) -> f32,
    value: impl Fn(&K) -> T,
    ease: impl Fn(&K) -> Interpolation,
    blend: impl Fn(T, T, Interpolation, f32) -> T,
) -> Option<T> {
    let first = keys.first()?;
    let last = keys.last()?;

    if keys.len() == 1 {
        return Some(value(first));
    }

    // The wrap-around segment's length; only meaningful while looping.
    let seam = (length - at(last)) + at(first);

    if time <= at(first) {
        if !looping {
            return Some(value(first));
        }
        // The tail of the seam, past `length` and back round to zero.
        if seam <= 0.0 {
            return Some(value(first));
        }
        let along = (time + length - at(last)) / seam;
        return Some(blend(value(last), value(first), ease(last), along));
    }

    if time >= at(last) {
        if !looping || seam <= 0.0 {
            return Some(value(last));
        }
        let along = (time - at(last)) / seam;
        return Some(blend(value(last), value(first), ease(last), along));
    }

    // Linear scan: a track has tens of keys.
    for pair in keys.windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        if time < at(from) || time > at(to) {
            continue;
        }
        let span = at(to) - at(from);
        // Two keys at one time is a legal snap; do not divide by zero.
        let along = if span > 0.0 { (time - at(from)) / span } else { 0.0 };
        return Some(blend(value(from), value(to), ease(from), along));
    }

    Some(value(last))
}

/// Sample a track, each axis eased independently, so a key like
/// `[0.2, "ground", "free"]` mixes kinds.
pub fn sample_key(keys: &[Key], time: f32, length: f32, looping: bool) -> Option<[Axis; 3]> {
    sample_at(
        keys,
        time,
        length,
        looping,
        |key| key.at,
        |key| key.to,
        |key| key.ease,
        |from, to, ease, along| {
            let mut blended = [Axis::Free; 3];
            for axis in 0..3 {
                blended[axis] = match (from[axis], to[axis]) {
                    (Axis::Value(a), Axis::Value(b)) => Axis::Value(ease.f32(a, b, along)),
                    // Stays relative to the floor throughout a lift.
                    (Axis::Ground { clearance: a }, Axis::Ground { clearance: b }) => {
                        Axis::Ground { clearance: ease.f32(a, b, along) }
                    }
                    // A change of kind has no halfway; switch on arrival.
                    (a, b) => {
                        if ease.ease(along) >= 1.0 {
                            b
                        } else {
                            a
                        }
                    }
                };
            }
            blended
        },
    )
}

/// Euler angles for a `Replace` key, with `Free` axes read back from
/// `beneath`. Decomposition is in [`EULER_ORDER`]; near gimbal lock it is not
/// unique, so a key that needs certainty should set all three.
///
/// [`EULER_ORDER`]: crate::common::rotation::EULER_ORDER
fn angles_for(to: [Axis; 3], beneath: Quat) -> Vec3 {
    let already = crate::common::rotation::euler_from_quat(beneath);
    let mut wanted = already;
    for axis in 0..3 {
        wanted[axis] = match to[axis] {
            Axis::Free => already[axis],
            Axis::Value(at) => at,
            // No floor under an angle; `Clip::problems` says so.
            Axis::Ground { .. } => already[axis],
        };
    }
    wanted
}

/// Euler angles for a `Compose` key. `Free` is zero here: reading the angle
/// beneath and multiplying it on would double it.
fn angles_onto(to: [Axis; 3]) -> Vec3 {
    let mut wanted = Vec3::ZERO;
    for axis in 0..3 {
        if let Axis::Value(at) = to[axis] {
            wanted[axis] = at;
        }
    }
    wanted
}

/// Insert a key at `at` holding the value the track already has there, so
/// keying never moves the pose; an empty track gets three free axes. Returns
/// the new key's index.
pub fn key_the_channel(channel: &mut Channel, at: f32, length: f32, looping: bool) -> usize {
    let at = at.clamp(0.0, length);
    let keys = channel.keys_mut();

    let to = sample_key(keys, at, length, looping).unwrap_or([Axis::Free; 3]);
    let ease = keys.last().map(|key| key.ease).unwrap_or_default();
    keys.push(Key { at, to, ease });
    keys.sort_by(|a, b| a.at.total_cmp(&b.at));
    keys.iter().position(|key| key.at == at).unwrap_or(0)
}

/// How many metres one of `unit` is on this body.
pub fn metres_per(unit: Unit, skeleton: &Skeleton) -> f32 {
    let proportions = skeleton.proportions();
    match unit {
        Unit::Metres => 1.0,
        Unit::LegLengths => ik::leg_length(skeleton),
        Unit::ArmLengths => proportions.arm_metres(),
        Unit::HipHeights => proportions.hip_metres(),
    }
}

/// What the spaces a clip names are, this frame, for this body. Filled by the
/// caller, since `prop::hold` and `game::collision` sit above this module.
pub struct Frames<'a> {
    /// The `root` [`ik::solve`] takes: feet on the floor, facing forward.
    pub body: Transform,
    /// The point between the shoulders, turned by the aim.
    pub carry: Transform,
    /// `None` for an empty hand; tracks in this space are then skipped.
    pub weapon: Option<Transform>,
    /// Eye position and look direction, for a viewmodel clip.
    pub eye: Option<Transform>,
    /// Floor height under a point. A closure so tests and the prop editor can
    /// answer with a plane and the game with a ray.
    pub ground: &'a dyn Fn(Vec3) -> f32,
    /// Each foot's phase in its own cycle, left first; see
    /// [`Timing`](super::clip::Timing). `None` puts the right foot half a
    /// stride after the left.
    pub footfalls: Option<[f32; 2]>,
}

impl Frames<'_> {
    /// Nothing in hand, no eye, no gait.
    pub fn standing(body: Transform, carry: Transform, ground: &dyn Fn(Vec3) -> f32) -> Frames<'_> {
        Frames {
            body,
            carry,
            weapon: None,
            eye: None,
            ground,
            footfalls: None,
        }
    }

    fn of(&self, space: Space) -> Option<Transform> {
        match space {
            Space::Body => Some(self.body),
            Space::Carry => Some(self.carry),
            Space::Shoulders => Some(Transform::from_translation(self.carry.translation)
                .with_rotation(self.body.rotation)),
            Space::Weapon => self.weapon,
            Space::Eye => self.eye,
        }
    }
}

/// Which part of the ranking a clip plays at. Later variants outrank earlier
/// ones wherever both claim a region; see `documentation/animation-layers.md`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// A body clip: what the animator plays, or the editor previews.
    #[default]
    Locomotion,
    /// A weapon's own clips. The hold sits here too, as the arms' default.
    Weapon,
    Humiliation,
}

impl Layer {
    /// What a clip on this layer claims unless it says otherwise.
    pub const fn claims(self) -> Regions {
        match self {
            Layer::Locomotion => Regions::ALL,
            Layer::Weapon => Regions::ARMS.union(Regions::WEAPON),
            Layer::Humiliation => Regions::ARMS
                .union(Regions::TORSO)
                .union(Regions::HEAD)
                .union(Regions::WEAPON),
        }
    }

    /// How a layer is written in `layers.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Layer::Locomotion => "locomotion",
            Layer::Weapon => "weapon",
            Layer::Humiliation => "humiliation",
        }
    }

    pub fn named(key: &str) -> Option<Layer> {
        Layer::ALL.into_iter().find(|layer| layer.key() == key)
    }

    pub const ALL: [Layer; 3] = [Layer::Locomotion, Layer::Weapon, Layer::Humiliation];
}

/// Seconds a layer takes to fade in over what is beneath it, and back out.
/// Zero snaps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Blend {
    #[serde(default, with = "crate::prop::nice_f32::scalar")]
    pub blend_in: f32,
    #[serde(default, with = "crate::prop::nice_f32::scalar")]
    pub blend_out: f32,
}

/// Each layer's [`Blend`], from the packs' `layers.toml`. A layer the file
/// does not name snaps.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct LayerBlends(pub std::collections::BTreeMap<Layer, Blend>);

impl LayerBlends {
    /// `clip`'s own numbers where it has them, its layer's otherwise.
    pub fn of(&self, layer: Layer, clip: Option<&Clip>) -> Blend {
        let layer = self.0.get(&layer).copied().unwrap_or_default();
        Blend {
            blend_in: clip.and_then(|clip| clip.blend_in).unwrap_or(layer.blend_in),
            blend_out: clip.and_then(|clip| clip.blend_out).unwrap_or(layer.blend_out),
        }
    }
}

/// Parts of a body a layer can claim. `WEAPON` is the weapon's placement, not
/// a bone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Regions(u8);

impl Regions {
    pub const NONE: Regions = Regions(0);
    /// The pelvis, the legs and the root offset.
    pub const LEGS: Regions = Regions(1);
    pub const TORSO: Regions = Regions(1 << 1);
    /// Neck and head.
    pub const HEAD: Regions = Regions(1 << 2);
    pub const ARM_L: Regions = Regions(1 << 3);
    pub const ARM_R: Regions = Regions(1 << 4);
    pub const WEAPON: Regions = Regions(1 << 5);
    pub const ARMS: Regions = Regions::ARM_L.union(Regions::ARM_R);
    pub const ALL: Regions = Regions(0b11_1111);

    pub const fn union(self, other: Regions) -> Regions {
        Regions(self.0 | other.0)
    }

    pub fn overlaps(self, other: Regions) -> bool {
        self.0 & other.0 != 0
    }

    /// `NONE` for a name the rig does not have, so it is never masked.
    pub fn of_bone(name: &str) -> Regions {
        use super::rig::bone;
        match name {
            bone::PELVIS => Regions::LEGS,
            bone::CHEST => Regions::TORSO,
            bone::NECK | bone::HEAD => Regions::HEAD,
            bone::CLAVICLE_L | bone::UPPER_ARM_L | bone::FOREARM_L | bone::HAND_L => Regions::ARM_L,
            bone::CLAVICLE_R | bone::UPPER_ARM_R | bone::FOREARM_R | bone::HAND_R => Regions::ARM_R,
            bone::THIGH_L | bone::SHIN_L | bone::FOOT_L | bone::THIGH_R | bone::SHIN_R | bone::FOOT_R => {
                Regions::LEGS
            }
            _ => Regions::NONE,
        }
    }

    /// What a track moves.
    pub fn of_track(channel: &Channel) -> Regions {
        match channel {
            Channel::Rotation { bone, .. } => Regions::of_bone(bone),
            Channel::Effector { chain, .. } => match chain {
                ChainRef::LeftArm => Regions::ARM_L,
                ChainRef::RightArm => Regions::ARM_R,
                ChainRef::LeftLeg | ChainRef::RightLeg => Regions::LEGS,
            },
            Channel::Weapon { .. } | Channel::WeaponRotation { .. } => Regions::WEAPON,
            Channel::Root { .. } => Regions::LEGS,
        }
    }
}

/// One clip playing on one body.
///
/// A handle, not a copy: an edit is re-inserted under it, and a handle whose
/// clip is missing plays nothing.
#[derive(Clone, Debug)]
pub struct Playing {
    pub clip: Handle<Clip>,
    /// On the clip's own clock.
    pub time: f32,
    /// `0..=1`.
    pub weight: f32,
    pub layer: Layer,
    /// Tracks outside these still play, unless a higher layer claims them.
    pub claims: Regions,
    /// Whoever started it is done with it: [`ClipStack::fade`] takes it down
    /// and then off. Its owner looks past it, and may take it back.
    pub leaving: bool,
}

impl Playing {
    /// At full weight, claiming what its layer does.
    pub fn new(layer: Layer, clip: Handle<Clip>, time: f32) -> Playing {
        Playing { clip, time, weight: 1.0, layer, claims: layer.claims(), leaving: false }
    }

    /// At no weight, for [`ClipStack::fade`] to bring in.
    pub fn entering(layer: Layer, clip: Handle<Clip>, time: f32) -> Playing {
        Playing { weight: 0.0, ..Playing::new(layer, clip, time) }
    }
}

/// Everything playing on this body, in any order: [`apply_stack`] ranks it.
#[derive(Component, Clone, Debug, Default)]
pub struct ClipStack(pub Vec<Playing>);

impl ClipStack {
    /// The highest layer audibly claiming any of `region`. `None` leaves it to
    /// whatever ran before the stack: the animator and the hold.
    pub fn owner(&self, region: Regions) -> Option<Layer> {
        self.0
            .iter()
            .filter(|playing| playing.weight > 0.0 && playing.claims.overlaps(region))
            .map(|playing| playing.layer)
            .max()
    }

    /// How much of `region` layers above `above` show, `0..=1`: the most any
    /// one of them does.
    pub fn cover(&self, region: Regions, above: Layer) -> f32 {
        self.0
            .iter()
            .filter(|playing| playing.layer > above && playing.claims.overlaps(region))
            .map(|playing| playing.weight)
            .fold(0.0, f32::max)
    }

    /// Bring entering clips up to full weight and leaving ones down and off,
    /// at their [`Blend`]s. A leaving clip's time is ticked here, since its
    /// owner has stopped ticking it.
    pub fn fade(&mut self, delta: f32, clips: &Assets<Clip>, blends: &LayerBlends) {
        for playing in &mut self.0 {
            let blend = blends.of(playing.layer, clips.get(&playing.clip));
            let step = |seconds: f32| if seconds > 0.0 { delta / seconds } else { 1.0 };
            if playing.leaving {
                playing.weight = (playing.weight - step(blend.blend_out)).max(0.0);
                playing.time += delta;
            } else if playing.weight < 1.0 {
                playing.weight = (playing.weight + step(blend.blend_in)).min(1.0);
            }
        }
        self.0.retain(|playing| !playing.leaving || playing.weight > 0.0);
    }

    /// Make `wanted` the one clip of `ours` that is staying: taken back if it
    /// was on its way out, brought in if it is not here, and every other clip
    /// of `ours` sent out. `None` sends them all. Returns the one staying, for
    /// its owner to tick.
    pub fn keep(
        &mut self,
        ours: impl Fn(Layer) -> bool,
        wanted: Option<(Layer, &Handle<Clip>)>,
    ) -> Option<&mut Playing> {
        let is_wanted = |playing: &Playing| {
            wanted.is_some_and(|(layer, clip)| playing.layer == layer && playing.clip == *clip)
        };
        let mut staying = None;
        for (index, playing) in self.0.iter_mut().enumerate() {
            if !ours(playing.layer) {
                continue;
            }
            if staying.is_none() && is_wanted(playing) {
                playing.leaving = false;
                staying = Some(index);
            } else {
                playing.leaving = true;
            }
        }
        let index = match (staying, wanted) {
            (Some(index), _) => index,
            (None, Some((layer, clip))) => {
                self.0.push(Playing::entering(layer, clip.clone(), 0.0));
                self.0.len() - 1
            }
            (None, None) => return None,
        };
        Some(&mut self.0[index])
    }

    /// Whether any clip of `ours` is playing and not on its way out.
    pub fn has_staying(&self, ours: impl Fn(Layer) -> bool) -> bool {
        self.0.iter().any(|playing| ours(playing.layer) && !playing.leaving)
    }

    /// Whether [`ClipStack::fade`] has anything to do.
    pub fn is_fading(&self) -> bool {
        self.0.iter().any(|playing| playing.leaving || playing.weight < 1.0)
    }

    /// Everything fully covered by layers above `layer`. A layer still blending
    /// in masks nothing, so it fades in over what is beneath it.
    fn claimed_above(&self, layer: Layer) -> Regions {
        self.0
            .iter()
            .filter(|playing| playing.weight >= 1.0 && playing.layer > layer)
            .fold(Regions::NONE, |claimed, playing| claimed.union(playing.claims))
    }
}

/// Registers `Clip` as an asset, whoever asks first.
///
/// Every plugin that plays a clip adds this, so no binary or test has to
/// remember to. Safe to add more than once — a second `init_asset` would
/// replace the storage and drop every clip in it, so it does nothing if the
/// storage is already there.
///
/// It needs `AssetPlugin` first, and says so rather than quietly making do: a
/// bare `Assets<Clip>` with no asset systems behind it sends no `AssetEvent`s
/// and never frees a clip whose last handle has gone, and nothing about that
/// would be loud. A headless test adds `AssetPlugin` like the game does.
pub struct ClipAssetsPlugin;

impl Plugin for ClipAssetsPlugin {
    fn build(&self, app: &mut App) {
        if app.world().contains_resource::<Assets<Clip>>() {
            return;
        }
        assert!(
            app.world().contains_resource::<AssetServer>(),
            "ClipAssetsPlugin needs AssetPlugin added before it",
        );
        app.init_asset::<Clip>();
    }

    fn is_unique(&self) -> bool {
        false
    }
}

/// Put `clip` under `handle`, unless that is already what it holds.
///
/// How a clip that was edited or saved reaches whatever is playing it: the
/// handle stays, the asset behind it changes. Compared first, so refilling
/// with the same clip is not a `Modified` event for nothing.
pub fn refill(assets: &mut Assets<Clip>, handle: &Handle<Clip>, clip: &Clip) {
    if assets.get(handle) != Some(clip) {
        // The caller holds `handle` strongly, so its id cannot have been
        // freed and the insert cannot fail.
        let _ = assets.insert(handle.id(), clip.clone());
    }
}

/// Where the end of a chain currently is, in world space.
fn chain_tip(skeleton: &Skeleton, pose: &Pose, root: &Transform, chain: &Chain) -> Option<Vec3> {
    // `solve` puts the lower bone's tail on its target.
    let lower = skeleton.index_of(chain.lower)?;
    Some(skeleton.posed_bone(pose, root, lower).tail)
}

/// Resolve a sampled target to a world point. `tip` is read back into the
/// track's space and unit so `Free` axes keep it.
fn resolve_point(
    to: [Axis; 3],
    tip: Vec3,
    frame: &Transform,
    scale: f32,
    combine: Combine,
    ground: &dyn Fn(Vec3) -> f32,
) -> Vec3 {
    let inverse = frame.to_matrix().inverse();
    let local = inverse.transform_point3(tip) / scale;

    let mut wanted = local;
    for axis in 0..3 {
        wanted[axis] = match to[axis] {
            Axis::Free => local[axis],
            Axis::Value(at) => match combine {
                Combine::Replace => at,
                Combine::Compose => local[axis] + at,
            },
            // Set in world space below; a height through a turned frame is not
            // a height.
            Axis::Ground { .. } => local[axis],
        };
    }

    let mut target = frame.transform_point(wanted * scale);
    if let Axis::Ground { clearance } = to[1] {
        target.y = ground(target) + clearance * scale;
    }
    target
}

/// A track's time given its clip's. A foot-timed track keeps the whole strides
/// and swaps the fraction for its foot's phase.
pub fn track_time(timing: super::clip::Timing, time: f32, footfalls: Option<[f32; 2]>) -> f32 {
    use super::clip::Timing;
    let foot = match timing {
        Timing::Clip => return time,
        Timing::LeftFoot => 0,
        Timing::RightFoot => 1,
    };
    let fraction = match footfalls {
        Some(feet) => feet[foot],
        None => (time + 0.5 * foot as f32).rem_euclid(1.0),
    };
    time.floor() + fraction
}

/// Apply `clip` at `time` to `pose`, in place. `weapon` is moved by weapon
/// tracks; the arms are solved onto it later by `grip_the_weapon`, so no hand
/// is keyed. Returns each effector track's [`Reach`], in track order.
pub fn apply(
    clip: &Clip,
    time: f32,
    skeleton: &Skeleton,
    pose: &mut Pose,
    frames: &Frames,
    weapon: Option<&mut Transform>,
) -> Vec<Reach> {
    apply_masked(clip, time, skeleton, pose, frames, weapon, Regions::NONE)
}

/// [`apply`], skipping every track in `masked`: regions a higher layer owns.
pub fn apply_masked(
    clip: &Clip,
    time: f32,
    skeleton: &Skeleton,
    pose: &mut Pose,
    frames: &Frames,
    mut weapon: Option<&mut Transform>,
    masked: Regions,
) -> Vec<Reach> {
    let time = clip.wrap(time);
    let length = clip.length;
    let looping = clip.looping;
    let mut reaches = Vec::new();

    for track in &clip.tracks {
        if !track.enabled || masked.overlaps(Regions::of_track(&track.channel)) {
            continue;
        }
        let time = track_time(track.timing, time, frames.footfalls);

        match &track.channel {
            Channel::Rotation { bone, keys } => {
                let Some(to) = sample_key(keys, time, length, looping) else {
                    continue;
                };
                // `Pose::set` needs the rig's `&'static str`. An unknown bone is
                // dropped here; `Clip::problems` reports it.
                let Some(index) = skeleton.index_of(bone) else {
                    continue;
                };
                let name = skeleton.bones()[index].name;
                let beneath = pose.joint(name);
                match track.combine {
                    Combine::Replace => {
                        pose.set(name, quat_from_euler(angles_for(to, beneath)))
                    }
                    Combine::Compose => {
                        pose.set(name, beneath * quat_from_euler(angles_onto(to)))
                    }
                }
            }

            Channel::Effector { chain, pole, keys } => {
                let Some(to) = sample_key(keys, time, length, looping) else {
                    continue;
                };
                // Skip rather than re-solve onto the current tip: `solve`
                // re-derives roll, so that is not the identity.
                if to.iter().all(|axis| matches!(axis, Axis::Free)) {
                    continue;
                }
                let Some(frame) = frames.of(track.space) else {
                    continue;
                };
                let solved = chain.chain();
                let Some(tip) = chain_tip(skeleton, pose, &frames.body, &solved) else {
                    continue;
                };

                let target = resolve_point(
                    to,
                    tip,
                    &frame,
                    metres_per(track.unit, skeleton),
                    track.combine,
                    frames.ground,
                );
                // Authored and default poles alike are in the body's frame.
                let pole = frames.body.rotation
                    * pole.map(Vec3::from_array).unwrap_or_else(|| default_pole(*chain));

                // A short leg straightens and points; a short arm is left as
                // the animation had it. See `ik::reach_or_leave`.
                reaches.push(if chain.is_leg() {
                    ik::solve(skeleton, pose, &frames.body, &solved, target, pole)
                } else {
                    ik::reach_or_leave(skeleton, pose, &frames.body, &solved, target, pole)
                });
            }

            Channel::Root { keys } => {
                let Some(to) = sample_key(keys, time, length, looping) else {
                    continue;
                };
                // Always hip-heights in the body frame; the track's space and
                // unit are ignored.
                for axis in 0..3 {
                    let was = pose.root_offset[axis];
                    pose.root_offset[axis] = match (to[axis], track.combine) {
                        (Axis::Free, _) => was,
                        (Axis::Value(at), Combine::Replace) => at,
                        (Axis::Value(at), Combine::Compose) => was + at,
                        // Meaningless here; `Clip::problems` says so.
                        (Axis::Ground { .. }, _) => was,
                    };
                }
            }

            Channel::Weapon { keys } => {
                let Some(placed) = weapon.as_deref_mut() else {
                    continue;
                };
                let Some(to) = sample_key(keys, time, length, looping) else {
                    continue;
                };
                let Some(frame) = frames.of(track.space) else {
                    continue;
                };
                placed.translation = resolve_point(
                    to,
                    placed.translation,
                    &frame,
                    metres_per(track.unit, skeleton),
                    track.combine,
                    frames.ground,
                );
            }

            Channel::WeaponRotation { keys } => {
                let Some(placed) = weapon.as_deref_mut() else {
                    continue;
                };
                let Some(to) = sample_key(keys, time, length, looping) else {
                    continue;
                };
                match track.combine {
                    // In the track's own space; free axes keep the hold's.
                    Combine::Replace => {
                        if let Some(frame) = frames.of(track.space) {
                            let beneath = frame.rotation.inverse() * placed.rotation;
                            placed.rotation =
                                frame.rotation * quat_from_euler(angles_for(to, beneath));
                        }
                    }
                    Combine::Compose => {
                        placed.rotation *= quat_from_euler(angles_onto(to));
                    }
                }
            }
        }
    }

    reaches
}

/// Which way a knee or an elbow points when a track does not say, in the
/// body's frame.
fn default_pole(chain: ChainRef) -> Vec3 {
    if chain.is_leg() {
        // Knees bend forwards, even in a sidestep.
        Vec3::NEG_Z
    } else {
        ik::elbow_pole(chain.side())
    }
}

/// Apply a whole stack, lowest layer first; within a layer, in stack order.
/// A track in a region a higher layer claims is skipped. A partial-weight
/// layer is applied to a copy and blended back with [`Pose::lerp`]; full
/// weight skips the clone. Clips are looked up in `clips` as they are sampled.
pub fn apply_stack(
    stack: &ClipStack,
    clips: &Assets<Clip>,
    skeleton: &Skeleton,
    pose: &mut Pose,
    frames: &Frames,
    mut weapon: Option<&mut Transform>,
) {
    let mut ranked: Vec<&Playing> = stack.0.iter().collect();
    // Stable, so a blend's two variants keep their order.
    ranked.sort_by_key(|playing| playing.layer);

    for playing in ranked {
        if playing.weight <= 0.0 {
            continue;
        }
        let Some(clip) = clips.get(&playing.clip) else {
            continue;
        };
        let masked = stack.claimed_above(playing.layer);

        if playing.weight >= 1.0 {
            apply_masked(clip, playing.time, skeleton, pose, frames, weapon.as_deref_mut(), masked);
            continue;
        }

        let before = pose.clone();
        // The weapon blends by weight too.
        let was = weapon.as_deref().copied();
        apply_masked(clip, playing.time, skeleton, pose, frames, weapon.as_deref_mut(), masked);
        *pose = Pose::lerp(&before, pose, playing.weight);

        if let (Some(placed), Some(was)) = (weapon.as_deref_mut(), was) {
            placed.translation = was.translation.lerp(placed.translation, playing.weight);
            placed.rotation = was.rotation.slerp(placed.rotation, playing.weight);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::skeleton::clip::{Clock, Track};
    use crate::common::skeleton::rig::{bone, default_humanoid};

    fn keys(times: &[f32]) -> Vec<Key> {
        times
            .iter()
            .enumerate()
            .map(|(index, at)| Key {
                at: *at,
                to: [Axis::Value(index as f32), Axis::Free, Axis::Free],
                ease: Interpolation::Linear,
            })
            .collect()
    }

    fn first_axis(sampled: Option<[Axis; 3]>) -> f32 {
        sampled.expect("expected a sample")[0]
            .value()
            .expect("expected a number")
    }

    /// A default from an empty track would overwrite the layer beneath.
    #[test]
    fn an_empty_track_has_nothing_to_say() {
        assert!(sample_key(&[], 0.5, 1.0, false).is_none());
        assert!(sample_key(&[], 0.5, 1.0, false).is_none());
    }

    /// One key is a constant, not a degenerate segment.
    #[test]
    fn one_key_is_a_constant() {
        let keys = keys(&[0.4]);
        for time in [0.0, 0.4, 0.9] {
            assert_eq!(first_axis(sample_key(&keys, time, 1.0, false)), 0.0);
        }
    }

    /// Extrapolating a one-shot puts a limb somewhere no key asked for.
    #[test]
    fn a_one_shot_holds_at_both_ends() {
        let keys = keys(&[0.25, 0.75]);
        assert_eq!(first_axis(sample_key(&keys, 0.0, 1.0, false)), 0.0);
        assert_eq!(first_axis(sample_key(&keys, 1.0, 1.0, false)), 1.0);
    }

    /// Rollback and backwards scrubbing ask for times out of order.
    #[test]
    fn sampling_is_stateless() {
        let keys = keys(&[0.0, 0.5, 1.0]);
        let forwards: Vec<f32> = (0..=20)
            .map(|step| first_axis(sample_key(&keys, step as f32 / 20.0, 1.0, false)))
            .collect();
        let backwards: Vec<f32> = (0..=20)
            .rev()
            .map(|step| first_axis(sample_key(&keys, step as f32 / 20.0, 1.0, false)))
            .collect();

        let mut backwards = backwards;
        backwards.reverse();
        assert_eq!(forwards, backwards);
    }

    /// A discontinuous seam makes a loop lurch.
    #[test]
    fn a_looping_clip_is_continuous_across_the_seam() {
        let keys = keys(&[0.0, 0.6]);
        let length = 1.0;

        let before = first_axis(sample_key(&keys, length - 1e-4, length, true));
        let after = first_axis(sample_key(&keys, 0.0, length, true));

        assert!(
            (before - after).abs() < 0.01,
            "the seam jumped: {before} then {after}"
        );
    }

    /// There is no blend between a `Ground` constraint and a value.
    #[test]
    fn a_change_of_kind_switches_rather_than_blending() {
        let keys = vec![
            Key { at: 0.0, to: [Axis::Free, Axis::ON_THE_GROUND, Axis::Free], ease: Interpolation::Linear },
            Key { at: 1.0, to: [Axis::Free, Axis::Value(0.5), Axis::Free], ease: Interpolation::Linear },
        ];
        assert_eq!(sample_key(&keys, 0.5, 1.0, false).unwrap()[1], Axis::ON_THE_GROUND);
        assert_eq!(sample_key(&keys, 1.0, 1.0, false).unwrap()[1], Axis::Value(0.5));
    }

    #[test]
    fn two_keys_at_one_time_do_not_divide_by_zero() {
        let keys = keys(&[0.5, 0.5]);
        let sampled = first_axis(sample_key(&keys, 0.5, 1.0, false));
        assert!(sampled.is_finite(), "a zero-length segment produced {sampled}");
    }

    #[test]
    fn a_unit_is_a_fraction_of_the_body_it_is_on() {
        let rig = default_humanoid();
        assert_eq!(metres_per(Unit::Metres, rig), 1.0);
        assert!((metres_per(Unit::LegLengths, rig) - ik::leg_length(rig)).abs() < 1e-6);
        assert!(metres_per(Unit::HipHeights, rig) > 0.0);
        assert!(metres_per(Unit::ArmLengths, rig) > 0.0);
    }

    /// One clip must land the foot on floors at different heights. The
    /// clearance is the ankle's resting height, since the effector is the
    /// ankle and a bare floor height is out of the leg's reach.
    #[test]
    fn a_ground_axis_lands_on_the_floor_under_the_body() {
        let rig = default_humanoid().clone();
        let scale = metres_per(Unit::LegLengths, &rig);

        let resting = chain_tip(&rig, &Pose::rest(), &Transform::IDENTITY, &ik::RIGHT_LEG)
            .expect("the rig has a right leg");
        let clearance = resting.y / scale;

        let clip = Clip {
            length: 1.0,
            clock: Clock::Seconds,
            tracks: vec![Track {
                combine: Combine::Replace,
                space: Space::Body,
                unit: Unit::LegLengths,
                enabled: true,
                timing: Default::default(),
                channel: Channel::Effector {
                    chain: ChainRef::RightLeg,
                    pole: None,
                    keys: vec![Key {
                        at: 0.0,
                        to: [Axis::Free, Axis::Ground { clearance }, Axis::Free],
                        ease: Interpolation::Linear,
                    }],
                },
            }],
            ..Clip::default()
        };

        for floor in [0.0_f32, 0.1, 0.2] {
            let root = Transform::IDENTITY;
            let ground = move |_: Vec3| floor;
            let frames = Frames::standing(root, Transform::IDENTITY, &ground);

            let mut pose = Pose::rest();
            apply(&clip, 0.0, &rig, &mut pose, &frames, None);

            let ankle = chain_tip(&rig, &pose, &root, &ik::RIGHT_LEG).expect("the rig has a leg");
            let wanted = floor + clearance * scale;
            assert!(
                (ankle.y - wanted).abs() < 1e-3,
                "with the floor at {floor} the ankle wanted {wanted} and landed at {}",
                ankle.y
            );
        }

        // Out of reach: `Short`, not stretched.
        let ground = |_: Vec3| -0.5;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let mut pose = Pose::rest();
        let reaches = apply(&clip, 0.0, &rig, &mut pose, &frames, None);
        assert_eq!(reaches, vec![Reach::Short], "a floor out of reach was reached");
    }

    /// Re-solving onto an unchanged tip rewrites the roll, so a silent clip
    /// slowly rolls a limb over.
    #[test]
    fn a_wholly_free_track_leaves_the_pose_exactly_alone() {
        let rig = default_humanoid().clone();
        let clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                combine: Combine::Replace,
                space: Space::Body,
                unit: Unit::Metres,
                enabled: true,
                timing: Default::default(),
                channel: Channel::Effector {
                    chain: ChainRef::RightArm,
                    pole: None,
                    keys: vec![Key {
                        at: 0.0,
                        to: [Axis::Free; 3],
                        ease: Interpolation::Linear,
                    }],
                },
            }],
            ..Clip::default()
        };

        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);

        let before = Pose::rest().with(bone::UPPER_ARM_R, Quat::from_rotation_x(0.3));
        let mut pose = before.clone();
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);

        // Exact: nothing should have run.
        assert_eq!(
            pose.joint(bone::UPPER_ARM_R),
            before.joint(bone::UPPER_ARM_R),
            "a wholly free track moved a joint"
        );
    }

    /// A `Replace` key turning one axis must not zero the other two.
    #[test]
    fn a_rotation_key_that_frees_an_axis_leaves_it_alone() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);

        let mut clip = Clip {
            length: 1.0,
            tracks: vec![Track::rotation(bone::CHEST)],
            ..Clip::default()
        };
        let Channel::Rotation { keys, .. } = &mut clip.tracks[0].channel else { unreachable!() };
        keys.push(Key {
            at: 0.0,
            to: [Axis::Value(0.3), Axis::Free, Axis::Free],
            ease: Interpolation::Linear,
        });

        let beneath = quat_from_euler(Vec3::new(-0.9, 0.4, -0.25));
        let mut pose = Pose::rest().with(bone::CHEST, beneath);
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);

        let after = crate::common::rotation::euler_from_quat(pose.joint(bone::CHEST));
        assert!((after.x - 0.3).abs() < 1e-4, "the keyed axis did not take: {after:?}");
        assert!((after.y - 0.4).abs() < 1e-4, "a free axis was overwritten: {after:?}");
        assert!((after.z + 0.25).abs() < 1e-4, "a free axis was overwritten: {after:?}");
    }

    /// Under `Compose`, keeping the angle beneath would double it.
    #[test]
    fn a_composed_rotation_key_turns_a_free_axis_by_nothing() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);

        let mut clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                combine: Combine::Compose,
                ..Track::rotation(bone::CHEST)
            }],
            ..Clip::default()
        };
        let Channel::Rotation { keys, .. } = &mut clip.tracks[0].channel else { unreachable!() };
        keys.push(Key {
            at: 0.0,
            to: [Axis::Value(0.2), Axis::Free, Axis::Free],
            ease: Interpolation::Linear,
        });

        let beneath = quat_from_euler(Vec3::new(0.0, 0.5, 0.0));
        let mut pose = Pose::rest().with(bone::CHEST, beneath);
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);

        let wanted = beneath * quat_from_euler(Vec3::new(0.2, 0.0, 0.0));
        assert!(
            pose.joint(bone::CHEST).abs_diff_eq(wanted, 1e-5),
            "a free axis contributed a turn of its own",
        );
    }

    /// Keying a fresh track must not move a body.
    #[test]
    fn keying_an_empty_track_changes_no_pose() {
        let mut channel = Channel::Rotation {
            bone: bone::CHEST.to_string(),
            keys: Vec::new(),
        };
        key_the_channel(&mut channel, 0.5, 1.0, false);

        let Channel::Rotation { keys, .. } = &channel else { unreachable!() };
        assert_eq!(keys[0].to, [Axis::Free; 3], "a fresh key had an opinion");
    }

    #[test]
    fn a_disabled_track_does_nothing() {
        let rig = default_humanoid().clone();
        let mut clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                enabled: false,
                timing: Default::default(),
                ..Track::rotation(bone::CHEST)
            }],
            ..Clip::default()
        };
        let Channel::Rotation { keys, .. } = &mut clip.tracks[0].channel else {
            unreachable!()
        };
        keys.push(Key::all(0.0, [0.5, 0.0, 0.0], Interpolation::Linear));

        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let mut pose = Pose::rest();
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);

        assert_eq!(pose.joint(bone::CHEST), Quat::IDENTITY);
    }

    #[test]
    fn compose_builds_on_what_was_there_and_replace_does_not() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let beneath = Quat::from_rotation_x(0.2);
        let turn = [0.3_f32, 0.0, 0.0];

        let build = |combine| {
            let mut clip = Clip { length: 1.0, tracks: vec![Track { combine, ..Track::rotation(bone::CHEST) }], ..Clip::default() };
            let Channel::Rotation { keys, .. } = &mut clip.tracks[0].channel else { unreachable!() };
            keys.push(Key::all(0.0, turn, Interpolation::Linear));
            clip
        };

        let mut replaced = Pose::rest().with(bone::CHEST, beneath);
        apply(&build(Combine::Replace), 0.0, &rig, &mut replaced, &frames, None);

        let mut composed = Pose::rest().with(bone::CHEST, beneath);
        apply(&build(Combine::Compose), 0.0, &rig, &mut composed, &frames, None);

        let authored = quat_from_euler(Vec3::from_array(turn));
        assert!(replaced.joint(bone::CHEST).abs_diff_eq(authored, 1e-5));
        assert!(composed.joint(bone::CHEST).abs_diff_eq(beneath * authored, 1e-5));
    }

    /// One mistyped bone must not blank the whole clip.
    #[test]
    fn a_clip_naming_an_unknown_bone_still_plays_its_other_tracks() {
        let rig = default_humanoid().clone();
        let mut clip = Clip {
            length: 1.0,
            tracks: vec![Track::rotation("elbow.q"), Track::rotation(bone::CHEST)],
            ..Clip::default()
        };
        for track in &mut clip.tracks {
            let Channel::Rotation { keys, .. } = &mut track.channel else { unreachable!() };
            keys.push(Key::all(0.0, [0.4, 0.0, 0.0], Interpolation::Linear));
        }

        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let mut pose = Pose::rest();
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);

        assert_ne!(pose.joint(bone::CHEST), Quat::IDENTITY, "the good track did not play");
    }

    #[test]
    fn a_weapon_track_moves_the_weapon() {
        let rig = default_humanoid().clone();
        let clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                combine: Combine::Compose,
                space: Space::Carry,
                unit: Unit::Metres,
                enabled: true,
                timing: Default::default(),
                channel: Channel::Weapon {
                    keys: vec![Key {
                        at: 0.0,
                        to: [Axis::Free, Axis::Value(-0.2), Axis::Free],
                        ease: Interpolation::Linear,
                    }],
                },
            }],
            ..Clip::default()
        };

        let ground = |_: Vec3| 0.0;
        let frames = Frames {
            body: Transform::IDENTITY,
            carry: Transform::IDENTITY,
            weapon: Some(Transform::IDENTITY),
            eye: None,
            ground: &ground,
            footfalls: None,
        };

        let mut placed = Transform::IDENTITY;
        let mut pose = Pose::rest();
        apply(&clip, 0.0, &rig, &mut pose, &frames, Some(&mut placed));

        assert!((placed.translation.y + 0.2).abs() < 1e-5, "the weapon did not dip: {placed:?}");
    }

    #[test]
    fn a_weapon_track_on_an_empty_hand_does_nothing() {
        let rig = default_humanoid().clone();
        let clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                combine: Combine::Compose,
                space: Space::Weapon,
                unit: Unit::Metres,
                enabled: true,
                timing: Default::default(),
                channel: Channel::Weapon {
                    keys: vec![Key { at: 0.0, to: [Axis::Value(1.0); 3], ease: Interpolation::Linear }],
                },
            }],
            ..Clip::default()
        };

        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let mut pose = Pose::rest();
        apply(&clip, 0.0, &rig, &mut pose, &frames, None);
        assert!(pose.posed_joints().next().is_none());
    }

    /// A key landing on a default would jump the body when added.
    #[test]
    fn inserting_a_key_holds_what_the_track_already_said() {
        let mut channel = Channel::Weapon {
            keys: vec![
                Key { at: 0.0, to: [Axis::Value(0.0), Axis::Free, Axis::Free], ease: Interpolation::Linear },
                Key { at: 1.0, to: [Axis::Value(1.0), Axis::Free, Axis::Free], ease: Interpolation::Linear },
            ],
        };

        let before = sample_key(
            match &channel { Channel::Weapon { keys } => keys, _ => unreachable!() },
            0.25, 1.0, false,
        );

        let landed = key_the_channel(&mut channel, 0.25, 1.0, false);

        let Channel::Weapon { keys } = &channel else { unreachable!() };
        assert_eq!(keys.len(), 3, "the key was not added");
        assert_eq!(keys[landed].at, 0.25, "the key did not land where it was asked for");
        assert_eq!(
            Some(keys[landed].to), before,
            "adding a key changed what the track said at that moment",
        );
    }

    #[test]
    fn inserting_a_key_does_not_bend_the_curve_around_it() {
        let keys = vec![
            Key { at: 0.0, to: [Axis::Value(0.0), Axis::Free, Axis::Free], ease: Interpolation::Linear },
            Key { at: 1.0, to: [Axis::Value(1.0), Axis::Free, Axis::Free], ease: Interpolation::Linear },
        ];
        let before: Vec<f32> = (0..=20)
            .map(|step| {
                sample_key(&keys, step as f32 / 20.0, 1.0, false).unwrap()[0].value().unwrap()
            })
            .collect();

        let mut channel = Channel::Weapon { keys };
        key_the_channel(&mut channel, 0.25, 1.0, false);
        let Channel::Weapon { keys } = &channel else { unreachable!() };

        for (step, was) in before.iter().enumerate() {
            let now = sample_key(keys, step as f32 / 20.0, 1.0, false).unwrap()[0]
                .value()
                .unwrap();
            assert!(
                (now - was).abs() < 1e-5,
                "adding a key moved the curve at {}: {was} became {now}",
                step as f32 / 20.0,
            );
        }
    }

    #[test]
    fn a_layer_shows_as_much_of_itself_as_its_weight() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);

        let mut clip = Clip { length: 1.0, tracks: vec![Track::rotation(bone::CHEST)], ..Clip::default() };
        let Channel::Rotation { keys, .. } = &mut clip.tracks[0].channel else { unreachable!() };
        keys.push(Key::all(0.0, [0.6, 0.0, 0.0], Interpolation::Linear));
        let mut clips = Assets::<Clip>::default();
        let clip = clips.add(clip);

        let full = {
            let mut pose = Pose::rest();
            apply_stack(
                &ClipStack(vec![Playing::new(Layer::Locomotion, clip.clone(), 0.0)]),
                &clips, &rig, &mut pose, &frames, None,
            );
            pose.joint(bone::CHEST)
        };

        let half = {
            let mut pose = Pose::rest();
            apply_stack(
                &ClipStack(vec![Playing { weight: 0.5, ..Playing::new(Layer::Locomotion, clip.clone(), 0.0) }]),
                &clips, &rig, &mut pose, &frames, None,
            );
            pose.joint(bone::CHEST)
        };

        let none = {
            let mut pose = Pose::rest();
            apply_stack(
                &ClipStack(vec![Playing { weight: 0.0, ..Playing::new(Layer::Locomotion, clip, 0.0) }]),
                &clips, &rig, &mut pose, &frames, None,
            );
            pose.joint(bone::CHEST)
        };

        assert_eq!(none, Quat::IDENTITY, "a weightless layer showed");
        assert!(half.angle_between(Quat::IDENTITY) < full.angle_between(Quat::IDENTITY));
        assert!(half.angle_between(Quat::IDENTITY) > 1e-3, "a half-weight layer did not show at all");
    }

    /// Whatever order a stack is in, the higher layer wins a region it claims,
    /// and a lower layer still plays where nothing outranks it.
    #[test]
    fn a_higher_layer_wins_what_it_claims_and_nothing_else() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let turning = |bones: &[(&str, f32)]| {
            let tracks = bones
                .iter()
                .map(|(bone, angle)| {
                    let mut track = Track::rotation(*bone);
                    let Channel::Rotation { keys, .. } = &mut track.channel else { unreachable!() };
                    keys.push(Key::all(0.0, [*angle, 0.0, 0.0], Interpolation::Linear));
                    track
                })
                .collect();
            Clip { length: 1.0, tracks, ..Clip::default() }
        };
        let mut clips = Assets::<Clip>::default();
        let below = clips.add(turning(&[(bone::CHEST, 0.6), (bone::THIGH_L, 0.4)]));
        let above = clips.add(turning(&[(bone::CHEST, 0.2)]));

        let mut pose = Pose::rest();
        let stack = ClipStack(vec![
            Playing { claims: Regions::TORSO, ..Playing::new(Layer::Humiliation, above, 0.0) },
            Playing::new(Layer::Locomotion, below, 0.0),
        ]);
        apply_stack(&stack, &clips, &rig, &mut pose, &frames, None);

        let angle = |joint| pose.joint(joint).angle_between(Quat::IDENTITY);
        assert!((angle(bone::CHEST) - 0.2).abs() < 1e-4, "the lower layer won the chest");
        assert!((angle(bone::THIGH_L) - 0.4).abs() < 1e-4, "the higher layer masked a leg it never claimed");
        assert_eq!(stack.owner(Regions::TORSO), Some(Layer::Humiliation));
        assert_eq!(stack.owner(Regions::LEGS), Some(Layer::Locomotion));
    }

    /// In over its blend, out over its blend, then off the stack; a layer
    /// with no blend snaps both ways.
    #[test]
    fn a_clip_fades_in_and_out_at_its_layers_blend() {
        let mut clips = Assets::<Clip>::default();
        let clip = clips.add(Clip::default());
        let blends = LayerBlends(
            [(Layer::Humiliation, Blend { blend_in: 0.2, blend_out: 0.4 })].into_iter().collect(),
        );
        let wanted = Some((Layer::Humiliation, &clip));
        let ours = |layer| layer == Layer::Humiliation;

        let mut stack = ClipStack::default();
        stack.keep(ours, wanted);
        assert_eq!(stack.0[0].weight, 0.0, "a clip came in already showing");
        stack.fade(0.1, &clips, &blends);
        assert!((stack.0[0].weight - 0.5).abs() < 1e-5, "{}", stack.0[0].weight);
        stack.fade(0.1, &clips, &blends);
        assert_eq!(stack.0[0].weight, 1.0);

        stack.keep(ours, None);
        stack.fade(0.2, &clips, &blends);
        assert!((stack.0[0].weight - 0.5).abs() < 1e-5, "{}", stack.0[0].weight);
        assert!((stack.0[0].time - 0.2).abs() < 1e-5, "a leaving clip stopped its clock");

        // Wanted back halfway out: the same clip returns, from where it was.
        stack.keep(ours, wanted);
        assert_eq!(stack.0.len(), 1, "a second copy was brought in");
        stack.fade(0.05, &clips, &blends);
        assert!((stack.0[0].weight - 0.75).abs() < 1e-5, "{}", stack.0[0].weight);

        stack.keep(ours, None);
        stack.fade(1.0, &clips, &blends);
        assert!(stack.0.is_empty(), "a faded-out clip stayed on the stack");

        let mut snapping = ClipStack::default();
        snapping.keep(ours, wanted);
        snapping.fade(1e-3, &clips, &LayerBlends::default());
        assert_eq!(snapping.0[0].weight, 1.0, "a layer with no blend faded");
    }

    /// A layer fading in masks nothing, so it blends over what is beneath.
    #[test]
    fn a_layer_fading_in_shows_over_the_one_beneath() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);
        let mut clips = Assets::<Clip>::default();
        let below = clips.add(chest_turned_by(0.6));
        let above = clips.add(chest_turned_by(0.2));

        let mut pose = Pose::rest();
        let stack = ClipStack(vec![
            Playing { weight: 0.5, ..Playing::new(Layer::Humiliation, above, 0.0) },
            Playing::new(Layer::Locomotion, below, 0.0),
        ]);
        apply_stack(&stack, &clips, &rig, &mut pose, &frames, None);
        let angle = pose.joint(bone::CHEST).angle_between(Quat::IDENTITY);
        assert!((angle - 0.4).abs() < 1e-3, "halfway from 0.6 to 0.2 should be 0.4, not {angle}");
    }

    fn chest_turned_by(angle: f32) -> Clip {
        let mut clip = Clip { length: 1.0, tracks: vec![Track::rotation(bone::CHEST)], ..Clip::default() };
        let Channel::Rotation { keys, .. } = &mut clip.tracks[0].channel else { unreachable!() };
        keys.push(Key::all(0.0, [angle, 0.0, 0.0], Interpolation::Linear));
        clip
    }

    /// **Updating a clip is inserting it under the handle it already has.** A
    /// stack built once goes on playing whatever that handle now holds, so a
    /// saved or re-sent clip reaches every body playing it without anybody
    /// rebuilding a stack or copying a clip onto it each frame.
    #[test]
    fn a_clip_reinserted_under_its_handle_is_what_plays_next() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);

        let mut clips = Assets::<Clip>::default();
        let handle = clips.add(chest_turned_by(0.2));
        let stack = ClipStack(vec![Playing::new(Layer::Locomotion, handle.clone(), 0.0)]);

        let chest = |clips: &Assets<Clip>| {
            let mut pose = Pose::rest();
            apply_stack(&stack, clips, &rig, &mut pose, &frames, None);
            pose.joint(bone::CHEST)
        };

        let before = chest(&clips);
        clips.insert(handle.id(), chest_turned_by(0.6)).unwrap();
        let after = chest(&clips);

        assert!(
            after.angle_between(Quat::IDENTITY) > before.angle_between(Quat::IDENTITY) + 0.3,
            "the stack went on playing the clip it was built with: {before} then {after}",
        );
    }

    /// A handle with nothing behind it plays nothing, rather than panicking —
    /// the same "key on absence" every other lookup on a body follows.
    #[test]
    fn a_handle_whose_clip_is_gone_plays_nothing() {
        let rig = default_humanoid().clone();
        let ground = |_: Vec3| 0.0;
        let frames = Frames::standing(Transform::IDENTITY, Transform::IDENTITY, &ground);

        let mut clips = Assets::<Clip>::default();
        let handle = clips.add(chest_turned_by(0.6));
        clips.remove(&handle);

        let mut pose = Pose::rest();
        apply_stack(
            &ClipStack(vec![Playing::new(Layer::Locomotion, handle, 0.0)]),
            &clips, &rig, &mut pose, &frames, None,
        );
        assert_eq!(pose.joint(bone::CHEST), Quat::IDENTITY);
    }
}
