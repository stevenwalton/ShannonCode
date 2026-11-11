//! `e`: editing a turn or a message in `$EDITOR`.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

#[test]
fn editing_a_session_open_nowhere_seeds_its_words() {
    let dir = TempDir::new().unwrap();
    let old = app_with_n_turns(dir.path(), 2).conversation().episode().id;
    let mut app = relaunched(dir.path());
    select_stored_turn(&mut app, old, 1);
    let plan = app.edit_plan().expect("the stored turn is editable");
    assert_eq!((plan.episode, plan.start, plan.end), (old, 1, 2));
    assert!(
        plan.seed.contains("Hello 1") && plan.seed.contains("reply 1"),
        "the seed is the turn's words: {}",
        plan.seed
    );
}

/// **An edit over a reply that names a value an alias masks, a thought
/// after it** (review stop 15, E H1): the edit masks what you wrote as a
/// typed prompt is masked (a plain alias — the home's `~` is one —
/// included), and the thought's place moves with the text. Kept where it
/// was, past the masked text's end, the branch failed its content hash on
/// load and was gone at the next launch. And saved unchanged, the edit IS
/// the reply as the model wrote it: masked alike, it matches, and nothing
/// is composed — it made a branch of nothing.
#[tokio::test]
async fn an_edit_over_a_masked_value_before_a_thought() {
    use shannon_core::Store;
    let dir = TempDir::new().unwrap();
    let mut app = super::position_marks::over_store(dir.path());
    let mut table = shannon_core::Aliases::default();
    let path = "/src/some/long/repo/path";
    table.insert("repo", path, "config", false).unwrap();
    app.aliases = Arc::new(table);
    app.submit("where is it");
    app.feed_stream(StreamMsg::Chunk(format!("it is in {path} now")));
    super::thinking::think(&mut app, &["checked it"]);
    app.feed_stream(StreamMsg::Complete);
    let original = app.conversation().episode().id;
    let turn = app.conversation().messages().to_vec();
    assert!(turn[1].content.contains(path), "premise: as written");
    assert_eq!(turn[1].blocks.len(), 1, "premise: a thought after it");
    assert!(
        app.commit_slice_edit(original, 0, 1, &to_editable(&turn))
            .is_none(),
        "unchanged: nothing composed"
    );
    assert_eq!(
        app.notice().as_deref(),
        Some("edit unchanged; nothing composed")
    );
    let mut edited = turn.clone();
    edited[0].content = "where is it now".into();
    let branch = (app.commit_slice_edit(original, 0, 1, &to_editable(&edited)))
        .unwrap_or_else(|| panic!("composes: {:?}", app.notice()));
    let store = JsonlStore::new(dir.path()).unwrap();
    store
        .load_episode(&branch)
        .unwrap_or_else(|e| panic!("the branch loads: {e}"));
}

#[test]
fn a_slice_edit_of_a_session_open_nowhere_keeps_its_untouched_turns() {
    let dir = TempDir::new().unwrap();
    let old = app_with_n_turns(dir.path(), 2).conversation().episode().id;
    let mut app = relaunched(dir.path());
    let edited = to_editable(&[Message::user("Hello again"), Message::assistant("reply 1")]);
    let branch = app
        .commit_slice_edit(old, 1, 2, &edited)
        .unwrap_or_else(|| panic!("composes: {:?}", app.notice()));
    let g = app.graph_reading(&[branch]).unwrap();
    let words: Vec<String> = g
        .records(&branch)
        .expect("hydrated")
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| m.content.clone()))
        .collect();
    assert_eq!(words, ["Hello 0", "reply 0", "Hello again", "reply 1"]);
}

/// **An edit that only takes a turn out is yours to lose** (the close
/// verifier at review stop 15, HIGH 1, probed) — every turn it keeps is
/// shared, so it owns none and stays unwritten, a `Branch` as a checkout
/// is; read as a look by its kind, `:qa` let it go without a word, and the
/// edit was gone at the next launch. It is a composition: `:qa` names it.
#[test]
fn an_edit_that_only_prunes_is_named_at_quit() {
    use shannon_core::Role::{Assistant, User};
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let original = app.conversation().episode().id;
    let pruned = edited_transcript(&[
        (User, "Hello 0"),
        (Assistant, "reply 0"),
        (User, "Hello 2"),
        (Assistant, "reply 2"),
    ]);
    let branch = app.commit_slice_edit(original, 0, 3, &pruned).unwrap();
    let ep = app.conversation().episode();
    assert_eq!((ep.id, ep.own_len()), (branch, 0), "premise: owns nothing");
    assert_eq!(app.buffer().on_disk, OnDisk::Absent, "premise: unwritten");
    let ends = app.quitting_ends().join("\n");
    assert!(ends.contains("unwritten"), "the prune named: {ends:?}");
}

#[test]
fn e_edits_a_reply_and_forks_a_branch_sharing_the_prefix() {
    // `e` edits the conversation up to a point → a branch: the unchanged prefix is
    // shared (Slice), the edited turn is owned, turns AFTER the point are dropped,
    // and the original is untouched.
    use shannon_core::Role::{Assistant, User};
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3); // Hello 0,1,2 (persisted)
    let original = app.conversation().episode().id;

    // Edit turn 1's assistant reply, editing up to point 1 (turns 0 and 1).
    let edited = edited_transcript(&[
        (User, "Hello 0"),
        (Assistant, "reply 0"),
        (User, "Hello 1"),
        (Assistant, "edited reply"),
    ]);
    let branch = app.commit_slice_edit(original, 0, 3, &edited).unwrap();

    assert_eq!(
        branch_msgs(&app),
        vec!["Hello 0", "reply 0", "Hello 1", "edited reply"],
        "prefix + edit; the DELIBERATELY omitted turn 2 is dropped (whole-range edit)"
    );
    let ep = app.conversation().episode();
    assert_eq!(ep.id, branch);
    assert_eq!(ep.origin.unwrap().kind, shannon_core::EdgeKind::Branch);
    // Turn 0 is shared by reference; the edited turn 1 is owned.
    assert!(matches!(
        ep.segments(),
        [Segment::Slice { episode, start: 0, end: 1 }, Segment::Own(_)]
            if *episode == original
    ));

    // memory == disk (the edit persisted eagerly; no truncation on reload).
    let graph = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    let disk: Vec<String> = graph
        .records(&branch)
        .expect("a full load holds every body")
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| m.content.clone()))
        .collect();
    assert_eq!(
        branch_msgs(&app),
        disk,
        "in-memory view equals on-disk derivation"
    );
    // The original is untouched.
    assert_eq!(graph.full_turns(&original).len(), 3, "still its 3 turns");
}

#[test]
fn a_message_edit_splices_into_its_turn_only() {
    // Hierarchy fidelity: `e` on a MESSAGE row edits that one message; the
    // rest of its turn — and every other turn — is untouched. Deleting the
    // content (empty array) removes just that message from the turn.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    let graph = app.current_graph();
    let turn1 = graph.full_turns(&original)[1];

    // Replace turn 1's ASSISTANT message (index 1) alone.
    let edited = to_editable(&[Message::assistant("replaced reply")]);
    let slice_text = spliced_message_edit(&graph, turn1, 1, &edited).unwrap();
    app.commit_slice_edit(original, 1, 2, &slice_text).unwrap();
    assert_eq!(
        branch_msgs(&app),
        vec!["Hello 0", "reply 0", "Hello 1", "replaced reply"],
        "one message changed; its sibling and the other turn are untouched"
    );

    // Out-of-range and malformed edits refuse with a reason.
    assert!(spliced_message_edit(&graph, turn1, 9, &edited).is_err());
    assert!(spliced_message_edit(&graph, turn1, 1, "nope {").is_err());
}

#[test]
fn e_editing_a_middle_turn_shares_both_flanks_cow() {
    // The SURGICAL edit (WYSIWYG: a turn row edits that one turn): the editor
    // shows ONLY turn 1's messages, and the untouched flanks — before AND
    // after — survive by reference: `[Slice, Own, Slice]`. The suffix is kept
    // without ever appearing in the edit surface.
    use shannon_core::Role::{Assistant, User};
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let original = app.conversation().episode().id;

    // The edit surface is turn 1 alone.
    let edited = edited_transcript(&[(User, "Hello 1"), (Assistant, "edited middle")]);
    app.commit_slice_edit(original, 1, 2, &edited).unwrap();

    assert_eq!(
        branch_msgs(&app),
        vec![
            "Hello 0",
            "reply 0",
            "Hello 1",
            "edited middle",
            "Hello 2",
            "reply 2"
        ],
    );
    assert!(
        matches!(
            app.conversation().episode().segments(),
            [
                Segment::Slice { episode: e0, start: 0, end: 1 },
                Segment::Own(_),
                Segment::Slice { episode: e2, start: 2, end: 3 },
            ] if *e0 == original && *e2 == original
        ),
        "shared head + owned middle + shared tail — both flanks are pointers"
    );
}

#[test]
fn e_rejects_a_malformed_edit() {
    // A hand-edit that isn't valid Messages-API JSON cancels cleanly (no branch,
    // session unchanged) rather than composing garbage.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    assert_eq!(
        app.commit_slice_edit(original, 0, 2, "this is not json {"),
        None
    );
    assert_eq!(
        app.conversation().episode().id,
        original,
        "still on the original"
    );
}

#[test]
fn e_prepending_at_the_tip_re_roots_sharing_the_whole_suffix() {
    // Re-root via the "edit and prepend" path: `e` on the tip edits the WHOLE
    // conversation; prepending a fresh root in the JSON yields a re-root — owned new
    // root + the entire original shared as one suffix Slice. The diff decides the op.
    use shannon_core::Role::{Assistant, User};
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2); // Hello 0/reply 0, Hello 1/reply 1
    let original = app.conversation().episode().id;
    let edited = edited_transcript(&[
        (User, "new root"),
        (Assistant, "new root reply"),
        (User, "Hello 0"),
        (Assistant, "reply 0"),
        (User, "Hello 1"),
        (Assistant, "reply 1"),
    ]);
    app.commit_slice_edit(original, 0, 2, &edited).unwrap();

    assert_eq!(
        branch_msgs(&app),
        vec![
            "new root",
            "new root reply",
            "Hello 0",
            "reply 0",
            "Hello 1",
            "reply 1"
        ],
    );
    assert!(
        matches!(
            app.conversation().episode().segments(),
            [Segment::Own(_), Segment::Slice { episode, start: 0, end: 2 }]
                if *episode == original
        ),
        "re-root shape: [Own(new root), Slice(whole original)] — the suffix is one pointer"
    );
}

#[test]
fn e_can_edit_the_user_message() {
    // The whole point of the format: the user side is editable too, not just the
    // reply. Editing turn 0's user message owns from turn 0 (common = 0).
    use shannon_core::Role::{Assistant, User};
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let original = app.conversation().episode().id;

    let edited = edited_transcript(&[(User, "greetings"), (Assistant, "reply 0")]);
    app.commit_slice_edit(original, 0, 3, &edited).unwrap();

    assert_eq!(
        branch_msgs(&app),
        vec!["greetings", "reply 0"],
        "the user message is edited; later turns dropped"
    );
    assert!(
        app.conversation().episode().own_turns().len() == 1,
        "nothing shared (first turn changed) — the one edited turn is owned"
    );
}

#[test]
fn e_with_an_unchanged_edit_is_a_noop() {
    // Feeding back the exact prefix reseals identically → no branch, active session
    // unchanged.
    use shannon_core::Role::{Assistant, User};
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;

    let same = edited_transcript(&[
        (User, "Hello 0"),
        (Assistant, "reply 0"),
        (User, "Hello 1"),
        (Assistant, "reply 1"),
    ]);
    assert_eq!(
        app.commit_slice_edit(original, 0, 2, &same),
        None,
        "no change"
    );
    assert_eq!(
        app.conversation().episode().id,
        original,
        "still on the original"
    );
}

#[tokio::test]
async fn slice_edit_on_an_empty_session_refuses_instead_of_panicking() {
    // Deep-review regression (Reviewer B): commit_slice_edit's own bounds
    // permitted start==end==0 on an empty episode, then `full_ids.len()-1`
    // underflowed and indexed an empty slice. Directly exercised here.
    let mut app = test_app();
    app.reset_foreground(); // a fresh, turn-less episode
    let empty = app.conversation().episode().id;
    let edited = to_editable(&[Message::user("hi"), Message::assistant("there")]);
    assert_eq!(
        app.commit_slice_edit(empty, 0, 0, &edited),
        None,
        "an empty session has no turn to anchor an edit — refused, not panicked",
    );
}

/// The environment probe the user asked for: `e` can rewrite what the
/// ASSISTANT said, and the next request carries the edited history — the
/// model's past is genuinely changed, not overlaid. Asked of the request the
/// next turn sends (it read `snapshot()`, which no request is built from
/// alone: review stop 12, T1 5).
#[tokio::test]
async fn editing_the_assistants_own_words_rewrites_what_the_model_is_sent() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_with_store(
        backend.clone(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    for i in 0..2 {
        app.conversation_mut().begin_turn(format!("Hello {i}"));
        app.conversation_mut().push_chunk(&format!("reply {i}"));
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }
    let episode = app.conversation().episode().id;

    // The full history as the `e` editor would show it, with the assistant's
    // first reply gaslit.
    let graph = app.graph_reading(&[episode]).unwrap();
    let mut msgs: Vec<Message> = graph
        .records(&episode)
        .expect("hydrated")
        .into_iter()
        .flat_map(|t| t.messages)
        .collect();
    let target = msgs
        .iter_mut()
        .find(|m| m.content == "reply 0")
        .expect("the original reply is present");
    target.content = "I never said that".to_string();
    let edited = to_editable(&msgs);

    app.commit_slice_edit(episode, 0, 2, &edited)
        .expect("the edit composes a branch");
    app.submit("and now?");
    let sent = last_sent(&backend);
    assert!(
        sent.contains(&"I never said that".to_string()),
        "the payload the backend receives carries the rewritten assistant turn"
    );
    assert!(
        !sent.contains(&"reply 0".to_string()),
        "the original assistant words are gone from the working history"
    );
    assert!(
        sent.contains(&"reply 1".to_string()),
        "untouched turns survive (shared by reference)"
    );
}
