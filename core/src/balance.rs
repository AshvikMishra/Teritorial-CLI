//! Every tunable gameplay number, in one flat integer-only struct.
//!
//! `Balance::default()` is kept equal to `assets/balance.toml`, which lists every
//! field with a comment. The cli parses that TOML (or a `--balance` override);
//! core never touches TOML. Because the struct is `#[serde(default)]`, a balance
//! file only needs to mention the fields it changes.
//!
//! Units: troop amounts are whole troops, `*_milli` values are milli-troops,
//! `*_pct` are percents, `*_permille` are thousandths and `*_ticks` are simulation
//! ticks (10 per second).

use serde::{Deserialize, Serialize};

/// Tunable gameplay numbers. See `assets/balance.toml` for a description of each field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Balance {
    // --- Start ---
    /// Troops in each player's pool at the start.
    pub start_pool: i64,
    /// Garrison of each player's start region.
    pub start_garrison: i64,
    /// Minimum graph hops between random start regions (relaxed if impossible).
    pub start_min_hops: u32,
    /// Random starts prefer regions with at most this value.
    pub start_max_value: u32,

    // --- Troop pool economy ---
    /// Pool cap with no territory.
    pub pool_cap_base: i64,
    /// Extra pool cap per point of owned value.
    pub pool_cap_per_value: i64,
    /// Base pool growth, milli-troops per second.
    pub growth_base_milli: i64,
    /// Extra pool growth per point of owned value, milli-troops per second.
    pub growth_per_value_milli: i64,
    /// Pool interest: permille of the current pool added per second (before the logistic factor).
    pub growth_pool_permille: i64,

    // --- Garrisons ---
    /// Garrison cap of an owned region with value 0.
    pub garrison_cap_base: i64,
    /// Extra garrison cap per point of region value.
    pub garrison_cap_per_value: i64,
    /// Flat garrison regeneration per second while below the cap.
    pub garrison_regen_flat: i64,
    /// Extra regeneration per second, as permille of the gap to the cap.
    pub garrison_regen_permille: i64,
    /// Garrison above the cap decays by this permille of the excess per second.
    pub garrison_overcap_decay_permille: i64,
    /// Neutral garrison with value 0.
    pub neutral_garrison_base: i64,
    /// Extra neutral garrison per point of region value.
    pub neutral_garrison_per_value: i64,

    // --- Defense ---
    /// Defender multiplier for owned regions (1000 = 1.0x).
    pub defense_base_permille: u32,
    /// Defender multiplier for neutral regions.
    pub neutral_defense_permille: u32,
    /// Added to the multiplier when the region is its owner's capital.
    pub capital_bonus_permille: u32,
    /// Added to the multiplier while the region is fortified.
    pub fortified_bonus_permille: u32,
    /// How long a fortify keeps the fortified bonus.
    pub fortify_duration_ticks: u64,
    /// Delay before recalled troops reach the pool.
    pub recall_delay_ticks: u64,

    // --- Marching ---
    /// March time per 1000 km of edge length.
    pub march_ticks_per_1000km: u64,
    /// Minimum march time.
    pub min_march_ticks: u64,
    /// Percent attacks are clamped to this percent of the pool.
    pub max_attack_pct: u32,

    // --- Score ---
    /// Score troop bonus: one point per this many troops (pool plus garrisons).
    pub score_troops_per_point: i64,

    // --- Alliances ---
    /// Maximum simultaneous allies per player.
    pub max_allies: u32,
    /// Ticks before an unanswered alliance proposal expires.
    pub proposal_ttl_ticks: u64,
    /// Ticks after a broken alliance during which neither side may attack the other.
    pub truce_ticks: u64,

    // --- Bots: reaction and difficulty ---
    /// Easy bot reaction interval, minimum.
    pub bot_react_easy_min_ticks: u64,
    /// Easy bot reaction interval, maximum.
    pub bot_react_easy_max_ticks: u64,
    /// Normal bot reaction interval, minimum.
    pub bot_react_normal_min_ticks: u64,
    /// Normal bot reaction interval, maximum.
    pub bot_react_normal_max_ticks: u64,
    /// Hard bot reaction interval, minimum.
    pub bot_react_hard_min_ticks: u64,
    /// Hard bot reaction interval, maximum.
    pub bot_react_hard_max_ticks: u64,
    /// Random utility noise for Easy bots (permille, plus or minus).
    pub bot_noise_easy_permille: u32,
    /// Random utility noise for Normal bots.
    pub bot_noise_normal_permille: u32,
    /// Random utility noise for Hard bots.
    pub bot_noise_hard_permille: u32,

    // --- Bots: anti-stall and strategy ---
    /// Ticks without any capture before bots escalate one stall level.
    pub bot_stall_ticks: u64,
    /// Maximum stall level.
    pub bot_stall_max_level: u32,
    /// Attack margin reduction per stall level (permille).
    pub bot_stall_margin_cut_permille: u32,
    /// Attack percent increase per stall level.
    pub bot_stall_attack_pct_step: u32,
    /// Minimum ticks between a bot's diplomacy evaluations.
    pub bot_diplomacy_interval_ticks: u64,
    /// A Warlord pivots to a new focus front after this many ticks.
    pub bot_warlord_pivot_ticks: u64,
    /// Bots recall interior garrisons when their pool is below this percent of its cap.
    pub bot_recall_below_cap_pct: u32,
    /// Turtles invest pool into border garrisons when the pool is above this percent of its cap.
    pub bot_excess_fortify_pct: u32,
    /// Normal and Hard bots attack with their full attack percent when the pool is above this percent of its cap.
    pub bot_surplus_pool_pct: u32,

    // --- Bots: personality weights ---
    /// Aggressor: max percent of pool per attack (Easy bots send exactly this).
    pub bot_aggressor_attack_pct: u32,
    /// Aggressor: percent of pool kept in reserve.
    pub bot_aggressor_reserve_pct: u32,
    /// Aggressor: troops sent vs. estimated defense (permille) for Normal and Hard bots.
    pub bot_aggressor_margin_permille: u32,
    /// Aggressor: extra troops on top of the requirement, percent.
    pub bot_aggressor_overkill_pct: u32,
    /// Aggressor: utility weight of enemy-owned targets vs. neutral (100 = equal).
    pub bot_aggressor_enemy_weight_pct: u32,
    /// Aggressor: utility weight of defensive fortifies.
    pub bot_aggressor_fortify_weight_pct: u32,
    /// Aggressor: base alliance willingness (permille).
    pub bot_aggressor_alliance_permille: u32,

    /// Turtle: max percent of pool per attack.
    pub bot_turtle_attack_pct: u32,
    /// Turtle: percent of pool kept in reserve.
    pub bot_turtle_reserve_pct: u32,
    /// Turtle: troops vs. estimated defense (permille).
    pub bot_turtle_margin_permille: u32,
    /// Turtle: extra troops on top of the requirement, percent.
    pub bot_turtle_overkill_pct: u32,
    /// Turtle: utility weight of enemy-owned targets.
    pub bot_turtle_enemy_weight_pct: u32,
    /// Turtle: utility weight of defensive fortifies.
    pub bot_turtle_fortify_weight_pct: u32,
    /// Turtle: base alliance willingness (permille).
    pub bot_turtle_alliance_permille: u32,

    /// Opportunist: max percent of pool per attack.
    pub bot_opportunist_attack_pct: u32,
    /// Opportunist: percent of pool kept in reserve.
    pub bot_opportunist_reserve_pct: u32,
    /// Opportunist: troops vs. estimated defense (permille).
    pub bot_opportunist_margin_permille: u32,
    /// Opportunist: extra troops on top of the requirement, percent.
    pub bot_opportunist_overkill_pct: u32,
    /// Opportunist: utility weight of enemy-owned targets.
    pub bot_opportunist_enemy_weight_pct: u32,
    /// Opportunist: utility weight of defensive fortifies.
    pub bot_opportunist_fortify_weight_pct: u32,
    /// Opportunist: base alliance willingness (permille).
    pub bot_opportunist_alliance_permille: u32,

    /// Warlord: max percent of pool per attack.
    pub bot_warlord_attack_pct: u32,
    /// Warlord: percent of pool kept in reserve.
    pub bot_warlord_reserve_pct: u32,
    /// Warlord: troops vs. estimated defense (permille).
    pub bot_warlord_margin_permille: u32,
    /// Warlord: extra troops on top of the requirement, percent.
    pub bot_warlord_overkill_pct: u32,
    /// Warlord: utility weight of enemy-owned targets.
    pub bot_warlord_enemy_weight_pct: u32,
    /// Warlord: utility weight of defensive fortifies.
    pub bot_warlord_fortify_weight_pct: u32,
    /// Warlord: base alliance willingness (permille).
    pub bot_warlord_alliance_permille: u32,
}

impl Default for Balance {
    fn default() -> Balance {
        Balance {
            start_pool: 1200,
            start_garrison: 150,
            start_min_hops: 3,
            start_max_value: 12,

            pool_cap_base: 3000,
            pool_cap_per_value: 500,
            growth_base_milli: 6000,
            growth_per_value_milli: 500,
            growth_pool_permille: 10,

            garrison_cap_base: 50,
            garrison_cap_per_value: 25,
            garrison_regen_flat: 1,
            garrison_regen_permille: 15,
            garrison_overcap_decay_permille: 3,
            neutral_garrison_base: 30,
            neutral_garrison_per_value: 20,

            defense_base_permille: 1000,
            neutral_defense_permille: 1200,
            capital_bonus_permille: 500,
            fortified_bonus_permille: 500,
            fortify_duration_ticks: 300,
            recall_delay_ticks: 30,

            march_ticks_per_1000km: 60,
            min_march_ticks: 20,
            max_attack_pct: 100,

            score_troops_per_point: 100,

            max_allies: 3,
            proposal_ttl_ticks: 300,
            truce_ticks: 300,

            bot_react_easy_min_ticks: 60,
            bot_react_easy_max_ticks: 100,
            bot_react_normal_min_ticks: 30,
            bot_react_normal_max_ticks: 50,
            bot_react_hard_min_ticks: 10,
            bot_react_hard_max_ticks: 20,
            bot_noise_easy_permille: 400,
            bot_noise_normal_permille: 200,
            bot_noise_hard_permille: 60,

            bot_stall_ticks: 1200,
            bot_stall_max_level: 5,
            bot_stall_margin_cut_permille: 60,
            bot_stall_attack_pct_step: 10,
            bot_diplomacy_interval_ticks: 300,
            bot_warlord_pivot_ticks: 1800,
            bot_recall_below_cap_pct: 40,
            bot_excess_fortify_pct: 70,
            bot_surplus_pool_pct: 60,

            bot_aggressor_attack_pct: 60,
            bot_aggressor_reserve_pct: 5,
            bot_aggressor_margin_permille: 1150,
            bot_aggressor_overkill_pct: 60,
            bot_aggressor_enemy_weight_pct: 160,
            bot_aggressor_fortify_weight_pct: 40,
            bot_aggressor_alliance_permille: 60,

            bot_turtle_attack_pct: 25,
            bot_turtle_reserve_pct: 40,
            bot_turtle_margin_permille: 1600,
            bot_turtle_overkill_pct: 25,
            bot_turtle_enemy_weight_pct: 60,
            bot_turtle_fortify_weight_pct: 200,
            bot_turtle_alliance_permille: 700,

            bot_opportunist_attack_pct: 40,
            bot_opportunist_reserve_pct: 20,
            bot_opportunist_margin_permille: 1250,
            bot_opportunist_overkill_pct: 30,
            bot_opportunist_enemy_weight_pct: 120,
            bot_opportunist_fortify_weight_pct: 100,
            bot_opportunist_alliance_permille: 300,

            bot_warlord_attack_pct: 50,
            bot_warlord_reserve_pct: 15,
            bot_warlord_margin_permille: 1250,
            bot_warlord_overkill_pct: 50,
            bot_warlord_enemy_weight_pct: 130,
            bot_warlord_fortify_weight_pct: 70,
            bot_warlord_alliance_permille: 400,
        }
    }
}

impl Balance {
    /// Sanity-checks values that would break the simulation (zero divisors,
    /// inverted ranges, negative amounts). Returns a human readable reason.
    pub fn validate(&self) -> Result<(), String> {
        if self.start_pool < 0 || self.start_garrison < 1 {
            return Err("start_pool must be >= 0 and start_garrison >= 1".to_string());
        }
        if self.pool_cap_base < 1 || self.pool_cap_per_value < 0 {
            return Err("pool_cap_base must be >= 1 and pool_cap_per_value >= 0".to_string());
        }
        if self.growth_base_milli < 0
            || self.growth_per_value_milli < 0
            || self.growth_pool_permille < 0
        {
            return Err("growth values must not be negative".to_string());
        }
        if self.garrison_cap_base < 1
            || self.garrison_cap_per_value < 0
            || self.garrison_regen_flat < 0
            || self.garrison_regen_permille < 0
            || self.garrison_overcap_decay_permille < 0
            || self.neutral_garrison_base < 0
            || self.neutral_garrison_per_value < 0
        {
            return Err("garrison values must not be negative (garrison_cap_base >= 1)".to_string());
        }
        if self.defense_base_permille == 0 || self.neutral_defense_permille == 0 {
            return Err("defense permille values must be > 0".to_string());
        }
        if self.max_attack_pct == 0 || self.max_attack_pct > 100 {
            return Err("max_attack_pct must be 1..=100".to_string());
        }
        if self.score_troops_per_point < 1 {
            return Err("score_troops_per_point must be >= 1".to_string());
        }
        if self.bot_react_easy_min_ticks > self.bot_react_easy_max_ticks
            || self.bot_react_normal_min_ticks > self.bot_react_normal_max_ticks
            || self.bot_react_hard_min_ticks > self.bot_react_hard_max_ticks
        {
            return Err("bot reaction minimums must not exceed maximums".to_string());
        }
        if self.bot_stall_ticks == 0 {
            return Err("bot_stall_ticks must be > 0".to_string());
        }
        Ok(())
    }

    /// Initial garrison of a neutral region of the given value.
    pub fn neutral_garrison(&self, value: u32) -> i64 {
        self.neutral_garrison_base + self.neutral_garrison_per_value * value as i64
    }

    /// Garrison regeneration cap of an owned region of the given value.
    pub fn garrison_cap(&self, value: u32) -> i64 {
        self.garrison_cap_base + self.garrison_cap_per_value * value as i64
    }
}
