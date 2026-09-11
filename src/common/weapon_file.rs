//! Reading the weapon catalogue off disk.
//!
//! The types here are the *on-disk* shape and
//! [`WeaponCatalogue`](crate::common::weapon::WeaponCatalogue) is what they
//! are converted into. They are deliberately the same shape, in the same way
//! `Outgoing` and `Incoming` are in [`crate::common::map_sync`]: the
//! difference is that a file names a weapon with a string and the catalogue
//! names it with the hash of that string, so the conversion is where ids stop
//! being text.
//!
//! **Only the authority ever reads a file.** A client is told its catalogue by
//! the server and must never disagree — see
//! [`crate::common::weapon_sync`]. What a client loads at startup is a
//! catalogue it then throws away on connect, which is the same arrangement its
//! own `FeatureTimeline` has before it adopts one.
//!
//! Every read goes through [`AssetSource`], so the wasm build has one place to
//! replace rather than a `std::fs` call per loader — and one file per kind of
//! thing rather than a directory, because nothing behind a `fetch` can
//! enumerate a directory and a manifest listing what one holds would be a
//! second description of the pack.
//!
//! **A bad file is not fatal.** A file that will not parse leaves the
//! catalogue as it was and logs which file and why. A single bad entry — a
//! colliding id, a loadout default outside its own permitted list, a loadout
//! naming a weapon nobody has heard of — is dropped and logged, and the rest
//! of the file still loads. The failure then reads as "the Mercenary has no
//! rocket launcher", which is visible in the HUD, rather than as a panic on a
//! listen server with people connected.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

use crate::common::assets::Assets;
use crate::common::class::Class;
use crate::common::weapon::{
    Loadout, Magazine, Mounted, Slot, SlotChoices, Weapon, WeaponCatalogue, WeaponId,
};

pub const WEAPONS_FILE: &str = "weapons.toml";
pub const LOADOUTS_FILE: &str = "loadouts.toml";

/// `weapons.toml`: one entry per weapon, keyed by its id.
///
/// **Ordered maps, and that is load-bearing.** The catalogue is a list because
/// it crosses the wire and is compared for equality to decide whether to send
/// it; filling that list in `HashMap` order would make two reads of the same
/// file compare unequal, often enough to be maddening and rarely enough to
/// ship. A `BTreeMap` walks in the order the ids sort in, which is an order
/// somebody chose.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WeaponFile {
    weapons: BTreeMap<String, WeaponEntry>,
}

/// A weapon as it is written down. [`Weapon`] minus the id, which is the key.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WeaponEntry {
    name_key: String,
    /// The prop under `props/` this weapon is drawn as. Absent for a weapon
    /// nobody has modelled yet.
    ///
    /// **Not checked here.** Weapons load at `Startup` and a prop is read when
    /// something needs to draw one, so verifying the name would mean reading
    /// every model in the pack to load the catalogue. A name that goes
    /// nowhere therefore reads as a weapon that draws nothing, which is what a
    /// weapon with no model does anyway.
    #[serde(default)]
    model: Option<String>,
    primary: Mounted,
    secondary: Mounted,
    magazine: Magazine,
}

/// `loadouts.toml`: one entry per class, keyed by its snake_case name.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoadoutFile {
    loadouts: BTreeMap<String, BTreeMap<Slot, SlotEntry>>,
}

/// A slot as it is written down: weapons named as strings rather than hashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlotEntry {
    permitted: Vec<String>,
    default: String,
}

/// Read the catalogue out of every pack, lowest priority first.
///
/// A pack that has no weapons file is not an error — most packs will be map
/// content — but a pack whose file is there and will not parse is logged
/// loudly, because that is somebody's edit that did not take.
pub fn load_catalogue(assets: &Assets, packs: &[PathBuf]) -> WeaponCatalogue {
    let mut catalogue = WeaponCatalogue::default();

    for pack in packs {
        load_weapons(assets, pack, &mut catalogue);
    }
    // Loadouts second, and in their own pass, because a loadout names weapons
    // and a weapon from a later pack has to be there to be named. A pack that
    // adds a weapon another pack's class may carry is the whole point of a
    // pack list.
    for pack in packs {
        load_loadouts(assets, pack, &mut catalogue);
    }

    catalogue
}

fn read(assets: &Assets, pack: &Path, file: &str) -> Option<String> {
    let bytes = match assets.read(pack, file) {
        Ok(bytes) => bytes,
        // A pack with no weapons is an ordinary pack.
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

        // Two names that hash alike would otherwise let the second quietly
        // replace the first, and the symptom is a weapon that is simply
        // somebody else's. A pack deliberately overriding a weapon writes the
        // same id, which is the same name, so this only fires on a collision.
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

    for (key, slots) in file.loadouts {
        let Some(class) = Class::named(&key) else {
            error!("{}/{LOADOUTS_FILE}: no class called {key}", pack.display());
            continue;
        };

        let mut loadout = Loadout::default();
        for (slot, entry) in slots {
            let Some(choices) = slot_choices(catalogue, &key, slot, &entry, pack) else {
                continue;
            };
            loadout.set(slot, choices);
        }
        catalogue.set_loadout(class, loadout);
    }
}

/// One slot, validated.
///
/// Everything a class may carry has to be a weapon that exists, and what it
/// starts with has to be something it may carry. A default outside the
/// permitted list is a permitted list that means nothing, so the slot is
/// dropped rather than half-believed.
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

/// Fill the catalogue from the packs, once, at startup.
///
/// Unconditional and in every process, including a closed solo one: a body can
/// be handed out the moment a round starts, so the catalogue has to precede
/// it. Not authority-gated either — a process starts `Solo` and may become a
/// host, and `NetRole` is written in exactly one place which is not this one.
pub fn load_catalogue_at_startup(assets: Res<Assets>, mut catalogue: ResMut<WeaponCatalogue>) {
    let loaded = load_catalogue(&assets, &crate::common::assets::default_packs());
    info!(
        "Loaded {} weapons and {} class loadouts",
        loaded.weapons.len(),
        loaded.loadouts.len()
    );
    *catalogue = loaded;
}

/// The catalogue the default packs describe, read through the ordinary asset
/// source.
///
/// What [`load_catalogue_at_startup`] installs, and what a test that needs a
/// body to be armed the way a body in a round is armed should use. There is no
/// second catalogue written in Rust: one that drifted from the files would be
/// a game that plays differently under test than it does in front of you.
pub fn default_catalogue() -> WeaponCatalogue {
    load_catalogue(&Assets::default(), &crate::common::assets::default_packs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon::WeaponAction;

    fn the_default_pack() -> WeaponCatalogue {
        default_catalogue()
    }

    /// The shipped files parse, and everything they say exists does.
    ///
    /// This is the test that fails when somebody hand-edits `weapons.toml` and
    /// gets a field name wrong — which otherwise shows up as a class with
    /// empty hands and no error at all.
    #[test]
    fn the_default_pack_loads() {
        let catalogue = the_default_pack();
        assert!(!catalogue.weapons.is_empty(), "no weapons loaded");
        assert!(
            catalogue.loadout(Class::Mercenary).is_some(),
            "the class everybody plays as carries nothing"
        );
    }

    /// The Mercenary is a sampler: between its buttons it reaches every kind
    /// of action there is, so the whole table can be checked by walking up to
    /// it and clicking. A new variant that nothing carries fails here.
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

        // Every variant of `WeaponAction`, named so that adding one fails here
        // with a list rather than with a count.
        let wanted = [
            WeaponAction::None,
            WeaponAction::Hitscan(crate::common::weapon::HitscanSpec {
                range: 0.0,
                damage: 0,
                headshot_multiplier: 0,
                shove_per_damage: 0.0,
                shove_radius: 0.0,
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

    /// A body spawned as the placeholder class is armed and loaded, which is
    /// what `spawn_player` relies on.
    #[test]
    fn the_placeholder_class_starts_with_full_clips_and_full_reserves() {
        let catalogue = the_default_pack();
        let (equipped, ammo) = catalogue.starting_equipment(Class::Mercenary);

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

    /// Every weapon the default pack defines is reachable by pressing a
    /// digit.
    ///
    /// Nothing switches *within* a slot yet — `permitted` is for a loadout
    /// screen that does not exist — so a weapon sharing a slot with another is
    /// a weapon nobody can get at, and a weapon nobody can get at is a weapon
    /// nobody will notice is broken. Delete this test when there is a way to
    /// choose between a slot's permitted weapons, not before.
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

    /// A class is written in a file the way its variant is, derived from the
    /// variant rather than listed beside it.
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

    /// A default the class may not carry is a permitted list that means
    /// nothing, so the slot goes rather than being half-believed.
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

    /// A weapon nobody defines is dropped from the list rather than carried as
    /// an id that resolves to nothing.
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

    /// Two ids that hash alike are caught here rather than in a fight.
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
