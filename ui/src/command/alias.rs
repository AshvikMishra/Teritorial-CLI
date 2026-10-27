//! `!!` repetition and user alias expansion.
//!
//! Expansion runs before parsing, segment by segment:
//!
//! * A segment whose first token is `!!` is replaced by the last submitted line
//!   (`CommandContext::last_line`); any text after `!!` is appended, so after
//!   `a fr` the line `!! 60%` runs `a fr 60%`. `!!` works inside chains.
//! * A segment whose first word is a user alias (and not a built-in verb or
//!   short form, which always win) is replaced by the alias body with `$1`..`$9`
//!   set to the arguments, `$*` to all of them and `$$` to a literal `$`. A body
//!   without placeholders gets the arguments appended. Missing `$n` arguments
//!   and surplus arguments are errors.
//! * Bodies may chain with `;` and may use other aliases or `!!`. Expansion is
//!   limited to [`MAX_ALIAS_DEPTH`] levels and [`MAX_SEGMENTS`] segments, so
//!   recursive aliases fail with an error instead of hanging.
//!
//! Alias definitions are validated by [`validate`]: names are 1 to 16
//! characters of `a-z 0-9 - _` starting with a letter, may not be a built-in
//! verb or short form, and every chained part of the body must start with a
//! known verb, another existing alias, `!!` or a `$` placeholder.

use std::collections::BTreeMap;

use super::lexer;
use super::parser::Verb;

/// Maximum nesting of alias / `!!` expansion.
pub const MAX_ALIAS_DEPTH: usize = 8;
/// Maximum number of command segments one line may expand to.
pub const MAX_SEGMENTS: usize = 32;
/// Maximum alias name length in characters.
pub const MAX_ALIAS_NAME_LEN: usize = 16;
/// Maximum alias body length in characters.
pub const MAX_ALIAS_BODY_LEN: usize = 200;

/// One command segment after expansion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpandedSegment {
    /// The expanded command text (or the text that failed to expand).
    pub text: String,
    /// Why expansion failed, if it did.
    pub error: Option<String>,
}

/// A fully expanded line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expanded {
    /// Every resulting segment, in execution order. Empty segments are dropped.
    pub segments: Vec<ExpandedSegment>,
    /// The line with top-level `!!` replaced (aliases kept as typed), segments
    /// joined by `"; "`. This is what history and `last_line` should store.
    pub history_line: String,
}

/// Checks an alias name (already lowercased by the caller).
pub fn validate_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return Err("alias name is empty".to_string()),
    };
    if !first.is_ascii_lowercase() {
        return Err(format!("alias name '{}' must start with a letter", name));
    }
    if name.chars().count() > MAX_ALIAS_NAME_LEN {
        return Err(format!(
            "alias name '{}' is too long (max {} characters)",
            name, MAX_ALIAS_NAME_LEN
        ));
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
        return Err(format!(
            "alias name '{}' may only use a-z, 0-9, - and _",
            name
        ));
    }
    if Verb::from_exact(name).is_some() {
        return Err(format!(
            "'{}' is a built-in command and cannot be redefined",
            name
        ));
    }
    Ok(())
}

/// Validates a complete alias definition against the existing aliases.
pub fn validate(name: &str, body: &str, aliases: &BTreeMap<String, String>) -> Result<(), String> {
    validate_name(name)?;
    let body_t = body.trim();
    if body_t.is_empty() {
        return Err(format!("alias {} needs a command", name));
    }
    if body_t.chars().count() > MAX_ALIAS_BODY_LEN {
        return Err(format!(
            "alias body is too long (max {} characters)",
            MAX_ALIAS_BODY_LEN
        ));
    }
    if lexer::has_open_quote(body_t) {
        return Err("alias body has an unclosed quote".to_string());
    }
    for seg in lexer::split_chain(body_t) {
        if seg.is_empty() {
            continue;
        }
        let toks = lexer::tokenize(&seg);
        let first = match toks.first() {
            Some(t) => t.text.to_lowercase(),
            None => continue,
        };
        if first == name {
            return Err(format!("alias {} cannot call itself", name));
        }
        if first == "!!" || first.starts_with('$') {
            continue;
        }
        if Verb::lookup(&first).is_some() {
            continue;
        }
        if aliases.contains_key(first.as_str()) {
            continue;
        }
        return Err(format!("unknown command '{}' in alias body", first));
    }
    Ok(())
}

/// Removes one pair of surrounding double quotes, if present, and trims.
pub fn strip_quotes(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].trim().to_string()
    } else {
        t.to_string()
    }
}

fn quote_arg(a: &str) -> String {
    if a.chars().any(|c| c.is_whitespace() || c == ';') {
        format!("\"{}\"", a)
    } else {
        a.to_string()
    }
}

fn join_args(args: &[String]) -> String {
    let parts: Vec<String> = args.iter().map(|a| quote_arg(a)).collect();
    parts.join(" ")
}

/// Substitutes `$1`..`$9`, `$*` and `$$` in an alias body. Without any
/// placeholder, the arguments are appended.
pub fn substitute(body: &str, args: &[String]) -> Result<String, String> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len() + 16);
    let mut max_ref: usize = 0;
    let mut star = false;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '$' && i + 1 < chars.len() {
            let n = chars[i + 1];
            if n == '*' {
                star = true;
                out.push_str(&join_args(args));
                i += 2;
                continue;
            }
            if n == '$' {
                out.push('$');
                i += 2;
                continue;
            }
            if let Some(d) = n.to_digit(10) {
                if d >= 1 {
                    let k = d as usize;
                    if k > max_ref {
                        max_ref = k;
                    }
                    match args.get(k - 1) {
                        Some(a) => out.push_str(&quote_arg(a)),
                        None => return Err(format!("missing argument ${}", k)),
                    }
                    i += 2;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    if !star {
        if max_ref == 0 {
            if !args.is_empty() {
                out.push(' ');
                out.push_str(&join_args(args));
            }
        } else if args.len() > max_ref {
            return Err(format!(
                "too many arguments (takes {})",
                max_ref
            ));
        }
    }
    Ok(out)
}

/// Expands `!!` and aliases in a whole line. See the module docs.
pub fn expand_line(
    line: &str,
    aliases: &BTreeMap<String, String>,
    last_line: Option<&str>,
) -> Expanded {
    let mut segments: Vec<ExpandedSegment> = Vec::new();
    expand_text(line, aliases, last_line, 0, &mut segments);

    let mut parts: Vec<String> = Vec::new();
    for seg in lexer::split_chain(line) {
        if seg.is_empty() {
            continue;
        }
        let toks = lexer::tokenize(&seg);
        if let Some(first) = toks.first() {
            if first.text == "!!" {
                if let Some(last) = last_line {
                    let rest: &str = seg.get(first.end..).unwrap_or("");
                    parts.push(format!("{}{}", last.trim(), rest));
                    continue;
                }
            }
        }
        parts.push(seg);
    }
    Expanded {
        segments,
        history_line: parts.join("; "),
    }
}

fn push_error(out: &mut Vec<ExpandedSegment>, text: &str, error: String) {
    out.push(ExpandedSegment {
        text: text.to_string(),
        error: Some(error),
    });
}

fn expand_text(
    text: &str,
    aliases: &BTreeMap<String, String>,
    last_line: Option<&str>,
    depth: usize,
    out: &mut Vec<ExpandedSegment>,
) {
    for seg in lexer::split_chain(text) {
        if seg.is_empty() {
            continue;
        }
        if out.len() >= MAX_SEGMENTS {
            push_error(
                out,
                &seg,
                format!("too many chained commands (max {})", MAX_SEGMENTS),
            );
            return;
        }
        if depth > MAX_ALIAS_DEPTH {
            push_error(
                out,
                &seg,
                "alias expansion is too deep (recursive alias?)".to_string(),
            );
            return;
        }
        let toks = lexer::tokenize(&seg);
        let first = match toks.first() {
            Some(t) => t,
            None => continue,
        };
        if first.text == "!!" {
            match last_line {
                Some(last) => {
                    let rest: &str = seg.get(first.end..).unwrap_or("");
                    let replaced = format!("{}{}", last.trim(), rest);
                    expand_text(&replaced, aliases, last_line, depth + 1, out);
                }
                None => push_error(out, &seg, "no previous command to repeat".to_string()),
            }
            continue;
        }
        let lower = first.text.to_lowercase();
        if Verb::from_exact(&lower).is_none() {
            if let Some(body) = aliases.get(lower.as_str()) {
                let args: Vec<String> = toks[1..].iter().map(|t| t.text.clone()).collect();
                match substitute(body, &args) {
                    Ok(expanded) => expand_text(&expanded, aliases, last_line, depth + 1, out),
                    Err(e) => push_error(
                        out,
                        &seg,
                        format!("{}: {}  ({} = {})", lower, e, lower, body),
                    ),
                }
                continue;
            }
        }
        out.push(ExpandedSegment {
            text: seg,
            error: None,
        });
    }
}
