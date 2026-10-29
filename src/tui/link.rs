//! Links between a line and its constraints: clicking a Formula or Proof
//! line number shows the constraints that line gave in the Database pane;
//! clicking a Database ID shows the line it came from. `:where` does the
//! same from the prompt. Both ends are highlighted until their pane next
//! scrolls.

use crate::checker::parse::{ConstraintRef, constraint_refs};
use crate::commands::where_::{self, Link, Target};
use crate::session::ConstraintSource;
use crate::tui::{App, Pane};

impl App {
    /// Whether links can move the panes: not while one is being edited.
    fn links_active(&self) -> bool {
        self.vim.is_none() && self.formula_browse.is_none()
    }

    /// Shows both ends of `link`: its line in the Formula or Proof pane,
    /// and those of its constraints still in the database in the Database
    /// pane. The `clicked` pane, already showing its end, only highlights
    /// it.
    fn follow_link(&mut self, link: &Link, clicked: Option<Pane>) {
        let Some(session) = &self.session else {
            return;
        };
        let rows: Vec<usize> = match session.database() {
            Ok(database) => link
                .ids
                .iter()
                .filter_map(|&id| {
                    database
                        .entries
                        .binary_search_by_key(&id, |entry| entry.id)
                        .ok()
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        let (pane, row) = match link.source {
            ConstraintSource::Formula { line } => (Pane::Formula, line),
            ConstraintSource::Proof { display_line } => (Pane::Proof, display_line - 1),
        };
        for (pane, rows) in [(pane, vec![row]), (Pane::Database, rows)] {
            if rows.is_empty() {
                continue;
            }
            if clicked == Some(pane) {
                self.mark(pane, rows);
            } else {
                self.jump_to_rows(pane, rows);
            }
        }
    }

    /// Follows `target`'s link from a click on `clicked`, saying in the
    /// Output pane if there is none.
    fn follow_target(&mut self, target: &Target, clicked: Pane) {
        let Some(session) = &self.session else {
            return;
        };
        match where_::link(session, target) {
            Ok(Ok(link)) => self.follow_link(&link, Some(clicked)),
            Ok(Err(message)) => self.scrollback.push(&message),
            Err(err) => self.scrollback.push(&format!("Error: {err:#}")),
        }
    }

    /// After `:where <args>` has printed its link: shows it in the panes.
    pub(super) fn follow_where(&mut self, args: &str) {
        let Some(session) = &self.session else {
            return;
        };
        if !self.links_active() {
            return;
        }
        if let Ok(target) = where_::parse(session, args)
            && let Ok(Ok(link)) = where_::link(session, &target)
        {
            self.follow_link(&link, None);
        }
    }

    /// The unfiltered row of top pane `pane` drawn at terminal `row` in the
    /// last frame, if it is one of its content rows.
    fn content_row_at(&self, pane: Pane, row: u16) -> Option<usize> {
        let (h, scroll) = match pane {
            Pane::Formula => (self.formula_content_h, self.formula_scroll),
            Pane::Database => (self.database_content_h, self.database_scroll),
            Pane::Proof => (self.proof_content_h, self.proof_scroll),
            Pane::Output => return None,
        };
        let row = usize::from(row);
        if row < 1 || row > h {
            return None;
        }
        self.row_at_offset(pane, scroll + row - 1)
    }

    /// A left click at (`column`, `row`): on a Formula or Proof line number,
    /// or a Database ID, follows that link and returns `true`.
    pub(super) fn click_link(&mut self, column: u16, row: u16) -> bool {
        if !self.links_active() {
            return false;
        }
        let (Some(pane), Some(layout)) = (self.pane_at(column, row), self.last_layout) else {
            return false;
        };
        let prefix_w = match pane {
            Pane::Formula => self.prefix_widths[0],
            Pane::Database => self.prefix_widths[1],
            Pane::Proof => self.prefix_widths[2],
            Pane::Output => return false,
        };
        // The Proof pane's first column is the breakpoint marker.
        let numbers = if pane == Pane::Proof { 1 } else { 0 }..prefix_w;
        let offset = usize::from(column.saturating_sub(layout.content_x0(pane)));
        let Some(content_row) = self.content_row_at(pane, row) else {
            return false;
        };
        if offset >= prefix_w {
            let hscroll = match pane {
                Pane::Formula => self.formula_hscroll,
                Pane::Proof => self.proof_hscroll,
                _ => return false,
            };
            return self.click_reference(pane, content_row, offset - prefix_w + hscroll);
        }
        let (true, Some(session)) = (numbers.contains(&offset), &self.session) else {
            return false;
        };
        match pane {
            Pane::Formula if content_row < session.formula.len() => {
                let link = where_::formula_link(session, content_row);
                self.follow_link(&link, Some(Pane::Formula));
            }
            Pane::Database => {
                let Some(id) = session
                    .database()
                    .ok()
                    .and_then(|database| database.entries.get(content_row).map(|entry| entry.id))
                else {
                    return false;
                };
                self.follow_target(&Target::Constraint(id), Pane::Database);
            }
            // The preamble's lines link to nothing.
            Pane::Proof if session.buffer_index(content_row + 1).is_some() => {
                self.follow_target(&Target::Line(content_row + 1), Pane::Proof);
            }
            _ => return false,
        }
        true
    }

    /// A click on character `col` of the text of row `row` of the Formula
    /// or Proof pane: on a constraint reference (a label, or an ID where
    /// the rule takes one — see `constraint_refs`), shows that constraint
    /// in the Database pane, highlighted, and returns `true`.
    fn click_reference(&mut self, pane: Pane, row: usize, col: usize) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        let text = match pane {
            Pane::Formula => session.formula.get(row),
            Pane::Proof => session
                .buffer_index(row + 1)
                .map(|idx| &session.buffer[idx]),
            _ => None,
        };
        let Some(text) = text else {
            return false;
        };
        let Some((byte, _)) = text.char_indices().nth(col) else {
            return false;
        };
        let Some((_, reference)) = constraint_refs(text)
            .into_iter()
            .find(|(range, _)| range.contains(&byte))
        else {
            return false;
        };
        let id = match reference {
            ConstraintRef::Id(id) => id,
            ConstraintRef::Label(name) => {
                match session
                    .label_id(name)
                    .and_then(|id| usize::try_from(id).ok())
                {
                    Some(id) => id,
                    None => {
                        let message = format!("No checked constraint is labelled {name}.");
                        self.scrollback.push(&message);
                        return true;
                    }
                }
            }
        };
        let row = session.database().ok().and_then(|database| {
            database
                .entries
                .binary_search_by_key(&id, |entry| entry.id)
                .ok()
        });
        match row {
            Some(row) => self.jump_to_rows(Pane::Database, vec![row]),
            None => self
                .scrollback
                .push(&format!("ConstraintId {id} is not in the database.")),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::NamedTempFile;

    use super::*;
    use crate::session::Session;
    use crate::tui::layout::Layout;

    const FORMULA: &str = "@f1 1 x1 1 x2 >= 1 ;\n1 x2 1 x3 >= 1 ;\n1 x3 1 x4 = 1 ;\n";

    /// An app on `FORMULA` with these lines checked (display lines 3-6,
    /// deriving IDs 5, 6 and 7 after the formula's 1-4), laid out 180x40
    /// with the line numbers 5, 3 and 5 columns wide.
    fn app() -> Option<(NamedTempFile, App)> {
        std::env::var_os("VERIPB_REPL_VERIPB_BIN")?;
        let mut file = NamedTempFile::with_suffix(".opb").unwrap();
        file.write_all(FORMULA.as_bytes()).unwrap();
        let mut session = Session::load(file.path().to_str().unwrap()).unwrap();
        for line in [
            "@p rup 1 x1 1 x2 1 x3 >= 1 ;",
            "rup 1 x2 1 x3 1 x5 >= 1 ;",
            "del id 6 ;",
            "pol 1 2 + ;",
        ] {
            session.buffer.push(line.to_string());
        }
        session.drive_forward().unwrap();
        let mut app = App::new(None).unwrap();
        app.session = Some(session);
        app.last_layout = Layout::compute(180, 40, 0, None);
        app.formula_content_h = 10;
        app.database_content_h = 10;
        app.proof_content_h = 10;
        app.prefix_widths = [5, 3, 5];
        Some((file, app))
    }

    /// Clicks on `pane`'s content row `row`, `offset` columns in.
    fn click(app: &mut App, pane: Pane, row: u16, offset: u16) -> bool {
        let x0 = app.last_layout.unwrap().content_x0(pane);
        app.click_link(x0 + offset, 1 + row)
    }

    #[test]
    fn where_shows_both_ends() {
        let Some((_file, mut app)) = app() else {
            return;
        };
        app.execute(":where line 6");
        // ID 6 is deleted, so ID 7 is the database's sixth row.
        assert_eq!(app.marked(Pane::Proof), [5]);
        assert_eq!(app.marked(Pane::Database), [5]);
        app.execute(":where @f1");
        assert_eq!(app.marked(Pane::Formula), [0]);
        assert_eq!(app.marked(Pane::Database), [0]);
        assert!(app.marked(Pane::Proof).is_empty());
    }

    #[test]
    fn clicking_a_number_shows_the_other_end() {
        let Some((_file, mut app)) = app() else {
            return;
        };
        // Database ID 5 came from proof line 3.
        app.database_scroll = 2;
        assert!(click(&mut app, Pane::Database, 2, 1));
        assert_eq!(app.marked(Pane::Database), [4]);
        assert_eq!(app.database_scroll, 2, "the clicked pane stays put");
        assert_eq!(app.marked(Pane::Proof), [2]);

        // Formula line 3 holds IDs 3 and 4.
        assert!(click(&mut app, Pane::Formula, 2, 0));
        assert_eq!(app.marked(Pane::Formula), [2]);
        assert_eq!(app.marked(Pane::Database), [2, 3]);

        // The preamble, and text that refers to nothing, link nothing.
        assert!(!click(&mut app, Pane::Proof, 0, 1));
        assert!(!click(&mut app, Pane::Proof, 2, 5 + 3));
    }

    #[test]
    fn clicks_in_a_filtered_pane_keep_the_filter() {
        let Some((_file, mut app)) = app() else {
            return;
        };
        app.open_find(Pane::Proof);
        for c in "pol".chars() {
            app.find_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.find_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(click(&mut app, Pane::Proof, 0, 2));
        assert!(app.find(Pane::Proof).is_some());
        assert_eq!(app.marked(Pane::Proof), [5]);
        assert_eq!(app.marked(Pane::Database), [5]);

        // A scroll stops highlighting that pane only.
        app.scroll_lines(Pane::Database, false, 1);
        assert!(app.marked(Pane::Database).is_empty());
        assert_eq!(app.marked(Pane::Proof), [5]);
    }

    #[test]
    fn clicking_a_title_shows_that_panes_stats() {
        let Some((_file, mut app)) = app() else {
            return;
        };
        let layout = app.last_layout.unwrap();
        for (pane, heading) in [
            (Pane::Formula, "Formula: "),
            (Pane::Database, "Database: "),
            (Pane::Proof, "Proof: "),
        ] {
            let x = layout.content_x0(pane) + 2;
            assert_eq!(layout.top_header_at(x, 0), Some(pane));
            assert_eq!(layout.top_header_at(x, 1), None);
            app.show_stats(pane);
            let shown =
                (0..app.scrollback.len()).any(|i| app.scrollback.get(i).starts_with(heading));
            assert!(shown, "{heading}");
        }
    }

    #[test]
    fn clicking_a_reference_in_a_line_shows_its_constraint() {
        let Some((_file, mut app)) = app() else {
            return;
        };
        // Proof row 5 is `pol 1 2 + ;`; its text starts 5 columns in.
        assert!(click(&mut app, Pane::Proof, 5, 5 + 6));
        assert_eq!(app.marked(Pane::Database), [1]);
        assert!(
            app.marked(Pane::Proof).is_empty(),
            "the line's own end isn't marked"
        );

        // `@p` on row 2 names constraint 5, the database's fifth row.
        assert!(click(&mut app, Pane::Proof, 2, 5 + 1));
        assert_eq!(app.marked(Pane::Database), [4]);

        // `del id 6 ;`: constraint 6 is deleted, so nothing is highlighted.
        app.marked = Default::default();
        assert!(click(&mut app, Pane::Proof, 4, 5 + 7));
        assert!(app.marked(Pane::Database).is_empty());

        // Not a reference: the `+`, and constraint text.
        assert!(!click(&mut app, Pane::Proof, 5, 5 + 8));
        assert!(!click(&mut app, Pane::Proof, 3, 5 + 6));

        // `@f1` in the formula.
        assert!(click(&mut app, Pane::Formula, 0, 5 + 1));
        assert_eq!(app.marked(Pane::Database), [0]);
    }
}
