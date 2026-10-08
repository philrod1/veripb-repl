//! End-to-end tests for constraint labels and multi-constraint formula
//! lines. Requires `$VERIPB_REPL_VERIPB_BIN`; tests skip otherwise.

use std::io::Write;

use tempfile::NamedTempFile;
use veripb_repl::commands::show;
use veripb_repl::output::Output;
use veripb_repl::session::{AppendOutcome, Session};

/// Two constraints on line 1 (`<==>`), then one each on lines 2 and 3.
const FORMULA: &str = "\
@r_up @r_dn x3 <==> 1 x1 1 x2 >= 1 ;
@c 1 ~x3 >= 1 ;
@d 1 x1 1 ~x2 >= 1 ;
";

macro_rules! require_veripb {
    () => {
        if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
            eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
            return;
        }
    };
}

struct Capture(Vec<String>);

impl Output for Capture {
    fn line(&mut self, text: &str) {
        self.0.push(text.to_string());
    }
}

fn load() -> Session {
    let mut file = NamedTempFile::with_suffix(".opb").expect("failed to create temp formula file");
    file.write_all(FORMULA.as_bytes())
        .expect("failed to write temp formula file");
    Session::load_checked(file.path().to_str().expect("temp path is valid UTF-8"))
        .expect("failed to load session")
}

fn append_ok(session: &mut Session, line: &str) {
    match session
        .append_line(line)
        .expect("append should not fail to invoke")
    {
        AppendOutcome::Verified { .. } => {}
        AppendOutcome::Rejected { error, .. } => panic!("{line:?} was rejected: {error}"),
        AppendOutcome::Deferred => panic!("{line:?} was deferred"),
    }
}

fn show(session: &Session, args: &str) -> String {
    let mut out = Capture(Vec::new());
    show::run(session, args, &mut out);
    out.0.join("\n")
}

#[test]
fn counts_constraints_not_lines() {
    require_veripb!();
    let session = load();
    assert_eq!(session.formula.len(), 3);
    assert_eq!(session.formula_constraint_count(), 4);
    assert_eq!(session.formula_first_ids(), vec![1, 3, 4]);
    assert_eq!(session.preamble_lines()[1], "f 4;");
    assert_eq!(session.formula_line_ids(0), "1-2");
    assert_eq!(session.formula_line_ids(1), "3");
}

#[test]
fn formula_and_proof_labels_name_their_ids() {
    require_veripb!();
    let mut session = load();
    append_ok(&mut session, "@nx1 rup 1 ~x1 >= 1 : @c @r_dn ;");
    let labels = session.label_ids();
    assert_eq!(labels.get("@r_up"), Some(&1));
    assert_eq!(labels.get("@r_dn"), Some(&2));
    assert_eq!(labels.get("@c"), Some(&3));
    assert_eq!(labels.get("@nx1"), Some(&5));
    assert_eq!(
        show(&session, "@nx1"),
        "  ConstraintId 5: @nx1 1 ~x1 >= 1 [derived]"
    );
    assert!(show(&session, "").contains("ConstraintId 2: @r_dn "));
}

#[test]
fn a_later_label_definition_wins() {
    require_veripb!();
    let mut session = load();
    append_ok(&mut session, "@c rup 1 ~x1 >= 1 ;");
    assert_eq!(session.label_ids().get("@c"), Some(&5));
}

#[test]
fn formula_edits_cannot_change_a_lines_constraint_count() {
    require_veripb!();
    let mut session = load();
    let err = session
        .replace_formula_constraint(2, "1 x1 1 x2 >= 1")
        .expect_err("a two-constraint line can't become one");
    assert!(
        format!("{err:#}").contains("loads as 2 constraint(s)"),
        "{err:#}"
    );
    session
        .replace_formula_constraint(3, "@c 1 ~x3 1 x1 >= 1")
        .expect("same count is fine");
    assert_eq!(session.formula[1], "@c 1 ~x3 1 x1 >= 1 ;");
}
