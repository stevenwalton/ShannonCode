//! **The transcript's window** — the wheel, `H` `M` `L` and `zt` `zz` `zb`
//! in a focused transcript, through the routes a user's input takes
//! (`wheel_scroll_at`, `key_for_test`).
//!
//! Steven, 2026-10-04, from inside Shannon: "`H`,`M`,`L` aren't working on the
//! chat pane and mouse scrolling is odd. It doesn't push the scroll window and
//! seems to stutter." The motions were refused; the wheel moved the cursor
//! three LOGICAL lines and never the window.
//!
//! Every line of `transcript`'s fixture is one row at 80 columns, so the
//! expected tops and lines below are vim's rules done by hand: a 24-row
//! window, `scrolloff` 5 (the default), a band of rows `top+5 ..= top+18`.

use super::*;

const SIZE: (u16, u16) = (80, 24);

/// `app`'s transcript, written as a reply streams in: a turn per `(prompt,
/// reply)`.
fn turns<'a>(app: &mut App, each: impl IntoIterator<Item = (String, &'a str)>) {
    for (prompt, reply) in each {
        app.output_mut().begin_turn(&prompt);
        app.output_mut().append_chunk(reply);
        app.output_mut().end_turn();
    }
}

/// A focused transcript of `n` short turns, its chat synced as a frame would.
fn transcript(n: usize) -> (App, crate::wm::PaneId) {
    let mut app = test_app();
    // Indented replies, so `H` landing on the first non-blank is visible.
    turns(&mut app, (0..n).map(|i| (format!("ask {i}"), "  ok")));
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    let output = &app.pane_session(pane).output;
    let shown = output.visible_window(
        &app.pane_viewport(pane),
        &app.pane_shut(pane),
        SIZE.0,
        SIZE.1,
    );
    assert_eq!(
        shown.total_rows,
        output.line_count(),
        "premise: one row per line, so the arithmetic below is in both"
    );
    assert!(
        shown.total_rows > 3 * SIZE.1 as usize,
        "premise: room to scroll"
    );
    (app, pane)
}

/// The first row `pane` shows.
fn top(app: &App, pane: crate::wm::PaneId) -> usize {
    let output = &app.pane_session(pane).output;
    output
        .visible_window(
            &app.pane_viewport(pane),
            &app.pane_shut(pane),
            SIZE.0,
            SIZE.1,
        )
        .top
}

/// The logical line `pane`'s cursor is on.
fn line(app: &App, pane: crate::wm::PaneId) -> usize {
    app.pane_chat(pane).cursor().0
}

/// `notches` of the wheel over `pane`.
fn wheel(app: &mut App, pane: crate::wm::PaneId, down: bool, notches: usize) {
    for _ in 0..notches {
        app.wheel_scroll_at(pane, !down, SIZE);
    }
}

/// **The wheel moves the WINDOW, three rows a notch, and the cursor only
/// when the window would leave it** — vim's rule. It moved the cursor three
/// logical lines and never the window, so nothing scrolled until the next
/// key, and then everything did at once.
#[test]
fn the_wheel_scrolls_the_focused_transcript() {
    let (mut app, pane) = transcript(30);
    type_keys_sized(&mut app, SIZE, "gg11j");
    assert_eq!((top(&app, pane), line(&app, pane)), (0, 11), "premise");

    wheel(&mut app, pane, true, 1);
    assert_eq!(top(&app, pane), 3, "the window moved three rows");
    assert_eq!(
        line(&app, pane),
        11,
        "and the cursor, inside the band, stayed"
    );

    wheel(&mut app, pane, true, 2);
    assert_eq!(top(&app, pane), 9);
    assert_eq!(
        line(&app, pane),
        14,
        "dragged to the band: scrolloff below the top"
    );

    wheel(&mut app, pane, false, 1);
    assert_eq!((top(&app, pane), line(&app, pane)), (6, 14), "up: it stays");

    // The window the wheel left is the one the next key moves in — no snap.
    type_keys_sized(&mut app, SIZE, "j");
    assert_eq!((top(&app, pane), line(&app, pane)), (6, 15));
}

/// **In rows, not lines** — a reply is one logical line over many rows, and
/// a notch is three of them wherever it lands. In lines, one notch over a
/// paragraph moved by the paragraph: the stutter.
#[test]
fn the_wheel_moves_by_rows_inside_a_long_reply() {
    let mut app = test_app();
    let long = "a word of reply ".repeat(400);
    let before = (0..3).map(|n| (format!("short {n}"), "ok"));
    let after = (0..10).map(|n| (format!("after {n}"), "ok"));
    turns(
        &mut app,
        before
            .chain(std::iter::once((
                "tell me at length".to_string(),
                long.as_str(),
            )))
            .chain(after),
    );
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    let output = &app.pane_session(pane).output;
    let lines = output.lines();
    let long = (0..lines.len())
        .max_by_key(|&i| lines[i].len())
        .expect("lines");
    let starts = output
        .logical_to_visual(&app.pane_shut(pane), long, 0, SIZE.0)
        .0;
    let ends = output
        .logical_to_visual(
            &app.pane_shut(pane),
            long,
            lines[long].chars().count(),
            SIZE.0,
        )
        .0;
    assert!(
        ends - starts > 2 * SIZE.1 as usize,
        "premise: taller than two windows"
    );

    // The cursor at the reply's first char, the window where `follow` put it.
    type_keys_sized(&mut app, SIZE, &format!("gg{long}j0"));
    assert_eq!(line(&app, pane), long, "premise");
    for _ in 0..12 {
        let before = top(&app, pane);
        wheel(&mut app, pane, true, 1);
        let now = top(&app, pane);
        assert_eq!(now, before + 3, "three rows a notch, mid-paragraph");
        let (l, c) = app.pane_chat(pane).cursor();
        let row = app
            .pane_session(pane)
            .output
            .logical_to_visual(&app.pane_shut(pane), l, c, SIZE.0)
            .0;
        assert_eq!(l, long, "still in the reply: dragged by rows within it");
        assert!(
            (now + 5..=now + 18).contains(&row),
            "the cursor's row {row} stays in the band of top {now}"
        );
    }

    // The window now shows only the reply's middle: it is the one line on
    // screen, so `H` goes to its first char — above the window — and the
    // window follows the cursor there rather than leaving it off screen.
    assert!(top(&app, pane) > starts, "premise: the reply starts above");
    type_keys_sized(&mut app, SIZE, "H");
    assert_eq!(app.pane_chat(pane).cursor(), (long, 0));
    let shown = top(&app, pane);
    assert!(
        (shown..shown + SIZE.1 as usize).contains(&starts),
        "the cursor's row {starts} is on screen (top {shown})"
    );
}

/// **`H` `M` `L` land on lines on screen**, `scrolloff` in from an edge that
/// is not the buffer's, and move nothing but the cursor.
#[test]
fn h_m_l_land_on_the_lines_on_screen() {
    let (mut app, pane) = transcript(30);
    type_keys_sized(&mut app, SIZE, "gg");
    wheel(&mut app, pane, true, 4);
    assert_eq!(top(&app, pane), 12, "premise");

    type_keys_sized(&mut app, SIZE, "L");
    assert_eq!(
        (top(&app, pane), line(&app, pane)),
        (12, 30),
        "L: 12 + 23 - 5"
    );
    type_keys_sized(&mut app, SIZE, "M");
    assert_eq!(
        (top(&app, pane), line(&app, pane)),
        (12, 23),
        "M: the middle row"
    );
    type_keys_sized(&mut app, SIZE, "H");
    assert_eq!((top(&app, pane), line(&app, pane)), (12, 17), "H: 12 + 5");
    type_keys_sized(&mut app, SIZE, "3L");
    assert_eq!(
        line(&app, pane),
        30,
        "3L inside the band is the band's edge"
    );
    assert!(
        app.notice().is_none(),
        "applied, not refused: {:?}",
        app.notice()
    );

    // At the buffer's own top there is no margin to keep.
    type_keys_sized(&mut app, SIZE, "ggH");
    assert_eq!((top(&app, pane), line(&app, pane)), (0, 0));
    type_keys_sized(&mut app, SIZE, "L");
    assert_eq!((top(&app, pane), line(&app, pane)), (0, 18));

    // On its first non-blank char, as vim's `H` is: reply line 10 (`  ok`),
    // put at the band's top by `zt`, is where `H` lands — at column 2.
    type_keys_sized(&mut app, SIZE, "gg10jzt");
    assert_eq!(top(&app, pane), 5, "premise: zt, 10 - 5");
    type_keys_sized(&mut app, SIZE, "jjjH");
    assert_eq!(app.pane_chat(pane).cursor(), (10, 2), "not column 0");
}

/// **`zt` `zz` `zb` place the cursor's line**, and `20zt` goes to line 20
/// first, keeping the column (`:h zt`).
#[test]
fn z_places_the_cursors_line() {
    let (mut app, pane) = transcript(30);
    type_keys_sized(&mut app, SIZE, "gg30j");
    assert_eq!(line(&app, pane), 30, "premise");
    type_keys_sized(&mut app, SIZE, "zt");
    assert_eq!((top(&app, pane), line(&app, pane)), (25, 30), "zt: 30 - 5");
    type_keys_sized(&mut app, SIZE, "zz");
    assert_eq!(top(&app, pane), 19, "zz: 30 - 11");
    type_keys_sized(&mut app, SIZE, "zb");
    assert_eq!(top(&app, pane), 12, "zb: 30 + 1 + 5 - 24");
    // From the end of `> ask 5`, the line below: line 23 is `  ok`, so the
    // column kept is its last char, 3 — not 0.
    type_keys_sized(&mut app, SIZE, "j$");
    assert_eq!(
        app.pane_chat(pane).cursor(),
        (31, 6),
        "premise: `> ask 5`'s end"
    );
    type_keys_sized(&mut app, SIZE, "23zt");
    assert_eq!(top(&app, pane), 17, "line 23 at the top");
    assert_eq!(app.pane_chat(pane).cursor(), (22, 3), "in the same column");
    assert!(
        app.notice().is_none(),
        "applied, not refused: {:?}",
        app.notice()
    );
}

/// **`]]` `[[` move between turns** (Steven, 10-15: "vim like commands to
/// jump to conversations"; neovim makes each shell prompt a section the same
/// way) — onto the rule above each message, counted, and `y]]` from a turn's
/// rule copies that turn whole and nothing of the next. Each of the
/// fixture's turns is six lines: a rule, `> ask N`, a rule, a blank, `  ok`,
/// the blank that ends it.
#[test]
fn brackets_move_between_turns() {
    let (mut app, pane) = transcript(30);
    type_keys_sized(&mut app, SIZE, "gg]]");
    assert_eq!(
        app.pane_chat(pane).cursor(),
        (6, 0),
        "the second turn's rule"
    );
    assert_eq!(app.pane_chat(pane).line_text(7).as_deref(), Some("> ask 1"));
    type_keys_sized(&mut app, SIZE, "3]]");
    assert_eq!(line(&app, pane), 24, "three on");
    type_keys_sized(&mut app, SIZE, "[[");
    assert_eq!(line(&app, pane), 18);
    type_keys_sized(&mut app, SIZE, "G[[");
    assert_eq!(line(&app, pane), 174, "from the end, the last turn's rule");
    type_keys_sized(&mut app, SIZE, "gg]]y]]");
    assert_eq!(
        app.prompt().register_text().as_deref(),
        Some("\n> ask 1\n\n\n  ok\n\n"),
        "one turn, linewise"
    );
    // `][` `[]`: a turn's last line of text — its reply's `  ok`, on its
    // first non-blank.
    type_keys_sized(&mut app, SIZE, "gg][");
    assert_eq!(app.pane_chat(pane).cursor(), (4, 2), "the first turn's end");
    type_keys_sized(&mut app, SIZE, "2][");
    assert_eq!(line(&app, pane), 16);
    type_keys_sized(&mut app, SIZE, "[]");
    assert_eq!(line(&app, pane), 10);
    assert!(
        app.notice().is_none(),
        "applied, not refused: {:?}",
        app.notice()
    );
}

/// **An operator over the screen is still refused in the transcript**, and
/// says so in its own words — the motion works, so the sentence for the
/// prompt ("don't work in the prompt yet") would be false here.
#[test]
fn an_operator_over_the_screen_is_refused_in_the_transcript() {
    let (mut app, pane) = transcript(30);
    type_keys_sized(&mut app, SIZE, "gg5j");
    type_keys_sized(&mut app, SIZE, "yL");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("doesn't work here yet"), "{said}");
    assert_eq!(line(&app, pane), 5, "and nothing moved");
}

/// **`H`, `M` and `L` never move the window, whatever the replies' height**
/// (review of c816165, Reviewer C). Every test above has one row per line —
/// the one shape in which `scrolloff` in lines and in rows agree. With
/// replies ~19 rows tall, `H` and `M` scrolled the window up to four rows and
/// `L` scrolled it right after `gg`.
#[test]
fn h_m_l_never_move_a_window_of_tall_replies() {
    // Replies about 3, 5, 8, 12 and 19 rows tall: where lines and rows
    // disagree depends on the height, so each is its own case.
    for words in [40, 70, 120, 180, 290] {
        h_m_l_hold_the_window(words);
    }
}

fn h_m_l_hold_the_window(words: usize) {
    let mut app = test_app();
    let reply = "word ".repeat(words);
    turns(
        &mut app,
        (0..12).map(|i| (format!("ask {i}"), reply.as_str())),
    );
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    type_keys_sized(&mut app, SIZE, "gg");
    // The row each line starts on, and the row the cursor is on.
    let start = |app: &App, l: usize| {
        let output = &app.pane_session(pane).output;
        output
            .logical_to_visual(&app.pane_shut(pane), l, 0, SIZE.0)
            .0
    };
    let cursor_row = |app: &App| {
        let (l, c) = app.pane_chat(pane).cursor();
        app.pane_session(pane)
            .output
            .logical_to_visual(&app.pane_shut(pane), l, c, SIZE.0)
            .0
    };
    for notches in 0..60 {
        for key in ["H", "M", "L"] {
            let before = top(&app, pane);
            type_keys_sized(&mut app, SIZE, key);
            assert_eq!(
                top(&app, pane),
                before,
                "{key} after {notches} notches moved the window"
            );
            // And it rests inside the `scrolloff` band whenever a line on
            // screen starts there, so the next `j` does not scroll either —
            // the band a scroll keeps the cursor in (`viewport::drag`, which
            // has no margin at the buffer's own edges).
            let output = &app.pane_session(pane).output;
            let total = output
                .visible_window(
                    &app.pane_viewport(pane),
                    &app.pane_shut(pane),
                    SIZE.0,
                    SIZE.1,
                )
                .total_rows;
            let last = before + SIZE.1 as usize - 1;
            let in_band = |row: usize| crate::viewport::drag(row, before, last, total, 5) == row;
            let lines = output.line_count();
            if (0..lines).any(|l| in_band(start(&app, l))) {
                assert!(
                    in_band(cursor_row(&app)),
                    "{key} after {notches} notches left the band"
                );
            }
        }
        wheel(&mut app, pane, true, 1);
    }
}

/// A frame's size, and the transcript pane's content inside it — what
/// `before_frame` measures, so keys here move in the window it keeps.
const FRAME: (u16, u16) = (80, 40);

/// `pane`'s window and its cursor, as one frame leaves them: the window's
/// top, its height, the cursor's logical line, and the cursor's row.
fn streamed(app: &mut App, pane: crate::wm::PaneId, line: &str) -> (usize, usize, usize, usize) {
    app.output_mut().append_chunk(line);
    // What the stream path sets for each chunk.
    app.chat_dirty = true;
    app.before_frame(FRAME.0, FRAME.1);
    let (w, h) = app.pane_content_size(pane, Rect::new(0, 0, FRAME.0, FRAME.1));
    let (l, c) = app.pane_chat(pane).cursor();
    let output = &app.pane_session(pane).output;
    let top = output
        .visible_window(&app.pane_viewport(pane), &app.pane_shut(pane), w, h)
        .top;
    (
        top,
        h as usize,
        l,
        output.logical_to_visual(&app.pane_shut(pane), l, c, w).0,
    )
}

/// **A window following a reply takes the cursor with it** (review of
/// c816165, Reviewer C, F4). It moved as the reply streamed, and the cursor
/// stayed on its line — carried up and off the screen, until the next `j`
/// snapped the window back to it. A cursor on the last line rides it, as a
/// terminal's does; one above it stays on its line until the window would
/// leave it, then is dragged, as the wheel drags it.
#[test]
fn a_streaming_reply_keeps_the_cursor_on_screen() {
    for (keys, rides) in [("G", true), ("Gkkk", false)] {
        let (mut app, pane) = transcript(30);
        type_keys_in(&mut app, Rect::new(0, 0, FRAME.0, FRAME.1), keys);
        assert!(
            app.pane_viewport(pane).is_following_tail(),
            "premise: {keys}"
        );
        let start = line(&app, pane);
        app.output_mut().begin_turn("more");
        let mut dragged = false;
        for i in 0..60 {
            let (top, h, l, row) = streamed(&mut app, pane, &format!("line {i}\n"));
            assert!(
                (top..top + h).contains(&row),
                "{keys}: chunk {i}: the cursor's row {row} is off the window at {top}"
            );
            let last = app.pane_chat(pane).line_count() - 1;
            match rides {
                true => assert_eq!(l, last, "{keys}: it rides the last line"),
                false if l == start => assert!(!dragged, "{keys}: dragged, then back?"),
                false => dragged = true,
            }
        }
        assert!(rides || dragged, "premise: {keys} was carried to the edge");
    }
}

/// **A transcript is entered where its window is**: at its end, on screen.
/// The cursor of a transcript never focused sat on line 0 while its window
/// showed the tail — off screen in any long one, so the first `j` threw the
/// window to the top.
#[test]
fn a_transcript_is_entered_where_its_window_is() {
    let mut app = test_app();
    turns(&mut app, (0..40).map(|i| (format!("ask {i}"), "ok")));
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let (top, h, l, row) = streamed(&mut app, pane, "");
    assert_eq!(l, app.pane_chat(pane).line_count() - 1, "at the end");
    assert!(
        (top..top + h).contains(&row),
        "on screen: row {row}, top {top}"
    );
    assert!(top > 0, "premise: a transcript longer than its window");
}

/// **A reply streaming in leaves a selection alone**: moving the cursor under
/// it would grow what `y` takes, and the reply is not the user's doing
/// (review of 628df82, Reviewer A).
#[test]
fn a_streaming_reply_leaves_a_selection_alone() {
    let (mut app, pane) = transcript(30);
    type_keys_in(&mut app, Rect::new(0, 0, FRAME.0, FRAME.1), "Gv");
    let chat = app.pane_chat(pane);
    let before = (chat.anchor(), chat.cursor());
    assert!(before.0.is_some(), "premise: selecting");
    app.output_mut().begin_turn("more");
    for i in 0..20 {
        streamed(&mut app, pane, &format!("line {i}\n"));
        let chat = app.pane_chat(pane);
        assert_eq!((chat.anchor(), chat.cursor()), before, "chunk {i}");
    }
}

/// **Only a window that follows the tail is ridden**: one wheeled away
/// while another pane had focus kept its cursor where it was, and focusing
/// it again does not throw the cursor to the end (review of 628df82,
/// Reviewer A).
#[test]
fn a_window_wheeled_away_is_not_ridden_on_focus() {
    let (mut app, pane) = transcript(30);
    type_keys_in(&mut app, Rect::new(0, 0, FRAME.0, FRAME.1), "G");
    let size = app.pane_content_size(pane, Rect::new(0, 0, FRAME.0, FRAME.1));
    app.set_focus(PaneKind::Prompt);
    // A reply streams in while another pane has focus — the window follows
    // it, and this pane's cursor is not synced — then the wheel moves the
    // window back up, over that cursor, still on its old last line.
    app.output_mut().begin_turn("more");
    for i in 0..17 {
        app.output_mut().append_chunk(&format!("line {i}\n"));
    }
    app.wheel_scroll_at(pane, true, size);
    assert!(
        !app.pane_viewport(pane).is_following_tail(),
        "premise: wheeled away"
    );
    let line_before = line(&app, pane);
    let shown = app
        .pane_session(pane)
        .output
        .visible_window(
            &app.pane_viewport(pane),
            &app.pane_shut(pane),
            size.0,
            size.1,
        )
        .top;
    assert!(
        (shown..shown + size.1 as usize).contains(&line_before),
        "premise: the wheel brought the window back over the cursor"
    );
    app.set_focus(PaneKind::Output);
    let (top, h, l, row) = streamed(&mut app, pane, "");
    assert!((top..top + h).contains(&row), "on screen");
    assert!(
        l <= line_before,
        "not thrown to the end: {line_before} -> {l}"
    );
}

/// **A window you scrolled away stays, and so does its cursor**: a reply
/// streaming in below moves neither.
#[test]
fn a_streaming_reply_leaves_a_window_scrolled_away() {
    let (mut app, pane) = transcript(30);
    type_keys_in(&mut app, Rect::new(0, 0, FRAME.0, FRAME.1), "gg5j");
    assert!(!app.pane_viewport(pane).is_following_tail(), "premise");
    app.output_mut().begin_turn("more");
    let before = streamed(&mut app, pane, "line 0\n");
    for i in 1..40 {
        assert_eq!(streamed(&mut app, pane, &format!("line {i}\n")), before);
    }
}

#[test]
fn focused_chat_renders_the_transcript_from_output() {
    // A turn in the transcript; focus the chat (the navigable view).
    let mut app = test_app();
    app.output_mut().begin_turn("hello there");
    app.output_mut().append_chunk("a reply with several words");
    app.output_mut().end_turn();
    app.set_focus(PaneKind::Output);
    app.sync_chat();

    // Tall enough that the output pane gets real rows under the default
    // 10-row prompt + status (a short terminal squeezes it to nothing).
    let screen = text_of(&mut app, 40, 24);

    // The focused chat renders the transcript content (from Output), not a
    // blank pane or the old plain Buffer path.
    assert!(
        screen.contains("hello there"),
        "user echo missing:\n{screen}"
    );
    assert!(
        screen.contains("a reply with several words"),
        "assistant reply missing:\n{screen}"
    );
}

/// **Your message is drawn between two rules**, a blank row before the reply
/// (Steven, 10-15: the user's side "easier to notice when scrolling
/// through"), streaming and focused alike — read off the drawn cells, since
/// a role can be right in `Output` and still be painted as a blank row.
#[test]
fn your_message_is_drawn_between_two_rules() {
    use crate::style::Ink;
    use ratatui::style::Color;
    let mut app = test_app();
    app.output_mut().begin_turn("hello there");
    app.output_mut().append_chunk("a reply");
    app.output_mut().end_turn();
    let rule_fg = app.theme().fg(Ink::BorderUser);
    assert_ne!(rule_fg, Color::Reset, "premise: the rule has a colour");
    for focused in [false, true] {
        if focused {
            app.set_focus(PaneKind::Output);
            app.sync_chat();
        }
        let buf = &frame_of(&mut app, 40, 24);
        let text = rendered_text(buf);
        let rows: Vec<&str> = text.lines().collect();
        let echo = rows
            .iter()
            .position(|r| r.contains("> hello there"))
            .unwrap_or_else(|| panic!("the echo is drawn:\n{text}"));
        // Inside the pane's border: columns 1..39.
        for y in [echo - 1, echo + 1] {
            let y = y as u16;
            let cells: String = (1..39).map(|x| buf[(x, y)].symbol()).collect();
            assert_eq!(cells, "─".repeat(38), "focused {focused}:\n{text}");
            assert_eq!(buf[(1, y)].style().fg, Some(rule_fg), "focused {focused}");
        }
        let after: String = rows[echo + 2].chars().skip(1).collect();
        assert_eq!(after.trim_end_matches(['│', ' ']), "", "a blank row");
        assert!(
            rows[echo + 3].contains("a reply"),
            "focused {focused}:\n{text}"
        );
    }
}

/// **Two transcript panes are independent.**
///
/// Steven, splitting one: "I noticed that it changed both panes… I think we
/// should leave them independent." They could not be: the transcript cursor was
/// a single field on `App`, so "this pane's cursor" and "the focused pane's
/// cursor" were the same object. It is per-pane state now — vim's rule, where
/// two windows on one buffer share the text and nothing else.
#[test]
fn two_transcript_panes_keep_their_own_cursors() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    for line in ["one", "two", "three", "four", "five"] {
        app.output_mut().begin_turn(line);
        app.output_mut().end_turn();
    }
    let a = app.workspace.focused();
    app.split_command("output", SplitDir::Horizontal);
    let b = app.workspace.focused();
    assert_ne!(a, b, "two transcript panes");

    // Drive it the way a user does: a pane's cursor buffer mirrors the
    // transcript when that pane is focused, so focus each in turn.
    app.workspace.focus_pane(b);
    app.sync_chat();
    app.chat_mut().unwrap().place_cursor(3, 0);
    assert_eq!(app.pane_chat(b).cursor().0, 3);

    app.workspace.focus_pane(a);
    app.sync_chat();
    app.chat_mut().unwrap().place_cursor(1, 0);

    // Both remember their own, and neither followed the other.
    assert_eq!(app.pane_chat(a).cursor().0, 1, "A is where we left it");
    assert_eq!(app.pane_chat(b).cursor().0, 3, "B kept its own");
}

/// A closed pane's cursor goes with it — view state must not outlive the window
/// it describes, or a reused `PaneId` inherits a stranger's cursor. The window
/// lives in the pane's slot, so the close takes it (by type since V5.3 (4) —
/// what is asked here is the half that is not: V5.3 verify, F6); and the
/// close's leftover is drained by the frame's reconcile, not kept for a later
/// reader.
#[test]
fn closing_a_pane_drops_its_view_state() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("output", SplitDir::Horizontal);
    let doomed = app.workspace.focused();
    app.pane_chat_mut(doomed).unwrap().place_cursor(0, 0);

    app.close_pane_command(Asked::Plain);
    assert!(app.window_of(doomed).is_none(), "premise: closed");
    assert!(
        app.workspace.take_closed().is_empty(),
        "nothing closed is left waiting for a reader"
    );
}

/// The persist cursor belongs to the **buffer**, not the app.
///
/// Leaving a session open and coming back must not resume with a cursor
/// describing a different transcript — that either re-appends turns already on
/// disk or skips ones that are not.
#[test]
fn the_persist_cursor_follows_the_session() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3); // A: 3 persisted turns
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0"); // B: 1 persisted turn
    assert_eq!(app.buffer().on_disk, OnDisk::Own(1), "B's own cursor");

    app.open_by_hash(&a.to_hex()[..8]);
    assert_eq!(app.buffer().on_disk, OnDisk::Own(3), "A's, not B's");

    app.open_by_hash(&b.to_hex()[..8]);
    assert_eq!(
        app.buffer().on_disk,
        OnDisk::Own(1),
        "B's again, not re-derived"
    );
}

/// Pointing a pane at another session drops its cursor: the cursor indexes rows
/// of a transcript, and carried across a switch it is a live index into
/// somebody else's text.
#[test]
fn rebinding_a_pane_forgets_where_its_cursor_was() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _sibling, focused) = app_with_two_sessions(dir.path());
    // Open A first: it is loaded from the store, so its transcript has real
    // rows to put a cursor on.
    app.open_by_hash(&a.to_hex()[..8]);
    app.sync_chat();
    app.chat_mut().unwrap().place_cursor(1, 0);
    assert_eq!(app.pane_chat(focused).cursor().0, 1);

    app.open_by_hash(&b.to_hex()[..8]);
    assert_eq!(
        app.pane_chat(focused).cursor().0,
        0,
        "a new session starts at the top, not at the old row"
    );
}

/// **A split of a transcript is a second window where you were** (V4; the
/// 10-21 docket's "every split kind") — vim's `:split` keeps the cursor and
/// the top line. The copy began as a fresh window, following the tail: back
/// in a long reply, reading, a split showed its end.
#[test]
fn a_split_transcript_is_a_second_window_where_you_were() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    for i in 0..40 {
        app.output_mut().begin_turn(&format!("question {i}"));
        app.output_mut().end_turn();
    }
    app.sync_chat();
    let from = app.workspace.focused();
    let view = app.window_of_mut(from).unwrap().session_view_mut().unwrap();
    view.chat.place_cursor(7, 0);
    view.viewport.freeze_at(5, 0);
    let window = |app: &App, pane| {
        let v = app.window_of(pane)?.session_view()?;
        Some((v.chat.cursor(), v.viewport.top()))
    };
    let before = window(&app, from);
    assert_eq!(
        before,
        Some(((7, 0), Some((5, 0)))),
        "premise: reading, line 7"
    );

    app.split_command("", SplitDir::Horizontal);
    let copy = app.workspace.focused();
    assert_ne!(copy, from, "premise: split");
    assert_eq!(
        window(&app, copy),
        before,
        "the cursor and the scroll, copied"
    );
    app.sync_chat();
    assert_eq!(window(&app, copy), before, "and a re-sync keeps them");
}

/// Two `:help` panes page against **their own** height and width.
///
/// The first version of this test could not fail: a 50/50 vsplit gives the two
/// panes IDENTICAL sizes, so `first_of(kind)` — the bug — returned the right
/// answer by accident and every assertion held. Reviewer C proved it
/// arithmetically. The panes must genuinely differ before asking.
#[test]
fn a_second_text_pane_scrolls_against_its_own_size() {
    use crate::keymap::WindowCmd;
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.open_help_command(None);
    let first = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    let second = app.workspace.focused();

    // Make them different: widen the focused one until the split is lopsided.
    for _ in 0..8 {
        let _ = app.window_command_for_test(WindowCmd::ResizeWider);
    }

    let area = Rect::new(0, 0, 120, 40);
    let layout = app.workspace.frame_layout(area);
    let want_first = layout.pane(first).expect("on screen").content_size();
    let want_second = layout.pane(second).expect("on screen").content_size();
    assert_ne!(
        want_first, want_second,
        "the arrangement must distinguish the two, or the test proves nothing"
    );

    assert_eq!(app.pane_content_size(first, area), want_first);
    assert_eq!(
        app.pane_content_size(second, area),
        want_second,
        "each pane is measured as itself, not as the first of its kind"
    );
}

/// The wheel scrolls the session under the pointer, not the foreground one —
/// driven through the routing that had the bug.
///
/// The first version was vacuous twice over: it discarded the positive half
/// with `let _ = before;`, and its negative half could not fail because the
/// transcript (6 rows) fit the viewport (10), so nothing could scroll however
/// the pane was resolved. It also called `scroll_pane` directly, while the
/// regression was `handle_mouse` reducing a `PaneId` to a `PaneKind`.
#[test]
fn the_wheel_scrolls_the_pane_under_the_pointer() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 12); // long enough to scroll
    let long = app.conversation().episode().id;
    let short = add_persisted_session(&mut app, "B0");
    app.reset_foreground(); // let go of both, so each :open replays a real transcript

    app.set_focus(PaneKind::Output);
    let here = app.workspace.focused();
    app.split_command("output", SplitDir::Horizontal);
    let there = app.workspace.focused();
    app.open_by_hash(&short.to_hex()[..8]);
    app.workspace.focus_pane(here);
    app.open_by_hash(&long.to_hex()[..8]);

    assert!(
        app.pane_session(here).output.line_count() > 10,
        "the pane under the pointer has something to scroll"
    );
    assert!(app.pane_viewport(here).is_following_tail());
    assert!(app.pane_viewport(there).is_following_tail());

    // Point at `here` while `there` is focused: kind-keyed routing would have
    // moved whichever transcript came first / was in front.
    app.workspace.focus_pane(there);
    app.wheel_scroll_at(here, true, (40, 6));

    assert!(
        !app.pane_viewport(here).is_following_tail(),
        "the pane I pointed at scrolled"
    );
    assert!(
        app.pane_viewport(there).is_following_tail(),
        "and the focused pane, which I did not point at, did not"
    );
}

/// **A transcript mirror reset elsewhere is copied whole** — the mirror
/// copies only what changed since its last copy (buffers review, B F5), as
/// it recorded then; a mirror reset by another hand holds less than that
/// record says, and keeping "what is still right" left it one empty line.
#[test]
fn a_mirror_reset_elsewhere_is_copied_whole() {
    let mut app = test_app();
    for i in 0..3 {
        app.output_mut().begin_turn(&format!("question {i}"));
        app.output_mut().append_chunk(&format!("reply {i}"));
        app.output_mut().end_turn();
    }
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.chat_dirty = true;
    app.before_frame(80, 24);
    let lines = app.pane_session(pane).output.lines();
    assert!(lines.len() > 1, "premise: a transcript");
    assert_eq!(app.pane_chat(pane).line_count(), lines.len(), "premise");
    app.window_of_mut(pane)
        .unwrap()
        .session_view_mut()
        .unwrap()
        .chat = Buffer::default();
    app.chat_dirty = true;
    app.before_frame(80, 24);
    assert_eq!(app.pane_chat(pane).line_count(), lines.len(), "whole again");
}

/// **…and a mirror the app forgets is forgotten with its record** — a
/// one-line transcript's record says one line, as an empty buffer has, so
/// the cross-check above cannot tell the two apart and yank and search met
/// an empty line (review stop 4, A2 6).
#[test]
fn a_forgotten_one_line_mirror_is_copied_again() {
    let mut app = test_app();
    app.output_mut().append_chunk("hello");
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.chat_dirty = true;
    app.before_frame(80, 24);
    assert_eq!(
        app.pane_session(pane).output.lines(),
        ["hello"],
        "premise: one line"
    );
    assert_eq!(app.pane_chat(pane).line_count(), 1, "premise: mirrored");
    app.forget_cursor(pane);
    app.chat_dirty = true;
    app.before_frame(80, 24);
    let mirrored: Vec<String> = app.pane_chat(pane).lines().collect();
    assert_eq!(mirrored, ["hello"]);
}
