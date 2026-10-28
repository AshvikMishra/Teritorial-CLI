//! Parsing one (already alias-expanded) command segment into an [`Action`]
//! plus a one-line hint for the hint strip.
//!
//! Grammar: `<verb> <target> [amount]`. For `attack`, `fortify` and `recall`
//! the amount may also come first (`a 40% fr`), and a target may span several
//! words (`a south africa 30%`). There are no options in v1; anything that is
//! neither target nor amount is reported as part of an unknown target.
//!
//! Amounts: `40%` (1..=100), `500`, `4,200`, `1.2k`, `2k`, `1.5m`, `half` (50%),
//! `all`/`max` (100%). `attack` without an amount uses the default percent from
//! the context: `default_attack_pct_neutral` for neutral targets and
//! `default_attack_pct_enemy` for enemy-owned ones. `fortify` and `recall`
//! **require** an amount (moving garrisons by accident is costly).

use territorial_core::{Amount, Command, CommandPreview, Game, PlayerId, RegionId, SmartTarget};

use super::alias;
use super::fuzzy::{self, Relevance};
use super::help;
use super::lexer::{self, Token};
use super::{fmt_num, fmt_ticks, Action, CommandContext, ParsedCommand};

/// Lowest accepted game speed, in percent.
pub const SPEED_MIN_PCT: u32 = 25;
/// Highest accepted game speed, in percent.
pub const SPEED_MAX_PCT: u32 = 800;
/// Canonical theme names carried by [`Action::Theme`].
pub const THEME_NAMES: [&str; 3] = ["dark", "high-contrast", "colorblind"];
/// Longest accepted save name.
pub const MAX_SAVE_NAME_LEN: usize = 48;

/// A built-in command verb.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verb {
    /// `attack|a <target> [amount]`
    Attack,
    /// `fortify|f <region> <amount>`
    Fortify,
    /// `recall|r <region> <amount>`
    Recall,
    /// `ally|al <player>`
    Ally,
    /// `unally|ua <player>`
    Unally,
    /// `info|i [region]`
    Info,
    /// `focus|w [region]`
    Focus,
    /// `status|s`
    Status,
    /// `speed|sp [multiplier]`
    Speed,
    /// `pause|p`
    Pause,
    /// `alias [name [command...]]`
    Alias,
    /// `unalias <name>`
    Unalias,
    /// `help|h [topic]`
    Help,
    /// `save|sv [name]`
    Save,
    /// `quit|q`
    Quit,
    /// `theme [name]`
    Theme,
}

impl Verb {
    /// Every verb, in help order.
    pub const ALL: [Verb; 16] = [
        Verb::Attack,
        Verb::Fortify,
        Verb::Recall,
        Verb::Ally,
        Verb::Unally,
        Verb::Info,
        Verb::Focus,
        Verb::Status,
        Verb::Speed,
        Verb::Pause,
        Verb::Alias,
        Verb::Unalias,
        Verb::Help,
        Verb::Save,
        Verb::Quit,
        Verb::Theme,
    ];

    /// The full verb, e.g. `attack`.
    pub fn name(self) -> &'static str {
        match self {
            Verb::Attack => "attack",
            Verb::Fortify => "fortify",
            Verb::Recall => "recall",
            Verb::Ally => "ally",
            Verb::Unally => "unally",
            Verb::Info => "info",
            Verb::Focus => "focus",
            Verb::Status => "status",
            Verb::Speed => "speed",
            Verb::Pause => "pause",
            Verb::Alias => "alias",
            Verb::Unalias => "unalias",
            Verb::Help => "help",
            Verb::Save => "save",
            Verb::Quit => "quit",
            Verb::Theme => "theme",
        }
    }

    /// The short alias, e.g. `a`, if the verb has one.
    pub fn short(self) -> Option<&'static str> {
        match self {
            Verb::Attack => Some("a"),
            Verb::Fortify => Some("f"),
            Verb::Recall => Some("r"),
            Verb::Ally => Some("al"),
            Verb::Unally => Some("ua"),
            Verb::Info => Some("i"),
            Verb::Focus => Some("w"),
            Verb::Status => Some("s"),
            Verb::Speed => Some("sp"),
            Verb::Pause => Some("p"),
            Verb::Alias => None,
            Verb::Unalias => None,
            Verb::Help => Some("h"),
            Verb::Save => Some("sv"),
            Verb::Quit => Some("q"),
            Verb::Theme => None,
        }
    }

    /// Whether the verb becomes a `territorial_core::Command`.
    pub fn is_game(self) -> bool {
        matches!(
            self,
            Verb::Attack | Verb::Fortify | Verb::Recall | Verb::Ally | Verb::Unally
        )
    }

    /// Exact, case-insensitive match on the full verb or its short alias.
    pub fn from_exact(word: &str) -> Option<Verb> {
        let w = word.trim().to_lowercase();
        Verb::ALL
            .iter()
            .copied()
            .find(|&v| v.name() == w.as_str() || v.short() == Some(w.as_str()))
    }

    /// [`Verb::from_exact`], then a unique prefix (2+ characters) of a full
    /// verb (`att` -> attack, `fo` is ambiguous between fortify and focus).
    pub fn lookup(word: &str) -> Option<Verb> {
        if let Some(v) = Verb::from_exact(word) {
            return Some(v);
        }
        let w = word.trim().to_lowercase();
        if w.chars().count() < 2 {
            return None;
        }
        let matches: Vec<Verb> = Verb::ALL
            .iter()
            .copied()
            .filter(|v| v.name().starts_with(w.as_str()))
            .collect();
        if matches.len() == 1 {
            Some(matches[0])
        } else {
            None
        }
    }
}

/// The region preference used to break ties for a verb's target.
pub fn relevance_for(verb: Verb) -> Relevance {
    match verb {
        Verb::Attack => Relevance::Attackable,
        Verb::Fortify | Verb::Recall => Relevance::Owned,
        Verb::Info | Verb::Focus => Relevance::Nearby,
        _ => Relevance::Any,
    }
}

/// Whether a token should be read as an amount rather than a region: `all`,
/// `half`, `max`, or anything starting with a digit or `.`.
pub fn looks_like_amount(token: &str) -> bool {
    let t = token.trim().to_lowercase();
    if t == "all" || t == "half" || t == "max" {
        return true;
    }
    match t.chars().next() {
        Some(c) => c.is_ascii_digit() || c == '.',
        None => false,
    }
}

/// Parses a non-negative decimal like `1.25` into an integer scaled by
/// `10^scale` (`parse_scaled("1.2", 3) == Some(1200)`). More fractional digits
/// than `scale`, signs, or overflow give `None`.
pub fn parse_scaled(s: &str, scale: u32) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let (int_part, frac_part) = match s.find('.') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    if !int_part.chars().all(|c| c.is_ascii_digit())
        || !frac_part.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    if frac_part.len() > scale as usize {
        return None;
    }
    let mut v: u64 = 0;
    for c in int_part.chars() {
        let d = c.to_digit(10)? as u64;
        v = v.checked_mul(10)?.checked_add(d)?;
    }
    let mut frac_digits: u32 = 0;
    for c in frac_part.chars() {
        let d = c.to_digit(10)? as u64;
        v = v.checked_mul(10)?.checked_add(d)?;
        frac_digits += 1;
    }
    let mut k = frac_digits;
    while k < scale {
        v = v.checked_mul(10)?;
        k += 1;
    }
    Some(v)
}

/// Parses an amount token: `40%`, `500`, `4,200`, `1.2k`, `2k`, `1.5m`,
/// `half` (50%), `all` or `max` (100%).
pub fn parse_amount(token: &str) -> Result<Amount, String> {
    let t = token.trim().to_lowercase();
    if t.is_empty() {
        return Err("missing amount".to_string());
    }
    if t == "all" || t == "max" {
        return Ok(Amount::Percent(100));
    }
    if t == "half" {
        return Ok(Amount::Percent(50));
    }
    if let Some(num) = t.strip_suffix('%') {
        let digits = num.trim();
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!(
                "bad percent '{}' (use a whole number like 40%)",
                token.trim()
            ));
        }
        let v: u64 = match digits.parse::<u64>() {
            Ok(v) => v,
            Err(_) => return Err(format!("{} is more than 100%", digits)),
        };
        if v == 0 {
            return Err("amount must be greater than zero".to_string());
        }
        if v > 100 {
            return Err(format!("{}% is more than 100%", v));
        }
        return Ok(Amount::Percent(v as u8));
    }
    let cleaned: String = t.chars().filter(|&c| c != ',' && c != '_').collect();
    let parsed = if let Some(n) = cleaned.strip_suffix('k') {
        parse_scaled(n, 3)
    } else if let Some(n) = cleaned.strip_suffix('m') {
        parse_scaled(n, 6)
    } else {
        parse_scaled(&cleaned, 0)
    };
    match parsed {
        Some(0) => Err("amount must be greater than zero".to_string()),
        Some(v) => Ok(Amount::Flat(v)),
        None => Err(format!(
            "bad amount '{}' (try 500, 1.2k, 40%, half or all)",
            token.trim()
        )),
    }
}

/// Parses a game speed: `2`, `0.5`, `1.5x`, `x2` or `150%`, into percent
/// (`2` -> 200). Accepts [`SPEED_MIN_PCT`]..=[`SPEED_MAX_PCT`].
pub fn parse_speed(token: &str) -> Result<u32, String> {
    let t = token.trim().to_lowercase();
    let pct: Option<u64> = if let Some(n) = t.strip_suffix('%') {
        parse_scaled(n.trim(), 0)
    } else {
        let n: &str = match t.strip_suffix('x') {
            Some(rest) => rest,
            None => match t.strip_prefix('x') {
                Some(rest) => rest,
                None => t.as_str(),
            },
        };
        parse_scaled(n.trim(), 2)
    };
    match pct {
        Some(p) if p >= SPEED_MIN_PCT as u64 && p <= SPEED_MAX_PCT as u64 => Ok(p as u32),
        Some(_) => Err(format!(
            "speed must be between {} and {}",
            fmt_speed(SPEED_MIN_PCT),
            fmt_speed(SPEED_MAX_PCT)
        )),
        None => Err(format!(
            "bad speed '{}' (try sp 2, sp 0.5 or sp 150%)",
            token.trim()
        )),
    }
}

/// Formats a speed percent as a multiplier: `200` -> `2x`, `150` -> `1.5x`, `25` -> `0.25x`.
pub fn fmt_speed(pct: u32) -> String {
    let whole = pct / 100;
    let frac = pct % 100;
    if frac == 0 {
        format!("{}x", whole)
    } else if frac % 10 == 0 {
        format!("{}.{}x", whole, frac / 10)
    } else {
        format!("{}.{:02}x", whole, frac)
    }
}

/// Maps a theme word to its canonical name in [`THEME_NAMES`]: `dark`,
/// `high-contrast` (`hc`, `contrast`, `high`), `colorblind` (`cb`,
/// `colorblind-safe`). Unique prefixes work too.
pub fn parse_theme(word: &str) -> Option<&'static str> {
    let w = fuzzy::normalize(word);
    match w.as_str() {
        "hc" | "contrast" => return Some("high-contrast"),
        "cb" | "colourblind" | "colorblindsafe" | "colourblindsafe" | "cbsafe" => {
            return Some("colorblind")
        }
        "default" => return Some("dark"),
        _ => {}
    }
    let mut found: Option<&'static str> = None;
    let mut count = 0usize;
    for name in THEME_NAMES {
        if fuzzy::normalize(name).starts_with(w.as_str()) {
            found = Some(name);
            count += 1;
        }
    }
    if count == 1 {
        found
    } else {
        None
    }
}

/// Parses a smart target token (`@weakest`, `@w`, `@richest`, `@r`,
/// `@player:<name>`, `@p:<name>`).
pub fn parse_smart(text: &str, ctx: &CommandContext<'_>) -> Result<SmartTarget, String> {
    let shown = text.trim();
    let lower = shown.to_lowercase();
    let rest: &str = match lower.strip_prefix('@') {
        Some(r) => r,
        None => return Err(format!("'{}' is not a smart target", shown)),
    };
    let named: Option<&str> = match rest.strip_prefix("player:") {
        Some(n) => Some(n),
        None => rest.strip_prefix("p:"),
    };
    if let Some(name) = named {
        let p = fuzzy::resolve_player(ctx.game, ctx.player, name)?;
        return Ok(SmartTarget::Player(p));
    }
    if !rest.is_empty() && "weakest".starts_with(rest) {
        return Ok(SmartTarget::Weakest);
    }
    if !rest.is_empty() && "richest".starts_with(rest) {
        return Ok(SmartTarget::Richest);
    }
    if rest == "player" || rest == "p" {
        return Err("use @player:<name>, e.g. @p:bot-3".to_string());
    }
    Err(format!(
        "unknown smart target '{}' (try @weakest, @richest or @player:<name>)",
        shown
    ))
}

/// Resolves a target phrase: a smart target (starts with `@`) or a region
/// reference. Returns the region and, for smart targets, the typed label.
pub fn target_region(
    text: &str,
    ctx: &CommandContext<'_>,
    rel: Relevance,
) -> Result<(RegionId, Option<String>), String> {
    let t = text.trim();
    if t.starts_with('@') {
        let st = parse_smart(t, ctx)?;
        let r = territorial_core::resolve_target(ctx.game, ctx.player, &st)
            .map_err(|e| format!("{}: {}", t, e))?;
        return Ok((r, Some(t.to_lowercase())));
    }
    let r = fuzzy::resolve_region(ctx.game, ctx.player, t, rel)?;
    Ok((r, None))
}

/// The display name of player `p`.
pub fn player_name(game: &Game, p: PlayerId) -> String {
    match game.get_player(p) {
        Some(pl) => pl.name.clone(),
        None => format!("Player {}", p),
    }
}

/// `neutral`, `yours`, or the owner's name.
pub fn owner_label(game: &Game, owner: Option<PlayerId>, me: PlayerId) -> String {
    match owner {
        None => "neutral".to_string(),
        Some(p) if p == me => "yours".to_string(),
        Some(p) => player_name(game, p),
    }
}

/// The default attack percent for a target, from the context's settings.
pub fn default_attack_pct(ctx: &CommandContext<'_>, region: RegionId) -> u8 {
    let pct = match ctx.game.region_state(region).owner {
        None => ctx.default_attack_pct_neutral,
        Some(_) => ctx.default_attack_pct_enemy,
    };
    pct.clamp(1, 100)
}

/// Parses one command segment (no `;`, aliases and `!!` already expanded).
/// Game commands are validated with `Game::preview`, so a parse that succeeds
/// would be accepted by the simulation right now. For a segment that follows
/// other commands in a chain, use [`parse_chained_segment`].
pub fn parse_segment(text: &str, ctx: &CommandContext<'_>) -> Result<ParsedCommand, String> {
    parse_chained_segment(text, ctx, 0).map(|(pc, _)| pc)
}

/// [`parse_segment`] for a segment of a chain whose earlier segments take
/// `pool_spent` troops from the pool before this one runs. Attack and fortify
/// amounts are checked against what is left of the pool, and percents are
/// resolved on it for the hint (the queued command keeps its typed amount; the
/// simulation resolves it on the pool at that moment, which is the same).
/// Also returns how many pool troops this segment takes: the resolved amount
/// for attack and fortify, 0 for everything else.
pub fn parse_chained_segment(
    text: &str,
    ctx: &CommandContext<'_>,
    pool_spent: i64,
) -> Result<(ParsedCommand, i64), String> {
    let toks = lexer::tokenize(text);
    if toks.is_empty() {
        return Err("empty command".to_string());
    }
    let verb = match Verb::lookup(&toks[0].text) {
        Some(v) => v,
        None => return Err(verb_error(&toks[0].text, ctx)),
    };
    let args: Vec<String> = toks[1..].iter().map(|t| t.text.clone()).collect();
    match verb {
        Verb::Attack => parse_attack(&args, ctx, pool_spent),
        Verb::Fortify | Verb::Recall => parse_garrison(verb, &args, ctx, pool_spent),
        Verb::Ally | Verb::Unally => no_spend(parse_diplomacy(verb, &args, ctx)),
        Verb::Info | Verb::Focus => no_spend(parse_view(verb, &args, ctx)),
        Verb::Status => no_spend(parse_status(&args, ctx)),
        Verb::Speed => no_spend(parse_speed_cmd(&args)),
        Verb::Pause => {
            no_args(verb, &args)?;
            Ok((
                ParsedCommand {
                    action: Action::TogglePause,
                    hint: "PAUSE  pause or resume the game".to_string(),
                },
                0,
            ))
        }
        Verb::Alias => no_spend(parse_alias(text, &toks, ctx)),
        Verb::Unalias => no_spend(parse_unalias(&args, ctx)),
        Verb::Help => no_spend(parse_help(&args)),
        Verb::Save => no_spend(parse_save(&args)),
        Verb::Quit => {
            no_args(verb, &args)?;
            Ok((
                ParsedCommand {
                    action: Action::Quit,
                    hint: "QUIT  leave the match".to_string(),
                },
                0,
            ))
        }
        Verb::Theme => no_spend(parse_theme_cmd(&args)),
    }
}

/// A parse result for a command that takes nothing from the pool.
fn no_spend(r: Result<ParsedCommand, String>) -> Result<(ParsedCommand, i64), String> {
    r.map(|pc| (pc, 0))
}

/// Troops an attack or fortify takes from the pool when earlier commands of
/// the same chain already take `pool_spent`. `preview_troops` is what
/// `Game::preview` resolved against the whole pool; `max_pct` caps percents
/// (the balance's `max_attack_pct` for attacks, 100 for fortify).
fn chained_troops(
    ctx: &CommandContext<'_>,
    amount: Amount,
    preview_troops: i64,
    pool_spent: i64,
    max_pct: u32,
) -> Result<i64, String> {
    if pool_spent <= 0 {
        return Ok(preview_troops);
    }
    let left = ctx.game.pool(ctx.player).saturating_sub(pool_spent).max(0);
    let troops = match amount {
        Amount::Percent(p) => {
            let eff = (p as u32).min(max_pct.clamp(1, 100)) as i64;
            left.saturating_mul(eff) / 100
        }
        Amount::Flat(_) => preview_troops,
    };
    if troops < 1 || troops > left {
        return Err(format!(
            "not enough troops after the earlier commands in this chain (needs {}, {} left)",
            fmt_num(troops.max(1)),
            fmt_num(left)
        ));
    }
    Ok(troops)
}

fn verb_error(word: &str, ctx: &CommandContext<'_>) -> String {
    let w = word.trim().to_lowercase();
    if w.chars().count() >= 2 {
        let prefixed: Vec<&'static str> = Verb::ALL
            .iter()
            .map(|v| v.name())
            .filter(|n| n.starts_with(w.as_str()))
            .collect();
        if prefixed.len() > 1 {
            return format!("'{}' is ambiguous: {}", word, prefixed.join(", "));
        }
    }
    let mut names: Vec<String> = Verb::ALL.iter().map(|v| v.name().to_string()).collect();
    for k in ctx.aliases.keys() {
        names.push(k.clone());
    }
    match fuzzy::closest(&w, &names, 2) {
        Some(s) => format!("unknown command '{}' (did you mean {}?)", word, s),
        None => format!("unknown command '{}' (type help)", word),
    }
}

fn no_args(verb: Verb, args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("{} takes no arguments", verb.name()))
    }
}

/// Splits troop-command arguments into a target phrase and an optional amount.
fn split_target_amount(verb: Verb, args: &[String]) -> Result<(String, Option<Amount>), String> {
    let n = args.len();
    if n == 0 || (n == 1 && looks_like_amount(&args[0])) {
        let what = if verb == Verb::Attack {
            "target"
        } else {
            "region"
        };
        return Err(format!(
            "{} needs a {}, e.g. '{}'",
            verb.name(),
            what,
            help::example(verb)
        ));
    }
    if n >= 2 && looks_like_amount(&args[n - 1]) {
        let a = parse_amount(&args[n - 1])?;
        return Ok((args[..n - 1].join(" "), Some(a)));
    }
    if n >= 2 && looks_like_amount(&args[0]) {
        let a = parse_amount(&args[0])?;
        return Ok((args[1..].join(" "), Some(a)));
    }
    Ok((args.join(" "), None))
}

fn parse_attack(
    args: &[String],
    ctx: &CommandContext<'_>,
    pool_spent: i64,
) -> Result<(ParsedCommand, i64), String> {
    let (target_text, amount) = split_target_amount(Verb::Attack, args)?;
    let (region, smart) = target_region(&target_text, ctx, Relevance::Attackable)?;
    let amount = match amount {
        Some(a) => a,
        None => Amount::Percent(default_attack_pct(ctx, region)),
    };
    let cmd = Command::Attack {
        target: region,
        amount,
    };
    let pv = ctx
        .game
        .preview(ctx.player, &cmd)
        .map_err(|e| e.to_string())?;
    let max_pct = ctx.game.balance().max_attack_pct;
    let troops = chained_troops(ctx, amount, pv.troops, pool_spent, max_pct)?;
    let hint = attack_hint(ctx, region, smart.as_deref(), &pv, troops, amount);
    Ok((
        ParsedCommand {
            action: Action::Game(cmd),
            hint,
        },
        troops,
    ))
}

fn amount_note(amount: Amount) -> String {
    match amount {
        Amount::Percent(p) => format!(" ({}%)", p),
        Amount::Flat(_) => String::new(),
    }
}

fn attack_hint(
    ctx: &CommandContext<'_>,
    region: RegionId,
    smart: Option<&str>,
    pv: &CommandPreview,
    troops: i64,
    amount: Amount,
) -> String {
    let game = ctx.game;
    let name = game.map().region(region).name.clone();
    let label = match smart {
        Some(s) => format!("{} -> {}", s, name),
        None => name,
    };
    let owner = owner_label(game, pv.target_owner, ctx.player);
    let eta = match pv.eta_ticks {
        Some(t) => format!(", ETA {}", fmt_ticks(t)),
        None => String::new(),
    };
    let en_route: i64 = game
        .marches_to(region)
        .iter()
        .filter(|m| m.owner == ctx.player)
        .map(|m| m.troops)
        .sum();
    let mut s = format!(
        "ATTACK  {} ({}, garrison {})  send {} troops{}{}",
        label,
        owner,
        fmt_num(pv.target_garrison),
        fmt_num(troops),
        amount_note(amount),
        eta
    );
    if en_route > 0 {
        s.push_str(&format!(", +{} en route", fmt_num(en_route)));
    }
    let total = troops.saturating_add(en_route);
    if total > pv.defense_needed {
        s.push_str(&format!(
            "  [captures: defense {}]",
            fmt_num(pv.defense_needed)
        ));
    } else {
        s.push_str(&format!(
            "  [too few: defense {}]",
            fmt_num(pv.defense_needed)
        ));
    }
    s
}

fn parse_garrison(
    verb: Verb,
    args: &[String],
    ctx: &CommandContext<'_>,
    pool_spent: i64,
) -> Result<(ParsedCommand, i64), String> {
    let (target_text, amount) = split_target_amount(verb, args)?;
    if target_text.trim_start().starts_with('@') {
        return Err(format!(
            "{} needs one of your own regions; smart targets pick enemy regions",
            verb.name()
        ));
    }
    let amount = match amount {
        Some(a) => a,
        None => {
            return Err(format!(
                "{} needs an amount, e.g. '{}'",
                verb.name(),
                help::example(verb)
            ))
        }
    };
    let region = fuzzy::resolve_region(ctx.game, ctx.player, &target_text, Relevance::Owned)?;
    let cmd = if verb == Verb::Fortify {
        Command::Fortify { region, amount }
    } else {
        Command::Recall { region, amount }
    };
    let pv = ctx
        .game
        .preview(ctx.player, &cmd)
        .map_err(|e| e.to_string())?;
    // Fortify takes from the pool (shared with earlier commands of a chain);
    // recall takes from the region's garrison and returns troops only later.
    let spend = if verb == Verb::Fortify {
        chained_troops(ctx, amount, pv.troops, pool_spent, 100)?
    } else {
        0
    };
    let moved = if verb == Verb::Fortify { spend } else { pv.troops };
    let hint = garrison_hint(ctx, verb, region, pv.target_garrison, moved, amount);
    Ok((
        ParsedCommand {
            action: Action::Game(cmd),
            hint,
        },
        spend,
    ))
}

/// The hint for a fortify or recall of `troops` on `region` (garrison `g`).
fn garrison_hint(
    ctx: &CommandContext<'_>,
    verb: Verb,
    region: RegionId,
    g: i64,
    troops: i64,
    amount: Amount,
) -> String {
    let name = ctx.game.map().region(region).name.clone();
    if verb == Verb::Fortify {
        format!(
            "FORTIFY  {} (garrison {})  +{} troops from pool{} -> {}, fortified for {}",
            name,
            fmt_num(g),
            fmt_num(troops),
            amount_note(amount),
            fmt_num(g.saturating_add(troops)),
            fmt_ticks(ctx.game.balance().fortify_duration_ticks)
        )
    } else {
        format!(
            "RECALL  {} (garrison {})  pull {} troops{} back to pool in {}",
            name,
            fmt_num(g),
            fmt_num(troops),
            amount_note(amount),
            fmt_ticks(ctx.game.balance().recall_delay_ticks)
        )
    }
}

fn parse_diplomacy(
    verb: Verb,
    args: &[String],
    ctx: &CommandContext<'_>,
) -> Result<ParsedCommand, String> {
    if args.is_empty() {
        return Err(format!(
            "{} needs a player, e.g. '{}'",
            verb.name(),
            help::example(verb)
        ));
    }
    let text = args.join(" ");
    let p = fuzzy::resolve_player(ctx.game, ctx.player, &text)?;
    let cmd = if verb == Verb::Ally {
        Command::ProposeAlliance { with: p }
    } else {
        Command::BreakAlliance { with: p }
    };
    ctx.game
        .preview(ctx.player, &cmd)
        .map_err(|e| e.to_string())?;
    Ok(ParsedCommand {
        action: Action::Game(cmd),
        hint: diplomacy_hint(ctx, verb, p),
    })
}

/// The hint for `ally` or `unally` with player `p`.
fn diplomacy_hint(ctx: &CommandContext<'_>, verb: Verb, p: PlayerId) -> String {
    let name = player_name(ctx.game, p);
    if verb == Verb::Ally {
        if ctx.game.pending_proposal(p, ctx.player) {
            format!("ALLY  {}  accept their alliance proposal", name)
        } else {
            format!("ALLY  {}  propose an alliance", name)
        }
    } else {
        format!(
            "UNALLY  {}  break the alliance ({} truce follows)",
            name,
            fmt_ticks(ctx.game.balance().truce_ticks)
        )
    }
}

/// Validates a ready-made game command (built by a hotkey instead of typed)
/// with `Game::preview` and describes it with the same hint the typed form
/// gets, so hotkeys and typed commands share one validation path. `smart` is
/// the smart-target label shown for an attack (`@weakest`).
pub fn describe_command(
    cmd: Command,
    smart: Option<&str>,
    ctx: &CommandContext<'_>,
) -> Result<ParsedCommand, String> {
    let pv = ctx
        .game
        .preview(ctx.player, &cmd)
        .map_err(|e| e.to_string())?;
    let hint = match cmd {
        Command::Attack { target, amount } => {
            attack_hint(ctx, target, smart, &pv, pv.troops, amount)
        }
        Command::Fortify { region, amount } => {
            garrison_hint(ctx, Verb::Fortify, region, pv.target_garrison, pv.troops, amount)
        }
        Command::Recall { region, amount } => {
            garrison_hint(ctx, Verb::Recall, region, pv.target_garrison, pv.troops, amount)
        }
        Command::ProposeAlliance { with } => diplomacy_hint(ctx, Verb::Ally, with),
        Command::BreakAlliance { with } => diplomacy_hint(ctx, Verb::Unally, with),
    };
    Ok(ParsedCommand {
        action: Action::Game(cmd),
        hint,
    })
}

fn parse_view(
    verb: Verb,
    args: &[String],
    ctx: &CommandContext<'_>,
) -> Result<ParsedCommand, String> {
    let game = ctx.game;
    let (region, smart) = if args.is_empty() {
        match game.get_player(ctx.player).and_then(|p| p.capital) {
            Some(r) => (r, None),
            None => {
                return Err(format!(
                    "{} needs a region, e.g. '{}'",
                    verb.name(),
                    help::example(verb)
                ))
            }
        }
    } else {
        target_region(&args.join(" "), ctx, Relevance::Nearby)?
    };
    let info = game.map().region(region);
    let label = match &smart {
        Some(s) => format!("{} -> {}", s, info.name),
        None => info.name.clone(),
    };
    if verb == Verb::Info {
        let st = game.region_state(region);
        let hint = format!(
            "INFO  {} [{}]  {}, garrison {}, value {}, {} neighbors",
            label,
            info.code,
            owner_label(game, st.owner, ctx.player),
            fmt_num(st.garrison),
            info.value,
            game.map().neighbors(region).len()
        );
        Ok(ParsedCommand {
            action: Action::Info(region),
            hint,
        })
    } else {
        Ok(ParsedCommand {
            action: Action::Focus(region),
            hint: format!("FOCUS  {} [{}]", label, info.code),
        })
    }
}

fn parse_status(args: &[String], ctx: &CommandContext<'_>) -> Result<ParsedCommand, String> {
    no_args(Verb::Status, args)?;
    let g = ctx.game;
    let me = ctx.player;
    let hint = if g.get_player(me).is_some() {
        let board = g.leaderboard();
        let rank = match board.iter().position(|&p| p == me) {
            Some(i) => i + 1,
            None => 0,
        };
        format!(
            "STATUS  {} regions, pool {} (+{}/s), land {}%, rank {}/{}",
            g.region_count(me),
            fmt_num(g.pool(me)),
            fmt_num(g.growth_per_sec(me)),
            g.land_pct(me),
            rank,
            board.len()
        )
    } else {
        "STATUS  match overview".to_string()
    };
    Ok(ParsedCommand {
        action: Action::Status,
        hint,
    })
}

fn parse_speed_cmd(args: &[String]) -> Result<ParsedCommand, String> {
    match args.len() {
        0 => Ok(ParsedCommand {
            action: Action::Speed(100),
            hint: "SPEED  back to normal speed (1x)".to_string(),
        }),
        1 => {
            let pct = parse_speed(&args[0])?;
            Ok(ParsedCommand {
                action: Action::Speed(pct),
                hint: format!("SPEED  set game speed to {}", fmt_speed(pct)),
            })
        }
        _ => Err("usage: speed <multiplier>, e.g. 'sp 2' or 'sp 0.5'".to_string()),
    }
}

fn parse_alias(
    text: &str,
    toks: &[Token],
    ctx: &CommandContext<'_>,
) -> Result<ParsedCommand, String> {
    if toks.len() < 2 {
        let n = ctx.aliases.len();
        return Ok(ParsedCommand {
            action: Action::ListAliases,
            hint: format!(
                "ALIAS  list your {} alias{}",
                n,
                if n == 1 { "" } else { "es" }
            ),
        });
    }
    let name = toks[1].text.to_lowercase();
    alias::validate_name(&name)?;
    if toks.len() == 2 {
        return Err(match ctx.aliases.get(name.as_str()) {
            Some(body) => format!("{} = {}  (add a command to redefine it)", name, body),
            None => format!(
                "alias {} needs a command, e.g. 'alias {} a $1 80%'",
                name, name
            ),
        });
    }
    let raw_body: &str = match text.get(toks[1].end..) {
        Some(s) => s.trim(),
        None => "",
    };
    let body = alias::strip_quotes(raw_body);
    alias::validate(&name, &body, ctx.aliases)?;
    let hint = match ctx.aliases.get(name.as_str()) {
        Some(old) => format!("ALIAS  {} = {}  (replaces: {})", name, body, old),
        None => format!("ALIAS  {} = {}", name, body),
    };
    Ok(ParsedCommand {
        action: Action::DefineAlias { name, body },
        hint,
    })
}

fn parse_unalias(args: &[String], ctx: &CommandContext<'_>) -> Result<ParsedCommand, String> {
    if args.len() != 1 {
        return Err("usage: unalias <name>".to_string());
    }
    let name = args[0].to_lowercase();
    let hint = match ctx.aliases.get(name.as_str()) {
        Some(body) => format!("UNALIAS  remove {} ({})", name, body),
        None => return Err(format!("no alias named '{}'", name)),
    };
    Ok(ParsedCommand {
        action: Action::RemoveAlias(name),
        hint,
    })
}

fn parse_help(args: &[String]) -> Result<ParsedCommand, String> {
    if args.is_empty() {
        return Ok(ParsedCommand {
            action: Action::Help(None),
            hint: "HELP  command overview".to_string(),
        });
    }
    let word = args.join(" ");
    match help::topic_key(&word) {
        Some(k) => Ok(ParsedCommand {
            action: Action::Help(Some(k.to_string())),
            hint: format!("HELP  {}", k),
        }),
        None => Err(format!(
            "no help for '{}' (try help, help attack, help targets)",
            word
        )),
    }
}

/// Checks a save name: 1 to [`MAX_SAVE_NAME_LEN`] characters of `A-Z a-z 0-9 - _`.
pub fn validate_save_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("save name is empty".to_string());
    }
    if name.chars().count() > MAX_SAVE_NAME_LEN {
        return Err(format!(
            "save name is too long (max {} characters)",
            MAX_SAVE_NAME_LEN
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("save names may only use letters, digits, - and _".to_string());
    }
    Ok(())
}

fn parse_save(args: &[String]) -> Result<ParsedCommand, String> {
    match args.len() {
        0 => Ok(ParsedCommand {
            action: Action::Save(None),
            hint: "SAVE  save the match (quick save)".to_string(),
        }),
        1 => {
            let name = args[0].trim().to_string();
            validate_save_name(&name)?;
            Ok(ParsedCommand {
                hint: format!("SAVE  save the match as '{}'", name),
                action: Action::Save(Some(name)),
            })
        }
        _ => Err("save names cannot contain spaces".to_string()),
    }
}

fn parse_theme_cmd(args: &[String]) -> Result<ParsedCommand, String> {
    match args.len() {
        0 => Ok(ParsedCommand {
            action: Action::Theme(None),
            hint: "THEME  switch to the next color theme".to_string(),
        }),
        1 => match parse_theme(&args[0]) {
            Some(t) => Ok(ParsedCommand {
                action: Action::Theme(Some(t.to_string())),
                hint: format!("THEME  switch to {}", t),
            }),
            None => Err(format!(
                "unknown theme '{}' (dark, high-contrast, colorblind)",
                args[0]
            )),
        },
        _ => Err("usage: theme [dark|high-contrast|colorblind]".to_string()),
    }
}
