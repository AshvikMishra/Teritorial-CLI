//! Command-line flags (clap derive).

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use territorial_core::{Difficulty, Preset};
use territorial_render::{RendererChoice, ThemeKind};

const AFTER_HELP: &str = "\
Start modes:
  (no flags)        splash, then the setup screen prefilled from your last match
  --quick           skip the splash and setup and start right away
  --start <region>  start right away in that region (implies an immediate start)
  --bot-only-watch  start right away and watch a match played only by bots
  --load <save>     resume a saved match (a save name or a path to a .json file)
  --replay <file>   watch a recorded match (a path, or a name in the replays folder)
  --simulate        run a bot-only match headless (no TUI) and print the result

Match flags override the last match settings remembered from the setup screen,
except with --simulate, which starts from the standard preset so that a run is
reproducible from its command line alone.

Exit codes: 0 ok, 1 terminal or I/O error, 2 invalid flags, settings or files,
3 --simulate reached --max-ticks without a result.";

// The about text comes from the package description in Cargo.toml, so this
// struct deliberately has no doc comment (it would replace that text).
#[derive(Parser, Debug)]
#[command(name = "territorial", version, about, long_about = None, after_help = AFTER_HELP)]
pub struct Cli {
    /// Match preset: blitz (8 min), standard (25 min), epic (75 min) or custom.
    #[arg(long, value_enum)]
    pub preset: Option<PresetArg>,

    /// Custom match length in minutes (0 = no timer); switches the preset to custom.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(0..=600))]
    pub minutes: Option<u32>,

    /// Number of bot opponents (3 to 40).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(3..=40))]
    pub bots: Option<u8>,

    /// Bot difficulty.
    #[arg(long, value_enum)]
    pub difficulty: Option<DifficultyArg>,

    /// Win condition.
    #[arg(long, value_enum)]
    pub win: Option<WinArg>,

    /// Share of world value needed for a domination win (10 to 100).
    #[arg(long, value_name = "PCT", value_parser = clap::value_parser!(u8).range(10..=100))]
    pub domination_pct: Option<u8>,

    /// Fixed RNG seed (same seed and commands = same match). Random when omitted.
    #[arg(long, value_name = "N")]
    pub seed: Option<u64>,

    /// Start region: a code (fr, us-w) or a name (France). Starts the match right away.
    #[arg(long, value_name = "REGION")]
    pub start: Option<String>,

    /// Disable alliances for this match.
    #[arg(long)]
    pub no_alliances: bool,

    /// Color theme (overrides the saved setting).
    #[arg(long, value_enum)]
    pub theme: Option<ThemeArg>,

    /// Map renderer (overrides the saved setting).
    #[arg(long, value_enum)]
    pub renderer: Option<RendererArg>,

    /// Balance TOML file; it only needs the values it changes.
    #[arg(long, value_name = "FILE")]
    pub balance: Option<PathBuf>,

    /// Resume a saved match: a save name (see the saves folder) or a path.
    #[arg(long, value_name = "SAVE", conflicts_with_all = ["replay", "simulate", "bot_only_watch"])]
    pub load: Option<String>,

    /// Play back a replay file (read-only).
    #[arg(long, value_name = "FILE", conflicts_with_all = ["simulate", "bot_only_watch"])]
    pub replay: Option<String>,

    /// Skip the splash and setup screens and start right away.
    #[arg(long)]
    pub quick: bool,

    /// Run a headless bot-only match and print the result (no TUI).
    #[arg(long)]
    pub simulate: bool,

    /// Give up a --simulate run after this many ticks (10 ticks = 1 second).
    #[arg(long, value_name = "N", requires = "simulate")]
    pub max_ticks: Option<u64>,

    /// Watch a bot-only match in the TUI (player 0 is a bot too).
    #[arg(long, conflicts_with = "simulate")]
    pub bot_only_watch: bool,
}

impl Cli {
    /// Whether any flag that changes the match configuration was given.
    pub fn has_match_flags(&self) -> bool {
        self.preset.is_some()
            || self.minutes.is_some()
            || self.bots.is_some()
            || self.difficulty.is_some()
            || self.win.is_some()
            || self.domination_pct.is_some()
            || self.seed.is_some()
            || self.start.is_some()
            || self.no_alliances
            || self.bot_only_watch
    }
}

/// `--preset` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum PresetArg {
    /// 8 minutes, double growth, 8 bots.
    Blitz,
    /// 25 minutes, normal growth, 12 bots.
    Standard,
    /// 75 minutes, 60% growth, 20 bots.
    Epic,
    /// Standard values; combine with --minutes.
    Custom,
}

impl PresetArg {
    /// The core preset.
    pub fn to_core(self) -> Preset {
        match self {
            PresetArg::Blitz => Preset::Blitz,
            PresetArg::Standard => Preset::Standard,
            PresetArg::Epic => Preset::Epic,
            PresetArg::Custom => Preset::Custom,
        }
    }
}

/// `--difficulty` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum DifficultyArg {
    /// Slow reactions, simple expansion.
    Easy,
    /// Checks the odds before attacking.
    Normal,
    /// Fast reactions, coordinated attacks.
    Hard,
    /// A mix of all three.
    Mixed,
}

impl DifficultyArg {
    /// The core difficulty.
    pub fn to_core(self) -> Difficulty {
        match self {
            DifficultyArg::Easy => Difficulty::Easy,
            DifficultyArg::Normal => Difficulty::Normal,
            DifficultyArg::Hard => Difficulty::Hard,
            DifficultyArg::Mixed => Difficulty::Mixed,
        }
    }
}

/// `--win` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum WinArg {
    /// Control --domination-pct percent of the world's value (default 60).
    Domination,
    /// Last player or alliance standing.
    Elimination,
    /// Highest score when the timer runs out.
    Timed,
}

/// `--theme` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ThemeArg {
    /// Dark theme (default).
    Dark,
    /// High-contrast theme.
    #[value(name = "high-contrast", alias = "hc")]
    HighContrast,
    /// Colorblind-safe theme.
    #[value(name = "colorblind", alias = "cb")]
    Colorblind,
}

impl ThemeArg {
    /// The render theme kind.
    pub fn to_kind(self) -> ThemeKind {
        match self {
            ThemeArg::Dark => ThemeKind::Dark,
            ThemeArg::HighContrast => ThemeKind::HighContrast,
            ThemeArg::Colorblind => ThemeKind::ColorblindSafe,
        }
    }
}

/// `--renderer` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum RendererArg {
    /// Detect from the terminal (default).
    Auto,
    /// 24-bit color half-blocks.
    #[value(name = "truecolor")]
    TrueColor,
    /// 256-color half-blocks.
    #[value(name = "256", alias = "ansi256")]
    Ansi256,
    /// ASCII characters with 16 colors.
    Ascii,
}

impl RendererArg {
    /// The render crate's renderer choice.
    pub fn to_choice(self) -> RendererChoice {
        match self {
            RendererArg::Auto => RendererChoice::Auto,
            RendererArg::TrueColor => RendererChoice::TrueColor,
            RendererArg::Ansi256 => RendererChoice::Ansi256,
            RendererArg::Ascii => RendererChoice::Ascii,
        }
    }
}
