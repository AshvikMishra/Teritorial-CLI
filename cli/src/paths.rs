//! Where settings, saves and replays live, without the `dirs` crate.
//!
//! * Linux and macOS: `$XDG_CONFIG_HOME` (or `~/.config`) + `/territorial/settings.json`,
//!   and `$XDG_DATA_HOME` (or `~/.local/share`) + `/territorial/{saves,replays}`.
//! * Windows: `%APPDATA%\territorial\settings.json` and `%APPDATA%\territorial\{saves,replays}`.
//!
//! Directories are only created when something is written.

use std::ffi::OsString;
use std::path::PathBuf;

const APP_DIR: &str = "territorial";

/// Resolved file locations. A `None` means the base directory could not be
/// determined (no `HOME` / `APPDATA`), so that kind of file cannot be stored.
#[derive(Clone, Debug)]
pub struct Paths {
    /// The settings JSON file.
    pub settings_file: Option<PathBuf>,
    /// Directory of saved matches (`<name>.json`).
    pub saves_dir: Option<PathBuf>,
    /// Directory of replays.
    pub replays_dir: Option<PathBuf>,
}

impl Paths {
    /// Paths for this platform and environment.
    pub fn detect() -> Paths {
        Paths::from_env(|key| std::env::var_os(key), cfg!(windows))
    }

    /// Paths computed from an environment lookup (`var`) for Windows or Unix rules.
    pub fn from_env<F>(var: F, windows: bool) -> Paths
    where
        F: Fn(&str) -> Option<OsString>,
    {
        let config_root: Option<PathBuf>;
        let data_root: Option<PathBuf>;
        if windows {
            let appdata: Option<PathBuf> = match non_empty(var("APPDATA")) {
                Some(dir) => Some(PathBuf::from(dir)),
                None => non_empty(var("USERPROFILE"))
                    .map(|home| PathBuf::from(home).join("AppData").join("Roaming")),
            };
            config_root = appdata.clone();
            data_root = appdata;
        } else {
            let home: Option<PathBuf> = non_empty(var("HOME")).map(PathBuf::from);
            config_root = match absolute(var("XDG_CONFIG_HOME")) {
                Some(dir) => Some(dir),
                None => home.as_ref().map(|h| h.join(".config")),
            };
            data_root = match absolute(var("XDG_DATA_HOME")) {
                Some(dir) => Some(dir),
                None => home.as_ref().map(|h| h.join(".local").join("share")),
            };
        }
        let config_dir: Option<PathBuf> = config_root.map(|root| root.join(APP_DIR));
        let data_dir: Option<PathBuf> = data_root.map(|root| root.join(APP_DIR));
        Paths {
            settings_file: config_dir.map(|dir| dir.join("settings.json")),
            saves_dir: data_dir.as_ref().map(|dir| dir.join("saves")),
            replays_dir: data_dir.map(|dir| dir.join("replays")),
        }
    }
}

/// The value if it is set and not empty.
fn non_empty(value: Option<OsString>) -> Option<OsString> {
    match value {
        Some(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// The value as a path if it is set and absolute (the XDG spec says relative
/// values must be ignored).
fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    match non_empty(value) {
        Some(v) => {
            let path = PathBuf::from(v);
            if path.is_absolute() {
                Some(path)
            } else {
                None
            }
        }
        None => None,
    }
}
