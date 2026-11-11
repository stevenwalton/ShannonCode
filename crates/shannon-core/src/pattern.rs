//! **`re:` — a regex, wherever a rule or a list takes a pattern** (Steven,
//! 2026-10-03: regex for the advanced user, "share primitives where you
//! can").
//!
//! Two shapes, one engine:
//!
//! - [`RePattern`], the pattern itself: what a whole-rule `re:` matches a
//!   command's words against (`permission::rule`).
//! - [`PathRe`], a pattern for a PATH: a rule's operand (`allow rm
//!   re:^build/.*\.o$`) and a `[secrets] paths` entry. It is matched only
//!   through [`PathRe::matches`], which reads a path the way the glob
//!   matchers do — so a path regex cannot be handed raw text by mistake.
//!
//! The engine is Rust's `regex` crate: no look-around, no backreferences,
//! and time linear in the input, so a pattern cannot hang the gate. Its own
//! engine, `regex-automata`, answers the one question `regex` cannot: could
//! a command's glob expand to a path a pattern matches
//! ([`PathRe::could_name`]).

/// A compiled `re:` pattern. Compared by its SOURCE, so one pattern written
/// twice is one rule (`Rule::same`).
#[derive(Debug, Clone)]
pub struct RePattern {
    regex: regex::Regex,
    /// Built case-insensitive — a `[secrets] paths` entry, as every entry
    /// there is. Kept so a DFA of the same pattern is built the same way.
    fold: bool,
    /// The pattern as a DFA, built the first time a word that still expands
    /// asks ([`could_find`](Self::could_find)) and shared by every clone —
    /// the `listed` memo and a rule set's clones hand out clones, so it is
    /// built once per pattern. A build is ~3 ms in debug, the walk a fraction
    /// of that. `None` inside: too large to build, so undecided.
    dfa: std::sync::Arc<std::sync::OnceLock<Option<Dfa>>>,
}

impl PartialEq for RePattern {
    fn eq(&self, other: &Self) -> bool {
        self.regex.as_str() == other.regex.as_str()
    }
}

impl Eq for RePattern {}

impl RePattern {
    /// Compile a pattern, or say why it cannot be one.
    ///
    /// The crate's error draws a caret under the pattern across several
    /// lines; the notice row is one, so its last line — the reason — is what
    /// is said, and a pattern written across lines is quoted on one.
    pub(crate) fn compile(pattern: &str) -> Result<Self, String> {
        Self::build(pattern, false)
    }

    /// [`compile`](Self::compile), case-insensitive when `fold` — the
    /// pattern's own `(?-i)` turns that back off.
    ///
    /// Bounded at a megabyte compiled, a tenth of the crate's default: a
    /// rule is a line of config, and at the default 60 sourced rules of
    /// counted Unicode classes held 762 MB (review 20, Reviewer B). Past it
    /// the pattern is refused, saying what usually makes one that large —
    /// `\w{50}` is past it, and the ASCII class it nearly always means is
    /// not.
    fn build(pattern: &str, fold: bool) -> Result<Self, String> {
        if pattern.is_empty() {
            return Err("`re:` needs a pattern — `re:^rm(\\s|$)`, for instance".into());
        }
        regex::RegexBuilder::new(pattern)
            .case_insensitive(fold)
            .size_limit(1 << 20)
            .build()
            .map(|regex| Self {
                regex,
                fold,
                dfa: Default::default(),
            })
            .map_err(|e| {
                if let regex::Error::CompiledTooBig(_) = e {
                    return format!(
                        "`re:{}` is too large to keep — a repeated Unicode class (`\\w{{50}}`) \
                         usually is; the ASCII one is small: `[A-Za-z0-9_]{{50}}` or `(?-u:\\w){{50}}`",
                        crate::permission::one_line(pattern)
                    );
                }
                let shown = e.to_string();
                let reason = shown
                    .lines()
                    .rfind(|l| !l.trim().is_empty())
                    .unwrap_or(&shown)
                    .trim()
                    .trim_start_matches("error: ")
                    .to_string();
                format!(
                    "`re:{}` is not a regex Shannon can read: {reason}",
                    crate::permission::one_line(pattern)
                )
            })
    }

    /// Where in `text` the pattern matches, if it does.
    pub(crate) fn find<'t>(&self, text: &'t str) -> Option<&'t str> {
        self.regex.find(text).map(|m| m.as_str())
    }

    /// **Can a word that still expands be checked against this pattern at
    /// all?** Not when its automaton is too large to build — then every such
    /// question is undecided, for the life of the process. Asked once at
    /// config load so that is said there, naming the pattern, rather than
    /// discovered as every glob asking (review 20, Reviewer F).
    pub(crate) fn checks_expansions(&self) -> bool {
        self.automaton().is_some()
    }

    /// The pattern's DFA, built once and shared by every clone.
    fn automaton(&self) -> Option<&Dfa> {
        self.dfa
            .get_or_init(|| dfa(self.regex.as_str(), self.fold, false))
            .as_ref()
    }

    /// The pattern as written.
    pub(crate) fn as_str(&self) -> &str {
        self.regex.as_str()
    }
}

/// A dense DFA, as `regex-automata` builds one.
type Dfa = regex_automata::dfa::dense::DFA<Vec<u32>>;

/// How many `[secrets] paths` patterns [`PathRe::listed`] keeps compiled
/// before it starts over — far past any list a person writes, well short of
/// what a session of edits could pile up.
const MOST_LISTED: usize = 256;

/// [`PathRe::listed`]'s memo, held for the life of the process.
fn listed_memo()
-> std::sync::MutexGuard<'static, std::collections::HashMap<String, Result<PathRe, String>>> {
    static COMPILED: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Result<PathRe, String>>>,
    > = std::sync::OnceLock::new();
    COMPILED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `pattern` as a DFA, bounded: past a megabyte it is not built, and the
/// caller takes that as undecided. `bytes`: over bytes, not Unicode — for a
/// glob, whose `[^/]` it shrinks to one class (`meets`).
fn dfa(pattern: &str, fold: bool, bytes: bool) -> Option<Dfa> {
    use regex_automata::dfa::dense;
    const LIMIT: usize = 1 << 20;
    dense::Builder::new()
        .configure(
            dense::Config::new()
                .dfa_size_limit(Some(LIMIT))
                .determinize_size_limit(Some(LIMIT))
                // A Unicode `\b` refuses to build at all without this; with
                // it, the DFA quits past ASCII instead (`meets`).
                .unicode_word_boundary(true),
        )
        .syntax(
            regex_automata::util::syntax::Config::new()
                .case_insensitive(fold)
                .unicode(!bytes)
                .utf8(!bytes),
        )
        .build(pattern)
        .ok()
}

/// **A path as a pattern reads it, in every spelling that names it**: `.`,
/// `..`, `//` and a trailing `/` resolved, home expanded — and, under home,
/// its `~` spelling too, since `~/x` and `/Users/you/x` name one file and a
/// pattern may name either; through a root link likewise (`/tmp/x` and
/// `/private/tmp/x`). (A path, not a token: which paths a word may carry is
/// the acts reader's, `permission::acts`.)
fn spellings(path: &str) -> impl Iterator<Item = String> {
    let full = crate::shell::pipeline::normalize_path(&crate::permission::expand_home(path));
    let tilde = crate::secrets::home_dir().and_then(|home| {
        let rest = full.strip_prefix(home)?;
        (rest.is_empty() || rest.starts_with('/')).then(|| format!("~{rest}"))
    });
    let links: Vec<String> = crate::shell::pipeline::link_spellings(&full).collect();
    std::iter::once(full).chain(tilde).chain(links)
}

/// **A `re:` pattern for a path.** Matched against the path as Shannon
/// reads one ([`matches`](Self::matches)), never as raw text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRe {
    pattern: RePattern,
}

impl PathRe {
    fn new(pattern: RePattern) -> Self {
        Self { pattern }
    }

    /// A path pattern written `re:<pattern>`: `None` when `token` is not
    /// one, else the compiled pattern or why it cannot be one.
    pub(crate) fn parse(token: &str) -> Option<Result<PathRe, String>> {
        token
            .strip_prefix("re:")
            .map(|pattern| RePattern::compile(pattern).map(PathRe::new))
    }

    /// **A `[secrets] paths` entry as a regex**: one written `re:<pattern>`,
    /// as [`parse`](Self::parse) reads it, or a glob as the regex bash's
    /// reading of it is (`secrets::glob_entry_regex`) — `None` for an entry
    /// that list reads itself. Either way case-INsensitive, as that list's
    /// every entry is (`secrets::paths`: APFS and NTFS fold case, and a
    /// missed spelling costs the layer; `(?-i)` opts out of a `re:`), and
    /// compiled ONCE for the life of the process.
    ///
    /// The one door for the list's patterns: a path's match, a command
    /// glob's (`could_name`) and the load's notes all come through here, so
    /// a glob entry is checked by the engine a regex one is.
    ///
    /// The list is read on every decision, as text, and a compile costs
    /// about a hundred matches (measured 2026-10-03: ~0.1 ms against ~3 µs,
    /// release). Keyed by the entry, so an edited list compiles only what
    /// is new. The entries are the user's config, but every edit adds a key
    /// and none removes one, so the memo is emptied at [`MOST_LISTED`]
    /// rather than trusted to stay small (review 20, Reviewer B).
    pub(crate) fn listed(token: &str) -> Option<Result<PathRe, String>> {
        let (source, glob) = match token.strip_prefix("re:") {
            Some(pattern) => (pattern.to_string(), false),
            None => (crate::secrets::glob_entry_regex(token)?, true),
        };
        let mut compiled = listed_memo();
        if compiled.len() >= MOST_LISTED {
            compiled.clear();
        }
        Some(
            compiled
                .entry(token.to_string())
                .or_insert_with(|| {
                    RePattern::build(&source, true)
                        .map(PathRe::new)
                        .map_err(|why| match glob {
                            // Its regex is ours, not the user's: name the entry.
                            true => format!("`{token}` is too large a pattern to keep"),
                            false => why,
                        })
                })
                .clone(),
        )
    }

    /// **Does the pattern match `path`, read as a path?** What a glob reads
    /// too (`permission::glob::matches`): `.`, `..`, `//` and a trailing `/`
    /// resolved, so `build/../../etc/passwd` is `../etc/passwd` and an
    /// allow's `^build/` cannot be walked out of; and every spelling of one
    /// file — `~/x` and `/Users/you/x`, `/tmp/x` and `/private/tmp/x` — as
    /// a pattern may name it any way ([`spellings`]). A relative path stays
    /// relative (to where Shannon started); the caller has already applied
    /// the line's `cd`.
    pub(crate) fn matches(&self, path: &str) -> bool {
        spellings(path).any(|read| self.pattern.regex.is_match(&read))
    }

    /// Can a glob be checked against this pattern at all
    /// ([`RePattern::checks_expansions`])?
    pub(crate) fn checks_globs(&self) -> bool {
        self.pattern.checks_expansions()
    }

    /// The pattern as written, without its `re:` — what a rule naming it
    /// prints back (`permission::line::PathPat::show`).
    pub(crate) fn as_str(&self) -> &str {
        self.pattern.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(pattern: &str) -> PathRe {
        PathRe::listed(pattern).expect("re:").expect("compiles")
    }

    /// **A pattern is bounded at a megabyte compiled** — and the refusal
    /// says how to write the usual culprit smaller. `\w{50}` compiles under
    /// the crate's default and not under ours (measured 2026-10-03: `\w{20}`
    /// is the last under it), so this holds the bound itself.
    #[test]
    fn a_pattern_is_bounded_and_says_how_to_shrink() {
        assert!(RePattern::compile(r"\w{20}").is_ok(), "premise");
        assert!(regex::Regex::new(r"\w{50}").is_ok(), "premise: the default");
        let err = RePattern::compile(r"\w{50}").expect_err("too large");
        assert!(err.contains("[A-Za-z0-9_]"), "{err}");
        assert!(RePattern::compile(r"[A-Za-z0-9_]{50}").is_ok());
        assert!(RePattern::compile(r"(?-u:\w){50}").is_ok());
    }

    /// **The memo starts over rather than grows without end** — and what
    /// it hands out after starting over still matches.
    #[test]
    fn the_listed_memo_is_bounded() {
        for i in 0..MOST_LISTED + 8 {
            listed(&format!("re:^/memo-bound-{i}$"));
        }
        assert!(listed_memo().len() <= MOST_LISTED);
        assert!(listed("re:^/memo-bound-0$").matches("/memo-bound-0"));
    }
}
