//! `:show [filters]` — list database constraints, optionally filtered by
//! variable name (or glob, e.g. `i[vertex0]*`), ID/ID-range, label (`@name`,
//! or an `@`-prefixed glob), and/or `core`/`derived` status (combinable).

use veripb_formula::prelude::*;

use crate::output::{Output, outln};
use crate::session::Session;

/// A parsed `:show` filter. Filters combine with AND: a constraint must
/// match every filter given on the line to be printed.
enum ShowFilter {
    Variable(VarIdx),
    /// A bare token containing `*` (see [`glob_match`]) — matches if the
    /// constraint mentions *any* variable whose name fits the pattern.
    /// An exact name resolves to a single [`Variable`](ShowFilter::Variable)
    /// instead, since it's cheaper and gives a clean "no such variable"
    /// error a glob can't (no matches just means an empty result).
    VariableGlob(String),
    IdRange(usize, usize),
    Core(bool),
    /// `@`-prefixed with at least one `*` in it — the label equivalent of
    /// `VariableGlob`, and for the same reason an exact `@name` resolves
    /// to an [`IdRange`](ShowFilter::IdRange) instead: labels are unique,
    /// so there's nothing left to match once one's found by name.
    LabelGlob(String),
}

/// Whether `text` matches `pattern`, where `*` matches any run of
/// characters (including none) and everything else must match literally —
/// the standard greedy `*`-only wildcard algorithm (no `?`, no character
/// classes: `:show`'s labels don't need more than "loosen an exact name
/// into a prefix/suffix/contains match").
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
    let var_names = &session.current_checker.context.var_names;
    args.split_whitespace()
        .map(|tok| match tok {
            "core" => Ok(ShowFilter::Core(true)),
            "derived" => Ok(ShowFilter::Core(false)),
            // A `*`-glob can match any number of labels, so unlike an
            // exact one it can't resolve to a single ID up front — stays
            // a pattern, matched per-entry once labels are known below.
            tok if tok.starts_with('@') && tok.contains('*') => {
                Ok(ShowFilter::LabelGlob(tok.to_string()))
            }
            // An exact label filters down to the one constraint it
            // currently names — resolved here against the live label
            // map, so it reuses the plain ID filter underneath.
            tok if tok.starts_with('@') => {
                let id = session
                    .label_map
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
                } else {
                    var_names
                        .get_idx(tok)
                        .map(ShowFilter::Variable)
                        .ok_or_else(|| format!("no variable named '{tok}'"))
                }
            }
        })
        .collect()
}

fn constraint_mentions_var(constraint: &PBConstraintEnum, var: VarIdx) -> bool {
    (0..constraint.len())
        .filter_map(|i| constraint.get_lit(i))
        .any(|lit| lit.get_var() == var)
}

fn constraint_mentions_var_glob(
    constraint: &PBConstraintEnum,
    pattern: &str,
    var_names: &VarNameManager,
) -> bool {
    (0..constraint.len())
        .filter_map(|i| constraint.get_lit(i))
        .any(|lit| glob_match(pattern, var_names.get_name(lit.get_var())))
}

fn matches_filters(
    id: usize,
    entry: &DBConstraint,
    filters: &[ShowFilter],
    labels_by_id: &ahash::AHashMap<isize, Vec<String>>,
    var_names: &VarNameManager,
) -> bool {
    filters.iter().all(|filter| match filter {
        ShowFilter::Variable(var) => constraint_mentions_var(&entry.constraint, *var),
        ShowFilter::VariableGlob(pattern) => {
            constraint_mentions_var_glob(&entry.constraint, pattern, var_names)
        }
        ShowFilter::IdRange(lo, hi) => id >= *lo && id <= *hi,
        ShowFilter::Core(want_core) => entry.is_core_constraint() == *want_core,
        ShowFilter::LabelGlob(pattern) => labels_by_id
            .get(&(id as isize))
            .is_some_and(|names| names.iter().any(|name| glob_match(pattern, name))),
    })
}

pub fn run(session: &Session, args: &str, out: &mut dyn Output) {
    let checker = &session.current_checker;
    let var_names = &checker.context.var_names;
    let filters = match parse_show_filters(args, session) {
        Ok(filters) => filters,
        Err(msg) => {
            outln!(out, "Error: {msg}");
            return;
        }
    };

    let labels_by_id = session.labels_by_id();
    let mut shown = 0;
    for (id, entry) in checker.database.entries.iter().enumerate() {
        let Some(entry) = entry else { continue };
        if !matches_filters(id, entry, &filters, &labels_by_id, var_names) {
            continue;
        }
        let tag = if entry.is_core_constraint() {
            "core"
        } else {
            "derived"
        };
        let labels = labels_by_id
            .get(&(id as isize))
            .map(|names| format!("{} ", names.join(" ")))
            .unwrap_or_default();
        outln!(
            out,
            "  ConstraintId {id}: {labels}{} [{tag}]",
            entry.constraint.to_pretty_string(var_names)
        );
        shown += 1;
    }
    if shown == 0 {
        outln!(out, "No constraints match.");
    }
}
