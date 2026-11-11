//! The model format, held to itself and to the directory of files.

use std::path::{Path, PathBuf};

use super::load::{canon_of, parse};
use super::*;

/// The models written by hand (P1 of the rebuild, 2026-10-09): find, gh,
/// git, sed, awk, uv, curl, cargo, rg, fd, python, python3, xargs, timeout.
/// Every other shipped model was generated from the registry this format
/// replaced (`shell::program::PROGRAMS`, deleted in P2) and is a seed to be
/// verified and extended by hand, so it says `trust = "generated"` — the
/// word the pane shows beside a verdict no rule made. All 79 said
/// `verified` until review stop 12 (T8; Steven approved Q13, 10-27). A model
/// checked by hand moves here, and its file says `verified`:
/// `a_shipped_model_is_verified_exactly_when_written_by_hand`.
const HAND_WRITTEN: &[&str] = &[
    "awk", "cargo", "curl", "fd", "find", "gh", "git", "python", "python3", "rg", "sed", "timeout",
    "uv", "xargs",
];

fn models_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("models")
}

/// The file a program's model lives in: its name, but for the two names a
/// file cannot wear well (`.` would be hidden; `[` reads as a glob).
fn file_stem(program: &str) -> String {
    match program {
        "." => "dot".to_string(),
        "[" => "bracket".to_string(),
        other => other.to_string(),
    }
}

/// The models the files under `models/` hold, by the name each file says.
fn models_on_disk() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(models_dir())
        .expect("models/ exists")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".toml"))
        .map(|e| {
            let file = e.file_name().to_string_lossy().to_string();
            let text = std::fs::read_to_string(e.path()).expect("read a model file");
            let model = parse(&text, Source::Shipped).unwrap_or_else(|why| panic!("{file}: {why}"));
            (file, model.name)
        })
        .collect();
    out.sort();
    out
}

/// **Every file under `models/` is compiled in, and every compiled-in name
/// has its file** — a model added without its `include_str!` row is a test
/// failure, not a silently missing program.
#[test]
fn every_model_file_is_shipped() {
    let on_disk = models_on_disk();
    let mut disk_names: Vec<String> = on_disk.iter().map(|(_, n)| n.clone()).collect();
    disk_names.sort();
    let mut shipped: Vec<String> = SHIPPED_FILES.iter().map(|(n, _)| n.to_string()).collect();
    shipped.sort();
    assert_eq!(disk_names, shipped, "models/ and SHIPPED_FILES disagree");
    for (file, name) in &on_disk {
        assert_eq!(
            *file,
            format!("{}.toml", file_stem(name)),
            "the model `{name}` lives in `{file}`"
        );
    }
    // And each row's text is the file's: a row pointing at the wrong file
    // would ship one model under another's name.
    for (name, text) in SHIPPED_FILES {
        let model = parse(text, Source::Shipped).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            model.name, *name,
            "the row `{name}` holds the model `{}`",
            model.name
        );
    }
}

/// **The shipped set parses, once, and knows its programs by basename.**
#[test]
fn the_shipped_models_parse_and_resolve_by_basename() {
    let models = Models::shipped();
    assert_eq!(models.len(), SHIPPED_FILES.len());
    assert!(models.get("find").is_some());
    assert!(
        models.get("/usr/bin/find").is_some(),
        "a path resolves to its basename's model"
    );
    assert!(models.get("frobnicate").is_none());
    assert!(
        models.can_escalate("frobnicate"),
        "an unmodelled program is assumed escalatable"
    );
    assert!(models.can_escalate("find"), "find escalates by -exec");
    assert_eq!(models.canonical_of("find", "-delete"), Some("delete"));
    assert_eq!(
        models.canonical_of("find", "-rm"),
        Some("delete"),
        "bfs's spelling"
    );
    assert_eq!(
        models.canonical_of("gh", "-X"),
        Some("method"),
        "a subcommand's flag is found from the root"
    );
}

/// **A flag's canon defaults to its long spelling without dashes, else its
/// first spelling** — what a rule names.
#[test]
fn a_flags_canon_defaults_to_its_long_spelling() {
    let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(canon_of(&s(&["-n", "--line-number"])), "line-number");
    assert_eq!(canon_of(&s(&["-n"])), "n");
    assert_eq!(canon_of(&s(&["-delete", "-rm"])), "delete");
    assert_eq!(canon_of(&s(&["--pre"])), "pre");
}

/// **A subcommand chain builds the parents it does not declare**, as the
/// parent's shadow.
#[test]
fn a_subcommand_chain_builds_its_parents() {
    let m = parse(
        "name = \"x\"\neffect = \"write\"\n[command.\"pr view\"]\neffect = \"read\"\nflag = [{ names = [\"--web\"] }]\n",
        Source::Shipped,
    )
    .unwrap();
    let pr = m.root.sub("pr").expect("pr made along the chain");
    assert_eq!(
        pr.effect,
        Effect::Write,
        "the shadow takes the parent's effect"
    );
    let view = pr.sub("view").expect("view declared");
    assert_eq!(view.effect, Effect::Read);
    assert_eq!(view.flags[0].canon, "web");
    assert_eq!(m.canonical_of("--web"), Some("web"));
}

/// **What a model may not say is refused with the reason**, never read as
/// a model that says nothing.
#[test]
fn the_loader_refuses_what_a_model_may_not_say() {
    let refused = |text: &str, needle: &str| {
        let why = parse(text, Source::Shipped).expect_err(&format!("should refuse: {text}"));
        assert!(why.contains(needle), "{text:?}: {why}");
    };
    refused("effect = \"read\"\n", "`name` is required");
    refused("name = \"x\"\neffcet = \"read\"\n", "effcet");
    refused(
        "name = \"x\"\nflag = [{ names = [\"-a\"], canon = \"one\" }, { names = [\"-a\"], canon = \"two\" }]\n",
        "spells both",
    );
    refused(
        "name = \"x\"\nflag = [{ names = [\"-a\"] }, { names = [\"-b\"], canon = \"a\" }]\n",
        "two flags named",
    );
    refused(
        "name = \"x\"\npositional = [{ at = 0, unless = [\"ghost\"] }]\n",
        "ghost",
    );
    refused(
        "name = \"x\"\nflag = [{ names = [\"-a\"], benign = false }]\n",
        "says `why`",
    );
    refused(
        "name = \"x\"\nflag = [{ names = [\"-a\"], arity = 2 }]\n",
        "arity 2",
    );
    refused(
        "name = \"x\"\nflag = [{ names = [\"-a\"], arity = \"many\" }]\n",
        "arity `many`",
    );
    refused(
        "name = \"x\"\nflag = [{ names = [\"a\"] }]\n",
        "begin with a dash",
    );
    refused(
        "name = \"x\"\nflag = [{ names = [\"--a=b\"] }]\n",
        "holds `=`",
    );
    refused(
        "name = \"x\"\nflag = [{ names = [\"-a\"], implies = { b = \"1\", c = \"2\" } }]\n",
        "one fact",
    );
    refused(
        "name = \"x\"\nflag = [{ names = [\"-a\"], implies = { ghost = \"1\" } }]\n",
        "ghost",
    );
    refused(
        "name = \"x\"\nexamples.match = [\"y -a\"]\n",
        "does not begin with `x`",
    );
    refused(
        "name = \"x\"\nswitch = [{ when = \"ghost\", from = \"read_file\", to = \"write_file\" }]\n",
        "ghost",
    );
    refused(
        "name = \"x\"\nfamily = { numeric_option = \"ghost\" }\n",
        "ghost",
    );
    refused(
        "name = \"x\"\n[command.a]\neffect = \"read\"\n[command.b]\naliases = [\"a\"]\n",
        "two subcommands answer to `a`",
    );
    refused(
        "name = \"x\"\npositional = [{ at = \"first\" }]\n",
        "not an index",
    );
    refused("name = \"x/y\"\n", "a basename");
}

/// **The loader never panics**, whatever the file holds: a user's model is
/// untrusted text.
#[test]
fn the_loader_never_panics() {
    use proptest::prelude::*;
    proptest!(ProptestConfig::with_cases(256), |(text in "[ -~\\n]{0,200}")| {
        let _ = parse(&text, Source::Shipped);
    });
    // And the shapes a fuzzer is slow to find: every key with a wrong type.
    for text in [
        "name = 1\n",
        "name = \"x\"\nflag = 1\n",
        "name = \"x\"\nflag = [1]\n",
        "name = \"x\"\nflag = [{ names = \"-a\" }]\n",
        "name = \"x\"\nflag = [{ names = [\"-a\"], arity = { until = \";\" } }]\n",
        "name = \"x\"\nfamily = 1\n",
        "name = \"x\"\n[command]\n",
        "name = \"x\"\n[command.a]\nflag = { names = [\"-a\"] }\n",
        "name = \"x\"\nexamples = [\"x\"]\n",
    ] {
        let _ = parse(text, Source::Shipped);
    }
}

/// **Every hand-written model carries examples** that the reader will hold
/// it to (P3), and every example names the program.
#[test]
fn every_hand_written_model_carries_examples() {
    let shipped = Models::shipped();
    for name in HAND_WRITTEN {
        let m = shipped
            .get(name)
            .unwrap_or_else(|| panic!("{name} shipped"));
        assert!(!m.examples.matches.is_empty(), "{name}: no examples.match");
        assert!(
            !m.examples.not_matches.is_empty(),
            "{name}: no examples.not_match"
        );
        assert_eq!(
            m.trust,
            Trust::Verified,
            "{name}: a shipped hand-written model is verified"
        );
    }
}

/// **A shipped model says `verified` exactly when it was written by hand** —
/// and every other says `generated`, the truth the pane tells (review stop
/// 12, Q13). Both ways, so a hand-written model relabelled and a generated
/// one promoted without joining `HAND_WRITTEN` are each a failure.
#[test]
fn a_shipped_model_is_verified_exactly_when_written_by_hand() {
    let shipped = Models::shipped();
    let wrong: Vec<String> = shipped
        .iter()
        .filter(|m| m.source == Source::Shipped)
        .filter_map(|m| {
            let want = if HAND_WRITTEN.contains(&m.name.as_str()) {
                Trust::Verified
            } else {
                Trust::Generated
            };
            (m.trust != want).then(|| format!("{}: {:?}, want {want:?}", m.name, m.trust))
        })
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
    // The subject: both labels are in the set this read.
    let generated = shipped
        .iter()
        .filter(|m| m.trust == Trust::Generated)
        .count();
    assert_eq!(generated, 65, "the generated models, counted 10-27");
}

/// **Every shipped model reads its own examples as they say**: a `match`
/// line in full, a `not_match` line with a word unknown — the claim the
/// examples make, held by the reader (`acts::example_misreads`), which a
/// user's file is held to at load too.
#[test]
fn every_shipped_model_reads_its_examples() {
    let shipped = Models::shipped();
    let misread: Vec<String> = shipped
        .iter()
        .flat_map(|m| crate::permission::acts::example_misreads(m, shipped))
        .collect();
    assert!(misread.is_empty(), "{misread:#?}");
    // The subject: the hand-written models carry examples, so this read
    // some of each kind.
    let lines: usize = shipped
        .iter()
        .map(|m| m.examples.matches.len() + m.examples.not_matches.len())
        .sum();
    assert!(lines > 2 * HAND_WRITTEN.len(), "{lines}");
}

/// **The language readers are reached through the model's roles**: sed's
/// and awk's scripts are code the readers scan — which the flat view today's
/// engine reads cannot do, so it presents them as executors
/// (`shell::program::lookup`).
#[test]
fn sed_and_awk_scripts_are_code() {
    let shipped = Models::shipped();
    let sed = shipped.get("sed").unwrap();
    assert_eq!(sed.root.positionals[0].role, Role::Code);
    assert_eq!(sed.root.positionals[0].unless, ["e", "f"]);
    assert_eq!(sed.switches.len(), 2);
    let awk = shipped.get("awk").unwrap();
    assert_eq!(awk.root.positionals[0].role, Role::Code);
    for code in ["sed", "awk"] {
        let flat = crate::shell::program::lookup(code).unwrap_or_else(|| panic!("{code} is flat"));
        assert_eq!(
            flat.base,
            crate::shell::Effect::Executor,
            "{code}: an executor to the flat view"
        );
    }
}
