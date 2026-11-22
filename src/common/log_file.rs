//! The second drain for the log: everything the console gets also goes to `logs/` under the
//! working directory, one file per launch, split by size, each line with the file and line that
//! wrote it. The appender deletes nothing on its own, so files older than
//! [`Settings::log_days`] go at startup; an unbounded `logs/` fills a server's disk. Native only:
//! a browser tab logs to its console.

use std::sync::OnceLock;

use bevy::prelude::*;

use crate::common::settings::LogLevel;
#[cfg(not(target_arch = "wasm32"))]
use crate::common::settings::Settings;

/// Bytes a file grows to before the next part of the same launch starts.
#[cfg(not(target_arch = "wasm32"))]
const SPLIT_AT: u64 = 32 * 1024 * 1024;
pub const DIRECTORY: &str = "logs";

static STARTUP_LEVEL: OnceLock<LogLevel> = OnceLock::new();

/// Called once, with the level `LogPlugin` is built with.
pub fn note_startup_level(level: LogLevel) {
    let _ = STARTUP_LEVEL.set(level);
}

/// `None` in a process that never set up the log, such as a test.
pub fn startup_level() -> Option<LogLevel> {
    STARTUP_LEVEL.get().copied()
}

/// What pruning could not delete, logged once the log exists.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct PruneProblems(Vec<String>);

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
    // The process id too: a server and a client launched together would share one file.
    let file = match SplitFile::open(std::path::Path::new(DIRECTORY), format!("grackle_{stamp}_{}", std::process::id())) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("Not logging to {DIRECTORY}/: {error}");
            return None;
        }
    };
    let days = app.world().get_resource::<Settings>().map_or(crate::common::settings::DEFAULT_LOG_DAYS, |settings| settings.log_days);
    let problems = prune(std::path::Path::new(DIRECTORY), std::time::Duration::from_secs(u64::from(days) * 24 * 60 * 60), std::time::SystemTime::now());
    if !problems.is_empty() {
        app.insert_resource(PruneProblems(problems));
        app.add_systems(Startup, report_prune_problems);
    }
    let (writer, guard) = tracing_appender::non_blocking(file);
    app.insert_resource(LogFileGuard(guard));
    Some(fmt::layer().with_writer(writer).with_ansi(false).with_target(true).with_file(true).with_line_number(true).boxed())
}

#[cfg(not(target_arch = "wasm32"))]
fn report_prune_problems(problems: Res<PruneProblems>, mut commands: Commands) {
    for problem in &problems.0 {
        warn!("{problem}");
    }
    commands.remove_resource::<PruneProblems>();
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

/// Deletes every `grackle_*` file in `logs/` last written more than `kept` before `now`: every
/// part of every launch's log, and the ECS dumps. Returns what it could not delete.
#[cfg(not(target_arch = "wasm32"))]
fn prune(directory: &std::path::Path, kept: std::time::Duration, now: std::time::SystemTime) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new() };
    let mut problems = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let ours = path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("grackle_"));
        if !ours || !path.is_file() {
            continue;
        }
        let Ok(written) = entry.metadata().and_then(|metadata| metadata.modified()) else { continue };
        if now.duration_since(written).is_ok_and(|age| age > kept)
            && let Err(error) = std::fs::remove_file(&path)
        {
            problems.push(format!("Could not remove the old log {}: {error}", path.display()));
        }
    }
    problems
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

    /// Breaks if a log past its days survives, a recent one goes, or something else is deleted.
    #[test]
    fn only_logs_past_their_days_are_deleted() {
        let directory = scratch("prune");
        std::fs::create_dir_all(&directory).unwrap();
        let now = std::time::SystemTime::now();
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        for (name, age) in [
            ("grackle_old.log", 10),
            ("grackle_ecsdump_old.txt", 10),
            ("grackle_recent.log", 2),
            ("notes.log", 10),
        ] {
            let file = std::fs::File::create(directory.join(name)).unwrap();
            file.set_modified(now - day * age).unwrap();
        }
        assert!(prune(&directory, day * 7, now).is_empty());
        let mut left: Vec<String> = std::fs::read_dir(&directory).unwrap().map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["grackle_recent.log", "notes.log"]);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
