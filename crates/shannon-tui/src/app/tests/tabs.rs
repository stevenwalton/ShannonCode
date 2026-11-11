//! **Tab pages** (`docs/DesignDocs/tabs.md` §6b, T2): a named layout of
//! panes, the buffers the app's, each tab's prompt aimed its own way.

use super::*;
use crate::style::Ink;

/// Where you are: (how many tabs, which is shown, from 1).
fn tabs(app: &App) -> (usize, usize) {
    (app.tabs.count(), app.tabs.current() + 1)
}

/// The session the transcript pane of the tab shown holds.
fn shown_session(app: &App) -> BufferId {
    let pane = (app.workspace.panes_of(PaneKind::Output).next()).expect("a transcript here");
    app.pane_session(pane).id
}

/// **`:tabnew` opens a tab after this one, on a fresh session, aimed there**
/// — vim's `:tabnew` with Shannon's `:new` (measured, Neovim 0.12.6: after
/// the current tab, not at the end); the tab left keeps its arrangement.
#[test]
fn tabnew_opens_a_tab_after_this_one_on_a_fresh_session_aimed_there() {
    let mut app = test_app();
    let first = shown_session(&app);
    let first_panes = app.workspace.pane_ids();
    app.dispatch_command_for_test("tabnew");
    assert_eq!(tabs(&app), (2, 2), "a second tab, shown");
    let new = shown_session(&app);
    assert_ne!(new, first, "on a fresh session");
    assert_eq!(app.prompt_target(), new, "its prompt aimed there");
    assert!(
        (first_panes.iter()).all(|p| app.workspace.window(*p).is_none()),
        "none of the first tab's panes here"
    );
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(tabs(&app), (2, 1), "premise: back on the first");
    assert_eq!(
        app.workspace.pane_ids(),
        first_panes,
        "its arrangement as it was"
    );
    app.dispatch_command_for_test("tabnew");
    assert_eq!(tabs(&app), (3, 2), "after this one, not at the end");
}

/// **`:tabnew N` opens on session N** — by its `:ls` number, as `:b N`; a
/// page's number is said to be a page, as every session verb says it.
#[test]
fn tabnew_n_opens_on_session_n() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    assert_eq!(
        app.store.get_ref("HEAD").unwrap(),
        Some(b),
        "premise: HEAD on B"
    );
    app.dispatch_command_for_test(&format!("tabnew {}", &a.to_hex()[..16]));
    assert_eq!(tabs(&app), (2, 2), "premise: a tab");
    let shown = app
        .session_of(shown_session(&app))
        .map(|s| s.conversation.episode().id);
    assert_eq!(shown, Some(a), "on the session named, loaded");
    assert_eq!(
        app.store.get_ref("HEAD").unwrap(),
        Some(a),
        "made active, as `:b N`"
    );
    let n = shown_session(&app).number();
    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test(&format!("tabnew {n}"));
    assert_eq!(tabs(&app), (3, 2), "by its `:ls` number too");
    assert_eq!(shown_session(&app).number(), n, "the same session");
    app.dispatch_command_for_test("help");
    let manual = (app.buffers.pages())
        .find(|p| p.kind() == PaneKind::Page(PageKind::Help))
        .map(|p| p.id());
    let manual = manual.expect("premise: the manual is a buffer");
    app.dispatch_command_for_test(&format!("tabnew {}", manual.number()));
    assert_eq!(tabs(&app), (3, 2), "no tab for a page");
    assert!(
        app.notice().is_some_and(|n| n.contains("not a session")),
        "{:?}",
        app.notice()
    );
}

/// **`gt` and `gT` go round, a count goes to tab N or N back** — vim's
/// (measured, Neovim 0.12.6): `gt` after the last is the first, `gT`
/// before the first the last, `2gt` tab 2, `3gT` three back round the end;
/// and tab 9 of 3 is said, from the key and from `:tabnext` alike (vim's
/// `9gt` says nothing; here that would read as a dead key).
#[test]
fn gt_and_g_shift_t_go_round_and_a_count_goes_to_tab_n() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnew");
    assert_eq!(tabs(&app), (4, 4), "premise: four, on the last");
    app.set_focus(PaneKind::Output);
    type_keys(&mut app, "gt");
    assert_eq!(tabs(&app), (4, 1), "gt after the last: the first");
    app.set_focus(PaneKind::Output);
    type_keys(&mut app, "gT");
    assert_eq!(tabs(&app), (4, 4), "gT before the first: the last");
    app.set_focus(PaneKind::Output);
    type_keys(&mut app, "2gt");
    assert_eq!(tabs(&app), (4, 2), "2gt: tab 2");
    app.set_focus(PaneKind::Output);
    type_keys(&mut app, "3gT");
    assert_eq!(tabs(&app), (4, 3), "3gT from 2: round the front to 3");
    app.set_focus(PaneKind::Output);
    type_keys(&mut app, "9gt");
    assert_eq!(tabs(&app), (4, 3), "no tab 9: where you were");
    let no_9 = "there is no tab 9 — this is tab 3 of 4";
    assert_eq!(app.notice().as_deref(), Some(no_9));
    app.dispatch_command_for_test("tabnext 9");
    assert_eq!(
        app.notice().as_deref(),
        Some(format!("`:tabnext`: {no_9}").as_str()),
        "one sentence for the key and the command"
    );
    app.dispatch_command_for_test("tabprevious 2");
    assert_eq!(tabs(&app), (4, 1), ":tabprevious 2 from 3: tab 1");

    app.dispatch_command_for_test("tabnext +2");
    assert_eq!(tabs(&app), (4, 3), "`+2`: two over, not tab 2");
    app.dispatch_command_for_test("tabnext -");
    assert_eq!(tabs(&app), (4, 2), "`-`: one back");
    app.dispatch_command_for_test("tabnext $");
    assert_eq!(tabs(&app), (4, 4), "`$`: the last");
    app.dispatch_command_for_test("tabnext +");
    assert_eq!(tabs(&app), (4, 4), "never round the end");
    assert!(app.notice().is_some_and(|n| n.contains("no tab +")));
    app.dispatch_command_for_test("tabprevious $");
    assert_eq!(tabs(&app), (4, 4), "a count only");
    assert!(app.notice().is_some_and(|n| n.contains("not a count")));
}

/// **A tab argument is vim's** (`tabs::tab_named`, measured in Neovim
/// 0.12.6, from tab 2 of 4): `N` from 1, `+N` / `-N` relative with bare
/// `+` / `-` one, `$` the last; `0`, past either end, a sign glued to a
/// word, refused — never wrapped.
#[test]
fn a_tab_argument_is_vims() {
    use crate::app::tabs::tab_named;
    let from_2_of_4 = [
        ("1", Some(0)),
        ("2", Some(1)),
        ("4", Some(3)),
        ("$", Some(3)),
        ("+", Some(2)),
        ("-", Some(0)),
        ("+1", Some(2)),
        ("+2", Some(3)),
        ("-1", Some(0)),
        ("+3", None),
        ("-2", None),
        ("0", None),
        ("5", None),
        ("9", None),
        ("x", None),
        ("+x", None),
        ("1x", None),
        ("+-1", None),
    ];
    for (arg, want) in from_2_of_4 {
        assert_eq!(tab_named(arg, 1, 4).ok(), want, "tab `{arg}` from 2 of 4");
    }
}

/// **`:tabclose N` closes tab N and leaves you where you are** (vim's,
/// measured: `:tabclose 3` from 2 of 4 leaves A,B,D on B), its sessions
/// open; `:tabclose -` the one before, `:tabclose 9` refused with nothing
/// closed. It took its argument and dropped it before T4, closing the tab
/// you were in.
#[test]
fn tabclose_n_closes_that_tab_and_stays() {
    let mut app = test_app();
    for _ in 0..3 {
        app.dispatch_command_for_test("tabnew");
    }
    app.dispatch_command_for_test("tabnext 3");
    let third = shown_session(&app);
    app.dispatch_command_for_test("tabnext 2");
    let here = shown_session(&app);
    app.dispatch_command_for_test("tabclose 3");
    assert_eq!(tabs(&app), (3, 2), "one fewer, still the second");
    assert_eq!(shown_session(&app), here, "where you were");
    assert!(app.session_of(third).is_some(), "its session open still");
    app.dispatch_command_for_test("tabclose -");
    assert_eq!(tabs(&app), (2, 1), "the one before closed: now the first");
    assert_eq!(shown_session(&app), here);
    app.dispatch_command_for_test("tabclose 9");
    assert_eq!(tabs(&app), (2, 1), "nothing closed");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("`:tabclose`: there is no tab 9"))
    );
}

/// **`:tabclose` shows the tab to the right, else the left; never the
/// only one** (vim's, measured: E784).
#[test]
fn tabclose_shows_the_right_else_the_left_and_keeps_the_last() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabclose");
    assert_eq!(tabs(&app), (1, 1), "the only tab stays");
    assert!(
        app.notice().is_some_and(|n| n.contains("the only tab")),
        "said: {:?}",
        app.notice()
    );
    for _ in 0..3 {
        app.dispatch_command_for_test("tabnew");
    }
    app.dispatch_command_for_test("tabnext 2");
    let third = {
        app.dispatch_command_for_test("tabnext 3");
        let s = shown_session(&app);
        app.dispatch_command_for_test("tabnext 2");
        s
    };
    app.dispatch_command_for_test("tabclose");
    assert_eq!(tabs(&app), (3, 2), "the one to its right, now second");
    assert_eq!(shown_session(&app), third, "the old third");
    app.dispatch_command_for_test("tabnext 3");
    app.dispatch_command_for_test("tabclose");
    assert_eq!(tabs(&app), (2, 2), "from the last: the one to its left");
}

/// **`:q` on a tab's last window closes the tab while another is open** —
/// vim's, on a tab page's last window (measured): the transcript beside the
/// bar, from either (`App::last_window`), and the bar alone, by `:q` or
/// `:close`; the sessions stay, as every close leaves them.
#[test]
fn q_on_a_tabs_last_pane_closes_the_tab() {
    for stand in [PaneKind::Output, PaneKind::Prompt] {
        let mut app = test_app();
        app.dispatch_command_for_test("tabnew");
        let second = shown_session(&app);
        app.set_focus(stand);
        assert!(!app.quit_window(false), "{stand:?}: the app stays");
        assert_eq!(tabs(&app), (1, 1), "{stand:?}: the tab closed");
        assert!(
            app.session_of(second).is_some(),
            "{stand:?}: its session open still"
        );
    }

    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    let second = shown_session(&app);
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("close");
    assert_eq!(app.workspace.pane_count(), 1, "premise: the prompt left");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("`:q` closes the tab")),
        "the notice says what `:q` does here: {:?}",
        app.notice()
    );
    app.set_focus(PaneKind::Prompt);
    let quit = app.quit_window(false);
    assert!(!quit, "the app stays");
    assert_eq!(tabs(&app), (1, 1), "the tab closed");
    assert!(app.session_of(second).is_some(), "its session open still");

    app.dispatch_command_for_test("tabnew");
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("close");
    app.set_focus(PaneKind::Prompt);
    app.close_pane_command(Asked::Plain);
    assert_eq!(tabs(&app), (1, 1), "`:close` too");
}

/// **Each tab keeps where its prompt is aimed, wherever you stand** (Q2,
/// "switching back restores it"): in the first tab you stand in A's window
/// with the prompt aimed at C beside it — an aim focus would not give — and
/// B is in a second tab; back in the first, the prompt goes to C, said by
/// nothing. The switch read as a focus move, and `focus_moved` aimed the
/// prompt at A, the transcript you stood in (r12-A M-1, its probe P9). This
/// test stood in A with the prompt on A, so focus re-aimed it to the right
/// answer whatever was parked, and held neither the aim's parking nor this.
#[test]
fn each_tab_keeps_where_its_prompt_is_aimed() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let a = shown_session(&app);
    let a_pane = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    app.dispatch_command_for_test("new");
    let c = app.pane_session(app.workspace.focused()).id;
    assert_ne!(c, a, "premise: C beside A");
    app.focus_pane(a_pane);
    app.settle();
    app.reveal(c);
    app.settle();
    assert_eq!(
        app.workspace.focused(),
        a_pane,
        "premise: still in A's window"
    );
    assert_eq!(app.prompt_target(), c, "premise: aimed at C, not A");
    app.dispatch_command_for_test("tabnew");
    let b = app.prompt_target();
    assert!(b != a && b != c, "premise: B, the second tab's");
    app.notices.clear();
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(
        app.workspace.focused(),
        a_pane,
        "premise: back in A's window"
    );
    assert_eq!(app.prompt_target(), c, "the first tab's aim, back");
    assert!(app.notice().is_none(), "nothing said: {:?}", app.notice());
}

/// **The aim is this tab's to show** (Q2: the aim must be on screen) — A
/// in both tabs, its transcript closed in the second: the prompt there
/// names it, as it does when no pane shows it, though the first tab's
/// does; and a view asked for from the prompt bar opens here, across the
/// top, not in the first tab's window, where nothing would show it. With
/// one transcript in each tab, the prompt names nothing: there is nothing
/// to tell apart.
#[test]
fn the_aim_and_its_window_are_this_tabs() {
    let mut app = test_app();
    let a = shown_session(&app);
    let first_tab_panes = app.workspace.pane_ids();
    app.dispatch_command_for_test(&format!("tabnew {}", a.number()));
    assert_eq!(
        prompt_title(&app),
        " prompt ",
        "one transcript a tab: nothing to say"
    );
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("close");
    assert_eq!(app.prompt_target(), a, "premise: still aimed at A");
    assert!(
        prompt_title(&app).contains("prompt →"),
        "named, no pane here showing it: {:?}",
        prompt_title(&app)
    );
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test("help");
    let help = (app
        .workspace
        .panes_of(PaneKind::Page(PageKind::Help))
        .next())
    .expect("the manual, in this tab");
    assert!(
        !first_tab_panes.contains(&help),
        "not in the first tab's window"
    );
}

/// **A decision coming back with its tab waits again** — its keys armed as
/// a call arriving is (`tab_shown`): parked, its delay ran out unseen, so a
/// key in the burst after `gt` answered a call no frame had drawn since
/// (the tab site list's finding 5; review stop 8, E 1-2's class).
#[tokio::test]
async fn a_decision_coming_back_with_its_tab_waits_again() {
    let mut app = test_app();
    let a = app.current;
    let turn = app.fake_stream_on(a);
    app.session_of_mut(a).unwrap().conversation.begin_turn("1");
    let call = shannon_core::Block::bash_call("tu_a", "rm -rf /tmp/shannon-test-absent-tu_a");
    app.feed_turn(turn, StreamMsg::ToolUse(call));
    app.feed_turn(turn, StreamMsg::Complete);
    let pane = app.decision_pane_of(a).expect("premise: its pane");
    app.read_the_call_for_test();
    assert_eq!(app.answer_keys(pane), AnswerKeys::Live, "premise: read");
    app.dispatch_command_for_test("tabnew");
    assert_eq!(app.decision_tab(a), Some(1), "premise: parked in tab 1");
    app.read_the_call_for_test();
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(
        app.decision_pane_of(a),
        Some(pane),
        "its pane, back with its tab"
    );
    assert_eq!(
        app.answer_keys(pane),
        AnswerKeys::Waiting,
        "its keys wait again"
    );
}

/// **A decision in another tab is said, with the key that goes there** —
/// `DECIDE in tab 1 · 1gt` (Q1: announced, never gone to), not "hidden";
/// `:decide` goes there, as you asked to.
#[tokio::test]
async fn a_decision_in_another_tab_is_said_and_decide_goes_there() {
    let mut app = test_app();
    let a = app.current;
    let turn = app.fake_stream_on(a);
    app.session_of_mut(a).unwrap().conversation.begin_turn("1");
    let call = shannon_core::Block::bash_call("tu_a", "rm -rf /tmp/shannon-test-absent-tu_a");
    app.feed_turn(turn, StreamMsg::ToolUse(call));
    app.feed_turn(turn, StreamMsg::Complete);
    let pane = app.decision_pane_of(a).expect("premise: its pane");
    app.dispatch_command_for_test("tabnew");
    let row = crate::view::hidden_value(&app);
    assert_eq!(row, "DECIDE in tab 1 · `1gt`");
    assert_eq!(
        crate::view::decide_value(&app, &[]).text,
        "",
        "and not \"no room to answer\": its room is in its tab"
    );
    assert!(!app.decision_in_a_pane(), "none on this tab's screen");
    app.dispatch_command_for_test(&format!("decide {}", a.number()));
    assert_eq!(tabs(&app), (2, 1), "`:decide` went to its tab");
    assert_eq!(app.workspace.focused(), pane, "and to its pane");
}

/// **Every window is reached in every tab** — a parked tab's window is
/// rows renumbered with the rest (`forget_cursors_on`, through
/// `App::windows`), and found by id (`window_of`): a stored id names its
/// window wherever its tab is.
#[test]
fn a_parked_tabs_windows_are_reached_by_every_window_walk() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let a = shown_session(&app);
    let pane = app.workspace.focused();
    for i in 0..30 {
        app.output_mut().begin_turn(&format!("q {i}"));
        app.output_mut().end_turn();
    }
    app.sync_chat();
    let view = app.window_of_mut(pane).unwrap().session_view_mut().unwrap();
    view.viewport.freeze_at(3, 0);
    app.dispatch_command_for_test("tabnew");
    assert!(app.workspace.window(pane).is_none(), "premise: parked");
    assert!(app.window_of(pane).is_some(), "found by id in its tab");
    assert_eq!(
        app.kind_of(pane),
        Some(PaneKind::Output),
        "its kind, in its tab"
    );
    assert_eq!(app.tab_of(pane), Some(0), "in the first");
    app.forget_cursors_on(a);
    let viewport = app
        .window_of(pane)
        .and_then(Window::session_view)
        .map(|v| v.viewport);
    assert!(
        viewport.is_some_and(|v| v.is_following_tail()),
        "its place let go with its rows, parked or not"
    );
}

/// **A tree in a parked tab shows the tree's marks when its tab comes
/// back** (`tab_shown`): marked in another tab's window on it, while this
/// one was parked; and asked first at the event's end, the parked one gave
/// the tree its stale marks, the other tab's lost (V4b verify, H1's class).
#[test]
fn a_parked_tree_shows_the_trees_marks_when_its_tab_comes_back() {
    let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    app.open_graph_command();
    app.settle();
    let first = app.workspace.focused();
    app.graph_view_mut().unwrap().reveal_and_select_episode(a);
    app.graph_key_for_test(k(' '));
    app.dispatch_command_for_test("tabnew");
    app.open_graph_command();
    app.settle();
    assert_eq!(
        app.graph_view().map(|v| v.marked_nodes().len()),
        Some(1),
        "premise: the tree's mark, in the second tab's window"
    );
    app.graph_view_mut().unwrap().reveal_and_select_episode(b);
    app.graph_key_for_test(k(' '));
    app.dispatch_command_for_test("tabnext 1");
    let marks = app.pane_graph(first).map(|v| v.marked_nodes().len());
    assert_eq!(marks, Some(2), "both marks, in the window that was parked");
    let tree = app
        .buffers
        .trees()
        .next()
        .map(|t| t.marks_differ(app.pane_graph(first).unwrap()));
    assert_eq!(tree, Some(false), "and the tree kept them");
}

/// **Work in a parked tab is not "in no pane"** — a session answering in
/// the first tab's window says nothing when you go to another: it is in a
/// window, which `settle_sight`'s notice would say it is not.
#[tokio::test]
async fn work_in_a_parked_tab_is_not_said_to_be_in_no_pane() {
    let mut app = test_app();
    let a = app.current;
    let _turn = app.fake_stream_on(a);
    app.settle();
    app.dispatch_command_for_test("tabnew");
    assert!(
        !(app.notices.iter()).any(|(_, text)| text.contains("in no pane")),
        "{:?}",
        app.notices
    );
}

/// **A tab parked through a resize is fitted to the frame when shown** —
/// `Event::Resize` clamps the tab shown then, and this one is clamped as
/// it comes back: the same invariant, every tab's arrangement fitted to the
/// frame it is drawn in (`shrinking_the_frame_reclamps_the_prompt`'s,
/// measured the same way, in a tall frame after the squeeze).
#[test]
fn a_tab_parked_through_a_resize_is_fitted_when_shown() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    assert!(
        !app.apply_window(crate::keymap::WindowCmd::ResizeGrow, Some(9999), 80, 50),
        "not a quit"
    );
    let prompt = (app.workspace.panes_of(PaneKind::Prompt).next()).expect("the prompt");
    let rows = |app: &App| {
        let frame = ratatui::layout::Rect::new(0, 0, 80, 50);
        (app.workspace.frame_layout(frame).pane(prompt)).map(|p| p.rect.height)
    };
    let grown = rows(&app).expect("premise: laid out");
    assert!(grown > 12, "premise: a tall prompt, {grown} rows");
    app.dispatch_command_for_test("tabnew");
    // The terminal shrinks while the first tab is parked: the event's
    // clamp reaches the tab shown, as `Event::Resize` does.
    app.frame = Some((80, 12));
    app.workspace.clamp(80, 12);
    app.dispatch_command_for_test("tabnext 1");
    let fitted = rows(&app).expect("laid out");
    assert!(
        fitted <= 12 - 4,
        "fitted to the 12-row frame: {fitted} rows"
    );
}

/// **`:tabclose` is refused over a summary's review; `:tabclose!` closes,
/// the review waiting, hidden** — a request the tab holds, as a call is
/// (Steven, 10-26: "It is an explicit command. So not a deny, a close").
/// Hidden with its session still shown, in the tab kept: in no window,
/// hidden and unplaced look alike, and deleting the reconcile's `hide`
/// passed this when the tab kept showed a fresh session (review stop 12,
/// T5 F5).
#[tokio::test]
async fn tabclose_is_refused_over_a_review() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = super::review::reviewed(&mut app);
    app.dispatch_command_for_test(&format!("tabnew {}", chat.number()));
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(
        app.decision_tab(chat),
        None,
        "premise: its pane, in this tab"
    );
    assert!(app.decision_pane_of(chat).is_some(), "premise: its pane");
    app.dispatch_command_for_test("tabclose");
    assert_eq!(tabs(&app), (2, 1), "not closed");
    // Named a review, not a "request" (review stop 11, F 13).
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("`:tabclose!` closes anyway")
                && n.ends_with("summary review waits in its pane")),
        "{:?}",
        app.notice()
    );
    app.dispatch_command_for_test("tabclose!");
    assert_eq!(tabs(&app), (1, 1), "forced: closed");
    assert_eq!(shown_session(&app), chat, "premise: its session shown");
    assert!(app.decisions.review(chat).is_some(), "the draft kept");
    assert_eq!(app.decision_pane_of(chat), None, "waiting, hidden");
    let said = format!("waits, hidden: `:decide {}`", chat.number());
    assert!(
        (app.notices.iter()).any(|(_, text)| text.contains(&said)),
        "said: {:?}",
        app.notices
    );
}

/// **A twin in another tab does not keep the chat on screen here** (review
/// stop 12, T5 F3): a request's chat closes freely only when another
/// window in THIS tab shows its session (`Over::ThisTab` in
/// `close_pane_command`) — there, the pane goes under the twin and the
/// chat stays in view. With the only other window in another tab, `:q` on
/// the chat is refused as with none, naming whose; and `:q!` hides the
/// request, not opening its pane in that tab. `EveryTab` passed the suite.
#[tokio::test]
async fn a_twin_in_another_tab_does_not_free_the_chat() {
    for kind in Request::BOTH {
        let mut app = test_app();
        let chat = kind.ask(&mut app);
        let pane = app.decision_pane_of(chat).expect("premise: its pane");
        let host = (app.window_of(pane).and_then(Window::decides)).map(|(_, h)| h);
        let host = host.expect("premise: under a window");
        app.dispatch_command_for_test(&format!("tabnew {}", chat.number()));
        assert_eq!(
            (tabs(&app), shown_session(&app)),
            ((2, 2), chat),
            "{kind:?}: premise: its session in a second tab"
        );
        app.dispatch_command_for_test("tabnext 1");
        app.focus_pane(host);
        app.settle();
        app.notices.clear();

        app.dispatch_command_for_test("q");
        assert!(
            app.workspace.pane_ids().contains(&host),
            "{kind:?}: not closed"
        );
        assert_eq!(
            app.notice().unwrap_or_default(),
            format!(
                "not closed — `:hide` closes anyway (it waits, hidden); b{}'s {} waits in its \
                 pane",
                chat.number(),
                kind.named()
            ),
            "{kind:?}: said, whose"
        );

        app.dispatch_command_for_test("q!");
        assert!(
            !app.workspace.pane_ids().contains(&host),
            "{kind:?}: forced: closed"
        );
        waits_hidden(&app, chat);
        app.dispatch_command_for_test("tabnext 2");
        assert_eq!(
            app.decision_pane_of(chat),
            None,
            "{kind:?}: nor opened in the other tab"
        );
    }
}

/// Row `y` of a drawn buffer, as text.
fn row(buf: &ratatui::buffer::Buffer, y: u16) -> String {
    (0..buf.area().width)
        .map(|x| buf[(x, y)].symbol())
        .collect()
}

/// **The tab line names each tab and picks out yours** (vim's
/// `showtabline=1`): none with one tab, the panes from the top; with two,
/// a line along the top — each tab's number and what its focused pane
/// shows — yours in `ui.tabline.selected`, the other in `ui.tabline`, the
/// panes below it in every tab, and the status row last. Back to one tab,
/// the line goes. Drawn and asserted cell by cell: a role asserted and a
/// style never rendered is the gap this project shipped twice.
#[test]
fn the_tab_line_names_each_tab_and_picks_out_yours() {
    let mut app = test_app();
    let one = app
        .workspace
        .frame_layout(ratatui::layout::Rect::new(0, 0, 80, 24));
    assert_eq!(one.tabline.height, 0, "one tab: no line");
    assert_eq!(
        one.panes.iter().map(|p| p.rect.y).min(),
        Some(0),
        "the panes from the top"
    );

    app.dispatch_command_for_test("tabnew");
    let buf = frame_of(&mut app, 80, 24);
    let line = row(&buf, 0);
    assert!(
        line.starts_with(" 1 ") && line.contains(" 2 "),
        "both named: {line:?}"
    );
    let (other, yours) = (
        app.theme().ui(Ink::TabLine),
        app.theme().ui(Ink::TabLineSelected),
    );
    assert_ne!(other, yours, "premise: two looks");
    let at = |n: &str| line.find(&format!(" {n} ")).expect("a label") as u16 + 1;
    let style = |x: u16| {
        let cell = &buf[(x, 0)];
        (cell.fg, cell.bg)
    };
    assert_eq!(
        style(at("2")),
        (yours.fg.unwrap(), yours.bg.unwrap()),
        "yours picked out"
    );
    assert_eq!(style(at("1")).0, other.fg.unwrap(), "the other quiet");
    let layout = app.workspace.frame_layout(*buf.area());
    assert_eq!(
        layout.panes.iter().map(|p| p.rect.y).min(),
        Some(1),
        "the panes below it"
    );
    assert_eq!(layout.status.y, 23, "the status row last");

    app.dispatch_command_for_test("tabnext 1");
    let layout = app.workspace.frame_layout(*buf.area());
    assert_eq!(
        layout.panes.iter().map(|p| p.rect.y).min(),
        Some(1),
        "in the first tab too"
    );
    app.dispatch_command_for_test("tabclose");
    let layout = app.workspace.frame_layout(*buf.area());
    assert_eq!(layout.tabline.height, 0, "one tab again: no line");
    assert_eq!(
        layout.panes.iter().map(|p| p.rect.y).min(),
        Some(0),
        "the panes from the top"
    );
}

/// **A click on the tab line is on no pane, and a narrow line keeps every
/// number** — the row is above the panes for the mouse as for the draw
/// (one body, `Workspace::body`); cut to share what is left, the names go
/// before the numbers `{N}gt` needs.
#[test]
fn the_tab_line_is_no_pane_and_keeps_its_numbers_narrow() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnew");
    let frame = ratatui::layout::Rect::new(0, 0, 80, 24);
    for x in [0, 10, 40, 79] {
        let pos = ratatui::layout::Position::new(x, 0);
        assert_eq!(
            app.workspace.pane_at(frame, pos),
            None,
            "x {x}: the tab line, no pane"
        );
    }
    assert!(
        app.workspace
            .pane_at(frame, ratatui::layout::Position::new(5, 1))
            .is_some()
    );
    let line = row(&frame_of(&mut app, 12, 10), 0);
    for n in ["1", "2", "3"] {
        assert!(
            line.contains(&format!(" {n} ")),
            "tab {n} numbered in 12 cells: {line:?}"
        );
    }
}

/// **The tab line's row is kept out of the clamp** — a prompt grown to the
/// most a 12-row frame allows stops one row shorter with the line than
/// without it (the clamp's reserve, `Workspace::clamp`, counts the row).
/// Measured as `shrinking_the_frame_reclamps_the_prompt` measures, in a
/// tall frame after: what the clamp keeps is the size, which the draw then
/// fits either way.
#[test]
fn the_tab_line_row_is_kept_out_of_the_clamp() {
    let grown = |tabs: usize| {
        let mut app = test_app();
        for _ in 1..tabs {
            app.dispatch_command_for_test("tabnew");
        }
        app.set_focus(PaneKind::Prompt);
        assert!(
            !app.apply_window(crate::keymap::WindowCmd::ResizeGrow, Some(9999), 80, 12),
            "not a quit"
        );
        let prompt = (app.workspace.panes_of(PaneKind::Prompt).next()).expect("the prompt");
        let layout = app
            .workspace
            .frame_layout(ratatui::layout::Rect::new(0, 0, 80, 50));
        layout
            .pane(prompt)
            .map(|p| p.rect.height)
            .expect("laid out")
    };
    let (one, two) = (grown(1), grown(2));
    assert_eq!(
        two,
        one - 1,
        "the tab line's row kept: {one} rows with one tab, {two} with two"
    );
}

/// A call asked by session `a` from a fake stream.
fn ask_a_call(app: &mut App, a: BufferId) {
    let turn = app.fake_stream_on(a);
    app.session_of_mut(a).unwrap().conversation.begin_turn("1");
    let call = shannon_core::Block::bash_call("tu_a", "rm -rf /tmp/shannon-test-absent-tu_a");
    app.feed_turn(turn, StreamMsg::ToolUse(call));
    app.feed_turn(turn, StreamMsg::Complete);
}

/// **A decision for a session shown only in another tab opens in THAT
/// tab** (`tabs.md` Q1 (a)) — announced, never gone to: the pane opens
/// under the session's window there, the tab you are in and its focus as
/// they were; the tab line draws that tab in the hue of an ask, the status
/// row and `:ls` name it with the key that goes there; and shown, its keys
/// wait as a call's just arrived do. It waited, "in no window", until T3.
#[tokio::test]
async fn a_decision_for_a_session_in_another_tab_opens_there() {
    let mut app = test_app();
    let a = app.current;
    app.dispatch_command_for_test("tabnew");
    let here = app.workspace.pane_ids();
    let focused = app.workspace.focused();
    ask_a_call(&mut app, a);
    let pane = app.decision_pane_of(a).expect("its pane");
    assert!(
        !here.contains(&pane) && app.workspace.window(pane).is_none(),
        "not in this tab"
    );
    assert_eq!(app.decision_tab(a), Some(1), "in the first tab, where A is");
    assert_eq!(app.workspace.focused(), focused, "nothing moved here");
    assert_eq!(tabs(&app), (2, 2), "and no tab gone to");
    assert_eq!(crate::view::hidden_value(&app), "DECIDE in tab 1 · `1gt`");
    let ls = app.buffers_listing(false);
    assert!(ls.contains("needs you — in tab 1, `1gt`"), "{ls}");
    assert!(ls.contains("pane t1:"), "its window named in its tab: {ls}");

    let labels = app.tab_labels();
    assert!(labels[0].decides && !labels[1].decides, "{labels:?}");
    let buf = frame_of(&mut app, 80, 24);
    let line = row(&buf, 0);
    let x = line.find(" 1 ").expect("a label") as u16 + 1;
    let caution = app.theme().ui(Ink::Caution);
    assert_eq!(
        buf[(x, 0)].fg,
        caution.fg.unwrap(),
        "drawn as an ask waiting: {line:?}"
    );

    app.read_the_call_for_test();
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(
        app.answer_keys(pane),
        AnswerKeys::Waiting,
        "shown, its keys wait"
    );
}

/// **A review's sentence names the tab its pane will open in** — beside its
/// session, which only another tab shows (`App::where_shown`): "beside the
/// session" alone pointed at a pane you could not see.
#[test]
fn a_reviews_sentence_names_the_tab_it_opens_in() {
    let mut app = test_app();
    let a = app.current;
    assert_eq!(
        app.review_when_written(a),
        "its review opens beside the session when it is written",
        "premise: here"
    );
    app.dispatch_command_for_test("tabnew");
    assert_eq!(
        app.review_when_written(a),
        "its review opens beside the session, in tab 1, when it is written"
    );
}

/// **A tab whose session has news wears the alerts' mark** — the glyph
/// `:ls`'s mark column shows (the shipped `unshown_session` alert marks),
/// on the tab line, while you are in another tab: the hotlist's "new
/// output", user-defined as every alert is.
#[tokio::test]
async fn a_tab_with_news_wears_the_alerts_mark() {
    let mut app = test_app();
    let a = app.current;
    let turn = app.fake_stream_on(a);
    app.session_of_mut(a).unwrap().conversation.begin_turn("q");
    app.dispatch_command_for_test("tabnew");
    frame_of(&mut app, 80, 24);
    assert!(
        app.tab_labels().iter().all(|l| l.mark.is_none()),
        "premise: no news"
    );
    app.feed_turn(turn, StreamMsg::Chunk("news".into()));
    app.feed_turn(turn, StreamMsg::Complete);
    frame_of(&mut app, 80, 24);
    let labels = app.tab_labels();
    assert!(labels[0].mark.is_some(), "the first tab marked: {labels:?}");
    assert!(labels[1].mark.is_none(), "this one not");
    let glyph = labels[0].mark.clone().unwrap();
    assert!(
        row(&frame_of(&mut app, 80, 24), 0).contains(&glyph),
        "drawn"
    );
}

/// **A decision pane closed with its tab waits, hidden, while its session
/// is shown in another tab** — the reconcile's door, every window gone
/// through it (`Tabs::take_dropped`): A shown in the third tab, parked, and
/// the tab shown after the close the second, which does not show it.
/// `:tabclose` is refused; `:tabclose!` answers nothing (Steven, 10-26: "So
/// not a deny, a close"), and A's pane does not open again in the third.
#[tokio::test]
async fn a_decision_closed_with_its_tab_waits_hidden_when_shown_elsewhere() {
    let mut app = test_app();
    let a = app.current;
    let turn = app.fake_stream_on(a);
    app.session_of_mut(a).unwrap().conversation.begin_turn("1");
    let call = shannon_core::Block::bash_call("tu_a", "rm -rf /tmp/shannon-test-absent-tu_a");
    app.feed_turn(turn, StreamMsg::ToolUse(call));
    app.feed_turn(turn, StreamMsg::Complete);
    assert!(app.decision_pane_of(a).is_some(), "premise: its pane");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test(&format!("tabnew {}", a.number()));
    assert_eq!(
        (tabs(&app), shown_session(&app)),
        ((3, 3), a),
        "premise: A in the third"
    );
    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test("tabclose");
    assert_eq!(tabs(&app), (3, 1), "refused");
    app.dispatch_command_for_test("tabclose!");
    assert_ne!(
        shown_session(&app),
        a,
        "premise: the tab shown does not show A"
    );
    assert_eq!(tabs(&app).0, 2, "closed");
    waits_hidden(&app, a);
}

/// **`:tabmove` puts the tab where vim's does** — every case measured in
/// Neovim 0.12.6 (`tabs.md` §6b), from tab 2 of 4 unless said: bare and `$`
/// the end; `N` after tab N, `0` the front, N itself or the tab before it
/// no move; `+N`/`-N` and bare `+`/`-` relative; past either end refused,
/// never wrapped (`+1` from the last too); a word not a place refused.
#[test]
fn tabmove_puts_the_tab_where_vims_does() {
    use crate::app::tabs::move_to;
    let from_2_of_4 = [
        ("", Some(3)),
        ("$", Some(3)),
        ("0", Some(0)),
        ("1", Some(1)),
        ("2", Some(1)),
        ("3", Some(2)),
        ("4", Some(3)),
        ("+", Some(2)),
        ("-", Some(0)),
        ("+1", Some(2)),
        ("-1", Some(0)),
        ("+2", Some(3)),
        ("+3", None),
        ("+5", None),
        ("-2", None),
        ("9", None),
        ("5", None),
        ("x", None),
        ("0x", None),
        ("+x", None),
        ("++1", None),
    ];
    for (arg, want) in from_2_of_4 {
        assert_eq!(
            move_to(arg, 1, 4).ok(),
            want,
            "`:tabmove {arg}` from 2 of 4"
        );
    }
    assert_eq!(move_to("+1", 3, 4).ok(), None, "from the last: no wrap");
    assert_eq!(move_to("-", 0, 4).ok(), None, "from the first: no wrap");
    assert_eq!(move_to("", 0, 1), Ok(0), "one tab: where it is");
}

/// The names the tab line gives, in tab order.
fn tab_names(app: &App) -> Vec<String> {
    app.tab_labels().into_iter().map(|l| l.name).collect()
}

/// **A tab's name is its place's** (`:tabrename`, Q3): given, the tab line
/// shows it instead of what the pane shows; it stays on its tab while
/// another is shown, moves with it (`:tabmove`), and is taken off by
/// `:tabrename!`. Bare, it is said; a name that is not one line of plain
/// text, or longer than a session's may be, is refused — spaces are
/// allowed, a tab's name being read and never typed back.
#[test]
fn a_tabs_name_is_its_places_and_moves_with_it() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabrename");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("tab 1 has no name")),
        "{:?}",
        app.notice()
    );
    app.dispatch_command_for_test("tabrename  front end  ");
    assert_eq!(
        app.notice().as_deref(),
        Some("named tab 1 `front end`"),
        "trimmed, spaces kept"
    );
    app.dispatch_command_for_test("tabrename front end");
    assert_eq!(
        app.notice().as_deref(),
        Some("tab 1 is already `front end`")
    );
    app.dispatch_command_for_test("tabnew");
    let handle = tab_names(&app)[1].clone();
    assert_eq!(tab_names(&app)[0], "front end", "kept while parked");
    app.dispatch_command_for_test("tabrename back");
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(tab_names(&app), ["front end", "back"], "each its own");
    let line = row(&frame_of(&mut app, 80, 24), 0);
    assert!(
        line.contains(" 1 front end ") && line.contains(" 2 back "),
        "drawn: {line:?}"
    );

    app.dispatch_command_for_test("tabmove");
    assert_eq!(tabs(&app), (2, 2), "moved to the end, still shown");
    assert_eq!(
        tab_names(&app),
        ["back", "front end"],
        "the names with their tabs"
    );

    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test("tabrename! back");
    assert!(app.notice().is_some_and(|n| n.contains("takes no text")));
    app.dispatch_command_for_test("tabrename!");
    assert_eq!(
        app.notice().as_deref(),
        Some("took the name `back` off tab 1")
    );
    assert_eq!(tab_names(&app)[0], handle, "what its pane shows again");
    app.dispatch_command_for_test("tabrename!");
    assert_eq!(
        app.notice().as_deref(),
        Some("tab 1 has no name to take off")
    );

    app.dispatch_command_for_test(&format!("tabrename {}", "x".repeat(49)));
    assert!(app.notice().is_some_and(|n| n.contains("at most 48")));
    app.dispatch_command_for_test("tabrename a\u{202E}b");
    assert!(app.notice().is_some_and(|n| n.contains("plain text")));
    assert_eq!(tab_names(&app)[0], handle, "neither given");
}

/// **`:tabonly` closes every tab but this one, or but tab N** (vim's,
/// measured): the sessions stay open, the tab line goes with the last
/// other tab; one tab is said; a tab not there is refused.
#[test]
fn tabonly_closes_every_other_tab_and_keeps_their_sessions() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabonly");
    assert_eq!(app.notice().as_deref(), Some("this is the only tab"));
    let first = shown_session(&app);
    app.dispatch_command_for_test("tabnew");
    let second = shown_session(&app);
    app.dispatch_command_for_test("tabnew");
    let third = shown_session(&app);
    app.dispatch_command_for_test("tabnext 2");
    app.dispatch_command_for_test("tabonly 9");
    assert_eq!(tabs(&app), (3, 2), "no tab 9: nothing closed");
    assert_eq!(
        app.notice().as_deref(),
        Some("`:tabonly`: there is no tab 9 — this is tab 2 of 3")
    );
    app.dispatch_command_for_test("tabonly +x");
    assert!(app.notice().is_some_and(|n| n.contains("not a tab number")));

    app.dispatch_command_for_test("tabonly");
    assert_eq!(tabs(&app), (1, 1), "this one alone");
    assert_eq!(shown_session(&app), second, "the one you were in");
    assert_eq!(
        app.notice().as_deref(),
        Some("closed 2 tabs — their sessions stay open (`:ls`)")
    );
    assert!(app.session_of(first).is_some() && app.session_of(third).is_some());
    let layout = app
        .workspace
        .frame_layout(ratatui::layout::Rect::new(0, 0, 80, 24));
    assert_eq!(layout.tabline.height, 0, "no tab line");
    assert_eq!(layout.panes.iter().map(|p| p.rect.y).min(), Some(0));

    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnew");
    let kept = shown_session(&app);
    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test("tabonly 3");
    assert_eq!(tabs(&app), (1, 1), "tab 3 alone");
    assert_eq!(shown_session(&app), kept, "shown first");
}

/// **`:tabonly` and `:tabclose N` are refused over another tab's review,
/// naming it** — as `:tabclose` is over this one's (`App::held_in_tab`,
/// the one rule); kept by `:tabonly N` itself, the review's tab is the one
/// left.
#[tokio::test]
async fn tabonly_is_refused_over_another_tabs_review() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = super::review::reviewed(&mut app);
    app.dispatch_command_for_test("tabnew");
    assert_eq!(
        app.decision_tab(chat),
        Some(1),
        "premise: its pane in tab 1"
    );
    app.dispatch_command_for_test("tabonly");
    assert_eq!(tabs(&app), (2, 2), "nothing closed");
    assert_eq!(
        app.notice().unwrap_or_default(),
        format!(
            "`:tabonly`: not closed — `:tabonly!` closes anyway (it waits, hidden); in tab 1 \
             (`1gt`), b{}'s summary review waits in its pane",
            chat.number()
        )
    );
    app.dispatch_command_for_test("tabclose 1");
    assert_eq!(tabs(&app), (2, 2), "nothing closed");
    assert!(
        app.notice()
            .is_some_and(|n| n.starts_with("`:tabclose`: not closed — ")
                && n.contains("; in tab 1 (`1gt`), ")),
        "{:?}",
        app.notice()
    );
    app.dispatch_command_for_test("tabonly 1");
    assert_eq!(tabs(&app), (1, 1), "its tab kept");
    assert!(
        app.decision_pane_of(chat).is_some(),
        "the review's pane with it"
    );
}

/// **A decision pane closed by `:tabonly!` goes through the reconcile** —
/// waiting, hidden, though its session is shown in the tab kept, as one
/// closed with `:tabclose!` is (`Tabs::close_others` keeps the windows for
/// it); `:tabonly` refused first. It was denied here: the close answered.
#[tokio::test]
async fn a_decision_closed_by_tabonly_waits_hidden_when_shown_in_the_tab_kept() {
    for kind in Request::BOTH {
        let mut app = test_app();
        let a = kind.ask(&mut app);
        app.dispatch_command_for_test(&format!("tabnew {}", a.number()));
        assert_eq!(
            (tabs(&app), shown_session(&app)),
            ((2, 2), a),
            "{kind:?}: premise: A here too"
        );
        assert_eq!(
            app.decision_tab(a),
            Some(1),
            "{kind:?}: premise: its pane in tab 1"
        );
        app.dispatch_command_for_test("tabonly");
        assert_eq!(tabs(&app), (2, 2), "{kind:?}: refused");
        app.dispatch_command_for_test("tabonly!");
        assert_eq!(tabs(&app), (1, 1), "{kind:?}");
        waits_hidden(&app, a);
    }
}

/// **A decision pane closed with a PARKED tab goes through the reconcile**
/// — `:tabclose! 1` from tab 2, A shown here: waiting, hidden, as one
/// closed with the tab you are in is (`Tabs::close` keeps a parked tab's
/// windows for it); `:tabclose 1` refused first.
#[tokio::test]
async fn a_decision_closed_with_a_parked_tab_waits_hidden_when_shown_here() {
    let mut app = test_app();
    let a = app.current;
    ask_a_call(&mut app, a);
    app.dispatch_command_for_test(&format!("tabnew {}", a.number()));
    assert_eq!(app.decision_tab(a), Some(1), "premise: its pane in tab 1");
    app.dispatch_command_for_test("tabclose 1");
    assert_eq!(tabs(&app), (2, 2), "refused");
    app.dispatch_command_for_test("tabclose! 1");
    assert_eq!((tabs(&app), shown_session(&app)), ((1, 1), a));
    waits_hidden(&app, a);
}

/// **Closing another tab leaves this one's decision answerable** — the tab
/// you are in is not shown anew (`close_tab` runs `tab_shown` only for the
/// tab it closed in), so a call you have read keeps its keys: re-armed, the
/// keys typed after `:tabclose 2` would wait out a delay for a call already
/// on screen.
#[tokio::test]
async fn closing_another_tab_leaves_this_tabs_decision_answerable() {
    let mut app = test_app();
    let a = app.current;
    ask_a_call(&mut app, a);
    let pane = app.decision_pane_of(a).expect("its pane");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnext 1");
    app.read_the_call_for_test();
    assert_eq!(
        app.answer_keys(pane),
        AnswerKeys::Live,
        "premise: read, live"
    );
    app.dispatch_command_for_test("tabclose 2");
    assert_eq!(tabs(&app), (1, 1), "premise: closed");
    assert_eq!(app.answer_keys(pane), AnswerKeys::Live, "not armed again");
}

/// The column where `needle` begins on row `y` of a drawn buffer, cell by
/// cell ([`cells_of`]) — a wide glyph is one symbol in its first cell, so a
/// byte offset into the row's text is not a column.
fn col_of(buf: &ratatui::buffer::Buffer, y: u16, needle: &str) -> u16 {
    let at = cells_of(buf, needle).find(|at| at.y == y);
    at.unwrap_or_else(|| panic!("`{needle}` not drawn on row {y}: {:?}", row(buf, y)))
        .x
}

/// **A click on the tab line goes to the tab under it** — on its number,
/// on its name, after a wide mark; found where the line DREW each label, so
/// the click and the draw are measured apart (one layout,
/// `view::tabline_texts`). Past the last label it does nothing; on a pane's
/// row it is the pane's; with one tab there is no line to click.
#[tokio::test]
async fn a_click_on_the_tab_line_goes_to_the_tab_under_it() {
    use ratatui::layout::{Position, Rect};
    let frame = Rect::new(0, 0, 80, 24);
    let mut app = test_app();
    assert!(
        !app.click_tab_line(frame, Position::new(1, 0)),
        "one tab: no line"
    );
    let a = app.current;
    let turn = app.fake_stream_on(a);
    app.session_of_mut(a).unwrap().conversation.begin_turn("q");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabrename second tab");
    app.dispatch_command_for_test("tabnew");
    frame_of(&mut app, 80, 24);
    app.feed_turn(turn, StreamMsg::Chunk("news".into()));
    app.feed_turn(turn, StreamMsg::Complete);
    let buf = frame_of(&mut app, 80, 24);
    let glyph = (app.tab_labels()[0].mark.clone()).expect("premise: tab 1 marked");
    assert!(crate::wrap::str_cells(&glyph) > 1, "premise: a wide mark");
    let line = row(&buf, 0);
    assert!(line.contains(" 2 second tab "), "premise: {line:?}");

    let at = |needle: &str| Position::new(col_of(&buf, 0, needle), 0);
    assert!(app.click_tab_line(frame, at("2 second")), "the line's");
    assert_eq!(tabs(&app), (3, 2), "on its number, past tab 1's wide mark");
    assert!(app.click_tab_line(frame, at(" 3 ")));
    assert_eq!(tabs(&app), (3, 3));
    assert!(app.click_tab_line(frame, at("tab ")));
    assert_eq!(tabs(&app), (3, 2), "on its name");
    assert!(app.click_tab_line(frame, Position::new(79, 0)));
    assert_eq!(tabs(&app), (3, 2), "past the last label: nothing");
    assert!(
        !app.click_tab_line(frame, Position::new(col_of(&buf, 0, " 1 "), 1)),
        "the row below is a pane's"
    );
    assert_eq!(tabs(&app), (3, 2));
    assert!(app.click_tab_line(frame, at(" 1 ")));
    assert_eq!(tabs(&app), (3, 1), "tab 1");
}

/// **The last transcript closed in a tab says where its session still is**
/// — "in no pane here — tab 1 shows it", and no `:q` offered while a viewer
/// is left (the prompt's `:q` would refuse, "the only prompt"). It said "in
/// no pane" and offered `:q` either way (r12-F M5).
#[test]
fn the_last_transcript_closed_in_a_tab_says_where_its_session_is() {
    let mut app = test_app();
    let a = app.current;
    app.dispatch_command_for_test(&format!("tabnew {}", a.number()));
    app.dispatch_command_for_test("sgraph");
    app.set_focus(PaneKind::Output);
    assert!(!app.dispatch_command_for_test("q"), "premise: not leaving");
    app.settle();
    assert!(
        !app.workspace.has(PaneKind::Output),
        "premise: no transcript here"
    );
    let n = a.number();
    assert_eq!(
        app.notice(),
        Some(format!(
            "the last transcript closed — b{n} is in no pane here — tab 1 shows it (`:b{n}` shows it)"
        ))
    );
}

/// **A tab comes back aimed where it was, though the foreground moved**
/// (r12-F H1). The foreground is the app's and moves with every tab:
/// `:tabnew` moves it to the new tab's session, `:new` in tab 1 moves it
/// again. Each tab's prompt keeps its own aim — its window's (10-26) — so
/// back in tab 1 the prompt goes to A, and back in tab 2 to B, the titles
/// plain. It was an aim of `None`, "follow the foreground", pinned as each
/// tab was parked (`pin_aim`); unpinned, a tab came back aimed at the other
/// tab's session, said only by the title. Standing in the prompt: a
/// transcript's focus re-aims it on return whatever it kept.
#[test]
fn a_tab_parked_comes_back_aimed_where_it_was() {
    let mut app = test_app();
    let a = shown_session(&app);
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test("tabnew");
    let b = shown_session(&app);
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(app.prompt_target(), a, "tab 1's own aim");
    assert_eq!(prompt_title(&app), " prompt ");

    // Tab 2 left standing in its prompt, and tab 1 moving the foreground.
    app.dispatch_command_for_test("tabnext 2");
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test("new");
    assert_ne!(app.current, b, "premise: the foreground moved");
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test("tabnext 2");
    assert_eq!(app.prompt_target(), b, "tab 2's own aim");
    assert_eq!(prompt_title(&app), " prompt ");
}

/// **A prompt height set in one tab reaches every tab** — `:set
/// tui.prompt.height=15` in tab 2, and tab 1's prompt is 15 rows when it
/// comes back (`App::workspaces_mut`). It was set on the tab shown alone
/// (r12-A M-2, its probe P2).
#[test]
fn a_prompt_height_set_in_one_tab_reaches_every_tab() {
    use ratatui::{Terminal, backend::TestBackend};
    let frame = ratatui::layout::Rect::new(0, 0, 80, 40);
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let first = (app.workspace.panes_of(PaneKind::Prompt).next()).unwrap();
    let height = |app: &App, p| app.pane_content_size(p, frame).1;
    let before = height(&app, first);
    assert_ne!(before, 15, "premise");
    app.dispatch_command_for_test("tabnew");
    draw_frame(&mut app, &mut terminal);
    app.dispatch_command_for_test("set tui.prompt.height=15");
    let here = (app.workspace.panes_of(PaneKind::Prompt).next()).unwrap();
    assert_eq!(height(&app, here), 15, "premise: this tab's");
    app.dispatch_command_for_test("tabnext 1");
    draw_frame(&mut app, &mut terminal);
    assert_eq!(
        height(&app, first),
        15,
        "the parked tab's too (was {before})"
    );
}

/// **`:ls` names a session and a tree shown only in another tab** — `a`,
/// drawn in a pane, and the pane column `t1:N`, for both (`holdings` and the
/// pane labels walk every tab). Flipped to this tab, the session read `h`
/// in no pane (r12-A M-3, its probe P1; the reach F7).
#[test]
fn ls_names_a_session_and_a_tree_shown_only_in_another_tab() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let a = shown_session(&app);
    app.split_command("", SplitDir::Vertical);
    app.open_graph_command();
    app.settle();
    assert!(
        app.workspace.has(PaneKind::Graph),
        "premise: a tree in tab 1"
    );
    app.dispatch_command_for_test("tabnew");
    let ls = app.buffers_listing(true);
    let row = |n: u64| {
        (ls.lines())
            .find(|l| l.trim_start().starts_with(&format!("b{n} ")))
            .unwrap_or_else(|| panic!("no row b{n}: {ls}"))
            .to_string()
    };
    // `  b{n:<3} {flags}`: the flags at 7..9.
    let flags = |row: &str| row.get(7..9).unwrap_or_default().to_string();
    let row_a = row(a.number());
    assert_eq!(flags(&row_a), " a", "{row_a}");
    assert!(row_a.contains("pane t1:"), "{row_a}");
    let tree = (app.buffers.trees().next()).expect("the tree").id();
    let row_t = row(tree.number());
    assert_eq!(flags(&row_t), "ua", "{row_t}");
    assert!(row_t.contains("pane t1:"), "{row_t}");
}

/// **`gt` works from the prompt's Normal mode and from every Visual** — the
/// three key paths but the viewer's Normal one, which no test drove (r12-A
/// M-4, probes P4, P5, P5b): Visual ends, as vim's; and `dgt` in the prompt
/// cancels with the text kept (P7).
#[test]
fn gt_works_from_the_prompts_normal_and_every_visual() {
    let typed = |app: &mut App, text: &str| type_keys(app, &format!("i{text}<Esc>"));
    let mut app = test_app();
    for i in 0..5 {
        app.output_mut().begin_turn(&format!("q {i}"));
        app.output_mut().end_turn();
    }
    app.dispatch_command_for_test("tabnew");
    app.set_focus(PaneKind::Prompt);
    typed(&mut app, "hello");
    assert_eq!(
        app.prompt_keymap.mode(),
        crate::keymap::Mode::Normal,
        "premise"
    );
    type_keys(&mut app, "gt");
    assert_eq!(tabs(&app), (2, 1), "from the prompt's Normal");

    app.set_focus(PaneKind::Output);
    type_keys(&mut app, "v");
    assert!(app.chat_keymap.mode().is_visual(), "premise: Visual");
    type_keys(&mut app, "gt");
    assert_eq!(tabs(&app), (2, 2), "from a transcript's Visual");
    app.dispatch_command_for_test("tabnext 1");
    assert!(!app.chat_keymap.mode().is_visual(), "Visual ended");

    app.set_focus(PaneKind::Prompt);
    type_keys(&mut app, "v");
    assert!(app.prompt_keymap.mode().is_visual(), "premise: Visual");
    type_keys(&mut app, "gt");
    assert_eq!(tabs(&app), (2, 2), "from the prompt's Visual");

    app.set_focus(PaneKind::Prompt);
    type_keys(&mut app, "<Esc>");
    let text = app.prompt().text().to_string();
    type_keys(&mut app, "0dgt");
    assert_eq!(tabs(&app), (2, 2), "`dgt`: no tab moved");
    assert_eq!(app.prompt().text(), text, "and nothing deleted");
}

/// **A landing asked for in this tab is made visible here** — `reveal`, the
/// door `:continue`, `:clone` and `:open <turn>` land by, finds a window in
/// THIS tab, else where `:b N` would put it: entered in another tab's
/// window, the reply streamed and billed where nobody could see it (r12-A
/// HIGH-2, F8).
#[test]
fn a_landing_from_another_tab_is_made_visible_here() {
    let mut app = test_app();
    let a = shown_session(&app);
    app.dispatch_command_for_test("tabnew");
    app.reveal(a);
    app.settle();
    assert!(
        app.panes_on(a, Over::ThisTab).next().is_some(),
        "shown in this tab"
    );
    assert_eq!(app.prompt_target(), a);
}

/// **A call you type here shows its session here** — `:tool` from a tab
/// whose prompt aims at A, which only tab 1 shows: A is shown in this tab
/// and the call's pane opens under it, nothing gone to (`show_if_unseen`
/// asks THIS tab; r12-A HIGH-2, F4).
#[tokio::test]
async fn a_typed_call_shows_its_session_here_when_only_another_tab_does() {
    let mut app = test_app();
    let a = app.current;
    app.dispatch_command_for_test(&format!("tabnew {}", a.number()));
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("close");
    assert!(
        !app.workspace.has(PaneKind::Output),
        "premise: no transcript here"
    );
    assert_eq!(app.prompt_target(), a, "premise: aimed at A");
    app.dispatch_command_for_test("tool rm -rf /tmp/shannon-test-absent-tabs");
    assert_eq!(tabs(&app), (2, 2), "nothing gone to");
    assert!(
        app.panes_with_session(a, Over::ThisTab).next().is_some(),
        "A shown here"
    );
    assert!(app.decision_pane_of(a).is_some(), "premise: a call asked");
    assert_eq!(app.decision_tab(a), None, "its pane here");
}

/// **A decision for a session shown here and in another tab opens here** —
/// under the window you can see, the tab shown first in `fit_decision_pane`
/// (r12-A HIGH-2, F9).
#[tokio::test]
async fn a_decision_for_a_session_shown_here_and_elsewhere_opens_here() {
    let mut app = test_app();
    let a = app.current;
    app.dispatch_command_for_test(&format!("tabnew {}", a.number()));
    ask_a_call(&mut app, a);
    let pane = app.decision_pane_of(a).expect("its pane");
    assert!(app.workspace.window(pane).is_some(), "in this tab");
    assert_eq!(app.decision_tab(a), None);
}

/// **`:new` does not reuse a blank session another tab shows** — from its
/// transcript here, and from the prompt with no transcript here: reused, the
/// other tab's window would change under it (`shown_elsewhere` and
/// `blank_buffer` walk every tab; r12-A HIGH-2, F6).
#[test]
fn new_does_not_reuse_a_blank_session_another_tab_shows() {
    let mut app = test_app();
    let a = app.current;
    assert!(app.is_blank(a), "premise: launch's session is blank");
    app.dispatch_command_for_test(&format!("tabnew {}", a.number()));
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("new");
    assert_ne!(shown_session(&app), a, "a fresh session here");
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(shown_session(&app), a, "tab 1's window as it was");

    app.dispatch_command_for_test(&format!("tabnew {}", a.number()));
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("close");
    assert_eq!(
        app.prompt_target(),
        a,
        "premise: aimed at A, in no pane here"
    );
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test("new");
    assert_ne!(
        app.prompt_target(),
        a,
        "from the prompt too: a fresh session"
    );
}

/// **A composition does not fill a blank session another tab shows** — the
/// clone gets a buffer of its own (`unnoticed_if_reused` counts every tab's
/// windows; r12-A HIGH-2, F6).
#[tokio::test]
async fn a_composition_does_not_fill_a_blank_session_another_tab_shows() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    app.new_session_command();
    let blank = app.current;
    app.dispatch_command_for_test(&format!("tabnew {}", blank.number()));
    assert_eq!(app.current, blank, "premise: on the blank one, in two tabs");
    app.clone_episode(original);
    app.settle();
    assert_ne!(app.current, blank, "a buffer of its own");
    assert!(
        (app.session_of(blank)).is_some_and(|b| b.conversation.episode().segments().is_empty()),
        "the blank one untouched"
    );
}

/// **Closing a tab's last transcript keeps the tab's aim** — B, which this
/// tab showed, not tab 1's A: the re-aim's fallback is this tab's first
/// transcript, else the foreground (r12-A HIGH-2, F3).
#[test]
fn closing_a_tabs_last_transcript_keeps_its_aim() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    let b = shown_session(&app);
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("close");
    assert_eq!(app.prompt_target(), b);
}

/// **The tab line names what each tab's focused pane shows, in tab order** —
/// the prompt's aim (tab 1), a viewer's kind (tab 2), a companion's host
/// (tab 3: the inspector is the tree's window, F1 — it read "inspector",
/// r12-F L6), a decision's session (tab 4); with tab 1 current, the order
/// is the tabs' (the parked ones are gathered first and sorted). None of it
/// was held (r12-F M3).
#[tokio::test]
async fn the_tab_line_names_what_each_tabs_focused_pane_shows() {
    let mut app = test_app();
    let a = app.current;
    let handle =
        |app: &App, b: BufferId| app.session_of(b).unwrap().conversation.episode().handle();
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test("tabnew");
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("help");
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Help),
        "premise: a viewer focused"
    );
    app.dispatch_command_for_test("tabnew");
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.settle();
    app.set_focus(PaneKind::Inspector);
    assert_eq!(
        app.focus(),
        PaneKind::Inspector,
        "premise: the inspector focused"
    );
    app.dispatch_command_for_test("tabnew");
    let c = shown_session(&app);
    ask_a_call(&mut app, c);
    let pane = app.decision_pane_of(c).expect("its pane");
    app.focus_pane(pane);
    app.settle();
    assert_eq!(
        app.workspace.focused(),
        pane,
        "premise: in the decision pane"
    );
    app.dispatch_command_for_test("tabnext 1");
    let labels = app.tab_labels();
    let names: Vec<&str> = labels.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        [
            handle(&app, a).as_str(),
            PaneKind::Page(PageKind::Help).title(),
            PaneKind::Graph.title(),
            handle(&app, c).as_str()
        ]
    );
    let numbers: Vec<usize> = labels.iter().map(|l| l.number).collect();
    assert_eq!(numbers, [1, 2, 3, 4]);
}

/// **A call withdrawn while its tab is parked takes its pane with it** — the
/// close finds the pane in its own tab (`close_window`'s parked arm): left,
/// the stale pane would take the session's next call, said as "in tab N"
/// (r12-A LOW-1).
#[tokio::test]
async fn a_call_withdrawn_while_its_tab_is_parked_takes_its_pane() {
    let mut app = test_app();
    let a = app.current;
    let turn = app.fake_stream_on(a);
    app.session_of_mut(a).unwrap().conversation.begin_turn("1");
    let call = shannon_core::Block::bash_call("tu_a", "rm -rf /tmp/shannon-test-absent-tu_a");
    app.feed_turn(turn, StreamMsg::ToolUse(call));
    app.feed_turn(turn, StreamMsg::Complete);
    let pane = app.decision_pane_of(a).expect("premise: its pane");
    app.dispatch_command_for_test("tabnew");
    assert!(app.withdraw_decisions(turn), "premise: withdrawn");
    app.settle();
    assert!(
        app.window_of(pane).is_none(),
        "its pane gone, parked as it was"
    );
}

/// **`gt` with one tab says so** — vim is silent; here a key that does
/// nothing reads as broken, the `9gt` rule (r12-F L1).
#[test]
fn gt_with_one_tab_says_so() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    type_keys(&mut app, "gt");
    assert_eq!(
        app.notice().as_deref(),
        Some("this is the only tab — `:tabnew` opens another")
    );
    app.notices.clear();
    app.dispatch_command_for_test("tabprevious");
    assert!(app.notice().is_some_and(|n| n.contains("the only tab")));
}

/// **A narrow tab line keeps the tab you are in** — twelve tabs in 30 cells
/// drew tabs 1–10 and not the twelfth, the one you are in; now the labels
/// around it, `<` saying some are cut, and a click on the `<` goes nowhere.
/// And a name is shown whole or not at all below six cells: three tabs in
/// 12 cells drew `…` and id fragments no verb reads (r12-F L3).
#[test]
fn a_narrow_tab_line_keeps_the_tab_you_are_in() {
    use ratatui::layout::{Position, Rect};
    let mut app = test_app();
    for _ in 0..11 {
        app.dispatch_command_for_test("tabnew");
    }
    assert_eq!(tabs(&app), (12, 12), "premise: on the last of 12");
    let line = row(&frame_of(&mut app, 30, 10), 0);
    assert!(line.contains(" 12 "), "the tab you are in: {line:?}");
    assert!(line.starts_with('<'), "said to be cut: {line:?}");
    assert!(app.click_tab_line(Rect::new(0, 0, 30, 10), Position::new(0, 0)));
    assert_eq!(tabs(&app), (12, 12), "the `<` goes nowhere");

    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnew");
    let line = row(&frame_of(&mut app, 12, 10), 0);
    assert_eq!(
        line.trim_end(),
        " 1  2  3",
        "numbers, no cut names: {line:?}"
    );
    // 21 cells: three left for each name — an id's two characters and a
    // `…`, below the floor. In 12 there was no room at all, where nothing is
    // cut with the floor or without it.
    let line = row(&frame_of(&mut app, 21, 10), 0);
    assert_eq!(
        line.trim_end(),
        " 1  2  3",
        "no names cut to fragments: {line:?}"
    );
    let line = row(&frame_of(&mut app, 60, 10), 0);
    let handle = &app.tab_labels()[0].name;
    assert!(
        line.contains(handle.as_str()),
        "room, and the names: {line:?}"
    );
}

/// **vim's leading count is the tab argument** — `:2tabnext` is `:tabnext
/// 2`, `:3tabclose` closes tab 3, `:1tabonly` keeps tab 1; a count and an
/// argument both are refused, said. It was refused as "no count allowed"
/// (r12-F L7).
#[test]
fn a_leading_count_is_the_tab_argument() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("2tabnext");
    assert_eq!(tabs(&app), (3, 2), "`:2tabnext`");
    app.dispatch_command_for_test("3tabclose");
    assert_eq!(tabs(&app), (2, 2), "`:3tabclose`");
    app.dispatch_command_for_test("1tabnext 2");
    assert!(
        (app.notice()).is_some_and(|n| n.contains("a number before it or after it, not both")),
        "{:?}",
        app.notice()
    );
    app.dispatch_command_for_test("1tabonly");
    assert_eq!(tabs(&app), (1, 1), "`:1tabonly`");
}

/// **`:decide` goes to the tab that shows a hidden call's session** — A's
/// call hidden (`:hide`, no pane), A shown only in tab 1, `:decide` from
/// tab 2: tab 1, the call's pane opened under A and focused, and tab 2's
/// window still on B. It showed A in tab 2's window, B put in no window
/// (r12-F H2; `tabs.md` Q1 (a)).
#[tokio::test]
async fn decide_goes_to_the_tab_that_shows_a_hidden_calls_session() {
    let mut app = test_app();
    let a = app.current;
    ask_a_call(&mut app, a);
    let pane = app.decision_pane_of(a).expect("its pane");
    app.focus_pane(pane);
    app.dispatch_command_for_test("hide");
    assert!(app.decision_pane_of(a).is_none(), "premise: hidden");
    app.dispatch_command_for_test("tabnew");
    let b = shown_session(&app);
    app.dispatch_command_for_test(&format!("decide {}", a.number()));
    assert_eq!(tabs(&app), (2, 1), "to A's tab");
    let pane = app.decision_pane_of(a).expect("its pane, opened");
    assert!(app.workspace.window(pane).is_some(), "in this tab");
    assert_eq!(app.workspace.focused(), pane, "and in it");
    app.dispatch_command_for_test("tabnext 2");
    assert_eq!(shown_session(&app), b, "tab 2's window keeps B");
}

/// `<C-w>` then `c`, settled as the loop does.
fn ctrl_w(app: &mut App, c: char) {
    app.key_for_test(
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
        (80, 24),
    );
    type_keys(app, &c.to_string());
}

/// **`:tab help` opens the manual alone in a new tab after this one** —
/// vim's, measured (Neovim 0.12.6: from tab 1 of 3, tab 2 of 4, the help
/// its one window): here the manual over the tab's prompt, the tab left as
/// it was. The manual showed in the pane you stood in until `:tab` sent
/// the window `host_pane` would have used to a tab of its own.
#[test]
fn tab_help_opens_the_manual_alone_in_a_new_tab_after_this_one() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnext 1");
    let first = app.workspace.pane_ids();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("tab help");
    assert_eq!(tabs(&app), (4, 2), "after this one");
    assert!(
        app.workspace.has(PaneKind::Page(PageKind::Help)),
        "the manual here"
    );
    assert_eq!(app.workspace.pane_count(), 2, "with its prompt alone");
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(app.workspace.pane_ids(), first, "the tab left as it was");
    assert!(!app.workspace.has(PaneKind::Page(PageKind::Help)));
}

/// **`:tab b N`, `:tab sp` and `:tab graph` open what they show in a new
/// tab** — a session (where vim's `:tab b 2` opens no tab: here `:b` and
/// `:help` are one door), a copy of the window split, a tree with its
/// inspector beside it there (a companion beside its host, the one door
/// taken once); the window you stood in keeps its session.
#[test]
fn tab_b_sp_and_graph_open_what_they_show_in_a_new_tab() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, _, _, _) = app_with_two_sessions(dir.path());
    let here = shown_session(&app);
    let a_buffer = app.buffer_for_episode(a).unwrap();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test(&format!("tab b {}", a_buffer.number()));
    assert_eq!(tabs(&app), (2, 2), "`:tab b N`: a tab");
    assert_eq!(shown_session(&app), a_buffer, "on it");
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(
        shown_session(&app),
        here,
        "the window you stood in, as it was"
    );

    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("tab sp");
    assert_eq!(tabs(&app), (3, 2), "`:tab sp`: a tab after this one");
    assert_eq!(shown_session(&app), here, "a copy of the window split");
    assert_eq!(app.workspace.pane_count(), 2, "alone over its prompt");

    app.dispatch_command_for_test("tab graph");
    assert_eq!(tabs(&app), (4, 3), "`:tab graph`");
    assert!(app.workspace.has(PaneKind::Graph), "the tree here");
    assert!(
        app.workspace.has(PaneKind::Inspector),
        "its inspector beside it"
    );
}

/// **A command that opens no window opens no tab** (vim's), and `:tab`
/// leaves nothing armed: the next `:help` shows in the pane you stand in,
/// as ever. Bare `:tab` says what it takes.
#[test]
fn tab_with_a_command_that_opens_no_window_opens_no_tab() {
    let mut app = test_app();
    app.dispatch_command_for_test("tab set tui.prompt.height=12");
    assert_eq!(tabs(&app), (1, 1), "no window, no tab");
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("help");
    assert_eq!(tabs(&app), (1, 1), "the flag left nothing armed");
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Help),
        "the manual in this pane"
    );
    app.dispatch_command_for_test("tab");
    assert!(app.notice().is_some_and(|n| n.contains("`:tab {cmd}`")));
}

/// **`:tab fork` forks into a new tab, and says so** — the fork's window is
/// born by the one door, and its sentence says where: not "beside it".
#[tokio::test]
async fn tab_fork_forks_into_a_new_tab_and_says_so() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let source = shown_session(&app);
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("tab fork");
    assert_eq!(tabs(&app), (2, 2), "in a tab of its own");
    assert_ne!(shown_session(&app), source, "on the fork");
    assert!(
        app.notice().is_some_and(|n| n.contains("in a new tab")),
        "{:?}",
        app.notice()
    );
}

/// **`<C-w>T` moves this window to a new tab** after this one (vim's,
/// measured): the window C, its session shown there and focused, the
/// prompt aimed at it; the tab left keeps A. That tab's prompt, which went
/// to C, goes to A now — said of that tab, in one sentence.
#[test]
fn ctrl_w_shift_t_moves_this_window_to_a_new_tab() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let a = shown_session(&app);
    app.split_command("", SplitDir::Vertical);
    app.dispatch_command_for_test("new");
    let c = app.pane_session(app.workspace.focused()).id;
    assert_eq!(
        app.prompt_target(),
        c,
        "premise: standing in C, aimed at it"
    );
    ctrl_w(&mut app, 'T');
    app.settle();
    assert_eq!(tabs(&app), (2, 2), "a tab after this one");
    assert_eq!(shown_session(&app), c, "holding C's window");
    assert_eq!(app.focus(), PaneKind::Output, "in it");
    assert_eq!(app.prompt_target(), c);
    assert_eq!(
        app.notice(),
        Some(format!(
            "tab 1's prompt now goes to {}",
            app.session_tag(app.session_of(a).unwrap().conversation.episode().id)
        ))
    );
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(
        app.workspace.panes_of(PaneKind::Output).count(),
        1,
        "A alone"
    );
    assert_eq!((shown_session(&app), app.prompt_target()), (a, a));
}

/// **`<C-w>T` is refused, said, where it has nothing to move** — the prompt
/// (every tab has its own), a decision pane (it opens beside its session),
/// a tab's only window (vim says "Already only one window", measured;
/// here the sentence points at `:tab sp`).
#[tokio::test]
async fn ctrl_w_shift_t_is_refused_where_nothing_moves() {
    let mut app = test_app();
    let a = app.current;
    app.set_focus(PaneKind::Output);
    ctrl_w(&mut app, 'T');
    assert_eq!(tabs(&app), (1, 1));
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("the tab's only window"))
    );
    app.set_focus(PaneKind::Prompt);
    ctrl_w(&mut app, 'T');
    assert!(app.notice().is_some_and(|n| n.contains("the prompt stays")));
    ask_a_call(&mut app, a);
    let pane = app.decision_pane_of(a).expect("its pane");
    app.focus_pane(pane);
    app.settle();
    ctrl_w(&mut app, 'T');
    assert_eq!(tabs(&app), (1, 1));
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("a decision pane opens beside its session"))
    );
    assert!(app.shown_call_in(a).is_some(), "and the call stands");
}

/// **`<C-w>T` in a companion moves its host, the companion made again
/// beside it** — the inspector is the tree's window (F1): the tree goes to
/// the new tab with an inspector there, and none is left behind.
#[test]
fn ctrl_w_shift_t_in_a_companion_moves_its_host() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.open_graph_command();
    app.settle();
    app.set_focus(PaneKind::Inspector);
    assert_eq!(
        app.focus(),
        PaneKind::Inspector,
        "premise: in the inspector"
    );
    ctrl_w(&mut app, 'T');
    app.settle();
    assert_eq!(tabs(&app), (2, 2));
    assert!(app.workspace.has(PaneKind::Graph) && app.workspace.has(PaneKind::Inspector));
    app.dispatch_command_for_test("tabnext 1");
    assert!(
        !app.workspace.has(PaneKind::Graph) && !app.workspace.has(PaneKind::Inspector),
        "neither left behind"
    );
}

/// **A decision follows its window to the new tab** — A's window moved
/// with `<C-w>T`, its call's pane opens again under it there, armed (new
/// under your cursor), the call unanswered: a window moving is claimed, no
/// decision about what it shows. Its old pane, its host gone, closed by
/// placement (`settle_decisions`), not by you.
#[tokio::test]
async fn a_decision_follows_its_window_to_the_new_tab() {
    let mut app = test_app();
    let a = app.current;
    app.set_focus(PaneKind::Output);
    let a_pane = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    app.dispatch_command_for_test("new");
    ask_a_call(&mut app, a);
    let old = app.decision_pane_of(a).expect("its pane");
    app.read_the_call_for_test();
    app.focus_pane(a_pane);
    app.settle();
    ctrl_w(&mut app, 'T');
    app.settle();
    assert_eq!(
        (tabs(&app), shown_session(&app)),
        ((2, 2), a),
        "premise: A moved"
    );
    assert!(app.shown_call_in(a).is_some(), "the call unanswered");
    let pane = app.decision_pane_of(a).expect("its pane, again");
    assert_ne!(pane, old, "a new pane");
    assert!(app.workspace.window(pane).is_some(), "here, under A");
    assert_eq!(app.answer_keys(pane), AnswerKeys::Waiting, "armed");
    assert!(
        !(app.notices.iter()).any(|(_, t)| t.contains("DENIED")),
        "nothing denied: {:?}",
        app.notices
    );
}

// ---- the r13 reviews (`docs/Reviews/2026-10-26-t3t4/{A,F}.md`) ----------------

/// **`:tab sp` from the prompt copies the aim's window, and never panics**
/// — a tab is never made around a prompt (every tab has its own): standing
/// in the prompt, the window split is the one it is aimed at
/// (`existing_host`'s). It built a tab around a second prompt and panicked
/// (r13-F C1, r13-A 1). `:tab sp prompt` is refused, said; so is `:tab sp`
/// from a prompt whose aim no window here shows.
#[test]
fn tab_sp_from_the_prompt_copies_the_aims_window() {
    let mut app = test_app();
    let a = shown_session(&app);
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test("tab sp");
    assert_eq!(tabs(&app), (2, 2), "a tab");
    assert_eq!(shown_session(&app), a, "on the aim's window, copied");
    assert_eq!(
        app.workspace.panes_of(PaneKind::Prompt).count(),
        1,
        "one prompt"
    );

    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("tab sp prompt");
    assert_eq!(tabs(&app), (2, 2), "no tab around a prompt");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("a tab is not made around a prompt"))
    );

    app.dispatch_command_for_test("close");
    assert!(
        !app.workspace.has(PaneKind::Output),
        "premise: no window on the aim here"
    );
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test("tab sp");
    assert_eq!(tabs(&app), (2, 2), "nothing to copy: no tab");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("a tab is not made around a prompt"))
    );
}

/// **`:tab sp` from a companion splits its host into the new tab** — the
/// tree, with an inspector made beside it there, named as the tree's window
/// is; it made a tab of an inspector with no tree, named "" (r13-F M1,
/// r13-A 3).
#[test]
fn tab_sp_from_a_companion_takes_its_host() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.settle();
    app.set_focus(PaneKind::Inspector);
    app.dispatch_command_for_test("tab sp");
    assert_eq!(tabs(&app), (2, 2));
    assert!(app.workspace.has(PaneKind::Graph) && app.workspace.has(PaneKind::Inspector));
    assert_eq!(app.tab_labels()[1].name, PaneKind::Graph.title());
}

/// **`:tab new` is `:tabnew`'s fresh session** — it reused the blank one
/// this pane showed, opening a second tab on it and saying "already on a
/// fresh session" (r13-A 2).
#[test]
fn tab_new_opens_a_fresh_session_as_tabnew_does() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let first = shown_session(&app);
    app.dispatch_command_for_test("tab new");
    assert_eq!(tabs(&app), (2, 2));
    assert_ne!(shown_session(&app), first, "a fresh session");
    assert!(
        !(app.notices.iter()).any(|(_, t)| t.contains("already on a fresh session")),
        "{:?}",
        app.notices
    );
}

/// **`:tab vnew` and `:tab snew` open a tab as `:tab new` does** — `:tab
/// sp`'s tab around a copy of the window, the fresh session in it: one new
/// tab, on a fresh session, holding one transcript, and this tab as it was
/// (11-10).
#[test]
fn tab_vnew_and_snew_open_a_tab_as_tab_new_does() {
    for cmd in ["tab vnew", "tab snew"] {
        let mut app = test_app();
        app.set_focus(PaneKind::Output);
        let first = shown_session(&app);
        let here = app.workspace.slots().len();
        app.dispatch_command_for_test(cmd);
        assert_eq!(tabs(&app), (2, 2), "`:{cmd}`: {:?}", app.notice());
        assert_ne!(shown_session(&app), first, "`:{cmd}`: a fresh session");
        let transcripts = app.workspace.panes_of(PaneKind::Output).count();
        assert_eq!(
            transcripts, 1,
            "`:{cmd}`: one window, no split in the new tab"
        );
        app.dispatch_command_for_test("tabprevious");
        assert_eq!(
            app.workspace.slots().len(),
            here,
            "`:{cmd}`: this tab untouched"
        );
        assert_eq!(shown_session(&app), first, "`:{cmd}`: on its session still");
    }
}

/// **`:tab b N` of a page opens it in a new tab, as of a session** — it
/// did nothing for a page this pane showed and showed one held in this
/// window's history here (r13-A 6).
#[test]
fn tab_b_of_a_page_opens_a_tab_as_of_a_session() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("help");
    let manual = (app.buffers.pages())
        .find(|p| p.kind() == PaneKind::Page(PageKind::Help))
        .map(|p| p.id())
        .expect("premise: the manual");
    app.dispatch_command_for_test(&format!("tab b {}", manual.number()));
    assert_eq!(tabs(&app), (2, 2), "shown here already: a tab");
    assert!(app.workspace.has(PaneKind::Page(PageKind::Help)));
    app.dispatch_command_for_test("tabnext 1");
    let session = app.current;
    app.dispatch_command_for_test(&format!("b {}", session.number()));
    app.dispatch_command_for_test(&format!("tab b {}", manual.number()));
    assert_eq!(tabs(&app), (3, 2), "held in this window's history: a tab");
    assert!(app.workspace.has(PaneKind::Page(PageKind::Help)));
}

/// **What `:tab {cmd}` leaves to the terminal stays under `:tab`** — a
/// pager's effect, a `:squash`'s `$EDITOR`, wrapped (`CmdEffect::InTab`), so
/// the window it opens after is born in a tab; run after the flag was
/// cleared, it opened here (r13-A 7). A command done at once is not
/// wrapped.
#[test]
fn what_tab_leaves_to_the_terminal_stays_under_tab() {
    let mut app = test_app();
    assert!(matches!(app.tab_command("keymap"), CmdEffect::InTab(_)));
    assert!(matches!(app.tab_command("help"), CmdEffect::Done(_)));
    assert!(!app.into_a_new_tab, "nothing left armed");
}

/// **`{N}<C-w>T` puts the new tab before tab N, and a marked window stays
/// marked** — vim's count (measured: before tab N); it was dropped (r13-F
/// L2, r13-A 8). The tab left, now tab 2, is named so in the sentence
/// about its prompt; and the window marked for `:Send` keeps its mark in
/// the slot it is born in.
#[test]
fn a_counted_ctrl_w_shift_t_goes_before_tab_n_and_keeps_its_mark() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let a = shown_session(&app);
    app.split_command("", SplitDir::Vertical);
    app.dispatch_command_for_test("new");
    let c_pane = app.workspace.focused();
    app.workspace.set_mark(c_pane, true);
    type_keys(&mut app, "1");
    ctrl_w(&mut app, 'T');
    app.settle();
    assert_eq!(tabs(&app), (2, 1), "before tab 1: the front");
    let moved = app.workspace.focused();
    assert!(app.workspace.is_marked(moved), "still marked");
    let a_tag = app.session_tag(app.session_of(a).unwrap().conversation.episode().id);
    assert_eq!(
        app.notice(),
        Some(format!("tab 2's prompt now goes to {a_tag}")),
        "the tab left, numbered as it is now"
    );
}

/// **`:tabclose` lands on its neighbour as `gt` does** — the aim it came
/// back with kept (not taken by focus) and a decision there armed again:
/// `close_tab`'s `tab_shown`, which only `gt` and `:tabnext` reached in a
/// test (r13-F H1).
#[tokio::test]
async fn tabclose_lands_on_its_neighbour_as_gt_does() {
    let mut app = test_app();
    let a = app.current;
    app.set_focus(PaneKind::Output);
    let a_pane = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    app.dispatch_command_for_test("new");
    let c = app.pane_session(app.workspace.focused()).id;
    app.focus_pane(a_pane);
    app.settle();
    app.reveal(c);
    app.settle();
    assert_eq!(app.prompt_target(), c, "premise: in A's window, aimed at C");
    ask_a_call(&mut app, a);
    let pane = app.decision_pane_of(a).expect("A's call");
    app.focus_pane(a_pane);
    app.settle();
    app.read_the_call_for_test();
    app.dispatch_command_for_test("tabnew");
    app.read_the_call_for_test();
    app.dispatch_command_for_test("tabclose");
    assert_eq!(tabs(&app), (1, 1));
    assert_eq!(app.prompt_target(), c, "the aim it came back with");
    assert_eq!(
        app.answer_keys(pane),
        AnswerKeys::Waiting,
        "its call armed again"
    );
}

/// **`<C-w>T` is refused on a tab's only window beside a decision or its
/// companion** — [A, A's decision, prompt] and [tree, inspector, prompt]:
/// neither the decision pane nor the companion is a window to leave behind
/// (r13-F H2: both filters were held by nothing).
#[tokio::test]
async fn ctrl_w_shift_t_counts_no_decision_or_companion_as_another_window() {
    let mut app = test_app();
    let a = app.current;
    ask_a_call(&mut app, a);
    app.set_focus(PaneKind::Output);
    ctrl_w(&mut app, 'T');
    assert_eq!(tabs(&app), (1, 1), "beside its decision: refused");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("the tab's only window"))
    );

    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.settle();
    assert!(
        app.workspace.has(PaneKind::Inspector),
        "premise: tree and inspector"
    );
    app.set_focus(PaneKind::Graph);
    ctrl_w(&mut app, 'T');
    assert_eq!(tabs(&app), (1, 1), "beside its companion: refused");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("the tab's only window"))
    );
}

/// **`9gT` from tab 2 of 4 goes round to tab 1, and `:tabprevious 0` is
/// refused** — vim's (measured); the inner `% count` keeps `at + count - 9`
/// from underflowing, and `n > 0` keeps 0 from being a silent no-op (r13-F
/// H3: both held by nothing).
#[test]
fn nine_g_shift_t_goes_round_and_tabprevious_zero_is_refused() {
    let mut app = test_app();
    for _ in 0..3 {
        app.dispatch_command_for_test("tabnew");
    }
    app.dispatch_command_for_test("tabnext 2");
    app.set_focus(PaneKind::Output);
    type_keys(&mut app, "9gT");
    assert_eq!(tabs(&app), (4, 1), "nine back from 2 of 4");
    app.dispatch_command_for_test("tabprevious 0");
    assert_eq!(tabs(&app), (4, 1));
    assert!(app.notice().is_some_and(|n| n.contains("not a count")));
}

/// **A chord armed in a tab does not outlive a click away and back** — `m`
/// in a tree, a click to tab 2 on the tab line and back: still armed, the
/// next `f` would mark a row; a click moves no focus, so `tab_shown`'s
/// `cancel_chords` is the one door that ends it (r13-A 4, r13-F L8: held by
/// nothing; the probe is r13-A's).
#[test]
fn a_chord_does_not_survive_a_click_away_and_back() {
    use ratatui::layout::{Position, Rect};
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnext 1");
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.settle();
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
    assert!(
        app.graph_view().is_some_and(|g| g.chord_armed()),
        "premise: `m` armed"
    );
    let frame = Rect::new(0, 0, 80, 24);
    let x_of = |app: &App, n: usize| {
        (0..80u16)
            .find(|&x| crate::view::tab_at(&app.tab_labels(), 80, x) == Some(n))
            .expect("a label")
    };
    let x2 = x_of(&app, 2);
    assert!(app.click_tab_line(frame, Position::new(x2, 0)));
    app.settle();
    let x1 = x_of(&app, 1);
    assert!(app.click_tab_line(frame, Position::new(x1, 0)));
    app.settle();
    assert_eq!(tabs(&app), (2, 1), "premise: back");
    assert!(
        !app.graph_view().is_some_and(|g| g.chord_armed()),
        "the chord ended"
    );
}

/// **Back in a tab left standing in its decision pane, you are in it, its
/// keys waiting** — vim's: the pane you left (Steven, 10-26: "back to the
/// last pane we were in … let's not change the state when moving away").
/// It fell to the prompt (`settle_focus`'s "entered only on purpose" read
/// the return as a fall); armed again on return, a key in the burst after
/// `gt` answers nothing, and the call stands.
#[tokio::test]
async fn back_in_a_tab_left_in_its_decision_pane_you_are_in_it() {
    let mut app = test_app();
    let a = app.current;
    ask_a_call(&mut app, a);
    let pane = app.decision_pane_of(a).expect("its pane");
    app.focus_pane(pane);
    app.settle();
    app.read_the_call_for_test();
    assert_eq!(app.focus(), PaneKind::Tool, "premise: in it");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(app.workspace.focused(), pane, "the pane you left");
    assert_eq!(app.answer_keys(pane), AnswerKeys::Waiting, "armed again");
    type_keys(&mut app, "a");
    assert!(
        app.shown_call_in(a).is_some(),
        "a key in the burst answers nothing"
    );
    // Nor a close in it, which denies once its keys are live: it waits as
    // `q` does (review stop 11, A 2). Each refusal re-arms, as a key's does.
    let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
    for via in [":q", ":close", "<C-w>c", "<C-w>q"] {
        match via.strip_prefix(':') {
            Some(command) => {
                app.dispatch_command_for_test(command);
            }
            None => {
                app.key_for_test(ctrl_w, (80, 30));
                let c = via.chars().last().unwrap();
                app.key_for_test(
                    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                    (80, 30),
                );
            }
        }
        assert!(app.shown_call_in(a).is_some(), "{via}: not answered");
        assert_eq!(app.decision_pane_of(a), Some(pane), "{via}: not closed");
        assert!(
            app.notice()
                .is_some_and(|said| said.starts_with("not answered")),
            "{via}: said: {:?}",
            app.notice()
        );
    }
}

/// **A decision in a tab a narrow line cuts is still announced on it** —
/// the `<` that cuts it drawn in `ui.caution`; it was plain, and the hotlist
/// went with the label (r13-F M2).
#[tokio::test]
async fn a_narrow_line_announces_a_cut_tabs_decision() {
    let mut app = test_app();
    let a = app.current;
    ask_a_call(&mut app, a);
    for _ in 0..11 {
        app.dispatch_command_for_test("tabnew");
    }
    app.settle();
    assert_eq!(app.decision_tab(a), Some(1), "premise: tab 1 decides");
    let buf = frame_of(&mut app, 30, 10);
    assert_eq!(buf[(0, 0)].symbol(), "<", "premise: tab 1 cut");
    let caution = app.theme().ui(Ink::Caution);
    assert_eq!(
        buf[(0, 0)].fg,
        caution.fg.unwrap(),
        "the `<` in the hue of an ask"
    );
    let line = row(&buf, 0);
    let gt = line.rfind('>');
    assert!(gt.is_none(), "nothing cut on the right: {line:?}");
}

/// **The tab you are in is drawn as yours though its decision waits** — on
/// screen there, its caution is the pane's to show; the tab line says where
/// you are (r13-F L7: the order of the two looks was held by nothing).
#[tokio::test]
async fn the_tab_you_are_in_is_drawn_as_yours_though_it_decides() {
    let mut app = test_app();
    let a = app.current;
    app.dispatch_command_for_test(&format!("tabnew {}", a.number()));
    ask_a_call(&mut app, a);
    assert!(app.tab_labels()[1].decides, "premise: this tab decides");
    let buf = frame_of(&mut app, 80, 24);
    let line = row(&buf, 0);
    let x = line.find(" 2 ").expect("its label") as u16 + 1;
    let yours = app.theme().ui(Ink::TabLineSelected);
    assert_eq!(
        (buf[(x, 0)].fg, buf[(x, 0)].bg),
        (yours.fg.unwrap(), yours.bg.unwrap())
    );
}

/// **`:tabmove` with one tab, `:[N]tabnew`, and another tab closed — each
/// said or placed as vim's** (measured): one tab, said (no line to see it
/// on; vim silent — the `gt` rule, r13-F L3); `:0tabnew` at the front,
/// `:1tabnew` after tab 1 (L4: refused as "no count allowed"); `:tabclose 3`
/// from tab 1 says what closed, as `:tabonly` does (L9).
#[test]
fn tabmove_alone_counted_tabnew_and_another_tab_closed_are_said() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabmove");
    assert_eq!(app.notice().as_deref(), Some("this is the only tab"));
    app.dispatch_command_for_test("tabrename first");
    app.dispatch_command_for_test("0tabnew");
    assert_eq!(tabs(&app), (2, 1), "`:0tabnew`: at the front");
    assert_eq!(app.tab_labels()[1].name, "first");
    app.dispatch_command_for_test("1tabnew");
    assert_eq!(tabs(&app), (3, 2), "`:1tabnew`: after tab 1");
    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test("tabclose 3");
    assert_eq!(tabs(&app), (2, 1));
    assert_eq!(
        app.notice().as_deref(),
        Some("closed tab 3 — its sessions stay open (`:ls`)")
    );
}

/// **The sentences that name a tab name the right one** — `:tabclose` over
/// this tab's review says no tab (`tab_holds`' arm for this one); a
/// review arriving for a session only another tab shows names that tab and
/// its key (`review_arrives`); the last transcript closed over a session a
/// view here still holds says so (`Shown::Here`). A wrong number in any
/// passed (r13-F L6).
#[tokio::test]
async fn the_sentences_that_name_a_tab_name_the_right_one() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let chat = super::review::reviewed(&mut app);
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnext 1");
    assert!(
        app.decision_pane_of(chat).is_some(),
        "premise: its review here"
    );
    app.dispatch_command_for_test("tabclose");
    assert_eq!(
        app.notice().unwrap_or_default(),
        format!(
            "`:tabclose`: not closed — `:tabclose!` closes anyway (it waits, hidden); b{}'s \
             summary review waits in its pane",
            chat.number()
        )
    );

    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);
    app.dispatch_command_for_test("tabnew");
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    assert!(
        (app.notices.iter()).any(|(_, t)| t.contains("in tab 1 — `1gt`")),
        "{:?}",
        app.notices
    );

    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let a = shown_session(&app);
    app.split_command("", SplitDir::Vertical);
    app.dispatch_command_for_test("help");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnext 1");
    let transcript = (app.workspace.panes_of(PaneKind::Output).next()).expect("A's other window");
    app.focus_pane(transcript);
    app.dispatch_command_for_test("q");
    let n = a.number();
    assert_eq!(
        app.notice(),
        Some(format!(
            "the last transcript closed — b{n} is beneath a view here (`:b{n}` shows it)"
        ))
    );
}

/// **The tab arguments never panic and always land in range** — every
/// sign, `$`, empty, overflowing and non-ASCII digit, at every place in up
/// to five tabs (r13-A's probe, kept).
#[test]
fn the_tab_arguments_never_panic_and_land_in_range() {
    use crate::app::tabs::{move_to, tab_named};
    let args = [
        "",
        "$",
        "+",
        "-",
        "+0",
        "-0",
        "0",
        "1",
        "2",
        "3",
        "9",
        "00",
        "01",
        "+1",
        "-1",
        "+2",
        "-2",
        "+99",
        "-99",
        "++",
        "--",
        "+-1",
        "-+1",
        "1+",
        "$1",
        "$$",
        " 1",
        "1 ",
        "١",
        "²",
        "18446744073709551615",
        "18446744073709551616",
        "+18446744073709551615",
        "-18446744073709551615",
        "99999999999999999999999",
        "a",
        "+a",
        "é",
        "+é",
    ];
    for count in 1..6usize {
        for at in 0..count {
            for arg in args {
                if let Ok(to) = tab_named(arg, at, count) {
                    assert!(to < count, "tab_named({arg:?}, {at}, {count}) = {to}");
                }
                if let Ok(to) = move_to(arg, at, count) {
                    assert!(to < count, "move_to({arg:?}, {at}, {count}) = {to}");
                }
            }
        }
    }
}

/// **A random walk over the tab commands keeps the tabs whole** — 600
/// seeded steps of opening, closing, moving, renaming, `:tab {cmd}`,
/// splits and `<C-w>T`: the shown tab in range, a label per tab, a prompt
/// in every tab, nothing left dropped, and the line drawn (r13-A's probe,
/// kept).
#[test]
fn a_random_walk_over_the_tab_commands_keeps_the_tabs_whole() {
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    let mut app = test_app();
    for step in 0..600 {
        let cmd = match next(15) {
            0 | 1 => "tabnew".to_string(),
            2 => format!("tabclose {}", next(6) + 1),
            3 => format!("tabmove {}", next(6)),
            4 => format!(
                "tabmove {}{}",
                if next(2) == 0 { "+" } else { "-" },
                next(3)
            ),
            5 => format!("tabonly {}", next(6) + 1),
            6 => format!("tabnext {}", next(6) + 1),
            7 => "tabprevious".to_string(),
            8 => "tab help".to_string(),
            9 => "vsp".to_string(),
            10 => "q".to_string(),
            11 => format!("tabrename t{step}"),
            12 => "tab graph".to_string(),
            13 => "tab sp".to_string(),
            _ => "close".to_string(),
        };
        if next(4) == 0 {
            app.set_focus(PaneKind::Output);
            ctrl_w(&mut app, 'T');
        }
        if app.dispatch_command_for_test(&cmd) {
            break;
        }
        app.settle();
        let (count, at) = (app.tabs.count(), app.tabs.current());
        assert!(
            count >= 1 && at < count,
            "step {step} `{cmd}`: {count} {at}"
        );
        assert_eq!(app.tab_labels().len(), count, "step {step} `{cmd}`");
        assert_eq!(app.tabs.parked().count(), count - 1, "step {step} `{cmd}`");
        assert!(
            app.workspace.has(PaneKind::Prompt),
            "step {step} `{cmd}`: a prompt"
        );
        for (_, tab) in app.tabs.parked() {
            assert!(
                tab.workspace.has(PaneKind::Prompt),
                "step {step} `{cmd}`: parked"
            );
        }
        assert!(
            app.tabs.take_dropped().is_empty(),
            "step {step} `{cmd}`: drained"
        );
        frame_of(&mut app, 40, 12);
    }
}

/// **The door makes no tab around a prompt, a decision pane or a
/// companion** — `new_window` under `:tab`, handed one directly: no tab,
/// no panic. The split door refuses first, saying why; this is the floor
/// under it, so `open_tab_holding` is never handed one (r13-F C1).
#[test]
fn the_door_makes_no_tab_around_a_prompt() {
    let mut app = test_app();
    app.into_a_new_tab = true;
    let window = Window::prompt(&app.registers, app.current);
    let born = app.new_window(window, Born::AcrossTop);
    assert_eq!(born, None, "no window born");
    assert_eq!(tabs(&app), (1, 1), "no tab");
    assert!(!app.into_a_new_tab, "the flag taken");
}

// ---- Steven's tab test pass (10-26): ranges, `:ball`, `:tab` said ---------

/// **vim's ranges before the tab commands** (measured, Neovim 0.12.6, from
/// tab 2 of 4): `:+tabmove` one right, `:-tabmove` one left, `:$tabmove`
/// the end; `:+tabnext` the next, `:$tabnext` the last, `:.tabclose` this
/// one; `:+tabonly` keeps the next; `:-tabnew` opens just before this one,
/// `:$tabnew` at the end; `:+tabprevious` refused (E16), `:+b` refused.
/// They read as "not a command" (Steven, 10-26: `:+tabm`).
#[test]
fn vims_ranges_before_the_tab_commands() {
    let four = || {
        let mut app = test_app();
        for n in 1..=4 {
            if n > 1 {
                app.dispatch_command_for_test("tabnew");
            }
            app.dispatch_command_for_test(&format!("tabrename t{n}"));
        }
        app.dispatch_command_for_test("tabnext 2");
        app
    };
    let names = |app: &App| -> Vec<String> { tab_names(app) };
    let run = |cmd: &str| {
        let mut app = four();
        app.dispatch_command_for_test(cmd);
        app
    };
    let app = run("+tabmove");
    assert_eq!(
        (names(&app), tabs(&app)),
        (
            vec!["t1".into(), "t3".into(), "t2".into(), "t4".into()],
            (4, 3)
        )
    );
    let app = run("-tabmove");
    assert_eq!(names(&app)[0], "t2");
    let app = run("$tabmove");
    assert_eq!(tabs(&app), (4, 4));
    let app = run("+tabnext");
    assert_eq!(tabs(&app), (4, 3));
    let app = run("$tabnext");
    assert_eq!(tabs(&app), (4, 4));
    let app = run(".tabclose");
    assert_eq!((tabs(&app), names(&app)[1].as_str()), ((3, 2), "t3"));
    let app = run("+tabonly");
    assert_eq!(names(&app), ["t3"]);
    let app = run("-tabnew");
    assert_eq!(
        (tabs(&app), names(&app)[0].as_str(), names(&app)[2].as_str()),
        ((5, 2), "t1", "t2")
    );
    let app = run("$tabnew");
    assert_eq!(tabs(&app), (5, 5));
    let mut app = run("+tabprevious");
    assert_eq!(tabs(&app), (4, 2), "`:tabprevious` takes a count only");
    assert!(app.notice().is_some_and(|n| n.contains("not a count")));
    app.dispatch_command_for_test("+b");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("a count before it is a number"))
    );
}

/// **A `:tabmove` that moves nothing says so** — vim counts the tabs before
/// the move, so from tab 2 `:tabmove 1` ("after tab 1") and `:tabmove 2`
/// leave it there, silently; read as broken (Steven, 10-26: "`:tabm1`
/// doesn't appear to work").
#[test]
fn a_tabmove_that_moves_nothing_says_so() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabm1");
    assert_eq!(tabs(&app), (2, 2), "no move: vim's");
    assert!(
        app.notice()
            .is_some_and(|n| n.starts_with("tab 2 is there already")),
        "{:?}",
        app.notice()
    );
    app.dispatch_command_for_test("tabm0");
    assert_eq!(tabs(&app), (2, 1), "`:tabm0`: first");
}

/// **A session in no window, written in** — as one `:ls` lists is: a blank
/// one in no window is not called yet (`App::called`), and `:ball` passes it
/// by, as `:ls` does.
fn a_session_in_no_window(app: &mut App) -> BufferId {
    let made = app.fresh_session();
    let conversation = &mut app.session_of_mut(made).expect("just made").conversation;
    conversation.begin_turn("a question");
    conversation.push_chunk("a reply");
    conversation.commit_turn();
    made
}

/// **`:ls`, `:ball` and `:Send ball` are about one set of sessions, a blank
/// one in no window among them** — no session is hidden (Steven, 11-11:
/// "That is a secret buffer"), as vim lists a hidden buffer. `:ball` and
/// `:Send ball` read `listed_sessions`, `:ls` its rows; the old `called` rule
/// put back in `listed_sessions` alone passed the whole suite (review of
/// 850a72d, M3), the tutor's test having no session to disagree about.
#[test]
fn ls_and_ball_agree_on_every_session_a_blank_one_too() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("vnew");
    let blank = app.prompt_target();
    app.dispatch_command_for_test("close");
    assert!(
        app.panes_with_session(blank, Over::EveryTab)
            .next()
            .is_none(),
        "premise: blank, in no window"
    );
    let listed: Vec<u64> = (app.listed_sessions().iter()).map(|b| b.number()).collect();
    let rows: Vec<u64> = (app.buffers_listing(false).lines())
        .filter_map(|l| {
            let digits = l.trim_start().strip_prefix('b')?;
            digits
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .collect();
    assert_eq!(rows, listed, "`:ls` and `:ball` disagree");
    assert!(
        listed.contains(&blank.number()),
        "the blank one in both: {listed:?}"
    );
}

/// **`:ball` opens a window for each session this tab does not show** —
/// vim's "one window for each buffer" (measured): split below in `:ls`
/// order, focus kept, nothing closed; again, said; `:ball 2` stops at two
/// sessions, the one you are in counted first.
#[test]
fn ball_opens_a_window_for_each_session_not_shown_here() {
    let mut app = test_app();
    let a = shown_session(&app);
    let b = a_session_in_no_window(&mut app);
    let c = a_session_in_no_window(&mut app);
    app.set_focus(PaneKind::Output);
    let focused = app.workspace.focused();
    app.dispatch_command_for_test("ball");
    let shown: Vec<BufferId> = (app.workspace.ring_order().into_iter())
        .filter(|&p| app.kind_of(p) == Some(PaneKind::Output))
        .map(|p| app.pane_session(p).id)
        .collect();
    assert_eq!(shown, [a, b, c], "one each, in `:ls` order below");
    assert_eq!(app.workspace.focused(), focused, "focus kept");
    assert!(app.notice().is_some_and(|n| n.contains("2 windows")));
    app.dispatch_command_for_test("ball");
    assert_eq!(
        app.notice().as_deref(),
        Some("`:ball`: every session has a window here already")
    );

    // Standing in the last, `:ball 2` is it and the first: vim's count
    // takes the one you are in first (measured: from b, `:ball 2` is b, a).
    let mut app = test_app();
    let a = shown_session(&app);
    let _ = a_session_in_no_window(&mut app);
    let c = a_session_in_no_window(&mut app);
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test(&format!("b {}", c.number()));
    app.dispatch_command_for_test("ball 2");
    let mut shown: Vec<BufferId> = (app.workspace.panes_of(PaneKind::Output))
        .map(|p| app.pane_session(p).id)
        .collect();
    shown.sort();
    let mut want = vec![a, c];
    want.sort();
    assert_eq!(shown, want, "the one you are in, and the first");

    // vim's count before it is the same: `:2ball`.
    let mut app = test_app();
    let _ = a_session_in_no_window(&mut app);
    let _ = a_session_in_no_window(&mut app);
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("2ball");
    assert_eq!(
        app.workspace.panes_of(PaneKind::Output).count(),
        2,
        "`:2ball`"
    );
}

/// **`:tab ball` opens a tab for each session no tab shows** — vim's
/// (measured: after this one, in list order, ending in the last); a session
/// another tab shows already gets none; again, said.
#[test]
fn tab_ball_opens_a_tab_for_each_session_no_tab_shows() {
    let mut app = test_app();
    let a = shown_session(&app);
    let b = a_session_in_no_window(&mut app);
    let c = a_session_in_no_window(&mut app);
    app.dispatch_command_for_test(&format!("tabnew {}", b.number()));
    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test("tab ball");
    assert_eq!(tabs(&app), (3, 2), "one more tab, after this one, shown");
    assert_eq!(shown_session(&app), c);
    app.dispatch_command_for_test("tabnext 1");
    assert_eq!(shown_session(&app), a, "this tab as it was");
    app.dispatch_command_for_test("tab ball");
    assert_eq!(
        app.notice().as_deref(),
        Some("`:tab ball`: every session is in a tab already")
    );
    assert!(!app.into_a_new_tab, "nothing left armed");
}

/// **`:tab` with no such command says so, as `:tab`'s** — the inner
/// refusal, "not a command: `:ball`", named no `:tab` (Steven, 10-26).
#[test]
fn tab_with_no_such_command_says_so_as_tabs() {
    let mut app = test_app();
    app.dispatch_command_for_test("tab frob");
    assert_eq!(tabs(&app), (1, 1));
    assert_eq!(
        app.notice().as_deref(),
        Some("`:tab frob`: `:frob` is not a command — `:tab` runs one, as `:tab help`")
    );
}

// ---- `:tabs` (T4's listing) ------------------------------------------------

/// `:tabs`' lines, as text.
fn tabs_text(app: &App) -> Vec<String> {
    (app.tabs_lines().lines().iter())
        .map(|line| line.text())
        .collect()
}

/// **`:tabs` lists each tab and its windows, vim's way** — `Tab page N`
/// and the name you gave it, then each window numbered as its corner is,
/// `>` the one you are in (the tab you are in only, as vim marks it), `+` a
/// prompt holding a draft, a decision pane by whose.
#[tokio::test]
async fn tabs_lists_each_tab_and_its_windows() {
    let mut app = test_app();
    let a = app.current;
    app.set_focus(PaneKind::Prompt);
    app.prompt_mut().insert_pasted("half written");
    app.dispatch_command_for_test("tabrename work");
    ask_a_call(&mut app, a);
    app.dispatch_command_for_test("tabnew");
    let b = shown_session(&app);
    app.set_focus(PaneKind::Output);
    let text = tabs_text(&app);
    let handle = |s: BufferId| app.session_of(s).unwrap().conversation.episode().handle();
    let (a_said, b_said) = (
        format!("b{} {}", a.number(), handle(a)),
        format!("b{} {}", b.number(), handle(b)),
    );
    assert_eq!(text[0], "Tab page 1  \"work\"", "{text:#?}");
    assert!(
        text.iter()
            .any(|l| l.starts_with("  ") && l.ends_with(&a_said)),
        "tab 1's transcript, unmarked (not the tab you are in): {text:#?}"
    );
    assert!(
        text.iter()
            .any(|l| l.starts_with(" +") && l.ends_with(&format!("prompt → {a_said}"))),
        "tab 1's prompt, its draft marked: {text:#?}"
    );
    assert!(
        text.iter()
            .any(|l| l.ends_with(&format!("decision for {a_said}"))),
        "the decision pane, whose: {text:#?}"
    );
    let at = text
        .iter()
        .position(|l| l == "Tab page 2")
        .expect("tab 2's heading");
    assert!(
        text[at + 1..]
            .iter()
            .any(|l| l.starts_with(">  ") && l.ends_with(&b_said)),
        "the window you are in, marked: {text:#?}"
    );
}

/// **Each window is numbered as its corner is** — its place in its tab's
/// ring, from 1 (`<C-w>w`'s count). Unpinned, `n + 7` passed (review stop
/// 11, F M-D).
#[test]
fn tabs_numbers_each_window_as_its_corner() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    let ring = app.workspace.ring_order();
    let text = tabs_text(&app);
    for (n, &pane) in ring.iter().enumerate() {
        let line = &text[1 + n];
        let listed = line[2..].split_whitespace().next();
        assert_eq!(
            listed,
            Some((n + 1).to_string()).as_deref(),
            "window {}: {line:?}",
            n + 1
        );
        // And as its corner is — the relation the name claims, which `n +
        // 1` alone never compared (review stop 12, T5 F10).
        assert_eq!(
            listed.and_then(|n| n.parse().ok()),
            app.corner_of(pane),
            "{line:?}"
        );
    }
}

/// **`<CR>` on a tab's lines goes to it** — any line of its block (each
/// names it, `Row::Tab`); on the tab you are in, said. Three tabs, the
/// cursor on the MIDDLE one's block: two tabs and `G` could not tell "the
/// tab under the cursor" from "the last" or "the next" (review stop 11, F
/// M-E).
#[test]
fn enter_in_tabs_goes_to_the_tab_under_the_cursor() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test("tabs");
    let size = (80, 30);
    // `{N}G` to the line `heading` names, plus `below` lines.
    let to = |app: &mut App, heading: &str, below: usize| {
        let at = (tabs_text(app).iter()).position(|l| l == heading).unwrap();
        type_keys_sized(app, size, &format!("{}G<CR>", at + 1 + below));
    };
    // A window line of the middle tab: that tab, standing in that window
    // (review stop 11, F 10) — its first, the transcript.
    to(&mut app, "Tab page 2", 1);
    assert_eq!(tabs(&app), (3, 2), "a line of tab 2's block: gone there");
    assert_eq!(
        app.focus(),
        PaneKind::Output,
        "in the window the line named"
    );
    // A heading: the tab; the one you are in, said.
    app.dispatch_command_for_test("tabs");
    to(&mut app, "Tab page 2", 0);
    assert_eq!(tabs(&app), (3, 2), "the tab you are in");
    assert_eq!(app.notice().as_deref(), Some("tab 2 is the one you are in"));
    to(&mut app, "Tab page 1", 0);
    assert_eq!(tabs(&app), (3, 1), "another's heading: gone there");
}

/// **A refusal's remedy survives an 80-column row** — drawn, not only
/// compared: the `!` came last and the row cut it (review stop 11, F 4).
#[tokio::test]
async fn a_close_refusal_keeps_its_remedy_at_80_columns() {
    let mut app = test_app();
    let a = app.current;
    ask_a_call(&mut app, a);
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabclose 1");
    assert_eq!(tabs(&app).0, 2, "premise: refused");
    let buf = frame_of(&mut app, 80, 24);
    let last = row(&buf, 23);
    assert!(last.contains("closes anyway"), "the remedy drawn: {last:?}");
}

/// **`:tabs` marks `%` on the prompt a tab with two writes in** — the one
/// `<Enter>` and `:Send` would take, which nothing marked (review stop 11,
/// F 2); `>` stays the window you are in.
#[test]
fn tabs_marks_the_prompt_you_write_in() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    app.split_command("", SplitDir::Vertical);
    app.settle();
    let writing = app.workspace.focused();
    app.set_focus(PaneKind::Output);
    let n = app.corner_of(writing).expect("its corner");
    let text = tabs_text(&app);
    let line = (text.iter())
        .find(|l| l[2..].split_whitespace().next() == Some(n.to_string().as_str()))
        .expect("its line");
    assert!(line.starts_with('%'), "{text:#?}");
    assert_eq!(
        text.iter().filter(|l| l.starts_with('%')).count(),
        1,
        "one: {text:#?}"
    );
    assert!(
        text.iter().any(|l| l.starts_with('>')),
        "and the one you are in"
    );
}

/// **The cursor in `:tabs` keeps its window when a tab before it closes** —
/// a window line names its pane, not a place (review stop 11, F 9): it slid
/// into another tab's block.
#[test]
fn tabs_keeps_its_place_when_a_tab_before_closes() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabs");
    let size = (80, 30);
    app.key_for_test(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT), size);
    let row = |app: &App| app.page_view().and_then(|w| w.row_under_cursor().cloned());
    let before = row(&app).expect("premise: a row under the cursor");
    app.dispatch_command_for_test("tabclose 1");
    app.before_frame(size.0, size.1);
    assert_eq!(row(&app), Some(before), "the same window's line");
}

/// **`<CR>` on a decision pane's line arms it** — it puts a call under your
/// cursor, and the key after is not an answer to it (as `:decide` arms it).
#[tokio::test]
async fn enter_on_a_decision_line_arms_it() {
    let mut app = test_app();
    let a = app.current;
    ask_a_call(&mut app, a);
    let pane = app.decision_pane_of(a).expect("its pane");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabs");
    let at = (tabs_text(&app).iter())
        .position(|l| {
            l.ends_with(&format!("decision for b{} {}", a.number(), {
                app.session_of(a).unwrap().conversation.episode().handle()
            }))
        })
        .expect("its line");
    let size = (80, 30);
    type_keys_sized(&mut app, size, &format!("{}G<CR>", at + 1));
    assert_eq!(app.workspace.focused(), pane, "stood in it, its tab shown");
    assert_eq!(app.answer_keys(pane), AnswerKeys::Waiting, "armed");

    // In the tab you are in, no tab shown to arm it: `<CR>` arms it itself.
    app.read_the_call_for_test();
    assert_eq!(app.answer_keys(pane), AnswerKeys::Live, "premise: read");
    app.set_focus(PaneKind::Prompt);
    app.split_command("tabs", SplitDir::Horizontal);
    // The `:split tabs` event's end: unsettled, the first key's settle saw
    // the split's focus move and dropped the count it began.
    app.settle();
    assert_eq!(app.decision_pane_of(a), Some(pane), "premise: still up");
    let at = (tabs_text(&app).iter())
        .position(|l| l.contains("decision for"))
        .expect("its line");
    type_keys_sized(&mut app, size, &format!("{}G<CR>", at + 1));
    assert_eq!(app.workspace.focused(), pane, "stood in it");
    assert_eq!(
        app.answer_keys(pane),
        AnswerKeys::Waiting,
        "armed by `<CR>`"
    );
}

/// **`<CR>` on a line naming a tab closed since it was drawn says so** — a
/// frame renews the list, but the keys of the same event as the close meet
/// the old lines; it went nowhere, in silence.
#[test]
fn enter_on_a_tab_closed_since_drawn_is_said() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnext 1");
    app.dispatch_command_for_test("tabs");
    app.dispatch_command_for_test("tabclose 3");
    let size = (80, 30);
    app.key_for_test(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT), size);
    app.key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), size);
    assert_eq!(tabs(&app), (2, 1), "nowhere to go");
    assert!(
        app.notice()
            .is_some_and(|said| said.starts_with("that window is gone")),
        "said: {:?}",
        app.notice()
    );
}

/// **`:tabs` is live** — a tab made while its pane is parked is listed
/// when the pane is drawn again (`renew_listings`, as `:ls` is).
#[test]
fn tabs_is_live() {
    let mut app = test_app();
    app.dispatch_command_for_test("tabs");
    let pane = app.workspace.focused();
    app.dispatch_command_for_test("tabnew");
    app.dispatch_command_for_test("tabnext 1");
    app.before_frame(80, 30);
    let listing = app.pane_listing(pane).expect("the `:tabs` pane");
    let text: Vec<String> = (listing.lines().iter()).map(|l| l.text()).collect();
    assert!(
        text.iter().any(|l| l == "Tab page 2"),
        "the new tab listed: {text:#?}"
    );
}
