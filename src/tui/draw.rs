//! Rendering: turns the current `App` state into one full frame and queues
//! it to the terminal. Every draw rewrites every cell (no diffing) at the
//! current terminal size.
//!
//! Rules:
//! - Character counts are display columns (assumes no wide glyphs).
//!   Styling is applied only after text is fitted to its width, so escape
//!   codes never enter width math.
//! - Scrollbars sit inside their pane (last column / last row), appear only
//!   on overflow, and are the only overflow indicator; content is clipped.
//! - Highlighting is limited to variable names and `@labels`. Formula and
//!   database constraints are tokenized by layout (`tokenize_constraint`);
//!   raw proof-rule text by name lookup (`tokenize_proof_line`). The
//!   scrollback adds structural roles (headings, dim, error/ok) in
//!   `scrollback`.
//! - Row backgrounds (cursor, assertion, core/derived, hint, error) are
//!   chosen only in `panel_cell` and applied via `style_span_on_bg`.
//! - Every colour comes from `theme::Theme`. Plain text goes through `base`
//!   and borders/scrollbar chrome through `chrome`; nothing is printed
//!   with the terminal's default colours.

use std::io::Write;
use std::ops::Range;

use crossterm::{cursor, queue, style::Color, style::Print, style::Stylize, terminal};

use crate::checker::Database;
use crate::commands::edit;
use crate::session::Session;
use crate::tui::layout::{self, Layout};
use crate::tui::{App, Pane, Zoom, theme::Theme};
use crate::varnames::VarNames;

mod scrollback;

const NORMAL_PROMPT: &str = "pbp> ";
const FORMULA_PROMPT: &str = "opb> ";
const DEBUG_PROMPT: &str = "debug> ";

/// Fixed height of the suggestion strip whenever it's visible (the layout
/// may grant fewer rows on a small terminal). All-or-nothing so the
/// prompt only ever sits at two positions: strip hidden, or strip shown.
const SUGGESTION_ROWS: usize = 6;

/// The colour role of a run of text. The top panes use only
/// `Plain`/`Variable`/`Label`; the rest are scrollback roles (see
/// `scrollback::tokenize`).
#[derive(Clone, Copy, PartialEq, Debug)]
enum TokenStyle {
    Plain,
    Variable,
    Label,
    /// A section header (`line 3:`, `Line 3 needed:`): bold `fg`.
    Heading,
    /// Context rather than content (`ConstraintId 3:`, the veripb banner).
    Dim,
    /// A lone symbol worth picking out (`~` as a hint).
    Accent,
    Error,
    Ok,
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

fn styled(text: &str, style: TokenStyle) -> Span {
    Span {
        text: text.to_string(),
        style,
    }
}

/// The foreground `style` draws in, and whether it's bold.
fn style_fg(style: TokenStyle, theme: Theme) -> (Color, bool) {
    match style {
        TokenStyle::Plain => (theme.fg, false),
        TokenStyle::Variable => (theme.variable, false),
        TokenStyle::Label => (theme.label, false),
        TokenStyle::Heading => (theme.fg, true),
        TokenStyle::Dim => (theme.dim, false),
        TokenStyle::Accent => (theme.cursor_accent, false),
        TokenStyle::Error => (theme.error_fg, false),
        TokenStyle::Ok => (theme.ok_fg, false),
    }
}

/// `text` in `theme.fg` on `theme.bg`: the style for all text that is not
/// an accent, a row highlight, or chrome.
fn base(text: &str, theme: Theme) -> String {
    text.with(theme.fg).on(theme.bg).to_string()
}

/// `text` in `theme.dim` on `theme.bg`: borders, scrollbar tracks and
/// other non-content chrome.
fn chrome(text: &str, theme: Theme) -> String {
    text.with(theme.dim).on(theme.bg).to_string()
}

/// `span` styled on `theme.bg`, ANSI embedded. Call only on text already
/// sliced to its final width; slicing styled text corrupts the escapes.
fn style_span(span: &Span, theme: Theme) -> String {
    style_span_on_bg(span, theme.bg, theme)
}

/// `span`'s foreground style on background `bg`. Used by every row-level
/// highlight.
fn style_span_on_bg(span: &Span, bg: Color, theme: Theme) -> String {
    let (fg, bold) = style_fg(span.style, theme);
    let text = span.text.clone().with(fg).on(bg);
    if bold { text.bold() } else { text }.to_string()
}

fn spans_char_len(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.text.chars().count()).sum()
}

/// Drops the first `n` characters across `spans`, splitting the span that
/// straddles the boundary and keeping styles. Used for horizontal scroll.
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

/// Keeps only the first `n` characters across `spans`, splitting the span
/// that straddles the boundary.
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

/// One display line of a top panel. Styling priority (resolved in
/// `panel_cell`): `error` > `debug_current` > `dim` > `cursor` >
/// `assertion` > `hint` > `core`.
#[derive(Clone)]
struct PanelLine {
    /// Line numbering, pinned at the left edge under horizontal scroll;
    /// drawn in the base colours (or the row background).
    prefix: String,
    /// Content, shifted by horizontal scroll.
    text: Vec<Span>,
    /// Render all text in `theme.dim`, ignoring span styles (preamble,
    /// placeholders).
    dim: bool,
    /// Under the Proof pane's Vim cursor or the Formula pane's browse cursor.
    cursor: bool,
    /// An `a`-rule (unchecked assertion) in the Proof pane.
    assertion: bool,
    /// Database pane only: `Some(true)` core, `Some(false)` derived.
    core: Option<bool>,
    /// Database pane only: during `:deassert`, a live constraint sharing a
    /// variable with the assertion being replaced (see `database_lines`).
    /// Drawn on `cursor_bg`.
    hint: bool,
    /// The buffer line `:verify` most recently rejected
    /// (`Session::known_bad`).
    error: bool,
    /// The Proof-pane line `:debug` is stopped at; drawn like `cursor`.
    debug_current: bool,
    /// Colour for the first `prefix` char (the breakpoint marker) only; see
    /// `render_prefix`. `None` outside the Proof pane's buffer/preamble.
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

/// A top pane's content. Only the rows on screen are built, so drawing a
/// pane costs the same however long its content is.
struct PaneContent<'a> {
    /// Content lines, on screen or not.
    total: usize,
    /// Width of the widest line, prefix included.
    longest: usize,
    /// Builds line `i` (`< total`).
    line: Box<dyn Fn(usize) -> PanelLine + 'a>,
}

impl PaneContent<'_> {
    /// Content held whole: placeholders and error lines.
    fn whole(lines: Vec<PanelLine>) -> Self {
        PaneContent {
            total: lines.len(),
            longest: longest_line(&lines),
            line: Box::new(move |i| lines[i].clone()),
        }
    }
}

/// A top pane ready to render row by row: visible content plus scrollbar
/// geometry. Scrollbars take space from the pane itself (last column /
/// last row).
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

/// Resolves one top pane: clamps both offsets (written back), builds the
/// visible lines, and decides both scrollbars. The bars' mutual dependency
/// is broken by assuming the vertical bar from the raw height, deciding the
/// horizontal bar against the reduced width, then finalizing.
fn pane_view(
    content: PaneContent,
    w: usize,
    h: usize,
    v_offset: &mut usize,
    h_offset: &mut usize,
) -> PaneView {
    let PaneContent {
        total,
        longest,
        line,
    } = content;
    let vbar_expected = total > h;
    // Pre-pass width (vertical bar assumed from raw height); the clamp and
    // the horizontal-bar decision must both use it so they agree.
    let avail_w = w.saturating_sub(vbar_expected as usize);
    *h_offset = clamp_hscroll(*h_offset, longest, avail_w);

    let hbar_on = longest > avail_w;
    let content_h = if hbar_on { h - 1 } else { h };

    let window = window_range(total, content_h, v_offset);
    let vbar = vbar(window.start, total, content_h);
    let eff_w = w - (vbar.is_some() as usize);
    let hbar = hbar_on.then(|| hbar_range(eff_w, *h_offset, longest));

    PaneView {
        lines: window.map(line).collect(),
        vbar,
        hbar,
        eff_w,
        content_h,
        h_offset: *h_offset,
    }
}

/// Row `i` of the pane at exactly the pane's width: a content row plus its
/// vertical-bar cell, or, past `content_h`, the horizontal bar.
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
            // Corner under the vertical track.
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
    // Every frame, so scrolls and resizes also update the hover (computed
    // against the previous frame's layout).
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
    // Full inner width: the Output pane's, and a solo-zoomed top pane's.
    let inner = (width - 2) as usize;
    let top_h = layout.top_h as usize;
    // `Some` only when a top pane is solo-zoomed; `None` when nothing or
    // `Output` is zoomed. See `Layout::zoomed_top_pane`.
    let zoomed_pane = layout.zoomed_top_pane().map(|(pane, _)| pane);
    let output_zoom = match layout.zoomed {
        Some((Pane::Output, level)) => Some(level),
        _ => None,
    };
    // `Output` full-maximised: `top_h == 0` and no top `PaneView` is built
    // (`pane_view` underflows on zero height).
    let output_fullscreen = layout.output_fullscreen();

    // Which top panes are shown: a solo-zoomed one gets `inner` width and
    // the other two aren't built; none are when `output_fullscreen`.
    let show_top = !output_fullscreen;
    let shown = |pane| show_top && (zoomed_pane.is_none() || zoomed_pane == Some(pane));
    let pane_w = |pane, w| if zoomed_pane == Some(pane) { inner } else { w };

    // The assertion `:deassert` is replacing, if any; named in the Output
    // and Database headings and drives the Database hint highlighting.
    let deassert_target = app.vim_deassert_source().map(str::to_string);
    // Names the assertion being replaced while `:deassert` is active.
    let output_title = match &deassert_target {
        Some(source) => format!("Output (deasserting {source})"),
        None => "Output".to_string(),
    };

    // Panel content and titles. Only the rows on screen are built (see
    // `PaneContent`).
    let formula_title = match &app.session {
        Some(session) => format!(
            "Formula: {} ({})",
            file_name(&session.formula_path),
            session.formula_constraint_count()
        ),
        None => "Formula".to_string(),
    };
    let formula_cursor = app.formula_browse_cursor();
    let formula = shown(Pane::Formula).then(|| {
        let content = match &app.session {
            Some(session) => formula_content(session, formula_cursor, &mut app.pane_widths),
            None => PaneContent::whole(vec![pinned("(no formula loaded)".to_string())]),
        };
        pane_view(
            content,
            pane_w(Pane::Formula, w1),
            top_h,
            &mut app.formula_scroll,
            &mut app.formula_hscroll,
        )
    });

    // Variables of the assertion being replaced; see `PanelLine::hint`.
    let hint_vars = match (&app.session, &deassert_target) {
        (Some(session), Some(source)) => session.variables.mentioned(source),
        _ => Vec::new(),
    };
    let debug_ids = app.debug_current_constraint_ids().to_vec();
    let database_shown = shown(Pane::Database);
    let database_db = match &app.session {
        Some(session) if database_shown => {
            let began = app.begin_activity("Updating the Database pane");
            let database = session.database();
            if began {
                app.end_activity();
            }
            Some(database)
        }
        _ => None,
    };
    let (database_total, database) = match (&app.session, &database_db) {
        (Some(session), Some(db)) => {
            let content =
                database_content(session, db, &hint_vars, &debug_ids, &mut app.pane_widths);
            let total = content.total;
            let view = pane_view(
                content,
                pane_w(Pane::Database, w2),
                top_h,
                &mut app.database_scroll,
                &mut app.database_hscroll,
            );
            (total, Some(view))
        }
        _ => (
            0,
            database_shown.then(|| {
                pane_view(
                    PaneContent::whole(Vec::new()),
                    pane_w(Pane::Database, w2),
                    top_h,
                    &mut app.database_scroll,
                    &mut app.database_hscroll,
                )
            }),
        ),
    };
    let database_title = match &deassert_target {
        // Same phrasing as the Output heading.
        Some(source) => format!("Database ({database_total}) (deasserting {source})"),
        None => format!("Database ({database_total})"),
    };

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
    let proof_w = pane_w(Pane::Proof, w3);
    // Adjust `app.proof_hscroll` to keep the Vim cursor column visible;
    // must run before `pane_view`, which clamps only to content width.
    if let Some((_, col)) = app.vim_cursor() {
        let num_w = app.session.as_ref().map_or(1, |s| {
            number_width(s.preamble_lines().len() + s.buffer.len())
        });
        let target_col = proof_prefix_width(num_w) + col;
        // Reserve a column for a possible vertical scrollbar.
        let avail_w = proof_w.saturating_sub(1);
        if target_col < app.proof_hscroll {
            app.proof_hscroll = target_col;
        } else if avail_w > 0 && target_col >= app.proof_hscroll + avail_w {
            app.proof_hscroll = target_col + 1 - avail_w;
        }
    }
    // In Vim `Insert` mode, the cursor line shows `App::editor`'s
    // uncommitted text; see `proof_content`.
    let vim_live_text: Option<(usize, String)> = app
        .vim_inserting()
        .then(|| app.vim_cursor_line().map(|line| (line, app.editor.text())))
        .flatten();
    let (proof_cursor, proof_hover, proof_debug_line) = (
        app.vim_cursor_line(),
        app.bp_hover(),
        app.debug_current_line(),
    );
    let proof = shown(Pane::Proof).then(|| {
        let content = match &app.session {
            Some(session) => proof_content(
                session,
                proof_cursor,
                vim_live_text.as_ref().map(|(l, t)| (*l, t.as_str())),
                proof_hover,
                proof_debug_line,
                theme,
            ),
            None => PaneContent::whole(Vec::new()),
        };
        pane_view(
            content,
            proof_w,
            top_h,
            &mut app.proof_scroll,
            &mut app.proof_hscroll,
        )
    });

    // Scrollback view: the tail scrolled up by `scroll_up`, with horizontal
    // offset and scrollbars resolved in the same order as `pane_view`.
    // Horizontal extent is the longest line in the whole scrollback.
    let sb_h = layout.scrollback_h as usize;
    let sb_total = app.scrollback.len();
    let sb_longest = app.scrollback.longest_chars();
    let sb_vbar_expected = sb_total > sb_h;
    let sb_avail_w = inner.saturating_sub(sb_vbar_expected as usize);
    app.output_hscroll = clamp_hscroll(app.output_hscroll, sb_longest, sb_avail_w);
    let sb_hbar_on = sb_longest > sb_avail_w;
    let sb_content_h = if sb_hbar_on { sb_h - 1 } else { sb_h };

    // Store this frame's geometry for the next events' hit-testing, paging
    // and cursor row math. `*_content_h` excludes a horizontal-bar row and
    // is `0` for a hidden pane.
    app.last_layout = Some(layout);
    app.proof_content_h = proof.as_ref().map_or(0, |v| v.content_h);
    app.formula_content_h = formula.as_ref().map_or(0, |v| v.content_h);
    app.database_content_h = database.as_ref().map_or(0, |v| v.content_h);
    app.scroll_up = app.scroll_up.min(sb_total.saturating_sub(sb_content_h));
    let sb_end = sb_total - app.scroll_up;
    let sb_start = sb_end.saturating_sub(sb_content_h);
    let visible: Vec<&String> = app.scrollback.range(sb_start..sb_end).collect();
    let output_vbar = vbar(sb_start, sb_total, sb_content_h);
    let sb_eff_w = inner - (output_vbar.is_some() as usize);
    let output_hbar = sb_hbar_on.then(|| hbar_range(sb_eff_w, app.output_hscroll, sb_longest));

    // Build every row of the frame, top to bottom. Box-drawing characters
    // always go through `chrome`.
    let mut rows: Vec<String> = Vec::with_capacity(height as usize);
    let bar = chrome("│", theme);
    if output_fullscreen {
        // No top area: Output's top border is the first row.
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
                // One full-width title segment. Its right-aligned buttons
                // must land where `Layout::header_button_at` expects
                // (`inner` ends at the same column as `cols[2]`).
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
        // The horizontal scrollbar, when present, is the last row.
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
                // Corner under the vertical track.
                row.push_str(&chrome("─", theme));
            }
            rows.push(format!("{bar}{row}{bar}"));
            continue;
        }
        let line = visible.get(i).map_or("", |line| line.as_str());
        let vars = app.session.as_ref().map(|s| &s.variables);
        let cell = scrollback_cell(line, vars, app.output_hscroll, sb_eff_w, theme);
        rows.push(match &output_vbar {
            Some(range) if range.contains(&i) => {
                format!("{bar}{cell}{}{bar}", "┃".with(theme.fg).on(theme.bg))
            }
            Some(_) => format!("{bar}{cell}{}{bar}", chrome("│", theme)),
            None => format!("{bar}{cell}{bar}"),
        });
    }
    // Prompt label indicates the mode: `debug>` / `opb>` (formula browse)
    // drawn in `theme.cursor_accent`, else `pbp>`. Vim mode replaces the
    // whole row with its `-- NORMAL --`/`-- INSERT --` status.
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
            "-- INSERT --  Esc commits the line · Enter splits it"
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
        app.omitted_candidates,
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
    // In Vim mode the terminal cursor is placed in the Proof pane at the
    // Vim (line, column); otherwise, or if the Proof pane is hidden, at
    // the prompt.
    let vim_pane_cursor = (|| {
        let (line, col) = app.vim_cursor()?;
        let proof_view = proof.as_ref()?;
        let x0 = layout.proof_content_x0() as usize;
        let num_w = app.session.as_ref().map_or(1, |s| {
            number_width(s.preamble_lines().len() + s.buffer.len())
        });
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

/// Draws `text` on the prompt row, in place of the prompt, while a command
/// runs (see `tui::Activity`). `width` is the row's width inside the
/// border.
pub(crate) fn progress_line(
    w: &mut impl Write,
    text: &str,
    row: u16,
    width: usize,
    theme: Theme,
) -> std::io::Result<()> {
    let styled = fit(text, width)
        .with(theme.cursor_accent)
        .on(theme.bg)
        .to_string();
    queue!(w, cursor::Hide, cursor::MoveTo(1, row), Print(styled))?;
    w.flush()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The vertical thumb: the rows (of `h`) it covers, or `None` when the
/// content fits. Length is proportional to the visible fraction, position
/// to `top`.
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

/// The horizontal thumb over `width` bar cells for `content` columns of
/// content at `offset`. Full-width if the content fits.
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

/// Width of the widest line, prefix included.
fn longest_line(lines: &[PanelLine]) -> usize {
    lines
        .iter()
        .map(|l| l.prefix.chars().count() + spans_char_len(&l.text))
        .max()
        .unwrap_or(0)
}

/// The suggestion strip: exactly `rows` `(text, is_selected)` entries,
/// blank-padded, each an aligned left column (usage or file name) plus
/// summary. With no selection an overflowing list ends in a
/// `… (+N more)` row, counting the `omitted` matches not in `candidates`
/// too; with a selection the window scrolls to keep it visible.
fn suggestion_rows(
    candidates: &[crate::tui::complete::Candidate],
    omitted: usize,
    selected: Option<usize>,
    rows: usize,
) -> Vec<(String, bool)> {
    if rows == 0 || candidates.is_empty() {
        return vec![(String::new(), false); rows];
    }
    let (start, shown, marker) = match selected {
        None if candidates.len() + omitted > rows => (0, rows - 1, true),
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
        out.push((
            format!(" … (+{} more)", candidates.len() + omitted - shown),
            false,
        ));
    }
    while out.len() < rows {
        out.push((String::new(), false));
    }
    out
}

/// `prefix` in `default_fg` on `bg`, except that its first character (the
/// breakpoint marker, see `breakpoint_marker`) uses `marker_color` when
/// set.
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

/// A whole row (prefix, already-clipped spans, padding) on background
/// `bg`, so the highlight fills the entire cell.
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

/// Line `i` of `lines` rendered at exactly `width` columns (blank if past
/// the end): pinned prefix, then text shifted by `h_offset`, clipped and
/// padded, then styled per [`PanelLine`]'s priority.
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
    // Truncate the prefix so the rest of the math can assume it fits.
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

/// Clamps `offset` to `longest - avail_w` (saturating), the offset at which
/// the longest line's right edge meets the pane's. Same bound as
/// `hbar_range`'s `max_off`.
fn clamp_hscroll(offset: usize, longest: usize, avail_w: usize) -> usize {
    offset.min(longest.saturating_sub(avail_w))
}

/// Digits needed to print `n`; the width line numbers are padded to.
fn number_width(n: usize) -> usize {
    n.to_string().len()
}

/// Width in columns of a Proof-pane line prefix: marker, space, `num_w`
/// digits, `": "`. Must match the prefix `proof_lines` builds; `draw`'s
/// Vim cursor placement and auto-scroll rely on it.
fn proof_prefix_width(num_w: usize) -> usize {
    2 + num_w + 2
}

/// The breakpoint-marker glyph and its colour override (see
/// `render_prefix`) for a Proof-pane line. `buffer_idx` is `None` for
/// preamble lines. Set breakpoint: `●` in `theme.breakpoint`; not set but
/// `hovered` (see `tui::App::proof_marker_at`): `●` in `theme.dim`;
/// otherwise a space with no override. Always one column wide.
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

/// Styled `"@label @other "` spans for constraint `id` (empty if it has no
/// labels), to prepend to the constraint's spans.
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

/// The width of `label_spans(labels_by_id, id)`.
fn labels_width(labels_by_id: &ahash::AHashMap<isize, Vec<String>>, id: isize) -> usize {
    labels_by_id.get(&id).map_or(0, |names| {
        names.iter().map(|name| name.chars().count() + 1).sum()
    })
}

/// The width of `tokenize_constraint(text)`.
fn constraint_width(text: &str) -> usize {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let terms_len = tokens.len().saturating_sub(2);
    if tokens.len() < 2 || !terms_len.is_multiple_of(2) {
        return text.chars().count();
    }
    tokens
        .iter()
        .map(|token| token.chars().count())
        .sum::<usize>()
        + tokens.len()
        - 1
}

/// Tokenizes a `ToPrettyString`-formatted constraint
/// (`"<coeff> <lit> ... >= <degree>"`, as emitted by `veripb-formula`):
/// variable names coloured, everything else plain. Returns one plain span
/// if the text doesn't match that shape. Collapses whitespace to single
/// spaces.
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
        // `terms_len` is even, so every chunk has exactly 2 elements.
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

/// Splits a literal (`"~name"` or `"name"`) into a plain `~` (if negated)
/// and a coloured variable name.
fn tokenize_literal(lit: &str) -> Vec<Span> {
    match lit.strip_prefix('~') {
        Some(var) => vec![plain("~".to_string()), variable(var.to_string())],
        None => vec![variable(lit.to_string())],
    }
}

/// Tokenizes a raw proof-rule line by lookup: a whitespace-separated token
/// starting with `@` is a label; one naming a variable in `var_names`
/// (after an optional `~`) is coloured; everything else is plain.
///
/// Output is char-for-char the same width as `line` (each whitespace char
/// becomes one space): the Vim cursor column (`App::vim_cursor`) indexes
/// into it, and tabs would break the pane border.
fn tokenize_proof_line(line: &str, var_names: &VarNames) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        // Whitespace run: one space per input char.
        let ws_end = rest
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len());
        if ws_end > 0 {
            spans.push(plain(" ".repeat(rest[..ws_end].chars().count())));
            rest = &rest[ws_end..];
            continue;
        }
        // Non-whitespace token; non-empty since `ws_end == 0`.
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
        if !name.is_empty() && var_names.contains(name) {
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

/// The top panes' widest lines, kept between frames and recomputed only
/// when the content changes. The Proof pane's comes from
/// `ProofBuffer::longest_chars` instead.
#[derive(Default)]
pub(crate) struct PaneWidths {
    /// For a `FormulaLines::epoch`: the ID column's width and the widest
    /// shown constraint text.
    formula: Option<(u64, usize, usize)>,
    /// For a `Session::state_key`: the widest Database line, prefix
    /// included.
    database: Option<((u64, u64), usize)>,
}

/// What the Formula pane shows of formula line `line`: its trailing `;`
/// (required when re-serialized to a temp .opb file) dropped.
fn formula_text(line: &str) -> &str {
    line.trim_end().strip_suffix(';').unwrap_or(line).trim_end()
}

/// Formula lines, each prefixed by its constraint ID (or ID range, for a
/// line that loads as two constraints), matching the Database pane. Labels
/// and variables are coloured by lookup, since formula lines may use
/// implication syntax. `cursor` (a 1-based line) marks `:formula` browse
/// mode's row.
fn formula_content<'a>(
    session: &'a Session,
    cursor: Option<usize>,
    widths: &mut PaneWidths,
) -> PaneContent<'a> {
    let epoch = session.formula.epoch();
    let (num_w, text_w) = match widths.formula {
        Some((cached, num_w, text_w)) if cached == epoch => (num_w, text_w),
        _ => {
            let num_w = (0..session.formula.len())
                .map(|idx| session.formula_line_ids(idx).len())
                .max()
                .unwrap_or(1);
            let text_w = session
                .formula
                .iter()
                .map(|line| formula_text(line).chars().count())
                .max()
                .unwrap_or(0);
            widths.formula = Some((epoch, num_w, text_w));
            (num_w, text_w)
        }
    };
    PaneContent {
        total: session.formula.len(),
        longest: if session.formula.is_empty() {
            0
        } else {
            num_w + 2 + text_w
        },
        line: Box::new(move |idx| {
            let spans =
                tokenize_proof_line(formula_text(&session.formula[idx]), &session.variables);
            let id = session.formula_line_ids(idx);
            let mut line = content(format!("{id:>num_w$}: "), spans, false);
            line.cursor = cursor == Some(idx + 1);
            line
        }),
    }
}

/// The live constraint database (as `:show` with no filters): core/derived
/// tagged, labels first. Rows mentioning any of `hint_vars` are marked
/// `hint`; rows whose ID is in `debug_ids` (produced by the last `:debug`
/// step) are marked `debug_current`. On error, one pinned error line.
fn database_content<'a>(
    session: &'a Session,
    database: &'a anyhow::Result<std::cell::Ref<'a, Database>>,
    hint_vars: &'a [String],
    debug_ids: &'a [usize],
    widths: &mut PaneWidths,
) -> PaneContent<'a> {
    let database = match database {
        Ok(database) => database,
        Err(err) => return PaneContent::whole(vec![pinned(format!("Error: {err:#}"))]),
    };
    let labels_by_id = session.labels_by_id();
    // Entries are in ascending ID order.
    let num_w = number_width(database.entries.last().map_or(0, |entry| entry.id));
    let key = session.state_key();
    let longest = match widths.database {
        Some((cached, longest)) if cached == key => longest,
        _ => {
            let longest = database
                .entries
                .iter()
                .map(|entry| {
                    num_w
                        + 2
                        + labels_width(&labels_by_id, entry.id as isize)
                        + constraint_width(&entry.text)
                })
                .max()
                .unwrap_or(0);
            widths.database = Some((key, longest));
            longest
        }
    };
    PaneContent {
        total: database.entries.len(),
        longest,
        line: Box::new(move |i| {
            let entry = &database.entries[i];
            let mut spans = label_spans(&labels_by_id, entry.id as isize);
            spans.extend(tokenize_constraint(&entry.text));
            let mut line = content(format!("{:>num_w$}: ", entry.id), spans, false);
            line.core = Some(entry.is_core);
            line.hint = !hint_vars.is_empty() && {
                let mentioned = session.variables.mentioned(&entry.text);
                mentioned.iter().any(|v| hint_vars.contains(v))
            };
            line.debug_current = debug_ids.contains(&entry.id);
            line
        }),
    }
}

/// The dimmed, untokenized preamble followed by the tokenized buffer lines,
/// numbered as one sequence matching the checker's `line N:` trace.
///
/// - `cursor`: Vim mode's display line (1-based), marked `cursor`.
/// - `editing`: `(display_line, live_text)` in Vim `Insert` mode; that
///   line shows `live_text`. Checked/error status still reflects the
///   committed buffer. The widest line counts both texts.
/// - `hover`: buffer index under the mouse in the marker column; affects
///   only `breakpoint_marker`.
/// - `debug_current`: buffer index `:debug` is stopped at; marked
///   `debug_current` and not dimmed.
///
/// Checked `a`-rule lines are marked `assertion`. Lines at or past
/// `session.checked_len` are dimmed, except the `session.known_bad` line
/// (always at `checked_len`), which is marked `error`.
fn proof_content<'a>(
    session: &'a Session,
    cursor: Option<usize>,
    editing: Option<(usize, &'a str)>,
    hover: Option<usize>,
    debug_current: Option<usize>,
    theme: Theme,
) -> PaneContent<'a> {
    let preamble = session.preamble_lines();
    let base = preamble.len();
    let total = base + session.buffer.len();
    let num_w = number_width(total);
    // `tokenize_proof_line` keeps every line's width.
    let text_w = preamble
        .iter()
        .map(|line| line.chars().count())
        .chain([session.buffer.longest_chars()])
        .chain(editing.map(|(_, live)| live.chars().count()))
        .max()
        .unwrap_or(0);
    PaneContent {
        total,
        longest: proof_prefix_width(num_w) + text_w,
        line: Box::new(move |i| {
            let mut line = if i < base {
                let (marker, marker_color) = breakpoint_marker(session, None, false, theme);
                let mut line = content(
                    format!("{marker} {:>num_w$}: ", i + 1),
                    vec![plain(preamble[i].clone())],
                    true,
                );
                line.marker_color = marker_color;
                line
            } else {
                let idx = i - base;
                let display_line = i + 1;
                let text = match editing {
                    Some((edit_line, live)) if edit_line == display_line => live,
                    _ => session.buffer[idx].as_str(),
                };
                let checked = idx < session.checked_len;
                let (marker, marker_color) =
                    breakpoint_marker(session, Some(idx), hover == Some(idx), theme);
                let mut line = content(
                    format!("{marker} {display_line:>num_w$}: "),
                    tokenize_proof_line(text, &session.variables),
                    !checked,
                );
                line.marker_color = marker_color;
                line.assertion = checked && edit::is_assertion_line(text);
                if !checked && idx == session.checked_len && session.known_bad.is_some() {
                    line.dim = false;
                    line.error = true;
                } else if debug_current == Some(idx) {
                    line.dim = false;
                    line.debug_current = true;
                }
                line
            };
            line.cursor = cursor == Some(i + 1);
            line
        }),
    }
}

/// The rows shown of `total` content lines in an `h`-row window starting
/// `offset` lines from the top (top-anchored, so appended content doesn't
/// move the view). Clamps `offset` to the content and writes it back.
fn window_range(total: usize, h: usize, offset: &mut usize) -> Range<usize> {
    if total <= h {
        *offset = 0;
        return 0..total;
    }
    *offset = (*offset).min(total - h);
    *offset..*offset + h
}

/// Pads or truncates `s` to exactly `width` columns, marking truncation
/// with a trailing `…`. For fields without scrollbars (prompt, suggestion
/// strip, status line).
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

/// One scrollback row: `line` tokenized (see `scrollback::tokenize`),
/// shifted by `offset` chars, clipped and padded to exactly `width`.
fn scrollback_cell(
    line: &str,
    vars: Option<&VarNames>,
    offset: usize,
    width: usize,
    theme: Theme,
) -> String {
    let spans = scrollback::tokenize(line, vars);
    let clipped = spans_take(&spans_skip(&spans, offset), width);
    let pad = width - spans_char_len(&clipped);
    let mut cell: String = clipped.iter().map(|span| style_span(span, theme)).collect();
    cell.push_str(&base(&" ".repeat(pad), theme));
    cell
}

/// A `width`-column border segment `─ Title ────` (title truncated to fit;
/// dashes only when `title` is `None`), reverse video when `focused`.
///
/// `levels` are the zoom buttons drawn right-aligned, if they fit (see
/// `layout::button_offsets`); the one equal to `active` (the pane's current
/// zoom) is drawn in reverse video. Clicks are handled by
/// `Layout::header_button_at`.
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

/// One `[X]` zoom button: reverse video when `active`, else chrome.
fn zoom_button(glyph: char, active: bool, theme: Theme) -> String {
    let text = format!("[{glyph}]");
    if active {
        text.with(theme.fg).on(theme.bg).reverse().to_string()
    } else {
        chrome(&text, theme)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;
    use crate::tui::theme::ThemeName;

    /// The pane as `pane_view` built it before `PaneContent`: every line,
    /// then the window.
    fn eager_pane_view(
        lines: Vec<PanelLine>,
        w: usize,
        h: usize,
        v_offset: &mut usize,
        h_offset: &mut usize,
    ) -> PaneView {
        let total = lines.len();
        let longest = longest_line(&lines);
        let avail_w = w.saturating_sub((total > h) as usize);
        *h_offset = clamp_hscroll(*h_offset, longest, avail_w);
        let hbar_on = longest > avail_w;
        let content_h = if hbar_on { h - 1 } else { h };
        let top = if total <= content_h {
            *v_offset = 0;
            0
        } else {
            *v_offset = (*v_offset).min(total - content_h);
            *v_offset
        };
        let vbar = vbar(top, total, content_h);
        let eff_w = w - (vbar.is_some() as usize);
        PaneView {
            lines: lines.into_iter().skip(top).take(content_h).collect(),
            vbar,
            hbar: hbar_on.then(|| hbar_range(eff_w, *h_offset, longest)),
            eff_w,
            content_h,
            h_offset: *h_offset,
        }
    }

    fn rows(view: &PaneView, h: usize) -> Vec<String> {
        let theme = ThemeName::default().theme();
        (0..h).map(|i| pane_row(view, i, theme)).collect()
    }

    /// Asserts that `content` draws as `all` (every one of its lines) would
    /// have, at a range of sizes and scroll offsets.
    fn assert_draws_like(
        content: impl Fn() -> PaneContent<'static>,
        all: impl Fn() -> Vec<PanelLine>,
    ) {
        assert_eq!(content().total, all().len());
        assert_eq!(content().longest, longest_line(&all()));
        for (w, h) in [(10, 3), (30, 5), (80, 20), (5, 40)] {
            for (v, hs) in [(0, 0), (2, 3), (7, 50), (1000, 1000)] {
                let (mut v1, mut h1, mut v2, mut h2) = (v, hs, v, hs);
                let lazy = pane_view(content(), w, h, &mut v1, &mut h1);
                let eager = eager_pane_view(all(), w, h, &mut v2, &mut h2);
                assert_eq!((v1, h1), (v2, h2), "offsets at {w}x{h} from ({v}, {hs})");
                assert_eq!(
                    rows(&lazy, h),
                    rows(&eager, h),
                    "rows at {w}x{h} from ({v}, {hs})"
                );
            }
        }
    }

    fn sample_lines(n: usize) -> Vec<PanelLine> {
        (0..n)
            .map(|i| {
                content(
                    format!("{i:>3}: "),
                    vec![plain("x".repeat(i * 7 % 40))],
                    i % 3 == 0,
                )
            })
            .collect()
    }

    #[test]
    fn a_window_draws_like_the_whole_content() {
        for n in [0, 1, 4, 25, 200] {
            assert_draws_like(|| PaneContent::whole(sample_lines(n)), || sample_lines(n));
        }
    }

    /// A session leaked for `'static` panes, with `proof` as its buffer.
    fn session(formula: &str, proof: &[&str]) -> &'static Session {
        let mut file = tempfile::NamedTempFile::with_suffix(".opb").unwrap();
        file.write_all(formula.as_bytes()).unwrap();
        let mut session = Session::load(file.path().to_str().unwrap()).unwrap();
        for line in proof {
            session.buffer.push(line.to_string());
        }
        session.checked_len = proof.len() / 2;
        Box::leak(Box::new(session))
    }

    const FORMULA: &str = "* comment\n@a 1 x1 1 x2 >= 1 ;\n1 x1 1 x2 = 1 ;\n1 ~x1 >= 1 ;\n";

    #[test]
    fn the_formula_pane_draws_like_every_line() {
        let session = session(FORMULA, &[]);
        let content = || formula_content(session, Some(2), &mut PaneWidths::default());
        let all = || {
            let c = content();
            (0..c.total).map(|i| (c.line)(i)).collect()
        };
        assert_draws_like(content, all);
        let lines = all();
        assert_eq!(lines[1].prefix, "2-3: ", "a two-constraint line");
        assert!(lines[1].cursor && !lines[0].cursor);
    }

    #[test]
    fn the_proof_pane_draws_like_every_line() {
        let proof = [
            "rup 1 x2 >= 1 ;",
            "@l a 1 ~x1 >= 1 ;",
            "pol 1 2 + ;",
            "rup >= 1 ;",
        ];
        let session = session(FORMULA, &proof);
        let theme = ThemeName::default().theme();
        for editing in [None, Some((4, "a much longer line typed in Insert mode ;"))] {
            let content = || proof_content(session, Some(5), editing, Some(1), None, theme);
            let all = || {
                let c = content();
                (0..c.total).map(|i| (c.line)(i)).collect()
            };
            assert_draws_like(content, all);
        }
        let c = proof_content(session, Some(5), None, None, None, theme);
        assert_eq!(c.total, 2 + proof.len());
        assert!((c.line)(4).cursor);
        assert!(!(c.line)(2).dim && (c.line)(4).dim, "checked vs unchecked");
    }

    #[test]
    fn the_proof_width_follows_edits() {
        let mut file = tempfile::NamedTempFile::with_suffix(".opb").unwrap();
        file.write_all(FORMULA.as_bytes()).unwrap();
        let mut session = Session::load(file.path().to_str().unwrap()).unwrap();
        for line in ["short ;", "the longest line of all ;", "mid line ;"] {
            session.buffer.push(line.to_string());
        }
        let theme = ThemeName::default().theme();
        let longest = |s: &Session| {
            let c = proof_content(s, None, None, None, None, theme);
            let all: Vec<PanelLine> = (0..c.total).map(|i| (c.line)(i)).collect();
            (c.longest, longest_line(&all))
        };
        let (cached, actual) = longest(&session);
        assert_eq!(cached, actual);
        session.delete_line(1).unwrap();
        let (cached, actual) = longest(&session);
        assert_eq!(cached, actual, "after deleting the longest line");
        session
            .set_line(0, "now this one is the longest line ;")
            .unwrap();
        let (cached, actual) = longest(&session);
        assert_eq!(cached, actual, "after lengthening a line");
    }

    #[test]
    fn constraint_widths_match_their_tokens() {
        for text in [
            "1 x1 1 ~x2 >= 1",
            ">= 1",
            "3 x1  2   x2 >= 4",
            "not a constraint",
            "",
            "1 x1 >= 1 extra",
        ] {
            assert_eq!(
                constraint_width(text),
                spans_char_len(&tokenize_constraint(text)),
                "{text:?}"
            );
        }
        let mut labels = ahash::AHashMap::new();
        labels.insert(3, vec!["@a".to_string(), "@longer".to_string()]);
        for id in [3, 4] {
            assert_eq!(
                labels_width(&labels, id),
                spans_char_len(&label_spans(&labels, id))
            );
        }
    }
}
