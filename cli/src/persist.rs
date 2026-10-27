//! File I/O for the UI: saves, replays and settings, as JSON.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use territorial_core::{
    Game, Replay, SaveGame, WorldMap, REPLAY_FORMAT_VERSION, SAVE_FORMAT_VERSION,
};
use territorial_render::{RendererChoice, ThemeKind};
use territorial_ui::{Persistence, Settings};

use crate::paths::Paths;

const NO_DATA_DIR: &str =
    "cannot find a data directory (set HOME, XDG_DATA_HOME or APPDATA)";
const NO_CONFIG_DIR: &str =
    "cannot find a config directory (set HOME, XDG_CONFIG_HOME or APPDATA)";

/// Longest save name kept after sanitizing.
const MAX_NAME_LEN: usize = 64;

/// Borrowing twin of [`SaveGame`]; serializes identically without cloning the game.
#[derive(Serialize)]
struct SaveGameRef<'a> {
    format_version: u32,
    game: &'a Game,
}

/// Saves, replays and settings stored under [`Paths`].
pub struct FilePersistence {
    paths: Paths,
    /// `--theme` override for this run and the theme saved on disk.
    theme_override: Option<(ThemeKind, ThemeKind)>,
    /// `--renderer` override for this run and the renderer saved on disk.
    renderer_override: Option<(RendererChoice, RendererChoice)>,
}

impl FilePersistence {
    /// Persistence rooted at the given paths.
    pub fn new(paths: Paths) -> FilePersistence {
        FilePersistence {
            paths,
            theme_override: None,
            renderer_override: None,
        }
    }

    /// Marks `flag` as a one-run `--theme` override of the saved `saved` theme:
    /// settings are written with `saved` until the player picks another theme.
    pub fn override_theme(&mut self, flag: ThemeKind, saved: ThemeKind) {
        self.theme_override = Some((flag, saved));
    }

    /// Marks `flag` as a one-run `--renderer` override of the saved `saved`
    /// renderer: settings are written with `saved` until the player picks
    /// another renderer.
    pub fn override_renderer(&mut self, flag: RendererChoice, saved: RendererChoice) {
        self.renderer_override = Some((flag, saved));
    }

    /// Loads the settings. A missing file gives the defaults silently; an
    /// unreadable or corrupt file gives the defaults plus a warning (the corrupt
    /// file is copied to `settings.json.bak` first).
    pub fn load_settings(&self) -> (Settings, Option<String>) {
        let path = match &self.paths.settings_file {
            Some(p) => p,
            None => {
                return (
                    Settings::default(),
                    Some(format!("{}; settings will not be saved", NO_CONFIG_DIR)),
                )
            }
        };
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    return (Settings::default(), None);
                }
                return (
                    Settings::default(),
                    Some(format!(
                        "cannot read settings {}: {}; using defaults",
                        path.display(),
                        e
                    )),
                );
            }
        };
        match serde_json::from_str::<Settings>(&text) {
            Ok(settings) => (sanitize_settings(settings), None),
            Err(e) => {
                let backup = path.with_extension("json.bak");
                let note = match fs::copy(path, &backup) {
                    Ok(_) => format!(" (the old file was copied to {})", backup.display()),
                    Err(_) => String::new(),
                };
                (
                    Settings::default(),
                    Some(format!(
                        "settings file {} is invalid ({}); using defaults{}",
                        path.display(),
                        e,
                        note
                    )),
                )
            }
        }
    }

    /// Finds a save from a `--load` argument: a save name in the saves folder
    /// (with or without `.json`), or else an existing file path.
    pub fn resolve_save(&self, arg: &str) -> Result<PathBuf, String> {
        let direct = PathBuf::from(arg);
        if looks_like_path(arg) {
            if direct.is_file() {
                return Ok(direct);
            }
            return Err(format!("save file '{}' not found", arg));
        }
        if let Some(dir) = &self.paths.saves_dir {
            // The exact name first (files copied in by hand keep their names),
            // then the sanitized name the game itself saves under.
            let exact = dir.join(format!("{}.json", strip_json(arg)));
            if exact.is_file() {
                return Ok(exact);
            }
            let name = sanitize_name(strip_json(arg));
            let candidate = dir.join(format!("{}.json", name));
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        if direct.is_file() {
            return Ok(direct);
        }
        let dir = match &self.paths.saves_dir {
            Some(d) => d,
            None => return Err(NO_DATA_DIR.to_string()),
        };
        let saves = self.list_saves();
        if saves.is_empty() {
            Err(format!(
                "no save named '{}' (there are no saves in {})",
                arg,
                dir.display()
            ))
        } else {
            let shown: Vec<String> = saves.into_iter().take(10).collect();
            Err(format!(
                "no save named '{}' in {}. Saves, newest first: {}",
                arg,
                dir.display(),
                shown.join(", ")
            ))
        }
    }

    /// Finds a replay from a `--replay` argument: an existing file path, or a
    /// replay name in the replays folder (with or without `.json`).
    pub fn resolve_replay(&self, arg: &str) -> Result<PathBuf, String> {
        let direct = PathBuf::from(arg);
        if direct.is_file() {
            return Ok(direct);
        }
        if !looks_like_path(arg) {
            if let Some(dir) = &self.paths.replays_dir {
                let candidate = dir.join(format!("{}.json", strip_json(arg)));
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
        Err(format!("replay file '{}' not found", arg))
    }

    fn saves_dir(&self) -> Result<&Path, String> {
        match &self.paths.saves_dir {
            Some(d) => Ok(d.as_path()),
            None => Err(NO_DATA_DIR.to_string()),
        }
    }

    fn replays_dir(&self) -> Result<&Path, String> {
        match &self.paths.replays_dir {
            Some(d) => Ok(d.as_path()),
            None => Err(NO_DATA_DIR.to_string()),
        }
    }
}

impl Persistence for FilePersistence {
    fn save_game(&mut self, name: &str, game: &Game) -> Result<String, String> {
        let dir = self.saves_dir()?;
        let clean = sanitize_name(strip_json(name));
        let path = dir.join(format!("{}.json", clean));
        let wrapper = SaveGameRef {
            format_version: SAVE_FORMAT_VERSION,
            game,
        };
        let json = serde_json::to_string(&wrapper)
            .map_err(|e| format!("cannot encode the save: {}", e))?;
        write_file(&path, &json)?;
        Ok(path.display().to_string())
    }

    fn list_saves(&self) -> Vec<String> {
        let dir = match &self.paths.saves_dir {
            Some(d) => d,
            None => return Vec::new(),
        };
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        let mut rows: Vec<(SystemTime, String)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            rows.push((modified, stem));
        }
        rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        rows.into_iter().map(|row| row.1).collect()
    }

    fn load_game(&mut self, name: &str) -> Result<Game, String> {
        let path = self.resolve_save(name)?;
        read_save(&path)
    }

    fn save_replay(&mut self, replay: &Replay) -> Result<String, String> {
        let dir = self.replays_dir()?;
        let stamp = utc_stamp(unix_secs());
        let mut path = dir.join(format!("replay-{}.json", stamp));
        let mut n: u32 = 2;
        while path.exists() && n < 1000 {
            path = dir.join(format!("replay-{}-{}.json", stamp, n));
            n += 1;
        }
        let json = serde_json::to_string_pretty(replay)
            .map_err(|e| format!("cannot encode the replay: {}", e))?;
        write_file(&path, &json)?;
        Ok(path.display().to_string())
    }

    fn save_settings(&mut self, s: &Settings) -> Result<(), String> {
        let path = match &self.paths.settings_file {
            Some(p) => p.clone(),
            None => return Err(NO_CONFIG_DIR.to_string()),
        };
        // Keep one-run --theme/--renderer overrides out of the file. Once the
        // player picks a different value in the UI, that value is saved.
        let mut out = s.clone();
        if let Some((flag, saved)) = self.theme_override {
            if out.theme == flag {
                out.theme = saved;
            } else {
                self.theme_override = None;
            }
        }
        if let Some((flag, saved)) = self.renderer_override {
            if out.renderer == flag {
                out.renderer = saved;
            } else {
                self.renderer_override = None;
            }
        }
        let json = serde_json::to_string_pretty(&out)
            .map_err(|e| format!("cannot encode the settings: {}", e))?;
        write_file(&path, &json)
    }
}

/// Reads a save file, checks it against the built-in map and its balance
/// values, and re-attaches the map.
pub fn read_save(path: &Path) -> Result<Game, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("cannot read save {}: {}", path.display(), e))?;
    let save: SaveGame = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a valid save file: {}", path.display(), e))?;
    if save.format_version > SAVE_FORMAT_VERSION {
        return Err(format!(
            "{} was written by a newer version (save format {}, this build reads up to {})",
            path.display(),
            save.format_version,
            SAVE_FORMAT_VERSION
        ));
    }
    let map = WorldMap::builtin();
    let bad = |why: String| format!("{} is not a valid save file: {}", path.display(), why);
    save.game.check_map(&map).map_err(bad)?;
    save.game
        .config()
        .validate(&map)
        .map_err(|e| bad(format!("invalid match settings: {}", e)))?;
    save.game
        .balance()
        .validate()
        .map_err(|e| bad(format!("invalid balance values: {}", e)))?;
    Ok(save.into_game(map))
}

/// Reads a replay file.
pub fn read_replay(path: &Path) -> Result<Replay, String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("cannot read replay {}: {}", path.display(), e))?;
    let replay: Replay = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a valid replay file: {}", path.display(), e))?;
    if replay.format_version > REPLAY_FORMAT_VERSION {
        return Err(format!(
            "{} was written by a newer version (replay format {}, this build reads up to {})",
            path.display(),
            replay.format_version,
            REPLAY_FORMAT_VERSION
        ));
    }
    Ok(replay)
}

/// Keeps `[A-Za-z0-9_-]`, turns spaces and dots into `-`, drops everything
/// else, and caps the length. An empty result becomes `save`.
pub fn sanitize_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if out.len() >= MAX_NAME_LEN {
            break;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else if ch == ' ' || ch == '.' {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "save".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Clamps settings values that the UI expects to be in range.
fn sanitize_settings(mut s: Settings) -> Settings {
    let defaults = Settings::default();
    if s.default_attack_pct_neutral == 0 || s.default_attack_pct_neutral > 100 {
        s.default_attack_pct_neutral = defaults.default_attack_pct_neutral;
    }
    if s.default_attack_pct_enemy == 0 || s.default_attack_pct_enemy > 100 {
        s.default_attack_pct_enemy = defaults.default_attack_pct_enemy;
    }
    if s.history_size == 0 || s.history_size > 100_000 {
        s.history_size = defaults.history_size;
    }
    s
}

/// `name` without a trailing `.json` (any case).
fn strip_json(name: &str) -> &str {
    let trimmed = name.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.ends_with(".json") && trimmed.len() >= 5 {
        &trimmed[..trimmed.len() - 5]
    } else {
        trimmed
    }
}

/// Whether the argument names a path rather than a bare save name.
fn looks_like_path(arg: &str) -> bool {
    arg.contains('/') || arg.contains('\\')
}

/// Writes `contents` to `path` through a temporary file and a rename, creating
/// the parent directory if needed.
fn write_file(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
    }
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    fs::write(&tmp, contents).map_err(|e| format!("cannot write {}: {}", tmp.display(), e))?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(format!("cannot write {}: {}", path.display(), e))
        }
    }
}

/// Seconds since the Unix epoch (0 if the clock is before it).
fn unix_secs() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs(),
        Err(_) => 0,
    }
}

/// `YYYYMMDD-HHMMSS` in UTC.
fn utc_stamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        year,
        month,
        day,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097; // 0..=146_096
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // 0..=399
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..=365
    let mp = (5 * doy + 2) / 153; // 0..=11
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let carry: i64 = if month <= 2 { 1 } else { 0 };
    let year = yoe + era * 400 + carry;
    (year, month, day)
}
