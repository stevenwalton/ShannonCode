//! A window's history: the alternate, `:b#`, `<C-^>`, and where a window lands.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// --- P3: a pane can show another session --------------------------------
//
// The headline of the buffer model: two panes, two sessions, one app. Many of
// these drive `open_by_hash` — the `:open <hash>` path, which is also what
// `<CR>` in `:graph` runs; the others reach a second session another way. (It said
// "every test here", true of the section it headed in `tests.rs`, not of the
// module it heads now — review stop 13, C 4.)

/// **`:b N` brings back the view it names, of those the window holds** —
/// found in the history by its buffer, not the first view there (V5.3
/// (3a); a mutation finding any view passed while every test held one).
#[test]
fn b_n_brings_back_the_held_view_it_names() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let here = app.workspace.focused();
    // The window's session, beneath the views (V5.3 (3c)).
    let beneath = format!("session b{}", app.live_session(here).unwrap().number());
    for command in ["help", "ls", "graph"] {
        app.dispatch_command_for_test(command);
    }
    assert_eq!(
        history_of(&app, here),
        [beneath.as_str(), "view help", "view ls"],
        "premise: two views held, the one named the later"
    );
    let ls = app
        .page_of_kind(PaneKind::Page(PageKind::Buffers))
        .expect(":ls's page")
        .id();
    app.dispatch_command_for_test(&format!("b{}", ls.number()));
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Page(PageKind::Buffers)),
        "the view it names"
    );
    assert_eq!(
        history_of(&app, here),
        [beneath.as_str(), "view help", "view graph"],
        "the other kept where it was, the tree set aside in its place"
    );
}

/// **`<C-^>` back to a session makes it the active one, as `:b N` to it
/// does** — `HEAD` follows, so a launch resuming the last reopens it; it
/// stayed on the session left (review stop 10, A 5).
#[test]
fn ctrl_hat_to_a_session_moves_head_as_b_n_does() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _left, right) = app_with_two_sessions(dir.path());
    app.focus_pane(right);
    app.dispatch_command_for_test(&format!("b {}", &a.to_hex()[..8]));
    app.dispatch_command_for_test(&format!("b {}", &b.to_hex()[..8]));
    assert_eq!(
        app.store.get_ref("HEAD").unwrap(),
        Some(b),
        "premise: `:b` moved it"
    );
    app.dispatch_command_for_test("b#");
    let shown = app.pane_session(right).conversation.episode().id;
    assert_eq!(shown, a, "premise: `<C-^>` went back to A");
    assert_eq!(
        app.store.get_ref("HEAD").unwrap(),
        Some(a),
        "`<C-^>` to A records A"
    );
}

/// **A session entered where it is shown leaves the window as it was** —
/// `:new` on the blank session it shows stays on it, sets nothing aside,
/// and `<C-^>` has nothing to go back to (`Window::enter`'s "already shows
/// it"; review stop 10, A 1).
#[test]
fn new_that_stays_on_a_blank_session_leaves_no_alternate() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.new_session_command();
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("already on a fresh session")),
        "premise: :new stayed"
    );
    assert!(
        app.window_of(pane).is_some_and(|w| w.history.is_empty()),
        "the window set nothing aside: {:?}",
        history_of(&app, pane)
    );
    app.dispatch_command_for_test("b#");
    assert_eq!(
        app.notice().as_deref(),
        Some(NO_ALTERNATE),
        "nothing to go back to"
    );
}

/// **A tree given back over `:themes` puts its inspector where it was** —
/// in the space the list's preview had, which the reconcile ending `:q`
/// closes (V5.4 (2)). Written to measure whether releasing the preview
/// BEFORE the inspector is paired mattered: it did not — the same panes in
/// the same places without it — so the release went, and this holds what
/// the user sees.
#[test]
fn a_tree_given_back_puts_its_inspector_where_it_was() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let body = ratatui::layout::Rect::new(0, 0, 120, 40);
    let inspector_rect = |app: &App| {
        let inspector = (app.companions_of(pane))
            .find(|&c| app.workspace.kind_of(c) == Some(PaneKind::Inspector))
            .expect("an inspector");
        app.workspace
            .frame_layout(body)
            .pane(inspector)
            .expect("laid out")
            .rect
    };
    app.dispatch_command_for_test("graph");
    let first = inspector_rect(&app);
    app.dispatch_command_for_test("themes");
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Graph),
        "premise: the tree back"
    );
    assert_eq!(inspector_rect(&app), first, "its inspector where it was");
}

/// **A mark spent elsewhere stays spent when a tree set aside comes back**
/// — by `:q`, `:b#` or `:b N`: the window's view is set aside under `:help`
/// while another window on the same tree spends the mark, and the view
/// comes back showing the tree's marks as they are NOW, not the ones it
/// held, nor giving them back to the tree (V4b verify H1; V5.3 verify Q6,
/// whose probe this is — every way back comes through `bring_back`).
#[test]
fn a_mark_spent_elsewhere_stays_spent_when_the_tree_comes_back() {
    let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    for back in ["q", "b#", "bN"] {
        let dir = TempDir::new().unwrap();
        let (mut app, a, _, here, other) = app_with_two_sessions(dir.path());
        let tree = |app: &App, pane| app.pane_graph(pane).map(GraphView::tree);
        let marks = |app: &App, pane| app.pane_graph(pane).unwrap().marked_nodes();
        app.focus_pane(here);
        app.dispatch_command_for_test("graph");
        let t = tree(&app, here).expect("premise: a tree");
        app.graph_view_mut().unwrap().reveal_and_select_episode(a);
        app.graph_key_for_test(k(' '));
        assert_eq!(
            marks(&app, here),
            vec![Node::Episode(a)],
            "{back}: premise: marked"
        );
        app.dispatch_command_for_test("help");
        app.focus_pane(other);
        app.dispatch_command_for_test("graph");
        assert_eq!(
            tree(&app, other),
            Some(t),
            "{back}: premise: the same tree, set aside under `here`'s help"
        );
        assert_eq!(
            marks(&app, other),
            vec![Node::Episode(a)],
            "{back}: premise: the tree's mark shown"
        );
        app.graph_view_mut().unwrap().reveal_and_select_episode(a);
        app.graph_key_for_test(k(' '));
        assert!(marks(&app, other).is_empty(), "{back}: premise: spent");
        app.focus_pane(here);
        let quit = match back {
            "bN" => app.dispatch_command_for_test(&format!("b{}", t.number())),
            cmd => app.dispatch_command_for_test(cmd),
        };
        assert!(!quit, "{back}: premise: the command did not quit");
        app.settle();
        assert_eq!(tree(&app, here), Some(t), "{back}: premise: back");
        assert!(
            marks(&app, here).is_empty(),
            "{back}: the spent mark stays spent: {:?}",
            marks(&app, here)
        );
        app.focus_pane(other);
        app.settle();
        assert!(
            marks(&app, other).is_empty(),
            "{back}: and is not given back to the tree"
        );
    }
}

/// **Going back to a session puts the window where it was in it** — its
/// cursor and its scroll (Steven, 10-25, V4b Q2: vim's `:b`); a window that
/// never showed it starts at the end, as before. It started over every time
/// (`show_buffer_in` forgot the cursor). A session whose rows were rewritten
/// since (`forget_cursors_on`) starts over: the place indexed rows now gone.
#[test]
fn going_back_to_a_session_puts_the_window_where_it_was() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, here, _) = app_with_two_sessions(dir.path());
    let episode = |app: &App| app.pane_session(here).conversation.episode().id;
    app.focus_pane(here);
    app.open_by_hash(&a.to_hex()[..8]);
    let was = stand_back_in(&mut app, here);

    app.open_by_hash(&b.to_hex()[..8]);
    assert_eq!(episode(&app), b, "premise: B here");
    assert!(
        app.pane_viewport(here).is_following_tail(),
        "a window new to B follows its end"
    );

    app.open_by_hash(&a.to_hex()[..8]);
    assert_eq!(episode(&app), a);
    let vp = app.pane_viewport(here);
    assert_eq!(
        (app.pane_chat(here).cursor(), vp.top()),
        was,
        "where it was in A"
    );
    assert!(!vp.is_following_tail(), "still scrolled back");

    let a_buffer = app.pane_session(here).id;
    app.open_by_hash(&b.to_hex()[..8]);
    app.forget_cursors_on(a_buffer);
    app.open_by_hash(&a.to_hex()[..8]);
    assert!(
        app.pane_viewport(here).is_following_tail(),
        "A rewritten: at its end again"
    );
}

/// **Every door a window comes to a session it never showed by puts it at
/// the session's entry** — its end, today (`SessionBuffer::entry`; Steven,
/// 10-26: "keep it at the end for now… allow markers to specify where").
/// Each of these doors was at the end by construction (a window born, a
/// place made from nothing); they ask the buffer now, so these hold that
/// they ASK: with the entry mutated to the top, each goes red (`enter` and
/// the forget are held by `going_back_to_a_session_puts_the_window_where_it_was`
/// and `a_deleted_session_leaves_no_place_behind`).
///
/// A pane made for a session no pane shows (`App::session_host`): `:b N`
/// from the prompt bar, the only transcript closed.
#[test]
fn a_session_shown_in_a_pane_made_for_it_starts_at_its_entry() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 12);
    let session = app.prompt_target();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("close");
    assert_eq!(
        app.workspace.panes_of(PaneKind::Output).count(),
        0,
        "premise: no transcript"
    );
    app.set_focus(PaneKind::Prompt);
    app.dispatch_command_for_test(&format!("b{}", session.number()));
    let pane = (app.workspace.panes_of(PaneKind::Output).next()).expect("premise: made");
    assert!(app.pane_viewport(pane).is_following_tail(), "at its end");
}

/// A transcript split from a viewer, on the aim it never showed.
#[test]
fn a_transcript_split_from_a_viewer_starts_at_its_entry() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 12);
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("sgraph");
    assert_eq!(
        app.focus(),
        PaneKind::Graph,
        "premise: a tree of its own pane"
    );
    app.split_command("output", SplitDir::Horizontal);
    let pane = app.workspace.focused();
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Output),
        "premise: split"
    );
    assert!(app.pane_viewport(pane).is_following_tail(), "at its end");
}

/// The screen the width tests draw at: a full pane wide enough that a reply
/// wraps once, and twice as often at half of it.
const WIDE: (u16, u16) = (120, 40);

/// Thirty turns of long replies over the store, drawn at [`WIDE`], the
/// transcript focused and scrolled back from its end: its pane.
///
/// **Screens back, not rows**: at 12 rows the shift a narrower width makes
/// stayed inside the window misread at it, so a cursor dragged into that
/// window never moved and a drag before the re-anchor passed every test
/// here (review stop 15, A H1 / B H1 — an assumed margin). 60 rows is
/// past the 38 a [`WIDE`] window shows.
fn scrolled_back_wide(app: &mut App) -> crate::wm::PaneId {
    for i in 0..30 {
        app.submit(&format!("ask {i}"));
        let reply = format!(
            "reply {i}: {}",
            "the quick brown fox jumps over the lazy dog ".repeat(4)
        );
        app.feed_stream(StreamMsg::Chunk(reply));
        app.feed_stream(StreamMsg::Complete);
    }
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    frame_of(app, WIDE.0, WIDE.1);
    top_at_row(app, pane, WIDE, 60);
    // The cursor in the window, as a scroll drags it: under its top line.
    let (top, _) = top_of(app, pane, WIDE);
    app.pane_chat_mut(pane).unwrap().place_cursor(top + 1, 0);
    frame_of(app, WIDE.0, WIDE.1);
    pane
}

/// `pane`'s top put on row `row` of a `frame`-sized screen — frozen on the
/// text there, as a scroll freezes it.
fn top_at_row(app: &mut App, pane: crate::wm::PaneId, frame: (u16, u16), row: usize) {
    let (w, _) = app.pane_content_size(pane, Rect::new(0, 0, frame.0, frame.1));
    let shut = app.pane_shut(pane);
    let (line, col) = (app.pane_session(pane).output).visual_to_logical(&shut, row, 0, w);
    let view = app.window_of_mut(pane).unwrap().session_view_mut().unwrap();
    view.viewport.freeze_at(line, col);
}

/// The row at `pane`'s top, on a `frame`-sized screen.
fn top_row_of(app: &App, pane: crate::wm::PaneId, frame: (u16, u16)) -> usize {
    let (w, h) = app.pane_content_size(pane, Rect::new(0, 0, frame.0, frame.1));
    let output = &app.pane_session(pane).output;
    let vp = app.pane_viewport(pane);
    output
        .top_row(&vp, &app.pane_shut(pane), w, usize::from(h))
        .0
}

/// The line and column at `pane`'s top, on a `frame`-sized screen.
fn top_of(app: &App, pane: crate::wm::PaneId, frame: (u16, u16)) -> (usize, usize) {
    let (w, _) = app.pane_content_size(pane, Rect::new(0, 0, frame.0, frame.1));
    let top = top_row_of(app, pane, frame);
    (app.pane_session(pane).output).visual_to_logical(&app.pane_shut(pane), top, 0, w)
}

/// Whether `pane`'s first row, on a `frame`-sized screen, holds the
/// character at `at` (a line and column) — the same text at its top,
/// whatever column its row now starts at.
fn at_top(app: &App, pane: crate::wm::PaneId, frame: (u16, u16), at: (usize, usize)) -> bool {
    let (w, _) = app.pane_content_size(pane, Rect::new(0, 0, frame.0, frame.1));
    let output = &app.pane_session(pane).output;
    let row = output
        .logical_to_visual(&app.pane_shut(pane), at.0, at.1, w)
        .0;
    row == top_row_of(app, pane, frame)
}

/// **A `:vsplit` keeps a scrolled window's top line, in both windows** —
/// as vim keeps `topline`. Its scroll was a row, and a row is a width's:
/// halved, the same row stood earlier in the text, and the window jumped
/// back (Steven, 11-01, images 7–8).
#[tokio::test]
async fn a_vsplit_keeps_a_scrolled_windows_top_line() {
    let dir = TempDir::new().unwrap();
    let mut app = super::position_marks::over_store(dir.path());
    let from = scrolled_back_wide(&mut app);
    let was = top_of(&app, from, WIDE);
    let cursor = app.pane_chat(from).cursor();
    assert!(
        was.0 > 0 && !app.pane_viewport(from).is_following_tail(),
        "premise: {was:?}"
    );
    app.dispatch_command_for_test("vsplit");
    let split = app.workspace.focused();
    assert_ne!(split, from, "premise: a second window");
    frame_of(&mut app, WIDE.0, WIDE.1);
    let (w, _) = app.pane_content_size(from, Rect::new(0, 0, WIDE.0, WIDE.1));
    assert!(w < WIDE.0 / 2 + 1, "premise: narrower ({w})");
    assert!(at_top(&app, from, WIDE, was), "the window split");
    assert!(at_top(&app, split, WIDE, was), "and its copy");
    // The copy is focused, so its frame re-syncs and drags its cursor:
    // kept, or pushed into the top's band, never back (review stop 15).
    let now = app.pane_chat(split).cursor();
    assert!(now >= cursor, "the copy's cursor {cursor:?} → {now:?}");
}

/// **A terminal made narrower keeps a scrolled window's top line.**
#[tokio::test]
async fn a_narrower_screen_keeps_a_scrolled_windows_top_line() {
    let dir = TempDir::new().unwrap();
    let mut app = super::position_marks::over_store(dir.path());
    let pane = scrolled_back_wide(&mut app);
    let was = top_of(&app, pane, WIDE);
    let narrow = (WIDE.0 / 2, WIDE.1);
    frame_of(&mut app, narrow.0, narrow.1);
    assert!(at_top(&app, pane, narrow, was), "{was:?}");
}

/// **…in a frame that re-syncs its cursor too** — a reply streaming in,
/// so the frame drags the cursor into the window: it read the old width's
/// rows before the top was kept, dragged the cursor into a window misread
/// at the new width, and the top followed the cursor there (review stop
/// 15, A H1 / B H1: line 172 → 147).
#[tokio::test]
async fn a_narrower_screen_while_a_reply_streams_keeps_the_top_line() {
    let dir = TempDir::new().unwrap();
    let mut app = super::position_marks::over_store(dir.path());
    let pane = scrolled_back_wide(&mut app);
    let was = top_of(&app, pane, WIDE);
    let cursor = app.pane_chat(pane).cursor();
    app.submit("ask more");
    app.feed_stream(StreamMsg::Chunk("arriving".into()));
    app.set_focus(PaneKind::Output);
    assert_eq!(app.workspace.focused(), pane, "premise: in that window");
    let narrow = (WIDE.0 / 2, WIDE.1);
    frame_of(&mut app, narrow.0, narrow.1);
    assert!(at_top(&app, pane, narrow, was), "{was:?}");
    // Kept, or pushed into the top's `scrolloff` band — never dragged
    // back to a window read at the old width (it went (53, 0) → (52, 172)).
    let now = app.pane_chat(pane).cursor();
    assert!(now >= cursor, "the cursor {cursor:?} → {now:?}");
}

/// **A frame with no room is not a size** — one frame of a terminal two
/// columns wide, then the wide one again, and the same top: at a width of
/// 0 every row is line 0's, and noting it put the window at the start for
/// good (review stop 15, A M6 / B L1).
#[tokio::test]
async fn a_frame_with_no_room_keeps_the_top_line() {
    let dir = TempDir::new().unwrap();
    let mut app = super::position_marks::over_store(dir.path());
    let pane = scrolled_back_wide(&mut app);
    let was = top_of(&app, pane, WIDE);
    frame_of(&mut app, 2, WIDE.1);
    frame_of(&mut app, WIDE.0, WIDE.1);
    assert!(at_top(&app, pane, WIDE, was), "{was:?}");
}

/// A `:tool` of yours running while the reply streams below it, its
/// transcript focused, every fold open (so the record's rows move the top
/// as its lines move the cursor), scrolled back into the reply with the
/// cursor three lines under the top — the pane.
fn reading_below_a_running_tool(app: &mut App) -> crate::wm::PaneId {
    manual_tool(app, "echo zz-typed");
    app.submit("tell me");
    let reply: String = (1..=80).map(|n| format!("reply line {n}\n")).collect();
    app.feed_stream(StreamMsg::Chunk(reply));
    app.set_focus(PaneKind::Output);
    app.sync_chat();
    let pane = app.workspace.focused();
    type_keys_sized(app, WIDE, "zR");
    frame_of(app, WIDE.0, WIDE.1);
    top_at_row(app, pane, WIDE, 20);
    let (top, _) = top_of(app, pane, WIDE);
    app.pane_chat_mut(pane).unwrap().place_cursor(top + 3, 0);
    frame_of(app, WIDE.0, WIDE.1);
    pane
}

/// The text of the line `pane`'s cursor is on.
fn cursor_text(app: &App, pane: crate::wm::PaneId) -> String {
    let (line, _) = app.pane_chat(pane).cursor();
    app.pane_chat(pane).line_text(line).unwrap_or_default()
}

/// **A record closing above a reader keeps the reader where it was**
/// (review stop 15, F H2) — a `:tool` of yours ran while the reply
/// streamed below it; the record closing it takes its running line's
/// place, lines longer, and a window scrolled back into the reply stays
/// on the line its cursor read and the line at its top, as vim keeps a
/// window past an edit above it (`mark_adjust`). By line, both stood six
/// lines up the reply.
#[tokio::test]
async fn a_record_closing_above_a_reader_keeps_its_place() {
    let mut app = test_app();
    let pane = reading_below_a_running_tool(&mut app);
    let read = |app: &App| cursor_text(app, pane);
    let (cursor, shown) = (read(&app), top_text(&app, pane, WIDE));
    assert!(
        cursor.starts_with("reply line") && shown.starts_with("reply line"),
        "premise: reading the reply: {cursor:?}, {shown:?}"
    );
    assert!(
        !app.pane_viewport(pane).is_following_tail(),
        "premise: scrolled back"
    );
    settle(&mut app).await;
    let lines = app.output().lines();
    assert!(
        !lines.iter().any(|l| l.starts_with("⚙ running")),
        "premise: its line replaced: {lines:#?}"
    );
    assert_eq!(read(&app), cursor, "the cursor on the line it read");
    assert_eq!(
        top_text(&app, pane, WIDE),
        shown,
        "the top on the line it showed"
    );
}

/// **…and a selection keeps both its ends** (the close verifier at review
/// stop 15, MEDIUM 2, probed) — its anchor a flat line in the window's
/// copy, as the cursor is: it slid up the lines the record put above it
/// while the cursor stayed, and `y` then took them.
#[tokio::test]
async fn a_record_closing_above_a_selection_keeps_it_whole() {
    let mut app = test_app();
    let pane = reading_below_a_running_tool(&mut app);
    type_keys_in(&mut app, Rect::new(0, 0, WIDE.0, WIDE.1), "Vjj");
    let anchor_text = |app: &App| {
        let (line, _) = app.pane_chat(pane).anchor().expect("a selection");
        app.pane_chat(pane).line_text(line).unwrap_or_default()
    };
    let (anchor, cursor) = (anchor_text(&app), cursor_text(&app, pane));
    assert!(anchor.starts_with("reply line"), "premise: {anchor:?}");
    assert_ne!(anchor, cursor, "premise: three lines");
    settle(&mut app).await;
    assert_eq!(anchor_text(&app), anchor, "its anchor on the line it was");
    assert_eq!(cursor_text(&app, pane), cursor, "its cursor too");
}

/// **…and a window that set the session aside comes back to where it
/// was** (the close verifier, MEDIUM 3: only the shown window was held) —
/// its place kept in its history, found again by its blocks.
#[tokio::test]
async fn a_record_closing_in_a_session_set_aside_keeps_its_place() {
    let mut app = test_app();
    let pane = reading_below_a_running_tool(&mut app);
    let (cursor, shown) = (cursor_text(&app, pane), top_text(&app, pane, WIDE));
    let reading = app.pane_session(pane).conversation.episode().id;
    app.dispatch_command_for_test("new");
    assert_ne!(
        app.pane_session(pane).conversation.episode().id,
        reading,
        "premise: set aside"
    );
    settle(&mut app).await;
    app.dispatch_command_for_test("b#");
    assert_eq!(
        app.pane_session(pane).conversation.episode().id,
        reading,
        "premise: back"
    );
    assert_eq!(
        cursor_text(&app, pane),
        cursor,
        "the cursor on the line it read"
    );
    assert_eq!(
        top_text(&app, pane, WIDE),
        shown,
        "the top on the line it showed"
    );
}

/// The text of the line at `pane`'s top, on a `frame`-sized screen.
fn top_text(app: &App, pane: crate::wm::PaneId, frame: (u16, u16)) -> String {
    let (line, _) = top_of(app, pane, frame);
    app.pane_session(pane).output.lines()[line].clone()
}

/// **A window in a tab not shown keeps its place through a landing** — a
/// compaction run in another tab rebuilds the session under it: measured
/// in no room at all (the frame lays out only the tab shown), its top was
/// found at width 0, line 0's, and it came back at its end (review stop
/// 15, A M6, measured on "reply 18").
#[tokio::test]
async fn a_window_in_a_tab_not_shown_keeps_its_place_through_a_compaction() {
    let dir = TempDir::new().unwrap();
    let mut app = super::position_marks::over_store(dir.path());
    let pane = scrolled_back_wide(&mut app);
    let output = &app.pane_session(pane).output;
    let line = (output.lines().iter())
        .position(|l| l.starts_with("reply 18:"))
        .expect("premise: reply 18");
    let view = app.window_of_mut(pane).unwrap().session_view_mut().unwrap();
    view.viewport.freeze_at(line, 0);
    app.pane_chat_mut(pane).unwrap().place_cursor(line, 0);
    frame_of(&mut app, WIDE.0, WIDE.1);
    let was = top_text(&app, pane, WIDE);
    assert!(was.starts_with("reply 18:"), "premise: {was:?}");
    app.dispatch_command_for_test("tab sp");
    assert_ne!(app.workspace.focused(), pane, "premise: another tab");
    frame_of(&mut app, WIDE.0, WIDE.1);
    app.dispatch_command_for_test("compact --keep 15");
    for msg in [
        StreamMsg::Chunk("what was done".into()),
        StreamMsg::Complete,
    ] {
        app.feed_summary(msg);
    }
    app.dispatch_command_for_test("tabprevious");
    frame_of(&mut app, WIDE.0, WIDE.1);
    assert!(
        !app.pane_viewport(pane).is_following_tail(),
        "not sent to its end"
    );
    assert_eq!(top_text(&app, pane, WIDE), was);
}

/// **A window following its end goes on following at another width** —
/// it has no row to keep, and keeping one unpinned it: a reply streaming
/// after a `:vsplit` or a resize ran off its bottom (review stop 15, A H2 /
/// B H4 / C H2, the guard held by nothing).
#[tokio::test]
async fn a_window_following_its_end_follows_on_at_another_width() {
    let dir = TempDir::new().unwrap();
    let mut app = super::position_marks::over_store(dir.path());
    let pane = scrolled_back_wide(&mut app);
    let view = app.window_of_mut(pane).unwrap().session_view_mut().unwrap();
    view.viewport.snap_to_tail();
    frame_of(&mut app, WIDE.0, WIDE.1);
    let narrow = (WIDE.0 / 2, WIDE.1);
    frame_of(&mut app, narrow.0, narrow.1);
    assert!(app.pane_viewport(pane).is_following_tail(), "following");
    app.submit("ask more");
    app.feed_stream(StreamMsg::Chunk("the newest words".into()));
    let text = text_of(&mut app, narrow.0, narrow.1);
    assert!(text.contains("the newest words"), "drawn:\n{text}");
}

/// **…unless that would leave its cursor below it**: the lines between
/// grow as they wrap, and a window keeps its cursor on screen, as vim's
/// does when it is resized — moved that far and no further.
#[tokio::test]
async fn a_narrower_screen_keeps_the_cursor_on_screen() {
    let dir = TempDir::new().unwrap();
    let mut app = super::position_marks::over_store(dir.path());
    let pane = scrolled_back_wide(&mut app);
    let (w, h) = app.pane_content_size(pane, Rect::new(0, 0, WIDE.0, WIDE.1));
    let bottom = top_row_of(&app, pane, WIDE) + usize::from(h) - 1;
    let output = &app.pane_session(pane).output;
    let (line, _) = output.visual_to_logical(&app.pane_shut(pane), bottom, 0, w);
    app.pane_chat_mut(pane).unwrap().place_cursor(line, 0);
    frame_of(&mut app, WIDE.0, WIDE.1);
    let narrow = (WIDE.0 / 2, WIDE.1);
    frame_of(&mut app, narrow.0, narrow.1);
    let (w, h) = app.pane_content_size(pane, Rect::new(0, 0, narrow.0, narrow.1));
    let output = &app.pane_session(pane).output;
    let row = output.logical_to_visual(&app.pane_shut(pane), line, 0, w).0;
    let top = top_row_of(&app, pane, narrow);
    assert_eq!(row, top + usize::from(h) - 1, "on its last row");
}

/// **…and so does a place put back in a window grown narrower meanwhile**
/// — set aside at one width (`:new`), the screen narrowed, `<C-^>` back:
/// the window's first draw on the session is a new width
/// (`note_transcript_sizes`). The place carried the size it was laid out
/// at for this until S21-Q10; a top kept as text needs no size, and a
/// first draw is the change it stood for.
#[tokio::test]
async fn a_place_put_back_narrower_keeps_its_cursor_on_screen() {
    let dir = TempDir::new().unwrap();
    let mut app = super::position_marks::over_store(dir.path());
    let pane = scrolled_back_wide(&mut app);
    let source = app.pane_session(pane).id;
    let (w, h) = app.pane_content_size(pane, Rect::new(0, 0, WIDE.0, WIDE.1));
    let bottom = top_row_of(&app, pane, WIDE) + usize::from(h) - 1;
    let output = &app.pane_session(pane).output;
    let (line, _) = output.visual_to_logical(&app.pane_shut(pane), bottom, 0, w);
    app.pane_chat_mut(pane).unwrap().place_cursor(line, 0);
    frame_of(&mut app, WIDE.0, WIDE.1);

    app.dispatch_command_for_test("new");
    assert_ne!(app.pane_session(pane).id, source, "premise: set aside");
    let narrow = (WIDE.0 / 2, WIDE.1);
    frame_of(&mut app, narrow.0, narrow.1);
    press_ctrl_hat(&mut app, false);
    assert_eq!(app.pane_session(pane).id, source, "premise: back");
    assert_eq!(app.pane_chat(pane).cursor().0, line, "premise: its cursor");
    frame_of(&mut app, narrow.0, narrow.1);

    let (w, h) = app.pane_content_size(pane, Rect::new(0, 0, narrow.0, narrow.1));
    let output = &app.pane_session(pane).output;
    let row = output.logical_to_visual(&app.pane_shut(pane), line, 0, w).0;
    let top = top_row_of(&app, pane, narrow);
    assert!(!app.pane_viewport(pane).is_following_tail(), "still back");
    assert_eq!(row, top + usize::from(h) - 1, "on its last row");
}

/// **A carried place keeps its top's text at any width** — a `:clone` of a
/// scrolled window, then a first frame narrower than the one the clone was
/// placed for, and the same text at its top. (The place once had to say
/// the size its rows were found at, so the narrower frame could read them
/// at it; a top is text now, S21-Q10.)
#[tokio::test]
async fn a_clone_drawn_narrower_keeps_its_top_line() {
    let dir = TempDir::new().unwrap();
    let mut app = super::position_marks::over_store(dir.path());
    let pane = scrolled_back_wide(&mut app);
    let was = top_of(&app, pane, WIDE);
    let original = app.pane_session(pane).conversation.episode().id;
    app.clone_episode(original);
    assert_ne!(
        app.pane_session(pane).conversation.episode().id,
        original,
        "premise: the clone"
    );
    let narrow = (WIDE.0 / 2, WIDE.1);
    frame_of(&mut app, narrow.0, narrow.1);
    assert!(at_top(&app, pane, narrow, was), "{was:?}");
}

/// **`:vfork` keeps the top line in both windows, and `<C-^>` between the
/// fork and what it forked keeps it** (Steven, 11-01, images 8–9: "both
/// scroll and in different ways", and CTRL-^ "scrolls").
#[tokio::test]
async fn a_vfork_and_ctrl_hat_keep_the_top_line() {
    let dir = TempDir::new().unwrap();
    let mut app = super::position_marks::over_store(dir.path());
    let from = scrolled_back_wide(&mut app);
    let was = top_of(&app, from, WIDE);
    let source = app.pane_session(from).id;
    app.dispatch_command_for_test("vfork");
    let fork = app.workspace.focused();
    assert_ne!(app.pane_session(fork).id, source, "premise: the fork shown");
    frame_of(&mut app, WIDE.0, WIDE.1);
    assert!(at_top(&app, from, WIDE, was), "the original");
    assert!(at_top(&app, fork, WIDE, was), "the fork");
    press_ctrl_hat(&mut app, false);
    frame_of(&mut app, WIDE.0, WIDE.1);
    assert_eq!(
        app.pane_session(fork).id,
        source,
        "premise: back to the source"
    );
    assert!(at_top(&app, fork, WIDE, was), "the source, by CTRL-^");
    press_ctrl_hat(&mut app, false);
    frame_of(&mut app, WIDE.0, WIDE.1);
    assert_ne!(app.pane_session(fork).id, source, "premise: the fork again");
    assert!(at_top(&app, fork, WIDE, was), "and the fork again");
}

/// **`:vfork` starts where the window it splits from is** (S21-Q4: "Carry
/// over whole place by position"), as `:split` copies a window: the fork
/// at that cursor and scroll, and the session it forks — the new window's
/// alternate, which `<C-^>` brings back — there too. It held the source at
/// its entry, which a window never moved could not tell apart.
#[tokio::test]
async fn vfork_starts_where_the_window_it_splits_from_is() {
    let dir = TempDir::new().unwrap();
    // Turns as live ones are, so the transcript has lines to stand on.
    let mut app = super::position_marks::over_store(dir.path());
    for i in 0..6 {
        app.submit(&format!("ask {i}"));
        app.feed_stream(StreamMsg::Chunk(format!("reply {i}")));
        app.feed_stream(StreamMsg::Complete);
    }
    let source = app.prompt_target();
    app.set_focus(PaneKind::Output);
    let from = app.workspace.focused();
    let was = stand_back_in(&mut app, from);
    app.dispatch_command_for_test("vfork");
    let pane = app.workspace.focused();
    assert_ne!(app.pane_session(pane).id, source, "premise: the fork shown");
    let place = |app: &App| (app.pane_chat(pane).cursor(), app.pane_viewport(pane).top());
    assert_eq!(place(&app), was, "the fork, where you were");
    app.dispatch_command_for_test("b#");
    assert_eq!(
        app.pane_session(pane).id,
        source,
        "premise: back to the source"
    );
    assert_eq!(place(&app), was, "and the source there too");
}

/// A composition landing beside a session a viewer covers
/// (`Window::repoint_beneath`): `:q` on the viewer brings it up at its entry.
#[tokio::test]
async fn a_landing_beneath_a_view_comes_up_at_its_entry() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(base, t1, 1);
    app.conversation_mut().begin_turn("branch prompt");
    app.conversation_mut().push_chunk("branch reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(branch);
    open_graph_pane(&mut app);
    let pane = app.workspace.focused();
    app.set_graph_view_for_test(gv);
    let asked_from = app.prompt_target();
    let plan = app.squash_plan().expect("the branch resolves");
    let text = to_editable(&[Message::user("q"), Message::assistant("a")]);
    let id = app
        .commit_summary_into(
            asked_from,
            plan.graph,
            plan.target,
            &plan.sources,
            &text,
            None,
        )
        .expect("composes");
    assert_eq!(
        app.focus(),
        PaneKind::Graph,
        "premise: the tree still shown"
    );
    app.focus_pane(pane);
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.pane_session(pane).conversation.episode().id,
        id,
        "premise: the landing, beneath the tree"
    );
    assert!(app.pane_viewport(pane).is_following_tail(), "at its end");
}

/// A composition landing in the window that shows the session it lands
/// beside (`show_buffer_in`'s `Window::enter` half): `:clone` from the
/// transcript, the clone in that window at its entry (review of the 10-26
/// answers, M4: the one ask no test held).
#[test]
fn a_landing_in_the_window_beside_comes_up_at_its_entry() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 12);
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let from = app.pane_session(pane).id;
    app.dispatch_command_for_test("clone");
    assert_ne!(
        app.pane_session(pane).id,
        from,
        "premise: the clone in this window"
    );
    assert!(app.pane_viewport(pane).is_following_tail(), "at its end");
}

/// The launch's window, on the session a resume loaded (`App::new`): it
/// was born before any app could ask (review of the 10-26 answers, M3).
#[test]
fn a_resumed_launch_window_starts_at_its_entry() {
    let dir = TempDir::new().unwrap();
    {
        let mut app = test_app_with_store(
            test_backend(),
            Arc::new(Config::default()),
            Box::new(JsonlStore::new(dir.path()).unwrap()),
        );
        app.conversation_mut().begin_turn("hello");
        app.conversation_mut().push_chunk("Hi there.");
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
    }
    let app = test_app_with_store(
        test_backend(),
        config_resume_last(),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    assert_eq!(app.conversation().messages().len(), 2, "premise: resumed");
    let pane = (app.workspace.panes_of(PaneKind::Output).next()).expect("the launch window");
    assert!(app.pane_viewport(pane).is_following_tail(), "at its end");
}

/// **What `:open` and `:b` replace is kept** (Steven, 10-25, V4b Q1:
/// "Definitely vim's way") — a viewer as it was, and the session it lay
/// over with where the window was in it; coming back to that session finds
/// the window where it was. They were dropped: "the session is replaced,
/// not hidden". And `:q` on a transcript closes its window still, history
/// or none (Q3): only a viewer gives back.
#[test]
fn opening_a_session_over_a_viewer_keeps_both() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, here, _) = app_with_two_sessions(dir.path());
    app.focus_pane(here);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.pane_session(here).id;
    let was = stand_back_in(&mut app, here);
    app.dispatch_command_for_test("graph");
    app.dispatch_command_for_test("help");
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Page(PageKind::Help)),
        "premise"
    );

    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.pane_session(here).id;
    assert_eq!(app.workspace.kind_of(here), Some(PaneKind::Output));
    assert_eq!(app.pane_session(here).conversation.episode().id, b);
    assert_eq!(
        history_of(&app, here),
        vec![
            format!("session b{}", a_buffer.number()),
            "view graph".into(),
            "view help".into()
        ],
        "A, the tree over it, then the manual: in the order they were left"
    );

    app.open_by_hash(&a.to_hex()[..8]);
    let vp = app.pane_viewport(here);
    assert_eq!(
        (app.pane_chat(here).cursor(), vp.top()),
        was,
        "back in A where it was"
    );
    assert_eq!(
        history_of(&app, here),
        vec![
            "view graph".to_string(),
            "view help".into(),
            format!("session b{}", b_buffer.number())
        ],
        "a buffer in it once"
    );

    let panes = app.workspace.pane_count();
    assert!(!app.window_command_for_test(crate::keymap::WindowCmd::Quit));
    assert_eq!(
        app.workspace.pane_count(),
        panes - 1,
        ":q on a transcript closes it"
    );
    assert!(app.workspace.slot(here).is_none(), "its window gone");
}

/// **`<C-^>` toggles a window between its two sessions, each where it was**
/// (Steven, 10-25, V4b Q1–Q2: vim's CTRL-^), and `:b#` is the same verb —
/// typed in the prompt bar, it acts on the aim's window, as `:b N` there
/// does.
#[test]
fn ctrl_hat_toggles_a_window_between_its_two_sessions() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, here, other) = app_with_two_sessions(dir.path());
    let episode = |app: &App| app.pane_session(here).conversation.episode().id;
    // The other pane on A, so the one showing B — the aim's window, from
    // the prompt — is this one.
    app.focus_pane(other);
    app.open_by_hash(&a.to_hex()[..8]);
    app.focus_pane(here);
    app.open_by_hash(&a.to_hex()[..8]);
    let was = stand_back_in(&mut app, here);
    app.open_by_hash(&b.to_hex()[..8]);
    assert_eq!(episode(&app), b, "premise");

    press_ctrl_hat(&mut app, false);
    assert_eq!(episode(&app), a, "<C-^>: back to A");
    let vp = app.pane_viewport(here);
    assert_eq!(
        (app.pane_chat(here).cursor(), vp.top()),
        was,
        "where it was"
    );
    press_ctrl_hat(&mut app, true);
    assert_eq!(episode(&app), b, "and to B again, however the key is sent");

    app.set_focus(PaneKind::Prompt);
    assert_eq!(
        app.prompt_target(),
        app.pane_session(here).id,
        "premise: aimed at B"
    );
    assert_eq!(app.aim_window(), Some(here), "premise: B's one window");
    app.dispatch_command_for_test("b#");
    assert_eq!(episode(&app), a, ":b# from the prompt: the aim's window");
}

/// **`<C-^>` brings the tree back as it was** — its cursor, its marks — in
/// the window `<CR>` left it in (V4b Q1, netrw's `<CR>` then CTRL-^), and
/// toggles back to the session from inside the tree.
#[test]
fn ctrl_hat_brings_the_tree_back_as_it_was() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, here, _) = app_with_two_sessions(dir.path());
    app.focus_pane(here);
    app.open_by_hash(&a.to_hex()[..8]);
    app.dispatch_command_for_test("graph");
    let view = app.graph_view_mut().expect("premise: the tree");
    view.reveal_and_select_episode(a);
    view.toggle_mark();
    view.reveal_and_select_episode(b);
    let marked = app.graph_view().map(GraphView::marked_nodes);
    assert_eq!(marked, Some(vec![Node::Episode(a)]), "premise: A marked");
    // The mark's event ends as the loop ends it: the tree takes the mark.
    app.settle();
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Output),
        "premise: B open"
    );
    assert!(
        app.companions_of(here).next().is_none(),
        "premise: its inspector gone with it"
    );

    press_ctrl_hat(&mut app, false);
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Graph),
        "the tree back"
    );
    let tree = app.pane_graph(here).expect("its content too");
    assert_eq!(
        tree.selected_node(),
        Some(Node::Episode(b)),
        "on the row <CR> was on"
    );
    assert_eq!(tree.marked_nodes(), vec![Node::Episode(a)], "its marks");
    // (Rebuilt, as a tree given back by `:q` is — `bring_back`, and
    // `a_tree_given_back_holds_what_landed_meanwhile`.)
    assert!(
        app.companions_of(here).next().is_some(),
        "its inspector beside it"
    );

    press_ctrl_hat(&mut app, false);
    assert_eq!(app.workspace.kind_of(here), Some(PaneKind::Output));
    assert_eq!(
        app.pane_session(here).conversation.episode().id,
        b,
        "B again"
    );
}

/// **`<C-^>` and `:b#` with nothing to go back to say so** — vim's `E23`,
/// in our words (Steven, 10-25, V4b Q4); vim's `:b#` there says nothing, a
/// dead key (my call, agreed).
#[test]
fn ctrl_hat_with_nothing_to_go_back_to_says_so() {
    let mut app = test_app();
    let transcript = app
        .session_panes(Over::ThisTab)
        .next()
        .expect("a transcript");
    app.focus_pane(transcript);
    app.notices.clear();
    press_ctrl_hat(&mut app, false);
    assert_eq!(app.notice().as_deref(), Some(NO_ALTERNATE), "<C-^>");
    app.notices.clear();
    app.dispatch_command_for_test("b#");
    assert_eq!(app.notice().as_deref(), Some(NO_ALTERNATE), ":b#");
    assert_eq!(
        app.workspace.kind_of(transcript),
        Some(PaneKind::Output),
        "unmoved"
    );
}

/// **A session deleted while a window holds it leaves no place behind** —
/// its buffer, holding the fresh session that took the deleted one's place
/// (`reset_buffer`, as the panes showing it do), is what going back finds,
/// at its end: the place kept indexed rows the delete took (`hold` →
/// `forget_cursors_on`). Buffers outlive their sessions, so the history
/// keeps the buffer (V4b.5; the plan said the session would leave every
/// history — it is its place that leaves).
#[test]
fn a_deleted_session_leaves_no_place_behind() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, here, _) = app_with_two_sessions(dir.path());
    app.focus_pane(here);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.pane_session(here).id;
    stand_back_in(&mut app, here);
    app.open_by_hash(&b.to_hex()[..8]);
    assert_eq!(
        history_of(&app, here),
        vec![format!("session b{}", a_buffer.number())],
        "premise: A held, where the window was in it"
    );

    app.delete_episodes(&[a]);
    let fresh = app
        .session_of(a_buffer)
        .map(|buf| buf.conversation.episode().id);
    assert!(
        fresh.is_some_and(|f| f != a),
        "premise: a fresh session in its buffer"
    );

    press_ctrl_hat(&mut app, false);
    assert_eq!(app.pane_session(here).id, a_buffer, "back to its buffer");
    assert_eq!(
        app.pane_session(here).conversation.episode().id,
        fresh.unwrap()
    );
    assert!(
        app.pane_viewport(here).is_following_tail(),
        "at its end, not where the deleted one was read"
    );
    assert_eq!(
        app.pane_chat(here).cursor(),
        (0, 0),
        "no cursor carried over"
    );
}

/// **`<C-^>` from a tree to the view set aside under it takes the tree's
/// inspector with it** (V4b verify, M2 3): a companion describes its host,
/// and left beside the manual it inspected a tree no longer shown. And
/// `:b#` from the prompt bar back to the tree shows it in the aim's window
/// and focuses it there, as a view's opening does (M2, LOW).
#[test]
fn ctrl_hat_to_a_view_takes_the_inspector_and_b_hash_focuses_its_pane() {
    let mut app = test_app();
    let here = app
        .session_panes(Over::ThisTab)
        .next()
        .expect("a transcript");
    app.focus_pane(here);
    app.dispatch_command_for_test("help");
    app.dispatch_command_for_test("graph");
    assert!(
        app.companions_of(here).next().is_some(),
        "premise: the tree's inspector"
    );

    press_ctrl_hat(&mut app, false);
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Page(PageKind::Help)),
        "the manual back"
    );
    assert!(
        !app.workspace.has(PaneKind::Inspector),
        "the inspector gone with the tree"
    );

    app.set_focus(PaneKind::Prompt);
    assert_eq!(app.aim_window(), Some(here), "premise: the aim's window");
    app.dispatch_command_for_test("b#");
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Graph),
        "the tree back"
    );
    assert_eq!(
        app.workspace.focused(),
        here,
        "focused, as a view's opening is"
    );
}

/// **`{count}<C-^>` is `:b N`** — vim's (measured, Neovim 0.12.6: `1<C-^>`
/// edits buffer 1; one that does not exist says `E92`). The count rode
/// along and was dropped (V4b verify, LOW).
#[test]
fn a_count_before_ctrl_hat_names_the_buffer() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, here, _) = app_with_two_sessions(dir.path());
    app.focus_pane(here);
    app.open_by_hash(&a.to_hex()[..8]);
    let n = app.pane_session(here).id.number();
    app.open_by_hash(&b.to_hex()[..8]);
    // The manual over B: a bare `<C-^>` goes back to B, so the count is
    // what names A.
    app.dispatch_command_for_test("help");
    // The keys as typed: the count, then CTRL-^.
    for digit in n.to_string().chars() {
        let key = KeyEvent::new(KeyCode::Char(digit), KeyModifiers::NONE);
        let _ = app.focused_keymap_mut().translate(key);
    }
    match app.focused_keymap_mut().translate(ctrl_hat(false)) {
        KeyOutcome::Window(WindowCmd::Alternate, count) => {
            assert_eq!(count, Some(n as usize), "premise: the count rides along");
            let _ = app.apply_window(WindowCmd::Alternate, count, 120, 40);
        }
        other => panic!("not the alternate: {other:?}"),
    }
    assert_eq!(
        app.pane_session(here).conversation.episode().id,
        a,
        "buffer {n}"
    );
}

/// **From the prompt bar with the aim in no pane, there is no window to go
/// back in** — said as that, not "…in this window" (V4b verify, LOW).
#[test]
fn ctrl_hat_with_no_window_says_there_is_none() {
    let mut app = test_app();
    let transcript = app
        .session_panes(Over::ThisTab)
        .next()
        .expect("a transcript");
    app.focus_pane(transcript);
    app.close_pane_command(Asked::Plain);
    assert!(
        app.session_panes(Over::ThisTab).next().is_none(),
        "premise: no transcript left"
    );
    app.set_focus(PaneKind::Prompt);
    app.notices.clear();
    app.dispatch_command_for_test("b#");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("no window to go back in"), "{said}");
}
