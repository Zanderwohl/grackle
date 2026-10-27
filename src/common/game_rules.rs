//! What a game mode changes, from a pack's `modes.toml`. See
//! [documentation/arena.md](../../documentation/arena.md).
//!
//! [`Modes`] is the table, read by the authority and sent to clients like the
//! weapons. [`GameRules`] is the entry for the map's mode, written only by
//! [`publish_game_rules`](crate::editor::map_metadata::publish_game_rules); a
//! mode with no entry plays by [`ModeRules::default`], which is how every mode
//! played before there were rules.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use crate::common::assets::{AssetError, Assets};
use crate::common::class::Class;
use crate::common::mode::GameMode;
use crate::common::weapon::WeaponId;

const MODES_FILE: &str = "modes.toml";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModeRules {
    pub teams: bool,
    /// Empty allows every class.
    pub classes: Vec<Class>,
    /// `None` is each class's own loadout. Each weapon lands in the slot its
    /// class's loadout permits it in; one it permits nowhere is left out.
    pub loadout: Option<Vec<WeaponId>>,
    /// The round ends when somebody reaches it. `None` has no scoreboard.
    pub frag_limit: Option<u32>,
}

impl Default for ModeRules {
    fn default() -> Self {
        Self { teams: true, classes: Vec::new(), loadout: None, frag_limit: None }
    }
}

impl ModeRules {
    /// The class a new body is, until classes can be picked: the first the mode
    /// allows, else the Mercenary placeholder.
    pub fn class_to_play(&self) -> Class {
        self.classes.first().copied().unwrap_or(Class::Mercenary)
    }
}

/// Keyed by [`GameMode::prefix`]. A `BTreeMap` so two loads of the same files
/// compare equal, which decides whether the table is sent.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Modes(pub BTreeMap<String, ModeRules>);

impl Modes {
    pub fn of(&self, mode: GameMode) -> ModeRules {
        self.0.get(mode.prefix()).cloned().unwrap_or_default()
    }

    pub fn shipped() -> Self {
        load_modes(&Assets::default(), &crate::common::assets::default_packs())
    }
}

/// The rules of the mode the open map is played in.
#[derive(Resource, Clone, Debug, Default, PartialEq, Deref)]
pub struct GameRules(pub ModeRules);

/// As written on disk: classes and weapons by name. A rule the game does not
/// have yet is refused rather than read as nothing.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeEntry {
    #[serde(default = "yes")]
    teams: bool,
    #[serde(default)]
    classes: Vec<String>,
    #[serde(default)]
    loadout: Option<Vec<String>>,
    #[serde(default)]
    frag_limit: Option<u32>,
}

fn yes() -> bool {
    true
}

/// Lowest priority first; a later pack replaces a mode's whole entry.
pub fn load_modes(assets: &Assets, packs: &[PathBuf]) -> Modes {
    let mut modes = Modes::default();
    for pack in packs {
        load_pack(assets, pack, &mut modes);
    }
    modes
}

fn load_pack(assets: &Assets, pack: &Path, modes: &mut Modes) {
    let named = format!("{}/{MODES_FILE}", pack.display());
    let text = match assets.read(pack, MODES_FILE) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => return error!("{named} is not valid UTF-8: {e}"),
        },
        Err(AssetError::NotFound(_)) => return,
        Err(e) => return error!("{e}"),
    };
    merge(&named, &text, modes);
}

fn merge(named: &str, text: &str, modes: &mut Modes) {
    let file: toml::Table = match toml::from_str(text) {
        Ok(file) => file,
        Err(e) => return error!("{named} will not parse: {e}"),
    };
    for (key, value) in file {
        let checked = ModeEntry::deserialize(value)
            .map_err(|e| e.to_string())
            .and_then(|entry| check(&key, entry));
        match checked {
            Ok(rules) => {
                modes.0.insert(key, rules);
            }
            Err(why) => warn!("{named}: `{key}` is dropped: {why}"),
        }
    }
}

fn check(key: &str, entry: ModeEntry) -> Result<ModeRules, String> {
    if !GameMode::iter().any(|mode| mode.prefix() == key) {
        return Err("there is no game mode by that name".into());
    }
    let classes = entry
        .classes
        .iter()
        .map(|name| Class::named(name).ok_or_else(|| format!("there is no class called `{name}`")))
        .collect::<Result<_, _>>()?;
    Ok(ModeRules {
        teams: entry.teams,
        classes,
        loadout: entry.loadout.map(|names| names.iter().map(|name| WeaponId::of(name)).collect()),
        frag_limit: entry.frag_limit.filter(|limit| *limit > 0),
    })
}

/// In every process: a client's is replaced by the server's on connect.
pub fn load_modes_at_startup(assets: Res<Assets>, mut modes: ResMut<Modes>) {
    *modes = load_modes(&assets, &crate::common::assets::default_packs());
    info!("Loaded rules for {} game modes", modes.0.len());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon_file::default_catalog;

    fn parse(text: &str) -> Modes {
        let mut modes = Modes::default();
        merge("test.toml", text, &mut modes);
        modes
    }

    #[test]
    fn a_mode_with_no_entry_plays_as_before() {
        let modes = parse("[arena]\nteams = false\n");
        assert_eq!(modes.of(GameMode::CTF), ModeRules::default());
        assert!(ModeRules::default().teams);
    }

    #[test]
    fn the_shipped_arena_has_no_teams_and_only_the_mercenary() {
        let arena = Modes::shipped().of(GameMode::Arena);
        assert!(!arena.teams);
        assert_eq!(arena.classes, vec![Class::Mercenary]);
    }

    /// A misspelled weapon would spawn everybody short of it, silently.
    #[test]
    fn every_shipped_loadout_names_weapons_that_exist() {
        let catalog = default_catalog();
        for (mode, rules) in Modes::shipped().0 {
            for id in rules.loadout.iter().flatten() {
                assert!(catalog.get(*id).is_some(), "`{mode}` names a weapon nobody defines");
            }
        }
    }

    #[test]
    fn unknown_modes_classes_and_rules_are_dropped() {
        let modes = parse(
            "[deathball]\nteams = false\n\
             [ctf]\nclasses = [\"wizard\"]\n\
             [koth]\nself_damage = 0.5\n",
        );
        assert!(modes.0.is_empty(), "{modes:?}");
    }
}
