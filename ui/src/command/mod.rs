//! The command system: everything between the typed line and the actions the
//! app performs. Pure logic, no terminal I/O.
//!
//! Pipeline for a line such as `a @weakest 25% ; rush fr ; !!`:
//!
//! 1. [`lexer`] splits it on `;` (outside double quotes) and into tokens.
//! 2. [`alias`] replaces `!!` with the last submitted line and expands user
//!    aliases (`$1..$9`, `$*`), recursively up to depth 8.
//! 3. [`parser`] turns each segment into an [`Action`] plus a hint line. Verbs
//!    and their short forms: `attack|a`, `fortify|f`, `recall|r`, `ally|al`,
//!    `unally|ua` (game verbs, validated with `Game::preview`), and `info|i`,
//!    `focus|w`, `status|s`, `speed|sp`, `pause|p`, `alias`, `unalias`,
//!    `help|h`, `save|sv`, `quit|q`, `theme` (UI verbs). Region and player
//!    names go through [`fuzzy`].
//! 4. [`complete`] computes ghost text for the token being typed.
//!
//! The app calls [`analyze`] on every edit (hint strip + ghost text) and
//! [`submit`] on Enter. A submit with errors returns no actions and the app
//! keeps the line so the player can fix it. [`History`] handles Up/Down.
//!
//! Decisions: `attack` without an amount uses the context's default percent
//! (neutral or enemy target); `fortify` and `recall` require an amount;
//! `info`/`focus` without a region use your capital; `speed` without a value
//! resets to 100%; `theme` without a name cycles; `save` without a name quick-saves.

pub mod alias;
pub mod complete;
pub mod fuzzy;
pub mod help;
pub mod history;
pub mod lexer;
pub mod parser;

use std::collections::BTreeMap;

use territorial_core::{Command, Game, PlayerId, RegionId, TICKS_PER_SECOND};

pub use history::History;
pub use parser::Verb;

/// Everything the command system needs to read. Cheap to build every frame.
#[derive(Clone, Copy, Debug)]
pub struct CommandContext<'a> {
    /// The live game (read only).
    pub game: &'a Game,
    /// The player issuing commands (normally `territorial_core::HUMAN`).
    pub player: PlayerId,
    /// User aliases, name (lowercase) -> body, from `Settings::aliases`.
    pub aliases: &'a BTreeMap<String, String>,
    /// Attack percent used for neutral targets when no amount is typed (1..=100).
    pub default_attack_pct_neutral: u8,
    /// Attack percent used for enemy-owned targets when no amount is typed (1..=100).
    pub default_attack_pct_enemy: u8,
    /// The last successfully submitted line, used by `!!`
    /// (store [`SubmitResult::history_line`]; [`History::last`] gives it back).
    pub last_line: Option<&'a str>,
}

impl<'a> CommandContext<'a> {
    /// A context with the default attack percents (25% neutral, 40% enemy) and no last line.
    pub fn new(
        game: &'a Game,
        player: PlayerId,
        aliases: &'a BTreeMap<String, String>,
    ) -> CommandContext<'a> {
        CommandContext {
            game,
            player,
            aliases,
            default_attack_pct_neutral: 25,
            default_attack_pct_enemy: 40,
            last_line: None,
        }
    }
}

/// What the app should do for one successfully parsed command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Queue this command for the player (`Game::queue`).
    Game(Command),
    /// Show region details (`info`).
    Info(RegionId),
    /// Move the camera to a region (`focus`).
    Focus(RegionId),
    /// Show the status panel (`status`).
    Status,
    /// Set the game speed, in percent (100 = normal, 25..=800). Solo only.
    Speed(u32),
    /// Pause or resume. Solo only.
    TogglePause,
    /// Create or replace a user alias (name is lowercase and validated).
    DefineAlias {
        /// Alias name.
        name: String,
        /// Command text it expands to.
        body: String,
    },
    /// Remove a user alias (it exists in the context's aliases).
    RemoveAlias(String),
    /// List the user's aliases (`alias` with no arguments).
    ListAliases,
    /// Show help: `None` = overview, `Some(topic)` = a canonical verb name or
    /// a [`help::TOPICS`] entry. Render with [`help::help_lines`].
    Help(Option<String>),
    /// Save the match: `None` = quick save, `Some(name)` = validated name.
    Save(Option<String>),
    /// Leave the match.
    Quit,
    /// Change theme: `None` = cycle, `Some` = one of [`parser::THEME_NAMES`]
    /// (`"dark"`, `"high-contrast"`, `"colorblind"`).
    Theme(Option<String>),
}

/// A successfully parsed command segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCommand {
    /// What to do.
    pub action: Action,
    /// One-line description for the hint strip and the event log, e.g.
    /// `ATTACK  Brazil (neutral, garrison 120)  send 4,200 troops (40%), ETA 8s  [captures: defense 132]`.
    pub hint: String,
}

/// One command of a (possibly chained) line, after `!!` and alias expansion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// The expanded command text.
    pub text: String,
    /// The parsed command, or a short human-readable reason it is invalid.
    pub result: Result<ParsedCommand, String>,
}

/// Live analysis of the line being typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    /// Every segment in execution order (empty for a blank line).
    pub segments: Vec<Segment>,
    /// Ghost-text suffix for the token at the end of the line (may be empty).
    /// Show it dimmed after the cursor; Tab appends it to the line.
    pub ghost: String,
    /// The hint for the last segment as it would read with the ghost accepted,
    /// when that parses (e.g. while `a fr` is typed as `a f`).
    pub ghost_hint: Option<String>,
    /// Usage and example for the verb being typed, e.g.
    /// `attack <target> [amount]    e.g. a br 40%`.
    pub usage: Option<String>,
}

impl Analysis {
    /// Whether any segment is invalid.
    pub fn has_errors(&self) -> bool {
        self.segments.iter().any(|s| s.result.is_err())
    }

    /// The hint strip text: each segment's hint, or `! reason` for invalid
    /// ones, joined with `" | "`. Empty for a blank line.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = self
            .segments
            .iter()
            .map(|s| match &s.result {
                Ok(pc) => pc.hint.clone(),
                Err(e) => format!("! {}", e),
            })
            .collect();
        parts.join(" | ")
    }

    /// `input` with the ghost text accepted (what Tab produces).
    pub fn accept_ghost(&self, input: &str) -> String {
        let mut s = String::with_capacity(input.len() + self.ghost.len());
        s.push_str(input);
        s.push_str(&self.ghost);
        s
    }
}

/// The result of pressing Enter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubmitResult {
    /// Actions to perform, in order. Empty whenever `errors` is not empty
    /// (a chain runs all or nothing).
    pub actions: Vec<Action>,
    /// One hint line per action (same order), for flashing in the event log.
    pub echoes: Vec<String>,
    /// Why the line was rejected. When non-empty, keep the line in the prompt.
    pub errors: Vec<String>,
    /// On success: the line with `!!` expanded. Push it to [`History`] and use
    /// it as the next `last_line`. `None` on errors or for a blank line.
    pub history_line: Option<String>,
}

impl SubmitResult {
    /// Whether the line was accepted (no errors and something to do).
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty() && !self.actions.is_empty()
    }
}

fn parse_expanded(seg: &alias::ExpandedSegment, ctx: &CommandContext<'_>) -> Segment {
    let result = match &seg.error {
        Some(e) => Err(e.clone()),
        None => parser::parse_segment(&seg.text, ctx),
    };
    Segment {
        text: seg.text.clone(),
        result,
    }
}

/// Analyzes the line being typed: parsed segments with hints or errors, ghost
/// text for the last token, and a usage line. Call on every edit.
pub fn analyze(input: &str, ctx: &CommandContext<'_>) -> Analysis {
    let mut out = Analysis::default();
    if input.trim().is_empty() {
        return out;
    }
    let expanded = alias::expand_line(input, ctx.aliases, ctx.last_line);
    for seg in &expanded.segments {
        out.segments.push(parse_expanded(seg, ctx));
    }
    out.ghost = complete::ghost(input, ctx);
    if !out.ghost.is_empty() {
        let completed = out.accept_ghost(input);
        let again = alias::expand_line(&completed, ctx.aliases, ctx.last_line);
        if let Some(last) = again.segments.last() {
            if let Ok(pc) = parse_expanded(last, ctx).result {
                out.ghost_hint = Some(pc.hint);
            }
        }
    }
    out.usage = complete::usage_hint(input, ctx);
    out
}

/// Parses and validates a submitted line. On success every segment's action
/// is returned in order; on any error nothing is returned except the errors
/// (prefixed with the segment text when the line is a chain).
pub fn submit(line: &str, ctx: &CommandContext<'_>) -> SubmitResult {
    let mut res = SubmitResult::default();
    if line.trim().is_empty() {
        return res;
    }
    let expanded = alias::expand_line(line, ctx.aliases, ctx.last_line);
    if expanded.segments.is_empty() {
        res.errors.push("nothing to run".to_string());
        return res;
    }
    let multi = expanded.segments.len() > 1;
    let mut actions: Vec<Action> = Vec::new();
    let mut echoes: Vec<String> = Vec::new();
    for seg in &expanded.segments {
        let parsed = parse_expanded(seg, ctx);
        match parsed.result {
            Ok(pc) => {
                actions.push(pc.action);
                echoes.push(pc.hint);
            }
            Err(e) => {
                if multi {
                    res.errors.push(format!("{}: {}", parsed.text, e));
                } else {
                    res.errors.push(e);
                }
            }
        }
    }
    if res.errors.is_empty() {
        res.actions = actions;
        res.echoes = echoes;
        res.history_line = Some(expanded.history_line);
    }
    res
}

/// Formats an integer with thousands separators: `4200` -> `4,200`, `-1500` -> `-1,500`.
pub fn fmt_num(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Formats a tick count as game time: `80` -> `8s`, `45` -> `4.5s`, `1250` -> `2m05s`.
pub fn fmt_ticks(ticks: u64) -> String {
    let tps = TICKS_PER_SECOND.max(1);
    let secs = ticks / tps;
    if secs >= 60 {
        return format!("{}m{:02}s", secs / 60, secs % 60);
    }
    let tenths = (ticks % tps) * 10 / tps;
    if tenths == 0 {
        format!("{}s", secs)
    } else {
        format!("{}.{}s", secs, tenths)
    }
}
