//! Deleting what servers sent once no recent session used it. Runs at startup, as the logs' and the
//! trash's cleanup do. A session record lists every object it used
//! ([`crate::common::resource::sync`]); an object no kept record lists goes, and while what is
//! kept is over [`SIZE_CAP`] the oldest records go first.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use bevy::prelude::*;

use crate::common::resource::sync::{DownloadsRoot, SessionRecord};

pub const SIZE_CAP: u64 = 1024 * 1024 * 1024;

/// Inserted by the app binary only, so tests that share a downloads folder never prune it under
/// each other.
#[derive(Resource, Default)]
pub struct PruneDownloads;

pub fn prune_old_downloads(root: Res<DownloadsRoot>, settings: Option<Res<crate::common::settings::Settings>>) {
    let days = settings.map_or(crate::common::settings::DEFAULT_DOWNLOAD_DAYS, |settings| settings.download_days);
    let root = root.0.clone();
    let keep = Duration::from_secs(u64::from(days) * 24 * 60 * 60);
    #[cfg(not(target_arch = "wasm32"))]
    bevy::tasks::IoTaskPool::get()
        .spawn(async move {
            for problem in prune(&root, keep, SIZE_CAP, SystemTime::now()) {
                warn!("{problem}");
            }
        })
        .detach();
    #[cfg(target_arch = "wasm32")]
    let _ = (root, keep);
}

struct Record {
    file: PathBuf,
    used: SystemTime,
    objects: BTreeSet<String>,
}

fn records(root: &Path, problems: &mut Vec<String>) -> Vec<Record> {
    let mut found = Vec::new();
    for server in std::fs::read_dir(root.join("sessions")).into_iter().flatten().filter_map(Result::ok) {
        for file in std::fs::read_dir(server.path()).into_iter().flatten().filter_map(Result::ok).map(|entry| entry.path()) {
            let used = std::fs::metadata(&file).and_then(|metadata| metadata.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            let read = std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|text| toml::from_str::<SessionRecord>(&text).map_err(|e| e.to_string()));
            match read {
                Ok(record) => found.push(Record { file, used, objects: record.objects }),
                Err(e) => problems.push(format!("Ignoring the session record {}: {e}", file.display())),
            }
        }
    }
    found.sort_by_key(|record| record.used);
    found
}

/// Every object in the store, by its name there (`ab/cdef….ext`), with its size.
fn objects(root: &Path) -> Vec<(String, PathBuf, u64)> {
    let mut found = Vec::new();
    for folder in std::fs::read_dir(root.join("objects")).into_iter().flatten().filter_map(Result::ok) {
        let prefix = folder.file_name().to_string_lossy().into_owned();
        for file in std::fs::read_dir(folder.path()).into_iter().flatten().filter_map(Result::ok) {
            let size = file.metadata().map_or(0, |metadata| metadata.len());
            found.push((format!("{prefix}/{}", file.file_name().to_string_lossy()), file.path(), size));
        }
    }
    found
}

/// Returns what could not be done, for the log.
pub fn prune(root: &Path, keep: Duration, cap: u64, now: SystemTime) -> Vec<String> {
    let mut problems = Vec::new();
    let mut kept = records(root, &mut problems);
    let remove = |path: &Path, problems: &mut Vec<String>| {
        if let Err(e) = std::fs::remove_file(path) {
            problems.push(format!("Could not delete {}: {e}", path.display()));
        }
    };
    kept.retain(|record| {
        let fresh = now.duration_since(record.used).unwrap_or_default() <= keep;
        if !fresh {
            remove(&record.file, &mut problems);
        }
        fresh
    });

    let stored = objects(root);
    let size_of = |kept: &[Record]| -> u64 {
        let wanted: BTreeSet<&String> = kept.iter().flat_map(|record| &record.objects).collect();
        stored.iter().filter(|(name, _, _)| wanted.contains(name)).map(|(_, _, size)| size).sum()
    };
    while size_of(&kept) > cap && !kept.is_empty() {
        let oldest = kept.remove(0);
        remove(&oldest.file, &mut problems);
    }

    let wanted: BTreeSet<&String> = kept.iter().flat_map(|record| &record.objects).collect();
    for (name, path, _) in &stored {
        if !wanted.contains(name) {
            remove(path, &mut problems);
        }
    }
    problems
}

/// Settings → Developer's button.
pub fn delete_all_downloads(root: &Path) {
    for folder in ["objects", "sessions"] {
        let path = root.join(folder);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => info!("Deleted {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("Could not delete {}: {e}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn record(objects: &[&str]) -> String {
        let objects = objects.iter().map(|name| format!("{name:?}")).collect::<Vec<_>>().join(", ");
        format!("server = \"a\"\nstarted = \"x\"\nrounds = []\nobjects = [{objects}]\n")
    }

    #[test]
    fn an_object_no_recent_session_used_is_deleted() {
        let root = std::env::temp_dir().join(format!("grackle-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root.join("objects/aa/kept.gpp"), b"kept");
        write(&root.join("objects/bb/old.gpp"), b"old");
        write(&root.join("objects/cc/stray.gpp"), b"stray");
        write(&root.join("sessions/a/new.toml"), record(&["aa/kept.gpp"]).as_bytes());
        write(&root.join("sessions/a/old.toml"), record(&["bb/old.gpp"]).as_bytes());

        // A day later, keeping a day and a half: both records are fresh.
        let later = SystemTime::now() + Duration::from_secs(24 * 60 * 60);
        assert!(prune(&root, Duration::from_secs(36 * 60 * 60), SIZE_CAP, later).is_empty());
        assert!(root.join("objects/bb/old.gpp").exists());
        assert!(!root.join("objects/cc/stray.gpp").exists(), "an object no record lists was kept");

        // Two days later, keeping one: both go.
        let much_later = SystemTime::now() + Duration::from_secs(2 * 24 * 60 * 60);
        assert!(prune(&root, Duration::from_secs(24 * 60 * 60), SIZE_CAP, much_later).is_empty());
        assert!(!root.join("objects/aa/kept.gpp").exists() && !root.join("sessions/a/new.toml").exists());
    }

    #[test]
    fn the_oldest_session_goes_first_when_over_the_cap() {
        let root = std::env::temp_dir().join(format!("grackle-prune-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root.join("objects/aa/first.gpp"), &[0; 10]);
        write(&root.join("sessions/a/first.toml"), record(&["aa/first.gpp"]).as_bytes());
        std::thread::sleep(Duration::from_millis(20));
        write(&root.join("objects/bb/second.gpp"), &[0; 10]);
        write(&root.join("sessions/a/second.toml"), record(&["bb/second.gpp"]).as_bytes());
        assert!(prune(&root, Duration::from_secs(60), 15, SystemTime::now()).is_empty());
        assert!(!root.join("objects/aa/first.gpp").exists(), "the older session's object was kept over the cap");
        assert!(root.join("objects/bb/second.gpp").exists());
    }
}
