//! Writing weapons back into a pack's `weapons.toml` in place, with `toml_edit`, so the comments
//! that explain each weapon's numbers survive a save.
//!
//! An edited weapon is merged three ways: the file's own tree, the entry the file parsed to, and
//! the entry as edited. Only fields whose value changed are touched, which keeps a field a file
//! left to its serde default out of the file, keeps keys serde does not know (and so ignores),
//! and keeps each value's spacing. A weapon whose entry did not change is not touched at all.

use std::path::Path;

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use crate::common::weapon_file::{parse_weapons, WeaponEntry, WEAPONS_FILE};

/// `weapons` in any order. Refuses a file that does not parse, rather than writing over whatever
/// somebody was halfway through.
pub fn rewrite(original: &str, weapons: &[(String, WeaponEntry)]) -> Result<String, String> {
    let before = match original.trim().is_empty() {
        true => Vec::new(),
        false => parse_weapons(original)?,
    };
    let mut doc: DocumentMut = original.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;

    if !doc.contains_key("weapons") {
        let mut table = Table::new();
        table.set_implicit(true);
        doc.insert("weapons", Item::Table(table));
    }
    let Some(table) = doc.get_mut("weapons").and_then(Item::as_table_like_mut) else {
        return Err(format!("{WEAPONS_FILE}: `weapons` is not a table"));
    };

    for (key, _) in &before {
        if !weapons.iter().any(|(kept, _)| kept == key) {
            table.remove(key);
        }
    }

    for (key, entry) in weapons {
        let after = to_table(entry)?;
        let old = before.iter().find(|(old, _)| old == key).map(|(_, entry)| entry);
        match (old, table.get_mut(key).and_then(Item::as_table_like_mut)) {
            (Some(old), _) if old == entry => {}
            (Some(old), Some(existing)) => merge(existing, &to_table(old)?, &after),
            _ => {
                let mut new = Table::new();
                for (field, value) in &after {
                    new.insert(field, Item::Value(convert(value)));
                }
                new.decor_mut().set_prefix("\n");
                table.insert(key, Item::Table(new));
            }
        }
    }

    Ok(doc.to_string())
}

/// Authoring side, so `std::fs`. A missing file is written from nothing. `expected` is what the
/// draft believes the file holds; a file changed on disk since is refused rather than written
/// over, since `rewrite` would drop what was added and undo what was edited there.
pub fn save_pack(
    pack: &Path,
    weapons: &[(String, WeaponEntry)],
    expected: &[(String, WeaponEntry)],
) -> Result<(), String> {
    let path = pack.join(WEAPONS_FILE);
    let original = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if !holds(&original, expected) {
        return Err(format!("{} changed on disk since it was read; revert to pick it up", path.display()));
    }
    let text = rewrite(&original, weapons).map_err(|e| format!("{}: {e}", path.display()))?;
    if text == original {
        return Ok(());
    }
    std::fs::create_dir_all(pack).map_err(|e| format!("{}: {e}", pack.display()))?;
    // Written beside and renamed over, so a failed write cannot leave half a catalog.
    let partial = path.with_extension("toml.partial");
    std::fs::write(&partial, text).map_err(|e| format!("{}: {e}", partial.display()))?;
    std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))
}

fn holds(text: &str, expected: &[(String, WeaponEntry)]) -> bool {
    let on_disk = match text.trim().is_empty() {
        true => Vec::new(),
        false => match parse_weapons(text) {
            Ok(weapons) => weapons,
            Err(_) => return false,
        },
    };
    on_disk.len() == expected.len() && expected.iter().all(|weapon| on_disk.contains(weapon))
}

/// Renames in `permitted` lists and `default`s only, in order. `None` when nothing in it names one.
pub fn rename_in_loadouts(original: &str, renames: &[(String, String)]) -> Result<Option<String>, String> {
    let mut doc: DocumentMut = original.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let mut touched = false;
    if let Some(loadouts) = doc.get_mut("loadouts").and_then(Item::as_table_like_mut) {
        for (_, class) in loadouts.iter_mut() {
            let Some(class) = class.as_table_like_mut() else { continue };
            for (_, slot) in class.iter_mut() {
                let Some(slot) = slot.as_table_like_mut() else { continue };
                if let Some(permitted) = slot.get_mut("permitted").and_then(Item::as_array_mut) {
                    for name in permitted.iter_mut() {
                        touched |= rename_value(name, renames);
                    }
                }
                if let Some(default) = slot.get_mut("default").and_then(Item::as_value_mut) {
                    touched |= rename_value(default, renames);
                }
            }
        }
    }
    Ok(touched.then(|| doc.to_string()))
}

fn rename_value(value: &mut Value, renames: &[(String, String)]) -> bool {
    let Some(mut name) = value.as_str().map(str::to_owned) else { return false };
    let before = name.clone();
    for (from, to) in renames {
        if &name == from {
            name = to.clone();
        }
    }
    if name == before {
        return false;
    }
    let decor = value.decor().clone();
    *value = Value::from(name);
    *value.decor_mut() = decor;
    true
}

pub fn rename_in_pack(pack: &Path, renames: &[(String, String)]) -> Result<bool, String> {
    let path = pack.join(crate::common::weapon_file::LOADOUTS_FILE);
    let original = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let Some(text) = rename_in_loadouts(&original, renames).map_err(|e| format!("{}: {e}", path.display()))? else {
        return Ok(false);
    };
    let partial = path.with_extension("toml.partial");
    std::fs::write(&partial, text).map_err(|e| format!("{}: {e}", partial.display()))?;
    std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

fn to_table(entry: &WeaponEntry) -> Result<toml::Table, String> {
    match toml::Value::try_from(entry).map_err(|e| e.to_string())? {
        toml::Value::Table(table) => Ok(table),
        _ => Err("a weapon did not serialize as a table".into()),
    }
}

/// `before` is what `old` parsed to, so a key it has and `old` lacks was left to its default.
fn merge(old: &mut dyn TableLike, before: &toml::Table, after: &toml::Table) {
    for (key, new) in after {
        let was = before.get(key);
        if was == Some(new) {
            continue;
        }
        match old.get_mut(key) {
            Some(item) => match (was, new, item.as_table_like_mut()) {
                (Some(toml::Value::Table(was)), toml::Value::Table(new), Some(nested)) => {
                    merge(nested, was, new)
                }
                _ => replace(item, new),
            },
            None => {
                old.insert(key, Item::Value(convert(new)));
            }
        }
    }
    // Only keys serde knows: one it ignores is somebody's, not ours to drop.
    for key in before.keys().filter(|key| !after.contains_key(*key)) {
        old.remove(key);
    }
}

/// Keeps the spacing around the value it replaces.
fn replace(item: &mut Item, new: &toml::Value) {
    match item.as_value_mut() {
        Some(value) => {
            let decor = value.decor().clone();
            *value = convert(new);
            *value.decor_mut() = decor;
        }
        None => *item = Item::Value(convert(new)),
    }
}

/// Nested tables become inline, the shipped style.
fn convert(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::from(s.as_str()),
        toml::Value::Integer(i) => Value::from(*i),
        // Every float in a weapon is an `f32`, widened on the way through serde.
        toml::Value::Float(f) => Value::from(crate::prop::nice_f32::widen(*f as f32)),
        toml::Value::Boolean(b) => Value::from(*b),
        toml::Value::Datetime(d) => Value::from(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(convert).collect::<Array>()),
        toml::Value::Table(table) => {
            let mut inline = InlineTable::new();
            for (key, value) in table {
                inline.insert(key, convert(value));
            }
            Value::InlineTable(inline)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon::WeaponAction;

    const SHIPPED: &str = "assets/default/weapons.toml";

    fn shipped() -> (String, Vec<(String, WeaponEntry)>) {
        let text = std::fs::read_to_string(SHIPPED).expect("the default pack ships weapons");
        let weapons = parse_weapons(&text).expect("the shipped weapons parse");
        (text, weapons)
    }

    fn entry<'a>(weapons: &'a mut [(String, WeaponEntry)], key: &str) -> &'a mut WeaponEntry {
        &mut weapons.iter_mut().find(|(k, _)| k == key).expect("shipped").1
    }

    fn changed_lines(a: &str, b: &str) -> usize {
        let (a, b): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
        assert_eq!(a.len(), b.len(), "lines were added or removed");
        a.iter().zip(&b).filter(|(x, y)| x != y).count()
    }

    #[test]
    fn saving_an_untouched_catalog_changes_no_bytes() {
        let (text, weapons) = shipped();
        assert_eq!(rewrite(&text, &weapons).expect("rewrites"), text);
    }

    /// Fails if a defaulted field is written out, a float is widened, or a whole entry is
    /// re-serialized for one number.
    #[test]
    fn one_edit_changes_one_line() {
        let (text, mut weapons) = shipped();
        let WeaponAction::Projectile(spec) = &mut entry(&mut weapons, "rocket_launcher").primary.action
        else {
            panic!("the rocket launcher fires a projectile");
        };
        spec.damage = 95;
        spec.initial_speed = 21.5;
        let written = rewrite(&text, &weapons).expect("rewrites");

        assert_eq!(changed_lines(&text, &written), 1, "{written}");
        assert!(written.contains("initial_speed = 21.5,"), "the float was not written short");
        assert_eq!(parse_weapons(&written).expect("parses"), weapons);
    }

    #[test]
    fn a_deleted_weapon_takes_its_comment_with_it() {
        let (text, mut weapons) = shipped();
        weapons.retain(|(key, _)| key != "rpg");
        let written = rewrite(&text, &weapons).expect("rewrites");

        assert!(!written.contains("TF2C-style RPG"), "the comment above it was left behind");
        assert!(written.contains("Every weapon in the default pack."), "the file's header went too");
        assert_eq!(parse_weapons(&written).expect("parses"), weapons);
    }

    #[test]
    fn a_new_weapon_is_written_after_the_rest_and_reads_back() {
        let (text, mut weapons) = shipped();
        let mut copy = entry(&mut weapons, "shotgun").clone();
        copy.name_key = "weapon.names.scattergun".into();
        copy.model = None;
        weapons.push(("scattergun".into(), copy));
        let written = rewrite(&text, &weapons).expect("rewrites");

        assert!(written.starts_with(&text), "something above the new weapon changed");
        assert!(written.contains("\n[weapons.scattergun]\n"));
        assert!(written.contains("magazine = { clip = 6,"), "nested tables were not written inline");
        let mut back = parse_weapons(&written).expect("parses");
        back.sort_by(|a, b| a.0.cmp(&b.0));
        weapons.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(back, weapons);
    }

    #[test]
    fn an_empty_file_gets_a_catalog() {
        let (_, weapons) = shipped();
        let written = rewrite("", &weapons[..1]).expect("rewrites");
        assert!(!written.contains("[weapons]\n"), "the parent table was given a header");
        assert_eq!(parse_weapons(&written).expect("parses"), weapons[..1].to_vec());
    }

    #[test]
    fn a_cleared_option_leaves_the_file_and_a_key_serde_ignores_stays() {
        let text = "[weapons.x]\nname_key = \"k\"\nmodel = \"m\"\nnote = \"mine\"\nbucket = 1\n\
                    deploy = 0.5\nmagazine = { reserve = 1, reload = 0.0 }\n\
                    primary = { cadence = { kind = \"semi\" }, cost = { kind = \"free\" }, action = { kind = \"none\" } }\n\
                    secondary = { cadence = { kind = \"semi\" }, cost = { kind = \"free\" }, action = { kind = \"none\" } }\n";
        let mut weapons = parse_weapons(text).expect("parses");
        weapons[0].1.model = None;
        let written = rewrite(text, &weapons).expect("rewrites");
        assert!(!written.contains("model"), "{written}");
        assert!(written.contains("note = \"mine\""), "{written}");
    }

    #[test]
    fn a_file_changed_since_it_was_read_is_noticed() {
        let (text, weapons) = shipped();
        assert!(holds(&text, &weapons));
        assert!(!holds(&text, &weapons[1..]), "a weapon added on disk went unnoticed");
        let mut edited = weapons.clone();
        edited[0].1.deploy += 1.0;
        assert!(!holds(&text, &edited), "a weapon edited on disk went unnoticed");
        assert!(holds("", &[]));
    }

    #[test]
    fn a_rename_reaches_every_slot_and_nothing_else() {
        let text = std::fs::read_to_string("assets/default/loadouts.toml").expect("the default pack ships loadouts");
        let renames = [("pistol".to_owned(), "sidearm".to_owned())];
        let written = rename_in_loadouts(&text, &renames).expect("rewrites").expect("the pistol is named");
        assert!(!written.contains("\"pistol\""), "a pistol was left behind");
        assert_eq!(text.lines().count(), written.lines().count());
        let unrenamed = written.replace("\"sidearm\"", "\"pistol\"");
        assert_eq!(unrenamed, text, "something besides the name changed");
        assert!(rename_in_loadouts(&text, &[("nothing".into(), "x".into())]).expect("reads").is_none());
    }

    #[test]
    fn a_file_that_will_not_parse_is_not_written_over() {
        assert!(rewrite("[weapons.x\n", &[]).is_err());
    }
}
