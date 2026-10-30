//! Plain-English explanations of checked proof lines, written for students:
//! what the rule means, then this line's derivation step by step. Built from
//! the same checker output `:explain` shows.
//!
//! Covers `rup`, `pol`, `red`, `a`, `setlvl` and `wiplvl`; other rules,
//! rejected lines and lines the narration can't follow give `None`.
//!
//! Text is in [`super::markup`]: constraints and code in backticks, links
//! to a glossary entry as `[text](g:<id>)` and to a constraint as
//! `[text](c:<id>)`.

use std::collections::HashMap;

use crate::checker::parse::{
    last_line_constraint_ids, pol_table_with_typed_operands, rule_keyword, split_rup_hints,
};
use crate::checker::{Database, RupHint};
use crate::session::Session;

/// One explanation: a title and its blocks, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Explanation {
    pub title: String,
    pub blocks: Vec<Block>,
    /// For a `rup` line, the constraints the step-by-step replay used, as
    /// `rup` hints (see [`rup_hints`]); `None` otherwise.
    pub replay_hints: Option<Vec<RupHint>>,
}

impl Explanation {
    /// The explanation as plain text (see [`super::markup::plain`]): the
    /// title, then each block after a blank line, steps numbered.
    pub fn text_lines(&self) -> Vec<String> {
        let mut lines = vec![self.title.clone()];
        for block in &self.blocks {
            lines.push(String::new());
            match block {
                Block::Paragraph(text) => lines.push(super::markup::plain(text)),
                Block::Note(text) => lines.push(format!("Note: {}", super::markup::plain(text))),
                Block::Steps(items) => lines.extend(
                    items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| format!("  {}. {}", i + 1, super::markup::plain(item))),
                ),
            }
        }
        lines
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Paragraph(String),
    /// An aside, such as how to read `~x`.
    Note(String),
    /// Numbered steps.
    Steps(Vec<String>),
}

/// Explains checked proof line `n` (a display line number), or `None` for
/// a line it doesn't cover.
pub fn explain(session: &Session, n: usize) -> Option<Explanation> {
    let idx = session.checked_index(n)?;
    let line = session.buffer[idx].as_str();
    let doc = match rule_keyword(line)? {
        "rup" => rup(session, n, idx, line),
        "pol" => pol(session, n, line),
        "red" => red(session, n, idx, line),
        "a" => assertion(session, n, line),
        "setlvl" => set_level(session, n, idx, line),
        "wiplvl" => wipe_level(session, n, idx, line),
        _ => None,
    }?;
    Some(Explanation {
        title: doc.title,
        blocks: doc.blocks,
        replay_hints: doc.hints,
    })
}

// ---- Document ----

struct Doc {
    title: String,
    blocks: Vec<Block>,
    hints: Option<Vec<RupHint>>,
}

impl Doc {
    fn new(title: String) -> Doc {
        Doc {
            title,
            blocks: Vec::new(),
            hints: None,
        }
    }
    fn p(&mut self, text: impl Into<String>) {
        self.blocks.push(Block::Paragraph(text.into()));
    }
    fn note(&mut self, text: impl Into<String>) {
        self.blocks.push(Block::Note(text.into()));
    }
    fn steps(&mut self, items: Vec<String>) {
        if !items.is_empty() {
            self.blocks.push(Block::Steps(items));
        }
    }
    /// The `~x` reading, when any shown constraint uses a negated literal.
    fn negation_note(&mut self, shown: &[&Pb]) {
        if shown
            .iter()
            .any(|pb| pb.terms.iter().any(|(_, lit)| lit.neg))
        {
            self.note("`~x` means “not x”: it is 1 when x is 0, and 0 when x is 1 (see [literals](g:variable)).");
        }
    }
}

// ---- Constraints ----

#[derive(Clone, PartialEq)]
struct Lit {
    var: String,
    neg: bool,
}

impl Lit {
    fn parse(token: &str) -> Option<Lit> {
        let (neg, var) = match token.strip_prefix('~') {
            Some(var) => (true, var),
            None => (false, token),
        };
        let first = var.chars().next()?;
        (!first.is_ascii_digit() && !"+-@>=;:~".contains(first)).then(|| Lit {
            var: var.to_string(),
            neg,
        })
    }
    fn negated(&self) -> Lit {
        Lit {
            var: self.var.clone(),
            neg: !self.neg,
        }
    }
    fn text(&self) -> String {
        if self.neg {
            format!("~{}", self.var)
        } else {
            self.var.clone()
        }
    }
    /// The assignment that makes this literal true, e.g. `x1 = 0` for `~x1`.
    fn made_true(&self) -> String {
        format!("{} = {}", self.var, if self.neg { 0 } else { 1 })
    }
}

/// `Σ coef·lit >= rhs` with positive coefficients.
#[derive(Clone)]
struct Pb {
    terms: Vec<(i128, Lit)>,
    rhs: i128,
}

impl Pb {
    /// veripb's `1 ~x1 2 x3 >= 1` form, or `<=` (a trailing `;` is ignored).
    fn parse(text: &str) -> Option<Pb> {
        let text = text.trim();
        let text = text.strip_suffix(';').unwrap_or(text);
        if text.contains("==>") || text.contains("<==") {
            return None;
        }
        if let Some((lhs, rhs)) = text.split_once("<=") {
            // `Σ a·l <= k` is `Σ -a·l >= -k`.
            let negated: Vec<String> = lhs
                .split_whitespace()
                .collect::<Vec<_>>()
                .chunks(2)
                .map(|pair| match pair {
                    [coef, lit] => Some(format!(
                        "{} {lit}",
                        -coef
                            .strip_prefix('+')
                            .unwrap_or(coef)
                            .parse::<i128>()
                            .ok()?
                    )),
                    _ => None,
                })
                .collect::<Option<_>>()?;
            return Pb::parse(&format!(
                "{} >= {}",
                negated.join(" "),
                -rhs.trim().parse::<i128>().ok()?
            ));
        }
        let (lhs, rhs) = text.split_once(">=")?;
        let mut pb = Pb {
            terms: Vec::new(),
            rhs: rhs.trim().parse().ok()?,
        };
        let tokens: Vec<&str> = lhs.split_whitespace().collect();
        if !tokens.len().is_multiple_of(2) {
            return None;
        }
        for pair in tokens.chunks(2) {
            let coef: i128 = pair[0].strip_prefix('+').unwrap_or(pair[0]).parse().ok()?;
            let lit = Lit::parse(pair[1])?;
            match coef {
                0 => {}
                c if c < 0 => {
                    pb.rhs -= c;
                    pb.terms.push((-c, lit.negated()));
                }
                c => pb.terms.push((c, lit)),
            }
        }
        Some(pb)
    }

    /// `~x1 + 2 x3 ≥ 1`.
    fn pretty(&self) -> String {
        let lhs = if self.terms.is_empty() {
            "0".to_string()
        } else {
            self.terms
                .iter()
                .map(|(c, lit)| {
                    if *c == 1 {
                        lit.text()
                    } else {
                        format!("{c} {}", lit.text())
                    }
                })
                .collect::<Vec<_>>()
                .join(" + ")
        };
        format!("{lhs} ≥ {}", self.rhs)
    }

    fn code(&self) -> String {
        format!("`{}`", self.pretty())
    }

    /// `Σ a·l <= rhs - 1`, written as `Σ a·~l >= Σa - rhs + 1`.
    fn negation(&self) -> Pb {
        let total: i128 = self.terms.iter().map(|(c, _)| c).sum();
        Pb {
            terms: self
                .terms
                .iter()
                .map(|(c, lit)| (*c, lit.negated()))
                .collect(),
            rhs: total - self.rhs + 1,
        }
    }

    fn is_contradiction(&self) -> bool {
        self.terms.is_empty() && self.rhs > 0
    }

    fn is_clause(&self) -> bool {
        self.rhs == 1 && self.terms.iter().all(|(c, _)| *c == 1)
    }
}

/// Pretty form of a constraint as veripb printed it, or the text as is.
fn pretty_text(text: &str) -> String {
    match Pb::parse(text) {
        Some(pb) => pb.code(),
        None => format!("`{}`", text.trim().trim_end_matches(';').trim()),
    }
}

fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// `@label` (constraint 3), or constraint 3, linked to the constraint.
fn constraint_name(labels: &HashMap<usize, String>, id: usize) -> String {
    match labels.get(&id) {
        Some(label) => format!("`{label}` ([constraint {id}](c:{id}))"),
        None => format!("[constraint {id}](c:{id})"),
    }
}

fn first_labels(session: &Session) -> HashMap<usize, String> {
    session
        .labels_by_id()
        .iter()
        .filter_map(|(id, names)| Some((usize::try_from(*id).ok()?, names.first()?.clone())))
        .collect()
}

/// A rule's constraint: the line's text after its labels and keyword, up to
/// `:` or `;`.
fn rule_constraint(line: &str) -> Option<Pb> {
    let body = line.split(':').next()?;
    let text: Vec<&str> = body
        .split_whitespace()
        .skip_while(|t| t.starts_with('@'))
        .skip(1)
        .collect();
    Pb::parse(&text.join(" "))
}

/// The constraint id the checker gave line `n`, from its trace.
fn added_id(session: &Session, n: usize) -> Option<usize> {
    let trace = session.explain_line(n).ok()?.ok()?;
    last_line_constraint_ids(&trace).last().copied()
}

fn added_sentence(id: Option<usize>, labels: &HashMap<usize, String>) -> String {
    match id {
        Some(id) => format!(
            "It is added to the database as {}.",
            constraint_name(labels, id)
        ),
        None => "It is added to the database.".to_string(),
    }
}

// ---- rup ----

/// Unit propagation over `pb`: a conflict when the literals not yet false
/// can't reach the right-hand side, else every unassigned literal whose
/// coefficient exceeds the slack is forced true.
enum Propagation {
    Conflict {
        max: i128,
    },
    Forced {
        max: i128,
        slack: i128,
        lits: Vec<Lit>,
    },
    Nothing,
}

fn propagate(pb: &Pb, values: &HashMap<String, bool>) -> Propagation {
    let value = |lit: &Lit| values.get(&lit.var).map(|v| *v != lit.neg);
    let max: i128 = pb
        .terms
        .iter()
        .filter(|(_, l)| value(l) != Some(false))
        .map(|(c, _)| c)
        .sum();
    let slack = max - pb.rhs;
    if slack < 0 {
        return Propagation::Conflict { max };
    }
    let lits: Vec<Lit> = pb
        .terms
        .iter()
        .filter(|(c, l)| value(l).is_none() && *c > slack)
        .map(|(_, l)| l.clone())
        .collect();
    if lits.is_empty() {
        Propagation::Nothing
    } else {
        Propagation::Forced { max, slack, lits }
    }
}

/// The variables of `pb` whose current values make one of its literals false.
fn falsified(pb: &Pb, values: &HashMap<String, bool>) -> Vec<String> {
    pb.terms
        .iter()
        .filter(|(_, l)| values.get(&l.var).is_some_and(|v| *v == l.neg))
        .map(|(_, l)| l.var.clone())
        .collect()
}

/// The database just before buffer line `idx`.
fn database_before(session: &Session, idx: usize) -> Option<Database> {
    session.database_before(idx).ok()
}

/// One propagation (or the conflict) during the replay.
struct Event {
    /// Index into the replay's constraints; 0 is the supposition.
    constraint: usize,
    /// The variables whose values triggered it.
    context: Vec<String>,
    outcome: Propagation,
}

/// Largest replay attempted, in constraints visited: each round visits
/// every constraint.
const REPLAY_LIMIT: usize = 20_000_000;

/// [`replay`]'s result.
enum Replay {
    /// Every event, the conflict last.
    Conflict(Vec<Event>),
    /// Propagation stopped without a conflict.
    Fixpoint,
    /// The replay passed [`REPLAY_LIMIT`].
    TooLong,
}

/// Unit propagation from `constraints` (the supposition first, then the
/// database) to a conflict, a round at a time: each round checks every
/// constraint against the values the earlier rounds fixed, so the conflict
/// found is one with the fewest rounds of forced values before it, as
/// short an account as propagation gives.
fn replay(constraints: &[(Option<usize>, Pb)]) -> Replay {
    let mut values: HashMap<String, bool> = HashMap::new();
    let mut events = Vec::new();
    let mut work = 0;
    loop {
        work += constraints.len();
        if work > REPLAY_LIMIT {
            return Replay::TooLong;
        }
        // The values this round forces, applied once it is over.
        let mut forced: HashMap<String, bool> = HashMap::new();
        for (i, (_, pb)) in constraints.iter().enumerate() {
            let mut outcome = propagate(pb, &values);
            if let Propagation::Forced { lits, .. } = &mut outcome {
                lits.retain(|lit| !forced.contains_key(&lit.var));
                if lits.is_empty() {
                    continue;
                }
                for lit in lits.iter() {
                    forced.insert(lit.var.clone(), !lit.neg);
                }
            }
            if matches!(outcome, Propagation::Nothing) {
                continue;
            }
            let conflict = matches!(outcome, Propagation::Conflict { .. });
            let context = falsified(pb, &values);
            events.push(Event {
                constraint: i,
                context,
                outcome,
            });
            if conflict {
                return Replay::Conflict(events);
            }
        }
        if forced.is_empty() {
            return Replay::Fixpoint;
        }
        values.extend(forced);
    }
}

/// The events the conflict depends on, in order: the conflict's own
/// context, the events that set those variables, and so on back.
fn needed(events: &[Event]) -> Vec<&Event> {
    let mut setter: HashMap<&str, usize> = HashMap::new();
    for (i, event) in events.iter().enumerate() {
        if let Propagation::Forced { lits, .. } = &event.outcome {
            for lit in lits {
                setter.entry(lit.var.as_str()).or_insert(i);
            }
        }
    }
    let mut keep = vec![false; events.len()];
    let mut stack = vec![events.len() - 1];
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut keep[i], true) {
            continue;
        }
        stack.extend(
            events[i]
                .context
                .iter()
                .filter_map(|var| setter.get(var.as_str()).copied()),
        );
    }
    events
        .iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(e, _)| e)
        .collect()
}

/// The constraints [`needed`] lists, as `rup` hints: `~` for the
/// supposition, else constraint IDs, each once.
fn hints(constraints: &[(Option<usize>, Pb)], needed: &[&Event]) -> Vec<RupHint> {
    let mut hints = Vec::new();
    for event in needed {
        let hint = match constraints[event.constraint].0 {
            None => RupHint::NegatedPremise,
            Some(id) => RupHint::ConstraintId(id),
        };
        if !hints.contains(&hint) {
            hints.push(hint);
        }
    }
    hints
}

/// A replay's constraints: the supposition (`None`), then database entries
/// by ID.
type Constraints = Vec<(Option<usize>, Pb)>;

/// `rup` line `line`'s claim and the constraints its replay runs over: the
/// claim's negation first, then `database`. `None` if any doesn't parse.
fn rup_constraints(line: &str, database: &Database) -> Option<(Pb, Constraints)> {
    let (body, _) = split_rup_hints(line);
    let claim = rule_constraint(body)?;
    let mut constraints: Constraints = vec![(None, claim.negation())];
    for entry in &database.entries {
        constraints.push((Some(entry.id), Pb::parse(&entry.text)?));
    }
    Some((claim, constraints))
}

/// What unit propagation shows about a `rup` line's constraint, without its
/// typed hints, over `database`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RupReplay {
    /// It reached a conflict using these constraints, as `rup` hints in the
    /// order they were used (`~` for the line's negation).
    Hints(Vec<RupHint>),
    /// It stopped without a conflict: the constraint isn't implied by
    /// unit propagation.
    NotImplied,
    /// No answer: the line isn't a constraint the replay reads, or the
    /// replay passed its limit.
    Unknown,
}

/// Replays `rup` line `line` over `database` as [`rup`]'s explanation does:
/// its hints are those of the shortest conflict propagation finds.
pub fn rup_hints(line: &str, database: &Database) -> RupReplay {
    let Some((_, constraints)) = rup_constraints(line, database) else {
        return RupReplay::Unknown;
    };
    match replay(&constraints) {
        Replay::Conflict(events) => RupReplay::Hints(hints(&constraints, &needed(&events))),
        Replay::Fixpoint => RupReplay::NotImplied,
        Replay::TooLong => RupReplay::Unknown,
    }
}

fn rup(session: &Session, n: usize, idx: usize, line: &str) -> Option<Doc> {
    let labels = first_labels(session);
    let database = database_before(session, idx)?;
    let (claim, constraints) = rup_constraints(line, &database)?;

    let mut doc = Doc::new(format!("Line {n}: reverse unit propagation (rup)"));
    doc.p(
        "[Reverse unit propagation](g:rup) proves a constraint by contradiction. Suppose it is false, \
         then follow the consequences one forced value at a time: whenever a constraint can only \
         be satisfied in one way, that value is forced. If this reaches a constraint that can no \
         longer be satisfied at all, the supposition was impossible, so the constraint holds.",
    );
    let Replay::Conflict(events) = replay(&constraints) else {
        doc.note(
            "The step-by-step replay did not reach the contradiction veripb found; \
             the checker view shows veripb's own account.",
        );
        return Some(doc);
    };
    let needed = needed(&events);
    doc.hints = Some(hints(&constraints, &needed));
    if let Some(last) = events.last()
        && let (Some(id), pb) = &constraints[last.constraint]
        && pb.is_contradiction()
    {
        doc.p(format!(
            "The claim is {}. But the database already contains {}, {}, which no assignment \
             satisfies. From a contradiction every constraint follows, this one included. {}",
            claim.code(),
            constraint_name(&labels, *id),
            pb.code(),
            added_sentence(added_id(session, n), &labels)
        ));
        doc.negation_note(&[&claim]);
        return Some(doc);
    }
    if claim.is_contradiction() {
        doc.p(format!(
            "This line claims {}, which no assignment satisfies. So it says that the constraints \
             so far already contradict each other, and unit propagation alone shows it.",
            claim.code()
        ));
    } else {
        doc.p(format!("The claim: {}.", claim.code()));
    }

    let mut values: HashMap<String, bool> = HashMap::new();
    let mut steps = Vec::new();
    let mut shown: Vec<&Pb> = vec![&claim];
    let suppose = format!(
        "Suppose {} is false. Then its [opposite](g:negation) holds: {}.",
        claim.code(),
        claim.negation().code()
    );
    if !claim.is_contradiction() && needed.first().is_none_or(|e| e.constraint != 0) {
        steps.push(suppose.clone());
    }
    for event in needed {
        let (id, pb) = &constraints[event.constraint];
        shown.push(pb);
        let context: Vec<String> = event
            .context
            .iter()
            .map(|var| {
                format!(
                    "{var} = {}",
                    u8::from(values.get(var).copied().unwrap_or(false))
                )
            })
            .collect();
        let with = if context.is_empty() {
            String::new()
        } else {
            format!("with {}, ", join_and(&context))
        };
        let lead = match id {
            None if steps.is_empty() => suppose.clone(),
            None => format!("The supposition, {}:", pb.code()),
            Some(id) => format!(
                "{}: {}.",
                capitalize(&constraint_name(&labels, *id)),
                pb.code()
            ),
        };
        match &event.outcome {
            Propagation::Conflict { max } => steps.push(format!(
                "{lead} {} This constraint can't be satisfied: a [contradiction](g:contradiction).",
                capitalize(&format!(
                    "{with}the left side can reach at most {max}, but it needs {}.",
                    pb.rhs
                ))
            )),
            Propagation::Forced { max, slack, lits } => {
                let forced: Vec<String> = lits.iter().map(Lit::made_true).collect();
                let reason = if id.is_none() && context.is_empty() {
                    format!("That forces {}.", join_and(&forced))
                } else if pb.is_clause() {
                    capitalize(&format!(
                        "{with}the only way {}to satisfy it is {}.",
                        if with.is_empty() { "" } else { "left " },
                        join_and(&forced)
                    ))
                } else if *slack == 0 {
                    capitalize(&format!(
                        "{with}the left side can reach at most {max}, exactly the {} it needs, \
                         so every remaining term must count: {}.",
                        pb.rhs,
                        join_and(&forced)
                    ))
                } else {
                    capitalize(&format!(
                        "{with}the left side can reach at most {max}, only {slack} more than the {} \
                         it needs, so any term worth more than {slack} must count: {}.",
                        pb.rhs,
                        join_and(&forced)
                    ))
                };
                steps.push(format!("{lead} {reason}"));
            }
            Propagation::Nothing => {}
        }
        if let Propagation::Forced { lits, .. } = &event.outcome {
            for lit in lits {
                values.insert(lit.var.clone(), !lit.neg);
            }
        }
    }
    doc.steps(steps);

    let added = added_sentence(added_id(session, n), &labels);
    doc.p(if claim.is_contradiction() {
        format!("So the constraints are contradictory. {added}")
    } else {
        format!("So {} can't be false: it must hold. {added}", claim.code())
    });
    doc.negation_note(&shown);
    Some(doc)
}

/// Upper-cases the first letter, also inside a leading `[link](…)`.
fn capitalize(text: &str) -> String {
    if let Some(rest) = text.strip_prefix('[') {
        return format!("[{}", capitalize(rest));
    }
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() => first.to_uppercase().chain(chars).collect(),
        _ => text.to_string(),
    }
}

// ---- pol ----

enum Item {
    Constraint { name: String, text: String },
    Number(i128),
    Literal { text: String },
    Result { step: usize, text: String },
}

fn pol(session: &Session, n: usize, line: &str) -> Option<Doc> {
    let trace = session.explain_line(n).ok()?.ok()?;
    let table = pol_table_with_typed_operands(&trace, line);
    let labels = first_labels(session);

    let mut rows = Vec::new();
    let mut in_table = false;
    for row in table.lines() {
        let tokens: Vec<&str> = row.split_whitespace().collect();
        if tokens.first() == Some(&"depth") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        match tokens.as_slice() {
            [depth, operand, top @ ..] if !top.is_empty() && depth.parse::<usize>().is_ok() => {
                rows.push((
                    depth.parse::<usize>().ok()?,
                    operand.to_string(),
                    top.join(" "),
                ));
            }
            _ => break,
        }
    }
    if rows.is_empty() {
        return None;
    }

    let mut doc = Doc::new(format!("Line {n}: cutting planes (pol)"));
    doc.p(
        "This line uses [cutting planes](g:cutting-planes): it combines constraints with arithmetic steps that keep \
         every solution, so the result holds whenever the constraints it started from do. The \
         steps are written in [reverse Polish notation](g:rpn): the operands first, then the operation.",
    );

    let mut stack: Vec<Item> = Vec::new();
    let mut steps: Vec<String> = Vec::new();
    let mut shown: Vec<Pb> = Vec::new();
    let mut depth = 0usize;
    let mut final_text = String::new();
    for (row_depth, operand, top) in rows {
        if row_depth > depth {
            let item = if let Ok(number) = top.parse::<i128>() {
                Item::Number(number)
            } else if operand.starts_with('@') || operand.parse::<i64>().is_ok() {
                let name = if operand.starts_with('@') {
                    format!("`{operand}`")
                } else if let Ok(id) = operand.parse::<usize>() {
                    constraint_name(&labels, id)
                } else {
                    format!("constraint {operand}")
                };
                let relative = match operand.as_str() {
                    "-1" => " (the newest one)".to_string(),
                    _ if operand.starts_with('-') => {
                        format!(" ({} back from the newest)", &operand[1..])
                    }
                    _ => String::new(),
                };
                steps.push(format!("Take {name}{relative}: {}.", pretty_text(&top)));
                Item::Constraint {
                    name,
                    text: top.clone(),
                }
            } else {
                steps.push(format!(
                    "Take the [literal axiom](g:literal-axiom) {}: it holds for any value of {}.",
                    pretty_text(&top),
                    operand.trim_start_matches('~')
                ));
                Item::Literal {
                    text: operand.clone(),
                }
            };
            stack.push(item);
        } else {
            let popped = depth + 1 - row_depth;
            if popped > stack.len() {
                return None;
            }
            let args: Vec<Item> = stack.split_off(stack.len() - popped);
            let step_no = steps.len() + 1;
            let result = pretty_text(&top);
            steps.push(pol_step(&operand, &args, &result, step_no));
            stack.push(Item::Result {
                step: step_no,
                text: top.clone(),
            });
        }
        depth = row_depth;
        final_text = top.clone();
        if let Some(pb) = Pb::parse(&top) {
            shown.push(pb);
        }
    }
    doc.steps(steps);

    let result = Pb::parse(&final_text);
    let id = last_line_constraint_ids(&trace).last().copied();
    match result {
        Some(pb) if pb.is_contradiction() => doc.p(format!(
            "The result is {}: no assignment can make 0 reach {}, so the constraints used are \
             contradictory. {}",
            pb.code(),
            pb.rhs,
            added_sentence(id, &labels)
        )),
        _ => doc.p(format!(
            "The result is {}. {}",
            pretty_text(&final_text),
            added_sentence(id, &labels)
        )),
    }
    let shown: Vec<&Pb> = shown.iter().collect();
    doc.negation_note(&shown);
    Some(doc)
}

fn describe(item: &Item, step_no: usize) -> String {
    match item {
        Item::Constraint { name, .. } => name.clone(),
        Item::Result { step, .. } if *step + 1 == step_no => "that".to_string(),
        Item::Result { step, .. } => format!("the result of step {step}"),
        Item::Literal { text } => format!("the axiom for {text}"),
        Item::Number(n) => n.to_string(),
    }
}

fn item_text(item: &Item) -> Option<&str> {
    match item {
        Item::Constraint { text, .. } | Item::Result { text, .. } => Some(text),
        _ => None,
    }
}

fn pol_step(op: &str, args: &[Item], result: &str, step_no: usize) -> String {
    let d = |i: usize| {
        args.get(i)
            .map(|a| describe(a, step_no))
            .unwrap_or_default()
    };
    match (op, args) {
        ("+", [a, b]) => {
            let mut text = format!("[Add](g:addition) {} and {}: {result}.", d(0), d(1));
            let (pa, pb) = (
                item_text(a).and_then(Pb::parse),
                item_text(b).and_then(Pb::parse),
            );
            let (pa, pb) = match (pa, pb) {
                (Some(pa), Some(pb)) => (pa, pb),
                // A literal axiom: `1 x >= 0`.
                (Some(pa), None) => (
                    pa,
                    literal_axiom(b).unwrap_or(Pb {
                        terms: vec![],
                        rhs: 0,
                    }),
                ),
                (None, Some(pb)) => (
                    literal_axiom(a).unwrap_or(Pb {
                        terms: vec![],
                        rhs: 0,
                    }),
                    pb,
                ),
                _ => return text,
            };
            let cancelled: Vec<(String, String)> = pa
                .terms
                .iter()
                .filter(|(_, la)| {
                    pb.terms
                        .iter()
                        .any(|(_, lb)| lb.var == la.var && lb.neg != la.neg)
                })
                .map(|(_, l)| (l.text(), l.negated().text()))
                .collect();
            if let [(first, first_neg), rest @ ..] = cancelled.as_slice() {
                let mut pairs = vec![format!("`{first}` cancels against `{first_neg}`")];
                pairs.extend(rest.iter().map(|(l, nl)| format!("`{l}` against `{nl}`")));
                text.push_str(&format!(
                    " {}: since x + ~x is always exactly 1, {} a constant, which moves to the \
                     right-hand side.",
                    capitalize(&join_and(&pairs)),
                    if rest.is_empty() {
                        "the pair is"
                    } else {
                        "each pair is"
                    }
                ));
            }
            text
        }
        ("*", [_, Item::Number(m)]) => {
            format!(
                "Multiply {} by {m} (every coefficient and the right-hand side): {result}.",
                d(0)
            )
        }
        ("d" | "c", [a, Item::Number(m)]) => {
            let mut text = format!(
                "[Divide](g:division) {} by {m}, rounding up: {result}.",
                d(0)
            );
            if let Some(k) = item_text(a).and_then(Pb::parse).map(|pb| pb.rhs)
                && k % m != 0
            {
                let ceil = k.div_euclid(*m) + i128::from(k.rem_euclid(*m) != 0);
                text.push_str(&format!(
                    " Rounding up is safe because the left side is always a whole number: \
                     if it is at least {k}/{m}, it is at least {ceil}."
                ));
            }
            text
        }
        ("s", [a]) => {
            let k = item_text(a).and_then(Pb::parse).map(|pb| pb.rhs);
            match k {
                Some(k) => format!(
                    "[Saturate](g:saturation) {}: cap every coefficient at the right-hand side, {k}: {result}. \
                     A true literal with coefficient {k} or more satisfies the constraint on its \
                     own, so a bigger coefficient adds nothing.",
                    d(0)
                ),
                None => format!("[Saturate](g:saturation) {}: {result}.", d(0)),
            }
        }
        ("w", [_, Item::Literal { text }]) => format!(
            "[Weaken](g:weakening) {} by dropping {}: {result}. This assumes the worst, that {} helped as much \
             as it could, so its coefficient comes off the right-hand side.",
            d(0),
            text.trim_start_matches('~'),
            text.trim_start_matches('~')
        ),
        _ => {
            format!(
                "Apply `{op}` to {}: {result}.",
                args.iter()
                    .map(|a| describe(a, step_no))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

fn literal_axiom(item: &Item) -> Option<Pb> {
    match item {
        Item::Literal { text } => Some(Pb {
            terms: vec![(1, Lit::parse(text)?)],
            rhs: 0,
        }),
        _ => None,
    }
}

// ---- red ----

struct Goal {
    id: String,
    category: String,
    text: String,
}

fn red(session: &Session, n: usize, idx: usize, line: &str) -> Option<Doc> {
    let trace = session.explain_line(n).ok()?.ok()?;
    let labels = first_labels(session);
    let parts: Vec<&str> = line.split(':').collect();
    let claim = rule_constraint(line)?;
    let witness_text = parts
        .get(1)
        .map(|w| w.trim().trim_end_matches(';').trim())
        .unwrap_or("");
    let has_subproof = line
        .trim_end()
        .trim_end_matches(';')
        .trim_end()
        .ends_with("subproof");
    let witness_text = if witness_text == "subproof" {
        ""
    } else {
        witness_text
    };
    let witness: Vec<(String, String)> = {
        let tokens: Vec<&str> = witness_text
            .split_whitespace()
            .filter(|t| *t != "->")
            .collect();
        tokens
            .chunks(2)
            .filter(|p| p.len() == 2)
            .map(|p| (p[0].to_string(), p[1].to_string()))
            .collect()
    };

    let mut goals: Vec<Goal> = Vec::new();
    let mut reasons: HashMap<String, String> = HashMap::new();
    let mut category = String::new();
    let mut rup_only = false;
    let mut autoproving = false;
    for row in trace.lines() {
        let row = row.trim();
        if let Some(rest) = row.strip_prefix("** proofgoal from ") {
            category = rest.trim_end_matches('*').trim().to_string();
        } else if row.starts_with("* constraint is RUP") {
            rup_only = true;
        } else if row.starts_with("autoproving remaining proofgoals") {
            autoproving = true;
        } else if let Some(rest) = row.strip_prefix("proofgoal ") {
            if autoproving {
                if let Some((id, reason)) = rest.split_once(" is ") {
                    reasons.insert(id.trim().to_string(), reason.trim().to_string());
                }
            } else if let Some((id, text)) = rest.split_once(':') {
                let text = text.split_once("|-").map(|(_, t)| t).unwrap_or(text);
                goals.push(Goal {
                    id: id.trim().to_string(),
                    category: category.clone(),
                    text: text.trim().to_string(),
                });
            }
        }
    }
    let explicit = if has_subproof {
        subproof_goals(session, idx)
    } else {
        HashMap::new()
    };

    let mut doc = Doc::new(format!("Line {n}: redundance (red)"));
    let optimising = goals.iter().any(|g| g.category == "objective");
    doc.p(format!(
        "[Redundance-based strengthening](g:redundance) adds a constraint C that need not follow from what we \
         already have. That is allowed when adding C can't lose every solution{}: any solution \
         that breaks C can be repaired, using a substitution called the [witness](g:witness), into one that \
         satisfies C and every other constraint{}. A common use is breaking symmetry: of several \
         equivalent solutions, keep only one.",
        if optimising { ", nor the best one" } else { "" },
        if optimising { ", and is no worse for the [objective](g:objective)" } else { "" },
    ));

    if witness.is_empty() {
        doc.p(format!("Here C is {}, with no witness.", claim.code()));
        if rup_only {
            doc.p(format!(
                "C already follows by unit propagation, so veripb checks it as a `rup` step. {}",
                added_sentence(added_id(session, n), &labels)
            ));
            doc.negation_note(&[&claim]);
            return Some(doc);
        }
    } else {
        let mapping: Vec<String> = witness.iter().map(|(v, x)| format!("{v} → {x}")).collect();
        doc.p(format!(
            "Here C is {}, and the witness is {}. Applying the witness replaces each variable on \
             the left of an arrow by the value or literal on its right.",
            claim.code(),
            join_and(&mapping)
        ));
    }

    if !goals.is_empty() {
        doc.p(format!(
            "To check this, veripb assumes C is false (so {} holds) and asks for these [proof goals](g:proof-goal):",
            claim.negation().code()
        ));
        let database = database_before(session, idx);
        let items = goals
            .iter()
            .map(|goal| {
                let what = match goal.category.as_str() {
                    "satisfying added constraint" => {
                        format!(
                            "C itself holds once the witness is applied: {}.",
                            pretty_text(&goal.text)
                        )
                    }
                    "objective" => format!(
                        "The repaired solution is no worse for the objective: {}.",
                        pretty_text(&goal.text)
                    ),
                    "formula" => {
                        let id: Option<usize> = goal.id.parse().ok();
                        let original = id
                            .and_then(|id| database.as_ref()?.get(id).map(|e| pretty_text(&e.text)))
                            .map(|text| format!(", {text},"))
                            .unwrap_or_default();
                        let name = id
                            .map(|id| constraint_name(&labels, id))
                            .unwrap_or_else(|| format!("constraint {}", goal.id));
                        format!(
                            "{}{original} mentions variables the witness changes. After the change \
                             it reads {}, which must still hold.",
                            capitalize(&name),
                            pretty_text(&goal.text)
                        )
                    }
                    other => format!("{}: {}.", capitalize(other), pretty_text(&goal.text)),
                };
                let how = if let Some((start, end)) = explicit.get(&goal.id) {
                    format!("Proved explicitly in lines {start}–{end}.")
                } else if let Some(reason) = reasons.get(&goal.id) {
                    match reason.as_str() {
                        "implied by additional premise" => {
                            "Proved automatically: it follows directly from assuming C is false."
                                .to_string()
                        }
                        "trivial" => {
                            "Proved automatically: it holds for every assignment.".to_string()
                        }
                        "RUP" => "Proved automatically by unit propagation, as in a `rup` step."
                            .to_string(),
                        other => format!("Proved automatically ({other})."),
                    }
                } else {
                    String::new()
                };
                format!("Goal {}: {what} {how}", goal.id)
                    .trim_end()
                    .to_string()
            })
            .collect();
        doc.steps(items);
    }

    let id = if has_subproof {
        None
    } else {
        added_id(session, n)
    };
    doc.p(if has_subproof {
        "Once every goal is proved, C is added to the database (at the subproof's final `qed`)."
            .to_string()
    } else {
        format!(
            "Every goal is proved, so C is accepted. {}",
            added_sentence(id, &labels)
        )
    });
    let mut shown = vec![claim.clone(), claim.negation()];
    shown.extend(goals.iter().filter_map(|g| Pb::parse(&g.text)));
    let shown: Vec<&Pb> = shown.iter().collect();
    doc.negation_note(&shown);
    Some(doc)
}

/// The top-level `proofgoal <id> ... qed` blocks of the subproof that opens
/// at buffer line `idx`, as display line ranges.
fn subproof_goals(session: &Session, idx: usize) -> HashMap<String, (usize, usize)> {
    let mut goals = HashMap::new();
    // One frame per open subproof: the goal it is inside, if any.
    let mut frames: Vec<Option<(String, usize)>> = vec![None];
    for (i, text) in session.buffer.iter().enumerate().skip(idx + 1) {
        let text = text.trim();
        let keyword = text
            .split_whitespace()
            .find(|t| !t.starts_with('@'))
            .unwrap_or("");
        if keyword == "proofgoal" {
            let id = text
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .trim_end_matches(';')
                .to_string();
            if let Some(frame) = frames.last_mut() {
                *frame = Some((id, i));
            }
        } else if keyword.starts_with("qed") {
            match frames.last_mut() {
                Some(frame @ Some(_)) => {
                    let (id, start) = frame.take().unwrap();
                    if frames.len() == 1 {
                        goals.insert(id, (session.display_line(start), session.display_line(i)));
                    }
                }
                _ => {
                    frames.pop();
                    if frames.is_empty() {
                        break;
                    }
                }
            }
        } else if text.trim_end_matches(';').trim_end().ends_with("subproof") {
            frames.push(None);
        }
    }
    goals
}

// ---- a ----

fn assertion(session: &Session, n: usize, line: &str) -> Option<Doc> {
    let claim = rule_constraint(line)?;
    let labels = first_labels(session);
    let mut doc = Doc::new(format!("Line {n}: assertion (a)"));
    doc.p(format!(
        "This line claims {} without any proof, and veripb accepts it unchecked. {}",
        claim.code(),
        added_sentence(added_id(session, n), &labels)
    ));
    doc.p(
        "An [assertion](g:assertion) is a placeholder: it lets you build and check the rest of a proof before \
         this step is justified. A proof that still contains an `a` line is not a valid proof.",
    );
    doc.p(format!(
        "To finish it, replace the line with a real derivation, for example a `rup` or `pol` \
         step: `:deassert {n}` helps with that."
    ));
    doc.negation_note(&[&claim]);
    Some(doc)
}

// ---- setlvl and wiplvl ----

/// The level a `setlvl` or `wiplvl` line names.
fn level_of(line: &str) -> Option<usize> {
    line.split_whitespace()
        .skip_while(|t| t.starts_with('@'))
        .nth(1)?
        .trim_end_matches(';')
        .parse()
        .ok()
}

/// The `keyword` lines among buffer lines `range`, as (buffer index,
/// level).
fn level_lines<'a>(
    session: &'a Session,
    keyword: &'a str,
    range: std::ops::Range<usize>,
) -> impl DoubleEndedIterator<Item = (usize, usize)> + 'a {
    range.filter_map(move |i| {
        let line = &session.buffer[i];
        (rule_keyword(line) == Some(keyword))
            .then(|| level_of(line))
            .flatten()
            .map(|level| (i, level))
    })
}

fn set_level(session: &Session, n: usize, idx: usize, line: &str) -> Option<Doc> {
    let level = level_of(line)?;
    let mut doc = Doc::new(format!("Line {n}: set level (setlvl)"));
    doc.p(format!(
        "This line sets the current [level](g:levels) to {level}. From here until the next `setlvl`, every \
         constraint the proof adds is recorded at level {level}. The line adds no constraint itself."
    ));
    doc.p(match level_lines(session, "setlvl", 0..idx).next_back() {
        Some((i, before)) => format!("Before it, the level was {before} (set on line {}).", session.display_line(i)),
        None => "Before it, no level was set: the constraints added so far are recorded at no level, so no \
                 `wiplvl` deletes them."
            .to_string(),
    });
    doc.p(match level_lines(session, "wiplvl", idx + 1..session.buffer.len()).find(|&(_, wipe)| wipe <= level) {
        Some((i, wipe)) => format!(
            "Line {} (`wiplvl {wipe}`) deletes the constraints recorded at level {level}, along with those at the \
             other levels from {wipe} up.",
            session.display_line(i)
        ),
        None => format!("No later `wiplvl` deletes level {level}: its constraints stay in the database."),
    });
    Some(doc)
}

/// Constraints shown by name before the rest are counted.
const LISTED: usize = 12;

fn wipe_level(session: &Session, n: usize, idx: usize, line: &str) -> Option<Doc> {
    let level = level_of(line)?;
    let mut doc = Doc::new(format!("Line {n}: wipe level (wiplvl)"));
    doc.p(format!(
        "This line deletes every constraint recorded at [level](g:levels) {level} or higher: the constraints added \
         while a `setlvl` of {level} or more was in force. The current level stays as it was."
    ));
    let set: Vec<String> = level_lines(session, "setlvl", 0..idx)
        .filter(|&(_, set)| set >= level)
        .map(|(i, set)| format!("line {} (`setlvl {set}`)", session.display_line(i)))
        .collect();
    if !set.is_empty() {
        doc.p(format!("Those levels were set on {}.", join_and(&set)));
    }
    let before = database_before(session, idx)?;
    let after = database_before(session, idx + 1)?;
    let kept: std::collections::HashSet<usize> = after.entries.iter().map(|e| e.id).collect();
    let deleted: Vec<_> = before
        .entries
        .iter()
        .filter(|e| !kept.contains(&e.id))
        .collect();
    if deleted.is_empty() {
        doc.p(format!(
            "Here that deletes nothing: no constraint still in the database was recorded at level {level} or above \
             (a constraint already deleted, by `del` or an earlier `wiplvl`, is not deleted again)."
        ));
        return Some(doc);
    }
    // Not linked: they are no longer in the database to jump to.
    let labels = first_labels(session);
    let name = |id: usize| match labels.get(&id) {
        Some(label) => format!("`{label}` (constraint {id})"),
        None => format!("constraint {id}"),
    };
    let mut names: Vec<String> = deleted.iter().take(LISTED).map(|e| name(e.id)).collect();
    if deleted.len() > LISTED {
        names.push(format!("{} more", deleted.len() - LISTED));
    }
    doc.p(format!(
        "Here that deletes {} constraint{}: {}.",
        deleted.len(),
        if deleted.len() == 1 { "" } else { "s" },
        join_and(&names)
    ));
    let core = deleted.iter().filter(|e| e.is_core).count();
    if core > 0 {
        doc.note(format!(
            "{core} of them {} [core](g:core). As with `del`, deleting a core constraint is checked: it must be \
             re-derivable from the remaining core constraints.",
            if core == 1 { "is" } else { "are" }
        ));
    }
    Some(doc)
}
