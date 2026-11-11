//! **The rule line**: one line, one target, one verdict — written to look
//! like the thing it names (design §8.2).
//!
//! ```text
//! <allow|ask|deny> <program> [subcommand …] [-flag | --flag= | --flag=VALUE …] [*]
//! <allow|ask|deny> :<read|write|delete|exec|*> <path-or-glob> …
//! <allow|ask|deny> :<fetch|send|web> <host-or-glob> …
//! <allow|ask|deny> re:<regex>
//! ```
//!
//! A bare word after the program is a POSITION in the subcommand chain
//! (`gh pr view`), never a selector found anywhere. A program rule is
//! about flags and arguments, a path rule about access (Steven, 10-10): a
//! path word on an allow is an ARGUMENT the command must carry, as written
//! (`allow rm build/a.o`), never a grant — `allow :delete build` grants
//! the place; on a deny or an ask it FENCES the place off from the program
//! where the rule's form is met (`deny rm /etc/**`). `*` on an allow widens
//! it to the nodes nothing else covers, and means nothing on a deny; a
//! directory holds what is below it, so `/mnt` and `/mnt/**` are one rule
//! and a glob is for the name, not the depth.
//!
//! **`--flag=` declares a value.** A model says which flags take one; for
//! a program Shannon does not model, `allow bat -l=` says `-l` does, so in
//! `bat -l rust` the word `rust` is the flag's and not an argument the rule
//! left out. `--flag=VALUE` names one value (a glob). A declared value is
//! still a place to the path rules wherever it could be one, so a wrong
//! declaration costs a prompt and never hides a target; and where a model
//! says a flag takes no value, a value on it is refused.
//!
//! **Every older spelling still reads.** `find:-exec` is `find -exec`;
//! `-a,-b` is both; `-X|--method` is the spellings of one flag (two
//! different flags make two rules); `:read p/**` is what is below `p`
//! (`:read p` the directory too); `* /etc/**` is `:* /etc/**`; `re:` is
//! itself. A class rule (`allow :readonly`, `deny
//! :destructive`) is refused with the reason: the defaults already allow a
//! modelled read, and a class names no node a more specific rule could be
//! compared with. A spelling nothing here reads is refused, never read as
//! a different rule — the discipline the old parser earned the hard way
//! (`deny sudo` inert, `deny find: -delete` never firing, `[` read as
//! itself).
//!
//! Paths stay AS WRITTEN: a relative path or a `~` is resolved against
//! where Shannon started and the home when the rule is matched
//! ([`PathPat::resolve`]), since a rule is parsed before the start is
//! known and a config may be shared across machines.

use super::acts::{Dir, absolute};
use super::model::{Arity, Effect, Model, Models, Node, Role};
use super::{Decision, Origin};
use crate::pattern::{PathRe, RePattern};

/// The trust tier a rule comes from, lowest first: specificity decides
/// within a tier, trust across tiers (§7.2, §9.3 — trust-ordered, a lower
/// tier only narrows).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum Tier {
    Shipped,
    /// The user's own: their config, the project's (Steven, 10-13: a
    /// project file's rules are the user's own, one tier with the global
    /// file's), and what `[memory]`/`[skills]` put in force.
    User,
    /// `:allow`, `:deny`, `:ask`, `A` — this session.
    Session,
}

impl Tier {
    /// The tier a rule's origin puts it in.
    pub fn of(origin: Origin) -> Tier {
        match origin {
            Origin::Shipped => Tier::Shipped,
            Origin::Config | Origin::Project | Origin::Knowledge => Tier::User,
            Origin::Session => Tier::Session,
        }
    }
}

impl Tier {
    /// The trust group a tier decides in: the user's own tiers are one
    /// group (a `:allow` typed now and a rule in either config file are
    /// the same person's, so specificity orders them, not trust); the
    /// shipped rules below them. A lower group only narrows (`decide`).
    pub fn group(self) -> u8 {
        match self {
            Tier::Shipped => 0,
            Tier::User | Tier::Session => 2,
        }
    }
}

/// The operation a path rule is at.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Op {
    Read,
    Write,
    Delete,
    Exec,
    Any,
}

impl Op {
    /// Does this rule's operation cover an act at `act`? Operations are a
    /// ladder, read < write < delete < exec. A grant covers its own rung
    /// and the reads below it (`allow :write build` reads and writes
    /// there; `:delete` writes too; `:exec` runs, and reads) — never a rung
    /// above. A deny or an ask (`contact`) fires at its rung and every one
    /// above (`deny :write /etc`: no write, delete or code there). An
    /// unmodelled program's word is at no operation: only `:*` grants it,
    /// and every fence meets it — it may be any of them.
    pub fn covers(self, act: Option<Effect>, contact: bool) -> bool {
        let Some(act) = act else {
            return self == Op::Any || contact;
        };
        let rung = |op: Op| match op {
            Op::Read => Some(Effect::Read),
            Op::Write => Some(Effect::Write),
            Op::Delete => Some(Effect::Delete),
            Op::Exec => Some(Effect::Exec),
            Op::Any => None,
        };
        let Some(own) = rung(self) else {
            return true;
        };
        if contact {
            act >= own
        } else {
            act == own || act == Effect::Read || (own == Effect::Delete && act == Effect::Write)
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Op::Read => "read",
            Op::Write => "write",
            Op::Delete => "delete",
            Op::Exec => "exec",
            Op::Any => "*",
        }
    }

    fn parse(name: &str) -> Option<Op> {
        Some(match name {
            "read" => Op::Read,
            "write" => Op::Write,
            "delete" => Op::Delete,
            "exec" => Op::Exec,
            "*" => Op::Any,
            _ => return None,
        })
    }
}

/// A path pattern, as written: a path whose segments may each be a glob
/// (`*`, `?` within one name), anchored at a root (`/etc`, `build/*`,
/// `~/notes`, `~/.claude/projects/*/memory`) or found anywhere (`**/.env*`,
/// `**/a/b`); or a regex over the absolute path (`re:^/etc/pass[w]d$`). A
/// directory holds what is below it, so a pattern names every path whose
/// segments begin with (anchored) or contain (anywhere) a run its own
/// segments match, one each: `build/*` names `build/sub/a.o` through the
/// entry `sub`, and not `build` itself. A trailing `/**` names what is
/// BELOW the directory and not the directory: `allow rm build/**` does
/// not cover `rm -rf build`, `allow rm build` does (Steven, 10-10: flags
/// and paths apart, and `./**` must never be the project itself).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PathPat {
    Path {
        path: String,
        anywhere: bool,
        fold: bool,
        /// Written with a trailing `/**`: below the path, not the path.
        below: bool,
    },
    Re(PathRe),
}

/// A path pattern resolved against a start and a home: its segments.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Resolved {
    Segs {
        segs: Vec<String>,
        anywhere: bool,
        fold: bool,
        /// Below the segments, not the segments themselves (`/**`).
        below: bool,
    },
    /// A regex, matched against the absolute path and — below the start —
    /// against the path as written from it (`re:^build/.*\.o$`).
    Re { re: PathRe, start: String },
}

/// `*` and `?` over one name, no `/`.
pub(crate) fn glob_match(pat: &str, name: &str, fold: bool) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = if fold {
        (
            pat.to_lowercase().chars().collect(),
            name.to_lowercase().chars().collect(),
        )
    } else {
        (pat.chars().collect(), name.chars().collect())
    };
    // The two-pointer match with one backtrack point per `*`: linear
    // space and no recursion, so a 256k-character word from the model
    // cannot overflow the stack (review 2026-10-10, B).
    let (mut i, mut j) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while j < n.len() {
        match p.get(i) {
            Some('*') => {
                star = Some((i, j));
                i += 1;
            }
            Some('?') => {
                i += 1;
                j += 1;
            }
            Some(c) if *c == n[j] => {
                i += 1;
                j += 1;
            }
            _ => match star {
                Some((si, sj)) => {
                    i = si + 1;
                    j = sj + 1;
                    star = Some((si, sj + 1));
                }
                None => return false,
            },
        }
    }
    p[i..].iter().all(|c| *c == '*')
}

/// The segments of a path, empty ones dropped (`/` has none).
fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// Is `path` the tree `tree` or below it? (Both absolute, normalised.)
pub fn in_tree(path: &str, tree: &str) -> bool {
    path == tree || (tree == "/" && path.starts_with('/')) || path.starts_with(&format!("{tree}/"))
}

/// `a ⊑ b` over one-name globs: equal, `b` is `*`, or a literal `a` that
/// `b` matches. Sound and incomplete (`ab*` ⊑ `a*` is not seen): an
/// unseen inclusion makes two rules incomparable, and the strictest
/// decides — never the wrong one.
fn name_within(a: &str, b: &str, fold: bool) -> bool {
    a == b || b == "*" || (!a.contains(['*', '?']) && glob_match(b, a, fold))
}

/// Do the pattern segments `pat` match the path segments `segs` from `at`?
/// For contact, a path segment that is itself a glob (`.en*`, placed as
/// written) meets the pattern when the two could name one entry.
fn run_matches(pat: &[String], segs: &[&str], at: usize, fold: bool, contact: bool) -> bool {
    at + pat.len() <= segs.len()
        && pat.iter().enumerate().all(|(i, g)| {
            let seg = segs[at + i];
            glob_match(g, seg, fold) || (contact && globs_meet(g, seg, fold))
        })
}

/// **`path` as a pattern must read it too, where a root segment is a
/// glob.** The gate's one spelling takes a root link to its target on
/// both sides (`shell::pipeline::normalize_path`), but a glob names no link
/// it could take: `/*/secret` no longer met `/tmp/secret` once that was
/// `/private/tmp/secret`, and `cat /et?/x` left `deny cat /etc/*`, Ask
/// becoming Allow (found by `a_glob_meets_a_fence_softly`). So a pattern's
/// glob is shown the path as each link spells it, and a path's glob, by
/// contact only as any glob is (no grant covers it), each link it could
/// pick. Empty unless a root segment is a glob, which is rare.
fn across_root_links(pat: &[String], path: &str, contact: bool) -> Vec<String> {
    use crate::shell::pipeline::{link_spellings, root_links};
    let wild = |seg: &str| seg.contains(['*', '?', '[']);
    let mut read = Vec::new();
    if pat.first().is_some_and(|seg| wild(seg)) {
        read.extend(link_spellings(path));
    }
    if let (true, Some(rest)) = (contact, path.strip_prefix('/')) {
        let (first, below) = rest.split_once('/').unwrap_or((rest, ""));
        if wild(first) {
            read.extend(
                (root_links().iter())
                    .filter(|(name, _)| globs_meet(name, first, cfg!(target_os = "macos")))
                    .map(|(_, to)| format!("{to}/{below}")),
            );
        }
    }
    read
}

/// Could the pattern `pat` and the shell glob `glob` match one name? `*`
/// and `?` on either side stand for any text or character; the shell's
/// `*` never matches a leading `.`, so `*.rs` meets no `.env*`. A `[…]`
/// class is read as `?` — a sound widening for contact. Sound and
/// incomplete the other way only for a literal (where `glob_match` is
/// exact), so a deny meets every glob that could reach it.
pub(crate) fn globs_meet(pat: &str, glob: &str, fold: bool) -> bool {
    if !glob.contains(['*', '?', '[']) {
        return glob_match(pat, glob, fold);
    }
    // A class as bash reads one: a `]` right after `[`, `[!` or `[^` is a
    // member, not the close (`[]e]` is `]` or `e`); the whole class is one
    // character; an unclosed `[` is literal (review 2026-10-10, A).
    let shape = |s: &str| -> Vec<char> {
        let chars: Vec<char> = s.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '[' {
                let mut k = i + 1;
                if matches!(chars.get(k), Some('!' | '^')) {
                    k += 1;
                }
                if chars.get(k) == Some(&']') {
                    k += 1;
                }
                // To the closing `]`, stepping over a `[:alpha:]` class name.
                let mut close = None;
                let mut j = k;
                while j < chars.len() {
                    if chars[j] == '[' && chars.get(j + 1) == Some(&':') {
                        match chars[j + 2..].windows(2).position(|w| w == [':', ']']) {
                            Some(p) => j += 2 + p + 2,
                            None => break,
                        }
                        continue;
                    }
                    if chars[j] == ']' {
                        close = Some(j);
                        break;
                    }
                    j += 1;
                }
                if let Some(close) = close {
                    out.push('?');
                    i = close + 1;
                    continue;
                }
            }
            if fold {
                out.extend(chars[i].to_lowercase());
            } else {
                out.push(chars[i]);
            }
            i += 1;
        }
        out
    };
    let (a, b) = (shape(pat), shape(glob));
    // The shell's `*` and `?` never match a leading `.`: a glob opening with
    // one meets no pattern that names a dotfile. (A pattern's `*` DOES match
    // a leading dot through `glob_match`, so the mirror check was unsound:
    // `.x*` picks `.x.pem` — review 2026-10-10, A and C.)
    if a.first() == Some(&'.') && matches!(b.first(), Some('*' | '?')) {
        return false;
    }
    // Bottom-up: can a[i..] and b[j..] match one string? A table of
    // (|a|+1)·(|b|+1) cells, no recursion (review 2026-10-10, B).
    let (n, m) = (a.len(), b.len());
    let mut meet = vec![vec![false; m + 1]; n + 1];
    meet[n][m] = true;
    for i in (0..=n).rev() {
        for j in (0..=m).rev() {
            if i == n && j == m {
                continue;
            }
            meet[i][j] = match (a.get(i), b.get(j)) {
                (Some('*'), _) => meet[i + 1][j] || (j < m && meet[i][j + 1]),
                (_, Some('*')) => meet[i][j + 1] || (i < n && meet[i + 1][j]),
                (Some(x), Some(y)) if *x == '?' || *y == '?' || x == y => meet[i + 1][j + 1],
                _ => false,
            };
        }
    }
    meet[0][0]
}

/// Are the pattern segments `a` from `at` each within the corresponding one
/// of `b`?
fn run_within(a: &[String], at: usize, b: &[String], fold: bool) -> bool {
    at + b.len() <= a.len()
        && b.iter()
            .enumerate()
            .all(|(i, g)| name_within(&a[at + i], g, fold))
}

impl PathPat {
    /// Read one path word: a `re:` pattern; `**/…` for a run of segments
    /// found anywhere; a trailing `/**` or `/` meaning the directory itself;
    /// else a path from its root, each segment a name or a glob of one. A
    /// leading `\c` folds case (the secrets list's spelling). `[` and `{`
    /// are refused: neither reads as the shell reads it here, and a rule
    /// that cannot meet what it seems to name is worse than none.
    pub fn parse(word: &str) -> Result<PathPat, String> {
        if let Some(re) = PathRe::parse(word) {
            return re.map(PathPat::Re);
        }
        // `\c` folds case; `\C` asks for case to matter (the default, said).
        let (word, fold) = match (word.strip_prefix("\\c"), word.strip_prefix("\\C")) {
            (Some(rest), _) => (rest, true),
            (None, Some(rest)) => (rest, false),
            (None, None) => (word, false),
        };
        if word.is_empty() {
            return Err("a path rule names a path".into());
        }
        if word.contains('[') {
            return Err(format!(
                "`{word}`: a rule's path reads `[` as itself, not as a class, so it would not \
                 meet the files it seems to name — for a class write `re:` (`re:^/etc/pass[w]d$`)"
            ));
        }
        if word.contains(['{', '}']) {
            return Err(format!(
                "`{word}`: a rule's path has no `{{a,b}}` — write one rule per name"
            ));
        }
        let (w, anywhere) = match word.strip_prefix("**/") {
            Some(rest) => (rest, true),
            None => (word, false),
        };
        // `DIR/**` is what is below DIR; `**` alone and `/**` are below the
        // root — everywhere but the root itself.
        let (w, below) = match w.strip_suffix("/**") {
            Some(rest) => (rest, true),
            None if w == "**" => ("", true),
            None => (w, false),
        };
        let w = w.trim_end_matches('/');
        if w.contains("**") {
            return Err(format!(
                "`{word}`: `**` only begins a pattern (`**/NAME`, found anywhere) or ends one \
                 (`DIR/**`, what is below DIR; `DIR` alone is the directory and all below it)"
            ));
        }
        if anywhere {
            if w.is_empty() {
                return Err(format!("`{word}`: `**/` takes a name (`**/.env*`)"));
            }
            if segments(w).iter().any(|s| *s == "." || *s == "..") {
                return Err(format!(
                    "`{word}`: a name found anywhere is written without `.` or `..`"
                ));
            }
        }
        let path = if w.is_empty() { "/" } else { w };
        Ok(PathPat::Path {
            path: path.to_string(),
            anywhere,
            fold,
            below,
        })
    }

    /// This pattern against where Shannon started and the home: an anchored
    /// path made absolute (a glob segment stays as it is), a name found
    /// anywhere as it is.
    pub fn resolve(&self, start: &str, home: &str) -> Resolved {
        match self {
            PathPat::Path {
                path,
                anywhere: false,
                fold,
                below,
            } => Resolved::Segs {
                segs: segments(&absolute(start, path, home))
                    .into_iter()
                    .map(String::from)
                    .collect(),
                anywhere: false,
                fold: *fold,
                below: *below,
            },
            PathPat::Path {
                path,
                anywhere: true,
                fold,
                below,
            } => Resolved::Segs {
                segs: segments(path).into_iter().map(String::from).collect(),
                anywhere: true,
                fold: *fold,
                below: *below,
            },
            PathPat::Re(re) => Resolved::Re {
                re: re.clone(),
                start: start.to_string(),
            },
        }
    }

    /// The pattern as a rule would write it.
    pub fn show(&self) -> String {
        let tail = |below: bool| if below { "/**" } else { "" };
        match self {
            PathPat::Path {
                path,
                anywhere: false,
                below,
                ..
            } => format!("{}{}", path.trim_end_matches('/'), tail(*below)),
            PathPat::Path { path, below, .. } => format!("**/{path}{}", tail(*below)),
            PathPat::Re(re) => format!("re:{}", re.as_str()),
        }
    }
}

impl Resolved {
    /// Does this pattern name `path` (absolute, normalised)? Anchored: the
    /// path's segments begin with a run the pattern's match; anywhere: they
    /// contain one. A directory holds what is below it, so nothing past the
    /// run matters.
    pub fn matches(&self, path: &str) -> bool {
        self.meets(path, false, false)
    }

    /// As [`matches`](Self::matches), for a grant of a DELETE (or of a word
    /// at no operation, which may be one): a pattern written `DIR/**` then
    /// grants what is below the directory and not the directory itself —
    /// `allow rm build/**` does not cover `rm -rf build`, while reading
    /// `build` under `allow :read build/**` is reading what is below it.
    pub fn grants_delete(&self, path: &str) -> bool {
        self.meets(path, false, true)
    }

    /// Does the pattern reach `path` by contact: as [`matches`](Self::matches),
    /// and also where `path` is a glob placed as written that could pick an
    /// entry the pattern names (`.en*` reaches a deny of `.env*`). What a
    /// deny or an ask fires on; a grant needs [`matches`](Self::matches).
    pub fn reaches(&self, path: &str) -> bool {
        self.meets(path, true, false)
    }

    /// Is `path` a directory ABOVE what the pattern names — one whose
    /// removal, or recursive read, reaches the pattern's files (`rm -rf /`
    /// under `deny rm /etc/**`)? Only an anchored pattern has directories
    /// above it; one found anywhere, or a regex, has none.
    pub fn above(&self, path: &str) -> bool {
        match self {
            Resolved::Segs {
                segs: pat,
                anywhere: false,
                fold,
                below,
            } => {
                // By contact too: `/srv/d?ta` is above `/srv/data/keep`
                // (review 2026-10-10, A).
                let _ = below;
                let is_above = |path: &str| {
                    let segs = segments(path);
                    segs.len() < pat.len()
                        && segs.iter().enumerate().all(|(i, s)| {
                            glob_match(&pat[i], s, *fold) || globs_meet(&pat[i], s, *fold)
                        })
                };
                is_above(path)
                    || across_root_links(pat, path, true)
                        .iter()
                        .any(|p| is_above(p))
            }
            _ => false,
        }
    }

    fn meets(&self, path: &str, contact: bool, strict: bool) -> bool {
        match self {
            Resolved::Segs {
                segs: pat,
                anywhere,
                fold,
                below,
            } => {
                let met = |path: &str| {
                    // Below the run, strictly: the path must go on past it.
                    let segs = segments(path);
                    let holds = |at: usize| {
                        run_matches(pat, &segs, at, *fold, contact)
                            && (!*below || !strict || segs.len() > at + pat.len())
                    };
                    if *anywhere {
                        (0..=segs.len().saturating_sub(pat.len())).any(holds)
                    } else {
                        holds(0)
                    }
                };
                met(path) || across_root_links(pat, path, contact).iter().any(|p| met(p))
            }
            Resolved::Re { re, start } => {
                re.matches(path)
                    || path
                        .strip_prefix(start.as_str())
                        .and_then(|rest| rest.strip_prefix('/'))
                        .is_some_and(|relative| re.matches(relative))
            }
        }
    }

    /// `self ⊑ other`: everything `self` names, `other` names — the
    /// inclusion that orders path rules by specificity. An anchored pattern
    /// is within another when its segments begin with a run each within the
    /// other's; within an anywhere pattern when they contain one; an
    /// anywhere pattern is within no anchored one but the root (it may fall
    /// anywhere), and two regexes only when equal. Incomparable patterns
    /// are neither within the other, and the strictest of them decides.
    /// Reflexive and transitive (tested), so "the minimal rules" is well
    /// defined. Case: the container's folding governs — a folded pattern
    /// holds the exact one.
    pub fn within(&self, other: &Resolved) -> bool {
        self.within_as(other, true)
    }

    /// [`within`](Self::within), for an act that is a `delete` or not: `DIR/**`
    /// names less than `DIR` only for a delete, as it MATCHES less only for
    /// a delete grant ([`grants_delete`](Self::grants_delete)). For any other
    /// act the two name the same paths, and are as specific: a user's
    /// `allow :read d/**` was narrower than their `deny :read d`, or than a
    /// disabled skill's, and took its place (review of e10b6d5, A and C).
    pub fn within_as(&self, other: &Resolved, delete: bool) -> bool {
        match (self, other) {
            (
                Resolved::Segs {
                    segs: a,
                    anywhere: a_any,
                    below: a_below,
                    ..
                },
                Resolved::Segs {
                    segs: b,
                    anywhere: b_any,
                    fold,
                    below: b_below,
                },
            ) => {
                // Within a pattern that names only what is BELOW its run,
                // `self` must lie strictly below it: deeper, or itself below.
                let deep = |at: usize| !delete || !*b_below || *a_below || a.len() > at + b.len();
                match (a_any, b_any) {
                    (false, false) => run_within(a, 0, b, *fold) && deep(0),
                    (true, false) => b.is_empty() && deep(0),
                    _ => (0..=a.len().saturating_sub(b.len()))
                        .any(|at| run_within(a, at, b, *fold) && deep(at)),
                }
            }
            (Resolved::Re { re: a, .. }, Resolved::Re { re: b, .. }) => a == b,
            _ => false,
        }
    }
}

/// A host pattern: a host, a `*.suffix`, or any.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HostPat {
    Any,
    Exact(String),
    /// `*.github.com`: any host under it (not the apex itself).
    Under(String),
}

impl HostPat {
    pub fn parse(word: &str) -> Result<HostPat, String> {
        if word.is_empty() {
            return Err("a host rule names a host".into());
        }
        if word == "*" {
            return Ok(HostPat::Any);
        }
        if let Some(suffix) = word.strip_prefix("*.") {
            if suffix.is_empty() || suffix.contains('*') {
                return Err(format!("`{word}`: a host glob is `*.example.com`"));
            }
            return Ok(HostPat::Under(suffix.to_ascii_lowercase()));
        }
        if word.contains(['*', '?', '/']) {
            return Err(format!(
                "`{word}`: a host is a name, or `*.example.com` for the hosts under one"
            ));
        }
        Ok(HostPat::Exact(word.to_ascii_lowercase()))
    }

    pub fn matches(&self, host: &str) -> bool {
        let h = host.to_ascii_lowercase();
        match self {
            HostPat::Any => true,
            HostPat::Exact(e) => *e == h,
            HostPat::Under(s) => h.ends_with(&format!(".{s}")),
        }
    }

    /// `self ⊑ other`.
    pub fn within(&self, other: &HostPat) -> bool {
        match (self, other) {
            (_, HostPat::Any) => true,
            (HostPat::Exact(a), HostPat::Exact(b)) => a == b,
            (HostPat::Exact(a), HostPat::Under(s)) => a.ends_with(&format!(".{s}")),
            (HostPat::Under(a), HostPat::Under(b)) => a == b || a.ends_with(&format!(".{b}")),
            _ => false,
        }
    }

    pub fn show(&self) -> String {
        match self {
            HostPat::Any => "*".into(),
            HostPat::Exact(e) => e.clone(),
            HostPat::Under(s) => format!("*.{s}"),
        }
    }
}

/// A rule on a program: the form a command must take.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProgForm {
    /// The program's basename, or `*` for any program.
    pub program: String,
    /// The subcommand chain below the program.
    pub chain: Vec<String>,
    /// Flags that must be present, by canonical name (modelled) or spelling
    /// (unmodelled, or a `*` rule), each with the value it must carry.
    pub flags: Vec<(String, Option<String>)>,
    /// A trailing `*`: the allow covers what nothing else does.
    pub blanket: bool,
    /// Chain words and flags as written, in order — the literal prefix an
    /// unmodelled program's allow is read as.
    pub words: Vec<String>,
    /// A deny's or an ask's path words: places fenced off from this program
    /// where its form is met (`deny rm /etc/**`). An allow has none — its
    /// path words are arguments it must carry, and a place is granted by a
    /// path rule (Steven, 10-10: "the rules for programs are about
    /// arguments/flags while the rules about paths are about access").
    pub fences: Vec<PathPat>,
    /// Flags and words the model does not know (or, with no model, the
    /// flags), named only as written: another spelling of the same option
    /// is not caught, and the rule's note says so.
    pub literal: Vec<String>,
    /// Operands the model reads as data — a pattern, a remote (`rg TODO`,
    /// `git push origin`) — which the command must carry as written. Known
    /// to the model, so no note.
    pub positionals: Vec<String>,
}

impl ProgForm {
    /// How many of [`flags`](Self::flags) the form gives before its `k`-th
    /// chain word: where a chain runs into an inner program, the flags of
    /// what it runs past, not of what it runs (`allow sudo -u=pg psql -c *`:
    /// `-u` is sudo's, `-c` psql's). Every flag writes one dashed word, in
    /// order, and every chain word is a word too.
    pub fn flags_before(&self, k: usize) -> usize {
        let (mut chain_at, mut flags) = (0, 0);
        for w in &self.words {
            if chain_at < self.chain.len() && *w == self.chain[chain_at] {
                if chain_at == k {
                    return flags;
                }
                chain_at += 1;
            } else if w.starts_with('-') {
                flags += 1;
            }
        }
        flags
    }
}

/// What a rule is about.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Body {
    Program(ProgForm),
    /// A path at an operation; `scope` names the one program a grant from a
    /// program rule is for; `except` the places carved out of it — a
    /// shipped credential guard minus `[secrets] paths_except`, where the
    /// guard does not apply and nothing else is said.
    Path {
        op: Op,
        pat: PathPat,
        scope: Option<String>,
        except: Vec<PathPat>,
    },
    /// A host at a direction (`None`: either).
    Host {
        dir: Option<Dir>,
        host: HostPat,
    },
    /// A whole-line regex: the advanced tier, as before.
    Regex(RePattern),
}

impl Body {
    /// Is `other` the same question, however either was spelled? A program
    /// form by its program, chain, flags (canonical, so `-R` is
    /// `--recursive`), blanket and fences — not by the words as written. A
    /// path by everything but whether it folds case: you name a rule by its
    /// words, and a `remove = ["ask :write **/.claude"]` written before the
    /// shipped guard became `\c**/.claude` still names it (review
    /// 2026-10-13, B).
    pub fn same(&self, other: &Body) -> bool {
        let unfolded = |pat: &PathPat| match pat {
            PathPat::Path {
                path,
                anywhere,
                below,
                ..
            } => PathPat::Path {
                path: path.clone(),
                anywhere: *anywhere,
                fold: false,
                below: *below,
            },
            re => re.clone(),
        };
        match (self, other) {
            (
                Body::Path {
                    op,
                    pat,
                    scope,
                    except,
                },
                Body::Path {
                    op: op_b,
                    pat: pat_b,
                    scope: scope_b,
                    except: except_b,
                },
            ) => {
                op == op_b
                    && unfolded(pat) == unfolded(pat_b)
                    && scope == scope_b
                    && except == except_b
            }
            (Body::Program(a), Body::Program(b)) => {
                a.program == b.program
                    && a.chain == b.chain
                    && a.flags == b.flags
                    && a.blanket == b.blanket
                    && a.fences == b.fences
                    && a.positionals == b.positionals
                    && a.literal
                        .iter()
                        .filter(|w| !w.starts_with('-'))
                        .eq(b.literal.iter().filter(|w| !w.starts_with('-')))
            }
            _ => self == other,
        }
    }
}

/// One line, read: its verdict and the bodies it names (a path head with
/// several paths names several; an alternation of different flags is one
/// rule per flag).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Parsed {
    pub action: Decision,
    pub bodies: Vec<Body>,
}

impl Parsed {
    /// The program this line is about, when it is a program rule.
    pub fn program(&self) -> Option<&str> {
        self.bodies.iter().find_map(|b| match b {
            Body::Program(p) => Some(p.program.as_str()),
            _ => None,
        })
    }

    /// What to say where this rule is written, when it names a flag only as
    /// written: another spelling of the same option is not caught.
    pub fn as_written_note(&self, models: &Models) -> Option<String> {
        let form = self.bodies.iter().find_map(|b| match b {
            Body::Program(p) if !p.literal.is_empty() && p.program != "*" => Some(p),
            _ => None,
        })?;
        let named = form
            .literal
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let program = &form.program;
        Some(if models.get(program).is_some() {
            format!(
                "{named}: not a flag or subcommand Shannon knows for `{program}`, so it is \
                 matched only as written — another spelling is not caught"
            )
        } else {
            format!(
                "Shannon does not model `{program}`, so {named} is matched only as written — \
                 another spelling of the same option is not caught"
            )
        })
    }
}

/// Is this word a path, to a program rule? `.` and `~` are, and anything
/// with a `/` or a glob character; a bare word is a subcommand.
pub fn path_word(w: &str) -> bool {
    path_like(w)
}

fn path_like(w: &str) -> bool {
    w.contains('/') || w.contains(['*', '?', '[']) || w.starts_with('~') || w == "." || w == ".."
}

/// The shell operator a rule's word is, or ends in — `;`, `&&`, `|` — which
/// joins two commands where a rule names one. An escaped `\;` (find's
/// `-exec … \;`) is a word.
fn joins_commands(word: &str) -> Option<&'static str> {
    const OPS: [&str; 7] = [";", ";;", "&", "&&", "|", "||", "|&"];
    if let Some(op) = OPS.iter().find(|op| **op == word) {
        return Some(op);
    }
    (word.len() > 1 && word.ends_with(';') && !word.ends_with("\\;")).then_some(";")
}

/// Read one rule line. Errors are for the person who wrote it: what was
/// refused and what to write instead.
pub fn parse(line: &str, models: &Models) -> Result<Parsed, String> {
    let line = line.trim();
    if line.is_empty() {
        return Err("empty rule".into());
    }
    let (verb, target) = line
        .split_once(char::is_whitespace)
        .ok_or_else(|| format!("rule '{line}' has a verb but no target"))?;
    let action = match verb {
        "allow" => Decision::Allow,
        "ask" => Decision::Ask,
        "deny" => Decision::Deny,
        other => {
            return Err(format!(
                "unknown rule verb '{other}' (expected allow, ask, or deny)"
            ));
        }
    };
    let target = target.trim();
    if target.is_empty() {
        return Err("rule has no target".into());
    }
    // A whole-rule regex: the rest of the line is the pattern.
    if let Some(pattern) = target.strip_prefix("re:") {
        return Ok(Parsed {
            action,
            bodies: vec![Body::Regex(RePattern::compile(pattern)?)],
        });
    }
    // One rule names one command. `allow rm * ; allow :delete build` read
    // `;`, `allow` and `:delete` as words of `rm` and was added as one rule
    // that allowed neither (Steven's test, 10-11).
    if let Some(op) = target.split_whitespace().find_map(joins_commands) {
        return Err(format!(
            "`{op}` joins commands in the shell, and a rule names one — write each rule \
             by itself (`allow rm *`, then `allow :delete build`)"
        ));
    }
    let mut tokens = target.split_whitespace();
    let head = tokens.next().expect("a non-empty target has a first token");
    let rest: Vec<&str> = tokens.collect();

    if let Some(kind) = head.strip_prefix(':') {
        return heads(action, kind, &rest);
    }
    if head == "*" {
        return star(action, &rest, models);
    }
    program_rule(action, head, &rest, models)
}

/// `:<op> PATH…`, `:<dir> HOST…`, and the class heads that are no longer
/// rules.
fn heads(action: Decision, kind: &str, rest: &[&str]) -> Result<Parsed, String> {
    if let Some(op) = Op::parse(kind) {
        if rest.is_empty() {
            return Err(format!(
                "`:{kind}` takes paths: `:{kind} build`, `:{kind} ~/notes`"
            ));
        }
        let mut bodies = Vec::new();
        for word in rest {
            if word.starts_with('-') {
                return Err(format!(
                    "`:{kind} {word}`: a path rule takes paths, and `{word}` is a flag of nothing"
                ));
            }
            bodies.push(Body::Path {
                op,
                pat: PathPat::parse(word)?,
                scope: None,
                except: Vec::new(),
            });
        }
        return Ok(Parsed { action, bodies });
    }
    let dir = match kind {
        "fetch" => Some(Some(Dir::Fetch)),
        "send" => Some(Some(Dir::Send)),
        "web" => Some(None),
        _ => None,
    };
    if let Some(dir) = dir {
        if rest.is_empty() {
            return Err(format!("`:{kind}` takes hosts: `:{kind} api.github.com`"));
        }
        let mut bodies = Vec::new();
        for word in rest {
            bodies.push(Body::Host {
                dir,
                host: HostPat::parse(word)?,
            });
        }
        return Ok(Parsed { action, bodies });
    }
    match kind {
        "readonly" | "mutating" | "destructive" | "executor" | "unknown" => Err(format!(
            "`:{kind}` is no longer a rule: a modelled program at a read-only node is allowed \
             by default, and a class names no program, path or host a more specific rule \
             could be compared with — write `allow <program> *`, `:read <path>` or `:fetch \
             <host>` instead"
        )),
        other => Err(format!(
            "unknown head `:{other}` (the operations :read, :write, :delete, :exec, :*; the \
             directions :fetch, :send, :web)"
        )),
    }
}

/// `* …`: with path words, `:*` on each; with flags, a rule on any program
/// read by each command's model; alone, any program.
fn star(action: Decision, rest: &[&str], models: &Models) -> Result<Parsed, String> {
    let paths: Vec<&str> = rest
        .iter()
        .copied()
        .filter(|w| *w != "*" && path_like(w))
        .collect();
    if !paths.is_empty() {
        let mut bodies = Vec::new();
        for p in paths {
            // `* **`: every program, its paths anywhere (below the root).
            let p = if p == "**" { "/**" } else { p };
            bodies.push(Body::Path {
                op: Op::Any,
                pat: PathPat::parse(p)?,
                scope: None,
                except: Vec::new(),
            });
        }
        let flags: Vec<&str> = rest
            .iter()
            .copied()
            .filter(|w| w.starts_with('-'))
            .collect();
        if !flags.is_empty() {
            return Err(format!(
                "`* {}` mixes a path with a flag: a path rule (`:* PATH`) names no flag",
                rest.join(" ")
            ));
        }
        return Ok(Parsed { action, bodies });
    }
    // `allow *` alone: every program, whatever it carries (its paths still
    // judged where they are) — the blanket, not a form naming nothing.
    if rest.is_empty() {
        return program_rule(action, "*", &["*"], models);
    }
    program_rule(action, "*", rest, models)
}

/// `<program>[:constraint] <words…>`.
fn program_rule(
    action: Decision,
    head: &str,
    rest: &[&str],
    models: &Models,
) -> Result<Parsed, String> {
    let (program, inline) = match head.split_once(':') {
        Some((p, c)) => (p, Some(c)),
        None => (head, None),
    };
    if program.contains('/') {
        let name = program.rsplit('/').next().unwrap_or(program);
        return Err(format!(
            "a rule names a program, not a path: write `{name}`, which covers `{program}` \
             too — and a command run by a path is an exec of that file, which `:exec` grants"
        ));
    }
    if inline == Some("") {
        return Err(format!(
            "`{program}:` needs something after the colon — `{program} *` for any `{program}`, \
             or `{program} -flag` for a flag"
        ));
    }
    if program.is_empty() {
        return Err("rule has no program".into());
    }
    let model = (program != "*").then(|| models.get(program)).flatten();
    // The node of the model the chain has reached, for reading the words
    // after it; `None` once the chain leaves what the model knows.
    let mut node: Option<&Node> = model.map(|m| &m.root);
    // Past a node that runs the rest as a command: the inner program's
    // model (`Some(None)`: an inner program Shannon does not model), which
    // reads the flags that follow.
    let mut inner: Option<Option<&Model>> = None;
    // Every alternative reading of the flags: an alternation of different
    // flags is one rule per flag.
    let mut forms: Vec<ProgForm> = vec![ProgForm {
        program: program.to_string(),
        chain: Vec::new(),
        flags: Vec::new(),
        blanket: false,
        words: Vec::new(),
        fences: Vec::new(),
        literal: Vec::new(),
        positionals: Vec::new(),
    }];
    let allow = action == Decision::Allow;
    // Which operand of the node the next bare word is, for its role.
    let mut bare_at = 0usize;
    let tokens: Vec<&str> = inline.into_iter().chain(rest.iter().copied()).collect();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        i += 1;
        if tok == "*" || tok == "**" {
            // `*`: any flag, any further word, its places judged by the path
            // rules where they are. `**` on a deny or an ask also fences off
            // every place (below the root); on an allow it once granted them
            // — places are path rules' now (`allow :* /**`), so there it is
            // `*`.
            let anywhere = (tok == "**" && !allow)
                .then(|| PathPat::parse("/**"))
                .transpose()?;
            for f in &mut forms {
                f.blanket = true;
                f.fences.extend(anywhere.clone());
            }
            continue;
        }
        if let Some(re) = PathRe::parse(tok) {
            let pat = PathPat::Re(re?);
            if allow {
                return Err(format!(
                    "`{tok}` names places, and an allow on a program names its flags and \
                     arguments — grant the places with a path rule: `allow :<read|write|\
                     delete|exec|*> {tok}`, beside `allow {program} …`"
                ));
            }
            for f in &mut forms {
                f.fences.push(pat.clone());
            }
            continue;
        }
        // `word:-flag`: a subcommand and what it must carry.
        let tok = match subcommand_constraint(tok)? {
            Some((word, constraint)) => {
                node = node.and_then(|n| n.sub(word));
                for f in &mut forms {
                    f.chain.push(word.to_string());
                    f.words.push(word.to_string());
                }
                constraint
            }
            None => tok,
        };
        if tok.starts_with('-') {
            // `,` lists flags that must ALL be present; `|` lists
            // alternatives — spellings of one flag, or different flags.
            let parts: Vec<&str> = tok.split(',').collect();
            let parts: Vec<&str> =
                if parts.len() > 1 && parts.iter().all(|p| p.is_empty() || p.starts_with('-')) {
                    parts
                } else {
                    vec![tok]
                };
            let reader = match inner {
                Some(inner_model) => inner_model,
                None => model,
            };
            // A short cluster the model reads letter by letter (`rm -rf` is
            // `-r -f`) is the same flags here: each must be present. An
            // unmodelled program's `-rf` stays one word, as the reader keeps it.
            let parts: Vec<String> = parts
                .into_iter()
                .flat_map(|p| match split_bundle(p, program, reader) {
                    Some(letters) if !p.contains('|') => letters,
                    _ => vec![p.to_string()],
                })
                .collect();
            let single = parts.len() == 1;
            for part in &parts {
                let part = part.as_str();
                let alternatives: Vec<&str> = part.split('|').collect();
                let alternatives: Vec<&str> = if alternatives.len() > 1
                    && alternatives
                        .iter()
                        .all(|a| a.is_empty() || a.starts_with('-'))
                {
                    alternatives
                } else {
                    vec![part]
                };
                let mut readings: Vec<FlagReading> = Vec::new();
                for alt in alternatives {
                    let reading = flag_token(program, alt, reader)?;
                    if !readings
                        .iter()
                        .any(|r| r.name == reading.name && r.value == reading.value)
                    {
                        readings.push(reading);
                    }
                }
                // `--flag VALUE`: the model says the flag takes one value, so
                // the next word is it — unless it is another flag or `*`.
                if single
                    && readings.len() == 1
                    && readings[0].value.is_none()
                    && readings[0].takes_one
                    && let Some(next) = tokens.get(i)
                    && !next.starts_with('-')
                    && *next != "*"
                    && *next != "**"
                {
                    readings[0].value = Some(next.to_string());
                    i += 1;
                }
                let has_value = readings.iter().any(|r| r.value.is_some());
                let no_value = readings.iter().any(|r| r.value.is_none());
                if has_value && no_value {
                    return Err(format!(
                        "`{part}` mixes a flag with a flag's value — an alternation lists \
                         spellings of one question: `-X|--method`, or `-X=GET|--method=GET`"
                    ));
                }
                // One reading: every form takes it. Several: the forms
                // multiply, one per reading.
                let mut next = Vec::new();
                for f in &forms {
                    for r in &readings {
                        let mut g = f.clone();
                        // (A file the flag names is its value, matched as
                        // written; the place is the path rules', as an
                        // operand's is — `allow sort -o out.txt` and
                        // `allow :write out.txt`.)
                        g.flags.push((r.name.clone(), r.value.clone()));
                        // The word as written, the value attached: `--method
                        // GET`, `--method=GET` and (declared) `--method=` all
                        // write one word, so the form's next word is never
                        // read as this flag's value. Each reading's own
                        // spelling: an alternation's second flag is its own
                        // word, not the first's.
                        let written = part.split('|').next().unwrap_or(part);
                        let written = r.literal.as_deref().unwrap_or(written);
                        let spelling = written.split_once('=').map_or(written, |(f, _)| f);
                        g.words.push(match &r.value {
                            Some(v) => format!("{spelling}={v}"),
                            None => spelling.to_string(),
                        });
                        if let Some(l) = &r.literal {
                            g.literal.push(l.clone());
                        }
                        next.push(g);
                    }
                }
                forms = next;
                // Alternations multiply the forms: a line of twenty is a
                // million rules, read on every keystroke (review
                // 2026-10-10, B). Refused, with the way to write it.
                if forms.len() > 64 {
                    return Err(format!(
                        "`{tok}`: too many alternatives in one rule (over 64 readings) — write \
                         several rules instead"
                    ));
                }
            }
            continue;
        }
        // An operand the model reads as DATA — a refspec, a pattern, a
        // remote — is carried as written even when it looks like a path:
        // `allow git push origin feature/x` names the branch and grants no
        // place (review 2026-10-10, E2: read as a grant, it narrowed
        // nothing and `git push origin main` ran under it).
        if let Some(n) = node
            && n.subs.is_empty()
            && let Some(role) = positional_role(n, bare_at)
            && !role.is_file()
            && role != Role::Argv
        {
            bare_at += 1;
            for f in &mut forms {
                f.positionals.push(tok.to_string());
                f.words.push(tok.to_string());
            }
            continue;
        }
        // A place: a word that looks like a path, or an operand the model
        // reads as a file (`rm build`). A deny's or an ask's fences it off
        // from the program where its form is met. An allow's is an argument
        // the command must carry, as written — never a grant: the place is
        // judged by the path rules (Steven, 10-10), so `allow rm build`
        // covers `rm build` and `allow :delete build` lets it delete there.
        let operand = node
            .filter(|n| n.subs.is_empty())
            .and_then(|n| positional_role(n, bare_at));
        if path_like(tok) || operand.is_some_and(Role::is_file) {
            if operand.is_some() {
                bare_at += 1;
            }
            if !allow {
                let pat = PathPat::parse(tok)?;
                for f in &mut forms {
                    f.fences.push(pat.clone());
                }
                continue;
            }
            let leaf = node.is_some_and(|n| n.subs.is_empty());
            if node.is_some() && !leaf {
                node = None;
            }
            for f in &mut forms {
                f.words.push(tok.to_string());
                if leaf {
                    f.positionals.push(tok.to_string());
                } else if model.is_none() || inner.is_some() {
                    f.chain.push(tok.to_string());
                } else {
                    f.literal.push(tok.to_string());
                }
            }
            continue;
        }
        // A bare word, read by the model: the subcommand it names; at a node
        // that runs the rest as a command (`uv run PYTEST`, `git FOO` for
        // `git-foo`), the inner program, whose own model then reads what
        // follows; else a word Shannon does not know, matched only as
        // written. Unmodelled: a word of the form.
        match node {
            Some(n) if n.sub(tok).is_some() => {
                node = n.sub(tok);
                bare_at = 0;
                for f in &mut forms {
                    f.chain.push(tok.to_string());
                    f.words.push(tok.to_string());
                }
            }
            Some(n) if n.external || n.positionals.iter().any(|p| p.role == Role::Argv) => {
                node = None;
                inner = Some(models.get(tok));
                for f in &mut forms {
                    f.chain.push(tok.to_string());
                    f.words.push(tok.to_string());
                }
            }
            Some(_) => {
                node = None;
                for f in &mut forms {
                    f.words.push(tok.to_string());
                    f.literal.push(tok.to_string());
                }
            }
            None => {
                for f in &mut forms {
                    if model.is_none() || inner.is_some() {
                        f.chain.push(tok.to_string());
                    } else {
                        f.literal.push(tok.to_string());
                    }
                    f.words.push(tok.to_string());
                }
            }
        }
    }
    Ok(Parsed {
        action,
        bodies: forms.into_iter().map(Body::Program).collect(),
    })
}

/// The role of the `idx`-th operand at `node`: the positional at that index,
/// else the one for the rest, else the one for the last (which may be it).
fn positional_role(node: &Node, idx: usize) -> Option<Role> {
    use super::model::At;
    let indexed = node
        .positionals
        .iter()
        .filter(|p| matches!(p.at, At::Index(_)))
        .count();
    node.positionals
        .iter()
        .find(|p| p.at == At::Index(idx))
        .or_else(|| {
            node.positionals
                .iter()
                .find(|p| p.at == At::Rest && idx >= indexed)
        })
        .or_else(|| node.positionals.iter().find(|p| p.at == At::Last))
        .map(|p| p.role)
}

/// One flag of a rule, read.
struct FlagReading {
    /// The canonical name (modelled) or the spelling.
    name: String,
    value: Option<String>,
    /// The spelling, when the model does not know it.
    literal: Option<String>,
    /// The model says it takes one value, so a following word is it.
    takes_one: bool,
}

/// One flag token: `-f`, `--flag`, `--flag=VALUE`, `-f=VALUE`.
fn flag_token(program: &str, tok: &str, model: Option<&Model>) -> Result<FlagReading, String> {
    if tok.is_empty() || tok == "-" || tok.starts_with("-=") {
        return Err(format!(
            "`{tok}` is not a flag — a flag constraint is `-name` or `--name=value`"
        ));
    }
    // `-f=` DECLARES that the flag takes a value, any value — the word after
    // it is its value, not an argument (`allow bat -l=` covers `bat -l
    // rust`); `-f=V` names one. Either way the value is still judged as a
    // path wherever it could be one (Steven, 10-10: "preferred").
    let (spelling, value) = match tok.split_once('=') {
        Some((f, "")) if f.len() > 1 && f != "--" => (f, Some("*".to_string())),
        Some((f, v)) if f.len() > 1 && f != "--" => (f, Some(v.to_string())),
        _ => (tok, None),
    };
    if let Some(v) = &value
        && v.starts_with("re:")
    {
        return Err(format!(
            "`{tok}`: a flag's value takes a glob (`{spelling}=/etc/*`), not `re:` — for a \
             regex, write the rule as one: `deny re:…{spelling}=…`"
        ));
    }
    // A `*` rule's flag is read by each command's model when they meet; a
    // named program's resolves now, or stays literal.
    let flag = if program == "*" {
        None
    } else {
        model.and_then(|m| m.flag_named(spelling))
    };
    // The model's facts win: a value on a flag it says takes none would be a
    // rule that never meets a command — refused, with the reason.
    if let Some(f) = flag
        && f.arity == Arity::Zero
        && value.is_some()
    {
        // Whose model: a user's own file is named as theirs (review
        // 2026-10-11, A: "Shannon's model" pointed away from the file to fix).
        let whose = match model.map(|m| &m.source) {
            Some(super::model::Source::User(file)) => {
                format!(
                    "your model of `{program}` ({}/{file})",
                    super::model::USER_DIR
                )
            }
            _ => format!("Shannon's model of `{program}`"),
        };
        return Err(format!(
            "`{tok}`: {whose} says `{spelling}` takes no value — write `{spelling}`"
        ));
    }
    Ok(match flag {
        Some(f) => FlagReading {
            name: f.canon.clone(),
            value,
            literal: None,
            takes_one: f.arity == Arity::One,
        },
        None => FlagReading {
            name: spelling.to_string(),
            value,
            literal: Some(spelling.to_string()),
            takes_one: false,
        },
    })
}

/// `-rf` under a model whose family bundles short flags, when the model
/// names no `-rf` but names each letter: the letters as `-r`, `-f`. A
/// letter that takes a value ends the cluster, the rest being its value
/// (`-n5`), which is kept attached as `-n=5`.
fn split_bundle(tok: &str, program: &str, model: Option<&Model>) -> Option<Vec<String>> {
    let model = model?;
    if program == "*"
        || !model.family.bundling
        || tok.starts_with("--")
        || tok.contains('=')
        || tok.len() < 3
        || model.flag_named(tok).is_some()
    {
        return None;
    }
    let letters: Vec<char> = tok[1..].chars().collect();
    let mut out = Vec::new();
    for (at, c) in letters.iter().enumerate() {
        let one = format!("-{c}");
        let flag = model.flag_named(&one)?;
        if flag.arity == Arity::Zero {
            out.push(one);
        } else {
            let rest: String = letters[at + 1..].iter().collect();
            out.push(if rest.is_empty() {
                one
            } else {
                format!("{one}={rest}")
            });
            break;
        }
    }
    Some(out)
}

/// `word:constraint` after the program: a subcommand and what it must
/// carry — `gh api:-X` is `gh api -X`. Splits only where nothing else is
/// meant: a plain word, then a flag. `word:` and `word:*` are refused
/// rather than read either way.
fn subcommand_constraint(tok: &str) -> Result<Option<(&str, &str)>, String> {
    let Some((word, constraint)) = tok.split_once(':') else {
        return Ok(None);
    };
    let plain = !word.is_empty()
        && !word.starts_with('-')
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !plain {
        return Ok(None);
    }
    match constraint {
        "" => Err(format!(
            "`{word}:` needs something after the colon — `{word} -flag` for a flag, or \
             `{word} *` for any arguments"
        )),
        "*" | "**" => Err(format!(
            "`{tok}` reads two ways: write `{word} *` for `{word}` then any arguments, or \
             `{word}:?*` for an argument that begins `{word}:`"
        )),
        _ if constraint.starts_with('-') => Ok(Some((word, constraint))),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(line: &str) -> Parsed {
        parse(line, Models::shipped()).unwrap_or_else(|e| panic!("{line}: {e}"))
    }

    fn refused(line: &str) -> String {
        parse(line, Models::shipped()).expect_err(&format!("should refuse: {line}"))
    }

    fn path(w: &str) -> PathPat {
        PathPat::parse(w).unwrap()
    }

    fn form(line: &str) -> ProgForm {
        match &read(line).bodies[..] {
            [Body::Program(p)] => p.clone(),
            other => panic!("{line}: not one program rule: {other:?}"),
        }
    }

    /// **Every older spelling reads as the rule it meant.**
    #[test]
    fn the_older_spellings_still_read() {
        let f = form("allow find:*");
        assert!(f.blanket && f.flags.is_empty() && f.chain.is_empty());
        let f = form("ask find:-exec");
        assert_eq!(f.flags, [("exec".to_string(), None)]);
        assert_eq!(f, form("ask find -exec"), "`prog:-flag` is `prog -flag`");
        let f = form("deny find:-delete,-exec");
        assert_eq!(
            f.flags,
            [("delete".to_string(), None), ("exec".to_string(), None)]
        );
        let f = form("deny gh api:-X|--method");
        assert_eq!(f.chain, ["api"]);
        assert_eq!(
            f.flags,
            [("method".to_string(), None)],
            "two spellings of one flag"
        );
        assert_eq!(f, form("deny gh api -X"), "one rule, however spelled");
        let r = read("allow find -name|-iname");
        assert_eq!(r.bodies.len(), 2, "two different flags are two rules");
        // An allow's places are arguments, never fences or grants (Steven,
        // 10-10); a deny's are fences.
        let f = form("allow rm build/* *");
        assert!(f.blanket);
        assert!(f.fences.is_empty());
        assert_eq!(f.positionals, ["build/*"]);
        let f = form("allow rm:-r build");
        assert_eq!(f.flags, [("recursive".to_string(), None)]);
        assert_eq!(f.positionals, ["build"]);
        let f = form("deny rm:-r build");
        assert_eq!(f.fences, [path("build")]);
        assert!(f.positionals.is_empty());
        let f = form("deny tar --file=/etc/*");
        assert_eq!(f.flags, [("file".to_string(), Some("/etc/*".to_string()))]);
        let f = form("deny tar --exclude=a,b");
        assert_eq!(f.flags.len(), 1, "a value keeps its comma");
        // Levels: a trailing `/` is the tree itself (`/**` is below it).
        let r = read("allow :read docs/");
        assert_eq!(
            r.bodies,
            [Body::Path {
                op: Op::Read,
                pat: path("docs"),
                scope: None,
                except: Vec::new()
            }]
        );
        let r = read("allow :write build/** out/");
        assert_eq!(r.bodies.len(), 2);
        // `* PATH` is `:* PATH`.
        let r = read("deny * /etc/**");
        assert_eq!(
            r.bodies,
            [Body::Path {
                op: Op::Any,
                pat: path("/etc/**"),
                scope: None,
                except: Vec::new()
            }]
        );
        let f = form("deny * -r");
        assert_eq!(f.program, "*");
        assert_eq!(
            f.flags,
            [("-r".to_string(), None)],
            "a `*` rule's flag is read by each model"
        );
        let f = form("allow *");
        assert_eq!(f.program, "*");
        // The regex tier, as before.
        let r = read("deny re:^rm(\\s|$)");
        assert!(matches!(&r.bodies[..], [Body::Regex(_)]));
        // An operand the model reads as data is carried as written, known,
        // so no note; the chain stops where the model does.
        let r = read("allow gh pr view 12");
        let Body::Program(p) = &r.bodies[0] else {
            panic!()
        };
        assert_eq!(p.chain, ["pr", "view"]);
        assert_eq!(p.positionals, ["12"]);
        assert!(p.literal.is_empty());
        assert_eq!(p.words, ["pr", "view", "12"]);
        assert!(r.as_written_note(Models::shipped()).is_none());
        // A word past what the model describes is matched as written, and
        // the rule's note says so.
        let r = read("allow gh pr view 12 extra");
        let Body::Program(p) = &r.bodies[0] else {
            panic!()
        };
        assert_eq!(p.literal, ["extra"]);
        assert!(
            r.as_written_note(Models::shipped())
                .is_some_and(|n| n.contains("`extra`"))
        );
        // Unmodelled: every word is the form, with no note.
        let f = form("allow make test");
        assert_eq!(f.chain, ["test"]);
        assert!(f.literal.is_empty());
    }

    /// **The new targets.**
    #[test]
    fn the_new_targets_read() {
        let r = read("deny :* /mnt");
        assert!(matches!(&r.bodies[..], [Body::Path { op: Op::Any, .. }]));
        let r = read("allow :delete build");
        assert!(matches!(&r.bodies[..], [Body::Path { op: Op::Delete, .. }]));
        let r = read("allow :exec ./scripts");
        assert!(matches!(&r.bodies[..], [Body::Path { op: Op::Exec, .. }]));
        let r = read("allow :fetch api.github.com *.example.com");
        assert_eq!(
            r.bodies,
            [
                Body::Host {
                    dir: Some(Dir::Fetch),
                    host: HostPat::Exact("api.github.com".into())
                },
                Body::Host {
                    dir: Some(Dir::Fetch),
                    host: HostPat::Under("example.com".into())
                },
            ]
        );
        let r = read("deny :send *");
        assert_eq!(
            r.bodies,
            [Body::Host {
                dir: Some(Dir::Send),
                host: HostPat::Any
            }]
        );
        let r = read("allow :web example.com");
        assert!(matches!(&r.bodies[..], [Body::Host { dir: None, .. }]));
        let r = read("ask :* **/.env*");
        assert!(matches!(
            &r.bodies[..],
            [Body::Path {
                pat: PathPat::Path { anywhere: true, .. },
                ..
            }]
        ));
        let r = read("ask :read \\c~/.SSH");
        assert!(matches!(
            &r.bodies[..],
            [Body::Path {
                pat: PathPat::Path { fold: true, .. },
                ..
            }]
        ));
        // `--flag VALUE`: the model says `--method` takes one word.
        let f = form("allow gh api --method GET");
        assert_eq!(f.chain, ["api"]);
        assert_eq!(f.flags, [("method".to_string(), Some("GET".to_string()))]);
        assert_eq!(
            f,
            form("allow gh api --method=GET"),
            "one rule, however spelled"
        );
        let g = form("allow gh api -X GET");
        assert_eq!(
            (&g.chain, &g.flags),
            (&f.chain, &f.flags),
            "another spelling, one rule"
        );
        let f = form("allow gh api --method *");
        assert_eq!(
            f.flags,
            [("method".to_string(), None)],
            "`*` is never a value"
        );
        assert!(f.blanket);
        let f = form("allow git -c x log");
        assert_eq!(
            f.chain,
            ["log"],
            "`-c` takes one word, so `log` is the chain"
        );
        assert_eq!(f.flags, [("config".to_string(), Some("x".to_string()))]);
        // A zero-arity flag takes no word: the next one is read on its own.
        let f = form("allow rm -r build");
        assert_eq!(f.flags, [("recursive".to_string(), None)]);
        assert_eq!(f.positionals, ["build"]);
        let f = form("deny sudo");
        assert_eq!(
            f.program, "sudo",
            "a privilege wrapper may be named now: a user deny makes it a deny"
        );
    }

    /// **What is refused is refused with the reason**, never read as a
    /// different rule.
    #[test]
    fn what_cannot_be_meant_is_refused() {
        for (line, needle) in [
            ("deny /bin/rm -r", "names a program, not a path"),
            ("deny find: -delete", "needs something after the colon"),
            ("deny gh api: -X", "needs something after the colon"),
            ("deny gh api:* x", "reads two ways"),
            ("deny rm /etc/pass[w]d", "reads `[` as itself"),
            ("deny rm /etc/{a,b}", "no `{a,b}`"),
            ("allow :readonly", "no longer a rule"),
            ("deny :destructive", "no longer a rule"),
            ("deny tar --file=re:^/etc/", "not `re:`"),
            ("deny rm -=x", "is not a flag"),
            (
                "deny find -name|--file=x",
                "mixes a flag with a flag's value",
            ),
            ("allow :read", "takes paths"),
            ("allow :fetch", "takes hosts"),
            ("allow :fetch http://x/", "a host is a name"),
            ("allow :frob x", "unknown head"),
            ("allow :read -r", "a flag of nothing"),
            ("frob rm", "unknown rule verb"),
            ("allow", "verb but no target"),
            ("allow :read a/**/b", "`**` only begins"),
            ("allow :read **/../x", "without `.` or `..`"),
            ("allow * /etc -r", "mixes a path with a flag"),
            ("allow rm * ; allow :delete build", "`;` joins commands"),
            ("allow rm *; deny x", "`;` joins commands"),
            ("allow :read a && b", "`&&` joins commands"),
            ("deny cat x | tee", "`|` joins commands"),
        ] {
            let why = refused(line);
            assert!(why.contains(needle), "{line}: {why}");
        }
        // An escaped `;` is a word, and a regex's `|` is the regex's.
        read("ask find -exec * \\;");
        read("deny re:^(rm|mv) ");
    }

    /// **Flags resolve through the model, or stay literal and say so.**
    #[test]
    fn a_flag_resolves_through_the_model_or_is_literal() {
        let f = form("deny rm -R");
        assert_eq!(f.flags, [("recursive".to_string(), None)]);
        assert!(f.literal.is_empty());
        let r = read("deny curl -k");
        assert_eq!(form("deny curl -k").flags, [("insecure".to_string(), None)]);
        assert!(r.as_written_note(Models::shipped()).is_none());
        let r = read("deny frob --flag");
        assert_eq!(form("deny frob --flag").literal, ["--flag"]);
        assert!(
            r.as_written_note(Models::shipped())
                .is_some_and(|n| n.contains("does not model `frob`"))
        );
        let r = read("deny rg --frob");
        assert!(
            r.as_written_note(Models::shipped())
                .is_some_and(|n| n.contains("not a flag or subcommand Shannon knows"))
        );
        let r = read("deny rg --hidden");
        assert_eq!(
            form("deny rg --hidden").flags,
            [("hidden".to_string(), None)],
            "a guarded flag resolves: a rule may name it"
        );
        assert!(r.as_written_note(Models::shipped()).is_none());
    }

    /// **`--flag=` declares a value, any value**, and is one word of the
    /// form with it, so the next word is never read as the flag's; where
    /// the model says the flag takes none, a value is refused.
    #[test]
    fn an_empty_value_declares_one() {
        let f = form("allow frob -l= build");
        assert_eq!(f.flags, [("-l".to_string(), Some("*".to_string()))]);
        assert_eq!(f.words, ["-l=*", "build"]);
        assert!(read("allow frob -l=").bodies[0].same(&read("allow frob -l=*").bodies[0]));
        assert!(!read("allow frob -l=").bodies[0].same(&read("allow frob -l").bodies[0]));
        // A spelling of each: two rules, each with its own word.
        let r = read("allow frob -l=|--language=");
        assert_eq!(r.bodies.len(), 2);
        let words: Vec<&[String]> = r
            .bodies
            .iter()
            .map(|b| match b {
                Body::Program(f) => f.words.as_slice(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(words, [["-l=*"], ["--language=*"]]);
        // Modelled: the arity is the model's.
        assert_eq!(
            form("deny curl --data=").flags,
            [("data".to_string(), Some("*".to_string()))]
        );
        assert!(refused("allow rm -r=x").contains("takes no value"));
        assert!(refused("deny rm -r=").contains("takes no value"));
    }

    /// **A path pattern names what it says, and inclusion orders them.**
    #[test]
    fn path_patterns_match_and_nest() {
        let r = |w: &str| {
            PathPat::parse(w)
                .unwrap()
                .resolve("/work/me/proj", "/work/me")
        };
        assert!(r("/mnt").matches("/mnt/a/b"));
        assert!(r("/mnt").matches("/mnt"));
        assert!(!r("/mnt").matches("/mnta"));
        assert!(r("build").matches("/work/me/proj/build/x"));
        assert!(r("~/notes").matches("/work/me/notes/a"));
        assert!(r("build/*").matches("/work/me/proj/build/a.o"));
        assert!(
            r("build/*").matches("/work/me/proj/build/sub/a.o"),
            "the entry `sub` holds what is below it"
        );
        assert!(
            !r("build/*").matches("/work/me/proj/build"),
            "the directory itself is not an entry"
        );
        assert!(
            !r("build/*.o").matches("/work/me/proj/build/sub/a.o"),
            "the ENTRY's name is matched"
        );
        assert!(r("build/s*").matches("/work/me/proj/build/sub/a.o"));
        assert!(r("*.log").matches("/work/me/proj/x.log"));
        assert!(r("**/.env*").matches("/srv/app/.env.local"));
        assert!(r("**/secret").matches("/mnt/a/secret/x"));
        assert!(r("/").matches("/anything"));
        assert!(
            r("\\c~/.SSH").matches("/work/me/.ssh/config"),
            "case folded"
        );
        assert!(r("re:^/etc/pass[w]d$").matches("/etc/passwd"));
        // Inclusion.
        assert!(r("/mnt/foo").within(&r("/mnt")));
        assert!(!r("/mnt").within(&r("/mnt/foo")));
        assert!(r("/mnt/foo/x").within(&r("/mnt/foo/*")));
        assert!(r("/mnt/foo/*").within(&r("/mnt")));
        assert!(!r("/mnt").within(&r("/mnt/foo/*")));
        assert!(r("/a/secret").within(&r("**/secret")));
        assert!(
            !r("**/secret").within(&r("/mnt")),
            "incomparable: a name anywhere against a tree"
        );
        assert!(!r("/mnt").within(&r("**/secret")));
        assert!(r("/mnt/foo/*").within(&r("/mnt/foo/*")));
        assert!(r("/mnt/foo/x*").within(&r("/mnt/foo/*")));
        assert!(!r("/mnt/foo/*").within(&r("/mnt/foo/x*")));
        assert!(
            r("/mnt/foo/x/y").within(&r("/mnt/foo/*")),
            "below an entry the glob names"
        );
        assert!(r("/mnt/foo/x/*").within(&r("/mnt/foo/*")));
        assert!(
            r("/a/*").within(&r("**/a")),
            "every match has the segment `a`"
        );
        assert!(
            r("/x/a.log").within(&r("**/*.log")),
            "a literal name a glob matches"
        );
        assert!(!r("/x/*.log").within(&r("**/a.log")));
        // A glob above the last segment, and a run of names found anywhere.
        let memory = r("~/.claude/projects/*/memory");
        assert!(memory.matches("/work/me/.claude/projects/p/memory/notes.md"));
        assert!(!memory.matches("/work/me/.claude/projects/p/other"));
        assert!(
            memory.within(&r("~/.claude")),
            "the exception is within the guard"
        );
        assert!(!r("~/.claude").within(&memory));
        assert!(r("**/a/b").matches("/x/a/b/c"));
        assert!(!r("**/a/b").matches("/x/b/a"));
        assert!(r("/x/a/b").within(&r("**/a/b")));
        assert!(r("**/a/b").within(&r("**/b")));
        assert!(!r("**/b").within(&r("**/a/b")));
        assert!(
            r("**/x").within(&r("/")),
            "only the root holds a name found anywhere"
        );
        // Shown as written.
        assert_eq!(PathPat::parse("build/**").unwrap().show(), "build/**");
        assert_eq!(PathPat::parse("build/").unwrap().show(), "build");
        assert_eq!(PathPat::parse("**/.env*").unwrap().show(), "**/.env*");
        assert_eq!(PathPat::parse("*.log").unwrap().show(), "*.log");
        assert_eq!(PathPat::parse("/**").unwrap().show(), "/**");
        assert_eq!(PathPat::parse("**").unwrap().show(), "/**");
    }

    /// **`within` is a preorder**: reflexive and transitive, so "the minimal
    /// rules" is well defined — **and sound against `matches`**: a pattern
    /// within another holds no path the other does not, or "most specific
    /// wins" could pick a rule that covers what the wider one refused. The
    /// preorder alone held without it (review stop 12, T8).
    #[test]
    fn within_is_reflexive_and_transitive() {
        use proptest::prelude::*;
        let seg = prop_oneof![
            "[a-c]{1,2}".prop_map(String::from),
            Just("*".to_string()),
            "[a-c]\\*".prop_map(String::from),
        ];
        let pat = prop_oneof![
            proptest::collection::vec(seg.clone(), 0..4).prop_map(|v| format!("/{}", v.join("/"))),
            proptest::collection::vec(seg.clone(), 0..4)
                .prop_map(|v| format!("/{}/**", v.join("/"))),
            proptest::collection::vec(seg.clone(), 1..3)
                .prop_map(|v| format!("**/{}", v.join("/"))),
            proptest::collection::vec(seg, 1..3).prop_map(|v| format!("**/{}/**", v.join("/"))),
        ];
        let path =
            proptest::collection::vec("[a-c]{1,2}", 0..5).prop_map(|v| format!("/{}", v.join("/")));
        proptest!(ProptestConfig::with_cases(300), |(
            a in pat.clone(),
            b in pat.clone(),
            c in pat,
            paths in proptest::collection::vec(path, 1..8),
        )| {
            let r = |w: &str| PathPat::parse(w).unwrap().resolve("/p", "/h");
            let (a, b, c) = (r(&a), r(&b), r(&c));
            prop_assert!(a.within(&a));
            if a.within(&b) && b.within(&c) {
                prop_assert!(a.within(&c), "{a:?} ⊑ {b:?} ⊑ {c:?}");
            }
            if a.within(&b) {
                for p in &paths {
                    prop_assert!(!a.matches(p) || b.matches(p), "{a:?} ⊑ {b:?}, and only it holds {p}");
                }
            }
        });
    }

    /// **Hosts: exact, under, any.**
    #[test]
    fn host_patterns_match_and_nest() {
        let h = |w: &str| HostPat::parse(w).unwrap();
        assert!(h("api.github.com").matches("API.GITHUB.COM"));
        assert!(!h("api.github.com").matches("github.com"));
        assert!(h("*.github.com").matches("api.github.com"));
        assert!(!h("*.github.com").matches("github.com"), "not the apex");
        assert!(h("*").matches("anything"));
        assert!(h("api.github.com").within(&h("*.github.com")));
        assert!(h("*.github.com").within(&h("*")));
        assert!(!h("*.github.com").within(&h("api.github.com")));
        assert!(h("a.b.github.com").within(&h("*.github.com")));
        assert!(h("*.b.github.com").within(&h("*.github.com")));
    }

    /// **The reader never panics.**
    #[test]
    fn the_reader_never_panics() {
        use proptest::prelude::*;
        proptest!(ProptestConfig::with_cases(400), |(line in "[ -~]{0,40}")| {
            let _ = parse(&line, Models::shipped());
        });
    }
}
