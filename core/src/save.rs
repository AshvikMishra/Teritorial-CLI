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
use crate::command::{Command, IssuedCommand};
use crate::config::MatchConfig;
use crate::map::WorldMap;
use crate::state::{Game, GameEvent, Outcome};
use crate::{PlayerId, RegionId, Tick};

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

    /// Unwraps the game and attaches the map. Does not check that the game fits
    /// the map; prefer [`SaveGame::into_checked_game`] for files from disk.
    pub fn into_game(self, map: Arc<WorldMap>) -> Game {
        let mut game = self.game;
        game.attach_map(map);
        game
    }

    /// Like [`SaveGame::into_game`], but first checks the saved balance
    /// ([`Balance::validate`]) and that the game fits `map`
    /// ([`Game::check_map`]). Returns a human readable reason on failure instead
    /// of a game that would panic later.
    pub fn into_checked_game(self, map: Arc<WorldMap>) -> Result<Game, String> {
        self.game
            .balance
            .validate()
            .map_err(|e| format!("the save has invalid balance values: {}", e))?;
        self.game.check_map(&map)?;
        Ok(self.into_game(map))
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

fn check_player(p: PlayerId, players: usize, what: &str) -> Result<(), String> {
    if (p as usize) < players {
        Ok(())
    } else {
        Err(format!("the save is corrupt: {} names unknown player {}", what, p))
    }
}

fn check_region(r: RegionId, regions: usize, what: &str) -> Result<(), String> {
    if (r as usize) < regions {
        Ok(())
    } else {
        Err(format!(
            "the save does not match this world map: {} names region {}, but the map has {} regions",
            what, r, regions
        ))
    }
}

fn check_command(
    player: PlayerId,
    command: &Command,
    players: usize,
    regions: usize,
    what: &str,
) -> Result<(), String> {
    check_player(player, players, what)?;
    if let Some(r) = command.region() {
        check_region(r, regions, what)?;
    }
    if let Some(o) = command.other_player() {
        check_player(o, players, what)?;
    }
    Ok(())
}

impl Game {
    /// Checks that a deserialized game fits `map` before it is attached: the
    /// region table has exactly one entry per map region, there is at least one
    /// player (as many as the config implies, with ids matching their index),
    /// and every region and player id stored anywhere in the state is in range.
    /// Returns a human readable reason on failure.
    pub fn check_map(&self, map: &WorldMap) -> Result<(), String> {
        let nr = map.len();
        let np = self.players.len();
        if map.is_empty() {
            return Err("the world map has no regions".to_string());
        }
        if self.regions.len() != nr {
            return Err(format!(
                "the save does not match this world map: it has {} regions, the map has {}",
                self.regions.len(),
                nr
            ));
        }
        if np == 0 {
            return Err("the save is corrupt: it has no players".to_string());
        }
        let expected = self.config.bots.min(crate::MAX_BOTS) as usize + 1;
        if np != expected {
            return Err(format!(
                "the save is corrupt: it has {} players, its config implies {}",
                np, expected
            ));
        }
        for (i, pl) in self.players.iter().enumerate() {
            if pl.id as usize != i {
                return Err(format!("the save is corrupt: player {} has id {}", i, pl.id));
            }
            if let Some(c) = pl.capital {
                check_region(c, nr, "a player's capital")?;
            }
            if let Some(o) = pl.last_lost_to {
                check_player(o, np, "a player's last attacker")?;
            }
            if let Some(o) = pl.stats.eliminated_by {
                check_player(o, np, "a player's eliminator")?;
            }
        }
        for st in &self.regions {
            if let Some(o) = st.owner {
                check_player(o, np, "a region owner")?;
            }
        }
        for m in &self.marches {
            check_player(m.owner, np, "a march")?;
            check_region(m.from, nr, "a march")?;
            check_region(m.to, nr, "a march")?;
        }
        for r in &self.recalls {
            check_player(r.player, np, "a recall")?;
        }
        for &(a, b) in &self.alliances {
            check_player(a, np, "an alliance")?;
            check_player(b, np, "an alliance")?;
        }
        for pr in &self.proposals {
            check_player(pr.from, np, "an alliance proposal")?;
            check_player(pr.to, np, "an alliance proposal")?;
        }
        for t in &self.truces {
            check_player(t.a, np, "a truce")?;
            check_player(t.b, np, "a truce")?;
        }
        for q in &self.pending_commands {
            check_command(q.player, &q.command, np, nr, "a pending command")?;
        }
        for c in &self.command_log {
            check_command(c.player, &c.command, np, nr, "a logged command")?;
        }
        match &self.outcome {
            Some(Outcome::Winner { player, .. }) => {
                check_player(*player, np, "the match outcome")?;
            }
            Some(Outcome::AlliedVictory { players, top }) => {
                for &p in players {
                    check_player(p, np, "the match outcome")?;
                }
                check_player(*top, np, "the match outcome")?;
            }
            None => {}
        }
        for br in &self.brains {
            check_player(br.player, np, "a bot brain")?;
            if let Some(f) = br.focus {
                check_player(f, np, "a bot brain's focus")?;
            }
            for &(from, _) in &br.considered {
                check_player(from, np, "a bot brain's judged proposal")?;
            }
        }
        Ok(())
    }

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
    /// Like [`ReplayPlayer::new`], but first checks the replay's config against
    /// `map` ([`MatchConfig::validate`]) and its balance ([`Balance::validate`]).
    /// Returns a human readable reason on failure.
    pub fn try_new(map: Arc<WorldMap>, replay: Replay) -> Result<ReplayPlayer, String> {
        replay
            .config
            .validate(&map)
            .map_err(|e| format!("the replay has an invalid config: {}", e))?;
        replay
            .balance
            .validate()
            .map_err(|e| format!("the replay has invalid balance values: {}", e))?;
        Ok(ReplayPlayer::new(map, replay))
    }

    /// Starts a replay from tick 0. Does not validate the replay; prefer
    /// [`ReplayPlayer::try_new`] for files from disk.
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
