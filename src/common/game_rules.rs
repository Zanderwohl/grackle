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
use crate::common::weapon::{WeaponCatalog, WeaponId};

const MODES_FILE: &str = "modes.toml";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModeRules {
    pub teams: bool,
    /// In the class picker's order. Empty allows every class.
    pub classes: Vec<Class>,
    /// `None` is each class's own loadout. Each weapon lands in the slot its
    /// class's loadout permits it in; one it permits nowhere is left out.
    pub loadout: Option<Vec<WeaponId>>,
    /// The round ends when somebody reaches it. `None` has no scoreboard.
    pub frag_limit: Option<u32>,
    /// What hurting yourself costs, as a fraction of what it would cost
    /// anybody else. Knockback is not scaled, so a rocket jump costs this and
    /// flies full.
    pub self_damage: f32,
}

impl Default for ModeRules {
    fn default() -> Self {
        Self { teams: true, classes: Vec::new(), loadout: None, frag_limit: None, self_damage: 1.0 }
    }
}

impl ModeRules {
    /// What the class picker offers, in its order.
    pub fn allowed_classes(&self) -> Vec<Class> {
        match self.classes.is_empty() {
            true => Class::iter().collect(),
            false => self.classes.clone(),
        }
    }

    /// The class somebody's next body is: the one they chose if the mode allows it, else the
    /// mode's only class. `None` asks them to choose first.
    pub fn class_for(&self, chosen: Option<Class>) -> Option<Class> {
        let allowed = self.allowed_classes();
        match chosen.filter(|class| allowed.contains(class)) {
            Some(class) => Some(class),
            None if allowed.len() == 1 => allowed.first().copied(),
            None => None,
        }
    }

    /// The allowed classes that start with a weapon under the mode's loadout, or every allowed
    /// class if none does.
    pub fn armed_classes(&self, catalog: &WeaponCatalog) -> Vec<Class> {
        let allowed = self.allowed_classes();
        let armed: Vec<Class> = allowed
            .iter()
            .copied()
            .filter(|class| !catalog.starting_equipment(*class, self.loadout.as_deref()).0.carried().is_empty())
            .collect();
        match armed.is_empty() {
            true => allowed,
            false => armed,
        }
    }

    /// What a spawn's first weapon is judged against, and a bot's class where its seat has none:
    /// the first of [`Self::armed_classes`].
    pub fn class_for_a_bot(&self, catalog: &WeaponCatalog) -> Class {
        self.armed_classes(catalog).first().copied().unwrap_or(Class::Mercenary)
    }

    pub fn random_class_for_a_bot(&self, catalog: &WeaponCatalog, rng: &mut impl rand::Rng) -> Class {
        use rand::seq::IndexedRandom;
        self.armed_classes(catalog).choose(rng).copied().unwrap_or(Class::Mercenary)
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

/// `--class KEY`: the mode allows this class and no other, so nobody is asked. Read only by
/// `publish_game_rules`.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayAs(pub Option<Class>);

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
    #[serde(default = "whole")]
    self_damage: f32,
}

fn yes() -> bool {
    true
}

fn whole() -> f32 {
    1.0
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
            Err(e) => return warn!("{named} is not valid UTF-8, so it is skipped: {e}"),
        },
        Err(AssetError::NotFound(_)) => return,
        Err(e) => return warn!("{e}; skipping it"),
    };
    merge(&named, &text, modes);
}

fn merge(named: &str, text: &str, modes: &mut Modes) {
    let file: toml::Table = match toml::from_str(text) {
        Ok(file) => file,
        Err(e) => return warn!("{named} will not parse, so it is skipped: {e}"),
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
        self_damage: entry.self_damage.max(0.0),
    })
}

/// In every process: a client's is replaced by the server's on connect.
pub fn load_modes_at_startup(assets: Res<Assets>, mut modes: ResMut<Modes>) {
    *modes = load_modes(&assets, &assets.match_packs());
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
    fn the_shipped_arena_has_no_teams_and_offers_the_mercenary_first() {
        let arena = Modes::shipped().of(GameMode::Arena);
        assert!(!arena.teams);
        assert_eq!(arena.classes, vec![Class::Mercenary, Class::Scout, Class::Raptor]);
    }

    /// Breaks if a team mode offers a class it is meant to leave out, or more than ten, which is
    /// more than the picker has digits for.
    #[test]
    fn every_other_shipped_mode_leaves_out_the_spy_the_sniper_and_the_raptor() {
        let modes = Modes::shipped();
        for mode in GameMode::iter().filter(|mode| *mode != GameMode::Arena) {
            let allowed = modes.of(mode).allowed_classes();
            assert!(allowed.len() <= 10, "{mode:?} offers {} classes", allowed.len());
            for left_out in [Class::Spy, Class::Sniper, Class::Raptor] {
                assert!(!allowed.contains(&left_out), "{mode:?} offers the {left_out:?}");
            }
        }
    }

    /// Breaks if a choice the mode no longer allows is kept, or a mode of one class asks anyway.
    #[test]
    fn a_body_is_the_class_chosen_or_the_only_one() {
        let rules = |classes: Vec<Class>| ModeRules { classes, ..default() };
        let two = rules(vec![Class::Scout, Class::Mercenary]);
        assert_eq!(two.class_for(None), None, "nobody was asked");
        assert_eq!(two.class_for(Some(Class::Scout)), Some(Class::Scout));
        assert_eq!(two.class_for(Some(Class::Spy)), None, "a class the mode leaves out was played");
        assert_eq!(rules(vec![Class::Pyro]).class_for(None), Some(Class::Pyro));
        assert_eq!(rules(Vec::new()).allowed_classes().len(), Class::iter().count(), "empty allows every class");
    }

    /// Breaks if a bot is made a class with nothing to fight with when an armed one is allowed.
    #[test]
    fn a_bot_plays_a_class_that_starts_armed() {
        let catalog = default_catalog();
        let rules = ModeRules { classes: vec![Class::Soldier, Class::Mercenary, Class::Pyro], ..default() };
        assert_eq!(rules.class_for_a_bot(&catalog), Class::Mercenary);
        let mut rng = rand::rng();
        for _ in 0..20 {
            assert_eq!(rules.random_class_for_a_bot(&catalog, &mut rng), Class::Mercenary);
        }
    }

    /// Breaks if bots in the arena stop spreading across its classes.
    #[test]
    fn arena_bots_roll_every_class_the_arena_offers() {
        let catalog = default_catalog();
        let arena = Modes::shipped().of(GameMode::Arena);
        let mut rng = rand::rng();
        let rolled: std::collections::HashSet<Class> = (0..200).map(|_| arena.random_class_for_a_bot(&catalog, &mut rng)).collect();
        assert_eq!(rolled.len(), 3, "{rolled:?}");
    }

    /// Breaks if the arena's Scout or Raptor would spawn with nothing in hand.
    #[test]
    fn every_arena_class_starts_with_the_pistol() {
        let catalog = default_catalog();
        let arena = Modes::shipped().of(GameMode::Arena);
        for class in arena.allowed_classes() {
            let (equipped, _) = catalog.starting_equipment(class, arena.loadout.as_deref());
            assert_eq!(equipped.carried(), &[WeaponId::of("pistol")], "{class:?}");
        }
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
             [koth]\nrespawn_waves = true\n",
        );
        assert!(modes.0.is_empty(), "{modes:?}");
    }
}
