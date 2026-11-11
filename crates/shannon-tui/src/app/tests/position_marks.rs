//! **Your marks in a transcript** — `m{a-z}` and `'{a-z}` (`navigation.md`
//! §7, N3), through the routes a keypress takes (`key_for_test`): set where
//! the cursor is, gone back to at the same place — from every window on the
//! session, through what streams below — and refused, said, where none is
//! set and in a pane that keeps none.
//!
//! (`marks.rs` is the panes' marks, `:Send`'s: "a marked pane".)

use super::*;

pub(super) const SIZE: (u16, u16) = (80, 24);

/// A focused transcript of `n` turns, synced as a frame would. Each turn is
/// six lines: a rule, `> ask N`, a rule, a blank, `  ok`, a blank.
fn transcript(n: usize) -> (App, crate::wm::PaneId) {
    let mut app = test_app();
    for i in 0..n {
        app.output_mut().begin_turn(&format!("ask {i}"));
        app.output_mut().append_chunk("  ok");
        app.output_mut().end_turn();
    }
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    (app, pane)
}

/// The first line of `pane`'s copy reading `text`.
pub(super) fn line_reading(app: &App, pane: crate::wm::PaneId, text: &str) -> usize {
    let chat = app.pane_chat(pane);
    (0..chat.line_count())
        .find(|&l| chat.line_text(l).as_deref() == Some(text))
        .unwrap_or_else(|| panic!("no line reads {text:?}"))
}

/// **A mark is gone back to at its place, its column too** (Steven, 10-26:
/// `'` keeps vim's exact place, `` ` `` being the agent's), and setting one
/// says which turn it is in — nothing else on screen shows it yet.
#[test]
fn a_mark_is_gone_back_to_at_its_place() {
    let (mut app, pane) = transcript(10);
    type_keys_sized(&mut app, SIZE, "gg7j3l");
    assert_eq!(
        app.pane_chat(pane).cursor(),
        (7, 3),
        "premise: in `> ask 1`"
    );
    type_keys_sized(&mut app, SIZE, "ma");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("mark `a` — turn 2"), "{said}");
    type_keys_sized(&mut app, SIZE, "G");
    assert_ne!(app.pane_chat(pane).cursor().0, 7, "premise: away from it");
    type_keys_sized(&mut app, SIZE, "'a");
    assert_eq!(app.pane_chat(pane).cursor(), (7, 3));
    assert!(
        app.notice().is_none(),
        "gone, not refused: {:?}",
        app.notice()
    );
}

/// **Steven's case** (10-25): "mark something while streaming, split, and
/// jump back to that mark". The mark is the session's, so the split has
/// it; and what streams on below it — more of its own line, a record —
/// does not move it off its text.
#[test]
fn a_mark_set_while_a_reply_streams_is_gone_back_to_from_a_split() {
    let (mut app, pane) = transcript(10);
    app.output_mut().begin_turn("stream");
    app.output_mut().append_chunk("first");
    app.sync_chat();
    type_keys_sized(&mut app, SIZE, "G");
    let at = app.pane_chat(pane).cursor();
    let streaming = app.pane_chat(pane).line_text(at.0);
    assert_eq!(
        streaming.as_deref(),
        Some("first"),
        "premise: the streaming line"
    );
    type_keys_sized(&mut app, SIZE, "ma");

    app.output_mut().append_chunk(" and more\nsecond line");
    app.output_mut().push_tool_activity("$ ls", "a.txt");
    app.output_mut().append_chunk("after the tool");
    // Through the command seam, which ends as the loop ends the Enter's
    // event: called bare, the split's focus move was settled by the next
    // key, which cancelled that key's chord — the `gg` below went nowhere
    // and the jump "landed" where the cursor already stood.
    app.dispatch_command_for_test("split");
    let split = app.workspace.focused();
    assert_ne!(split, pane, "premise: a second window");
    assert_eq!(
        app.pane_session(split).id,
        app.pane_session(pane).id,
        "premise: on the same session"
    );
    draw_at(&mut app, SIZE.0, SIZE.1);
    type_keys_sized(&mut app, SIZE, "gg");
    assert_eq!(
        app.pane_chat(split).cursor(),
        (0, 0),
        "premise: away from it"
    );
    type_keys_sized(&mut app, SIZE, "'a");
    assert_eq!(app.pane_chat(split).cursor(), at, "{:?}", app.notice());
    assert_eq!(
        app.pane_chat(split).line_text(at.0).as_deref(),
        Some("first and more"),
        "its text, grown"
    );
}

/// **A mark not set is said, and nothing moves** — vim's E20 — under an
/// operator too, which copies nothing.
#[test]
fn a_mark_not_set_is_said_and_nothing_moves() {
    let (mut app, pane) = transcript(10);
    type_keys_sized(&mut app, SIZE, "gg5j'z");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("mark `z` is not set"), "{said}");
    assert_eq!(app.pane_chat(pane).cursor().0, 5);
    type_keys_sized(&mut app, SIZE, "y'z");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("mark `z` is not set"), "{said}");
    assert!(app.prompt().register_text().is_none(), "nothing copied");
}

/// **A mark in a turn that was undone is not set** — though the next turn
/// takes its index and its first lines, which a mark by turn and line would
/// land on (`output::Anchor`).
#[test]
fn a_mark_in_a_turn_undone_is_not_set() {
    let (mut app, pane) = transcript(3);
    app.output_mut().begin_turn("never sent");
    app.sync_chat();
    let echo = line_reading(&app, pane, "> never sent");
    type_keys_sized(&mut app, SIZE, &format!("gg{echo}jma"));
    assert!(
        app.output_mut().abandon_turn("you stopped it"),
        "premise: nothing ran"
    );
    app.output_mut().begin_turn("sent");
    app.sync_chat();
    assert_eq!(
        line_reading(&app, pane, "> sent"),
        echo,
        "premise: its line, reused"
    );
    type_keys_sized(&mut app, SIZE, "gg'a");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("mark `a` is not set"), "{said}");
    assert_eq!(app.pane_chat(pane).cursor().0, 0, "nothing moved");
}

/// **An operator over a mark is linewise**, as vim's `'` is: `y'a` from
/// above copies the whole lines between.
#[test]
fn an_operator_over_a_mark_is_linewise() {
    let (mut app, _) = transcript(10);
    type_keys_sized(&mut app, SIZE, "gg7j3lma");
    type_keys_sized(&mut app, SIZE, "ggy'a");
    assert_eq!(
        app.prompt().register_text().as_deref(),
        Some("\n> ask 0\n\n\n  ok\n\n\n> ask 1\n"),
    );
}

/// **`m` in Visual sets the mark where the cursor is, and the selection
/// stays** — vim's `vjma` (Neovim checked 10-31: still Visual, `'a` at the
/// cursor, the selection's start unmoved; review stop 14, A 8). It did
/// nothing, said nothing, and its letter then armed a text object.
#[test]
fn m_in_visual_sets_the_mark_and_keeps_the_selection() {
    let (mut app, pane) = transcript(3);
    type_keys_sized(&mut app, SIZE, "gg7jvjl");
    let (start, cursor) = (app.pane_chat(pane).anchor(), app.pane_chat(pane).cursor());
    assert!(start.is_some(), "premise: selecting");
    type_keys_sized(&mut app, SIZE, "ma");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("mark `a` — turn"), "set, said: {said}");
    assert_eq!(
        app.focused_mode(),
        crate::keymap::Mode::Visual,
        "still Visual"
    );
    assert_eq!(app.pane_chat(pane).anchor(), start, "the selection kept");
    assert_eq!(app.pane_chat(pane).cursor(), cursor, "nothing moved");
    type_keys_sized(&mut app, SIZE, "<Esc>G'a");
    assert_eq!(app.pane_chat(pane).cursor(), cursor, "and `'a` goes there");
}

/// **A pane that keeps no marks says so** — the prompt and a page — for
/// `m` and for `'`, rather than a silent key (transcripts first: `marks.md`
/// Q1).
#[tokio::test]
async fn a_pane_that_keeps_no_marks_says_so() {
    // Every kind but the transcript, from the type (review stop 14, C 11:
    // two were hand-listed). `:graph`'s own `m` marks its rows, and says so.
    let kinds: Vec<PaneKind> = (PaneKind::all().into_iter())
        .filter(|kind| *kind != PaneKind::Output)
        .collect();
    assert!(kinds.len() > 5, "premise: the kinds: {kinds:?}");
    for kind in kinds {
        for keys in ["ma", "'a", "`a"] {
            let mut app = read_only_pane(kind).await;
            if kind == PaneKind::Prompt {
                type_keys_sized(&mut app, SIZE, "<Esc>");
            }
            type_keys_sized(&mut app, SIZE, keys);
            let said = app.notice().unwrap_or_default();
            let wanted = match (kind, keys) {
                (PaneKind::Graph, "ma") => "is not a graph command",
                _ => "marks are a transcript's",
            };
            assert!(said.contains(wanted), "{kind:?} `{keys}`: {said:?}");
        }
    }
}

// ---- N4: the agent's marks — its `mark` tool, gone to with `` ` `` ------------

/// The live turn the agent marks in: your prompt, and what it has written.
fn agents_turn(app: &mut App, reply: &str) {
    app.conversation_mut().begin_turn("go");
    app.output_mut().begin_turn("go");
    app.output_mut().append_chunk(reply);
}

/// The agent calls `mark` with `input`, as the stream delivers a call — and
/// ends the round as a stream ends it.
fn agent_marks(app: &mut App, call: &str, input: serde_json::Value) {
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::ToolUse {
        id: call.into(),
        name: shannon_core::MARK_TOOL.into(),
        input,
    }));
    app.feed_stream(StreamMsg::Complete);
}

/// What `call` was answered, and whether as an error — pending, or in the
/// session's history.
fn answer_to(app: &App, call: &str) -> Option<(String, bool)> {
    let returned = (app.buffers.sessions()).flat_map(|b| {
        b.conversation
            .messages()
            .iter()
            .flat_map(|m| m.blocks.iter())
    });
    (app.tool_results.values().chain(returned)).find_map(|b| match b {
        shannon_core::Block::ToolResult {
            tool_use_id,
            content,
            is_error,
        } if tool_use_id == call => Some((content.clone(), *is_error)),
        _ => None,
    })
}

/// **The agent marks where it is, and `` `a `` goes there** — the call's own
/// record, which says it was marked, under what it wrote first. Answered at
/// once (it runs nothing: no decision), named in the result so the reply
/// names a mark that exists; apart from yours, so `'a` is not it.
#[tokio::test]
async fn the_agent_marks_here_and_backtick_goes_there() {
    let (mut app, pane) = transcript(5);
    agents_turn(&mut app, "Here is the plan: keep a BTreeMap.");
    agent_marks(&mut app, "tu_1", serde_json::json!({ "note": "the plan" }));
    assert!(app.shown_call().is_none(), "runs nothing: no decision");
    let (said, error) = answer_to(&app, "tu_1").expect("answered");
    assert!(!error, "{said}");
    assert!(said.contains("mark `a` set here"), "{said}");
    assert_eq!(
        said, "mark `a` set here.",
        "the letter, for the reply to name"
    );
    app.sync_chat();
    let record = line_reading(&app, pane, "⚙ marked: mark \"the plan\"");
    type_keys_sized(&mut app, SIZE, "gg");
    assert_eq!(
        app.pane_chat(pane).cursor(),
        (0, 0),
        "premise: away from it"
    );
    type_keys_sized(&mut app, SIZE, "`a");
    assert_eq!(
        app.pane_chat(pane).cursor(),
        (record, 0),
        "{:?}",
        app.notice()
    );
    type_keys_sized(&mut app, SIZE, "gg'a");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("mark `a` is not set"),
        "yours are apart: {said}"
    );
}

/// **The agent marks an earlier place by quoting it** — the latest turn
/// that has the phrase, or the turn `turns_ago` names; `` `a `` lands on the
/// phrase itself.
#[tokio::test]
async fn the_agent_marks_an_earlier_place_by_a_quote() {
    let (mut app, pane) = transcript(5);
    agents_turn(&mut app, "Now the next step.");
    let at = |quote: &str, ago: Option<u64>| match ago {
        Some(ago) => {
            serde_json::json!({ "note": "asked", "at": { "quote": quote, "turns_ago": ago } })
        }
        None => serde_json::json!({ "note": "asked", "at": { "quote": quote } }),
    };
    agent_marks(&mut app, "tu_1", at("ask", None));
    agent_marks(&mut app, "tu_2", at("ask", Some(3)));
    let (said, error) = answer_to(&app, "tu_1").expect("answered");
    let landed = "mark `a` set at \"ask\", in the user's message 1 turn ago.";
    assert!(!error && said.contains(landed), "{said}");
    app.sync_chat();
    type_keys_sized(&mut app, SIZE, "gg`a");
    let at_a = (line_reading(&app, pane, "> ask 4"), 2);
    assert_eq!(
        app.pane_chat(pane).cursor(),
        at_a,
        "the latest turn that has it"
    );
    type_keys_sized(&mut app, SIZE, "gg`b");
    let at_b = (line_reading(&app, pane, "> ask 2"), 2);
    assert_eq!(
        app.pane_chat(pane).cursor(),
        at_b,
        "three turns back from `go`"
    );
}

/// **A quote lands on the conversation, not on what asked or on a mark**
/// (review stop 14, A 6): searching back, this turn's own request is left
/// out ("mark where we chose BTreeMap" marked your request), and so is a
/// `mark`'s record, whose result repeats the quote (a second mark at the
/// phrase landed on the first's record). The result says where it landed,
/// in `turns_ago`'s terms; named, this turn's request is searched too.
#[tokio::test]
async fn a_quote_lands_on_the_conversation_and_says_where() {
    let (mut app, pane) = transcript(1);
    app.output_mut().begin_turn("pick a map");
    app.output_mut()
        .append_chunk("We chose BTreeMap for order.");
    app.output_mut().end_turn();
    let wanted = "mark where we chose BTreeMap";
    app.conversation_mut().begin_turn(wanted);
    app.output_mut().begin_turn(wanted);
    app.output_mut().append_chunk("Marking it.");
    let at = |ago: Option<u64>| {
        let mut at = serde_json::json!({ "quote": "BTreeMap" });
        if let Some(ago) = ago {
            at["turns_ago"] = ago.into();
        }
        serde_json::json!({ "note": "the choice", "at": at })
    };
    agent_marks(&mut app, "tu_1", at(None));
    agent_marks(&mut app, "tu_2", at(None));
    agent_marks(&mut app, "tu_3", at(Some(0)));
    let said = |call| answer_to(&app, call).expect("answered").0;
    let reply = "set at \"BTreeMap\", in your reply 1 turn ago.";
    assert!(said("tu_1").contains(reply), "{}", said("tu_1"));
    assert!(
        said("tu_2").contains(reply),
        "not on the first's record: {}",
        said("tu_2")
    );
    let asked = "set at \"BTreeMap\", in the user's message this turn.";
    assert!(said("tu_3").contains(asked), "{}", said("tu_3"));
    app.sync_chat();
    let chose = line_reading(&app, pane, "We chose BTreeMap for order.");
    for name in ['a', 'b'] {
        type_keys_sized(&mut app, SIZE, &format!("gg`{name}"));
        assert_eq!(app.pane_chat(pane).cursor().0, chose, "`{name}");
    }
    type_keys_sized(&mut app, SIZE, "gg`c");
    let echo = line_reading(&app, pane, &format!("> {wanted}"));
    assert_eq!(app.pane_chat(pane).cursor().0, echo, "named: your request");
}

/// **A quote cannot read a held value** (review stop 14, B F1 — security):
/// the transcript draws a value where the model wrote its handle, and a quote
/// matched against the drawn text answered "set" or "not found" about a value
/// the model was never sent — one bit a call, a prefix at a time. Now a
/// value's prefix is refused exactly as text never written, in your own line,
/// in a reply and in a tool's output; the handle, as the model has it, is
/// found. And a quote past the bound is refused unsearched (B F5).
#[tokio::test]
async fn a_quote_cannot_read_a_held_value() {
    let mut app = test_app();
    let value = "hunter2beornot2be";
    let handle = app.mark_secret(value, true).expect("marked");
    app.submit(&format!("password is {value}"));
    app.feed_stream(StreamMsg::Chunk(format!("export TOKEN={handle}\n")));
    app.feed_stream(StreamMsg::ToolRecord {
        call: shannon_core::Block::ToolUse {
            id: "t9".into(),
            name: "Bash".into(),
            input: serde_json::json!({ "command": "env" }),
        },
        result: shannon_core::Block::ToolResult {
            tool_use_id: "t9".into(),
            content: format!("out={handle}"),
            is_error: false,
        },
    });
    // Premise: each source draws the value, so the old search would see it.
    let drawn = app.output_mut().lines().join("\n");
    for line in [
        format!("password is {value}"),
        format!("TOKEN={value}"),
        format!("out={value}"),
    ] {
        assert!(drawn.contains(&line), "premise: drawn {line:?}:\n{drawn}");
    }
    let at = |quote: &str| serde_json::json!({ "note": "n", "at": { "quote": quote } });
    let absent = {
        agent_marks(&mut app, "t0", at("TOKEN=zzz"));
        answer_to(&app, "t0").expect("answered")
    };
    assert!(absent.1, "premise: an absent quote is refused: {absent:?}");
    for (call, quote) in [
        ("t1", "TOKEN=hunter2b"),
        ("t2", "password is hunter"),
        ("t3", "out=hunter2"),
        ("t4", value),
    ] {
        agent_marks(&mut app, call, at(quote));
        let (said, error) = answer_to(&app, call).expect("answered");
        assert!(error, "{quote}: a value's prefix must not be found: {said}");
        assert_eq!(
            said,
            absent.0.replace("TOKEN=zzz", quote),
            "{quote}: refused exactly as text never written"
        );
    }
    agent_marks(&mut app, "t5", at(&format!("TOKEN={handle}")));
    let (said, error) = answer_to(&app, "t5").expect("answered");
    assert!(!error, "the handle, as the model has it, is found: {said}");
    let long = "x".repeat(201);
    agent_marks(&mut app, "t6", at(&long));
    let (said, error) = answer_to(&app, "t6").expect("answered");
    assert!(
        error && said.contains("200 at most") && !said.contains(&long),
        "{said}"
    );
}

/// **Nor a value across lines** (review stop 14, Reviewer E F2): a PEM key
/// drawn where the model wrote its handle sits on three lines, and a line at
/// a time sent its middle one in clear — `MIIEvQ` found, `MIIEvX` not. Now
/// its block is searched whole, as sent: a middle line's prefix, and the
/// drawn first line, are refused as text never written; the handle is found.
#[tokio::test]
async fn a_quote_cannot_read_a_value_across_lines() {
    let mut app = test_app();
    let value = "-----BEGIN KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\n-----END KEY-----";
    let handle = app.mark_secret(value, true).expect("marked");
    app.submit("show me");
    app.feed_stream(StreamMsg::Chunk(format!(
        "the key is {handle}\nkept safe\n"
    )));
    let drawn = app.output_mut().lines().join("\n");
    assert!(
        drawn.contains("MIIEvQIBADANBgkqhkiG9w0BAQEFAASC"),
        "premise: the value is drawn across lines:\n{drawn}"
    );
    let at = |quote: &str| serde_json::json!({ "note": "n", "at": { "quote": quote } });
    agent_marks(&mut app, "t0", at("MIIEvX"));
    let absent = answer_to(&app, "t0").expect("answered");
    assert!(absent.1, "premise: an absent quote is refused: {absent:?}");
    for (call, quote) in [
        ("t1", "MIIEvQ"),
        ("t2", "MIIEvQIBADANB"),
        ("t3", "the key is -----BEGIN"),
    ] {
        agent_marks(&mut app, call, at(quote));
        let (said, error) = answer_to(&app, call).expect("answered");
        assert!(error, "{quote}: a value's line must not be found: {said}");
        assert_eq!(
            said,
            absent.0.replace("MIIEvX", quote),
            "{quote}: refused exactly as text never written"
        );
    }
    agent_marks(&mut app, "t4", at(&format!("the key is {handle}")));
    let (said, error) = answer_to(&app, "t4").expect("answered");
    assert!(!error, "the handle, as the model has it, is found: {said}");
    agent_marks(&mut app, "t5", at("kept safe"));
    let (said, error) = answer_to(&app, "t5").expect("answered");
    assert!(!error, "and the text around it: {said}");
}

/// **Nor a value the transcript draws in another form** (the close's
/// verifier, V 2): a tab drawn as a space, a CRLF key's `\r` dropped — the
/// drawn text no exact mask matched, so a quote found what the model was
/// never sent. Each refused exactly as text never written; the handle still
/// found.
#[tokio::test]
async fn a_quote_cannot_read_a_value_drawn_in_another_form() {
    for value in [
        "abc\tdefghij-tabbed",
        "-----BEGIN KEY-----\r\nMIIBsecretline0123\r\n-----END KEY-----",
    ] {
        let mut app = test_app();
        let handle = app.mark_secret(value, true).expect("marked");
        app.submit("show me");
        app.feed_stream(StreamMsg::Chunk(format!("it is {handle}\nkept safe\n")));
        let at = |quote: &str| serde_json::json!({ "note": "n", "at": { "quote": quote } });
        agent_marks(&mut app, "t0", at("never written here"));
        let absent = answer_to(&app, "t0").expect("answered");
        assert!(absent.1, "premise: an absent quote is refused");
        for (call, quote) in [("t1", "defghij-tab"), ("t2", "MIIBsecret")] {
            agent_marks(&mut app, call, at(quote));
            let (said, _) = answer_to(&app, call).expect("answered");
            assert_eq!(
                said,
                absent.0.replace("never written here", quote),
                "{value:?}: {quote} refused as never written"
            );
        }
        agent_marks(&mut app, "t3", at(&format!("it is {handle}")));
        let (said, error) = answer_to(&app, "t3").expect("answered");
        assert!(!error, "{value:?}: the handle is found: {said}");
    }
}

/// **What the agent's `mark` cannot use is refused, saying why** (review
/// stop 14): a `turns_ago` that is no whole number was read as "any turn"
/// and told "set" (B F9); a note past 200 characters would be written into
/// every save again and drawn in `:marks` (B F6).
#[tokio::test]
async fn a_mark_the_agent_shapes_wrong_is_refused() {
    let (mut app, _) = transcript(3);
    agents_turn(&mut app, "Done.");
    let ok = serde_json::json!({ "note": "n", "at": { "quote": "ask", "turns_ago": 1 } });
    agent_marks(&mut app, "t0", ok);
    assert!(
        !answer_to(&app, "t0").expect("answered").1,
        "premise: 1 is set"
    );
    for (call, ago) in [
        ("t1", serde_json::json!("2")),
        ("t2", serde_json::json!(-1)),
        ("t3", serde_json::json!(1.5)),
    ] {
        let input = serde_json::json!({ "note": "n", "at": { "quote": "ask", "turns_ago": ago } });
        agent_marks(&mut app, call, input);
        let (said, error) = answer_to(&app, call).expect("answered");
        assert!(error && said.contains("whole number"), "{ago}: {said}");
    }
    agent_marks(
        &mut app,
        "t4",
        serde_json::json!({ "note": "x".repeat(201) }),
    );
    let (said, error) = answer_to(&app, "t4").expect("answered");
    assert!(error && said.contains("200 at most"), "{said}");
    agent_marks(
        &mut app,
        "t5",
        serde_json::json!({ "note": "x".repeat(200) }),
    );
    assert!(!answer_to(&app, "t5").expect("answered").1, "200 is kept");
}

/// **A saved turn is read from disk, so `:marks` draws it without
/// overflow** (review stop 14, B F2): `turn + 1` on `usize::MAX` panicked
/// the page in a debug build.
#[tokio::test]
async fn marks_draws_a_saved_turn_past_any() {
    let (mut app, _) = transcript(1);
    app.buffer_mut().unplaced_marks.push(shannon_core::Mark {
        name: 'z',
        whose: shannon_core::Whose::Yours,
        turn: usize::MAX,
        block: "you".into(),
        nth: 0,
        line: 0,
        col: 0,
        note: String::new(),
    });
    let page = app.marks_lines(80).plain_text();
    assert!(page.contains("'z (turn"), "{page}");
}

/// **A mark the agent cannot place is refused to it, saying why** — and on
/// the record, so you see it too; no mark is made.
#[tokio::test]
async fn a_mark_the_agent_cannot_place_is_refused_to_it() {
    let (mut app, pane) = transcript(3);
    agents_turn(&mut app, "Done.");
    for (call, input, why) in [
        (
            "tu_1",
            serde_json::json!({ "note": "  " }),
            "needs a `note`",
        ),
        (
            "tu_2",
            serde_json::json!({ "note": "x", "at": { "quote": "never written" } }),
            "no line in this conversation holds",
        ),
        (
            "tu_3",
            serde_json::json!({ "note": "x", "at": { "quote": "ask", "turns_ago": 99 } }),
            "before the first turn",
        ),
        (
            "tu_4",
            serde_json::json!({ "note": "x", "at": { "quote": "" } }),
            "`at` needs a `quote`",
        ),
    ] {
        agent_marks(&mut app, call, input);
        let (said, error) = answer_to(&app, call).expect("answered");
        assert!(error && said.contains(why), "{call}: {said}");
    }
    let shown = app.output().lines().join("\n");
    assert!(shown.contains("⚙ NOT MARKED: mark"), "{shown}");
    app.sync_chat();
    type_keys_sized(&mut app, SIZE, "gg`a");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("the agent has set no mark `a`"), "{said}");
    assert_eq!(app.pane_chat(pane).cursor(), (0, 0), "nothing moved");
}

/// **Shannon names the agent's marks — the next free letter, and past 26
/// the oldest's** (NQ5), so an old summary's `` `c `` may name another
/// place, never nothing.
#[tokio::test]
async fn the_agent_is_given_the_next_letter_and_the_oldest_past_26() {
    let (mut app, pane) = transcript(1);
    agents_turn(&mut app, "Marks.");
    for i in 0..27 {
        let note = format!("place {i}");
        agent_marks(
            &mut app,
            &format!("tu_{i}"),
            serde_json::json!({ "note": note }),
        );
    }
    let named = |call: &str| answer_to(&app, call).expect("answered").0;
    assert!(named("tu_0").contains("mark `a`"), "{}", named("tu_0"));
    assert!(named("tu_1").contains("mark `b`"), "{}", named("tu_1"));
    assert!(named("tu_25").contains("mark `z`"), "{}", named("tu_25"));
    assert!(
        named("tu_26").contains("mark `a`"),
        "the oldest's: {}",
        named("tu_26")
    );
    app.sync_chat();
    type_keys_sized(&mut app, SIZE, "gg`a");
    let at = app.pane_chat(pane).cursor().0;
    let text = app.pane_chat(pane).line_text(at).unwrap_or_default();
    assert_eq!(text, "⚙ marked: mark \"place 26\"", "`a is the newest now");
}

// ---- kept with the session (Steven, 10-28) ---------------------------------------

/// An app over the store at `dir`, as a launch that starts a session.
pub(super) fn over_store(dir: &std::path::Path) -> App {
    test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir).unwrap()),
    )
}

/// A turn made as a live one is — your prompt, streamed text, the agent's
/// `mark` call at a quote, a command that runs (its `⚙ running:` line, then
/// the record that closes it in that line's place: two blocks live where a
/// reopened turn drew one until review stop 15, F H2 — stop 14, A 1), more
/// text, the agent's `mark` here, more, the round ended — in an app over
/// the store at `dir`, its transcript focused and synced.
async fn live_story(dir: &std::path::Path) -> (App, crate::wm::PaneId) {
    let mut app = over_store(dir);
    app.submit("tell me a story in two chapters");
    app.feed_stream(StreamMsg::Chunk(
        "# Chapter One\n\nShe looked out at the water.\n".into(),
    ));
    let at = serde_json::json!({ "note": "chapter one", "at": { "quote": "Chapter One" } });
    agent_marks(&mut app, "tu_1", at);
    // Allowed unasked; its outcome fed back as the loop does.
    app.feed_stream(StreamMsg::ToolUse(shannon_core::Block::bash_call(
        "tu_cmd",
        "echo the tide",
    )));
    app.feed_stream(StreamMsg::Complete);
    settle(&mut app).await;
    app.feed_stream(StreamMsg::Chunk("# Chapter Two\n\nThe letter came.".into()));
    agent_marks(
        &mut app,
        "tu_2",
        serde_json::json!({ "note": "chapter two, here" }),
    );
    app.feed_stream(StreamMsg::Chunk("The end.".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    (app, pane)
}

/// The same store, opened again as a launch that resumes.
pub(super) fn reopened(dir: &std::path::Path) -> (App, crate::wm::PaneId) {
    let mut app = test_app_with_store(
        test_backend(),
        config_resume_last(),
        Box::new(JsonlStore::new(dir).unwrap()),
    );
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    (app, pane)
}

/// Each block of a transcript a reopened one draws — all but a call's
/// running line — its turn, its role, its lines.
pub(super) fn drawn(app: &App) -> Vec<(Option<usize>, crate::output::BlockRole, Vec<String>)> {
    (app.output().spans())
        .filter(|s| !s.live_only)
        .map(|s| (s.turn, s.role, s.lines.to_vec()))
        .collect()
}

/// **A reopened turn draws the blocks it drew live** — the same roles, in
/// the same order, with the same lines, and nothing drawn live only: the
/// record that closed the call took its `⚙ running:` line's place (review
/// stop 15, F H2), where it once stood below it. What a kept mark's place
/// stands on (`output::Position`: a turn, a block by role and which, a
/// line): were a replay to draw one block more or less, every mark after
/// it would name another place.
#[tokio::test]
async fn a_reopened_turn_draws_the_blocks_it_drew_live() {
    let dir = TempDir::new().unwrap();
    let (app, _) = live_story(dir.path()).await;
    let running = (app.output().spans()).filter(|s| s.live_only).count();
    assert_eq!(running, 0, "the command's running line, replaced");
    let live = drawn(&app);
    assert!(
        (live.iter()).any(|(_, _, lines)| lines.iter().any(|l| l == "⚙ ran:")),
        "premise: the record that closed it: {live:?}"
    );
    let again = drawn(&reopened(dir.path()).0);
    assert_eq!(again, live);
}

/// **A call you denied reads reopened as it read live** (Steven, 11-01:
/// `DENIED — nothing ran · 4 lines` live, `refused or failed · 7 lines`
/// reopened) — the same lines under its header, what the model was told
/// with them, live too; only the verb differs, the stored result saying no
/// more than that the call failed (`replayed_verb`).
#[tokio::test]
async fn a_denied_call_reads_reopened_as_it_did_live() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    app.submit("clear it out");
    let call = shannon_core::Block::bash_call("tu_rm", "rm -rf /tmp/shannon-test-absent");
    app.feed_stream(StreamMsg::ToolUse(call));
    app.feed_stream(StreamMsg::Complete);
    app.settle(); // it asks: nothing runs, nothing to await
    let buffer = app.prompt_target();
    tool_key_in(&mut app, buffer, 'd');
    app.feed_stream(StreamMsg::Chunk("Left it alone.".into()));
    app.feed_stream(StreamMsg::Complete);
    let record = |app: &App| {
        let tool = crate::output::BlockRole::Tool;
        let span = (app.output().spans()).find(|s| s.role == tool);
        span.expect("its record").lines.to_vec()
    };
    let live = record(&app);
    let again = record(&reopened(dir.path()).0);
    assert!(live[0].starts_with("⚙ DENIED — nothing ran"), "{live:#?}");
    assert!(again[0].starts_with("⚙ refused or failed"), "{again:#?}");
    assert_eq!(live[1..], again[1..], "the same lines under the header");
    let told = live.iter().any(|l| l.contains("denied by the user"));
    assert!(told, "what the model was told: {live:#?}");
}

/// **Marks are kept with the session** (Steven, 10-28: "We should store
/// marks along with the conversation, so agent marks shouldn't be destroyed
/// when we exit") — reopened, yours are yours and the agent's the agent's,
/// with its note, each on the text it marked, and the agent's in the order
/// they were made, so the next it sets takes the next letter.
#[tokio::test]
async fn marks_are_kept_with_the_session_and_reopen_as_they_were() {
    let dir = TempDir::new().unwrap();
    let place = |app: &App, anchor| {
        let (line, col) = app.output().locate(anchor).expect("set");
        (app.output().lines()[line].clone(), col)
    };
    let (yours, output, agents) = {
        let (mut app, pane) = live_story(dir.path()).await;
        let line = line_reading(&app, pane, "She looked out at the water.");
        type_keys_sized(&mut app, SIZE, &format!("gg{line}j4lma"));
        // And on what the command printed (the test seam's account): the
        // record in its running line's place, the turn's second record live
        // and reopened (the third live, below that line, until review stop
        // 15, F H2).
        let chat = app.pane_chat(pane);
        let printed = (0..chat.line_count())
            .find(|&l| {
                chat.line_text(l)
                    .is_some_and(|t| t.contains("not run) echo the tide"))
            })
            .unwrap_or_else(|| panic!("the command's output: {:#?}", app.output().lines()));
        type_keys_sized(&mut app, SIZE, &format!("gg{printed}jmb"));
        let said = app.notice().unwrap_or_default();
        assert!(!said.contains("not saved"), "kept: {said}");
        let session = app.buffer();
        let yours = place(&app, session.marks[&'a']);
        let output = place(&app, session.marks[&'b']);
        let agents: Vec<_> = (session.agent_marks.iter())
            .map(|(&n, m)| (n, place(&app, m.anchor), m.note.clone()))
            .collect();
        assert_eq!(agents.len(), 2, "premise: the agent's two: {agents:?}");
        (yours, output, agents)
    };
    let (app, _) = reopened(dir.path());
    let session = app.buffer();
    assert_eq!(
        session.marks.len(),
        2,
        "yours, and only yours: {:?}",
        session.marks
    );
    assert_eq!(place(&app, session.marks[&'a']), yours);
    assert_eq!(yours, ("She looked out at the water.".to_string(), 4));
    assert_eq!(
        place(&app, session.marks[&'b']),
        output,
        "on what it printed"
    );
    let again: Vec<_> = (session.agent_marks.iter())
        .map(|(&n, m)| (n, place(&app, m.anchor), m.note.clone()))
        .collect();
    assert_eq!(again, agents, "the agent's, as the agent's");
    assert!(
        session.unplaced_marks.is_empty(),
        "{:?}",
        session.unplaced_marks
    );
    let made: Vec<char> = {
        let mut by: Vec<_> = session.agent_marks.iter().collect();
        by.sort_by_key(|(_, m)| m.made);
        by.into_iter().map(|(&n, _)| n).collect()
    };
    assert_eq!(made, ['a', 'b'], "the order they were made in");
}

/// **The agent's marks alone outlive the run** — Steven's case: the agent
/// marked, nothing of yours, and the app quit. Kept when they were made:
/// a session first written at its turn's end takes them in its first
/// header. (Its own test: with one of yours set after, your save carried
/// theirs, and a mutant deleting the agent's own save went GREEN.)
#[tokio::test]
async fn the_agents_marks_alone_outlive_the_run() {
    let dir = TempDir::new().unwrap();
    let made = {
        let (app, _) = live_story(dir.path()).await;
        assert!(app.buffer().marks.is_empty(), "premise: none of yours");
        app.buffer().agent_marks.len()
    };
    assert_eq!(made, 2, "premise: the agent's two");
    let (app, _) = reopened(dir.path());
    let session = app.buffer();
    let names: Vec<char> = session.agent_marks.keys().copied().collect();
    assert_eq!(names, ['a', 'b'], "{:?}", session.unplaced_marks);
    assert!(session.marks.is_empty(), "and none became yours");
}

/// A turn answered `reply`, sealed as a stream seals it.
fn answered(app: &mut App, prompt: &str, reply: &str) {
    app.submit(prompt);
    app.feed_stream(StreamMsg::Chunk(reply.into()));
    app.feed_stream(StreamMsg::Complete);
}

/// `pane` focused on the transcript, synced, its cursor on the first line
/// reading `text`.
fn cursor_on(app: &mut App, text: &str) -> crate::wm::PaneId {
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    let line = line_reading(app, pane, text);
    // `{n}G`, which names a line: `j` steps over a shut fold as one (N5a).
    type_keys_sized(app, SIZE, &format!("{}G", line + 1));
    pane
}

/// **A failure before any reply leaves the turns numbered as the store
/// numbers them** (review stop 14, A 2). The conversation drops the turn;
/// the transcript keeps your words and the failure under them, as no
/// turn's. Numbered, the transcript ran a turn ahead of the store: every
/// mark set after it reopened on the next turn's text — "reply three"'s
/// on "reply four".
#[tokio::test]
async fn a_failure_before_any_reply_keeps_the_stores_numbering() {
    let dir = TempDir::new().unwrap();
    {
        let mut app = over_store(dir.path());
        answered(&mut app, "first", "reply one");
        app.submit("second");
        app.feed_stream(StreamMsg::Error(BackendError::RateLimited));
        let lines = app.output().lines();
        let at = |text: &str| (lines.iter()).position(|l| l.contains(text));
        let (asked, failed) = (at("> second").expect("kept"), at("[error:").expect("said"));
        assert!(asked < failed, "the failure under what asked: {lines:?}");
        assert_eq!(app.output().turn_of_line(failed), None, "in no turn");
        assert_eq!(app.output().last_turn(), Some(0), "the store's one turn");
        answered(&mut app, "third", "reply three");
        answered(&mut app, "fourth", "reply four");
        cursor_on(&mut app, "reply three");
        type_keys_sized(&mut app, SIZE, "ma");
        let said = app.notice().unwrap_or_default();
        assert!(
            said.contains("mark `a` — turn 2"),
            "the store's second: {said}"
        );
    }
    let (mut app, pane) = reopened(dir.path());
    type_keys_sized(&mut app, SIZE, "G'a");
    let (line, _) = app.pane_chat(pane).cursor();
    let text = app.pane_chat(pane).line_text(line);
    assert_eq!(text.as_deref(), Some("reply three"), "{:?}", app.notice());
}

/// **A mark in a turn you stopped does not come back on the next** (review
/// stop 14, A 3): its blocks go with the turn, and so does the mark from
/// what the session keeps — at once, said. Kept until the next save, it
/// named the stopped turn's index, which the next turn took: reopened,
/// `'a` landed on that turn's echo.
#[tokio::test]
async fn a_mark_in_a_stopped_turn_does_not_reopen_on_the_next() {
    let dir = TempDir::new().unwrap();
    {
        let mut app = over_store(dir.path());
        answered(&mut app, "first", "reply one");
        app.submit("second");
        cursor_on(&mut app, "> second");
        type_keys_sized(&mut app, SIZE, "ma");
        let said = app.notice().unwrap_or_default();
        assert!(said.contains("mark `a` — turn 2"), "premise: kept: {said}");
        app.ctrl_c_stop_for_test();
        let said = app.notice().unwrap_or_default();
        assert!(
            said.contains("no longer saved with the session — mark `a`: its line is gone"),
            "{said}"
        );
        answered(&mut app, "third", "reply three");
    }
    let (mut app, pane) = reopened(dir.path());
    assert!(
        app.buffer().unplaced_marks.is_empty(),
        "not kept at all: {:?}",
        app.buffer().unplaced_marks
    );
    type_keys_sized(&mut app, SIZE, "gg'a");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("mark `a` is not set"), "{said}");
    assert_eq!(app.pane_chat(pane).cursor().0, 0, "nothing moved");
}

/// **A mark no reopened transcript would draw is set, and said not saved**
/// — on a call's running line (review stop 14, A 1: the record closing the
/// call is the one a reopen draws) and on a line in no turn (A 11) — never
/// dropped from what is kept unsaid.
#[tokio::test]
async fn a_mark_no_reopen_would_draw_is_said_not_saved() {
    let mut app = test_app();
    app.output_mut().begin_turn("run it");
    let running = crate::tool_record::Record::started(
        "running",
        shannon_core::BASH_TOOL,
        &serde_json::json!({ "command": "sleep 9" }),
    );
    app.output_mut().push_tool_record(running);
    let pane = cursor_on(&mut app, "⚙ running: sleep 9");
    type_keys_sized(&mut app, SIZE, "ma");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains(
            "mark `a` — turn 1, not saved: its line is drawn only this run, not when the session \
             reopens"
        ),
        "{said}"
    );
    // The turn dropped, its record kept as no turn's (`abandon_turn`), and
    // your words with it — not in the conversation, said so.
    assert!(
        !app.output_mut().abandon_turn("you stopped it"),
        "premise: kept, not removed"
    );
    cursor_on(&mut app, "> run it");
    type_keys_sized(&mut app, SIZE, "mb");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("mark `b` set, not saved: its turn is not in the conversation"),
        "{said}"
    );
    type_keys_sized(&mut app, SIZE, "gg'b");
    assert_ne!(
        app.pane_chat(pane).cursor().0,
        0,
        "set, for this run: {said}"
    );
    // A line no turn ever had: what lands with no turn open (the safety
    // net of `append_chunk`).
    app.output_mut().append_chunk("outside any turn");
    cursor_on(&mut app, "outside any turn");
    type_keys_sized(&mut app, SIZE, "mc");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("mark `c` set, not saved: its line is in no turn"),
        "{said}"
    );
    assert!(
        app.buffer()
            .conversation
            .episode()
            .marks
            .as_deref()
            .unwrap_or_default()
            .is_empty()
    );
}

/// A mark of yours saved at `turn`, in that turn's second block of yours
/// (the blank under its frame), at `line`.
fn saved_mark(name: char, turn: usize, line: usize) -> shannon_core::Mark {
    shannon_core::Mark {
        name,
        whose: shannon_core::Whose::Yours,
        turn,
        block: "you".into(),
        nth: 1,
        line,
        col: 0,
        note: String::new(),
    }
}

/// **A saved mark this transcript does not draw is kept, never dropped
/// unsaid** (review stop 14, A 4 / B F3 / C 1) — one whose turn is not
/// here, and one past its block's last line: listed by `:marks`, saved
/// again past a mark set since, and replaced by a mark set under its name.
/// A build that draws a turn differently saves such places; each of the
/// four was held by a comment alone.
#[tokio::test]
async fn a_saved_mark_not_drawn_is_kept_listed_and_replaced() {
    let dir = TempDir::new().unwrap();
    {
        let mut app = over_store(dir.path());
        answered(&mut app, "first", "reply one");
        let buffer = app.current;
        let unplaced = vec![saved_mark('y', 7, 0), saved_mark('z', 0, 99)];
        app.conversation_mut().set_marks(unplaced);
        app.write_preferences(buffer).expect("header written");
    }
    let names = |app: &App| -> Vec<char> {
        (app.buffer().unplaced_marks.iter())
            .map(|m| m.name)
            .collect()
    };
    {
        let (mut app, _) = reopened(dir.path());
        assert_eq!(names(&app), ['y', 'z'], "kept aside, both");
        let page = app.marks_lines(200).plain_text();
        assert!(
            page.contains("saved, but not found in this transcript: 'y (turn 8), 'z (turn 1)"),
            "{page}"
        );
        cursor_on(&mut app, "reply one");
        type_keys_sized(&mut app, SIZE, "ma");
    }
    {
        let (mut app, _) = reopened(dir.path());
        assert_eq!(names(&app), ['y', 'z'], "saved again past `ma`");
        assert!(app.buffer().marks.contains_key(&'a'), "and `ma` with them");
        cursor_on(&mut app, "> first");
        type_keys_sized(&mut app, SIZE, "mz");
    }
    let (app, _) = reopened(dir.path());
    assert_eq!(names(&app), ['y'], "`z` is the one set since");
    let (line, _) = app.output().locate(app.buffer().marks[&'z']).expect("set");
    assert_eq!(app.output().lines()[line], "> first");
}

/// **A saved entry this build does not read is said, and kept past a save**
/// (review stop 14, B F4 / F8): another build's shape, and a name no key
/// reaches (`'1'`) — `:marks` counts them, and a mark set since saves them
/// again as they were, where one such entry used to cost every mark.
#[tokio::test]
async fn a_saved_entry_not_read_is_said_and_kept() {
    let dir = TempDir::new().unwrap();
    let newer = serde_json::json!({ "name": "c", "whose": "shared", "turn": 0 });
    let digit = serde_json::to_value(saved_mark('1', 0, 0)).unwrap();
    {
        let mut app = over_store(dir.path());
        answered(&mut app, "first", "reply one");
        let buffer = app.current;
        app.conversation_mut().set_marks(Vec::new());
        app.write_preferences(buffer).expect("header written");
        // A header as a newer build, or a hand, would append it.
        let id = app.conversation().episode().id;
        let path = (dir.path().join("episodes")).join(format!("{}.jsonl", id.to_hex()));
        let text = std::fs::read_to_string(&path).unwrap();
        let last = (text.lines())
            .rfind(|l| l.contains(r#""kind":"episode""#))
            .expect("a header");
        let mut header: serde_json::Value = serde_json::from_str(last).unwrap();
        header["marks"] = serde_json::json!([newer, digit]);
        std::fs::write(&path, format!("{text}{header}\n")).unwrap();
    }
    {
        let (mut app, _) = reopened(dir.path());
        let page = app.marks_lines(200).plain_text();
        assert!(
            page.contains("saved, but not read by this build: 2, kept as they were"),
            "{page}"
        );
        assert!(!page.contains("'1"), "no mark a key cannot reach: {page}");
        cursor_on(&mut app, "reply one");
        type_keys_sized(&mut app, SIZE, "ma");
    }
    let (app, _) = reopened(dir.path());
    let episode = app.buffer().conversation.episode();
    assert_eq!(
        episode.unread_marks,
        [newer, digit],
        "saved again as they were"
    );
    assert!(
        app.buffer().marks.contains_key(&'a'),
        "with the mark set since"
    );
}

// ---- :marks — every mark, a live page ------------------------------------------

/// **`:marks` is a table per session** (Steven's draft, 10-28): the session,
/// a header, yours, a soft rule, the agent's — each in the order they stand
/// in the conversation, not set order nor letters' — one row a mark: mark,
/// turn, line, column, the WORD it stands on, context; the agent's context
/// its note first.
#[tokio::test]
async fn marks_is_a_table_yours_then_the_agents_each_in_turn_order() {
    let (mut app, pane) = transcript(6);
    // Yours set out of order: `b` on turn 2, then `a` on turn 4.
    let ask = |app: &App, n: usize| line_reading(app, pane, &format!("> ask {n}"));
    let (two, four) = (ask(&app, 1), ask(&app, 3));
    type_keys_sized(&mut app, SIZE, &format!("gg{four}j2lmagg{two}j0mb"));
    // The agent's, set later in the conversation first: here, then turn 3.
    agents_turn(&mut app, "The plan is set.");
    agent_marks(&mut app, "tu_1", serde_json::json!({ "note": "the plan" }));
    let at_three = serde_json::json!({ "note": "asked", "at": { "quote": "ask 2" } });
    agent_marks(&mut app, "tu_2", at_three);
    let page = app.marks_lines(200).plain_text();
    let rows: Vec<&str> = page.lines().collect();
    let handle = app.conversation().episode().handle();
    assert_eq!(
        rows[0],
        format!("Session b{} {handle}", app.buffer().id.number())
    );
    assert!(
        rows[1].starts_with("mark  turn   line   col  word"),
        "{page}"
    );
    let at = |row: &str, n| row.chars().nth(n);
    assert_eq!(
        at(rows[2], 6),
        Some('2'),
        "the turn under its header: {page}"
    );
    let cell = |row: &str| {
        row.split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let b = cell(rows[2]);
    assert_eq!(
        b[..5],
        ["'b", "2", &(two + 1).to_string(), "0", ">"],
        "{page}"
    );
    let a = cell(rows[3]);
    assert_eq!(
        a[..5],
        ["'a", "4", &(four + 1).to_string(), "2", "ask"],
        "its WORD: {page}"
    );
    assert!(rows[4].starts_with('─'), "a soft rule between: {page}");
    assert!(
        rows[5].starts_with("`b    3"),
        "turn order, not letters: {page}"
    );
    assert!(
        rows[5].contains("asked · > ask 2"),
        "the note first: {page}"
    );
    assert!(rows[6].starts_with("`a    7"), "{page}");
    assert_eq!(rows.len(), 7, "one row a mark: {page}");
}

/// **The columns line up by cells** — a WORD of wide characters, two cells
/// each, padded and measured as it is drawn: every row's context under the
/// header's, the word whole (review stop 14, B F7: padded by chars, a wide
/// word pushed its row's context off the column).
#[tokio::test]
async fn marks_columns_line_up_by_cells() {
    let (mut app, pane) = transcript(1);
    app.output_mut().begin_turn("wide");
    app.output_mut().append_chunk("日本語のことば ok");
    app.sync_chat();
    let at = line_reading(&app, pane, "日本語のことば ok");
    type_keys_sized(&mut app, SIZE, &format!("gg{at}jma"));
    let echo = line_reading(&app, pane, "> wide");
    type_keys_sized(&mut app, SIZE, &format!("gg{echo}jmb"));
    let page = app.marks_lines(120).plain_text();
    let header = (page.lines()).find(|r| r.starts_with("mark")).unwrap();
    let column = crate::wrap::str_cells(&header[..header.find("context").unwrap()]);
    for (name, context) in [('a', "日本語のことば ok"), ('b', "> wide")] {
        let row = (page.lines())
            .find(|r| r.starts_with(&format!("'{name}")))
            .unwrap();
        let at = crate::wrap::str_cells(&row[..row.rfind(context).unwrap()]);
        assert_eq!(at, column, "`{name}`'s context under the header's: {page}");
    }
    assert!(
        page.contains("  日本語のことば  "),
        "the wide WORD whole: {page}"
    );
}

/// **A row is cut to its window, never wrapped** — measured in cells, at
/// every width, the fixed columns too; the context only if there is room
/// ("if we have space, might as well use it"), its cut said.
#[tokio::test]
async fn marks_rows_are_cut_to_the_window() {
    let (mut app, pane) = transcript(3);
    // A reply line far wider than any window: what a story's paragraph is;
    // a WORD longer than its column; a line of wide characters, two cells
    // each (review stop 14, B F7 / C 7: measured in chars, with a 4-char
    // word, the test sat on the overflow — rows wrapped below ~49 columns,
    // and a CJK line drew 131 cells in 80).
    let long = "word ".repeat(60);
    let path = "/a/very/long/path/that/is/one/word.rs";
    let wide = "日本語の行は二つのセルを使う".repeat(4);
    app.output_mut().begin_turn("long");
    let reply = format!("{}\n{path}\n{wide}", long.trim_end());
    app.output_mut().append_chunk(&reply);
    app.sync_chat();
    for (text, name) in [(long.trim_end(), 'a'), (path, 'b'), (wide.as_str(), 'c')] {
        let at = line_reading(&app, pane, text);
        type_keys_sized(&mut app, SIZE, &format!("gg{at}jm{name}"));
    }
    for width in 1..=120 {
        for line in app.marks_lines(width).lines() {
            assert!(line.cells() <= width, "{width}: {:?}", line.text());
        }
    }
    for width in [40, 80, 120] {
        let page = app.marks_lines(width).plain_text();
        let row = (page.lines())
            .find(|r| r.starts_with("'a"))
            .expect("its row");
        assert!(row.ends_with('…'), "the context cut, said: {row:?}");
    }
    let page = app.marks_lines(120).plain_text();
    let row = (page.lines()).find(|r| r.starts_with("'b")).unwrap();
    assert!(
        row.contains("/a/very/long/path/that/…"),
        "the word cut to its column: {row:?}"
    );
    let narrow = app.marks_lines(28).plain_text();
    assert!(
        !narrow.contains("word word"),
        "no room, no context: {narrow}"
    );
}

/// **What the eye wants stands out** — on the rendered page: the session in
/// `ui.secret`, every mark in `ui.tool`, the word bold, the context faint.
#[tokio::test]
async fn marks_draws_what_matters_to_stand_out() {
    use crate::style::Ink;
    use ratatui::style::Modifier;
    let (mut app, _) = transcript(3);
    // On `ask`, so the first `ask` drawn is the word, not the context's.
    type_keys_sized(&mut app, SIZE, "gg7j2lma");
    agents_turn(&mut app, "Done.");
    agent_marks(&mut app, "tu_1", serde_json::json!({ "note": "the plan" }));
    app.dispatch_command_for_test("marks");
    let buf = frame_of(&mut app, 120, 30);
    let style = |needle: &str| {
        let at = cells_of(&buf, needle)
            .next()
            .unwrap_or_else(|| panic!("{needle} drawn"));
        buf[at].style()
    };
    let ink = |i| app.theme.ui(i).fg;
    assert!(
        ink(Ink::Secret).is_some(),
        "premise: the theme paints `ui.secret`"
    );
    assert_ne!(
        ink(Ink::Tool),
        ink(Ink::Text),
        "premise: `ui.tool` is a colour of its own"
    );
    assert_eq!(style("Session").fg, ink(Ink::Secret), "the session");
    // Steven, 10-30: "All marks should share the same color", "I still
    // wanted the marks colored".
    assert_eq!(style("`a"), style("'a"), "every mark alike");
    assert_eq!(style("'a").fg, ink(Ink::Tool), "coloured");
    assert!(
        style("'a").add_modifier.contains(Modifier::BOLD),
        "and bold"
    );
    assert!(
        style("ask").add_modifier.contains(Modifier::BOLD),
        "the word"
    );
    assert_eq!(style("the plan ·").fg, ink(Ink::TextFaint), "the context");
}

/// **`<CR>` on a mark goes to it** — its session in this pane, as `:b N`
/// shows it, the cursor on the mark; on a line naming no mark, said.
#[tokio::test]
async fn enter_on_a_mark_goes_to_it() {
    let (mut app, pane) = transcript(10);
    type_keys_sized(&mut app, SIZE, "gg7j3lma");
    let at = app.pane_chat(pane).cursor();
    // Away from it first: the transcript under the page keeps its cursor,
    // so one left on the mark "landed" with no landing done (mutated).
    type_keys_sized(&mut app, SIZE, "G");
    assert_ne!(app.pane_chat(pane).cursor(), at, "premise: away from it");
    app.dispatch_command_for_test("marks");
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Marks),
        "premise: the page"
    );
    draw_at(&mut app, SIZE.0, SIZE.1);
    type_keys_sized(&mut app, SIZE, "gg<CR>");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("no mark on this line"), "the heading: {said}");
    type_keys_sized(&mut app, SIZE, "G<CR>");
    assert_eq!(app.focus(), PaneKind::Output, "{:?}", app.notice());
    let here = app.workspace.focused();
    assert_eq!(app.pane_session(here).id, app.pane_session(pane).id);
    assert_eq!(app.pane_chat(here).cursor(), at, "{:?}", app.notice());
}

/// **A mark that is not set is not listed**, and with none the page says
/// how one is made, rather than nothing.
#[tokio::test]
async fn marks_lists_only_what_is_set_and_says_how_when_none() {
    let (mut app, pane) = transcript(3);
    let page = app.marks_lines(200).plain_text();
    assert!(page.starts_with("no marks — `m{a-z}` sets one"), "{page}");
    app.output_mut().begin_turn("never sent");
    app.sync_chat();
    let echo = line_reading(&app, pane, "> never sent");
    type_keys_sized(&mut app, SIZE, &format!("gg{echo}jma"));
    assert!(
        app.marks_lines(200).plain_text().contains("'a"),
        "premise: listed"
    );
    assert!(
        app.output_mut().abandon_turn("you stopped it"),
        "premise: undone"
    );
    let page = app.marks_lines(200).plain_text();
    assert!(page.starts_with("no marks"), "gone with its turn: {page}");
}

// ---- guards held by a test (review stop 14, A 7 / C 2-5, C 9) -----------------

/// **Nothing to mark is said** — `m` in a transcript with no line yet
/// (review stop 14, C 5: the refusal was held by nothing).
#[test]
fn m_with_nothing_to_mark_says_so() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    type_keys_sized(&mut app, SIZE, "ma");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("nothing here to mark yet"), "{said}");
    assert!(app.buffer().marks.is_empty());
}

/// **`<CR>` on a row whose mark went after the page was drawn says so** —
/// and moves nothing (review stop 14, A 7 / C 5): without the check, the
/// motion found no name in the copy's table and did nothing, silently.
#[test]
fn going_to_a_mark_gone_since_the_page_says_so() {
    let (mut app, pane) = transcript(3);
    app.output_mut().begin_turn("gone soon");
    app.sync_chat();
    let echo = line_reading(&app, pane, "> gone soon");
    type_keys_sized(&mut app, SIZE, &format!("gg{echo}jma"));
    type_keys_sized(&mut app, SIZE, "gg");
    assert!(
        app.output_mut().abandon_turn("you stopped it"),
        "premise: gone"
    );
    let buffer = app.pane_session(pane).id;
    app.notices.clear();
    app.go_to_mark(buffer, crate::keymap::Whose::Yours, 'a');
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("mark `a` is not set"), "{said}");
    assert_eq!(app.pane_chat(pane).cursor().0, 0, "nothing moved");
}

/// **`<CR>` on a mark of a session another pane shows shows it here, and
/// goes** — as `:b N` would, the page's pane standing over another session
/// (review stop 14, A 7 / C 5) — and `:marks` lists the two sessions apart,
/// a blank row between (MA10, held by nothing: every test had one session).
/// `go_to_mark`'s "shown in another pane" refusal is for a pane that cannot
/// host, which the page's never is: not reached from `<CR>` (traced 10-31).
#[test]
fn going_to_a_mark_of_another_session_shows_it_here() {
    let (mut app, pane) = transcript(3);
    type_keys_sized(&mut app, SIZE, "gg7jma");
    let a = app.pane_session(pane).id;
    app.dispatch_command_for_test("vsplit");
    app.dispatch_command_for_test("new");
    let other = app.workspace.focused();
    assert_ne!(
        app.pane_session(other).id,
        a,
        "premise: another session here"
    );
    app.output_mut().begin_turn("b's own");
    app.sync_chat();
    type_keys_sized(&mut app, SIZE, "ggjmb");
    app.dispatch_command_for_test("marks");
    let page = app.marks_lines(120).plain_text();
    let lines: Vec<&str> = page.lines().collect();
    let second = (lines.iter())
        .rposition(|l| l.starts_with("Session b"))
        .expect("two sessions");
    assert!(
        second > 0 && lines[second - 1].is_empty(),
        "a blank between: {page}"
    );
    app.notices.clear();
    app.go_to_mark(a, crate::keymap::Whose::Yours, 'a');
    let now = app.workspace.focused();
    assert_eq!(now, other, "in the pane you stand in: {:?}", app.notice());
    assert_eq!(app.pane_session(now).id, a, "its session shown here");
    let marked = line_reading(&app, now, "> ask 1");
    assert_eq!(app.pane_chat(now).cursor().0, marked, "and on the mark");
}

/// **A child's mark lands in the child** — offered to every unattended
/// reply (`ToolDef::unattended_with`), answered at once, and the job goes
/// on (review stop 14, C 2: the job path's caller was held by nothing —
/// every mark test used the foreground stream).
#[tokio::test]
async fn a_childs_mark_lands_in_the_child_and_it_goes_on() {
    let mut app = test_app();
    app.submit("open a turn so the child has something to hang off");
    app.feed_stream(StreamMsg::Complete);
    let parent = app.current;
    let child = app.spawn_child_job("go", None).expect("spawned");
    app.feed_job(child, StreamMsg::Chunk("Here is the plan.".into()));
    app.feed_job(
        child,
        StreamMsg::ToolUse(shannon_core::Block::ToolUse {
            id: "tu_c".into(),
            name: shannon_core::MARK_TOOL.into(),
            input: serde_json::json!({ "note": "the plan" }),
        }),
    );
    app.feed_job(child, StreamMsg::Complete);
    let (said, error) = answer_to(&app, "tu_c").expect("answered");
    assert!(!error && said.contains("mark `a` set here."), "{said}");
    let session = (app.buffers.sessions())
        .find(|b| b.conversation.episode().id == child)
        .expect("the child's session");
    assert!(session.agent_marks.contains_key(&'a'), "the child's");
    let parents = &app.session_of(parent).unwrap().agent_marks;
    assert!(parents.is_empty(), "not the parent's: {parents:?}");
    // Answered at once, so the round went on — here to its end, the stub
    // answering the next request with nothing — never held for a person.
    let said = app.notice().unwrap_or_default();
    assert!(!said.contains("held"), "not held: {said}");
    let answered_in_child = (session.conversation.messages().iter())
        .flat_map(|m| &m.blocks)
        .any(|b| matches!(b, shannon_core::Block::ToolResult { tool_use_id, .. } if tool_use_id == "tu_c"));
    assert!(answered_in_child, "its round went on with the result");
}

/// **A mark that cannot be saved says so, and stays set** — yours and the
/// agent's, over a store that refuses the header (review stop 14, C 3: on a
/// disk failure the user saw "mark `a` — turn 3", a success).
#[tokio::test]
async fn a_mark_not_saved_says_so_and_stays_set() {
    use shannon_core::store::FailingStore;
    let (store, refusing) = FailingStore::refusing_headers();
    let mut app = test_app_with_store(test_backend(), Arc::new(Config::default()), Box::new(store));
    answered(&mut app, "first", "reply one");
    refusing.store(true, std::sync::atomic::Ordering::SeqCst);
    let pane = cursor_on(&mut app, "reply one");
    app.notices.clear();
    type_keys_sized(&mut app, SIZE, "ma");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("mark `a` set, but not saved with the session"),
        "{said}"
    );
    type_keys_sized(&mut app, SIZE, "gg'a");
    let line = line_reading(&app, pane, "reply one");
    assert_eq!(app.pane_chat(pane).cursor().0, line, "set, for this run");
    agents_turn(&mut app, "Marking.");
    app.notices.clear();
    agent_marks(&mut app, "tu_s", serde_json::json!({ "note": "here" }));
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("the agent's mark `a` is not saved with the session"),
        "{said}"
    );
    assert!(
        app.buffer().agent_marks.contains_key(&'a'),
        "set, for this run"
    );
}

/// **Past 26 the oldest goes, by when it was made — through a reopen too**
/// (review stop 14, C 4): the 27th takes `a`, the 28th `b`; saved and
/// reopened, the next takes `c`, the oldest left, not `a`, the first letter.
/// At the 27th the two orders agree, which is all the earlier test reached.
#[tokio::test]
async fn past_26_the_oldest_goes_through_a_reopen_too() {
    let dir = TempDir::new().unwrap();
    {
        let mut app = over_store(dir.path());
        app.submit("mark a lot");
        for i in 0..28 {
            agent_marks(
                &mut app,
                &format!("tu_{i}"),
                serde_json::json!({ "note": format!("place {i}") }),
            );
        }
        let named = |call: &str| answer_to(&app, call).expect("answered").0;
        assert!(named("tu_26").contains("mark `a`"), "{}", named("tu_26"));
        assert!(named("tu_27").contains("mark `b`"), "{}", named("tu_27"));
        app.feed_stream(StreamMsg::Chunk("Done.".into()));
        app.feed_stream(StreamMsg::Complete);
    }
    let (mut app, _) = reopened(dir.path());
    assert_eq!(app.buffer().agent_marks.len(), 26, "premise: all 26 kept");
    app.submit("one more");
    agent_marks(&mut app, "tu_more", serde_json::json!({ "note": "more" }));
    let said = answer_to(&app, "tu_more").expect("answered").0;
    assert!(said.contains("mark `c`"), "the oldest by made: {said}");
}

/// **A mark set inside a WORD shows the whole WORD** — vim's WORD back to
/// its first non-blank (review stop 14, C 9: every test marked a word's
/// start, so the backward step was held by nothing).
#[test]
fn a_mark_inside_a_word_shows_the_whole_word() {
    let (mut app, pane) = transcript(3);
    type_keys_sized(&mut app, SIZE, "gg7j3lma");
    assert_eq!(
        app.pane_chat(pane).cursor(),
        (7, 3),
        "premise: inside `ask`"
    );
    let page = app.marks_lines(120).plain_text();
    let row = (page.lines()).find(|r| r.starts_with("'a")).unwrap();
    assert!(row.contains("  ask "), "the whole WORD: {row:?}");
}

// ---- the clone principle (S16-Q2): what a session made from another keeps -----

/// Three answered turns over the store: `ma` on the first reply, the
/// agent's mark here in the second turn, `mb` on the third reply. Their
/// session's episode.
fn three_marked(app: &mut App) -> shannon_core::EpisodeId {
    answered(app, "first", "reply one");
    app.submit("second");
    app.feed_stream(StreamMsg::Chunk("reply two".into()));
    agent_marks(app, "tu_two", serde_json::json!({ "note": "two" }));
    app.feed_stream(StreamMsg::Complete);
    answered(app, "third", "reply three");
    cursor_on(app, "reply one");
    type_keys_sized(app, SIZE, "ma");
    cursor_on(app, "reply three");
    type_keys_sized(app, SIZE, "mb");
    app.conversation().episode().id
}

/// The line yours `name` stands on in the session of `episode`, if set.
fn yours_at(app: &App, episode: shannon_core::EpisodeId, name: char) -> Option<String> {
    let session = (app.buffers.sessions()).find(|b| b.conversation.episode().id == episode)?;
    let (line, _) = session.output.locate(*session.marks.get(&name)?)?;
    Some(session.output.lines()[line].clone())
}

/// **A clone carries the marks, and then they diverge** (Steven, 10-30,
/// S16-Q2: "share all the same history and memories up to the point of
/// cloning… at that point in time they diverge"; review stop 14, A 5: a
/// clone, a fork and a `:vfork` started with none) — yours and the agent's,
/// each on its text; one set in either after is that one's alone.
#[tokio::test]
async fn a_clone_carries_the_marks_and_then_they_diverge() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    app.clone_episode(original);
    let clone = app.conversation().episode().id;
    assert_ne!(clone, original, "premise: in the clone: {:?}", app.notice());
    assert_eq!(yours_at(&app, clone, 'a').as_deref(), Some("reply one"));
    assert_eq!(yours_at(&app, clone, 'b').as_deref(), Some("reply three"));
    assert!(
        app.buffer().agent_marks.contains_key(&'a'),
        "and the agent's"
    );
    cursor_on(&mut app, "reply two");
    type_keys_sized(&mut app, SIZE, "mc");
    assert_eq!(yours_at(&app, clone, 'c').as_deref(), Some("reply two"));
    assert_eq!(yours_at(&app, original, 'c'), None, "the clone's alone");
}

/// The text of the line the focused transcript's cursor is on — after a
/// session is made from the one it read, where the window now reads.
fn reading(app: &mut App) -> String {
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let chat = app.pane_chat(app.workspace.focused());
    chat.line_text(chat.cursor().0).unwrap_or_default()
}

/// **A clone starts where you were reading** (S21-Q4, Steven 11-01:
/// "Carry over whole place by position") — the window that showed the
/// original now shows the clone, its cursor on the same text, and what it
/// opened still open.
#[tokio::test]
async fn a_clone_starts_where_you_were_reading() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    let pane = cursor_on(&mut app, "reply two");
    type_keys_sized(&mut app, SIZE, "zR");
    app.clone_episode(original);
    assert_ne!(
        app.conversation().episode().id,
        original,
        "premise: the clone"
    );
    assert_eq!(app.workspace.focused(), pane, "premise: in the same window");
    assert_eq!(reading(&mut app), "reply two");
    let output = &app.pane_session(pane).output;
    assert_eq!(
        output.shut_lines(&app.pane_shut(pane), None),
        Vec::new(),
        "zR's, kept"
    );
}

/// **A drawn window keeps the line at its top**, by position as its
/// cursor (vim's `winsaveview` keeps `topline`): scrolled back with `zt`
/// (its line `scrolloff` below the top, as vim's), a clone, and the same
/// line at the clone window's top — not its end, the entry's.
#[tokio::test]
async fn a_clone_keeps_the_line_at_the_windows_top() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    // Room below it, so its top is not the end's.
    for i in 0..4 {
        answered(&mut app, &format!("more {i}"), &format!("more reply {i}"));
    }
    let pane = cursor_on(&mut app, "reply two");
    frame_of(&mut app, SIZE.0, SIZE.1);
    type_keys_sized(&mut app, SIZE, "zt");
    let top = |app: &App| {
        let (w, h) = app.pane_content_size(pane, Rect::new(0, 0, SIZE.0, SIZE.1));
        let output = &app.pane_session(pane).output;
        let shut = app.pane_shut(pane);
        let row = (output.top_row(&app.pane_viewport(pane), &shut, w, usize::from(h))).0;
        let line = output.visual_to_logical(&shut, row, 0, w).0;
        (line, output.lines()[line].clone())
    };
    let was = top(&app);
    assert!(
        !app.pane_viewport(pane).is_following_tail() && was.0 > 0,
        "premise: scrolled back, {was:?}"
    );
    app.clone_episode(original);
    assert_ne!(
        app.conversation().episode().id,
        original,
        "premise: the clone"
    );
    assert_eq!(top(&app), was);
}

/// The turn of `episode` `:graph` opened on, if it opened on one of its.
fn graph_turn_of(app: &App, episode: shannon_core::EpisodeId) -> Option<usize> {
    match app.graph_view().and_then(|v| v.selected_node()) {
        Some(shannon_core::Node::Turn {
            episode: of, index, ..
        }) if of == episode => Some(index),
        _ => None,
    }
}

/// **`:graph` opens on the turn you were reading** (Steven, 11-01, image
/// 11: the cursor on turn 2, the graph on turn 3) — the turn under the
/// cursor, whether the window follows its end or not: a transcript that
/// fits its window always follows its end, and an arm here asserted turn 3
/// with the cursor on turn 1 (review stop 15, C H3). A window that never
/// moved is at its end, its cursor with it: the last turn.
#[tokio::test]
async fn graph_opens_on_the_turn_you_were_reading() {
    for (case, want) in [("following", 0), ("scrolled back", 0), ("never moved", 2)] {
        let dir = TempDir::new().unwrap();
        let mut app = over_store(dir.path());
        let original = three_marked(&mut app);
        let pane = cursor_on(&mut app, "reply one");
        let view = app.window_of_mut(pane).unwrap().session_view_mut().unwrap();
        match case {
            "scrolled back" => view.viewport.freeze_at(0, 0),
            // A window that never showed it: the tree's pane, then `:b N`.
            "never moved" => {
                let number = app.pane_session(pane).id.number();
                app.dispatch_command_for_test("sgraph");
                app.dispatch_command_for_test(&format!("b {number}"));
                let here = app.workspace.focused();
                assert_ne!(here, pane, "premise: another window");
                assert_eq!(app.kind_of(here), Some(PaneKind::Output), "premise");
            }
            _ => {}
        }
        if case == "following" {
            assert!(app.pane_viewport(pane).is_following_tail(), "premise");
        }
        app.dispatch_command_for_test("graph");
        assert_eq!(graph_turn_of(&app, original), Some(want), "{case}");
    }
}

/// **…and on a turn a branch shares, the row that draws it** — under the
/// session that owns it: a checkout read at its end, on the turn it was
/// checked out at, opened the tree on the checkout's own row (Steven,
/// 11-01, image 15; review stop 15, A M7 / C H3).
#[tokio::test]
async fn graph_opens_on_a_shared_turn_where_it_is_drawn() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    let turn = app.conversation().episode().own_turns()[0];
    cursor_on(&mut app, "reply one");
    app.checkout_turn(original, turn, 0);
    assert_ne!(
        app.conversation().episode().id,
        original,
        "premise: a branch"
    );
    app.dispatch_command_for_test("graph");
    assert_eq!(graph_turn_of(&app, original), Some(0));
}

/// **A checkout lands on the turn you picked** — at its end, not where the
/// window was reading (S21-Q5, Steven 11-01: "it is correct to land on the
/// turn you are in"): picking a turn in `:graph` is picking the place.
#[tokio::test]
async fn a_checkout_lands_on_the_turn_picked() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    let turn = app.conversation().episode().own_turns()[1];
    let pane = cursor_on(&mut app, "reply one");
    let view = app.window_of_mut(pane).unwrap().session_view_mut().unwrap();
    view.viewport.freeze_at(1, 0);
    app.checkout_turn(original, turn, 1);
    assert_ne!(app.conversation().episode().id, original, "premise");
    assert_eq!(app.workspace.focused(), pane, "premise: in that window");
    assert!(app.pane_viewport(pane).is_following_tail(), "at its end");
    let last = app.pane_session(pane).output.lines().len() - 1;
    assert_eq!(app.pane_chat(pane).cursor().0, last, "on its last line");
    assert_ne!(reading(&mut app), "reply one", "not where you were reading");
}

/// **A checkout of the session's LAST turn lands at its end too** — the
/// session itself, its window put at its end, not left where it was reading
/// (Steven, 11-01, image 12: `<CR>` on the tip in `:graph` showed the
/// session on turn 2, while turns 1 and 2 landed at their ends). One verb,
/// one landing, whichever turn is picked.
#[tokio::test]
async fn a_checkout_of_the_last_turn_lands_at_its_end() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    // Room below it, so its end is not where you were reading.
    for i in 0..4 {
        answered(&mut app, &format!("more {i}"), &format!("more reply {i}"));
    }
    let tip = app.conversation().episode().own_turns().len() - 1;
    let pane = cursor_on(&mut app, "reply one");
    let view = app.window_of_mut(pane).unwrap().session_view_mut().unwrap();
    view.viewport.freeze_at(0, 0);
    app.dispatch_command_for_test("graph");
    let view = app.graph_view_mut().expect("premise: the tree");
    view.reveal_and_select_turn(original, tip);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.conversation().episode().id,
        original,
        "premise: the session itself"
    );
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Output),
        "premise: in that window"
    );
    assert!(app.pane_viewport(pane).is_following_tail(), "at its end");
    let last = app.pane_session(pane).output.lines().len() - 1;
    assert_eq!(app.pane_chat(pane).cursor().0, last, "on its last line");
}

/// **An edit's branch starts where you were reading** — on a turn after
/// the edit, moved where its suffix went.
#[tokio::test]
async fn an_edit_starts_where_you_were_reading() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    cursor_on(&mut app, "reply three");
    let edited = to_editable(&[
        Message::user("second, again"),
        Message::assistant("reply 2"),
        Message::user("and more"),
        Message::assistant("reply 2b"),
    ]);
    app.commit_slice_edit(original, 1, 2, &edited)
        .expect("composed");
    assert_ne!(app.conversation().episode().id, original, "premise");
    assert_eq!(reading(&mut app), "reply three");
}

/// **A window that set the session aside finds its place carried too** —
/// a compaction lands while the window shows another session, and `<C-^>`
/// back finds it where it was reading, not at the entry.
#[tokio::test]
async fn a_compaction_carries_a_place_set_aside() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    cursor_on(&mut app, "reply three");
    app.dispatch_command_for_test("compact --keep 1");
    app.dispatch_command_for_test("new");
    assert!(
        reading(&mut app).is_empty(),
        "premise: another session here"
    );
    for msg in [
        StreamMsg::Chunk("what was done".into()),
        StreamMsg::Complete,
    ] {
        app.feed_summary(msg);
    }
    press_ctrl_hat(&mut app, false);
    assert_ne!(
        app.conversation().episode().id,
        original,
        "premise: composed"
    );
    assert_eq!(reading(&mut app), "reply three");
}

/// **A carried view keeps its cursor on screen** — an edit that adds turns
/// between the line at a window's top and its cursor's would leave the
/// cursor below it, and a window keeps its view, not its cursor; so the
/// view moves, to the cursor and no further.
#[tokio::test]
async fn an_edit_that_grows_a_view_keeps_its_cursor_on_screen() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    let pane = cursor_on(&mut app, "reply three");
    let size = (80, 40);
    frame_of(&mut app, size.0, size.1);
    let one = line_reading(&app, pane, "reply one");
    let view = app.window_of_mut(pane).unwrap().session_view_mut().unwrap();
    view.viewport.freeze_at(one, 0);
    let (w, h) = app.pane_content_size(pane, Rect::new(0, 0, size.0, size.1));
    let seen = |app: &App| {
        let output = &app.pane_session(pane).output;
        let shut = app.pane_shut(pane);
        let (line, col) = app.pane_chat(pane).cursor();
        let row = output.logical_to_visual(&shut, line, col, w).0;
        let top = (output.top_row(&app.pane_viewport(pane), &shut, w, usize::from(h))).0;
        (top..top + usize::from(h)).contains(&row)
    };
    assert!(seen(&app), "premise: on screen");
    let five: Vec<Message> = (0..5)
        .flat_map(|i| {
            [
                Message::user(format!("again {i}")),
                Message::assistant("so"),
            ]
        })
        .collect();
    app.commit_slice_edit(original, 1, 2, &to_editable(&five))
        .expect("composed");
    assert_eq!(reading(&mut app), "reply three", "premise: carried");
    assert!(seen(&app), "on screen after it grew");
}

/// **A compaction lands over the session, every window on it where it was**
/// — the turn it keeps moved up under the summary.
#[tokio::test]
async fn a_compaction_keeps_each_window_where_it_was() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    cursor_on(&mut app, "reply three");
    app.dispatch_command_for_test("compact --keep 1");
    for msg in [
        StreamMsg::Chunk("what was done".into()),
        StreamMsg::Complete,
    ] {
        app.feed_summary(msg);
    }
    assert_ne!(
        app.conversation().episode().id,
        original,
        "premise: composed"
    );
    assert_eq!(reading(&mut app), "reply three");
}

/// **A checkout keeps the marks on the turns it shares** — not one past
/// the turn checked out, which is not in its history and not missed.
#[tokio::test]
async fn a_checkout_keeps_the_marks_on_the_turns_it_shares() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    let turn = app.conversation().episode().own_turns()[1];
    app.notices.clear();
    app.checkout_turn(original, turn, 1);
    let branch = app.conversation().episode().id;
    assert_ne!(branch, original, "premise: on the checkout");
    assert_eq!(yours_at(&app, branch, 'a').as_deref(), Some("reply one"));
    assert!(
        app.buffer().agent_marks.contains_key(&'a'),
        "turn 2's, the agent's"
    );
    assert_eq!(yours_at(&app, branch, 'b'), None, "turn 3 is not in it");
    let said = app.notice().unwrap_or_default();
    assert!(!said.contains("not carried"), "nothing missed: {said}");
}

/// **An edit's branch keeps the marks on the turns it left as they were** —
/// before the edit where they were, after it where its suffix went — and
/// says the ones on the turn it changed.
#[tokio::test]
async fn an_edit_keeps_the_marks_on_the_turns_it_left() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    // The second turn made two: the suffix moves one turn on.
    let edited = to_editable(&[
        Message::user("second, again"),
        Message::assistant("reply 2"),
        Message::user("and more"),
        Message::assistant("reply 2b"),
    ]);
    app.notices.clear();
    let branch = app
        .commit_slice_edit(original, 1, 2, &edited)
        .expect("composed");
    assert_eq!(yours_at(&app, branch, 'a').as_deref(), Some("reply one"));
    assert_eq!(
        yours_at(&app, branch, 'b').as_deref(),
        Some("reply three"),
        "the suffix's, moved"
    );
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("not carried into the edit — the agent's mark `a`: their turns changed"),
        "{said}"
    );
}

/// **A compaction keeps the marks on the turns it keeps** — moved to where
/// they are after the summary — and says the ones on the turns it folded.
#[tokio::test]
async fn a_compaction_keeps_the_marks_on_the_turns_it_keeps() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    app.dispatch_command_for_test("compact --keep 1");
    app.notices.clear();
    for msg in [
        StreamMsg::Chunk("what was done".into()),
        StreamMsg::Complete,
    ] {
        app.feed_summary(msg);
    }
    let now = app.conversation().episode().id;
    assert_ne!(now, original, "premise: composed: {:?}", app.notice());
    assert_eq!(yours_at(&app, now, 'b').as_deref(), Some("reply three"));
    assert_eq!(yours_at(&app, now, 'a'), None, "folded");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("not carried into it — mark `a`, the agent's mark `a`"),
        "{said}"
    );
}

/// **`:continue` keeps every mark** — its turns are its turns, the one it
/// continued grown at its end (S16-Q2; review stop 14, A 5: every mark
/// gone, live and saved, unsaid) — and reopened, they stand.
#[tokio::test]
async fn continue_keeps_every_mark() {
    use shannon_core::{ContinueStrategy, StopReason};
    let dir = TempDir::new().unwrap();
    let mut cfg = Config::default();
    cfg.continue_.strategy = ContinueStrategy::Prefill;
    {
        let mut app = test_app_with_store(
            test_backend(),
            Arc::new(cfg),
            Box::new(JsonlStore::new(dir.path()).unwrap()),
        );
        answered(&mut app, "first", "reply one");
        app.submit("write a poem");
        app.feed_stream(StreamMsg::Chunk("Roses are".into()));
        app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
        app.feed_stream(StreamMsg::Complete);
        cursor_on(&mut app, "reply one");
        type_keys_sized(&mut app, SIZE, "ma");
        cursor_on(&mut app, "Roses are");
        type_keys_sized(&mut app, SIZE, "mb");
        let original = app.conversation().episode().id;
        app.continue_last_turn();
        app.feed_continuation(StreamMsg::Chunk(" red".into()));
        app.feed_continuation(StreamMsg::Stopped(StopReason::EndTurn));
        app.feed_continuation(StreamMsg::Complete);
        let now = app.conversation().episode().id;
        assert_ne!(now, original, "premise: composed: {:?}", app.notice());
        assert_eq!(yours_at(&app, now, 'a').as_deref(), Some("reply one"));
        assert_eq!(yours_at(&app, now, 'b').as_deref(), Some("Roses are red"));
    }
    let (app, _) = reopened(dir.path());
    let now = app.conversation().episode().id;
    assert_eq!(
        yours_at(&app, now, 'b').as_deref(),
        Some("Roses are red"),
        "saved"
    );
}

/// **A squash whose summary is several turns moves a later mark by its
/// length** (the close's verifier, V 1: counted as one turn, `'b` after a
/// two-turn summary landed on the turn before its own, saved so, unsaid).
#[tokio::test]
async fn a_squash_into_several_turns_moves_later_marks_by_its_length() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    let ids = app.current_graph().full_turns(&original);
    let target = SquashTarget::Range {
        episode: original,
        start: 0,
        end: 1,
        fork_turn: ids[0],
        config: app.conversation().episode().config.clone(),
    };
    let summary = to_editable(&[
        Message::user("the gist"),
        Message::assistant("part one"),
        Message::user("and"),
        Message::assistant("part two"),
    ]);
    let graph = app.current_graph();
    let now = (app.commit_summary_into(app.current, graph, target, &ids[0..1], &summary, None))
        .expect("composed");
    assert_eq!(
        yours_at(&app, now, 'b').as_deref(),
        Some("reply three"),
        "after a two-turn summary"
    );
}

/// **A mark on a turn still running is not carried, and is said** (the
/// close's verifier, V 4): an edit while a turn runs carried its mark past
/// the branch's end, where the next turn took the index and a reopen put it
/// on that turn's words.
#[tokio::test]
async fn a_mark_on_a_running_turn_is_not_carried_and_is_said() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    let original = three_marked(&mut app);
    app.submit("fourth");
    cursor_on(&mut app, "> fourth");
    type_keys_sized(&mut app, SIZE, "mc");
    assert!(app.conversation().turn_in_progress(), "premise: running");
    let edited = to_editable(&[
        Message::user("second, again"),
        Message::assistant("reply 2"),
    ]);
    app.notices.clear();
    let branch = (app.commit_slice_edit(original, 1, 2, &edited)).expect("composed");
    assert_eq!(yours_at(&app, branch, 'c'), None, "not carried");
    let kept = (app.buffers.sessions())
        .find(|b| b.conversation.episode().id == branch)
        .map(|b| b.unplaced_marks.len());
    assert_eq!(kept, Some(0), "nor kept to land on another turn");
    let said = app.notice().unwrap_or_default();
    assert!(
        said.contains("mark `c`: their turn is still running"),
        "{said}"
    );
}
