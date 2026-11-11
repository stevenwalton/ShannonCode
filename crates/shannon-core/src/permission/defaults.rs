//! The shipped rules: GUARDS — asks and denies only a rule of yours at least
//! as specific lifts — and the few whole-command allows.
//!
//! This is a **seed**, deliberately small, real, and honestly labeled. The
//! engine already asks for anything that is not a modelled read inside the
//! project, so an empty policy is never reckless; these keep the dangerous
//! corners asking beneath a broad allow of yours that does not speak to
//! them — `allow :read ~` under which `~/.ssh` still asks, `allow rm *`
//! under which `--no-preserve-root` is still refused. **An allow of yours
//! that names the place or the flag lifts a guard** (`allow :read ~/.ssh`,
//! `allow rm --no-preserve-root`): protection by default, never a floor.
//!
//! What is NOT here, and was: the carvings — `ask find:-exec` and its family,
//! `sort --compress-program`, `rg --pre`, `fd --exec`. A flag that raises a
//! program past a read is never benign to the model
//! (`acts::Touch::benign`), so an allow that does not name it does not cover
//! it, and with no rule such a program asks (design §8.1, Q7). The model
//! carries what the rules said, complete per program by construction. A
//! `[permissions] remove` naming one is told so (`RuleSet::shipped_after_by`).
//!
//! The credential paths are guards too, read from `[secrets] paths`
//! ([`credential_lines`]): one grammar and one reading for every path the
//! gate judges, instead of a list with a dialect of its own.
//!
//! Every fixed rule can be dropped by naming it in `[permissions] remove`
//! (`RuleSet::configured`).

use super::line::parse;
use super::model::Models;
use super::rule::Guard;
use super::{Origin, Rule};

/// The fixed shipped lines. Kept as text so the exact same path a user's
/// config takes (read → parse) is exercised by the defaults too — if a
/// spelling here were wrong, the parser would reject it, and the test below
/// does.
const SHIPPED_LINES: &[&str] = &[
    // The single flag that turns `rm -rf /` from a protected no-op into a
    // catastrophe: refused under any blanket, lifted only by a rule naming
    // it.
    "deny rm --no-preserve-root",
    // The harness's own instruction surfaces: a write into them is a soft
    // ask under a project-wide grant, lifted by a grant naming them (Q13).
    // Case-insensitive (`\c`), as the credential guards are: on macOS and
    // Windows `.SHANNON/config.toml` IS `.shannon/config.toml`, and the
    // case-sensitive guard let the agent write a trusted project's rules
    // unasked by spelling it so (review 2026-10-13, B).
    r"ask :write \c**/.claude",
    r"ask :write \c**/.shannon",
    r"ask :write \c**/.git/hooks",
    r"ask :write \c~/.config/shannon",
    // `date` that only prints: display flags, then a `+format`. Anything else
    // can SET the clock — GNU's `-s`/`--set` (which it takes after the format
    // too: options permute), and on BSD and macOS any operand that is not a
    // format, before or AFTER one (`date +%s 0101000026`; `date.c` reads the
    // operands in any order) — and busybox and toybox set `12:00`, `@0` or
    // `+12:00` after a format (review, Reviewer B). The words arrive joined by
    // spaces, so a quoted format's pieces look like operands: a piece may
    // follow the format only if it does not begin with `-` and holds no digit,
    // which every one of those times does (`%H:%M:%S` and `UTC` hold none).
    // A model is the better home for `date`; this is the seed.
    r"allow re:^date( -u| --utc| --universal| -R| --rfc-email| -I[a-z]*| --iso-8601(=[a-z]+)?| --rfc-3339=[a-z]+)*( \+[^ ]*( [^ 0-9-][^ 0-9]*)*)?$",
    // (`sed -n 1,60p a.rs` was a `re:` allow here until the sed model read
    // scripts. Deleted 10-27, Q3: on its whole domain no decision moved, and
    // the model is never looser than `cat` over the same words —
    // `sed_printing_lines_is_never_looser_than_cat`.)
];

/// The fixed shipped rules. Panics only if a line above is malformed — a
/// programming error caught immediately by the tests, never something a
/// user can trigger.
pub(super) fn fixed() -> Vec<Rule> {
    SHIPPED_LINES
        .iter()
        // By the shipped set: the shipped rules' own reading, which
        // `RuleSet::configured_by` reads again by the models in force.
        .filter_map(|line| Rule::read_by(Origin::Shipped, line, super::model::Models::shipped()))
        .map(|rule| rule.expect("built-in default rule must parse"))
        .collect()
}

/// **A `[secrets] paths` entry as a path word of the rule line.** The
/// entries are bash globs in the list's own dialect — `~/x` anchored at a
/// home, a name or a glob of one found anywhere, every one case-insensitive
/// — so `~/.ssh` is `\c~/.ssh`, `*.pem` is `\c**/*.pem`, and a `re:` keeps
/// its pattern with `(?i)`. A class or a brace, which the rule line refuses,
/// keeps its protection as the dialect's own regex (`glob_entry_regex`),
/// where only an identical `re:` grant compares with it.
fn secret_word(entry: &str) -> Result<String, String> {
    if let Some(re) = entry.strip_prefix("re:") {
        return Ok(format!("re:(?i){re}"));
    }
    if entry.contains(['[', '{']) || entry.contains("**") {
        let re = crate::secrets::glob_entry_regex(entry)
            .ok_or_else(|| format!("`{entry}`: not a pattern Shannon reads"))?;
        return Ok(format!("re:(?i){re}"));
    }
    if entry.starts_with("~/") || entry.starts_with('/') {
        return Ok(format!("\\c{entry}"));
    }
    Ok(format!("\\c**/{entry}"))
}

/// **The credential guards a `[secrets]` configuration asks for**, as rule
/// lines: every entry of `paths` (the seed and the user's) as `ask :* P` —
/// or `deny :* P` under `path_policy = "deny"`, none under `allow` — with
/// `{claude-home}` filled from `claude_home` (`secrets::placed_entries`).
///
/// **A place under Claude Code's home is a harness surface as well as a
/// credential store** — its `settings.json` runs hooks — so under `allow`
/// its WRITE still asks, as Shannon's own state's does
/// ([`own_state_lines`]) and as `**/.claude`'s shipped ask does for a home
/// of that name. Entries the line cannot read come back as notes, and so
/// does one naming Claude Code's home in a run that knows none: nothing is
/// dropped silently.
pub(super) fn credential_lines(
    secrets: &crate::SecretsConfig,
    claude_home: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut lines = Vec::new();
    let mut notes = Vec::new();
    for entry in secrets.all_paths() {
        let verb = match (
            secrets.path_policy,
            entry.starts_with(crate::secrets::CLAUDE_HOME),
        ) {
            (crate::PathPolicy::Ask, _) => "ask :*",
            (crate::PathPolicy::Deny, _) => "deny :*",
            (crate::PathPolicy::Allow, true) => "ask :write",
            (crate::PathPolicy::Allow, false) => continue,
        };
        let placed = match crate::secrets::placed_entries(entry, claude_home) {
            Ok(placed) => placed,
            Err(why) => {
                notes.push(format!("[secrets] paths: {why}"));
                continue;
            }
        };
        if placed.is_empty() && secrets.paths.iter().any(|p| p == entry) {
            notes.push(format!(
                "[secrets] paths: `{entry}` protects nothing — Claude Code's home is not known \
                 in this run"
            ));
        }
        for placed in placed {
            match secret_word(&placed) {
                Ok(word) => lines.push(format!("{verb} {word}")),
                Err(why) => notes.push(format!("[secrets] paths: {why}")),
            }
        }
    }
    (lines, notes)
}

/// **The guards on Shannon's own state** — its config, program models, key,
/// log and store, registered by launch from each resolver — as rule lines.
/// Each path is a LITERAL, never a pattern: `\c<path>`, or where the path
/// holds a space or a character the rule line or a glob would read
/// (`~/Library/Application Support`, `[1]`), its escaped regex — read as a
/// `[secrets]` entry, a space split the line and the guard protected nothing,
/// silently (review 2026-10-11, E2). Under `path_policy = "allow"` a write
/// still asks: the knob says how a credential's READ is treated, and a write
/// into Shannon's own state loosens the gate itself.
pub(super) fn own_state_lines(paths: &[String], policy: crate::PathPolicy) -> Vec<String> {
    let verb = match policy {
        crate::PathPolicy::Ask => "ask :*",
        crate::PathPolicy::Deny => "deny :*",
        crate::PathPolicy::Allow => "ask :write",
    };
    paths
        .iter()
        .map(|p| {
            let word = if crate::secrets::reads_as_written(p) {
                format!("\\c{p}")
            } else {
                format!("re:(?i)^{}(?:/.*)?$", regex::escape(p))
            };
            format!("{verb} {word}")
        })
        .collect()
}

/// **The credential guards, read**: each line of [`credential_lines`] and of
/// [`own_state_lines`] parsed, with every `[secrets] paths_except` place
/// CARVED OUT of its body — where a guard holds the whole place (`~/.claude`
/// holds `~/.claude/projects/*/memory`) the guard does not apply there, and
/// nothing else is said: a carved-out read outside the project still asks as
/// any other does. Not a grant: a `paths_except` written as a shipped allow
/// approved the memory place for every session (review, 10-10).
pub(super) fn credential_bodies(
    secrets: &crate::SecretsConfig,
    own_state: &[String],
    claude_home: &[String],
) -> (Vec<(String, super::line::Parsed, Guard)>, Vec<String>) {
    use super::line::{Body, PathPat};
    let (lines, mut notes) = credential_lines(secrets, claude_home);
    let lines = lines.into_iter().map(|l| (l, Guard::Credential)).chain(
        own_state_lines(own_state, secrets.path_policy)
            .into_iter()
            .map(|l| (l, Guard::OwnState)),
    );
    // Carved where Claude Code's home really is, as its guard is placed: a
    // guard filled in and its carve-out not made the home's memories Deny.
    let mut except: Vec<PathPat> = Vec::new();
    for entry in &secrets.paths_except {
        let read = crate::secrets::placed_entries(entry, claude_home).and_then(|placed| {
            placed
                .iter()
                .map(|p| secret_word(p).and_then(|w| PathPat::parse(&w)))
                .collect::<Result<Vec<_>, _>>()
        });
        match read {
            Ok(pats) => except.extend(pats),
            Err(why) => notes.push(format!("[secrets] paths_except: {why}")),
        }
    }
    let mut bodies = Vec::new();
    for (line, guard) in lines {
        match parse(&line, Models::shipped()) {
            Ok(mut parsed) => {
                for body in &mut parsed.bodies {
                    if let Body::Path { except: carved, .. } = body {
                        *carved = except.clone();
                    }
                }
                bodies.push((line, parsed, guard));
            }
            Err(why) => notes.push(format!("[secrets]: `{line}` is not in force: {why}")),
        }
    }
    (bodies, notes)
}

/// The `[secrets]` entry a credential guard was made from, for a message
/// that says which pattern matched.
pub(super) fn credential_entry(source: &str) -> String {
    let word = source.split_once(" :* ").map(|(_, w)| w).unwrap_or(source);
    let word = word.strip_prefix("\\c").unwrap_or(word);
    let word = word.strip_prefix("**/").unwrap_or(word);
    word.strip_prefix("re:(?i)")
        .map(|re| format!("re:{re}"))
        .unwrap_or_else(|| word.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{Decision, RuleSet};

    /// **The one shipped deny is the catastrophic one.** Every other default
    /// is an ask, so the user can still approve it; a deny added here is a
    /// decision to take that away, and must be made on purpose.
    #[test]
    fn the_only_shipped_deny_is_rm_no_preserve_root() {
        let denies: Vec<&str> = SHIPPED_LINES
            .iter()
            .copied()
            .filter(|line| line.starts_with("deny"))
            .collect();
        assert_eq!(denies, ["deny rm --no-preserve-root"]);
    }

    /// **A shipped allow is a whole command, read in full**: a `re:`
    /// anchored at both ends, so an unanchored one cannot match a longer
    /// command than its author meant.
    #[test]
    fn every_default_parses_and_a_shipped_allow_is_a_whole_command() {
        let rules = fixed();
        assert_eq!(rules.len(), SHIPPED_LINES.len());
        for r in rules.iter().filter(|r| r.action == Decision::Allow) {
            let regex = r
                .source
                .strip_prefix("allow re:")
                .unwrap_or_else(|| panic!("a shipped allow names no whole command: {}", r.source));
            assert!(
                regex.starts_with('^') && regex.ends_with('$'),
                "a shipped allow anchored at both ends: {}",
                r.source
            );
        }
    }

    /// **Every seed entry of `[secrets] paths` reads as a guard**, and a
    /// user's entry the line cannot read is a note, never dropped.
    #[test]
    fn the_credential_seed_reads_and_an_unreadable_entry_is_said() {
        let (lines, notes) = credential_lines(&crate::SecretsConfig::default(), &[]);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(lines.iter().any(|l| l == "ask :* \\c~/.ssh"));
        assert!(lines.iter().any(|l| l == "ask :* \\c**/*.pem"));
        // The carve-outs ride on every guard, not as grants.
        let (bodies, notes) = credential_bodies(&crate::SecretsConfig::default(), &[], &[]);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(bodies.iter().all(|(_, p, _)| p.action != Decision::Allow));
        let memory = super::super::line::PathPat::parse("\\c~/.claude/projects/*/memory").unwrap();
        assert!(
            bodies
                .iter()
                .all(|(_, p, _)| p.bodies.iter().all(|b| matches!(
                    b,
                    super::super::line::Body::Path { except, .. } if except.contains(&memory)
                )))
        );
        let mut cfg = crate::SecretsConfig::default();
        cfg.paths.push("/srv/keys/[ab].pem".into());
        cfg.paths.push("re:^/opt/vault".into());
        let (lines, notes) = credential_lines(&cfg, &[]);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(
            lines.iter().any(|l| l.starts_with("ask :* re:(?i)")),
            "a class keeps its protection as a regex"
        );
        assert!(lines.iter().any(|l| l == "ask :* re:(?i)^/opt/vault"));
        cfg.path_policy = crate::PathPolicy::Deny;
        let (lines, _) = credential_lines(&cfg, &[]);
        assert!(lines.iter().all(|l| !l.starts_with("ask ")));
        cfg.path_policy = crate::PathPolicy::Allow;
        assert!(credential_lines(&cfg, &[]).0.is_empty());
        assert_eq!(credential_entry("ask :* \\c~/.ssh"), "~/.ssh");
        assert_eq!(credential_entry("ask :* \\c**/*.pem"), "*.pem");
    }

    /// **`sed -n` printing numbered lines is allowed; a script that writes,
    /// reads another file or runs a command is not**, nor an in-place edit —
    /// all by the sed model, which reads the script.
    #[test]
    fn sed_is_allowed_to_print_lines_and_nothing_else() {
        let rs = RuleSet::defaults().with_start("/home/me/proj");
        for line in [
            "sed -n 1,60p crates/a.rs",
            "sed -n '270,310p' crates/a.rs",
            "sed -n 5p a.rs b.rs",
            "sed -n '$p' a.rs",
            "sed -n '10,$p' a.rs",
            "ls | sed -n 1,5p",
            "sed -n 1p ./a.rs",
            "sed -n 1,60p a.rs; echo -----; sed -n 270,310p a.rs",
            // A substitution to stdout is a read of the project, allowed by
            // default (the suite's `sed-substitute-to-stdout-with-a-model`),
            // and a redirect into the project is a write inside, allowed by
            // default (Q7).
            "sed 's/a/b/' a.rs",
            "sed -n 1,5p a.rs > out",
        ] {
            assert_eq!(rs.decide_line(line).decision, Decision::Allow, "{line}");
        }
        for line in [
            "sed -n 1p -i a.rs",
            "sed -i -n 1p a.rs",
            "sed -n 1,5p --in-place a.rs",
            "sed -n '1w out' a.rs",
            "sed -n 1pw a.rs",
            "sed -n '1p;w x' a.rs",
            "sed -n '1r /etc/passwd' a.rs",
            "sed -n 1e a.rs",
            "sed -n 1,5p a.rs > /tmp/out",
            "sed -n 1p $F",
            // Only where Shannon started, as a read with no rule.
            "sed -n 1p /etc/hosts",
            "sed -n 1p ../../etc/hosts",
            "sed -n 1p src/../../x",
            "sed -n 1p a.rs /etc/hosts",
            "cd /etc && sed -n 1p hosts",
        ] {
            assert_ne!(rs.decide_line(line).decision, Decision::Allow, "{line}");
        }
        assert_eq!(
            rs.decide_line("sed -n 1p ~/.ssh/id_rsa").decision,
            Decision::Ask,
            "a credential path still asks (the shipped guard)"
        );
    }

    /// **`date` that prints is allowed; `date` that could set the clock is
    /// not** — every spelling of setting it that the rule's comment names.
    #[test]
    fn date_is_allowed_to_print_and_never_to_set() {
        let rs = RuleSet::defaults().with_start("/home/me/proj");
        for line in [
            "date",
            "date -u",
            "date +%s",
            "date -u '+%Y-%m-%d %H:%M:%S UTC'",
            "date -Iseconds",
            "date --iso-8601=seconds",
            "date -R",
            "ls x 2>&1 | tail -5; date -u '+%Y-%m-%d %H:%M:%S UTC'",
            // An assignment that is no hazard (`TZ`) rides on an allow, as it
            // does on `allow date *`; one that changes what runs or what it
            // loads (`PATH`, `LD_PRELOAD`) is an unsound stage, whatever the
            // rule. The old engine held every assignment against a `re:`
            // allow; the acts engine judges the assignment itself.
            "TZ=x date",
        ] {
            assert_eq!(rs.decide_line(line).decision, Decision::Allow, "{line}");
        }
        for line in [
            "date -s 2020-01-01",
            "date --set=2020-01-01",
            "date --set 2020-01-01",
            "date 0101000026",
            "date +%s 0101000026",
            "date '+%s' 0101.00",
            "date +%s -s x",
            "date -j 0101000026",
            "date -d tomorrow",
            "date $X",
            "date -u > /tmp/x",
            "LD_PRELOAD=/tmp/x.so date",
            "date -u -s x",
            "date -Iseconds --set=x",
            "date -R 0101000026",
            "date +%s @0",
            "date +%s 12:00",
            "date +%s +12:00",
            "date -u '+%Y-%m-%d %H:%M' 23:59",
        ] {
            assert_ne!(rs.decide_line(line).decision, Decision::Allow, "{line}");
        }
    }

    /// **`sed -n` printing lines is never looser than `cat` over the same
    /// words** — in decision, and in what may run with no one watching
    /// (`safe_unattended`). It was a shipped `re:` allow and is the sed
    /// model's alone now (Q3, 10-27): before the allow was deleted, its whole
    /// domain — 8943 lines under both credential policies — was decided with
    /// and without it, and no decision moved; the lines that became safe
    /// unattended (2394, as no rule ever is) were each safe for `cat`, and
    /// where the two differed sed was the stricter (a glob that may become
    /// `-i`; `$p`, which the shell expands). This is that probe's property,
    /// kept.
    #[test]
    fn sed_printing_lines_is_never_looser_than_cat() {
        let scripts = [
            "1p",
            "$p",
            "10p",
            "1,60p",
            "10,$p",
            "0,0p",
            "5,1p",
            "99999999999999999999p",
        ];
        let words = [
            "a.rs",
            "src/x.rs",
            "./a.rs",
            "*.rs",
            "a?.rs",
            "/etc/hosts",
            "../x",
            "src/../../x",
            "~/.ssh/id_rsa",
            ".env",
            "~/x",
            "$F",
            "\"$F\"",
            "'a b'",
            "x=1",
            "+x",
            "{a,b}",
            "~",
            ".",
            "/",
            "/home/me/proj/.env",
            ".git/hooks/pre-commit",
            "\\-x",
            "\"-x\"",
        ];
        let few = ["a.rs", "*.rs", "/etc/hosts", ".env", "$F", "\\-x"];
        let mut tails: Vec<String> = vec![String::new()];
        tails.extend(words.iter().map(|w| format!(" {w}")));
        tails.extend(
            few.iter()
                .flat_map(|a| few.iter().map(move |b| format!(" {a} {b}"))),
        );
        for policy in [crate::PathPolicy::Ask, crate::PathPolicy::Deny] {
            let rs = RuleSet::defaults()
                .with_start("/home/me/proj")
                .with_secrets(crate::SecretsConfig {
                    path_policy: policy,
                    ..Default::default()
                });
            let sed = rs.decide_line("sed -n 1,60p src/x.rs");
            assert!(
                sed.decision == Decision::Allow && sed.safe_unattended(),
                "premise: a print inside runs, unattended ({policy:?})"
            );
            for script in scripts {
                for tail in &tails {
                    let line = format!("sed -n {script}{tail}");
                    let (sed, cat) = (rs.decide_line(&line), rs.decide_line(&format!("cat{tail}")));
                    assert!(sed.decision >= cat.decision, "{policy:?} `{line}`: {sed:?}");
                    assert!(
                        !sed.safe_unattended() || cat.safe_unattended(),
                        "{policy:?} `{line}` runs unattended where `cat{tail}` does not"
                    );
                }
            }
        }
    }
}
