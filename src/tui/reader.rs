//! The reader: a box over the panes showing `:explain <n>`, `:help <topic>`
//! or `?<topic>`, with links to follow. An explanation has two views, in
//! plain English and the checker's account; ←/→ switch between them. A
//! glossary link opens that entry in the reader, Backspace goes back; a
//! constraint link closes it and shows the constraint in the panes, as
//! `:where` does. `p` prints the page into the Output pane.
//!
//! The box is drawn by `draw::reader`, which records where each link
//! landed (`ReaderFrame`) for the keys and the mouse.

use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};

use crate::commands::{self, explain, help};
use crate::output::Output;
use crate::teach::{self, markup};
use crate::tui::{App, WHEEL_STEP};

/// How a reader line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// A usage or a term: bold.
    Heading,
    /// Markup text.
    Text,
    /// An aside: dim markup text.
    Note,
    /// Command output, highlighted as the Output pane does; not markup.
    Output,
}

/// One logical line, wrapped to the box at draw time. Rows after the
/// first are indented by `lead`'s width.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Line {
    pub(super) lead: String,
    pub(super) text: String,
    pub(super) kind: Kind,
}

impl Line {
    fn new(lead: impl Into<String>, text: impl Into<String>, kind: Kind) -> Line {
        Line {
            lead: lead.into(),
            text: text.into(),
            kind,
        }
    }

    fn blank() -> Line {
        Line::new("", "", Kind::Text)
    }

    /// The line's runs: its markup, or for `Output` its text as is.
    pub(super) fn inlines(&self) -> Vec<markup::Inline> {
        match self.kind {
            Kind::Output if self.text.is_empty() => Vec::new(),
            Kind::Output => vec![markup::Inline::Text(self.text.clone())],
            _ => markup::parse(&self.text),
        }
    }

    /// The line as plain text (see [`markup::plain`]).
    fn plain(&self) -> String {
        match self.kind {
            Kind::Output => format!("{}{}", self.lead, self.text),
            _ => format!("{}{}", self.lead, markup::plain(&self.text)),
        }
    }
}

/// Where a link goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// A glossary entry or rule, by glossary id.
    Entry(String),
    /// A constraint, by ID.
    Constraint(usize),
}

/// One of a page's views: its name and lines.
pub(super) struct View {
    pub(super) name: &'static str,
    pub(super) lines: Vec<Line>,
}

impl View {
    /// The view's links, in reading order: what Tab steps through.
    pub(super) fn links(&self) -> Vec<Target> {
        self.lines
            .iter()
            .flat_map(Line::inlines)
            .filter_map(|inline| match inline {
                markup::Inline::Term { id, .. } => Some(Target::Entry(id)),
                markup::Inline::Constraint { id, .. } => Some(Target::Constraint(id)),
                _ => None,
            })
            .collect()
    }
}

/// One page: an explanation or a help topic, and where it is scrolled to.
pub(super) struct Page {
    pub(super) title: String,
    pub(super) views: Vec<View>,
    /// Index into `views`.
    pub(super) view: usize,
    /// Rows scrolled off the top; clamped at draw time.
    pub(super) scroll: usize,
    /// Index into the view's links of the focused one.
    pub(super) focus: Option<usize>,
    /// Scroll the focused link into view at the next draw.
    pub(super) reveal: bool,
}

impl Page {
    fn new(title: String, views: Vec<View>) -> Page {
        Page {
            title,
            views,
            view: 0,
            scroll: 0,
            focus: None,
            reveal: false,
        }
    }

    pub(super) fn current(&self) -> &View {
        &self.views[self.view]
    }

    /// `:help <topic>`'s page, `None` for an unknown topic.
    fn help(topic: &str) -> Option<Page> {
        let lines = help::topic_lines(topic)?;
        let lines = lines
            .iter()
            .map(|line| {
                let text = line.trim_start();
                let lead = " ".repeat(line.len() - text.len());
                match (line.is_empty(), lead.is_empty()) {
                    (true, _) => Line::blank(),
                    (false, true) => Line::new("", text, Kind::Heading),
                    (false, false) => Line::new(lead, text, Kind::Text),
                }
            })
            .collect();
        let topic = topic.trim().trim_start_matches(':');
        let topic = topic.strip_prefix("rule-").unwrap_or(topic);
        Some(Page::new(
            format!(":help {topic}"),
            vec![View {
                name: "Help",
                lines,
            }],
        ))
    }

    /// The page for line `n` (`rejected` if it is the pending rejected
    /// line): in plain English, when `teach` covers it, and the checker's
    /// account.
    fn explain(session: &crate::session::Session, n: usize, rejected: bool) -> Page {
        let mut checker = Lines::default();
        if rejected {
            explain::print_rejection(session, n, &mut checker);
            return Page::new(format!("Line {n}: rejected"), vec![checker.view("Checker")]);
        }
        let explanation = teach::explain::explain(session, n);
        let hints = explanation.as_ref().and_then(|e| e.replay_hints.as_deref());
        explain::print_checker(session, n, hints, &mut checker);
        let Some(explanation) = explanation else {
            return Page::new(format!("Line {n}"), vec![checker.view("Checker")]);
        };
        let mut lines = Vec::new();
        for block in &explanation.blocks {
            if !lines.is_empty() {
                lines.push(Line::blank());
            }
            match block {
                teach::explain::Block::Paragraph(text) => {
                    lines.push(Line::new("", text.as_str(), Kind::Text));
                }
                teach::explain::Block::Note(text) => {
                    lines.push(Line::new("Note: ", text.as_str(), Kind::Note));
                }
                teach::explain::Block::Steps(items) => {
                    let width = items.len().to_string().len();
                    for (i, item) in items.iter().enumerate() {
                        let lead = format!("{:>width$}. ", i + 1);
                        lines.push(Line::new(lead, item.as_str(), Kind::Text));
                    }
                }
            }
        }
        Page::new(
            explanation.title.clone(),
            vec![
                View {
                    name: "Plain English",
                    lines,
                },
                checker.view("Checker"),
            ],
        )
    }
}

/// Command output collected as reader lines.
#[derive(Default)]
struct Lines(Vec<String>);

impl Output for Lines {
    fn line(&mut self, text: &str) {
        self.0.push(text.to_string());
    }
}

impl Lines {
    fn view(self, name: &'static str) -> View {
        View {
            name,
            lines: self
                .0
                .into_iter()
                .map(|text| Line::new("", text, Kind::Output))
                .collect(),
        }
    }
}

/// The reader's pages: the one shown last, those it was followed from
/// before it.
pub(super) struct Reader {
    pub(super) pages: Vec<Page>,
}

impl Reader {
    pub(super) fn page(&self) -> &Page {
        self.pages.last().expect("a reader has a page")
    }

    fn page_mut(&mut self) -> &mut Page {
        self.pages.last_mut().expect("a reader has a page")
    }
}

/// The reader as last drawn: the box, its content rows, and where each
/// visible link is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ReaderFrame {
    pub(super) x: Range<u16>,
    pub(super) y: Range<u16>,
    /// Content rows in the box.
    pub(super) content_h: usize,
    /// Each visible piece of a link: its row, columns and link index.
    pub(super) hits: Vec<(u16, Range<u16>, usize)>,
}

/// What a submitted line asks the reader for.
#[derive(Debug, PartialEq, Eq)]
enum Request<'a> {
    Help(&'a str),
    Explain(&'a str),
}

/// `?<topic>`, `:help <topic>` or `:explain [<n>]`; `None` for anything
/// else, including `:help` with no topic (its list stays in the Output
/// pane).
fn request(line: &str) -> Option<Request<'_>> {
    if let Some(topic) = help::question(line) {
        return Some(Request::Help(topic.trim()));
    }
    let rest = line.trim().strip_prefix(':')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    match commands::resolve_command(name).ok()? {
        "help" if !args.trim().is_empty() => Some(Request::Help(args.trim())),
        "explain" => Some(Request::Explain(args)),
        _ => None,
    }
}

impl App {
    /// Opens the reader for `line` if it asks for one (see [`request`]),
    /// echoing it to the Output pane; returns whether it was handled.
    /// Anything that can't be shown (an unknown topic, a line with
    /// nothing to explain) is said in the Output pane instead.
    pub(super) fn open_reader(&mut self, line: &str) -> bool {
        let Some(request) = request(line) else {
            return false;
        };
        if matches!(request, Request::Explain(_)) && self.session.is_none() {
            return false;
        }
        let prompt = if self.debug.is_some() {
            "debug>"
        } else if self.formula_browse.is_some() {
            "opb>"
        } else {
            "pbp>"
        };
        self.scrollback.push(&format!("{prompt} {line}"));
        let page = match request {
            Request::Help(topic) => match Page::help(topic) {
                Some(page) => page,
                None => {
                    help::run(topic, &mut self.scrollback);
                    return true;
                }
            },
            Request::Explain(args) => {
                let session = self.session.as_ref().expect("checked above");
                let Some((n, rejected)) = explain::target(session, args, &mut self.scrollback)
                else {
                    return true;
                };
                Page::explain(session, n, rejected)
            }
        };
        self.reader = Some(Reader { pages: vec![page] });
        true
    }

    /// A key while the reader is open; it owns every key.
    pub(super) fn reader_key(&mut self, key: KeyEvent) {
        let content_h = self.reader_frame.as_ref().map_or(1, |f| f.content_h);
        let Some(reader) = self.reader.as_mut() else {
            return;
        };
        let pages = reader.pages.len();
        let page = reader.page_mut();
        let links = page.current().links().len();
        match key.code {
            KeyCode::Esc => self.reader = None,
            KeyCode::Up => page.scroll = page.scroll.saturating_sub(1),
            KeyCode::Down => page.scroll = page.scroll.saturating_add(1),
            KeyCode::PageUp => page.scroll = page.scroll.saturating_sub(content_h.max(2) - 1),
            KeyCode::PageDown => page.scroll = page.scroll.saturating_add(content_h.max(2) - 1),
            KeyCode::Home => page.scroll = 0,
            KeyCode::End => page.scroll = usize::MAX,
            KeyCode::Tab | KeyCode::BackTab if links > 0 => {
                let forward = key.code == KeyCode::Tab;
                page.focus = Some(match page.focus {
                    None if forward => 0,
                    None => links - 1,
                    Some(i) if forward => (i + 1) % links,
                    Some(i) => (i + links - 1) % links,
                });
                page.reveal = true;
            }
            KeyCode::Left | KeyCode::Right if page.views.len() > 1 => {
                let count = page.views.len();
                page.view = if key.code == KeyCode::Right {
                    (page.view + 1) % count
                } else {
                    (page.view + count - 1) % count
                };
                page.scroll = 0;
                page.focus = None;
            }
            KeyCode::Backspace if pages > 1 => {
                reader.pages.pop();
            }
            KeyCode::Enter => {
                if let Some(focus) = page.focus {
                    self.follow(focus);
                }
            }
            KeyCode::Char('p') => self.print_page(),
            _ => {}
        }
    }

    /// Follows the current view's link `index`: a glossary entry opens in
    /// the reader; a constraint closes it and shows the constraint in the
    /// panes.
    fn follow(&mut self, index: usize) {
        let Some(reader) = self.reader.as_mut() else {
            return;
        };
        let page = reader.page_mut();
        page.focus = Some(index);
        match page.current().links().get(index) {
            Some(Target::Entry(id)) => {
                if let Some(page) = Page::help(id) {
                    reader.pages.push(page);
                }
            }
            Some(Target::Constraint(id)) => {
                self.reader = None;
                self.follow_where(&id.to_string());
            }
            None => {}
        }
    }

    /// Prints the current view into the Output pane, as plain text.
    fn print_page(&mut self) {
        let Some(reader) = &self.reader else {
            return;
        };
        let page = reader.page();
        let mut text = vec![page.title.clone()];
        if page.views.len() > 1 {
            text.push(format!("({})", page.current().name));
        }
        text.extend(page.current().lines.iter().map(Line::plain));
        for line in text {
            self.scrollback.push(&line);
        }
    }

    /// A mouse event while the reader is open: the wheel scrolls it, a
    /// click on a link follows it, a click outside the box closes it.
    pub(super) fn reader_mouse(&mut self, mouse: MouseEvent) {
        let Some(frame) = self.reader_frame.clone() else {
            return;
        };
        let inside = frame.x.contains(&mouse.column) && frame.y.contains(&mouse.row);
        let Some(reader) = self.reader.as_mut() else {
            return;
        };
        let page = reader.page_mut();
        match mouse.kind {
            MouseEventKind::ScrollUp if inside => {
                page.scroll = page.scroll.saturating_sub(WHEEL_STEP);
            }
            MouseEventKind::ScrollDown if inside => {
                page.scroll = page.scroll.saturating_add(WHEEL_STEP);
            }
            MouseEventKind::Down(MouseButton::Left) if !inside => self.reader = None,
            MouseEventKind::Down(MouseButton::Left) => {
                let hit = frame
                    .hits
                    .iter()
                    .find(|(row, columns, _)| *row == mouse.row && columns.contains(&mouse.column));
                if let Some(&(_, _, index)) = hit {
                    self.follow(index);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::Pane;
    use crate::tui::layout::Layout;
    use crossterm::event::KeyModifiers;

    /// An app on the chain demo, checked, laid out 120x40; `None` without
    /// veripb.
    fn app() -> Option<App> {
        std::env::var_os("VERIPB_REPL_VERIPB_BIN")?;
        let files = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("demos/files");
        let mut app = App::new(None).unwrap();
        app.session = Some(crate::session::Session::load(files.join("chain.opb").to_str()?).ok()?);
        let source = format!(":source {}", files.join("chain.pbp").display());
        commands::dispatch(&mut app.session, &source, &mut app.scrollback).unwrap();
        // The demo's first line leaves out a hint it needs.
        let session = app.session.as_mut()?;
        session.set_line(0, "@x3 rup 1 x3 >= 1 ;").unwrap();
        session.drive_forward().unwrap();
        app.last_layout = Layout::compute(120, 40, 0, None);
        Some(app)
    }

    fn press(app: &mut App, code: KeyCode) {
        app.reader_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// Draws the reader, as the frame would.
    fn draw(app: &mut App) {
        let layout = app.last_layout.unwrap();
        let theme = app.theme.theme();
        crate::tui::draw::reader::draw(&mut Vec::new(), app, &layout, 120, theme).unwrap();
    }

    fn page(app: &App) -> &Page {
        app.reader.as_ref().expect("the reader is open").page()
    }

    /// The display line of `@not_x3`, a checked `rup`.
    fn not_x3(app: &App) -> usize {
        let session = app.session.as_ref().unwrap();
        let idx = (0..session.checked_len)
            .find(|&idx| session.buffer[idx].starts_with("@not_x3"))
            .expect("@not_x3 checked");
        session.display_line(idx)
    }

    #[test]
    fn explain_opens_both_views_and_follows_links() {
        let Some(mut app) = app() else {
            return;
        };
        let n = not_x3(&app);
        app.execute(&format!(":explain {n}"));
        let echoed = (0..app.scrollback.len())
            .any(|i| app.scrollback.get(i) == format!("pbp> :explain {n}"));
        assert!(
            echoed,
            "{:#?}",
            (0..app.scrollback.len())
                .map(|i| app.scrollback.get(i))
                .collect::<Vec<_>>()
        );
        let names: Vec<&str> = page(&app).views.iter().map(|v| v.name).collect();
        assert_eq!(names, ["Plain English", "Checker"]);
        assert!(page(&app).title.starts_with(&format!("Line {n}: ")));

        press(&mut app, KeyCode::Right);
        assert_eq!(page(&app).view, 1);
        assert!(
            page(&app)
                .current()
                .lines
                .iter()
                .any(|l| l.text.contains("needed:")),
            "the checker's account"
        );
        press(&mut app, KeyCode::Left);
        assert_eq!(page(&app).view, 0);

        // Tab to the first link, a glossary entry, and follow it; back.
        press(&mut app, KeyCode::Tab);
        assert_eq!(page(&app).focus, Some(0));
        let Target::Entry(id) = &page(&app).current().links()[0] else {
            panic!("a glossary link first");
        };
        let title = format!(":help {id}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(page(&app).title, title);
        press(&mut app, KeyCode::Backspace);
        assert!(page(&app).title.starts_with("Line "));
        assert_eq!(page(&app).focus, Some(0), "back where it was");

        // Shift+Tab from the first link wraps to the last.
        press(&mut app, KeyCode::BackTab);
        assert_eq!(
            page(&app).focus,
            Some(page(&app).current().links().len() - 1)
        );

        // `p` prints the view.
        press(&mut app, KeyCode::Char('p'));
        let printed: Vec<&str> = (0..app.scrollback.len())
            .map(|i| app.scrollback.get(i))
            .collect();
        assert!(printed.contains(&"(Plain English)"), "{printed:#?}");

        // A constraint link closes the reader and shows the constraint.
        let links = page(&app).current().links();
        let index = links
            .iter()
            .position(|t| matches!(t, Target::Constraint(_)))
            .expect("a constraint link");
        app.follow(index);
        assert!(app.reader.is_none());
        assert!(!app.marked(Pane::Database).is_empty());
    }

    #[test]
    fn help_opens_and_unknown_topics_say_so() {
        let Some(mut app) = app() else {
            return;
        };
        app.execute("?slack");
        assert_eq!(page(&app).title, ":help slack");
        assert_eq!(page(&app).views.len(), 1);
        press(&mut app, KeyCode::Esc);
        assert!(app.reader.is_none());

        app.execute(":help nonsense");
        assert!(app.reader.is_none());
        let last = app.scrollback.get(app.scrollback.len() - 1);
        assert!(last.starts_with("No help for 'nonsense'"), "{last}");
        // Bare `:help` stays in the Output pane.
        app.execute(":help");
        assert!(app.reader.is_none());
    }

    #[test]
    fn esc_in_debug_closes_only_the_reader() {
        let Some(mut app) = app() else {
            return;
        };
        app.execute(":debug");
        assert!(app.debug_active());
        app.execute("?rup");
        assert_eq!(app.scrollback.get(app.scrollback.len() - 1), "debug> ?rup");
        press(&mut app, KeyCode::Esc);
        assert!(app.reader.is_none());
        assert!(app.debug_active());
    }

    #[test]
    fn the_mouse_scrolls_follows_and_closes() {
        let Some(mut app) = app() else {
            return;
        };
        let n = not_x3(&app);
        app.execute(&format!(":explain {n}"));
        draw(&mut app);
        let frame = app.reader_frame.clone().expect("drawn");
        assert_eq!(frame.x, 8..112);
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        // A click on a glossary link follows it.
        let &(row, ref columns, index) = frame
            .hits
            .iter()
            .find(|(_, _, i)| matches!(page(&app).current().links()[*i], Target::Entry(_)))
            .expect("a glossary link drawn");
        app.reader_mouse(click(columns.start, row));
        assert_eq!(app.reader.as_ref().unwrap().pages.len(), 2, "{index}");
        // Inside the box, off a link: nothing; outside: closed.
        app.reader_mouse(click(frame.x.start + 1, frame.y.end - 1));
        assert!(app.reader.is_some());
        app.reader_mouse(click(0, frame.y.end + 1));
        assert!(app.reader.is_none());
    }

    #[test]
    fn requests_are_help_topics_and_explains() {
        assert_eq!(request("?slack"), Some(Request::Help("slack")));
        assert_eq!(request(":help rup"), Some(Request::Help("rup")));
        assert_eq!(request(":explain 7"), Some(Request::Explain("7")));
        assert_eq!(request(":explain"), Some(Request::Explain("")));
        assert_eq!(request(":help"), None);
        assert_eq!(request(":verify"), None);
        assert_eq!(request("rup 1 x >= 1 ;"), None);
    }

    #[test]
    fn help_pages_link_their_related_topics() {
        let page = Page::help("slack").unwrap();
        assert_eq!(page.title, ":help slack");
        let view = page.current();
        assert_eq!(view.lines[0], Line::new("", "Slack", Kind::Heading));
        assert!(view.lines[1].lead == "  " && view.lines[1].kind == Kind::Text);
        assert!(
            view.links()
                .iter()
                .all(|target| matches!(target, Target::Entry(_)))
        );
        assert!(!view.links().is_empty());
        assert!(Page::help("rule-rup").is_some_and(|page| page.title == ":help rup"));
        assert!(Page::help("nonsense").is_none());
    }
}
