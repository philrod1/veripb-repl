//! The hand-rolled line editor for the TUI prompt: cursor movement,
//! editing, and ↑/↓ history — the part of rustyline this REPL actually
//! needed. Keystrokes always go to the prompt; the editor knows nothing
//! about the screen beyond producing a visible window of its buffer.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub enum EditResult {
    /// Keystroke consumed (or ignored); nothing for the caller to do.
    None,
    /// Enter was pressed: here's the line, and the buffer is reset.
    Submitted(String),
    /// Ctrl-D on an empty line: the caller should exit.
    Quit,
}

pub struct LineEditor {
    /// `Vec<char>` rather than `String` so cursor movement and edits are
    /// simple indexing, not byte-boundary bookkeeping.
    buffer: Vec<char>,
    /// Cursor position in chars, 0..=buffer.len().
    cursor: usize,
    history: Vec<String>,
    /// `Some(i)` while browsing history entry `i`; `None` while editing a
    /// fresh line.
    history_pos: Option<usize>,
    /// The fresh line stashed away while browsing history, restored by
    /// pressing ↓ past the newest entry.
    stash: Vec<char>,
}

impl LineEditor {
    pub fn new() -> Self {
        LineEditor {
            buffer: Vec::new(),
            cursor: 0,
            history: Vec::new(),
            history_pos: None,
            stash: Vec::new(),
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> EditResult {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Ctrl *or* Alt turns ←/→ into a word jump. Two modifiers for the
        // same reason the mouse wheel accepts both (see `tui::run`):
        // terminals disagree about which of them they forward rather than
        // swallow, and between the two at least one arrives everywhere.
        let by_word = ctrl || key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('c') if ctrl => {
                self.buffer.clear();
                self.cursor = 0;
                self.history_pos = None;
            }
            KeyCode::Char('d') if ctrl => {
                if self.buffer.is_empty() {
                    return EditResult::Quit;
                }
                if self.cursor < self.buffer.len() {
                    self.buffer.remove(self.cursor);
                    self.history_pos = None;
                }
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.buffer.len(),
            KeyCode::Char('u') if ctrl => {
                if self.cursor > 0 {
                    self.history_pos = None;
                }
                self.buffer.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => {
                if self.cursor < self.buffer.len() {
                    self.history_pos = None;
                }
                self.buffer.truncate(self.cursor);
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.buffer.insert(self.cursor, c);
                self.cursor += 1;
                // Editing a recalled line ends the history walk — it's a
                // fresh line of its own now (see `browsing_history`).
                self.history_pos = None;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.buffer.remove(self.cursor);
                    self.history_pos = None;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.buffer.len() {
                    self.buffer.remove(self.cursor);
                    self.history_pos = None;
                }
            }
            KeyCode::Left if by_word => self.cursor = self.prev_word_start(),
            KeyCode::Right if by_word => self.cursor = self.next_word_start(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                if self.cursor < self.buffer.len() {
                    self.cursor += 1;
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.buffer.len(),
            KeyCode::Up => self.history_prev(),
            KeyCode::Down => self.history_next(),
            KeyCode::Enter => {
                let line: String = self.buffer.iter().collect();
                if !line.trim().is_empty()
                    && self.history.last().map(String::as_str) != Some(line.as_str())
                {
                    self.history.push(line.clone());
                }
                self.buffer.clear();
                self.cursor = 0;
                self.history_pos = None;
                return EditResult::Submitted(line);
            }
            _ => {}
        }
        EditResult::None
    }

    /// The start of the word at or before the cursor — Ctrl/Alt+← .
    /// Skips any whitespace immediately behind the cursor first, then the
    /// run of non-whitespace before that, so repeated presses step from
    /// word to word rather than stalling on the gap between two. Column 0
    /// once there's nothing left to skip.
    fn prev_word_start(&self) -> usize {
        let mut i = self.cursor;
        while i > 0 && self.buffer[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.buffer[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    /// The start of the next word after the cursor — Ctrl/Alt+→ . The
    /// mirror of [`Self::prev_word_start`]: skip the run of non-whitespace
    /// the cursor is sitting in, then the whitespace following it, landing
    /// on the next word's first character. Lands on the end of the line
    /// when there is no further word, so the key is never a dead no-op
    /// part-way along a line.
    ///
    /// "Start of the next word" rather than readline's "end of the next
    /// word": it pairs symmetrically with `prev_word_start`, so Ctrl+→
    /// followed by Ctrl+← returns the cursor where it began, and it is
    /// what Ctrl+arrow does in most editors.
    fn next_word_start(&self) -> usize {
        let mut i = self.cursor;
        while i < self.buffer.len() && !self.buffer[i].is_whitespace() {
            i += 1;
        }
        while i < self.buffer.len() && self.buffer[i].is_whitespace() {
            i += 1;
        }
        i
    }

    fn history_prev(&mut self) {
        let next = match self.history_pos {
            None if self.history.is_empty() => return,
            None => {
                self.stash = std::mem::take(&mut self.buffer);
                self.history.len() - 1
            }
            Some(0) => return,
            Some(i) => i - 1,
        };
        self.history_pos = Some(next);
        self.buffer = self.history[next].chars().collect();
        self.cursor = self.buffer.len();
    }

    fn history_next(&mut self) {
        match self.history_pos {
            None => {}
            Some(i) if i + 1 < self.history.len() => {
                self.history_pos = Some(i + 1);
                self.buffer = self.history[i + 1].chars().collect();
                self.cursor = self.buffer.len();
            }
            Some(_) => {
                self.history_pos = None;
                self.buffer = std::mem::take(&mut self.stash);
                self.cursor = self.buffer.len();
            }
        }
    }

    /// The whole buffer as typed so far — what completion and the
    /// suggestion strip filter on.
    pub fn text(&self) -> String {
        self.buffer.iter().collect()
    }

    /// Replace the buffer (cursor lands at the end) — how Tab completion
    /// writes its result back.
    pub fn set_text(&mut self, text: &str) {
        self.buffer = text.chars().collect();
        self.cursor = self.buffer.len();
        self.history_pos = None;
    }

    /// Move the cursor to a specific character position, clamped to the
    /// buffer's length — how the Proof pane's Vim-style insert mode
    /// (`tui::App`) positions the cursor after `set_text` pre-fills a
    /// line's existing content, instead of always landing at the end.
    pub fn set_cursor(&mut self, col: usize) {
        self.cursor = col.min(self.buffer.len());
    }

    /// The cursor's current character position — the Proof pane's Vim-
    /// style insert mode reads this at commit time (Esc/Enter) so
    /// `Normal` mode resumes with the cursor where typing left it,
    /// instead of always snapping back to column 0.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether ↑/↓ are currently walking history: true from a recall
    /// until the line is edited (which makes it a fresh line of its own),
    /// submitted, or ↓ steps past the newest entry. While true, the
    /// frontend suppresses suggestions so the arrows stay on history and
    /// the prompt doesn't move mid-walk.
    pub fn browsing_history(&self) -> bool {
        self.history_pos.is_some()
    }

    /// End a history walk without touching the buffer: the recalled text
    /// stays, but it's a fresh line of the user's own now. The frontend
    /// calls this on Tab, so completing a recalled line is one keystroke.
    pub fn end_history_walk(&mut self) {
        self.history_pos = None;
    }

    /// The window of the buffer that fits in `avail` columns, plus the
    /// cursor's offset within that window — scrolled horizontally so the
    /// cursor is always in view.
    pub fn view(&self, avail: usize) -> (String, usize) {
        if avail == 0 {
            return (String::new(), 0);
        }
        let start = if self.cursor >= avail {
            self.cursor + 1 - avail
        } else {
            0
        };
        let end = (start + avail).min(self.buffer.len());
        (
            self.buffer[start..end].iter().collect(),
            self.cursor - start,
        )
    }
}
