//! What reaches the log, and what never may.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// --- what reaches the log -------------------------------------------------

/// The premise: the harness can see a log line at all.
///
/// A capture that silently returned `""` would make every assertion below pass
/// for the wrong reason, which is the shape that has bitten this suite before
/// (`[[feedback_assert_the_subject]]`).
#[test]
fn the_log_capture_actually_captures() {
    let logs = captured_logs(|| tracing::warn!(marker = "canary", "premise check"));
    assert!(
        logs.contains("canary") && logs.contains("premise check"),
        "the harness saw nothing: {logs:?}"
    );
}

/// **A notice quoting a lent secret reaches the screen whole and the log
/// masked.**
///
/// The two sinks have opposite audiences, and `post` is where that is decided.
/// Messages quote what they are about, so a refusal naming the offending
/// command carried it verbatim into a durable file — the one place `:secret`
/// exists to keep it out of.
#[test]
fn a_notice_naming_a_secret_is_masked_in_the_log_and_whole_on_screen() {
    let mut app = test_app();
    let secret = "hunter2beornot2be";
    let handle = app.mark_secret(secret, true).expect("marked");

    let logs = captured_logs(|| {
        app.notify(format!("could not reach {secret}"));
        app.refuse(format!("refusing to spend {secret}"));
    });

    // Diagnostics on EVERY assertion below: this test flaked at roughly one run
    // in forty and none of the failures were reproducible on demand, so the
    // next occurrence has to explain itself rather than just go red.
    let ctx = || {
        format!(
            "handle={handle:?}\naliases={:?}\nmessages={:?}\nlogs={logs:?}",
            app.aliases
                .entries()
                .iter()
                .map(|e| e.name.clone())
                .collect::<Vec<_>>(),
            app.messages().collect::<Vec<_>>(),
        )
    };
    assert!(
        !logs.contains(secret),
        "the value reached the log\n{}",
        ctx()
    );
    // Masked, not merely dropped — the line still says which secret it was
    // about, or the log stops being diagnosable at the point it matters.
    assert!(
        logs.contains(&handle),
        "the handle should stand in for it\n{}",
        ctx()
    );
    // Both levels, because `post` maps kind -> level and only one arm would
    // otherwise be covered.
    assert!(
        logs.contains("ui notice") && logs.contains("refused a user action"),
        "a level arm went missing\n{}",
        ctx()
    );

    // And the other half of the rule: the user is not who it is hidden from.
    assert!(
        app.messages().any(|m| m.contains(secret)),
        "the screen must still show the user their own words\n{}",
        ctx()
    );
}

/// **A protocol-surprise block logs its shape, not its payload.**
///
/// `warn!(?block, …)` Debug-prints the whole block: a `Text` block is model
/// output, a `ToolUse`'s `input` is the argument object — for `bash`, the
/// command. Five sites did that, all at `warn!`, which is on by default, and
/// the log is not scrubbed.
///
/// **Each live site driven**, each premised on its own line so a site that
/// stopped running cannot pass as one that logs nothing: a turn's tool
/// channel and a job's handed a block that is no call, and a distillation,
/// offered no tools, handed a bash call. This test called the accessors and
/// drove none: two sites put back to `?block` passed it (review stop 12, T2
/// 1). The other two of the five were unreachable — the dispatcher answers a
/// call on work offered none first — and are gone.
#[tokio::test]
async fn an_unexpected_block_logs_its_kind_not_its_content() {
    let payload = "rm -rf /tmp/correcthorsebatterystaple";
    let prose = "some model prose, correcthorse";
    let text = || {
        StreamMsg::ToolUse(shannon_core::Block::Text {
            text: prose.to_string(),
        })
    };
    let dir = TempDir::new().unwrap();
    let mut turn_app = app_with_n_turns(dir.path(), 2);
    let turn = captured_logs(|| turn_app.feed_stream(text()));
    let dir = TempDir::new().unwrap();
    let mut job_app = app_with_n_turns(dir.path(), 2);
    let child = job_app.spawn_child_job("go", None).expect("spawned");
    let job = captured_logs(|| job_app.feed_job(child, text()));
    let dir = TempDir::new().unwrap();
    let mut summary_app = app_with_n_turns(dir.path(), 2);
    let t = summary_app.conversation().episode().own_turns()[0];
    let target = SquashTarget::Gather {
        episode: summary_app.conversation().episode().id,
        turn: t,
        index: 0,
        config: summary_app.conversation().episode().config.clone(),
    };
    summary_app.fake_summary_on(summary_app.current, target, vec![t]);
    let call = StreamMsg::ToolUse(shannon_core::Block::bash_call("tool-1", payload));
    let summary = captured_logs(|| summary_app.feed_summary(call));
    for (site, logs, line, kind) in [
        ("turn", &turn, "non-tool_use block", "text"),
        ("job", &job, "non-tool_use block", "text"),
        ("distillation", &summary, "offered no tools", "bash"),
    ] {
        assert!(
            logs.contains(line) && logs.contains(kind),
            "premise, the {site}'s site ran and said its kind: {logs}"
        );
        assert!(
            !logs.contains("correcthorse"),
            "the {site}'s payload reached the log: {logs}"
        );
    }

    // The accessors the sites log through, the unit half.
    let call = shannon_core::Block::bash_call("tool-1", payload);
    let text = shannon_core::Block::Text {
        text: "some model prose".to_string(),
    };

    assert_eq!(call.kind(), "tool_use");
    assert_eq!(text.kind(), "text");
    assert_eq!(call.tool_name(), Some("bash"));
    assert_eq!(text.tool_name(), None);

    // The accessors are the whole defence, so pin that neither carries the
    // payload — a `kind()` that returned `format!("{self:?}")` would satisfy
    // "is a string" and leak everything.
    assert!(!call.kind().contains("rm -rf"));
    assert!(!call.tool_name().unwrap_or_default().contains("rm -rf"));
    assert!(!text.kind().contains("model prose"));
}

/// **A completion record names its command and its call.**
///
/// It used to pass `""` for the command and, at the default `Decisions` level,
/// drop the output too — so the only line saying whether an authorized command
/// actually succeeded read `decision=ok command=""`, with no exit code and
/// nothing to correlate. You could tell what had finished only by adjacency to
/// the earlier `run` line, which `:Send` and `:child` break by design.
///
/// **Driven through `handle_exec_outcome`, not `audit` directly.** Calling the
/// helper with a good command tests the door and says nothing about the caller
/// that walked through it with `""` — asserted that way first, and the mutation
/// restoring the bug passed clean (`[[feedback_wiring_is_untested]]`).
#[test]
fn an_audit_completion_record_can_be_matched_to_its_command() {
    let mut app = test_app();
    let outcome = shannon_core::ExecOutcome {
        tool_use_id: "toolu_01".to_string(),
        command: "cargo test --quiet".to_string(),
        output: "3 passed".to_string(),
        code: Some(0),
        timed_out: false,
        truncated: false,
        spawn_error: None,
        redacted: Vec::new(),
        spent: Vec::new(),
        attempts: 1,
        kept: None,
        left: shannon_core::LeftBehind::Nothing,
        file: None,
    };
    let logs = captured_logs(|| app.handle_exec_outcome(outcome));

    let done = logs
        .lines()
        .find(|l| l.contains(r#"decision="ok""#))
        .unwrap_or_else(|| panic!("no completion record:\n{logs}"));
    assert!(
        done.contains(r#"command="cargo test --quiet""#),
        "the outcome does not name its command: {done}"
    );
    assert!(
        done.contains(r#"tool_use_id="toolu_01""#),
        "nothing correlates it to the run record: {done}"
    );
    assert!(done.contains("code=Some(0)"), "no exit status: {done}");
    assert!(done.contains("timed_out=false"), "{done}");
    // At the DEFAULT level the output stays out — it is the part most likely to
    // carry credentials, and a log is durable.
    assert!(
        !done.contains("3 passed"),
        "output leaked at Decisions: {done}"
    );
}

/// **A literal credential in a command never reaches the log**, whichever
/// line logs the command — the audit, and the diary's lines for an allow, a
/// deny, an auto-run, an edit, a stop, a quit, a held child's call, and the
/// prompt itself. Each scrubs; deleting any one passed every test, because
/// every secret test fed a handle, which is expanded only at exec, after
/// the log (full review 10-21, E 1, mutated: 11 sites GREEN).
#[tokio::test]
async fn a_literal_credential_in_a_command_never_reaches_the_log() {
    const VALUE: &str = "hunter2seekrit";
    let bound = Commands {
        asks: "frobnicate --token password=hunter2seekrit",
        by_rules: "echo password=hunter2seekrit",
        left: "frobnicate --serve password=hunter2seekrit &",
    };
    for (name, logs) in logs_of_every_command_line(&bound) {
        assert!(!logs.contains(VALUE), "{name}:\n{logs}");
    }
    let mut app = test_app();
    let logs = captured_logs(|| {
        app.prompt_mut()
            .insert_pasted("tell me about password=hunter2seekrit");
        app.submit_prompt();
    });
    assert!(
        logs.contains("tell me"),
        "the prompt: premise: logged:\n{logs}"
    );
    assert!(!logs.contains(VALUE), "the prompt:\n{logs}");
}

/// **A credential written after its flag never reaches the log either** —
/// `--password hunter2`, the commonest CLI spelling, which the gate refuses
/// to make a rule of and every one of these lines kept in clear: the text
/// rule reads only `--password=x`, and keeps a floor this value is under
/// (review stop 17, E1 M1). A command is read where the shell reads it; the
/// prompt is prose, where the same words would be a guess, and stays out.
#[tokio::test]
async fn a_credential_after_its_flag_never_reaches_the_log() {
    const VALUE: &str = "hunter2";
    // Not `echo`: its words are all operands, so the gate reads no flag there,
    // and neither does the scrub that asks it.
    let spaced = Commands {
        asks: "frobnicate --password hunter2 now",
        by_rules: "frobnicate --password hunter2 --dry-run",
        left: "frobnicate --serve --password hunter2 &",
    };
    for (name, logs) in logs_of_every_command_line(&spaced) {
        assert!(!logs.contains(VALUE), "{name}:\n{logs}");
    }
}

/// **A credential after its flag is kept off every record of its call** —
/// the quit's list (running, waiting, what a finished one left), the pane
/// kept for the session's next call, the refusals naming a call nothing
/// holds, a rule's revisit of the open one. Each is drawn, and a notice is
/// logged too; each was handed the command by the text door, and putting one
/// back passed every test (review stop 17, E1 M1: the doors' mutants).
#[tokio::test]
async fn a_credential_after_its_flag_is_kept_off_every_record_of_its_call() {
    const VALUE: &str = "hunter2";
    const ASKS: &str = "frobnicate --password hunter2 now";
    let kept_off = |what: &str, said: &str| {
        assert!(
            said.contains("frobnicate"),
            "{what}: premise: named: {said}"
        );
        assert!(!said.contains(VALUE), "{what}: {said}");
    };
    let notices = |app: &App| {
        (app.notices.iter())
            .map(|(_, said)| said.clone())
            .collect::<Vec<_>>()
            .join("\n")
    };

    // The quit's list: one running, one waiting, one a finished call left.
    let mut app = test_app();
    app.executor = Arc::new(LeftBehindExecutor(
        "frobnicate --serve --password hunter2 &",
    ));
    manual_tool(&mut app, ASKS);
    tool_key(&mut app, 'a');
    manual_tool(&mut app, "frobnicate --password hunter2 later");
    let ends = app.quitting_ends().join("\n");
    for part in ["is running", "waiting on your decision", "left"] {
        assert!(ends.contains(part), "premise: {part}: {ends}");
    }
    kept_off("the quit's list", &ends);

    // A child held on the call: the quit's list says what it is held on,
    // and that line was kept raw (stop 17, V2a M1).
    let mut app = test_app();
    app.conversation_mut().begin_turn("hello");
    app.conversation_mut().push_chunk("hi");
    app.conversation_mut().commit_turn();
    app.spawn_child_job("go and look at something", None)
        .expect("spawns");
    let jobs = app.generation_ids_for_test();
    app.gate_job_tool_call_for_test(jobs[0], ASKS);
    let ends = app.quitting_ends().join("\n");
    assert!(ends.contains("held on"), "premise: held: {ends}");
    kept_off("a held child in the quit's list", &ends);

    // The pane kept open for the session's next call, while one runs.
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    let chat = app.current;
    manual_tool(&mut app, ASKS);
    manual_tool(&mut app, "rm /tmp/shannon-test-absent-2");
    app.settle();
    tool_key_in(&mut app, chat, 'a');
    let pane = app.decision_pane_of(chat).expect("premise: kept open");
    let (_, body) = app.decision_wait_text(pane).expect("premise: it waits");
    kept_off("the pane kept for the next call", &body);

    // The refusals naming a call nothing holds.
    let mut app = test_app();
    app.decide_tool_call("tu_none", ASKS);
    kept_off("not put to you", &notices(&app));
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    two_calls_asking(&mut app);
    app.queue_call_for_test(app.current, "tu_stale", ASKS, true);
    tool_key(&mut app, 'd');
    kept_off("dropped from the waiting", &notices(&app));
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.conversation_mut().begin_turn("go");
    for (id, line) in [("tu_1", ASKS), ("tu_2", "rm -rf /tmp/shannon-test-absent")] {
        app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(id, line)));
    }
    app.feed_stream(StreamMsg::Complete);
    app.forget_tool_call("tu_1");
    tool_key(&mut app, 'a');
    kept_off("not run", &notices(&app));

    // A rule's revisit of the call on the pane.
    let mut app = test_app();
    manual_tool(&mut app, ASKS);
    app.dispatch_command_for_test("allow frobnicate *");
    kept_off("the revisit", &notices(&app));
}

/// What a command-logging scenario is driven with: a command that asks, one
/// the rules run, and one a finished command left running.
struct Commands {
    asks: &'static str,
    by_rules: &'static str,
    left: &'static str,
}

/// Runs nothing, and reports one finished command still running — what
/// quitting ends and logs by name.
#[derive(Debug)]
struct LeftBehindExecutor(&'static str);

impl shannon_core::Executor for LeftBehindExecutor {
    fn spawn(
        &self,
        _: String,
        _: String,
        _: shannon_core::ExecConfig,
        _: tokio::sync::mpsc::UnboundedSender<shannon_core::ExecOutcome>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async {})
    }
    fn file(
        &self,
        _: String,
        _: shannon_core::files::Job,
        _: shannon_core::ExecConfig,
        _: tokio::sync::mpsc::UnboundedSender<shannon_core::ExecOutcome>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async {})
    }
    fn leftovers(&self) -> Vec<shannon_core::Leftover> {
        vec![shannon_core::Leftover {
            tool_use_id: "tu_left".into(),
            command: self.0.into(),
            since: std::time::SystemTime::now(),
            processes: Vec::new(),
            printed: 0,
            last_line: None,
        }]
    }
}

/// **Each line that logs a command, driven with `commands`**, each
/// scenario's log by its name, every one premised on having logged its
/// command — a site that stopped running cannot pass as one that hides the
/// value.
fn logs_of_every_command_line(commands: &Commands) -> Vec<(&'static str, String)> {
    type Scenario = fn(&mut App, &Commands);
    let scenarios: [(&str, Scenario); 8] = [
        ("allowed once", |app, c| {
            manual_tool(app, c.asks);
            tool_key(app, 'a');
        }),
        ("denied", |app, c| {
            manual_tool(app, c.asks);
            tool_key(app, 'd');
        }),
        ("run by the rules", |app, c| {
            app.dispatch_command_for_test("allow frobnicate *");
            manual_tool(app, c.by_rules);
        }),
        ("edited", |app, c| {
            manual_tool(app, "frobnicate --walk .");
            let seed = app.shown_call().expect("asks").command().to_string();
            app.apply_edit_for_test(&seed, c.asks);
            tool_key(app, 'a');
        }),
        ("stopped by CTRL-C", |app, c| {
            model_call(app, "tu_1", c.asks);
            tool_key(app, 'a');
            app.set_focus(PaneKind::Prompt);
            app.interrupt();
        }),
        ("ended by quitting", |app, c| {
            manual_tool(app, c.asks);
            tool_key(app, 'a');
            let _ = app.leaving(true);
        }),
        ("a held child's call", |app, c| {
            app.conversation_mut().begin_turn("hello");
            app.conversation_mut().push_chunk("hi");
            app.conversation_mut().commit_turn();
            app.spawn_child_job("go and look", None).expect("spawns");
            let jobs = app.generation_ids_for_test();
            app.gate_job_tool_call_for_test(jobs[0], c.asks);
        }),
        (
            "what a finished command left running, ended on quitting",
            |app, c| {
                app.executor = Arc::new(LeftBehindExecutor(c.left));
                let _ = app.leaving(true);
            },
        ),
    ];
    (scenarios.into_iter())
        .map(|(name, scenario)| {
            let mut app = test_app();
            app.executor = Arc::new(shannon_core::RecordingExecutor::default());
            let logs = captured_logs(|| scenario(&mut app, commands));
            // The scenario reached a line that logs the command as one: a
            // rule's notice names the program too, and is no command line.
            let logged = (logs.lines()).any(|l| {
                l.contains("command=") && (l.contains("frobnicate") || l.contains("echo"))
            });
            assert!(logged, "{name}: premise: logged:\n{logs}");
            (name, logs)
        })
        .collect()
}

/// The other half: `full` does record the output, or the level means nothing.
#[test]
fn the_full_audit_level_records_the_output() {
    let logs = audit_of_an_outcome(shannon_core::AuditLevel::Full);
    let done = logs
        .lines()
        .find(|l| l.contains(r#"decision="ok""#))
        .unwrap_or_else(|| panic!("no completion record:\n{logs}"));
    assert!(done.contains(r#"output="3 passed""#), "{done}");
}

/// And `off` records nothing at all.
#[test]
fn the_off_audit_level_writes_no_record() {
    assert!(
        audit_of_an_outcome(shannon_core::AuditLevel::Decisions).contains("decision="),
        "premise: the same outcome is recorded at the default level"
    );
    let logs = audit_of_an_outcome(shannon_core::AuditLevel::Off);
    assert!(!logs.contains("decision="), "audit=off still wrote: {logs}");
}

/// **A refusal from a key press carries the session it happened in.**
///
/// The wiring half. The mechanism is covered in `logging.rs`; this is the part
/// that would otherwise be untested by construction — that `handle_event`
/// actually enters a span, with the episode of the buffer the key went to.
/// **The span resolves the buffer's episode as it is NOW, not as it was.**
///
/// The failure mode `span_for` exists to prevent: `adopt_session` repoints an
/// existing `BufferId` at a different `EpisodeId` — which is how `:clone`, a
/// checkout and every compose land their result — so an episode captured when a
/// generation started goes stale under it, and the log would confidently name a
/// session the buffer no longer holds.
///
/// Note on coverage: this tests the *resolution*, which is the half that can be
/// wrong in an interesting way. The `.entered()` call inside `handle_event` is
/// not covered, because `handle_event` takes a live terminal and is unreachable
/// from `cargo test` — the same reason the seams that stand in for it
/// (`key_for_test`, `dispatch_command_for_test`) exist at all
/// (`[[feedback_wiring_is_untested]]`). The other two dispatch sites are
/// covered, below and in `an_exec_outcome_tags_its_lines_with_the_asking_session`.
#[test]
fn the_session_span_follows_a_buffer_that_is_repointed() {
    let mut app = test_app();
    let buffer = app.current;
    let before = app.conversation().episode().id;

    let logs_before = captured_logs(|| {
        app.span_for(buffer).in_scope(|| tracing::warn!("first"));
    });
    assert!(logs_before.contains(&before.short_hex()), "{logs_before}");

    // Repoint the buffer at a different episode, exactly as a compose does.
    let adopted = shannon_core::Episode::root(
        shannon_core::EpisodeId::from_seed(b"adopted here"),
        shannon_core::TurnConfig {
            model: "stub".to_string(),
            max_tokens: None,
            effort: None,
        },
    );
    let after = adopted.id;
    assert_ne!(before, after, "premise: a genuinely different episode");
    let recall = app.unlock_turns(&[]).expect("nothing to unlock");
    app.adopt_session(
        buffer,
        adopted,
        Vec::new(),
        OnDisk::Absent,
        recall,
        Landing::Asked,
    );

    let logs_after = captured_logs(|| {
        app.span_for(buffer).in_scope(|| tracing::warn!("second"));
    });
    assert!(
        logs_after.contains(&after.short_hex()),
        "the span kept the old episode after the buffer moved:\n{logs_after}"
    );
    assert!(
        !logs_after.contains(&before.short_hex()),
        "it named a session this buffer no longer holds:\n{logs_after}"
    );
}

/// **A finished command is tagged with the session that asked for it.**
///
/// `ExecOutcome` carries only a `tool_use_id`; the span is what turns that back
/// into "which conversation ran this".
#[test]
fn an_exec_outcome_tags_its_lines_with_the_asking_session() {
    let mut app = test_app();
    let episode = app.conversation().episode().id;
    // A command has a record from its arrival on; this test fed an outcome
    // for a call never tracked, which only `tool_buffer`'s old fallback to
    // `current` resolved (full review 10-21, A 1).
    app.track_tool_call("toolu_span", None);
    let outcome = shannon_core::ExecOutcome {
        tool_use_id: "toolu_span".to_string(),
        command: "echo hi".to_string(),
        output: "hi".to_string(),
        code: Some(0),
        timed_out: false,
        truncated: false,
        spawn_error: None,
        redacted: Vec::new(),
        spent: Vec::new(),
        attempts: 1,
        kept: None,
        left: shannon_core::LeftBehind::Nothing,
        file: None,
    };

    let logs = captured_logs(|| app.handle_exec_outcome(outcome));
    let audit = logs
        .lines()
        .find(|l| l.contains("tool call"))
        .unwrap_or_else(|| panic!("no audit record:\n{logs}"));
    assert!(
        audit.contains(&episode.short_hex()),
        "the audit record does not say which session ran it: {audit}"
    );
}
