//! The decisions waiting: `n`/`N`, sessions, waits, close/hide/`:decide`.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **The pane stays with you through one session's calls** (Steven, 10-19:
/// "With several queued calls I think staying in the approval box is
/// fine"). Answered from the pane with the same session's call next: a deny
/// hands the pane to it at once; an allow holds the pane, focused, while the
/// command runs — saying so, its keys answering nothing — and the next call
/// fills the pane you are in. The last answer closes it.
#[tokio::test]
async fn the_decision_pane_stays_with_you_through_one_sessions_calls() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent-1");
    model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent-2");
    model_call(&mut app, "tu_3", "rm -rf /tmp/shannon-test-absent-3");
    assert_eq!(
        app.waiting_calls().len(),
        2,
        "premise: two wait behind the first"
    );
    let on_pane = |app: &App| app.shown_call().map(|p| p.tool_use_id().to_string());

    tool_key(&mut app, 'd'); // the key's event ends
    assert_eq!(app.focus(), PaneKind::Tool, "a deny: still in the pane");
    assert_eq!(
        on_pane(&app).as_deref(),
        Some("tu_2"),
        "the next call, there"
    );

    tool_key(&mut app, 'a');
    assert_eq!(
        on_pane(&app),
        None,
        "premise: tu_2 runs before tu_3 is asked"
    );
    assert_eq!(
        app.focus(),
        PaneKind::Tool,
        "an allow: the pane holds while it runs"
    );
    let (_, body) = app
        .wait_text_for_test()
        .expect("and says what it waits for");
    assert!(body.contains("rm -rf /tmp/shannon-test-absent-2"), "{body}");
    {
        let screen = text_of(&mut app, 100, 30);
        assert!(
            screen.contains("waiting for this session's next call")
                && screen.contains("running  rm -rf /tmp/shannon-test-absent-2"),
            "the pane, drawn as it waits\n{screen}"
        );
    }
    // An `a` typed into the wait answers nothing, and is not held for the call
    // that arrives next. Routed as production routes it: the wait's keys are
    // the pane's, not the keymap's (whose `a` would start an append).
    app.key_for_test(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        (80, 100),
    );
    assert_eq!(app.tool_results.len(), 1, "only tu_1's deny is answered");
    assert_eq!(app.notice_kind(), Some(NoticeKind::Refusal), "and says why");
    assert_eq!(
        app.chat_keymap.mode(),
        Mode::Normal,
        "nor did it start an append"
    );

    settle(&mut app).await; // tu_2 finishes
    app.settle();
    assert_eq!(
        on_pane(&app).as_deref(),
        Some("tu_3"),
        "the next call fills the pane"
    );
    assert_eq!(app.focus(), PaneKind::Tool, "you never left it");

    tool_key(&mut app, 'd');
    assert!(
        !app.workspace.has(PaneKind::Tool),
        "nothing next: the pane closes"
    );
    assert_eq!(app.focus(), PaneKind::Prompt);
}

/// **The calls waiting are counted** (Steven, 10-19: "how many are in the
/// background", then "1 is hidden then we say `1 queued`"). One call: no
/// count. Three: `2 queued` in the title and `n/N cycle` in the last row — and
/// nothing above the box: the stacked edge is gone until it looks like a
/// stack (Steven, 10-20, image 23: "I still want the line removed"), so the
/// pane is the size and place a lone call's is. Answered in order, the count
/// goes down and leaves with the last.
#[tokio::test]
async fn the_calls_waiting_are_counted() {
    let (w, h) = (100, 40);
    // Room for the report: a pane at `approval_max_height` would not show
    // a row more (a mutant giving the edge its row back survived there).
    let mut config = Config::default();
    config.tui.approval_max_height = 30;
    let mut app = test_app_with(config);
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent-1");
    app.settle();
    // (the decision pane's rect, its top row, its last row inside)
    let drawn = |app: &mut App| {
        let frame = frame_of(app, w, h);
        let rect = pane_rect(
            &app.workspace.frame_layout(Rect::new(0, 0, w, h)),
            PaneKind::Tool,
        );
        let buf = frame.clone();
        let row = |y: u16| -> String {
            (rect.x..rect.x + rect.width)
                .map(|x| buf[(x, y)].symbol())
                .collect()
        };
        (rect, row(rect.y), row(rect.y + rect.height - 2))
    };
    let (alone, top, legend) = drawn(&mut app);
    let content = (app.shown_call().unwrap()).content_rows(alone.width - 2);
    assert_eq!(
        alone.height - 2,
        content,
        "premise: the pane is its report's size"
    );
    assert!(
        top.starts_with('┌') && !top.contains("queued"),
        "one call: the box at the top, no count: {top:?}"
    );
    assert!(
        legend.contains("answer:") && !legend.contains("n/N cycle"),
        "{legend:?}"
    );

    model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent-2");
    model_call(&mut app, "tu_3", "rm -rf /tmp/shannon-test-absent-3");
    app.settle();
    let (queued, top, legend) = drawn(&mut app);
    // No "next …'s": the next call is this session's. (It read `!contains(
    // "another")`, which a title naming the next session by its tag passed
    // whatever session that was.)
    assert!(
        top.starts_with('┌') && top.contains("· 2 queued ") && !top.contains("next"),
        "two behind this one, this session's next: {top:?}"
    );
    assert!(legend.contains("n/N cycle"), "{legend:?}");
    assert_eq!(queued, alone, "nothing above the box: a lone call's pane");
    // And sized again while they wait — as an edit's re-gate sizes it — the
    // count takes no row of the pane's.
    app.fit_decision_pane(app.shown_buffer());
    let (refit, _, _) = drawn(&mut app);
    assert_eq!(refit, alone, "the count is the title's, not a row");

    tool_key(&mut app, 'd');
    let (_, top, _) = drawn(&mut app);
    assert!(top.contains("· 1 queued"), "one fewer: {top:?}");
    tool_key(&mut app, 'd');
    let (rect, top, legend) = drawn(&mut app);
    assert!(
        top.starts_with('┌') && !top.contains("queued"),
        "the last: {top:?}"
    );
    assert!(!legend.contains("n/N cycle"), "and `n` with it: {legend:?}");
    assert_eq!(rect, alone);
}

/// **`n` shows the next call and sets this one behind the rest** (Steven,
/// 10-20: "allow `n` for next, to cycle through the queue"). Nothing is
/// answered — no result, no record — the ring keeps its order, so `n` in
/// turn comes back to the first, and you stay in the pane while the call
/// next is this session's. The call `n` shows is a new one: an answer key
/// pressed at once waits, as for any call just shown.
#[tokio::test]
async fn n_shows_the_next_call_and_sets_this_one_behind() {
    let mut app = test_app();
    for (id, command) in [
        ("tu_1", "rm -rf /tmp/shannon-test-absent-1"),
        ("tu_2", "rm /tmp/shannon-test-absent-2"),
        ("tu_3", "rm /tmp/shannon-test-absent-3"),
    ] {
        model_call(&mut app, id, command);
    }
    app.settle();
    let shown = |app: &App| app.shown_call().map(|p| p.tool_use_id().to_string());
    let mut seen = vec![shown(&app)];
    for _ in 0..3 {
        tool_key(&mut app, 'n');
        seen.push(shown(&app));
    }
    let ring = ["tu_1", "tu_2", "tu_3", "tu_1"].map(|id| Some(id.to_string()));
    assert_eq!(seen, ring, "the next each time, and round to the first");
    assert_eq!(app.focus(), PaneKind::Tool, "this session's: you stay");
    assert!(tool_results(&app).is_empty(), "nothing answered");
    let transcript = app.output().lines().join("\n");
    assert!(
        !transcript.contains("DENIED"),
        "nothing recorded: {transcript}"
    );

    tool_key(&mut app, 'n');
    app.handle_tool_key(
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        (80, 100),
    );
    assert_eq!(
        shown(&app).as_deref(),
        Some("tu_2"),
        "an answer at once waits: the call `n` showed is new"
    );
    assert!(tool_results(&app).is_empty());
}

/// **`N` walks the ring the other way, and undoes `n`** (Steven, 10-20: "Can
/// we make `N` cycle in reverse?"): from the first of three, the LAST is
/// shown, then the one before it; `n` then gives back the call `N` left —
/// the one set aside went to the front, not the back.
#[tokio::test]
async fn shift_n_cycles_the_other_way_and_undoes_n() {
    let mut app = test_app();
    for (id, command) in [
        ("tu_1", "rm -rf /tmp/shannon-test-absent-1"),
        ("tu_2", "rm /tmp/shannon-test-absent-2"),
        ("tu_3", "rm /tmp/shannon-test-absent-3"),
    ] {
        model_call(&mut app, id, command);
    }
    app.settle();
    let shown = |app: &App| app.shown_call().map(|p| p.tool_use_id().to_string());
    let mut seen = vec![shown(&app)];
    for key in ['N', 'N', 'n', 'n'] {
        tool_key(&mut app, key);
        seen.push(shown(&app));
    }
    let walked = ["tu_1", "tu_3", "tu_2", "tu_3", "tu_1"].map(|id| Some(id.to_string()));
    assert_eq!(seen, walked, "back twice, then forward over the same calls");
    assert!(tool_results(&app).is_empty(), "nothing answered");
}

/// **`n` with nothing behind says so** — a key that does nothing reads as
/// broken — and leaves the call as it was.
#[tokio::test]
async fn n_with_nothing_queued_says_so() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent-1");
    tool_key(&mut app, 'n');
    assert_eq!(app.shown_call().map(|p| p.tool_use_id()), Some("tu_1"));
    let notice = app.notice().unwrap_or_default();
    assert!(notice.contains("no other call is waiting"), "{notice:?}");
    assert!(tool_results(&app).is_empty(), "nothing answered");
}

/// **An edit `n` sets aside comes back as the edit**, the model's own request
/// with it: allowed once it is shown again, the transcript says it replaced
/// the model's command, and so does the result the model reads. The queue
/// holds only the line, so the request waits on the call's record
/// (`ToolCall::edited_from`) — where an answered edit's waits for its
/// result — and goes back on the call shown.
#[tokio::test]
async fn an_edit_set_aside_by_n_comes_back_with_the_models_request() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "echo hi > /tmp/x");
    model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent-2");
    edit_shown(&mut app, "rm -rf /tmp/shannon-test-absent-1");
    tool_key(&mut app, 'n');
    tool_key(&mut app, 'n');
    let prompt = app.shown_call().expect("shown again");
    assert_eq!(prompt.tool_use_id(), "tu_1", "premise: round again");
    assert_eq!(
        prompt.command(),
        "rm -rf /tmp/shannon-test-absent-1",
        "the edit"
    );
    assert_eq!(
        prompt.edited_from(),
        Some("echo hi > /tmp/x"),
        "and the model's request"
    );

    tool_key(&mut app, 'a');
    settle(&mut app).await;
    let transcript = app.output().lines().join("\n");
    assert!(
        transcript.contains("edited, replaces the model's command"),
        "{transcript}"
    );
    let (result, _) = tool_result_for(&app, "tu_1").expect("answered");
    assert!(
        result.contains("you asked for: echo hi > /tmp/x"),
        "{result}"
    );
}

/// **The queue keeps its order when a call finishes behind the pane.** A
/// call that ran unasked ending while another is shown used to promote the
/// queue's head — and, the pane being taken, put it at the back: two
/// waiting were then asked out of order.
#[tokio::test]
async fn a_call_finishing_behind_the_pane_keeps_the_queue_in_order() {
    let mut app = test_app();
    for (id, command) in [
        ("tu_1", "rm -rf /tmp/shannon-test-absent-1"),
        ("tu_2", "rm /tmp/shannon-test-absent-2"),
        ("tu_3", "rm /tmp/shannon-test-absent-3"),
    ] {
        model_call(&mut app, id, command);
    }
    // A `:tool` that ran, finishing now.
    app.track_tool_call("manual-x", None);
    app.handle_exec_outcome(shannon_core::ExecOutcome {
        tool_use_id: "manual-x".to_string(),
        command: "true".to_string(),
        output: String::new(),
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
    });
    app.settle();
    tool_key(&mut app, 'd');
    assert_eq!(
        app.shown_call().map(|p| p.tool_use_id()),
        Some("tu_2"),
        "asked in the order they came"
    );
}

/// **Each session's call opens in its own pane** (`decision-panes-and-alerts.md`
/// §3; Steven, 10-22: "The decision pane should also be associated with
/// panes it is making decisions for") — B's under B's conversation, A's
/// under A's, each titled with its own session and nothing of the other's.
/// Answering A's closes A's pane and leaves you at the prompt, never in B's,
/// the next pane over (Steven, 10-19: "You should not answer an approval box
/// in one chat then teleport the cursor to the approval box of another
/// chat"; design review DR-A #6) — and B's waits as it was.
#[tokio::test]
async fn each_sessions_call_opens_in_its_own_pane() {
    let dir = TempDir::new().unwrap();
    let (mut app, a_buffer, b_buffer) = two_sessions_side_by_side(dir.path());
    model_call(&mut app, "tu_b1", "rm -rf /tmp/shannon-test-absent-1");
    app.aim_prompt_at(a_buffer);
    let typed = manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent-2");
    app.settle();
    let id = |app: &App, b| app.shown_call_in(b).map(|p| p.tool_use_id().to_string());
    assert_eq!(id(&app, b_buffer).as_deref(), Some("tu_b1"), "B's, shown");
    assert_eq!(id(&app, a_buffer), Some(typed.clone()), "A's, shown too");
    let pa = app.decision_pane_of(a_buffer).expect("A's pane");
    let pb = app.decision_pane_of(b_buffer).expect("B's pane");
    assert_ne!(pa, pb, "two panes");
    // Each under its own conversation: the column of the window showing it.
    let rects = app.workspace.rects(Rect::new(0, 0, 120, 40));
    let x = |pane| rects.iter().find(|r| r.id == pane).map(|r| r.rect.x);
    let window = |b| {
        app.panes_with_session(b, Over::ThisTab)
            .next()
            .expect("shown")
    };
    assert_eq!(x(pa), x(window(a_buffer)), "A's under A's");
    assert_eq!(x(pb), x(window(b_buffer)), "B's under B's");
    // Named, each its own — no "next" of another's (review stop 6, F 7).
    let screen = text_of(&mut app, 120, 40);
    let tag = |app: &App, b: BufferId| {
        app.session_tag(app.session_of(b).unwrap().conversation.episode().id)
    };
    for b in [a_buffer, b_buffer] {
        let title = format!("run this? · {}", tag(&app, b));
        assert!(screen.contains(&title), "{title}:\n{screen}");
    }
    assert!(!screen.contains("next "), "{screen}");

    tool_key_in(&mut app, a_buffer, 'd');

    assert_eq!(id(&app, a_buffer), None, "A's answered");
    assert_eq!(
        id(&app, b_buffer).as_deref(),
        Some("tu_b1"),
        "B's untouched"
    );
    assert_eq!(app.focus(), PaneKind::Prompt, "at the prompt, not in B's");
}

/// **`n` walks this session's calls, and only them** (Steven, 10-20) — with
/// B's call in B's pane, `n` in A's finds nothing of A's behind it and says
/// so, B's untouched; with a second of A's queued, `n` shows it in A's pane,
/// where you stand, the first set aside behind it.
#[tokio::test]
async fn n_walks_only_this_sessions_calls() {
    let dir = TempDir::new().unwrap();
    let (mut app, a_buffer, b_buffer) = two_sessions_side_by_side(dir.path());
    model_call(&mut app, "tu_b1", "rm -rf /tmp/shannon-test-absent-1");
    app.aim_prompt_at(a_buffer);
    let first = manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent-2");
    app.settle();
    let id = |app: &App, b| app.shown_call_in(b).map(|p| p.tool_use_id().to_string());

    app.notices.clear();
    tool_key_in(&mut app, a_buffer, 'n');
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("no other call is waiting"), "{said}");
    assert_eq!(id(&app, a_buffer), Some(first.clone()), "A's stays");
    assert_eq!(
        id(&app, b_buffer).as_deref(),
        Some("tu_b1"),
        "B's untouched"
    );

    let second = manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent-3");
    tool_key_in(&mut app, a_buffer, 'n');
    assert_eq!(id(&app, a_buffer), Some(second), "A's second, shown");
    let behind: Vec<&str> = app.waiting_calls().into_iter().map(|(id, _)| id).collect();
    assert_eq!(behind, [first.as_str()], "A's first behind it");
    assert_eq!(app.focused_decision(), Some(a_buffer), "where you stand");
    assert_eq!(
        id(&app, b_buffer).as_deref(),
        Some("tu_b1"),
        "B's untouched"
    );
    assert!(tool_results(&app).is_empty(), "nothing answered");
}

/// **The pane is held for the ANSWERED call's session**, not the one in
/// front: two calls typed for A while B is current — the axis every other
/// fixture here leaves equal. Answering the first holds the pane for A's
/// second.
#[tokio::test]
async fn the_held_pane_is_the_answered_calls_sessions() {
    let dir = TempDir::new().unwrap();
    let (mut app, a_buffer, b_buffer) = two_sessions_side_by_side(dir.path());
    app.aim_prompt_at(a_buffer);
    let first = manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent-1");
    manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent-2");
    assert_eq!(
        app.tool_buffer(&first),
        Some(a_buffer),
        "premise: A's calls"
    );
    assert_eq!(app.current, b_buffer, "premise: B in front");

    tool_key(&mut app, 'a');

    let (title, _) = app.wait_text_for_test().expect("held for A's next call");
    assert_eq!(app.focus(), PaneKind::Tool, "and you are still in it");
    // The call it waits for is hidden too, so counted, in the title's word
    // (review stop 6, F 7: from two, and this session's alone).
    assert!(title.contains("· 1 queued"), "{title:?}");
}

/// **Another session's call ARRIVING while a pane waits opens its own** —
/// B's pane, kept open for B's next call, is never filled by A's: A's opens
/// in A's pane, beside you, and you stay in B's, still waiting for B's
/// (full review 10-21, A 3: B's call filled A's band, under A's
/// conversation, by arriving in the wait).
#[tokio::test]
async fn another_sessions_call_arriving_in_the_wait_opens_its_own_pane() {
    let dir = TempDir::new().unwrap();
    let (mut app, a_buffer, b_buffer) = two_sessions_side_by_side(dir.path());
    model_call(&mut app, "tu_b1", "rm -rf /tmp/shannon-test-absent-1");
    model_call(&mut app, "tu_b2", "rm -rf /tmp/shannon-test-absent-2");
    tool_key_in(&mut app, b_buffer, 'a');
    assert!(
        app.wait_text_for_test().is_some(),
        "premise: B's pane waits for B's next"
    );
    assert_eq!(app.focused_decision(), Some(b_buffer), "premise: in it");

    app.aim_prompt_at(a_buffer);
    let typed = manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent-3");
    app.settle();

    assert_eq!(
        app.shown_call_in(a_buffer).map(|p| p.tool_use_id()),
        Some(typed.as_str()),
        "A's call, in A's pane"
    );
    assert_ne!(
        app.decision_pane_of(a_buffer),
        app.decision_pane_of(b_buffer),
        "not B's"
    );
    assert!(
        app.wait_text_for_test().is_some(),
        "B's still waits for B's"
    );
    assert_eq!(app.focused_decision(), Some(b_buffer), "and you stay in it");
}

/// **The wait lets go when what it waited for is gone** — the turn stopped:
/// its command killed, its queued call withdrawn. Nothing will fill the pane,
/// so it closes and you are back at the prompt. `q` leaves a wait at once,
/// the next call then opening beside you; `:close` forgets it.
#[tokio::test]
async fn a_wait_with_nothing_coming_closes_and_q_leaves_one() {
    for how in ["stop", "q", "close"] {
        let mut app = test_app();
        model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent-1");
        model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent-2");
        tool_key(&mut app, 'a');
        assert!(
            app.wait_text_for_test().is_some(),
            "premise: the pane waits"
        );

        match how {
            // From the prompt: the waiting pane, where `a` left the focus,
            // stops no work (Steven, 10-21; full review A 13) and says how
            // to leave it. Stopped from the prompt, it closes all the same,
            // nothing coming to fill it.
            "stop" => {
                assert_eq!(app.focus(), PaneKind::Tool, "premise: in the wait");
                app.interrupt();
                let said = app.notice().unwrap_or_default();
                assert!(said.contains("`q` leaves this pane"), "{said}");
                assert_eq!(app.waiting_calls().len(), 1, "nothing withdrawn");
                app.set_focus(PaneKind::Prompt);
                app.ctrl_c_stop_for_test();
                app.settle();
                assert!(app.waiting_calls().is_empty(), "premise: tu_2 withdrawn");
                assert!(!app.workspace.has(PaneKind::Tool), "nothing will fill it");
                assert_eq!(app.focus(), PaneKind::Prompt);
            }
            "q" => {
                app.key_for_test(
                    KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
                    (80, 100),
                );
                assert!(!app.workspace.has(PaneKind::Tool), "`q` leaves the wait");
                assert_eq!(app.focus(), PaneKind::Prompt);
                settle(&mut app).await; // tu_1 finishes
                app.settle();
                assert!(app.shown_call().is_some(), "tu_2 is asked");
                assert_ne!(app.focus(), PaneKind::Tool, "beside you");
            }
            _ => {
                app.close_pane_command(Asked::Plain);
                app.settle();
                assert!(
                    app.wait_text_for_test().is_none(),
                    "a closed pane holds nothing"
                );
            }
        }
    }
}

/// **Closing a pane must not orphan the decision it was showing.**
///
/// `tool_prompt` had one creation door and four destruction doors; two had
/// never heard of it. Closing the decision pane left it alive with no pane
/// to answer it in — unresolvable, and `tool_turn_open` wedged the turn so
/// every later prompt queued in silence. (Reviewers E and F, independently.)
///
/// **Every plain door that closes the pane itself denies** — `:close`, `:q`
/// (`decision-panes-and-alerts.md` §3.6; `<C-w>c` and `<C-w>q` reach the
/// same `close_pane_command`): the one close that answers, made where the
/// question is (Steven, 10-26). `:hide` and `:q!` keep it
/// ([`hide_keeps_the_decision_until_decide`]); a close made anywhere else
/// answers nothing ([`a_chat_a_request_sits_under_closes_only_by_force`]).
#[tokio::test]
async fn closing_the_decision_pane_denies_rather_than_orphaning_it() {
    // The keys driven too: named and not pressed (review stop 12, T5 F9).
    for close_via in [":close", ":q", "<C-w>c", "<C-w>q"] {
        let mut app = test_app();
        model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
        assert!(app.shown_call().is_some());
        to_decision(&mut app);

        match close_via.strip_prefix(':') {
            Some(command) => _ = app.dispatch_command_for_test(command),
            None => type_keys(&mut app, close_via),
        }

        assert!(
            app.shown_call().is_none(),
            "{close_via}: the decision must not outlive its pane"
        );
        assert!(
            !app.awaiting_decision(),
            "{close_via}: and the turn is not parked on an answer that can \
             never come — the wedge that queued every later prompt in silence"
        );
        let said = app.notice().unwrap_or_default();
        assert!(
            said.contains("DENIED"),
            "{close_via}: and it says so: {said}"
        );
    }
}

/// **The chat a request's pane sits under closes only by force** (Steven,
/// 10-26: "the request is tied to a chat, so let's make sure to not close
/// the associated chat too") — `:close`, `:q`, `<C-w>c` and `<C-w>q` on it
/// are refused, naming whose, and nothing closes; `:hide` and `:q!` close
/// it and its band, answering nothing: the request waits, hidden — its
/// session shown in another tab, so hidden is what "no pane" means
/// ([`waits_hidden`]) — and `:decide` brings it back.
#[tokio::test]
async fn a_chat_a_request_sits_under_closes_only_by_force() {
    let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
    let size = (80, 30);
    let asked = |app: &mut App, kind: Request| {
        let chat = kind.ask(app);
        let pane = app.decision_pane_of(chat).expect("premise: its pane");
        let host = (app.window_of(pane).and_then(Window::decides)).map(|(_, host)| host);
        let host = host.expect("premise: under a window");
        app.focus_pane(host);
        app.settle();
        (chat, host)
    };
    for (kind, via) in (Request::BOTH.into_iter())
        .flat_map(|kind| [":close", ":q", "<C-w>c", "<C-w>q"].map(|via| (kind, via)))
    {
        let mut app = test_app();
        let (chat, _) = asked(&mut app, kind);
        let before = app.workspace.pane_ids();
        app.notices.clear(); // the review's arrival, said

        match via.strip_prefix(':') {
            Some(command) => {
                app.dispatch_command_for_test(command);
            }
            None => {
                app.key_for_test(ctrl_w, size);
                app.key_for_test(key(via.chars().last().unwrap()), size);
            }
        }
        app.settle();
        assert_eq!(
            app.workspace.pane_ids(),
            before,
            "{kind:?} {via}: nothing closed"
        );
        assert!(
            app.decisions.on_pane(chat).is_some(),
            "{kind:?} {via}: kept"
        );
        assert!(
            tool_results(&app).is_empty(),
            "{kind:?} {via}: nothing answered"
        );
        assert_eq!(
            app.notice().unwrap_or_default(),
            format!(
                "not closed — `:hide` closes anyway (it waits, hidden); b{}'s {} waits in its \
                 pane",
                chat.number(),
                kind.named()
            ),
            "{kind:?} {via}: said, whose"
        );
    }
    for (kind, via) in
        (Request::BOTH.into_iter()).flat_map(|kind| ["hide", "q!"].map(|v| (kind, v)))
    {
        let mut app = test_app();
        let (chat, host) = asked(&mut app, kind);
        // In view in another tab: no twin here (`Over::ThisTab`), and the
        // session shown, so a pane not opened there again is hidden.
        app.dispatch_command_for_test(&format!("tabnew {}", chat.number()));
        app.dispatch_command_for_test("tabnext 1");
        app.focus_pane(host);
        app.settle();
        app.dispatch_command_for_test(via);
        assert!(
            !app.workspace.pane_ids().contains(&host),
            "{via}: the chat closed"
        );
        waits_hidden(&app, chat);
        assert!(tool_results(&app).is_empty(), "{via}: nothing answered");
        if kind == Request::Call {
            assert!(app.awaiting_decision(), "{via}: its turn waits on it");
        }
        app.dispatch_command_for_test(&format!("decide {}", chat.number()));
        assert!(
            app.decision_pane_of(chat).is_some(),
            "{via}: `:decide` brings it back"
        );
    }
}

/// **A session whose chat closed by force over its request waits on you,
/// said so** — not "still answering": its turn is held on the request just
/// hidden (review stop 11, F 5).
#[tokio::test]
async fn a_forced_chat_close_says_its_session_waits_on_you() {
    let mut app = test_app();
    let chat = app.current;
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    app.settle();
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    let host = (app.window_of(pane).and_then(Window::decides))
        .map(|(_, h)| h)
        .unwrap();
    app.focus_pane(host);
    app.notices.clear();
    app.dispatch_command_for_test("q!");
    let n = chat.number();
    assert!(
        (app.notices.iter()).any(|(_, s)| s.contains("waits on your decision, in no pane")
            && s.contains(&format!("`:decide {n}`"))),
        "{:?}",
        app.notices
    );
    assert!(
        !(app.notices.iter()).any(|(_, s)| s.contains("still answering")),
        "{:?}",
        app.notices
    );
}

/// **A chat with a twin — a second window on its session — closes, its
/// request's pane going under the twin** (review stop 11, F 6; my call): the
/// chat the request is decided by stays on screen. Refused, a duplicate
/// view read as "the chat"; forced, the request hid with its session still
/// in view. `:q` on the host, and `:only` from the twin, alike: nothing
/// answered, nothing hidden, the pane under the window left.
#[tokio::test]
async fn a_chat_with_a_twin_closes_and_its_request_moves_under_the_twin() {
    for (kind, via) in (Request::BOTH.into_iter()).flat_map(|k| ["q", "only"].map(|v| (k, v))) {
        let mut app = test_app();
        let chat = kind.ask(&mut app);
        let pane = app.decision_pane_of(chat).expect("premise: its pane");
        let host = (app.window_of(pane).and_then(Window::decides))
            .map(|(_, h)| h)
            .unwrap();
        app.focus_pane(host);
        app.split_command("", SplitDir::Vertical);
        app.settle();
        let twin = app.workspace.focused();
        assert_ne!(twin, host, "premise: a second window on it");
        match via {
            "q" => app.focus_pane(host),
            _ => app.focus_pane(twin),
        }
        app.notices.clear();
        app.dispatch_command_for_test(via);
        assert!(
            !app.workspace.pane_ids().contains(&host),
            "{kind:?} {via}: the host closed: {:?}",
            app.notice()
        );
        assert!(
            app.decisions.on_pane(chat).is_some(),
            "{kind:?} {via}: kept"
        );
        assert!(
            tool_results(&app).is_empty(),
            "{kind:?} {via}: nothing answered"
        );
        let now = app.decision_pane_of(chat).expect("a pane for it still");
        let under = (app.window_of(now).and_then(Window::decides)).map(|(_, h)| h);
        assert_eq!(under, Some(twin), "{kind:?} {via}: under the twin");
        assert!(
            !(app.notices.iter()).any(|(_, said)| said.contains("hidden")),
            "{kind:?} {via}: not hidden: {:?}",
            app.notices
        );
    }
}

/// **A pane kept open for a next call holds no request** — the call
/// allowed and running, the next queued behind it: the chat it sits under
/// closes, and `:only` elsewhere closes the pane, unforced, as nothing on it
/// is a close's to take (`App::requests_in`). The call behind still waits,
/// not hidden: no close hid it (the reconcile asks what was ON the pane).
#[tokio::test]
async fn a_pane_kept_for_a_next_call_does_not_hold_its_chat() {
    for via in ["close", "only"] {
        let mut app = test_app();
        let chat = app.current;
        manual_tool(&mut app, "rm /tmp/shannon-test-absent-1");
        manual_tool(&mut app, "rm /tmp/shannon-test-absent-2");
        app.settle();
        tool_key_in(&mut app, chat, 'a');
        let pane = app.decision_pane_of(chat).expect("premise: kept open");
        assert!(app.on_pane_in(pane).is_none(), "premise: nothing on it");
        let host = (app.window_of(pane).and_then(Window::decides)).map(|(_, host)| host);
        let host = host.expect("premise: under a window");
        match via {
            "close" => app.focus_pane(host),
            _ => app.set_focus(PaneKind::Prompt),
        }
        app.dispatch_command_for_test(via);
        assert!(
            !(app.workspace.pane_ids()).contains(&host),
            "{via}: closed: {:?}",
            app.notice()
        );
        assert!(
            app.decisions.waiting_of(chat).next().is_some(),
            "{via}: the call behind still waits"
        );
        assert!(
            !(app.notices.iter()).any(|(_, said)| said.contains("hidden")),
            "{via}: and nothing hid it: {:?}",
            app.notices
        );
        // Let go with its pane, in the event that closed it — not at the
        // next one's end, which a second settle stood in for (review stop
        // 13, C 2).
        assert!(!app.decisions.kept_open(chat), "{via}: no longer held open");
    }
}

/// **`:hide` keeps the decision, hidden, until `:decide`** (Steven, 10-22:
/// "`:q` removes from the buffer while hide doesn't") — vim's `:hide`, which
/// keeps a buffer `:q` abandons. The pane goes and nothing is answered; the
/// turn waits on it; it is said, at the row's right in the shipped row; the
/// pane does not come back at the event's end, though its conversation is
/// still on screen, nor when the session is shown again; `:decide` brings
/// it back, into its pane, where it answers. And the way out still takes it.
///
/// **`:q!` in the pane is the same close** (Steven, 10-26: "close as
/// unanswered. `:q!` denying seems non-obvious to me"): it denied, as `:q`
/// there does. `:wq!` too, its write done first.
#[tokio::test]
async fn hide_keeps_the_decision_until_decide() {
    for via in ["hide", "q!", "wq!"] {
        let mut app = test_app();
        let chat = app.current;
        model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
        to_decision(&mut app);
        app.dispatch_command_for_test(via);

        assert!(app.shown_call().is_some(), "{via}: the call is kept");
        assert!(tool_results(&app).is_empty(), "{via}: nothing answered");
        assert!(app.awaiting_decision(), "{via}: its turn waits on it");
        assert_eq!(app.decision_pane_of(chat), None, "{via}: with no pane");
        assert!(
            app.panes_with_session(chat, Over::ThisTab).next().is_some(),
            "{via}: premise: its conversation still on screen"
        );
        let said = app.notice().unwrap_or_default();
        assert!(said.contains("decision hidden"), "{via}: said: {said}");
        assert!(
            said.contains("`:decide 1`"),
            "{via}: with what brings it: {said}"
        );
        app.new_moment();
        let row = screen(&mut app, 100).lines().last().unwrap().to_string();
        assert!(
            row.trim_end().ends_with("decision hidden · `:decide 1`"),
            "{via}: at the shipped row's right: {row:?}"
        );

        // Off screen and shown again by `:b` — still hidden: you hid the
        // decision, not its session, which brings back one it took with it.
        app.new_session_command();
        app.settle();
        assert!(
            app.panes_with_session(chat, Over::ThisTab).next().is_none(),
            "{via}: premise: off screen"
        );
        app.dispatch_command_for_test("b 1");
        assert!(
            app.panes_with_session(chat, Over::ThisTab).next().is_some(),
            "{via}: premise: shown again"
        );
        assert_eq!(app.decision_pane_of(chat), None, "{via}: still hidden");

        app.dispatch_command_for_test("decide");
        let pane = app
            .decision_pane_of(chat)
            .expect("`:decide` brings it back");
        assert_eq!(app.workspace.focused(), pane, "{via}: and stands you in it");
        app.settle();
        assert_eq!(
            app.decision_pane_of(chat),
            Some(pane),
            "{via}: and it stays"
        );
        let row = screen(&mut app, 100).lines().last().unwrap().to_string();
        assert!(!row.contains("hidden"), "{via}: hidden no more: {row:?}");
        tool_key_in(&mut app, chat, 'd');
        assert!(app.shown_call().is_none(), "{via}: answered there");
    }

    // Hidden, then quit: the way out takes a call with no pane too — a
    // typed one, which no turn's stop withdraws, as a model's call is.
    let mut app = test_app();
    manual_tool(&mut app, "rm /tmp/shannon-test-absent");
    to_decision(&mut app);
    app.dispatch_command_for_test("hide");
    assert!(app.shown_call().is_some(), "premise: hidden, still up");
    assert!(app.leaving(true), "premise: it leaves");
    assert!(app.shown_call().is_none(), "nothing left shown");
}

/// **`:tool` shows a hidden decision of its session** — its call waits
/// behind the one hidden, and with that one hidden it would wait unasked.
#[tokio::test]
async fn a_typed_tool_brings_back_its_sessions_hidden_decision() {
    let mut app = test_app();
    let chat = app.current;
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app);
    app.dispatch_command_for_test("hide");
    assert_eq!(app.decision_pane_of(chat), None, "premise: hidden");
    manual_tool(&mut app, "rm /tmp/shannon-test-absent-2");
    app.settle();
    assert!(app.decision_pane_of(chat).is_some(), "shown again");
}

/// **A hidden session's next call opens no pane** — its first withdrawn
/// (Claude Code stopped waiting), the next of its calls is shown hidden, as
/// the first was: no pane, not even until the event's end, where placement
/// would close it again — a transcript split and given back for nothing
/// (`fit_decision_pane`). Taken off to be shown, the last waiting call left
/// the session holding nothing, and its hiding was forgotten with it.
///
/// And once all are answered, hidden no more: a call that comes after
/// opens its pane.
#[tokio::test]
async fn a_hidden_sessions_next_call_opens_no_pane() {
    let (mut app, _provider) = provider_app();
    let chat = app.current;
    let rm = |n: u8| serde_json::json!({ "command": format!("rm /tmp/shannon-test-absent-{n}") });
    provider_asks(&mut app, "tu_1", "Bash", rm(1));
    provider_asks(&mut app, "tu_2", "Bash", rm(2));
    to_decision(&mut app);
    app.dispatch_command_for_test("hide");
    let turn = app.foreground_id().expect("premise: its turn");
    app.dispatch_stream_msg(turn, StreamMsg::ToolWithdrawn(question("tu_1")));
    assert_eq!(
        app.shown_call().map(|p| p.tool_use_id()),
        Some("tu_2"),
        "premise: the next shown"
    );
    assert!(
        !app.workspace.has(PaneKind::Tool),
        "with no pane, even before the event's end"
    );
    app.settle();
    assert_eq!(app.decision_pane_of(chat), None, "nor after it");

    app.feed_stream(StreamMsg::ToolWithdrawn(question("tu_2")));
    assert!(app.shown_call().is_none(), "premise: nothing left");
    provider_asks(&mut app, "tu_3", "Bash", rm(3));
    assert!(
        app.decision_pane_of(chat).is_some(),
        "a call after them all opens its pane"
    );
}

/// **`:only` keeps the window a decision sits under, with it** (§3.6;
/// design review DR-A #8) — from the conversation the pane stays, and from
/// the pane its conversation does: `:only` is the no-room advice, and
/// closing the pane with the rest denied the call it was advice for. From
/// a window that is not the conversation's — the prompt — `:only` is
/// refused, naming whose request it would take (Steven, 10-26), and
/// `:only!` closes the conversation, its call waiting, hidden, said on the
/// row: a window closed, not the question.
#[tokio::test]
async fn only_keeps_the_window_a_decision_sits_under() {
    let froms = ["conversation", "pane", "prompt"];
    for (kind, from) in (Request::BOTH.into_iter()).flat_map(|k| froms.map(|f| (k, f))) {
        let mut app = test_app();
        let chat = kind.ask(&mut app);
        let pane = app.decision_pane_of(chat).expect("premise: its pane");
        let window = app
            .window_of(pane)
            .and_then(Window::decides)
            .map(|(_, host)| host);
        let window = window.expect("premise: under a window");
        // A second window on it, for `:only` to close.
        app.focus_pane(window);
        app.split_command("", SplitDir::Vertical);
        match from {
            "conversation" => app.focus_pane(window),
            "pane" => app.focus_pane(pane),
            _ => app.set_focus(PaneKind::Prompt),
        }
        if from == "prompt" {
            let before = app.workspace.pane_ids();
            app.dispatch_command_for_test("only");
            assert_eq!(app.workspace.pane_ids(), before, "{kind:?} {from}: refused");
            assert_eq!(
                app.notice().unwrap_or_default(),
                format!(
                    "`:only`: not closed — `:only!` closes anyway (it waits, hidden); b{}'s {} \
                     waits in its pane",
                    chat.number(),
                    kind.named()
                ),
            );
            app.dispatch_command_for_test("only!");
        } else {
            app.dispatch_command_for_test("only");
        }
        app.settle();

        assert!(
            app.decisions.on_pane(chat).is_some(),
            "{kind:?} {from}: nothing answered"
        );
        assert!(tool_results(&app).is_empty(), "{kind:?} {from}: nor denied");
        if from == "prompt" {
            assert_eq!(
                app.decision_pane_of(chat),
                None,
                "{from}: no window, no pane"
            );
            let row = status_row(&mut app, "prompt", "$hidden");
            assert!(
                row.contains("decision hidden · `:decide 1`"),
                "{from}: said: {row}"
            );
        } else {
            assert_eq!(
                app.decision_pane_of(chat),
                Some(pane),
                "{from}: the same pane"
            );
            let live = app.workspace.pane_ids();
            assert!(live.contains(&window), "{from}: under its window");
            assert_eq!(live.len(), 3, "{from}: and only those, with the prompt");
        }
    }
}
