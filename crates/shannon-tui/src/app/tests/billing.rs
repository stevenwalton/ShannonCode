//! What a turn costs and is billed: `:model` per session, usage, the request on the wire.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// ---- :model — per session, so two panes can run two models ----------------

/// `:model <name>` sets the AIMED session's model, resolves a short name, and
/// leaves its sibling alone. Per session, not per app: two panes on two models
/// answering one prompt is the point.
#[test]
fn model_is_set_per_session_and_resolves_a_short_name() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let first = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    app.new_session_command(); // the split pane gets its own session
    let second = app.workspace.focused();
    let before = app
        .pane_session(first)
        .conversation
        .episode()
        .config
        .model
        .clone();

    app.model_command("opus");

    assert_eq!(
        app.pane_session(second).conversation.episode().config.model,
        shannon_core::backend::models::resolve("opus"),
        "the aimed session moved to the resolved id, not the word `opus`"
    );
    assert_eq!(
        app.pane_session(first).conversation.episode().config.model,
        before,
        "and its sibling kept the model it had"
    );
}

/// An unknown name passes through, so a model this build has never heard of is
/// still reachable — the table is a convenience, not a gate on what exists.
#[test]
fn an_unknown_model_name_is_accepted_verbatim() {
    let mut app = test_app();
    app.model_command("claude-from-the-future-9");
    assert_eq!(
        app.conversation().episode().config.model,
        "claude-from-the-future-9"
    );
}

/// Switching models mid-turn is refused: the turn was SENT under the old one,
/// and sealing it under a different model would misattribute what answered.
///
/// **And it names the right thing.** `:model` phrased its own refusal ("a turn
/// is running here"), which is simply false during a distillation or a
/// continuation — neither is a turn. Eight sites invented a sentence like that
/// and between them covered different subsets of the four kinds of work
/// (Reviewer D); they all consult `busy_reason` now.
///
/// The previous version of this test asserted `contains("running")`, which the
/// old wording satisfied and so did several wrong ones — the substring shape
/// that looks most like coverage (`[[feedback_mutation_check_guards]]`).
#[tokio::test]
async fn model_refuses_while_something_holds_the_session_and_names_it() {
    let mut app = test_app();
    app.submit("hello");
    assert!(app.streaming(), "premise: a turn is in flight");
    let before = app.conversation().episode().config.model.clone();

    app.model_command("opus");

    let notice = app.notice().expect("a visible refusal").to_string();
    assert!(
        notice.contains("a turn is in flight"),
        "names what holds it: {notice}"
    );
    assert!(
        notice.contains("CTRL-C"),
        "and how to get out of it: {notice}"
    );
    assert_eq!(app.conversation().episode().config.model, before);

    // The case the hand-written wording got wrong: a distillation is not a
    // turn, and saying "a turn is running here" about one is a lie in a
    // refusal — the one place the user is already confused.
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

    app.model_command("opus");

    let notice = app.notice().expect("a visible refusal").to_string();
    assert!(
        notice.contains("distilling"),
        "a distillation is named as one: {notice}"
    );
    assert!(
        !notice.contains("a turn is"),
        "and NOT described as a turn, which is what the private wording said: \
         {notice}"
    );
}

/// The REQUEST carries the session's model — the wiring, not just the record.
///
/// The episode has recorded a model since `TurnConfig` existed and
/// `send_foreground` never read it, which was invisible while every session ran
/// the backend default. Without this test `:model` would set a field nobody
/// sends.
#[tokio::test]
async fn the_request_carries_the_sessions_model() {
    let backend = std::sync::Arc::new(shannon_core::backend::RecordingStub::new());
    let mut app = test_app_backed(backend.clone(), Arc::new(Config::default()));
    app.executor = Arc::new(shannon_core::RecordingExecutor::default());
    app.model_command("opus");

    app.submit("hello");

    assert_eq!(
        backend.last_model(),
        Some(shannon_core::backend::models::resolve("opus")),
        "the turn went out under the model the session was set to"
    );
}

/// `[tui] model` decides what the FIRST session starts on.
///
/// It didn't: construction read `backend.model()` directly, so the launch
/// session — the one everybody uses, since `resume = fresh` is the default —
/// ignored the config knob entirely. (Reviewer C, HIGH.)
#[test]
fn the_config_model_seeds_the_launch_session() {
    let mut cfg = Config::default();
    cfg.tui.model = "opus".into();
    let app = test_app_with(cfg);
    assert_eq!(
        app.conversation().episode().config.model,
        shannon_core::backend::models::resolve("opus"),
        "the session we launch into is on the configured model"
    );
}

/// **A turn that took a tool round-trip is billed for BOTH calls.**
///
/// `Usage::merge` is a per-field max, and its doc is right about why — `input`
/// and `output` arrive on different events and each is cumulative *within one
/// request*. A tool round is a **different request**: `resume_after_tools`
/// re-sends under the same `GenerationId`, and the new response's
/// `message_start` restarts `input_tokens` from zero, re-counting the whole
/// (now larger) prompt. Maxing the two therefore discarded round 1's input and
/// round 2's smaller output.
///
/// Reviewer A executed it: two rounds that really cost 2200/510 displayed as
/// 1200/500. Agentic turns are exactly the expensive ones, and the error grows
/// monotonically with round count — so the number on screen drifts further
/// from the invoice the harder the turn worked.
#[tokio::test]
async fn a_turn_with_a_tool_round_is_billed_for_both_calls() {
    let mut app = test_app();
    app.submit("do a thing");

    // Round 1: the model asks for a command, having spent 1000/500.
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 1000,
        output_tokens: 500,
        ..Default::default()
    }));
    // `ls -la` is read-only, so it auto-approves and the round resumes without
    // a decision — this test is about arithmetic, not about the gate.
    model_call(&mut app, "tu-1", "ls -la");
    settle(&mut app).await;
    assert!(
        app.streaming(),
        "premise: the tool result went back and round 2 is under way"
    );

    // Round 2: a bigger prompt (the transcript grew) and a short reply.
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 1200,
        output_tokens: 10,
        ..Default::default()
    }));
    app.feed_stream(StreamMsg::Complete);

    assert_eq!(
        app.buffer().billing.total,
        Usage {
            input_tokens: 2200,
            output_tokens: 510,
            ..Default::default()
        },
        "both requests were billed, so both are counted — a max would show \
         1200/500 and silently drop round 1's input and round 2's output"
    );

    // **…and the CONTEXT is the last round alone.** Round 2's 1200-token
    // prompt already contains round 1, so adding them counts the shared prefix
    // twice: this read 2710 and the honest figure is 1210 (Reviewer A,
    // reproduced). The bill and the window are different intervals over the
    // same numbers, and `resolve` has to hand `Billing::record` both — a unit
    // test on `record` alone leaves the WIRING untested, which is how the
    // sibling bug in `resume_after_tools` stayed green (Reviewer C).
    assert_eq!(
        app.buffer().billing.context,
        1210,
        "the window holds one conversation: round 2's prompt plus its reply, \
         not the sum of every round"
    );
}

/// **Every request that offers a tool also explains it.**
///
/// The `bash` tool went out with no system prompt at all, so the model was
/// never told it was attached to a terminal on somebody's machine, which
/// directory, or that looking at the machine means running a command rather
/// than recalling one. A bare tool with no framing is a tool most models
/// decline in favour of answering from memory, which is what the demo saw.
///
/// Asserted at `tool_request`, the one door both the foreground and a job go
/// through, so a third caller cannot offer a tool without the framing for it.
#[test]
fn a_request_that_offers_tools_carries_the_system_prompt() {
    let app = test_app();
    for unattended in [false, true] {
        let req = app.tool_request(
            vec![Message::user("hi")],
            &app.sealing_for("m".into(), None),
            unattended,
        );
        assert!(!req.tools.is_empty(), "premise: tools are offered");
        let system = req
            .system
            .unwrap_or_else(|| panic!("unattended={unattended}: no system prompt"));
        assert!(system.contains("bash"), "{system}");
    }
}

/// **A turn billed for output it never showed says so.**
///
/// The demo's central mystery: four turns billed a full 4,096 output tokens and
/// rendered nothing. Whatever filled them was not text, and the two things it
/// could have been — a `tool_use` cut off mid-arguments, or thinking the request
/// never asked to see — were both discarded in silence. The user watched empty
/// bubbles arrive and concluded the program had frozen.
///
/// This does not diagnose which; it reports the discrepancy, which is the part
/// we can always know.
#[test]
fn output_tokens_with_nothing_to_show_are_reported() {
    let billed = |out: u32, chars: usize| {
        crate::app::unaccounted_output(
            Usage {
                output_tokens: out,
                ..Usage::default()
            },
            chars,
        )
    };

    // The incident shape: a full cap, no text.
    let note = billed(4096, 0).expect("4096 tokens for nothing must be reported");
    assert!(note.contains("4096"), "{note}");

    // An ordinary reply is silent — roughly four characters a token, and the
    // margin is wide on purpose so this polices gross mismatch, not rounding.
    assert_eq!(billed(1000, 4000), None, "a normal reply says nothing");
    assert_eq!(billed(1000, 2200), None, "…and a terse one still does not");

    // A short turn is below the floor either way: its overhead is a bigger
    // share than any real discrepancy, so the arithmetic would be noise.
    assert_eq!(billed(50, 0), None, "a short turn is not evidence");

    // But a long one that showed a token's worth of text is.
    assert!(billed(3000, 40).is_some(), "a gross mismatch is reported");
}

/// …and it reaches the SCREEN, which is a different claim from computing it.
/// A function that returns the right string and a caller that drops it is
/// exactly the shape of the three silences that made the demo undiagnosable
/// ([[feedback_no_silent_failure]]).
#[tokio::test]
async fn the_unaccounted_output_report_reaches_the_user() {
    let mut app = test_app();
    app.submit("please implement it");
    app.feed_stream(StreamMsg::Usage(Usage {
        output_tokens: 4096,
        ..Usage::default()
    }));
    // No chunks at all — the incident shape exactly.
    app.feed_stream(StreamMsg::Complete);

    let notice = app.notice().unwrap_or_default().to_string();
    assert!(
        notice.contains("4096"),
        "an empty reply that billed 4096 tokens must say so: {notice:?}"
    );
}

/// **Every `[section] key` we tell a user to change must exist.**
///
/// Found in this batch, in the fix for the incident it belongs to: the
/// truncation notice and `BackendError::Truncated` both told the user to
/// "raise `[turn] max_output_tokens`", and that key did not exist. Telling
/// someone to do something impossible is a worse failure than saying nothing,
/// and it is the same class as the messages this batch was written to fix
/// ([[project_shannon_message_honesty]], [[feedback_no_silent_failure]]).
///
/// Checked against `DEFAULT_TOML`, which a separate test already holds equal to
/// the coded defaults — so this cannot pass by agreeing with a second stale
/// list. A message naming a key is a CLAIM about the config, and this is the
/// count.
#[test]
fn a_message_never_names_a_config_key_that_does_not_exist() {
    use shannon_core::config::DEFAULT_TOML;

    // The strings a user can actually be shown. Add to this list when a new
    // message starts naming a knob.
    let mut messages: Vec<String> = Vec::new();
    for reason in [
        shannon_core::StopReason::MaxTokens,
        shannon_core::StopReason::ModelContextWindowExceeded,
        shannon_core::StopReason::Refusal,
        shannon_core::StopReason::PauseTurn,
    ] {
        // Both phrasings: the recovery differs for the foreground turn and for
        // a job, and each is a string a user reads.
        for here in [true, false] {
            if let Some(m) = crate::app::truncation_notice(&reason, here) {
                messages.push(m);
            }
        }
    }
    messages.push(
        shannon_core::BackendError::Truncated {
            tool: "bash".into(),
            bytes: 12,
        }
        .to_string(),
    );
    messages.push(
        crate::app::unaccounted_output(
            Usage {
                output_tokens: 4096,
                ..Usage::default()
            },
            0,
        )
        .unwrap_or_default(),
    );

    // Does `DEFAULT_TOML` declare `[section] key`? A textual scan rather than a
    // parse, so this test needs no TOML dependency of its own — and it is sound
    // because a separate test already holds `DEFAULT_TOML` equal to the coded
    // defaults, field for field. The reference cannot be stale without that
    // test failing first.
    //
    // **Known limit, on purpose.** Some keys appear in `DEFAULT_TOML` only as
    // commented examples (`# extra = [...]`), and those are NOT accepted here.
    // A message naming one would fail this test — which is the right direction
    // to be wrong in: the fix is to un-comment the key or reword the message,
    // and both are better than a scan lenient enough to accept a typo. A
    // commented line is also unchecked by the completeness test, so treating it
    // as a declaration would import an unverified claim.
    let declares = |section: &str, key: &str| -> bool {
        let mut in_section = false;
        for line in DEFAULT_TOML.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_section = line == format!("[{section}]");
                continue;
            }
            if in_section && line.starts_with(key) {
                let rest = line[key.len()..].trim_start();
                if rest.starts_with('=') {
                    return true;
                }
            }
        }
        false
    };
    // `[section] key` — the shape every one of these messages uses. Hand-rolled
    // rather than pulling a regex crate in for one test: the shape is fixed and
    // a scanner that is wrong is louder than one that is clever.
    let named = |text: &str| -> Vec<(String, String)> {
        let mut out = Vec::new();
        let bytes: Vec<char> = text.chars().collect();
        let word = |from: usize| -> (String, usize) {
            let mut i = from;
            let mut w = String::new();
            while i < bytes.len() && (bytes[i].is_ascii_lowercase() || bytes[i] == '_') {
                w.push(bytes[i]);
                i += 1;
            }
            (w, i)
        };
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == '[' {
                let (section, mut j) = word(i + 1);
                if !section.is_empty() && j < bytes.len() && bytes[j] == ']' {
                    j += 1;
                    while j < bytes.len() && bytes[j].is_whitespace() {
                        j += 1;
                    }
                    let (key, _) = word(j);
                    if !key.is_empty() {
                        out.push((section, key));
                    }
                }
            }
            i += 1;
        }
        out
    };

    let mut checked = 0usize;
    for message in &messages {
        for (section, key) in named(message) {
            let found = declares(&section, &key);
            assert!(
                found,
                "a message names `[{section}] {key}`, which is not in the config:\n  {message}"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "premise: at least one message names a knob — otherwise this measures nothing"
    );
}

/// **The configured ceiling reaches the request, and `0` means "the model's".**
///
/// The knob exists because a message told the user to raise it. A knob a
/// request never reads would make that message true and still useless — the
/// same failure one layer down.
#[test]
fn the_configured_reply_ceiling_reaches_the_request() {
    let mut app = test_app();
    let msgs = || vec![Message::user("hi")];

    assert_eq!(
        app.tool_request(msgs(), &app.sealing_for("m".into(), None), false)
            .max_tokens,
        None,
        "the default (0) leaves the ceiling to the model, not to us"
    );

    let mut cfg = (*app.config).clone();
    cfg.turn.max_output_tokens = 8_000;
    app.config = std::sync::Arc::new(cfg);
    assert_eq!(
        app.tool_request(msgs(), &app.sealing_for("m".into(), None), false)
            .max_tokens,
        Some(8_000),
        "a configured bound is the user's, and it must actually be sent"
    );
}

/// **A turn is not accused of losing output it actually showed.**
///
/// `unaccounted_output` compares what a turn was billed against what it showed.
/// The bill accumulates across tool rounds; the draft did not, because
/// `finish_tool_round` takes the assistant text each round. So the two sides
/// covered different intervals and the alarm fired on every ordinary
/// multi-round tool turn — the successful agentic path, told it "may have been
/// cut off" (Reviewer A, reproduced end to end). A warning that fires on
/// success is a warning nobody reads.
#[test]
fn a_multi_round_tool_turn_is_not_accused_of_losing_output() {
    let mut c = shannon_core::Conversation::new();
    c.begin_turn("do the thing");
    c.push_chunk("I will look at that for you, one moment.");
    let round_one = c.draft_len().expect("a turn is in progress");
    assert!(round_one > 0);

    // The round ends with a tool call and its result — the text is taken.
    c.push_tool_use(shannon_core::Block::bash_call("tu_1", "ls"));
    c.finish_tool_round(shannon_core::Message::tool_result("tu_1", "a b c", false));
    assert_eq!(
        c.draft_len(),
        Some(round_one),
        "what round one showed is still counted after the round ends"
    );

    c.push_chunk("There are three files.");
    let total = c.draft_len().expect("still in progress");
    assert!(
        total > round_one,
        "round two adds to the turn's total rather than replacing it"
    );
}

/// …and the characters are CHARACTERS. `len()` runs two to four times higher
/// on CJK or emoji, which biases the comparison and makes the doc a lie.
#[test]
fn shown_text_is_counted_in_characters() {
    let mut c = shannon_core::Conversation::new();
    c.begin_turn("hi");
    c.push_chunk("世界🙂");
    assert_eq!(c.draft_len(), Some(3), "three characters, not eleven bytes");
}

/// A draft larger than a `u32` of tokens must not panic the report that
/// measures it. `explained * 4` panicked in debug and wrapped in release at
/// ~2.1 billion characters (Reviewer B, reproduced). Unreachable while
/// `max_output_tokens` holds — which is exactly the kind of "unreachable" this
/// file has already been wrong about once today.
#[test]
fn the_unaccounted_report_survives_an_enormous_draft() {
    let huge = crate::app::unaccounted_output(
        Usage {
            output_tokens: 300,
            ..Usage::default()
        },
        4_000_000_000,
    );
    assert_eq!(huge, None, "a draft that large plainly explains 300 tokens");
    // …and the other extreme still reports.
    assert!(
        crate::app::unaccounted_output(
            Usage {
                output_tokens: u32::MAX,
                ..Usage::default()
            },
            0,
        )
        .is_some()
    );
}

/// **The context budget is the user's when they set one, and the model's
/// otherwise.**
///
/// Steven's ask: a long context costs more on every later turn and gets less
/// reliable well before it gets full, so a bound is both a spend control and a
/// quality control. The model's window is a hard limit, not a recommendation,
/// and 1M is a lot of rope.
#[test]
fn a_configured_context_budget_replaces_the_models_own() {
    let mut app = test_app();
    let buffer = app.buffer().id;

    // Unset: the model's own window. The test app is on the stub, so assert
    // against what the table says for the model the session actually names —
    // not a number typed here, which would just be the table twice.
    let model = app.buffer().conversation.episode().config.model.clone();
    assert_eq!(
        app.context_budget(buffer),
        shannon_core::backend::models::context_window(&model)
    );
    assert!(!app.config.turn.budget_is_yours());

    let mut cfg = (*app.config).clone();
    cfg.turn.max_context_tokens = 50_000;
    app.config = std::sync::Arc::new(cfg);
    assert_eq!(app.context_budget(buffer), 50_000, "the user's bound wins");
    assert!(
        app.config.turn.budget_is_yours(),
        "…and the gauge is told it is a choice, so it can say so"
    );
}

/// The budget follows the SESSION's model, not the backend's — `:model opus`
/// is a per-session lever, and a gauge measuring the wrong model's window is a
/// wrong number on screen.
#[test]
fn the_context_budget_follows_the_sessions_model() {
    // A live backend, so the session's model IS the answering one. On a
    // stub-only process it is not, and the second half below is the point.
    let mut app = test_app_backed(
        std::sync::Arc::new(shannon_core::backend::HangingBackend::new()),
        Arc::new(Config::default()),
    );
    let buffer = app.buffer().id;
    app.buffer_mut().conversation.set_model("claude-opus-5");
    assert_eq!(app.context_budget(buffer), 1_000_000);
    app.buffer_mut()
        .conversation
        .set_model("claude-haiku-4-5-20251001");
    assert_eq!(app.context_budget(buffer), 200_000);
}

/// **"How full is the window" is a question about the model that will answer.**
///
/// With no API key the stub answers every model, so a session set to Opus is
/// not getting Opus's million-token window — and a budget bar drawn against a
/// window the request will never have is the same class of lie as a status row
/// naming a model that is not answering. Falls out of routing `session_model`
/// through `answering_model`; asserted so it is a decision rather than a
/// side effect.
#[test]
fn a_stub_only_process_budgets_against_the_stub() {
    let mut app = test_app();
    assert!(app.only_stub(), "premise: nothing but the stub here");
    let buffer = app.buffer().id;
    app.buffer_mut().conversation.set_model("claude-opus-5");
    assert_eq!(
        app.context_budget(buffer),
        200_000,
        "the stub's window, not the one that was asked for"
    );
    assert_eq!(
        app.buffer().conversation.episode().config.model,
        "claude-opus-5",
        "and the PREFERENCE is untouched — a later run with a key honours it"
    );
}

/// **A resumed turn's context is the last round, not the sum — on disk too.**
///
/// The live half of this was fixed one commit earlier: `Billing::record` takes
/// the whole turn for the bill and the final request for the window, because
/// round 2's prompt already contains round 1 and adding them counts the shared
/// prefix twice. `render_session` then handed the SAME value to both, under a
/// comment asserting that on replay the two intervals coincide — true of a
/// single-round turn, which is nearly all of them, and false of exactly the
/// agentic ones the gauge matters for. Reopening a session put the 2.24×
/// figure straight back on the status line
/// (`[[feedback_fix_neighbourhood]]`, `[[feedback_doc_comments_are_claims]]`).
///
/// The fix is a second field on the turn, because the sum cannot answer for
/// the final round: 2200/510 does not contain 1200. So this asserts the two
/// paths agree, which is the property — not a number the replay path could
/// satisfy on its own.
#[tokio::test]
async fn a_replayed_tool_round_reports_the_same_context_as_the_live_one() {
    let mut app = test_app();
    app.submit("do a thing");
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 1000,
        output_tokens: 500,
        ..Default::default()
    }));
    // Read-only, so it auto-approves and round 2 starts without a decision.
    model_call(&mut app, "tu-1", "ls -la");
    settle(&mut app).await;
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 1200,
        output_tokens: 10,
        ..Default::default()
    }));
    app.feed_stream(StreamMsg::Complete);

    let live = app.buffer().billing.context;
    assert_eq!(
        live, 1210,
        "premise: the live path already reads the window"
    );

    // `clone_by_hash` re-renders from the STORED turns — the replay path.
    // `:open` would find the still-live buffer instead, because buffers are
    // never closed, so it cannot exercise this.
    let episode = app.conversation().episode().id;
    app.clone_by_hash(&episode.to_hex()[..8]);
    assert_eq!(
        app.buffer().billing.context,
        live,
        "reopening a session must not change how full it says it is; this read          2710 (the sum of both rounds' prompts) while the live session read 1210"
    );

    // And the BILL is still the sum — the two intervals stay distinct across
    // the round trip rather than being reconciled by making them equal.
    assert_eq!(
        app.buffer().billing.total,
        Usage {
            input_tokens: 2200,
            output_tokens: 510,
            ..Default::default()
        },
        "both requests were billed, and replay carries that too"
    );
}

/// **A session keeps the model you set for it, through every door back into
/// it — and a session that never named one still gets a default.**
///
/// `adopt_session` and `open_buffer_for_episode` both ended with
/// `set_model(self.default_model())` under the comment "new turns under the
/// live model". The episode in hand names its own model, so reading "live" as
/// *the app's* meant `:open`, `:clone`, a `:graph` checkout, `:reply` and every
/// composition quietly put the session back on `[tui] model`. Set `:model
/// opus`, check out one of that session's turns, and you were on the default
/// with nothing on screen to say so.
///
/// Both halves are asserted because the overwrite had a real job: an episode
/// stored before models were recorded carries a placeholder, and dropping the
/// line entirely would have left those on `"unspecified"` — a fix that trades
/// one silent wrong model for another (`[[feedback_fix_neighbourhood]]`).
#[tokio::test]
async fn reopening_a_session_keeps_the_model_it_was_set_to() {
    const MODEL: &str = "claude-opus-5";
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    app.model_command(MODEL);
    let episode = app.conversation().episode().id;
    app.conversation_mut().begin_turn("under the new model");
    app.conversation_mut().push_chunk("ok");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();

    // Door 1: a `:graph` checkout, which mints a branch and adopts it.
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(episode, t1, 1);
    assert_eq!(
        app.conversation().episode().config.model,
        MODEL,
        "a branch continues the session it forked, on the model that session \
         was set to"
    );

    // Door 2: opening the original again in another buffer.
    app.reset_foreground();
    app.open_command(&episode.to_hex()[..8]);
    assert_eq!(
        app.conversation().episode().config.model,
        MODEL,
        "`:open` returns you to the session you left, not to the app default"
    );

    // Door 3: **a relaunch** — the one that matters most, and the one memory
    // alone could never survive. `set_model` wrote to the in-process
    // conversation and nowhere else, so the header on disk still named the
    // model the episode was created under.
    let mut cfg = Config::default();
    cfg.tui.resume = shannon_core::ResumeMode::Last;
    let relaunched = test_app_with_store(
        test_backend(),
        Arc::new(cfg),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    assert_eq!(
        relaunched.conversation().episode().id,
        episode,
        "premise: we really resumed that session"
    );
    assert_eq!(
        relaunched.conversation().episode().config.model,
        MODEL,
        "`:model` outlives the process — otherwise the choice quietly expires \
         when you close the terminal"
    );

    // And the half the overwrite existed for: a session with no model recorded
    // is given the live one rather than left on the placeholder.
    let legacy = shannon_core::graph::Episode::root(
        fresh_episode_id(),
        shannon_core::graph::TurnConfig {
            model: shannon_core::conversation::UNSPECIFIED_MODEL.into(),
            max_tokens: None,
            effort: None,
        },
    );
    let buffer = app.buffer().id;
    let recall = app.unlock_turns(&[]).expect("nothing to unlock");
    app.adopt_session(
        buffer,
        legacy,
        Vec::new(),
        OnDisk::Absent,
        recall,
        Landing::Asked,
    );
    assert_eq!(
        app.conversation().episode().config.model,
        app.default_model(),
        "an episode that never named a model is filled in, which is what the \
         unconditional overwrite was really for"
    );
}

/// **The incident shape says what to DO, not what went wrong.**
///
/// `max_tokens` with almost no text is exactly the turn that broke the demo:
/// billed 4,096, rendered nothing. Two notices fired for it — the truncation
/// one when the stream said why it stopped, the accounting one a beat later at
/// commit — and `notify` is last-write-wins, so the user was left holding
/// *"billed 4096 output tokens and showed almost none of it"* and never
/// *"`:continue` resumes it"*. The diagnostic replaced the answer.
///
/// They were never two messages: `unaccounted_output` **guesses** ("may have
/// been cut off… or spent on thinking") at the thing the stop reason **states**.
/// One composer, and the fact wins over the hypothesis.
#[tokio::test]
async fn a_truncated_turn_is_told_how_to_resume_not_how_it_failed() {
    use shannon_core::StopReason;
    let mut app = test_app();
    app.submit("write something long");
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 100,
        output_tokens: 4096,
        ..Default::default()
    }));
    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);

    let notice = app.notice().expect("a cut-off turn must say so");
    assert!(
        notice.contains(":continue"),
        "the recovery is what the user needs, and it is what the accounting \
         line used to overwrite: {notice:?}"
    );
    assert!(
        !notice.contains("showed almost none"),
        "the accounting line is a hypothesis about the reason we already know, \
         so it must not be the sentence on screen: {notice:?}"
    );

    // …and with no reason to explain it, the accounting line is exactly what
    // should show. Otherwise this test would pass by the check being dead.
    let mut app = test_app();
    app.submit("write something long");
    app.feed_stream(StreamMsg::Usage(Usage {
        input_tokens: 100,
        output_tokens: 4096,
        ..Default::default()
    }));
    app.feed_stream(StreamMsg::Complete);
    let notice = app.notice().unwrap_or_default();
    assert!(
        notice.contains("showed almost none"),
        "a turn billed for output nobody saw, with nothing to explain it, is \
         still worth saying: {notice:?}"
    );
}

/// **The alarm's threshold is a number, so pin the number.**
///
/// `unaccounted_output` decides with a ratio that was spread across two
/// constants — "characters per token, halved", and a bare `* 4` at the
/// comparison — which multiplied to the real rule while naming neither.
/// Reviewer C's finding: the bare factor could be anything in `[1, ~149]` and
/// the suite stayed green, because the tests exercised the *branch* (it fires /
/// it does not) and never its edge. A guard whose threshold no test can see is
/// a guard nobody can change safely (`[[feedback_mutation_check_guards]]`).
///
/// So this walks the edge from both sides. It is deliberately arithmetic rather
/// than a round number: the assertion is *the rule*, not a value copied out of
/// the implementation, so changing the rule fails here rather than being
/// rubber-stamped.
#[test]
fn the_unaccounted_alarm_fires_exactly_at_its_threshold() {
    let billed = |n: u32| Usage {
        output_tokens: n,
        ..Usage::default()
    };
    // A whole reply is ~4 chars a token; the alarm wants an eighth of that.
    let threshold = |tokens: u32| (u64::from(tokens) * 4).div_ceil(8) as usize;

    const N: u32 = 4_096;
    let edge = threshold(N);
    assert!(
        crate::app::unaccounted_output(billed(N), edge).is_none(),
        "exactly at the threshold is quiet — {edge} chars for {N} tokens"
    );
    assert!(
        crate::app::unaccounted_output(billed(N), edge - 1).is_some(),
        "one character below it is not — {} chars for {N} tokens",
        edge - 1
    );

    // The floor is a threshold too, and the same test applies to it: 199
    // tokens is noise however little showed, 200 is not.
    assert!(
        crate::app::unaccounted_output(billed(199), 0).is_none(),
        "under the floor, the arithmetic is noise"
    );
    assert!(
        crate::app::unaccounted_output(billed(200), 0).is_some(),
        "at the floor it starts counting"
    );

    // And a whole reply — ~4 chars a token — is nowhere near it, which is the
    // property that keeps this off the working path. An alarm that fires on
    // success is an alarm nobody reads; it did, once, for a different reason.
    assert!(
        crate::app::unaccounted_output(billed(N), N as usize * 4).is_none(),
        "an ordinary reply must never trip it"
    );
}
