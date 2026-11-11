//! `:continue`: a turn taken up where it stopped.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

#[tokio::test]
async fn the_stream_stop_reason_lands_in_the_sealed_turns_provenance() {
    use shannon_core::StopReason;
    // The wire order: chunks → usage → Stopped (message_delta) → Complete.
    // The sealed turn must record why it ended — this is `:continue`'s
    // ground truth for spotting a truncated reply.
    let mut app = test_app();
    app.output_mut().begin_turn("q");
    app.conversation_mut().begin_turn("q");
    app.feed_stream(StreamMsg::Chunk("cut of".into()));
    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(
        app.conversation()
            .head_turn()
            .unwrap()
            .provenance
            .stop_reason,
        Some(StopReason::MaxTokens),
        "truncation is visible in provenance"
    );
}

#[tokio::test]
async fn continue_refuses_with_the_reason_when_nothing_is_resumable() {
    use shannon_core::StopReason;
    let mut app = test_app();
    app.continue_last_turn();
    assert!(app.notice().expect("refused").contains("no turns"));

    // A turn that ended normally is not resumable — say why.
    app.output_mut().begin_turn("q");
    app.conversation_mut().begin_turn("q");
    app.feed_stream(StreamMsg::Chunk("done.".into()));
    app.feed_stream(StreamMsg::Stopped(StopReason::EndTurn));
    app.feed_stream(StreamMsg::Complete);
    app.continue_last_turn();
    assert!(app.notice().expect("refused").contains("ended normally"));

    // A pre-P7 turn recorded nothing — refuse honestly, don't guess.
    let mut app = test_app();
    app.conversation_mut().begin_turn("old");
    app.conversation_mut().push_chunk("reply");
    app.conversation_mut().commit_turn();
    app.continue_last_turn();
    assert!(app.notice().expect("refused").contains("predates"));
}

#[tokio::test]
async fn continue_instruct_sends_the_templated_follow_up() {
    use shannon_core::StopReason;
    // The universal strategy: a truncated turn is resumed by an ordinary
    // follow-up quoting the tail of the partial — honest in the
    // transcript, works on every model.
    let mut app = test_app();
    app.output_mut().begin_turn("write a poem");
    app.conversation_mut().begin_turn("write a poem");
    app.feed_stream(StreamMsg::Chunk("Roses are".into()));
    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);

    app.continue_last_turn();
    let last = app.conversation().messages().last().unwrap().clone();
    assert_eq!(last.role, shannon_core::Role::User);
    assert!(last.content.contains("Roses are"), "quotes the tail");
    assert!(last.content.contains("Continue exactly"), "the template");
    assert!(app.streaming(), "an ordinary turn is in flight");
}

#[tokio::test]
async fn continue_prefill_composes_one_combined_turn_in_place() {
    use shannon_core::{ContinueStrategy, StopReason};
    // The exact-splice strategy: the model extends the partial and the
    // pair seals back into ONE turn, composed over the truncated one.
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
    let original = app.conversation().episode().id;
    // Named: the continuation goes on AS this session, so its name follows
    // (review of 34033e8, Reviewer A: the carry was held by nothing).
    app.dispatch_command_for_test("rename poem");

    app.continue_last_turn();
    assert!(app.continuing(), "prefill streams off-transcript");
    // Prompts submitted meanwhile queue behind the continuation.
    app.submit("queued prompt");
    assert_eq!(app.conversation().messages().len(), 2, "nothing mid-flight");

    app.feed_continuation(StreamMsg::Chunk(" red".into()));
    app.feed_continuation(StreamMsg::Usage(Usage {
        input_tokens: 7,
        output_tokens: 2,
        ..Default::default()
    }));
    app.feed_continuation(StreamMsg::Stopped(StopReason::EndTurn));
    app.feed_continuation(StreamMsg::Complete);

    assert!(!app.continuing());
    let composed = app.conversation().episode().id;
    assert_ne!(
        composed, original,
        "a NEW composed session — originals kept"
    );
    assert_eq!(
        app.label_of(composed).name.as_deref(),
        Some("poem"),
        "the name went on"
    );
    assert_eq!(
        app.label_of(original).name,
        None,
        "and left the truncated original"
    );
    let msgs = branch_msgs(&app);
    assert_eq!(msgs[0], "write a poem");
    assert_eq!(msgs[1], "Roses are red", "partial + continuation, spliced");
    // The queued prompt drained onto the composed session.
    assert_eq!(msgs[2], "queued prompt");

    // The combined turn's provenance: the continuation's own stop reason
    // (no longer interrupted) and the summed cost of both calls.
    let g = app.current_graph();
    let combined = g.placed(&composed)[0];
    assert_eq!(combined.provenance.stop_reason, Some(StopReason::EndTurn));
    assert_eq!(
        combined.provenance.usage,
        Some(Usage {
            input_tokens: 7,
            output_tokens: 2,
            ..Default::default()
        }),
    );
    assert!(
        g.episode(&original).is_some(),
        "the truncated original stays in :graph"
    );
}

/// **A continued turn's window is measured** — the combined turn takes the
/// truncated one's place, so it follows what THAT turn followed (here the
/// first turn), and its window stands. Claiming to follow nothing, it would
/// read as moved and its exact window as an estimate (`~`).
#[tokio::test]
async fn a_continued_later_turn_keeps_its_measured_window() {
    use shannon_core::{ContinueStrategy, StopReason};
    let mut cfg = Config::default();
    cfg.continue_.strategy = ContinueStrategy::Prefill;
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    for (prompt, reply, stop) in [
        ("first", "done.", StopReason::EndTurn),
        ("write a poem", "Roses are", StopReason::MaxTokens),
    ] {
        app.output_mut().begin_turn(prompt);
        app.conversation_mut().begin_turn(prompt);
        app.feed_stream(StreamMsg::Chunk(reply.into()));
        app.feed_stream(StreamMsg::Stopped(stop));
        app.feed_stream(StreamMsg::Complete);
    }
    app.continue_last_turn();
    app.feed_continuation(StreamMsg::Chunk(" red".into()));
    app.feed_continuation(StreamMsg::Usage(Usage {
        input_tokens: 700,
        output_tokens: 20,
        ..Default::default()
    }));
    app.feed_continuation(StreamMsg::Stopped(StopReason::EndTurn));
    app.feed_continuation(StreamMsg::Complete);
    assert_eq!(branch_msgs(&app)[3], "Roses are red", "premise: composed");
    let billing = app.buffer().billing;
    assert!(!billing.estimated, "measured on the history it follows");
    assert_eq!(billing.context, 720, "the continuation's own window");
}

/// **Identical exchanges show their own facts wherever they are read.** Two
/// `hi` → `Hello.` turns — one content — the first complete and cheap, the
/// second cut off and dearer. Read by content, each reader got the FIRST's
/// facts: `:continue` refused the cut-off tip, the inspector showed the wrong
/// stop and tokens, and the session's spend undercounted. The graph keeps them
/// apart (43); these readers had no test of their own (Reviewer A).
#[tokio::test]
async fn identical_turns_show_their_own_facts_to_every_reader() {
    use shannon_core::StopReason;
    let mut app = test_app();
    for (out, stop) in [(10, StopReason::EndTurn), (30, StopReason::MaxTokens)] {
        app.conversation_mut().begin_turn("hi");
        app.conversation_mut().push_chunk("Hello.");
        app.conversation_mut()
            .record_usage(shannon_core::TurnCost::single_round(Usage {
                output_tokens: out,
                ..Default::default()
            }));
        app.conversation_mut().record_stop_reason(stop);
        app.conversation_mut().commit_turn();
    }
    let ep = app.conversation().episode().id;
    let graph = app.current_graph();
    let turn = graph.placed(&ep)[1].turn;
    assert_eq!(graph.placed(&ep)[0].turn, turn, "premise: one content");

    let inspect = |app: &App, node: Node| -> String {
        let mut view = app.build_graph_view();
        assert!(view.select_node(node), "premise: {node:?} is a row");
        crate::view::inspector_body(&view, true, "unix")
            .iter()
            .map(crate::paint::PageLine::text)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let second = inspect(
        &app,
        Node::Turn {
            episode: ep,
            turn,
            index: 1,
        },
    );
    assert!(
        second.contains("↓30") && second.contains("max_tokens"),
        "the second turn's own bill and stop:\n{second}"
    );
    let session = inspect(&app, Node::Episode(ep));
    assert!(
        session.contains("↓40"),
        "both bills, not one twice:\n{session}"
    );

    app.continue_last_turn();
    assert!(
        app.streaming(),
        "the cut-off tip continues: {:?}",
        app.notice()
    );
}

#[tokio::test]
async fn continue_reads_the_derived_tip_not_own_head_after_a_re_root() {
    // Deep-review regression (Reviewer A): `:continue` must resolve the
    // session's DERIVED tip — the turn the user sees last — not the
    // conversation's own_head. They diverge on a re-rooted session
    // ([Own(new root), Slice(original)]): own_head is the authored root
    // (stop_reason None), while the visible tip is the shared, truncated
    // original turn. Reading own_head wrongly refused a continuable tip.
    use shannon_core::Role::{Assistant, User};
    use shannon_core::StopReason;
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()), // Instruct strategy
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    // Build an original whose TIP is truncated (max_tokens).
    app.conversation_mut().begin_turn("Hello 0");
    app.conversation_mut().push_chunk("reply 0");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    app.conversation_mut().begin_turn("Hello 1");
    app.conversation_mut().push_chunk("reply 1 cut");
    app.conversation_mut()
        .record_stop_reason(StopReason::MaxTokens);
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let original = app.conversation().episode().id;

    // Edit-prepend a fresh root → re-root: [Own(new root), Slice(original)].
    let edited = edited_transcript(&[
        (User, "new root"),
        (Assistant, "new root reply"),
        (User, "Hello 0"),
        (Assistant, "reply 0"),
        (User, "Hello 1"),
        (Assistant, "reply 1 cut"),
    ]);
    app.commit_slice_edit(original, 0, 2, &edited).unwrap();
    assert!(
        matches!(
            app.conversation().episode().segments(),
            [Segment::Own(_), Segment::Slice { .. }]
        ),
        "re-rooted: own_head is the authored root, tip is the shared original",
    );

    // :continue must NOT refuse — the derived tip is continuable.
    app.continue_last_turn();
    assert!(
        app.streaming(),
        "continue resumed the truncated tip (before the fix it refused, reading own_head)",
    );
    let sent = app.conversation().messages().last().unwrap();
    assert!(
        sent.content.contains("reply 1 cut"),
        "the instruction quotes the TIP's partial, not own_head's",
    );
}
