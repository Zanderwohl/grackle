//! Taunts: pack content in `taunts.toml`. Sent to clients like the weapon
//! catalogue, since movement is part of the predicted step and both ends must
//! agree on it. A bad entry is dropped with a line saying why.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::assets::{AssetError, Assets};
use crate::common::skeleton::play::Regions;

const TAUNTS_FILE: &str = "taunts.toml";

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
        let towards = |from: f32, by: f32| from + by.clamp(-step, step);
        let yaw_by = (aim.0 - body.0 + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        // The aim itself, whole turns and all, so a caught-up body equals it.
        if yaw_by.abs() <= step && (aim.1 - body.1).abs() <= step {
            return aim;
        }
        (towards(body.0, yaw_by), towards(body.1, aim.1 - body.1))
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
    pub claims: Regions,
    pub movement: TauntMovement,
    pub turning: TauntTurning,
    pub weapon: TauntWeapon,
    /// Seconds until the authority ends it; `None` lasts until `G` again. Not
    /// read off the clip: a clip is each machine's own.
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
    length: Option<f32>,
}

/// Lowest priority first.
pub fn load_taunts(assets: &Assets, packs: &[PathBuf]) -> Taunts {
    let mut taunts = Vec::new();
    for pack in packs {
        load_pack(assets, pack, &mut taunts);
    }
    Taunts(taunts)
}

fn load_pack(assets: &Assets, pack: &Path, taunts: &mut Vec<Taunt>) {
    let named = format!("{}/{TAUNTS_FILE}", pack.display());
    let text = match assets.read(pack, TAUNTS_FILE) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => return error!("{named} is not valid UTF-8: {e}"),
        },
        Err(AssetError::NotFound(_)) => return,
        Err(e) => return error!("{e}"),
    };
    merge(&named, &text, taunts);
}

/// In file order; a later pack's entry replaces an earlier one where it stood.
fn merge(named: &str, text: &str, taunts: &mut Vec<Taunt>) {
    let file: toml::Table = match toml::from_str(text) {
        Ok(file) => file,
        Err(e) => return error!("{named} will not parse: {e}"),
    };
    for (key, value) in file {
        let checked = TauntEntry::deserialize(value)
            .map_err(|e| e.to_string())
            .and_then(|entry| check(&key, entry));
        match checked {
            Ok(taunt) => match taunts.iter_mut().find(|known| known.key == key) {
                Some(known) => *known = taunt,
                None => taunts.push(taunt),
            },
            Err(why) => warn!("{named}: `{key}` is dropped: {why}"),
        }
    }
}

fn check(key: &str, entry: TauntEntry) -> Result<Taunt, String> {
    let mut claims = Regions::NONE;
    for name in &entry.claims {
        let region = Regions::named(name).ok_or_else(|| format!("there is no body part called `{name}`"))?;
        claims = claims.union(region);
    }
    if let TauntMovement::Slowed(factor) = entry.movement
        && !(factor.is_finite() && (0.0..=1.0).contains(&factor))
    {
        return Err(format!("a slowed taunt moves at 0 to 1 of normal speed, not {factor}"));
    }
    if let TauntTurning::AtMost(degrees) = entry.turning
        && !(degrees.is_finite() && degrees >= 0.0)
    {
        return Err(format!("a taunt turns at some degrees a second, not {degrees}"));
    }
    if let Some(length) = entry.length
        && !(length.is_finite() && length > 0.0)
    {
        return Err(format!("a taunt lasts some seconds, not {length}"));
    }
    Ok(Taunt {
        id: TauntId::of(key),
        key: key.to_string(),
        clip: entry.clip,
        claims,
        movement: entry.movement,
        turning: entry.turning,
        weapon: entry.weapon,
        length: entry.length,
    })
}

/// In every process: a client's is replaced by the server's on connect.
pub fn load_taunts_at_startup(assets: Res<Assets>, mut taunts: ResMut<Taunts>) {
    *taunts = load_taunts(&assets, &crate::common::assets::default_packs());
    info!("Loaded {} taunts", taunts.0.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Taunts {
        let mut taunts = Vec::new();
        merge("test.toml", text, &mut taunts);
        Taunts(taunts)
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
        assert!(wave.claims.overlaps(Regions::ARM_R));
        assert!(wave.claims.overlaps(Regions::LEGS), "the wave left its legs to locomotion");
    }

    #[test]
    fn every_movement_is_written_the_way_the_plan_says() {
        let taunts = parse(
            "[a]\nclip = \"x\"\nclaims = []\nmovement = \"free\"\nturning = \"free\"\nweapon = \"shown\"\n\
             [b]\nclip = \"x\"\nclaims = [\"legs\"]\nmovement = { slowed = 0.5 }\nturning = { at_most = 90 }\nweapon = \"hidden\"\n",
        );
        assert_eq!(taunts.0[0].movement, TauntMovement::Free);
        assert_eq!(taunts.0[1].movement, TauntMovement::Slowed(0.5));
        assert_eq!(taunts.0[1].claims, Regions::LEGS);
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
             [elbow]\nclip = \"x\"\nclaims = [\"elbow\"]\nmovement = \"free\"\nturning = \"free\"\nweapon = \"shown\"\n\
             [fast]\nclip = \"x\"\nclaims = []\nmovement = { slowed = 2.0 }\nturning = \"free\"\nweapon = \"shown\"\n",
        );
        let keys: Vec<&str> = taunts.0.iter().map(|taunt| taunt.key.as_str()).collect();
        assert_eq!(keys, ["good"]);
    }
}
