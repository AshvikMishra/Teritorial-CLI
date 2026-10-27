//! The fixed-tick simulation: match setup, command validation and application,
//! economy, marches, combat, diplomacy timers, eliminations and win checks.
//!
//! All math is integer. One call to [`Game::step`] advances exactly one tick
//! (a tenth of a second) in this order (ARCHITECTURE 3.6):
//!
//! 1. `tick += 1`.
//! 2. Apply queued commands in queue order, re-validating each.
//! 3. Run bots whose reaction timer is due, in ascending player id.
//! 4. Economy: pool growth, then garrison regeneration (once per second).
//! 5. Resolve march arrivals ordered by `(arrive_tick, march id)`.
//! 6. Recalled troops reach pools.
//! 7. Expire proposals and truces.
//! 8. Eliminations, capital moves, then the win check.
//!
//! Once the match has an outcome the simulation is frozen: `step()` still
//! advances the tick counter (so the UI clock and replays keep moving), but it
//! rejects every queued command with [`CommandError::MatchOver`] and changes
//! nothing else. The `MatchOver` event is emitted exactly once.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::balance::Balance;
use crate::bots::{reaction_range, BotBrain, Personality};
use crate::command::{Amount, Command, CommandError, CommandPreview, IssuedCommand};
use crate::config::{Difficulty, MatchConfig, StartChoice, WinCondition};
use crate::map::WorldMap;
use crate::rng::Rng;
use crate::state::{
    pair, EventKind, Game, GameEvent, March, Outcome, PendingRecall, Player, PlayerKind,
    PlayerStats, Proposal, QueuedCommand, RegionState, Truce, WinReason,
};
use crate::{PlayerId, RegionId, MAX_BOTS, TICKS_PER_SECOND};

/// A command that passed validation, with its resolved amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Validated {
    pub(crate) troops: i64,
    pub(crate) source: Option<RegionId>,
    pub(crate) eta: Option<u64>,
}

impl Game {
    /// Creates a new match: neutral garrisons, players (human 0, bots `1..=bots`),
    /// personalities and difficulties, spaced start regions and bot brains.
    ///
    /// The config should be validated with [`MatchConfig::validate`] first; an
    /// unvalidated config never panics, but players without a start region are
    /// eliminated on the first tick.
    pub fn new(map: Arc<WorldMap>, balance: Balance, config: MatchConfig) -> Game {
        let mut rng = Rng::new(config.seed);
        let bots = config.bots.min(MAX_BOTS);
        let player_count = bots as usize + 1;

        let regions: Vec<RegionState> = map
            .regions
            .iter()
            .map(|info| RegionState {
                owner: None,
                garrison: balance.neutral_garrison(info.value).max(0),
                fortified_until: 0,
            })
            .collect();

        let mut personalities: Vec<Personality> = Personality::ALL.to_vec();
        rng.shuffle(&mut personalities);
        let mixed: [Difficulty; 3] = [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard];
        let diff_offset = rng.below(3) as usize;

        let mut players: Vec<Player> = Vec::with_capacity(player_count);
        let mut brains: Vec<BotBrain> = Vec::new();
        for i in 0..player_count {
            let id = i as PlayerId;
            let personality = personalities[i % personalities.len()];
            let difficulty = match config.difficulty {
                Difficulty::Mixed => mixed[(diff_offset + i) % mixed.len()],
                d => d,
            };
            let bot_driven = i != 0 || config.spectate;
            let kind = if bot_driven {
                PlayerKind::Bot {
                    personality,
                    difficulty,
                }
            } else {
                PlayerKind::Human
            };
            let name = if i == 0 {
                let n = config.human_name.trim();
                if n.is_empty() {
                    "You".to_string()
                } else {
                    n.to_string()
                }
            } else {
                format!("Bot-{}", i)
            };
            players.push(Player {
                id,
                name,
                kind,
                alive: true,
                capital: None,
                stats: PlayerStats::default(),
                pool_milli: balance.start_pool.max(0).saturating_mul(1000),
                owned_value: 0,
                region_count: 0,
                last_lost_to: None,
            });
            if bot_driven {
                let (lo, _) = reaction_range(&balance, difficulty);
                let first = rng.range_u64(1, lo.max(1));
                brains.push(BotBrain::new(id, personality, difficulty, first));
            }
        }

        let starts = choose_starts(&map, &balance, &config, player_count, &mut rng);

        let mut game = Game {
            map,
            balance,
            config,
            tick: 0,
            rng,
            players,
            regions,
            marches: Vec::new(),
            next_march_id: 1,
            recalls: Vec::new(),
            alliances: BTreeSet::new(),
            proposals: Vec::new(),
            truces: Vec::new(),
            pending_commands: Vec::new(),
            command_log: Vec::new(),
            outcome: None,
            brains,
            last_capture_tick: 0,
        };
        let start_garrison = game.balance.start_garrison.max(1);
        for (i, start) in starts.iter().enumerate() {
            if let Some(r) = *start {
                game.set_owner(r, Some(i as PlayerId));
                game.regions[r as usize].garrison = start_garrison;
                game.players[i].capital = Some(r);
            }
        }
        game
    }

    /// Queues a command for player `p`. It is validated and applied at the start
    /// of the next [`Game::step`]; if valid it is appended to the command log.
    pub fn queue(&mut self, p: PlayerId, cmd: Command) {
        self.pending_commands.push(QueuedCommand {
            player: p,
            command: cmd,
        });
    }

    /// Validates a command for player `p` against the current state without
    /// changing anything, and reports what it would do.
    pub fn preview(&self, p: PlayerId, cmd: &Command) -> Result<CommandPreview, CommandError> {
        let v = self.validate_command(p, cmd)?;
        let (target_garrison, target_owner, defense_needed) = match *cmd {
            Command::Attack { target, .. } => {
                let st = self.regions[target as usize];
                (st.garrison, st.owner, self.effective_defense(target))
            }
            Command::Fortify { region, .. } | Command::Recall { region, .. } => {
                let st = self.regions[region as usize];
                (st.garrison, st.owner, 0)
            }
            Command::ProposeAlliance { .. } | Command::BreakAlliance { .. } => (0, None, 0),
        };
        Ok(CommandPreview {
            troops: v.troops,
            eta_ticks: v.eta,
            source: v.source,
            target_garrison,
            target_owner,
            defense_needed,
        })
    }

    /// Advances the simulation by exactly one tick and returns what happened.
    pub fn step(&mut self) -> Vec<GameEvent> {
        let mut events: Vec<GameEvent> = Vec::new();
        self.tick += 1;
        let queued = std::mem::take(&mut self.pending_commands);

        if self.outcome.is_some() {
            for q in queued {
                events.push(GameEvent {
                    tick: self.tick,
                    kind: EventKind::CommandRejected {
                        player: q.player,
                        command: q.command,
                        reason: CommandError::MatchOver,
                    },
                });
            }
            return events;
        }

        for q in queued {
            if self.apply_command(q.player, q.command, &mut events) {
                self.command_log.push(IssuedCommand {
                    tick: self.tick,
                    player: q.player,
                    command: q.command,
                });
            }
        }
        self.run_bots(&mut events);
        self.run_economy();
        self.advance_marches(&mut events);
        self.arrive_recalls(&mut events);
        self.expire_diplomacy(&mut events);
        self.check_eliminations(&mut events);
        self.check_win(&mut events);
        events
    }

    // -------------------------------------------------------- validation --

    /// The single validation path shared by `preview`, queued commands and bots.
    pub(crate) fn validate_command(
        &self,
        p: PlayerId,
        cmd: &Command,
    ) -> Result<Validated, CommandError> {
        if self.outcome.is_some() {
            return Err(CommandError::MatchOver);
        }
        let me = match self.players.get(p as usize) {
            Some(pl) => pl,
            None => return Err(CommandError::UnknownPlayer),
        };
        if !me.alive {
            return Err(CommandError::IssuerEliminated);
        }
        let pool = me.pool();
        match *cmd {
            Command::Attack { target, amount } => {
                self.check_region(target)?;
                let owner = self.regions[target as usize].owner;
                if owner == Some(p) {
                    return Err(CommandError::OwnRegion {
                        region: self.region_name(target),
                    });
                }
                if let Some(o) = owner {
                    if self.are_allied(p, o) {
                        return Err(CommandError::Allied {
                            player: self.player_name(o),
                        });
                    }
                    if let Some(left) = self.truce_ticks_left(p, o) {
                        return Err(CommandError::Truce {
                            player: self.player_name(o),
                            ticks_left: left,
                        });
                    }
                }
                let (source, eta) = match self.attack_route(p, target) {
                    Some(route) => route,
                    None => {
                        return Err(CommandError::NotAdjacent {
                            region: self.region_name(target),
                        })
                    }
                };
                let max_pct = self.balance.max_attack_pct.clamp(1, 100);
                let troops = resolve_amount(amount, pool, max_pct)?;
                Ok(Validated {
                    troops,
                    source: Some(source),
                    eta: Some(eta),
                })
            }
            Command::Fortify { region, amount } => {
                self.check_region(region)?;
                if self.regions[region as usize].owner != Some(p) {
                    return Err(CommandError::NotOwned {
                        region: self.region_name(region),
                    });
                }
                let troops = resolve_amount(amount, pool, 100)?;
                Ok(Validated {
                    troops,
                    source: None,
                    eta: None,
                })
            }
            Command::Recall { region, amount } => {
                self.check_region(region)?;
                let st = self.regions[region as usize];
                if st.owner != Some(p) {
                    return Err(CommandError::NotOwned {
                        region: self.region_name(region),
                    });
                }
                let movable = (st.garrison - 1).max(0);
                let troops = resolve_amount(amount, movable, 100)?;
                Ok(Validated {
                    troops,
                    source: None,
                    eta: None,
                })
            }
            Command::ProposeAlliance { with } => {
                self.check_diplomacy_target(p, with)?;
                if self.are_allied(p, with) {
                    return Err(CommandError::AlreadyAllied {
                        player: self.player_name(with),
                    });
                }
                if self.pending_proposal(p, with) {
                    return Err(CommandError::AlreadyProposed {
                        player: self.player_name(with),
                    });
                }
                let max = self.balance.max_allies as usize;
                if self.allies_of(p).len() >= max {
                    return Err(CommandError::AllianceLimit {
                        player: self.player_name(p),
                    });
                }
                if self.allies_of(with).len() >= max {
                    return Err(CommandError::AllianceLimit {
                        player: self.player_name(with),
                    });
                }
                Ok(Validated {
                    troops: 0,
                    source: None,
                    eta: None,
                })
            }
            Command::BreakAlliance { with } => {
                self.check_diplomacy_target(p, with)?;
                if !self.are_allied(p, with) {
                    return Err(CommandError::NotAllied {
                        player: self.player_name(with),
                    });
                }
                Ok(Validated {
                    troops: 0,
                    source: None,
                    eta: None,
                })
            }
        }
    }

    fn check_region(&self, r: RegionId) -> Result<(), CommandError> {
        if (r as usize) < self.regions.len() && (r as usize) < self.map.len() {
            Ok(())
        } else {
            Err(CommandError::UnknownRegion)
        }
    }

    fn check_diplomacy_target(&self, p: PlayerId, with: PlayerId) -> Result<(), CommandError> {
        if !self.config.alliances {
            return Err(CommandError::AlliancesDisabled);
        }
        if with == p {
            return Err(CommandError::SelfTarget);
        }
        let other = match self.players.get(with as usize) {
            Some(pl) => pl,
            None => return Err(CommandError::UnknownPlayer),
        };
        if !other.alive {
            return Err(CommandError::PlayerEliminated {
                player: other.name.clone(),
            });
        }
        Ok(())
    }

    // ------------------------------------------------------- application --

    /// Validates and applies one command. Returns `true` if it was applied;
    /// a rejection is reported as a `CommandRejected` event.
    pub(crate) fn apply_command(
        &mut self,
        p: PlayerId,
        cmd: Command,
        events: &mut Vec<GameEvent>,
    ) -> bool {
        match self.validate_command(p, &cmd) {
            Ok(v) => {
                self.execute(p, cmd, v, events);
                true
            }
            Err(reason) => {
                events.push(GameEvent {
                    tick: self.tick,
                    kind: EventKind::CommandRejected {
                        player: p,
                        command: cmd,
                        reason,
                    },
                });
                false
            }
        }
    }

    fn execute(&mut self, p: PlayerId, cmd: Command, v: Validated, events: &mut Vec<GameEvent>) {
        let tick = self.tick;
        let pi = p as usize;
        match cmd {
            Command::Attack { target, .. } => {
                let (from, eta) = match (v.source, v.eta) {
                    (Some(s), Some(e)) => (s, e),
                    _ => return,
                };
                {
                    let pl = &mut self.players[pi];
                    pl.pool_milli -= v.troops.saturating_mul(1000);
                    pl.stats.troops_sent = pl.stats.troops_sent.saturating_add(v.troops as u64);
                }
                let id = self.next_march_id;
                self.next_march_id = self.next_march_id.wrapping_add(1).max(1);
                self.marches.push(March {
                    id,
                    owner: p,
                    from,
                    to: target,
                    troops: v.troops,
                    depart_tick: tick,
                    arrive_tick: tick + eta,
                });
                events.push(GameEvent {
                    tick,
                    kind: EventKind::CommandApplied {
                        player: p,
                        command: cmd,
                        troops: v.troops,
                    },
                });
                events.push(GameEvent {
                    tick,
                    kind: EventKind::MarchStarted {
                        march_id: id,
                        player: p,
                        from,
                        to: target,
                        troops: v.troops,
                        eta_ticks: eta,
                    },
                });
            }
            Command::Fortify { region, .. } => {
                self.players[pi].pool_milli -= v.troops.saturating_mul(1000);
                let until = tick + self.balance.fortify_duration_ticks;
                {
                    let st = &mut self.regions[region as usize];
                    st.garrison += v.troops;
                    st.fortified_until = until;
                }
                events.push(GameEvent {
                    tick,
                    kind: EventKind::CommandApplied {
                        player: p,
                        command: cmd,
                        troops: v.troops,
                    },
                });
            }
            Command::Recall { region, .. } => {
                self.regions[region as usize].garrison -= v.troops;
                let arrive_tick = tick + self.balance.recall_delay_ticks;
                self.recalls.push(PendingRecall {
                    player: p,
                    troops: v.troops,
                    arrive_tick,
                });
                events.push(GameEvent {
                    tick,
                    kind: EventKind::CommandApplied {
                        player: p,
                        command: cmd,
                        troops: v.troops,
                    },
                });
            }
            Command::ProposeAlliance { with } => {
                events.push(GameEvent {
                    tick,
                    kind: EventKind::CommandApplied {
                        player: p,
                        command: cmd,
                        troops: 0,
                    },
                });
                if self.pending_proposal(with, p) {
                    self.proposals.retain(|pr| {
                        !((pr.from == with && pr.to == p) || (pr.from == p && pr.to == with))
                    });
                    let key = pair(p, with);
                    self.alliances.insert(key);
                    self.truces.retain(|t| (t.a, t.b) != key);
                    events.push(GameEvent {
                        tick,
                        kind: EventKind::AllianceFormed { a: key.0, b: key.1 },
                    });
                } else {
                    let expires = tick + self.balance.proposal_ttl_ticks.max(1);
                    self.proposals.push(Proposal {
                        from: p,
                        to: with,
                        expires,
                    });
                    events.push(GameEvent {
                        tick,
                        kind: EventKind::AllianceProposed { from: p, to: with },
                    });
                }
            }
            Command::BreakAlliance { with } => {
                let key = pair(p, with);
                self.alliances.remove(&key);
                self.truces.retain(|t| (t.a, t.b) != key);
                let until = tick + self.balance.truce_ticks;
                if until > tick {
                    self.truces.push(Truce {
                        a: key.0,
                        b: key.1,
                        until,
                    });
                }
                events.push(GameEvent {
                    tick,
                    kind: EventKind::CommandApplied {
                        player: p,
                        command: cmd,
                        troops: 0,
                    },
                });
                events.push(GameEvent {
                    tick,
                    kind: EventKind::AllianceBroken { by: p, with },
                });
            }
        }
    }

    // ---------------------------------------------------------- ownership --

    /// Changes the owner of `r`, keeping the per-player territory caches in sync.
    pub(crate) fn set_owner(&mut self, r: RegionId, new_owner: Option<PlayerId>) {
        let ri = r as usize;
        if ri >= self.regions.len() {
            return;
        }
        let old = self.regions[ri].owner;
        if old == new_owner {
            return;
        }
        let value = match self.map.get_region(r) {
            Some(info) => info.value as u64,
            None => 0,
        };
        if let Some(o) = old {
            if let Some(pl) = self.players.get_mut(o as usize) {
                pl.owned_value = pl.owned_value.saturating_sub(value);
                pl.region_count = pl.region_count.saturating_sub(1);
            }
        }
        if let Some(n) = new_owner {
            if let Some(pl) = self.players.get_mut(n as usize) {
                pl.owned_value += value;
                pl.region_count += 1;
                if pl.region_count > pl.stats.peak_regions {
                    pl.stats.peak_regions = pl.region_count;
                }
            }
        }
        self.regions[ri].owner = new_owner;
    }

    /// Rebuilds the per-player territory caches from the region owners.
    pub(crate) fn recompute_caches(&mut self) {
        for pl in self.players.iter_mut() {
            pl.owned_value = 0;
            pl.region_count = 0;
        }
        for (i, st) in self.regions.iter().enumerate() {
            if let Some(o) = st.owner {
                let value = match self.map.get_region(i as RegionId) {
                    Some(info) => info.value as u64,
                    None => 0,
                };
                if let Some(pl) = self.players.get_mut(o as usize) {
                    pl.owned_value += value;
                    pl.region_count += 1;
                }
            }
        }
    }

    // ------------------------------------------------------------ economy --

    fn run_economy(&mut self) {
        for i in 0..self.players.len() {
            if !self.players[i].alive {
                continue;
            }
            let per_tick = self.growth_milli_per_sec(i as PlayerId) / TICKS_PER_SECOND as i64;
            let cap = self.pool_cap(i as PlayerId).saturating_mul(1000);
            let pl = &mut self.players[i];
            if pl.pool_milli < cap && per_tick > 0 {
                pl.pool_milli = (pl.pool_milli + per_tick).min(cap);
            }
        }

        if self.tick % TICKS_PER_SECOND != 0 {
            return;
        }
        let b = &self.balance;
        let flat = b.garrison_regen_flat;
        let regen = b.garrison_regen_permille;
        let decay = b.garrison_overcap_decay_permille;
        let n = self.regions.len().min(self.map.len());
        for i in 0..n {
            let value = self.map.regions[i].value;
            let owned = self.regions[i].owner.is_some();
            let cap = if owned {
                self.balance.garrison_cap(value)
            } else {
                self.balance.neutral_garrison(value)
            };
            let st = &mut self.regions[i];
            if st.garrison < cap {
                let add = flat + (cap - st.garrison) * regen / 1000;
                st.garrison = (st.garrison + add.max(0)).min(cap);
            } else if owned && st.garrison > cap {
                let excess = st.garrison - cap;
                // Rounded up so a small excess still drains to the cap, and
                // never below the cap (decay 0 disables it).
                let d = (excess * decay + 999) / 1000;
                st.garrison -= d.clamp(0, excess);
            }
        }
    }

    // ------------------------------------------------------------ marches --

    fn advance_marches(&mut self, events: &mut Vec<GameEvent>) {
        let tick = self.tick;
        if !self.marches.iter().any(|m| m.arrive_tick <= tick) {
            return;
        }
        let all = std::mem::take(&mut self.marches);
        let mut arrived: Vec<March> = Vec::new();
        for m in all {
            if m.arrive_tick <= tick {
                arrived.push(m);
            } else {
                self.marches.push(m);
            }
        }
        arrived.sort_by_key(|m| (m.arrive_tick, m.id));
        for m in arrived {
            self.resolve_arrival(m, events);
        }
    }

    fn resolve_arrival(&mut self, m: March, events: &mut Vec<GameEvent>) {
        let tick = self.tick;
        let ri = m.to as usize;
        if ri >= self.regions.len() {
            return;
        }
        let attacker = m.owner;
        let ai = attacker as usize;
        if ai >= self.players.len() {
            return;
        }
        let owner = self.regions[ri].owner;

        if owner == Some(attacker) {
            self.regions[ri].garrison += m.troops;
            events.push(GameEvent {
                tick,
                kind: EventKind::Reinforced {
                    region: m.to,
                    player: attacker,
                    troops: m.troops,
                },
            });
            return;
        }
        if let Some(o) = owner {
            if self.are_allied(attacker, o) || self.has_truce(attacker, o) {
                if self.players[ai].alive {
                    self.players[ai].pool_milli += m.troops.saturating_mul(1000);
                    events.push(GameEvent {
                        tick,
                        kind: EventKind::RecallArrived {
                            player: attacker,
                            troops: m.troops,
                        },
                    });
                }
                return;
            }
        }

        let bonus = self.defense_bonus_permille(m.to).max(1) as i64;
        let garrison = self.regions[ri].garrison.max(0);
        let def = garrison * bonus / 1000;
        if m.troops > def {
            let survivors = m.troops - def;
            self.set_owner(m.to, Some(attacker));
            {
                let st = &mut self.regions[ri];
                st.garrison = survivors;
                st.fortified_until = 0;
            }
            {
                let stats = &mut self.players[ai].stats;
                stats.regions_captured += 1;
                stats.troops_lost = stats.troops_lost.saturating_add(def as u64);
            }
            if let Some(o) = owner {
                if let Some(pl) = self.players.get_mut(o as usize) {
                    pl.stats.regions_lost += 1;
                    pl.stats.troops_lost = pl.stats.troops_lost.saturating_add(garrison as u64);
                    pl.last_lost_to = Some(attacker);
                }
            }
            self.last_capture_tick = tick;
            events.push(GameEvent {
                tick,
                kind: EventKind::Captured {
                    region: m.to,
                    by: attacker,
                    from: owner,
                    survivors,
                },
            });
        } else {
            let loss = (m.troops.saturating_mul(1000) / bonus).min(garrison).max(0);
            self.regions[ri].garrison = garrison - loss;
            {
                let stats = &mut self.players[ai].stats;
                stats.troops_lost = stats.troops_lost.saturating_add(m.troops.max(0) as u64);
            }
            if let Some(o) = owner {
                if let Some(pl) = self.players.get_mut(o as usize) {
                    pl.stats.troops_lost = pl.stats.troops_lost.saturating_add(loss as u64);
                }
            }
            events.push(GameEvent {
                tick,
                kind: EventKind::AttackRepelled {
                    region: m.to,
                    attacker,
                    defender: owner,
                    attacker_lost: m.troops,
                    defender_lost: loss,
                },
            });
        }
    }

    // ---------------------------------------------------- recalls, timers --

    fn arrive_recalls(&mut self, events: &mut Vec<GameEvent>) {
        let tick = self.tick;
        if !self.recalls.iter().any(|r| r.arrive_tick <= tick) {
            return;
        }
        let all = std::mem::take(&mut self.recalls);
        for rc in all {
            if rc.arrive_tick > tick {
                self.recalls.push(rc);
                continue;
            }
            let alive = self.is_alive(rc.player);
            if alive {
                self.players[rc.player as usize].pool_milli += rc.troops.saturating_mul(1000);
                events.push(GameEvent {
                    tick,
                    kind: EventKind::RecallArrived {
                        player: rc.player,
                        troops: rc.troops,
                    },
                });
            }
        }
    }

    fn expire_diplomacy(&mut self, events: &mut Vec<GameEvent>) {
        let tick = self.tick;
        if self.proposals.iter().any(|pr| pr.expires <= tick) {
            let all = std::mem::take(&mut self.proposals);
            for pr in all {
                if pr.expires <= tick {
                    events.push(GameEvent {
                        tick,
                        kind: EventKind::ProposalExpired {
                            from: pr.from,
                            to: pr.to,
                        },
                    });
                } else {
                    self.proposals.push(pr);
                }
            }
        }
        self.truces.retain(|t| t.until > tick);
    }

    // ------------------------------------------------ eliminations, win --

    fn check_eliminations(&mut self, events: &mut Vec<GameEvent>) {
        let tick = self.tick;
        for i in 0..self.players.len() {
            if !self.players[i].alive || self.players[i].region_count > 0 {
                continue;
            }
            let pid = i as PlayerId;
            let by = self.players[i].last_lost_to;
            {
                let pl = &mut self.players[i];
                pl.alive = false;
                pl.capital = None;
                pl.pool_milli = 0;
                pl.stats.eliminated_at = Some(tick);
                pl.stats.eliminated_by = by;
            }
            self.marches.retain(|m| m.owner != pid);
            self.recalls.retain(|r| r.player != pid);
            self.alliances.retain(|&(a, b)| a != pid && b != pid);
            self.proposals.retain(|pr| pr.from != pid && pr.to != pid);
            self.truces.retain(|t| t.a != pid && t.b != pid);
            events.push(GameEvent {
                tick,
                kind: EventKind::Eliminated { player: pid, by },
            });
        }

        // Capitals: a living player whose capital was lost moves it to their
        // highest-value region (ties to the lower id).
        for i in 0..self.players.len() {
            if !self.players[i].alive {
                continue;
            }
            let pid = i as PlayerId;
            let still_owned = match self.players[i].capital {
                Some(c) => match self.regions.get(c as usize) {
                    Some(st) => st.owner == Some(pid),
                    None => false,
                },
                None => false,
            };
            if !still_owned {
                let new_capital = self.best_region_of(pid);
                self.players[i].capital = new_capital;
            }
        }
    }

    /// The highest-value region owned by `p` (ties to the lower id).
    fn best_region_of(&self, p: PlayerId) -> Option<RegionId> {
        let mut best: Option<(u32, RegionId)> = None;
        for (i, st) in self.regions.iter().enumerate() {
            if st.owner != Some(p) {
                continue;
            }
            let r = i as RegionId;
            let value = match self.map.get_region(r) {
                Some(info) => info.value,
                None => 0,
            };
            best = match best {
                None => Some((value, r)),
                Some((bv, br)) => {
                    if value > bv {
                        Some((value, r))
                    } else {
                        Some((bv, br))
                    }
                }
            };
        }
        best.map(|(_, r)| r)
    }

    fn check_win(&mut self, events: &mut Vec<GameEvent>) {
        if self.outcome.is_some() {
            return;
        }
        if let Some(outcome) = self.evaluate_outcome() {
            self.outcome = Some(outcome.clone());
            events.push(GameEvent {
                tick: self.tick,
                kind: EventKind::MatchOver { outcome },
            });
        }
    }

    fn evaluate_outcome(&self) -> Option<Outcome> {
        let alive = self.alive_players();
        let timed_only = self.config.win == WinCondition::Timed;
        if !timed_only {
            if let WinCondition::Domination { pct } = self.config.win {
                let qualifiers: Vec<PlayerId> = alive
                    .iter()
                    .copied()
                    .filter(|&p| self.land_pct(p) >= pct as u32)
                    .collect();
                if let Some(w) = self.best_by_score(&qualifiers) {
                    return Some(Outcome::Winner {
                        player: w,
                        reason: WinReason::Domination,
                    });
                }
            }
            if alive.len() == 1 {
                return Some(Outcome::Winner {
                    player: alive[0],
                    reason: WinReason::Elimination,
                });
            }
            if alive.len() >= 2 && self.all_mutually_allied(&alive) {
                if let Some(top) = self.best_by_score(&alive) {
                    return Some(Outcome::AlliedVictory {
                        players: alive.clone(),
                        top,
                    });
                }
            }
        }
        if let Some(limit) = self.config.time_limit_ticks() {
            if self.tick >= limit {
                let pool: Vec<PlayerId> = if alive.is_empty() {
                    self.players.iter().map(|pl| pl.id).collect()
                } else {
                    alive.clone()
                };
                if let Some(w) = self.best_by_score(&pool) {
                    return Some(Outcome::Winner {
                        player: w,
                        reason: WinReason::Timer,
                    });
                }
            }
        }
        None
    }

    fn all_mutually_allied(&self, players: &[PlayerId]) -> bool {
        for (i, &a) in players.iter().enumerate() {
            for &b in &players[i + 1..] {
                if !self.are_allied(a, b) {
                    return false;
                }
            }
        }
        true
    }
}

/// Resolves an amount against `available` troops. Percents are clamped to `max_pct`.
fn resolve_amount(amount: Amount, available: i64, max_pct: u32) -> Result<i64, CommandError> {
    let available = available.max(0);
    let troops = match amount {
        Amount::Percent(pct) => {
            if pct == 0 {
                return Err(CommandError::ZeroAmount);
            }
            if pct > 100 {
                return Err(CommandError::InvalidPercent { pct });
            }
            let eff = (pct as u32).min(max_pct) as i64;
            available * eff / 100
        }
        Amount::Flat(n) => {
            if n == 0 {
                return Err(CommandError::ZeroAmount);
            }
            if n > i64::MAX as u64 {
                i64::MAX
            } else {
                n as i64
            }
        }
    };
    if troops < 1 {
        return Err(CommandError::InsufficientTroops {
            needed: 1,
            available,
        });
    }
    if troops > available {
        return Err(CommandError::InsufficientTroops {
            needed: troops,
            available,
        });
    }
    Ok(troops)
}

/// Picks start regions: index 0 is the human (the chosen region if any), the
/// rest are random regions at least `start_min_hops` apart, relaxing the
/// spacing one hop at a time when that is impossible.
fn choose_starts(
    map: &WorldMap,
    balance: &Balance,
    config: &MatchConfig,
    count: usize,
    rng: &mut Rng,
) -> Vec<Option<RegionId>> {
    let n = map.len();
    let mut result: Vec<Option<RegionId>> = vec![None; count];
    if n == 0 || count == 0 {
        return result;
    }
    let fixed: Option<RegionId> = match config.start {
        StartChoice::Region(r) if (r as usize) < n => Some(r),
        _ => None,
    };
    let need = if fixed.is_some() { count - 1 } else { count };

    let mut candidates: Vec<RegionId> = (0..n)
        .map(|i| i as RegionId)
        .filter(|&r| map.region(r).value <= balance.start_max_value && Some(r) != fixed)
        .collect();
    if candidates.len() < need {
        candidates = (0..n)
            .map(|i| i as RegionId)
            .filter(|&r| Some(r) != fixed)
            .collect();
    }
    rng.shuffle(&mut candidates);

    let mut hops = balance.start_min_hops;
    let chosen: Vec<RegionId> = loop {
        let mut chosen: Vec<RegionId> = Vec::with_capacity(count);
        if let Some(f) = fixed {
            chosen.push(f);
        }
        let mut dist: Vec<u32> = if chosen.is_empty() {
            vec![u32::MAX; n]
        } else {
            map.hop_distances(&chosen)
        };
        for &c in &candidates {
            if chosen.len() >= count {
                break;
            }
            if dist[c as usize] >= hops {
                chosen.push(c);
                let from_c = map.hop_distances(&[c]);
                for (d, fc) in dist.iter_mut().zip(from_c.iter()) {
                    if *fc < *d {
                        *d = *fc;
                    }
                }
            }
        }
        if chosen.len() >= count || hops == 0 {
            break chosen;
        }
        hops -= 1;
    };

    for (slot, r) in result.iter_mut().zip(chosen.iter()) {
        *slot = Some(*r);
    }
    result
}
