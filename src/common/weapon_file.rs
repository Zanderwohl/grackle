//! Reading the weapon catalog off disk. These are the on-disk types, which
//! name weapons by string; [`WeaponCatalog`](crate::common::weapon::WeaponCatalog)
//! names them by hash.
//!
//! A client's startup catalog is replaced by the server's on connect (see
//! [`crate::common::pack_sync`]).
//!
//! Each weapon is its own file, `weapons/{key}.toml` in a pack, its key the
//! file's stem; the loadouts are one `loadouts.toml` per pack. Reads go
//! through [`Assets`] so wasm has one thing to replace.
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
    Loadout, Magazine, Mounted, Slot, SlotChoices, Weapon, WeaponCatalog, WeaponId, BUCKETS,
};

pub const WEAPONS_DIR: &str = "weapons";
pub const WEAPON_EXTENSION: &str = "toml";
pub const LOADOUTS_FILE: &str = "loadouts.toml";

pub fn is_weapon_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

pub fn weapon_file(key: &str) -> String {
    format!("{WEAPONS_DIR}/{key}.{WEAPON_EXTENSION}")
}

pub fn weapon_path(pack: &Path, key: &str) -> PathBuf {
    pack.join(weapon_file(key))
}

/// [`Weapon`] minus the id and key, which is the file's stem. Public for the
/// weapon editor, which edits these and writes them back.
///
/// Field order is the order a new entry is written in, which is the shipped
/// file's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeaponEntry {
    pub name_key: String,
    /// Not checked here: that would mean reading every model in the pack at
    /// startup. A bad name draws nothing.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    pub bucket: u8,
    pub deploy: f32,
    pub magazine: Magazine,
    pub primary: Mounted,
    pub secondary: Mounted,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoped_primary: Option<Mounted>,
}

impl WeaponEntry {
    pub fn into_weapon(self, key: &str) -> Weapon {
        Weapon {
            id: WeaponId::of(key),
            key: key.to_string(),
            name_key: self.name_key,
            model: self.model,
            icon: self.icon,
            bucket: self.bucket,
            deploy: self.deploy,
            primary: self.primary,
            secondary: self.secondary,
            scoped_primary: self.scoped_primary,
            magazine: self.magazine,
        }
    }
}

/// A weapon as a pack file states it, before it has met the other packs.
#[derive(Debug, Clone, PartialEq)]
pub struct PackWeapon {
    pub pack: PathBuf,
    pub key: String,
    pub entry: WeaponEntry,
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
    #[serde(default)]
    health: Option<u32>,
    #[serde(default)]
    armor: Option<u32>,
    /// Not checked here: a taunt with no clip plays nothing.
    #[serde(default)]
    picker_taunt: Option<String>,
    #[serde(flatten)]
    slots: BTreeMap<Slot, SlotEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlotEntry {
    permitted: Vec<String>,
    /// Absent starts the slot empty.
    #[serde(default)]
    default: Option<String>,
}

/// Lowest priority first. A missing file is normal; one that will not parse
/// is logged.
pub fn load_catalog(assets: &Assets, packs: &[PathBuf]) -> WeaponCatalog {
    let catalog = catalog_of(assets, packs, &read_pack_weapons(assets, packs));
    for weapon in &catalog.weapons {
        if let Some(icon) = weapon.icon.as_deref().filter(|icon| !icon_exists(assets, icon)) {
            warn!("{}: no pack has icons/{icon}.svg; its kills draw the skull", weapon.key);
        }
    }
    catalog
}

/// Not a [`problem`]: a weapon with a missing icon still plays, and its kills draw the skull.
pub fn icon_exists(assets: &Assets, icon: &str) -> bool {
    assets.read_from_packs(&format!("icons/{icon}.svg")).is_some()
}

/// Every pack's weapons as written, lowest priority first and in key order
/// within a pack. Key order, not listing order: the catalog is compared for
/// equality to decide whether to send it.
pub fn read_pack_weapons(assets: &Assets, packs: &[PathBuf]) -> Vec<PackWeapon> {
    let mut weapons = Vec::new();
    for pack in packs {
        for key in assets.stems(pack, WEAPONS_DIR, WEAPON_EXTENSION) {
            if !is_weapon_key(&key) {
                warn!(
                    "{}: a weapon's file is named by its key, lowercase letters, digits and underscores; ignoring it",
                    weapon_path(pack, &key).display()
                );
                continue;
            }
            if let Some(weapon) = read_pack_weapon(assets, pack, &key) {
                weapons.push(weapon);
            }
        }
    }
    weapons
}

/// `None`, logged, when the file is missing or will not parse.
pub fn read_pack_weapon(assets: &Assets, pack: &Path, key: &str) -> Option<PackWeapon> {
    let file = weapon_file(key);
    let text = read(assets, pack, &file)?;
    match parse_weapon(&text) {
        Ok(entry) => Some(PackWeapon { pack: pack.to_path_buf(), key: key.to_owned(), entry }),
        Err(e) => {
            warn!("{}/{file} will not parse: {e}", pack.display());
            None
        }
    }
}

pub fn parse_weapon(text: &str) -> Result<WeaponEntry, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

/// Every pack's loadouts as written, read once so a catalog can be rebuilt
/// from them without touching the disk.
#[derive(Debug, Clone, Default)]
pub struct LoadoutFiles(Vec<(PathBuf, LoadoutFile)>);

impl LoadoutFiles {
    /// Each class slot that may carry or starts with `key`.
    pub fn naming(&self, key: &str) -> Vec<(PathBuf, String, Slot)> {
        let mut found = Vec::new();
        for (pack, file) in &self.0 {
            for (class, entry) in &file.loadouts {
                for (slot, choices) in &entry.slots {
                    if choices.permitted.iter().any(|name| name == key) || choices.default.as_deref() == Some(key) {
                        found.push((pack.clone(), class.clone(), *slot));
                    }
                }
            }
        }
        found
    }

    /// As these files would read with each `(from, to)` weapon renamed, in order.
    pub fn renamed(&self, renames: &[(String, String)]) -> LoadoutFiles {
        let mut files = self.clone();
        for (_, file) in &mut files.0 {
            for entry in file.loadouts.values_mut() {
                for choices in entry.slots.values_mut() {
                    for (from, to) in renames {
                        for name in choices.permitted.iter_mut().chain(choices.default.iter_mut()) {
                            if name == from {
                                *name = to.clone();
                            }
                        }
                    }
                }
            }
        }
        files
    }
}

pub fn read_loadouts(assets: &Assets, packs: &[PathBuf]) -> LoadoutFiles {
    let mut files = Vec::new();
    for pack in packs {
        let Some(text) = read(assets, pack, LOADOUTS_FILE) else { continue };
        match toml::from_str(&text) {
            Ok(file) => files.push((pack.clone(), file)),
            Err(e) => warn!("{}/{LOADOUTS_FILE} will not parse: {e}", pack.display()),
        }
    }
    LoadoutFiles(files)
}

/// The catalog these weapons make, with every pack's loadouts read against
/// them. A weapon with a [`WeaponProblem`] is left out, with a line saying why.
pub fn catalog_of(assets: &Assets, packs: &[PathBuf], weapons: &[PackWeapon]) -> WeaponCatalog {
    catalog_with(weapons, &read_loadouts(assets, packs))
}

pub fn catalog_with(weapons: &[PackWeapon], loadouts: &LoadoutFiles) -> WeaponCatalog {
    let mut catalog = WeaponCatalog::default();
    for weapon in weapons {
        match problem(&catalog, weapon) {
            Some(why) => warn!("{}: {why}; ignoring it", weapon_path(&weapon.pack, &weapon.key).display()),
            None => catalog.insert(weapon.entry.clone().into_weapon(&weapon.key)),
        }
    }
    // A separate pass, so a loadout can name a weapon from a later pack.
    for (pack, file) in &loadouts.0 {
        apply_loadouts(pack, file, &mut catalog);
    }
    catalog
}

/// Why a weapon is left out of the catalog.
#[derive(Debug, Clone, PartialEq)]
pub enum WeaponProblem {
    Collides { key: String, with: String },
    Bucket { key: String, bucket: u8 },
    Deploy { key: String, deploy: f32 },
    ScopedPrimary { key: String },
}

/// English, for the log.
impl std::fmt::Display for WeaponProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WeaponProblem::Collides { key, with } => write!(f, "{key} collides with {with}, already loaded"),
            WeaponProblem::Bucket { key, bucket } => {
                write!(f, "{key} is in bucket {bucket}, which no key selects (1–{BUCKETS})")
            }
            WeaponProblem::Deploy { key, deploy } => write!(f, "{key} deploys in {deploy} s"),
            WeaponProblem::ScopedPrimary { key } => {
                write!(f, "{key} has a primary while sighted but no scope on its secondary")
            }
        }
    }
}

impl WeaponProblem {
    /// Through the lang layer, for the weapon editor.
    pub fn describe(&self) -> String {
        match self {
            WeaponProblem::Collides { key, with } => {
                crate::get!("weapon_editor.problems.collides", "key", key, "with", with)
            }
            WeaponProblem::Bucket { key, bucket } => {
                crate::get!("weapon_editor.problems.bucket", "key", key, "bucket", bucket, "most", BUCKETS)
            }
            WeaponProblem::Deploy { key, deploy } => {
                crate::get!("weapon_editor.problems.deploy", "key", key, "deploy", deploy)
            }
            WeaponProblem::ScopedPrimary { key } => crate::get!("weapon_editor.problems.scoped_primary", "key", key),
        }
    }
}

/// Why each weapon would be left out of the catalog, in the order given.
/// What the loader logs and what the weapon editor lists are this.
pub fn problems(weapons: &[PackWeapon]) -> Vec<(usize, WeaponProblem)> {
    let mut catalog = WeaponCatalog::default();
    let mut found = Vec::new();
    for (index, weapon) in weapons.iter().enumerate() {
        match problem(&catalog, weapon) {
            Some(why) => found.push((index, why)),
            None => catalog.insert(weapon.entry.clone().into_weapon(&weapon.key)),
        }
    }
    found
}

/// Against the weapons accepted so far.
fn problem(catalog: &WeaponCatalog, weapon: &PackWeapon) -> Option<WeaponProblem> {
    let PackWeapon { key, entry, .. } = weapon;
    // A hash collision, not an override: an override reuses the name and
    // so the `name_key` would normally match.
    if let Some(already) = catalog.get(WeaponId::of(key))
        && already.name_key != entry.name_key
    {
        return Some(WeaponProblem::Collides { key: key.clone(), with: already.key.clone() });
    }
    if !(1..=BUCKETS).contains(&entry.bucket) {
        return Some(WeaponProblem::Bucket { key: key.clone(), bucket: entry.bucket });
    }
    if !(entry.deploy >= 0.0) {
        return Some(WeaponProblem::Deploy { key: key.clone(), deploy: entry.deploy });
    }
    if entry.scoped_primary.is_some() && !matches!(entry.secondary.action, crate::common::weapon::WeaponAction::Scope(_)) {
        return Some(WeaponProblem::ScopedPrimary { key: key.clone() });
    }
    None
}

fn read(assets: &Assets, pack: &Path, file: &str) -> Option<String> {
    let bytes = match assets.read(pack, file) {
        Ok(bytes) => bytes,
        Err(crate::common::assets::AssetError::NotFound(_)) => return None,
        Err(e) => {
            warn!("{e}; skipping it");
            return None;
        }
    };
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(e) => {
            warn!("{}/{file} is not valid UTF-8: {e}", pack.display());
            None
        }
    }
}

fn apply_loadouts(pack: &Path, file: &LoadoutFile, catalog: &mut WeaponCatalog) {
    for (key, entry) in &file.loadouts {
        let Some(class) = Class::named(key) else {
            warn!("{}/{LOADOUTS_FILE}: no class called {key}", pack.display());
            continue;
        };

        let mut loadout = Loadout {
            hat: entry.hat.clone(),
            health: entry.health.filter(|health| *health > 0),
            armor: entry.armor,
            picker_taunt: entry.picker_taunt.clone(),
            ..default()
        };
        for (slot, entry) in &entry.slots {
            let Some(choices) = slot_choices(catalog, key, *slot, entry, pack) else {
                continue;
            };
            loadout.set(*slot, choices);
        }
        catalog.set_loadout(class, loadout);
    }
}

/// Unknown permitted weapons are dropped; a default outside the permitted list
/// drops the whole slot.
fn slot_choices(
    catalog: &WeaponCatalog,
    class: &str,
    slot: Slot,
    entry: &SlotEntry,
    pack: &Path,
) -> Option<SlotChoices> {
    let mut permitted = Vec::with_capacity(entry.permitted.len());
    for name in &entry.permitted {
        let id = WeaponId::of(name);
        if catalog.get(id).is_none() {
            warn!(
                "{}/{LOADOUTS_FILE}: {class}'s {slot:?} may carry {name}, which no pack defines",
                pack.display()
            );
            continue;
        }
        permitted.push(id);
    }

    let default = entry.default.as_deref().map(WeaponId::of);
    if let (Some(name), Some(id)) = (&entry.default, default)
        && !permitted.contains(&id)
    {
        warn!(
            "{}/{LOADOUTS_FILE}: {class}'s {slot:?} starts with {name}, which it may not carry; \
             dropping the slot",
            pack.display(),
        );
        return None;
    }

    Some(SlotChoices { permitted, default })
}

/// In every process and not authority-gated: a process starts `Solo` and may
/// become a host, and a body can be handed out as soon as a round starts.
pub fn load_catalog_at_startup(assets: Res<Assets>, mut catalog: ResMut<WeaponCatalog>) {
    let loaded = load_catalog(&assets, &crate::common::assets::default_packs());
    info!(
        "Loaded {} weapons and {} class loadouts",
        loaded.weapons.len(),
        loaded.loadouts.len()
    );
    *catalog = loaded;
}

/// What tests should use. There is deliberately no catalog written in
/// Rust, which could drift from the files.
pub fn default_catalog() -> WeaponCatalog {
    load_catalog(&Assets::default(), &crate::common::assets::default_packs())
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;
    use crate::common::weapon::WeaponAction;

    fn the_default_pack() -> WeaponCatalog {
        default_catalog()
    }

    /// Fails on a bad hand edit to the shipped files, which otherwise shows up
    /// only as empty hands.
    #[test]
    fn the_default_pack_loads() {
        let catalog = the_default_pack();
        assert!(!catalog.weapons.is_empty(), "no weapons loaded");
        assert!(
            catalog.loadout(Class::Mercenary).is_some(),
            "the class everybody plays as carries nothing"
        );
    }

    /// Fails if a class states no health of its own, and so silently gets the default.
    #[test]
    fn every_class_states_its_health() {
        use strum::IntoEnumIterator;
        let catalog = the_default_pack();
        let missing: Vec<Class> =
            Class::iter().filter(|class| catalog.loadout(*class).and_then(|loadout| loadout.health).is_none()).collect();
        assert!(missing.is_empty(), "{missing:?} state no health in loadouts.toml");
        assert_eq!(catalog.health_of(Class::Heavy), 300);
    }

    /// Fails when a `WeaponAction` variant is carried by nothing the
    /// placeholder class has.
    #[test]
    fn the_placeholder_class_carries_one_of_everything() {
        let catalog = the_default_pack();
        let loadout = catalog.loadout(Class::Mercenary).expect("no placeholder loadout");

        let mut seen: Vec<std::mem::Discriminant<WeaponAction>> = Vec::new();
        for (_, choices) in &loadout.slots {
            for id in &choices.permitted {
                let weapon = catalog.get(*id).expect("a permitted weapon is missing");
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
                pellets: None,
                kill_shove: None,
                ramp_up: None,
                falloff: None,
                trail: crate::common::weapon::HitscanTrail::Streak,
            }),
            WeaponAction::Projectile(crate::common::projectile::ProjectileSpec::ROCKET),
            WeaponAction::Flame(crate::common::flame::FlameSpec::FLAMETHROWER),
            WeaponAction::Airblast(crate::common::weapon::AirblastSpec {
                range: 0.0,
                spread: 0.0,
                push: 0.0,
                lift: 0.0,
                reflect: false,
                reflect_speed: 1.0,
                extinguish: false,
            }),
            WeaponAction::HealBeam(crate::common::weapon::HealBeamSpec {
                range: 0.0,
                heal_per_second: 0,
            }),
            WeaponAction::Invulnerable(crate::common::weapon::InvulnSpec {
                duration: 0.0,
                cooldown: 0.0,
            }),
            WeaponAction::Scope(crate::common::weapon::ScopeSpec::default()),
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
        let catalog = the_default_pack();
        let (equipped, ammo) = catalog.starting_equipment(Class::Mercenary, None);

        for &id in equipped.carried() {
            let weapon = catalog.get(id).expect("carrying a weapon nobody defines");
            let state = ammo.at(id).expect("carrying a weapon with no ammo");
            assert_eq!(state.clip, weapon.magazine.clip.unwrap_or(0));
            assert_eq!(state.reserve, weapon.magazine.reserve);
            assert_eq!(state.reloading, 0.0, "{} spawned mid-reload", weapon.key);
        }
        assert_eq!(ammo.weapons.len(), equipped.carried().len(), "ammo for a weapon it does not carry");
        assert!(!equipped.carried().is_empty(), "the placeholder class spawned empty-handed");
        assert!(equipped.held().is_some(), "it spawned holding nothing while carrying something");
    }

    #[test]
    fn a_modes_loadout_carries_only_what_it_names() {
        let catalog = the_default_pack();
        let only = [WeaponId::of("medi_wrench"), WeaponId::of("nothing_called_this")];
        let (equipped, ammo) = catalog.starting_equipment(Class::Mercenary, Some(&only));

        assert_eq!(equipped.carried(), [only[0]]);
        assert_eq!(equipped.held(), Some(only[0]));
        assert_eq!(ammo.weapons.len(), 1, "ammo for a weapon it does not carry");
    }

    /// Fails if a shipped weapon is permitted in no slot, so no mode's
    /// loadout can start a body with it. A pickup can still hand one out, but
    /// only on a map that places it.
    #[test]
    fn every_shipped_weapon_is_on_a_digit() {
        let catalog = the_default_pack();
        let loadout = catalog.loadout(Class::Mercenary).expect("no placeholder loadout");

        for weapon in &catalog.weapons {
            assert!(
                loadout.slots.iter().any(|(_, choices)| choices.permitted.contains(&weapon.id)),
                "{} is defined but no slot permits it, so no loadout can start with it",
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
        let catalog = the_default_pack();
        let entry = SlotEntry {
            permitted: vec!["sniper_rifle".into()],
            default: Some("rocket_launcher".into()),
        };

        assert!(
            slot_choices(&catalog, "mercenary", Slot::Primary, &entry, Path::new("test"))
                .is_none()
        );
    }

    #[test]
    fn a_permitted_weapon_that_does_not_exist_is_dropped() {
        let catalog = the_default_pack();
        let entry = SlotEntry {
            permitted: vec!["sniper_rifle".into(), "no_such_weapon".into()],
            default: Some("sniper_rifle".into()),
        };

        let choices =
            slot_choices(&catalog, "mercenary", Slot::Primary, &entry, Path::new("test"))
                .expect("the slot was dropped for one bad entry");
        assert_eq!(choices.permitted, vec![WeaponId::of("sniper_rifle")]);
    }

    /// Fails on a hash collision between shipped weapon names, or a shipped file that will not
    /// parse.
    #[test]
    fn no_two_shipped_weapons_share_an_id() {
        let catalog = the_default_pack();
        let files = Assets::default().stems(Path::new("assets/default"), WEAPONS_DIR, WEAPON_EXTENSION);

        assert_eq!(
            catalog.weapons.len(),
            files.len(),
            "a weapon file is missing from the catalog, which is a collision or a parse error"
        );
    }

    /// Fails if a weapon a map or the loadouts name by key stops loading under that key.
    #[test]
    fn every_weapon_keeps_the_key_maps_name_it_by() {
        let catalog = the_default_pack();
        for key in
            ["sniper_rifle", "rocket_launcher", "rpg", "pill_launcher", "pistol", "shotgun", "flamethrower", "medi_wrench"]
        {
            assert!(catalog.get(WeaponId::of(key)).is_some(), "no weapon is called {key}");
        }
    }

    #[test]
    fn a_later_packs_file_retunes_the_weapon() {
        use crate::common::music::tests::MemoryFiles;
        let shipped = std::fs::read_to_string("assets/default/weapons/pistol.toml").unwrap();
        let retuned = shipped.replace("deploy = 0.35", "deploy = 2.0");
        let assets = Assets(Box::new(MemoryFiles(
            [
                ("low/weapons/pistol.toml".to_owned(), shipped.into_bytes()),
                ("high/weapons/pistol.toml".to_owned(), retuned.into_bytes()),
                ("high/weapons/notes.txt".to_owned(), b"not a weapon".to_vec()),
            ]
            .into(),
        )));
        let packs = [PathBuf::from("low"), PathBuf::from("high")];
        let weapons = read_pack_weapons(&assets, &packs);
        assert_eq!(weapons.len(), 2);
        let catalog = catalog_with(&weapons, &LoadoutFiles::default());
        assert_eq!(catalog.get(WeaponId::of("pistol")).unwrap().deploy, 2.0);
    }

    /// Fails if a shipped projectile that bounces has a trail, or the rocket launcher or the RPG
    /// loses its trail.
    #[test]
    fn only_projectiles_that_never_bounce_trail() {
        use crate::common::projectile::Trail;
        let catalog = the_default_pack();
        let mut trailed = vec![];
        for weapon in &catalog.weapons {
            for mounted in [&weapon.primary, &weapon.secondary] {
                let WeaponAction::Projectile(spec) = &mounted.action else { continue };
                if spec.trail != Trail::None {
                    assert_eq!(spec.max_bounces, 0, "{} bounces and trails", weapon.key);
                    trailed.push(weapon.key.as_str());
                }
            }
        }
        trailed.sort();
        assert_eq!(trailed, ["rocket_launcher", "rpg"]);
    }
}
