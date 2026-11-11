//! Checkout, clone, open, and the titles they give.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

#[test]
fn checkout_a_past_turn_goes_back_to_that_point() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 5); // Hello 0..4
    let episode = app.conversation().episode().id;
    let turn0 = app.conversation().episode().own_turns()[0];
    let turn2 = app.conversation().episode().own_turns()[2];

    // Enter on the FIRST turn → the log up to turn 0 only.
    app.checkout_turn(episode, turn0, 0);
    assert_eq!(
        branch_msgs(&app),
        vec!["Hello 0", "reply 0"],
        "checking out turn 0 shows just turn 0 — not all 5"
    );
    assert_ne!(
        app.conversation().episode().id,
        episode,
        "detached onto a branch, not the full episode"
    );

    // Checkout turn 2 of the (untouched) original → turns 0..=2.
    app.checkout_turn(episode, turn2, 2);
    assert_eq!(
        branch_msgs(&app),
        vec![
            "Hello 0", "reply 0", "Hello 1", "reply 1", "Hello 2", "reply 2"
        ],
        "checking out turn 2 shows the log up to turn 2"
    );
}

#[test]
fn checkout_the_tip_opens_the_whole_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let episode = app.conversation().episode().id;
    let own = app.conversation().episode().own_turns();
    let tip = *own.last().unwrap();
    let tip_index = own.len() - 1;
    app.reset_foreground(); // move off it so the checkout isn't a no-op

    app.checkout_turn(episode, tip, tip_index);
    assert_eq!(
        app.conversation().episode().id,
        episode,
        "checking out the tip opens the whole session"
    );
    assert_eq!(branch_msgs(&app).len(), 6, "all 3 turns (6 messages)");
}

#[test]
fn continuing_after_a_checkout_forks_a_branch_and_leaves_the_original() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 4); // Hello 0..3
    let episode = app.conversation().episode().id;
    let turn1 = app.conversation().episode().own_turns()[1];

    app.checkout_turn(episode, turn1, 1); // → [Hello 0, Hello 1]
    let branch = app.conversation().episode().id;
    assert_ne!(branch, episode);
    assert_eq!(
        app.buffer().on_disk,
        OnDisk::Absent,
        "the checkout is lazy / unpersisted"
    );

    // Continue from the checkout → it forks a branch (persists).
    app.conversation_mut().begin_turn("Hello new");
    app.conversation_mut().push_chunk("reply new");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();

    let store = JsonlStore::new(dir.path()).unwrap();
    // Episode-as-path: the branch's own log stores ONLY its divergent turn —
    // the shared prefix (turns 0..=1) is never copied to disk.
    let loaded = store.load_episode(&branch).unwrap();
    let own: Vec<String> = loaded
        .turns
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| m.content.clone()))
        .collect();
    assert_eq!(
        own,
        vec!["Hello new", "reply new"],
        "on disk the branch owns only the divergent turn, not the shared prefix"
    );
    // The full history reconstructs by deriving through the shared prefix.
    let graph = store.load_graph().unwrap();
    let full: Vec<String> = graph
        .records(&branch)
        .expect("a full load holds every body")
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| m.content.clone()))
        .collect();
    assert_eq!(
        full,
        vec![
            "Hello 0",
            "reply 0",
            "Hello 1",
            "reply 1",
            "Hello new",
            "reply new"
        ],
        "full history derives as [shared prefix up to turn 1] + the new turn"
    );
    assert_eq!(
        store.load_episode(&episode).unwrap().turns.len(),
        4,
        "the original log is untouched"
    );
    assert_eq!(
        store.get_ref("HEAD").unwrap(),
        Some(branch),
        "HEAD moved onto the new branch"
    );
}

#[test]
fn clone_forks_a_sibling_sharing_the_full_history() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3); // Hello 0..2
    let original = app.conversation().episode().id;

    app.clone_session();
    let clone = app.conversation().episode().id;
    assert_ne!(clone, original, "the clone is a distinct episode");
    assert_eq!(
        branch_msgs(&app).len(),
        6,
        "the clone opens on the full 3-turn transcript it shares"
    );
    assert_eq!(
        app.buffer().on_disk,
        OnDisk::Absent,
        "the clone is lazy / unpersisted"
    );

    // Until the clone gets a turn, nothing changed on disk: HEAD stays put.
    let store = JsonlStore::new(dir.path()).unwrap();
    assert_eq!(
        store.get_ref("HEAD").unwrap(),
        Some(original),
        "HEAD stays on the original until the clone is continued"
    );

    // Continue on the clone → it persists divergent-only and takes HEAD.
    app.conversation_mut().begin_turn("branch off");
    app.conversation_mut().push_chunk("new reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();

    let store = JsonlStore::new(dir.path()).unwrap();
    let own: Vec<String> = store
        .load_episode(&clone)
        .unwrap()
        .turns
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| m.content.clone()))
        .collect();
    assert_eq!(
        own,
        vec!["branch off", "new reply"],
        "on disk the clone owns only its divergent turn — the shared history is not copied"
    );
    assert_eq!(
        store.load_graph().unwrap().full_turns(&clone).len(),
        4,
        "full history = 3 shared turns + the 1 new one"
    );
    assert_eq!(
        store.get_ref("HEAD").unwrap(),
        Some(clone),
        "HEAD moved to the clone on its first turn"
    );
    assert_eq!(
        store.load_episode(&original).unwrap().turns.len(),
        3,
        "the original session is untouched"
    );
}

/// **`:clone` says where you are**, as every composing verb and `:fork` do:
/// it landed and took the aim in silence (full review 10-21, D 5, probed).
#[test]
fn clone_says_it_cloned_and_where_you_are() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    app.notices.clear();
    app.clone_episode(original);
    let clone = app.aimed_buffer().conversation.episode().id;
    assert_ne!(clone, original, "premise: landed and aimed at");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains(&format!(
            "cloned {} → now in session {}",
            app.session_tag(original),
            app.session_tag(clone)
        )),
        "{said}"
    );
}

#[test]
fn clone_is_a_noop_on_a_session_with_no_committed_turns() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 0); // blank
    let before = app.conversation().episode().id;
    app.clone_session();
    assert_eq!(
        app.conversation().episode().id,
        before,
        "cloning a blank session is a no-op (nothing to fork, no persisted parent)"
    );
}

#[test]
fn clone_by_hash_forks_a_named_session_while_on_a_different_one() {
    // The reported flow: a persisted session A exists, the *active* session is
    // a different (fresh) one B, and `:clone <A's hash>` must fork A — not
    // silently no-op on the empty active session.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2); // session A, active + persisted
    let a = app.conversation().episode().id;
    app.reset_foreground(); // active is now a fresh, empty session B
    let b = app.conversation().episode().id;
    assert_ne!(a, b);

    app.clone_by_hash(&a.to_hex()[..8]); // short-id, like the user typed
    let clone = app.conversation().episode().id;
    assert_ne!(clone, a);
    assert_ne!(clone, b);
    assert_eq!(
        app.conversation().episode().origin.unwrap().episode,
        a,
        "cloned A (named by hash), not the active empty B"
    );
    assert_eq!(
        branch_msgs(&app).len(),
        4,
        "the clone shares A's full 2-turn history"
    );
}

#[test]
fn clone_from_graph_clones_the_selected_session_not_the_active_one() {
    // Bare `:clone` while `:graph` is open forks the SELECTED node's session,
    // not the active one — matching where the user is pointing.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2); // session A, persisted
    let a = app.conversation().episode().id;
    app.reset_foreground(); // active is now empty session B
    let b = app.conversation().episode().id;

    // Open `:graph` and land the selection on A (the non-active session).
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(a);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);

    app.clone_session(); // bare `:clone` → clones the SELECTED A
    assert_ne!(
        app.conversation().episode().id,
        b,
        "did not clone the active B"
    );
    assert_eq!(
        app.conversation().episode().origin.unwrap().episode,
        a,
        "cloned the selected session A"
    );
    // The viewer was rebuilt so the fork shows up where the user was looking.
    assert!(app.graph_view().is_some(), "graph stays open, refreshed");
}

/// `:clone` typed in `:graph` clones the session it has selected (it was `c`,
/// retired to the command — Steven, 10-14).
#[test]
fn clone_in_graph_clones_the_selected_session_as_a_clone_edge() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2); // session A, persisted
    let a = app.conversation().episode().id;
    app.reset_foreground(); // active empty B
    let b = app.conversation().episode().id;

    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(a);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);

    app.dispatch_command_for_test("clone"); // the selection, not the active B
    assert_ne!(
        app.conversation().episode().id,
        b,
        "did not clone the active B"
    );
    let link = app.conversation().episode().origin.unwrap();
    assert_eq!(link.episode, a, "cloned the selected session A");
    assert_eq!(
        link.kind,
        shannon_core::EdgeKind::Clone,
        "recorded as a clone edge — distinct from a branch"
    );
    assert!(
        app.graph_view().is_some(),
        "the viewer refreshes and stays open"
    );
}

#[test]
fn clone_from_a_turn_row_clones_the_whole_session_by_reference() {
    // `:clone` copies the WHOLE session by reference (a full-share re-root),
    // regardless of which turn is selected — the clone starts identical to the
    // original and shares all of it.
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3); // Hello 0,1,2

    let episode = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(episode);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 0
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 1 (a past turn)

    app.dispatch_command_for_test("clone"); // the whole session, from a turn row
    assert_eq!(
        branch_msgs(&app),
        vec![
            "Hello 0", "reply 0", "Hello 1", "reply 1", "Hello 2", "reply 2"
        ],
        "the clone shares the ENTIRE history, not just up to the selection"
    );
    let clone = app.conversation().episode();
    let origin = clone.origin.unwrap();
    assert_eq!(origin.kind, shannon_core::EdgeKind::Clone);
    assert_eq!(origin.episode, episode, "clones the selected session");
    assert_eq!(
        clone.segments(),
        &[Segment::Slice {
            episode,
            start: 0,
            end: 3,
        }],
        "one Slice of the source's whole history — owns nothing yet"
    );
    // One node, one place: the clone nests under its origin turn (it used to
    // ALSO appear as a second forest root — the double-render bug) and
    // displays its full derived history there (it owns nothing; the derived
    // sequence is its content, drawn grayed as shared).
    let g = app.current_graph();
    assert!(!g.roots().contains(&Node::Episode(clone.id)));
    // The clone anchors at the source's FIRST turn (occurrence 0) — and with
    // occurrence-matched origins it appears there and ONLY there. (This
    // assertion previously claimed index 2 and passed anyway, because the
    // pre-index matching ignored occurrences entirely.)
    let anchor = Node::Turn {
        episode,
        turn: origin.turn,
        index: 0,
    };
    assert!(
        g.children(anchor).contains(&Node::Episode(clone.id)),
        "the clone hangs off the source's first turn — exactly once"
    );
    let elsewhere = Node::Turn {
        episode,
        turn: origin.turn,
        index: 2,
    };
    assert!(
        !g.children(elsewhere).contains(&Node::Episode(clone.id)),
        "and under no other occurrence/row"
    );
    assert!(g.episode(&clone.id).unwrap().shows_full_history());
}

#[test]
fn checking_out_a_past_turn_of_a_lazy_clone_shows_up_to_that_turn() {
    // Regression (Fable #3 / reported): picking a past turn on an unpersisted
    // clone must check out the history UP TO that turn — not silently no-op and
    // leave you on the whole conversation.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3); // Hello 0,1,2
    let original = app.conversation().episode().id;
    app.clone_episode(original); // full-share clone (active, lazy/unpersisted)
    let clone = app.conversation().episode().id;

    let turn0 = app.current_graph().full_turns(&clone)[0]; // "Hello 0"
    app.checkout_turn(clone, turn0, 0);

    assert_eq!(
        branch_msgs(&app),
        vec!["Hello 0", "reply 0"],
        "checkout shows only up to Hello 0, not the full clone"
    );
    assert_ne!(
        app.conversation().episode().id,
        clone,
        "detached onto a new episode"
    );
    // The checkout anchors on the persisted source, not the lazy clone.
    assert_eq!(
        app.conversation().episode().origin.unwrap().episode,
        original
    );
}

#[test]
fn open_by_hash_resolves_episodes_and_turns() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 4); // Hello 0..3
    let episode = app.conversation().episode().id;
    let turn1 = app.conversation().episode().own_turns()[1];
    app.reset_foreground(); // move off it so a switch is observable

    // An episode hash opens that whole branch.
    app.open_by_hash(&episode.to_hex()[..8]);
    assert_eq!(
        app.conversation().episode().id,
        episode,
        "episode hash opens that branch"
    );
    assert_eq!(branch_msgs(&app).len(), 8, "all 4 turns");

    // A turn hash checks out that point.
    app.open_by_hash(&turn1.to_hex()[..8]);
    assert_eq!(
        branch_msgs(&app),
        vec!["Hello 0", "reply 0", "Hello 1", "reply 1"],
        "turn hash checks out up to that turn"
    );

    // An unknown hash is a no-op.
    let before = app.conversation().episode().id;
    app.open_by_hash("ffffffff");
    assert_eq!(
        app.conversation().episode().id,
        before,
        "unknown hash no-ops"
    );

    // A turn two sessions share — every turn a clone inherits — is ONE
    // reading. Without the dedup, `:open <turn>` refused "names 2 things
    // (turn X, turn X)", which no longer prefix could fix (Reviewer C,
    // round 17).
    app.open_by_hash(&episode.to_hex()[..8]);
    app.dispatch_command_for_test("clone");
    let graph = app.current_graph();
    let holders = graph
        .episode_ids()
        .into_iter()
        .filter(|e| graph.full_turns(e).contains(&turn1))
        .count();
    assert!(
        holders >= 2,
        "premise: the turn is in two sessions ({holders})"
    );
    app.open_by_hash(&turn1.to_hex()[..8]);
    assert!(
        !app.notice().unwrap_or_default().contains("names"),
        "{:?}",
        app.notice()
    );
    assert_eq!(
        branch_msgs(&app),
        vec!["Hello 0", "reply 0", "Hello 1", "reply 1"],
        "a shared turn checks out"
    );
}

#[tokio::test]
async fn a_transcript_pane_titles_itself_with_its_own_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 4);
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let root = app.conversation().episode().id;

    // A root session names itself and carries no branch marker.
    let title = app.transcript_title(pane);
    assert!(
        title.contains(&root.short_hex()),
        "names the session: {title}"
    );
    assert!(!title.contains('←'), "root session is unmarked: {title}");

    // Checking out a past (non-tip) turn detaches there: nothing sent, it is
    // AT that turn, not a branch yet (S21-Q8: "branch" said one was made, of
    // a look — Steven's observation 3), and landing says what it means.
    let fork = app.conversation().episode().own_turns()[1];
    app.open_by_hash(&fork.to_hex()[..8]);
    let title = app.transcript_title(pane);
    let at = format!("— at {} ", fork.short_hex());
    assert!(title.contains(&at), "where it stands: {title}");
    assert!(!title.contains("branch"), "{title}");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.starts_with(&format!("at {} (turn 2 of ", fork.short_hex()))
            && said.contains("sending continues from here as a branch; leaving keeps nothing"),
        "{said}"
    );
    // Its first turn its own, it is a branch — named by where it forked
    // (the inspector's and the tree's wording: `Episode::origin_clause`).
    app.submit("and then?");
    app.feed_stream(StreamMsg::Chunk("then this".into()));
    app.feed_stream(StreamMsg::Complete);
    let title = app.transcript_title(pane);
    let branch = format!("— branch ← {} ", fork.short_hex());
    assert!(
        title.contains(&branch),
        "a branch once it owns a turn: {title}"
    );
}

/// Two panes on two sessions get **two** titles. The title used to be computed
/// once from the foreground and handed to every transcript pane — invisible
/// while they all showed the same session, and a lie the moment they did not.
#[test]
fn each_transcript_pane_titles_its_own_session() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());
    app.open_by_hash(&a.to_hex()[..8]);

    let here = app.transcript_title(focused);
    let there = app.transcript_title(sibling);
    assert!(here.contains(&a.short_hex()), "this pane shows A: {here}");
    assert!(there.contains(&b.short_hex()), "that one shows B: {there}");
    assert_ne!(here, there, "two sessions, two titles");
}
