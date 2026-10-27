//! Reading the weapon catalogue off disk. These are the on-disk types, which
//! name weapons by string; [`WeaponCatalogue`](crate::common::weapon::WeaponCatalogue)
//! names them by hash.
//!
//! A client's startup catalogue is replaced by the server's on connect (see
//! [`crate::common::pack_sync`]).
//!
//! Reads go through [`AssetSource`] so wasm has one thing to replace, and are
//! one file per kind of thing because nothing behind a `fetch` can list a
//! directory.
//!
//! A bad file or entry is logged and skipped, never fatal, so the failure is
//! a visibly missing weapon rather than a panic on a listen server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::common::assets::Assets;
use crate::common::class::Class;
use crate::common::weapon::{
    Loadout, Magazine, Mounted, Slot, SlotChoices, Weapon, WeaponCatalogue, WeaponId,
};

pub const WEAPONS_FILE: &str = "weapons.toml";
pub const LOADOUTS_FILE: &str = "loadouts.toml";

/// `BTreeMap`, not `HashMap`: the catalogue is filled in this order and
/// compared for equality to decide whether to send it, and hash order made two
/// reads of one file compare unequal.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WeaponFile {
    weapons: BTreeMap<String, WeaponEntry>,
}

/// [`Weapon`] minus the id, which is the key.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WeaponEntry {
    name_key: String,
    /// Not checked here: that would mean reading every model in the pack at
    /// startup. A bad name draws nothing.
    #[serde(default)]
    model: Option<String>,
    primary: Mounted,
    secondary: Mounted,
    magazine: Magazine,
}

/// Keyed by [`Class::key`].
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoadoutFile {
    loadouts: BTreeMap<String, LoadoutEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoadoutEntry {
    /// Not checked here, as a weapon's model is not: a bad name draws nothing.
    #[serde(default)]
    hat: Option<String>,
    #[serde(flatten)]
    slots: BTreeMap<Slot, SlotEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlotEntry {
    permitted: Vec<String>,
    default: String,
}

/// Lowest priority first. A missing file is normal; one that will not parse
/// is logged.
pub fn load_catalogue(assets: &Assets, packs: &[PathBuf]) -> WeaponCatalogue {
    let mut catalogue = WeaponCatalogue::default();

    for pack in packs {
        load_weapons(assets, pack, &mut catalogue);
    }
    // A separate pass, so a loadout can name a weapon from a later pack.
    for pack in packs {
        load_loadouts(assets, pack, &mut catalogue);
    }

    catalogue
}

fn read(assets: &Assets, pack: &Path, file: &str) -> Option<String> {
    let bytes = match assets.read(pack, file) {
        Ok(bytes) => bytes,
        Err(crate::common::assets::AssetError::NotFound(_)) => return None,
        Err(e) => {
            error!("{e}");
            return None;
        }
    };
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(e) => {
            error!("{}/{file} is not valid UTF-8: {e}", pack.display());
            None
        }
    }
}

fn load_weapons(assets: &Assets, pack: &Path, catalogue: &mut WeaponCatalogue) {
    let Some(text) = read(assets, pack, WEAPONS_FILE) else { return };

    let file: WeaponFile = match toml::from_str(&text) {
        Ok(file) => file,
        Err(e) => {
            error!("{}/{WEAPONS_FILE} will not parse: {e}", pack.display());
            return;
        }
    };

    for (name, entry) in file.weapons {
        let id = WeaponId::of(&name);

        // A hash collision, not an override: an override reuses the name and
        // so the `name_key` would normally match.
        if let Some(already) = catalogue.get(id) {
            if already.name_key != entry.name_key {
                error!(
                    "{}/{WEAPONS_FILE}: {name} collides with a weapon already loaded; ignoring it",
                    pack.display()
                );
                continue;
            }
        }

        catalogue.insert(Weapon {
            id,
            name_key: entry.name_key,
            model: entry.model,
            primary: entry.primary,
            secondary: entry.secondary,
            magazine: entry.magazine,
        });
    }
}

fn load_loadouts(assets: &Assets, pack: &Path, catalogue: &mut WeaponCatalogue) {
    let Some(text) = read(assets, pack, LOADOUTS_FILE) else { return };

    let file: LoadoutFile = match toml::from_str(&text) {
        Ok(file) => file,
        Err(e) => {
            error!("{}/{LOADOUTS_FILE} will not parse: {e}", pack.display());
            return;
        }
    };

    for (key, entry) in file.loadouts {
        let Some(class) = Class::named(&key) else {
            error!("{}/{LOADOUTS_FILE}: no class called {key}", pack.display());
            continue;
        };

        let mut loadout = Loadout { hat: entry.hat, ..default() };
        for (slot, entry) in entry.slots {
            let Some(choices) = slot_choices(catalogue, &key, slot, &entry, pack) else {
                continue;
            };
            loadout.set(slot, choices);
        }
        catalogue.set_loadout(class, loadout);
    }
}

/// Unknown permitted weapons are dropped; a default outside the permitted list
/// drops the whole slot.
fn slot_choices(
    catalogue: &WeaponCatalogue,
    class: &str,
    slot: Slot,
    entry: &SlotEntry,
    pack: &Path,
) -> Option<SlotChoices> {
    let mut permitted = Vec::with_capacity(entry.permitted.len());
    for name in &entry.permitted {
        let id = WeaponId::of(name);
        if catalogue.get(id).is_none() {
            error!(
                "{}/{LOADOUTS_FILE}: {class}'s {slot:?} may carry {name}, which no pack defines",
                pack.display()
            );
            continue;
        }
        permitted.push(id);
    }

    let default = WeaponId::of(&entry.default);
    if !permitted.contains(&default) {
        error!(
            "{}/{LOADOUTS_FILE}: {class}'s {slot:?} starts with {}, which it may not carry; \
             dropping the slot",
            pack.display(),
            entry.default
        );
        return None;
    }

    Some(SlotChoices { permitted, default })
}

/// In every process and not authority-gated: a process starts `Solo` and may
/// become a host, and a body can be handed out as soon as a round starts.
pub fn load_catalogue_at_startup(assets: Res<Assets>, mut catalogue: ResMut<WeaponCatalogue>) {
    let loaded = load_catalogue(&assets, &crate::common::assets::default_packs());
    info!(
        "Loaded {} weapons and {} class loadouts",
        loaded.weapons.len(),
        loaded.loadouts.len()
    );
    *catalogue = loaded;
}

/// What tests should use. There is deliberately no catalogue written in
/// Rust, which could drift from the files.
pub fn default_catalogue() -> WeaponCatalogue {
    load_catalogue(&Assets::default(), &crate::common::assets::default_packs())
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;
    use crate::common::weapon::WeaponAction;

    fn the_default_pack() -> WeaponCatalogue {
        default_catalogue()
    }

    /// Fails on a bad hand edit to the shipped files, which otherwise shows up
    /// only as empty hands.
    #[test]
    fn the_default_pack_loads() {
        let catalogue = the_default_pack();
        assert!(!catalogue.weapons.is_empty(), "no weapons loaded");
        assert!(
            catalogue.loadout(Class::Mercenary).is_some(),
            "the class everybody plays as carries nothing"
        );
    }

    /// Fails when a `WeaponAction` variant is carried by nothing the
    /// placeholder class has.
    #[test]
    fn the_placeholder_class_carries_one_of_everything() {
        let catalogue = the_default_pack();
        let loadout = catalogue.loadout(Class::Mercenary).expect("no placeholder loadout");

        let mut seen: Vec<std::mem::Discriminant<WeaponAction>> = Vec::new();
        for (_, choices) in &loadout.slots {
            for id in &choices.permitted {
                let weapon = catalogue.get(*id).expect("a permitted weapon is missing");
                for mounted in [weapon.primary, weapon.secondary] {
                    let kind = std::mem::discriminant(&mounted.action);
                    if !seen.contains(&kind) {
                        seen.push(kind);
                    }
                }
            }
        }

        let wanted = [
            WeaponAction::None,
            WeaponAction::Hitscan(crate::common::weapon::HitscanSpec {
                range_m: 0.0,
                damage: 0,
                headshot_multiplier: 0,
                shove_per_damage: 0.0,
                shove_radius_m: 0.0,
            }),
            WeaponAction::Projectile(crate::common::projectile::ProjectileSpec::ROCKET),
            WeaponAction::Flame(crate::common::flame::FlameSpec::FLAMETHROWER),
            WeaponAction::Extinguish(crate::common::weapon::ExtinguishSpec {
                range: 0.0,
                spread: 0.0,
            }),
            WeaponAction::HealBeam(crate::common::weapon::HealBeamSpec {
                range: 0.0,
                heal_per_second: 0,
            }),
            WeaponAction::Invulnerable(crate::common::weapon::InvulnSpec {
                duration: 0.0,
                cooldown: 0.0,
            }),
            WeaponAction::Scope(crate::common::weapon::ScopeSpec { zoom: 0.0 }),
        ];
        for action in wanted {
            assert!(
                seen.contains(&std::mem::discriminant(&action)),
                "nothing the placeholder class carries does {action:?}"
            );
        }
    }

    #[test]
    fn the_placeholder_class_starts_with_full_clips_and_full_reserves() {
        let catalogue = the_default_pack();
        let (equipped, ammo) = catalogue.starting_equipment(Class::Mercenary, None);

        let mut carried = 0;
        for slot in Slot::ALL {
            let Some(id) = equipped.get(slot) else {
                assert_eq!(*ammo.at(slot), Default::default(), "{slot:?} has ammo for no weapon");
                continue;
            };
            carried += 1;
            let weapon = catalogue.get(id).expect("carrying a weapon nobody defines");
            assert_eq!(ammo.at(slot).clip, weapon.magazine.clip.unwrap_or(0));
            assert_eq!(ammo.at(slot).reserve, weapon.magazine.reserve);
            assert_eq!(ammo.at(slot).reloading, 0.0, "{slot:?} spawned mid-reload");
        }
        assert!(carried > 0, "the placeholder class spawned empty-handed");
        assert!(equipped.held().is_some(), "it spawned holding nothing while carrying something");
    }

    #[test]
    fn a_modes_loadout_carries_only_what_it_names() {
        let catalogue = the_default_pack();
        let only = [WeaponId::of("medi_wrench"), WeaponId::of("nothing_called_this")];
        let (equipped, ammo) = catalogue.starting_equipment(Class::Mercenary, Some(&only));

        let carried: Vec<WeaponId> = Slot::ALL.into_iter().filter_map(|slot| equipped.get(slot)).collect();
        assert_eq!(carried, vec![only[0]]);
        assert_eq!(equipped.held(), Some(only[0]));
        assert_eq!(*ammo.at(Slot::Primary), Default::default(), "ammo for a weapon it does not carry");
    }

    /// Fails if a shipped weapon is no slot's default. Nothing switches within
    /// a slot yet, so such a weapon is unreachable. Delete once something
    /// does.
    #[test]
    fn every_shipped_weapon_is_on_a_digit() {
        let catalogue = the_default_pack();
        let loadout = catalogue.loadout(Class::Mercenary).expect("no placeholder loadout");

        for weapon in &catalogue.weapons {
            assert!(
                loadout.slots.iter().any(|(_, choices)| choices.default == weapon.id),
                "{} is defined but no slot starts with it, so nobody can hold it",
                weapon.name_key
            );
        }
    }

    #[test]
    fn every_class_has_a_key_and_it_is_its_own() {
        let mut keys: Vec<String> = Vec::new();
        for class in Class::iter() {
            let key = Class::key(class);
            assert_eq!(Class::named(&key), Some(class), "{key} did not come back as {class:?}");
            assert!(!keys.contains(&key), "two classes are written the same way: {key}");
            keys.push(key);
        }
        assert_eq!(Class::named("no_such_class"), None);
    }

    #[test]
    fn a_default_outside_its_permitted_list_is_rejected() {
        let catalogue = the_default_pack();
        let entry = SlotEntry {
            permitted: vec!["laser_rifle".into()],
            default: "rocket_launcher".into(),
        };

        assert!(
            slot_choices(&catalogue, "mercenary", Slot::Primary, &entry, Path::new("test"))
                .is_none()
        );
    }

    #[test]
    fn a_permitted_weapon_that_does_not_exist_is_dropped() {
        let catalogue = the_default_pack();
        let entry = SlotEntry {
            permitted: vec!["laser_rifle".into(), "no_such_weapon".into()],
            default: "laser_rifle".into(),
        };

        let choices =
            slot_choices(&catalogue, "mercenary", Slot::Primary, &entry, Path::new("test"))
                .expect("the slot was dropped for one bad entry");
        assert_eq!(choices.permitted, vec![WeaponId::of("laser_rifle")]);
    }

    /// Fails on a hash collision between shipped weapon names.
    #[test]
    fn no_two_shipped_weapons_share_an_id() {
        let catalogue = the_default_pack();
        let text = std::fs::read_to_string("assets/default/weapons.toml").unwrap();
        let file: WeaponFile = toml::from_str(&text).unwrap();

        assert_eq!(
            catalogue.weapons.len(),
            file.weapons.len(),
            "a weapon in the file is missing from the catalogue, which is a collision"
        );
    }
}
