//! Claude Code as a provider: its gate, its hook, and what its calls say.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// --- Claude Code as a provider ----------------------------------------------

/// **A call that ran without passing the gate stops the turn, and says
/// so** — what ran stays on the record (it ran), the reason is in the
/// transcript where the turn stopped, and the turn is sealed as cut off.
/// The relay's ending for a successful result whose id was never asked
/// (`disableAllHooks`, review 21, Reviewer B); no TUI test drove it (review
/// stop 12, T4 3).
#[tokio::test]
async fn an_ungated_claude_code_call_stops_the_turn_and_says_why() {
    use shannon_core::Block;
    let (mut app, _) = provider_app();
    app.model_command("claude-code/sonnet");
    app.submit("go");
    app.feed_stream(StreamMsg::ToolRecord {
        call: Block::ToolUse {
            id: "tu_1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "touch /tmp/shannon-test-absent-ungated"}),
        },
        result: Block::ToolResult {
            tool_use_id: "tu_1".into(),
            content: String::new(),
            is_error: false,
        },
    });
    let why = "`Bash` ran without passing Shannon's gate, so the turn was stopped — a \
               Claude Code setting may turn hooks off";
    app.feed_stream(StreamMsg::Error(BackendError::ClaudeCode(why.into())));

    let shown = app.output().lines().join("\n");
    assert!(
        shown.contains("Claude Code ran:")
            && shown.contains("touch /tmp/shannon-test-absent-ungated"),
        "what ran is on the record: {shown}"
    );
    assert!(
        shown.contains("ran without passing Shannon's gate"),
        "and why the turn stopped, where it stopped: {shown}"
    );
    assert!(!app.streaming(), "the turn ended");
    let last = (app.conversation().own_turns().pop()).expect("the turn, sealed");
    assert_eq!(
        last.provenance.stop_reason,
        Some(shannon_core::StopReason::Interrupted),
        "as cut off"
    );
    assert!(
        (last.messages.iter().flat_map(|m| &m.blocks))
            .any(|b| matches!(b, Block::ToolUse { name, .. } if name == "Bash")),
        "holding the call that ran: {:?}",
        last.messages
    );
}

/// **Claude Code's calls go through Shannon's gate, and the answers back
/// through its hook.** A read the rules allow is answered at once; a command
/// they ask about waits for the human, whose answer is what goes back. Shannon
/// runs neither, and neither leaves a `tool_use` waiting on it — which would
/// park the turn for a result Shannon never produces.
#[tokio::test]
async fn claude_codes_calls_are_judged_by_the_gate_and_answered_through_the_hook() {
    use shannon_core::hook::Reply;
    let (mut app, provider) = provider_app();
    let dir = TempDir::new().unwrap();
    let file = plain_file(&mut app, &dir);

    provider_asks(
        &mut app,
        "tu_1",
        "Read",
        serde_json::json!({ "file_path": file }),
    );
    assert!(
        app.shown_call().is_none(),
        "a read the rules allow asks nobody"
    );
    assert_eq!(provider.answered(), vec![allowed("tu_1")]);

    provider_asks(
        &mut app,
        "tu_2",
        "Bash",
        serde_json::json!({"command": "rm -rf /tmp/shannon-test-absent"}),
    );
    assert!(
        app.shown_call().is_some(),
        "a command the rules ask about waits"
    );
    assert_eq!(
        provider.answered().len(),
        1,
        "…and is not answered before the human"
    );
    tool_key(&mut app, 'd');
    assert_eq!(
        provider.answered()[1],
        (
            "tu_2".to_string(),
            Reply::Deny {
                reason: "denied by the user".into()
            }
        )
    );

    assert!(app.running_tools.is_empty(), "Shannon ran nothing");
    assert!(
        !app.conversation().has_pending_tool_use(),
        "a question never leaves a tool_use waiting on Shannon"
    );
    let shown = app.output().lines().join("\n");
    // Named as the call it is — the tool and its file — not as the gate
    // reads it (`cat '…'`; review, Reviewers A and C).
    assert!(
        shown.contains(&format!("Claude Code runs: Read {file}")),
        "{shown}"
    );
    assert!(!shown.contains("cat '"), "{shown}");
    assert!(shown.contains("DENIED"), "{shown}");
}

/// **A Grep of a file is a read; a Grep of a tree asks** (Steven, 10-04) —
/// through the filesystem, as production asks it.
#[tokio::test]
async fn a_grep_of_a_file_reads_it_and_a_grep_of_a_tree_asks() {
    let (mut app, provider) = provider_app();
    let dir = TempDir::new().unwrap();
    let file = plain_file(&mut app, &dir);
    provider_asks(
        &mut app,
        "tu_1",
        "Grep",
        serde_json::json!({ "pattern": "name", "path": file }),
    );
    assert!(
        app.shown_call().is_none(),
        "a file: an ordinary read, allowed"
    );
    assert_eq!(provider.answered(), vec![allowed("tu_1")]);

    let tree = dir.path().display().to_string();
    let src = serde_json::json!({ "pattern": "name", "path": tree });
    provider_asks(&mut app, "tu_2", "Grep", src);
    let prompt = app.shown_call().expect("a directory is a tree: it asks");
    assert_eq!(prompt.command(), format!("grep -r -e 'name' '{tree}'"));
    assert_eq!(provider.answered().len(), 1);
}

/// **A second question for one call refuses both** (review of `buffers.md`,
/// 10-15, Reviewer B F2). Claude Code asks once per call, and any process
/// holding the gate's socket can ask naming a live one: which of the two is
/// Claude Code's cannot be told, and an allow to either would be recorded as
/// Claude Code running it. Both are answered "no", each by its own question,
/// and the decision leaves the pane saying why.
#[tokio::test]
async fn a_second_question_for_one_call_refuses_both() {
    use shannon_core::hook::Reply;
    let (mut app, provider) = provider_app();
    let rm = serde_json::json!({"command": "rm -rf /tmp/shannon-test-absent"});
    provider_asks(&mut app, "tu_1", "Bash", rm);
    assert!(
        app.shown_call().is_some(),
        "premise: the first is on the pane"
    );
    app.notices.clear();
    let second = shannon_core::AskId::for_test("tu_1", 2);
    app.feed_stream(StreamMsg::ToolAsk(shannon_core::ToolQuestion {
        id: "tu_1".into(),
        name: "Bash".into(),
        input: serde_json::json!({"command": "ls"}),
        cwd: here(&app),
        ask: second.clone(),
        answer_by: hook_answer_by(),
    }));
    let refused = |r: &Reply| matches!(r, Reply::Deny { .. });
    let answered = provider.answered();
    assert!(
        answered.len() == 2 && answered.iter().all(|(id, r)| id == "tu_1" && refused(r)),
        "{answered:?}"
    );
    let asked = provider.asked.lock().unwrap().clone();
    assert!(
        asked.contains(&second) && asked.contains(&question("tu_1")),
        "each by its own question: {asked:?}"
    );
    assert!(app.shown_call().is_none(), "the decision left the pane");
    assert!(
        app.notices
            .iter()
            .any(|(_, n)| n.contains("a second question claimed the same call")),
        "{:?}",
        app.notices
    );
}

/// **A withdrawal takes down only the question it names** — a second
/// asker's connection closing, under a serial the call was not asked as,
/// leaves the decision where it is; by id it took it off the pane.
#[tokio::test]
async fn a_withdrawal_takes_down_only_its_own_question() {
    let (mut app, _) = provider_app();
    let rm = serde_json::json!({"command": "rm -rf /tmp/shannon-test-absent"});
    provider_asks(&mut app, "tu_1", "Bash", rm);
    let impostor = shannon_core::AskId::for_test("tu_1", 2);
    app.feed_stream(StreamMsg::ToolWithdrawn(impostor));
    assert!(app.shown_call().is_some(), "still on the pane");
    app.feed_stream(StreamMsg::ToolWithdrawn(question("tu_1")));
    assert!(app.shown_call().is_none(), "its own withdrawal takes it");
}

/// **An allow goes to the question it was asked as** — the record's serial
/// (`Executor::Provider`'s `ask`), not one made from the call's id.
#[tokio::test]
async fn an_allow_goes_to_the_question_it_was_asked_as() {
    let (mut app, provider) = provider_app();
    let asked = shannon_core::AskId::for_test("tu_9", 41);
    app.conversation_mut().begin_turn("go");
    app.feed_stream(StreamMsg::ToolAsk(shannon_core::ToolQuestion {
        id: "tu_9".into(),
        name: "Bash".into(),
        input: serde_json::json!({"command": "rm -rf /tmp/shannon-test-absent"}),
        cwd: here(&app),
        ask: asked.clone(),
        answer_by: hook_answer_by(),
    }));
    tool_key(&mut app, 'a');
    assert_eq!(*provider.asked.lock().unwrap(), vec![asked]);
}

/// **A question Claude Code stopped waiting for leaves the screen** — the
/// pane closes, the next queued question takes it, the record says nothing
/// ran — an allow given after the hook is gone records "not delivered", never
/// "runs", and a turn that FINISHES with a question open takes it down too
/// (review 21, Reviewer A).
#[tokio::test]
async fn a_question_claude_code_stopped_waiting_for_leaves_the_screen() {
    let (mut app, provider) = provider_app();
    let fetch = |url: &str| serde_json::json!({ "url": url });
    provider_asks(&mut app, "tu_1", "WebFetch", fetch("https://a.example"));
    provider_asks(&mut app, "tu_2", "WebFetch", fetch("https://b.example"));
    let on_pane = |app: &App| app.shown_call().map(|p| p.tool_use_id().to_string());
    assert_eq!(on_pane(&app).as_deref(), Some("tu_1"));
    let cancelled = |logs: &str, id: &str| {
        (logs.lines())
            .filter(|l| {
                l.contains(r#"decision="cancelled""#)
                    && l.contains(&format!(r#"tool_use_id="{id}""#))
            })
            .count()
    };

    // Audited as cancelled — its audit line was held by no test (review stop
    // 8, E2 6).
    let logs = captured_logs(|| app.feed_stream(StreamMsg::ToolWithdrawn(question("tu_1"))));
    assert_eq!(cancelled(&logs, "tu_1"), 1, "{logs}");
    assert_eq!(
        on_pane(&app).as_deref(),
        Some("tu_2"),
        "the next question takes the pane"
    );
    let shown = app.output().lines().join("\n");
    assert!(
        shown.contains("WITHDRAWN") && shown.contains("https://a.example"),
        "{shown}"
    );
    assert!(
        provider.answered().is_empty(),
        "nothing was answered for it"
    );

    provider.gone.lock().unwrap().push("tu_2".into());
    app.notices.clear();
    let logs = captured_logs(|| {
        tool_key(&mut app, 'a');
    });
    let shown = app.output().lines().join("\n");
    assert!(shown.contains("NOT delivered"), "{shown}");
    assert!(!shown.contains("Claude Code runs"), "{shown}");
    // Said and audited as a withdrawal is — it had the transcript's line
    // alone (review stop 8, E2 6).
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("not delivered — Claude Code had stopped waiting"),
        "{said}"
    );
    assert_eq!(cancelled(&logs, "tu_2"), 1, "{logs}");

    provider_asks(&mut app, "tu_3", "WebFetch", fetch("https://c.example"));
    assert!(app.shown_call().is_some());
    app.feed_stream(StreamMsg::Complete);
    assert!(
        app.shown_call().is_none(),
        "the finished turn's question went with it"
    );
    assert!(app.tool_calls.is_empty(), "and nothing is left tracked");
}

/// **An allow at the hook's deadline is refused, not sent** (review stop 8,
/// E2 6) — "delivered" is only the hand-off, so an allow within a second of
/// the deadline could land after the hook refused, recorded "Claude Code
/// runs" for a call it never ran. A deny goes, said as the deadline's — and
/// the line it names is read as a command, a credential after its flag kept
/// out (review stop 17, E1 M1).
#[tokio::test]
async fn an_allow_at_the_hooks_deadline_is_refused_not_sent() {
    use shannon_core::hook::Reply;
    let (mut app, provider) = provider_app();
    app.let_time_pass(Duration::ZERO);
    app.conversation_mut().begin_turn("go");
    app.feed_stream(StreamMsg::ToolAsk(shannon_core::ToolQuestion {
        id: "tu_1".into(),
        name: "Bash".into(),
        input: serde_json::json!({"command": "rm /tmp/shannon-test-x --password hunter2"}),
        cwd: here(&app),
        ask: question("tu_1"),
        answer_by: std::time::Instant::now().checked_add(Duration::from_millis(500)),
    }));
    assert!(app.shown_call().is_some(), "premise: it asks");
    tool_key(&mut app, 'a');
    let answered = provider.answered();
    assert!(
        matches!(answered.as_slice(), [(id, Reply::Deny { .. })] if id == "tu_1"),
        "{answered:?}"
    );
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("refused — unanswered for 59 minutes"),
        "{said}"
    );
    assert!(!said.contains("hunter2"), "{said}");
    let shown = app.output().lines().join("\n");
    assert!(!shown.contains("Claude Code runs"), "{shown}");
}

/// A question re-using an id this session already saw is refused through the
/// hook, rather than left waiting out the turn (review 21, Reviewer A).
#[tokio::test]
async fn a_claude_code_question_with_a_used_id_is_refused_to_its_hook() {
    use shannon_core::hook::Reply;
    let (mut app, provider) = provider_app();
    let dir = TempDir::new().unwrap();
    let read = serde_json::json!({ "file_path": plain_file(&mut app, &dir) });
    provider_asks(&mut app, "tu_1", "Read", read.clone());
    provider_asks(&mut app, "tu_1", "Read", read);
    assert!(
        matches!(&provider.answered()[..], [_, (id, Reply::Deny { .. })] if id == "tu_1"),
        "{:?}",
        provider.answered()
    );
}

/// **A withdrawn question waiting in the queue never runs as Shannon's own**
/// (review stop 7, Reviewer E 1, its probe). Withdrawn, its record went and
/// its queue entry stayed; at the front of the queue, with no record, the
/// lookups that default on a missing one (`Executor::Shannon`,
/// `Judge::Rules`) put Claude Code's command on the pane as a call of
/// Shannon's, and `a` ran it in Shannon's shell.
#[tokio::test]
async fn a_withdrawn_queued_question_never_runs_as_shannons() {
    let (mut app, provider) = provider_app();
    let exec = Arc::new(shannon_core::RecordingExecutor::default());
    app.executor = exec.clone();
    let touch = |file: &str| serde_json::json!({ "command": format!("touch {file}") });
    provider_asks(&mut app, "tu_1", "Bash", touch("notes.txt"));
    provider_asks(&mut app, "tu_2", "Bash", touch("other.txt"));
    assert_eq!(
        app.waiting_calls().len(),
        1,
        "premise: the second waits its turn"
    );

    app.feed_stream(StreamMsg::ToolWithdrawn(question("tu_2")));
    assert!(
        app.waiting_calls().is_empty(),
        "the withdrawal took it from the queue"
    );
    tool_key(&mut app, 'd');
    if app.shown_call().is_some_and(|p| p.tool_use_id() == "tu_2") {
        tool_key(&mut app, 'a');
    }
    assert!(
        exec.ran().is_empty(),
        "Shannon ran nothing: {:?}",
        exec.ran()
    );
    assert!(
        app.shown_call().is_none(),
        "and nothing came back to the pane"
    );
    assert_eq!(provider.answered().len(), 1, "tu_1's deny alone");
}

/// **A queued call with no record is never decided, and never strands the one
/// behind it** — the fail-closed beneath the forget: decided, the lookups
/// that default on a missing record would put it on the pane as a call of
/// Shannon's. Dropped silently, the live call behind it never reached the
/// pane and its turn waited on nobody (full review 10-21, C 3, probed): it is
/// dropped out loud, and the next one shown. No path leaves such an entry
/// today (the forget clears the queue, above), so this one is planted.
#[tokio::test]
async fn a_stale_queued_call_is_dropped_out_loud_and_the_next_shown() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    two_calls_asking(&mut app);
    app.queue_call_for_test(app.current, "tu_stale", "touch stale.txt", true);
    tool_key(&mut app, 'd');
    assert_eq!(
        on_the_pane(&app).as_deref(),
        Some("tu_2"),
        "the live call next"
    );
    assert!(
        app.notices
            .iter()
            .any(|(_, n)| n.contains("dropped `touch stale.txt`") && n.contains("nothing ran")),
        "{:?}",
        app.notices
    );
}

/// **…and `decide_tool_call` refuses one out loud**, should anything else
/// reach it with no record — the guard beneath the queue's door.
#[tokio::test]
async fn a_call_with_no_record_is_never_decided() {
    let mut app = test_app();
    app.decide_tool_call("tu_none", "touch never.txt");
    assert!(app.shown_call().is_none(), "not put to you");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("`touch never.txt` was not put to you")),
        "{:?}",
        app.notice()
    );
}

/// **A call with no record is never run** (`run_tool_call`) — answered from
/// a pane whose record has gone, it would run in Shannon's shell whatever it
/// was. It is said, and the pane goes to the call behind it: `a` that did
/// nothing and said nothing was a dead key over a stranded queue (full
/// review 10-21, C 3). No path takes a record from under its pane today, so
/// it is taken by hand.
#[tokio::test]
async fn a_call_with_no_record_is_never_run() {
    let mut app = test_app();
    let exec = Arc::new(shannon_core::RecordingExecutor::default());
    app.executor = exec.clone();
    two_calls_asking(&mut app);
    app.forget_tool_call("tu_1");
    tool_key(&mut app, 'a');
    assert!(exec.ran().is_empty(), "{:?}", exec.ran());
    assert!(
        app.notices
            .iter()
            .any(|(_, n)| n.contains("was not run — nothing holds that call")),
        "{:?}",
        app.notices
    );
    app.settle();
    assert_eq!(
        on_the_pane(&app).as_deref(),
        Some("tu_2"),
        "the next call shown"
    );
}

/// **A child's question for the call on the pane refuses the one on the
/// pane** (review stop 7, Reviewer E 2, probed). Two questions claimed one
/// call, so neither is Claude Code's beyond doubt — on the unattended branch
/// too, where the child holds and its own question is left open. The one on
/// the pane stayed, and `a` recorded "Claude Code runs" for it.
///
/// **Defence in depth**, planted: since property 4 (10-22) each turn answers
/// only the calls its own output named, so a child's gate cannot forward
/// the foreground's id (review stop 12, T4 1).
#[tokio::test]
async fn a_childs_question_for_the_call_on_the_pane_refuses_the_one_on_the_pane() {
    use shannon_core::hook::Reply;
    let (mut app, provider) = provider_app();
    let child = claude_code_child(&mut app);
    let rm = serde_json::json!({"command": "rm -rf /tmp/shannon-test-absent"});
    provider_asks(&mut app, "tu_8", "Bash", rm.clone());
    assert!(
        app.shown_call().is_some(),
        "premise: the foreground's is on the pane"
    );

    let childs = shannon_core::AskId::for_test("tu_8", 2);
    let asked = StreamMsg::ToolAsk(shannon_core::ToolQuestion {
        id: "tu_8".into(),
        name: "Bash".into(),
        input: rm,
        cwd: here(&app),
        ask: childs,
        answer_by: hook_answer_by(),
    });
    app.dispatch_stream_msg(child, asked);
    app.settle();
    assert_eq!(app.held_jobs(JobKind::Child), 1, "the child holds");
    assert!(app.shown_call().is_none(), "the one on the pane is refused");
    assert_eq!(
        *provider.asked.lock().unwrap(),
        vec![question("tu_8")],
        "the foreground's answered; the held child's left open"
    );
    assert!(
        matches!(&provider.answered()[..], [(id, Reply::Deny { .. })] if id == "tu_8"),
        "{:?}",
        provider.answered()
    );
}

/// **A held child's question stays open when another claims its call**
/// (review stop 7, Reviewer E 3, probed). Holding leaves it unanswered on
/// purpose — that is what keeps Claude Code inside the child stopped — and
/// refusing the collision answered it, so the child carried on while `:graph`
/// said held. The newcomer is refused, as ever.
///
/// **Defence in depth**, planted across generations, which property 4 makes
/// unreachable (review stop 12, T4 1). The reachable shape — a later
/// question on the held child's own gate — is refused at once
/// (`a_claude_code_child_runs_only_what_is_safe_unattended`, Q4).
#[tokio::test]
async fn a_held_childs_question_stays_open_when_another_claims_its_call() {
    let (mut app, provider) = provider_app();
    let child = claude_code_child(&mut app);
    let rm = serde_json::json!({"command": "rm -rf /tmp/shannon-test-absent"});
    let asked = asked_by_hook(&app, "tu_7", "Bash", rm.clone());
    app.dispatch_stream_msg(child, asked);
    app.settle();
    assert_eq!(app.held_jobs(JobKind::Child), 1, "premise: held");
    assert!(provider.answered().is_empty(), "premise: its question open");

    let newcomer = shannon_core::AskId::for_test("tu_7", 2);
    app.conversation_mut().begin_turn("go");
    app.feed_stream(StreamMsg::ToolAsk(shannon_core::ToolQuestion {
        id: "tu_7".into(),
        name: "Bash".into(),
        input: rm,
        cwd: here(&app),
        ask: newcomer.clone(),
        answer_by: hook_answer_by(),
    }));
    assert_eq!(
        *provider.asked.lock().unwrap(),
        vec![newcomer],
        "the newcomer refused; the held child's left open"
    );
    assert_eq!(app.held_jobs(JobKind::Child), 1, "still held");
}

/// **A finished call's result is not rewritten by a question naming it**
/// (review stop 7, Reviewer E 4, probed). A call's record goes when it is
/// answered, while its result still waits in the round; a question naming
/// its id fails the claim, and the refusal went through `finish_tool_call`,
/// which overwrote "denied by the user" with "refused: this tool_use id is
/// already in use" — what the model would have read when the round resumed.
///
/// **Defence in depth**, planted: a generation never carries both Shannon's
/// `ToolUse` and Claude Code's `ToolAsk`, and a Claude Code result lives in
/// its conversation, not in `tool_results` (review stop 12, T4 1).
#[tokio::test]
async fn a_finished_calls_result_is_not_rewritten_by_a_question_naming_it() {
    use shannon_core::hook::Reply;
    let (mut app, provider) = provider_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.conversation_mut().begin_turn("go");
    for id in ["tu_a", "tu_b"] {
        let call = shannon_core::Block::bash_call(id, "rm -rf /tmp/shannon-test-absent");
        app.feed_stream(StreamMsg::ToolUse(call));
    }
    app.feed_stream(StreamMsg::Complete);
    tool_key(&mut app, 'd');
    let stored = |app: &App| match app.tool_results.get("tu_a") {
        Some(shannon_core::Block::ToolResult { content, .. }) => content.clone(),
        other => format!("{other:?}"),
    };
    assert!(
        stored(&app).contains("denied by the user"),
        "premise: {}",
        stored(&app)
    );
    assert_eq!(
        app.shown_call()
            .map(|p| p.tool_use_id().to_string())
            .as_deref(),
        Some("tu_b"),
        "premise: the round still waits, on tu_b"
    );

    let asked = asked_by_hook(&app, "tu_a", "Bash", serde_json::json!({"command": "ls"}));
    app.feed_stream(asked);
    assert!(
        stored(&app).contains("denied by the user"),
        "{}",
        stored(&app)
    );
    assert!(
        matches!(&provider.answered()[..], [(id, Reply::Deny { .. })] if id == "tu_a"),
        "the newcomer is refused to its hook: {:?}",
        provider.answered()
    );
}

/// **A distillation reads the filed Claude Code session, and the next turn
/// never sees it.** It resumes the session filed under exactly its history
/// — forked, so the filed one stays as filed — and the session it ran in is
/// never filed. Review 21's concern (Reviewer A: resuming put the squash
/// prompt into the session the next turn continues) asserted as the
/// property itself: after the distillation, a turn on that history still
/// resumes the ORIGINAL session. It used to start the distillation fresh,
/// from a handoff cut to 2000 characters a message.
#[tokio::test]
async fn a_distillation_reads_the_filed_session_and_the_next_turn_never_sees_it() {
    let dir = TempDir::new().unwrap();
    let (mut app, provider) = provider_app();
    app.claude_program = logged_in_claude(dir.path());
    app.model_command("claude-code/sonnet");
    app.submit("first");
    app.feed_stream(StreamMsg::ProviderSession("sess-1".into()));
    app.feed_stream(StreamMsg::Chunk("one".into()));
    app.feed_stream(StreamMsg::Complete);
    let resumed = |provider: &shannon_core::backend::RecordingProvider| {
        provider
            .requests
            .lock()
            .unwrap()
            .last()
            .and_then(|r| r.resume.clone())
    };

    let mut messages = app.conversation().snapshot();
    messages.push(shannon_core::Message::user("distil this"));
    let req = SendRequest::new(messages.clone()).with_model("claude-code/sonnet");
    let turn = app.conversation().episode().own_turns()[0];
    let target = SquashTarget::Gather {
        episode: app.conversation().episode().id,
        turn,
        index: 0,
        config: app.conversation().episode().config.clone(),
    };
    let summary = Work::Summary(Box::new(SummaryWork {
        target,
        seal: app.conversation().episode().config.clone(),
        sources: vec![turn],
        draft: String::new(),
        review: true,
        instruction: "distil this".into(),
    }));
    let distilling = app.start_generation(app.current, "test".into(), req, summary);
    assert_eq!(
        resumed(&provider).as_deref(),
        Some("sess-1"),
        "a distillation reads the session filed under its history"
    );
    // It runs in a session of its own (the fork), and finishes — and that
    // session must not become the one the history continues in.
    for msg in [
        StreamMsg::ProviderSession("sess-fork".into()),
        StreamMsg::Chunk("the summary".into()),
        StreamMsg::Complete,
    ] {
        app.dispatch_stream_msg(distilling, msg);
    }

    let req = SendRequest::new(messages).with_model("claude-code/sonnet");
    let turn_work = Work::Turn {
        stage: Stage::Streaming,
    };
    app.start_generation(app.current, "test".into(), req, turn_work);
    assert_eq!(
        resumed(&provider).as_deref(),
        Some("sess-1"),
        "a turn resumes the original, not the distillation's session"
    );
}

/// **A Bash call asking to leave Claude Code's sandbox asks every time**
/// (the 11-07 review, finding 17): the same `ls` the rules allow at once
/// asks with `dangerouslyDisableSandbox`, its pane analysed and saying why;
/// a rule added while it is open does not answer it; `A` offers none and
/// says so; `a` still allows it once. A rule allowing the command used to
/// allow it outside the sandbox, unseen.
/// **A deny still denies, outside the sandbox too**: leaving it turns an
/// allow into an ask and never a deny into one — `a` under "you decide"
/// would have run what your own rule forbids (review stop 17, C 1).
#[tokio::test]
async fn a_deny_holds_for_a_call_leaving_claude_codes_sandbox() {
    let (mut app, provider) = provider_app();
    let said = app.add_session_rule("deny", "ls *");
    assert!(said.is_ok(), "premise: the rule is taken: {said:?}");
    provider_asks(
        &mut app,
        "tu_1",
        "Bash",
        serde_json::json!({"command": "ls", "dangerouslyDisableSandbox": true}),
    );
    if let Some(prompt) = app.shown_call() {
        assert_eq!(
            prompt.decision(),
            shannon_core::permission::Decision::Deny,
            "the rule's verdict stands"
        );
    }
    tool_key(&mut app, 'a');
    assert!(
        !provider.answered().contains(&allowed("tu_1")),
        "never allowed: {:?}",
        provider.answered()
    );
}

/// **A deny that arrives while the call is open denies it, and one taken
/// back gives it its question again** — as the same call arriving now would
/// get. An allow still answers nothing (review stop 17, E2 1).
#[tokio::test]
async fn a_rule_changing_under_an_open_call_leaving_the_sandbox_is_read() {
    use shannon_core::permission::Decision;
    let leaving = serde_json::json!({"command": "ls", "dangerouslyDisableSandbox": true});
    let (mut app, provider) = provider_app();
    provider_asks(&mut app, "tu_1", "Bash", leaving.clone());
    assert!(app.shown_call().is_some(), "premise: it asks");
    let said = app.add_session_rule("deny", "ls *");
    assert!(said.is_ok(), "premise: the rule is taken: {said:?}");
    assert!(app.shown_call().is_none(), "denied, not left open");
    tool_key(&mut app, 'a');
    assert!(
        !provider.answered().contains(&allowed("tu_1")),
        "never allowed: {:?}",
        provider.answered()
    );

    let (mut app, provider) = provider_app();
    app.add_session_rule("deny", "ls *").expect("taken");
    provider_asks(&mut app, "tu_2", "Bash", leaving);
    let prompt = app.shown_call().expect("the pane");
    assert_eq!(prompt.decision(), Decision::Deny, "premise: denied");
    app.dispatch_command_for_test("undeny ls *");
    assert_eq!(
        app.shown_call().map(|p| p.decision()),
        Some(Decision::Ask),
        "the question again"
    );
    tool_key(&mut app, 'a');
    assert_eq!(provider.answered(), vec![allowed("tu_2")], "answerable");
}

#[tokio::test]
async fn a_call_leaving_claude_codes_sandbox_asks_whatever_a_rule_allows() {
    let (mut app, provider) = provider_app();
    provider_asks(
        &mut app,
        "tu_0",
        "Bash",
        serde_json::json!({"command": "ls"}),
    );
    assert!(app.shown_call().is_none(), "premise: the rules allow `ls`");
    assert_eq!(provider.answered(), vec![allowed("tu_0")]);

    provider_asks(
        &mut app,
        "tu_1",
        "Bash",
        serde_json::json!({"command": "ls", "dangerouslyDisableSandbox": true}),
    );
    let prompt = app.shown_call().expect("asks");
    assert_eq!(prompt.command(), "ls", "shown as the command it is");
    assert_eq!(
        prompt.decision(),
        shannon_core::permission::Decision::Ask,
        "the pane's own verdict, not the rules' Allow"
    );
    assert!(prompt.proposal().is_err(), "no rule to offer");
    let shown = screen(&mut app, 200);
    for said in ["outside Claude Code's sandbox", "OUTSIDE its sandbox"] {
        assert!(shown.contains(said), "the pane says why ({said}): {shown}");
    }

    let said = app.add_session_rule("allow", "ls *");
    assert!(said.is_ok(), "premise: the rule is taken: {said:?}");
    assert_eq!(
        provider.answered(),
        vec![allowed("tu_0")],
        "a rule allowing the command did not answer it"
    );
    assert!(app.shown_call().is_some(), "the call still asks");

    tool_key(&mut app, 'A');
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("OUTSIDE its sandbox")),
        "{:?}",
        app.notice()
    );
    tool_key(&mut app, 'a');
    assert_eq!(
        provider.answered(),
        vec![allowed("tu_0"), allowed("tu_1")],
        "allowed once, by the human"
    );
}

/// **A tool the rules cannot read asks every time**, whatever a rule says
/// about its line — which is not shell: `:allow WebFetch *` does not answer the
/// open call, and `A` offers no rule. `[claude_code] web = "allow"` is the way
/// past it, and only for the web.
/// **A deny does not answer a call the rules cannot read** — its line is
/// not what runs, so a rule matching it says nothing of the call, either way
/// (review stop 17, beside E2 1).
#[tokio::test]
async fn a_deny_does_not_answer_a_call_the_rules_cannot_read() {
    let (mut app, provider) = provider_app();
    provider_asks(
        &mut app,
        "tu_1",
        "WebFetch",
        serde_json::json!({"url": "https://example.com"}),
    );
    assert!(app.shown_call().is_some(), "premise: it asks");
    let said = app.add_session_rule("deny", "WebFetch *");
    assert!(said.is_ok(), "premise: the rule is taken: {said:?}");
    assert_eq!(
        app.rules_for_call("tu_1")
            .decide_line("WebFetch https://example.com")
            .decision,
        shannon_core::permission::Decision::Deny,
        "premise: the rules deny its line"
    );
    assert!(app.shown_call().is_some(), "it still asks");
    assert!(provider.answered().is_empty(), "{:?}", provider.answered());
}

#[tokio::test]
async fn a_tool_the_rules_cannot_read_asks_every_time() {
    let (mut app, provider) = provider_app();
    provider_asks(
        &mut app,
        "tu_1",
        "WebFetch",
        serde_json::json!({"url": "https://example.com"}),
    );
    let prompt = app.shown_call().expect("asks");
    assert_eq!(prompt.command(), "WebFetch https://example.com");
    assert!(
        prompt.proposal().is_err(),
        "no rule to offer: none would answer it"
    );

    // Rules that ALLOW its line, read as shell — the premise, asserted: since
    // the program models learned `WebFetch <url>` as a fetch, `WebFetch *`
    // alone asks for the host, so this test reached no guard and a mutant
    // disabling it passed (review stop 8, E2 1).
    let said = app.add_session_rule("allow", ":fetch example.com");
    assert!(said.is_ok(), "premise: the rule is taken: {said:?}");
    let said = app.add_session_rule("allow", "WebFetch *");
    assert!(said.is_ok(), "premise: the rule is taken: {said:?}");
    assert_eq!(
        app.rules_for_call("tu_1")
            .decide_line("WebFetch https://example.com")
            .decision,
        shannon_core::permission::Decision::Allow,
        "premise: the rules allow its line"
    );
    assert!(
        provider.answered().is_empty(),
        "a rule matching its line did not answer it: {:?}",
        provider.answered()
    );
    assert!(app.shown_call().is_some(), "the call still asks");

    tool_key(&mut app, 'A');
    assert!(
        app.notice().is_some_and(|n| n.contains("cannot read")),
        "{:?}",
        app.notice()
    );
    tool_key(&mut app, 'a');
    assert_eq!(provider.answered(), vec![allowed("tu_1")]);
    provider_asks(
        &mut app,
        "tu_2",
        "WebFetch",
        serde_json::json!({"url": "https://example.com"}),
    );
    assert!(app.shown_call().is_some(), "the next one asks again");
    // An edit keeps it a call the rules cannot read: analysed as shell, the
    // rule above would allow it.
    let rules = app.rules.clone();
    let prompt = app.shown_call_mut().expect("open");
    prompt.re_gate("WebFetch https://other.example", &rules);
    assert_eq!(prompt.decision(), shannon_core::permission::Decision::Ask);
    assert!(prompt.proposal().is_err());

    let mut config = Config::default();
    config.claude_code.web = shannon_core::config::WebPolicy::Allow;
    let provider = shannon_core::backend::RecordingProvider::new();
    let mut app = with_provider(test_app_with(config), &provider);
    provider_asks(
        &mut app,
        "tu_3",
        "WebSearch",
        serde_json::json!({"query": "rust"}),
    );
    assert!(app.shown_call().is_none(), "allowed by config");
    assert_eq!(provider.answered(), vec![allowed("tu_3")]);
    provider_asks(
        &mut app,
        "tu_4",
        "Task",
        serde_json::json!({"description": "explore"}),
    );
    assert!(
        app.shown_call().is_some(),
        "the web knob is the web's alone"
    );
}

/// **An edited line goes back to Claude Code only for `Bash`**, as the input
/// it runs (a `Write` or an `Edit` takes its edit as text:
/// `e_on_a_claude_code_write_edits_what_it_writes`). Any other tool's line is
/// Shannon's reading of the call and does not map back: `e` says so before
/// `$EDITOR` opens, and an edit that reached one anyway is refused — never
/// dropped, which would run what nobody approved.
#[tokio::test]
async fn an_edited_line_reaches_claude_code_only_for_bash() {
    use shannon_core::hook::Reply;
    let (mut app, provider) = provider_app();
    let rules = app.rules.clone();
    provider_asks(
        &mut app,
        "tu_1",
        "Bash",
        serde_json::json!({"command": "rm -rf /tmp/shannon-test-absent", "description": "d"}),
    );
    assert!(
        matches!(tool_key(&mut app, 'e'), ToolKeyEffect::Edit(_)),
        "Bash takes an edit"
    );
    app.shown_call_mut()
        .expect("open")
        .re_gate("ls /tmp", &rules);
    tool_key(&mut app, 'a');
    assert_eq!(
        provider.answered(),
        vec![(
            "tu_1".to_string(),
            Reply::Allow {
                updated_input: Some(serde_json::json!({"command": "ls /tmp", "description": "d"}))
            }
        )]
    );

    provider_asks(
        &mut app,
        "tu_2",
        "Read",
        serde_json::json!({"file_path": ".env"}),
    );
    assert!(
        app.shown_call().is_some(),
        "premise: a credential path puts a pane up (a deny, by default)"
    );
    assert!(
        matches!(tool_key(&mut app, 'e'), ToolKeyEffect::None),
        "no $EDITOR for an edit that cannot apply"
    );
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("Bash, Write and Edit, not Read")),
        "{:?}",
        app.notice()
    );
    assert!(app.shown_call().is_some(), "and the call is still open");

    app.shown_call_mut()
        .expect("open")
        .re_gate("cat README.md", &rules);
    tool_key(&mut app, 'a');
    assert!(
        matches!(&provider.answered()[1], (id, Reply::Deny { .. }) if id == "tu_2"),
        "{:?}",
        provider.answered()
    );
}

/// **A Claude Code line is shown scrubbed on every record of it** — allowed,
/// denied, withdrawn. Its model read tool output unredacted, so a line it
/// writes can carry a secret it read (review 21, Reviewer B).
///
/// By its shape (a key), and by where it sits (`--password x`, the value
/// under the text rule's floor): a command is read where the shell reads it
/// (review stop 17, E1 M1).
#[tokio::test]
async fn claude_codes_lines_are_shown_scrubbed() {
    let key = "Zk3pQ9vX2mL7tR4wB8nY"; // invented, never a real key: gitleaks:allow
    for (secret, value) in [
        (format!("OPENAI_API_KEY={key}"), key),
        ("--password hunter2".to_string(), "hunter2"),
    ] {
        let (mut app, provider) = provider_app();
        let asking = |n: u8| {
            serde_json::json!({
                "command": format!("rm -r /tmp/shannon-test-absent-{n} {secret}")
            })
        };
        provider_asks(&mut app, "tu_1", "Bash", asking(1));
        assert!(app.shown_call().is_some(), "premise: it asks");
        tool_key(&mut app, 'a');
        provider_asks(&mut app, "tu_2", "Bash", asking(2));
        tool_key(&mut app, 'd');
        provider_asks(&mut app, "tu_3", "Bash", asking(3));
        app.feed_stream(StreamMsg::ToolWithdrawn(question("tu_3")));
        assert_eq!(provider.answered().len(), 2, "{:?}", provider.answered());

        let shown = app.output().lines().join("\n");
        for verb in ["Claude Code runs", "DENIED", "WITHDRAWN"] {
            assert!(shown.contains(verb), "{verb} in {shown}");
        }
        assert!(!shown.contains(value), "{shown}");
        assert!(!app.notice().unwrap_or_default().contains(value));
    }
}

/// **A held child's line is said without a credential after its flag** —
/// in the log line that holds it, and in the refusal of its next question,
/// which names what it is held on (review stop 17, E1 M1).
#[tokio::test]
async fn a_held_childs_line_is_said_without_its_flags_credential() {
    let (mut app, _provider) = provider_app();
    let child = claude_code_child(&mut app);
    let held = serde_json::json!({"command": "frobnicate --password hunter2 now"});
    let asked = asked_by_hook(&app, "tu_7", "Bash", held);
    let logs = captured_logs(|| {
        app.dispatch_stream_msg(child, asked);
        app.settle();
    });
    assert_eq!(app.held_jobs(JobKind::Child), 1, "premise: held");
    assert!(logs.contains("frobnicate"), "premise: logged: {logs}");
    assert!(!logs.contains("hunter2"), "{logs}");
    let next = asked_by_hook(&app, "tu_8", "Bash", serde_json::json!({"command": "ls"}));
    app.dispatch_stream_msg(child, next);
    let said = (app.notices.iter())
        .map(|(_, n)| n.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        said.contains("held on `frobnicate"),
        "premise: refused, said: {said}"
    );
    assert!(!said.contains("hunter2"), "{said}");
}

/// **An edited allow's input goes with its turn** — kept for Claude Code's
/// record of the call, and dropped when the turn ends without one: a turn
/// stopped before the record arrived kept it for the rest of the run, for a
/// `Write` the whole file's text (full review 10-21, B 7).
#[tokio::test]
async fn an_edited_allows_input_is_dropped_when_its_turn_ends_without_a_record() {
    let (mut app, _provider) = provider_app();
    provider_asks(
        &mut app,
        "tu_1",
        "Bash",
        serde_json::json!({"command": "ls ~/src", "description": "d"}),
    );
    let seed = app.shown_call().expect("asks").command().to_string();
    app.apply_edit_for_test(&seed, "ls ~/src/rust");
    tool_key(&mut app, 'a');
    assert!(!app.provider_edits.is_empty(), "premise: the edit is kept");

    app.set_focus(PaneKind::Prompt); // the decision pane stops nothing (A 13)
    app.interrupt();
    assert!(app.turns().next().is_none(), "premise: the turn is over");
    assert!(app.provider_edits.is_empty(), "its edit went with it");
}

/// **A call Claude Code withdraws takes the edit `n` set aside with it** —
/// the record and the model's request kept for it leave at one door
/// (`forget_tool_call`), as an answered call's do; a call waiting in the
/// queue was only forgotten, and the request outlived it.
#[tokio::test]
async fn a_withdrawn_call_takes_the_edit_n_set_aside() {
    let (mut app, _) = provider_app();
    let asking =
        |n: u8| serde_json::json!({ "command": format!("rm -r /tmp/shannon-test-absent-{n}") });
    provider_asks(&mut app, "tu_1", "Bash", asking(1));
    provider_asks(&mut app, "tu_2", "Bash", asking(2));
    let rules = app.rules.clone();
    (app.shown_call_mut().expect("asks")).re_gate("rm -r /tmp/shannon-test-absent-9", &rules);
    tool_key(&mut app, 'n');
    assert!(app.is_edited("tu_1"), "premise: set aside, edited");

    // The edit is on the record since 10-21 (`ToolCall::edited_from`, D 7),
    // so it goes with the record by construction; what this pins is that
    // the withdrawal forgets the record at all.
    app.feed_stream(StreamMsg::ToolWithdrawn(question("tu_1")));
    assert!(app.tool_call("tu_1").is_none(), "gone, with its edit");
}

/// **What Claude Code ran is on the record** — shown, kept as a tool round in
/// the transcript, and scrubbed on the way into both: its model saw the output
/// as it was, but Shannon's screen and store are Shannon's sinks.
#[tokio::test]
async fn what_claude_code_ran_is_recorded_and_scrubbed() {
    use shannon_core::Block;
    let (mut app, _) = provider_app();
    let value = "Zk3pQ9vX2mL7tR4wB8nY";
    let secret = format!("OPENAI_API_KEY={value}");
    assert!(
        !app.scrub(&secret).contains(value),
        "premise: the redactor catches it"
    );
    let input = serde_json::json!({"command": format!("echo {secret}")});
    claude_code_ran(&mut app, "tu_1", "Bash", input, &format!("{secret}\n"));
    app.feed_stream(StreamMsg::Chunk("done".into()));
    claude_code_turn_ends(&mut app);

    assert!(
        !app.conversation().turn_in_progress(),
        "the turn sealed: a record leaves nothing pending"
    );
    let shown = app.output().lines().join("\n");
    assert!(shown.contains("Claude Code ran"), "{shown}");
    assert!(!shown.contains(value), "{shown}");
    let blocks: Vec<&Block> = app
        .conversation()
        .messages()
        .iter()
        .flat_map(|m| &m.blocks)
        .collect();
    assert!(
        matches!(
            blocks.as_slice(),
            [Block::ToolUse { name, .. }, Block::ToolResult { tool_use_id, .. }]
                if name == "Bash" && tool_use_id == "tu_1"
        ),
        "{blocks:?}"
    );
    let stored = serde_json::to_string(app.conversation().messages()).expect("json");
    assert!(!stored.contains(value), "{stored}");
}

/// **What Claude Code ran with a credential after its flag is recorded
/// without it** — on the screen and in what is kept (review stop 17, E1 M1,
/// measured with this line): the text rule has no position, and a call's
/// `"command"` is read where the shell reads it.
#[tokio::test]
async fn what_claude_code_ran_with_a_flags_credential_is_recorded_without_it() {
    let (mut app, _) = provider_app();
    let value = "correcthorse9staple";
    let line = format!("mysql --password {value} -h db.example.org");
    assert!(
        app.scrub(&line).contains(value),
        "premise: free text has no position"
    );
    let input = serde_json::json!({ "command": line });
    claude_code_ran(&mut app, "tu_1", "Bash", input, "ok\n");
    app.feed_stream(StreamMsg::Chunk("done".into()));
    claude_code_turn_ends(&mut app);
    let shown = app.output().lines().join("\n");
    assert!(
        shown.contains("db.example.org"),
        "premise: recorded: {shown}"
    );
    assert!(!shown.contains(value), "{shown}");
    let stored = serde_json::to_string(app.conversation().messages()).expect("json");
    assert!(
        stored.contains("--password [redacted: named secret]"),
        "{stored}"
    );
    assert!(!stored.contains(value), "{stored}");
}

/// **In a session Claude Code answers, a held secret shows as its value** —
/// in the reply and in a command's output, the tool record's door
/// (`record_tool_activity`) asking what answers as the stream does (Steven,
/// 10-19, image 10: `hunter2 ({{secret.f97e}})` in a reply; "No need to show
/// the hash").
#[tokio::test]
async fn a_claude_code_session_shows_a_held_secret_as_its_value() {
    const VALUE: &str = "s3cret-output-notreal";
    let (mut app, _) = provider_app();
    let handle = app.mark_secret(VALUE, true).unwrap();
    app.model_command("claude-code/sonnet");
    app.submit("go");
    app.feed_stream(StreamMsg::Chunk(format!("using {handle} now")));
    let input = serde_json::json!({"command": "printenv TOKEN"});
    claude_code_ran(&mut app, "tu_1", "Bash", input, &format!("token={handle}"));
    claude_code_turn_ends(&mut app);
    let shown = app.output().lines().join("\n");
    for said in [format!("using {VALUE} now"), format!("token={VALUE}")] {
        assert!(shown.contains(&said), "{said:?} in {shown}");
    }
    assert!(!shown.contains(&handle), "no handle beside it: {shown}");
}

/// **A Claude Code call is recorded as the tool it is, and reopens as it
/// ran.** An Edit is its file and its diff — not the `cat > path` the gate
/// reads it as — drawn in the record's colours: what ran in `TOOL`, the
/// markers in the verdict colours, what came of it in the record's colour,
/// dim. And the sealed
/// turn, replayed, draws the live lines (Steven, 10-05: tool calls "used to
/// appear in blue … this should be prettier and formatted") — exactly: the
/// one the allow wrote, `Claude Code runs:`, is the place the record that
/// closes the call takes (review stop 15, F H2). Until then it stood above
/// the record, and "exactly the live lines" held only while the record here
/// came with no question before it (review stop 12, T4 3).
#[tokio::test]
async fn a_claude_code_call_is_recorded_as_its_tool_and_reopens_as_it_ran() {
    use crate::palette;
    let (mut app, _) = provider_app();
    // A Claude Code turn, as a record only comes from one — its backend is
    // what replay names the call by (review, Reviewer C: this fed a record
    // into a turn no Claude Code ran).
    app.model_command("claude-code/sonnet");
    app.submit("go");
    app.feed_stream(StreamMsg::Chunk("Editing.".into()));
    let input = serde_json::json!({"file_path": "src/a.rs",
                                   "old_string": "let x = 1;", "new_string": "let x = 2;"});
    claude_code_ran(
        &mut app,
        "tu_1",
        "Edit",
        input,
        "The file src/a.rs has been updated.",
    );
    app.feed_stream(StreamMsg::Chunk("done".into()));
    claude_code_turn_ends(&mut app);

    let live = app.output().lines();
    for line in [
        "⚙ Claude Code ran: Edit src/a.rs",
        "  - let x = 1;",
        "  + let x = 2;",
        "  The file src/a.rs has been updated.",
    ] {
        assert!(live.iter().any(|l| l == line), "{line:?} in {live:#?}");
    }
    assert!(!live.iter().any(|l| l.contains("cat >")), "{live:#?}");

    // The switch raised setup over the transcript: the test's sandbox has no
    // `claude` (`NO_CLAUDE`). Closed, as the model it chose stays chosen.
    app.setup = None;
    // A record ships shut (N5a); its colours are what this asks of it open.
    app.open_every_fold_for_test();
    let buf = frame_of(&mut app, 100, 30);
    let fg = |text: &str| {
        let at = cell_of(&buf, text);
        let at = at.unwrap_or_else(|| panic!("{text:?} is drawn:\n{}", rendered_text(&buf)));
        buf[at].style().fg
    };
    assert_eq!(fg("⚙ Claude Code ran"), Some(palette::TOOL), "what ran");
    assert_eq!(
        fg("- let x = 1;"),
        Some(palette::DANGER),
        "a removed line's marker"
    );
    assert_eq!(
        fg("+ let x = 2;"),
        Some(palette::SAFE),
        "an added line's marker"
    );
    // What came of it: the record's colour, dim (`tool_record::OUTPUT`) —
    // what ran is the same colour, and not dim.
    let dim = |buf: &ratatui::buffer::Buffer, text: &str| {
        let at = cell_of(buf, text).unwrap_or_else(|| panic!("{text:?} is drawn"));
        (buf[at].style().add_modifier).contains(ratatui::style::Modifier::DIM)
    };
    assert_eq!(
        fg("The file src/a.rs"),
        Some(palette::TOOL),
        "what came of it"
    );
    assert!(dim(&buf, "The file src/a.rs") && !dim(&buf, "⚙ Claude Code ran"));

    // And in a FOCUSED transcript, which draws through its own path
    // (`base_style`): its mutant to flat `TOOL` passed everything (review of
    // 69f2e5b, Reviewer C) — and now draws output undimmed.
    app.set_focus(PaneKind::Output);
    let focused = frame_of(&mut app, 100, 30);
    let fg = |text: &str| {
        let at = cell_of(&focused, text);
        focused[at.unwrap_or_else(|| panic!("{text:?} is drawn, focused"))]
            .style()
            .fg
    };
    assert_eq!(
        fg("- let x = 1;"),
        Some(palette::DANGER),
        "focused: removed"
    );
    assert_eq!(fg("+ let x = 2;"), Some(palette::SAFE), "focused: added");
    assert_eq!(
        fg("The file src/a.rs"),
        Some(palette::TOOL),
        "focused: output"
    );
    assert!(dim(&focused, "The file src/a.rs"), "focused: output, dim");

    // Live, the allow said the call was starting, and the record that
    // closed it took that line's place (review stop 15, F H2): a reopened
    // turn draws what the live one does, line for line.
    assert!(
        !live.iter().any(|l| l.starts_with("⚙ Claude Code runs")),
        "the line that said it was starting, replaced: {live:#?}"
    );
    let (reopened, _) = App::render_session(
        &shannon_core::Aliases::default(),
        app.record_shape(),
        &app.conversation().own_turns(),
    );
    assert_eq!(
        reopened.lines(),
        live,
        "a reopened turn draws what the live one did"
    );
}

/// **A gated Claude Code call is named as its tool on every record**, and
/// an edited one is recorded as it RAN: the path production takes — asked,
/// allowed, then the stream's record (review, Reviewers A and C: the "runs"
/// record said `cat > path`; the closing one drew the model's diff for a
/// Write the user rewrote, and stored it as though it ran as asked).
#[tokio::test]
async fn a_gated_claude_code_call_is_named_and_recorded_as_it_ran() {
    use shannon_core::Block;
    let path = "/tmp/shannon-test-absent/a.rs";
    let edit = serde_json::json!({"file_path": path, "old_string": "a()", "new_string": "b()"});
    let record = |app: &mut App, content: &str| {
        app.feed_stream(StreamMsg::ToolRecord {
            call: Block::ToolUse {
                id: "tu_1".into(),
                name: "Edit".into(),
                input: edit.clone(),
            },
            result: Block::ToolResult {
                tool_use_id: "tu_1".into(),
                content: content.into(),
                is_error: false,
            },
        });
    };

    let (mut app, _) = provider_app();
    provider_asks(&mut app, "tu_1", "Edit", edit.clone());
    tool_key(&mut app, 'a');
    let running = app.output().lines().join("\n");
    assert!(
        running.contains(&format!("⚙ Claude Code runs: Edit {path}")),
        "while it runs: {running}"
    );
    record(&mut app, "updated");
    let shown = app.output().lines().join("\n");
    assert!(!shown.contains("cat >"), "{shown}");
    assert!(
        shown.contains(&format!("⚙ Claude Code ran: Edit {path}"))
            && !shown.contains("Claude Code runs"),
        "its record in that line's place: {shown}"
    );
    assert!(shown.contains("+ b()"), "{shown}");

    let (mut app, _) = provider_app();
    provider_asks(&mut app, "tu_1", "Edit", edit.clone());
    let (seed, _) = app.edit_seed_for_test().expect("a call is open");
    app.apply_edit_for_test(&seed, "c()");
    tool_key(&mut app, 'a');
    record(&mut app, "updated");
    let shown = app.output().lines().join("\n");
    assert!(!shown.contains("cat >"), "{shown}");
    assert!(
        shown.contains(&format!(
            "⚙ Claude Code ran (edited, replaces what the model asked): Edit {path}"
        )),
        "{shown}"
    );
    assert!(
        shown.contains("+ c()") && !shown.contains("+ b()"),
        "as it ran: {shown}"
    );
    let stored: Vec<&Block> = app
        .conversation()
        .messages()
        .iter()
        .flat_map(|m| &m.blocks)
        .collect();
    assert!(
        stored.iter().any(|b| matches!(b,
            Block::ToolResult { content, .. }
                if content.contains("you asked for: b()") && content.contains("what actually ran: c()"))),
        "{stored:?}"
    );
}

/// **A Claude Code turn continues the session that saw exactly this
/// history**: a finished turn files its session under the history it leaves,
/// and the next turn on that history resumes it. A failed turn files nothing —
/// whatever Claude Code kept of it is not what the transcript holds.
#[tokio::test]
async fn a_claude_code_session_is_resumed_on_the_history_it_saw() {
    let (mut app, provider) = provider_app();
    app.model_command("claude-code/sonnet");
    let resumes = |provider: &shannon_core::backend::RecordingProvider| -> Vec<Option<String>> {
        provider
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.resume.clone())
            .collect()
    };

    app.submit("first");
    app.feed_stream(StreamMsg::ProviderSession("sess-1".into()));
    app.feed_stream(StreamMsg::Chunk("one".into()));
    app.feed_stream(StreamMsg::Complete);
    app.submit("second");
    assert_eq!(resumes(&provider), vec![None, Some("sess-1".to_string())]);

    app.feed_stream(StreamMsg::ProviderSession("sess-2".into()));
    app.feed_stream(StreamMsg::Chunk("partial".into()));
    app.feed_stream(StreamMsg::Error(BackendError::ClaudeCode("broke".into())));
    app.submit("third");
    assert_eq!(
        resumes(&provider).last(),
        Some(&None),
        "a failed turn's session is not resumed"
    );
    assert!(
        app.claude_sessions.started("sess-2"),
        "but it is Shannon's, not listed as Claude Code's: {:?}",
        app.claude_sessions
    );
}

/// **A credential path on a call Claude Code runs is refused by default**
/// (`[claude_code] path_policy`, Steven 10-04): its model sees the output
/// unredacted, so the gate is the only layer. The same read in Shannon's own
/// turn still only asks. The pane says so and names the knob, an `:allow`
/// cannot loosen it — every site judges under `rules_for_call` — and the knob
/// is the user's.
#[tokio::test]
async fn a_credential_path_from_claude_code_is_denied_by_default() {
    use shannon_core::permission::Decision;
    let (mut app, provider) = provider_app();
    provider_asks(
        &mut app,
        "tu_1",
        "Read",
        serde_json::json!({"file_path": ".env"}),
    );
    let prompt = app.shown_call().expect("a pane");
    assert_eq!(prompt.decision(), Decision::Deny);
    let body: String = prompt.text().lines().iter().map(|l| l.text()).collect();
    assert!(body.contains("sees the output as it is"), "{body}");
    assert!(body.contains("[claude_code] path_policy"), "{body}");

    let said = app.add_session_rule("allow", "cat *");
    assert!(said.is_ok(), "premise: the rule is taken: {said:?}");
    assert_eq!(
        app.shown_call().map(|p| p.decision()),
        Some(Decision::Deny),
        "an allow does not loosen it: {said:?}"
    );
    assert!(provider.answered().is_empty());

    tool_key(&mut app, 'a');
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("[claude_code] path_policy") && !notice.contains("`e` edits"),
        "the lever is the knob — this call's `e` is struck through: {notice}"
    );

    // An edit is judged under the same knob: a Bash call edited INTO the read.
    let (mut app, _) = provider_app();
    let rm = "rm -rf /tmp/shannon-test-absent";
    provider_asks(&mut app, "tu_9", "Bash", serde_json::json!({"command": rm}));
    app.apply_edit_for_test(rm, "cat .env");
    assert_eq!(
        app.shown_call().map(|p| p.decision()),
        Some(Decision::Deny),
        "an edit re-gates under [claude_code], not [secrets]"
    );

    let mut own = test_app();
    model_call(&mut own, "tu_2", "cat .env");
    assert_eq!(
        own.shown_call().map(|p| p.decision()),
        Some(Decision::Ask),
        "Shannon's own turn redacts, and asks"
    );

    let mut config = Config::default();
    config.claude_code.path_policy = shannon_core::PathPolicy::Ask;
    let mut app = with_provider(test_app_with(config), &provider);
    provider_asks(
        &mut app,
        "tu_3",
        "Read",
        serde_json::json!({"file_path": ".env"}),
    );
    assert_eq!(
        app.shown_call().map(|p| p.decision()),
        Some(Decision::Ask),
        "the knob is the user's"
    );
    // And `A` offers the grant naming the file: a grant at least as specific
    // as the guard lifts it (Q9) — the user reads and confirms it.
    tool_key(&mut app, 'A');
    assert_eq!(app.cmdline.line(), Some("allow :read .env"));

    // "allow" lets it run unasked — the decision on arrival reads the knob too.
    let mut config = Config::default();
    config.claude_code.path_policy = shannon_core::PathPolicy::Allow;
    let provider = shannon_core::backend::RecordingProvider::new();
    let mut app = with_provider(test_app_with(config), &provider);
    provider_asks(
        &mut app,
        "tu_4",
        "Read",
        serde_json::json!({"file_path": ".env"}),
    );
    assert!(app.shown_call().is_none());
    assert_eq!(provider.answered(), vec![allowed("tu_4")]);
}

/// **Claude Code answers without an API key** — it runs under its own login,
/// so a `claude-code/…` model is never reported as the stub. Every surface that
/// names the answering model reads `answering`, which reads `route`, which
/// `backend_for` sends by.
#[tokio::test]
async fn claude_code_answers_without_an_api_key() {
    let (mut app, _) = provider_app();
    assert!(app.only_stub(), "premise: no API key");
    assert_eq!(
        app.answering_model("claude-code/sonnet"),
        "claude-code/sonnet"
    );
    assert_eq!(
        app.answering_model("claude-sonnet-4-5"),
        shannon_core::backend::models::STUB
    );
    app.model_command("claude-code/sonnet");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("Claude Code answers")),
        "{:?}",
        app.notice()
    );
    // And the double is in place for every test app, so none starts `claude`
    // — neither for a turn nor for the login check. (Pinned here rather than
    // mutation-checked: deleting it would run the real `claude` in every test
    // that switches to a Claude Code model, which is what it exists to stop.)
    assert_eq!(test_app().claude_code.model(), "claude-code/recording");
    assert_eq!(
        test_app().claude_program,
        std::path::PathBuf::from(NO_CLAUDE)
    );
}

/// **A `:child` on Claude Code decides at once**, as one on the API does: what
/// is safe unattended is allowed; anything else holds the child, its question
/// left open, so Claude Code waits — stopped. A question after that, its own
/// (a parallel call, a subagent's), is refused at once, said: nothing could
/// answer it, and left open it waited out the hook's 59 minutes (Steven,
/// 10-27, Q4: "restricted children that refuse calls").
#[tokio::test]
async fn a_claude_code_child_runs_only_what_is_safe_unattended() {
    let (mut app, provider) = provider_app();
    app.conversation_mut().begin_turn("hello");
    app.conversation_mut().push_chunk("hi");
    app.conversation_mut().commit_turn();
    app.spawn_child_job("go and look", None).expect("spawns");
    let job = app.generation_ids_for_test()[0];
    let ask = |app: &mut App, job, id: &str, tool: &str, input| {
        let asked = asked_by_hook(app, id, tool, input);
        app.dispatch_stream_msg(job, asked);
    };

    let dir = TempDir::new().unwrap();
    let read = serde_json::json!({ "file_path": plain_file(&mut app, &dir) });
    ask(&mut app, job, "tu_1", "Read", read.clone());
    assert_eq!(provider.answered(), vec![allowed("tu_1")]);
    // A tool that touches nothing (`Judge::Inert`) is allowed unattended too.
    let todos = serde_json::json!({ "todos": [] });
    ask(&mut app, job, "tu_1b", "TodoWrite", todos);
    assert_eq!(provider.answered(), vec![allowed("tu_1"), allowed("tu_1b")]);

    let rm = serde_json::json!({"command": "rm -rf /tmp/shannon-test-absent"});
    ask(&mut app, job, "tu_2", "Bash", rm);
    assert!(
        app.shown_call().is_none(),
        "nobody is watching, so nothing asks"
    );
    assert_eq!(app.held_jobs(JobKind::Child), 1, "held");
    assert_eq!(provider.answered().len(), 2, "its question left open");

    app.notices.clear();
    ask(&mut app, job, "tu_3", "Read", read);
    assert!(
        matches!(&provider.answered()[..], [_, _, (id, shannon_core::hook::Reply::Deny { reason })]
            if id == "tu_3" && reason.contains("held")),
        "refused at once, to its hook: {:?}",
        provider.answered()
    );
    assert_eq!(app.held_jobs(JobKind::Child), 1, "still held, on tu_2");
    assert!(
        app.notices
            .iter()
            .any(|(_, n)| n.contains("refused") && n.contains("held on `rm -rf")),
        "said, naming what holds it: {:?}",
        app.notices
    );

    // Its bar reads the knob the call is judged by: credential paths allowed
    // for Claude Code are allowed for its children, though `[secrets]` asks.
    let mut config = Config::default();
    config.claude_code.path_policy = shannon_core::PathPolicy::Allow;
    let provider = shannon_core::backend::RecordingProvider::new();
    let mut app = with_provider(test_app_with(config), &provider);
    app.conversation_mut().begin_turn("hello");
    app.conversation_mut().push_chunk("hi");
    app.conversation_mut().commit_turn();
    app.spawn_child_job("go and look", None).expect("spawns");
    let job = app.generation_ids_for_test()[0];
    let env = serde_json::json!({"file_path": ".env"});
    ask(&mut app, job, "tu_4", "Read", env);
    assert_eq!(provider.answered(), vec![allowed("tu_4")]);

    // A tool the rules cannot read always asks, so a child holds on it rather
    // than running it (review 21, Reviewer B: allowing it passed every test).
    let mcp = serde_json::json!({"query": "anything"});
    ask(&mut app, job, "tu_5", "mcp__search__run", mcp);
    assert_eq!(provider.answered(), vec![allowed("tu_4")], "not run");
    assert_eq!(app.held_jobs(JobKind::Child), 1, "held");

    // A Bash call leaving Claude Code's sandbox (finding 17): the same `ls`
    // a child runs unasked holds it, asked with the request.
    let provider = shannon_core::backend::RecordingProvider::new();
    let mut app = with_provider(test_app(), &provider);
    app.conversation_mut().begin_turn("hello");
    app.conversation_mut().push_chunk("hi");
    app.conversation_mut().commit_turn();
    app.spawn_child_job("go and look", None).expect("spawns");
    let job = app.generation_ids_for_test()[0];
    ask(
        &mut app,
        job,
        "tu_6",
        "Bash",
        serde_json::json!({"command": "ls"}),
    );
    assert_eq!(
        provider.answered(),
        vec![allowed("tu_6")],
        "premise: `ls` runs unattended"
    );
    let outside = serde_json::json!({"command": "ls", "dangerouslyDisableSandbox": true});
    ask(&mut app, job, "tu_7", "Bash", outside);
    assert_eq!(
        provider.answered(),
        vec![allowed("tu_6")],
        "not run outside the sandbox"
    );
    assert_eq!(app.held_jobs(JobKind::Child), 1, "held");
}

/// **A call Claude Code would run anywhere but where Shannon started is
/// refused** — the gate reads a line from the start, and after `cd src`,
/// `cat prod_config.txt` is a different file (review 21, Reviewer B). So is
/// one whose directory is not said; the same directory spelled through a
/// link is the same.
#[tokio::test]
async fn a_claude_code_call_from_another_directory_is_refused() {
    use shannon_core::hook::Reply;
    let (mut app, provider) = provider_app();
    app.conversation_mut().begin_turn("go");
    let elsewhere = TempDir::new().unwrap();
    let asked_in = |id: &str, cwd: Option<String>| {
        StreamMsg::ToolAsk(shannon_core::ToolQuestion {
            id: id.into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "cat prod_config.txt"}),
            cwd,
            ask: question(id),
            answer_by: hook_answer_by(),
        })
    };
    let refused = |provider: &shannon_core::backend::RecordingProvider, want: &str| {
        provider
            .answered()
            .iter()
            .any(|(id, reply)| id == want && matches!(reply, Reply::Deny { .. }))
    };
    app.feed_stream(asked_in(
        "tu_1",
        Some(elsewhere.path().display().to_string()),
    ));
    assert!(refused(&provider, "tu_1"), "{:?}", provider.answered());
    assert!(app.shown_call().is_none(), "nothing left to decide");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("not where Shannon started"), "{said}");

    app.feed_stream(asked_in("tu_2", None));
    assert!(refused(&provider, "tu_2"), "a call that does not say");

    let start = here(&app);
    app.feed_stream(asked_in("tu_3", start));
    assert!(
        !refused(&provider, "tu_3"),
        "where Shannon started, it is judged"
    );
}

/// **A Claude Code Read outside where Shannon started asks**, and says why
/// (review of 716f4f9, C-M5: with `outside_places` disabled no app test went
/// red — the `plain_file` tests passed by moving the start, and nothing
/// read past it).
#[tokio::test]
async fn a_claude_code_read_outside_the_start_asks() {
    let (mut app, provider) = provider_app();
    let (inside, outside) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let file = plain_file(&mut app, &inside);
    provider_asks(
        &mut app,
        "tu_1",
        "Read",
        serde_json::json!({ "file_path": file }),
    );
    assert_eq!(
        provider.answered(),
        vec![allowed("tu_1")],
        "premise: inside runs"
    );

    let other = outside.path().join("notes.txt");
    std::fs::write(&other, "hello\n").unwrap();
    let read = serde_json::json!({ "file_path": other.display().to_string() });
    provider_asks(&mut app, "tu_2", "Read", read);
    assert_eq!(
        provider.answered(),
        vec![allowed("tu_1")],
        "not answered yet"
    );
    let prompt = app.shown_call().expect("a read outside waits");
    let report: String = prompt
        .text()
        .lines()
        .iter()
        .map(|line| line.text())
        .collect();
    assert!(report.contains("outside where Shannon started"), "{report}");
}

/// `runs_elsewhere` compares directories, not spellings: a link to the start
/// is the start.
#[test]
fn runs_elsewhere_compares_directories() {
    let start = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let link = other.path().join("link");
    std::os::unix::fs::symlink(start.path(), &link).unwrap();
    let spelled = |p: &std::path::Path| Some(p.display().to_string());
    let at = Some(start.path());
    assert_eq!(
        crate::app::runs_elsewhere(spelled(start.path()).as_deref(), at),
        None
    );
    assert_eq!(
        crate::app::runs_elsewhere(spelled(&link).as_deref(), at),
        None
    );
    assert!(crate::app::runs_elsewhere(spelled(other.path()).as_deref(), at).is_some());
    assert!(crate::app::runs_elsewhere(None, at).is_some());
    assert!(crate::app::runs_elsewhere(spelled(start.path()).as_deref(), None).is_some());
}

/// **Work offered no tools refuses Claude Code's question** — answered, not
/// left open, or the distillation would wait out the hook's deadline.
///
/// Asked the way the hook asks — where it runs, a read of a plain file
/// inside the start directory, which the rules would otherwise ALLOW — so the
/// refusal is this guard's and no other's. It was asked with no `cwd`, which
/// the "where does it run" guard refuses first, and deleting this guard left
/// the suite green (review of 34033e8, Reviewer B) — while summaries now
/// resume the whole Claude Code session, tools and all.
#[tokio::test]
async fn a_distillation_refuses_claude_codes_tools() {
    use shannon_core::hook::Reply;
    let dir = TempDir::new().unwrap();
    let provider = shannon_core::backend::RecordingProvider::new();
    let mut app = with_provider(app_with_n_turns(dir.path(), 2), &provider);
    let file = plain_file(&mut app, &dir);
    let t = app.conversation().episode().own_turns()[0];
    let target = SquashTarget::Gather {
        episode: app.conversation().episode().id,
        turn: t,
        index: 0,
        config: app.conversation().episode().config.clone(),
    };
    app.fake_summary_on(app.current, target, vec![t]);
    let ask = asked_by_hook(&app, "tu_1", "Read", serde_json::json!({"file_path": file}));
    app.feed_summary(ask);
    assert!(
        matches!(
            provider.answered().as_slice(),
            [(id, Reply::Deny { reason })] if id == "tu_1" && reason.contains("may not run tools")
        ),
        "{:?}",
        provider.answered()
    );
    assert!(app.shown_call().is_none());

    // Its record is the notice line, never a round in the transcript — even
    // with a turn open there to receive one.
    app.conversation_mut().begin_turn("a turn beside it");
    app.feed_summary(StreamMsg::ToolRecord {
        call: shannon_core::Block::bash_call("tu_1", "ls"),
        result: shannon_core::Block::ToolResult {
            tool_use_id: "tu_1".into(),
            content: "refused".into(),
            is_error: true,
        },
    });
    assert!(
        !app.conversation().has_pending_tool_use()
            && !app
                .conversation()
                .messages()
                .iter()
                .any(|m| !m.blocks.is_empty()),
        "a draft's record entered the transcript"
    );

    // And its session is never filed: it saw a request the transcript does
    // not hold, so a turn resuming it would carry context nobody can see.
    // But it is Shannon's — noted, so `:graph` does not list it as a
    // conversation of Claude Code's own (Steven's screenshot, 10-16).
    app.feed_summary(StreamMsg::ProviderSession("sess-s".into()));
    app.feed_summary(StreamMsg::Complete);
    assert!(
        !app.claude_sessions.filed("sess-s"),
        "{:?}",
        app.claude_sessions
    );
    assert!(
        app.claude_sessions.started("sess-s"),
        "{:?}",
        app.claude_sessions
    );
}

/// **Allowing a question passes the pane to the next one waiting** — Claude
/// Code runs tools in parallel, and the queue behind an allow must not wait
/// for a result Claude Code produces (`tool_call_done`; review 21,
/// Reviewer C).
#[tokio::test]
async fn allowing_a_claude_code_question_passes_the_pane_on() {
    let (mut app, provider) = provider_app();
    let fetch = |url: &str| serde_json::json!({ "url": url });
    provider_asks(&mut app, "tu_1", "WebFetch", fetch("https://a.example"));
    provider_asks(&mut app, "tu_2", "WebFetch", fetch("https://b.example"));
    tool_key(&mut app, 'a');
    assert_eq!(provider.answered(), vec![allowed("tu_1")]);
    assert_eq!(
        app.shown_call()
            .map(|p| p.tool_use_id().to_string())
            .as_deref(),
        Some("tu_2")
    );
}

/// **Every string in a recorded call is scrubbed, inside arrays too** — a
/// `MultiEdit`'s `edits[]` and a `TodoWrite`'s `todos[]` (`scrub_value`;
/// review 21, Reviewer C).
#[tokio::test]
async fn a_recorded_call_is_scrubbed_inside_its_arrays() {
    let (mut app, _) = provider_app();
    let value = "Zk3pQ9vX2mL7tR4wB8nY";
    let secret = format!("OPENAI_API_KEY={value}");
    assert!(
        !app.scrub(&secret).contains(value),
        "premise: the redactor catches it"
    );
    let input = serde_json::json!({
        "file_path": "/tmp/x",
        "edits": [{ "old_string": "a", "new_string": secret }],
    });
    claude_code_ran(&mut app, "tu_1", "MultiEdit", input, "edited");
    claude_code_turn_ends(&mut app);
    let stored = serde_json::to_string(app.conversation().messages()).expect("json");
    assert!(!stored.contains(value), "{stored}");
}

/// **On Claude Code, a command naming a lent secret's variable asks**,
/// whatever the rules allow (Steven, 10-31, S18-Q1): the gate's verdict for
/// Claude Code's Bash comes from the rules built with this session's table,
/// so lending the variable is what turns an allowed read into a question.
#[tokio::test]
async fn naming_a_lent_variable_asks_on_claude_code() {
    let provider = shannon_core::backend::RecordingProvider::new();
    let mut app = with_provider(test_app(), &provider);
    let line = serde_json::json!({"command": "echo ${GITHUB_TOKEN:0:4}"});
    provider_asks(&mut app, "tu_1", "Bash", line.clone());
    assert!(app.shown_call().is_none(), "premise: nothing lent, it runs");
    assert_eq!(provider.answered(), vec![allowed("tu_1")]);

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
    app.aliases = Arc::new(table);
    app.rebuild_rules();
    provider_asks(&mut app, "tu_2", "Bash", line);
    assert!(app.shown_call().is_some(), "lent: it asks");
    assert_eq!(provider.answered().len(), 1, "and is not answered unasked");
}

/// **A variable of Shannon's holding a held value is withheld from `claude`
/// and asked of, however the value came to be held** (the close of review
/// stop 14, V 3): a token pasted into Shannon — marked, not lent — while
/// the shell exports it stayed in `claude`'s environment, and `echo
/// $GITHUB_TOKEN` ran allowed.
#[tokio::test]
async fn a_variable_holding_a_marked_value_is_withheld_and_asked_of() {
    let provider = shannon_core::backend::RecordingProvider::new();
    let mut app = with_provider(test_app(), &provider);
    let value = shannon_core::fakes::GITHUB_TOKEN_SHORT;
    app.process_env = vec![
        ("GITHUB_TOKEN".into(), value.into()),
        ("EDITOR".into(), "vim".into()),
    ];
    let line = serde_json::json!({"command": "echo ${GITHUB_TOKEN:0:4}"});
    provider_asks(&mut app, "tu_1", "Bash", line.clone());
    assert!(app.shown_call().is_none(), "premise: nothing held, it runs");
    assert!(
        !app.provider_env()
            .withheld
            .contains(&"GITHUB_TOKEN".to_string())
    );

    app.mark_secret(value, true).expect("marked, as a paste is");
    let withheld = app.provider_env().withheld;
    assert!(
        withheld.contains(&"GITHUB_TOKEN".to_string()),
        "{withheld:?}"
    );
    assert!(
        !withheld.contains(&"EDITOR".to_string()),
        "not another: {withheld:?}"
    );
    // What a command is handed is withheld the same (`exec_config`).
    assert!(
        app.withheld().contains(&"GITHUB_TOKEN".to_string()),
        "nor a command"
    );
    provider_asks(&mut app, "tu_2", "Bash", line);
    assert!(app.shown_call().is_some(), "held: it asks");
    assert_eq!(provider.answered().len(), 1, "and is not answered unasked");
}

/// **The login check runs `claude` in the environment a turn does** — under
/// `[claude_code] env = "minimal"`, a variable outside the minimal set is
/// not there for it either, so the login it reports is the one a turn uses
/// (Steven, 10-31, S18-Q1).
#[test]
fn the_login_check_is_given_a_turns_environment() {
    assert!(
        std::env::var_os("CARGO_PKG_NAME").is_some(),
        "premise: cargo's variable is in this process's environment"
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("claude");
    std::fs::write(
        &path,
        "#!/bin/sh\nprintf '{\"loggedIn\": true, \"authMethod\": \"%s\"}' \
         \"${CARGO_PKG_NAME:+inherited}\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let said = |policy| {
        let mut config = Config::default();
        config.claude_code.env = policy;
        let mut app = test_app_with(config);
        app.claude_program = path.clone();
        app.claude_login()
    };
    use shannon_core::backend::claude_code::LoginStatus;
    use shannon_core::config::EnvPolicy;
    assert_eq!(
        said(EnvPolicy::Inherit),
        LoginStatus::LoggedIn("inherited".into())
    );
    assert_eq!(
        said(EnvPolicy::Minimal),
        LoginStatus::LoggedIn(String::new())
    );
}
