//! Session state (formula + proof buffer) and the replay machinery every
//! checking command uses. No `:command` parsing/printing lives here.
//!
//! Rules:
//! - No checker state is kept between calls: every check re-runs `veripb`
//!   (via [`crate::checker`]) on the current formula plus candidate text, so
//!   retracting `checked_len` is pure bookkeeping.
//! - `buffer` holds every proof line, checked or not; `buffer[..checked_len]`
//!   is verified. Only an explicit delete shrinks it; a failed check never does.
//! - Editing a line (`set_line`/`insert_line`/`delete_line`) never checks;
//!   it retracts `checked_len` if the edit is inside the checked prefix.
//!   Checking (`drive_forward` etc.) walks forward from `checked_len` and
//!   stops at the first failing line.
//! - The formula is held as text (`formula`, `objective`, `preserved`) and
//!   written to a fresh temp file for each checker call; `formula_path` is
//!   never re-read after load.

use std::{
    collections::{BTreeSet, VecDeque},
    io::Write as _,
};

use anyhow::Context as _;

use crate::checker::{self, CheckOutcome};
use crate::varnames::VarNames;

/// Lines in a synthesized v3 proof preamble.
const PREAMBLE_LINES: usize = 2;

/// One `:undo`-stack entry: the state `restore_snapshot` restores.
pub(crate) struct Snapshot {
    formula: Vec<String>,
    objective: Option<String>,
    preserved: Option<String>,
    buffer: Vec<String>,
    checked_len: usize,
    known_bad: Option<String>,
    last_step_constraint_ids: Vec<usize>,
}

/// Maximum `Session::undo_stack` entries; the oldest is dropped beyond this.
/// Each entry clones the whole buffer, so the stack must stay bounded.
const UNDO_STACK_CAP: usize = 200;

/// A `:formula` edit's undo point. Set by `replace_formula_constraint`,
/// consumed by `:formula cancel`, invalidated by any other command (see
/// `commands::dispatch`).
pub(crate) struct FormulaEditSnapshot {
    /// 1-based formula constraint number that was edited.
    pub(crate) n: usize,
    /// The replaced constraint text, verbatim.
    pub(crate) constraint: String,
    /// The base buffer the edit reverifies against (see
    /// `Session::recoverable_buffer`).
    pub(crate) buffer: Vec<String>,
}

/// One REPL session: the loaded formula and the proof buffer (see the module
/// docs).
pub struct Session {
    /// The formula's source path, for display only.
    pub formula_path: String,
    /// The formula's constraint lines; index `i` is constraint `i + 1` in
    /// `:formula`/the Formula pane.
    pub formula: Vec<String>,
    /// The formula's `min:`/`max:` objective line, verbatim, if any.
    pub objective: Option<String>,
    /// The formula's `preserved: <var> ...;` line, verbatim, if any.
    pub preserved: Option<String>,
    pub variables: VarNames,
    /// Always empty: `@label`s are not yet correlated with constraint IDs.
    pub labels: ahash::AHashMap<String, isize>,
    pub label_map: ahash::AHashMap<String, isize>,
    /// Every proof line, in order, checked or not. A failed check never
    /// truncates it.
    pub buffer: Vec<String>,
    /// Number of verified lines at the front of `buffer`; also the index of
    /// the first unchecked line.
    pub checked_len: usize,
    /// The rejection message for `buffer[checked_len]`, when it is known to
    /// fail. Cleared by anything that changes `checked_len` or that line.
    /// Drives the Proof pane's error highlight and `:list`'s failure tag.
    pub known_bad: Option<String>,
    /// Constraint IDs derived by the most recently accepted line (see
    /// [`checker::parse::last_line_constraint_ids`]). Set on acceptance;
    /// cleared by every mutation that isn't a fresh acceptance. Used by
    /// `:debug`'s Database-pane highlight.
    pub last_step_constraint_ids: Vec<usize>,
    /// The most recent `:formula` edit, consumed by `:formula cancel`.
    pub(crate) last_formula_edit: Option<FormulaEditSnapshot>,
    /// The fullest buffer to retry across successive `:formula` edits, so a
    /// bad edit doesn't lose lines a later edit could recover. `None` means
    /// use `buffer`. Set by `replace_formula_constraint`; cleared by a fully
    /// successful reverify (`commands::formula::commit`), `:formula cancel`,
    /// or any other command (see `commands::dispatch`).
    pub(crate) recoverable_buffer: Option<Vec<String>>,
    /// Bumped on every successful mutation of `formula`/`objective`/`buffer`/
    /// `checked_len`. `commands::dispatch` uses it to decide whether to push
    /// an undo entry; `database_cache` is refreshed only when it changes.
    pub(crate) generation: u64,
    /// Pre-action snapshots, most recent last; capped at `UNDO_STACK_CAP`.
    pub(crate) undo_stack: VecDeque<Snapshot>,
    /// Buffer indices with a breakpoint. Not proof state: excluded from
    /// `Snapshot` and `generation`, ignored by `:verify`. Shifted by
    /// [`Self::insert_line`]/[`Self::delete_line`]; cleared by
    /// [`Self::reset`].
    pub breakpoints: BTreeSet<usize>,
    /// The database for `(generation, buffer[..checked_len])`, refreshed
    /// lazily by [`Self::database`]. A `RefCell` so read-only (`&Session`)
    /// contexts can populate it.
    database_cache: std::cell::RefCell<Option<(u64, checker::Database)>>,
}

/// What happened when a freshly-typed line was handed to [`Session::append_line`].
pub enum AppendOutcome {
    /// Nothing was pending; `line` was checked immediately and accepted.
    Verified { captured: String },
    /// Nothing was pending; `line` was checked immediately and rejected. It
    /// is not added to the buffer and state is unchanged.
    Rejected { captured: String, error: String },
    /// Lines were already pending; `line` was appended unchecked.
    Deferred,
}

impl Session {
    /// Loads an OPB formula, checks it with `veripb` (see
    /// [`Self::check_formula`]), and starts an empty session. Fails, with
    /// veripb's message, if the formula has an error.
    pub fn load_checked(formula_path: &str) -> anyhow::Result<Self> {
        let session = Self::load(formula_path)?;
        session.check_formula()?;
        Ok(session)
    }

    /// Runs `veripb` on the original formula file with an empty proof.
    /// Errors (with line/column in that file) if veripb rejects the formula.
    pub fn check_formula(&self) -> anyhow::Result<()> {
        let empty_proof = "pseudo-Boolean proof version 3.0\n";
        match checker::check(std::path::Path::new(&self.formula_path), empty_proof, None)? {
            CheckOutcome::Accepted { .. } => Ok(()),
            CheckOutcome::Rejected { message, .. } => {
                anyhow::bail!("veripb rejected the formula: {message}")
            }
        }
    }

    /// Loads an OPB formula and starts an empty session, without checking
    /// it; see [`Self::load_checked`].
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
            last_step_constraint_ids: Vec::new(),
            last_formula_edit: None,
            recoverable_buffer: None,
            generation: 0,
            undo_stack: VecDeque::new(),
            breakpoints: BTreeSet::new(),
            database_cache: std::cell::RefCell::new(None),
        })
    }

    /// Returns `label_map` inverted: each constraint ID's labels, sorted.
    /// Currently always empty (see `labels`).
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

    /// Returns the synthesized preamble every replayed proof starts with.
    /// Display line numbers count these lines, matching the checker's
    /// `line N:` trace.
    pub fn preamble_lines(&self) -> [String; PREAMBLE_LINES] {
        [
            "pseudo-Boolean proof version 3.0".to_string(),
            format!("f {};", self.formula.len()),
        ]
    }

    /// Returns the preamble followed by `lines`, newline-terminated, ready
    /// for [`checker::check`] and friends.
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

    /// Writes the current formula to a fresh temp `.opb` file.
    fn formula_temp_file(&self) -> anyhow::Result<tempfile::NamedTempFile> {
        write_formula_temp_file(&self.formula, &self.objective, &self.preserved)
    }

    /// Checks `text` against the current formula, tracing `trace_range`
    /// (display lines, inclusive).
    fn check(
        &self,
        text: &str,
        trace_range: Option<(usize, usize)>,
    ) -> anyhow::Result<CheckOutcome> {
        let formula_file = self.formula_temp_file()?;
        checker::check(formula_file.path(), text, trace_range)
    }

    /// Returns the live database for `buffer[..checked_len]`, re-running
    /// `veripb` only if `generation` changed since it was cached. Unchecked
    /// lines are not reflected. Takes `&self` so read-only contexts can call
    /// it; the returned `Ref` derefs to `checker::Database`.
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

    /// Returns the best-known objective bounds for `buffer[..checked_len]`.
    /// Uncached: one `veripb` call per call.
    pub fn objective_bounds(&self) -> anyhow::Result<checker::ObjectiveBounds> {
        let text = self.preamble_and_lines(&self.buffer[..self.checked_len]);
        let formula_file = self.formula_temp_file()?;
        checker::show_objective_bounds(formula_file.path(), &text)
    }

    /// Returns the preserved set as of the checked prefix: the declared set
    /// if no checked `preserved_add`/`preserved_rm` line exists (no
    /// subprocess), otherwise the `Preserved set:` line veripb traces for
    /// the last such line.
    ///
    /// TODO: read veripb's trace only until it has a `--dump-preserved` flag.
    pub fn preserved_set(&self) -> anyhow::Result<PreservedSet> {
        let declared = self
            .preserved
            .as_deref()
            .map(checker::parse::preserved_declaration);
        let Some(idx) = self.buffer[..self.checked_len]
            .iter()
            .rposition(|line| checker::parse::is_preserved_change_line(line))
        else {
            return Ok(PreservedSet {
                current: declared.clone(),
                declared,
                last_change: None,
            });
        };

        let line = self.display_line(idx);
        let text = self.preamble_and_lines(&self.buffer[..=idx]);
        let current = match self.check(&text, Some((line, line)))? {
            CheckOutcome::Accepted { trace } => checker::parse::last_preserved_set(&trace)
                .with_context(|| {
                    format!(
                        "veripb printed no `Preserved set:` line for line {line} — its trace \
                         format may have changed"
                    )
                })?,
            CheckOutcome::Rejected { message, .. } => anyhow::bail!(
                "internal error: replaying line {line}, which should already be checked, was \
                 rejected: {message}"
            ),
        };
        Ok(PreservedSet {
            declared,
            current: Some(current),
            last_change: Some(line),
        })
    }

    /// Checks `buffer[checked_len]`. Acceptance advances `checked_len` by
    /// one; rejection sets `known_bad` and changes nothing else. Returns the
    /// trace and the rejection message, if any. Panics if
    /// `checked_len == buffer.len()`.
    fn verify_next(&mut self) -> anyhow::Result<(String, Option<String>)> {
        let candidate_line = self.buffer[self.checked_len].clone();
        let mut text = self.preamble_and_lines(&self.buffer[..self.checked_len]);
        text.push_str(&candidate_line);
        text.push('\n');

        // Trace only the new line; the prefix was traced when accepted.
        let candidate_line_no = PREAMBLE_LINES + self.checked_len + 1;
        let outcome = self.check(&text, Some((candidate_line_no, candidate_line_no)))?;

        match outcome {
            CheckOutcome::Accepted { trace } => {
                self.checked_len += 1;
                self.known_bad = None;
                self.last_step_constraint_ids = checker::parse::last_line_constraint_ids(&trace);
                self.generation += 1;
                Ok((trace, None))
            }
            CheckOutcome::Rejected { message, trace, .. } => {
                self.known_bad = Some(message.clone());
                Ok((trace, Some(message)))
            }
        }
    }

    /// Advances `checked_len` to `boundary` (`<= buffer.len()`) with one
    /// batched `veripb` call. If the batch is rejected, its output is
    /// discarded and [`Self::verify_next`] runs line by line up to
    /// `boundary` to find the failing line. No-op returning an empty
    /// capture if `boundary <= checked_len`.
    fn verify_forward(&mut self, boundary: usize) -> anyhow::Result<(String, Option<String>)> {
        if boundary <= self.checked_len {
            return Ok((String::new(), None));
        }

        let candidate_start = PREAMBLE_LINES + self.checked_len + 1;
        let candidate_end = PREAMBLE_LINES + boundary;
        let text = self.preamble_and_lines(&self.buffer[..boundary]);
        let outcome = self.check(&text, Some((candidate_start, candidate_end)))?;
        if let CheckOutcome::Accepted { trace } = outcome {
            self.checked_len = boundary;
            self.known_bad = None;
            self.last_step_constraint_ids = checker::parse::last_line_constraint_ids(&trace);
            self.generation += 1;
            return Ok((trace, None));
        }

        let mut captured_all = String::new();
        while self.checked_len < boundary {
            let (captured, rejection) = self.verify_next()?;
            captured_all.push_str(&captured);
            if let Some(err) = rejection {
                return Ok((captured_all, Some(err)));
            }
        }
        Ok((captured_all, None))
    }

    /// Checks forward through the whole buffer (`verify_forward` to
    /// `buffer.len()`). Implements `:verify`; ignores breakpoints.
    pub fn drive_forward(&mut self) -> anyhow::Result<(String, Option<String>)> {
        self.verify_forward(self.buffer.len())
    }

    /// `:debug`'s `:step`: checks the next unchecked line. `Ok(None)` if
    /// nothing is unchecked.
    pub fn step(&mut self) -> anyhow::Result<Option<(String, Option<String>)>> {
        if self.checked_len >= self.buffer.len() {
            return Ok(None);
        }
        self.verify_next().map(Some)
    }

    /// `:debug`'s `:continue`/`:until <n>`: `verify_forward` up to
    /// the smallest of `buffer.len()`, `target`, and the first breakpoint
    /// after `checked_len` (strictly after, so resuming from a breakpoint
    /// makes progress).
    pub fn continue_run(
        &mut self,
        target: Option<usize>,
    ) -> anyhow::Result<(String, Option<String>)> {
        let next_breakpoint = self
            .breakpoints
            .range((self.checked_len + 1)..)
            .next()
            .copied();
        let boundary = [Some(self.buffer.len()), target, next_breakpoint]
            .into_iter()
            .flatten()
            .min()
            .expect("buffer.len() is always Some");
        self.verify_forward(boundary)
    }

    /// Appends a typed line. If nothing was pending, checks it immediately
    /// and drops it on rejection; otherwise appends it unchecked.
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

    /// Replaces `buffer[idx]` without checking it. Retracts `checked_len` to
    /// `idx` if `idx` was checked.
    pub fn set_line(&mut self, idx: usize, new_text: &str) -> anyhow::Result<()> {
        self.buffer[idx] = new_text.to_string();
        self.checked_len = self.checked_len.min(idx);
        self.known_bad = None;
        self.last_step_constraint_ids = Vec::new();
        self.generation += 1;
        Ok(())
    }

    /// Inserts an unchecked line before buffer index `idx`, shifting later
    /// lines and breakpoints. Retracts `checked_len` as [`Self::set_line`].
    pub fn insert_line(&mut self, idx: usize, text: &str) -> anyhow::Result<()> {
        self.buffer.insert(idx, text.to_string());
        self.checked_len = self.checked_len.min(idx);
        self.breakpoints = self
            .breakpoints
            .iter()
            .map(|&b| if b >= idx { b + 1 } else { b })
            .collect();
        self.known_bad = None;
        self.last_step_constraint_ids = Vec::new();
        self.generation += 1;
        Ok(())
    }

    /// Removes buffer index `idx` and its breakpoint, shifting later
    /// breakpoints. Retracts `checked_len` as [`Self::set_line`].
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
        self.last_step_constraint_ids = Vec::new();
        self.generation += 1;
        Ok(())
    }

    /// Toggles a breakpoint on buffer index `idx`. Doesn't bump
    /// `generation`.
    pub fn toggle_breakpoint(&mut self, idx: usize) {
        if !self.breakpoints.remove(&idx) {
            self.breakpoints.insert(idx);
        }
    }

    /// Replaces the buffer with `lines` and checks from the start as far as
    /// it holds.
    pub(crate) fn replace_buffer_and_verify(
        &mut self,
        lines: Vec<String>,
    ) -> anyhow::Result<(String, Option<String>)> {
        self.buffer = lines;
        self.checked_len = 0;
        self.known_bad = None;
        self.last_step_constraint_ids = Vec::new();
        self.verify_forward(self.buffer.len())
    }

    /// Restores `buffer`/`checked_len`/`known_bad` as given, leaving the
    /// formula alone (edit commands' `:cancel`). Clears
    /// `last_step_constraint_ids`.
    pub(crate) fn restore_buffer_state(
        &mut self,
        buffer: Vec<String>,
        checked_len: usize,
        known_bad: Option<String>,
    ) -> anyhow::Result<()> {
        self.buffer = buffer;
        self.checked_len = checked_len;
        self.known_bad = known_bad;
        self.last_step_constraint_ids = Vec::new();
        self.generation += 1;
        Ok(())
    }

    /// Clears the buffer, breakpoints, and undo stack, leaving
    /// `formula`/`variables`/`labels` alone.
    pub fn reset(&mut self) -> anyhow::Result<()> {
        self.buffer.clear();
        self.checked_len = 0;
        self.known_bad = None;
        self.last_step_constraint_ids = Vec::new();
        self.label_map = self.labels.clone();
        self.breakpoints.clear();
        self.generation += 1;
        self.undo_stack.clear();
        Ok(())
    }

    /// Retracts `checked_len` to `target` (`<= checked_len`) without
    /// changing any text.
    fn retract_checked_len(&mut self, target: usize) -> anyhow::Result<()> {
        debug_assert!(target <= self.checked_len);
        self.checked_len = target;
        self.known_bad = None;
        self.last_step_constraint_ids = Vec::new();
        self.generation += 1;
        Ok(())
    }

    /// `:back`: retracts `checked_len` by one line, without re-verifying.
    /// Returns whether it moved.
    pub fn step_back(&mut self) -> anyhow::Result<bool> {
        if self.checked_len == 0 {
            return Ok(false);
        }
        self.retract_checked_len(self.checked_len - 1)?;
        Ok(true)
    }

    /// `:restart`: retracts `checked_len` to 0, keeping the buffer (unlike
    /// [`Self::reset`]). No-op if nothing is checked.
    pub fn restart(&mut self) -> anyhow::Result<()> {
        if self.checked_len == 0 {
            return Ok(());
        }
        self.retract_checked_len(0)
    }

    /// Converts a 1-based display line number (preamble included, as in the
    /// checker's `line N:` trace) to a `buffer` index. `None` for a preamble
    /// line or past the end. Ignores checked status; see
    /// [`Self::checked_index`].
    pub fn buffer_index(&self, display_line: usize) -> Option<usize> {
        let idx = display_line.checked_sub(PREAMBLE_LINES + 1)?;
        (idx < self.buffer.len()).then_some(idx)
    }

    /// Like [`Self::buffer_index`], but also `None` for an unchecked line.
    pub fn checked_index(&self, display_line: usize) -> Option<usize> {
        let idx = self.buffer_index(display_line)?;
        (idx < self.checked_len).then_some(idx)
    }

    /// Inverse of [`Self::buffer_index`]: the display line number for a
    /// `buffer` index.
    pub fn display_line(&self, buffer_idx: usize) -> usize {
        buffer_idx + PREAMBLE_LINES + 1
    }

    /// Converts a 1-based formula constraint number (as in `:show`/the
    /// Formula pane) to a `formula` index. `None` if out of range.
    pub(crate) fn formula_index(&self, n: usize) -> Option<usize> {
        (n >= 1 && n <= self.formula.len()).then(|| n - 1)
    }

    /// Captures the current state for [`Self::restore_snapshot`].
    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot {
            formula: self.formula.clone(),
            objective: self.objective.clone(),
            preserved: self.preserved.clone(),
            buffer: self.buffer.clone(),
            checked_len: self.checked_len,
            known_bad: self.known_bad.clone(),
            last_step_constraint_ids: self.last_step_constraint_ids.clone(),
        }
    }

    /// Pushes `snapshot` onto the undo stack, dropping the oldest entry at
    /// `UNDO_STACK_CAP`.
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

    /// Restores the state `snapshot` captured and clears
    /// `last_formula_edit`/`recoverable_buffer`.
    pub(crate) fn restore_snapshot(&mut self, snapshot: Snapshot) -> anyhow::Result<()> {
        self.formula = snapshot.formula;
        self.objective = snapshot.objective;
        self.preserved = snapshot.preserved;
        self.buffer = snapshot.buffer;
        self.checked_len = snapshot.checked_len;
        self.known_bad = snapshot.known_bad;
        self.last_step_constraint_ids = snapshot.last_step_constraint_ids;
        self.last_formula_edit = None;
        self.recoverable_buffer = None;
        self.generation += 1;
        Ok(())
    }

    /// Replaces formula constraint `n` (1-based) with `new_text`. The new
    /// formula is validated first; on error `self` is unchanged. On success
    /// the buffer is cleared and the base buffer (`recoverable_buffer`, else
    /// `buffer`) is stored in `recoverable_buffer`/`last_formula_edit`; the
    /// caller reverifies it via `replace_buffer_and_verify`.
    ///
    /// Rejects `=` constraints: OPB splits them in two, changing the
    /// constraint count.
    pub fn replace_formula_constraint(&mut self, n: usize, new_text: &str) -> anyhow::Result<()> {
        let idx = self.formula_index(n).ok_or_else(|| {
            anyhow::anyhow!(
                "constraint {n} is out of range — the formula has {} constraint(s) (1-{}).",
                self.formula.len(),
                self.formula.len()
            )
        })?;

        // Add the trailing `;` if omitted.
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
        let candidate_file =
            write_formula_temp_file(&candidate_formula, &self.objective, &self.preserved)?;
        let empty_proof = format!(
            "pseudo-Boolean proof version 3.0\nf {};\n",
            candidate_formula.len()
        );
        let outcome = checker::check(candidate_file.path(), &empty_proof, None)
            .context("failed to validate the edited formula")?;
        if let CheckOutcome::Rejected { message, .. } = outcome {
            anyhow::bail!("the edited formula doesn't check out: {message}");
        }

        // Pick up any new variable names in `new_text`.
        let candidate_vars = VarNames::from_formula_file(candidate_file.path())
            .context("failed to re-scan variable names after the formula edit")?;

        // Prefer the fuller buffer an earlier, partially reverified edit kept.
        let base_buffer = match &self.recoverable_buffer {
            Some(b) => b.clone(),
            None => self.buffer.clone(),
        };

        // Nothing below this line can fail — commit as one unit.
        let prior_constraint = std::mem::replace(&mut self.formula[idx], new_text);
        self.buffer.clear();
        self.checked_len = 0;
        self.known_bad = None;
        self.last_step_constraint_ids = Vec::new();
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

    /// Returns the full `.pbp` text: preamble, the whole buffer (checked or
    /// not), and `output NONE; conclusion <conclusion>; end pseudo-Boolean
    /// proof;`. Not checked; see [`Self::dry_run_conclusion`].
    pub fn listing(&self, conclusion: &str) -> String {
        let mut text = self.preamble_and_lines(&self.buffer);
        text.push_str("output NONE;\n");
        text.push_str(&format!("conclusion {conclusion};\n"));
        text.push_str("end pseudo-Boolean proof;\n");
        text
    }

    /// Checks whether the checked prefix plus `output NONE; conclusion
    /// <conclusion>; end pseudo-Boolean proof;` is a complete valid proof,
    /// without changing state. `conclusion` is spliced verbatim (e.g.
    /// `"UNSAT"`, `"BOUNDS 0 10"`). Returns the trace and the result.
    pub fn dry_run_conclusion(
        &self,
        conclusion: &str,
    ) -> anyhow::Result<(String, Result<(), String>)> {
        let mut text = self.preamble_and_lines(&self.buffer[..self.checked_len]);
        text.push_str("output NONE;\n");
        text.push_str(&format!("conclusion {conclusion};\n"));
        text.push_str("end pseudo-Boolean proof;\n");

        match self.check(&text, None)? {
            CheckOutcome::Accepted { trace } => Ok((trace, Ok(()))),
            CheckOutcome::Rejected { message, trace, .. } => Ok((trace, Err(message))),
        }
    }

    /// Replays through checked line `display_line` and returns its trace.
    /// The inner `Err` is a user-facing message.
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

    /// Returns the minimized hints the checker needed for checked `rup`
    /// line `display_line`. The inner `Err` is a user-facing message.
    pub fn rup_needed_hints(
        &self,
        display_line: usize,
    ) -> anyhow::Result<Result<Vec<checker::RupHint>, String>> {
        let Some(idx) = self.checked_index(display_line) else {
            return Ok(Err(unchecked_or_out_of_range(self, display_line)));
        };

        let text = self.preamble_and_lines(&self.buffer[..=idx]);
        let formula_file = self.formula_temp_file()?;
        match checker::elaborate_rup(formula_file.path(), &text)? {
            Ok(Some(hints)) => Ok(Ok(hints)),
            Ok(None) => Ok(Err(format!(
                "line {display_line} doesn't look like a `rup` step — no hints to show."
            ))),
            Err(message) => Ok(Err(format!(
                "internal error: replaying line {display_line}, which should already be \
                 checked, was rejected: {message}"
            ))),
        }
    }

    /// Diagnoses a rejected `rup` line at `buffer[checked_len]`: typed hint
    /// IDs missing from the database, and the result of re-checking it with
    /// no hints. `None` unless `known_bad` is set and the line is `rup`.
    pub fn diagnose_rejected_rup(&self) -> anyhow::Result<Option<RejectionDiagnosis>> {
        if self.known_bad.is_none() {
            return Ok(None);
        }
        let Some(line) = self.buffer.get(self.checked_len) else {
            return Ok(None);
        };
        if !checker::parse::is_rup_line(line) {
            return Ok(None);
        }

        let (prefix, typed_hints) = checker::parse::split_rup_hints(line);
        let typed_hints = typed_hints.unwrap_or_default();

        let database = self.database()?;
        let missing_ids = typed_hints
            .iter()
            .filter_map(|hint| match hint {
                checker::RupHint::ConstraintId(id) => Some(*id),
                checker::RupHint::NegatedPremise => None,
            })
            .filter(|id| database.get(*id).is_none())
            .collect();
        drop(database);

        let mut text = self.preamble_and_lines(&self.buffer[..self.checked_len]);
        text.push_str(&format!("{prefix} ;\n"));
        let formula_file = self.formula_temp_file()?;
        let without_hints = match checker::elaborate_rup(formula_file.path(), &text)? {
            Ok(hints) => Ok(hints.unwrap_or_default()),
            Err(message) => Err(message),
        };

        Ok(Some(RejectionDiagnosis {
            display_line: self.display_line(self.checked_len),
            typed_hints,
            missing_ids,
            without_hints,
        }))
    }
}

/// [`Session::preserved_set`]'s result. Variable names are sorted.
pub struct PreservedSet {
    /// The formula's `preserved:` line, `None` if it has none.
    pub declared: Option<Vec<String>>,
    /// The set as of the checked prefix.
    pub current: Option<Vec<String>>,
    /// The display line of the last `preserved_add`/`preserved_rm` that
    /// changed it, `None` if nothing has.
    pub last_change: Option<usize>,
}

/// [`Session::diagnose_rejected_rup`]'s result.
pub struct RejectionDiagnosis {
    /// The rejected line's display line number.
    pub display_line: usize,
    /// The hints typed after the line's `:` (empty if it had none).
    pub typed_hints: Vec<checker::RupHint>,
    /// Typed `ConstraintId`s not in the live database.
    pub missing_ids: Vec<usize>,
    /// Re-checking the same constraint with no hints: `Ok` with the hints
    /// the checker then needed, or `Err` with its rejection message.
    pub without_hints: Result<Vec<checker::RupHint>, String>,
}

/// Returns the error message for a display line that isn't a checked buffer
/// line (preamble/out of range vs. unchecked).
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

/// Reads an OPB file as `(constraint lines, objective line, preserved
/// line)`, trimmed. Drops blank and `*` comment lines; validates nothing.
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

/// Writes `objective`, `preserved`, then `formula` to a fresh temp `.opb`
/// file.
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
