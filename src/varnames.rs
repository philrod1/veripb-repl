//! The set of variable names a formula mentions, for syntax highlighting and
//! tab completion.
//!
//! Built by a token scan of the formula file: skip `*` comment lines, split
//! on whitespace, strip a leading `~`, and keep tokens matching
//! `looks_like_variable_name`. In well-formed OPB/XPB nothing else has that
//! shape (coefficients are digits; operators, `;`, `@labels`, `min:`/`max:`
//! don't match). On malformed input it may under-detect, never mislabel.

use std::io;
use std::path::Path;

use ahash::AHashSet;

/// The unordered set of variable names in a formula.
pub struct VarNames {
    known: AHashSet<String>,
}

impl VarNames {
    /// Scans an OPB/XPB formula file for its variable names.
    pub fn from_formula_file(path: impl AsRef<Path>) -> io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self::from_formula_text(&text))
    }

    /// Scans formula text for its variable names.
    pub(crate) fn from_formula_text(text: &str) -> Self {
        let mut known = AHashSet::new();
        for line in text.lines() {
            // Skip comments
            if line.trim_start().starts_with('*') {
                continue;
            }
            for token in line.split_whitespace() {
                let name = token.strip_prefix('~').unwrap_or(token);
                if looks_like_variable_name(name) {
                    known.insert(name.to_string());
                }
            }
        }
        VarNames { known }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.known.contains(name)
    }

    /// Returns the known variable names `text` mentions, deduplicated, in
    /// order of first occurrence.
    pub fn mentioned(&self, text: &str) -> Vec<String> {
        let mut vars = Vec::new();
        for token in text.split_whitespace() {
            let name = token.strip_prefix('~').unwrap_or(token);
            if self.known.contains(name) && !vars.iter().any(|v| v == name) {
                vars.push(name.to_string());
            }
        }
        vars
    }

    /// Returns every known variable name, in arbitrary order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.known.iter().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.known.len()
    }

    pub fn is_empty(&self) -> bool {
        self.known.is_empty()
    }
}

/// Returns whether `tok` is a VeriPB v3 variable name: a letter or `_`, then
/// one or more of alphanumerics and `_^[]{}-`.
fn looks_like_variable_name(tok: &str) -> bool {
    let mut chars = tok.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    let mut has_second_symbol = false;
    for c in chars {
        has_second_symbol = true;
        if !(c.is_ascii_alphanumeric() || matches!(c, '_' | '^' | '[' | ']' | '{' | '}' | '-')) {
            return false;
        }
    }
    has_second_symbol
}
