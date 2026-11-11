//! `CTRL-C` and stopping: what stops, what is kept, what is said.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// ---- Ctrl-C stops the turn, and keeps what you paid for --------------------

/// `Ctrl-C` with a turn in flight STOPS it rather than quitting, and the
/// partial survives as an `interrupted` turn `:continue` can resume.
///
/// Before 2026-08-07 there was no foreground interrupt at all: a prompt you
/// regretted billed to completion unless you killed the program.
#[tokio::test]
async fn ctrl_c_stops_the_turn_instead_of_quitting() {
    let mut app = test_app();
    app.output_mut().begin_turn("write a poem");
    app.conversation_mut().begin_turn("write a poem");
    app.feed_stream(StreamMsg::Chunk("Roses are".into()));
    assert!(app.streaming(), "premise: a turn is in flight");

    let quit = app.ctrl_c_stop_for_test();

    assert!(quit, "there was a turn to stop");
    assert!(!app.streaming(), "and it is no longer running");
    let last = app
        .conversation()
        .episode()
        .own_turns()
        .len()
        .checked_sub(1)
        .map(|_| {
            app.conversation()
                .messages()
                .last()
                .unwrap()
                .content
                .clone()
        })
        .unwrap_or_default();
    assert!(
        last.contains("Roses are"),
        "what streamed was KEPT, not discarded: {last:?}"
    );
}

/// …and with nothing running there is nothing to stop.
///
/// **The prose here used to be stale and actively wrong**: it said "`Ctrl-C`
/// still quits — the escape hatch a wedged TUI needs". That meaning was removed
/// (see `interrupt`: *"The quit meaning is gone"*), and the correct behaviour —
/// showing a `:qa` hint instead — is asserted by
/// `ctrl_c_with_nothing_running_says_how_to_quit`. The *assertion* below was
/// always right, because it is a `stop_work` unit check rather than an
/// `interrupt` behaviour check; only the sentence around it described a program
/// that no longer exists (Reviewer C).
///
/// Worth the note rather than a silent edit: a comment that survives the
/// behaviour it describes is this codebase's top-risk artifact, and a test's
/// doc comment is the one place a reader assumes the two were checked together.
#[test]
fn ctrl_c_with_nothing_running_has_nothing_to_stop() {
    let mut app = test_app();
    assert!(!app.ctrl_c_stop_for_test(), "no turn to stop");
}

/// **Abandoning a generation actually stops the request.**
///
/// The highest-consequence guard in this file, and until now the least pinned:
/// every other stub's task finishes on its own, so aborting it and not
/// aborting it look identical, and deleting `Generation::abort` entirely left
/// all 808 tests green. Twice this has shipped broken — a forwarder handle that
/// had swallowed the request handle, and a continuation whose `JoinHandle` was
/// dropped rather than aborted (dropping DETACHES) — and both times the symptom
/// was money: an abandoned turn streaming on to completion and billing for it.
///
/// [`HangingBackend`](shannon_core::backend::HangingBackend) is what makes the
/// question answerable: its request task is `pending` forever, so the flag can
/// only be set by cancellation. The wait is a bounded yield loop, not a sleep —
/// `abort()` takes effect at the next scheduler poll, and there is no wall-clock
/// margin to guess wrong about.
#[tokio::test]
async fn stopping_a_turn_actually_cancels_the_request() {
    let backend = std::sync::Arc::new(shannon_core::backend::HangingBackend::new());
    let mut app = test_app_backed(backend.clone(), Arc::new(Config::default()));
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());

    app.submit("a turn that never finishes");
    assert!(app.streaming(), "premise: the request is in flight");
    assert!(
        !backend.cancelled(),
        "premise: and nothing has cancelled it yet"
    );

    assert!(app.ctrl_c_stop_for_test(), "there was something to stop");

    assert!(
        after_yielding(|| backend.cancelled()).await,
        "the request task was aborted, not merely detached — a detached task \
         keeps streaming and keeps billing"
    );
}

/// Stopping frees the slot, so a prompt queued behind it must go — on both the
/// job path and the foreground path. Neither called `drain_next`, so a
/// follow-up typed during a turn sat in `pending` until some unrelated event
/// happened to resolve; in a single-pane session there is none.
#[tokio::test]
async fn stopping_a_turn_drains_the_prompt_queued_behind_it() {
    let mut app = test_app();
    app.submit("first");
    assert!(app.streaming(), "premise: the slot is held");
    app.submit("second");
    assert_eq!(app.pending.len(), 1, "premise: the follow-up queued");

    app.ctrl_c_stop_for_test();
    app.settle(); // the CTRL-C's event ends

    // Started, not merely gone from the queue: an empty `pending` is also
    // what a dropped prompt leaves (review stop 12, T3 9).
    assert!(app.pending.is_empty(), "the queued prompt was not orphaned");
    assert!(app.streaming(), "it took the slot");
    let last = (app.conversation().messages().last())
        .expect("a message")
        .content
        .clone();
    assert!(
        last.contains("second"),
        "and it is the one we queued: {last}"
    );
}

// ---- CTRL-C cancels; it never quits ---------------------------------------

/// With nothing to interrupt, `CTRL-C` ANSWERS instead of acting: vim's reply
/// to the question it is asked most often. It used to quit — and it was matched
/// above the `:` line, the confirm and Insert, so a half-typed command or a
/// sentence in progress ended the program and took the text with it.
#[test]
fn ctrl_c_with_nothing_running_says_how_to_quit() {
    let mut app = test_app();
    app.interrupt();
    let notice = app.notice().expect("it answers");
    assert!(notice.contains(":qa"), "names the way out: {notice}");
}

/// It cancels the `:` line rather than the program.
#[test]
fn ctrl_c_cancels_the_command_line() {
    let mut app = test_app();
    app.cmdline.open();
    assert!(app.cmdline.is_open(), "premise");

    app.interrupt();

    assert!(!app.cmdline.is_open(), "the line went, not the program");
}

/// And a pending `(y/N)` — the safe answer, and the reflexive one.
#[test]
fn ctrl_c_cancels_a_pending_confirmation() {
    let mut app = test_app();
    app.ask(ConfirmAction::DeleteAll);
    assert!(!app.pending_confirm.is_empty(), "premise");

    app.interrupt();

    assert!(app.pending_confirm.is_empty(), "the question was withdrawn");
    assert!(
        app.notice().expect("says so").contains("nothing done"),
        "and says nothing happened"
    );
}

/// A distillation holds the backend slot too — `slot_busy_for` knows six
/// holders and `stop_work` handled two, so `S` then `CTRL-C` used to find
/// "nothing to stop" and (back when that quit) discard a billed generation.
#[tokio::test]
async fn ctrl_c_stops_a_distillation() {
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
        "premise: a distillation holds the slot"
    );

    let stopped = app.ctrl_c_stop_for_test();

    assert!(stopped, "there was something to stop");
    assert!(app.distilling().is_none(), "and it is gone");
}

/// Stopping must RELEASE every slot before refilling any. Each arm used to
/// drain as it finished, so a queued prompt was started by one arm and sealed
/// by the next — consumed, billed, killed, and never mentioned. My own fix for
/// a stranded queue destroying the thing it was meant to deliver.
/// (Reviewer A, CRITICAL.)
#[tokio::test]
async fn stopping_does_not_start_then_kill_the_queued_prompt() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let buffer = app.pane_session(pane).id;
    // A JOB, not a foreground turn. The bug needs an arm ABOVE the foreground
    // one to drain — with only a foreground turn the old code drained AFTER the
    // seal and never had it, so this test passed against the bug it was named
    // for (Reviewer A, mutation-verified both directions).
    app.start_job_on(buffer, "running")
        .expect("a job holds the slot");
    app.submit("second");
    assert_eq!(app.pending.len(), 1, "premise: queued behind the job");

    app.ctrl_c_stop_for_test();
    app.settle(); // the CTRL-C's event ends, and the queue settles once

    // Genuinely running, never consumed and discarded. Unsettled, the queue
    // never moved and the `if streaming` arm here never ran (review stop 12,
    // T3 LOW).
    assert!(app.streaming(), "the queued prompt was started");
    let last = (app.conversation().messages().last())
        .expect("a message")
        .content
        .clone();
    assert!(
        last.contains("second"),
        "and it is the one we queued: {last}"
    );
}

/// `CTRL-C` cancels the question you can SEE, not the ones behind it. `clear()`
/// discarded questions the user had never seen — including an `ApplySummary`
/// carrying a billed, hand-edited draft — under a message saying nothing was
/// done, contradicting `ask`'s own "none discarded" contract.
/// (Reviewers A and B, convergent.)
#[test]
fn ctrl_c_cancels_one_question_not_the_whole_queue() {
    let mut app = test_app();
    app.ask(ConfirmAction::DeleteAll);
    app.ask(ConfirmAction::DeleteAll);
    assert_eq!(app.pending_confirm.len(), 2, "premise: two waiting");

    app.interrupt();

    assert_eq!(
        app.pending_confirm.len(),
        1,
        "the one on screen went; the one behind it did not"
    );
    // Asserted on what the user SEES, not on the notice. The old version
    // checked the notice for "more waiting" — a string `draw_status` can never
    // render, because `confirm_prompt()` takes the row and returns before it
    // reaches the notice (Reviewer F). A correct assertion about an invisible
    // property is still a test of nothing; the question itself is the signal
    // that another is waiting.
    assert!(
        app.confirm_prompt().is_some(),
        "the next question is on the row, which is how the user knows"
    );
}

/// Stopping a DISTILLATION must not eat the prompt queued behind it either.
///
/// The round-4 fix landed two mechanisms — hold the queue aside, and inline the
/// summary cancel so its own `drain_next` cannot fire mid-release. Deleting
/// BOTH left the suite green, because every test asserted `pending.len() == 0`,
/// which is true whether the prompt was delivered or destroyed. Assert the
/// TEXT survives (Reviewer D, mutation-verified both directions).
#[tokio::test]
async fn stopping_a_distillation_does_not_eat_the_queued_prompt() {
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
        "premise: a distillation holds the slot"
    );

    app.submit("my words");
    assert_eq!(app.pending.len(), 1, "premise: the prompt queued behind it");

    assert!(app.ctrl_c_stop_for_test());

    // Delivered or still queued — never consumed and discarded.
    let delivered = app
        .conversation()
        .messages()
        .iter()
        .any(|m| m.content.contains("my words"));
    let queued = app.pending.iter().any(|(_, t)| t.contains("my words"));
    assert!(
        delivered || queued,
        "the queued prompt was destroyed by the stop"
    );
}

/// **A partial that did not reach disk is never reported as safe.**
///
/// `stop_work`'s `lost` accounting carried a comment reading, in capitals,
/// **NOT COVERED BY A TEST** — and the reason was not laziness: there was no
/// way to make a write fail. Every store in the tree succeeds, so aborting the
/// guard and keeping it looked identical, and the honesty guarantee was
/// believed rather than checked (Reviewer B: the scaffolding did not exist).
///
/// `FailingStore` is that scaffolding. When a stub cannot *distinguish* the
/// guarded behaviour from the unguarded one, the fix is a new stub, not a
/// cleverer assertion (`[[feedback_mutation_check_guards]]`).
#[tokio::test]
async fn ctrl_c_says_so_when_a_stopped_jobs_partial_could_not_be_saved() {
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(shannon_core::store::FailingStore::failing_appends()),
    );
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    // Something billed and streamed, so there is a partial worth saving.
    app.feed_job(child, StreamMsg::Chunk("work in progress".into()));
    // A child works in a session of its own: CTRL-C there stops it.
    aim_at(&mut app, child);

    assert!(app.ctrl_c_stop_for_test(), "there was something to stop");

    let notice = app.notice().expect("says what happened").to_string();
    assert!(
        notice.contains("NOT be saved to disk") || notice.contains("NOT saved to disk"),
        "the user is told their partial did not survive, not that it was kept: \
         {notice}"
    );
}

/// **`^C:stop` is promised for every kind of work, not three of four.**
///
/// The status row's hint enumerated `answering() || children + sends > 0`,
/// under a comment claiming it was "off the same predicate that decides it".
/// It was not: with only a distillation or a prefill `:continue` running the
/// row drew `^C`, which `:help` defines as *"nothing is running — type `:qa`
/// to leave"*, while the key would stop a billed request. `help.rs` states the
/// promise too, so the manual was wrong in the same breath (Reviewer D).
///
/// Asserting on `anything_to_stop` rather than on the rendered string, because
/// the string has too many other reasons to agree with a test — the rule is
/// what wants pinning, and `stop_work` branches on this same expression.
#[tokio::test]
async fn the_stop_hint_covers_every_kind_of_work() {
    // A distillation, which the old hint could not see.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let turn = app.conversation().episode().own_turns()[0];
    let target = SquashTarget::Gather {
        episode: app.conversation().episode().id,
        turn,
        index: 0,
        config: app.conversation().episode().config.clone(),
    };
    app.fake_summary_on(app.current, target, vec![turn]);
    assert!(!app.answering(), "premise: not a foreground turn");
    assert_eq!(app.running_children() + app.running_sends(), 0, "nor a job");
    assert!(
        app.anything_to_stop(),
        "but something IS running, so the row must offer to stop it"
    );
    assert!(
        app.ctrl_c_stop_for_test(),
        "and the key really does stop it"
    );

    // A prefill continuation, the other kind the old hint missed.
    let (mut app, _dir) = app_mid_prefill_continuation().await;
    assert!(!app.answering());
    assert_eq!(app.running_children() + app.running_sends(), 0);
    assert!(app.anything_to_stop());
    assert!(app.ctrl_c_stop_for_test());

    // And nothing running still reads as nothing running.
    assert!(!app.anything_to_stop(), "idle after the stop");
}

/// **A stopped turn kills the command it was running** — Ctrl-C, and a turn
/// that ends in an error mid-round: the model's command goes with its round
/// (the user's own, `:tool`, does not — `a_turns_teardown_leaves_a_manual_command_running`,
/// in `teardown.rs`). Its task cancelled,
/// on an executor whose commands never end (T3 1).
#[tokio::test]
async fn a_stopped_turn_kills_the_command_it_was_running() {
    for how in ["ctrl-c", "error"] {
        let (mut app, exec) = app_on_a_hanging_executor();
        app.conversation_mut().begin_turn("go");
        app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
            "t1",
            "rm /tmp/a",
        )));
        app.resolve_shown_for_test(crate::permission_view::ToolAction::AllowOnce);
        assert_eq!(exec.started(), 1, "{how}: premise, it runs");
        match how {
            "ctrl-c" => assert!(app.ctrl_c_stop_for_test(), "something to stop"),
            _ => app.feed_stream(StreamMsg::Error(BackendError::RateLimited)),
        }
        assert!(
            cancelled_after_yielding(&exec, 1).await,
            "{how}: the command's task was aborted, not detached"
        );
    }
}

/// **`CTRL-C` before anything arrives: three false beliefs from one keypress.**
///
/// The fast case, and the common one — you press it the moment you regret the
/// prompt. `interrupt_turn` seals nothing (no chunk, no finished round), so it
/// falls through to `fail_turn`, which truncates the user's message back out of
/// the conversation. What the user was told and shown, before this:
///
///  1. *"stopped — `:continue` resumes it"* — nothing was kept.
///  2. `:continue` then answers *"the session has no turns to resume"*.
///  3. The transcript still showed `> write me a poem about ducks` with nothing
///     under it, forever, while the conversation had dropped it — so the NEXT
///     turn went to a model that had never seen the question.
///
/// (3) is the serious one: the screen is the only copy the user can read, and
/// it was the one that lied (Reviewer F, all three executed).
#[tokio::test]
async fn ctrl_c_before_the_first_chunk_keeps_nothing_and_says_so() {
    let mut app = test_app();
    app.submit("write me a poem about ducks");
    assert!(app.streaming(), "premise: the turn is in flight");
    assert!(
        app.output().lines().iter().any(|l| l.contains("ducks")),
        "premise: the prompt was echoed into the transcript"
    );

    assert!(app.ctrl_c_stop_for_test(), "there was something to stop");

    let notice = app.notice().expect("says what happened").to_string();
    assert!(
        !notice.contains(":continue"),
        "no recovery is offered for a turn that left nothing to resume: {notice}"
    );
    assert!(
        notice.contains("nothing was kept"),
        "and the user is told that plainly: {notice}"
    );
    // The invariant, which is the pair rather than either half.
    let on_screen = app.output().lines().iter().any(|l| l.contains("ducks"));
    let in_conversation = app
        .conversation()
        .messages()
        .iter()
        .any(|m| m.content.contains("ducks"));
    assert_eq!(
        on_screen, in_conversation,
        "the transcript and the conversation agree about what was said — \
         showing a question the model never received is the worst version of \
         the ambiguous-record trap"
    );
    assert!(!in_conversation, "and here they agree it was dropped");
}

/// …but a turn that RAN A COMMAND keeps its record, even though the
/// conversation drops it.
///
/// Two correct-sounding rules collide: "the screen must match the record" and
/// "a machine action is always recorded". The second wins. A turn killed
/// mid-command has no finished round, so `interrupt_turn` keeps nothing — which
/// makes the `⚙ cancelled: …` line the ONLY surviving account of a command that
/// really ran on the user's computer. The echo describes something the model
/// never received; that line describes something that happened to their machine.
#[tokio::test]
async fn a_turn_killed_mid_command_keeps_the_record_of_what_ran() {
    let mut app = test_app();
    app.submit("run something for me");
    // `ls -la` is read-only, so it auto-approves and is left running (not
    // settled) — the state `CTRL-C` has to account for.
    app.arrive_and_gate_for_test(app.foreground_id(), "tu-1", "ls -la");

    app.ctrl_c_stop_for_test();

    let shown = app.output().lines().join("\n");
    assert!(
        shown.contains("cancelled"),
        "the account of what touched the machine survives: {shown}"
    );
    assert!(
        shown.contains("run something for me"),
        "and so does the prompt that caused it, because the record needs its \
         context to be readable: {shown}"
    );
    // Kept as no turn's, the conversation having none (review stop 14, A 2):
    // numbered, every mark set after it was kept a turn late.
    let lines = app.output().lines();
    let asked = (lines.iter()).position(|l| l.contains("run something for me"));
    assert_eq!(
        app.output().turn_of_line(asked.unwrap()),
        None,
        "in no turn"
    );
    assert_eq!(app.output().last_turn(), None, "the store has no turn");
}

/// The cell `above` rows over where `text` is first drawn in `buf`.
pub(super) fn cell_at<'b>(
    buf: &'b ratatui::buffer::Buffer,
    text: &str,
    above: u16,
) -> &'b ratatui::buffer::Cell {
    let rows: Vec<String> = (0..buf.area().height)
        .map(|y| {
            (0..buf.area().width)
                .map(|x| buf[(x, y)].symbol())
                .collect()
        })
        .collect();
    let y = (rows.iter().position(|r| r.contains(text)))
        .unwrap_or_else(|| panic!("{text:?} drawn: {rows:#?}"));
    let x = rows[y][..rows[y].find(text).unwrap()].chars().count() as u16;
    &buf[(x, y as u16 - above)]
}

/// The foreground of the cell where `text` is first drawn in `buf`.
fn fg_of(buf: &ratatui::buffer::Buffer, text: &str) -> Option<ratatui::style::Color> {
    cell_at(buf, text, 0).style().fg
}

/// **A turn not in the conversation says so, painted apart** (Steven,
/// 10-31: "making it clear in the conversation that this message was not
/// sent and not included… include the error message for proper
/// diagnosis"). Your words, what came of them — the failure, or the record
/// of a command stopped part-way — and a last line saying why, all in
/// `ui.danger`, in the pane and focused. A turn in the conversation keeps
/// its own colours.
#[tokio::test]
async fn a_turn_not_in_the_conversation_says_so_painted_apart() {
    use crate::style::Ink;
    let answered = || {
        let mut app = test_app();
        app.submit("first");
        app.feed_stream(StreamMsg::Chunk("reply one".into()));
        app.feed_stream(StreamMsg::Complete);
        app.submit("second");
        app
    };
    let failed = || {
        let mut app = answered();
        app.feed_stream(StreamMsg::Error(BackendError::RateLimited));
        (app, "[error:", "the request failed")
    };
    let killed = || {
        let mut app = answered();
        app.arrive_and_gate_for_test(app.foreground_id(), "tu-1", "ls -la");
        app.ctrl_c_stop_for_test();
        (
            app,
            "⚙ cancelled",
            "you stopped it before its round finished",
        )
    };
    for (mut app, came, why) in [failed(), killed()] {
        let said = format!("[not in the conversation: {why}]");
        let lines = app.output().lines();
        assert!(lines.contains(&said), "{said}: {lines:#?}");
        assert!(lines.iter().any(|l| l.starts_with(came)), "{came} kept");
        let danger = app.theme().ui(Ink::Danger).fg;
        assert!(danger.is_some(), "premise: the theme paints danger");
        for focused in [false, true] {
            if focused {
                app.set_focus(PaneKind::Output);
            }
            let buf = frame_of(&mut app, 120, 40);
            for text in ["> second", came, said.as_str()] {
                assert_eq!(fg_of(&buf, text), danger, "{text} (focused {focused})");
            }
            assert_ne!(
                fg_of(&buf, "> first"),
                danger,
                "in the conversation, its own"
            );
            assert_ne!(fg_of(&buf, "reply one"), danger, "and its reply");
            // The rule framing your words, too; the other turn's its own.
            for (words, apart) in [("> second", true), ("> first", false)] {
                let rule = cell_at(&buf, words, 1);
                assert_eq!(rule.symbol(), "─", "premise: the rule above {words}");
                assert_eq!(rule.style().fg == danger, apart, "{words}'s rule");
            }
        }
    }
}

/// **Both early rungs of `CTRL-C` warn that another press has work to stop.**
///
/// `interrupt` is innermost-first: an open `:` line, then a pending `(y/N)`,
/// then the running work. The first two cancel something small and RETURN — so
/// a user reaching for the key that means *stop spending* reads "cancelled" as
/// though the reply stopped, and stops pressing while the request keeps
/// billing.
///
/// Rung 1 had a version of this and asked the wrong question:
/// `slot_busy_for(prompt_target())` is *"may I START work here"*, which is
/// `Scope::Global` — so a `:Send` or `:child` streaming into **another**
/// session was invisible to it, and two billed replies carried on under "the
/// `:` line was cancelled". Rung 2 had no clause at all. Both now ask
/// `anything_to_stop`, which is what `stop_work` itself takes by (Reviewer
/// F, both executed).
///
/// **What another press stops, and only that** (§9 phase 6b): `CTRL-C`
/// stops the work where the prompt goes, so the clause is said for a turn
/// there, and not for a child running in a session of its own — which the
/// next press names as running elsewhere, rather than a clause promising a
/// stop that press does not make.
#[tokio::test]
async fn cancelling_a_line_or_a_question_says_work_is_still_running() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.submit("a turn where the prompt goes");
    app.feed_stream(StreamMsg::Chunk("half an ans".into()));
    assert!(app.anything_to_stop(), "premise: something is running here");

    // Each rung is a keypress in production, and a keypress begins a moment —
    // see `begin_moment`. Driving `interrupt` directly skips that, so each rung
    // names its own boundary.
    // Rung 1: an open `:` line.
    app.open_command_line();
    app.notices.clear();
    app.interrupt();
    let notice = app.notice().expect("says something").to_string();
    assert!(
        notice.contains("still running"),
        "cancelling the `:` line warns the reply did not stop: {notice}"
    );

    // Rung 2: a pending question.
    app.ask(ConfirmAction::DeleteAll);
    app.notices.clear();
    app.interrupt();
    let notice = app.notice().expect("says something").to_string();
    assert!(
        notice.contains("still running"),
        "and so does cancelling a question — 'nothing done' is an affirmative \
         claim about the machine, and it was false: {notice}"
    );

    // Work in a session of its own: no clause, and the next press says where.
    app.feed_stream(StreamMsg::Complete);
    let child = app.spawn_child_job("go", None).expect("spawned");
    assert!(
        !app.anything_to_stop(),
        "premise: nothing runs where the prompt goes"
    );
    app.open_command_line();
    app.notices.clear();
    app.interrupt();
    let notice = app.notice().expect("says something").to_string();
    assert!(
        !notice.contains("still running"),
        "no stop promised that the next press would not make: {notice}"
    );
    app.notices.clear();
    app.interrupt();
    let notice = app.notice().expect("says something").to_string();
    assert!(
        notice.contains("running elsewhere") && notice.contains(&child.to_hex()[..8]),
        "{notice}"
    );

    // And with nothing running, neither adds the clause.
    app.feed_job(child, StreamMsg::Complete);
    assert!(app.generations.is_empty(), "premise: idle now");
    app.open_command_line();
    app.notices.clear();
    app.interrupt();
    let notice = app.notice().expect("says something").to_string();
    assert!(
        !notice.contains("still running"),
        "so it is a delta rather than a constant: {notice}"
    );
}

/// **`Esc` cancelling a distillation says so** — the graph legend advertises
/// "(Esc cancels)", and pressing it used to bill the tokens, destroy the draft,
/// and give no feedback but a status segment vanishing mid-row.
///
/// `CTRL-C` cancels the same generation and announced it: one act, two
/// accounts, one of them silence. The compound is worse — a user who reads the
/// silence as "that key did nothing" presses `Esc` again, and the second press
/// closes the viewer (Reviewer F).
#[tokio::test]
async fn esc_cancelling_a_distillation_says_so() {
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
    assert!(
        app.distilling().is_some(),
        "premise: a distillation is running"
    );

    app.cancel_summary();

    assert!(app.distilling().is_none(), "it was cancelled");
    let notice = app.notice().expect("says so").to_string();
    assert!(
        notice.contains("distillation cancelled"),
        "in the same words CTRL-C uses: {notice}"
    );
}

/// **CTRL-C leaves an audit record for every call it ends.** Dogfooded
/// 2026-09-24: the model proposed a command, CTRL-C ended the turn, and the
/// audit file had nothing — so what it had asked to run could not be answered
/// afterwards. Three calls in three states, through the real stop: one allowed
/// and running (`killed` — it ran, part-way), one on the pane and one queued
/// behind it (`cancelled` — never ran).
#[tokio::test]
async fn ctrl_c_records_every_call_it_ends() {
    let mut app = test_app();
    model_call(&mut app, "tu_run", "frobnicate --slow");
    tool_key(&mut app, 'a');
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_ask",
        "frobnicate --ask",
    )));
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_queued",
        "frobnicate --queued",
    )));
    app.feed_stream(StreamMsg::Complete);
    assert!(
        app.running_tools.contains_key("tu_run"),
        "premise: one running"
    );
    assert_eq!(
        app.shown_call().map(|p| p.tool_use_id().to_string()),
        Some("tu_ask".to_string()),
        "premise: one on the pane"
    );
    assert!(
        app.waiting_calls().iter().any(|(id, _)| *id == "tu_queued"),
        "premise: one queued: {:?}",
        app.waiting_calls()
    );

    // From the prompt: the decision pane, where `a` left the focus, stops no
    // work (Steven, 10-21; full review A 13).
    app.set_focus(PaneKind::Prompt);
    let logs = captured_logs(|| {
        app.ctrl_c_stop_for_test();
    });
    let record = |decision: &str, id: &str| {
        logs.lines()
            .filter(|l| {
                l.contains(&format!(r#"decision="{decision}""#))
                    && l.contains(&format!(r#"tool_use_id="{id}""#))
            })
            .count()
    };
    assert_eq!(record("killed", "tu_run"), 1, "the running one:\n{logs}");
    assert_eq!(
        record("cancelled", "tu_ask"),
        1,
        "the one on the pane:\n{logs}"
    );
    assert_eq!(
        record("cancelled", "tu_queued"),
        1,
        "the queued one:\n{logs}"
    );
    assert!(
        logs.contains(r#"command="frobnicate --ask""#),
        "a record that does not name its command answers nothing:\n{logs}"
    );
}
