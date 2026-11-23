//! The main menu's and the joining screen's backgrounds: a picture chosen at random from every
//! pack's [`TITLE_BACKGROUNDS_DIR`] or [`LOADING_BACKGROUNDS_DIR`] each time the screen goes up,
//! sized to cover the window. A loading picture is never the one shown the time before.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use rand::seq::IndexedRandom;

use crate::common::app_mode::AppMode;
use crate::common::attribution::{scan_packs, PackListing};
use crate::constants::{LOADING_BACKGROUNDS_DIR, TITLE_BACKGROUNDS_DIR};

/// Lower case only: the `AssetServer` picks a loader by the extension as written. JPEG needs Bevy's
/// `jpeg` feature.
const PICTURE_EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

pub struct TitleBackgroundPlugin;

impl Plugin for TitleBackgroundPlugin {
    fn build(&self, app: &mut App) {
        let shown = |mode: Res<State<AppMode>>| matches!(mode.get(), AppMode::MainMenu | AppMode::Joining);
        app.init_resource::<TitlePictures>().add_systems(Update, (hang_a_background, cover_the_window.run_if(shown)).chain());
    }
}

#[derive(Component)]
pub struct TitleBackground;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BackgroundSet {
    Title,
    Loading,
}

impl BackgroundSet {
    fn folder(self) -> &'static str {
        match self {
            BackgroundSet::Title => TITLE_BACKGROUNDS_DIR,
            BackgroundSet::Loading => LOADING_BACKGROUNDS_DIR,
        }
    }
}

/// On a new screen root, so each time the screen goes up is a new pick.
#[derive(Component)]
pub struct WantsBackground(pub BackgroundSet);

/// Scanned once per set, and every picture shown kept loaded, so returning to the menu reads no
/// disk and shows no plain color while a picture decodes.
#[derive(Resource, Default)]
struct TitlePictures {
    paths: std::collections::HashMap<BackgroundSet, Vec<String>>,
    loaded: std::collections::HashMap<String, Handle<Image>>,
    last_loading: Option<String>,
}

/// Every picture directly in [`TITLE_BACKGROUNDS_DIR`] across `listings` (lowest priority first), as
/// `AssetServer` paths. A higher pack's file replaces a lower one's at the same path.
pub fn title_backgrounds(listings: &[PackListing]) -> Vec<String> {
    backgrounds(listings, TITLE_BACKGROUNDS_DIR)
}

pub fn backgrounds(listings: &[PackListing], folder: &str) -> Vec<String> {
    let mut found: Vec<(String, String)> = Vec::new();
    for listing in listings {
        let directly_in = |file: &&String| {
            file.strip_prefix(folder).and_then(|rest| rest.strip_prefix('/')).is_some_and(|name| !name.contains('/'))
        };
        for file in listing.files.iter().filter(directly_in).filter(|file| is_picture(file)) {
            let Some(path) = crate::common::assets::asset_path(&listing.pack, file) else { continue };
            match found.iter_mut().find(|(relative, _)| relative == file) {
                Some(entry) => entry.1 = path,
                None => found.push((file.clone(), path)),
            }
        }
    }
    found.into_iter().map(|(_, path)| path).collect()
}

fn is_picture(file: &str) -> bool {
    std::path::Path::new(file)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| PICTURE_EXTENSIONS.contains(&extension))
}

/// The smallest box of `image`'s aspect that covers `window`, centered on it, as
/// `(left, top, size)` in the window's logical pixels.
pub fn cover(window: Vec2, image: Vec2) -> (Vec2, Vec2) {
    let scale = (window.x / image.x).max(window.y / image.y);
    let size = image * scale;
    ((window - size) / 2.0, size)
}

/// A loading picture other than `last`, unless it is the only one.
fn pick<'a>(paths: &'a [String], last: Option<&String>) -> Option<&'a String> {
    let others: Vec<&String> = paths.iter().filter(|path| Some(*path) != last).collect();
    match others.is_empty() {
        true => paths.first(),
        false => others.choose(&mut rand::rng()).copied(),
    }
}

fn hang_a_background(
    mut commands: Commands,
    roots: Query<(Entity, &WantsBackground), Added<WantsBackground>>,
    asset_server: Option<Res<AssetServer>>,
    mut pictures: ResMut<TitlePictures>,
) {
    for (root, wants) in &roots {
        let Some(asset_server) = &asset_server else { continue };
        let TitlePictures { paths, loaded, last_loading } = &mut *pictures;
        let mut scan = |set: BackgroundSet| -> Vec<String> {
            paths
                .entry(set)
                .or_insert_with(|| {
                    let packs: Vec<PathBuf> = crate::common::lang::default_packs();
                    backgrounds(&scan_packs(&packs, set.folder()), set.folder())
                })
                .clone()
        };
        let path = match wants.0 {
            BackgroundSet::Title => scan(BackgroundSet::Title).choose(&mut rand::rng()).cloned(),
            BackgroundSet::Loading => {
                let mut choices = scan(BackgroundSet::Loading);
                if choices.is_empty() {
                    choices = scan(BackgroundSet::Title);
                }
                let picked = pick(&choices, last_loading.as_ref()).cloned();
                last_loading.clone_from(&picked);
                picked
            }
        };
        let Some(path) = path else { continue };
        info!("The {:?} background is {path}", wants.0);
        let image = loaded.entry(path.clone()).or_insert_with(|| asset_server.load(path.clone())).clone();
        commands.entity(root).with_child((
            TitleBackground,
            ImageNode::new(image),
            // Under the menu's other nodes, over its plain background color.
            ZIndex(-1),
            Pickable::IGNORE,
            // Hidden until it is sized, so it never flashes at the image's own size.
            Visibility::Hidden,
            Node { position_type: PositionType::Absolute, ..default() },
        ));
    }
}

/// Every frame rather than on resize alone, since the picture's size is unknown until it loads.
/// Writes only when the box changes.
fn cover_the_window(
    window: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Option<Res<UiScale>>,
    images: Option<Res<Assets<Image>>>,
    mut backgrounds: Query<(&ImageNode, &mut Node, &mut Visibility), With<TitleBackground>>,
) {
    let (Ok(window), Some(images)) = (window.single(), images) else { return };
    // `Val::Px` is in logical pixels divided by `UiScale`.
    let scale = ui_scale.map_or(1.0, |scale| scale.0);
    let window = Vec2::new(window.width(), window.height()) / scale;
    for (image, mut node, mut visibility) in &mut backgrounds {
        let Some(picture) = images.get(&image.image) else { continue };
        let size = picture.size_f32();
        if size.x <= 0.0 || size.y <= 0.0 || window.x <= 0.0 || window.y <= 0.0 {
            continue;
        }
        let (corner, size) = cover(window, size);
        let wanted = (Val::Px(corner.x), Val::Px(corner.y), Val::Px(size.x), Val::Px(size.y));
        if (node.left, node.top, node.width, node.height) != wanted {
            (node.left, node.top, node.width, node.height) = wanted;
        }
        visibility.set_if_neq(Visibility::Inherited);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(pack: &str, files: &[&str]) -> PackListing {
        let files = files.iter().map(|file| format!("images/backgrounds/title/{file}")).collect();
        PackListing { pack: pack.into(), files }
    }

    /// Breaks if a picture is missed, one that would not load or sits in a subfolder is offered, or
    /// a higher pack's copy does not win.
    #[test]
    fn every_packs_pictures_are_offered_and_the_higher_pack_wins() {
        let dir = "images/backgrounds/title";
        let found = title_backgrounds(&[
            listing("assets/default", &["dusk.png", "noon.jpg", "LOUD.JPG", "attribution.txt", "unused/draft.png"]),
            listing("assets/user", &["dusk.png", "mine.jpeg"]),
            listing("elsewhere/pack", &["lost.png"]),
        ]);
        assert_eq!(
            found,
            [format!("user/{dir}/dusk.png"), format!("default/{dir}/noon.jpg"), format!("user/{dir}/mine.jpeg")]
        );
    }

    /// Breaks if the box does not cover the window, loses the picture's aspect, or is off center.
    #[test]
    fn the_background_covers_the_window_at_its_own_aspect() {
        for (window, image) in [
            (Vec2::new(1920.0, 1080.0), Vec2::new(1000.0, 1000.0)),
            (Vec2::new(800.0, 1200.0), Vec2::new(1920.0, 1080.0)),
            (Vec2::new(1280.0, 720.0), Vec2::new(640.0, 360.0)),
        ] {
            let (corner, size) = cover(window, image);
            assert!(size.x >= window.x - 1e-3 && size.y >= window.y - 1e-3, "{window} not covered by {size}");
            assert!((size.x - window.x).abs() < 1e-3 || (size.y - window.y).abs() < 1e-3, "{size} larger than it needs to be for {window}");
            assert!((size.x / size.y - image.x / image.y).abs() < 1e-4, "{size} lost {image}'s aspect");
            assert!((corner * 2.0 + size - window).length() < 1e-3, "{corner} {size} is off center in {window}");
        }
    }

    #[test]
    fn a_loading_picture_is_never_the_last_one_shown() {
        let paths: Vec<String> = ["a", "b", "c"].map(str::to_owned).to_vec();
        for _ in 0..50 {
            assert_ne!(pick(&paths, Some(&paths[1])), Some(&paths[1]));
        }
        let one = vec!["a".to_owned()];
        assert_eq!(pick(&one, Some(&one[0])), Some(&one[0]), "with one picture, it is shown again");
        assert_eq!(pick(&[], None), None);
    }
}
