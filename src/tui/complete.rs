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

/// The most candidates [`reference_candidates`] builds: a large proof's
/// database can match millions. The rest are only counted.
const REFERENCE_CANDIDATE_CAP: usize = 1000;

/// What the current line could become: the candidates, in order, plus any
/// further matches counted but not built (see [`REFERENCE_CANDIDATE_CAP`]).
#[derive(Default)]
pub struct Suggestions {
    pub candidates: Vec<Candidate>,
    /// Matches after `candidates`, not built.
    pub omitted: usize,
    /// When `omitted > 0`: the longest common prefix of every match's line.
    common_line: String,
}

impl From<Vec<Candidate>> for Suggestions {
    fn from(candidates: Vec<Candidate>) -> Self {
        Suggestions {
            candidates,
            omitted: 0,
            common_line: String::new(),
        }
    }
}

impl Suggestions {
    /// Longest common prefix of every match's line — what an ambiguous Tab
    /// completes to. See [`common_line_prefix`].
    pub fn common_line_prefix(&self) -> &str {
        if self.omitted == 0 {
            common_line_prefix(&self.candidates)
        } else {
            &self.common_line
        }
    }
}

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
) -> Suggestions {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return Suggestions::default();
    }
    let Some((first, rest)) = trimmed.split_once(char::is_whitespace) else {
        return first_token(trimmed, session, mode_vocabulary).into();
    };

    if let Some(cmd) = first.strip_prefix(':') {
        let partial = rest.trim_start();
        if partial.contains(char::is_whitespace) {
            // Past the first argument (or a path with spaces, which this
            // completer doesn't handle).
            return Suggestions::default();
        }
        return match accepted_suffixes(cmd) {
            Some(suffixes) => file_candidates(first, Some(suffixes), partial),
            None if cmd == "help" => topic_candidates(first, partial),
            None if cmd == "theme" => theme_candidates(first, partial),
            None => Vec::new(),
        }
        .into();
    }

    // A bare proof-rule line, past its keyword — unlike a `:`-command,
    // these can run to many tokens (a `pol` expression especially), so
    // every token gets a turn, not just the first. Only once the keyword
    // itself is an exact, resolved name — no completing arguments for a
    // still-ambiguous or unrecognized one. Suppressed outright while a
    // narrower mode vocabulary is live — see this function's own docs.
    if mode_vocabulary.is_some() {
        return Suggestions::default();
    }
    let Some(session) = session else {
        return Suggestions::default();
    };
    if !help::RULES.iter().any(|t| t.name == first) {
        return Suggestions::default();
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
///
/// In string order. At most [`REFERENCE_CANDIDATE_CAP`] are built; the
/// rest are counted, without being listed, so this costs about the same
/// however large the database is.
fn reference_candidates(kept: &str, rule: &str, partial: &str, session: &Session) -> Suggestions {
    let cap = REFERENCE_CANDIDATE_CAP;
    let kind = rule_ref_kind(rule);
    let mut groups = Vec::new();

    if matches!(kind, RefKind::Constraint | RefKind::Both)
        && let Ok(database) = session.database()
    {
        groups.push(id_matches(
            &database.entries,
            |entry| entry.id as u64,
            partial,
            cap,
        ));
        groups.push(sorted_matches(&session.live_labels(), partial, cap));
    }

    if matches!(kind, RefKind::Variable | RefKind::Both) {
        let bare = session
            .variables
            .names()
            .filter(|name| name.starts_with(partial));
        let negated = partial
            .strip_prefix('~')
            .or(partial.is_empty().then_some(""))
            .into_iter()
            .flat_map(|rest| {
                session
                    .variables
                    .names()
                    .filter(move |name| name.starts_with(rest))
                    .map(|name| format!("~{name}"))
            });
        let mut literals: Vec<String> = bare.map(str::to_string).chain(negated).collect();
        literals.sort();
        groups.push(sorted_matches(&literals, partial, cap));
    }

    // Each group is in string order, so the first `cap` of all of them are
    // among the groups' first `cap`, and every match lies between the
    // first and the last.
    let mut lefts: Vec<String> = groups
        .iter()
        .flat_map(|g| g.first.iter().cloned())
        .collect();
    lefts.sort();
    lefts.truncate(cap);
    let total: usize = groups.iter().map(|g| g.count).sum();
    let common_line = match (
        lefts.first(),
        groups.iter().filter_map(|g| g.last.as_ref()).max(),
    ) {
        (Some(first), Some(last)) => format!("{kept}{}", common_prefix(first, last)),
        _ => String::new(),
    };
    Suggestions {
        omitted: total - lefts.len(),
        candidates: lefts
            .into_iter()
            .map(|left| Candidate {
                line: format!("{kept}{left}"),
                left,
                right: String::new(),
            })
            .collect(),
        common_line,
    }
}

/// Some matches of a prefix, in string order: the first few, how many
/// there are in all, and the last.
struct Matches {
    first: Vec<String>,
    count: usize,
    last: Option<String>,
}

/// The longest common prefix of `a` and `b`.
fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let len = a
        .char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((i, _), _)| i);
    &a[..len]
}

/// The strings in `sorted` starting with `prefix`: the first `cap`, the
/// count and the last.
fn sorted_matches(sorted: &[String], prefix: &str, cap: usize) -> Matches {
    let lo = sorted.partition_point(|s| s.as_str() < prefix);
    let count = sorted[lo..].partition_point(|s| s.starts_with(prefix));
    let matching = &sorted[lo..lo + count];
    Matches {
        first: matching.iter().take(cap).cloned().collect(),
        count,
        last: matching.last().cloned(),
    }
}

/// The IDs of `items` (by `id`, ascending) whose decimal form starts with
/// `prefix`, in string order (`1`, `10`, `100`, `11`, ...): the first
/// `cap`, the count and the last. Walks the decimal digits, counting each
/// branch by binary search, rather than every ID.
fn id_matches<T>(items: &[T], id: impl Fn(&T) -> u64, prefix: &str, cap: usize) -> Matches {
    let none = Matches {
        first: Vec::new(),
        count: 0,
        last: None,
    };
    let Some(max) = items.last().map(&id) else {
        return none;
    };
    let below = |n: u64| items.partition_point(|item| id(item) < n);
    // The IDs written `n` followed by `k` more digits, for every `k`.
    let count_from = |n: u64| -> usize {
        let mut count = 0;
        let (mut lo, mut width) = (n, 1u64);
        while lo <= max {
            let hi = lo.saturating_add(width);
            count += below(hi) - below(lo);
            let (Some(next_lo), Some(next_width)) = (lo.checked_mul(10), width.checked_mul(10))
            else {
                break;
            };
            (lo, width) = (next_lo, next_width);
        }
        count
    };
    let live = |n: u64| below(n) < items.len() && id(&items[below(n)]) == n;
    // The branches below `n` (or the first digits, for `None`) that hold an
    // ID, in string order.
    let children = |n: Option<u64>| -> Vec<u64> {
        let digits = if n.is_none() { 1..=9 } else { 0..=9 };
        digits
            .filter_map(|d| match n {
                None => Some(d),
                Some(n) => n.checked_mul(10)?.checked_add(d),
            })
            .filter(|&c| c <= max && count_from(c) > 0)
            .collect()
    };

    let start = if prefix.is_empty() {
        None
    } else if prefix.starts_with('0') {
        return none;
    } else {
        match prefix.parse::<u64>() {
            Ok(n) if prefix.bytes().all(|b| b.is_ascii_digit()) => Some(n),
            _ => return none,
        }
    };
    let count = start.map_or(items.len(), count_from);
    if count == 0 {
        return none;
    }

    let mut first = Vec::new();
    let mut stack: Vec<u64> = match start {
        Some(n) => vec![n],
        None => children(None).into_iter().rev().collect(),
    };
    while let Some(n) = stack.pop() {
        if first.len() == cap {
            break;
        }
        if live(n) {
            first.push(n.to_string());
        }
        stack.extend(children(Some(n)).into_iter().rev());
    }

    // The last in string order: the highest branch, all the way down.
    let mut last = match start {
        Some(n) => n,
        None => *children(None).last().expect("count > 0"),
    };
    while let Some(&child) = children(Some(last)).last() {
        last = child;
    }

    Matches {
        first,
        count,
        last: Some(last.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `id_matches` the slow way: every ID as a string, filtered and sorted.
    fn brute_force(ids: &[u64], prefix: &str, cap: usize) -> (Vec<String>, usize, Option<String>) {
        let mut all: Vec<String> = ids
            .iter()
            .map(u64::to_string)
            .filter(|s| s.starts_with(prefix))
            .collect();
        all.sort();
        let last = all.last().cloned();
        (all.iter().take(cap).cloned().collect(), all.len(), last)
    }

    #[test]
    fn id_matches_agree_with_sorting_every_id() {
        // Deterministic gaps, as deletions leave them.
        let mut seed = 7u64;
        let ids: Vec<u64> = (1..=25_000u64)
            .filter(|_| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                seed >> 61 != 0
            })
            .collect();
        for prefix in [
            "", "1", "2", "19", "100", "2499", "25000", "7", "0", "x", "1x", "99999",
        ] {
            for cap in [1, 6, 1000] {
                let matches = id_matches(&ids, |&id| id, prefix, cap);
                let (first, count, last) = brute_force(&ids, prefix, cap);
                assert_eq!(matches.first, first, "{prefix:?} cap {cap}");
                assert_eq!(matches.count, count, "{prefix:?}");
                assert_eq!(matches.last, last, "{prefix:?}");
            }
        }
        assert_eq!(id_matches(&[] as &[u64], |&id| id, "", 5).count, 0);
    }

    #[test]
    fn sorted_matches_take_the_prefix_range() {
        let sorted: Vec<String> = ["@a", "@ab", "@abc", "@b", "x1", "x10", "x2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let m = sorted_matches(&sorted, "@a", 2);
        assert_eq!(m.first, ["@a", "@ab"]);
        assert_eq!(m.count, 3);
        assert_eq!(m.last.as_deref(), Some("@abc"));
        assert_eq!(sorted_matches(&sorted, "y", 2).count, 0);
        assert_eq!(sorted_matches(&sorted, "", 10).count, sorted.len());
    }

    /// `reference_candidates` as it was before it bounded its work: every
    /// match built, then sorted.
    fn every_reference(kept: &str, rule: &str, partial: &str, session: &Session) -> Vec<Candidate> {
        let mut out = Vec::new();
        let kind = rule_ref_kind(rule);
        let candidate = |left: String| Candidate {
            line: format!("{kept}{left}"),
            left,
            right: String::new(),
        };
        if matches!(kind, RefKind::Constraint | RefKind::Both)
            && let Ok(database) = session.database()
        {
            let labels_by_id = session.labels_by_id();
            for entry in &database.entries {
                let id_str = entry.id.to_string();
                if id_str.starts_with(partial) {
                    out.push(candidate(id_str));
                }
                for name in labels_by_id.get(&(entry.id as isize)).into_iter().flatten() {
                    if name.starts_with(partial) {
                        out.push(candidate(name.clone()));
                    }
                }
            }
        }
        if matches!(kind, RefKind::Variable | RefKind::Both) {
            for name in session.variables.names() {
                for literal in [name.to_string(), format!("~{name}")] {
                    if literal.starts_with(partial) {
                        out.push(candidate(literal));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.left.cmp(&b.left));
        out
    }

    #[test]
    fn reference_candidates_agree_with_building_every_match() {
        if std::env::var_os("VERIPB_REPL_VERIPB_BIN").is_none() {
            eprintln!("skipping: VERIPB_REPL_VERIPB_BIN not set");
            return;
        }
        use std::io::Write as _;
        let n = 1500;
        let mut formula = String::new();
        for i in 1..n {
            let label = if i % 3 == 0 {
                format!("@c{i} ")
            } else {
                String::new()
            };
            formula.push_str(&format!("{label}1 ~x{i} 1 x{} >= 1 ;\n", i + 1));
        }
        let mut file = tempfile::NamedTempFile::with_suffix(".opb").unwrap();
        file.write_all(formula.as_bytes()).unwrap();
        let session = Session::load(file.path().to_str().unwrap()).unwrap();

        for (rule, partial) in [
            ("pol", ""),
            ("pol", "1"),
            ("pol", "14"),
            ("pol", "@c1"),
            ("pol", "~x1"),
            ("pol", "x14"),
            ("del", "@"),
            ("rup", "9999"),
            ("red", "x"),
        ] {
            let kept = format!("{rule} 1 ");
            let got = reference_candidates(&kept, rule, partial, &session);
            let all = every_reference(&kept, rule, partial, &session);
            let lefts = |c: &[Candidate]| c.iter().map(|c| c.left.clone()).collect::<Vec<_>>();
            assert_eq!(
                lefts(&got.candidates),
                lefts(&all[..all.len().min(REFERENCE_CANDIDATE_CAP)]),
                "{rule} {partial:?}"
            );
            assert_eq!(
                got.candidates.len() + got.omitted,
                all.len(),
                "{rule} {partial:?}"
            );
            assert_eq!(
                got.common_line_prefix(),
                common_line_prefix(&all),
                "{rule} {partial:?}"
            );
        }
    }

    #[test]
    fn common_prefix_stops_at_the_first_difference() {
        assert_eq!(common_prefix("1234", "1299"), "12");
        assert_eq!(common_prefix("12", "1234"), "12");
        assert_eq!(common_prefix("@é1", "@é2"), "@é");
        assert_eq!(common_prefix("a", "b"), "");
    }
}
