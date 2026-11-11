//! Sessions in panes: opening, reclaiming, and the session a pane shows.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// `:continue` shows you the transcript it is about to stream into.
///
/// It costs money, so a `:continue` typed while a viewer had borrowed your only
/// transcript pane sent a billable reply somewhere you could not see. Same dead
/// key as `:new`, on a command with a price.
#[tokio::test]
async fn continue_shows_the_transcript_it_streams_into() {
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()), // Instruct strategy — no runtime needed
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    // A turn that ended normally: `:continue` must refuse it.
    app.conversation_mut().begin_turn("Hello 0");
    app.conversation_mut().push_chunk("reply 0");
    app.conversation_mut()
        .record_stop_reason(StopReason::EndTurn);
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();

    app.set_focus(PaneKind::Output);
    let here = app.workspace.focused();
    app.open_help_command(None);
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Page(PageKind::Help))
    );

    app.continue_last_turn();
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Page(PageKind::Help)),
        "a REFUSED continue leaves your manual alone"
    );

    // Now a truncated tip — genuinely continuable.
    app.conversation_mut().begin_turn("Hello 1");
    app.conversation_mut().push_chunk("reply 1 cut");
    app.conversation_mut()
        .record_stop_reason(StopReason::MaxTokens);
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();

    app.continue_last_turn();
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Output),
        "and a real one gives the pane back, so you can watch it arrive"
    );
}

/// `:open` behaves the same way whichever id space the hash lands in.
///
/// An episode id went through `switch_session` (which gives a lent viewport
/// back) and a turn id through `checkout_turn` (which did not), so **one
/// command had two behaviours** decided by something the user cannot see: which
/// kind of thing their eight hex characters happened to name.
#[test]
fn open_gives_the_viewport_back_for_a_turn_id_as_it_does_for_a_session_id() {
    // **From both places you can type it from.** This ran only with the graph
    // pane focused, which is the one starting position where the old
    // `reveal` — "is the FOCUSED pane a session pane?" — happened to say yes.
    // Typed at the prompt, where you actually type, `:open <turn-id>` checked
    // out the turn, detached the foreground onto a branch, and changed nothing
    // on screen, while `:open <session-id>` one keystroke away gave the
    // viewport back. The test's doc stated the general rule; its body tested
    // one starting position (Reviewer A, round 3).
    for from_prompt in [false, true] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 3);
        let turn0 = app.conversation().episode().own_turns()[0];
        open_graph_pane(&mut app);
        let here = app.workspace.focused();
        assert_eq!(app.workspace.kind_of(here), Some(PaneKind::Graph));
        if from_prompt {
            app.set_focus(PaneKind::Prompt);
            assert_ne!(
                app.workspace.focused(),
                here,
                "premise: standing somewhere that cannot host a transcript"
            );
        }

        app.open_command(&turn0.to_hex()[..8]);

        assert_eq!(
            app.workspace.kind_of(here),
            Some(PaneKind::Output),
            "from_prompt={from_prompt}: checking out a turn lands you in the \
             transcript, like opening a session does"
        );
        assert_eq!(
            branch_msgs(&app),
            vec!["Hello 0", "reply 0"],
            "from_prompt={from_prompt}: and that transcript is the checked-out point"
        );
    }
}

/// **`:wa` moves `HEAD` only where you are** (review stop 2, A F3 · C 4;
/// D1): the quit refusal names a lazy clone, `:wa` writes it — and its first
/// write made it `HEAD`, so the next launch opened the clone, not the
/// session you were in.
#[test]
fn write_all_moves_head_only_where_you_are() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, left, right) = app_with_two_sessions(dir.path());
    app.focus_pane(right);
    app.open_by_hash(&b.to_hex()[..8]);
    app.dispatch_command_for_test("clone");
    let clone = app.conversation().episode().id;
    assert!(
        matches!(app.store.episode_len(&clone), Ok(None)),
        "premise: a lazy clone"
    );
    app.focus_pane(left);
    app.open_by_hash(&a.to_hex()[..8]);
    assert_eq!(
        app.store.get_ref("HEAD").unwrap(),
        Some(a),
        "premise: HEAD is A"
    );

    app.dispatch_command_for_test("wa");
    assert!(
        matches!(app.store.episode_len(&clone), Ok(Some(_))),
        "the clone is written"
    );
    assert_eq!(
        app.store.get_ref("HEAD").unwrap(),
        Some(a),
        "and the next launch opens where you are"
    );
}

/// **A turn that finishes in a pane you have left does not move `HEAD`**
/// (D1; Steven: "If we put a pane in the background I don't want to
/// teleport to it"): a fresh session's first turn, its first write, landing
/// while you are aimed elsewhere.
#[tokio::test]
async fn a_first_write_in_a_pane_you_left_leaves_head() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, _, left, right) = app_with_two_sessions(dir.path());
    app.focus_pane(right);
    app.new_session_command();
    let fresh = app.conversation().episode().id;
    app.submit("a question");
    app.focus_pane(left);
    app.open_by_hash(&a.to_hex()[..8]);
    assert_eq!(
        app.store.get_ref("HEAD").unwrap(),
        Some(a),
        "premise: HEAD is A"
    );

    app.feed_stream(StreamMsg::Chunk("an answer".into()));
    app.feed_stream(StreamMsg::Complete);
    assert!(
        matches!(app.store.episode_len(&fresh), Ok(Some(1))),
        "premise: its first write landed"
    );
    assert_eq!(app.store.get_ref("HEAD").unwrap(), Some(a), "HEAD stays");
}

/// **`:graph`'s `(HEAD)` is the stored ref** (review stop 2, A F7; D1): it
/// read `current`, and named a session the next launch would not open
/// whenever the two parted.
#[test]
fn the_graphs_head_flag_is_the_stored_ref() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, left, _) = app_with_two_sessions(dir.path());
    app.focus_pane(left);
    app.open_by_hash(&a.to_hex()[..8]);
    app.store.put_ref("HEAD", &b).unwrap();
    assert_eq!(app.conversation().episode().id, a, "premise: A in front");
    assert_eq!(app.build_graph_view().active(), Some(b));
}

/// `:open` points **the pane you are in** at another session, and leaves its
/// sibling alone.
///
/// The sibling is the whole test. A pane nobody had bound followed the
/// foreground — exactly wrong the instant the foreground moves, because then
/// *every* unbound pane followed: "it changed both panes" in its
/// buffer-shaped form. `pin_session_panes` stopped it by pinning each pane
/// first; since V5.3 (2) each is born on its session, and there is nothing
/// to pin (making the first one unbound turns 55 tests red, this among them).
#[test]
fn opening_a_session_changes_only_the_pane_you_are_in() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());

    app.open_by_hash(&a.to_hex()[..8]);

    assert_eq!(
        app.pane_session(focused).conversation.episode().id,
        a,
        "the focused pane shows what was opened"
    );
    assert_eq!(
        app.pane_session(sibling).conversation.episode().id,
        b,
        "its sibling stayed where it was"
    );
    // The prompt follows the pane you opened it in — one prompt, one foreground.
    assert_eq!(app.conversation().episode().id, a, "the foreground moved");
}

/// One session, one buffer, however many panes show it.
///
/// Find-**or**-create: a second load would be a second `Conversation` over the
/// same episode, and a turn appended through one pane would leave the other
/// showing a transcript that is quietly out of date.
#[test]
fn two_panes_on_one_session_share_a_buffer() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, _b, sibling, focused) = app_with_two_sessions(dir.path());

    app.open_by_hash(&a.to_hex()[..8]);
    app.workspace.focus_pane(sibling);
    app.open_by_hash(&a.to_hex()[..8]);

    assert_eq!(
        app.pane_session(focused).id,
        app.pane_session(sibling).id,
        "both panes resolve to the same buffer"
    );
    assert_eq!(
        app.buffers.sessions().count(),
        2,
        "A and B — A was not loaded twice"
    );
}

/// A split shows the same session as the pane it split, which is vim (`:split`
/// gives you a second window on the same buffer) — and is required, not
/// decorative: the newcomer would otherwise fall back to the foreground and
/// show a different transcript under a command that promised the same one.
#[test]
fn a_split_inherits_the_session_of_the_pane_it_split() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, _focused) = app_with_two_sessions(dir.path());
    // Open A here, so the FOREGROUND is A while `sibling` still shows B. Split
    // the sibling: an un-inherited newcomer falls back to the foreground, so
    // this is the only arrangement where inheriting and not inheriting give
    // different answers.
    app.open_by_hash(&a.to_hex()[..8]);
    app.workspace.focus_pane(sibling);

    app.split_command("", SplitDir::Horizontal);
    let fresh = app.workspace.focused();
    assert_ne!(fresh, sibling, "a new pane");
    assert_eq!(
        app.pane_session(fresh).conversation.episode().id,
        b,
        "the newcomer shows what was split, not the foreground"
    );
}

/// **A split VIEWER inherits no session** — only a transcript has one to
/// inherit. `split_pane`'s comment said so and its filter excluded the prompt
/// alone, so `:sgraph` handed the graph its neighbour's session, against
/// `Slot::buffer`'s own doc (review 2026-10-15, found fixing B F3 — the
/// status row read the split graph as that session's pane).
#[test]
fn a_split_viewer_inherits_no_session() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, _, sibling, _) = app_with_two_sessions(dir.path());
    app.focus_pane(sibling);
    // A, not B: B is in front already, and opening what an unbound pane
    // already shows binds nothing.
    app.open_by_hash(&a.to_hex()[..8]);
    assert!(app.live_session(sibling).is_some(), "premise: bound");
    app.dispatch_command_for_test("sgraph");
    let graph = app.workspace.focused();
    assert_eq!(
        app.workspace.kind_of(graph),
        Some(PaneKind::Graph),
        "premise"
    );
    assert_eq!(app.live_session(graph), None);
}

/// **A transcript split FROM a viewer shows what the viewer is about** — the
/// aim (review 2026-10-16, Reviewer A F8). A split viewer carries no session
/// (above), so `:sp output` typed in it made an unbound transcript, which
/// follows `current`: B in front, aimed at A, and the new pane showed B
/// beside a status row that had described A.
#[test]
fn a_transcript_split_from_a_viewer_shows_the_aim() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (here, there) = (panes[0], panes[1]);
    app.focus_pane(here);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    app.focus_pane(there);
    app.open_by_hash(&b.to_hex()[..8]);
    app.focus_pane(here);
    assert_eq!(app.prompt_target(), a_buffer, "premise: aimed at A");
    assert_eq!(app.conversation().episode().id, b, "premise: B is in front");

    app.dispatch_command_for_test("sgraph");
    assert_eq!(app.focus(), PaneKind::Graph, "premise: in a split viewer");
    app.split_command("output", SplitDir::Horizontal);
    let fresh = app.workspace.focused();
    assert_eq!(
        app.workspace.kind_of(fresh),
        Some(PaneKind::Output),
        "premise"
    );
    assert_eq!(
        app.pane_session(fresh).conversation.episode().id,
        a,
        "the session the viewer was about, not the one in front"
    );
}

/// From the prompt bar with **several** transcript panes, the session opens in
/// the pane the prompt is AIMED at.
///
/// This used to refuse — "several transcript panes, focus one first" — because
/// the pane would otherwise be picked by creation order, which is nowhere on
/// screen (`[[feedback_predictable_from_screen]]`). P4 put the answer on the
/// screen: the prompt's title names the target and that pane wears the accent.
/// The refusal outlived its reason, and it dead-ended the manual's own recipe
/// (`:vs`, `:new`, then type) at the exact place you stand to type.
#[test]
fn opening_from_the_prompt_lands_on_the_aimed_pane() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());
    // Aim at `sibling` explicitly, then go to the prompt bar.
    // Give the panes DIFFERENT sessions, so the aim names exactly one of them —
    // with both showing one buffer, "the pane showing the aim" is ambiguous and
    // the test would only be pinning down the tie-break.
    app.focus_pane(focused);
    app.open_by_hash(&a.to_hex()[..8]); // focused → A, sibling pinned to B, aim = A
    assert_eq!(app.pane_session(sibling).conversation.episode().id, b);
    app.set_focus(PaneKind::Prompt);

    app.new_session_command();

    assert!(app.notice().is_none(), "no refusal: {:?}", app.notice());
    assert_ne!(
        app.pane_session(focused).conversation.episode().id,
        a,
        "the aimed pane is the one that got the new session"
    );
    assert_eq!(
        app.pane_session(sibling).conversation.episode().id,
        b,
        "and the pane we were not aimed at kept its session"
    );
}

/// With **one** transcript pane there is nothing to guess: it is the only
/// candidate, so `:open` from the prompt — where you actually stand when you
/// type it — just works. (This is the single-pane case the whole suite runs in,
/// pinned so the ambiguity guard above can never swallow it.)
#[test]
fn opening_from_the_prompt_with_one_transcript_just_works() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let a = app.conversation().episode().id;
    add_persisted_session(&mut app, "B0");
    app.set_focus(PaneKind::Prompt);

    app.open_by_hash(&a.to_hex()[..8]);

    assert_eq!(app.conversation().episode().id, a, "opened the only pane");
    assert!(app.notice().is_none(), "no refusal: {:?}", app.notice());
}
