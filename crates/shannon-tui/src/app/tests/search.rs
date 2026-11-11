//! **Search** — `/` `?` `n` `N` `*` `#` through the keys a user presses, in a
//! page, the transcript and the prompt (Steven, 10-07: "We should prioritize
//! searching").

use super::*;

/// `/{pattern}` (or `?`), then Enter.
fn search(app: &mut App, glyph: char, pattern: &str) {
    type_keys(app, &format!("{glyph}{pattern}<CR>"));
}

/// `:help`, focused and drawn once.
fn help() -> (App, crate::wm::PaneId) {
    let mut app = test_app();
    open_pane_of(&mut app, PaneKind::Page(PageKind::Help));
    let pane = app.workspace.focused();
    (app, pane)
}

/// Where the page's cursor is, and the text from there on its line.
fn at(app: &mut App, pane: crate::wm::PaneId) -> ((usize, usize), String) {
    let text = app.text_pane_mut_for_test(pane).expect("a page");
    let (line, col) = text.position();
    let rest = text.lines()[line].text().chars().skip(col).collect();
    ((line, col), rest)
}

/// **`/` lands on the next match; `n` goes on, `N` comes back** — and the
/// search line is drawn after a `/`, the `:` line's own row.
#[test]
fn slash_lands_on_a_match_and_n_and_shift_n_walk_them() {
    let (mut app, pane) = help();
    type_keys(&mut app, "/");
    assert_eq!(app.cmdline.kind().glyph(), '/', "the line is a search");
    type_keys(&mut app, "SPLITTING");
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    draw_frame(&mut app, &mut terminal);
    let buf = terminal.backend().buffer();
    let status: String = (0..10).map(|x| buf[(x, 23)].symbol()).collect();
    assert_eq!(status, "/SPLITTING", "drawn after its `/`");
    type_keys(&mut app, "<CR>");

    let (first, rest) = at(&mut app, pane);
    assert!(rest.starts_with("SPLITTING"), "{first:?}: {rest:?}");

    // `n` and `N` over a pattern with MANY matches — with two, going on
    // from the second wraps to the first, and `N` cannot be told from `n`.
    // From the top, so no step wraps.
    type_keys(&mut app, "gg");
    search(&mut app, '/', "window-");
    let p1 = at(&mut app, pane).0;
    type_keys(&mut app, "n");
    let p2 = at(&mut app, pane).0;
    type_keys(&mut app, "n");
    let p3 = at(&mut app, pane).0;
    assert!(p1 < p2 && p2 < p3, "{p1:?} {p2:?} {p3:?}");
    type_keys(&mut app, "N");
    assert_eq!(at(&mut app, pane).0, p2, "`N` comes back one");
    // An empty pattern is the last one again (vim's `/<CR>`).
    search(&mut app, '/', "");
    assert_eq!(at(&mut app, pane).0, p3, "`/<CR>` goes on");
    // `?` searches up, and `n` then keeps going up.
    search(&mut app, '?', "window-");
    assert_eq!(at(&mut app, pane).0, p2, "`?` goes back");
    type_keys(&mut app, "n");
    assert_eq!(at(&mut app, pane).0, p1, "and `n` after it, further back");
}

/// **Past an end it wraps, and says so; a pattern found nowhere, or not a
/// pattern at all, is said and moves nothing.**
#[test]
fn a_search_wraps_and_says_so_and_a_miss_moves_nothing() {
    let (mut app, pane) = help();
    type_keys(&mut app, "G");
    search(&mut app, '/', "Shannon Code help");
    assert_eq!(at(&mut app, pane).0.0, 0, "round to the top");
    assert_eq!(
        app.notice(),
        Some("search hit BOTTOM, continuing at TOP".to_string())
    );

    let before = at(&mut app, pane).0;
    search(&mut app, '/', "zzqqnowhere");
    assert_eq!(at(&mut app, pane).0, before, "nothing moved");
    // The seam begins a moment per key, as a keypress does, so the row
    // holds this search's word alone (review stop 12, T6 11; it said the
    // seam did not, and read the refusal as the first of several).
    assert!(
        app.notice()
            .is_some_and(|n| n == "pattern not found: zzqqnowhere"),
        "{:?}",
        app.notice()
    );
    // A `^` no line can match there is named, with its escape (S21-Q14).
    search(&mut app, '/', "zzqq^nowhere");
    assert!(
        (app.notice()).is_some_and(|n| n.contains("`^` mid-pattern matches a line's start")),
        "{:?}",
        app.notice()
    );
    search(&mut app, '/', "a(b");
    assert!(
        app.notice().is_some_and(|n| n.contains("is not a pattern")),
        "{:?}",
        app.notice()
    );
    assert_eq!(at(&mut app, pane).0, before);
}

/// `n` with nothing searched yet says what starts one.
#[test]
fn n_before_any_search_says_what_starts_one() {
    let (mut app, _) = help();
    type_keys(&mut app, "n");
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("no previous search")),
        "{:?}",
        app.notice()
    );
}

/// **`*` searches the word under the cursor, whole** — and `n` repeats it.
#[test]
fn star_searches_the_word_under_the_cursor() {
    let (mut app, pane) = help();
    // Line 0 is `*help*   Shannon Code help ... *shannon.txt*`: the
    // keyword at the cursor is `help`, and its next whole occurrence is
    // the one in "Shannon Code help".
    type_keys(&mut app, "*");
    let ((line, col), rest) = at(&mut app, pane);
    assert_eq!(line, 0);
    assert!(col > 1 && rest.starts_with("help "), "{col}: {rest:?}");
}

/// **The transcript searches too**, its cursor landing on the match.
#[tokio::test]
async fn the_transcript_finds_a_pattern() {
    let mut app = test_app();
    app.submit("alpha beta gamma");
    app.feed_stream(StreamMsg::Chunk("ok".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.before_frame(80, 24);
    type_keys(&mut app, "gg");
    search(&mut app, '/', "gam+a");
    let (row, col) = app.chat_mut().unwrap().cursor();
    let line = app.chat_mut().unwrap().lines().nth(row).expect("a line");
    let rest: String = line.chars().skip(col).collect();
    assert!(rest.starts_with("gamma"), "{rest:?}");
}

/// **And the prompt** — vim's `/` in a buffer you are editing.
#[test]
fn the_prompt_finds_a_pattern() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    app.prompt_mut().replace_with("one two one");
    type_keys(&mut app, "0");
    search(&mut app, '/', "one");
    assert_eq!(app.prompt().cursor(), (0, 8), "the second `one`");
    search(&mut app, '?', "two");
    assert_eq!(app.prompt().cursor(), (0, 4));
}

/// **`[tui] ignorecase` and `smartcase`, as vim's** (Steven, 10-30: "In vim
/// `/mara` searches both `mara` and `Mara` but `/Mara` only searches
/// `Mara`" — his vim sets both; the defaults stay vim's, off). `*` ignores
/// case without `smartcase`, its word not typed, and so do `n` and `/<CR>`
/// after it (vim's `no_scs`).
#[test]
fn ignorecase_and_smartcase_are_vims() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    // `Mara` at 2, `mara` at 7, `MARA` at 12.
    app.prompt_mut().replace_with("x Mara mara MARA");
    let from_start = |app: &mut App, pattern| {
        type_keys(app, "0");
        search(app, '/', pattern);
        app.prompt().cursor().1
    };
    assert_eq!(
        from_start(&mut app, "mara"),
        7,
        "vim's default: case matched"
    );
    app.dispatch_command_for_test("set tui.ignorecase");
    assert_eq!(from_start(&mut app, "MARA"), 2, "ignorecase");
    app.dispatch_command_for_test("set tui.smartcase");
    assert_eq!(from_start(&mut app, "mara"), 2, "smartcase, no capital");
    assert_eq!(from_start(&mut app, "MARA"), 12, "smartcase, a capital");
    assert_eq!(from_start(&mut app, "MARA\\c"), 2, "\\c decides");
    // `*` on `MARA`: its word ignores case, so the next is `Mara`, round
    // the end; `n` then `mara`, still ignoring — with `smartcase` both would
    // find `MARA` alone.
    type_keys(&mut app, "$b*");
    assert_eq!(app.prompt().cursor(), (0, 2), "`*`: no smartcase");
    type_keys(&mut app, "n");
    assert_eq!(app.prompt().cursor(), (0, 7), "nor for `n` after it");
    search(&mut app, '/', "");
    search(&mut app, '/', "");
    assert_eq!(
        app.prompt().cursor(),
        (0, 2),
        "nor `/<CR>`: round to `Mara`"
    );
}
