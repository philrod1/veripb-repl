//! Finding lines in the Formula, Database and Proof panes: what a query
//! typed into a pane's find means, and which lines it matches.
//!
//! - `1234` goes to line 1234 (Proof) or constraint ID 1234 (Formula,
//!   Database).
//! - `$` goes to the last line.
//! - `@name` goes to where that label is defined; if it isn't a label, it
//!   is text, as below.
//! - `/regex/` shows the lines matching the regular expression (the closing
//!   `/` is optional). It is case-sensitive unless it starts with `(?i)`.
//! - `"text"` shows the lines containing `text`, taken literally (the
//!   closing `"` is optional).
//! - Anything else shows the lines containing it, ignoring case.

use regex::{Regex, RegexBuilder};

/// A parsed find query; see the module docs.
pub enum Query {
    /// `$`.
    End,
    /// A line number (Proof) or constraint ID (Formula, Database).
    Line(usize),
    /// `@name`, with the filter to use if it isn't a label.
    Label(String, Filter),
    Filter(Filter),
}

/// Which lines a query shows.
#[derive(Clone)]
pub struct Filter {
    regex: Regex,
    /// The text, lowercased, for a query that matches text ignoring case.
    text: Option<String>,
}

impl Query {
    /// Parses `query`; `Ok(None)` if it is blank. Fails on an invalid
    /// regular expression.
    pub fn parse(query: &str) -> Result<Option<Query>, regex::Error> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(None);
        }
        if query == "$" {
            return Ok(Some(Query::End));
        }
        if let Ok(line) = query.parse() {
            return Ok(Some(Query::Line(line)));
        }
        if let Some(pattern) = query.strip_prefix('/') {
            let pattern = pattern.strip_suffix('/').unwrap_or(pattern);
            return Ok(Some(Query::Filter(Filter {
                regex: Regex::new(pattern)?,
                text: None,
            })));
        }
        if let Some(text) = query.strip_prefix('"') {
            let text = text.strip_suffix('"').unwrap_or(text);
            return Ok(Some(Query::Filter(Filter::text(text))));
        }
        if query.starts_with('@') && !query.contains(char::is_whitespace) {
            return Ok(Some(Query::Label(query.to_string(), Filter::text(query))));
        }
        Ok(Some(Query::Filter(Filter::text(query))))
    }
}

impl Filter {
    /// Lines containing `text`, ignoring ASCII case.
    fn text(text: &str) -> Filter {
        let regex = RegexBuilder::new(&regex::escape(text))
            .unicode(false)
            .case_insensitive(true)
            .build()
            .expect("an escaped literal is a valid pattern");
        Filter {
            regex,
            text: Some(text.to_ascii_lowercase()),
        }
    }

    pub fn matches(&self, line: &str) -> bool {
        self.regex.is_match(line)
    }

    /// Whether every line this matches is also matched by `previous`, so
    /// only `previous`'s matches need checking.
    pub fn narrows(&self, previous: &Filter) -> bool {
        match (&self.text, &previous.text) {
            (Some(text), Some(previous)) => text.contains(previous.as_str()),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(query: &str) -> Filter {
        match Query::parse(query) {
            Ok(Some(Query::Filter(filter) | Query::Label(_, filter))) => filter,
            _ => panic!("not a filter: {query}"),
        }
    }

    #[test]
    fn numbers_dollar_and_labels_go_somewhere() {
        assert!(matches!(
            Query::parse(" 1234 "),
            Ok(Some(Query::Line(1234)))
        ));
        assert!(matches!(Query::parse("$"), Ok(Some(Query::End))));
        assert!(matches!(Query::parse("@u5"), Ok(Some(Query::Label(label, _))) if label == "@u5"));
        assert!(matches!(Query::parse("  "), Ok(None)));
    }

    #[test]
    fn text_ignores_case_and_quotes_take_it_literally() {
        let rup = filter("RUP 1 ~X7");
        assert!(rup.matches("rup 1 ~x7 >= 1 ;"));
        assert!(!rup.matches("rup 1 x7 >= 1 ;"));

        let number = filter("\"123");
        assert!(number.matches("pol 123 4 +"));
        let dollar = filter("\"$\"");
        assert!(dollar.matches("a$b"));
        assert!(!dollar.matches("ab"));
        assert!(filter("x.y").matches("x.y") && !filter("x.y").matches("xzy"));
        assert!(filter("@u5 >=").matches("@u5 >= 1"));
    }

    #[test]
    fn regexes_match_whole_words_and_respect_case() {
        let word = filter("/x12\\b/");
        assert!(word.matches("1 x12 >= 1"));
        assert!(!word.matches("1 x120 >= 1"));
        assert!(filter("/^red").matches("red 1 x1 >= 1 ; x1 -> 1"));
        assert!(!filter("/RUP/").matches("rup"));
        assert!(filter("/(?i)RUP/").matches("rup"));
        assert!(Query::parse("/x(").is_err());
    }

    #[test]
    fn longer_text_narrows_shorter_text() {
        assert!(filter("rup ~x7").narrows(&filter("RUP")));
        assert!(!filter("rup").narrows(&filter("rup ~x7")));
        assert!(!filter("/rup x/").narrows(&filter("rup")));
        assert!(!filter("rup").narrows(&filter("/rup/")));
    }
}
