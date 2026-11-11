//! A reply streaming in: chunks, ends, and the transcript they build.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

#[tokio::test]
async fn complete_records_assistant_message_and_clears_stream() {
    let mut app = test_app();
    app.fake_stream();
    // Mirror a real turn caught mid-stream.
    app.conversation_mut().begin_turn("hi");
    app.conversation_mut().push_chunk("Hi there.");

    app.feed_stream(StreamMsg::Complete);

    assert!(!app.streaming());
    assert!(!app.conversation().turn_in_progress());
    let msgs = app.conversation().messages();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].content, "Hi there.");
}

#[tokio::test]
async fn usage_merges_per_turn_and_sums_across_turns() {
    let mut app = test_app();
    // Turn 1: input on `message_start`, output on `message_delta` —
    // separate readings, each with the other field 0, merged per-field.
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 100,
        output_tokens: 0,
        ..Default::default()
    }));
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 0,
        output_tokens: 40,
        ..Default::default()
    }));
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(
        app.buffer().billing.total,
        Usage {
            input_tokens: 100,
            output_tokens: 40,
            ..Default::default()
        }
    );
    // The per-turn accumulator resets, so turn 2 doesn't double-count.
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 30,
        output_tokens: 7,
        ..Default::default()
    }));
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(
        app.buffer().billing.total,
        Usage {
            input_tokens: 130,
            output_tokens: 47,
            ..Default::default()
        }
    );
}

/// **A backend's own tool rounds are separate requests** (`NextRequest`,
/// Claude Code): the bill sums them, and the window is the LAST request's
/// prompt and reply. Steven, 10-15: a Claude Code session on a 1M window read
/// `ctx 100%` — every round's prompt summed as one request.
#[tokio::test]
async fn a_backends_own_rounds_bill_the_sum_and_fill_the_window_with_the_last() {
    let mut app = test_app();
    for (prompt, out) in [(100_000, 500), (101_000, 300), (102_000, 900)] {
        app.feed_stream(StreamMsg::NextRequest);
        app.feed_stream(StreamMsg::Usage(Usage {
            input_tokens: 10,
            cache_read_input_tokens: prompt - 10,
            ..Default::default()
        }));
        app.feed_stream(StreamMsg::Usage(Usage {
            output_tokens: out,
            ..Default::default()
        }));
    }
    app.feed_stream(StreamMsg::Complete);
    let billing = app.buffer().billing;
    assert_eq!(
        billing.total.prompt_tokens(),
        303_000,
        "the bill: every request"
    );
    assert_eq!(billing.total.output_tokens, 1_700);
    assert_eq!(
        billing.context, 102_900,
        "the window: the last request's prompt and its reply"
    );
}

/// **A window replayed on a history it did not measure is estimated — from
/// the latest window this history DID measure** (`render_session`): that
/// window plus the estimated text added since, so the per-character error of
/// the history both share cancels. A turn whose window measured another
/// history (here: answered after a turn no longer before it) is not used.
#[test]
fn a_window_from_another_history_is_estimated_from_this_ones_last_measurement() {
    use shannon_core::graph::{Follows, Turn, TurnConfig};
    let config = TurnConfig {
        model: "stub".into(),
        max_tokens: None,
        effort: None,
    };
    let turn = |text: &str, follows: Follows, window: u32| {
        let mut t = Turn::seal(
            vec![Message::user(text), Message::assistant(text)],
            config.clone(),
            std::time::SystemTime::UNIX_EPOCH,
        );
        let round = Usage {
            input_tokens: window,
            ..Default::default()
        };
        t.provenance.usage = Some(round);
        t.provenance.last_round = Some(round);
        t.provenance.follows = Some(follows);
        t
    };
    // 2,000 characters a turn: 500 tokens at four a token.
    let text = "x".repeat(1_000);
    let first = turn(&text, Follows::Start, 20_500);
    let second = turn(&text, Follows::Turn(first.id), 21_000);
    let elsewhere = turn("gone", Follows::Start, 1);
    let moved = turn(&text, Follows::Turn(elsewhere.id), 99_999);
    let (_, billing) = App::render_session(
        &shannon_core::Aliases::default(),
        None,
        &[first, second, moved],
    );
    assert!(
        billing.estimated,
        "the last window measured another history"
    );
    assert_eq!(
        billing.context, 21_500,
        "the second's measured 21,000 and the third's 500 estimated, not 99,999"
    );
}

/// **Claude Code compacting its own context is said** — from there its model
/// works from a summary while this session keeps every turn, and nothing on
/// screen would otherwise differ. Unasked, the notice names the setting that
/// stops it.
#[tokio::test]
async fn claude_codes_own_compaction_is_said() {
    let mut app = test_app();
    app.submit("long work");
    app.notices.clear();
    app.feed_stream(StreamMsg::ProviderCompacted {
        auto: true,
        before: 912_345,
    });
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("compacted its context at 912345 tokens"),
        "{said}"
    );
    assert!(
        said.contains("auto_compact = false"),
        "the way to stop it: {said}"
    );
    assert!(
        !said.starts_with(':'),
        "the foreground's own: unlabelled: {said}"
    );
}

/// **An agentic turn's tool rounds are not "output it never showed"** —
/// Steven, 10-15: every Claude Code turn ended on "billed 25972 output tokens
/// and showed almost none of it". The rounds' output is their tool calls; the
/// reply is the last request's, and that is what the alarm judges.
#[tokio::test]
async fn tool_rounds_output_does_not_trip_the_unshown_output_alarm() {
    let mut app = test_app();
    app.submit("do the work");
    for out in [12_000, 13_000] {
        app.feed_stream(StreamMsg::NextRequest);
        app.feed_stream(StreamMsg::Usage(Usage {
            output_tokens: out,
            ..Default::default()
        }));
    }
    app.feed_stream(StreamMsg::NextRequest);
    app.feed_stream(StreamMsg::Chunk("a closing paragraph. ".repeat(40)));
    app.feed_stream(StreamMsg::Usage(Usage {
        output_tokens: 210,
        ..Default::default()
    }));
    app.notices.clear();
    app.feed_stream(StreamMsg::Complete);
    let said = app.notice().unwrap_or_default();
    assert!(!said.contains("showed almost none"), "{said}");
    assert_eq!(
        app.buffer().billing.total.output_tokens,
        25_210,
        "premise: every round was billed"
    );
}

#[tokio::test]
async fn error_seals_a_billed_partial_and_drops_an_empty_turn() {
    use shannon_core::StopReason;
    // P7: a mid-stream failure no longer destroys billed work. With
    // partial content, the turn seals as `interrupted` (real, persisted,
    // continuable); with nothing streamed, the user message still drops
    // so history stays role-alternating.
    let mut app = test_app();
    app.fake_stream();
    app.conversation_mut().begin_turn("hi");
    app.conversation_mut().push_chunk("partial");

    app.feed_stream(StreamMsg::Error(BackendError::RateLimited));

    assert!(!app.streaming());
    assert!(!app.conversation().turn_in_progress());
    let m = app.conversation().messages();
    assert_eq!(
        (m.len(), m[1].content.as_str()),
        (2, "partial"),
        "the billed partial is a real turn, not vapor"
    );
    assert_eq!(
        app.conversation()
            .head_turn()
            .unwrap()
            .provenance
            .stop_reason,
        Some(StopReason::Interrupted),
    );
    assert!(app.notice().expect("announced").contains(":continue"));
    let raw = app.output().lines().join("\n");
    assert!(raw.contains("rate limited"));

    // Nothing streamed → the old drop, so no dangling user message.
    app.fake_stream();
    app.conversation_mut().begin_turn("again");
    app.feed_stream(StreamMsg::Error(BackendError::RateLimited));
    assert_eq!(
        app.conversation().messages().len(),
        2,
        "an empty failure leaves no dangling user message"
    );
}

#[tokio::test]
async fn submit_mid_stream_queues_without_disturbing_output() {
    let mut app = test_app();
    app.output_mut().begin_turn("first");
    app.output_mut().append_chunk("partial");
    app.fake_stream();
    let lines_before = app.output().lines();

    app.submit("second");
    app.submit("third");

    assert_eq!(app.output().lines(), lines_before);
    assert_eq!(
        app.pending
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        vec!["second", "third"]
    );
    assert!(app.streaming());
}

#[tokio::test]
async fn drain_budget_never_loses_messages() {
    // A burst longer than STREAM_DRAIN_BUDGET must arrive intact:
    // the budget may defer messages to the next loop turn, but it
    // must never pull one off the channel and drop it. Regression
    // test for exactly that off-by-one (a dropped Chunk lost text;
    // a dropped Complete wedged the turn gate).
    let mut app = test_app();
    app.output_mut().begin_turn("hi");
    app.conversation_mut().begin_turn("hi");
    let generation = app.fake_stream();

    let n_chunks = STREAM_DRAIN_BUDGET + 5;
    for _ in 0..n_chunks {
        app.stream_tx
            .send((generation, StreamMsg::Chunk("x".to_string())))
            .unwrap();
    }
    app.stream_tx
        .send((generation, StreamMsg::Complete))
        .unwrap();

    // Emulate the event loop: each wakeup handles one received
    // message plus a budgeted drain of the backlog.
    while let Ok(first) = app.stream_rx.try_recv() {
        app.drain_stream(first);
    }

    assert!(!app.streaming(), "Complete must be handled");
    let msgs = app.conversation().messages();
    assert_eq!(msgs.len(), 2);
    assert_eq!(
        msgs[1].content.chars().count(),
        n_chunks,
        "every chunk applied"
    );
}

/// **The row says when a queued prompt goes** — it said "queued (1)" and
/// nothing replaced it, so it went on announcing a queue that had drained
/// while the queued turn ran (Steven, 10-15, from the stub).
#[tokio::test]
async fn the_row_says_when_a_queued_prompt_is_sent() {
    let mut app = test_app();
    app.output_mut().begin_turn("first");
    app.conversation_mut().begin_turn("first");
    app.fake_stream();
    app.submit("second");
    assert!(
        app.notice().is_some_and(|n| n.starts_with("queued (1)")),
        "premise: {:?}",
        app.notice()
    );
    app.feed_stream(StreamMsg::Complete);
    assert!(app.streaming(), "premise: the queued turn runs");
    assert_eq!(app.notice().as_deref(), Some("the queued prompt is sent"));
}

#[tokio::test]
async fn complete_drains_next_queued_prompt() {
    let mut app = test_app();
    app.output_mut().begin_turn("first");
    app.output_mut().append_chunk("partial");
    app.conversation_mut().begin_turn("first");
    app.fake_stream();
    app.pending.push_back((app.current, "second".to_string()));

    app.feed_stream(StreamMsg::Complete);

    let lines = app.output().lines();
    assert_eq!(
        lines[..8],
        ["", "> first", "", "", "partial", "", "", "> second"]
    );
    assert!(app.pending.is_empty());
    assert!(app.streaming());
    // Transcript after the Complete drain: [first user, first
    // assistant (empty — no chunks pushed), second user]. The
    // backend produces the second turn's reply on its own; we
    // don't await it here.
    assert_eq!(app.conversation().messages().len(), 3);
}

/// A session costs the same whether you **resumed** into it or **opened** it.
///
/// Startup resume hand-rolled the replay loop instead of calling
/// `render_session` — whose doc named it as a consumer — and so skipped the
/// usage tally. The token total read zero on relaunch and its real value after
/// `:open`, for the same session: one number decided by how you got there.
/// Flagged by a reviewer on 2026-07-15 and left unapplied until 2026-08-03.
#[test]
fn a_resumed_session_carries_the_tokens_it_already_spent() {
    let dir = TempDir::new().unwrap();
    let spent = shannon_core::Usage {
        input_tokens: 120,
        output_tokens: 45,
        ..Default::default()
    };
    let episode = {
        let mut app = test_app_with_store(
            test_backend(),
            Arc::new(Config::default()),
            Box::new(JsonlStore::new(dir.path()).unwrap()),
        );
        app.conversation_mut().begin_turn("Hello");
        app.conversation_mut().push_chunk("reply");
        app.conversation_mut()
            .record_usage(TurnCost::single_round(spent));
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
        app.conversation().episode().id
    };

    // Relaunch resuming the last session — what `[tui] resume = "last"` does.
    let mut cfg = Config::default();
    cfg.tui.resume = shannon_core::ResumeMode::Last;
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    assert_eq!(
        app.conversation().episode().id,
        episode,
        "we really resumed that session"
    );
    let resumed = app.buffer().billing.total;
    assert_ne!(
        resumed,
        Usage::default(),
        "the resumed session knows its cost"
    );

    // And the other door onto the same session agrees.
    app.reset_foreground();
    app.open_command(&episode.to_hex()[..8]);
    assert_eq!(
        app.buffer().billing.total,
        resumed,
        "`:open` and resume report the same total for the same session"
    );
}

/// A turn's **token tally belongs to the turn**, so leaving cannot carry it.
///
/// `turn_usage` lived on `App` — one field for whichever turn happened to be
/// running — so a tally with no turn left to close (an aborted stream, or a
/// turn belonging to a child job) rode across a session switch and was billed
/// to the next session. It was patched by zeroing the field on every exit
/// path, which is a rule to remember at each of them. The tally is on the
/// generation now, and a generation is on exactly one buffer, so there is
/// nowhere for it to ride to.
#[tokio::test]
async fn a_turns_tally_cannot_ride_across_to_the_next_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let was = app.current;
    app.submit("spend some tokens");
    app.feed_stream(StreamMsg::Chunk("partial".into()));
    app.feed_stream(StreamMsg::Usage(shannon_core::Usage {
        input_tokens: 100,
        output_tokens: 40,
        ..Default::default()
    }));

    app.new_session_command();

    assert_ne!(app.current, was, "we did move to a new session");
    assert_eq!(
        app.buffer().billing.total,
        Usage::default(),
        "the fresh session was not billed for a turn it never ran"
    );

    // The turn finishes where it started, and that is who pays.
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(
        app.session_of(was)
            .expect("the old buffer stays open")
            .billing
            .total,
        Usage {
            input_tokens: 100,
            output_tokens: 40,
            ..Default::default()
        },
        "the tally stayed with the session that earned it"
    );
    assert_eq!(
        app.buffer().billing.total,
        Usage::default(),
        "and never reached the fresh one"
    );
}

/// **A reply's `**bold**` and `*italic*` are drawn so, as it streams**
/// (Steven, 11-01: "looks like Claude is trying to use Markdown, don't we
/// support that?") — the rendered cells, in the pane and focused. A run
/// whose partner has not arrived is plain until it does; the markers stay,
/// dim, so nothing moves when it does.
#[tokio::test]
async fn emphasis_is_drawn_as_it_streams_its_markers_dim() {
    use super::stop::cell_at;
    use ratatui::style::Modifier;
    let mut app = test_app();
    app.submit("look");
    app.feed_stream(StreamMsg::Chunk("**firm".into()));
    let early = frame_of(&mut app, 100, 30);
    assert!(
        cell_at(&early, "**firm", 0).style().add_modifier.is_empty()
            && cell_at(&early, "firm", 0).style().add_modifier.is_empty(),
        "unpartnered yet: as written"
    );
    app.feed_stream(StreamMsg::Chunk("** then *soft*".into()));
    let partnered = frame_of(&mut app, 100, 30);
    assert!(
        (cell_at(&partnered, "firm", 0).style().add_modifier).contains(Modifier::BOLD),
        "bold the moment its partner arrives"
    );
    app.feed_stream(StreamMsg::Complete);
    for focused in [false, true] {
        if focused {
            app.set_focus(PaneKind::Output);
        }
        let buf = frame_of(&mut app, 100, 30);
        let worn = |text: &str| cell_at(&buf, text, 0).style().add_modifier;
        assert_eq!(worn("firm"), Modifier::BOLD, "focused {focused}");
        assert_eq!(worn("**firm"), Modifier::DIM, "focused {focused}");
        assert_eq!(worn("soft"), Modifier::ITALIC, "focused {focused}");
        assert_eq!(worn("*soft"), Modifier::DIM, "focused {focused}");
        assert_eq!(worn("then"), Modifier::empty(), "focused {focused}");
    }
}

/// **A `:tool` typed while the reply streams does not cut it** (Steven,
/// 11-01: "the old brass h", the record, "and lamp") — its record waits for
/// the reply's words to stop and goes in below them; and you are told so,
/// or the command you typed seemed to do nothing. A model's call is the
/// turn's, and goes in as it comes.
#[tokio::test]
async fn a_typed_call_does_not_cut_the_reply_streaming() {
    let mut app = test_app();
    app.submit("tell me a story");
    app.feed_stream(StreamMsg::Chunk("the old brass h".into()));
    let buffer = app.prompt_target();
    manual_tool(&mut app, "rm -rf /tmp/shannon-test-absent");
    app.settle();
    app.notices.clear();
    tool_key_in(&mut app, buffer, 'd');
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("when the reply's words stop"), "{said}");
    app.feed_stream(StreamMsg::Chunk("and lamp".into()));
    app.feed_stream(StreamMsg::Complete);
    let lines = app.output().lines();
    let at = |text: &str| lines.iter().position(|l| l.contains(text));
    assert!(at("the old brass hand lamp").is_some(), "whole: {lines:#?}");
    assert!(at("hand lamp") < at("DENIED"), "below it: {lines:#?}");
}
