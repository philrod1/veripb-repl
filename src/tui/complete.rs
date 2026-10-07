//! What the current input line could become: the single source both Tab
//! completion and the suggestion strip draw from, so they can never
//! disagree. Five contexts exist so far — the first token (command or
//! rule names, from `:help`'s registry), the first argument of the
//! file-taking commands (`:load`, `:source`, `:save`), completed against
//! the filesystem (relative, absolute, or `~/`-prefixed paths) and
//! filtered to the file type each command actually deals in, `:help`'s
//! argument, completed against the same topic registry the command
//! itself answers from, `:theme`'s argument, completed against the fixed
//! `ThemeName::ALL` list, and — once a bare proof-rule keyword is fully
//! typed — its arguments, completed against live constraint IDs/labels
//! and variable names (see [`reference_candidates`]).

use std::fs;

use crate::commands::help::Topic;
use crate::commands::{expand_tilde, help};
use crate::session::Session;
use crate::tui::theme::ThemeName;

/// One way the current line could be completed.
pub struct Candidate {
    /// The full input line after choosing this candidate.
    pub line: String,
    /// Left column of the suggestion strip (usage, or file name).
    pub left: String,
    /// Right column of the strip (summary; empty for files).
    pub right: String,
}

/// Formula files `:load` offers. Plain text only: formulas are read with
/// `read_to_string`, so compressed `.opb.gz`/`.zst`/`.xz` files don't load.
const OPB_SUFFIXES: &[&str] = &[".opb"];

/// Proof files `:source`/`:save` offer; plain text only.
const PBP_SUFFIXES: &[&str] = &[".pbp"];

/// `:instance` takes either half of a formula/proof pair (or their bare
/// stem, which isn't a file and so isn't offered).
const INSTANCE_SUFFIXES: &[&str] = &[".opb", ".pbp"];

/// Which filename suffixes count as a match for `cmd`'s file argument —
/// `None` means "don't filter" (not one of the file-taking commands).
/// Directories are never filtered by this; only regular files are.
fn accepted_suffixes(cmd: &str) -> Option<&'static [&'static str]> {
    match cmd {
        "load" => Some(OPB_SUFFIXES),
        "source" | "save" => Some(PBP_SUFFIXES),
        "instance" => Some(INSTANCE_SUFFIXES),
        _ => None,
    }
}

/// All candidates for `text` as typed so far, given the live `session` a
/// bare rule line's arguments complete against (`None` before any formula
/// is loaded — nothing to reference yet, so those never offer anything).
/// Empty for an empty line (the strip stays out of the way until typing
/// starts; `:` alone is enough to see every command) and in argument
/// positions we can't complete yet.
///
/// `mode_vocabulary` — `Some` while formula-browse or `:debug` is active
/// (see `formula::MODE_VOCABULARY`/`debug::MODE_VOCABULARY`) — replaces
/// the normal `:`-command registry with exactly what that mode actually
/// recognizes: most of the real registry (`:load`, `:save`, `:reset`,
/// ...) does nothing there, and `:formula cancel`/`:debug`'s own
/// `:step`/`:back`/`:continue`/`:until`/`:restart`/`:break` aren't in the
/// registry at all, so without this the strip would both offer things
/// that don't work and omit the ones that do. Vim mode needs no such
/// carve-out: `Normal` never feeds the prompt at all (so this never runs
/// then), and `Insert` wants the ordinary registry, same as typing a
/// fresh line always has. Affects the first `:`-token directly;
/// argument-position completion (file paths, `:help`/`:theme`
/// arguments) is already unaffected either way — none of the commands
/// that take arguments there are in any mode's vocabulary to begin
/// with, so those branches already fall through to empty on their own.
/// A bare proof-rule line (no leading `:`) is suppressed outright
/// whenever `mode_vocabulary` is `Some`, in both branches below that
/// would otherwise complete one (see each's own note): a no-op for
/// formula-browse, whose retyped constraint text never starts with a
/// rule keyword anyway (so this was already unreachable there in
/// practice), but the meaningful case for `:debug`, which — unlike
/// formula-browse — doesn't accept a bare non-empty line at all (see
/// `commands::debug::handle`), so offering rule-keyword completion for
/// one would just be wrong.
pub fn suggestions(
    text: &str,
    session: Option<&Session>,
    mode_vocabulary: Option<&[&Topic]>,
) -> Vec<Candidate> {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Some((first, rest)) = trimmed.split_once(char::is_whitespace) else {
        return first_token(trimmed, session, mode_vocabulary);
    };

    if let Some(cmd) = first.strip_prefix(':') {
        let partial = rest.trim_start();
        if partial.contains(char::is_whitespace) {
            // Past the first argument (or a path with spaces, which this
            // completer doesn't handle).
            return Vec::new();
        }
        return match accepted_suffixes(cmd) {
            Some(suffixes) => file_candidates(first, Some(suffixes), partial),
            None if cmd == "help" => topic_candidates(first, partial),
            None if cmd == "theme" => theme_candidates(first, partial),
            None => Vec::new(),
        };
    }

    // A bare proof-rule line, past its keyword — unlike a `:`-command,
    // these can run to many tokens (a `pol` expression especially), so
    // every token gets a turn, not just the first. Only once the keyword
    // itself is an exact, resolved name — no completing arguments for a
    // still-ambiguous or unrecognized one. Suppressed outright while a
    // narrower mode vocabulary is live — see this function's own docs.
    if mode_vocabulary.is_some() {
        return Vec::new();
    }
    let Some(session) = session else {
        return Vec::new();
    };
    if !help::RULES.iter().any(|t| t.name == first) {
        return Vec::new();
    }
    let partial = trimmed.rsplit(char::is_whitespace).next().unwrap_or("");
    let kept = &trimmed[..trimmed.len() - partial.len()];
    reference_candidates(kept, first, partial, session)
}

/// Longest common prefix of every candidate's replacement line — what an
/// ambiguous Tab completes to. Char-boundary safe, since file names
/// aren't always ASCII.
pub fn common_line_prefix(candidates: &[Candidate]) -> &str {
    let Some(first) = candidates.first() else {
        return "";
    };
    let mut lcp: &str = &first.line;
    for candidate in &candidates[1..] {
        while !candidate.line.starts_with(lcp) {
            let mut end = lcp.len() - 1;
            while end > 0 && !lcp.is_char_boundary(end) {
                end -= 1;
            }
            lcp = &lcp[..end];
        }
    }
    // A prefix landing right on a trailing '.' only happens when
    // candidates share a stem but diverge in extension (`:instance`
    // listing both a `.opb` and a `.pbp` for the same name is the case
    // that motivated this) — the dot alone isn't a valid path, just an
    // extra character to delete before Enter works. Stop one short
    // instead: the bare stem is still real progress, and for `:instance`
    // specifically it's a complete, valid completion on its own.
    lcp.strip_suffix('.').unwrap_or(lcp)
}

/// Which candidate, if any, should be highlighted before the user has
/// pressed an arrow key — so a bare Tab (which inserts whatever's
/// highlighted, see `App::complete`) lands straight on it instead of
/// stopping at an ambiguous common prefix. Currently just the one case
/// worth defaulting: `:instance` — loading a formula/proof pair to work on
/// is common enough, loaded or not (switching to a different instance
/// mid-session included), that it's always worth pre-selecting whenever
/// it's actually among the candidates, unconditionally on session state —
/// everything else is still one ↑/↓ away, never hidden. Matched by the
/// generated line rather than trusting a fixed index, so this keeps
/// working even if the priority order below ever changes.
pub fn default_selection(candidates: &[Candidate]) -> Option<usize> {
    candidates
        .iter()
        .position(|c| c.line.trim_end() == ":instance")
}

/// Command and rule names complete with a trailing space — landing the
/// cursor ready for whatever comes next (an argument, or just Enter) —
/// unlike a file or `:help` topic completion, where the name itself is
/// usually the whole rest of the line.
///
/// `:instance` sorts first unconditionally (see `default_selection`).
/// Before any formula is loaded, almost every *other* command is a no-op
/// (`:show`, `:edit`, ... all just report "no formula loaded"); only
/// `:load`, `:help`, `:theme`, and `:quit` do anything, so with no
/// `session` yet those sort next (see `help::no_session_tab_rank`) — a
/// stable sort, so anything else keeps `COMMANDS`' own relative order
/// after them. Once a session exists, every other command is equally live
/// again and registry order is left alone (`:instance` aside). None of
/// this reordering applies when `mode_vocabulary` is `Some` — an
/// edit-family mode can't be active without a session already loaded, so
/// the "no session yet" case never arises there, and the mode's own
/// vocabulary is already in a sensible order (action keywords first, the
/// read-only inspection commands after) that doesn't need `:instance`
/// singled out (it isn't even in any mode's vocabulary to begin with).
fn first_token(
    trimmed: &str,
    session: Option<&Session>,
    mode_vocabulary: Option<&[&Topic]>,
) -> Vec<Candidate> {
    match trimmed.strip_prefix(':') {
        Some(prefix) => {
            if let Some(vocabulary) = mode_vocabulary {
                return vocabulary
                    .iter()
                    .filter(|t| t.name.starts_with(prefix))
                    .map(|t| Candidate {
                        line: format!(":{} ", t.name),
                        left: t.usage.to_string(),
                        right: t.summary.to_string(),
                    })
                    .collect();
            }
            let mut matches = help::command_matches(prefix);
            if session.is_none() {
                matches.sort_by_key(|t| help::no_session_tab_rank(t.name));
            } else {
                matches.sort_by_key(|t| t.name != "instance");
            }
            matches
                .into_iter()
                .map(|t| Candidate {
                    line: format!(":{} ", t.name),
                    left: t.usage.to_string(),
                    right: t.summary.to_string(),
                })
                .collect()
        }
        // A bare (no leading `:`) first token, still being typed: rule-name
        // completion — but only outside any narrower mode vocabulary (see
        // this module's `suggestions` docs for why `:debug` specifically
        // needs this suppressed, not just left inert).
        None if mode_vocabulary.is_some() => Vec::new(),
        None => help::rule_matches(trimmed)
            .into_iter()
            .map(|t| Candidate {
                line: format!("{} ", t.name),
                left: t.usage.to_string(),
                right: t.summary.to_string(),
            })
            .collect(),
    }
}

/// Filesystem candidates for `partial` as the argument of `cmd` (the
/// command token as typed, e.g. ":load"). The user's spelling is
/// preserved in the completed line — `~` is expanded only to read the
/// directory, never written back expanded. Directories complete with a
/// trailing `/` so the next Tab drills into them, regardless of
/// `suffixes` — you need to navigate through a directory whatever its
/// name, and there's no way to know what's inside without listing it;
/// only regular files are filtered, case-insensitively, to the file type
/// the command actually deals in (`None` = show everything, for a
/// command without a fixed file type). Dot-files stay hidden until the
/// partial itself starts with a dot, shell-style. A directory that can't
/// be read simply yields no candidates.
fn file_candidates(
    cmd: &str,
    suffixes: Option<&'static [&'static str]>,
    partial: &str,
) -> Vec<Candidate> {
    // A lone `~` can't be treated as a name filter in `.` — offer the
    // home directory itself and let the next Tab list inside it.
    if partial == "~" {
        return vec![Candidate {
            line: format!("{cmd} ~/"),
            left: "~/".to_string(),
            right: String::new(),
        }];
    }

    let (dir_part, base) = match partial.rfind('/') {
        Some(i) => partial.split_at(i + 1),
        None => ("", partial),
    };
    let read_dir = if dir_part.is_empty() {
        ".".to_string()
    } else {
        expand_tilde(dir_part)
    };
    let Ok(entries) = fs::read_dir(&read_dir) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        // Non-UTF-8 names can't round-trip through the line editor; skip.
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(base) || (name.starts_with('.') && !base.starts_with('.')) {
            continue;
        }
        // `metadata` (not `file_type`) so a symlink to a directory also
        // completes with a trailing slash.
        let is_dir = fs::metadata(entry.path())
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if !is_dir && !matches_suffix(name, suffixes) {
            continue;
        }
        let suffix = if is_dir { "/" } else { "" };
        out.push(Candidate {
            line: format!("{cmd} {dir_part}{name}{suffix}"),
            left: format!("{name}{suffix}"),
            right: String::new(),
        });
    }
    out.sort_by(|a, b| a.left.cmp(&b.left));
    out
}

/// Whether a file name matches one of `suffixes` (`None` = accept
/// anything). Case-insensitive, since filesystems that matter here
/// (macOS, Windows) commonly are too — `Formula.OPB` should still show
/// up for `:load`.
fn matches_suffix(name: &str, suffixes: Option<&'static [&'static str]>) -> bool {
    let Some(suffixes) = suffixes else {
        return true;
    };
    let lower = name.to_ascii_lowercase();
    suffixes.iter().any(|suf| lower.ends_with(suf))
}

/// `:help`'s argument: topic names from the same registry the command
/// answers from. A leading `:` (help accepts either spelling) narrows to
/// commands and is kept in the completion; a bare partial matches
/// commands and rules alike.
fn topic_candidates(cmd: &str, partial: &str) -> Vec<Candidate> {
    let (colon, bare) = match partial.strip_prefix(':') {
        Some(bare) => (":", bare),
        None => ("", partial),
    };
    let mut topics = help::command_matches(bare);
    if colon.is_empty() {
        topics.extend(help::rule_matches(bare));
    }
    topics
        .into_iter()
        .map(|t| Candidate {
            line: format!("{cmd} {colon}{}", t.name),
            left: t.usage.to_string(),
            right: t.summary.to_string(),
        })
        .collect()
}

/// `:theme`'s argument: the three fixed theme names — no filesystem or
/// registry involved, just `ThemeName::ALL` filtered by prefix like
/// everything else here.
fn theme_candidates(cmd: &str, partial: &str) -> Vec<Candidate> {
    ThemeName::ALL
        .into_iter()
        .filter(|t| t.name().starts_with(partial))
        .map(|t| Candidate {
            line: format!("{cmd} {}", t.name()),
            left: t.name().to_string(),
            right: String::new(),
        })
        .collect()
}

/// Which pool(s) of live references are worth offering for a resolved
/// rule's arguments. Coarse and per-*rule*, not per-token-position — a
/// real grammar model would know that, say, `red`'s constraint text and
/// its substitution witness want different things, but that's a lot more
/// machinery for not much daily benefit; this covers the common cases
/// without it.
#[derive(Clone, Copy)]
enum RefKind {
    /// Only ever names a variable/literal directly — constraint text, a
    /// substitution witness, or a logged solution — never a reference to
    /// something already in the database.
    Variable,
    /// Only ever references something already in the database.
    Constraint,
    /// Could plausibly be either at any position, so both pools are
    /// offered and the typed prefix (a digit, `@`, `~`, or a name) sorts
    /// out which one the user actually meant. `pol`'s reverse-polish
    /// sequence is the clearest example — an operand can be a database
    /// reference or a fresh literal to push — but this is also the safe
    /// default for anything not explicitly classified below.
    Both,
}

fn rule_ref_kind(rule: &str) -> RefKind {
    match rule {
        "red" | "pbc" | "obju" | "sol" | "soli" | "solx" | "a" | "preserved_add"
        | "preserved_rm" | "epreserved" => RefKind::Variable,
        "delc" | "deld" | "core" | "proofgoal" | "qed" => RefKind::Constraint,
        _ => RefKind::Both,
    }
}

/// Completions for whichever token comes after a resolved bare rule
/// keyword. `kept` is everything typed before that token (so a candidate
/// line is just `kept` plus the replacement) and `partial` is the token
/// itself, prefix-matched against live constraint IDs, live labels, and
/// variable names (both bare and negated, since either can appear as a
/// literal) depending on what `rule_ref_kind` says `rule` takes.
fn reference_candidates(
    kept: &str,
    rule: &str,
    partial: &str,
    session: &Session,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    let kind = rule_ref_kind(rule);

    if matches!(kind, RefKind::Constraint | RefKind::Both)
        && let Ok(database) = session.database()
    {
        let labels_by_id = session.labels_by_id();
        for entry in &database.entries {
            let id_str = entry.id.to_string();
            if id_str.starts_with(partial) {
                out.push(Candidate {
                    line: format!("{kept}{id_str}"),
                    left: id_str.clone(),
                    right: String::new(),
                });
            }
            for name in labels_by_id.get(&(entry.id as isize)).into_iter().flatten() {
                if name.starts_with(partial) {
                    out.push(Candidate {
                        line: format!("{kept}{name}"),
                        left: name.clone(),
                        right: String::new(),
                    });
                }
            }
        }
    }

    if matches!(kind, RefKind::Variable | RefKind::Both) {
        for name in session.variables.names() {
            for candidate in [name.to_string(), format!("~{name}")] {
                if candidate.starts_with(partial) {
                    out.push(Candidate {
                        line: format!("{kept}{candidate}"),
                        left: candidate.clone(),
                        right: String::new(),
                    });
                }
            }
        }
    }

    out.sort_by(|a, b| a.left.cmp(&b.left));
    out
}
