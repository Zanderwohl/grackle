use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};
use bevy::platform::collections::HashMap;
use bevy::prelude::{error, warn};
use regex::{Captures, Regex};

/// The default pack's lang directory — the reference the parity tests measure
/// against, and the seed loaded before anyone has scanned for packs.
pub const LANG_DIR: &str = "assets/default/lang";

/// The language every other one is measured against, and the one to fall back
/// to when a configured language turns out not to exist.
pub const FALLBACK_LANG: &str = "en-US";

/// The pack list until Grackle grows a real one: the default pack alone.
///
/// The merge machinery below takes a list rather than a single root because
/// the map format is meant to carry a game's own content, and a game that
/// ships its own tools will want to name them. Until that exists, every
/// caller passes this.
pub fn default_packs() -> Vec<PathBuf> {
    vec![PathBuf::from("assets/default")]
}

static LANG: LazyLock<RwLock<toml::Value>> = LazyLock::new(|| RwLock::new(
    load_lang(FALLBACK_LANG).unwrap_or_else(|e| {
        // `eprintln!` rather than `error!`: this can be forced before the
        // tracing subscriber is installed, and a broken install is exactly
        // when the message must not vanish. Every lookup then renders as
        // `<some.key>`, which is diagnosable — a panic in a `LazyLock` also
        // poisons it, so every later read panics too.
        eprintln!("{}; every string will render as its key", e);
        toml::Value::Table(toml::Table::new())
    })
));

/// The slots [`fill_template`] substitutes: `{ name }`, with optional spaces.
static TEMPLATE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\s*([A-Za-z_]+)\s*\}").unwrap());

/// Resolved strings: filled templates keyed by [`template_pair_path`], plain
/// lookups keyed by the dotted key itself. The two key shapes cannot collide —
/// a template key carries a `|`.
///
/// Only ever written to while holding the `LANG` read lock, and emptied by
/// [`change_lang`] while it holds the write lock, so a fill that began before
/// a language switch cannot land its now-stale result after it.
static CACHE: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// How many filled templates to keep before starting over.
///
/// The key space is unbounded — arguments are feature ids, counts and error
/// strings — so an editor left open would otherwise hold one entry per
/// distinct message it has ever shown. Clearing wholesale rather than evicting
/// the least recently used costs two lines instead of a data structure, and
/// what the panels draw every frame is back inside a frame.
const CACHE_LIMIT: usize = 1024;

/// The language code the table currently holds, so a pack reload can re-merge
/// the same language over the new pack list without asking anyone.
static CURRENT_LANG: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new(FALLBACK_LANG.to_string()));

/// Switch languages, leaving the current one in place if the new one will not
/// load.
///
/// `packs` is every pack root in priority order — highest first, default
/// pinned last — so a pack can name its own features and even retranslate the
/// base editor.
///
/// Loading before taking the lock is deliberate: a language picked from a
/// dropdown can still fail — a deleted file, a bad hand edit — and
/// half-applying it would leave every string reading `<some.key>` with no way
/// back. Failing here changes nothing at all.
pub fn change_lang(lang: &str, packs: &[PathBuf]) -> Result<(), String> {
    let loaded = load_merged(lang, packs)?;
    let mut lang_lock = LANG.write()
        .map_err(|_| "Could not change language; could not acquire lock.")?;
    *lang_lock = loaded;
    // Emptied under the write lock, which excludes the fills that write to it:
    // every string in there was filled from the table just replaced.
    if let Ok(mut cache) = CACHE.write() {
        cache.clear();
    }
    drop(lang_lock);
    if let Ok(mut current) = CURRENT_LANG.write() {
        *current = lang.to_string();
    }
    Ok(())
}

/// Re-merge the current language over a changed pack list — what a pack
/// reload calls, since adding or removing a pack changes which strings exist
/// without changing which language the user wants.
pub fn remerge(packs: &[PathBuf]) {
    let current = CURRENT_LANG
        .read()
        .map(|current| current.clone())
        .unwrap_or_else(|_| FALLBACK_LANG.to_string());
    if let Err(e) = change_lang(&current, packs) {
        error!("{e}; keeping the previous strings");
    }
}

/// Load `lang` at start-up, falling back to [`FALLBACK_LANG`] if it will not.
///
/// Returns the language actually in effect. Callers should store *that* rather
/// than what they asked for: a settings file naming a language which is not
/// installed should not go on claiming it is active.
///
/// Shared by every binary so their behaviour cannot drift.
pub fn change_lang_or_fallback(lang: &str, packs: &[PathBuf]) -> String {
    match change_lang(lang, packs) {
        Ok(()) => lang.to_string(),
        Err(e) => {
            error!("{}; falling back to `{}`", e, FALLBACK_LANG);
            if let Err(e) = change_lang(FALLBACK_LANG, packs) {
                // Nothing left to fall back to. Every lookup renders as its
                // own key from here, which is ugly but readable and points
                // straight at the missing pack.
                error!("{}", e);
            }
            FALLBACK_LANG.to_string()
        }
    }
}

/// The merged view of one language across every pack.
///
/// Two layers, each walking packs lowest-priority-first so a later (higher)
/// pack overrides leaf keys:
///
/// 1. every pack's `en-US` file — the base, so a key no translation covers
///    shows English rather than `<editor.foo>`;
/// 2. the chosen language's files on top.
///
/// A pack with no such file is silent. A file that will not parse is warned
/// and skipped — one pack's typo must not cost another pack its names. But a
/// language *no* pack provides is an error, so asking for `zz-ZZ` fails
/// instead of quietly serving English under the wrong name.
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

/// Merges one language code from every pack into `merged`, lowest priority
/// first. Returns how many files actually contributed.
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

/// Later tables win on leaves; tables merge recursively, so a pack adding
/// `[tools] foo` does not wipe out the base editor's whole `[tools]` section.
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

/// Read one language file from the default pack alone — the seed for the
/// process-global table before packs are scanned, and the reference the
/// parity tests measure against. Runtime switching goes through
/// [`load_merged`] instead.
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

/// A language the user can choose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    /// The file stem — `de-DE`. What gets stored in settings.
    pub code: String,
    /// What the language calls itself, from its own `language.name`.
    pub name: String,
}

/// Every language any pack ships a file for, sorted by code — a pack may add
/// a whole language, not just retranslate keys.
///
/// Read from disk rather than listed in code so that adding a translation is
/// dropping in a file.
///
/// A file whose `language.name` is missing or unreadable still appears, under
/// its code: a language that is awkward to identify is far better than one the
/// user cannot select at all. When several packs name the same language, the
/// highest-priority pack's name wins, matching how its strings would.
pub fn available_languages(packs: &[PathBuf]) -> Vec<Language> {
    let mut languages: Vec<Language> = Vec::new();

    // Lowest priority first, like the merge, so later packs override names.
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

    // Stable order, so the dropdown does not reshuffle between runs on the
    // whim of the directory listing.
    languages.sort_by(|a, b| a.code.cmp(&b.code));
    languages
}

/// Walk `keys` down an already-borrowed table. Split out of [`get_maybe`] so
/// that [`fill_template`] can read the table and write the cache under one
/// `LANG` read lock — `RwLock` is not reentrant, so it cannot simply call
/// `get_maybe` while holding it.
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

/// The plain lookup behind `get!`, answering from [`CACHE`] when this key has
/// been read since the last language change.
///
/// Cached for the same reason a filled template is: the panels ask for their
/// labels on every frame they draw, and a miss splits the key, takes the
/// `LANG` lock and allocates.
pub fn get_parsed(keys: &str) -> String {
    // Dropped before the `LANG` lock is taken, for the ordering reason
    // `fill_template` gives.
    if let Some(cached) = CACHE.read().ok().and_then(|cache| cache.get(keys).cloned()) {
        return cached;
    }

    let path = keys.split('.').collect::<Vec<&str>>();
    // Held across the read *and* the store, so a language switch landing in
    // between cannot leave a string from the old table behind in the cache.
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

/// As `get!`, but formats arguments with `{:?}` — for keys whose slots are
/// filled with types that have no `Display`.
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

/// Substitute `pairs` into the string at `keys`, answering from [`CACHE`] when
/// this exact key and arguments have been filled since the last language
/// change.
fn fill_template(keys: &[&str], pairs: &[(&str, &str)]) -> Result<String, String> {
    let cache_key = template_pair_path(keys, pairs);
    // Dropped before the `LANG` lock is taken: every other path takes `LANG`
    // first, and holding this across that would be the one ordering that can
    // deadlock against `change_lang`.
    if let Some(cached) = CACHE.read().ok().and_then(|cache| cache.get(&cache_key).cloned()) {
        return Ok(cached);
    }

    // Held across the fill *and* the store, so that a language switch landing
    // in between cannot leave a string from the old table behind in the cache.
    let lang = LANG.read().unwrap();
    let template = lookup(&lang, keys)
        .ok_or(format!("Failed to find value for key {:?}", keys))?;

    // One pass over the slots the template actually has, rather than one
    // freshly compiled regex per argument over the whole string. Two things
    // fall out of that beyond the compiles it saves:
    //
    // - arguments land verbatim. Handing `replace_all` a `&str` makes the
    //   regex crate read `$name` in it as a capture-group reference, so a
    //   path or error string containing one used to vanish from the sentence
    //   reporting it. A closure's return value is never expanded.
    // - a replacement whose text contains another slot is not itself
    //   rescanned, so the result no longer depends on argument order.
    //
    // A slot no argument names is returned as it was found, which is what
    // `every_language_keeps_its_placeholders` exists to catch in the files.
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

/// The loaded language is process-global and cargo runs tests on parallel
/// threads, so anything that *switches* it has to exclude anything that
/// *reads* it. Tests calling only [`load_lang`] are pure and need no lock.
///
/// Outside the test module so that other modules' tests — anything driving
/// [`change_lang`] through a system — can take the same lock.
#[cfg(test)]
static LANG_SWITCHING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take the lock and start from a known language.
///
/// Poisoning is ignored on purpose: a panic in one test leaves the global
/// language wherever it was, and every user of this sets what it needs first,
/// so there is no broken invariant for the poison flag to protect.
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

    /// The old filler compiled `\{\s*$key\s*\}` per argument, but the statics
    /// it drove were written for a `${name}` syntax the lang files never used.
    /// This is the syntax they do use.
    #[test]
    fn a_brace_slot_is_what_actually_gets_filled() {
        let _guard = switching_language();
        assert_eq!(
            get_template(&["room", "messages", "ghost"], &[("me", "3"), ("other", "7")]),
            "Room 3 is fully inside 7 and will not appear!"
        );
    }

    /// Arguments are feature ids, paths and formatted errors — all free to
    /// contain a `$`, which the regex crate reads as a capture-group reference
    /// when the replacement is a `&str`. `$dollar_bill` named no group, so the
    /// whole substitution expanded to nothing and the argument vanished from
    /// the sentence reporting it.
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

    /// A replacement is not itself rescanned, so text that happens to look
    /// like a slot survives whatever the other arguments are called.
    #[test]
    fn an_argument_containing_a_slot_is_not_substituted_into() {
        let _guard = switching_language();
        assert_eq!(
            get_template(&["editor", "title_special"], &[("game", "{ game }")]),
            "{ game } Grackle Level Editor"
        );
    }

    /// The cache was a plain `HashMap` nothing could write to, so it missed
    /// every time and cost a fresh regex compile per argument on every frame
    /// the panels drew a templated string.
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

        // And the stored string is what the next caller gets, rather than the
        // cache being written but never read.
        CACHE.write().unwrap().insert(key, "from the cache".to_string());
        assert_eq!(
            get_template(&["editor", "title_special"], &[("game", "Grackle")]),
            "from the cache"
        );
    }

    /// A cache that survived a language switch would be worse than no cache:
    /// every string already drawn would keep its old language until something
    /// happened to evict it.
    #[test]
    fn switching_language_forgets_what_was_filled() {
        let _guard = switching_language();
        assert_eq!(get_parsed("viewport.free"), "Freecam");

        change_lang("de-DE", &default_packs()).unwrap();
        assert_eq!(get_parsed("viewport.free"), "Freie Kamera");
    }

    /// Arguments are counts, ids and error strings, so an editor left open
    /// would otherwise keep one entry per distinct message it had ever shown.
    #[test]
    fn the_cache_does_not_grow_without_bound() {
        let _guard = switching_language();
        for i in 0..CACHE_LIMIT + 10 {
            get_template(&["editor", "title_special"], &[("game", &i.to_string())]);
        }
        assert!(CACHE.read().unwrap().len() <= CACHE_LIMIT,
            "cache grew to {}", CACHE.read().unwrap().len());
    }

    /// Every leaf key in a lang file, as dotted paths.
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

    /// The `{ name }` slots a string expects filled in.
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

    /// A translation that drops `{ me }` loses information with no error
    /// anywhere — the sentence still reads fine, just about nothing in
    /// particular. Renaming one is worse: `get!` leaves an unfilled `{ ich }`
    /// sitting in the output.
    ///
    /// Only keys a translation actually covers are checked. Grackle's `de-DE`
    /// is behind `en-US`, and the English base layer in [`load_merged`] means
    /// a key it has not reached yet renders as English rather than as
    /// `<editor.foo>`. See the coverage note in CLAUDE.md.
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
            // Falling back to the code is what happens when the key is absent,
            // so this catches a file that forgot to introduce itself.
            assert_ne!(language.name, language.code,
                "{} has no `language.name`, so the dropdown would show its code", language.code);
        }

        assert!(languages.iter().any(|l| l.code == FALLBACK_LANG));
        // Sorted, so the dropdown does not reorder itself between runs.
        let mut sorted = languages.clone();
        sorted.sort_by(|a, b| a.code.cmp(&b.code));
        assert_eq!(languages, sorted);
    }

    #[test]
    fn a_missing_language_is_an_error_and_not_a_panic() {
        let _guard = switching_language();
        // Reachable from a menu click once there is one, not only from start-up.
        assert!(load_lang("zz-ZZ").is_err());

        // And a failed switch must leave the working language alone rather
        // than emptying it.
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

        // Every binary picks a language before it can render anything, so one
        // that will not load has to degrade rather than abort.
        let effective = change_lang_or_fallback("zz-ZZ", &default_packs());

        assert_eq!(effective, FALLBACK_LANG);
        assert_eq!(get_parsed("viewport.free"), "Freecam",
            "reported a fallback it had not actually loaded");
    }

    /// The reason the English base layer exists: a translation that is behind
    /// shows readable English for what it has not reached, not `<editor.foo>`.
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

    /// A throwaway pack directory holding only lang files.
    fn lang_fixture(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("grackle-lang-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lang")).unwrap();
        for (file, content) in files {
            fs::write(dir.join("lang").join(file), content).unwrap();
        }
        dir
    }

    /// UI order: the fixture pack above the default pack.
    fn with_default(pack: PathBuf) -> Vec<PathBuf> {
        vec![pack, PathBuf::from("assets/default")]
    }

    /// The reason merging exists: a game's own tools get real names instead of
    /// `<tools.foo>` on screen.
    #[test]
    fn a_pack_can_name_its_own_strings() {
        let _guard = switching_language();
        let pack = lang_fixture("own-names", &[("en-US.toml", "[tools]\nfoo = \"Foo Tool\"")]);

        change_lang(FALLBACK_LANG, &with_default(pack)).unwrap();
        assert_eq!(get_parsed("tools.foo"), "Foo Tool");
        assert_eq!(get_parsed("tools.select"), "Select", "adding a pack must not lose the base strings");
    }

    /// Higher in the pack list wins.
    #[test]
    fn a_higher_priority_pack_overrides_a_name() {
        let _guard = switching_language();
        let pack = lang_fixture("override", &[("en-US.toml", "[tools]\nselect = \"Pick\"")]);

        change_lang(FALLBACK_LANG, &with_default(pack)).unwrap();
        assert_eq!(get_parsed("tools.select"), "Pick");
        assert_eq!(get_parsed("tools.point"), "Point", "keys the pack does not name are untouched");
    }

    /// A pack that only ships English must still show *something* readable
    /// under another language — English, not `<tools.foo>`.
    #[test]
    fn a_pack_without_the_chosen_language_still_shows_its_english_names() {
        let _guard = switching_language();
        let pack = lang_fixture("english-only", &[("en-US.toml", "[tools]\nfoo = \"Foo Tool\"")]);

        change_lang("de-DE", &with_default(pack)).unwrap();
        assert_eq!(get_parsed("viewport.free"), "Freie Kamera", "the chosen language still wins where it exists");
        assert_eq!(get_parsed("tools.foo"), "Foo Tool");
    }

    /// One pack's typo must cost that pack its strings, never another pack's.
    #[test]
    fn a_broken_lang_file_in_one_pack_does_not_break_the_others() {
        let _guard = switching_language();
        let pack = lang_fixture("broken", &[("en-US.toml", "this is not [ toml")]);

        change_lang(FALLBACK_LANG, &with_default(pack)).unwrap();
        assert_eq!(get_parsed("tools.select"), "Select");
    }

    /// Asking for a language nobody ships must fail — quietly serving the
    /// English base layer under the wrong name would hide the problem.
    #[test]
    fn a_language_no_pack_provides_is_still_an_error() {
        let _guard = switching_language();
        let pack = lang_fixture("no-such-lang", &[("en-US.toml", "[tools]\nfoo = \"Foo\"")]);

        assert!(change_lang("zz-ZZ", &with_default(pack)).is_err());
        assert_eq!(get_parsed("tools.select"), "Select", "the failed switch must change nothing");
    }

    /// A pack can ship a language the base editor does not have at all.
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
