//! Writing a weapon back into its own file, `weapons/{key}.toml`, in place with `toml_edit`, so the
//! comments that explain its numbers survive a save.
//!
//! An edited weapon is merged three ways: the file's own tree, the entry the file parsed to, and
//! the entry as edited. Only fields whose value changed are touched, which keeps a field a file
//! left to its serde default out of the file, keeps keys serde does not know (and so ignores),
//! and keeps each value's spacing. A file whose entry did not change is not touched at all.

use std::path::Path;

use toml_edit::{Array, DocumentMut, InlineTable, Item, TableLike, Value};

use crate::common::weapon_file::{parse_weapon, weapon_path, WeaponEntry};

/// `original` as it would read holding `entry`. Refuses a file that does not parse, rather than
/// writing over whatever somebody was halfway through.
pub fn rewrite(original: &str, entry: &WeaponEntry) -> Result<String, String> {
    let before = parsed(original)?;
    if before.as_ref() == Some(entry) {
        return Ok(original.to_owned());
    }
    let mut doc: DocumentMut = original.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let after = to_table(entry)?;
    match &before {
        Some(before) => merge(doc.as_table_mut(), &to_table(before)?, &after),
        None => {
            for (field, value) in &after {
                doc.insert(field, Item::Value(convert(value)));
            }
        }
    }
    Ok(doc.to_string())
}

/// `None` for an empty file.
fn parsed(text: &str) -> Result<Option<WeaponEntry>, String> {
    match text.trim().is_empty() {
        true => Ok(None),
        false => parse_weapon(text).map(Some),
    }
}

fn read_or_empty(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// A file changed on disk since the draft read it is refused rather than written over, since the
/// merge would drop what was added and undo what was edited there.
fn check_unchanged(path: &Path, text: &str, expected: Option<&WeaponEntry>) -> Result<(), String> {
    match parsed(text) {
        Ok(on_disk) if on_disk.as_ref() == expected => Ok(()),
        _ => Err(format!("{} changed on disk since it was read; revert to pick it up", path.display())),
    }
}

/// Authoring side, so `std::fs`. `expected` is what the draft believes the file holds, `None` for
/// a file it believes is not there.
///
/// `from` is a file this one was renamed from, `(key, expected, delete)`: its text is the starting
/// point, so its comments carry over, and with `delete` it is deleted once the new file is
/// written. The inner result is that deletion's: the new file is written either way.
pub fn save_weapon(
    pack: &Path,
    key: &str,
    entry: &WeaponEntry,
    expected: Option<&WeaponEntry>,
    from: Option<(&str, &WeaponEntry, bool)>,
) -> Result<Result<(), String>, String> {
    let path = weapon_path(pack, key);
    let original = read_or_empty(&path)?;
    check_unchanged(&path, &original, expected)?;
    let (start, old) = match from {
        Some((old_key, old_expected, delete)) => {
            let old = weapon_path(pack, old_key);
            let text = read_or_empty(&old)?;
            check_unchanged(&old, &text, Some(old_expected))?;
            (text, delete.then_some(old))
        }
        None => (original.clone(), None),
    };
    let text = rewrite(&start, entry).map_err(|e| format!("{}: {e}", path.display()))?;
    if text != original {
        let folder = path.parent().unwrap_or(pack);
        std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        // Written beside and renamed over, so a failed write cannot leave half a weapon.
        let partial = path.with_extension("toml.partial");
        std::fs::write(&partial, text).map_err(|e| format!("{}: {e}", partial.display()))?;
        std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(match old {
        Some(old) => std::fs::remove_file(&old).map_err(|e| format!("{}: {e}", old.display())),
        None => Ok(()),
    })
}

/// What `key`'s file in `pack` holds now; `None` when it is missing, empty or will not parse.
pub fn on_disk(pack: &Path, key: &str) -> Option<WeaponEntry> {
    parsed(&read_or_empty(&weapon_path(pack, key)).ok()?).ok().flatten()
}

/// Refused, as a save is, when the file no longer holds `expected`.
pub fn delete_weapon(pack: &Path, key: &str, expected: &WeaponEntry) -> Result<(), String> {
    let path = weapon_path(pack, key);
    let text = read_or_empty(&path)?;
    if text.is_empty() {
        return Ok(());
    }
    check_unchanged(&path, &text, Some(expected))?;
    crate::common::trash::trash(crate::common::recent_files::DocumentKind::Weapon, &path).map(|_| ())
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

    fn shipped(key: &str) -> (String, WeaponEntry) {
        let text = std::fs::read_to_string(format!("assets/default/weapons/{key}.toml")).expect("the default pack ships it");
        let entry = parse_weapon(&text).expect("the shipped weapon parses");
        (text, entry)
    }

    fn changed_lines(a: &str, b: &str) -> usize {
        let (a, b): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
        assert_eq!(a.len(), b.len(), "lines were added or removed");
        a.iter().zip(&b).filter(|(x, y)| x != y).count()
    }

    #[test]
    fn saving_an_untouched_weapon_changes_no_bytes() {
        for key in crate::common::assets::Assets::default().stems(
            Path::new("assets/default"),
            crate::common::weapon_file::WEAPONS_DIR,
            crate::common::weapon_file::WEAPON_EXTENSION,
        ) {
            let (text, entry) = shipped(&key);
            assert_eq!(rewrite(&text, &entry).expect("rewrites"), text, "{key}");
            // Through the merge too, not only the shortcut for an unchanged entry.
            let mut doc: DocumentMut = text.parse().unwrap();
            let table = to_table(&entry).unwrap();
            merge(doc.as_table_mut(), &table, &table);
            assert_eq!(doc.to_string(), text, "{key}");
        }
    }

    /// Fails if a defaulted field is written out, a float is widened, or a whole entry is
    /// re-serialized for one number.
    #[test]
    fn one_edit_changes_one_line() {
        let (text, mut entry) = shipped("rocket_launcher");
        let WeaponAction::Projectile(spec) = &mut entry.primary.action else {
            panic!("the rocket launcher fires a projectile");
        };
        spec.damage = 95;
        spec.initial_speed = 21.5;
        let written = rewrite(&text, &entry).expect("rewrites");

        assert_eq!(changed_lines(&text, &written), 1, "{written}");
        assert!(written.contains("initial_speed = 21.5,"), "the float was not written short");
        assert!(written.starts_with("# TF2's"), "the comment at the top went");
        assert_eq!(parse_weapon(&written).expect("parses"), entry);
    }

    #[test]
    fn an_empty_file_gets_a_weapon() {
        let (_, entry) = shipped("shotgun");
        let written = rewrite("", &entry).expect("rewrites");
        assert!(written.starts_with("name_key = "), "{written}");
        assert!(written.contains("magazine = { clip = 6,"), "nested tables were not written inline");
        assert_eq!(parse_weapon(&written).expect("parses"), entry);
    }

    #[test]
    fn a_cleared_option_leaves_the_file_and_a_key_serde_ignores_stays() {
        let text = "name_key = \"k\"\nmodel = \"m\"\nnote = \"mine\"\nbucket = 1\n\
                    deploy = 0.5\nmagazine = { reserve = 1, reload = 0.0 }\n\
                    primary = { cadence = { kind = \"semi\" }, cost = { kind = \"free\" }, action = { kind = \"none\" } }\n\
                    secondary = { cadence = { kind = \"semi\" }, cost = { kind = \"free\" }, action = { kind = \"none\" } }\n";
        let mut entry = parse_weapon(text).expect("parses");
        entry.model = None;
        let written = rewrite(text, &entry).expect("rewrites");
        assert!(!written.contains("model"), "{written}");
        assert!(written.contains("note = \"mine\""), "{written}");
    }

    #[test]
    fn a_file_changed_since_it_was_read_is_noticed() {
        let (text, entry) = shipped("pistol");
        let path = Path::new("pistol.toml");
        assert!(check_unchanged(path, &text, Some(&entry)).is_ok());
        assert!(check_unchanged(path, &text, None).is_err(), "a file made on disk went unnoticed");
        let mut edited = entry.clone();
        edited.deploy += 1.0;
        assert!(check_unchanged(path, &text, Some(&edited)).is_err(), "an edit on disk went unnoticed");
        assert!(check_unchanged(path, "", None).is_ok());
    }

    #[test]
    fn a_rename_keeps_the_comments_and_removes_the_old_file() {
        let pack = std::env::temp_dir().join(format!("grackle_weapon_rename_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&pack);
        std::fs::create_dir_all(pack.join("weapons")).unwrap();
        let (text, mut entry) = shipped("pistol");
        std::fs::write(weapon_path(&pack, "pistol"), &text).unwrap();
        let was = entry.clone();
        entry.deploy = 0.4;
        save_weapon(&pack, "sidearm", &entry, None, Some(("pistol", &was, true))).expect("saves").expect("deletes the old file");
        assert!(!weapon_path(&pack, "pistol").exists(), "the old file was left");
        let written = std::fs::read_to_string(weapon_path(&pack, "sidearm")).unwrap();
        assert!(written.starts_with("# Weak and quick"), "{written}");
        assert_eq!(parse_weapon(&written).unwrap(), entry);
        delete_weapon(&pack, "sidearm", &entry).expect("deletes");
        assert!(!weapon_path(&pack, "sidearm").exists());
        let _ = std::fs::remove_dir_all(&pack);
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
        let (_, entry) = shipped("pistol");
        assert!(rewrite("name_key = \n", &entry).is_err());
    }
}
