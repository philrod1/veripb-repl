//! Rendering: turn the current `App` state into one full frame of text and
//! queue it to the terminal. No diffing — the frame is small, so every draw
//! rewrites every cell (which also makes resize handling trivial: each
//! frame re-reads the terminal size and covers it completely).
//!
//! Character counts are treated as display columns. That's exact for the
//! ASCII-ish constraint/rule text this REPL renders and the box-drawing
//! borders; if wide glyphs ever show up in variable names, this is the
//! assumption to revisit. Styling (the dimmed preamble lines and such) is
//! applied only after a line is fitted to its column, so the zero-width
//! escape codes never enter the width math.
//!
//! Scrollbars live *inside* their pane — a `┃` (or `━`) thumb on a dimmed
//! `│` (or `─`) track in the last column (vertical) or last row
//! (horizontal), appearing only when that pane's content overflows — so
//! there's never a question of which pane a bar belongs to, and the dim
//! track can't be confused with the bright pane border beside it. The
//! bars are also the only overflow signal: no `… (+N)` marker rows, no
//! ellipses; content is simply clipped at the pane edge and the bars say
//! where you are.
//!
//! Pedagogical highlighting is deliberately restrained: variable names get
//! one accent color, `@labels` another, and nothing else — no full syntax
//! highlighting of keywords/operators/numbers, which would compete with
//! reading the actual constraint math rather than help it. Formula and
//! database constraints are tokenized *exactly*, by exploiting the fact
//! that every `ToPrettyString` impl in `veripb-formula` (`Clause`,
//! `Cardinality`, `GeneralPBConstraint`) emits the same deterministic
//! `"<coeff> <lit> <coeff> <lit> ... >= <degree>"` layout — see
//! `tokenize_constraint`. Raw proof-rule text has no such fixed grammar
//! (rup/pol/red/... all differ), so it's colored by a safer method
//! instead: any whitespace token that resolves to a real variable name is
//! colored, everything else stays plain — a lookup can never mislabel a
//! keyword or a constraint-ID hint as a variable, unlike guessing from
//! position.
//!
//! The Proof pane's Vim-mode cursor gets its own signal on top of all
//! that: the line it sits on renders on a background color (token
//! foreground colors still apply on top of it), tying it visually to the
//! `-- NORMAL --`/`-- INSERT --` status, which uses the matching accent.
//! An `a`-rule (unchecked assertion) line gets its own background the
//! same way, flagging it as scaffolding still owed a real derivation —
//! see `is_assertion_line`. The Database pane's rows get a background
//! too, core or derived — `row_on_bg` is the one place that logic lives,
//! shared by all three.
//!
//! Every color used anywhere in this module, dimming included, comes from
//! `theme::Theme` — explicit RGB, not one of crossterm's named ANSI
//! colors, since those are indices into a palette the terminal (and its
//! user) defines for itself and render unpredictably from machine to
//! machine; see the module docs on `theme` for why that actually bit this
//! project three times before it was worth fixing. This extends to every
//! character in the frame, not just the accents/highlights above: plain
//! content goes through `base` (theme fg-on-bg), borders and scrollbar
//! chrome go through `chrome` (theme dim-on-bg), and both are threaded
//! down to wherever text actually gets assembled instead of leaving
//! "unstyled" text to fall back on the terminal's own default colors —
//! otherwise switching to `light` would only recolor the accents while
//! everything else stayed whatever the terminal's own background already
//! was.

use std::io::Write;
use std::ops::Range;

use crossterm::{cursor, queue, style::Color, style::Print, style::Stylize, terminal};
// use veripb_formula::prelude::*;

use crate::commands::edit;
use crate::session::Session;
use crate::tui::layout::{self, Layout};
use crate::tui::{App, Pane, Zoom, theme::Theme};

const NORMAL_PROMPT: &str = "pbp> ";
const FORMULA_PROMPT: &str = "opb> ";
const DEBUG_PROMPT: &str = "debug> ";

/// Fixed height of the suggestion strip whenever it's visible (the layout
/// may grant fewer rows on a small terminal). All-or-nothing so the
/// prompt only ever sits at two positions: strip hidden, or strip shown.
const SUGGESTION_ROWS: usize = 6;

/// What color, if any, a run of text in a panel line carries. Kept to
/// exactly two accents plus "none" — see the module docs for why.
#[derive(Clone, Copy, PartialEq)]
enum TokenStyle {
    Plain,
    Variable,
    Label,
}

/// One run of same-styled text within a panel line's content.
#[derive(Clone)]
struct Span {
    text: String,
    style: TokenStyle,
}

fn plain(text: String) -> Span {
    Span {
        text,
        style: TokenStyle::Plain,
    }
}

fn variable(text: String) -> Span {
    Span {
        text,
        style: TokenStyle::Variable,
    }
}

fn label(text: String) -> Span {
    Span {
        text,
        style: TokenStyle::Label,
    }
}

/// Plain content text on the theme's base background — the "nothing
/// special going on here" style. Everywhere text isn't part of an accent,
/// a row highlight, or border/scrollbar chrome (see `chrome`) goes
/// through this, which is what makes theming genuinely app-wide: `light`
/// renders light everywhere, not just in the bits this app chooses to
/// accent.
fn base(text: &str, theme: Theme) -> String {
    text.with(theme.fg).on(theme.bg).to_string()
}

/// Border lines, scrollbar tracks, and other structural "chrome" that
/// isn't content: `theme.dim` on the base background, so it recedes
/// relative to actual content without ever falling back to the
/// terminal's own (unpredictable) default colors.
fn chrome(text: &str, theme: Theme) -> String {
    text.with(theme.dim).on(theme.bg).to_string()
}

/// Apply a span's style, producing text ready to print (ANSI embedded, if
/// styled). Only ever called on a span already sliced to its final
/// visible width — never on text that might still get char-sliced
/// afterward, since slicing an ANSI-embedded string would corrupt it.
fn style_span(span: &Span, theme: Theme) -> String {
    match span.style {
        TokenStyle::Plain => base(&span.text, theme),
        TokenStyle::Variable => span
            .text
            .clone()
            .with(theme.variable)
            .on(theme.bg)
            .to_string(),
        TokenStyle::Label => span.text.clone().with(theme.label).on(theme.bg).to_string(),
    }
}

/// Same per-token foreground coloring as `style_span`, plus a solid
/// background — the two channels (foreground hue, background highlight)
/// don't compete, so labels/variables stay legible on top of it. Shared
/// by every row-level highlight (`:edit` browse cursor, an `a`-rule
/// assertion, a Database-pane row's core/derived status): they only ever
/// differ in which `theme` background they pass in.
fn style_span_on_bg(span: &Span, bg: Color, theme: Theme) -> String {
    match span.style {
        TokenStyle::Plain => span.text.clone().with(theme.fg).on(bg).to_string(),
        TokenStyle::Variable => span.text.clone().with(theme.variable).on(bg).to_string(),
        TokenStyle::Label => span.text.clone().with(theme.label).on(bg).to_string(),
    }
}

fn spans_char_len(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.text.chars().count()).sum()
}

/// Drop the first `n` characters across `spans` (splitting a span if the
/// boundary falls inside it), keeping each surviving run's style. Used
/// for horizontal scrolling — the character-level equivalent of
/// `window_scrolled`'s line-level skip.
fn spans_skip(spans: &[Span], n: usize) -> Vec<Span> {
    let mut remaining = n;
    let mut out = Vec::new();
    for span in spans {
        let len = span.text.chars().count();
        if remaining >= len {
            remaining -= len;
            continue;
        }
        out.push(Span {
            text: span.text.chars().skip(remaining).collect(),
            style: span.style,
        });
        remaining = 0;
    }
    out
}

/// Keep only the first `n` characters across `spans`, splitting the span
/// that straddles the boundary. The clipping counterpart to `spans_skip`.
fn spans_take(spans: &[Span], n: usize) -> Vec<Span> {
    let mut remaining = n;
    let mut out = Vec::new();
    for span in spans {
        if remaining == 0 {
            break;
        }
        let len = span.text.chars().count();
        if len <= remaining {
            out.push(span.clone());
            remaining -= len;
        } else {
            out.push(Span {
                text: span.text.chars().take(remaining).collect(),
                style: span.style,
            });
            remaining = 0;
        }
    }
    out
}

/// One display line of a top panel, in two parts: `prefix` stays pinned
/// at the left edge under horizontal scrolling (the fixed-width line
/// numbering) and never carries accent color — just the theme base, so it
/// stays a stable visual anchor — while `text` is the (possibly
/// multi-styled) content that shifts. `dim` lines render in the active
/// theme's `dim` color regardless of any span styling: the synthesized
/// preamble in the proof panel, placeholders, and other text that's
/// context rather than content.
/// `cursor` marks the one line (if any) under the Vim-mode editing
/// cursor in the Proof panel, or `:formula`'s browse cursor in the
/// Formula panel — mutually exclusive with `dim` in practice, since only
/// real buffer/formula lines can ever be the cursor. `assertion` marks an
/// `a`-rule (unchecked assertion) in the Proof panel, flagged with a
/// background as scaffolding still owed a real derivation — cursor wins if
/// both apply (see `panel_cell`), an active edit being the more urgent
/// signal. `core` is the Database panel's equivalent: `Some(true)`/
/// `Some(false)` for a core/derived constraint's row, `None` everywhere
/// else. `hint` is *also* Database-panel-only, and can coincide with
/// `core`: while `:deassert` is active, it marks every live constraint
/// sharing a variable with the assertion being replaced (see
/// `database_lines`) — reusing the browse cursor's own color, since the
/// Database panel never has an actual browse cursor of its own to
/// collide with, and wins over the plain core/derived background when
/// both apply (see `panel_cell`), the same "more urgent signal" logic
/// `cursor` already gets over `assertion`. `error` marks the one buffer
/// line `:verify` most recently rejected (`Session::known_bad`), waiting
/// to be fixed (see `proof_lines`) — wins over everything else
/// (see `panel_cell`), since nothing else marks a line as actively broken
/// the way this does. Never coincides with `assertion` in practice (an
/// `a`-rule is never itself rejected — it's unchecked), but would win
/// regardless if it somehow did. `marker_color`, when set, overrides
/// just the *first character* of `prefix` — the Proof pane's
/// breakpoint-marker glyph — with its own color regardless of any of
/// the above; see `render_prefix`. `None` everywhere but the Proof
/// pane's own buffer/preamble lines (the only ones a marker glyph ever
/// occupies at all), reproducing the plain single-color-prefix behavior
/// every line had before this field existed. `debug_current` marks the
/// one Proof-panel line `:debug` mode is currently stopped at — the
/// same `theme.cursor_bg` highlight `cursor` gets, since it's the same
/// idea ("this is the line you're looking at right now"), just driven
/// by `checked_len` during stepping instead of the Vim cursor; loses to
/// `error` (see `panel_cell`) since a rejection at that exact line is
/// the more urgent signal, but otherwise takes the same priority
/// `cursor` does.
struct PanelLine {
    prefix: String,
    text: Vec<Span>,
    dim: bool,
    cursor: bool,
    assertion: bool,
    core: Option<bool>,
    hint: bool,
    error: bool,
    debug_current: bool,
    marker_color: Option<Color>,
}

/// A numbered content line: pinned `prefix`, scrollable styled `text`.
fn content(prefix: String, text: Vec<Span>, dim: bool) -> PanelLine {
    PanelLine {
        prefix,
        text,
        dim,
        cursor: false,
        assertion: false,
        core: None,
        hint: false,
        error: false,
        debug_current: false,
        marker_color: None,
    }
}

/// A fully pinned line (placeholders): never scrolls, never styled.
fn pinned(text: String) -> PanelLine {
    PanelLine {
        prefix: text,
        text: Vec::new(),
        dim: true,
        cursor: false,
        assertion: false,
        core: None,
        hint: false,
        error: false,
        debug_current: false,
        marker_color: None,
    }
}

/// A pane's visible window plus where it sits in the full content — the
/// vertical scrollbar's inputs.
struct Window {
    lines: Vec<PanelLine>,
    /// Index into the full content of the first visible line.
    top: usize,
    /// Total content lines, visible or not.
    total: usize,
}

/// A fully prepared top pane, ready to render row by row: its visible
/// content plus scrollbar geometry. The bars borrow from the pane itself
/// — a column for the vertical one, a row for the horizontal one — so
/// content dimensions and bar presence are resolved together here.
struct PaneView {
    lines: Vec<PanelLine>,
    /// Vertical thumb over the content rows; `None` = no bar column.
    vbar: Option<Range<usize>>,
    /// Horizontal thumb over the content columns; `None` = no bar row.
    hbar: Option<Range<usize>>,
    /// Content width: the pane minus the vertical bar's column, if any.
    eff_w: usize,
    /// Content rows: the pane minus the horizontal bar's row, if any.
    content_h: usize,
    /// The (clamped) horizontal offset the content is rendered at.
    h_offset: usize,
}

/// Resolve one top pane: clamp its offsets, window its lines, and decide
/// both scrollbars. The circular dependency (a bar steals space, which
/// can change whether the other bar is needed) is broken with one
/// pre-pass: assume the vertical bar from the raw height, decide the
/// horizontal bar against the reduced width, then finalize.
fn pane_view(
    lines: Vec<PanelLine>,
    w: usize,
    h: usize,
    v_offset: &mut usize,
    h_offset: &mut usize,
) -> PaneView {
    let total = lines.len();
    let longest = longest_line(&lines);
    let vbar_expected = total > h;
    // The real `eff_w` isn't known until the vertical bar is finalized
    // below — this is the same "assume the vertical bar from the raw
    // height" pre-pass the module doc above already relies on to break
    // the circular dependency, reused here so the clamp bound and the
    // horizontal bar's own on/off decision never disagree with each other.
    let avail_w = w.saturating_sub(vbar_expected as usize);
    *h_offset = clamp_hscroll(*h_offset, longest, avail_w);

    let hbar_on = longest > avail_w;
    let content_h = if hbar_on { h - 1 } else { h };

    let window = window_scrolled(lines, content_h, v_offset);
    let vbar = vbar(window.top, window.total, content_h);
    let eff_w = w - (vbar.is_some() as usize);
    let hbar = hbar_on.then(|| hbar_range(eff_w, *h_offset, longest));

    PaneView {
        lines: window.lines,
        vbar,
        hbar,
        eff_w,
        content_h,
        h_offset: *h_offset,
    }
}

/// Render row `i` (of the pane's full height) as a string of exactly the
/// pane's width: a content row plus its slice of the vertical bar, or —
/// on the last row when present — the horizontal bar with a track-filled
/// corner under the vertical one.
fn pane_row(view: &PaneView, i: usize, theme: Theme) -> String {
    if i >= view.content_h {
        let Some(thumb) = &view.hbar else {
            return base(
                &" ".repeat(view.eff_w + view.vbar.is_some() as usize),
                theme,
            );
        };
        let mut row = String::new();
        row.push_str(&chrome(&"─".repeat(thumb.start), theme));
        row.push_str(
            &"━"
                .repeat(thumb.len())
                .with(theme.fg)
                .on(theme.bg)
                .to_string(),
        );
        row.push_str(&chrome(&"─".repeat(view.eff_w - thumb.end), theme));
        if view.vbar.is_some() {
            // Corner under the vertical track: the horizontal track
            // continues through it, dimmed like the rest.
            row.push_str(&chrome("─", theme));
        }
        return row;
    }
    let cell = panel_cell(&view.lines, i, view.eff_w, view.h_offset, theme);
    match &view.vbar {
        Some(range) if range.contains(&i) => format!("{cell}{}", "┃".with(theme.fg).on(theme.bg)),
        Some(_) => format!("{cell}{}", chrome("│", theme)),
        None => cell,
    }
}

pub fn draw(w: &mut impl Write, app: &mut App) -> anyhow::Result<()> {
    let theme = app.theme.theme();
    // Recomputed fresh every frame (not just on a mouse-move event) so
    // it's never stale after something *other* than the mouse changed
    // what's actually under it — a keyboard-driven scroll, a resize —
    // against `self.last_layout`, i.e. one frame behind, the same
    // "as of the last frame drawn" tradeoff `App::pane_at` already
    // documents and accepts. See `App::recompute_bp_hover`.
    app.recompute_bp_hover();
    let (width, height) = terminal::size()?;
    queue!(w, cursor::Hide)?;

    let wanted_suggestions = if app.candidates.is_empty() {
        0
    } else {
        SUGGESTION_ROWS as u16
    };

    let Some(layout) = Layout::compute(width, height, wanted_suggestions, app.zoom) else {
        queue!(
            w,
            terminal::Clear(terminal::ClearType::All),
            cursor::MoveTo(0, 0),
            Print(base(
                "Terminal too small — enlarge it, or run with --plain.",
                theme
            ))
        )?;
        w.flush()?;
        return Ok(());
    };

    let (w1, w2, w3) = (
        layout.cols[0] as usize,
        layout.cols[1] as usize,
        layout.cols[2] as usize,
    );
    // Also a solo-zoomed pane's full width: no internal column separators
    // to give up space to, same as the bottom Output pane never has any.
    let inner = (width - 2) as usize;
    let top_h = layout.top_h as usize;
    // `None` both when nothing's zoomed and when `Output` is (semi- or
    // full-maximised alike) — a semi-maximised `Output` still shows all
    // three columns, just shrunk, and a fully-maximised one shows none of
    // them, but neither is "one of the three columns took over the row"
    // the way a solo-zoomed top pane is. See `Layout::zoomed_top_pane`.
    let zoomed_pane = layout.zoomed_top_pane().map(|(pane, _)| pane);
    let output_zoom = match layout.zoomed {
        Some((Pane::Output, level)) => Some(level),
        _ => None,
    };
    // `Output` full-maximised: no top area at all (`top_h == 0`), so
    // Formula/Database/Proof get no `PaneView` built this frame — a
    // zero-height one would underflow `pane_view`'s own math, which
    // assumes at least a sliver of real space to work with.
    let output_fullscreen = layout.output_fullscreen();

    // Panel content and titles.
    let (formula_title, formula) = match &app.session {
        Some(session) => (
            format!(
                "Formula: {} ({})",
                file_name(&session.formula_path),
                session.formula.len()
            ),
            formula_lines(session, app.formula_browse_cursor()),
        ),
        None => (
            "Formula".to_string(),
            vec![pinned("(no formula loaded)".to_string())],
        ),
    };
    // What `:deassert` is currently replacing, if anything — shared by
    // every place that needs to say so: the Output heading, the Database
    // heading (so a highlighted subset there doesn't read as "this is
    // the whole database"), and the highlighting itself.
    let deassert_target = app.vim_deassert_source();
    // While `:deassert` is active, every live constraint sharing a
    // variable with the assertion being replaced gets highlighted in this
    // pane — kept current every frame, unlike the one-time hint list
    // `:deassert` also prints (see `PanelLine::hint`'s own docs).
    let hint_vars = match (&app.session, deassert_target) {
        (Some(session), Some(source)) => {
            edit::mentioned_vars(source, &session.current_checker.context.var_names)
        }
        _ => Vec::new(),
    };
    let database = app
        .session
        .as_ref()
        .map_or_else(Vec::new, |s| database_lines(s, &hint_vars));
    let database_title = match deassert_target {
        // Only some rows are highlighted below — say so right in the
        // heading, the same phrasing Output's own heading uses, so the
        // two read as one signal rather than needing to be puzzled out
        // separately.
        Some(source) => format!("Database ({}) (deasserting {source})", database.len()),
        None => format!("Database ({})", database.len()),
    };
    // While Vim mode's `Insert` sub-mode is active, the line under the
    // cursor renders `App::editor`'s live text instead of the buffer's
    // own (not-yet-committed) content — see `proof_lines`'s own docs.
    let vim_live_text: Option<(usize, String)> = app
        .vim_inserting()
        .then(|| app.vim_cursor_line().map(|line| (line, app.editor.text())))
        .flatten();
    let proof = app.session.as_ref().map_or_else(Vec::new, |s| {
        proof_lines(
            s,
            app.vim_cursor_line(),
            vim_live_text.as_ref().map(|(l, t)| (*l, t.as_str())),
            app.bp_hover(),
            app.debug_current_line(),
            theme,
        )
    });
    let proof_title = match &app.session {
        Some(session) => {
            let total = session.buffer.len();
            let unchecked = total - session.checked_len;
            if unchecked > 0 {
                format!("Proof ({total}) ({unchecked} unchecked)")
            } else {
                format!("Proof ({total})")
            }
        }
        None => "Proof (0)".to_string(),
    };
    // While `:deassert` is active, the assertion it's replacing stays
    // named in the Output heading itself — the only thing on screen
    // that's still visible once you've started typing the real derivation
    // steps replacing it, since by then the intro message that first
    // announced it has scrolled off and the prompt is showing whatever
    // you're currently typing instead. Plain `:edit`/`:insert` (and no
    // edit at all) leave the heading as plain "Output".
    let output_title = match deassert_target {
        Some(source) => format!("Output (deasserting {source})"),
        None => "Output".to_string(),
    };

    // A pane zoomed to something else is hidden outright — no `PaneView`
    // built for it at all. The zoomed pane itself gets `single_width`
    // instead of its normal column share, since there are no column
    // separators to give up space to when it's the only thing on the top
    // row. When `Output` is full-maximised, none of the three get a
    // `PaneView` at all — see `output_fullscreen` above.
    let formula_w = if zoomed_pane == Some(Pane::Formula) {
        inner
    } else {
        w1
    };
    let database_w = if zoomed_pane == Some(Pane::Database) {
        inner
    } else {
        w2
    };
    let proof_w = if zoomed_pane == Some(Pane::Proof) {
        inner
    } else {
        w3
    };
    let show_top = !output_fullscreen;
    let formula =
        (show_top && (zoomed_pane.is_none() || zoomed_pane == Some(Pane::Formula))).then(|| {
            pane_view(
                formula,
                formula_w,
                top_h,
                &mut app.formula_scroll,
                &mut app.formula_hscroll,
            )
        });
    let database = (show_top && (zoomed_pane.is_none() || zoomed_pane == Some(Pane::Database)))
        .then(|| {
            pane_view(
                database,
                database_w,
                top_h,
                &mut app.database_scroll,
                &mut app.database_hscroll,
            )
        });
    // Auto-scroll the Proof pane horizontally to keep Vim mode's cursor
    // column in view — the same "keep the cursor visible" idea
    // `LineEditor::view` already applies to the bottom prompt, applied
    // here to `app.proof_hscroll` instead, ahead of `pane_view`'s own
    // clamp below (which only knows about the *content*'s width, not
    // where the cursor happens to sit within it).
    if let Some((_, col)) = app.vim_cursor() {
        let num_w = app
            .session
            .as_ref()
            .map_or(1, |s| number_width(s.preamble_lines().len() + s.buffer.len()));
        let target_col = proof_prefix_width(num_w) + col;
        // Reserve a column for a vertical scrollbar that might appear —
        // `pane_view`'s own clamp (right after) settles the exact value.
        let avail_w = proof_w.saturating_sub(1);
        if target_col < app.proof_hscroll {
            app.proof_hscroll = target_col;
        } else if avail_w > 0 && target_col >= app.proof_hscroll + avail_w {
            app.proof_hscroll = target_col + 1 - avail_w;
        }
    }
    let proof =
        (show_top && (zoomed_pane.is_none() || zoomed_pane == Some(Pane::Proof))).then(|| {
            pane_view(
                proof,
                proof_w,
                top_h,
                &mut app.proof_scroll,
                &mut app.proof_hscroll,
            )
        });

    // Scrollback view: the tail, scrolled up by `scroll_up` (clamped now
    // that we know the pane height) — plus, same as the three top panes,
    // a horizontal offset and its own scrollbar when the widest line in
    // the whole scrollback (not just what's vertically visible right now,
    // matching `pane_view`'s own scope) overruns the pane. Resolved in
    // the same order `pane_view` uses, for the same reason: the two bars
    // are circularly dependent (one steals space the other's decision
    // depends on), so decide the horizontal bar against the raw height
    // first, then window vertically against whatever's left.
    let sb_h = layout.scrollback_h as usize;
    let sb = app.scrollback.lines();
    let sb_total = sb.len();
    let sb_longest = sb.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let sb_vbar_expected = sb_total > sb_h;
    let sb_avail_w = inner.saturating_sub(sb_vbar_expected as usize);
    app.output_hscroll = clamp_hscroll(app.output_hscroll, sb_longest, sb_avail_w);
    let sb_hbar_on = sb_longest > sb_avail_w;
    let sb_content_h = if sb_hbar_on { sb_h - 1 } else { sb_h };

    // Remember this frame's geometry — mouse hit-testing, PgUp/PgDn
    // paging, and the browse cursor's row math all interpret the next
    // events against what's actually on screen now. `content_h` (not
    // `top_h`) is what the Proof pane really shows — a horizontal
    // scrollbar row, decided only here in `pane_view`, can claim one. `0`
    // when the pane's hidden behind a different zoom this frame — nothing
    // to scroll a cursor within if it isn't shown.
    app.last_layout = Some(layout);
    app.proof_content_h = proof.as_ref().map_or(0, |v| v.content_h);
    app.formula_content_h = formula.as_ref().map_or(0, |v| v.content_h);
    app.scroll_up = app.scroll_up.min(sb_total.saturating_sub(sb_content_h));
    let sb_end = sb_total - app.scroll_up;
    let sb_start = sb_end.saturating_sub(sb_content_h);
    let visible = &sb[sb_start..sb_end];
    let output_vbar = vbar(sb_start, sb_total, sb_content_h);
    let sb_eff_w = inner - (output_vbar.is_some() as usize);
    let output_hbar = sb_hbar_on.then(|| hbar_range(sb_eff_w, app.output_hscroll, sb_longest));

    // Build every row of the frame, top to bottom. Every literal
    // box-drawing character goes through `chrome` (theme.dim on
    // theme.bg) rather than being printed bare — that's what makes the
    // frame's background genuinely app-wide instead of just its content.
    let mut rows: Vec<String> = Vec::with_capacity(height as usize);
    let bar = chrome("│", theme);
    if output_fullscreen {
        // No top area at all: Output's own top border is the very first
        // row of the frame — see `Layout::output_fullscreen`. `top_h == 0`
        // means no content-row loop and no separator row either; nothing
        // to separate Output from up here.
        rows.push(format!(
            "{}{}{}",
            chrome("┌", theme),
            title_segment(
                Some(&output_title),
                inner,
                app.focus == Pane::Output,
                layout::zoom_levels(Pane::Output),
                output_zoom,
                theme
            ),
            chrome("┐", theme),
        ));
    } else {
        match layout.zoomed_top_pane() {
            None => {
                let formula_view = formula.as_ref().expect("shown — always built above");
                let database_view = database.as_ref().expect("shown — always built above");
                let proof_view = proof.as_ref().expect("shown — always built above");
                rows.push(format!(
                    "{}{}{}{}{}{}{}",
                    chrome("┌", theme),
                    title_segment(
                        Some(&formula_title),
                        w1,
                        app.focus == Pane::Formula,
                        layout::zoom_levels(Pane::Formula),
                        None,
                        theme
                    ),
                    chrome("┬", theme),
                    title_segment(
                        Some(&database_title),
                        w2,
                        app.focus == Pane::Database,
                        layout::zoom_levels(Pane::Database),
                        None,
                        theme
                    ),
                    chrome("┬", theme),
                    title_segment(
                        Some(&proof_title),
                        w3,
                        app.focus == Pane::Proof,
                        layout::zoom_levels(Pane::Proof),
                        None,
                        theme
                    ),
                    chrome("┐", theme),
                ));
                for i in 0..top_h {
                    rows.push(format!(
                        "{bar}{}{bar}{}{bar}{}{bar}",
                        pane_row(formula_view, i, theme),
                        pane_row(database_view, i, theme),
                        pane_row(proof_view, i, theme)
                    ));
                }
                // `Output`'s row label lives under the Formula column (no
                // buttons there — `&[]`); its own buttons sit at the far
                // right instead, under Proof, matching every other
                // header's "buttons at the pane's own right edge" rule —
                // see `Layout::header_button_at`'s doc for why.
                rows.push(format!(
                    "{}{}{}{}{}{}{}",
                    chrome("├", theme),
                    title_segment(
                        Some(&output_title),
                        w1,
                        app.focus == Pane::Output,
                        &[],
                        None,
                        theme
                    ),
                    chrome("┴", theme),
                    chrome(&"─".repeat(w2), theme),
                    chrome("┴", theme),
                    title_segment(
                        None,
                        w3,
                        false,
                        layout::zoom_levels(Pane::Output),
                        output_zoom,
                        theme
                    ),
                    chrome("┤", theme),
                ));
            }
            Some((pane, level)) => {
                let (title, view) = match pane {
                    Pane::Formula => (&formula_title, formula.as_ref()),
                    Pane::Database => (&database_title, database.as_ref()),
                    Pane::Proof => (&proof_title, proof.as_ref()),
                    Pane::Output => unreachable!(
                        "Output never solo-zooms the top row — see Layout::zoomed_top_pane"
                    ),
                };
                let view = view.expect("the zoomed pane is always built above");
                rows.push(format!(
                    "{}{}{}",
                    chrome("┌", theme),
                    title_segment(
                        Some(title),
                        inner,
                        app.focus == pane,
                        layout::zoom_levels(pane),
                        Some(level),
                        theme
                    ),
                    chrome("┐", theme),
                ));
                for i in 0..top_h {
                    rows.push(format!("{bar}{}{bar}", pane_row(view, i, theme)));
                }
                rows.push(format!(
                    "{}{}{}",
                    chrome("├", theme),
                    title_segment(
                        Some(&output_title),
                        inner,
                        app.focus == Pane::Output,
                        layout::zoom_levels(Pane::Output),
                        output_zoom,
                        theme
                    ),
                    chrome("┤", theme),
                ));
            }
        }
    }
    for i in 0..sb_h {
        // The horizontal scrollbar's own row, if it exists, is always the
        // last one — same "steals the bottom row" convention `pane_row`
        // uses for the top panes.
        if i >= sb_content_h {
            let thumb = output_hbar
                .clone()
                .expect("content_h < sb_h only when the horizontal bar is on");
            let mut row = String::new();
            row.push_str(&chrome(&"─".repeat(thumb.start), theme));
            row.push_str(
                &"━"
                    .repeat(thumb.len())
                    .with(theme.fg)
                    .on(theme.bg)
                    .to_string(),
            );
            row.push_str(&chrome(&"─".repeat(sb_eff_w - thumb.end), theme));
            if output_vbar.is_some() {
                // Corner under the vertical track, dimmed like the rest —
                // same as `pane_row`'s equivalent corner.
                row.push_str(&chrome("─", theme));
            }
            rows.push(format!("{bar}{row}{bar}"));
            continue;
        }
        let line = visible.get(i).map(String::as_str).unwrap_or("");
        let cell = base(&windowed(line, app.output_hscroll, sb_eff_w), theme);
        rows.push(match &output_vbar {
            Some(range) if range.contains(&i) => {
                format!("{bar}{cell}{}{bar}", "┃".with(theme.fg).on(theme.bg))
            }
            Some(_) => format!("{bar}{cell}{}{bar}", chrome("│", theme)),
            None => format!("{bar}{cell}{bar}"),
        });
    }
    // While `:formula`'s browse mode (or `:debug` mode) is active, the
    // prompt label itself is the "what mode am I in?" cue — a distinct
    // label text (matching the plain frontend, which has no color to lean
    // on) plus the same accent color Vim mode's cursor row uses
    // (`theme.cursor_accent`, paired with `theme.cursor_bg`), so it can't
    // be confused with the focus-reverse already used elsewhere (including
    // on this very prompt, for "Output has focus"). Vim mode replaces this
    // row entirely with its own `-- NORMAL --`/`-- INSERT --` status
    // instead (below) — its own cursor lives in the Proof pane, not here;
    // `:debug` has no such replacement, since (unlike Vim/formula-browse)
    // it's not a custom TUI UI — it's the same `debug>` prompt the plain
    // frontend shows, just with this one label swap and a couple of extra
    // keyboard shortcuts (see `App::execute`/`run`).
    let editing = app.formula_editing();
    let debugging = app.debug_active();
    let prompt_label = if debugging {
        DEBUG_PROMPT
    } else if editing {
        FORMULA_PROMPT
    } else {
        NORMAL_PROMPT
    };
    let (input_view, cursor_offset) = app.editor.view(inner - prompt_label.len());
    if app.vim_active() {
        let status_text = if app.vim_inserting() {
            "-- INSERT --  (Esc/Enter commits the line)"
        } else {
            "-- NORMAL --  hjkl/arrows move · i/a/o/O insert · x del char · dd del line · \
             : command · Esc leave"
        };
        let styled = fit(status_text, inner)
            .with(theme.cursor_accent)
            .on(theme.bg)
            .to_string();
        rows.push(format!("{bar}{styled}{bar}"));
    } else {
        let styled_label = if editing || debugging {
            prompt_label
                .with(theme.cursor_accent)
                .on(theme.bg)
                .to_string()
        } else {
            base(prompt_label, theme)
        };
        rows.push(format!(
            "{bar}{styled_label}{}{bar}",
            base(&fit(&input_view, inner - prompt_label.len()), theme)
        ));
    }
    for (line, highlighted) in suggestion_rows(
        &app.candidates,
        app.selected,
        layout.suggestion_rows as usize,
    ) {
        let cell = fit(&line, inner);
        if highlighted {
            rows.push(format!(
                "{bar}{}{bar}",
                cell.with(theme.fg).on(theme.bg).reverse()
            ));
        } else {
            rows.push(format!("{bar}{}{bar}", chrome(&cell, theme)));
        }
    }
    rows.push(format!(
        "{}{}{}",
        chrome("└", theme),
        chrome(&"─".repeat(inner), theme),
        chrome("┘", theme)
    ));

    for (y, row) in rows.iter().enumerate() {
        queue!(w, cursor::MoveTo(0, y as u16), Print(row))?;
    }
    // Vim mode's real terminal cursor renders directly in the Proof pane,
    // at the exact (line, column) `VimState` tracks — not at the bottom
    // prompt row, which shows a status line instead while it's active
    // (above). Falls back to the ordinary prompt position if the Proof
    // pane isn't actually on screen this frame (a different pane is
    // solo-zoomed, or Output is full-maximised): better a visible cursor
    // in the wrong-but-sensible place than one silently vanishing.
    let vim_pane_cursor = (|| {
        let (line, col) = app.vim_cursor()?;
        let proof_view = proof.as_ref()?;
        let x0 = layout.proof_content_x0() as usize;
        let num_w = app
            .session
            .as_ref()
            .map_or(1, |s| number_width(s.preamble_lines().len() + s.buffer.len()));
        let prefix_w = proof_prefix_width(num_w);
        let row_in_window = (line - 1).saturating_sub(app.proof_scroll);
        let y = 1 + row_in_window.min(proof_view.content_h.saturating_sub(1));
        let x = x0 + prefix_w + col.saturating_sub(app.proof_hscroll);
        Some((x as u16, y as u16))
    })();
    let (cursor_x, cursor_y) = vim_pane_cursor.unwrap_or((
        (1 + prompt_label.len() + cursor_offset) as u16,
        layout.prompt_row,
    ));
    queue!(w, cursor::MoveTo(cursor_x, cursor_y), cursor::Show)?;
    w.flush()?;
    Ok(())
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The vertical scrollbar thumb for a pane: which of its `h` content rows
/// the thumb covers, or `None` when everything fits (no bar at all).
/// Thumb length is proportional to the visible fraction, position to how
/// far down the content the window sits.
fn vbar(top: usize, total: usize, h: usize) -> Option<Range<usize>> {
    if total <= h || h == 0 {
        return None;
    }
    let thumb = ((h * h) / total).max(1);
    let max_top = total - h;
    let max_start = h - thumb;
    let start = (top.min(max_top) * max_start + max_top / 2) / max_top;
    Some(start..start + thumb)
}

/// The horizontal thumb over `width` bar cells: proportional to how much
/// of the pane's widest line (`content` columns) is visible, positioned
/// by the current offset. Full-width if everything somehow fits.
fn hbar_range(width: usize, offset: usize, content: usize) -> Range<usize> {
    if content <= width || width == 0 {
        return 0..width;
    }
    let thumb = ((width * width) / content).max(1);
    let max_off = content - width;
    let max_start = width - thumb;
    let start = (offset.min(max_off) * max_start + max_off / 2) / max_off;
    start..start + thumb
}

/// The widest line of a pane, pinned prefix included — what the
/// horizontal scrollbar measures against.
fn longest_line(lines: &[PanelLine]) -> usize {
    lines
        .iter()
        .map(|l| l.prefix.chars().count() + spans_char_len(&l.text))
        .max()
        .unwrap_or(0)
}

/// The suggestion strip's rows: left column (usage or file name, aligned)
/// plus right column (summary, empty for files), each flagged with
/// whether it's the highlighted selection. Always exactly `rows` entries
/// (blank-padded), so the strip — and with it the prompt — never changes
/// height while visible. With no selection, an overflowing list spends
/// its last row on a `… (+N more)` marker; once ↑/↓ browsing starts, the
/// window slides to keep the highlight in view and the highlight itself
/// shows position.
fn suggestion_rows(
    candidates: &[crate::tui::complete::Candidate],
    selected: Option<usize>,
    rows: usize,
) -> Vec<(String, bool)> {
    if rows == 0 || candidates.is_empty() {
        return vec![(String::new(), false); rows];
    }
    let (start, shown, marker) = match selected {
        None if candidates.len() > rows => (0, rows - 1, true),
        None => (0, candidates.len().min(rows), false),
        Some(s) => {
            let shown = rows.min(candidates.len());
            let start = s.saturating_sub(shown - 1).min(candidates.len() - shown);
            (start, shown, false)
        }
    };
    let window = &candidates[start..start + shown];
    let width = window
        .iter()
        .map(|c| c.left.chars().count())
        .max()
        .unwrap_or(0);
    let mut out: Vec<(String, bool)> = window
        .iter()
        .enumerate()
        .map(|(i, c)| {
            (
                format!(" {:width$}  {}", c.left, c.right),
                selected == Some(start + i),
            )
        })
        .collect();
    if marker {
        out.push((format!(" … (+{} more)", candidates.len() - shown), false));
    }
    while out.len() < rows {
        out.push((String::new(), false));
    }
    out
}

/// Render `prefix` in `default_fg` on `bg` — except its very first
/// character, which renders in `marker_color` instead when set,
/// regardless of what row-level styling is asking for everywhere else
/// (`default_fg`/`bg` included). The one place a `PanelLine`'s
/// otherwise-uncolored `prefix` (see its own docs) gets to carry a
/// color of its own: the Proof pane's breakpoint-marker glyph at
/// column 0, which needs to stay a consistent, recognizable color
/// (active-red or hover-dimmed — see `breakpoint_marker`) whatever the
/// row underneath it is doing. `None` reproduces the plain,
/// single-color-for-the-whole-prefix rendering every other panel line
/// (and a marker-less Proof line) still gets.
fn render_prefix(
    prefix: &str,
    marker_color: Option<Color>,
    default_fg: Color,
    bg: Color,
) -> String {
    let Some(marker_color) = marker_color else {
        return prefix.with(default_fg).on(bg).to_string();
    };
    let mut chars = prefix.chars();
    let Some(marker) = chars.next() else {
        return String::new();
    };
    let rest: String = chars.collect();
    format!(
        "{}{}",
        marker.to_string().with(marker_color).on(bg),
        rest.with(default_fg).on(bg)
    )
}

/// A whole row — prefix (see `render_prefix`), each already-clipped
/// span, and the trailing padding — on a solid background, prefix and
/// padding included so the highlight fills the entire cell rather than
/// just the text. Shared by every row-level highlight `panel_cell` can
/// produce.
fn row_on_bg(
    prefix: &str,
    clipped: &[Span],
    padding: &str,
    bg: Color,
    theme: Theme,
    marker_color: Option<Color>,
) -> String {
    let mut cell = render_prefix(prefix, marker_color, theme.fg, bg);
    for span in clipped {
        cell.push_str(&style_span_on_bg(span, bg, theme));
    }
    cell.push_str(&padding.with(theme.fg).on(bg).to_string());
    cell
}

/// One content cell: the panel's line `i` clipped hard to `width` (the
/// scrollbars signal continuation now — no ellipses), blank-padded past
/// the end of the panel's lines. The prefix stays pinned at the left edge
/// under horizontal scrolling and never carries accent/highlight color
/// of its own (just the theme base, or whatever row-level background
/// applies) — except its very first character when `marker_color` is
/// set (see `render_prefix`) — while the text after it shifts left by
/// `h_offset` columns and, for a non-dim line, keeps each token's own
/// color — styling is applied per-span only after slicing/padding, so
/// the embedded ANSI never has a chance to get cut mid-sequence.
fn panel_cell(
    lines: &[PanelLine],
    i: usize,
    width: usize,
    h_offset: usize,
    theme: Theme,
) -> String {
    let Some(line) = lines.get(i) else {
        return base(&" ".repeat(width), theme);
    };
    // Truncate the prefix itself first (only relevant on a pathologically
    // narrow pane) so the rest of the math can assume it always fits.
    let prefix: String = line.prefix.chars().take(width).collect();
    let avail = width - prefix.chars().count();

    let sliced = if h_offset == 0 {
        line.text.clone()
    } else {
        spans_skip(&line.text, h_offset)
    };
    let clipped = spans_take(&sliced, avail);
    let pad = avail - spans_char_len(&clipped);
    let padding = " ".repeat(pad);

    if line.error {
        row_on_bg(
            &prefix,
            &clipped,
            &padding,
            theme.error_bg,
            theme,
            line.marker_color,
        )
    } else if line.debug_current {
        row_on_bg(
            &prefix,
            &clipped,
            &padding,
            theme.cursor_bg,
            theme,
            line.marker_color,
        )
    } else if line.dim {
        let mut body: String = clipped.iter().map(|s| s.text.as_str()).collect();
        body.push_str(&padding);
        format!(
            "{}{}",
            render_prefix(&prefix, line.marker_color, theme.dim, theme.bg),
            body.with(theme.dim).on(theme.bg)
        )
    } else if line.cursor {
        row_on_bg(
            &prefix,
            &clipped,
            &padding,
            theme.cursor_bg,
            theme,
            line.marker_color,
        )
    } else if line.assertion {
        row_on_bg(
            &prefix,
            &clipped,
            &padding,
            theme.assertion_bg,
            theme,
            line.marker_color,
        )
    } else if line.hint {
        row_on_bg(
            &prefix,
            &clipped,
            &padding,
            theme.cursor_bg,
            theme,
            line.marker_color,
        )
    } else if line.core == Some(true) {
        row_on_bg(
            &prefix,
            &clipped,
            &padding,
            theme.core_bg,
            theme,
            line.marker_color,
        )
    } else if line.core == Some(false) {
        row_on_bg(
            &prefix,
            &clipped,
            &padding,
            theme.derived_bg,
            theme,
            line.marker_color,
        )
    } else {
        let mut body: String = clipped.iter().map(|s| style_span(s, theme)).collect();
        body.push_str(&base(&padding, theme));
        format!(
            "{}{body}",
            render_prefix(&prefix, line.marker_color, theme.fg, theme.bg)
        )
    }
}

/// Largest useful horizontal offset for a pane `avail_w` columns wide
/// whose longest line is `longest`: exactly far enough that the content's
/// own right edge lands on the pane's right edge, no further — scrolling
/// past that would only show blank padding, since everything the line
/// has left already fits. Matches the bound `hbar_range` already assumes
/// for the thumb's own rightmost position (`max_off = content - width`);
/// previously this clamped to `longest - 1` instead, which let scrolling
/// continue well past a fully-revealed line, all the way to pinning its
/// very last character at the pane's *left* edge.
fn clamp_hscroll(offset: usize, longest: usize, avail_w: usize) -> usize {
    offset.min(longest.saturating_sub(avail_w))
}

/// Digits needed to print `n` — the fixed width panes pad their line
/// numbers to, so the numbering column doesn't wobble: `  9: `, ` 10: `.
fn number_width(n: usize) -> usize {
    n.to_string().len()
}

/// Width, in columns, of everything in the Proof panel's numbering
/// column before a line's own text starts: the breakpoint-marker slot
/// (`●`/` `, always exactly one column either way — see
/// `breakpoint_marker`) plus a separating space, then `num_w` digits of
/// line number, then `": "`. `proof_lines` builds exactly this many
/// characters of prefix per line; `vim_pane_cursor` and the Proof pane's
/// horizontal auto-scroll (both in `draw`) need the identical number
/// without actually rendering a line, so this is the one formula both
/// read rather than each guessing their own — the same "one shared
/// source of truth" reason `tokenize_proof_line`'s char-for-char width
/// guarantee exists at all.
fn proof_prefix_width(num_w: usize) -> usize {
    2 + num_w + 2
}

/// The breakpoint-marker glyph for one Proof-panel line, and — unlike
/// every other character in `prefix` — the color to force it to
/// regardless of the row's own styling (`None` meaning "no marker here,
/// render the plain space like any other prefix character"; see
/// `render_prefix`). Three states, `buffer_idx` naming a real buffer
/// line (never the two synthesized preamble lines, which can't carry
/// a breakpoint or a hover preview either): already in
/// `session.breakpoints` renders `●` in `theme.breakpoint` (a
/// consistent, always-legible "stop here" red/orange, active whether
/// or not the mouse happens to be over it right now); not set but
/// `hovered` — the mouse resting exactly on this line's marker column,
/// see `tui::App::proof_marker_at` — previews the same glyph, dimmed
/// (`theme.dim`, the same "this is context, not committed yet" token
/// every other preview/placeholder in this app already uses), so a
/// click's effect is visible before it's taken; neither set nor
/// hovered is a plain space with no color override, exactly what every
/// line had before mouse-settable breakpoints existed. Always exactly
/// one column wide either way — `●` renders single-width in every
/// terminal this pane already assumes that of, the same class of
/// glyph as the `▭`/`⛶` zoom buttons already in the pane headers — so
/// which lines have (or might get) a breakpoint never changes the
/// numbering column's width, only what's drawn inside one fixed slot
/// of it.
fn breakpoint_marker(
    session: &Session,
    buffer_idx: Option<usize>,
    hovered: bool,
    theme: Theme,
) -> (char, Option<Color>) {
    match buffer_idx {
        Some(idx) if session.breakpoints.contains(&idx) => ('●', Some(theme.breakpoint)),
        Some(_) if hovered => ('●', Some(theme.dim)),
        _ => (' ', None),
    }
}

/// Styled `"@label @other "` spans for constraint `id`, empty when it has
/// no labels — meant to be prepended ahead of the constraint's own spans.
fn label_spans(labels_by_id: &ahash::AHashMap<isize, Vec<String>>, id: isize) -> Vec<Span> {
    let Some(names) = labels_by_id.get(&id) else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    for name in names {
        spans.push(label(name.clone()));
        spans.push(plain(" ".to_string()));
    }
    spans
}

/// Tokenize one `ToPrettyString`-formatted constraint into styled spans:
/// coefficients, the relation, and the degree stay plain; each literal's
/// variable name is colored, its optional leading `~` negation marker
/// stays plain. Exact by construction (see the module docs) — falls back
/// to a single plain span, rather than guessing, if the text doesn't
/// match the expected `"<coeff> <lit> ... >= <degree>"` shape, so a
/// future formatting change degrades to "no highlighting" instead of
/// silently dropping or garbling text.
fn tokenize_constraint(text: &str) -> Vec<Span> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let terms_len = tokens.len().saturating_sub(2);
    if tokens.len() < 2 || terms_len % 2 != 0 {
        return vec![plain(text.to_string())];
    }
    let (terms, tail) = tokens.split_at(terms_len);

    let mut spans = Vec::new();
    for (i, pair) in terms.chunks(2).enumerate() {
        if i > 0 {
            spans.push(plain(" ".to_string()));
        }
        // `terms_len` was checked even above, so every chunk here has
        // exactly 2 elements — no partial last chunk to guard against.
        spans.push(plain(pair[0].to_string()));
        spans.push(plain(" ".to_string()));
        spans.extend(tokenize_literal(pair[1]));
    }
    if !terms.is_empty() {
        spans.push(plain(" ".to_string()));
    }
    spans.push(plain(tail[0].to_string()));
    spans.push(plain(" ".to_string()));
    spans.push(plain(tail[1].to_string()));
    spans
}

/// A literal token (`"~name"` or `"name"`, `Lit::to_pretty_string`'s only
/// two forms) split into its plain `~` marker, if negated, and its
/// colored variable name.
fn tokenize_literal(lit: &str) -> Vec<Span> {
    match lit.strip_prefix('~') {
        Some(var) => vec![plain("~".to_string()), variable(var.to_string())],
        None => vec![variable(lit.to_string())],
    }
}

/// Tokenize one raw, as-typed proof-rule line: each whitespace token that
/// — after stripping an optional leading `~` — names a real variable in
/// `var_names` is colored; a token starting with `@` is a label (always,
/// per the v3 lexer grammar, regardless of whether it resolves in the
/// current label map); everything else (keywords, constraint-ID hints,
/// punctuation, numbers) stays plain. A lookup-based approach rather than
/// positional parsing, since proof-rule syntax has no single fixed
/// grammar the way a formatted constraint does (`rup`, `pol`, `red`, ...
/// all differ) — this can under-highlight (e.g. a variable name with
/// punctuation glued directly to it, with no separating space) but can
/// never mislabel a keyword or ID as a variable.
///
/// Crucially, the result is **character-for-character the same width as
/// `line`**: whitespace runs are preserved, not collapsed, so the Nth
/// character of `line` is always drawn in the Nth column of the pane.
/// That one-to-one mapping is load-bearing rather than cosmetic. Vim
/// mode's column (`App::vim_cursor`) is a character index into this very
/// string, and `draw` positions the real terminal cursor at that column,
/// so any rendering that changed a line's width would silently draw the
/// cursor on the wrong character. Rebuilding the line from
/// `split_whitespace` did exactly that: deleting the `1` from
/// `rup 1 ~x1 ...` leaves two spaces in the buffer but drew only one, so
/// the caret appeared to land on the `~` and the surviving space looked
/// as though it had been deleted along with the `1`; leading whitespace
/// vanished outright, sliding a whole line left underneath the cursor.
///
/// Whitespace is normalized to one space *per character* rather than
/// passed through verbatim: that keeps the width identical either way,
/// while stopping a stray tab from jumping to the terminal's next tab
/// stop and tearing the pane's own border off the right-hand side.
fn tokenize_proof_line(line: &str, var_names: &VarNameManager) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        // A run of whitespace: exactly one space out per character in.
        let ws_end = rest
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len());
        if ws_end > 0 {
            spans.push(plain(" ".repeat(rest[..ws_end].chars().count())));
            rest = &rest[ws_end..];
            continue;
        }
        // Otherwise a run of non-whitespace: one token, styled by what it
        // turns out to name. `ws_end == 0` guarantees this is non-empty,
        // so the loop always consumes something.
        let tok_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (token, tail) = rest.split_at(tok_end);
        rest = tail;
        if token.starts_with('@') {
            spans.push(label(token.to_string()));
            continue;
        }
        let (marker, name) = match token.strip_prefix('~') {
            Some(stripped) => ("~", stripped),
            None => ("", token),
        };
        if !name.is_empty() && var_names.get_idx(name).is_some() {
            if !marker.is_empty() {
                spans.push(plain(marker.to_string()));
            }
            spans.push(variable(name.to_string()));
        } else {
            spans.push(plain(token.to_string()));
        }
    }
    spans
}

/// Numbered pretty-printed formula constraints, 1-based to match the
/// constraint IDs the checker assigns them, labels shown ahead of the
/// constraint they name. `cursor`, when `Some`, highlights that
/// constraint's row — the Formula-pane equivalent of `proof_lines`'s own
/// `cursor` parameter, for `:formula`'s browse mode.
fn formula_lines(session: &Session, cursor: Option<usize>) -> Vec<PanelLine> {
    let var_names = &session.current_checker.context.var_names;
    let labels_by_id = session.labels_by_id();
    let num_w = number_width(session.formula.len());
    let mut lines: Vec<PanelLine> = session
        .formula
        .constraints
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut spans = label_spans(&labels_by_id, (i + 1) as isize);
            spans.extend(tokenize_constraint(&c.to_pretty_string(var_names)));
            content(format!("{:>num_w$}: ", i + 1), spans, false)
        })
        .collect();
    if let Some(cursor) = cursor
        && let Some(entry) = lines.get_mut(cursor.saturating_sub(1))
    {
        entry.cursor = true;
    }
    lines
}

/// The live constraint database, same iteration as `:show` with no
/// filters: deleted entries skipped, core/derived tagged, labels shown
/// ahead of the constraint they name. `hint_vars` — non-empty only while
/// `:deassert` is active, derived by `draw` from `App::vim_deassert_source`
/// via `edit::mentioned_vars` — marks every
/// row sharing any of those variables, kept current every frame rather
/// than frozen at whatever the database looked like when the edit began
/// (see `PanelLine::hint`'s own docs for why that matters).
fn database_lines(session: &Session, hint_vars: &[VarIdx]) -> Vec<PanelLine> {
    let checker = &session.current_checker;
    let var_names = &checker.context.var_names;
    let labels_by_id = session.labels_by_id();
    let num_w = number_width(checker.database.entries.len().saturating_sub(1));
    checker
        .database
        .entries
        .iter()
        .enumerate()
        .filter_map(|(id, entry)| {
            let entry = entry.as_ref()?;
            let mut spans = label_spans(&labels_by_id, id as isize);
            spans.extend(tokenize_constraint(
                &entry.constraint.to_pretty_string(var_names),
            ));
            let mut panel_line = content(format!("{id:>num_w$}: "), spans, false);
            panel_line.core = Some(entry.is_core_constraint());
            panel_line.hint = !hint_vars.is_empty()
                && (0..entry.constraint.len())
                    .filter_map(|i| entry.constraint.get_lit(i))
                    .any(|lit| hint_vars.contains(&lit.get_var()));
            Some(panel_line)
        })
        .collect()
}

/// The synthesized preamble (dimmed, left untokenized — it's boilerplate,
/// not content worth highlighting) followed by every buffer line
/// (tokenized per-line), numbered as one sequence so the numbers here
/// match the `line N:` numbers in the checker's trace output exactly.
/// `cursor` is Vim mode's current display line, if active — marked on
/// the matching entry for `panel_cell` to highlight. Every `a`-rule
/// (unchecked assertion) line is marked too, the same way `:deassert`
/// recognizes one — flagged red as scaffolding still owed a real
/// derivation.
///
/// `editing` is `Some((display_line, live_text))` while Vim mode's
/// `Insert` sub-mode is active (see `App::vim_inserting`): the named
/// line renders `live_text` — `App::editor`'s current, not-yet-committed
/// content — instead of `session.buffer`'s own text, so typing shows up
/// directly in the pane exactly where the cursor sits, not just once
/// Esc/Enter commits it. Checked/error status still reflects the
/// buffer's last-committed state regardless — genuinely reverifying on
/// every keystroke is `:verify`'s job, not this render.
///
/// Lines past `session.checked_len` are dimmed exactly like the preamble
/// — "not yet real" describes an unchecked line just as well as a
/// boilerplate one — except the one `session.known_bad` names, if any
/// (always the line right at `checked_len`: nothing beyond a rejection
/// is ever attempted), which gets `error` instead: the only thing in the
/// buffer that's actively broken, not just not-yet-tried.
///
/// `hover` is the buffer index (if any) the mouse currently sits over in
/// the marker column — see `App::proof_marker_at`, recomputed fresh every
/// frame from the last known mouse position, so it can never go stale
/// the way an event-driven-only update could after a keyboard scroll
/// moves what's actually under a stationary mouse. Only ever changes
/// `breakpoint_marker`'s glyph/color for the one named line.
///
/// `debug_current` is the buffer index `:debug` mode is currently
/// stopped at (see `App::debug_current_line`), or `None` when it isn't
/// active — marks that one line `debug_current` (see `PanelLine`'s own
/// docs) so `panel_cell` highlights it the same way the Vim cursor
/// highlights its line, and clears the "unchecked tail" dimming it
/// would otherwise get from sitting at or past `checked_len`.
fn proof_lines(
    session: &Session,
    cursor: Option<usize>,
    editing: Option<(usize, &str)>,
    hover: Option<usize>,
    debug_current: Option<usize>,
    theme: Theme,
) -> Vec<PanelLine> {
    let var_names = &session.current_checker.context.var_names;
    let preamble = session.preamble_lines();
    let base = preamble.len();
    let num_w = number_width(base + session.buffer.len());
    let mut lines: Vec<PanelLine> = preamble
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let (marker, marker_color) = breakpoint_marker(session, None, false, theme);
            let mut panel_line = content(
                format!("{marker} {:>num_w$}: ", i + 1),
                vec![plain(text)],
                true,
            );
            panel_line.marker_color = marker_color;
            panel_line
        })
        .collect();
    lines.extend(session.buffer.iter().enumerate().map(|(i, line)| {
        let display_line = base + i + 1;
        let text = match editing {
            Some((edit_line, live)) if edit_line == display_line => live,
            _ => line.as_str(),
        };
        let checked = i < session.checked_len;
        let (marker, marker_color) = breakpoint_marker(session, Some(i), hover == Some(i), theme);
        let mut panel_line = content(
            format!("{marker} {:>num_w$}: ", display_line),
            tokenize_proof_line(text, var_names),
            !checked,
        );
        panel_line.marker_color = marker_color;
        panel_line.assertion = checked && edit::is_assertion_line(text);
        if !checked && i == session.checked_len && session.known_bad.is_some() {
            panel_line.dim = false;
            panel_line.error = true;
        } else if debug_current == Some(i) {
            panel_line.dim = false;
            panel_line.debug_current = true;
        }
        panel_line
    }));
    if let Some(cursor) = cursor
        && let Some(entry) = lines.get_mut(cursor.saturating_sub(1))
    {
        entry.cursor = true;
    }
    lines
}

/// The visible window of a pane's lines, `h` rows of them — top-anchored:
/// `offset` lines down from the top, 0 meaning "line one is at the top."
/// All three top panes work this way, so newly-appended content (a
/// `:source`'d file, a fresh proof line, a new formula constraint) never
/// yanks the view down to follow it; only Output's own scrollback stays
/// tail-anchored (see the `draw` function's inline scrollback handling
/// below, which doesn't go through this at all). Clamps the offset
/// against the content, writing it back so stale offsets self-heal.
/// Overflow is the scrollbar's to report — the window is a plain slice.
fn window_scrolled(lines: Vec<PanelLine>, h: usize, offset: &mut usize) -> Window {
    let n = lines.len();
    if n <= h {
        *offset = 0;
        return Window {
            lines,
            top: 0,
            total: n,
        };
    }
    let max = n - h;
    *offset = (*offset).min(max);
    let top = *offset;
    Window {
        lines: lines.into_iter().skip(top).take(h).collect(),
        top,
        total: n,
    }
}

/// Pad or truncate `s` to exactly `width` columns, marking truncation
/// with a trailing `…`. Used by the prompt's own input line and the
/// suggestion strip — both single-line fields with no scrollbar of their
/// own to signal continuation, unlike the scrollback (see [`windowed`]).
fn fit(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > width {
        let mut out: String = chars[..width - 1].iter().collect();
        out.push('…');
        out
    } else {
        let padding = width - chars.len();
        let mut out: String = chars.into_iter().collect();
        out.extend(std::iter::repeat(' ').take(padding));
        out
    }
}

/// Skip `offset` characters of `s`, then take/pad to exactly `width` —
/// the scrollback's equivalent of `panel_cell`'s span-based windowing,
/// simpler since scrollback lines carry no styling/tokenizing to
/// preserve (unlike the top panes' constraint text): just plain
/// character-level slicing.
fn windowed(s: &str, offset: usize, width: usize) -> String {
    let taken: String = s.chars().skip(offset).take(width).collect();
    let pad = width.saturating_sub(taken.chars().count());
    let mut out = taken;
    out.extend(std::iter::repeat_n(' ', pad));
    out
}

/// A border segment carrying a pane title: `─ Title ────`, truncated
/// outright if the pane is too narrow for it, or (`title: None`) no label
/// at all — just dashes — for a segment that exists purely to host zoom
/// buttons at its right edge, matching wherever the pane's own title
/// segment happens to render its label (`Output`'s label and its buttons
/// can land in different segments of the same header row — see the
/// callers in `draw`). The focused pane's title renders in reverse
/// video — styling applied only after the width math, so the zero-width
/// escapes can't skew the border.
///
/// `levels` are the zoom buttons this segment offers, left to right —
/// empty for none, `&[Wide, Full]` for every zoomable pane (see
/// `layout::zoom_levels`); every caller passes its own levels whether or
/// not the pane is *currently* zoomed. When non-empty and the segment's
/// wide enough to fit them (see `layout::button_offsets`), `active` marks
/// which button (if any) reflects this pane's *current* zoom state — that
/// one renders in reverse video the same way a focused title does,
/// doubling as a status indicator, not just a control. Clicking is
/// handled entirely by `Layout::header_button_at`/`App::toggle_zoom`, not
/// here; this only draws.
fn title_segment(
    title: Option<&str>,
    width: usize,
    focused: bool,
    levels: &[Zoom],
    active: Option<Zoom>,
    theme: Theme,
) -> String {
    if width == 0 {
        return String::new();
    }
    let show_buttons = layout::button_offsets(width as u16, levels).is_some();
    let button_zone = if show_buttons {
        layout::button_zone(levels) as usize
    } else {
        0
    };
    let rest_width = width - button_zone;

    let mut out = match title {
        Some(title) => {
            let label: String = {
                let text = format!(" {title} ");
                let chars: Vec<char> = text.chars().collect();
                let avail = rest_width.saturating_sub(1);
                if chars.len() > avail {
                    chars[..avail].iter().collect()
                } else {
                    text
                }
            };
            let used = 1 + label.chars().count();
            let styled_label = if focused {
                label.with(theme.fg).on(theme.bg).reverse().to_string()
            } else {
                base(&label, theme)
            };
            let trailing: String = std::iter::repeat('─')
                .take(rest_width.saturating_sub(used))
                .collect();
            format!(
                "{}{styled_label}{}",
                chrome("─", theme),
                chrome(&trailing, theme)
            )
        }
        None => chrome(&"─".repeat(rest_width), theme),
    };
    if show_buttons {
        out.push_str(&chrome(" ", theme));
        for &level in levels {
            let glyph = match level {
                Zoom::Wide => '▭',
                Zoom::Full => '⛶',
            };
            out.push_str(&zoom_button(glyph, active == Some(level), theme));
        }
    }
    out
}

/// One `[X]` zoom button: reverse video (like a focused title) when
/// `active` — this pane's current zoom level matches this specific
/// button — dimmed chrome like the rest of the border otherwise.
fn zoom_button(glyph: char, active: bool, theme: Theme) -> String {
    let text = format!("[{glyph}]");
    if active {
        text.with(theme.fg).on(theme.bg).reverse().to_string()
    } else {
        chrome(&text, theme)
    }
}
