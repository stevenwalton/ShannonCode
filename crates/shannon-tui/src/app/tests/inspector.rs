//! The inspector beside the graph.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **The inspector is a page you move through, not a paragraph that gets cut.**
///
/// It rendered a wrapped `Paragraph` with no scroll offset, so a selected
/// message whose body ran past the last row was silently gone; it opened
/// `Extent::Flexible` while a comment justified its dead scroll by claiming it
/// was "sized to its content"; and `focused_viewer` did not know the kind
/// existed, so `j`, `k`, `gg`, `G` and `q` were all dead with no notice. Three
/// statements of "this pane is not really a pane", in a pane with a border, a
/// focus ring and a number (Reviewer F).
///
/// Driven through the same seam as its three siblings — the point of the fix is
/// that it IS one of them now, so a test written specially for it would be
/// testing a different route from the one production takes.
#[tokio::test]
async fn the_inspector_pages_like_its_siblings() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("a question");
    // Long enough that the body cannot fit a short pane — the premise is
    // asserted below rather than assumed.
    app.conversation_mut()
        .push_chunk(&(1..=60).map(|i| format!("line {i}\n")).collect::<String>());
    app.conversation_mut().commit_turn();
    open_graph(&mut app);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    // `j` `l` selects the turn and expands it; `j` `j` lands on a message node,
    // whose body is the long one.
    for code in [
        KeyCode::Char('j'),
        KeyCode::Char('l'),
        KeyCode::Char('j'),
        KeyCode::Char('j'),
    ] {
        app.graph_key_for_test(KeyEvent::new(code, KeyModifiers::NONE));
    }
    draw_frame(&mut app, &mut terminal);

    let inspector = app
        .workspace
        .rects(Rect::new(0, 0, 80, 24))
        .into_iter()
        .find(|p| p.kind == PaneKind::Inspector)
        .expect("the graph brings its inspector");
    let rows = inspector.content_size().1 as usize;
    let len = app
        .text_pane_len_for_test(inspector.id)
        .expect("the inspector is a text page");
    assert!(
        len > rows,
        "premise — the body must overflow the pane, or nothing here is testable \
         ({len} lines in {rows} rows)"
    );

    app.workspace.focus_pane(inspector.id);
    assert_eq!(
        app.focused_viewer(),
        Some(PaneKind::Inspector),
        "the pane must own its keys, or they fall through to the chat keymap"
    );
    let press = |app: &mut App, code| {
        app.handle_text_pane_key_for_test(
            PaneKind::Inspector,
            KeyEvent::new(code, KeyModifiers::NONE),
            (80, 24),
        )
    };

    // `G` reaches the end that used to be unreachable.
    let top = app
        .text_pane_pos_for_test(inspector.id)
        .expect("a position");
    press(&mut app, KeyCode::Char('G'));
    let bottom = app
        .text_pane_pos_for_test(inspector.id)
        .expect("a position");
    assert!(
        bottom.1 > top.1,
        "`G` must scroll to content that does not fit: {top:?} -> {bottom:?}"
    );
    // …and `k` comes back, so it is navigation and not a one-way jump.
    press(&mut app, KeyCode::Char('k'));
    let up = app
        .text_pane_pos_for_test(inspector.id)
        .expect("a position");
    assert!(
        up.0 < bottom.0,
        "`k` must move the cursor up: {bottom:?} -> {up:?}"
    );

    // `:q` closes it, like every other window. `q` is an ordinary key here.
    press(&mut app, KeyCode::Char('q'));
    assert!(
        app.workspace
            .rects(Rect::new(0, 0, 80, 24))
            .iter()
            .any(|p| p.kind == PaneKind::Inspector),
        "a bare `q` must not close a window"
    );
    app.dispatch_command_for_test("q");
    assert!(
        !app.workspace
            .rects(Rect::new(0, 0, 80, 24))
            .iter()
            .any(|p| p.kind == PaneKind::Inspector),
        "`:q` must close the inspector, as its legend says"
    );
}

/// **A message in the inspector is painted as the transcript paints it**
/// (Steven, 10-14: "Same as the conversation, so they can have the same
/// primitives") — the reply's markdown styled, the question dim — asserted
/// on the drawn cells of the inspector's own rect, since the graph beside it
/// draws the same words in its own look.
#[tokio::test]
async fn the_inspector_paints_a_message_as_the_conversation_does() {
    use crate::style::{CellStyle, Ink};
    use crate::theme::Context;
    use ratatui::style::Modifier;
    let mut app = test_app();
    app.conversation_mut().begin_turn("a question");
    app.conversation_mut()
        .push_chunk("# Heading\nplain then `snippet` here");
    app.conversation_mut().commit_turn();
    open_graph(&mut app);
    let area = Rect::new(0, 0, 120, 30);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    // The cell of `word`'s first character inside the inspector, and of a
    // plain word beside it.
    type Term = ratatui::Terminal<ratatui::backend::TestBackend>;
    let cell = |app: &mut App, terminal: &mut Term, word: &str| {
        draw_frame(app, terminal);
        let rect = app
            .workspace
            .rects(area)
            .into_iter()
            .find(|p| p.kind == PaneKind::Inspector)
            .expect("the graph brings its inspector")
            .rect;
        let buf: &ratatui::buffer::Buffer = terminal.backend().buffer();
        for y in rect.y..rect.y + rect.height {
            let row: String = (rect.x..rect.x + rect.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            if let Some(at) = row.find(word) {
                let x = rect.x + row[..at].chars().count() as u16;
                return buf[(x, y)].style();
            }
        }
        panic!("`{word}` is not in the inspector");
    };
    // `j` `l` expands the turn; `j` is its question, `j` again its reply.
    for code in [KeyCode::Char('j'), KeyCode::Char('l'), KeyCode::Char('j')] {
        app.graph_key_for_test(KeyEvent::new(code, KeyModifiers::NONE));
    }
    let asked = cell(&mut app, &mut terminal, "a question");
    assert!(
        asked.add_modifier.contains(Modifier::DIM),
        "the question dim: {asked:?}"
    );

    // Each of the reply's roles drawn as the theme paints that role where
    // the transcript draws it (`Context::Code`), and unlike plain text — so
    // the comparison cannot pass by both being nothing.
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    let plain = cell(&mut app, &mut terminal, "plain");
    assert!(
        !plain.add_modifier.contains(Modifier::DIM),
        "the reply is not dim: {plain:?}"
    );
    for (word, ink) in [("Heading", Ink::Heading), ("`snippet", Ink::Raw)] {
        let drawn = cell(&mut app, &mut terminal, word);
        let painted = app.theme().style(Context::Code, CellStyle::ink(ink));
        let fg = painted.fg.unwrap_or(ratatui::style::Color::Reset);
        assert_eq!(drawn.fg, Some(fg), "{word} as its role paints: {drawn:?}");
        assert_ne!(drawn.fg, plain.fg, "{word} unlike plain text: {drawn:?}");
    }
}

/// **A selected message is painted once, not every frame.** Painting runs
/// syntect; each frame re-derived the inspector's page, so a long reply
/// selected in `:graph` took every pane's frame rate down with it — 119 ms a
/// frame in release (review of 34033e8, Reviewer B, measured). Counted, not
/// timed: once the screen is warm, frames style nothing.
#[tokio::test]
async fn the_inspector_paints_a_message_once_not_every_frame() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("a question");
    app.conversation_mut()
        .push_chunk("```rust\nfn main() { println!(\"hi\"); }\n```\nand prose");
    app.conversation_mut().commit_turn();
    open_graph(&mut app);
    for code in [
        KeyCode::Char('j'),
        KeyCode::Char('l'),
        KeyCode::Char('j'),
        KeyCode::Char('j'),
    ] {
        app.graph_key_for_test(KeyEvent::new(code, KeyModifiers::NONE));
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let pane = app
        .workspace
        .rects(Rect::new(0, 0, 120, 30))
        .into_iter()
        .find(|p| p.kind == PaneKind::Inspector)
        .expect("the graph brings its inspector")
        .id;
    let page: String = app
        .text_pane_mut_for_test(pane)
        .expect("a page")
        .lines()
        .iter()
        .map(crate::paint::PageLine::text)
        .collect();
    assert!(
        page.contains("println!"),
        "premise: the reply is on the page: {page}"
    );
    let warm = crate::markdown::STYLED.with(std::cell::Cell::get);
    for _ in 0..5 {
        draw_frame(&mut app, &mut terminal);
    }
    assert_eq!(
        crate::markdown::STYLED.with(std::cell::Cell::get),
        warm,
        "a warm frame styles nothing"
    );
}

/// **A turn's time is shown as `tui.time_format` says, and follows a
/// `:set`** — the locale's by default, the seconds for `unix` — while what is
/// saved stays the seconds (Steven, 10-14). The setting reaches the page
/// through `refresh_inspector`, which no unit of `when` can see.
#[tokio::test]
async fn the_inspector_shows_a_time_as_the_setting_says() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("a question");
    app.conversation_mut().push_chunk("an answer");
    app.conversation_mut().commit_turn();
    open_graph(&mut app);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    let time = |app: &mut App, terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>| {
        draw_frame(app, terminal);
        let pane = app
            .workspace
            .rects(Rect::new(0, 0, 120, 30))
            .into_iter()
            .find(|p| p.kind == PaneKind::Inspector)
            .expect("the graph brings its inspector")
            .id;
        let lines = app
            .text_pane_mut_for_test(pane)
            .expect("a page")
            .lines()
            .to_vec();
        lines
            .iter()
            .map(crate::paint::PageLine::text)
            .find(|l| l.trim_start().starts_with("time "))
            .unwrap_or_else(|| panic!("a time field: {lines:?}"))
    };
    let locale = time(&mut app, &mut terminal);
    let episode = app.conversation().episode().id;
    let at = app.current_graph().placed(&episode)[0].provenance.at;
    assert!(
        locale.ends_with(&shannon_core::when::shown(at, "locale")),
        "the locale's by default: {locale}"
    );
    app.dispatch_command_for_test("set tui.time_format=unix");
    let unix = time(&mut app, &mut terminal);
    assert!(unix.ends_with("s (unix)"), "follows the setting: {unix}");
}

/// **The inspector follows the graph's selection, and keeps your place while it
/// does not move.**
///
/// Its body is derived from the pane next door, so it is the one text page that
/// can change without this pane being touched. Two halves: a new selection is a
/// new page (content changes, position resets), and a redraw of the same
/// selection must not throw away where you had scrolled to — a pane that
/// jumped back to the top every frame would be unreadable.
#[tokio::test]
async fn the_inspector_tracks_the_selection_and_keeps_your_place() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("a question");
    app.conversation_mut()
        .push_chunk(&(1..=60).map(|i| format!("line {i}\n")).collect::<String>());
    app.conversation_mut().commit_turn();
    open_graph(&mut app);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let inspector = app
        .workspace
        .rects(Rect::new(0, 0, 80, 24))
        .into_iter()
        .find(|p| p.kind == PaneKind::Inspector)
        .expect("the graph brings its inspector")
        .id;

    // **The text, not the line count.** Two different nodes can render the same
    // number of lines — the root and its turn both do — so a length comparison
    // would pass on a pane that never refreshed at all
    // ([[feedback_assert_the_subject]]).
    let body = |app: &mut App| -> String {
        app.text_pane_mut_for_test(inspector)
            .expect("a text page")
            .lines()
            .iter()
            .map(crate::paint::PageLine::text)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let before = body(&mut app);
    assert!(
        before.starts_with("Turn"),
        "premise — `:graph` opens on the active turn: {before}"
    );
    // Move the selection in the GRAPH pane; the inspector is never touched.
    // `l` expands the turn, `j` steps onto one of its messages — a node whose
    // body is a different shape, not merely different digits.
    for code in [KeyCode::Char('l'), KeyCode::Char('j')] {
        app.graph_key_for_test(KeyEvent::new(code, KeyModifiers::NONE));
    }
    draw_frame(&mut app, &mut terminal);
    let after = body(&mut app);
    assert_ne!(
        before, after,
        "the inspector still shows the previous node — it is a cache with no way \
         to be invalidated"
    );

    // Scroll it, then redraw WITHOUT moving the selection.
    app.workspace.focus_pane(inspector);
    app.handle_text_pane_key_for_test(
        PaneKind::Inspector,
        KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
        (80, 24),
    );
    let placed = app.text_pane_pos_for_test(inspector).expect("a position");
    assert!(
        placed.0 > 0,
        "premise — `G` must have moved something: {placed:?}"
    );
    let node_at_placed = app.inspected_node_for_test(inspector);
    draw_frame(&mut app, &mut terminal);
    assert_eq!(
        app.text_pane_pos_for_test(inspector),
        Some(placed),
        "a redraw of the same node must not throw away where you were reading"
    );

    // …and the other half: a DIFFERENT node is a different document, so the
    // reader starts at the top rather than at whatever offset the last page
    // left them at. Without this assertion, deleting the node-identity check
    // from `refresh_inspector` passes — the content still updates through
    // `set_lines`, and only the reset is lost.
    app.workspace.focus_kind(PaneKind::Graph);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
    app.workspace.focus_pane(inspector);
    draw_frame(&mut app, &mut terminal);
    assert_ne!(
        app.inspected_node_for_test(inspector),
        node_at_placed,
        "premise — `k` must have selected a different node"
    );
    assert_eq!(
        app.text_pane_pos_for_test(inspector).map(|p| p.0),
        Some(0),
        "a new page starts at the top"
    );
}

/// **A message the store holds only an outline of still shows its words.**
///
/// The graph snapshot carries outlines; the inspector is the one view that
/// prints a body, so selecting a message loads that turn on demand. The
/// session here lives only on disk — a second launch starts fresh — so no live
/// buffer overlays the body, and without the load the page says the body is
/// not loaded.
#[tokio::test]
async fn the_inspector_loads_a_stored_messages_words_on_selection() {
    let dir = TempDir::new().unwrap();
    let old = app_with_n_turns(dir.path(), 1).conversation().episode().id;
    let (mut app, body_loads) = relaunched_counting(dir.path());
    let turn = app.current_graph().full_turns(&old)[0];
    assert_eq!(
        app.current_graph().messages(&turn),
        None,
        "premise: the snapshot holds no body for it"
    );
    let loads = || body_loads.load(std::sync::atomic::Ordering::SeqCst);

    let mut gv = app.build_graph_view();
    gv.reveal_and_select_episode(old);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    // `j` onto the turn, `l` opens it, `j` onto its first message.
    for code in [KeyCode::Char('j'), KeyCode::Char('l'), KeyCode::Char('j')] {
        app.graph_key_for_test(KeyEvent::new(code, KeyModifiers::NONE));
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let inspector = app
        .workspace
        .rects(Rect::new(0, 0, 80, 24))
        .into_iter()
        .find(|p| p.kind == PaneKind::Inspector)
        .expect("the graph brings its inspector")
        .id;
    let body: Vec<String> = app
        .text_pane_mut_for_test(inspector)
        .expect("a text page")
        .lines()
        .iter()
        .map(crate::paint::PageLine::text)
        .collect();
    assert!(
        body.first().is_some_and(|l| l.starts_with("Message")),
        "premise: a message is selected: {body:?}"
    );
    assert!(
        body.iter().any(|l| l == "Hello 0"),
        "the stored words, loaded for the selection: {body:?}"
    );

    // **Once, not once a frame.** The refresh runs every frame; on the store
    // that ships (JSONL) a body request re-reads the whole history, so a
    // check that let a held body ask again would be a full disk read per
    // keypress and per streamed chunk (Reviewer B).
    let after_first = loads();
    assert_eq!(after_first, 1, "premise: the selection loaded it");
    draw_frame(&mut app, &mut terminal);
    draw_frame(&mut app, &mut terminal);
    assert_eq!(loads(), after_first, "a held body asks the store nothing");
}

/// **A background job finishing must not scroll the inspector back to the top.**
///
/// The body carries a child session's activity, so a job ending rewrites the
/// page with no selection change and no keypress. Keying "should the reader
/// start over?" on the CONTENT answered that question wrong: you are reading a
/// long message, a child finishes somewhere else, and you are back at line 0
/// (Reviewer A). Identity and staleness are two questions.
///
/// The content change is forced the way production forces it — by re-rendering
/// after the underlying page really did change — rather than by calling
/// `set_lines` directly, so this drives the same door.
#[tokio::test]
async fn a_content_change_under_a_still_reader_keeps_their_place() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("a question");
    app.conversation_mut()
        .push_chunk(&(1..=60).map(|i| format!("line {i}\n")).collect::<String>());
    app.conversation_mut().commit_turn();
    open_graph(&mut app);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
    for code in [
        KeyCode::Char('j'),
        KeyCode::Char('l'),
        KeyCode::Char('j'),
        KeyCode::Char('j'),
    ] {
        app.graph_key_for_test(KeyEvent::new(code, KeyModifiers::NONE));
    }
    draw_frame(&mut app, &mut terminal);
    let inspector = app
        .workspace
        .rects(Rect::new(0, 0, 80, 24))
        .into_iter()
        .find(|p| p.kind == PaneKind::Inspector)
        .expect("the graph brings its inspector")
        .id;

    app.workspace.focus_pane(inspector);
    app.handle_text_pane_key_for_test(
        PaneKind::Inspector,
        KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
        (80, 24),
    );
    // **Settle at the real geometry before measuring.** The key seam clamps
    // against the size it is handed (a whole 80x24), while a frame clamps
    // against the pane's actual share of it — so a position captured between
    // the two differs from the next frame's by the re-clamp alone, and every
    // assertion below would compare two different questions.
    draw_frame(&mut app, &mut terminal);
    let placed = app.text_pane_pos_for_test(inspector).expect("a position");
    assert!(
        placed.0 > 0,
        "premise — the reader must be somewhere other than the top: {placed:?}"
    );

    // **The state an activity change produces**: what is stored no longer
    // matches what the node renders to, while the SELECTION is untouched. A
    // running child is the production route there; doctoring the stored page
    // reaches the identical state without needing a live job, and it is the
    // state `refresh_inspector` branches on. (Rebuilding the graph view was the
    // obvious route and is the wrong one — it moves the selection, so it
    // exercises the reset path instead. The premise assertion below is what
    // caught that.)
    let stale = {
        let text = app
            .text_pane_mut_for_test(inspector)
            .expect("the inspector is a text page");
        let mut doctored = text.lines().to_vec();
        doctored.push(ratatui::text::Line::from("‖ held").into());
        text.set_lines(doctored);
        text.lines().len()
    };
    let node_before = app.inspected_node_for_test(inspector);

    draw_frame(&mut app, &mut terminal);

    assert_eq!(
        app.inspected_node_for_test(inspector),
        node_before,
        "premise — the SELECTION must be unchanged, or this tests the reset path"
    );
    assert_ne!(
        app.text_pane_len_for_test(inspector),
        Some(stale),
        "premise — the refresh must actually have replaced the doctored page"
    );
    assert_eq!(
        app.text_pane_pos_for_test(inspector),
        Some(placed),
        "the page changed under a reader who did not move; their place must survive"
    );
}

#[test]
fn graph_inspector_shows_selected_node_internals() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    app.conversation_mut().begin_turn("a question");
    app.conversation_mut().push_chunk("an answer");
    let u = Usage {
        input_tokens: 12,
        output_tokens: 8,
        ..Default::default()
    };
    // One round, so the whole turn and its final request are the same call.
    app.conversation_mut()
        .record_usage(TurnCost::single_round(u));
    app.conversation_mut().commit_turn();
    open_graph(&mut app);

    // Select the turn (j) → the inspector pane shows its provenance.
    app.graph_key_for_test(k(KeyCode::Char('j')));
    // Tall enough that the inspector's lower rows (the capability stub) are
    // on screen. It needs more room than it used to: the inspector is a
    // PANE now, sharing the transcript's half of the frame with the tree,
    // rather than a 42%-wide column of a full-screen takeover.
    let screen = text_of(&mut app, 80, 54);

    assert!(
        screen.contains("Turn"),
        "inspector heading missing:\n{screen}"
    );
    assert!(
        screen.contains("model"),
        "provenance field missing:\n{screen}"
    );
    assert!(screen.contains("msgs"), "message roster missing:\n{screen}");
    // Turn position (1st of 1) and the real per-turn token usage.
    assert!(screen.contains("1/1"), "turn position missing:\n{screen}");
    assert!(
        screen.contains("↑12 ↓8"),
        "per-turn token usage missing:\n{screen}"
    );
    assert!(
        screen.contains("none in scope yet"),
        "capability placeholder missing:\n{screen}"
    );
}
