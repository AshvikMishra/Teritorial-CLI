//! The single-line editor behind the `> ` prompt. The cursor is a character
//! index (not a byte index), so multi-byte input is edited safely.

/// The command line being typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prompt {
    text: String,
    cursor: usize,
}

impl Prompt {
    /// An empty prompt.
    pub fn new() -> Prompt {
        Prompt {
            text: String::new(),
            cursor: 0,
        }
    }

    /// The current text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The cursor position in characters.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether the line is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Number of characters.
    pub fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    /// Whether the cursor is at the end of the line.
    pub fn at_end(&self) -> bool {
        self.cursor >= self.len_chars()
    }

    /// Clears the line.
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Replaces the line and moves the cursor to the end.
    pub fn set(&mut self, text: String) {
        self.cursor = text.chars().count();
        self.text = text;
    }

    /// Inserts a character at the cursor.
    pub fn insert(&mut self, c: char) {
        let b = self.byte_index(self.cursor);
        self.text.insert(b, c);
        self.cursor += 1;
    }

    /// Deletes the character before the cursor. Returns whether anything changed.
    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let b = self.byte_index(self.cursor - 1);
        self.text.remove(b);
        self.cursor -= 1;
        true
    }

    /// Deletes the character under the cursor. Returns whether anything changed.
    pub fn delete(&mut self) -> bool {
        if self.cursor >= self.len_chars() {
            return false;
        }
        let b = self.byte_index(self.cursor);
        self.text.remove(b);
        true
    }

    /// Deletes the word before the cursor (Ctrl+W). Returns whether anything changed.
    pub fn delete_word(&mut self) -> bool {
        let chars: Vec<char> = self.text.chars().collect();
        let mut start = self.cursor.min(chars.len());
        let end = start;
        while start > 0 && chars[start - 1] == ' ' {
            start -= 1;
        }
        while start > 0 && chars[start - 1] != ' ' {
            start -= 1;
        }
        if start == end {
            return false;
        }
        let mut out = String::with_capacity(self.text.len());
        for (i, c) in chars.iter().enumerate() {
            if i < start || i >= end {
                out.push(*c);
            }
        }
        self.text = out;
        self.cursor = start;
        true
    }

    /// Moves the cursor one character left.
    pub fn left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Moves the cursor one character right.
    pub fn right(&mut self) {
        if self.cursor < self.len_chars() {
            self.cursor += 1;
        }
    }

    /// Moves the cursor to the start.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// Moves the cursor to the end.
    pub fn end(&mut self) {
        self.cursor = self.len_chars();
    }

    fn byte_index(&self, char_idx: usize) -> usize {
        match self.text.char_indices().nth(char_idx) {
            Some((b, _)) => b,
            None => self.text.len(),
        }
    }
}
