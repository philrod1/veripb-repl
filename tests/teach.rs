//! Tests for the plain-English explanations (`teach::explain`), from the
//! web REPL's `teach-check.mjs`: sample `rup`, `pol`, `red`, `a`, `setlvl`
//! and `wiplvl` lines give the expected steps, and every such checked line
//! of veripb's version-3 instances gets an explanation whose links exist.
//! Require `$VERIPB_REPL_VERIPB_BIN`; skip otherwise.

use std::path::{Path, PathBuf};

use veripb_repl::commands;
use veripb_repl::output::Output;
use veripb_repl::session::Session;
use veripb_repl::teach::explain::{Block, Explanation, explain};
use veripb_repl::teach::{glossary, markup};

macro_rules! require_veripb {
    () => {
        if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
            eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
            return;
        }
    };
}

struct Discard;

impl Output for Discard {
    fn line(&mut self, _: &str) {}
}

fn demos() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("demos/files")
}

fn instances() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("veripb-src/tests/instances/correct/version3")
}

/// A session over `<dir>/<base>.opb` and `.pbp`, with buffer lines
/// replaced by `edits` (buffer index, text), verified.
fn session(dir: &Path, base: &str, edits: &[(usize, &str)]) -> Option<Session> {
    let formula = dir.join(format!("{base}.opb"));
    let proof = dir.join(format!("{base}.pbp"));
    let mut session = Some(Session::load(formula.to_str()?).ok()?);
    let mut out = Discard;
    commands::dispatch(
        &mut session,
        &format!(":source {}", proof.display()),
        &mut out,
    )
    .ok()?;
    for (idx, text) in edits {
        session.as_mut()?.set_line(*idx, text).ok()?;
    }
    session.as_mut()?.drive_forward().ok()?;
    session
}

/// The explanation as text, links read as their text.
fn text(explanation: &Explanation) -> String {
    explanation
        .blocks
        .iter()
        .map(|block| match block {
            Block::Paragraph(text) | Block::Note(text) => markup::plain(text),
            Block::Steps(items) => items
                .iter()
                .map(|item| markup::plain(item))
                .collect::<Vec<_>>()
                .join("\n"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn steps(explanation: &Explanation) -> usize {
    explanation
        .blocks
        .iter()
        .find_map(|block| match block {
            Block::Steps(items) => Some(items.len()),
            _ => None,
        })
        .unwrap_or(0)
}

/// Line `n` of `session` explains with a title containing `title`, every
/// one of `phrases`, and `steps` steps if given.
fn content(
    session: &Session,
    n: usize,
    title: &str,
    steps_expected: Option<usize>,
    phrases: &[&str],
) {
    let explanation = explain(session, n).unwrap_or_else(|| panic!("line {n}: no explanation"));
    assert!(
        explanation.title.contains(title),
        "line {n}: {}",
        explanation.title
    );
    let body = text(&explanation);
    for phrase in phrases {
        assert!(
            body.contains(phrase),
            "line {n}: missing {phrase:?} in\n{body}"
        );
    }
    if let Some(expected) = steps_expected {
        assert_eq!(steps(&explanation), expected, "line {n}:\n{body}");
    }
}

#[test]
fn demo_lines_explain_step_by_step() {
    require_veripb!();
    let chain = session(
        &demos(),
        "chain",
        &[(0, "@x3 rup 1 x3 >= 1 : @x1_or_x2 @x1_imp_x3 @x2_imp_x3 ;")],
    )
    .unwrap();
    content(
        &chain,
        3,
        "reverse unit propagation",
        Some(4),
        &[
            "Suppose `x3 ≥ 1` is false",
            "That forces x3 = 0",
            "With x1 = 0 and x2 = 0",
            "a contradiction",
            "as `@x3` (constraint 6)",
        ],
    );
    content(
        &chain,
        5,
        "reverse unit propagation",
        Some(2),
        &["claims `0 ≥ 1`", "constraints are contradictory"],
    );

    let two_of_three = session(&demos(), "two-of-three", &[]).unwrap();
    content(
        &two_of_three,
        3,
        "cutting planes",
        Some(3),
        &["Add `@not_12` and `@not_13`", "`@sum_12_13` (constraint 5)"],
    );
    content(
        &two_of_three,
        5,
        "cutting planes",
        Some(2),
        &[
            "Divide `@sum_all` by 2, rounding up",
            "at least 3/2, it is at least 2",
        ],
    );
    content(
        &two_of_three,
        6,
        "cutting planes",
        None,
        &["`x1` cancels against `~x1`"],
    );
}

#[test]
fn instance_lines_explain_redundance_assertions_and_levels() {
    require_veripb!();
    let symmetry = session(&instances(), "redundance_explicit_subproof", &[]).unwrap();
    content(
        &symmetry,
        6,
        "cutting planes",
        None,
        &["Saturate constraint 1", "at the right-hand side, 1"],
    );
    content(
        &symmetry,
        15,
        "redundance",
        Some(2),
        &[
            "witness is x1 → x3",
            "(so `~x1 ≥ 1` holds)",
            "Proved explicitly in lines 16–25",
            "lines 30–33",
            "Constraint 1, `2 ~x1 + ~x3 ≥ 1`,",
        ],
    );
    content(
        &symmetry,
        24,
        "cutting planes",
        None,
        &[
            "literal axiom `x6 ≥ 0`",
            "constraint -1 (the newest one)",
            "result is `0 ≥ 1`",
        ],
    );
    content(
        &symmetry,
        38,
        "reverse unit propagation",
        None,
        &["Constraint 18: `x1 ≥ 1`", "a contradiction"],
    );

    let objective = session(&instances(), "redundance_symmetry_with_objective", &[]).unwrap();
    content(
        &objective,
        3,
        "redundance",
        Some(4),
        &[
            "nor the best one",
            "no worse for the objective",
            "follows directly from assuming C is false",
            "by unit propagation",
        ],
    );
    content(
        &objective,
        4,
        "redundance",
        None,
        &["with no witness", "checks it as a `rup` step"],
    );

    let assertion = session(&instances(), "assertion_rule", &[]).unwrap();
    content(
        &assertion,
        5,
        "assertion",
        None,
        &["without any proof", "`:deassert 5`"],
    );

    // setlvl 0; pol (6); e; wiplvl 0; setlvl 1; pol (7); setlvl 0; pol (8); wiplvl 1
    let levels = session(&instances(), "set_level_withdraw", &[]).unwrap();
    content(
        &levels,
        3,
        "set level",
        None,
        &[
            "current level to 0",
            "no level was set",
            "Line 6 (`wiplvl 0`)",
        ],
    );
    content(
        &levels,
        6,
        "wipe level",
        None,
        &[
            "level 0 or higher",
            "line 3 (`setlvl 0`)",
            "deletes 1 constraint: constraint 6.",
        ],
    );
    content(
        &levels,
        7,
        "set level",
        None,
        &[
            "Before it, the level was 0 (set on line 3)",
            "Line 11 (`wiplvl 1`)",
        ],
    );
    content(
        &levels,
        9,
        "set level",
        None,
        &["No later `wiplvl` deletes level 0"],
    );
    content(
        &levels,
        11,
        "wipe level",
        None,
        &["line 7 (`setlvl 1`)", "deletes 1 constraint: constraint 7."],
    );
}

#[test]
fn every_instance_line_explains_and_links_exist() {
    require_veripb!();
    let ids: Vec<String> = glossary::entries().into_iter().map(|e| e.id).collect();
    let covered = ["rup", "pol", "red", "a", "setlvl", "wiplvl"];
    let (mut lines, mut missing, mut unfinished, mut bad_links) = (0, vec![], vec![], vec![]);
    let mut bases: Vec<String> = std::fs::read_dir(instances())
        .unwrap()
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            Some(name.strip_suffix(".pbp")?.to_string())
        })
        .filter(|base| instances().join(format!("{base}.opb")).exists())
        .collect();
    bases.sort();
    for base in bases {
        let Some(session) = session(&instances(), &base, &[]) else {
            continue;
        };
        for idx in 0..session.checked_len {
            let line = &session.buffer[idx];
            let rule = line.split_whitespace().find(|t| !t.starts_with('@'));
            if !rule.is_some_and(|rule| covered.contains(&rule))
                || line.contains("==>")
                || line.contains("<==")
            {
                continue;
            }
            lines += 1;
            let n = session.display_line(idx);
            let Some(explanation) = explain(&session, n) else {
                missing.push(format!("{base}:{n} {line}"));
                continue;
            };
            if text(&explanation).contains("did not reach") {
                unfinished.push(format!("{base}:{n} {line}"));
            }
            let blocks = explanation.blocks.iter().flat_map(|block| match block {
                Block::Paragraph(text) | Block::Note(text) => vec![text.clone()],
                Block::Steps(items) => items.clone(),
            });
            for id in blocks.flat_map(|text| markup::term_ids(&text)) {
                if !ids.contains(&id) {
                    bad_links.push(format!("{base}:{n} -> {id}"));
                }
            }
        }
    }
    assert!(lines > 100, "{lines} lines explained");
    assert!(missing.is_empty(), "no explanation: {missing:#?}");
    assert!(
        unfinished.is_empty(),
        "replay did not finish: {unfinished:#?}"
    );
    assert!(
        bad_links.is_empty(),
        "missing glossary entries: {bad_links:#?}"
    );
}

struct Capture(Vec<String>);

impl Output for Capture {
    fn line(&mut self, text: &str) {
        self.0.push(text.to_string());
    }
}

fn run(session: &mut Option<Session>, command: &str) -> Vec<String> {
    let mut out = Capture(Vec::new());
    commands::dispatch(session, command, &mut out).expect("dispatch");
    out.0
}

#[test]
fn explain_prints_plain_english_then_the_checker() {
    require_veripb!();
    let mut session = session(&demos(), "two-of-three", &[]);
    let output = run(&mut session, ":explain 5");
    assert_eq!(output[0], "Line 5: cutting planes (pol)");
    assert!(
        output[2].starts_with("This line uses cutting planes:"),
        "{output:#?}"
    );
    assert!(output.iter().any(|l| l.starts_with("  1. Take `@sum_all`")));
    let checker = output
        .iter()
        .position(|l| l == "Checker:")
        .expect("Checker:");
    assert!(
        output[checker + 1..]
            .iter()
            .any(|l| l.contains("ConstraintId 7:")),
        "{output:#?}"
    );
    // A rule without a plain explanation: the checker's account only.
    let mut session = self::session(&instances(), "redundance_explicit_subproof", &[]);
    let plain = run(&mut session, ":explain 3");
    assert!(!plain.iter().any(|l| l == "Checker:"), "{plain:#?}");
}

#[test]
fn help_and_question_marks_show_concepts() {
    let formula = demos().join("chain.opb");
    let mut session = Some(Session::load(formula.to_str().unwrap()).unwrap());
    let slack = run(&mut session, ":help slack");
    assert_eq!(slack[0], "Slack");
    assert_eq!(slack.last().unwrap(), "  See also: :help unit-propagation");
    assert_eq!(run(&mut session, "?slack"), slack);
    assert_eq!(run(&mut session, "?Slack"), slack, "by term, ignoring case");

    let rup = run(&mut session, "?rup");
    assert!(rup[0].starts_with("rup <constraint>"), "{rup:#?}");
    assert!(rup.contains(&"Reverse unit propagation (RUP)".to_string()));

    let list = run(&mut session, ":help");
    let concepts = list.iter().position(|l| l.starts_with("Concepts")).unwrap();
    assert!(list[concepts + 1].contains("unit-propagation"));

    session
        .as_mut()
        .unwrap()
        .buffer
        .push("rup 1 x1 >= 1 ;".to_string());
    if let Ok(commands::Flow::StartDebug(mut state)) =
        commands::dispatch(&mut session, ":debug", &mut Capture(Vec::new()))
    {
        let mut out = Capture(Vec::new());
        commands::debug::handle(session.as_mut().unwrap(), &mut state, "?slack", &mut out).unwrap();
        assert_eq!(out.0, slack, "in :debug too");
    } else {
        panic!(":debug did not start");
    }
}
