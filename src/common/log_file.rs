//! The second drain for the log: everything the console gets also goes to `logs/` under the
//! working directory, one file per launch, split by size. The appender deletes nothing on its
//! own, so the oldest files past [`KEPT_FILES`] go at startup; an unbounded `logs/` fills a
//! server's disk. Native only: a browser tab logs to its console.

use bevy::prelude::*;

/// Bytes a file grows to before the next part of the same launch starts.
#[cfg(not(target_arch = "wasm32"))]
const SPLIT_AT: u64 = 32 * 1024 * 1024;
/// Files kept in `logs/`, every part of every launch counted, newest by name.
#[cfg(not(target_arch = "wasm32"))]
const KEPT_FILES: usize = 30;
const DIRECTORY: &str = "logs";

/// Holds the background writer open; dropping it flushes what is queued.
#[derive(Resource)]
#[allow(dead_code)]
struct LogFileGuard(#[cfg(not(target_arch = "wasm32"))] tracing_appender::non_blocking::WorkerGuard);

/// For `LogPlugin::custom_layer`. A log that cannot be opened is a line on the console, not a
/// failure to start.
#[cfg(not(target_arch = "wasm32"))]
pub fn layer(app: &mut App) -> Option<bevy::log::BoxedLayer> {
    use bevy::log::tracing_subscriber::{fmt, Layer};

    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let file = match SplitFile::open(std::path::Path::new(DIRECTORY), format!("grackle_{stamp}")) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("Not logging to {DIRECTORY}/: {error}");
            return None;
        }
    };
    prune(std::path::Path::new(DIRECTORY), KEPT_FILES);
    let (writer, guard) = tracing_appender::non_blocking(file);
    app.insert_resource(LogFileGuard(guard));
    Some(fmt::layer().with_writer(writer).with_ansi(false).boxed())
}

#[cfg(target_arch = "wasm32")]
pub fn layer(_: &mut App) -> Option<bevy::log::BoxedLayer> {
    None
}

/// `logs/{stem}.log`, then `{stem}.2.log` and on once each reaches [`SPLIT_AT`]. A line is never
/// split across two files: the check is made before each write, and the fmt layer writes a line
/// at a time.
#[cfg(not(target_arch = "wasm32"))]
struct SplitFile {
    directory: std::path::PathBuf,
    stem: String,
    part: u32,
    written: u64,
    file: std::fs::File,
}

#[cfg(not(target_arch = "wasm32"))]
impl SplitFile {
    fn open(directory: &std::path::Path, stem: String) -> std::io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let file = Self::create(directory, &stem, 1)?;
        Ok(SplitFile { directory: directory.to_owned(), stem, part: 1, written: 0, file })
    }

    fn path(directory: &std::path::Path, stem: &str, part: u32) -> std::path::PathBuf {
        match part {
            1 => directory.join(format!("{stem}.log")),
            part => directory.join(format!("{stem}.{part}.log")),
        }
    }

    fn create(directory: &std::path::Path, stem: &str, part: u32) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new().create(true).append(true).open(Self::path(directory, stem, part))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl std::io::Write for SplitFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.written > 0 && self.written + bytes.len() as u64 > SPLIT_AT {
            self.file.flush()?;
            self.part += 1;
            self.file = Self::create(&self.directory, &self.stem, self.part)?;
            self.written = 0;
        }
        let wrote = self.file.write(bytes)?;
        self.written += wrote as u64;
        Ok(wrote)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Deletes all but the newest `kept` logs. Names start with the launch's stamp, so their order is
/// their age; a part sorts after its first file, which only matters within one launch.
#[cfg(not(target_arch = "wasm32"))]
fn prune(directory: &std::path::Path, kept: usize) {
    let Ok(entries) = std::fs::read_dir(directory) else { return };
    let mut logs: Vec<std::path::PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
        .filter(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("grackle_")))
        .collect();
    logs.sort();
    let surplus = logs.len().saturating_sub(kept);
    for old in logs.into_iter().take(surplus) {
        if let Err(error) = std::fs::remove_file(&old) {
            eprintln!("Could not remove {}: {error}", old.display());
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::io::Write;

    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("grackle_log_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    /// Breaks if a full file keeps growing, or a line is split across two.
    #[test]
    fn a_full_file_goes_on_in_the_next_part() {
        let directory = scratch("split");
        let mut file = SplitFile::open(&directory, "grackle_x".into()).unwrap();
        file.written = SPLIT_AT - 4;
        file.write_all(b"a long line\n").unwrap();
        file.flush().unwrap();
        assert_eq!(std::fs::read_to_string(directory.join("grackle_x.2.log")).unwrap(), "a long line\n");
        assert_eq!(std::fs::read_to_string(directory.join("grackle_x.log")).unwrap(), "");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    /// Breaks if the oldest logs survive past the cap, or something else in the folder is deleted.
    #[test]
    fn only_the_newest_logs_are_kept() {
        let directory = scratch("prune");
        std::fs::create_dir_all(&directory).unwrap();
        for name in ["grackle_2026-01-01.log", "grackle_2026-01-02.log", "grackle_2026-01-03.log", "notes.log"] {
            std::fs::write(directory.join(name), "").unwrap();
        }
        prune(&directory, 2);
        let mut left: Vec<String> = std::fs::read_dir(&directory).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["grackle_2026-01-02.log", "grackle_2026-01-03.log", "notes.log"]);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
