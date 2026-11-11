//! Children and sends: spawning, the gate, holding, and the ids you type.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

#[tokio::test]
async fn returning_a_child_open_nowhere_brings_its_answer() {
    let dir = TempDir::new().unwrap();
    let child = {
        let mut app = app_with_n_turns(dir.path(), 1);
        let child = app.spawn_child_job("compute the answer", None).unwrap();
        app.feed_job(child, StreamMsg::Chunk("the answer is 42".into()));
        app.feed_job(child, StreamMsg::Complete);
        child
    };
    let mut app = relaunched(dir.path());
    app.return_result(child);
    assert_eq!(
        branch_msgs(&app),
        [
            "Hello 0",
            "reply 0",
            "compute the answer",
            "the answer is 42"
        ],
        "{:?}",
        app.notice()
    );
}

#[tokio::test]
async fn a_child_spawns_off_the_head_turn_and_runs_in_the_background() {
    // The P6.1 core: `spawn_child_job` composes a FRESH episode hung off the
    // parent's head turn (ToolCall edge, occurrence-indexed), persists its
    // header immediately (inspectable while running), and the foreground
    // conversation is untouched. Stream handling is driven directly (the
    // channel path is the same shared-arm pattern the summary flow proved).
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let parent = app.conversation().episode().id;

    let child = app
        .spawn_child_job("say hi", None)
        .expect("spawns off the head turn");
    assert_eq!(app.running_children(), 1);
    assert_eq!(
        app.conversation().episode().id,
        parent,
        "the foreground session is untouched"
    );

    // Inspectable while running: the header is on disk, nested at the head.
    let g = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    let origin = g.episode(&child).unwrap().origin.unwrap();
    assert_eq!(origin.episode, parent);
    assert_eq!(
        origin.index,
        Some(1),
        "hangs off the HEAD turn's occurrence"
    );
    assert_eq!(origin.kind, shannon_core::EdgeKind::ToolCall);

    // The briefing turn streams into the CHILD's conversation only, and
    // Complete commits + persists it and retires the job — announced.
    app.feed_job(child, StreamMsg::Chunk("hi ".into()));
    app.feed_job(child, StreamMsg::Chunk("there".into()));
    app.feed_job(
        child,
        StreamMsg::Usage(Usage {
            input_tokens: 5,
            output_tokens: 2,
            ..Default::default()
        }),
    );
    assert!(!branch_msgs(&app).contains(&"say hi".to_string()));
    app.feed_job(child, StreamMsg::Complete);
    assert_eq!(
        app.running_children(),
        0,
        "the job retires; the episode stays"
    );
    // On the CHILD's bill, not the parent's. This asserted `app.buffer()` —
    // the foreground — which was the only buffer there was when it was written,
    // and quietly became the wrong one when usage moved per-session.
    let child_usage = app
        .buffers
        .sessions()
        .find(|b| b.conversation.episode().id == child)
        .expect("the child's buffer outlives its job")
        .billing
        .total;
    assert_eq!(
        child_usage.input_tokens, 5,
        "the child pays for its own turn"
    );
    assert_eq!(
        app.buffer().billing.total.input_tokens,
        0,
        "and the parent's bill is untouched"
    );
    assert!(
        app.notice()
            .expect("resolution announced")
            .contains("finished")
    );

    let g = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    let msgs: Vec<String> = g
        .records(&child)
        .expect("a full load holds every body")
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| m.content.clone()))
        .collect();
    assert_eq!(
        msgs,
        vec!["say hi".to_string(), "hi there".to_string()],
        "the child's turn is on disk — briefing + reply"
    );
}

#[tokio::test]
async fn a_child_needs_a_turn_to_hang_from_and_can_override_its_model() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.reset_foreground(); // empty foreground
    assert!(
        app.spawn_child_job("hi", None)
            .unwrap_err()
            .contains("send a message first"),
        "no turn to hang from → visible refusal"
    );

    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("hi", Some("haiku".into())).unwrap();
    let g = app.current_graph();
    assert_eq!(
        g.episode(&child).unwrap().config.model,
        shannon_core::backend::models::resolve("haiku"),
        "the -m/--model override lands in the child's config, RESOLVED — it \
         used to reach the API as the literal string \"haiku\", which is a 400"
    );

    // A failed child announces itself and retires without composing.
    app.feed_job(child, StreamMsg::Error(BackendError::RateLimited));
    assert_eq!(app.running_children(), 0);
    assert!(app.notice().expect("announced").contains("failed"));
}

#[tokio::test]
async fn child_dispatch_reads_the_shared_flag_grammar() {
    // `:child -m haiku fix the tests` — flags front-parsed, remainder is
    // the briefing verbatim, model lands in the child's config.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    assert_eq!(
        app.child_dispatch("-m haiku fix the tests"),
        ChildDispatch::Done
    );
    assert_eq!(app.running_children(), 1);
    let child = app.job_episodes()[0];
    let g = app.current_graph();
    assert_eq!(
        g.episode(&child).unwrap().config.model,
        shannon_core::backend::models::resolve("haiku")
    );

    // A malformed flag is a visible refusal, not a silent no-op.
    assert_eq!(app.child_dispatch("--nope hi"), ChildDispatch::Done);
    assert!(app.notice().expect("refusal shown").contains("--nope"));
    assert_eq!(app.running_children(), 1, "nothing new spawned");
}

#[test]
fn a_flags_only_child_invocation_still_wants_the_editor() {
    // The user's exact spec: "If we call `/child -m haiku` or any
    // `/child --model <agent>` then we should consider this as still
    // opening $EDITOR for instructions."
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    assert_eq!(
        app.child_dispatch("-m haiku"),
        ChildDispatch::NeedsBriefing {
            model: Some("haiku".into())
        }
    );
    assert_eq!(
        app.child_dispatch(""),
        ChildDispatch::NeedsBriefing { model: None },
        "bare `:child` goes to $EDITOR too"
    );
    assert_eq!(app.running_children(), 0, "no briefing, no spawn");
}

#[tokio::test]
async fn guardrail_postures_confirm_block_and_advise() {
    // confirm (the default): the spawn parks on the y/N line with the
    // reason; `y` approves the crossing and fires the held spawn.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_two_turns(dir.path(), agents_config(0, 4, OnLimit::Confirm));
    app.spawn_child_guarded("hi".into(), None);
    assert_eq!(app.running_children(), 0, "parked, not spawned");
    let prompt = app.confirm_prompt().expect("the y/N line shows why").text();
    assert!(prompt.contains("spawns deep") && prompt.contains("spawn anyway"));
    type_keys(&mut app, "y");
    assert_eq!(app.running_children(), 1, "y fires the held spawn");

    // block: a visible refusal, nothing spawned.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_two_turns(dir.path(), agents_config(0, 4, OnLimit::Block));
    app.spawn_child_guarded("hi".into(), None);
    assert_eq!(app.running_children(), 0);
    assert!(app.notice().expect("refusal shown").contains("refused"));

    // advisory: spawned AND announced.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_two_turns(dir.path(), agents_config(0, 4, OnLimit::Advisory));
    app.spawn_child_guarded("hi".into(), None);
    assert_eq!(app.running_children(), 1);
    assert!(
        app.notice()
            .expect("crossing announced")
            .contains("over the limit")
    );
}

#[tokio::test]
async fn fanout_counts_child_sessions_not_user_branches() {
    // max_fanout = 1: a manual BRANCH hanging off this session must not
    // eat the budget (user surgery ≠ fan-out); the second SPAWN must.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_two_turns(dir.path(), agents_config(3, 1, OnLimit::Block));
    let parent = app.conversation().episode().id;
    let head = app.conversation().episode().own_turns()[1];
    let branch = Episode::from_segments(
        fresh_episode_id(),
        TurnConfig {
            model: "m".into(),
            max_tokens: None,
            effort: None,
        },
        vec![],
        Some(Origin {
            episode: parent,
            turn: head,
            index: Some(1),
            kind: EdgeKind::Branch,
        }),
    );
    app.persist_episode(&branch, &[]).unwrap();

    app.spawn_child_guarded("first".into(), None);
    assert_eq!(
        app.running_children(),
        1,
        "a user branch never eats the fan-out budget"
    );
    app.spawn_child_guarded("second".into(), None);
    assert_eq!(app.running_children(), 1, "the real second child is over");
    assert!(app.notice().expect("refused").contains("child sessions"));
}

#[tokio::test]
async fn stop_cancels_a_running_child_by_id_prefix() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("long task", None).unwrap();
    // In capitals: ids are lowercase, and `--stop ABCD` matched nothing
    // while `:delete ABCD` worked (Reviewer D, round 16).
    let prefix = child.to_hex()[..8].to_uppercase();

    app.child_dispatch(&format!("--stop {prefix}"));
    assert_eq!(app.running_children(), 0);
    assert!(app.notice().expect("announced").contains("stopped"));
    // The header persisted at spawn remains — inspectable, empty.
    let g = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    assert!(g.episode(&child).is_some(), "the episode outlives the job");
    assert!(g.full_turns(&child).is_empty(), "nothing was committed");

    // A prefix matching nothing is a visible refusal — and `--stop` WINS
    // over any briefing text on the same line (a stop must never
    // accidentally spawn).
    app.child_dispatch("--stop ffffffff say hi anyway");
    assert!(
        app.notice()
            .expect("refused")
            .contains("no session matches")
    );
    assert_eq!(app.running_children(), 0, "the trailing text did not spawn");
    // Read as every session verb reads a session (`session_named`): a word
    // is a name, a short run of hex a name or an id too short.
    app.child_dispatch("--stop zzzz");
    assert!(
        app.notice()
            .expect("refused")
            .contains("no session is named")
    );
    app.child_dispatch("--stop ab");
    assert!(
        app.notice()
            .expect("refused")
            .contains("first 8 characters or more")
    );
    // Four to seven hex characters read as an id, and are refused as too
    // short — not "no session matches", a search that never ran (11-10).
    app.child_dispatch(&format!("--stop {}", &child.to_hex()[..7]));
    assert!(
        app.notice().expect("refused").contains("too short"),
        "{:?}",
        app.notice()
    );
    // The child's session, stopped: nothing runs in it now.
    app.child_dispatch(&format!("--stop {}", &child.to_hex()[..8]));
    assert!(
        app.notice()
            .expect("refused")
            .contains("no child is running")
    );
    assert_eq!(app.running_children(), 0);
}

/// **`:child --stop` takes the child as every session verb does** — its
/// `:ls` number (`b3`) and `%`, not only its id: `b3` was refused as "too
/// short … or `b<N>` from `:ls`", recommending the form it refused (full
/// review 10-21, A 9, probed).
#[tokio::test]
async fn stop_takes_the_child_by_its_ls_number_and_by_percent() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("long task", None).unwrap();
    let number = app.buffer_for_episode(child).expect("open").number();
    app.child_dispatch(&format!("--stop b{number}"));
    assert_eq!(app.running_children(), 0, "by its number");
    assert!(app.notice().expect("said").contains("stopped"));

    let child = app.spawn_child_job("another", None).unwrap();
    aim_at(&mut app, child);
    app.child_dispatch("--stop %");
    assert_eq!(app.running_children(), 0, "by `%`, aimed at it");
}

/// **`:child --stop` stops the child it names, and no other** — with two
/// running, the one named goes and the other runs on. Every test ran one
/// child, so matching any child passed them all (full review 10-21, A 9,
/// mutated).
#[tokio::test]
async fn stop_takes_only_the_child_it_names() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let first = app.spawn_child_job("one", None).unwrap();
    let second = app.spawn_child_job("two", None).unwrap();
    assert_eq!(app.running_children(), 2, "premise: two run");
    let number = app.buffer_for_episode(first).expect("open").number();
    app.child_dispatch(&format!("--stop b{number}"));
    assert_eq!(app.job_episodes(), [second], "the other runs on");
}

#[tokio::test]
async fn a_failing_child_keeps_its_billed_partial_on_disk() {
    use shannon_core::StopReason;
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("long task", None).unwrap();
    app.feed_job(child, StreamMsg::Chunk("halfway th".into()));
    app.feed_job(child, StreamMsg::Error(BackendError::RateLimited));

    assert_eq!(app.running_children(), 0);
    assert!(app.notice().expect("announced").contains("partial kept"));
    // The partial is a real, persisted, marked turn — inspectable in
    // :graph and continuable after switching in.
    let g = JsonlStore::new(dir.path()).unwrap().load_graph().unwrap();
    let turns = g.records(&child).expect("a full load holds every body");
    assert_eq!(turns.len(), 1, "the interrupted turn is on disk");
    let t = &turns[0];
    assert_eq!(t.messages[1].content, "halfway th");
    assert_eq!(t.provenance.stop_reason, Some(StopReason::Interrupted));
}

#[tokio::test]
async fn r_returns_a_finished_child_as_one_turn_in_the_parent() {
    // P6.3, the third composition of an episode's life: birth (spawn),
    // life (the background turn), RETURN — the child collapses to one
    // briefing→answer turn appended to the parent, its full transcript
    // one hop away via distilled_from.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let parent = app.conversation().episode().id;
    let child = app.spawn_child_job("compute the answer", None).unwrap();
    app.feed_job(child, StreamMsg::Chunk("the answer is 42".into()));
    app.feed_job(child, StreamMsg::Complete);

    app.return_result(child);
    let returned = app.conversation().episode().id;
    assert_ne!(returned, parent, "a NEW composed session — originals kept");
    let msgs = branch_msgs(&app);
    assert_eq!(msgs.len(), 6, "the parent's 2 turns + ONE result turn");
    assert_eq!(msgs[4], "compute the answer", "the briefing goes in");
    assert_eq!(msgs[5], "the answer is 42", "the final answer comes out");

    let g = app.current_graph();
    let t = *g.placed(&returned).last().unwrap();
    assert_eq!(
        t.provenance.distilled_from,
        g.episode(&child).unwrap().own_turns(),
        "the result stands in for the child's own turns"
    );
    assert_eq!(
        t.provenance.usage, None,
        "composing is free — the spend stays on the child (no double count)"
    );
    // The result holds the child's exact exchange — ONE content, two
    // productions — and each keeps its own facts in the same snapshot. Keyed
    // by id, whichever buffer the snapshot overlaid last won both, which is why
    // `snapshot_graph` once had to put the foreground last.
    let answered = *g.placed(&child).last().unwrap();
    assert_eq!(
        answered.turn, t.turn,
        "premise: the result repeats the child's turn"
    );
    assert!(
        answered.provenance.distilled_from.is_empty(),
        "the child's own turn distilled nothing"
    );
    assert!(
        answered.provenance.usage.is_some(),
        "and it keeps the child's bill"
    );
    assert!(g.episode(&child).is_some(), "the child is preserved");
    assert!(g.episode(&parent).is_some(), "the parent is preserved");
    assert!(app.notice().expect("announced").contains("returned"));
}

#[tokio::test]
async fn return_refuses_roots_and_unfinished_children_with_reasons() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let root = app.conversation().episode().id;
    app.return_result(root);
    assert!(app.notice().expect("refused").contains("root"));

    // A child mid-turn has committed nothing — the refusal says WHY.
    let child = app.spawn_child_job("busy work", None).unwrap();
    app.return_result(child);
    assert!(app.notice().expect("refused").contains("still running"));
    assert_eq!(app.conversation().episode().id, root, "nothing switched");
}

/// A pane showing a child mid-turn says so on its own border. The status row's
/// count answers "is something running", never "which of these panes".
#[tokio::test]
async fn a_pane_showing_a_running_child_says_so() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("say hi", None).expect("spawned");
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();

    assert!(!app.transcript_title(pane).contains("running"), "not here");
    app.open_by_hash(&child.to_hex()[..8]);
    assert!(
        app.transcript_title(pane).contains("running"),
        "the child's pane is marked: {}",
        app.transcript_title(pane)
    );

    app.feed_job(child, StreamMsg::Complete);
    assert!(
        !app.transcript_title(pane).contains("running"),
        "and unmarked when it finishes"
    );
}

/// An empty `--stop` value is not a wildcard.
///
/// `:child --stop=` parses to an empty id, and `starts_with("")` is true of
/// every running child — so a typo naming nothing killed your only child.
/// Reviewer E flagged it as informational in July; it is a dead-typo that
/// destroys work, which makes it worth the two lines.
#[tokio::test]
async fn an_empty_stop_id_refuses_instead_of_matching_every_child() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("go", None).unwrap();
    assert_eq!(app.job_episodes().len(), 1);

    app.stop_child("");

    assert_eq!(app.job_episodes().len(), 1, "the child is still running");
    assert_eq!(app.job_episodes()[0], child);
}

/// A session already running a turn refuses a second one — through `:Send`,
/// through a child, either way. Both branches of the refusal were unguarded:
/// the dedup test only covered two marks in ONE call, never a second call.
/// (Reviewers A and C, convergent, both mutation-verified.)
#[tokio::test]
async fn a_session_already_running_refuses_a_second_send() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let buffer = app.pane_session(pane).id;

    // A job is running here…
    app.start_job_on(buffer, "first")
        .expect("the first send goes");
    assert_eq!(app.job_episodes().len(), 1);
    let again = app.start_job_on(buffer, "second");
    assert!(
        again.is_err_and(|e| e.contains("is running in this session")),
        "a second job on one session would double-book its conversation"
    );

    // …and the same holds when the FOREGROUND owns it — each refused in its
    // holder's own words (full review 10-21, D 6: "already running a turn"
    // was said of every kind).
    let mut app = test_app();
    app.submit("foreground turn");
    assert!(
        app.streaming(),
        "premise: the foreground holds this session"
    );
    let buffer = app.pane_session(app.workspace.focused()).id;
    assert!(
        app.start_job_on(buffer, "sneak in")
            .is_err_and(|e| e.contains("a turn is in flight")),
        "a job must not run alongside the foreground turn in one session"
    );
}

/// Deleting a session abandons the job writing into it, so nothing bleeds into
/// the replacement.
#[tokio::test]
async fn resetting_a_session_abandons_its_job() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let buffer = app.pane_session(pane).id;
    app.start_job_on(buffer, "running").expect("a job");
    assert_eq!(
        app.job_episodes().len(),
        1,
        "premise: a job on this session"
    );

    app.reset_buffer(buffer);

    assert!(
        app.job_episodes().is_empty(),
        "the job went with the session"
    );
}

/// `:model` refuses while a JOB holds the session, not just the foreground —
/// a turn is sealed under the config it finds at commit time.
#[tokio::test]
async fn model_refuses_while_a_job_holds_this_session() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let buffer = app.pane_session(pane).id;
    app.start_job_on(buffer, "running").expect("a job");
    let before = app
        .pane_session(pane)
        .conversation
        .episode()
        .config
        .model
        .clone();

    app.model_command("opus");

    assert!(
        app.notice().expect("a refusal").contains("running"),
        "says why"
    );
    assert_eq!(
        app.pane_session(pane).conversation.episode().config.model,
        before
    );
}

// --- the owner scopings, each pinned separately ---------------------------
//
// `abandon_tool_round` and `cancel_running_tools` landed three owner scopings
// in one commit and the message called all three "pinned". One was. These are
// the other two plus the queue, because "scoped by owner, and pinned" is two
// claims and they have to be counted separately (Reviewers A and E, both by
// mutation).

/// **A job ending must not kill the foreground's running command.**
///
/// `cancel_running_tools` is the function that kills processes, and its owner
/// filter was held in place by nothing but its comment: deleting
/// `owned.contains(id)` left the whole suite green while a child erroring in
/// the background killed the user's approved command mid-write.
#[tokio::test]
async fn a_job_ending_does_not_kill_the_foregrounds_running_command() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    let child = app.spawn_child_job("go", None).expect("spawned");
    // The user approved something of their own, and it is still running.
    model_call(&mut app, "tu-fg", "rm -rf /tmp/shannon-test-absent-fg");
    tool_key(&mut app, 'a');
    assert!(
        app.running_tools.contains_key("tu-fg"),
        "premise: the foreground's command is in flight"
    );

    app.feed_job(child, StreamMsg::Error(BackendError::RateLimited));

    assert!(
        app.running_tools.contains_key("tu-fg"),
        "the child's teardown killed a command it does not own"
    );
}

/// **A job ending must not kill a command the USER started.**
///
/// The `!manual_tool_ids.contains(id)` half of the same filter, unpinned for
/// the same reason. A `:tool` belongs to nobody, so an owner-scoped sweep can
/// only spare it by naming it.
#[tokio::test]
async fn a_job_ending_does_not_kill_a_manual_tool_command() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    let child = app.spawn_child_job("go", None).expect("spawned");
    let manual = manual_tool(&mut app, "echo hi");
    assert!(
        app.running_tools.contains_key(&manual),
        "premise: the user's own command is in flight"
    );

    app.feed_job(child, StreamMsg::Error(BackendError::RateLimited));

    assert!(
        app.running_tools.contains_key(&manual),
        "the child's teardown killed the user's own command"
    );
}

/// **A job ending must leave the foreground's QUEUED call answerable.**
///
/// The pane half of this was pinned; the queue half was a comment. Under the
/// mutation (`retain` → `clear`) any background job ending discards every
/// queued foreground call — and those calls are already in
/// `pending_tool_use_ids`, so `resume_after_tools` can never be satisfied and
/// the foreground turn is wedged open forever.
#[tokio::test]
async fn a_job_ending_leaves_the_foregrounds_queued_call_alone() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    let child = app.spawn_child_job("go", None).expect("spawned");
    // Two calls from the foreground: one on the pane, one behind it.
    model_call(&mut app, "tu-fg1", "rm -rf /tmp/shannon-test-absent-1");
    model_call(&mut app, "tu-fg2", "rm -rf /tmp/shannon-test-absent-2");
    assert_eq!(app.waiting_calls().len(), 1, "premise: one call is queued");

    app.feed_job(child, StreamMsg::Error(BackendError::RateLimited));

    assert_eq!(
        app.waiting_calls().len(),
        1,
        "the child's teardown swept away a call the foreground still owes an answer for"
    );
}

/// **A rule the USER wrote does not authorize a child to run something.**
///
/// `safe_unattended` is stricter than `Decision::Allow` for a stated reason: an
/// `allow` rule was written thinking about the user's own foreground work, not
/// about an agent they spawned and walked away from. The predicate was pinned
/// in core; its use *as the job gate* was not, and every TUI test runs
/// `RuleSet::default()`, where `Allow` and `DefaultRead` coincide — so nothing
/// ever put a job in front of a line allowed by a user rule, which is the only
/// shape that tells the two bars apart (Reviewer A, by mutation).
#[tokio::test]
async fn a_user_written_allow_rule_does_not_let_a_child_run_unattended() {
    let mut app = test_app();
    // The program and the place: `allow rm *` decides the program, and a
    // delete outside the project needs the place granted (Steven, 10-06).
    app.rules
        .extend_from_lines("allow rm *\nallow :delete /tmp")
        .expect("rules the user might well write");
    // Premise: the foreground WOULD run this without asking.
    assert_eq!(
        app.rules.decide_line("rm -rf /tmp/gone").decision,
        shannon_core::permission::Decision::Allow,
        "premise: the user's own rule allows it"
    );
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");

    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-1", "rm -rf /tmp/gone")),
    );

    assert!(
        app.running_tools.is_empty(),
        "a consent given for the keyboard is not a consent given for a child"
    );
    assert!(
        app.notice().is_some_and(|n| n.contains("held on")),
        "and it holds, visibly: {:?}",
        app.notice()
    );
}

/// **An alias a child cannot resolve holds rather than running the raw text.**
///
/// Pins the BEHAVIOUR, and deliberately not the specific door. Reviewer E
/// mutation-checked `gate_job_tool_call`'s `resolve_for_gate` arm and found it
/// unpinned; it is now unpinnable, because two independent mechanisms produce
/// the same answer — the door holds on the resolution failure, and a line still
/// carrying `{{a:…}}` is a word the gate cannot place, which `safe_unattended`
/// refuses on its own (a hard ask is never by default). Deleting the door
/// still leaves this green.
///
/// Keeping it anyway is the fail-closed call, not a second guard: "what the
/// analyzer classified and what the shell runs must be one text" is the door's
/// rule, and it should not depend on the *analyzer* happening to also object.
/// Said in these words rather than dressed as belt-and-braces.
#[tokio::test]
async fn a_child_holds_on_an_alias_it_cannot_resolve() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");

    // The spawn's own notice satisfied "a notice is up" (review stop 12, T3 7).
    app.notices.clear();
    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-1", "cat {{a:nosuch}}")),
    );

    assert!(
        app.running_tools.is_empty(),
        "the line we would run is not the line we analyzed"
    );
    assert!(
        app.notices
            .iter()
            .any(|(_, n)| n.contains("held on") && n.contains("{{a:nosuch}}")),
        "and the user is told what it holds on, rather than the child silently stalling: {:?}",
        app.notices
    );
}

/// **A background model cannot claim the id of a command the USER ran.**
///
/// `tool_use_id` is chosen by the model and is the key of six maps here. That
/// was safe by accident — one generation could have a round open, so a
/// collision could not occur — and (81) removed the accident while leaving the
/// key with the party the maps arbitrate between.
///
/// The concrete theft: `manual_tool_ids` is the single flag that makes a
/// user-typed `:tool` result undeliverable, and `finish_tool_call` consumes it
/// with `remove()` on the first finish for that id, from any owner. A child
/// emitting the user's id cleared the flag, and the output of a command the
/// user approved *because it stays local* was packaged and sent to the child's
/// model (Reviewer E, HIGH).
#[tokio::test]
async fn a_child_cannot_claim_the_id_of_a_users_own_command() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    // The user's own command, in flight.
    let stolen = manual_tool(&mut app, "echo secret-user-output");
    assert!(
        app.tool_call(&stolen).is_some_and(|c| c.owner.is_none()),
        "premise: the call is recorded as the USER's, which is what keeps its \
         output undeliverable"
    );

    // The child asks for a tool under the user's id.
    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call(&stolen, "ls")),
    );

    // **One assertion now, and that is the fix.** These were two — "the flag
    // is still set" and "the owner is still nobody" — over two collections that
    // could disagree, and the bug was exactly them disagreeing: the child's
    // finish consumed the flag while the owner map still said the call was the
    // user's. One record cannot half-change.
    assert!(
        app.tool_call(&stolen).is_some_and(|c| c.owner.is_none()),
        "the child took the user's call: its output is a local-only command's, \
         and an owned call gets packaged and sent to that owner's model"
    );
}

/// The same door on the foreground side: a second call re-using a live id is
/// refused, visibly — and **the first keeps its own answer**. The duplicate
/// is never answered: an answer to it lands in the first call's round, a
/// result forged into it (review stop 7, E 4). This test was named for the
/// unsafe behaviour ("and still answered"), its check held from the first
/// call alone, and putting the forged answer back passed it (review stop 12,
/// T3 3).
#[tokio::test]
async fn a_reused_tool_id_is_refused_visibly_and_the_first_keeps_its_answer() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    model_call(&mut app, "tu-1", "echo first");
    app.notices.clear();
    app.arrive_and_gate_for_test(app.foreground_id(), "tu-1", "echo second");

    assert!(
        app.notice().is_some_and(|n| n.contains("tu-1")),
        "a refusal the user can see, naming the id: {:?}",
        app.notice()
    );
    settle(&mut app).await;
    let answers: Vec<_> = (tool_results(&app).into_iter())
        .filter(|(id, _, _)| id == "tu-1")
        .collect();
    assert_eq!(
        answers,
        [(
            "tu-1".to_string(),
            "(recorded, not run) echo first".to_string(),
            false
        )],
        "one answer, the first call's own output"
    );
}

/// `:tool` ids must not be guessable. `manual-1` is spelled out in the manual
/// and starts at one, which is what made the collision above *reachable*
/// rather than merely possible — a model need not race for a `toolu_01…` it
/// cannot see when the id is written down.
#[test]
fn manual_tool_ids_are_not_guessable() {
    let a = test_app();
    let b = test_app();
    assert_ne!(
        a.manual_tool_id(),
        b.manual_tool_id(),
        "two runs must not mint the same first id"
    );
    // Not the documented sequence, for any of the first few serials. Asserted
    // over the whole id rather than a prefix: `starts_with("manual-1")` is
    // true of any hex id beginning with a 1, which is a one-in-sixteen flake in
    // a test about unpredictability.
    let mut app = test_app();
    for seq in 1..=8u64 {
        app.manual_tool_seq = seq;
        assert_ne!(
            app.manual_tool_id(),
            format!("manual-{seq}"),
            "the id a reader of the manual would guess"
        );
    }
}

/// **A held child reads as held, everywhere it is reported.**
///
/// `Held::Command` was write-only: constructed in `hold_job`, read only as
/// `held.is_none()`, and its payload rendered nowhere. So the safety state wore
/// the working state's face — `busy_phrase` said "is running in this session",
/// `busy_recovery` said "stop it (CTRL-C) or **wait**" (an infinite wait, since
/// the thing holding the slot is waiting on *you*), `:graph` drew `▶ running`,
/// and the status row counted it among the children that were working. The
/// identical distinction was already drawn correctly one arm above, for
/// `Work::Turn { AwaitingDecision }` (Reviewers A and E, converging).
///
/// A guard's user-facing half is part of the guard: a safety state only the
/// type system knows about is a wedge, not a protection.
#[tokio::test]
async fn a_held_child_is_reported_as_held_and_not_as_running() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-1", "rm -rf /tmp/gone")),
    );

    assert_eq!(app.held_children(), 1, "counted as held");
    assert_eq!(app.running_children(), 0, "and NOT as running");

    // The refusal a guard shows when this generation holds the slot names the
    // state and a recovery that exists.
    let buffer = app.buffer_for_episode(child).expect("the child's buffer");
    let (phrase, recovery) = app
        .busy_reason_and_recovery(buffer)
        .expect("something holds the child's session");
    assert!(phrase.contains("held on"), "{phrase}");
    assert!(phrase.contains("rm -rf"), "and names it: {phrase}");
    assert!(
        !recovery.contains("wait"),
        "waiting is what will never work here: {recovery}"
    );

    // And `:graph` marks it distinctly from a child that is working.
    let view = app.build_graph_view();
    assert_eq!(
        view.activity_of(&child),
        Some(crate::graph_view::Activity::Held),
        "the graph row says held, not running"
    );

    // …and so does the pane border, which read `· running` off a surviving
    // `bool` that could not say "held" (Reviewer D). "Everywhere it is
    // reported" is the claim in this test's title; the border is a place it is
    // reported.
    app.open_by_hash(&child.to_hex()[..8]);
    let pane = app.workspace.focused();
    let title = app.transcript_title(pane);
    assert!(
        !title.contains("running"),
        "the border calls a held child running: {title}"
    );
    assert!(title.contains("held"), "and does not say held: {title}");
}

/// A held job records only the FIRST thing it stopped on. A held job is not
/// running, so there is no second — but the model can still emit more
/// `tool_use` blocks in the same message, and overwriting would lose the one
/// the user has to act on. Unpinned when it landed (Reviewer E, by mutation).
#[tokio::test]
async fn a_held_child_remembers_the_first_thing_it_stopped_on() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");

    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-1", "rm -rf /tmp/first")),
    );
    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-2", "rm -rf /tmp/second")),
    );

    let buffer = app.buffer_for_episode(child).expect("the child's buffer");
    let (phrase, _) = app
        .busy_reason_and_recovery(buffer)
        .expect("something holds the child's session");
    assert!(
        phrase.contains("/tmp/first"),
        "the one the user has to act on, not the last one seen: {phrase}"
    );
}

/// **Once held, held.** A message can carry several `tool_use` blocks, and a
/// child that stopped on `rm -rf` used to go on and run the `ls` after it. Each
/// was individually safe with nobody watching, so it was never a bypass — but
/// the manual promises "it does not route around the block and it does not try
/// something else", and a promise that is false is worse than a narrower one
/// (Reviewer E).
#[tokio::test]
async fn a_held_child_does_not_run_the_next_call_in_the_same_message() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");

    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-1", "rm -rf /tmp/gone")),
    );
    // Individually safe with nobody watching — and irrelevant, because the
    // child is stopped.
    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-2", "ls")),
    );

    assert!(
        app.running_tools.is_empty(),
        "a stopped child kept working: {:?}",
        app.running_tools.keys().collect::<Vec<_>>()
    );
}

/// **A command that ran is not recorded as one that did not.**
///
/// A turn can end (a rate limit, `CTRL-C`) after some of its commands have run
/// and reported. `abandon_tool_round` discarded those answers and `commit_turn`
/// then synthesized "the turn ended before this ran" for *every* request — so
/// `⚙ output of: ls` sat in the transcript while the persisted conversation
/// told the model it never ran, and `:continue` handed it a false record. The
/// same failure as the "I changed the `rm` to an `echo`" bug, pointed the other
/// way (Reviewer A).
#[tokio::test]
async fn an_abandoned_round_keeps_the_results_that_did_arrive() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("go");
    // Something streamed, so the failed turn is sealed rather than discarded.
    app.feed_stream(StreamMsg::Chunk("let me look".into()));
    // Two calls: one auto-allowed, one waiting on the user. BOTH before either
    // is answered — otherwise the round completes and drains, which is the
    // healthy path and not the one under test.
    model_call(&mut app, "tu-ran", "ls");
    model_call(&mut app, "tu-never", "rm -rf /tmp/shannon-test-absent");
    app.finish_tool_call("tu-ran", "a b c".into(), false);
    assert!(
        app.tool_results.contains_key("tu-ran"),
        "premise: one call has a real answer"
    );

    // The backend dies mid-round.
    app.feed_stream(StreamMsg::Error(BackendError::RateLimited));

    let sealed = app
        .conversation()
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            shannon_core::Block::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some((tool_use_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        sealed.get("tu-ran"),
        Some(&"a b c"),
        "the real output survives into the sealed turn: {sealed:?}"
    );
    assert!(
        sealed
            .get("tu-never")
            .is_some_and(|c| c.contains("before this ran")),
        "and the one that genuinely did not run still says so: {sealed:?}"
    );
}

/// **A job resumes even when the foreground has a call queued.**
///
/// `a6a5dcb` made two corrections in one paragraph: drain the queue **and**
/// resume, rather than drain-*or*-resume; and ask every generation rather than
/// the owner of the finished call. The second was pinned. The first was not —
/// adding `return;` after the drain, which is literally the pre-fix `else`,
/// left 901/901 green (Reviewer A).
///
/// Without it a child stops mid-turn forever, holding its session's slot, with
/// nothing in flight and no message: the queue pop re-gates a foreground call
/// that goes straight back on the queue (the pane is still up), and the child
/// is never asked.
#[tokio::test]
async fn a_job_resumes_even_when_the_foreground_had_a_queued_call() {
    let backend = std::sync::Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_backed(backend.clone(), Arc::new(Config::default()));
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");

    app.submit("go");
    model_call(&mut app, "fg-1", "rm -rf /tmp/shannon-test-absent");
    model_call(&mut app, "fg-2", "rm -rf /tmp/shannon-test-absent-2");
    assert!(
        app.shown_call().is_some(),
        "premise: a decision is on screen"
    );
    assert_eq!(app.waiting_calls().len(), 1, "premise: one call is queued");

    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("ch-1", "ls")),
    );
    app.feed_job(child, StreamMsg::Complete);
    let sent = backend.requests.lock().unwrap().len();
    app.finish_tool_call("ch-1", "a b c".into(), false);

    assert_eq!(
        backend.requests.lock().unwrap().len(),
        sent + 1,
        "the child's answer arrived while the foreground had a queued call, and \
         the child never resumed — it holds its session slot forever"
    );
}

/// **A command that was KILLED is not recorded as one that never ran.**
///
/// The distinction is the whole point: a killed command may have done half its
/// work. `cancel_running_tools` recorded the kill in the transcript
/// (`⚙ cancelled: … killed part-way`) and contributed no `tool_result`, so
/// `commit_turn` synthesized "the turn ended before this ran" for it — the
/// screen and the persisted conversation disagreeing about the same command,
/// with `:continue` handed the false one (Reviewer A).
///
/// The sibling of `an_abandoned_round_keeps_the_results_that_did_arrive`, and
/// the more dangerous half: that one loses a result we HAVE, this one asserts
/// something that is not true.
#[tokio::test]
async fn a_killed_command_is_recorded_as_killed_not_as_never_run() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("go");
    app.feed_stream(StreamMsg::Chunk("let me look".into()));
    // Auto-allowed, so it starts running immediately and is still in flight.
    model_call(&mut app, "tu-running", "ls -la");
    assert!(
        app.running_tools.contains_key("tu-running"),
        "premise: the command is in flight, not finished"
    );

    // The backend dies; the round is abandoned and the command killed.
    app.feed_stream(StreamMsg::Error(BackendError::RateLimited));

    let sealed: std::collections::HashMap<&str, &str> = app
        .conversation()
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            shannon_core::Block::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some((tool_use_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect();
    let recorded = sealed.get("tu-running").copied().unwrap_or("<absent>");
    assert!(
        recorded.contains("cancelled part-way"),
        "the model must be told it RAN and was killed, not that it never \
         started: {recorded:?}"
    );
}

/// **Refusing a colliding id must not answer the call it collided WITH.**
///
/// `finish_tool_call` resolves through `tool_owner`, which for a taken id
/// points at the other claimant. So the refusal wrote a forged result into the
/// victim's `tool_results`, took the victim's ownership of a command that is
/// still executing — after which its own teardown can no longer kill it — and
/// completed the victim's round on the forgery (Reviewer A).
///
/// Ownership became a parameter of the gate in `fd419fc`; the ANSWER path
/// stayed ambient, which is the same bug one function over.
#[tokio::test]
async fn refusing_a_colliding_id_leaves_the_other_generations_call_alone() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");

    // The child owns a running command under id `shared`.
    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("shared", "ls")),
    );
    assert!(
        app.running_tools.contains_key("shared"),
        "premise: the child's command is in flight"
    );
    let owner_before = app.tool_generation("shared");
    assert!(owner_before.is_some(), "premise: the child owns it");

    // The foreground now emits the same id.
    app.submit("go");
    model_call(&mut app, "shared", "echo collision");

    assert_eq!(
        app.tool_generation("shared"),
        owner_before,
        "the refusal took the child's ownership of a command still running — \
         after which nothing can kill it"
    );
    assert!(
        !app.tool_results.contains_key("shared"),
        "and it forged an answer into the child's round"
    );
    assert!(
        app.running_tools.contains_key("shared"),
        "the child's command is still running and still owned"
    );
}

/// **Every arrival arm claims the id, including the ones that answer directly.**
///
/// `claim_tool_id` sat inside the gate, which the unknown-tool and
/// unreadable-command arms never reach — they call `finish_tool_call`, which
/// keys on the id in three maps. So those ids were never claimed and a later
/// call could take one as fresh (Reviewer A, round 1).
///
/// **This test exists because the thing round 1 left here was a `probe_` with
/// no assertions at all** — it printed state and always passed, and was the
/// only caller of a seam added for it. Reintroducing the exact bug left 911
/// green (Reviewer C, round 2). A test that measures nothing is worse than no
/// test: it is why the guard was believed.
#[tokio::test]
async fn an_unknown_tool_call_still_claims_its_id() {
    let mut app = test_app();
    app.submit("go");
    // An arm that answers directly rather than going through the gate.
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::ToolUse {
        id: "shared".into(),
        name: "nosuchtool".into(),
        input: Default::default(),
    }));
    assert!(
        app.issued_for_probe("shared"),
        "the unknown-tool arm keys on the id and must claim it first"
    );

    // A later call reusing it is refused rather than treated as fresh.
    let before = app.notice();
    app.arrive_and_gate_for_test(app.foreground_id(), "shared", "echo reused");
    let notice = app.notice().expect("a visible refusal");
    assert_ne!(Some(notice.to_string()), before);
    assert!(notice.contains("shared"), "naming the id: {notice}");
}

/// **A `:tool` decision on screen blocks a new prompt** — and that rule rests
/// on `busy_reason`'s non-generation arm, which three successive comments
/// called unreachable.
///
/// A `:tool` has no generation by construction, so with nothing else running
/// `conflict()` finds nothing and only that arm answers. Deleting it left the
/// suite green while the behaviour it implements is one a user meets by typing
/// two commands (Reviewer A).
#[tokio::test]
async fn a_manual_tool_decision_blocks_a_new_prompt() {
    let mut app = test_app();
    manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent-busy");
    assert!(
        app.shown_call().is_some(),
        "premise: the decision is on screen"
    );
    assert!(
        app.turns().next().is_none(),
        "premise: and NO generation is running — the arm under test is the \
         only one that can answer"
    );

    let reason = app
        .busy_reason_for_probe(app.current)
        .expect("a `:tool` decision holds the session");
    assert!(reason.contains("awaiting your decision"), "{reason}");
}

/// **A duplicate `tool_use` id must not wedge the turn that sent it.**
///
/// Refusing a colliding id first forged a result into the victim's round
/// (Reviewer A); the fix for that tore down the REFUSING generation's round
/// instead — with the wrong buffer, and while it was still live — so the
/// decision pane vanished, the turn stayed `AwaitingDecision` holding the
/// global slot, and its named recovery ("answer it first") was impossible
/// because there was nothing on screen to answer (Reviewer D).
///
/// The right answer touches nobody: drop the block. It has not been pushed
/// into the conversation yet, so nothing is left unanswered.
#[tokio::test]
async fn a_duplicate_tool_id_in_one_message_leaves_the_first_answerable() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("go");
    // The model sends the same id twice in one message.
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent-dup");
    assert!(
        app.shown_call().is_some(),
        "premise: the first opened a pane"
    );
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent-dup2");

    assert!(
        app.shown_call().is_some(),
        "the duplicate tore down the decision the user was reading"
    );
    assert!(
        app.awaiting_decision(),
        "and the turn is still open to answer into"
    );
}

/// **A child that FINISHES but does not reach disk says so.**
///
/// The `Complete` arm announced success only on `Ok`, and on `Err` said
/// nothing at all — so the last line on screen stayed the spawn notice, and the
/// user was told the child was still running when it had finished and its turn
/// existed only in memory, for `leaving()` to drop on quit.
///
/// The silence was justified in a comment by "the failure `persist_job_turns`
/// just reported"; `persist_job_turns` reports nothing, it returns `Err`. The
/// premise stopped being true and the comment did not (Reviewer D). Both
/// sibling exits already route through `partial_fate`; this is the third.
#[tokio::test]
async fn a_child_that_finishes_without_reaching_disk_says_so() {
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(shannon_core::store::FailingStore::failing_appends()),
    );
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    app.feed_job(child, StreamMsg::Chunk("work worth keeping".into()));

    app.feed_job(child, StreamMsg::Complete);

    let notice = app.notice().expect("says what happened").to_string();
    assert!(
        notice.contains("finished"),
        "it did finish, and that is half the truth: {notice}"
    );
    assert!(
        notice.contains("NOT saved") || notice.contains("not saved"),
        "and the other half — the work is memory-only: {notice}"
    );
}

/// **A marked send ASKS; only a child holds.**
///
/// Marking two panes is the documented reason the whole window model exists —
/// one question answered twice, side by side. Marks are sticky, so from the
/// moment Steven marked two panes in his demo, every Enter became a job, every
/// tool call went to the unattended gate, and the approval pane he was trying
/// to show his friend could not appear at all. The code was right and there is
/// a test asserting it ("two background turns, not one foreground"); the axis
/// was wrong. `:Send` means "not the session in front of you", not "nobody is
/// watching" — you just pressed Enter.
#[tokio::test]
async fn a_marked_send_asks_and_a_child_holds() {
    // A command the unattended bar refuses and the foreground gate asks about:
    // `mkdir` is not modelled, so it is `Ask` either way.
    const COMMAND: &str = "mkdir build";

    // The SEND path: a decision pane, answerable.
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    for p in panes {
        app.workspace.toggle_mark(p);
    }
    app.submit("do the thing");
    let sends: Vec<GenerationId> = app.generation_ids_for_test();
    assert!(!sends.is_empty(), "premise: the marked prompt made jobs");
    app.gate_job_tool_call_for_test(sends[0], COMMAND);
    // **Drawn and answerable, not merely recorded.** This asserted
    // `tool_prompt.is_some()` — the field, not the pane — so gutting
    // `fit_tool_pane` left it green while the doc above it says "must ASK",
    // which means visible with keys you can press (Reviewer C). "Is it on
    // screen" and "can it be answered" are one question here, and the assertion
    // has to ask the one the sentence promises
    // (`[[project_shannon_two_predicates]]`, `[[feedback_assert_the_subject]]`).
    assert!(
        app.shown_call().is_some(),
        "premise: a send you are watching asks rather than holding"
    );
    assert!(
        app.workspace.has(PaneKind::Tool),
        "the decision gets a PANE, not just a field on the app"
    );
    let screen = text_of(&mut app, 80, 24);
    assert!(
        screen.contains(COMMAND),
        "…drawn, showing the command it is asking about\n{screen}"
    );
    assert!(
        screen.contains("allow") && screen.contains("deny"),
        "…and the keys that answer it — on screen, or the pane is a picture of \
         a question\n{screen}"
    );

    // The CHILD path: held, unchanged. A spawned child really is unspectated.
    let mut app = test_app();
    // A child hangs off a turn, so give it one first.
    app.conversation_mut().begin_turn("hello");
    app.conversation_mut().push_chunk("hi");
    app.conversation_mut().commit_turn();
    app.spawn_child_job("go and look at something", None)
        .expect("spawns");
    let jobs: Vec<GenerationId> = app.generation_ids_for_test();
    app.gate_job_tool_call_for_test(jobs[0], COMMAND);
    assert!(
        app.shown_call().is_none(),
        "a child has nobody watching, so it holds rather than asking"
    );
    assert!(
        app.held_jobs(JobKind::Child) > 0,
        "…and it is held, not silently dropped"
    );

    // **And it keeps following it on the SECOND round.** `resume_after_tools`
    // had one binding doing two jobs — which blockers apply, and what the model
    // is told about who is watching — so a send answered a decision pane in
    // round one and was told "NOBODY IS WATCHING" from round two on, while its
    // author sat there answering panes (Reviewer A, reproduced). A predicate is
    // only one door if every door calls it.
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    for p in panes {
        app.workspace.toggle_mark(p);
    }
    app.submit("do the thing");
    // (The resumed round is asserted on the WIRE by
    // `a_resumed_send_sends_the_attended_description` — reading the predicate
    // here is what let the production binding regress unnoticed.)
    let _ = app.generation_ids_for_test();

    // **The DESCRIPTION follows the same axis as the gate.** Both used to be
    // decided separately and both said "unattended", so a model answering a
    // marked send was told "NOBODY IS WATCHING" while its author sat reading
    // the reply — a wrong hint is a real anti-control (`secrets.md` §10c).
    let app = test_app();
    let described = |unattended: bool| -> String {
        app.tool_request(
            vec![Message::user("hi")],
            &app.sealing_for("m".into(), None),
            unattended,
        )
        .tools[0]
            .description
            .clone()
    };
    let send = described(!crate::app::JobKind::Send.attended());
    let child = described(!crate::app::JobKind::Child.attended());
    assert!(
        !send.contains("NOBODY IS WATCHING"),
        "a send is watched, and must not be told otherwise: {send}"
    );
    assert!(
        child.contains("NOBODY IS WATCHING"),
        "…and a child is not, and must be told so: {child}"
    );
}

/// **A held job names the id you need, not an `<id>` to go and find.**
///
/// The notice read "held on: … — see `:graph`, or `:child --stop <id>`", and
/// the id vanishes the instant you type `:`, so there was no way to act on it
/// short of memorising a hash. Steven, dogfooding: *"unless I memorize the hash
/// (lol, not going to happen with humans) AND type it really fast, then there's
/// no good way to actually kill the child."*
///
/// **The hold's own notice, the command whole.** The spawn notice names the
/// child's id too, so `contains(short)` held with the id dropped from the
/// stop, or the hold notice gone (review stop 12, T3 2): the set-up's words
/// are cleared first, and what is asked for is what you would type.
#[tokio::test]
async fn a_held_job_names_the_id_you_would_type() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("hello");
    app.conversation_mut().push_chunk("hi");
    app.conversation_mut().commit_turn();
    let child = app
        .spawn_child_job("go and look at something", None)
        .expect("spawns");
    let jobs = app.generation_ids_for_test();
    app.notices.clear();
    app.gate_job_tool_call_for_test(jobs[0], "mkdir build");

    let notice = app.notice().unwrap_or_default().to_string();
    let stop = format!("`:child --stop {}`", child.short_hex());
    assert!(
        notice.contains(&stop),
        "the notice must carry the stop with its id ({stop}), not a placeholder: {notice}"
    );
    assert!(
        !notice.contains("<id>"),
        "a placeholder is not an instruction: {notice}"
    );
}

/// **A `:Send` you are watching is REFUSED, not held — even when the refusal
/// comes from a site that runs before the gate.**
///
/// `gate_job_tool_call` forked on `attended` in the middle of its own
/// preamble, so the two endings above the fork — an unknown tool, and a
/// `{{handle}}` that will not resolve — held *whatever* asked. A held send is
/// the worst of both: it wedges the single backend slot, it opens no decision
/// pane (jobs never do), and the only recovery any surface named was
/// `:child --stop`, which `stop_child` refuses by design because it filters
/// `JobKind::Child`. So the fix is placement, not a new predicate: everything
/// that ends a job's tool call early now asks the same question the gate does
/// (`[[feedback_placement_not_predicate]]`).
#[tokio::test]
async fn an_attended_send_refuses_instead_of_wedging_on_it() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    for p in panes {
        app.workspace.toggle_mark(p);
    }
    app.submit("do the thing");
    let sends: Vec<GenerationId> = app.generation_ids_for_test();
    assert!(!sends.is_empty(), "premise: the marked prompt made jobs");
    let id = sends[0];
    let g = app.generations.iter().find(|g| g.id == id).unwrap();
    let buffer = g.buffer;
    let Work::Job(j) = &g.work else {
        panic!("premise: a job")
    };
    let episode = j.episode;

    // A tool we do not offer. `bash_command()` is `None`, so this lands on the
    // site that used to hold unconditionally.
    let block = shannon_core::Block::ToolUse {
        id: "tu-unknown".into(),
        name: "write_file".into(),
        // `Value::Null` — the input is irrelevant here; the NAME is what
        // makes `bash_command()` return `None`.
        input: Default::default(),
    };
    app.gate_job_tool_call(id, episode, block);

    assert!(
        app.job_of(id).is_none_or(|j| j.held.is_none()),
        "an attended send must not be left held: nobody can answer a job, and \
         the targeted stop refuses a send"
    );
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("write_file"),
        "and the refusal names the tool it turned down: {notice:?}"
    );
    // **And the round is CLOSED**, which is the half that keeps the slot
    // usable: a `tool_use` recorded with no `tool_result` is an orphan the API
    // rejects, so "did not hold" would be a hollow win if the call were simply
    // dropped. This asserts the answer exists, not that something is running.
    let answered = app
        .session_of(buffer)
        .expect("the send's buffer")
        .conversation
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .any(|b| {
            matches!(
                b,
                shannon_core::Block::ToolResult { tool_use_id, is_error, .. }
                    if tool_use_id == "tu-unknown" && *is_error
            )
        });
    assert!(
        answered,
        "the refused call is answered as an error, so the turn can carry on \
         rather than hanging on a `tool_use` nothing will ever reply to"
    );
    // **Recorded as a reopened turn draws it** (`replay_turn`): the call by
    // its tool and input, what the model was told under it — not the reason
    // in the command's place, with nothing under it (Steven, 11-01).
    let live = (app.session_of(buffer).unwrap().output.spans())
        .find(|s| s.role == crate::output::BlockRole::Tool)
        .map(|s| s.lines.to_vec())
        .expect("its record");
    let input = serde_json::Value::default();
    let shape = app.record_shape();
    let mut replayed = crate::tool_record::Record::call("refused", "write_file", &input, shape);
    replayed.output("an unknown tool: write_file");
    let (mut drawn, _) = replayed.into_parts();
    drawn.push(String::new()); // the separator a record carries
    assert_eq!(live[1..], drawn[1..], "under the header, as reopened");

    // **The other early ending, same rule.** A `{{a:…}}` naming no alias fails
    // `resolve_for_gate`, which was the second site above the fork. Two
    // operands rather than one, because a rule that agrees at n=1 is a
    // coincidence (`[[project_shannon_n_equals_two]]`) — and because the fix
    // was a *move*, so the site that no longer runs on this path must be shown
    // to be handled by the one it moved below.
    app.gate_job_tool_call_for_test(id, "cat {{a:no-such-alias}}");
    assert!(
        app.job_of(id).is_none_or(|j| j.held.is_none()),
        "an unresolvable handle refuses an attended send too, rather than \
         holding it where nobody can answer"
    );
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("no-such-alias"),
        "and says which handle it could not resolve: {notice:?}"
    );
}

/// **The two surfaces that tell you how to unwedge a held job say the same
/// thing.**
///
/// `hold_job`'s notice branched on `JobKind`; `Generation::busy_recovery` did
/// not, and hardcoded the child's answer — so a held send's notice said
/// `CTRL-C` while the refusal you got from trying to type into it said
/// `:child --stop <id>`, which refuses a send. One `JobWork::held_recovery` is
/// consulted by both now, and this asserts the AGREEMENT rather than either
/// string, so the test does not have to be rewritten when the wording improves
/// (`[[feedback_one_door_per_invariant]]`).
#[tokio::test]
async fn a_held_job_names_one_recovery_wherever_you_meet_it() {
    // A SEND held the only way an attended job still can: a duplicate
    // `tool_use` id, where holding is the deliberate ending (neither claimant
    // can be shown to be the impostor).
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    for p in panes {
        app.workspace.toggle_mark(p);
    }
    app.submit("do the thing");
    let id = app.generation_ids_for_test()[0];
    let buffer = app.generations.iter().find(|g| g.id == id).unwrap().buffer;
    // Twice with the same id: the first claims it, the second collides.
    app.gate_job_tool_call_for_test(id, "mkdir build");
    app.gate_job_tool_call_for_test(id, "mkdir build");

    let held = app.job_of(id).expect("still a job");
    assert!(held.held.is_some(), "premise: the duplicate id held it");
    let recovery = held.held_recovery();
    assert!(
        !recovery.contains(":child --stop"),
        "`:child --stop` refuses a send, and naming it is naming a refusal: \
         {recovery:?}"
    );
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains(&recovery),
        "the notice shows the one recovery: {notice:?} vs {recovery:?}"
    );
    let (_, how) = app
        .busy_reason_and_recovery(buffer)
        .expect("the held job holds the slot");
    assert!(
        how.contains(&recovery),
        "…and so does the refusal you get for typing into it: {how:?} vs \
         {recovery:?}"
    );

    // A CHILD, whose answer is the other one — so the test cannot pass by the
    // two surfaces agreeing on a single hardcoded string
    // (`[[project_shannon_n_equals_two]]`).
    let mut app = test_app();
    app.conversation_mut().begin_turn("hello");
    app.conversation_mut().push_chunk("hi");
    app.conversation_mut().commit_turn();
    let child = app.spawn_child_job("go", None).expect("spawns");
    let id = app.generation_ids_for_test()[0];
    let buffer = app.generations.iter().find(|g| g.id == id).unwrap().buffer;
    app.gate_job_tool_call_for_test(id, "mkdir build");

    let recovery = app.job_of(id).expect("a job").held_recovery();
    assert!(
        recovery.contains(&format!(":child --stop {}", child.short_hex())),
        "a child HAS a targeted stop, and it is named with the real id: \
         {recovery:?}"
    );
    let notice = app.notice().unwrap_or_default();
    assert!(notice.contains(&recovery), "notice: {notice:?}");
    let (_, how) = app
        .busy_reason_and_recovery(buffer)
        .expect("the held child holds the slot");
    assert!(how.contains(&recovery), "refusal: {how:?}");
}

/// **A background turn's ending is reported, named, and points somewhere that
/// works.**
///
/// Three defects in one sentence's worth of code. `unaccounted_output` ran only
/// on the foreground path, so a `:Send` billed for output it never showed said
/// nothing at all — and the incident that made the check exist *was* a `:Send`.
/// `truncation_notice` did fire for jobs, and named `:continue`, which acts on
/// the session the prompt is aimed at rather than the episode that just ended.
/// And with several jobs running, N unattributed notices name no session to act
/// on.
#[tokio::test]
async fn a_jobs_ending_is_reported_with_its_name_and_a_recovery_that_applies() {
    use shannon_core::StopReason;
    let mut app = test_app();
    app.conversation_mut().begin_turn("hello");
    app.conversation_mut().push_chunk("hi");
    app.conversation_mut().commit_turn();
    // `spawn_child_job` hands back the child's EPISODE, which is what
    // `feed_job` addresses.
    let child = app.spawn_child_job("go and look", None).expect("spawns");
    let label = app
        .generations
        .iter()
        .find(|g| matches!(&g.work, Work::Job(j) if j.episode == child))
        .expect("running")
        .label
        .clone();

    app.feed_job(
        child,
        StreamMsg::Usage(Usage {
            input_tokens: 100,
            output_tokens: 4096,
            ..Default::default()
        }),
    );
    app.feed_job(child, StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_job(child, StreamMsg::Complete);

    let notice = app.notice().expect("a job that was cut off says so");
    assert!(
        notice.contains(&label),
        "N jobs can be running, so the notice names which one: {notice:?} vs \
         {label:?}"
    );
    assert!(
        notice.contains(":open"),
        "`:continue` follows the prompt's aim, not the episode that ended, so \
         the recovery has to name getting there first: {notice:?}"
    );
}

/// **The other door onto the same theft: a reused id must not consume the
/// USER's own call.**
///
/// `a_child_cannot_claim_the_id_of_a_users_own_command` closed the job door,
/// where a collision HOLDS. This is the `OnDuplicate::Refuse` door — the
/// foreground and `:tool` — and it was still open, for a reason that is the
/// whole subject of this refactor: the guard asked *"does an owner exist for
/// this id"*, and a manual call has no owner **by definition**, so it took the
/// branch meant for ids nobody holds and called `finish_tool_call` on somebody
/// else's live call.
///
/// The consequence was the same HIGH-severity one (Reviewer E): finishing
/// consumed the flag that makes a `:tool` result undeliverable, so when the
/// user's command actually completed, the output of a command they approved
/// *because it stays local* was packaged as a `tool_result` and sent.
///
/// The map could not represent the case, so the question could not be asked.
/// Asking whether the CALL exists — rather than whether an owner was recorded
/// for it — is the same question without the hole
/// (`[[feedback_features_split_identities]]`).
#[tokio::test]
async fn a_reused_id_cannot_consume_the_users_own_call() {
    let mut app = test_app();
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    let mine = manual_tool(&mut app, "echo secret-user-output");
    assert!(
        app.tool_call(&mine).is_some_and(|c| c.owner.is_none()),
        "premise: the user's call is tracked, and owned by nobody"
    );

    // Something else claims the same id through the door that REFUSES.
    assert!(
        !app.arrive_tool_call(&mine, OnDuplicate::Refuse),
        "premise: the collision is refused"
    );

    assert!(
        app.tool_call(&mine).is_some_and(|c| c.owner.is_none()),
        "the refusal finished the USER's call out from under it — after which \
         its output is deliverable, and a local-only command's result goes to a \
         model"
    );
    assert!(
        app.notice().is_some_and(|n| n.contains(&mine)),
        "and the user is told, naming the id"
    );
}

/// **A QUEUED tool call is alias-expanded twice.**
///
/// `gate_tool_call` resolves aliases at the top — deliberately, so what the
/// analyzer classifies, the human reads, and the shell runs are one text — and
/// then pushes the **resolved** command onto `tool_queue`. The drain hands that
/// text back to `gate_tool_call`, which resolves it again. So the second call
/// of any assistant message that makes two is judged, shown, and run as text no
/// single expansion produces.
///
/// Third instance of this class today: the job gate had it (fixed by moving the
/// attended fork above the resolution), and this is the queue. The cause is the
/// same one the `ToolCall` collapse is about — the command text is carried by
/// whichever container currently holds the call, so it is re-processed every
/// time it moves.
#[tokio::test]
async fn a_queued_tool_call_is_expanded_once_not_twice() {
    use crate::permission_view::ToolAction;
    let mut cfg = Config::default();
    cfg.alias = vec![
        shannon_core::config::AliasSpec {
            name: "outer".into(),
            value: Some("{{a:inner}}".into()),
            env: None,
            sensitive: Some(false),
        },
        shannon_core::config::AliasSpec {
            name: "inner".into(),
            value: Some("SECOND-PASS".into()),
            env: None,
            sensitive: Some(false),
        },
    ];
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.submit("do two things");
    // Two calls in one message: the first takes the pane, the second queues.
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu-1",
        "mkdir one",
    )));
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu-2",
        "mkdir {{a:outer}}",
    )));
    app.feed_stream(StreamMsg::Complete);
    assert!(
        app.shown_call()
            .is_some_and(|p| p.command().contains("mkdir one")),
        "premise: the first call owns the pane"
    );

    // Answering the first hands the pane to the queued one.
    app.resolve_shown_for_test(ToolAction::Deny);
    let shown = app
        .shown_call()
        .map(|p| p.command().to_string())
        .expect("premise: the queued call took the pane");
    assert!(
        shown.contains("{{a:inner}}"),
        "one pass: `outer` expands to literal text, not to a handle to expand \
         again — got {shown:?}"
    );
    assert!(
        !shown.contains("SECOND-PASS"),
        "two passes resolved a handle nobody wrote, so the line the user is \
         asked to approve is not the line the model asked for: {shown:?}"
    );
}

/// **Holding a job withdraws its question, and the next one takes the pane.**
///
/// An attended `:Send` asks through the decision pane; a duplicate `tool_use`
/// id then holds the job. The job's first question used to stay on the pane,
/// and `a` on it really started `mkdir build` for a job that will never read
/// the result (probed 2026-09-24). Now it is withdrawn with a `cancelled`
/// record, and the `:tool` the user queued behind it is presented — the
/// promotion only an ANSWER used to do, so a withdrawal left the queue with
/// nothing on screen to answer it.
#[tokio::test]
async fn holding_a_job_withdraws_its_question_and_promotes_the_next() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    for p in panes {
        app.workspace.toggle_mark(p);
    }
    app.submit("do the thing");
    let id = app.generation_ids_for_test()[0];
    app.gate_job_tool_call_for_test(id, "mkdir build");
    assert_eq!(
        app.shown_call().map(|p| p.command().to_string()),
        Some("mkdir build".to_string()),
        "premise: the job's question is on the pane"
    );
    let mine = manual_tool(&mut app, "frobnicate --mine");
    assert!(
        app.waiting_calls().iter().any(|(q, _)| *q == mine),
        "premise: the user's own :tool queued behind it: {:?}",
        app.waiting_calls()
    );

    let logs = captured_logs(|| app.gate_job_tool_call_for_test(id, "mkdir build"));
    assert!(
        app.job_of(id).is_some_and(|j| j.held.is_some()),
        "premise: the duplicate id held it"
    );
    assert!(
        logs.lines().any(
            |l| l.contains(r#"decision="cancelled""#) && l.contains(r#"command="mkdir build""#)
        ),
        "the withdrawn question is on the record:\n{logs}"
    );
    assert_eq!(
        app.shown_call().map(|p| p.tool_use_id().to_string()),
        Some(mine),
        "the queued :tool is presented, not stranded"
    );
    // The job's question is gone, so nothing the user presses can run it.
    tool_key(&mut app, 'a');
    assert!(
        !app.running_commands().contains(&"mkdir build"),
        "a held job's command must not run: {:?}",
        app.running_commands()
    );
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("withdrawn"),
        "and the vanished pane is explained: {notice:?}"
    );
}
