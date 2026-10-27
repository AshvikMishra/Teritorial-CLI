//! Game state: players, regions, marches, diplomacy, events, and the [`Game`]
//! struct with its read-only query API.
//!
//! The mutating half of `Game` (construction, command validation and
//! application, and the tick loop) lives in [`crate::sim`]; bot decisions live
//! in [`crate::bots`]; save/replay helpers live in [`crate::save`].

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::balance::Balance;
use crate::bots::{BotBrain, Personality};
use crate::command::{Command, CommandError, IssuedCommand};
use crate::config::{Difficulty, MatchConfig};
use crate::map::WorldMap;
use crate::rng::Rng;
use crate::{PlayerId, RegionId, Tick, TICKS_PER_SECOND};

/// Who controls a player.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerKind {
    /// Controlled by typed commands.
    Human,
    /// Controlled by the bot AI (player 0 is a bot too when `config.spectate`).
    Bot {
        /// Play style.
        personality: Personality,
        /// Decision quality and reaction speed (never `Mixed`).
        difficulty: Difficulty,
    },
}

/// Per-player statistics for the leaderboard and game-over screen.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerStats {
    /// Regions captured (including neutrals).
    pub regions_captured: u32,
    /// Regions lost to other players.
    pub regions_lost: u32,
    /// Troops sent in attacks.
    pub troops_sent: u64,
    /// Troops lost (attackers destroyed or spent, garrisons killed).
    pub troops_lost: u64,
    /// Most regions held at once.
    pub peak_regions: u32,
    /// Tick of elimination.
    pub eliminated_at: Option<Tick>,
    /// The player who captured this player's last region.
    pub eliminated_by: Option<PlayerId>,
}

/// One player. The pool and territory caches are private; use the `Game` accessors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    /// Player id (index into `Game::players`).
    pub id: PlayerId,
    /// Display name.
    pub name: String,
    /// Human or bot.
    pub kind: PlayerKind,
    /// False once eliminated.
    pub alive: bool,
    /// Capital region (start region, moved to the best region if lost).
    pub capital: Option<RegionId>,
    /// Statistics.
    pub stats: PlayerStats,
    pub(crate) pool_milli: i64,
    pub(crate) owned_value: u64,
    pub(crate) region_count: u32,
    pub(crate) last_lost_to: Option<PlayerId>,
}

impl Player {
    /// Whether the bot AI controls this player.
    pub fn is_bot(&self) -> bool {
        matches!(self.kind, PlayerKind::Bot { .. })
    }

    /// The pool in whole troops.
    pub fn pool(&self) -> i64 {
        self.pool_milli.max(0) / 1000
    }

    /// Bot personality, if this player is a bot.
    pub fn personality(&self) -> Option<Personality> {
        match self.kind {
            PlayerKind::Bot { personality, .. } => Some(personality),
            PlayerKind::Human => None,
        }
    }
}

/// Dynamic state of one region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionState {
    /// Owner, or `None` for neutral.
    pub owner: Option<PlayerId>,
    /// Defending troops.
    pub garrison: i64,
    /// The fortified bonus applies while `tick < fortified_until`.
    pub fortified_until: Tick,
}

/// A force marching along an edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct March {
    /// Unique march id (ascending in launch order).
    pub id: u32,
    /// Attacking player.
    pub owner: PlayerId,
    /// Departure region (owned by `owner` at launch).
    pub from: RegionId,
    /// Target region.
    pub to: RegionId,
    /// Troops in the march.
    pub troops: i64,
    /// Launch tick.
    pub depart_tick: Tick,
    /// Arrival tick.
    pub arrive_tick: Tick,
}

/// Troops recalled from a garrison, on their way to the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRecall {
    /// Owner.
    pub player: PlayerId,
    /// Troops.
    pub troops: i64,
    /// Tick the troops reach the pool.
    pub arrive_tick: Tick,
}

/// A pending alliance proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// Proposer.
    pub from: PlayerId,
    /// Addressee.
    pub to: PlayerId,
    /// Tick the proposal expires.
    pub expires: Tick,
}

/// A truce after a broken alliance (`a < b`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truce {
    /// Lower player id.
    pub a: PlayerId,
    /// Higher player id.
    pub b: PlayerId,
    /// Tick the truce ends.
    pub until: Tick,
}

/// A command waiting for the next `step()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct QueuedCommand {
    pub(crate) player: PlayerId,
    pub(crate) command: Command,
}

/// Why a player won.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WinReason {
    /// Reached the domination percent.
    Domination,
    /// Last player standing.
    Elimination,
    /// Highest score when the timer ended.
    Timer,
}

/// How the match ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// One player won.
    Winner {
        /// The winner.
        player: PlayerId,
        /// Why.
        reason: WinReason,
    },
    /// Every surviving player is in one mutual alliance.
    AlliedVictory {
        /// The allied survivors, ascending.
        players: Vec<PlayerId>,
        /// The highest-scoring of them.
        top: PlayerId,
    },
}

impl Outcome {
    /// Whether `p` is among the winners.
    pub fn is_winner(&self, p: PlayerId) -> bool {
        match self {
            Outcome::Winner { player, .. } => *player == p,
            Outcome::AlliedVictory { players, .. } => players.contains(&p),
        }
    }
}

/// Something that happened during a tick.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameEvent {
    /// The tick it happened on.
    pub tick: Tick,
    /// What happened.
    pub kind: EventKind,
}

/// Event payloads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    /// A command was applied (`troops` = resolved amount, 0 for diplomacy).
    CommandApplied {
        /// Issuer.
        player: PlayerId,
        /// The command.
        command: Command,
        /// Troops moved.
        troops: i64,
    },
    /// A queued or bot command failed validation.
    CommandRejected {
        /// Issuer.
        player: PlayerId,
        /// The command.
        command: Command,
        /// Why.
        reason: CommandError,
    },
    /// A march left.
    MarchStarted {
        /// March id.
        march_id: u32,
        /// Attacker.
        player: PlayerId,
        /// Departure region.
        from: RegionId,
        /// Target region.
        to: RegionId,
        /// Troops.
        troops: i64,
        /// Ticks until arrival.
        eta_ticks: u64,
    },
    /// A region changed hands.
    Captured {
        /// Region.
        region: RegionId,
        /// New owner.
        by: PlayerId,
        /// Previous owner (`None` = neutral).
        from: Option<PlayerId>,
        /// Surviving attackers, now the garrison.
        survivors: i64,
    },
    /// An attack failed.
    AttackRepelled {
        /// Region.
        region: RegionId,
        /// Attacker.
        attacker: PlayerId,
        /// Defender (`None` = neutral).
        defender: Option<PlayerId>,
        /// Attackers lost (all of them).
        attacker_lost: i64,
        /// Garrison lost.
        defender_lost: i64,
    },
    /// A march arrived at a region its owner already holds and joined the garrison.
    Reinforced {
        /// Region.
        region: RegionId,
        /// Owner.
        player: PlayerId,
        /// Troops added.
        troops: i64,
    },
    /// Troops reached the pool (a recall, or a march that bounced off an ally or truce).
    RecallArrived {
        /// Owner.
        player: PlayerId,
        /// Troops added to the pool.
        troops: i64,
    },
    /// A player lost their last region.
    Eliminated {
        /// The eliminated player.
        player: PlayerId,
        /// Who captured their last region.
        by: Option<PlayerId>,
    },
    /// An alliance was proposed.
    AllianceProposed {
        /// Proposer.
        from: PlayerId,
        /// Addressee.
        to: PlayerId,
    },
    /// An alliance formed (`a < b`).
    AllianceFormed {
        /// Lower id.
        a: PlayerId,
        /// Higher id.
        b: PlayerId,
    },
    /// An alliance was broken.
    AllianceBroken {
        /// The player who broke it.
        by: PlayerId,
        /// The former ally.
        with: PlayerId,
    },
    /// A proposal expired unanswered.
    ProposalExpired {
        /// Proposer.
        from: PlayerId,
        /// Addressee.
        to: PlayerId,
    },
    /// The match ended. Emitted exactly once.
    MatchOver {
        /// The result.
        outcome: Outcome,
    },
}

/// The whole match state. Serializes everything except the map, which is
/// re-attached with [`Game::attach_map`] after loading.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    #[serde(skip, default = "empty_map")]
    pub(crate) map: Arc<WorldMap>,
    pub(crate) balance: Balance,
    pub(crate) config: MatchConfig,
    pub(crate) tick: Tick,
    pub(crate) rng: Rng,
    pub(crate) players: Vec<Player>,
    pub(crate) regions: Vec<RegionState>,
    pub(crate) marches: Vec<March>,
    pub(crate) next_march_id: u32,
    pub(crate) recalls: Vec<PendingRecall>,
    pub(crate) alliances: BTreeSet<(PlayerId, PlayerId)>,
    pub(crate) proposals: Vec<Proposal>,
    pub(crate) truces: Vec<Truce>,
    pub(crate) pending_commands: Vec<QueuedCommand>,
    pub(crate) command_log: Vec<IssuedCommand>,
    pub(crate) outcome: Option<Outcome>,
    pub(crate) brains: Vec<BotBrain>,
    pub(crate) last_capture_tick: Tick,
}

fn empty_map() -> Arc<WorldMap> {
    Arc::new(WorldMap::default())
}

/// Orders a pair of players as `(low, high)`.
pub(crate) fn pair(a: PlayerId, b: PlayerId) -> (PlayerId, PlayerId) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

impl Game {
    // ------------------------------------------------------------ basics --

    /// The world map.
    pub fn map(&self) -> &Arc<WorldMap> {
        &self.map
    }

    /// The balance numbers in use.
    pub fn balance(&self) -> &Balance {
        &self.balance
    }

    /// The match configuration.
    pub fn config(&self) -> &MatchConfig {
        &self.config
    }

    /// The current tick (0 before the first `step()`).
    pub fn tick(&self) -> Tick {
        self.tick
    }

    /// Whole in-game seconds elapsed.
    pub fn elapsed_secs(&self) -> u64 {
        self.tick / TICKS_PER_SECOND
    }

    /// Seconds left on the match timer, if there is one.
    pub fn remaining_secs(&self) -> Option<u64> {
        self.config
            .active_time_limit_secs()
            .map(|t| (t as u64).saturating_sub(self.elapsed_secs()))
    }

    /// All players, indexed by id.
    pub fn players(&self) -> &[Player] {
        &self.players
    }

    /// One player. Panics on an unknown id.
    pub fn player(&self, p: PlayerId) -> &Player {
        &self.players[p as usize]
    }

    /// One player, or `None` for an unknown id.
    pub fn get_player(&self, p: PlayerId) -> Option<&Player> {
        self.players.get(p as usize)
    }

    /// Ids of players still alive, ascending.
    pub fn alive_players(&self) -> Vec<PlayerId> {
        self.players
            .iter()
            .filter(|pl| pl.alive)
            .map(|pl| pl.id)
            .collect()
    }

    /// Whether `p` exists and is alive.
    pub fn is_alive(&self, p: PlayerId) -> bool {
        match self.players.get(p as usize) {
            Some(pl) => pl.alive,
            None => false,
        }
    }

    /// One region's state. Panics on an unknown id.
    pub fn region_state(&self, r: RegionId) -> &RegionState {
        &self.regions[r as usize]
    }

    /// All region states, indexed by region id.
    pub fn regions(&self) -> &[RegionState] {
        &self.regions
    }

    /// Marches in flight, in launch order.
    pub fn marches(&self) -> &[March] {
        &self.marches
    }

    /// Recalled troops on their way to pools.
    pub fn recalls(&self) -> &[PendingRecall] {
        &self.recalls
    }

    /// Pending alliance proposals.
    pub fn proposals(&self) -> &[Proposal] {
        &self.proposals
    }

    /// Active truces.
    pub fn truces(&self) -> &[Truce] {
        &self.truces
    }

    /// The outcome, once the match is over.
    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    /// Whether the match has an outcome.
    pub fn is_over(&self) -> bool {
        self.outcome.is_some()
    }

    /// Every applied externally queued command (normally the human's), stamped with its tick.
    pub fn command_log(&self) -> &[IssuedCommand] {
        &self.command_log
    }

    /// The bot brain driving `p`, if `p` is bot-controlled.
    pub fn bot_brain(&self, p: PlayerId) -> Option<&BotBrain> {
        self.brains.iter().find(|b| b.player == p)
    }

    /// The tick of the most recent capture anywhere (0 if none yet).
    pub fn last_capture_tick(&self) -> Tick {
        self.last_capture_tick
    }

    // ----------------------------------------------------------- economy --

    /// Pool of `p` in whole troops (0 for unknown players).
    pub fn pool(&self, p: PlayerId) -> i64 {
        match self.players.get(p as usize) {
            Some(pl) => pl.pool(),
            None => 0,
        }
    }

    /// Pool cap of `p`: `pool_cap_base + pool_cap_per_value * owned_value`.
    pub fn pool_cap(&self, p: PlayerId) -> i64 {
        let v = self.owned_value(p) as i64;
        (self.balance.pool_cap_base + self.balance.pool_cap_per_value * v).max(1)
    }

    /// Current pool growth of `p` in troops per second (after growth scaling).
    pub fn growth_per_sec(&self, p: PlayerId) -> i64 {
        self.growth_milli_per_sec(p) / 1000
    }

    /// Current pool growth of `p` in milli-troops per second (after growth scaling).
    pub fn growth_milli_per_sec(&self, p: PlayerId) -> i64 {
        let pl = match self.players.get(p as usize) {
            Some(pl) => pl,
            None => return 0,
        };
        if !pl.alive {
            return 0;
        }
        let b = &self.balance;
        let cap = self.pool_cap(p).saturating_mul(1000);
        let pool = pl.pool_milli.max(0);
        if pool >= cap {
            return 0;
        }
        let v = pl.owned_value as i64;
        let raw = b.growth_base_milli
            + b.growth_per_value_milli * v
            + (pool / 1000) * b.growth_pool_permille;
        let raw = raw.max(0);
        let logistic = (raw as i128) * ((cap - pool) as i128) / (cap as i128);
        let scaled = logistic * (self.config.growth_scale_pct as i128) / 100;
        scaled as i64
    }

    /// Total value of regions owned by `p`.
    pub fn owned_value(&self, p: PlayerId) -> u64 {
        match self.players.get(p as usize) {
            Some(pl) => pl.owned_value,
            None => 0,
        }
    }

    /// Owned value as a percent of world value, rounded down.
    pub fn land_pct(&self, p: PlayerId) -> u32 {
        let total = self.map.total_value();
        if total == 0 {
            return 0;
        }
        (self.owned_value(p) * 100 / total) as u32
    }

    /// Number of regions owned by `p`.
    pub fn region_count(&self, p: PlayerId) -> u32 {
        match self.players.get(p as usize) {
            Some(pl) => pl.region_count,
            None => 0,
        }
    }

    /// Pool plus all garrisons of `p`, in troops.
    pub fn total_troops(&self, p: PlayerId) -> i64 {
        let garrisons: i64 = self
            .regions
            .iter()
            .filter(|st| st.owner == Some(p))
            .map(|st| st.garrison.max(0))
            .sum();
        self.pool(p) + garrisons
    }

    /// Score: `owned_value * 100 + (pool + garrisons) / score_troops_per_point`.
    pub fn score(&self, p: PlayerId) -> u64 {
        let per_point = self.balance.score_troops_per_point.max(1);
        let bonus = (self.total_troops(p).max(0) / per_point) as u64;
        self.owned_value(p) * 100 + bonus
    }

    /// Player ids ordered alive first, then by score descending, then id ascending.
    pub fn leaderboard(&self) -> Vec<PlayerId> {
        let mut rows: Vec<(bool, u64, PlayerId)> = self
            .players
            .iter()
            .map(|pl| (pl.alive, self.score(pl.id), pl.id))
            .collect();
        rows.sort_by(|x, y| y.0.cmp(&x.0).then(y.1.cmp(&x.1)).then(x.2.cmp(&y.2)));
        rows.into_iter().map(|row| row.2).collect()
    }

    /// Players in `candidates` with the highest score; ties go to the lower id.
    pub(crate) fn best_by_score(&self, candidates: &[PlayerId]) -> Option<PlayerId> {
        let mut best: Option<(u64, PlayerId)> = None;
        for &p in candidates {
            let s = self.score(p);
            best = match best {
                None => Some((s, p)),
                Some((bs, bp)) => {
                    if s > bs || (s == bs && p < bp) {
                        Some((s, p))
                    } else {
                        Some((bs, bp))
                    }
                }
            };
        }
        best.map(|(_, p)| p)
    }

    // ----------------------------------------------------------- regions --

    /// Defender multiplier of region `r` in permille (see ARCHITECTURE 3.6).
    pub fn defense_bonus_permille(&self, r: RegionId) -> u32 {
        let b = &self.balance;
        let st = match self.regions.get(r as usize) {
            Some(st) => st,
            None => return b.defense_base_permille.max(1),
        };
        match st.owner {
            None => b.neutral_defense_permille.max(1),
            Some(o) => {
                let mut bonus = b.defense_base_permille;
                let capital = self.players.get(o as usize).and_then(|pl| pl.capital);
                if capital == Some(r) {
                    bonus = bonus.saturating_add(b.capital_bonus_permille);
                }
                if self.tick < st.fortified_until {
                    bonus = bonus.saturating_add(b.fortified_bonus_permille);
                }
                bonus.max(1)
            }
        }
    }

    /// Effective defense of region `r`: `garrison * bonus / 1000`.
    pub fn effective_defense(&self, r: RegionId) -> i64 {
        let garrison = match self.regions.get(r as usize) {
            Some(st) => st.garrison.max(0),
            None => return 0,
        };
        garrison * self.defense_bonus_permille(r) as i64 / 1000
    }

    /// The garrison regeneration cap of region `r` if owned.
    pub fn garrison_cap(&self, r: RegionId) -> i64 {
        match self.map.get_region(r) {
            Some(info) => self.balance.garrison_cap(info.value),
            None => 0,
        }
    }

    /// Regions owned by `p`, ascending.
    pub fn owned_regions(&self, p: PlayerId) -> Vec<RegionId> {
        self.regions
            .iter()
            .enumerate()
            .filter(|(_, st)| st.owner == Some(p))
            .map(|(i, _)| i as RegionId)
            .collect()
    }

    /// Regions owned by `p` that touch at least one region `p` does not own, ascending.
    pub fn border_regions(&self, p: PlayerId) -> Vec<RegionId> {
        let mut out = Vec::new();
        for (i, st) in self.regions.iter().enumerate() {
            if st.owner != Some(p) {
                continue;
            }
            let r = i as RegionId;
            let exposed = self.map.neighbors(r).iter().any(|&(n, _, _)| {
                match self.regions.get(n as usize) {
                    Some(ns) => ns.owner != Some(p),
                    None => false,
                }
            });
            if exposed {
                out.push(r);
            }
        }
        out
    }

    /// Marches currently heading to region `r`.
    pub fn marches_to(&self, r: RegionId) -> Vec<March> {
        self.marches.iter().filter(|m| m.to == r).copied().collect()
    }

    /// For an attack by `p` on `r`: the departure region (the owned neighbor with
    /// the shortest edge, ties to the lower id) and the march time in ticks.
    /// `None` if `p` owns no neighbor of `r`.
    pub fn attack_route(&self, p: PlayerId, r: RegionId) -> Option<(RegionId, u64)> {
        let mut best: Option<(RegionId, u32)> = None;
        for &(n, len, _) in self.map.neighbors(r) {
            let owned = match self.regions.get(n as usize) {
                Some(st) => st.owner == Some(p),
                None => false,
            };
            if !owned {
                continue;
            }
            best = match best {
                None => Some((n, len)),
                Some((bn, bl)) => {
                    if len < bl {
                        Some((n, len))
                    } else {
                        Some((bn, bl))
                    }
                }
            };
        }
        best.map(|(n, len)| (n, self.march_ticks(len)))
    }

    /// March time in ticks for an edge of `length_km`.
    pub fn march_ticks(&self, length_km: u32) -> u64 {
        let b = &self.balance;
        let t = (length_km as u64) * b.march_ticks_per_1000km / 1000;
        t.max(b.min_march_ticks).max(1)
    }

    /// Whether `p` may attack `r` right now: `p` is alive, `r` is adjacent to `p`'s
    /// territory, not owned by `p`, not owned by an ally and not under a truce.
    pub fn is_attackable_by(&self, p: PlayerId, r: RegionId) -> bool {
        if !self.is_alive(p) {
            return false;
        }
        let st = match self.regions.get(r as usize) {
            Some(st) => st,
            None => return false,
        };
        if let Some(o) = st.owner {
            if o == p || self.are_allied(p, o) || self.has_truce(p, o) {
                return false;
            }
        }
        self.attack_route(p, r).is_some()
    }

    /// Every region `p` may attack right now, ascending by id.
    pub fn attackable_regions(&self, p: PlayerId) -> Vec<RegionId> {
        let mut out: Vec<RegionId> = Vec::new();
        if !self.is_alive(p) {
            return out;
        }
        let n = self.regions.len();
        let mut seen: Vec<bool> = vec![false; n];
        for (i, st) in self.regions.iter().enumerate() {
            if st.owner != Some(p) {
                continue;
            }
            for &(nb, _, _) in self.map.neighbors(i as RegionId) {
                let ni = nb as usize;
                if ni >= n || seen[ni] {
                    continue;
                }
                seen[ni] = true;
                if let Some(o) = self.regions[ni].owner {
                    if o == p || self.are_allied(p, o) || self.has_truce(p, o) {
                        continue;
                    }
                }
                out.push(nb);
            }
        }
        out.sort_unstable();
        out
    }

    // --------------------------------------------------------- diplomacy --

    /// Whether `a` and `b` are allied.
    pub fn are_allied(&self, a: PlayerId, b: PlayerId) -> bool {
        a != b && self.alliances.contains(&pair(a, b))
    }

    /// Allies of `p`, ascending.
    pub fn allies_of(&self, p: PlayerId) -> Vec<PlayerId> {
        let mut out: Vec<PlayerId> = Vec::new();
        for &(a, b) in &self.alliances {
            if a == p {
                out.push(b);
            } else if b == p {
                out.push(a);
            }
        }
        out.sort_unstable();
        out
    }

    /// Whether `from` has a pending proposal to `to`.
    pub fn pending_proposal(&self, from: PlayerId, to: PlayerId) -> bool {
        self.proposals.iter().any(|pr| pr.from == from && pr.to == to)
    }

    /// Whether a truce between `a` and `b` is in effect.
    pub fn has_truce(&self, a: PlayerId, b: PlayerId) -> bool {
        self.truce_ticks_left(a, b).is_some()
    }

    /// Ticks left on the truce between `a` and `b`, if one is in effect.
    pub fn truce_ticks_left(&self, a: PlayerId, b: PlayerId) -> Option<u64> {
        if a == b {
            return None;
        }
        let key = pair(a, b);
        for t in &self.truces {
            if (t.a, t.b) == key && t.until > self.tick {
                return Some(t.until - self.tick);
            }
        }
        None
    }

    /// Finds a player by name: case-insensitive exact match first, then a unique prefix.
    pub fn find_player(&self, name: &str) -> Option<PlayerId> {
        let q = name.trim().to_lowercase();
        if q.is_empty() {
            return None;
        }
        for pl in &self.players {
            if pl.name.to_lowercase() == q {
                return Some(pl.id);
            }
        }
        let mut found: Option<PlayerId> = None;
        let mut count = 0usize;
        for pl in &self.players {
            if pl.name.to_lowercase().starts_with(&q) {
                count += 1;
                found = Some(pl.id);
            }
        }
        if count == 1 {
            found
        } else {
            None
        }
    }

    // ----------------------------------------------------------- helpers --

    /// Display name of region `r` (or `#r` if unknown).
    pub(crate) fn region_name(&self, r: RegionId) -> String {
        match self.map.get_region(r) {
            Some(info) => info.name.clone(),
            None => format!("#{}", r),
        }
    }

    /// Display name of player `p` (or `Player p` if unknown).
    pub(crate) fn player_name(&self, p: PlayerId) -> String {
        match self.players.get(p as usize) {
            Some(pl) => pl.name.clone(),
            None => format!("Player {}", p),
        }
    }
}
