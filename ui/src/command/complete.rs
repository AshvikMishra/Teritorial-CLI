//! Ghost-text completion for the token being typed at the end of the line.
//!
//! [`ghost`] returns only the missing suffix (shown dimmed after the cursor;
//! Tab appends it). It completes verbs, alias names, region codes and names,
//! smart targets, player names, amounts (`40` -> `40%`, `h` -> `half`), help
//! topics and theme names. Candidates are ranked by likely intent: attackable
//! regions first for `attack`, owned regions first for `fortify`/`recall`,
//! owned or attackable first for `info`/`focus`, non-allied players for smart
//! targets, proposers then non-allies for `ally`, allies for `unally`.
//! A token that is already complete gets no ghost.

use territorial_core::{PlayerId, RegionId};

use super::fuzzy::{self, Relevance};
use super::help;
use super::lexer;
use super::parser::{self, Verb};
use super::CommandContext;

/// Verb completion order: most used first.
const VERB_ORDER: [Verb; 16] = [
    Verb::Attack,
    Verb::Fortify,
    Verb::Recall,
    Verb::Info,
    Verb::Focus,
    Verb::Status,
    Verb::Ally,
    Verb::Unally,
    Verb::Speed,
    Verb::Pause,
    Verb::Help,
    Verb::Alias,
    Verb::Unalias,
    Verb::Save,
    Verb::Quit,
    Verb::Theme,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlayerPref {
    /// Smart target: non-allies first.
    Enemy,
    /// `ally`: players who proposed to you, then non-allies, then allies.
    AllyTarget,
    /// `unally`: allies first.
    UnallyTarget,
}

/// The ghost-text suffix for the end of `input` (empty when there is nothing
/// useful to suggest). Assumes the cursor is at the end of the line.
pub fn ghost(input: &str, ctx: &CommandContext<'_>) -> String {
    let seg = lexer::last_segment(input);
    if seg.trim().is_empty() || lexer::has_open_quote(seg) {
        return String::new();
    }
    let ends_with_space = match seg.chars().last() {
        Some(c) => c.is_whitespace(),
        None => true,
    };
    if ends_with_space {
        return String::new();
    }
    let toks = lexer::tokenize(seg);
    if toks.is_empty() {
        return String::new();
    }
    let idx = toks.len() - 1;
    let cur = toks[idx].text.as_str();
    if cur.is_empty() {
        return String::new();
    }
    if idx == 0 {
        return complete_verb(cur, ctx);
    }
    let verb = match segment_verb(&toks[0].text, ctx) {
        Some(v) => v,
        None => return String::new(),
    };
    let args: Vec<&str> = toks[1..].iter().map(|t| t.text.as_str()).collect();
    match verb {
        Verb::Attack | Verb::Fortify | Verb::Recall | Verb::Info | Verb::Focus => {
            complete_target_or_amount(verb, &args, ctx)
        }
        Verb::Ally => complete_player(&args.join(" "), PlayerPref::AllyTarget, ctx),
        Verb::Unally => complete_player(&args.join(" "), PlayerPref::UnallyTarget, ctx),
        Verb::Help => {
            if idx == 1 {
                let words: Vec<String> = help::topic_words()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                complete_word(cur, &words)
            } else {
                String::new()
            }
        }
        Verb::Theme => {
            if idx == 1 {
                let words: Vec<String> =
                    parser::THEME_NAMES.iter().map(|s| s.to_string()).collect();
                complete_word(cur, &words)
            } else {
                String::new()
            }
        }
        Verb::Unalias => {
            if idx == 1 {
                let words: Vec<String> = ctx.aliases.keys().cloned().collect();
                complete_word(cur, &words)
            } else {
                String::new()
            }
        }
        Verb::Alias => {
            if idx == 2 {
                complete_verb(cur, ctx)
            } else {
                String::new()
            }
        }
        Verb::Status | Verb::Speed | Verb::Pause | Verb::Save | Verb::Quit => String::new(),
    }
}

/// A usage line for the verb of the segment being typed, e.g.
/// `attack <target> [amount]    e.g. a br 40%`, or an alias definition.
pub fn usage_hint(input: &str, ctx: &CommandContext<'_>) -> Option<String> {
    let seg = lexer::last_segment(input);
    let toks = lexer::tokenize(seg);
    let first = toks.first()?;
    if first.text == "!!" {
        return Some(match ctx.last_line {
            Some(l) => format!("!! repeats: {}", l),
            None => "!! repeats the last command (none yet)".to_string(),
        });
    }
    let lower = first.text.to_lowercase();
    if Verb::from_exact(&lower).is_none() {
        if let Some(body) = ctx.aliases.get(lower.as_str()) {
            return Some(format!("{} = {}", lower, body));
        }
    }
    let v = Verb::lookup(&lower)?;
    Some(format!("{}    e.g. {}", help::usage(v), help::example(v)))
}

/// The verb a segment starts with: a built-in (exact or unique prefix), or the
/// first verb of a user alias body.
fn segment_verb(word: &str, ctx: &CommandContext<'_>) -> Option<Verb> {
    if let Some(v) = Verb::from_exact(word) {
        return Some(v);
    }
    let lower = word.to_lowercase();
    if let Some(body) = ctx.aliases.get(lower.as_str()) {
        let toks = lexer::tokenize(body);
        return match toks.first() {
            Some(t) => Verb::from_exact(&t.text).or_else(|| Verb::lookup(&t.text)),
            None => None,
        };
    }
    Verb::lookup(word)
}

/// Completes `cur` against `words` in order; no ghost if `cur` already equals one.
fn complete_word(cur: &str, words: &[String]) -> String {
    let lower = cur.to_lowercase();
    if words.iter().any(|w| w.to_lowercase() == lower) {
        return String::new();
    }
    for w in words {
        if let Some(rest) = fuzzy::ci_prefix_rest(w, cur) {
            return rest;
        }
    }
    String::new()
}

fn complete_verb(cur: &str, ctx: &CommandContext<'_>) -> String {
    if cur == "!" {
        return "!".to_string();
    }
    let mut words: Vec<String> = Vec::new();
    for v in VERB_ORDER.iter() {
        words.push(v.name().to_string());
    }
    for v in Verb::ALL.iter() {
        if let Some(s) = v.short() {
            words.push(s.to_string());
        }
    }
    for k in ctx.aliases.keys() {
        words.push(k.clone());
    }
    complete_word(cur, &words)
}

fn complete_target_or_amount(verb: Verb, args: &[&str], ctx: &CommandContext<'_>) -> String {
    if args.is_empty() {
        return String::new();
    }
    let idx = args.len() - 1;
    let cur = args[idx];
    if cur.starts_with('@') {
        if idx == 0 || !matches!(verb, Verb::Fortify | Verb::Recall) {
            return complete_smart(cur, ctx);
        }
        return String::new();
    }
    let troops = matches!(verb, Verb::Attack | Verb::Fortify | Verb::Recall);
    // Arguments before the current one may already name a complete target, in
    // which case the current token is an amount.
    if troops && idx >= 1 {
        let before = args[..idx].join(" ");
        let target_done = before.trim_start().starts_with('@')
            || fuzzy::resolve_region(ctx.game, ctx.player, &before, parser::relevance_for(verb))
                .is_ok();
        if target_done {
            if let Some(g) = complete_amount(cur) {
                return g;
            }
        }
    }
    let start = if troops && args.len() > 1 && parser::looks_like_amount(args[0]) {
        1
    } else {
        0
    };
    if start > idx {
        return String::new();
    }
    let phrase = args[start..].join(" ");
    if troops && start == 0 && idx == 0 && parser::looks_like_amount(cur) {
        // A leading amount (`a 40`): complete it as an amount.
        return complete_amount(cur).unwrap_or_default();
    }
    complete_region(&phrase, parser::relevance_for(verb), ctx)
}

/// Completes an amount token: `40` -> `%` (1..=100), `a` -> `ll`, `h` -> `alf`.
/// `None` if the token does not look like an amount.
fn complete_amount(cur: &str) -> Option<String> {
    let lower = cur.to_lowercase();
    if lower.is_empty() {
        return None;
    }
    if lower.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(v) = lower.parse::<u32>() {
            if (1..=100).contains(&v) {
                return Some("%".to_string());
            }
        }
        return Some(String::new());
    }
    if lower == "all" || lower == "half" || lower == "max" {
        return Some(String::new());
    }
    for w in ["all", "half"] {
        if w.starts_with(lower.as_str()) {
            return Some(w[lower.len()..].to_string());
        }
    }
    if parser::looks_like_amount(&lower) {
        return Some(String::new());
    }
    None
}

fn complete_region(phrase: &str, rel: Relevance, ctx: &CommandContext<'_>) -> String {
    let typed = phrase.trim_start();
    if typed.is_empty() {
        return String::new();
    }
    let game = ctx.game;
    let map = game.map();
    if map.find_code(&typed.to_lowercase()).is_some() {
        return String::new();
    }
    let single_token = !typed.chars().any(|c| c.is_whitespace());
    // Key: (relevance tier, 0 = code / 1 = name, completion length, id).
    let mut best: Option<((u8, u8, usize, RegionId), String)> = None;
    for info in &map.regions {
        let name_rest = fuzzy::ci_prefix_rest(&info.name, typed);
        if let Some(r) = &name_rest {
            if r.is_empty() {
                // The phrase already is a full name.
                return String::new();
            }
        }
        let tier: u8 = if rel == Relevance::Any || rel.matches(game, ctx.player, info.id) {
            0
        } else {
            1
        };
        let mut cands: Vec<(u8, String)> = Vec::new();
        if single_token {
            if let Some(rest) = fuzzy::ci_prefix_rest(&info.code, typed) {
                cands.push((0, rest));
            }
        }
        if let Some(rest) = name_rest {
            cands.push((1, rest));
        }
        for (kind, rest) in cands {
            let key = (tier, kind, rest.chars().count(), info.id);
            let better = match &best {
                None => true,
                Some((bk, _)) => key < *bk,
            };
            if better {
                best = Some((key, rest));
            }
        }
    }
    match best {
        Some((_, rest)) => rest,
        None => String::new(),
    }
}

fn complete_smart(cur: &str, ctx: &CommandContext<'_>) -> String {
    let lower = cur.to_lowercase();
    for prefix in ["@player:", "@p:"] {
        if lower.starts_with(prefix) {
            let typed = cur.get(prefix.len()..).unwrap_or("");
            return complete_player(typed, PlayerPref::Enemy, ctx);
        }
    }
    let words: Vec<String> = ["@weakest", "@richest", "@player:"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    complete_word(cur, &words)
}

fn complete_player(typed: &str, pref: PlayerPref, ctx: &CommandContext<'_>) -> String {
    let game = ctx.game;
    let me: PlayerId = ctx.player;
    let typed = typed.trim_start();
    // Key: (tier, completion length, id).
    let mut best: Option<((u8, usize, PlayerId), String)> = None;
    for pl in game.players() {
        if pl.id == me || !pl.alive {
            continue;
        }
        let rest = match fuzzy::ci_prefix_rest(&pl.name, typed) {
            Some(r) => r,
            None => continue,
        };
        if rest.is_empty() {
            return String::new();
        }
        let allied = game.are_allied(me, pl.id);
        let tier: u8 = match pref {
            PlayerPref::Enemy => {
                if allied {
                    1
                } else {
                    0
                }
            }
            PlayerPref::AllyTarget => {
                if game.pending_proposal(pl.id, me) {
                    0
                } else if allied {
                    2
                } else {
                    1
                }
            }
            PlayerPref::UnallyTarget => {
                if allied {
                    0
                } else {
                    1
                }
            }
        };
        let key = (tier, rest.chars().count(), pl.id);
        let better = match &best {
            None => true,
            Some((bk, _)) => key < *bk,
        };
        if better {
            best = Some((key, rest));
        }
    }
    match best {
        Some((_, rest)) => rest,
        None => String::new(),
    }
}
