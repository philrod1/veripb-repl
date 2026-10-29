//! Each pane's find, opened with `/` while the pane is focused or with its
//! header's `[/]`: the query typed goes to a line, or shows only the lines
//! it matches (see `crate::find` for what a query means). In the Output
//! pane every query shows the lines containing it, and a filter takes in
//! new output as it arrives. The pane filters as you type; Enter keeps the
//! result, Esc restores what was there before.

use crossterm::event::{KeyCode, KeyEvent};

use crate::checker::parse::leading_labels;
use crate::find::{Filter, Query, constraint_kind};
use crate::tui::input::EditResult;
use crate::tui::{App, Pane};

/// A filtered pane: the rows its query matches.
#[derive(Clone)]
pub(super) struct PaneFind {
    /// The query as typed.
    pub(super) text: String,
    filter: Filter,
    /// Database only: `Some(true)` for core constraints only,
    /// `Some(false)` for derived only (see `crate::find::constraint_kind`).
    core: Option<bool>,
    /// The rows shown, ascending, as indices into the pane's whole content
    /// (the Proof pane's count the preamble).
    pub(super) rows: Vec<usize>,
    /// The content `rows` were found in; see `App::find_content_key`.
    key: (u64, u64),
}

/// A find query being typed, in `App::find_editor`.
pub(super) struct FindInput {
    pub(super) pane: Pane,
    /// The pane's filter and scroll offset before, restored by Esc.
    before: (Option<PaneFind>, usize),
    /// The row at the top of the pane when the input opened. A filter
    /// shows its first match at or after it.
    origin: usize,
    /// Why the query does nothing, such as an invalid regex.
    pub(super) note: Option<String>,
}

/// `pane`'s index into `App::finds` and `App::marked`.
fn slot(pane: Pane) -> Option<usize> {
    Some(match pane {
        Pane::Formula => 0,
        Pane::Database => 1,
        Pane::Proof => 2,
        Pane::Output => 3,
    })
}

impl App {
    /// `pane`'s filter, if it has one.
    pub(super) fn find(&self, pane: Pane) -> Option<&PaneFind> {
        self.finds[slot(pane)?].as_ref()
    }

    /// Whether `pane` has a filter or a query being typed.
    pub(super) fn find_shown(&self, pane: Pane) -> bool {
        self.find(pane).is_some()
            || self
                .find_input
                .as_ref()
                .is_some_and(|input| input.pane == pane)
    }

    /// Whether `/`, Home and End act on the focused pane: nothing is typed
    /// and no edit is under way.
    pub(super) fn pane_keys_active(&self) -> bool {
        self.vim.is_none() && self.formula_browse.is_none() && self.editor.text().is_empty()
    }

    /// Whether a find can open: not while a pane is being edited.
    fn can_find(&self) -> bool {
        self.vim.is_none() && self.formula_browse.is_none()
    }

    fn scroll_of(&mut self, pane: Pane) -> &mut usize {
        match pane {
            Pane::Formula => &mut self.formula_scroll,
            Pane::Database => &mut self.database_scroll,
            Pane::Proof => &mut self.proof_scroll,
            Pane::Output => &mut self.scroll_up,
        }
    }

    fn content_h(&self, pane: Pane) -> usize {
        match pane {
            Pane::Formula => self.formula_content_h,
            Pane::Database => self.database_content_h,
            Pane::Proof => self.proof_content_h,
            Pane::Output => self.output_content_h,
        }
    }

    /// The rows `pane` has unfiltered.
    fn content_total(&self, pane: Pane) -> usize {
        if pane == Pane::Output {
            return self.scrollback.len();
        }
        let Some(session) = &self.session else {
            return 0;
        };
        match pane {
            Pane::Formula => session.formula.len(),
            Pane::Database => session.database().map_or(0, |d| d.entries.len()),
            Pane::Proof => session.display_line(session.buffer.len()) - 1,
            Pane::Output => 0,
        }
    }

    /// Changes whenever `pane`'s content does.
    fn find_content_key(&self, pane: Pane) -> (u64, u64) {
        if pane == Pane::Output {
            return (self.scrollback.pushed(), 0);
        }
        let Some(session) = &self.session else {
            return (0, 0);
        };
        match pane {
            Pane::Formula => (session.formula.epoch(), 0),
            Pane::Database => session.state_key(),
            Pane::Proof => (session.buffer.revision(), 0),
            Pane::Output => unreachable!("handled above"),
        }
    }

    /// The rows of `pane` that `filter` matches, out of `candidates` (all
    /// rows if `None`). Database rows match on their labels or their text,
    /// and must be core (or derived) if `core` says so.
    fn matching_rows(
        &self,
        pane: Pane,
        filter: &Filter,
        core: Option<bool>,
        candidates: Option<&[usize]>,
    ) -> Vec<usize> {
        // Asks `matches` about each row in ascending order.
        let select = |total: usize, matches: &mut dyn FnMut(usize) -> bool| -> Vec<usize> {
            match candidates {
                Some(rows) => rows.iter().copied().filter(|&row| matches(row)).collect(),
                None => (0..total).filter(|&row| matches(row)).collect(),
            }
        };
        if pane == Pane::Output {
            return select(self.scrollback.len(), &mut |row| {
                filter.matches(self.scrollback.get(row))
            });
        }
        let Some(session) = &self.session else {
            return Vec::new();
        };
        match pane {
            Pane::Formula => select(session.formula.len(), &mut |row| {
                filter.matches(&session.formula[row])
            }),
            Pane::Database => {
                let Ok(database) = session.database() else {
                    return Vec::new();
                };
                // The IDs with a matching label, ascending like the rows.
                let mut labelled: Vec<usize> = session
                    .labels_by_id()
                    .iter()
                    .filter(|(_, labels)| labels.iter().any(|label| filter.matches(label)))
                    .filter_map(|(&id, _)| usize::try_from(id).ok())
                    .collect();
                labelled.sort_unstable();
                let mut labelled = labelled.into_iter().peekable();
                select(database.entries.len(), &mut |row| {
                    let entry = &database.entries[row];
                    while labelled.next_if(|&id| id < entry.id).is_some() {}
                    core.is_none_or(|core| entry.is_core == core)
                        && (labelled.peek() == Some(&entry.id) || filter.matches(&entry.text))
                })
            }
            // The preamble never matches.
            Pane::Proof => {
                let base = session.display_line(0) - 1;
                select(base + session.buffer.len(), &mut |row| {
                    row >= base && filter.matches(&session.buffer[row - base])
                })
            }
            Pane::Output => unreachable!("handled above"),
        }
    }

    /// Finds `pane`'s filter's rows again if its content changed.
    pub(super) fn refresh_find(&mut self, pane: Pane) {
        let Some(slot) = slot(pane) else {
            return;
        };
        let key = self.find_content_key(pane);
        let Some(find) = &self.finds[slot] else {
            return;
        };
        if find.key == key {
            return;
        }
        let rows = self.matching_rows(pane, &find.filter, find.core, None);
        let find = self.finds[slot].as_mut().expect("checked above");
        find.rows = rows;
        find.key = key;
    }

    /// Filters `pane` to the rows `filter` matches, starting the view at
    /// the first one at or after row `origin`.
    fn show_matches(
        &mut self,
        pane: Pane,
        text: &str,
        filter: Filter,
        core: Option<bool>,
        origin: usize,
    ) {
        let Some(slot) = slot(pane) else {
            return;
        };
        let key = self.find_content_key(pane);
        let narrowing = self.finds[slot].take().filter(|find| {
            find.key == key
                && filter.narrows(&find.filter)
                && find.core.is_none_or(|kind| core == Some(kind))
        });
        let candidates = narrowing.as_ref().map(|find| &find.rows[..]);
        let rows = self.matching_rows(pane, &filter, core, candidates);
        let first = rows.partition_point(|&row| row < origin);
        self.finds[slot] = Some(PaneFind {
            text: text.to_string(),
            filter,
            core,
            rows,
            key,
        });
        self.scroll_to_offset(pane, first);
        self.marked[slot].clear();
    }

    /// How many rows `pane` shows: its filter's, or all of them.
    fn shown_total(&self, pane: Pane) -> usize {
        match self.find(pane) {
            Some(find) => find.rows.len(),
            None => self.content_total(pane),
        }
    }

    /// Which of the rows `pane` shows is at its top. The Output pane's
    /// scroll counts up from its newest line.
    fn top_offset(&self, pane: Pane) -> usize {
        match pane {
            Pane::Formula => self.formula_scroll,
            Pane::Database => self.database_scroll,
            Pane::Proof => self.proof_scroll,
            Pane::Output => self
                .shown_total(pane)
                .saturating_sub(self.scroll_up + self.output_content_h),
        }
    }

    /// Scrolls `pane` so that the shown row `offset` is at its top.
    fn scroll_to_offset(&mut self, pane: Pane, offset: usize) {
        *self.scroll_of(pane) = match pane {
            Pane::Output => self
                .shown_total(pane)
                .saturating_sub(offset + self.output_content_h),
            _ => offset,
        };
    }

    /// The unfiltered row at the top of `pane`.
    pub(super) fn top_row(&self, pane: Pane) -> usize {
        let offset = self.top_offset(pane);
        match self.find(pane) {
            Some(find) => find
                .rows
                .get(offset)
                .or(find.rows.last())
                .copied()
                .unwrap_or(0),
            None => offset,
        }
    }

    /// The unfiltered row shown at `offset` rows into `pane`'s content.
    pub(super) fn row_at_offset(&self, pane: Pane, offset: usize) -> Option<usize> {
        match self.find(pane) {
            Some(find) => find.rows.get(offset).copied(),
            None => Some(offset),
        }
    }

    /// Removes `pane`'s filter, keeping the row at its top at the top.
    pub(super) fn clear_find(&mut self, pane: Pane) {
        if self.find(pane).is_none() {
            return;
        }
        let top = self.top_row(pane);
        if let Some(slot) = slot(pane) {
            self.finds[slot] = None;
        }
        self.scroll_to_offset(pane, top);
    }

    /// Shows `row` of `pane` unfiltered, a third of the way down, and
    /// highlights it.
    fn jump(&mut self, pane: Pane, row: usize) {
        self.jump_to_rows(pane, vec![row]);
    }

    /// Shows `rows` (ascending, at least one) of `pane` unfiltered, the
    /// first a third of the way down, and highlights them until the pane
    /// next scrolls.
    pub(super) fn jump_to_rows(&mut self, pane: Pane, rows: Vec<usize>) {
        let Some(slot) = slot(pane) else {
            return;
        };
        self.finds[slot] = None;
        let h = self.content_h(pane);
        *self.scroll_of(pane) = rows[0].saturating_sub(h / 3);
        self.marked[slot] = rows;
    }

    /// The rows of `pane` highlighted by the last jump to it.
    #[cfg(test)]
    pub(super) fn marked(&self, pane: Pane) -> &[usize] {
        slot(pane).map_or(&[], |slot| &self.marked[slot])
    }

    /// Highlights `rows` of `pane` without moving it.
    pub(super) fn mark(&mut self, pane: Pane, rows: Vec<usize>) {
        if let Some(slot) = slot(pane) {
            self.marked[slot] = rows;
        }
    }

    /// Stops highlighting rows of `pane`.
    pub(super) fn unmark(&mut self, pane: Pane) {
        if let Some(slot) = slot(pane) {
            self.marked[slot].clear();
        }
    }

    /// Home or End: the focused pane's first or last row.
    pub(super) fn scroll_pane_to_end(&mut self, end: bool) {
        let pane = self.focus;
        let last_page = self.shown_total(pane).saturating_sub(self.content_h(pane));
        self.scroll_to_offset(pane, if end { last_page } else { 0 });
        self.unmark(pane);
    }

    /// The row of `pane` showing line `n`: Proof's display line, or the
    /// Formula or Database row holding constraint ID `n`.
    fn line_row(&self, pane: Pane, n: usize) -> Option<usize> {
        let session = self.session.as_ref()?;
        match pane {
            Pane::Proof => (1..=self.content_total(pane)).contains(&n).then(|| n - 1),
            Pane::Formula => session.formula_index(n),
            Pane::Database => {
                let database = session.database().ok()?;
                database
                    .entries
                    .binary_search_by_key(&n, |entry| entry.id)
                    .ok()
            }
            Pane::Output => None,
        }
    }

    /// The row of `pane` where label `name` is defined. In the Proof pane,
    /// the last checked line defining it, else the first unchecked one.
    fn label_row(&self, pane: Pane, name: &str) -> Option<usize> {
        let session = self.session.as_ref()?;
        let defines = |line: &String| leading_labels(line).contains(&name);
        match pane {
            Pane::Proof => {
                let checked = &session.buffer[..session.checked_len];
                let idx = checked.iter().rposition(defines).or_else(|| {
                    session.buffer[session.checked_len..]
                        .iter()
                        .position(defines)
                        .map(|idx| idx + session.checked_len)
                })?;
                Some(session.display_line(idx) - 1)
            }
            Pane::Formula => session.formula.iter().rposition(defines),
            Pane::Database => {
                let id = usize::try_from(session.label_id(name)?).ok()?;
                self.line_row(Pane::Database, id)
            }
            Pane::Output => None,
        }
    }

    /// Opens `pane`'s find, with its current query to edit.
    pub(super) fn open_find(&mut self, pane: Pane) {
        if !self.can_find() {
            return;
        }
        let Some(slot) = slot(pane) else {
            return;
        };
        let before = self.finds[slot].clone();
        self.find_editor
            .set_text(before.as_ref().map_or("", |find| find.text.as_str()));
        let origin = match &before {
            // Matches start from the top of the unfiltered view the
            // filter was made from.
            Some(_) => self.top_row(pane),
            None => self.top_offset(pane),
        };
        let scroll = *self.scroll_of(pane);
        self.find_input = Some(FindInput {
            pane,
            before: (before, scroll),
            origin,
            note: None,
        });
        self.focus = pane;
    }

    /// A key pressed while a find query is being typed.
    pub(super) fn find_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.cancel_find();
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => return,
            _ => {}
        }
        match self.find_editor.handle_key(key) {
            EditResult::Submitted(_) => self.find_input = None,
            EditResult::Quit => self.cancel_find(),
            EditResult::None => self.update_find(),
        }
    }

    /// Esc while typing a query: puts the pane back as it was.
    fn cancel_find(&mut self) {
        let Some(input) = self.find_input.take() else {
            return;
        };
        let (find, scroll) = input.before;
        if let Some(slot) = slot(input.pane) {
            self.finds[slot] = find;
        }
        *self.scroll_of(input.pane) = scroll;
        self.unmark(input.pane);
        self.find_editor.set_text("");
    }

    /// Applies the query typed so far.
    fn update_find(&mut self) {
        let Some(input) = &self.find_input else {
            return;
        };
        let (pane, origin) = (input.pane, input.origin);
        let text = self.find_editor.text();
        let note = self.apply_query(pane, &text, origin);
        if let Some(input) = &mut self.find_input {
            input.note = note;
        }
    }

    /// Applies `text` to `pane` (see `crate::find`), returning why it does
    /// nothing, if it doesn't.
    fn apply_query(&mut self, pane: Pane, text: &str, origin: usize) -> Option<String> {
        let (core, rest) = match pane {
            Pane::Database => constraint_kind(text),
            _ => (None, text),
        };
        let query = match Query::parse(rest) {
            Ok(query) => query,
            // The last line of the message says what is wrong; the lines
            // before it repeat the pattern.
            Err(err) => {
                let message = err.to_string();
                let last = message.lines().last().unwrap_or_default();
                return Some(last.trim_start_matches("error: ").to_string());
            }
        };
        // The Output pane has no lines or IDs to go to, and after `core:` or
        // `derived:` the query only narrows those down: it is all text.
        let filters_only = pane == Pane::Output || core.is_some();
        match query {
            None if core.is_some() => {
                self.show_matches(pane, text, Filter::text(""), core, origin);
            }
            None => {
                if let Some(slot) = slot(pane) {
                    self.finds[slot] = None;
                }
                self.scroll_to_offset(pane, origin);
                self.unmark(pane);
            }
            Some(query) if filters_only => {
                self.show_matches(pane, text, query.into_filter(rest), core, origin);
            }
            Some(Query::End) => {
                let total = self.content_total(pane);
                self.jump(pane, total.saturating_sub(1));
            }
            Some(Query::Line(n)) => match self.line_row(pane, n) {
                Some(row) => self.jump(pane, row),
                None if pane == Pane::Proof => return Some(format!("no line {n}")),
                None => return Some(format!("no constraint {n} here")),
            },
            Some(Query::Label(name, filter)) => match self.label_row(pane, &name) {
                Some(row) => self.jump(pane, row),
                None => self.show_matches(pane, text, filter, None, origin),
            },
            Some(Query::Filter(filter)) => self.show_matches(pane, text, filter, None, origin),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use crossterm::event::KeyModifiers;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::session::Session;
    use crate::tui::SCROLLBACK_CAP;

    const FORMULA: &str = "@f1 1 x1 1 x2 >= 1 ;\n1 x2 1 x3 >= 1 ;\n1 x12 1 x120 >= 1 ;\n";

    /// An app on `FORMULA` with `proof` in its buffer, unchecked, and every
    /// top pane `h` rows high.
    fn app(proof: &[String], h: usize) -> (NamedTempFile, App) {
        let mut file = NamedTempFile::with_suffix(".opb").unwrap();
        file.write_all(FORMULA.as_bytes()).unwrap();
        let mut session = Session::load(file.path().to_str().unwrap()).unwrap();
        session.buffer.extend(proof.iter().cloned());
        let mut app = App::new(None).unwrap();
        app.session = Some(session);
        app.proof_content_h = h;
        app.formula_content_h = h;
        app.database_content_h = h;
        (file, app)
    }

    fn lines(n: usize) -> Vec<String> {
        (1..=n)
            .map(|i| match i % 3 {
                0 => format!("@l{i} rup 1 x{i} >= 1 ;"),
                1 => format!("pol {i} 1 +"),
                _ => format!("rup 1 ~x{i} 1 x1 >= 1 ;"),
            })
            .collect()
    }

    fn press(app: &mut App, code: KeyCode) {
        app.find_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// Opens `pane`'s find and types `query`, leaving the input open.
    fn type_query(app: &mut App, pane: Pane, query: &str) {
        app.open_find(pane);
        app.find_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        for c in query.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    fn find(app: &mut App, pane: Pane, query: &str) {
        type_query(app, pane, query);
        press(app, KeyCode::Enter);
    }

    /// The display lines of the Proof rows `app` shows from the top.
    fn shown_proof_lines(app: &App, count: usize) -> Vec<usize> {
        (0..count)
            .filter_map(|i| app.row_at_offset(Pane::Proof, app.proof_scroll + i))
            .map(|row| row + 1)
            .collect()
    }

    #[test]
    fn text_filters_the_pane_and_narrows_as_typed() {
        let (_file, mut app) = app(&lines(300), 10);
        type_query(&mut app, Pane::Proof, "RUP 1 ~X1");
        let typed = app.find(Pane::Proof).unwrap().rows.clone();
        press(&mut app, KeyCode::Enter);
        assert!(app.find_input.is_none());

        let session = app.session.as_ref().unwrap();
        let fresh: Vec<usize> = (0..session.buffer.len())
            .filter(|&idx| session.buffer[idx].to_lowercase().contains("rup 1 ~x1"))
            .map(|idx| session.display_line(idx) - 1)
            .collect();
        assert_eq!(typed, fresh);
        // Buffer lines 11, 14 and 17: `rup 1 ~x11 1 x1 >= 1 ;`, ...
        assert_eq!(&shown_proof_lines(&app, 3), &[13, 16, 19]);
        assert_eq!(
            draw_title(&app),
            format!("{} of 300 matching RUP 1 ~X1", fresh.len())
        );
    }

    fn draw_title(app: &App) -> String {
        crate::tui::draw::matching(app.find(Pane::Proof), 300).unwrap()
    }

    #[test]
    fn an_edit_finds_the_rows_again() {
        let (_file, mut app) = app(&lines(30), 10);
        find(&mut app, Pane::Proof, "pol");
        assert_eq!(app.find(Pane::Proof).unwrap().rows.len(), 10);
        let session = app.session.as_mut().unwrap();
        session.buffer.set(0, "rup 1 x1 >= 1 ;".to_string());
        session.buffer.push("pol 1 2 +".to_string());
        app.refresh_find(Pane::Proof);
        let rows = &app.find(Pane::Proof).unwrap().rows;
        assert_eq!(rows.len(), 10);
        assert_eq!(rows.last(), Some(&32));
    }

    #[test]
    fn regexes_match_whole_words_and_bad_ones_say_why() {
        let (_file, mut app) = app(&[], 10);
        find(&mut app, Pane::Formula, "/x12\\b/");
        assert_eq!(app.find(Pane::Formula).unwrap().rows, [2]);
        find(&mut app, Pane::Formula, "x12");
        assert_eq!(app.find(Pane::Formula).unwrap().rows, [2]);
        find(&mut app, Pane::Formula, "x1");
        assert_eq!(app.find(Pane::Formula).unwrap().rows, [0, 2]);

        type_query(&mut app, Pane::Formula, "/x(");
        let note = app.find_input.as_ref().unwrap().note.clone().unwrap();
        assert!(note.contains("unclosed group"), "{note}");
        assert_eq!(
            app.find(Pane::Formula).unwrap().rows,
            [0, 1, 2],
            "as `/x` left it"
        );
    }

    #[test]
    fn numbers_labels_and_dollar_jump_unfiltered() {
        let (_file, mut app) = app(&lines(300), 9);
        find(&mut app, Pane::Proof, "pol");
        find(&mut app, Pane::Proof, "200");
        assert!(app.find(Pane::Proof).is_none());
        assert_eq!(app.marked(Pane::Proof), [199]);
        assert_eq!(app.proof_scroll, 199 - 3);

        // `@l30` is on buffer line 30, display line 32; `@l3` is a label
        // too, so it doesn't filter on its text.
        find(&mut app, Pane::Proof, "@l30");
        assert_eq!(app.marked(Pane::Proof), [31]);
        find(&mut app, Pane::Proof, "@nope");
        assert!(app.find(Pane::Proof).unwrap().rows.is_empty());

        find(&mut app, Pane::Proof, "$");
        assert_eq!(app.marked(Pane::Proof), [301]);
        app.scroll_lines(Pane::Proof, true, 1);
        assert!(app.marked(Pane::Proof).is_empty());

        find(&mut app, Pane::Formula, "@f1");
        assert_eq!(app.marked(Pane::Formula), [0]);
        find(&mut app, Pane::Formula, "3");
        assert_eq!(app.marked(Pane::Formula), [2]);

        type_query(&mut app, Pane::Proof, "999");
        let note = app.find_input.as_ref().unwrap().note.clone();
        assert_eq!(note.as_deref(), Some("no line 999"));
    }

    #[test]
    fn clearing_keeps_the_top_row_and_esc_restores() {
        let (_file, mut app) = app(&lines(300), 10);
        app.proof_scroll = 100;
        find(&mut app, Pane::Proof, "pol");
        // The first match at or after the top row (display line 101):
        // line 102 holds `pol 100 1 +`.
        assert_eq!(shown_proof_lines(&app, 2), [102, 105]);
        app.scroll_lines(Pane::Proof, false, 5);
        assert_eq!(shown_proof_lines(&app, 1), [117]);

        type_query(&mut app, Pane::Proof, "rup");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.find(Pane::Proof).unwrap().text, "pol");
        assert_eq!(shown_proof_lines(&app, 1), [117]);

        app.clear_find(Pane::Proof);
        assert!(app.find(Pane::Proof).is_none());
        assert_eq!(shown_proof_lines(&app, 2), [117, 118]);

        find(&mut app, Pane::Proof, "pol");
        find(&mut app, Pane::Proof, "");
        assert!(app.find(Pane::Proof).is_none(), "an empty query clears");
    }

    #[test]
    fn clicks_and_home_end_follow_the_filter() {
        let (_file, mut app) = app(&lines(300), 10);
        find(&mut app, Pane::Proof, "pol");
        app.scroll_lines(Pane::Proof, false, 2);
        // Row 1 is the pane's first content row.
        assert_eq!(app.proof_line_at_row(1), Some(9));

        app.focus = Pane::Proof;
        app.scroll_pane_to_end(true);
        assert_eq!(app.proof_scroll, 100 - 10);
        app.scroll_pane_to_end(false);
        assert_eq!(app.proof_scroll, 0);
        app.clear_find(Pane::Proof);
        app.scroll_pane_to_end(true);
        assert_eq!(app.proof_scroll, 302 - 10);
    }

    /// An app with `n` pairs of trace-like lines in its Output pane, which
    /// shows 3 lines.
    fn output_app(n: usize) -> (NamedTempFile, App) {
        let (file, mut app) = app(&[], 10);
        app.output_content_h = 3;
        for i in 1..=n {
            push_pair(&mut app, i);
        }
        (file, app)
    }

    fn push_pair(app: &mut App, i: usize) {
        app.scrollback.push(&format!("line {i}: rup 1 x{i} >= 1 ;"));
        app.scrollback
            .push(&format!("  ConstraintId {}: 1 x{i} >= 1", 100 + i));
    }

    #[test]
    fn the_output_pane_filters_and_follows_new_output() {
        let (_file, mut app) = output_app(60);
        find(&mut app, Pane::Output, "constraintid 15");
        // At the newest output, the newest matches.
        assert_eq!(
            shown_output(&app),
            [
                "  ConstraintId 157: 1 x57 >= 1",
                "  ConstraintId 158: 1 x58 >= 1",
                "  ConstraintId 159: 1 x59 >= 1",
            ]
        );
        assert_eq!(app.find(Pane::Output).unwrap().rows.len(), 10);

        push_pair(&mut app, 50);
        app.refresh_find(Pane::Output);
        assert_eq!(
            shown_output(&app).last().unwrap(),
            "  ConstraintId 150: 1 x50 >= 1",
            "new output joins the filter"
        );

        // Numbers and `$` are text here.
        find(&mut app, Pane::Output, "42");
        assert!(shown_output(&app).iter().all(|line| line.contains("42")));
        assert!(app.marked(Pane::Output).is_empty());
    }

    #[test]
    fn clearing_the_output_filter_keeps_the_top_line() {
        let (_file, mut app) = output_app(60);
        find(&mut app, Pane::Output, "line 1");
        app.scroll_lines(Pane::Output, true, 4);
        let top = shown_output(&app)[0].clone();
        app.clear_find(Pane::Output);
        assert!(app.find(Pane::Output).is_none());
        assert_eq!(shown_output(&app)[0], top);

        app.focus = Pane::Output;
        app.scroll_pane_to_end(false);
        assert_eq!(shown_output(&app)[0], app.scrollback.get(0));
        app.scroll_pane_to_end(true);
        assert_eq!(app.scroll_up, 0);
    }

    #[test]
    fn the_output_filter_survives_old_lines_being_dropped() {
        let (_file, mut app) = output_app(10);
        find(&mut app, Pane::Output, "ConstraintId");
        for i in 11..=SCROLLBACK_CAP / 2 + 20 {
            push_pair(&mut app, i);
        }
        app.refresh_find(Pane::Output);
        let rows = &app.find(Pane::Output).unwrap().rows;
        assert_eq!(rows.len(), SCROLLBACK_CAP / 2);
        assert!(
            rows.iter()
                .all(|&row| app.scrollback.get(row).contains("ConstraintId"))
        );
    }

    /// The Output lines `app` shows, oldest first, as drawing works them out.
    fn shown_output(app: &App) -> Vec<String> {
        let rows: Vec<usize> = match app.find(Pane::Output) {
            Some(find) => find.rows.clone(),
            None => (0..app.scrollback.len()).collect(),
        };
        let end = rows.len() - app.scroll_up.min(rows.len());
        let start = end.saturating_sub(app.output_content_h);
        rows[start..end]
            .iter()
            .map(|&row| app.scrollback.get(row).to_string())
            .collect()
    }

    #[test]
    fn the_database_pane_finds_ids_labels_and_text() {
        if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
            eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
            return;
        }
        let proof = [
            "@p rup 1 x1 1 x2 1 x3 >= 1 ;".to_string(),
            "rup 1 x2 1 x3 1 x4 >= 1 ;".to_string(),
        ];
        let (_file, mut app) = app(&proof, 10);
        let (_, rejection) = app.session.as_mut().unwrap().drive_forward().unwrap();
        assert!(rejection.is_none(), "{rejection:?}");

        find(&mut app, Pane::Database, "5");
        assert_eq!(app.marked(Pane::Database), [4]);
        find(&mut app, Pane::Database, "@p");
        assert_eq!(app.marked(Pane::Database), [3]);
        find(&mut app, Pane::Database, "@f");
        assert_eq!(
            app.find(Pane::Database).unwrap().rows,
            [0],
            "labels count as text"
        );
        find(&mut app, Pane::Database, "x3");
        assert_eq!(app.find(Pane::Database).unwrap().rows, [1, 3, 4]);

        let rows = |app: &App| app.find(Pane::Database).unwrap().rows.clone();
        find(&mut app, Pane::Database, "core:");
        assert_eq!(rows(&app), [0, 1, 2]);
        find(&mut app, Pane::Database, "Derived:");
        assert_eq!(rows(&app), [3, 4]);
        find(&mut app, Pane::Database, "derived: x3");
        assert_eq!(rows(&app), [3, 4]);
        find(&mut app, Pane::Database, "core: x3");
        assert_eq!(rows(&app), [1]);
        find(&mut app, Pane::Database, "core: @f");
        assert_eq!(rows(&app), [0], "labels count");
        // After a prefix, a number is text, not a jump.
        find(&mut app, Pane::Database, "derived: 4");
        assert_eq!(rows(&app), [4]);
        assert!(app.marked(Pane::Database).is_empty());
        // Only the Database pane has core and derived constraints.
        find(&mut app, Pane::Proof, "core:");
        assert!(app.find(Pane::Proof).unwrap().rows.is_empty());
    }
}
