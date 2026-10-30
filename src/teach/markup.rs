//! The markup explanations and glossary entries are written in: `` `code` ``,
//! `[text](g:<glossary id>)` for a link to a glossary entry, and
//! `[text](c:<constraint id>)` for a link to a constraint. Anything else,
//! including markup that doesn't parse, is plain text.

/// One run of marked-up text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    /// Constraints, rule text and commands, without the backticks.
    Code(String),
    /// A link to glossary entry `id`.
    Term {
        text: String,
        id: String,
    },
    /// A link to constraint `id`.
    Constraint {
        text: String,
        id: usize,
    },
}

/// Splits `text` into its runs.
pub fn parse(text: &str) -> Vec<Inline> {
    let mut runs = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some((inline, after)) = code(rest).or_else(|| link(rest)) {
            if !plain.is_empty() {
                runs.push(Inline::Text(std::mem::take(&mut plain)));
            }
            runs.push(inline);
            rest = after;
        } else {
            plain.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    if !plain.is_empty() {
        runs.push(Inline::Text(plain));
    }
    runs
}

/// `` `code` `` at the start of `text`, and what follows it.
fn code(text: &str) -> Option<(Inline, &str)> {
    let body = text.strip_prefix('`')?;
    let end = body.find('`')?;
    Some((Inline::Code(body[..end].to_string()), &body[end + 1..]))
}

/// `[text](g:id)` or `[text](c:id)` at the start of `text`, and what
/// follows it.
fn link(text: &str) -> Option<(Inline, &str)> {
    let body = text.strip_prefix('[')?;
    let close = body.find("](")?;
    let label = &body[..close];
    let target = &body[close + 2..];
    let end = target.find(')')?;
    let (scheme, id) = target[..end].split_once(':')?;
    let after = &target[end + 1..];
    let inline = match scheme {
        "g" if !id.is_empty() => Inline::Term {
            text: label.to_string(),
            id: id.to_string(),
        },
        "c" => Inline::Constraint {
            text: label.to_string(),
            id: id.parse().ok()?,
        },
        _ => return None,
    };
    Some((inline, after))
}

/// `text` for a plain-text frontend: links become their text; code keeps
/// its backticks.
pub fn plain(text: &str) -> String {
    parse(text)
        .into_iter()
        .map(|inline| match inline {
            Inline::Text(text) => text,
            Inline::Code(code) => format!("`{code}`"),
            Inline::Term { text, .. } | Inline::Constraint { text, .. } => text,
        })
        .collect()
}

/// The glossary ids `text` links to.
pub fn term_ids(text: &str) -> Vec<String> {
    parse(text)
        .into_iter()
        .filter_map(|inline| match inline {
            Inline::Term { id, .. } => Some(id),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_and_links_are_runs() {
        assert_eq!(
            parse("Add `@a` and [constraint 3](c:3), by [addition](g:addition)."),
            [
                Inline::Text("Add ".into()),
                Inline::Code("@a".into()),
                Inline::Text(" and ".into()),
                Inline::Constraint {
                    text: "constraint 3".into(),
                    id: 3
                },
                Inline::Text(", by ".into()),
                Inline::Term {
                    text: "addition".into(),
                    id: "addition".into()
                },
                Inline::Text(".".into()),
            ]
        );
    }

    #[test]
    fn broken_markup_is_text() {
        for text in [
            "x [y] z",
            "a ` b",
            "[c](q:1)",
            "[c](c:x)",
            "[c](g:)",
            "[unclosed](g:x",
            "≥ [é](g:é)",
        ] {
            let runs = parse(text);
            let rebuilt: String = runs
                .iter()
                .map(|inline| match inline {
                    Inline::Text(text) => text.clone(),
                    Inline::Term { text, id } => format!("[{text}](g:{id})"),
                    other => panic!("{text}: {other:?}"),
                })
                .collect();
            assert_eq!(rebuilt, text);
        }
    }

    #[test]
    fn plain_text_keeps_code_and_drops_link_markup() {
        assert_eq!(
            plain("The claim: `x1 ≥ 1`, see [RUP](g:rup) and [constraint 6](c:6)."),
            "The claim: `x1 ≥ 1`, see RUP and constraint 6."
        );
        assert_eq!(term_ids("[a](g:x) [b](c:2) [c](g:y)"), ["x", "y"]);
    }
}
