//! Teardown: every kind of work, every way it can end.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// ---- one door for "everything running on this buffer" ----------------------
// Reviewer D's matrix: four kinds of work, six functions that must end them,
// thirty cells, no two columns alike. These are the two cells that were still
// open, both reproduced before being fixed.

/// Composing over a buffer ends the DISTILLATION that names it.
///
/// `S` in `:graph`, then `c`. `adopt_session` replaced the conversation while
/// `pending_summary` still pointed at that buffer, so the draft composed over
/// the clone — and a lazy clone leaves no trace, so it vanished silently.
#[tokio::test]
async fn composing_over_a_buffer_ends_the_distillation_on_it() {
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
    assert!(
        app.distilling().is_some(),
        "premise: a distillation is running"
    );

    // Over the buffer the distillation names (`S` started on the aim).
    app.supersede_for_test(app.prompt_target());

    assert!(
        app.distilling().is_none(),
        "the distillation went with the session it was composing into"
    );
}

/// Deleting a session ends the distillation on it, so the slot is released.
///
/// `S` then `d` — the sequence `ask`'s own doc cites as why `pending_confirm`
/// became a queue. `reset_buffer` covered the foreground, the jobs and the
/// prompt queue and left the summary holding the slot, so every later prompt
/// queued in silence and `$EDITOR` opened unbidden minutes later.
#[tokio::test]
async fn deleting_a_session_ends_the_distillation_on_it() {
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
    let buffer = app.current;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(branch);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
    assert!(
        app.distilling().is_some(),
        "premise: a distillation is running"
    );

    app.reset_buffer(buffer);

    assert!(
        app.distilling().is_none(),
        "the distillation went with the session it belonged to"
    );
    assert!(
        !app.slot_busy_for(buffer),
        "and the slot is free, so later prompts do not queue in silence"
    );
}

/// A prefill continuation ends with the session it was composing into — the
/// **other half** of the two data-loss paths round 5 reproduced.
///
/// D's report named both (`S` then `c`, and `:continue --prefill` then `c`) and
/// the fix covered both, but only the distillation half got tests. The two are
/// now the same code path by construction — `end_work_on` loops rather than
/// enumerating — and that identity is exactly what wants pinning: a shared path
/// is worth less than nothing if nobody checks that both kinds are on it
/// (`[[feedback_doc_comments_are_claims]]`).
///
/// Without it, `finish_continuation` composes over the clone. A lazy clone
/// leaves no trace, so it vanishes silently.
#[tokio::test]
async fn composing_over_a_buffer_ends_the_continuation_on_it() {
    let (mut app, _dir) = app_mid_prefill_continuation().await;
    assert!(app.continuing(), "premise: a continuation is running");

    app.supersede_for_test(app.current);

    assert!(
        !app.continuing(),
        "the continuation went with the session it was composing into"
    );
}

/// Deleting a session ends the continuation on it, so the slot is released.
/// The sibling of `deleting_a_session_ends_the_distillation_on_it`, and the
/// same silence if it regresses: every later prompt queues with no message.
#[tokio::test]
async fn deleting_a_session_ends_the_continuation_on_it() {
    let (mut app, _dir) = app_mid_prefill_continuation().await;
    let buffer = app.current;
    assert!(app.continuing(), "premise: a continuation is running");

    app.reset_buffer(buffer);

    assert!(
        !app.continuing(),
        "the continuation went with the session it belonged to"
    );
    assert!(
        !app.slot_busy_for(buffer),
        "and the slot is free, so later prompts do not queue in silence"
    );
}

/// `CTRL-C` stops a prefill continuation, and says so.
///
/// The sibling of `ctrl_c_stops_a_distillation`. Before `stop_work` knew about
/// all the slot-holders, a continuation left the key with nothing to stop —
/// and `CTRL-C` used to *quit* when it found nothing, so this cell of the
/// matrix was "the program exits while a billable request runs on".
#[tokio::test]
async fn ctrl_c_stops_a_prefill_continuation() {
    let (mut app, _dir) = app_mid_prefill_continuation().await;
    assert!(app.continuing(), "premise: a continuation is running");

    // CTRL-C is a keypress, and a keypress is the start of a new moment — the
    // event loop clears what the last one said before dispatching. This test
    // calls `stop_work` directly, so it has to say where its moment begins or
    // it asserts against everything said since the app started
    // (`begin_moment`, and `a_keypress_begins_a_new_moment`).
    app.notices.clear();
    assert!(app.ctrl_c_stop_for_test(), "there was something to stop");

    assert!(!app.continuing(), "the continuation was cancelled");
    let notice = app.notice().expect("says what it stopped").to_string();
    assert!(
        notice.contains("continuation cancelled"),
        "names what it stopped: {notice}"
    );
    assert!(
        !notice.contains("`:continue` resumes it"),
        "and does NOT offer to resume: a cancelled continuation leaves no \
         interrupted turn, so `:continue` would refuse ({notice})"
    );
}

/// **A turn interrupted mid-decision keeps what it billed.**
///
/// `handle_turn_msg`'s `Error` arm called `abandon_tool_round()` first, and
/// that cleared any turn parked on a decision — so the `take_generation(id)`
/// below it returned `None`, `usage` fell back to `Usage::default()`, and the
/// sealed turn recorded that a hundred billed input tokens had cost nothing.
/// Reopening the session then recomputed the real number from provenance: one
/// session, two numbers (`[[project_shannon_message_honesty]]`).
///
/// Reachable because **`Error` can follow `Complete` on one request** — a fact
/// I asserted the opposite of, and Reviewer A refuted by reading
/// `anthropic.rs`: `run_turn` sets `completed` on `message_stop` and keeps
/// draining the byte stream, so a connection reset or a stray trailing byte
/// after it yields exactly that pair. When the `Complete` carried tool calls,
/// the turn is parked, which is this test.
#[tokio::test]
async fn an_error_on_a_turn_parked_on_a_decision_keeps_what_it_billed() {
    let mut app = test_app();
    let spent = Usage {
        input_tokens: 100,
        output_tokens: 20,
        ..Default::default()
    };
    app.submit("do a thing");
    app.feed_stream(StreamMsg::Usage(spent));
    // A gating command, so `Complete` parks the turn instead of sealing it.
    model_call(&mut app, "tu-1", "rm -rf /tmp/shannon-test-absent-billing");
    assert!(app.awaiting_decision(), "premise: parked on a decision");

    app.feed_stream(StreamMsg::Error(BackendError::Transport(
        "connection reset".into(),
    )));

    assert_eq!(
        app.buffer().billing.total,
        spent,
        "the tokens were spent whatever happened to the reply"
    );
}

/// **A killed command's record lands in the session that ran it.**
///
/// `cancel_running_tools` wrote through `turn_buffer()`, whose
/// `map_or(self.current, …)` fallback fires exactly when there is no
/// generation — and `resolve` takes the generation out of the
/// collection *before* calling `abandon_tool_round`, deliberately, so that
/// nothing can end it twice. So the fallback fired on every teardown.
///
/// What the user saw: `:open` another session while a command is running,
/// press `CTRL-C`, and `⚙ cancelled: …` appears in the session you just
/// opened — a machine action filed under a conversation that never asked for
/// it — while the session that *did* run it keeps `⚙ running: …` as its last
/// word forever. That is the exact ambiguous-record trap `cancel_running_tools`
/// says it exists to prevent (Reviewer A, reproduced).
///
/// `CTRL-C` from the other session is that session's now (§9 phase 6b), so
/// it stops nothing of A's and says where A's work runs; what still ends a
/// turn from in front of another session is `:qa!`, which stops them all.
#[tokio::test]
async fn a_killed_commands_record_lands_in_the_session_that_ran_it() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _sibling, focused) = app_with_two_sessions(dir.path());
    app.workspace.focus_pane(focused);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;

    app.submit("run something");
    // `ls -la` is read-only, so it auto-approves and goes straight to running —
    // no decision to answer. Deliberately NOT settled: the command is still in
    // `running_tools`, which is what `CTRL-C` has to kill and account for.
    app.arrive_and_gate_for_test(app.foreground_id(), "tu-1", "ls -la");

    // Walk to the other session mid-command. This is the whole point of the
    // buffer model: it is an ordinary thing to do, and it must not move where
    // the turn's records land.
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.current;
    assert_ne!(a_buffer, b_buffer, "premise: the foreground really moved");

    app.notices.clear();
    app.interrupt();
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("running elsewhere") && said.contains(&app.session_label(a)),
        "CTRL-C in B stops nothing of A's, and says where it runs: {said}"
    );
    // First, so a narrow row's cut from the end keeps it (F 2, probed).
    assert!(said.starts_with("running elsewhere: "), "{said}");
    assert!(
        app.running_tools.contains_key("tu-1"),
        "A's command runs on"
    );
    assert!(app.leaving(true), "`:qa!` stops every session's work");

    let in_a = app
        .session_of(a_buffer)
        .expect("A is open")
        .output
        .lines()
        .join("\n");
    let in_b = app
        .session_of(b_buffer)
        .expect("B is open")
        .output
        .lines()
        .join("\n");
    assert!(
        in_a.contains("cancelled"),
        "the session that ran it is told it was killed: {in_a}"
    );
    assert!(
        !in_b.contains("cancelled"),
        "and the session that did not run it is told nothing: {in_b}"
    );
}

/// **Deleting a session with a job in flight must not put it back on disk.**
///
/// `Keep::Discard` exists for exactly this, and **nothing pinned it**: Reviewer
/// A replaced the Job arm's `Keep::Discard` early return with `if false` — so a
/// deleted session's job seals instead of being dropped — and all 812 tests
/// stayed green. The path it opens is worse than a stray turn:
/// `delete_episode` removes the episode from the store first, then `seal_job` →
/// `persist_job_turns` finds `EpisodeNotFound`, reads that as "never written",
/// and takes the **register** branch — re-creating the header and every turn.
/// The session the user just confirmed deleting is back, with one extra
/// `interrupted` turn, on the next `:graph` rebuild or relaunch.
///
/// A "delete" that does not delete is also a confidentiality failure: the user
/// was told it was gone.
#[tokio::test]
async fn deleting_a_session_with_a_job_in_flight_does_not_resurrect_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("go", None).expect("spawned");
    // Something billed and streamed, so there is a partial worth sealing —
    // otherwise `interrupt_turn` has nothing to write and the test passes for
    // the wrong reason.
    app.feed_job(child, StreamMsg::Chunk("partial answer".into()));
    assert!(
        app.store.load_episode(&child).is_ok(),
        "premise: the child's header is on disk (spawn persists it)"
    );

    app.delete_episode(child);

    assert!(
        matches!(
            app.store.load_episode(&child),
            Err(shannon_core::StoreError::EpisodeNotFound(_))
        ),
        "the deleted session stayed deleted"
    );
    assert!(
        !app.job_episodes().contains(&child),
        "and its job went with it rather than streaming into a gap"
    );
}

/// **`Outcome::seals()` is pinned where it decides something: a `:Send` into
/// a session never written to disk, deleted mid-reply.**
///
/// The child test above cannot pin it: a child is on disk from spawn
/// (`OnDisk::Own(0)`), so a seal APPENDS, the append to a deleted episode
/// fails, and the episode stays deleted either way — `seals() -> true` left
/// all 1754 tests green (review 2026-10-15, C F2). Here nothing was ever
/// written (`OnDisk::Absent`), so a seal takes the REGISTER branch and writes
/// the deleted session back, with the partial in it: a delete that does not
/// delete.
#[tokio::test]
async fn deleting_a_never_written_session_with_a_send_in_flight_does_not_resurrect_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 0);
    let session = app.conversation().episode().id;
    assert!(
        app.store.load_episode(&session).is_err(),
        "premise: never written"
    );
    let current = app.current;
    app.start_job_on(current, "go").expect("a :Send into it");
    app.feed_job(session, StreamMsg::Chunk("partial answer".into()));
    assert!(
        app.job_episodes().contains(&session),
        "premise: the reply is streaming"
    );

    app.delete_episode(session);

    assert!(
        matches!(
            app.store.load_episode(&session),
            Err(shannon_core::StoreError::EpisodeNotFound(_))
        ),
        "the deleted session stayed deleted"
    );
}

/// **Deleting a session with a live turn must not then offer to resume it.**
/// The `Keep::Discard` twin of the test above, for `Work::Turn`.
///
/// Also unpinned: dropping the `if keep == Keep::Seal` guard left the suite
/// green. But finding what the guard actually *protects* took two wrong
/// guesses, and both are worth recording because each is a vacuity shape:
///
/// - **Guess 1, on the conversation.** A seal writes into the OLD conversation,
///   which `reset_buffer` throws away one line later — invisible by
///   construction. Asserting on the replacement's `own_len()` measures nothing.
/// - **Guess 2, on disk.** Unlike the job twin, this does not resurrect the
///   episode: `persist_committed_turn_on` takes the *append* branch (the
///   session was already registered), and an orphan turn record does not make
///   `load_episode` succeed again. Probed both ways — the files are
///   byte-identical.
///
/// What actually differs is the **message**. Without the guard the user deletes
/// a session and is told *"the turn you left was interrupted — `:continue`
/// resumes it"* about a session that no longer exists, and `:continue` would
/// refuse. That is the message-honesty rule: never describe a recovery the app
/// cannot perform (`[[project_shannon_message_honesty]]`). So the guard is real,
/// its consequence is user-visible, and it is smaller than Reviewer A's framing
/// suggested — worth saying, because "must not `interrupt_turn` into the
/// deleted episode" would send the next reader looking on disk.
#[tokio::test]
async fn deleting_a_session_with_a_live_turn_does_not_offer_to_resume_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let doomed = app.conversation().episode().id;
    let buffer = app.current;

    app.submit("a turn that will not survive");
    app.feed_stream(StreamMsg::Chunk("half an answer".into()));
    assert!(app.streaming(), "premise: a turn is in flight");

    app.delete_episode(doomed);

    let notice = app.notice().unwrap_or_default().to_string();
    assert!(
        !notice.contains(":continue"),
        "a deleted session's turn is not resumable, so nothing may say it is: \
         {notice:?}"
    );
    assert!(
        app.turns().next().is_none(),
        "the turn went with the session"
    );
    assert!(
        !app.slot_busy_for(buffer),
        "and the slot is free rather than held by a turn with nowhere to land"
    );
}

/// **Deleting a session leaves another session's work and queue alone** —
/// and the queue still drains when that session's own slot frees.
///
/// This was `deleting_a_session_starts_the_prompt_that_was_queued_behind_it`:
/// with one turn app-wide, B's prompt waited on A's turn, and deleting A had
/// to start it (Reviewer D, reproduced: `end_work_on`'s callers did not
/// drain). Sessions run apart now (`buffers.md` §9), so nothing of B's waits
/// on A, and the same-session case — a prompt queued behind its own
/// session's deleted turn — is dropped out loud
/// (`a_queued_prompt_for_a_deleted_session_is_dropped_out_loud`). What is
/// left to pin is the independence: A goes; B's turn runs on, B's prompt
/// stays queued behind it, and starts when B's turn ends (`settle`).
#[tokio::test]
async fn deleting_a_session_leaves_another_sessions_queue_alone() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());
    app.workspace.focus_pane(focused);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    app.submit("ask A");
    app.workspace.focus_pane(sibling);
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.current;
    assert_ne!(a_buffer, b_buffer, "premise: two buffers");
    app.submit("ask B");
    app.submit("ask B again — queued behind B's own turn");
    assert_eq!(app.turns().count(), 2, "premise: A and B both answering");
    assert_eq!(app.pending.len(), 1, "premise: queued for B");

    app.delete_episodes(&[a]);
    app.settle();
    let b_turn = app
        .turns()
        .find(|g| g.buffer == b_buffer)
        .map(|g| g.id)
        .expect("B's turn runs on");
    assert_eq!(
        app.pending.front().map(|(buffer, _)| *buffer),
        Some(b_buffer),
        "B's prompt still waits, for B"
    );

    app.feed_turn(b_turn, StreamMsg::Complete);
    assert!(app.pending.is_empty(), "B's own slot freed: it started");
    assert!(
        app.turns().any(|g| g.buffer == b_buffer),
        "as B's next turn"
    );
}

/// **A batch delete starts nothing it is about to delete** (review 2026-10-15,
/// Reviewer C F5).
///
/// A's turn runs and a prompt waits for B; the user deletes both.
/// `reset_buffer` drained per holder, so resetting a session freed a slot, a
/// waiting prompt was SENT — billed — and its own session's reset ended it,
/// under "the queued prompt is sent" and never the honest "dropped". Both
/// batches: `:delete` of the marked pair, and `:delete all` with A in front.
///
/// The prompt waited on A's turn, the one turn app-wide; since sessions run
/// apart (`buffers.md` §9) it waits behind B's own turn, which the batch
/// ends too — the same hazard, through B's slot.
#[tokio::test]
async fn a_batch_delete_starts_nothing_it_is_about_to_delete() {
    for all in [false, true] {
        let dir = TempDir::new().unwrap();
        let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());
        app.workspace.focus_pane(sibling);
        app.open_by_hash(&b.to_hex()[..8]);
        let b_buffer = app.current;
        app.workspace.focus_pane(focused);
        app.open_by_hash(&a.to_hex()[..8]);
        let a_buffer = app.current;
        assert_ne!(a_buffer, b_buffer, "premise: two buffers");
        app.submit("ask A");
        assert!(app.streaming(), "premise: A answering, and in front");
        app.submit_to(b_buffer, "ask B");
        app.submit_to(b_buffer, "ask B again");
        assert_eq!(app.pending.len(), 1, "premise: B's second prompt is queued");

        if all {
            app.delete_all_sessions(&[]);
        } else {
            app.delete_episodes(&[a, b]);
        }

        let said = app.notice().unwrap_or_default();
        assert!(app.pending.is_empty(), "all={all}: nothing left queued");
        assert!(app.generations.is_empty(), "all={all}: nothing running");
        // Said in the delete's sentence, beside what it was for: per session
        // for `:delete`, across the buffers for `:delete all`.
        let dropped = match all {
            false => "(with a turn in flight and 1 queued prompt)",
            // Counted across the buffers, alike with alike (`Lost`): it read
            // "a turn in flight, 1 queued prompt and a turn in flight".
            true => {
                "stored sessions (with 2 turns in flight and 1 queued prompt) \
                 — every open session is now a fresh empty one"
            }
        };
        assert!(said.contains(dropped), "all={all}: said, not sent: {said}");
        assert!(!said.contains("is sent"), "all={all}: {said}");
        if !all {
            assert!(
                said.contains("fresh empty sessions took their places in their buffers"),
                "two replaced, said of two: {said}"
            );
        }
    }
}

/// **A busy session does not hold back a free one's queued prompt**
/// (review 2026-10-15, Reviewer C F6 — its probe P4).
///
/// B has a `:Send` reply running and a prompt queued behind it; A's turn
/// holds A, with a prompt queued behind THAT. A's turn ends. The drain
/// looked only at the queue's front — B's, still busy — and returned, so A sat
/// idle beside "queued (2)" until B's job happened to end.
#[tokio::test]
async fn a_busy_session_does_not_hold_back_a_free_sessions_queued_prompt() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());
    app.workspace.focus_pane(sibling);
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.current;
    app.workspace.focus_pane(focused);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    assert_ne!(a_buffer, b_buffer, "premise: two buffers");

    app.start_job_on(b_buffer, "a reply for B")
        .expect("a job on B");
    app.submit_to(b_buffer, "for B");
    app.submit("ask A");
    assert!(app.streaming(), "premise: A's turn runs beside B's job");
    app.submit_to(a_buffer, "next for A");
    let queued: Vec<BufferId> = app.pending.iter().map(|&(b, _)| b).collect();
    assert_eq!(
        queued,
        [b_buffer, a_buffer],
        "premise: B's prompt is in front"
    );

    app.feed_stream(StreamMsg::Chunk("reply A".into()));
    app.feed_stream(StreamMsg::Complete);

    assert_eq!(
        app.pending
            .iter()
            .map(|(b, t)| (*b, t.as_str()))
            .collect::<Vec<_>>(),
        [(b_buffer, "for B")],
        "B's prompt still waits on B's job; A's went"
    );
    assert_eq!(app.turns().next().map(|g| g.buffer), Some(a_buffer));
    let last = app
        .session_of(a_buffer)
        .and_then(|b| b.conversation.messages().last())
        .map(|m| m.content.clone());
    assert_eq!(last.as_deref(), Some("next for A"));
}

/// **A new prompt never overtakes one already queued for its session**
/// (review 2026-10-15, Reviewer C F6).
///
/// The state is built by hand — a free slot with a prompt still waiting on it.
/// Since every refill is the event's (`settle`), that is an ordinary state
/// between a release and its event's end; a prompt submitted in it queues
/// behind the earlier one, and the settle starts the earlier one first.
#[tokio::test]
async fn a_new_prompt_never_overtakes_one_queued_for_its_session() {
    let mut app = test_app();
    let target = app.prompt_target();
    app.pending.push_back((target, "typed first".into()));
    assert!(
        app.busy_reason(target).is_none(),
        "premise: the slot is free"
    );

    app.submit("typed second");
    app.settle(); // the key's event ends

    assert_eq!(
        app.pending
            .iter()
            .map(|(b, t)| (*b, t.as_str()))
            .collect::<Vec<_>>(),
        [(target, "typed second")],
        "the second waits"
    );
    let last = app
        .conversation()
        .messages()
        .last()
        .map(|m| m.content.clone());
    assert_eq!(last.as_deref(), Some("typed first"), "the first went first");
    assert!(app.streaming());
}

/// **A marked `<Enter>` never overtakes a prompt queued for that session**
/// (review 2026-10-16, Reviewer A F4, its probe 3). "first for B" queues;
/// marking B and sending "second for B" went through `start_job_on`, which
/// asked `conflict` alone — B got the user's words out of order. Now it
/// refuses, saying the earlier one goes first.
///
/// What holds B is a `:tool` typed there and waiting on its decision — no
/// work on B, so `conflict` passed and the queue was what refused, until the
/// waiting call itself held a send as it holds a prompt (review stop 8, A 9).
/// The queue still refuses between the call answered and the event's end,
/// where the prompt it holds starts. (It was A's turn, which held every
/// session's slot until sessions ran apart.)
#[tokio::test]
async fn a_send_never_overtakes_a_prompt_queued_for_its_session() {
    let b_said = |app: &App, b_buffer: BufferId| -> Vec<String> {
        (app.session_of(b_buffer).into_iter())
            .flat_map(|b| b.conversation.messages().iter().map(|m| m.content.clone()))
            .collect()
    };
    // A's prompt, B's `:tool` asking, "first for B" queued behind it, B's
    // pane marked, and a marked send refused — the moment both halves below
    // go on from.
    let stand = |dir: &std::path::Path| {
        let (mut app, a, b, _, _) = app_with_two_sessions(dir);
        let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
        let (here, there) = (panes[0], panes[1]);
        app.focus_pane(there);
        app.open_by_hash(&b.to_hex()[..8]);
        let b_buffer = app.current;
        manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent");
        assert!(app.shown_call().is_some(), "premise: B's `:tool` is asking");
        app.focus_pane(here);
        app.open_by_hash(&a.to_hex()[..8]);
        app.submit_to(b_buffer, "first for B");
        assert_eq!(
            app.pending.len(),
            1,
            "premise: queued for B, behind its decision"
        );
        assert!(
            app.workspace.set_mark(there, true),
            "premise: B's pane marked"
        );
        app.submit("second for B");
        assert!(
            !b_said(&app, b_buffer).iter().any(|m| m == "second for B"),
            "not ahead of the first"
        );
        assert_eq!(
            (app.pending.iter())
                .map(|(b, t)| (*b, t.as_str()))
                .collect::<Vec<_>>(),
            [(b_buffer, "first for B")],
            "the first still waits its turn"
        );
        let said = app.notice().unwrap_or_default();
        assert!(
            said.contains("a command is awaiting your decision"),
            "{said}"
        );
        (app, b_buffer)
    };

    // B's call answered, and the event ended as the loop ends it: the
    // prompt queued for B starts — it went first — and the send behind it
    // waits on that turn. (This asserted the state between the answer and
    // the event's end, which no keypress reaches: the helper did not end
    // the event — review stop 12, T1 4.)
    let dir = TempDir::new().unwrap();
    let (mut app, b_buffer) = stand(dir.path());
    tool_key_in(&mut app, b_buffer, 'd');
    assert_eq!(
        b_said(&app, b_buffer).last().map(String::as_str),
        Some("first for B"),
        "the first went first"
    );
    app.notices.clear();
    app.submit("second for B");
    assert!(
        !b_said(&app, b_buffer).iter().any(|m| m == "second for B"),
        "not ahead of it"
    );
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("a turn is in flight"), "{said}");

    // **The queue's own refusal, defence in depth**: between the answer and
    // the event's end nothing holds B but the prompt queued for it. No key
    // reaches that moment — the settle is the event's end — so it is driven
    // here by hand, unsettled, to hold the guard should the order ever move.
    let dir = TempDir::new().unwrap();
    let (mut mid, b_buffer) = stand(dir.path());
    to_decision_of(&mut mid, b_buffer);
    mid.handle_tool_key(
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        (80, 100),
    );
    mid.notices.clear();
    mid.submit("second for B");
    let said = mid.notice().unwrap_or_default();
    assert!(said.contains("goes first"), "{said}");
}

/// **A session its review holds refuses a `:Send`, and `:qa` names the
/// draft** (review 2026-10-16, Reviewer A F5, of the question this was).
/// "Unreachable while the question is up" was false: a `:` line left open
/// when "apply it?" arrived kept its keys, so `:Send` started a job on the
/// held session (which `y` then superseded), and `:qa` quit over the billed
/// draft without a word.
#[tokio::test]
async fn a_held_session_refuses_a_send_and_quitting_names_the_draft() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let held = app.current;
    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);
    app.feed_summary(StreamMsg::Chunk("the gist".into()));
    app.feed_summary(StreamMsg::Complete);
    assert!(app.decisions.review(held).is_some(), "premise: its review");

    assert!(
        app.start_job_on(held, "sneak in")
            .is_err_and(|why| why.contains("waiting for your review")),
        "a job on the held session is refused, saying why"
    );
    assert!(app.generations.is_empty(), "nothing started");
    let ends = app.quitting_ends();
    assert!(
        ends.iter().any(|e| e.contains("summary draft")),
        ":qa names the draft it would discard: {ends:?}"
    );
    assert!(app.leaving(true), "premise: `:qa!` leaves");
    assert!(app.decisions.review(held).is_none(), "and takes it with it");
}

/// **A turn ending must not kill a command the USER started.**
///
/// `cancel_running_tools` drained the whole `running_tools` map, so an
/// unrelated background event — a backend error, `CTRL-C` on the reply — also
/// killed a `:tool` command the user had launched themselves. Their own
/// foreground action, cancelled by something they were not doing (Reviewer E).
///
/// `manual_tool_ids` is the distinction every other consumer of that map
/// already makes; this was the one that did not.
#[tokio::test]
async fn a_turns_teardown_leaves_a_manual_command_running() {
    let mut app = test_app();
    // A `:tool` command of the user's own, with no turn running.
    manual_tool(&mut app, "echo hi");
    assert_eq!(
        app.running_tools.len(),
        1,
        "premise: the user's command is running"
    );

    // Now a turn, which then ends badly.
    app.submit("meanwhile, ask something");
    app.feed_stream(StreamMsg::Error(BackendError::RateLimited));

    assert_eq!(
        app.running_tools.len(),
        1,
        "the user's own command survived a turn it had nothing to do with"
    );
}

/// **A prompt submitted while a distillation runs says so.**
///
/// `submit` carried private arms for a turn, a job and a decision — three of
/// the five things that can hold the slot — so submitting during an `S` or a
/// prefill `:continue` fell through to "a turn is already in flight", about
/// something that is not a turn. Two of those arms reproduced `busy_phrase`'s
/// wording verbatim, which is the tell that the extraction had a consumer it
/// never reached (Reviewer D).
#[tokio::test]
async fn a_prompt_queued_behind_a_distillation_names_the_distillation() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let t = app.conversation().episode().own_turns()[0];
    let target = SquashTarget::Gather {
        episode: app.conversation().episode().id,
        turn: t,
        index: 0,
        config: app.conversation().episode().config.clone(),
    };
    app.fake_summary_on(app.current, target, vec![t]);

    app.submit("a question typed while it distills");

    assert_eq!(
        app.pending.len(),
        1,
        "premise: it queued rather than sending"
    );
    let notice = app.notice().expect("says why it queued").to_string();
    assert!(
        notice.contains("distilling"),
        "names the distillation: {notice}"
    );
    assert!(
        !notice.contains("a turn is already in flight"),
        "and does not call it a turn: {notice}"
    );
}

/// A prompt queued behind a **decision** is told to answer it — the one case
/// where the actionable sentence differs from "a turn is in flight", because
/// the turn is waiting on the user rather than on the backend.
#[tokio::test]
async fn a_prompt_queued_behind_a_decision_says_a_decision_is_waiting() {
    let mut app = test_app();
    model_call(&mut app, "tu-1", "rm -rf /tmp/shannon-test-absent-queue");
    assert!(app.awaiting_decision(), "premise: parked on a decision");

    app.submit("something else");

    let notice = app.notice().expect("says why it queued").to_string();
    assert!(
        notice.contains("awaiting your decision"),
        "the user is told what to DO, not merely that something is happening: \
         {notice}"
    );
}

/// `:child --stop <id>` acts on **children**, and says so when the id is not one.
///
/// It filtered on `Work::Job` alone, so it silently retired a `:Send` reply —
/// a command named `:child`, under a flag documented as "cancel a running
/// child", acting on the thing the manual spends a chapter saying is not one
/// (Reviewer C, who found it untested in *either* direction).
///
/// The second half matters as much: refusing with "nothing running matches"
/// about an id that IS running would send the user looking for the wrong
/// problem.
#[tokio::test]
async fn child_stop_acts_on_children_and_names_a_send_as_a_send() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let episode = app.conversation().episode().id;
    let buffer = app.current;
    app.start_job_on(buffer, "a background reply")
        .expect("free");
    assert_eq!(app.running_sends(), 1, "premise: a :Send is running");

    app.stop_child(&episode.to_hex()[..8]);

    assert_eq!(
        app.running_sends(),
        1,
        "the `:Send` is untouched by a command that acts on children"
    );
    let notice = app.notice().expect("a visible refusal").to_string();
    assert!(
        notice.contains(":Send"),
        "and the refusal names what it actually is: {notice}"
    );
    assert!(
        !notice.contains("nothing running matches"),
        "rather than claiming nothing matched an id that plainly did: {notice}"
    );
}

/// **A job that finishes normally commits; one that is interrupted does not.**
///
/// The `(Work::Job, Outcome)` row maps `Finished` to `JobEnd::Finished` and
/// everything else to `Interrupted`, and **that mapping was pinned by nothing**:
/// forcing every job down the `Interrupted` branch left all 834 tests green.
/// The two differ in the sealed turn's provenance, which is what `:continue`
/// and the inspector read — so a child that answered perfectly would have been
/// recorded as having been cut off.
#[tokio::test]
async fn a_job_that_finishes_is_not_recorded_as_interrupted() {
    // The job's own turn, landed: `pop()` alone read the seed turn when the
    // job's never landed (review stop 12, T3 15). And both halves of the
    // mapping — a failed job IS recorded as cut off.
    let ended = |end: StreamMsg| {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 1);
        let episode = app.conversation().episode().id;
        let buffer = app.current;
        let before = app.conversation().own_turns().len();
        app.start_job_on(buffer, "a question").expect("free");
        app.feed_job(episode, StreamMsg::Chunk("the job's answer".into()));
        app.feed_job(episode, end);
        let mut turns = app.conversation().own_turns();
        assert_eq!(turns.len(), before + 1, "premise: the job's turn landed");
        let last = turns.pop().expect("a turn");
        let text = (last.messages.iter().map(|m| m.content.clone())).collect::<Vec<_>>();
        assert!(
            text.iter().any(|t| t.contains("the job's answer")),
            "premise: and it is the job's: {text:?}"
        );
        last.provenance.stop_reason
    };
    assert_ne!(
        ended(StreamMsg::Complete),
        Some(shannon_core::StopReason::Interrupted),
        "a job that ran to completion is not marked as cut off — provenance is \
         what `:continue` and the inspector read"
    );
    assert_eq!(
        ended(StreamMsg::Error(BackendError::RateLimited)),
        Some(shannon_core::StopReason::Interrupted),
        "and one that failed is"
    );
}

/// **Superseding a session says the turn is resumable — and how, which is
/// not `:continue` where you stand.** Deleting it says neither.
///
/// Both are seals of the same partial; the sentences are not interchangeable,
/// and `seal_interrupted_turn` keys them on the `Outcome`. Asserting only the
/// negative half left the positive arm unpinned — deleting the "the turn you
/// left was interrupted" line entirely stayed green.
///
/// And asserting the sentence only MENTIONED `:continue` let it lie: the
/// buffer now shows the clone, which does not hold the partial, so the bare
/// `:continue` it offered answered "the last turn doesn't record how it ended"
/// (review 2026-10-15, probed). The sentence is followed here, the way a user
/// would follow it.
///
/// **What this does NOT pin, said plainly:** `Outcome::seals()` for a
/// `Work::Turn` on the delete path. That combination has *no* observable
/// effect — the conversation it writes into is discarded by `reset_buffer` a
/// line later, and `persist_committed_turn_on`'s append to an episode the store
/// has already dropped fails and is handled, leaving the files byte-identical
/// (probed both ways). So the property is over-determined: two guards, either
/// one sufficient, neither individually mutable. `seals()` is pinned instead by
/// a `:Send` into a session never written
/// (`deleting_a_never_written_session_with_a_send_in_flight_does_not_resurrect_it`),
/// where the same flag is the difference between a deleted session staying
/// deleted and being re-registered on disk.
#[tokio::test]
async fn superseding_offers_to_resume_the_turn_it_interrupted() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let base = app.conversation().episode().id;
    app.submit("a turn that a compose will land on");
    app.feed_stream(StreamMsg::Chunk("half an answer".into()));
    assert!(app.streaming(), "premise: a turn is in flight");
    // Its label BEFORE the clone: `b<N> <tag>`, whose `b<N>` is about to name
    // the clone instead (review 2026-10-16, C F3 — `contains("{tag}: ")` also
    // matched `b1 {tag}: `, so naming it that way passed).
    let label_then = app.session_label(base);
    let tag = app.session_tag(base);
    assert_ne!(
        label_then, tag,
        "premise: the label carries a buffer number"
    );

    app.supersede_for_test(app.current);
    assert_ne!(
        app.conversation().episode().id,
        base,
        "premise: the buffer shows the clone, which does not hold the partial"
    );

    let notice = app.notice().expect("says what happened").to_string();
    assert!(
        notice.contains(&format!("{tag}: ")) && notice.contains("`:open` it, then `:continue`"),
        "a superseded turn IS resumable, and the app says where: {notice}"
    );
    assert!(
        !notice.contains(&label_then),
        "named by its tag alone, not by the buffer that now holds the clone: {notice}"
    );
    // Do what it says.
    app.open_by_hash(&base.to_hex()[..8]);
    app.notices.clear();
    app.continue_last_turn();
    assert!(
        !app.generations.is_empty(),
        "and `:continue` resumes it there: {:?}",
        app.notice()
    );
}

/// **An ending says how to resume it from where you are** (review
/// 2026-10-15, Reviewer C F4 — its probe P3).
///
/// `:continue` acts on the aimed session. The three endings of a turn that
/// offer it — the length limit, a failure, CTRL-C — said "`:continue` resumes
/// it" wherever the aim was; with the aim moved to B while A ran (an ordinary
/// thing to do), `:continue` then answered about B, or resumed B, a billed
/// turn on the wrong conversation. Each ending, aimed and not, and each
/// sentence FOLLOWED: the user does what it says, and A resumes.
#[tokio::test]
async fn an_ending_says_how_to_resume_it_from_where_you_are() {
    use shannon_core::StopReason;
    #[derive(Debug, Clone, Copy)]
    enum End {
        Length,
        Failure,
        Interrupt,
    }
    for end in [End::Length, End::Failure, End::Interrupt] {
        for aimed in [true, false] {
            let dir = TempDir::new().unwrap();
            let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());
            app.workspace.focus_pane(sibling);
            app.open_by_hash(&b.to_hex()[..8]);
            let b_buffer = app.current;
            app.workspace.focus_pane(focused);
            app.open_by_hash(&a.to_hex()[..8]);
            let a_buffer = app.current;
            app.submit("ask A");
            app.feed_stream(StreamMsg::Chunk("half an ans".into()));
            if !aimed {
                app.workspace.focus_pane(sibling);
                app.open_by_hash(&b.to_hex()[..8]);
                assert_eq!(app.prompt_target(), b_buffer, "premise: aimed at B");
            }
            if matches!(end, End::Interrupt) && !aimed {
                // CTRL-C in B is B's (§9 phase 6b): A's turn is not stopped
                // from here, so there is no ending to word — the press says
                // where A's work runs instead.
                app.notices.clear();
                app.interrupt();
                let said = app.notice().unwrap_or_default().to_string();
                assert!(
                    said.contains("running elsewhere") && said.contains(&app.session_label(a)),
                    "{said}"
                );
                assert!(
                    app.generations.iter().any(|g| g.buffer == a_buffer),
                    "A's turn runs on"
                );
                continue;
            }
            app.notices.clear();
            match end {
                End::Length => {
                    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
                    app.feed_stream(StreamMsg::Complete);
                }
                End::Failure => app.feed_stream(StreamMsg::Error(BackendError::RateLimited)),
                End::Interrupt => app.interrupt(),
            }
            let said = app.notice().unwrap_or_default();
            let named = format!("{}: ", app.session_label(a));
            if aimed {
                assert!(said.contains("`:continue` resumes it"), "{end:?}: {said}");
                assert!(!said.contains(&named), "{end:?}: unnamed here: {said}");
            } else {
                assert!(said.contains(&named), "{end:?}: names A: {said}");
                assert!(
                    said.contains("`:open` it, then `:continue`"),
                    "{end:?}: {said}"
                );
                assert!(!said.contains("`:continue` resumes it"), "{end:?}: {said}");
                // Do what it says.
                app.workspace.focus_pane(focused);
                app.open_by_hash(&a.to_hex()[..8]);
            }
            app.notices.clear();
            app.continue_last_turn();
            assert!(
                !app.generations.is_empty() && app.generations.iter().all(|g| g.buffer == a_buffer),
                "{end:?} aimed={aimed}: `:continue` resumed A: {:?}",
                app.notice()
            );
        }
    }
}

/// **A `:Send` cut off in the session you are in offers `:continue` there**
/// (review 2026-10-15, C F4's job half). The job's ending passed a constant
/// "not here", so it told you to `:open` the session you were already in.
#[tokio::test]
async fn a_send_cut_off_in_the_aimed_session_offers_continue_there() {
    use shannon_core::StopReason;
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let here = app.prompt_target();
    let episode = app.conversation().episode().id;
    app.start_job_on(here, "a long one").expect("a job");
    app.feed_job(episode, StreamMsg::Chunk("half".into()));
    app.feed_job(episode, StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_job(episode, StreamMsg::Complete);
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("`:continue` resumes it"), "{said}");
    assert!(!said.contains("`:open` it"), "{said}");
    app.notices.clear();
    app.continue_last_turn();
    assert!(
        !app.generations.is_empty(),
        "and it does: {:?}",
        app.notice()
    );
}

/// **Work a compose lands on says it ended** (review 2026-10-15, Reviewer C
/// F8).
///
/// A composition landing OVER a session supersedes whatever ran there,
/// through `end_work_on`, which keeps no account. The distillation got its
/// sentence in a review; a `:Send` reply simply stopped arriving, and a
/// prefill `:continue` vanished with only a log line. The rest of the family,
/// here — the defensive ending since 10-21, when compositions began landing
/// beside (`supersede_for_test`).
#[tokio::test]
async fn work_a_compose_lands_on_says_it_ended() {
    use shannon_core::{ContinueStrategy, StopReason};
    // A `:Send` reply streaming into the session.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let here = app.prompt_target();
    let episode = app.conversation().episode().id;
    app.start_job_on(here, "a long one").expect("a job");
    app.feed_job(episode, StreamMsg::Chunk("half".into()));
    let label = app.generations[0].label.clone();
    app.notices.clear();
    app.supersede_for_test(here);
    assert!(app.generations.is_empty(), "premise: the clone ended it");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains(&format!(
            "{label}: interrupted — its session changed under it; partial kept in :graph \
             — `:open` it, then `:continue`"
        )),
        "{said}"
    );
    // Do what it says: the partial is in the session the clone replaced.
    app.open_by_hash(&episode.to_hex()[..8]);
    app.notices.clear();
    app.continue_last_turn();
    assert!(
        !app.generations.is_empty(),
        "and `:continue` resumes it there: {:?}",
        app.notice()
    );

    // A prefill continuation drafting off-transcript.
    let mut cfg = Config::default();
    cfg.continue_.strategy = ContinueStrategy::Prefill;
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.output_mut().begin_turn("write a poem");
    app.conversation_mut().begin_turn("write a poem");
    app.feed_stream(StreamMsg::Chunk("Roses are".into()));
    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);
    app.continue_last_turn();
    assert!(app.continuing(), "premise: a continuation drafts");
    app.feed_continuation(StreamMsg::Chunk(" red".into()));
    // Its label names the session it continues, as a distillation's does
    // (D 3) — read before the clone replaces it.
    let tag = app.session_tag(app.conversation().episode().id);
    app.notices.clear();
    app.supersede_for_test(app.current);
    assert!(!app.continuing(), "premise: the clone ended it");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains(&format!(
            "the continuation of {tag}: dropped — its session changed before it arrived"
        )),
        "{said}"
    );
}

/// **A job's kept partial offers `:continue`, from where you are** (Steven,
/// 10-16, review stop 1; Reviewer C F4). A turn's endings offered it and a
/// job's — failed, stopped, superseded — did not, though it resumes there.
/// Failed in the session you are aimed at: `:continue` here, and it does. A
/// child stopped in no pane: `:open` it first. (Superseded: above.)
#[tokio::test]
async fn a_jobs_kept_partial_offers_continue() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let here = app.prompt_target();
    let episode = app.conversation().episode().id;
    app.start_job_on(here, "a long one").expect("a job");
    app.feed_job(episode, StreamMsg::Chunk("half".into()));
    app.notices.clear();
    app.feed_job(episode, StreamMsg::Error(BackendError::RateLimited));
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("partial kept in :graph — `:continue` resumes it"),
        "{said}"
    );
    app.notices.clear();
    app.continue_last_turn();
    assert!(
        !app.generations.is_empty(),
        "and it does: {:?}",
        app.notice()
    );

    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("long task", None).unwrap();
    app.feed_job(child, StreamMsg::Chunk("halfway th".into()));
    app.notices.clear();
    app.stop_child(&child.to_hex()[..8]);
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("partial kept in :graph — `:open` it, then `:continue`"),
        "{said}"
    );
}

/// **A superseded `:Send` whose partial could not be written says so as a
/// refusal**, not a report (review 2026-10-16, Reviewers A M-B, B F5, C F2 —
/// the exit's own spelling of the rule was held by nothing; `say_partial` is
/// the one now). The store fails only after the session is on disk, so the
/// clone can still land and the seal is what fails.
#[tokio::test]
async fn a_superseded_jobs_lost_write_is_a_refusal() {
    let (store, failing) = shannon_core::store::FailingStore::recoverable();
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    continue_with(&mut app, "Hello 0");
    let episode = app.conversation().episode().id;
    let here = app.prompt_target();
    app.start_job_on(here, "a long one").expect("a job");
    app.feed_job(episode, StreamMsg::Chunk("work in progress".into()));
    failing.store(true, std::sync::atomic::Ordering::SeqCst);
    app.notices.clear();
    app.supersede_for_test(here);
    assert!(
        app.generations.is_empty(),
        "premise: the clone ended the job"
    );
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("NOT saved"), "{said}");
    assert_eq!(app.notice_kind(), Some(NoticeKind::Refusal), "{said}");
}

/// **A distillation and a continuation are offered no tools**, and a call from
/// one is announced rather than swallowed.
///
/// `tool_request` is the one place tools are attached; `Work::offers_tools` is
/// the same fact stated where the *stream* can consult it. Background work that
/// quietly ignored a request would be the invisible-feedback bug.
///
/// This test used to include `Work::Job` and was written to START FAILING when
/// (81) gave jobs a tool round. It did, on the commit that gave them one, which
/// is what the pin was for.
#[tokio::test]
async fn work_that_was_offered_no_tools_says_so_when_asked_for_one() {
    use shannon_core::{ContinueStrategy, StopReason};
    let call = || StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-1", "rm -rf /tmp/nope"));
    // The announcement, by the work's own label — it asserted only that the
    // draft carried on, for a distillation alone, and removing the
    // announcement passed (review stop 12, T3 8).
    let announced = |app: &App| {
        let label = (app.generations.first()).expect("the work").label.clone();
        let said = app
            .notices
            .iter()
            .any(|(_, n)| n.contains(&label) && n.contains("requested a tool — not supported yet"));
        assert!(said, "`{label}`: {:?}", app.notices);
    };

    // A distillation.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let t = app.conversation().episode().own_turns()[0];
    let target = SquashTarget::Gather {
        episode: app.conversation().episode().id,
        turn: t,
        index: 0,
        config: app.conversation().episode().config.clone(),
    };
    app.fake_summary_on(app.current, target, vec![t]);
    app.notices.clear();
    app.feed_summary(call());
    announced(&app);
    assert!(
        app.distilling().is_some(),
        "the draft carries on — a protocol surprise does not fail the turn"
    );

    // A continuation, drafting off-transcript.
    let mut cfg = Config::default();
    cfg.continue_.strategy = ContinueStrategy::Prefill;
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.output_mut().begin_turn("write a poem");
    app.conversation_mut().begin_turn("write a poem");
    app.feed_stream(StreamMsg::Chunk("Roses are".into()));
    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);
    app.continue_last_turn();
    assert!(app.continuing(), "premise: a continuation drafts");
    app.notices.clear();
    app.feed_continuation(call());
    announced(&app);
    assert!(app.continuing(), "and carries on");
}

/// **A child runs what is safe unattended, and nothing else.**
///
/// The bar is `Verdict::safe_unattended`, not "allowed": a job never
/// manufactures a question, so the two consents that mean "allowed" — a rule
/// the user wrote for their own foreground work, and a remembered `A` — do not
/// carry over to an agent they spawned and walked away from.
#[tokio::test]
async fn a_child_runs_a_read_and_holds_on_everything_else() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("look around", None).expect("spawned");

    // A default read: it just runs, with no pane and no keypress.
    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-1", "ls -la")),
    );
    assert!(
        app.shown_call().is_none(),
        "a job never manufactures a question"
    );
    assert!(
        app.running_tools.contains_key("tu-1") || !app.tool_results.is_empty(),
        "it ran without asking anyone"
    );

    // Something destructive: it holds, and says what it wanted.
    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-2", "rm -rf /tmp/gone")),
    );
    assert!(
        app.shown_call().is_none(),
        "still no question — that is the whole difference from the foreground"
    );
    let notice = app.notice().expect("says so").to_string();
    assert!(
        notice.contains("held on"),
        "the user is told the child STOPPED — not that it is running, which is \
         what it used to say: {notice}"
    );
    assert!(
        !notice.contains("approve it there"),
        "and is not pointed at an approval surface that does not exist: {notice}"
    );
    assert!(
        notice.contains("rm -rf"),
        "named, so they can decide without going to find it: {notice}"
    );
}

/// A held child stays held: **nothing resumes it**, and it keeps holding its
/// session's slot until a human deals with it. "Pauses and holds" is the
/// contract — an agent that routed around the block would be the eager
/// optimizer doing exactly what the gate exists to stop.
///
/// Asserted by COUNTING REQUESTS. My first version checked that the job was
/// still listed and its turn still open, and both are true whether or not it
/// resumed — the mutation that drops the `held.is_none()` guard passed. What
/// distinguishes a held job from a running one is that it does not talk to the
/// backend again.
#[tokio::test]
async fn a_held_child_does_not_resume_itself() {
    let backend = std::sync::Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_backed(backend.clone(), Arc::new(Config::default()));
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("do something", None).expect("spawned");
    let sent = backend.requests.lock().unwrap().len();

    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-1", "rm -rf /tmp/gone")),
    );
    // Even a result arriving for it must not restart the job.
    app.finish_tool_call("tu-1", "whatever".into(), false);

    assert_eq!(
        backend.requests.lock().unwrap().len(),
        sent,
        "a held job sends nothing more — it waits for a human"
    );
    assert_eq!(app.job_episodes(), vec![child], "and is still there, held");
}

/// **A job ending must not tear down the question the user is looking at.**
///
/// Giving jobs a tool round meant giving them a teardown, and the teardown
/// closed the decision pane, cleared the queue and dropped every pending result
/// — all of which belong to the FOREGROUND. A `:child` failing in the
/// background would have closed a `(y/N)` the user was reading, about a command
/// with nothing to do with it.
///
/// That is the exact cross-generation bug `tool_owner` exists to prevent,
/// walked in through the door built to stop it — which is why the door is
/// scoped by owner rather than clearing wholesale.
#[tokio::test]
async fn a_job_ending_leaves_the_foregrounds_decision_alone() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("go", None).expect("spawned");
    // The user is being asked something, about their own turn.
    model_call(&mut app, "tu-fg", "rm -rf /tmp/shannon-test-absent-fg");
    assert!(
        app.shown_call().is_some(),
        "premise: a decision is on screen"
    );

    // The child fails in the background.
    app.feed_job(child, StreamMsg::Error(BackendError::RateLimited));

    assert!(
        app.shown_call().is_some(),
        "the decision is still there and still answerable"
    );
    assert!(
        app.awaiting_decision(),
        "and the turn it belongs to is still open to answer into"
    );
}

/// **Streaming into a buffer nobody is watching does not dirty the chat.**
///
/// `sync_chat` rebuilds the focused pane's rows from `Output` — O(transcript) —
/// so setting `chat_dirty` unconditionally means a full deep copy every frame
/// producing byte-identical output, for a session that is not on screen. Found
/// on the job path, then found again on the foreground path once `:open` could
/// move away mid-turn (Reviewer D), and now one guard in one place.
///
/// Against a thousands-of-FPS target this is the difference between a cost
/// proportional to what changed and one proportional to the whole history.
#[tokio::test]
async fn streaming_into_an_unwatched_buffer_does_not_dirty_the_chat() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("go", None).expect("spawned");
    // The focused pane shows the parent, not the child.
    app.chat_dirty = false;

    app.feed_job(child, StreamMsg::Chunk("nobody is reading this".into()));

    assert!(
        !app.chat_dirty,
        "no re-render was scheduled for a session that is not on screen"
    );

    // And the converse, so this is a delta rather than a constant: open the
    // child in the focused pane and the same chunk DOES schedule one.
    app.open_by_hash(&child.to_hex()[..8]);
    app.chat_dirty = false;
    app.feed_job(child, StreamMsg::Chunk(" but now they are".into()));
    assert!(
        app.chat_dirty,
        "premise: watching it really does schedule a redraw"
    );
}

/// **`:model stub` sends to the stub, and leaves its neighbour on the real
/// backend.**
///
/// A model switch from the user's chair ("what answers me") and a provider
/// switch underneath. Making the model name carry it is what keeps those the
/// same question — and, more importantly, keeps the choice PER SESSION. An
/// app-wide toggle would have made "mark two panes and send to both" meaningless
/// the moment one of them was on the stub, and that workflow is the reason the
/// window model exists.
///
/// Asserted on which backend received the request, not on the model string: the
/// string was always set correctly, and the question is whether anything routed
/// on it.
#[tokio::test]
async fn the_stub_is_reachable_by_name_and_only_the_session_that_asked() {
    let real = std::sync::Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_backed(real.clone(), Arc::new(Config::default()));
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());

    // Session A stays on the configured backend.
    app.submit("ask the real one");
    assert_eq!(
        real.requests.lock().unwrap().len(),
        1,
        "premise: the configured backend took the first turn"
    );
    app.feed_stream(StreamMsg::Complete);

    // Session B is pointed at the stub.
    app.new_session_command();
    app.model_command("stub");
    assert_eq!(
        app.conversation().episode().config.model,
        shannon_core::backend::models::STUB,
        "premise: the session records what will answer it"
    );

    app.submit("ask the stub");

    assert_eq!(
        real.requests.lock().unwrap().len(),
        1,
        "the configured backend did NOT see it — the stub did"
    );
}

// ---- the teardown matrix: every kind of work x every way it can end --------
//
// `the_teardown_matrix` holds the whole table; the tests after it pin one
// cell's own wording or premise, which a table's shared assertions cannot.
//
// Reviewer C built the matrix from the tests and found seven empty cells, then
// probed each one: every mechanism was already correct, which is the expected
// result now that `Work`/`resolve`/`dispatch_stream_msg` are exhaustive
// and kind-agnostic where it matters. They are still findings — nothing would
// catch a regression in any of them — and a probe that lives in a reviewer's
// worktree did not happen ([[feedback_mutation_check_guards]]). So they land
// here.

/// **Every kind of work, ended every way** (review stop 12, T3 11): the
/// thirty cells of `resolve`'s `(Work, Outcome)` match, each ended through
/// the door a user's ending takes, and each asserting what every ending
/// owes —
///
/// - **gone**: the work is in no list, and nothing holds its session;
/// - **cancelled**: its request is stopped, not detached;
/// - **kept as it should be, on disk**: a turn's partial a turn, committed
///   when it finished and `interrupted` otherwise; an off-transcript work's
///   session as it was, unless it finished and composed; a deleted session
///   deleted;
/// - **said**: a failure its reason, a stop and a supersede that they
///   happened, a delete what went with the session (`Lost`'s words);
/// - **free**: the session takes the next prompt at once.
///
/// The cells were spread over four sections in three files, each asserting
/// a different subset, and `Job(Child) × Superseded` was claimed by a test
/// that drove a `:Send`. The rows are the product's types (`WorkKind::of`'s
/// exhaustive match) and the columns its `Outcome`s, so a new kind of work
/// or a new ending fails to compile here until it has a row; what stays by
/// hand is the two lists the loop walks, beside the matches that class them.
/// What should be kept is written out, not read off `Outcome::seals()` — a
/// mutated `seals()` must not be able to agree with itself.
///
/// **One cell cannot happen**: a turn in its tool round is not finished by
/// a `Complete` — a call is pending, and a round ends by resuming — so that
/// cell asserts the `Complete` leaves the round as it was.
#[tokio::test]
async fn the_teardown_matrix() {
    use shannon_core::StopReason;
    let kinds = [
        WorkKind::Streaming,
        WorkKind::ToolRound,
        WorkKind::Child,
        WorkKind::Send,
        WorkKind::Summary,
        WorkKind::Continuation,
    ];
    let failure = BackendError::RateLimited;
    let endings = [
        Outcome::Finished,
        Outcome::Failed(failure.to_string()),
        Outcome::Stopped,
        Outcome::Superseded,
        Outcome::Deleted,
    ];
    // Every cell reached its last assertion, or the one that cannot happen.
    let (mut walked, mut cannot) = (0, 0);
    for kind in kinds {
        for how in &endings {
            let cell = format!("{kind:?} x {how:?}");
            let mut s = standing(kind);
            let app = &mut s.app;
            let before = records_on_disk(app, &s.episode);
            let name = app.session_label(s.episode);
            if matches!(how, Outcome::Stopped) && app.prompt_target() != s.buffer {
                aim_at(app, s.episode);
            }
            app.notices.clear();
            match how {
                Outcome::Finished => app.feed_turn(s.id, StreamMsg::Complete),
                Outcome::Failed(_) => app.feed_turn(s.id, StreamMsg::Error(failure.clone())),
                Outcome::Stopped => {
                    assert!(app.ctrl_c_stop_for_test(), "{cell}: nothing to stop");
                }
                Outcome::Superseded => app.land_over_for_test(s.buffer, s.first),
                Outcome::Deleted => app.delete_episodes(&[s.episode]),
            }
            app.settle();
            if (kind, how) == (WorkKind::ToolRound, &Outcome::Finished) {
                let still = app.generations.iter().find(|g| g.id == s.id);
                assert!(
                    still.is_some_and(|g| WorkKind::of(&g.work) == WorkKind::ToolRound)
                        && app.shown_call_in(s.buffer).is_some(),
                    "{cell}: a `Complete` leaves a round with a call pending as it was"
                );
                cannot += 1;
                continue;
            }

            // Gone, and cancelled.
            assert!(
                !app.generations.iter().any(|g| g.buffer == s.buffer),
                "{cell}: nothing holds the session"
            );
            assert!(
                after_yielding(|| s.backend.cancelled()).await,
                "{cell}: its request was cancelled"
            );

            // Kept as it should be, read from the store.
            let after = records_on_disk(app, &s.episode);
            let said = (app.notices.iter().map(|(_, m)| m.as_str()))
                .collect::<Vec<_>>()
                .join(" | ");
            let ended = |turns: &OnDiskTurns| {
                let last = turns.as_ref().and_then(|t| t.last()).cloned();
                last.map(|(words, stop)| (words.iter().any(|w| w.contains("half")), stop))
            };
            match (kind.makes_a_turn(), how) {
                (_, Outcome::Deleted) => assert!(
                    matches!(
                        app.store.load_episode(&s.episode),
                        Err(shannon_core::StoreError::EpisodeNotFound(_))
                    ),
                    "{cell}: the deleted session stayed deleted"
                ),
                (true, Outcome::Finished) => {
                    let (holds, stop) = ended(&after).expect("a turn on disk");
                    assert!(holds, "{cell}: the partial is a turn: {after:?}");
                    assert_ne!(stop, Some(StopReason::Interrupted), "{cell}: finished");
                }
                (true, Outcome::Failed(_) | Outcome::Stopped | Outcome::Superseded) => {
                    let (holds, stop) = ended(&after).expect("a turn on disk");
                    assert!(holds, "{cell}: the partial is a turn: {after:?}");
                    assert_eq!(stop, Some(StopReason::Interrupted), "{cell}: cut off");
                }
                (false, Outcome::Finished) => {
                    let shown = (app.session_of(s.buffer).expect("its buffer"))
                        .conversation
                        .messages()
                        .iter()
                        .any(|m| m.content.contains("half"));
                    assert!(shown, "{cell}: composed into the session");
                }
                (false, Outcome::Failed(_) | Outcome::Stopped | Outcome::Superseded) => {
                    assert_eq!(after, before, "{cell}: the session as it was")
                }
            }

            // Said — a failure where the work's words were going: inline in
            // a turn's transcript, as every kind of turn's is
            // (`App::say_failure`); on the notice line for what streams off
            // it.
            let words: &[&str] = match how {
                Outcome::Finished => &[],
                Outcome::Failed(_) => &["rate limited"],
                Outcome::Stopped => &["stopped", "cancelled"],
                Outcome::Superseded => &["interrupted", "dropped"],
                Outcome::Deleted => &["deleted"],
            };
            let shown = (app
                .session_of(s.buffer)
                .map(|b| b.output.lines().join("\n")))
            .unwrap_or_default();
            let record = match how {
                Outcome::Failed(_) if kind.makes_a_turn() => &shown,
                _ => &said,
            };
            assert!(
                words.is_empty() || words.iter().any(|w| record.contains(w)),
                "{cell}: said one of {words:?}: {record}"
            );
            if matches!(how, Outcome::Deleted) {
                let clause = format!("deleted {name} (with {})", kind.went_with_it());
                assert!(
                    said.contains(&clause),
                    "{cell}: the delete names what went with it ({clause:?}): {said}"
                );
            }

            // Free: the session takes the next prompt at once.
            let now = (app.session_of(s.buffer).expect("its buffer"))
                .conversation
                .episode()
                .id;
            if app.prompt_target() != s.buffer {
                aim_at(app, now);
            }
            app.submit("next");
            assert!(
                app.pending.is_empty() && app.generations.iter().any(|g| g.buffer == s.buffer),
                "{cell}: the next prompt runs, not queued"
            );
            walked += 1;
        }
    }
    assert_eq!(
        (walked, cannot),
        (kinds.len() * endings.len() - 1, 1),
        "every cell walked"
    );
}

/// Job(Child) x the ordinary `CTRL-C`, as opposed to `:child --stop`.
///
/// The explicit cancel was tested and the generic one was not, so the path a
/// user actually reaches by reflex was the uncovered one.
///
/// **In the child's session** (§9 phase 6b): a child works in a session of
/// its own, and `CTRL-C` stops the work where the prompt goes — so pressed
/// in the parent it leaves the child running, and names it.
#[tokio::test]
async fn ctrl_c_stops_a_spawned_child() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let child = app.spawn_child_job("go", None).expect("spawned");
    app.feed_job(child, StreamMsg::Chunk("half an answer".into()));
    assert_eq!(app.running_children(), 1, "premise: a child is running");

    assert!(
        !app.ctrl_c_stop_for_test(),
        "the parent's CTRL-C is the parent's"
    );
    assert_eq!(app.running_children(), 1, "the child runs on");
    app.interrupt();
    let said = app.notice().unwrap_or_default().to_string();
    assert!(
        said.contains("running elsewhere") && said.contains(&child.to_hex()[..8]),
        "and the press says where: {said}"
    );

    aim_at(&mut app, child);
    assert!(app.ctrl_c_stop_for_test(), "there was something to stop");

    assert_eq!(app.running_children(), 0, "the child was stopped");
    let notice = app.notice().expect("says so").to_string();
    assert!(notice.contains("stopped"), "and says so: {notice}");
}

/// Turn x composed-over while **streaming**, as opposed to while parked on a
/// decision. `resolve`'s `Work::Turn` arm does not branch on `Stage`, so
/// the risk is low — but the landed test only drove the parked variant, so
/// nothing proved the streaming one.
#[tokio::test]
async fn composing_over_a_buffer_ends_a_streaming_turn_on_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    app.submit("a turn that is streaming, not parked");
    app.feed_stream(StreamMsg::Chunk("partial".into()));
    assert!(
        app.streaming(),
        "premise: streaming, not awaiting a decision"
    );

    app.supersede_for_test(app.current);

    assert!(
        app.turns().next().is_none(),
        "the turn went with the session it was writing into"
    );
}

/// **Composing beside a session leaves its work running** (Steven, 10-20,
/// `buffers.md` §9 call 1: "A prompt in flight should stay in flight unless
/// someone `<C-c>` or `:q!`/`:qa!`s a conversation"). A clone made while the
/// aimed session has a turn streaming, a decision open or a `:Send` job
/// lands in a buffer of its own — in the window that showed the session,
/// aimed at — and the work goes on in the session it began in, which keeps
/// its buffer. It was rewritten in place, and the work cut off (review
/// 2026-10-15, A F5).
#[tokio::test]
async fn composing_beside_a_session_leaves_its_work_running() {
    #[derive(Debug, Clone, Copy)]
    enum Work {
        Streaming,
        Deciding,
        Job,
    }
    for work in [Work::Streaming, Work::Deciding, Work::Job] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 2);
        let base = app.conversation().episode().id;
        let buffer = app.current;
        match work {
            Work::Streaming => {
                app.submit("a turn streaming");
                app.feed_stream(StreamMsg::Chunk("half".into()));
            }
            Work::Deciding => app.park_on_decision(),
            Work::Job => {
                app.start_job_on(buffer, "in the background")
                    .expect("the session is free");
                app.feed_job(base, StreamMsg::Chunk("working".into()));
            }
        }
        let running = |app: &App| {
            app.generations
                .iter()
                .filter(|g| g.buffer == buffer)
                .count()
        };
        assert_eq!(running(&app), 1, "{work:?}: premise: work on it");
        let window = (app.session_panes(Over::ThisTab))
            .find(|&p| app.pane_shows(p, buffer))
            .expect("premise: a window shows it");

        app.clone_episode(base);

        let landed = app.prompt_target();
        assert_ne!(landed, buffer, "{work:?}: a buffer of its own, aimed at");
        assert_eq!(running(&app), 1, "{work:?}: the work runs on");
        let held = |b: BufferId| app.session_of(b).map(|b| b.conversation.episode().id);
        assert_eq!(
            held(buffer),
            Some(base),
            "{work:?}: its session keeps its buffer"
        );
        assert_ne!(held(landed), Some(base), "{work:?}: the clone is the other");
        assert_eq!(
            app.pane_session(window).id,
            landed,
            "{work:?}: in the window that showed it"
        );
    }
}

/// **A lazy session is not lost to a composition beside it** — a clone
/// never written (it shares, owns nothing) is still a session: composing
/// again from there leaves it in its own buffer, unwritten, which `:qa`
/// names once it holds something of its own (a name here; a bare look
/// owes a quit nothing, S21-Q12). Rewritten in place, it went without a
/// trace (review 2026-10-15, A F5; the checkout's comment says the same of
/// its own path).
#[test]
fn a_lazy_session_keeps_its_buffer_when_composed_beside() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    app.clone_episode(original);
    app.dispatch_command_for_test("rename first-look");
    let lazy = app.current;
    let lazy_id = app.conversation().episode().id;
    assert_eq!(app.buffer().on_disk, OnDisk::Absent, "premise: lazy");

    app.clone_episode(original);

    assert_ne!(app.current, lazy, "the second clone has its own buffer");
    assert_eq!(
        app.session_of(lazy).map(|b| b.conversation.episode().id),
        Some(lazy_id),
        "the first is still held"
    );
    let label = app.session_label(lazy_id);
    assert!(
        app.quitting_ends().iter().any(|e| e.contains(&label)),
        "and `:qa` names it: {:?}",
        app.quitting_ends()
    );
}

/// **`:new` on a lazy clone starts a fresh session** — a clone owns no turn
/// and shows its source's every one, and `:new` read that as blank: "already
/// on a fresh session" over a screen of history, and stayed (found 10-21;
/// `App::is_blank` reads history, not ownership).
#[test]
fn new_on_a_lazy_clone_starts_a_fresh_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    app.clone_episode(original);
    let clone = app.current;
    assert!(
        !app.conversation().episode().segments().is_empty(),
        "premise: the clone has history"
    );
    app.new_session_command();
    assert_ne!(app.current, clone, "{:?}", app.notice());
}

/// **A blank aim is filled, not multiplied** — `:new`, then a clone: no
/// history, nothing running, one pane, so nobody could tell the buffer was
/// reused, and `:ls` gains nothing (`blank_buffer`'s rule, over a buffer).
#[tokio::test]
async fn a_blank_aim_is_filled_by_a_composition() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    app.new_session_command();
    let blank = app.current;
    assert!(
        app.conversation().episode().segments().is_empty(),
        "premise: blank"
    );
    let buffers = app.buffers.sessions().count();

    app.clone_episode(original);

    assert_eq!(app.current, blank, "the blank buffer holds the clone");
    assert_eq!(
        app.buffers.sessions().count(),
        buffers,
        "and no buffer was added"
    );

    // Not blank while its FIRST turn streams — no history yet, but you
    // could tell: the reply would be cut off.
    app.new_session_command();
    let first = app.current;
    app.submit("a first turn");
    app.feed_stream(StreamMsg::Chunk("half".into()));
    assert!(
        app.conversation().episode().segments().is_empty(),
        "premise: no history yet"
    );
    app.clone_episode(original);
    assert_ne!(app.current, first, "a buffer of its own");
    assert!(
        app.generations.iter().any(|g| g.buffer == first),
        "and the first turn runs on"
    );

    // Not blank in two panes either — the clone in one, the fresh one kept.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    app.new_session_command();
    let blank = app.current;
    app.set_focus(PaneKind::Output);
    app.split_command("output", SplitDir::Vertical);
    let windows: Vec<_> = (app.session_panes(Over::ThisTab))
        .filter(|&p| app.pane_shows(p, blank))
        .collect();
    assert_eq!(windows.len(), 2, "premise: blank, in two windows");
    app.clone_episode(original);
    assert_ne!(app.current, blank, "not welded into both");
    let shown: Vec<BufferId> = windows.iter().map(|&p| app.pane_session(p).id).collect();
    assert_eq!(shown, [app.current, blank], "{shown:?}");
}

/// **Two panes on one session are not welded by a composition** — `:vsp`,
/// then a clone: the window it lands in shows the clone, the other keeps
/// the session, each its own (the welded panes `:new` once made, 08-06).
#[test]
fn a_composition_lands_in_one_window_of_a_session_shown_in_two() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let base = app.conversation().episode().id;
    let buffer = app.current;
    app.set_focus(PaneKind::Output);
    app.split_command("output", SplitDir::Vertical);
    let windows: Vec<_> = (app.session_panes(Over::ThisTab))
        .filter(|&p| app.pane_shows(p, buffer))
        .collect();
    assert_eq!(windows.len(), 2, "premise: two windows on it");

    app.clone_episode(base);

    let landed = app.prompt_target();
    let shown: Vec<BufferId> = windows.iter().map(|&p| app.pane_session(p).id).collect();
    assert_eq!(
        shown,
        [landed, buffer],
        "the first window shows the clone, the other keeps the session"
    );
}

/// **Work that goes out of sight says where it went** (review 2026-10-15,
/// B F6): a turn streaming in a window that now shows another session — a
/// clone landed beside it — or in a pane that closed, is named once, with
/// the `:b` that shows it; shown again and left again, named again. A
/// `:child`, unseen from its start, is not news.
#[tokio::test]
async fn work_going_out_of_sight_says_where_it_went() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let base = app.conversation().episode().id;
    let buffer = app.current;
    app.submit("a turn streaming");
    app.feed_stream(StreamMsg::Chunk("half".into()));
    let label = app.session_label(base);
    let said = format!(
        "{label} is still answering, in no pane — `:b{}` shows it",
        buffer.number()
    );
    // On the row, cleared between steps (the history collapses a repeat).
    let said_now = |app: &mut App| {
        let said_it = app.notices.iter().any(|(_, m)| m.contains(&said));
        app.notices.clear();
        said_it
    };
    assert!(!said_now(&mut app), "premise: in sight");

    app.clone_episode(base);
    app.settle();
    assert!(
        said_now(&mut app),
        "{:?}",
        app.messages().collect::<Vec<_>>()
    );

    // Said once: another event changes nothing.
    app.settle();
    assert!(!said_now(&mut app), "not again");

    // Do what it says; then leave it again.
    app.dispatch_command_for_test(&format!("b{}", buffer.number()));
    assert!(
        (app.session_panes(Over::ThisTab)).any(|p| app.pane_shows(p, buffer)),
        "`:b` shows it"
    );
    assert!(!said_now(&mut app), "in sight again");
    app.clone_episode(base);
    app.settle();
    assert!(said_now(&mut app), "left again, said again");

    // B F6's own case: the only pane on A closed while A answers.
    let dir = TempDir::new().unwrap();
    let (mut app, a, _, _, focused) = app_with_two_sessions(dir.path());
    app.workspace.focus_pane(focused);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    app.submit("ask A");
    app.feed_stream(StreamMsg::Chunk("half".into()));
    app.notices.clear();
    app.workspace.focus_pane(focused);
    app.close_pane_command(Asked::Plain);
    app.settle();
    let said = format!(
        "{} is still answering, in no pane — `:b{}` shows it",
        app.session_label(a),
        a_buffer.number()
    );
    assert!(
        app.notices.iter().any(|(_, m)| m.contains(&said)),
        "{:?}",
        app.notices
    );

    // A child runs unseen from the start: nothing to say.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.spawn_child_job("in the background", None)
        .expect("spawned");
    app.notices.clear();
    app.settle();
    assert!(
        !app.messages().any(|m| m.contains("still answering")),
        "{:?}",
        app.messages().collect::<Vec<_>>()
    );
}

/// **A turn the queue starts is in sight from that event** (review stop 13,
/// C 1): A answers, a prompt queues behind it, A's reply completes and the
/// event's end starts the queued turn — then the very next event closes A's
/// only pane. The queue ran after `settle_sight`, so the started turn was
/// recorded only at the following event, and the close found it never seen:
/// "still answering, in no pane" was never said, and A streamed unseen. One
/// settle per event, as the loop runs.
#[tokio::test]
async fn a_turn_the_queue_starts_is_said_when_its_pane_closes() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, _, _, focused) = app_with_two_sessions(dir.path());
    app.workspace.focus_pane(focused);
    app.open_by_hash(&a.to_hex()[..8]);
    app.settle();
    let a_buffer = app.current;
    app.submit("ask A");
    app.feed_stream(StreamMsg::Chunk("half".into()));
    app.submit("then this");
    assert_eq!(app.pending.len(), 1, "premise: queued behind A's reply");
    app.feed_stream(StreamMsg::Complete);
    assert!(
        app.pending.is_empty(),
        "premise: the event's end started it"
    );
    assert!(
        app.slot_busy_for(a_buffer),
        "premise: A answers the queued prompt"
    );
    app.notices.clear();

    app.workspace.focus_pane(focused);
    app.close_pane_command(Asked::Plain);
    app.settle();
    let said = format!(
        "{} is still answering, in no pane — `:b{}` shows it",
        app.session_label(a),
        a_buffer.number()
    );
    assert!(
        app.notices.iter().any(|(_, m)| m.contains(&said)),
        "{:?}",
        app.notices
    );
}

/// Job(Send) x composed-over, as opposed to the delete path — a `:Send`,
/// though this said `Job(Child)`: `resolve`'s job arm does not branch on
/// the kind, and the child's cell is `the_teardown_matrix`'s (review stop
/// 12, T3 11).
///
/// This is the scenario `persist_job_turns`' doc names as the reason it takes
/// the generation and the episode as separate parameters — open a child in a
/// pane, then compose from there, and the buffer is repointed under the job.
#[tokio::test]
async fn composing_over_a_buffer_ends_the_job_on_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let base = app.conversation().episode().id;
    let buffer = app.current;
    app.start_job_on(buffer, "answer this in the background")
        .expect("the session is free");
    app.feed_job(base, StreamMsg::Chunk("working".into()));
    assert_eq!(app.job_episodes().len(), 1, "premise: a job on this buffer");

    app.supersede_for_test(buffer);

    assert!(
        app.job_episodes().is_empty(),
        "the job went with the buffer it was streaming into, rather than \
         writing one session's turns into another's log"
    );
}

/// Job(Send) x `Error` — every `feed_job(_, Error)` in the suite followed
/// `spawn_child_job`, so the `:Send` half of the same arm was unexercised.
#[tokio::test]
async fn a_failing_send_job_keeps_its_billed_partial() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let episode = app.conversation().episode().id;
    let buffer = app.current;
    app.start_job_on(buffer, "a question").expect("free");
    app.feed_job(episode, StreamMsg::Chunk("half an ans".into()));

    app.feed_job(episode, StreamMsg::Error(BackendError::RateLimited));

    assert!(app.job_episodes().is_empty(), "the job retired");
    let notice = app.notice().expect("says what happened").to_string();
    assert!(
        notice.contains("partial kept"),
        "what streamed was billed, so it is kept rather than dropped: {notice}"
    );
}

/// Continuation x `Error` — no test fed one, and its arm is distinct code from
/// the job and summary Error arms (it promises the truncated turn is
/// *unchanged*, which is the claim worth pinning).
#[tokio::test]
async fn a_failed_continuation_leaves_the_truncated_turn_unchanged() {
    let (mut app, _dir) = app_mid_prefill_continuation().await;
    let before = app.conversation().episode().id;
    let turns_before = app.conversation().episode().own_len();

    app.feed_continuation(StreamMsg::Error(BackendError::RateLimited));

    assert!(!app.continuing(), "the attempt was abandoned");
    assert_eq!(
        app.conversation().episode().id,
        before,
        "and composed nothing — the session is the same one"
    );
    assert_eq!(
        app.conversation().episode().own_len(),
        turns_before,
        "with the same turns"
    );
    let notice = app.notice().expect("says why").to_string();
    assert!(
        notice.contains("unchanged"),
        "and says the truncated turn survived, so `:continue` can go again: \
         {notice}"
    );
}

// ---- "is something running" is answerable -----------------------------------

/// The status row had a segment for every kind of in-flight work EXCEPT the
/// reply you are waiting on. That is the question deciding what `CTRL-C` does
/// and whether a queued prompt will ever drain. (Reviewer F, twice.)
#[tokio::test]
async fn a_foreground_turn_is_visible() {
    let mut app = test_app();
    assert!(!app.answering(), "premise: idle");

    app.submit("hello");

    assert!(app.answering(), "a streaming reply is reportable");
}

/// `:Send` turns are not children. `running_children` counted every job, so
/// sending to two of your OWN sessions claimed you had spawned two subagents —
/// in the one place a user looks to find out what is happening.
#[tokio::test]
async fn sends_are_counted_apart_from_children() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let buffer = app.pane_session(pane).id;
    app.start_job_on(buffer, "compare this").expect("a send");

    assert_eq!(app.running_sends(), 1, "it is a send");
    assert_eq!(
        app.running_children(),
        0,
        "and not a child — `:child` is a different thing with its own chapter"
    );
}

/// `S` is refused while a JOB is answering the session it would distil.
///
/// `llm_squash` hand-wrote its own list of slot-holders and omitted `job_on`,
/// so a `:Send` or a child answering that very session did not stop it — the
/// narrow copy with the narrower signature. Every guard asks `busy_reason` now.
/// (Reviewer D's matrix: six functions, no two enumerations alike.)
#[tokio::test]
async fn distilling_is_refused_while_a_job_answers_this_session() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("q");
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let buffer = app.pane_session(pane).id;
    app.start_job_on(buffer, "running").expect("a job");
    assert!(
        app.busy_reason(buffer).is_some(),
        "premise: the slot is held"
    );

    mark_the_aims_session(&mut app);
    distill_reviewed(&mut app);

    assert!(
        app.distilling().is_none(),
        "a distillation must not start alongside a job on the same session"
    );
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("running"),
        "names what is holding it: {notice}"
    );
    // `S` wrote its own tail ("distill once it finishes") instead of asking
    // the cause; it goes through `refuse_if_busy` now, like `:model` and
    // `:continue`, and says the recovery the cause gives.
    assert!(notice.contains("stop it (CTRL-C) or wait"), "{notice}");
}
