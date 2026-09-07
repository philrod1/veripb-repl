//! Turn `invoke`'s raw subprocess output into `types`'s plain shapes.


use super::invoke::RawInvocation;
use super::types::{CheckOutcome, RupHint};

/// The minimized hint list from the *last* `rup` step in an elaborated
/// proof: `None` if the text contains no `rup` line at all, matching
/// `:why`'s existing "line X doesn't look like a rup step" case.
///
/// Relies on the same construction the in-process code always used:
/// callers elaborate a proof truncated to end exactly at the line
/// they're asking about, so whatever `rup` line comes *last* in the
/// output is unambiguously the one being asked about — no line-number
/// matching needed here at all.
pub fn last_rup_hints(elaborated_proof: &str) -> Option<Vec<RupHint>> {
    let last_rup_line = elaborated_proof.lines().rev().find(|line| is_rup_line(line))?;
    let (_constraint, hints) = last_rup_line.split_once(':')?;
    let hints = hints.trim();
    let hints = hints.strip_suffix(';').unwrap_or(hints);
    Some(
        hints
            .split_whitespace()
            .filter_map(|tok| {
                if tok == "~" {
                    Some(RupHint::NegatedPremise)
                } else {
                    // An unparsable token is dropped rather than aborting
                    tok.parse::<usize>().ok().map(RupHint::ConstraintId)
                }
            })
            .collect(),
    )
}

/// Whether `line` is a `rup` rule line, optionally preceded by a
/// `@label` — the same exact token match convention (not a prefix check)
/// the REPL already uses to recognise an `a`-rule elsewhere, so a rule
/// that merely starts with the letters "rup" can never false-match.
fn is_rup_line(line: &str) -> bool {
    let mut tokens = line.split_whitespace();
    match tokens.next() {
        Some(t) if t.starts_with('@') => tokens.next() == Some("rup"),
        Some(t) => t == "rup",
        None => false,
    }
}

/// Best-effort: did checking succeed, and if not, which line broke it?
pub fn check_outcome(raw: &RawInvocation) -> anyhow::Result<CheckOutcome> {

    let combined = format!("{}{}", raw.stdout, raw.stderr);

    if raw.success {
        return Ok(CheckOutcome::Accepted { trace: combined });
    }

    match find_line_number(&combined) {
        Some(line) => Ok(CheckOutcome::Rejected {
            line,
            message: combined.clone(),
            trace: combined,
        }),
        None => anyhow::bail!(
            "veripb exited with a failure (code {:?}) but this parser couldn't find a `line \
             N` it could point to — this is exactly the unverified case this module's own \
             docs warn about (possibly \"ran out of input\", not a real rejection, or possibly \
             a real error in a format this parser doesn't recognize yet). Raw output:\n\
             --- stdout ---\n{}\n--- stderr ---\n{}",
            raw.code,
            raw.stdout,
            raw.stderr,
        ),
    }
}

/// A starting guess for where a rejection's line number shows up in
/// `veripb`'s own text, based on the one thing actually known for
/// certain: every piece of this REPL's *own* user-facing text already
/// phrases a rejection as "line N: ..."
fn find_line_number(text: &str) -> Option<usize> {
    let after = text.find("line ").map(|idx| &text[idx + "line ".len()..])?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}
