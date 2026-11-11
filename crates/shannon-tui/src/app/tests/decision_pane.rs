//! The decision pane: layout, focus, keys, room, and what it says.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// The decision pane is a PANE, not a takeover: the transcript keeps its own
/// rect (so the conversation stays on screen and scrollable) and the prompt
/// keeps its full configured size — the decision borrows rows from the
/// transcript, never from what the user types into.
#[tokio::test]
async fn the_decision_pane_shares_the_frame_with_chat_and_prompt() {
    let frame = Rect::new(0, 0, 80, 40);
    let mut app = test_app();
    let before = app.workspace.frame_layout(frame);

    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    let after = app.workspace.frame_layout(frame);

    let tool = maybe_pane_rect(&after, PaneKind::Tool).expect("the decision pane has a rect");
    assert!(tool.height > 0 && tool.width > 0, "it is actually drawn");
    assert!(
        pane_rect(&after, PaneKind::Output).height > 0,
        "the transcript is still on screen"
    );
    assert!(
        pane_rect(&after, PaneKind::Output).height < pane_rect(&before, PaneKind::Output).height,
        "the rows came out of the transcript"
    );
    assert_eq!(
        pane_rect(&after, PaneKind::Prompt).height,
        pane_rect(&before, PaneKind::Prompt).height,
        "never out of the prompt the user sized"
    );
    // Stacked between transcript and prompt.
    assert!(
        pane_rect(&after, PaneKind::Output).y + pane_rect(&after, PaneKind::Output).height
            <= tool.y
    );
    assert!(tool.y + tool.height <= pane_rect(&after, PaneKind::Prompt).y);
}

/// **Typing ahead does not answer a decision** (review stop 5, E2, CRITICAL,
/// probed; Steven, 10-19: "Create the pane, but leave the cursor where it
/// is"; vision.md: "No accidental permissions by approving the wrong
/// thing"). Writing the next prompt in Insert while a turn ran, a call
/// arrived and took focus, and the rest of the sentence went to it — its
/// `a` ALLOWED a command nobody read. Now the sentence stays in the prompt,
/// the decision waits, and its border says how to reach it: `{n}<C-w>w`,
/// the pane's own number (its title said `{n}^Ww to answer` until Steven,
/// 10-19, asked for `(n) more pending` there). Routed as production routes
/// keys.
#[tokio::test]
async fn typing_ahead_does_not_answer_a_decision() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    let size = (80, 24);
    type_keys(&mut app, "ileave");
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    type_keys(&mut app, " a test, done");
    assert!(app.shown_call().is_some(), "nothing answered");
    assert!(app.tool_results.is_empty(), "nothing ran");
    assert_eq!(
        app.prompt().text(),
        "leave a test, done",
        "the sentence, whole"
    );

    // Drawn at the size the keys are routed at: a window number is a place
    // in THAT layout (a pane that does not fit is not numbered).
    let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let screen = rendered_text(terminal.backend().buffer());
    let title = screen
        .lines()
        .find(|l| l.contains("run this?"))
        .unwrap_or_else(|| panic!("the decision, drawn\n{screen}"));
    // The pane's number, at its border's right end.
    let digits: String = title
        .trim_end_matches(|c: char| !c.is_ascii_digit())
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect();
    let n: usize = digits
        .chars()
        .rev()
        .collect::<String>()
        .parse()
        .unwrap_or_else(|_| panic!("how to reach it: {title}"));
    // `{n}<C-w>w`, as the keymap hands it on (window commands need the
    // frame, so the seam is `apply_window`, as the counted-cycle test's).
    assert!(!app.apply_window(WindowCmd::FocusCycle, Some(n), size.0, size.1));
    assert_eq!(app.focus(), PaneKind::Tool, "`{n}<C-w>w` reaches it");
}

/// **A call that fills the pane under your cursor waits before a key answers
/// it** (Steven, 10-19: "a few hundred ms… configurable… disable it (with
/// 0)"). The case it is for: an allowed command ends, the session's next call
/// fills the pane you stand in, and the second `a` of a double-tap — or a
/// key held down — reaches it unread. Each answer key pressed too soon pushes
/// the wait out again, so a held key (repeating faster than any delay) never
/// answers, and one pressed after the wait does. Every answer waits, a deny
/// too; a reading key does not.
#[tokio::test]
async fn a_call_that_fills_your_pane_waits_before_a_key_answers_it() {
    use std::time::Duration;
    let mut app = test_app();
    assert_eq!(
        app.config.tui.approval_delay_ms, 300,
        "premise: the default"
    );
    // The clock frozen: no real time passes between the presses below, so
    // every step is the one written (review stop 6, C 10).
    app.let_time_pass(Duration::ZERO);
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent-1");
    model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent-2");
    let on_pane = |app: &App| app.shown_call().map(|p| p.tool_use_id().to_string());
    let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    tool_key(&mut app, 'a'); // read, then allowed
    app.settle();
    settle(&mut app).await; // tu_1 finishes
    app.settle();
    assert_eq!(on_pane(&app).as_deref(), Some("tu_2"), "premise: it filled");
    assert_eq!(app.focus(), PaneKind::Tool, "premise: under your cursor");

    for c in ['a', 'd'] {
        app.key_for_test(key(c), (80, 100));
        assert_eq!(on_pane(&app).as_deref(), Some("tu_2"), "`{c}` answered");
    }
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("`tui.approval_delay_ms`")),
        "says why, and the setting: {:?}",
        app.notice()
    );
    app.key_for_test(key('v'), (80, 100));
    assert_eq!(app.focused_mode(), Mode::Visual, "reading keys are live");
    app.key_for_test(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), (80, 100));

    // Held down: 250 ms apart, each inside the wait the one before restarted.
    for _ in 0..5 {
        app.let_time_pass(Duration::from_millis(250));
        app.key_for_test(key('a'), (80, 100));
        assert_eq!(on_pane(&app).as_deref(), Some("tu_2"), "a held key");
    }
    app.let_time_pass(Duration::from_millis(300));
    app.key_for_test(key('a'), (80, 100));
    assert_eq!(on_pane(&app), None, "let go, read, answered");
}

/// **`tui.approval_delay_ms = 0` turns the wait off**: a call answers the
/// moment it is shown.
#[tokio::test]
async fn an_approval_delay_of_zero_answers_at_once() {
    let mut config = Config::default();
    config.tui.approval_delay_ms = 0;
    let mut app = test_app_with(config);
    // Frozen: the key lands on the very instant the call was shown.
    app.let_time_pass(std::time::Duration::ZERO);
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent-1");
    app.set_focus(PaneKind::Tool); // not read: no time passes
    app.key_for_test(
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        (80, 100),
    );
    assert!(app.shown_call().is_none(), "answered at once");
}

/// Non-blocking: with a decision pending the user can move focus away and
/// scroll the transcript — the context that produced a command is often what
/// tells you whether to run it — and the decision is still there afterwards.
#[tokio::test]
async fn a_pending_decision_does_not_trap_focus() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    assert_ne!(
        app.focus(),
        PaneKind::Tool,
        "it does not take focus as it arrives (review stop 5, E2)"
    );
    to_decision(&mut app);

    app.toggle_focus();
    assert_ne!(app.focus(), PaneKind::Tool, "and can leave it");
    assert!(app.shown_call().is_some(), "the decision still waits");

    // Keys now drive whatever is focused, not the gate: `d` must not
    // resolve anything while focus is elsewhere. Pressed through the route a
    // key takes: called on the decision's handler with focus away, it asked
    // a question the router never asks (review stop 12, T1 10).
    app.press_for_test(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(
        app.shown_call().is_some(),
        "the gate only answers while focused"
    );

    // And it is REACHABLE AGAIN — the bug was leaving and never getting
    // back, so assert the property (returns within a full cycle) rather
    // than a hardcoded ring order.
    let mut returned = false;
    for _ in 0..4 {
        app.toggle_focus();
        if app.focus() == PaneKind::Tool {
            returned = true;
            break;
        }
    }
    assert!(returned, "cycling must come back to the pending decision");

    tool_key(&mut app, 'd');
    // The key's event ends: a pane answered from is held for its session's
    // next call until then, and lets go with nothing next.
    app.settle();
    assert!(app.shown_call().is_none(), "answered");
    assert_eq!(app.focus(), PaneKind::Prompt, "focus returns to the prompt");
    assert!(!app.workspace.has(PaneKind::Tool), "and the pane closes");
}

/// **`v` in the decision pane selects over its report, and while a selection
/// is up no key answers** — the `va…` bug (reproduced, docketed): `v` was
/// refused there and the keymap left in Normal, so the `a` of a reflexive
/// `vap` or `vaw` ALLOWED the call (Steven, 10-18: "Agreed. Please build.").
/// `vap` selects a paragraph and answers nothing, nor do the answer keys
/// after it; the legend says they are dead rather than offering `a allow`;
/// `Esc` ends it, the legend's keys come back, and `a` answers again.
///
/// Routed as production routes it: the decision handler first.
#[tokio::test]
async fn v_in_the_decision_pane_selects_and_answers_nothing() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app);
    let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let legend = |app: &mut App| text_of(app, 100, 30);
    assert!(
        legend(&mut app).contains("a allow"),
        "premise: its keys offered"
    );

    let report = |app: &App| app.shown_call().unwrap().text().clone();
    assert_eq!(
        report(&app).cursor(),
        0,
        "premise: `vap` from the first line"
    );
    for c in ['v', 'a', 'p'] {
        app.key_for_test(key(c), (80, 24));
    }
    assert_eq!(app.focused_mode(), crate::keymap::Mode::Visual);
    // The paragraph, not merely a selection: the `v` alone made one, so this
    // passed while `ap` selected nothing — there was no such object (Steven,
    // 10-19: "`ap` doesn't seem to be working in the tool prompt pane").
    let shown = report(&app);
    let blank = |i: &usize| shown.lines()[*i].text().trim().is_empty();
    let rows = 0..shown.lines().len();
    let first_blank = rows.clone().find(blank).expect("premise: a blank line");
    let past = (first_blank..rows.end).take_while(blank).last().unwrap();
    assert_eq!(
        shown.selected_lines(),
        Some(0..=past),
        "the first paragraph and the blank lines after it"
    );
    for c in ['a', 'A', 'e', 'q', 'n'] {
        app.key_for_test(key(c), (80, 24));
        assert!(app.shown_call().is_some(), "`{c}` answered nothing");
    }
    let shown = legend(&mut app);
    assert!(
        app.decision_selecting()
            && shown.contains("Esc back to the answers")
            && !shown.contains("a allow"),
        "the legend says its keys are dead: {shown}"
    );

    app.key_for_test(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), (80, 24));
    assert!(!app.decision_selecting(), "Esc ended it");
    assert!(legend(&mut app).contains("a allow"), "its keys again");
    app.key_for_test(key('a'), (80, 24));
    assert!(app.shown_call().is_none(), "and `a` answers");

    // `y` copies and ends it too — both halves of Visual, or the keys stay
    // dead with nothing on screen to end it.
    model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent-2");
    to_decision(&mut app);
    for c in ['v', 'j', 'y'] {
        app.key_for_test(key(c), (80, 24));
    }
    assert!(
        app.shown_call().is_some(),
        "premise: `vjy` answered nothing"
    );
    assert!(!app.decision_selecting(), "`y` ended it");
    assert!(
        app.prompt()
            .register_text()
            .as_deref()
            .is_some_and(|t| !t.is_empty()),
        "copied, for `p` in the prompt"
    );
    assert!(
        !app.shown_call().unwrap().text().selecting(),
        "the range gone with it"
    );
    assert!(legend(&mut app).contains("a allow"));
    // A writer over the selection: refused as the transcript refuses one, the
    // selection still up — `d` over a range denies nothing — and `Esc` ends
    // it as ever.
    for c in ['v', 'd'] {
        app.key_for_test(key(c), (80, 24));
    }
    assert!(app.shown_call().is_some(), "`vd` answered nothing");
    assert!(
        app.notice().is_some_and(|n| n.contains("read-only")),
        "{:?}",
        app.notice()
    );
    assert_eq!(
        app.focused_mode(),
        crate::keymap::Mode::Visual,
        "still selecting, as the legend says"
    );
    assert!(app.decision_selecting());
    app.key_for_test(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), (80, 24));
    assert!(!app.decision_selecting(), "Esc ended it");
    app.key_for_test(key('d'), (80, 24));
    assert!(app.shown_call().is_none(), "then `d` answers");

    // A selection in ANOTHER pane is not the decision's: read the transcript
    // beside a decision waiting, select there, and its keys are still offered.
    model_call(&mut app, "tu_3", "rm -rf /tmp/shannon-test-absent-3");
    app.output_mut().begin_turn("hello");
    app.output_mut().end_turn();
    app.sync_chat();
    app.set_focus(PaneKind::Output);
    app.press_for_test(key('v'));
    assert_eq!(app.focused_mode(), crate::keymap::Mode::Visual, "premise");
    assert!(!app.decision_selecting());
    assert!(legend(&mut app).contains("a allow"), "its keys, offered");

    // …and the transcript, which CAN select, is untouched by the guard.
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.output_mut().begin_turn("hello");
    app.output_mut().end_turn();
    app.sync_chat();
    app.press_for_test(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(
        app.focused_mode(),
        crate::keymap::Mode::Visual,
        "the transcript still enters Visual"
    );
}

/// **A chord in the decision pane answers nothing** (review stop 6, F 1,
/// CRITICAL). `y` was answered "not a decision key" and dropped, so the `a`
/// of a reflexive `yap` or `yaw` ALLOWED the call and the `p` pasted into the
/// prompt — the `va…` bug of 10-18, one key over. Every key the read-only
/// report can honour reaches the keymap now, so `yap` copies the paragraph,
/// and `ma` and `'a` name a mark (refused here: marks are a transcript's);
/// and a key the pane refuses — a writer (`c`, `r`, `x`) or one it does not
/// have (`"`, `@`) — starts the approval wait again, so the key typed
/// after it is never an answer. The legend says what the keys are: a
/// chord's, mid-chord; dimmed, while they wait. Read, the call answers.
#[tokio::test]
async fn a_chord_in_the_decision_pane_answers_nothing() {
    use crate::style::Ink;
    let mut app = test_app();
    // The terminal the legend is drawn in, and so the one the keys are typed in.
    let frame = Rect::new(0, 0, 100, 30);
    // The legend's `a allow`, as drawn: its row's text, and the cell's style.
    let legend = |app: &mut App| {
        let buf = frame_of(app, frame.width, frame.height);
        let cell = cell_of(&buf, "a allow").map(|at| buf[at].style());
        (rendered_text(&buf), cell)
    };
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app); // read; the clock frozen, so a chord's keys come at once
    let (shown, live) = legend(&mut app);
    assert!(live.is_some(), "premise: its keys offered: {shown}");
    assert!(
        app.prompt()
            .register_text()
            .as_deref()
            .is_none_or(str::is_empty),
        "premise: nothing copied"
    );

    type_keys_in(&mut app, frame, "yap");
    assert!(app.shown_call().is_some(), "`yap` answered nothing");
    let first = app.shown_call().unwrap().text().lines()[0]
        .text()
        .trim()
        .to_string();
    assert!(
        app.prompt()
            .register_text()
            .is_some_and(|t| t.contains(&first)),
        "the paragraph copied: {:?}",
        app.prompt().register_text().as_deref()
    );
    for chord in ["yaw", "yy", "cap", "ra", "xa", "\"ayy", "ma", "'a", "@a"] {
        type_keys_in(&mut app, frame, chord);
        assert!(app.shown_call().is_some(), "`{chord}` answered nothing");
        assert_eq!(app.focus(), PaneKind::Tool, "`{chord}` stayed in the pane");
    }

    // Waiting (the refused `@` started it again): the same legend, dimmed —
    // the keys look as dead as they are.
    let quiet = app.theme().ui(Ink::TextQuiet).fg;
    let (_, waiting) = legend(&mut app);
    assert_ne!(live.unwrap().fg, quiet, "premise: a live key is not quiet");
    assert_eq!(waiting.map(|s| s.fg), Some(quiet), "dimmed while it waits");
    app.read_the_call_for_test();
    let (_, again) = legend(&mut app);
    assert_eq!(
        again.map(|s| s.fg),
        live.map(|s| s.fg),
        "live once the wait is over"
    );

    // Mid-chord, the keys are the chord's, and the legend says so.
    type_keys_in(&mut app, frame, "y");
    let (shown, offered) = legend(&mut app);
    assert!(
        offered.is_none() && shown.contains("Esc back to the answers"),
        "{shown}"
    );
    type_keys_in(&mut app, frame, "<Esc>");
    assert!(legend(&mut app).1.is_some(), "Esc: its keys again");
    type_keys_in(&mut app, frame, "a");
    assert!(app.shown_call().is_none(), "and `a` answers");
}

/// The mouse must be able to reach every pane that exists. `pane_at` used to
/// test a hand-written list of two rects, so a click on the decision pane
/// did nothing and `wheel_scroll`'s `PaneKind::Tool` arm was unreachable code.
#[tokio::test]
async fn the_mouse_can_reach_the_decision_pane() {
    let mut app = test_app();
    let frame = Rect::new(0, 0, 80, 40);

    // No decision open: the tool pane has no rect, and nothing reports it.
    let layout = app.workspace.frame_layout(frame);
    assert!(maybe_pane_rect(&layout, PaneKind::Tool).is_none());
    assert_eq!(
        kind_at(&app, frame, center_of(pane_rect(&layout, PaneKind::Output))),
        Some(PaneKind::Output)
    );
    assert_eq!(
        kind_at(&app, frame, center_of(pane_rect(&layout, PaneKind::Prompt))),
        Some(PaneKind::Prompt)
    );

    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    let layout = app.workspace.frame_layout(frame);
    let tool = maybe_pane_rect(&layout, PaneKind::Tool).expect("open");
    assert_eq!(
        kind_at(&app, frame, center_of(tool)),
        Some(PaneKind::Tool),
        "a click on the decision pane names the decision pane"
    );
    // Every pane still answers for its own rect, and the status row for none.
    assert_eq!(
        kind_at(&app, frame, center_of(pane_rect(&layout, PaneKind::Output))),
        Some(PaneKind::Output)
    );
    assert_eq!(
        kind_at(&app, frame, center_of(pane_rect(&layout, PaneKind::Prompt))),
        Some(PaneKind::Prompt)
    );
    assert_eq!(
        kind_at(&app, frame, center_of(layout.status)),
        None,
        "the status row is not a pane"
    );
}

/// A window command acts on **the window you are in**. `<C-w>+` with the
/// decision pane focused reached the workspace as "not the prompt" and
/// resized *the transcript* — a pane the user wasn't looking at, from a
/// keystroke aimed at one they were. Then it refused instead; now the
/// decision pane is the user's to size and move (Steven, 10-04), so it grows
/// — taking its rows from the conversation it borders, never the prompt —
/// and moves, itself. `<C-w>=` gives it back to its content.
#[tokio::test]
async fn a_window_command_on_the_decision_pane_acts_on_the_decision_pane() {
    let (w, h) = (80u16, 40u16);
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app);
    let frame = Rect::new(0, 0, w, h);
    let before = app.workspace.frame_layout(frame);

    assert!(
        !app.apply_window(WindowCmd::ResizeGrow, Some(3), w, h),
        "not a quit"
    );
    let after = app.workspace.frame_layout(frame);
    assert_eq!(
        pane_rect(&after, PaneKind::Tool).height,
        pane_rect(&before, PaneKind::Tool).height + 3,
        "the decision pane grew"
    );
    assert_eq!(
        pane_rect(&after, PaneKind::Prompt).height,
        pane_rect(&before, PaneKind::Prompt).height,
        "the prompt did not move"
    );
    assert!(
        app.notice().is_none(),
        "nothing refused: {:?}",
        app.notice()
    );

    assert!(!app.apply_window(WindowCmd::Relocate(Direction::Left), Some(1), w, h));
    let moved = app.workspace.frame_layout(frame);
    assert!(
        pane_rect(&moved, PaneKind::Tool).x < pane_rect(&moved, PaneKind::Output).x,
        "the decision pane moved to the left of the conversation"
    );

    assert!(!app.apply_window(WindowCmd::Equalize, None, w, h));
    let reset = app.workspace.frame_layout(frame);
    assert_eq!(
        pane_rect(&reset, PaneKind::Tool),
        pane_rect(&before, PaneKind::Tool),
        "back to fitting its content, where it was"
    );

    // And the panes that ARE the user's to size still resize normally.
    app.set_focus(PaneKind::Prompt);
    let before = app.workspace.frame_layout(frame);
    app.set_focus(PaneKind::Prompt);
    assert!(
        !app.apply_window(WindowCmd::ResizeGrow, Some(3), w, h),
        "not a quit"
    );
    assert!(
        pane_rect(
            &app.workspace.frame_layout(Rect::new(0, 0, w, h)),
            PaneKind::Prompt
        )
        .height
            > pane_rect(&before, PaneKind::Prompt).height,
        "the prompt still grows"
    );
}

/// A decision the fit pass dropped must still let you LEAVE.
///
/// The `visible == 0` guard returned for every key, above the window-chord
/// forward it was written alongside — so the pane trapped all input.
/// Scope the guard to the keys that resolve the decision. (Reviewers A, E.)
#[tokio::test]
async fn an_unfittable_decision_refuses_answers_but_not_escape() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app);
    let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);

    // `a` is refused, and says why.
    assert!(matches!(
        app.handle_tool_key(k('a'), (80, 0)),
        ToolKeyEffect::None
    ));
    assert!(app.shown_call().is_some(), "not approved unseen");
    assert!(app.notice().is_some());
    // …but the way out is forwarded, not eaten.
    assert!(
        matches!(app.handle_tool_key(ctrl_w, (80, 0)), ToolKeyEffect::Forward),
        "a fail-closed guard must not close the exit"
    );
    assert!(
        matches!(app.handle_tool_key(k(':'), (80, 0)), ToolKeyEffect::Forward),
        "`:` must reach the command line"
    );
}

/// `PgUp`/`PgDn` scroll **whatever pane is focused**, which is what the
/// shortcut has always claimed to do. It moved the transcript and nothing
/// else, so a permission report too long for its box — the one thing a user
/// must read *before* answering — took `j`/`k` and ignored the page keys.
#[tokio::test]
async fn page_keys_scroll_the_focused_pane() {
    use crate::keymap::ScrollMagnitude::Page;

    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app);
    let rows = app.shown_call().expect("gated").text().lines().len();
    let visible = 3usize;
    assert!(rows > visible, "the report must overflow its box to scroll");

    // Focused on the decision: PgDn moves ITS report.
    app.scroll_focused(ScrollCmd::Down(Page), (80, visible as u16));
    let after = app.shown_call().unwrap().text().scroll();
    assert!(after > 0, "the decision pane paged down");

    app.scroll_focused(ScrollCmd::Up(Page), (80, visible as u16));
    assert_eq!(app.shown_call().unwrap().text().scroll(), 0, "and back up");

    // Focused elsewhere, the decision's report must not move — the page key
    // belongs to the focused pane, not to whatever happens to be open.
    app.set_focus(PaneKind::Output);
    app.scroll_focused(ScrollCmd::Down(Page), (80, visible as u16));
    assert_eq!(
        app.shown_call().unwrap().text().scroll(),
        0,
        "an unfocused pane does not answer the page key"
    );
}

/// Closing the decision pane releases focus only if it HELD focus. Answering
/// a decision from the transcript (which is deliberately allowed) used to
/// yank the user into the prompt mid-read.
#[tokio::test]
async fn closing_the_decision_pane_does_not_steal_focus_from_elsewhere() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app);

    // Move to the transcript, then let the turn end under us.
    app.set_focus(PaneKind::Output);
    let buffer = app.current;
    // The turn in its round. This was `.unwrap_or(app.fake_stream())`, which
    // is eager: it opened a second turn beside the first every time.
    let g = app.foreground_id().expect("the turn in its round");
    app.abandon_tool_round(g, buffer);
    assert!(app.shown_call().is_none(), "the pane closed");
    assert_eq!(
        app.focus(),
        PaneKind::Output,
        "and left the reader where they were"
    );

    // The pane that *does* hold focus still hands it back.
    model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app);
    let buffer = app.current;
    let g = app.foreground_id().expect("the turn in its round");
    app.abandon_tool_round(g, buffer);
    assert_eq!(
        app.focus(),
        PaneKind::Prompt,
        "a vanishing pane releases focus"
    );
}

/// On a short terminal the approval pane YIELDS: the transcript must stay
/// readable behind a decision, which is the entire reason it is a pane and
/// not a takeover.
#[tokio::test]
async fn the_pane_yields_rather_than_squeezing_the_transcript() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    // Short frame: default prompt (10 rows + border) + status leaves little.
    let layout = app.workspace.frame_layout(Rect::new(0, 0, 80, 22));
    let (_, out_rows) = content_size(pane_rect(&layout, PaneKind::Output));
    assert!(
        out_rows >= 4,
        "the conversation must stay readable (got {out_rows} rows)"
    );
    if let Some(tool) = maybe_pane_rect(&layout, PaneKind::Tool) {
        assert!(tool.height > 0, "if shown at all, it is usable");
    }
}

/// A decision that is not on screen must not be answerable. Rendering and
/// input routing used to be two different predicates, so on a short terminal
/// `a` approved a command nobody had seen. (Reviewer E.)
#[tokio::test]
async fn an_invisible_decision_cannot_be_approved() {
    // Sweep EVERY key, rather than the one key the fix happened to be about.
    // Pressing only `a` here left the guard's real content untested: replacing
    // the whole condition with `matches!(key.code, Char('a'))` passed all 637
    // tests, which means `A` — allow-always, the only key that writes standing
    // session policy — was answerable on a command that had never been drawn.
    // The guard asks `ToolPrompt::resolve`, so the test does too: whatever that
    // says resolves must be refused while invisible, and everything else must
    // fall through to the window manager rather than being swallowed.
    for c in ('a'..='z').chain('A'..='Z') {
        let mut app = test_app();
        model_call(&mut app, "tu_1", "rm -r /tmp/shannon-test-absent");
        to_decision(&mut app);
        let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        let resolving = app
            .shown_call()
            .expect("a decision is pending")
            .resolve(&key)
            .is_some();

        // `visible == 0` is what the computed layout reports when the frame had
        // no room for the pane.
        let effect = app.handle_tool_key(key, (80, 0));

        assert!(
            app.shown_call().is_some(),
            "`{c}` answered an unseen decision"
        );
        assert!(app.tool_results.is_empty(), "`{c}` ran something unseen");
        if resolving {
            assert!(app.notice().is_some(), "`{c}` refused silently");
        } else {
            assert!(
                matches!(effect, ToolKeyEffect::Forward),
                "`{c}` resolves nothing, so it must reach the window manager \
                 rather than being trapped by the gate"
            );
        }
    }

    // A modified key answers nothing — it is the window manager's — so it is
    // not refused as an answer either: `<C-a>` was, the guard asking of its
    // character alone. Asked of `resolve`'s own say, the sweep above could
    // not tell.
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -r /tmp/shannon-test-absent");
    to_decision(&mut app);
    let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
    assert_eq!(app.shown_call().unwrap().resolve(&ctrl_a), None);
    assert!(
        matches!(app.handle_tool_key(ctrl_a, (80, 0)), ToolKeyEffect::Forward),
        "`<C-a>` reaches the window manager"
    );

    // Esc answers no decision, seen or not.
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -r /tmp/shannon-test-absent");
    app.handle_tool_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), (80, 0));
    assert!(
        app.shown_call().is_some(),
        "Esc answered an unseen decision"
    );
    assert!(app.tool_results.is_empty(), "Esc ran something unseen");
}

/// **A viewer can no longer hide the gate, because it no longer covers
/// anything.**
///
/// This used to be the "`:graph` is open, so `a` must refuse" test — a real
/// bug (a `q` meant for the graph would DENY an unseen decision, an `a`
/// would APPROVE AND RUN it) patched by teaching `tool_visible_rows` about
/// viewers. Making viewers *panes* deletes the situation instead of
/// handling it: siblings in a tiling layout cannot occlude each other, and
/// the gate keeps taking its own keys only while focused.
///
/// What is asserted here is the property that outlived the mechanism: with
/// a viewer open the decision is **still drawn and still answerable by its
/// own pane**, and keys aimed at the viewer reach the *viewer*.
#[tokio::test]
async fn a_viewer_and_a_decision_coexist_without_stealing_each_others_keys() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -r /tmp/shannon-test-absent");
    assert!(app.shown_call().is_some());

    open_graph(&mut app);
    assert!(app.workspace.has(PaneKind::Graph), "the viewer is a pane");
    assert!(
        app.pane_content_size(
            app.workspace.first_of(PaneKind::Tool).unwrap(),
            Rect::new(0, 0, 80, 40)
        )
        .1 > 0,
        "and the decision is still on screen beside it"
    );

    // Focus is on the graph, so the gate behind it must not see this key.
    // `<Esc>` is the sharpest case: at the gate it means DENY, and a decision
    // answered by a key aimed at another pane is the worst version of this bug.
    // It no longer closes the viewer either (a window closes with `:q`), so
    // here it is simply inert — and inert is the assertion that matters.
    assert_eq!(app.focus(), PaneKind::Graph);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        app.graph_view().is_some(),
        "`<Esc>` must not close a window"
    );
    assert!(
        app.shown_call().is_some(),
        "and must not answer the decision behind it"
    );
}

/// The predicate that *did* survive: a decision the frame has no room to
/// draw cannot be answered. Same guard, now the only reason it can fire.
#[tokio::test]
async fn a_decision_with_no_room_to_draw_cannot_be_answered() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -r /tmp/shannon-test-absent");
    to_decision(&mut app);
    // A frame too short for the prompt bar, a readable transcript AND the
    // decision: the fit pass drops the decision rather than squeezing.
    let visible = app
        .pane_content_size(
            app.workspace.first_of(PaneKind::Tool).unwrap(),
            Rect::new(0, 0, 80, 14),
        )
        .1 as usize;
    assert_eq!(visible, 0, "not drawn on a short frame");

    app.handle_tool_key(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        (80, visible as u16),
    );
    assert!(app.shown_call().is_some(), "not approved unseen");
    assert!(app.tool_results.is_empty(), "and nothing ran");
    assert!(app.notice().is_some(), "and it says why");
}

/// The box follows the text: a pane sized to its content cannot be scrolled
/// (there is nothing hidden), and scrolling stops at the last row rather
/// than letting the report leave the screen.
#[tokio::test]
async fn scrolling_is_bounded_by_the_content() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app);
    // The scrollable BODY (the pinned action legend is not part of it).
    let rows = app.shown_call().expect("gated").text().lines().len();

    // Everything visible → j/k do nothing at all. The pane holds the body AND
    // the pinned legend, so "fits" is one row taller than the body; this gave
    // it `rows` and called that everything visible, while the draw was
    // covering the last row with the legend. Through the real route: the
    // decision pane forwards `j` to the keymap.
    for _ in 0..10 {
        app.key_for_test(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            (80, rows as u16 + 1),
        );
    }
    assert_eq!(
        app.shown_call().unwrap().text().scroll(),
        0,
        "a report that fits must not scroll away"
    );

    // A cramped pane → scrolling works, but stops with content on screen: four
    // rows are the legend, the fold marker, and a body of two.
    let (pane, body) = (4u16, 2usize);
    for _ in 0..100 {
        app.key_for_test(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            (80, pane),
        );
    }
    let scroll = app.shown_call().unwrap().text().scroll() as usize;
    assert_eq!(
        scroll,
        rows.saturating_sub(body),
        "stops when the last row is in view — never past it, never short of it"
    );
}

/// The pane is sized from its content, capped by config — not a fixed box.
#[tokio::test]
async fn the_pane_is_sized_to_its_content_and_capped() {
    let frame = Rect::new(0, 0, 80, 60);
    let mut cfg = Config::default();
    cfg.tui.approval_max_height = 6; // a deliberately tight cap
    let mut app = test_app_with(cfg);
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");

    let rect = maybe_pane_rect(&app.workspace.frame_layout(frame), PaneKind::Tool).expect("open");
    let content = content_size(rect).1;
    assert_eq!(content, 6, "content taller than the cap is capped");

    // And a roomy cap lets a short report keep its natural (smaller) size.
    let mut app = test_app(); // default cap (18)
    model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent");
    let natural = app
        .shown_call()
        .unwrap()
        .content_rows(frame.width.saturating_sub(2));
    let rect = maybe_pane_rect(&app.workspace.frame_layout(frame), PaneKind::Tool).expect("open");
    assert_eq!(
        content_size(rect).1,
        natural,
        "the box matches the text, not a constant"
    );
}

/// Flatten a rendered `TestBackend` buffer into newline-joined row text.
/// **The fold marker must not eat the command you are approving.**
///
/// It was painted over the right-hand end of the body's last row, so a wrapping
/// command lost its tail *in place*: ` | head -50` became `↓ 3 more`, with no
/// ellipsis and no seam, leaving a line that reads as a complete pipeline. On
/// the one surface whose whole purpose is that you read the exact text before
/// it runs (Reviewer F).
///
/// Asserted on the CELL GRID, because that is where the defect was — every
/// model-side assertion (`content_rows`, `rows_at`, the marker's own text) was
/// true throughout.
#[tokio::test]
async fn the_fold_marker_never_overwrites_the_command() {
    let tail = "head -50-UNIQUE-TAIL";
    let cmd = format!(
        "grep -RIn --binary-files=without-match 'a-very-long-needle-string-here' \
         /Users/someone/development/some/deeply/nested/project/src | sort -u | {tail}"
    );
    let mut app = test_app();
    app.arrive_and_gate_for_test(None, "t1", &cmd);
    let screen = text_of(&mut app, 78, 26);

    assert!(
        screen.contains("more") || screen.contains("above"),
        "premise — the report must overflow, or the marker never draws:\n{screen}"
    );
    // The marker is on its own row, so no row holds both it and command text.
    for line in screen.lines() {
        let has_marker = line.contains("more") || line.contains("above");
        assert!(
            !(has_marker && line.contains("grep")),
            "the marker shares a row with the command it is describing:\n{line}"
        );
    }
    // The marker's row holds NOTHING but the marker and the pane border. That
    // is the property; "the tail is on screen" is not, because content below a
    // fold is legitimately off screen — which is what the marker is for.
    let row = screen
        .lines()
        .find(|l| l.contains("more") || l.contains("above"))
        .expect("checked above");
    let bare = row.trim_matches(|c| c == '│' || c == ' ');
    assert!(
        bare.starts_with('↓') || bare.starts_with('↑'),
        "the marker is painted over content — its row must be its own:\n{row:?}"
    );
    let _ = tail;
}

/// **Focus never falls into a decision** (review stop 5, E2 #3, probed): a
/// selection begun in the transcript, and `:close` on it — the decision band
/// beside it becomes the first pane, where `Focus::reconcile` lands. It
/// carried Visual in (no `focus_moved`), and the next `a` — or `v`, then `a`
/// — ALLOWED. Now the close's event ends at the prompt, Visual over.
///
/// **No user close reaches it since 10-26**: the chat a band sits under is
/// not closed, and a forced close takes the band first
/// ([`a_chat_a_request_sits_under_closes_only_by_force`]). The premise is
/// the workspace's own close, under every door: `settle_focus` is the floor
/// for a door that forgets.
#[tokio::test]
async fn a_close_never_drops_you_into_a_decision() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    app.set_focus(PaneKind::Output);
    let size = (80, 30);
    app.key_for_test(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE), size);
    assert!(app.chat_keymap.mode().is_visual(), "premise: selecting");

    assert!(app.workspace.close_focused(), "premise: the chat closed");
    app.reconcile_pane_state();
    assert_eq!(
        app.focus(),
        PaneKind::Tool,
        "premise: the close fell into the decision"
    );
    app.settle(); // the `:close` event ends

    assert_eq!(app.focus(), PaneKind::Prompt, "you stand at the prompt");
    assert!(
        !app.chat_keymap.mode().is_visual(),
        "the selection ended with its pane"
    );
    app.key_for_test(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE), size);
    assert!(app.shown_call().is_some(), "and nothing was answered");
}

/// The catch-all's other half: focus moved by a close that lands somewhere
/// ordinary still ends what the closed pane had going.
#[tokio::test]
async fn a_close_ends_the_selection_of_the_pane_it_closed() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("output", SplitDir::Vertical);
    app.settle(); // the split's event ends: focus moved into the new pane
    let size = (80, 30);
    app.key_for_test(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE), size);
    assert!(app.chat_keymap.mode().is_visual(), "premise: selecting");
    app.close_pane_command(Asked::Plain);
    assert_eq!(
        app.focus(),
        PaneKind::Output,
        "premise: the other transcript"
    );
    app.settle();
    assert!(!app.chat_keymap.mode().is_visual());
}

/// **A decision with no room says what answers it: room** (review stop 5,
/// E2 #2). With no rows to draw it in, its keys refuse ("doesn't fit"), and
/// the row offered them anyway; now the row offers what works, and the keys
/// it names are the ones that do.
#[tokio::test]
async fn a_decision_with_no_room_offers_room_not_dead_keys() {
    let mut app = test_app();
    app.arrive_and_gate_for_test(None, "t1", "rm -rf /tmp/whatever");
    let size = (100, 6);
    let screen = text_of(&mut app, size.0, size.1);
    assert!(
        !screen.contains("run this?"),
        "premise: the pane is not drawn\n{screen}"
    );
    let row = screen.lines().last().unwrap_or_default();
    assert!(
        row.contains("no room to answer") && !row.contains("a allow"),
        "{row:?}"
    );
    // ...and the keys it does not offer indeed refuse.
    to_decision(&mut app);
    app.handle_tool_key(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        (size.0, 0),
    );
    assert!(app.shown_call().is_some(), "nothing answered");
}

/// **A chord's second key goes to the chord, room or no room** (review stop
/// 8, E1 5) — with no room the room guard ran first, refused the key as an
/// answer that "doesn't fit", and left the prefix armed for the next key in
/// any pane.
#[tokio::test]
async fn a_chord_in_a_pane_with_no_room_keeps_its_second_key() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    to_decision(&mut app);
    app.key_for_test(
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
        (100, 0),
    );
    assert_eq!(
        app.answer_keys(app.workspace.focused()),
        AnswerKeys::Chord,
        "premise: a chord pending"
    );
    let effect = app.handle_tool_key(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        (100, 0),
    );
    assert_eq!(
        effect,
        ToolKeyEffect::Forward,
        "the chord's, not an answer refused"
    );
    assert!(app.shown_call().is_some(), "nothing answered");
}

/// **A call coming into view by room waits before a key answers it** (review
/// stop 8, E1 2) — `:only`, `<C-w>_` or a resize is new on screen as much as
/// a call shown. Its first `a` allowed a call no frame had drawn: at once
/// after a frame with room, and before any, in a burst of keys handled before
/// the draw (`:only<CR>a`).
#[tokio::test]
async fn a_call_coming_into_view_by_room_waits_before_a_key_answers() {
    use ratatui::{Terminal, backend::TestBackend};
    let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    let mut app = test_app();
    app.arrive_and_gate_for_test(None, "t1", "rm -rf /tmp/shannon-test-x");
    to_decision(&mut app);
    let mut small = Terminal::new(TestBackend::new(100, 6)).unwrap();
    draw_frame(&mut app, &mut small);
    // In a burst: room as the key is read, no frame drawn with it.
    app.handle_tool_key(key, (100, 10));
    assert!(
        app.shown_call().is_some(),
        "not answered before a frame drew it"
    );
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("not on screen yet"), "{said}");

    // Drawn with room: its keys wait again, from that frame.
    app.read_the_call_for_test();
    let mut roomy = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_frame(&mut app, &mut small);
    draw_frame(&mut app, &mut roomy);
    app.handle_tool_key(key, (100, 10));
    assert!(
        app.shown_call().is_some(),
        "not answered at once on coming into view"
    );
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("not answered — still open"), "{said}");
    app.read_the_call_for_test();
    app.handle_tool_key(key, (100, 10));
    assert!(app.shown_call().is_none(), "read, it answers");
}

/// **The decision pane is sized at the width it is DRAWN at, not the frame's.**
///
/// The consent surface got this wrong twice, and the second time survived the
/// first fix. `content_rows` counted logical lines while the pane renders
/// wrapped (Reviewer E) — fixed. But the width handed to that measurement was
/// `last_frame_width`, and the pane is a band inside ONE transcript, so a
/// single `:vsp` halves it: Reviewer F reproduced 20 rendered rows squeezed
/// into a 15-row box at 120x40, five rows of findings below the fold of a
/// border that draws as complete.
///
/// The class survived its own repair because the *unit* was fixed and the
/// *width* was not — which is why this asserts the relationship (`rows_at(the
/// drawn width)` fits the box) rather than a number.
#[tokio::test]
async fn the_decision_pane_fits_the_pane_it_is_drawn_in_not_the_frame() {
    let frame = Rect::new(0, 0, 120, 40);
    let mut app = test_app();
    // Two TRANSCRIPTS side by side, so the decision band is half the frame.
    // Focus first: `:vsp` splits the focused pane, and from the prompt bar it
    // splits the prompt — which leaves the transcript full width and the test
    // proving nothing.
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.clamp_text_panes(frame.width, frame.height);

    // A command the model controls the length of, with something destructive
    // after the padding — exactly the shape that pushes findings off the end.
    let padding = "x".repeat(360);
    // Arrived, then gated, as a model's call is (review stop 12, T3 16).
    app.arrive_and_gate_for_test(
        app.foreground_id(),
        "tu-1",
        &format!("grep -rn {padding} . ; rm -rf /tmp/shannon-test-absent-fold"),
    );
    // The frame the fit was computed against.
    app.clamp_text_panes(frame.width, frame.height);

    let layout = app.workspace.frame_layout(frame);
    let pane = layout
        .panes
        .iter()
        .find(|p| app.workspace.kind_of(p.id) == Some(PaneKind::Tool))
        .expect("the decision pane is open");
    let (drawn_w, drawn_h) = pane.content_size();
    let dump: Vec<String> = layout
        .panes
        .iter()
        .map(|p| {
            format!(
                "{:?}:{}x{}",
                app.workspace.kind_of(p.id),
                p.content_size().0,
                p.content_size().1
            )
        })
        .collect();
    assert!(
        drawn_w < frame.width - 2,
        "premise: the pane is narrower than the frame ({drawn_w} vs {}), or \
         this test would pass without the fix — layout was {dump:?}",
        frame.width
    );

    let prompt = app.shown_call().expect("gated");
    let footer = prompt.legend().is_some() as u16;
    let body = drawn_h.saturating_sub(footer) as usize;
    let needed = prompt.text().rows_at(drawn_w);
    let cap = app.config.tui.approval_max_height;
    assert!(
        needed <= body || drawn_h >= cap,
        "the report fits the box, unless the cap is what limited it: needed \
         {needed} rendered rows at width {drawn_w}, body is {body} rows \
         (pane {drawn_h}, cap {cap})"
    );
}

/// **The recovery a refusal names must be one that works.**
///
/// `busy_phrase` collapsed eight hand-written refusals into one sentence,
/// correctly. What it did NOT absorb was the *recovery*, and the three sites
/// that kept their own tails had all written them assuming a running request
/// that ends by itself — "stop it (CTRL-C) or wait", "resume it once that
/// finishes". Against a turn parked on a decision every one of them is wrong in
/// the same way: waiting is exactly what will never work, because the thing
/// holding the slot is waiting on *you*.
///
/// `submit` read correctly and was the only one with no tail — which is the
/// tell that the tails, not the phrase, were the problem (Reviewer F).
#[tokio::test]
async fn a_refusal_blocked_on_a_decision_says_to_answer_it() {
    let mut app = test_app();
    model_call(&mut app, "tu-1", "rm -rf /tmp/shannon-test-absent-recovery");
    assert!(app.awaiting_decision(), "premise: parked on a decision");

    app.model_command("opus");

    let notice = app.notice().expect("a visible refusal").to_string();
    assert!(
        notice.contains("answer it first"),
        "names a recovery that works: {notice}"
    );
    // `"or wait"`, not `"wait"` — the phrase under test is the recovery, and
    // `"awaiting your decision"` contains the shorter substring. The vacuity
    // shape from the other direction: a negative assertion that matches its own
    // subject ([[feedback_mutation_check_guards]]).
    assert!(
        !notice.contains("or wait"),
        "and not one that never will — the slot is waiting on the user: {notice}"
    );
}

/// **A chord is not its letter.** `<C-q>` must not close a pane.
///
/// The close arm matched `key.code` alone, which makes every modifier a
/// silent alias: `<C-q>` — a real chord on many terminals, and next to
/// `<C-w>` on the keyboard — closed the pane you were reading. This is the
/// half of a guard the type system cannot see, so it gets its own assertion
/// ([[feedback_guard_has_a_visible_half]]).
///
/// **On the one pane a bare `q` closes**: a decision pane kept open, waiting
/// for its session's next call (`a_wait_with_nothing_coming_closes_and_q_leaves_one`
/// pins the bare `q`). It was asked of the graph and the help page, which no
/// bare `q` closes since 10-14, so the guard it named was held by nothing
/// (review stop 12, T3 12).
#[tokio::test]
async fn a_modified_q_does_not_close_a_waiting_decision() {
    for mods in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        let mut app = test_app();
        model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent-1");
        model_call(&mut app, "tu_2", "rm -rf /tmp/shannon-test-absent-2");
        tool_key(&mut app, 'a');
        assert!(
            app.wait_text_for_test().is_some(),
            "premise: the pane waits"
        );
        app.key_for_test(KeyEvent::new(KeyCode::Char('q'), mods), (80, 100));
        assert!(
            app.workspace.has(PaneKind::Tool) && app.wait_text_for_test().is_some(),
            "{mods:?}+q left the wait — a chord aliased to its letter"
        );
        // The premise, here: the letter alone does close it.
        app.key_for_test(KeyEvent::from(KeyCode::Char('q')), (80, 100));
        assert!(
            !app.workspace.has(PaneKind::Tool),
            "premise: a bare `q` closes the waiting pane"
        );
    }
}

/// **Every line of a capped decision report can be brought on screen.**
///
/// The report's view was sized by the pane's content height while the draw
/// gave the body two rows fewer (the pinned legend, the fold marker). So on a
/// report longer than `tui.approval_max_height`, `G` put the cursor — and the
/// last two lines, the per-stage detail — under the legend: "↓ 2 more" drawn,
/// and no key that reached them. Asserted on RENDERED text, because the view
/// always believed it was showing them.
#[tokio::test]
async fn every_line_of_a_capped_decision_report_can_be_brought_on_screen() {
    let (mut app, mut terminal, lines) = capped_decision(8).await;
    assert!(lines.len() > 3, "premise: the report does not fit");
    let j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    let mut seen = rendered_text(terminal.backend().buffer());
    for _ in 0..lines.len() {
        seen.push_str(&decision_key(&mut app, &mut terminal, j));
    }
    for line in &lines {
        let words = line.trim();
        assert!(
            seen.contains(words),
            "never on screen, whatever the keys: {words:?}"
        );
    }
}

/// **One row left below is marked below the text.** The marker's side was
/// chosen against the whole area, before its own row was taken, so with
/// exactly one row below the body it went to the TOP — `fold_marker` counts
/// against the body, so it still said "↓ 1 more", drawn above the line it
/// points past. Asserted on POSITION, which is the half that was wrong.
#[tokio::test]
async fn one_row_below_the_fold_is_marked_below() {
    let (mut app, mut terminal, lines) = capped_decision(8).await;
    let pane = app.workspace.focused();
    let j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    // A one-row body here, so the window's top is the cursor's line: walk to
    // the second-to-last line, leaving exactly one below.
    let mut screen = String::new();
    for _ in 0..lines.len() - 2 {
        screen = decision_key(&mut app, &mut terminal, j);
    }
    let (_, top) = app.text_pane_pos_for_test(pane).expect("a report");
    assert_eq!(top as usize, lines.len() - 2, "premise: one row left below");
    let row_of = |needle: &str| screen.lines().position(|l| l.contains(needle));
    let shown = row_of(lines[lines.len() - 2].trim()).expect("the body row is drawn");
    let marker = row_of("↓ 1 more").expect("the row below is counted");
    assert!(marker > shown, "the marker sits below the text:\n{screen}");
}

/// **A scrolled decision pane stays scrolled across the next frame.**
///
/// `clamp_text_panes` runs before every draw and `clamp` follows the cursor.
/// The decision pane was the one text page still scrolling the VIEW, so its
/// cursor never moved and the follow dragged the scroll back to zero on the
/// very next frame — the report became unscrollable while its legend said
/// `j/k scroll`, and that pane's hidden content is a safety question you are
/// being asked to approve (Reviewer D).
///
/// The two existing scroll tests could not see it: neither renders a frame, so
/// `clamp_text_panes` never ran. **Driving a key is not the same as living
/// through a frame**, and for anything the per-frame sweep touches, the frame
/// is the thing to simulate.
#[tokio::test]
async fn a_scrolled_decision_pane_survives_the_next_frame() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("go");
    model_call(&mut app, "tu-1", "rm -rf /tmp/a | grep x | wc -l > /tmp/b");
    to_decision(&mut app);
    let pane = app
        .workspace
        .first_of(PaneKind::Tool)
        .expect("a decision pane");

    // Cramped, so there is something below the fold. Through the real route:
    // `j` is the keymap's now, forwarded by the decision pane.
    for _ in 0..6 {
        app.key_for_test(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            (60, 3),
        );
    }
    let scrolled = app.text_pane_pos_for_test(pane).expect("content");
    assert!(scrolled.1 > 0, "premise: it scrolled at all");

    app.clamp_text_panes(80, 24);
    let after = app.text_pane_pos_for_test(pane).expect("content");
    assert!(
        after.1 > 0,
        "the next frame dragged the report back to the top: {scrolled:?} -> {after:?}"
    );
}

/// **A decision the frame cannot draw must still be answerable.**
///
/// Reproduction of the demo failure. While a decision is outstanding its
/// session accepts no prompt — correct when the decision is on screen and one
/// keystroke answers it. When the fit pass cannot give the pane rows, the
/// decision is outstanding, invisible, and unanswerable, and the status row
/// must name a recovery.
///
/// **It holds its own session only** (`buffers.md` §9, call 2; Steven,
/// 10-20: "No. Sessions are independent"). This was `a_decision_that_does_
/// not_fit_still_blocks_every_session`, and while a decision held every
/// session it froze the whole program; and its second session was the one
/// the decision was typed in, so "a session that has nothing to do with it"
/// had everything to do with it — the other one is asked now.
#[test]
fn a_decision_that_does_not_fit_holds_its_session_and_says_how_to_answer() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app();
    let first = app.current;
    // A second session, where the decision is asked; the first has nothing
    // to do with it.
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let other = app.pane_session(app.workspace.focused()).id;
    assert_ne!(first, other, "premise: two sessions");

    // A long command on a short terminal — the shape a real tool call has.
    let command = "find / -name '*.md' -type f -newer /tmp/x -o -name '*.rst' \
                   -print0 2>/dev/null | xargs -0 grep -l TODO";
    // Through `:tool`'s door, its arrival claim included (review stop 12,
    // T3 16: a bare `gate_tool_call` is a shape no typed call has).
    let call = manual_tool(&mut app, command);
    assert!(app.shown_call().is_some(), "premise: a decision is pending");

    // Draw at a height where the decision cannot get rows.
    let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let screen = rendered_text(terminal.backend().buffer());

    // Its session refuses prompts while the decision is outstanding — which
    // is why the decision has to stay ANSWERABLE — and no other does.
    assert_eq!(
        app.tool_buffer(&call),
        Some(other),
        "premise: the second's call"
    );
    assert!(
        app.busy_reason(other).is_some(),
        "the decision holds its own session"
    );
    assert_eq!(
        app.busy_reason(first),
        None,
        "and not one that has nothing to do with it"
    );

    // **Recoverable, which is the promise that can actually be kept.** On a
    // frame small enough, nothing fits — and a decision not on screen cannot
    // be answered (`handle_tool_key`, Reviewer E) — so the guarantee is not
    // "the pane is drawn", it is "the way to MAKE it answerable is on
    // screen". It once lived in the `Hint` legend, the FIRST segment dropped
    // when the line is full; then the row offered the keys, which refuse here
    // (review stop 5, E2 #2). Room is what answers it.
    assert!(
        screen.contains("no room to answer") && screen.contains(":only"),
        "what makes it answerable must survive a crowded status line\n{screen}"
    );
    assert!(
        !screen.contains("a allow"),
        "not keys that refuse\n{screen}"
    );

    // And whichever way the frame went, the decision is reachable: either the
    // pane is drawn, or the status line is carrying it.
    assert!(
        screen.contains("┌ decision") || screen.contains("DECIDE:"),
        "a pending decision must be visible somehow\n{screen}"
    );
}

/// **A truncated turn says so on screen.**
///
/// The stop reason was folded into provenance and nowhere else, so it reached
/// the `:graph` inspector and `:continue` and no other surface. Seven turns of
/// a live demo were cut off at the token ceiling and the screen never mentioned
/// it once — the reply just stopped, or never arrived, and the app read as
/// frozen. Every failure has to be tied to a message (Steven's ruling, and the
/// biggest lesson of that demo).
#[tokio::test]
async fn a_turn_that_was_cut_off_says_so() {
    use shannon_core::StopReason as R;

    // Every reason, classed by an exhaustive match: a cut-off reaches the
    // user, and the ordinary endings stay quiet, or the notice line becomes
    // noise nobody reads. Two hand lists left out `PauseTurn` and `Other(_)`,
    // so a pause going silent passed (review stop 12, T3 13).
    let every = [
        R::EndTurn,
        R::MaxTokens,
        R::StopSequence,
        R::ToolUse,
        R::PauseTurn,
        R::Refusal,
        R::ModelContextWindowExceeded,
        R::Interrupted,
        R::Other("brand_new_reason".into()),
    ];
    for reason in &every {
        let reaches_the_user = match reason {
            R::MaxTokens | R::ModelContextWindowExceeded | R::Refusal | R::PauseTurn => true,
            R::EndTurn | R::StopSequence | R::ToolUse | R::Interrupted | R::Other(_) => false,
        };
        let note = crate::app::truncation_notice(reason, true);
        assert_eq!(note.is_some(), reaches_the_user, "{reason}: {note:?}");
        assert!(note.is_none_or(|n| !n.is_empty()), "{reason}");
    }
    // And on screen, through the stream: a paused turn says so.
    let mut app = test_app();
    app.submit("go");
    app.feed_stream(StreamMsg::Stopped(R::PauseTurn));
    app.feed_stream(StreamMsg::Complete);
    assert!(
        app.notices.iter().any(|(_, n)| n.contains("paused")),
        "{:?}",
        app.notices
    );
    // The recovery is named, not merely the problem: `:continue` is what the
    // user does next and there is nothing on screen that would suggest it.
    let note = crate::app::truncation_notice(&R::MaxTokens, true).unwrap();
    assert!(note.contains(":continue"), "{note}");
    assert!(note.contains("max_output_tokens"), "{note}");

    // **Not MANGLED.** `rustfmt` bakes source indentation into a bare
    // multi-line literal, and both of these carried ten- and fourteen-space
    // runs mid-sentence (Reviewer A). `request.rs` has had a test for exactly
    // this class since the tool descriptions hit it; the class is "a string a
    // user reads", not "a tool description", so it belongs on every one.
    let mut shown: Vec<String> = [
        R::MaxTokens,
        R::ModelContextWindowExceeded,
        R::Refusal,
        R::PauseTurn,
    ]
    .iter()
    .filter_map(|r| crate::app::truncation_notice(r, true))
    .collect();
    shown.push(
        crate::app::unaccounted_output(
            Usage {
                output_tokens: 4096,
                ..Usage::default()
            },
            0,
        )
        .unwrap_or_default(),
    );
    for text in shown {
        assert!(
            !text.contains("   "),
            "a run of spaces mid-sentence — the literal baked in its own \
             indentation: {text:?}"
        );
    }
}

/// **The prompt bar gives rows back so a decision can be drawn.**
///
/// A bar is a size the user chose and is not touched while there is any other
/// way to balance the frame — but "the user chose ten rows of prompt" must not
/// outrank "there is a decision every session is blocked on". In the demo the
/// transcripts were squeezed to their titles, the decision was dropped, and the
/// prompt kept all eleven rows it had asked for.
#[test]
fn a_bar_yields_rows_before_a_decision_is_dropped() {
    use ratatui::{Terminal, backend::TestBackend};

    // 16 rows is the frame where it matters: room for the transcript floor,
    // the decision, and a SHRUNK prompt — but not for the ten rows the prompt
    // asked for. Above this the prompt keeps its size; below it, nothing fits
    // and the status line carries the decision instead
    // (`a_decision_that_does_not_fit_still_blocks_every_session`).
    let mut app = test_app();
    manual_tool(&mut app, "mkdir build");
    assert!(app.shown_call().is_some(), "premise: a decision is pending");

    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let squeezed = rendered_text(terminal.backend().buffer());
    assert!(
        squeezed.contains("run this?"),
        "the decision is drawn because the prompt gave rows back\n{squeezed}"
    );
    assert!(
        squeezed.contains("deny"),
        "…with its answer keys, in the pane\n{squeezed}"
    );

    // The prompt is still there — shrunk, not closed. It is the only way to
    // type, so it yields rows and never the pane itself.
    assert!(squeezed.contains("┌ prompt"), "{squeezed}");
    let prompt_rows = |s: &str| s.matches("│ ").count();
    let mut roomy = Terminal::new(TestBackend::new(80, 24)).unwrap();
    draw_frame(&mut app, &mut roomy);
    let roomy = rendered_text(roomy.backend().buffer());
    assert!(
        prompt_rows(&squeezed) < prompt_rows(&roomy),
        "the bar gave rows back on the tight frame: {} vs {}",
        prompt_rows(&squeezed),
        prompt_rows(&roomy)
    );
}

/// **A command you allowed, running, is not a decision you owe** — through
/// the real path: the model's call arrives and parks (`Complete`), you press
/// `a`, it runs, and you type the next prompt while it does.
///
/// Dogfooded 2026-09-24 on a slow `grep -r`: the prompt queued with
/// "a command is awaiting your decision" and no pane anywhere, because the
/// turn stays in its tool round for the whole run and the refusal answered by
/// stage. `:model`, `:continue` and `S` all said "answer it first". And the
/// status row said `answering…` — the model was saying nothing.
///
/// Then the precedence the fix rests on: with one command running and a
/// second still at the gate, the decision IS what the user owes, and wins.
#[tokio::test]
async fn an_allowed_running_command_is_not_awaiting_a_decision() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = test_app();
    model_call(&mut app, "tu_1", "frobnicate --walk .");
    assert!(
        app.shown_call().is_some(),
        "premise: an unmodelled program asks"
    );
    tool_key(&mut app, 'a');
    assert!(
        app.shown_call().is_none() && app.waiting_calls().is_empty(),
        "premise: nothing left to decide"
    );
    assert!(
        app.running_tools.contains_key("tu_1"),
        "premise: it is running"
    );

    app.submit("and then this");
    let notice = app.notice().unwrap_or_default();
    assert!(notice.contains("queued"), "premise: it queued: {notice:?}");
    assert!(
        !notice.contains("awaiting your decision"),
        "nothing is awaiting a decision: {notice:?}"
    );
    let (why, how) = app
        .busy_reason_and_recovery(app.prompt_target())
        .expect("the slot is held");
    assert_eq!(why, "a command is running");
    // `a` answered the only call: the event's end closed its pane and gave
    // the prompt back. It asserted "still in the pane" here, a state no
    // frame shows — the helper did not end the event (review stop 12, T1 4).
    assert_eq!(app.focus(), PaneKind::Prompt, "back at the prompt");
    assert_eq!(how, "stop it (CTRL-C) or wait", "from the prompt");
    // Where a pane IS kept — a next call waiting behind the one running —
    // the recovery is that call's, wherever you stand: measured, not the
    // "CTRL-C at the prompt" this pinned from the unsettled pane. That
    // sentence is real elsewhere — ANOTHER session's decision pane, the aim
    // answering (`StopKey::AtPrompt`, held in `parallel.rs`); the stop-12
    // report called it unreached, and the double check found it reached.
    {
        let mut kept = test_app();
        let chat = kept.current;
        manual_tool(&mut kept, "rm /tmp/shannon-test-absent-1");
        manual_tool(&mut kept, "rm /tmp/shannon-test-absent-2");
        kept.settle();
        tool_key_in(&mut kept, chat, 'a');
        let pane = kept.decision_pane_of(chat).expect("premise: kept open");
        kept.focus_pane(pane);
        assert_eq!(
            kept.busy_reason_and_recovery(chat),
            Some((
                "a command is running, and its next call waits behind it".to_string(),
                "that call is asked when it finishes".to_string()
            ))
        );
    }
    // (`S` names this cause too: `distilling_is_refused_while_a_job_answers_
    // this_session`, with something marked to distil, as `S` needs since it
    // resolves its plan first — full review 10-21, D 1.)

    // The notice owns the bottom row until the next key; asserted above, so
    // clear it to see the status row it covers.
    app.notices.clear();
    let mut terminal = Terminal::new(TestBackend::new(160, 12)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let row = rendered_text(terminal.backend().buffer())
        .lines()
        .last()
        .expect("a status row")
        .to_string();
    assert!(
        row.contains("running: frobnicate"),
        "the row names the run:\n{row}"
    );
    assert!(
        !row.contains("answering"),
        "the model is not talking:\n{row}"
    );
    assert!(row.contains("^C:stop"), "and says what stops it:\n{row}");

    // A second call, still at the gate, while the first runs.
    model_call(&mut app, "tu_2", "frobnicate --again");
    assert!(app.shown_call().is_some(), "premise: the second one asks");
    assert!(
        app.running_tools.contains_key("tu_1"),
        "premise: first still runs"
    );
    let (why, how) = app
        .busy_reason_and_recovery(app.prompt_target())
        .expect("the slot is held");
    assert_eq!(why, "a command is awaiting your decision");
    assert_eq!(how, "answer it first");
    draw_frame(&mut app, &mut terminal);
    let row = rendered_text(terminal.backend().buffer())
        .lines()
        .last()
        .expect("a status row")
        .to_string();
    assert!(
        !row.contains("answering"),
        "never `answering…` beside DECIDE:\n{row}"
    );
}

/// **The decision pane navigates like `:help`** — `G`, `gg`, counts, `H`, `L`
/// — through the keymap it forwards to. It had two hand-matched arms, `j` and
/// `k`, and nothing else.
#[tokio::test]
async fn the_decision_pane_takes_the_keymaps_motions() {
    let (mut app, len) = decision_of().await;
    assert!(len > 4, "premise: the report is longer than the body");
    decision_keys(&mut app, "G");
    assert_eq!(decision_cursor(&mut app), len - 1, "G");
    decision_keys(&mut app, "gg");
    assert_eq!(decision_cursor(&mut app), 0, "gg");
    decision_keys(&mut app, "3j");
    assert_eq!(decision_cursor(&mut app), 3, "3j");
    decision_keys(&mut app, "ggL");
    assert_eq!(
        decision_cursor(&mut app),
        3,
        "L: the last of four body rows"
    );
    decision_keys(&mut app, "H");
    assert_eq!(decision_cursor(&mut app), 0, "H");
    assert!(
        app.shown_call().is_some(),
        "and none of it answered the decision"
    );
}

/// **The decision pane's answers stay safe with the keymap behind it.** A key
/// that begins a chord owns the keys after it — `za` is a dead `z` chord, and
/// `3ya` a `y` waiting for its object, neither an allow — until `Esc` drops
/// it. This test once asserted the opposite for `y` (the legend, nothing
/// armed, "instead of silently swallowing the `a` after it"), and that `a`
/// ALLOWED the call: a swallowed `a` is the safe failure (review stop 6, F 1).
/// And a count does not block an answer: `3d` denies.
#[tokio::test]
async fn the_decision_panes_answers_survive_the_keymap_behind_it() {
    let (mut app, _) = decision_of().await;
    decision_keys(&mut app, "za");
    assert!(
        app.shown_call().is_some(),
        "`za` is a dead `z` chord, not an allow"
    );

    decision_keys(&mut app, "3ya");
    assert!(
        app.shown_call().is_some(),
        "`3ya` is a `y` and the start of its object, not an allow"
    );
    assert!(
        app.chat_keymap.pending_chord(),
        "…still waiting for the object"
    );
    app.key_for_test(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), (80, 6));
    assert!(!app.chat_keymap.pending_chord(), "`Esc` dropped it");

    decision_keys(&mut app, "3d");
    assert!(
        app.shown_call().is_none(),
        "`3d` denies — a count is not a prefix"
    );
}

/// **A prompt queued behind a decision starts once the decision is answered**
/// — including a decision no turn is waiting on (a typed `:tool`), where no
/// turn's end will drain the queue for it.
#[tokio::test]
async fn a_prompt_queued_behind_a_typed_tool_decision_starts_when_it_is_answered() {
    use crate::permission_view::ToolAction;
    let mut app = test_app();
    manual_tool(&mut app, "rm -f /tmp/shannon-never-runs");
    manual_tool(&mut app, "rm -f /tmp/shannon-never-runs-2");
    assert!(app.shown_call().is_some(), "premise: it asked");
    assert_eq!(
        app.waiting_calls().len(),
        1,
        "premise: a second decision waits"
    );
    app.submit("hello");
    assert_eq!(app.pending.len(), 1, "premise: queued behind the decision");

    // The next decision takes the pane, and still holds the prompt back —
    // through the answer's event settling.
    app.resolve_shown_for_test(ToolAction::Deny);
    app.settle();
    assert!(
        app.shown_call().is_some(),
        "premise: the second was promoted"
    );
    assert_eq!(app.pending.len(), 1, "a decision is still on screen");

    app.resolve_shown_for_test(ToolAction::Deny);
    app.settle();
    assert!(app.shown_call().is_none(), "premise: answered");
    assert!(
        app.pending.is_empty(),
        "the queued prompt started rather than waiting for a turn that will never end"
    );
    assert!(app.answering(), "…as a real turn");
}

/// **A decision opens under the session that asked.** It took the first
/// session pane in ring order, so with two sessions side by side the right
/// one's question opened under the left one's conversation (Steven,
/// 2026-09-29).
#[tokio::test]
async fn a_decision_opens_under_the_session_that_asked() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (first, asker) = (panes[0], panes[1]);
    assert_ne!(
        app.pane_session(first).id,
        app.pane_session(asker).id,
        "premise: two sessions, and the one that asks is not the first"
    );

    // A send to the second session alone, which asks.
    app.workspace.toggle_mark(asker);
    app.submit("do the thing");
    let send = app.generation_ids_for_test()[0];
    app.gate_job_tool_call_for_test(send, "mkdir build");
    assert!(app.shown_call().is_some(), "premise: it asked");

    let rects = app
        .workspace
        .rects(ratatui::layout::Rect::new(0, 0, 120, 40));
    let x_of = |kind_or_id: &dyn Fn(&crate::layout::PaneRect) -> bool| {
        rects.iter().find(|r| kind_or_id(r)).map(|r| r.rect.x)
    };
    assert_ne!(x_of(&|r| r.id == first), x_of(&|r| r.id == asker));
    assert_eq!(
        x_of(&|r| r.kind == PaneKind::Tool),
        x_of(&|r| r.id == asker),
        "the decision sits under the pane of the session that asked"
    );

    // Two panes on ONE session: the one you are standing in hosts it.
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let standing = panes[1];
    app.workspace.focus_pane(standing);
    manual_tool(&mut app, "mkdir build");
    let rects = app
        .workspace
        .rects(ratatui::layout::Rect::new(0, 0, 120, 40));
    let x_of = |kind_or_id: &dyn Fn(&crate::layout::PaneRect) -> bool| {
        rects.iter().find(|r| kind_or_id(r)).map(|r| r.rect.x)
    };
    assert_ne!(x_of(&|r| r.id == panes[0]), x_of(&|r| r.id == standing));
    assert_eq!(
        x_of(&|r| r.kind == PaneKind::Tool),
        x_of(&|r| r.id == standing),
        "the pane you stand in, not the first on that session"
    );
}

/// **A deny pane says its own reason, and offers no allow key.** `a` said
/// "denied by a rule" whatever denied it, and the hint for any other key
/// advertised `a/A allow` on a pane that has none (Reviewer F, round 17).
#[tokio::test]
async fn a_deny_pane_names_its_reason_and_no_allow_key() {
    let mut app = test_app();
    app.dispatch_command_for_test("deny touch /tmp/zz-y");
    model_call(&mut app, "tu_1", "touch /tmp/zz-y");
    // Cleared: the `:deny` notice names the rule too, and a check that read
    // it passed with the cause deleted (mutation U3).
    app.notices.clear();
    tool_key(&mut app, 'a');
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("denied by rule: deny touch /tmp/zz-y") && notice.contains("`e` edits"),
        "{notice:?}"
    );
    app.notices.clear();
    tool_key(&mut app, 'x');
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("not a decision key") && !notice.contains("allow"),
        "{notice:?}"
    );
}

/// **On a shipped ask, the pane offers no rule it cannot keep.** `find -exec`
/// is carved out by `ask find:-exec`, which outranks any allow; the hint
/// advertised `a/A allow`, and `A` then offered an `:allow` that could not
/// stop the asking (Reviewer F, round 18). The hint lists `a` alone, and `A`
/// says why there is no rule.
#[tokio::test]
async fn a_shipped_ask_offers_once_and_no_rule() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", r"find build -exec rm -rf build/out \;");
    app.notices.clear();
    tool_key(&mut app, 'x');
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("a allow once") && !notice.contains("a/A"),
        "{notice:?}"
    );
}

/// **A rule already in force still answers the open call** — why it still
/// asks — as the first `:allow` of it did (Reviewer F, round 17).
#[tokio::test]
async fn a_rule_already_in_force_still_says_why_the_call_asks() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "sudo cat /etc/hosts");
    assert!(
        app.shown_call().is_some(),
        "premise: a privilege wrapper asks"
    );
    app.dispatch_command_for_test("allow cat *");
    app.dispatch_command_for_test("allow cat *");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("already in force") && notice.contains("still asks"),
        "{notice:?}"
    );
    // And a session's own rule, taken out, is said to be just that.
    app.dispatch_command_for_test("unallow cat *");
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("removed — it was this session's"),
        "{:?}",
        app.notice()
    );
}

/// **A command in a notice is spelled as its pane spells it** (review stop 8,
/// E2 2) — `:qa`'s list and a rule's revisit quoted it through the log's
/// scrub, which writes your home as `~`, beside the pane's literal path: one
/// call spelled two ways on one screen (Steven, 10-20: "it should show the
/// literal").
#[tokio::test]
async fn a_command_in_a_notice_is_spelled_as_its_pane_spells_it() {
    let home = shannon_core::secrets::home_dir().expect("tests run with a HOME");
    let command = format!("mkdir -p {home}/shannon-test-x");
    let mut app = test_app();
    manual_tool(&mut app, &command);
    assert!(app.shown_call().is_some(), "premise: it asks");
    app.notices.clear();
    app.dispatch_command_for_test("qa");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains(&format!("`{command}` is waiting on your decision")),
        "{said}"
    );
    app.notices.clear();
    app.dispatch_command_for_test("deny mkdir:*");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains(&format!("`{command}`")) && !said.contains("~/shannon-test-x"),
        "{said}"
    );
}

/// **An edit to your home's full path runs as typed, and its record shows it
/// as typed** (Steven, 10-20, image 24: "I edited `ls ~` to `ls
/// /Users/…` and it ran `ls ~`"; then "it should show the literal").
/// What Shannon tells Claude Code to run is the line as edited. The records
/// on screen — the allow, and what ran with its note — show the path as
/// written; what is kept and sent says `~` (`[secrets] alias_home`), the
/// model's note included. They showed `~` too, and the edit vanished: "you
/// asked for: \ls ~/src / what actually ran: \ls ~/src".
#[tokio::test]
async fn an_edit_to_the_home_path_runs_and_shows_as_typed() {
    use shannon_core::Block;
    use shannon_core::hook::Reply;
    let home = shannon_core::secrets::home_dir().expect("tests run with a HOME");
    let edited = format!("ls {home}/src");
    let (mut app, provider) = provider_app();
    provider_asks(
        &mut app,
        "tu_1",
        "Bash",
        serde_json::json!({"command": "ls ~/src", "description": "d"}),
    );
    let seed = app.shown_call().expect("asks").command().to_string();
    // What `edit_tool_command` does once $EDITOR returns.
    app.apply_edit_for_test(&seed, &edited);
    tool_key(&mut app, 'a');
    let answered = provider.answered();
    let [
        (
            id,
            Reply::Allow {
                updated_input: Some(input),
            },
        ),
    ] = answered.as_slice()
    else {
        panic!("one allow, with the edit: {answered:?}");
    };
    assert_eq!(id, "tu_1");
    assert_eq!(input["command"], edited.as_str(), "as typed");
    let running = app.output().lines().join("\n");
    assert!(
        running.contains(&format!(
            "runs (edited, replaces what the model asked): {edited}"
        )),
        "the allow, as typed: {running}"
    );

    // Claude Code's record of it: what the model asked, and its output.
    app.feed_stream(StreamMsg::ToolRecord {
        call: Block::ToolUse {
            id: "tu_1".into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "ls ~/src", "description": "d"}),
        },
        result: Block::ToolResult {
            tool_use_id: "tu_1".into(),
            content: "Fun\n".into(),
            is_error: false,
        },
    });
    let shown = app.output().lines().join("\n");
    assert!(
        shown.contains("⚙ Claude Code ran (edited, replaces what the model asked):"),
        "its record, in the allow's place: {shown}"
    );
    assert!(
        shown.contains(&format!("what actually ran: {edited}"))
            && shown.contains("you asked for: ls ~/src"),
        "the note, as typed against as asked: {shown}"
    );
    let sent = tool_results(&app)
        .into_iter()
        .find(|(call, ..)| call == "tu_1")
        .map(|(_, content, _)| content)
        .expect("the result, kept");
    assert!(
        sent.contains("what actually ran: ls ~/src") && !sent.contains(home),
        "kept and sent as `~`: {sent}"
    );
}

/// **Shannon's own edited call shows the home's path as typed too** — the
/// note on screen (`handle_exec_outcome`'s `shown`) against the model's
/// result, which keeps `~`.
#[tokio::test]
async fn a_shannon_edit_to_the_home_path_shows_as_typed() {
    let home = shannon_core::secrets::home_dir().expect("tests run with a HOME");
    let edited = format!("ls {home}/src");
    let mut app = test_app();
    model_call(&mut app, "tu_1", "ls ~/src");
    let seed = (app.shown_call().expect("premise: it asks"))
        .command()
        .to_string();
    app.apply_edit_for_test(&seed, &edited);
    tool_key(&mut app, 'a');
    settle(&mut app).await;
    let shown = app.output().lines().join("\n");
    assert!(
        shown.contains(&format!("what actually ran: {edited}")),
        "on screen, as typed: {shown}"
    );
    let (result, _) = tool_result_for(&app, "tu_1").expect("answered");
    // The note alone: the recording executor's stand-in output is the raw
    // command, where the real one's is scrubbed.
    let note = result.split("\n\n").next().unwrap_or_default();
    assert!(
        note.contains("what actually ran: ls ~/src") && !note.contains(home),
        "to the model, as `~`: {result}"
    );
}

/// **Keys drawn as keys answer where you stand** (review stop 17, F M4):
/// squeezed to one row, a decision's keys are on the status row, and from
/// another pane `a` starts an append. There the row names the way to the
/// pane first, and draws the keys as words; the way it names goes there.
#[tokio::test]
async fn a_squeezed_decisions_keys_are_keys_only_from_its_pane() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut config = Config::default();
    config.tui.approval_max_height = 1;
    let mut app = test_app_with(config);
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    // Where the row has `needle`, whether `key` (a character after its
    // start) is drawn differently from the character before the keys.
    let key_stands_out = |app: &mut App, needle: &str, key: usize| {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        draw_frame(app, &mut terminal);
        let buffer = terminal.backend().buffer().clone();
        let y = buffer.area().height - 1;
        let row: Vec<String> = (0..buffer.area().width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect();
        let text = row.concat();
        let at = (text.find(needle)).unwrap_or_else(|| panic!("{needle} in {text:?}"));
        let x = |offset: usize| text[..at + offset].chars().count() as u16;
        buffer[(x(key), y)].style() != buffer[(x(key) - 1, y)].style()
    };
    app.set_focus(PaneKind::Tool);
    assert!(
        key_stands_out(&mut app, "DECIDE: d deny", "DECIDE: ".len()),
        "premise: in its pane the keys are drawn as keys"
    );
    app.set_focus(PaneKind::Prompt);
    // By its number: bare, `:decide` can pick another session's decision
    // than the one this row is about (the close verifier's M2).
    let number = app.current.number();
    let lead = format!("DECIDE: `:decide {number}`, then ");
    let needle = format!("{lead}d deny");
    assert!(
        !key_stands_out(&mut app, &needle, lead.len()),
        "from elsewhere they are words"
    );
    app.dispatch_command_for_test(&format!("decide {number}"));
    assert_eq!(app.focus(), PaneKind::Tool, "the way it names goes there");
}

/// **A deny says what lifts it, and an ask with no `A` says why — on the
/// pane** (Steven, 10-11: `cat .env` denied "without a means to escape";
/// `git -c core.pager=less log` with no `A` and no reason). And the lift it
/// names is true: typing it answers the call.
#[tokio::test]
async fn a_held_call_says_on_the_pane_what_would_lift_it() {
    use shannon_core::permission::Decision;
    let body = |app: &App| -> String {
        let prompt = app.shown_call().expect("a pane");
        prompt
            .text()
            .lines()
            .iter()
            .map(|l| l.text() + "\n")
            .collect()
    };
    let (mut app, provider) = provider_app();
    let dir = TempDir::new().unwrap();
    app.start = Some(dir.path().to_path_buf());
    app.rebuild_rules();
    let cat = serde_json::json!({"command": "cat .env"});
    provider_asks(&mut app, "tu_1", "Bash", cat);
    assert_eq!(app.shown_call().map(|p| p.decision()), Some(Decision::Deny));
    let shown = body(&app);
    assert!(
        shown.contains("to lift it: `:allow :read .env` lifts it, or `[claude_code] path_policy`"),
        "{shown}"
    );
    assert!(
        shown.contains("matches a pattern that commonly holds credentials"),
        "{shown}"
    );
    assert!(
        !shown.contains("runs it once"),
        "a deny has no `a`: {shown}"
    );
    // The named lift is a real one.
    let said = app.add_session_rule("allow", ":read .env");
    assert!(said.is_ok(), "{said:?}");
    assert!(
        app.shown_call().is_none(),
        "the lift answered the call: {said:?}"
    );
    assert_eq!(provider.answered(), vec![allowed("tu_1")]);

    // An ask no allow lifts: the reason is on the pane, not behind `A`.
    let pager = serde_json::json!({"command": "git -c core.pager=less log"});
    provider_asks(&mut app, "tu_2", "Bash", pager);
    let shown = body(&app);
    assert!(
        shown.contains("no A: no allow lifts this — `a` runs it once")
            && shown.contains("names a program it runs — this asks whatever a rule allows"),
        "{shown}"
    );
    assert!(!shown.contains("code no rule covers"), "{shown}");
}

/// **A rule names exactly what it names, and the pane says what covers the
/// rest** (Steven, 10-11: `allow lsd -a` read as covering `lsd -a .`; kept
/// exact, the `*` form named).
#[tokio::test]
async fn an_exact_allow_names_the_star_form_for_what_it_leaves() {
    let mut app = test_app();
    let dir = TempDir::new().unwrap();
    app.start = Some(dir.path().to_path_buf());
    app.rebuild_rules();
    assert!(app.add_session_rule("allow", "lsd -a").is_ok());
    model_call(&mut app, "tu_1", "lsd -a .");
    let prompt = app.shown_call().expect("it asks");
    let shown: String = prompt
        .text()
        .lines()
        .iter()
        .map(|l| l.text() + "\n")
        .collect();
    assert!(
        shown.contains("which does not cover: word . — asking for that · `allow lsd -a *` covers"),
        "{shown}"
    );
    // And the form it names does answer it.
    assert!(app.add_session_rule("allow", "lsd -a *").is_ok());
    assert!(app.shown_call().is_none(), "the `*` form covered it");
}

/// **Met from above, a guard is said as held, not matched**: `.` with
/// Shannon's state below it "holds Shannon's own state", where it read as
/// "`.` matches a pattern that commonly holds credentials" — false twice
/// (Steven's test, 10-11, state under the start). No grant lifts it from
/// above, so the knob alone is named.
#[tokio::test]
async fn a_place_above_shannons_state_is_said_to_hold_it() {
    let (mut app, _provider) = provider_app();
    let dir = TempDir::new().unwrap();
    let state = dir.path().join("state/shannon");
    std::fs::create_dir_all(&state).unwrap();
    app.start = Some(dir.path().to_path_buf());
    app.protect_paths(vec![state.display().to_string()]);
    // A search: what the files hold reaches the output (a listing, `ls .`,
    // reaches no guard below it since 10-11).
    let rg = serde_json::json!({"command": "rg foo ."});
    provider_asks(&mut app, "tu_1", "Bash", rg);
    let prompt = app.shown_call().expect("it asks");
    let shown: String = prompt
        .text()
        .lines()
        .iter()
        .map(|l| l.text() + "\n")
        .collect();
    assert!(
        shown.contains("`.` holds Shannon's own state, which this may reach"),
        "{shown}"
    );
    assert!(!shown.contains("matches a pattern"), "{shown}");
    assert!(
        !shown.contains("lifts it, or"),
        "no grant from above: {shown}"
    );
    assert!(
        shown.contains("`[claude_code] path_policy` decides these"),
        "{shown}"
    );
    assert_eq!(
        shown.matches("which this may reach").count(),
        1,
        "the `no A` line does not repeat the finding: {shown}"
    );

    // Named outright, it is Shannon's own state.
    tool_key(&mut app, 'd');
    let cat = format!("cat {}/x", state.display());
    provider_asks(
        &mut app,
        "tu_2",
        "Bash",
        serde_json::json!({"command": cat}),
    );
    let prompt = app.shown_call().expect("a pane");
    let shown: String = prompt
        .text()
        .lines()
        .iter()
        .map(|l| l.text() + "\n")
        .collect();
    assert!(shown.contains("is Shannon's own state"), "{shown}");
}

/// **`e` is struck through where it cannot apply, and only there** — a
/// Claude Code call other than `Bash`. Asserted on the rendered cells: a style
/// the view never paints passes every test of the role behind it.
#[tokio::test]
async fn the_edit_key_is_struck_through_where_it_cannot_apply() {
    use ratatui::style::Modifier;
    let struck = |app: &mut App| {
        let buf = frame_of(app, 140, 40);
        // `e edit` after a legend's `answer:`, on its row.
        let at = (cells_of(&buf, "answer:"))
            .find_map(|legend| {
                (cells_of(&buf, "e edit")).find(|c| c.y == legend.y && c.x > legend.x)
            })
            .expect("a legend offering `e edit` on screen");
        buf[at].style().add_modifier.contains(Modifier::CROSSED_OUT)
    };

    let (mut app, _) = provider_app();
    provider_asks(
        &mut app,
        "tu_1",
        "Read",
        serde_json::json!({"file_path": ".env"}),
    );
    assert!(struck(&mut app), "a Read takes no edit");

    let (mut app, _) = provider_app();
    provider_asks(
        &mut app,
        "tu_2",
        "WebFetch",
        serde_json::json!({"url": "https://x.example"}),
    );
    assert!(struck(&mut app), "nor does a call the rules cannot read");

    let (mut app, _) = provider_app();
    let rm = serde_json::json!({"command": "rm -rf /tmp/shannon-test-absent"});
    provider_asks(&mut app, "tu_3", "Bash", rm);
    assert!(!struck(&mut app), "Claude Code's Bash takes an edit");

    let mut app = test_app();
    model_call(&mut app, "tu_4", "rm -rf /tmp/shannon-test-absent");
    assert!(!struck(&mut app), "Shannon's own call takes an edit");
}

/// **`e` on a Claude Code Write or Edit edits what it writes** (Steven,
/// 10-05: "`e` can open in `$EDITOR`"): the content, or the new text, opens
/// as its file's language; saved, the pane shows the edit under the same
/// verdict (the gate reads the call as `cat > <file>`, and the file is the
/// same), `e` again opens the edit, and the allow tells Claude Code to write
/// it — the rest of the input as the model gave it.
#[tokio::test]
async fn e_on_a_claude_code_write_edits_what_it_writes() {
    use shannon_core::hook::Reply;
    let path = "/tmp/shannon-test-absent/a.rs";
    let shown = |app: &App| -> String {
        app.shown_call()
            .map(|p| {
                p.text()
                    .lines()
                    .iter()
                    .map(|l| l.text())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };

    let (mut app, provider) = provider_app();
    let write = serde_json::json!({"file_path": path, "content": "fn main() {}\n"});
    provider_asks(&mut app, "tu_1", "Write", write);
    let verdict = app.shown_call().map(|p| p.decision());
    let (seed, filetype) = app.edit_seed_for_test().expect("a call is open");
    assert_eq!(seed, "fn main() {}\n");
    assert_eq!(filetype, crate::editor::Filetype::Code("rs"));
    // The editor hands its text back without the final newline.
    app.apply_edit_for_test(&seed, "fn main() {\n    run();\n}");
    let after = shown(&app);
    assert!(after.contains("+     run();"), "{after}");
    assert!(
        !after.contains("+ fn main() {}"),
        "the model's text is gone: {after}"
    );
    assert_eq!(app.shown_call().map(|p| p.decision()), verdict);
    let (again, _) = app.edit_seed_for_test().expect("still open");
    assert_eq!(
        again, "fn main() {\n    run();\n}\n",
        "`e` again opens the edit"
    );
    tool_key(&mut app, 'a');
    let answered = provider.answered();
    assert!(
        matches!(
            &answered[..],
            [(id, Reply::Allow { updated_input: Some(input) })]
                if id == "tu_1"
                    && input["content"] == "fn main() {\n    run();\n}\n"
                    && input["file_path"] == path
        ),
        "{answered:?}"
    );

    let (mut app, provider) = provider_app();
    let edit = serde_json::json!({"file_path": path, "old_string": "a()", "new_string": "b()"});
    provider_asks(&mut app, "tu_2", "Edit", edit);
    let (seed, _) = app.edit_seed_for_test().expect("a call is open");
    assert_eq!(seed, "b()");
    app.apply_edit_for_test(&seed, "b()");
    assert!(app.notice().unwrap_or_default().contains("unchanged"));
    app.apply_edit_for_test(&seed, "c()");
    tool_key(&mut app, 'a');
    let answered = provider.answered();
    assert!(
        matches!(
            &answered[..],
            [(_, Reply::Allow { updated_input: Some(input) })]
                if input["new_string"] == "c()" && input["old_string"] == "a()"
        ),
        "{answered:?}"
    );
    // The record says the user's text replaced the model's (review of
    // bc2b532, Reviewer C: nothing held it).
    let shown = app.output().lines().join("\n");
    assert!(
        shown.contains("edited, replaces what the model asked"),
        "{shown}"
    );

    // The editor's round trip takes one final newline off; one is given
    // back exactly when the seed ended in one (review of bc2b532, Reviewer
    // A): a blank last line saved untouched is unchanged, and one added is
    // an edit.
    for (seed_text, saved, sent) in [
        ("a\n\n", "a\n", None),
        ("x\n", "x\n", Some("x\n\n")),
        ("x", "x", None),
    ] {
        let (mut app, provider) = provider_app();
        let edit =
            serde_json::json!({"file_path": path, "old_string": "o", "new_string": seed_text});
        provider_asks(&mut app, "tu_3", "Edit", edit);
        let (seed, _) = app.edit_seed_for_test().expect("a call is open");
        app.apply_edit_for_test(&seed, saved);
        tool_key(&mut app, 'a');
        let answered = provider.answered();
        let got = match &answered[..] {
            [(_, Reply::Allow { updated_input })] => updated_input
                .as_ref()
                .and_then(|i| i["new_string"].as_str().map(str::to_string)),
            other => panic!("{other:?}"),
        };
        assert_eq!(got.as_deref(), sent, "{seed_text:?} saved as {saved:?}");
    }
}

/// **`e` opens a long command as the pane lays it out**, as shell — its runs
/// of flags each a ` \` continuation — and saving it as opened changes
/// nothing (Steven, 10-05: "are we able to pass that pretty-printed variant
/// to `$EDITOR` when editing? That'll make editing easier"). Under
/// `tui.pretty_commands = false` it opens as written.
#[tokio::test]
async fn e_opens_a_long_command_as_the_pane_lays_it_out() {
    let long = "rm --recursive --force --verbose --one-file-system \
                --preserve-root=all /tmp/shannon-test-absent/first-thing-to-remove \
                /tmp/shannon-test-absent/second-thing";
    let mut app = test_app();
    model_call(&mut app, "tu_1", long);
    let (seed, filetype) = app.edit_seed_for_test().expect("a call is open");
    assert_eq!(filetype, crate::editor::Filetype::Shell);
    assert!(seed.lines().count() > 1, "{seed}");
    assert!(
        seed.lines().rev().skip(1).all(|l| l.ends_with(" \\")),
        "{seed}"
    );
    app.apply_edit_for_test(&seed, &format!("{seed}\n"));
    assert!(
        app.notice().unwrap_or_default().contains("unchanged"),
        "saved as opened: {:?}",
        app.notice()
    );
    // A `:set` since it was gated does not re-lay what `e` opens: the pane
    // keeps its breaks, and `e` opens the layout on screen (review of
    // 716f4f9, A-L12).
    app.dispatch_command_for_test("set tui.command_width=0");
    assert_eq!(app.config.tui.command_width, 0, "premise: it was set");
    let (again, _) = app.edit_seed_for_test().expect("still open");
    assert_eq!(again, seed, "the pane's layout, not the new setting's");

    let mut config = Config::default();
    config.tui.pretty_commands = false;
    let mut plain = test_app_with(config);
    model_call(&mut plain, "tu_1", long);
    assert_eq!(
        plain.edit_seed_for_test().map(|(seed, _)| seed).as_deref(),
        Some(long)
    );
}

/// **A Claude Code write is approved seeing what it writes** (Steven, 10-05:
/// the pane showed `run this? — cat > '…'`, a command beside commands you
/// can edit, and nothing of the write). Asserted on the drawn cells: the
/// title names the tool and the file, the diff's markers carry their colour,
/// and `A`'s rule names the write and is shown in full.
#[tokio::test]
async fn a_claude_code_write_shows_what_it_writes() {
    use crate::palette;
    use ratatui::style::Modifier;
    let screen = |app: &mut App| frame_of(app, 140, 40);
    let row_of =
        |buf: &ratatui::buffer::Buffer, text: &str| cell_of(buf, text).map(|at| (at.x, at.y));
    let path = "/tmp/shannon-test-absent/a.rs";

    let (mut app, _) = provider_app();
    let write = serde_json::json!({"file_path": path, "content": "fn main() {}\nlet x = 1;"});
    provider_asks(&mut app, "tu_1", "Write", write);
    let buf = screen(&mut app);
    assert!(
        row_of(&buf, &format!("write this? — Claude Code Write {path}")).is_some(),
        "titled as the tool and its file, not as `cat >`"
    );
    let (x, y) = row_of(&buf, "+ fn main() {}").expect("the content is shown");
    assert_eq!(
        buf[(x, y)].style().fg,
        Some(palette::SAFE),
        "an added line's marker"
    );
    assert_eq!(
        app.shown_call().map(|p| p.proposal().clone()),
        Some(Ok(format!(":write {path}"))),
        "`A` grants the write, not a `cat`"
    );
    assert!(
        row_of(&buf, &format!("A adds, this session: allow :write {path}")).is_some(),
        "the rule `A` adds, in full"
    );

    let (mut app, _) = provider_app();
    let edit =
        serde_json::json!({"file_path": path, "old_string": "old line", "new_string": "new line"});
    provider_asks(&mut app, "tu_2", "Edit", edit);
    let buf = screen(&mut app);
    let (x, y) = row_of(&buf, "- old line").expect("the replaced text is shown");
    assert_eq!(
        buf[(x, y)].style().fg,
        Some(palette::DANGER),
        "a removed line's marker"
    );
    let text = row_of(&buf, "old line").expect("its text");
    assert_ne!(
        buf[text].style().fg,
        Some(palette::DANGER),
        "removed text is not red: red is the verdict's colour here"
    );
    assert!(
        !buf[text].style().add_modifier.contains(Modifier::DIM),
        "removed text at full contrast, not faint (S21-Q16)"
    );
    assert!(row_of(&buf, "+ new line").is_some());
    assert!(row_of(&buf, &format!("write this? — Claude Code Edit {path}")).is_some());

    // Any other tool is named as itself; only a command is "run this?". And
    // what the body shows, the title does not repeat (Steven, 10-05, image
    // 92): the reading of a Read, the command of a Bash call.
    let (mut app, _) = provider_app();
    // (An ordinary read would not ask at all; a credential path does.)
    provider_asks(
        &mut app,
        "tu_3",
        "Read",
        serde_json::json!({"file_path": ".env"}),
    );
    // At the decision: unfocused, its title adds how to reach it. Whose call
    // it is ends the title (`ToolPrompt::whose`) — not a repeat of the body.
    to_decision(&mut app);
    let buf = screen(&mut app);
    let tag = app.session_tag(app.conversation().episode().id);
    let (_, title) = row_of(&buf, &format!("allow this? — Claude Code Read · {tag} ─"))
        .expect("named, and no more");
    let (_, body) = row_of(&buf, "cat '.env'").expect("the reading, in the body");
    assert!(body > title);
    let (mut app, _) = provider_app();
    let rm = serde_json::json!({"command": "rm -rf /tmp/shannon-test-absent"});
    provider_asks(&mut app, "tu_4", "Bash", rm);
    to_decision(&mut app);
    let buf = screen(&mut app);
    let tag = app.session_tag(app.conversation().episode().id);
    let (_, title) =
        row_of(&buf, &format!(" run this? · {tag} ─")).expect("the question, and no more");
    let (_, body) = row_of(&buf, "rm -rf /tmp/shannon-test-absent").expect("the command");
    assert!(body > title, "the command is in the body, not the title");
}

/// **The pane colours a command by its syntax, under its danger** (Steven,
/// 10-05, image 92: "We should have syntax highlighting here too"). Read off
/// the drawn cells: the yellow block is the alarm, on the word it is about —
/// `mkdir`, a program no model reads — and no longer the whole stage
/// (Steven, 10-11: "highlight less"), so its flag takes its syntax; a safe
/// stage's green goes on its command's name, the claim's subject, and its
/// flags, strings and plumbing take their syntax.
#[tokio::test]
async fn the_pane_colours_a_command_by_its_syntax_under_its_danger() {
    use crate::palette;
    use ratatui::style::Modifier;
    let mut app = test_app();
    // (An unresolved operand would make `rg` a warning — `stage_tone` — so
    // the expansion is `echo`'s, which makes no claim.)
    model_call(
        &mut app,
        "tu_1",
        "mkdir -p build && rg --count 'TODO' src | head -5; echo \"at $DIR\"",
    );
    app.set_focus(PaneKind::Tool);
    let buf = frame_of(&mut app, 120, 30);
    // The cell where `needle` first starts on screen.
    let cell = |needle: &str| {
        buf[cell_of(&buf, needle).unwrap_or_else(|| panic!("{needle} is drawn"))].clone()
    };
    let mkdir = cell("mkdir");
    assert_eq!(
        mkdir.style().bg,
        Some(palette::CAUTION),
        "the warning, on what it is about"
    );
    let rg = cell("rg --count");
    assert_eq!(rg.style().fg, Some(palette::SAFE), "green, on the name");
    assert!(rg.style().add_modifier.contains(Modifier::BOLD));
    // The rest as the app's theme paints the decision pane.
    let pane = |ink| app.theme().paint(crate::theme::Context::Decision, ink).fg;
    use crate::style::Ink;
    let p = cell("-p");
    assert_eq!(
        (p.style().fg, p.style().bg),
        (pane(Ink::Parameter), Some(ratatui::style::Color::Reset)),
        "the warning's flag, in its syntax"
    );
    assert_eq!(cell("--count").style().fg, pane(Ink::Parameter));
    assert_eq!(cell("'TODO'").style().fg, pane(Ink::String));
    assert_eq!(cell("$DIR").style().fg, pane(Ink::VariableSpecial));
    assert_eq!(cell("| head").style().fg, pane(Ink::Operator));
    assert_eq!(cell("-5").style().fg, pane(Ink::Parameter));
    assert_eq!(cell("&&").style().fg, pane(Ink::Operator));
    // …each a colour, and no two of the four alike.
    let hues = [
        Ink::Parameter,
        Ink::String,
        Ink::VariableSpecial,
        Ink::Operator,
    ]
    .map(pane);
    for (i, a) in hues.iter().enumerate() {
        assert!(a.is_some(), "{hues:?}");
        assert!(!hues[i + 1..].contains(a), "{hues:?}");
    }
}

/// **The configured breaks reach every layout** — the pane, `e`'s seed and
/// the record (`Breaks::of`, `Breaks::for_records`): a knob one consumer
/// reads is the half-built setting (`[[feedback_setting_has_three_halves]]`).
#[tokio::test]
async fn the_configured_breaks_reach_the_pane_the_editor_and_the_record() {
    let command = "frobnicate --alpha=1 --beta=2 --gamma=3 --delta=4 --epsilon=5 --zeta=6 target";
    let mut config = Config::default();
    config.tui.flags_per_line = 1;
    let mut app = test_app_with(config);
    model_call(&mut app, "tu_1", command);

    let screen = text_of(&mut app, 120, 40);
    for flag in ["--beta=2", "--zeta=6 target"] {
        assert!(
            screen
                .lines()
                .any(|row| row.trim_start_matches('│').trim().starts_with(flag)),
            "{flag} leads a line of the pane\n{screen}"
        );
    }

    let (seed, _) = app.edit_seed_for_test().expect("a command to edit");
    assert_eq!(
        seed.lines().count(),
        6,
        "one flag a line, as shell:\n{seed}"
    );

    // Re-gated after an edit, and refreshed after a policy change, it is
    // laid out as it was first.
    let rules = app.rules.clone();
    let rows = |app: &App| app.shown_call().map(|p| p.text().lines().len());
    let first = rows(&app);
    app.shown_call_mut().unwrap().re_gate(command, &rules);
    assert_eq!(rows(&app), first, "re-gated");
    app.shown_call_mut().unwrap().refresh(&rules);
    assert_eq!(rows(&app), first, "refreshed");

    tool_key(&mut app, 'd');
    let lines = app.output().lines();
    let block = lines
        .iter()
        .position(|l| l == "  ```bash")
        .expect("the denial's command block");
    assert_eq!(lines[block + 1], "  frobnicate --alpha=1 \\");
    assert_eq!(lines[block + 2].trim(), "--beta=2 \\");
}

/// **A decision says its keys once, and its command once** (Steven, 10-05,
/// image 92: the title repeated the command the pane showed, and the status
/// row said `d deny · a allow` twice beside a pane saying it a third time).
/// Where the pane is too short for either, it still says each once: the
/// command in the title, the keys on the status row — the CRITICAL in
/// `TODO.md` (a one-row pane showing only its verdict) must not be made worse
/// by the decluttering.
#[tokio::test]
async fn a_denied_call_in_a_sliver_offers_no_allow_on_the_row() {
    // Review stop 8, D 4: the row's short keys were verdict-blind, and
    // offered `a allow` on a call the rules deny — a dead key, said where
    // the legend could not be.
    let mut config = Config::default();
    config.tui.approval_max_height = 1;
    let mut app = test_app_with(config);
    app.dispatch_command_for_test("deny touch /tmp/shannon-test-denied");
    model_call(&mut app, "tu_1", "touch /tmp/shannon-test-denied");
    app.set_focus(PaneKind::Tool);
    app.new_moment(); // the `:deny`'s own notice borrows the row
    let screen = text_of(&mut app, 100, 30);
    let status = screen.lines().last().unwrap_or_default();
    assert!(
        status.contains("DECIDE: d deny · e edit") && !status.contains("a allow"),
        "{status:?}\n{screen}"
    );
}

/// **A call's legend cut only in its tail keeps its keys off the row**
/// (review stop 8, F 3) — every answer drawn, `j/k scroll` and `A`'s rule
/// cut: the row says nothing more. Asking whether the WHOLE legend fit put
/// `DECIDE:` on this row with every answer on screen beside it.
#[tokio::test]
async fn a_calls_legend_cut_only_in_its_tail_keeps_the_row_quiet() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "mkdir -p build");
    app.set_focus(PaneKind::Tool);
    app.new_moment();
    let screen = text_of(&mut app, 50, 30);
    let legend = (screen.lines())
        .find(|l| l.contains("answer: "))
        .unwrap_or_else(|| panic!("premise: a legend row\n{screen}"));
    assert!(
        legend.contains("e edit") && !legend.contains("A always"),
        "premise: every answer drawn, the tail cut — {legend:?}"
    );
    let status = screen.lines().last().unwrap_or_default();
    assert!(!status.contains("DECIDE"), "{status:?}\n{screen}");
}

/// **A selection's legend draws its keys as keys**, not the answers it has
/// put aside — the rendered cell (review stop 17, C 7).
#[tokio::test]
async fn a_selections_legend_draws_its_keys_as_keys() {
    let mut app = test_app();
    model_call(&mut app, "tu_1", "mkdir -p build");
    app.set_focus(PaneKind::Tool);
    type_keys(&mut app, "v");
    let buf = frame_of(&mut app, 100, 30);
    let (at, row) = (0..buf.area().height)
        .find_map(|y| {
            let row: String = (0..buf.area().width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            row.contains("selecting — y copies").then_some((y, row))
        })
        .expect("the legend");
    let byte = row.find("y copies").expect("the key");
    let x = row[..byte].chars().count() as u16;
    let code = crate::view::painter(PaneKind::Tool, app.theme())
        .style(&crate::paint::Run::role(
            "x",
            crate::style::CellStyle::ink(crate::style::Ink::HelpCode),
        ))
        .fg;
    assert_eq!(buf[(x, at)].style().fg, code, "the key: {row}");
    assert_ne!(buf[(x + 2, at)].style().fg, code, "its word: {row}");
}

/// **The keys the status row stands in for the legend with are drawn as
/// keys** — a rendered cell, the row's own, past `DECIDE:`.
#[tokio::test]
async fn the_status_rows_keys_are_drawn_as_keys() {
    let mut config = Config::default();
    config.tui.approval_max_height = 1;
    let mut app = test_app_with(config);
    model_call(&mut app, "tu_1", "mkdir -p build");
    app.set_focus(PaneKind::Tool);
    let buf = frame_of(&mut app, 100, 30);
    let y = buf.area().height - 1;
    let row: String = (0..buf.area().width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect();
    let lead = "DECIDE: ";
    let at = row
        .find("DECIDE: d deny")
        .unwrap_or_else(|| panic!("{row}"))
        + lead.len();
    let x = row[..at].chars().count() as u16;
    let code = crate::view::painter(PaneKind::Tool, app.theme())
        .style(&crate::paint::Run::role(
            "x",
            crate::style::CellStyle::ink(crate::style::Ink::HelpCode),
        ))
        .fg;
    assert_eq!(buf[(x, y)].style().fg, code, "the key: {row}");
    assert_ne!(buf[(x + 2, y)].style().fg, code, "its word: {row}");
    assert_ne!(buf[(x - 2, y)].style().fg, code, "the lead-in: {row}");
}

#[tokio::test]
async fn a_decision_says_its_keys_once_and_its_command_once() {
    use ratatui::{Terminal, backend::TestBackend};
    // A flag, so `A`'s rule (`mkdir build`) is not another copy of it.
    let command = "mkdir -p build";
    let drawn = |cap: u16| {
        let mut config = Config::default();
        config.tui.approval_max_height = cap;
        let mut app = test_app_with(config);
        model_call(&mut app, "tu_1", command);
        app.set_focus(PaneKind::Tool);
        let screen = text_of(&mut app, 100, 30);
        let title = screen
            .lines()
            .find(|l| l.contains("run this?"))
            .unwrap_or_else(|| panic!("the pane is drawn\n{screen}"))
            .to_string();
        let status = screen.lines().last().unwrap_or_default().to_string();
        (screen, title, status)
    };

    let (roomy, title, status) = drawn(18);
    assert_eq!(
        roomy.matches(command).count(),
        1,
        "once, in the body\n{roomy}"
    );
    assert!(!title.contains(command), "not in the title: {title:?}");
    assert!(
        roomy.contains("answer: d/q deny"),
        "the pane's legend\n{roomy}"
    );
    assert!(
        !status.contains("deny") && !status.contains("DECIDE"),
        "the legend is on screen; the status row repeats none of it: {status:?}"
    );
    // It said `^Ww leave`, the way out; `^W` leaves every pane alike, so the
    // row names where the pane is explained instead (Steven, 10-14).
    assert!(
        status.contains("help `:help decision`"),
        "where the pane is explained: {status:?}"
    );

    // Capped below the command: the legend and the verdict fit, the command
    // does not, so the title carries it.
    let (tight, title, status) = drawn(3);
    assert!(
        title.contains(command),
        "the body cannot show it, so the title does: {title:?}\n{tight}"
    );
    assert_eq!(tight.matches(command).count(), 1, "still once\n{tight}");
    assert!(!status.contains("DECIDE"), "the legend fits: {status:?}");

    // Capped to one row: no legend either, so the status row has the keys.
    let (sliver, title, status) = drawn(1);
    assert!(title.contains(command), "{sliver}");
    assert!(
        status.contains("DECIDE: d deny · a allow"),
        "no legend row, so the keys are here: {status:?}\n{sliver}"
    );
    assert_eq!(sliver.matches(command).count(), 1, "still once\n{sliver}");

    // Selecting in that sliver: the keys are Visual's, and the row says so
    // rather than offering dead ones.
    let mut config = Config::default();
    config.tui.approval_max_height = 1;
    let mut app = test_app_with(config);
    model_call(&mut app, "tu_1", command);
    app.set_focus(PaneKind::Tool);
    app.key_for_test(
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
        (100, 1),
    );
    assert!(app.decision_selecting(), "premise: selecting");
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let screen = rendered_text(terminal.backend().buffer());
    let status = screen.lines().last().unwrap_or_default();
    assert!(
        status.contains("DECIDE: selecting") && !status.contains("a allow"),
        "{status:?}"
    );

    // A chord pending there: the keys are its own, and the row says so too
    // (review stop 6, F 1).
    for key in [KeyCode::Esc, KeyCode::Char('y')] {
        app.key_for_test(KeyEvent::new(key, KeyModifiers::NONE), (100, 1));
    }
    assert!(!app.decision_selecting(), "premise: the selection ended");
    draw_frame(&mut app, &mut terminal);
    let screen = rendered_text(terminal.backend().buffer());
    let status = screen.lines().last().unwrap_or_default();
    assert!(
        status.contains("DECIDE: finishing a command") && !status.contains("a allow"),
        "{status:?}"
    );
}

/// **The command is in the title whenever the body may not show it all** —
/// scrolled past (review of 716f4f9, C-H4: `TextView::shows`' scroll clause
/// was untested, and without it the command was in neither the body nor the
/// title while the user approved), or wrapped past the rows the body has.
/// A wrap the body shows whole is not repeated: the rows counted are the rows
/// drawn now (one ruler, `PageLine::wrap`), where a wrapped command was once
/// always put in the title because the two could differ (Reviewer A).
#[tokio::test]
async fn a_command_the_body_may_not_show_is_in_the_title() {
    let screen_of = |app: &mut App| text_of(app, 100, 30);
    let title_of = |app: &mut App| {
        let screen = screen_of(app);
        screen
            .lines()
            .find(|l| l.contains("run this?"))
            .unwrap_or_else(|| panic!("the pane is drawn\n{screen}"))
            .to_string()
    };
    let gated = |config: Config, command: &str| {
        let mut app = test_app_with(config);
        model_call(&mut app, "tu_1", command);
        app.set_focus(PaneKind::Tool);
        app
    };

    // Capped so the report overflows (a fit report cannot scroll: the draw
    // clamps it back) with the command still in view at the top.
    let command = "mkdir -p build";
    let mut config = Config::default();
    config.tui.approval_max_height = 6;
    let mut app = gated(config.clone(), command);
    assert!(
        !title_of(&mut app).contains(command),
        "premise: the body shows it"
    );
    // `j` to the end, as the legend says: the window follows the cursor.
    for _ in 0..8 {
        type_keys(&mut app, "j");
    }
    let title = title_of(&mut app);
    assert!(
        app.shown_call().expect("gated").text().scroll() > 2,
        "premise: the report scrolled past the command's row"
    );
    assert!(title.contains(command), "scrolled past it: {title:?}");

    // Never broken, and wider than the pane: it wraps. With room, every
    // word of it is drawn in the body, and the title does not say it again.
    let long = format!("mkdir -p {}", vec!["build/a"; 16].join(" "));
    config.tui.command_width = 0;
    config.tui.approval_max_height = 20;
    let mut app = gated(config.clone(), &long);
    let screen = screen_of(&mut app);
    let title = title_of(&mut app);
    assert!(
        !title.contains("mkdir -p build/a"),
        "shown whole: {title:?}"
    );
    assert_eq!(
        screen.matches("build/a").count(),
        16,
        "every word drawn\n{screen}"
    );
    // Capped short of its rows: the title carries it.
    config.tui.approval_max_height = 5;
    let mut app = gated(config, &long);
    let title = title_of(&mut app);
    assert!(
        title.contains("mkdir -p build/a"),
        "wrapped past: {title:?}"
    );
}

/// **PgDn pages a short decision pane** — sized as production sizes it
/// (`pane_content_size`, the pane's inside, legend and all), after a draw.
/// Docketed 10-05 as "PgDn does not page"; that probe handed the BODY's
/// height, which `text_rows` then shrank by the legend and the marker again.
#[tokio::test]
async fn page_down_pages_a_short_decision_pane() {
    use crate::keymap::ScrollMagnitude::Page;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    let mut config = Config::default();
    config.tui.approval_max_height = 6;
    let mut app = test_app_with(config);
    model_call(&mut app, "tu_1", "mkdir -p build");
    app.set_focus(PaneKind::Tool);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let pane = app.workspace.focused();
    let size = app.pane_content_size(pane, Rect::new(0, 0, 100, 30));
    let scroll = |app: &App| app.shown_call().expect("gated").text().scroll();
    let mut seen = vec![scroll(&app)];
    for _ in 0..3 {
        app.scroll_focused(ScrollCmd::Down(Page), size);
        draw_frame(&mut app, &mut terminal);
        seen.push(scroll(&app));
    }
    assert!(
        seen[1] > seen[0],
        "the first page moves the window: {seen:?}"
    );
    assert!(
        seen.windows(2).all(|w| w[1] >= w[0]),
        "and never back: {seen:?}"
    );
}
