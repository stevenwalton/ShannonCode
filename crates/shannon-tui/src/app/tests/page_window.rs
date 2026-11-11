//! A page's window: its cursor and the window following it, clamping,
//! counts, `H` `M` `L` and `z`, the keys a vim hand reaches for, leaving it
//! (`:q`, a chord left behind), and `:help`'s completion.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **Every read-only pane navigates the same way: the cursor travels through
/// the window, and the window follows only when it must.**
///
/// This is the (67) property, asserted across the kinds rather than in one of
/// them — the whole finding was that the transcript, the graph and the text
/// panes each had their own answer, and two of the three were correct in
/// different spellings while the third had no cursor at all.
///
/// Written as a loop over kinds on purpose: a per-kind test is what let them
/// diverge, since each one passed about itself. The kinds come from the type,
/// by the property this asks of them — a cursor on a character, which the
/// transcript and every text page have: the loop named three of twelve
/// (review stop 12, T3 14). Filtered by that property, not by read-only less
/// a name: `:graph` is read-only with a row selection for a cursor, and a
/// later row pane would have dropped in unasked (stop 13, A 3).
#[tokio::test]
async fn every_read_only_pane_moves_a_cursor_and_lets_the_window_follow() {
    for kind in PaneKind::all()
        .into_iter()
        .filter(|k| k.is_text_page() || *k == PaneKind::Output)
    {
        let mut app = test_app();
        open_pane_of(&mut app, kind);
        assert_eq!(app.focus(), kind, "{kind:?} is focused");
        let pane = app.workspace.focused();
        // Each kind's own home for its cursor, and the line count it moves
        // over: a text page's `TextView`, the transcript's `Buffer`.
        let at = |app: &mut App| match kind {
            PaneKind::Output => (
                app.pane_chat(pane).cursor().0,
                app.pane_chat(pane).line_count(),
            ),
            _ => (
                app.text_pane_pos_for_test(pane).expect("content").0,
                app.text_pane_len_for_test(pane).expect("content"),
            ),
        };
        assert!(
            at(&mut app).1 >= 2,
            "premise: {kind:?} has a second line for `j` to reach"
        );
        // Normalise the starting point: `:messages` opens at the NEWEST line
        // (pinned separately below), so without this the loop would be asking
        // one kind to move past its last line.
        //
        // **`gg`, not `g`.** A single `g` used to jump to the top, because the
        // pane's hand-written match treated it as a whole command; through the
        // shared keymap `g` is a PREFIX, as in vim. That is the fidelity the
        // routing buys, and it is visible right here in the test.
        type_keys(&mut app, "gg");
        assert_eq!(at(&mut app).0, 0, "{kind:?}: `gg` is the top");
        let start = app.text_pane_scroll_for_test(pane);

        type_keys(&mut app, "j");
        assert_eq!(
            at(&mut app).0,
            1,
            "{kind:?}: one `j` moves the cursor one line"
        );
        // The transcript's window follows its tail, a rule of its own
        // (`going_to_the_end_keeps_following_the_tail`, `viewport.rs`).
        if kind != PaneKind::Output {
            assert_eq!(start, Some(0), "{kind:?}: `gg` puts the window at the top");
            assert_eq!(
                app.text_pane_scroll_for_test(pane),
                start,
                "{kind:?}: and `j` does NOT move the window — the cursor is inside it"
            );
        }
    }
}

/// `:messages` opens on the NEWEST message, cursor and all — the one you came
/// for is the one the status row just cut in half, and it is the last.
#[tokio::test]
async fn messages_opens_with_the_cursor_on_the_newest() {
    let mut app = test_app();
    for i in 0..80 {
        app.notify(format!("message {i}"));
    }
    app.open_messages_command();
    // The reveal is HELD until a width is known, and resolved by the clamp the
    // renderer runs before each draw — so simulate the frame rather than a
    // keypress, which would also move the cursor.
    app.clamp_text_panes(80, 24);
    let pane = app.workspace.focused();
    let (cursor, _) = app.text_pane_pos_for_test(pane).expect("content");
    assert_eq!(
        cursor, 79,
        "the cursor is on the last message, not the first"
    );
}

/// **Every kind that holds a text page is clamped — asked of the TYPE.**
///
/// `clamp_text_panes` once hand-listed `help` and `analyze` under a comment
/// claiming it was "not a hand-list of which kinds are static". `Messages` was
/// added and never clamped, so its reveal never resolved and a resize could
/// leave it past its own end, rendering blank.
///
/// The first version of THIS test then hand-listed three kinds under a comment
/// claiming it walked every one — and missed `PaneKind::Tool`, whose arm in
/// `text_pane_mut` is live and whose *only* consumer is this very clamp sweep
/// (its interactive scrolling goes through `scroll_pane`'s own arm). Mutating
/// that arm to `None` left the entire suite green: a long approval report,
/// scrolled near its end, then the terminal shrunk, could render blank with
/// nothing to catch it (Reviewer C).
///
/// So this starts from `PaneKind::all()` and asserts BOTH directions: exactly
/// this set of kinds holds a text page, and every one of them survives a
/// resize. A kind that gains or loses a page fails here — which is the
/// difference between a list and a check ([[feedback_enumeration_means_unmodeled]]).
#[tokio::test]
async fn every_kind_that_holds_a_text_page_is_clamped() {
    let mut holds_text = Vec::new();

    for kind in PaneKind::all() {
        let mut app = test_app();
        // Every kind opened; those that hold no text page answer `None` below.
        open_pane_of(&mut app, kind);
        let pane = (app.workspace.first_of(kind)).expect("every kind opens");
        // **The predicate, held to what the window holds.** The tests that
        // walk `is_text_page` cannot see it lose a member — the member leaves
        // their loop: `Scripts` dropped from it passed all 2530 (review stop
        // 12, Tier 3, by mutation), and its `<C-f>` would have been dead.
        let holds = app.text_pane_pos_for_test(pane).is_some();
        assert_eq!(
            holds,
            kind.is_page(),
            "{kind:?}: `is_page` disagrees with its window"
        );
        if !holds {
            continue;
        }
        holds_text.push(kind);

        // Drive it to the end, then shrink the frame under it and let the
        // per-frame sweep settle it — the resize-recovery this door exists for.
        app.clamp_text_panes(80, 40);
        if let Some(text) = app.text_pane_mut_for_test(pane) {
            text.cursor_bottom(0);
        }
        app.clamp_text_panes(40, 8);

        let (cursor, scroll) = app.text_pane_pos_for_test(pane).expect("content");
        // At the pane's own width: a companion halves it (`:themes`).
        let width = app.pane_content_size(pane, Rect::new(0, 0, 40, 8)).0;
        let rows = app.text_pane_rows_for_test(pane, width).expect("content");
        let len = app.text_pane_len_for_test(pane).expect("content");
        assert!(
            (scroll as usize) <= rows,
            "{kind:?}: scrolled past its own end after a resize ({scroll} of {rows})"
        );
        assert!(
            cursor < len.max(1),
            "{kind:?}: the cursor is past the last line"
        );
    }

    assert_eq!(
        holds_text,
        // In `PaneKind::all`'s order: the companions, then each page. The
        // inspector too, now it is opened: it was never, so its clamp was
        // never measured (review stop 12, Tier 3).
        vec![
            PaneKind::Tool,
            PaneKind::Inspector,
            PaneKind::ThemePreview,
            PaneKind::Page(PageKind::Analyze),
            PaneKind::Page(PageKind::Help),
            PaneKind::Page(PageKind::Messages),
            PaneKind::Page(PageKind::Buffers),
            PaneKind::Page(PageKind::Aliases),
            PaneKind::Page(PageKind::Rules),
            PaneKind::Page(PageKind::Scripts),
            PaneKind::Page(PageKind::Themes),
            PaneKind::Page(PageKind::Tabs),
            PaneKind::Page(PageKind::Marks),
            // A page you change is clamped as any page is (`clamp_text_panes`
            // walks its window's `TextView`, which an edit restyles).
            PaneKind::Page(PageKind::Tutor),
        ],
        "the set of kinds holding a text page changed; if that is deliberate, \
         the clamp sweep and this list both want looking at"
    );
}

/// **`:q` on a borrowed pane gives the transcript back; it does not leave.**
///
/// Reviewer A reported `quit_window`'s `!focused_pane_gives_back()` guard as
/// unpinned — deleting it left all 1040 tests green. Trying to pin it found out
/// why: **the branch it guards appears to be unreachable.** `pane_count()`
/// counts the prompt bar, and `close_pane_command` refuses to close the only
/// prompt, so a workspace cannot get down to one slot; `pane_count() == 1`
/// short-circuits the condition before the guard is consulted.
///
/// So this pins the behaviour a user can actually produce, and says plainly
/// that it does NOT pin the guard. Deleting the guard today changes nothing
/// observable, which makes it either dead code or a control waiting on a state
/// the workspace cannot currently reach — and the state in question is one
/// Reviewer A separately reported as a defect (a viewer can borrow the last
/// prompt pane, leaving nowhere to type). Resolving the two together is the
/// honest order; claiming coverage here would be the vacuity this suite hunts
/// ([[feedback_mutation_check_guards]]).
///
/// **Resolved at review stop 10 (F 2):** the prompt bar became dedicated (it
/// hosts no view, V5.3), so the state the guard waited on cannot arise — the
/// last pane is the prompt, with nothing to give back — and the guard went.
#[test]
fn quitting_a_borrowed_pane_gives_it_back_instead_of_leaving() {
    let mut app = test_app();
    // A transcript pane the viewer will BORROW rather than a pane it creates —
    // lending is the condition, so the setup has to produce it.
    app.split_command("output", SplitDir::Vertical);
    let pane = app.workspace.focused();
    app.open_help_command(None);
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Help)),
        "premise: the viewer took over this pane"
    );
    assert!(
        app.gives_back(pane),
        "premise: and borrowed it rather than creating one"
    );

    let leaving = app.quit_window(false);

    assert!(
        !leaving,
        "`:q` on a borrowed pane must not leave the editor"
    );
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Output),
        "…it hands the transcript back"
    );
}

/// **A half-typed graph chord does not survive leaving the graph.**
///
/// The chord slot was modelled on keymaps, and `cancel_chords` enumerated the
/// two keymap FIELDS. `:graph` holds a third, so a prefix armed in the tree
/// survived `<C-c>`, `<C-l>`, `PgUp`/`PgDn`, an asynchronously-arriving
/// `y`/`n` question, the command line, and a `<C-w>` trip to another pane and
/// back — six routes, none of which reaches the graph's own dispatcher, which
/// is where the prefix is taken.
///
/// The user-visible version: arm `m`, move to another pane, come back, press
/// `f`, and a row silently marks. `<C-w>` is not plain, so the graph never sees
/// it (Reviewers A and D, both executed).
///
/// Driven through `cancel_chords` and `focus_moved` — the two doors — rather
/// than through one of the six routes, because the routes are what keeps
/// growing ([[feedback_one_door_per_invariant]]).
#[test]
fn a_graph_chord_does_not_survive_leaving_the_pane() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    app.conversation_mut().begin_turn("a question");
    app.conversation_mut().push_chunk("an answer");
    app.conversation_mut().commit_turn();
    open_graph(&mut app);

    for (door, abandon) in [
        (
            "cancel_chords",
            (|a: &mut App| a.cancel_chords()) as fn(&mut App),
        ),
        ("focus_moved", |a: &mut App| a.focus_moved()),
    ] {
        app.graph_key_for_test(k(KeyCode::Char('m')));
        abandon(&mut app);
        app.graph_key_for_test(k(KeyCode::Char('f')));
        assert_eq!(
            app.graph_view().map(|v| v.marked_nodes().len()),
            Some(0),
            "a stale `m` survived {door}, so `f` completed it as `mf`"
        );
    }

    // …and the prefix still works when nothing abandoned it, so the fix did
    // not simply disarm the chord machinery.
    app.graph_key_for_test(k(KeyCode::Char('m')));
    app.graph_key_for_test(k(KeyCode::Char('f')));
    assert_eq!(
        app.graph_view().map(|v| v.marked_nodes().len()),
        Some(1),
        "premise: `mf` marks when the chord is left alone"
    );
}

/// **A viewer is a window: a bare key does not close it, `:q` does.**
///
/// This one ran both ways, and the history is the point. The status row
/// advertised `q close` while only `<Esc>` did it, so `q` — what a vim user
/// reaches for in `:help` — was a dead key under a legend promising otherwise.
/// That was fixed by making `q` close. Then the model settled: these panes are
/// **windows**, and vim closes a window with `:q`. Checked against the shipped
/// runtime rather than remembered: vim's `ftplugin/help.vim` contains no
/// mappings at all, and netrw's `q` is a *prefix* (`qb`, `qf`, `qF`, `qL`) with
/// no bare binding. Spending `q` here would cost us that family and give us
/// panes that close unlike every other window in the app.
///
/// Both halves are asserted. "`q` stopped closing" is only half a feature — if
/// `:q` does not close it, the pane is a trap, which is a defect this codebase
/// has shipped before (the approval pane that trapped focus).
#[tokio::test]
async fn a_text_pane_closes_with_the_command_not_a_bare_key() {
    // Every text page, from the type: this named three of eleven (review
    // stop 12, T3 14).
    for kind in (PaneKind::all().into_iter()).filter(|k| k.is_text_page()) {
        let mut app = test_app();
        open_pane_of(&mut app, kind);
        let pane = app.workspace.focused();
        assert_eq!(app.focused_viewer(), Some(kind), "premise: {kind:?} is up");

        // Ordinary keys now — they reach the shared keymap like any other.
        for code in [KeyCode::Char('q'), KeyCode::Esc] {
            app.handle_text_pane_key_for_test(
                kind,
                KeyEvent::new(code, KeyModifiers::NONE),
                (80, 20),
            );
            assert_eq!(
                app.focused_viewer(),
                Some(kind),
                "{kind:?}: {code:?} must not close a window"
            );
        }

        // The window, not the focus: a preview's or an inspector's close
        // hands focus to the viewer that opened it, still a viewer; and a
        // page over a borrowed window gives it back (`:analyze` over the
        // transcript leaves the transcript), so the window may remain. The
        // last window's `:q` leaves instead — the tutor's, over nothing
        // (`App::last_window`) — and only the last window's.
        let last = app.last_window();
        let left = app.dispatch_command_for_test("q");
        assert_eq!(
            left, last,
            "{kind:?}: `:q` leaves from the last window only"
        );
        if !last {
            assert_ne!(
                app.kind_of(pane),
                Some(kind),
                "{kind:?}: `:q` did not close it"
            );
        }
    }
}

/// **Bare `f`, `b`, `d` and `u` no longer do what their Ctrl-chords do.**
///
/// The pane's hand-written key match read `key.code` and dropped the
/// modifiers, so `f` paged the document exactly as `<C-f>` does. In vim those
/// are find-char, back-word, delete and undo — a vim user pressing `f` to jump
/// to a character got a page instead, which is the "one verb, two behaviours"
/// trap with the second behaviour belonging to a different key entirely
/// (`[[feedback_teardown_doors]]`).
#[tokio::test]
async fn bare_letters_do_not_mean_their_ctrl_chords() {
    let mut app = test_app();
    app.open_help_command(None);
    // Settle the geometry, then remember where we are.
    app.clamp_text_panes(80, 24);
    let pane = app.workspace.focused();
    let before = app.text_pane_pos_for_test(pane).expect("content");

    for c in ['f', 'b', 'd', 'u'] {
        app.handle_text_pane_key_for_test(
            PaneKind::Page(PageKind::Help),
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            (80, 20),
        );
        assert_eq!(
            app.text_pane_pos_for_test(pane).expect("content"),
            before,
            "bare `{c}` moved the page; in vim it is not a page key"
        );
    }

    // …while the chord it was impersonating still works — and takes the
    // CURSOR with it.
    //
    // Asserted on the cursor specifically, not on `(cursor, scroll)`: a page
    // that moved only the view would change the tuple too, and that is exactly
    // the bug worth pinning. `<C-f>` used to call `scroll_by`, which was
    // harmless while these panes had no cursor and abandons it off-screen the
    // moment they do — after which `j` snaps the page back to wherever the
    // cursor was left ([[feedback_mutation_check_guards]]: the first version of
    // this assertion passed under exactly that mutation).
    app.handle_text_pane_key_for_test(
        PaneKind::Page(PageKind::Help),
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
        (80, 20),
    );
    let (cursor, scroll) = app.text_pane_pos_for_test(pane).expect("content");
    assert!(
        cursor > before.0,
        "`<C-f>` must move the cursor, not just the page"
    );
    assert!(scroll > before.1, "and the window follows it");
}

/// **Counts work, because they were never this pane's to implement.**
///
/// `5j` is one `Move(Down)` with `count == 5`, from the same keymap the
/// transcript uses. The hand-written match had no notion of a count at all, so
/// a digit was simply ignored — the vocabulary a vim user brings did not
/// survive the pane boundary.
///
/// **And the count outlives the event's end** — `5` is one event and `j`
/// another, with `App::settle` between them, as the loop runs. Every page
/// and the decision's report, from the type: this asked `:help` alone,
/// through a seam that never settled (review stop 12, Tier 3).
#[tokio::test]
async fn a_count_prefix_works_in_a_text_pane() {
    for kind in PaneKind::all().into_iter().filter(|k| k.is_page()) {
        let mut app = test_app();
        open_pane_of(&mut app, kind);
        let pane = app.workspace.focused();
        type_keys(&mut app, "gg5j");
        let (cursor, _) = app.text_pane_pos_for_test(pane).expect("content");
        assert_eq!(
            cursor, 5,
            "{kind:?}: `5j` moved five lines, not one and not none"
        );
    }
}

/// **Every page answers the keys a vim hand reaches for** — `j` `k`, `^F`
/// `^B`, `gg` `G`, `:q` — in every text page.
///
/// It was the gate for the pages' key legend, which said `q close` while `q`
/// did nothing; only `<Esc>` closed. The legend left the status row (Steven,
/// 10-14: "We don't need to mention the vim style movements"), and what it
/// promised is still promised by every vim hand's fingers, so the keys are
/// still driven, through the real routing, in every page.
#[tokio::test]
async fn every_page_answers_the_keys_a_vim_hand_reaches_for() {
    // **Membership comes from the predicate, not from a list here.** A fourth
    // text page (the inspector) was added and this gate would have gone on
    // testing three — the same "two lists, one updated" shape that left `<C-f>`
    // dead in `:messages` and, later, in the inspector. A kind that becomes a
    // text page now joins this gate on the day it does; its fixture is
    // `open_pane_of`'s arm, which the compiler asks for.
    let mut ran = 0;
    for kind in PaneKind::all() {
        if kind.is_text_page() {
            every_advertised_key_works_in(kind).await;
            ran += 1;
        }
    }
    assert!(
        ran >= 4,
        "premise — the text pages are populated ({ran} kinds)"
    );
}

/// **`H` `M` `L` land where vim does**, through the real keys — the rule's
/// numbers are pinned against Neovim in `viewport`; this pins the WIRING.
#[test]
fn h_m_l_move_to_the_top_middle_and_bottom_of_a_help_window() {
    let (mut app, pane) = help_at(0);
    keys_in_help(&mut app, "100G");
    let (_, top) = app.text_pane_pos_for_test(pane).expect("a text page");
    let top = top as usize;
    assert!(top > 0, "premise: scrolled into the page, not at its start");

    keys_in_help(&mut app, "H");
    assert_eq!(app.text_pane_pos_for_test(pane).unwrap().0, top, "H");
    keys_in_help(&mut app, "3H");
    assert_eq!(app.text_pane_pos_for_test(pane).unwrap().0, top + 2, "3H");
    keys_in_help(&mut app, "L");
    assert_eq!(app.text_pane_pos_for_test(pane).unwrap().0, top + 23, "L");
    keys_in_help(&mut app, "3L");
    assert_eq!(app.text_pane_pos_for_test(pane).unwrap().0, top + 21, "3L");
    keys_in_help(&mut app, "M");
    assert_eq!(app.text_pane_pos_for_test(pane).unwrap().0, top + 11, "M");
    assert_eq!(
        app.text_pane_pos_for_test(pane).unwrap().1 as usize,
        top,
        "none of them scrolls the window"
    );
}

/// **`zt` `zz` `zb` put the line where `H` `M` `L` then find it** — two
/// independent paths that must agree, at a real `scrolloff`.
#[test]
fn z_places_the_line_where_h_m_l_find_it() {
    for (place, find) in [("zt", "H"), ("zz", "M"), ("zb", "L")] {
        let (mut app, pane) = help_at(3);
        keys_in_help(&mut app, "200G");
        let line = app.text_pane_pos_for_test(pane).unwrap().0;
        keys_in_help(&mut app, place);
        assert_eq!(
            app.text_pane_pos_for_test(pane).unwrap().0,
            line,
            "{place} keeps the cursor"
        );
        keys_in_help(&mut app, "gg");
        keys_in_help(&mut app, "200G");
        keys_in_help(&mut app, place);
        keys_in_help(&mut app, find);
        assert_eq!(
            app.text_pane_pos_for_test(pane).unwrap().0,
            line,
            "after {place}, {find} lands on the same line"
        );
    }
}

/// `20zt`: the count is the line to put at the top (`:h zt`), and `z<CR>`
/// lands the same way.
#[test]
fn a_count_before_z_is_the_line() {
    let (mut app, pane) = help_at(0);
    keys_in_help(&mut app, "20zt");
    assert_eq!(
        app.text_pane_pos_for_test(pane).unwrap(),
        (19, 19),
        "line 20 at the top"
    );
    keys_in_help(&mut app, "40z\r");
    assert_eq!(
        app.text_pane_pos_for_test(pane).unwrap(),
        (39, 39),
        "z<CR> the same"
    );
}

/// **An operator over `H` `M` `L` is refused, not swallowed** (`dH`) — rather
/// than run with a motion that does not move, which would act on the wrong
/// lines. The motions themselves work in the prompt (`prompt_window` tests).
#[test]
fn an_operator_over_the_screen_is_refused_out_loud() {
    let mut app = test_app();
    app.prompt_mut().insert_pasted("one\ntwo\nthree\nfour");
    app.set_focus(PaneKind::Prompt);
    let before = app.prompt().text();
    type_keys(&mut app, "<Esc>");
    app.notices.clear();
    type_keys_sized(&mut app, (80, 10), "dH");
    let notice = app.notice().unwrap_or_default();
    assert!(notice.contains("doesn't work here yet"), "dH: {notice:?}");
    assert_eq!(app.prompt().text(), before, "`dH` changed nothing");
}

/// **`zt` on the last lines scrolls past the end, and stays there** — vim's
/// rule, measured in Neovim (`95Gzt` → top 95; `k` → top 94), which Steven uses
/// to bring text to eye level. The rows past the end draw `~` in vim's
/// `NonText` hue: asserted on the RENDERED cells, since a style that exists
/// only in the model is a style nobody sees.
#[test]
fn zt_at_the_end_scrolls_past_it_and_draws_tildes() {
    use ratatui::layout::Rect;
    let (mut app, pane) = help_at(0);
    // Keys at the SAME geometry the frame will draw: a different size is a
    // resize, and a resize refills the last page (vim does that too — growing
    // the window snaps a past-end view back), which is not what is under test.
    let size = app.pane_content_size(pane, Rect::new(0, 0, 80, 30));
    let keys = |app: &mut App, keys: &str| {
        for c in keys.chars() {
            let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
            app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), key, size);
        }
    };
    keys(&mut app, "G");
    let (last, full_page_top) = app.text_pane_pos_for_test(pane).unwrap();
    // `scroll` counts wrapped ROWS; at this pane's width some 80-column lines
    // wrap, so a line's top row comes from the view, not from its index.
    let row_of = |app: &mut App, line: usize| {
        let text = app.text_pane_mut_for_test(pane).unwrap();
        text.rows_before(line, size.0) as u16
    };
    keys(&mut app, "zt");
    let top = row_of(&mut app, last);
    assert!(
        top > full_page_top,
        "premise: past the full last page ({full_page_top})"
    );
    assert_eq!(
        app.text_pane_pos_for_test(pane).unwrap(),
        (last, top),
        "the last line at the top"
    );
    keys(&mut app, "k");
    let above = row_of(&mut app, last - 1);
    assert_eq!(
        app.text_pane_pos_for_test(pane).unwrap(),
        (last - 1, above),
        "k scrolls up to the line above, as vim does — not back to a full page"
    );

    let buf = &frame_of(&mut app, 80, 30);
    let tilde_rows: Vec<u16> = (0..30)
        .filter(|&y| {
            buf[(1, y)].symbol() == "~" && buf[(1, y)].style().fg == Some(crate::palette::NON_TEXT)
        })
        .collect();
    assert!(
        tilde_rows.len() >= 5,
        "the rows past the end draw `~` in NON_TEXT: {tilde_rows:?}\n{}",
        rendered_text(buf)
    );
}

/// **`:help window-<Tab>` offers the tags** — Steven's report: the manual
/// opens with dozens of them and none completed, while `:help
/// shannon-keymap` typed out worked (the tags lost that prefix, 11-08).
/// Through the real command line.
#[test]
fn help_tab_completes_tags_not_only_chapters() {
    let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let mut app = test_app();
    type_command(&mut app, "help window-");
    app.command_input(tab);
    let (cands, _) = app
        .completion_menu()
        .expect("several tags match `window-`: a menu");
    assert!(
        cands.iter().all(|c| c.starts_with("window-")) && cands.iter().any(|c| c == "window-focus"),
        "{cands:?}"
    );

    let mut app = test_app();
    type_command(&mut app, "help keym");
    app.command_input(tab);
    assert_eq!(
        app.cmdline.line(),
        Some("help keymap"),
        "a unique tag fills in"
    );
}
