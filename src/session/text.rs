//! The session's formula and proof lines, behind types that let them be read
//! as slices but only changed through methods that record the change, so the
//! caches derived from them know what to drop.

use std::cell::Cell;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::checker;

/// The proof lines. Reads as a `[String]`; every change goes through a
/// method here, which lowers the low-water mark (see
/// [`Self::take_changed_from`]).
#[derive(Clone)]
pub struct ProofBuffer {
    lines: Vec<String>,
    /// The lowest index changed since [`Self::take_changed_from`] last ran;
    /// `usize::MAX` if none.
    changed_from: Cell<usize>,
    /// [`Self::longest_chars`], or `None` until it is next asked for.
    longest: Cell<Option<usize>>,
}

impl ProofBuffer {
    fn changed(&self, idx: usize) {
        self.changed_from.set(self.changed_from.get().min(idx));
    }

    /// Records that `line` was added.
    fn added(&self, line: &str) {
        if let Some(longest) = self.longest.get() {
            self.longest.set(Some(longest.max(line.chars().count())));
        }
    }

    /// Records that `line` was removed.
    fn removed(&self, line: &str) {
        if self.longest.get() == Some(line.chars().count()) {
            self.longest.set(None);
        }
    }

    /// Returns the length in characters of the longest line (0 if none).
    pub fn longest_chars(&self) -> usize {
        if let Some(longest) = self.longest.get() {
            return longest;
        }
        let longest = self
            .lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        self.longest.set(Some(longest));
        longest
    }

    /// Returns the lowest index changed since the last call, if any, and
    /// resets the mark. Lines before it are unchanged since then.
    pub fn take_changed_from(&self) -> Option<usize> {
        let idx = self.changed_from.replace(usize::MAX);
        (idx != usize::MAX).then_some(idx)
    }

    pub fn push(&mut self, line: String) {
        self.changed(self.lines.len());
        self.added(&line);
        self.lines.push(line);
    }

    pub fn pop(&mut self) -> Option<String> {
        let line = self.lines.pop()?;
        self.changed(self.lines.len());
        self.removed(&line);
        Some(line)
    }

    pub fn insert(&mut self, idx: usize, line: String) {
        self.changed(idx);
        self.added(&line);
        self.lines.insert(idx, line);
    }

    pub fn remove(&mut self, idx: usize) -> String {
        self.changed(idx);
        let line = self.lines.remove(idx);
        self.removed(&line);
        line
    }

    /// Replaces line `idx`, returning the old text.
    pub fn set(&mut self, idx: usize, line: String) -> String {
        if self.lines[idx] != line {
            self.changed(idx);
        }
        self.added(&line);
        let old = std::mem::replace(&mut self.lines[idx], line);
        self.removed(&old);
        old
    }

    pub fn clear(&mut self) {
        self.changed(0);
        self.longest.set(None);
        self.lines.clear();
    }

    pub fn extend(&mut self, lines: impl IntoIterator<Item = String>) {
        self.changed(self.lines.len());
        self.longest.set(None);
        self.lines.extend(lines);
    }

    /// Replaces every line, returning the old ones. Lines before the first
    /// difference count as unchanged.
    pub fn replace(&mut self, lines: Vec<String>) -> Vec<String> {
        let common = self
            .lines
            .iter()
            .zip(&lines)
            .take_while(|(old, new)| old == new)
            .count();
        if common < self.lines.len().max(lines.len()) {
            self.changed(common);
        }
        self.longest.set(None);
        std::mem::replace(&mut self.lines, lines)
    }
}

impl Deref for ProofBuffer {
    type Target = [String];

    fn deref(&self) -> &[String] {
        &self.lines
    }
}

impl Default for ProofBuffer {
    fn default() -> Self {
        Vec::new().into()
    }
}

impl From<Vec<String>> for ProofBuffer {
    fn from(lines: Vec<String>) -> Self {
        ProofBuffer {
            lines,
            changed_from: Cell::new(usize::MAX),
            longest: Cell::new(None),
        }
    }
}

/// Source of [`FormulaLines::epoch`] values, so no two formulas in the
/// process share one.
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(0);

fn next_epoch() -> u64 {
    NEXT_EPOCH.fetch_add(1, Ordering::Relaxed)
}

/// The formula's constraint lines, verbatim (including any leading
/// `@label`s), with each line's first constraint ID. Reads as a `[String]`;
/// every change goes through a method here, which changes [`Self::epoch`].
#[derive(Clone)]
pub struct FormulaLines {
    lines: Vec<String>,
    /// The first constraint ID of each line (see
    /// `checker::parse::formula_line_constraints`), then one past the last.
    first_ids: Vec<usize>,
    epoch: u64,
}

impl FormulaLines {
    pub fn new(lines: Vec<String>) -> Self {
        let mut formula = FormulaLines {
            lines,
            first_ids: Vec::new(),
            epoch: next_epoch(),
        };
        formula.recount();
        formula
    }

    fn recount(&mut self) {
        let mut next = 1;
        self.first_ids = Vec::with_capacity(self.lines.len() + 1);
        for line in &self.lines {
            self.first_ids.push(next);
            next += checker::parse::formula_line_constraints(line);
        }
        self.first_ids.push(next);
    }

    /// Identifies these lines: different whenever they may differ, including
    /// from every other `FormulaLines` in the process.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Returns the first constraint ID of each line, in order.
    pub fn first_ids(&self) -> &[usize] {
        &self.first_ids[..self.lines.len()]
    }

    /// Returns the number of constraints veripb loads from these lines.
    pub fn constraint_count(&self) -> usize {
        self.first_ids[self.lines.len()] - 1
    }

    /// Replaces line `idx`, returning the old text.
    pub fn set(&mut self, idx: usize, line: String) -> String {
        let old = std::mem::replace(&mut self.lines[idx], line);
        self.recount();
        self.epoch = next_epoch();
        old
    }

    /// Replaces every line, returning the old ones.
    pub fn replace(&mut self, lines: Vec<String>) -> Vec<String> {
        let old = std::mem::replace(&mut self.lines, lines);
        self.recount();
        self.epoch = next_epoch();
        old
    }
}

impl Deref for FormulaLines {
    type Target = [String];

    fn deref(&self) -> &[String] {
        &self.lines
    }
}
