//! Where a cache about one pack file goes: `cache/{kind}/{pack}/...`, parallel to the packs, so
//! what is cached about a file can be found from the file's path. Caches keyed by a content hash
//! (lightmaps, nav graphs) are not filed this way.

use std::path::{Component, Path, PathBuf};

use crate::common::assets::ASSET_ROOT;

pub const CACHE_ROOT: &str = "cache";

/// The pack folder for a document with no file, or a file under no pack.
pub const NO_PACK: &str = "_no_pack";

/// The name folder for a document with no file.
pub const UNTITLED: &str = "untitled";

/// The pack's folder name and the path inside it, of a file under `assets/{pack}/`. A path from a
/// file dialog is absolute, so it is taken relative to the working directory first.
pub fn in_pack(file: &Path) -> Option<(String, PathBuf)> {
    let relative = match file.is_absolute() {
        true => file.strip_prefix(std::env::current_dir().ok()?).ok()?,
        false => file,
    };
    let mut parts = relative.strip_prefix(ASSET_ROOT).ok()?.components();
    let Some(Component::Normal(pack)) = parts.next() else { return None };
    let rest = parts.as_path();
    if rest.as_os_str().is_empty() || rest.components().any(|part| !matches!(part, Component::Normal(_))) {
        return None;
    }
    Some((pack.to_str()?.to_owned(), rest.to_path_buf()))
}

/// `cache/{kind}/{pack}/{stem}`, a folder for what is cached about one document. A document with
/// no file, or one under no pack, is filed under [`NO_PACK`].
pub fn document_folder(kind: &str, file: Option<&Path>) -> PathBuf {
    let stem = file.and_then(|file| file.file_stem()).map_or_else(|| UNTITLED.to_owned(), |stem| stem.to_string_lossy().into_owned());
    let pack = file.and_then(in_pack).map_or_else(|| NO_PACK.to_owned(), |(pack, _)| pack);
    Path::new(CACHE_ROOT).join(kind).join(pack).join(stem)
}

/// `cache/{kind}/{pack}/{path in the pack}` with `extension` in place of the file's own; `None` for
/// a file under no pack.
pub fn mirrored_file(kind: &str, file: &Path, extension: &str) -> Option<PathBuf> {
    let (pack, rest) = in_pack(file)?;
    Some(Path::new(CACHE_ROOT).join(kind).join(pack).join(rest).with_extension(extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pack_file_is_mirrored_under_its_pack() {
        let prop = Path::new("assets/default/props/rocket_launcher.gpp");
        assert_eq!(in_pack(prop), Some(("default".into(), PathBuf::from("props/rocket_launcher.gpp"))));
        assert_eq!(mirrored_file("thumbnails", prop, "png"), Some(PathBuf::from("cache/thumbnails/default/props/rocket_launcher.png")));
        let absolute = std::env::current_dir().unwrap().join("assets/user/blueprints/hall.gmb");
        assert_eq!(document_folder("postcards", Some(&absolute)), PathBuf::from("cache/postcards/user/hall"));
    }

    #[test]
    fn a_file_outside_the_packs_is_filed_under_no_pack() {
        assert_eq!(in_pack(Path::new("/tmp/elsewhere.gmb")), None);
        assert_eq!(in_pack(Path::new("assets/../escape.gmb")), None);
        assert_eq!(in_pack(Path::new("assets/default")), None);
        assert_eq!(document_folder("rounds", Some(Path::new("/tmp/elsewhere.gmb"))), PathBuf::from("cache/rounds/_no_pack/elsewhere"));
        assert_eq!(document_folder("rounds", None), PathBuf::from("cache/rounds/_no_pack/untitled"));
    }
}
