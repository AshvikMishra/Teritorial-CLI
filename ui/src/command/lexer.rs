//! Tokenizer for the command line.
//!
//! A line is a chain of segments separated by `;`. A segment is a list of
//! whitespace-separated tokens. Double quotes group text into one token and
//! protect `;` from chaining (used by `alias name "a fr 40%; f es 10%"`). The
//! quote characters themselves are not part of a token's text.

/// One token of a command segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// The token text, with double quotes removed.
    pub text: String,
    /// Byte offset of the token's first character (or opening quote) in the segment.
    pub start: usize,
    /// Byte offset just past the token's last character (or closing quote) in the segment.
    pub end: usize,
}

/// Splits a line on `;` outside double quotes. Every part is trimmed; empty
/// parts are kept (callers skip them), so `"a fr;"` gives `["a fr", ""]`.
pub fn split_chain(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_quote = false;
    let mut start = 0usize;
    for (i, c) in line.char_indices() {
        if c == '"' {
            in_quote = !in_quote;
        } else if c == ';' && !in_quote {
            out.push(line[start..i].trim().to_string());
            start = i + 1;
        }
    }
    out.push(line[start..].trim().to_string());
    out
}

/// The raw text after the last `;` outside double quotes (the segment being
/// typed). Not trimmed, so trailing whitespace is preserved.
pub fn last_segment(line: &str) -> &str {
    let mut in_quote = false;
    let mut start = 0usize;
    for (i, c) in line.char_indices() {
        if c == '"' {
            in_quote = !in_quote;
        } else if c == ';' && !in_quote {
            start = i + 1;
        }
    }
    &line[start..]
}

/// Whether `s` has an unbalanced double quote.
pub fn has_open_quote(s: &str) -> bool {
    s.chars().filter(|&c| c == '"').count() % 2 == 1
}

/// Splits one segment into tokens on whitespace outside double quotes.
pub fn tokenize(segment: &str) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let mut cur = String::new();
    let mut start: Option<usize> = None;
    let mut in_quote = false;
    for (i, c) in segment.char_indices() {
        if c == '"' {
            in_quote = !in_quote;
            if start.is_none() {
                start = Some(i);
            }
            continue;
        }
        if c.is_whitespace() && !in_quote {
            if let Some(s) = start {
                out.push(Token {
                    text: std::mem::take(&mut cur),
                    start: s,
                    end: i,
                });
                start = None;
            }
            continue;
        }
        if start.is_none() {
            start = Some(i);
        }
        cur.push(c);
    }
    if let Some(s) = start {
        out.push(Token {
            text: cur,
            start: s,
            end: segment.len(),
        });
    }
    out
}
