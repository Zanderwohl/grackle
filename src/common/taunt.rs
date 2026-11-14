//! Taunts: pack content in `taunts.toml`. Sent to clients like the weapon
//! catalog, since movement is part of the predicted step and both ends must
//! agree on it. A bad entry is dropped with a line saying why.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::assets::{AssetError, Assets};
use crate::common::skeleton::play::Regions;

pub const TAUNTS_FILE: &str = "taunts.toml";

/// A hash of the taunt's key, so an id does not depend on load order.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Reflect, Serialize, Deserialize,
)]
pub struct TauntId(pub u32);

impl TauntId {
    pub const fn of(key: &str) -> Self {
        Self(crate::common::weapon::WeaponId::of(key).0)
    }
}

/// What a taunt does to the body's own movement.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TauntMovement {
    Free,
    /// No horizontal input; the body still falls.
    Stopped,
    /// Speed multiplied by this.
    Slowed(f32),
}

impl TauntMovement {
    pub fn speed_factor(self) -> f32 {
        match self {
            TauntMovement::Free => 1.0,
            TauntMovement::Stopped => 0.0,
            TauntMovement::Slowed(factor) => factor,
        }
    }
}

/// How far the body may follow the aim while taunting. The view is never
/// held back; this is the body, which "forward" is measured from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TauntTurning {
    Free,
    /// The body keeps the facing it started the taunt with.
    Locked,
    /// Degrees per second, yaw and pitch alike.
    AtMost(f32),
}

impl TauntTurning {
    /// Yaw goes the short way round: both are unwrapped, and the aim may have
    /// spun whole turns while the body was held.
    pub fn follow(self, body: (f32, f32), aim: (f32, f32), dt: f32) -> (f32, f32) {
        let step = match self {
            TauntTurning::Free => return aim,
            TauntTurning::Locked => return body,
            TauntTurning::AtMost(degrees) => degrees.to_radians() * dt,
        };
        let toward = |from: f32, by: f32| from + by.clamp(-step, step);
        let yaw_by = (aim.0 - body.0 + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        // The aim itself, whole turns and all, so a caught-up body equals it.
        if yaw_by.abs() <= step && (aim.1 - body.1).abs() <= step {
            return aim;
        }
        (toward(body.0, yaw_by), toward(body.1, aim.1 - body.1))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TauntWeapon {
    Hidden,
    Shown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Taunt {
    pub id: TauntId,
    pub key: String,
    /// A file under the pack's `animations/`, without its extension.
    pub clip: String,
    /// Region names, resolved against each body's rig as it taunts (`Anatomy::claims`), so one
    /// taunt can claim a strider's tail and a humanoid's arms.
    pub claims: Vec<String>,
    pub movement: TauntMovement,
    pub turning: TauntTurning,
    pub weapon: TauntWeapon,
    /// Seconds until the authority ends it; `None` lasts until `G` again. `length = "clip"` is
    /// resolved where the table is loaded, so a client gets the authority's number.
    pub length: Option<f32>,
}

/// In file order: `G` plays the first. A list rather than a map so two loads
/// of the same files compare equal, which decides whether it is sent.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Taunts(pub Vec<Taunt>);

impl Taunts {
    pub fn get(&self, id: TauntId) -> Option<&Taunt> {
        self.0.iter().find(|taunt| taunt.id == id)
    }

    pub fn first(&self) -> Option<&Taunt> {
        self.0.first()
    }

    pub fn shipped() -> Taunts {
        load_taunts(&Assets::default(), &crate::common::assets::default_packs())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TauntLength {
    /// `length` left out.
    UntilAgain,
    /// `length = "clip"`.
    Clip,
    Seconds(f32),
}

/// A taunt as its file states it, before `length = "clip"` is resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct TauntSpec {
    pub clip: String,
    pub claims: Vec<String>,
    pub movement: TauntMovement,
    pub turning: TauntTurning,
    pub weapon: TauntWeapon,
    pub length: TauntLength,
}

impl TauntSpec {
    pub fn new(clip: String) -> TauntSpec {
        TauntSpec {
            clip,
            claims: Regions::NAMED.iter().map(|(_, name)| name.to_string()).collect(),
            movement: TauntMovement::Stopped,
            turning: TauntTurning::Locked,
            weapon: TauntWeapon::Hidden,
            length: TauntLength::Clip,
        }
    }

    pub fn check(&self) -> Result<(), String> {
        if let TauntMovement::Slowed(factor) = self.movement
            && !(factor.is_finite() && (0.0..=1.0).contains(&factor))
        {
            return Err(format!("a slowed taunt moves at 0 to 1 of normal speed, not {factor}"));
        }
        if let TauntTurning::AtMost(degrees) = self.turning
            && !(degrees.is_finite() && degrees >= 0.0)
        {
            return Err(format!("a taunt turns at some degrees a second, not {degrees}"));
        }
        if let TauntLength::Seconds(length) = self.length
            && !(length.is_finite() && length > 0.0)
        {
            return Err(format!("a taunt lasts some seconds, not {length}"));
        }
        Ok(())
    }

    /// A taunt whose clip is in no pack still applies; under `length = "clip"` it then lasts until
    /// `G` again.
    pub fn resolve(&self, key: &str, clip_length: impl FnOnce(&str) -> Option<f32>) -> Result<Taunt, String> {
        self.check()?;
        let length = match self.length {
            TauntLength::UntilAgain => None,
            TauntLength::Seconds(seconds) => Some(seconds),
            TauntLength::Clip => clip_length(&self.clip).filter(|length| length.is_finite() && *length > 0.0),
        };
        Ok(Taunt {
            id: TauntId::of(key),
            key: key.to_string(),
            clip: self.clip.clone(),
            claims: self.claims.clone(),
            movement: self.movement,
            turning: self.turning,
            weapon: self.weapon,
            length,
        })
    }
}

/// As written on disk: claims by name, so the file reads as body parts.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TauntEntry {
    clip: String,
    claims: Vec<String>,
    movement: TauntMovement,
    turning: TauntTurning,
    weapon: TauntWeapon,
    #[serde(default)]
    length: Option<LengthEntry>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LengthEntry {
    Seconds(f32),
    Word(String),
}

impl TauntEntry {
    fn spec(self) -> Result<TauntSpec, String> {
        let length = match self.length {
            None => TauntLength::UntilAgain,
            Some(LengthEntry::Seconds(seconds)) => TauntLength::Seconds(seconds),
            Some(LengthEntry::Word(word)) if word == "clip" => TauntLength::Clip,
            Some(LengthEntry::Word(word)) => {
                return Err(format!("a length is some seconds or \"clip\", not \"{word}\""));
            }
        };
        let spec = TauntSpec {
            clip: self.clip,
            claims: self.claims,
            movement: self.movement,
            turning: self.turning,
            weapon: self.weapon,
            length,
        };
        spec.check()?;
        Ok(spec)
    }
}

/// Lowest priority first.
pub fn load_taunts(assets: &Assets, packs: &[PathBuf]) -> Taunts {
    let taunts = load_taunt_specs(assets, packs)
        .into_iter()
        .filter_map(|(key, spec)| {
            let clip_length = |clip: &str| {
                let found = crate::common::clip_file::load_named_clip(assets, packs, clip).map(|clip| clip.length);
                if found.is_none() {
                    warn!("{TAUNTS_FILE}: `{key}` lasts as long as `{clip}`, which is in no pack, so until G again");
                }
                found
            };
            spec.resolve(&key, clip_length)
                .map_err(|why| warn!("{TAUNTS_FILE}: `{key}` is dropped: {why}"))
                .ok()
        })
        .collect();
    Taunts(taunts)
}

/// Every pack's entries, merged in file order.
pub fn load_taunt_specs(assets: &Assets, packs: &[PathBuf]) -> Vec<(String, TauntSpec)> {
    let mut specs = Vec::new();
    for pack in packs {
        load_pack(assets, pack, &mut specs);
    }
    specs
}

fn load_pack(assets: &Assets, pack: &Path, specs: &mut Vec<(String, TauntSpec)>) {
    let named = format!("{}/{TAUNTS_FILE}", pack.display());
    let text = match assets.read(pack, TAUNTS_FILE) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => return error!("{named} is not valid UTF-8: {e}"),
        },
        Err(AssetError::NotFound(_)) => return,
        Err(e) => return error!("{e}"),
    };
    merge(&named, &text, specs);
}

pub fn taunt_specs_in(text: &str) -> Vec<(String, TauntSpec)> {
    let mut specs = Vec::new();
    merge(TAUNTS_FILE, text, &mut specs);
    specs
}

/// In file order; a later pack's entry replaces an earlier one where it stood.
fn merge(named: &str, text: &str, specs: &mut Vec<(String, TauntSpec)>) {
    let file: toml::Table = match toml::from_str(text) {
        Ok(file) => file,
        Err(e) => return error!("{named} will not parse: {e}"),
    };
    for (key, value) in file {
        let checked = TauntEntry::deserialize(value)
            .map_err(|e| e.to_string())
            .and_then(TauntEntry::spec);
        match checked {
            Ok(spec) => match specs.iter_mut().find(|(known, _)| *known == key) {
                Some((_, known)) => *known = spec,
                None => specs.push((key, spec)),
            },
            Err(why) => warn!("{named}: `{key}` is dropped: {why}"),
        }
    }
}

/// Edited into `existing` where there is one, so its comments survive.
pub fn write_taunts(existing: Option<&str>, specs: &[(String, TauntSpec)]) -> String {
    use toml_edit::{value, Array, DocumentMut, InlineTable, Item, Table, Value};

    let mut doc: DocumentMut = existing.and_then(|text| text.parse().ok()).unwrap_or_default();
    let stale: Vec<String> = doc
        .iter()
        .map(|(key, _)| key.to_string())
        .filter(|key| !specs.iter().any(|(kept, _)| kept == key))
        .collect();
    for key in stale {
        doc.remove(&key);
    }

    let number = |x: f32| Value::from(crate::prop::nice_f32::widen(x));
    let one = |key: &str, x: f32| {
        let mut table = InlineTable::new();
        table.insert(key, number(x));
        Value::InlineTable(table)
    };
    for (position, (key, spec)) in specs.iter().enumerate() {
        if !doc.get(key).is_some_and(Item::is_table) {
            doc.insert(key, Item::Table(Table::new()));
        }
        let Some(table) = doc.get_mut(key).and_then(Item::as_table_mut) else { continue };
        table.set_position(position);

        let movement = match spec.movement {
            TauntMovement::Free => Value::from("free"),
            TauntMovement::Stopped => Value::from("stopped"),
            TauntMovement::Slowed(factor) => one("slowed", factor),
        };
        let turning = match spec.turning {
            TauntTurning::Free => Value::from("free"),
            TauntTurning::Locked => Value::from("locked"),
            TauntTurning::AtMost(degrees) => one("at_most", degrees),
        };
        let weapon = match spec.weapon {
            TauntWeapon::Hidden => "hidden",
            TauntWeapon::Shown => "shown",
        };
        let fields = [
            ("clip", Some(Value::from(spec.clip.as_str()))),
            ("claims", Some(Value::Array(spec.claims.iter().map(String::as_str).collect::<Array>()))),
            ("movement", Some(movement)),
            ("turning", Some(turning)),
            ("weapon", Some(Value::from(weapon))),
            (
                "length",
                match spec.length {
                    TauntLength::UntilAgain => None,
                    TauntLength::Clip => Some(Value::from("clip")),
                    TauntLength::Seconds(seconds) => Some(number(seconds)),
                },
            ),
        ];
        for (field, wanted) in fields {
            match wanted {
                // Unchanged values are left alone so a trailing comment survives.
                Some(wanted) => {
                    let same = table
                        .get(field)
                        .and_then(Item::as_value)
                        .is_some_and(|had| had.to_string().trim() == wanted.to_string().trim());
                    if !same {
                        table.insert(field, value(wanted));
                    }
                }
                None => {
                    table.remove(field);
                }
            }
        }
    }
    doc.to_string()
}

/// In every process: a client's is replaced by the server's on connect.
pub fn load_taunts_at_startup(assets: Res<Assets>, mut taunts: ResMut<Taunts>) {
    *taunts = load_taunts(&assets, &crate::common::assets::default_packs());
    info!("Loaded {} taunts", taunts.0.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs(text: &str) -> Vec<(String, TauntSpec)> {
        let mut specs = Vec::new();
        merge("test.toml", text, &mut specs);
        specs
    }

    fn parse(text: &str) -> Taunts {
        Taunts(
            specs(text)
                .into_iter()
                .filter_map(|(key, spec)| spec.resolve(&key, |_| Some(1.0)).ok())
                .collect(),
        )
    }

    #[test]
    fn the_first_taunt_is_the_first_in_its_file() {
        let taunts = parse(
            "[wave]\nclip = \"x\"\nclaims = []\nmovement = \"free\"\nturning = \"free\"\nweapon = \"shown\"\n\
             [conga]\nclip = \"y\"\nclaims = []\nmovement = \"free\"\nturning = \"free\"\nweapon = \"shown\"\n",
        );
        assert_eq!(taunts.first().unwrap().key, "wave", "the taunts were sorted by name");
    }

    #[test]
    fn the_default_pack_ships_a_wave_that_stops_you() {
        let taunts = Taunts::shipped();
        let wave = taunts.get(TauntId::of("wave")).expect("no wave in the default pack");
        assert_eq!(wave.movement, TauntMovement::Stopped);
        assert_eq!(wave.weapon, TauntWeapon::Hidden);
        assert!(wave.length.is_some(), "the wave should end by itself");
        let claims = crate::common::skeleton::default_humanoid().anatomy().claims(wave.claims.iter().map(String::as_str));
        assert!(claims.overlaps(Regions::ARM_R));
        assert!(claims.overlaps(Regions::LEGS), "the wave left its legs to locomotion");
    }

    #[test]
    fn every_movement_is_written_the_way_the_plan_says() {
        let taunts = parse(
            "[a]\nclip = \"x\"\nclaims = []\nmovement = \"free\"\nturning = \"free\"\nweapon = \"shown\"\n\
             [b]\nclip = \"x\"\nclaims = [\"legs\"]\nmovement = { slowed = 0.5 }\nturning = { at_most = 90 }\nweapon = \"hidden\"\n",
        );
        assert_eq!(taunts.0[0].movement, TauntMovement::Free);
        assert_eq!(taunts.0[1].movement, TauntMovement::Slowed(0.5));
        assert_eq!(taunts.0[1].claims, ["legs"]);
        assert_eq!(taunts.0[1].turning, TauntTurning::AtMost(90.0));
    }

    /// The yaw goes the short way round even when the aim has spun whole turns.
    #[test]
    fn a_body_follows_the_aim_only_as_far_as_its_taunt_lets_it() {
        let body = (0.0, 0.0);
        assert_eq!(TauntTurning::Free.follow(body, (1.0, 0.5), 0.1), (1.0, 0.5));
        assert_eq!(TauntTurning::Locked.follow(body, (1.0, 0.5), 0.1), body);

        let (yaw, pitch) = TauntTurning::AtMost(90.0).follow(body, (1.0, -0.5), 0.5);
        let step = 45.0_f32.to_radians();
        assert!((yaw - step).abs() < 1e-5, "{yaw}");
        assert!((pitch + 0.5).abs() < 1e-5, "a turn within the rate was held back: {pitch}");

        // Three whole turns and well to the left: a step to the left.
        let spun = -1.5 + 3.0 * std::f32::consts::TAU;
        let (yaw, _) = TauntTurning::AtMost(90.0).follow(body, (spun, 0.0), 0.5);
        assert!((yaw + step).abs() < 1e-4, "the body went the long way round: {yaw}");

        // Within a step it arrives on the aim itself, whole turns and all.
        let near = -0.1 + 3.0 * std::f32::consts::TAU;
        let (yaw, _) = TauntTurning::AtMost(90.0).follow(body, (near, 0.0), 0.5);
        assert_eq!(yaw, near, "an arrived body is a turn off its aim");
    }

    #[test]
    fn the_wave_holds_the_body_still() {
        let taunts = Taunts::shipped();
        assert_eq!(taunts.get(TauntId::of("wave")).unwrap().turning, TauntTurning::Locked);
    }

    #[test]
    fn a_bad_entry_is_dropped_and_the_rest_kept() {
        let taunts = parse(
            "[good]\nclip = \"x\"\nclaims = []\nmovement = \"free\"\nturning = \"free\"\nweapon = \"shown\"\n\
             [fast]\nclip = \"x\"\nclaims = []\nmovement = { slowed = 2.0 }\nturning = \"free\"\nweapon = \"shown\"\n",
        );
        let keys: Vec<&str> = taunts.0.iter().map(|taunt| taunt.key.as_str()).collect();
        assert_eq!(keys, ["good"]);
    }

    /// A claim names a region on some rig: one the humanoid lacks claims nothing on it.
    #[test]
    fn a_claim_on_another_rigs_region_is_kept_and_claims_nothing_here() {
        let taunts = parse(
            "[wag]\nclip = \"x\"\nclaims = [\"tail\", \"head\"]\nmovement = \"free\"\nturning = \"free\"\nweapon = \"shown\"\n",
        );
        assert_eq!(taunts.0[0].claims, ["tail", "head"]);
        let humanoid = crate::common::skeleton::default_humanoid().anatomy();
        assert_eq!(humanoid.claims(taunts.0[0].claims.iter().map(String::as_str)), Regions::HEAD);
    }

    /// Fails if `length = "clip"` does not resolve to the clip's length.
    #[test]
    fn a_taunt_may_last_as_long_as_its_clip() {
        let wave = &Taunts::shipped().0[0];
        let clip = crate::common::clip_file::load_named_clip(
            &Assets::default(),
            &crate::common::assets::default_packs(),
            &wave.clip,
        )
        .unwrap();
        assert_eq!(wave.length, Some(clip.length));

        let packs = crate::common::assets::default_packs();
        for (key, spec) in load_taunt_specs(&Assets::default(), &packs) {
            if spec.length == TauntLength::UntilAgain {
                continue;
            }
            let clip = crate::common::clip_file::load_named_clip(&Assets::default(), &packs, &spec.clip);
            assert!(clip.is_some_and(|clip| !clip.looping), "`{key}` ends, but its clip loops or is missing");
        }

        let [(key, spec)] = &specs(
            "[a]\nclip = \"gone\"\nclaims = []\nmovement = \"free\"\nturning = \"free\"\nweapon = \"shown\"\nlength = \"clip\"\n",
        )[..] else {
            panic!("the entry was not read");
        };
        assert_eq!(spec.length, TauntLength::Clip);
        assert_eq!(spec.resolve(key, |_| None).unwrap().length, None, "a clip in no pack gave a length");
        assert_eq!(spec.resolve(key, |_| Some(2.5)).unwrap().length, Some(2.5));
    }

    /// Fails if writing back an unedited file changes it, or an edit does not read back.
    #[test]
    fn a_save_keeps_the_file_as_it_was_written() {
        let text = std::fs::read_to_string("assets/default/taunts.toml").unwrap();
        let mut read = specs(&text);
        assert_eq!(write_taunts(Some(&text), &read), text);

        read[0].1.movement = TauntMovement::Slowed(0.5);
        read[0].1.turning = TauntTurning::AtMost(90.0);
        read.push(("conga".into(), TauntSpec { length: TauntLength::UntilAgain, ..TauntSpec::new("conga".into()) }));
        let written = write_taunts(Some(&text), &read);
        assert!(written.starts_with("# Each taunt"), "the header comment went:\n{written}");
        assert!(written.contains("movement = { slowed = 0.5 }"), "{written}");
        assert!(written.contains("turning = { at_most = 90.0 }"), "{written}");
        assert_eq!(specs(&written), read, "what was written does not read back:\n{written}");

        read.remove(0);
        let written = write_taunts(Some(&text), &read);
        assert_eq!(specs(&written), read, "{written}");
        assert!(!written.contains("[wave]"), "{written}");
    }
}
