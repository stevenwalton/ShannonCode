//! **Memory and skills through the app** — what a turn tells the model, what
//! the transcript says it told, and the rules the locations put in force.
//! Every test here builds its roots in a temporary directory: none reads the
//! user's config, home, or this repository's `AGENTS.md`.

use super::*;
use shannon_core::knowledge::{Roots, Session};
use shannon_core::{ContextKind, SendRequest};
use std::path::PathBuf;

type Sent = Arc<std::sync::Mutex<Vec<SendRequest>>>;

/// An app told from a temporary workspace: a project (a repository) where
/// it starts, a config directory, a home.
struct Known {
    app: App,
    sent: Sent,
    _dir: tempfile::TempDir,
    start: PathBuf,
    config: PathBuf,
}

impl Known {
    fn new() -> Known {
        Self::with(Config::default())
    }

    fn with(config: Config) -> Known {
        let backend = Arc::new(shannon_core::backend::RecordingStub::new());
        let sent = backend.requests.clone();
        let mut app = test_app_backed(backend, Arc::new(config));
        let dir = tempfile::tempdir().unwrap();
        let start = dir.path().join("proj");
        let config = dir.path().join("config");
        let home = dir.path().join("home");
        for d in [start.join(".git"), config.clone(), home.clone()] {
            std::fs::create_dir_all(d).unwrap();
        }
        // A project you work in: trusted, so its `.shannon/` is read (a
        // `.shannon/` is read only where its directory is — Steven, 10-13).
        app.trusted
            .trust(&start, crate::trust::Lasting::ThisRun)
            .unwrap();
        let said = app.set_knowledge_roots(Roots {
            start: start.clone(),
            config_dir: Some(config.clone()),
            project: "proj-1234abcd".into(),
            // Unset `CLAUDE_CONFIG_DIR`'s answer: the home's `.claude`.
            claude_home: Some(home.join(".claude")),
            home: Some(home),
            claude_memory: Some(dir.path().join("claude/projects/-proj/memory")),
            project_root: shannon_core::config::MemoryConfig::default().project_root,
        });
        assert!(said.is_empty(), "{said:?}");
        Known {
            app,
            sent,
            _dir: dir,
            start,
            config,
        }
    }

    fn write(&self, path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// One turn, sent and finished; what its prompt carried.
    fn turn(&mut self, prompt: &str) -> Vec<(ContextKind, String, String)> {
        self.app.submit(prompt);
        let told = told_in(self.sent.lock().unwrap().last().expect("sent"));
        self.app.feed_stream(StreamMsg::Complete);
        told
    }

    fn memory_dir(&self) -> PathBuf {
        self.config.join("projects/proj-1234abcd/memory")
    }

    /// A skill at `<start>/.shannon/skills/<name>/SKILL.md`.
    fn skill(&self, name: &str, description: &str, body: &str) -> PathBuf {
        let path = self.start.join(format!(".shannon/skills/{name}/SKILL.md"));
        let text = format!("---\nname: {name}\ndescription: {description}\n---\n{body}");
        self.write(&path, &text);
        path
    }
}

/// **A `.shannon/` is read only where its directory is trusted** (Steven,
/// 10-13: "Trust should gate `.shannon/*`"): the repository's skill in
/// `.shannon/skills` — a default location — is listed and refused, saying
/// how to read it, cannot be invoked, and reaches no turn; trusted, it is
/// read.
#[tokio::test]
async fn a_shannon_dir_is_read_only_where_trusted() {
    use shannon_core::knowledge::State;
    // The premise, apart: trusted, the skill is told a turn — so "reaches
    // no turn" below measures the trust (review 2026-10-13, C).
    let mut trusted = Known::new();
    trusted.skill("tidy", "tidies the tree", "the steps");
    let told = trusted.turn("hi");
    assert!(
        told.iter()
            .any(|(_, _, text)| text.contains("tidies the tree")),
        "premise: {told:?}"
    );
    let mut k = Known::new();
    let path = k.skill("tidy", "tidies the tree", "the steps");
    let item = |app: &App| {
        (app.found(Session::Api).expect("roots").items.into_iter())
            .find(|i| i.path == path)
            .expect("listed")
    };
    assert!(
        !matches!(item(&k.app).state, State::Refused(_)),
        "premise: trusted, read"
    );
    k.app.trusted.revoke(&k.start).unwrap();
    match item(&k.app).state {
        State::Refused(why) => assert!(
            why.contains("not trusted") && why.contains("`:trust "),
            "{why}"
        ),
        other => panic!("read untrusted: {other:?}"),
    }
    let found = k.app.found(Session::Api).unwrap();
    assert!(found.skill("tidy").is_none(), "cannot be invoked");
    let told = k.turn("hi");
    assert!(
        !told
            .iter()
            .any(|(_, _, text)| text.contains("tidies the tree")),
        "{told:?}"
    );
}

/// The open `:memories`/`:skills` screen as text — empty when none is.
fn shelf(app: &App) -> String {
    app.setup().map(|s| s.shown(74, 60)).unwrap_or_default()
}

/// One key to the open screen, through the event loop's own door, in its
/// order (`handle_event`: the moment begins, then the screen) — and the
/// file `$EDITOR` was asked to open, if any (the editor here opens nothing).
/// Without the moment, no shelf test could see notices pile up under it
/// (review 2026-10-13, A).
fn shelf_key(app: &mut App, code: KeyCode) -> Option<PathBuf> {
    let mut opened = None;
    let key = Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
    if app.begin_moment(&key) {
        app.setup_event(&key, |p| {
            opened = Some(p.to_path_buf());
            Ok(())
        });
    }
    opened
}

/// **A float says each key's outcome itself, and the row only the last**
/// (review 2026-10-13, A): keys kept the row's notices under every screen,
/// so `x x q` on `:memories` left "disabled · enabled" behind it when it
/// closed. Only a screen over the whole frame keeps them.
#[test]
fn a_floats_keys_leave_no_pile_on_the_row() {
    let mut k = Known::new();
    k.write(&k.memory_dir().join("MEMORY.md"), "- [a](a.md)");
    k.write(&k.memory_dir().join("a.md"), "---\ndescription: d\n---\nx");
    k.app.dispatch_command_for_test("memories");
    let on_a = |k: &Known| {
        k.app
            .setup()
            .and_then(|s| s.shelf_selected())
            .is_some_and(|p| p.ends_with("a.md"))
    };
    for _ in 0..10 {
        if on_a(&k) {
            break;
        }
        shelf_key(&mut k.app, KeyCode::Char('j'));
    }
    assert!(on_a(&k), "premise: the cursor on a.md");
    shelf_key(&mut k.app, KeyCode::Char('x'));
    shelf_key(&mut k.app, KeyCode::Char('x'));
    let said = k.app.notice().unwrap_or_default();
    assert!(
        said.contains("enabled for this session") && !said.contains("disabled for this session"),
        "the last key's outcome, not the pile: {said}"
    );
}

/// **A prefix armed on the shelf owns the next key** — `fx` is a find (and
/// a row has no columns, so it is refused), not `f` then the shelf's `x`,
/// which disabled the memory under the cursor and left `f` armed to eat the
/// `j` after (review stop 16, A M3).
#[test]
fn a_find_on_a_shelf_takes_its_char_not_the_shelfs_verb() {
    let mut k = Known::new();
    k.write(
        &k.memory_dir().join("MEMORY.md"),
        "- [a](a.md)\n- [b](b.md)",
    );
    k.write(&k.memory_dir().join("a.md"), "---\ndescription: d\n---\nx");
    k.write(&k.memory_dir().join("b.md"), "---\ndescription: e\n---\ny");
    k.app.dispatch_command_for_test("memories");
    let selected = |k: &Known| k.app.setup().and_then(|s| s.shelf_selected());
    let before = selected(&k);
    shelf_key(&mut k.app, KeyCode::Char('f'));
    shelf_key(&mut k.app, KeyCode::Char('x'));
    let said = k.app.notice().unwrap_or_default();
    assert!(
        !said.contains("disabled"),
        "`x` was the find's char: {said}"
    );
    shelf_key(&mut k.app, KeyCode::Char('j'));
    assert_ne!(
        selected(&k),
        before,
        "the `j` after moved: nothing left armed"
    );
}

/// The context blocks on the newest user message of `req`.
fn told_in(req: &SendRequest) -> Vec<(ContextKind, String, String)> {
    let user = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .expect("a user message");
    user.blocks
        .iter()
        .filter_map(|b| match b {
            shannon_core::Block::Context { kind, source, text } => {
                Some((*kind, source.clone(), text.clone()))
            }
            _ => None,
        })
        .collect()
}

/// **A test app tells nothing** — no roots — though this repository, where
/// the tests run, holds an `AGENTS.md` (the seam in `sandboxed`).
#[tokio::test]
async fn a_test_app_tells_nothing() {
    let backend = Arc::new(shannon_core::backend::RecordingStub::new());
    let sent = backend.requests.clone();
    let mut app = test_app_backed(backend, Arc::new(Config::default()));
    app.submit("hi");
    assert!(told_in(sent.lock().unwrap().last().unwrap()).is_empty());
}

/// **A session's first turn tells everything; later ones what changed** —
/// an edited file again, whole; a deleted one as no longer loaded — and the
/// transcript says what each turn told.
#[tokio::test]
async fn a_turn_tells_what_is_new_and_only_that() {
    let mut k = Known::new();
    let agents = k.start.join("AGENTS.md");
    k.write(&agents, "be terse");
    let first = k.turn("hi");
    let sources: Vec<&str> = first.iter().map(|(_, s, _)| s.as_str()).collect();
    assert_eq!(
        sources,
        [agents.display().to_string().as_str(), "shannon:memory"],
        "the instructions and how to keep memories"
    );
    assert_eq!(first[0].2, "be terse");
    assert!(first[1].2.contains(&k.memory_dir().display().to_string()));
    let shown = k.app.output().lines().join("\n");
    assert!(
        shown.contains("⚙ Shannon told the model: AGENTS.md · how to keep memories"),
        "{shown}"
    );

    assert!(k.turn("again").is_empty(), "nothing new");

    k.write(&agents, "be very terse");
    let edited = k.turn("and again");
    assert_eq!(edited.len(), 1);
    assert_eq!(
        (edited[0].0, edited[0].2.as_str()),
        (ContextKind::Instructions, "be very terse")
    );

    std::fs::remove_file(&agents).unwrap();
    let gone = k.turn("once more");
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].0, ContextKind::Unloaded);
    assert!(
        k.app
            .output()
            .lines()
            .join("\n")
            .contains("no longer AGENTS.md")
    );
}

/// **A memory written mid-session is told the next turn** — the index the
/// model wrote to, read back — and a value marked secret in a file leaves as
/// its handle, never twice for being unchanged.
#[tokio::test]
async fn a_memory_written_is_told_the_next_turn_masked() {
    let mut k = Known::new();
    k.turn("hi");
    let index = k.memory_dir().join("MEMORY.md");
    k.write(&index, "- [deploy](deploy.md) — the key is hunter2");
    k.app.dispatch_command_for_test("secret hunter2");
    let told = k.turn("what do you remember");
    let memory = told
        .iter()
        .find(|(kind, _, _)| *kind == ContextKind::Memory)
        .expect("the index");
    assert!(!memory.2.contains("hunter2"), "masked: {}", memory.2);
    // Recorded masked too — the session and its file on disk — not only on
    // the wire, where the outbound door masks everything anyway.
    let recorded = format!("{:?}", k.app.buffer().conversation.messages());
    assert!(recorded.contains("the key is") && !recorded.contains("hunter2"));
    assert!(k.turn("again").is_empty(), "a masked file compares equal");
}

/// **`:Send` and a `:child` tell as the prompt does** — every door that
/// starts a turn.
#[tokio::test]
async fn every_turn_starter_tells() {
    let mut k = Known::new();
    k.write(&k.start.join("SHANNON.md"), "use the house style");
    k.turn("hi");
    // The child's own history is empty: told everything, as a session is.
    k.app.spawn_child_job("look around", None).expect("spawned");
    let child = told_in(k.sent.lock().unwrap().last().expect("the child's request"));
    assert!(
        child
            .iter()
            .any(|(_, _, text)| text == "use the house style"),
        "{child:?}"
    );
    // …and its own transcript says so, as the prompt's does (Reviewer A,
    // round 27: deleting the line at either starter passed the suite).
    let shows = |app: &App, buffer: BufferId| {
        app.session_of(buffer)
            .expect("its session")
            .output
            .lines()
            .join("\n")
            .contains("⚙ Shannon told the model: SHANNON.md")
    };
    let id = *k.app.generation_ids_for_test().last().expect("the child");
    let child_buffer = k
        .app
        .generations
        .iter()
        .find(|g| g.id == id)
        .unwrap()
        .buffer;
    assert!(shows(&k.app, child_buffer), "the child's transcript");

    let mut k = Known::new();
    k.write(&k.start.join("SHANNON.md"), "use the house style");
    let target = k.app.prompt_target();
    k.app.start_job_on(target, "do it").expect("sent");
    let job = told_in(k.sent.lock().unwrap().last().expect("the job's request"));
    assert!(
        job.iter().any(|(_, _, text)| text == "use the house style"),
        "{job:?}"
    );
    assert!(shows(&k.app, target), "the sent-to session's transcript");
}

/// **A reopened session shows what it told** — the replay draws the same
/// line the live turn did.
#[tokio::test]
async fn a_replayed_turn_shows_what_it_told() {
    let mut k = Known::new();
    k.write(&k.start.join("AGENTS.md"), "x");
    k.turn("hi");
    let turn = k
        .app
        .graph_reading(&[k.app.buffer().conversation.episode().id])
        .unwrap()
        .records(&k.app.buffer().conversation.episode().id)
        .and_then(|mut r| r.pop())
        .expect("a committed turn")
        .clone();
    let mut output = Output::new();
    App::replay_turn(&shannon_core::Aliases::default(), None, &mut output, &turn);
    assert!(
        output
            .lines()
            .join("\n")
            .contains("⚙ Shannon told the model: AGENTS.md"),
        "{:?}",
        output.lines()
    );
}

/// **The locations are in force as rules of their own**, listed apart in
/// `:rules`; `:set memory=off` takes the write rule away at once.
#[tokio::test]
async fn the_locations_are_rules_listed_apart() {
    let mut k = Known::new();
    let write = k.memory_dir().display().to_string();
    assert!(k.memory_dir().is_dir(), "made at launch");
    let decide = |app: &App, line: &str| app.rules.decide_line(line).decision;
    let echo = format!("echo hi > {write}/m.md");
    assert_eq!(
        decide(&k.app, &echo),
        shannon_core::permission::Decision::Allow
    );
    let listing = k.app.rules_listing();
    let section = listing
        .split("FROM [memory] AND [skills]")
        .nth(1)
        .expect("the section");
    // The place as a rule writes it: a directory holds what is below it.
    assert!(
        section
            .lines()
            .any(|l| l.trim_end().ends_with(&format!("allow :write {write}"))),
        "{listing}"
    );
    k.app.dispatch_command_for_test("set memory=off");
    assert_eq!(
        decide(&k.app, &echo),
        shannon_core::permission::Decision::Ask
    );
    assert!(!k.app.rules_listing().contains("allow :write"));

    // Off from launch, then on: the write location is made and vouched for
    // when it is set, not only at launch.
    let mut off = Config::default();
    off.memory.mode = shannon_core::MemoryMode::Off;
    let mut k = Known::with(off);
    assert!(!k.memory_dir().exists(), "off: nothing made");
    k.app.dispatch_command_for_test("set memory=auto");
    assert!(k.memory_dir().is_dir(), "made when set");
    let echo = format!("echo hi > {}/m.md", k.memory_dir().display());
    assert_eq!(
        decide(&k.app, &echo),
        shannon_core::permission::Decision::Allow
    );
}

/// **Naming the project again moves its places at once** — `{project}` is
/// derived again when `memory.project` changes (one derivation, launch's),
/// so the write place is made, vouched and approved where the new name puts
/// it, and the old one is no project's now.
#[test]
fn naming_the_project_again_moves_its_places() {
    use shannon_core::permission::Decision;
    let mut k = Known::new();
    let before = k.memory_dir();
    assert!(before.is_dir(), "premise: the launch's place");
    k.app
        .dispatch_command_for_test("set memory.project=claude-code");
    let name = k.app.knowledge_roots.as_ref().unwrap().project.clone();
    assert!(name.ends_with("-proj") && !name.contains('/'), "{name}");
    let after = k.config.join("projects").join(&name).join("memory");
    assert!(after.is_dir(), "made and vouched");
    let write = |dir: &Path| {
        let line = format!("echo hi > {}/m.md", dir.display());
        k.app.rules.decide_line(&line).decision
    };
    assert_eq!(write(&after), Decision::Allow);
    assert_ne!(write(&before), Decision::Allow);
}

/// **Changing what marks the project's root moves the walk at once** — the
/// next turn is told what the new root holds (`Roots::follow`, launch's
/// derivation, run again by `adopt_config`).
#[tokio::test]
async fn changing_the_project_root_markers_moves_the_walk() {
    let mut k = Known::new();
    let outer = k.start.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(outer.join(".hg")).unwrap();
    k.write(&outer.join("AGENTS.md"), "the outer rule");
    let told = |k: &mut Known| {
        k.turn("hi")
            .iter()
            .any(|(_, _, text)| text.contains("the outer rule"))
    };
    // `proj` holds `.git`: the root is where Shannon started.
    assert!(!told(&mut k), "premise: the outer file is above the root");
    k.app
        .dispatch_command_for_test("set memory.project_root=.hg");
    assert_eq!(
        k.app.knowledge_roots.as_ref().unwrap().project_root,
        [".hg"]
    );
    assert!(told(&mut k), "the walk now reaches `.hg`'s directory");
}

/// A `skill` call as the stream delivers one.
fn skill_block(id: &str, name: &str, arguments: &str) -> shannon_core::Block {
    shannon_core::Block::ToolUse {
        id: id.into(),
        name: shannon_core::SKILL_TOOL.into(),
        input: serde_json::json!({ "name": name, "arguments": arguments }),
    }
}

/// The result answering `call`, wherever it is: pending, or returned into any
/// session's history (a job's round lands in its own).
fn answered(app: &App, call: &str) -> Option<(String, bool)> {
    let returned = app.buffers.sessions().flat_map(|b| {
        b.conversation
            .messages()
            .iter()
            .flat_map(|m| m.blocks.iter())
    });
    app.tool_results
        .values()
        .chain(returned)
        .find_map(|b| match b {
            shannon_core::Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } if tool_use_id == call => Some((content.clone(), *is_error)),
            _ => None,
        })
}

/// **The model loads a skill by name** — answered at once with its body (the
/// frontmatter dropped, `$ARGUMENTS` filled), never gated, and recorded in
/// the transcript; a name that is no skill is an error naming the ones there
/// are.
#[tokio::test]
async fn the_model_loads_a_skill_by_name() {
    let mut k = Known::new();
    k.skill("review", "Review the diff", "Check $ARGUMENTS carefully.");
    k.app.conversation_mut().begin_turn("go");
    // Each round ends as a stream ends it (`Complete`): the call fed without
    // it made a round no backend sends (review stop 12, T4 8).
    k.app
        .feed_stream(StreamMsg::ToolUse(skill_block("tu_1", "review", "src/")));
    k.app.feed_stream(StreamMsg::Complete);
    assert!(k.app.shown_call().is_none(), "runs nothing: no decision");
    let (body, error) = answered(&k.app, "tu_1").expect("answered");
    assert!(!error, "{body}");
    assert!(body.contains("Check src/ carefully."), "{body}");
    assert!(
        !body.contains("description:"),
        "frontmatter dropped: {body}"
    );
    let shown = k.app.output().lines().join("\n");
    assert!(shown.contains("skill review src/"), "{shown}");

    k.app
        .feed_stream(StreamMsg::ToolUse(skill_block("tu_2", "nope", "")));
    k.app.feed_stream(StreamMsg::Complete);
    let (why, error) = answered(&k.app, "tu_2").expect("answered");
    assert!(error);
    assert!(
        why.contains("no skill named `nope`") && why.contains("review"),
        "{why}"
    );
    // Recorded as what happened, not as a load (Reviewer A, round 27).
    let shown = k.app.output().lines().join("\n");
    assert!(shown.contains("NOT LOADED"), "{shown}");
}

/// **A `:child` loads a skill without being held** — an unknown tool holds an
/// unwatched child; a skill runs nothing, so its answer is the same with
/// nobody watching.
#[tokio::test]
async fn a_child_loads_a_skill_without_being_held() {
    let mut k = Known::new();
    k.skill("review", "Review the diff", "Check it.");
    k.turn("hi"); // a child hangs off a turn
    k.app.spawn_child_job("look around", None).expect("spawned");
    let id = *k.app.generation_ids_for_test().last().expect("the child's");
    let Some(Work::Job(job)) = k.app.work_mut(id) else {
        panic!("premise: a job")
    };
    let episode = job.episode;
    k.app
        .gate_job_tool_call(id, episode, skill_block("tu_c", "review", ""));
    assert!(
        k.app.job_of(id).is_none_or(|j| j.held.is_none()),
        "a skill never holds"
    );
    let (body, error) = answered(&k.app, "tu_c").expect("answered");
    assert!(!error && body.contains("Check it."), "{body}");
}

/// **`:skill <name> [args]` runs a skill now** — a turn whose prompt says to
/// run it and whose message carries the body as a `skill` block, so the
/// session records what the model was given.
#[tokio::test]
async fn skill_runs_a_skill_as_a_turn() {
    let mut k = Known::new();
    k.skill("review", "Review the diff", "Check $ARGUMENTS carefully.");
    k.app.dispatch_command_for_test("skill review src/");
    let sent = k.sent.lock().unwrap();
    let req = sent.last().expect("a turn was sent");
    let skill: Vec<_> = told_in(req)
        .into_iter()
        .filter(|(kind, _, _)| *kind == ContextKind::Skill)
        .collect();
    assert_eq!(skill.len(), 1, "{skill:?}");
    assert_eq!(skill[0].1, "review");
    assert!(
        skill[0].2.contains("Check src/ carefully."),
        "{}",
        skill[0].2
    );
    let user = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .unwrap();
    assert_eq!(user.content, "Run the `review` skill: src/");
}

/// **`:skill` refuses with a reason** — no name, a name that is no skill (the
/// ones there are, named), and a session already busy — and sends nothing.
#[tokio::test]
async fn skill_refuses_with_a_reason_and_sends_nothing() {
    let mut k = Known::new();
    k.skill("review", "Review the diff", "Check it.");
    let count = |k: &Known| k.sent.lock().unwrap().len();

    k.app.dispatch_command_for_test("skill");
    assert!(k.app.notice().unwrap_or_default().contains("which?"));
    k.app.dispatch_command_for_test("skill nope");
    let said = k.app.notice().unwrap_or_default();
    assert!(
        said.contains("no skill named `nope`") && said.contains("review"),
        "{said}"
    );
    assert_eq!(count(&k), 0, "neither sent anything");

    k.app.submit("hi");
    let before = count(&k);
    k.app.dispatch_command_for_test("skill review");
    assert_eq!(count(&k), before, "busy: nothing sent");
    assert_eq!(
        k.app.notice_kind(),
        Some(crate::app::NoticeKind::Refusal),
        "{:?}",
        k.app.notice()
    );
}

/// **`:memories` and `:skills` are one screen of files, and `<Enter>`
/// edits the one under the cursor** (Steven, 10-06: "an interface similar
/// to `:models`") — a file per row under its place, headings never a row;
/// `Tab` turns to the other list; `<Enter>` opens the file under the cursor
/// in the editor `setup_event` is given (`$EDITOR`, from the event loop).
#[tokio::test]
async fn enter_on_a_shelf_opens_the_file_under_the_cursor() {
    let mut k = Known::new();
    let agents = k.start.join("AGENTS.md");
    k.write(&agents, "be terse");
    let index = k.memory_dir().join("MEMORY.md");
    k.write(&index, "- [x](x.md) — a thing");
    let skill = k.skill("review", "Review the diff", "Check it.");

    k.app.dispatch_command_for_test("memories");
    let shown = shelf(&k.app);
    let at = |needle: &str| {
        shown
            .lines()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle} listed: {shown}"))
    };
    assert!(at("Instructions") < at("AGENTS.md"), "{shown}");
    assert!(at("· the model writes here") < at("MEMORY.md"), "{shown}");
    assert!(shown.contains("[Memories 2]"), "{shown}");
    assert!(
        shown.contains("the model writes to") && shown.contains("unasked"),
        "{shown}"
    );
    assert!(
        !shown.lines().any(|l| l.trim() == "~"),
        "no vim filler: {shown}"
    );
    assert_eq!(shelf_key(&mut k.app, KeyCode::Enter), Some(agents));
    shelf_key(&mut k.app, KeyCode::Char('j'));
    assert_eq!(shelf_key(&mut k.app, KeyCode::Enter), Some(index));

    shelf_key(&mut k.app, KeyCode::Tab);
    assert!(shelf(&k.app).contains("[Skills 1]"), "{}", shelf(&k.app));
    assert_eq!(shelf_key(&mut k.app, KeyCode::Enter), Some(skill));
    assert_eq!(shelf_key(&mut k.app, KeyCode::Esc), None);
    assert!(
        k.app.setup().is_some(),
        "Esc stays: vim's way back to Normal"
    );
    shelf_key(&mut k.app, KeyCode::Char('q'));
    assert!(k.app.setup().is_none(), "q leaves");

    // `:skills` opens on the skills.
    k.app.dispatch_command_for_test("skills");
    assert!(shelf(&k.app).contains("[Skills 1]"), "{}", shelf(&k.app));
}

/// **An edit is said, and the screen stays open, showing what the edit
/// made; `<Enter>` on an empty list says why it opens nothing** — through
/// the door the event loop passes `$EDITOR` to. The pane this replaced
/// answered the event loop `Ok(true)`, QUIT, so both ended Shannon (Steven,
/// 10-06).
#[tokio::test]
async fn enter_on_a_shelf_edits_or_says_why_not() {
    let mut k = Known::new();
    let skill = k.skill("review", "Review the diff", "Check it.");
    k.app.dispatch_command_for_test("skills");
    let key = Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    k.app.setup_event(&key, |p| {
        std::fs::write(p, "---\nname: review\ndescription: Renamed it\n---\nx")?;
        Ok(())
    });
    let said = k.app.notice().unwrap_or_default();
    assert!(
        said.contains("edited") && said.contains("SKILL.md"),
        "{said}"
    );
    let shown = shelf(&k.app);
    assert!(shown.contains("Renamed it"), "read again: {shown}");
    assert!(
        std::fs::read_to_string(&skill)
            .unwrap()
            .contains("Renamed it")
    );

    let mut empty = Known::new();
    empty.app.dispatch_command_for_test("skills");
    assert_eq!(shelf_key(&mut empty.app, KeyCode::Enter), None);
    let shown = shelf(&empty.app);
    assert!(shown.contains("nothing to edit — no skills"), "{shown}");
    assert!(
        shown.contains(".shannon/skills"),
        "where a skill goes: {shown}"
    );
}

/// **A memory the model writes during a turn is listed when the turn
/// ends** — the screen open throughout (it is modal; the turn is not).
#[tokio::test]
async fn a_turns_end_refreshes_the_open_screen() {
    let mut k = Known::new();
    k.app.submit("remember that I like green");
    k.app.dispatch_command_for_test("memories");
    assert!(
        !shelf(&k.app).contains("MEMORY.md"),
        "premise: {}",
        shelf(&k.app)
    );
    k.write(
        &k.memory_dir().join("MEMORY.md"),
        "- [g](g.md) — likes green",
    );
    k.app.feed_stream(StreamMsg::Complete);
    assert!(shelf(&k.app).contains("MEMORY.md"), "{}", shelf(&k.app));
}

/// **The screen does not open over a y/N waiting** — as `:models` and
/// `:effort` do not (`refuse_over_a_confirm`): it would take the key that
/// answers it. An app with roots, or `:memories` refuses for having none.
#[tokio::test]
async fn no_shelf_opens_over_a_question() {
    for verb in ["memories", "skills"] {
        let mut k = Known::new();
        k.app
            .pending_confirm
            .push_back(crate::app::ConfirmAction::DeleteAll);
        k.app.notices.clear();
        k.app.dispatch_command_for_test(verb);
        assert!(k.app.setup().is_none(), "`:{verb}` over a y/N");
        let said = k.app.notice().unwrap_or_default();
        assert!(said.contains("answer the question waiting first"), "{said}");
    }
}

/// **The `skill` tool is offered only where a skill is found** — `bash`
/// and `mark` first, always; a tool that can only fail is noise in every
/// request.
#[tokio::test]
async fn the_skill_tool_is_offered_only_where_a_skill_is_found() {
    let mut k = Known::new();
    let tools = |k: &Known| -> Vec<String> {
        let sent = k.sent.lock().unwrap();
        sent.last()
            .unwrap()
            .tools
            .iter()
            .map(|t| t.name.clone())
            .collect()
    };
    k.turn("hi");
    let offered = ["bash", "Read", "Edit", "Write", "mark"];
    assert_eq!(tools(&k), offered);
    k.skill("review", "Review the diff", "Check it.");
    k.turn("again");
    assert_eq!(tools(&k), [&offered[..], &["skill"]].concat());
}

/// **`memory.claude = off` reaches the provider** — the next request says
/// not to read CLAUDE.md (`SendRequest::own_instructions`) nor keep its own
/// memories (`own_memory`), from `:set` at once; by default it says nothing.
/// And `memory = off` is off for claude's memories too, its CLAUDE.md still
/// read.
#[tokio::test]
async fn claude_off_reaches_the_next_request() {
    let mut k = Known::new();
    k.turn("hi");
    let last = |k: &Known| {
        let sent = k.sent.lock().unwrap();
        let req = sent.last().unwrap();
        (req.own_instructions, req.own_memory)
    };
    assert_eq!(last(&k), (true, true), "kept by default");
    k.app.dispatch_command_for_test("set memory=off");
    k.turn("again");
    assert_eq!(last(&k), (true, false), "{:?}", k.app.notice());
    k.app.dispatch_command_for_test("set memory=auto");
    k.app.dispatch_command_for_test("set memory.claude=off");
    k.turn("once more");
    assert_eq!(last(&k), (false, false), "{:?}", k.app.notice());
}

/// **`[claude_code] auto_compact = false` reaches the next request** —
/// Claude Code's auto-compaction off (`SendRequest::auto_compact`, its
/// `DISABLE_AUTO_COMPACT`), from `:set` at once; on by default, as Claude
/// Code's own is (Steven, 10-15: "are we forced to use autocompaction?").
#[tokio::test]
async fn auto_compact_off_reaches_the_next_request() {
    let mut k = Known::new();
    k.turn("hi");
    let last = |k: &Known| k.sent.lock().unwrap().last().unwrap().auto_compact;
    assert!(last(&k), "Claude Code's own by default");
    k.app
        .dispatch_command_for_test("set claude_code.auto_compact=false");
    k.turn("again");
    assert!(!last(&k), "{:?}", k.app.notice());
}

/// **`[claude_code] background_tasks = false` reaches the next request** —
/// Claude Code's background work off (`SendRequest::background_tasks`, its
/// `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS`), from `:set` at once; on by
/// default, as Claude Code's own is (Steven, 10-22: kept, "and let people
/// close the gap if they want").
#[tokio::test]
async fn background_tasks_off_reaches_the_next_request() {
    let mut k = Known::new();
    k.turn("hi");
    let last = |k: &Known| k.sent.lock().unwrap().last().unwrap().background_tasks;
    assert!(last(&k), "Claude Code's own by default");
    k.app
        .dispatch_command_for_test("set claude_code.background_tasks=false");
    k.turn("again");
    assert!(!last(&k), "{:?}", k.app.notice());
}

/// **The gate reads a withheld variable as a command has it: unset** — not
/// placed where Shannon's own value would put it (Steven, 10-31, S18-Q1).
#[tokio::test]
async fn the_gate_reads_a_withheld_variable_as_unset() {
    use shannon_core::permission::ActKind;
    let unplaced = |k: &Known| {
        let v = k.app.rules.decide_line("cat $HOME/notes.txt");
        v.stages()
            .iter()
            .flat_map(|s| s.acts.iter())
            .any(|a| matches!(a.act, ActKind::Unplaced(_)))
    };
    assert!(!unplaced(&Known::new()), "premise: HOME is placed");
    let mut config = Config::default();
    config.exec.withhold = vec!["HOME".into()];
    assert!(unplaced(&Known::with(config)), "withheld: it is not");
}

/// **A secret's variable reaches no command** — the request names it for
/// `claude` to be started without (`ProviderEnv::withheld`), and `[exec]
/// env` cannot hand it back, as a name or as a `$NAME`, to a request or to
/// a command of Shannon's own; the note says why (review stop 14, Reviewer
/// E F1: on Claude Code, `echo $GITHUB_TOKEN` read a lent value, allowed
/// unattended). **Nor one `[exec] withhold` names**, and `[claude_code] env
/// = "minimal"` reaches the request (Steven, 10-31, S18-Q1).
#[tokio::test]
async fn a_lent_secrets_variable_reaches_no_command() {
    let mut config = Config::default();
    config.exec.env = [
        ("GITHUB_TOKEN", "$GITHUB_TOKEN"),
        ("PAIRED", "x-$GITHUB_TOKEN"),
        ("AWS_SECRET_ACCESS_KEY", "x"),
    ]
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .into();
    config.exec.withhold = vec!["AWS_SECRET_ACCESS_KEY".into()];
    config.claude_code.env = shannon_core::config::EnvPolicy::Minimal;
    let mut k = Known::with(config);
    let mut table = shannon_core::Aliases::default();
    let source = format!("{}GITHUB_TOKEN", shannon_core::ENV_SOURCE);
    table
        .insert(
            "github-token",
            shannon_core::fakes::GITHUB_TOKEN_SHORT,
            &source,
            true,
        )
        .unwrap();
    k.app.aliases = Arc::new(table);
    k.turn("hi");
    let req = k.sent.lock().unwrap().last().unwrap().clone();
    let withheld = ["GITHUB_TOKEN", "AWS_SECRET_ACCESS_KEY"];
    assert_eq!(req.env.withheld, withheld);
    assert!(req.env.minimal, "the policy reaches the request");
    let paired = vec![("PAIRED".to_string(), "x-".to_string())];
    assert_eq!(req.env.set, paired, "a request's");
    let own = k.app.exec_config();
    assert_eq!(own.env, paired, "a command's own");
    assert_eq!(own.withheld, withheld, "a command's own");
    let notes = k.app.env_notes().join("\n");
    assert!(
        notes.contains("`AWS_SECRET_ACCESS_KEY` is in [exec] withhold"),
        "{notes}"
    );
    assert!(
        notes.contains("`GITHUB_TOKEN` is lent as a secret"),
        "{notes}"
    );
}

/// **`memory = off` reaches the request that resumes a tool round** — the
/// second door a request leaves by (`spawn_tagged`'s other caller). The
/// setters sit in that one door and only a comment said so: moving them to
/// `start_generation` left every test green, since each drove a first
/// request only (review 28, Reviewer E2).
#[tokio::test]
async fn memory_off_reaches_a_resumed_tool_round() {
    let mut k = Known::new();
    k.skill("review", "Review the diff", "Check it.");
    k.app.dispatch_command_for_test("set memory=off");
    k.app.submit("go");
    k.app
        .feed_stream(StreamMsg::ToolUse(skill_block("tu_1", "review", "")));
    k.app.feed_stream(StreamMsg::Complete);
    let sent = k.sent.lock().unwrap();
    assert_eq!(sent.len(), 2, "premise: the round resumed");
    assert!(!sent[1].own_memory, "the resumed request keeps memory off");
}

/// **In a Claude Code session, a memory write to its own directory runs
/// unasked under `auto`** — once vouched (`scratch::prepare_owned`, made
/// where Claude Code has kept THIS project); before it has, it asks, and
/// Shannon makes nothing under `~/.claude` (reviews 28, A and C: a launch
/// made `projects/<start>/memory` wherever `projects` existed).
#[tokio::test]
async fn claude_code_writes_its_own_memories_unasked() {
    let (mut app, provider) = provider_app();
    let dir = tempfile::tempdir().unwrap();
    let start = dir.path().join("proj");
    std::fs::create_dir_all(&start).unwrap();
    let theirs = dir.path().join("claude/projects/-proj/memory");
    let roots = Roots {
        start,
        config_dir: Some(dir.path().join("config")),
        project: "proj-1234abcd".into(),
        home: Some(dir.path().join("home")),
        claude_home: Some(dir.path().join("home/.claude")),
        claude_memory: Some(theirs.clone()),
        project_root: shannon_core::config::MemoryConfig::default().project_root,
    };
    let write = |app: &mut App, id: &str| {
        provider_asks(
            app,
            id,
            "Write",
            serde_json::json!({ "file_path": theirs.join("green.md"), "content": "green" }),
        );
    };
    // Claude Code keeps projects, but not this one.
    std::fs::create_dir_all(dir.path().join("claude/projects")).unwrap();
    app.set_knowledge_roots(roots.clone());
    write(&mut app, "tu_1");
    assert!(app.shown_call().is_some(), "not kept here yet: it asks");
    assert!(
        !theirs.parent().unwrap().exists(),
        "and Shannon made nothing under Claude Code's projects"
    );

    let (mut app, provider2) = provider_app();
    std::fs::create_dir_all(theirs.parent().unwrap()).unwrap();
    let said = app.set_knowledge_roots(roots);
    assert!(said.is_empty(), "{said:?}");
    write(&mut app, "tu_2");
    assert!(app.shown_call().is_none(), "ran unasked");
    assert_eq!(provider2.answered(), vec![allowed("tu_2")]);
    assert!(provider.answered().is_empty());
}

/// **…and where its directory is a `.claude`, as it always is** (P6a,
/// 10-13): the test above names `claude/projects`, so the shipped `ask
/// :write **/.claude` never met it, and the diary's replay asked for all
/// four of Claude Code's memory writes and reads (`~/.claude/projects/…/
/// memory`). The vouched grant must answer the guard.
#[tokio::test]
async fn claude_codes_memory_under_a_dot_claude_is_written_unasked() {
    let (mut app, provider) = provider_app();
    let dir = tempfile::tempdir().unwrap();
    let start = dir.path().join("proj");
    std::fs::create_dir_all(&start).unwrap();
    let theirs = dir.path().join(".claude/projects/-proj/memory");
    std::fs::create_dir_all(theirs.parent().unwrap()).unwrap();
    // The premise: with no `[memory]` rules, the write meets the guard.
    let bare =
        shannon_core::permission::RuleSet::defaults().with_start(&start.display().to_string());
    let held = bare.decide_line(&format!("cat > {}", theirs.join("green.md").display()));
    assert!(
        held.stages()[0].acts.iter().any(|a| matches!(&a.by,
            shannon_core::permission::By::Rules(c) if c.iter().any(|c| c.source == r"ask :write \c**/.claude"))),
        "{held:?}"
    );
    let said = app.set_knowledge_roots(Roots {
        start,
        config_dir: Some(dir.path().join("config")),
        project: "proj-1234abcd".into(),
        home: Some(dir.path().join("home")),
        claude_home: Some(dir.path().join("home/.claude")),
        claude_memory: Some(theirs.clone()),
        project_root: shannon_core::config::MemoryConfig::default().project_root,
    });
    assert!(said.is_empty(), "{said:?}");
    provider_asks(
        &mut app,
        "tu_1",
        "Write",
        serde_json::json!({ "file_path": theirs.join("green.md"), "content": "green" }),
    );
    assert!(
        app.shown_call().is_none(),
        "{}",
        app.shown_call()
            .map(|p| p
                .text()
                .lines()
                .iter()
                .map(|l| l.text() + "\n")
                .collect::<String>())
            .unwrap_or_default()
    );
    assert_eq!(provider.answered(), vec![allowed("tu_1")]);
}

/// **Claude Code's first turn in a project makes the place its memories
/// go, and from the end of that turn they are written unasked** — not from
/// the next launch (`vouch_claude_memory_late`, at every turn's end).
#[tokio::test]
async fn claude_codes_first_turn_in_a_project_vouches_its_memory() {
    let mut k = Known::new();
    let theirs = k._dir.path().join("claude/projects/-proj/memory");
    assert!(
        k.app.claude_memory_write.is_empty(),
        "premise: not kept yet"
    );
    k.turn("hi");
    assert!(k.app.claude_memory_write.is_empty(), "still not kept");
    // Claude Code ran here: its project directory now exists.
    std::fs::create_dir_all(theirs.parent().unwrap()).unwrap();
    k.turn("again");
    assert!(
        !k.app.claude_memory_write.is_empty(),
        "vouched at the turn's end"
    );
    assert!(theirs.is_dir());
}

/// **`<Tab>` after `:skill` offers the skills found**, by what is typed.
#[tokio::test]
async fn skill_names_complete() {
    let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let mut k = Known::new();
    k.skill("review", "Review the diff", "Check it.");
    k.skill("release", "Cut a release", "Tag it.");
    type_command(&mut k.app, "skill rev");
    k.app.command_input(tab);
    assert_eq!(k.app.cmdline.line(), Some("skill review"));
    k.app.command_input(esc);
    type_command(&mut k.app, "skill re");
    k.app.command_input(tab);
    let (shown, _) = k.app.cmdline.menu().expect("two match, a menu");
    assert_eq!(shown, ["release", "review"]);
}

/// **A credential path is never told, and says so** — by the gate's own
/// judge (`[secrets] paths`), at the app: not in the request, NOT READ in
/// `:memories`, NOT USED in `:skills`, refused by `:skill` and the tool
/// (Reviewers A and C, round 27: deleting the app's judge passed every
/// test). Under `path_policy = "allow"`, your choice, it is any file.
#[tokio::test]
async fn a_credential_path_is_never_told_and_says_so() {
    let mut config = Config::default();
    config.secrets.paths = vec!["SHANNON.md".into(), "vault".into()];
    let mut k = Known::with(config.clone());
    k.write(&k.start.join("SHANNON.md"), "the key is hunter2");
    k.skill("vault", "Open the vault", "Use hunter2.");
    let told = k.turn("hi");
    assert!(
        told.iter()
            .all(|(_, s, t)| !s.ends_with("SHANNON.md") && !t.contains("hunter2")),
        "{told:?}"
    );
    k.app.dispatch_command_for_test("memories");
    let shown = shelf(&k.app);
    let not_read = shown.split("Not read").nth(1).expect("a Not read group");
    assert!(
        not_read.contains("SHANNON.md") && not_read.contains("credential"),
        "{shown}"
    );
    k.app.dispatch_command_for_test("skills");
    let skills = shelf(&k.app);
    let unused = skills.split("Not used").nth(1).expect("a Not used group");
    assert!(unused.contains("vault"), "{skills}");
    shelf_key(&mut k.app, KeyCode::Char('q'));
    k.app.dispatch_command_for_test("skill vault");
    assert_eq!(
        k.app.notice_kind(),
        Some(crate::app::NoticeKind::Refusal),
        "{:?}",
        k.app.notice()
    );
    k.app.conversation_mut().begin_turn("go");
    k.app
        .feed_stream(StreamMsg::ToolUse(skill_block("tu_v", "vault", "")));
    k.app.feed_stream(StreamMsg::Complete);
    let (why, error) = answered(&k.app, "tu_v").expect("answered");
    assert!(error && !why.contains("hunter2"), "{why}");

    config.secrets.path_policy = shannon_core::PathPolicy::Allow;
    let mut k = Known::with(config);
    k.write(&k.start.join("SHANNON.md"), "be terse");
    let told = k.turn("hi");
    assert!(
        told.iter().any(|(_, s, _)| s.ends_with("SHANNON.md")),
        "allow: read as any file — {told:?}"
    );
}

/// **What a file tells is redacted as a command's output is** — a
/// credential's shape becomes `[redacted: …]`, and the transcript says the
/// file was scrubbed (Reviewers A and B, round 27: a `cat` of the same file
/// was redacted, and the told text was not).
#[tokio::test]
async fn what_is_told_is_redacted_as_output_is() {
    let mut k = Known::new();
    let token = shannon_core::fakes::GITHUB_TOKEN_RUN;
    k.write(&k.start.join("AGENTS.md"), &format!("deploy with {token}"));
    k.skill("ship", "Ship it", &format!("push with {token}"));
    let told = k.turn("hi");
    let agents = told
        .iter()
        .find(|(_, s, _)| s.ends_with("AGENTS.md"))
        .expect("told");
    assert!(!agents.2.contains(token), "{}", agents.2);
    assert!(agents.2.contains("[redacted: "), "{}", agents.2);
    let shown = k.app.output().lines().join("\n");
    assert!(shown.contains("AGENTS.md (redacted)"), "{shown}");
    // A skill's text, by `:skill` and by the tool alike.
    k.app.dispatch_command_for_test("skill ship");
    let sent = format!("{:?}", k.sent.lock().unwrap().last().unwrap().messages);
    assert!(!sent.contains(token), "the skill block");
    k.app.feed_stream(StreamMsg::Complete);
    k.app.conversation_mut().begin_turn("go");
    k.app
        .feed_stream(StreamMsg::ToolUse(skill_block("tu_s", "ship", "")));
    k.app.feed_stream(StreamMsg::Complete);
    let (body, error) = answered(&k.app, "tu_s").expect("answered");
    assert!(!error && !body.contains(token), "{body}");
}

/// **A value you mark secret after a file told it is said to have gone
/// out** — the rotate warning reads context blocks as it reads prompts
/// (Reviewer C, round 27: that arm had no test).
#[tokio::test]
async fn marking_a_value_a_file_already_told_says_rotate_it() {
    let mut k = Known::new();
    k.write(&k.start.join("AGENTS.md"), "the password is hunter2");
    k.turn("hi");
    k.app.dispatch_command_for_test("secret hunter2");
    assert!(
        k.app
            .messages()
            .any(|m| m.contains("already sent in clear")),
        "{:?}",
        k.app.messages().collect::<Vec<_>>()
    );
}

/// **A memory location that cannot be vouched for is not written unasked —
/// and nothing says it is**: the gate asks, `:rules` holds no write allow,
/// the model is told writes ask, `:memories` says why (Reviewers A and C,
/// round 27: the vouch's refusal was consulted by no test, and both said
/// "unasked" from the setting alone). Another account able to write there
/// is the refusal (`scratch::prepare_owned`; others READING it is not).
#[cfg(unix)]
#[tokio::test]
async fn a_write_location_not_vouched_for_asks_and_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let mut k = Known::new();
    let dir = k.memory_dir();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o775)).unwrap();
    k.app.dispatch_command_for_test("set memory=ask");
    k.app.dispatch_command_for_test("set memory=auto");
    assert!(
        k.app.messages().any(|m| m.contains("not written unasked")),
        "the refusal is said"
    );
    let echo = format!("echo hi > {}/m.md", dir.display());
    assert_eq!(
        k.app.rules.decide_line(&echo).decision,
        shannon_core::permission::Decision::Ask
    );
    assert!(!k.app.rules_listing().contains("allow :write"));
    let told = k.turn("hi");
    let guidance = told
        .iter()
        .find(|(_, s, _)| s == "shannon:memory")
        .expect("the guidance");
    assert!(
        guidance.2.contains("Each write there asks"),
        "{}",
        guidance.2
    );
    k.app.dispatch_command_for_test("memories");
    let shown = shelf(&k.app).replace('\n', " ");
    assert!(
        shown.contains("is not approved (:messages says why)"),
        "{shown}"
    );
}

/// **Where Claude Code has not kept the project yet, `:memories` says so** —
/// it pointed at `:messages`, where nothing had been said (Reviewer A, round
/// 28).
#[test]
fn claude_codes_unkept_project_is_said_not_pointed_at() {
    let mut k = Known::new();
    k.app.model_command("claude-code/sonnet");
    assert!(k.app.claude_memory_pending, "premise: not kept yet");
    k.app.dispatch_command_for_test("memories");
    let shown = shelf(&k.app).replace('\n', " ");
    assert!(shown.contains("has not kept this project yet"), "{shown}");
    assert!(!shown.contains(":messages says why"), "{shown}");
}

/// **Drawn through the frame, the screen scrolls inside its own box** — the
/// room `draw` gives `page` is the room it draws in, so with forty memories
/// on a short terminal the cursor's row is on screen and `setup`'s own
/// "… more below" never cuts the list (Reviewer C, round 28: giving `page`
/// forty rows too many passed every test, none of which drew a frame).
#[test]
fn the_memories_screen_scrolls_inside_its_box() {
    let mut k = Known::new();
    for i in 0..40 {
        k.write(
            &k.memory_dir().join(format!("m{i:02}.md")),
            "---\ndescription: a memory\n---\n",
        );
    }
    k.app.dispatch_command_for_test("memories");
    for _ in 0..20 {
        shelf_key(&mut k.app, KeyCode::Char('j'));
    }
    let screen = text_of(&mut k.app, 80, 16);
    assert!(screen.contains("▸ m20.md"), "the cursor's row: {screen}");
    assert!(!screen.contains("more below"), "{screen}");
    assert!(
        screen.contains("more"),
        "its own scrolling says so: {screen}"
    );
}

/// **`x` disables a skill for the session, wherever it could be loaded** —
/// marked on the screen, refused to `:skill` and to Claude Code's `Skill`
/// call with how to use it again, a read of its files refused — and `x`
/// again enables it (Steven, 10-06: "we disable further reading").
#[tokio::test]
async fn x_disables_a_skill_wherever_it_could_be_loaded() {
    use shannon_core::permission::Decision;
    let mut k = Known::new();
    let provider = shannon_core::backend::RecordingProvider::new();
    k.app.claude_code = Arc::new(provider.clone());
    // Under `{config}/skills`: a placed location, approved for reads (a
    // project's own is outside the test app's start).
    let skill = k.config.join("skills/review/SKILL.md");
    k.write(
        &skill,
        "---\nname: review\ndescription: Review it\n---\nCheck it.",
    );
    let cat = format!("cat {}", skill.display());
    let said = |k: &Known| k.app.notice().unwrap_or_default();
    assert_eq!(
        k.app.rules.decide_line(&cat).decision,
        Decision::Allow,
        "premise"
    );

    k.app.dispatch_command_for_test("skills");
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert_eq!(k.app.config.skills.disabled, vec!["review".to_string()]);
    assert!(
        said(&k).contains("disabled for this session"),
        "{}",
        said(&k)
    );
    assert!(shelf(&k.app).contains("disabled"), "{}", shelf(&k.app));
    assert_eq!(k.app.rules.decide_line(&cat).decision, Decision::Deny);
    shelf_key(&mut k.app, KeyCode::Char('q'));

    k.app.dispatch_command_for_test("skill review");
    assert!(said(&k).contains("`review` is disabled"), "{}", said(&k));
    provider_asks(
        &mut k.app,
        "tu_s",
        "Skill",
        serde_json::json!({ "skill": "review" }),
    );
    assert!(k.app.shown_call().is_none(), "refused, not asked");
    assert!(
        matches!(
            provider.answered().as_slice(),
            [(id, shannon_core::hook::Reply::Deny { reason })]
                if id == "tu_s" && reason.contains("disabled")
        ),
        "{:?}",
        provider.answered()
    );

    k.app.dispatch_command_for_test("skills");
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert!(k.app.config.skills.disabled.is_empty());
    assert_eq!(k.app.rules.decide_line(&cat).decision, Decision::Allow);
}

/// **`x` on a memory disables that file, by its path** — refused at the
/// gate while its neighbours stay read.
#[test]
fn x_disables_a_memory_by_its_path() {
    use shannon_core::permission::Decision;
    let mut k = Known::new();
    let old = k.memory_dir().join("old.md");
    k.write(
        &k.memory_dir().join("MEMORY.md"),
        "- [old](old.md)\n- [new](new.md)",
    );
    k.write(&old, "---\ndescription: stale\n---\nx");
    k.write(
        &k.memory_dir().join("new.md"),
        "---\ndescription: fresh\n---\ny",
    );
    k.app.dispatch_command_for_test("memories");
    let on_old = |k: &Known| k.app.setup().and_then(|s| s.shelf_selected()) == Some(old.clone());
    for _ in 0..10 {
        if on_old(&k) {
            break;
        }
        shelf_key(&mut k.app, KeyCode::Char('j'));
    }
    assert!(on_old(&k), "premise: the cursor on old.md");
    shelf_key(&mut k.app, KeyCode::Char('x'));
    let held = &k.app.config.memory.disabled;
    assert!(
        held.len() == 1 && held[0].ends_with("/memory/old.md"),
        "{held:?}"
    );
    let decide = |p: &Path| {
        k.app
            .rules
            .decide_line(&format!("cat {}", p.display()))
            .decision
    };
    assert_eq!(decide(&old), Decision::Deny);
    assert_eq!(decide(&k.memory_dir().join("new.md")), Decision::Allow);
}

/// **`x` writes the row's name as ONE entry, and takes back only an entry
/// naming that row** (review 29): a `,` in a name split it into two junk
/// entries; `x` on a skill disabled by its place said "enabled" and changed
/// nothing — it says which entry holds it and the line that takes it back.
#[test]
fn x_takes_back_only_an_entry_naming_the_row() {
    let mut k = Known::new();
    let skill = |k: &Known, dir: &str| {
        let path = k.config.join(format!("skills/{dir}/SKILL.md"));
        k.write(
            &path,
            &format!("---\nname: {dir}\ndescription: d\n---\nbody"),
        );
    };
    skill(&k, "a,b");
    k.app.dispatch_command_for_test("skills");
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert_eq!(k.app.config.skills.disabled, vec!["a,b".to_string()]);
    // What `x` came to: on the screen, where the eyes are, AND in
    // `:messages` (review 29, Reviewer C: D2) — an outcome, not drawn in
    // the colour a refusal is.
    let said = "`a,b` disabled for this session";
    assert!(shelf(&k.app).contains(said), "{}", shelf(&k.app));
    assert!(k.app.messages().any(|m| m.contains(said)));
    assert_ne!(drawn_fg(&mut k.app, said), Some(crate::palette::CAUTION));
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert!(k.app.config.skills.disabled.is_empty(), "taken back whole");
    shelf_key(&mut k.app, KeyCode::Char('q'));

    k.app
        .dispatch_command_for_test("set skills.disabled+={config}/skills");
    k.app.dispatch_command_for_test("skills");
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert_eq!(
        k.app.config.skills.disabled,
        vec!["{config}/skills".to_string()],
        "a place holding more is not taken back by a row"
    );
    let shown = shelf(&k.app).replace('\n', " ");
    assert!(
        shown.contains("disabled by `{config}/skills`, which holds more than this")
            && shown.contains(":set skills.disabled-={config}/skills"),
        "{shown}"
    );
    assert!(
        k.app
            .messages()
            .any(|m| m.contains("which holds more than this")),
        "a refusal outlives the screen"
    );
    assert_eq!(
        drawn_fg(&mut k.app, "disabled by"),
        Some(crate::palette::CAUTION),
        "a refusal is drawn as one"
    );
}

/// The colour the open screen draws `text` in — its first cell, drawn. It
/// read the span that `page()` built, which a view that painted it some
/// other way would not have changed (review stop 12, T4 8).
fn drawn_fg(app: &mut App, text: &str) -> Option<ratatui::style::Color> {
    assert!(app.setup().and_then(|s| s.shelf_screen()).is_some(), "open");
    let buf = frame_of(app, 200, 60);
    let at = cell_of(&buf, text).unwrap_or_else(|| panic!("{text} not drawn"));
    buf[at].style().fg
}

/// **In a Claude Code session, `x` then `x` again leaves the gate as it
/// was** — the deny `x` put on claude's own skill is gone when it is
/// enabled (Steven, 10-07: "enabled and now it looks like it is disabled";
/// the `x` tests above all ran in an API session).
#[test]
fn x_twice_in_a_claude_code_session_leaves_the_gate_as_it_was() {
    use shannon_core::permission::Decision;
    let mut k = Known::new();
    k.app.model_command("claude-code/sonnet");
    let skill = k.start.join(".claude/skills/review/SKILL.md");
    k.write(&skill, "---\nname: review\ndescription: d\n---\nbody");
    // A relative place is approved only if it is there at a rebuild (a
    // docketed QUESTION): rebuilt once, so "before" is the gate's own.
    k.app.rebuild_rules();
    let cat = format!("cat {}", skill.display());
    let decide = |k: &Known| k.app.claude_rules.decide_line(&cat).decision;
    let before = decide(&k);
    assert_eq!(before, Decision::Allow, "premise: claude's own, read");
    k.app.dispatch_command_for_test("skills");
    assert!(shelf(&k.app).contains("claude's"), "{}", shelf(&k.app));
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert_eq!(decide(&k), Decision::Deny, "disabled");
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert!(k.app.config.skills.disabled.is_empty());
    assert_eq!(decide(&k), before, "enabled: as it was");
    assert!(!shelf(&k.app).contains("disabled  "), "{}", shelf(&k.app));
}

/// **`x` enables, for the session, an entry a config file holds — and
/// says the file brings it back** (Steven, 10-07: `:set x-=v` removes `v`
/// wherever it came from; it used to stay, the lists stacking).
#[test]
fn x_enables_a_files_entry_for_the_session_and_says_so() {
    use shannon_core::config::Origin;
    let mut k = Known::new();
    k.write(
        &k.config.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: d\n---\nbody",
    );
    let mine = k.config.join("config.toml");
    std::fs::write(&mine, "[skills]\ndisabled = [\"review\"]\n").unwrap();
    k.app.source_file(Origin::User, &mine);
    k.app.dispatch_command_for_test("skills");
    assert!(shelf(&k.app).contains("disabled"), "premise");
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert!(k.app.config.skills.disabled.is_empty(), "taken out");
    let shown = shelf(&k.app).replace('\n', " ");
    assert!(
        shown.contains("`review` enabled for this session")
            && shown.contains("config.toml disables it again at the next launch"),
        "{shown}"
    );
}

/// **`claude` reads its own CLAUDE.md, so `x` on it says what does stop
/// it** — `[memory] claude = "off"` — and writes nothing (review 29: "not
/// told" while claude read it every turn).
#[test]
fn x_on_claudes_own_instructions_says_what_stops_them() {
    let mut k = Known::new();
    let theirs = k.start.join("CLAUDE.md");
    k.write(&theirs, "be terse");
    k.app.model_command("claude-code/sonnet");
    k.app.dispatch_command_for_test("memories");
    assert_eq!(
        k.app.setup().and_then(|s| s.shelf_selected()),
        Some(theirs),
        "premise: the cursor on CLAUDE.md"
    );
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert!(k.app.config.memory.disabled.is_empty(), "nothing written");
    let shown = shelf(&k.app).replace('\n', " ");
    assert!(shown.contains("`[memory] claude = \"off\"`"), "{shown}");
}

/// **Claude Code's `Skill` call is refused by every spelling of a disabled
/// name** — `/review`, and a plugin's `p:review` Shannon never found, held
/// by the name alone (review 29: `/review` was asked; Reviewer C's M7).
#[tokio::test]
async fn a_skill_call_is_refused_by_every_spelling_of_its_name() {
    let mut k = Known::new();
    let provider = shannon_core::backend::RecordingProvider::new();
    k.app.claude_code = Arc::new(provider.clone());
    k.app
        .dispatch_command_for_test("set skills.disabled+=review");
    for (id, called) in [("tu_1", "/review"), ("tu_2", "plug:review")] {
        provider_asks(
            &mut k.app,
            id,
            "Skill",
            serde_json::json!({ "skill": called }),
        );
        assert!(k.app.shown_call().is_none(), "{called}: refused, not asked");
        let answered = provider.answered();
        assert!(
            matches!(
                answered.last(),
                Some((i, shannon_core::hook::Reply::Deny { reason }))
                    if i == id && reason.contains("by `review`")
            ),
            "{called}: {answered:?}"
        );
    }
    provider_asks(
        &mut k.app,
        "tu_3",
        "Skill",
        serde_json::json!({ "skill": "plug:ship" }),
    );
    assert!(
        k.app.shown_call().is_some(),
        "another skill asks, as before"
    );
}

/// **The suggested `:set` line means the one entry** — plain where it
/// can be; a `,`, a space, a backslash or a leading `[` in a TOML literal,
/// escaped for `:set`'s words — checked by parsing it back.
#[test]
fn the_take_back_line_parses_to_its_one_entry() {
    use shannon_core::config::set;
    for entry in [
        "review",
        "{config}/skills",
        "a,b",
        "~/my skills",
        "a\\b",
        "[x]",
        " r ",
    ] {
        let line = super::super::knowledge_flow::take_back_line("skills", entry);
        let typed = line.strip_prefix(":set ").unwrap();
        let mut config = Config::default();
        config.skills.disabled = vec![entry.to_string(), "other".into()];
        let plan = set::plan(set::parse(typed).unwrap(), &config).unwrap();
        let left = plan.table["skills"]["disabled"].as_array().unwrap().clone();
        assert_eq!(left, vec![toml::Value::from("other")], "{entry:?} → {line}");
    }
}

/// **`d` asks on the screen, saying what goes; `y` removes, any other key
/// keeps** — `Esc` included, which keeps the file AND the screen: the
/// reflexive key is the safe one.
#[test]
fn d_asks_what_goes_and_only_y_removes() {
    let mut k = Known::new();
    let index = k.memory_dir().join("MEMORY.md");
    let old = k.memory_dir().join("old.md");
    k.write(&index, "- [old](old.md) — stale\n");
    k.write(&old, "---\ndescription: stale\n---\nx");
    k.app.dispatch_command_for_test("memories");
    let on_old = |k: &Known| k.app.setup().and_then(|s| s.shelf_selected()) == Some(old.clone());
    for _ in 0..10 {
        if on_old(&k) {
            break;
        }
        shelf_key(&mut k.app, KeyCode::Char('j'));
    }
    assert!(on_old(&k), "premise: the cursor on old.md");

    shelf_key(&mut k.app, KeyCode::Char('d'));
    let shown = shelf(&k.app).replace('\n', " ");
    assert!(
        shown.contains("remove `old.md`") && shown.contains("its line in"),
        "{shown}"
    );
    shelf_key(&mut k.app, KeyCode::Esc);
    assert!(old.exists(), "kept");
    assert!(k.app.setup().is_some(), "and the screen stays");
    assert!(shelf(&k.app).contains("kept — nothing removed"));

    // Changed between the question and `y`: asked again, not done.
    shelf_key(&mut k.app, KeyCode::Char('d'));
    k.write(&index, "- [old](old.md) — stale\n- [also](./old.md)\n");
    shelf_key(&mut k.app, KeyCode::Char('y'));
    assert!(old.exists(), "not removed on a stale question");
    let shown = shelf(&k.app).replace('\n', " ");
    assert!(
        shown.contains("changed since you were asked") && shown.contains("2 lines"),
        "{shown}"
    );
    shelf_key(&mut k.app, KeyCode::Char('y'));
    assert!(!old.exists(), "removed");
    assert_eq!(std::fs::read_to_string(&index).unwrap(), "");
    assert!(shelf(&k.app).contains("removed"), "said: {}", shelf(&k.app));
    // Gone from the LIST: no row the cursor can reach is it. Not "the screen
    // never says `old.md`" — the outcome names it (`removed <path>`), whole
    // where the temp directory's path is short (Linux) and cut where it is
    // long (macOS), so that check passed on one and failed on the other.
    let mut reachable = Vec::new();
    for _ in 0..10 {
        reachable.extend(k.app.setup().and_then(|s| s.shelf_selected()));
        shelf_key(&mut k.app, KeyCode::Char('j'));
    }
    assert!(!reachable.contains(&old), "{reachable:?}");
}

/// **`n` names a new memory and opens its template; written, it is made,
/// listed, and given its index line** — the name typed on the screen,
/// where every key is the name's (`n`, `x`, `d` too).
#[test]
fn n_makes_a_memory_you_write() {
    let mut k = Known::new();
    k.app.dispatch_command_for_test("memories");
    shelf_key(&mut k.app, KeyCode::Char('n'));
    for c in "Likes green dxnqZ".chars() {
        shelf_key(&mut k.app, KeyCode::Char(c));
    }
    shelf_key(&mut k.app, KeyCode::Backspace);
    assert!(k.app.setup().is_some(), "`q` was the name's, not a leave");
    assert!(
        shelf(&k.app).contains("a new memory, its name: Likes green dxnq▏"),
        "{}",
        shelf(&k.app)
    );
    let enter = Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    k.app.setup_event(&enter, |path| {
        std::fs::write(
            path,
            "---\nname: Likes green\ndescription: the user's colour\ntype: user\n---\n\nGreen.\n",
        )?;
        Ok(())
    });
    let made = k.memory_dir().join("likes-green-dxnq.md");
    assert!(made.is_file(), "made");
    assert!(
        shelf(&k.app).contains("likes-green-dxnq.md"),
        "{}",
        shelf(&k.app)
    );
    let index = std::fs::read_to_string(k.memory_dir().join("MEMORY.md")).unwrap();
    assert!(
        index.contains("(likes-green-dxnq.md) — the user's colour"),
        "{index}"
    );

    // Left as written: nothing made.
    shelf_key(&mut k.app, KeyCode::Char('n'));
    for c in "blank".chars() {
        shelf_key(&mut k.app, KeyCode::Char(c));
    }
    shelf_key(&mut k.app, KeyCode::Enter);
    assert!(!k.memory_dir().join("blank.md").exists());
}

/// **What `n` and `N` cannot do, they say; a draft saved is kept, a
/// failing editor said** (review 29; Steven, 10-07): an editor that fails
/// (vim's `:cq`) after saving keeps what it saved, and says it failed; one
/// that failed on the template as written makes nothing; a skill name
/// already found is refused before any template, as a second would shadow
/// or be shadowed; `N` for a memory where none can be written says so
/// rather than draft a request.
#[test]
fn create_says_why_not_and_keeps_a_saved_draft() {
    let mut k = Known::new();
    let enter = Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let name = |k: &mut Known, name: &str| {
        shelf_key(&mut k.app, KeyCode::Char('n'));
        for c in name.chars() {
            shelf_key(&mut k.app, KeyCode::Char(c));
        }
    };
    k.app.dispatch_command_for_test("memories");
    name(&mut k, "saved");
    k.app.setup_event(&enter, |path| {
        std::fs::write(path, "---\nname: a\ndescription: d\n---\nx")?;
        Err(anyhow::anyhow!("editor exited with status 1"))
    });
    assert!(
        k.memory_dir().join("saved.md").exists(),
        "what was saved, kept"
    );
    let said = k.app.messages().last().unwrap_or_default().to_string();
    assert!(
        said.contains("the editor failed") && said.contains("made"),
        "{said}"
    );
    name(&mut k, "unsaved");
    k.app
        .setup_event(&enter, |_| Err(anyhow::anyhow!("no editor")));
    assert!(
        !k.memory_dir().join("unsaved.md").exists(),
        "nothing written"
    );
    let said = k.app.messages().last().unwrap_or_default().to_string();
    assert!(
        said.contains("the editor failed (no editor) — nothing made"),
        "{said}"
    );
    shelf_key(&mut k.app, KeyCode::Char('q'));

    k.skill("review", "Review it", "Check it.");
    k.app.dispatch_command_for_test("skills");
    name(&mut k, "Review");
    let mut opened = false;
    k.app.setup_event(&enter, |_| {
        opened = true;
        Ok(())
    });
    assert!(!opened, "no template for a name already found");
    let shown = shelf(&k.app).replace('\n', " ");
    assert!(
        shown.contains("a skill `review` is found already"),
        "{shown}"
    );
    shelf_key(&mut k.app, KeyCode::Char('q'));

    k.app.dispatch_command_for_test("set memory=off");
    k.app.dispatch_command_for_test("memories");
    shelf_key(&mut k.app, KeyCode::Char('N'));
    assert!(k.app.setup().is_some(), "refused, on the screen");
    assert!(k.app.prompt().text().is_empty(), "nothing drafted");
    let shown = shelf(&k.app).replace('\n', " ");
    assert!(shown.contains("no place to write memories"), "{shown}");
}

/// **`N` drafts a request for the model to write one** — the screen
/// closed, the request in the prompt, nothing sent; a draft of yours is
/// never replaced, and the screen says why.
#[test]
fn shift_n_drafts_a_request_for_the_model() {
    let mut k = Known::new();
    k.app.dispatch_command_for_test("skills");
    shelf_key(&mut k.app, KeyCode::Char('N'));
    assert!(k.app.setup().is_none(), "the screen closed");
    let draft = k.app.prompt().text();
    assert!(
        draft.starts_with("Write a new skill in ") && draft.contains("/skills/<name>/SKILL.md"),
        "{draft}"
    );
    assert!(k.sent.lock().unwrap().is_empty(), "nothing sent");

    k.app.dispatch_command_for_test("memories");
    shelf_key(&mut k.app, KeyCode::Char('N'));
    assert!(k.app.setup().is_some(), "refused, on the screen");
    assert_eq!(k.app.prompt().text(), draft, "yours untouched");
    assert!(
        shelf(&k.app).contains("holds a draft of yours"),
        "{}",
        shelf(&k.app)
    );
}

/// **`[exec] env` reaches both backends' commands, live, and what it holds
/// that a file could run from is said** — Shannon's own commands
/// (`exec_config`) and a request to a provider that runs its own (`req.env`)
/// get the same variables; a `:set` putting `.` on `PATH` says so at once.
#[tokio::test]
async fn the_commands_environment_is_set_live_and_said() {
    let mut k = Known::new();
    k.app
        .dispatch_command_for_test("set exec.env.PATH=.:/usr/bin");
    assert!(
        k.app.messages().any(|m| m.contains("PATH holds .")),
        "said: {:?}",
        k.app.messages().collect::<Vec<_>>()
    );
    let path = ("PATH".to_string(), ".:/usr/bin".to_string());
    assert!(
        k.app.exec_config().env.contains(&path),
        "Shannon's own commands"
    );
    k.turn("hi");
    let sent = k.sent.lock().unwrap();
    assert!(
        sent.last().unwrap().env.set.contains(&path),
        "and a provider's"
    );
}

/// **Claude Code's MCP servers are listed, and `x` disables one: every call
/// to its tools refused** — known from its `init`, said as unknown before;
/// a server is Claude Code's to configure, so Enter says why, and a server
/// left alone asks as before (Steven, 10-06: "a bit more obvious with
/// MCPs").
#[tokio::test]
async fn an_mcp_server_disabled_has_every_call_refused() {
    let mut k = Known::new();
    let provider = shannon_core::backend::RecordingProvider::new();
    k.app.claude_code = Arc::new(provider.clone());
    // The row a name is on, as the screen draws it.
    let line = |k: &Known, name: &str| {
        let shown = shelf(&k.app);
        shown
            .lines()
            .find(|l| l.contains(name))
            .unwrap_or_else(|| panic!("no row for {name}: {shown}"))
            .to_string()
    };
    k.app.dispatch_command_for_test("set mcp.disabled+=jira");
    k.app.dispatch_command_for_test("skills");
    shelf_key(&mut k.app, KeyCode::Tab);
    assert!(
        line(&k, "jira").contains("not known yet"),
        "before any turn"
    );
    shelf_key(&mut k.app, KeyCode::Char('q'));

    k.app.submit("go");
    let server = |name: &str, status: &str| shannon_core::McpServer {
        name: name.into(),
        status: status.into(),
    };
    k.app.feed_stream(StreamMsg::ProviderServers(vec![
        server("github", "connected"),
        server("slack", "failed"),
        server("my server", "connected"),
    ]));
    k.app.feed_stream(StreamMsg::Complete);
    k.app.dispatch_command_for_test("skills");
    shelf_key(&mut k.app, KeyCode::Tab);
    assert!(line(&k, "github").contains("connected"));
    assert!(line(&k, "slack").contains("down"));
    assert!(line(&k, "jira").contains("not started this time"));
    assert_eq!(
        shelf_key(&mut k.app, KeyCode::Enter),
        None,
        "no file opened"
    );
    let shown = shelf(&k.app).replace('\n', " ");
    assert!(shown.contains("`claude mcp` adds, removes"), "{shown}");
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert_eq!(k.app.config.mcp.disabled, ["jira", "github"]);
    assert!(line(&k, "github").contains("disabled"), "its badge");
    let shown = shelf(&k.app).replace('\n', " ");
    assert!(
        shown.contains("a call to any of its tools refused") && !shown.contains("a read of it"),
        "a server's own sentence: {shown}"
    );
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert_eq!(k.app.config.mcp.disabled, ["jira"], "x again enables");
    assert!(!line(&k, "github").contains("disabled"));
    shelf_key(&mut k.app, KeyCode::Char('x'));
    for _ in 0..3 {
        shelf_key(&mut k.app, KeyCode::Char('j'));
    }
    assert_eq!(
        k.app.setup().and_then(|s| s.shelf_selected()),
        Some(PathBuf::from("mcp:jira")),
        "premise: on the row of a server not started"
    );
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert_eq!(
        k.app.config.mcp.disabled,
        ["github"],
        "enabled from its row"
    );
    shelf_key(&mut k.app, KeyCode::Char('q'));
    // Spelled as Claude Code's tools spell it: one server, on the screen
    // and at the gate.
    k.app
        .dispatch_command_for_test("set mcp.disabled+=my_server");
    k.app.dispatch_command_for_test("skills");
    shelf_key(&mut k.app, KeyCode::Tab);
    assert!(line(&k, "my server").contains("disabled"));
    assert!(
        !shelf(&k.app).contains("my_server"),
        "it started: no second row"
    );
    for _ in 0..2 {
        shelf_key(&mut k.app, KeyCode::Char('j'));
    }
    assert_eq!(
        k.app.setup().and_then(|s| s.shelf_selected()),
        Some(PathBuf::from("mcp:my server")),
        "premise"
    );
    shelf_key(&mut k.app, KeyCode::Char('x'));
    assert_eq!(
        k.app.config.mcp.disabled,
        ["github"],
        "its entry taken back"
    );
    shelf_key(&mut k.app, KeyCode::Char('q'));
    k.app
        .dispatch_command_for_test("set mcp.disabled+=my_server");

    let refused = |k: &mut Known, id: &str, tool: &str, input: serde_json::Value| {
        provider_asks(&mut k.app, id, tool, input);
        assert!(k.app.shown_call().is_none(), "{tool}: refused, not asked");
        let answered = provider.answered();
        assert!(
            matches!(
                answered.last(),
                Some((i, shannon_core::hook::Reply::Deny { reason }))
                    if i == id && reason.contains("is disabled")
            ),
            "{tool}: {answered:?}"
        );
    };
    use serde_json::json;
    refused(&mut k, "tu_m", "mcp__github__create_issue", json!({}));
    refused(&mut k, "tu_s", "mcp__my_server__x", json!({}));
    refused(
        &mut k,
        "tu_r",
        "ReadMcpResourceTool",
        json!({ "server": "github", "uri": "repo://x" }),
    );
    refused(
        &mut k,
        "tu_l",
        "ListMcpResourcesTool",
        json!({ "server": "github" }),
    );
    refused(
        &mut k,
        "tu_p",
        "Skill",
        json!({ "skill": "mcp__github__review" }),
    );
    // As `/` lists a prompt (Claude Code's MCP docs).
    refused(
        &mut k,
        "tu_q",
        "Skill",
        json!({ "skill": "/github:review" }),
    );
    provider_asks(&mut k.app, "tu_n", "mcp__slack__post", json!({}));
    assert!(
        k.app.shown_call().is_some(),
        "a server left alone asks, as before"
    );
}

/// **A told file disabled is said to be gone, and nothing more** — the next
/// turn carries its `unloaded` block; what the model had stays in the
/// history, untouched (Steven, 10-06: "we're not going to automatically
/// modify context history").
#[tokio::test]
async fn a_told_file_disabled_is_unloaded_not_erased() {
    let mut k = Known::new();
    let index = k.memory_dir().join("MEMORY.md");
    k.write(&index, "- [g](g.md) — likes green");
    let first = k.turn("hi");
    assert!(
        first.iter().any(|(_, s, _)| s.ends_with("MEMORY.md")),
        "premise: told"
    );
    let path = index.display().to_string();
    k.app
        .dispatch_command_for_test(&format!("set memory.disabled+={path}"));
    let next = k.turn("again");
    assert!(
        next.iter()
            .any(|(kind, s, _)| *kind == ContextKind::Unloaded && s.ends_with("MEMORY.md")),
        "{next:?}"
    );
    let kept = k.app.conversation().messages().iter().any(|m| {
        m.blocks.iter().any(|b| {
            matches!(b, shannon_core::Block::Context { text, .. } if text.contains("likes green"))
        })
    });
    assert!(kept, "the history keeps what was told");
}

/// **A place no rule can spell is said once, not once a session** — both
/// sessions' rules are built, and both meet the same place (Reviewer C,
/// round 28: deleting the de-duplication passed every test).
#[test]
fn an_unspellable_place_is_said_once() {
    let dir = tempfile::tempdir().unwrap();
    let spaced = dir.path().join("with space");
    std::fs::create_dir_all(&spaced).unwrap();
    let mut config = Config::default();
    config.memory.paths = vec![spaced.display().to_string()];
    let mut app = test_app_with(config);
    let start = dir.path().join("proj");
    std::fs::create_dir_all(&start).unwrap();
    let said = app.set_knowledge_roots(Roots {
        start,
        config_dir: Some(dir.path().join("config")),
        project: "proj-1234abcd".into(),
        home: Some(dir.path().join("home")),
        claude_home: Some(dir.path().join("home/.claude")),
        claude_memory: None,
        project_root: shannon_core::config::MemoryConfig::default().project_root,
    });
    let about = said.iter().filter(|s| s.contains("with space")).count();
    assert_eq!(about, 1, "{said:?}");
}

/// **A decision that arrives behind the screen is said on it** — the screen
/// takes every key and is drawn alone, so a turn's tool call waited on a
/// question nothing showed (Reviewer A, round 28; only the `:effort` panel
/// said so). Its bar says it, and which key leaves — `q`: `Esc` stays on
/// this screen (32ac5c9), and the bar said `Esc` until the float.
#[tokio::test]
async fn a_decision_behind_the_screen_is_said_on_it() {
    let provider = shannon_core::backend::RecordingProvider::new();
    let mut app = with_provider(test_app(), &provider);
    let dir = tempfile::tempdir().unwrap();
    let start = dir.path().join("proj");
    std::fs::create_dir_all(&start).unwrap();
    app.set_knowledge_roots(Roots {
        start,
        config_dir: Some(dir.path().join("config")),
        project: "proj-1234abcd".into(),
        home: Some(dir.path().join("home")),
        claude_home: Some(dir.path().join("home/.claude")),
        claude_memory: None,
        project_root: shannon_core::config::MemoryConfig::default().project_root,
    });
    app.dispatch_command_for_test("memories");
    assert!(!text_of(&mut app, 80, 20).contains("a decision is waiting"));
    provider_asks(
        &mut app,
        "tu_1",
        "Bash",
        serde_json::json!({ "command": "rm -rf /tmp/shannon-test-nothing" }),
    );
    assert!(app.shown_call().is_some(), "premise: a question");
    assert!(app.setup().is_some(), "premise: the screen still open");
    let screen = text_of(&mut app, 80, 20);
    assert!(
        screen.contains("a decision is waiting — q leaves"),
        "{screen}"
    );
}

/// **`:memories` is a float over the panes, the file under its cursor
/// beside the list** (Steven, 10-07, images 94–95) — the panes still drawn
/// around it and none focused, its keys on the status row; the preview
/// follows the cursor and is read again after an edit.
#[tokio::test]
async fn memories_float_over_the_panes_with_the_file_beside_them() {
    let mut k = Known::new();
    k.write(&k.start.join("AGENTS.md"), "first rule\nsecond rule\n");
    k.write(&k.start.join("SHANNON.md"), "the other file\n");
    k.app.dispatch_command_for_test("memories");
    let wide = text_of(&mut k.app, 140, 30);
    assert!(wide.contains("memories and skills"), "{wide}");
    assert!(
        wide.contains("AGENTS.md") && wide.contains("second rule"),
        "{wide}"
    );
    let rows: Vec<&str> = wide.lines().collect();
    assert!(
        rows[0].starts_with('┌'),
        "the panes are drawn behind it: {}",
        rows[0]
    );
    assert!(
        rows.last().is_some_and(|r| r.contains("Enter edit")),
        "its keys on the status row: {:?}",
        rows.last()
    );
    // Nothing behind it is focused: no pane's border in the focus colour —
    // each pane's top-left corner, outside the float, looked at.
    let backend = ratatui::backend::TestBackend::new(140, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    draw_frame(&mut k.app, &mut terminal);
    let layout = k
        .app
        .workspace
        .frame_layout(ratatui::layout::Rect::new(0, 0, 140, 30));
    assert!(layout.panes.len() >= 2, "premise: panes behind it");
    for pane in &layout.panes {
        let corner = terminal.backend().buffer()[(pane.rect.x, pane.rect.y)]
            .style()
            .fg;
        assert_ne!(
            corner,
            Some(crate::palette::FOCUSED),
            "{:?} drawn as taking keys behind the float",
            pane.id
        );
    }
    // Narrow: the list alone, at the float's width.
    let narrow = text_of(&mut k.app, 80, 24);
    assert!(
        narrow.contains("AGENTS.md") && !narrow.contains("second rule"),
        "{narrow}"
    );
    // The preview follows the cursor.
    shelf_key(&mut k.app, KeyCode::Char('j'));
    assert!(text_of(&mut k.app, 140, 30).contains("the other file"));
    shelf_key(&mut k.app, KeyCode::Char('k'));
    // An edit is shown at once.
    k.write(&k.start.join("AGENTS.md"), "a third rule\n");
    shelf_key(&mut k.app, KeyCode::Enter);
    assert!(text_of(&mut k.app, 140, 30).contains("a third rule"));
    // A file past what a preview reads is read in part, and says so.
    k.write(&k.start.join("AGENTS.md"), &"a rule\n".repeat(4_000));
    shelf_key(&mut k.app, KeyCode::Enter);
    assert!(text_of(&mut k.app, 140, 30).contains("the file goes on"));
}

/// **The float's list moves by the shared keymap** (Steven, 10-07: "share
/// primitives") — counts, `gg`, `G`, `{n}G` through the keys, the way every
/// pane takes them; what a list of rows has no use for is said, and a `v`
/// leaves no Visual behind.
#[tokio::test]
async fn the_floats_list_moves_by_the_shared_keymap() {
    let mut k = Known::new();
    for name in ["AGENTS.md", "SHANNON.md"] {
        k.write(&k.start.join(name), "a rule\n");
    }
    let memory = k.memory_dir();
    k.write(
        &memory.join("MEMORY.md"),
        "- [one](one.md)\n- [two](two.md)\n",
    );
    k.write(&memory.join("one.md"), "x");
    k.write(&memory.join("two.md"), "y");
    k.app.dispatch_command_for_test("memories");
    let rows: Vec<PathBuf> = {
        let mut rows = Vec::new();
        loop {
            let at = k.app.setup().unwrap().shelf_selected().unwrap();
            if rows.last() == Some(&at) {
                break rows;
            }
            rows.push(at);
            shelf_key(&mut k.app, KeyCode::Char('j'));
        }
    };
    assert!(rows.len() >= 4, "premise: rows to move over: {rows:?}");
    let at = |k: &Known| k.app.setup().unwrap().shelf_selected().unwrap();
    let keys = |k: &mut Known, keys: &str| {
        for c in keys.chars() {
            shelf_key(&mut k.app, KeyCode::Char(c));
        }
    };
    keys(&mut k, "gg");
    assert_eq!(at(&k), rows[0], "gg");
    keys(&mut k, "3j");
    assert_eq!(at(&k), rows[3], "a count");
    keys(&mut k, "G");
    assert_eq!(at(&k), *rows.last().unwrap(), "G");
    keys(&mut k, "2G");
    assert_eq!(at(&k), rows[1], "{{n}}G");
    let said = |k: &Known| shelf(&k.app);
    keys(&mut k, "v");
    assert!(said(&k).contains("a list selects one row"), "{}", said(&k));
    assert_eq!(k.app.chat_keymap.mode(), crate::keymap::Mode::Normal);
    keys(&mut k, "w");
    assert!(said(&k).contains("a row has no columns"), "{}", said(&k));
    keys(&mut k, "/");
    assert!(said(&k).contains("not on this list"), "{}", said(&k));
    assert_eq!(at(&k), rows[1], "nothing refused moved the row");
    // A Visual left standing from a pane (the float opens over it): `Esc`
    // leaves it, and that wants no saying.
    k.app.chat_keymap.translate(crossterm::event::KeyEvent::new(
        KeyCode::Char('v'),
        crossterm::event::KeyModifiers::NONE,
    ));
    assert!(k.app.chat_keymap.mode().is_visual(), "premise");
    shelf_key(&mut k.app, KeyCode::Esc);
    assert!(!said(&k).contains("not on this list"), "{}", said(&k));
    assert!(!k.app.chat_keymap.mode().is_visual());
}

/// **A file the model is not shown is not shown in the preview either** —
/// it says why. An `AGENTS.md` that leads out of the repository is not
/// read (reads stay inside, Steven 10-05); its target's text stays off the
/// screen too.
#[tokio::test]
async fn the_preview_of_a_file_not_read_says_why() {
    let mut k = Known::new();
    let outside = k.start.parent().unwrap().join("outside.txt");
    std::fs::write(&outside, "text from outside\n").unwrap();
    std::os::unix::fs::symlink(&outside, k.start.join("AGENTS.md")).unwrap();
    k.app.dispatch_command_for_test("memories");
    let wide = text_of(&mut k.app, 140, 30);
    assert!(
        wide.contains("not read"),
        "premise: the row says so: {wide}"
    );
    assert!(!wide.contains("text from outside"), "{wide}");
}

/// **Setting `memory=auto` under an open decision about a memory write says
/// what it did to it** — one rebuild, its outcome reported; a second,
/// discarded rebuild ran the write unannounced (Reviewer C, round 27).
#[tokio::test]
async fn memory_auto_under_an_open_write_says_so() {
    let mut off = Config::default();
    off.memory.mode = shannon_core::MemoryMode::Off;
    let mut k = Known::with(off);
    let echo = format!("echo hi > {}/m.md", k.memory_dir().display());
    model_call(&mut k.app, "tu_w", &echo);
    assert!(k.app.shown_call().is_some(), "premise: it asks");
    k.app.dispatch_command_for_test("set memory=auto");
    assert!(
        k.app.messages().any(|m| m.contains("the open decision")),
        "{:?}",
        k.app.messages().collect::<Vec<_>>()
    );
}

/// **A told file's PATH leaves masked too** — it names your home and your
/// project; a value you hold as a secret is a handle there as in the text
/// (Reviewer A, round 27: the source went in clear).
#[tokio::test]
async fn a_told_files_path_is_masked_as_its_text_is() {
    let mut k = Known::new();
    k.write(&k.start.join("AGENTS.md"), "x");
    let dir = k
        ._dir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    k.app.dispatch_command_for_test(&format!("secret {dir}"));
    let told = k.turn("hi");
    let agents = told
        .iter()
        .find(|(_, s, _)| s.ends_with("AGENTS.md"))
        .expect("told");
    assert!(!agents.1.contains(&dir), "{}", agents.1);
    // STORED masked too: the outbound door masks a request, never the
    // session it is filed in (a mutation that left the source in clear
    // passed the check above).
    let stored = format!("{:?}", k.app.buffer().conversation.messages());
    assert!(stored.contains("AGENTS.md"), "premise: the block is stored");
    assert!(!stored.contains(&dir), "{stored}");
}

/// **Claude Code reads its own memories** (Steven, 10-06, image 82: `cat
/// ~/.claude/projects/…/memory/MEMORY.md` DENIED as a credential path, so
/// the model wrote its memory elsewhere). Carved out of `~/.claude` and
/// approved for a Claude Code session; the rest of `~/.claude` is still a
/// credential path, and an API session under `claude = auto` is not
/// approved there. The gate reads text, so the place need not exist.
///
/// **Under THIS home** — the gate adds a `~` spelling only for it, and that
/// is the spelling that asked (review 28, Reviewer A: this test passed on
/// `/Users/someone` while every real read asked). `memory = off` so that
/// vouching makes nothing in the real `~/.claude`; reads are approved all
/// the same. **The path is the real home's on purpose**, and nothing here
/// reads or writes it: the gate decides text, and the vouch's write is
/// stopped by `memory = off` (review stop 12, T4 8).
#[tokio::test]
async fn claude_code_reads_its_own_memories() {
    use shannon_core::permission::Decision;
    let provider = shannon_core::backend::RecordingProvider::new();
    let mut config = Config::default();
    config.memory.mode = shannon_core::MemoryMode::Off;
    let mut app = with_provider(test_app_with(config), &provider);
    let home = shannon_core::secrets::home_dir().expect("tests run with a HOME");
    let theirs = PathBuf::from(format!(
        "{home}/.claude/projects/-shannon-test-absent/memory"
    ));
    let dir = tempfile::tempdir().unwrap();
    let start = dir.path().join("proj");
    std::fs::create_dir_all(&start).unwrap();
    app.set_knowledge_roots(Roots {
        start,
        config_dir: Some(dir.path().join("config")),
        project: "proj-1234abcd".into(),
        home: Some(dir.path().join("home")),
        claude_home: Some(dir.path().join("home/.claude")),
        claude_memory: Some(theirs.clone()),
        project_root: shannon_core::config::MemoryConfig::default().project_root,
    });
    let index = theirs.join("MEMORY.md").display().to_string();
    provider_asks(
        &mut app,
        "tu_1",
        "Read",
        serde_json::json!({ "file_path": index }),
    );
    assert!(app.shown_call().is_none(), "ran unasked");
    assert_eq!(provider.answered(), vec![allowed("tu_1")]);

    provider_asks(
        &mut app,
        "tu_2",
        "Read",
        serde_json::json!({ "file_path": format!("{home}/.claude/.credentials.json") }),
    );
    assert_eq!(
        app.shown_call().map(|p| p.decision()),
        Some(Decision::Deny),
        "the rest of ~/.claude is a credential path still"
    );

    assert_ne!(
        app.rules.decide_line(&format!("cat {index}")).decision,
        Decision::Allow,
        "an API session under `claude = auto` is not approved there"
    );
}

/// **What shows the policy reads the policy of the session in front** —
/// `:analyze`, `:rules`, `:unallow` read the API's alone, so in a Claude Code
/// session `:analyze` called a read of its own memory a question its gate
/// runs unasked, and `:rules` listed the other session's places (Reviewer A,
/// round 28). Asserted from both sides: the same line, two sessions, two
/// verdicts.
#[test]
fn the_policy_shown_is_the_policy_of_the_session_in_front() {
    let mut app = test_app();
    let dir = tempfile::tempdir().unwrap();
    let theirs = dir.path().join("claude/projects/-proj/memory");
    let start = dir.path().join("proj");
    std::fs::create_dir_all(&start).unwrap();
    app.set_knowledge_roots(Roots {
        start,
        config_dir: Some(dir.path().join("config")),
        project: "proj-1234abcd".into(),
        home: Some(dir.path().join("home")),
        claude_home: Some(dir.path().join("home/.claude")),
        claude_memory: Some(theirs.clone()),
        project_root: shannon_core::config::MemoryConfig::default().project_root,
    });
    let line = format!("cat {}", theirs.join("MEMORY.md").display());
    let shown = |app: &mut App| {
        app.set_focus(PaneKind::Output);
        let here = app.workspace.focused();
        app.open_analyze_command(&line);
        let report = app.pane_page(here).expect("a report").text().lines().iter();
        report.map(|l| l.text()).collect::<Vec<_>>().join("\n")
    };
    let place = theirs.display().to_string();

    let api = shown(&mut app);
    assert!(!api.contains("ALLOW"), "an API session asks: {api}");
    assert!(!app.rules_listing().contains(&place), "nor lists the place");

    app.model_command("claude-code/sonnet");
    let claude = shown(&mut app);
    assert!(
        claude.contains("ALLOW"),
        "Claude Code reads its own: {claude}"
    );
    let listed = app.rules_listing();
    assert!(listed.contains(&place), "{listed}");
    assert!(listed.contains("claude_code.path_policy"), "{listed}");
}

/// **What `claude` reads itself is never "unloaded"** — a session that
/// answered through the API, told its CLAUDE.md under `claude = on`, then
/// switched to Claude Code, must not be told that file is gone while
/// `claude` has it (Reviewer A, round 27). Shannon's memory guidance IS:
/// in a Claude Code session claude's own prompt names where memories go
/// (`knowledge::Write::ClaudeCode`), and the old place must not stand.
#[tokio::test]
async fn claude_codes_own_file_is_not_unloaded_by_a_switch() {
    let mut config = Config::default();
    config.memory.claude = shannon_core::ClaudeFiles::On;
    let k = Known::with(config);
    k.write(&k.start.join("CLAUDE.md"), "be terse");
    let first = k.app.told(Session::Api, &[]);
    assert!(
        first
            .iter()
            .any(|b| matches!(b, shannon_core::Block::Context { source, .. } if source.ends_with("CLAUDE.md"))),
        "premise: told through the API"
    );
    let mut sent = shannon_core::Message::user("hi");
    sent.blocks = first;
    let then = k.app.told(Session::ClaudeCode, &[sent]);
    let unloaded: Vec<&str> = then
        .iter()
        .filter_map(|b| match b {
            shannon_core::Block::Context {
                kind: ContextKind::Unloaded,
                source,
                ..
            } => Some(source.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(unloaded, ["shannon:memory"], "{then:?}");
}

/// **Claude Code's own skill, in its session, is `claude`'s to load** —
/// `:skill` sends the prompt alone; Shannon reads none of it (Reviewers A
/// and C, round 27: listed, completed, then refused as a credential path).
#[tokio::test]
async fn claude_codes_own_skill_is_claudes_to_load() {
    let k = Known::new();
    k.write(
        &k.start.join(".claude/skills/cc/SKILL.md"),
        "---\ndescription: theirs\n---\nbody",
    );
    assert_eq!(
        k.app.skill_text(Session::ClaudeCode, "cc", ""),
        Ok(None),
        "claude's"
    );
    assert!(
        k.app.skill_text(Session::Api, "cc", "").is_err(),
        "not found through the API under `claude = auto`"
    );
}

/// **A name a repository chose cannot split a row** — a newline in a
/// skill's directory made two lines of one, and `<Enter>` below it opened
/// its neighbour's file (Reviewer B, round 27). A row is a file now; its
/// name still shows on one line.
#[cfg(unix)]
#[tokio::test]
async fn a_newline_in_a_name_keeps_the_shelf_in_step() {
    let mut k = Known::new();
    k.skill("odd\nname", "split me", "x");
    let after = k.skill("zzz", "after it", "y");
    k.app.dispatch_command_for_test("skills");
    let shown = shelf(&k.app);
    assert!(shown.contains("odd\u{fffd}name"), "{shown}");
    shelf_key(&mut k.app, KeyCode::Char('j'));
    assert_eq!(shelf_key(&mut k.app, KeyCode::Enter), Some(after));
}
