//! **A summary's review as its session's decision**
//! (`docs/DesignDocs/decision-panes-and-alerts.md` §4, A2.2): it arrives in
//! a pane beside its session, taking nothing; its keys answer it there; it
//! holds its session until applied or dropped, and keeps its draft through a
//! failed editor, a close, a hide.

use super::*;

/// A session of two turns, the whole of it marked in a `:graph` over its
/// only transcript, distilled with a review, and the draft come back: its
/// review up. The session's buffer.
pub(super) fn reviewed(app: &mut App) -> BufferId {
    let chat = app.current;
    mark_the_aims_session(app);
    distill_reviewed(app);
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    assert!(
        app.decisions.review(chat).is_some(),
        "premise: its review is up"
    );
    chat
}

/// **A review arrives beside its session, and takes nothing** — no
/// `$EDITOR`, no focus: a pane opens under the window showing its session,
/// its keys waiting as a call's just shown, said; drawn as a call is, its
/// own title and legend over the draft. It took the terminal into
/// `$EDITOR` the moment the model finished, over whatever you were typing
/// (Steven, D 4: "an informed and expected event").
#[tokio::test]
async fn a_review_arrives_beside_its_session_and_takes_nothing() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = app.current;
    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);
    let focused = app.workspace.focused();
    app.feed_summary(StreamMsg::Chunk("the gist\nline two\nline three".into()));
    app.feed_summary(StreamMsg::Complete);

    let pane = app
        .decision_pane_of(chat)
        .expect("a pane beside its session");
    assert_eq!(app.workspace.focused(), focused, "focus stays where it was");
    assert_eq!(app.answer_keys(pane), AnswerKeys::Waiting, "its keys wait");
    assert!(
        app.arming_ends_at().is_some(),
        "and the loop wakes to draw them live"
    );
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("summary written — its review waits in the pane beside it"),
        "{said}"
    );
    app.new_moment();
    let shown = screen(&mut app, 100);
    assert!(
        shown.contains("review this summary? — 2 turns of"),
        "{shown}"
    );
    assert!(shown.contains("the gist"), "the draft: {shown}");
    assert!(
        shown.contains("<CR> review · a apply · d drop"),
        "its keys: {shown}"
    );

    // Its page moves as any page does.
    app.focus_pane(pane);
    app.key_for_test(
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        (80, 10),
    );
    assert_eq!(
        app.text_pane(pane).map(|t| t.cursor()),
        Some(1),
        "`j` moves through the draft"
    );
}

/// **A review waits for every one of its session's calls** — with one
/// allowed and running and another waiting, the pane is the calls', not
/// the review's: shown, the review would be replaced under your cursor
/// again when the next call came up.
#[tokio::test]
async fn a_review_waits_for_every_call_of_its_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    manual_tool(&mut app, "rm /tmp/shannon-test-absent-1");
    manual_tool(&mut app, "rm /tmp/shannon-test-absent-2");
    app.settle();
    tool_key_in(&mut app, chat, 'a');
    assert!(
        app.decisions.waiting_of(chat).next().is_some(),
        "premise: the second call waits"
    );
    assert!(
        !matches!(app.decisions.on_pane(chat), Some(OnPane::Review(_))),
        "not the review while a call waits"
    );
}

/// **A waiting review is a fact of its session** (B1.2) — `review` is 1,
/// and it is one of the session's decisions, beside each call waiting.
#[tokio::test]
async fn a_waiting_review_is_a_fact_of_its_session() {
    use crate::facts::SessionFact;
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    assert_eq!(app.session_fact(SessionFact::Review, chat), 1);
    assert_eq!(app.session_fact(SessionFact::Decision, chat), 1);
    manual_tool(&mut app, "rm /tmp/shannon-test-absent-1");
    app.settle();
    assert_eq!(
        app.session_fact(SessionFact::Decision, chat),
        2,
        "and a call"
    );
}

/// **Over a review in its pane, setup's legend says one waits** — a
/// decision on screen, as a call's; setup opens over it (Steven, 10-26: "I'd
/// open it up so these don't block"; refused, DR-A #11, until then).
#[tokio::test]
async fn setup_over_a_review_in_its_pane_says_one_waits() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    reviewed(&mut app);
    assert!(app.decision_in_a_pane(), "premise: a decision up");
    app.dispatch_command_for_test("setup");
    assert!(app.setup().is_some(), "opened over it");
    let shown = screen(&mut app, 120);
    assert!(shown.contains("a decision is waiting"), "{shown}");
}

/// **A review's keys wait, as a call's do** — `a` pressed the moment its
/// pane comes up, meant for what was there a moment ago, applies nothing,
/// and says the review is still open.
#[tokio::test]
async fn a_reviews_keys_wait_as_a_calls_do() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    app.focus_pane(pane);
    assert_eq!(app.answer_keys(pane), AnswerKeys::Waiting, "premise");
    let effect = app.handle_tool_key(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        (80, 100),
    );
    assert_eq!(effect, ToolKeyEffect::None);
    assert!(app.decisions.review(chat).is_some(), "nothing applied");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("not answered — still open"), "{said}");
}

/// **Answered, a review's pane goes with it** — at the event's end, as a
/// call's does when nothing is next: `a` composing, `d` dropping. `d` from
/// inside it leaves you at the prompt, never in another decision (DR-A #6).
#[tokio::test]
async fn an_answered_review_takes_its_pane_with_it() {
    for key in ['a', 'd'] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 2);
        let chat = reviewed(&mut app);
        let pane = app.decision_pane_of(chat).expect("premise: its pane");
        review_key_in(&mut app, chat, KeyCode::Char(key));
        assert!(app.decisions.review(chat).is_none(), "`{key}`: answered");
        assert_eq!(app.workspace.kind_of(pane), None, "`{key}`: its pane goes");
        if key == 'd' {
            assert_eq!(app.focus(), PaneKind::Prompt, "`d`: to the prompt");
        }
    }
}

/// **A review's pane too short for its legend gives its own keys on the
/// row** — `<CR> review · a apply · d drop`, not a call's `d deny · a
/// allow`: there `d` would read as refusing a command.
#[tokio::test]
async fn a_review_in_a_sliver_gives_its_own_keys_on_the_row() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    std::sync::Arc::make_mut(&mut app.config)
        .tui
        .approval_max_height = 1;
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    app.focus_pane(pane);
    app.new_moment();
    let shown = text_of(&mut app, 100, 30);
    let row = shown.lines().last().unwrap_or_default();
    assert!(
        row.contains("DECIDE: <CR> review · a apply · d drop"),
        "no legend row, so its keys are here: {row:?}\n{shown}"
    );
}

/// **A review's legend cut before an answer gives its keys on the row**
/// (review stop 8, F 3) — in the `:graph` layout `S` is pressed from, the
/// pane is half the terminal, its legend was cut before `d drop`, and the
/// row, seeing a legend, said nothing: `d drop` was on no screen. Forty
/// columns: the legend's inside is 38, and its answers run to 40.
#[tokio::test]
async fn a_review_whose_legend_is_cut_before_an_answer_gives_its_keys_on_the_row() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    app.focus_pane(pane);
    app.new_moment();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 30)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let shown = rendered_text(terminal.backend().buffer());
    let legend = (shown.lines())
        .find(|l| l.contains("answer: "))
        .unwrap_or_else(|| panic!("premise: a legend row\n{shown}"));
    assert!(
        !legend.contains("d drop"),
        "premise: cut before it\n{shown}"
    );
    // The row is as narrow as the terminal, so it is asked whether it took
    // the keys; what it then says is the short form's
    // (`a_review_in_a_sliver_gives_its_own_keys_on_the_row`).
    let row = shown.lines().last().unwrap_or_default();
    assert!(
        row.contains("DECIDE: <CR> rev"),
        "the legend lost an answer, so the row has the keys: {row:?}\n{shown}"
    );
}

/// **Setup over a hidden decision keeps its own bar** — its refusal and
/// its bar ask one question, a decision in a pane (review stop 8, C 5): the
/// bar asked of any, and said one waited behind the screen, where nothing
/// was, its own keys gone.
#[tokio::test]
async fn setup_over_a_hidden_decision_keeps_its_own_bar() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    app.focus_pane(pane);
    app.dispatch_command_for_test("hide");
    assert!(!app.decision_in_a_pane(), "premise: hidden, in no pane");
    app.open_setup();
    app.new_moment();
    let shown = screen(&mut app, 120);
    assert!(!shown.contains("a decision is waiting"), "{shown}");
}

/// **A review's pane with no room is said on the row** — its keys, or with
/// no rows at all what makes room, and what it is: `the summary of 2
/// turns of …`, where a call's row gives its command.
#[tokio::test]
async fn a_review_with_no_room_is_said_on_the_row() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    reviewed(&mut app);
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test("set tui.status.prompt=$decide");
    app.new_moment();
    let shown = text_of(&mut app, 140, 13);
    let row = shown.lines().last().unwrap_or_default();
    assert!(
        row.contains("DECIDE: ") && row.contains("the summary of 2 turns of"),
        "{shown}"
    );
}

/// **Calls first, in the same pane** (Steven, 10-22) — a call typed while a
/// review is up takes its pane, its keys waiting; answered, the review comes
/// back to the same pane (not one closed and another opened), its keys
/// waiting again: what is under your cursor changed twice.
#[tokio::test]
async fn a_call_comes_first_and_the_review_takes_its_pane_back() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");

    manual_tool(&mut app, "rm /tmp/shannon-test-absent");
    app.settle();
    assert!(
        matches!(app.decisions.on_pane(chat), Some(OnPane::Call(_))),
        "the call shown first"
    );
    assert_eq!(
        app.decision_pane_of(chat),
        Some(pane),
        "in the review's pane"
    );

    tool_key_in(&mut app, chat, 'd');
    assert!(
        matches!(app.decisions.on_pane(chat), Some(OnPane::Review(_))),
        "the review back"
    );
    assert_eq!(app.decision_pane_of(chat), Some(pane), "in the same pane");
    assert_eq!(
        app.answer_keys(pane),
        AnswerKeys::Waiting,
        "its keys waiting"
    );
    // Its pane holds the review, not a wait for a next call: kept open, its
    // wait text would describe a pane that shows something else (review
    // stop 8, C 11).
    assert!(!app.decisions.kept_open(chat), "no longer kept for a call");
}

/// **`<CR>` reviews the draft in `$EDITOR`, and `:wq` composes it — where
/// you were** (design review DR-B #6). The key hands the editor over; the
/// editor opens on the instruction and the draft, as one exchange; saved,
/// it composes, and the `:graph` you pressed `S` in, borrowing your only
/// transcript, stays: answered from its pane, the landing read the decision
/// pane as where you stand, and sent the result into the aim's window — the
/// graph's.
#[tokio::test]
async fn enter_reviews_in_the_editor_and_a_save_composes_where_you_were() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let episode = app.conversation().episode().id;
    let chat = reviewed(&mut app);
    let graph = (app.workspace.panes_of(PaneKind::Graph))
        .next()
        .expect("premise: the graph `S` was pressed in");
    let host = (app.decision_pane_of(chat))
        .and_then(|p| app.window_of(p))
        .and_then(Window::decides)
        .map(|(_, host)| host);
    assert_eq!(
        host,
        Some(graph),
        "premise: the review sits under the graph"
    );

    assert_eq!(
        review_key_in(&mut app, chat, KeyCode::Enter),
        ToolKeyEffect::Edit(chat)
    );
    let saved = to_editable(&[Message::user("q"), Message::assistant("edited gist")]);
    let mut editor = Scripted::answering([Ok(Some(saved))]);
    app.edit_decision(chat, &mut editor);
    app.settle();
    assert_eq!(editor.opened.len(), 1, "opened once");
    let (seed, kind) = &editor.opened[0];
    assert_eq!(*kind, crate::editor::Filetype::Markdown);
    assert!(seed.contains("the gist"), "on the draft: {seed}");

    assert!(app.decisions.review(chat).is_none(), "applied");
    assert_ne!(app.conversation().episode().id, episode, "composed");
    assert_eq!(branch_msgs(&app)[1], "edited gist", "as saved");
    assert_eq!(
        app.workspace.kind_of(graph),
        Some(PaneKind::Graph),
        "the graph you were in stays"
    );
}

/// **A review's pane does not close over its draft** — vim's E37 for a
/// modified buffer: `:q`, `:close`, `<C-w>c` and `<C-w>q` are refused,
/// saying what keeps it and what drops it (§3.6); `:q!` drops it, as vim's
/// abandons a buffer's changes, said; `q` keeps it too, said.
#[tokio::test]
async fn a_reviews_pane_refuses_to_close_and_q_bang_drops_it() {
    // All four the doc names, driven — the keys were named and not pressed
    // (review stop 12, T5 F9).
    for close in [":q", ":close", "<C-w>c", "<C-w>q"] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 2);
        let chat = reviewed(&mut app);
        let pane = app.decision_pane_of(chat).expect("premise: its pane");
        app.focus_pane(pane);
        match close.strip_prefix(':') {
            Some(command) => _ = app.dispatch_command_for_test(command),
            None => type_keys(&mut app, close),
        }
        assert_eq!(
            app.decision_pane_of(chat),
            Some(pane),
            "`{close}`: still up"
        );
        assert!(app.decisions.review(chat).is_some(), "`{close}`: kept");
        let said = app.notice().unwrap_or_default().to_string();
        assert!(
            said.contains("`:hide` keeps it for later"),
            "`{close}`: {said}"
        );
    }
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    review_key_in(&mut app, chat, KeyCode::Char('q'));
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("`:hide` keeps it for later"), "`q`: {said}");
    assert!(app.decisions.review(chat).is_some(), "`q`: kept");

    app.dispatch_command_for_test("q!");
    assert!(app.decisions.review(chat).is_none(), "`:q!` drops it");
    assert_eq!(app.decision_pane_of(chat), None, "and its pane goes");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("dropped the summary draft of 2 turns of"),
        "{said}"
    );
}

/// **A hidden review is said at the row's right, `:decide` brings it, and
/// `:ls` says it needs you** — a decision like a call's, in every reader
/// of one; and what goes to it is named where a prompt meets its hold.
#[tokio::test]
async fn a_hidden_review_is_said_and_decide_brings_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    app.focus_pane(pane);
    app.dispatch_command_for_test("hide");
    assert_eq!(app.decision_pane_of(chat), None, "hidden");
    let row = status_row(&mut app, "prompt", "$hidden");
    assert!(row.contains("decision hidden · `:decide 1`"), "{row}");
    let listed = app.buffers_listing(false);
    assert!(listed.contains("needs you — `:decide 1`"), "{listed}");
    assert!(app.refuse_if_busy("continue", chat), "premise: held");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("its summary draft is waiting for your review — `:decide 1` goes to it"),
        "{said}"
    );

    app.dispatch_command_for_test("decide");
    let back = app
        .decision_pane_of(chat)
        .expect("`:decide` brings it back");
    assert_eq!(app.workspace.focused(), back, "and stands you in it");

    // And by its number, as the row says: a session with a review and no
    // call has a decision to go to.
    app.dispatch_command_for_test("hide");
    assert_eq!(app.decision_pane_of(chat), None, "premise: hidden again");
    app.dispatch_command_for_test(&format!("decide {}", chat.number()));
    let back = (app.decision_pane_of(chat)).expect("`:decide N` brings it back");
    assert_eq!(app.workspace.focused(), back, "and stands you in it");
}

/// **Deleting a session drops its review, said** (design review DR-B #2):
/// a draft of a session that no longer exists composes nothing, and the
/// delete's one sentence names it with what else went.
#[tokio::test]
async fn deleting_a_session_drops_its_review_and_says_so() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let lost = app.reset_buffer(chat);
    assert!(app.decisions.review(chat).is_none(), "the review goes");
    assert_eq!(lost.clause(), " (with a summary draft)", "and is counted");
    app.settle();
    assert_eq!(app.decision_pane_of(chat), None, "its pane with it");
}

/// **A review's keys are refused on a pane with no room** — as a call's
/// allow is: `<CR>` is no character, so a guard asking of characters let it
/// through (DR-B #8). Navigation still reaches the window manager.
#[tokio::test]
async fn a_review_with_no_room_takes_no_answer() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    app.focus_pane(pane);
    app.read_the_call_for_test();
    for code in [KeyCode::Enter, KeyCode::Char('a'), KeyCode::Char('d')] {
        let effect = app.handle_tool_key(KeyEvent::new(code, KeyModifiers::NONE), (80, 0));
        assert_eq!(effect, ToolKeyEffect::None, "{code:?}: taken");
        assert!(
            app.decisions.review(chat).is_some(),
            "{code:?}: answered nothing"
        );
        let said = app.notice().unwrap_or_default().to_string();
        assert!(said.contains("doesn't fit"), "{code:?}: {said}");
    }
    let j = app.handle_tool_key(
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        (80, 0),
    );
    assert_eq!(j, ToolKeyEffect::Forward, "a motion goes on to the keymap");
    // A modified key answers nothing, so it is not refused as an answer:
    // `<C-e>` is vim's scroll, not `e`. The guard asks `resolve`, ahead of
    // the plain-keys check, so only `resolve` keeps it out.
    let ctrl_e = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert_eq!(app.handle_tool_key(ctrl_e, (80, 0)), ToolKeyEffect::Forward);
}

/// **`:only` from a second window on the review's session keeps the review,
/// under the window kept** — as `:only` in the window it sits under keeps
/// it: the session it decides for stays in view (review stop 11, F 6; my
/// call). It was refused, and `:only!` hid it with its session on screen.
/// Not answered either way. (`:only!` over a request of ANOTHER session
/// hides it: `only_keeps_the_window_a_decision_sits_under`'s prompt case.)
#[tokio::test]
async fn only_from_a_twin_keeps_the_review_under_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    // Two windows on the session; `S` is pressed in a `:graph` over one, so
    // the review sits under that one, and `:only` in the other closes it.
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    let chat = reviewed(&mut app);
    let other = (app.workspace.panes_of(PaneKind::Output))
        .next()
        .expect("premise: a window on the session, the graph's apart");
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    let host = (app.window_of(pane).and_then(Window::decides)).map(|(_, host)| host);
    assert_ne!(host, Some(other), "premise: under the graph's window");
    app.focus_pane(other);
    app.only_command(false);
    app.settle();
    assert!(app.decisions.review(chat).is_some(), "not answered");
    let now = app.decision_pane_of(chat).expect("still in a pane");
    let under = (app.window_of(now).and_then(Window::decides)).map(|(_, h)| h);
    assert_eq!(under, Some(other), "under the window kept");
    assert!(
        !(app.notices.iter()).any(|(_, said)| said.contains("hidden")),
        "not hidden: {:?}",
        app.notices
    );
}

/// **A review's pane points to the review's manual** (review stop 8, F 1) —
/// `$help` said `:help decision`, a call's chapter, where `d` is the harmless
/// deny; on a review `d` drops paid work.
#[tokio::test]
async fn a_reviews_pane_points_to_the_reviews_manual() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    app.focus_pane(pane);
    app.new_moment();
    let row = screen(&mut app, 120)
        .lines()
        .last()
        .unwrap_or_default()
        .to_string();
    assert!(row.contains("help `:help review`"), "{row}");
    assert!(crate::help_view::anchor("review").is_some(), "and it opens");
}

/// **CTRL-C in a review's pane stops nothing, and says what does what
/// there** — a review's form of the call's hint (§4).
#[tokio::test]
async fn ctrl_c_in_a_reviews_pane_says_what_its_keys_do() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    app.focus_pane(pane);
    app.interrupt();
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("`d` drops the draft, `:hide` keeps it"),
        "{said}"
    );
    assert!(app.decisions.review(chat).is_some(), "nothing answered");
}

// --- review stop 8 ----------------------------------------------------------

/// **A save the compose refuses keeps the draft, and your edit** (review
/// stop 8, A 1 / B 1) — the editable form is JSON, so a draft re-typed as
/// prose does not parse: it threw the paid draft away, and the edit with it.
/// Kept, said beside the refusal, you stood back in its pane, and `<CR>`
/// opens what you wrote, not the model's draft. Saved empty — the editor's
/// cancel — it is kept as closed unsaved.
#[tokio::test]
async fn a_save_that_does_not_compose_keeps_the_draft_and_your_edit() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    let prose = "the gist, edited\nwith a line of its own".to_string();
    let mut editor = Scripted::answering([Ok(Some(prose.clone())), Ok(None)]);
    assert_eq!(
        review_key_in(&mut app, chat, KeyCode::Enter),
        ToolKeyEffect::Edit(chat)
    );
    app.edit_decision(chat, &mut editor);
    let said = app.notice().unwrap_or_default().to_string();
    app.settle();
    assert!(app.decisions.review(chat).is_some(), "the draft kept");
    assert_eq!(app.decision_pane_of(chat), Some(pane), "in its pane");
    assert_eq!(app.workspace.focused(), pane, "and you in it");
    assert!(
        said.contains("could not parse the edit")
            && said.contains("the summary draft waits — `<CR>` opens your edit again")
            && said.contains("`a` applies the model's draft instead"),
        "{said}"
    );
    let title = app
        .decisions
        .review(chat)
        .map(|r| crate::decision_page::DecisionPage::title(r, 0, 0))
        .unwrap_or_default();
    // The state ahead of the subject: a narrow pane cuts its end (review
    // stop 8, F 3).
    assert!(
        title.starts_with(" review this summary? · your edit not applied — "),
        "{title}"
    );
    app.new_moment();
    let shown = screen(&mut app, 120);
    assert!(
        shown.contains("<CR> your edit · a apply the model's draft"),
        "the legend\n{shown}"
    );
    review_key_in(&mut app, chat, KeyCode::Enter);
    app.edit_decision(chat, &mut editor);
    assert_eq!(editor.opened[1].0, prose, "`<CR>` opens what you wrote");
    // Closed unsaved, your edit is kept, and said so.
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("your edit waits too"), "{said}");
    // `a` now applies the model's draft — and says your edit went unused,
    // which it dropped without a word (review stop 8, F 2).
    app.settle();
    review_key_in(&mut app, chat, KeyCode::Char('a'));
    let said = app.notice().unwrap_or_default().to_string();
    assert!(app.decisions.review(chat).is_none(), "premise: applied");
    assert!(said.contains("your edit was not used"), "{said}");

    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    review_key_in(&mut app, chat, KeyCode::Enter);
    let mut editor = Scripted::answering([Ok(Some(String::new()))]);
    app.edit_decision(chat, &mut editor);
    app.settle();
    let title = app
        .decisions
        .review(chat)
        .map(|r| crate::decision_page::DecisionPage::title(r, 0, 0))
        .unwrap_or_default();
    assert!(
        title.contains("closed unsaved"),
        "an emptied save keeps it: {title}"
    );
}

/// **An `a` the compose refuses keeps the draft too** — its session changed
/// since the summary was asked for (a turn cut beneath it, by a door no
/// review's hold reaches): said, and waiting; `d` is the way it goes.
#[tokio::test]
async fn a_stale_apply_keeps_the_draft_and_says_so() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let episode = app.conversation().episode().id;
    let chat = reviewed(&mut app);
    app.remove_turns_from(episode, &[0])
        .expect("a turn cut beneath it");
    review_key_in(&mut app, chat, KeyCode::Char('a'));
    let said = app.notice().unwrap_or_default().to_string();
    app.settle();
    assert!(app.decisions.review(chat).is_some(), "kept");
    assert!(
        said.contains("changed since the summary was asked for")
            && said.contains("`<CR>` opens it again, `d` drops it"),
        "{said}"
    );
}

/// **A compose not asked from its pane moves no one** — focus goes to the
/// review's window only from inside its pane (DR-B #6), and back to the
/// pane on a refusal only from there: asked from elsewhere, a refused
/// compose stood you in a pane you were never in (review stop 8, C 11).
#[tokio::test]
async fn a_compose_asked_from_elsewhere_moves_no_one() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let episode = app.conversation().episode().id;
    let chat = reviewed(&mut app);
    app.remove_turns_from(episode, &[0])
        .expect("a turn cut beneath it");
    app.set_focus(PaneKind::Prompt);
    assert_eq!(app.compose_review(chat, None), None, "premise: refused");
    assert_eq!(app.focus(), PaneKind::Prompt, "you stay where you were");
}

/// **`:wq!` and `:x!` keep a review's draft** — their `!` forces the write
/// and the quit, never the abandon (review stop 8, A 5): they dropped the
/// draft the manual says only `d` or `:q!` drop.
#[tokio::test]
async fn write_quit_bang_keeps_a_reviews_draft() {
    for command in ["wq!", "x!"] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 2);
        let chat = reviewed(&mut app);
        let pane = app.decision_pane_of(chat).expect("premise: its pane");
        app.focus_pane(pane);
        app.dispatch_command_for_test(command);
        let said = app.notice().unwrap_or_default().to_string();
        app.settle();
        assert!(app.decisions.review(chat).is_some(), "`:{command}`: kept");
        assert_eq!(
            app.decision_pane_of(chat),
            Some(pane),
            "`:{command}`: its pane up"
        );
        assert!(
            said.contains("`:hide` keeps it for later"),
            "`:{command}`: {said}"
        );
    }
}

/// **A turn cut from a session its review holds is refused, said** (review
/// stop 8, A 4): the delete asked only for running work, so it went through,
/// and the draft's `a` then found its turns gone.
#[tokio::test]
async fn a_turn_cut_from_a_session_its_review_holds_is_refused() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let episode = app.conversation().episode().id;
    let chat = reviewed(&mut app);
    app.notices.clear();
    app.request_delete(first_turn_delete(&app, episode));
    assert!(app.pending_confirm.is_empty(), "refused, not asked");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("not deleted:")
            && said.contains("its summary draft is waiting for your review"),
        "{said}"
    );
    assert!(app.decisions.review(chat).is_some(), "the draft waits on");
}

/// **A branch, and a review that folds it filed in another session** — as
/// `S` on the branch files it under the session it was pressed in: the
/// branch's buffer, its episode, and the review's session.
fn a_branch_and_its_review_elsewhere(app: &mut App) -> (BufferId, EpisodeId, BufferId) {
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1);
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let on_branch = app.current;
    let branch = app.conversation().episode().id;
    let graph = app.current_graph();
    let (b, fork_turn, fork_index, config) = app.merge_target(&graph, branch).unwrap();
    let sources = vec![*app.conversation().episode().own_turns().last().unwrap()];
    app.dispatch_command_for_test("new");
    let home = app.current;
    assert_ne!(home, on_branch, "premise: the review lives elsewhere");
    app.review_arrives(
        home,
        Box::new(SummaryWork {
            target: SquashTarget::Branch {
                base: b,
                branch,
                fork_turn,
                fork_index,
                config: config.clone(),
            },
            seal: config,
            sources,
            draft: "the gist".into(),
            review: true,
            instruction: "summarise".into(),
        }),
    );
    (on_branch, branch, home)
}

/// **Deleting the session a review would replace takes the review with it,
/// said** (review stop 8, D 3) — as deleting the review's own session does:
/// it waited on unnamed, and its `a` failed with the merge's "episode not
/// found". A review whose session is gone by another door says so.
#[tokio::test]
async fn deleting_what_a_review_replaces_drops_it_and_says_so() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let (_, branch, home) = a_branch_and_its_review_elsewhere(&mut app);
    app.notices.clear();
    app.delete_episodes(&[branch]);
    assert!(app.decisions.review(home).is_none(), "the review goes");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("(with a summary draft)"), "{said}");

    // `:delete all` keeping the review's own session, not the branch it
    // folds: the same, at that door (review stop 8, E1 3).
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let (_, _, home) = a_branch_and_its_review_elsewhere(&mut app);
    let kept = app
        .session_of(home)
        .map(|b| b.conversation.episode().id)
        .unwrap();
    app.notices.clear();
    app.delete_all_sessions(&[kept]);
    assert!(
        app.decisions.review(home).is_none(),
        "`:delete all`: the review goes"
    );
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("with a summary draft"),
        "`:delete all`: {said}"
    );

    // Gone by another door: said as gone, not in the merge's words.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let gone = SquashTarget::Range {
        episode: super::super::fresh_episode_id(),
        start: 0,
        end: 1,
        fork_turn: app.conversation().episode().own_turns()[0],
        config: app.conversation().episode().config.clone(),
    };
    app.decisions
        .review_mut(chat)
        .unwrap()
        .work_mut_for_test()
        .target = gone;
    review_key_in(&mut app, chat, KeyCode::Char('a'));
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("the session it replaces is gone"), "{said}");
    assert!(app.decisions.review(chat).is_some(), "kept, for `d`");
}

/// **A review holds the session it replaces, not only its own** (review
/// stop 8, A 4): `S` on a branch files its review under the session it was
/// pressed in, and a turn sent into the branch made the draft stale, thrown
/// away at its `a`. The branch is held, and says by what.
#[tokio::test]
async fn a_review_holds_the_branch_it_replaces() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let (on_branch, _, home) = a_branch_and_its_review_elsewhere(&mut app);
    app.notices.clear();
    assert!(
        app.refuse_if_busy("continue", on_branch),
        "the branch is held"
    );
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("a summary draft that replaces it")
            && said.contains(&format!("`:decide {}` goes to it", home.number())),
        "{said}"
    );
}

/// **A key in a pane kept for a session's next call answers nothing, a
/// review waiting behind those calls included** (review stop 8, A 2 / C 1):
/// the keys read a second copy of "calls first", and with it broken `a`
/// applied a draft the pane, waiting, did not show.
#[tokio::test]
async fn a_review_behind_waiting_calls_takes_no_key() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    manual_tool(&mut app, "rm /tmp/shannon-test-absent-1");
    manual_tool(&mut app, "rm /tmp/shannon-test-absent-2");
    app.settle();
    tool_key_in(&mut app, chat, 'a');
    assert!(
        app.decisions.waiting_of(chat).next().is_some(),
        "premise: the second call waits"
    );
    let pane = app.decision_pane_of(chat).expect("premise: kept open");
    app.focus_pane(pane);
    app.read_the_call_for_test();
    for code in [KeyCode::Char('a'), KeyCode::Enter] {
        let effect = app.handle_tool_key(KeyEvent::new(code, KeyModifiers::NONE), (80, 100));
        assert_eq!(effect, ToolKeyEffect::None, "{code:?}");
        assert!(
            app.decisions.review(chat).is_some(),
            "{code:?}: nothing applied"
        );
        let said = app.notice().unwrap_or_default().to_string();
        assert!(said.contains("nothing to answer yet"), "{code:?}: {said}");
    }
}

/// **A review's keys, once live, stay live through the events after** —
/// it re-arms coming onto its pane, not at every settle (review stop 8,
/// C 2): every event settles, a stream batch from any session among them,
/// so re-armed each time it could not be answered while anything streamed.
#[tokio::test]
async fn a_reviews_live_keys_stay_live_across_a_settle() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    app.read_the_call_for_test();
    assert_eq!(app.answer_keys(pane), AnswerKeys::Live, "premise");
    app.settle();
    assert_eq!(
        app.answer_keys(pane),
        AnswerKeys::Live,
        "a settle re-arms nothing"
    );
}

/// **A secret in a draft is masked on its page** (review stop 8, B 2 / C 3)
/// — a summary quotes the conversation it summarises; the one line that
/// masks it was held by no test.
#[tokio::test]
async fn a_secret_in_a_draft_is_masked_on_its_page() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);
    app.feed_summary(StreamMsg::Chunk(
        "the key was sk-ant-api03-abcdefghijklmnopqrstuvw".into(),
    ));
    app.feed_summary(StreamMsg::Complete);
    app.new_moment();
    let shown = screen(&mut app, 120);
    assert!(
        shown.contains("the key was"),
        "premise: the draft drawn\n{shown}"
    );
    assert!(!shown.contains("sk-ant-api03-abcdefghijklmnop"), "{shown}");
}

/// **A key a review's pane refuses starts the wait over**, as a call's
/// does (review stop 8, C 4): the `a` of `ra`, `ma` or `"a` applied the
/// draft unread.
#[tokio::test]
async fn a_refused_key_in_a_review_starts_its_wait_over() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    // `n` is its page's search — a review has no calls for it to walk.
    assert_eq!(
        review_key_in(&mut app, chat, KeyCode::Char('n')),
        ToolKeyEffect::Forward
    );
    review_key_in(&mut app, chat, KeyCode::Char('r'));
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("not a decision key — <CR> review · a apply · d drop"),
        "its own keys, not a call's: {said}"
    );
    let effect = app.handle_tool_key(
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        (80, 100),
    );
    assert_eq!(effect, ToolKeyEffect::None);
    assert!(app.decisions.review(chat).is_some(), "nothing applied");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("not answered — still open"), "{said}");
}

/// **Held, with nothing to answer yet, is said as it is** (review stop 8,
/// D 1) — an allowed command running, its session's next call waiting
/// behind it, and the review behind that. A prompt was refused "`:decide 1`
/// goes to it", and `:decide 1` then said "no decision is waiting"; with
/// calls only, "answer it first" sent you to a pane saying there was
/// nothing to answer.
#[tokio::test]
async fn held_with_nothing_to_answer_yet_is_said_as_it_is() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    manual_tool(&mut app, "rm /tmp/shannon-test-absent-1");
    manual_tool(&mut app, "rm /tmp/shannon-test-absent-2");
    app.settle();
    tool_key_in(&mut app, chat, 'a');
    assert!(
        app.decisions.on_pane(chat).is_none(),
        "premise: nothing to answer yet"
    );

    app.set_focus(PaneKind::Prompt);
    app.notices.clear();
    assert!(app.refuse_if_busy("continue", chat), "premise: held");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains(
            "its summary draft is waiting for your review — it comes up after its session's calls"
        ),
        "{said}"
    );
    app.dispatch_command_for_test(&format!("decide {}", chat.number()));
    let said = app.notice().unwrap_or_default().to_string();
    assert!(said.contains("nothing to answer in"), "{said}");
    let listed = app.buffers_listing(false);
    assert!(
        listed.contains("needs you — after its running command"),
        "{listed}"
    );

    // Calls only: the refusal says what holds it.
    let mut app = test_app();
    let chat = app.current;
    manual_tool(&mut app, "rm /tmp/shannon-test-absent-1");
    manual_tool(&mut app, "rm /tmp/shannon-test-absent-2");
    app.settle();
    tool_key_in(&mut app, chat, 'a');
    app.set_focus(PaneKind::Prompt);
    app.notices.clear();
    assert!(app.refuse_if_busy("continue", chat), "premise: held");
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("a command is running, and its next call waits behind it"),
        "{said}"
    );
}

/// **`q` on a review starts its wait over too** (review stop 8, D 2) — its
/// refusal returned before the wait, so the `a` of `qa` applied the draft
/// and the `d` of `qd` dropped it: `q{reg}` is vim's macro chord.
#[tokio::test]
async fn q_on_a_review_starts_its_wait_over() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    review_key_in(&mut app, chat, KeyCode::Char('q'));
    for key in ['a', 'd'] {
        let effect = app.handle_tool_key(
            KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
            (80, 100),
        );
        assert_eq!(effect, ToolKeyEffect::None, "`q{key}`");
        assert!(app.decisions.review(chat).is_some(), "`q{key}`: kept");
        let said = app.notice().unwrap_or_default().to_string();
        assert!(
            said.contains("not answered — still open"),
            "`q{key}`: {said}"
        );
    }
}

/// **A review coming back to its pane is fitted to its draft** (review
/// stop 8, C 6) — a call shown there meanwhile sized the box to its report,
/// and the review came back in it.
#[tokio::test]
async fn a_review_back_on_its_pane_is_fitted_to_its_draft() {
    let rows = |app: &App, pane| match app.workspace.slot(pane).map(|s| s.extent) {
        Some(crate::workspace::Extent::Transient { rows }) => rows,
        other => panic!("not a fitted pane: {other:?}"),
    };
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    let review = rows(&app, pane);
    manual_tool(&mut app, "rm /tmp/shannon-test-absent");
    app.settle();
    assert_ne!(
        rows(&app, pane),
        review,
        "premise: the call's box is another size"
    );
    tool_key_in(&mut app, chat, 'd');
    assert_eq!(
        app.decision_pane_of(chat),
        Some(pane),
        "premise: the same pane"
    );
    assert_eq!(rows(&app, pane), review, "fitted to the draft again");
}

/// **Where a review waits is said truly** (review stop 8, C 7, D 1):
/// arriving behind a call of its session — here one you hid — it comes up
/// after the call, said so, not "the pane beside it" (no pane opens) nor
/// `:decide N` (which goes to the call); at the press, a session no window
/// shows says `:decide N`.
#[tokio::test]
async fn where_a_review_waits_is_said_truly() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = app.current;
    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);
    manual_tool(&mut app, "rm /tmp/shannon-test-absent");
    app.settle();
    let pane = app
        .decision_pane_of(chat)
        .expect("premise: the call's pane");
    app.focus_pane(pane);
    app.dispatch_command_for_test("hide");
    app.notices.clear();
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    assert_eq!(app.decision_pane_of(chat), None, "premise: hidden, no pane");
    let said: Vec<&str> = (app.notices.iter()).map(|(_, s)| s.as_str()).collect();
    let reach = "its review comes up after its session's calls";
    assert!(said.iter().any(|s| s.contains(reach)), "{said:?}");

    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = app.current;
    app.dispatch_command_for_test("new");
    assert!(
        app.panes_with_session(chat, Over::ThisTab).next().is_none(),
        "premise: in no window"
    );
    assert_eq!(
        app.review_when_written(chat),
        format!(
            "its review waits for you when it is written — `:decide {}`",
            chat.number()
        )
    );
}

/// **`e` reviews in `$EDITOR`, as `<CR>` does; closed unsaved, the legend
/// says `<CR>` opens it again** (review stop 8, C 8: the manual promises
/// both, and a mutant of either passed).
#[tokio::test]
async fn e_reviews_too_and_an_unsaved_close_is_on_the_legend() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    assert_eq!(
        review_key_in(&mut app, chat, KeyCode::Char('e')),
        ToolKeyEffect::Edit(chat)
    );
    let mut editor = Scripted::answering([Ok(None)]);
    app.edit_decision(chat, &mut editor);
    app.settle();
    let title = app
        .decisions
        .review(chat)
        .map(|r| crate::decision_page::DecisionPage::title(r, 0, 0))
        .unwrap_or_default();
    assert!(title.contains("closed unsaved"), "{title}");
    app.new_moment();
    let shown = screen(&mut app, 120);
    assert!(shown.contains("<CR> review again"), "the legend\n{shown}");
}

/// **`:hide` in a pane kept for a session's next call closes it, and hides
/// nothing** (review stop 8, A 3): nothing is on it to keep, so the next
/// call opens a pane beside you and nothing is said hidden. With the guard
/// gone it hid the session, and the next call waited behind `$hidden`.
#[tokio::test]
async fn hide_on_a_pane_kept_for_the_next_call_hides_nothing() {
    let mut app = test_app();
    let chat = app.current;
    manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent-1");
    manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent-2");
    app.settle();
    tool_key_in(&mut app, chat, 'a');
    let pane = app.decision_pane_of(chat).expect("premise: kept open");
    app.focus_pane(pane);
    app.dispatch_command_for_test("hide");
    assert_eq!(app.workspace.kind_of(pane), None, "the pane closes");
    settle(&mut app).await; // the first finishes
    app.settle();
    assert!(app.shown_call_in(chat).is_some(), "premise: the next is up");
    assert!(app.decision_pane_of(chat).is_some(), "in a pane beside you");
    let row = status_row(&mut app, "prompt", "$hidden");
    assert!(!row.contains("hidden"), "nothing hidden: {row}");
}

/// **`:q` on a call shown over a review denies the call, and the review
/// comes up as a call waiting there would** (review stop 8, A 8 / C 10):
/// it was hidden, where `q` on the same call brought it back — two denies,
/// two ends. A close answers what the pane was on.
#[tokio::test]
async fn closing_a_calls_pane_over_a_review_leaves_the_review_up() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    manual_tool(&mut app, "rm /tmp/shannon-test-absent");
    app.settle();
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    assert!(
        matches!(app.decisions.on_pane(chat), Some(OnPane::Call(_))),
        "premise: the call on it"
    );
    app.focus_pane(pane);
    // Read first: a close in its pane is an answer, and waits the delay as
    // `q` does (review stop 11, A 2).
    app.read_the_call_for_test();
    app.dispatch_command_for_test("q");
    let said: Vec<&str> = (app.notices.iter()).map(|(_, s)| s.as_str()).collect();
    assert!(said.iter().any(|s| s.contains("DENIED")), "{said:?}");
    // "waits, hidden", as every hide says it: this probed "waits hidden",
    // which nothing says (review stop 12, T5 F6).
    assert!(
        !said.iter().any(|s| s.contains("waits, hidden")),
        "not hidden: {said:?}"
    );
    assert!(
        matches!(app.decisions.on_pane(chat), Some(OnPane::Review(_))),
        "the review next"
    );
    assert!(app.decision_pane_of(chat).is_some(), "in a pane");
}

/// **A `:tool` your rules let run reveals nothing you hid** (review stop 8,
/// A 7) — it asks nothing, so nothing waits behind what you hid; it brought
/// the hidden review back beside you all the same. One that asks does
/// (`a_typed_tool_brings_back_its_sessions_hidden_decision`).
#[tokio::test]
async fn a_tool_that_asks_nothing_reveals_nothing() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = reviewed(&mut app);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    app.focus_pane(pane);
    app.dispatch_command_for_test("hide");
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test("allow echo:*");
    manual_tool(&mut app, "echo probe");
    app.settle();
    assert!(
        app.shown_call_in(chat).is_none(),
        "premise: it asked nothing"
    );
    assert_eq!(app.decision_pane_of(chat), None, "the review stays hidden");
}

/// **An empty reply composes nothing, said** (review stop 8, B 3) — it was a
/// pane with no draft and no legend whose `a` composed an empty summary.
#[tokio::test]
async fn an_empty_draft_is_said_and_puts_nothing_up() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = app.current;
    let episode = app.conversation().episode().id;
    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);
    app.feed_summary(StreamMsg::Chunk("  \n\t\n ".into()));
    app.feed_summary(StreamMsg::Complete);
    assert!(app.decisions.review(chat).is_none(), "no review");
    assert_eq!(app.decision_pane_of(chat), None, "and no pane");
    let said: Vec<&str> = (app.notices.iter()).map(|(_, s)| s.as_str()).collect();
    assert!(
        said.iter()
            .any(|s| s.contains("the summary came back empty")),
        "{said:?}"
    );
    assert_eq!(
        app.session_of(chat).map(|b| b.conversation.episode().id),
        Some(episode),
        "nothing replaced"
    );
}

/// **A draft longer than a `u16` of rows still gets the capped pane, its
/// legend in it** (review stop 8, B 5): its rows wrapped past `u16::MAX` to
/// a few, and a 65 536-line draft came up in a three-row pane with no
/// legend.
#[tokio::test]
async fn a_draft_past_a_u16_of_rows_gets_the_capped_pane() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = app.current;
    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);
    app.feed_summary(StreamMsg::Chunk("x\n".repeat(65_536)));
    app.feed_summary(StreamMsg::Complete);
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    let rows = match app.workspace.slot(pane).map(|s| s.extent) {
        Some(crate::workspace::Extent::Transient { rows }) => rows,
        other => panic!("not a fitted pane: {other:?}"),
    };
    assert_eq!(
        rows, app.config.tui.approval_max_height,
        "the cap, not a wrap"
    );
}

/// **A draft's page keeps the transcript's rule for the model's text** —
/// a tab is a space, a control byte is gone (review stop 8, B 4: the tab
/// vanished, and only ratatui stood between an escape and the terminal).
#[tokio::test]
async fn a_drafts_page_is_sanitised_as_the_transcript_is() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = app.current;
    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);
    app.feed_summary(StreamMsg::Chunk("tab\there \u{1b}]52;c;x\u{7} end".into()));
    app.feed_summary(StreamMsg::Complete);
    let review = app.decisions.review(chat).expect("premise: up");
    let page: String = (crate::decision_page::DecisionPage::text(review)
        .lines()
        .iter())
    .map(|l| l.text())
    .collect::<Vec<_>>()
    .join("\n");
    assert!(page.contains("tab here"), "the tab a space: {page:?}");
    assert!(
        !page.chars().any(char::is_control),
        "no control byte: {page:?}"
    );
}

/// **`:b N` in a review's chat is not refused** — as in a call's (Steven,
/// 10-26, Q3; `b_in_a_requests_chat_takes_its_pane_and_ctrl_caret_brings_it_back`):
/// the review waits, its draft kept, in no pane while no window shows its
/// session; CTRL-^ brings the session back to the window, and its pane
/// with it. §3.6's row 12 had no review test (review stop 12, T5 F4).
#[tokio::test]
async fn b_in_a_reviews_chat_takes_its_pane_and_ctrl_caret_brings_it_back() {
    let mut app = test_app();
    let chat = Request::Review.ask(&mut app);
    let host_of = |app: &App| {
        let pane = app.decision_pane_of(chat)?;
        (app.window_of(pane).and_then(Window::decides)).map(|(_, host)| host)
    };
    let host = host_of(&app).expect("premise: under its chat");
    app.focus_pane(host);
    app.dispatch_command_for_test("new");
    let other = app.pane_session(host).id;
    assert_ne!(other, chat, "premise: another session in the chat's window");
    app.window_command_for_test(WindowCmd::Alternate);
    app.settle();
    assert_eq!(host_of(&app), Some(host), "premise: back, and its pane");

    app.notices.clear();
    app.dispatch_command_for_test(&format!("b {}", other.number()));
    let said = app.notice().unwrap_or_default().to_string();
    assert_eq!(app.pane_session(host).id, other, "`:b` not refused: {said}");
    assert_eq!(app.decision_pane_of(chat), None, "its pane gone with it");
    assert!(app.decisions.review(chat).is_some(), "the draft kept");

    app.window_command_for_test(WindowCmd::Alternate);
    app.settle();
    assert_eq!(app.pane_session(host).id, chat, "CTRL-^: back");
    assert_eq!(host_of(&app), Some(host), "and its pane with it");
    assert!(app.decisions.review(chat).is_some(), "still waiting");
}

/// **An unsaved review stays, says so, and its keys answer** — vim's
/// `:x`/`ZZ` on an UNMODIFIED draft writes nothing, on disk byte-identical to
/// `:q!`, so the app cannot guess (the mutt lineage: edit in `$EDITOR`,
/// decide in the app). It asked "apply it?" (`y`/`n`); the review is a
/// decision now (§4), so it stays in its pane, marked closed unsaved, and
/// `a` composes the draft exactly as drafted, `d` drops it, said.
#[tokio::test]
async fn an_unsaved_review_stays_and_its_keys_answer() {
    for (code, composes) in [(KeyCode::Char('a'), true), (KeyCode::Char('d'), false)] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 2);
        let chat = app.current;
        let episode = app.conversation().episode().id;
        mark_the_aims_session(&mut app);
        distill_reviewed(&mut app);
        app.feed_summary(StreamMsg::Chunk("the distilled gist".into()));
        app.feed_summary(StreamMsg::Complete);
        app.edit_review(chat, &mut Scripted::answering([Ok(None)]));
        let review = app.decisions.review(chat).expect("it stays");
        let title = crate::decision_page::DecisionPage::title(review, 0, 0);
        assert!(title.contains("closed unsaved"), "{code:?}: {title}");
        let said = app.notice().unwrap_or_default().to_string();
        assert!(said.contains("closed without saving"), "{code:?}: {said}");

        review_key_in(&mut app, chat, code);
        assert!(app.decisions.review(chat).is_none(), "{code:?}: answered");
        assert_eq!(
            app.conversation().episode().id != episode,
            composes,
            "{code:?}"
        );
        if composes {
            let msgs = branch_msgs(&app);
            assert_eq!(msgs.len(), 2, "the summary-only session: {msgs:?}");
            assert_eq!(msgs[1], "the distilled gist", "the draft, verbatim");
        } else {
            let said = app.notice().unwrap_or_default().to_string();
            assert!(said.contains("dropped the summary draft"), "{said}");
        }
    }
}

/// **A review holds the prompt queued behind it until it is answered** —
/// then sends it to whichever session the answer made (review 2026-10-15,
/// Reviewer C F1, of the "apply it?" this was): the prompt started in the
/// pre-squash session, the draft composed over it and superseded the turn,
/// billed and in neither transcript. Held from the review's arrival (DR-B
/// #1), through any other event's end and through keys that answer nothing
/// (CTRL-C, `q`); let go by `a` (it composes) or `d` (it does not), each
/// driven, as each is its own door out.
#[tokio::test]
async fn a_review_holds_the_prompt_queued_behind_it() {
    for (code, composes) in [(KeyCode::Char('a'), true), (KeyCode::Char('d'), false)] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 2);
        let chat = app.current;
        let episode = app.conversation().episode().id;
        mark_the_aims_session(&mut app);
        distill_reviewed(&mut app);
        assert!(app.distilling().is_some());
        app.submit("typed while distilling");
        assert_eq!(app.pending.len(), 1, "queued behind the distillation");

        app.feed_summary(StreamMsg::Chunk("the gist".into()));
        app.feed_summary(StreamMsg::Complete);
        assert!(app.decisions.review(chat).is_some(), "premise: its review");
        assert_eq!(app.pending.len(), 1, "{code:?}: held by its review");
        assert!(!app.streaming(), "{code:?}: nothing started");
        // Any other event ends meanwhile — a `:Send` reply finishing.
        app.settle();
        assert_eq!(
            app.pending.len(),
            1,
            "{code:?}: a settle from elsewhere waits"
        );
        // Keys that answer nothing: CTRL-C in its pane, `q`.
        let pane = app.decision_pane_of(chat).expect("premise: its pane");
        app.focus_pane(pane);
        app.interrupt();
        review_key_in(&mut app, chat, KeyCode::Char('q'));
        assert_eq!(
            app.pending.len(),
            1,
            "{code:?}: CTRL-C and `q` answer nothing"
        );
        assert!(app.decisions.review(chat).is_some(), "{code:?}: still up");

        review_key_in(&mut app, chat, code); // the key's event ends
        assert!(app.decisions.review(chat).is_none(), "{code:?}: answered");
        assert!(app.pending.is_empty(), "{code:?}: the prompt goes");
        assert!(app.streaming(), "{code:?}: as a turn");
        let msgs = branch_msgs(&app);
        assert_eq!(
            msgs.last().map(String::as_str),
            Some("typed while distilling"),
            "{code:?}: {msgs:?}"
        );
        assert_eq!(
            app.conversation().episode().id != episode,
            composes,
            "{code:?}: into the session the answer made"
        );
        if composes {
            assert_eq!(msgs.len(), 3, "the draft's exchange, then the prompt");
            assert_eq!(msgs[1], "the gist");
        } else {
            assert_eq!(msgs.len(), 5, "the original's two turns, then the prompt");
        }
    }
}

/// **A review holds its session whatever is on screen** (review
/// 2026-10-16, Reviewer B F2, of the question this was) — `S`, a prompt
/// queued, then `d` in `:graph` while it distils, so a y/N question is up
/// when the draft arrives; and the review hidden, in no pane at all.
/// Scanning only the question on screen let the prompt start into the
/// pre-squash session — the bug the held slot exists for, with the whole
/// suite green.
#[tokio::test]
async fn a_review_holds_its_session_whatever_is_on_screen() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = app.current;
    let episode = app.conversation().episode().id;
    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);
    app.submit("typed while distilling");
    assert_eq!(app.pending.len(), 1, "premise: queued");
    app.ask(ConfirmAction::sessions(vec![episode])); // `d` in :graph meanwhile
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    assert_eq!(app.pending_confirm.len(), 1, "premise: a question is up");
    assert!(
        app.decisions.review(chat).is_some(),
        "premise: and the review"
    );
    assert_eq!(app.pending.len(), 1, "held, the question on top");
    assert!(
        !app.streaming(),
        "nothing started into the pre-squash session"
    );
    type_keys(&mut app, "n");
    assert_eq!(app.pending.len(), 1, "held, the question answered");

    // Hidden, in no pane: held still.
    let pane = app.decision_pane_of(chat).expect("premise: its pane");
    app.focus_pane(pane);
    app.dispatch_command_for_test("hide");
    assert_eq!(app.decision_pane_of(chat), None, "premise: hidden");
    assert_eq!(app.pending.len(), 1, "held, hidden");
    assert!(!app.streaming(), "and nothing started");
}

/// **A review holds only its own session** (review 2026-10-16, Reviewers
/// A M-A, C F1, of the question this was). A's draft waits for its review;
/// a prompt for B is not about A and starts. Holding every session left the
/// suite green.
///
/// Typed while A distils, B's prompt queued behind the distillation and was
/// let go only at the question; sessions run apart now (`buffers.md` §9), so
/// it starts at once — and a second one, typed with the review up, starts
/// too, which is the property.
#[tokio::test]
async fn a_review_holds_only_its_own_session() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());
    app.workspace.focus_pane(sibling);
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.current;
    app.workspace.focus_pane(focused);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(a);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    distill_reviewed(&mut app);
    app.submit_to(b_buffer, "for B");
    assert!(
        app.pending.is_empty(),
        "B's prompt starts beside A's distillation"
    );
    // B's turn ends, so B is free when A's question comes.
    let b_turn = app
        .generations
        .iter()
        .find(|g| g.buffer == b_buffer)
        .map(|g| g.id)
        .expect("B runs");
    app.feed_turn(b_turn, StreamMsg::Complete);
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    assert!(
        app.decisions.review(a_buffer).is_some(),
        "premise: the draft is A's"
    );
    app.submit_to(b_buffer, "for B, again");
    assert!(
        app.pending.is_empty(),
        "B's prompt is not held by A's review"
    );
    assert!(
        app.generations.iter().any(|g| g.buffer == b_buffer),
        "B's prompt runs"
    );
}

/// **A review the editor failed keeps the draft, and the prompt typed
/// during `S` with it, until `d`** — `:cq` once cancelled the review and
/// started the prompt (review 2026-10-16, Reviewer A M-U, pinned that); paid
/// work goes only by `d` now (design review DR-B #9), and the prompt is
/// meant for whichever session the draft's answer makes, so it waits too.
/// `d` lets it go, into the session as it was.
#[tokio::test]
async fn a_failed_review_keeps_the_draft_and_the_prompt_until_d() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = app.current;
    let episode = app.conversation().episode().id;
    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);
    app.submit("typed while distilling");
    assert_eq!(app.pending.len(), 1, "premise: queued");
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    let mut editor = Scripted::answering([Err(anyhow::anyhow!("editor exited :cq"))]);
    assert_eq!(
        review_key_in(&mut app, chat, KeyCode::Enter),
        ToolKeyEffect::Edit(chat)
    );
    app.edit_decision(chat, &mut editor);
    app.settle();
    assert!(app.decisions.review(chat).is_some(), "the draft waits");
    assert_eq!(app.pending.len(), 1, "and the prompt with it");
    assert!(!app.streaming(), "nothing started");

    review_key_in(&mut app, chat, KeyCode::Char('d'));
    assert!(app.pending.is_empty(), "`d`: the prompt goes");
    assert!(
        app.streaming(),
        "as a turn, in the session it was typed for"
    );
    assert_eq!(app.conversation().episode().id, episode, "nothing composed");
}
