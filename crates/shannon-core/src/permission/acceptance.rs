//! **The acceptance suite** (`acceptance.toml`): what the rules must decide,
//! by the goals, run against the engine as it stands.
//!
//! A case the engine decides as the goals call for HOLDS. One it does not
//! carries a `gap`, saying why — and the test fails when a gap closes
//! without its marker going (so the file stays the measurement) or a case
//! without one stops holding (a regression). A case today's grammar cannot
//! spell has no `rules`, and is counted, not run.
//!
//! **Held-out models** ([`FIXTURES`]): a case that `needs` a model nothing
//! ships (`model:bat`) is run with that model loaded through the door a
//! user's own file takes (`model::load::parse`, `Source::User`). Two things
//! are proved at once: that a person can model a program and add it, and
//! that the GRAMMAR says what the goal asks once the facts are there — the
//! rules of such a case are written in the grammar only, never in the model
//! (Steven, 10-10). The unmodelled cases (`make`) are the other half of the
//! held-out set: the language must hold with no model at all.

use std::sync::Arc;

use super::model::{Models, Source, load};
use super::{By, Decision, RuleSet, Verdict};

/// Models the suite loads as a user would, and nothing ships.
const FIXTURES: &[(&str, &str)] = &[
    (
        "bat",
        r#"
name = "bat"
effect = "read"
trust = "author"
family = { bundling = true, interspersed = true }
positional = [{ at = "rest", role = "read_file" }]
flag = [
  { names = ["-p", "--plain"] },
  { names = ["-n", "--number"] },
  { names = ["-l", "--language"], arity = 1, role = "data" },
  { names = ["--pager"], arity = 1, role = "program" },
  { names = ["-P"] },
]
"#,
    ),
    (
        "mkdir",
        r#"
name = "mkdir"
effect = "write"
trust = "author"
family = { bundling = true }
positional = [{ at = "rest", role = "write_file" }]
flag = [
  { names = ["-p", "--parents"] },
  { names = ["-v", "--verbose"] },
  { names = ["-m", "--mode"], arity = 1, role = "data" },
]
"#,
    ),
];

/// The shipped models with the held-out ones a case needs added.
fn models_for(case: &Case) -> Result<Option<Arc<Models>>, String> {
    let needed: Vec<&str> = case
        .needs
        .iter()
        .filter_map(|n| n.strip_prefix("model:"))
        .filter(|m| Models::shipped().get(m).is_none())
        .collect();
    if needed.is_empty() {
        return Ok(None);
    }
    let mut models = Models::shipped().clone();
    for name in needed {
        let (_, text) = FIXTURES
            .iter()
            .find(|(n, _)| *n == name)
            .ok_or_else(|| format!("needs `model:{name}`, which neither ships nor is a fixture"))?;
        let model = load::parse(text, Source::User(format!("{name}.toml")))
            .map_err(|e| format!("fixture `{name}`: {e}"))?;
        models.insert(model);
    }
    Ok(Some(Arc::new(models)))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Suite {
    pub(super) defaults: Defaults,
    #[serde(rename = "case")]
    pub(super) cases: Vec<Case>,
}

impl Suite {
    /// The suite as checked in.
    pub(super) fn load() -> Suite {
        toml::from_str(include_str!("acceptance.toml")).expect("acceptance.toml")
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Defaults {
    pub(super) start: String,
    pub(super) base: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Case {
    pub(super) id: String,
    pub(super) goals: Vec<String>,
    pub(super) intent: String,
    /// The policy in today's grammar; absent where it cannot be spelled.
    pub(super) rules: Option<Vec<String>>,
    /// What the verdict assumes exists beyond the grammar (`model:gh`).
    #[serde(default)]
    pub(super) needs: Vec<String>,
    pub(super) command: String,
    pub(super) expect: String,
    pub(super) gap: Option<String>,
    /// With a `gap`, what the engine decides TODAY — `allow`, `ask`, `deny`,
    /// or `unspellable` where its rules cannot be said — checked, so a gap
    /// that moves (an ask that now allows) fails as a regression does. The
    /// gap's own verdict went unread: a case expecting `deny` could slide
    /// from `ask` to `allow` with the suite green (review stop 12, T8 H2).
    pub(super) today: Option<String>,
    /// `false` for an `allow` or `deny` case whose verdict the shipped rules
    /// and models give already — said, because otherwise its rules must make
    /// the difference: 12 allow cases held with no rule at all, the G3
    /// "a person can model a program" cases among them (T8 H3). Checked
    /// both ways: a `false` whose rules do matter is stale.
    #[serde(default = "rules_matter")]
    pub(super) rules_matter: bool,
    /// A case that holds for a reason other than the one the goals give
    /// (an unmodelled program asks about everything), said so — and checked:
    /// a `note` needs a [`by`](Self::by). Unread, twelve said a reason and
    /// nine of them went stale (review stop 12, T8 M7).
    pub(super) note: Option<String>,
    /// **What decides the verdict** — `rule`, `guard`, `default[:reason]` or
    /// `hard[:kind]`, checked against the acts that decided it
    /// ([`decided_by`]).
    pub(super) by: Option<String>,
    pub(super) start: Option<String>,
    pub(super) base: Option<String>,
}

fn rules_matter() -> bool {
    true
}

/// How one case came out.
#[derive(Debug, PartialEq)]
enum Outcome {
    Holds,
    /// Not run: today's grammar cannot spell its policy.
    Unspellable,
    /// The engine decided otherwise, as the case's `today` says, and why.
    Gap(Decision),
}

/// The case's starting rules — its base, its start, the models it needs —
/// before its own.
fn base_rules(suite: &Suite, case: &Case) -> Result<RuleSet, String> {
    let base = case.base.as_deref().unwrap_or(&suite.defaults.base);
    let mut rs = match base {
        "defaults" => RuleSet::defaults(),
        "empty" => RuleSet::empty(),
        other => return Err(format!("unknown base `{other}`")),
    }
    .with_start(case.start.as_deref().unwrap_or(&suite.defaults.start));
    if let Some(models) = models_for(case)? {
        rs = rs.with_models(models);
    }
    Ok(rs)
}

fn word(decision: Decision) -> &'static str {
    match decision {
        Decision::Allow => "allow",
        Decision::Ask => "ask",
        Decision::Deny => "deny",
    }
}

fn run(suite: &Suite, case: &Case) -> Result<Outcome, String> {
    let today = match (&case.gap, case.today.as_deref()) {
        (None, None) => None,
        (Some(_), Some(today)) => Some(today),
        (Some(_), None) => return Err("has a `gap` and no `today` saying where it stands".into()),
        (None, Some(_)) => return Err("says `today` with no `gap`".into()),
    };
    let unspellable = today == Some("unspellable");
    let Some(rules) = &case.rules else {
        return match unspellable {
            true => Ok(Outcome::Unspellable),
            false => Err("has no `rules`, and no gap with `today = \"unspellable\"`".into()),
        };
    };
    let mut rs = base_rules(suite, case)?;
    if let Err(why) = rs.extend_from_lines(&rules.join("\n")) {
        // A policy the grammar refuses is a gap of the grammar's, to be
        // said in the case.
        return match unspellable {
            true => Ok(Outcome::Unspellable),
            false => Err(format!(
                "its rules do not parse, and no gap says `today = \"unspellable\"`: {why}"
            )),
        };
    }
    let verdict = rs.decide_line(&case.command);
    match (&case.note, &case.by) {
        (Some(_), None) => return Err("has a `note` and no `by` checking it".into()),
        (_, Some(by)) => decided_by(&verdict, by)?,
        (None, None) => {}
    }
    let got = verdict.decision;
    let holds = match case.expect.as_str() {
        "allow" => got == Decision::Allow,
        "ask" => got == Decision::Ask,
        "deny" => got == Decision::Deny,
        "not-allow" => got != Decision::Allow,
        other => return Err(format!("unknown verdict `{other}`")),
    };
    match (holds, today) {
        (true, None) => {
            the_rules_matter(suite, case, rules, got)?;
            Ok(Outcome::Holds)
        }
        (false, Some(today)) if today == word(got) => Ok(Outcome::Gap(got)),
        (false, Some(today)) => Err(format!(
            "the gap moved: `today` says {today}, the engine says {}",
            word(got)
        )),
        (true, Some(_)) => Err(format!(
            "HOLDS now, but is marked as a gap — take the marker off: {}",
            case.gap.as_deref().unwrap_or_default()
        )),
        (false, None) => Err(format!(
            "expected {} for `{}` and got {} — a regression, or a gap to mark",
            case.expect,
            case.command,
            word(got)
        )),
    }
}

/// **What decided a verdict, as a case says it** (`by`): some act whose own
/// decision is the verdict, held for that reason — `rule`, a rule of the
/// verdict's own action cited, no guard; `uncovered`, an allow cited that
/// did not reach the act (an unmodelled prefix stopping at a word); `guard`,
/// a shipped guard cited; `default`, no rule spoke, `default:<reason>`
/// naming it (`outside`, `unmodelled`, `effect`, …); `hard`, what nothing
/// lifts, `hard:<kind>` naming it (`code`, `unsound`, …). The reason is
/// matched by the start of its name, lowercased.
fn decided_by(v: &Verdict, by: &str) -> Result<(), String> {
    let (kind, detail) = by.split_once(':').map_or((by, None), |(k, d)| (k, Some(d)));
    if !["rule", "uncovered", "guard", "default", "hard"].contains(&kind) {
        return Err(format!("unknown `by` kind `{kind}`"));
    }
    let named = |why: String| detail.is_none_or(|d| why.to_lowercase().starts_with(d));
    let deciding: Vec<&By> = (v.stages().iter())
        .flat_map(|s| &s.acts)
        .filter(|a| a.decision == v.decision)
        .map(|a| &a.by)
        .collect();
    let held = deciding.iter().any(|by| match (kind, by) {
        ("rule", By::Rules(cited)) => {
            (cited.iter()).any(|c| c.guard.is_none() && c.action == v.decision)
        }
        ("uncovered", By::Rules(cited)) => {
            v.decision != Decision::Allow && cited.iter().any(|c| c.action == Decision::Allow)
        }
        ("guard", By::Rules(cited)) => cited.iter().any(|c| c.guard.is_some()),
        ("default", By::Default(why)) => named(format!("{why:?}")),
        ("hard", By::Hard(why)) => named(format!("{why:?}")),
        _ => false,
    });
    match held {
        true => Ok(()),
        false => Err(format!(
            "says `by = \"{by}\"`, and is decided by {deciding:?}"
        )),
    }
}

/// **An `allow` case's allows (a `deny` case's denies) make its verdict** —
/// without them, the engine decides otherwise. Not asked of a case with no
/// rule of its verdict's kind (its point is that the others do not reach
/// it), nor of one saying `rules_matter = false`, which is checked the other
/// way: a marker on rules that do make the difference is stale (T8 H3).
fn the_rules_matter(
    suite: &Suite,
    case: &Case,
    rules: &[String],
    got: Decision,
) -> Result<(), String> {
    let verb = match case.expect.as_str() {
        "allow" => "allow ",
        "deny" => "deny ",
        _ => return Ok(()),
    };
    let others: Vec<&str> = (rules.iter().map(String::as_str))
        .filter(|rule| !rule.trim_start().starts_with(verb))
        .collect();
    if others.len() == rules.len() {
        return match case.rules_matter {
            true => Ok(()),
            false => Err(format!(
                "says `rules_matter = false`, and has no `{verb}` rule"
            )),
        };
    }
    let mut without = base_rules(suite, case)?;
    without
        .extend_from_lines(&others.join("\n"))
        .map_err(|why| format!("its other rules do not parse alone: {why}"))?;
    let without = without.decide_line(&case.command).decision;
    match (case.rules_matter, without == got) {
        (true, true) => Err(format!(
            "holds with none of its `{verb}` rules ({} without them): make them the \
             difference, or say `rules_matter = false`",
            word(without)
        )),
        (false, false) => Err(format!(
            "says `rules_matter = false`, but its `{verb}` rules make it {} ({} without)",
            word(got),
            word(without)
        )),
        _ => Ok(()),
    }
}

/// Every case is spelled right, and stands where the file says it stands.
#[test]
fn the_acceptance_suite_stands_where_it_says() {
    let suite = Suite::load();
    let mut ids = std::collections::HashSet::new();
    let (mut holds, mut gaps, mut unspellable) = (0, Vec::new(), Vec::new());
    let mut problems = Vec::new();
    for case in &suite.cases {
        assert!(ids.insert(&case.id), "case `{}` twice", case.id);
        assert!(
            !case.goals.is_empty() && !case.intent.is_empty(),
            "{}",
            case.id
        );
        match run(&suite, case) {
            Ok(Outcome::Holds) => holds += 1,
            Ok(Outcome::Gap(got)) => gaps.push((case, got)),
            Ok(Outcome::Unspellable) => unspellable.push(case),
            Err(why) => problems.push(format!("{}: {why}", case.id)),
        }
    }
    // The measurement, for `--nocapture`.
    eprintln!(
        "acceptance: {} cases — {holds} hold, {} gaps, {} unspellable today",
        suite.cases.len(),
        gaps.len(),
        unspellable.len()
    );
    for (case, got) in &gaps {
        eprintln!(
            "  gap   {:<48} expected {:<9} got {:<6} — {}{}",
            case.id,
            case.expect,
            format!("{got:?}").to_lowercase(),
            case.gap.as_deref().unwrap_or(""),
            needs(case)
        );
    }
    for case in &unspellable {
        eprintln!(
            "  unspellable {:<42} — {}{}",
            case.id,
            case.gap.as_deref().unwrap_or(""),
            needs(case)
        );
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// **The harness itself, arm by arm** — over a suite written here, because
/// the checked-in one has no gap, no `not-allow`, no `base` or `start` of
/// its own, so those arms ran on nothing (review stop 12, T8 H2). Each case
/// is named for what the harness must say of it.
#[test]
fn every_arm_of_the_harness_says_what_it_should() {
    let suite: Suite = toml::from_str(
        r#"
[defaults]
start = "/home/me/proj"
base = "defaults"

[[case]]
id = "holds"
goals = ["G4"]
intent = "a deny on a flag"
rules = ["deny find:-delete"]
command = "find . -delete"
expect = "deny"

[[case]]
id = "a-gap-that-stands"
goals = ["G4"]
intent = "x"
rules = ["allow ls *"]
command = "ls -la /etc"
expect = "allow"
gap = "outside the approved places it asks"
today = "ask"

[[case]]
id = "a-gap-that-moved"
goals = ["G4"]
intent = "x"
rules = ["allow ls *"]
command = "ls -la /etc"
expect = "allow"
gap = "outside the approved places it asks"
today = "deny"

[[case]]
id = "a-gap-that-closed"
goals = ["G4"]
intent = "x"
rules = ["allow ls *"]
command = "ls -la /etc"
expect = "ask"
gap = "x"
today = "ask"

[[case]]
id = "a-regression"
goals = ["G4"]
intent = "x"
rules = ["allow ls *"]
command = "ls -la /etc"
expect = "allow"

[[case]]
id = "unspellable-without-rules"
goals = ["G4"]
intent = "x"
command = "ls"
expect = "allow"
gap = "the grammar cannot say it"
today = "unspellable"

[[case]]
id = "no-rules-and-no-gap"
goals = ["G4"]
intent = "x"
command = "ls"
expect = "allow"

[[case]]
id = "rules-that-do-not-parse-said"
goals = ["G4"]
intent = "x"
rules = ["frobnicate ls"]
command = "ls"
expect = "allow"
gap = "no such verb yet"
today = "unspellable"

[[case]]
id = "rules-that-do-not-parse-unsaid"
goals = ["G4"]
intent = "x"
rules = ["frobnicate ls"]
command = "ls"
expect = "allow"

[[case]]
id = "a-gap-without-today"
goals = ["G4"]
intent = "x"
rules = ["allow ls *"]
command = "ls -la /etc"
expect = "allow"
gap = "x"

[[case]]
id = "today-without-a-gap"
goals = ["G4"]
intent = "x"
rules = ["allow ls *"]
command = "ls -la /etc"
expect = "ask"
today = "ask"

[[case]]
id = "an-unknown-base"
goals = ["G4"]
intent = "x"
rules = []
command = "ls"
expect = "allow"
base = "everything"

[[case]]
id = "an-unknown-verdict"
goals = ["G4"]
intent = "x"
rules = []
command = "ls"
expect = "maybe"

[[case]]
id = "not-allow-holds-on-an-ask"
goals = ["G4"]
intent = "x"
rules = ["allow ls *"]
command = "ls -la /etc"
expect = "not-allow"

[[case]]
id = "rules-that-decide-nothing"
goals = ["G4"]
intent = "x"
rules = ["allow ls *"]
command = "ls -la src"
expect = "allow"

[[case]]
id = "rules-that-decide-nothing-said"
goals = ["G4"]
intent = "x"
rules = ["allow ls *"]
command = "ls -la src"
expect = "allow"
rules_matter = false

[[case]]
id = "a-stale-rules-matter"
goals = ["G4"]
intent = "x"
rules = ["allow ls *"]
command = "ls --hyperlink=always src"
expect = "allow"
rules_matter = false

[[case]]
id = "the-shipped-deny"
goals = ["G4"]
intent = "x"
rules = []
command = "rm --no-preserve-root -rf /x"
expect = "deny"

[[case]]
id = "an-empty-base-has-no-shipped-deny"
goals = ["G4"]
intent = "x"
rules = []
command = "rm --no-preserve-root -rf /x"
expect = "ask"
base = "empty"

[[case]]
id = "outside-the-default-start"
goals = ["G4"]
intent = "x"
rules = []
command = "cat /srv/notes.txt"
expect = "ask"

[[case]]
id = "inside-its-own-start"
goals = ["G4"]
intent = "x"
rules = []
command = "cat /srv/notes.txt"
expect = "allow"
start = "/srv"

[[case]]
id = "a-note-without-by"
goals = ["G4"]
intent = "x"
rules = []
command = "cat /srv/notes.txt"
expect = "ask"
note = "holds because it is outside"

[[case]]
id = "by-its-reason"
goals = ["G4"]
intent = "x"
rules = []
command = "cat /srv/notes.txt"
expect = "ask"
by = "default:outside"

[[case]]
id = "by-another-reason"
goals = ["G4"]
intent = "x"
rules = []
command = "cat /srv/notes.txt"
expect = "ask"
by = "hard:code"

[[case]]
id = "by-an-unknown-kind"
goals = ["G4"]
intent = "x"
rules = []
command = "cat /srv/notes.txt"
expect = "ask"
by = "magic"

[[case]]
id = "by-its-rule"
goals = ["G4"]
intent = "x"
rules = ["deny find:-delete"]
command = "find . -delete"
expect = "deny"
by = "rule"

[[case]]
id = "by-an-allow-that-does-not-reach"
goals = ["G4"]
intent = "x"
rules = ["allow make test"]
command = "make test CC=cc"
expect = "ask"
by = "uncovered"

[[case]]
id = "an-allow-not-reaching-is-no-rule-of-the-ask"
goals = ["G4"]
intent = "x"
rules = ["allow make test"]
command = "make test CC=cc"
expect = "ask"
by = "rule"
"#,
    )
    .expect("the suite here");
    let outcome = |id: &str| {
        let case = suite.cases.iter().find(|c| c.id == id).expect(id);
        run(&suite, case)
    };
    let said = |id: &str, words: &str| match outcome(id) {
        Err(why) => assert!(why.contains(words), "{id}: {why}"),
        other => panic!("{id}: {other:?}, not an error saying `{words}`"),
    };
    for holds in [
        "holds",
        "not-allow-holds-on-an-ask",
        "rules-that-decide-nothing-said",
        "the-shipped-deny",
        "an-empty-base-has-no-shipped-deny",
        "outside-the-default-start",
        "inside-its-own-start",
        "by-its-reason",
        "by-its-rule",
        "by-an-allow-that-does-not-reach",
    ] {
        assert_eq!(outcome(holds), Ok(Outcome::Holds), "{holds}");
    }
    // `by`, and a `note` that needs one (review stop 12, T8 M7).
    said("a-note-without-by", "has a `note` and no `by`");
    said("by-another-reason", "says `by = \"hard:code\"`");
    said("by-an-unknown-kind", "unknown `by` kind");
    said(
        "an-allow-not-reaching-is-no-rule-of-the-ask",
        "says `by = \"rule\"`",
    );
    assert_eq!(
        outcome("a-gap-that-stands"),
        Ok(Outcome::Gap(Decision::Ask))
    );
    for unspellable in ["unspellable-without-rules", "rules-that-do-not-parse-said"] {
        assert_eq!(
            outcome(unspellable),
            Ok(Outcome::Unspellable),
            "{unspellable}"
        );
    }
    said("a-gap-that-moved", "the gap moved");
    said("a-gap-that-closed", "HOLDS now");
    said("a-regression", "a regression");
    said("no-rules-and-no-gap", "has no `rules`");
    said("rules-that-do-not-parse-unsaid", "do not parse");
    said("a-gap-without-today", "no `today`");
    said("today-without-a-gap", "with no `gap`");
    said("an-unknown-base", "unknown base");
    said("an-unknown-verdict", "unknown verdict");
    said("rules-that-decide-nothing", "holds with none of its");
    said("a-stale-rules-matter", "but its `allow ` rules");
}

fn needs(case: &Case) -> String {
    if case.needs.is_empty() {
        String::new()
    } else {
        format!(" [needs {}]", case.needs.join(", "))
    }
}
