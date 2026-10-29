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

mod line_ids;
mod text;
use line_ids::LineIds;
use text::Hunk;
pub use text::{FormulaLines, ProofBuffer};

use crate::checker::{self, CheckOutcome, ProofText};
use crate::varnames::VarNames;

/// Lines in a synthesized v3 proof preamble.
const PREAMBLE_LINES: usize = 2;

/// The most lines a batched check (`:verify`, `:continue`) traces in full.
/// veripb's trace runs to several times the proof's size, and is read whole
/// and printed; past this, only the batch's last line is traced (what
/// `last_step_constraint_ids` needs).
pub const FULL_TRACE_MAX_LINES: usize = 10_000;

/// The most separate line ranges `database()` passes to `--trace-lines`
/// (each a few bytes of veripb's command line); past this, one range from
/// the first labelled line to the last.
const MAX_TRACE_RANGES: usize = 1_000;

/// What an `:undo` or `:redo` entry restores besides the lines.
struct PointState {
    checked_len: usize,
    known_bad: Option<String>,
    last_step_constraint_ids: Vec<usize>,
    intended_conclusion: Option<String>,
}

/// One `:undo` or `:redo` entry: a state to return to, and the hunks that
/// take the lines there from the state of the entry above it. The top
/// `:undo` entry's hunks are the ones `buffer` and `formula` are recording,
/// from the current lines.
struct UndoEntry {
    state: PointState,
    buffer: Hunk,
    formula: Hunk,
    /// `buffer.bytes() + formula.bytes()`.
    bytes: usize,
}

impl UndoEntry {
    fn new(state: PointState, buffer: Hunk, formula: Hunk) -> Self {
        let bytes = buffer.bytes() + formula.bytes();
        UndoEntry {
            state,
            buffer,
            formula,
            bytes,
        }
    }
}

/// An action's undo point, from [`Session::begin_undo_point`] until
/// [`Session::end_undo_point`] decides whether to keep it.
struct PendingPoint {
    stamp: (u64, u64),
    state: PointState,
    /// The changes recorded before the action, which belong to the entry
    /// below.
    buffer: Hunk,
    formula: Hunk,
}

/// Maximum `Session::undo_stack` entries; the oldest is dropped beyond this.
const UNDO_STACK_CAP: usize = 200;

/// Most bytes of replaced lines the undo stack keeps (see [`Hunk::bytes`]);
/// the oldest entries are dropped beyond this, though never the newest.
const UNDO_BYTES_CAP: usize = 256 << 20;

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
    /// Unique to this session in the process; see [`Self::state_key`].
    id: u64,
    /// The formula's source path, for display only.
    pub formula_path: String,
    /// The formula's constraint lines, verbatim (including any leading
    /// `@label`s). A line may load as two constraints (see
    /// `checker::parse::formula_line_constraints`); [`Self::formula_first_ids`]
    /// gives each line's first constraint ID.
    pub formula: FormulaLines,
    /// The formula's `min:`/`max:` objective line, verbatim, if any.
    pub objective: Option<String>,
    /// The formula's `preserved: <var> ...;` line, verbatim, if any.
    pub preserved: Option<String>,
    pub variables: VarNames,
    /// Every proof line, in order, checked or not. A failed check never
    /// truncates it.
    pub buffer: ProofBuffer,
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
    /// The conclusion the proof means to reach (e.g. `BOUNDS 23 23`), which
    /// bare `:check` and `:save` try first: the sourced file's own, or one
    /// set with `:conclusion`. Whitespace-normalized, without `;`. Set by
    /// [`Self::set_intended_conclusion`].
    pub intended_conclusion: Option<String>,
    /// The most recent `:formula` edit, consumed by `:formula cancel`.
    pub(crate) last_formula_edit: Option<FormulaEditSnapshot>,
    /// The fullest buffer to retry across successive `:formula` edits, so a
    /// bad edit doesn't lose lines a later edit could recover. `None` means
    /// use `buffer`. Set by `replace_formula_constraint`; cleared by a fully
    /// successful reverify (`commands::formula::commit`), `:formula cancel`,
    /// or any other command (see `commands::dispatch`).
    pub(crate) recoverable_buffer: Option<Vec<String>>,
    /// Bumped on every successful mutation of `formula`/`objective`/`buffer`/
    /// `checked_len`. `database_cache` is refreshed only when it changes.
    pub(crate) generation: u64,
    /// Bumped by [`Self::mark_undo_point`]: a command that starts a mode
    /// changes nothing, but its start is still an undo point. See
    /// [`Self::change_stamp`].
    undo_marker: u64,
    /// The states before each undoable action, most recent last; capped at
    /// `UNDO_STACK_CAP` entries and `UNDO_BYTES_CAP` bytes.
    undo_stack: VecDeque<UndoEntry>,
    /// The states `:undo` left, most recently undone last.
    redo_stack: Vec<UndoEntry>,
    /// [`Self::change_stamp`] after the last `:undo` or `:redo`; any other
    /// change since makes `redo_stack` stale.
    redo_stamp: (u64, u64),
    pending_point: Option<PendingPoint>,
    /// Buffer indices with a breakpoint. Not proof state: excluded from
    /// undo entries and `generation`, ignored by `:verify`. Shifted by
    /// [`Self::insert_line`]/[`Self::delete_line`]; cleared by
    /// [`Self::reset`].
    pub breakpoints: BTreeSet<usize>,
    /// The database and label map for `(generation, buffer[..checked_len])`,
    /// refreshed lazily by [`Self::database`]. A `RefCell` so read-only
    /// (`&Session`) contexts can populate it.
    database_cache: std::cell::RefCell<Option<DatabaseCache>>,
    /// [`Self::objective_bounds`] for a `generation`.
    objective_cache: std::cell::RefCell<Option<(u64, checker::ObjectiveBounds)>>,
    /// [`Self::dry_run_conclusion`]'s results for a `generation`, by
    /// conclusion.
    conclusion_cache: std::cell::RefCell<(u64, ahash::AHashMap<String, DryRun>)>,
    /// The lowest buffer index changed since `database_cache` was last
    /// filled (`usize::MAX` if none); see [`Self::sync_changes`].
    changed_since_database: std::cell::Cell<usize>,
    /// Set by [`Self::database`], cleared when a verify run refreshes the
    /// cache from its own dump: verify runs dump the database only while it
    /// is in use.
    database_wanted: std::cell::Cell<bool>,
    /// See [`Self::line_ids`]. A `RefCell` as `database_cache` is.
    line_ids: std::cell::RefCell<LineIds>,
    /// [`Self::formula_temp_file`]'s file, for a `FormulaLines::epoch`.
    formula_file: std::cell::RefCell<Option<(u64, std::rc::Rc<tempfile::NamedTempFile>)>>,
}

/// One refresh of [`Session::database`]: the generation it was computed
/// for, and the database with every label's constraint ID at that point, or
/// the error computing them.
struct DatabaseCache {
    generation: u64,
    /// The `checked_len` and `FormulaLines::epoch` it was computed for.
    checked_len: usize,
    epoch: u64,
    contents: DatabaseContents,
    /// [`Session::labels_by_id`], once asked for.
    labels_by_id: std::cell::OnceCell<std::rc::Rc<LabelsById>>,
    /// [`Session::live_labels`], once asked for.
    live_labels: std::cell::OnceCell<std::rc::Rc<Vec<String>>>,
}

/// Where a constraint came from (see [`Session::constraint_source`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintSource {
    /// Formula line `line` (an index into `Session::formula`).
    Formula { line: usize },
    /// The checked proof line at `display_line`.
    Proof { display_line: usize },
}

/// A verify run's database dump and, if asked for, its objective dump.
type VerifyDumps = (
    checker::Database,
    Option<anyhow::Result<checker::ObjectiveBounds>>,
);

/// [`Session::dry_run_conclusion`]'s result: the checker's output, and the
/// rejection message if the conclusion doesn't hold.
pub type DryRun = (String, Result<(), String>);

/// [`DatabaseCache::contents`]: the database and every label's constraint ID,
/// or the error computing them.
type DatabaseContents = Result<
    (
        std::rc::Rc<checker::Database>,
        ahash::AHashMap<String, isize>,
    ),
    String,
>;

/// Each constraint ID's labels, sorted.
pub type LabelsById = ahash::AHashMap<isize, Vec<String>>;

/// Source of [`Session::state_key`]'s session part.
static NEXT_SESSION_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
            id: NEXT_SESSION_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            formula_path: formula_path.to_string(),
            formula: FormulaLines::new(formula),
            objective,
            preserved,
            variables,
            buffer: ProofBuffer::default(),
            checked_len: 0,
            known_bad: None,
            last_step_constraint_ids: Vec::new(),
            intended_conclusion: None,
            last_formula_edit: None,
            recoverable_buffer: None,
            generation: 0,
            undo_marker: 0,
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
            redo_stamp: (0, 0),
            pending_point: None,
            breakpoints: BTreeSet::new(),
            database_cache: std::cell::RefCell::new(None),
            objective_cache: std::cell::RefCell::new(None),
            conclusion_cache: std::cell::RefCell::default(),
            database_wanted: std::cell::Cell::new(false),
            changed_since_database: std::cell::Cell::new(usize::MAX),
            line_ids: std::cell::RefCell::default(),
            formula_file: std::cell::RefCell::new(None),
        })
    }

    /// Returns the total number of constraints veripb loads from the
    /// formula (what `f N;` must say).
    pub fn formula_constraint_count(&self) -> usize {
        self.formula.constraint_count()
    }

    /// Returns the first constraint ID of each formula line, in order.
    pub fn formula_first_ids(&self) -> &[usize] {
        self.formula.first_ids()
    }

    /// Returns formula line `idx`'s constraint ID, or ID range for a line
    /// that loads as two constraints (`"3"`, `"1-2"`).
    pub fn formula_line_ids(&self, idx: usize) -> String {
        let first = self.formula_first_ids()[idx];
        match checker::parse::formula_line_constraints(&self.formula[idx]) {
            1 => first.to_string(),
            n => format!("{first}-{}", first + n - 1),
        }
    }

    /// Returns the labels defined in the formula (each line's leading
    /// `@label`s, the i-th naming its i-th constraint).
    fn formula_labels(&self) -> ahash::AHashMap<String, isize> {
        let mut labels = ahash::AHashMap::new();
        for (line, &first) in self.formula.iter().zip(self.formula_first_ids()) {
            for (i, label) in checker::parse::leading_labels(line).into_iter().enumerate() {
                labels.insert(label.to_string(), (first + i) as isize);
            }
        }
        labels
    }

    /// Returns every label's constraint ID for the checked prefix: formula
    /// labels, then labels on checked proof lines (a later definition
    /// replaces an earlier one). Refreshes with [`Self::database`]; if that
    /// fails, returns the formula labels only. Labels on `e` lines (which
    /// name an existing constraint rather than derive one) are not tracked.
    pub fn label_ids(&self) -> ahash::AHashMap<String, isize> {
        match self.database() {
            Ok(_) => match self.database_cache.borrow().as_ref().map(|c| &c.contents) {
                Some(Ok((_, labels))) => labels.clone(),
                _ => self.formula_labels(),
            },
            Err(_) => self.formula_labels(),
        }
    }

    /// Returns [`Self::label_ids`] inverted: each constraint ID's labels,
    /// sorted. Cached with [`Self::database`].
    pub fn labels_by_id(&self) -> std::rc::Rc<LabelsById> {
        let invert = |labels: &ahash::AHashMap<String, isize>| {
            let mut by_id = LabelsById::new();
            for (name, id) in labels {
                by_id.entry(*id).or_default().push(name.clone());
            }
            for names in by_id.values_mut() {
                names.sort();
            }
            std::rc::Rc::new(by_id)
        };
        if self.database().is_ok()
            && let Some(cache) = &*self.database_cache.borrow()
            && let Ok((_, labels)) = &cache.contents
        {
            return cache.labels_by_id.get_or_init(|| invert(labels)).clone();
        }
        invert(&self.formula_labels())
    }

    /// Returns every label naming a live constraint, sorted. Cached with
    /// [`Self::database`]; empty if that fails.
    pub fn live_labels(&self) -> std::rc::Rc<Vec<String>> {
        let Ok(database) = self.database() else {
            return std::rc::Rc::default();
        };
        let labels_by_id = self.labels_by_id();
        let cache = self.database_cache.borrow();
        let cache = cache.as_ref().expect("populated by database()");
        cache
            .live_labels
            .get_or_init(|| {
                let mut labels: Vec<String> = labels_by_id
                    .iter()
                    .filter(|(id, _)| {
                        usize::try_from(**id).is_ok_and(|id| database.get(id).is_some())
                    })
                    .flat_map(|(_, names)| names.iter().cloned())
                    .collect();
                labels.sort();
                std::rc::Rc::new(labels)
            })
            .clone()
    }

    /// Identifies the session's state: differs whenever `generation`
    /// changes, and from every other session's.
    pub fn state_key(&self) -> (u64, u64) {
        (self.id, self.generation)
    }

    /// Returns the synthesized preamble every replayed proof starts with.
    /// Display line numbers count these lines, matching the checker's
    /// `line N:` trace.
    pub fn preamble_lines(&self) -> [String; PREAMBLE_LINES] {
        [
            "pseudo-Boolean proof version 3.0".to_string(),
            format!("f {};", self.formula_constraint_count()),
        ]
    }

    /// The current formula in a temp `.opb` file, written once per
    /// `FormulaLines::epoch`.
    fn formula_temp_file(&self) -> anyhow::Result<std::rc::Rc<tempfile::NamedTempFile>> {
        let epoch = self.formula.epoch();
        let mut cached = self.formula_file.borrow_mut();
        if let Some((cached_epoch, file)) = &*cached
            && *cached_epoch == epoch
        {
            return Ok(file.clone());
        }
        let file = std::rc::Rc::new(write_formula_temp_file(
            &self.formula,
            &self.objective,
            &self.preserved,
        )?);
        *cached = Some((epoch, file.clone()));
        Ok(file)
    }

    /// Checks `proof` against the current formula, tracing `trace_range`
    /// (display lines, inclusive).
    fn check(
        &self,
        proof: ProofText,
        trace_range: Option<(usize, usize)>,
    ) -> anyhow::Result<CheckOutcome> {
        let formula_file = self.formula_temp_file()?;
        checker::check(formula_file.path(), proof, trace_range)
    }

    /// Returns the live database for `buffer[..checked_len]`, re-running
    /// `veripb` only if `generation` changed since it was cached. Unchecked
    /// lines are not reflected. Takes `&self` so read-only contexts can call
    /// it.
    pub fn database(&self) -> anyhow::Result<std::rc::Rc<checker::Database>> {
        self.database_wanted.set(true);
        let fresh = matches!(
            &*self.database_cache.borrow(),
            Some(cache) if cache.generation == self.generation
        );
        if !fresh && !self.database_from_longer_prefix() {
            self.refresh_database(&[]);
        }
        match &self
            .database_cache
            .borrow()
            .as_ref()
            .expect("just populated")
            .contents
        {
            Ok((database, _)) => Ok(database.clone()),
            Err(message) => Err(anyhow::anyhow!("{message}")),
        }
    }

    /// Recomputes the database cache, tracing buffer lines `extra` too if
    /// their IDs aren't kept (see [`Self::line_ids`]).
    fn refresh_database(&self, extra: &[usize]) {
        let contents = self
            .compute_database(extra)
            .map_err(|err| format!("{err:#}"));
        self.set_database_cache(contents);
    }

    /// Makes `contents` the database cache for the current state.
    fn set_database_cache(&self, contents: DatabaseContents) {
        self.sync_changes();
        self.changed_since_database.set(usize::MAX);
        *self.database_cache.borrow_mut() = Some(DatabaseCache {
            generation: self.generation,
            checked_len: self.checked_len,
            epoch: self.formula.epoch(),
            contents,
            labels_by_id: std::cell::OnceCell::new(),
            live_labels: std::cell::OnceCell::new(),
        });
    }

    /// Makes the database for the current state from the cached one for a
    /// longer checked prefix, without running `veripb`, if every line
    /// between only adds constraints (see
    /// [`checker::parse::constraints_added`]) and nothing before them or in
    /// the formula changed since: each such line's constraint has the
    /// highest ID yet, so the shorter prefix's database is the longer one's
    /// without its newest entries. Returns whether it did.
    fn database_from_longer_prefix(&self) -> bool {
        self.sync_changes();
        let (added, relabel) = {
            let cache = self.database_cache.borrow();
            let Some(cache) = cache.as_ref() else {
                return false;
            };
            if cache.contents.is_err()
                || cache.epoch != self.formula.epoch()
                || cache.checked_len <= self.checked_len
                || self.changed_since_database.get() < cache.checked_len
            {
                return false;
            }
            let between = &self.buffer[self.checked_len..cache.checked_len];
            let Some(added) = between
                .iter()
                .map(|line| checker::parse::constraints_added(line))
                .sum::<Option<usize>>()
            else {
                return false;
            };
            let relabel = between
                .iter()
                .any(|line| !checker::parse::leading_labels(line).is_empty());
            (added, relabel)
        };
        if relabel && !self.lines_to_trace(self.checked_len, &[]).is_empty() {
            return false;
        }
        let labels = relabel.then(|| self.labels_from_kept_ids());

        let mut cache = self.database_cache.borrow_mut();
        let cache = cache.as_mut().expect("checked above");
        let Ok((database, cached_labels)) = &mut cache.contents else {
            return false;
        };
        let keep = database.entries.len().saturating_sub(added);
        if database.entries.len() < added || database.entries[keep..].iter().any(|e| e.is_core) {
            return false;
        }
        std::rc::Rc::make_mut(database).entries.truncate(keep);
        if let Some(labels) = labels {
            *cached_labels = labels;
        }
        cache.generation = self.generation;
        cache.checked_len = self.checked_len;
        cache.labels_by_id = std::cell::OnceCell::new();
        cache.live_labels = std::cell::OnceCell::new();
        true
    }

    /// Runs `veripb` over `buffer[..checked_len]` for [`Self::database`]:
    /// the database and every label's constraint ID. Traces the checked
    /// labelled lines whose IDs aren't kept, and the checked lines of
    /// `extra`, keeping what they derived.
    fn compute_database(
        &self,
        extra: &[usize],
    ) -> anyhow::Result<(
        std::rc::Rc<checker::Database>,
        ahash::AHashMap<String, isize>,
    )> {
        let checked = &self.buffer[..self.checked_len];
        let wanted = self.lines_to_trace(self.checked_len, extra);

        let preamble = self.preamble_lines();
        let proof = proof(&preamble, checked, &[]);
        let formula_file = self.formula_temp_file()?;
        // The trace is read as veripb writes it, keeping only the wanted
        // lines' IDs: it can be several times the proof's size.
        let is_wanted = |n: usize| {
            n.checked_sub(PREAMBLE_LINES + 1)
                .is_some_and(|idx| wanted.binary_search(&idx).is_ok())
        };
        let mut traced = checker::parse::ConstraintIdsByLine::new(is_wanted);
        let (outcome, database, bounds) = checker::check_with_dumps(
            formula_file.path(),
            proof,
            &self.trace_ranges(&wanted),
            self.objective.is_some(),
            |line| traced.feed(line),
        )?;
        if outcome.is_accepted() {
            self.keep_objective_bounds(bounds);
        }
        self.line_ids().insert(
            traced
                .finish()
                .into_iter()
                .map(|(n, ids)| (n - PREAMBLE_LINES - 1, ids)),
        );
        Ok((std::rc::Rc::new(database), self.labels_from_kept_ids()))
    }

    /// Every label's constraint ID for the checked prefix, from the formula
    /// and the kept IDs of the checked labelled lines (see
    /// [`Self::label_ids`]).
    fn labels_from_kept_ids(&self) -> ahash::AHashMap<String, isize> {
        let ids = self.line_ids();
        let mut labels = self.formula_labels();
        for (idx, line) in self.buffer[..self.checked_len].iter().enumerate() {
            let names = checker::parse::leading_labels(line);
            if names.is_empty() {
                continue;
            }
            let Some(line_ids) = ids.get(idx) else {
                continue;
            };
            for (label, id) in names.into_iter().zip(line_ids) {
                labels.insert(label.to_string(), *id as isize);
            }
        }
        labels
    }

    /// Refreshes the database cache from `database`, the dump of a verify
    /// run over the current checked prefix, if the kept IDs give every
    /// checked label's; and the objective cache from `bounds`, that run's
    /// objective dump if it made one.
    fn fill_database_cache(&self, (database, bounds): VerifyDumps) {
        self.database_wanted.set(false);
        self.keep_objective_bounds(bounds);
        if !self.lines_to_trace(self.checked_len, &[]).is_empty() {
            return;
        }
        let labels = self.labels_from_kept_ids();
        self.set_database_cache(Ok((std::rc::Rc::new(database), labels)));
    }

    /// The lines before buffer index `end` whose IDs aren't kept that are
    /// labelled or in `extra`, ascending: what [`Self::compute_database`]
    /// traces, with `end` the checked length.
    fn lines_to_trace(&self, end: usize, extra: &[usize]) -> Vec<usize> {
        let checked = &self.buffer[..end];
        let ids = self.line_ids();
        let mut wanted: Vec<usize> = checked
            .iter()
            .enumerate()
            .filter(|(idx, line)| {
                !ids.contains(*idx) && !checker::parse::leading_labels(line).is_empty()
            })
            .map(|(idx, _)| idx)
            .collect();
        wanted.extend(extra.iter().filter(|&&idx| idx < end && !ids.contains(idx)));
        wanted.sort_unstable();
        wanted.dedup();
        wanted
    }

    /// What proof lines derived, as far as it is known, with what edits
    /// since made stale dropped.
    fn line_ids(&self) -> std::cell::RefMut<'_, LineIds> {
        self.sync_changes();
        self.line_ids.borrow_mut()
    }

    /// Passes the buffer's changes since the last call (its low-water mark)
    /// to what depends on them: the kept line IDs, and
    /// `changed_since_database`.
    fn sync_changes(&self) {
        let changed_from = self.buffer.take_changed_from();
        self.line_ids
            .borrow_mut()
            .sync(self.formula.epoch(), changed_from);
        if let Some(idx) = changed_from {
            self.changed_since_database
                .set(self.changed_since_database.get().min(idx));
        }
    }

    /// Keeps what each line traced in `trace` (a checker's output for the
    /// current buffer) derived.
    fn keep_traced_ids(&self, trace: &str) {
        let mut traced = checker::parse::ConstraintIdsByLine::new(|_| true);
        for line in trace.lines() {
            traced.feed(line);
        }
        let traced = traced
            .finish()
            .into_iter()
            .filter_map(|(n, ids)| n.checked_sub(PREAMBLE_LINES + 1).map(|idx| (idx, ids)));
        self.line_ids().insert(traced);
    }

    /// The display-line ranges of buffer lines `lines` (ascending),
    /// consecutive lines merged; one range from the first to the last if
    /// there would be more than [`MAX_TRACE_RANGES`].
    fn trace_ranges(&self, lines: &[usize]) -> Vec<(usize, usize)> {
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for &idx in lines {
            let n = self.display_line(idx);
            match ranges.last_mut() {
                Some((_, hi)) if *hi + 1 == n => *hi = n,
                _ => ranges.push((n, n)),
            }
        }
        if ranges.len() > MAX_TRACE_RANGES {
            ranges = vec![(ranges[0].0, ranges[ranges.len() - 1].1)];
        }
        ranges
    }

    /// Returns the constraint IDs checked line `display_line` derived, or
    /// `None` if it isn't a checked proof line. Kept once known; else one
    /// `veripb` run, which also refreshes [`Self::database`].
    pub fn line_constraint_ids(&self, display_line: usize) -> anyhow::Result<Option<Vec<usize>>> {
        let Some(idx) = self.checked_index(display_line) else {
            return Ok(None);
        };
        if let Some(ids) = self.line_ids().get(idx) {
            return Ok(Some(ids.to_vec()));
        }
        self.refresh_database(&[idx]);
        self.database()?;
        Ok(self.line_ids().get(idx).map(<[usize]>::to_vec))
    }

    /// Returns where constraint `id` came from: a formula line, or the
    /// checked proof line that derived it. `None` if neither. Kept once
    /// known; else one `veripb` run tracing every checked line whose IDs
    /// aren't kept, which also refreshes [`Self::database`].
    pub fn constraint_source(&self, id: usize) -> anyhow::Result<Option<ConstraintSource>> {
        if (1..=self.formula_constraint_count()).contains(&id) {
            return Ok(self
                .formula_index(id)
                .map(|line| ConstraintSource::Formula { line }));
        }
        let to_proof_line = |idx: usize| ConstraintSource::Proof {
            display_line: self.display_line(idx),
        };
        if let Some(idx) = self.line_ids().line_of(id, self.checked_len) {
            return Ok(Some(to_proof_line(idx)));
        }
        let unkept: Vec<usize> = {
            let ids = self.line_ids();
            (0..self.checked_len)
                .filter(|&idx| !ids.contains(idx))
                .collect()
        };
        if unkept.is_empty() {
            return Ok(None);
        }
        self.refresh_database(&unkept);
        self.database()?;
        Ok(self
            .line_ids()
            .line_of(id, self.checked_len)
            .map(to_proof_line))
    }

    /// Returns the best-known objective bounds for `buffer[..checked_len]`,
    /// re-running `veripb` only if `generation` changed since they were
    /// cached (a database refresh fills the cache too).
    pub fn objective_bounds(&self) -> anyhow::Result<checker::ObjectiveBounds> {
        if let Some((generation, bounds)) = &*self.objective_cache.borrow()
            && *generation == self.generation
        {
            return Ok(bounds.clone());
        }
        let preamble = self.preamble_lines();
        let proof = proof(&preamble, &self.buffer[..self.checked_len], &[]);
        let formula_file = self.formula_temp_file()?;
        let bounds = checker::show_objective_bounds(formula_file.path(), proof)?;
        *self.objective_cache.borrow_mut() = Some((self.generation, bounds.clone()));
        Ok(bounds)
    }

    /// Caches `bounds`, an objective dump for the current state, unless the
    /// dump failed or wasn't made.
    fn keep_objective_bounds(&self, bounds: Option<anyhow::Result<checker::ObjectiveBounds>>) {
        if let Some(Ok(bounds)) = bounds {
            *self.objective_cache.borrow_mut() = Some((self.generation, bounds));
        }
    }

    /// The database for the current state if it is already cached, without
    /// running `veripb`.
    pub(crate) fn cached_database(&self) -> Option<std::rc::Rc<checker::Database>> {
        match &*self.database_cache.borrow() {
            Some(DatabaseCache {
                generation,
                contents: Ok((database, _)),
                ..
            }) if *generation == self.generation => Some(database.clone()),
            _ => None,
        }
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
        let preamble = self.preamble_lines();
        let current = match self.check(
            proof(&preamble, &self.buffer[..=idx], &[]),
            Some((line, line)),
        )? {
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

    /// One `veripb` run over `buffer[..end]` for a verify step, tracing
    /// display lines `shown`, and keeping what the accepted lines it traced
    /// derived. While the database is in use (see `database_wanted`), also
    /// dumps it (and the objective bounds, if there is an objective),
    /// returned if every line was accepted, and traces the
    /// labelled lines before `end` whose IDs aren't kept, so their labels
    /// come with it; only `shown`'s trace is left in the outcome.
    fn verify_run(
        &self,
        end: usize,
        shown: (usize, usize),
    ) -> anyhow::Result<(CheckOutcome, Option<VerifyDumps>)> {
        let preamble = self.preamble_lines();
        let proof = proof(&preamble, &self.buffer[..end], &[]);
        if !self.database_wanted.get() {
            let outcome = self.check(proof, Some(shown))?;
            if let CheckOutcome::Accepted { trace } = &outcome {
                self.keep_traced_ids(trace);
            }
            return Ok((outcome, None));
        }

        let mut ranges = self.trace_ranges(&self.lines_to_trace(end, &[]));
        ranges.push(shown);
        ranges.sort_unstable();
        ranges.dedup_by(|(lo, hi), (_, prev_hi)| {
            let overlaps = *lo <= *prev_hi + 1;
            if overlaps {
                *prev_hi = (*prev_hi).max(*hi);
            }
            overlaps
        });
        let is_shown = |n: usize| (shown.0..=shown.1).contains(&n);
        let mut traced = checker::parse::ConstraintIdsByLine::new(|_| true);
        let formula_file = self.formula_temp_file()?;
        let (outcome, database, bounds) = checker::check_with_dumps(
            formula_file.path(),
            proof,
            &ranges,
            self.objective.is_some(),
            |line| traced.feed(line) && !traced.current_line().is_some_and(is_shown),
        )?;
        let accepted_before = match &outcome {
            CheckOutcome::Accepted { .. } => usize::MAX,
            CheckOutcome::Rejected { line, .. } => *line,
        };
        self.line_ids().insert(
            traced
                .finish()
                .into_iter()
                .filter(|(n, _)| *n < accepted_before)
                .filter_map(|(n, ids)| n.checked_sub(PREAMBLE_LINES + 1).map(|idx| (idx, ids))),
        );
        let database = outcome.is_accepted().then_some((database, bounds));
        Ok((outcome, database))
    }

    /// Checks `buffer[checked_len]`. Acceptance advances `checked_len` by
    /// one; rejection sets `known_bad` and changes nothing else. Returns the
    /// trace and the rejection message, if any. Panics if
    /// `checked_len == buffer.len()`.
    fn verify_next(&mut self) -> anyhow::Result<(String, Option<String>)> {
        // The checked prefix and the candidate line, tracing only the new
        // line; the prefix was traced when accepted.
        let candidate_line_no = PREAMBLE_LINES + self.checked_len + 1;
        let (outcome, database) =
            self.verify_run(self.checked_len + 1, (candidate_line_no, candidate_line_no))?;

        match outcome {
            CheckOutcome::Accepted { trace } => {
                self.checked_len += 1;
                self.known_bad = None;
                self.last_step_constraint_ids = checker::parse::last_line_constraint_ids(&trace);
                self.generation += 1;
                if let Some(database) = database {
                    self.fill_database_cache(database);
                }
                Ok((trace, None))
            }
            CheckOutcome::Rejected { message, trace, .. } => {
                // Name any constraint the reason mentions by ID (looking the
                // labels up can take a veripb run, so only if there is one).
                let message = if message.contains("ID ") {
                    checker::parse::annotate_constraint_ids(&message, &self.labels_by_id())
                } else {
                    message
                };
                self.known_bad = Some(message.clone());
                Ok((trace, Some(message)))
            }
        }
    }

    /// Advances `checked_len` to `boundary` (`<= buffer.len()`) with one
    /// batched `veripb` call, tracing every new line, or only the last if
    /// there are more than [`FULL_TRACE_MAX_LINES`]. If the batch is
    /// rejected, its output is discarded: every line before the one veripb
    /// names was accepted, so `checked_len` moves to that line and
    /// [`Self::verify_next`] checks it for the rejection message (going on
    /// line by line up to `boundary` if it is accepted after all, or if the
    /// named line is outside the batch). No-op returning an empty capture if
    /// `boundary <= checked_len`.
    fn verify_forward(&mut self, boundary: usize) -> anyhow::Result<(String, Option<String>)> {
        if boundary <= self.checked_len {
            return Ok((String::new(), None));
        }

        let candidate_end = PREAMBLE_LINES + boundary;
        let candidate_start = if boundary - self.checked_len > FULL_TRACE_MAX_LINES {
            candidate_end
        } else {
            PREAMBLE_LINES + self.checked_len + 1
        };
        let (outcome, database) = self.verify_run(boundary, (candidate_start, candidate_end))?;
        let rejected_idx = match outcome {
            CheckOutcome::Accepted { trace } => {
                self.checked_len = boundary;
                self.known_bad = None;
                self.last_step_constraint_ids = checker::parse::last_line_constraint_ids(&trace);
                self.generation += 1;
                if let Some(database) = database {
                    self.fill_database_cache(database);
                }
                return Ok((trace, None));
            }
            CheckOutcome::Rejected { line, .. } => line.checked_sub(PREAMBLE_LINES + 1),
        };
        if let Some(idx) = rejected_idx
            && idx > self.checked_len
            && idx < boundary
        {
            self.checked_len = idx;
            self.known_bad = None;
            self.last_step_constraint_ids = Vec::new();
            self.generation += 1;
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

    /// `:debug`'s `:step [n]`: `verify_forward` over the next `n` unchecked
    /// lines (fewer at the end of the buffer). `Ok(None)` if nothing is
    /// unchecked.
    pub fn step(&mut self, n: usize) -> anyhow::Result<Option<(String, Option<String>)>> {
        if self.checked_len >= self.buffer.len() {
            return Ok(None);
        }
        let boundary = self.checked_len.saturating_add(n).min(self.buffer.len());
        self.verify_forward(boundary).map(Some)
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
        self.buffer.set(idx, new_text.to_string());
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
        self.replace_buffer_unchecked(lines);
        self.verify_forward(self.buffer.len())
    }

    /// Replaces the buffer with `lines`, all unchecked.
    pub(crate) fn replace_buffer_unchecked(&mut self, lines: Vec<String>) {
        self.buffer.replace(lines);
        self.checked_len = 0;
        self.known_bad = None;
        self.last_step_constraint_ids = Vec::new();
        self.generation += 1;
    }

    /// Replaces formula line `idx` with `text` (`:formula cancel` restoring
    /// the replaced constraint). The caller reverifies the buffer.
    pub(crate) fn restore_formula_line(&mut self, idx: usize, text: String) {
        self.formula.set(idx, text);
        self.checked_len = 0;
        self.known_bad = None;
        self.last_step_constraint_ids = Vec::new();
        self.generation += 1;
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
        self.buffer.replace(buffer);
        self.checked_len = checked_len;
        self.known_bad = known_bad;
        self.last_step_constraint_ids = Vec::new();
        self.generation += 1;
        Ok(())
    }

    /// Sets [`Self::intended_conclusion`] to `conclusion` (`None` clears it),
    /// trimmed, without its `conclusion` keyword or `;`, whitespace
    /// collapsed. A change is an undo point (see [`Self::mark_undo_point`]).
    pub fn set_intended_conclusion(&mut self, conclusion: Option<&str>) {
        let conclusion = conclusion
            .map(normalize_conclusion)
            .filter(|c| !c.is_empty());
        if conclusion != self.intended_conclusion {
            self.intended_conclusion = conclusion;
            self.mark_undo_point();
        }
    }

    /// Clears the buffer, breakpoints, intended conclusion and undo history
    /// (see [`Self::clear_undo_history`]), leaving `formula`/`variables`
    /// alone.
    pub fn reset(&mut self) -> anyhow::Result<()> {
        self.clear_undo_history();
        self.intended_conclusion = None;
        self.buffer.clear();
        self.checked_len = 0;
        self.known_bad = None;
        self.last_step_constraint_ids = Vec::new();
        self.breakpoints.clear();
        self.generation += 1;
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

    /// Converts a formula constraint ID (as in `:show`/the Formula pane) to
    /// the index of the `formula` line holding it. `None` if out of range.
    pub(crate) fn formula_index(&self, n: usize) -> Option<usize> {
        if n == 0 || n > self.formula_constraint_count() {
            return None;
        }
        // The last line starting at or before `n`.
        Some(
            self.formula_first_ids()
                .partition_point(|&first| first <= n)
                - 1,
        )
    }

    /// Marks an undo point without changing anything: [`Self::change_stamp`]
    /// differs afterwards, so [`Self::end_undo_point`] keeps the state before
    /// as one undo step.
    pub(crate) fn mark_undo_point(&mut self) {
        self.undo_marker += 1;
    }

    /// Differs whenever the state changed or [`Self::mark_undo_point`] ran:
    /// compared before and after an action to decide whether to keep its undo
    /// point.
    pub(crate) fn change_stamp(&self) -> (u64, u64) {
        (self.generation, self.undo_marker)
    }

    fn point_state(&self) -> PointState {
        PointState {
            checked_len: self.checked_len,
            known_bad: self.known_bad.clone(),
            last_step_constraint_ids: self.last_step_constraint_ids.clone(),
            intended_conclusion: self.intended_conclusion.clone(),
        }
    }

    fn set_point_state(&mut self, state: PointState) {
        self.checked_len = state.checked_len;
        self.known_bad = state.known_bad;
        self.last_step_constraint_ids = state.last_step_constraint_ids;
        self.intended_conclusion = state.intended_conclusion;
    }

    /// Records the hunks that undo the lines' changes from now on, or stops
    /// recording if `None`.
    fn record(&mut self, hunks: Option<(Hunk, Hunk)>) {
        let (buffer, formula) = hunks.unzip();
        self.buffer.record(buffer);
        self.formula.record(formula);
    }

    /// Returns the hunks recorded since the last call.
    fn take_hunks(&mut self) -> (Hunk, Hunk) {
        (self.buffer.take_hunk(), self.formula.take_hunk())
    }

    /// Sets the top `:undo` entry's hunks, if there is one.
    fn set_top_hunks(&mut self, (buffer, formula): (Hunk, Hunk)) {
        if let Some(top) = self.undo_stack.back_mut() {
            top.bytes = buffer.bytes() + formula.bytes();
            top.buffer = buffer;
            top.formula = formula;
        }
    }

    /// Starts an undo point for the action about to run: if the action
    /// changes [`Self::change_stamp`], [`Self::end_undo_point`] makes the
    /// state before it an `:undo` entry. Ends any point still open.
    pub(crate) fn begin_undo_point(&mut self) {
        self.end_undo_point();
        let (buffer, formula) = self.take_hunks();
        self.record(Some(Default::default()));
        self.pending_point = Some(PendingPoint {
            stamp: self.change_stamp(),
            state: self.point_state(),
            buffer,
            formula,
        });
    }

    /// Ends the point [`Self::begin_undo_point`] started, keeping it as an
    /// `:undo` entry if anything changed since. Clears the redo history if
    /// so.
    pub(crate) fn end_undo_point(&mut self) {
        let Some(point) = self.pending_point.take() else {
            return;
        };
        let changes = self.take_hunks();
        if self.change_stamp() != point.stamp || !changes.0.is_empty() || !changes.1.is_empty() {
            self.set_top_hunks((point.buffer, point.formula));
            self.undo_stack.push_back(UndoEntry::new(
                point.state,
                Hunk::default(),
                Hunk::default(),
            ));
            self.record(Some(changes));
            self.redo_stack.clear();
            self.trim_undo_stack();
        } else {
            let recording = !self.undo_stack.is_empty();
            self.record(recording.then_some((point.buffer, point.formula)));
        }
    }

    /// Drops the oldest `:undo` entries beyond `UNDO_STACK_CAP` and
    /// `UNDO_BYTES_CAP`.
    fn trim_undo_stack(&mut self) {
        let mut bytes: usize = self.undo_stack.iter().map(|entry| entry.bytes).sum();
        while self.undo_stack.len() > UNDO_STACK_CAP
            || (self.undo_stack.len() > 1 && bytes > UNDO_BYTES_CAP)
        {
            let dropped = self.undo_stack.pop_front().expect("not empty");
            bytes -= dropped.bytes;
        }
    }

    /// Forgets every `:undo` and `:redo` entry. An undo point still open
    /// stays open, as the first entry to come.
    pub(crate) fn clear_undo_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        match &mut self.pending_point {
            Some(point) => {
                point.buffer = Hunk::default();
                point.formula = Hunk::default();
                self.take_hunks();
            }
            None => self.record(None),
        }
    }

    /// `:undo [n]`: returns to the state before each of the last `n`
    /// actions in turn, keeping them for [`Self::redo`]. Returns how many
    /// it undid.
    pub(crate) fn undo(&mut self, n: usize) -> usize {
        self.end_undo_point();
        self.drop_stale_redo();
        let mut undone = 0;
        while undone < n {
            let Some(entry) = self.undo_stack.pop_back() else {
                break;
            };
            let (buffer, formula) = self.take_hunks();
            let state = self.point_state();
            let redo = UndoEntry::new(
                state,
                self.buffer.apply(buffer),
                self.formula.apply(formula),
            );
            self.set_point_state(entry.state);
            self.redo_stack.push(redo);
            // The entry below is now the top, so its hunks are recorded on.
            let below = self.undo_stack.back_mut().map(|top| {
                top.bytes = 0;
                (
                    std::mem::take(&mut top.buffer),
                    std::mem::take(&mut top.formula),
                )
            });
            self.record(below);
            undone += 1;
        }
        if undone > 0 {
            self.undone_or_redone();
        }
        undone
    }

    /// `:redo [n]`: redoes the last `n` actions `:undo` undid, if nothing
    /// else changed since. Returns how many it redid.
    pub(crate) fn redo(&mut self, n: usize) -> usize {
        self.end_undo_point();
        self.drop_stale_redo();
        let mut redone = 0;
        while redone < n {
            let Some(entry) = self.redo_stack.pop() else {
                break;
            };
            let below = self.take_hunks();
            self.set_top_hunks(below);
            let state = self.point_state();
            let undo = (
                self.buffer.apply(entry.buffer),
                self.formula.apply(entry.formula),
            );
            self.set_point_state(entry.state);
            self.undo_stack
                .push_back(UndoEntry::new(state, Hunk::default(), Hunk::default()));
            self.record(Some(undo));
            redone += 1;
        }
        if redone > 0 {
            self.trim_undo_stack();
            self.undone_or_redone();
        }
        redone
    }

    /// Forgets the `:redo` entries if anything but `:undo` or `:redo` changed
    /// the session since they were made.
    fn drop_stale_redo(&mut self) {
        if self.change_stamp() != self.redo_stamp {
            self.redo_stack.clear();
        }
    }

    fn undone_or_redone(&mut self) {
        self.last_formula_edit = None;
        self.recoverable_buffer = None;
        self.generation += 1;
        self.redo_stamp = self.change_stamp();
    }

    /// Replaces the formula line holding constraint ID `n` with `new_text`.
    /// The new formula is validated first; on error `self` is unchanged. On
    /// success the buffer is cleared and the base buffer
    /// (`recoverable_buffer`, else `buffer`) is stored in
    /// `recoverable_buffer`/`last_formula_edit`; the caller reverifies it via
    /// `replace_buffer_and_verify`.
    ///
    /// Rejects a replacement that loads as a different number of
    /// constraints than the line it replaces: later constraint IDs would
    /// shift.
    pub fn replace_formula_constraint(&mut self, n: usize, new_text: &str) -> anyhow::Result<()> {
        let total = self.formula_constraint_count();
        let idx = self.formula_index(n).ok_or_else(|| {
            anyhow::anyhow!(
                "constraint {n} is out of range — the formula has {total} constraint(s) (1-{total})."
            )
        })?;

        // Add the trailing `;` if omitted.
        let mut new_text = new_text.trim().to_string();
        if !new_text.ends_with(';') {
            new_text.push_str(" ;");
        }

        let (old_count, new_count) = (
            checker::parse::formula_line_constraints(&self.formula[idx]),
            checker::parse::formula_line_constraints(&new_text),
        );
        if old_count != new_count {
            anyhow::bail!(
                "this line loads as {old_count} constraint(s) and the replacement as \
                 {new_count}; an edit can't change the count, since later constraint IDs \
                 would shift. Edit the OPB file and :load it instead."
            );
        }

        let mut candidate_formula = self.formula.to_vec();
        candidate_formula[idx] = new_text.clone();

        // Validate the candidate formula alone (no proof lines) before
        // touching `self`.
        let candidate_file =
            write_formula_temp_file(&candidate_formula, &self.objective, &self.preserved)?;
        let empty_proof = format!("pseudo-Boolean proof version 3.0\nf {total};\n");
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
            None => self.buffer.to_vec(),
        };

        // Nothing below this line can fail — commit as one unit.
        let prior_constraint = self.formula.set(idx, new_text);
        self.buffer.clear();
        self.checked_len = 0;
        self.known_bad = None;
        self.last_step_constraint_ids = Vec::new();
        self.variables = candidate_vars;
        self.recoverable_buffer = Some(base_buffer.clone());
        self.last_formula_edit = Some(FormulaEditSnapshot {
            n,
            constraint: prior_constraint,
            buffer: base_buffer,
        });
        self.generation += 1;
        Ok(())
    }

    /// Writes the full `.pbp` text to `w`: preamble, the whole buffer
    /// (checked or not), and `output NONE; conclusion <conclusion>; end
    /// pseudo-Boolean proof;`. Not checked; see [`Self::dry_run_conclusion`].
    pub fn write_listing(
        &self,
        conclusion: &str,
        w: &mut impl std::io::Write,
    ) -> std::io::Result<()> {
        let preamble = self.preamble_lines();
        let tail = closing_lines(conclusion);
        proof(&preamble, &self.buffer, &tail).write_to(w, || true)
    }

    /// Checks whether the checked prefix plus `output NONE; conclusion
    /// <conclusion>; end pseudo-Boolean proof;` is a complete valid proof,
    /// without changing state. `conclusion` is spliced verbatim (e.g.
    /// `"UNSAT"`, `"BOUNDS 0 10"`). Returns the trace and the result,
    /// cached until `generation` changes.
    pub fn dry_run_conclusion(&self, conclusion: &str) -> anyhow::Result<DryRun> {
        {
            let mut cache = self.conclusion_cache.borrow_mut();
            if cache.0 != self.generation {
                *cache = (self.generation, ahash::AHashMap::new());
            }
            if let Some(result) = cache.1.get(conclusion) {
                return Ok(result.clone());
            }
        }
        let preamble = self.preamble_lines();
        let tail = closing_lines(conclusion);
        let result = match self.check(
            proof(&preamble, &self.buffer[..self.checked_len], &tail),
            None,
        )? {
            CheckOutcome::Accepted { trace } => (trace, Ok(())),
            CheckOutcome::Rejected { message, trace, .. } => (trace, Err(message)),
        };
        self.conclusion_cache
            .borrow_mut()
            .1
            .insert(conclusion.to_string(), result.clone());
        Ok(result)
    }

    /// Replays through checked line `display_line` and returns its trace.
    /// The inner `Err` is a user-facing message.
    pub fn explain_line(&self, display_line: usize) -> anyhow::Result<Result<String, String>> {
        let Some(idx) = self.checked_index(display_line) else {
            return Ok(Err(unchecked_or_out_of_range(self, display_line)));
        };

        let preamble = self.preamble_lines();
        let formula_file = self.formula_temp_file()?;
        match checker::explain_line(
            formula_file.path(),
            proof(&preamble, &self.buffer[..=idx], &[]),
        )? {
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

        let preamble = self.preamble_lines();
        let formula_file = self.formula_temp_file()?;
        match checker::elaborate_rup(
            formula_file.path(),
            proof(&preamble, &self.buffer[..=idx], &[]),
        )? {
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

        let labels = self.label_ids();
        let mut unknown_labels = Vec::new();
        let ids: Vec<usize> = typed_hints
            .iter()
            .filter_map(|hint| match hint {
                checker::RupHint::ConstraintId(id) => Some(*id),
                checker::RupHint::Label(name) => match labels.get(name) {
                    Some(id) => Some(*id as usize),
                    None => {
                        unknown_labels.push(name.clone());
                        None
                    }
                },
                checker::RupHint::NegatedPremise => None,
            })
            .collect();
        let database = self.database()?;
        let missing_ids = ids.into_iter().filter(|id| database.get(*id).is_none()).collect();
        drop(database);

        let preamble = self.preamble_lines();
        let tail = [format!("{prefix} ;")];
        let formula_file = self.formula_temp_file()?;
        let proof = proof(&preamble, &self.buffer[..self.checked_len], &tail);
        let without_hints = match checker::elaborate_rup(formula_file.path(), proof)? {
            Ok(hints) => Ok(hints.unwrap_or_default()),
            Err(message) => Err(message),
        };

        Ok(Some(RejectionDiagnosis {
            display_line: self.display_line(self.checked_len),
            typed_hints,
            missing_ids,
            unknown_labels,
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
    /// Typed hint IDs (or labels' IDs) not in the live database.
    pub missing_ids: Vec<usize>,
    /// Typed `@label` hints that no formula or checked line defines.
    pub unknown_labels: Vec<String>,
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

/// The proof `preamble`, `body`, `tail`, for [`checker`].
fn proof<'a>(preamble: &'a [String], body: &'a [String], tail: &'a [String]) -> ProofText<'a> {
    ProofText::Lines {
        preamble,
        body,
        tail,
    }
}

/// A proof's closing lines, concluding `conclusion`.
fn closing_lines(conclusion: &str) -> [String; 3] {
    [
        "output NONE;".to_string(),
        format!("conclusion {conclusion};"),
        "end pseudo-Boolean proof;".to_string(),
    ]
}

/// `conclusion` as [`Session::intended_conclusion`] keeps it: without a
/// leading `conclusion` keyword or a trailing `;`, whitespace collapsed.
pub fn normalize_conclusion(conclusion: &str) -> String {
    let conclusion = conclusion.trim().trim_end_matches(';');
    let conclusion = conclusion
        .strip_prefix("conclusion")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .unwrap_or(conclusion);
    conclusion.split_whitespace().collect::<Vec<_>>().join(" ")
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
    let mut writer = std::io::BufWriter::new(file.as_file_mut());
    let lines = objective
        .iter()
        .chain(preserved.iter())
        .chain(formula.iter());
    for line in lines {
        writeln!(writer, "{line}").context("failed to write the temp formula file")?;
    }
    writer
        .flush()
        .context("failed to write the temp formula file")?;
    drop(writer);
    Ok(file)
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

    type State = (Vec<String>, Vec<String>, usize);

    fn state(session: &Session) -> State {
        (
            session.formula.to_vec(),
            session.buffer.to_vec(),
            session.checked_len,
        )
    }

    /// One random change through `Session`'s own methods.
    fn change(session: &mut Session, rng: &mut Rng, log: &mut Vec<String>) {
        let text = ["a", "b", "c"][rng.below(3)];
        let len = session.buffer.len();
        let op = rng.below(7);
        log.push(format!("  change {op} on {:?}", session.buffer.to_vec()));
        match op {
            0 if len > 0 => session.set_line(rng.below(len), text).unwrap(),
            1 => session.insert_line(rng.below(len + 1), text).unwrap(),
            2 if len > 0 => session.delete_line(rng.below(len)).unwrap(),
            3 => {
                let lines = (0..rng.below(6)).map(|i| ["a", "b"][i % 2].to_string());
                session.replace_buffer_unchecked(lines.collect());
            }
            4 => {
                // As a verify that accepts every line.
                session.checked_len = len;
                session.generation += 1;
            }
            5 => {
                let idx = rng.below(session.formula.len());
                session.restore_formula_line(idx, format!("1 x{} >= 1 ;", rng.below(3) + 1));
            }
            _ => session.mark_undo_point(),
        }
    }

    /// Random actions, edits outside any action (as in `:edit` mode),
    /// `undo` and `redo`, against a model that keeps a full copy of every
    /// state.
    #[test]
    fn undo_and_redo_agree_with_copying_every_state() {
        let mut file = tempfile::Builder::new().suffix(".opb").tempfile().unwrap();
        writeln!(file, "1 x1 >= 1 ;\n1 x2 >= 1 ;").unwrap();
        let path = file.path().to_str().unwrap().to_string();
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        for _ in 0..200 {
            let mut session = Session::load(&path).unwrap();
            let (mut undo, mut redo): (Vec<State>, Vec<State>) = (Vec::new(), Vec::new());
            let mut log = Vec::new();
            for _ in 0..rng.below(40) {
                let arm = rng.below(6);
                log.push(format!("arm {arm}"));
                match arm {
                    0 | 1 => {
                        let before = state(&session);
                        let stamp = session.change_stamp();
                        session.begin_undo_point();
                        for _ in 0..rng.below(3) + 1 {
                            change(&mut session, &mut rng, &mut log);
                        }
                        session.end_undo_point();
                        if session.change_stamp() != stamp {
                            undo.push(before);
                            redo.clear();
                        }
                    }
                    2 => {
                        let stamp = session.change_stamp();
                        change(&mut session, &mut rng, &mut log);
                        if session.change_stamp() != stamp {
                            redo.clear();
                        }
                    }
                    3 | 4 => {
                        let n = rng.below(3) + 1;
                        let mut current = state(&session);
                        for _ in 0..n.min(undo.len()) {
                            redo.push(current);
                            current = undo.pop().unwrap();
                        }
                        session.undo(n);
                        assert_eq!(state(&session), current, "after undo {n}: {log:#?}");
                    }
                    _ => {
                        let n = rng.below(3) + 1;
                        let mut current = state(&session);
                        for _ in 0..n.min(redo.len()) {
                            undo.push(current);
                            current = redo.pop().unwrap();
                        }
                        session.redo(n);
                        assert_eq!(state(&session), current, "after redo {n}: {log:#?}");
                    }
                }
            }
        }
    }

    #[test]
    fn edits_to_a_large_buffer_keep_only_their_lines() {
        let mut file = tempfile::Builder::new().suffix(".opb").tempfile().unwrap();
        writeln!(file, "1 x1 >= 1 ;").unwrap();
        let mut session = Session::load(file.path().to_str().unwrap()).unwrap();
        session
            .replace_buffer_unchecked((0..100_000).map(|i| format!("rup 1 x{i} >= 1 ;")).collect());
        for i in 0..100 {
            session.begin_undo_point();
            session.set_line(i * 997, "rup 1 x1 >= 1 ;").unwrap();
            session.end_undo_point();
        }
        session.begin_undo_point();
        session.end_undo_point();
        assert_eq!(session.undo_stack.len(), 100);
        let bytes: usize = session.undo_stack.iter().map(|entry| entry.bytes).sum();
        assert!(bytes < 100 * 64, "{bytes} bytes");
        assert_eq!(session.undo(100), 100);
        assert_eq!(session.buffer[997], "rup 1 x997 >= 1 ;");
    }

    /// Whether `$VERIPB_REPL_VERIPB_BIN` is set, so `veripb` can run.
    fn have_veripb() -> bool {
        let set = std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_some();
        if !set {
            eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
        }
        set
    }

    const CHAIN: usize = 8;

    /// x1 -> x2 -> ... -> x8 (labelled @c<i>) and ~x8, so every
    /// `rup 1 ~x<i> >= 1 ;` holds.
    fn chain_session() -> (tempfile::NamedTempFile, Session) {
        let mut file = tempfile::Builder::new().suffix(".opb").tempfile().unwrap();
        for i in 1..CHAIN {
            writeln!(file, "@c{i} 1 ~x{i} 1 x{} >= 1 ;", i + 1).unwrap();
        }
        writeln!(file, "1 ~x{CHAIN} >= 1 ;").unwrap();
        let session = Session::load(file.path().to_str().unwrap()).unwrap();
        (file, session)
    }

    /// A proof line: mostly a `rup` that holds, often labelled, sometimes
    /// one that doesn't, or a comment (which shifts the IDs after it).
    fn random_line(rng: &mut Rng) -> String {
        let label = ["", "@a ", "@b ", "@c "][rng.below(4)];
        match rng.below(10) {
            0 => format!("{label}rup 1 x1 >= 1 ;"),
            1 | 2 => "% derives nothing".to_string(),
            3 => format!("{label}pol 1 2 + ;"),
            4 => format!("{label}a 1 x{} >= 1 ;", rng.below(CHAIN) + 1),
            _ => format!("{label}rup 1 ~x{} >= 1 ;", rng.below(CHAIN) + 1),
        }
    }

    /// What every checked line derived, from one run tracing them all.
    fn traced_ids(session: &Session) -> std::collections::BTreeMap<usize, Vec<usize>> {
        if session.checked_len == 0 {
            return Default::default();
        }
        let preamble = session.preamble_lines();
        let range = (PREAMBLE_LINES + 1, PREAMBLE_LINES + session.checked_len);
        let outcome = session
            .check(
                proof(&preamble, &session.buffer[..session.checked_len], &[]),
                Some(range),
            )
            .unwrap();
        let CheckOutcome::Accepted { trace } = outcome else {
            panic!("the checked lines were rejected");
        };
        let mut traced = checker::parse::ConstraintIdsByLine::new(|_| true);
        for line in trace.lines() {
            traced.feed(line);
        }
        traced
            .finish()
            .into_iter()
            .map(|(n, ids)| (n - PREAMBLE_LINES - 1, ids))
            .collect()
    }

    /// The labels [`Session::label_ids`] should give, from `traced_ids`.
    fn traced_labels(session: &Session) -> ahash::AHashMap<String, isize> {
        let traced = traced_ids(session);
        let mut labels = session.formula_labels();
        for (idx, line) in session.buffer[..session.checked_len].iter().enumerate() {
            for (label, id) in checker::parse::leading_labels(line)
                .into_iter()
                .zip(traced.get(&idx).into_iter().flatten())
            {
                labels.insert(label.to_string(), *id as isize);
            }
        }
        labels
    }

    /// Random edits, checks, steps back, formula changes and undos: the
    /// kept IDs and the labels always match a run tracing every line.
    #[test]
    fn kept_ids_agree_with_tracing_every_line() {
        if !have_veripb() {
            return;
        }
        let mut rng = Rng(0x853c_49e6_748f_ea9b);
        for _ in 0..12 {
            let (_file, mut session) = chain_session();
            for _ in 0..25 {
                session.database_wanted.set(rng.below(2) == 0);
                let len = session.buffer.len();
                let op = rng.below(9);
                if op == 8 {
                    session.undo(1);
                } else {
                    session.begin_undo_point();
                    match op {
                        0 | 1 => {
                            session.append_line(&random_line(&mut rng)).unwrap();
                        }
                        2 if len > 0 => session
                            .set_line(rng.below(len), &random_line(&mut rng))
                            .unwrap(),
                        3 => session
                            .insert_line(rng.below(len + 1), &random_line(&mut rng))
                            .unwrap(),
                        4 if len > 0 => session.delete_line(rng.below(len)).unwrap(),
                        6 => {
                            session.step_back().unwrap();
                        }
                        7 => {
                            let i = rng.below(CHAIN - 1) + 1;
                            let k = rng.below(2) + 1;
                            let text = format!("@c{i} {k} ~x{i} {k} x{} >= {k} ;", i + 1);
                            session.restore_formula_line(i - 1, text);
                        }
                        _ => {
                            session.drive_forward().unwrap();
                        }
                    }
                    session.end_undo_point();
                }

                let labels = session.label_ids();
                let traced = traced_ids(&session);
                assert_eq!(
                    entries(&session.database().unwrap()),
                    entries(&fresh_database(&session)),
                    "{:?}",
                    &session.buffer[..]
                );
                let ids = session.line_ids();
                for (idx, expected) in &traced {
                    if let Some(kept) = ids.get(*idx) {
                        assert_eq!(
                            kept,
                            &expected[..],
                            "line {idx} of {:?}",
                            &session.buffer[..]
                        );
                    }
                }
                drop(ids);
                assert_eq!(labels, traced_labels(&session), "{:?}", &session.buffer[..]);
                assert!(session.lines_to_trace(session.checked_len, &[]).is_empty());
            }
        }
    }

    fn verified_chain() -> (tempfile::NamedTempFile, Session) {
        let (file, mut session) = chain_session();
        for i in (1..=CHAIN).rev() {
            session.buffer.push(format!("@u{i} rup 1 ~x{i} >= 1 ;"));
        }
        session.buffer.push("rup 1 ~x1 >= 1 ;".to_string());
        session.drive_forward().unwrap();
        assert_eq!(session.checked_len, CHAIN + 1);
        (file, session)
    }

    #[test]
    fn verified_lines_need_no_trace_later() {
        if !have_veripb() {
            return;
        }
        let (_file, mut session) = verified_chain();
        assert!(
            session.lines_to_trace(session.checked_len, &[]).is_empty(),
            "kept by :verify"
        );
        let runs = checker::invoke::invocation_count();
        assert_eq!(
            session.line_constraint_ids(3).unwrap(),
            Some(vec![CHAIN + 1])
        );
        assert_eq!(checker::invoke::invocation_count(), runs);

        session.database().unwrap();
        session.step_back().unwrap();
        session.restart().unwrap();
        session.drive_forward().unwrap();
        assert!(
            session.lines_to_trace(session.checked_len, &[]).is_empty(),
            "kept across :restart"
        );
        assert_eq!(session.label_ids()["@u1"], (2 * CHAIN) as isize);

        session.set_line(2, "@u6 rup 1 ~x2 >= 1 ;").unwrap();
        session.drive_forward().unwrap();
        session.database().unwrap();
        assert_eq!(session.label_ids()["@u6"], (CHAIN + 3) as isize);
    }

    #[test]
    fn constraint_sources_are_found_and_kept() {
        if !have_veripb() {
            return;
        }
        let (_file, session) = verified_chain();
        assert_eq!(
            session.constraint_source(2).unwrap(),
            Some(ConstraintSource::Formula { line: 1 })
        );
        assert_eq!(
            session.constraint_source(CHAIN + 2).unwrap(),
            Some(ConstraintSource::Proof { display_line: 4 })
        );
        assert_eq!(session.constraint_source(1000).unwrap(), None);
        let runs = checker::invoke::invocation_count();
        assert_eq!(
            session.constraint_source(CHAIN + 3).unwrap(),
            Some(ConstraintSource::Proof { display_line: 5 })
        );
        assert_eq!(checker::invoke::invocation_count(), runs);
    }

    fn entries(database: &checker::Database) -> Vec<(usize, bool, String)> {
        database
            .entries
            .iter()
            .map(|entry| (entry.id, entry.is_core, entry.text.clone()))
            .collect()
    }

    /// The database after the checked lines, from a run of its own.
    fn fresh_database(session: &Session) -> checker::Database {
        let preamble = session.preamble_lines();
        let formula_file = session.formula_temp_file().unwrap();
        checker::show_database(
            formula_file.path(),
            proof(&preamble, &session.buffer[..session.checked_len], &[]),
        )
        .unwrap()
    }

    /// `n` lines `@l<k> rup 1 ~x<i> >= 1 ;`, each deriving one constraint.
    fn push_labelled(session: &mut Session, n: usize) {
        for k in 0..n {
            let i = k % CHAIN + 1;
            session.buffer.push(format!("@l{k} rup 1 ~x{i} >= 1 ;"));
        }
    }

    /// Runs `f`, returning its result and the `veripb` runs and database
    /// dumps it made.
    fn counting<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
        let (runs, dumps) = (
            checker::invoke::invocation_count(),
            checker::invoke::database_dump_count(),
        );
        let result = f();
        (
            result,
            checker::invoke::invocation_count() - runs,
            checker::invoke::database_dump_count() - dumps,
        )
    }

    #[test]
    fn verify_fills_the_database_while_it_is_in_use() {
        if !have_veripb() {
            return;
        }
        let (_file, mut session) = chain_session();
        session.database().unwrap();
        push_labelled(&mut session, 12);
        let (_, runs, dumps) = counting(|| session.drive_forward().unwrap());
        assert_eq!((runs, dumps), (1, 1));
        assert_eq!(session.checked_len, 12);

        let (labels, runs, _) = counting(|| {
            session.database().unwrap();
            session.label_ids()
        });
        assert_eq!(runs, 0);
        assert_eq!(labels, traced_labels(&session));
        assert_eq!(labels["@l11"], (CHAIN + 12) as isize);
        assert_eq!(
            entries(&session.database().unwrap()),
            entries(&fresh_database(&session))
        );

        session.buffer.push("@typed rup 1 ~x3 >= 1 ;".to_string());
        let (_, runs, dumps) = counting(|| session.step(1).unwrap());
        assert_eq!((runs, dumps), (1, 1));
        let (labels, runs, _) = counting(|| session.label_ids());
        assert_eq!(runs, 0);
        assert_eq!(labels["@typed"], (CHAIN + 13) as isize);
    }

    #[test]
    fn verify_makes_no_dump_until_the_database_is_used() {
        if !have_veripb() {
            return;
        }
        let (_file, mut session) = chain_session();
        push_labelled(&mut session, 12);
        let (_, runs, dumps) = counting(|| session.drive_forward().unwrap());
        assert_eq!((runs, dumps), (1, 0));
        let (_, runs, _) = counting(|| session.database().unwrap());
        assert_eq!(runs, 1);
    }

    #[test]
    fn step_n_is_one_run() {
        if !have_veripb() {
            return;
        }
        let (_file, mut session) = chain_session();
        push_labelled(&mut session, 9);
        session.database().unwrap();
        let (_, runs, _) = counting(|| session.step(3).unwrap());
        assert_eq!(runs, 1);
        assert_eq!(session.checked_len, 3);
        let (_, runs, _) = counting(|| session.database().unwrap());
        assert_eq!(runs, 0);

        let (stepped, _, _) = counting(|| session.step(100).unwrap());
        assert!(stepped.is_some_and(|(_, rejection)| rejection.is_none()));
        assert_eq!(session.checked_len, 9);
        assert!(session.step(1).unwrap().is_none());
    }

    #[test]
    fn a_rejected_verify_is_refreshed_on_demand() {
        if !have_veripb() {
            return;
        }
        let (_file, mut session) = chain_session();
        session.database().unwrap();
        push_labelled(&mut session, 6);
        session.buffer.insert(4, "@bad rup 1 x1 >= 1 ;".to_string());
        let (_, rejection) = session.drive_forward().unwrap();
        assert!(rejection.is_some());
        assert_eq!(session.checked_len, 4);
        let (labels, runs, _) = counting(|| session.label_ids());
        assert_eq!(runs, 1);
        assert_eq!(labels, traced_labels(&session));
        assert!(!labels.contains_key("@bad"));
        assert_eq!(
            entries(&session.database().unwrap()),
            entries(&fresh_database(&session))
        );
    }

    #[test]
    fn a_large_batch_learns_its_labels_without_tracing_every_line() {
        if !have_veripb() {
            return;
        }
        let (_file, mut session) = chain_session();
        session.database().unwrap();
        let n = FULL_TRACE_MAX_LINES + 50;
        push_labelled(&mut session, n);
        let ((trace, rejection), runs, dumps) = counting(|| session.drive_forward().unwrap());
        assert!(rejection.is_none());
        assert_eq!((runs, dumps), (1, 1));
        let markers: Vec<&str> = trace.lines().filter(|l| l.starts_with("line ")).collect();
        assert_eq!(markers.len(), 1, "{markers:?}");
        assert!(markers[0].contains(&format!("@l{}", n - 1)));

        let (labels, runs, _) = counting(|| session.label_ids());
        assert_eq!(runs, 0);
        assert_eq!(labels["@l0"], (CHAIN + 1) as isize);
        assert_eq!(labels[&format!("@l{}", n - 1)], (CHAIN + n) as isize);
    }

    #[test]
    fn stepping_back_over_added_constraints_runs_nothing() {
        if !have_veripb() {
            return;
        }
        let (_file, mut session) = chain_session();
        for line in [
            format!("@p rup 1 ~x{CHAIN} >= 1 ;"),
            "rup 1 ~x7 >= 1 ;".to_string(),
            format!("del id {} ;", CHAIN + 1),
            "@q rup 1 ~x7 >= 1 ;".to_string(),
            "% a comment".to_string(),
            "pol 1 2 + ;".to_string(),
            "@q rup 1 ~x6 >= 1 ;".to_string(),
        ] {
            session.buffer.push(line);
        }
        session.database().unwrap();
        session.drive_forward().unwrap();
        assert_eq!(session.checked_len, 7);
        let held = session.database().unwrap();

        // Back over each line in turn: only the `del` line needs a run.
        for expected_runs in [0, 0, 0, 0, 1, 0, 0] {
            session.step_back().unwrap();
            let (database, runs, _) = counting(|| session.database().unwrap());
            assert_eq!(runs, expected_runs, "back to {}", session.checked_len);
            assert_eq!(
                entries(&database),
                entries(&fresh_database(&session)),
                "back to {}",
                session.checked_len
            );
            assert_eq!(session.label_ids(), traced_labels(&session));
        }
        assert_eq!(
            held.entries.len(),
            CHAIN + 4,
            "a database handed out is unchanged"
        );
    }

    #[test]
    fn an_edit_after_the_database_stops_stepping_back_from_it() {
        if !have_veripb() {
            return;
        }
        let (_file, mut session) = chain_session();
        push_labelled(&mut session, 6);
        session.database().unwrap();
        session.drive_forward().unwrap();
        session.set_line(4, "rup 1 ~x2 >= 1 ;").unwrap();
        session.step_back().unwrap();
        let (database, runs, _) = counting(|| session.database().unwrap());
        assert_eq!(runs, 1);
        assert_eq!(entries(&database), entries(&fresh_database(&session)));
    }
}
