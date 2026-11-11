//! Every test here builds its own roots in a temporary directory: none
//! reads the user's config, home or this repository's `AGENTS.md`.

use std::path::{Path, PathBuf};

use super::*;
use crate::config::{ClaudeFiles, MemoryConfig, MemoryMode, SkillsConfig};
use crate::message::{Block, ContextKind, Message};

/// A workspace: a repository root holding `a/b` (where Shannon starts), a
/// config directory, and a home — each a directory of its own.
struct Space {
    _dir: tempfile::TempDir,
    root: PathBuf,
    roots: Roots,
}

impl Space {
    fn new() -> Space {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let start = root.join("a/b");
        std::fs::create_dir_all(&start).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let config = dir.path().join("config");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let roots = Roots {
            start,
            config_dir: Some(config),
            project: "proj-1234abcd".into(),
            // Unset `CLAUDE_CONFIG_DIR`'s answer: the home's `.claude`.
            claude_home: Some(home.join(".claude")),
            home: Some(home),
            claude_memory: Some(dir.path().join("claude/projects/-repo/memory")),
            project_root: MemoryConfig::default().project_root,
        };
        Space {
            _dir: dir,
            root,
            roots,
        }
    }

    fn write(&self, path: &Path, text: &str) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        path.to_path_buf()
    }

    fn config(&self) -> &Path {
        self.roots.config_dir.as_deref().unwrap()
    }
}

fn none(_: &Path) -> Option<String> {
    None
}

fn found(space: &Space, memory: &MemoryConfig, skills: &SkillsConfig, session: Session) -> Found {
    find(memory, skills, &space.roots, session, &none)
}

/// **A placed location is one place; a relative one walks from where
/// Shannon started up to the repository root, the root's first** — and with
/// no repository above, only where it started.
#[test]
fn a_location_is_placed_or_walked_to_the_repository_root() {
    let s = Space::new();
    let walked = locate::places("AGENTS.md", &s.roots);
    assert_eq!(
        walked,
        [
            s.root.join("AGENTS.md"),
            s.root.join("a/AGENTS.md"),
            s.root.join("a/b/AGENTS.md")
        ]
    );
    assert_eq!(
        locate::places("{config}/projects/{project}/memory", &s.roots),
        [s.config().join("projects/proj-1234abcd/memory")]
    );
    assert_eq!(
        locate::places("~/notes.md", &s.roots),
        [s.roots.home.as_ref().unwrap().join("notes.md")]
    );
    assert_eq!(
        locate::places("{start}/x", &s.roots),
        [s.roots.start.join("x")]
    );
    assert_eq!(
        locate::places("/abs/x", &s.roots),
        [PathBuf::from("/abs/x")]
    );
    let mut bare = s.roots.clone();
    bare.config_dir = None;
    assert!(locate::places("{config}/x", &bare).is_empty());
    std::fs::remove_dir_all(s.root.join(".git")).unwrap();
    assert_eq!(
        locate::places("AGENTS.md", &s.roots),
        [s.roots.start.join("AGENTS.md")]
    );
}

/// **AGENTS.md and SHANNON.md, from the root down; a directory is its
/// `*.md`** (Steven, 10-06, D1 and D2).
#[test]
fn instructions_are_found_from_the_root_down() {
    let s = Space::new();
    let top = s.write(&s.root.join("AGENTS.md"), "top");
    let here = s.write(&s.roots.start.join("SHANNON.md"), "here");
    let rules = s.root.join("rules");
    let b = s.write(&rules.join("b.md"), "b");
    let a = s.write(&rules.join("a.md"), "a");
    s.write(&rules.join("not.txt"), "x");
    let mut memory = MemoryConfig::default();
    memory.instructions.push(rules.display().to_string());
    let f = found(&s, &memory, &SkillsConfig::default(), Session::Api);
    let paths: Vec<PathBuf> = f
        .items
        .iter()
        .filter(|i| i.role == Role::Instructions)
        .map(|i| i.path.clone())
        .collect();
    assert_eq!(paths, [top, here, a, b]);
}

/// **CLAUDE.md is opt-in, and on in a Claude Code session** — where
/// `claude` reads it, so Shannon lists it and never sends it again (Steven,
/// 10-06).
#[test]
fn claude_md_is_read_by_whoever_the_setting_says() {
    let s = Space::new();
    let file = s.write(&s.root.join("CLAUDE.md"), "claude's");
    let skill = s.write(&s.root.join(".claude/skills/rev/SKILL.md"), "body");
    let has = |f: &Found, path: &Path, state: State| {
        f.items.iter().any(|i| i.path == path && i.state == state)
    };
    for (claude, session, state) in [
        (ClaudeFiles::Auto, Session::Api, None),
        (ClaudeFiles::On, Session::Api, Some(State::Loaded)),
        (
            ClaudeFiles::Auto,
            Session::ClaudeCode,
            Some(State::ClaudeCode),
        ),
        (
            ClaudeFiles::On,
            Session::ClaudeCode,
            Some(State::ClaudeCode),
        ),
        (ClaudeFiles::Off, Session::ClaudeCode, None),
        (ClaudeFiles::Off, Session::Api, None),
    ] {
        let memory = MemoryConfig {
            claude,
            ..MemoryConfig::default()
        };
        let f = found(&s, &memory, &SkillsConfig::default(), session);
        match &state {
            Some(state) => assert!(has(&f, &file, state.clone()), "{claude:?} {session:?}"),
            None => assert!(
                !f.items.iter().any(|i| i.path == file),
                "{claude:?} {session:?}"
            ),
        }
        let skill_state = state.map(|s| match s {
            State::Loaded => State::Listed,
            other => other,
        });
        // Shannon names a skill to the model only when Shannon reads it:
        // `claude` lists its own.
        let listed = context(&f, MemoryMode::Off, session)
            .iter()
            .any(|t| t.kind == ContextKind::Skills && t.text.contains("- rev:"));
        assert_eq!(
            listed,
            skill_state == Some(State::Listed),
            "{claude:?} {session:?}"
        );
        match skill_state {
            Some(state) => assert!(has(&f, &skill, state), "{claude:?} {session:?}"),
            None => assert!(
                !f.items.iter().any(|i| i.path == skill),
                "{claude:?} {session:?}"
            ),
        }
    }
}

/// **A memory directory's index is read and its files listed; a file is
/// read whole; where the model writes is read too, and its own** — the
/// write location kept under `mode = off`, only no longer written.
#[test]
fn memories_are_an_index_and_files_and_the_write_place_is_read() {
    let s = Space::new();
    let shared = s.root.join("shared");
    let index = s.write(&shared.join("MEMORY.md"), "- [a](a.md) — a");
    let a = s.write(
        &shared.join("a.md"),
        "---\nname: a\ndescription: \"the a fact\"\ntype: user\n---\nbody",
    );
    let one = s.write(&s.root.join("one.md"), "a memory");
    let write = s.config().join("projects/proj-1234abcd/memory");
    let mine = s.write(&write.join("MEMORY.md"), "- [m](m.md)");
    let memory = MemoryConfig {
        paths: vec![shared.display().to_string(), one.display().to_string()],
        ..MemoryConfig::default()
    };
    let f = found(&s, &memory, &SkillsConfig::default(), Session::Api);
    let item = |p: &Path| f.items.iter().find(|i| i.path == p).expect("found").clone();
    assert_eq!(
        (item(&index).role, item(&index).state),
        (Role::MemoryIndex, State::Loaded)
    );
    assert_eq!(
        (item(&a).role, item(&a).state),
        (Role::Memory, State::Listed)
    );
    assert_eq!(item(&a).description.as_deref(), Some("the a fact"));
    assert_eq!(
        (item(&one).role, item(&one).state),
        (Role::Memory, State::Loaded)
    );
    assert!(item(&mine).writable && !item(&a).writable);
    assert_eq!(f.write, Some(Write::Shannon(write.clone())));

    let off = MemoryConfig {
        mode: MemoryMode::Off,
        ..memory
    };
    let f = found(&s, &off, &SkillsConfig::default(), Session::Api);
    assert_eq!(f.write, None, "off: nowhere to write");
    assert!(f.items.iter().any(|i| i.path == mine), "still read");
}

/// **Skills by directory name, in name order; a symlinked twin is one
/// skill; a second of one name is shadowed, and says by what.** The twin is
/// a renamed skill whose old directory is linked under its new name, which
/// sorts first.
#[test]
fn skills_are_named_by_their_directory_and_found_once() {
    let s = Space::new();
    let dir = s.config().join("skills");
    let rev = s.write(
        &dir.join("review-old/SKILL.md"),
        "---\nname: review\ndescription: >\n  Review the\n  changes.\n---\nRun it.",
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.join("review-old"), dir.join("review")).unwrap();
    s.write(&dir.join("zeta/SKILL.md"), "z");
    let local = s.write(
        &s.roots.start.join(".shannon/skills/zeta/SKILL.md"),
        "local z",
    );
    let one = s.write(
        &s.root.join("solo.md"),
        "---\ndescription: one file\n---\nsolo",
    );
    let skills = SkillsConfig {
        paths: vec![
            "{config}/skills".into(),
            ".shannon/skills".into(),
            one.display().to_string(),
        ],
        ..SkillsConfig::default()
    };
    let f = found(&s, &MemoryConfig::default(), &skills, Session::Api);
    let names: Vec<&str> = f.skills().map(|i| i.name.as_str()).collect();
    #[cfg(unix)]
    assert_eq!(names, ["review", "zeta", "solo"], "the twin once");
    let review = f.skill("review").or(f.skill("review-old")).unwrap();
    assert_eq!(review.description.as_deref(), Some("Review the changes."));
    assert_eq!(
        std::fs::canonicalize(&review.path).unwrap(),
        std::fs::canonicalize(&rev).unwrap()
    );
    let shadowed = f.items.iter().find(|i| i.path == local).expect("listed");
    assert!(
        matches!(&shadowed.state, State::Refused(why) if why.contains("shadowed")),
        "{:?}",
        shadowed.state
    );
}

/// **A credential path is never read** — refused, and no description taken
/// from its head; a Claude Code file there is still Claude Code's.
#[test]
fn a_credential_path_is_never_read() {
    let s = Space::new();
    let secret = s.write(&s.root.join("AGENTS.md"), "token=hunter2");
    let skill = s.write(
        &s.config().join("skills/x/SKILL.md"),
        "---\ndescription: d\n---\n",
    );
    // The app's judge says it whole (`App::never_read`).
    let judge = |p: &Path| {
        (p == secret || p == skill).then(|| "a credential path (test) — never read".to_string())
    };
    let f = find(
        &MemoryConfig::default(),
        &SkillsConfig::default(),
        &s.roots,
        Session::Api,
        &judge,
    );
    for path in [&secret, &skill] {
        let item = f.items.iter().find(|i| &i.path == path).expect("listed");
        assert!(matches!(&item.state, State::Refused(why) if why.contains("credential")));
        assert_eq!(item.description, None);
    }
    assert!(f.skill("x").is_none(), "a refused skill cannot be invoked");
    let told = context(&f, MemoryMode::Auto, Session::Api);
    assert!(!told.iter().any(|t| t.text.contains("hunter2")));
    assert!(skill_text(&skill, "", &judge).is_err());
}

/// **A link is judged by where it leads** — a repository's `AGENTS.md` or a
/// skill linked to a key is the key, and never read, though its own name is
/// nothing the judge knows (Reviewer B, round 27).
#[cfg(unix)]
#[test]
fn a_link_to_a_credential_path_is_never_read() {
    let s = Space::new();
    // Two keys: links to one file are one file, found once.
    let key = s.write(&s.config().join("keys/id_rsa"), "BEGIN KEY hunter2");
    let other = s.write(&s.config().join("keys/id_ed25519"), "BEGIN KEY hunter2");
    let agents = s.root.join("AGENTS.md");
    std::os::unix::fs::symlink(&key, &agents).unwrap();
    let skill = s.config().join("skills/helper/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&other, &skill).unwrap();
    // Lexical, as the gate's is: only the keys' own paths are named.
    let judge = |p: &Path| {
        p.parent()
            .is_some_and(|d| d.ends_with("keys"))
            .then(|| "a credential path (test) — never read".to_string())
    };
    assert!(
        judge(&agents).is_none() && judge(&skill).is_none(),
        "premise"
    );
    let f = find(
        &MemoryConfig::default(),
        &SkillsConfig::default(),
        &s.roots,
        Session::Api,
        &judge,
    );
    for path in [&agents, &skill] {
        let item = f.items.iter().find(|i| &i.path == path).expect("listed");
        assert!(
            matches!(&item.state, State::Refused(why) if why.contains("credential")),
            "{path:?}: {:?}",
            item.state
        );
    }
    let told = context(&f, MemoryMode::Auto, Session::Api);
    assert!(!told.iter().any(|t| t.text.contains("hunter2")));
    assert!(skill_text(&skill, "", &judge).is_err());
}

/// **A repository's file or place that links OUT of the repository is not
/// read** — no credential needed: `AGENTS.md -> ~/.bash_history` was told
/// every session, and a linked `.shannon/skills` approved reads wherever it
/// led (Reviewer E1, round 27). A place of YOURS (`{config}`) follows its
/// links.
#[cfg(unix)]
#[test]
fn a_repository_link_out_of_the_repository_is_not_read() {
    let home = |s: &Space| s.roots.home.clone().unwrap();
    let s = Space::new();
    let history = s.write(&home(&s).join(".history"), "ssh prod-db");
    let agents = s.root.join("AGENTS.md");
    std::os::unix::fs::symlink(&history, &agents).unwrap();
    let elsewhere = home(&s).join("notes/skills");
    s.write(
        &elsewhere.join("x/SKILL.md"),
        "---\ndescription: d\n---\nbody",
    );
    std::os::unix::fs::symlink(&elsewhere, s.root.join(".shannon-skills")).unwrap();
    let skills = SkillsConfig {
        paths: vec![".shannon-skills".into()],
        ..SkillsConfig::default()
    };
    let f = find(
        &MemoryConfig::default(),
        &skills,
        &s.roots,
        Session::Api,
        &none,
    );
    let item = f.items.iter().find(|i| i.path == agents).expect("listed");
    assert!(
        matches!(&item.state, State::Refused(why) if why.contains("out of the repository")),
        "{:?}",
        item.state
    );
    assert!(
        f.skill("x").is_none(),
        "a linked skill location is not read"
    );
    let told = context(&f, MemoryMode::Off, Session::Api);
    assert!(!told.iter().any(|t| t.text.contains("prod-db")));
    let (approved, said) = rules(
        &MemoryConfig::default(),
        &skills,
        &s.roots,
        &[],
        Session::Api,
    );
    assert!(
        !approved
            .iter()
            .any(|r| r.source.contains(".shannon-skills")),
        "no read rule through the link"
    );
    assert!(
        said.iter()
            .any(|w| w.contains("leads out of the repository")),
        "{said:?}"
    );

    // Yours: a placed location follows its link.
    let s = Space::new();
    let mine = home(&s).join("my-skills");
    s.write(&mine.join("y/SKILL.md"), "---\ndescription: d\n---\nbody");
    std::os::unix::fs::symlink(&mine, s.config().join("skills")).unwrap();
    let f = found(
        &s,
        &MemoryConfig::default(),
        &SkillsConfig::default(),
        Session::Api,
    );
    assert!(f.skill("y").is_some(), "{:?}", f.items);
}

/// **A place is read for at most `SCAN_CAP` entries, and says so** — a
/// location can be a link to a system directory (Reviewer E1, round 27).
#[test]
fn a_huge_place_is_read_in_part_and_said() {
    let s = Space::new();
    for i in 0..=super::SCAN_CAP {
        s.write(&s.config().join(format!("skills/s{i:04}/SKILL.md")), "x");
    }
    let f = found(
        &s,
        &MemoryConfig::default(),
        &SkillsConfig::default(),
        Session::Api,
    );
    // The cap, read whole: `<= 1000` passed with none read (review stop
    // 12, T9), and the numbers are the cap's, not typed.
    assert_eq!(f.skills().count(), super::SCAN_CAP);
    let said = format!("only {} were read", super::SCAN_CAP);
    assert!(f.notes.iter().any(|n| n.contains(&said)), "{:?}", f.notes);
}

/// **A skill's text is bounded after its arguments are placed** — each
/// `$ARGUMENTS` is a copy of them, and the model writes them.
#[test]
fn a_skills_text_is_bounded_whatever_its_arguments() {
    let s = Space::new();
    let many = "$ARGUMENTS ".repeat(2000);
    let skill = s.write(&s.root.join("x/SKILL.md"), &many);
    let text = skill_text(&skill, &"a".repeat(1000), &none).unwrap();
    assert!(text.len() < 70 << 10, "{} bytes", text.len());
    assert!(text.ends_with("the rest is in the file]"), "said");
    // A multibyte argument cut on a character, never inside one.
    let text = skill_text(&skill, &"é".repeat(500), &none).unwrap();
    assert!(text.len() < 70 << 10);
    // The WORK is bounded too, not only what is sent: 6000 places of 100 KB
    // arguments was 600 MB built before the cut (Reviewer E1, round 27).
    // Bounded by the loop's shape, NOT asserted by time: the old code built
    // those 600 MB in ~60 ms here, so a time bound at a size a test may
    // allocate cannot tell the two apart (measured; an assertion that could
    // was vacuous). What this pins is the output.
    let many = "$ARGUMENTS".repeat(6000);
    let skill = s.write(&s.root.join("y/SKILL.md"), &many);
    let text = skill_text(&skill, &"a".repeat(100 << 10), &none).unwrap();
    assert!(text.len() < 70 << 10);
}

/// **What the model is told**: each loaded file, cut where long; the memory
/// guidance where it may write, saying whether writes ask; the skills it may
/// load, worded for its session.
#[test]
fn the_model_is_told_the_files_the_guidance_and_the_skills() {
    let s = Space::new();
    s.write(&s.root.join("AGENTS.md"), "be terse");
    s.write(&s.root.join("SHANNON.md"), &"x".repeat(50_000));
    s.write(&s.root.join("a/AGENTS.md"), "   ");
    s.write(
        &s.config().join("skills/rev/SKILL.md"),
        "---\ndescription: reviews\n---\n",
    );
    let f = found(
        &s,
        &MemoryConfig::default(),
        &SkillsConfig::default(),
        Session::Api,
    );
    let told = context(&f, MemoryMode::Auto, Session::Api);
    let kinds: Vec<(ContextKind, &str)> =
        told.iter().map(|t| (t.kind, t.source.as_str())).collect();
    assert_eq!(
        kinds.len(),
        4,
        "two files (an empty one is not told), guidance, skills: {kinds:?}"
    );
    assert_eq!(told[0].text, "be terse");
    assert!(told[1].text.len() < 42 << 10 && told[1].text.contains("cut at 40 KB"));
    assert_eq!(told[2].source, "shannon:memory");
    assert!(told[2].text.contains("without asking"));
    assert_eq!(told[3].source, "shannon:skills");
    assert!(told[3].text.contains("`skill` tool") && told[3].text.contains("- rev: reviews"));

    let asks = context(&f, MemoryMode::Ask, Session::Api);
    assert!(asks[2].text.contains("asks the user"));
    let cc = context(&f, MemoryMode::Auto, Session::ClaudeCode);
    assert!(cc[3].text.contains("SKILL.md"), "{}", cc[3].text);
    let off = MemoryConfig {
        mode: MemoryMode::Off,
        ..MemoryConfig::default()
    };
    let f = found(&s, &off, &SkillsConfig::default(), Session::Api);
    assert!(
        !context(&f, MemoryMode::Off, Session::Api)
            .iter()
            .any(|t| t.source == "shannon:memory")
    );
}

/// **The skills list stays within its budget**, and says how many it left
/// out.
#[test]
fn the_skills_list_is_bounded() {
    let s = Space::new();
    for i in 0..80 {
        s.write(
            &s.config().join(format!("skills/s{i:02}/SKILL.md")),
            &format!("---\ndescription: {}\n---\n", "d".repeat(2_000)),
        );
    }
    let f = found(
        &s,
        &MemoryConfig::default(),
        &SkillsConfig::default(),
        Session::Api,
    );
    let told = context(&f, MemoryMode::Off, Session::Api);
    let list = &told
        .iter()
        .find(|t| t.kind == ContextKind::Skills)
        .unwrap()
        .text;
    assert!(list.len() <= 8 << 10, "{}", list.len());
    // The count is of what is NOT listed (Reviewer C, round 27: untested).
    let listed = list.lines().filter(|l| l.starts_with("- ")).count();
    assert!(
        list.contains(&format!(
            "… and {} more (`:skills` lists them)",
            80 - listed
        )),
        "{listed} listed"
    );
    assert!(list.contains(&"d".repeat(1_536)), "a description to 1,536");
    assert!(!list.contains(&"d".repeat(1_537)), "and no further");
}

/// **What is told is bounded in all, not only file by file** — an
/// `AGENTS.md` that is a directory of many files is named past the budget,
/// root first, and never read there (Reviewer B, round 27: 20 MB a turn).
#[test]
fn what_is_told_is_bounded_in_all() {
    let s = Space::new();
    for i in 0..20 {
        s.write(
            &s.root.join(format!("AGENTS.md/f{i:02}.md")),
            &"x".repeat(30 << 10),
        );
    }
    let f = found(
        &s,
        &MemoryConfig::default(),
        &SkillsConfig::default(),
        Session::Api,
    );
    let told = context(&f, MemoryMode::Off, Session::Api);
    let files: Vec<&Told> = told
        .iter()
        .filter(|t| t.kind == ContextKind::Instructions && !t.source.starts_with("shannon:"))
        .collect();
    assert_eq!(files.len(), 20, "every file named");
    let total: usize = files.iter().map(|t| t.text.len()).sum();
    assert!(total < (128 + 40) << 10, "{total}");
    let unsent = files
        .iter()
        .filter(|t| t.text.starts_with("[not sent"))
        .count();
    assert!(unsent > 10, "{unsent}");
    assert!(files[0].text.starts_with('x'), "the first is sent");
}

fn told(kind: ContextKind, source: &str, text: &str) -> Told {
    Told {
        kind,
        source: source.into(),
        text: text.into(),
    }
}

fn sent_with(blocks: Vec<Block>) -> Message {
    let mut m = Message::user("q");
    m.blocks = blocks;
    m
}

/// **A turn carries what is new or changed, and unloads what went** — read
/// from the history alone, so a composed turn that dropped the blocks has
/// them sent again, and a skill's text is never compared.
#[test]
fn a_turn_carries_only_what_changed() {
    let a = told(ContextKind::Instructions, "/a", "one");
    let b = told(ContextKind::Memory, "/b", "two");
    let first = attach(&[], &[a.clone(), b.clone()], &[]);
    assert_eq!(first.len(), 2, "everything, the first time");
    let history = vec![sent_with(first), Message::assistant("ok")];
    assert!(
        attach(&history, &[a.clone(), b.clone()], &[]).is_empty(),
        "nothing new"
    );

    let a2 = told(ContextKind::Instructions, "/a", "one, edited");
    let changed = attach(&history, &[a2.clone(), b.clone()], &[]);
    assert_eq!(
        changed,
        [Block::Context {
            kind: ContextKind::Instructions,
            source: "/a".into(),
            text: "one, edited".into()
        }]
    );

    let gone = attach(&history, std::slice::from_ref(&a), &[]);
    assert!(
        matches!(&gone[..], [Block::Context { kind: ContextKind::Unloaded, source, .. }] if source == "/b")
    );
    // Not told, and not gone: the model has it from elsewhere (`claude`
    // reads its CLAUDE.md in a Claude Code session).
    assert!(
        attach(&history, std::slice::from_ref(&a), &["/b".to_string()]).is_empty(),
        "kept is never unloaded"
    );
    let mut later = history.clone();
    later.push(sent_with(gone));
    assert!(
        attach(&later, std::slice::from_ref(&a), &[]).is_empty(),
        "unloaded stays unloaded"
    );
    assert_eq!(
        attach(&later, &[a.clone(), b.clone()], &[]).len(),
        1,
        "and comes back"
    );

    let skill = Block::Context {
        kind: ContextKind::Skill,
        source: "rev".into(),
        text: "body".into(),
    };
    let with_skill = vec![sent_with(vec![skill])];
    assert_eq!(
        attach(&with_skill, &[a], &[]).len(),
        1,
        "a skill is never compared nor unloaded"
    );
}

/// **A skill's text**: its frontmatter dropped, where it lives said first,
/// `$ARGUMENTS` filled — or the arguments added when it does not place them.
#[test]
fn a_skill_is_invoked_with_its_arguments() {
    let s = Space::new();
    let placed = s.write(
        &s.root.join("rev/SKILL.md"),
        "---\nname: rev\ndescription: d\n---\nReview $ARGUMENTS now.",
    );
    let text = skill_text(&placed, "deep", &none).unwrap();
    assert!(text.starts_with(&format!(
        "Base directory for this skill: {}",
        s.root.join("rev").display()
    )));
    assert!(text.ends_with("Review deep now."), "{text}");
    assert!(!text.contains("description: d"));
    let bare = s.write(&s.root.join("plain/SKILL.md"), "Just do it.");
    assert!(
        skill_text(&bare, "fast", &none)
            .unwrap()
            .ends_with("Just do it.\n\nARGUMENTS: fast")
    );
    assert!(
        skill_text(&bare, "", &none)
            .unwrap()
            .ends_with("Just do it.")
    );
    assert!(skill_text(&s.root.join("missing/SKILL.md"), "", &none).is_err());
    // Only the BODY places them: a description naming `$ARGUMENTS` does not.
    let described = s.write(
        &s.root.join("d/SKILL.md"),
        "---\ndescription: call with $ARGUMENTS\n---\nGo.",
    );
    assert!(
        skill_text(&described, "x", &none)
            .unwrap()
            .ends_with("Go.\n\nARGUMENTS: x")
    );
}

/// **Frontmatter as skills write it** — quoted values, a folded value over
/// indented lines; unclosed, it is no frontmatter at all.
#[test]
fn frontmatter_reads_as_skills_write_it() {
    let (keys, body) =
        read::frontmatter("---\nname: 'x'\ndescription: |\n  a\n  b\nother: \"q\"\n---\nbody\n");
    assert_eq!(read::key(&keys, "name"), Some("x"));
    assert_eq!(read::key(&keys, "description"), Some("a b"));
    assert_eq!(read::key(&keys, "other"), Some("q"));
    assert_eq!(body, "body\n");
    let (keys, body) = read::frontmatter("---\nname: x\nno end");
    assert!(keys.is_empty() && body.starts_with("---"));
    let (keys, body) = read::frontmatter("plain");
    assert!(keys.is_empty() && body == "plain");
    // An editor's byte-order mark is not text before the frontmatter.
    let (keys, body) = read::frontmatter("\u{feff}---\ndescription: d\n---\nbody");
    assert_eq!(read::key(&keys, "description"), Some("d"));
    assert_eq!(body, "body");
}

/// **A read is bounded and waits for nothing** — a directory or a FIFO is
/// refused, a long file cut on a character.
#[test]
fn a_read_is_bounded_and_never_waits() {
    let s = Space::new();
    let wide = s.write(&s.root.join("wide.md"), &"é".repeat(10));
    let cut = read::bounded(&wide, 5).unwrap();
    assert!(cut.cut && cut.text == "éé", "{:?}", cut.text);
    assert!(read::bounded(&s.root, 10).is_err());
    // Made, not skipped when `mkfifo` could not run, and read with a
    // deadline, so a regression fails here instead of hanging the suite —
    // `config::stack`'s shape (review stop 12, T9).
    #[cfg(unix)]
    {
        let fifo = s.root.join("fifo");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path and a mode; no memory is shared.
        assert_eq!(
            unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) },
            0,
            "premise: a FIFO"
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let reading = fifo.clone();
        std::thread::spawn(move || tx.send(read::bounded(&reading, 10).is_err()));
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(refused) => assert!(refused, "a FIFO is not read"),
            Err(_) => {
                // Release the blocked reader so the thread can end, then fail.
                let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
                panic!("the read BLOCKED on a FIFO");
            }
        }
    }
}

/// **The locations are places the gate approves** — read unasked; the write
/// location written unasked under `auto`, a delete there still asked; under
/// `ask`, a write asks — as rules the gate already reads, of their own
/// origin. A path no rule can spell is left out, and said.
#[test]
fn the_locations_are_places_the_gate_approves() {
    use crate::config::PermissionsConfig;
    use crate::permission::{Decision, Origin, RuleSet};
    let s = Space::new();
    let shared = s.config().join("memory");
    s.write(&shared.join("MEMORY.md"), "- [a](a.md)");
    let write = s.config().join("projects/proj-1234abcd/memory");
    std::fs::create_dir_all(&write).unwrap();
    let (shared, write) = (shared.display().to_string(), write.display().to_string());
    let policy = |memory: &MemoryConfig| {
        let written = std::slice::from_ref(&write);
        let (rules, said) = rules(
            memory,
            &SkillsConfig::default(),
            &s.roots,
            written,
            Session::Api,
        );
        assert!(said.is_empty(), "{said:?}");
        assert!(rules.iter().all(|r| r.origin == Origin::Knowledge));
        let (set, _) = RuleSet::configured(&PermissionsConfig::default(), &rules, &[]);
        set.with_start(&s.roots.start.display().to_string())
    };
    let decide = |set: &RuleSet, line: &str| set.decide_line(line).decision;
    let auto = policy(&MemoryConfig::default());
    assert_eq!(
        decide(&auto, &format!("cat {shared}/a.md")),
        Decision::Allow
    );
    assert_eq!(
        decide(&auto, &format!("cat {write}/MEMORY.md")),
        Decision::Allow
    );
    assert_eq!(
        decide(&auto, &format!("echo hi > {write}/m.md")),
        Decision::Allow
    );
    assert_eq!(
        decide(&auto, &format!("rm {write}/m.md")),
        Decision::Ask,
        "a delete asks"
    );
    assert_eq!(
        decide(&auto, "cat /etc/hosts"),
        Decision::Ask,
        "elsewhere asks"
    );
    // `paths` without the write location: it is read all the same.
    let ask = policy(&MemoryConfig {
        mode: MemoryMode::Ask,
        paths: vec!["{config}/memory".into()],
        ..MemoryConfig::default()
    });
    assert_eq!(
        decide(&ask, &format!("echo hi > {write}/m.md")),
        Decision::Ask
    );
    assert_eq!(
        decide(&ask, &format!("cat {write}/MEMORY.md")),
        Decision::Allow
    );

    // Relative: only where found — not a rule for every directory up a walk.
    s.write(&s.root.join(".shannon/skills/x/SKILL.md"), "x");
    let (found, _) = rules(
        &MemoryConfig::default(),
        &SkillsConfig::default(),
        &s.roots,
        &[],
        Session::Api,
    );
    let lines: Vec<&str> = found.iter().map(|r| r.source.as_str()).collect();
    assert!(
        lines
            .iter()
            .any(|l| *l == format!("allow :read {}/.shannon/skills", s.root.display())),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("a/b/.shannon")),
        "{lines:?}"
    );
    // A space, a wildcard or a comment no rule can spell: said, not
    // approved. A `*` spelled into a rule would approve every directory it
    // matches; a `#` would end the rule there.
    for odd in ["/my notes/memory", "/m*/memory", "/m#x/memory"] {
        let config = MemoryConfig {
            paths: vec![odd.into()],
            ..MemoryConfig::default()
        };
        let (approved, said) = rules(
            &config,
            &SkillsConfig::default(),
            &s.roots,
            &[],
            Session::Api,
        );
        assert!(said.iter().any(|w| w.contains(odd)), "{odd}: {said:?}");
        let parent = odd.rsplit_once('/').unwrap().0;
        assert!(!approved.iter().any(|r| r.source.contains(parent)), "{odd}");
    }
}

/// **Shannon's memory is granted past its config directory's ask** (P6a,
/// 10-13): it lives under `~/.config/shannon`, which the shipped `ask
/// :write ~/.config/shannon` keeps, and `Space` puts the config elsewhere,
/// so no test met the two together — the diary's replay, which has no
/// `[memory]` rules, asked for all five of its reads and writes. The rules
/// are `rules`' own for a write place under the real home that does not
/// exist (`paths` empty, so nothing is read there).
#[test]
fn shannons_memory_is_granted_past_its_config_dirs_ask() {
    use crate::config::PermissionsConfig;
    use crate::permission::{Decision, RuleSet};
    let home = crate::secrets::home_dir().expect("tests run with a HOME");
    let s = Space::new();
    let roots = Roots {
        config_dir: Some(PathBuf::from(format!("{home}/.config/shannon"))),
        project: "shannon-test-absent".into(),
        claude_memory: None,
        ..s.roots.clone()
    };
    let memory = MemoryConfig {
        paths: Vec::new(),
        ..MemoryConfig::default()
    };
    let dir = write_place(&memory, &roots, Session::Api)
        .expect("a write place")
        .dir()
        .display()
        .to_string();
    assert!(
        dir.starts_with(&format!("{home}/.config/shannon/")),
        "{dir}"
    );
    let gate = |rules: &[crate::permission::Rule]| {
        let (set, _) = RuleSet::configured(&PermissionsConfig::default(), rules, &[]);
        set.with_start(&roots.start.display().to_string())
    };
    let write = format!("echo x > {dir}/green.md");
    let bare = gate(&[]).decide_line(&write);
    assert!(
        bare.stages()[0].acts.iter().any(|a| matches!(&a.by,
            crate::permission::By::Rules(c) if c.iter().any(|c| c.source == r"ask :write \c~/.config/shannon"))),
        "premise: the config directory's ask meets it: {bare:?}"
    );
    let (rules, said) = rules(
        &memory,
        &SkillsConfig::default(),
        &roots,
        std::slice::from_ref(&dir),
        Session::Api,
    );
    assert!(said.is_empty(), "{said:?}");
    let set = gate(&rules);
    assert_eq!(set.decide_line(&write).decision, Decision::Allow);
    assert_eq!(
        set.decide_line(&format!("cat {dir}/MEMORY.md")).decision,
        Decision::Allow
    );
    // The directory itself too (Steven's `ls -la …/memory/`, 10-07).
    assert_eq!(
        set.decide_line(&format!("ls -la {dir}/")).decision,
        Decision::Allow
    );
    assert_eq!(
        set.decide_line(&format!("echo x > {home}/.config/shannon/config.toml"))
            .decision,
        Decision::Ask,
        "the config itself still asks"
    );
}

/// **In a Claude Code session, Claude Code's memory is the memory** (Steven,
/// 10-06: "When using a Claude Code session we should be able to access the
/// Claude Code memories") — its directory the write place, approved for
/// reads and, under `auto`, writes; and Shannon says nothing of its own
/// place, since claude's prompt names one and two made the model ask which.
/// `claude = "off"` gives the session Shannon's memory back. An API session
/// reads Claude Code's only under `claude = "on"`, and never writes there.
#[test]
fn a_claude_code_session_keeps_claude_codes_memory() {
    use crate::config::PermissionsConfig;
    use crate::permission::{Decision, RuleSet};
    let s = Space::new();
    let theirs = s.roots.claude_memory.clone().unwrap();
    let index = s.write(
        &theirs.join("MEMORY.md"),
        "- [green](green.md) — likes green",
    );
    let green = s.write(
        &theirs.join("green.md"),
        "---\ndescription: likes green\n---\n",
    );
    let ours = s.config().join("projects/proj-1234abcd/memory");
    let skills = SkillsConfig::default();
    let state = |f: &Found, path: &Path| {
        f.items
            .iter()
            .find(|i| i.path == path)
            .map(|i| (i.state.clone(), i.writable))
    };
    let guided = |f: &Found, mode| {
        context(f, mode, Session::ClaudeCode)
            .iter()
            .any(|t| t.source == "shannon:memory")
    };

    // `auto` (the default), a Claude Code session: claude's own.
    let memory = MemoryConfig::default();
    let f = found(&s, &memory, &skills, Session::ClaudeCode);
    assert_eq!(f.write, Some(Write::ClaudeCode(theirs.clone())));
    assert_eq!(state(&f, &index), Some((State::ClaudeCode, true)));
    assert_eq!(state(&f, &green), Some((State::ClaudeCode, true)));
    assert!(!guided(&f, MemoryMode::Auto), "claude's prompt says how");
    let spelled = theirs.display().to_string();
    // Each session's own write place, vouched — what the app passes.
    let gate = |session: Session, memory: &MemoryConfig| {
        let write = write_place(memory, &s.roots, session).map(|w| w.dir().display().to_string());
        let (rules, _) = rules(memory, &skills, &s.roots, &Vec::from_iter(write), session);
        let (set, _) = RuleSet::configured(&PermissionsConfig::default(), &rules, &[]);
        set.with_start(&s.roots.start.display().to_string())
    };
    let decide = |set: &RuleSet, line: &str| set.decide_line(line).decision;
    let set = gate(Session::ClaudeCode, &memory);
    assert_eq!(
        decide(&set, &format!("cat {spelled}/green.md")),
        Decision::Allow
    );
    assert_eq!(
        decide(&set, &format!("echo x > {spelled}/blue.md")),
        Decision::Allow
    );
    // Its skills' own files too (`~/.claude/skills`).
    let theirs_skills = s
        .roots
        .home
        .clone()
        .unwrap()
        .join(".claude/skills/x/notes.md");
    assert_eq!(
        decide(&set, &format!("cat {}", theirs_skills.display())),
        Decision::Allow
    );

    // An API session under `auto`: none of it — not read, not approved.
    let f = found(&s, &memory, &skills, Session::Api);
    assert_eq!(f.write, Some(Write::Shannon(ours.clone())));
    assert_eq!(state(&f, &index), None);
    let set = gate(Session::Api, &memory);
    assert_ne!(
        decide(&set, &format!("cat {spelled}/green.md")),
        Decision::Allow
    );

    // `claude = "on"`, an API session: read and told, written by Shannon's.
    let on = MemoryConfig {
        claude: ClaudeFiles::On,
        ..MemoryConfig::default()
    };
    let f = found(&s, &on, &skills, Session::Api);
    assert_eq!(state(&f, &index), Some((State::Loaded, false)));
    assert_eq!(state(&f, &green), Some((State::Listed, false)));
    assert_eq!(f.write, Some(Write::Shannon(ours.clone())));
    assert!(
        context(&f, MemoryMode::Auto, Session::Api)
            .iter()
            .any(|t| t.text.contains("likes green")),
        "its index told"
    );
    let set = gate(Session::Api, &on);
    assert_eq!(
        decide(&set, &format!("cat {spelled}/green.md")),
        Decision::Allow,
        "a memory there read as any memory location's"
    );
    assert_ne!(
        decide(&set, &format!("echo x > {spelled}/blue.md")),
        Decision::Allow,
        "but not written: Shannon's place is the API session's"
    );

    // `claude = "off"`, a Claude Code session: Shannon's memory, guided.
    let off = MemoryConfig {
        claude: ClaudeFiles::Off,
        ..MemoryConfig::default()
    };
    let f = found(&s, &off, &skills, Session::ClaudeCode);
    assert_eq!(f.write, Some(Write::Shannon(ours)));
    assert_eq!(state(&f, &index), None);
    assert!(guided(&f, MemoryMode::Auto));

    // `memory = "off"`, a Claude Code session: claude runs with its memory
    // off, so none of it is claude's — said, not hidden.
    let quiet = MemoryConfig {
        mode: MemoryMode::Off,
        ..MemoryConfig::default()
    };
    let f = found(&s, &quiet, &skills, Session::ClaudeCode);
    for path in [&index, &green] {
        assert!(
            matches!(state(&f, path), Some((State::Refused(why), _)) if why.contains("memory = off")),
            "{path:?}: {:?}",
            state(&f, path)
        );
    }
}

/// **`{project}` is named as `[memory] project` says** — `hash` keeps each
/// directory apart; `claude-code` names a repository once, from any directory
/// in it, as Claude Code does, and a path Claude Code's documented naming
/// does not settle (past its documented cut) takes Shannon's own name, told
/// apart.
#[test]
fn the_project_is_named_as_the_setting_says() {
    use crate::config::ProjectName;
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let repo = base.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    let hash = |p: &Path| project_name(p, ProjectName::Hash);
    let claude = |p: &Path| project_name(p, ProjectName::ClaudeCode);
    assert!(hash(&repo).starts_with("repo-"), "{}", hash(&repo));
    assert_ne!(
        hash(&repo),
        hash(&repo.join("src")),
        "each directory its own"
    );
    let dashed: String = repo
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    assert_eq!(claude(&repo), dashed);
    assert_eq!(claude(&repo.join("src")), dashed, "one for the repository");
    let long = |tail: &str| PathBuf::from(format!("/{}/{tail}", "a".repeat(260)));
    let (one, two) = (claude(&long("x")), claude(&long("y")));
    assert_eq!(one, hash(&long("x")), "Shannon's own name: {one}");
    assert_ne!(one, two, "told apart");
}

/// **What you disable is not told, not listed, and refused at the gate** —
/// a memory by its path, a skill by its name (every skill of it), the skills
/// under a disabled place; its neighbours untouched (Steven, 10-06: "block
/// the file"). One answer: the gate's refusals are built from what `find`
/// marked.
#[test]
fn what_is_disabled_is_not_told_and_refused() {
    use crate::permission::{Decision, RuleSet};
    let s = Space::new();
    let memories = s.config().join("projects/proj-1234abcd/memory");
    s.write(
        &memories.join("MEMORY.md"),
        "- [old](old.md)\n- [new](new.md)\n",
    );
    let old = s.write(&memories.join("old.md"), "---\ndescription: stale\n---\nx");
    let new = s.write(&memories.join("new.md"), "---\ndescription: fresh\n---\ny");
    let skills_dir = s.config().join("skills");
    let review = s.write(
        &skills_dir.join("review/SKILL.md"),
        "---\nname: review\ndescription: Review it\n---\nbody",
    );
    let helper = s.write(&skills_dir.join("review/notes.md"), "notes");
    let ship = s.write(
        &skills_dir.join("ship/SKILL.md"),
        "---\nname: ship\ndescription: Ship it\n---\nbody",
    );
    let elsewhere = s.config().join("vendor-skills");
    let vendored = s.write(
        &elsewhere.join("lint/SKILL.md"),
        "---\nname: lint\ndescription: Lint it\n---\nbody",
    );
    let memory = MemoryConfig {
        disabled: vec![old.display().to_string()],
        ..MemoryConfig::default()
    };
    let skills = SkillsConfig {
        paths: vec!["{config}/skills".into(), "{config}/vendor-skills".into()],
        disabled: vec!["review".into(), "{config}/vendor-skills".into()],
    };
    let f = found(&s, &memory, &skills, Session::Api);
    let state = |path: &Path| {
        f.items
            .iter()
            .find(|i| i.path == path)
            .map(|i| i.state.clone())
    };
    for off in [&old, &review, &vendored] {
        assert_eq!(state(off), Some(State::Disabled), "{off:?}");
    }
    assert_eq!(state(&new), Some(State::Listed));
    assert_eq!(state(&ship), Some(State::Listed));
    let usable: Vec<&str> = f.skills().map(|i| i.name.as_str()).collect();
    assert_eq!(usable, vec!["ship"], "a disabled skill is not usable");
    let told = context(&f, MemoryMode::Auto, Session::Api);
    let skills_told = told.iter().find(|t| t.source == "shannon:skills").unwrap();
    assert!(!skills_told.text.contains("review") && skills_told.text.contains("ship"));

    let write = write_place(&memory, &s.roots, Session::Api).map(|w| w.dir().display().to_string());
    let (rules, said) = rules(
        &memory,
        &skills,
        &s.roots,
        &Vec::from_iter(write),
        Session::Api,
    );
    assert!(said.is_empty(), "{said:?}");
    let (set, _) = RuleSet::configured(&crate::config::PermissionsConfig::default(), &rules, &[]);
    let set = set.with_start(&s.roots.start.display().to_string());
    let cat = |p: &Path| set.decide_line(&format!("cat {}", p.display())).decision;
    for refused in [&old, &review, &helper, &vendored] {
        assert_eq!(cat(refused), Decision::Deny, "{refused:?}");
    }
    assert_eq!(cat(&new), Decision::Allow, "a neighbour stays read");
    assert_eq!(cat(&ship), Decision::Allow);

    // Your own grant of the place, spelled `DIR/**` as `:help` teaches,
    // names what the refusal names: the refusal stands (review of e10b6d5,
    // A, B and C — `deny :read DIR` counted broader than `allow :read
    // DIR/**`, which took its place, and a disabled skill was read unasked).
    let yours = crate::config::PermissionsConfig {
        rules: vec![
            format!("allow :read {}/**", skills_dir.join("review").display()),
            format!("allow :read {}/**", elsewhere.display()),
        ],
        ..Default::default()
    };
    let (set, problems) = RuleSet::configured(&yours, &rules, &[]);
    assert!(problems.is_empty(), "{problems:?}");
    let set = set.with_start(&s.roots.start.display().to_string());
    for refused in [&review, &helper, &vendored] {
        let decided = set
            .decide_line(&format!("cat {}", refused.display()))
            .decision;
        assert_eq!(decided, Decision::Deny, "{refused:?} under your allow");
    }
}

/// **Disabled holds wherever the file is reached** (review 29): an
/// instruction file is disabled as a memory is; the gate refuses what is
/// under a disabled place that `find` never marks (`notes.txt`), and a place
/// not there yet as a file or a directory; a place no rule can spell is said.
/// The index beside a disabled memory is told as it is.
#[test]
fn a_disabled_memory_is_refused_wherever_it_is_reached() {
    use crate::permission::{Decision, RuleSet};
    let s = Space::new();
    let memories = s.config().join("projects/proj-1234abcd/memory");
    let index = s.write(
        &memories.join("MEMORY.md"),
        "- [old](old.md) — stale\n- [new](./new.md) — fresh\n- [both](old.md) and [new](new.md)\n",
    );
    let old = s.write(&memories.join("old.md"), "x");
    s.write(&memories.join("new.md"), "y");
    let agents = s.write(&s.root.join("AGENTS.md"), "be terse");
    let shared = s.config().join("shared");
    let unmarked = s.write(&shared.join("notes.txt"), "not markdown");
    let later = s.config().join("later");
    let spaced = s.config().join("my notes");
    std::fs::create_dir_all(&spaced).unwrap();
    // A brace the rule grammar refuses: said, never dropped unsaid (review
    // of e10b6d5, A).
    let braced = s.config().join("odd{x}");
    std::fs::create_dir_all(&braced).unwrap();
    let memory = MemoryConfig {
        paths: vec!["{config}/shared".into()],
        disabled: vec![
            old.display().to_string(),
            agents.display().to_string(),
            "{config}/shared".into(),
            "{config}/later".into(),
            "{config}/my notes".into(),
            braced.display().to_string(),
        ],
        ..MemoryConfig::default()
    };
    let skills = SkillsConfig::default();
    let f = found(&s, &memory, &skills, Session::Api);
    let item = |p: &Path| f.items.iter().find(|i| i.path == p).cloned();
    assert_eq!(item(&agents).map(|i| i.state), Some(State::Disabled));
    assert_eq!(
        item(&index).map(|i| i.state),
        Some(State::Loaded),
        "the premise"
    );

    let told = context(&f, MemoryMode::Auto, Session::Api);
    assert!(
        !told
            .iter()
            .any(|t| t.source == agents.display().to_string())
    );
    let index_told = told
        .iter()
        .find(|t| t.source == index.display().to_string())
        .unwrap();
    // The index is told as it is: disabling refuses the FILE, it does not
    // edit the index for you (Steven, 10-07).
    assert_eq!(
        index_told.text,
        "- [old](old.md) — stale\n- [new](./new.md) — fresh\n- [both](old.md) and [new](new.md)\n"
    );

    let (rules, said) = rules(&memory, &skills, &s.roots, &[], Session::Api);
    for odd in ["my notes", "odd{x}"] {
        assert!(
            said.iter()
                .any(|w| w.contains(odd) && w.contains("not refused")),
            "{odd}: {said:?}"
        );
    }
    let (set, _) = RuleSet::configured(&crate::config::PermissionsConfig::default(), &rules, &[]);
    let set = set.with_start(&s.roots.start.display().to_string());
    let cat = |p: &Path| set.decide_line(&format!("cat {}", p.display())).decision;
    for refused in [&old, &agents, &unmarked, &later, &later.join("x.md")] {
        assert_eq!(cat(refused), Decision::Deny, "{refused:?}");
    }
}

/// **A skill is disabled by every name it answers to** (review 29): `/x` is
/// `x`; a plain entry holds a plugin's `p:x`, a qualified one only its own;
/// its frontmatter name counts as its directory's; an entry is kept as
/// written, so `-=` can take it back. A skill disabled by its place does
/// not refuse a usable one of that name elsewhere.
#[test]
fn a_skill_is_disabled_by_every_name_it_answers_to() {
    let s = Space::new();
    let skills_dir = s.config().join("skills");
    s.write(
        &skills_dir.join("rev/SKILL.md"),
        "---\nname: review-it\ndescription: Review\n---\nbody",
    );
    let vendor = s.config().join("vendor");
    s.write(
        &vendor.join("lint/SKILL.md"),
        "---\nname: lint\ndescription: Lint\n---\nbody",
    );
    let claudes = s
        .roots
        .home
        .as_ref()
        .unwrap()
        .join(".claude/skills/lint/SKILL.md");
    s.write(
        &claudes,
        "---\nname: lint\ndescription: Claude's lint\n---\nbody",
    );
    let skills = |disabled: &[&str]| SkillsConfig {
        paths: vec!["{config}/skills".into(), "{config}/vendor".into()],
        disabled: disabled.iter().map(|d| d.to_string()).collect(),
    };
    let memory = MemoryConfig::default();

    let f = found(
        &s,
        &memory,
        &skills(&[" review ", "other:ship"]),
        Session::Api,
    );
    let set = &f.disabled;
    assert_eq!(
        set.skill_named("/review"),
        Some(" review "),
        "kept as written"
    );
    assert_eq!(set.skill_named("plugin:review"), Some(" review "));
    assert_eq!(
        set.skill_named("ship"),
        None,
        "a qualified entry holds its own"
    );
    assert_eq!(set.skill_named("plugin:ship"), None);
    assert_eq!(set.skill_named("other:ship"), Some("other:ship"));
    assert_eq!(set.skill_named("/other:ship"), Some("other:ship"));

    for by in ["review-it", "rev"] {
        let f = found(&s, &memory, &skills(&[by]), Session::Api);
        let rev = f.items.iter().find(|i| i.name == "rev").unwrap();
        assert_eq!(rev.alias.as_deref(), Some("review-it"));
        assert_eq!(rev.state, State::Disabled, "by {by}");
        assert_eq!(f.skill_disabled("review-it").as_deref(), Some(by));
    }

    let f = found(&s, &memory, &skills(&["{config}/vendor"]), Session::Api);
    assert_eq!(f.skill_disabled("lint").as_deref(), Some("{config}/vendor"));
    let f = found(
        &s,
        &memory,
        &skills(&["{config}/vendor"]),
        Session::ClaudeCode,
    );
    let theirs = f.items.iter().find(|i| i.path == claudes).unwrap();
    assert_eq!(theirs.state, State::ClaudeCode, "the premise");
    assert_eq!(f.skill_disabled("lint"), None, "Claude Code's is usable");
}

/// **A new memory or skill: a template, kept only if written** — left as it
/// was it is taken away (save-to-apply); saved, a memory's line lands in its
/// index, made `0600` if there was none; a name already there is refused.
#[test]
fn a_draft_is_kept_only_if_written() {
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(slug("Likes Green!").as_deref(), Some("likes-green"));
    assert_eq!(
        slug(" ../../etc ").as_deref(),
        Some("etc"),
        "no path in a name"
    );
    assert_eq!(slug("--").as_deref(), None);

    let s = Space::new();
    let dir = s.config().join("projects/proj-1234abcd/memory");
    let draft = draft_memory(&dir, "Likes green").unwrap();
    draft.write().unwrap();
    assert_eq!(draft.path, dir.join("likes-green.md"));
    let said = draft.settle().unwrap();
    assert!(
        said.contains("nothing made") && said.contains("left as written"),
        "{said}"
    );
    assert!(!draft.path.exists());

    let draft = draft_memory(&dir, "Likes green").unwrap();
    draft.write().unwrap();
    let mode = std::fs::metadata(&draft.path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    std::fs::write(
        &draft.path,
        "---\nname: Likes green\ndescription: the user's colour\ntype: user\n---\n\nGreen.\n",
    )
    .unwrap();
    let said = draft.settle().unwrap();
    assert!(
        said.starts_with("made") && said.contains("its line in"),
        "{said}"
    );
    let index = dir.join("MEMORY.md");
    assert_eq!(
        std::fs::read_to_string(&index).unwrap(),
        "- [Likes green](likes-green.md) — the user's colour\n"
    );
    assert_eq!(
        std::fs::metadata(&index).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        draft_memory(&dir, "likes green")
            .unwrap()
            .write()
            .unwrap_err()
            .contains("already")
    );

    let skills = s.config().join("skills");
    let draft = draft_skill(&skills, "Review").unwrap();
    draft.write().unwrap();
    assert_eq!(draft.path, skills.join("review/SKILL.md"));
    for made in [&skills, &skills.join("review")] {
        let mode = std::fs::metadata(made).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{made:?}");
    }
    assert!(draft.settle().unwrap().contains("nothing made"));
    assert!(
        !skills.join("review").exists(),
        "its directory taken away too"
    );
}

/// **What an edit came to, said as it is** (review 29, M5): a draft the
/// editor deleted is nothing made — no `- [](gone.md) — ` line told every
/// turn — and a skill's directory goes with it; a memory with no
/// description gets no index line, and says so; an index that is a link is
/// not written through; a draft that cannot be read back is left alone,
/// its index unchanged; `memory` is its index's own name (on macOS,
/// `memory.md` IS `MEMORY.md`).
#[test]
fn a_draft_says_what_it_came_to() {
    let s = Space::new();
    let dir = s.config().join("projects/proj-1234abcd/memory");
    let index = dir.join("MEMORY.md");
    let draft = draft_memory(&dir, "gone").unwrap();
    draft.write().unwrap();
    std::fs::remove_file(&draft.path).unwrap();
    let said = draft.settle().unwrap();
    assert!(said.contains("deleted in the editor"), "{said}");
    assert!(!index.exists(), "no line for a file not there");

    let skills = s.config().join("skills");
    let draft = draft_skill(&skills, "gone").unwrap();
    draft.write().unwrap();
    std::fs::remove_file(&draft.path).unwrap();
    assert!(draft.settle().unwrap().contains("deleted in the editor"));
    assert!(!skills.join("gone").exists(), "its directory taken away");

    let draft = draft_memory(&dir, "bare").unwrap();
    draft.write().unwrap();
    std::fs::write(
        &draft.path,
        "---\nname: bare\ndescription:\n---\n\nA fact.\n",
    )
    .unwrap();
    let said = draft.settle().unwrap();
    assert!(
        said.contains("needs a `name` and a `description`"),
        "{said}"
    );
    assert!(draft.path.exists() && !index.exists());

    let elsewhere = s.write(&s.config().join("elsewhere.md"), "theirs\n");
    std::os::unix::fs::symlink(&elsewhere, &index).unwrap();
    let draft = draft_memory(&dir, "linked").unwrap();
    draft.write().unwrap();
    std::fs::write(&draft.path, "---\nname: l\ndescription: d\n---\nx").unwrap();
    let said = draft.settle().unwrap();
    assert!(said.contains("its index NOT changed"), "{said}");
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "theirs\n");
    std::fs::remove_file(&index).unwrap();

    // Unreadable to anyone — a directory where the draft was — since a
    // `0o000` file is read by root, where this failed (review stop 12, T9).
    let draft = draft_memory(&dir, "locked").unwrap();
    draft.write().unwrap();
    std::fs::remove_file(&draft.path).unwrap();
    std::fs::create_dir(&draft.path).unwrap();
    let why = draft.settle().unwrap_err();
    assert!(why.contains("could not be read back"), "{why}");
    assert!(draft.path.exists() && !index.exists(), "left alone");

    for name in ["memory", "MEMORY", " Memory "] {
        assert!(
            draft_memory(&dir, name).unwrap_err().contains("index"),
            "{name}"
        );
    }
}

/// **A new skill goes to a place of yours, never a repository's** — the
/// first `[skills]` location that is placed and one place, whatever comes
/// before it (Reviewer C's M9: held only by the defaults' order).
#[test]
fn a_new_skill_never_goes_to_a_repositorys_place() {
    let s = Space::new();
    std::fs::create_dir_all(s.root.join(".shannon/skills")).unwrap();
    let skills = |paths: &[&str]| SkillsConfig {
        paths: paths.iter().map(|p| p.to_string()).collect(),
        ..SkillsConfig::default()
    };
    assert_eq!(
        skill_home(&skills(&[".shannon/skills", "{config}/skills"]), &s.roots),
        Some(s.config().join("skills"))
    );
    assert_eq!(skill_home(&skills(&[".shannon/skills"]), &s.roots), None);
}

/// **Removing takes exactly what it says** — a memory and its index lines
/// (the others, and the file's last newline, kept), a skill's directory
/// whole, a linked memory unlinked and what it named left; never an
/// instruction file, an index, a location of its own, or a file not read.
#[test]
fn a_removal_takes_what_it_says_and_no_more() {
    let s = Space::new();
    let memories = s.config().join("projects/proj-1234abcd/memory");
    // CRLF kept, and a line that links another memory too kept: taking it
    // would take that one out of the index (review 29, Reviewer B).
    let index = s.write(
        &memories.join("MEMORY.md"),
        "# Index\r\n- [old](old.md) — stale\r\n- [new](new.md) — fresh\r\n- [again](./old.md)\r\n\
         - [pair](old.md) with [new](new.md)\r\n",
    );
    let old = s.write(&memories.join("old.md"), "x");
    let new = s.write(&memories.join("new.md"), "y");
    let elsewhere = s.write(&s.root.join("kept.md"), "z");
    std::os::unix::fs::symlink(&elsewhere, memories.join("linked.md")).unwrap();
    let skill = s.write(
        &s.config().join("skills/review/SKILL.md"),
        "---\nname: review\n---\n",
    );
    s.write(&s.config().join("skills/review/notes.md"), "notes");
    let agents = s.write(&s.root.join("AGENTS.md"), "be terse");
    let memory = MemoryConfig::default();
    let skills = SkillsConfig::default();
    let f = found(&s, &memory, &skills, Session::Api);
    let item = |path: &Path| f.items.iter().find(|i| i.path == path).unwrap().clone();
    let plan = |path: &Path| removal(&item(path), &s.roots, &memory, &skills);

    let r = plan(&old).unwrap();
    assert_eq!(r.index, Some((index.clone(), 2)));
    assert!(
        r.question("old.md").contains("its 2 lines in"),
        "{}",
        r.question("old.md")
    );
    r.apply().unwrap();
    assert!(!old.exists() && new.exists());
    assert_eq!(
        std::fs::read_to_string(&index).unwrap(),
        "# Index\r\n- [new](new.md) — fresh\r\n- [pair](old.md) with [new](new.md)\r\n"
    );

    let r = plan(&skill).unwrap();
    assert_eq!(
        (r.target.clone(), r.files),
        (skill.parent().unwrap().to_path_buf(), 2)
    );
    r.apply().unwrap();
    assert!(!skill.parent().unwrap().exists());

    plan(&memories.join("linked.md")).unwrap().apply().unwrap();
    assert!(
        !memories.join("linked.md").exists() && elsewhere.exists(),
        "unlinked, not followed"
    );

    for (refused, why) in [(&agents, "instruction"), (&index, "index")] {
        let said = plan(refused).unwrap_err();
        assert!(said.contains(why), "{said}");
    }
    let holds = |said: Result<Removal, String>| {
        let said = said.unwrap_err();
        assert!(said.contains("holds a location you configured"), "{said}");
    };

    // A memory a SECOND location names, in either order (review 29,
    // Reviewer B: the found-by location alone was checked).
    let notes = s.root.join("notes");
    let solo = s.write(&notes.join("solo.md"), "a memory named outright");
    for paths in [
        vec![notes.display().to_string(), solo.display().to_string()],
        vec![solo.display().to_string(), notes.display().to_string()],
    ] {
        let memory = MemoryConfig {
            paths,
            ..MemoryConfig::default()
        };
        let f = found(&s, &memory, &skills, Session::Api);
        let solo_item = f.items.iter().find(|i| i.path == solo).unwrap();
        holds(removal(solo_item, &s.roots, &memory, &skills));
    }

    // A skill whose directory IS a location, and one configured as its
    // `SKILL.md` FILE — `d` deleted the tool it sat in (review 29,
    // Reviewers A and B: HIGH).
    let whole = s.root.join("one-skill");
    let own = s.write(&whole.join("SKILL.md"), "---\nname: one-skill\n---\n");
    let tool = s.root.join("tools/foo");
    let file_skill = s.write(&tool.join("SKILL.md"), "---\nname: foo\n---\n");
    s.write(&tool.join("src/main.rs"), "fn main() {}");
    for (location, skill) in [(&whole, &own), (&file_skill, &file_skill)] {
        let skills = SkillsConfig {
            paths: vec![location.display().to_string()],
            ..SkillsConfig::default()
        };
        let f = found(&s, &MemoryConfig::default(), &skills, Session::Api);
        let it = f.items.iter().find(|i| &i.path == skill).expect("found");
        holds(removal(it, &s.roots, &MemoryConfig::default(), &skills));
    }
    assert!(tool.join("src/main.rs").exists());

    // Not read is not removed (a credential path, here by the judge).
    let secret = s.write(&memories.join("secret.md"), "k");
    let judge = |p: &Path| {
        p.ends_with("secret.md")
            .then(|| "a credential path (test) — never read".to_string())
    };
    let f = find(&memory, &skills, &s.roots, Session::Api, &judge);
    let it = f.items.iter().find(|i| i.path == secret).unwrap();
    let said = removal(it, &s.roots, &memory, &skills).unwrap_err();
    assert!(said.contains("not read"), "{said}");
}

/// **What a link leads to is what a removal deletes, and a repository's
/// location that links out of it is not removed** — in a Claude Code
/// session too, where such a file is `claude's` rather than refused
/// (review 29, Reviewer B: HIGH, `d` deleted outside while the question
/// named a path inside). A directory counted past the cap says so; an index
/// that cannot be read is said, its lines left.
#[test]
fn a_removal_names_where_it_deletes_and_stays_in_the_repository() {
    let s = Space::new();
    let outside = s._dir.path().join("shared-skills");
    s.write(&outside.join("foo/SKILL.md"), "---\nname: foo\n---\n");
    std::fs::create_dir_all(s.root.join(".claude")).unwrap();
    std::os::unix::fs::symlink(&outside, s.root.join(".claude/skills")).unwrap();
    let memory = MemoryConfig::default();
    let skills = SkillsConfig::default();
    let f = found(&s, &memory, &skills, Session::ClaudeCode);
    let foo = s.root.join(".claude/skills/foo/SKILL.md");
    let it = f.items.iter().find(|i| i.path == foo).expect("found");
    let said = removal(it, &s.roots, &memory, &skills).unwrap_err();
    assert!(said.contains("leads out of the repository"), "{said}");
    assert!(outside.join("foo/SKILL.md").exists());

    // Through a linked place of YOURS: removable, the question naming where.
    let real_home = s._dir.path().join("real-skills");
    s.write(&real_home.join("bar/SKILL.md"), "---\nname: bar\n---\n");
    for i in 0..1100 {
        s.write(&real_home.join(format!("bar/many/{i}.md")), "x");
    }
    std::os::unix::fs::symlink(&real_home, s.config().join("skills")).unwrap();
    let f = found(&s, &memory, &skills, Session::Api);
    let bar = s.config().join("skills/bar/SKILL.md");
    let it = f.items.iter().find(|i| i.path == bar).expect("found");
    let r = removal(it, &s.roots, &memory, &skills).unwrap();
    let asked = r.question("bar");
    let real_bar = std::fs::canonicalize(&real_home).unwrap().join("bar");
    assert!(
        asked.contains(&format!("(→ {})", real_bar.display())),
        "{asked}"
    );
    assert!(r.capped && asked.contains("+ files"), "{asked}");

    // An index that is a link is not read, and said.
    let memories = s.config().join("projects/proj-1234abcd/memory");
    let real_index = s.write(&s.root.join("elsewhere.md"), "- [m](m.md)\n");
    std::fs::create_dir_all(&memories).unwrap();
    std::os::unix::fs::symlink(&real_index, memories.join("MEMORY.md")).unwrap();
    let m = s.write(&memories.join("m.md"), "x");
    let f = found(&s, &memory, &skills, Session::Api);
    let it = f.items.iter().find(|i| i.path == m).expect("found");
    let asked = removal(it, &s.roots, &memory, &skills)
        .unwrap()
        .question("m.md");
    assert!(
        asked.contains("is not read") && asked.contains("its lines stay"),
        "{asked}"
    );
}

/// **The project's root is what `[memory] project_root` marks** (Steven,
/// 10-07, gutentags' markers): the walk stops at the NEAREST directory
/// holding any marker, a file or a directory; with none found, or none
/// listed, it is where Shannon started alone — and the repository a
/// relative location must stay inside moves with it.
#[test]
fn the_project_root_is_what_its_markers_mark() {
    let mut s = Space::new();
    let a = s.root.join("a");
    let walked = |s: &Space| locate::places("AGENTS.md", &s.roots);
    // The default list holds Mercurial's, nearer than the `.git` above.
    std::fs::create_dir_all(a.join(".hg")).unwrap();
    assert_eq!(walked(&s), [a.join("AGENTS.md"), a.join("b/AGENTS.md")]);
    assert_eq!(locate::repository(&s.roots), resolved(&a));
    // Only `.git`: the walk passes `.hg` and goes on to the repository.
    s.roots.project_root = vec![".git".into()];
    assert_eq!(walked(&s).len(), 3, "{:?}", walked(&s));
    assert_eq!(locate::repository(&s.roots), resolved(&s.root));
    // A marker that is a FILE (Fossil's checkout is one).
    s.write(&a.join("b/.fslckout"), "");
    s.roots.project_root = vec![".fslckout".into()];
    assert_eq!(walked(&s), [a.join("b/AGENTS.md")]);
    // Listed but found nowhere, and none listed: where Shannon started.
    for none in [vec!["_darcs".to_string()], Vec::new()] {
        s.roots.project_root = none;
        assert_eq!(walked(&s), [s.roots.start.join("AGENTS.md")]);
        assert_eq!(locate::repository(&s.roots), resolved(&s.roots.start));
    }
}

/// **`follow` takes both of `[memory]`'s answers about the roots** — the
/// one derivation launch and a config change share.
#[test]
fn roots_follow_the_memory_config() {
    let mut s = Space::new();
    let memory = MemoryConfig {
        project_root: vec!["_darcs".into()],
        ..MemoryConfig::default()
    };
    s.roots.follow(&memory);
    assert_eq!(s.roots.project_root, ["_darcs"]);
    assert_eq!(
        s.roots.project,
        project_name(&s.roots.start, memory.project)
    );
}
