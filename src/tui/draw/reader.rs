//! Draws the reader (see `tui::reader`) over the finished frame: a box
//! across the top area, its text wrapped at spaces to the box's width, and
//! records where each link landed in `App::reader_frame`.

use std::io::Write;

use crossterm::style::Stylize;
use crossterm::{cursor, queue, style::Print};

use super::{Span, TokenStyle, base, chrome, scrollback, style_span, tokenize_proof_line, vbar};
use crate::teach::markup::Inline;
use crate::tui::layout::Layout;
use crate::tui::reader::{Kind, Line, ReaderFrame};
use crate::tui::{App, theme::Theme};
use crate::varnames::VarNames;

/// How one character of the reader's text is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Look {
    Style(TokenStyle),
    /// Constraints and rule text, coloured as the Proof pane does.
    Code,
    /// Part of the link with this index in the view.
    Link(usize),
}

/// A wrapped row's characters.
type Row = Vec<(char, Look)>;

/// The most columns the box takes, borders included.
const MAX_WIDTH: u16 = 104;

/// With fewer top-area rows than this, the box takes the whole screen
/// above the prompt instead.
const MIN_TOP_ROWS: u16 = 10;

/// `line`'s characters with their looks; links are numbered on from
/// `next_link`.
fn cells(line: &Line, vars: Option<&VarNames>, next_link: &mut usize) -> Row {
    let text = match line.kind {
        Kind::Heading => TokenStyle::Heading,
        Kind::Note => TokenStyle::Dim,
        Kind::Text | Kind::Output => TokenStyle::Plain,
    };
    let mut cells: Row = line.lead.chars().map(|c| (c, Look::Style(text))).collect();
    if line.kind == Kind::Output {
        for span in scrollback::tokenize(&line.text, vars) {
            cells.extend(span.text.chars().map(|c| (c, Look::Style(span.style))));
        }
        return cells;
    }
    for inline in line.inlines() {
        let (run, look) = match inline {
            Inline::Text(run) => (run, Look::Style(text)),
            Inline::Code(run) => (run, Look::Code),
            Inline::Term { text, .. } | Inline::Constraint { text, .. } => {
                *next_link += 1;
                (text, Look::Link(*next_link - 1))
            }
        };
        cells.extend(run.chars().map(|c| (c, look)));
    }
    cells
}

/// Wraps `cells` to `width` columns, breaking at spaces where it can;
/// rows after the first are indented by `indent` columns.
fn wrap(cells: &[(char, Look)], indent: usize, width: usize) -> Vec<Row> {
    let indent = if indent * 2 > width { 0 } else { indent };
    let mut rows = Vec::new();
    let mut start = 0;
    loop {
        let avail = if rows.is_empty() {
            width
        } else {
            width - indent
        };
        let mut row: Row = if rows.is_empty() {
            Vec::new()
        } else {
            vec![(' ', Look::Style(TokenStyle::Plain)); indent]
        };
        if cells.len() - start <= avail {
            row.extend_from_slice(&cells[start..]);
            rows.push(row);
            return rows;
        }
        // The last space that leaves the row within `avail`, else a hard
        // break.
        let end = (start + 1..=start + avail)
            .rev()
            .find(|&i| cells[i].0 == ' ')
            .unwrap_or(start + avail);
        row.extend_from_slice(&cells[start..end]);
        rows.push(row);
        start = end;
        while cells.get(start).is_some_and(|(c, _)| *c == ' ') {
            start += 1;
        }
        if start == cells.len() {
            return rows;
        }
    }
}

/// The view's lines wrapped to `width`, and the first row of each link.
fn wrap_lines(lines: &[Line], vars: Option<&VarNames>, width: usize) -> (Vec<Row>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut link_rows = Vec::new();
    let mut next_link = 0;
    for line in lines {
        let cells = cells(line, vars, &mut next_link);
        let indent = match line.kind {
            Kind::Output => line.text.len() - line.text.trim_start().len() + 2,
            _ => line.lead.chars().count(),
        };
        for row in wrap(&cells, indent, width) {
            for (_, look) in &row {
                if let Look::Link(i) = look
                    && link_rows.len() == *i
                {
                    link_rows.push(rows.len());
                }
            }
            rows.push(row);
        }
    }
    (rows, link_rows)
}

/// One row drawn at column `x` of terminal row `y`, padded to `width`;
/// adds its link pieces to `hits`.
#[allow(clippy::too_many_arguments)]
fn render_row(
    row: &[(char, Look)],
    width: usize,
    x: u16,
    y: u16,
    focus: Option<usize>,
    vars: Option<&VarNames>,
    theme: Theme,
    hits: &mut Vec<(u16, std::ops::Range<u16>, usize)>,
) -> String {
    let mut out = String::new();
    let mut column = x;
    let mut i = 0;
    while i < row.len() {
        let look = row[i].1;
        let end = (i..row.len())
            .find(|&j| row[j].1 != look)
            .unwrap_or(row.len());
        let text: String = row[i..end].iter().map(|(c, _)| c).collect();
        let len = (end - i) as u16;
        match look {
            Look::Style(style) => out.push_str(&style_span(&Span { text, style }, theme)),
            Look::Code => match vars {
                Some(vars) => {
                    for span in tokenize_proof_line(&text, vars) {
                        out.push_str(&style_span(&span, theme));
                    }
                }
                None => out.push_str(&base(&text, theme)),
            },
            Look::Link(index) => {
                let styled = text.with(theme.cursor_accent).on(theme.bg).underlined();
                let styled = if focus == Some(index) {
                    styled.reverse()
                } else {
                    styled
                };
                out.push_str(&styled.to_string());
                hits.push((y, column..column + len, index));
            }
        }
        column += len;
        i = end;
    }
    out.push_str(&base(&" ".repeat(width.saturating_sub(row.len())), theme));
    out
}

/// The top border: the page's title, then its views with the current one
/// in reverse video, in `inner` columns between the corners.
fn title_row(title: &str, views: &[&str], current: usize, inner: usize, theme: Theme) -> String {
    let views_w: usize = if views.len() > 1 {
        views.iter().map(|v| v.chars().count() + 2).sum::<usize>() + views.len() + 1
    } else {
        0
    };
    let title_room = inner.saturating_sub(views_w + 3);
    let title: String = title.chars().take(title_room.saturating_sub(2)).collect();
    let mut out = chrome("─", theme);
    let mut used = 1;
    if !title.is_empty() {
        out.push_str(&style_span(
            &Span {
                text: format!(" {title} "),
                style: TokenStyle::Heading,
            },
            theme,
        ));
        used += title.chars().count() + 2;
    }
    let fill = inner.saturating_sub(used + views_w);
    out.push_str(&chrome(&"─".repeat(fill), theme));
    if views.len() > 1 && used + views_w <= inner {
        for (i, view) in views.iter().enumerate() {
            out.push_str(&chrome(if i == 0 { "┤" } else { "│" }, theme));
            let text = format!(" {view} ");
            let styled = text.with(theme.fg).on(theme.bg);
            out.push_str(&if i == current {
                styled.reverse().to_string()
            } else {
                styled.to_string()
            });
        }
        out.push_str(&chrome("├", theme));
    }
    out
}

/// Draws the open reader over the frame, if there is one, and records
/// its geometry.
pub(in crate::tui) fn draw(
    w: &mut impl Write,
    app: &mut App,
    layout: &Layout,
    width: u16,
    theme: Theme,
) -> anyhow::Result<()> {
    let Some(reader) = app.reader.as_mut() else {
        app.reader_frame = None;
        return Ok(());
    };
    let vars = app.session.as_ref().map(|s| &s.variables);
    let box_w = width.min(MAX_WIDTH);
    let box_h = if layout.top_h >= MIN_TOP_ROWS {
        layout.top_h + 2
    } else {
        layout.prompt_row
    };
    if box_w < 24 || box_h < 4 {
        app.reader_frame = None;
        return Ok(());
    }
    let x0 = (width - box_w) / 2;
    let inner = usize::from(box_w) - 2;
    let text_w = inner - 2;
    let content_h = usize::from(box_h) - 2;

    let page = reader.pages.last_mut().expect("a reader has a page");
    let (rows, link_rows) = wrap_lines(&page.views[page.view].lines, vars, text_w);
    if page.reveal
        && let Some(row) = page.focus.and_then(|focus| link_rows.get(focus))
    {
        if *row < page.scroll {
            page.scroll = *row;
        } else if *row >= page.scroll + content_h {
            page.scroll = row + 1 - content_h;
        }
    }
    page.reveal = false;
    page.scroll = page.scroll.min(rows.len().saturating_sub(content_h));

    let views: Vec<&str> = page.views.iter().map(|v| v.name).collect();
    let bar = chrome("│", theme);
    let thumb = vbar(page.scroll, rows.len(), content_h);
    let mut hits = Vec::new();
    let mut lines = vec![format!(
        "{}{}{}",
        chrome("┌", theme),
        title_row(&page.title, &views, page.view, inner, theme),
        chrome("┐", theme)
    )];
    for i in 0..content_h {
        let y = 1 + i as u16;
        let text = match rows.get(page.scroll + i) {
            Some(row) => render_row(row, text_w, x0 + 2, y, page.focus, vars, theme, &mut hits),
            None => base(&" ".repeat(text_w), theme),
        };
        let track = match &thumb {
            Some(range) if range.contains(&i) => "┃".with(theme.fg).on(theme.bg).to_string(),
            Some(_) => chrome("│", theme),
            None => base(" ", theme),
        };
        lines.push(format!("{bar}{}{text}{track}{bar}", base(" ", theme)));
    }
    lines.push(format!(
        "{}{}{}",
        chrome("└", theme),
        chrome(&"─".repeat(inner), theme),
        chrome("┘", theme)
    ));
    for (y, line) in lines.iter().enumerate() {
        queue!(w, cursor::MoveTo(x0, y as u16), Print(line))?;
    }
    app.reader_frame = Some(ReaderFrame {
        x: x0..x0 + box_w,
        y: 0..box_h,
        content_h,
        hits,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(lead: &str, text: &str) -> Line {
        Line {
            lead: lead.to_string(),
            text: text.to_string(),
            kind: Kind::Text,
        }
    }

    fn text(row: &Row) -> String {
        row.iter().map(|(c, _)| c).collect()
    }

    #[test]
    fn lines_wrap_at_spaces_with_a_hanging_indent() {
        let lines = [line("1. ", "one two three four five")];
        let (rows, _) = wrap_lines(&lines, None, 12);
        let rows: Vec<String> = rows.iter().map(text).collect();
        assert_eq!(rows, ["1. one two", "   three", "   four five"]);
        // A word longer than the row is broken where it must be.
        let (rows, _) = rows_of("abcdefghij", 4);
        assert_eq!(rows, ["abcd", "efgh", "ij"]);
        // An empty line is one empty row.
        assert_eq!(rows_of("", 10).0, [""]);
    }

    fn rows_of(text: &str, width: usize) -> (Vec<String>, Vec<usize>) {
        let (rows, links) = wrap_lines(&[line("", text)], None, width);
        (rows.iter().map(self::text).collect(), links)
    }

    #[test]
    fn links_are_numbered_in_order_with_their_rows() {
        let lines = [
            line("", "See [slack](g:slack) and [constraint 3](c:3)."),
            line("", "Then [a long link text](g:rup)."),
        ];
        let (rows, link_rows) = wrap_lines(&lines, None, 20);
        let looks: Vec<Look> = rows
            .iter()
            .flatten()
            .filter_map(|(_, look)| matches!(look, Look::Link(_)).then_some(*look))
            .collect();
        assert_eq!(looks.first(), Some(&Look::Link(0)));
        assert_eq!(looks.last(), Some(&Look::Link(2)));
        assert_eq!(text(&rows[0]), "See slack and");
        assert_eq!(link_rows, [0, 1, 2]);
        // At any width the links keep their numbers: Tab order doesn't
        // depend on wrapping.
        let (_, wide) = wrap_lines(&lines, None, 200);
        assert_eq!(wide, [0, 0, 1]);
    }

    #[test]
    fn code_drops_its_backticks() {
        let (rows, _) = rows_of("Take `@c1`: `x1 ≥ 1`.", 40);
        assert_eq!(rows, ["Take @c1: x1 ≥ 1."]);
    }
}
