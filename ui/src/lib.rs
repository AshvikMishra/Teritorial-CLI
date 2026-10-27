//! # territorial_ui
//!
//! The ratatui application for Territorial CLI: splash, setup, start-region
//! picker, countdown, the playing screen (status bar, world map, leaderboard,
//! event log, hint strip and the always-on command prompt) and the game-over
//! screen, plus the [`command`] system that turns typed lines into actions.
//!
//! The cli crate calls [`run`] with [`UiOptions`] and a [`Persistence`]
//! implementation that does the file I/O (saves, replays, settings).
//!
//! Keys while playing: type commands at the prompt (Enter submits, Esc clears,
//! Up/Down history, Tab accepts the ghost completion or, with an empty prompt,
//! cycles the camera through your regions). Shift/Ctrl+arrows pan the map
//! (plain Left/Right pan too while the prompt is empty), `+`/`-` zoom while the
//! prompt is empty, PageUp/PageDown scroll the event log (or the help overlay),
//! Ctrl+U clears the line, Ctrl+W deletes a word, and Ctrl+C quits at once.
//! `quit` (or `q`) ends the match without saving a game (the replay is still
//! auto-saved) and shows the game-over screen; `q` there exits.

pub mod command;

mod app;
mod log;
mod prompt;
mod settings;
mod setup;
mod term;
mod view;

pub use settings::{Persistence, Settings, StartMode, UiOptions};

/// Runs the interactive UI until the player quits.
///
/// Enters raw mode and the alternate screen, and always restores the terminal
/// on return, on error and on panic.
pub fn run(opts: UiOptions, persistence: Box<dyn Persistence>) -> std::io::Result<()> {
    app::run_app(opts, persistence)
}
