//! A rule: a verb, the line it was written as, where it came from, and what
//! the line names — read by [`line::parse`](super::line::parse), the one
//! reader of the grammar, and decided by [`decide`](super::decide).
//!
//! The written form (design §8.2; `:help rules`):
//!
//! ```text
//! <allow|ask|deny> <program> [subcommand …] [-flag | --flag=VALUE …] [*]
//! <allow|ask|deny> :<read|write|delete|exec|*> <path> …
//! <allow|ask|deny> :<fetch|send|web> <host> …
//! <allow|ask|deny> re:<regex>
//! ```
//!
//! Every older spelling (`find:-exec`, `-a,-b`, `-X|--method`, `:read
//! p/**`, `* /etc/**`) still reads as the rule it meant. A rule is READ by
//! the models: a flag resolves to its canonical name, so `deny rm -r` holds
//! for `rm -R` and `rm --recursive`; a program Shannon does not model keeps
//! the spelling, and the rule says so ([`Rule::as_written_note`]).

use super::line::{self, Body, Parsed, Tier};
use super::model::Models;

/// The three things a rule can say — and, once decided, the verdict itself.
///
/// The declaration order **is** the strictness order (`Allow` < `Ask` <
/// `Deny`): among the rules that decide an act — the most specific, by
/// inclusion — the strictest wins, whatever order they were written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Decision {
    /// Run without asking.
    Allow,
    /// Prompt the user first.
    Ask,
    /// Refuse, and tell the model why (never silently downgraded to a prompt —
    /// that would erase the distinction between "never" and "check with me").
    Deny,
}

impl Decision {
    /// The lowercase keyword, for round-tripping and messages.
    pub fn keyword(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Ask => "ask",
            Decision::Deny => "deny",
        }
    }
}

/// **Where a rule came from** — what `:rules` groups by, what a decision
/// pane names ("denied by rule: … (your config)", so you know whether to edit
/// a file or undo a session line), what removing it means, and the trust
/// tier it decides in ([`Tier::of`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// The shipped defaults (`defaults.rs`).
    Shipped,
    /// `[permissions] rules` in a config file.
    Config,
    /// `[permissions] rules` in the project's `.shannon/config.toml` — the
    /// user's own rules, as their config's are (Steven, 10-13), named apart
    /// so `:rules` and a pane say which file to edit.
    Project,
    /// Typed this session: `:allow` / `:deny` / `:ask`, or `A`.
    Session,
    /// Put in force by `[memory]` and `[skills]`: reads of their locations,
    /// writes to the memory write location under `mode = auto`
    /// (`knowledge::rules`).
    Knowledge,
}

impl Origin {
    /// How the user is told where a rule came from.
    pub fn describe(self) -> &'static str {
        match self {
            Origin::Shipped => "shipped",
            Origin::Config => "your config",
            Origin::Project => "the project's config",
            Origin::Session => "this session",
            Origin::Knowledge => "[memory] and [skills]",
        }
    }
}

/// A rule: a verb and what it names, with the text it came from kept for
/// display and in-app editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// What to do when it applies.
    pub action: Decision,
    /// The line the user wrote, verbatim — so "denied by rule `<this>`" can show
    /// the exact text and a one-key edit lands on it.
    pub source: String,
    /// Where it came from ([`Rule::read_by`] sets it).
    pub origin: Origin,
    /// What the line names, as read.
    parsed: Parsed,
}

/// A rule as a decision cites it: its verb, its text, where it came from —
/// enough to say "denied by rule `deny rm -r` (your config)".
///
/// **Carried, not indexed.** A decision outlives the policy it was made under
/// (an open prompt, a stored proposal), and an index into that policy's rules
/// named a DIFFERENT rule once one before it was removed — or none at all:
/// `:unask` (or `:unsource` of a file holding the ask) with a call open, then
/// `A`, indexed past the end and took Shannon down (Reviewers A, D, round 17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cited {
    pub action: Decision,
    pub source: String,
    pub origin: Origin,
    /// The shipped guard this rule is, where it is one — said by where the
    /// guard came from, not read back off its spelling (a `[secrets]` path
    /// and Shannon's own state are both `deny :* \c…`).
    pub guard: Option<Guard>,
    /// Met from above or by contact, not by name: the act's place holds
    /// what the rule names, or may (`ls .` with Shannon's state below).
    pub reached: bool,
}

/// What a shipped guard keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// A place `[secrets] paths` names: one that commonly holds credentials.
    Credential,
    /// Shannon's own state: its config and models, key, logs and store.
    OwnState,
}

impl Rule {
    /// Parse one rule line, read by the shipped models — for tests: code
    /// reads by [`parse_by`](Self::parse_by), and so must say which models
    /// (every door that read by the shipped set while the gate read by the
    /// user's was a bug of its own; review 2026-10-11, C).
    #[cfg(test)]
    pub fn parse(line: &str) -> Result<Rule, String> {
        Rule::parse_by(line, Models::shipped())
    }

    /// **Parse one rule line**, read by `models` — the shipped set with a
    /// user's directory over it ([`Models::with_user_dir`]): a rule on a
    /// program the user models resolves its flags and operands by that
    /// model, as the command will be read. Returns a human-facing error on
    /// malformed input — surfaced, never swallowed, so a typo in a policy is
    /// visible rather than silently inert.
    pub fn parse_by(line: &str, models: &Models) -> Result<Rule, String> {
        let line = line.trim();
        let parsed = line::parse(line, models)?;
        Ok(Rule {
            action: parsed.action,
            source: line.to_string(),
            // A line typed by hand; `read` sets the origin for every source.
            origin: Origin::Session,
            parsed,
        })
    }

    /// **This rule read again by `models`**, from its text, keeping where
    /// it came from: a rule read before the models in force were known (a
    /// session rule typed under other models, a shipped rule) reads as the
    /// command will.
    pub fn reread(&self, models: &Models) -> Result<Rule, String> {
        Rule::parse_by(&self.source, models).map(|rule| Rule {
            origin: self.origin,
            ..rule
        })
    }

    /// As [`read_by`](Self::read_by), by the shipped models — for tests.
    #[cfg(test)]
    pub fn read(origin: Origin, line: &str) -> Option<Result<Rule, String>> {
        Rule::read_by(origin, line, Models::shipped())
    }

    /// **THE one reader of a rule line**, for every source — shipped, config,
    /// session: strip a `#` comment, skip a blank line (`None`), parse the
    /// rest by `models`, and say where it came from.
    ///
    /// The comment stripper used to live in a reader only tests called
    /// (`extend_from_lines`), so the config and `:deny` read `#` literally:
    /// `:deny git push * # never` became a deny requiring the operands `#` and
    /// `never`, which never fired (Reviewer A, round 16). One reader, so no
    /// source can read a line differently (Reviewer D).
    pub fn read_by(origin: Origin, line: &str, models: &Models) -> Option<Result<Rule, String>> {
        let stripped = strip_comment(line);
        let line = stripped.trim();
        if line.is_empty() {
            return None;
        }
        Some(Rule::parse_by(line, models).map(|rule| Rule { origin, ..rule }))
    }

    /// Is `other` this rule — the same verb and the same question, however
    /// either was spelled (`ask find:-exec` is `ask find -exec`, `deny rm -R`
    /// is `deny rm --recursive`)? Where it came from does not matter: the one
    /// comparison for `remove`, for removing a rule this session, and for
    /// "already in force".
    pub fn same(&self, other: &Rule) -> bool {
        self.action == other.action
            && self.parsed.bodies.len() == other.parsed.bodies.len()
            && self
                .parsed
                .bodies
                .iter()
                .zip(&other.parsed.bodies)
                .all(|(a, b)| a.same(b))
    }

    /// This rule as a decision cites it.
    pub fn cited(&self) -> Cited {
        Cited {
            action: self.action,
            source: self.source.clone(),
            origin: self.origin,
            guard: None,
            reached: false,
        }
    }

    /// **This rule on one line**, for a place that has one — a notice, the
    /// decision pane's reason. A rule a config writes across lines reads
    /// with each break, and the indent around it, as one space.
    pub fn one_line(&self) -> std::borrow::Cow<'_, str> {
        one_line(&self.source)
    }

    /// What the line names: one body per target (a program rule with path
    /// words also grants; `:read a b` names two; `-name|-iname` is two).
    pub fn bodies(&self) -> &[Body] {
        &self.parsed.bodies
    }

    /// The trust tier this rule decides in.
    pub fn tier(&self) -> Tier {
        Tier::of(self.origin)
    }

    /// The program this rule is about, when it is a program rule.
    pub fn program(&self) -> Option<&str> {
        self.parsed.program()
    }

    /// What to say where this rule is written, when it names a flag or a
    /// word only as written — the program is not modelled, or its model does
    /// not know the spelling — so another spelling of the same option is not
    /// caught: `deny gh api -X` stops `-X DELETE` and `--method DELETE` once
    /// gh is modelled, and only `-X DELETE` for a program that is not. The
    /// rule fires; it is narrower than it reads. `models`: those it was read
    /// by ([`RuleSet::models`](super::RuleSet::models)).
    pub fn as_written_note(&self, models: &Models) -> Option<String> {
        self.parsed.as_written_note(models)
    }
}

impl Cited {
    /// The cited rule on one line, as [`Rule::one_line`] puts it.
    pub fn one_line(&self) -> std::borrow::Cow<'_, str> {
        one_line(&self.source)
    }
}

/// **A rule's text on one line.** A config may write a rule across lines (a
/// TOML `'''` string, so a long regex reads with `(?x)` and comments); a
/// notice or a reason has one line, where each break and the indent around
/// it reads as one space. `:rules` shows the rule as written.
pub(crate) fn one_line(source: &str) -> std::borrow::Cow<'_, str> {
    if !source.contains('\n') {
        return source.into();
    }
    source
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .into()
}

/// **Strip a `#` comment**, per line — one that begins the line or follows
/// whitespace, so `deny gh api -H # no headers` is `deny gh api -H`. A rule
/// written across lines keeps its breaks, each line stripped on its own,
/// and may open with comment lines.
///
/// **A regex rule has no comment: its `#` is the pattern's.** Stripped,
/// `deny re:^echo #x` would be stored as `deny re:^echo` — the very "parses
/// into a different rule, silently" this function exists to stop. (Under
/// `(?x)` the pattern's own `#` comments end at their lines too.) Read from
/// the rule's first line past blank and comment lines: a rule laid out
/// across lines may open with a comment, and that `#` was taken for the
/// verb, so the regex was not seen and its `#x` cut (review 20, Reviewer A).
pub(super) fn strip_comment(line: &str) -> std::borrow::Cow<'_, str> {
    let mut from = 0;
    for piece in line.split_inclusive('\n') {
        let text = piece.trim();
        if !text.is_empty() && !text.starts_with('#') {
            break;
        }
        from += piece.len();
    }
    let rule = &line[from..];
    let mut words = rule.split_whitespace();
    if matches!(words.next(), Some("allow" | "ask" | "deny"))
        && words.next().is_some_and(|w| w.starts_with("re:"))
    {
        return rule.into();
    }
    if !rule.contains('#') {
        return rule.into();
    }
    if !rule.contains('\n') {
        return strip_line_comment(rule).into();
    }
    rule.lines()
        .map(strip_line_comment)
        .collect::<Vec<_>>()
        .join("\n")
        .into()
}

fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            return &line[..i];
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **One reader for every source**: a comment is stripped, a blank line
    /// is nothing, and the origin is the caller's.
    #[test]
    fn read_strips_comments_and_keeps_the_origin() {
        let r = Rule::read(Origin::Config, "deny gh api -H # no headers")
            .unwrap()
            .unwrap();
        assert_eq!(r.source, "deny gh api -H");
        assert_eq!(r.origin, Origin::Config);
        assert_eq!(r.action, Decision::Deny);
        assert!(Rule::read(Origin::Config, "   # only a comment").is_none());
        assert!(Rule::read(Origin::Config, "").is_none());
        let r = Rule::read(Origin::Session, r"deny re:^foo#bar")
            .unwrap()
            .unwrap();
        assert_eq!(r.source, r"deny re:^foo#bar", "a `#` inside a word is kept");
        // **A regex's `#` after a blank is the pattern's** — the one row the
        // regex rule changes: `foo#bar` has no blank before its `#`, so it
        // kept its `#` with the regex guard off, and the guard was held in
        // shannon-tui alone (review stop 12, T8 M12).
        let r = Rule::read(Origin::Config, r"deny re:^echo #x")
            .unwrap()
            .unwrap();
        assert_eq!(r.source, r"deny re:^echo #x", "the pattern, whole");
        assert!(Rule::read(Origin::Config, "allwo rm *").unwrap().is_err());
    }

    /// **Same, however spelled**: the comparison `remove` and `:unallow` use.
    #[test]
    fn same_is_the_question_not_the_spelling() {
        let p = |s: &str| Rule::parse(s).unwrap();
        assert!(p("ask find:-exec").same(&p("ask find -exec")));
        assert!(p("deny rm -R").same(&p("deny rm --recursive")));
        assert!(p("allow :read docs/").same(&p("allow :read docs")));
        assert!(
            !p("allow :read docs/**").same(&p("allow :read docs")),
            "`docs/**` is what is below docs; `docs` is the directory too"
        );
        assert!(
            !p("allow find -name").same(&p("deny find -name")),
            "the verb counts"
        );
        assert!(!p("allow rm *").same(&p("allow rm")), "`*` counts");
        let mut shipped = p("deny rm --no-preserve-root");
        shipped.origin = Origin::Shipped;
        assert!(
            shipped.same(&p("deny rm --no-preserve-root")),
            "the origin does not"
        );
    }

    /// A rule written across lines reads on one.
    #[test]
    fn a_rule_across_lines_reads_on_one() {
        let r = Rule::parse("deny re:(?x)\n    ^rm\n    \\s").unwrap();
        assert_eq!(r.one_line(), "deny re:(?x) ^rm \\s");
    }
}
