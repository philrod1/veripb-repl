//! The REPL's core engine: session state plus the replay machinery shared by
//! every command that checks something against it (typing a bare rule,
//! `:check`, `:save`, `:source`, `:edit`, ...).
//!
//! Holds no `:command`-syntax parsing/printing logic — that belongs to the
//! frontends' dispatch and the individual `commands::*` modules. This module
//! builds a candidate buffer, hands it to the `veripb` subprocess (via
//! `crate::checker`), and classifies/applies the result. No checker object
//! is kept resident between calls: every check re-invokes `veripb` from
//! scratch against the current formula plus whatever candidate text is being
//! tried, so retracting `checked_len` is pure bookkeeping, not a rebuild.
//!
//! **The buffer is a live, always-present list of proof-format lines, only
//! some prefix of which is currently checked.** `buffer: Vec<String>` never
//! shrinks except by an explicit, user-requested delete. `checked_len: usize`
//! marks how much of the front of `buffer` has been verified; everything
//! from there on is unverified text that may not even parse. Typing a new
//! line at the end (`append_line`) checks it immediately if nothing was
//! already pending. Editing an existing line (`set_line`/`insert_line`/
//! `delete_line`) never checks anything — it retracts `checked_len` if the
//! edit falls inside the checked prefix. Checking is always an explicit act
//! (`drive_forward`, `:verify`'s implementation) that walks forward from
//! `checked_len` and stops — without discarding anything — at the first line
//! that doesn't check out.
//!
//! **The formula is also just text this REPL owns**, not a parsed object:
//! `formula: Vec<String>` holds its constraint lines (comments and the
//! `#variable=`/`#constraint=` header stripped at load time), `objective`,
//! separately, its `min:`/`max:` line if it has one, and `preserved`,
//! separately again, its `preserved: ...;` line if it declares a preserved
//! variable set. Every `checker::` call writes the current formula to a
//! fresh temp file first (see `formula_temp_file`); no permanent on-disk
//! copy is kept in sync with `formula_path` after the initial load.

use std::{
    collections::{BTreeSet, VecDeque},
    io::Write as _,
};

use anyhow::Context as _;

use crate::checker::{self, CheckOutcome};
use crate::varnames::VarNames;

/// Lines in a synthesized v3 proof preamble.
const PREAMBLE_LINES: usize = 2;

/// One `:undo`-stack entry — everything `restore_snapshot` needs to fully
/// restore a `Session`.
pub(crate) struct Snapshot {
    formula: Vec<String>,
    objective: Option<String>,
    preserved: Option<String>,
    buffer: Vec<String>,
    checked_len: usize,
    known_bad: Option<String>,
}

/// Maximum entries kept in `Session::undo_stack` before the oldest is
/// discarded. Unbounded would be O(actions²) worst-case memory — each entry
/// clones the buffer as it stood then, pushed once per accepted line.
const UNDO_STACK_CAP: usize = 200;

/// A `:formula` edit's undo point: the formula constraint it replaced, plus
/// the buffer to restore alongside it. Set by `replace_formula_constraint`,
/// consumed by `:formula cancel`; invalidated by anything else the user
/// types in between (see `commands::dispatch`).
pub(crate) struct FormulaEditSnapshot {
    /// 1-based formula constraint number that was edited.
    pub(crate) n: usize,
    /// What used to occupy it, verbatim.
    pub(crate) constraint: String,
    /// The buffer this edit tried to reverify against — see
    /// `Session::recoverable_buffer`, not necessarily just `buffer` as it
    /// stood right before this call.
    pub(crate) buffer: Vec<String>,
}

/// The state of one REPL session: the formula it was loaded with and the
/// live proof buffer (see the module docs).
pub struct Session {
    /// The path the formula was loaded from, for display only — the file is
    /// never re-read after load.
    pub formula_path: String,
    /// The formula's constraint lines, 1-based indices matching
    /// `:formula`/the Formula pane's own numbering.
    pub formula: Vec<String>,
    /// The formula's `min:`/`max:` objective line, verbatim, if it has one.
    pub objective: Option<String>,
    /// The formula's `preserved: <variable> <variable> ...;` line, verbatim,
    /// if it declares a preserved variable set (used by `preserved_add`/
    /// `preserved_rm`/`epreserved`/`solx` proof rules).
    pub preserved: Option<String>,
    pub variables: VarNames,
    /// Always empty for now: labels need a correlation mechanism now that
    /// no in-process parser tracks `@label` tokens against the constraint
    /// IDs they name. Kept as a real field so `labels_by_id` and its
    /// callers need no second API change once that's solved.
    pub labels: ahash::AHashMap<String, isize>,
    pub label_map: ahash::AHashMap<String, isize>,
    /// Every proof-format line currently in the proof, in order, checked or
    /// not. Never truncated as a side effect of a failed check — only grown,
    /// shrunk by an explicit delete, or has an entry's text replaced in
    /// place. See the module docs.
    pub buffer: Vec<String>,
    /// How many lines from the front of `buffer` have been verified. Lines
    /// at index `>= checked_len` haven't been checked against the buffer's
    /// current content.
    pub checked_len: usize,
    /// Set when `buffer[checked_len]` is known, right now, to fail — the
    /// line `drive_forward`/`:verify` most recently stopped on. Cleared by
    /// anything that changes `checked_len` or that line's text. Drives the
    /// Proof pane's error highlight and `:list`'s failure tag.
    pub known_bad: Option<String>,
    /// Set by the most recent `:formula` edit, consumed by `:formula
    /// cancel` — see `FormulaEditSnapshot`.
    pub(crate) last_formula_edit: Option<FormulaEditSnapshot>,
    /// The fullest buffer still worth retrying against a formula edit,
    /// remembered across a sequence of edits so one bad edit doesn't
    /// permanently drop lines a later, corrected edit could still recover.
    /// `None` means "use `buffer` itself." Set by
    /// `replace_formula_constraint`; cleared by `commands::formula::commit`
    /// once a reverify fully succeeds, by `:formula cancel`, and by
    /// anything else typed in between (see `commands::dispatch`).
    pub(crate) recoverable_buffer: Option<Vec<String>>,
    /// Bumped by every method that mutates `formula`/`objective`/`buffer`/
    /// `checked_len` on success. What `commands::dispatch` checks to decide
    /// whether to push onto `undo_stack`, and what `database_cache` keys
    /// its staleness check on.
    pub(crate) generation: u64,
    /// Snapshots of state from before each undo-able action, most recent
    /// last — see `Snapshot`, `push_undo`, `pop_undo`, `restore_snapshot`.
    pub(crate) undo_stack: VecDeque<Snapshot>,
    /// Buffer indices with a debugging breakpoint set, for the TUI's
    /// Vim-mode `b` key (and, eventually, `:debug`'s own `:break`). Not
    /// consulted by any checking path, and not part of [`Snapshot`]/`:undo`
    /// — a breakpoint is editor furniture, not proof state. Kept in step
    /// with the buffer by [`Self::insert_line`]/[`Self::delete_line`];
    /// [`Self::set_line`] never needs to touch them. [`Self::reset`] clears
    /// them.
    pub breakpoints: BTreeSet<usize>,
    /// The live database as of some earlier `generation`, refreshed lazily
    /// by [`Self::database`] rather than kept in sync by every mutator.
    /// `RefCell`, not a plain field: populating the cache is not a
    /// domain-level mutation, and several read-only call sites
    /// (`commands::edit::run_readonly`, `:debug`'s allowlisted commands)
    /// need `&Session` to be enough to read from.
    database_cache: std::cell::RefCell<Option<(u64, checker::Database)>>,
}

/// What happened when a freshly-typed line was handed to [`Session::append_line`].
pub enum AppendOutcome {
    /// Nothing was pending; `line` was checked immediately and accepted.
    Verified { captured: String },
    /// Nothing was pending; `line` was checked immediately and rejected —
    /// not added to the buffer, state unchanged.
    Rejected { captured: String, error: String },
    /// Something was already pending, so `line` joined the end of it,
    /// unchecked — no check was attempted.
    Deferred,
}

impl Session {
    /// Loads a formula (OPB only, for now) and starts a fresh, empty
    /// session.
    pub fn load(formula_path: &str) -> anyhow::Result<Self> {
        let (formula, objective, preserved) = read_formula_lines(formula_path)
            .with_context(|| format!("failed to read formula file {formula_path}"))?;
        let variables = VarNames::from_formula_file(formula_path)
            .with_context(|| format!("failed to read formula file {formula_path}"))?;

        Ok(Session {
            formula_path: formula_path.to_string(),
            formula,
            objective,
            preserved,
            variables,
            labels: ahash::AHashMap::new(),
            label_map: ahash::AHashMap::new(),
            buffer: Vec::new(),
            checked_len: 0,
            known_bad: None,
            last_formula_edit: None,
            recoverable_buffer: None,
            generation: 0,
            undo_stack: VecDeque::new(),
            breakpoints: BTreeSet::new(),
            database_cache: std::cell::RefCell::new(None),
        })
    }

    /// Returns the labels naming each constraint ID, inverted from
    /// `label_map` (several labels can name one ID, so each gets a sorted
    /// list). Always empty today — see `label_map`'s own docs.
    pub fn labels_by_id(&self) -> ahash::AHashMap<isize, Vec<String>> {
        let mut by_id: ahash::AHashMap<isize, Vec<String>> = ahash::AHashMap::new();
        for (name, id) in &self.label_map {
            by_id.entry(*id).or_default().push(name.clone());
        }
        for names in by_id.values_mut() {
            names.sort();
        }
        by_id
    }

    /// Returns the synthesized preamble lines every replay buffer starts
    /// with, for `:list`/the TUI's proof panel to number their own display
    /// consistently with the checker's `line N:` trace output.
    pub fn preamble_lines(&self) -> [String; PREAMBLE_LINES] {
        [
            "pseudo-Boolean proof version 3.0".to_string(),
            format!("f {};", self.formula.len()),
        ]
    }

    /// Returns the synthesized v3 preamble plus `lines`, ready to hand to
    /// [`checker::check`] and friends. `lines` is a parameter rather than
    /// always `&self.buffer`: checking machinery wants only the checked
    /// prefix (`buffer[..checked_len]`), while `listing` (what `:save`
    /// writes) wants the whole buffer regardless of check status.
    fn preamble_and_lines(&self, lines: &[String]) -> String {
        let mut text = String::new();
        for line in self.preamble_lines() {
            text.push_str(&line);
            text.push('\n');
        }
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// Writes this session's current formula to a fresh temp `.opb` file —
    /// every `checker::` call needs one on disk.
    fn formula_temp_file(&self) -> anyhow::Result<tempfile::NamedTempFile> {
        write_formula_temp_file(&self.formula, &self.objective, &self.preserved)
    }

    /// Checks `text` against this session's current formula, tracing scoped
    /// to `trace_range`. The shared entry point every replay in this module
    /// funnels through.
    fn check(&self, text: &str, trace_range: Option<(usize, usize)>) -> anyhow::Result<CheckOutcome> {
        let formula_file = self.formula_temp_file()?;
        checker::check(formula_file.path(), text, trace_range)
    }

    /// Returns the live database reflecting `buffer[..checked_len]`,
    /// refreshed (one subprocess call) only if `generation` has moved since
    /// it was last cached. Never checks anything itself — call
    /// `drive_forward`/`:verify` first for unchecked content to be
    /// reflected here.
    ///
    /// Takes `&self`, not `&mut self`: populating the cache stays callable
    /// from read-only contexts (`commands::edit::run_readonly`, `:debug`'s
    /// allowlisted commands) — see `database_cache`'s own docs. Returns a
    /// `Ref` for the same reason; derefs like `&checker::Database` at every
    /// call site (`session.database()?.entries`, etc.).
    pub fn database(&self) -> anyhow::Result<std::cell::Ref<'_, checker::Database>> {
        let fresh = matches!(
            &*self.database_cache.borrow(),
            Some((cached_gen, _)) if *cached_gen == self.generation
        );
        if !fresh {
            let text = self.preamble_and_lines(&self.buffer[..self.checked_len]);
            let formula_file = self.formula_temp_file()?;
            let database = checker::show_database(formula_file.path(), &text)?;
            *self.database_cache.borrow_mut() = Some((self.generation, database));
        }
        Ok(std::cell::Ref::map(self.database_cache.borrow(), |cache| {
            &cache.as_ref().expect("just populated above").1
        }))
    }

    /// Attempts `buffer[checked_len]` — the first unchecked line. On
    /// acceptance, advances `checked_len` by one. On rejection, `buffer`/
    /// `checked_len` are unchanged and `known_bad` is set. Panics if nothing
    /// is pending (`checked_len == buffer.len()`) — callers
    /// ([`Self::drive_forward`], [`Self::append_line`]) only call this after
    /// confirming there is.
    fn verify_next(&mut self) -> anyhow::Result<(String, Option<String>)> {
        let candidate_line = self.buffer[self.checked_len].clone();
        let mut text = self.preamble_and_lines(&self.buffer[..self.checked_len]);
        text.push_str(&candidate_line);
        text.push('\n');

        // Trace only the new line, not the whole replayed prefix (already
        // shown when it was first accepted).
        let candidate_line_no = PREAMBLE_LINES + self.checked_len + 1;
        let outcome = self.check(&text, Some((candidate_line_no, candidate_line_no)))?;

        match outcome {
            CheckOutcome::Accepted { trace } => {
                self.checked_len += 1;
                self.known_bad = None;
                self.generation += 1;
                Ok((trace, None))
            }
            CheckOutcome::Rejected { message, trace, .. } => {
                self.known_bad = Some(message.clone());
                Ok((trace, Some(message)))
            }
        }
    }

    /// Drives `checked_len` forward through `buffer` as far as it will go:
    /// [`Self::verify_next`] in a loop, committing each accepted line and
    /// stopping — without discarding anything — at the first rejection.
    /// `:verify`'s entire implementation, and what `:formula`'s post-edit
    /// reverify reduces to once its candidate buffer is in place (see
    /// [`Self::replace_buffer_and_verify`]). Concatenates every attempted
    /// line's captured trace in order.
    pub fn drive_forward(&mut self) -> anyhow::Result<(String, Option<String>)> {
        let mut captured_all = String::new();
        while self.checked_len < self.buffer.len() {
            let (captured, rejection) = self.verify_next()?;
            captured_all.push_str(&captured);
            if let Some(err) = rejection {
                return Ok((captured_all, Some(err)));
            }
        }
        Ok((captured_all, None))
    }

    /// `:debug`'s `:step` primitive: attempts exactly the next unchecked
    /// line — the same check [`Self::append_line`]'s "check immediately"
    /// path runs, for a line already in the buffer rather than one just
    /// typed. Returns `Ok(None)`, not an error, once nothing is left to
    /// step onto.
    pub fn step(&mut self) -> anyhow::Result<Option<(String, Option<String>)>> {
        if self.checked_len >= self.buffer.len() {
            return Ok(None);
        }
        self.verify_next().map(Some)
    }

    /// `:debug`'s `:continue`/`:until <n>` engine: runs forward one line at
    /// a time via [`Self::verify_next`], stopping at the first of: a
    /// rejection, `target` (a one-off destination for `:until`), or a
    /// registered [`Self::breakpoints`] entry. The breakpoint check is
    /// skipped on the first iteration (`first`, below) so resuming from a
    /// line that's itself marked doesn't immediately re-stop there. Not
    /// used by `drive_forward`: `:verify` stays breakpoint-oblivious.
    pub fn continue_run(&mut self, target: Option<usize>) -> anyhow::Result<(String, Option<String>)> {
        let mut captured_all = String::new();
        let mut first = true;
        loop {
            if self.checked_len >= self.buffer.len() {
                break;
            }
            if target.is_some_and(|t| self.checked_len >= t) {
                break;
            }
            if !first && self.breakpoints.contains(&self.checked_len) {
                break;
            }
            first = false;
            let (captured, rejection) = self.verify_next()?;
            captured_all.push_str(&captured);
            if let Some(err) = rejection {
                return Ok((captured_all, Some(err)));
            }
        }
        Ok((captured_all, None))
    }

    /// The ordinary `pbp>`-prompt path for a freshly-typed line. If nothing
    /// was already pending, checks `line` immediately (rejection: not added
    /// to the buffer, state unchanged). If something was already pending,
    /// `line` joins the end of it, unchecked, same as `:source` appending.
    pub fn append_line(&mut self, line: &str) -> anyhow::Result<AppendOutcome> {
        let was_live_edge = self.checked_len == self.buffer.len();
        self.buffer.push(line.to_string());
        if !was_live_edge {
            return Ok(AppendOutcome::Deferred);
        }
        let (captured, rejection) = self.verify_next()?;
        match rejection {
            None => Ok(AppendOutcome::Verified { captured }),
            Some(error) => {
                self.buffer.pop();
                self.known_bad = None;
                Ok(AppendOutcome::Rejected { captured, error })
            }
        }
    }

    /// Replaces `buffer[idx]`'s text in place — the mechanism behind
    /// `:edit`/`:deassert`/`:insert`'s retyping and the TUI Vim-mode's
    /// commit action. Never checks the new text and never touches any other
    /// buffer entry. If `idx` was inside the checked prefix, retracts
    /// `checked_len` to `idx`.
    pub fn set_line(&mut self, idx: usize, new_text: &str) -> anyhow::Result<()> {
        self.buffer[idx] = new_text.to_string();
        self.checked_len = self.checked_len.min(idx);
        self.known_bad = None;
        self.generation += 1;
        Ok(())
    }

    /// Inserts a new, unchecked line before buffer index `idx` (`:insert`'s
    /// mechanism), shifting everything at/after `idx` down. Same
    /// retract-`checked_len` rule as [`Self::set_line`].
    pub fn insert_line(&mut self, idx: usize, text: &str) -> anyhow::Result<()> {
        self.buffer.insert(idx, text.to_string());
        self.checked_len = self.checked_len.min(idx);
        self.breakpoints = self
            .breakpoints
            .iter()
            .map(|&b| if b >= idx { b + 1 } else { b })
            .collect();
        self.known_bad = None;
        self.generation += 1;
        Ok(())
    }

    /// Removes buffer index `idx` outright (`:delete`'s mechanism). Same
    /// retract-`checked_len` rule as [`Self::set_line`].
    pub fn delete_line(&mut self, idx: usize) -> anyhow::Result<()> {
        self.buffer.remove(idx);
        self.checked_len = self.checked_len.min(idx);
        self.breakpoints = self
            .breakpoints
            .iter()
            .filter_map(|&b| match b.cmp(&idx) {
                std::cmp::Ordering::Less => Some(b),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(b - 1),
            })
            .collect();
        self.known_bad = None;
        self.generation += 1;
        Ok(())
    }

    /// Toggles a breakpoint on buffer index `idx`. Doesn't bump
    /// `generation`: a breakpoint isn't proof state (see `breakpoints`'s
    /// own docs).
    pub fn toggle_breakpoint(&mut self, idx: usize) {
        if !self.breakpoints.remove(&idx) {
            self.breakpoints.insert(idx);
        }
    }

    /// Replaces the buffer wholesale with `lines` and checks as much of it
    /// as still holds: one O(n) batched replay first; only if that fails
    /// does it fall back to [`Self::drive_forward`]'s one-line-at-a-time
    /// loop, to pinpoint exactly where, leaving everything after it in the
    /// buffer, unchecked.
    pub(crate) fn replace_buffer_and_verify(&mut self, lines: Vec<String>) -> anyhow::Result<(String, Option<String>)> {
        self.buffer = lines;
        self.checked_len = 0;
        self.known_bad = None;
        if self.buffer.is_empty() {
            return Ok((String::new(), None));
        }

        let text = self.preamble_and_lines(&self.buffer);
        let candidate_start = PREAMBLE_LINES + 1;
        let candidate_end = candidate_start + self.buffer.len() - 1;
        let outcome = self.check(&text, Some((candidate_start, candidate_end)))?;
        if let CheckOutcome::Accepted { trace } = outcome {
            self.checked_len = self.buffer.len();
            self.generation += 1;
            return Ok((trace, None));
        }
        // The batched attempt's output is dropped: `drive_forward` below
        // re-derives the successful prefix, and printing both would
        // duplicate the trace.
        self.drive_forward()
    }

    /// Restores `buffer`/`checked_len`/`known_bad` directly (formula
    /// untouched) — the edit-family commands' `:cancel` mechanism.
    pub(crate) fn restore_buffer_state(
        &mut self,
        buffer: Vec<String>,
        checked_len: usize,
        known_bad: Option<String>,
    ) -> anyhow::Result<()> {
        self.buffer = buffer;
        self.checked_len = checked_len;
        self.known_bad = known_bad;
        self.generation += 1;
        Ok(())
    }

    /// Drops every buffer line, checked or not, without touching
    /// `formula`/`variables`/`labels`.
    pub fn reset(&mut self) -> anyhow::Result<()> {
        self.buffer.clear();
        self.checked_len = 0;
        self.known_bad = None;
        self.label_map = self.labels.clone();
        self.breakpoints.clear();
        self.generation += 1;
        // Mutates in place rather than replacing the session, so
        // `undo_stack` needs an explicit clear.
        self.undo_stack.clear();
        Ok(())
    }

    /// Retracts `checked_len` to `target` (must be `<= checked_len`) —
    /// `:debug`'s analog of [`Self::set_line`]'s retract rule, without any
    /// text changing. Shared by [`Self::step_back`] (`target = checked_len
    /// - 1`) and [`Self::restart`] (`target = 0`).
    fn retract_checked_len(&mut self, target: usize) -> anyhow::Result<()> {
        debug_assert!(target <= self.checked_len);
        self.checked_len = target;
        self.known_bad = None;
        self.generation += 1;
        Ok(())
    }

    /// `:back`: steps the checked prefix backward by exactly one line, if
    /// possible. Nothing is re-verified going backward. Returns whether
    /// anything moved.
    pub fn step_back(&mut self) -> anyhow::Result<bool> {
        if self.checked_len == 0 {
            return Ok(false);
        }
        self.retract_checked_len(self.checked_len - 1)?;
        Ok(true)
    }

    /// `:restart`: retracts the checked prefix to the start, keeping every
    /// buffer line as-is — the non-destructive sibling of [`Self::reset`].
    /// A no-op if nothing was checked yet.
    pub fn restart(&mut self) -> anyhow::Result<()> {
        if self.checked_len == 0 {
            return Ok(());
        }
        self.retract_checked_len(0)
    }

    /// Converts a 1-based display line number — `:list`'s/the proof
    /// panel's/the checker's own `line N:` numbering, preamble first — into
    /// a 0-based index into `buffer`. `None` for a preamble line or
    /// anything at or past the end of the buffer. Says nothing about
    /// whether the line is checked — see [`Self::checked_index`].
    pub fn buffer_index(&self, display_line: usize) -> Option<usize> {
        let idx = display_line.checked_sub(PREAMBLE_LINES + 1)?;
        (idx < self.buffer.len()).then_some(idx)
    }

    /// Like [`Self::buffer_index`], but additionally requires the line to
    /// be checked — `None` for the preamble, past the end of the buffer, or
    /// the unchecked tail. Used by `:explain`/`:why`, which replay
    /// already-accepted content.
    pub fn checked_index(&self, display_line: usize) -> Option<usize> {
        let idx = self.buffer_index(display_line)?;
        (idx < self.checked_len).then_some(idx)
    }

    /// The inverse of `buffer_index`: the display line number for a 0-based
    /// `buffer` index. Always valid for an in-range index. Unaffected by
    /// checked status, since the buffer is never truncated.
    pub fn display_line(&self, buffer_idx: usize) -> usize {
        buffer_idx + PREAMBLE_LINES + 1
    }

    /// Converts a 1-based formula constraint number — `:show`'s/the Formula
    /// pane's numbering — into a 0-based index into `formula`. A distinct
    /// numbering space from `buffer_index`'s. `None` for anything out of
    /// range.
    pub(crate) fn formula_index(&self, n: usize) -> Option<usize> {
        (n >= 1 && n <= self.formula.len()).then(|| n - 1)
    }

    /// Returns a clone of everything `restore_snapshot` needs to bring
    /// `self` back to exactly its current state.
    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot {
            formula: self.formula.clone(),
            objective: self.objective.clone(),
            preserved: self.preserved.clone(),
            buffer: self.buffer.clone(),
            checked_len: self.checked_len,
            known_bad: self.known_bad.clone(),
        }
    }

    /// Pushes `snapshot` onto the undo stack, discarding the oldest entry
    /// first if already at `UNDO_STACK_CAP`.
    pub(crate) fn push_undo(&mut self, snapshot: Snapshot) {
        if self.undo_stack.len() == UNDO_STACK_CAP {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(snapshot);
    }

    /// Pops the most recent undo-stack entry, if any.
    pub(crate) fn pop_undo(&mut self) -> Option<Snapshot> {
        self.undo_stack.pop_back()
    }

    /// Restores `self` to exactly what `snapshot` captured. Also clears
    /// `last_formula_edit`/`recoverable_buffer`, since whatever they
    /// referred to is no longer the most recent thing that happened.
    pub(crate) fn restore_snapshot(&mut self, snapshot: Snapshot) -> anyhow::Result<()> {
        self.formula = snapshot.formula;
        self.objective = snapshot.objective;
        self.preserved = snapshot.preserved;
        self.buffer = snapshot.buffer;
        self.checked_len = snapshot.checked_len;
        self.known_bad = snapshot.known_bad;
        self.last_formula_edit = None;
        self.recoverable_buffer = None;
        self.generation += 1;
        Ok(())
    }

    /// Replaces formula constraint `n` with `new_text` (`:formula`'s
    /// mechanism). Validated against a candidate formula file before `self`
    /// is touched, so a rejection leaves `self` unchanged. On success,
    /// `buffer` is cleared (`checked_len`/`known_bad` reset with it);
    /// reverifying whatever of the base buffer still holds against the new
    /// formula is the caller's job (`commands::formula`, via
    /// [`Self::replace_buffer_and_verify`]). That base is
    /// `recoverable_buffer` if one is already remembered, or `buffer`
    /// itself otherwise; either way it's what
    /// `last_formula_edit`/`recoverable_buffer` are set to, so `:formula
    /// cancel` can undo both together.
    ///
    /// Only in-place, same-count edits are supported: an OPB `=` constraint
    /// is really two constraints (`>=` and `<=`), which would change the
    /// total count — rejected rather than mishandled.
    pub fn replace_formula_constraint(&mut self, n: usize, new_text: &str) -> anyhow::Result<()> {
        let idx = self.formula_index(n).ok_or_else(|| {
            anyhow::anyhow!(
                "constraint {n} is out of range — the formula has {} constraint(s) (1-{}).",
                self.formula.len(),
                self.formula.len()
            )
        })?;

        // The trailing `;` OPB syntax requires is implied when editing one
        // already-formatted constraint at a time.
        let mut new_text = new_text.trim().to_string();
        if !new_text.ends_with(';') {
            new_text.push_str(" ;");
        }

        if is_equality_constraint(&new_text) {
            anyhow::bail!(
                "an `=` constraint splits into two (`>=` and `<=`) — editing one formula \
                 constraint in place can't change how many there are. Use `>=` or `<=` \
                 instead, or edit the OPB file and :load it if the constraint count needs \
                 to change."
            );
        }

        let mut candidate_formula = self.formula.clone();
        candidate_formula[idx] = new_text.clone();

        // Validate the candidate formula alone (no proof lines) before
        // touching `self`.
        let candidate_file = write_formula_temp_file(&candidate_formula, &self.objective, &self.preserved)?;
        let empty_proof = format!(
            "pseudo-Boolean proof version 3.0\nf {};\n",
            candidate_formula.len()
        );
        let outcome = checker::check(candidate_file.path(), &empty_proof, None)
            .context("failed to validate the edited formula")?;
        if let CheckOutcome::Rejected { message, .. } = outcome {
            anyhow::bail!("the edited formula doesn't check out: {message}");
        }

        // Re-scan the already-written candidate file to pick up any new
        // variable names in `new_text`.
        let candidate_vars = VarNames::from_formula_file(candidate_file.path())
            .context("failed to re-scan variable names after the formula edit")?;

        // The base to reverify against isn't necessarily `buffer` as it
        // stands now — `recoverable_buffer` holds the fuller buffer an
        // earlier, only-partially-reverified edit was trying to restore.
        let base_buffer = match &self.recoverable_buffer {
            Some(b) => b.clone(),
            None => self.buffer.clone(),
        };

        // Nothing below this line can fail — commit as one unit.
        let prior_constraint = std::mem::replace(&mut self.formula[idx], new_text);
        self.buffer.clear();
        self.checked_len = 0;
        self.known_bad = None;
        self.variables = candidate_vars;
        self.label_map = self.labels.clone();
        self.recoverable_buffer = Some(base_buffer.clone());
        self.last_formula_edit = Some(FormulaEditSnapshot {
            n,
            constraint: prior_constraint,
            buffer: base_buffer,
        });
        self.generation += 1;
        Ok(())
    }

    /// Returns the full synthesized `.pbp` text this session currently
    /// represents: the v3 preamble, the `f N;` line, the entire buffer
    /// (checked or not), and a closing `output NONE; conclusion
    /// <conclusion>; end pseudo-Boolean proof;` sequence. Pure text
    /// construction — never checked, so makes no claim the result actually
    /// verifies; see `dry_run_conclusion` for that.
    pub fn listing(&self, conclusion: &str) -> String {
        let mut text = self.preamble_and_lines(&self.buffer);
        text.push_str("output NONE;\n");
        text.push_str(&format!("conclusion {conclusion};\n"));
        text.push_str("end pseudo-Boolean proof;\n");
        text
    }

    /// Non-destructively tests whether the checked prefix of the buffer,
    /// followed by `output NONE; conclusion <conclusion>; end
    /// pseudo-Boolean proof;`, would form a complete, valid proof. Ignores
    /// any unchecked tail. `conclusion` is spliced in verbatim (e.g.
    /// `"NONE"`, `"UNSAT"`, `"BOUNDS 0 10"`), validated by the real
    /// checker. The captured `String` is whatever the checker printed
    /// during the run.
    pub fn dry_run_conclusion(&self, conclusion: &str) -> anyhow::Result<(String, Result<(), String>)> {
        let mut text = self.preamble_and_lines(&self.buffer[..self.checked_len]);
        text.push_str("output NONE;\n");
        text.push_str(&format!("conclusion {conclusion};\n"));
        text.push_str("end pseudo-Boolean proof;\n");

        match self.check(&text, None)? {
            CheckOutcome::Accepted { trace } => Ok((trace, Ok(()))),
            CheckOutcome::Rejected { message, trace, .. } => Ok((trace, Err(message))),
        }
    }

    /// Non-destructively re-derives proof line `display_line` alone, with
    /// tracing scoped to just that line. Only replays through
    /// `display_line`, not the whole buffer.
    pub fn explain_line(&self, display_line: usize) -> anyhow::Result<Result<String, String>> {
        let Some(idx) = self.checked_index(display_line) else {
            return Ok(Err(unchecked_or_out_of_range(self, display_line)));
        };

        let text = self.preamble_and_lines(&self.buffer[..=idx]);
        let formula_file = self.formula_temp_file()?;
        match checker::explain_line(formula_file.path(), &text)? {
            CheckOutcome::Accepted { trace } => Ok(Ok(trace)),
            CheckOutcome::Rejected { message, .. } => Ok(Err(format!(
                "internal error: replaying line {display_line}, which should already be \
                 checked, was rejected: {message}"
            ))),
        }
    }

    /// Non-destructively computes the minimized set of hints the checker
    /// needed to derive proof line `display_line` (must be a `rup` step).
    /// Only replays through `display_line`.
    pub fn rup_needed_hints(&self, display_line: usize) -> anyhow::Result<Result<Vec<checker::RupHint>, String>> {
        let Some(idx) = self.checked_index(display_line) else {
            return Ok(Err(unchecked_or_out_of_range(self, display_line)));
        };

        let text = self.preamble_and_lines(&self.buffer[..=idx]);
        let formula_file = self.formula_temp_file()?;
        match checker::why_rup(formula_file.path(), &text)? {
            Some(hints) => Ok(Ok(hints)),
            None => Ok(Err(format!(
                "line {display_line} doesn't look like a `rup` step — nothing to explain with \
                 :why."
            ))),
        }
    }
}

/// Returns the shared "nothing to explain there" message for
/// `explain_line`/`rup_needed_hints`, distinguishing an out-of-range/
/// preamble line from one sitting unchecked.
fn unchecked_or_out_of_range(session: &Session, display_line: usize) -> String {
    match session.buffer_index(display_line) {
        None => format!(
            "line {display_line} is the synthesized preamble or past the end of the buffer — \
             nothing to explain there."
        ),
        Some(_) => format!(
            "line {display_line} hasn't been checked yet — :verify it first (or fix it with \
             :edit if something's wrong)."
        ),
    }
}

/// Reads `path` as an OPB formula, returning its constraint lines (in
/// order), its `min:`/`max:` objective line if it has one, and its
/// `preserved: ...;` line if it declares a preserved variable set. Comment
/// lines (starting with `*`) are dropped; nothing else is parsed or
/// validated.
fn read_formula_lines(path: &str) -> anyhow::Result<(Vec<String>, Option<String>, Option<String>)> {
    let text = std::fs::read_to_string(path)?;
    let mut formula = Vec::new();
    let mut objective = None;
    let mut preserved = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('*') {
            continue;
        }
        if trimmed.starts_with("min:") || trimmed.starts_with("max:") {
            objective = Some(trimmed.to_string());
        } else if trimmed.starts_with("preserved:") {
            preserved = Some(trimmed.to_string());
        } else {
            formula.push(trimmed.to_string());
        }
    }
    Ok((formula, objective, preserved))
}

/// Writes `formula`/`objective`/`preserved` to a fresh temp `.opb` file —
/// shared by [`Session::formula_temp_file`] and
/// [`Session::replace_formula_constraint`]'s candidate validation.
fn write_formula_temp_file(
    formula: &[String],
    objective: &Option<String>,
    preserved: &Option<String>,
) -> anyhow::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::Builder::new()
        .suffix(".opb")
        .tempfile()
        .context("failed to create a temp file for the formula")?;
    if let Some(objective) = objective {
        writeln!(file, "{objective}").context("failed to write the temp formula file")?;
    }
    if let Some(preserved) = preserved {
        writeln!(file, "{preserved}").context("failed to write the temp formula file")?;
    }
    for line in formula {
        writeln!(file, "{line}").context("failed to write the temp formula file")?;
    }
    Ok(file)
}

/// Returns whether `line` uses a bare `=` relational operator rather than
/// `>=`/`<=`.
fn is_equality_constraint(line: &str) -> bool {
    !line.contains(">=") && !line.contains("<=") && line.contains('=')
}
