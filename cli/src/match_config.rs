//! Building a [`MatchConfig`] from a base config plus the command-line flags,
//! resolving `--start`, and choosing random seeds.
//!
//! Rules, applied in this order on top of the base config:
//!
//! 1. `--preset P` replaces the timer, growth scale and bot count with P's values.
//! 2. `--minutes N` sets a custom timer (and growth scale) of N minutes; 0 removes the timer.
//! 3. `--bots`, `--difficulty`, `--seed`, `--no-alliances` set their field.
//! 4. `--win` sets the win condition. `--domination-pct` sets the domination
//!    percent and, without `--win`, also switches the win condition to domination.
//! 5. `--start` sets a fixed start region; `--bot-only-watch` sets `spectate`
//!    and names player 0 `Bot-0` (it is a bot in that mode).

use std::time::{SystemTime, UNIX_EPOCH};

use territorial_core::config::DEFAULT_SEED;
use territorial_core::{MatchConfig, Preset, RegionId, StartChoice, WinCondition, WorldMap};

use crate::args::{Cli, WinArg};
use crate::text::{edit_distance, normalize};
use crate::Failure;

/// Domination percent used when switching to domination without a percent.
const DEFAULT_DOMINATION_PCT: u8 = 60;
/// Name of player 0 when it is a bot (`--bot-only-watch`, `--simulate`).
pub const BOT_ONLY_NAME: &str = "Bot-0";

/// The base config for interactive starts: the last match from the setup
/// screen, or the standard preset. Per-match leftovers (spectating and its
/// `Bot-0` name, a fixed start region) are reset.
pub fn interactive_base(last: Option<&MatchConfig>) -> MatchConfig {
    match last {
        Some(cfg) => {
            let mut base = cfg.clone();
            if base.spectate {
                base.human_name = MatchConfig::default().human_name;
            }
            base.spectate = false;
            base.start = StartChoice::Random;
            base
        }
        None => MatchConfig::default(),
    }
}

/// Applies the flags to `base` (see the module docs). Does not validate.
pub fn apply_flags(base: MatchConfig, cli: &Cli, map: &WorldMap) -> Result<MatchConfig, Failure> {
    let mut cfg = base;

    if let Some(p) = cli.preset {
        let preset = p.to_core();
        let fresh = MatchConfig::preset(preset);
        cfg.preset = preset;
        cfg.time_limit_secs = fresh.time_limit_secs;
        cfg.growth_scale_pct = fresh.growth_scale_pct;
        cfg.bots = fresh.bots;
    }

    if let Some(minutes) = cli.minutes {
        if minutes == 0 {
            cfg.preset = Preset::Custom;
            cfg.time_limit_secs = None;
        } else {
            cfg = cfg.with_custom_minutes(minutes);
        }
    }

    if let Some(bots) = cli.bots {
        cfg.bots = bots;
    }
    if let Some(d) = cli.difficulty {
        cfg.difficulty = d.to_core();
    }

    let current_pct: Option<u8> = match cfg.win {
        WinCondition::Domination { pct } => Some(pct),
        _ => None,
    };
    match cli.win {
        Some(WinArg::Domination) => {
            let pct = match cli.domination_pct {
                Some(p) => p,
                None => current_pct.unwrap_or(DEFAULT_DOMINATION_PCT),
            };
            cfg.win = WinCondition::Domination { pct };
        }
        Some(WinArg::Elimination) => {
            if cli.domination_pct.is_some() {
                return Err(Failure::usage(
                    "--domination-pct only applies to --win domination".to_string(),
                ));
            }
            cfg.win = WinCondition::Elimination;
        }
        Some(WinArg::Timed) => {
            if cli.domination_pct.is_some() {
                return Err(Failure::usage(
                    "--domination-pct only applies to --win domination".to_string(),
                ));
            }
            cfg.win = WinCondition::Timed;
        }
        None => {
            if let Some(pct) = cli.domination_pct {
                cfg.win = WinCondition::Domination { pct };
            }
        }
    }

    if let Some(seed) = cli.seed {
        cfg.seed = seed;
    }
    if let Some(start) = &cli.start {
        let region = resolve_region(map, start).map_err(Failure::usage)?;
        cfg.start = StartChoice::Region(region);
    }
    if cli.no_alliances {
        cfg.alliances = false;
    }
    if cli.bot_only_watch {
        cfg.spectate = true;
        cfg.human_name = BOT_ONLY_NAME.to_string();
    }
    Ok(cfg)
}

/// Resolves a `--start` argument: exact code, then exact name (ignoring case,
/// dashes and punctuation), then a unique code or name prefix. On failure the
/// error lists close matches.
pub fn resolve_region(map: &WorldMap, input: &str) -> Result<RegionId, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("--start needs a region code (fr, us-w) or a name (France)".to_string());
    }
    if let Some(id) = map.find_code(raw) {
        return Ok(id);
    }
    let key = normalize(raw);
    let code_key = raw.to_ascii_lowercase();
    for info in &map.regions {
        if normalize(&info.name) == key {
            return Ok(info.id);
        }
    }

    let mut candidates: Vec<RegionId> = Vec::new();
    for info in &map.regions {
        if info.code.starts_with(code_key.as_str()) || normalize(&info.name).starts_with(key.as_str())
        {
            candidates.push(info.id);
        }
    }
    if candidates.len() == 1 {
        return Ok(candidates[0]);
    }

    if candidates.is_empty() {
        let mut scored: Vec<(usize, RegionId)> = Vec::new();
        for info in &map.regions {
            let name = normalize(&info.name);
            let contains = !key.is_empty() && name.contains(key.as_str());
            let name_d = edit_distance(&name, &key);
            let code_d = edit_distance(&info.code, &code_key);
            let close = contains || name_d <= 2 || code_d <= 1;
            if close {
                let rank = if contains { 0 } else { name_d.min(code_d + 1) };
                scored.push((rank, info.id));
            }
        }
        scored.sort();
        candidates = scored.into_iter().map(|(_, id)| id).collect();
    }

    if candidates.is_empty() {
        return Err(format!(
            "unknown region '{}'. Use a region code such as 'fr' or 'us-w', or a name such as 'France'.",
            raw
        ));
    }
    let total = candidates.len();
    let shown: Vec<String> = candidates
        .iter()
        .take(8)
        .map(|&id| {
            let info = map.region(id);
            format!("{} ({})", info.code, info.name)
        })
        .collect();
    let more = if total > shown.len() {
        format!(" and {} more", total - shown.len())
    } else {
        String::new()
    };
    Err(format!(
        "unknown or ambiguous region '{}'. Did you mean: {}{}?",
        raw,
        shown.join(", "),
        more
    ))
}

/// A fresh random seed from the clock and process id (never `DEFAULT_SEED`,
/// which the UI treats as "random").
pub fn time_seed() -> u64 {
    let nanos: u64 = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as u64,
        Err(_) => 0x1234_5678_9ABC_DEF0,
    };
    let pid = u64::from(std::process::id());
    let mut z = (nanos ^ pid.rotate_left(32)).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    if z == DEFAULT_SEED {
        z ^ 1
    } else {
        z
    }
}
