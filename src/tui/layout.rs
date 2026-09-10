//! Pure layout math for the fixed four-pane frame: three columns over a
//! full-width bottom pane, one cell of border everywhere. No drawing here
//! — `draw.rs` turns a `Layout` into rows of text, top to bottom, so all
//! it needs are the column widths and region heights, not full rects.
//!
//! ```text
//! ┌─ Formula ────┬─ Database ────┬─ Proof ─────┐   row 0
//! │              │               │             │   top_h rows
//! ├──────────────┴───────────────┴─────────────┤
//! │ scrollback                                 │   scrollback_h rows
//! │ pbp> _                                      │   prompt_row
//! │ (suggestions)                               │   suggestion_rows rows
//! └────────────────────────────────────────────┘   row height-1
//! ```
//!
//! A pane can also be *zoomed* (`Zoom::Wide`/`Zoom::Full` — see
//! `tui::Zoom`), taking over more of the frame than its normal share.
//! For one of the three top panes that means displacing the other two:
//! `Wide` takes over the whole top row in their place (`top_h` unchanged
//! from the three-column case), `Full` additionally shrinks the bottom
//! pane down to its small-terminal floor so the zoomed pane gets nearly
//! the entire screen. `Output` is the mirror image, both in *what* it
//! displaces and in degree: `Wide` (semi-maximise) shrinks the top area
//! down to *its* floor instead — all three columns stay visible, just
//! squeezed to a few lines — while `Full` (full-maximise) removes the top
//! area entirely, so Output alone fills the screen. Every zoomable header
//! carries two small buttons toggling this, `[▭]` then `[⛶]`, laid out by
//! `button_offsets` below and drawn by `draw::title_segment` at the exact
//! same columns this module hit-tests clicks against, so the two can
//! never disagree about where a button actually is. For the three top
//! panes the buttons sit in that pane's own header, at its right edge;
//! `Output`'s sit at the right edge of its own row too, which in the
//! ordinary three-column view means under the Proof column, not under
//! Formula where the row's title label happens to live.

use crate::tui::{Pane, Zoom};

pub const MIN_WIDTH: u16 = 40;
pub const MIN_HEIGHT: u16 = 12;

/// Smallest number of rows the three top panes keep when `Output` is
/// zoomed to `Wide` (semi-maximise) — the same floor the normal
/// three-column split's `clamp` already keeps them from shrinking past.
const TOP_FLOOR: u16 = 3;

/// Smallest number of rows the bottom pane keeps when a top pane is
/// zoomed to `Full` (3 scrollback + prompt).
const BOTTOM_FLOOR: u16 = 4;

/// Every pane offers both zoom levels — `Wide` then `Full`, left to
/// right on its header. `Output`'s levels mean something different from a
/// top pane's (see the module docs), but the two-buttons-in-that-order
/// shape is the same across all four panes.
pub(crate) fn zoom_levels(_pane: Pane) -> &'static [Zoom] {
    &[Zoom::Wide, Zoom::Full]
}

/// Total header columns `levels`' buttons occupy, gap included — e.g.
/// `" [▭][⛶]"` (7) for two, `" [⛶]"` (4) for one. `pub(crate)` so
/// `draw::title_segment` reserves exactly this much rather than a
/// separately-maintained magic number.
pub(crate) fn button_zone(levels: &[Zoom]) -> u16 {
    if levels.is_empty() {
        0
    } else {
        1 + 3 * levels.len() as u16
    }
}

/// Smallest pane width worth showing `levels`' buttons in at all — the
/// button zone itself plus a sliver of room for the title (`─ X ─`, 4
/// columns), so a very narrow pane doesn't render as all-buttons-no-title.
const TITLE_SLIVER: u16 = 4;

/// Where a pane's zoom buttons sit, as columns from that pane's own left
/// edge (0-based), one offset per entry in `levels` (same order), each 3
/// columns wide (`[X]`), flush against the pane's right edge. `None` when
/// `levels` is empty or `pane_width` is too narrow to show them (see
/// `TITLE_SLIVER`).
pub(crate) fn button_offsets(pane_width: u16, levels: &[Zoom]) -> Option<Vec<u16>> {
    if levels.is_empty() || pane_width < button_zone(levels) + TITLE_SLIVER {
        return None;
    }
    let n = levels.len() as u16;
    Some((0..n).map(|i| pane_width - 3 * (n - i)).collect())
}

/// The frame's dimensions for one draw, computed fresh from the terminal
/// size every frame (which is what makes resize handling free). The app
/// keeps a copy of the last one drawn, so mouse coordinates and page
/// sizes are interpreted against what's actually on screen.
#[derive(Clone, Copy)]
pub struct Layout {
    /// Widths of the three top columns (border cells excluded) — the
    /// three-column case's own geometry. Still shown, at `top_h` rows,
    /// whenever no *top* pane is solo-zoomed and `Output` isn't
    /// full-maximised — that includes `Output` being *semi*-maximised
    /// (`Wide`), which shrinks `top_h` but never hides the columns.
    /// Meaningless when `zoomed_top_pane` is `Some`, or when `Output` is
    /// full-maximised (`top_h` is `0` then — nothing to give a width to).
    pub cols: [u16; 3],
    /// Rows of content in the top area, zoomed or not — a solo-zoomed top
    /// pane just gets `single_width` columns of it instead of one of
    /// `cols`; an `Output`-semi-maximised (`Wide`) frame gives the (still
    /// three-column) top area its floor instead of its usual ~3/5 share;
    /// `Output` full-maximised (`Full`) drops this to `0` — no top area
    /// at all.
    pub top_h: u16,
    /// Rows of scrollback in the bottom pane, above the prompt.
    pub scrollback_h: u16,
    /// Terminal row of the `pbp>` prompt.
    pub prompt_row: u16,
    /// Rows of completion suggestions below the prompt — the requested
    /// count, possibly reduced to keep the scrollback usable.
    pub suggestion_rows: u16,
    /// `Some((pane, level))` when any pane is zoomed, top pane or
    /// `Output` alike. Carried through from what `App` asked `compute`
    /// for, so `draw` doesn't need `App` in scope to know which branch to
    /// take. Use [`Self::zoomed_top_pane`] when what matters is
    /// specifically whether a *top* pane has taken over the top row.
    pub zoomed: Option<(Pane, Zoom)>,
    /// A solo-zoomed pane's full content width (border columns excluded)
    /// — wider than any single entry in `cols`, since there are no
    /// internal column separators to give up space to. What a solo-zoomed
    /// top pane's own title row spans; what `Output`'s title row spans
    /// when a top pane is solo-zoomed above it, or when `Output` itself is
    /// full-maximised (see `draw`). Meaningless when nothing is zoomed at
    /// all.
    pub single_width: u16,
}

impl Layout {
    /// `None` when the terminal is too small to lay the frame out at all —
    /// the caller shows a plea to enlarge instead of a garbled frame.
    /// `suggestions` is how many suggestion rows the caller would like
    /// under the prompt; the layout grants what fits. `zoom` mirrors
    /// `App::zoom` — see the module docs for what it changes.
    pub fn compute(
        width: u16,
        height: u16,
        suggestions: u16,
        zoom: Option<(Pane, Zoom)>,
    ) -> Option<Layout> {
        if width < MIN_WIDTH || height < MIN_HEIGHT {
            return None;
        }

        // Normally three border rows (top, separator, bottom) with the
        // rest split ~3:2 between the columns and the bottom pane —
        // except a `Zoom::Full` top pane, which instead gives the bottom
        // pane its floor so the zoomed pane gets nearly everything else;
        // or `Output` semi-maximised (`Wide`), the mirror image: the top
        // area (still all three columns) drops to *its* floor instead; or
        // `Output` full-maximised (`Full`), which removes the top area
        // (and the border row that would separate it from Output)
        // entirely — just Output's own top border plus a bottom border
        // surround everything else, one fewer border row than every other
        // case. `MIN_HEIGHT` guarantees there's always room for whichever
        // floor applies, and the normal split's `clamp` bounds are always
        // valid.
        let (top_h, bottom_h) = match zoom {
            Some((Pane::Output, Zoom::Full)) => (0, height - 2),
            Some((Pane::Output, Zoom::Wide)) => (TOP_FLOOR, height - 3 - TOP_FLOOR),
            Some((_, Zoom::Full)) => {
                let content = height - 3;
                (content - BOTTOM_FLOOR, BOTTOM_FLOOR)
            }
            _ => {
                let content = height - 3;
                let bottom_h = (content * 2 / 5).clamp(BOTTOM_FLOOR, content - TOP_FLOOR);
                (content - bottom_h, bottom_h)
            }
        };

        // One border column each side plus two column separators.
        let inner = width - 4;
        let w1 = inner / 3;
        let w2 = inner / 3;
        let w3 = inner - w1 - w2;

        // Suggestions live inside the bottom pane, below the prompt, and
        // never squeeze the scrollback under 2 rows.
        let suggestion_rows = suggestions.min(bottom_h.saturating_sub(3));

        Some(Layout {
            cols: [w1, w2, w3],
            top_h,
            scrollback_h: bottom_h - 1 - suggestion_rows,
            prompt_row: height - 2 - suggestion_rows,
            suggestion_rows,
            zoomed: zoom,
            // Just the two border columns — no internal separators to
            // share with, unlike `cols`.
            single_width: width - 2,
        })
    }

    /// `zoomed`, but `None` whenever it's `Output` rather than one of the
    /// three top panes — `Output` being zoomed changes row heights (or, at
    /// `Full`, removes the top area outright), not which *of the three
    /// columns* is visible up top, so `pane_at`/the header-row drawing
    /// branch in `draw` both want "is a top pane taking over the row?"
    /// rather than "is anything zoomed at all?".
    pub(crate) fn zoomed_top_pane(&self) -> Option<(Pane, Zoom)> {
        match self.zoomed {
            Some((Pane::Output, _)) => None,
            other => other,
        }
    }

    /// The Proof pane's content area's leftmost column — `1` (right
    /// after the frame's own left border) when it's solo-zoomed and
    /// therefore spans the full terminal width, `3 + cols[0] + cols[1]`
    /// (past Formula's and Database's own border-plus-content) in the
    /// ordinary three-column case. The one shared source of truth for
    /// this — same reasoning as `proof_prefix_width` in `draw.rs` — so
    /// `draw::draw`'s real-cursor placement and `tui::App`'s mouse
    /// hit-testing (`proof_marker_at`, for the breakpoint gutter) can
    /// never disagree about which column the pane's content actually
    /// starts at.
    pub(crate) fn proof_content_x0(&self) -> u16 {
        if self.zoomed_top_pane().map(|(pane, _)| pane) == Some(Pane::Proof) {
            1
        } else {
            3 + self.cols[0] + self.cols[1]
        }
    }

    /// Whether `Output` is full-maximised — the one case with no top area
    /// at all (`top_h == 0`) and no separator row between it and the
    /// (nonexistent) top area: `Output`'s own top border is the very first
    /// row of the frame instead. `pane_at`/`header_button_at`/`draw` all
    /// need this specific case distinguished from "`Output` semi-maximised
    /// (`Wide`)", which still has a normal three-column top area (just
    /// shrunk to its floor) and an ordinary separator row above `Output`.
    pub(crate) fn output_fullscreen(&self) -> bool {
        matches!(self.zoomed, Some((Pane::Output, Zoom::Full)))
    }

    /// The header row `Output`'s own zoom buttons sit in: row `0` when
    /// [`Self::output_fullscreen`] (its top border doubles as the frame's,
    /// per the module docs), else the ordinary separator row at
    /// `top_h + 1`.
    fn output_header_row(&self) -> u16 {
        if self.output_fullscreen() {
            0
        } else {
            self.top_h + 1
        }
    }

    /// Which pane the terminal cell (`column`, `row`) belongs to — `None`
    /// for the top border and the column separators. The whole bottom
    /// region (separator bar, scrollback, prompt, suggestions) reads as
    /// Output: a generous hit box beats pixel-perfect borders for clicks.
    /// While a top pane is solo-zoomed, every content cell in the top row
    /// belongs to it — there's nothing else up there to hit — and the
    /// header row itself (row 0) is left to `header_button_at` instead,
    /// zoomed or not. `Output` semi-maximised (`Wide`) falls through to
    /// the ordinary three-column case below — the columns are still all
    /// there, just shorter; full-maximised (`Full`, `top_h == 0`) also
    /// falls through, but its empty `1..=0` row range never matches, so
    /// every row past the header (row 0, left to `header_button_at`) reads
    /// as Output, which is exactly right — nothing else is on screen.
    pub fn pane_at(&self, column: u16, row: u16) -> Option<Pane> {
        if let Some((pane, _)) = self.zoomed_top_pane() {
            if (1..=self.top_h).contains(&row) {
                return Some(pane);
            }
            return (row > self.top_h).then_some(Pane::Output);
        }
        let [w1, w2, _] = self.cols;
        if (1..=self.top_h).contains(&row) {
            if (1..=w1).contains(&column) {
                return Some(Pane::Formula);
            }
            if (2 + w1..2 + w1 + w2).contains(&column) {
                return Some(Pane::Database);
            }
            if column >= 3 + w1 + w2 {
                return Some(Pane::Proof);
            }
            return None;
        }
        if row > self.top_h {
            return Some(Pane::Output);
        }
        None
    }

    /// Which pane's zoom button (if any) sits at (`column`, `row`).
    /// `Output`'s live in its own header row ([`Self::output_header_row`])
    /// — at the row's right edge, same as every other pane's: `single_width`
    /// wide, starting right after the left border, when a top pane is
    /// solo-zoomed above it or `Output` is itself full-maximised (both
    /// full-width rows with nothing else sharing them); `cols[2]` wide,
    /// starting where the Proof column starts, in the ordinary
    /// three-column case — `draw.rs` renders `Output`'s title spanning
    /// that whole row, not split at the column boundaries above it, but
    /// its buttons still right-align against the row's own right edge,
    /// landing in this same `cols[2]`-wide zone; matches every other
    /// header's "buttons at the pane's own right edge" rule. A top pane's
    /// own header (`row == 0`, whichever top pane(s) are actually visible)
    /// is the other case, each only where the pane in question is wide
    /// enough to show its buttons at all (see `button_offsets`). The
    /// returned `Zoom` is which level that specific button toggles, not
    /// necessarily the pane's current one — `App` decides what a click
    /// actually does (see `App::toggle_zoom`).
    pub fn header_button_at(&self, column: u16, row: u16) -> Option<(Pane, Zoom)> {
        if row == self.output_header_row() {
            let (start, width) = if self.output_fullscreen() || self.zoomed_top_pane().is_some() {
                (1u16, self.single_width)
            } else {
                (3 + self.cols[0] + self.cols[1], self.cols[2])
            };
            let levels = zoom_levels(Pane::Output);
            let offsets = button_offsets(width, levels)?;
            let col = column.checked_sub(start)?;
            if col >= width {
                return None;
            }
            return Self::button_hit(Pane::Output, col, &offsets, levels);
        }
        if row != 0 {
            return None;
        }
        if let Some((pane, _)) = self.zoomed_top_pane() {
            let levels = zoom_levels(pane);
            let offsets = button_offsets(self.single_width, levels)?;
            let col = column.checked_sub(1)?;
            return Self::button_hit(pane, col, &offsets, levels);
        }
        let [w1, w2, w3] = self.cols;
        for (pane, start, width) in [
            (Pane::Formula, 1u16, w1),
            (Pane::Database, 2 + w1, w2),
            (Pane::Proof, 3 + w1 + w2, w3),
        ] {
            let levels = zoom_levels(pane);
            let Some(offsets) = button_offsets(width, levels) else {
                continue;
            };
            let Some(col) = column.checked_sub(start) else {
                continue;
            };
            if col >= width {
                continue;
            }
            if let Some(hit) = Self::button_hit(pane, col, &offsets, levels) {
                return Some(hit);
            }
        }
        None
    }

    /// Whether `col` (already relative to the pane's own left edge) lands
    /// on one of its 3-column-wide buttons — `offsets`/`levels` paired up
    /// index-for-index, as `button_offsets` returns them.
    fn button_hit(pane: Pane, col: u16, offsets: &[u16], levels: &[Zoom]) -> Option<(Pane, Zoom)> {
        offsets
            .iter()
            .zip(levels)
            .find(|&(&off, _)| (off..off + 3).contains(&col))
            .map(|(_, &level)| (pane, level))
    }
}
