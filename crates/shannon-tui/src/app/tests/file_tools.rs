//! Shannon's own file tools — `Read`, `Edit`, `Write` (`file-tools.md`, F2b):
//! admitted by `App::admit_file_call`, judged as their reading, shown with
//! their change, run through the executor's file seam (a recorder here, so
//! nothing is written), recorded by their tool and file.

use super::*;

/// The model asks for one of the file tools, its turn then complete —
/// the shape a real stream has, as `model_call` gives a `bash` call.
fn model_file_call(app: &mut App, id: &str, name: &str, input: serde_json::Value) {
    if !app.conversation().turn_in_progress() {
        app.conversation_mut().begin_turn("go");
    }
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
    }));
    app.feed_stream(StreamMsg::Complete);
}

/// What the model was told for `id`, once its round returned it.
fn told(app: &App, id: &str) -> Option<String> {
    app.conversation()
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .find_map(|b| match b {
            shannon_core::Block::ToolResult {
                tool_use_id,
                content,
                ..
            } if tool_use_id == id => Some(content.clone()),
            _ => None,
        })
}

/// A test app whose executor the test can read.
fn recorded() -> (App, Arc<shannon_core::RecordingExecutor>) {
    let mut app = test_app();
    let exec = Arc::new(shannon_core::RecordingExecutor::default());
    app.executor = exec.clone();
    (app, exec)
}

/// The file `rel` names under the app's start, as a call's subject says it.
fn under_start(app: &App, rel: &str) -> String {
    app.start
        .as_ref()
        .expect("a start")
        .join(rel)
        .display()
        .to_string()
}

/// **The model reads `path` first** (F3, read-first) — allowed where it
/// asks, and its round answered, so an Edit of it after is admitted.
async fn read_first(app: &mut App, id: &str, path: &str) {
    let input = serde_json::json!({ "file_path": path });
    model_file_call(app, id, "Read", input);
    if app.shown_call().is_some() {
        tool_key(app, 'a');
    }
    settle(app).await;
    assert!(told(app, id).is_some(), "premise: the read answered");
}

/// **A file call inside the working directory runs unasked** (Q7: a read or
/// a write inside, no rule, is allowed) — through the executor's file seam,
/// named by its tool and file, and its answer reaches the model.
#[tokio::test]
async fn a_file_call_inside_runs_unasked_and_answers_its_model() {
    let (mut app, exec) = recorded();
    let input = serde_json::json!({"file_path": "notes.txt", "content": "planted-content\n"});
    model_file_call(&mut app, "tu_w", "Write", input);
    assert!(app.shown_call().is_none(), "no pane for an inside write");
    settle(&mut app).await;
    let file = under_start(&app, "notes.txt");
    assert_eq!(exec.ran(), [format!("Write {file}")]);
    let said = told(&app, "tu_w").expect("the model is answered");
    assert!(
        said.contains(&format!("(recorded, not run) Write {file}")),
        "{said}"
    );
    // Recorded as the call it was, its content shown — a command's record
    // of the subject would show neither.
    let drawn = app.output().lines().join("\n");
    assert!(drawn.contains("planted-content"), "{drawn}");
}

/// **A file call outside asks, showing what it changes** — the pane names
/// the tool and the file, says nothing of Claude Code, and its credential
/// knob is `[secrets]`' (Shannon redacts what its own tools return); a deny
/// runs nothing and tells the model so.
#[tokio::test]
async fn a_file_call_outside_asks_with_its_change_and_a_deny_runs_nothing() {
    let (mut app, exec) = recorded();
    read_first(&mut app, "tu_r", "/tmp/shannon-elsewhere.txt").await;
    let read = exec.ran();
    let input = serde_json::json!({
        "file_path": "/tmp/shannon-elsewhere.txt",
        "old_string": "old line",
        "new_string": "new line",
    });
    model_file_call(&mut app, "tu_e", "Edit", input);
    let prompt = app.shown_call().expect("an outside edit asks");
    let call = prompt.call().expect("shown as a call, not a command line");
    assert_eq!(call.runs, crate::permission_view::Runner::Shannon);
    assert!(call.change.is_some() && call.editable, "{call:?}");
    assert_eq!(
        prompt.question(true),
        (
            "write this?",
            Some("Edit /tmp/shannon-elsewhere.txt".to_string())
        )
    );
    assert_eq!(prompt.credential_knob(), "[secrets] path_policy");
    assert_eq!(
        prompt.command(),
        "cat '/tmp/shannon-elsewhere.txt' > '/tmp/shannon-elsewhere.txt'",
        "judged as a read and a write"
    );
    tool_key(&mut app, 'd');
    assert_eq!(exec.ran(), read, "a deny ran nothing");
    let said = told(&app, "tu_e").expect("the model is answered");
    assert!(said.contains("denied"), "{said}");
    // The refusal names the call and what it would have changed.
    let drawn = app.output().lines().join("\n");
    assert!(
        drawn.contains("DENIED") && drawn.contains("new line"),
        "{drawn}"
    );
}

/// **`e` edits the text an Edit writes, and the model is told** — the
/// seed is its `new_string`, the edit is what runs, and the answer says the
/// file holds the user's text, not the model's.
#[tokio::test]
async fn an_edited_file_call_runs_the_edit_and_says_so() {
    let (mut app, exec) = recorded();
    read_first(&mut app, "tu_r", "/tmp/shannon-elsewhere.txt").await;
    let input = serde_json::json!({
        "file_path": "/tmp/shannon-elsewhere.txt",
        "old_string": "a",
        "new_string": "the model's",
    });
    model_file_call(&mut app, "tu_e", "Edit", input);
    let (seed, _) = app
        .edit_seed_for_test()
        .expect("an Edit's text is editable");
    assert_eq!(seed, "the model's");
    app.apply_edit_for_test(&seed, "the user's");
    let call = app
        .shown_call()
        .and_then(|p| p.call())
        .expect("still the call");
    let shows = format!("{:?}", call.change);
    assert!(
        shows.contains("the user's"),
        "the pane shows the edit: {shows}"
    );
    tool_key(&mut app, 'a');
    settle(&mut app).await;
    assert_eq!(
        exec.ran().last().unwrap(),
        "Edit /tmp/shannon-elsewhere.txt"
    );
    let said = told(&app, "tu_e").expect("answered");
    assert!(
        said.starts_with("NOTE: the user changed your text"),
        "{said}"
    );
}

/// **What is no file call is refused with the reason, and nothing asks or
/// runs** — a key the tool does not take, and one it needs left out.
#[tokio::test]
async fn a_file_call_that_cannot_run_is_refused_with_its_reason() {
    let (mut app, exec) = recorded();
    model_file_call(
        &mut app,
        "tu_r",
        "Read",
        serde_json::json!({"file_path": "a.rs", "pages": "1-5"}),
    );
    let said = told(&app, "tu_r").expect("answered at once");
    assert!(said.contains("`pages` is not one"), "{said}");
    model_file_call(
        &mut app,
        "tu_w",
        "Write",
        serde_json::json!({"file_path": "a.txt"}),
    );
    let said = told(&app, "tu_w").expect("answered at once");
    assert!(said.contains("needs `content`"), "{said}");
    assert!(app.shown_call().is_none(), "nothing asked");
    assert!(exec.ran().is_empty(), "nothing ran: {:?}", exec.ran());
}

/// **A secret's handle in what a file call writes is a hard ask** (S21-Q27,
/// Steven: "as bash") — inside the project, where a write runs unasked, it
/// asks, its line saying the spend; approved, it runs with the handle in
/// it, for the run to put the value in (`exec::run_file`). An `e` that
/// takes the handle out is judged again and runs as approved — not refused
/// as a call changed since it was judged.
#[tokio::test]
async fn a_secret_a_file_call_writes_is_a_hard_ask() {
    let (mut app, exec) = recorded();
    let mut table = shannon_core::Aliases::default();
    let handle = table
        .insert("db", "planted-db-value", "test", true)
        .unwrap();
    app.aliases = Arc::new(table);
    let write = |content: String| serde_json::json!({"file_path": "notes.txt", "content": content});
    model_file_call(&mut app, "tu_w", "Write", write(format!("KEY={handle}\n")));
    let prompt = app.shown_call().expect("a hard ask, inside too");
    let spend = format!("printf '%s' '{handle}' > ");
    assert!(prompt.command().contains(&spend), "{}", prompt.command());
    tool_key(&mut app, 'a');
    settle(&mut app).await;
    let jobs = exec.jobs();
    let written = match &jobs[0].call {
        shannon_core::files::Call::Write { content, .. } => content.clone(),
        other => panic!("a Write: {other:?}"),
    };
    assert_eq!(
        written,
        format!("KEY={handle}\n"),
        "the handle, for the run"
    );

    model_file_call(&mut app, "tu_w2", "Write", write(format!("KEY={handle}\n")));
    assert!(app.shown_call().is_some(), "premise: it asks");
    let (seed, _) = app.edit_seed_for_test().expect("its text is editable");
    app.apply_edit_for_test(&seed, "KEY=plain");
    let command = app
        .shown_call()
        .map(|p| p.command().to_string())
        .unwrap_or_default();
    assert!(
        !command.contains("printf"),
        "judged again, the spend gone: {command}"
    );
    tool_key(&mut app, 'a');
    settle(&mut app).await;
    let told = told(&app, "tu_w2").expect("answered");
    assert!(!told.contains("refused"), "run as approved: {told}");
    assert_eq!(exec.jobs().len(), 2, "{:?}", exec.ran());
}

/// **An Edit whose `new_string` spends a handle asks, inside too; one that
/// only finds text by a handle does not** (review stop 16, E 2, E's probe:
/// the Edit arm of `Call::spends` emptied, every suite passed, and the edit
/// wrote the value into the file unasked).
#[tokio::test]
async fn an_edit_spending_a_handle_is_a_hard_ask() {
    let (mut app, exec) = recorded();
    let mut table = shannon_core::Aliases::default();
    let handle = table
        .insert("db", "planted-db-value", "test", true)
        .unwrap();
    app.aliases = Arc::new(table);
    let file = under_start(&app, "notes.txt");
    read_first(&mut app, "tu_r", &file).await;
    let edit = |old: String, new: String| serde_json::json!({"file_path": file, "old_string": old, "new_string": new});
    model_file_call(
        &mut app,
        "tu_e",
        "Edit",
        edit("KEY=old".into(), format!("KEY={handle}")),
    );
    let prompt = app.shown_call().expect("a hard ask, inside too");
    let spend = format!("printf '%s' '{handle}' > ");
    assert!(prompt.command().contains(&spend), "{}", prompt.command());
    tool_key(&mut app, 'd');
    settle(&mut app).await;

    let ran = exec.ran().len();
    model_file_call(
        &mut app,
        "tu_e2",
        "Edit",
        edit(format!("KEY={handle}"), "KEY=plain".into()),
    );
    assert!(
        app.shown_call().is_none(),
        "finding by a handle writes none of it"
    );
    settle(&mut app).await;
    assert_eq!(exec.ran().len(), ran + 1, "{:?}", exec.ran());
}

/// A start of its own with a link `notes` in it to `secret` outside it, and
/// a plain file beside the link — temp directories as spelled, so the
/// system's root links (`/tmp`, `/var` on macOS) keep their spelling as the
/// landing does.
fn linked_out(app: &mut App) -> (tempfile::TempDir, tempfile::TempDir) {
    let (start, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(outside.path().join("secret"), "x").unwrap();
    std::fs::write(start.path().join("plain.txt"), "x").unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret"), start.path().join("notes")).unwrap();
    // Started there, through the door production uses (`plain_file`'s).
    app.start = Some(start.path().to_path_buf());
    app.rebuild_rules();
    (start, outside)
}

/// **A file call through a link is judged where it lands** (F2c) — a read
/// of a link in the project to a file outside asks, as a read of that file
/// does, its pane naming both; a plain file beside it still reads unasked.
/// Allowed, it runs on the landing judged.
#[tokio::test]
async fn a_file_call_through_a_link_is_judged_where_it_lands() {
    let (mut app, exec) = recorded();
    let (_start, outside) = linked_out(&mut app);
    model_file_call(
        &mut app,
        "tu_p",
        "Read",
        serde_json::json!({"file_path": "plain.txt"}),
    );
    assert!(
        app.shown_call().is_none(),
        "a plain read inside runs unasked"
    );
    settle(&mut app).await;

    model_file_call(
        &mut app,
        "tu_n",
        "Read",
        serde_json::json!({"file_path": "notes"}),
    );
    let (notes, secret) = (
        under_start(&app, "notes"),
        std::fs::canonicalize(outside.path())
            .unwrap()
            .join("secret")
            .display()
            .to_string(),
    );
    let prompt = app.shown_call().expect("a link out of the project asks");
    assert_eq!(
        prompt.question(true),
        ("allow this?", Some(format!("Read {notes} → {secret}")))
    );
    assert_eq!(prompt.command(), format!("cat '{notes}'; cat '{secret}'"));
    tool_key(&mut app, 'a');
    settle(&mut app).await;
    assert_eq!(
        exec.ran(),
        [
            format!("Read {}", under_start(&app, "plain.txt")),
            format!("Read {notes} → {secret}")
        ]
    );
}

/// **A write to a file of two names asks inside the project, and says why**
/// (review stop 16, E 3; Steven, 11-04: "default to asking and saying why")
/// — a hard link in the project to a file outside: the pane says the file
/// has 2 names, written in place under each, and offers no rule, since none
/// would answer it; allowed once, it runs. A write of a file of one name
/// beside it runs unasked, as ever.
#[tokio::test]
async fn a_write_to_a_file_of_two_names_asks_and_says_why() {
    let (mut app, exec) = recorded();
    let (start, outside) = linked_out(&mut app);
    std::fs::hard_link(outside.path().join("secret"), start.path().join("rc")).unwrap();
    let write = |file: &str| serde_json::json!({"file_path": file, "content": "y"});
    model_file_call(&mut app, "tu_p", "Write", write("plain.txt"));
    assert!(app.shown_call().is_none(), "a file of one name: unasked");
    settle(&mut app).await;

    model_file_call(&mut app, "tu_rc", "Write", write("rc"));
    let rc = under_start(&app, "rc");
    let prompt = app
        .shown_call()
        .expect("a file of two names asks, inside too");
    assert!(
        prompt
            .command()
            .ends_with(&format!("; cat > $(find / -samefile '{rc}')")),
        "{}",
        prompt.command()
    );
    let said: Vec<String> = (prompt.text().lines().iter())
        .map(|line| line.text().to_string())
        .collect();
    assert!(
        said.iter()
            .any(|line| line.contains(&format!("{rc} has 2 names"))),
        "{said:#?}"
    );
    assert!(prompt.proposal().is_err(), "{:?}", prompt.proposal());
    tool_key(&mut app, 'a');
    settle(&mut app).await;
    assert_eq!(exec.ran().last(), Some(&format!("Write {rc}")));
}

/// **A link pointed elsewhere after the judgment stops the run** — the call
/// allowed was the one judged, so one now landing on another file is refused
/// with why, nothing run, and the model told.
#[tokio::test]
async fn a_link_moved_after_the_judgment_refuses_the_run() {
    let (mut app, exec) = recorded();
    let (start, outside) = linked_out(&mut app);
    model_file_call(
        &mut app,
        "tu_n",
        "Read",
        serde_json::json!({"file_path": "notes"}),
    );
    assert!(app.shown_call().is_some(), "premise: it asks");
    let notes = start.path().join("notes");
    std::fs::remove_file(&notes).unwrap();
    std::fs::write(outside.path().join("other"), "x").unwrap();
    std::os::unix::fs::symlink(outside.path().join("other"), &notes).unwrap();
    // Refused at the run, at once: no outcome comes (a `settle` here waited
    // its whole ten seconds for one).
    tool_key(&mut app, 'a');
    assert!(exec.ran().is_empty(), "nothing ran: {:?}", exec.ran());
    let said = told(&app, "tu_n").expect("the model is answered");
    assert!(said.contains("not where it was"), "{said}");
}

/// A session on the stub is answered by the app's own (`App::stub`), at the
/// cadence a person watches — 4 s a test that waits on it. This one's is
/// fast.
fn fast_stub(app: &mut App) {
    app.stub = Arc::new(shannon_core::backend::CannedStub::with_cadence(
        std::time::Duration::from_millis(1),
        64,
    ));
}

/// **A second stub session's call runs too** — each of the stub's calls has
/// an id of its own: by turn, a second session's first call reused the
/// first's (`toolu_stub_1_0`), and the app refused it as an id it had
/// issued (`App::claim_tool_id`).
#[tokio::test]
async fn a_second_stub_sessions_call_is_not_a_duplicate() {
    let (mut app, exec) = recorded();
    fast_stub(&mut app);
    for round in 1..=2 {
        stub_turn(
            &mut app,
            "```tool Read\n{\"file_path\": \"Cargo.toml\"}\n```",
        )
        .await;
        assert_eq!(exec.ran().len(), round, "{:?}", app.notice());
        app.dispatch_command_for_test("new");
    }
}

/// **An Edit of a file the context never read is refused before any pane**
/// (F3, `[files] read_first = "any"`, shipped) — said to the model,
/// nothing run, inside the project too, where it would have run unasked;
/// `"off"` lets it through.
#[tokio::test]
async fn an_edit_of_a_file_never_read_is_refused_before_any_pane() {
    let edit =
        |path: &str| serde_json::json!({"file_path": path, "old_string": "a", "new_string": "b"});
    let (mut app, exec) = recorded();
    model_file_call(&mut app, "tu_e", "Edit", edit("/tmp/shannon-elsewhere.txt"));
    assert!(
        app.shown_call().is_none(),
        "no pane for a change that cannot run"
    );
    let said = told(&app, "tu_e").expect("answered at once");
    assert!(said.contains("has not been read"), "{said}");
    model_file_call(&mut app, "tu_i", "Edit", edit("notes.txt"));
    let said = told(&app, "tu_i").expect("answered at once");
    assert!(said.contains("has not been read"), "inside too: {said}");
    assert!(exec.ran().is_empty(), "nothing ran: {:?}", exec.ran());

    let mut config = Config::default();
    config.files.read_first = shannon_core::config::ReadFirst::Off;
    let mut app = test_app_with(config);
    let exec = Arc::new(shannon_core::RecordingExecutor::default());
    app.executor = exec.clone();
    model_file_call(&mut app, "tu_i", "Edit", edit("notes.txt"));
    settle(&mut app).await;
    assert_eq!(
        exec.ran(),
        [format!("Edit {}", under_start(&app, "notes.txt"))]
    );
}

/// **A Read lets an Edit of its file through, run under what it saw** — the
/// Read's stamp is in the job the Edit runs as (`files::Context`), for the
/// run to check it is still the file; and only in the session whose
/// conversation holds the Read: another session's Edit of the same file is
/// refused, its context never read it (not a side cache).
#[tokio::test]
async fn a_read_lets_an_edit_of_its_file_through_in_its_own_session_alone() {
    const READ: &str = "```tool Read\n{\"file_path\": \"notes.txt\"}\n```";
    const EDIT: &str = "```tool Edit\n{\"file_path\": \"notes.txt\", \"old_string\": \"a\", \"new_string\": \"b\"}\n```";
    let (mut app, exec) = recorded();
    fast_stub(&mut app);
    // Each a turn of its own, through the stub, so each ends as a real one.
    stub_turn(&mut app, READ).await;
    stub_turn(&mut app, EDIT).await;
    let file = std::path::PathBuf::from(under_start(&app, "notes.txt"));
    let jobs = exec.jobs();
    assert_eq!(jobs.len(), 2, "the Read, then the Edit: {:?}", exec.ran());
    assert!(matches!(
        jobs[1].call,
        shannon_core::files::Call::Edit { .. }
    ));
    let stamps = &jobs[1].context.stamps;
    assert!(
        stamps.len() == 1 && stamps[0].path == file && stamps[0].shown,
        "{stamps:?}"
    );

    app.dispatch_command_for_test("new");
    app.submit(EDIT);
    pump(&mut app).await;
    pump(&mut app).await;
    assert_eq!(
        exec.jobs().len(),
        2,
        "another session's Edit ran: {:?}",
        exec.ran()
    );
    let drawn = app.output().lines().join("\n");
    assert!(
        drawn.contains("has not been read"),
        "its context never read it:\n{drawn}"
    );
}

/// **A stamp counts only for its file, and only while its very result is in
/// the context** (`stamps_handed`) — a result edited since it was handed
/// back (an edit of history) lends nothing, nor does a call whose result is
/// not in the messages at all, nor another file's stamp.
#[test]
fn a_stamp_counts_while_its_very_result_is_in_the_context() {
    use shannon_core::files::{Lines, Stamp};
    let stamp = |path: &str| Stamp {
        path: path.into(),
        seen: Lines::span(1, 3),
        total: 3,
        hash: [7; 32],
        shown: true,
    };
    let bound = |content: &str| *blake3::hash(content.as_bytes()).as_bytes();
    let stamps = std::collections::HashMap::from([
        ("t_a".to_string(), (bound("1\ta"), stamp("/w/a.rs"))),
        ("t_b".to_string(), (bound("1\tb"), stamp("/w/b.rs"))),
        ("t_gone".to_string(), (bound("1\ta"), stamp("/w/a.rs"))),
    ]);
    let messages = [
        shannon_core::Message::tool_result("t_a", "1\ta", false),
        shannon_core::Message::tool_result("t_b", "1\tb", false),
    ];
    let found = stamps_handed(&messages, &stamps, std::path::Path::new("/w/a.rs"));
    assert_eq!(
        found,
        [stamp("/w/a.rs")],
        "its own, once: t_gone is not in the context"
    );
    let edited = [shannon_core::Message::tool_result(
        "t_a",
        "1\tedited",
        false,
    )];
    let none = stamps_handed(&edited, &stamps, std::path::Path::new("/w/a.rs"));
    assert_eq!(none, [], "a result edited since lends nothing");
}

/// The backend's stream drained into `app` until its turn stops — the
/// loop's stream arm, as `drain_budget_never_loses_messages` emulates it.
async fn pump(app: &mut App) {
    let wait = std::time::Duration::from_secs(10);
    while app.streaming() {
        match tokio::time::timeout(wait, app.stream_rx.recv()).await {
            Ok(Some(msg)) => app.drain_stream(msg),
            _ => panic!("the stream stalled"),
        }
    }
}

/// **One turn through the stub whose call runs unasked** — `prompt` sent,
/// its call asked for, its outcome handled, and the round answered to the
/// turn's end: the loop's stream and exec arms, in the order they fire.
async fn stub_turn(app: &mut App, prompt: &str) {
    app.submit(prompt);
    pump(app).await;
    settle(app).await;
    pump(app).await;
}

/// **The offline stub asks for the calls a prompt writes out** (F2d), and
/// the whole path answers them — through the real stream, not a fed one: a
/// `tool Read` block inside the project runs unasked through the file seam,
/// and the stub's next reply says what the model was handed; a `tool bash`
/// block that deletes asks, and a deny reaches the stub as an error, nothing
/// run.
#[tokio::test]
async fn the_stub_asks_for_the_calls_a_prompt_writes_out() {
    let (mut app, exec) = recorded();
    fast_stub(&mut app);
    stub_turn(
        &mut app,
        "```tool Read\n{\"file_path\": \"Cargo.toml\"}\n```",
    )
    .await;
    let file = under_start(&app, "Cargo.toml");
    assert_eq!(exec.ran(), [format!("Read {file}")]);
    let drawn = app.output().lines().join("\n");
    assert!(
        drawn.contains("`Read` answered:") && drawn.contains("(recorded, not run) Read"),
        "{drawn}"
    );

    app.submit("```tool bash\nrm /tmp/shannon-stub-never-made\n```");
    pump(&mut app).await;
    assert!(app.shown_call().is_some(), "a delete asks");
    tool_key(&mut app, 'd');
    pump(&mut app).await;
    let drawn = app.output().lines().join("\n");
    assert!(drawn.contains("`bash` answered, as an error:"), "{drawn}");
    assert_eq!(
        exec.ran().len(),
        1,
        "the deny ran nothing: {:?}",
        exec.ran()
    );
}

/// **A child reads inside unasked and holds on a write** — what is safe
/// with nobody watching runs (a read inside); a change does not, and the
/// child stops, as on a command that changes anything.
#[tokio::test]
async fn an_unattended_child_reads_inside_and_holds_on_a_write() {
    let (mut app, exec) = recorded();
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    let use_ = |id: &str, name: &str, input| {
        StreamMsg::ToolUse(shannon_core::Block::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        })
    };
    app.feed_job(
        child,
        use_(
            "tu_r",
            "Read",
            serde_json::json!({"file_path": "Cargo.toml"}),
        ),
    );
    assert_eq!(
        exec.ran(),
        [format!("Read {}", under_start(&app, "Cargo.toml"))]
    );
    assert_eq!(app.held_children(), 0, "a read inside does not hold");
    let write = serde_json::json!({"file_path": "out.txt", "content": "x"});
    app.feed_job(child, use_("tu_w", "Write", write));
    assert_eq!(app.held_children(), 1, "a write holds the child");
    assert_eq!(exec.ran().len(), 1, "and did not run: {:?}", exec.ran());
}
