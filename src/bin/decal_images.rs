//! Rasterizes the default pack's decal SVGs, `assets/default/decals/svg/*.svg`, into PNGs beside
//! the folder, at the size each kind is drawn. Run from the repo root:
//!
//! ```text
//! cargo run --bin decal_images
//! ```

use std::path::Path;

use grackle::common::icons::render_svg;

/// Pixels on a side, by the start of the file's name.
const SIZES: [(&str, u32); 3] = [("bullet_hole", 128), ("blood", 256), ("scorch", 256)];

fn main() {
    let dir = Path::new("assets/default/decals");
    let mut entries: Vec<_> = std::fs::read_dir(dir.join("svg")).expect("run from the repo root").flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "svg") {
            continue;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let size = SIZES.iter().find(|(prefix, _)| stem.starts_with(prefix)).map_or(256, |(_, size)| *size);
        let pixmap = render_svg(&std::fs::read(&path).unwrap(), size).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let out = dir.join(format!("{stem}.png"));
        pixmap.save_png(&out).unwrap();
        println!("{}", out.display());
    }
}
