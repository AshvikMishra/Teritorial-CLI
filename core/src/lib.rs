//! # territorial_core
//!
//! The deterministic, I/O-free simulation core of Territorial CLI.
//!
//! This crate owns everything that decides the outcome of a match:
//!
//! * [`map`]: the world graph (regions, land and sea edges) and the region raster,
//!   loaded from the mapgen assets.
//! * [`balance`]: every tunable gameplay number, as one flat integer struct.
//! * [`config`]: match configuration (preset, timer, bots, win condition, seed).
//! * [`command`]: the single command API shared by the human player and the bots.
//! * [`state`]: the [`Game`] state, players, regions, marches and events.
//! * [`sim`]: the fixed-tick simulation (`Game::new`, `Game::step`, validation).
//! * [`bots`]: utility-scoring bot opponents with personalities and difficulties.
//! * [`targets`]: smart targets such as `@weakest` and `@richest`.
//! * [`save`]: save games and replays.
//! * [`rng`]: the in-crate seeded random number generator.
//!
//! Determinism rules: the simulation uses integer math only, never iterates hash
//! containers, never reads the clock and draws all randomness from the seeded
//! [`Rng`]. Identical configuration, balance, seed and command log therefore
//! replay identically.

pub mod balance;
pub mod bots;
pub mod command;
pub mod config;
pub mod map;
pub mod rng;
pub mod save;
pub mod sim;
pub mod state;
pub mod targets;

/// Index of a region in [`WorldMap::regions`].
pub type RegionId = u16;
/// Player identifier: `0` is the human, `1..=40` are bots.
pub type PlayerId = u8;
/// Simulation tick counter. The simulation runs at [`TICKS_PER_SECOND`].
pub type Tick = u64;
/// Number of simulation ticks per in-game second.
pub const TICKS_PER_SECOND: u64 = 10;
/// The human player's id.
pub const HUMAN: PlayerId = 0;
/// The maximum number of bots in a match.
pub const MAX_BOTS: u8 = 40;

pub use balance::Balance;
pub use bots::{BotBrain, Personality};
pub use command::{Amount, Command, CommandError, CommandPreview, IssuedCommand};
pub use config::{Difficulty, MatchConfig, Preset, StartChoice, WinCondition};
pub use map::{Edge, EdgeKind, MapError, Raster, RegionInfo, WorldMap};
pub use rng::Rng;
pub use save::{Replay, ReplayPlayer, SaveGame, REPLAY_FORMAT_VERSION, SAVE_FORMAT_VERSION};
pub use state::{
    EventKind, Game, GameEvent, March, Outcome, PendingRecall, Player, PlayerKind, PlayerStats,
    Proposal, RegionState, Truce, WinReason,
};
pub use targets::{resolve as resolve_target, SmartTarget};
