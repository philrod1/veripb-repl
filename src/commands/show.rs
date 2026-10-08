//! `:show [filters]` — list database constraints, optionally filtered by
//! variable name (or glob, e.g. `i[vertex0]*`), ID/ID-range, label (`@name`,
//! or an `@`-prefixed glob), and/or `core`/`derived` status (combinable).

use crate::output::{Output, outln};
use crate::session::Session;

/// A parsed `:show` filter. Filters combine with AND: a constraint must
/// match every filter given on the line to be printed.
enum ShowFilter {
    Variable(String),
    /// A bare token containing `*` (see [`glob_match`]): matches a
    /// constraint mentioning any variable whose name fits the pattern.
    VariableGlob(String),
    IdRange(usize, usize),
    Core(bool),
    /// An `@`-prefixed token containing `*`: matches a constraint carrying
    /// any label fitting the pattern.
    LabelGlob(String),
}

/// Returns whether `text` matches `pattern`, where `*` matches any run of
/// characters (including none) and everything else matches literally — the
/// standard greedy `*`-only wildcard algorithm (no `?`, no character
/// classes).
fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while ti < txt.len() {
        if pi < pat.len() && pat[pi] == txt[ti] {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
            backtrack = Some((pi, ti));
            pi += 1;
        } else if let Some((star, matched)) = backtrack {
            pi = star + 1;
            ti = matched + 1;
            backtrack = Some((star, ti));
        } else {
            return false;
        }
    }
    pat[pi..].iter().all(|&c| c == '*')
}

fn parse_show_filters(args: &str, session: &Session) -> Result<Vec<ShowFilter>, String> {
    args.split_whitespace()
        .map(|tok| match tok {
            "core" => Ok(ShowFilter::Core(true)),
            "derived" => Ok(ShowFilter::Core(false)),
            tok if tok.starts_with('@') && tok.contains('*') => {
                Ok(ShowFilter::LabelGlob(tok.to_string()))
            }
            // An exact label resolves to the one constraint ID it names,
            // via the plain ID filter.
            tok if tok.starts_with('@') => {
                let id = session
                    .label_ids()
                    .get(tok)
                    .copied()
                    .ok_or_else(|| format!("no label named '{tok}'"))?;
                let id = usize::try_from(id)
                    .map_err(|_| format!("label '{tok}' names constraint {id}"))?;
                Ok(ShowFilter::IdRange(id, id))
            }
            tok => {
                if let Some((lo, hi)) = tok.split_once('-') {
                    let lo: usize = lo
                        .parse()
                        .map_err(|_| format!("invalid ID range '{tok}'"))?;
                    let hi: usize = hi
                        .parse()
                        .map_err(|_| format!("invalid ID range '{tok}'"))?;
                    Ok(ShowFilter::IdRange(lo, hi))
                } else if let Ok(id) = tok.parse::<usize>() {
                    Ok(ShowFilter::IdRange(id, id))
                } else if tok.contains('*') {
                    Ok(ShowFilter::VariableGlob(tok.to_string()))
                } else if session.variables.contains(tok) {
                    Ok(ShowFilter::Variable(tok.to_string()))
                } else {
                    Err(format!("no variable named '{tok}'"))
                }
            }
        })
        .collect()
}

/// Whether entry `id` (mentioning variables `mentioned`, core status
/// `is_core`) matches every filter in `filters`.
fn matches_filters(
    id: usize,
    mentioned: &[String],
    is_core: bool,
    filters: &[ShowFilter],
    labels_by_id: &ahash::AHashMap<isize, Vec<String>>,
) -> bool {
    filters.iter().all(|filter| match filter {
        ShowFilter::Variable(name) => mentioned.iter().any(|v| v == name),
        ShowFilter::VariableGlob(pattern) => mentioned.iter().any(|v| glob_match(pattern, v)),
        ShowFilter::IdRange(lo, hi) => id >= *lo && id <= *hi,
        ShowFilter::Core(want_core) => is_core == *want_core,
        ShowFilter::LabelGlob(pattern) => labels_by_id
            .get(&(id as isize))
            .is_some_and(|names| names.iter().any(|name| glob_match(pattern, name))),
    })
}

pub fn run(session: &Session, args: &str, out: &mut dyn Output) {
    let filters = match parse_show_filters(args, session) {
        Ok(filters) => filters,
        Err(msg) => {
            outln!(out, "Error: {msg}");
            return;
        }
    };

    let database = match session.database() {
        Ok(database) => database,
        Err(err) => {
            outln!(out, "Error: {err:#}");
            return;
        }
    };

    let labels_by_id = session.labels_by_id();
    let mut shown = 0;
    for entry in &database.entries {
        let mentioned = session.variables.mentioned(&entry.text);
        if !matches_filters(entry.id, &mentioned, entry.is_core, &filters, &labels_by_id) {
            continue;
        }
        let tag = if entry.is_core { "core" } else { "derived" };
        let labels = labels_by_id
            .get(&(entry.id as isize))
            .map(|names| format!("{} ", names.join(" ")))
            .unwrap_or_default();
        outln!(
            out,
            "  ConstraintId {}: {labels}{} [{tag}]",
            entry.id,
            entry.text
        );
        shown += 1;
    }
    if shown == 0 {
        outln!(out, "No constraints match.");
    }
}
