//! The session's formula and proof lines, behind types that let them be read
//! as slices but only changed through methods that record the change, so the
//! caches derived from them know what to drop, and so `:undo` can store each
//! action as a [`Hunk`] rather than a copy.

use std::cell::Cell;
use std::ops::{Deref, Range};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::checker;

/// A change to a run of lines: lines `at..at + new_len` replaced `old`.
/// [`ProofBuffer::apply`]/[`FormulaLines::apply`] swap `old` back in.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Hunk {
    at: usize,
    old: Vec<String>,
    new_len: usize,
}

impl Hunk {
    pub(crate) fn is_empty(&self) -> bool {
        self.old.is_empty() && self.new_len == 0
    }

    /// Bytes `old` holds, counting each line's `String` as well as its text.
    pub(crate) fn bytes(&self) -> usize {
        self.old
            .iter()
            .map(|line| line.len() + std::mem::size_of::<String>())
            .sum()
    }
}

/// Lines plus, while recording, the [`Hunk`] that undoes every change since
/// recording started.
#[derive(Clone, Default)]
struct Lines {
    lines: Vec<String>,
    recording: Option<Hunk>,
}

impl Lines {
    fn new(lines: Vec<String>) -> Self {
        Lines {
            lines,
            recording: None,
        }
    }

    /// Replaces `lines[range]` with `new`, growing the recorded hunk to
    /// cover the change. Returns the removed lines if `return_removed`;
    /// otherwise the recording keeps the ones it needs without copying them.
    fn splice(
        &mut self,
        range: Range<usize>,
        new: Vec<String>,
        return_removed: bool,
    ) -> Vec<String> {
        let (i, j, k) = (range.start, range.end, new.len());
        let Some(hunk) = &mut self.recording else {
            let removed = self.lines.splice(range, new);
            return if return_removed {
                removed.collect()
            } else {
                Vec::new()
            };
        };
        if hunk.is_empty() {
            let removed: Vec<String> = self.lines.splice(range, new).collect();
            let (old, removed) = if return_removed {
                (removed.clone(), removed)
            } else {
                (removed, Vec::new())
            };
            *hunk = Hunk {
                at: i,
                old,
                new_len: k,
            };
            return removed;
        }

        let end = hunk.at + hunk.new_len;
        // Unchanged lines between the hunk and the change, which the hunk
        // grows to cover.
        let gap_before = if j < hunk.at {
            self.lines[j..hunk.at].to_vec()
        } else {
            Vec::new()
        };
        let gap_after = if i > end {
            self.lines[end..i].to_vec()
        } else {
            Vec::new()
        };
        let mut removed: Vec<String> = self.lines.splice(range, new).collect();
        // Removed lines outside the hunk are unchanged since recording
        // started, so the hunk keeps them; those inside it are dropped.
        let n_before = hunk.at.saturating_sub(i).min(j - i);
        let n_after = j.saturating_sub(end).min(j - i);
        let (before, after) = if return_removed {
            (
                removed[..n_before].to_vec(),
                removed[removed.len() - n_after..].to_vec(),
            )
        } else {
            let after = removed.split_off(removed.len() - n_after);
            removed.truncate(n_before);
            (std::mem::take(&mut removed), after)
        };

        let mut old = before;
        old.extend(gap_before);
        old.append(&mut hunk.old);
        old.extend(gap_after);
        old.extend(after);
        let (lo, hi) = (i.min(hunk.at), j.max(end));
        *hunk = Hunk {
            at: lo,
            old,
            new_len: hi - lo - (j - i) + k,
        };
        removed
    }

    /// Starts recording from `hunk` (as if its changes had just been made),
    /// or stops recording if `None`.
    fn record(&mut self, hunk: Option<Hunk>) {
        self.recording = hunk;
    }

    /// Returns the hunk recorded so far, with lines that ended up unchanged
    /// left out, and records on from an empty one.
    fn take_hunk(&mut self) -> Hunk {
        let Some(hunk) = &mut self.recording else {
            return Hunk::default();
        };
        let mut hunk = std::mem::take(hunk);
        let current = &self.lines[hunk.at..hunk.at + hunk.new_len];
        let prefix = hunk
            .old
            .iter()
            .zip(current)
            .take_while(|(old, new)| old == new)
            .count();
        let suffix = hunk.old[prefix..]
            .iter()
            .rev()
            .zip(current[prefix..].iter().rev())
            .take_while(|(old, new)| old == new)
            .count();
        hunk.old.truncate(hunk.old.len() - suffix);
        hunk.old.drain(..prefix);
        hunk.at += prefix;
        hunk.new_len -= prefix + suffix;
        if hunk.is_empty() {
            return Hunk::default();
        }
        hunk
    }

    /// Undoes `hunk` without recording it, returning the hunk that redoes
    /// it.
    fn apply(&mut self, hunk: Hunk) -> Hunk {
        let Hunk { at, old, new_len } = hunk;
        let restored = old.len();
        let removed = self.lines.splice(at..at + new_len, old).collect();
        Hunk {
            at,
            old: removed,
            new_len: restored,
        }
    }
}

/// The proof lines. Reads as a `[String]`; every change goes through a
/// method here, which lowers the low-water mark (see
/// [`Self::take_changed_from`]).
#[derive(Clone)]
pub struct ProofBuffer {
    lines: Lines,
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
        let len = self.len();
        self.changed(len);
        self.added(&line);
        self.lines.splice(len..len, vec![line], false);
    }

    pub fn pop(&mut self) -> Option<String> {
        let idx = self.len().checked_sub(1)?;
        Some(self.remove(idx))
    }

    pub fn insert(&mut self, idx: usize, line: String) {
        self.changed(idx);
        self.added(&line);
        self.lines.splice(idx..idx, vec![line], false);
    }

    pub fn remove(&mut self, idx: usize) -> String {
        self.changed(idx);
        let line = self.lines.splice(idx..idx + 1, Vec::new(), true).remove(0);
        self.removed(&line);
        line
    }

    /// Replaces line `idx`, returning the old text.
    pub fn set(&mut self, idx: usize, line: String) -> String {
        if self[idx] == line {
            return line;
        }
        self.changed(idx);
        self.added(&line);
        let old = self.lines.splice(idx..idx + 1, vec![line], true).remove(0);
        self.removed(&old);
        old
    }

    pub fn clear(&mut self) {
        self.changed(0);
        self.longest.set(None);
        self.lines.splice(0..self.len(), Vec::new(), false);
    }

    pub fn extend(&mut self, lines: impl IntoIterator<Item = String>) {
        let len = self.len();
        self.changed(len);
        self.longest.set(None);
        self.lines
            .splice(len..len, lines.into_iter().collect(), false);
    }

    /// Replaces every line. Lines before the first difference count as
    /// unchanged.
    pub fn replace(&mut self, mut lines: Vec<String>) {
        let prefix = self
            .iter()
            .zip(&lines)
            .take_while(|(old, new)| old == new)
            .count();
        let suffix = self[prefix..]
            .iter()
            .rev()
            .zip(lines[prefix..].iter().rev())
            .take_while(|(old, new)| old == new)
            .count();
        if prefix == self.len() && prefix == lines.len() {
            return;
        }
        self.changed(prefix);
        self.longest.set(None);
        lines.truncate(lines.len() - suffix);
        lines.drain(..prefix);
        let end = self.len() - suffix;
        self.lines.splice(prefix..end, lines, false);
    }

    /// Starts recording changes from `hunk` (see [`Hunk`]), or stops if
    /// `None`.
    pub(crate) fn record(&mut self, hunk: Option<Hunk>) {
        self.lines.record(hunk);
    }

    /// Returns the hunk that undoes every change recorded so far, and
    /// records on from nothing. Empty if not recording.
    pub(crate) fn take_hunk(&mut self) -> Hunk {
        self.lines.take_hunk()
    }

    /// Undoes `hunk` (unrecorded), returning the hunk that redoes it.
    pub(crate) fn apply(&mut self, hunk: Hunk) -> Hunk {
        if hunk.is_empty() {
            return hunk;
        }
        self.changed(hunk.at);
        self.longest.set(None);
        self.lines.apply(hunk)
    }
}

impl Deref for ProofBuffer {
    type Target = [String];

    fn deref(&self) -> &[String] {
        &self.lines.lines
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
            lines: Lines::new(lines),
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
    lines: Lines,
    /// The first constraint ID of each line (see
    /// `checker::parse::formula_line_constraints`), then one past the last.
    first_ids: Vec<usize>,
    epoch: u64,
}

impl FormulaLines {
    pub fn new(lines: Vec<String>) -> Self {
        let mut formula = FormulaLines {
            lines: Lines::new(lines),
            first_ids: Vec::new(),
            epoch: next_epoch(),
        };
        formula.recount();
        formula
    }

    /// Recounts the IDs after a change.
    fn changed(&mut self) {
        self.recount();
        self.epoch = next_epoch();
    }

    fn recount(&mut self) {
        let mut next = 1;
        self.first_ids = Vec::with_capacity(self.len() + 1);
        for line in &self.lines.lines {
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
        &self.first_ids[..self.len()]
    }

    /// Returns the number of constraints veripb loads from these lines.
    pub fn constraint_count(&self) -> usize {
        self.first_ids[self.len()] - 1
    }

    /// Replaces line `idx`, returning the old text.
    pub fn set(&mut self, idx: usize, line: String) -> String {
        let old = self.lines.splice(idx..idx + 1, vec![line], true).remove(0);
        self.changed();
        old
    }

    /// Replaces every line.
    pub fn replace(&mut self, lines: Vec<String>) {
        let len = self.len();
        self.lines.splice(0..len, lines, false);
        self.changed();
    }

    /// As [`ProofBuffer::record`].
    pub(crate) fn record(&mut self, hunk: Option<Hunk>) {
        self.lines.record(hunk);
    }

    /// As [`ProofBuffer::take_hunk`].
    pub(crate) fn take_hunk(&mut self) -> Hunk {
        self.lines.take_hunk()
    }

    /// As [`ProofBuffer::apply`].
    pub(crate) fn apply(&mut self, hunk: Hunk) -> Hunk {
        if hunk.is_empty() {
            return hunk;
        }
        let redo = self.lines.apply(hunk);
        self.changed();
        redo
    }
}

impl Deref for FormulaLines {
    type Target = [String];

    fn deref(&self) -> &[String] {
        &self.lines.lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic generator (xorshift), so failures reproduce.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n.max(1) as u64) as usize
        }
    }

    fn lines(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|t| t.to_string()).collect()
    }

    /// One random change, with texts drawn from a few values so changes
    /// often put back what was there.
    fn mutate(buffer: &mut ProofBuffer, rng: &mut Rng) {
        let t = ["a", "b", "c"][rng.below(3)];
        let text = || t.to_string();
        let len = buffer.len();
        match rng.below(8) {
            0 => buffer.push(text()),
            1 => {
                buffer.pop();
            }
            2 => buffer.insert(rng.below(len + 1), text()),
            3 if len > 0 => {
                buffer.remove(rng.below(len));
            }
            4 if len > 0 => {
                buffer.set(rng.below(len), text());
            }
            5 => buffer.extend([text(), text()]),
            6 if rng.below(4) == 0 => buffer.clear(),
            _ => {
                let mut new = buffer.to_vec();
                let at = rng.below(new.len() + 1);
                new.splice(at..(at + rng.below(3)).min(new.len()), [text()]);
                buffer.replace(new);
            }
        }
    }

    #[test]
    fn hunks_undo_and_redo_random_changes() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..500 {
            let start: Vec<String> = (0..rng.below(8)).map(|i| i.to_string()).collect();
            let mut buffer = ProofBuffer::from(start.clone());
            buffer.record(Some(Hunk::default()));
            for _ in 0..rng.below(10) {
                mutate(&mut buffer, &mut rng);
            }
            let end = buffer.to_vec();
            let hunk = buffer.take_hunk();
            assert!(hunk.old.len() <= start.len());
            let redo = buffer.apply(hunk.clone());
            assert_eq!(&*buffer, &start[..], "undo of {hunk:?}");
            buffer.apply(redo);
            assert_eq!(&*buffer, &end[..], "redo of {hunk:?}");
        }
    }

    #[test]
    fn a_hunk_holds_only_the_changed_lines() {
        let mut buffer = ProofBuffer::from(lines(&["a", "b", "c", "d"]));
        buffer.record(Some(Hunk::default()));
        buffer.set(2, "C".to_string());
        assert_eq!(
            buffer.take_hunk(),
            Hunk {
                at: 2,
                old: lines(&["c"]),
                new_len: 1
            }
        );

        buffer.clear();
        buffer.replace(lines(&["a", "b", "C", "d"]));
        assert!(buffer.take_hunk().is_empty(), "restored as it was");

        buffer.push("e".to_string());
        assert_eq!(
            buffer.take_hunk(),
            Hunk {
                at: 4,
                old: Vec::new(),
                new_len: 1
            }
        );
    }

    #[test]
    fn nothing_is_recorded_until_asked() {
        let mut buffer = ProofBuffer::from(lines(&["a"]));
        buffer.push("b".to_string());
        assert!(buffer.take_hunk().is_empty());
    }

    #[test]
    fn a_formula_hunk_restores_the_ids() {
        let mut formula = FormulaLines::new(lines(&["1 x1 >= 1 ;", "1 x2 >= 1 ;"]));
        formula.record(Some(Hunk::default()));
        formula.set(0, "1 x1 1 x2 = 1 ;".to_string());
        assert_eq!(formula.constraint_count(), 3);
        let epoch = formula.epoch();
        let hunk = formula.take_hunk();
        formula.apply(hunk);
        assert_eq!(&*formula, &lines(&["1 x1 >= 1 ;", "1 x2 >= 1 ;"])[..]);
        assert_eq!(formula.constraint_count(), 2);
        assert_ne!(formula.epoch(), epoch);
    }
}
