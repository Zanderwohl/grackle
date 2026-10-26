//! Taunts: pack content in `taunts.toml`, each a clip plus what performing it
//! does to the body. Held by the authority and sent to clients like the
//! weapon catalogue, since movement is part of the predicted step and both
//! ends have to agree on it.
//!
//! A bad entry is dropped with a line saying why; a taunt that does not load
//! is one nobody can start, which is something you can see.

use std::collections::BTreeMap;
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
    pub weapon: TauntWeapon,
}

/// Every taunt the packs define, in key order: `G` plays the first until
/// there is a way to choose. A list rather than a map so two loads of the
/// same files compare equal, which is what decides whether it is sent.
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
    weapon: TauntWeapon,
}

/// Lowest priority first; a later pack replaces a taunt's whole entry.
pub fn load_taunts(assets: &Assets, packs: &[PathBuf]) -> Taunts {
    let mut taunts = BTreeMap::new();
    for pack in packs {
        load_pack(assets, pack, &mut taunts);
    }
    Taunts(taunts.into_values().collect())
}

fn load_pack(assets: &Assets, pack: &Path, taunts: &mut BTreeMap<String, Taunt>) {
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

fn merge(named: &str, text: &str, taunts: &mut BTreeMap<String, Taunt>) {
    let file: BTreeMap<String, TauntEntry> = match toml::from_str(text) {
        Ok(file) => file,
        Err(e) => return error!("{named} will not parse: {e}"),
    };
    for (key, entry) in file {
        match check(&key, entry) {
            Ok(taunt) => {
                taunts.insert(key, taunt);
            }
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
    Ok(Taunt {
        id: TauntId::of(key),
        key: key.to_string(),
        clip: entry.clip,
        claims,
        movement: entry.movement,
        weapon: entry.weapon,
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
        let mut taunts = BTreeMap::new();
        merge("test.toml", text, &mut taunts);
        Taunts(taunts.into_values().collect())
    }

    #[test]
    fn the_default_pack_ships_a_wave_that_stops_you() {
        let taunts = Taunts::shipped();
        let wave = taunts.get(TauntId::of("wave")).expect("no wave in the default pack");
        assert_eq!(wave.movement, TauntMovement::Stopped);
        assert_eq!(wave.weapon, TauntWeapon::Hidden);
        assert!(wave.claims.overlaps(Regions::ARM_R));
        assert!(!wave.claims.overlaps(Regions::LEGS), "the wave took the legs from locomotion");
    }

    #[test]
    fn every_movement_is_written_the_way_the_plan_says() {
        let taunts = parse(
            "[a]\nclip = \"x\"\nclaims = []\nmovement = \"free\"\nweapon = \"shown\"\n\
             [b]\nclip = \"x\"\nclaims = [\"legs\"]\nmovement = { slowed = 0.5 }\nweapon = \"hidden\"\n",
        );
        assert_eq!(taunts.0[0].movement, TauntMovement::Free);
        assert_eq!(taunts.0[1].movement, TauntMovement::Slowed(0.5));
        assert_eq!(taunts.0[1].claims, Regions::LEGS);
    }

    #[test]
    fn a_bad_entry_is_dropped_and_the_rest_kept() {
        let taunts = parse(
            "[good]\nclip = \"x\"\nclaims = []\nmovement = \"free\"\nweapon = \"shown\"\n\
             [elbow]\nclip = \"x\"\nclaims = [\"elbow\"]\nmovement = \"free\"\nweapon = \"shown\"\n\
             [fast]\nclip = \"x\"\nclaims = []\nmovement = { slowed = 2.0 }\nweapon = \"shown\"\n",
        );
        let keys: Vec<&str> = taunts.0.iter().map(|taunt| taunt.key.as_str()).collect();
        assert_eq!(keys, ["good"]);
    }
}
