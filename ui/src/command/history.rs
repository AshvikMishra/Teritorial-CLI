//! Bounded command history with Up/Down navigation.
//!
//! Navigation keeps the line that was being typed when browsing started (the
//! draft) and gives it back when the user walks past the newest entry with
//! Down. Typical wiring:
//!
//! * Up: `if let Some(l) = history.up(&line) { line = l; }`
//! * Down: `if let Some(l) = history.down() { line = l; }`
//! * any edit of the line, Esc, or a submit: `history.reset_navigation()`
//!   (a successful submit calls [`History::push`], which also resets).

/// Default number of remembered lines.
pub const DEFAULT_HISTORY_SIZE: usize = 200;

/// Command history, oldest entry first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct History {
    entries: Vec<String>,
    max: usize,
    cursor: Option<usize>,
    draft: String,
}

impl Default for History {
    fn default() -> History {
        History::new(DEFAULT_HISTORY_SIZE)
    }
}

impl History {
    /// An empty history that keeps at most `max` lines (at least 1).
    pub fn new(max: usize) -> History {
        History {
            entries: Vec::new(),
            max: max.max(1),
            cursor: None,
            draft: String::new(),
        }
    }

    /// A history pre-filled with `entries` (oldest first), trimmed to `max`.
    pub fn with_entries(max: usize, entries: Vec<String>) -> History {
        let mut h = History::new(max);
        for e in entries {
            h.push(&e);
        }
        h
    }

    /// Appends a submitted line. Blank lines and an exact repeat of the newest
    /// entry are skipped. The oldest entries are dropped beyond the size limit.
    /// Always ends any navigation in progress.
    pub fn push(&mut self, line: &str) {
        self.reset_navigation();
        let t = line.trim();
        if t.is_empty() {
            return;
        }
        if self.entries.last().map(|l| l.as_str()) == Some(t) {
            return;
        }
        self.entries.push(t.to_string());
        self.trim_to_max();
    }

    fn trim_to_max(&mut self) {
        if self.entries.len() > self.max {
            let excess = self.entries.len() - self.max;
            self.entries = self.entries.split_off(excess);
            // Indices shifted; stop browsing rather than point at the wrong line.
            self.cursor = None;
        }
    }

    /// Moves to the next older entry and returns it. On the first call,
    /// `current` (the line being typed) is saved as the draft. Returns `None`
    /// when there is nothing older (the caller keeps its line unchanged).
    pub fn up(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let next = match self.cursor {
            None => {
                self.draft = current.to_string();
                self.entries.len() - 1
            }
            Some(0) => return None,
            Some(i) => i - 1,
        };
        self.cursor = Some(next);
        Some(self.entries[next].clone())
    }

    /// Moves to the next newer entry and returns it. Past the newest entry it
    /// returns the saved draft and ends navigation. Returns `None` when not
    /// navigating (the caller keeps its line unchanged).
    pub fn down(&mut self) -> Option<String> {
        let i = self.cursor?;
        if i + 1 < self.entries.len() {
            self.cursor = Some(i + 1);
            Some(self.entries[i + 1].clone())
        } else {
            self.cursor = None;
            Some(std::mem::take(&mut self.draft))
        }
    }

    /// Ends navigation and forgets the draft (call when the line is edited).
    pub fn reset_navigation(&mut self) {
        self.cursor = None;
        self.draft.clear();
    }

    /// Whether Up/Down browsing is in progress.
    pub fn is_navigating(&self) -> bool {
        self.cursor.is_some()
    }

    /// The newest entry: pass it as `CommandContext::last_line` for `!!`.
    pub fn last(&self) -> Option<&str> {
        self.entries.last().map(|s| s.as_str())
    }

    /// All entries, oldest first (for persisting).
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The size limit.
    pub fn max_size(&self) -> usize {
        self.max
    }

    /// Changes the size limit (at least 1), dropping the oldest entries if needed.
    pub fn set_max_size(&mut self, max: usize) {
        self.max = max.max(1);
        self.trim_to_max();
    }

    /// Removes every entry and ends navigation.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.reset_navigation();
    }
}
