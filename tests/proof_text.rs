//! Tests for `checker::ProofText`, the proof as written to `veripb`.

use veripb_repl::checker::ProofText;

fn lines(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|t| t.to_string()).collect()
}

fn written(proof: ProofText) -> String {
    let mut out = Vec::new();
    proof.write_to(&mut out, || true).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn lines_are_written_one_to_a_line() {
    let preamble = lines(&["pseudo-Boolean proof version 3.0", "f 2;"]);
    let body = lines(&["rup 1 x1 >= 1 ;", "@l pol 1 2 + ;"]);
    let tail = lines(&[
        "output NONE;",
        "conclusion NONE;",
        "end pseudo-Boolean proof;",
    ]);
    let proof = ProofText::Lines {
        preamble: &preamble,
        body: &body,
        tail: &tail,
    };
    let expected = preamble
        .iter()
        .chain(&body)
        .chain(&tail)
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    assert_eq!(written(proof), expected);
    assert_eq!(proof.line_count(), 7);

    let no_tail = ProofText::Lines {
        preamble: &preamble,
        body: &body[..1],
        tail: &[],
    };
    assert_eq!(
        written(no_tail),
        "pseudo-Boolean proof version 3.0\nf 2;\nrup 1 x1 >= 1 ;\n"
    );
    assert_eq!(no_tail.line_count(), 3);
}

#[test]
fn text_is_written_as_it_is() {
    for text in ["a\nb\n", "a\nb", "", "\n\n"] {
        assert_eq!(written(ProofText::from(text)), text);
        assert_eq!(ProofText::from(text).line_count(), text.lines().count());
    }
}

#[test]
fn writing_stops_when_asked() {
    let body = lines(&["1", "2", "3", "4"]);
    let proof = ProofText::Lines {
        preamble: &[],
        body: &body,
        tail: &[],
    };
    let mut out = Vec::new();
    let mut count = 0;
    proof
        .write_to(&mut out, || {
            count += 1;
            count < 2
        })
        .unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "1\n2\n");
}
