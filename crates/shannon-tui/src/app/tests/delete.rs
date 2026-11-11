//! Deleting sessions and turns: what is refused, what is asked, what goes.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

#[test]
fn delete_is_gated_by_confirmation_then_removes_the_session() {
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.conversation_mut().begin_turn("doomed");
    app.conversation_mut().push_chunk("reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let id = app.conversation().episode().id;

    // Arm the confirmation (what `:delete` does) — nothing deleted yet.
    app.pending_confirm.push_back(ConfirmAction::sessions(vec![
        app.conversation().episode().id,
    ]));
    assert!(app.confirm_prompt().is_some(), "prompt shown");

    // `n` cancels — the session survives, on disk and in memory.
    type_keys(&mut app, "n");
    assert!(app.pending_confirm.is_empty(), "prompt closed on cancel");
    assert_eq!(
        app.conversation().episode().id,
        id,
        "same session after cancel"
    );
    assert!(
        JsonlStore::new(dir.path())
            .unwrap()
            .load_episode(&id)
            .is_ok(),
        "cancel did not delete",
    );

    // Re-arm and confirm with `y` — deleted, fresh session started.
    app.pending_confirm.push_back(ConfirmAction::sessions(vec![
        app.conversation().episode().id,
    ]));
    type_keys(&mut app, "y");
    assert!(app.pending_confirm.is_empty());
    assert_ne!(
        app.conversation().episode().id,
        id,
        "a fresh session after delete"
    );
    assert!(app.output().is_empty(), "transcript cleared");

    let store = JsonlStore::new(dir.path()).unwrap();
    assert!(
        matches!(store.load_episode(&id), Err(StoreError::EpisodeNotFound(_))),
        "the old session is gone from disk",
    );

    // The fresh session persists lazily; once it has a turn it registers and
    // HEAD lands on it — never the deleted episode.
    let fresh_id = app.conversation().episode().id;
    app.conversation_mut().begin_turn("after delete");
    app.conversation_mut().push_chunk("ok");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let store = JsonlStore::new(dir.path()).unwrap();
    assert_eq!(
        store.get_ref("HEAD").unwrap(),
        Some(fresh_id),
        "HEAD points at the fresh session, never the deleted one",
    );
}

#[test]
fn delete_all_prunes_every_session_and_is_sticky_confirmation_gated() {
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    // Two persisted sessions.
    app.conversation_mut().begin_turn("one");
    app.conversation_mut().push_chunk("a");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    app.reset_foreground();
    app.conversation_mut().begin_turn("two");
    app.conversation_mut().push_chunk("b");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let count = || {
        JsonlStore::new(dir.path())
            .unwrap()
            .list_episodes()
            .unwrap()
            .len()
    };
    assert_eq!(count(), 2, "two sessions on disk");

    // Arm `:delete all`. A stray key is STICKY — it neither fires nor cancels.
    app.pending_confirm.push_back(ConfirmAction::DeleteAll);
    type_keys(&mut app, "x");
    assert!(
        !app.pending_confirm.is_empty(),
        "a stray key keeps the prompt up (sticky)"
    );
    assert_eq!(count(), 2, "nothing pruned until confirmed");

    // Confirm with `y` → everything pruned, fresh empty session.
    type_keys(&mut app, "y");
    assert!(app.pending_confirm.is_empty());
    assert!(app.output().is_empty(), "a fresh session after the prune");
    assert_eq!(count(), 0, "all sessions pruned from disk");
}

#[test]
fn deleting_a_turn_of_a_session_open_nowhere_keeps_the_rest() {
    let dir = TempDir::new().unwrap();
    let old = app_with_n_turns(dir.path(), 2).conversation().episode().id;
    let mut app = relaunched(dir.path());
    select_stored_turn(&mut app, old, 0);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    let action = app.pending_confirm.pop_front().expect("asks first");
    app.run_confirmed(action);

    // The rewrite carries every kept turn's words — it read them first.
    let g = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    let words: Vec<String> = g
        .records(&old)
        .expect("a full load holds every body")
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| m.content.clone()))
        .collect();
    assert_eq!(words, ["Hello 1", "reply 1"], "{:?}", app.notice());
}

/// **Deleting what a clone shares leaves the clone whole** (Fable #2's case,
/// inverted 10-04). A clone stores only a Slice reference and the turn
/// contents live in the source's log, so deleting the source orphaned it —
/// which is why this was refused, and why a forked conversation could not be
/// deleted at all. The clone is given its own copy first; on disk, and in a
/// way the next turn appends to rather than re-writing.
#[test]
fn deleting_a_session_that_a_clone_shares_keeps_the_clone_whole() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2); // original, persisted
    let original = app.conversation().episode().id;
    app.clone_episode(original);
    let clone = app.conversation().episode().id;
    continue_with(&mut app, "fwd"); // persisted: its Slice into `original` lands
    let before = on_disk(dir.path(), &clone);
    assert_eq!(before, ["Hello 0", "Hello 1", "fwd"], "premise");

    // `replaced`: the original keeps a buffer of its own since the clone
    // lands beside it (10-21), and deleting it resets that buffer.
    assert_eq!(
        app.delete_episode(original),
        Deleted::Removed {
            replaced: true,
            kept: 1,
            with: Lost::default(),
        }
    );
    let store = JsonlStore::new(dir.path()).unwrap();
    assert!(store.load_episode(&original).is_err(), "the source is gone");
    assert_eq!(on_disk(dir.path(), &clone), before, "the clone is whole");
    continue_with(&mut app, "after");
    assert_eq!(
        on_disk(dir.path(), &clone),
        ["Hello 0", "Hello 1", "fwd", "after"],
        "and goes on from there"
    );
}

/// **A clone that is not on disk yet is kept whole too** (the deep-review
/// data-loss edge: 3/4 reviewers). The active session is a LAZY clone of
/// `original`, invisible to `load_graph`, still slicing `original` — so the
/// live buffers are where it is found, and the copy registers it.
#[test]
fn deleting_the_source_of_a_live_unpersisted_clone_keeps_the_clone_whole() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2); // original, persisted
    let original = app.conversation().episode().id;
    app.clone_episode(original);
    let clone = app.conversation().episode().id;
    assert_eq!(app.buffer().on_disk, OnDisk::Absent, "premise: unpersisted");

    assert!(matches!(
        app.delete_episode(original),
        Deleted::Removed { kept: 1, .. }
    ));
    assert_eq!(on_disk(dir.path(), &clone), ["Hello 0", "Hello 1"]);
    continue_with(&mut app, "after");
    assert_eq!(on_disk(dir.path(), &clone), ["Hello 0", "Hello 1", "after"]);
}

/// **A second question must not silently replace the first.**
///
/// `pending_confirm` was one slot with four writers, and one of them arrives
/// *asynchronously* — a summary's "apply it?" was, whenever the backend
/// finished and `$EDITOR` exited (two keystrokes, `S` then `d` in `:graph`);
/// a child over the `[agents]` limit is, whenever the model asks for one.
/// Whichever landed second overwrote the other.
///
/// The failure is the worst kind. The prompt changes under a user already
/// reaching for `y`, so their answer lands on a question they never read — and
/// the replaced question's work is simply gone. A queue is the fix rather than
/// a refusal, because by the time the second one is asked its work exists;
/// refusing would lose it just as surely.
#[tokio::test]
async fn a_second_question_queues_behind_the_first_instead_of_replacing_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let episode = app.conversation().episode().id;

    // `d` in `:graph` — a delete waiting on y/N.
    app.request_delete(ConfirmAction::sessions(vec![episode]));
    let first = app.confirm_prompt().expect("a question is up").text();
    assert!(first.contains("delete"), "{first}");

    // …and a child asked for meanwhile crosses the `[agents]` limit, wanting
    // an answer too.
    app.ask(ConfirmAction::SpawnChild {
        briefing: "look around".into(),
        model: None,
        reason: "a third child would pass [agents] max_fanout".into(),
    });

    let still = app.confirm_prompt().expect("still a question").text();
    assert!(
        still.contains("delete"),
        "the question on screen is the one the user was answering: {still}"
    );
    assert!(
        still.contains("1 more"),
        "and it says another is waiting, so a second prompt reads as \
         `next question` rather than `it did not take`: {still}"
    );

    // Answering the first reveals the second rather than losing it.
    type_keys(&mut app, "n");
    let second = app
        .confirm_prompt()
        .expect("the child's question survived")
        .text();
    assert!(second.contains("max_fanout"), "{second}");
    assert!(!second.contains("more"), "and it is the last one: {second}");
}

#[test]
fn delete_by_hash_gates_then_deletes_a_specific_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2); // session A
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0"); // active

    // `:delete <A>` gates on a confirmation (default on) — nothing deleted yet.
    app.delete_by_hash(&a.to_hex()[..8]);
    assert_eq!(
        app.pending_confirm.front(),
        Some(&ConfirmAction::sessions(vec![a]))
    );
    assert!(
        app.current_graph().episode_ids().contains(&a),
        "still present"
    );

    // Confirm → A is pruned; B (active) is untouched.
    let action = app.pending_confirm.pop_front().unwrap();
    app.run_confirmed(action);
    let ids = app.current_graph().episode_ids();
    assert!(!ids.contains(&a), "A pruned");
    assert!(ids.contains(&b), "B kept");
    assert_eq!(
        app.conversation().episode().id,
        b,
        "active session unchanged"
    );
}

#[test]
fn delete_skips_confirmation_when_disabled() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2); // session A
    let a = app.conversation().episode().id;
    add_persisted_session(&mut app, "B0"); // move off A
    let mut cfg = (*app.config).clone();
    cfg.tui.confirm_delete = false;
    app.config = std::sync::Arc::new(cfg);

    app.delete_by_hash(&a.to_hex()[..8]);
    assert!(
        app.pending_confirm.is_empty(),
        "no confirmation when disabled"
    );
    assert!(
        !app.current_graph().episode_ids().contains(&a),
        "deleted immediately"
    );
}

#[test]
fn delete_by_hash_rejects_a_turn_id() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let turn = app.conversation().episode().own_turns()[0];
    // A turn id resolves to no session — nothing is staged or deleted.
    app.delete_by_hash(&turn.to_hex()[..8]);
    assert!(app.pending_confirm.is_empty(), "a turn id is not deletable");
}

#[test]
fn d_in_graph_deletes_selected_session_and_keeps_viewer_open() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2); // session A
    let a = app.conversation().episode().id;
    add_persisted_session(&mut app, "B0"); // active = B

    open_graph(&mut app);
    app.graph_view_mut().unwrap().reveal_and_select_episode(a);

    // `d` stages the delete (confirmation on) and leaves the viewer open —
    // the tree it was pressed in carried, for its marks.
    let tree = app.workspace.focused();
    app.graph_key_for_test(k(KeyCode::Char('d')));
    assert_eq!(
        app.pending_confirm.front(),
        Some(&ConfirmAction::Delete {
            episodes: vec![a],
            turns: Vec::new(),
            leaves_gap: false,
            claude_left: 0,
            from: Some(tree),
        })
    );
    assert!(
        app.graph_view().is_some(),
        "viewer stays open while confirming"
    );

    // Confirm → A is gone and the viewer is rebuilt (still open).
    let action = app.pending_confirm.pop_front().unwrap();
    app.run_confirmed(action);
    assert!(!app.current_graph().episode_ids().contains(&a), "A deleted");
    assert!(app.graph_view().is_some(), "viewer refreshed, still open");
}

/// **A shared session's delete says the clone keeps its turns** — on screen,
/// since the clone's storage changing is otherwise invisible, and the clone
/// reads what it read.
#[test]
fn deleting_a_shared_session_says_the_clone_keeps_a_copy() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(original);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.dispatch_command_for_test("clone");
    let clone = app.conversation().episode().id;
    assert_ne!(clone, original, "premise: cloned");
    let before = app.current_graph().full_turns(&clone);

    app.notices.clear();
    app.delete_by_hash(&original.to_hex()[..8]);
    if app.confirm_prompt().is_some() {
        type_keys(&mut app, "y");
    }
    let notice = app.notice().unwrap_or_default();
    assert!(notice.contains("keeps its own copy"), "{notice}");
    let graph = app.current_graph();
    assert!(
        !graph.episode_ids().contains(&original),
        "the original went"
    );
    assert_eq!(graph.full_turns(&clone), before, "the clone is whole");
}

/// **Deleted and kept, in one notice.** Two notices put the refusal on the
/// status line and then replaced it with the count — the shape Steven met.
/// Sharing no longer refuses, so the kept session here is one whose clone's
/// copy could not be written: the disk says no, and the original stays
/// rather than leave the clone reading nothing.
#[test]
fn a_batch_says_what_it_deleted_and_what_it_kept_together() {
    use shannon_core::store::FailingStore;
    let (store, failing) = FailingStore::recoverable();
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    continue_with(&mut app, "Hello 0");
    let original = app.conversation().episode().id;
    let spare = add_persisted_session(&mut app, "spare");
    // In front: a clone sharing the original's turns.
    app.clone_episode(original);
    failing.store(true, std::sync::atomic::Ordering::SeqCst);
    open_graph_pane(&mut app);
    let spare_name = app.session_label(spare);
    delete_marked(
        &mut app,
        &[Node::Episode(original), Node::Episode(spare)],
        'y',
    );
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with(&format!("deleted {spare_name};")),
        "the one that went, by name: {said}"
    );
    assert!(said.contains("not deleted"), "{said}");
    assert!(said.contains(&original.to_hex()[..8]), "{said}");
    assert!(said.contains("own copy"), "and why: {said}");
    let graph = app.current_graph();
    assert!(!graph.episode_ids().contains(&spare), "the spare went");
    assert!(
        graph.episode_ids().contains(&original),
        "the original stayed"
    );
}

/// **A store that cannot be listed rules nothing out**: the delete is refused,
/// with why, rather than taken as "nobody shares it".
#[cfg(unix)]
#[test]
fn a_delete_is_refused_when_the_store_cannot_be_listed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    // Nothing shares it: listable, this delete would go through.
    let mode = |m| std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(m));
    mode(0o000).unwrap();
    let why = app.delete_from_store(original);
    mode(0o700).unwrap();
    let why = why.expect_err("refused");
    assert!(why.contains("could not be read"), "{why}");
    assert!(
        app.current_graph().episode_ids().contains(&original),
        "still there"
    );
}

/// **A declined delete spends nothing.** The marks are the user's work;
/// clearing them on `n` would punish the safe answer.
#[test]
fn a_declined_delete_keeps_the_marks() {
    let dir = TempDir::new().unwrap();
    let (mut app, original, clone) = original_and_clone(dir.path());
    delete_marked(
        &mut app,
        &[Node::Episode(original), Node::Episode(clone)],
        'n',
    );
    assert_eq!(marked(&app), Some(2), "both still marked");
    assert!(app.current_graph().episode_ids().contains(&original));
}

/// **A shared session marked alone goes, and its mark is spent** — the
/// clone, unmarked, keeps its turns. This is the delete Steven could not
/// make: marked sessions refused for forks he had not marked, still lit
/// afterwards (10-04).
#[test]
fn a_shared_session_marked_alone_goes_and_spends_the_mark() {
    let dir = TempDir::new().unwrap();
    let (mut app, original, clone) = original_and_clone(dir.path());
    let before = app.current_graph().full_turns(&clone);
    let name = app.session_label(original);
    delete_marked(&mut app, &[Node::Episode(original)], 'y');
    let said = app.notice().unwrap_or_default();
    assert!(said.starts_with(&format!("deleted {name}")), "{said}");
    assert!(!said.contains("not deleted"), "{said}");
    assert_eq!(app.current_graph().full_turns(&clone), before);
    assert_eq!(marked(&app), Some(0), "the marks are spent");
}

/// **A session and the clone that shares it, marked together, both go — the
/// clone first**, so it is never given a copy of turns only to be deleted a
/// moment later (`forks_first`), whatever order the marks came in.
#[test]
fn a_batch_deletes_a_session_after_the_clone_that_shares_it() {
    let dir = TempDir::new().unwrap();
    let (mut app, original, clone) = original_and_clone(dir.path());
    let names = [app.session_label(original), app.session_label(clone)];
    delete_marked(
        &mut app,
        &[Node::Episode(original), Node::Episode(clone)],
        'y',
    );
    let said = app.notice().unwrap_or_default();
    assert!(said.starts_with("deleted "), "{said}");
    for name in &names {
        assert!(said.contains(name.as_str()), "both named — {name}: {said}");
    }
    assert!(!said.contains("not deleted"), "{said}");
    assert!(!said.contains("own copy"), "nothing was copied: {said}");
}

/// **The report names the sessions in the order they were asked about** —
/// not the order they went in, which puts a clone before what it shares
/// (`forks_first`), so the account reads like the question did.
#[test]
fn a_delete_reports_in_the_order_it_was_asked() {
    let dir = TempDir::new().unwrap();
    let (mut app, original, clone) = original_and_clone(dir.path());
    let (first, second) = (app.session_label(original), app.session_label(clone));
    assert_eq!(
        app.forks_first(&[original, clone]),
        [clone, original],
        "premise: the clone goes first"
    );
    app.delete_episodes(&[original, clone]);
    let said = app.notice().unwrap_or_default();
    let at = |name: &str| {
        said.find(name)
            .unwrap_or_else(|| panic!("{name} named: {said}"))
    };
    assert!(at(&first) < at(&second), "asked order: {said}");
}

/// Deleting a session that **another pane is showing** leaves that pane on a
/// fresh empty session, not on a transcript of something that no longer exists.
///
/// "Fall back visibly" was written down as a hazard before this was buildable
/// (`docs/DesignDocs/buffers.md` §4); the fix is that every holder falls back,
/// rather than the foreground being special-cased.
#[test]
fn deleting_a_session_open_in_another_pane_falls_that_pane_back() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());
    app.open_by_hash(&a.to_hex()[..8]); // `focused` shows A, `sibling` shows B
    app.workspace.focus_pane(sibling);
    app.open_by_hash(&b.to_hex()[..8]); // pin B explicitly
    // Back to A, so B is held ONLY by a background pane — otherwise the delete
    // takes the foreground path and the fallback under test never runs.
    app.workspace.focus_pane(focused);
    app.open_by_hash(&a.to_hex()[..8]);
    assert_ne!(
        app.conversation().episode().id,
        b,
        "B is not the foreground"
    );

    let mut cfg = (*app.config).clone();
    cfg.tui.confirm_delete = false;
    app.config = std::sync::Arc::new(cfg);
    app.delete_by_hash(&b.to_hex()[..8]);

    assert!(
        !app.current_graph().episode_ids().contains(&b),
        "B is gone from the store"
    );
    let fallen = app.pane_session(sibling).conversation.episode().id;
    assert_ne!(fallen, b, "the pane is not showing the deleted session");
    assert!(
        app.pane_session(sibling).output.is_empty(),
        "it fell back to an empty session, not a stale transcript"
    );
    assert_eq!(
        app.pane_session(focused).conversation.episode().id,
        a,
        "the other pane is untouched"
    );
}

/// The search for sessions sharing a delete's turns sees **every open
/// buffer**, not just the foreground one — or a clone in another pane is
/// orphaned rather than given its copy.
///
/// A clone is a `Slice` reference into the original's log, and an unpersisted
/// one is invisible to `load_graph` — so it can only be found by asking the
/// live episodes. Asking only the foreground covered one of them.
#[test]
fn a_clone_living_in_another_pane_keeps_its_history() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    // Clone it (lazy — the clone slices `original` and is not on disk), then
    // move the FOREGROUND elsewhere so only a background buffer holds it.
    app.clone_episode(original);
    let clone = app.conversation().episode().id;
    assert_ne!(clone, original, "cloned");

    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.split_command("output", SplitDir::Horizontal);
    app.open_by_hash(&original.to_hex()[..8]); // foreground → the original
    assert_eq!(
        app.pane_session(pane).conversation.episode().id,
        clone,
        "the clone still lives in the other pane"
    );

    let before = app.current_graph().full_turns(&clone);
    assert_eq!(app.delete_from_store(original), Ok(1), "found, and copied");
    assert_eq!(
        app.pane_session(pane).conversation.episode().own_turns(),
        before,
        "the pane's clone owns what it shared"
    );
    assert_eq!(on_disk(dir.path(), &clone), ["Hello 0", "Hello 1"]);
}

// --- leaving a session cuts every live wire ------------------------------
//
// Reviewer A killed `seal_foreground_turn` — replaced its whole body with
// `return;` — and all 691 tests passed. A guard whose deletion passes the suite
// is a top finding regardless of what it guards, and it is why the bugs below
// shipped in the first place.

/// A queued prompt whose session is **gone** is dropped, out loud (one whose
/// session lives goes to it: `a_queued_prompt_goes_to_the_session_it_was_typed_for`,
/// in `parallel.rs`) — in
/// the delete's own sentence, beside the session it was for and the turn
/// that went with it (Steven, 10-16: "always say *what* was deleted"). It
/// said "1 queued prompt dropped — the session they were for is gone",
/// naming none, and the turn said nothing.
#[tokio::test]
async fn a_queued_prompt_for_a_deleted_session_is_dropped_out_loud() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let session = app.conversation().episode().id;
    let name = app.session_label(session);
    app.submit("first");
    app.submit("second — queued behind it");
    assert_eq!(app.pending.len(), 1);

    app.delete_episodes(&[session]);

    assert!(app.pending.is_empty(), "not delivered anywhere");
    let notice = app.notice().expect("a silent drop is a lost prompt");
    assert!(
        notice.contains(&format!(
            "deleted {name} (with a turn in flight and 1 queued prompt)"
        )),
        "says so, of the session it was for: {notice}"
    );
    assert!(
        !notice.contains("is gone"),
        "once, not again in a sentence of its own: {notice}"
    );
}

/// **A delete names what went with its session, in its own sentence, once**
/// (Steven, 10-16: "consistency but in structure. We must always say *what*
/// was deleted"). Each kind of work a delete ends — a turn, a job, a
/// distillation, a continuation — is named after the session it ran on, and
/// says nothing of its own. The distillation said its session "changed";
/// the turn, the job and the continuation said nothing at all beside
/// "session deleted", which named no session.
#[tokio::test]
async fn a_delete_names_what_it_ended_in_its_own_sentence() {
    use shannon_core::{ContinueStrategy, StopReason};
    let said_once = |app: &App, name: &str, what: &str| {
        let said = app.notice().unwrap_or_default();
        assert!(
            said.starts_with(&format!("deleted {name} (with {what})")),
            "{what}, named after its session: {said}"
        );
        assert!(
            !said.contains("changed") && !said.contains("dropped"),
            "and nowhere else: {said}"
        );
    };

    // A turn.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let session = app.conversation().episode().id;
    let name = app.session_label(session);
    app.fake_stream();
    app.new_moment(); // the key that runs `:delete`
    app.delete_episodes(&[session]);
    said_once(&app, &name, "a turn in flight");

    // A job: a child, deleted while it answers.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("go", None).expect("spawned");
    let name = app.session_label(child);
    assert_eq!(app.job_episodes(), [child], "premise: its job runs");
    app.new_moment();
    app.delete_episodes(&[child]);
    said_once(&app, &name, "a turn in flight");

    // A distillation.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let session = app.conversation().episode().id;
    let name = app.session_label(session);
    let turn = app.conversation().episode().own_turns()[0];
    let target = SquashTarget::Gather {
        episode: session,
        turn,
        index: 0,
        config: app.conversation().episode().config.clone(),
    };
    let here = app.current;
    app.fake_summary_on(here, target, vec![turn]);
    app.new_moment();
    app.delete_episodes(&[session]);
    said_once(&app, &name, "a distillation");

    // A prefill continuation.
    let mut cfg = Config::default();
    cfg.continue_.strategy = ContinueStrategy::Prefill;
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.output_mut().begin_turn("q");
    app.conversation_mut().begin_turn("q");
    app.feed_stream(StreamMsg::Chunk("cut".into()));
    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);
    app.continue_last_turn();
    assert!(app.continuing(), "premise: a continuation streams");
    let session = app.conversation().episode().id;
    let name = app.session_label(session);
    app.new_moment();
    app.delete_episodes(&[session]);
    said_once(&app, &name, "a continuation");
}

/// **A turn delete names the turn and its session** — "deleted turn
/// 3f2a91ab from b1 …", as the question named them (Steven, 10-16). It said
/// "turn deleted", and no test held its wording.
#[test]
fn a_turn_delete_names_the_turn_and_its_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let episode = app.conversation().episode().id;
    let name = app.session_label(episode);
    let action = first_turn_delete(&app, episode);
    let ConfirmAction::Delete { turns, .. } = &action else {
        unreachable!("first_turn_delete builds a Delete")
    };
    let short = turns[0].turn.to_hex()[..8].to_string();
    app.run_confirmed(action);
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with(&format!("deleted turn {short} from {name}")),
        "{said}"
    );
}

/// **`:delete all` says that it worked.**
///
/// It emitted a message only on partial failure, so a clean prune of every
/// session on disk was silent — and `reset_foreground` then mints a fresh
/// session that reappears as a row in `:graph`. The screen therefore
/// contradicted the action, which is the exact bug `delete_episodes` one
/// function above documents fixing: *"Steven deleted the top row repeatedly,
/// watched a row stay there, and read it as a loop where the delete did
/// nothing."* That fix landed on the instance that was reported and not on the
/// class, and the member it missed is the most destructive command in the
/// product ([[feedback_class_not_instance]]).
#[test]
fn delete_all_reports_success_not_only_failure() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, _b, _sibling, _focused) = app_with_two_sessions(dir.path());
    assert!(
        app.store.list_episodes().is_ok_and(|ids| ids.contains(&a)),
        "premise: there is something on disk to delete"
    );
    app.notices.clear();

    app.delete_all_sessions(&[]);

    // Its own sentence, not a word of it: `contains("deleted")` matched "not
    // deleted", and "fresh empty session" matched "every buffer but theirs"
    // (review stop 2, C 10) — a partial refusal passed for a success.
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with("deleted all 2 stored sessions"),
        "a successful prune must say so: {said:?}"
    );
    assert!(
        said.ends_with("— every open session is now a fresh empty one"),
        "…and name the replacement, which is why a row is still on screen: {said:?}"
    );
    assert_ne!(app.notice_kind(), Some(NoticeKind::Refusal), "a success");
}

/// `:delete all` resets EVERY buffer. It reset only the foreground, leaving a
/// background pane rendering a session that is gone from disk.
#[test]
fn delete_all_resets_every_buffer_not_just_the_foreground() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());
    app.open_by_hash(&a.to_hex()[..8]);
    app.workspace.focus_pane(sibling);
    app.open_by_hash(&b.to_hex()[..8]);
    app.workspace.focus_pane(focused);
    app.open_by_hash(&a.to_hex()[..8]);

    app.delete_all_sessions(&[]);

    for pane in [focused, sibling] {
        let shown = app.pane_session(pane).conversation.episode().id;
        assert!(
            shown != a && shown != b,
            "no pane still shows a deleted session"
        );
        assert!(app.pane_session(pane).output.is_empty(), "and it is empty");
    }
}

/// **`:delete all` resets only what it deleted** (Steven, 10-16: "we
/// shouldn't refresh in the setting where we fail to delete. This will likely
/// confuse the user to making them believe the action was successful when it
/// wasn't"). A session the store would not delete stays, and so does the
/// buffer showing it; the rest go, in one sentence that names what did NOT.
#[test]
fn delete_all_keeps_the_buffers_of_what_it_could_not_delete() {
    let (mut app, refused, a, a_buffer, b, b_buffer) = two_buffers_on_a_store_refusing_deletes();
    refused.lock().unwrap().push(a);
    let a_name = app.session_label(a);
    app.notices.clear();

    app.delete_all_sessions(&[]);

    let shown = |app: &App, buffer| app.session_of(buffer).unwrap().conversation.episode().id;
    assert_eq!(shown(&app, a_buffer), a, "A's buffer keeps A");
    assert!(
        matches!(app.store.episode_len(&a), Ok(Some(_))),
        "and A is on disk"
    );
    assert_ne!(shown(&app, b_buffer), b, "B's buffer starts fresh");
    assert!(matches!(app.store.episode_len(&b), Ok(None)), "B is gone");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with(&format!(
            "deleted 1 of 2 stored sessions; not deleted: {a_name} ("
        )),
        "{said}"
    );
    assert!(
        said.ends_with("— every open session but theirs is now a fresh empty one"),
        "{said}"
    );
    assert_eq!(
        app.notice_kind(),
        Some(NoticeKind::Refusal),
        "part of it did not happen"
    );
}

/// **Every delete says it in one order, most important first** (Steven,
/// 10-16, D2): what was deleted, what was not and why, then what took their
/// place. `:delete` put the replacement before "not deleted" and `:delete
/// all` after it (review stop 2, C 7); both are `say_deleted` now. And all
/// refused is "— nothing was deleted" for both.
#[test]
fn a_partial_delete_says_what_went_what_stayed_then_what_replaced_it() {
    let (mut app, refused, a, _, b, _) = two_buffers_on_a_store_refusing_deletes();
    refused.lock().unwrap().push(b);
    let (a_name, b_name) = (app.session_label(a), app.session_label(b));
    app.notices.clear();
    app.delete_episodes(&[a, b]);
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with(&format!("deleted {a_name}; not deleted: {b_name} ("))
            && said.ends_with("— a fresh empty session took its place in its buffer"),
        "{said}"
    );
    assert_eq!(app.notice_kind(), Some(NoticeKind::Refusal));

    app.notices.clear();
    app.delete_episodes(&[b]);
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with(&format!("not deleted: {b_name} ("))
            && said.ends_with("— nothing was deleted"),
        "{said}"
    );
}

/// **A session `:delete all` could not delete keeps what it reads** (review
/// stop 2, A F4 · B 1 · C 9): C, a written clone of A with a turn of its own,
/// refused; A deleted. C kept its buffer whole while its log, read again,
/// came back one turn long — A's two gone with A. It is given its own copy
/// first, as every other delete gives it. And ONLY it: L, a clone of A open
/// and never written, is going too, and a copy would have written it.
///
/// Built until the store lists A before C — the order forks-first exists
/// for: ids are random and the reference store's listing is a map's, so in
/// the other order a batch in listed order passes too, and the test pinned
/// `forks_first` half the time.
#[test]
fn delete_all_keeps_what_a_refused_session_reads() {
    let built = || {
        let (store, refused) = shannon_core::store::FailingStore::refusing_deletes();
        let mut app =
            test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
        continue_with(&mut app, "A 0");
        continue_with(&mut app, "A 1");
        let a = app.conversation().episode().id;
        app.dispatch_command_for_test("clone");
        continue_with(&mut app, "C 0");
        let c = app.conversation().episode().id;
        (app, refused, a, c)
    };
    let source_first = |(app, _, a, c): &(App, _, EpisodeId, EpisodeId)| {
        app.store.list_episodes().ok() == Some(vec![*a, *c])
    };
    let (mut app, refused, a, c) = std::iter::repeat_with(built)
        .take(64)
        .find(source_first)
        .expect("premise: the store lists the source before its reader");
    app.set_focus(PaneKind::Output);
    app.split_command("output", SplitDir::Vertical);
    app.dispatch_command_for_test(&format!("open {}", a.to_hex()));
    app.dispatch_command_for_test("clone");
    let l = app.conversation().episode().id;
    assert!(a != c && c != l && l != a, "premise: three sessions");
    assert!(
        matches!(app.store.episode_len(&l), Ok(None)),
        "premise: L is never written"
    );
    refused.lock().unwrap().push(c);

    app.delete_all_sessions(&[]);

    assert_eq!(
        app.store.list_episodes().ok(),
        Some(vec![c]),
        "A gone, L unwritten"
    );
    let disk = app.store.load_graph().unwrap();
    assert_eq!(
        disk.full_turns(&c).len(),
        3,
        "C keeps A's two turns, and its own"
    );
}

/// **A session kept by `shannon-only` keeps what it reads too** — C, a
/// written clone of A with a turn of its own, kept; A deleted. A kept reader
/// is given its own copy as a refused one is: left out, C reloaded with 1 of
/// its 3 turns and nothing said (review stop 4, A2 2). A kept session is
/// skipped in either order, so no order is pinned.
#[test]
fn a_session_kept_from_delete_all_keeps_what_it_reads() {
    let (store, _) = shannon_core::store::FailingStore::refusing_deletes();
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    continue_with(&mut app, "A 0");
    continue_with(&mut app, "A 1");
    let a = app.conversation().episode().id;
    app.dispatch_command_for_test("clone");
    continue_with(&mut app, "C 0");
    let c = app.conversation().episode().id;
    assert_ne!(a, c, "premise: two sessions");

    app.delete_all_sessions(&[c]);

    assert_eq!(
        app.store.list_episodes().ok(),
        Some(vec![c]),
        "A gone, C kept"
    );
    let disk = app.store.load_graph().unwrap();
    assert_eq!(
        disk.full_turns(&c).len(),
        3,
        "C keeps A's two turns, and its own"
    );
}

/// **A `shannon-only` that deletes nothing names what it kept beside what it
/// could not** — the refusal said only the refused (review stop 4, A2 7).
#[test]
fn delete_all_keeping_some_that_deletes_nothing_names_both() {
    let (store, refused) = shannon_core::store::FailingStore::refusing_deletes();
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    continue_with(&mut app, "kept");
    let kept = app.conversation().episode().id;
    app.new_session_command();
    continue_with(&mut app, "refused");
    let a = app.conversation().episode().id;
    refused.lock().unwrap().push(a);

    app.delete_all_sessions(&[kept]);

    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with("not deleted: ")
            && said.contains("kept 1 session going on from Claude Code")
            && said.ends_with("— nothing was deleted"),
        "{said}"
    );
}

/// **…through a slice of a slice too** — X a clone of Y, and C slicing all
/// of X, Y's turns included (a slice that does not name their owner: what a
/// legacy branch migrates to; a projection names owners). Given its own copy
/// as X goes, C then slices Y itself (`Graph::detach` keeps a turn X did not
/// own a slice of its owner), so it is given one again as Y goes — found
/// only by reading the structure afresh after the first copy, since it is
/// read once for the batch (A2 3). C is open in no buffer: the store is all
/// that can say it reads Y.
#[test]
fn a_session_kept_from_delete_all_keeps_a_chain_it_reads() {
    let (store, _) = shannon_core::store::FailingStore::refusing_deletes();
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    continue_with(&mut app, "Y 0");
    continue_with(&mut app, "Y 1");
    app.dispatch_command_for_test("clone");
    continue_with(&mut app, "X 0");
    let x = app.conversation().episode().id;
    let config = app.conversation().episode().config.clone();
    let c = EpisodeId::from_seed(b"a slice of a slice");
    app.store
        .put_episode(&shannon_core::Episode::root(c, config))
        .unwrap();
    app.store.append_slice(&c, &x, 0, 3).unwrap();
    assert_eq!(
        app.store.load_graph().unwrap().full_turns(&c).len(),
        3,
        "premise: C reads Y's two turns through X"
    );

    app.delete_all_sessions(&[c]);

    assert_eq!(
        app.store.list_episodes().ok(),
        Some(vec![c]),
        "Y and X gone"
    );
    let disk = app.store.load_graph().unwrap();
    assert_eq!(disk.full_turns(&c).len(), 3, "C keeps the chain");
}

/// **`:delete all shannon-only` reads the store's structure once, not once
/// per session it deletes** — it did, and that cost 3.7 s at 160 sessions on
/// the most destructive command's UI thread (review stop 4, A2 3). Counted
/// at two sizes, so a read elsewhere does not move it.
#[test]
fn delete_all_keeping_some_reads_the_store_alike_at_any_size() {
    use std::sync::atomic::Ordering::SeqCst;
    let loads_deleting = |n: usize| {
        let (store, _) = shannon_core::store::FailingStore::refusing_deletes();
        let loads = store.structure_loads();
        let mut app =
            test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
        continue_with(&mut app, "kept");
        let kept = app.conversation().episode().id;
        for i in 0..n {
            app.new_session_command();
            continue_with(&mut app, &format!("S{i}"));
        }
        assert_eq!(app.store.list_episodes().unwrap().len(), n + 1, "premise");
        let before = loads.load(SeqCst);
        app.delete_all_sessions(&[kept]);
        assert_eq!(app.store.list_episodes().unwrap(), vec![kept], "premise");
        loads.load(SeqCst) - before
    };
    assert_eq!(loads_deleting(6), loads_deleting(3));
}

/// **…and resets nothing when it deleted nothing** — every buffer as it was,
/// the work in it still running, and the refusal says nothing was deleted.
/// It reset every buffer regardless, so a fresh session in front read as a
/// delete that had happened.
#[tokio::test]
async fn delete_all_that_deletes_nothing_resets_nothing() {
    let (mut app, refused, a, a_buffer, b, b_buffer) = two_buffers_on_a_store_refusing_deletes();
    refused.lock().unwrap().extend([a, b]);
    app.fake_stream();
    app.notices.clear();

    app.delete_all_sessions(&[]);

    let shown = |app: &App, buffer| app.session_of(buffer).unwrap().conversation.episode().id;
    assert_eq!(
        (shown(&app, a_buffer), shown(&app, b_buffer)),
        (a, b),
        "both kept"
    );
    assert!(app.turns().next().is_some(), "and the turn still runs");
    let said = app.notice().unwrap_or_default();
    assert!(said.starts_with("not deleted: "), "{said}");
    assert!(said.ends_with("— nothing was deleted"), "{said}");
}

/// **…and nothing when the store cannot be listed.** It refused "could not
/// list sessions to delete" and reset every buffer anyway.
#[cfg(unix)]
#[tokio::test]
async fn delete_all_that_cannot_list_resets_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let session = app.conversation().episode().id;
    app.fake_stream();
    let episodes = dir.path().join("episodes");
    let mode = |m| std::fs::set_permissions(&episodes, std::fs::Permissions::from_mode(m));
    app.notices.clear();
    mode(0o000).unwrap();
    app.delete_all_sessions(&[]);
    mode(0o700).unwrap();

    assert_eq!(
        app.conversation().episode().id,
        session,
        "the buffer keeps it"
    );
    assert!(app.turns().next().is_some(), "and its turn still runs");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with("could not list sessions to delete: "),
        "{said}"
    );
    assert!(said.ends_with("— nothing deleted"), "{said}");
}

/// **`:delete all` counts sessions, not buffers** — with the manual open, a
/// page in the list, no session stays, and the notice says every one is
/// fresh, not "every … but theirs": the page is a buffer, and no session
/// (V5.4 (3); the design review's L7 — a count of every buffer would still
/// compile there).
#[test]
fn delete_all_counts_sessions_not_pages() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    app.open_help_command(None);
    assert!(
        app.page_of_kind(PaneKind::Page(PageKind::Help)).is_some(),
        "premise: a page"
    );
    app.notices.clear();

    app.delete_all_sessions(&[]);

    let said = app.notice().unwrap_or_default();
    assert!(
        said.ends_with("— every open session is now a fresh empty one"),
        "{said}"
    );
}

/// **A session never written goes with `:delete all`, and is named** — its
/// buffer is its only copy, so resetting it is deleting it.
#[test]
fn delete_all_names_a_session_never_written() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let source = app.conversation().episode().id;
    app.clone_episode(source);
    let name = app.session_label(app.conversation().episode().id);
    assert_eq!(
        app.buffer().on_disk,
        OnDisk::Absent,
        "premise: never written"
    );
    app.notices.clear();

    app.delete_all_sessions(&[]);

    assert_eq!(
        app.notice().unwrap_or_default(),
        format!(
            "deleted the one stored session and {name}, never written — every open \
             session is now a fresh empty one"
        )
    );
}

/// **A delete that would rebuild a session with work running on it is
/// refused before it is asked** (Steven, 10-16: "refuse while it runs";
/// review stop 1, Reviewer A F1, its probes P8a and P8c).
///
/// A turn deleted while its session streamed: the rewrite ran first, the
/// supersede seal then appended the partial, and the buffer adopted turns
/// computed before it — memory and disk disagreed, and the partial came back
/// mid-history on relaunch. A session deleted while its lazy clone streamed:
/// the clone, rebuilt to own its copy, was registered a second time.
#[tokio::test]
async fn a_delete_that_would_rebuild_a_running_session_is_refused() {
    // The turn's own session is streaming.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let episode = app.conversation().episode().id;
    app.submit("a fourth, streaming");
    assert!(app.streaming(), "premise: a turn is in flight");
    app.notices.clear();
    app.request_delete(first_turn_delete(&app, episode));
    assert!(app.pending_confirm.is_empty(), "refused, not asked");
    assert!(app.streaming(), "the turn runs on");
    assert_eq!(
        app.conversation().episode().own_turns().len(),
        3,
        "nothing cut"
    );
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("not deleted:") && said.contains("a turn is in flight"),
        "{said}"
    );

    // A clone of the session being deleted is streaming.
    let dir = TempDir::new().unwrap();
    let (mut app, a, _, _, _) = app_with_two_sessions(dir.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (here, there) = (panes[0], panes[1]);
    app.focus_pane(here);
    app.open_by_hash(&a.to_hex()[..8]);
    app.focus_pane(there);
    app.clone_by_hash(&a.to_hex()[..8]);
    let clone = app.pane_session(there).conversation.episode().id;
    assert_ne!(clone, a, "premise: a clone, beside A");
    app.submit("the clone, streaming");
    assert!(app.streaming(), "premise: the clone's turn is in flight");
    app.notices.clear();
    app.request_delete(ConfirmAction::sessions(vec![a]));
    assert!(app.pending_confirm.is_empty(), "refused, not asked");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("not deleted:") && said.contains(&app.session_label(clone)),
        "names the clone it would rebuild: {said}"
    );
    assert!(
        said.contains("stop it (CTRL-C) or wait"),
        "aimed at the clone, CTRL-C here stops it: {said}"
    );
    assert!(
        app.store.load_graph().unwrap().episode(&a).is_some(),
        "A is still there"
    );
    // Asked from A's pane, the clone's work is not where the prompt goes:
    // its CTRL-C is pressed in its session (§9 phase 6b).
    app.focus_pane(here);
    app.notices.clear();
    app.request_delete(ConfirmAction::sessions(vec![a]));
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("CTRL-C in its session"), "{said}");

    // Deleted WITH its streaming clone: the clone is not rebuilt — it goes —
    // so its work ends with it, as any deleted session's does. Asked.
    app.request_delete(ConfirmAction::sessions(vec![a, clone]));
    assert_eq!(
        app.pending_confirm.len(),
        1,
        "a clone deleted alongside is not one the delete rebuilds: {:?}",
        app.notice()
    );
}

/// **…and refused at the answer, when the work started while the question
/// was up.** Asked with nothing running on A; then a prompt for A starts —
/// and `y` would have rebuilt A under it. (The prompt was queued behind B's
/// turn and drained when it ended, while one turn ran app-wide; sessions
/// run apart now, so it starts at once — the same hazard, sooner.)
#[tokio::test]
async fn a_delete_refuses_at_the_answer_when_work_started_meanwhile() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, _, _, _) = app_with_two_sessions(dir.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    app.focus_pane(panes[0]);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;

    app.request_delete(first_turn_delete(&app, a));
    assert_eq!(app.pending_confirm.len(), 1, "premise: asked — A was idle");
    app.submit_to(a_buffer, "for A");
    assert!(
        app.generations.iter().any(|g| g.buffer == a_buffer),
        "premise: A's prompt started while the question was up"
    );
    let before = app
        .session_of(a_buffer)
        .map(|b| b.conversation.episode().own_turns().len());

    type_keys(&mut app, "y");

    let said = app.notice().unwrap_or_default();
    assert!(said.contains("not deleted:"), "{said}");
    assert_eq!(
        app.session_of(a_buffer)
            .map(|b| b.conversation.episode().own_turns().len()),
        before,
        "nothing cut"
    );
    assert!(
        app.generations.iter().any(|g| g.buffer == a_buffer),
        "A's turn runs on"
    );
}

/// **Deleting the live session says a fresh one replaced it.**
///
/// The graph overlays live conversations, so the top row is the in-memory
/// session; deleting it resets that buffer to a *fresh* session, which
/// reappears as a row. Every message on the delete path was a failure message —
/// success was silent — so the screen contradicted the action and Steven read
/// it as a loop where `d` "did nothing" (`[[feedback_no_silent_failure]]`).
#[test]
fn deleting_the_live_session_explains_the_new_one() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let live = app.conversation().episode().id;

    app.run_confirmed(ConfirmAction::sessions(vec![live]));

    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("deleted"),
        "a successful delete must say so: {notice:?}"
    );
    assert!(
        notice.contains("fresh"),
        "…and must explain the row that took its place, or it reads as a no-op: \
         {notice:?}"
    );
    assert_ne!(
        app.conversation().episode().id,
        live,
        "premise: the pane really is on a new session"
    );
}

/// **The protective question must not be dressed as a threat.**
///
/// `VaultPaste` is the inverted member: `y` keeps a credential off the wire and
/// declining transmits it verbatim. `yes_is_safe`'s doc comment claimed the
/// door was already closed — "one row, one key set and one red style … the
/// polarity lives on the action so every surface asks the type" — while the
/// style went on painting all five black-on-red. The comment was the
/// hypothesis; this is the test.
///
/// It asserts the **content** of the rung table, not the derivation from it.
/// `yes_is_safe()` now reads `alarm()`, so checking that the two agree would be
/// checking that `==` works. What can still be wrong is the judgement: which
/// action sits on which rung.
///
/// Completeness is the compiler's job, not this list's — `alarm()` matches
/// exhaustively with no `_` arm, so a new variant cannot reach a rung without
/// someone naming it. (`ApplySummary` is gone: a summary's review is a
/// decision since A2.2, not a question.)
#[test]
fn a_safe_yes_is_never_drawn_as_an_alarm() {
    let app = test_app();
    let episode = app.conversation().episode().id;
    let vault = ConfirmAction::VaultPaste {
        text: "sk-xxxx".into(),
        kind: "github token",
    };
    let spawn = ConfirmAction::SpawnChild {
        briefing: "hi".into(),
        model: None,
        reason: "spawns deep".into(),
    };
    let dangerous = [
        ConfirmAction::DeleteAll,
        ConfirmAction::sessions(vec![episode]),
        spawn.clone(),
    ];

    assert_eq!(
        vault.alarm(),
        Alarm::Safe,
        "the one question whose `y` protects you is not an alarm"
    );
    assert_eq!(vault.keys(), "(Y/n)", "and its keys say so");
    assert!(
        vault.alarm().tag().is_none(),
        "a protective question never wears the warning sign"
    );

    for action in &dangerous {
        assert_ne!(
            action.alarm(),
            Alarm::Safe,
            "a consequential action must not read as a friendly question: \
             {action:?}"
        );
        assert_eq!(action.keys(), "(y/N)", "{action:?}");
    }

    // **The sign tracks loss, not consequence.** Spawning a child is worth a
    // question and costs nothing permanent, so warning about it would be
    // crying wolf; both deletes take something that does not come back.
    assert!(ConfirmAction::DeleteAll.alarm().tag().is_some());
    assert!(
        ConfirmAction::sessions(vec![episode])
            .alarm()
            .tag()
            .is_some()
    );
    assert!(
        spawn.alarm().tag().is_none(),
        "a recoverable change carries no warning"
    );

    // **And as drawn** — the role is the app's; the paint is the view's, and
    // painting the safe rung red (`Alarm::Safe => Ink::Danger`) passed with
    // every role above intact (review stop 12, T3 4). The question a pasted
    // token raises, on screen: its verb in the question's ink, no sign.
    use crate::style::Ink;
    let mut app = test_app();
    app.handle_paste("ghp_abcdefghijklmnopqrstuvwxyz0123456789");
    assert!(app.confirm_prompt().is_some(), "premise: asked");
    let (question, danger) = (
        app.theme().ui(Ink::Question).fg,
        app.theme().ui(Ink::Danger).fg,
    );
    assert_ne!(question, danger, "premise: the two inks can be told apart");
    let buf = frame_of(&mut app, 160, 20);
    let rows: Vec<String> = (0..buf.area().height)
        .map(|y| {
            (0..buf.area().width)
                .map(|x| buf[(x, y)].symbol())
                .collect()
        })
        .collect();
    let verb = "keep it off the wire";
    let y = (rows.iter().position(|r| r.contains(verb)))
        .unwrap_or_else(|| panic!("the question drawn: {rows:#?}"));
    let x = rows[y][..rows[y].find(verb).unwrap()].chars().count() as u16;
    assert_eq!(
        buf[(x, y as u16)].style().fg,
        question,
        "the verb is a question's"
    );
    assert!(
        !rows[y].contains("WARNING"),
        "and wears no sign: {}",
        rows[y]
    );
}

/// Both deletes warn; only the unbounded one shouts.
///
/// Both are unrecoverable, so both are red and both carry the sign — the hue
/// cannot separate them and should not try. What separates them is scope: one
/// session is named and checkable against the screen, the whole store is
/// neither. So the *word* is the same and the volume is not, which is what
/// keeps a preface on every delete from degrading into wallpaper.
///
/// The property is the **difference**. Asserting only "the prune is loud"
/// would survive making every rung loud, which is the uniform alarm this
/// whole line of work replaced.
#[test]
fn both_deletes_warn_but_only_the_unbounded_one_shouts() {
    let mut app = test_app();
    let episode = app.conversation().episode().id;

    app.ask(ConfirmAction::sessions(vec![episode]));
    let one = app.confirm_prompt().expect("a question is up");
    app.pending_confirm.clear();
    app.ask(ConfirmAction::DeleteAll);
    let all = app.confirm_prompt().expect("a question is up");

    assert_ne!(
        one.alarm, all.alarm,
        "deleting one session must not look like deleting every session"
    );
    assert_eq!(one.alarm, Alarm::Destructive);
    assert_eq!(all.alarm, Alarm::Grave);

    // Same word on both, so severity never needs a second vocabulary…
    assert!(
        one.text().starts_with("WARNING:"),
        "an unrecoverable delete says so in the text, not only in colour: {one}"
    );
    assert!(
        all.text().starts_with("WARNING:"),
        "the sign reaches the text the user reads, not just the styling: {all}"
    );
    // …and the escalation is how it is drawn.
    assert!(
        !one.alarm.sign_is_loud(),
        "the routine delete gets red text, not a solid block: {one}"
    );
    assert!(
        all.alarm.sign_is_loud(),
        "the block is spent on the case the screen cannot verify: {all}"
    );

    // **And as drawn** — `sign_is_loud` is the app's role; the paint is the
    // view's, and drawing the grave tag in the routine one's bold red passed
    // with every assertion above intact (review stop 12, T7 11). Each delete
    // asked and drawn; the tag's first cell read.
    use crate::style::Ink;
    use ratatui::style::Modifier;
    let tag_style = |action: ConfirmAction| {
        let mut app = test_app();
        app.ask(action);
        let buf = frame_of(&mut app, 160, 20);
        let at = cell_of(&buf, "WARNING:").expect("the tag, drawn");
        (
            buf[at].style(),
            app.theme().ui(Ink::DangerSign),
            app.theme().ui(Ink::Danger),
        )
    };
    let (one, sign, danger) = tag_style(ConfirmAction::sessions(vec![episode]));
    let (all, ..) = tag_style(ConfirmAction::DeleteAll);
    assert_ne!(
        sign.bg, danger.bg,
        "premise: the sign's fill is not the text's"
    );
    assert_eq!(
        (all.fg, all.bg),
        (sign.fg, sign.bg),
        "the prune's tag a solid block"
    );
    // Its background the row's, whatever that is: not the sign's fill.
    assert_eq!(one.fg, danger.fg, "the routine delete's red text");
    assert_ne!(one.bg, sign.bg, "and no block behind it");
    assert!(one.add_modifier.contains(Modifier::BOLD), "bold");
}

/// **Every confirmation names both the *what* and the *where*.**
///
/// Marking the operand alone tells the user where something is about to
/// happen and leaves the verb in plain prose — the state this replaced, where
/// `delete session 9a8b7c6d permanently?` picked out the id and let the word
/// `delete` sit unstyled (Steven, drive round 4). The failure is quiet: the
/// row still looks marked up, so nothing on screen says the action word was
/// never identified.
///
/// Asserting both roles *exactly* is what keeps a later edit from marking the
/// whole line, which would flatten the row back into the uniform loudness the
/// rungs exist to avoid.
#[test]
fn a_confirmation_names_both_the_action_and_its_target() {
    let mut app = test_app();
    let episode = app.conversation().episode().id;
    app.ask(ConfirmAction::sessions(vec![episode]));
    let prompt = app.confirm_prompt().expect("a question is up");

    let of = |want: Mark| -> Vec<&str> {
        prompt
            .runs
            .iter()
            .filter(|run| run.mark == want)
            .map(|run| run.text.as_str())
            .collect()
    };
    assert_eq!(
        of(Mark::Action),
        vec!["delete"],
        "the verb carries the danger hue, so it has to be its own run: {prompt}"
    );
    // The session, named as `:ls` names it — the buffer AND the id, the two
    // names the screen can be checked against — and nothing else.
    let label = format!("b1 {}", &episode.to_hex()[..8]);
    assert_eq!(
        of(Mark::Target),
        vec![label.as_str()],
        "the session, and only the session, is the target: {prompt}"
    );
    assert!(
        prompt.tail.contains("(y/N)"),
        "the answer keys live in the reserved tail, never in the elidable \
         question: {prompt}"
    );
}

/// **Unrecoverable is red whether or not it is routine.**
///
/// The first cut ranked purely by rarity, so a permanent session delete —
/// common, and the single most destructive thing `d` does — shared a rung with
/// spawning a child, and came out the same yellow. Frequency is the tag's
/// axis; the hue's axis is whether it comes back.
#[test]
fn deleting_outranks_a_merely_consequential_change() {
    let app = test_app();
    let episode = app.conversation().episode().id;

    let delete = ConfirmAction::sessions(vec![episode]);
    let spawn = ConfirmAction::SpawnChild {
        briefing: "hi".into(),
        model: None,
        reason: "spawns deep".into(),
    };
    assert_eq!(delete.alarm(), Alarm::Destructive);
    assert_eq!(spawn.alarm(), Alarm::Caution);
    assert_ne!(
        delete.alarm(),
        spawn.alarm(),
        "losing a session must not look like starting a child"
    );
    // …and the sign follows loss, not consequence: the delete wears it, the
    // spawn does not, and only the unbounded delete shouts it.
    assert!(delete.alarm().tag().is_some());
    assert!(!delete.alarm().sign_is_loud());
    assert!(spawn.alarm().tag().is_none());
    assert!(ConfirmAction::DeleteAll.alarm().sign_is_loud());
}

/// **Asserted on rendered cells, because the colour *is* the feature.**
///
/// `Mark::Action` and `Mark::Target` are app-side facts; a style is what the
/// user gets. Testing only the marks would leave the mapping — the half that
/// reaches the screen — free to paint both roles identically and still pass,
/// which is exactly how the row shipped with `delete` unstyled while the id
/// beside it was picked out.
#[test]
fn the_rendered_row_colours_the_verb_and_marks_its_target() {
    use ratatui::style::{Color, Modifier};

    let mut app = test_app();
    let episode = app.conversation().episode().id;
    let short = episode.to_hex()[..8].to_string();
    app.ask(ConfirmAction::sessions(vec![episode]));

    let buf = &frame_of(&mut app, 100, 12);
    let y = buf.area().height - 1;
    let row: String = (0..buf.area().width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect();
    let style_at = |needle: &str| {
        let col = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} is not on the row: {row:?}"));
        buf[(col as u16, y)].style()
    };

    let verb = style_at("delete");
    let target = style_at(&short);
    let prose = style_at(" permanently?");

    assert_eq!(
        verb.fg,
        Some(Color::Red),
        "an unrecoverable action is red however routine it is: {row:?}"
    );
    assert!(
        verb.add_modifier.contains(Modifier::BOLD),
        "…and carries weight, so it survives a low-contrast theme: {row:?}"
    );
    assert_ne!(
        target.fg, verb.fg,
        "*what* and *where* must be told apart at a glance, or marking both \
         is the same as marking neither: {row:?}"
    );
    assert!(
        target.add_modifier.contains(Modifier::BOLD),
        "the id is still emphasised, just differently: {row:?}"
    );
    // **Weight alone is not a mark.** The first cut gave the id white bold
    // against near-white prose, which reads as ordinary text however bold it
    // is (Steven, drive round 5). A hue, and specifically not one of the
    // neutrals, is what makes it findable — colour distance is not something a
    // test can judge, but "did someone put it back to a grey" is.
    assert!(
        !matches!(
            target.fg,
            Some(Color::White | Color::Gray | Color::DarkGray | Color::Reset) | None
        ),
        "the target needs a hue, not just weight: {row:?}"
    );
    assert!(
        prose.fg != verb.fg && prose.fg != target.fg,
        "the grammar around them stays quiet, or nothing stands out: {row:?}"
    );

    // Every unrecoverable action says so in words, and this one is the routine
    // delete, so the sign is red text rather than a filled block.
    let sign = style_at("WARNING:");
    assert_eq!(sign.fg, Some(Color::Red), "{row:?}");
    assert!(
        sign.bg.is_none() || sign.bg == Some(Color::Reset),
        "the solid block is reserved for the unbounded delete: {row:?}"
    );
}

/// **A trailing turn deletes with no warning, and really leaves.**
///
/// The quiet end of the rule: nothing follows the last turn, so nothing can be
/// left answering a question that is gone. It must reach `Alarm::Destructive`
/// (red, unrecoverable) and *not* `Grave` — if every turn delete shouted, the
/// gap warning would carry no information, which is the whole reason the rungs
/// exist.
///
/// It also asserts the turn is gone **from the store**, not merely from the
/// in-memory graph: a removal that lives only in memory comes back on the next
/// load, which reads as the delete having silently failed
/// ([[feedback_wiring_is_untested]]).
#[test]
fn deleting_the_last_turn_is_quiet_and_persists() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let episode = app.conversation().episode().id;
    let before = app.current_graph().full_turns(&episode);

    open_graph(&mut app);
    app.graph_view_mut()
        .unwrap()
        .reveal_and_select_episode(episode);
    app.graph_key_for_test(k(KeyCode::Char('l')));
    for _ in 0..3 {
        app.graph_key_for_test(k(KeyCode::Char('j')));
    }
    let Some(Node::Turn { index, .. }) = app.graph_view().and_then(GraphView::selected_node) else {
        panic!("premise: the cursor is on a turn row");
    };
    assert_eq!(index, 2, "premise: it is the LAST turn");

    app.graph_key_for_test(k(KeyCode::Char('d')));
    let action = app.pending_confirm.front().cloned().expect("a question");
    assert_eq!(
        action.alarm(),
        Alarm::Destructive,
        "unrecoverable, but nothing is left dangling — no sign escalation"
    );
    assert!(
        !app.confirm_prompt()
            .expect("a question")
            .text()
            .contains("gone"),
        "and no gap clause, because there is no gap"
    );

    app.run_confirmed(action);
    assert_eq!(
        app.current_graph().full_turns(&episode),
        before[..2].to_vec(),
        "the tail turn is gone from the graph"
    );

    // Reload from disk: the store is the only thing that survives the process.
    let reloaded = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    assert_eq!(
        reloaded.full_turns(&episode),
        before[..2].to_vec(),
        "…and from the log, or it returns on the next start"
    );
}

/// **After deleting a turn of a branch, the next turn still reaches disk.**
///
/// The persist cursor counts the episode's OWN turns on disk; the delete
/// re-adopted the buffer with the full derived length instead. On a root the
/// two agree, which is why the test above never saw it. On a branch (here two
/// shared turns and two of its own) the cursor sat past the end of what it
/// owned, so every later turn answered "up to date" and stayed in memory only
/// — lost at exit with nothing on screen (Reviewer A, round 18).
#[test]
fn deleting_a_branch_turn_keeps_persisting_what_follows() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1);
    for word in ["own a", "own b"] {
        app.conversation_mut().begin_turn(word);
        app.conversation_mut().push_chunk("ok");
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }
    let branch = app.conversation().episode().id;
    assert_eq!(app.current_graph().full_turns(&branch).len(), 4, "premise");

    app.remove_turns_from(branch, &[3]).expect("deletes");
    app.conversation_mut().begin_turn("after the delete");
    app.conversation_mut().push_chunk("ok");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();

    let disk = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    let words: Vec<String> = disk
        .records(&branch)
        .expect("a full load holds every body")
        .iter()
        .map(|t| t.messages[0].content.clone())
        .collect();
    assert_eq!(
        words,
        ["Hello 0", "Hello 1", "own a", "after the delete"],
        "what memory shows is what the log holds"
    );
}

/// **A turn deleted above a reader leaves it on what it read** (review stop
/// 15, A L8) — the session rebuilt in place, every window on it put back
/// by position through the delete's map, the later turns moved up: it went
/// to its end, as no other rebuild of a session being read does.
#[tokio::test]
async fn a_turn_deleted_above_a_reader_leaves_it_where_it_was() {
    use super::position_marks::{SIZE, line_reading, over_store};
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    for i in 0..10 {
        app.submit(&format!("ask {i}"));
        app.feed_stream(StreamMsg::Chunk(format!("reply {i}")));
        app.feed_stream(StreamMsg::Complete);
    }
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    let line = line_reading(&app, pane, "reply 7");
    // At the pane's own size (review stop 15, A L10): typed at the frame's,
    // `G` saw the line on screen and the frame then dragged the cursor.
    let frame = Rect::new(0, 0, SIZE.0, SIZE.1);
    type_keys_in(&mut app, frame, &format!("{}G", line + 1));
    frame_of(&mut app, SIZE.0, SIZE.1);
    let reading = |app: &App| {
        let (at, _) = app.pane_chat(pane).cursor();
        app.pane_chat(pane).line_text(at)
    };
    assert_eq!(
        reading(&app).as_deref(),
        Some("reply 7"),
        "premise: reading it"
    );
    let episode = app.conversation().episode().id;
    app.remove_turns_from(episode, &[2]).expect("deletes");
    let lines = app.output().lines();
    assert!(!lines.iter().any(|l| l == "reply 2"), "premise: deleted");
    assert_eq!(
        reading(&app).as_deref(),
        Some("reply 7"),
        "{:?}",
        app.notice()
    );
}

/// **A turn a clone reads is deleted from its session, and the clone keeps
/// it** — never quietly shifted, and never refused.
///
/// A `Slice` names a *range of positions* in this episode, so removing an
/// earlier one would leave the clone resolving the same range over a shifted
/// sequence: the same length, silently different content. That was refused
/// outright, and a forked conversation could not be cut at all (Steven,
/// 10-04). The clone is given its own copy first, so it reads what it read —
/// in memory, and on disk where a restart will look.
#[test]
fn a_turn_a_clone_reaches_past_is_deleted_and_the_clone_keeps_it() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let episode = app.conversation().episode().id;

    // A clone shares the whole history by reference — a Slice reaching to 3.
    app.clone_episode(episode);
    let clone = app.conversation().episode().id;
    let before = app.current_graph().full_turns(&episode);
    assert_eq!(
        app.slice_holders(episode, 0),
        Some(vec![clone]),
        "premise: the clone really does slice this session"
    );

    open_graph(&mut app);
    app.graph_view_mut()
        .unwrap()
        .reveal_and_select_episode(episode);
    app.graph_key_for_test(k(KeyCode::Char('l')));
    app.graph_key_for_test(k(KeyCode::Char('j')));
    app.graph_key_for_test(k(KeyCode::Char('d')));
    type_keys(&mut app, "y");

    let notice = app.notice().unwrap_or_default();
    assert!(notice.contains("keeps its own copy"), "{notice:?}");
    let graph = app.current_graph();
    assert_eq!(graph.full_turns(&episode), before[1..], "the turn went");
    assert_eq!(
        graph.full_turns(&clone),
        before,
        "the clone reads what it read"
    );
    let disk = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    assert_eq!(disk.full_turns(&clone), before, "and so does a restart");
}

/// **A mixed selection deletes both units and says so.**
///
/// Marking a session and a turn in a *different* session is one keystroke, and
/// it used to be refused outright for containing a turn row. The confirmation
/// has to name both counts: approving "1 session" while a turn rides along
/// invisibly is the same defect as the count that disagreed with the marks.
#[test]
fn a_mixed_selection_names_both_units() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let live = app.conversation().episode().id;
    // A second session to delete whole, so the selection spans both units.
    app.clone_episode(live);
    let other = app.conversation().episode().id;
    assert_ne!(other, live, "premise: two distinct sessions");

    let action = ConfirmAction::Delete {
        episodes: vec![other],
        turns: vec![TurnTarget {
            episode: live,
            index: 2,
            turn: app.current_graph().full_turns(&live)[2],
            of_total: 3,
        }],
        leaves_gap: false,
        claude_left: 0,
        from: None,
    };
    app.ask(action);
    let text = app.confirm_prompt().expect("a question").text();
    assert!(
        text.contains("1 session") && text.contains("1 turn"),
        "both units are counted, singular: {text:?}"
    );
    assert!(
        text.contains(&other.to_hex()[..8]),
        "and the session is named so the screen can be checked: {text:?}"
    );
}

/// **A turn inside a session that is also going is not counted twice.**
///
/// Otherwise the prompt claims one more deletion than happens, and the removal
/// path rewrites a log that is about to be unlinked.
#[test]
fn a_turn_in_a_doomed_session_drops_out_of_the_count() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let episode = app.conversation().episode().id;

    open_graph(&mut app);
    app.graph_view_mut()
        .unwrap()
        .reveal_and_select_episode(episode);
    // Mark the session row, expand, and mark a turn inside it too.
    app.graph_key_for_test(k(KeyCode::Char('m')));
    app.graph_key_for_test(k(KeyCode::Char('f')));
    app.graph_key_for_test(k(KeyCode::Char('l')));
    app.graph_key_for_test(k(KeyCode::Char('j')));
    app.graph_key_for_test(k(KeyCode::Char('m')));
    app.graph_key_for_test(k(KeyCode::Char('f')));

    app.graph_key_for_test(k(KeyCode::Char('d')));
    match app.pending_confirm.front() {
        Some(ConfirmAction::Delete {
            episodes, turns, ..
        }) => {
            assert_eq!(episodes.as_slice(), &[episode]);
            assert!(
                turns.is_empty(),
                "the turn is inside a session already going: {turns:?}"
            );
        }
        other => panic!("expected a delete, got {other:?}"),
    }
}

/// **"turn 2" and "2 turns" must not be the same shape.**
///
/// The singular prompt read `delete turn 2 of session …` and the plural read
/// `delete 2 turns permanently from …`. One names a *position*, the other a
/// *count*, and they differ only by word order — at the speed someone answers
/// a y/N they are indistinguishable, so the swap gets glossed over (Steven,
/// drive round 6). The fix is structural rather than cosmetic: the singular
/// leads with the turn's own short id, so the first thing after the verb is a
/// hash where the plural has a digit.
///
/// The position is still there, in the inspector's `2/3` notation — the pane
/// beside the graph says the same thing the same way, so the row can be
/// checked against the screen rather than counted.
#[test]
fn a_single_turn_and_a_count_of_turns_cannot_be_misread() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let episode = app.conversation().episode().id;
    let turns = app.current_graph().full_turns(&episode);

    // The colliding pair: ONE turn at position 2, and TWO turns.
    let at = |index: usize| TurnTarget {
        episode,
        index,
        turn: turns[index],
        of_total: turns.len(),
    };
    app.ask(ConfirmAction::Delete {
        episodes: Vec::new(),
        turns: vec![at(1)],
        leaves_gap: false,
        claude_left: 0,
        from: None,
    });
    let one = app.confirm_prompt().expect("a question").text();
    app.pending_confirm.clear();
    app.ask(ConfirmAction::Delete {
        episodes: Vec::new(),
        turns: vec![at(0), at(1)],
        leaves_gap: false,
        claude_left: 0,
        from: None,
    });
    let many = app.confirm_prompt().expect("a question").text();

    // Premise: the plural really does put a count next to the word, or there
    // is no collision to avoid and this test is measuring nothing.
    assert!(
        many.contains("2 turns"),
        "premise: the plural reads as a count: {many:?}"
    );
    assert!(
        !one.contains("turn 2"),
        "the singular must not read as a count with the words swapped: {one:?}"
    );
    assert!(
        one.contains(&turns[1].to_hex()[..8]),
        "it names the turn by the id the graph row shows: {one:?}"
    );
    assert!(
        one.contains("(2/3)"),
        "and keeps the position, in the notation the inspector already uses: \
         {one:?}"
    );
}

/// **A delete removes the turn it asked about, or none** — its position
/// names a place, and a turn removed above between the question and the
/// answer put ANOTHER turn there: the answer deleted turn 3 for a question
/// about turn 2. Unreachable while every renumbering change is work the
/// answer refuses; held closed regardless, said, and before the marks are
/// spent, so they are there to retry with (V4 verify, L3).
#[test]
fn a_delete_whose_turn_moved_since_the_question_deletes_nothing() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 4);
    let episode = app.conversation().episode().id;
    let turns = app.current_graph().full_turns(&episode);
    let asked = TurnTarget {
        episode,
        index: 2,
        turn: turns[2],
        of_total: turns.len(),
    };
    open_graph(&mut app);
    let tree = app.workspace.focused();
    let marked = [Node::Turn {
        episode,
        turn: turns[2],
        index: 2,
    }];
    assert_eq!(
        app.graph_view_mut().unwrap().mark_nodes(&marked),
        1,
        "premise"
    );
    app.remove_turns_from(episode, &[0])
        .expect("premise: removed");
    let before = app.current_graph().full_turns(&episode);
    assert_eq!(before, turns[1..], "premise: turn 3 now in turn 2's place");
    let held = app.pane_graph(tree).unwrap().marked_nodes();
    assert!(!held.is_empty(), "premise: the mark is held");
    app.run_delete(&[], &[asked], Some(tree));
    assert_eq!(
        app.current_graph().full_turns(&episode),
        before,
        "nothing deleted"
    );
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("changed since you were asked"), "{said}");
    assert_eq!(
        app.pane_graph(tree).unwrap().marked_nodes(),
        held,
        "the marks kept"
    );
}

/// The gap clause is styled as the WARNING's explanation, not as an operand.
///
/// Red is "this is the danger" across the whole row — the sign, the verb, and
/// the reason the sign is there. Magenta is "this is what you check". Letting
/// the clause take the operand colour put three magenta runs on one line, the
/// longest of them prose, which dilutes exactly the signal the marks exist to
/// concentrate.
#[test]
fn the_gap_clause_speaks_the_warning_vocabulary() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let episode = app.conversation().episode().id;
    let turns = app.current_graph().full_turns(&episode);
    app.ask(ConfirmAction::Delete {
        episodes: Vec::new(),
        turns: vec![TurnTarget {
            episode,
            index: 0,
            turn: turns[0],
            of_total: 3,
        }],
        leaves_gap: true,
        claude_left: 0,
        from: None,
    });

    let prompt = app.confirm_prompt().expect("a question");
    let marks: Vec<Mark> = prompt.runs.iter().map(|r| r.mark).collect();
    assert!(
        marks.contains(&Mark::Consequence),
        "the clause has its own role: {prompt}"
    );
    let clause = prompt
        .runs
        .iter()
        .find(|r| r.mark == Mark::Consequence)
        .expect("the clause");
    assert!(
        clause.text.contains("gone"),
        "and it is the gap explanation: {clause:?}"
    );
    assert_eq!(
        prompt
            .runs
            .iter()
            .filter(|r| r.mark == Mark::Target)
            .count(),
        2,
        "the operands are the turn and its session — and only those: {prompt}"
    );
}

/// The clause's colour, asserted on the rendered cells.
///
/// Its *role* being `Consequence` is an app-side fact; what the user gets is a
/// colour, and the mapping between them is its own place to be wrong. Marking
/// it correctly and then painting it the operand hue passes every mark-level
/// assertion while putting the row back in the state this fixed — which is how
/// the mutation that flipped exactly that survived until this test existed.
#[test]
fn the_rendered_gap_clause_shares_the_warning_colour() {
    use ratatui::style::Color;

    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let episode = app.conversation().episode().id;
    let turns = app.current_graph().full_turns(&episode);
    app.ask(ConfirmAction::Delete {
        episodes: Vec::new(),
        turns: vec![TurnTarget {
            episode,
            index: 0,
            turn: turns[0],
            of_total: 3,
        }],
        leaves_gap: true,
        claude_left: 0,
        from: None,
    });

    let buf = &frame_of(&mut app, 140, 12);
    let y = buf.area().height - 1;
    let row: String = (0..buf.area().width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect();
    let style_at = |needle: &str| {
        let col = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} is not on the row: {row:?}"));
        buf[(col as u16, y)].style()
    };

    let clause = style_at("this leaves");
    let verb = style_at("delete");
    let target = style_at(&turns[0].to_hex()[..8]);

    assert_eq!(
        clause.fg, verb.fg,
        "the sign's explanation speaks the sign's colour: {row:?}"
    );
    assert_eq!(clause.fg, Some(Color::Red), "{row:?}");
    assert_ne!(
        clause.fg, target.fg,
        "…and not the operand hue, or the row has three things competing for \
         one meaning: {row:?}"
    );
}
