//! Recognising places credentials live, before a command reads one.
//!
//! See `docs/DesignDocs/secrets.md` §7. This is **P2**, and it exists because the
//! effect lattice answers the wrong question once output leaves the machine:
//! `cat ~/.ssh/id_ed25519` is `ReadOnly` — it damages nothing locally, so policy
//! runs it with no pane and no keypress — and the key goes to a provider and into
//! our own episode log. This module is what makes that stop for a human.
//!
//! It is the *forward* half of a pair. This catches a read we can see coming; a
//! key arriving from a path we never listed is caught on the way out by
//! [`redact`](super::redact). Neither alone is enough, and the overlap is the
//! point.

use super::SecretsConfig;

/// **Claude Code's home, as an entry names it** — `{claude-home}`, alone or
/// leading a place under it (`{claude-home}/projects/*/memory`): the home
/// launch resolved (`claude_import::claude_home`, `$CLAUDE_CONFIG_DIR` else
/// `~/.claude`) and its real path, each a LITERAL, never a pattern. Filled
/// in by the gate (`placed_entries`); a gate told no home fills it with
/// nothing. The same name leads a `[memory]` or `[skills]` location.
pub const CLAUDE_HOME: &str = "{claude-home}";

/// **An entry with [`CLAUDE_HOME`] filled in**, as the entries the gate
/// reads: one per spelling of the home, the home a literal and what follows
/// it the entry's own glob. A home plain enough to be read as the dialect
/// is written into the entry; one holding a space or a character a glob or
/// the rule line would read (`/Users/me/My Claude`, `[1]`) becomes a `re:`
/// of the escaped home — read as a glob, a space split the line and the
/// guard held nothing, silently (the own-state guards' lesson, review
/// 2026-10-11, E2). An entry without the placeholder is itself; one with it
/// anywhere but first names no place, and says so.
pub(crate) fn placed_entries(entry: &str, homes: &[String]) -> Result<Vec<String>, String> {
    let Some(rest) = entry.strip_prefix(CLAUDE_HOME) else {
        return match entry.contains(CLAUDE_HOME) {
            true => Err(format!(
                "`{entry}`: {CLAUDE_HOME} leads an entry or is not one"
            )),
            false => Ok(vec![entry.to_string()]),
        };
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return Err(format!(
            "`{entry}`: what follows {CLAUDE_HOME} starts with `/`"
        ));
    }
    Ok(homes
        .iter()
        .map(|home| {
            if reads_as_written(home) {
                return format!("{home}{rest}");
            }
            let below = rest.strip_suffix("/**").unwrap_or(rest);
            format!("re:^{}{}(?:/.*)?$", regex::escape(home), glob_regex(below))
        })
        .collect())
}

/// **Does a literal path read as itself in a path word** — no space to split
/// the rule line, nothing a glob or the line would read (`*?[]{}`, `\`, `#`,
/// a quote)? The one answer for every literal the gate is handed: Claude
/// Code's home ([`placed_entries`]) and Shannon's own state
/// (`permission::defaults::own_state_lines`). Where it does not, the path
/// is written as an escaped `re:`.
pub(crate) fn reads_as_written(path: &str) -> bool {
    !path
        .chars()
        .any(|c| c.is_whitespace() || "*?[]{}\\#\"'".contains(c))
}

/// Paths that commonly hold credentials — a **seed, not the list**.
///
/// Extended by [`SecretsConfig::paths`]. Secrets live wherever a person decided
/// to put them, so a fixed list protects the median setup and silently fails
/// everyone else; this is the same concentric-rings rule as the credential
/// prefixes in [`redact`](super::redact).
///
/// Every entry, here and the user's, is a bash glob ([`glob_entry_regex`]):
/// a leading `~/` is anchored at your home (`~/.claude` is not a project's
/// `.claude`, nor another account's — that is outside where Shannon started,
/// and asks; Steven, 10-27), a name matches anywhere, and a literal name
/// matches a whole path segment — so `.ssh` matches `/home/x/.ssh/config`
/// without matching `my.ssh.notes`. The gate reads each as a guard
/// (`permission::defaults::credential_lines`); what it holds is specified by
/// `permission::credential_tests`, asked of the gate.
const DEFAULT_SENSITIVE_PATHS: &[&str] = &[
    "~/.ssh",
    "~/.aws",
    "~/.gnupg",
    "~/.config/gh",
    "~/.claude",
    // **Claude Code's home where it really is** — `$CLAUDE_CONFIG_DIR` when
    // set, which holds everything `~/.claude` would (`.credentials.json`,
    // `.claude.json`, transcripts, `settings.json` and its hooks), and the
    // real path behind either: resolved once at launch and filled in by the
    // gate ([`CLAUDE_HOME`]). With only `~/.claude` named, a home set
    // anywhere else was an ordinary path — Deny became Ask, and under a
    // broad read allow, Allow (review stop 5, L1, HIGH; Steven, 10-19, the
    // placeholder). `~/.claude` stays: a home moved away still holds what
    // it held.
    CLAUDE_HOME,
    // Claude Code's global config BESIDE that directory — account details
    // and MCP server configurations, whose `env` can hold tokens (its
    // settings documentation). A `~/` entry ends at its segment, so
    // `~/.claude` never held it (found 10-06, carving Claude Code's
    // memories out of `~/.claude`). A literal, not `~/.claude*.json`: a
    // glob entry holds what is below it too, and every `cat $D/*.rs` could
    // then become one, and asked. Another file of Claude Code's beside it,
    // undocumented, is yours to add (`[secrets] paths`).
    "~/.claude.json",
    // ...and its copies beside it: `.claude.json.backup` (legacy) and
    // `.claude.json.backup.<ts>` — the same keys, a name over. A stem on the last segment, as `.env*` is,
    // which holds nothing below it (Reviewer A, round 28).
    ".claude.json.backup*",
    "~/.kube",
    "~/.docker/config.json",
    ".env*",
    // `<stage>.env` — `prod.env`, `dev.env`. `.env*` is a STEM match on the
    // final segment, so it covers `.env.production` and misses this spelling
    // entirely (Reviewer E).
    "*.env",
    "*.pem",
    "*.key",
    // A key-file convention: a key kept in a project as `*.api` (Shannon's
    // own dev key was kept so, read from where it started until 10-11),
    // where a child the executor runs could read the key this
    // process authenticates with (Reviewer A). The RESOLVED path — whatever
    // `SHANNON_API_KEY_FILE` names, or the key setup saved — is also
    // registered at startup, because a pattern cannot cover a path the user
    // chose; this covers the convention, that covers the instance.
    "*.api",
    "*.gpg",
    "*.kdbx",
    "*.p12",
    "*.pfx",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "credentials",
    "credentials.json",
    ".git-credentials",
    "secrets.yaml",
    "secrets.yml",
];

impl SecretsConfig {
    /// Shipped seed plus the user's additions.
    pub fn all_paths(&self) -> impl Iterator<Item = &str> {
        DEFAULT_SENSITIVE_PATHS
            .iter()
            .copied()
            .chain(self.paths.iter().map(|s| s.as_str()))
    }

    /// The `paths` entries that protect nothing — a `re:` pattern that does
    /// not compile — as sentences, for the notes every config load shows.
    /// An entry that silently matched nothing would read as protection.
    ///
    /// And one that cannot be CHECKED against a glob (its automaton too large
    /// to build): it still matches a path as written, but makes every command
    /// holding a glob ask, for the whole session, and nothing said why until
    /// a prompt did — without naming the entry (review 20, Reviewer F).
    pub fn unhonoured(&self) -> Vec<String> {
        self.paths
            .iter()
            .filter_map(|entry| match crate::pattern::PathRe::listed(entry) {
                Some(Err(why)) => Some(format!("secrets.paths: {why} — it protects nothing")),
                Some(Ok(regex)) if !regex.checks_globs() => Some(format!(
                    "secrets.paths: `{entry}` is too large to check a glob against, so every \
                     command holding a glob asks under it — a smaller pattern is not (`\\w` as \
                     the ASCII class `[A-Za-z0-9_]` is far smaller)"
                )),
                _ => None,
            })
            .collect()
    }
}

/// **A `[secrets] paths` entry as the bash glob it is** — a regex for
/// `pattern::PathRe::listed` to compile — or `None` for an entry the rule
/// line reads as itself (`permission::defaults::secret_word`): a `re:` one,
/// one with no wildcard, and a name with a lone leading `*` (`*.pem`) or
/// trailing `*` (`.env*`), whose meaning is this one's.
///
/// Read as bash reads a pathname glob, with `globstar` (Steven, 10-06: "treat
/// it like you'd expect in a bash description"): `*` and `?` and a `[…]` class
/// never cross a `/`; `**` does, and as a whole segment it is zero or more
/// directories (`/srv/**/*.api` names `/srv/x.api`); `{a,b}` is either. Then:
///
/// - a NAME (no `/`) matches a file's name anywhere, as `find -name`
///   (`id_*.pub`, `cred*.json`);
/// - a PATH is anchored — `~/` at YOUR home, in either spelling (`PathRe`
///   reads a path both ways), as `~/x` is; `/` at the root; otherwise where
///   Shannon started — and protects everything below what it names
///   (`/srv/keys/*` holds `/srv/keys/a/b`). `~/` was any account's home
///   here and yours alone in a plain entry, so `~/vault/**` and `~/vault/x`
///   disagreed about `/home/other/vault` (Steven, 10-27: "your home").
///
/// Every other wildcard used to compare as the character itself, so
/// `/srv/keys/*.api` and `/srv/keys/**` protected nothing, silently, with
/// rules taking `**` the spelling a user reaches for (found 10-06).
pub(crate) fn glob_entry_regex(entry: &str) -> Option<String> {
    const WILD: [char; 4] = ['*', '?', '[', '{'];
    if entry.starts_with("re:") || !entry.contains(WILD) {
        return None;
    }
    let plain = |s: &str| !s.contains(WILD);
    let name = !entry.contains('/');
    if name
        && (entry.strip_prefix('*').is_some_and(plain)
            || entry.strip_suffix('*').is_some_and(plain))
    {
        return None;
    }
    if name {
        return Some(format!("(?:^|/){}$", glob_regex(entry)));
    }
    let entry = crate::shell::pipeline::normalize_path(entry);
    let (anchor, body) = match entry.strip_prefix("~/") {
        Some(rest) => ("^~/", rest),
        None => ("^", entry.as_str()),
    };
    // A trailing `/**` is everything below — what the tail already says —
    // and the place itself, which `rm -rf` and `ls` reach too.
    let body = body.strip_suffix("/**").unwrap_or(body);
    Some(format!("{anchor}{}(?:/.*)?$", glob_regex(body)))
}

/// A glob's body as a regex, as bash reads a pathname glob (see
/// [`glob_entry_regex`]). Unanchored; every literal escaped.
fn glob_regex(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    while at < chars.len() {
        match chars[at] {
            '*' if chars.get(at + 1) == Some(&'*') => {
                let whole = at == 0 || chars[at - 1] == '/';
                if whole && chars.get(at + 2) == Some(&'/') {
                    // `**/` — zero or more directories.
                    out.push_str("(?:.*/)?");
                    at += 3;
                } else {
                    out.push_str(".*");
                    at += 2;
                }
                continue;
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            '[' => match class_regex(&chars, at) {
                Some((class, close)) => {
                    out.push_str(&class);
                    at = close;
                }
                None => out.push_str(r"\["),
            },
            '{' => match brace_regex(&chars, at) {
                Some((either, close)) => {
                    out.push_str(&either);
                    at = close;
                }
                None => out.push_str(r"\{"),
            },
            c => out.push_str(&regex::escape(c.encode_utf8(&mut [0; 4]))),
        }
        at += 1;
    }
    out
}

/// The `[…]` class opening at `open`, as a regex class that never matches
/// `/`, and where it closes — `None` if it never does (a literal `[`, as in
/// bash). A `!` or `^` first negates; a `]` first is a member.
fn class_regex(chars: &[char], open: usize) -> Option<(String, usize)> {
    let mut at = open + 1;
    let negated = matches!(chars.get(at), Some('!' | '^'));
    if negated {
        at += 1;
    }
    let first = at;
    let close = (first..chars.len()).find(|&i| chars[i] == ']' && i > first)?;
    let mut members = String::new();
    for &c in &chars[first..close] {
        match c {
            '/' => {}
            '\\' | ']' | '[' | '^' | '&' | '~' => {
                members.push('\\');
                members.push(c);
            }
            c => members.push(c),
        }
    }
    // A class of nothing but `/`: no name holds one — or, negated, any.
    let class = match (negated, members.is_empty()) {
        (false, true) => r"[^\s\S]".to_string(),
        (true, true) => "[^/]".to_string(),
        (false, false) => format!("[{members}]"),
        (true, false) => format!("[^/{members}]"),
    };
    Some((class, close))
}

/// The `{a,b}` braces opening at `open`, as an alternation of their globs,
/// and where they close — `None` without a top-level comma (`{x}` is literal
/// to bash) or a close.
fn brace_regex(chars: &[char], open: usize) -> Option<(String, usize)> {
    let mut depth = 0;
    let mut parts = vec![open + 1];
    for (i, &c) in chars.iter().enumerate().skip(open + 1) {
        match c {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            '}' => {
                if parts.len() < 2 {
                    return None;
                }
                parts.push(i + 1);
                let either: Vec<String> = parts
                    .windows(2)
                    .map(|w| glob_regex(&chars[w[0]..w[1] - 1].iter().collect::<String>()))
                    .collect();
                return Some((format!("(?:{})", either.join("|")), i));
            }
            ',' if depth == 0 => parts.push(i + 1),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A glob entry is honoured** — none is said at load as protecting
    /// nothing. (What each holds is the gate's to say, and
    /// `permission::credential_tests` asks it.)
    #[test]
    fn every_glob_entry_is_honoured() {
        let every = SecretsConfig {
            paths: [
                "/srv/keys/*.vlt",
                "/srv/keys/**",
                "~/vault/*.vlt",
                "id_*.vlt",
                "*.{a,b}",
            ]
            .map(String::from)
            .to_vec(),
            ..SecretsConfig::default()
        };
        assert!(every.unhonoured().is_empty(), "{:?}", every.unhonoured());
    }

    /// **The glob grammar's edges**, as bash reads them: a `]` first in a
    /// class is a member, an unclosed `[` or a comma-less `{x}` is literal,
    /// and a class never matches `/`.
    #[test]
    fn a_globs_edges_are_bashs() {
        let regex = |entry: &str| glob_entry_regex(entry).expect("a glob");
        let re = |entry: &str| regex::Regex::new(&regex(entry)).unwrap();
        assert!(re("/a/[]x]").is_match("/a/]"));
        assert!(
            re("/a/[x*").is_match("/a/[xyz"),
            "an unclosed `[` is literal"
        );
        assert!(re("/a/{x}*").is_match("/a/{x}y"), "`{{x}}` is literal");
        assert!(
            !re("/a/[!xy]b").is_match("/a//b"),
            "a class never matches `/`"
        );
        assert!(!re("/a/[!x]b").is_match("/a//b"));
        assert!(re("/a/[!xy]b").is_match("/a/zb"));
        assert!(
            re("/a/[!/]b").is_match("/a/xb"),
            "a class of only `/`, negated, is `?`"
        );
        assert!(!re("/a/[!/]b").is_match("/a//b"));
        assert!(
            !re("/a/[/]b").is_match("/a//b"),
            "a class of only `/` names nothing"
        );
        assert!(re("/a/{b,c/d}").is_match("/a/c/d"));
        // The fast paths stay fast paths, and a `re:` is not a glob.
        for shape in ["*.pem", ".env*", "id_rsa", "~/.ssh", "/opt/x", "re:^/x/.*"] {
            assert_eq!(glob_entry_regex(shape), None, "{shape}");
        }
    }

    /// **A `re:` entry that protects nothing is said so at load** — one that
    /// does not compile, and one too large to check a glob against (it still
    /// matches; the note says what it costs, naming it: review 20, Reviewer
    /// F). One that compiles is said nothing of. (What each holds is the
    /// gate's: `permission::credential_tests`.)
    #[test]
    fn a_listed_regex_that_protects_nothing_is_said() {
        let cfg = SecretsConfig {
            paths: vec![
                r"re:^/srv/keys/.*\.(vlt|box)$".to_string(),
                r"re:^~/vault/".to_string(),
                r"re:(?-i)^/opt/Exact/".to_string(),
            ],
            ..SecretsConfig::default()
        };
        assert!(cfg.unhonoured().is_empty(), "{:?}", cfg.unhonoured());

        let broken = SecretsConfig {
            paths: vec!["re:(unclosed".to_string()],
            ..SecretsConfig::default()
        };
        let said = broken.unhonoured();
        assert!(
            said.len() == 1 && said[0].contains("protects nothing"),
            "{said:?}"
        );

        let huge = SecretsConfig {
            paths: vec![r"re:^/x/(a|b)*a(a|b){24}$".to_string()],
            ..SecretsConfig::default()
        };
        let said = huge.unhonoured();
        assert!(
            said.len() == 1
                && said[0].contains("too large to check a glob")
                && said[0].contains("(a|b)"),
            "{said:?}"
        );
    }
}
