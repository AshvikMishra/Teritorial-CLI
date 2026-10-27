//! The UI's public configuration types: [`Settings`], [`UiOptions`],
//! [`StartMode`] and the [`Persistence`] trait the cli implements.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use territorial_core::{Balance, Game, MatchConfig, Replay, WorldMap};
use territorial_render::{Capabilities, RendererChoice, ThemeKind};

use crate::command::history::DEFAULT_HISTORY_SIZE;

/// User settings, persisted by the cli as JSON. Every field has a default, so
/// old or partial settings files load fine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Color theme.
    pub theme: ThemeKind,
    /// Renderer choice, applied on top of the detected capabilities.
    pub renderer: RendererChoice,
    /// Attack percent used for neutral targets when no amount is typed (1..=100).
    pub default_attack_pct_neutral: u8,
    /// Attack percent used for enemy-owned targets when no amount is typed (1..=100).
    pub default_attack_pct_enemy: u8,
    /// User command aliases, lowercase name -> body.
    pub aliases: BTreeMap<String, String>,
    /// Number of command lines kept in the Up/Down history.
    pub history_size: usize,
    /// Show the usage line for the verb being typed in the hint strip.
    pub show_hints: bool,
    /// The configuration of the last match started from the setup screen.
    /// A random seed is stored as `territorial_core::config::DEFAULT_SEED`.
    pub last_config: Option<MatchConfig>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            theme: ThemeKind::Dark,
            renderer: RendererChoice::Auto,
            default_attack_pct_neutral: 25,
            default_attack_pct_enemy: 40,
            aliases: BTreeMap::new(),
            history_size: DEFAULT_HISTORY_SIZE,
            show_hints: true,
            last_config: None,
        }
    }
}

/// How the UI starts after the splash screen.
#[derive(Clone, Debug)]
pub enum StartMode {
    /// Show the setup screen, prefilled with this configuration.
    Setup(MatchConfig),
    /// Start this match right away (countdown, then play).
    Immediate(MatchConfig),
    /// Continue a saved match.
    Resume(Game),
    /// Play a replay back (read-only), then show the game-over screen.
    Replay(Replay),
}

/// Everything [`crate::run`] needs.
#[derive(Clone, Debug)]
pub struct UiOptions {
    /// User settings (theme, renderer, aliases, ...).
    pub settings: Settings,
    /// What to show first.
    pub start: StartMode,
    /// Detected terminal capabilities; `settings.renderer` is applied on top.
    pub caps: Capabilities,
    /// The world map.
    pub map: Arc<WorldMap>,
    /// Balance for new matches.
    pub balance: Balance,
    /// Skip the splash animation.
    pub skip_splash: bool,
}

/// File I/O the UI needs, implemented by the cli. Errors are short human
/// readable messages shown in the UI.
pub trait Persistence {
    /// Saves a match under `name`. Returns the path written.
    fn save_game(&mut self, name: &str, game: &Game) -> Result<String, String>;
    /// Names of the saved matches, newest first (the UI loads the first one
    /// for "load latest").
    fn list_saves(&self) -> Vec<String>;
    /// Loads a saved match (the UI re-attaches the map either way).
    fn load_game(&mut self, name: &str) -> Result<Game, String>;
    /// Saves a replay. Returns the path written.
    fn save_replay(&mut self, replay: &Replay) -> Result<String, String>;
    /// Persists the settings.
    fn save_settings(&mut self, s: &Settings) -> Result<(), String>;
}
