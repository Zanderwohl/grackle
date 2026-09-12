//! What an animation *is*, as data.
//!
//! The first thing in the project to have a notion of time in a file, and the
//! answer to the question left open at the bottom of
//! [the weapons plan](../../../documentation/weapons-in-hand.md).
//!
//! # A key holds a target, not a value
//!
//! This is the whole idea, and it is not new — it is the generalisation of two
//! things the rig already does. [`FootOffset`] is a target in **leg-lengths**
//! resolved against the floor by [`plant_feet`]; [`HoldSpec::carry`] is a
//! target in **arm-lengths** resolved against the chest. Both are that way
//! because a Heavy's arms are not a Scout's, so a point in metres is wrong for
//! somebody. A key that stored a *value* would throw that away on the first
//! clip anybody authored.
//!
//! So a key says four things rather than one: what to move, **where**, in
//! which [`Space`], in which [`Unit`] — and it says the position per axis, so
//! "don't hit a height, hit the floor" is one axis set to [`Axis::Ground`] and
//! "relative to the weapon, not the player" is one track set to
//! [`Space::Weapon`]. That is deliberately the shape the map editor's
//! `PointRef` already has: a per-axis absolute-or-relative reference.
//!
//! # A clip is sparse, the way a pose is
//!
//! A clip names the channels it has an opinion about and says nothing about
//! the rest, and a sampled channel with no keys returns `None` rather than a
//! default. That is what makes layering work: a layer that wrote a zero would
//! be a layer that overwrote the one beneath it, and the rule "absence means
//! rest" is already how [`Pose`] is read.
//!
//! # The wire
//!
//! [`Channel`] is internally tagged and [`Axis`] serialises as a bare string
//! or a bare number. **Neither may ever cross the wire as a typed value** —
//! postcard refuses `deserialize_any`, and the failure is on the *receive*
//! side, inside Lightyear, silently, which is the bug `weapon_sync` and
//! `prop::sync` were both written against. It is handled by construction: a
//! `.gpp` already crosses as the text the file holds, so clips in a prop reach
//! clients through machinery that exists, and nothing here is registered as a
//! message. `a_clip_would_not_survive_the_wire` pins that rather than trusting
//! it.
//!
//! [`FootOffset`]: super::gait::FootOffset
//! [`plant_feet`]: super::ik::plant_feet
//! [`HoldSpec::carry`]: crate::prop::hold::HoldSpec::carry
//! [`Pose`]: super::rig::Pose

use std::collections::BTreeMap;
use std::fmt;

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
/// The frames themselves are supplied by the caller rather than looked up
/// here — see `play::Frames`. `common::skeleton` must not reach for a
/// `CollisionWorld` or a `HoldSpec`, because a clip has to be samplable in a
/// unit test with no App, which is the same rule `src/prop/` states as
/// "nothing above `view` touches a `World`".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Space {
    /// The body's own frame: origin at the feet, `+X` its right, `-Z` the way
    /// it faces.
    #[default]
    Body,
    /// The carry pivot — the point between the shoulders, turned by the aim.
    /// What a weapon hangs from.
    Carry,
    /// The weapon's own frame, origin at its grip. A target here stays put
    /// when the weapon moves, which is what makes a hand on a magazine well
    /// stay on it through a recoil.
    Weapon,
    /// The eye, for a viewmodel clip.
    Eye,
}

/// What a number on a key is measured in.
///
/// The parametric half. A clip authored in leg-lengths fits every build; one
/// authored in metres fits the build it was drawn against and nobody else.
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
///
/// The axis nothing in the repo had: a reload runs on seconds, an idle on the
/// shared clock, and a walk on **distance travelled**, which is what
/// [`Gait::phase`] already is. Unifying those is what lets one timeline show
/// every animation.
///
/// [`Gait::phase`]: super::gait::Gait::phase
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Clock {
    /// Seconds since this performance started. A fire, a reload, a deploy.
    #[default]
    Seconds,
    /// The body's stride, wrapping at 1. A walk, a run.
    Stride,
    /// The shared clock plus the body's own phase, wrapping at `length`. An
    /// idle — which is what keeps sixty bodies in an animation grid out of
    /// lockstep without anything storing a per-body time.
    Shared,
}

/// What a track does to the layer beneath it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Combine {
    /// Overwrite whatever was there.
    #[default]
    Replace,
    /// Multiply onto it, in the joint's own frame — the composition
    /// `look_with_the_head` performs onto the pose the animator left. What a
    /// recoil wants: "thirty millimetres back from wherever the hands are".
    Compose,
}

/// Which two-bone chain an effector track drives.
///
/// Named rather than spelled as two bone names, because `ik::LEFT_ARM` and
/// friends are consts and `Pose::set` takes a `&'static str` that cannot be
/// built at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainRef {
    LeftArm,
    RightArm,
    LeftLeg,
    RightLeg,
}

impl ChainRef {
    pub fn chain(self) -> Chain {
        match self {
            ChainRef::LeftArm => ik::LEFT_ARM,
            ChainRef::RightArm => ik::RIGHT_ARM,
            ChainRef::LeftLeg => ik::LEFT_LEG,
            ChainRef::RightLeg => ik::RIGHT_LEG,
        }
    }

    /// Which side of the body, as a multiplier — what an elbow or knee pole is
    /// mirrored by.
    pub fn side(self) -> f32 {
        match self {
            ChainRef::LeftArm | ChainRef::LeftLeg => -1.0,
            ChainRef::RightArm | ChainRef::RightLeg => 1.0,
        }
    }

    pub fn is_leg(self) -> bool {
        matches!(self, ChainRef::LeftLeg | ChainRef::RightLeg)
    }
}

/// One component of a target.
///
/// The centre of the whole design, and the reason a key is not three floats.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Axis {
    /// Leave whatever the layer beneath put here.
    ///
    /// The default, so a track that only cares about height says only that.
    #[default]
    Free,
    /// `clearance` above whatever surface is under the point, measured at
    /// sample time and in the track's own unit.
    ///
    /// The offset is not decoration. A leg chain's effector is the **ankle**,
    /// which is never on the floor — the sole is — so "this foot is planted"
    /// is the ankle at its own resting height above the ground, and a bare
    /// floor height is a target the leg cannot even reach. A hand held five
    /// centimetres off a crate is the same shape with a different number.
    ///
    /// Meaningful on the vertical axis only; [`Clip::problems`] says so rather
    /// than the resolver inventing a meaning for the others.
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

    /// Resting on the surface itself.
    pub const ON_THE_GROUND: Axis = Axis::Ground { clearance: 0.0 };

    pub fn is_ground(self) -> bool {
        matches!(self, Axis::Ground { .. })
    }

    /// The number the panel edits, whichever kind of axis this is.
    pub fn offset(self) -> Option<f32> {
        match self {
            Axis::Value(at) | Axis::Ground { clearance: at } => Some(at),
            Axis::Free => None,
        }
    }
}

/// Hand-written rather than derived, and the reason is the file.
///
/// Derived, `Value(f32)` is a table — `{ value = 0.3 }` — which is unreadable
/// three times over on one line. `#[serde(untagged)]` gives the right shape
/// but routes the float through serde's own widening, which prints
/// `0.05000000074505806` and is the whole reason `nice_f32` exists. So both
/// halves are written out, and a key reads
///
/// ```toml
/// to = [0.0, "ground", "free"]
/// to = [0.2, ["ground", 0.08], "free"]
/// ```
///
/// Three forms because there are three things to say and one of them takes a
/// number: a bare word for "leave it" and "on the floor", a bare number for an
/// ordinary coordinate, and a pair for "this far off the floor". A clearance
/// of nothing writes itself back as the bare word, so the common case stays
/// the short one.
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

            // TOML tells integers and floats apart, so a hand-typed `0` is an
            // integer and would otherwise be rejected as the wrong type — an
            // error about a number that is plainly a number.
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

/// A key on a rotation track.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RotationKey {
    /// Where on the clip's own clock this key sits.
    #[serde(with = "crate::prop::nice_f32::scalar")]
    pub at: f32,
    /// Euler angles in the order [`crate::common::rotation`] owns, which is
    /// the one `HoldSpec::grip_rotation` already uses — so the Hold panel's
    /// `degrees_ui` works on a key without a second spelling of "rotation".
    #[serde(with = "crate::prop::nice_f32::array")]
    pub turn: [f32; 3],
    /// Used **leaving** this key. See [`Interpolation`].
    #[serde(default)]
    pub ease: Interpolation,
}

/// A key on a track that drives a point.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PointKey {
    #[serde(with = "crate::prop::nice_f32::scalar")]
    pub at: f32,
    pub to: [Axis; 3],
    #[serde(default)]
    pub ease: Interpolation,
}

/// What a track drives.
///
/// Internally tagged for the same reason `FeatureOp` is: it is what makes the
/// file readable. See the note on the wire at the top of this module.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "channel", rename_all = "snake_case")]
pub enum Channel {
    /// A bone's rotation in its own rest frame — exactly what a [`Pose`]
    /// stores, so this is the channel with no translation to do at all.
    ///
    /// [`Pose`]: super::rig::Pose
    Rotation {
        bone: String,
        #[serde(default)]
        keys: Vec<RotationKey>,
    },
    /// A two-bone chain solved onto a target through [`ik::solve`].
    Effector {
        chain: ChainRef,
        /// Which way the elbow or knee points. Defaults per chain when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pole: Option<[f32; 3]>,
        #[serde(default)]
        keys: Vec<PointKey>,
    },
    /// Where the weapon itself is, as a deviation from the hold's carry.
    ///
    /// This is what a reload *is*: the weapon dips and the hands follow it,
    /// because `grip_the_weapon` runs afterwards and solves both arms onto
    /// wherever it ended up. No per-hand authoring, and a recoil written once
    /// is correct for a Scout and a Heavy alike.
    Weapon {
        #[serde(default)]
        keys: Vec<PointKey>,
    },
    /// And how it is turned there.
    WeaponRotation {
        #[serde(default)]
        keys: Vec<RotationKey>,
    },
    // Feature { feature: PropFeatureId, .. } — reserved for a magazine that
    // drops out. Building it needs `prop::baked` to bake per-feature groups
    // rather than one mesh per surface, which is a baking change rather than a
    // timeline one.
}

impl Channel {
    /// The lang key for what kind of channel this is.
    pub fn kind_key(&self) -> &'static str {
        match self {
            Channel::Rotation { .. } => "anim.channel.rotation",
            Channel::Effector { .. } => "anim.channel.effector",
            Channel::Weapon { .. } => "anim.channel.weapon",
            Channel::WeaponRotation { .. } => "anim.channel.weapon_rotation",
        }
    }

    /// How many keys it has, whichever kind of key those are.
    pub fn key_count(&self) -> usize {
        match self {
            Channel::Rotation { keys, .. } | Channel::WeaponRotation { keys } => keys.len(),
            Channel::Effector { keys, .. } | Channel::Weapon { keys } => keys.len(),
        }
    }

    /// When each key sits, in order. What the timeline draws.
    pub fn key_times(&self) -> Vec<f32> {
        match self {
            Channel::Rotation { keys, .. } | Channel::WeaponRotation { keys } => {
                keys.iter().map(|key| key.at).collect()
            }
            Channel::Effector { keys, .. } | Channel::Weapon { keys } => {
                keys.iter().map(|key| key.at).collect()
            }
        }
    }

    /// Put the keys back in time order.
    ///
    /// Called at load and after every edit rather than at sample time: sorting
    /// inside the sampler would be a sort per channel per body per frame, and
    /// the sampler is the one thing in here that runs in a match.
    pub fn sort(&mut self) {
        match self {
            Channel::Rotation { keys, .. } | Channel::WeaponRotation { keys } => {
                keys.sort_by(|a, b| a.at.total_cmp(&b.at))
            }
            Channel::Effector { keys, .. } | Channel::Weapon { keys } => {
                keys.sort_by(|a, b| a.at.total_cmp(&b.at))
            }
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
    /// Off for a track being auditioned. Mirrors `PropFeature`'s own flag, and
    /// for the same reason: turning one off to see what it was doing is how
    /// anybody debugs a layered thing.
    #[serde(default = "yes")]
    pub enabled: bool,
    pub channel: Channel,
}

impl Track {
    /// A rotation track on a named bone, with nothing on it yet.
    pub fn rotation(bone: impl Into<String>) -> Track {
        Track {
            combine: Combine::default(),
            space: Space::default(),
            unit: Unit::default(),
            enabled: true,
            channel: Channel::Rotation {
                bone: bone.into(),
                keys: Vec::new(),
            },
        }
    }
}

/// One authored motion.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// The lang key this clip's name is under. Empty for one nobody has named.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name_key: String,
    /// How long, **in this clip's own clock's unit** — seconds, or strides.
    #[serde(default = "one", with = "crate::prop::nice_f32::scalar")]
    pub length: f32,
    #[serde(default)]
    pub clock: Clock,
    #[serde(default)]
    pub looping: bool,
    /// Why the numbers in it are what they are.
    ///
    /// Saved as data rather than left as a comment, because
    /// `toml::to_string_pretty` does not preserve comments and the editor
    /// rewrites the file on save — so an argument written as a comment is an
    /// argument that survives until somebody drags a key. The hand-written
    /// animation functions carry theirs as doc comments on named constants;
    /// this is where those go when one is migrated.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
}

impl Clip {
    /// Put every track's keys back in time order.
    pub fn normalise(&mut self) {
        for track in &mut self.tracks {
            track.channel.sort();
        }
    }

    /// How far through the clip `time` is, in the clip's own units, having
    /// wrapped if it loops.
    ///
    /// A clip that does not loop holds its last key rather than stopping: the
    /// sampler clamps, so a one-shot that has finished keeps the pose it
    /// ended in until something takes it off the stack. That is the right
    /// resting state — the alternative is a weapon that snaps back to the
    /// hold the instant a reload's last key passes.
    pub fn wrap(&self, time: f32) -> f32 {
        if !self.looping || self.length <= 0.0 {
            return time;
        }
        time.rem_euclid(self.length)
    }

    /// What is wrong with this clip, in words, for the Problems panel.
    ///
    /// Nothing here refuses — the rule `prop::feature`'s evaluator already
    /// keeps. A clip with a dangling bone name plays its other tracks, and the
    /// note is what tells you why one hand is not moving. A tool that stopped
    /// at the first bad track would blank the viewport every time somebody
    /// mistyped a name.
    pub fn problems(&self, known_bone: impl Fn(&str) -> bool) -> Vec<String> {
        let mut problems = Vec::new();

        if self.length <= 0.0 {
            problems.push(format!("length is {}, which is no time at all", self.length));
        }

        // A stride is a whole cycle of the gait, so half of one is a clip that
        // lurches every other step — and the symptom is a limp nobody authored.
        if self.clock == Clock::Stride && (self.length - self.length.round()).abs() > 1e-3 {
            problems.push(format!(
                "a stride clip's length should be a whole number of strides, not {}",
                self.length
            ));
        }

        for (index, track) in self.tracks.iter().enumerate() {
            match &track.channel {
                Channel::Rotation { bone, .. } if !known_bone(bone) => {
                    problems.push(format!("track {index} names bone '{bone}', which this rig has not got"));
                }
                Channel::Effector { keys, .. } | Channel::Weapon { keys } => {
                    for key in keys {
                        // Sideways has no floor to find. Rejected rather than
                        // given a meaning, because any meaning invented here
                        // would be one nobody could guess from the panel.
                        if key.to[0].is_ground() || key.to[2].is_ground() {
                            problems.push(format!(
                                "track {index} has a key grounded on a horizontal axis at {}",
                                key.at
                            ));
                        }
                    }
                }
                _ => {}
            }

            for at in track.channel.key_times() {
                if at < 0.0 || at > self.length {
                    problems.push(format!("track {index} has a key at {at}, outside the clip"));
                }
            }
        }

        problems
    }
}

/// Everything a prop, or a pack, has to say about motion.
///
/// One container type so the sampler never sees which home a clip came from —
/// a `.gpp`'s `[animations]` table and a pack's `.gpa` file are the same
/// bytes, read the same way.
///
/// A `BTreeMap` rather than a `HashMap` for the reason `weapon_file` gives:
/// a catalogue compared for equality to decide whether to re-send walks in an
/// order nobody chose otherwise, and two reads of one file then compare
/// unequal often enough to broadcast at frame rate and rarely enough to ship.
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
            note: String::new(),
            tracks: vec![
                Track {
                    combine: Combine::Replace,
                    space: Space::Body,
                    unit: Unit::LegLengths,
                    enabled: true,
                    channel: Channel::Effector {
                        chain: ChainRef::RightLeg,
                        pole: None,
                        keys: vec![
                            PointKey {
                                at: 0.0,
                                to: [Axis::Value(0.25), Axis::Ground { clearance: 0.04 }, Axis::Free],
                                ease: Interpolation::SmoothStep,
                            },
                            PointKey {
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
                    channel: Channel::Rotation {
                        bone: "chest".into(),
                        keys: vec![RotationKey {
                            at: 0.5,
                            turn: [0.05, 0.0, 0.0],
                            ease: Interpolation::CubicOut,
                        }],
                    },
                },
            ],
        }
    }

    /// The format is text so somebody can read it, which is worth nothing if
    /// what comes back is not what went in.
    #[test]
    fn a_clip_round_trips_through_its_own_text() {
        let clip = a_clip();
        let text = toml::to_string_pretty(&clip).expect("a clip should serialise");
        let back: Clip = toml::from_str(&text).expect("a clip should come back");
        assert_eq!(clip, back, "round trip changed the clip:\n{text}");
    }

    /// `Axis` is hand-written on both sides precisely so a file reads
    /// `to = [0.0, "ground", "free"]` rather than three tables — and so the
    /// number goes through `nice_f32` instead of serde's exact widening.
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

    /// A hand-typed `0` is an integer in TOML, not a float. Without
    /// `visit_i64` it is refused, and the error is about a number that is
    /// plainly a number — which is the kind of thing somebody hits once and
    /// then works around by typing `0.0` forever.
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

    /// Postcard refuses `deserialize_any`, which is what an internally tagged
    /// enum and a bare-word-or-number need to read themselves back. Encoding
    /// is *not* the half that fails, which is why this was worth a test: the
    /// send reports success and the receive errors inside Lightyear, on
    /// somebody else's machine.
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

    /// Sorting happens on edit, not in the sampler, so it has to actually
    /// happen. A key dragged past its neighbour and left out of order is a
    /// segment the sampler reads backwards.
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

    /// Nothing refuses — a bad track is a note in a panel, not a blank
    /// viewport. But the notes have to name the real problems.
    #[test]
    fn problems_names_what_is_actually_wrong() {
        let mut clip = a_clip();
        clip.tracks.push(Track::rotation("elbow.q"));
        clip.tracks.push(Track {
            combine: Combine::Replace,
            space: Space::Body,
            unit: Unit::Metres,
            enabled: true,
            channel: Channel::Weapon {
                keys: vec![PointKey {
                    at: 0.5,
                    to: [Axis::ON_THE_GROUND, Axis::Free, Axis::Free],
                    ease: Interpolation::Linear,
                }],
            },
        });

        let known = |bone: &str| bone == "chest";
        let problems = clip.problems(known);

        assert!(
            problems.iter().any(|note| note.contains("elbow.q")),
            "a bone the rig has not got went unmentioned: {problems:?}"
        );
        assert!(
            problems.iter().any(|note| note.contains("horizontal")),
            "a sideways ground axis went unmentioned: {problems:?}"
        );
    }

    /// A stride clip's length is a count of gait cycles. Half of one is a limp
    /// that nobody authored and that looks like a bug in the gait rather than
    /// in the clip.
    #[test]
    fn half_a_stride_is_a_problem() {
        let clip = Clip {
            length: 1.5,
            clock: Clock::Stride,
            ..Clip::default()
        };
        assert!(
            clip.problems(|_| true).iter().any(|note| note.contains("whole number")),
            "a fractional stride length went unmentioned"
        );
    }

    /// A looping clip wraps; a one-shot holds past its end rather than
    /// snapping back, because the alternative is a weapon that returns to the
    /// hold the instant a reload's last key passes.
    #[test]
    fn a_one_shot_holds_where_a_loop_wraps() {
        let looping = Clip { length: 2.0, looping: true, ..Clip::default() };
        let once = Clip { length: 2.0, looping: false, ..Clip::default() };

        assert!((looping.wrap(2.5) - 0.5).abs() < 1e-6);
        assert!((once.wrap(2.5) - 2.5).abs() < 1e-6);
    }
}
