//! What is written, when: resume, `:wq`, quit-time writes.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **A launch that cannot open the session you left says so** (review stop
/// 2, B 2): it started fresh with the reason in the log alone, and the
/// session you were in was simply not there. Named, with why, and that its
/// file is untouched.
#[test]
fn a_launch_that_cannot_open_the_last_session_says_so() {
    let dir = TempDir::new().unwrap();
    let id = app_with_n_turns(dir.path(), 2).conversation().episode().id;
    assert_eq!(
        JsonlStore::new(dir.path())
            .unwrap()
            .get_ref("HEAD")
            .unwrap(),
        Some(id),
        "premise: HEAD names it"
    );
    // A line mid-file no reader can parse: corruption, not a torn tail.
    let file = dir
        .path()
        .join("episodes")
        .join(format!("{}.jsonl", id.to_hex()));
    let text = std::fs::read_to_string(&file).unwrap();
    let (first, rest) = text.split_once('\n').unwrap();
    std::fs::write(&file, format!("{first}\n{{not json\n{rest}")).unwrap();

    let app = test_app_with_store(
        test_backend(),
        config_resume_last(),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    assert_ne!(app.conversation().episode().id, id, "premise: fresh");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains(&format!(
            "could not open the session you left ({}",
            id.short_hex()
        )) && said.contains("its file is as it was"),
        "{said:?} {:?}",
        app.notices
    );
}

/// **`:wq` quits only if the write succeeded — and that is the whole command.**
///
/// Both reviewers mutation-proved this guard was held by nothing: replacing the
/// arm with `Command::WriteQuit => {}`, or dropping the `if !`, left all 1017
/// tests green. `commands.py` only asserted that `:wq` exits, which the mutant
/// also does, and there was no `WriteQuit` test anywhere. A guard whose
/// deletion passes the suite is a comment, not a fix.
///
/// The failing half needs a store that refuses appends, because a stub that
/// cannot distinguish the guarded behaviour from the unguarded one is not
/// scaffolding for this ([[feedback_mutation_check_guards]]).
// `submit` spawns the backend, so this needs a runtime.
#[tokio::test]
async fn write_quit_does_not_quit_when_the_write_was_refused() {
    // A store that says no. `:wq` on the last pane must NOT be a quit — the
    // turns it just reported it could not save are exactly what quitting
    // discards, which is the one outcome this command exists to prevent.
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(shannon_core::store::FailingStore::failing_appends()),
    );
    app.submit("something worth losing");
    app.feed_stream(StreamMsg::Complete);
    app.only_command(false);
    assert_eq!(
        app.workspace.pane_count(),
        1,
        "premise: this is the last pane"
    );

    assert!(
        matches!(app.dispatch_command("wq"), CmdEffect::Done(Flow::Stay)),
        "a refused write must not become a quit"
    );

    // And the happy half, or "never quits" would pass just as well.
    let mut ok = test_app();
    ok.submit("this one saves");
    ok.feed_stream(StreamMsg::Complete);
    ok.only_command(false);
    assert!(
        matches!(ok.dispatch_command("wq"), CmdEffect::Done(Flow::Quit)),
        "a successful write on the last pane quits"
    );
}

#[test]
fn turns_persist_and_a_fresh_app_resumes_them() {
    // The whole point of P2's wiring: a turn committed in one session is
    // there when you relaunch in the same directory.
    let dir = TempDir::new().unwrap();
    {
        let mut app = test_app_with_store(
            test_backend(),
            Arc::new(Config::default()),
            Box::new(JsonlStore::new(dir.path()).unwrap()),
        );
        app.conversation_mut().begin_turn("hello");
        app.conversation_mut().push_chunk("Hi there.");
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }

    // A brand-new App over the same store dir, in resume mode, restores it...
    let app = test_app_with_store(
        test_backend(),
        config_resume_last(),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    let msgs = app.conversation().messages();
    assert_eq!(msgs.len(), 2, "resumed the committed turn");
    assert_eq!(msgs[0].content, "hello");
    assert_eq!(msgs[1].content, "Hi there.");
    // ...and replays it into the output pane so it's actually visible.
    assert!(
        app.output().lines().iter().any(|l| l.contains("Hi there.")),
        "resumed turn is rendered in the output pane",
    );
}

#[test]
fn fresh_mode_starts_clean_and_leaves_no_empty_episode() {
    let dir = TempDir::new().unwrap();
    // A first session (default Fresh mode) commits a turn → one episode.
    {
        let mut app = test_app_with_store(
            test_backend(),
            Arc::new(Config::default()),
            Box::new(JsonlStore::new(dir.path()).unwrap()),
        );
        app.conversation_mut().begin_turn("hello");
        app.conversation_mut().push_chunk("hi");
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }

    // Relaunch in Fresh (the default): does NOT resume — a clean transcript.
    let app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    assert!(
        app.conversation().messages().is_empty(),
        "a fresh launch does not resume the last session"
    );
    assert!(
        app.output().is_empty(),
        "transcript is empty on a fresh launch"
    );

    // And a launch with no committed turn leaves no second (empty) episode —
    // persistence is lazy.
    let store = JsonlStore::new(dir.path()).unwrap();
    assert_eq!(
        store.list_episodes().unwrap().len(),
        1,
        "a launch with no message persists nothing new"
    );
}

/// Pins "what you see is what's sent": after a resume, the messages a new
/// turn ships to the backend include the prior turns, not just the new
/// prompt. Asked of the REQUEST: this read `snapshot()` after a bare
/// `begin_turn`, and a turn that sent only its newest message passed it
/// (review stop 12, T1 5).
#[tokio::test]
async fn resumed_history_is_carried_into_the_next_request() {
    let dir = TempDir::new().unwrap();
    {
        // A session's requests go to the model it was sealed under, so it is
        // made on the recorder's model too.
        let mut app = test_app_with_store(
            Arc::new(shannon_core::backend::RecordingStub::new()),
            Arc::new(Config::default()),
            Box::new(JsonlStore::new(dir.path()).unwrap()),
        );
        app.conversation_mut().begin_turn("first question");
        app.conversation_mut().push_chunk("first answer");
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }

    let backend = Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_with_store(
        backend.clone(),
        config_resume_last(),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.submit("second question");
    assert_eq!(
        last_sent(&backend),
        vec!["first question", "first answer", "second question"],
        "the request carries resumed history + the new prompt"
    );
}

#[test]
fn new_session_archives_the_old_and_resumes_the_new() {
    let dir = TempDir::new().unwrap();
    let cfg = config_resume_last();
    let mut app = test_app_with_store(
        test_backend(),
        cfg.clone(),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    // Session one.
    app.conversation_mut().begin_turn("session one");
    app.conversation_mut().push_chunk("reply one");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let old_id = app.conversation().episode().id;

    // Start fresh.
    app.reset_foreground();
    let new_id = app.conversation().episode().id;
    assert_ne!(new_id, old_id, "a new session is a distinct episode");
    assert!(app.output().is_empty(), "the transcript pane is cleared");
    assert_eq!(
        app.buffer().on_disk,
        OnDisk::Absent,
        "persist cursor reset for the new episode"
    );

    // Session two.
    app.conversation_mut().begin_turn("session two");
    app.conversation_mut().push_chunk("reply two");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();

    // Both episodes are on disk (old archived), HEAD points at the new one.
    let store = JsonlStore::new(dir.path()).unwrap();
    let episodes = store.list_episodes().unwrap();
    assert!(
        episodes.contains(&old_id) && episodes.contains(&new_id),
        "both sessions are kept on disk"
    );
    assert_eq!(
        store.get_ref("HEAD").unwrap(),
        Some(new_id),
        "HEAD follows the active (new) session"
    );

    // A relaunch resumes the NEW session, not the archived one.
    let resumed = test_app_with_store(
        test_backend(),
        cfg,
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    let msgs: Vec<&str> = resumed
        .conversation()
        .messages()
        .iter()
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(msgs, vec!["session two", "reply two"]);
}

#[test]
fn a_legacy_default_episode_without_head_is_adopted() {
    // A session created before `:new` has the fixed `default` episode but no
    // HEAD ref. Startup must resume it and write HEAD (transparent migration).
    let dir = TempDir::new().unwrap();
    {
        let mut store = JsonlStore::new(dir.path()).unwrap();
        let mut c = Conversation::with_config(TurnConfig {
            model: "old".into(),
            max_tokens: None,
            effort: None,
        });
        c.begin_turn("legacy");
        c.push_chunk("turn");
        c.commit_turn();
        store.put_episode(c.episode()).unwrap();
        for t in c.own_turns() {
            store.append_turn(&c.episode().id, &t).unwrap();
        }
        assert_eq!(
            store.get_ref("HEAD").unwrap(),
            None,
            "no HEAD before migration"
        );
    }

    let app = test_app_with_store(
        test_backend(),
        config_resume_last(),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    assert!(
        app.output().lines().iter().any(|l| l.contains("turn")),
        "the legacy session is resumed",
    );
    let store = JsonlStore::new(dir.path()).unwrap();
    assert_eq!(
        store.get_ref("HEAD").unwrap(),
        Some(Conversation::default_episode_id()),
        "HEAD now points at the adopted default episode",
    );
}

#[test]
fn persist_is_deduped_so_a_noop_commit_does_not_double_write() {
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.conversation_mut().begin_turn("x");
    app.conversation_mut().push_chunk("y");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    // Calling again with no new turn must not append a second line.
    app.persist_committed_turn();

    let store = JsonlStore::new(dir.path()).unwrap();
    let loaded = store
        .load_episode(&app.conversation().episode().id)
        .unwrap();
    assert_eq!(loaded.turns.len(), 1, "the tip was written exactly once");
}

#[test]
fn identical_consecutive_turns_both_persist_and_resume() {
    // Regression: TurnId is content-addressed, so two identical turns (same
    // user text → same reply, e.g. the same prompt typed twice) share an id.
    // Persistence must be positional, not id-deduped, or the repeat is lost.
    let dir = TempDir::new().unwrap();
    {
        let mut app = test_app_with_store(
            test_backend(),
            Arc::new(Config::default()),
            Box::new(JsonlStore::new(dir.path()).unwrap()),
        );
        for _ in 0..2 {
            app.conversation_mut().begin_turn("same");
            app.conversation_mut().push_chunk("identical reply");
            app.conversation_mut().commit_turn();
            app.persist_committed_turn();
        }
    }
    let app = test_app_with_store(
        test_backend(),
        config_resume_last(),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    // Two identical turns → 4 messages on resume, not 2.
    assert_eq!(
        app.conversation().messages().len(),
        4,
        "both identical turns persisted and resumed"
    );
}

#[test]
fn new_command_is_a_noop_on_a_blank_session() {
    let mut app = test_app(); // a fresh, blank session
    let blank = app.conversation().episode().id;
    app.new_session_command();
    assert_eq!(
        app.conversation().episode().id,
        blank,
        "`:new` on a blank session is a no-op (no redundant episode)"
    );

    // Once it has a committed turn, `:new` mints a fresh session.
    app.conversation_mut().begin_turn("hi");
    app.conversation_mut().push_chunk("there");
    app.conversation_mut().commit_turn();
    app.new_session_command();
    assert_ne!(
        app.conversation().episode().id,
        blank,
        "a non-blank session → `:new` starts fresh"
    );
    assert!(
        app.conversation().episode().own_is_empty(),
        "the fresh session is blank"
    );
}

/// Every way out seals a billed turn. `:qa` is one keystroke and returned
/// `true` unconditionally, throwing away a partial you had already paid for —
/// the hole `CTRL-C` was built to close, reachable from the command line.
/// (A plain `:qa` now refuses while the turn runs; `:qa!` is the way out.)
#[tokio::test]
async fn quitting_seals_the_turn_you_paid_for() {
    let mut app = test_app();
    app.output_mut().begin_turn("write a poem");
    app.conversation_mut().begin_turn("write a poem");
    app.feed_stream(StreamMsg::Chunk("Roses are".into()));
    assert!(app.streaming(), "premise: a billed turn in flight");

    let quit = app.dispatch_command_for_test("qa!");

    assert!(quit, "we are still quitting");
    assert!(!app.streaming(), "and the turn was sealed on the way out");
    assert!(
        app.conversation()
            .messages()
            .last()
            .unwrap()
            .content
            .contains("Roses are"),
        "what streamed was kept"
    );
}

/// Quitting must never START work. Each `stop_work` released a slot and drained
/// one queued prompt into a live request, which the next iteration sealed — so
/// `:qa` with prompts queued sent them all to the API on the way out.
#[tokio::test]
async fn quitting_does_not_fire_the_queue_at_the_api() {
    // Counts REQUESTS, not queue length. Draining on the way out also empties
    // `pending` and leaves nothing streaming, so asserting on those passes
    // either way — the question is whether the backend was asked.
    let backend = std::sync::Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_backed(backend.clone(), Arc::new(Config::default()));
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("first");
    app.submit("second");
    app.submit("third");
    assert_eq!(app.pending.len(), 2, "premise: two queued");
    let sent_before = backend.requests.lock().unwrap().len();
    assert_eq!(sent_before, 1, "premise: only the first went");

    let quit = app.dispatch_command_for_test("qa!");

    assert!(quit);
    assert_eq!(
        backend.requests.lock().unwrap().len(),
        sent_before,
        "quitting sent a queued prompt to the API on the way out"
    );
}

/// **A failed write says WHY, and what to do about it.**
///
/// `persist_job_turns` knew the reason — "disk full", "read-only filesystem",
/// "permission denied" are different problems and a user can only act on the
/// one they are told — and `refuse`d with it. Then the caller's own `refuse` a
/// few frames later overwrote it with a generic phrase: a last-write-wins
/// regression inside the fix for last-write-wins (Reviewer F).
///
/// And "partial kept in memory" was honest about state and silent about action,
/// while `leaving()` will discard it without a word. It names the verbs:
/// `:wa` retries the save, and copying it out keeps it whatever the disk
/// does — and `:wa` does save it, once the disk will take it.
///
/// **The job's own sentence**: the foreground's `Complete` in the setup had
/// already said "NOT SAVED TO DISK … the disk said no", so dropping the
/// job's reason passed (review stop 12, T3 6). Its notices are cleared first.
#[tokio::test]
async fn a_failed_write_keeps_its_reason_and_names_a_recovery() {
    let (store, failing) = shannon_core::store::FailingStore::recoverable();
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    app.feed_job(child, StreamMsg::Chunk("work worth keeping".into()));

    app.notices.clear();
    app.feed_job(child, StreamMsg::Error(BackendError::RateLimited));

    let said = (app.notices.iter())
        .find(|(_, n)| n.contains("NOT saved to disk ("))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| panic!("the job's partial says it: {:?}", app.notices));
    assert!(
        said.contains("the disk said no"),
        "the REASON survives to the one line the user reads: {said}"
    );
    assert!(
        said.contains("`:wa` retries") && said.contains("copy it out"),
        "and it names what they can actually do: {said}"
    );

    // And `:wa` is a real remedy: once the disk takes it, the partial is
    // saved.
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    app.notices.clear();
    app.dispatch_command_for_test("wa");
    let saved = (app.store.load_episode(&child))
        .unwrap_or_else(|e| panic!("the child's session saved: {e}; {:?}", app.notices))
        .turns
        .iter()
        .flat_map(|t| t.messages.iter().map(|m| m.content.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(saved.contains("work worth keeping"), "{saved}");
}

/// **A job resolving on an already-persisted session appends; it does not
/// re-register.**
///
/// `persist_job_turns` decides between "append the new turns" and "register
/// this episode and write all of them" by asking the store whether the episode
/// exists. That branch has been wrong three times — its own comment says so —
/// and Reviewer B mutation-confirmed it was covered by nothing: forcing every
/// job resolution down the register branch left all 812 tests green.
///
/// Re-registering an already-written session re-appends its whole history. On
/// the append-only JSONL log that is a second header plus duplicate turns, so
/// the assertion is on what the store hands back after a round trip, not on
/// what the app believes.
#[tokio::test]
async fn a_send_job_appends_to_an_already_persisted_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let episode = app.conversation().episode().id;
    let before = app
        .store
        .load_episode(&episode)
        .expect("on disk")
        .turns
        .len();
    assert_eq!(before, 2, "premise: two turns already written");

    let buffer = app.current;
    app.start_job_on(buffer, "one more")
        .expect("the session is free");
    app.feed_job(episode, StreamMsg::Chunk("an answer".into()));
    app.feed_job(episode, StreamMsg::Complete);

    let after = app
        .store
        .load_episode(&episode)
        .expect("still on disk")
        .turns;
    assert_eq!(
        after.len(),
        before + 1,
        "exactly one turn was added — a re-registration would have written the \
         history again, giving {} turns",
        before * 2 + 1
    );
}

/// **Quitting kills every command, including the user's own.**
///
/// The other half, and the opposite answer: killing a manual command is right
/// when the user is *leaving* and wrong when a turn merely ended. `stop_work`
/// returns `false` with nothing running, so `:qa` never reached
/// `abandon_tool_round` at all — and the command's `JoinHandle` was dropped,
/// which DETACHES rather than cancels, so `:tool sleep 300` outlived the app
/// that spawned it (Reviewer E).
///
/// **Killed, not merely forgotten**: on an executor whose commands never end,
/// the task must be cancelled. Emptying `running_tools` was all this asked,
/// and both `abort()`s on these paths could become drops — which detach —
/// with the suite green (review stop 12, T3 1).
#[tokio::test]
async fn quitting_kills_a_manual_command_rather_than_detaching_it() {
    let (mut app, exec) = app_on_a_hanging_executor();
    manual_tool(&mut app, "echo hi");
    assert_eq!(app.running_tools.len(), 1, "premise: it is running");
    assert_eq!(exec.started(), 1, "premise: and its task started");
    assert!(
        !app.ctrl_c_stop_for_test(),
        "premise: nothing is in the backend slot, so `stop_work` alone finds \
         nothing — which is exactly why quitting needs its own sweep"
    );
    assert_eq!(exec.cancelled(), 0, "premise: nor did that kill it");

    assert!(app.dispatch_command_for_test("qa!"), "the app is going");

    assert!(app.running_tools.is_empty(), "and took the command with it");
    assert!(
        cancelled_after_yielding(&exec, 1).await,
        "the command's task was aborted, not detached"
    );
}

/// **`:q` asks while something runs; `:q!` goes anyway** — vim's E37, with
/// "running" standing in for "modified" (Steven, 2026-09-30). Every way out
/// asks: `:q` on the last window, `:qa`, `:wq`, and `<C-w>q`.
#[tokio::test]
async fn quitting_while_work_runs_asks_and_bang_forces() {
    let mut app = test_app();
    app.output_mut().begin_turn("write a poem");
    app.conversation_mut().begin_turn("write a poem");
    app.feed_stream(StreamMsg::Chunk("Roses are".into()));
    assert!(app.streaming(), "premise: a turn in flight");
    // Closing a pane that is not the last stops nothing, so it does not ask.
    // (Bounded: a refused close would otherwise loop forever.)
    app.split_command("output", SplitDir::Vertical);
    for _ in 0..8 {
        if app.last_window() {
            break;
        }
        assert!(!app.window_command_for_test(crate::keymap::WindowCmd::Quit));
    }
    assert!(app.last_window(), "premise: `:q` would now leave");
    assert!(
        !app.notice()
            .unwrap_or_default()
            .contains("quitting would end"),
        "closing a pane asked: {:?}",
        app.notice()
    );

    for way_out in ["q", "qa", "wq"] {
        assert!(!app.dispatch_command_for_test(way_out), "`:{way_out}` left");
        let notice = app.notice().unwrap_or_default();
        assert!(
            notice.contains("a turn is in flight") && notice.contains("`:q!`"),
            "`:{way_out}` says what runs and how to go anyway: {notice:?}"
        );
        assert!(
            app.streaming(),
            "`:{way_out}` stopped the turn it refused for"
        );
    }
    assert!(
        !app.window_command_for_test(crate::keymap::WindowCmd::Quit),
        "`<C-w>q` on the last pane left"
    );

    assert!(app.dispatch_command_for_test("q!"), "`:q!` leaves");
    assert!(!app.streaming(), "and seals the turn on the way out");
}

/// The warning names a command you typed, and a turn waiting on you as
/// waiting, not as running: its stage covers both, and only the App knows.
#[tokio::test]
async fn the_quit_warning_names_what_quitting_would_end() {
    let mut app = test_app();
    manual_tool(&mut app, "echo hi");
    assert!(!app.dispatch_command_for_test("qa"));
    assert!(
        app.notice()
            .unwrap_or_default()
            .contains("`echo hi` is running"),
        "{:?}",
        app.notice()
    );
    assert!(app.dispatch_command_for_test("qa!"));

    let mut app = test_app();
    model_call(&mut app, "tu_1", "frobnicate");
    assert!(app.shown_call().is_some(), "premise: the turn asks");
    assert!(!app.dispatch_command_for_test("qa"));
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("a turn is waiting on your decision") && !notice.contains("is running"),
        "{notice:?}"
    );

    // A JOB merely running is "running", and one blocked on you is waiting —
    // never `busy_phrase`'s "running in this session": the list is app-wide.
    // Both arms survived deletion (Reviewer C, round 17).
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    for p in panes {
        app.workspace.toggle_mark(p);
    }
    app.submit("do the thing");
    let sends = app.generation_ids_for_test();
    assert!(!sends.is_empty(), "premise: the marked prompt made jobs");
    assert!(!app.dispatch_command_for_test("qa"));
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains(" is running") && !notice.contains("in this session"),
        "{notice:?}"
    );
    app.gate_job_tool_call_for_test(sends[0], "mkdir build");
    assert!(app.shown_call().is_some(), "premise: the send asks");
    assert!(!app.dispatch_command_for_test("qa"));
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("is waiting on your decision") && !notice.contains("in this session"),
        "{notice:?}"
    );
}

/// With nothing running, quitting is one keystroke, as it always was.
#[test]
fn quitting_with_nothing_running_does_not_ask() {
    let mut app = test_app();
    assert!(app.dispatch_command_for_test("qa"));
}

/// A `!` on a command that takes none is refused, not dropped (vim's E477).
#[test]
fn a_bang_where_none_is_declared_is_refused() {
    let mut app = test_app();
    assert!(!app.dispatch_command_for_test("graph!"));
    assert_eq!(app.notice().as_deref(), Some("no `!` allowed: `:graph!`"));
}

// --- `:write` -------------------------------------------------------------

/// `:w` on a session with nothing pending must ANSWER. A verb that works and a
/// verb that does nothing look identical in silence, and the whole reason
/// `:write` exists is that a state with no verb is worse than a blunt one.
#[test]
fn write_says_so_when_there_is_nothing_to_save() {
    let mut app = test_app();
    assert!(app.write_command(""), "nothing to lose, so `:wq` may quit");
    let notice = app.notice().expect("a verb that answers");
    // Not "already on disk": a blank session is not, and need not be.
    assert!(notice.contains("nothing to save"), "{notice}");
}

/// **`:write` saves a session that owns nothing — and `:wq` no longer quits
/// without it.** A lazy clone (or a straight merge, or a prune) shares every
/// turn it shows, so the persist compared `0 <= 0`, said "already on disk",
/// and returned "saved" to `:wq` — which quit, and the session the user had
/// just composed was gone (Reviewer D, reproduced).
#[test]
fn write_saves_a_session_that_owns_nothing() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let source = app.conversation().episode().id;
    app.clone_episode(source);
    let clone = app.conversation().episode().id;
    assert_ne!(clone, source, "premise: the clone is in front");
    assert_eq!(
        app.buffer().on_disk,
        OnDisk::Absent,
        "premise: lazy, unwritten"
    );

    assert!(app.write_command(""), "on disk now");
    assert!(
        app.notice().is_some_and(|n| n.contains("saved")),
        "{:?}",
        app.notice()
    );
    let disk = JsonlStore::new(dir.path()).unwrap();
    assert_eq!(
        disk.episode_len(&clone).unwrap(),
        Some(0),
        "registered, sharing its turns"
    );
    assert_eq!(
        disk.load_graph().unwrap().full_turns(&clone).len(),
        2,
        "and it reloads whole"
    );
}

/// **A branch written whole, reopened in a later run, owes nothing** — by
/// `resume = last` and by `:open`, so `:qa` lets it go. Probed for Steven's
/// image 27 (10-21: a reopened branch "always requires me to type `:qa!`"):
/// not this — a branch's own turns and the store's count agree — so that
/// one's cause is elsewhere (asked: what `:qa` names).
#[test]
fn a_reopened_branch_owes_nothing() {
    let dir = TempDir::new().unwrap();
    let branch = {
        let mut app = app_with_n_turns(dir.path(), 4);
        let episode = app.conversation().episode().id;
        let turn1 = app.conversation().episode().own_turns()[1];
        app.checkout_turn(episode, turn1, 1);
        app.conversation_mut().begin_turn("Hello new");
        app.conversation_mut().push_chunk("reply new");
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
        let branch = app.conversation().episode().id;
        assert!(app.quitting_ends().is_empty(), "premise: written");
        branch
    };
    let mut cfg = Config::default();
    cfg.tui.resume = shannon_core::ResumeMode::Last;
    let app = test_app_with_store(
        test_backend(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    assert_eq!(
        app.conversation().episode().id,
        branch,
        "premise: resumed it"
    );
    assert_eq!(app.quitting_ends(), Vec::<String>::new(), "resumed");
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.open_command(&branch.to_hex()[..8]);
    assert_eq!(
        app.conversation().episode().id,
        branch,
        "premise: opened it"
    );
    assert_eq!(app.quitting_ends(), Vec::<String>::new(), "opened");
}

/// **`:q` refuses over a session that is not on disk and holds something of
/// its own** (Steven, 10-16: "any 'unwritten session' and refusing without a
/// force"). A lazy clone — written on its first turn or `:w` — was quit over
/// without a word, and was gone (probed 10-16: no notice, nothing on disk).
/// Named, with the verb that saves it; `!` quits over it; `:wa` writes it
/// and the exit is free. **One only looked at holds nothing a quit loses**
/// (S21-Q12, Steven 11-01: agreed) — the refusal's `:wa` wrote a branch he
/// never made (review stop 15, F H3) — so a clone given nothing quits free,
/// and one given a name is held.
#[test]
fn quitting_refuses_over_an_unwritten_session() {
    {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 2);
        let source = app.conversation().episode().id;
        app.clone_episode(source);
        assert_eq!(app.buffer().on_disk, OnDisk::Absent, "premise: lazy");
        assert_eq!(app.quitting_ends(), Vec::<String>::new(), "a look");
        // Described, it holds a line of its own, as a name is.
        app.dispatch_command_for_test("description the token refresh race");
        let ends = app.quitting_ends().join("\n");
        assert!(ends.contains("unwritten"), "described: {ends}");
    }
    for force in [false, true] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 2);
        let source = app.conversation().episode().id;
        app.clone_episode(source);
        app.dispatch_command_for_test("rename kept-one");
        let clone = app.conversation().episode().id;
        let name = app.session_label(clone);
        assert_eq!(app.buffer().on_disk, OnDisk::Absent, "premise: lazy");

        if force {
            assert!(
                matches!(app.dispatch_command("qa!"), CmdEffect::Done(Flow::Quit)),
                "`!` quits over it"
            );
            continue;
        }
        assert!(
            matches!(app.dispatch_command("qa"), CmdEffect::Done(Flow::Stay)),
            "held"
        );
        let said = app.notice().unwrap_or_default();
        assert!(
            said.contains(&format!("{name}, unwritten (`:wa` writes it)")),
            "named, with the verb: {said}"
        );
        app.dispatch_command("wa");
        assert!(
            matches!(app.dispatch_command("qa"), CmdEffect::Done(Flow::Quit)),
            "written, the exit is free: {:?}",
            app.notice()
        );
    }
}

/// **A `:vfork` only looked at quits free too** — the third door that makes
/// a look (`App::fork_command`), beside `:clone` and a checkout.
#[test]
fn a_vfork_only_looked_at_quits_free() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    app.set_focus(PaneKind::Output);
    let source = app.conversation().episode().id;
    app.dispatch_command_for_test("vfork");
    let fork = app.focused_buffer();
    assert_ne!(fork.conversation.episode().id, source, "premise: forked");
    assert_eq!(fork.on_disk, OnDisk::Absent, "premise: lazy");
    assert_eq!(app.quitting_ends(), Vec::<String>::new(), "a look");
}

/// **A mark set in a look is something of its own; one it came with is
/// not** — a checkout carries the marks on the turns it shares, and those
/// are its source's (`SessionBuffer::has_its_own`, against what it was
/// made with): the look quits free until you mark it.
#[tokio::test]
async fn a_mark_set_in_a_checkout_is_something_a_quit_loses() {
    use super::position_marks::{SIZE, line_reading, over_store};
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    for i in 0..3 {
        app.submit(&format!("ask {i}"));
        app.feed_stream(StreamMsg::Chunk(format!("reply {i}")));
        app.feed_stream(StreamMsg::Complete);
    }
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    let line = line_reading(&app, pane, "reply 0");
    type_keys_sized(&mut app, SIZE, &format!("{}Gma", line + 1));
    let turn = app.conversation().episode().own_turns()[1];
    app.open_by_hash(&turn.to_hex()[..8]);
    assert_eq!(app.buffer().on_disk, OnDisk::Absent, "premise: lazy");
    assert!(app.buffer().marks.contains_key(&'a'), "premise: carried");
    assert_eq!(app.quitting_ends(), Vec::<String>::new(), "a look");
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    let line = line_reading(&app, pane, "reply 1");
    type_keys_sized(&mut app, SIZE, &format!("{}Gmb", line + 1));
    let ends = app.quitting_ends().join("\n");
    assert!(
        ends.contains("unwritten"),
        "marked, it holds something: {ends}"
    );
}

/// **…and over one whose write failed** — its turns are in memory only, which
/// "NOT SAVED TO DISK" said once and the exit then forgot.
#[tokio::test]
async fn quitting_refuses_over_a_session_whose_write_failed() {
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(shannon_core::store::FailingStore::failing_appends()),
    );
    app.submit("something worth losing");
    app.feed_stream(StreamMsg::Complete);
    let name = app.session_label(app.conversation().episode().id);
    assert!(
        matches!(app.dispatch_command("qa"), CmdEffect::Done(Flow::Stay)),
        "held"
    );
    // Which, and why, and what `!` does — not "`:wa` writes it", which would
    // fail the same way (review stop 2, B 6; D6).
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains(&format!(
            "{name}, its write failed (this session is in memory only"
        )) && said.contains("`:qa!` drops it")
            && !said.contains("`:wa`"),
        "{said}"
    );
}

/// **…and over a session written once whose LATER write failed** (review
/// stop 2, C 3): a failed first write rolls back, so the test above only ever
/// met a session never registered — and the guard could ask "is it on disk at
/// all?" with the suite green. Written, then a full disk: "NOT SAVED TO DISK"
/// once, and `:qa` must still hold, or the second turn is lost silently.
#[tokio::test]
async fn quitting_refuses_over_a_session_whose_later_write_failed() {
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(shannon_core::store::FailingStore::failing_after(1)),
    );
    app.submit("written");
    app.feed_stream(StreamMsg::Complete);
    let id = app.conversation().episode().id;
    assert!(
        matches!(app.store.episode_len(&id), Ok(Some(1))),
        "premise: the first turn is on disk"
    );
    app.submit("not written");
    app.feed_stream(StreamMsg::Complete);
    assert!(
        matches!(app.store.episode_len(&id), Ok(Some(1))),
        "premise: the second is not"
    );
    let name = app.session_label(id);
    app.new_moment();
    assert!(
        matches!(app.dispatch_command("qa"), CmdEffect::Done(Flow::Stay)),
        "held"
    );
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains(&format!(
            "{name}, its write failed (this turn is in memory only"
        )),
        "{said}"
    );
}

/// **…until a write succeeds** (D6): the failure is the LAST write's, so a
/// disk that comes back and a `:w` that lands clear it — and with nothing
/// owed, the exit is not held.
#[tokio::test]
async fn a_write_that_lands_clears_the_failure() {
    let (store, failing) = shannon_core::store::FailingStore::recoverable();
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    app.submit("held back");
    app.feed_stream(StreamMsg::Complete);
    assert!(app.buffer().write_failed.is_some(), "premise: it failed");
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    app.dispatch_command_for_test("w");
    assert_eq!(app.buffer().write_failed, None, "written, so not failed");
    assert!(
        matches!(app.dispatch_command("qa"), CmdEffect::Done(Flow::Quit)),
        "nothing holds the exit: {:?}",
        app.notice()
    );
}

/// **A store that cannot say holds the exit too, and an empty session never
/// does.** A resumed session starts with its cursor unread (`OnDisk::Unknown`);
/// a store that cannot be read is not "on disk" — the gate never guesses. A
/// fresh launch session has no history and owes nothing, whatever the store
/// says, so it is not asked about.
#[cfg(unix)]
#[test]
fn quitting_asks_the_store_and_never_guesses() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let name = app.session_label(app.conversation().episode().id);
    // As a resumed session starts: on disk, the cursor not yet read.
    app.buffer_mut().on_disk = OnDisk::Unknown;
    let empty = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    assert_eq!(
        empty.buffer().on_disk,
        OnDisk::Unknown,
        "premise: a launch's"
    );
    let mode = |m| std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(m));
    mode(0o000).unwrap();
    let (held, fresh) = (app.quitting_ends(), empty.quitting_ends());
    mode(0o700).unwrap();
    assert!(
        held.iter()
            .any(|e| e.starts_with(&format!("{name}, perhaps unwritten — could not tell"))),
        "{held:?}"
    );
    assert!(fresh.is_empty(), "an empty session owes nothing: {fresh:?}");
}

/// **A child's first failed write leaves its header where spawn put it.** A
/// child is written at spawn, owning nothing — `OnDisk::Own(0)`, not `Absent`.
/// Read as never written, its first persist would register it again, and a
/// registration that fails ROLLS BACK by deleting the episode: the header
/// spawn wrote, gone, because one append failed.
#[tokio::test]
async fn a_childs_failed_first_write_keeps_its_header() {
    let (store, failing) = shannon_core::store::FailingStore::recoverable();
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    app.conversation_mut().begin_turn("a parent turn");
    app.conversation_mut().push_chunk("ok");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let child = app.spawn_child_job("compute", None).expect("spawned");
    assert_eq!(
        app.store.episode_len(&child).unwrap(),
        Some(0),
        "premise: written at spawn"
    );

    failing.store(true, std::sync::atomic::Ordering::SeqCst);
    app.feed_job(child, StreamMsg::Chunk("an answer".into()));
    app.feed_job(child, StreamMsg::Complete);
    assert_eq!(
        app.store.episode_len(&child).unwrap(),
        Some(0),
        "the write failed; the header spawn wrote is still there"
    );
}

/// **A log that cannot be read is never registered again.** A cursor that does
/// not know asks the store; any answer but absence refuses. Registering an
/// episode that IS on disk appends a second header, and the registration's
/// rollback deletes the file — an earlier comment claimed a test for this that
/// only ever existed in a reviewer's worktree (Reviewers A and E, round 16).
#[test]
fn an_unreadable_log_is_refused_not_registered_twice() {
    let dir = TempDir::new().unwrap();
    let episode = app_with_n_turns(dir.path(), 2).conversation().episode().id;
    // Garble a line that is not the last, the shape JSONL reports as corrupt.
    let path = dir
        .path()
        .join("episodes")
        .join(format!("{}.jsonl", episode.to_hex()));
    assert!(path.exists(), "premise: the session's log at {path:?}");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    lines.insert(1, "{ not json");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let before = std::fs::read_to_string(&path).unwrap();

    let mut app = relaunched(dir.path());
    let id = app.buffer().id;
    let mut conversation = Conversation::with_episode(
        episode,
        TurnConfig {
            model: "claude-haiku".into(),
            max_tokens: None,
            effort: None,
        },
    );
    conversation.begin_turn("more");
    conversation.push_chunk("words");
    conversation.commit_turn();
    app.session_of_mut(id).unwrap().conversation = conversation;
    app.session_of_mut(id).unwrap().on_disk = OnDisk::Unknown;

    assert!(!app.write_command(""), "refused: the store could not say");
    assert!(
        app.notice().is_some_and(|n| n.contains("could not tell")),
        "{:?}",
        app.notice()
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        before,
        "the log is untouched"
    );
}

/// The real case: a turn that failed to persist is still in memory, and `:w`
/// is the retry. Same door as the automatic path — a recovery that writes
/// through different code than the thing it is recovering is a recovery for a
/// different bug.
#[tokio::test]
async fn write_retries_a_persist_that_failed() {
    let (store, failing) = shannon_core::store::FailingStore::recoverable();
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    app.submit("something worth keeping");
    app.feed_stream(StreamMsg::Complete);
    assert!(
        app.notice().is_some_and(|n| n.contains("NOT SAVED")),
        "the failure is on screen: {:?}",
        app.notice()
    );

    // The disk is emptied / the permission is fixed. This is the window
    // `:write` exists for, and it is the one a double that fails forever
    // cannot express.
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    app.write_command("");
    let notice = app.notice().expect("says what it did").to_string();
    assert!(notice.contains("written"), "{notice}");

    // And now there is genuinely nothing left to do.
    app.write_command("");
    assert!(
        app.notice().is_some_and(|n| n.contains("already on disk")),
        "{:?}",
        app.notice()
    );
}

/// **`:w <name>` names the session, as vim's `:w {file}` names an unnamed
/// buffer** (Steven, 10-14: "`:w foo` for vim users") — and says so FIRST,
/// since a vim user may have meant `:w notes.md` as an export: what it did
/// is the first thing on the line, never a silent rename. A session already
/// named otherwise is refused toward `:rename`, as vim's `:w other` would
/// write a copy rather than rename, and a second word is no session name.
#[test]
fn write_with_a_name_names_the_session_and_says_so() {
    let mut app = test_app();
    app.write_command("notes.md");
    let notice = app.notice().expect("says what it did").to_string();
    assert!(notice.starts_with("named `notes.md`"), "{notice}");
    assert_eq!(
        app.conversation().episode().label.name.as_deref(),
        Some("notes.md")
    );

    app.notices.clear();
    assert!(app.write_command("notes.md"), "the same name again writes");
    let notice = app.notice().expect("says what it did").to_string();
    assert!(
        !notice.contains("named `") && notice.contains("nothing to save"),
        "the same name again only writes: {notice}"
    );

    app.notices.clear();
    assert!(!app.write_command("other"), "named otherwise");
    let notice = app.notice().expect("a visible refusal").to_string();
    assert!(notice.contains("`:rename other`"), "{notice}");
    assert_eq!(
        app.conversation().episode().label.name.as_deref(),
        Some("notes.md"),
        "unchanged"
    );

    app.notices.clear();
    assert!(!app.write_command("two words"), "one word");
    assert!(app.notice().is_some_and(|n| n.contains("one word")));
}

/// `:wall` exists for the multi-session case: `:Send` to three panes with a
/// disk problem leaves three sessions in memory, and making the user `:open`
/// each one to save it is a recovery worse than the failure.
#[test]
fn wall_reports_across_every_open_session() {
    let mut app = test_app();
    app.write_all_command();
    assert!(
        app.notice().is_some_and(|n| n.contains("already on disk")),
        "{:?}",
        app.notice()
    );
}

/// **A child's tool call survives the `Complete` that arrives right behind it.**
///
/// This is the assertion whose absence made the whole of (81) inert. The
/// Anthropic stream emits `content_block_stop` → `ToolUse` and then
/// `message_stop` → `Complete` microseconds later, so a job's `Complete` ALWAYS
/// lands while its tool round is open. `handle_job_msg` sealed on it — the
/// foreground arm had the guard and the job arm did not — so every child
/// command was spawned, killed mid-flight by the teardown, and then recorded as
/// "the turn ended before this ran", while the user was told the child
/// *finished* (Reviewer A, BLOCKING).
///
/// Neither existing job-tool test fed `Complete`, which is why 861 tests stayed
/// green over a feature that could not work.
#[tokio::test]
async fn a_childs_tool_call_survives_the_complete_that_follows_it() {
    let backend = std::sync::Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_backed(backend.clone(), Arc::new(Config::default()));
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    let sent = backend.requests.lock().unwrap().len();

    // The real stream order: the call, then the end of the message.
    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-1", "ls")),
    );
    app.feed_job(child, StreamMsg::Complete);

    assert_eq!(
        app.job_episodes(),
        vec![child],
        "the child is still alive — a turn that asked for a tool is not finished"
    );
    // And when the answer comes back it resumes, which is the whole point.
    app.finish_tool_call("tu-1", "a b c".into(), false);
    assert_eq!(
        backend.requests.lock().unwrap().len(),
        sent + 1,
        "the child sent a second request carrying the tool result"
    );
}

/// The same, for a HELD call: `Complete` must not seal a job that is waiting on
/// a human either. Without this, `hold_job` and `Held` are unreachable code.
#[tokio::test]
async fn a_complete_does_not_seal_a_child_that_is_holding() {
    let backend = std::sync::Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_backed(backend.clone(), Arc::new(Config::default()));
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    let sent = backend.requests.lock().unwrap().len();

    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::bash_call("tu-1", "rm -rf /tmp/gone")),
    );
    app.feed_job(child, StreamMsg::Complete);

    assert_eq!(
        app.job_episodes(),
        vec![child],
        "still held, still holding its session's slot"
    );
    assert_eq!(
        backend.requests.lock().unwrap().len(),
        sent,
        "and still silent — it is waiting on a human, not working"
    );
}
