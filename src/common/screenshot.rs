//! The screenshot key: the primary window as drawn, HUD and egui included, saved to
//! `screenshots/` under the working directory as
//! `{YYYY}-{MM}-{DD}_{HH}-{MM}-{SS}_{discriminator}.png`. The discriminator is the
//! millisecond, four digits wide, raised past 999 when a file of that name already exists.
//! Native only: a browser tab has no working directory to write to.

use bevy::prelude::*;

pub const DIRECTORY: &str = "screenshots";

pub struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        crate::common::keybind::ensure(app);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Startup, || {
            native::ensure_directory();
        })
        .add_systems(Update, native::take_screenshot);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::{BufWriter, ErrorKind};
    use std::path::{Path, PathBuf};

    use bevy::prelude::*;
    use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
    use chrono::{DateTime, Local, Timelike};

    use super::DIRECTORY;
    use crate::common::keybind::{Action, Actions};

    pub fn ensure_directory() -> bool {
        match std::fs::create_dir_all(DIRECTORY) {
            Ok(()) => true,
            Err(error) => {
                warn!("Could not make {DIRECTORY}/: {error}");
                false
            }
        }
    }

    pub fn take_screenshot(actions: Res<Actions>, mut commands: Commands) {
        if !actions.just_pressed(Action::Screenshot) {
            return;
        }
        // Named at the press, not at the capture a frame or two later.
        let now = Local::now();
        commands.spawn(Screenshot::primary_window()).observe(move |captured: On<ScreenshotCaptured>| {
            let image = captured.image.clone();
            // PNG encoding a large window takes long enough to be felt as a hitch.
            bevy::tasks::IoTaskPool::get().spawn(async move { save(image, now) }).detach();
        });
    }

    fn save(image: Image, at: DateTime<Local>) {
        if !ensure_directory() {
            return;
        }
        let image = match image.try_into_dynamic() {
            // The surface's alpha is whatever the compositor left there, not transparency.
            Ok(image) => image.into_rgb8(),
            Err(error) => {
                warn!("Could not read the screenshot: {error}");
                return;
            }
        };
        let (path, file) = match claim(Path::new(DIRECTORY), at) {
            Ok(claimed) => claimed,
            Err(error) => {
                warn!("Could not make a screenshot file in {DIRECTORY}/: {error}");
                return;
            }
        };
        let written = image.write_to(&mut BufWriter::new(file), image::ImageFormat::Png);
        match written {
            Ok(()) => info!("Saved a screenshot to {}", path.display()),
            Err(error) => {
                warn!("Could not save a screenshot to {}: {error}", path.display());
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    /// Creates the first free name for `at`. `create_new` fails if the name exists, checked and
    /// created in one step, so two saves at once cannot take the same name. Each try is a higher
    /// number, so it ends once past the files already there.
    fn claim(directory: &Path, at: DateTime<Local>) -> std::io::Result<(PathBuf, std::fs::File)> {
        let mut discriminator = first_discriminator(at);
        loop {
            let path = name(directory, at, discriminator);
            match std::fs::File::create_new(&path) {
                Ok(file) => return Ok((path, file)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => discriminator += 1,
                Err(error) => return Err(error),
            }
        }
    }

    fn first_discriminator(at: DateTime<Local>) -> u32 {
        // `nanosecond` runs past a billion during a leap second.
        (at.nanosecond() / 1_000_000).min(999)
    }

    fn name(directory: &Path, at: DateTime<Local>, discriminator: u32) -> PathBuf {
        directory.join(format!("{}_{discriminator:04}.png", at.format("%Y-%m-%d_%H-%M-%S")))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use chrono::TimeZone;

        fn at(milliseconds: i64) -> DateTime<Local> {
            Local.with_ymd_and_hms(2026, 3, 7, 9, 5, 2).unwrap() + chrono::Duration::milliseconds(milliseconds)
        }

        #[test]
        fn a_screenshot_is_named_by_its_time_and_millisecond() {
            let at = at(42);
            assert_eq!(
                name(Path::new("shots"), at, first_discriminator(at)),
                Path::new("shots/2026-03-07_09-05-02_0042.png"),
            );
        }

        #[test]
        fn a_taken_name_moves_the_discriminator_on() {
            let directory = std::env::temp_dir().join(format!("grackle_screenshot_{}", std::process::id()));
            std::fs::create_dir_all(&directory).unwrap();
            let first = claim(&directory, at(999)).unwrap().0;
            let second = claim(&directory, at(999)).unwrap().0;
            std::fs::remove_dir_all(&directory).unwrap();
            assert!(first.ends_with("2026-03-07_09-05-02_0999.png"));
            assert!(second.ends_with("2026-03-07_09-05-02_1000.png"));
        }
    }
}
