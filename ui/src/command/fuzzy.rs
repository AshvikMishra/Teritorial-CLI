//! Forgiving name resolution for regions and players.
//!
//! Region lookup runs these stages in order and stops at the first stage that
//! finds anything:
//!
//! 1. exact region code, case-insensitive (`fr`, `US-W`);
//! 2. exact name, ignoring case, spaces and punctuation (`south-africa`);
//! 3. prefix of a code or name, also ignoring punctuation (`fra`, `us-s`, `usw`);
//! 4. prefix of any word of a name, or the name's initials (`korea`, `nz`);
//! 5. subsequence of a name, for 3+ characters (`swtz`);
//! 6. Damerau-Levenshtein distance of at most 1 (queries up to 4 characters)
//!    or 2 (longer queries) to a name or one of its words (`frnace`).
//!
//! When a stage finds several regions and exactly one of them is relevant to
//! the verb (attackable for `attack`, owned for `fortify`/`recall`, owned or
//! attackable for `info`/`focus`), that one is chosen, so `a br` picks the
//! Brazil region you actually border. Otherwise the result is an error that
//! lists up to three candidates, relevant ones first.
//!
//! One exception to "first stage wins": if stage 3 finds only regions that are
//! not relevant to the verb, stage 4 is tried too and wins when it finds a
//! relevant region (`a russia` picks the Russian region you border rather than
//! the only name starting with "russia"). The looser stages 5 and 6 never
//! override a prefix match.

use territorial_core::{Game, PlayerId, RegionId};

/// Which regions count as "what the player probably means" when a name is ambiguous.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relevance {
    /// No preference: any ambiguity is an error.
    Any,
    /// Regions the player can attack right now.
    Attackable,
    /// Regions the player owns.
    Owned,
    /// Regions the player owns or can attack.
    Nearby,
}

impl Relevance {
    /// Whether region `r` is relevant for `player` (always false for `Any`).
    pub fn matches(self, game: &Game, player: PlayerId, r: RegionId) -> bool {
        match self {
            Relevance::Any => false,
            Relevance::Attackable => game.is_attackable_by(player, r),
            Relevance::Owned => game.region_state(r).owner == Some(player),
            Relevance::Nearby => {
                game.region_state(r).owner == Some(player) || game.is_attackable_by(player, r)
            }
        }
    }
}

/// Lowercases `s` and keeps only alphanumeric characters (`"Guinea-Bissau"` -> `"guineabissau"`).
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_alphanumeric() {
            for l in c.to_lowercase() {
                out.push(l);
            }
        }
    }
    out
}

/// The normalized words of `s` (`"Papua New Guinea"` -> `["papua", "new", "guinea"]`).
pub fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(normalize)
        .collect()
}

/// Optimal-string-alignment Damerau-Levenshtein distance between two strings,
/// counted in characters.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let n = a.len();
    let m = b.len();
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let w = m + 1;
    let mut d: Vec<usize> = vec![0; (n + 1) * w];
    for i in 0..=n {
        d[i * w] = i;
    }
    for (j, cell) in d.iter_mut().take(m + 1).enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let mut v = (d[(i - 1) * w + j] + 1)
                .min(d[i * w + j - 1] + 1)
                .min(d[(i - 1) * w + j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[(i - 2) * w + j - 2] + 1);
            }
            d[i * w + j] = v;
        }
    }
    d[n * w + m]
}

/// Whether every character of `needle` appears in `hay` in order.
pub fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    for c in needle.chars() {
        let mut found = false;
        for h in it.by_ref() {
            if h == c {
                found = true;
                break;
            }
        }
        if !found {
            return false;
        }
    }
    true
}

/// If `typed` is a case-insensitive prefix of `full` (compared character by
/// character), returns the rest of `full` in its original case.
pub fn ci_prefix_rest(full: &str, typed: &str) -> Option<String> {
    let mut fi = full.chars();
    for t in typed.chars() {
        let f = fi.next()?;
        if !f.to_lowercase().eq(t.to_lowercase()) {
            return None;
        }
    }
    Some(fi.collect())
}

/// The candidate closest to `query` by edit distance, if within `max`. Ties go
/// to the earlier candidate. Queries shorter than 2 characters match nothing.
pub fn closest(query: &str, candidates: &[String], max: usize) -> Option<String> {
    let q = query.to_lowercase();
    if q.chars().count() < 2 {
        return None;
    }
    let mut best: Option<(usize, usize)> = None;
    for (i, c) in candidates.iter().enumerate() {
        let d = edit_distance(&q, &c.to_lowercase());
        if d > max {
            continue;
        }
        let better = match best {
            None => true,
            Some((bd, _)) => d < bd,
        };
        if better {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| candidates[i].clone())
}

/// `"code (Name)"` for messages.
pub fn region_desc(game: &Game, r: RegionId) -> String {
    let info = game.map().region(r);
    format!("{} ({})", info.code, info.name)
}

fn ambiguous_message(shown: &str, items: Vec<String>) -> String {
    let n = items.len();
    let listed: Vec<String> = items.into_iter().take(3).collect();
    let mut msg = format!("'{}' is ambiguous: {}", shown, listed.join(", "));
    if n > 3 {
        msg.push_str(&format!(" (+{} more)", n - 3));
    }
    msg
}

fn pick_region(
    game: &Game,
    player: PlayerId,
    shown: &str,
    found: Vec<RegionId>,
    rel: Relevance,
) -> Result<RegionId, String> {
    if found.len() == 1 {
        return Ok(found[0]);
    }
    let relevant: Vec<RegionId> = found
        .iter()
        .copied()
        .filter(|&r| rel.matches(game, player, r))
        .collect();
    if relevant.len() == 1 {
        return Ok(relevant[0]);
    }
    let mut ordered: Vec<RegionId> = relevant.clone();
    for &r in &found {
        if !relevant.contains(&r) {
            ordered.push(r);
        }
    }
    let items: Vec<String> = ordered.iter().map(|&r| region_desc(game, r)).collect();
    Err(ambiguous_message(shown, items))
}

/// Whether `rel` is a real preference and at least one of `found` satisfies it.
/// Always true for [`Relevance::Any`], so a match with no preference is final.
fn any_relevant(game: &Game, player: PlayerId, found: &[RegionId], rel: Relevance) -> bool {
    rel == Relevance::Any || found.iter().any(|&r| rel.matches(game, player, r))
}

/// Resolves a typed region reference (code, name, prefix, typo...) to a region.
/// See the module docs for the stages. Errors are short human-readable reasons.
pub fn resolve_region(
    game: &Game,
    player: PlayerId,
    query: &str,
    rel: Relevance,
) -> Result<RegionId, String> {
    let shown = query.trim();
    if shown.is_empty() {
        return Err("missing region".to_string());
    }
    let map = game.map();
    let lower = shown.to_lowercase();
    // 1. exact code
    if let Some(r) = map.find_code(&lower) {
        return Ok(r);
    }
    let q = normalize(shown);
    if q.is_empty() {
        return Err(format!("unknown region '{}'", shown));
    }
    let qlen = q.chars().count();
    let mut found: Vec<RegionId> = Vec::new();

    // 2. exact name, ignoring case and punctuation
    for info in &map.regions {
        if normalize(&info.name) == q {
            found.push(info.id);
        }
    }
    if !found.is_empty() {
        return pick_region(game, player, shown, found, rel);
    }

    // Stages 3 and 4 are prefix matches. A stage whose matches contain no
    // region relevant to the verb does not end the search: the next prefix
    // stage may find the relevant one (`a russia` -> the Russian region you
    // border, not Russian Far East). If neither stage has a relevant match,
    // the first non-empty one decides (and its error is reported).
    let mut fallback: Option<Vec<RegionId>> = None;

    // 3. prefix of code or name (includes a code without punctuation, e.g. "usw")
    for info in &map.regions {
        if info.code.to_lowercase().starts_with(lower.as_str())
            || normalize(&info.code).starts_with(q.as_str())
            || normalize(&info.name).starts_with(q.as_str())
        {
            found.push(info.id);
        }
    }
    if !found.is_empty() {
        if any_relevant(game, player, &found, rel) {
            return pick_region(game, player, shown, found, rel);
        }
        fallback = Some(std::mem::take(&mut found));
    }

    // 4. word prefix or initials
    for info in &map.regions {
        let ws = words(&info.name);
        let initials: String = ws.iter().filter_map(|w| w.chars().next()).collect();
        let word_hit = ws.iter().any(|w| w.starts_with(q.as_str()));
        let initials_hit = qlen >= 2 && ws.len() >= 2 && initials == q;
        if word_hit || initials_hit {
            found.push(info.id);
        }
    }
    if !found.is_empty() && (fallback.is_none() || any_relevant(game, player, &found, rel)) {
        return pick_region(game, player, shown, found, rel);
    }
    if let Some(earlier) = fallback {
        return pick_region(game, player, shown, earlier, rel);
    }

    // 5. subsequence
    if qlen >= 3 {
        for info in &map.regions {
            if is_subsequence(&q, &normalize(&info.name)) {
                found.push(info.id);
            }
        }
        if !found.is_empty() {
            return pick_region(game, player, shown, found, rel);
        }
    }

    // 6. edit distance
    if qlen >= 3 {
        let limit: usize = if qlen <= 4 { 1 } else { 2 };
        let mut best = usize::MAX;
        for info in &map.regions {
            let mut d = edit_distance(&q, &normalize(&info.name));
            for w in words(&info.name) {
                if w.chars().count() >= 3 {
                    d = d.min(edit_distance(&q, &w));
                }
            }
            if d > limit {
                continue;
            }
            if d < best {
                best = d;
                found.clear();
            }
            if d == best {
                found.push(info.id);
            }
        }
        if !found.is_empty() {
            return pick_region(game, player, shown, found, rel);
        }
    }

    Err(format!("unknown region '{}'", shown))
}

/// Resolves a typed player reference: `Game::find_player` (exact name, then
/// unique prefix), then a player number (`3`), then normalized name, prefix,
/// subsequence and edit distance <= 2. When several match, the single alive
/// player other than `me` among them wins; otherwise it is an error listing
/// up to three names.
pub fn resolve_player(game: &Game, me: PlayerId, query: &str) -> Result<PlayerId, String> {
    let shown = query.trim();
    if shown.is_empty() {
        return Err("missing player name".to_string());
    }
    if let Some(p) = game.find_player(shown) {
        return Ok(p);
    }
    if shown.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(n) = shown.parse::<u32>() {
            if (n as usize) < game.players().len() {
                return Ok(n as PlayerId);
            }
        }
        return Err(format!("no player number {}", shown));
    }
    let q = normalize(shown);
    if q.is_empty() {
        return Err(format!("unknown player '{}'", shown));
    }
    let qlen = q.chars().count();
    let mut found: Vec<PlayerId> = Vec::new();
    for pl in game.players() {
        if normalize(&pl.name) == q {
            found.push(pl.id);
        }
    }
    if found.is_empty() {
        for pl in game.players() {
            if normalize(&pl.name).starts_with(q.as_str()) {
                found.push(pl.id);
            }
        }
    }
    if found.is_empty() && qlen >= 2 {
        for pl in game.players() {
            if is_subsequence(&q, &normalize(&pl.name)) {
                found.push(pl.id);
            }
        }
    }
    if found.is_empty() && qlen >= 3 {
        let mut best = usize::MAX;
        for pl in game.players() {
            let d = edit_distance(&q, &normalize(&pl.name));
            if d > 2 {
                continue;
            }
            if d < best {
                best = d;
                found.clear();
            }
            if d == best {
                found.push(pl.id);
            }
        }
    }
    match found.len() {
        0 => Err(format!("unknown player '{}'", shown)),
        1 => Ok(found[0]),
        _ => {
            let alive: Vec<PlayerId> = found
                .iter()
                .copied()
                .filter(|&p| p != me && game.is_alive(p))
                .collect();
            if alive.len() == 1 {
                return Ok(alive[0]);
            }
            let mut ordered: Vec<PlayerId> = alive.clone();
            for &p in &found {
                if !alive.contains(&p) {
                    ordered.push(p);
                }
            }
            let names: Vec<String> = ordered
                .iter()
                .map(|&p| match game.get_player(p) {
                    Some(pl) => pl.name.clone(),
                    None => format!("Player {}", p),
                })
                .collect();
            Err(ambiguous_message(shown, names))
        }
    }
}
