//! **`:compact`** (Steven, 10-14: "We'll need to implement that for daily
//! usage"): a session's history into one summary the model writes, replacing
//! the session — by default as Claude Code's `/compact` does (10-16): no
//! review, nothing kept verbatim, the words after it instructions. Tests of
//! something else that lean on a kept turn or a review say so with
//! [`keeping_one`] and [`reviewing`].

use super::*;

/// `[merge] compact_keep = 1`: the last exchange stays as it was — for a
/// test whose subject is the kept turn, not the default.
fn keeping_one(app: &mut App) {
    app.dispatch_command_for_test("set merge.compact_keep=1");
    assert_eq!(
        app.config.merge.compact_keep,
        1,
        "premise: {:?}",
        app.notice()
    );
}

/// `[merge] compact_review = true`: the summary opens in `$EDITOR` first.
fn reviewing(app: &mut App) {
    app.dispatch_command_for_test("set merge.compact_review=true");
    assert!(
        app.config.merge.compact_review,
        "premise: {:?}",
        app.notice()
    );
}

/// The instruction the distillation in flight RECORDS — what its summary is
/// sealed with. What was sent is the request's
/// (`compact_sends_its_instructions`).
fn instruction_recorded(app: &App) -> String {
    app.generations
        .iter()
        .find_map(|g| match &g.work {
            Work::Summary(s) => Some(s.instruction.clone()),
            _ => None,
        })
        .expect("a distillation is running")
}

/// The distillation in flight: its id and what it will replace.
fn distilling(app: &App) -> (GenerationId, SquashTarget, Vec<TurnId>, bool) {
    app.generations
        .iter()
        .find_map(|g| match &g.work {
            Work::Summary(s) => Some((g.id, s.target.clone(), s.sources.clone(), s.review)),
            _ => None,
        })
        .expect("a distillation is running")
}

/// **`:compact` is Claude Code's `/compact` by default** (Steven, 10-16: "I
/// want the default `:compact` to act like Claude Code's default
/// `/compact`"): the whole history into one summary, replaced unread — and
/// `[merge] compact_keep` keeps that many last turns as they were. It kept
/// one, and opened the summary in `$EDITOR` first.
#[tokio::test]
async fn compact_by_default_summarises_everything_unread() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 4);
    let episode = app.conversation().episode().id;
    let turns = app.current_graph().full_turns(&episode);
    app.notices.clear();
    app.dispatch_command_for_test("compact");
    let (_, target, sources, review) = distilling(&app);
    assert!(
        matches!(target, SquashTarget::Range { episode: e, start: 0, end: 4, .. } if e == episode),
        "{target:?}"
    );
    assert_eq!(sources, turns, "every turn");
    assert!(!review, "no review, as /compact asks none");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("compacting 4 turns") && said.contains("it replaces them when it is written"),
        "{said}"
    );
    assert_eq!(
        instruction_recorded(&app),
        app.config.merge.squash_instruction,
        "the configured instruction, nothing added"
    );

    let other = TempDir::new().unwrap();
    let mut app = app_with_n_turns(other.path(), 4);
    keeping_one(&mut app);
    app.dispatch_command_for_test("compact");
    assert_eq!(
        distilling(&app).2.len(),
        3,
        "`compact_keep = 1` keeps the last"
    );
}

/// **`--keep N` keeps the last N turns, for this one compaction** (Steven,
/// 10-16: "I like your idea about `--keep`") — the one-off `[merge]
/// compact_keep`, where `:compact N` was. In the grammar every flag-taking
/// command shares (`flags::parse_flags`, review stop 2, C 5): flags lead;
/// `--` ends them; a flag it does not know is said, not sent to the model as
/// words — a single-dash one too, as everywhere else.
#[test]
fn compact_reads_keep_then_instructions() {
    use crate::app::session_ops::compact_args;
    assert_eq!(compact_args(""), Ok((None, "")));
    assert_eq!(compact_args("keep the bugs"), Ok((None, "keep the bugs")));
    assert_eq!(compact_args("--keep 2"), Ok((Some(2), "")));
    assert_eq!(
        compact_args("--keep=2 keep the bugs"),
        Ok((Some(2), "keep the bugs"))
    );
    assert_eq!(
        compact_args("  --keep 1   the  rest  "),
        Ok((Some(1), "the  rest")),
        "the instructions as typed, between their ends"
    );
    assert_eq!(
        compact_args("-- --keep is a word here"),
        Ok((None, "--keep is a word here"))
    );
    assert_eq!(
        compact_args("the --keep later is words"),
        Ok((None, "the --keep later is words")),
        "only leading flags are flags"
    );
    assert_eq!(
        compact_args("-- 3"),
        Ok((None, "3")),
        "after `--`, a number is words"
    );
    assert_eq!(compact_args("--keep 2 5"), Ok((Some(2), "5")));
    assert!(
        compact_args("--keep")
            .unwrap_err()
            .contains("needs a value")
    );
    assert!(
        compact_args("--keep=").unwrap_err().contains("`--keep=2`"),
        "an empty value says what to write, not \"not ``\""
    );
    assert!(
        compact_args("--keep two")
            .unwrap_err()
            .contains("not `two`")
    );
    assert!(
        compact_args("--frob x")
            .unwrap_err()
            .contains("unknown flag --frob")
    );
    assert!(
        compact_args("-k 2")
            .unwrap_err()
            .contains("unknown flag -k"),
        "a single-dash flag is a flag here too"
    );
}

/// **…and reaches the compaction with the instructions after it.**
#[tokio::test]
async fn compact_keep_flag_keeps_and_instructs() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 4);
    app.dispatch_command_for_test("compact --keep 1 keep the open bugs");
    assert_eq!(distilling(&app).2.len(), 3, "the last turn kept, for once");
    assert!(
        instruction_recorded(&app).ends_with("Additional instructions: keep the open bugs"),
        "{}",
        instruction_recorded(&app)
    );
    assert_eq!(app.config.merge.compact_keep, 0, "the setting is untouched");
}

/// **`:compact <instructions>` tells the summary what to keep**, as
/// `/compact <instructions>` does: asked after the configured instruction
/// and recorded with it, for that one summary.
#[tokio::test]
async fn compact_takes_instructions_as_compact_does() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    app.dispatch_command_for_test("compact keep the API decisions and the open bugs");
    assert_eq!(
        instruction_recorded(&app),
        format!(
            "{}\n\nAdditional instructions: keep the API decisions and the open bugs",
            app.config.merge.squash_instruction
        )
    );
    assert_eq!(distilling(&app).2.len(), 3, "still the whole history");
}

/// **…and the instructions are SENT** (review stop 2, C 2): the tests above
/// read what the distillation records, and the request is built beside it —
/// a mutant sending the bare instruction while recording the composed one
/// passed every one of them, and the summary would ignore what you asked.
#[tokio::test]
async fn compact_sends_its_instructions() {
    let dir = TempDir::new().unwrap();
    let (mut app, provider) = provider_app();
    app.claude_program = logged_in_claude(dir.path());
    app.model_command("claude-code/sonnet");
    app.submit("first");
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::Complete);
    app.dispatch_command_for_test("compact keep the open bugs");
    let sent = provider
        .requests
        .lock()
        .unwrap()
        .last()
        .and_then(|r| r.messages.last())
        .map(|m| m.content.clone());
    assert_eq!(
        sent,
        Some(format!(
            "{}\n\nAdditional instructions: keep the open bugs",
            app.config.merge.squash_instruction
        )),
        "the request closes with the instruction and what you added"
    );
}

/// **An unreviewed `:compact` composes when the summary arrives, and moves
/// no viewport** — the session becomes the summary and the kept turn, its
/// name going on with it and the result written at once; the original stays,
/// unnamed; and a pane you turned to `:help` meanwhile goes on showing the
/// manual (a result that ARRIVES does not take a viewport back).
#[tokio::test]
async fn compact_composes_when_the_summary_arrives() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    keeping_one(&mut app);
    let original = app.conversation().episode().id;
    let kept = *app.current_graph().full_turns(&original).last().unwrap();
    app.dispatch_command_for_test("rename auth-fix");
    app.dispatch_command_for_test("compact");
    let (_, _, _, review) = distilling(&app);
    assert!(!review, "premise: no review asked for");

    app.dispatch_command_for_test("help");
    let pane = app.workspace.focused();
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Help)),
        "premise"
    );
    for msg in [
        StreamMsg::Chunk("what was done".into()),
        StreamMsg::Complete,
    ] {
        app.feed_summary(msg);
        assert!(
            app.decisions.every_review().next().is_none(),
            "nothing to review"
        );
    }

    let now = app.buffer().conversation.episode().id;
    assert_ne!(now, original, "the session was replaced");
    let full = app.current_graph().full_turns(&now);
    assert_eq!(full.len(), 2, "one summary, one kept turn");
    assert_eq!(full[1], kept, "the last turn kept as it was");
    assert_eq!(
        app.label_of(now).name.as_deref(),
        Some("auth-fix"),
        "the name went on"
    );
    assert_eq!(app.label_of(original).name, None, "and left the original");
    assert!(
        matches!(app.store.episode_len(&now), Ok(Some(_))),
        "written at once"
    );
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Help)),
        "the manual you turned to stays — an arrival takes no viewport"
    );
}

/// **A composition's sources are written before it is** — the reproduction
/// of review 34033e8 (Reviewers A and B): a squash result not yet written,
/// named, then compacted — the compacted result was written at once (its
/// name carried) and the squash result it shares turns of never was, so a
/// relaunch read it back a turn short, with no word said.
///
/// A squash result is written at once now (10-16, decision (a)), so its own
/// summary is unwritten only when that write FAILED — the state built here,
/// a store directory refusing new files while it composes. (A source that
/// owns nothing, a lazy clone, is never sliced: `compose` projects a span
/// onto the turns' owners.)
#[cfg(unix)]
#[tokio::test]
async fn a_composition_from_an_unsaved_source_reloads_whole() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let graph = app.current_graph();
    let config = graph.episode(&base).unwrap().config.clone();
    let turns = graph.full_turns(&base);
    let summary = Turn::seal(
        vec![Message::user("S"), Message::assistant("first summary")],
        config.clone(),
        std::time::SystemTime::UNIX_EPOCH,
    );
    let episodes = dir.path().join("episodes");
    let mode = |m| std::fs::set_permissions(&episodes, std::fs::Permissions::from_mode(m));
    mode(0o500).unwrap(); // no new episode file: the squash result's write fails
    // Turns [1,3) squashed, so the result's LAST turn is its own summary —
    // the turn a `:compact! 1` keeps, by reference to this unsaved session.
    let squashed = app
        .adopt_composed(
            app.current,
            graph,
            &Squash {
                episode: base,
                start: 1,
                end: 3,
                summary: vec![summary],
            },
            Anchor {
                base,
                turn: turns[1],
                index: 1,
            },
            config,
            None,
            Landing::Asked,
        )
        .expect("composed")
        .0;
    mode(0o700).unwrap();
    assert!(
        matches!(app.store.episode_len(&squashed), Ok(None)),
        "premise: the squash result's write failed, so it is not written"
    );
    app.dispatch_command_for_test("rename r-one");
    keeping_one(&mut app);
    app.dispatch_command_for_test("compact");
    distilling(&app);
    for msg in [
        StreamMsg::Chunk("second summary".into()),
        StreamMsg::Complete,
    ] {
        app.feed_summary(msg);
    }
    let compacted = app.buffer().conversation.episode().id;
    assert_ne!(compacted, squashed, "premise: compacted");
    assert!(
        matches!(app.store.episode_len(&squashed), Ok(Some(_))),
        "what it shares was written first"
    );

    let app = relaunched(dir.path());
    let (_, reloaded) = load_full_session(&*app.store, &compacted).expect("loads");
    assert_eq!(
        reloaded.len(),
        2,
        "summary and kept turn, whole after a relaunch"
    );
}

/// **One request at a time, per session**: `:compact` while a turn runs in
/// the session is refused, and starts nothing — two generations on one
/// session would have the summary land on a turn still writing (review of
/// 34033e8, Reviewers B and C: the guard was held by nothing). Per session
/// since sessions run apart; another session's turn does not refuse it
/// (`parallel.rs` holds that half). It argued from "two Global
/// generations", a scope gone with them (review stop 12, T5 F8).
#[tokio::test]
async fn compact_is_refused_while_a_turn_runs() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    app.fake_stream();
    app.notices.clear();
    app.dispatch_command_for_test("compact");
    // A refusal, not any notice (review stop 2, C 10).
    assert_eq!(
        app.notice_kind(),
        Some(NoticeKind::Refusal),
        "refused visibly: {:?}",
        app.notice()
    );
    assert!(app.distilling().is_none(), "and nothing distilling");
}

/// **A prompt typed while an unreviewed `:compact` runs is sent when it
/// lands** — queued behind the summary, and started when the stream arm
/// settles after the landing that composes it (review of 34033e8, Reviewer
/// C; the landing's own drain until every refill became the event's).
#[tokio::test]
async fn a_prompt_queued_behind_compact_is_sent_when_it_lands() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    app.dispatch_command_for_test("compact");
    assert!(!distilling(&app).3, "premise: no review, so it lands");
    distilling(&app);
    app.submit("after the summary");
    assert_eq!(app.pending.len(), 1, "premise: queued behind it");
    for msg in [
        StreamMsg::Chunk("what was done".into()),
        StreamMsg::Complete,
    ] {
        app.feed_summary(msg); // each message is its own event
    }
    assert!(app.pending.is_empty(), "the queued prompt went");
}

/// **A compaction is written at once, named or not** (Steven, 10-16,
/// decision (a)): its summary is text nothing else holds. It stayed lazy
/// unless a name was carried on — written on its first turn, so a quit, a
/// `:q!` or a crash before one lost a summary that was billed and maybe
/// edited in `$EDITOR`. (This test pinned the old rule, "an unnamed
/// replacement stays lazy"; the rule it pinned is the one decided against.)
#[tokio::test]
async fn a_compaction_is_written_at_once_named_or_not() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    app.dispatch_command_for_test("compact");
    distilling(&app);
    for msg in [
        StreamMsg::Chunk("what was done".into()),
        StreamMsg::Complete,
    ] {
        app.feed_summary(msg);
    }
    let now = app.buffer().conversation.episode().id;
    assert!(
        matches!(app.store.episode_len(&now), Ok(Some(1))),
        "on disk with its summary, before any turn: {:?}",
        app.store.episode_len(&now)
    );
}

/// **A replacement that cannot be written leaves the name where it was** —
/// on the original, on screen and on disk, the result unnamed, and the
/// refusal says so (review of 34033e8, Reviewers B and C: the rollback was
/// held by nothing).
#[tokio::test]
async fn a_carry_whose_write_fails_leaves_the_name_on_the_original() {
    use shannon_core::store::FailingStore;
    let (store, failing) = FailingStore::recoverable();
    failing.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    for prompt in ["one", "two", "three"] {
        app.conversation_mut().begin_turn(prompt);
        app.conversation_mut().push_chunk("r");
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }
    let original = app.conversation().episode().id;
    app.dispatch_command_for_test("rename auth-fix");
    app.dispatch_command_for_test("compact");
    distilling(&app);
    failing.store(true, std::sync::atomic::Ordering::SeqCst);
    app.notices.clear();
    for msg in [
        StreamMsg::Chunk("what was done".into()),
        StreamMsg::Complete,
    ] {
        app.feed_summary(msg);
    }
    let now = app.buffer().conversation.episode().id;
    assert_ne!(now, original, "premise: composed");
    assert_eq!(app.label_of(now).name, None, "the result takes no name");
    assert_eq!(
        app.label_of(original).name.as_deref(),
        Some("auth-fix"),
        "the name stays where it was"
    );
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("the name stays on"), "{said}");
}

/// **`:compact` on a Claude Code session asks the session Claude Code holds
/// for exactly the turns it replaces** — through `start_distill`, the door
/// `S` and `:compact` share: the request resumes the session filed after
/// the last turn it summarises, and closes with the instruction (review of
/// 34033e8, Reviewer C: the only test built its request by hand).
#[tokio::test]
async fn compact_on_claude_code_resumes_the_session_its_turns_were_filed_under() {
    let dir = TempDir::new().unwrap();
    let (mut app, provider) = provider_app();
    app.claude_program = logged_in_claude(dir.path());
    app.model_command("claude-code/sonnet");
    for (prompt, session) in [("first", "sess-1"), ("second", "sess-2")] {
        app.submit(prompt);
        app.feed_stream(StreamMsg::ProviderSession(session.into()));
        app.feed_stream(StreamMsg::Chunk("ok".into()));
        app.feed_stream(StreamMsg::Complete);
    }
    // Both turns summarised (`compact_keep = 0`, as `/compact`): the session
    // filed after the second. With the last kept it was the first's.
    app.dispatch_command_for_test("compact");
    let requests = provider.requests.lock().unwrap();
    let last = requests.last().expect("a request went");
    assert_eq!(
        last.resume.as_deref(),
        Some("sess-2"),
        "the session filed after the last turn it summarises"
    );
    assert_eq!(
        last.messages.last().map(|m| m.content.as_str()),
        Some(app.config.merge.squash_instruction.as_str()),
        "the instruction closes the request"
    );
}

/// **A compaction whose session is replaced under it says it was dropped** —
/// unreviewed, nothing else would: no review to open, no notice of arrival
/// (review of 34033e8, Reviewer A).
#[tokio::test]
async fn a_compaction_superseded_under_it_says_so() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    app.dispatch_command_for_test("compact");
    assert!(app.distilling().is_some(), "premise: distilling");
    app.notices.clear();
    // A composition lands OVER the buffer the summary would have — the
    // defensive ending, since a clone lands beside (10-21).
    app.supersede_for_test(app.current);
    assert!(app.distilling().is_none(), "premise: superseded");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("dropped"), "{said}");
}

/// **Nothing to compact is said** — a session with no turns, or one whose
/// turns are all kept — and so is a bare number, which was `:compact N`
/// (the turns to keep) until the words became instructions: read as an
/// instruction it would summarise everything, told "2".
#[tokio::test]
async fn compact_says_when_there_is_nothing_to_compact() {
    let mut app = test_app();
    app.notices.clear();
    app.dispatch_command_for_test("compact");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("nothing to compact — the session has no turns"),
        "{said}"
    );
    assert!(app.distilling().is_none(), "nothing asked");

    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    keeping_one(&mut app);
    app.notices.clear();
    app.dispatch_command_for_test("compact");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("the last 1 are kept (`[merge] compact_keep`)"),
        "{said}"
    );
    assert!(app.distilling().is_none(), "nothing asked");

    app.notices.clear();
    app.dispatch_command_for_test("compact 2");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("takes instructions") && said.contains("`:compact --keep 2`"),
        "the spelling that does what `:compact 2` did: {said}"
    );
    assert!(app.distilling().is_none(), "nothing asked");

    // Kept by the flag, the flag is named — it blamed the setting (review
    // stop 2, A F6 · B 4).
    app.dispatch_command_for_test("set merge.compact_keep=0");
    app.notices.clear();
    app.dispatch_command_for_test("compact --keep 1");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("the last 1 are kept (`--keep 1`)"), "{said}");
    assert!(app.distilling().is_none(), "nothing asked");
}

/// **After `--`, a number is words** (review stop 2, A F6 · B 4): `--` ends
/// the flags, so `:compact -- 3` is "3" for the summary — it was refused as
/// the old `:compact N`.
#[tokio::test]
async fn a_number_after_the_flags_is_an_instruction() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    app.dispatch_command_for_test("compact -- 3");
    assert!(
        instruction_recorded(&app).ends_with("Additional instructions: 3"),
        "{:?}",
        app.notice()
    );
}

/// **A distillation is billed to the session, and leaves its window alone**
/// — the summarizer's request (the history plus an instruction) is not what
/// the session's next request carries. A reviewed `:compact` whose summary
/// has arrived but not landed: the session is as it was, so is `ctx`.
#[tokio::test]
async fn a_distillation_bills_the_session_but_measures_nothing_of_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let window = shannon_core::Usage {
        input_tokens: 40_000,
        output_tokens: 1_000,
        ..Default::default()
    };
    app.buffer_mut()
        .billing
        .record(shannon_core::TurnCost::single_round(window));
    let before = app.buffer().billing;
    reviewing(&mut app);
    app.dispatch_command_for_test("compact");
    distilling(&app);
    app.feed_summary(StreamMsg::Chunk("what was done".into()));
    app.feed_summary(StreamMsg::Usage(shannon_core::Usage {
        input_tokens: 2_000,
        output_tokens: 300,
        ..Default::default()
    }));
    app.feed_summary(StreamMsg::Complete);
    assert!(
        app.decisions.review(app.current).is_some(),
        "premise: waiting for its review, not landed"
    );
    let after = app.buffer().billing;
    assert_eq!(after.context, before.context, "the window is the session's");
    assert!(!after.estimated);
    assert_eq!(
        after.total.output_tokens,
        before.total.output_tokens + 300,
        "and the summarizer's request is billed"
    );
}

/// **A compacted session's `ctx` is no longer the window it replaced** —
/// Steven, 10-15: after `:compact` the prompt row read the old 100%. The kept
/// turn is the SAME turn, its recorded window the whole pre-compaction
/// history; replayed after a summary it measures nothing that exists, so the
/// window is ESTIMATED (`~`) from this history's text.
///
/// Not from the old window's overhead: `window − text/4` over a large history
/// carries the chars-per-token error of all of it (a 600k window at 3.3 chars
/// a token "overhead" ~100k), so it would mislead most exactly here, where the
/// history just shrank. The text alone errs low by the system prompt and tool
/// definitions — bounded; `examples/ctx_estimate.rs` measures both.
///
/// Realistic proportions: each turn 4,000 characters (1,000 tokens at four a
/// token) and every request 20,000 tokens beyond its history (a system
/// prompt, tool definitions).
#[tokio::test]
async fn compaction_estimates_the_window_it_left() {
    const OVERHEAD: u32 = 20_000;
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    app.set_focus(PaneKind::Prompt);
    for k in 1..=3u32 {
        // The prompt carries the history so far and this ask; the reply's
        // 900 tokens complete the window.
        let round = shannon_core::Usage {
            input_tokens: OVERHEAD + k * 1_000 - 900,
            output_tokens: 900,
            ..Default::default()
        };
        let cost = shannon_core::TurnCost {
            whole: round,
            final_round: round,
        };
        app.conversation_mut().begin_turn("p".repeat(400));
        app.conversation_mut().push_chunk(&"r".repeat(3_600));
        app.conversation_mut().record_usage(cost);
        app.buffer_mut().billing.record(cost);
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }
    let before = app.buffer().billing.context;
    assert_eq!(
        before,
        OVERHEAD + 3_000,
        "premise: the last request's window"
    );

    keeping_one(&mut app);
    app.dispatch_command_for_test("compact");
    distilling(&app);
    for msg in [StreamMsg::Chunk("s".repeat(800)), StreamMsg::Complete] {
        app.feed_summary(msg);
    }
    let now = app.buffer().conversation.episode().id;
    assert_eq!(
        app.current_graph().full_turns(&now).len(),
        2,
        "premise: compacted to a summary and the kept turn"
    );
    let billing = app.buffer().billing;
    assert!(billing.estimated, "measured by no request: an estimate");
    // ~1,200 tokens of history: the summary and the kept turn.
    assert!(
        (1_000..=1_600).contains(&billing.context),
        "the history's text, not the old {before}: {}",
        billing.context
    );
    app.notices.clear(); // the compaction's own notice borrows the row
    let row = screen(&mut app, 120)
        .lines()
        .last()
        .unwrap_or_default()
        .to_string();
    assert!(row.contains("ctx ~"), "said to be an estimate: {row}");
}

/// **`!` does the other for once, whichever `[merge] compact_review` says**
/// (Steven, 10-15: "a setting that lets you auto accept the compaction
/// result"; off by default since 10-16, as `/compact` asks nothing).
#[tokio::test]
async fn compact_review_and_bang_do_the_other_for_once() {
    let unread = "it replaces them when it is written";
    let read = "its review opens beside the session when it is written";
    for (setting, keys, review, said) in [
        ("false", "compact", false, unread),
        ("false", "compact!", true, read),
        ("true", "compact", true, read),
        ("true", "compact!", false, unread),
    ] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 4);
        app.dispatch_command_for_test(&format!("set merge.compact_review={setting}"));
        assert_eq!(
            app.config.merge.compact_review.to_string(),
            setting,
            "premise: {:?}",
            app.notice()
        );
        app.notices.clear();
        app.dispatch_command_for_test(keys);
        assert_eq!(distilling(&app).3, review, "{setting}, `:{keys}`");
        let notice = app.notice().unwrap_or_default();
        assert!(notice.contains(said), "{setting}, `:{keys}`: {notice}");
    }
}

/// **A review arriving in a session no window shows says what brings it** —
/// `:decide N`, not "the pane beside it": none opens there, and nothing else
/// on screen points to it (§4: "the notice says where the review will
/// wait").
#[tokio::test]
async fn a_review_in_no_window_says_what_brings_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 4);
    let chat = app.current;
    app.dispatch_command_for_test("compact!");
    distilling(&app);
    app.dispatch_command_for_test("new");
    assert!(
        app.panes_with_session(chat, Over::ThisTab).next().is_none(),
        "premise: no window shows it: {:?}",
        app.notice()
    );
    for msg in [
        StreamMsg::Chunk("what was done".into()),
        StreamMsg::Complete,
    ] {
        app.feed_summary(msg);
    }
    app.settle();
    assert!(app.decisions.review(chat).is_some(), "premise: it waits");
    assert_eq!(app.decision_pane_of(chat), None, "with no pane");
    let said: Vec<&str> = (app.notices.iter()).map(|(_, s)| s.as_str()).collect();
    let reach = format!("its review waits — `:decide {}`", chat.number());
    assert!(said.iter().any(|s| s.contains(&reach)), "{said:?}");
}

/// **A compaction where you are becomes `HEAD`** — a relaunch opens the
/// compacted session, as `claude --continue` does after `/compact` (Steven,
/// 10-16). Before compositions were written at once, `HEAD` stayed on the
/// original until the compacted session's first turn.
#[tokio::test]
async fn a_compaction_where_you_are_becomes_head() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    app.dispatch_command_for_test("compact");
    distilling(&app);
    for msg in [
        StreamMsg::Chunk("what was done".into()),
        StreamMsg::Complete,
    ] {
        app.feed_summary(msg);
    }
    let now = app.buffer().conversation.episode().id;
    assert_eq!(app.store.get_ref("HEAD").unwrap(), Some(now));
}

/// **A summary that lands in a session you have left moves `HEAD` only when
/// it replaces the session `HEAD` names** (Steven, 10-16: "Only if it lands
/// in front"; the replacement clause is a rebase moving its branch ref — a
/// launch must not reopen an original that handed its name and work on).
/// One standing beside its source — `S` gathering turns — leaves `HEAD`
/// where it was. Named and unnamed alike: a carried name writes through the
/// same door (`write_made`).
#[tokio::test]
async fn a_summary_landing_elsewhere_moves_head_only_over_what_it_replaces() {
    for (replaces, named) in [(false, false), (true, false), (true, true)] {
        let dir = TempDir::new().unwrap();
        let (mut app, a, b, left, right) = app_with_two_sessions(dir.path());
        app.focus_pane(right);
        app.open_by_hash(&b.to_hex()[..8]);
        app.focus_pane(left);
        app.open_by_hash(&a.to_hex()[..8]);
        let a_buffer = app.current;
        if named {
            app.dispatch_command_for_test("rename auth-fix");
        }
        app.focus_pane(right); // aimed at B now; HEAD stays A
        assert_eq!(
            app.store.get_ref("HEAD").unwrap(),
            Some(a),
            "premise: HEAD is A"
        );
        assert_ne!(app.prompt_target(), a_buffer, "premise: you have left A");

        let made = summary_lands(&mut app, a_buffer, a, replaces);
        let head = app.store.get_ref("HEAD").unwrap();
        let case = format!("replaces={replaces}, named={named}");
        match replaces {
            true => assert_eq!(head, Some(made), "HEAD follows the replacement: {case}"),
            false => assert_eq!(head, Some(a), "HEAD stays where it was: {case}"),
        }
    }
}

/// A summary of all of `a` lands in `a_buffer` — replacing `a`, or standing
/// beside it — and is written at once: the made session.
fn summary_lands(app: &mut App, a_buffer: BufferId, a: EpisodeId, replaces: bool) -> EpisodeId {
    let graph = app.current_graph();
    let config = graph.episode(&a).unwrap().config.clone();
    let turns = graph.full_turns(&a);
    let summary = Turn::seal(
        vec![Message::user("S"), Message::assistant("what was done")],
        config.clone(),
        std::time::SystemTime::UNIX_EPOCH,
    );
    // Its marks where they were: this test asks of names, not marks.
    let same: &dyn Fn(usize) -> Option<usize> = &Some;
    let made = app
        .adopt_composed(
            a_buffer,
            graph,
            &Squash {
                episode: a,
                start: 0,
                end: turns.len(),
                summary: vec![summary],
            },
            Anchor {
                base: a,
                turn: turns[0],
                index: 0,
            },
            config,
            replaces.then_some((a, same)),
            Landing::Asked,
        )
        .expect("composed")
        .0;
    assert!(
        matches!(app.store.episode_len(&made), Ok(Some(_))),
        "premise: written at once"
    );
    made
}

/// **Each half of `HEAD`'s rule, alone** (review stop 2, A F2 · C 1: both
/// halves could be deleted with the suite green — every case before this
/// had `HEAD` naming the source, so either half passed it). Where you are,
/// replacing nothing, with `HEAD` elsewhere: only "where you are" moves it.
/// Elsewhere, replacing A, with `HEAD` naming B: "replaces the session
/// `HEAD` names" does not hold, so it stays — a summary of A landing late
/// must not reopen A′ over the B you `:open`ed since.
#[tokio::test]
async fn each_half_of_heads_rule_moves_it_alone() {
    for here in [true, false] {
        let dir = TempDir::new().unwrap();
        let (mut app, a, b, left, right) = app_with_two_sessions(dir.path());
        app.focus_pane(right);
        app.open_by_hash(&b.to_hex()[..8]);
        app.focus_pane(left);
        app.open_by_hash(&a.to_hex()[..8]);
        let a_buffer = app.current;
        if !here {
            app.focus_pane(right);
        }
        app.store.put_ref("HEAD", &b).unwrap();
        assert_eq!(
            app.prompt_target() == a_buffer,
            here,
            "premise: aimed at A iff here"
        );

        let made = summary_lands(&mut app, a_buffer, a, !here);
        let head = app.store.get_ref("HEAD").unwrap();
        match here {
            true => assert_eq!(head, Some(made), "where you are, it moves"),
            false => assert_eq!(head, Some(b), "over A while HEAD names B, it stays"),
        }
    }
}
