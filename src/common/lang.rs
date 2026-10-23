use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};
use bevy::platform::collections::HashMap;
use bevy::prelude::{error, warn};
use regex::{Captures, Regex};

/// The default pack's lang directory: the reference for the parity tests and
/// the seed before packs are scanned.
pub const LANG_DIR: &str = "assets/default/lang";

pub const FALLBACK_LANG: &str = "en-US";

/// The pack list until Grackle has a real one.
pub fn default_packs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("assets/default"),
        PathBuf::from(crate::common::assets::USER_PACK),
    ]
}

static LANG: LazyLock<RwLock<toml::Value>> = LazyLock::new(|| RwLock::new(
    load_lang(FALLBACK_LANG).unwrap_or_else(|e| {
        // `eprintln!` because this can run before the tracing subscriber is
        // installed. Not a panic: that would poison the `LazyLock` and every
        // later read would panic too.
        eprintln!("{}; every string will render as its key", e);
        toml::Value::Table(toml::Table::new())
    })
));

static TEMPLATE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\s*([A-Za-z_]+)\s*\}").unwrap());

/// Filled templates keyed by [`template_pair_path`] (which contains a `|`),
/// plain lookups by the dotted key.
///
/// Only written while holding the `LANG` read lock, and cleared by
/// [`change_lang`] under the write lock, so a fill that began before a
/// language switch cannot store a stale result after it.
static CACHE: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The key space is unbounded (arguments are ids, counts, error strings), so
/// the cache is cleared wholesale when it reaches this size.
const CACHE_LIMIT: usize = 1024;

/// So [`remerge`] can reload the same language over a new pack list.
static CURRENT_LANG: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new(FALLBACK_LANG.to_string()));

/// `packs` is in priority order, highest first, default pack last.
///
/// Loads before taking the lock, so a language that fails to load changes
/// nothing.
pub fn change_lang(lang: &str, packs: &[PathBuf]) -> Result<(), String> {
    let loaded = load_merged(lang, packs)?;
    let mut lang_lock = LANG.write()
        .map_err(|_| "Could not change language; could not acquire lock.")?;
    *lang_lock = loaded;
    if let Ok(mut cache) = CACHE.write() {
        cache.clear();
    }
    drop(lang_lock);
    if let Ok(mut current) = CURRENT_LANG.write() {
        *current = lang.to_string();
    }
    Ok(())
}

pub fn remerge(packs: &[PathBuf]) {
    let current = CURRENT_LANG
        .read()
        .map(|current| current.clone())
        .unwrap_or_else(|_| FALLBACK_LANG.to_string());
    if let Err(e) = change_lang(&current, packs) {
        error!("{e}; keeping the previous strings");
    }
}

/// Returns the language actually in effect; callers should store that rather
/// than what they asked for.
pub fn change_lang_or_fallback(lang: &str, packs: &[PathBuf]) -> String {
    match change_lang(lang, packs) {
        Ok(()) => lang.to_string(),
        Err(e) => {
            error!("{}; falling back to `{}`", e, FALLBACK_LANG);
            if let Err(e) = change_lang(FALLBACK_LANG, packs) {
                error!("{}", e);
            }
            FALLBACK_LANG.to_string()
        }
    }
}

/// Every pack's `en-US` as a base, then the chosen language on top, each
/// walking packs lowest priority first.
///
/// A file that will not parse is skipped with a warning. A language no pack
/// provides is an error, rather than English served under the wrong name.
fn load_merged(lang: &str, packs: &[PathBuf]) -> Result<toml::Value, String> {
    let mut merged = toml::Table::new();
    if lang != FALLBACK_LANG {
        merge_layer(&mut merged, FALLBACK_LANG, packs);
    }
    if merge_layer(&mut merged, lang, packs) == 0 {
        return Err(format!("No pack provides a lang file for `{}`", lang));
    }
    Ok(toml::Value::Table(merged))
}

/// Returns how many files contributed.
fn merge_layer(merged: &mut toml::Table, lang: &str, packs: &[PathBuf]) -> usize {
    let mut contributed = 0;
    for pack in packs.iter().rev() {
        let path = pack.join("lang").join(format!("{lang}.toml"));
        if !path.exists() {
            continue;
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                warn!("{}: could not be read ({e}); skipping this lang file", path.display());
                continue;
            }
        };
        match toml::from_str::<toml::Table>(&text) {
            Ok(table) => {
                deep_merge(merged, table);
                contributed += 1;
            }
            Err(e) => {
                warn!("{}: is not valid toml; skipping this lang file ({e})", path.display());
            }
        }
    }
    contributed
}

/// Tables merge recursively, so a pack adding `[tools] foo` keeps the rest of
/// `[tools]`.
fn deep_merge(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match value {
            toml::Value::Table(incoming) => match base.get_mut(&key) {
                Some(toml::Value::Table(existing)) => deep_merge(existing, incoming),
                _ => {
                    base.insert(key, toml::Value::Table(incoming));
                }
            },
            other => {
                base.insert(key, other);
            }
        }
    }
}

/// The default pack alone; runtime switching goes through [`load_merged`].
fn load_lang(lang: &str) -> Result<toml::Value, String> {
    let lang_path = lang_path(lang);
    let toml_str = fs::read_to_string(&lang_path)
        .map_err(|e| format!("Could not read lang file `{}`: {}", lang_path, e))?;

    toml::from_str(&toml_str)
        .map_err(|e| format!("Could not parse lang file `{}`: {}", lang_path, e))
}

fn lang_path(lang: &str) -> String {
    format!("{}/{}.toml", LANG_DIR, lang)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    /// The file stem, e.g. `de-DE`. What settings store.
    pub code: String,
    /// From the file's own `language.name`.
    pub name: String,
}

/// Sorted by code. A file with no readable `language.name` still appears,
/// under its code. When packs disagree on a name, the highest priority wins.
pub fn available_languages(packs: &[PathBuf]) -> Vec<Language> {
    let mut languages: Vec<Language> = Vec::new();

    for pack in packs.iter().rev() {
        let lang_dir = pack.join("lang");
        let Ok(entries) = fs::read_dir(&lang_dir) else { continue };
        for path in entries.filter_map(|entry| entry.ok()).map(|entry| entry.path()) {
            if !path.extension().is_some_and(|ext| ext == "toml") {
                continue;
            }
            let Some(code) = path.file_stem().and_then(|stem| stem.to_str()) else { continue };
            let name = fs::read_to_string(&path)
                .ok()
                .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
                .and_then(|table| {
                    table.get("language")?.get("name")?.as_str().map(str::to_string)
                });

            match languages.iter_mut().find(|language| language.code == code) {
                Some(language) => {
                    if let Some(name) = name {
                        language.name = name;
                    }
                }
                None => languages.push(Language {
                    code: code.to_string(),
                    name: name.unwrap_or_else(|| code.to_string()),
                }),
            }
        }
    }

    languages.sort_by(|a, b| a.code.cmp(&b.code));
    languages
}

/// Takes an already-borrowed table so [`fill_template`] can read and store
/// under one `LANG` read lock; `RwLock` is not reentrant.
fn lookup<'a>(table: &'a toml::Value, keys: &[&str]) -> Option<&'a str> {
    let mut current = table;
    for key in keys {
        current = current.get(*key)?;
    }
    current.as_str()
}

pub fn get_maybe(keys: &[&str]) -> Option<String> {
    lookup(&LANG.read().unwrap(), keys).map(str::to_string)
}

pub fn get_infallible(keys: &[&str]) -> String {
    get_maybe(keys).unwrap_or(format!("<{}>", full_path(keys)))
}

/// Cached because the panels ask for every label on every frame.
pub fn get_parsed(keys: &str) -> String {
    // Lock ordering: see `fill_template`.
    if let Some(cached) = CACHE.read().ok().and_then(|cache| cache.get(keys).cloned()) {
        return cached;
    }

    let path = keys.split('.').collect::<Vec<&str>>();
    // Held across the read and the store; see `CACHE`.
    let lang = LANG.read().unwrap();
    let product = lookup(&lang, &path)
        .map(str::to_string)
        .unwrap_or_else(|| format!("<{}>", full_path(&path)));

    if let Ok(mut cache) = CACHE.write() {
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(keys.to_string(), product.clone());
    }

    product
}

fn full_path(keys: &[&str]) -> String {
    let mut debug_string = "".into();
    for (idx, key) in keys.iter().enumerate() {
        debug_string += *key;
        if idx < keys.len() - 1 {
            debug_string += "."
        }
    }
    debug_string
}

fn pair_path(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}:{}", k, v)).collect::<Vec<_>>().join("::")
}

fn template_pair_path(keys: &[&str], pairs: &[(&str, &str)]) -> String {
    format!("{}|{}", full_path(keys), pair_path(pairs))
}

pub fn get_template_maybe(keys: &[&str], pairs: &[(&str, &str)]) -> Result<String, String> {
    fill_template(keys, pairs)
}

pub fn get_template(keys: &[&str], pairs: &[(&str, &str)]) -> String {
    fill_template(keys, pairs)
        .unwrap_or_else(|_| format!("<{}; {:?}>", full_path(keys), pairs))
}

pub fn get_template_parsed(key: &str, pairs: &[(&str, &str)]) -> String {
    let keys = key.split('.').collect::<Vec<&str>>();
    get_template(&keys, pairs)
}

#[macro_export]
macro_rules! get {
    ($key:expr) => {
        $crate::common::lang::get_parsed(&$key)
    };
    ($key:expr, $($var:expr, $val:expr),*) => {
        $crate::common::lang::get_template_parsed(&$key, &[
            $(($var, &format!("{}", $val))),*
        ])
    };
}

/// As `get!`, formatting arguments with `{:?}`.
#[macro_export]
macro_rules! get_with_debug {
    ($key:expr) => {
        $crate::common::lang::get_parsed(&$key)
    };
    ($key:expr, $($var:expr, $val:expr),*) => {
        $crate::common::lang::get_template_parsed(&$key, &[
            $(($var, &format!("{:?}", $val))),*
        ])
    };
}

fn fill_template(keys: &[&str], pairs: &[(&str, &str)]) -> Result<String, String> {
    let cache_key = template_pair_path(keys, pairs);
    // Dropped before taking `LANG`: every other path takes `LANG` first, and
    // the reverse order can deadlock against `change_lang`.
    if let Some(cached) = CACHE.read().ok().and_then(|cache| cache.get(&cache_key).cloned()) {
        return Ok(cached);
    }

    // Held across the fill and the store; see `CACHE`.
    let lang = LANG.read().unwrap();
    let template = lookup(&lang, keys)
        .ok_or(format!("Failed to find value for key {:?}", keys))?;

    // A closure rather than a `&str` replacement: the regex crate expands `$name`
    // in a `&str`, which would eat a `$` in a path or error string. One pass
    // also means a replacement containing a slot is not rescanned. A slot no
    // argument names is left as found.
    let product = TEMPLATE_REGEX.replace_all(template, |found: &Captures| {
        pairs.iter()
            .find(|(key, _)| *key == &found[1])
            .map_or_else(|| found[0].to_string(), |(_, replacement)| replacement.to_string())
    }).into_owned();

    if let Ok(mut cache) = CACHE.write() {
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(cache_key, product.clone());
    }

    Ok(product)
}

/// The language is process-global and tests run in parallel, so tests that
/// switch it take this. Outside `tests` so other modules' tests can too.
#[cfg(test)]
static LANG_SWITCHING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Poisoning is ignored: every user sets the language it needs first, so there
/// is no invariant for it to protect.
#[cfg(test)]
pub(crate) fn switching_language() -> std::sync::MutexGuard<'static, ()> {
    let guard = LANG_SWITCHING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    change_lang(FALLBACK_LANG, &default_packs()).unwrap();
    guard
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashSet};

    #[test]
    fn test_get_template() {
        let _guard = switching_language();
        let title = get_template(&["editor", "title_special"], &[("game", "Grackle")]);
        assert_eq!(title, "Grackle Grackle Level Editor");
    }

    #[test]
    fn a_brace_slot_is_what_actually_gets_filled() {
        let _guard = switching_language();
        assert_eq!(
            get_template(&["room", "messages", "ghost"], &[("me", "3"), ("other", "7")]),
            "Room 3 is fully inside 7 and will not appear!"
        );
    }

    /// Fails if a `&str` replacement lets the regex crate expand `$...` and
    /// drop the argument.
    #[test]
    fn a_dollar_sign_in_an_argument_survives() {
        let _guard = switching_language();

        for argument in ["$dollar_bill", "$1", "$$", "US$5", "${name}"] {
            assert_eq!(
                get_template(&["editor", "title_special"], &[("game", argument)]),
                format!("{argument} Grackle Level Editor")
            );
        }
    }

    #[test]
    fn an_argument_containing_a_slot_is_not_substituted_into() {
        let _guard = switching_language();
        assert_eq!(
            get_template(&["editor", "title_special"], &[("game", "{ game }")]),
            "{ game } Grackle Level Editor"
        );
    }

    /// Fails if the cache is never written, or written but never read.
    #[test]
    fn a_filled_template_is_remembered() {
        let _guard = switching_language();
        let key = template_pair_path(&["editor", "title_special"], &[("game", "Grackle")]);

        assert_eq!(CACHE.read().unwrap().get(&key), None, "the switch left the cache dirty");
        get_template(&["editor", "title_special"], &[("game", "Grackle")]);
        assert_eq!(
            CACHE.read().unwrap().get(&key).map(String::as_str),
            Some("Grackle Grackle Level Editor")
        );

        CACHE.write().unwrap().insert(key, "from the cache".to_string());
        assert_eq!(
            get_template(&["editor", "title_special"], &[("game", "Grackle")]),
            "from the cache"
        );
    }

    #[test]
    fn switching_language_forgets_what_was_filled() {
        let _guard = switching_language();
        assert_eq!(get_parsed("viewport.free"), "Freecam");

        change_lang("de-DE", &default_packs()).unwrap();
        assert_eq!(get_parsed("viewport.free"), "Freie Kamera");
    }

    #[test]
    fn the_cache_does_not_grow_without_bound() {
        let _guard = switching_language();
        for i in 0..CACHE_LIMIT + 10 {
            get_template(&["editor", "title_special"], &[("game", &i.to_string())]);
        }
        assert!(CACHE.read().unwrap().len() <= CACHE_LIMIT,
            "cache grew to {}", CACHE.read().unwrap().len());
    }

    fn keys(value: &toml::Value, prefix: &str, into: &mut BTreeSet<String>) {
        let Some(table) = value.as_table() else { return };
        for (key, child) in table {
            let path = if prefix.is_empty() { key.clone() } else { format!("{}.{}", prefix, key) };
            if child.is_table() {
                keys(child, &path, into);
            } else {
                into.insert(path);
            }
        }
    }

    fn key_set(lang: &str) -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        keys(&load_lang(lang).unwrap(), "", &mut set);
        set
    }

    fn leaf<'a>(value: &'a toml::Value, path: &str) -> Option<&'a toml::Value> {
        path.split('.').try_fold(value, |current, key| current.get(key))
    }

    fn placeholders(value: &toml::Value) -> HashSet<String> {
        let text = match value {
            toml::Value::String(text) => text.clone(),
            toml::Value::Array(items) => items.iter()
                .filter_map(|item| item.as_str()).collect::<Vec<_>>().join(" "),
            _ => String::new(),
        };
        TEMPLATE_REGEX.captures_iter(&text)
            .map(|found| found[1].to_string())
            .collect()
    }

    /// Fails if a translation drops or renames a `{ slot }`. Only keys the
    /// translation covers are checked; see the coverage note in CLAUDE.md.
    #[test]
    fn every_language_keeps_its_placeholders() {
        let reference = load_lang(FALLBACK_LANG).unwrap();

        for language in available_languages(&default_packs()) {
            if language.code == FALLBACK_LANG {
                continue;
            }
            let translated = load_lang(&language.code).unwrap();

            for key in key_set(FALLBACK_LANG) {
                let (Some(from), Some(to)) = (leaf(&reference, &key), leaf(&translated, &key))
                    else { continue };
                assert_eq!(placeholders(from), placeholders(to),
                    "{} changed the placeholders of `{}`", language.code, key);
            }
        }
    }

    #[test]
    fn every_language_names_itself() {
        let languages = available_languages(&default_packs());
        assert!(languages.len() >= 2, "expected several languages, found {:?}", languages);

        for language in &languages {
            assert_ne!(language.name, language.code,
                "{} has no `language.name`, so the dropdown would show its code", language.code);
        }

        assert!(languages.iter().any(|l| l.code == FALLBACK_LANG));
        let mut sorted = languages.clone();
        sorted.sort_by(|a, b| a.code.cmp(&b.code));
        assert_eq!(languages, sorted);
    }

    #[test]
    fn a_missing_language_is_an_error_and_not_a_panic() {
        let _guard = switching_language();
        assert!(load_lang("zz-ZZ").is_err());

        assert!(change_lang("zz-ZZ", &default_packs()).is_err());
        assert_eq!(get_parsed("viewport.free"), "Freecam", "a failed switch cleared the language");
    }

    #[test]
    fn start_up_uses_the_configured_language() {
        let _guard = switching_language();

        assert_eq!(change_lang_or_fallback("de-DE", &default_packs()), "de-DE");
        assert_eq!(get_parsed("viewport.free"), "Freie Kamera");
    }

    #[test]
    fn start_up_falls_back_rather_than_giving_up() {
        let _guard = switching_language();
        change_lang("de-DE", &default_packs()).unwrap();

        let effective = change_lang_or_fallback("zz-ZZ", &default_packs());

        assert_eq!(effective, FALLBACK_LANG);
        assert_eq!(get_parsed("viewport.free"), "Freecam",
            "reported a fallback it had not actually loaded");
    }

    /// Fails if a key `de-DE` lacks renders as `<key>` rather than English.
    #[test]
    fn an_incomplete_translation_falls_back_key_by_key() {
        let _guard = switching_language();
        let english = key_set(FALLBACK_LANG);
        let german = key_set("de-DE");
        let uncovered: Vec<_> = english.difference(&german).collect();
        assert!(!uncovered.is_empty(), "de-DE has caught up; turn on a coverage test instead");

        change_lang("de-DE", &default_packs()).unwrap();
        for key in uncovered {
            let shown = get_parsed(key);
            assert!(!shown.starts_with('<'), "`{}` rendered as its key under de-DE", key);
        }
    }

    fn lang_fixture(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("grackle-lang-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lang")).unwrap();
        for (file, content) in files {
            fs::write(dir.join("lang").join(file), content).unwrap();
        }
        dir
    }

    fn with_default(pack: PathBuf) -> Vec<PathBuf> {
        vec![pack, PathBuf::from("assets/default")]
    }

    #[test]
    fn a_pack_can_name_its_own_strings() {
        let _guard = switching_language();
        let pack = lang_fixture("own-names", &[("en-US.toml", "[tools]\nfoo = \"Foo Tool\"")]);

        change_lang(FALLBACK_LANG, &with_default(pack)).unwrap();
        assert_eq!(get_parsed("tools.foo"), "Foo Tool");
        assert_eq!(get_parsed("tools.select"), "Select", "adding a pack must not lose the base strings");
    }

    #[test]
    fn a_higher_priority_pack_overrides_a_name() {
        let _guard = switching_language();
        let pack = lang_fixture("override", &[("en-US.toml", "[tools]\nselect = \"Pick\"")]);

        change_lang(FALLBACK_LANG, &with_default(pack)).unwrap();
        assert_eq!(get_parsed("tools.select"), "Pick");
        assert_eq!(get_parsed("tools.point"), "Point", "keys the pack does not name are untouched");
    }

    #[test]
    fn a_pack_without_the_chosen_language_still_shows_its_english_names() {
        let _guard = switching_language();
        let pack = lang_fixture("english-only", &[("en-US.toml", "[tools]\nfoo = \"Foo Tool\"")]);

        change_lang("de-DE", &with_default(pack)).unwrap();
        assert_eq!(get_parsed("viewport.free"), "Freie Kamera", "the chosen language still wins where it exists");
        assert_eq!(get_parsed("tools.foo"), "Foo Tool");
    }

    /// The loader skips broken files, so without this a bad shipped file
    /// surfaces as some unrelated test failing. The usual cause is a key used
    /// both as a string and as a table.
    #[test]
    fn every_shipped_lang_file_parses() {
        let mut checked = 0;
        let Ok(entries) = std::fs::read_dir(std::path::Path::new("assets")) else {
            // Not run from the repo root.
            return;
        };

        for pack in entries.flatten() {
            let Ok(files) = std::fs::read_dir(pack.path().join("lang")) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                    continue;
                }
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                toml::from_str::<toml::Value>(&text)
                    .unwrap_or_else(|e| panic!("{} is not valid TOML: {e}", path.display()));
                checked += 1;
            }
        }
        assert!(checked > 0, "no lang files were found to check");
    }

    #[test]
    fn a_broken_lang_file_in_one_pack_does_not_break_the_others() {
        let _guard = switching_language();
        let pack = lang_fixture("broken", &[("en-US.toml", "this is not [ toml")]);

        change_lang(FALLBACK_LANG, &with_default(pack)).unwrap();
        assert_eq!(get_parsed("tools.select"), "Select");
    }

    #[test]
    fn a_language_no_pack_provides_is_still_an_error() {
        let _guard = switching_language();
        let pack = lang_fixture("no-such-lang", &[("en-US.toml", "[tools]\nfoo = \"Foo\"")]);

        assert!(change_lang("zz-ZZ", &with_default(pack)).is_err());
        assert_eq!(get_parsed("tools.select"), "Select", "the failed switch must change nothing");
    }

    #[test]
    fn a_pack_can_add_a_whole_language() {
        let _guard = switching_language();
        let pack = lang_fixture(
            "new-lang",
            &[("eo-EO.toml", "[language]\nname = \"Esperanto\"\n\n[tools]\nselect = \"Elekti\"")],
        );
        let packs = with_default(pack);

        let languages = available_languages(&packs);
        assert!(languages.iter().any(|language| language.code == "eo-EO" && language.name == "Esperanto"));

        change_lang("eo-EO", &packs).unwrap();
        assert_eq!(get_parsed("tools.select"), "Elekti");
        assert_eq!(get_parsed("tools.point"), "Point",
            "keys the new language does not cover fall back to English");
    }
}
