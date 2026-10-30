//! The glossary: concepts, written for students who are new to
//! pseudo-Boolean proofs, and the proof rules from `:help`'s topics
//! (`commands::help::RULES`). Text is in [`super::markup`]: entries link to
//! each other as `[text](g:<id>)`.

use crate::commands::help::{self, Topic};

struct Concept {
    id: &'static str,
    term: &'static str,
    body: &'static [&'static str],
    example: Option<&'static str>,
    see: &'static [&'static str],
}

const CONCEPTS: &[Concept] = &[
    Concept {
        id: "variable",
        term: "Variable and literal",
        body: &[
            "A variable stands for a yes/no choice: its value is 1 (true) or 0 (false).",
            "A literal is a variable `x` or its negation `~x` (“not x”). `~x` is 1 exactly when \
             `x` is 0, so `x + ~x` is always 1.",
        ],
        example: Some("If x1 = 0, the literal `x1` is 0 and the literal `~x1` is 1."),
        see: &["constraint", "assignment"],
    },
    Concept {
        id: "constraint",
        term: "Pseudo-Boolean constraint",
        body: &[
            "A condition on [literals](g:variable) of the form `a1 l1 + a2 l2 + … ≥ k`: each \
             literal l has a whole-number coefficient a, and k is the right-hand side (also called \
             the degree). The left side adds up the coefficients of the literals that are true; \
             the constraint holds when that sum reaches k.",
            "In files it is written with coefficients first and a semicolon: `2 x1 1 ~x2 >= 2 ;`. \
             Here it is shown as `2 x1 + ~x2 ≥ 2`. A clause, such as `x1 + x2 ≥ 1` (“x1 or x2”), \
             is the special case with every coefficient and the right-hand side 1.",
        ],
        example: Some(
            "`2 x1 + ~x2 ≥ 2` holds when x1 = 1, and fails when x1 = 0 (the left side is then at most 1).",
        ),
        see: &["variable", "assignment", "formula", "slack"],
    },
    Concept {
        id: "assignment",
        term: "Assignment and solution",
        body: &[
            "An assignment gives each variable a value, 0 or 1. It satisfies a \
             [constraint](g:constraint) when the left side reaches the right-hand side, and \
             violates it otherwise.",
            "A solution is an assignment that satisfies every constraint of the \
             [formula](g:formula). A formula with no solution is unsatisfiable.",
        ],
        example: None,
        see: &["formula", "contradiction"],
    },
    Concept {
        id: "formula",
        term: "Formula",
        body: &[
            "The problem to reason about: a list of [constraints](g:constraint) in an `.opb` file, \
             possibly with an [objective](g:objective). `:load` reads it; the Formula pane shows it.",
            "A proof usually shows that the formula is unsatisfiable, or that a solution is the \
             best possible.",
        ],
        example: None,
        see: &["proof", "objective", "database"],
    },
    Concept {
        id: "objective",
        term: "Objective",
        body: &[
            "An optional weighted sum of literals to minimise, written `min: …` at the top of an `.opb` \
             file. The problem is then to find the solution with the smallest value; a proof can \
             show that no solution does better than a given bound.",
        ],
        example: Some(
            "`min: x1 + x2 + x3` asks for a solution with as few true variables as possible.",
        ),
        see: &["formula", "rule-soli", "rule-obju"],
    },
    Concept {
        id: "proof",
        term: "Proof",
        body: &[
            "A `.pbp` file: a sequence of steps, each deriving one new constraint by a proof rule \
             (such as [rup](g:rule-rup) or [pol](g:rule-pol)). veripb checks every step; a step \
             it can't confirm is rejected. `:source` reads a proof; the Proof pane shows it.",
            "It ends with an `output` line and a `conclusion`, such as UNSAT: the formula has no \
             solution, shown by deriving a [contradiction](g:contradiction).",
        ],
        example: None,
        see: &["database", "contradiction", "rule-conclusion"],
    },
    Concept {
        id: "database",
        term: "Constraint database, IDs and labels",
        body: &[
            "veripb keeps every constraint it knows in a database, shown in the Database pane. \
             Each constraint has an ID number, in order: the formula's constraints first, then \
             one per derived constraint.",
            "A step refers to earlier constraints by ID, or by a label: a name such as \
             `@x1_or_x2` written at the start of a line. In some rules a negative ID counts back \
             from the newest constraint: `-1` is the newest.",
        ],
        example: None,
        see: &["core", "proof"],
    },
    Concept {
        id: "core",
        term: "Core and derived constraints",
        body: &[
            "Core constraints are the formula's, plus any moved there with the \
             [core](g:rule-core) rule. Derived constraints are the ones the proof has proved. \
             [Redundance](g:redundance) reasoning checks its [proof goals](g:proof-goal) against \
             the core constraints.",
            "The Database pane marks which is which.",
        ],
        example: None,
        see: &["database", "redundance"],
    },
    Concept {
        id: "contradiction",
        term: "Contradiction",
        body: &[
            "The constraint `0 ≥ 1` (in files, `>= 1` with nothing on the left). No assignment \
             satisfies it, so deriving it from the formula shows that the formula has no \
             solution.",
            "A rule that reaches a contradiction under an assumption shows that the assumption \
             is impossible: that is how [RUP](g:rup) works.",
        ],
        example: None,
        see: &["proof", "rup"],
    },
    Concept {
        id: "negation",
        term: "Negation of a constraint",
        body: &[
            "The negation of a constraint C holds exactly when C fails. For `a1 l1 + … ≥ k`, it \
             says the left side is at most k − 1. Written with negated literals, it becomes \
             `a1 ~l1 + … ≥ (a1 + … ) − k + 1`.",
        ],
        example: Some("The negation of `x1 + x2 ≥ 1` is `~x1 + ~x2 ≥ 2`: both x1 and x2 are 0."),
        see: &["rup", "redundance"],
    },
    Concept {
        id: "unit-propagation",
        term: "Unit propagation",
        body: &[
            "Given some variables already fixed, a constraint may leave only one way to be \
             satisfied: some of its literals must be true. Fixing those and repeating, constraint \
             by constraint, is unit propagation.",
            "If it reaches a constraint that can no longer be satisfied at all, that is a \
             conflict: the fixed values can't all hold together.",
        ],
        example: Some("With x3 = 0, the clause `~x1 + x3 ≥ 1` can only be satisfied by x1 = 0."),
        see: &["slack", "rup"],
    },
    Concept {
        id: "slack",
        term: "Slack",
        body: &[
            "How much room a [constraint](g:constraint) has left under the values fixed so far: \
             the sum of the coefficients of literals not yet false, minus the right-hand side.",
            "Negative slack means the constraint can't be satisfied (a conflict). A literal not \
             yet fixed whose coefficient is greater than the slack must be true: without it the \
             left side would fall short.",
        ],
        example: Some(
            "In `2 x1 + x2 + x3 ≥ 2` with x3 = 0, the slack is 2 + 1 − 2 = 1. The coefficient of \
             x1 is 2 > 1, so x1 must be 1.",
        ),
        see: &["unit-propagation"],
    },
    Concept {
        id: "rup",
        term: "Reverse unit propagation (RUP)",
        body: &[
            "A way to prove a constraint C by contradiction: assume its [negation](g:negation), \
             then run [unit propagation](g:unit-propagation). If that reaches a conflict, C can't \
             be false, so it holds. The [rup](g:rule-rup) rule is checked this way.",
            "Hints after a colon list the constraints to propagate with, in order; `~` stands for \
             the negation itself.",
        ],
        example: Some(
            "To prove `x3 ≥ 1` from `x1 + x2 ≥ 1`, `~x1 + x3 ≥ 1` and `~x2 + x3 ≥ 1`: assume x3 = 0; then x1 = 0 and x2 = 0, and `x1 + x2 ≥ 1` fails.",
        ),
        see: &["unit-propagation", "negation", "rule-rup"],
    },
    Concept {
        id: "cutting-planes",
        term: "Cutting planes",
        body: &[
            "Rules that derive a new constraint from old ones by arithmetic: \
             [addition](g:addition), multiplication by a positive number, [division](g:division), \
             [saturation](g:saturation) and [weakening](g:weakening). Each keeps every solution: \
             any assignment that satisfies the inputs satisfies the result.",
            "The [pol](g:rule-pol) rule writes a sequence of these steps in \
             [reverse Polish notation](g:rpn).",
        ],
        example: None,
        see: &["rule-pol", "rpn"],
    },
    Concept {
        id: "rpn",
        term: "Reverse Polish notation",
        body: &[
            "A way to write a calculation with the operands first and each operation after \
             them, so no brackets are needed. `pol` steps use it.",
        ],
        example: Some(
            "`pol 1 2 + 3 d ;` means: add constraints 1 and 2, then divide the result by 3.",
        ),
        see: &["rule-pol", "cutting-planes"],
    },
    Concept {
        id: "addition",
        term: "Addition",
        body: &[
            "Add two constraints term by term, and add their right-hand sides. Where a literal \
             meets its negation, `x + ~x` is always 1, so the pair becomes a constant that moves \
             to the right-hand side: the literals cancel.",
        ],
        example: Some(
            "`x1 + x2 ≥ 1` plus `~x1 + x3 ≥ 1` gives `x2 + x3 ≥ 1` (x1 cancels against ~x1).",
        ),
        see: &["cutting-planes"],
    },
    Concept {
        id: "division",
        term: "Division",
        body: &[
            "Divide every coefficient and the right-hand side by a positive number, rounding up. \
             Rounding up is sound because the left side is always a whole number: if it is at \
             least 3/2, it is at least 2.",
        ],
        example: Some("`2 x1 + 2 x2 ≥ 3` divided by 2 gives `x1 + x2 ≥ 2`."),
        see: &["cutting-planes"],
    },
    Concept {
        id: "saturation",
        term: "Saturation",
        body: &[
            "Cap every coefficient at the right-hand side k. A true literal with coefficient k or \
             more already satisfies the constraint on its own, so a bigger coefficient adds \
             nothing.",
        ],
        example: Some("`3 x1 + x2 ≥ 2` saturates to `2 x1 + x2 ≥ 2`."),
        see: &["cutting-planes"],
    },
    Concept {
        id: "weakening",
        term: "Weakening",
        body: &[
            "Drop a variable from a constraint, assuming the worst: that its literal was true \
             and contributed its full coefficient. That coefficient comes off the right-hand \
             side. The result is weaker, but it no longer mentions the variable.",
        ],
        example: Some("Weakening `2 x1 + x2 ≥ 2` on x1 gives `x2 ≥ 0`."),
        see: &["cutting-planes", "literal-axiom"],
    },
    Concept {
        id: "literal-axiom",
        term: "Literal axiom",
        body: &[
            "The constraint `x ≥ 0` (or `~x ≥ 0`), true for any value of x. A `pol` step can use \
             a literal as an operand; adding one is a way to cancel a literal or weaken a \
             constraint.",
        ],
        example: None,
        see: &["addition", "weakening"],
    },
    Concept {
        id: "redundance",
        term: "Redundance-based strengthening",
        body: &[
            "A rule ([red](g:rule-red)) that adds a constraint C even though C need not follow \
             from what is known. That is allowed if any solution that breaks C can be repaired, \
             with a [witness](g:witness), into a solution that satisfies C and every other \
             constraint (and, with an [objective](g:objective), is no worse).",
            "veripb checks this through [proof goals](g:proof-goal). It is often used to break \
             symmetry: of several equivalent solutions, keep only one.",
        ],
        example: None,
        see: &["witness", "proof-goal", "rule-red"],
    },
    Concept {
        id: "witness",
        term: "Witness",
        body: &[
            "A substitution that says how to repair a solution in a [redundance](g:redundance) \
             step: each listed variable is replaced by a constant (0 or 1) or by another literal. \
             It is written after the constraint, as `x1 -> x2 x2 -> x1` (the arrows are \
             optional).",
        ],
        example: Some(
            "`red 1 x1 1 ~x2 >= 1 : x1 -> x2 x2 -> x1 ;` swaps x1 and x2: any solution breaking the \
             constraint has x1 = 0 and x2 = 1, and swapping gives x1 = 1 and x2 = 0, which \
             satisfies it.",
        ),
        see: &["redundance", "proof-goal"],
    },
    Concept {
        id: "proof-goal",
        term: "Proof goal and subproof",
        body: &[
            "What a [redundance](g:redundance) step must show, assuming its constraint is false: \
             that the constraint holds once the [witness](g:witness) is applied, that each core \
             constraint the witness changes still holds, and, with an objective, that it does \
             not get worse. veripb proves many goals automatically.",
            "Goals it can't prove automatically need a subproof: `proofgoal <id>` … `qed` blocks \
             after the step. Each block assumes the goal is false and derives a \
             [contradiction](g:contradiction) from that.",
        ],
        example: None,
        see: &["redundance", "rule-proofgoal"],
    },
    Concept {
        id: "levels",
        term: "Levels",
        body: &[
            "A proof can record the constraints it adds at numbered levels, to delete them together later. \
             `setlvl k` sets the current level: every constraint added after it, until the \
             next `setlvl`, is recorded at level k. `wiplvl k` deletes every constraint \
             recorded at level k or higher, in one step.",
            "Constraints added before the first `setlvl` are recorded at no level, and no `wiplvl` deletes \
             them. Deleting a [core](g:core) constraint this way is checked, as with [del](g:rule-del).",
        ],
        example: Some(
            "`setlvl 1;` … `setlvl 2;` … `wiplvl 2;` deletes what was added at level 2; a later \
             `wiplvl 1;` deletes what was added at level 1.",
        ),
        see: &["database", "rule-del"],
    },
    Concept {
        id: "assertion",
        term: "Assertion",
        body: &[
            "A step ([a](g:rule-a)) that claims a constraint without any proof. veripb accepts it \
             unchecked, so a proof that still contains one is not valid. It is useful as a \
             placeholder while building a proof top-down; `:deassert` helps replace it with a \
             real derivation.",
        ],
        example: None,
        see: &["rule-a"],
    },
];

/// Concepts that explain each rule's ideas.
fn rule_see(name: &str) -> &'static [&'static str] {
    match name {
        "rup" => &["rup", "unit-propagation"],
        "pol" => &["cutting-planes", "rpn"],
        "red" => &["redundance", "witness", "proof-goal"],
        "pbc" => &["negation", "contradiction"],
        "del" | "delc" | "deld" => &["database", "core"],
        "core" => &["core"],
        "obju" | "sol" | "soli" | "obji" | "solx" => &["objective", "assignment"],
        "f" => &["formula"],
        "a" => &["assertion"],
        "proofgoal" | "qed" => &["proof-goal"],
        "output" | "conclusion" | "end" => &["proof", "contradiction"],
        "setlvl" | "wiplvl" => &["levels", "database"],
        _ => &[],
    }
}

/// Whether an entry is a concept or a proof rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Concept,
    Rule,
}

/// One glossary entry.
#[derive(Clone, Debug)]
pub struct Entry {
    /// `slack`, or `rule-<name>` for a rule.
    pub id: String,
    pub term: &'static str,
    pub group: Group,
    /// Paragraphs; a rule's is its summary.
    pub body: Vec<&'static str>,
    pub example: Option<&'static str>,
    /// A rule's syntax.
    pub usage: Option<&'static str>,
    /// A rule's details, from `:help`.
    pub details: &'static [&'static str],
    /// Related entries' ids.
    pub see: &'static [&'static str],
}

fn concept_entry(concept: &Concept) -> Entry {
    Entry {
        id: concept.id.to_string(),
        term: concept.term,
        group: Group::Concept,
        body: concept.body.to_vec(),
        example: concept.example,
        usage: None,
        details: &[],
        see: concept.see,
    }
}

fn rule_entry(topic: &'static Topic) -> Entry {
    Entry {
        id: format!("rule-{}", topic.name),
        term: topic.name,
        group: Group::Rule,
        body: vec![topic.summary],
        example: None,
        usage: Some(topic.usage),
        details: topic.details,
        see: rule_see(topic.name),
    }
}

/// Every entry: the concepts, then the rules.
pub fn entries() -> Vec<Entry> {
    CONCEPTS
        .iter()
        .map(concept_entry)
        .chain(help::RULES.iter().map(rule_entry))
        .collect()
}

/// The concepts' ids, in glossary order.
pub fn concept_ids() -> impl Iterator<Item = &'static str> {
    CONCEPTS.iter().map(|concept| concept.id)
}

/// The concepts related to rule `name` (empty for an unknown rule).
pub fn rule_concepts(name: &str) -> &'static [&'static str] {
    rule_see(name)
}

/// The entry with id `id`.
pub fn entry(id: &str) -> Option<Entry> {
    if let Some(name) = id.strip_prefix("rule-") {
        return help::RULES.iter().find(|t| t.name == name).map(rule_entry);
    }
    CONCEPTS.iter().find(|c| c.id == id).map(concept_entry)
}

/// The entry `topic` names: an id, a rule's name, or a concept's term or
/// id ignoring case.
pub fn lookup(topic: &str) -> Option<Entry> {
    let topic = topic.trim();
    entry(topic)
        .or_else(|| entry(&format!("rule-{topic}")))
        .or_else(|| {
            CONCEPTS
                .iter()
                .find(|c| c.id.eq_ignore_ascii_case(topic) || c.term.eq_ignore_ascii_case(topic))
                .map(concept_entry)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teach::markup;

    #[test]
    fn every_link_and_see_also_exists() {
        let all = entries();
        let ids: Vec<&str> = all.iter().map(|e| e.id.as_str()).collect();
        let mut seen = std::collections::HashSet::new();
        assert!(ids.iter().all(|id| seen.insert(*id)), "ids are unique");
        for e in &all {
            let texts = e.body.iter().chain(&e.example).chain(e.details);
            let links = texts.flat_map(|text| markup::term_ids(text));
            for id in links.chain(e.see.iter().map(|s| s.to_string())) {
                assert!(ids.contains(&id.as_str()), "{} -> {id}", e.id);
            }
        }
    }

    #[test]
    fn every_rule_has_an_entry_and_lookup_finds_by_name_or_term() {
        for topic in help::RULES {
            assert!(
                entry(&format!("rule-{}", topic.name)).is_some(),
                "{}",
                topic.name
            );
        }
        // A rule's name finds it, unless a concept has that id (`rup`).
        assert_eq!(lookup("pol").unwrap().id, "rule-pol");
        assert_eq!(lookup("rup").unwrap().id, "rup");
        assert_eq!(lookup("slack").unwrap().term, "Slack");
        assert_eq!(lookup("Unit propagation").unwrap().id, "unit-propagation");
        assert_eq!(
            lookup("rule-pol").unwrap().usage.unwrap().split(' ').next(),
            Some("pol")
        );
        assert!(lookup("nothing").is_none());
        assert!(concept_ids().count() >= 25);
    }
}
