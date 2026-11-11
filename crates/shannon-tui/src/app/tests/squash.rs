//! Pick, merge, squash and distill.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// `s`/`S` with no marks squash the SELECTED branch — the other half of
/// `squash_graph`'s reading list. Every branch-squash test squashed a session
/// already open, whose bodies the overlay supplies (Reviewer C).
#[test]
fn squashing_a_branch_open_nowhere_seeds_its_words() {
    let dir = TempDir::new().unwrap();
    let branch = {
        let mut app = app_with_n_turns(dir.path(), 3);
        let base = app.conversation().episode().id;
        let t1 = app.conversation().episode().own_turns()[1];
        app.checkout_turn(base, t1, 1);
        app.conversation_mut().begin_turn("branch prompt");
        app.conversation_mut().push_chunk("branch reply");
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
        app.conversation().episode().id
    };
    let mut app = relaunched(dir.path());
    let mut gv = app.build_graph_view();
    gv.reveal_and_select_episode(branch);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);

    let SquashPlan { sources, msgs, .. } = app.squash_plan().expect("the stored branch squashes");
    assert_eq!(sources.len(), 1, "its one divergent turn");
    let words: Vec<&str> = msgs.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(words, ["branch prompt", "branch reply"]);
}

/// **A background child must not wipe the marks you were assembling.**
///
/// `refresh_graph_views` runs on every job event — spawn, chunk, completion —
/// and rebuilt each open tree from scratch, dropping the `<Space>` marks and
/// resetting the cursor to the active session. So a `p` (pick) selection you
/// were putting together evaporated the moment a child streamed, with nothing
/// on screen connecting the two: the marks were simply not there any more.
///
/// Marks and the cursor are the user's, not the tree's, and they travel by
/// node identity because a rebuilt tree renumbers its rows.
#[tokio::test]
async fn a_child_event_does_not_wipe_the_marks_you_were_assembling() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let episode = app.conversation().episode().id;

    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(episode);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 0
    app.graph_key_for_test(k(KeyCode::Char(' '))); // mark it
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 1
    let marked = app.graph_view().unwrap().marked_nodes();
    let selected = app.graph_view().unwrap().selected_node();
    assert_eq!(marked.len(), 1, "one node marked");

    // A child spawns and streams — every one of these refreshes the tree.
    let child = app.spawn_child_job("go", None).expect("spawned");
    app.feed_job(child, StreamMsg::Chunk("thinking".into()));

    assert_eq!(
        app.graph_view().unwrap().marked_nodes(),
        marked,
        "the marks are still yours"
    );
    assert_eq!(
        app.graph_view().unwrap().selected_node(),
        selected,
        "and so is the cursor — it did not jump back to the active session"
    );
}

#[test]
fn p_picks_marked_nodes_into_a_new_session() {
    // The `<Space>`-mark + `p` flow: mark turns 0 and 2, pick → a new session with
    // just those (turn 1 pruned), all shared by reference; the original is untouched.
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3); // Hello 0,1,2 with replies
    let original = app.conversation().episode().id;

    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(original);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 0
    app.graph_key_for_test(k(KeyCode::Char(' '))); // mark turn 0
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 1
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 2
    app.graph_key_for_test(k(KeyCode::Char(' '))); // mark turn 2
    app.graph_key_for_test(k(KeyCode::Char('p'))); // pick the marked

    assert_ne!(
        app.conversation().episode().id,
        original,
        "switched to the pick"
    );
    assert_eq!(
        branch_msgs(&app),
        vec!["Hello 0", "reply 0", "Hello 2", "reply 2"],
        "picked turns 0 and 2 in graph order — turn 1 pruned"
    );
    assert!(
        app.conversation().episode().own_turns().is_empty(),
        "a pick owns nothing — every turn is a shared span"
    );
    assert_eq!(
        app.current_graph().full_turns(&original).len(),
        3,
        "the original is untouched"
    );
    assert_eq!(
        marked(&app),
        Some(0),
        "the marks are spent — left lit, they read as still waiting for a verb"
    );
}

#[test]
fn m_merges_a_branch_straight_into_its_base() {
    // The `:graph` `m` flow: base A→B→C, a branch off B with a divergent turn,
    // merged straight in → A→B→[branch turn]→C. Originals preserved; the lazy
    // merged session persists correctly on first continue.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3); // Hello 0,1,2 (base)
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];

    // Fork a branch off turn 1 (shares [turn0, turn1]) and give it a divergent turn.
    app.checkout_turn(base, t1, 1);
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id;
    assert_ne!(branch, base);

    // Merge the branch straight into its base.
    app.merge_branch_into_base(branch);
    assert_eq!(
        branch_msgs(&app),
        vec![
            "Hello 0",
            "reply 0",
            "Hello 1",
            "reply 1",
            "branch prompt",
            "branch reply",
            "Hello 2",
            "reply 2",
        ],
        "branch's divergent turn splices in at the fork; base's tail follows"
    );
    let merged = app.conversation().episode().id;
    assert_eq!(
        app.conversation().episode().origin.unwrap().kind,
        shannon_core::EdgeKind::Merge
    );
    assert!(
        app.conversation().episode().own_turns().is_empty(),
        "a straight merge owns nothing — all spans"
    );
    // So it stays lazy: only a result owning new content is written at once
    // (Steven, 10-16, decision (a)) — and until it is, the exit names it.
    assert!(
        matches!(
            JsonlStore::new(dir.path()).unwrap().episode_len(&merged),
            Ok(None)
        ),
        "a pure-reference result is not written until it is continued"
    );
    assert_eq!(
        app.unwritten().owed,
        [(app.session_label(merged), None)],
        "named at `:q`"
    );

    // Originals preserved on disk.
    let store = JsonlStore::new(dir.path()).unwrap();
    assert_eq!(store.load_graph().unwrap().full_turns(&base).len(), 3);
    assert!(store.load_episode(&branch).is_ok());

    // Continue the (lazy) merged session → it persists; memory == disk.
    app.conversation_mut().begin_turn("after merge");
    app.conversation_mut().push_chunk("ok");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let graph = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    let disk: Vec<String> = graph
        .records(&merged)
        .expect("a full load holds every body")
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| m.content.clone()))
        .collect();
    assert_eq!(
        branch_msgs(&app),
        disk,
        "merged session persists — memory == disk"
    );
}

#[test]
fn s_squash_merges_a_branch_into_its_base() {
    // The `:graph` `s` flow (bypassing $EDITOR): base A→B→C, a branch off B, squash
    // its divergence into a summary P → A→B→P→C. Crucially the squash session OWNS
    // its summary yet is lazy, so this exercises the persist fix — the summary must
    // survive the round-trip (memory == disk after continue).
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3); // base Hello 0,1,2
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1);
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id;

    let graph = app.current_graph();
    let (b, fork, fork_index, config) = app.merge_target(&graph, branch).unwrap();
    assert_eq!(b, base);
    let summary = Turn::seal(
        vec![Message::user("P")],
        config.clone(),
        std::time::SystemTime::UNIX_EPOCH,
    );
    let id = app
        .adopt_composed(
            app.current,
            graph,
            &SquashMerge {
                base,
                branch,
                summary: vec![summary],
            },
            Anchor {
                base,
                turn: fork,
                index: fork_index,
            },
            config,
            None,
            Landing::Asked,
        )
        .unwrap()
        .0;

    assert_eq!(
        branch_msgs(&app),
        vec![
            "Hello 0", "reply 0", "Hello 1", "reply 1", "P", "Hello 2", "reply 2"
        ],
        "branch's divergence compressed to P, spliced at the fork, base tail kept"
    );
    assert_eq!(
        app.conversation().episode().own_turns().len(),
        1,
        "a squash owns exactly its summary"
    );

    // Continue the lazy squash session → the OWNED summary + segment structure
    // persist (the bug would have skipped them); memory == disk.
    app.conversation_mut().begin_turn("after");
    app.conversation_mut().push_chunk("ok");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let g = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    let disk: Vec<String> = g
        .records(&id)
        .expect("a full load holds every body")
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| m.content.clone()))
        .collect();
    assert_eq!(
        branch_msgs(&app),
        disk,
        "squash session persists with its summary — memory == disk"
    );
}

#[test]
fn a_squash_summary_keeps_its_user_and_assistant_messages_distinct() {
    // The bug the user hit: the summary collapsed into ONE user message carrying
    // both the prompt and the reply. A summary is a properly-formed turn — a user
    // message AND an assistant message — and both survive to disk with their roles.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1);
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id;
    // A named branch: folded into its base, it stands beside both, so its
    // name stays its own (review of 34033e8, Reviewer A: held by nothing).
    app.dispatch_command_for_test("rename the-branch");

    // A summary authored via the editable Messages-API JSON, committed through
    // `commit_summary` — the exact tail the `s`/`S` flows run on the edited
    // `$EDITOR` text.
    let text = to_editable(&[
        Message::user("summarized question"),
        Message::assistant("summarized answer"),
    ]);
    let graph = app.current_graph();
    let (b, fork, fork_index, config) = app.merge_target(&graph, branch).unwrap();
    let target = SquashTarget::Branch {
        base: b,
        branch,
        fork_turn: fork,
        fork_index,
        config,
    };
    let id = app
        .commit_summary_into(app.current, graph, target, &[], &text, None)
        .unwrap();
    assert_eq!(app.label_of(id).name, None, "a squash-merge takes no name");
    assert_eq!(
        app.label_of(branch).name.as_deref(),
        Some("the-branch"),
        "the branch keeps its own"
    );

    // Continue so the owned summary persists, then read the summary turn off disk
    // and confirm its two messages kept their roles.
    app.conversation_mut().begin_turn("after");
    app.conversation_mut().push_chunk("ok");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let g = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    let summary_roles: Vec<(Role, String)> = g
        .records(&id)
        .expect("a full load holds every body")
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| (m.role, m.content.clone())))
        .filter(|(_, c)| c.starts_with("summarized"))
        .collect();
    assert_eq!(
        summary_roles,
        vec![
            (Role::User, "summarized question".to_string()),
            (Role::Assistant, "summarized answer".to_string()),
        ],
        "the summary is a user message + an assistant message, not one user lump"
    );
}

#[test]
fn marked_range_squash_composes_in_place_with_provenance() {
    // `<Space>`-mark a contiguous run of turns, squash through the shared
    // `commit_summary` tail: the range collapses to the summary IN PLACE
    // (user-driven compaction) and the summary's provenance names exactly
    // the turns it distilled.
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3); // Hello 0,1,2
    let episode = app.conversation().episode().id;

    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(episode);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 0
    app.graph_key_for_test(k(KeyCode::Char(' '))); // mark turn 0
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 1
    app.graph_key_for_test(k(KeyCode::Char(' '))); // mark turn 1

    let SquashPlan {
        graph,
        target,
        sources,
        msgs,
    } = app.squash_plan().expect("marked range resolves");
    assert!(
        matches!(
            target,
            SquashTarget::Range {
                start: 0,
                end: 2,
                ..
            }
        ),
        "two marked turns → the range [0, 2)"
    );
    assert_eq!(sources.len(), 2);
    assert_eq!(msgs.len(), 4, "two turns' user + assistant messages");

    let text = to_editable(&[
        Message::user("condensed ask"),
        Message::assistant("condensed answer"),
    ]);
    let id = app
        .commit_summary_into(app.current, graph, target, &sources, &text, None)
        .expect("composes");
    assert_eq!(
        branch_msgs(&app),
        vec!["condensed ask", "condensed answer", "Hello 2", "reply 2"],
        "the marked range collapsed in place; the tail survives by reference"
    );
    let g = app.current_graph();
    assert_eq!(
        g.placed(&id)[0].provenance.distilled_from,
        sources,
        "the summary records the turns it distilled"
    );
}

#[test]
fn marked_range_with_gaps_gathers_and_whole_episode_mark_is_everything() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let episode = app.conversation().episode().id;

    // Mark turns 0 and 2 (a gap at 1): NOT widened to 0..3 (that would
    // squash a turn the user never marked) and no longer refused — the
    // marks GATHER: exactly those two turns, destined for a new
    // summary-only session.
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(episode);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 0
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 1
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 2
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    let SquashPlan {
        target, sources, ..
    } = app.squash_plan().expect("gapped marks gather");
    assert!(
        matches!(target, SquashTarget::Gather { .. }),
        "a gap means gather, not an in-place range"
    );
    assert_eq!(
        sources.len(),
        2,
        "exactly the marked turns, nothing widened"
    );

    // A single marked EPISODE row means exactly what the tree shows under
    // it. A root session owns everything → the whole conversation.
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(episode);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char(' '))); // mark the episode row
    let SquashPlan {
        target, sources, ..
    } = app.squash_plan().expect("whole session resolves");
    assert!(
        matches!(
            target,
            SquashTarget::Range {
                start: 0,
                end: 3,
                ..
            }
        ),
        "a marked root row → the full range"
    );
    assert_eq!(sources.len(), 3);
}

#[test]
fn a_marked_branch_row_means_its_displayed_turns_not_its_full_history() {
    // The dogfooding bug: the tree displays a branch with only its DIVERGENT
    // turns as children, but marking the row distilled the full derived
    // history ("distilling 4 turns" while the screen showed 2). WYSIWYG: the
    // row must mean what's drawn under it.
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3); // 3 shared-line turns
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1); // fork after turn 1 (2 shared turns)
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id; // shares 2, owns 1 → full = 3

    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(branch);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char(' '))); // mark the BRANCH row

    let SquashPlan {
        target, sources, ..
    } = app.squash_plan().expect("branch row resolves");
    assert!(
        matches!(
            target,
            SquashTarget::Range {
                start: 2,
                end: 3,
                ..
            }
        ),
        "the marked branch row means its displayed (divergent) turns only"
    );
    assert_eq!(sources.len(), 1, "one divergent turn — what the tree shows");
}

#[tokio::test]
async fn s_is_refused_while_a_prefill_continuation_streams() {
    // Deep-review regression (Reviewer C): the single backend slot is
    // shared by a streaming turn, a distillation, AND a prefill
    // continuation. `llm_squash` guarded the first two but not the third,
    // so `S` mid-continuation double-fired the backend.
    use shannon_core::{ContinueStrategy, StopReason};
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
    app.continue_last_turn(); // prefill → pending_continuation Some
    assert!(app.continuing());

    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);
    assert!(
        app.distilling().is_none(),
        "S must not start a second backend job while a continuation streams",
    );
    assert!(app.notice().expect("refused").contains("continuation"));
}

#[test]
fn cross_session_marks_gather_into_a_summary_only_session() {
    // The vision's "distill based on selected nodes": mark a turn in the
    // base AND a turn in a branch — an impossible in-place range — and the
    // marks gather into a NEW session holding only the summary, whose
    // provenance names both sources. Originals untouched.
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1);
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id;
    // A named source: a gather replaces nothing, so its name stays put.
    app.dispatch_command_for_test(&format!("rename {} keep-me", base.short_hex()));

    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(base);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → base turn 0
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    app.graph_view_mut()
        .unwrap()
        .reveal_and_select_episode(branch);
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → the branch's divergent turn
    app.graph_key_for_test(k(KeyCode::Char(' ')));

    let SquashPlan {
        graph,
        target,
        sources,
        msgs,
    } = app.squash_plan().expect("cross-session gathers");
    assert!(matches!(target, SquashTarget::Gather { .. }));
    assert_eq!(sources.len(), 2, "one turn from each session");
    assert_eq!(msgs.len(), 4, "both turns' messages feed the distillation");

    let text = to_editable(&[
        Message::user("what mattered?"),
        Message::assistant("the gist of both."),
    ]);
    let id = app
        .commit_summary_into(app.current, graph, target, &sources, &text, None)
        .expect("composes");
    assert_eq!(
        branch_msgs(&app),
        vec!["what mattered?", "the gist of both."],
        "the gathered session holds ONLY the summary"
    );
    let g = app.current_graph();
    assert_eq!(
        g.placed(&id)[0].provenance.distilled_from,
        sources,
        "the summary names both distilled sources"
    );
    assert_eq!(g.full_turns(&base).len(), 3, "base untouched");
    assert_eq!(g.full_turns(&branch).len(), 3, "branch untouched");
    assert_eq!(
        app.label_of(base).name.as_deref(),
        Some("keep-me"),
        "and its name with it — a gather replaces nothing"
    );
    assert_eq!(app.label_of(id).name, None, "the gather starts unnamed");
}

#[test]
fn a_multi_turn_summary_seals_multiple_turns_each_with_provenance() {
    // `from_editable_turns` lets a reviewed summary be MORE than one turn —
    // each seals separately and every one records the distilled sources.
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let episode = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(episode);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char(' '))); // whole session

    let SquashPlan {
        graph,
        target,
        sources,
        ..
    } = app.squash_plan().unwrap();
    let text = to_editable(&[
        Message::user("part one?"),
        Message::assistant("part one."),
        Message::user("part two?"),
        Message::assistant("part two."),
    ]);
    let id = app
        .commit_summary_into(app.current, graph, target, &sources, &text, None)
        .expect("composes");
    let g = app.current_graph();
    let turns = g.placed(&id);
    assert_eq!(turns.len(), 2, "a new turn opens at each user message");
    for t in &turns {
        assert_eq!(
            t.provenance.distilled_from, sources,
            "every summary turn names the distilled sources"
        );
    }
}

/// **`S` on marks spends them once the squash is under way** — left lit, they
/// read as still waiting for a verb (Steven, 10-04).
#[tokio::test]
async fn a_marked_squash_spends_the_marks() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(base);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 0
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    app.graph_key_for_test(k(KeyCode::Char('j'))); // → turn 1
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    assert_eq!(marked(&app), Some(2));
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
    assert!(app.distilling().is_some(), "premise: the squash started");
    assert_eq!(marked(&app), Some(0), "the marks are spent");
}

#[tokio::test]
async fn llm_squash_streams_off_transcript_and_esc_cancels() {
    // `S` on a branch spawns the distillation on its own channel: pending
    // state carries the target + sources, chunks accumulate OFF-transcript,
    // a second `S` is refused, a submit queues, and Esc cancels the
    // generation (not the viewer) then drains the queued prompt.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1);
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id;

    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(branch);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
    let label = app.distilling().expect("distillation pending").to_string();
    assert!(label.starts_with("branch "), "labelled by target: {label}");
    let before = branch_msgs(&app);

    // Chunks land on the pending buffer, never in the transcript.
    app.feed_summary(StreamMsg::Chunk("dra".into()));
    app.feed_summary(StreamMsg::Chunk("ft".into()));
    assert_eq!(app.summary_draft().unwrap(), "draft");
    assert_eq!(
        branch_msgs(&app),
        before,
        "the draft streams off-transcript"
    );

    // One generation at a time; prompts queue behind it.
    distill_reviewed(&mut app);
    assert_eq!(app.distilling(), Some(label.as_str()), "second `S` refused");
    app.submit("queued while distilling");
    assert!(!app.streaming(), "no turn starts mid-distill");
    assert_eq!(app.pending.len(), 1);

    // Esc cancels the generation, keeps the viewer, and frees the queue —
    // which refills when the key's event settles.
    app.graph_key_for_test(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.distilling().is_none(), "cancelled");
    assert!(
        app.graph_view().is_some(),
        "Esc hit the distillation, not the viewer"
    );
    assert!(app.streaming(), "the queued prompt drained into a turn");
}

/// **An unreviewed `S` of a branch lands apart** (full review 10-21, A 8).
/// Its result stands beside two sessions and arrives later, so it moves no
/// pane and opens no `$EDITOR`: a buffer of its own, shown in none, and a
/// notice saying how to show it — as an unreviewed `:compact` composes
/// (Steven, 10-21: "Distilling is like compaction so they should act
/// similarly").
#[tokio::test]
async fn an_unreviewed_branch_distillation_lands_apart() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1);
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(branch);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    let shown = |app: &App| -> std::collections::HashMap<_, _> {
        let panes = app.workspace.pane_ids();
        panes
            .into_iter()
            .map(|p| (p, app.live_session(p)))
            .collect()
    };
    let (panes, aim, buffers) = (shown(&app), app.current, app.buffers.sessions().count());

    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    assert!(
        app.decisions.every_review().next().is_none(),
        "unreviewed by default: nothing waits for review"
    );
    assert_eq!(
        app.buffers.sessions().count(),
        buffers + 1,
        "the result has a buffer"
    );
    let landed = app.buffers.sessions().last().expect("just opened");
    let result = landed.conversation.episode().id;
    assert!(result != base && result != branch, "premise: it composed");
    assert_eq!(shown(&app), panes, "no pane moved");
    assert_eq!(app.current, aim, "the aim stays");
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("in no pane") && notice.contains(&format!(":b{}", landed.id.number())),
        "says how to show it: {notice}"
    );
}

/// **A squash you asked for, landing beside its sources, says you are in
/// it, unnamed** — the aim moves to the result, so the session it was
/// asked from is not where you are, and naming it ("b1 …: squashed … →
/// now in session …") would read as about another session (full review
/// 10-21, F 6: the one case where the two differ, which nothing pinned).
#[tokio::test]
async fn a_squash_you_asked_for_says_you_are_in_it_unnamed() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1);
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(branch);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    let asked_from = app.prompt_target();
    let plan = app.squash_plan().expect("the branch resolves");
    let text = to_editable(&[Message::user("q"), Message::assistant("a")]);
    app.notices.clear();
    let id = app
        .commit_summary_into(
            asked_from,
            plan.graph,
            plan.target,
            &plan.sources,
            &text,
            None,
        )
        .expect("composes");
    assert_ne!(
        app.prompt_target(),
        asked_from,
        "premise: the aim moved to the result"
    );
    let said = app.notice().unwrap_or_default();
    assert!(said.starts_with("squashed "), "unnamed: {said}");
    assert!(
        said.contains(&format!("→ now in session {}", app.session_tag(id))),
        "{said}"
    );
}

/// **`:distill!` is the other review for once** (full review 10-21, A 8) —
/// `[merge] distill_review` says whether `S` and `:distill` open `$EDITOR`,
/// and the bang flips it, as `:compact` and `compact_review`.
#[tokio::test]
async fn distill_bang_flips_the_configured_review() {
    for (configured, bang) in [(false, false), (false, true), (true, false), (true, true)] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 2);
        std::sync::Arc::make_mut(&mut app.config)
            .merge
            .distill_review = configured;
        let episode = app.conversation().episode().id;
        let mut gv = GraphView::new(app.current_graph());
        gv.reveal_and_select_episode(episode);
        open_graph_pane(&mut app);
        app.set_graph_view_for_test(gv);
        app.graph_key_for_test(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        app.llm_squash(bang);
        app.feed_summary(StreamMsg::Chunk("the gist".into()));
        app.feed_summary(StreamMsg::Complete);
        let handed = app.decisions.every_review().next().is_some();
        assert_eq!(
            handed,
            configured != bang,
            "distill_review = {configured}, bang = {bang}"
        );
        // Unreviewed, it composed: the session's own turns squashed, over it.
        assert_eq!(
            app.conversation().episode().id != episode,
            !handed,
            "distill_review = {configured}, bang = {bang}"
        );
    }
}

/// **A reviewed `S`'s draft waits as its session's decision, its cost
/// counted** — the accumulator half of the flow: `Complete` puts the draft
/// to its session (it was handed back for the event loop to take the
/// terminal with), usage is folded into the session totals, the draft
/// composes through the same tail as `s`, and an `Error` abandons a later
/// attempt cleanly.
#[tokio::test]
async fn summary_complete_puts_the_draft_up_and_counts_usage() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let episode = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(episode);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char(' '))); // mark the whole session
    distill_reviewed(&mut app);
    assert!(app.distilling().is_some());

    app.feed_summary(StreamMsg::Chunk("the draft".into()));
    app.feed_summary(StreamMsg::Usage(Usage {
        input_tokens: 7,
        output_tokens: 3,
        ..Default::default()
    }));
    app.feed_summary(StreamMsg::Complete);
    let chat = app.current;
    let review = app
        .decisions
        .review(chat)
        .expect("the draft waits for review");
    assert_eq!(review.work().draft, "the draft");
    let sources = review.work().sources.clone();
    assert!(app.distilling().is_none());
    assert_eq!(
        app.buffer().billing.total.input_tokens,
        7,
        "distillation cost shows"
    );
    assert_eq!(app.buffer().billing.total.output_tokens, 3);

    // The reviewed draft composes through the same tail as `s`.
    let text = to_editable(&[
        Message::user("instruction"),
        Message::assistant("the draft"),
    ]);
    let id = app.compose_review(chat, Some(text)).expect("composes");
    let g = app.current_graph();
    assert_eq!(g.placed(&id)[0].provenance.distilled_from, sources);

    // A failed generation clears state and composes nothing.
    let before = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(id);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    distill_reviewed(&mut app);
    assert!(app.distilling().is_some());
    app.feed_summary(StreamMsg::Error(BackendError::RateLimited));
    assert!(
        app.decisions.every_review().next().is_none(),
        "nothing to review"
    );
    assert!(app.distilling().is_none(), "error abandons the attempt");
    assert_eq!(app.conversation().episode().id, before, "nothing composed");
}

#[test]
fn refused_squash_surfaces_a_notice_on_the_status_row() {
    // The dogfooding bug: an invalid mark set made `s`/`S` look like DEAD
    // KEYS — the refusal reached only the log file. A refusal must surface
    // where the user is looking: the status row, until the next keypress.
    //
    // **The refusing case had to change.** It used a lone marked CLONE row,
    // which refused with "owns no turns" — and that refusal was itself the bug
    // Reviewer D found: the row had turns drawn under it, and a clone's row now
    // expands to what it shows. A row with genuinely nothing under it — a fresh
    // session — still refuses, and that is the case worth keeping, because it
    // is the one where there is really nothing to squash.
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    app.new_session_command();
    let empty = app.conversation().episode().id;
    app.notices.clear();
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(empty);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(k(KeyCode::Char(' '))); // mark the empty row
    assert!(app.notice().is_none(), "marking is quiet");
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));

    let notice = app
        .notice()
        .expect("the refusal is surfaced, not just logged");
    assert!(
        notice.contains("own no turns"),
        "says WHY it refused: {notice}"
    );
    assert!(app.distilling().is_none(), "nothing was spawned");
    assert_eq!(marked(&app), Some(1), "a refused verb spends nothing");

    // And it renders — the status row shows the notice, not the ambient hints.
    let text = text_of(&mut app, 100, 24);
    assert!(text.contains("own no turns"), "notice reaches the screen");

    // `p` with nothing marked notifies too (same channel).
    let mut app2 = app_with_n_turns(dir.path(), 2);
    let ep2 = app2.conversation().episode().id;
    let mut gv2 = GraphView::new(app2.current_graph());
    gv2.reveal_and_select_episode(ep2);
    // In a graph PANE: without it the view hung off the focused prompt, a
    // state the product cannot produce, and only a seam that skipped the
    // router could press a key at it.
    open_graph_pane(&mut app2);
    app2.set_graph_view_for_test(gv2);
    app2.graph_key_for_test(k(KeyCode::Char('p')));
    assert!(
        app2.notice()
            .expect("pick hint surfaces")
            .contains("<Space>")
    );
}
