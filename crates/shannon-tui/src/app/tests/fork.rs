//! **`:fork` / `:vfork`** (Steven, 10-14: "split the pane and branch the
//! conversation — the equivalent of `/btw`, but extendable").

use super::*;

/// The session panes other than the focused one.
fn other_session_panes(app: &App) -> Vec<crate::wm::PaneId> {
    let here = app.workspace.focused();
    app.session_panes(Over::ThisTab)
        .filter(|p| *p != here)
        .collect()
}

/// **A fork opens beside the conversation, and the conversation never sees
/// it** — a new pane, focused and aimed, on a clone holding the whole
/// history; the original goes on in its own pane, and a turn in the fork
/// leaves it as it was.
#[test]
fn a_fork_opens_beside_and_the_original_never_sees_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    let before = branch_msgs(&app);
    let panes = app.session_panes(Over::ThisTab).count();

    app.dispatch_command_for_test("vfork");
    assert_eq!(
        app.session_panes(Over::ThisTab).count(),
        panes + 1,
        "a pane beside"
    );
    let here = app.workspace.focused();
    let fork = app.pane_session(here).conversation.episode().clone();
    assert_ne!(fork.id, original, "the focused pane shows the fork");
    let origin = fork.origin.expect("a clone");
    assert_eq!(
        (origin.episode, origin.kind),
        (original, shannon_core::EdgeKind::Clone)
    );
    assert_eq!(
        app.prompt_target(),
        app.pane_session(here).id,
        "the prompt aims at it"
    );
    assert_eq!(branch_msgs(&app), before, "the whole history, shared");
    let others = other_session_panes(&app);
    assert_eq!(
        others.len(),
        panes,
        "premise: the original's pane is still there"
    );
    assert_eq!(
        app.pane_session(others[0]).conversation.episode().id,
        original,
        "the original goes on in its own pane"
    );

    app.conversation_mut().begin_turn("a side question");
    app.conversation_mut().push_chunk("a side answer");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let graph = app.current_graph();
    assert_eq!(
        graph.full_turns(&original).len(),
        2,
        "the original never saw it"
    );
    assert_eq!(graph.full_turns(&fork.id).len(), 3, "the fork goes on");
}

/// **A fork's window keeps the session it forked as its alternate** — born
/// on it, as a `:sp` window is, then showing the fork: `:b#` there goes
/// back to it, vim's `:split` then `:b` (V5.3 (3c): the window is born
/// holding its session, which showing the fork sets aside).
#[test]
fn a_forks_window_keeps_its_source_as_the_alternate() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let source = app.prompt_target();
    app.dispatch_command_for_test("vfork");
    let here = app.workspace.focused();
    assert_ne!(app.pane_session(here).id, source, "premise: the fork shown");
    app.dispatch_command_for_test("b#");
    assert_eq!(
        app.pane_session(here).id,
        source,
        "the session it forked, back"
    );
}

/// **A source not on disk yet is written first** — a clone references what
/// it shares — and one with no turns has nothing to fork, said, with no
/// pane opened.
#[test]
fn a_fork_writes_an_unsaved_source_and_refuses_an_empty_one() {
    let mut app = test_app();
    app.notices.clear();
    app.dispatch_command_for_test("fork");
    assert!(
        app.notice().unwrap_or_default().contains("no turns"),
        "{:?}",
        app.notice()
    );
    assert_eq!(
        app.session_panes(Over::ThisTab).count(),
        1,
        "no pane for nothing"
    );

    app.conversation_mut().begin_turn("one");
    app.conversation_mut().push_chunk("reply");
    app.conversation_mut().commit_turn();
    let source = app.conversation().episode().id;
    assert!(
        matches!(app.store.episode_len(&source), Ok(None)),
        "premise: not on disk"
    );
    app.dispatch_command_for_test("fork");
    assert!(
        matches!(app.store.episode_len(&source), Ok(Some(_))),
        "written first"
    );
    let fork = app
        .pane_session(app.workspace.focused())
        .conversation
        .episode()
        .id;
    assert_ne!(fork, source, "and forked");

    app.notices.clear();
    app.dispatch_command_for_test("fork now");
    assert!(app.notice().unwrap_or_default().contains("takes nothing"));
}

// ---- threading: a thread is a branch, started where you are reading --------

/// `:reply` on an earlier turn checks out that point, so the next message forks
/// a thread there. Threading is a view over the graph we already had; what was
/// missing was the gesture — `:graph` could do it, so the capability existed
/// and nobody could reach it from where they were reading.
#[tokio::test]
async fn reply_starts_a_thread_at_the_turn_under_the_cursor() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 4);
    let trunk = app.conversation().episode().id;
    app.reset_foreground();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.open_by_hash(&trunk.to_hex()[..8]);
    let turns = app.current_graph().full_turns(&trunk).len();
    assert!(turns >= 4, "premise: several turns to point at");

    // Put the cursor on the FIRST turn's echo line and reply there.
    app.sync_chat();
    app.pane_chat_mut(pane).unwrap().place_cursor(0, 0);
    app.reply_command();

    assert_eq!(
        app.pane_session(pane).output.turn_of_line(0),
        Some(0),
        "line 0 belongs to turn 0 — the mapping the gesture rests on"
    );
    assert_ne!(
        app.conversation().episode().id,
        trunk,
        "we are on a checkout of that point, not still on the trunk"
    );
    assert!(
        app.conversation().episode().origin.is_some(),
        "and it hangs off the turn we pointed at"
    );
}

/// Replying to the LAST turn is just continuing, and says so rather than
/// silently making a branch nobody asked for.
#[tokio::test]
async fn reply_on_the_last_turn_says_to_just_type() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let trunk = app.conversation().episode().id;
    app.reset_foreground();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.open_by_hash(&trunk.to_hex()[..8]);
    app.sync_chat(); // the cursor indexes the mirror, so it has to exist
    let last_line = app.pane_session(pane).output.line_count().saturating_sub(1);
    app.pane_chat_mut(pane).unwrap().place_cursor(last_line, 0);

    app.reply_command();

    assert!(
        app.notice()
            .expect("it says something")
            .contains("just type"),
        "names the reason instead of forking: {:?}",
        app.notice()
    );
    assert_eq!(
        app.conversation().episode().id,
        trunk,
        "and nothing was checked out"
    );
}

/// A line that belongs to no turn refuses visibly rather than guessing.
#[test]
fn reply_from_outside_a_transcript_refuses() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    app.reply_command();
    assert!(
        app.notice().expect("a refusal").contains("transcript"),
        "says where to stand"
    );
}

/// **`:reply` from a view lent over a transcript refuses** — the pane owns
/// the session beneath (`session_panes`), but its cursor is not on screen:
/// it replied from a turn you could not see, under a refusal that says to
/// stand in a transcript (V5.3 design review, finding 6).
#[tokio::test]
async fn reply_from_a_view_over_a_transcript_refuses() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 4);
    let trunk = app.conversation().episode().id;
    app.reset_foreground();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.open_by_hash(&trunk.to_hex()[..8]);
    app.sync_chat();
    app.pane_chat_mut(pane).unwrap().place_cursor(0, 0);
    app.open_help_command(None);
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Help)),
        "premise"
    );
    app.reply_command();
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("stand in a transcript")),
        "{:?}",
        app.notice()
    );
    assert_eq!(
        app.conversation().episode().id,
        trunk,
        "nothing checked out"
    );
}

/// `G` in the transcript means "take me to the live end" — so it must LEAVE you
/// following it. It routed through the raw `scroll_to`, which clears follow
/// unconditionally, making the one key that means "show me what is happening"
/// the key that stopped new replies from being shown.
#[test]
fn going_to_the_end_keeps_following_the_tail() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 12);
    let long = app.conversation().episode().id;
    app.reset_foreground();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.open_by_hash(&long.to_hex()[..8]);
    app.sync_chat();

    // `gg` — cursor to the top, which pulls the view off the tail.
    app.pane_chat_mut(pane).unwrap().place_cursor(0, 0);
    app.follow_pane_cursor(pane, (80, 10));
    assert!(
        !app.pane_viewport(pane).is_following_tail(),
        "premise: moving to the top left the tail"
    );

    // `G` — cursor to the end.
    let last = app.pane_session(pane).output.line_count().saturating_sub(1);
    app.pane_chat_mut(pane).unwrap().place_cursor(last, 0);
    app.follow_pane_cursor(pane, (80, 10));

    assert!(
        app.pane_viewport(pane).is_following_tail(),
        "arriving at the end IS following the end"
    );
}
