//! The tool loop: a call's round trip, deny/allow, edit, manual `:tool`.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// Whatever the outcome, the transcript records what happened — a status
/// line scrolls away, but "what did it run on my machine" must not.
#[tokio::test]
async fn tool_activity_is_recorded_in_the_transcript() {
    // An auto-allowed command: no prompt, but still visible.
    let mut app = test_app();
    model_call(&mut app, "tu_1", "echo hi");
    assert!(app.shown_call().is_none());
    settle(&mut app).await;
    assert!(
        app.output().lines().iter().any(|l| l.contains("echo hi")),
        "the transcript shows what ran: {:?}",
        app.output().lines()
    );

    // A denial is recorded too — the refusal is part of the history.
    let mut app = test_app();
    model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent");
    tool_key(&mut app, 'd');
    let lines = app.output().lines().join("\n");
    assert!(lines.contains("DENIED"), "{lines}");
    assert!(
        !lines.contains("output of"),
        "a denied command must produce no output block: {lines}"
    );
    assert!(lines.contains("rm -rf /tmp/shannon-test-absent"), "{lines}");
}

/// A command recorded in the transcript is laid out the way the gate showed
/// it, not flattened into a wall of shell — and the knob turns it off.
#[tokio::test]
async fn the_transcript_pretty_prints_a_recorded_command() {
    let cmd = r#"while read -r F; do rm -r "$F"; done < <(find .)"#;
    let mut app = test_app();
    model_call(&mut app, "tu_1", cmd);
    tool_key(&mut app, 'd');
    let lines = app.output().lines();
    assert!(
        lines.iter().any(|l| l.trim() == "while read -r F; do"),
        "control flow gets its own line: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("    ") && l.trim().starts_with("rm -r")),
        "the loop body is indented under it: {lines:?}"
    );

    // Verbatim when the knob is off.
    let mut cfg = Config::default();
    cfg.tui.pretty_commands = false;
    let mut app = test_app_with(cfg);
    model_call(&mut app, "tu_1", cmd);
    tool_key(&mut app, 'd');
    assert!(
        app.output().lines().iter().any(|l| l.contains(cmd)),
        "kept on one line when asked"
    );
}

/// The loop closes: a tool call holds the turn open, the human's decision
/// produces a result, and the result goes back so the assistant can
/// continue — all inside ONE turn (one prompt, one turn, however many
/// round-trips it took).
#[tokio::test]
async fn a_tool_call_holds_the_turn_open_and_resumes_with_the_result() {
    let mut app = test_app();
    app.start_turn("what's here?");
    app.feed_stream(StreamMsg::Chunk("Let me look.".into()));
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_1",
        "rm -r /tmp/shannon-test-absent",
    )));
    // The stream finishes, but the turn must NOT seal: the model is waiting.
    app.feed_stream(StreamMsg::Complete);
    assert!(app.awaiting_decision(), "the turn is held open");
    assert!(
        app.conversation().turn_in_progress(),
        "and is not committed while a call is unanswered"
    );

    // The human allows it → the result is sent back and the turn resumes.
    tool_key(&mut app, 'a');
    settle(&mut app).await;
    assert!(!app.awaiting_decision(), "resumed");
    assert!(app.streaming(), "a continuation was sent");

    // The transcript carries the pair the API needs: the assistant message
    // that ASKED (with its tool_use) and the user message that ANSWERED.
    let msgs = app.conversation().snapshot();
    let asked = msgs
        .iter()
        .find(|m| m.tool_uses().next().is_some())
        .expect("the assistant's tool_use is preserved");
    let (id, _, _) = asked.tool_uses().next().unwrap();
    assert_eq!(id, "tu_1");
    let answered = msgs
        .iter()
        .rev()
        .find(|m| {
            m.blocks
                .iter()
                .any(|b| matches!(b, shannon_core::Block::ToolResult { .. }))
        })
        .expect("the result is in the transcript");
    assert!(matches!(
        &answered.blocks[0],
        shannon_core::Block::ToolResult { tool_use_id, .. } if tool_use_id == "tu_1"
    ));

    // Finishing without a pending call now seals the whole thing as ONE turn.
    app.feed_stream(StreamMsg::Chunk(" All done.".into()));
    app.feed_stream(StreamMsg::Complete);
    assert!(!app.conversation().turn_in_progress(), "sealed");
}

/// A decision made BEFORE the stream finishes must not fire a second request
/// while one is in flight — only one backend slot.
#[tokio::test]
async fn deciding_before_the_stream_finishes_waits_for_it() {
    let mut app = test_app();
    app.start_turn("go");
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_1",
        "rm -r /tmp/shannon-test-absent-x",
    )));
    let in_flight = app.streaming();
    tool_key(&mut app, 'a'); // decide while the stream is still open
    assert!(
        !app.awaiting_decision(),
        "nothing to resume yet — Complete hasn't arrived"
    );
    assert_eq!(
        app.streaming(),
        in_flight,
        "no second request while one is open"
    );

    // Once the command finishes AND the stream ends, the turn resumes.
    settle(&mut app).await;
    app.feed_stream(StreamMsg::Complete);
    assert!(app.streaming(), "resumed after the stream ended");
}

/// Several calls in one message are answered one at a time (a decision
/// deserves undivided attention) and returned together, as the API requires.
#[tokio::test]
async fn multiple_tool_calls_are_queued_and_answered_together() {
    let mut app = test_app();
    app.start_turn("go");
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "a",
        "rm -r /tmp/shannon-test-absent-1",
    )));
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "b",
        "rm -r /tmp/shannon-test-absent-2",
    )));
    assert_eq!(app.waiting_calls().len(), 1, "the second waits its turn");
    assert_eq!(
        app.shown_call().unwrap().command(),
        "rm -r /tmp/shannon-test-absent-1"
    );

    tool_key(&mut app, 'a');
    settle(&mut app).await;
    assert_eq!(
        app.shown_call().expect("the second is now asked").command(),
        "rm -r /tmp/shannon-test-absent-2"
    );
    tool_key(&mut app, 'd');
    app.feed_stream(StreamMsg::Complete);

    let answered = app
        .conversation()
        .snapshot()
        .into_iter()
        .rev()
        .find(|m| {
            m.blocks
                .iter()
                .any(|b| matches!(b, shannon_core::Block::ToolResult { .. }))
        })
        .expect("results returned");
    assert_eq!(answered.blocks.len(), 2, "one message, both results");
}

/// An edited command must be DISCLOSED to the model.
///
/// The model's context holds the command it asked for; handing back the
/// output of a different one without saying so is how "I changed the `rm` to
/// an `echo`" became "the agent thinks we deleted everything". (Steven's
/// report — the same message-honesty rule we hold for the user, pointed at
/// the model, because what misleads it comes back to mislead the user.)
#[tokio::test]
async fn an_edited_command_is_disclosed_to_the_model() {
    let mut app = test_app();
    app.start_turn("clean up");
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_1",
        "rm -r /tmp/shannon-test-absent",
    )));
    edit_shown(&mut app, "echo hi");
    tool_key(&mut app, 'a');
    settle(&mut app).await;

    let (content, _) = tool_result(&app).expect("answered");
    assert!(
        content.contains("did not run your command"),
        "the substitution is stated: {content}"
    );
    assert!(
        content.contains("rm -r /tmp/shannon-test-absent"),
        "{content}"
    );
    assert!(content.contains("echo hi"), "{content}");
}

/// **…and a credential typed into the edit is disclosed by its place** — the
/// note goes to the model and onto the screen, the edited command the
/// likeliest place one is typed by hand (review stop 17, E1 M1: the text
/// rule kept `--password hunter2`). `true`, not `echo`: echo's words are
/// operands, and the gate reads no flag there.
#[tokio::test]
async fn an_edited_commands_flag_credential_is_disclosed_by_its_place() {
    let mut app = test_app();
    app.start_turn("clean up");
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_1",
        "rm -r /tmp/shannon-test-absent",
    )));
    edit_shown(&mut app, "true --password hunter2");
    tool_key(&mut app, 'a');
    settle(&mut app).await;

    let (content, _) = tool_result(&app).expect("answered");
    // The note, not the output after it: the test executor's stands in as
    // `(recorded, not run) <command>`, which a real `true` never prints.
    let note = content.split("\n\n").next().unwrap_or_default();
    assert!(
        note.contains("what actually ran: true --password [redacted: named secret]"),
        "{content}"
    );
    assert!(!note.contains("hunter2"), "{content}");
    let lines = app.output().lines();
    let ran = (lines.iter())
        .find(|line| line.contains("what actually ran:"))
        .unwrap_or_else(|| panic!("premise: the note is drawn: {lines:?}"));
    assert!(!ran.contains("hunter2"), "{ran}");
}

/// The counterpart: an UNedited command carries no such note — a disclosure
/// that fires when nothing changed is noise that teaches the model (and the
/// reader) to skip it.
#[tokio::test]
async fn an_unedited_command_carries_no_disclosure() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -r /tmp/shannon-test-absent");
    tool_key(&mut app, 'a');
    settle(&mut app).await;
    let (content, _) = tool_result(&app).expect("answered");
    assert!(!content.contains("did not run your command"), "{content}");
}

/// A backend failure mid-round must not leave a decision pane attached to a
/// dead turn: answering it later would splice a `tool_result` into a turn
/// that never made the request.
#[tokio::test]
async fn a_failed_turn_abandons_its_pending_tool_round() {
    let mut app = test_app();
    app.start_turn("go");
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_1",
        "rm -r /tmp/shannon-test-absent",
    )));
    assert!(app.shown_call().is_some(), "gated");

    app.feed_stream(StreamMsg::Error(shannon_core::BackendError::RateLimited));
    assert!(app.shown_call().is_none(), "the pane goes with the turn");
    assert!(app.tool_results.is_empty(), "no stale results");
    assert!(app.waiting_calls().is_empty(), "no stale queue");
    assert!(!app.awaiting_decision(), "nothing left to resume");
    assert!(
        app.notice().is_some(),
        "and it says so, rather than vanishing"
    );
}

/// Every call must be answered before the turn resumes — a reply missing a
/// `tool_result` for any `tool_use` is a protocol error the API rejects.
///
/// Uses two AUTO-ALLOWED calls on purpose: they never open a prompt, so the
/// completeness check is the only thing holding the reply back. (An earlier
/// version of this test used gated calls and passed even with the check
/// removed — the open prompt was doing the work.)
#[tokio::test]
async fn the_turn_waits_until_every_call_is_answered() {
    let mut app = test_app();
    app.start_turn("go");
    for (id, cmd) in [("a", "echo one"), ("b", "echo two")] {
        app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(id, cmd)));
    }
    app.feed_stream(StreamMsg::Complete);
    assert!(app.shown_call().is_none(), "both auto-allowed; no prompt");
    assert_eq!(app.conversation().pending_tool_uses(), 2);

    // One result in: the reply must NOT go out short.
    settle(&mut app).await;
    assert_eq!(app.tool_results.len(), 1);
    // And it is filed against the call it answers, not just counted.
    let (first, _) = tool_result_for(&app, "a").expect("a is answered");
    assert!(first.contains("echo one"), "{first}");
    assert!(tool_result_for(&app, "b").is_none(), "b is still open");
    assert!(app.awaiting_decision(), "still waiting on the second call");
    assert!(!app.streaming(), "no short reply sent");

    // Both in: now it resumes, carrying both results.
    settle(&mut app).await;
    assert!(!app.awaiting_decision(), "resumed");
    assert!(app.streaming());
    let answered = app
        .conversation()
        .snapshot()
        .into_iter()
        .rev()
        .find(|m| {
            m.blocks
                .iter()
                .any(|b| matches!(b, shannon_core::Block::ToolResult { .. }))
        })
        .expect("results returned");
    assert_eq!(answered.blocks.len(), 2, "one message, both results");
}

/// A result for a call the turn never made is DROPPED, not queued. Such an
/// orphan would satisfy the "are we done" check, let the reply go out before
/// the real result arrived, and reach the wire as a `tool_result` matching no
/// `tool_use`. (All three reviewers found this.)
#[test]
fn a_result_for_an_unknown_call_is_dropped() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("go");
    app.finish_tool_call("never-asked", "output".into(), false);
    assert!(
        app.tool_results.is_empty(),
        "an orphan result never enters the set"
    );
}

/// `:tool` answers no model request, so its result is shown and never sent —
/// and each invocation gets a fresh id, so the command works more than once
/// per session. Through `:tool`'s door (`manual_tool`): the test minted the
/// ids itself and asserted its own count, so `typed_tool_call` reusing one
/// passed it (review stop 12, T1 7).
#[tokio::test]
async fn manual_tool_calls_are_repeatable_and_never_returned() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("go");
    // Two `:tool` runs. A hardcoded id made the second hit the
    // at-most-once guard and die with a misleading "already run".
    let mut ids = Vec::new();
    for _ in 0..2 {
        ids.push(manual_tool(&mut app, "echo hi"));
        settle(&mut app).await;
    }
    assert_ne!(ids[0], ids[1], "a fresh id each time");
    assert!(
        app.tool_results.is_empty(),
        "manual results are shown, never queued for the wire"
    );
    // Both actually ran — the second was not refused as a duplicate.
    let ran = app.output().lines().join("\n").matches("echo hi").count();
    assert!(ran >= 2, "both invocations ran: {ran}");
}

/// A turn paused on a human decision still holds the backend slot. `Complete`
/// clears `current_stream` before the gate is answered, so the app *looked*
/// idle: submitting then called `begin_turn`, which overwrote the in-flight
/// draft and discarded the model's unanswered `tool_use`. (Reviewer A.)
#[tokio::test]
async fn a_pending_decision_still_counts_as_a_turn_in_flight() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -r /tmp/shannon-test-absent");
    app.feed_stream(StreamMsg::Complete);
    assert!(!app.streaming(), "the stream really did end");
    assert!(app.turn_in_flight().is_some(), "but the turn has not");

    // A prompt submitted meanwhile queues instead of clobbering the draft.
    app.submit("something else");
    assert_eq!(app.pending.len(), 1, "queued, not started");
    assert_eq!(
        app.conversation().pending_tool_use_ids(),
        vec!["tu_1".to_string()],
        "the model's request survives"
    );
}

/// Finishing one call must never REPLACE a decision already on screen: the
/// pane would swap under the cursor (a keystroke aimed at one command
/// resolving another), and the displaced call would never be answered,
/// wedging the turn open. (Reviewer E.)
#[tokio::test]
async fn a_finished_call_never_replaces_the_decision_on_screen() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("go");
    // A: auto-allowed. B: gated. C: queued behind B.
    for (id, cmd) in [
        ("a", "echo hi"),
        ("b", "rm -r /tmp/shannon-test-absent-b"),
        ("c", "rm -r /tmp/shannon-test-absent-c"),
    ] {
        app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(id, cmd)));
    }
    let showing = app.shown_call().expect("gated").command().to_string();
    assert!(showing.contains("absent-b"), "B is on screen: {showing}");

    // A's result arriving must not swap the pane.
    settle(&mut app).await;
    assert_eq!(
        app.shown_call().expect("still B").command(),
        showing,
        "the decision under the cursor is untouched"
    );

    // Answering B then brings C up, and every call ends up answered.
    tool_key(&mut app, 'd');
    assert!(app.shown_call().unwrap().command().contains("absent-c"));
    tool_key(&mut app, 'd');
    assert_eq!(app.tool_results.len(), 3, "no call is left unanswered");
}

/// **`e` on a call, through `$EDITOR` and back** (`editor::Editor`): the
/// key hands the editor over (`ToolKeyEffect::Edit`, the event loop's
/// door), the editor opens on the command as shell, and what it saves is
/// the call, re-gated; quit unsaved, the call is as it was, and said; a
/// failed editor is said, the call still waiting. Tests drove the halves
/// either side of the editor (`apply_edit_for_test`), never the door.
#[tokio::test]
async fn e_edits_a_call_through_the_editor_and_back() {
    use crate::editor::Filetype;
    let mut app = test_app();
    let chat = app.current;
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    assert_eq!(tool_key(&mut app, 'e'), ToolKeyEffect::Edit(chat));

    let mut editor = Scripted::answering([Ok(Some("echo hi".to_string()))]);
    app.edit_tool_command(chat, &mut editor);
    assert_eq!(
        editor.opened,
        [(
            "rm -rf /tmp/shannon-test-absent".to_string(),
            Filetype::Shell
        )],
        "opened on the command, as shell"
    );
    assert_eq!(
        app.shown_call().map(|p| p.command()),
        Some("echo hi"),
        "the call, edited and re-gated"
    );

    let mut editor = Scripted::answering([Ok(None)]);
    app.edit_tool_command(chat, &mut editor);
    assert_eq!(app.shown_call().map(|p| p.command()), Some("echo hi"));
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("edit cancelled"), "{said}");

    let mut editor = Scripted::answering([Err(anyhow::anyhow!("no $EDITOR"))]);
    app.edit_tool_command(chat, &mut editor);
    assert_eq!(app.shown_call().map(|p| p.command()), Some("echo hi"));
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("could not open the editor: no $EDITOR"),
        "{said}"
    );
}

/// A manual `:tool` call finishing LAST must still drain the round. Returning
/// early left `tool_turn_open` true forever, so every later prompt queued in
/// silence and Enter appeared to stop working. (Reviewer F.)
#[tokio::test]
async fn a_manual_call_finishing_last_still_closes_the_round() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -r /tmp/shannon-test-absent");
    app.feed_stream(StreamMsg::Complete);
    // A manual call queued behind the model's decision, through `:tool`'s
    // door.
    manual_tool(&mut app, "rm -r /tmp/shannon-test-absent-manual");
    assert_eq!(app.waiting_calls().len(), 1, "queued behind the decision");

    tool_key(&mut app, 'a'); // answer the model's call
    settle(&mut app).await; // its command finishes → drains to the manual one
    assert!(
        app.shown_call()
            .expect("manual is now gated")
            .command()
            .contains("absent-manual")
    );
    tool_key(&mut app, 'a'); // answer the manual one
    settle(&mut app).await; // the MANUAL command finishes last
    assert!(
        !app.awaiting_decision(),
        "the round closed rather than hanging on a call whose result is never sent"
    );
}

/// A dangerous call must STOP for a human — the gate is the whole point.
#[tokio::test]
async fn a_dangerous_tool_call_raises_the_gate_and_runs_nothing() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    assert!(app.shown_call().is_some(), "a human must decide");
    assert!(tool_result(&app).is_none(), "nothing resolved yet");
}

/// Deny returns an ERROR result (so the model re-plans) and runs nothing.
#[tokio::test]
async fn denying_returns_an_error_result() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    tool_key(&mut app, 'd');
    assert!(app.shown_call().is_none(), "the decision is resolved");
    let (content, is_error) = tool_result(&app).expect("a result was produced");
    assert!(is_error, "a denial is an error result");
    assert!(content.contains("denied"), "{content}");
}

/// **`Esc` answers nothing; `q` denies.** A vim user presses `Esc` by reflex,
/// before `<C-w>`, without reading — as a deny it cost them the call they
/// meant to allow (Steven, 10-11). It leaves the call open and says nothing;
/// `q`, the dismissal they reach for deliberately, is a denial.
#[tokio::test]
async fn escape_answers_nothing_and_q_denies() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app);
    let before = app.notice();
    app.handle_tool_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), (80, 100));
    assert!(app.shown_call().is_some(), "Esc left the call open");
    assert!(tool_result(&app).is_none(), "and resolved nothing");
    assert_eq!(app.notice(), before, "and said nothing");
    app.handle_tool_key(
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        (80, 100),
    );
    assert!(app.shown_call().is_none());
    let (_, is_error) = tool_result(&app).expect("q resolves the call");
    assert!(is_error, "q is a denial, not a silent drop");
}

/// Allow-once runs THIS call and leaves standing policy alone — the next
/// identical call must stop at the gate again.
#[tokio::test]
async fn allow_once_does_not_widen_policy() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    tool_key(&mut app, 'a');
    assert!(app.shown_call().is_none());
    settle(&mut app).await;
    assert!(tool_result(&app).is_some(), "it ran");
    assert!(app.session_rules.is_empty(), "nothing was taught");

    app.tool_results.clear();
    model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent");
    assert!(app.shown_call().is_some(), "asks again — once means once");
}

/// **Quitting names what it would throw away that is not running**: a
/// decision no turn waits on (a typed `:tool`) and the prompt queued behind
/// it. A plain `:qa` dropped both without a word (Reviewer A, round 16).
#[tokio::test]
async fn quitting_names_a_typed_decision_and_the_prompts_queued_behind_it() {
    let mut app = test_app();
    manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent");
    assert!(app.shown_call().is_some(), "premise: a typed command asks");
    app.submit("after the decision");
    assert_eq!(app.pending.len(), 1, "premise: queued behind it");

    assert!(!app.dispatch_command_for_test("qa"));
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("`rm -rf /tmp/shannon-test-absent` is waiting on your decision"),
        "{notice:?}"
    );
    assert!(
        notice.contains("a queued prompt, not yet sent"),
        "{notice:?}"
    );
    assert!(app.dispatch_command_for_test("qa!"), "`!` still goes");
}

/// Editing answers the SAME call differently: it keeps the call open and
/// bound to its id, runs nothing on its own, and does not leak the command
/// into the prompt (which would merely message the model, leaving the call
/// unanswered — the bug this replaced).
#[tokio::test]
async fn editing_keeps_the_call_open_and_bound() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    let asked = app.shown_buffer();
    assert_eq!(
        tool_key(&mut app, 'e'),
        ToolKeyEffect::Edit(asked),
        "`e` asks the caller to open $EDITOR, on the call's own session"
    );
    assert!(app.shown_call().is_some(), "the call is still pending");
    assert!(tool_result(&app).is_none(), "nothing was answered yet");
    assert!(
        app.prompt().text().trim().is_empty(),
        "the command must NOT become a chat message"
    );
}

/// The edited command is re-gated from scratch under the SAME `tool_use_id`
/// — an edit is never an inherited approval, and the answer still belongs to
/// the call the model made. Across a verdict: an ask edited into a deny is a
/// deny, and `a` runs nothing. (It edited an ask into an ask, so "re-judged"
/// asserted the verdict it already had: review stop 12, T1 6.)
#[tokio::test]
async fn an_edited_command_is_regated_under_the_same_id() {
    use shannon_core::permission::Decision;
    let mut app = test_app();
    let exec = Arc::new(shannon_core::RecordingExecutor::default());
    app.executor = exec.clone();
    model_call(&mut app, "tu_1", "echo hi > /tmp/x");
    assert_eq!(
        app.shown_call().expect("gated").decision(),
        Decision::Ask,
        "premise"
    );

    edit_shown(
        &mut app,
        "rm --no-preserve-root -rf /tmp/shannon-test-absent",
    );
    let prompt = app.shown_call().expect("still pending");
    assert_eq!(prompt.tool_use_id(), "tu_1", "still answers the same call");
    assert_eq!(
        prompt.decision(),
        Decision::Deny,
        "re-judged on its own merits"
    );
    tool_key(&mut app, 'a');
    assert!(exec.ran().is_empty(), "an allow on a deny runs nothing");
    assert!(tool_result(&app).is_none(), "and answers nothing");

    edit_shown(&mut app, "rm -rf /tmp/shannon-test-absent");
    let prompt = app.shown_call().expect("still pending");
    assert_eq!(
        prompt.command(),
        "rm -rf /tmp/shannon-test-absent",
        "the new command"
    );
    assert_eq!(prompt.decision(), Decision::Ask, "an ask again");

    // Approving now runs the EDITED command and answers the original call.
    tool_key(&mut app, 'a');
    settle(&mut app).await;
    assert!(tool_result(&app).is_some(), "answered");
    assert!(
        app.output()
            .lines()
            .join("\n")
            .contains("rm -rf /tmp/shannon-test-absent"),
        "the transcript records what actually ran"
    );
}

/// A read-only call the rules already allow runs without interrupting —
/// the gate must not nag about what policy already settled.
#[tokio::test]
async fn an_allowed_command_runs_without_a_prompt() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "echo hi");
    assert!(
        app.shown_call().is_none(),
        "no prompt for an allowed command"
    );
    settle(&mut app).await;
    let (out, is_error) = tool_result(&app).expect("it ran");
    assert!(!is_error, "{out}");
    assert!(out.contains("echo hi"), "the result names what ran: {out}");
}

/// A command the analyzer REFUSES to parse must stop at the gate (fail
/// closed), not slip through as "unanalyzed but fine".
#[tokio::test]
async fn an_unanalyzable_command_still_stops_for_a_human() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "echo $(whoami"); // unbalanced substitution
    assert!(app.shown_call().is_some(), "fail closed");
}

/// An unknown tool name is refused by name rather than guessed at.
#[tokio::test]
async fn an_unknown_tool_is_refused() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("go");
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::ToolUse {
        id: "tu_1".into(),
        name: "definitely_not_a_tool".into(),
        input: Default::default(),
    }));
    assert!(app.shown_call().is_none(), "never gated — never offered");
    let (content, is_error) = tool_result(&app).expect("the call is answered");
    assert!(is_error);
    assert!(content.contains("unknown tool"), "{content}");
}

/// A `bash` call routed through the real stream path reaches the gate.
#[tokio::test]
async fn a_bash_tool_call_from_the_stream_reaches_the_gate() {
    let mut app = test_app();
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_1",
        "rm -rf /tmp/shannon-test-absent",
    )));
    let prompt = app.shown_call().expect("gated");
    assert_eq!(prompt.command(), "rm -rf /tmp/shannon-test-absent");
    assert_eq!(prompt.tool_use_id(), "tu_1");
}

/// **A `:tool` you typed answers where you typed it.** Its owner was read
/// again when the output arrived — "where the user stands" by then — so
/// typed in one session and `:open`ed away from, its "⚙ running:" stayed
/// and its "⚙ output of:" (now `⚙ ran:`) landed in the other (Reviewer D,
/// round 17).
#[tokio::test]
async fn a_typed_tool_answers_where_it_was_typed() {
    let mut app = test_app();
    let typed_in = app.current;
    manual_tool(&mut app, "echo zz-typed");
    // A committed turn, so `:new` really leaves it (an empty session is
    // reused).
    app.conversation_mut().begin_turn("hello");
    app.conversation_mut().push_chunk("hi");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    assert_ne!(app.current, typed_in, "premise: another session in front");
    settle(&mut app).await;
    // `⚙ ran:`, then the command's block beneath it.
    let holds = |b: BufferId| {
        app.session_of(b)
            .unwrap()
            .output
            .lines()
            .windows(3)
            .any(|w| w[0] == "⚙ ran:" && w[2].contains("echo zz-typed"))
    };
    assert!(holds(typed_in), "the output went where it was typed");
    assert!(!holds(app.current), "not where the user stands now");
    let lines = app.session_of(typed_in).unwrap().output.lines();
    assert!(
        !lines.iter().any(|l| l.starts_with("⚙ running:")),
        "its running line, replaced there: {lines:#?}"
    );
}
