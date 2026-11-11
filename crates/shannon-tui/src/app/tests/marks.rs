//! Pane marks and `:Send`: one question, several sessions.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// ---- P5: marks + :Send — one question, several sessions -------------------

/// Two marked panes, one prompt, two sessions answering — as **jobs**, so no
/// turn starts (while one turn ran app-wide, that kept its slot free) and
/// both replies stream where you can see them (`buffers.md` §4 settled this
/// before it was built).
#[tokio::test]
async fn a_marked_prompt_sends_to_every_marked_session() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let first = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let second = app.workspace.focused();
    assert_ne!(
        app.pane_session(first).id,
        app.pane_session(second).id,
        "premise: two sessions"
    );
    app.workspace.toggle_mark(first);
    app.workspace.toggle_mark(second);

    app.submit("same question");

    assert_eq!(
        app.job_episodes().len(),
        2,
        "two background turns, not one foreground"
    );
    assert!(app.turns().next().is_none(), "no turn: every send is a job");
    for pane in [first, second] {
        assert!(
            app.pane_session(pane).conversation.turn_in_progress(),
            "each marked session began a turn"
        );
    }
}

/// Two panes on ONE session get ONE send, not two — a mark names a pane, but a
/// turn belongs to a session, and two requests writing one conversation is the
/// double-booking the job layer refuses everywhere else.
#[tokio::test]
async fn two_panes_on_one_session_are_sent_to_once() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let first = app.workspace.focused();
    app.split_command("", SplitDir::Vertical); // same session, inherited
    let second = app.workspace.focused();
    assert_eq!(app.pane_session(first).id, app.pane_session(second).id);
    app.workspace.toggle_mark(first);
    app.workspace.toggle_mark(second);

    app.submit("once please");

    assert_eq!(app.job_episodes().len(), 1, "one session, one turn");
    // And no refusal: the per-episode guard would ALSO hold this to one turn,
    // but by refusing the second send — reporting the first send as what
    // holds the session, for something the user did not do wrong. The dedup
    // is what makes the message honest, so the message is what pins it. (It
    // asserted "already running" absent, which went vacuous when the refusal
    // took the holder's words, "… is running in this session" — D 6.)
    let notice = app.notice().expect("it says what happened");
    assert!(
        !notice.contains("is running in this session") && !notice.contains("nothing sent"),
        "marking one session twice is not an error: {notice}"
    );
}

/// `:Send 1,2` names panes by the numbers already on screen, and marks them so
/// the set stays visible and repeatable.
#[tokio::test]
async fn send_names_panes_by_their_screen_numbers() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    app.prompt_mut().insert_pasted("compare this");

    app.send_command("1,2");

    assert_eq!(app.job_episodes().len(), 2, "both numbered panes got it");
    assert_eq!(
        app.workspace.marked_panes().len(),
        2,
        "and the set is left marked, so you can see what you just did"
    );
    assert!(app.prompt().text().is_empty(), "the prompt was consumed");
}

/// **`:Send all` sends to every transcript pane** — lost in the rebuild
/// (Steven, 2026-09-29) — and marks them, so a bare `:Send` repeats it.
#[tokio::test]
async fn send_all_goes_to_every_transcript_pane() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    app.prompt_mut().insert_pasted("compare this");

    app.dispatch_command_for_test("Send all");

    assert_eq!(app.job_episodes().len(), 2, "both transcripts got it");
    assert_eq!(app.workspace.marked_panes().len(), 2, "and both are marked");
    assert!(app.prompt().text().is_empty(), "the prompt was consumed");
}

/// A pane number that names no transcript refuses visibly rather than sending
/// somewhere surprising.
#[test]
fn send_refuses_a_number_that_is_not_a_transcript() {
    let mut app = test_app();
    app.prompt_mut().insert_pasted("hi");
    app.send_command("99");
    assert!(
        app.notice().expect("a refusal").contains("no pane 99"),
        "says which number was wrong"
    );
    assert!(app.job_episodes().is_empty(), "and nothing was sent");
}

/// Marks are pane-bound, so closing a pane takes its mark with it — no set that
/// outlives what it named.
#[test]
fn a_closed_pane_takes_its_mark_with_it() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    let doomed = app.workspace.focused();
    app.workspace.toggle_mark(doomed);
    assert_eq!(app.workspace.marked_panes().len(), 1);

    app.close_pane_command(Asked::Plain);

    assert!(
        app.workspace.marked_panes().is_empty(),
        "the mark went with the pane"
    );
}

/// Two windows on ONE session scroll independently.
///
/// The viewport lived on `Output` — per buffer — so paging one paged both, and
/// `j`/`k` in one dragged its twin through `follow_cursor`. P2 moved the cursor
/// to the pane and stopped there, leaving two windows on one session with a
/// single degree of freedom; that is what made a split read as "welded"
/// (Reviewer F). `buffers.md` §2 always said a window owns cursor AND scroll.
#[test]
fn two_windows_on_one_session_scroll_independently() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 12); // long enough to scroll
    let long = app.conversation().episode().id;
    app.reset_foreground(); // let go, so `:open` replays a real transcript
    app.set_focus(PaneKind::Output);
    let first = app.workspace.focused();
    app.open_by_hash(&long.to_hex()[..8]);
    app.split_command("", SplitDir::Vertical); // same session, inherited
    let second = app.workspace.focused();
    assert_eq!(
        app.pane_session(first).id,
        app.pane_session(second).id,
        "premise: one session, two windows"
    );
    assert!(app.pane_viewport(first).is_following_tail());
    assert!(app.pane_viewport(second).is_following_tail());
    assert!(
        app.pane_session(first).output.line_count() > 10,
        "premise: there is something to scroll ({} lines)",
        app.pane_session(first).output.line_count()
    );

    // Scroll the FIRST pane back through its history. It is NOT focused, so
    // the wheel moves its viewport rather than its cursor.
    app.wheel_scroll_at(first, true, (80, 20));

    assert!(
        !app.pane_viewport(first).is_following_tail(),
        "the pane we scrolled left the tail"
    );
    assert!(
        app.pane_viewport(second).is_following_tail(),
        "and its twin stayed where it was"
    );
}

/// `:Send` into a session that has never been written must REGISTER it, not
/// append to a header that isn't there. Every session starts unpersisted, so
/// this is the ordinary case, not an edge: the two-pane compare workflow
/// rendered both replies and lost them on quit. (Reviewer A, CRITICAL.)
#[tokio::test]
async fn a_marked_send_persists_a_session_that_was_never_written() {
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.workspace.toggle_mark(pane);
    let episode = app.pane_session(pane).conversation.episode().id;
    assert_eq!(
        JsonlStore::new(dir.path())
            .unwrap()
            .episode_len(&episode)
            .unwrap(),
        None,
        "premise: never written"
    );

    app.submit("compare this");
    let job = app.job_episodes()[0];
    app.feed_job(job, StreamMsg::Chunk("an answer".into()));
    app.feed_job(job, StreamMsg::Complete);

    assert!(
        app.notice().is_none_or(|n| !n.contains("NOT saved")),
        "no persistence failure: {:?}",
        app.notice()
    );
    let loaded = app.store.load_episode(&episode);
    assert!(loaded.is_ok(), "the session reached disk: {loaded:?}");
}

/// CTRL-C with `:Send` replies streaming must STOP them, not quit. With marks
/// set the foreground is never used, so asking only about it found "nothing to
/// stop" and fell through to quitting — dropping every reply unsealed while the
/// status row said they were running. (Reviewer A, HIGH.)
#[tokio::test]
async fn ctrl_c_stops_marked_sends_instead_of_quitting() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let first = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let second = app.workspace.focused();
    app.workspace.toggle_mark(first);
    app.workspace.toggle_mark(second);
    app.submit("same question");
    assert_eq!(app.job_episodes().len(), 2, "premise: two running sends");
    assert!(
        app.turns().next().is_none(),
        "premise: nothing in the foreground"
    );

    // Pressed, not through the seam: the key's own reach is what is under
    // test (full review 10-21, C 1).
    app.notices.clear();
    app.interrupt();

    assert!(app.job_episodes().is_empty(), "both were stopped");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("stopped") && !said.contains(":qa"),
        "there was something to stop — so CTRL-C does not answer how to leave: {said}"
    );
}

/// `:Send 1,1` marks that pane once rather than toggling it back off.
#[tokio::test]
async fn send_with_a_repeated_pane_number_still_marks_it() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.prompt_mut().insert_pasted("hi");

    app.send_command("1,1");

    assert_eq!(
        app.workspace.marked_panes().len(),
        1,
        "named twice, marked once"
    );
    assert_eq!(app.job_episodes().len(), 1, "and it was sent to");
}

/// The prompt survives when every marked session refuses — the unmarked path
/// queues the text, so destroying it here was the odd one out.
#[tokio::test]
async fn a_refused_marked_send_keeps_your_text() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let buffer = app.pane_session(pane).id;
    app.start_job_on(buffer, "already running").expect("a job");
    app.workspace.toggle_mark(pane);
    app.prompt_mut().insert_pasted("do not lose me");

    app.send_command("");

    assert!(
        app.prompt().text().contains("do not lose me"),
        "the text is still there to retry: {:?}",
        app.prompt().text()
    );
}

/// A `:Send` masks a secret the same way the foreground does — and this is the
/// test that holds it there. The substitution was correct and **nothing pinned
/// it**: swapping `begin_turn(&outbound)` for `begin_turn(text)`, i.e. sending
/// the cleartext to N sessions and to disk, left the whole suite green
/// (Reviewer E, mutation-verified both directions).
#[tokio::test]
async fn a_marked_send_masks_secrets_like_the_foreground_does() {
    let (mut app, handle) = app_with_secret("token", "hunter2beornot2be");
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.workspace.toggle_mark(pane);

    app.submit("my password is hunter2beornot2be");

    let sent = app
        .pane_session(pane)
        .conversation
        .messages()
        .last()
        .unwrap()
        .content
        .clone();
    assert!(
        !sent.contains("hunter2beornot2be"),
        "the value must not reach the wire or the log: {sent}"
    );
    assert!(sent.contains(&handle), "it travels as its handle: {sent}");
    assert!(
        app.notice().is_some_and(|n| n.contains("withheld")),
        "and the user is told: {:?}",
        app.notice()
    );
}

/// **A resumed round's request really does say a send is attended.**
///
/// Asserted on the `SendRequest` the backend receives, not on the predicate
/// that decides it — see [`RecordingBackend`].
#[tokio::test]
async fn a_resumed_send_sends_the_attended_description() {
    let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let backend = std::sync::Arc::new(RecordingBackend { sent: sent.clone() });
    let mut app = test_app_backed(
        backend,
        std::sync::Arc::new(shannon_core::Config::default()),
    );

    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    for p in panes {
        app.workspace.toggle_mark(p);
    }
    app.submit("do the thing");
    let send = app.generation_ids_for_test()[0];

    // Round one: a tool call, answered. Then the round resumes — and THAT is
    // the request under test.
    app.gate_job_tool_call_for_test(send, "mkdir build");
    assert!(app.shown_call().is_some(), "premise: it asked");
    let before = sent.lock().unwrap().len();
    // DENY — a denial is still an answer, so the round resumes, and nothing
    // runs on the machine this test is running on.
    app.resolve_shown_for_test(crate::permission_view::ToolAction::Deny);
    let requests = sent.lock().unwrap();
    assert!(
        requests.len() > before,
        "premise: the resume actually sent a request"
    );
    let description = &requests.last().unwrap().tools[0].description;
    assert!(
        !description.contains("NOBODY IS WATCHING"),
        "the resumed round of a send you are watching must not say otherwise: {description}"
    );
}

/// **A tool call is alias-expanded exactly ONCE, whichever gate it goes
/// through.**
///
/// `expand_for` is a single left-to-right pass, so a value that itself contains
/// handle syntax comes out literal — deliberately, since one pass is what the
/// user can predict. `gate_job_tool_call` resolved before forking on
/// `attended()`, and `gate_tool_call` resolves again, so an attended `:Send`
/// got two passes: the analyzer judged, and the shell would have run, text that
/// no single expansion produces. Not a bypass — the two gates simply disagreed
/// about what "the command" was, which is worse than either answer because
/// neither is the one on screen.
///
/// The fix was moving the fork above the resolution (`A4`), so this is the
/// assertion that keeps it moved: it fails if either gate starts resolving
/// again on the other's behalf.
#[tokio::test]
async fn an_attended_sends_tool_call_is_expanded_once_not_twice() {
    let mut cfg = Config::default();
    // `outer` expands to text that LOOKS like another handle. One pass leaves
    // it alone; two passes resolve it.
    cfg.alias = vec![
        shannon_core::config::AliasSpec {
            name: "outer".into(),
            value: Some("{{a:inner}}".into()),
            env: None,
            sensitive: Some(false),
        },
        shannon_core::config::AliasSpec {
            name: "inner".into(),
            value: Some("SECOND-PASS".into()),
            env: None,
            sensitive: Some(false),
        },
    ];
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    for p in panes {
        app.workspace.toggle_mark(p);
    }
    app.submit("do the thing");
    let id = app.generation_ids_for_test()[0];
    app.gate_job_tool_call_for_test(id, "mkdir {{a:outer}}");

    let shown = app
        .shown_call()
        .map(|p| p.command().to_string())
        .expect("an attended send asks, so there is a command on screen");
    assert!(
        shown.contains("{{a:inner}}"),
        "one pass: the expansion of `outer` is literal text, not a handle to \
         expand again — got {shown:?}"
    );
    assert!(
        !shown.contains("SECOND-PASS"),
        "two passes resolved a handle the user never wrote, so what the \
         analyzer judged is not what they typed: {shown:?}"
    );
}
