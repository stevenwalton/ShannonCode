//! **The places the model may read unasked, and write under `auto`** — as
//! rules the gate already reads, so no second notion of a place exists.

use std::path::PathBuf;

use super::{ClaudeReads, Roots, Session, State, claude_reads, locate};
use crate::config::{MemoryConfig, MemoryMode, SkillsConfig};
use crate::permission::{Origin, Rule};

/// **The rules memory and skills put in force in a `session`** — and what
/// could not be one.
///
/// - Every memory and skill location, and the write location:
///   `allow :read <place>` (a directory holds what is below it), so the model reads a memory or
///   a skill's supporting files without asking — reads stay inside the
///   approved places (Steven, 10-05), and these are approved by your config.
///   Claude Code's memory and skill locations too, where the session reads
///   its files (`[memory] claude`).
/// - Under `mode = auto`, the session's write location's vouched spellings
///   (`write`, from `scratch::prepare_owned`, for [`super::write_place`]):
///   `allow :write <dir>` — writes there run unasked, a delete still
///   asks. Under `ask` or `off`, none: writes there ask as any write outside
///   does.
/// - What you disabled (`[memory] disabled`, `[skills] disabled`): `deny
///   :read`, whatever approves its place.
///
/// A placed location counts whether it exists yet or not; a relative one
/// where it is found, as a walk names a place in every directory up to the
/// root. Each is [`Origin::Knowledge`], so `:rules` lists it apart and
/// `:unallow` takes it out like any other — the rules of the session in
/// front, so a Claude Code session's are its own. The credential check overrides
/// every allow: what of `~/.claude` is not carved out of it
/// (`[secrets] paths_except`) is still asked or refused.
///
/// A path the rule grammar cannot spell — a space splits a rule, a wildcard
/// would grant more than the place — is left out, and the sentence says
/// reads there ask.
pub fn rules(
    memory: &MemoryConfig,
    skills: &SkillsConfig,
    roots: &Roots,
    write: &[String],
    session: Session,
) -> (Vec<Rule>, Vec<String>) {
    let mut reads: Vec<PathBuf> = Vec::new();
    let mut said = Vec::new();
    let repository = locate::repository(roots);
    let mut add = |entry: &str| {
        let relative = locate::is_relative(entry);
        for place in locate::places(entry, roots) {
            if relative && !place.exists() || reads.contains(&place) {
                continue;
            }
            // A repository's place that is a link out of it approves no reads
            // there: the gate reads the link's spelling, and the repository
            // chose where it leads (Reviewer E1, round 27).
            let real = super::resolved(&place);
            if locate::leaves_the_repository(entry, &real, &repository) {
                said.push(format!(
                    "`{}` leads out of the repository, to {} — reads there ask",
                    place.display(),
                    real.display()
                ));
                continue;
            }
            reads.push(place);
        }
    };
    for entry in memory.paths.iter().chain(&skills.paths) {
        add(entry);
    }
    if claude_reads(memory.claude, session) != ClaudeReads::Nobody {
        for entry in [super::CLAUDE_MEMORY].iter().chain(&super::CLAUDE_SKILLS) {
            add(entry);
        }
    }
    // Shannon's write place, read in every session. Claude Code's, where it
    // is the session's (`super::write_place`), is `{claude-memory}` above —
    // added exactly when that answer is `ClaudeCode`, so asking it here added
    // nothing (Reviewer C, round 28).
    if let Some(place) = locate::write_place(&memory.write, roots)
        && !reads.contains(&place)
    {
        reads.push(place);
    }
    let mut lines: Vec<String> = Vec::new();
    let mut spell = |path: &str, line: String| match spellable(path) {
        true => lines.push(line),
        false => said.push(format!(
            "`{path}` cannot be written as a rule (a space or a wildcard) — reads there ask"
        )),
    };
    // A place holds what is below it, a file or a directory alike, so one
    // spelling names either — and a place not there yet, whichever it
    // becomes. (`DIR/**` differs only for a delete — in what it names and
    // in how specific it is, `Resolved::within_as` — which none of these
    // is: a user's `allow :read DIR/**` ties a disabled place's deny, and
    // the deny stands.)
    for place in &reads {
        let path = place.display().to_string();
        let path = path.trim_end_matches('/');
        spell(path, format!("allow :read {path}"));
    }
    if memory.mode == MemoryMode::Auto {
        for dir in write {
            let dir = dir.trim_end_matches('/');
            spell(dir, format!("allow :write {dir}"));
        }
    }
    // What you disabled is REFUSED, whatever approves its place (Steven,
    // 10-06: "block the file"; a deny beats every allow): each place named
    // that is there, and each file the finding marked — a skill by its
    // directory, as its supporting files are its own. Built from `find`'s
    // marks, so the screen's `disabled` and the gate are one answer; no
    // credential judge there, since a refusal of one costs nothing. Nothing
    // disabled, nothing to find: a rebuild scanned every memory's head for
    // no rule at all (review 29, Reviewer B).
    if !super::DisabledSet::of(memory, skills, roots).is_empty() {
        let found = super::find(memory, skills, roots, session, &|_| None);
        let mut blocked: Vec<PathBuf> = found.disabled.places().cloned().collect();
        for item in found.items.iter().filter(|i| i.state == State::Disabled) {
            let place = match item.path.file_name().is_some_and(|n| n == "SKILL.md") {
                true => item.path.parent().unwrap_or(&item.path).to_path_buf(),
                false => item.path.clone(),
            };
            if !blocked.iter().any(|b| place.starts_with(b)) {
                blocked.push(place);
            }
        }
        for place in &blocked {
            let path = place
                .display()
                .to_string()
                .trim_end_matches('/')
                .to_string();
            if !spellable(&path) {
                said.push(format!(
                    "`{path}` cannot be written as a rule (a space or a wildcard) — disabled, \
                     it is not told or listed, but a read of it is not refused"
                ));
                continue;
            }
            // A place not there yet is refused as either, as it may come as
            // a file or a directory (review 29: it was readable until a
            // rebuild) — which the one spelling is.
            lines.push(format!("deny :read {path}"));
        }
    }
    let rules = lines
        .iter()
        // Path rules (`allow :read <dir>`), which no program model reads —
        // and `RuleSet::configured_by` reads every rule again by the models
        // in force.
        .filter_map(|line| {
            Rule::read_by(
                Origin::Knowledge,
                line,
                crate::permission::model::Models::shipped(),
            )?
            .ok()
        })
        .collect();
    (rules, said)
}

/// Can `path` stand in a rule as itself?
/// (A brace is refused by the rule grammar: admitted here, a disabled place
/// spelled with one was dropped at `Rule::read_by` and never said — review
/// of e10b6d5, A.)
fn spellable(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(char::is_whitespace)
        && !path.contains(['*', '?', '[', '#', '{', '}'])
}
