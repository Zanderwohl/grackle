//! The icons a death is drawn with in the kill feed: square white SVGs at `icons/{name}.svg` in
//! any pack, as the toolbar's are, rasterized once per name into an `Image` that a UI node tints.
//!
//! Afterburn draws [`FLAME`] and a blast's splash [`EXPLOSION`], whatever set them off. The map,
//! a suicide, a team switch, and a weapon with no icon (or one whose file is missing) draw
//! [`SKULL`].

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use resvg::usvg;

use crate::common::app_mode::AppMode;
use crate::common::damage::{Cause, How};
use crate::common::icons::{demultiplied, render_svg};
use crate::common::weapon::WeaponCatalog;

pub const SKULL: &str = "kill_skull";
pub const EXPLOSION: &str = "kill_explosion";
pub const FLAME: &str = "kill_flame";
/// Texels on a side: sharp at the feed's size up to a display scale of two.
const TEXELS: u32 = 64;

pub fn icon_for<'a>(cause: &Cause, catalog: Option<&'a WeaponCatalog>) -> &'a str {
    match cause.how {
        How::Burn => FLAME,
        How::Splash | How::Exploded => EXPLOSION,
        How::Trigger | How::Kill => SKULL,
        _ => cause.weapon.and_then(|weapon| catalog?.get(weapon)?.icon.as_deref()).unwrap_or(SKULL),
    }
}

pub struct KillIconPlugin;

impl Plugin for KillIconPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KillIcons>().add_systems(OnEnter(AppMode::Play), draw_the_icons_ahead);
    }
}

/// Each name asked for since the round began, read once whether or not a pack has it.
#[derive(Resource, Default)]
pub struct KillIcons {
    images: HashMap<String, Option<Handle<Image>>>,
}

/// Forgets every icon, so one added or changed on disk since is read again, and draws the ones a
/// death can name now rather than in the frame somebody dies.
fn draw_the_icons_ahead(
    mut icons: ResMut<KillIcons>,
    catalog: Option<Res<WeaponCatalog>>,
    pack: Option<Res<crate::common::assets::Assets>>,
    images: Option<ResMut<Assets<Image>>>,
) {
    icons.images.clear();
    let (Some(pack), Some(mut images)) = (pack, images) else { return };
    let named = catalog.iter().flat_map(|catalog| catalog.weapons.iter().filter_map(|weapon| weapon.icon.clone()));
    for name in named.collect::<Vec<_>>().iter().map(String::as_str).chain([SKULL, EXPLOSION, FLAME]) {
        icons.get(name, |relative| pack.read_from_packs(relative), &mut images);
    }
}

impl KillIcons {
    /// `name`'s image, else the skull's, else `None`.
    pub fn get(&mut self, name: &str, read: impl Fn(&str) -> Option<Vec<u8>>, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        for candidate in [name, SKULL] {
            let image = self.images.entry(candidate.to_owned()).or_insert_with(|| {
                let bytes = read(&format!("icons/{candidate}.svg"))?;
                rasterize(&bytes).map(|image| images.add(image)).inspect_err(|e| warn!("icons/{candidate}.svg: {e}")).ok()
            });
            if let Some(image) = image {
                return Some(image.clone());
            }
        }
        None
    }
}

/// White where the SVG is, so `ImageNode::color` is the color it is drawn in.
pub fn rasterize(svg: &[u8]) -> Result<Image, usvg::Error> {
    let pixmap = render_svg(svg, TEXELS)?;
    let rgba = demultiplied(&pixmap).flatten().collect();
    Ok(Image::new(
        Extent3d { width: TEXELS, height: TEXELS, depth_or_array_layers: 1 },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::weapon::WeaponId;

    fn shipped(relative: &str) -> Option<Vec<u8>> {
        std::fs::read(format!("assets/default/{relative}")).ok()
    }

    /// Breaks if afterburn, splash or the map stop drawing their own icons, or a weapon's icon
    /// is not what a direct hit, a shot or a flame draws.
    #[test]
    fn each_kind_of_death_draws_its_icon() {
        let catalog = crate::common::weapon_file::default_catalog();
        let rocket = Some(WeaponId::of("rocket_launcher"));
        let icon = |weapon, how| icon_for(&Cause::of(weapon, how), Some(&catalog));
        assert_eq!(icon(rocket, How::Direct), "kill_rocket");
        assert_eq!(icon(rocket, How::Splash), EXPLOSION);
        assert_eq!(icon(Some(WeaponId::of("flamethrower")), How::Flame), "kill_flamethrower");
        assert_eq!(icon(Some(WeaponId::of("flamethrower")), How::Burn), FLAME);
        assert_eq!(icon(None, How::Trigger), SKULL);
        assert_eq!(icon(rocket, How::Kill), SKULL, "a suicide is not the weapon held");
        assert_eq!(icon(Some(WeaponId::of("medi_wrench")), How::Other), SKULL, "no icon of its own");
        assert_eq!(icon_for(&Cause::of(rocket, How::Direct), None), SKULL, "no catalog yet");
    }

    /// Breaks if a shipped weapon names an icon no pack has, or one does not draw.
    #[test]
    fn every_icon_a_shipped_weapon_names_draws() {
        let catalog = crate::common::weapon_file::default_catalog();
        let mut named = 0;
        for name in catalog.weapons.iter().filter_map(|weapon| weapon.icon.as_deref()).chain([SKULL, EXPLOSION, FLAME]) {
            let bytes = shipped(&format!("icons/{name}.svg")).unwrap_or_else(|| panic!("no icons/{name}.svg"));
            let image = rasterize(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(image.data.as_ref().unwrap().chunks(4).any(|pixel| pixel[3] > 0), "{name} is empty");
            named += 1;
        }
        assert!(named > 3, "no shipped weapon names an icon");
    }

    #[test]
    fn a_missing_icon_draws_the_skull_and_is_read_once() {
        let mut images = Assets::<Image>::default();
        let mut icons = KillIcons::default();
        let reads = std::cell::Cell::new(0);
        let read = |relative: &str| {
            reads.set(reads.get() + 1);
            shipped(relative)
        };
        let skull = icons.get(SKULL, read, &mut images);
        assert!(skull.is_some());
        assert_eq!(icons.get("kill_nothing", read, &mut images), skull);
        assert_eq!(icons.get("kill_nothing", read, &mut images), skull);
        assert_eq!(reads.get(), 2);
    }
}
