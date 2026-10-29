//! `:where <id>|@label` — the constraint, and where it came from: the
//! formula, or the checked proof line that derived it. `:where line <n>` —
//! the constraints checked proof line `n` derived. Read-only; finding a
//! line's constraints may take one `veripb` run (see
//! [`Session::constraint_source`]). The TUI also shows both ends in its
//! panes, as clicking a line number or a Database ID does.

use crate::checker::parse::formula_line_constraints;
use crate::output::{Output, outln};
use crate::session::{ConstraintSource, Session};

/// What `:where` asks about.
pub enum Target {
    Constraint(usize),
    /// A proof line, by display line number.
    Line(usize),
}

/// Constraints and the line they came from.
pub struct Link {
    /// Ascending.
    pub ids: Vec<usize>,
    pub source: ConstraintSource,
}

/// Parses `:where`'s arguments.
pub fn parse(session: &Session, args: &str) -> Result<Target, String> {
    let words: Vec<&str> = args.split_whitespace().collect();
    match words[..] {
        ["line", n] => n
            .parse()
            .map(Target::Line)
            .map_err(|_| format!("not a line number: {n}")),
        [label] if label.starts_with('@') => match session.label_id(label) {
            Some(id) => usize::try_from(id)
                .map(Target::Constraint)
                .map_err(|_| format!("{label} names no constraint")),
            None => Err(format!("no label {label}")),
        },
        [id] => id
            .parse()
            .map(Target::Constraint)
            .map_err(|_| "usage :where <id>|@label|line <n>".to_string()),
        _ => Err("usage :where <id>|@label|line <n>".to_string()),
    }
}

/// Formula line `line`'s constraints (an index into `Session::formula`).
pub fn formula_link(session: &Session, line: usize) -> Link {
    let first = session.formula_first_ids()[line];
    let count = formula_line_constraints(&session.formula[line]);
    Link {
        ids: (first..first + count).collect(),
        source: ConstraintSource::Formula { line },
    }
}

/// Finds `target`'s link. `Ok(Err)` says why there is none.
pub fn link(session: &Session, target: &Target) -> anyhow::Result<Result<Link, String>> {
    Ok(match *target {
        Target::Constraint(id) => match session.constraint_source(id)? {
            Some(source) => Ok(Link {
                ids: vec![id],
                source,
            }),
            None => Err(format!(
                "No constraint {id} in the formula or from a checked line."
            )),
        },
        Target::Line(n) => {
            if session.buffer_index(n).is_none() {
                Err(format!("No proof line {n}."))
            } else if session.checked_index(n).is_none() {
                Err(format!("Line {n} is not checked yet."))
            } else {
                let ids = session.line_constraint_ids(n)?.unwrap_or_default();
                Ok(Link {
                    ids,
                    source: ConstraintSource::Proof { display_line: n },
                })
            }
        }
    })
}

pub fn run(session: &Session, args: &str, out: &mut dyn Output) {
    let target = match parse(session, args) {
        Ok(target) => target,
        Err(message) => {
            outln!(out, "Error: {message}");
            return;
        }
    };
    match link(session, &target) {
        Ok(Ok(link)) => print(session, &target, &link, out),
        Ok(Err(message)) => outln!(out, "{message}"),
        Err(err) => outln!(out, "Error: {err:#}"),
    }
}

fn print(session: &Session, target: &Target, link: &Link, out: &mut dyn Output) {
    let line_text = |display_line: usize| {
        let idx = session
            .buffer_index(display_line)
            .expect("a link's proof line is in the buffer");
        session.buffer[idx].trim()
    };
    match *target {
        Target::Constraint(_) => {
            for &id in &link.ids {
                outln!(out, "{}", constraint(session, id));
            }
            match link.source {
                ConstraintSource::Formula { line } => {
                    outln!(out, "  from the formula: {}", session.formula[line].trim())
                }
                ConstraintSource::Proof { display_line } => {
                    outln!(
                        out,
                        "  from line {display_line}: {}",
                        line_text(display_line)
                    )
                }
            }
        }
        Target::Line(n) => {
            outln!(out, "Line {n}: {}", line_text(n));
            if link.ids.is_empty() {
                outln!(out, "  derived no constraints");
            }
            for &id in &link.ids {
                outln!(out, "  derived {}", constraint(session, id));
            }
        }
    }
}

/// `ConstraintId <id>: <labels> <constraint> [core|derived]`, as `:show`
/// lists it, or that it has been deleted.
fn constraint(session: &Session, id: usize) -> String {
    let entry = session
        .database()
        .ok()
        .and_then(|database| database.get(id).cloned());
    match entry {
        Some(entry) => {
            let labels = session
                .labels_by_id()
                .get(&(id as isize))
                .map(|names| format!("{} ", names.join(" ")))
                .unwrap_or_default();
            let tag = if entry.is_core { "core" } else { "derived" };
            format!("ConstraintId {id}: {labels}{} [{tag}]", entry.text)
        }
        None => format!("ConstraintId {id} — no longer in the database"),
    }
}
