//! **A user's own program models** (P6b): a directory of model files over
//! the shipped set, which the rules and the commands are both read by.

use super::*;
use crate::config::PermissionsConfig;
use model::{Models, Source, USER_DIR};

const START: &str = "/work/me/proj";

/// A model of `bat`, which ships none: its operands are files it reads,
/// `-l` takes a value.
const BAT: &str = r#"name = "bat"
effect = "read"
family = { bundling = true, interspersed = true }
examples.match = ["bat -p notes.txt", "bat -l rust notes.txt"]
examples.not_match = ["bat --frob notes.txt"]
positional = [{ at = "rest", role = "read_file" }]
flag = [
  { names = ["-p", "--plain"] },
  { names = ["-l", "--language"], arity = 1, role = "data" },
]
"#;

/// A user's directory holding `files`, loaded over the shipped set.
fn user_models(files: &[(&str, &str)]) -> (Arc<Models>, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let (models, notes) = Models::with_user_dir(dir.path());
    (Arc::new(models), notes)
}

fn permissions(rules: &[&str]) -> PermissionsConfig {
    PermissionsConfig {
        rules: rules.iter().map(|r| r.to_string()).collect(),
        ..Default::default()
    }
}

/// Decide `line` under `rules`, every rule read by `models` (`None`: the
/// shipped set), as the app builds the policy.
fn decide_by(rules: &[&str], models: Option<Arc<Models>>, line: &str) -> Verdict {
    let (rs, problems) = RuleSet::configured_by(&permissions(rules), &[], &[], models);
    assert!(problems.is_empty(), "{problems:?}");
    rs.with_start(START).decide_line(line)
}

/// **A user's model reads the rules and the commands alike**: a rule's flag
/// resolves by it (every spelling), and its operands are files at the
/// operation it says — so Steven's `bat` example holds once `bat` is
/// modelled: `allow bat -p` covers `bat foo.txt -p`, and the place is the
/// path rules'.
#[test]
fn a_users_model_reads_the_rules_and_the_commands() {
    assert!(
        Models::shipped().get("bat").is_none(),
        "premise: none ships"
    );
    let (models, notes) = user_models(&[("bat.toml", BAT)]);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(
        models.get("bat").unwrap().source,
        Source::User("bat.toml".into())
    );
    let deny = ["allow bat *", "deny bat --language"];
    let line = "bat -l rust notes.txt";
    assert_eq!(
        decide_by(&deny, None, line).decision,
        Decision::Allow,
        "premise: unmodelled, `--language` is matched only as written"
    );
    assert_eq!(
        decide_by(&deny, Some(models.clone()), line).decision,
        Decision::Deny
    );
    // The rule itself is read by the model: `rust` is `--language`'s value
    // (the model's arity), where unmodelled it was a subcommand word.
    let value = ["allow bat *", "deny bat --language rust"];
    assert_eq!(
        decide_by(&value, None, line).decision,
        Decision::Allow,
        "premise"
    );
    assert_eq!(
        decide_by(&value, Some(models.clone()), line).decision,
        Decision::Deny
    );
    let read = ["allow bat -p", "allow :read /srv/x"];
    assert_eq!(
        decide_by(&read, None, "bat -p /srv/x/a").decision,
        Decision::Ask,
        "premise: an unmodelled word is at no operation, which `:read` does not grant"
    );
    for line in ["bat -p /srv/x/a", "bat /srv/x/a -p"] {
        assert_eq!(
            decide_by(&read, Some(models.clone()), line).decision,
            Decision::Allow,
            "{line}"
        );
    }
    assert_eq!(
        decide_by(&read, Some(models.clone()), "bat -p /srv/y/a").decision,
        Decision::Ask,
        "the place is the path rules'"
    );
}

/// **A completion model backs no allow** (`trust = "completion"`, imported
/// from a completion spec: what the words are, not what they do). With no
/// rule its call asks; its places are at no operation, so a `:read` grant
/// does not cover them; and a user's own allow still runs it, its flags
/// read by the model.
#[test]
fn a_completion_model_backs_no_allow() {
    let completion = BAT.replace(
        "effect = \"read\"",
        "effect = \"read\"\ntrust = \"completion\"",
    );
    let (verified, _) = user_models(&[("bat.toml", BAT)]);
    let (models, notes) = user_models(&[("bat.toml", &completion)]);
    assert!(notes.is_empty(), "{notes:?}");
    let program = |v: &Verdict| {
        acts_of(v)
            .into_iter()
            .find(|a| matches!(a.act, ActKind::Program { .. }))
            .unwrap()
            .clone()
    };
    let line = "bat -p notes.txt";
    assert_eq!(
        decide_by(&[], Some(verified.clone()), line).decision,
        Decision::Allow,
        "premise: an author's model reads it by default"
    );
    let v = decide_by(&[], Some(models.clone()), line);
    assert_eq!(v.decision, Decision::Ask);
    assert_eq!(program(&v).by, By::Default(NoRule::Completion));
    // An allow covers the flags it names: none rides past it.
    assert_eq!(
        decide_by(&["allow bat"], Some(verified.clone()), line).decision,
        Decision::Allow,
        "premise: an author's harmless flag rides on a read"
    );
    assert_eq!(
        decide_by(&["allow bat"], Some(models.clone()), line).decision,
        Decision::Ask
    );
    // So `A` names the flag the call used.
    let (rs, _) = RuleSet::configured_by(&permissions(&[]), &[], &[], Some(models.clone()));
    assert_eq!(
        rs.with_start(START).proposed_allows(line),
        Ok(vec!["bat -p".to_string()])
    );
    let read = ["allow bat -p", "allow :read /srv/x"];
    assert_eq!(
        decide_by(&read, Some(verified), "bat -p /srv/x/a").decision,
        Decision::Allow,
        "premise: an author's read role meets a read grant"
    );
    assert_eq!(
        decide_by(&read, Some(models.clone()), "bat -p /srv/x/a").decision,
        Decision::Ask
    );
    let any = ["allow bat -p", "allow :* /srv/x"];
    assert_eq!(
        decide_by(&any, Some(models.clone()), "bat -p /srv/x/a").decision,
        Decision::Allow
    );
    let deny = ["allow bat *", "deny bat --language"];
    assert_eq!(
        decide_by(&deny, Some(models), "bat -l rust notes.txt").decision,
        Decision::Deny,
        "it still reads the words: a deny meets every spelling"
    );
}

/// **A completion model hides no place and no subcommand** (review
/// 2026-10-11, E1): its roles are a guess, so a word it calls data is
/// placed as an unmodelled program's is, and a read node it declares does
/// not ride on an allow. The author's same model shows what the guess alone
/// would do.
#[test]
fn a_completion_model_hides_no_place_and_no_subcommand() {
    let guess = BAT.replace("role = \"read_file\"", "role = \"data\"")
        + "[command.cache]\neffect = \"read\"\n";
    let completion = guess.replace(
        "effect = \"read\"\nfamily",
        "effect = \"read\"\ntrust = \"completion\"\nfamily",
    );
    let (author, _) = user_models(&[("bat.toml", &guess)]);
    let (models, notes) = user_models(&[("bat.toml", &completion)]);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(
        models.get("bat").unwrap().trust,
        model::Trust::Completion,
        "premise"
    );
    let fenced = ["allow bat *", "deny :* /srv/keep"];
    for (rules, line, want) in [
        (&fenced[..], "bat /srv/keep/a", Decision::Deny),
        (&["allow bat *"][..], "bat .env", Decision::Ask),
        (&["allow bat"][..], "bat cache", Decision::Ask),
    ] {
        assert_eq!(
            decide_by(rules, Some(author.clone()), line).decision,
            Decision::Allow,
            "premise: the author's guess alone lets {line} run"
        );
        assert_eq!(
            decide_by(rules, Some(models.clone()), line).decision,
            want,
            "{line}"
        );
    }
}

/// **A user's model adds caution, never takes Shannon's away** (Steven,
/// 10-11): privilege, code a program runs, a program or file it runs, a
/// credential it prints are the union of the user's reading and Shannon's
/// (review 2026-10-11, E2: a user `sudo.toml` without `privilege` and an
/// `eval.toml` of a read each ran unasked).
#[test]
fn a_users_model_adds_caution_never_takes_it_away() {
    let files = [
        (
            "sudo.toml",
            "name = \"sudo\"\neffect = \"read\"\npositional = [{ at = \"rest\", role = \"argv\" }]\n",
        ),
        (
            "eval.toml",
            "name = \"eval\"\neffect = \"read\"\npositional = [{ at = \"rest\", role = \"data\" }]\n",
        ),
        (
            "bash.toml",
            "name = \"bash\"\neffect = \"read\"\npositional = [{ at = \"rest\", role = \"data\" }]\n",
        ),
        (
            "gh.toml",
            "name = \"gh\"\neffect = \"read\"\n[command.\"auth token\"]\neffect = \"read\"\n",
        ),
        (
            "find.toml",
            "name = \"find\"\neffect = \"read\"\npositional = [{ at = \"rest\", role = \"data\" }]\n",
        ),
        (
            "awk.toml",
            "name = \"awk\"\neffect = \"read\"\npositional = [{ at = \"rest\", role = \"data\" }]\n",
        ),
    ];
    let (models, notes) = user_models(&files);
    assert!(notes.is_empty(), "{notes:?}");
    for (file, _) in &files {
        let name = file.trim_end_matches(".toml");
        assert!(
            matches!(models.get(name).unwrap().source, Source::User(_)),
            "premise: {name} is the user's"
        );
    }
    for (rules, line) in [
        (&[][..], "sudo cat notes.txt"),
        (&["allow cat *"][..], "sudo cat notes.txt"),
        (&[][..], "eval 'rm -rf src'"),
        (&[][..], "bash -c 'rm src/x'"),
        (&[][..], "gh auth token"),
        (&["allow find *"][..], "find src -exec rm {} ;"),
        // Shannon's awk is a read whose program text it reads as code: the
        // code, not the effect, is what asks.
        (&[][..], "awk 'BEGIN { system(\"rm src/x\") }'"),
    ] {
        let v = decide_by(rules, Some(models.clone()), line);
        assert_eq!(v.decision, Decision::Ask, "{line} under {rules:?}: {v:#?}");
    }
    // And a user's model may add the mark: their `cat` raises privilege.
    let (models, _) = user_models(&[(
        "cat.toml",
        "name = \"cat\"\neffect = \"read\"\nprivilege = true\n\
         positional = [{ at = \"rest\", role = \"read_file\" }]\n",
    )]);
    assert_eq!(
        decide_by(&["allow cat *"], Some(models), "cat notes.txt").decision,
        Decision::Ask
    );
}

/// **Every rule is read by the models in force, at every door** (review
/// 2026-10-11, A and C: each of these doors read by the shipped set with the
/// suite green): the shipped rules, a session rule's new reading, and `A`'s
/// check that its proposal covers the call.
#[test]
fn every_door_reads_rules_by_the_users_models() {
    // The shipped `deny rm --no-preserve-root`, read again by a user's `rm`
    // whose first name for the flag is another: still met (C's probe).
    let (models, _) = user_models(&[(
        "rm.toml",
        "name = \"rm\"\neffect = \"delete\"\nfamily = { bundling = true }\n\
         positional = [{ at = \"rest\", role = \"delete_file\" }]\n\
         flag = [{ names = [\"-r\"] }, { names = [\"-f\"] }, \
         { names = [\"--npr\", \"--no-preserve-root\"] }]\n",
    )]);
    let rules = ["allow rm *", "allow :delete build"];
    assert_eq!(
        decide_by(&rules, Some(models.clone()), "rm -rf build").decision,
        Decision::Allow,
        "premise: the call runs"
    );
    assert_eq!(
        decide_by(&rules, Some(models), "rm -rf --no-preserve-root build").decision,
        Decision::Deny
    );

    // A session rule typed before the model, read again by it: its flag is
    // the model's, every spelling (`-l` for `--language`).
    let (models, _) = user_models(&[("bat.toml", BAT)]);
    let typed = Rule::read(Origin::Session, "deny bat --language rust")
        .unwrap()
        .unwrap();
    let (rs, problems) = RuleSet::configured_by(
        &permissions(&["allow bat *"]),
        &[typed],
        &[],
        Some(models.clone()),
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        rs.with_start(START)
            .decide_line("bat -l rust notes.txt")
            .decision,
        Decision::Deny
    );

    // A rule removed this session, read by the models in force as what it
    // is compared with: removed as `--language` while `bat` was unmodelled,
    // it is the config's `-l` once a model says they are one flag (review
    // 2026-10-11, A: compared as read, a removal matched nothing and the
    // rule came back).
    let removed = Rule::read(Origin::Config, "deny bat --language rust")
        .unwrap()
        .unwrap();
    let (rs, _) = RuleSet::configured_by(
        &permissions(&["allow bat *", "deny bat -l rust"]),
        &[],
        &[removed],
        Some(models.clone()),
    );
    assert!(
        !rs.rules().iter().any(|r| r.source == "deny bat -l rust"),
        "removed"
    );

    // `A`: a writer the user models, `-l` taking a value — the proposal
    // names it, and is held to cover the call, by the same model.
    let (models, _) = user_models(&[(
        "mytool.toml",
        "name = \"mytool\"\neffect = \"write\"\n\
         positional = [{ at = \"rest\", role = \"data\" }]\n\
         flag = [{ names = [\"-l\"], arity = 1, role = \"data\" }]\n",
    )]);
    let (rs, _) = RuleSet::configured_by(&permissions(&[]), &[], &[], Some(models));
    assert_eq!(
        rs.with_start(START).proposed_allows("mytool -l rust"),
        Ok(vec!["mytool -l rust".to_string()])
    );
}

#[test]
fn a_users_model_says_it_lists_and_a_listing_reaches_no_guard_below() {
    // Steven's `lsd`, written to list (10-11).
    let lsd = r#"
name = "lsd"
effect = "read"
reads_here = "list"
positional = [{ at = "rest", role = "list" }]
flag = [{ names = ["-a", "--all"] }]

[command.cat]
reads_here = true

[command.tree]
"#;
    let (models, notes) = user_models(&[("lsd.toml", lsd)]);
    assert!(notes.is_empty(), "{notes:?}");
    let key = format!("{START}/keys/.k.api");
    // `allow lsd *`: so what asks is the guard, never a missing rule.
    let rs = |models: Option<Arc<Models>>| {
        let rules = permissions(&["allow lsd *"]);
        let (rs, problems) = RuleSet::configured_by(&rules, &[], &[], models);
        assert!(problems.is_empty(), "{problems:?}");
        rs.with_start(START)
            .with_own_state(vec![key.clone()])
            .with_path_policy(crate::PathPolicy::Deny)
    };
    let listed = rs(Some(models.clone()));
    for line in ["lsd", "lsd -a", "lsd -a keys", "lsd tree"] {
        assert_eq!(listed.decide_line(line).decision, Decision::Allow, "{line}");
    }
    // A subcommand that says it searches reaches below; named outright, a
    // listing meets the guard; and unmodelled, `lsd` may do anything.
    assert_ne!(listed.decide_line("lsd cat").decision, Decision::Allow);
    assert_eq!(
        listed.decide_line("lsd keys/.k.api").decision,
        Decision::Deny
    );
    assert_ne!(rs(None).decide_line("lsd -a").decision, Decision::Allow);
    // Imported from completions, a model says what words are, not that a
    // call only lists.
    let guessed = lsd.replace(
        "effect = \"read\"",
        "effect = \"read\"\ntrust = \"completion\"",
    );
    let (models, _) = user_models(&[("lsd.toml", &guessed)]);
    assert_ne!(
        rs(Some(models)).decide_line("lsd -a keys").decision,
        Decision::Allow
    );

    // Where it runs is read as `dir` or `list`, nothing else.
    let bad = "name = \"x\"\nreads_here = \"pattern\"\n";
    let why = model::load::parse(bad, Source::User("x.toml".into())).unwrap_err();
    assert!(why.contains("`dir` (searched) or `list` (listed)"), "{why}");
}

/// **The directory's load is bounded by its work, not its count** (review
/// 2026-10-11, B): a chain past [`model::MOST_CHAIN_WORDS`] is refused
/// before a tree is built (20,000 words overflowed the stack at launch);
/// files past [`model::MOST_USER_FILES`], bytes past
/// [`model::MOST_USER_BYTES`] and entries past [`model::MOST_ENTRIES`] are
/// said and not read; notes are at most [`model::MOST_NOTES`], and quote
/// a user's text cut short. (It sat on the `lsd` test above: review stop
/// 12, T8.)
#[test]
fn a_users_model_directory_is_bounded_by_its_work() {
    let chain = |n: usize| {
        format!(
            "name = \"deep\"\n[command.\"{}\"]\neffect = \"read\"\n",
            vec!["a"; n].join(" ")
        )
    };
    let parse = |t: &str| model::load::parse(t, Source::User("deep.toml".into()));
    assert!(
        parse(&chain(model::MOST_CHAIN_WORDS)).is_ok(),
        "at the bound"
    );
    assert!(parse(&chain(model::MOST_CHAIN_WORDS + 1)).is_err());
    assert!(
        parse(&chain(20_000)).is_err(),
        "and refused before it is built"
    );
    let examples = |n: usize, len: usize| {
        let line = format!("deep {}", "a".repeat(len.saturating_sub(5)));
        format!("name = \"deep\"\nexamples.match = {:?}\n", vec![line; n])
    };
    let most = model::MOST_EXAMPLE_BYTES;
    assert!(
        parse(&examples(model::MOST_EXAMPLES, most)).is_ok(),
        "at the bounds"
    );
    assert!(parse(&examples(model::MOST_EXAMPLES + 1, 10)).is_err());
    assert!(parse(&examples(1, most + 1)).is_err());

    let file = |i: usize| (format!("m{i:03}.toml"), format!("name = \"m{i:03}\"\n"));
    let files: Vec<(String, String)> = (0..=model::MOST_USER_FILES).map(file).collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let (models, notes) = user_models(&refs);
    assert!(models.get("m000").is_some());
    let last = format!("m{:03}", model::MOST_USER_FILES);
    assert!(models.get(&last).is_none(), "past the count");
    assert!(
        notes.iter().any(|n| n.contains("only the first")),
        "{notes:?}"
    );

    let broken: Vec<(String, String)> = (0..40)
        .map(|i| (format!("b{i:02}.toml"), "name = \n".to_string()))
        .collect();
    let refs: Vec<(&str, &str)> = broken
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let (_, notes) = user_models(&refs);
    assert_eq!(notes.len(), model::MOST_NOTES, "{notes:?}");
    assert!(notes.last().unwrap().contains("and 9 more"), "{notes:?}");

    let pad = format!("#{}\n", "x".repeat(model::MOST_USER_BYTES / 3));
    let big = |n: &str| (format!("{n}.toml"), format!("{pad}name = \"{n}\"\n"));
    let files = [big("p1"), big("p2"), big("p3"), big("p4")];
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let (models, notes) = user_models(&refs);
    assert!(
        models.get("p1").is_some() && models.get("p4").is_none(),
        "past the bytes"
    );
    assert!(
        notes.iter().any(|n| n.contains("nor any after it")),
        "{notes:?}"
    );

    let dir = tempfile::tempdir().unwrap();
    for i in 0..=model::MOST_ENTRIES {
        std::fs::write(dir.path().join(format!("e{i}.txt")), "").unwrap();
    }
    let (_, notes) = Models::with_user_dir(dir.path());
    assert!(notes.iter().any(|n| n.contains("entries")), "{notes:?}");

    let long = format!("bat --frob {}", "a".repeat(500));
    let text = BAT.replace(
        "examples.match = [",
        &format!("examples.match = [{long:?}, "),
    );
    let (_, notes) = user_models(&[("bat.toml", &text)]);
    let note = notes
        .iter()
        .find(|n| n.contains("bat --frob"))
        .expect("said");
    assert!(note.len() < 300 && note.contains('…'), "{note}");
}

/// **An example is held to the reader both ways** (review 2026-10-11, C:
/// only `match` was tested): a `not_match` line that reads in full is said,
/// and so is a line that is not one command of the program.
#[test]
fn a_models_examples_are_held_both_ways() {
    let wrong = BAT.replace(
        "examples.not_match = [\"bat --frob notes.txt\"]",
        "examples.not_match = [\"bat -p notes.txt\", \"bat -p a.txt | bat\"]",
    );
    let (models, notes) = user_models(&[("bat.toml", &wrong)]);
    assert!(models.get("bat").is_some(), "loaded all the same");
    let said = |what: &str| notes.iter().any(|n| n.contains(what));
    assert!(said("should leave a word unknown"), "{notes:?}");
    assert!(said("is not one command"), "{notes:?}");
}

/// **An attached short value is read only for a flag that takes one**
/// (review 2026-10-11, B): getopt's reading of `-Ic` for a zero-arity `-I`
/// is two flags, and `c` is not the model's — the cluster stays unknown and
/// asks. A fixture, so the premise cannot rot with a shipped model.
#[test]
fn an_attached_value_is_read_only_for_a_flag_that_takes_one() {
    let (models, notes) = user_models(&[(
        "fz.toml",
        "name = \"fz\"\neffect = \"read\"\nfamily = { bundling = false }\n\
         positional = [{ at = \"rest\", role = \"exec_file\" }]\n\
         flag = [{ names = [\"-I\"] }, { names = [\"-o\"], arity = 1, role = \"data\" }]\n",
    )]);
    assert!(notes.is_empty(), "{notes:?}");
    let rules = ["allow fz -I", "allow :exec /work/me/proj/**"];
    assert_eq!(
        decide_by(&rules, Some(models.clone()), "fz -I x").decision,
        Decision::Allow,
        "premise"
    );
    assert_eq!(
        decide_by(&rules, Some(models.clone()), "fz -Ic x").decision,
        Decision::Ask
    );
    let rules = ["allow fz -o", "allow :exec /work/me/proj/**"];
    assert_eq!(
        decide_by(&rules, Some(models), "fz -oout x").decision,
        Decision::Allow,
        "a flag that takes one takes it attached"
    );
}

/// The program acts of a verdict's first stage, for a test that reads why.
fn acts_of(v: &Verdict) -> Vec<&ActVerdict> {
    v.stages().iter().flat_map(|s| s.acts.iter()).collect()
}

/// **The directory says what it could not honour**: a file that does not
/// parse, one named after another program than it models (so `cat.toml`
/// cannot quietly replace a model), an example its model misreads (loaded
/// all the same); a shipped model is replaced whole; a file that is not
/// `.toml` is not read, and a missing directory is the shipped set.
#[test]
fn a_users_model_directory_says_what_it_could_not_honour() {
    let (models, notes) = user_models(&[
        ("cat.toml", BAT),
        ("broken.toml", "name = \n"),
        (
            "rm.toml",
            "name = \"rm\"\neffect = \"delete\"\nexamples.match = [\"rm -z x\"]\n",
        ),
        ("notes.txt", "name = \"ls\"\n"),
    ]);
    let said = |file: &str, what: &str| {
        notes
            .iter()
            .any(|n| n.starts_with(&format!("{USER_DIR}/{file}:")) && n.contains(what))
    };
    assert!(models.get("bat").is_none(), "named after another program");
    assert!(said("cat.toml", "it models `bat`"), "{notes:#?}");
    assert!(said("broken.toml", "not loaded"), "{notes:#?}");
    let rm = models.get("rm").unwrap();
    assert_eq!(rm.source, Source::User("rm.toml".into()));
    assert!(rm.root.flags.is_empty(), "replaced whole, not merged");
    assert!(said("rm.toml", "`rm -z x`"), "{notes:#?}");
    assert_eq!(notes.len(), 3, "{notes:#?}");
    assert_eq!(
        models.get("ls").unwrap().source,
        Source::Shipped,
        "a `.txt` is no model"
    );
    let dir = tempfile::tempdir().unwrap();
    let (models, notes) = Models::with_user_dir(&dir.path().join("absent"));
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(models.len(), Models::shipped().len());
}

/// **A rule only the user's model refuses is said, never in force**: a
/// value on a flag their model says takes none — read by the shipped set
/// (`bat` unmodelled) it parses, so `Config::review` cannot see it; and a
/// session rule typed before the model loaded is read again.
#[test]
fn a_rule_only_the_users_model_refuses_is_said() {
    let (models, _) = user_models(&[("bat.toml", BAT)]);
    let p = permissions(&["allow bat -p=x"]);
    let (_, by_shipped) = RuleSet::configured(&p, &[], &[]);
    assert!(by_shipped.is_empty(), "premise: {by_shipped:?}");
    let (rs, problems) = RuleSet::configured_by(&p, &[], &[], Some(models.clone()));
    assert!(
        problems.iter().any(|p| p.contains("takes no value")),
        "{problems:?}"
    );
    assert_eq!(rs.unhonoured(), problems);
    assert!(!rs.rules().iter().any(|r| r.source == "allow bat -p=x"));
    let typed = Rule::read(Origin::Session, "allow bat -p=x")
        .unwrap()
        .unwrap();
    let (rs, problems) =
        RuleSet::configured_by(&permissions(&[]), &[typed], &[], Some(models.clone()));
    assert!(
        problems
            .iter()
            .any(|p| p.contains("the session's rule") && p.contains("takes no value")),
        "{problems:?}"
    );
    assert!(!rs.rules().iter().any(|r| r.source == "allow bat -p=x"));
}

/// **A project's models say what words are, and allow nothing by
/// themselves** (Steven, 10-13): each loads at completion trust, whatever it
/// says, so `bat notes.txt` — a read under the same model of the user's —
/// asks, and a rule naming the program still runs it. One may replace a
/// model Shannon ships, as the user's may (Steven, 10-13: barred "No"), and
/// Shannon's facts stand under it: a flat `find.toml` reads `-exec rm {} \;`
/// as words, and Shannon's reading still finds the `rm` it runs.
#[test]
fn a_projects_models_allow_nothing_by_themselves() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bat.toml"), BAT).unwrap();
    std::fs::write(
        dir.path().join("find.toml"),
        "name = \"find\"\neffect = \"read\"\ntrust = \"verified\"\n\
         positional = [{ at = \"rest\", role = \"data\" }]\n",
    )
    .unwrap();
    let (models, notes) = Models::shipped().clone().with_project_dir(dir.path());
    assert!(notes.is_empty(), "{notes:?}");
    let find = models.get("find").expect("loaded");
    assert!(
        matches!(&find.source, Source::Project(f) if f == "find.toml"),
        "it replaces Shannon's"
    );
    assert_eq!(find.trust, model::Trust::Completion, "whatever it says");
    let bat = models.get("bat").expect("loaded");
    assert_eq!(bat.trust, model::Trust::Completion);
    assert!(matches!(&bat.source, Source::Project(f) if f == "bat.toml"));
    let models = Some(Arc::new(models));
    let (user, _) = user_models(&[("bat.toml", BAT)]);
    assert_eq!(
        decide_by(&[], Some(user), "bat notes.txt").decision,
        Decision::Allow,
        "premise: the user's model reads it"
    );
    assert_eq!(
        decide_by(&[], models.clone(), "bat notes.txt").decision,
        Decision::Ask
    );
    assert_eq!(
        decide_by(&["allow bat *"], models.clone(), "bat notes.txt").decision,
        Decision::Allow
    );
    assert_eq!(
        decide_by(&["allow find *"], models, "find . -exec rm {} \\;").decision,
        Decision::Ask,
        "Shannon's `-exec` stands under it"
    );
}

/// **Every trust word a project file can write loads as `completion`** —
/// the force is one overwrite (`Models::load_dir`), and the test above fed
/// it only `verified`: a force that spared `generated` (which decides as
/// `verified`) passed every test (review stop 13, B 2, mutated). The words
/// come from the type: the list and an exhaustive `match` are one spelling,
/// so a new `Trust` variant does not compile until it is here.
#[test]
fn a_project_model_is_completion_whatever_trust_it_writes() {
    macro_rules! every_trust {
        ($($v:ident),+) => {{
            #[deny(unreachable_patterns)]
            #[allow(dead_code)]
            fn every(of: model::Trust) {
                match of {
                    $(model::Trust::$v => {})+
                }
            }
            [$(model::Trust::$v),+]
        }};
    }
    for trust in every_trust!(Completion, Generated, Author, Verified) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("bat.toml"),
            format!(
                "name = \"bat\"\neffect = \"read\"\ntrust = \"{}\"\n",
                trust.name()
            ),
        )
        .unwrap();
        let (models, notes) = Models::shipped().clone().with_project_dir(dir.path());
        assert!(notes.is_empty(), "{trust:?}: {notes:?}");
        let bat = models.get("bat").expect("loaded");
        assert!(
            matches!(&bat.source, Source::Project(_)),
            "premise: the project's"
        );
        assert_eq!(
            bat.trust,
            model::Trust::Completion,
            "written {:?}",
            trust.name()
        );
    }
}
