//! **Thinking** (N1, `docs/DesignDocs/navigation.md` §4) — what a turn keeps
//! of it, what is said when the API drops some, and how the transcript draws
//! it (N1b).

use super::position_marks::{SIZE, drawn, line_reading, over_store, reopened};
use super::*;
use crate::output::BlockRole;

pub(super) fn thought(text: &str) -> StreamMsg {
    StreamMsg::Thought(shannon_core::Block::Thinking {
        thinking: text.into(),
        signature: "S".into(),
        at: 0,
    })
}

/// A thought as the API streams one: its text in `chunks`, then the block
/// whole.
pub(super) fn think(app: &mut App, chunks: &[&str]) {
    for chunk in chunks {
        app.feed_stream(StreamMsg::ThinkingChunk((*chunk).into()));
    }
    app.feed_stream(thought(&chunks.concat()));
}

/// The reply's blocks of the latest turn — its text and its thinking — each
/// role with its lines.
fn reply(app: &App) -> Vec<(BlockRole, Vec<String>)> {
    let turn = app.output().last_turn();
    (app.output().spans())
        .filter(|s| s.turn == turn && s.role.the_models())
        .map(|s| (s.role, s.lines.to_vec()))
        .collect()
}

/// The lines of each thinking block in the transcript.
fn thoughts(app: &App) -> Vec<Vec<String>> {
    (app.output().spans())
        .filter(|s| s.role == BlockRole::Thinking)
        .map(|s| s.lines.to_vec())
        .collect()
}

fn lines(of: &[&str]) -> Vec<String> {
    of.iter().map(|l| l.to_string()).collect()
}

/// **A thought is drawn where it was written** (N1b): a block of its own as
/// it streams, the reply's text so far above it and its next words below —
/// each closed with the blank line a record carries.
#[tokio::test]
async fn a_thought_is_drawn_where_it_was_written() {
    let mut app = test_app();
    app.submit("look");
    think(&mut app, &["pl", "an"]);
    app.feed_stream(StreamMsg::Chunk("Done".into()));
    think(&mut app, &["checked"]);
    app.feed_stream(StreamMsg::Chunk(".".into()));
    app.feed_stream(StreamMsg::Complete);
    use BlockRole::{Assistant, Thinking};
    assert_eq!(
        reply(&app),
        [
            (Thinking, lines(&["plan", ""])),
            (Assistant, lines(&["Done", ""])),
            (Thinking, lines(&["checked", ""])),
            (Assistant, lines(&[".", ""])),
        ]
    );
}

/// **A reopened turn draws its thinking as it drew it live**, and a mark in
/// a thought is kept and lands on the same text: a place is a block by its
/// role and which of that role (`output::Position`), so a replay drawing a
/// thought more or less would move every place after it.
#[tokio::test]
async fn a_reopened_turn_draws_its_thinking_as_it_drew_live() {
    let dir = TempDir::new().unwrap();
    let place = |app: &App, anchor| {
        let (line, col) = app.output().locate(anchor).expect("set");
        (app.output().lines()[line].clone(), col)
    };
    let (live, marked) = {
        let mut app = over_store(dir.path());
        app.submit("look");
        think(&mut app, &["Plan", "ning it"]);
        app.feed_stream(StreamMsg::Chunk("Done".into()));
        think(&mut app, &["Checked"]);
        app.feed_stream(StreamMsg::Chunk(".".into()));
        app.feed_stream(StreamMsg::Complete);
        app.set_focus(PaneKind::Output);
        app.sync_chat();
        let pane = app.workspace.focused();
        let line = line_reading(&app, pane, "Checked");
        type_keys_in(
            &mut app,
            Rect::new(0, 0, SIZE.0, SIZE.1),
            &format!("gg{line}j2lma"),
        );
        let said = app.notice().unwrap_or_default();
        assert!(
            said.contains("mark `a`") && !said.contains("not saved"),
            "{said}"
        );
        let marked = place(&app, app.buffer().marks[&'a']);
        assert_eq!(marked, ("Checked".to_string(), 2), "premise");
        // And a reply that only thought: its thought the last it drew.
        app.submit("again");
        think(&mut app, &["Nothing to add"]);
        app.feed_stream(StreamMsg::Complete);
        (drawn(&app), marked)
    };
    assert_eq!(
        (live.iter())
            .filter(|(_, role, _)| *role == BlockRole::Thinking)
            .count(),
        3,
        "premise: three thoughts drawn live: {live:?}"
    );
    let (app, _) = reopened(dir.path());
    assert_eq!(drawn(&app), live);
    assert_eq!(place(&app, app.buffer().marks[&'a']), marked);
}

/// **A thought is drawn once, whatever comes first**: one whose block came
/// with nothing streamed is drawn whole then — an empty chunk is nothing
/// streamed — and one that streamed is not drawn again when its block comes
/// after the reply moved on.
#[tokio::test]
async fn a_thought_is_drawn_once_whatever_comes_first() {
    let mut app = test_app();
    app.submit("look");
    app.feed_stream(StreamMsg::ThinkingChunk(String::new()));
    app.feed_stream(thought("unstreamed"));
    app.feed_stream(StreamMsg::ThinkingChunk("hm".into()));
    app.feed_stream(StreamMsg::Chunk("x".into()));
    app.feed_stream(thought("hm"));
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(
        thoughts(&app),
        [lines(&["unstreamed", ""]), lines(&["hm", ""])]
    );
    let kept = app.conversation().messages().last().unwrap().blocks.len();
    assert_eq!(kept, 2, "both kept with the reply");
}

/// **A thought reaches the keys' copy with the frame that draws it** —
/// streamed, and whole: each owes a focused transcript's copy a sync, or
/// its lines were drawn and `j` could not reach them until the reply's
/// next words (review stop 15, C L-e: both held by nothing).
#[tokio::test]
async fn a_thought_reaches_the_keys_with_its_frame() {
    for whole in [false, true] {
        let mut app = test_app();
        app.submit("look");
        app.set_focus(PaneKind::Output);
        frame_of(&mut app, SIZE.0, SIZE.1);
        let msg = match whole {
            false => StreamMsg::ThinkingChunk("weighing it".into()),
            true => thought("weighing it"),
        };
        app.feed_stream(msg);
        frame_of(&mut app, SIZE.0, SIZE.1);
        let chat = app.pane_chat(app.workspace.focused());
        let reached =
            (0..chat.line_count()).any(|l| chat.line_text(l).as_deref() == Some("weighing it"));
        assert!(reached, "whole {whole}");
    }
}

/// **…and one that comes whole after one that streamed is drawn whole** —
/// what streamed is the closed thought's alone (`Generation::thought_shown`,
/// taken as it closes): kept past it, a later block that streamed nothing
/// (a redacted one comes so) was drawn only when the session was reopened,
/// live and replay apart (review stop 15, C L-a: held by nothing).
#[tokio::test]
async fn a_thought_after_a_streamed_one_is_drawn_whole() {
    let mut app = test_app();
    app.submit("look");
    app.feed_stream(StreamMsg::ThinkingChunk("hm".into()));
    app.feed_stream(thought("hm"));
    app.feed_stream(thought("came whole"));
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(
        thoughts(&app),
        [lines(&["hm", ""]), lines(&["came whole", ""])]
    );
}

/// **A thought sent empty draws nothing — not even a break in the reply
/// around it; one sent encrypted says so** — both kept, to go back as they
/// came.
#[tokio::test]
async fn an_empty_thought_draws_nothing_and_an_encrypted_one_says_so() {
    let mut app = test_app();
    app.submit("look");
    app.feed_stream(StreamMsg::Thought(shannon_core::Block::RedactedThinking {
        data: "R".into(),
        at: 0,
    }));
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(thought(""));
    app.feed_stream(StreamMsg::Chunk(" more".into()));
    app.feed_stream(StreamMsg::Complete);
    let drawn = reply(&app);
    assert_eq!(drawn.len(), 2, "{drawn:?}");
    assert_eq!(drawn[0].0, BlockRole::Thinking);
    assert!(drawn[0].1[0].contains("encrypted"), "{drawn:?}");
    assert_eq!(drawn[1], (BlockRole::Assistant, lines(&["ok more", ""])));
    let kept = app.conversation().messages().last().unwrap().blocks.len();
    assert_eq!(kept, 2, "both kept with the reply");
}

/// **…and one of nothing but whitespace draws nothing either**, streamed or
/// whole — kept, as every thought is: one blankness rule, the fold's (no
/// text), where a thought of `" \n \n "` was kept as blank rows mid-turn
/// that no fold could shut (`zc` said "no fold here"; review stop 15, B L3).
#[tokio::test]
async fn a_thought_of_whitespace_draws_nothing() {
    for streamed in [false, true] {
        let mut app = test_app();
        app.submit("look");
        app.feed_stream(StreamMsg::Chunk("ok".into()));
        if streamed {
            app.feed_stream(StreamMsg::ThinkingChunk(" \n \n ".into()));
        }
        app.feed_stream(thought(" \n \n "));
        app.feed_stream(StreamMsg::Chunk(" more".into()));
        app.feed_stream(StreamMsg::Complete);
        let drawn = reply(&app);
        assert_eq!(
            drawn,
            [(BlockRole::Assistant, lines(&["ok more", ""]))],
            "streamed {streamed}"
        );
        // A message's blocks are what is not its text: the thought, kept.
        let kept = app.conversation().messages().last().unwrap().blocks.len();
        assert_eq!(kept, 1, "streamed {streamed}: kept with the reply");
    }
    // What waits is the next text's alone: a thought's leading line lands in
    // front of it, as the block keeps it (and replay draws it); one that met
    // no text does not reach the thought after it.
    let mut app = test_app();
    app.submit("look");
    app.feed_stream(StreamMsg::ThinkingChunk(" \n ".into()));
    app.feed_stream(thought(" \n "));
    app.feed_stream(StreamMsg::ThinkingChunk("\n".into()));
    app.feed_stream(StreamMsg::ThinkingChunk("Weighing".into()));
    app.feed_stream(thought("\nWeighing"));
    app.feed_stream(StreamMsg::Complete);
    assert_eq!(thoughts(&app), [lines(&["", "Weighing", ""])]);
}

/// **A thought the turn ended before says it was cut, and is drawn this run
/// only**: its block never came, so the turn keeps none of it — and a
/// failure's line, written as the turn ends, does not end it as whole. What
/// the stream held back of it lands in it first.
#[tokio::test]
async fn a_thought_cut_off_by_its_turn_says_so_and_is_not_kept() {
    let mut app = test_app();
    let handle = app.mark_secret("hunter2beornot2be", true).unwrap();
    let head = &handle[..6];
    app.submit("look");
    app.feed_stream(StreamMsg::Chunk("Hi".into()));
    app.feed_stream(StreamMsg::ThinkingChunk(format!("hm {head}")));
    app.feed_stream(StreamMsg::Error(shannon_core::BackendError::RateLimited));
    let drawn = thoughts(&app);
    assert_eq!(drawn.len(), 1, "{drawn:?}");
    assert_eq!(drawn[0][0], format!("hm {head}"));
    assert!(drawn[0][1].contains("cut off"), "{drawn:?}");
    let cut = (app.output().spans())
        .find(|s| s.role == BlockRole::Thinking)
        .unwrap();
    let anchor = crate::output::Anchor {
        block: cut.id,
        line: 0,
        col: 0,
    };
    assert_eq!(
        app.output().position_of(anchor),
        Err(crate::output::Unkept::LiveOnly)
    );
    let kept = app.conversation().messages().last().unwrap();
    assert_eq!(kept.content, "Hi", "premise: the turn kept, interrupted");
    assert!(kept.blocks.is_empty(), "and no thought: {kept:?}");
}

/// **What the stream held back lands where it was streamed** — a handle's
/// start at the end of the text stays in the text when a thought opens
/// below it; at a thought's end, in the thought, when its block closes it
/// or the reply's words come first — and the reply's own, held after
/// those words, stays the reply's.
#[tokio::test]
async fn a_held_tail_lands_where_it_was_streamed() {
    let mut app = test_app();
    let handle = app.mark_secret("hunter2beornot2be", true).unwrap();
    let head = &handle[..6];
    app.submit("look");
    app.feed_stream(StreamMsg::Chunk(format!("see {head}")));
    think(&mut app, &[&format!("hm {head}")]);
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::ThinkingChunk(format!("so {head}")));
    app.feed_stream(StreamMsg::Chunk("fine".into()));
    app.feed_stream(thought(&format!("so {head}")));
    app.feed_stream(StreamMsg::ThinkingChunk("um".into()));
    app.feed_stream(StreamMsg::Chunk(format!("then {head}")));
    app.feed_stream(StreamMsg::Chunk("x".into()));
    app.feed_stream(thought("um"));
    app.feed_stream(StreamMsg::Complete);
    use BlockRole::{Assistant, Thinking};
    assert_eq!(
        reply(&app),
        [
            (Assistant, lines(&[&format!("see {head}"), ""])),
            (Thinking, lines(&[&format!("hm {head}"), ""])),
            (Assistant, lines(&["ok", ""])),
            (Thinking, lines(&[&format!("so {head}"), ""])),
            (Assistant, lines(&["fine", ""])),
            (Thinking, lines(&["um", ""])),
            (Assistant, lines(&[&format!("then {head}x"), ""])),
        ]
    );
}

/// **A held tail lands in a thought that has drawn nothing yet** (the close
/// verifier at review stop 15, LOW 3) — a thought led by whitespace, or one
/// whose first chunk is only a handle's start, opens no block until its
/// text comes (B L3); asked "is a thought open?", the stream said no, and
/// what it held back was joined to the reply's first words.
#[tokio::test]
async fn a_held_tail_lands_in_a_thought_not_yet_drawn() {
    use BlockRole::{Assistant, Thinking};
    for lead in [" \n", ""] {
        let mut app = test_app();
        let handle = app.mark_secret("hunter2beornot2be", true).unwrap();
        let head = &handle[..6];
        app.submit("look");
        app.feed_stream(StreamMsg::ThinkingChunk(lead.into()));
        app.feed_stream(StreamMsg::ThinkingChunk(head.into()));
        app.feed_stream(StreamMsg::Chunk("Hi".into()));
        app.feed_stream(StreamMsg::Complete);
        let drawn = reply(&app);
        let holds = |role, wanted: &dyn Fn(&str) -> bool| {
            (drawn.iter()).any(|(r, lines)| *r == role && lines.iter().any(|l| wanted(l)))
        };
        assert!(
            holds(Thinking, &|l| l.contains(head)),
            "{lead:?}: {drawn:?}"
        );
        assert!(holds(Assistant, &|l| l == "Hi"), "{lead:?}: {drawn:?}");
    }
}

/// **A thought placed past its text, inside a char, or before the one
/// before it is drawn after the text it can follow** — a turn from a store
/// holds what it holds; replaying it never panics on a place no stream
/// could have made, nor walks to one a byte at a time.
#[test]
fn a_thought_placed_where_no_stream_could_is_drawn_after_its_text() {
    use shannon_core::{Block, Message, Role, Turn};
    let at = |said: &str, at| Block::Thinking {
        thinking: said.into(),
        signature: "S".into(),
        at,
    };
    let reply = Message {
        role: Role::Assistant,
        content: "é!".into(),
        blocks: vec![
            at("inside é", 1),
            at("past the end", usize::MAX),
            at("before the last", 0),
        ],
    };
    let turn = Turn::seal(
        vec![Message::user("ask"), reply],
        shannon_core::TurnConfig {
            model: "sonnet".to_string(),
            max_tokens: None,
            effort: None,
        },
        std::time::SystemTime::UNIX_EPOCH,
    );
    let mut output = crate::output::Output::new();
    App::replay_turn(&shannon_core::Aliases::default(), None, &mut output, &turn);
    let drawn: Vec<(BlockRole, Vec<String>)> = (output.spans())
        .filter(|s| s.role.the_models())
        .map(|s| (s.role, s.lines.to_vec()))
        .collect();
    use BlockRole::{Assistant, Thinking};
    assert_eq!(
        drawn,
        [
            (Thinking, lines(&["inside é", ""])),
            (Assistant, lines(&["é!", ""])),
            (Thinking, lines(&["past the end", ""])),
            (Thinking, lines(&["before the last", ""])),
        ]
    );
}

/// **Thinking is drawn quieter than the reply, and apart from a tool's
/// output** — in the theme's quiet text, italic and dim (Steven, 11-01:
/// "Italic helps but is not enough"): the rendered cells, in the pane and
/// focused, since the role is what both painters read (`view::base_style`).
/// A tool's output is dim too, apart by its colour and its slant
/// (`the_reply_a_thought_and_a_tools_output_are_drawn_apart`).
#[tokio::test]
async fn thinking_is_drawn_quieter_than_the_reply() {
    use super::stop::cell_at;
    use crate::style::Ink;
    use ratatui::style::Modifier;
    let mut app = test_app();
    app.submit("look");
    think(&mut app, &["weighing it `closely`"]);
    app.feed_stream(StreamMsg::Chunk("the answer".into()));
    app.feed_stream(StreamMsg::Complete);
    let thinking = app.theme().ui(Ink::Thinking);
    assert!(thinking.fg.is_some(), "premise: the theme paints thinking");
    assert_eq!(thinking.fg, app.theme().ui(Ink::TextQuiet).fg, "quiet text");
    let worn = Modifier::ITALIC | Modifier::DIM;
    let raw = app.theme().ui(Ink::Raw);
    assert!(
        raw.fg.is_some() && raw.fg != thinking.fg,
        "premise: code its own"
    );
    for focused in [false, true] {
        if focused {
            app.set_focus(PaneKind::Output);
        }
        let buf = frame_of(&mut app, 100, 30);
        let thought = cell_at(&buf, "weighing it", 0).style();
        assert_eq!(thought.fg, thinking.fg, "focused {focused}");
        assert!(thought.add_modifier.contains(worn), "focused {focused}");
        // Its markdown kept, in its italic and dim.
        let code = cell_at(&buf, "closely", 0).style();
        assert_eq!(code.fg, raw.fg, "focused {focused}: {code:?}");
        assert!(code.add_modifier.contains(worn), "focused {focused}");
        let said = cell_at(&buf, "the answer", 0).style();
        assert_ne!(
            said.fg, thinking.fg,
            "the reply its own (focused {focused})"
        );
        assert!(!said.add_modifier.intersects(worn), "focused {focused}");
    }
}

/// **The reply, a thought and a tool's output are drawn apart** (Steven,
/// 11-01: "when unfolded the user could easily recognize what is thinking,
/// what is tools … and what is text"): the reply plain; a thought in quiet
/// text, italic and dim; what came of a call in the record's colour, dim and
/// upright — the rendered cells, in the pane and focused, each pair apart.
#[tokio::test]
async fn the_reply_a_thought_and_a_tools_output_are_drawn_apart() {
    use super::stop::cell_at;
    use crate::style::Ink;
    use ratatui::style::Modifier;
    let mut app = test_app();
    app.submit("look");
    think(&mut app, &["weighing it"]);
    app.feed_stream(StreamMsg::Chunk("the answer".into()));
    app.output_mut()
        .push_tool_activity("⚙ output of: ls", "listing.txt");
    app.feed_stream(StreamMsg::Complete);
    app.open_every_fold_for_test();
    let tool = app.theme().ui(Ink::Tool).fg;
    assert!(tool.is_some(), "premise: the theme paints a record");
    for focused in [false, true] {
        if focused {
            app.set_focus(PaneKind::Output);
        }
        let buf = frame_of(&mut app, 100, 30);
        let worn = |text: &str| {
            let style = cell_at(&buf, text, 0).style();
            (style.fg, style.add_modifier)
        };
        let (reply, thought, output) =
            (worn("the answer"), worn("weighing it"), worn("listing.txt"));
        assert!(
            !reply.1.intersects(Modifier::ITALIC | Modifier::DIM),
            "focused {focused}: {reply:?}"
        );
        assert!(
            thought.1.contains(Modifier::ITALIC | Modifier::DIM),
            "focused {focused}: {thought:?}"
        );
        assert_eq!(output.0, tool, "focused {focused}: the record's colour");
        assert!(
            output.1.contains(Modifier::DIM) && !output.1.contains(Modifier::ITALIC),
            "focused {focused}: {output:?}"
        );
        assert!(
            reply != thought && thought != output && output != reply,
            "focused {focused}: three apart"
        );
    }
}

/// **A theme that names the thinking's and a tool output's own colours is
/// worn** — `ui.text.quiet.thinking` and `ui.tool.output`, each a colour
/// apart from the role it falls back to (`ui.text.quiet`, `ui.tool`): the
/// tests drew only themes that leave them unset, where the two answers are
/// one, so painting the fallback passed (review stop 15, C L-c: the
/// fixture omits the axis).
#[tokio::test]
async fn a_theme_naming_thinking_and_tool_output_is_worn() {
    use super::stop::cell_at;
    use ratatui::style::Color;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("t.toml"),
        "\"ui.text.quiet\" = \"#010101\"\n\"ui.text.quiet.thinking\" = \"#020202\"\n\
         \"ui.tool\" = \"#030303\"\n\"ui.tool.output\" = \"#040404\"\n",
    )
    .unwrap();
    let mut app = test_app();
    app.set_theme(
        crate::theme::Theme::load("t", Some(dir.path()))
            .expect("loads")
            .0,
    );
    app.submit("look");
    think(&mut app, &["weighing it"]);
    app.feed_stream(StreamMsg::Chunk("the answer".into()));
    app.output_mut()
        .push_tool_activity("⚙ output of: ls", "listing.txt");
    app.feed_stream(StreamMsg::Complete);
    app.open_every_fold_for_test();
    let grey = |n| Some(Color::Rgb(n, n, n));
    for focused in [false, true] {
        if focused {
            app.set_focus(PaneKind::Output);
        }
        let buf = frame_of(&mut app, 100, 30);
        let fg = |text: &str| cell_at(&buf, text, 0).style().fg;
        assert_eq!(fg("weighing it"), grey(2), "focused {focused}: thinking");
        assert_eq!(fg("listing.txt"), grey(4), "focused {focused}: output");
    }
}

/// **A thought ships shut to its first line** (N5a, NQ10): one row — `+ `,
/// its first line in the thinking's colour, then how many lines, the
/// decorations faint — in the pane and focused; and `j` steps over it as
/// one line, onto its row and past it.
#[tokio::test]
async fn a_thought_ships_shut_to_its_first_line() {
    use super::stop::cell_at;
    use crate::style::Ink;
    let mut app = test_app();
    app.submit("look");
    think(&mut app, &["Weighing the options\nfirst this\nthen that"]);
    app.feed_stream(StreamMsg::Chunk("The answer.".into()));
    app.feed_stream(StreamMsg::Complete);
    let (thinking, faint) = (
        app.theme().ui(Ink::Thinking),
        app.theme().ui(Ink::TextFaint),
    );
    assert_ne!(thinking.fg, faint.fg, "premise: told apart");
    for focused in [false, true] {
        if focused {
            app.set_focus(PaneKind::Output);
        }
        let buf = frame_of(&mut app, 100, 30);
        let drawn = rendered_text(&buf);
        assert!(
            drawn.contains("+ Weighing the options · 3 lines"),
            "focused {focused}:\n{drawn}"
        );
        assert!(!drawn.contains("first this"), "the rest shut");
        assert!(drawn.contains("The answer."), "the reply open");
        assert_eq!(cell_at(&buf, "+ Weighing", 0).style().fg, faint.fg);
        assert_eq!(cell_at(&buf, "Weighing", 0).style().fg, thinking.fg);
        assert_eq!(cell_at(&buf, "· 3 lines", 0).style().fg, faint.fg);
        // The fold's row alone wears it — not its separator below (review
        // stop 15, C L-b: the focused painter's filter was held by nothing).
        let worn = drawn.matches("· 3 lines").count();
        assert_eq!(worn, 1, "focused {focused}: one row\n{drawn}");
    }
    app.sync_chat();
    let pane = app.workspace.focused();
    let at = line_reading(&app, pane, "Weighing the options");
    type_keys_in(
        &mut app,
        Rect::new(0, 0, SIZE.0, SIZE.1),
        &format!("{at}Gj"),
    );
    assert_eq!(app.pane_chat(pane).cursor().0, at, "onto its row");
    type_keys_in(&mut app, Rect::new(0, 0, SIZE.0, SIZE.1), "j");
    assert_eq!(
        app.pane_chat(pane).cursor().0,
        at + 3,
        "past it, to its separator"
    );
}

/// **A shut thought says it holds a value the model wrote itself** (S21-Q13,
/// Steven 11-01: "Having that extra visibility will be a bit nice"; review
/// stop 15, E M3) — drawn open, the value is underlined in
/// `ui.secret.leaked`; shut, its row says `· leaked value` in that style,
/// in the pane and focused, and in a pane too narrow for it the thought's
/// text is cut again to leave it room. A thought holding none says nothing.
#[tokio::test]
async fn a_shut_thought_says_it_holds_a_leaked_value() {
    use super::stop::cell_at;
    use crate::style::Ink;
    use ratatui::style::Modifier;
    const VALUE: &str = "hunter2beornot2be";
    let mut app = test_app();
    app.mark_secret(VALUE, true).unwrap();
    app.submit("look");
    think(
        &mut app,
        &[&format!("Weighing the options\nthe key is {VALUE}\nthen")],
    );
    app.feed_stream(StreamMsg::Chunk("Between.".into()));
    think(&mut app, &["A second thought\nwith nothing held"]);
    app.feed_stream(StreamMsg::Chunk("The answer.".into()));
    app.feed_stream(StreamMsg::Complete);
    let leaked = app.theme().ui(Ink::SecretLeaked);
    for (width, focused) in [(100, false), (100, true), (40, true)] {
        if focused {
            app.set_focus(PaneKind::Output);
        }
        let buf = frame_of(&mut app, width, 30);
        let drawn = rendered_text(&buf);
        let row = |lead: &str| {
            (drawn.lines())
                .find(|row| row.contains(lead))
                .unwrap_or_else(|| panic!("{lead:?} at {width}:\n{drawn}"))
        };
        let said = row("+ Weighing");
        assert!(
            said.contains("· 3 lines · leaked value"),
            "{width}, focused {focused}: {said:?}"
        );
        assert!(!drawn.contains(VALUE), "shut, the value is not drawn");
        let word = cell_at(&buf, "leaked value", 0);
        assert_eq!(word.style().fg, leaked.fg, "{width}");
        assert!(word.style().add_modifier.contains(Modifier::UNDERLINED));
        assert!(!row("+ A second").contains("leaked"), "{drawn}");
    }
}

/// **…but not of a line too long to read for one** — past `WHOLE_LINE_MATCH`
/// a row is matched alone and no value on it is called leaked, so the row
/// that stands for it says what the open rows would (the close verifier at
/// review stop 15, LOW 1: the bound was held by nothing).
#[tokio::test]
async fn a_shut_thought_does_not_read_an_overlong_line_for_a_leak() {
    const VALUE: &str = "hunter2beornot2be";
    let mut app = test_app();
    app.mark_secret(VALUE, true).unwrap();
    app.submit("look");
    let long = format!("{}{VALUE}", "x".repeat(70_000));
    think(&mut app, &[&format!("Weighing it\n{long}\nthen")]);
    app.feed_stream(StreamMsg::Complete);
    let drawn = rendered_text(&frame_of(&mut app, 100, 30));
    assert!(
        drawn.contains("+ Weighing it · 3 lines"),
        "premise: shut\n{drawn}"
    );
    assert!(!drawn.contains("leaked value"), "{drawn}");
}

/// **A shut thought the turn cut off says so** — its last line says it,
/// under the fold: the row says `· cut off` (S21-Q13, E M3).
#[tokio::test]
async fn a_shut_thought_the_turn_cut_off_says_so() {
    let mut app = test_app();
    app.submit("look");
    app.feed_stream(StreamMsg::ThinkingChunk("Weighing it\nsecond".into()));
    app.feed_stream(StreamMsg::Error(shannon_core::BackendError::RateLimited));
    let drawn = rendered_text(&frame_of(&mut app, 100, 30));
    assert!(
        drawn.contains("+ Weighing it · 3 lines · cut off"),
        "{drawn}"
    );
    assert!(!drawn.contains("[cut off"), "its line, shut: {drawn}");
}

/// **A turn that mostly thought is not "billed for nothing shown"**: its
/// thinking was drawn, and counts (`Conversation::draft_len`).
#[tokio::test]
async fn shown_thinking_counts_against_the_bill() {
    let mut app = test_app();
    app.submit("look");
    think(&mut app, &[&"thought ".repeat(80)]);
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::Usage(shannon_core::Usage {
        output_tokens: 1000,
        ..Default::default()
    }));
    app.feed_stream(StreamMsg::Complete);
    let said = app.notice().unwrap_or_default();
    assert!(!said.contains("showed almost none"), "{said}");
}

/// **A turn keeps its thinking where it was written** — through the
/// production path (`dispatch_stream_msg`): the block whole, after the text
/// before it, in the reply the next request sends back.
#[tokio::test]
async fn a_turn_keeps_its_thinking_where_it_was_written() {
    let mut app = test_app();
    app.submit("look");
    app.feed_stream(thought("plan"));
    app.feed_stream(StreamMsg::Chunk("Done".into()));
    app.feed_stream(thought("checked"));
    app.feed_stream(StreamMsg::Chunk(".".into()));
    app.feed_stream(StreamMsg::Complete);
    let reply = app
        .conversation()
        .messages()
        .last()
        .expect("a reply")
        .clone();
    assert_eq!(reply.content, "Done.");
    let places: Vec<Option<usize>> = reply.blocks.iter().map(|b| b.at()).collect();
    assert_eq!(places, [Some(0), Some(4)], "{reply:?}");
    let wire = serde_json::to_value(&reply).unwrap();
    let kinds: Vec<&str> = (wire["content"].as_array().unwrap().iter())
        .map(|b| b["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["thinking", "text", "thinking", "text"]);
}

/// **What the API dropped is said, on a turn** — how many and why, each
/// reason once.
#[tokio::test]
async fn the_thinking_the_api_dropped_is_said() {
    use shannon_core::Dropped;
    let mut app = test_app();
    app.submit("look");
    app.feed_stream(StreamMsg::ThinkingDropped(vec![
        Dropped::Prefix,
        Dropped::Prefix,
        Dropped::Model,
    ]));
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("dropped 3 thinking blocks"), "{said}");
    assert_eq!(
        said.matches("the conversation before them changed").count(),
        1,
        "{said}"
    );
    assert!(said.contains("cannot read them"), "{said}");
}

/// **A draft's thinking stays out of the transcript** — a `:compact`'s
/// distillation and a `:continue`'s continuation think too (every kind of
/// work asks for thinking), and what they think is drawn nowhere and kept
/// nowhere: a draft composes text, not a turn. Held by nothing until
/// review stop 15 (C H1: both gates deleted, every test passed; measured,
/// the summariser's thought drawn under the session's last turn).
#[tokio::test]
async fn a_drafts_thinking_stays_out_of_the_transcript() {
    let dir = TempDir::new().unwrap();
    let mut app = over_store(dir.path());
    for i in 0..2 {
        app.submit(&format!("ask {i}"));
        app.feed_stream(StreamMsg::Chunk(format!("reply {i}")));
        app.feed_stream(StreamMsg::Complete);
    }
    app.dispatch_command_for_test("compact --keep 1");
    let (drawn, sent) = (app.output().lines(), app.conversation().messages().len());
    app.feed_summary(StreamMsg::ThinkingChunk("summariser weighing".into()));
    app.feed_summary(thought("summariser weighing"));
    assert_eq!(app.output().lines(), drawn, "a distillation's: not drawn");
    assert_eq!(app.conversation().messages().len(), sent, "nor kept");
    assert!(thoughts(&app).is_empty(), "{:?}", thoughts(&app));

    // The prefill strategy streams the continuation off-transcript.
    let mut cfg = Config::default();
    cfg.continue_.strategy = shannon_core::ContinueStrategy::Prefill;
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
    assert!(app.continuing(), "premise");
    let (drawn, sent) = (app.output().lines(), app.conversation().messages().len());
    app.feed_continuation(StreamMsg::ThinkingChunk("continuer weighing".into()));
    app.feed_continuation(thought("continuer weighing"));
    assert_eq!(app.output().lines(), drawn, "a continuation's: not drawn");
    assert_eq!(app.conversation().messages().len(), sent, "nor kept");
}
