//! Handing a path to the desktop: opening it with its default program, or showing it selected in
//! the file manager. Each platform's own command, run on the IO pool, since `explorer`, `open`
//! and `dbus-send` all ship with the system and a crate for this pulls in libdbus on Linux.
//! Native only: a browser tab has no file manager to ask.

use std::path::PathBuf;

use bevy::prelude::*;

#[derive(Message, Clone, Debug)]
pub struct RequestPathOpen {
    pub path: PathBuf,
    pub how: PathOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathOpen {
    /// With the program the desktop opens it with: a folder in the file manager.
    Open,
    /// Its folder in the file manager, with it selected. On a Linux desktop whose file manager
    /// does not answer `org.freedesktop.FileManager1`, the folder opens with nothing selected.
    Reveal,
}

pub struct PathOpenPlugin;

impl Plugin for PathOpenPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<RequestPathOpen>().add_systems(Update, handle_path_open_requests);
    }
}

pub fn handle_path_open_requests(mut requests: MessageReader<RequestPathOpen>) {
    for request in requests.read() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let request = request.clone();
            bevy::tasks::IoTaskPool::get()
                .spawn(async move {
                    if let Err(why) = native::open(&request) {
                        error!("Could not open {}: {why}", request.path.display());
                    }
                })
                .detach();
        }
        #[cfg(target_arch = "wasm32")]
        warn!("Could not open {}: a browser has no file manager", request.path.display());
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::Path;
    use std::process::Command;

    use super::{PathOpen, RequestPathOpen};

    /// Blocks until the command exits, so it is reaped and a failure can be reported.
    pub fn open(request: &RequestPathOpen) -> Result<(), String> {
        let path = std::path::absolute(&request.path).map_err(|e| e.to_string())?;
        if !path.exists() {
            return Err("it does not exist".to_owned());
        }
        match request.how {
            PathOpen::Open => open_path(&path),
            PathOpen::Reveal => reveal(&path),
        }
    }

    fn run(command: &mut Command) -> Result<(), String> {
        let status = command.status().map_err(|e| format!("{command:?}: {e}"))?;
        match status.success() {
            true => Ok(()),
            false => Err(format!("{command:?} exited with {status}")),
        }
    }

    #[cfg(target_os = "macos")]
    fn open_path(path: &Path) -> Result<(), String> {
        run(Command::new("open").arg(path))
    }

    #[cfg(target_os = "macos")]
    fn reveal(path: &Path) -> Result<(), String> {
        run(Command::new("open").arg("-R").arg(path))
    }

    #[cfg(target_os = "windows")]
    fn open_path(path: &Path) -> Result<(), String> {
        // Explorer's exit code is 1 whether or not it opened anything, so it is not checked.
        Command::new("explorer").arg(path).status().map(|_| ()).map_err(|e| e.to_string())
    }

    #[cfg(target_os = "windows")]
    fn reveal(path: &Path) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        // `/select,` and the path must reach explorer as one argument with the path quoted, which
        // `arg` would escape differently.
        Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .status()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn open_path(path: &Path) -> Result<(), String> {
        run(Command::new("xdg-open").arg(path))
    }

    /// The freedesktop file manager interface, which Nautilus, Dolphin, Nemo, Caja and Thunar
    /// answer; without it, the folder alone.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn reveal(path: &Path) -> Result<(), String> {
        let shown = run(Command::new("dbus-send")
            .args(["--session", "--print-reply", "--dest=org.freedesktop.FileManager1", "--type=method_call"])
            .args(["/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems"])
            .arg(format!("array:string:{}", file_uri(path)))
            .arg("string:"));
        match shown {
            Ok(()) => Ok(()),
            Err(_) => open_path(path.parent().unwrap_or(path)),
        }
    }

    /// `file://` and the path with every byte outside RFC 3986's unreserved set and `/`
    /// percent-encoded, as `ShowItems` takes URIs.
    #[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
    pub fn file_uri(path: &Path) -> String {
        let mut uri = String::from("file://");
        for byte in path.as_os_str().as_encoded_bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => uri.push(*byte as char),
                _ => uri.push_str(&format!("%{byte:02X}")),
            }
        }
        uri
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Breaks if a space or a non-ASCII name reaches the file manager unencoded.
        #[test]
        fn a_file_uri_encodes_what_is_not_plain() {
            assert_eq!(file_uri(Path::new("/home/a b/grackle_é.txt")), "file:///home/a%20b/grackle_%C3%A9.txt");
        }
    }
}
