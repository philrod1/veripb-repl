//! The set of variable names a loaded formula mentions — this REPL's
//! replacement for `veripb_formula::VarNameManager` now that it has no
//! compile-time dependency on any VeriPB library crate at all (checking
//! itself happens by shelling out to an installed `veripb` binary; see
//! `checker/`). `VarNameManager` gave every variable a stable index and
//! answered both "what's the Nth variable's name?" and "does this name
//! have an index?" — but nothing left in this codebase ever needs the
//! index half once checking moves out-of-process: the two things every
//! caller actually did with it were syntax-highlighting raw proof-rule
//! text (a pure "is this token a known variable name?" membership test)
//! and tab-completing variable-name arguments (iterate every known
//! name). Both are just as well served by a plain set of names, so
//! that's all this is.
//!
//! Built by scanning the *formula file itself* rather than asking the
//! `veripb` subprocess for a variable list — the formula's own grammar
//! (`docs/grammar.tex`'s XPB section, in the main VeriPB repo) is tight
//! enough that a lightweight per-line token scan, no real parser needed,
//! gets this right for any well-formed file: skip `*`-comment lines,
//! whitespace-tokenize what's left, and every token that's shaped like a
//! variable name (see [`looks_like_variable_name`]) is one, since the
//! grammar leaves nothing else that shape in a constraint or objective
//! line — coefficients are digit sequences, the relational operators and
//! `;` don't fit the shape, and `@labels`/`min:`/`max:` all contain a
//! character the shape forbids. This can only go wrong on a malformed
//! file, in which case the fallback is the same one every other lookup-
//! based heuristic in this codebase already accepts: under-detect rather
//! than mislabel something else as a variable. If this ever proves
//! unreliable in practice, the fix is a `--dump-variables`-style flag on
//! the core `veripb` binary, not a fancier scan here.

use std::io;
use std::path::Path;

use ahash::AHashSet;

/// Every variable name mentioned in a loaded formula, as plain strings —
/// no indices, no ordering guarantees beyond what [`Self::mentioned`]
/// promises for its own return value.
pub struct VarNames {
    known: AHashSet<String>,
}

impl VarNames {
    /// Scan an OPB/XPB-format formula file for its variable names.
    pub fn from_formula_file(path: impl AsRef<Path>) -> io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self::from_formula_text(&text))
    }

    /// The scan itself, split out from [`Self::from_formula_file`] so the
    /// logic doesn't depend on the source being a real file on disk.
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

    /// Every known variable name `text` mentions.  Uses to find
    /// which live Database rows share a variable with the assertion
    /// being replaced.
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

    /// Every known variable name, in arbitrary order, for tab
    /// completion to offer `x1`/`~x1`-style candidates.
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

/// Does it look like a variable name per the VeriPB v3 grammar?
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

