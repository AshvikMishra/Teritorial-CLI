//! Small text helpers: name normalization and edit distance for suggestions.

/// Lowercases, turns `-` and `_` into spaces, drops `.` and `'`, and collapses
/// whitespace, so "South-Africa" and "south  africa" compare equal.
pub fn normalize(s: &str) -> String {
    let mut cleaned = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '-' | '_' => cleaned.push(' '),
            '.' | '\'' => {}
            other => {
                for low in other.to_lowercase() {
                    cleaned.push(low);
                }
            }
        }
    }
    cleaned.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Levenshtein distance between two strings, counted in chars.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let deletion = prev[j] + 1;
            let insertion = cur[j - 1] + 1;
            let substitution = prev[j - 1] + cost;
            cur[j] = deletion.min(insertion).min(substitution);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Formats seconds as `m:ss`, or `h:mm:ss` from one hour on.
pub fn clock(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}
