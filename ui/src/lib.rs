//! # territorial_ui
//!
//! The ratatui application for Territorial CLI: splash, setup, start-region
//! picker, countdown, the playing screen (status bar, world map with a center
//! crosshair, leaderboard, event log, hint strip and the hotkey strip or
//! command prompt) and the game-over screen, plus the [`command`] system that
//! turns typed lines into actions.
//!
//! The cli crate calls [`run`] with [`UiOptions`] and a [`Persistence`]
//! implementation that does the file I/O (saves, replays, settings).
//!
//! Keys while playing are hotkeys by default and act at once on the region
//! under the crosshair: arrows pan (Shift/Ctrl 4x), `+`/`-` zoom, Tab and
//! Shift+Tab jump to attackable regions, `o`/`O` to your own, `c` to your
//! capital; `a`/`f`/`r` attack, fortify or recall it with the amount set by
//! `1`..`9`/`0` (10%..100%), `w`/`e` attack @weakest/@richest, `l`/`u`/`y`
//! handle alliances, `i`/`s` show info and status, `p` or Space pauses, `[`/`]`
//! change speed, `t` the theme, `?`/`h` help, `.` repeats, PageUp/PageDown
//! scroll the event log (or the help overlay), Esc closes panels. `:` or Enter
//! opens the typed command line (Enter submits and returns to hotkeys, Esc
//! clears the line and then returns, Up/Down history, Tab accepts the ghost
//! completion). Ctrl+C quits at once. `:quit` ends the match without saving a
//! game (the replay is still auto-saved) and shows the game-over screen; `q`
//! there exits.

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
