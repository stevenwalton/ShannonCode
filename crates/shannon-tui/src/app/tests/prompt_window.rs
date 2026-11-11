//! **The prompt's window** — the cursor pushing it at `scrolloff`, `H` `M`
//! `L` and `zt` `zz` `zb`, through the routes a user's input takes
//! (`key_for_test`, then a frame through `before_frame`).
//!
//! Steven, 2026-10-15, from inside Shannon: "The cursor is stuck to the
//! bottom of the prompt pane… press up (`k`) and the whole pane scrolls with
//! me. The intended action is that we only scroll up when the cursor nears
//! the top." The window top was re-derived from the cursor every frame since
//! M1, so the cursor sat on the bottom row and `zt` `H` were refused.
//!
//! Every line of the fixture is one row, so the expected tops are vim's rules
//! done by hand: `rows` visible, a margin of `scrolloff` clamped to half.

use super::*;

const FRAME: (u16, u16) = (80, 40);

/// A focused prompt of `n` lines, each indented two spaces (so `H` landing
/// on the first non-blank is visible), in Normal mode on the last line, one
/// frame drawn — and the prompt pane's content size in that frame.
fn prompt(n: usize) -> (App, (u16, u16)) {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    let text: Vec<String> = (0..n).map(|i| format!("  line {i}")).collect();
    app.prompt_mut().insert_pasted(&text.join("\n"));
    app.press_for_test(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let layout = app
        .workspace
        .frame_layout(Rect::new(0, 0, FRAME.0, FRAME.1));
    let size = layout
        .panes
        .iter()
        .find(|p| app.workspace.kind_of(p.id) == Some(PaneKind::Prompt))
        .expect("premise: a prompt pane")
        .content_size();
    app.before_frame(FRAME.0, FRAME.1);
    assert!(
        n > 2 * size.1 as usize,
        "premise: room to scroll ({n} lines, {} rows)",
        size.1
    );
    (app, size)
}

/// Keys typed into a prompt of `size` ([`type_keys_sized`]), then a frame.
fn keys(app: &mut App, size: (u16, u16), keys: &str) {
    type_keys_sized(app, size, keys);
    app.before_frame(FRAME.0, FRAME.1);
}

/// The first row the prompt shows, as drawn.
fn top(app: &App, size: (u16, u16)) -> usize {
    app.prompt()
        .visible_window(size.0 as usize, size.1 as usize)
        .top
}

/// The scrolloff margin a `rows`-row window keeps: the setting, at most half.
fn margin(app: &App, rows: usize) -> usize {
    (app.config.tui.scrolloff as usize).min((rows - 1) / 2)
}

/// **`k` from the bottom moves the cursor, not the window** — until the
/// cursor reaches the margin, and THEN each `k` pushes the window up a row.
/// The bug: every `k` scrolled, the cursor glued to the last row.
#[test]
fn the_cursor_pushes_the_prompt_window_up_only_at_the_margin() {
    let (mut app, size) = prompt(60);
    let rows = size.1 as usize;
    let m = margin(&app, rows);
    let bottom = top(&app, size);
    assert_eq!(bottom, 60 - rows, "premise: the last page");
    // Up to the margin, the window stays.
    let free = rows - 1 - m;
    for k in 1..=free {
        keys(&mut app, size, "k");
        assert_eq!(top(&app, size), bottom, "k #{k}: the window stays");
    }
    assert_eq!(
        app.prompt().cursor().0,
        59 - free,
        "premise: the cursor moved"
    );
    // At the margin, a `k` pushes.
    keys(&mut app, size, "k");
    assert_eq!(top(&app, size), bottom - 1, "the next k pushes one row");
}

/// **And `j` pushes it down the same way**: still in the middle, moving at
/// the bottom margin.
#[test]
fn the_cursor_pushes_the_prompt_window_down_only_at_the_margin() {
    let (mut app, size) = prompt(60);
    let rows = size.1 as usize;
    let m = margin(&app, rows);
    keys(&mut app, size, "gg");
    assert_eq!(top(&app, size), 0, "premise: the first page");
    for _ in 0..(rows - 1 - m) {
        keys(&mut app, size, "j");
    }
    assert_eq!(top(&app, size), 0, "inside the band, the window stays");
    keys(&mut app, size, "j");
    assert_eq!(top(&app, size), 1, "at the margin, j pushes");
}

/// **`zt` `zz` `zb` place the cursor's line** — vim's `place_top`, the rule
/// every other window uses; `20zt` goes to line 20 first (`:h zt`).
#[test]
fn zt_zz_zb_place_the_cursor_line_in_the_prompt() {
    let (mut app, size) = prompt(60);
    let rows = size.1 as usize;
    let m = margin(&app, rows);
    keys(&mut app, size, "30G");
    let line = app.prompt().cursor().0;
    assert_eq!(line, 29, "premise");
    keys(&mut app, size, "zt");
    assert_eq!(
        top(&app, size),
        line - m,
        "zt: the line at the top, less the margin"
    );
    keys(&mut app, size, "zb");
    assert_eq!(top(&app, size), line + 1 + m - rows, "zb: at the bottom");
    keys(&mut app, size, "zz");
    assert_eq!(top(&app, size), line - (rows - 1) / 2, "zz: in the middle");
    keys(&mut app, size, "10zt");
    assert_eq!(app.prompt().cursor().0, 9, "the count is the LINE");
    assert_eq!(top(&app, size), 9 - m, "and it goes to the top");
    assert!(
        app.notice().is_none(),
        "nothing refused: {:?}",
        app.notice()
    );
}

/// **`z<CR>` is `zt` that also goes to the first non-blank** (`:h z<CR>`);
/// `zt` leaves the column where it was.
#[test]
fn z_enter_goes_to_the_first_non_blank_and_zt_does_not() {
    let (mut app, size) = prompt(60);
    keys(&mut app, size, "30G$zt");
    let end = app.prompt().cursor().1;
    assert!(end > 2, "premise: the cursor is past the indent ({end})");
    app.key_for_test(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE), size);
    app.key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), size);
    app.before_frame(FRAME.0, FRAME.1);
    assert_eq!(app.prompt().cursor(), (29, 2), "z<CR>: the first non-blank");
}

/// **`H` `M` `L` go by place on the screen**, to the first non-blank — the
/// window does not move.
#[test]
fn h_m_l_go_by_place_on_the_prompt_screen() {
    let (mut app, size) = prompt(60);
    let rows = size.1 as usize;
    let m = margin(&app, rows);
    keys(&mut app, size, "30Gzz");
    let first = top(&app, size);
    keys(&mut app, size, "H");
    assert_eq!(
        app.prompt().cursor(),
        (first + m, 2),
        "H: first line in, first non-blank"
    );
    keys(&mut app, size, "L");
    assert_eq!(app.prompt().cursor().0, first + rows - 1 - m, "L");
    keys(&mut app, size, "M");
    assert_eq!(app.prompt().cursor().0, first + (rows - 1) / 2, "M");
    assert_eq!(top(&app, size), first, "the window did not move");
    assert!(
        app.notice().is_none(),
        "nothing refused: {:?}",
        app.notice()
    );
}

/// **A click lands through the same window** — the mouse's reverse map
/// read the old cursor-derived top, so once the window stopped following
/// the cursor, a click on the first row would have landed rows below it.
#[test]
fn a_click_on_the_prompt_lands_on_the_row_drawn() {
    let (mut app, size) = prompt(60);
    keys(&mut app, size, "kkk");
    let shown = top(&app, size);
    let (w, h) = (size.0 as usize, size.1 as usize);
    assert_eq!(app.prompt().cursor_from_visual(w, h, 0, 2), (shown, 2));
}

// --- Normal-mode "caret on a char" invariant (vim faithfulness) ---

#[test]
fn dollar_and_l_stop_on_the_last_char_in_normal() {
    let mut app = test_app(); // starts Normal
    type_into(&mut app, "abc"); // Insert-style fill leaves caret at (0,3)
    app.enforce_normal_cursor(); // entering Normal: onto 'c' (0,2)
    assert_eq!(app.prompt().cursor(), (0, 2));
    // `$` lands on the last char, not the past-end column.
    type_keys(&mut app, "$");
    assert_eq!(app.prompt().cursor(), (0, 2), "$ on the last char");
    // `l` cannot walk past it.
    type_keys(&mut app, "l");
    assert_eq!(app.prompt().cursor(), (0, 2), "l stops at the last char");
}

#[test]
fn h_and_l_do_not_cross_line_boundaries() {
    let mut app = test_app();
    type_into(&mut app, "ab");
    app.prompt_mut().apply(EditAction::Newline);
    type_into(&mut app, "cd"); // buffer "ab" / "cd", caret (1,2)
    // `h` at column 0 of line 1 stays on line 1.
    type_keys(&mut app, "0");
    type_keys(&mut app, "h");
    assert_eq!(app.prompt().cursor(), (1, 0), "h does not wrap up a line");
    // On line 0's last char, `l` must not drop to line 1.
    type_keys(&mut app, "k"); // (0,0)
    type_keys(&mut app, "$"); // (0,1) on 'b'
    type_keys(&mut app, "l");
    assert_eq!(app.prompt().cursor(), (0, 1), "l does not wrap down a line");
}

#[test]
fn x_at_end_of_line_pulls_caret_onto_the_new_last_char() {
    let mut app = test_app();
    type_into(&mut app, "ab");
    app.enforce_normal_cursor(); // (0,1) on 'b'
    type_keys(&mut app, "x"); // delete 'b'
    assert_eq!(app.prompt().text().lines().next().unwrap(), "a");
    assert_eq!(app.prompt().cursor(), (0, 0), "caret pulled back onto 'a'");
}

#[test]
fn insert_mode_keeps_the_end_of_line_caret_for_append() {
    // The clamp must NOT fire in Insert: appending needs the caret at
    // `len`. `a` enters Insert and steps right; the caret stays past the
    // text so the next char appends.
    let mut app = test_app();
    type_into(&mut app, "ab");
    app.enforce_normal_cursor(); // Normal: (0,1) on 'b'
    type_keys(&mut app, "a"); // append → Insert
    assert_eq!(app.focused_mode(), Mode::Insert);
    assert_eq!(
        app.prompt().cursor(),
        (0, 2),
        "Insert caret rests at len to append"
    );
}

#[test]
fn clamp_skips_the_prompt_while_chat_is_focused() {
    let mut app = test_app();
    type_into(&mut app, "abc"); // caret (0,3)
    app.toggle_focus(); // focus the chat viewer
    app.enforce_normal_cursor();
    assert_eq!(
        app.prompt().cursor(),
        (0, 3),
        "no clamp on the prompt when chat has focus"
    );
}

/// **Bound is not live.** `<C-f>`/`<C-b>`/`<C-d>`/`<C-u>` and PgUp/PgDn were
/// bound in `Context::Normal` and the prompt's scroll arm was a no-op with a
/// comment claiming there was nothing to scroll — so they were dead keys in the
/// pane you are standing in at launch, and the stub's first reply tells a brand
/// new user to press four of them (Reviewer F).
#[test]
fn the_prompt_pages_with_the_keys_that_are_bound_to_page_it() {
    let mut app = test_app();
    app.prompt_mut().insert_pasted(
        &(0..200)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    app.prompt_mut().place_cursor(0, 0);
    let before = app.prompt().cursor().0;

    let pane = app.workspace.focused();
    app.scroll_pane(
        pane,
        crate::keymap::ScrollCmd::Down(crate::keymap::ScrollMagnitude::Page),
        (80, 24),
    );
    let after = app.prompt().cursor().0;

    assert!(
        after > before,
        "a page must move the prompt cursor: {before} -> {after}"
    );
}
