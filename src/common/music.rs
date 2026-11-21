//! The packs' music, found at `Startup` into [`MusicLibrary`]. A track takes
//! one of two shapes:
//!
//! - `music/{file}.{ext}`: a sound file on its own, credited to nobody. This is
//!   what somebody drops into `assets/user/music` for themself.
//! - `music/{folder}/{file}.{ext}` beside `music/{folder}/attribution.txt`:
//!   every sound file in the folder, whatever it is named, credited by that
//!   file. A folder without one is still played, credited to nobody.
//!
//! The attribution's keys, all optional:
//!
//! ```text
//! Title: Dawn Patrol
//! Artist: Somebody
//! Copyright: Copyright (c) 2025 Somebody
//! License: Creative Commons Attribution 4.0
//! License File: LICENSE.txt
//! Source: https://example.com/dawn-patrol
//! ```
//!
//! Many licenses ask for their attribution word for word, so a file that is not
//! `Key: Value` lines naming one of those keys is kept as text and shown as it
//! is written.
//!
//! The highest-priority pack's copy of a folder or a loose file wins whole.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bevy::prelude::*;

use crate::common::assets::{AssetSource, Assets};
use crate::common::attribution::{self, PackListing, ATTRIBUTION_FILE};

pub const MUSIC_DIR: &str = "music";

/// Matched without case.
pub const SOUND_EXTENSIONS: [&str; 6] = ["ogg", "oga", "opus", "mp3", "wav", "flac"];

const KEYS: [&str; 6] = ["title", "artist", "copyright", "license", "license file", "source"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MusicAttribution {
    Fields {
        title: Option<String>,
        artist: Option<String>,
        copyright: Option<String>,
        license: Option<String>,
        /// Relative to the folder.
        license_file: Option<String>,
        source: Option<String>,
    },
    /// Shown as written.
    Text(String),
}

pub fn parse_attribution(text: &str) -> MusicAttribution {
    if let Ok(fields) = attribution::fields(text) {
        if fields.iter().any(|(key, _)| KEYS.contains(&key.as_str())) {
            let get = |key| attribution::field(&fields, key);
            return MusicAttribution::Fields {
                title: get("title"),
                artist: get("artist"),
                copyright: get("copyright"),
                license: get("license"),
                license_file: get("license file"),
                source: get("source"),
            };
        }
    }
    MusicAttribution::Text(text.trim_start_matches('\u{feff}').trim().to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub pack: PathBuf,
    /// Relative to the pack, such as `music/Dawn/theme.ogg`.
    pub path: String,
}

impl Track {
    /// The file name without its extension.
    pub fn name(&self) -> &str {
        let file = self.path.rsplit('/').next().unwrap_or(&self.path);
        file.rsplit_once('.').map_or(file, |(stem, _)| stem)
    }
}

/// One credit: a folder and its tracks, or a loose track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicCredit {
    /// The folder under `music/`; `None` for a loose track.
    pub folder: Option<String>,
    pub attribution: Option<MusicAttribution>,
    /// Sorted by path.
    pub tracks: Vec<Track>,
}

impl MusicCredit {
    /// The title, else the folder, else the track's name.
    pub fn name(&self) -> String {
        if let Some(MusicAttribution::Fields { title: Some(title), .. }) = &self.attribution {
            return title.clone();
        }
        match (&self.folder, self.tracks.first()) {
            (Some(folder), _) => folder.clone(),
            (None, Some(track)) => track.name().to_owned(),
            (None, None) => String::new(),
        }
    }
}

#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct MusicLibrary {
    /// The credited first, then the rest, each sorted by name.
    pub credits: Vec<MusicCredit>,
}

impl MusicLibrary {
    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.credits.iter().flat_map(|credit| credit.tracks.iter())
    }

    /// By [`Track::path`], the same in every pack.
    pub fn track(&self, path: &str) -> Option<&Track> {
        self.tracks().find(|track| track.path == path)
    }

    /// The path itself where no pack has the track.
    pub fn label(&self, path: &str) -> String {
        for credit in &self.credits {
            if let Some(track) = credit.tracks.iter().find(|track| track.path == path) {
                return match credit.tracks.len() {
                    1 => credit.name(),
                    _ => format!("{}: {}", credit.name(), track.name()),
                };
            }
        }
        path.to_owned()
    }
}

impl Track {
    /// For the `AssetServer`, whose root is `assets/`; `None` for a pack outside it.
    pub fn asset_path(&self) -> Option<String> {
        crate::common::assets::asset_path(&self.pack, &self.path)
    }
}

pub fn is_sound(file: &str) -> bool {
    file.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && SOUND_EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(ext))
    })
}

/// `listings` lowest priority first. A folder with nothing to play is skipped,
/// and an attribution that cannot be read is a warning, with its tracks kept
/// uncredited.
pub fn discover(listings: &[PackListing], source: &dyn AssetSource) -> (MusicLibrary, Vec<String>) {
    // Each later pack replaces what an earlier one had under the same key.
    let mut folders: BTreeMap<String, (PathBuf, Vec<String>)> = BTreeMap::new();
    let mut loose: BTreeMap<String, Track> = BTreeMap::new();
    for listing in listings {
        let mut in_this_pack: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for file in &listing.files {
            let Some(rest) = file.strip_prefix(MUSIC_DIR).and_then(|rest| rest.strip_prefix('/')) else { continue };
            match rest.split('/').collect::<Vec<_>>()[..] {
                [name] if is_sound(name) => {
                    loose.insert(name.to_owned(), Track { pack: listing.pack.clone(), path: file.clone() });
                }
                [folder, _] => in_this_pack.entry(folder.to_owned()).or_default().push(file.clone()),
                _ => {}
            }
        }
        for (folder, files) in in_this_pack {
            folders.insert(folder, (listing.pack.clone(), files));
        }
    }

    let mut warnings = Vec::new();
    let mut credits = Vec::new();
    for (folder, (pack, files)) in folders {
        let mut tracks: Vec<Track> = files
            .iter()
            .filter(|file| is_sound(file.rsplit('/').next().unwrap_or(file)))
            .map(|file| Track { pack: pack.clone(), path: file.clone() })
            .collect();
        if tracks.is_empty() {
            continue;
        }
        tracks.sort_by(|a, b| a.path.cmp(&b.path));
        let attribution_path = format!("{MUSIC_DIR}/{folder}/{ATTRIBUTION_FILE}");
        let attribution = match files.contains(&attribution_path) {
            false => None,
            true => match source.read(&pack, &attribution_path).map_err(|e| e.to_string()).and_then(|bytes| {
                String::from_utf8(bytes).map_err(|e| format!("{}/{attribution_path}: {e}", pack.display()))
            }) {
                Ok(text) => Some(parse_attribution(&text)),
                Err(problem) => {
                    warnings.push(format!("{problem}; its music is shown uncredited"));
                    None
                }
            },
        };
        credits.push(MusicCredit { folder: Some(folder), attribution, tracks });
    }
    credits.extend(loose.into_values().map(|track| MusicCredit { folder: None, attribution: None, tracks: vec![track] }));
    credits.sort_by_cached_key(|credit| (credit.attribution.is_none(), credit.name().to_lowercase()));
    (MusicLibrary { credits }, warnings)
}

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Assets>().init_resource::<MusicLibrary>().add_systems(Startup, find_music);
    }
}

fn find_music(assets: Res<Assets>, mut library: ResMut<MusicLibrary>) {
    let listings = attribution::scan_packs(&crate::common::assets::default_packs(), MUSIC_DIR);
    let (found, warnings) = discover(&listings, assets.0.as_ref());
    for warning in warnings {
        warn!("{warning}");
    }
    info!("Found {} music tracks", found.tracks().count());
    *library = found;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::common::assets::AssetError;
    use std::collections::HashMap;
    use std::path::Path;

    /// Packs held in memory, keyed by `{pack}/{relative}`.
    pub(crate) struct MemoryFiles(pub HashMap<String, Vec<u8>>);

    impl AssetSource for MemoryFiles {
        fn read(&self, pack: &Path, relative: &str) -> Result<Vec<u8>, AssetError> {
            let key = format!("{}/{relative}", pack.display());
            self.0.get(&key).cloned().ok_or(AssetError::NotFound(key))
        }

        fn list(&self, pack: &Path, folder: &str) -> Vec<String> {
            let prefix = format!("{}/{folder}/", pack.display());
            self.0
                .keys()
                .filter_map(|key| key.strip_prefix(&prefix))
                .filter(|name| !name.contains('/'))
                .map(str::to_owned)
                .collect()
        }
    }

    /// Each pack's files with their text, lowest priority first.
    pub(crate) fn memory_packs(packs: &[(&str, &[(&str, &str)])]) -> (Vec<PackListing>, MemoryFiles) {
        let mut bytes = HashMap::new();
        let listings = packs
            .iter()
            .map(|(pack, files)| {
                for (file, text) in *files {
                    bytes.insert(format!("{pack}/{file}"), text.as_bytes().to_vec());
                }
                PackListing { pack: PathBuf::from(pack), files: files.iter().map(|(file, _)| file.to_string()).collect() }
            })
            .collect();
        (listings, MemoryFiles(bytes))
    }

    #[test]
    fn keys_are_fields_and_anything_else_is_text() {
        let fields = parse_attribution("Title: Dawn\nArtist: Somebody\nSource: https://example.com/a\nMood: grim");
        let MusicAttribution::Fields { title, artist, source, license, .. } = fields else { panic!("{fields:?}") };
        assert_eq!((title.as_deref(), artist.as_deref(), license), (Some("Dawn"), Some("Somebody"), None));
        assert_eq!(source.as_deref(), Some("https://example.com/a"));

        let cc = "\"Dawn\" Somebody (example.com)\nLicensed under Creative Commons: By Attribution 4.0\n";
        assert_eq!(parse_attribution(cc), MusicAttribution::Text(cc.trim().to_owned()));
        assert_eq!(parse_attribution("Mood: grim"), MusicAttribution::Text("Mood: grim".to_owned()));
    }

    #[test]
    fn a_track_is_found_and_labeled_by_its_path() {
        let (listings, files) = memory_packs(&[(
            "assets/default",
            &[
                ("music/Dawn/attribution.txt", "Title: Dawn Suite"),
                ("music/Dawn/one.ogg", ""),
                ("music/Dawn/two.ogg", ""),
                ("music/Pump/attribution.txt", "Title: Pump"),
                ("music/Pump/Pump.mp3", ""),
            ],
        )]);
        let (library, _) = discover(&listings, &files);
        assert_eq!(library.label("music/Pump/Pump.mp3"), "Pump");
        assert_eq!(library.label("music/Dawn/two.ogg"), "Dawn Suite: two");
        assert_eq!(library.label("music/gone.ogg"), "music/gone.ogg");
        let pump = library.track("music/Pump/Pump.mp3").unwrap();
        assert_eq!(pump.asset_path().as_deref(), Some("default/music/Pump/Pump.mp3"));
        assert!(library.track("music/gone.ogg").is_none());
        let elsewhere = Track { pack: PathBuf::from("/somewhere/pack"), path: "music/a.ogg".into() };
        assert_eq!(elsewhere.asset_path(), None);
    }

    #[test]
    fn sound_files_are_known_by_extension_without_case() {
        assert!(is_sound("a.OGG") && is_sound("b.flac") && is_sound("c.d.mp3"));
        assert!(!is_sound("attribution.txt") && !is_sound(".ogg") && !is_sound("ogg"));
    }

    #[test]
    fn folders_credit_every_sound_file_and_loose_files_credit_nobody() {
        let (listings, files) = memory_packs(&[
            (
                "default",
                &[
                    ("music/Dawn/attribution.txt", "Title: Dawn Suite\nArtist: A"),
                    ("music/Dawn/not-named-dawn.ogg", ""),
                    ("music/Dawn/second.mp3", ""),
                    ("music/Dawn/LICENSE.txt", ""),
                    ("music/Bare/tune.wav", ""),
                    ("music/Empty/attribution.txt", "Title: Nothing"),
                    ("music/loose.flac", ""),
                    ("music/notes.txt", ""),
                    ("music/Deep/a/b.ogg", ""),
                ],
            ),
            (
                "user",
                &[("music/Dawn/attribution.txt", "Title: Override"), ("music/Dawn/only.ogg", ""), ("music/mine.ogg", "")],
            ),
        ]);
        let (library, warnings) = discover(&listings, &files);
        assert!(warnings.is_empty(), "{warnings:?}");
        let shown: Vec<(String, Vec<&str>)> = library
            .credits
            .iter()
            .map(|credit| (credit.name(), credit.tracks.iter().map(|track| track.path.as_str()).collect()))
            .collect();
        assert_eq!(
            shown,
            [
                ("Override".to_owned(), vec!["music/Dawn/only.ogg"]),
                ("Bare".to_owned(), vec!["music/Bare/tune.wav"]),
                ("loose".to_owned(), vec!["music/loose.flac"]),
                ("mine".to_owned(), vec!["music/mine.ogg"]),
            ]
        );
        assert_eq!(library.credits[0].tracks[0].pack, PathBuf::from("user"));
        assert_eq!(library.tracks().count(), 4);
    }

    #[test]
    fn an_unreadable_attribution_keeps_its_music_uncredited() {
        let (listings, mut files) = memory_packs(&[("p", &[("music/X/attribution.txt", ""), ("music/X/a.ogg", "")])]);
        files.0.insert("p/music/X/attribution.txt".to_owned(), vec![0xff, 0xfe, 0x00]);
        let (library, warnings) = discover(&listings, &files);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(library.credits[0].attribution, None);
        assert_eq!(library.credits[0].tracks.len(), 1);
    }

    /// Breaks if the default pack ships a track with no attribution.
    #[test]
    fn every_shipped_track_is_credited() {
        let packs = crate::common::assets::default_packs();
        let listings = crate::common::attribution::scan_packs(&packs[..1], MUSIC_DIR);
        let (library, warnings) = discover(&listings, &crate::common::assets::NativeFiles);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!library.credits.is_empty());
        for credit in &library.credits {
            assert!(credit.attribution.is_some(), "{:?} has no attribution", credit.tracks);
        }
    }
}
