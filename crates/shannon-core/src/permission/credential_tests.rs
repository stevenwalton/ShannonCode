//! **What a credential path is, asked of the gate** — the specification the
//! old `secrets::sensitive_path` held, every case through
//! [`RuleSet::credential_path`], the door memory, skills and every command's
//! words are judged by. That matcher was a second encoding of `[secrets]`
//! that nothing in the gate called, and the two disagreed about whose home
//! `~/` names; it was deleted, its tests moved here (review stop 12, Q1).
//!
//! **`~/` is YOUR home** (Steven, 10-27: "your home. Other homes should be
//! implicitly covered … because they are further back"): another home is
//! outside where Shannon started, and a read there asks by default. The
//! matcher anchored `~/` at anyone's.

use super::*;

/// The gate, started in a project, with `[secrets]` as `cfg` says.
fn gate(cfg: crate::SecretsConfig) -> RuleSet {
    RuleSet::defaults()
        .with_start("/home/me/proj")
        .with_secrets(cfg)
}

fn held(path: &str) -> bool {
    gate(crate::SecretsConfig::default())
        .credential_path(path)
        .is_some()
}

fn home() -> &'static str {
    crate::secrets::home_dir().expect("tests run with a HOME")
}

#[test]
fn credential_paths_are_recognised() {
    let home = home();
    for path in [
        "~/.ssh/id_ed25519".to_string(),
        format!("{home}/.ssh/config"),
        ".env".into(),
        ".env.local".into(),
        "config/.env.production".into(),
        "certs/server.pem".into(),
        "backup.kdbx".into(),
        "secrets.gpg".into(),
        "~/.aws/credentials".into(),
        "~/.config/gh/hosts.yml".into(),
        "/home/x/.netrc".into(),
        "deploy/id_rsa".into(),
    ] {
        assert!(held(&path), "should be a credential path: {path}");
    }
}

/// The counterpart, and the one that decides whether this is usable: an
/// ordinary path must not trip it. A guard that fires on `src/main.rs`
/// becomes a guard people switch off.
#[test]
fn ordinary_paths_are_not_flagged() {
    for path in [
        "src/main.rs",
        "README.md",
        "crates/shannon-core/src/secrets.rs",
        "/usr/bin/env",
        "my.ssh.notes",
        "environment.yml",
        "docs/keynote.md",
        "target/debug/shannon",
    ] {
        assert!(!held(path), "false positive on: {path}");
    }
}

/// **A `~/` entry is anchored at your home**, however spelled, and never
/// meets a segment of the same name deeper in a path: a project's
/// `.claude/worktrees/…` is not `~/.claude` (Steven, 10-04). Another home is
/// not yours (Q1): it is outside where Shannon started.
#[test]
fn a_home_entry_is_anchored_at_your_home() {
    let home = home();
    for path in [
        "~/.claude/settings.json".to_string(),
        format!("{home}/.claude/x"),
        format!("{home}/.CLAUDE/x"),
    ] {
        assert!(held(&path), "{path}");
    }
    let parent = std::path::Path::new(home).parent().unwrap().display();
    for path in [
        format!("{home}/proj/.claude/worktrees/a/src/main.rs"),
        format!("{home}/proj/.claude/worktrees/a"),
        ".claude/settings.json".to_string(),
        "/tmp/.claude/x".to_string(),
        format!("{parent}/other/.claude/x"),
        "/root/.claude/x".to_string(),
    ] {
        assert!(!held(&path), "{path}");
    }
}

/// **A carve-out lifts what holds the whole place, and nothing else**
/// (Steven, 10-06: Claude Code's memories, refused whole by `~/.claude`).
/// A file entry inside still holds; `..` out of the place is the file it
/// names; the rest of `~/.claude` is a credential path still; `[]` puts it
/// all back.
#[test]
fn a_carve_out_lifts_only_what_holds_the_whole_place() {
    let home = home();
    let memory = format!("{home}/.claude/projects/-Users-me-proj/memory");
    for free in [
        format!("{memory}/MEMORY.md"),
        format!("{memory}/user_likes_green.md"),
        memory.clone(),
        "~/.claude/projects/x/memory/a.md".to_string(),
        "~/.claude/CLAUDE.md".to_string(),
        "~/.claude/skills/review/SKILL.md".to_string(),
    ] {
        assert!(!held(&free), "{free}");
    }
    for kept in [
        // Claude Code's global config, beside `~/.claude`: an API key.
        "~/.claude.json".to_string(),
        format!("{home}/.claude.json"),
        // And its copies, the same key.
        "~/.claude.json.backup".to_string(),
        format!("{home}/.claude.json.backup.1759800000000"),
        // A file entry names a file in the place: it holds.
        format!("{memory}/deploy.pem"),
        format!("{memory}/credentials"),
        // Out of the place, by `..`: the file it names.
        format!("{memory}/../../../.credentials.json"),
        "~/.claude/skills/../settings.json".to_string(),
        // The rest of `~/.claude`.
        "~/.claude/.credentials.json".to_string(),
        "~/.claude/projects/x/session.jsonl".to_string(),
        "~/.claude/projects/x/memoryish/a.md".to_string(),
        "~/.claude/CLAUDE.md.bak".to_string(),
    ] {
        assert!(held(&kept), "{kept}");
    }
    // `[]`: all of `~/.claude` again.
    let none = gate(crate::SecretsConfig {
        paths_except: Vec::new(),
        ..Default::default()
    });
    assert!(
        none.credential_path(&format!("{memory}/MEMORY.md"))
            .is_some()
    );
    // A user's own carve-out, out of their own entry — and only theirs: a
    // `.pem` in the carved place is still one.
    let theirs = gate(crate::SecretsConfig {
        paths: vec!["~/vault".into()],
        paths_except: vec!["~/vault/public".into()],
        ..Default::default()
    });
    assert!(theirs.credential_path("~/vault/public/readme").is_none());
    assert!(theirs.credential_path("~/vault/private/key").is_some());
    assert!(theirs.credential_path("~/vault/public/k.pem").is_some());
    // And as a command reads it, the same door: every exception lifted every
    // guard, so a key file in a carved place read unasked (review stop 12,
    // found porting these tests).
    let rs = gate(Default::default()).with_start(home);
    for line in [
        format!("cat {memory}/deploy.pem"),
        format!("cat {memory}/id_rsa"),
        format!("cat {memory}/.env"),
    ] {
        assert_ne!(rs.decide_line(&line).decision, Decision::Allow, "{line}");
    }
    assert_eq!(
        rs.decide_line(&format!("cat {memory}/MEMORY.md")).decision,
        Decision::Allow,
        "premise: the carved place itself reads"
    );
}

/// **The carve-out reads a bounded part of a word, and fails closed past
/// it** — counted in places, as a work bound must be (a 16 KB
/// `~/.claude/a/a/…/x` was seconds: Reviewer B, round 28): a carved place
/// further down than the bound is never found, and what is in it is held.
/// One within it is carved, however deep the file below it.
#[test]
fn the_carve_out_reads_a_bounded_part_of_a_word() {
    assert!(!held(&format!("~/.claude/skills/{}x", "a/".repeat(8))));
    assert!(!held(&format!("~/.claude/skills/{}x", "a/".repeat(4000))));
    assert!(held(&format!("~/.claude/{}x", "a/".repeat(8000))));
    let deep = format!("~/vault/{}pub", "a/".repeat(40));
    let carve = |except: &str| {
        gate(crate::SecretsConfig {
            paths: vec!["~/vault".into()],
            paths_except: vec![except.to_string()],
            ..Default::default()
        })
    };
    assert!(
        carve("~/vault/a/pub")
            .credential_path("~/vault/a/pub/x")
            .is_none(),
        "premise: a place within the bound is carved"
    );
    assert!(
        carve(&deep).credential_path(&format!("{deep}/x")).is_some(),
        "past the bound: held"
    );
}

/// Secrets live wherever the user put them; the shipped list is a seed.
#[test]
fn users_can_add_their_own_credential_paths() {
    let rs = gate(crate::SecretsConfig {
        paths: vec!["~/work/vault".to_string(), "*.age".to_string()],
        ..Default::default()
    });
    assert!(rs.credential_path("~/work/vault/prod").is_some());
    assert!(rs.credential_path("notes.age").is_some());
    // And the seed still applies alongside them.
    assert!(rs.credential_path("~/.ssh/id_rsa").is_some());
}

/// **An entry is a bash glob** (Steven, 10-06: "treat it like you'd
/// expect in a bash description") — and Steven's own two,
/// `/srv/keys/*.api` and `/srv/keys/**`, protected nothing once: every
/// wildcard but a name's lone leading or trailing `*` compared as itself.
#[test]
fn an_entry_is_read_as_a_bash_glob() {
    let home = home();
    // Extensions no shipped entry names, so only the entry can answer.
    let holds = |entry: &str, path: &str| {
        gate(crate::SecretsConfig {
            paths: vec![entry.to_string()],
            ..Default::default()
        })
        .credential_path(path)
        .is_some()
    };
    assert!(holds("/srv/keys/*.vlt", "/srv/keys/prod.vlt"));
    assert!(holds("/srv/keys/*.vlt", "/SRV/Keys/PROD.VLT"), "case folds");
    assert!(!holds("/srv/keys/*.vlt", "/srv/keys/notes.txt"));
    assert!(
        !holds("/srv/keys/*.vlt", "/srv/keys/sub/x.vlt"),
        "`*` stays in its segment"
    );
    assert!(holds("/srv/keys/**", "/srv/keys/a/b/c.txt"));
    assert!(holds("/srv/keys/**", "/srv/keys"), "and the place itself");
    assert!(!holds("/srv/keys/**", "/srv/keysmith/x"));
    assert!(
        holds("/srv/**/*.vlt", "/srv/x.vlt"),
        "`**/` is zero or more directories"
    );
    assert!(holds("/srv/**/*.vlt", "/srv/a/b/x.vlt"));
    assert!(
        holds("/srv/keys/*", "/srv/keys/team/x.txt"),
        "a path holds what is below it"
    );
    // `?`, a class, braces — bash's.
    assert!(holds("/srv/k?y/*.vlt", "/srv/key/a.vlt"));
    assert!(holds("/srv/[ab]*/x.vlt", "/srv/beta/x.vlt"));
    assert!(!holds("/srv/[ab]*/x.vlt", "/srv/gamma/x.vlt"));
    assert!(!holds("/srv/[!ab]*/x.vlt", "/srv/beta/x.vlt"));
    assert!(holds("/srv/keys/*.{vlt,box}", "/srv/keys/a.box"));
    assert!(!holds("/srv/keys/*.{vlt,box}", "/srv/keys/a.txt"));
    // At your home, in either spelling, and only there — a plain entry and
    // one the rule line cannot spell (`**`, a brace: read as the dialect's
    // regex, `glob_entry_regex`) alike. The regex anchored `~/` at ANY
    // account's home while the plain entry held yours alone (Q1).
    let parent = std::path::Path::new(home).parent().unwrap().display();
    for entry in ["~/vault/*.vlt", "~/vault/**", "~/vault/{k,j}.vlt"] {
        assert!(holds(entry, &format!("{home}/vault/k.vlt")), "{entry}");
        assert!(holds(entry, "~/vault/k.vlt"), "{entry}");
        for elsewhere in [
            "/srv/vault/k.vlt".to_string(),
            format!("{parent}/other/vault/k.vlt"),
            "/home/other/vault/k.vlt".to_string(),
            "/root/vault/k.vlt".to_string(),
        ] {
            assert!(!holds(entry, &elsewhere), "{entry} holds {elsewhere}");
        }
    }
    // A name: a file's, anywhere, as `find -name`.
    assert!(holds("id_*.vlt", "/any/where/id_work.vlt"));
    assert!(
        !holds("id_*.vlt", "/any/where/myid_work.vlt"),
        "the whole name"
    );
    assert!(holds("cred*.box", "cred-prod.box"));
    // A name holds a DIRECTORY of that name too, and what is below it — the
    // gate's reading, stricter than the matcher's ("a name is a file's":
    // `.env*` then held nothing below it; Reviewer A, round 28). Asked of
    // Steven (review stop 12, Q9); pinned as it is, so it cannot move unsaid.
    assert!(
        holds("cred*.box", "/x/cred-prod.box/notes"),
        "Q9: as read now"
    );
    // A relative path: where Shannon started, and — the gate's reading, found
    // anywhere as a name is (`secret_word`), stricter than the matcher's
    // anchoring at the start (Q9, pinned as it is).
    assert!(holds("vault/*.vlt", "vault/k.vlt"));
    assert!(
        holds("vault/*.vlt", "/elsewhere/vault/k.vlt"),
        "Q9: as read now"
    );
    // And a `~/` path ends at its segment: `~/.ssh` is not `~/.sshrc`.
    assert!(held("~/.ssh/config"));
    assert!(!held("~/.sshrc"));
}

/// **`re:` — a regex in the list** (Steven, 2026-10-03), the primitive a
/// rule's path pattern is: read as a path (`..` resolved, home both ways),
/// and without regard to case — `(?-i)` to make it care.
#[test]
fn a_listed_regex_reads_a_path_as_the_list_does() {
    let home = home();
    let rs = gate(crate::SecretsConfig {
        paths: vec![
            r"re:^/srv/keys/.*\.(vlt|box)$".to_string(),
            r"re:^~/vault/".to_string(),
            r"re:(?-i)^/opt/Exact/".to_string(),
        ],
        ..Default::default()
    });
    let held = |p: &str| rs.credential_path(p).is_some();
    assert!(held("/srv/keys/prod/a.vlt"));
    assert!(held("/srv/KEYS/a.VLT"), "case folds");
    assert!(held("/srv/x/../keys/a.box"), "`..`");
    assert!(!held("/srv/keys/readme.md"));
    assert!(held(&format!("{home}/vault/x")), "home");
    assert!(held("/opt/Exact/x"));
    assert!(!held("/opt/exact/x"), "`(?-i)`");
    let broken = gate(crate::SecretsConfig {
        paths: vec!["re:(unclosed".to_string()],
        ..Default::default()
    });
    assert!(broken.credential_path("(unclosed").is_none());
}

/// A concrete path each shipped entry's shape is meant to catch — the
/// premise of the tests below, stated once.
fn probe_for(entry: &str) -> String {
    if let Some(rest) = entry.strip_prefix("~/") {
        format!("~/{rest}")
    } else if let Some(ext) = entry.strip_prefix('*') {
        format!("secret{ext}")
    } else if let Some(stem) = entry.strip_suffix('*') {
        format!("{stem}.local")
    } else if entry.contains('/') {
        entry.to_string()
    } else {
        format!("/home/me/{entry}")
    }
}

/// The shipped entries the gate reads: every one but `{claude-home}`, which
/// holds nothing until launch names Claude Code's home.
fn shipped() -> Vec<&'static str> {
    static CFG: std::sync::LazyLock<crate::SecretsConfig> =
        std::sync::LazyLock::new(crate::SecretsConfig::default);
    CFG.all_paths()
        .filter(|e| !e.starts_with(crate::secrets::CLAUDE_HOME))
        .collect()
}

/// **The shipped list is the fixture, not three hand-picked rows**: every
/// entry holds whatever the case (APFS and NTFS fold it, so `cat .ENV`
/// opens `.env`; Reviewer E). Driving the LIST means a row added later is
/// covered by construction (`[[feedback_enumeration_means_unmodeled]]`).
#[test]
fn every_shipped_entry_holds_whatever_the_case() {
    let entries = shipped();
    assert!(entries.len() > 20, "premise: the list, {}", entries.len());
    for entry in entries {
        let probe = probe_for(entry);
        assert!(held(&probe), "premise: {entry} should catch {probe}");
        let shouted = probe.to_ascii_uppercase();
        assert!(held(&shouted), "{entry} is case-sensitive: {shouted}");
    }
}

/// **The same file, spelled another way, is still the same file**: `//`,
/// `/./` and a trailing `/` (`cat ~/.docker//config.json` was `Allow`,
/// unattended, where the plain spelling asked).
#[test]
fn a_second_spelling_of_the_same_file_is_still_a_credential() {
    for entry in shipped() {
        let probe = probe_for(entry);
        assert!(held(&probe), "premise: {entry} should catch {probe}");
        for spelled in [
            probe.replace('/', "//"),
            probe.replace('/', "/./"),
            format!("{probe}/"),
        ] {
            assert!(
                held(&spelled),
                "{entry} misses the second spelling {spelled}"
            );
        }
    }
}

/// **A DIRECTORY entry holds what is inside it, not just its name** — and
/// not a sibling that merely shares a prefix.
#[test]
fn a_directory_entry_holds_what_is_inside_it() {
    let rs = gate(crate::SecretsConfig {
        paths: vec!["/var/lib/shannon/log".to_string()],
        ..Default::default()
    });
    for probe in [
        "/var/lib/shannon/log",
        "/var/lib/shannon/log/shannon.log",
        "/var/lib/shannon/log/rotated/old.log",
        "/var/lib/shannon/log//shannon.log",
        "/var/lib/shannon/log/./shannon.log",
    ] {
        assert!(rs.credential_path(probe).is_some(), "{probe}");
    }
    assert!(
        rs.credential_path("/var/lib/shannon/logger.txt").is_none(),
        "a prefix match must not reach past the directory boundary"
    );
}
