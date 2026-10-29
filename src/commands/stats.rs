//! `:stats [formula|database|proof]` — statistics for one pane's content,
//! or all three (see [`crate::stats`]). Read-only; the database's may take
//! one `veripb` run, as `:show` can.

use crate::output::{Output, outln};
use crate::session::Session;
use crate::stats::{self, Shapes};

pub fn run(session: &Session, args: &str, out: &mut dyn Output) {
    match args.trim() {
        "" => {
            formula(session, out);
            database(session, out);
            proof(session, out);
        }
        "formula" => formula(session, out),
        "database" => database(session, out),
        "proof" => proof(session, out),
        _ => outln!(out, "Error: usage :stats [formula|database|proof]"),
    }
}

/// `n` and `noun`, with an `s` unless `n` is 1.
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn formula(session: &Session, out: &mut dyn Output) {
    let stats = stats::formula(session);
    let mut summary = format!(
        "Formula: {}, {}",
        count(stats.lines, "line"),
        count(stats.constraints, "constraint")
    );
    if stats.two_constraint_lines > 0 {
        summary.push_str(&format!(
            " ({} loading as two)",
            count(stats.two_constraint_lines, "line")
        ));
    }
    summary.push_str(&format!(
        ", {} labelled, {}",
        stats.labelled,
        count(stats.variables, "variable")
    ));
    outln!(out, "{summary}");
    if let Some(terms) = stats.objective_terms {
        outln!(out, "  objective: {}", count(terms, "term"));
    }
    if let Some(preserved) = stats.preserved {
        outln!(out, "  preserved: {}", count(preserved, "variable"));
    }
    print_shapes(&stats.shapes, "line", out);
}

fn database(session: &Session, out: &mut dyn Output) {
    let stats = match stats::database(session) {
        Ok(stats) => stats,
        Err(err) => {
            outln!(out, "Database: Error: {err:#}");
            return;
        }
    };
    let mut summary = format!(
        "Database: {}: {} core, {} derived; {} of the formula's {} still there; {} labelled",
        count(stats.constraints, "constraint"),
        stats.core,
        stats.constraints - stats.core,
        stats.from_formula,
        stats.formula_constraints,
        stats.labelled
    );
    if let Some(id) = stats.highest_id {
        summary.push_str(&format!("; highest ID {id}"));
    }
    outln!(out, "{summary}");
    print_shapes(&stats.shapes, "constraint", out);
}

fn proof(session: &Session, out: &mut dyn Output) {
    let stats = stats::proof(session);
    let mut summary = format!(
        "Proof: {}, {} checked",
        count(stats.lines, "line"),
        stats.checked
    );
    if stats.rejected {
        summary.push_str(&format!(
            ", line {} rejected",
            session.display_line(stats.checked)
        ));
    }
    summary.push_str(&format!(
        "; {} labelled, {}",
        stats.labelled,
        count(stats.breakpoints, "breakpoint")
    ));
    outln!(out, "{summary}");
    if stats.rules.is_empty() {
        return;
    }
    let width = stats
        .rules
        .iter()
        .map(|(rule, _, _)| rule.len())
        .max()
        .unwrap_or(0)
        .max("rule".len());
    let lines_w = stats.lines.to_string().len().max("lines".len());
    outln!(out, "  {:<width$}  {:>lines_w$}  checked", "rule", "lines");
    for (rule, lines, checked) in &stats.rules {
        outln!(out, "  {rule:<width$}  {lines:>lines_w$}  {checked:>7}");
    }
}

/// The kinds, terms per `unit` and largest coefficient of `shapes`.
fn print_shapes(shapes: &Shapes, unit: &str, out: &mut dyn Output) {
    if shapes.kinds.is_empty() {
        return;
    }
    let kinds: Vec<String> = shapes
        .kinds
        .iter()
        .map(|(kind, n)| format!("{} {n}", kind.name()))
        .collect();
    outln!(out, "  kinds: {}", kinds.join(", "));
    if let Some(terms) = &shapes.terms {
        outln!(
            out,
            "  terms per {unit}: min {}, median {}, max {}, mean {:.1}",
            terms.min,
            terms.median,
            terms.max,
            terms.mean
        );
    }
    if let Some(largest) = &shapes.largest_coefficient {
        outln!(out, "  largest coefficient: {largest}");
    }
}
