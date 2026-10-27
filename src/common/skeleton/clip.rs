//! Animation clips as data.
//!
//! A key holds a target, not a value: what to move, where, in which [`Space`]
//! and [`Unit`], per axis. Points are in leg- or arm-lengths for the same
//! reason [`FootOffset`] and [`HoldSpec::carry`] are: a point in metres is
//! right for one build only. The per-axis shape mirrors the map editor's
//! `PointRef`.
//!
//! A clip is sparse: a channel with no keys samples to `None`, not a default,
//! so a layer never overwrites the one beneath it with zeros. Absence means
//! rest, as in [`Pose`].
//!
//! [`Channel`] is internally tagged and [`Axis`] needs `deserialize_any`, so
//! neither survives postcard; the failure is silent, on the receive side.
//! Clips reach clients as the text of the `.gpp` holding them, and nothing
//! here is registered as a message (`a_clip_would_not_survive_the_wire`).
//!
//! [`FootOffset`]: super::gait::FootOffset
//! [`HoldSpec::carry`]: crate::prop::hold::HoldSpec::carry
//! [`Pose`]: super::rig::Pose

use std::collections::BTreeMap;
use std::fmt;

use bevy::asset::Asset;
use bevy::reflect::TypePath;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::ik::{self, Chain};
use super::interpolation::Interpolation;

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

/// Where a point is measured from.
///
/// The frames are supplied by the caller (`play::Frames`) so a clip can be
/// sampled in a unit test with no `World`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Space {
    /// The body's own frame: origin at the feet, `+X` its right, `-Z` the way
    /// it faces.
    #[default]
    Body,
    /// The point between the shoulders, turned by the aim.
    Carry,
    /// The same point, turned with the body and not the aim: for hands up or
    /// a wave, which should not tilt as the player looks up.
    Shoulders,
    /// The weapon's frame, origin at its grip. A target here stays on the weapon
    /// through a recoil.
    Weapon,
    /// The eye, for a viewmodel clip.
    Eye,
    /// An arm's own grip on the weapon: origin where that hand holds, axes the
    /// weapon's. An arm effector here moves the grip rather than the hand, so
    /// a key at zero is holding and a reload can take the support hand to a
    /// magazine and back. Read by the grip, never solved by the clip.
    Grip,
}

/// What a number on a key is measured in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    #[default]
    Metres,
    LegLengths,
    ArmLengths,
    HipHeights,
}

/// What drives a clip's time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Clock {
    /// Seconds since this performance started. A fire, a reload, a deploy.
    #[default]
    Seconds,
    /// The body's stride, wrapping at 1. A walk, a run.
    Stride,
    /// The shared clock plus the body's own phase, wrapping at `length`. The
    /// per-body phase keeps an animation grid out of lockstep.
    Shared,
}

/// What a track does to the layer beneath it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Combine {
    #[default]
    Replace,
    /// Multiplied onto the layer beneath, in the joint's own frame.
    Compose,
}

/// What a track's keys are timed against.
///
/// A foot's cycle is normalised so 0–½ is that foot on the ground and ½–1 in
/// the air. Stance fraction changes with speed, so keys timed against a foot
/// stay right at every speed where keys timed against the stride would not.
/// Only meaningful on the stride clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    /// The clip's own clock.
    #[default]
    Clip,
    LeftFoot,
    RightFoot,
}

impl Timing {
    pub const ALL: [Timing; 3] = [Timing::Clip, Timing::LeftFoot, Timing::RightFoot];

    pub fn name_key(self) -> &'static str {
        match self {
            Timing::Clip => "anim.timing.clip",
            Timing::LeftFoot => "anim.timing.left_foot",
            Timing::RightFoot => "anim.timing.right_foot",
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Timing::Clip
    }
}

/// Which two-bone chain an effector track drives.
///
/// An enum rather than bone names because `Pose::set` takes a `&'static str`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainRef {
    LeftArm,
    RightArm,
    LeftLeg,
    RightLeg,
}

impl ChainRef {
    /// In menu order.
    pub const ALL: [ChainRef; 4] = [
        ChainRef::RightArm,
        ChainRef::LeftArm,
        ChainRef::RightLeg,
        ChainRef::LeftLeg,
    ];

    pub fn chain(self) -> Chain {
        match self {
            ChainRef::LeftArm => ik::LEFT_ARM,
            ChainRef::RightArm => ik::RIGHT_ARM,
            ChainRef::LeftLeg => ik::LEFT_LEG,
            ChainRef::RightLeg => ik::RIGHT_LEG,
        }
    }

    /// -1 on the left, 1 on the right; what a pole is mirrored by.
    pub fn side(self) -> f32 {
        match self {
            ChainRef::LeftArm | ChainRef::LeftLeg => -1.0,
            ChainRef::RightArm | ChainRef::RightLeg => 1.0,
        }
    }

    pub fn name_key(self) -> &'static str {
        match self {
            ChainRef::RightArm => "anim.chain.right_arm",
            ChainRef::LeftArm => "anim.chain.left_arm",
            ChainRef::RightLeg => "anim.chain.right_leg",
            ChainRef::LeftLeg => "anim.chain.left_leg",
        }
    }

    pub fn is_leg(self) -> bool {
        matches!(self, ChainRef::LeftLeg | ChainRef::RightLeg)
    }
}

/// One component of a target.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Axis {
    /// Leave whatever the layer beneath put here.
    #[default]
    Free,
    /// `clearance` above the surface under the point, measured at sample time, in
    /// the track's unit. A leg's effector is the ankle, so a planted foot is the
    /// ankle's resting height off the floor, not zero. Vertical axis only.
    Ground { clearance: f32 },
    /// A number, in the track's own unit.
    Value(f32),
}

impl Axis {
    pub fn value(self) -> Option<f32> {
        match self {
            Axis::Value(at) => Some(at),
            _ => None,
        }
    }

    pub const ON_THE_GROUND: Axis = Axis::Ground { clearance: 0.0 };

    pub fn is_ground(self) -> bool {
        matches!(self, Axis::Ground { .. })
    }

    pub fn offset(self) -> Option<f32> {
        match self {
            Axis::Value(at) | Axis::Ground { clearance: at } => Some(at),
            Axis::Free => None,
        }
    }
}

/// Hand-written so a key reads `to = [0.2, ["ground", 0.08], "free"]`.
/// Derived, `Value` is a table; `#[serde(untagged)]` widens the float through
/// serde and prints `0.05000000074505806`, which `nice_f32` exists to avoid.
impl Serialize for Axis {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Axis::Free => serializer.serialize_str("free"),
            Axis::Ground { clearance } if *clearance == 0.0 => serializer.serialize_str("ground"),
            Axis::Ground { clearance } => {
                ("ground", crate::prop::nice_f32::widen(*clearance)).serialize(serializer)
            }
            Axis::Value(at) => crate::prop::nice_f32::widen(*at).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Axis {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Axis, D::Error> {
        struct AxisVisitor;

        impl<'de> Visitor<'de> for AxisVisitor {
            type Value = Axis;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str(r#"a number, "free", or "ground""#)
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Axis, E> {
                match value {
                    "free" => Ok(Axis::Free),
                    "ground" => Ok(Axis::ON_THE_GROUND),
                    other => Err(E::unknown_variant(other, &["free", "ground"])),
                }
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Axis, E> {
                Ok(Axis::Value(value as f32))
            }

            // TOML parses a hand-typed `0` as an integer.
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Axis, E> {
                Ok(Axis::Value(value as f32))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Axis, E> {
                Ok(Axis::Value(value as f32))
            }

            /// The pair form, `["ground", 0.08]`.
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Axis, A::Error> {
                let datum: String = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                if datum != "ground" {
                    return Err(de::Error::unknown_variant(&datum, &["ground"]));
                }
                let clearance: f64 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                Ok(Axis::Ground { clearance: clearance as f32 })
            }
        }

        deserializer.deserialize_any(AxisVisitor)
    }
}

/// One key: a time, three optional axes, and the curve out of it.
///
/// One type for every channel: a rotation's axes are Euler angles and a
/// point's are coordinates, and [`Axis::Free`] leaves an axis to the layer
/// beneath either way.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    /// On the clip's own clock.
    #[serde(with = "crate::prop::nice_f32::scalar")]
    pub at: f32,
    /// For a rotation, Euler angles in the order [`crate::common::rotation`]
    /// owns, as `HoldSpec::grip_rotation` uses.
    pub to: [Axis; 3],
    /// Used **leaving** this key. See [`Interpolation`].
    #[serde(default)]
    pub ease: Interpolation,
}

impl Key {
    pub fn all(at: f32, to: [f32; 3], ease: Interpolation) -> Key {
        Key {
            at,
            to: to.map(Axis::Value),
            ease,
        }
    }
}

/// What a track drives.
///
/// Internally tagged for a readable file; see the module doc on the wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "channel", rename_all = "snake_case")]
pub enum Channel {
    /// A bone's rotation in its own rest frame, exactly as a [`Pose`] stores it.
    ///
    /// [`Pose`]: super::rig::Pose
    Rotation {
        bone: String,
        #[serde(default)]
        keys: Vec<Key>,
    },
    /// A two-bone chain solved onto a target through [`ik::solve`].
    Effector {
        chain: ChainRef,
        /// Which way the elbow or knee points. Defaults per chain when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pole: Option<[f32; 3]>,
        #[serde(default)]
        keys: Vec<Key>,
    },
    /// Where the weapon is, as a deviation from the hold's carry. The hands
    /// follow because `grip_the_weapon` solves both arms onto it afterwards.
    Weapon {
        #[serde(default)]
        keys: Vec<Key>,
    },
    /// How the weapon is turned there.
    WeaponRotation {
        #[serde(default)]
        keys: Vec<Key>,
    },
    /// Where the whole body sits, in hip-heights: [`Pose::root_offset`]. Its own
    /// channel because a root offset is a displacement in the body's own frame
    /// and has no [`Space`] to choose.
    ///
    /// [`Pose::root_offset`]: super::rig::Pose::root_offset
    Root {
        #[serde(default)]
        keys: Vec<Key>,
    },
    // Reserved: `Feature { feature: PropFeatureId, .. }`, for a dropped magazine.
    // Needs `prop::baked` to bake per-feature groups rather than one mesh per
    // surface.
}

impl Channel {
    pub fn kind_key(&self) -> &'static str {
        match self {
            Channel::Root { .. } => "anim.channel.root",
            Channel::Rotation { .. } => "anim.channel.rotation",
            Channel::Effector { .. } => "anim.channel.effector",
            Channel::Weapon { .. } => "anim.channel.weapon",
            Channel::WeaponRotation { .. } => "anim.channel.weapon_rotation",
        }
    }

    pub fn key_count(&self) -> usize {
        match self {
            Channel::Rotation { keys, .. }
            | Channel::WeaponRotation { keys }
            | Channel::Effector { keys, .. }
            | Channel::Weapon { keys }
            | Channel::Root { keys } => keys.len(),
        }
    }

    pub fn key_times(&self) -> Vec<f32> {
        match self {
            Channel::Rotation { keys, .. }
            | Channel::WeaponRotation { keys }
            | Channel::Effector { keys, .. }
            | Channel::Weapon { keys }
            | Channel::Root { keys } => keys.iter().map(|key| key.at).collect(),
        }
    }

    pub fn keys(&self) -> &[Key] {
        match self {
            Channel::Rotation { keys, .. }
            | Channel::WeaponRotation { keys }
            | Channel::Effector { keys, .. }
            | Channel::Weapon { keys }
            | Channel::Root { keys } => keys,
        }
    }

    pub fn keys_mut(&mut self) -> &mut Vec<Key> {
        match self {
            Channel::Rotation { keys, .. }
            | Channel::WeaponRotation { keys }
            | Channel::Effector { keys, .. }
            | Channel::Weapon { keys }
            | Channel::Root { keys } => keys,
        }
    }

    pub fn is_rotation(&self) -> bool {
        matches!(
            self,
            Channel::Rotation { .. } | Channel::WeaponRotation { .. }
        )
    }

    /// Move keys in time, re-sort, and return where the moved ones landed.
    ///
    /// Keys are addressed by index rather than id, so a drag that reorders them
    /// has to report new indices for the selection to follow. The sort is stable,
    /// so keys dragged onto the same time keep their order.
    pub fn nudge(&mut self, held: &[usize], by: f32, within: f32) -> Vec<usize> {
        fn shuffle<K: Copy>(
            keys: &mut Vec<K>,
            held: &[usize],
            by: f32,
            within: f32,
            at: impl Fn(&K) -> f32,
            set: impl Fn(&mut K, f32),
        ) -> Vec<usize> {
            let mut marked: Vec<(K, bool)> = keys
                .iter()
                .enumerate()
                .map(|(index, key)| (*key, held.contains(&index)))
                .collect();

            for (key, moving) in &mut marked {
                if *moving {
                    set(key, (at(key) + by).clamp(0.0, within));
                }
            }
            marked.sort_by(|a, b| at(&a.0).total_cmp(&at(&b.0)));

            *keys = marked.iter().map(|(key, _)| *key).collect();
            marked
                .iter()
                .enumerate()
                .filter_map(|(index, (_, moving))| moving.then_some(index))
                .collect()
        }

        match self {
            Channel::Rotation { keys, .. }
            | Channel::WeaponRotation { keys }
            | Channel::Effector { keys, .. }
            | Channel::Weapon { keys }
            | Channel::Root { keys } => shuffle(
                keys,
                held,
                by,
                within,
                |key| key.at,
                |key, at| key.at = at,
            ),
        }
    }

    /// Removed back to front so a removal does not shift later indices.
    pub fn remove_keys(&mut self, indices: &[usize]) {
        let mut doomed: Vec<usize> = indices.to_vec();
        doomed.sort_unstable();
        doomed.dedup();

        match self {
            Channel::Rotation { keys, .. }
            | Channel::WeaponRotation { keys }
            | Channel::Effector { keys, .. }
            | Channel::Weapon { keys }
            | Channel::Root { keys } => {
                for index in doomed.iter().rev() {
                    if *index < keys.len() {
                        keys.remove(*index);
                    }
                }
            }
        }
    }

    /// Called on load and after edits rather than in the sampler, which runs per
    /// channel per body per frame.
    pub fn sort(&mut self) {
        match self {
            Channel::Rotation { keys, .. }
            | Channel::WeaponRotation { keys }
            | Channel::Effector { keys, .. }
            | Channel::Weapon { keys }
            | Channel::Root { keys } => keys.sort_by(|a, b| a.at.total_cmp(&b.at)),
        }
    }
}

/// One channel of one clip, with the frame and units its keys are read in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    #[serde(default)]
    pub combine: Combine,
    #[serde(default)]
    pub space: Space,
    #[serde(default)]
    pub unit: Unit,
    /// Off for a track being auditioned.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Left out of a file when it is the clip's own clock.
    #[serde(default, skip_serializing_if = "Timing::is_default")]
    pub timing: Timing,
    pub channel: Channel,
}

impl Track {
    pub fn rotation(bone: impl Into<String>) -> Track {
        Track {
            combine: Combine::default(),
            space: Space::default(),
            unit: Unit::default(),
            enabled: true,
            timing: Default::default(),
            channel: Channel::Rotation {
                bone: bone.into(),
                keys: Vec::new(),
            },
        }
    }
}

/// One authored motion.
///
/// **An asset**, so a body playing one holds a `Handle<Clip>` the way a held
/// part holds a `Handle<Mesh>`: sixty bodies playing one deploy share it, and
/// whoever owns the clip — a baked prop, the animation editor's preview —
/// updates it by inserting under the same handle, which every body playing it
/// sees on the next sample. Nothing is loaded through the `AssetServer`; a
/// clip is read through `AssetSource` with everything else in a pack, and
/// only stored here.
#[derive(Asset, TypePath, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// The lang key this clip's name is under. Empty for one nobody has named.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name_key: String,
    /// In this clip's own clock's unit: seconds, or strides.
    #[serde(default = "one", with = "crate::prop::nice_f32::scalar")]
    pub length: f32,
    #[serde(default)]
    pub clock: Clock,
    #[serde(default)]
    pub looping: bool,
    /// Why the numbers are what they are. Data rather than a TOML comment,
    /// because the editor rewrites the file on save and `toml` drops comments.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
    /// Seconds to fade in over the layer beneath, overriding the layer's
    /// `layers.toml` entry. A clip that starts away from where its layer
    /// leaves things — a deploy, from below the frame — wants zero.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "crate::prop::nice_f32::option")]
    pub blend_in: Option<f32>,
    /// Seconds to fade back out, likewise.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "crate::prop::nice_f32::option")]
    pub blend_out: Option<f32>,
}

/// How many strides a [`Clock::Stride`] clip of `length` is played over, never
/// fewer than one. `stride` wraps at 1, so a one-stride clip plays once per step.
pub fn strides_spanned(length: f32) -> f32 {
    length.max(1.0)
}

impl Clip {
    /// Where this clip has got to, on whichever [`Clock`] it is authored against.
    pub fn time_from(&self, inputs: &super::state::PoseInputs) -> f32 {
        match self.clock {
            // They differ only in whether the clip wraps, which `looping` says.
            Clock::Seconds | Clock::Shared => inputs.seconds,
            Clock::Stride => inputs.stride * strides_spanned(self.length),
        }
    }

    pub fn normalise(&mut self) {
        for track in &mut self.tracks {
            track.channel.sort();
        }
    }

    /// `time` wrapped if the clip loops. A one-shot is left unwrapped and the
    /// sampler clamps, so it holds its last key rather than snapping back.
    pub fn wrap(&self, time: f32) -> f32 {
        if !self.looping || self.length <= 0.0 {
            return time;
        }
        time.rem_euclid(self.length)
    }

    /// What is wrong with this clip, for the Problems panel. Nothing refuses: a
    /// clip with a dangling bone name still plays its other tracks.
    pub fn problems(&self, known_bone: impl Fn(&str) -> bool) -> Vec<ClipProblem> {
        use crate::get;
        let mut problems = Vec::new();
        let clip_wide = |message: String| ClipProblem { track: None, message };
        if self.length <= 0.0 {
            problems.push(clip_wide(get!(
                "anim.problem.no_length",
                "length",
                self.length.to_string()
            )));
        }
        // A fractional stride count lurches every other step.
        if self.clock == Clock::Stride && (self.length - self.length.round()).abs() > 1e-3 {
            problems.push(clip_wide(get!(
                "anim.problem.stride_length",
                "length",
                self.length.to_string()
            )));
        }
        for (index, track) in self.tracks.iter().enumerate() {
            let mut on_track = |message: String| {
                problems.push(ClipProblem {
                    track: Some(index),
                    message,
                })
            };
            match &track.channel {
                Channel::Rotation { bone, .. } if !known_bone(bone) => {
                    on_track(get!("anim.problem.unknown_bone", "bone", bone.clone()));
                }
                Channel::Rotation { keys, .. } | Channel::WeaponRotation { keys } => {
                    for key in keys {
                        // An angle has no floor under it.
                        if key.to.iter().any(|axis| axis.is_ground()) {
                            on_track(get!("anim.problem.ground_angle", "at", key.at.to_string()));
                        }
                    }
                }
                Channel::Effector { keys, .. } | Channel::Weapon { keys } => {
                    for key in keys {
                        // Sideways has no floor; rejected rather than given an invented meaning.
                        if key.to[0].is_ground() || key.to[2].is_ground() {
                            on_track(get!(
                                "anim.problem.ground_sideways",
                                "at",
                                key.at.to_string()
                            ));
                        }
                    }
                }
                Channel::Root { keys } => {
                    for key in keys {
                        // A root offset is already measured from where the floor put the body.
                        if key.to.iter().any(|axis| axis.is_ground()) {
                            on_track(get!("anim.problem.ground_root", "at", key.at.to_string()));
                        }
                    }
                }
            }
            if !track.timing.is_default() && self.clock != Clock::Stride {
                on_track(get!("anim.problem.foot_timing_off_stride"));
            }
            for at in track.channel.key_times() {
                if at < 0.0 || at > self.length {
                    on_track(get!("anim.problem.outside", "at", at.to_string()));
                }
            }
        }
        problems
    }
}

/// One thing wrong with a clip. The track is carried rather than written into
/// the message so the strip can badge its row.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipProblem {
    /// `None` for something about the whole clip.
    pub track: Option<usize>,
    pub message: String,
}

/// What a body state plays: one clip, or several blended by heading.
///
/// Speed is handled exactly by [`Timing`]; heading is not, because a diagonal
/// tips the chest and mixes the swing, so a blend holds clips authored at
/// several headings. On disk a blend is `[[blend]]` tables, told apart by
/// looking for the key because `#[serde(untagged)]` only reports "did not
/// match any variant".
#[derive(Clone, Debug, PartialEq)]
pub enum StateAnimation {
    Single(Clip),
    /// Kept in heading order.
    Blend(Vec<Variant>),
}

/// One clip of a blend, and the heading it was authored at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    /// Radians from the state's own direction to where the body is going.
    /// Counter-clockwise seen from above is positive.
    #[serde(with = "crate::prop::nice_f32::scalar")]
    pub heading: f32,
    #[serde(flatten)]
    pub clip: Clip,
}

impl From<Clip> for StateAnimation {
    fn from(clip: Clip) -> StateAnimation {
        StateAnimation::Single(clip)
    }
}

#[derive(Serialize, Deserialize)]
struct BlendFile {
    blend: Vec<Variant>,
}

impl StateAnimation {
    pub fn from_toml(text: &str) -> Result<StateAnimation, toml::de::Error> {
        let table: toml::Table = toml::from_str(text)?;
        let mut animation = match table.contains_key("blend") {
            true => StateAnimation::Blend(toml::from_str::<BlendFile>(text)?.blend),
            false => StateAnimation::Single(toml::from_str(text)?),
        };
        animation.normalise();
        Ok(animation)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        match self {
            StateAnimation::Single(clip) => toml::to_string_pretty(clip),
            StateAnimation::Blend(variants) => toml::to_string_pretty(&BlendFile {
                blend: variants.clone(),
            }),
        }
    }

    /// Keys in time order, and variants in heading order.
    pub fn normalise(&mut self) {
        match self {
            StateAnimation::Single(clip) => clip.normalise(),
            StateAnimation::Blend(variants) => {
                for variant in variants.iter_mut() {
                    variant.clip.normalise();
                }
                variants.sort_by(|a, b| a.heading.total_cmp(&b.heading));
            }
        }
    }

    /// The clip a part of this names: the whole thing when it is one clip,
    /// or one variant of a blend.
    pub fn clip(&self, variant: Option<usize>) -> Option<&Clip> {
        match (self, variant) {
            (StateAnimation::Single(clip), None) => Some(clip),
            (StateAnimation::Blend(variants), Some(index)) => {
                variants.get(index).map(|variant| &variant.clip)
            }
            _ => None,
        }
    }

    pub fn clip_mut(&mut self, variant: Option<usize>) -> Option<&mut Clip> {
        match (self, variant) {
            (StateAnimation::Single(clip), None) => Some(clip),
            (StateAnimation::Blend(variants), Some(index)) => {
                variants.get_mut(index).map(|variant| &mut variant.clip)
            }
            _ => None,
        }
    }

    pub fn clips(&self) -> Vec<&Clip> {
        match self {
            StateAnimation::Single(clip) => vec![clip],
            StateAnimation::Blend(variants) => variants.iter().map(|variant| &variant.clip).collect(),
        }
    }

    /// The two clips either side of `heading`, and how far from the first to
    /// the second — or one clip at full weight, outside the range or for a
    /// plain clip. `None` for a blend with nothing in it.
    pub fn at_heading(&self, heading: f32) -> Option<(&Clip, Option<(&Clip, f32)>)> {
        let variants = match self {
            StateAnimation::Single(clip) => return Some((clip, None)),
            StateAnimation::Blend(variants) => variants,
        };
        // Found by looking rather than by position, so a heading edited in
        // the panel blends correctly before anything has re-sorted the list.
        let below = variants
            .iter()
            .filter(|variant| variant.heading <= heading)
            .max_by(|a, b| a.heading.total_cmp(&b.heading));
        let above = variants
            .iter()
            .filter(|variant| variant.heading > heading)
            .min_by(|a, b| a.heading.total_cmp(&b.heading));
        match (below, above) {
            (Some(below), Some(above)) => {
                let span = above.heading - below.heading;
                let t = if span > 0.0 { (heading - below.heading) / span } else { 0.0 };
                Some((&below.clip, Some((&above.clip, t))))
            }
            (Some(only), None) | (None, Some(only)) => Some((&only.clip, None)),
            (None, None) => None,
        }
    }

    /// Every clip's problems, plus whether a blend's variants can be blended.
    pub fn problems(&self, known_bone: impl Fn(&str) -> bool) -> Vec<ClipProblem> {
        let mut problems = Vec::new();
        for clip in self.clips() {
            problems.extend(clip.problems(&known_bone));
        }
        if let StateAnimation::Blend(variants) = self {
            if variants.is_empty() {
                problems.push(ClipProblem {
                    track: None,
                    message: crate::get!("anim.problem.empty_blend"),
                });
            }
            // A joint one variant drives and its neighbour does not blends towards
            // rest, so the body sags between headings.
            for pair in variants.windows(2) {
                let drives = |clip: &Clip| -> Vec<String> {
                    let mut names: Vec<String> = clip
                        .tracks
                        .iter()
                        .map(|track| match &track.channel {
                            Channel::Rotation { bone, .. } => bone.clone(),
                            other => other.kind_key().to_string(),
                        })
                        .collect();
                    names.sort();
                    names
                };
                if drives(&pair[0].clip) != drives(&pair[1].clip) {
                    problems.push(ClipProblem {
                        track: None,
                        message: crate::get!(
                            "anim.problem.unmatched_variants",
                            "from",
                            format!("{:.0}°", pair[0].heading.to_degrees()),
                            "to",
                            format!("{:.0}°", pair[1].heading.to_degrees())
                        ),
                    });
                }
            }
        }
        problems
    }
}

/// The clips a prop or pack has. A `.gpp`'s `[animations]` table and a `.gpa`
/// file are read the same way.
///
/// `BTreeMap` because it is compared for equality to decide whether to
/// resend, and `HashMap` order made equal reads compare unequal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipSet {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub clips: BTreeMap<String, Clip>,
}

impl ClipSet {
    pub fn get(&self, name: &str) -> Option<&Clip> {
        self.clips.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    pub fn normalise(&mut self) {
        for clip in self.clips.values_mut() {
            clip.normalise();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_clip() -> Clip {
        Clip {
            name_key: "anim.test".into(),
            length: 2.0,
            clock: Clock::Seconds,
            looping: false,
            blend_in: None,
            blend_out: None,
            note: String::new(),
            tracks: vec![
                Track {
                    combine: Combine::Replace,
                    space: Space::Body,
                    unit: Unit::LegLengths,
                    enabled: true,
                    timing: Default::default(),
                    channel: Channel::Effector {
                        chain: ChainRef::RightLeg,
                        pole: None,
                        keys: vec![
                            Key {
                                at: 0.0,
                                to: [Axis::Value(0.25), Axis::Ground { clearance: 0.04 }, Axis::Free],
                                ease: Interpolation::SmoothStep,
                            },
                            Key {
                                at: 1.5,
                                to: [Axis::Value(-0.25), Axis::Value(0.1), Axis::Free],
                                ease: Interpolation::Linear,
                            },
                        ],
                    },
                },
                Track {
                    combine: Combine::Compose,
                    space: Space::Carry,
                    unit: Unit::Metres,
                    enabled: true,
                    timing: Default::default(),
                    channel: Channel::Rotation {
                        bone: "chest".into(),
                        keys: vec![Key::all(0.5, [0.05, 0.0, 0.0], Interpolation::CubicOut)],
                    },
                },
            ],
        }
    }

    #[test]
    fn a_clip_round_trips_through_its_own_text() {
        let clip = a_clip();
        let text = toml::to_string_pretty(&clip).expect("a clip should serialise");
        let back: Clip = toml::from_str(&text).expect("a clip should come back");
        assert_eq!(clip, back, "round trip changed the clip:\n{text}");
    }

    /// A derived `Axis` writes tables and widens floats exactly.
    #[test]
    fn an_axis_is_a_bare_number_or_a_bare_word() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Holder {
            to: [Axis; 3],
        }

        let holder = Holder {
            to: [Axis::Value(0.05), Axis::ON_THE_GROUND, Axis::Free],
        };
        let text = toml::to_string(&holder).expect("axes should serialise");

        assert!(text.contains("0.05"), "nice_f32 was bypassed: {text}");
        assert!(
            !text.contains("0.05000000074505806"),
            "the float was widened exactly rather than nicely: {text}"
        );
        assert!(text.contains(r#""ground""#), "ground was not a bare word: {text}");
        assert!(text.contains(r#""free""#), "free was not a bare word: {text}");

        let back: Holder = toml::from_str(&text).expect("axes should come back");
        assert_eq!(holder, back);
    }

    /// TOML reads a hand-typed `0` as an integer, which `visit_i64` must accept.
    #[test]
    fn a_whole_number_axis_is_still_a_number() {
        #[derive(Deserialize)]
        struct Holder {
            to: [Axis; 3],
        }
        let back: Holder = toml::from_str(r#"to = [0, 1, -2]"#).expect("integers are numbers");
        assert_eq!(
            back.to,
            [Axis::Value(0.0), Axis::Value(1.0), Axis::Value(-2.0)]
        );
    }

    /// Postcard refuses `deserialize_any`: encoding succeeds and decoding fails,
    /// so clips must not be sent typed.
    #[test]
    fn a_clip_would_not_survive_the_wire() {
        let clip = a_clip();
        let bytes = postcard::to_allocvec(&clip).expect("encoding is not the half that fails");
        assert!(
            postcard::from_bytes::<Clip>(&bytes).is_err(),
            "a clip decoded from postcard — if this now works, the note about \
             sending clips as text can go, but check it on a real client first"
        );
    }

    /// Keys left out of order are a segment the sampler reads backwards.
    #[test]
    fn normalising_puts_keys_back_in_order() {
        let mut clip = a_clip();
        let Channel::Effector { keys, .. } = &mut clip.tracks[0].channel else {
            panic!("the fixture's first track is an effector");
        };
        keys.swap(0, 1);

        clip.normalise();

        let times = clip.tracks[0].channel.key_times();
        assert!(
            times.windows(2).all(|pair| pair[0] <= pair[1]),
            "keys came out of order: {times:?}"
        );
    }

    #[test]
    fn problems_names_what_is_actually_wrong() {
        let mut clip = a_clip();
        clip.tracks.push(Track::rotation("elbow.q"));
        clip.tracks.push(Track {
            combine: Combine::Replace,
            space: Space::Body,
            unit: Unit::Metres,
            enabled: true,
            timing: Default::default(),
            channel: Channel::Weapon {
                keys: vec![Key {
                    at: 0.5,
                    to: [Axis::ON_THE_GROUND, Axis::Free, Axis::Free],
                    ease: Interpolation::Linear,
                }],
            },
        });

        let known = |bone: &str| bone == "chest";
        let problems = clip.problems(known);

        assert!(
            problems.iter().any(|note| note.message.contains("elbow.q")),
            "a bone the rig has not got went unmentioned: {problems:?}"
        );
        assert!(
            problems.iter().any(|note| note.message.contains("horizontal")),
            "a sideways ground axis went unmentioned: {problems:?}"
        );
    }

    /// A fractional stride count is a limp that looks like a gait bug.
    #[test]
    fn half_a_stride_is_a_problem() {
        let clip = Clip {
            length: 1.5,
            clock: Clock::Stride,
            ..Clip::default()
        };
        assert!(
            clip.problems(|_| true).iter().any(|note| note.message.contains("whole number")),
            "a fractional stride length went unmentioned"
        );
    }

    fn times(channel: &Channel) -> Vec<f32> {
        channel.key_times()
    }

    fn point_track(at: &[f32]) -> Channel {
        Channel::Weapon {
            keys: at
                .iter()
                .map(|at| Key {
                    at: *at,
                    to: [Axis::Value(*at), Axis::Free, Axis::Free],
                    ease: Interpolation::Linear,
                })
                .collect(),
        }
    }

    /// `nudge` must report where a dragged key landed, or the next frame of the
    /// drag moves whichever key inherited its index.
    #[test]
    fn a_key_dragged_across_its_neighbour_is_followed() {
        let mut channel = point_track(&[0.0, 0.25, 0.5]);
        let landed = channel.nudge(&[0], 0.4, 1.0);

        assert_eq!(times(&channel), vec![0.25, 0.4, 0.5]);
        assert_eq!(landed, vec![1], "the moved key was not followed");

        // And it is still the key it was: the value travelled with the time.
        let Channel::Weapon { keys } = &channel else { unreachable!() };
        assert_eq!(keys[1].to[0], Axis::Value(0.0), "a key's value was left behind");
    }

    #[test]
    fn dragging_several_keys_moves_them_as_one() {
        let mut channel = point_track(&[0.0, 0.2, 0.8]);
        channel.nudge(&[0, 1], 0.1, 1.0);
        assert_eq!(times(&channel), vec![0.1, 0.3, 0.8]);
    }

    /// A key outside the clip is held forever and the strip cannot show it.
    #[test]
    fn a_drag_stops_at_the_ends() {
        let mut channel = point_track(&[0.2, 0.5]);
        channel.nudge(&[0, 1], -10.0, 1.0);
        assert_eq!(times(&channel), vec![0.0, 0.0]);

        let mut channel = point_track(&[0.2, 0.5]);
        channel.nudge(&[0, 1], 10.0, 1.0);
        assert_eq!(times(&channel), vec![1.0, 1.0]);
    }

    /// An unstable sort swaps tied keys and inverts a segment's easing.
    #[test]
    fn keys_landing_together_keep_their_order() {
        // Quarters, so the tie is exact; 0.1 + 0.8 is 0.90000004 and would not tie.
        let mut channel = point_track(&[0.25, 0.5]);
        let landed = channel.nudge(&[0], 0.25, 1.0);

        let times = times(&channel);
        assert_eq!(times, vec![0.5, 0.5], "the keys did not land together");

        let Channel::Weapon { keys } = &channel else { unreachable!() };
        assert_eq!(landed, vec![0], "a stable sort should leave the tie as it found it");
        assert_eq!(
            keys[landed[0]].to[0],
            Axis::Value(0.25),
            "the dragged key is not the one reported",
        );
    }

    /// Removed forwards, every index after the first is off by one.
    #[test]
    fn removing_keys_takes_the_ones_asked_for() {
        let mut channel = point_track(&[0.0, 0.1, 0.2, 0.3, 0.4]);
        channel.remove_keys(&[1, 3]);
        assert_eq!(times(&channel), vec![0.0, 0.2, 0.4]);
    }

    /// Stale indices come from a selection a re-sort or undo left behind.
    #[test]
    fn removing_nonsense_indices_does_not_panic() {
        let mut channel = point_track(&[0.0, 0.5]);
        channel.remove_keys(&[1, 1, 9]);
        assert_eq!(times(&channel), vec![0.0]);
    }

    /// Every channel stores the same three optional axes, so keys can be read
    /// without asking which kind a channel is.
    #[test]
    fn every_channel_holds_the_same_kind_of_key() {
        let mut channels = vec![
            Channel::Rotation { bone: "chest".into(), keys: vec![] },
            Channel::WeaponRotation { keys: vec![] },
            Channel::Effector { chain: ChainRef::RightArm, pole: None, keys: vec![] },
            Channel::Weapon { keys: vec![] },
            Channel::Root { keys: vec![] },
        ];

        for channel in &mut channels {
            channel
                .keys_mut()
                .push(Key::all(0.5, [1.0, 2.0, 3.0], Interpolation::Linear));
            assert_eq!(channel.key_count(), 1);
            assert_eq!(channel.keys()[0].to[1], Axis::Value(2.0));
        }

        let angles: Vec<bool> = channels.iter().map(|c| c.is_rotation()).collect();
        assert_eq!(angles, vec![true, true, false, false, false]);
    }

    #[test]
    fn grounding_an_angle_is_a_problem() {
        let clip = Clip {
            length: 1.0,
            tracks: vec![Track {
                channel: Channel::Rotation {
                    bone: "chest".into(),
                    keys: vec![Key {
                        at: 0.0,
                        to: [Axis::ON_THE_GROUND, Axis::Free, Axis::Free],
                        ease: Interpolation::Linear,
                    }],
                },
                ..Track::rotation("chest")
            }],
            ..Clip::default()
        };
        assert!(
            clip.problems(|_| true).iter().any(|note| note.message.contains("no floor")),
            "a grounded angle went unmentioned",
        );
    }

    /// A one-shot that wrapped would snap the weapon back to the hold as a reload
    /// ends.
    #[test]
    fn a_one_shot_holds_where_a_loop_wraps() {
        let looping = Clip { length: 2.0, looping: true, ..Clip::default() };
        let once = Clip { length: 2.0, looping: false, ..Clip::default() };

        assert!((looping.wrap(2.5) - 0.5).abs() < 1e-6);
        assert!((once.wrap(2.5) - 2.5).abs() < 1e-6);
    }
}
