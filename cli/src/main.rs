//! `territorial`: the Territorial CLI binary.
//!
//! Parses the flags, loads the balance and settings, then either runs a
//! headless `--simulate` match or hands over to the ratatui UI with a
//! file-backed [`persist::FilePersistence`].

mod args;
mod balance;
mod match_config;
mod paths;
mod persist;
mod simulate;
mod text;

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use territorial_core::config::DEFAULT_SEED;
use territorial_core::{Balance, MatchConfig, WorldMap};
use territorial_render::Capabilities;
use territorial_ui::{StartMode, UiOptions};

use crate::args::Cli;
use crate::paths::Paths;
use crate::persist::FilePersistence;

/// Exit code for terminal and I/O errors.
pub const EXIT_RUNTIME: u8 = 1;
/// Exit code for invalid flags, settings, balance, save or replay files.
pub const EXIT_USAGE: u8 = 2;
/// Exit code when `--simulate` hits `--max-ticks` without a result.
pub const EXIT_STALL: u8 = 3;

/// An error that ends the program with a message and an exit code.
#[derive(Debug)]
pub struct Failure {
    /// Process exit code.
    pub code: u8,
    /// Message printed to stderr.
    pub message: String,
}

impl Failure {
    /// A user error (bad flag value, bad file): exit code 2.
    pub fn usage(message: String) -> Failure {
        Failure {
            code: EXIT_USAGE,
            message,
        }
    }

    /// A runtime error (terminal or I/O): exit code 1.
    pub fn runtime(message: String) -> Failure {
        Failure {
            code: EXIT_RUNTIME,
            message,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(failure) => {
            eprintln!("territorial: error: {}", failure.message);
            ExitCode::from(failure.code)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, Failure> {
    let balance = balance::load(cli.balance.as_deref()).map_err(Failure::usage)?;
    let map = WorldMap::builtin();
    if cli.simulate {
        return simulate::run(&cli, map, balance);
    }
    run_interactive(cli, map, balance)
}

fn run_interactive(cli: Cli, map: Arc<WorldMap>, balance: Balance) -> Result<ExitCode, Failure> {
    let mut persistence = FilePersistence::new(Paths::detect());
    let mut warnings: Vec<String> = Vec::new();

    let (mut settings, settings_warning) = persistence.load_settings();
    if let Some(w) = settings_warning {
        warnings.push(w);
    }
    // `--theme` / `--renderer` apply to this run only: the persistence keeps
    // writing the saved values unless the player picks another one in the UI.
    if let Some(theme) = cli.theme {
        let kind = theme.to_kind();
        persistence.override_theme(kind, settings.theme);
        settings.theme = kind;
    }
    if let Some(renderer) = cli.renderer {
        let choice = renderer.to_choice();
        persistence.override_renderer(choice, settings.renderer);
        settings.renderer = choice;
    }

    let match_flags = cli.has_match_flags();
    let start: StartMode = if let Some(arg) = &cli.load {
        if match_flags {
            warnings.push("match flags are ignored with --load".to_string());
        }
        let path = persistence.resolve_save(arg).map_err(Failure::usage)?;
        // read_save checks the save against the map and validates its balance.
        let game = persist::read_save(&path).map_err(Failure::usage)?;
        StartMode::Resume(Box::new(game))
    } else if let Some(arg) = &cli.replay {
        if match_flags {
            warnings.push("match flags are ignored with --replay".to_string());
        }
        if cli.balance.is_some() {
            warnings.push("--balance is ignored with --replay (the replay carries its own)".to_string());
        }
        let path = persistence.resolve_replay(arg).map_err(Failure::usage)?;
        let replay = persist::read_replay(&path).map_err(Failure::usage)?;
        replay.config.validate(&map).map_err(|e| {
            Failure::usage(format!("replay {} has an invalid config: {}", path.display(), e))
        })?;
        replay.balance.validate().map_err(|e| {
            Failure::usage(format!(
                "replay {} has invalid balance values: {}",
                path.display(),
                e
            ))
        })?;
        StartMode::Replay(Box::new(replay))
    } else {
        let base = match_config::interactive_base(settings.last_config.as_ref());
        let mut cfg = match_config::apply_flags(base, &cli, &map)?;
        if let Err(e) = cfg.validate(&map) {
            if match_flags {
                return Err(Failure::usage(format!("invalid match settings: {}", e)));
            }
            warnings.push(format!(
                "the remembered match settings are invalid ({}); using the standard preset",
                e
            ));
            cfg = MatchConfig::default();
        }
        let immediate = cli.quick || cli.start.is_some() || cli.bot_only_watch;
        if immediate {
            if cfg.seed == DEFAULT_SEED && cli.seed.is_none() {
                cfg.seed = match_config::time_seed();
            }
            StartMode::Immediate(cfg)
        } else {
            StartMode::Setup(cfg)
        }
    };

    if !warnings.is_empty() {
        for w in &warnings {
            eprintln!("territorial: warning: {}", w);
        }
        // Give the reader a moment before the UI takes over the screen.
        if std::io::stderr().is_terminal() {
            std::thread::sleep(Duration::from_millis(1500));
        }
    }

    let opts = UiOptions {
        settings,
        start,
        caps: Capabilities::detect(),
        map,
        balance,
        skip_splash: cli.quick,
    };
    match territorial_ui::run(opts, Box::new(persistence)) {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(e) => Err(Failure::runtime(format!("terminal error: {}", e))),
    }
}
