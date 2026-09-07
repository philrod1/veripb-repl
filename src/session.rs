//! The REPL's core engine: session state plus the replay machinery shared by
//! every command that needs to check something against it (typing a bare
//! rule, `:check`, `:save`, `:source`, `:edit`, ...).
//!
//! Deliberately holds no printing/parsing-of-`:command`-syntax logic — that
//! belongs to the frontends' dispatch and the individual `commands::*`
//! modules. This module only knows how to build a candidate buffer, run it
//! through a fresh checker (capturing the checker's own stdout/stderr trace
//! output so callers can route it to whichever frontend is driving), and
//! classify/apply the result.
//!
//! **The buffer is a live, always-present list of proof-format lines, only
//! some prefix of which is currently checked.** `buffer: Vec<String>` never
//! shrinks except by an explicit, user-requested delete — nothing is ever
//! truncated as a side effect of a failed check. `checked_len: usize` marks
//! how much of the front of `buffer` is currently reflected in
//! `current_checker`; everything from there on is unverified text that may
//! not even parse. Typing a new line at the very end (via `append_line`)
//! checks it immediately, same as always, *provided* nothing was already
//! pending; editing an existing line (`set_line`/`insert_line`/
//! `delete_line`) never checks anything — it just retracts `checked_len` if
//! the edit falls inside the checked prefix, since everything from there
//! onward can no longer be assumed to still hold. Checking is always a
//! deliberate, explicit act (`drive_forward`, `:verify`'s entire
//! implementation) that walks forward from `checked_len` and stops — without
//! discarding anything — at the first line that doesn't check out.

use std::{
    collections::{BTreeSet, VecDeque},
    fs::File,
    io::{self, BufReader, Cursor, Read as _, Seek as _, SeekFrom, Write as _},
    ops::Bound,
};

use anyhow::Context as _;
// use veripb_checker::{error::ForwardsCheckerError, prelude::*};
// use veripb_formula::prelude::*;
// use veripb_parser::{
//     error::ParserError,
//     io::MaybeCompressed,
//     opb_parser::{parse_opb_from_file, parse_single_constraint},
//     opb_token::OPBToken,
//     pbp_parser::parser::{ParserArgs, parse_proof_with_labels},
// };

/// A synthesized v3 proof file has this preamble, occupying exactly two
/// physical lines, before any buffer/candidate lines follow.
const PREAMBLE_LINES: usize = 2;

/// `Formula` doesn't derive `Clone` (nothing in the checking path needs it
/// to), so reconstruct one from its individually-`Clone` fields instead of
/// requesting an upstream change for a REPL-only convenience.
fn clone_formula(formula: &Formula) -> Formula {
    Formula {
        constraints: formula.constraints.clone(),
        objective: formula.objective.clone(),
    }
}

/// Restores the real stdout/stderr on drop, so a panic mid-check can't
/// leave the process permanently wired to a capture file.
///
/// Platform-specific, but both sides end up operating on the same kind of
/// thing: "stdout"/"stderr" both ultimately mean small-integer file
/// descriptor slots 1/2 that `libc::dup`/`dup2`/`close` manipulate
/// identically on either platform — Unix natively, Windows via its C
/// runtime's POSIX-compatibility layer. That layer is also what Rust's
/// own stdio actually resolves through on Windows when writing, unlike
/// the raw `SetStdHandle`/`GetStdHandle` Win32 APIs, whose result can be
/// cached by code that's already running and so isn't reliably picked up
/// by a redirect performed after startup (see rust-lang/rust#40490) —
/// deliberately not used here for that reason. Only *acquiring* a
/// descriptor for the capture-file target differs between the two: Unix's
/// `std::fs::File` already has one; Windows' doesn't (it talks to a raw
/// HANDLE directly, bypassing the CRT descriptor table), so that side has
/// to mint one first — see `target_descriptor` below for the resulting
/// handle-ownership subtlety that needs care.
#[cfg(unix)]
struct RedirectGuard {
    saved_stdout: libc::c_int,
    saved_stderr: libc::c_int,
}

#[cfg(unix)]
impl RedirectGuard {
    /// Point both stdout and stderr at `target` until the guard drops.
    /// One shared target (rather than one per stream) keeps the relative
    /// interleaving of the checker's stdout and stderr writes intact.
    fn redirect_to(target: &File) -> anyhow::Result<Self> {
        use std::os::fd::AsRawFd as _;

        // Flush Rust's buffered stream wrappers so any pending text goes
        // to the real terminal, not the capture file.
        io::stdout().flush().ok();
        io::stderr().flush().ok();

        // SAFETY: `dup`/`dup2` on the standard descriptors. The std
        // stream handles keep referring to fds 1/2 throughout — only what
        // those fds point at changes, and `Drop` points them back.
        unsafe {
            let saved_stdout = libc::dup(1);
            let saved_stderr = libc::dup(2);
            if saved_stdout < 0 || saved_stderr < 0 {
                anyhow::bail!("failed to save the stdout/stderr descriptors");
            }
            if libc::dup2(target.as_raw_fd(), 1) < 0 || libc::dup2(target.as_raw_fd(), 2) < 0 {
                libc::dup2(saved_stdout, 1);
                libc::dup2(saved_stderr, 2);
                libc::close(saved_stdout);
                libc::close(saved_stderr);
                anyhow::bail!("failed to redirect stdout/stderr to the capture file");
            }
            Ok(RedirectGuard {
                saved_stdout,
                saved_stderr,
            })
        }
    }
}

#[cfg(unix)]
impl Drop for RedirectGuard {
    fn drop(&mut self) {
        // Flush while fds 1/2 still point at the capture file, so buffered
        // checker output lands in the capture rather than the terminal.
        io::stdout().flush().ok();
        io::stderr().flush().ok();
        // SAFETY: restoring the descriptors saved in `redirect_to`.
        unsafe {
            libc::dup2(self.saved_stdout, 1);
            libc::dup2(self.saved_stderr, 2);
            libc::close(self.saved_stdout);
            libc::close(self.saved_stderr);
        }
    }
}

#[cfg(windows)]
struct RedirectGuard {
    saved_stdout: libc::c_int,
    saved_stderr: libc::c_int,
}

#[cfg(windows)]
impl RedirectGuard {
    /// Mint a CRT file descriptor wrapping `target`'s content, for
    /// `redirect_to` to `dup2` onto 1/2 exactly like the Unix
    /// implementation does with `target.as_raw_fd()` directly. Windows'
    /// `std::fs::File` has no descriptor of its own to hand over — it
    /// talks to its underlying HANDLE directly — so one has to be built:
    /// clone the handle first (an *independent* duplicate, not
    /// `target`'s own one — `open_osfhandle` takes ownership of whatever
    /// handle it's given, and closing the resulting descriptor closes
    /// that handle with it, so handing over `target`'s own would leave
    /// `target` holding an already-closed handle by the time its content
    /// is read back afterward), then `mem::forget` the temporary `File`
    /// wrapper around that clone — ownership has already effectively
    /// moved to the descriptor about to wrap it, so letting the `File`
    /// *also* run its own drop would double-close the same handle.
    fn target_descriptor(target: &File) -> anyhow::Result<libc::c_int> {
        use std::os::windows::io::AsRawHandle as _;

        let duplicate = target
            .try_clone()
            .context("failed to duplicate the capture file handle")?;
        let handle = duplicate.as_raw_handle();
        std::mem::forget(duplicate);

        // SAFETY: `handle` is a valid, freshly-duplicated file handle
        // whose ownership is being handed to `open_osfhandle` — the
        // `mem::forget` above is what makes that sound, since nothing
        // else still thinks it owns `handle` after it.
        let fd = unsafe { libc::open_osfhandle(handle as libc::intptr_t, libc::O_BINARY) };
        if fd < 0 {
            anyhow::bail!("failed to open a descriptor for the capture file");
        }
        Ok(fd)
    }

    /// Point both stdout and stderr at `target` until the guard drops —
    /// see the struct's own doc comment for why this and the
    /// `#[cfg(unix)]` version both ultimately operate on the same kind of
    /// small-integer descriptor, and `target_descriptor` for why
    /// acquiring one for `target` needs the extra step here.
    fn redirect_to(target: &File) -> anyhow::Result<Self> {
        io::stdout().flush().ok();
        io::stderr().flush().ok();

        let target_fd = Self::target_descriptor(target)?;

        // SAFETY: `dup`/`dup2`/`close` on the standard descriptors, same
        // as the Unix implementation — Windows' C runtime provides the
        // same POSIX-compatibility semantics for these.
        unsafe {
            let saved_stdout = libc::dup(1);
            let saved_stderr = libc::dup(2);
            if saved_stdout < 0 || saved_stderr < 0 {
                libc::close(target_fd);
                anyhow::bail!("failed to save the stdout/stderr descriptors");
            }
            if libc::dup2(target_fd, 1) < 0 || libc::dup2(target_fd, 2) < 0 {
                libc::dup2(saved_stdout, 1);
                libc::dup2(saved_stderr, 2);
                libc::close(saved_stdout);
                libc::close(saved_stderr);
                libc::close(target_fd);
                anyhow::bail!("failed to redirect stdout/stderr to the capture file");
            }
            // Its job is done — fds 1/2 now hold their own independent
            // references to the same (duplicated) file, exactly like
            // `dup2` leaves things on the Unix side.
            libc::close(target_fd);
            Ok(RedirectGuard {
                saved_stdout,
                saved_stderr,
            })
        }
    }
}

#[cfg(windows)]
impl Drop for RedirectGuard {
    fn drop(&mut self) {
        io::stdout().flush().ok();
        io::stderr().flush().ok();
        // SAFETY: restoring the descriptors saved in `redirect_to`.
        unsafe {
            libc::dup2(self.saved_stdout, 1);
            libc::dup2(self.saved_stderr, 2);
            libc::close(self.saved_stdout);
            libc::close(self.saved_stderr);
        }
    }
}

/// Run `f` with stdout and stderr captured into a buffer, returning the
/// captured text alongside `f`'s result. The checker prints its trace
/// output (`ConstraintId N: ...`, `s VERIFIED ...`, propagation trails)
/// directly to the standard streams from dozens of call sites, so rather
/// than threading a writer through the whole upstream crate, each checking
/// call is bracketed at the fd level and the caller decides where the text
/// goes. Backed by a temp file, not a pipe, so an arbitrarily large trace
/// can't fill a pipe buffer and deadlock the checker. Cross-platform —
/// see `RedirectGuard`'s own doc comment for how the two platforms differ
/// underneath the same interface here.
fn capture_output<T>(f: impl FnOnce() -> T) -> anyhow::Result<(String, T)> {
    let mut file = tempfile::tempfile().context("failed to create capture file")?;
    let guard = RedirectGuard::redirect_to(&file)?;
    let result = f();
    drop(guard);

    let mut captured = String::new();
    file.seek(SeekFrom::Start(0))
        .context("failed to rewind capture file")?;
    file.read_to_string(&mut captured)
        .context("failed to read captured checker output")?;
    Ok((captured, result))
}

/// One `:undo`-stack entry — enough to fully restore a `Session` via
/// `restore_snapshot`. Not `current_checker` (always rebuilt from these,
/// never worth snapshotting itself) or `label_map` (purely derived —
/// replaying `buffer[..checked_len]` against `formula`/`variables`
/// recomputes it identically).
pub(crate) struct Snapshot {
    formula: Formula,
    variables: VarNameManager,
    buffer: Vec<String>,
    checked_len: usize,
    known_bad: Option<String>,
}

/// How many actions `Session::undo_stack` remembers before discarding the
/// oldest. Unbounded would mean O(actions²) worst-case memory in the
/// number of accepted lines — each entry clones the buffer *as it stood
/// then*, and a snapshot is pushed per accepted line, so summed across a
/// long session that's quadratic. 200 keeps memory bounded to a small
/// multiple of one proof's worth — comfortably more history than anyone
/// scrolls back through by hand.
const UNDO_STACK_CAP: usize = 200;

/// A `:formula` edit's undo point: enough to restore both the formula
/// constraint it replaced and the buffer exactly as it was, in one step.
/// Set by `replace_formula_constraint`, consumed by `:formula cancel` —
/// invalidated by anything else the user types in between (see
/// `commands::dispatch`), so it's never silently stale.
pub(crate) struct FormulaEditSnapshot {
    /// 1-based formula constraint number that was edited.
    pub(crate) n: usize,
    /// What used to occupy it.
    pub(crate) constraint: PBConstraintEnum,
    /// The buffer this edit tried to reverify against — the fullest one
    /// known at the time (see `Session::recoverable_buffer`), not
    /// necessarily just whatever `buffer` happened to be right before
    /// this call. Checked or not — nothing about it is discarded, only
    /// re-attempted against the new formula.
    pub(crate) buffer: Vec<String>,
}

/// The state of one REPL session: the formula it was loaded with, the
/// live proof buffer (see the module docs), and the checker instance that
/// results from replaying its checked prefix (used by inspection commands
/// like `:show`).
pub struct Session {
    /// The path the formula was loaded from, kept for display (e.g. the
    /// TUI's formula-panel title) — the file is never re-read after load.
    pub formula_path: String,
    pub formula: Formula,
    pub variables: VarNameManager,
    pub labels: ahash::AHashMap<String, isize>,
    /// The label→ConstraintID map from the most recently verified replay:
    /// the formula's own labels plus every `@label` the checked prefix of
    /// `buffer` defines. Display-only — replays are always seeded from
    /// `labels` (the formula's), so what's shown can never feed back into
    /// what's checked.
    pub label_map: ahash::AHashMap<String, isize>,
    /// Every proof-format line currently in the proof, in order, checked
    /// or not. Never truncated as a side effect of a failed check — only
    /// ever grown (a new line typed, sourced, or inserted), shrunk by an
    /// explicit, user-requested delete, or has an existing entry's text
    /// replaced in place. See the module docs for the full model.
    pub buffer: Vec<String>,
    /// How many lines from the front of `buffer` are currently reflected
    /// in `current_checker` — the checked prefix. Lines at index
    /// `>= checked_len` haven't been verified against the buffer's
    /// current content: either never attempted, or invalidated by an
    /// edit at or before their position.
    pub checked_len: usize,
    /// Set when `buffer[checked_len]` specifically is known, right now,
    /// to fail — the line `drive_forward`/`:verify` most recently stopped
    /// on. Cleared by anything that changes `checked_len` or that line's
    /// text (a fresh attempt deserves a fresh verdict, not a stale
    /// highlight). Drives the Proof pane's error highlight and `:list`'s
    /// failure tag.
    pub known_bad: Option<String>,
    pub current_checker: ForwardsChecker,
    /// Set by the most recent `:formula` edit, consumed by `:formula
    /// cancel` — see `FormulaEditSnapshot`.
    pub(crate) last_formula_edit: Option<FormulaEditSnapshot>,
    /// The fullest buffer still worth retrying against a formula edit,
    /// remembered across a *sequence* of them so one bad edit doesn't
    /// permanently drop proof lines a later, corrected edit could still
    /// recover. `None` means "just use `buffer` itself" — the normal
    /// case, and where every formula edit starts from. Set by
    /// `replace_formula_constraint` to whichever base it actually used;
    /// cleared back to `None` by `commands::formula::commit` once a
    /// reverify fully succeeds (nothing extra to remember once `buffer`
    /// already equals it), by `:formula cancel` (restoring puts them back
    /// in sync by construction), and by anything else the user types in
    /// between (see `commands::dispatch`) — the same invalidation scope
    /// `last_formula_edit` already has, and for the same reason: a
    /// deliberate action elsewhere means an old, longer buffer shouldn't
    /// silently reappear later.
    pub(crate) recoverable_buffer: Option<Vec<String>>,
    /// Bumped by every method that actually mutates `formula`/
    /// `variables`/`buffer`/`checked_len` on success — `verify_next`
    /// (on acceptance only), `append_line`, `set_line`, `insert_line`,
    /// `delete_line`, `replace_buffer_and_verify`,
    /// `replace_formula_constraint` — plus a deliberate marker bump from
    /// `commands::formula::start`, which doesn't otherwise mutate
    /// anything at mode entry but still needs `:undo` to notice a mode
    /// began. The only signal `commands::dispatch` needs to decide
    /// whether an action actually changed anything worth pushing onto
    /// `undo_stack` — no need for `PartialEq` on `Formula`/
    /// `VarNameManager` (neither derives it).
    pub(crate) generation: u64,
    /// Snapshots of state from before each undo-able action, most recent
    /// last — see `Snapshot`, `push_undo`, `pop_undo`, `restore_snapshot`.
    pub(crate) undo_stack: VecDeque<Snapshot>,
    /// Buffer indices with a debugging breakpoint toggled on, for the
    /// TUI's Vim-mode `b` key (and, eventually, `:debug`'s own `:break`)
    /// to stop stepping at. Purely a stopping-point marker — never
    /// consulted by any checking path, and deliberately *not* part of
    /// [`Snapshot`]/`:undo`: like scroll position or pane focus, a
    /// breakpoint is editor furniture, not proof state, so rewinding the
    /// buffer shouldn't rewind where you'd left a marker. Indices are
    /// kept in step with the buffer by [`Self::insert_line`]/
    /// [`Self::delete_line`] (shifted the same way `checked_len` already
    /// has to be); [`Self::set_line`] never needs to touch them, since
    /// retyping in place doesn't move anything. [`Self::reset`] clears
    /// them along with everything else.
    pub breakpoints: BTreeSet<usize>,
}

/// Everything one replay attempt produced.
struct Replay {
    accepted: bool,
    checker: ForwardsChecker,
    result: Result<(), ForwardsCheckerError>,
    captured: String,
    labels: ahash::AHashMap<String, isize>,
}

/// What happened when a freshly-typed line was handed to [`Session::append_line`].
pub enum AppendOutcome {
    /// Nothing was pending, so `line` was checked immediately and
    /// accepted — today's ordinary "type a line, see it land" flow.
    Verified { captured: String },
    /// Nothing was pending, `line` was checked immediately, and rejected
    /// — not added to the buffer at all; state left exactly as it was,
    /// same as always.
    Rejected {
        captured: String,
        error: ForwardsCheckerError,
    },
    /// Something was already pending (an unchecked tail from a prior
    /// `:source`/edit), so `line` just joined the end of it, unchecked —
    /// no check was attempted.
    Deferred,
}

impl Session {
    /// Load a formula (OPB only, for now) and start a fresh, empty session.
    pub fn load(formula_path: &str) -> anyhow::Result<Self> {
        let (formula, variables, labels, _preserved_vars) = parse_opb_from_file(formula_path)
            .with_context(|| format!("failed to parse formula file {formula_path}"))?;

        let current_checker = ForwardsChecker::new(
            Context::new(CheckerConfig::default(), variables.clone(), None),
            clone_formula(&formula),
        )
        .context("failed to initialize checker")?;

        Ok(Session {
            formula_path: formula_path.to_string(),
            formula,
            variables,
            label_map: labels.clone(),
            labels,
            buffer: Vec::new(),
            checked_len: 0,
            known_bad: None,
            current_checker,
            last_formula_edit: None,
            recoverable_buffer: None,
            generation: 0,
            undo_stack: VecDeque::new(),
            breakpoints: BTreeSet::new(),
        })
    }

    /// The labels naming each constraint ID, inverted from `label_map` —
    /// several labels can name one ID, so each gets a sorted list. Label
    /// names carry their `@` prefix already.
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

    /// The synthesized preamble lines every replay buffer starts with,
    /// exposed so `:list` and the TUI's proof panel can show them with
    /// numbering that matches the checker's own `line N:` trace output —
    /// sharing them with `preamble_and_lines` keeps what's displayed and
    /// what's checked from ever drifting apart.
    pub fn preamble_lines(&self) -> [String; PREAMBLE_LINES] {
        [
            "pseudo-Boolean proof version 3.0".to_string(),
            format!("f {};", self.formula.len()),
        ]
    }

    /// The synthesized v3 preamble plus `lines`, ready for a caller to
    /// append more text to before checking. `lines` is deliberately a
    /// parameter rather than always `&self.buffer`: checking machinery
    /// only ever wants the *checked* prefix (`buffer[..checked_len]`) fed
    /// to a fresh checker — the unchecked tail may not even parse — while
    /// `listing` (what `:save` writes) wants the whole buffer regardless
    /// of check status.
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

    /// Build a checker from arbitrary formula/variables — shared by
    /// `fresh_checker` below (the normal "rebuild from `self`" case) and
    /// `replace_formula_constraint`'s pre-commit validation, which needs
    /// to build one from *candidate*, not-yet-committed state.
    fn build_checker(
        formula: &Formula,
        variables: &VarNameManager,
        config: CheckerConfig,
    ) -> anyhow::Result<ForwardsChecker> {
        let context = Context::new(config, variables.clone(), None);
        ForwardsChecker::new(context, clone_formula(formula))
            .context("failed to initialize checker")
    }

    /// Build a fresh checker over this session's formula, configured with
    /// `config`. Every replay attempt (accepted or not) gets its own.
    fn fresh_checker(&self, config: CheckerConfig) -> anyhow::Result<ForwardsChecker> {
        Self::build_checker(&self.formula, &self.variables, config)
    }

    /// Attempt `buffer[checked_len]` — the first unchecked line — against
    /// `current_checker`. On acceptance, `checked_len` advances by one and
    /// `current_checker`/`label_map` adopt the result. On rejection,
    /// nothing about the buffer or `checked_len` changes — only
    /// `known_bad` is set, so the Proof pane/`:list` can keep flagging
    /// exactly this line until something changes it. Panics if nothing is
    /// pending (`checked_len == buffer.len()`) — callers ([`Self::drive_forward`],
    /// [`Self::append_line`]) only ever call this after confirming there is.
    fn verify_next(&mut self) -> anyhow::Result<(String, Option<ForwardsCheckerError>)> {
        let candidate_line = self.buffer[self.checked_len].clone();
        let mut text = self.preamble_and_lines(&self.buffer[..self.checked_len]);
        text.push_str(&candidate_line);
        text.push('\n');

        // Trace only the new line's derived constraints, not the whole
        // replayed prefix (which was already printed when it was first
        // accepted).
        let candidate_line_no = PREAMBLE_LINES + self.checked_len + 1;
        let config = CheckerConfig {
            trace_lines: Some(RangeSet::new(vec![(
                Bound::Included(candidate_line_no),
                Bound::Included(candidate_line_no),
            )])),
            trace_failed: true,
            print_verification_result: true,
            ..CheckerConfig::default()
        };

        let replay = self.replay_buffer(text, config)?;

        if replay.accepted {
            self.checked_len += 1;
            self.current_checker = replay.checker;
            self.label_map = replay.labels;
            self.known_bad = None;
            self.generation += 1;
            Ok((replay.captured, None))
        } else {
            let err = replay.result.unwrap_err();
            self.known_bad = Some(err.to_string());
            Ok((replay.captured, Some(err)))
        }
    }

    /// Drive `checked_len` forward through `buffer` as far as it will go:
    /// [`Self::verify_next`] in a loop, committing each accepted line and
    /// stopping — without discarding anything — at the first rejection.
    /// This is `:verify`'s entire implementation, and also what
    /// `:formula`'s post-edit reverify reduces to once its candidate
    /// buffer is in place (see [`Self::replace_buffer_and_verify`]):
    /// "some pending content exists, check as much of it as still holds."
    /// Every attempted line's captured trace is concatenated in order.
    pub fn drive_forward(&mut self) -> anyhow::Result<(String, Option<ForwardsCheckerError>)> {
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

    /// `:debug`'s `:step` primitive: attempt exactly the next unchecked
    /// line, the same underlying check [`Self::append_line`]'s "check
    /// immediately" path runs, but for a line already sitting in the
    /// buffer rather than one just typed. `Ok(None)` — not an error —
    /// once nothing is left to step onto, so callers get a REPL-native
    /// "nothing to step" outcome instead of running into
    /// [`Self::verify_next`]'s own "nothing pending" panic contract.
    pub fn step(&mut self) -> anyhow::Result<Option<(String, Option<ForwardsCheckerError>)>> {
        if self.checked_len >= self.buffer.len() {
            return Ok(None);
        }
        self.verify_next().map(Some)
    }

    /// `:debug`'s `:continue`/`:until <n>` engine: run forward one line
    /// at a time — [`Self::verify_next`] in a loop, the same engine
    /// [`Self::drive_forward`] already uses for `:verify` — stopping at
    /// the first of: a rejection, `target` (if given, a one-off
    /// destination for `:until` rather than a standing breakpoint), or a
    /// registered [`Self::breakpoints`] entry. The breakpoint check is
    /// skipped on the very first iteration (`first`, below) so resuming
    /// from a line that's itself marked doesn't just immediately re-stop
    /// there without making any progress — the same "continue past the
    /// breakpoint you're standing on" behavior most debuggers give
    /// `continue`. Deliberately *not* what `drive_forward` uses:
    /// `:verify` stays breakpoint-oblivious on purpose, so there's always
    /// a plain "just check everything" that no marker can interrupt —
    /// only `:debug`'s own forward-moving commands need this variant.
    pub fn continue_run(
        &mut self,
        target: Option<usize>,
    ) -> anyhow::Result<(String, Option<ForwardsCheckerError>)> {
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

    /// The ordinary `pbp>`-prompt path for a freshly-typed line. If
    /// nothing was already pending (`checked_len == buffer.len()` before
    /// this call), `line` is checked immediately — today's exact "instant
    /// feedback" behavior, rejection included (not added to the buffer at
    /// all, state left exactly as it was). If something was already
    /// pending, `line` just joins the end of it, unchecked, same as
    /// `:source` appending — there's no point checking a new line ahead
    /// of ones that haven't been checked yet.
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

    /// Replace `buffer[idx]`'s text in place — the mechanism behind
    /// `:edit`/`:deassert`/`:insert`'s retyping and the TUI browse mode's
    /// commit action. Never checks the new text (that's what `:verify` is
    /// for) and never touches any other buffer entry. If `idx` was inside
    /// the checked prefix, retract `checked_len` to `idx` and rebuild
    /// `current_checker` from the new, shorter (still-known-good) prefix
    /// — everything from `idx` on can no longer be assumed to still hold,
    /// but nothing is removed, only marked unchecked again.
    pub fn set_line(&mut self, idx: usize, new_text: &str) -> anyhow::Result<()> {
        self.buffer[idx] = new_text.to_string();
        if idx < self.checked_len {
            self.checked_len = idx;
            self.rebuild_checked_checker()?;
        }
        self.known_bad = None;
        self.generation += 1;
        Ok(())
    }

    /// Insert a new, unchecked line before buffer index `idx` (`:insert`'s
    /// mechanism) — shifts everything at/after `idx` down without
    /// touching its text. Same "retract `checked_len` if `idx` falls
    /// inside the checked prefix" rule as [`Self::set_line`].
    pub fn insert_line(&mut self, idx: usize, text: &str) -> anyhow::Result<()> {
        self.buffer.insert(idx, text.to_string());
        if idx < self.checked_len {
            self.checked_len = idx;
            self.rebuild_checked_checker()?;
        }
        self.breakpoints = self
            .breakpoints
            .iter()
            .map(|&b| if b >= idx { b + 1 } else { b })
            .collect();
        self.known_bad = None;
        self.generation += 1;
        Ok(())
    }

    /// Remove buffer index `idx` outright (`:delete`'s mechanism) — same
    /// "retract `checked_len` if `idx` falls inside the checked prefix"
    /// rule as [`Self::set_line`]; removing an already-unchecked line
    /// needs no rebuild at all.
    pub fn delete_line(&mut self, idx: usize) -> anyhow::Result<()> {
        self.buffer.remove(idx);
        if idx < self.checked_len {
            self.checked_len = idx;
            self.rebuild_checked_checker()?;
        }
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

    /// Toggle a breakpoint on buffer index `idx`: set it if absent, clear
    /// it if present. The TUI's Vim-mode `b` key and (eventually)
    /// `:debug`'s own `:break` command both reduce to this one call —
    /// same "click/keystroke does the obvious opposite of the current
    /// state" toggle already used for zoom (`App::toggle_zoom`). Doesn't
    /// bump `generation`: a breakpoint isn't proof state, so it's not
    /// something `:undo` should ever see as "something changed" (see
    /// `breakpoints`'s own docs).
    pub fn toggle_breakpoint(&mut self, idx: usize) {
        if !self.breakpoints.remove(&idx) {
            self.breakpoints.insert(idx);
        }
    }

    /// Rebuild `current_checker`/`label_map` from `buffer[..checked_len]`
    /// — always a subset of what was already verified before this call,
    /// so guaranteed to replay clean (an internal-bug backstop otherwise,
    /// same guarantee `restore_snapshot` already relies on). Shared by
    /// every mutator above that can shrink the checked prefix without
    /// touching what remains inside it.
    fn rebuild_checked_checker(&mut self) -> anyhow::Result<()> {
        let text = self.preamble_and_lines(&self.buffer[..self.checked_len]);
        let config = CheckerConfig {
            trace_failed: true,
            print_verification_result: true,
            ..CheckerConfig::default()
        };
        let replay = self.replay_buffer(text, config)?;
        if replay.accepted {
            self.current_checker = replay.checker;
            self.label_map = replay.labels;
            Ok(())
        } else {
            anyhow::bail!(
                "internal error: replaying an already-checked prefix was rejected: {}",
                replay.result.unwrap_err()
            )
        }
    }

    /// Replace the buffer wholesale with `lines` — fresh content to try
    /// (e.g. what a `:formula` edit is attempting to recover) — and check
    /// as much of it as still holds: one O(n) batched replay first, fast
    /// for the overwhelmingly common case that it all still does; only if
    /// that fails does it fall back to [`Self::drive_forward`]'s
    /// one-line-at-a-time loop, to pinpoint exactly where and leave
    /// everything after it sitting in the buffer, unchecked, rather than
    /// lost. Same two-tier approach `:source` used to use for its own
    /// bulk-loading, before it stopped checking on load at all.
    pub(crate) fn replace_buffer_and_verify(
        &mut self,
        lines: Vec<String>,
    ) -> anyhow::Result<(String, Option<ForwardsCheckerError>)> {
        self.buffer = lines;
        self.checked_len = 0;
        self.known_bad = None;
        if self.buffer.is_empty() {
            return Ok((String::new(), None));
        }

        let text = self.preamble_and_lines(&self.buffer);
        let candidate_start = PREAMBLE_LINES + 1;
        let candidate_end = candidate_start + self.buffer.len() - 1;
        let config = CheckerConfig {
            trace_lines: Some(RangeSet::new(vec![(
                Bound::Included(candidate_start),
                Bound::Included(candidate_end),
            )])),
            trace_failed: true,
            print_verification_result: true,
            ..CheckerConfig::default()
        };
        let replay = self.replay_buffer(text, config)?;
        if replay.accepted {
            self.checked_len = self.buffer.len();
            self.current_checker = replay.checker;
            self.label_map = replay.labels;
            self.generation += 1;
            return Ok((replay.captured, None));
        }
        // The batched attempt failed somewhere in here. Its own output is
        // deliberately dropped rather than printed — `drive_forward`
        // below re-derives the successful prefix for real, and printing
        // both would show the same trace lines twice.
        self.drive_forward()
    }

    /// Restore `buffer`/`checked_len`/`known_bad` directly (formula and
    /// variables untouched) and rebuild `current_checker` from the
    /// restored checked prefix — the edit-family commands' `:cancel`
    /// mechanism (`commands::edit`): they never touch the formula, so a
    /// full [`Snapshot`]/[`Self::restore_snapshot`] would be more than
    /// they need. Same "was genuinely valid when captured, so rebuilding
    /// it can't fail in practice" guarantee as everything else that calls
    /// `rebuild_checked_checker`.
    pub(crate) fn restore_buffer_state(
        &mut self,
        buffer: Vec<String>,
        checked_len: usize,
        known_bad: Option<String>,
    ) -> anyhow::Result<()> {
        self.buffer = buffer;
        self.checked_len = checked_len;
        self.known_bad = known_bad;
        self.rebuild_checked_checker()
    }

    /// Drop every buffer line — checked or not — and rebuild
    /// `current_checker` from the formula alone, as if nothing had ever
    /// been typed — without touching `formula`/`variables`/`labels`.
    pub fn reset(&mut self) -> anyhow::Result<()> {
        self.current_checker = self.fresh_checker(CheckerConfig::default())?;
        self.buffer.clear();
        self.checked_len = 0;
        self.known_bad = None;
        self.label_map = self.labels.clone();
        self.breakpoints.clear();
        // Mutates the session in place rather than replacing it (unlike
        // `:load`/`:instance`), so `undo_stack` needs an explicit clear —
        // same "hard boundary, nothing to undo past this" as those.
        self.undo_stack.clear();
        Ok(())
    }

    /// Retract `checked_len` to `target` (must be `<= checked_len`),
    /// rebuilding `current_checker`/`label_map` from the new, shorter
    /// checked prefix — the same "was genuinely valid when captured, so
    /// rebuilding it can't fail in practice" guarantee
    /// `rebuild_checked_checker` always relies on. Shared by
    /// [`Self::step_back`] (`target = checked_len - 1`) and
    /// [`Self::restart`] (`target = 0`) — `:debug`'s own analog of
    /// [`Self::set_line`]'s "retract if `idx` falls inside the checked
    /// prefix" rule, just without any text ever changing.
    fn retract_checked_len(&mut self, target: usize) -> anyhow::Result<()> {
        debug_assert!(target <= self.checked_len);
        self.checked_len = target;
        self.known_bad = None;
        self.rebuild_checked_checker()?;
        self.generation += 1;
        Ok(())
    }

    /// `:back`: step the checked prefix backward by exactly one line, if
    /// possible. Nothing is re-verified going backward — there's nothing
    /// to check, only to forget — so this can never fail the way
    /// stepping forward can. Returns whether anything actually moved
    /// (`false` right at the start, with nothing checked yet to step
    /// back from).
    pub fn step_back(&mut self) -> anyhow::Result<bool> {
        if self.checked_len == 0 {
            return Ok(false);
        }
        self.retract_checked_len(self.checked_len - 1)?;
        Ok(true)
    }

    /// `:restart`: retract the checked prefix all the way back to the
    /// start, keeping every buffer line exactly as it is — the
    /// non-destructive sibling of [`Self::reset`] (which also clears the
    /// buffer itself). A successful no-op if nothing was checked yet.
    pub fn restart(&mut self) -> anyhow::Result<()> {
        if self.checked_len == 0 {
            return Ok(());
        }
        self.retract_checked_len(0)
    }

    /// Convert a 1-based *display* line number — the numbering `:list`,
    /// the proof panel, and the checker's own `line N:` trace output all
    /// share, where the two synthesized preamble lines come first — into
    /// a 0-based index into `buffer`. `None` for a preamble line or
    /// anything at or past the end of the buffer, so callers (namely
    /// `:edit`) can turn it into a clear REPL-native error instead of a
    /// panic or a silently wrong line. Says nothing about whether the
    /// line is actually *checked* — see [`Self::checked_index`] for that.
    pub fn buffer_index(&self, display_line: usize) -> Option<usize> {
        let idx = display_line.checked_sub(PREAMBLE_LINES + 1)?;
        (idx < self.buffer.len()).then_some(idx)
    }

    /// Like [`Self::buffer_index`], but additionally requires the line to
    /// actually be checked — `None` for the preamble, past the end of the
    /// buffer, *or* sitting in the unchecked tail. Used by `:explain`/
    /// `:why`, which replay real, already-accepted content and would
    /// otherwise risk replaying text that may not even parse.
    pub fn checked_index(&self, display_line: usize) -> Option<usize> {
        let idx = self.buffer_index(display_line)?;
        (idx < self.checked_len).then_some(idx)
    }

    /// The inverse of `buffer_index`: the display line number for a
    /// 0-based `buffer` index. Always valid for an in-range index — no
    /// `Option` needed, unlike the other direction, since it's never
    /// asked to validate arbitrary user input. Unaffected by checked
    /// status: since the buffer is never truncated, a line's number never
    /// shifts just because something later became unchecked.
    pub fn display_line(&self, buffer_idx: usize) -> usize {
        buffer_idx + PREAMBLE_LINES + 1
    }

    /// Convert a 1-based *formula* constraint number — the numbering the
    /// Formula pane and `:show` already display constraints with — into a
    /// 0-based index into `formula.constraints`. A genuinely distinct
    /// numbering space from `buffer_index`'s (proof buffer lines): both
    /// happen to start near 1, but one counts formula constraints, the
    /// other counts buffer lines after the synthesized preamble. `None`
    /// for anything out of range, so `:formula` can turn it into a clear
    /// REPL-native error instead of a panic.
    pub(crate) fn formula_index(&self, n: usize) -> Option<usize> {
        (n >= 1 && n <= self.formula.len()).then(|| n - 1)
    }

    /// A clone of everything `restore_snapshot` needs to bring `self` back
    /// to exactly its current state.
    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot {
            formula: clone_formula(&self.formula),
            variables: self.variables.clone(),
            buffer: self.buffer.clone(),
            checked_len: self.checked_len,
            known_bad: self.known_bad.clone(),
        }
    }

    /// Push `snapshot` onto the undo stack, discarding the oldest entry
    /// first if already at `UNDO_STACK_CAP`.
    pub(crate) fn push_undo(&mut self, snapshot: Snapshot) {
        if self.undo_stack.len() == UNDO_STACK_CAP {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(snapshot);
    }

    /// Pop the most recent undo-stack entry, if any.
    pub(crate) fn pop_undo(&mut self) -> Option<Snapshot> {
        self.undo_stack.pop_back()
    }

    /// Restore `self` to exactly what `snapshot` captured: `formula`/
    /// `variables`/`buffer`/`checked_len`/`known_bad` set directly, then
    /// `current_checker`/`label_map` rebuilt from the restored checked
    /// prefix — same "was genuinely valid when captured, so rebuilding it
    /// can't fail in practice" guarantee `rebuild_checked_checker` always
    /// relies on. Also clears `last_formula_edit`/`recoverable_buffer`:
    /// whatever they referred to is, by definition, no longer "the most
    /// recent thing that happened" once something's been restored out
    /// from under them.
    pub fn restore_snapshot(&mut self, snapshot: Snapshot) -> anyhow::Result<()> {
        self.formula = snapshot.formula;
        self.variables = snapshot.variables;
        self.buffer = snapshot.buffer;
        self.checked_len = snapshot.checked_len;
        self.known_bad = snapshot.known_bad;
        self.last_formula_edit = None;
        self.recoverable_buffer = None;
        self.rebuild_checked_checker()
    }

    /// Replace formula constraint `n` with `new_text`, parsed as a single
    /// bare OPB constraint (`:formula`'s mechanism). Every fallible step —
    /// bounds check, parse, the "no `=` constraints" restriction, and
    /// actually constructing a checker against the edited formula —
    /// happens against candidate/cloned state before `self` is touched at
    /// all, so a rejection leaves `self` exactly as it was, not just
    /// mostly: `ForwardsChecker::new` itself can fail, and it must not do
    /// so after `self` has already been mutated. On success, `buffer` is
    /// cleared (`checked_len`/`known_bad` reset with it) and
    /// `current_checker` is the already-validated candidate checker —
    /// reverifying whatever of the base buffer still holds against the
    /// new formula is the caller's job (`commands::formula`, via
    /// [`Self::replace_buffer_and_verify`]). That base is
    /// `recoverable_buffer` if one is already remembered (a prior edit in
    /// this sequence only partially reverified) or `buffer` itself
    /// otherwise, checked or not — nothing from before a formula edit is
    /// discarded, only re-attempted against the new formula. Either way
    /// it's what `last_formula_edit`/`recoverable_buffer` both get set
    /// to, so `:formula cancel` can undo both the formula and the fullest
    /// known buffer together in one step, and the next edit (if this one
    /// doesn't fully succeed either) gets to try the same base again
    /// instead of compounding the loss.
    ///
    /// Only in-place, same-count edits are supported: an OPB `=`
    /// constraint parses as *two* constraints (see
    /// `veripb-parser::opb_parser::get_constraints_from_terms`), which
    /// would change the total count and reindex everything after it —
    /// out of scope here, so it's rejected rather than silently
    /// mishandled.
    ///
    /// New variable names in `new_text` are parsed against a *clone* of
    /// `variables`, not a fresh/empty one and not `variables` in place:
    /// `VarNameManager::add_by_name` assigns a new name whatever index
    /// equals the registry's current length, so parsing against an empty
    /// registry would hand out index `0` regardless of what `0` already
    /// means in the real session, silently corrupting variable identity
    /// in a way no later re-check could ever detect (the resulting
    /// constraint is syntactically fine — it would just mean something
    /// other than what was typed). Continuing from a clone of the real
    /// registry avoids the collision, and only committing that clone back
    /// on success keeps a rejected edit from leaving any trace.
    pub fn replace_formula_constraint(&mut self, n: usize, new_text: &str) -> anyhow::Result<()> {
        let idx = self.formula_index(n).ok_or_else(|| {
            anyhow::anyhow!(
                "constraint {n} is out of range — the formula has {} constraint(s) (1-{}).",
                self.formula.len(),
                self.formula.len()
            )
        })?;

        let mut candidate_vars = self.variables.clone();
        // The trailing `;` OPB constraint syntax requires is implied when
        // editing one already-formatted constraint at a time — infer it
        // if the caller didn't already type one, rather than rejecting an
        // otherwise-valid replacement over a missing terminator nothing
        // displayed to begin with (see `to_pretty_string`).
        let mut parse_text = new_text.trim().to_string();
        if !parse_text.ends_with(';') {
            parse_text.push_str(" ;");
        }
        let mut lex = OPBToken::lexer(&parse_text);
        let (geq, leq) = parse_single_constraint(&mut lex, &mut candidate_vars)
            .with_context(|| format!("failed to parse '{new_text}' as an OPB constraint"))?;

        if leq.is_some() {
            anyhow::bail!(
                "an `=` constraint splits into two (`>=` and `<=`) — editing one formula \
                 constraint in place can't change how many there are. Use `>=` or `<=` \
                 instead, or edit the OPB file and :load it if the constraint count needs \
                 to change."
            );
        }

        let mut candidate_formula = clone_formula(&self.formula);
        candidate_formula.constraints[idx] = geq;

        // Prove the candidate state actually builds a working checker
        // before committing anything.
        let new_checker = Self::build_checker(
            &candidate_formula,
            &candidate_vars,
            CheckerConfig::default(),
        )
        .context("failed to initialize checker for the edited formula")?;

        // The base to reverify against isn't necessarily `buffer` as it
        // stands right now — if an earlier edit in this same sequence
        // only partially reverified, `recoverable_buffer` still holds the
        // fuller buffer that edit was trying to restore, so this one gets
        // another shot at it too, rather than compounding the loss by
        // starting from an already-shortened base.
        let base_buffer = match &self.recoverable_buffer {
            Some(b) => b.clone(),
            None => self.buffer.clone(),
        };

        // Nothing below this line can fail — commit as one unit.
        let prior_constraint = std::mem::replace(
            &mut self.formula.constraints[idx],
            candidate_formula.constraints[idx].clone(),
        );
        self.buffer.clear();
        self.checked_len = 0;
        self.known_bad = None;
        self.variables = candidate_vars;
        self.current_checker = new_checker;
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

    /// Run `text` through a fresh checker configured with `config`, with
    /// the checker's own stdout/stderr output captured, classifying the
    /// outcome the same way `verify_next` always has: accepted if parsing
    /// succeeds outright, or if it fails exactly at the end of the text
    /// (the parser wanting more input, not rejecting what it saw). Shared
    /// by every caller that needs to replay candidate proof text —
    /// `verify_next`, `rebuild_checked_checker`, `replace_buffer_and_verify`,
    /// `undo`, and `dry_run_conclusion`.
    fn replay_buffer(&self, text: String, config: CheckerConfig) -> anyhow::Result<Replay> {
        let total_len = text.len();
        let mut checker = self.fresh_checker(config)?;

        let mut reader = MaybeCompressed::Plain(BufReader::new(Cursor::new(text.into_bytes())));
        let parser_args = ParserArgs {
            show_progress: false,
            trace_enabled: true,
            store_assertion_annotations: false,
        };

        let (captured, (labels, result)) = capture_output(|| {
            parse_proof_with_labels(
                Some(total_len as u64),
                &mut reader,
                &mut checker,
                self.labels.clone(),
                parser_args,
            )
        })?;

        let accepted = match &result {
            // Shouldn't normally happen without a synthesized tail, but
            // treat a clean finish as acceptance rather than asserting it
            // can't occur (it can — see `dry_run_conclusion` below, and a
            // manually-typed closing sequence).
            Ok(()) => true,
            Err(ForwardsCheckerError::Parse(ParserError::ProofFormatError { pos, .. })) => {
                pos.pos >= total_len
            }
            Err(_) => false,
        };

        Ok(Replay {
            accepted,
            checker,
            result,
            captured,
            labels,
        })
    }

    /// The full synthesized `.pbp` text this session currently
    /// represents: the v3 preamble, the `f N;` formula-constraint-count
    /// line (genuinely optional in v3 syntax — included here for parity
    /// with a real `.pbp` file, and because it's free, not because it's
    /// required), **the entire buffer, checked or not**, and a closing
    /// `output NONE; conclusion <conclusion>; end pseudo-Boolean proof;`
    /// sequence. Pure text construction — never runs this through the
    /// parser, so it makes no claim that the result actually verifies;
    /// see `dry_run_conclusion` for that (which deliberately uses only
    /// the *checked* prefix — see its own docs).
    pub fn listing(&self, conclusion: &str) -> String {
        let mut text = self.preamble_and_lines(&self.buffer);
        text.push_str("output NONE;\n");
        text.push_str(&format!("conclusion {conclusion};\n"));
        text.push_str("end pseudo-Boolean proof;\n");
        text
    }

    /// Non-destructively test whether the *checked* prefix of the buffer,
    /// followed by `output NONE; conclusion <conclusion>; end
    /// pseudo-Boolean proof;`, would form a complete, valid proof. Never
    /// mutates the session regardless of outcome — the resulting checker
    /// is discarded either way. Deliberately ignores any unchecked tail:
    /// checking a conclusion only means anything once the derivation
    /// leading to it has actually been verified, and running this against
    /// unverified (possibly unparseable) text would just be misleading.
    /// `conclusion` is spliced in verbatim (e.g. `"NONE"`, `"UNSAT"`,
    /// `"BOUNDS 0 10"`), validated by the real parser, not by this REPL.
    /// The captured `String` is whatever the checker printed during the
    /// run — `s VERIFIED ...` on success, normally nothing on failure —
    /// for the caller to route (or, for a silent probe, drop).
    pub fn dry_run_conclusion(
        &self,
        conclusion: &str,
    ) -> anyhow::Result<(String, Result<(), ForwardsCheckerError>)> {
        let mut text = self.preamble_and_lines(&self.buffer[..self.checked_len]);
        text.push_str("output NONE;\n");
        text.push_str(&format!("conclusion {conclusion};\n"));
        text.push_str("end pseudo-Boolean proof;\n");
        let config = CheckerConfig {
            trace_failed: true,
            print_verification_result: true,
            ..CheckerConfig::default()
        };

        let replay = self.replay_buffer(text, config)?;
        Ok((replay.captured, replay.result))
    }

    /// Non-destructively re-derive proof line `display_line` alone, with
    /// the checker's own tracing turned up for just that line — `pol`'s
    /// full step-by-step derivation table, `red`'s substitution witness
    /// and proofgoal listing, or (for every other rule, until a future
    /// checker change adds one) the same baseline `"ConstraintId N: ..."`
    /// print every rule gets. Reuses the exact non-destructive replay
    /// `dry_run_conclusion` already relies on — a fresh, throwaway checker
    /// per call, `self` never touched — and the same per-line trace
    /// scoping `verify_next` already uses to show only the newest line's
    /// own output, not the whole replayed prefix. Only ever replays
    /// through `display_line`, not the whole buffer — the unchecked tail
    /// (if any) past it is irrelevant to explaining an earlier line, and
    /// may not even parse.
    ///
    /// The inner `Result` is the REPL-native error channel (a bad
    /// `display_line`, or one that hasn't been checked yet — same checks
    /// [`Self::checked_index`] does) — a genuinely ordinary outcome,
    /// unlike the *other* way this can produce `Ok(Err(..))`: replay
    /// rejecting a line already in the checked prefix, which should be
    /// unreachable (replaying it again is expected to succeed identically
    /// — same invariant `rebuild_checked_checker` relies on) but is still
    /// reported rather than risking a panic if it somehow isn't.
    pub fn explain_line(&self, display_line: usize) -> anyhow::Result<Result<String, String>> {
        let Some(idx) = self.checked_index(display_line) else {
            return Ok(Err(unchecked_or_out_of_range(self, display_line)));
        };

        let text = self.preamble_and_lines(&self.buffer[..=idx]);
        let config = CheckerConfig {
            trace_lines: Some(RangeSet::new(vec![(
                Bound::Included(display_line),
                Bound::Included(display_line),
            )])),
            trace_pol: true,
            trace_failed: true,
            print_verification_result: false,
            ..CheckerConfig::default()
        };

        let replay = self.replay_buffer(text, config)?;
        if replay.accepted {
            Ok(Ok(replay.captured))
        } else {
            Ok(Err(format!(
                "internal error: replaying line {display_line}, which should already be \
                 checked, was rejected: {}",
                replay.result.unwrap_err()
            )))
        }
    }

    /// Non-destructively compute the *minimized* set of hints the checker
    /// actually needed to derive proof line `display_line` (which must be
    /// a `rup` step) — the already-derived constraints (and/or the rule's
    /// own negation) whose propagation genuinely contributed to the
    /// conflict, as opposed to whatever hint list, if any, was actually
    /// typed. This is real data the checker already computes internally
    /// for every accepted RUP step, explicit-hint or bare alike
    /// (`RUPRule::compute` in `veripb-checker/src/rules/rup.rs` builds
    /// exactly this list either way — filtering a typed hint down to only
    /// the ones that propagated something, or, with no hint typed at all,
    /// via `PropagationEngine::analyze`'s own trail-minimization) — but
    /// only once its `Elaborator` is switched on, which happens only when
    /// `CheckerConfig::elaboration_path` names a real file to write to
    /// (`Elaborator::new` does `File::create` directly; there's no
    /// in-memory variant). So this replays just the checked prefix through
    /// `display_line` — nothing after it can change what was needed to
    /// reach *this* conflict, and stopping exactly there makes it
    /// unambiguously the buffer's last rule — with elaboration pointed at
    /// a scratch [`tempfile::NamedTempFile`], reads back the one
    /// `rup <constraint> : <hints>;` line it wrote, and parses its hint
    /// tokens. Kept entirely inside `veripb-repl` on purpose: nothing in
    /// `veripb-checker`/`veripb-rules` changes for this — if the REPL
    /// becomes a permanent fixture, a proper in-memory elaboration hook
    /// would be the cleaner long-term answer, but this scratch-file
    /// replay is a reasonable way to get real data today without touching
    /// upstream at all.
    ///
    /// The inner `Result` is the REPL-native error channel: a bad or
    /// unchecked `display_line` (same check `explain_line` does), or
    /// `display_line` turning out not to be a `rup` step after all — the
    /// caller should already have checked this (see
    /// `commands::why::is_rup_line`), so reaching here is a backstop, not
    /// the expected path.
    pub fn rup_needed_hints(
        &self,
        display_line: usize,
    ) -> anyhow::Result<Result<Vec<RupHint>, String>> {
        let Some(idx) = self.checked_index(display_line) else {
            return Ok(Err(unchecked_or_out_of_range(self, display_line)));
        };

        let text = self.preamble_and_lines(&self.buffer[..=idx]);

        let scratch = tempfile::NamedTempFile::new()
            .context("failed to create a scratch file for RUP elaboration")?;
        let config = CheckerConfig {
            elaboration_path: Some(scratch.path().to_path_buf()),
            print_verification_result: false,
            ..CheckerConfig::default()
        };

        let replay = self.replay_buffer(text, config)?;
        if !replay.accepted {
            return Ok(Err(format!(
                "internal error: replaying line {display_line}, which should already be \
                 checked, was rejected: {}",
                replay.result.unwrap_err()
            )));
        }
        // The scratch file only finishes flushing once its owning checker
        // — and so the `Elaborator`'s buffered writer inside it — drops;
        // nothing exposes an explicit flush, so dropping here is the only
        // way to be sure everything it wrote actually reached disk before
        // reading it back below.
        drop(replay.checker);

        let elaborated = std::fs::read_to_string(scratch.path())
            .context("failed to read back the RUP elaboration scratch file")?;

        match parse_last_rup_hints(&elaborated) {
            Some(hints) => Ok(Ok(hints)),
            None => Ok(Err(format!(
                "line {display_line} doesn't look like a `rup` step — nothing to explain with \
                 :why."
            ))),
        }
    }
}

/// The shared "nothing to explain there" message for `explain_line`/
/// `rup_needed_hints`: distinguishes an out-of-range/preamble line from
/// one that's simply sitting unchecked, since the fix is different (a
/// real line number vs. `:verify` it first).
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

/// One hint the checker's own RUP-elaboration minimizer says was actually
/// needed to reach the conflict — see [`Session::rup_needed_hints`].
#[derive(Debug, Clone, Copy)]
pub enum RupHint {
    /// The rule's own negated constraint reached the conflict directly —
    /// no other constraint's propagation was needed at all.
    NegatedPremise,
    /// This already-derived constraint's ID contributed a needed
    /// propagation.
    ConstraintId(usize),
}

/// The hint tokens off the *last* `rup <constraint> : <hints>;` line in
/// elaborated proof text `text` — see `Session::rup_needed_hints`, whose
/// buffer always ends with exactly one rule, making "last" unambiguous
/// regardless of how many earlier `rup` steps the prefix also contains.
/// `None` if no such line exists, or its hint tokens don't all parse (an
/// empty hint list included — `PropagationEngine::analyze` always
/// populates at least one entry whenever a RUP step actually accepts, so
/// a genuinely empty list here would itself be a sign something's wrong).
fn parse_last_rup_hints(text: &str) -> Option<Vec<RupHint>> {
    let line = text
        .lines()
        .rev()
        .find(|line| line.split_whitespace().next() == Some("rup"))?;
    let content = line.trim_start().strip_prefix("rup ")?.trim_end();
    let content = content.strip_suffix(';')?;
    let (_, hints_part) = content.rsplit_once(':')?;
    hints_part
        .split_whitespace()
        .map(|tok| {
            if tok == "~" {
                Some(RupHint::NegatedPremise)
            } else {
                tok.parse::<usize>().ok().map(RupHint::ConstraintId)
            }
        })
        .collect()
}
