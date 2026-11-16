//! What the packs credit: the files under a pack's folder, and the
//! `attribution.txt` beside a font or a piece of music. [`music`] and
//! [`pack_fonts`] read their own keys from it; [`credits`] draws them.
//!
//! An `attribution.txt` is one `Key: Value` per line. Blank lines and lines
//! starting with `#` are skipped; a key is matched without case.
//!
//! [`music`]: crate::common::music
//! [`pack_fonts`]: crate::common::pack_fonts
//! [`credits`]: crate::common::credits

use std::path::{Path, PathBuf};

pub const ATTRIBUTION_FILE: &str = "attribution.txt";

/// The files of one pack under one folder, as paths relative to the pack with
/// `/` between parts, such as `music/Dawn/theme.ogg`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackListing {
    pub pack: PathBuf,
    pub files: Vec<String>,
}

/// How deep [`PackListing::scan`] goes below the folder: a file in it and a
/// file in a folder in it, which is every shape a credited asset takes.
const SCAN_DEPTH: usize = 3;

impl PackListing {
    /// Reads the directory, so native only, like the Developer page's icons; a
    /// browser build would read a manifest instead (see
    /// [`AssetSource`](crate::common::assets::AssetSource)). A pack without the
    /// folder lists nothing.
    pub fn scan(pack: &Path, folder: &str) -> Self {
        let mut files = Vec::new();
        walk(&pack.join(folder), folder, SCAN_DEPTH, &mut files);
        files.sort();
        Self { pack: pack.to_path_buf(), files }
    }
}

fn walk(dir: &Path, relative: &str, depth: usize, files: &mut Vec<String>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        if name.starts_with('.') {
            continue;
        }
        let path = format!("{relative}/{name}");
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk(&entry.path(), &path, depth - 1, files),
            Ok(_) => files.push(path),
            Err(_) => {}
        }
    }
}

/// Every pack in `packs`, lowest priority first, scanned for `folder`.
pub fn scan_packs(packs: &[PathBuf], folder: &str) -> Vec<PackListing> {
    packs.iter().map(|pack| PackListing::scan(pack, folder)).collect()
}

/// Each `Key: Value` line as `(key in lowercase, value)`, in order, with empty
/// values dropped. A line with no colon is an `Err` naming its number.
pub fn fields(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut found = Vec::new();
    // Some editors start a file with a byte-order mark, which would join the first key.
    for (number, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("line {}: no `Key: Value`", number + 1));
        };
        let value = value.trim();
        if !value.is_empty() {
            found.push((key.trim().to_ascii_lowercase(), value.to_owned()));
        }
    }
    Ok(found)
}

/// The value of `key` (lowercase), the last one if it repeats.
pub fn field(fields: &[(String, String)], key: &str) -> Option<String> {
    fields.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_skip_comments_and_blank_values_and_keep_colons_in_values() {
        let found = fields("\u{feff}# note\nName: A\n\nLicense File:\nSource: https://example.com/x\n").unwrap();
        assert_eq!(
            found,
            [("name".to_owned(), "A".to_owned()), ("source".to_owned(), "https://example.com/x".to_owned())]
        );
        assert_eq!(field(&found, "source").as_deref(), Some("https://example.com/x"));
        assert_eq!(fields("Name: A\nno colon").unwrap_err(), "line 2: no `Key: Value`");
    }
}
