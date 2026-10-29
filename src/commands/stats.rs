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

/// `part` as a percentage of `whole`, e.g. `33.3%`.
fn percent(part: impl Into<f64>, whole: impl Into<f64>) -> String {
    let whole = whole.into();
    if whole == 0.0 {
        return "-".to_string();
    }
    format!("{:.1}%", 100.0 * part.into() / whole)
}

/// `n` and its percentage of `whole`: `4 (66.7%)`.
fn share(n: usize, whole: usize) -> String {
    format!("{n} ({})", percent(n as f64, whole as f64))
}

/// `bytes` in B, KiB, MiB or GiB, e.g. `95.3 MiB`.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn formula(session: &Session, out: &mut dyn Output) {
    let stats = stats::formula(session);
    let mut summary = format!(
        "Formula: {} ({}), {}",
        count(stats.lines, "line"),
        size(stats.bytes),
        count(stats.constraints, "constraint")
    );
    if stats.two_constraint_lines > 0 {
        summary.push_str(&format!(
            " ({} loading as two)",
            count(stats.two_constraint_lines, "line")
        ));
    }
    summary.push_str(&format!(", {}", count(stats.variables, "variable")));
    outln!(out, "{summary}");
    outln!(
        out,
        "  labelled lines: {}",
        share(stats.labelled, stats.lines)
    );
    if let Some((ids, bytes)) = &stats.longest {
        outln!(out, "  longest line: {} (ID {ids})", size(*bytes as u64));
    }
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
    let total = stats.constraints;
    outln!(
        out,
        "Database: {} ({})",
        count(total, "constraint"),
        size(stats.bytes)
    );
    outln!(
        out,
        "  core: {}, derived: {}",
        share(stats.core, total),
        share(total - stats.core, total)
    );
    outln!(
        out,
        "  from the formula: {} of its {} still there ({})",
        stats.from_formula,
        stats.formula_constraints,
        percent(stats.from_formula as f64, stats.formula_constraints as f64)
    );
    let mut labelled = format!("  labelled: {}", share(stats.labelled, total));
    if let Some(id) = stats.highest_id {
        labelled.push_str(&format!("; highest ID {id}"));
    }
    outln!(out, "{labelled}");
    if let Some((id, bytes)) = stats.longest {
        outln!(
            out,
            "  longest constraint: {} (ID {id})",
            size(bytes as u64)
        );
    }
    print_shapes(&stats.shapes, "constraint", out);
}

fn proof(session: &Session, out: &mut dyn Output) {
    let stats = stats::proof(session);
    let mut summary = format!(
        "Proof: {} ({}), {} checked ({})",
        count(stats.lines, "line"),
        size(stats.bytes),
        stats.checked,
        percent(stats.checked as f64, stats.lines as f64)
    );
    if stats.rejected {
        summary.push_str(&format!(
            ", line {} rejected",
            session.display_line(stats.checked)
        ));
    }
    outln!(out, "{summary}");
    outln!(
        out,
        "  labelled lines: {}; {}",
        share(stats.labelled, stats.lines),
        count(stats.breakpoints, "breakpoint")
    );
    if let Some((line, bytes)) = stats.longest {
        outln!(out, "  longest line: {} (line {line})", size(bytes as u64));
    }
    if stats.rules.is_empty() {
        return;
    }
    let mut table = vec![[
        "rule".to_string(),
        "lines".to_string(),
        "%".to_string(),
        "bytes".to_string(),
        "%".to_string(),
        "checked".to_string(),
    ]];
    for rule in &stats.rules {
        table.push([
            rule.rule.clone(),
            rule.lines.to_string(),
            percent(rule.lines as f64, stats.lines as f64),
            size(rule.bytes),
            percent(rule.bytes as f64, stats.bytes as f64),
            rule.checked.to_string(),
        ]);
    }
    let widths: Vec<usize> = (0..6)
        .map(|column| table.iter().map(|row| row[column].len()).max().unwrap_or(0))
        .collect();
    for row in &table {
        // The rule name on the left, the numbers right-aligned.
        let mut line = format!("  {:<w$}", row[0], w = widths[0]);
        for (cell, width) in row.iter().zip(&widths).skip(1) {
            line.push_str(&format!("  {cell:>width$}"));
        }
        outln!(out, "{line}");
    }
}

/// The kinds, terms per `unit` and largest coefficient of `shapes`.
fn print_shapes(shapes: &Shapes, unit: &str, out: &mut dyn Output) {
    let total: usize = shapes.kinds.iter().map(|(_, n)| n).sum();
    if total == 0 {
        return;
    }
    let kinds: Vec<String> = shapes
        .kinds
        .iter()
        .map(|(kind, n)| format!("{} {}", kind.name(), share(*n, total)))
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
