//! The crossterm multi-panel frontend: three columns (formula, database,
//! proof buffer) over a full-width output pane with the familiar
//! `pbp>` prompt. Hand-rolled on raw crossterm rather than a widget
//! framework — the layout is small and fixed, and owning the event loop
//! outright leaves room for the depth-aware-prompt/subproof UX to come.
//!
//! Everything session-related is the exact same code the plain frontend
//! uses (`commands::dispatch` and friends); this module only decides what
//! the screen looks like and where keystrokes go. Checking never prints
//! mid-draw: the checker's output is captured per replay (see
//! `session.rs`) and lands in the scrollback like any other command text.
//!
//! `:edit` and `:formula` are the two places the TUI's behavior genuinely
//! diverges from `commands::dispatch`, rather than just rendering its
//! output differently — `:edit` (with `:deassert`/`:insert`) gets the
//! Proof pane's Vim-style modal editor (`VimState` below), `:formula` its
//! own arrow-cursor browse mode (`FormulaBrowse` below), instead of the
//! plain frontend's queue-based flow. See `App::execute`.

mod complete;
mod draw;
mod input;
mod layout;
mod theme;

use std::io;
use std::time::{Duration, Instant};

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind},
    execute, terminal,
};

use crate::commands::{self, Flow, debug, edit, formula, help};
use crate::output::Output;
use crate::session::Session;

/// How many lines the bottom pane remembers before dropping the oldest.
const SCROLLBACK_CAP: usize = 10_000;

/// Lines per mouse-wheel tick.
const WHEEL_STEP: usize = 3;

/// Columns per ←/→ horizontal-scroll step in a focused top pane.
const HSCROLL_STEP: usize = 8;

/// Columns per horizontal wheel tick (Shift+wheel or a native sideways
/// scroll) — smaller than the key step, since trackpads stream ticks.
const WHEEL_HSTEP: usize = 4;

/// Rows of context `scroll_proof_to_cursor` tries to keep above/below the
/// browse cursor, capped by the pane's actual height.
const BROWSE_MARGIN: usize = 3;

/// Two left-clicks on the same cell within this long count as a
/// double-click.
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);

/// The four scrollable panes. Shift+Tab cycles focus through them;
/// PgUp/PgDn scroll whichever is focused. Typing always goes to the
/// prompt regardless of focus.
#[derive(Clone, Copy, PartialEq)]
pub enum Pane {
    Formula,
    Database,
    Proof,
    Output,
}

impl Pane {
    fn next(self) -> Pane {
        match self {
            Pane::Output => Pane::Formula,
            Pane::Formula => Pane::Database,
            Pane::Database => Pane::Proof,
            Pane::Proof => Pane::Output,
        }
    }
}

/// The "make this pane bigger" states a pane can be zoomed to, toggled by
/// clicking its header's `[▭]`/`[⛶]` buttons (see `layout::zoom_levels`,
/// `layout::button_offsets`, and `App::toggle_zoom`) or cleared with Esc.
/// For a top pane (Formula, Database, or Proof), `Wide` takes over the
/// whole top row in place of the other two columns but leaves `top_h` —
/// and so the bottom Output/prompt area — exactly as it is normally;
/// `Full` goes further, additionally shrinking the bottom area down to
/// its existing small-terminal floor (see `Layout::compute`) so the
/// zoomed pane gets nearly the entire screen. `Output` is the mirror
/// image, both in direction and in degree: `Wide` (semi-maximise) shrinks
/// the top area down to *its* floor — all three columns stay visible,
/// just squeezed to a few lines — while `Full` (full-maximise) removes
/// the top area entirely, so Output alone fills the screen with no
/// separator row left to show it off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zoom {
    Wide,
    Full,
}

/// Bottom-pane transcript of everything commands and the checker have
/// emitted, plus an echo of each submitted line. The TUI's `Output` sink.
pub struct Scrollback {
    lines: Vec<String>,
}

impl Scrollback {
    fn new() -> Self {
        Scrollback { lines: Vec::new() }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    fn push(&mut self, text: &str) {
        if self.lines.len() == SCROLLBACK_CAP {
            self.lines.remove(0);
        }
        // A literal tab (e.g. `CheckingLineError`'s own `Display`, which
        // hand-indents its detail line with one) is one character by
        // `draw.rs`'s width math, but a real terminal renders it as an
        // actual tab — jumping to the next 8-column stop from wherever
        // the cursor happens to be, not advancing by exactly one column.
        // In a raw-mode, absolutely-positioned frame that mismatch throws
        // off everything rendered after it on the row: the fixed-width
        // padding meant to blank out stale content from a previous frame
        // lands at the wrong columns, most visible as leftover characters
        // that never get cleared when scrolling back to a tab-indented
        // line's start. Same class of bug as an embedded newline (see
        // `output::error`'s own docs) — sidestepped the same way, by
        // never letting the raw control character reach the terminal.
        // A fixed run of spaces reads as "indented" without the hazard.
        self.lines.push(text.replace('\t', "    "));
    }
}

impl Output for Scrollback {
    fn line(&mut self, text: &str) {
        self.push(text);
    }
}

/// Which half of Vim-style editing the Proof pane is in — see
/// `VimState`'s own docs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum VimSubMode {
    /// Read-only cursor navigation; single keys are commands
    /// (`h`/`j`/`k`/`l`, `i`/`a`/`o`/`O`, `x`, `dd`), not text.
    Normal,
    /// Free typing, via the same `App::editor: LineEditor` the ordinary
    /// prompt uses — Enter opens the next proof line; Esc commits and
    /// returns to `Normal`.
    Insert,
}

/// TUI-only Vim-style editing of the Proof pane, entered by `:edit`/
/// `:deassert`/`:insert` or double-clicking a Proof line — see
/// `USER_GUIDE.md`'s description of the minimal `hjkl`/`i`/`a`/`o`/`O`/
/// `x`/`dd` command set. Replaces the arrow-driven browse mode this pane
/// used to have: the cursor now has both a line *and* a column, and the
/// terminal's real cursor renders directly in the pane (see
/// `draw::draw`'s tail) instead of mirroring the current line in the
/// bottom prompt. Nothing touches the session at all in `Normal` mode —
/// only `x`/`dd` (immediate), splitting a line in `Insert` (`Enter`), and
/// committing out of `Insert` (`Esc`) ever call into `Session::set_line`,
/// `insert_line`, or `delete_line`; each edit is recorded as one undoable
/// action, following the same per-commit pattern as `formula_browse_apply`.
struct VimState {
    /// The proof line currently under the cursor — a display line number
    /// (1-based, proof-panel numbering). Can sit on a preamble line while
    /// merely navigating; every editing action refuses one, the same
    /// "nothing to edit there" `:edit`/`:delete` already give.
    line: usize,
    /// 0-based character column within `line`'s text. In `Normal` mode,
    /// clamped to the last character (or 0, for an empty line) — Vim's
    /// own convention, the cursor sits *on* a character, never past the
    /// end, the way `Insert` mode's `a`/`o`/`O` momentarily need to.
    col: usize,
    mode: VimSubMode,
    /// `true` right after a lone `d` in `Normal` mode, awaiting a second
    /// `d` to actually delete the line. Any other key clears this first,
    /// so `d` followed by anything but `d` is just a no-op on the stray
    /// `d` (see `App::vim_clear_pending`).
    pending_d: bool,
    /// The assertion text `:deassert` is replacing, set only by
    /// `start_vim_deassert` and cleared the moment `Insert` is left
    /// (`vim_commit_insert`/`vim_exit`) — mirrors the plain frontend's
    /// `EditState::deassert_source`, driving the same "(deasserting ...)"
    /// pane headings and Database-pane highlighting (`draw::draw`) for as
    /// long as the replacement is still being typed.
    deassert_source: Option<String>,
}

/// TUI-only interactive formula browsing, entered by `:formula`/`:formula
/// n`/`:formula n-m` — the Formula-pane counterpart to the Proof pane's
/// Vim-style editor (`VimState` above), sharing its "a range only picks
/// where the cursor starts, nothing is queued" shape (free-roam arrows own
/// navigation from there). Every commit is immediate
/// (`commands::formula::commit`), so there's no in-progress "pulled out
/// for editing" state to restore either; typing and abandoning with Esc is
/// still completely free, since nothing is submitted until Enter.
struct FormulaBrowse {
    /// The formula constraint currently under the cursor (1-based,
    /// matching the Formula pane's own numbering — no preamble offset the
    /// way Proof's display lines have).
    cursor: usize,
}

pub struct App {
    pub session: Option<Session>,
    pub scrollback: Scrollback,
    pub editor: input::LineEditor,
    /// How many lines the scrollback view is scrolled up from the live
    /// tail; 0 = following new output. Clamped against the actual
    /// scrollback length at draw time.
    pub scroll_up: usize,
    /// Which pane PgUp/PgDn scrolls; Shift+Tab cycles it. Locked to the
    /// pane being edited for the duration of Vim mode (`Proof`, see
    /// `VimState`) or a formula browse (`Formula`, see `FormulaBrowse`).
    pub focus: Pane,
    /// All three top-pane offsets are anchored at the top of their
    /// content: lines scrolled down from line one, 0 = starting there.
    /// Newly-appended content (a `:source`'d file, a fresh proof line, a
    /// new formula constraint) never moves these — only Output's own
    /// `scroll_up` is tail-anchored (0 = following the newest output).
    /// All clamped at draw time.
    pub formula_scroll: usize,
    pub database_scroll: usize,
    pub proof_scroll: usize,
    /// Horizontal offsets for the top panes: how many content columns are
    /// scrolled off to the left (line numbering stays pinned). Clamped at
    /// draw time against the longest visible line.
    pub formula_hscroll: usize,
    pub database_hscroll: usize,
    pub proof_hscroll: usize,
    /// The Output pane's own horizontal offset — same idea as the top
    /// panes' (see above), but scrollback lines carry no pinned line
    /// numbering to protect, so nothing stays fixed while scrolled.
    /// Clamped at draw time against the longest line anywhere in the
    /// scrollback, not just what's currently vertically visible — the
    /// same "whole content, not just the window" scope the top panes use.
    pub output_hscroll: usize,
    /// The geometry of the last frame drawn — how mouse coordinates and
    /// PgUp/PgDn page sizes map onto panes between draws.
    pub last_layout: Option<layout::Layout>,
    /// The Proof pane's actual content rows in the last frame drawn —
    /// `layout.top_h` minus one if a horizontal scrollbar row is eating
    /// the bottom row that frame. Written by `draw.rs` (which is the only
    /// place that already knows whether that scrollbar is present) so
    /// mapping a click or the browse cursor onto a screen row doesn't
    /// have to re-derive that decision.
    pub proof_content_h: usize,
    /// The Formula pane's actual content rows in the last frame drawn —
    /// same reasoning as `proof_content_h`, for mapping a click or the
    /// formula-browse cursor onto a screen row.
    pub formula_content_h: usize,
    /// The Database pane's actual content rows in the last frame drawn —
    /// same reasoning as `proof_content_h`, for `snap_database_to_row` to
    /// map a row index onto a scroll offset.
    pub database_content_h: usize,
    /// Position and time of the last left-click, for double-click
    /// detection — a second click on the same cell within
    /// `DOUBLE_CLICK_WINDOW` opens `:edit` on whatever Proof line is
    /// there, or formula-browse on whatever Formula line is there.
    last_click: Option<(u16, u16, Instant)>,
    /// `Some` while the Proof pane's Vim-style editor is active — entered
    /// by `:edit`/`:deassert`/`:insert` or double-clicking a Proof line
    /// (see `VimState`'s own docs). The prompt row shows a `-- NORMAL --`/
    /// `-- INSERT --` status instead of the ordinary prompt while this is
    /// `Some`, and the real terminal cursor renders in the Proof pane
    /// instead of at the bottom (see `draw::draw`).
    vim: Option<VimState>,
    /// `Some` while `:formula`'s interactive browse mode is active — the
    /// Formula-pane equivalent of `vim`: arrows move the cursor over
    /// formula constraints instead of proof lines, and Enter commits
    /// straight through `commands::formula::commit` (see
    /// `formula_browse_apply`) rather than `Session::set_line`.
    formula_browse: Option<FormulaBrowse>,
    /// `Some` while `:debug` mode is active. Unlike `vim`/`formula_browse`,
    /// this ISN'T a custom TUI-only UI: `:debug` is never intercepted in
    /// `execute` the way `:edit`/`:formula` are, so a line typed at the
    /// bottom prompt while this is `Some` is handed straight to
    /// `commands::debug::handle`, exactly like the plain frontend's own
    /// `debug_state` — see `App::execute`. The prompt row still gets a
    /// `debug> ` label of its own (`draw.rs`), and the event loop adds a
    /// couple of TUI-only shortcuts on top (Esc, Shift+Down/Shift+Up —
    /// see the `debug.is_some()` arms in `run`'s key match), but the
    /// actual stepping/breakpoint logic is the identical code the plain
    /// frontend runs.
    debug: Option<debug::DebugState>,
    /// Completion candidates for the current buffer, kept fresh by
    /// `refresh_candidates` after every keystroke — shared between the
    /// event loop (arrows, Tab, Enter) and the drawn strip.
    pub candidates: Vec<complete::Candidate>,
    /// The buffer text `candidates` was computed for.
    candidates_for: String,
    /// Index into `candidates` of the highlighted suggestion, if the user
    /// has started browsing with ↑/↓. Cleared whenever the buffer changes.
    pub selected: Option<usize>,
    /// The active color palette — `:theme <name>` switches it (see
    /// `execute`); `draw.rs` reads it fresh every frame, so a switch takes
    /// effect on the very next redraw.
    pub theme: theme::ThemeName,
    /// `Some((pane, level))` while a top pane is zoomed — see `Zoom`.
    /// `Layout::compute` reads this fresh every frame, same as `theme`.
    pub zoom: Option<(Pane, Zoom)>,
    /// The last mouse position any `Event::Mouse` reported, updated
    /// unconditionally regardless of that event's `kind` — a click needs
    /// it no less than a hover does. Purely an input, `bp_hover` (below)
    /// is what actually gets read for drawing; keeping the two separate
    /// means `bp_hover` can be recomputed fresh every frame (see
    /// `recompute_bp_hover`) from whatever this last was, rather than
    /// only updating on the mouse events that happened to fire.
    mouse_pos: Option<(u16, u16)>,
    /// The buffer index (if any) the mouse currently sits over in the
    /// Proof pane's breakpoint-marker column — see `proof_marker_at`.
    /// `draw::proof_lines` reads this to preview an unset breakpoint
    /// dimmed at that one line, the mouse-driven counterpart to Vim
    /// mode's `b` key. Recomputed every frame by `recompute_bp_hover`,
    /// not written directly from event handling — see that method's
    /// own docs for why.
    bp_hover: Option<usize>,
}

impl App {
    fn new(formula_path: Option<&str>) -> anyhow::Result<Self> {
        let mut scrollback = Scrollback::new();
        let session = commands::startup(formula_path, &mut scrollback)?;
        Ok(App {
            session,
            scrollback,
            editor: input::LineEditor::new(),
            scroll_up: 0,
            focus: Pane::Output,
            formula_scroll: 0,
            database_scroll: 0,
            proof_scroll: 0,
            formula_hscroll: 0,
            database_hscroll: 0,
            proof_hscroll: 0,
            output_hscroll: 0,
            last_layout: None,
            proof_content_h: 0,
            formula_content_h: 0,
            database_content_h: 0,
            last_click: None,
            vim: None,
            formula_browse: None,
            debug: None,
            candidates: Vec::new(),
            candidates_for: String::new(),
            selected: None,
            theme: theme::ThemeName::default(),
            mouse_pos: None,
            bp_hover: None,
            zoom: None,
        })
    }

    /// A header zoom-button click: toggle `pane` to `level` if it isn't
    /// already there, or back to normal (unzoomed) if it is — the same
    /// "does this button's state already match?" rule as the reverse-video
    /// highlighting `draw::title_segment` shows on it. Also focuses
    /// `pane`, since maximizing something other than what you're looking
    /// at doesn't make sense.
    fn toggle_zoom(&mut self, pane: Pane, level: Zoom) {
        self.zoom = if self.zoom == Some((pane, level)) {
            None
        } else {
            Some((pane, level))
        };
        self.focus = pane;
    }

    /// If a *different* pane is currently zoomed — another top pane, or
    /// `Output` — retarget the zoom to `pane` (keeping the same level)
    /// instead of leaving it hidden, squeezed, or (an `Output`
    /// full-maximise) off-screen entirely behind whatever else was
    /// maximized — called when `:edit`/`:formula` browse mode starts,
    /// since editing a pane you can't see, or barely can, isn't useful. A
    /// no-op when nothing's zoomed, or `pane` already is. `level` carries
    /// over unchanged and stays meaningful regardless which pane it's now
    /// paired with — both `Wide` and `Full` mean "more of this, less of
    /// everything else," for `Output` and a top pane alike, just to
    /// different degrees and in different directions (see `tui::Zoom`).
    fn retarget_zoom(&mut self, pane: Pane) {
        if let Some((zoomed, level)) = self.zoom
            && zoomed != pane
        {
            self.zoom = Some((pane, level));
        }
    }

    /// The Vim-mode cursor's display line number, for `draw.rs` to
    /// highlight in the Proof pane — `None` when not active.
    pub fn vim_cursor_line(&self) -> Option<usize> {
        self.vim.as_ref().map(|v| v.line)
    }

    /// The Vim-mode cursor's exact (line, column), for `draw.rs` to
    /// position the real terminal cursor and to keep that column
    /// horizontally in view — `None` when not active.
    ///
    /// Which half of the state owns the column depends on the sub-mode,
    /// and this is the one place that difference is resolved. In `Normal`,
    /// `VimState::col` is authoritative — the movement keys are the only
    /// thing that writes it. In `Insert`, every keystroke goes to
    /// `App::editor` instead (the same `input::LineEditor` the ordinary
    /// prompt uses, borrowed by `vim_enter_insert` purely for its
    /// character-level editing), so the editor's own cursor is the live
    /// insertion point while `VimState::col` stays frozen at whatever
    /// column `Insert` was entered at, until `vim_commit_insert` reads the
    /// editor back into it on the way out. Reporting `col` unconditionally
    /// would therefore leave the drawn cursor parked at that entry column
    /// for the whole `Insert` session, even as typing visibly moved the
    /// text under it — so read the live column from the editor instead.
    /// The rendered line comes from the same editor while inserting (see
    /// `draw::draw`'s `vim_live_text`), so the two always agree about what
    /// the column is indexing into.
    pub fn vim_cursor(&self) -> Option<(usize, usize)> {
        let vim = self.vim.as_ref()?;
        let col = match vim.mode {
            VimSubMode::Normal => vim.col,
            VimSubMode::Insert => self.editor.cursor(),
        };
        Some((vim.line, col))
    }

    /// Whether Vim mode is active and specifically in `Insert` — `draw.rs`
    /// picks the `-- INSERT --` status over `-- NORMAL --` for this.
    pub fn vim_inserting(&self) -> bool {
        matches!(
            self.vim,
            Some(VimState {
                mode: VimSubMode::Insert,
                ..
            })
        )
    }

    /// Whether Vim mode is active at all — `draw.rs` uses this to show
    /// the `-- NORMAL --`/`-- INSERT --` prompt-row status instead of the
    /// ordinary prompt.
    pub fn vim_active(&self) -> bool {
        self.vim.is_some()
    }

    /// The assertion text `:deassert` is currently replacing, for as long
    /// as its `Insert` session lasts — mirrors the plain frontend's
    /// `EditState::deassert_source`, driving the same pane headings and
    /// Database-pane highlighting in `draw::draw`.
    pub fn vim_deassert_source(&self) -> Option<&str> {
        self.vim.as_ref()?.deassert_source.as_deref()
    }

    /// Whether Vim mode is active and specifically in `Normal` — the main
    /// key-match in `run()` uses this to claim `hjkl`/`i`/`a`/`o`/`O`/`x`/
    /// `d`/`:`/Esc outright, since `Normal` has no free-typing fallback.
    fn vim_normal(&self) -> bool {
        matches!(
            self.vim,
            Some(VimState {
                mode: VimSubMode::Normal,
                ..
            })
        )
    }

    /// The formula-browse cursor's constraint number, for `draw.rs` to
    /// highlight in the Formula pane — `None` when not formula-browsing.
    pub fn formula_browse_cursor(&self) -> Option<usize> {
        self.formula_browse.as_ref().map(|b| b.cursor)
    }

    /// Whether formula-browse specifically is active — `draw.rs` uses this
    /// to choose the `opb> ` prompt label over `edit> `/`pbp> `.
    pub fn formula_editing(&self) -> bool {
        self.formula_browse.is_some()
    }

    /// Whether `:debug` mode is active — `draw.rs` uses this to choose the
    /// `debug> ` prompt label, and the event loop uses it to route bare
    /// Enter/Backspace and Esc to stepping/leaving instead of their
    /// ordinary meanings.
    pub fn debug_active(&self) -> bool {
        self.debug.is_some()
    }

    /// The Proof-buffer index `:debug` mode is currently stopped at, for
    /// `draw.rs` (`proof_lines`) to highlight — the same boundary
    /// `checked_len` already tracks: the next unchecked line, or
    /// wherever a rejection left `known_bad`, since neither case is ever
    /// attempted past (see `commands::debug`'s own docs). Once the whole
    /// buffer is checked there's nothing left at `checked_len` to point
    /// at, so this falls back to the last real line instead — still
    /// somewhere sensible to highlight, rather than `None` just because
    /// stepping ran out of runway. `None` whenever `:debug` isn't active
    /// or the buffer is empty (nothing to point at either way).
    fn debug_current_line(&self) -> Option<usize> {
        if !self.debug_active() {
            return None;
        }
        let session = self.session.as_ref()?;
        if session.buffer.is_empty() {
            return None;
        }
        Some(session.checked_len.min(session.buffer.len() - 1))
    }

    /// Snap the Proof pane to wherever `:debug` mode is currently
    /// stopped (see `debug_current_line`) — called after every command
    /// that can move `checked_len` while stepping, and once on entry, so
    /// the line being stepped through is always front and center rather
    /// than wherever the view already happened to be scrolled. A no-op
    /// (via `debug_current_line`'s own `None`) once debug mode has
    /// ended, so leaving it never yanks the view around on its way out.
    fn snap_proof_to_debug_line(&mut self) {
        let Some(idx) = self.debug_current_line() else {
            return;
        };
        let display_line = self
            .session
            .as_ref()
            .expect("debug_current_line already confirmed a session")
            .display_line(idx);
        self.snap_proof_to_line(display_line);
    }

    /// The constraint ID(s) `:debug` mode's most recently checked line
    /// produced, for `draw.rs` (`database_lines`) to highlight — see
    /// `Session::last_step_constraint_ids`. Empty whenever `:debug` isn't
    /// active, mirroring `debug_current_line`'s own gating: ordinary
    /// `:verify`/typing at the plain prompt never highlights the Database
    /// pane.
    fn debug_current_constraint_ids(&self) -> &[usize] {
        if !self.debug_active() {
            return &[];
        }
        self.session
            .as_ref()
            .map_or(&[], |s| s.last_step_constraint_ids.as_slice())
    }

    /// The database-pane equivalent of `snap_proof_to_line`: recenters
    /// `database_scroll` so the entry at 0-based row `row` (a position in
    /// `session.database()`'s own ascending-id order, matching
    /// `database_lines`'s iteration) lands about a third of the way down
    /// the pane's content height. A no-op before the first draw (no known
    /// height yet), same as `snap_proof_to_line`.
    fn snap_database_to_row(&mut self, row: usize) {
        let h = self.database_content_h;
        let Some(session) = &self.session else {
            return;
        };
        let Ok(total) = session.database().map(|d| d.entries.len()) else {
            return;
        };
        if h == 0 || total <= h {
            self.database_scroll = 0;
            return;
        }
        let max_offset = total - h;
        let row_idx = row.min(total - 1);
        self.database_scroll = row_idx.saturating_sub(h / 3).min(max_offset);
    }

    /// Snap the Database pane to wherever `:debug` mode's most recently
    /// checked line landed — see `debug_current_constraint_ids`. A no-op
    /// if debug mode isn't active, the last step produced no new
    /// constraint (e.g. a `del` line), or nothing checked yet at all.
    /// Scrolls to the highest id when a line produced more than one — the
    /// newest entry.
    fn snap_database_to_debug_step(&mut self) {
        let Some(&target) = self.debug_current_constraint_ids().iter().max() else {
            return;
        };
        let Some(session) = &self.session else {
            return;
        };
        let Ok(row) = session
            .database()
            .map(|d| d.entries.iter().position(|e| e.id == target))
        else {
            return;
        };
        let Some(row) = row else {
            return;
        };
        self.snap_database_to_row(row);
    }

    /// Esc while `:debug` mode is active: leave it. Mirrors `:done` (see
    /// `commands::debug::handle`) without going through the ordinary
    /// submit path, since Esc never types a line here either.
    fn debug_escape(&mut self) {
        self.debug = None;
        self.scrollback.push("Left debug mode.");
    }

    /// Shift+Down while `:debug` mode is active: step forward one line.
    /// A bare Enter used to do this, but that made Enter on an empty
    /// `debug>` prompt behave differently from every other empty prompt
    /// in the app (which is always a no-op) — Shift+Down is a dedicated
    /// shortcut instead, purely a TUI convenience with no bare-line
    /// meaning of its own in `commands::debug::handle` (see that
    /// module's doc comment), implemented by feeding `:step` through the
    /// normal `execute` path as if it had been typed and submitted.
    fn debug_step_forward(&mut self) {
        self.execute(":step");
    }

    /// Shift+Up while `:debug` mode is active: step back one line — the
    /// mirror image of `debug_step_forward`, `:back` typed the same way.
    fn debug_step_backward(&mut self) {
        self.execute(":back");
    }

    /// PgUp/PgDn: scroll the focused pane by one page (its height minus
    /// an overlap row).
    fn scroll_page(&mut self, up: bool) {
        let Some(layout) = self.last_layout else {
            return;
        };
        let page = match self.focus {
            Pane::Output => layout.scrollback_h as usize,
            _ => layout.top_h as usize,
        }
        .saturating_sub(1)
        .max(1);
        self.scroll_lines(self.focus, up, page);
    }

    /// Scroll `pane` by `amount` lines. `up` always means "toward earlier
    /// content": that shrinks the offset of the top-anchored panes
    /// (Formula/Database/Proof — see `draw::window_scrolled`) and grows
    /// the distance-from-tail of Output's own tail-anchored scrollback.
    /// Offsets are clamped against actual content at draw time.
    fn scroll_lines(&mut self, pane: Pane, up: bool, amount: usize) {
        let toward_larger = match pane {
            Pane::Output => up,
            _ => !up,
        };
        let offset = match pane {
            Pane::Formula => &mut self.formula_scroll,
            Pane::Database => &mut self.database_scroll,
            Pane::Proof => &mut self.proof_scroll,
            Pane::Output => &mut self.scroll_up,
        };
        *offset = if toward_larger {
            offset.saturating_add(amount)
        } else {
            offset.saturating_sub(amount)
        };
    }

    /// ←/→ with a top pane focused: shift its content horizontally.
    fn hscroll(&mut self, right: bool) {
        self.hscroll_pane(self.focus, right, HSCROLL_STEP);
    }

    /// Shift `pane`'s content horizontally by `amount` columns — Output
    /// included, via mouse only: ←/→ never reach here for it, since
    /// Output being focused means those keys belong to the prompt (see
    /// the key-handling match in `run`), and there's no way to bring up
    /// the suggestion strip's arrows for this instead. The mouse wheel
    /// (plain or Shift/Alt-modified) works uniformly across all four
    /// panes regardless, so it's still reachable.
    fn hscroll_pane(&mut self, pane: Pane, right: bool, amount: usize) {
        let offset = match pane {
            Pane::Formula => &mut self.formula_hscroll,
            Pane::Database => &mut self.database_hscroll,
            Pane::Proof => &mut self.proof_hscroll,
            Pane::Output => &mut self.output_hscroll,
        };
        *offset = if right {
            offset.saturating_add(amount)
        } else {
            offset.saturating_sub(amount)
        };
    }

    /// Which pane a mouse event at (`column`, `row`) lands in, per the
    /// last frame drawn.
    fn pane_at(&self, column: u16, row: u16) -> Option<Pane> {
        self.last_layout
            .and_then(|layout| layout.pane_at(column, row))
    }

    /// Recompute candidates after every key event. A buffer change
    /// invalidates any highlight (it indexed the old list); recomputation
    /// itself is unconditional, because the *mode* can change with the
    /// text unchanged (e.g. an edit that ends a history walk without
    /// altering the line). While the editor is walking history, there are
    /// no candidates at all: the strip stays hidden and ↑/↓ stay on
    /// history until the recalled line is edited.
    fn refresh_candidates(&mut self) {
        let text = self.editor.text();
        if text != self.candidates_for {
            self.selected = None;
            self.candidates_for = text.clone();
        }
        // Which `:`-vocabulary is actually live right now — the normal
        // registry (`None`), `:formula` browse's own narrower one, or
        // `:debug`'s (its mode-exclusive commands plus the shared
        // read-only set every edit-family mode allows through —
        // `edit::READONLY_DURING_EDIT` — chained on here rather than
        // duplicated into `debug::MODE_VOCABULARY` itself, so that
        // allowlist stays the one place deciding which read-only
        // commands work mid-mode), so the strip can never offer
        // something that doesn't work or omit something that does. Vim
        // mode needs no special case here: `Normal` never feeds
        // `self.editor` at all (so there's nothing to complete), and
        // `Insert` is typing an ordinary proof-rule line, wanting
        // exactly the same completion the ordinary prompt gives.
        let mode_vocabulary: Option<Vec<&help::Topic>> = if self.formula_browse.is_some() {
            Some(formula::MODE_VOCABULARY.iter().collect())
        } else if self.debug_active() {
            Some(
                debug::MODE_VOCABULARY
                    .iter()
                    .chain(
                        help::COMMANDS
                            .iter()
                            .filter(|t| edit::READONLY_DURING_EDIT.contains(&t.name)),
                    )
                    .collect(),
            )
        } else {
            None
        };
        self.candidates = if self.editor.browsing_history() {
            Vec::new()
        } else {
            complete::suggestions(&text, self.session.as_ref(), mode_vocabulary.as_deref())
        };
        if let Some(s) = self.selected
            && s >= self.candidates.len()
        {
            self.selected = None;
        }
        // No explicit choice yet (fresh candidates, or none ever made) —
        // offer `complete::default_selection`'s pick, if it has one, so a
        // bare Tab can land on it immediately (see that function's docs).
        // An actual arrow press always overrides this on the next event,
        // same as it would override any other highlight.
        if self.selected.is_none() {
            self.selected = complete::default_selection(&self.candidates);
        }
    }

    fn strip_visible(&self) -> bool {
        !self.candidates.is_empty()
    }

    /// ↓ moves the highlight down (starting at the top), wrapping.
    fn select_next(&mut self) {
        let len = self.candidates.len();
        if len == 0 {
            return;
        }
        self.selected = Some(match self.selected {
            None => 0,
            Some(i) => (i + 1) % len,
        });
    }

    /// ↑ moves the highlight up (starting at the bottom), wrapping.
    fn select_prev(&mut self) {
        let len = self.candidates.len();
        if len == 0 {
            return;
        }
        self.selected = Some(match self.selected {
            None => len - 1,
            Some(i) => (i + len - 1) % len,
        });
    }

    /// Insert the highlighted candidate into the buffer (never submits).
    /// Returns whether there was one to insert.
    fn accept_selected(&mut self) -> bool {
        match self.selected {
            Some(i) if i < self.candidates.len() => {
                let line = self.candidates[i].line.clone();
                self.editor.set_text(&line);
                true
            }
            _ => false,
        }
    }

    /// Tab: insert the highlighted candidate if there is one; otherwise
    /// complete outright on a unique candidate, or to the longest common
    /// prefix. Insert-only: nothing is ever submitted, and a completed
    /// token keeps matching its own candidate, so the strip stays up as
    /// the cue for what comes next (a directory completes to `dir/`, so
    /// the next Tab drills inside).
    fn complete(&mut self) {
        // Tab on a recalled-but-untouched line ends the history walk and
        // completes it in the same press — the recompute is needed
        // because candidates are suppressed during the walk.
        if self.editor.browsing_history() {
            self.editor.end_history_walk();
            self.refresh_candidates();
        }
        if self.accept_selected() {
            return;
        }
        match self.candidates.len() {
            0 => {}
            1 => {
                let line = self.candidates[0].line.clone();
                self.editor.set_text(&line);
            }
            _ => {
                let lcp = complete::common_line_prefix(&self.candidates).to_string();
                if !lcp.is_empty() {
                    self.editor.set_text(&lcp);
                }
            }
        }
    }

    // ---- Proof pane Vim-style editing -------------------------------

    /// Shared entry point behind `:edit`/`:deassert`/`:insert` and
    /// double-clicking a Proof line: focus the pane, retarget any active
    /// zoom to it, and enter `Normal` mode with the cursor on `line`,
    /// column 0. Callers that want to land straight in `Insert` (double-
    /// click/`:deassert` on an assertion, `:insert`) call
    /// `vim_enter_insert` immediately afterward.
    fn start_vim(&mut self, line: usize) {
        self.focus = Pane::Proof;
        self.retarget_zoom(Pane::Proof);
        self.vim = Some(VimState {
            line,
            col: 0,
            mode: VimSubMode::Normal,
            pending_d: false,
            deassert_source: None,
        });
        self.scroll_proof_to_cursor(line);
    }

    /// `:edit`/`:edit n`/`:edit n-m`, intercepted by `execute` before
    /// `commands::dispatch` ever sees them: enter Vim `Normal` mode,
    /// cursor on the named line (or the last buffer line, with no
    /// argument — a range's end is unused, same as before: it only ever
    /// picked where browsing began, never queued anything).
    fn start_vim_edit(&mut self, args: &str) {
        if self.formula_browse.is_some() {
            // Shouldn't happen — `:formula` is refused while formula-
            // browsing (see `execute`) — but refuse outright rather than
            // relying on that alone, the same defensive stance the browse
            // mode this replaced always took.
            return;
        }
        let Some(session) = &self.session else {
            self.scrollback
                .push("No formula loaded — use :load <formula.opb> first.");
            return;
        };
        if session.buffer.is_empty() {
            self.scrollback.push("Error: no proof lines yet to edit.");
            return;
        }
        let max_line = 2 + session.buffer.len();
        let start_line = if args.trim().is_empty() {
            max_line
        } else {
            match edit::parse_start_line(args.trim()) {
                Ok(n) => n,
                Err(msg) => {
                    self.scrollback.push(&format!("Error: {msg}"));
                    return;
                }
            }
        };
        if start_line < 3 || start_line > max_line {
            self.scrollback.push(&format!(
                "Error: line {start_line} is the synthesized preamble or past the end of the \
                 proof."
            ));
            return;
        }

        self.start_vim(start_line);
        self.scrollback.push(
            "Editing — hjkl/arrows to move, i/a/o/O to insert, x to delete a char, dd a line, \
             Esc to leave.",
        );
    }

    /// Enter, pressed on an empty prompt while the Proof pane has focus:
    /// the keyboard's way into Vim mode, so editing never requires either
    /// the mouse (double-clicking a line) or a typed `:edit`. Focus is
    /// already a first-class idea here — Shift+Tab cycles it and the
    /// focused pane's title shows in reverse video — so "focus the proof,
    /// press Enter" reads as the keyboard counterpart of clicking into it.
    ///
    /// Starts on the topmost line the pane is currently showing rather
    /// than on the last line the way bare `:edit` does. The two differ
    /// deliberately: `:edit` is typed blind at the prompt, where "the line
    /// I just added" is the obvious target, whereas this is pressed while
    /// looking at the pane, having very possibly scrolled somewhere
    /// specific first — jumping to the end of a long proof would be
    /// exactly wrong there. `:edit <n>` remains the way to name a line
    /// outright.
    fn start_vim_focused_pane(&mut self) {
        let Some(session) = &self.session else {
            self.scrollback
                .push("No formula loaded — use :load <formula.opb> first.");
            return;
        };
        if session.buffer.is_empty() {
            self.scrollback.push("Error: no proof lines yet to edit.");
            return;
        }
        let max_line = 2 + session.buffer.len();
        // `proof_scroll` counts content rows scrolled off the top and the
        // pane's first content row is display line 1, so the topmost
        // visible line is `proof_scroll + 1` — clamped past the
        // synthesized preamble to the first line actually worth editing.
        let start_line = (self.proof_scroll + 1).clamp(3, max_line);
        self.start_vim(start_line);
        self.scrollback.push(
            "Editing — hjkl/arrows to move, i/a/o/O to insert, x to delete a char, dd a line, \
             Esc to leave.",
        );
    }

    /// `:deassert [<n>]`, intercepted the same way `:edit` is: find the
    /// assertion exactly as the plain frontend's `edit::start_deassert`
    /// does (reusing its validation and `find_first_assertion`), then land
    /// straight in `Insert` at column 0 — replacing it is the whole
    /// point, so this skips the extra `i` keystroke, matching the plain
    /// frontend's own "ready to type immediately" deassert prompt. Prints
    /// the same "Constraints mentioning ..." hint list too.
    fn start_vim_deassert(&mut self, args: &str) {
        if self.formula_browse.is_some() {
            return;
        }
        let Some(session) = &self.session else {
            self.scrollback
                .push("No formula loaded — use :load <formula.opb> first.");
            return;
        };
        let line = if args.trim().is_empty() {
            match edit::find_first_assertion(session) {
                Some(line) => line,
                None => {
                    self.scrollback
                        .push("No `a` (unchecked assertion) rules left in the proof.");
                    return;
                }
            }
        } else {
            let Ok(n) = args.trim().parse::<usize>() else {
                self.scrollback.push("Error: usage :deassert [<n>]");
                return;
            };
            let Some(idx) = session.buffer_index(n) else {
                self.scrollback.push(&format!(
                    "Error: line {n} is the synthesized preamble or past the end of the proof \
                     — nothing to deassert there."
                ));
                return;
            };
            if !edit::is_assertion_line(&session.buffer[idx]) {
                self.scrollback.push(&format!(
                    "Error: line {n} isn't an `a`-rule (unchecked assertion) — nothing to \
                     deassert there. Try :edit {n} if you meant to retype it anyway."
                ));
                return;
            }
            n
        };

        let assertion_text = session.buffer[session.buffer_index(line).expect("just found")]
            .clone();
        self.start_vim(line);
        if let Some(vim) = &mut self.vim {
            vim.deassert_source = Some(edit::assertion_constraint_text(&assertion_text));
        }
        self.scrollback.push(&format!(
            "De-asserting line {line} — replace the `a` rule with one or more real derivation \
             steps, then Esc once you're satisfied."
        ));
        let session = self.session.as_ref().expect("just checked above");
        edit::suggest_related_constraints(session, &assertion_text, &mut self.scrollback);
        self.vim_enter_insert(false);
    }

    /// `:insert <n>`, intercepted the same way: splice a new, unchecked
    /// blank line in before display line `n` (or at the very end, if `n`
    /// names the line one past the last one — same convention
    /// `Vec::insert` uses for its index) and land in `Insert` on it —
    /// the direct equivalent of Vim's `O` at that position.
    fn start_vim_insert(&mut self, args: &str) {
        if self.formula_browse.is_some() {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            self.scrollback
                .push("No formula loaded — use :load <formula.opb> first.");
            return;
        };
        let Ok(n) = args.trim().parse::<usize>() else {
            self.scrollback.push("Error: usage :insert <n>");
            return;
        };
        let end_of_proof = session.display_line(session.buffer.len());
        let insert_idx = if n == end_of_proof {
            session.buffer.len()
        } else {
            match session.buffer_index(n) {
                Some(idx) => idx,
                None => {
                    self.scrollback.push(&format!(
                        "Error: line {n} is the synthesized preamble or past the end of the \
                         proof — nowhere to insert there. Use :insert {end_of_proof} to add \
                         lines at the end, or just type them normally."
                    ));
                    return;
                }
            }
        };
        // Its own independently-undoable action, same as every other Vim-
        // mode commit here — see `vim_commit_insert`'s identical pattern.
        let undo_before = (session.generation, session.snapshot());
        if let Err(err) = session.insert_line(insert_idx, "") {
            self.scrollback.push(&format!("Error: {err:#}"));
            return;
        }
        if session.generation != undo_before.0 {
            session.push_undo(undo_before.1);
        }
        self.start_vim(n);
        self.scrollback
            .push("Inserting — type the new line, then Esc once you're satisfied.");
        self.vim_enter_insert(false);
    }

    /// `:formula`/`:formula n`/`:formula n-m`, intercepted by `execute`
    /// before `commands::dispatch` ever sees them: enter formula-browse,
    /// cursor on the named constraint (or the last one, with no argument —
    /// mirrors `:edit`'s "last accepted line" default). A range only picks
    /// the starting constraint, exactly like `:edit n-m` in the TUI — free
    /// -roam arrows own navigation from there, not a queue (see
    /// `FormulaBrowse`'s own docs).
    fn start_formula_browse(&mut self, args: &str) {
        if self.vim.is_some() {
            // Mirrors `start_vim`'s own guard, the other direction.
            return;
        }
        let Some(session) = &self.session else {
            self.scrollback
                .push("No formula loaded — use :load <formula.opb> first.");
            return;
        };
        if session.formula.is_empty() {
            self.scrollback
                .push("Error: the formula has no constraints to edit.");
            return;
        }
        let max_line = session.formula.len();
        let start_line = if args.trim().is_empty() {
            max_line
        } else {
            match edit::parse_start_line(args.trim()) {
                Ok(n) => n,
                Err(msg) => {
                    self.scrollback.push(&format!("Error: {msg}"));
                    return;
                }
            }
        };
        if start_line < 1 || start_line > max_line {
            self.scrollback.push(&format!(
                "Error: constraint {start_line} is out of range — the formula has {max_line} \
                 constraint(s) (1-{max_line})."
            ));
            return;
        }

        self.focus = Pane::Formula;
        self.retarget_zoom(Pane::Formula);
        self.formula_browse = Some(FormulaBrowse { cursor: start_line });
        self.scrollback.push(
            "Editing the formula — ↑/↓ to browse, type to edit a constraint, Enter to apply, \
             Esc to cancel/leave, :done when finished.",
        );
        self.refill_formula_browse_prompt();
        self.scroll_formula_to_cursor(start_line);
    }

    /// `:theme [<name>]` — switch the active color palette. Intercepted
    /// here, ahead of `commands::dispatch`, since theming is purely a TUI
    /// concern: the plain frontend has no color to switch in the first
    /// place, so `:theme` there just says so instead of doing anything
    /// (see `commands::theme::run`). With no argument, reports the
    /// current theme and the available ones rather than erroring.
    fn start_theme(&mut self, args: &str) {
        let names = || {
            theme::ThemeName::ALL
                .iter()
                .map(|t| t.name())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let arg = args.trim();
        if arg.is_empty() {
            self.scrollback.push(&format!(
                "Current theme: {} (available: {}).",
                self.theme.name(),
                names()
            ));
            return;
        }
        match theme::ThemeName::parse(arg) {
            Some(name) => {
                self.theme = name;
                self.scrollback
                    .push(&format!("Theme set to {}.", name.name()));
            }
            None => {
                self.scrollback.push(&format!(
                    "Error: no theme named '{arg}' — available: {}.",
                    names()
                ));
            }
        }
    }

    /// The current Vim-mode line's text, or "" if that's somehow not
    /// resolvable (shouldn't happen while `vim` is `Some`, but this stays
    /// total rather than panicking).
    fn vim_line_text(&self) -> String {
        let (Some(vim), Some(session)) = (&self.vim, &self.session) else {
            return String::new();
        };
        session
            .buffer_index(vim.line)
            .and_then(|idx| session.buffer.get(idx))
            .cloned()
            .unwrap_or_default()
    }

    /// `Normal` mode's own column convention: on the last real character
    /// of a `line_len`-character line, or 0 for an empty one — never one
    /// past the end, the way `Insert`'s `a`/`o`/`O` momentarily allow.
    fn vim_clamp_col(col: usize, line_len: usize) -> usize {
        col.min(line_len.saturating_sub(1))
    }

    /// Re-clamp the Vim cursor's column against `line`'s current text
    /// length — shared by `vim_move_up`/`vim_move_down` (moving to a
    /// line of different length) and `vim_settle_cursor` (a delete can
    /// do the same).
    fn vim_settle_col(&mut self, line: usize) {
        let len = match &self.session {
            Some(session) => session
                .buffer_index(line)
                .and_then(|idx| session.buffer.get(idx))
                .map_or(0, |text| text.chars().count()),
            None => 0,
        };
        if let Some(vim) = &mut self.vim {
            vim.col = Self::vim_clamp_col(vim.col, len);
        }
    }

    /// `h`/←: move left, clamped to column 0.
    fn vim_move_left(&mut self) {
        if let Some(vim) = &mut self.vim {
            vim.col = vim.col.saturating_sub(1);
        }
    }

    /// `l`/→: move right, clamped to `Normal` mode's own convention
    /// (never past the last character).
    fn vim_move_right(&mut self) {
        let len = self.vim_line_text().chars().count();
        if let Some(vim) = &mut self.vim {
            vim.col = Self::vim_clamp_col(vim.col + 1, len);
        }
    }

    /// Ctrl/Alt+←: jump to the start of the word at or before the
    /// cursor, in `Normal` mode's own column convention. The same
    /// skip-whitespace-then-skip-word logic as
    /// `input::LineEditor::prev_word_start`, ported here because
    /// `Normal` has no `LineEditor` to delegate to — it indexes
    /// straight into `vim_line_text()` instead. Kept in lock-step with
    /// that method; if the definition of "word" ever changes there,
    /// change it here too.
    fn vim_word_left(&mut self) {
        let text: Vec<char> = self.vim_line_text().chars().collect();
        let Some(vim) = &mut self.vim else {
            return;
        };
        let mut i = vim.col.min(text.len());
        while i > 0 && text[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !text[i - 1].is_whitespace() {
            i -= 1;
        }
        vim.col = i;
    }

    /// Ctrl/Alt+→: jump to the start of the next word after the cursor,
    /// re-clamped to `Normal` mode's own convention (never past the
    /// last character) since "start of the next word" can otherwise
    /// land one past the end of the line. Mirrors
    /// `input::LineEditor::next_word_start`; see `vim_word_left`.
    fn vim_word_right(&mut self) {
        let text: Vec<char> = self.vim_line_text().chars().collect();
        let len = text.len();
        let start = match &self.vim {
            Some(vim) => vim.col.min(len),
            None => return,
        };
        let mut i = start;
        while i < len && !text[i].is_whitespace() {
            i += 1;
        }
        while i < len && text[i].is_whitespace() {
            i += 1;
        }
        if let Some(vim) = &mut self.vim {
            vim.col = Self::vim_clamp_col(i, len);
        }
    }

    /// `k`/↑: move up one line, clamped to the first real buffer line;
    /// column re-clamped to the new line's length (no sticky-column
    /// tracking — out of scope).
    fn vim_move_up(&mut self) {
        let Some(vim) = &mut self.vim else {
            return;
        };
        vim.line = vim.line.saturating_sub(1).max(3);
        let line = vim.line;
        self.vim_settle_col(line);
        self.scroll_proof_to_cursor(line);
    }

    /// `j`/↓: move down one line, clamped to the last buffer line; column
    /// re-clamped the same way `vim_move_up` does.
    fn vim_move_down(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let max_line = 2 + session.buffer.len();
        let Some(vim) = &mut self.vim else {
            return;
        };
        vim.line = (vim.line + 1).min(max_line);
        let line = vim.line;
        self.vim_settle_col(line);
        self.scroll_proof_to_cursor(line);
    }

    /// `i`/`a` in `Normal` mode: enter `Insert` at the cursor column
    /// (`i`) or one past it (`a`, clamped to the line's length) —
    /// pre-fills `self.editor` with the line's current text and
    /// positions its cursor to match. The *rendered* cursor is the real
    /// terminal cursor in the pane (see `draw::draw`), not this buffer —
    /// `self.editor` is reused here purely for its already-working
    /// character-level editing (Left/Right/Backspace/Delete/Home/End),
    /// same as the old browse mode relied on.
    fn vim_enter_insert(&mut self, after: bool) {
        let text = self.vim_line_text();
        let len = text.chars().count();
        let Some(vim) = &mut self.vim else {
            return;
        };
        if after {
            vim.col = (vim.col + 1).min(len);
        }
        vim.mode = VimSubMode::Insert;
        let col = vim.col;
        self.editor.set_text(&text);
        self.editor.set_cursor(col);
    }

    /// `o`/`O` in `Normal` mode: splice a new, unchecked blank line in
    /// after (`o`) or before (`O`) the cursor line and enter `Insert` on
    /// it at column 0 — `Session::insert_line`, its own independently-
    /// undoable action, same pattern as every other Vim-mode commit here.
    fn vim_open_line(&mut self, below: bool) {
        let Some(vim_line) = self.vim.as_ref().map(|v| v.line) else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(idx) = session.buffer_index(vim_line) else {
            self.scrollback.push(&format!(
                "Error: line {vim_line} is the synthesized preamble or past the end of the \
                 proof — nowhere to open a new line there."
            ));
            return;
        };
        let insert_idx = if below { idx + 1 } else { idx };
        let undo_before = (session.generation, session.snapshot());
        if let Err(err) = session.insert_line(insert_idx, "") {
            self.scrollback.push(&format!("Error: {err:#}"));
            return;
        }
        if session.generation != undo_before.0 {
            session.push_undo(undo_before.1);
        }
        let new_line = session.display_line(insert_idx);
        if let Some(vim) = &mut self.vim {
            vim.line = new_line;
            vim.col = 0;
        }
        self.scroll_proof_to_cursor(new_line);
        self.vim_enter_insert(false);
    }

    /// `x` in `Normal` mode: delete the character under the cursor, if
    /// the line is non-empty and actually editable (refuses a preamble
    /// line, same message `:edit`/`:delete` already give).
    fn vim_delete_char(&mut self) {
        let Some(vim) = &self.vim else {
            return;
        };
        let (line, col) = (vim.line, vim.col);
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(idx) = session.buffer_index(line) else {
            self.scrollback.push(&format!(
                "Error: line {line} is the synthesized preamble or past the end of the proof \
                 — nothing to edit there."
            ));
            return;
        };
        let mut chars: Vec<char> = session.buffer[idx].chars().collect();
        if col >= chars.len() {
            return;
        }
        chars.remove(col);
        let new_text: String = chars.into_iter().collect();
        let undo_before = (session.generation, session.snapshot());
        if let Err(err) = session.set_line(idx, &new_text) {
            self.scrollback.push(&format!("Error: {err:#}"));
            return;
        }
        if session.generation != undo_before.0 {
            session.push_undo(undo_before.1);
        }
        let new_len = new_text.chars().count();
        if let Some(vim) = &mut self.vim {
            vim.col = Self::vim_clamp_col(col, new_len);
        }
    }

    /// `b` in `Normal` mode: toggle a breakpoint on the cursor's line —
    /// set it if absent, clear it if present (`Session::toggle_breakpoint`
    /// does the actual work; this is just the keyboard route to it, ahead
    /// of the Proof pane's own clickable marker). Refuses a preamble
    /// line, same "nothing to break there" as `x`/`dd` give for editing
    /// one — nothing on it is ever checked, so a breakpoint there could
    /// never be hit. Doesn't touch `undo_stack`: a breakpoint isn't proof
    /// state (see `Session::breakpoints`'s own docs), so there's nothing
    /// here for `:undo` to need to know about.
    fn vim_toggle_breakpoint(&mut self) {
        let Some(vim) = &self.vim else {
            return;
        };
        let line = vim.line;
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(idx) = session.buffer_index(line) else {
            self.scrollback.push(&format!(
                "Error: line {line} is the synthesized preamble or past the end of the proof                  — nothing to break there."
            ));
            return;
        };
        session.toggle_breakpoint(idx);
    }

    /// `d` in `Normal` mode: the first `d` of a `dd` sequence sets
    /// `pending_d`; the second one deletes the line.
    fn vim_d_key(&mut self) {
        let Some(vim) = &mut self.vim else {
            return;
        };
        if vim.pending_d {
            vim.pending_d = false;
            self.vim_delete_line();
        } else {
            vim.pending_d = true;
        }
    }

    /// Any `Normal`-mode key that isn't itself a recognized command:
    /// clears a pending `d`, if any (so `d` followed by anything but `d`
    /// is just a no-op on the stray `d`) — `Normal` mode has no
    /// free-typing fallback.
    fn vim_clear_pending(&mut self) {
        if let Some(vim) = &mut self.vim {
            vim.pending_d = false;
        }
    }

    /// The second `d` of `dd`: delete the cursor line outright —
    /// `Session::delete_line`, refusing a preamble line the same way
    /// every other editing action here does. Re-clamps the cursor
    /// afterward (`vim_settle_cursor`) since the buffer just got shorter.
    fn vim_delete_line(&mut self) {
        let Some(line) = self.vim.as_ref().map(|v| v.line) else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(idx) = session.buffer_index(line) else {
            self.scrollback.push(&format!(
                "Error: line {line} is the synthesized preamble or past the end of the proof \
                 — nothing to delete there."
            ));
            return;
        };
        let undo_before = (session.generation, session.snapshot());
        if let Err(err) = session.delete_line(idx) {
            self.scrollback.push(&format!("Error: {err:#}"));
            return;
        }
        if session.generation != undo_before.0 {
            session.push_undo(undo_before.1);
        }
        self.vim_settle_cursor();
    }

    /// Clamp the Vim cursor back into range after a mutation that can
    /// shrink the buffer (`dd`), keep it in view, and re-clamp the
    /// column against whatever's now at that line. If deleting left the
    /// buffer empty (only reachable by deleting the one remaining line),
    /// there's nothing left to edit at all, so this leaves Vim mode
    /// entirely rather than stranding the cursor on a line that no
    /// longer exists.
    fn vim_settle_cursor(&mut self) {
        let Some(session) = &self.session else {
            self.vim_exit();
            return;
        };
        if session.buffer.is_empty() {
            self.scrollback
                .push("Left the editor — no proof lines left to edit.");
            self.vim = None;
            self.focus = Pane::Output;
            return;
        }
        let max_line = 2 + session.buffer.len();
        if let Some(vim) = &mut self.vim {
            vim.line = vim.line.min(max_line).max(3);
        }
        let line = self.vim.as_ref().map(|v| v.line);
        if let Some(line) = line {
            self.vim_settle_col(line);
            self.scroll_proof_to_cursor(line);
        }
    }

    /// Esc in `Normal` mode: leave Vim mode entirely, focus back to
    /// Output.
    fn vim_exit(&mut self) {
        self.vim = None;
        self.scrollback.push("Left the editor.");
        self.focus = Pane::Output;
    }

    /// `:` in `Normal` mode: leave Vim mode and hand focus to the
    /// ordinary prompt with `:` already typed — the practical way to run
    /// `:show`/`:list`/`:verify`/etc. without a special mid-mode
    /// allowlist, since every commit here is already atomic (no queue
    /// state a stray command could corrupt, unlike the plain frontend's
    /// `EditState`).
    fn vim_to_prompt(&mut self) {
        self.vim = None;
        self.focus = Pane::Output;
        self.editor.set_text(":");
    }

    /// Enter in `Insert` mode: split the current line at the editor cursor,
    /// insert the remainder as the next proof line, and stay in `Insert`
    /// mode at its start. The two buffer mutations form one undoable
    /// action, so the split reverses atomically.
    fn vim_insert_newline(&mut self) {
        let Some(vim) = &self.vim else {
            return;
        };
        let line = vim.line;
        let text = self.editor.text();
        let split_at = self.editor.cursor().min(text.chars().count());
        let chars: Vec<char> = text.chars().collect();
        let before: String = chars[..split_at].iter().collect();
        let after: String = chars[split_at..].iter().collect();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(idx) = session.buffer_index(line) else {
            return;
        };

        let undo_before = (session.generation, session.snapshot());
        let result = session
            .set_line(idx, &before)
            .and_then(|()| session.insert_line(idx + 1, &after));
        if session.generation != undo_before.0 {
            session.push_undo(undo_before.1);
        }
        if let Err(err) = result {
            self.scrollback.push(&format!("Error: {err:#}"));
            return;
        }

        let next_line = session.display_line(idx + 1);
        if let Some(vim) = &mut self.vim {
            vim.line = next_line;
            vim.col = 0;
        }
        self.editor.set_text(&after);
        self.editor.set_cursor(0);
        self.scroll_proof_to_cursor(next_line);
    }

    /// Move the Insert cursor to another proof line, committing the
    /// current line only when its text changed. The destination column is
    /// clamped to that line's length, matching a text editor's vertical
    /// cursor movement without marking untouched checked lines unchecked.
    fn vim_insert_move_to(&mut self, line: usize, col: usize) {
        let Some(current_line) = self.vim.as_ref().map(|vim| vim.line) else {
            return;
        };
        if line == current_line {
            return;
        }
        let text = self.editor.text();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(current_idx) = session.buffer_index(current_line) else {
            return;
        };
        let Some(target_idx) = session.buffer_index(line) else {
            return;
        };

        if session.buffer[current_idx] != text {
            let undo_before = (session.generation, session.snapshot());
            if let Err(err) = session.set_line(current_idx, &text) {
                self.scrollback.push(&format!("Error: {err:#}"));
                return;
            }
            if session.generation != undo_before.0 {
                session.push_undo(undo_before.1);
            }
        }

        let target_text = session.buffer[target_idx].clone();
        let target_col = col.min(target_text.chars().count());
        if let Some(vim) = &mut self.vim {
            vim.line = line;
            vim.col = target_col;
        }
        self.editor.set_text(&target_text);
        self.editor.set_cursor(target_col);
        self.scroll_proof_to_cursor(line);
    }

    /// Up/Down in Insert mode move through editable proof lines while
    /// preserving the current column where possible.
    fn vim_insert_move_vertical(&mut self, up: bool) {
        let Some((line, col)) = self.vim_cursor() else {
            return;
        };
        let max_line = self
            .session
            .as_ref()
            .map_or(3, |session| 2 + session.buffer.len());
        let target = if up {
            line.saturating_sub(1).max(3)
        } else {
            (line + 1).min(max_line)
        };
        self.vim_insert_move_to(target, col);
    }

    /// Left/Right cross into the neighboring proof line at the current
    /// line's beginning/end. Within a line, `LineEditor` keeps its normal
    /// character-level behavior.
    fn vim_insert_move_horizontal(&mut self, right: bool) {
        let Some((line, col)) = self.vim_cursor() else {
            return;
        };
        let current_len = self.editor.text().chars().count();
        if right && col == current_len {
            let max_line = self
                .session
                .as_ref()
                .map_or(3, |session| 2 + session.buffer.len());
            if line < max_line {
                self.vim_insert_move_to(line + 1, 0);
            }
        } else if !right && col == 0 && line > 3 {
            let previous_text = self
                .session
                .as_ref()
                .and_then(|session| session.buffer_index(line - 1))
                .and_then(|idx| self.session.as_ref()?.buffer.get(idx))
                .cloned()
                .unwrap_or_default();
            self.vim_insert_move_to(line - 1, previous_text.chars().count());
        }
    }

    /// Esc in `Insert` mode: commit `self.editor`'s current text as the
    /// line's new content and return to `Normal` on the same line, cursor
    /// where typing left it — its own independently-undoable action, same
    /// pattern as every other Vim-mode commit here. Never rejected:
    /// nothing is checked, so this always succeeds (barring an internal
    /// replay bug, reported like any other).
    fn vim_commit_insert(&mut self) {
        let Some(vim) = &self.vim else {
            return;
        };
        let line = vim.line;
        let text = self.editor.text();
        let cursor_at_commit = self.editor.cursor();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(idx) = session.buffer_index(line) else {
            return;
        };
        if session.buffer[idx] != text {
            let undo_before = (session.generation, session.snapshot());
            if let Err(err) = session.set_line(idx, &text) {
                self.scrollback.push(&format!("Error: {err:#}"));
            } else if session.generation != undo_before.0 {
                session.push_undo(undo_before.1);
            }
        }
        self.editor.set_text("");
        let len = text.chars().count();
        if let Some(vim) = &mut self.vim {
            vim.mode = VimSubMode::Normal;
            vim.col = Self::vim_clamp_col(cursor_at_commit, len);
            vim.deassert_source = None;
        }
    }

    /// Adjust `proof_scroll` so `cursor` (a display line number) sits
    /// within the Proof pane's last-drawn content height, with
    /// `BROWSE_MARGIN` rows of context above/below where the content
    /// allows. Top-anchored, like the Formula pane's own
    /// `scroll_formula_to_cursor` (which this otherwise mirrors) — no
    /// tail-relative math needed. A no-op before the first draw (no known
    /// height yet) — the pane's default view (line one at the top) is a
    /// reasonable fallback until then.
    fn scroll_proof_to_cursor(&mut self, cursor: usize) {
        let h = self.proof_content_h;
        let Some(session) = &self.session else {
            return;
        };
        let total = 2 + session.buffer.len();
        if h == 0 || total <= h {
            self.proof_scroll = 0;
            return;
        }
        let max_offset = total - h;
        let cursor_idx = cursor.saturating_sub(1).min(total - 1);
        let margin = BROWSE_MARGIN.min((h.saturating_sub(1)) / 2);

        let low = self.proof_scroll + margin;
        let high = (self.proof_scroll + h).saturating_sub(1 + margin);
        if cursor_idx < low {
            self.proof_scroll = cursor_idx.saturating_sub(margin);
        } else if cursor_idx > high {
            self.proof_scroll = (cursor_idx + margin + 1).saturating_sub(h);
        }
        self.proof_scroll = self.proof_scroll.min(max_offset);
    }

    /// Snap the Proof pane so display line `line` lands about a third of
    /// the way down its content height — used right after `:verify`
    /// leaves a line `known_bad`, so the line it stopped on is front and
    /// center instead of wherever the view already happened to be
    /// scrolled. Unlike `scroll_proof_to_cursor` (which nudges the view
    /// the *minimum* amount needed to keep an actively-moving browse
    /// cursor in sight), this always recenters outright — `:verify` is a
    /// discrete, deliberate action, not a continuous one, so its result
    /// deserves a definite jump rather than a minimal nudge. A no-op
    /// before the first draw (no known height yet), same as
    /// `scroll_proof_to_cursor`.
    fn snap_proof_to_line(&mut self, line: usize) {
        let h = self.proof_content_h;
        let Some(session) = &self.session else {
            return;
        };
        let total = 2 + session.buffer.len();
        if h == 0 || total <= h {
            self.proof_scroll = 0;
            return;
        }
        let max_offset = total - h;
        let line_idx = line.saturating_sub(1).min(total - 1);
        self.proof_scroll = line_idx.saturating_sub(h / 3).min(max_offset);
    }

    /// The display line number of whichever Proof line is drawn at
    /// terminal `row` in the last frame, or `None` if `row` isn't one of
    /// its content rows (its border/title, the horizontal scrollbar row
    /// if present, or outside the pane) or lands on the (uneditable)
    /// preamble. Top-anchored, like `formula_line_at_row` — `proof_scroll`
    /// already *is* the top offset, no reverse math needed.
    fn proof_line_at_row(&self, row: u16) -> Option<usize> {
        let row = row as usize;
        let h = self.proof_content_h;
        if row < 1 || row > h {
            return None;
        }
        let session = self.session.as_ref()?;
        let total = 2 + session.buffer.len();
        let idx = self.proof_scroll + (row - 1);
        if idx >= total {
            return None;
        }
        let display_line = idx + 1;
        (display_line > 2).then_some(display_line)
    }

    /// The buffer index (if any) whose breakpoint-marker glyph sits
    /// exactly at terminal (`column`, `row`) in the last frame drawn —
    /// the mouse hit-test both hover preview (`recompute_bp_hover`) and a
    /// click (`run`'s `MouseEventKind::Down` handling) share, so the two
    /// can never disagree about where the marker actually is. `None`
    /// whenever (`column`, `row`) isn't in the Proof pane at all, isn't
    /// that pane's marker *column* specifically (`Layout::
    /// proof_content_x0` — one fixed column regardless of horizontal
    /// scroll, see `proof_prefix_width`'s own docs), or lands on the
    /// preamble (`proof_line_at_row` already excludes it — a marker
    /// there would name no real buffer line, and `Session::buffer_index`
    /// would refuse it anyway).
    fn proof_marker_at(&self, column: u16, row: u16) -> Option<usize> {
        if self.pane_at(column, row) != Some(Pane::Proof) {
            return None;
        }
        if column != self.last_layout?.proof_content_x0() {
            return None;
        }
        let display_line = self.proof_line_at_row(row)?;
        self.session.as_ref()?.buffer_index(display_line)
    }

    /// The buffer index (if any) `draw::proof_lines` should preview a
    /// dimmed breakpoint marker on this frame — `None` when nothing's
    /// hovered, a breakpoint's already set there (nothing to preview,
    /// `breakpoint_marker` shows the active color regardless), or the
    /// mouse has never moved at all yet.
    pub fn bp_hover(&self) -> Option<usize> {
        self.bp_hover
    }

    /// Refresh `bp_hover` from `mouse_pos` — called once at the top of
    /// every `draw::draw`, not written directly from mouse-event
    /// handling, deliberately: recomputing it fresh every frame (against
    /// whichever pane geometry that frame draws with) is what keeps it
    /// accurate after something *other* than the mouse changes what's
    /// actually under a stationary pointer — scrolling the Proof pane
    /// with the keyboard, resizing the terminal, entering/leaving a zoom
    /// — none of which fire a `MouseEventKind` of their own to hang an
    /// update off of. A purely event-driven update would go stale in
    /// exactly those cases; this can't.
    pub(crate) fn recompute_bp_hover(&mut self) {
        self.bp_hover = self
            .mouse_pos
            .and_then(|(column, row)| self.proof_marker_at(column, row));
    }

    /// Whether display line `n` is currently an `a`-rule (unchecked
    /// assertion) — used by `double_click_proof` to decide whether a
    /// double-click there should mean "deassert this" rather than "edit
    /// this". `false` for anything unresolvable (no session, out-of-range
    /// line, preamble) rather than erroring — callers treat that the same
    /// as "not an assertion".
    fn line_is_assertion(&self, n: usize) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        let Some(idx) = session.buffer_index(n) else {
            return false;
        };
        session
            .buffer
            .get(idx)
            .is_some_and(|text| edit::is_assertion_line(text))
    }

    /// Double-click on a Proof-pane row: jump straight into Vim `Normal`
    /// mode on that line, or, if it's an `a`-rule, straight into
    /// `:deassert` on it instead — or, if already in Vim `Normal` mode,
    /// just move the cursor there — the same place a single click landed
    /// doesn't do anything by itself, so there's no conflict with plain
    /// clicking. A no-op while formula-browse is active, or while Vim
    /// mode is already in `Insert` (moving the cursor mid-insert would
    /// discard nothing, but it's not what a double-click while typing
    /// should mean).
    fn double_click_proof(&mut self, column: u16, row: u16) {
        if self.formula_browse.is_some() || self.pane_at(column, row) != Some(Pane::Proof) {
            return;
        }
        let Some(line) = self.proof_line_at_row(row) else {
            return;
        };
        if let Some(vim) = &mut self.vim {
            if vim.mode == VimSubMode::Normal {
                vim.line = line;
                vim.col = 0;
                self.scroll_proof_to_cursor(line);
            }
        } else if self.line_is_assertion(line) {
            self.execute(&format!(":deassert {line}"));
        } else {
            self.start_vim(line);
        }
    }

    /// The formula-pane equivalent of `scroll_proof_to_cursor`: adjusts
    /// `formula_scroll` so `cursor` (a 1-based constraint number) sits
    /// within the pane's last-drawn content height, with `BROWSE_MARGIN`
    /// rows of context above/below where the content allows.
    fn scroll_formula_to_cursor(&mut self, cursor: usize) {
        let h = self.formula_content_h;
        let Some(session) = &self.session else {
            return;
        };
        let total = session.formula.len();
        if h == 0 || total <= h {
            self.formula_scroll = 0;
            return;
        }
        let max_offset = total - h;
        let cursor_idx = cursor.saturating_sub(1).min(total - 1);
        let margin = BROWSE_MARGIN.min(h.saturating_sub(1) / 2);

        let low = self.formula_scroll + margin;
        let high = (self.formula_scroll + h).saturating_sub(1 + margin);
        if cursor_idx < low {
            self.formula_scroll = cursor_idx.saturating_sub(margin);
        } else if cursor_idx > high {
            self.formula_scroll = (cursor_idx + margin + 1).saturating_sub(h);
        }
        self.formula_scroll = self.formula_scroll.min(max_offset);
    }

    /// The constraint number of whichever Formula line is drawn at
    /// terminal `row` in the last frame, or `None` if `row` isn't one of
    /// its content rows or lands past the end of the formula. Mirrors
    /// `proof_line_at_row`, simplified for the top-anchored scroll.
    fn formula_line_at_row(&self, row: u16) -> Option<usize> {
        let row = row as usize;
        let h = self.formula_content_h;
        if row < 1 || row > h {
            return None;
        }
        let session = self.session.as_ref()?;
        let idx = self.formula_scroll + (row - 1);
        if idx >= session.formula.len() {
            return None;
        }
        Some(idx + 1)
    }

    /// The formula constraint's text currently under the browse cursor, or
    /// "" if that's somehow not resolvable (shouldn't happen while
    /// `formula_browse` is `Some`, but this stays total rather than
    /// panicking).
    fn formula_browse_cursor_text(&self) -> String {
        let (Some(browse), Some(session)) = (&self.formula_browse, &self.session) else {
            return String::new();
        };
        // `session.formula` entries carry a trailing `;`; stripped here —
        // `replace_formula_constraint` re-adds one on commit regardless.
        session
            .formula_index(browse.cursor)
            .and_then(|idx| session.formula.get(idx))
            .map(|c| c.trim_end().strip_suffix(';').unwrap_or(c).trim_end().to_string())
            .unwrap_or_default()
    }

    /// Whether the prompt buffer has diverged from the cursor constraint's
    /// current text, i.e. a replacement is part-typed and the arrows must
    /// stop moving the cursor out from under it.
    fn formula_browse_locked(&self) -> bool {
        self.formula_browse.is_some() && self.editor.text() != self.formula_browse_cursor_text()
    }

    fn refill_formula_browse_prompt(&mut self) {
        let text = self.formula_browse_cursor_text();
        self.editor.set_text(&text);
    }

    /// ↑/↓ while formula-browsing and unlocked: move the cursor (clamped
    /// to `1..=formula.len()`) and re-fill the prompt. Unlike the Proof
    /// pane's own cursor, there's no synthesized-preamble floor to clear
    /// — constraint 1 is a real, editable constraint.
    fn formula_browse_move(&mut self, up: bool) {
        let Some(session) = &self.session else {
            return;
        };
        let max_line = session.formula.len();
        let Some(browse) = &mut self.formula_browse else {
            return;
        };
        browse.cursor = if up {
            browse.cursor.saturating_sub(1).max(1)
        } else {
            (browse.cursor + 1).min(max_line)
        };
        let cursor = browse.cursor;
        self.scroll_formula_to_cursor(cursor);
        self.refill_formula_browse_prompt();
    }

    /// Esc while formula-browsing: revert an in-progress edit if locked,
    /// or leave the mode entirely if not.
    fn formula_browse_escape(&mut self) {
        if self.formula_browse_locked() {
            self.refill_formula_browse_prompt();
        } else {
            self.exit_formula_browse("Left formula mode.");
        }
    }

    fn exit_formula_browse(&mut self, message: &str) {
        self.formula_browse = None;
        self.editor.set_text("");
        self.scrollback.push(message);
        self.focus = Pane::Output;
    }

    /// Enter while formula-browsing: `:done` leaves the mode outright.
    /// `:formula cancel` undoes the most recent commit and stays browsing
    /// — with no counterpart in the Proof pane's Vim mode, added so the
    /// TUI doesn't drift from the plain frontend's mode, which recognizes
    /// the same command mid-edit (see the module docs' "shared so the
    /// plain CLI and the TUI can't drift apart" principle). Anything else
    /// — including an empty buffer, deliberately *not* treated as delete,
    /// since a formula constraint can't be removed this way — commits
    /// straight through `commands::formula::commit` for the cursor's
    /// constraint; an empty or otherwise invalid buffer naturally reports
    /// as an ordinary parse-error rejection through that same path, with
    /// nothing touched, so no special-casing is needed here. Either way
    /// stays browsing on the same constraint, prompt refreshed with
    /// whatever's actually there once the dust settles
    /// (`settle_formula_browse_cursor`).
    fn formula_browse_apply(&mut self) {
        let Some(cursor) = self.formula_browse.as_ref().map(|b| b.cursor) else {
            return;
        };
        let text = self.editor.text();
        let trimmed = text.trim().to_string();
        if trimmed == ":done" {
            self.exit_formula_browse("Left formula mode.");
            return;
        }
        if trimmed.starts_with(':') && trimmed != ":formula cancel" {
            self.scrollback.push(&format!("opb> {trimmed}"));
            self.scrollback.push(formula::ONLY_DONE_OR_CANCEL);
            self.refill_formula_browse_prompt();
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };

        self.scroll_up = 0;
        self.database_scroll = 0;
        self.proof_scroll = 0;
        self.formula_hscroll = 0;
        self.database_hscroll = 0;
        self.proof_hscroll = 0;
        self.output_hscroll = 0;

        if trimmed == ":formula cancel" {
            // Corrective, not a new action — no undo-stack push, same
            // reasoning as excluding `:formula cancel` in
            // `commands::dispatch`.
            self.scrollback.push("opb> :formula cancel");
            if let Err(err) = formula::cancel(session, &mut self.scrollback) {
                self.scrollback.push(&format!("Error: {err:#}"));
            }
        } else {
            // Each commit here is its own independent, addressable
            // action (same reasoning as the Proof pane's Vim-mode
            // commits — see `VimState`) —
            // tracked individually rather than bundled into one
            // mode-wide entry.
            let undo_before = (session.generation, session.snapshot());
            self.scrollback.push(&format!("opb> {trimmed}"));
            if let Err(err) = formula::commit(session, cursor, &trimmed, &mut self.scrollback) {
                self.scrollback.push(&format!("Error: {err:#}"));
            }
            if session.generation != undo_before.0 {
                session.push_undo(undo_before.1);
            }
        }

        self.settle_formula_browse_cursor();
    }

    /// Re-scroll and refill the prompt with whatever's now at the cursor's
    /// constraint — the shared tail of every `formula_browse_apply`
    /// action. Unlike the Proof pane's `vim_settle_cursor`, there's no
    /// shrunk-buffer case to clamp against: a formula edit never changes
    /// the constraint count, so the cursor's index is always still
    /// valid.
    fn settle_formula_browse_cursor(&mut self) {
        let Some(cursor) = self.formula_browse.as_ref().map(|b| b.cursor) else {
            return;
        };
        self.scroll_formula_to_cursor(cursor);
        self.refill_formula_browse_prompt();
    }

    /// Double-click on a Formula-pane row: jump straight into
    /// formula-browse on that constraint, or, if already browsing, just
    /// move the cursor there. Mirrors `double_click_proof`; a no-op while
    /// Vim mode is active, for the same reason.
    fn double_click_formula(&mut self, column: u16, row: u16) {
        if self.vim.is_some() || self.pane_at(column, row) != Some(Pane::Formula) {
            return;
        }
        let Some(line) = self.formula_line_at_row(row) else {
            return;
        };
        if self.formula_browse.is_some() {
            if let Some(browse) = &mut self.formula_browse {
                browse.cursor = line;
            }
            self.scroll_formula_to_cursor(line);
            self.refill_formula_browse_prompt();
        } else {
            self.start_formula_browse(&line.to_string());
        }
    }

    // ---- normal (non-browse) dispatch ------------------------------

    /// Echo and run one submitted line; returns `false` when the app
    /// should quit. Dispatch errors are shown in the scrollback rather
    /// than tearing the TUI down — the session survives them.
    ///
    /// `:edit`/`:edit n`/`:edit n-m`, `:deassert`/`:deassert n`,
    /// `:insert <n>`, and `:formula`/`:formula n`/`:formula n-m` are all
    /// intercepted here, ahead of `commands::dispatch`, and become
    /// `start_vim_edit`/`start_vim_deassert`/`start_vim_insert`/
    /// `start_formula_browse` instead — the TUI's own interactive modes
    /// rather than the plain frontend's queue-based `EditState`.
    /// (`:formula cancel` is *not* intercepted — it has no browse-mode
    /// equivalent of its own to start, so it flows through to `dispatch`
    /// normally, same as any other command needing an existing session.)
    /// The interception re-resolves the command name through
    /// `commands::resolve_command` (rather than pattern-matching the raw
    /// line) so abbreviations keep working, and everything else still
    /// flows through the exact same shared `dispatch` plain mode uses.
    /// `:delete` needs no special handling at all: it's immediate and
    /// non-interactive in both frontends, so it just flows through
    /// `dispatch` normally.
    fn execute(&mut self, line: &str) -> bool {
        // `:debug` mode, once entered, is checked before anything else:
        // every line typed at the bottom prompt while it's active — a
        // `:step`, a `:show`, `:done`, anything — belongs to
        // `commands::debug::handle`, not the ordinary `resolve_command`/
        // `dispatch` path below (which is how mode *entry* still works —
        // see the `Ok(Flow::StartDebug(state))` arm further down). This
        // mirrors `plain.rs`'s own `debug_state` routing exactly, which is
        // the point: unlike `:edit`/`:formula`, `:debug` gets no
        // TUI-specific UI here, just a couple of extra keyboard shortcuts
        // layered on top in `run`'s event loop.
        if let Some(state) = self.debug.as_mut() {
            self.scrollback.push(&format!("debug> {line}"));
            let session = self
                .session
                .as_mut()
                .expect("debug mode implies a loaded session");
            match debug::handle(session, state, line, &mut self.scrollback) {
                Ok(debug::DebugFlow::Continue) => {}
                Ok(debug::DebugFlow::Ended) => self.debug = None,
                Err(err) => self.scrollback.push(&format!("Error: {err:#}")),
            }
            self.snap_proof_to_debug_line();
            self.snap_database_to_debug_step();
            return true;
        }

        // What `line` resolves to, if it's a `:`-command — `Option<&'static
        // str>` (not the full `Result`) specifically so it's `Copy` and can
        // be checked more than once below without fighting the borrow
        // checker over a value that isn't.
        let (cmd, cmd_args) = match line.strip_prefix(':') {
            Some(rest) => {
                let mut parts = rest.splitn(2, char::is_whitespace);
                (
                    parts.next().unwrap_or(""),
                    parts.next().unwrap_or("").trim(),
                )
            }
            None => ("", ""),
        };
        let resolved: Option<&'static str> = (!cmd.is_empty())
            .then(|| commands::resolve_command(cmd).ok())
            .flatten();

        if resolved == Some("edit") {
            self.scrollback.push(&format!("pbp> {line}"));
            self.start_vim_edit(cmd_args);
            return true;
        }
        if resolved == Some("deassert") {
            self.scrollback.push(&format!("pbp> {line}"));
            self.start_vim_deassert(cmd_args);
            return true;
        }
        if resolved == Some("insert") {
            self.scrollback.push(&format!("pbp> {line}"));
            self.start_vim_insert(cmd_args);
            return true;
        }
        // `:formula cancel` has no browse-mode equivalent to start — it's
        // the same one-shot undo either way — so it's deliberately
        // excluded here and falls through to `dispatch` below, exactly
        // like it does in the plain frontend.
        if resolved == Some("formula") && cmd_args.trim() != "cancel" {
            self.scrollback.push(&format!("pbp> {line}"));
            self.start_formula_browse(cmd_args);
            return true;
        }
        if resolved == Some("theme") {
            self.scrollback.push(&format!("pbp> {line}"));
            self.start_theme(cmd_args);
            return true;
        }

        self.scrollback.push(&format!("pbp> {line}"));
        // `:source` (and `:instance`, which calls straight into it after
        // loading the formula) can process hundreds of lines in one call;
        // give an immediate visual acknowledgment that the (blocking —
        // this app is single-threaded, and `ForwardsChecker` holds `Rc`s,
        // not `Arc`s, ruling out a real background-thread spinner without
        // upstream changes) work has actually started, rather than
        // leaving the previous frame looking frozen for however long it
        // takes. One extra draw here, then the normal end-of-loop draw
        // shows the real result once `dispatch` returns. `:formula`'s own
        // full-buffer reverify doesn't need an entry here: mode entry
        // is intercepted above (nothing blocking happens until a commit),
        // and every commit itself goes through `formula_browse_apply`, not
        // this generic path — mirroring how a Vim-mode commit in the Proof
        // pane (`vim_commit_insert`) has no "Working…" indicator of its
        // own either. `:formula cancel` is the one `:formula` invocation that
        // does reach here, and it's neither slow nor filtered by this list.
        if matches!(resolved, Some("source") | Some("instance")) {
            self.scrollback.push("Working…");
            let mut stdout = io::stdout();
            let _ = draw::draw(&mut stdout, self);
        }
        // Snap every pane back to its default view — the command likely
        // changed what they show, making old offsets stale.
        self.scroll_up = 0;
        self.formula_scroll = 0;
        self.database_scroll = 0;
        self.proof_scroll = 0;
        self.formula_hscroll = 0;
        self.database_hscroll = 0;
        self.proof_hscroll = 0;
        self.output_hscroll = 0;
        match commands::dispatch(&mut self.session, line, &mut self.scrollback) {
            Ok(Flow::Quit) => false,
            Ok(Flow::Continue) => {
                // `:verify` stopping on a rejection is exactly when the
                // Proof pane's default "follow the tail" view (just reset
                // above) is least likely to already show the line that
                // matters — jump straight to it instead of leaving the
                // user to scroll and hunt for whatever `:list`/the error
                // message already named.
                if resolved == Some("verify")
                    && let Some(line) = self.session.as_ref().and_then(|s| {
                        s.known_bad
                            .is_some()
                            .then(|| s.display_line(s.checked_len))
                    })
                {
                    self.snap_proof_to_line(line);
                }
                true
            }
            Ok(Flow::StartEdit(_)) => {
                // Unreachable in practice: `:edit`/`:deassert`/`:insert`
                // are all intercepted above before dispatch ever runs —
                // see `start_vim_edit`/`start_vim_deassert`/
                // `start_vim_insert`. Handled defensively rather than left
                // to panic, mirroring `Flow::StartFormulaEdit`'s own "BUG:
                // ..." fallback below.
                self.scrollback
                    .push("BUG: edit mode reached dispatch unexpectedly.");
                true
            }
            Ok(Flow::StartFormulaEdit(_)) => {
                // Unreachable in practice: `:formula` (other than
                // `:formula cancel`, which never produces this variant) is
                // always intercepted above before dispatch ever runs — see
                // `start_formula_browse`. Handled defensively rather than
                // left to panic, mirroring `dispatch`'s own "BUG: ..."
                // fallback for a registered-but-unwired command.
                self.scrollback
                    .push("BUG: formula mode reached dispatch unexpectedly.");
                true
            }
            Ok(Flow::StartDebug(state)) => {
                // Genuinely reachable, unlike the two stubs above: `:debug`
                // isn't intercepted before `dispatch`, so this is the real
                // mode-entry path — see the guard at the very top of this
                // function for what happens to every line typed after.
                self.debug = Some(state);
                self.snap_proof_to_debug_line();
                self.snap_database_to_debug_step();
                true
            }
            Err(err) => {
                self.scrollback.push(&format!("Error: {err:#}"));
                true
            }
        }
    }
}

/// Puts the terminal into raw mode + the alternate screen, and restores
/// both on drop — including the drop that happens while unwinding, so an
/// error propagating out of the event loop can't leave the shell broken.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(
            io::stdout(),
            terminal::EnterAlternateScreen,
            event::EnableMouseCapture
        )?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            event::DisableMouseCapture,
            terminal::LeaveAlternateScreen,
            cursor::Show
        );
        let _ = terminal::disable_raw_mode();
    }
}

pub fn run(formula_path: Option<&str>) -> anyhow::Result<()> {
    // A panic inside the alternate screen with raw mode on would print its
    // message invisibly and leave the shell unusable — restore the
    // terminal first, then let the default hook report as normal.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            io::stdout(),
            event::DisableMouseCapture,
            terminal::LeaveAlternateScreen,
            cursor::Show
        );
        let _ = terminal::disable_raw_mode();
        default_hook(info);
    }));

    // Load the formula before touching the terminal, so a bad startup path
    // reports its error like any normal CLI failure.
    let mut app = App::new(formula_path)?;

    let _guard = TerminalGuard::enter()?;
    let mut stdout = io::stdout();

    loop {
        draw::draw(&mut stdout, &mut app)?;

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                match key.code {
                    // Vim `Insert` sub-mode: Esc commits and returns to
                    // `Normal`; Enter opens the next proof line; arrows
                    // navigate the proof buffer instead of prompt history.
                    KeyCode::Esc if app.vim_inserting() => app.vim_commit_insert(),
                    KeyCode::Enter if app.vim_inserting() => app.vim_insert_newline(),
                    KeyCode::Up if app.vim_inserting() => app.vim_insert_move_vertical(true),
                    KeyCode::Down if app.vim_inserting() => app.vim_insert_move_vertical(false),
                    KeyCode::Left
                        if app.vim_inserting() && app.editor.cursor() == 0 =>
                    {
                        app.vim_insert_move_horizontal(false);
                    }
                    KeyCode::Right
                        if app.vim_inserting()
                            && app.editor.cursor() == app.editor.text().chars().count() =>
                    {
                        app.vim_insert_move_horizontal(true);
                    }
                    // Vim `Normal` sub-mode owns every key outright while
                    // active — checked ahead of every other meaning these
                    // keys have, including each other's normal pane-
                    // scroll/history/suggestion-strip behavior. There's no
                    // free-typing fallback in `Normal`: anything not
                    // listed here just clears a pending `d` (if any) and
                    // does nothing else.
                    // Ctrl or Alt turns ←/→ into a word jump here too,
                    // same as the prompt/Insert editor (`input::
                    // LineEditor::handle_key`) and for the same reason:
                    // terminals disagree about which modifier they
                    // forward, so both are accepted. Checked ahead of the
                    // plain h/l/←/→ arm below, since that one doesn't
                    // look at modifiers at all and would otherwise catch
                    // these first.
                    KeyCode::Left
                        if app.vim_normal()
                            && key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        app.vim_clear_pending();
                        app.vim_word_left();
                    }
                    KeyCode::Right
                        if app.vim_normal()
                            && key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        app.vim_clear_pending();
                        app.vim_word_right();
                    }
                    KeyCode::Char('h') | KeyCode::Left if app.vim_normal() => {
                        app.vim_clear_pending();
                        app.vim_move_left();
                    }
                    KeyCode::Char('l') | KeyCode::Right if app.vim_normal() => {
                        app.vim_clear_pending();
                        app.vim_move_right();
                    }
                    KeyCode::Char('k') | KeyCode::Up if app.vim_normal() => {
                        app.vim_clear_pending();
                        app.vim_move_up();
                    }
                    KeyCode::Char('j') | KeyCode::Down if app.vim_normal() => {
                        app.vim_clear_pending();
                        app.vim_move_down();
                    }
                    KeyCode::Char('i') if app.vim_normal() => {
                        app.vim_clear_pending();
                        app.vim_enter_insert(false);
                    }
                    KeyCode::Char('a') if app.vim_normal() => {
                        app.vim_clear_pending();
                        app.vim_enter_insert(true);
                    }
                    KeyCode::Char('o') if app.vim_normal() => {
                        app.vim_clear_pending();
                        app.vim_open_line(true);
                    }
                    KeyCode::Char('O') if app.vim_normal() => {
                        app.vim_clear_pending();
                        app.vim_open_line(false);
                    }
                    KeyCode::Char('x') if app.vim_normal() => {
                        app.vim_clear_pending();
                        app.vim_delete_char();
                    }
                    KeyCode::Char('b') if app.vim_normal() => {
                        app.vim_clear_pending();
                        app.vim_toggle_breakpoint();
                    }
                    KeyCode::Char('d') if app.vim_normal() => app.vim_d_key(),
                    KeyCode::Char(':') if app.vim_normal() => app.vim_to_prompt(),
                    KeyCode::Esc if app.vim_normal() => app.vim_exit(),
                    _ if app.vim_normal() => app.vim_clear_pending(),
                    KeyCode::Up if app.formula_browse_locked() => {}
                    KeyCode::Down if app.formula_browse_locked() => {}
                    KeyCode::Up if app.formula_browse.is_some() => app.formula_browse_move(true),
                    KeyCode::Down if app.formula_browse.is_some() => app.formula_browse_move(false),
                    KeyCode::Esc if app.formula_browse.is_some() => app.formula_browse_escape(),
                    KeyCode::Enter if app.formula_browse.is_some() => app.formula_browse_apply(),
                    // `:debug` mode's own shortcuts — checked ahead of
                    // the ordinary Esc/Up/Down arms further down (zoom,
                    // pane scroll, the suggestion strip, history) the same
                    // way the two browse modes' arms just above are, so
                    // being in debug mode always wins over whatever those
                    // would otherwise do. Shift+Down/Shift+Up step
                    // forward/backward regardless of what's typed at the
                    // prompt (mirroring `formula_browse`'s own arrows just
                    // above, which likewise don't care about prompt
                    // text); bare Enter is deliberately NOT one of these
                    // shortcuts — an empty `debug>` prompt is a no-op, the
                    // same as every other empty prompt in the app, so it
                    // falls through to the ordinary editor/submit arms
                    // further down like any other Enter press.
                    KeyCode::Esc if app.debug_active() => app.debug_escape(),
                    KeyCode::Down
                        if app.debug_active() && key.modifiers.contains(KeyModifiers::SHIFT) =>
                    {
                        app.debug_step_forward();
                    }
                    KeyCode::Up
                        if app.debug_active() && key.modifiers.contains(KeyModifiers::SHIFT) =>
                    {
                        app.debug_step_backward();
                    }
                    KeyCode::Tab => app.complete(),
                    // With a top pane focused, ↑/↓ scroll it line by
                    // line. With Output focused (the default), they
                    // belong to the prompt: the suggestion strip while
                    // it's visible, history otherwise (↑ from an empty
                    // line, its most common use).
                    KeyCode::Up if app.focus != Pane::Output => {
                        app.scroll_lines(app.focus, true, 1);
                    }
                    KeyCode::Down if app.focus != Pane::Output => {
                        app.scroll_lines(app.focus, false, 1);
                    }
                    KeyCode::Up if app.strip_visible() => app.select_prev(),
                    KeyCode::Down if app.strip_visible() => app.select_next(),
                    // Same rule for ←/→: horizontal scroll in a focused
                    // top pane, prompt cursor movement otherwise — except
                    // while browsing, where the prompt is what you're
                    // editing, so ←/→ always belong to it (falls through
                    // to the editor below).
                    KeyCode::Left
                        if app.focus != Pane::Output
                            && app.vim.is_none()
                            && app.formula_browse.is_none() =>
                    {
                        app.hscroll(false);
                    }
                    KeyCode::Right
                        if app.focus != Pane::Output
                            && app.vim.is_none()
                            && app.formula_browse.is_none() =>
                    {
                        app.hscroll(true);
                    }
                    // Checked after both browse-mode Esc arms above, so
                    // leaving an active edit always takes priority over
                    // leaving zoom — one layer of "step back" at a time.
                    KeyCode::Esc if app.zoom.is_some() => app.zoom = None,
                    KeyCode::Esc => app.selected = None,
                    // Enter with a highlight inserts it — submitting takes
                    // a second Enter, once nothing is highlighted.
                    KeyCode::Enter if app.selected.is_some() => {
                        app.accept_selected();
                    }
                    // Enter on an empty prompt with the Proof pane focused
                    // opens Vim mode there — the keyboard route in, next to
                    // double-clicking a line and typing `:edit`. Costs
                    // nothing that was doing anything: a blank Enter is
                    // otherwise a no-op (see the editor fallback below), and
                    // with any text typed this arm doesn't apply, so the
                    // line still submits as usual. Both browse modes are
                    // already handled well above, so reaching here means
                    // neither is active.
                    KeyCode::Enter
                        if app.focus == Pane::Proof && app.editor.text().is_empty() =>
                    {
                        app.start_vim_focused_pane();
                    }
                    KeyCode::BackTab if app.vim.is_none() && app.formula_browse.is_none() => {
                        app.focus = app.focus.next()
                    }
                    KeyCode::PageUp => app.scroll_page(true),
                    KeyCode::PageDown => app.scroll_page(false),
                    _ => match app.editor.handle_key(key) {
                        input::EditResult::Submitted(line) => {
                            let line = line.trim();
                            // A blank Enter is a no-op — nothing was
                            // typed, nothing to do. (Vim mode's own Enter
                            // is intercepted well above this fallback, so
                            // it never reaches here.)
                            let should_execute = !line.is_empty();
                            if should_execute && !app.execute(line) {
                                break;
                            }
                        }
                        input::EditResult::Quit => break,
                        input::EditResult::None => {}
                    },
                }
                app.refresh_candidates();
            }
            Event::Mouse(mouse) => {
                // Every mouse event carries a position, click or not —
                // recorded unconditionally so `App::recompute_bp_hover`
                // (called once per frame, not once per event — see its own
                // docs) always has the latest one to work from, regardless
                // of which `kind` actually fires below.
                app.mouse_pos = Some((mouse.column, mouse.row));
                match mouse.kind {
                    // The wheel scrolls whichever pane it's hovering over,
                    // without moving focus; with Shift or Alt held it scrolls
                    // horizontally instead (wheel-up = left). Two modifiers
                    // because terminals disagree about which ones they forward
                    // rather than swallow.
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                        if let Some(pane) = app.pane_at(mouse.column, mouse.row) {
                            let up = mouse.kind == MouseEventKind::ScrollUp;
                            if mouse
                                .modifiers
                                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT)
                            {
                                app.hscroll_pane(pane, !up, WHEEL_HSTEP);
                            } else {
                                app.scroll_lines(pane, up, WHEEL_STEP);
                            }
                        }
                    }
                    // Native sideways scrolling (trackpad swipe, tilt wheel).
                    MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => {
                        if let Some(pane) = app.pane_at(mouse.column, mouse.row) {
                            let right = mouse.kind == MouseEventKind::ScrollRight;
                            app.hscroll_pane(pane, right, WHEEL_HSTEP);
                        }
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        // The breakpoint gutter takes priority over every
                        // other click meaning below: a hit here toggles and
                        // stops right there — never counted toward a
                        // double-click sequence (mirroring the header-button
                        // case, the other "this click meant something
                        // specific, not focus/edit" case), and never a
                        // reason to open Vim mode or formula-browse the way
                        // an ordinary Proof/Formula click can.
                        let marker_hit = app.proof_marker_at(mouse.column, mouse.row);
                        let header_hit = app
                            .last_layout
                            .and_then(|l| l.header_button_at(mouse.column, mouse.row));
                        if let Some(idx) = marker_hit {
                            if let Some(session) = app.session.as_mut() {
                                session.toggle_breakpoint(idx);
                            }
                            app.last_click = None;
                            if app.vim.is_none() && app.formula_browse.is_none() {
                                app.focus = Pane::Proof;
                            }
                        } else if let Some((pane, level)) = header_hit {
                            // A button click is never part of a double-click
                            // sequence — the content double-click actions
                            // (deassert/edit) only ever fire on row >= 1, but
                            // resetting this outright avoids a stray double
                            // registering against whatever's clicked next.
                            app.last_click = None;
                            app.toggle_zoom(pane, level);
                        } else {
                            let now = Instant::now();
                            let is_double = app.last_click.is_some_and(|(col, row, at)| {
                                col == mouse.column
                                    && row == mouse.row
                                    && now.duration_since(at) < DOUBLE_CLICK_WINDOW
                            });
                            app.last_click = Some((mouse.column, mouse.row, now));

                            if is_double {
                                app.double_click_proof(mouse.column, mouse.row);
                                app.double_click_formula(mouse.column, mouse.row);
                            } else if app.vim.is_none() && app.formula_browse.is_none() {
                                if let Some(pane) = app.pane_at(mouse.column, mouse.row) {
                                    app.focus = pane;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            // A resize is picked up by the next draw, which re-reads the
            // terminal size every frame anyway.
            _ => {}
        }
    }

    Ok(())
}
