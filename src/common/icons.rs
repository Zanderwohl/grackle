//! Toolbar icons: square SVGs at `icons/{name}.svg` in any pack, drawn white
//! so a button can tint them with its text color.
//!
//! A pack cannot be listed, so an icon is read the first time a button asks
//! for it by name. A name with no file draws [`FALLBACK`]; with neither, the
//! toolbar draws its placeholder.

use std::collections::HashSet;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::common::assets::Assets;

/// For tools that have no icon of their own yet.
pub const FALLBACK: &str = "hammer";
/// Points, not pixels: the SVG is rasterized at this times the display scale.
pub const ICON_SIZE: f32 = 16.0;
/// Drawn when not even the fallback icon loaded.
pub const PLACEHOLDER: &str = "□";

pub struct IconsPlugin;

impl Plugin for IconsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Assets>().add_systems(EguiPrimaryContextPass, load_wanted_icons);
    }
}

/// Kept in egui's memory, because the widgets asking for icons get only a `Ui`.
#[derive(Clone, Default)]
struct IconTable {
    loaded: HashSet<String>,
    tried: HashSet<String>,
    wanted: Vec<String>,
}

impl IconTable {
    fn want(&mut self, name: &str) {
        if !self.tried.contains(name) && !self.wanted.iter().any(|wanted| wanted == name) {
            self.wanted.push(name.to_owned());
        }
    }

    fn resolve(&self, name: &str) -> Option<String> {
        [name, FALLBACK].into_iter().find(|candidate| self.loaded.contains(*candidate)).map(uri)
    }
}

fn table_id() -> egui::Id {
    egui::Id::new("grackle_icons")
}

/// `.svg` on the end is what routes it to the SVG loader.
fn uri(name: &str) -> String {
    format!("bytes://icons/{name}.svg")
}

/// What to draw for `name`, or `None` for the placeholder.
pub fn image(ctx: &egui::Context, name: &str) -> Option<egui::Image<'static>> {
    let uri = ctx.data_mut(|data| {
        let table = data.get_temp_mut_or_default::<IconTable>(table_id());
        table.want(name);
        table.resolve(name)
    })?;
    Some(egui::Image::new(uri).fit_to_exact_size(egui::vec2(ICON_SIZE, ICON_SIZE)))
}

/// `name`'s icon, or the placeholder, in a slot [`ICON_SIZE`] square either
/// way so the text after it lines up.
pub fn atom(ctx: &egui::Context, name: &str) -> egui::Atom<'static> {
    use egui::AtomExt;
    let atom: egui::Atom<'static> = match image(ctx, name) {
        Some(image) => image.into(),
        None => PLACEHOLDER.into(),
    };
    atom.atom_size(egui::vec2(ICON_SIZE, ICON_SIZE))
}

fn load_wanted_icons(mut contexts: EguiContexts, assets: Res<Assets>, mut installed: Local<bool>) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    if !*installed {
        egui_extras::install_image_loaders(ctx);
        *installed = true;
    }
    fetch(ctx, |relative| assets.read_from_packs(relative));
}

/// Reads every icon asked for since the last call, and the fallback.
fn fetch(ctx: &egui::Context, read: impl Fn(&str) -> Option<Vec<u8>>) {
    let wanted = ctx.data_mut(|data| {
        let table = data.get_temp_mut_or_default::<IconTable>(table_id());
        table.want(FALLBACK);
        std::mem::take(&mut table.wanted)
    });
    if wanted.is_empty() {
        return;
    }
    let mut loaded = Vec::new();
    for name in &wanted {
        match read(&format!("icons/{name}.svg")) {
            Some(bytes) => {
                ctx.include_bytes(uri(name), bytes);
                loaded.push(name.clone());
            }
            None if name == FALLBACK => warn!("No pack has icons/{FALLBACK}.svg; tools without icons draw a box"),
            None => {}
        }
    }
    ctx.data_mut(|data| {
        let table = data.get_temp_mut_or_default::<IconTable>(table_id());
        table.tried.extend(wanted);
        table.loaded.extend(loaded);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped(name: &str) -> Option<Vec<u8>> {
        std::fs::read(format!("assets/default/{name}")).ok()
    }

    #[test]
    fn a_missing_icon_falls_back_to_the_hammer_and_then_to_nothing() {
        let ctx = egui::Context::default();
        assert!(image(&ctx, "room").is_none());

        fetch(&ctx, |_| None);
        assert!(image(&ctx, "room").is_none());

        let ctx = egui::Context::default();
        image(&ctx, "room");
        fetch(&ctx, shipped);
        let room = ctx.data_mut(|data| data.get_temp_mut_or_default::<IconTable>(table_id()).resolve("room"));
        assert_eq!(room, Some(uri(FALLBACK)));
    }

    #[test]
    fn an_icon_is_read_once() {
        let ctx = egui::Context::default();
        let reads = std::cell::Cell::new(0);
        for _ in 0..3 {
            image(&ctx, "room");
            fetch(&ctx, |_| {
                reads.set(reads.get() + 1);
                None
            });
        }
        // The fallback and `room`, each once.
        assert_eq!(reads.get(), 2);
    }

    /// Each shipped icon must be a square SVG the loader can draw, or it is a
    /// box on every toolbar with nothing to say why.
    #[test]
    fn every_shipped_icon_is_a_square_svg() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let mut count = 0;
        for entry in std::fs::read_dir("assets/default/icons").unwrap() {
            let path = entry.unwrap().path();
            let uri = format!("bytes://{}", path.display());
            ctx.include_bytes(uri.clone(), std::fs::read(&path).unwrap());
            let hint = egui::load::SizeHint::Size { width: 64, height: 64, maintain_aspect_ratio: true };
            let Ok(egui::load::ImagePoll::Ready { image }) = ctx.try_load_image(&uri, hint) else {
                panic!("{} does not load", path.display());
            };
            assert_eq!(image.size[0], image.size[1], "{} is not square", path.display());
            assert!(image.pixels.iter().any(|pixel| pixel.a() > 0), "{} is empty", path.display());
            count += 1;
        }
        assert!(count > 0);
        assert!(shipped(&format!("icons/{FALLBACK}.svg")).is_some());
    }
}


