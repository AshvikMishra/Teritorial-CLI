//! Match configuration: preset, timer, growth scaling, bots, difficulty, start,
//! win condition, seed and options.

use serde::{Deserialize, Serialize};

use crate::map::WorldMap;
use crate::RegionId;

/// Match length preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    /// 8 minutes, double growth, 8 bots.
    Blitz,
    /// 25 minutes, normal growth, 12 bots.
    Standard,
    /// 75 minutes, 60% growth, 20 bots.
    Epic,
    /// Custom timer (starts from Standard).
    Custom,
}

impl Preset {
    /// All presets in menu order.
    pub const ALL: [Preset; 4] = [Preset::Blitz, Preset::Standard, Preset::Epic, Preset::Custom];

    /// Lowercase display name.
    pub fn name(self) -> &'static str {
        match self {
            Preset::Blitz => "blitz",
            Preset::Standard => "standard",
            Preset::Epic => "epic",
            Preset::Custom => "custom",
        }
    }

    /// Parses a preset name (case-insensitive).
    pub fn from_name(s: &str) -> Option<Preset> {
        match s.trim().to_ascii_lowercase().as_str() {
            "blitz" => Some(Preset::Blitz),
            "standard" => Some(Preset::Standard),
            "epic" => Some(Preset::Epic),
            "custom" => Some(Preset::Custom),
            _ => None,
        }
    }
}

/// Bot difficulty. `Mixed` is resolved per bot at `Game::new`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Difficulty {
    /// Slow reactions, simple expansion, poor defense.
    Easy,
    /// Evaluates odds before attacking.
    Normal,
    /// Fast reactions, coordinates attacks, exploits openings.
    Hard,
    /// A mix of the three.
    Mixed,
}

impl Difficulty {
    /// All difficulties in menu order.
    pub const ALL: [Difficulty; 4] = [
        Difficulty::Easy,
        Difficulty::Normal,
        Difficulty::Hard,
        Difficulty::Mixed,
    ];

    /// Lowercase display name.
    pub fn name(self) -> &'static str {
        match self {
            Difficulty::Easy => "easy",
            Difficulty::Normal => "normal",
            Difficulty::Hard => "hard",
            Difficulty::Mixed => "mixed",
        }
    }

    /// Parses a difficulty name (case-insensitive).
    pub fn from_name(s: &str) -> Option<Difficulty> {
        match s.trim().to_ascii_lowercase().as_str() {
            "easy" => Some(Difficulty::Easy),
            "normal" => Some(Difficulty::Normal),
            "hard" => Some(Difficulty::Hard),
            "mixed" => Some(Difficulty::Mixed),
            _ => None,
        }
    }
}

/// How the match is won.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WinCondition {
    /// Control `pct` percent of total world value (also ends on elimination).
    Domination {
        /// Required share of world value, 10..=100.
        pct: u8,
    },
    /// Last player (or allied clique) standing; no time limit.
    Elimination,
    /// Highest score when the timer ends.
    Timed,
}

impl WinCondition {
    /// Short display name, e.g. `domination 60%`.
    pub fn label(self) -> String {
        match self {
            WinCondition::Domination { pct } => format!("domination {}%", pct),
            WinCondition::Elimination => "elimination".to_string(),
            WinCondition::Timed => "timed".to_string(),
        }
    }
}

/// Where the human starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartChoice {
    /// A random, well-spaced region.
    Random,
    /// A specific region.
    Region(RegionId),
}

/// Full configuration of one match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MatchConfig {
    /// The preset this config came from.
    pub preset: Preset,
    /// Configured timer in seconds. Ignored for Domination/Elimination; also sets preset pacing.
    /// `None` = no timer (Domination/Elimination only).
    pub time_limit_secs: Option<u32>,
    /// Pool growth scaling, 100 = normal.
    pub growth_scale_pct: u32,
    /// Number of bots, 3..=40.
    pub bots: u8,
    /// Bot difficulty.
    pub difficulty: Difficulty,
    /// Human start region.
    pub start: StartChoice,
    /// Win condition.
    pub win: WinCondition,
    /// RNG seed. Same seed + same commands = same match.
    pub seed: u64,
    /// Whether alliances are enabled.
    pub alliances: bool,
    /// Display name of the human player.
    pub human_name: String,
    /// When true, player 0 is also bot-driven (headless `--simulate`, bot-only matches).
    pub spectate: bool,
}

/// Default seed used when no seed is chosen.
pub const DEFAULT_SEED: u64 = 0x5EED_7E44_1709_2026;

impl MatchConfig {
    /// A config for the given preset.
    ///
    /// Blitz: 8 min, growth 200%, 8 bots. Standard: 25 min, 100%, 12 bots.
    /// Epic: 75 min, 60%, 20 bots. Custom: Standard values, preset `Custom`.
    pub fn preset(p: Preset) -> MatchConfig {
        let (minutes, growth, bots) = match p {
            Preset::Blitz => (8u32, 200u32, 8u8),
            Preset::Standard => (25, 100, 12),
            Preset::Epic => (75, 60, 20),
            Preset::Custom => (25, 100, 12),
        };
        MatchConfig {
            preset: p,
            time_limit_secs: Some(minutes * 60),
            growth_scale_pct: growth,
            bots,
            difficulty: Difficulty::Normal,
            start: StartChoice::Random,
            win: WinCondition::Domination { pct: 60 },
            seed: DEFAULT_SEED,
            alliances: true,
            human_name: "You".to_string(),
            spectate: false,
        }
    }

    /// Sets a custom timer of `minutes` (minimum 1) and adjusts `growth_scale_pct`
    /// to fit: shorter matches grow faster, longer ones slower. The curve passes
    /// through the presets (8 min = 200%, 25 min = 100%, 75 min = 60%).
    /// Sets `preset` to `Custom`.
    pub fn with_custom_minutes(self, minutes: u32) -> MatchConfig {
        let minutes = minutes.max(1);
        let mut cfg = self;
        cfg.preset = Preset::Custom;
        cfg.time_limit_secs = Some(minutes.saturating_mul(60));
        cfg.growth_scale_pct = growth_for_minutes(minutes);
        cfg
    }

    /// Checks the config against a map. Returns a human readable reason on failure.
    pub fn validate(&self, map: &WorldMap) -> Result<(), String> {
        if self.bots < 3 || self.bots > crate::MAX_BOTS {
            return Err(format!(
                "bots must be between 3 and {} (got {})",
                crate::MAX_BOTS,
                self.bots
            ));
        }
        if let WinCondition::Domination { pct } = self.win {
            if !(10..=100).contains(&pct) {
                return Err(format!(
                    "domination percent must be between 10 and 100 (got {})",
                    pct
                ));
            }
        }
        if let StartChoice::Region(r) = self.start {
            if r as usize >= map.len() {
                return Err(format!("start region {} does not exist", r));
            }
        }
        if map.len() < self.bots as usize + 1 {
            return Err(format!(
                "the map has {} regions, too few for {} players",
                map.len(),
                self.bots as usize + 1
            ));
        }
        if let Some(t) = self.time_limit_secs {
            if t == 0 {
                return Err("the time limit must be at least 1 second".to_string());
            }
        }
        if self.win == WinCondition::Timed && self.time_limit_secs.is_none() {
            return Err("a timed match needs a time limit".to_string());
        }
        if self.growth_scale_pct < 10 || self.growth_scale_pct > 1000 {
            return Err(format!(
                "growth scale must be between 10% and 1000% (got {}%)",
                self.growth_scale_pct
            ));
        }
        let name = self.human_name.trim();
        if name.is_empty() {
            return Err("the player name must not be empty".to_string());
        }
        if name.chars().count() > 24 {
            return Err("the player name must be at most 24 characters".to_string());
        }
        Ok(())
    }

    /// Active time limit in seconds. Only Timed matches have a deadline.
    pub fn active_time_limit_secs(&self) -> Option<u32> {
        match self.win {
            WinCondition::Timed => self.time_limit_secs,
            _ => None,
        }
    }

    /// Time limit in ticks, if any.
    pub fn time_limit_ticks(&self) -> Option<u64> {
        self.active_time_limit_secs()
            .map(|s| s as u64 * crate::TICKS_PER_SECOND)
    }
}

impl Default for MatchConfig {
    fn default() -> MatchConfig {
        MatchConfig::preset(Preset::Standard)
    }
}

/// Growth scale for a custom match length: piecewise linear through
/// (5, 250), (8, 200), (25, 100), (75, 60), (120, 50), flat beyond.
fn growth_for_minutes(minutes: u32) -> u32 {
    const POINTS: [(u32, u32); 5] = [(5, 250), (8, 200), (25, 100), (75, 60), (120, 50)];
    if minutes <= POINTS[0].0 {
        return POINTS[0].1;
    }
    let mut i = 1;
    while i < POINTS.len() {
        let (m0, g0) = POINTS[i - 1];
        let (m1, g1) = POINTS[i];
        if minutes <= m1 {
            // g decreases from g0 to g1 as minutes goes from m0 to m1.
            let span = m1 - m0;
            let drop = g0 - g1;
            return g0 - drop * (minutes - m0) / span;
        }
        i += 1;
    }
    POINTS[POINTS.len() - 1].1
}
