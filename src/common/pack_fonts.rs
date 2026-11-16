//! Fonts in the packs: `fonts/{folder}/{file}`, beside
//! `fonts/{folder}/attribution.txt` and the license the attribution names.
//!
//! ```text
//! Name: Alfa Slab One
//! File: AlfaSlabOne-Regular.ttf
//! Designer: José María Solé
//! Copyright: Copyright 2016 The Alfa Slab One Project Authors (http://www.jmsole.cl)
//! License: SIL Open Font License, Version 1.1
//! License File: OFL.txt
//! ```
//!
//! `Name` and `File` are required, and `File` may be in a folder of its own
//! (`static/JetBrainsMono-Regular.ttf`). An unknown key is ignored so a pack
//! may record more. A font without an attribution is not found here, though
//! [`fonts`](crate::common::fonts) may still load it by path.

use std::path::PathBuf;

use crate::common::assets::AssetSource;
use crate::common::attribution::{self, PackListing, ATTRIBUTION_FILE};

pub const FONTS_DIR: &str = "fonts";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontAttribution {
    pub name: String,
    /// The face to load, relative to the font's folder.
    pub file: String,
    pub designer: Option<String>,
    pub copyright: Option<String>,
    pub license: Option<String>,
    pub license_file: Option<String>,
}

pub fn parse_attribution(text: &str) -> Result<FontAttribution, String> {
    let fields = attribution::fields(text)?;
    let get = |key| attribution::field(&fields, key);
    Ok(FontAttribution {
        name: get("name").ok_or("no `Name`")?,
        file: get("file").ok_or("no `File`")?,
        designer: get("designer"),
        copyright: get("copyright"),
        license: get("license"),
        license_file: get("license file"),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackFont {
    /// The folder under `fonts/`, which also names the font uniquely.
    pub folder: String,
    pub attribution: FontAttribution,
    /// The highest-priority pack holding the face.
    pub face_pack: PathBuf,
}

impl PackFont {
    /// Relative to the pack.
    pub fn face_path(&self) -> String {
        format!("{FONTS_DIR}/{}/{}", self.folder, self.attribution.file)
    }
}

/// Every font with an attribution in any pack, `listings` lowest priority
/// first, with the highest-priority pack's attribution of each folder, sorted
/// by name. A broken attribution or a missing face is skipped and returned as a
/// warning.
pub fn discover(listings: &[PackListing], source: &dyn AssetSource) -> (Vec<PackFont>, Vec<String>) {
    let mut attributed: std::collections::BTreeMap<&str, &PackListing> = Default::default();
    for listing in listings {
        for file in &listing.files {
            let folder = file
                .strip_prefix(FONTS_DIR)
                .and_then(|rest| rest.strip_prefix('/'))
                .and_then(|rest| rest.strip_suffix(ATTRIBUTION_FILE))
                .and_then(|rest| rest.strip_suffix('/'));
            if let Some(folder) = folder.filter(|folder| !folder.contains('/')) {
                attributed.insert(folder, listing);
            }
        }
    }

    let mut fonts = Vec::new();
    let mut warnings = Vec::new();
    for (folder, listing) in attributed {
        let path = format!("{FONTS_DIR}/{folder}/{ATTRIBUTION_FILE}");
        let parsed = source
            .read(&listing.pack, &path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| String::from_utf8(bytes).map_err(|e| e.to_string()))
            .and_then(|text| parse_attribution(&text));
        let attribution = match parsed {
            Ok(attribution) => attribution,
            Err(problem) => {
                warnings.push(format!("Skipping font {}/{path}: {problem}", listing.pack.display()));
                continue;
            }
        };
        let face = format!("{FONTS_DIR}/{folder}/{}", attribution.file);
        let Some(face_pack) = listings.iter().rev().find(|l| l.files.contains(&face)).map(|l| l.pack.clone()) else {
            warnings.push(format!("Skipping font {FONTS_DIR}/{folder}: no pack has {face}"));
            continue;
        };
        fonts.push(PackFont { folder: folder.to_owned(), attribution, face_pack });
    }
    fonts.sort_by(|a, b| {
        a.attribution.name.to_lowercase().cmp(&b.attribution.name.to_lowercase()).then_with(|| a.folder.cmp(&b.folder))
    });
    (fonts, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::assets::{default_packs, NativeFiles};
    use crate::common::attribution::scan_packs;
    use crate::common::music::tests::memory_packs;

    #[test]
    fn an_attribution_reads_its_keys_and_skips_the_rest() {
        let text = "# a comment\nName: Nova Cut\nfile:  NovaCut-Regular.ttf \n\nLicense File: OFL.txt\nCopyright: Copyright (c) 2011: wmk69\nSource: somewhere\n";
        let attribution = parse_attribution(text).unwrap();
        assert_eq!(attribution.name, "Nova Cut");
        assert_eq!(attribution.file, "NovaCut-Regular.ttf");
        assert_eq!(attribution.license_file.as_deref(), Some("OFL.txt"));
        assert_eq!(attribution.copyright.as_deref(), Some("Copyright (c) 2011: wmk69"));
        assert_eq!(attribution.designer, None);
        assert!(parse_attribution("File: a.ttf").is_err());
        assert!(parse_attribution("Name: A\nFile: a.ttf\nno colon").is_err());
    }

    #[test]
    fn discovery_sorts_by_name_and_skips_fonts_without_a_face() {
        let (listings, files) = memory_packs(&[
            (
                "default",
                &[
                    ("fonts/Zed/attribution.txt", "Name: Alpha\nFile: z.ttf"),
                    ("fonts/Zed/z.ttf", ""),
                    ("fonts/Bee/attribution.txt", "Name: beta\nFile: b.ttf"),
                    ("fonts/Bee/b.ttf", ""),
                    ("fonts/Gone/attribution.txt", "Name: Gone\nFile: g.ttf"),
                ],
            ),
            ("zz", &[("fonts/Bee/attribution.txt", "Name: Beta Override\nFile: b.ttf")]),
        ]);
        let (fonts, warnings) = discover(&listings, &files);
        let names: Vec<&str> = fonts.iter().map(|font| font.attribution.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Beta Override"]);
        assert_eq!(fonts[1].face_pack, PathBuf::from("default"));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    /// Breaks if a shipped font's attribution stops parsing or names a face or
    /// license that is not there.
    #[test]
    fn every_shipped_font_is_attributed() {
        let listings = scan_packs(&default_packs()[..1], FONTS_DIR);
        let (fonts, warnings) = discover(&listings, &NativeFiles);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!fonts.is_empty());
        let folders: Vec<&str> = listings[0]
            .files
            .iter()
            .filter_map(|file| file.strip_prefix("fonts/")?.split_once('/').map(|(folder, _)| folder))
            .collect();
        for folder in folders {
            assert!(fonts.iter().any(|font| font.folder == folder), "fonts/{folder} has no attribution.txt");
        }
        for font in &fonts {
            let attribution = &font.attribution;
            assert!(attribution.copyright.is_some() && attribution.license.is_some(), "{} lacks a copyright or license", font.folder);
            let license = attribution.license_file.as_deref().unwrap_or_else(|| panic!("{} names no license file", font.folder));
            let path = format!("{FONTS_DIR}/{}/{license}", font.folder);
            assert!(listings[0].files.contains(&path), "{} lacks {license}", font.folder);
        }
    }
}
