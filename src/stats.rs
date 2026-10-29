//! Statistics for the formula, the constraint database and the proof (see
//! `:stats`): sizes, what the constraints look like, and which rules the
//! proof uses.

use std::collections::BTreeMap;

use crate::checker::parse::{formula_line_constraints, leading_labels, rule_keyword};
use crate::session::Session;

/// What kind of constraint a line is, judged from its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Equivalent to a clause: every coefficient is at least the degree
    /// (in `>=` form, with negative coefficients moved to the degree).
    Clause,
    /// All coefficients equal, and less than the degree.
    Cardinality,
    /// Any other inequality.
    General,
    /// `=`, loaded as two inequalities.
    Equality,
    /// `==>`, `<==` or `<==>`.
    Implication,
    /// Always holds: the degree is at most 0.
    Trivial,
    /// Never holds: the coefficients sum to less than the degree.
    Contradiction,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Clause => "clause",
            Kind::Cardinality => "cardinality",
            Kind::General => "general",
            Kind::Equality => "equality",
            Kind::Implication => "implication",
            Kind::Trivial => "trivial",
            Kind::Contradiction => "contradiction",
        }
    }
}

/// One constraint's kind, number of terms, and largest coefficient (its
/// magnitude, as written).
#[derive(Debug, PartialEq, Eq)]
pub struct Shape {
    pub kind: Kind,
    pub terms: usize,
    pub largest_coefficient: Option<String>,
}

/// Classifies `constraint`, a formula line or database entry: leading
/// `@label`s and a trailing `;` are ignored. A coefficient or degree too
/// large for 128 bits makes it `General`.
pub fn classify(constraint: &str) -> Shape {
    let tokens: Vec<&str> = constraint
        .split_whitespace()
        .skip_while(|token| token.starts_with('@'))
        .map(|token| token.trim_end_matches(';'))
        .filter(|token| !token.is_empty())
        .collect();
    let comparison = |token: &&str| matches!(*token, ">=" | "<=" | "=" | ">" | "<");
    let implication = |token: &&str| matches!(*token, "==>" | "<==" | "<==>");
    let is_number = |token: &str| {
        let digits = token.trim_start_matches(['+', '-']);
        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
    };

    // Each literal's coefficient, as written (1 if none).
    let lhs_end = tokens
        .iter()
        .position(|token| comparison(token) || implication(token))
        .unwrap_or(tokens.len());
    let mut coefficients: Vec<&str> = Vec::new();
    let mut pending = None;
    for token in &tokens {
        if comparison(token) || implication(token) {
            pending = None;
        } else if is_number(token) {
            pending = Some(*token);
        } else {
            coefficients.push(pending.take().unwrap_or("1"));
        }
    }
    let largest_coefficient = coefficients
        .iter()
        .map(|c| c.trim_start_matches(['+', '-']))
        .max_by(|a, b| a.len().cmp(&b.len()).then(a.cmp(b)))
        .map(str::to_string);
    let shape = |kind| Shape {
        kind,
        terms: coefficients.len(),
        largest_coefficient: largest_coefficient.clone(),
    };

    if tokens.iter().any(implication) {
        return shape(Kind::Implication);
    }
    let Some(&operator) = tokens.get(lhs_end) else {
        return shape(Kind::General);
    };
    if operator == "=" {
        return shape(Kind::Equality);
    }
    let parsed: Option<Vec<i128>> = coefficients.iter().map(|c| c.parse().ok()).collect();
    let degree: Option<i128> = tokens.get(lhs_end + 1).and_then(|d| d.parse().ok());
    let (Some(coefficients), Some(degree)) = (parsed, degree) else {
        return shape(Kind::General);
    };
    shape(inequality_kind(operator, coefficients, degree).unwrap_or(Kind::General))
}

/// The kind of `coefficients <operator> degree`, after normalizing to
/// `>=` with positive coefficients; `None` on overflow.
fn inequality_kind(operator: &str, mut coefficients: Vec<i128>, degree: i128) -> Option<Kind> {
    let mut degree = match operator {
        ">=" => degree,
        ">" => degree.checked_add(1)?,
        "<=" | "<" => {
            for c in &mut coefficients {
                *c = c.checked_neg()?;
            }
            let degree = degree.checked_neg()?;
            if operator == "<" {
                degree.checked_add(1)?
            } else {
                degree
            }
        }
        _ => return None,
    };
    // `c l` with `c < 0` is `|c| ~l - |c|`.
    for c in coefficients.iter().filter(|&&c| c < 0) {
        degree = degree.checked_add(c.checked_abs()?)?;
    }
    let magnitudes: Vec<i128> = coefficients
        .iter()
        .filter(|&&c| c != 0)
        .map(|c| c.abs())
        .collect();
    let sum = magnitudes
        .iter()
        .try_fold(0i128, |sum, &c| sum.checked_add(c))?;
    Some(if degree <= 0 {
        Kind::Trivial
    } else if sum < degree {
        Kind::Contradiction
    } else if magnitudes.iter().all(|&c| c >= degree) {
        Kind::Clause
    } else if magnitudes.windows(2).all(|pair| pair[0] == pair[1]) {
        Kind::Cardinality
    } else {
        Kind::General
    })
}

/// Terms per constraint.
#[derive(Debug, PartialEq)]
pub struct TermCounts {
    pub min: usize,
    pub median: usize,
    pub max: usize,
    pub mean: f64,
}

/// What a set of constraints looks like.
#[derive(Debug, PartialEq)]
pub struct Shapes {
    /// How many of each kind, most first.
    pub kinds: Vec<(Kind, usize)>,
    /// `None` if there are no constraints.
    pub terms: Option<TermCounts>,
    pub largest_coefficient: Option<String>,
}

fn shapes<'a>(constraints: impl Iterator<Item = &'a str>) -> Shapes {
    let mut kinds: BTreeMap<Kind, usize> = BTreeMap::new();
    let mut terms = Vec::new();
    let mut largest: Option<String> = None;
    for constraint in constraints {
        let shape = classify(constraint);
        *kinds.entry(shape.kind).or_default() += 1;
        terms.push(shape.terms);
        if let Some(c) = shape.largest_coefficient
            && largest
                .as_ref()
                .is_none_or(|l| (c.len(), &c) > (l.len(), l))
        {
            largest = Some(c);
        }
    }
    let mut kinds: Vec<(Kind, usize)> = kinds.into_iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let counts = (!terms.is_empty()).then(|| {
        let mean = terms.iter().sum::<usize>() as f64 / terms.len() as f64;
        let middle = terms.len() / 2;
        let (_, &mut median, _) = terms.select_nth_unstable(middle);
        TermCounts {
            min: *terms.iter().min().expect("not empty"),
            median,
            max: *terms.iter().max().expect("not empty"),
            mean,
        }
    });
    Shapes {
        kinds,
        terms: counts,
        largest_coefficient: largest,
    }
}

#[derive(Debug)]
pub struct FormulaStats {
    pub lines: usize,
    pub constraints: usize,
    /// Lines that load as two constraints (`=` or `<==>`).
    pub two_constraint_lines: usize,
    /// Lines with a leading `@label`.
    pub labelled: usize,
    pub variables: usize,
    /// The objective's terms, if there is one.
    pub objective_terms: Option<usize>,
    /// The preserved variables, if declared.
    pub preserved: Option<usize>,
    pub shapes: Shapes,
}

pub fn formula(session: &Session) -> FormulaStats {
    let lines = &session.formula;
    FormulaStats {
        lines: lines.len(),
        constraints: session.formula_constraint_count(),
        two_constraint_lines: lines
            .iter()
            .filter(|line| formula_line_constraints(line) == 2)
            .count(),
        labelled: lines
            .iter()
            .filter(|line| !leading_labels(line).is_empty())
            .count(),
        variables: session.variables.len(),
        objective_terms: session.objective.as_deref().map(|objective| {
            let terms = objective.trim_start();
            let terms = terms
                .strip_prefix("min:")
                .or_else(|| terms.strip_prefix("max:"));
            classify(terms.unwrap_or(objective)).terms
        }),
        preserved: session.preserved.as_deref().map(|preserved| {
            preserved
                .split_whitespace()
                .filter(|token| *token != "preserved:" && !token.ends_with(';'))
                .count()
        }),
        shapes: shapes(lines.iter().map(String::as_str)),
    }
}

#[derive(Debug)]
pub struct DatabaseStats {
    pub constraints: usize,
    pub core: usize,
    /// Constraints from the formula still in the database.
    pub from_formula: usize,
    pub formula_constraints: usize,
    pub labelled: usize,
    pub highest_id: Option<usize>,
    pub shapes: Shapes,
}

/// The database after the checked lines; may take a `veripb` run, as
/// [`Session::database`] does.
pub fn database(session: &Session) -> anyhow::Result<DatabaseStats> {
    let database = session.database()?;
    let labels_by_id = session.labels_by_id();
    let formula_constraints = session.formula_constraint_count();
    let entries = &database.entries;
    Ok(DatabaseStats {
        constraints: entries.len(),
        core: entries.iter().filter(|entry| entry.is_core).count(),
        from_formula: entries
            .iter()
            .filter(|entry| entry.id <= formula_constraints)
            .count(),
        formula_constraints,
        labelled: entries
            .iter()
            .filter(|entry| labels_by_id.contains_key(&(entry.id as isize)))
            .count(),
        highest_id: entries.last().map(|entry| entry.id),
        shapes: shapes(entries.iter().map(|entry| entry.text.as_str())),
    })
}

#[derive(Debug)]
pub struct ProofStats {
    pub lines: usize,
    pub checked: usize,
    /// Whether the line after the checked ones was rejected.
    pub rejected: bool,
    /// Lines with a leading `@label`.
    pub labelled: usize,
    pub breakpoints: usize,
    /// Each rule (`comment`, `blank`, or the line's keyword) with its lines
    /// and checked lines, most lines first.
    pub rules: Vec<(String, usize, usize)>,
}

pub fn proof(session: &Session) -> ProofStats {
    let mut rules: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (idx, line) in session.buffer.iter().enumerate() {
        let rule = match rule_keyword(line) {
            None => "blank".to_string(),
            Some(keyword) if keyword.starts_with('%') => "comment".to_string(),
            Some(keyword) => keyword.trim_end_matches(';').to_string(),
        };
        let counts = rules.entry(rule).or_default();
        counts.0 += 1;
        if idx < session.checked_len {
            counts.1 += 1;
        }
    }
    let mut rules: Vec<(String, usize, usize)> = rules
        .into_iter()
        .map(|(rule, (lines, checked))| (rule, lines, checked))
        .collect();
    rules.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ProofStats {
        lines: session.buffer.len(),
        checked: session.checked_len,
        rejected: session.known_bad.is_some(),
        labelled: session
            .buffer
            .iter()
            .filter(|line| !leading_labels(line).is_empty())
            .count(),
        breakpoints: session.breakpoints.len(),
        rules,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(constraint: &str) -> Kind {
        classify(constraint).kind
    }

    #[test]
    fn constraints_are_classified_by_shape() {
        assert_eq!(kind("@c1 1 x1 1 ~x2 >= 1 ;"), Kind::Clause);
        assert_eq!(kind("3 x1 2 x2 >= 2"), Kind::Clause);
        assert_eq!(kind("1 x1 1 x2 1 x3 >= 2"), Kind::Cardinality);
        assert_eq!(kind("1 x1 2 x2 1 x3 >= 2"), Kind::General);
        assert_eq!(kind("1 x1 1 x2 = 1 ;"), Kind::Equality);
        assert_eq!(kind("@i 1 x1 >= 1 ==> 1 x2 >= 1 ;"), Kind::Implication);
        assert_eq!(kind("1 x1 >= 0"), Kind::Trivial);
        assert_eq!(kind(">= 1"), Kind::Contradiction);
        assert_eq!(kind("1 x1 1 x2 >= 3"), Kind::Contradiction);
        // -1 x1 -1 x2 >= -1 is 1 ~x1 1 ~x2 >= 1.
        assert_eq!(kind("-1 x1 -1 x2 >= -1"), Kind::Clause);
        assert_eq!(kind("1 x1 1 x2 <= 1"), Kind::Clause);
        assert_eq!(kind("1 x1 1 x2 1 x3 < 3"), Kind::Clause);
        assert_eq!(kind("1 x1 1 x2 1 x3 <= 1"), Kind::Cardinality);
        assert_eq!(
            kind("99999999999999999999999999999999999999999 x1 >= 1"),
            Kind::General
        );
    }

    #[test]
    fn terms_and_the_largest_coefficient_are_counted() {
        let shape = classify("3 x1 -12 ~x2 x3 >= 2 ;");
        assert_eq!(shape.terms, 3);
        assert_eq!(shape.largest_coefficient.as_deref(), Some("12"));

        let shapes = shapes(["1 x1 >= 1", "1 x1 1 x2 1 x3 >= 2", "2 x1 2 x2 >= 1"].into_iter());
        assert_eq!(shapes.kinds, [(Kind::Clause, 2), (Kind::Cardinality, 1)]);
        let terms = shapes.terms.unwrap();
        assert_eq!((terms.min, terms.median, terms.max), (1, 2, 3));
        assert_eq!(terms.mean, 2.0);
        assert_eq!(shapes.largest_coefficient.as_deref(), Some("2"));
    }
}
