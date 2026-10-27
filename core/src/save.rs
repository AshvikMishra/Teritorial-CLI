//! Save games and replays.
//!
//! A [`SaveGame`] wraps the whole [`Game`] (everything except the map, which is
//! re-attached with [`Game::attach_map`] after loading). A [`Replay`] holds only
//! the config, balance and the externally issued command log: because the
//! simulation is deterministic, bots regenerate their own commands when the
//! replay is played back with [`ReplayPlayer`]. A resumed save carries its whole
//! command log, so a replay exported later still starts from tick 0.
//!
//! Core does no I/O; the cli serializes these types with `serde_json`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::balance::Balance;
use crate::command::IssuedCommand;
use crate::config::MatchConfig;
use crate::map::WorldMap;
use crate::state::{Game, GameEvent};
use crate::Tick;

/// Current save file format version.
pub const SAVE_FORMAT_VERSION: u32 = 1;
/// Current replay file format version.
pub const REPLAY_FORMAT_VERSION: u32 = 1;

/// A saved match.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SaveGame {
    /// File format version ([`SAVE_FORMAT_VERSION`]).
    pub format_version: u32,
    /// The match (map not included; call [`Game::attach_map`] after loading).
    pub game: Game,
}

impl SaveGame {
    /// Wraps a game with the current format version.
    pub fn new(game: Game) -> SaveGame {
        SaveGame {
            format_version: SAVE_FORMAT_VERSION,
            game,
        }
    }

    /// Unwraps the game and attaches the map.
    pub fn into_game(self, map: Arc<WorldMap>) -> Game {
        let mut game = self.game;
        game.attach_map(map);
        game
    }
}

/// A replay: everything needed to re-run a match from tick 0.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replay {
    /// File format version ([`REPLAY_FORMAT_VERSION`]).
    pub format_version: u32,
    /// Match configuration (including the seed).
    pub config: MatchConfig,
    /// Balance in use.
    pub balance: Balance,
    /// Externally issued commands, in application order, stamped with their tick.
    pub commands: Vec<IssuedCommand>,
    /// The last tick of the recorded match.
    pub final_tick: Tick,
}

impl Game {
    /// Attaches the map after deserializing (the map is not saved) and rebuilds
    /// the derived territory caches. The map must be the one the game was
    /// created with.
    pub fn attach_map(&mut self, map: Arc<WorldMap>) {
        self.map = map;
        self.recompute_caches();
    }

    /// Exports the match so far as a replay.
    pub fn to_replay(&self) -> Replay {
        Replay {
            format_version: REPLAY_FORMAT_VERSION,
            config: self.config.clone(),
            balance: self.balance.clone(),
            commands: self.command_log.clone(),
            final_tick: self.tick,
        }
    }
}

/// Plays a replay back tick by tick.
#[derive(Clone, Debug)]
pub struct ReplayPlayer {
    game: Game,
    commands: Vec<IssuedCommand>,
    cursor: usize,
    final_tick: Tick,
}

impl ReplayPlayer {
    /// Starts a replay from tick 0.
    pub fn new(map: Arc<WorldMap>, replay: Replay) -> ReplayPlayer {
        let Replay {
            config,
            balance,
            commands,
            final_tick,
            ..
        } = replay;
        ReplayPlayer {
            game: Game::new(map, balance, config),
            commands,
            cursor: 0,
            final_tick,
        }
    }

    /// Queues every recorded command stamped with the next tick, then steps the game.
    pub fn step(&mut self) -> Vec<GameEvent> {
        let next = self.game.tick() + 1;
        while self.cursor < self.commands.len() && self.commands[self.cursor].tick <= next {
            let c = self.commands[self.cursor];
            self.game.queue(c.player, c.command);
            self.cursor += 1;
        }
        self.game.step()
    }

    /// The game being replayed.
    pub fn game(&self) -> &Game {
        &self.game
    }

    /// The last tick of the recorded match.
    pub fn final_tick(&self) -> Tick {
        self.final_tick
    }

    /// True once the recorded final tick is reached or the match is over.
    pub fn finished(&self) -> bool {
        self.game.tick() >= self.final_tick || self.game.is_over()
    }
}
