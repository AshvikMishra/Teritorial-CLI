//! Bot opponents: a rate-limited, integer utility-scoring decision loop.
//!
//! Bots play by the same rules as the human and issue the same [`Command`]s
//! through the same validation path. Each bot has a [`BotBrain`] stored inside
//! the [`Game`] (so it serializes with saves) holding its personality,
//! difficulty, next decision tick, Warlord focus and diplomacy memory.
//!
//! When a bot's reaction timer fires it:
//!
//! 1. surveys marches and borders once (O(regions + marches)),
//! 2. answers alliance proposals addressed to it (each proposal is judged once),
//! 3. every `bot_diplomacy_interval_ticks`, considers breaking or proposing an alliance,
//! 4. scores attacks on every attackable region, emergency fortifies on
//!    threatened regions, Turtle investments in border garrisons and recalls
//!    from safe interior regions,
//! 5. issues **at most one** command, the one with the highest positive utility,
//! 6. re-rolls its reaction interval from the game RNG.
//!
//! Difficulty changes decision quality and reaction speed, never resources:
//! Easy bots send fixed fractions and only compare against the raw garrison,
//! Normal bots estimate defense and demand a margin, Hard bots also stack
//! attacks with their in-flight troops, account for other players' marches and
//! reinforcements, exploit drained garrisons and fortify earlier. Normal and
//! Hard bots whose pool is above `bot_surplus_pool_pct` of its cap (where growth
//! stalls) attack with their full personality attack percent.
//!
//! Anti-stall: every `bot_stall_ticks` without a capture anywhere raises a stall
//! level that lowers margins and reserves, raises attack sizes and enemy weight,
//! makes alliances rarer and makes bots break alliances, so matches always end.

use serde::{Deserialize, Serialize};

use crate::balance::Balance;
use crate::command::{Amount, Command};
use crate::config::Difficulty;
use crate::rng::Rng;
use crate::state::{Game, GameEvent};
use crate::{PlayerId, RegionId, Tick};

/// Bot play style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Personality {
    /// High attack percentage, frequent wars, low defense.
    Aggressor,
    /// Consolidates and fortifies, attacks only with clear advantage, likes alliances.
    Turtle,
    /// Targets weakened neighbors and players engaged in other wars.
    Opportunist,
    /// Snowballs on one front at a time, then pivots; allies with other fronts.
    Warlord,
}

impl Personality {
    /// All personalities.
    pub const ALL: [Personality; 4] = [
        Personality::Aggressor,
        Personality::Turtle,
        Personality::Opportunist,
        Personality::Warlord,
    ];

    /// Lowercase display name.
    pub fn name(self) -> &'static str {
        match self {
            Personality::Aggressor => "aggressor",
            Personality::Turtle => "turtle",
            Personality::Opportunist => "opportunist",
            Personality::Warlord => "warlord",
        }
    }
}

/// Persistent per-bot state, stored in the `Game` so it is saved and replayed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotBrain {
    /// The player this brain drives.
    pub player: PlayerId,
    /// Play style.
    pub personality: Personality,
    /// Difficulty (never `Mixed`).
    pub difficulty: Difficulty,
    /// The next tick this bot may act on.
    pub next_decision: Tick,
    /// Warlord: the player whose front it is fighting.
    pub focus: Option<PlayerId>,
    /// Tick the current focus was chosen.
    pub focus_since: Tick,
    /// The next tick this bot considers proposing or breaking alliances.
    pub next_diplomacy: Tick,
    /// Proposals already judged, as `(from, expires)`.
    pub considered: Vec<(PlayerId, Tick)>,
}

impl BotBrain {
    /// A fresh brain that first acts on `first_decision`.
    pub(crate) fn new(
        player: PlayerId,
        personality: Personality,
        difficulty: Difficulty,
        first_decision: Tick,
    ) -> BotBrain {
        BotBrain {
            player,
            personality,
            difficulty,
            next_decision: first_decision,
            focus: None,
            focus_since: 0,
            next_diplomacy: first_decision,
            considered: Vec::new(),
        }
    }
}

/// Reaction interval `(min, max)` in ticks for a difficulty (`Mixed` = Normal).
pub(crate) fn reaction_range(b: &Balance, d: Difficulty) -> (u64, u64) {
    let (lo, hi) = match d {
        Difficulty::Easy => (b.bot_react_easy_min_ticks, b.bot_react_easy_max_ticks),
        Difficulty::Normal | Difficulty::Mixed => {
            (b.bot_react_normal_min_ticks, b.bot_react_normal_max_ticks)
        }
        Difficulty::Hard => (b.bot_react_hard_min_ticks, b.bot_react_hard_max_ticks),
    };
    let lo = lo.max(1);
    (lo, hi.max(lo))
}

/// Utility noise in permille for a difficulty, capped at 900.
fn noise_permille(b: &Balance, d: Difficulty) -> i64 {
    let n = match d {
        Difficulty::Easy => b.bot_noise_easy_permille,
        Difficulty::Normal | Difficulty::Mixed => b.bot_noise_normal_permille,
        Difficulty::Hard => b.bot_noise_hard_permille,
    };
    (n as i64).min(900)
}

/// Personality weights, widened to i64 for arithmetic.
#[derive(Clone, Copy, Debug)]
struct Traits {
    attack_pct: i64,
    reserve_pct: i64,
    margin_permille: i64,
    overkill_pct: i64,
    enemy_weight_pct: i64,
    fortify_weight_pct: i64,
    alliance_permille: i64,
}

fn traits(b: &Balance, p: Personality) -> Traits {
    let raw: [u32; 7] = match p {
        Personality::Aggressor => [
            b.bot_aggressor_attack_pct,
            b.bot_aggressor_reserve_pct,
            b.bot_aggressor_margin_permille,
            b.bot_aggressor_overkill_pct,
            b.bot_aggressor_enemy_weight_pct,
            b.bot_aggressor_fortify_weight_pct,
            b.bot_aggressor_alliance_permille,
        ],
        Personality::Turtle => [
            b.bot_turtle_attack_pct,
            b.bot_turtle_reserve_pct,
            b.bot_turtle_margin_permille,
            b.bot_turtle_overkill_pct,
            b.bot_turtle_enemy_weight_pct,
            b.bot_turtle_fortify_weight_pct,
            b.bot_turtle_alliance_permille,
        ],
        Personality::Opportunist => [
            b.bot_opportunist_attack_pct,
            b.bot_opportunist_reserve_pct,
            b.bot_opportunist_margin_permille,
            b.bot_opportunist_overkill_pct,
            b.bot_opportunist_enemy_weight_pct,
            b.bot_opportunist_fortify_weight_pct,
            b.bot_opportunist_alliance_permille,
        ],
        Personality::Warlord => [
            b.bot_warlord_attack_pct,
            b.bot_warlord_reserve_pct,
            b.bot_warlord_margin_permille,
            b.bot_warlord_overkill_pct,
            b.bot_warlord_enemy_weight_pct,
            b.bot_warlord_fortify_weight_pct,
            b.bot_warlord_alliance_permille,
        ],
    };
    Traits {
        attack_pct: (raw[0] as i64).clamp(1, 100),
        reserve_pct: (raw[1] as i64).clamp(0, 95),
        margin_permille: (raw[2] as i64).max(1000),
        overkill_pct: raw[3] as i64,
        enemy_weight_pct: raw[4] as i64,
        fortify_weight_pct: raw[5] as i64,
        alliance_permille: (raw[6] as i64).min(1000),
    }
}

/// Stall level: completed `bot_stall_ticks` periods since the last capture, capped.
fn stall_level(game: &Game) -> i64 {
    let b = &game.balance;
    let since = game.tick.saturating_sub(game.last_capture_tick);
    let level = since / b.bot_stall_ticks.max(1);
    level.min(b.bot_stall_max_level as u64) as i64
}

/// Applies difficulty noise of plus or minus `noise` permille to a positive utility.
fn noisy(u: i64, noise: i64, rng: &mut Rng) -> i64 {
    if u <= 0 || noise <= 0 {
        return u;
    }
    let roll = rng.range_u64(0, (noise * 2) as u64) as i64;
    u * (1000 - noise + roll) / 1000
}

/// The result of one bot decision.
struct Decision {
    command: Option<Command>,
    focus: Option<PlayerId>,
    focus_since: Tick,
    next_diplomacy: Tick,
    considered: Vec<(PlayerId, Tick)>,
}

/// Per-decision survey of marches and borders.
struct Survey {
    /// My troops in flight, per target region.
    my_inflight: Vec<i64>,
    /// Reinforcements in flight by the target's own owner, per region.
    owner_inflight: Vec<i64>,
    /// Third-party attacks in flight on regions that are not mine, per region.
    third_inflight: Vec<i64>,
    /// Hostile troops in flight toward my regions, per region.
    hostile_incoming: Vec<i64>,
    /// Players currently at war with someone other than me.
    engaged: Vec<bool>,
    /// Players owning a region adjacent to my territory.
    borders_player: Vec<bool>,
    /// Whether any neutral region borders my territory.
    has_neutral_border: bool,
}

fn survey(game: &Game, me: PlayerId, owned: &[RegionId]) -> Survey {
    let n = game.regions.len().min(game.map.len());
    let pc = game.players.len();
    let mut s = Survey {
        my_inflight: vec![0; n],
        owner_inflight: vec![0; n],
        third_inflight: vec![0; n],
        hostile_incoming: vec![0; n],
        engaged: vec![false; pc],
        borders_player: vec![false; pc],
        has_neutral_border: false,
    };
    for m in &game.marches {
        let t = m.to as usize;
        if t >= n {
            continue;
        }
        let target_owner = game.regions[t].owner;
        if m.owner == me {
            s.my_inflight[t] += m.troops;
            continue;
        }
        if target_owner == Some(m.owner) {
            s.owner_inflight[t] += m.troops;
            continue;
        }
        if target_owner == Some(me) {
            if !game.are_allied(me, m.owner) {
                s.hostile_incoming[t] += m.troops;
            }
            continue;
        }
        s.third_inflight[t] += m.troops;
        if let Some(o) = target_owner {
            if (m.owner as usize) < pc {
                s.engaged[m.owner as usize] = true;
            }
            if (o as usize) < pc {
                s.engaged[o as usize] = true;
            }
        }
    }
    for &r in owned {
        for &(nb, _, _) in game.map.neighbors(r) {
            let ni = nb as usize;
            if ni >= n {
                continue;
            }
            match game.regions[ni].owner {
                Some(o) => {
                    if o != me && (o as usize) < pc {
                        s.borders_player[o as usize] = true;
                    }
                }
                None => s.has_neutral_border = true,
            }
        }
    }
    s
}

impl Game {
    /// Step 3 of the tick: every bot whose timer is due decides and acts, in
    /// ascending player id. Commands go through the normal validation path.
    pub(crate) fn run_bots(&mut self, events: &mut Vec<GameEvent>) {
        let tick = self.tick;
        for i in 0..self.brains.len() {
            let pid = self.brains[i].player;
            if !self.is_alive(pid) || tick < self.brains[i].next_decision {
                continue;
            }
            let brain = self.brains[i].clone();
            let mut rng = self.rng.clone();
            let decision = decide(&*self, &brain, &mut rng);
            self.rng = rng;
            if let Some(cmd) = decision.command {
                self.apply_command(pid, cmd, events);
            }
            let (lo, hi) = reaction_range(&self.balance, brain.difficulty);
            let delay = self.rng.range_u64(lo, hi).max(1);
            let slot = &mut self.brains[i];
            slot.focus = decision.focus;
            slot.focus_since = decision.focus_since;
            slot.next_diplomacy = decision.next_diplomacy;
            slot.considered = decision.considered;
            slot.next_decision = tick + delay;
        }
    }
}

/// One bot decision. Pure with respect to the game; randomness comes from `rng`.
fn decide(game: &Game, brain: &BotBrain, rng: &mut Rng) -> Decision {
    let me = brain.player;
    let tick = game.tick;
    let b = &game.balance;
    let personality = brain.personality;
    let difficulty = brain.difficulty;
    let easy = difficulty == Difficulty::Easy;
    let hard = difficulty == Difficulty::Hard;
    let noise = noise_permille(b, difficulty);

    let mut decision = Decision {
        command: None,
        focus: brain.focus,
        focus_since: brain.focus_since,
        next_diplomacy: brain.next_diplomacy,
        considered: brain
            .considered
            .iter()
            .copied()
            .filter(|&(_, expires)| expires > tick)
            .collect(),
    };

    // Personality, escalated by the anti-stall level.
    let stall = stall_level(game);
    let mut tr = traits(b, personality);
    if stall > 0 {
        let floor = tr.margin_permille.min(1050);
        tr.margin_permille =
            (tr.margin_permille - stall * b.bot_stall_margin_cut_permille as i64).max(floor);
        tr.attack_pct = (tr.attack_pct + stall * b.bot_stall_attack_pct_step as i64).min(100);
        tr.reserve_pct = (tr.reserve_pct - stall * 10).max(0);
        tr.enemy_weight_pct += stall * 25;
        tr.alliance_permille /= stall + 1;
    }

    let n = game.regions.len().min(game.map.len());
    let pc = game.players.len();
    let pool = game.pool(me);
    let cap = game.pool_cap(me);
    let my_score = game.score(me);
    let alive_count = game.alive_players().len();
    let owned = game.owned_regions(me);
    let attackable = game.attackable_regions(me);
    let sv = survey(game, me, &owned);

    // ---- Warlord focus: fight the weakest bordering enemy, pivot periodically.
    if personality == Personality::Warlord {
        let enemies: Vec<PlayerId> = (0..pc)
            .filter(|&x| {
                sv.borders_player[x] && game.players[x].alive && !game.are_allied(me, x as PlayerId)
            })
            .map(|x| x as PlayerId)
            .collect();
        let mut focus = decision.focus;
        if let Some(f) = focus {
            if !enemies.contains(&f) {
                focus = None;
            }
        }
        let pivot_due =
            focus.is_some() && tick.saturating_sub(decision.focus_since) >= b.bot_warlord_pivot_ticks;
        if focus.is_none() || pivot_due {
            let exclude = if pivot_due { focus } else { None };
            let mut best: Option<(u64, PlayerId)> = None;
            for &e in &enemies {
                if Some(e) == exclude {
                    continue;
                }
                let s = game.score(e);
                let better = match best {
                    None => true,
                    Some((bs, _)) => s < bs,
                };
                if better {
                    best = Some((s, e));
                }
            }
            match best {
                Some((_, e)) => {
                    focus = Some(e);
                    decision.focus_since = tick;
                }
                None => {
                    if pivot_due {
                        // Only one front: keep it and restart the pivot timer.
                        decision.focus_since = tick;
                    }
                }
            }
        }
        decision.focus = focus;
    } else {
        decision.focus = None;
    }

    // ---- Diplomacy.
    if game.config.alliances {
        // Answer proposals addressed to me (each judged exactly once).
        for pr in &game.proposals {
            if pr.to != me {
                continue;
            }
            let key = (pr.from, pr.expires);
            if decision.considered.contains(&key) {
                continue;
            }
            decision.considered.push(key);
            let chance = accept_chance(
                game,
                personality,
                &tr,
                me,
                pr.from,
                my_score,
                decision.focus,
                &sv,
                alive_count,
                stall,
            );
            if rng.chance_permille(chance) {
                let cmd = Command::ProposeAlliance { with: pr.from };
                if game.validate_command(me, &cmd).is_ok() {
                    decision.command = Some(cmd);
                    return decision;
                }
            }
        }
        // Periodic diplomacy: break or propose.
        if tick >= decision.next_diplomacy {
            decision.next_diplomacy = tick + b.bot_diplomacy_interval_ticks.max(1);
            let ctx = DiplomacyCtx {
                personality,
                tr: &tr,
                me,
                my_score,
                focus: decision.focus,
                sv: &sv,
                alive_count,
                stall,
                has_targets: !attackable.is_empty(),
            };
            if let Some(cmd) = diplomacy_move(game, &ctx, rng) {
                decision.command = Some(cmd);
                return decision;
            }
        }
    }

    // ---- Military: score every candidate and keep the best.
    let reserve = pool * tr.reserve_pct / 100;
    let avail = (pool - reserve).max(0);
    let cap_send = (pool * tr.attack_pct / 100).max(0);
    let mut best_cmd: Option<Command> = None;
    let mut best_u: i64 = 0;

    // Attacks.
    for &r in &attackable {
        let ri = r as usize;
        if ri >= n {
            continue;
        }
        let st = game.regions[ri];
        let value = game.map.regions[ri].value as i64;
        let owner = st.owner;
        let eta = match game.attack_route(me, r) {
            Some((_, e)) => e as i64,
            None => continue,
        };
        let send: i64;
        if easy {
            // Easy: fixed fraction, compared against the raw garrison only.
            if sv.my_inflight[ri] > 0 {
                continue;
            }
            send = cap_send.min(avail);
            if send <= st.garrison.max(0) {
                continue;
            }
        } else {
            let def = game.effective_defense(r);
            let bonus = game.defense_bonus_permille(r) as i64;
            // Estimated defense on arrival: a little regeneration on the way.
            let mut est = def + def / 20 + 2;
            if hard {
                est += sv.owner_inflight[ri] * bonus / 1000;
                let cut = (sv.third_inflight[ri] / 2).min(est * 3 / 4);
                est -= cut;
            } else if sv.my_inflight[ri] > 0 {
                continue;
            }
            let mut required = est * tr.margin_permille / 1000 + 1;
            if hard {
                // Stack with troops already on the way so the waves land together.
                required -= sv.my_inflight[ri];
                if required <= 0 {
                    continue;
                }
            }
            if required > avail || required > cap_send.max(1) {
                continue;
            }
            let wanted = required + required * tr.overkill_pct / 100 + value * 5;
            let mut sized = wanted.min(avail).min(cap_send).max(required);
            // A pool near its cap stops growing: spend the surplus in force.
            if pool * 100 >= cap * b.bot_surplus_pool_pct as i64 {
                sized = sized.max(cap_send.min(avail));
            }
            send = sized;
        }
        if send < 1 || send > pool {
            continue;
        }

        let mut gain = value * 1000;
        let mut weight: i64 = 100;
        if let Some(o) = owner {
            let oi = o as usize;
            weight = tr.enemy_weight_pct;
            if game.players[oi].capital == Some(r) {
                gain += value * 500;
            }
            if game.region_count(o) <= 1 {
                gain += 4000;
            }
            match personality {
                Personality::Warlord => {
                    if decision.focus == Some(o) {
                        weight *= 2;
                    } else {
                        weight /= 2;
                    }
                }
                Personality::Opportunist => {
                    if oi < sv.engaged.len() && sv.engaged[oi] {
                        weight = weight * 180 / 100;
                    }
                    if game.pool(o) * 2 < pool {
                        weight = weight * 150 / 100;
                    }
                }
                Personality::Aggressor | Personality::Turtle => {}
            }
            if hard && st.garrison < game.garrison_cap(r) / 3 {
                weight = weight * 130 / 100;
            }
        } else if personality == Personality::Turtle {
            weight = 120;
        }
        let mut u = gain * weight / 100 * 1000 / (send + 200);
        u = u * 100 / (100 + eta / 5);
        u = noisy(u, noise, rng);
        if u > best_u {
            best_u = u;
            best_cmd = Some(Command::Attack {
                target: r,
                amount: Amount::Flat(send as u64),
            });
        }
    }

    if !easy {
        let my_capital = game.players[me as usize].capital;

        // Emergency fortify against incoming hostile marches.
        for &r in &owned {
            let ri = r as usize;
            if ri >= n {
                continue;
            }
            let incoming = sv.hostile_incoming[ri];
            if incoming <= 0 {
                continue;
            }
            let def = game.effective_defense(r);
            let threatened = if hard {
                incoming * 10 > def * 9
            } else {
                incoming > def
            };
            if !threatened {
                continue;
            }
            let st = game.regions[ri];
            let is_capital = my_capital == Some(r);
            let mut bonus_f = b.defense_base_permille as i64 + b.fortified_bonus_permille as i64;
            if is_capital {
                bonus_f += b.capital_bonus_permille as i64;
            }
            let bonus_f = bonus_f.max(1);
            let needed_garrison = incoming * 1000 / bonus_f + 1 + incoming / 10;
            let mut troops = (needed_garrison - st.garrison).max(1);
            if troops > pool {
                if hard {
                    continue;
                }
                troops = pool;
            }
            if troops < 1 {
                continue;
            }
            let value = game.map.regions[ri].value as i64;
            let mut gain = value * 1000 + st.garrison.max(0) * 2;
            if is_capital {
                gain += value * 1000 + 3000;
            }
            let mut u = gain * tr.fortify_weight_pct / 100 * 1000 / (troops + 200) * 2;
            u = noisy(u, noise, rng);
            if u > best_u {
                best_u = u;
                best_cmd = Some(Command::Fortify {
                    region: r,
                    amount: Amount::Flat(troops as u64),
                });
            }
        }

        // Turtle: invest a full pool into the weakest border garrison facing a player.
        if personality == Personality::Turtle
            && stall == 0
            && pool * 100 >= cap * b.bot_excess_fortify_pct as i64
        {
            let mut target: Option<(i64, RegionId)> = None;
            for &r in &owned {
                let faces_enemy = game.map.neighbors(r).iter().any(|&(nb, _, _)| {
                    match game.regions.get(nb as usize).and_then(|s| s.owner) {
                        Some(o) => o != me && !game.are_allied(me, o),
                        None => false,
                    }
                });
                if !faces_enemy {
                    continue;
                }
                let gcap = game.garrison_cap(r).max(1);
                let garrison = game.regions[r as usize].garrison;
                if garrison >= gcap * 3 {
                    continue;
                }
                let ratio = garrison * 1000 / gcap;
                let better = match target {
                    None => true,
                    Some((br, _)) => ratio < br,
                };
                if better {
                    target = Some((ratio, r));
                }
            }
            if let Some((_, r)) = target {
                let troops = pool * 20 / 100;
                if troops >= 1 {
                    let u = noisy(1500, noise, rng);
                    if u > best_u {
                        best_u = u;
                        best_cmd = Some(Command::Fortify {
                            region: r,
                            amount: Amount::Flat(troops as u64),
                        });
                    }
                }
            }
        }

        // Recall surplus garrison from safe interior regions when the pool is low.
        if pool * 100 < cap * b.bot_recall_below_cap_pct as i64 {
            for &r in &owned {
                let ri = r as usize;
                if ri >= n {
                    continue;
                }
                let interior = game.map.neighbors(r).iter().all(|&(nb, _, _)| {
                    match game.regions.get(nb as usize).and_then(|s| s.owner) {
                        Some(o) => o == me || game.are_allied(me, o),
                        None => false,
                    }
                });
                if !interior {
                    continue;
                }
                let value = game.map.regions[ri].value as i64;
                let keep = 20 + value * 5;
                let garrison = game.regions[ri].garrison;
                if garrison <= keep * 2 {
                    continue;
                }
                let amount = garrison - keep;
                let mut u = amount * 4;
                if personality == Personality::Aggressor || personality == Personality::Warlord {
                    u = u * 3 / 2;
                }
                u = noisy(u, noise, rng);
                if u > best_u {
                    best_u = u;
                    best_cmd = Some(Command::Recall {
                        region: r,
                        amount: Amount::Flat(amount as u64),
                    });
                }
            }
        }
    }

    if let Some(cmd) = best_cmd {
        if best_u > 0 && game.validate_command(me, &cmd).is_ok() {
            decision.command = Some(cmd);
        }
    }
    decision
}

/// Probability (permille) that a bot accepts an alliance proposal from `from`.
#[allow(clippy::too_many_arguments)]
fn accept_chance(
    game: &Game,
    personality: Personality,
    tr: &Traits,
    me: PlayerId,
    from: PlayerId,
    my_score: u64,
    focus: Option<PlayerId>,
    sv: &Survey,
    alive_count: usize,
    stall: i64,
) -> u32 {
    // An alliance between the last two players would just end the match.
    if alive_count <= 2 || !game.is_alive(from) || from == me {
        return 0;
    }
    if personality == Personality::Warlord && focus == Some(from) {
        return 0;
    }
    let mut c = tr.alliance_permille;
    let theirs = game.score(from);
    if theirs > my_score {
        c += 200;
    } else if theirs.saturating_mul(2) < my_score {
        c -= 150;
    }
    let borders = sv
        .borders_player
        .get(from as usize)
        .copied()
        .unwrap_or(false);
    match personality {
        Personality::Warlord => {
            if borders {
                c += 300;
            }
        }
        Personality::Turtle => {
            if borders {
                c += 150;
            }
        }
        Personality::Opportunist => {
            if theirs > my_score {
                c += 100;
            }
        }
        Personality::Aggressor => {}
    }
    if stall >= 2 {
        c /= 2;
    }
    c.clamp(0, 1000) as u32
}

/// Inputs for periodic diplomacy.
struct DiplomacyCtx<'a> {
    personality: Personality,
    tr: &'a Traits,
    me: PlayerId,
    my_score: u64,
    focus: Option<PlayerId>,
    sv: &'a Survey,
    alive_count: usize,
    stall: i64,
    has_targets: bool,
}

/// Considers breaking one alliance, then proposing one. At most one command.
fn diplomacy_move(game: &Game, ctx: &DiplomacyCtx<'_>, rng: &mut Rng) -> Option<Command> {
    let me = ctx.me;
    let allies = game.allies_of(me);

    // Break: Opportunists turn on weakened allies, Aggressors on weaker ones,
    // Warlords on an ally once they run out of other fronts; anyone boxed in
    // by allies, or stalled, eventually breaks.
    for &a in &allies {
        let borders = ctx.sv.borders_player.get(a as usize).copied().unwrap_or(false);
        let theirs = game.score(a);
        let mut c: i64 = 0;
        match ctx.personality {
            Personality::Opportunist => {
                if borders && theirs.saturating_mul(2) < ctx.my_score {
                    c = 600;
                }
            }
            Personality::Aggressor => {
                if borders && theirs < ctx.my_score {
                    c = 300;
                }
            }
            Personality::Warlord => {
                if borders && ctx.focus.is_none() && !ctx.sv.has_neutral_border {
                    c = 500;
                }
            }
            Personality::Turtle => {}
        }
        if borders && !ctx.has_targets {
            c = c.max(400);
        }
        if ctx.stall >= 2 {
            c += ctx.stall * 150;
        }
        if c > 0 && rng.chance_permille(c.min(1000) as u32) {
            let cmd = Command::BreakAlliance { with: a };
            if game.validate_command(me, &cmd).is_ok() {
                return Some(cmd);
            }
        }
    }

    // Propose: Turtles to their strongest neighbor, Warlords to a neighbor that
    // is not their focus, Opportunists and Aggressors only to stronger neighbors.
    let max_allies = game.balance.max_allies as usize;
    if allies.len() >= max_allies || ctx.alive_count <= 2 || ctx.stall >= 2 {
        return None;
    }
    let mut best: Option<(u64, PlayerId)> = None;
    for x in 0..game.players.len() {
        let x = x as PlayerId;
        if x == me
            || !ctx.sv.borders_player.get(x as usize).copied().unwrap_or(false)
            || !game.is_alive(x)
            || game.are_allied(me, x)
            || game.pending_proposal(me, x)
            || game.has_truce(me, x)
        {
            continue;
        }
        let s = game.score(x);
        let eligible = match ctx.personality {
            Personality::Turtle => true,
            Personality::Warlord => ctx.focus != Some(x),
            Personality::Opportunist | Personality::Aggressor => s > ctx.my_score,
        };
        if !eligible {
            continue;
        }
        let better = match best {
            None => true,
            Some((bs, _)) => s > bs,
        };
        if better {
            best = Some((s, x));
        }
    }
    if let Some((_, x)) = best {
        let chance = ctx.tr.alliance_permille.clamp(0, 1000) as u32;
        if rng.chance_permille(chance) {
            let cmd = Command::ProposeAlliance { with: x };
            if game.validate_command(me, &cmd).is_ok() {
                return Some(cmd);
            }
        }
    }
    None
}
