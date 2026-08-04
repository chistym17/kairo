//! Multiline composer state: growth, cursor, and input history.

use super::app::ComposerState;

pub const MIN_HEIGHT: u16 = 5;
pub const MAX_HEIGHT: u16 = 10;
const HISTORY_CAP: usize = 50;

impl ComposerState {
    pub fn new(placeholder: impl Into<String>) -> Self {
        Self {
            text: String::new(),
            placeholder: placeholder.into(),
            height: MIN_HEIGHT,
            cursor: 0,
            scroll: 0,
            history: Vec::new(),
            history_index: None,
            draft_backup: String::new(),
        }
    }

    /// Recompute visible height from content (5..=10 rows; borders+padding included).
    pub fn reflow(&mut self, inner_width: u16) {
        let lines = visible_line_count(&self.text, inner_width.max(1));
        // +2 for top/bottom padding inside the bordered box.
        let needed = (lines as u16).saturating_add(2);
        self.height = needed.clamp(MIN_HEIGHT, MAX_HEIGHT);
        self.ensure_cursor_visible(inner_width.max(1));
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn insert_char(&mut self, c: char) {
        let byte = self.byte_index();
        self.text.insert(byte, c);
        self.cursor += 1;
        self.history_index = None;
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_index();
        self.cursor -= 1;
        let start = self.byte_index();
        self.text.replace_range(start..end, "");
        self.history_index = None;
    }

    pub fn delete_forward(&mut self) {
        if self.cursor >= self.text.chars().count() {
            return;
        }
        let start = self.byte_index();
        let mut chars = self.text[start..].char_indices();
        let _ = chars.next();
        let end = match chars.next() {
            Some((i, _)) => start + i,
            None => self.text.len(),
        };
        self.text.replace_range(start..end, "");
        self.history_index = None;
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.text.chars().count() {
            self.cursor += 1;
        }
    }

    pub fn move_home(&mut self) {
        let (row, _) = self.cursor_row_col(80);
        let line_starts = line_start_char_indices(&self.text);
        self.cursor = line_starts.get(row).copied().unwrap_or(0);
    }

    pub fn move_end(&mut self) {
        let (row, _) = self.cursor_row_col(80);
        let ranges = line_char_ranges(&self.text);
        if let Some((start, end)) = ranges.get(row) {
            self.cursor = *end;
            let _ = start;
        }
    }

    /// Up: move within multiline text, or step history when on the first line / empty.
    pub fn up(&mut self) {
        if self.text.contains('\n') {
            let (row, col) = self.cursor_row_col(10_000);
            if row > 0 {
                self.set_cursor_row_col(row - 1, col);
                return;
            }
        }
        self.history_prev();
    }

    /// Down: move within multiline text, or step history forward.
    pub fn down(&mut self) {
        if self.text.contains('\n') {
            let (row, col) = self.cursor_row_col(10_000);
            let rows = self.text.split('\n').count();
            if row + 1 < rows {
                self.set_cursor_row_col(row + 1, col);
                return;
            }
        }
        self.history_next();
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.draft_backup = self.text.clone();
                let idx = self.history.len() - 1;
                self.history_index = Some(idx);
                self.set_text(self.history[idx].clone());
            }
            Some(0) => {}
            Some(i) => {
                let idx = i - 1;
                self.history_index = Some(idx);
                self.set_text(self.history[idx].clone());
            }
        }
    }

    pub fn history_next(&mut self) {
        let Some(i) = self.history_index else {
            return;
        };
        if i + 1 < self.history.len() {
            let idx = i + 1;
            self.history_index = Some(idx);
            self.set_text(self.history[idx].clone());
        } else {
            self.history_index = None;
            let draft = std::mem::take(&mut self.draft_backup);
            self.set_text(draft);
        }
    }

    pub fn push_history(&mut self, entry: String) {
        if entry.trim().is_empty() {
            return;
        }
        if self.history.last().map(String::as_str) == Some(entry.as_str()) {
            return;
        }
        self.history.push(entry);
        if self.history.len() > HISTORY_CAP {
            self.history.remove(0);
        }
        self.history_index = None;
        self.draft_backup.clear();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.history_index = None;
        self.draft_backup.clear();
        self.height = MIN_HEIGHT;
    }

    pub fn take_for_send(&mut self) -> Option<String> {
        let line = self.text.trim().to_string();
        if line.is_empty() {
            return None;
        }
        self.push_history(line.clone());
        self.clear();
        Some(line)
    }

    pub fn set_text(&mut self, text: String) {
        self.cursor = text.chars().count();
        self.text = text;
        self.scroll = 0;
    }

    pub fn cursor_row_col(&self, _wrap_width: u16) -> (usize, usize) {
        // Logical lines (Shift+Enter), not soft-wrapped display lines.
        let mut row = 0usize;
        let mut col = 0usize;
        for (i, ch) in self.text.chars().enumerate() {
            if i == self.cursor {
                return (row, col);
            }
            if ch == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (row, col)
    }

    fn set_cursor_row_col(&mut self, row: usize, col: usize) {
        let ranges = line_char_ranges(&self.text);
        let Some((start, end)) = ranges.get(row).copied() else {
            return;
        };
        let line_len = end.saturating_sub(start);
        self.cursor = start + col.min(line_len);
    }

    fn byte_index(&self) -> usize {
        self.text
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

    fn ensure_cursor_visible(&mut self, _inner_width: u16) {
        let (row, _) = self.cursor_row_col(10_000);
        // Content rows after borders (2) and vertical padding (2).
        let inner_rows = self.height.saturating_sub(4).max(1) as usize;
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + inner_rows {
            self.scroll = row + 1 - inner_rows;
        }
    }
}

fn visible_line_count(text: &str, _inner_width: u16) -> usize {
    if text.is_empty() {
        return 1;
    }
    text.split('\n').count().max(1)
}

fn line_start_char_indices(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, ch) in text.chars().enumerate() {
        if ch == '\n' {
            starts.push(i + 1);
        }
    }
    starts
}

fn line_char_ranges(text: &str) -> Vec<(usize, usize)> {
    let starts = line_start_char_indices(text);
    let total = text.chars().count();
    let mut ranges = Vec::with_capacity(starts.len());
    for (i, start) in starts.iter().enumerate() {
        let end = if i + 1 < starts.len() {
            // Exclude the newline itself from the editable end of the line.
            starts[i + 1] - 1
        } else {
            total
        };
        ranges.push((*start, end));
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grow_with_newlines() {
        let mut c = ComposerState::new("…");
        c.reflow(40);
        assert_eq!(c.height, MIN_HEIGHT);
        c.insert_newline();
        c.insert_newline();
        c.insert_newline();
        c.insert_newline();
        c.reflow(40);
        assert!(c.height > MIN_HEIGHT);
        for _ in 0..20 {
            c.insert_newline();
        }
        c.reflow(40);
        assert_eq!(c.height, MAX_HEIGHT);
    }

    #[test]
    fn history_round_trip() {
        let mut c = ComposerState::new("…");
        c.set_text("first".into());
        let sent = c.take_for_send().unwrap();
        assert_eq!(sent, "first");
        assert!(c.is_empty());
        c.history_prev();
        assert_eq!(c.text, "first");
        c.history_next();
        assert!(c.is_empty());
    }

    #[test]
    fn shift_enter_style_multiline_then_send() {
        let mut c = ComposerState::new("…");
        c.insert_char('a');
        c.insert_newline();
        c.insert_char('b');
        assert_eq!(c.text, "a\nb");
        let sent = c.take_for_send().unwrap();
        assert_eq!(sent, "a\nb");
    }
}
