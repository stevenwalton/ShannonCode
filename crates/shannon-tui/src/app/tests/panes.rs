//! Properties every pane shares.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **Every `:` command reaches the handler its table row names.**
///
/// The whole command table → handler wiring had no coverage: rewiring
/// `Command::Messages` to `open_help_command` left all 999 tests green. Each
/// handler is well tested through its own helper, and the dispatch that chooses
/// between them was reachable only through `execute_command`, which takes a
/// real stdout (Reviewer D, mutation-proven). `dispatch_command` is the
/// terminal-free half, and this drives it.
///
/// The pane-opening family is the testable core of it: each of these has a
/// visible, distinguishable outcome, so a row pointing at the wrong handler
/// fails here rather than in front of a user.
#[tokio::test]
async fn every_view_command_opens_the_pane_its_table_row_names() {
    // Spelled as a user types them, including a short form, so the resolver is
    // in the loop rather than bypassed.
    let cases: &[(&str, PaneKind)] = &[
        ("messages", PaneKind::Page(PageKind::Messages)),
        ("mes", PaneKind::Page(PageKind::Messages)),
        ("help", PaneKind::Page(PageKind::Help)),
        ("h", PaneKind::Page(PageKind::Help)),
        ("graph", PaneKind::Graph),
        ("g", PaneKind::Graph),
        ("analyze ls -la", PaneKind::Page(PageKind::Analyze)),
    ];
    for (typed, want) in cases {
        let mut app = test_app();
        app.set_focus(PaneKind::Output);
        assert!(
            !app.workspace
                .rects(Rect::new(0, 0, 80, 24))
                .iter()
                .any(|p| p.kind == *want),
            "premise — `{typed}` must OPEN {want:?}, so it cannot already be there"
        );

        app.dispatch_command_for_test(typed);

        assert!(
            app.workspace
                .rects(Rect::new(0, 0, 80, 24))
                .iter()
                .any(|p| p.kind == *want),
            "`:{typed}` must open {want:?}: {:?}",
            app.workspace
                .rects(Rect::new(0, 0, 80, 24))
                .iter()
                .map(|p| p.kind)
                .collect::<Vec<_>>()
        );
    }
}

/// **The terminal's cursor is where the keys go, and nowhere else** — one
/// door (`view::place`) for every kind of pane, from the type: inside the
/// focused pane's content, for every kind but `:graph`, a list whose cursor
/// is its reversed row until a Visual range begins; and while the `:` line
/// is open, on the status row past what you typed, the one cursor on screen
/// — a page drew a second of its own there (Reviewer D, F5). Three draws
/// kept a focus gate each, and deleting any of them passed every test (F2):
/// the gate is the door's now, and this is its test.
#[tokio::test]
async fn the_terminals_cursor_is_where_the_keys_go() {
    let (w, h) = (100u16, 40u16);
    for kind in PaneKind::all() {
        let mut app = test_app();
        app.executor = Arc::new(shannon_core::RecordingExecutor::default());
        open_pane_of(&mut app, kind);
        app.clamp_text_panes(w, h);
        assert_eq!(app.focus(), kind, "premise: {kind:?} has the keys");
        let rect = (app.workspace.frame_layout(Rect::new(0, 0, w, h)))
            .pane(app.workspace.focused())
            .expect("the focused pane is on screen")
            .rect
            .inner(ratatui::layout::Margin::new(1, 1));
        let (_, at) = cursor_of(&mut app, w, h);
        match kind {
            PaneKind::Graph => assert_eq!(at, None, "a list's cursor is its row"),
            _ => assert!(
                at.is_some_and(|at| rect.contains(at)),
                "{kind:?}: the cursor at {at:?}, not in {rect:?}"
            ),
        }
        type_keys(&mut app, ":ab");
        let (_, at) = cursor_of(&mut app, w, h);
        assert_eq!(
            at,
            Some(Position::new(3, h - 1)),
            "{kind:?}: the `:` line's"
        );
        type_keys(&mut app, "<Esc>");
        if kind == PaneKind::Graph {
            type_keys(&mut app, "V");
            let (_, at) = cursor_of(&mut app, w, h);
            assert_eq!(at, Some(Position::new(rect.x, rect.y)), "a Visual range's");
        }
    }
}

/// **Exactly one pane may render focused.** Focus is one value, so the
/// screen must show one bright border — a user who sees two has no way to
/// know where their next key lands, and the terminal cursor (placed for the
/// focused pane) ends up somewhere it isn't.
///
/// This was live: the transcript's focus was `!prompt_focused`, so with a
/// decision pending BOTH the transcript and the decision drew the focused
/// border and the caret sat in the transcript while keys answered the gate.
///
/// Asserted across every focus value, so the invariant is about the count,
/// not about the one arrangement that was broken.
#[tokio::test]
async fn exactly_one_pane_renders_focused() {
    use ratatui::style::Color;
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app();
    // A pending decision is the only way all three panes coexist.
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");

    let (w, h) = (80u16, 40u16);
    let frame = Rect::new(0, 0, w, h);
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();

    // Every pane in turn, found by walking the registry — so this test does
    // not itself hardcode how many there are. Adding a pane extends the
    // sweep automatically, which is the point.
    let panes: Vec<crate::wm::PaneId> = app.workspace.slots().iter().map(|s| s.id).collect();
    assert!(panes.len() >= 3, "need the decision pane open: {panes:?}");
    for id in panes {
        app.focus_pane(id);
        app.sync_chat();
        draw_frame(&mut app, &mut terminal);
        let buf = terminal.backend().buffer();
        let layout = app.workspace.frame_layout(frame);

        let bright: Vec<PaneKind> = layout
            .panes
            .iter()
            .filter(|p| corner_color(buf, p.rect) == Color::Cyan)
            .map(|p| p.kind)
            .collect();

        assert_eq!(
            bright.len(),
            1,
            "focus {:?}: exactly one focused border, got {bright:?}",
            app.focus()
        );
        assert_eq!(
            bright[0],
            app.focus(),
            "and it must be the pane that is actually focused"
        );
    }
}

/// **Focus is the one title in reverse video** (S21-Q11, Steven 11-01:
/// "Let's give it a look"; review stop 15, F H1) — on every focus, and with
/// two windows on one session, where the aim's accent is the brighter
/// border: a hue alone told the window your keys reach, and in Steven's
/// palette the two drew alike.
#[tokio::test]
async fn the_focused_pane_alone_wears_its_title_reversed() {
    use ratatui::style::Modifier;
    let lit = |app: &mut App| -> Vec<crate::wm::PaneId> {
        let (w, h) = (100u16, 30u16);
        let buf = frame_of(app, w, h);
        let layout = app.workspace.frame_layout(Rect::new(0, 0, w, h));
        (layout.panes.iter())
            .filter(|p| {
                let top = p.rect.x + 1..p.rect.x + p.rect.width.saturating_sub(1);
                top.into_iter()
                    .any(|x| buf[(x, p.rect.y)].modifier.contains(Modifier::REVERSED))
            })
            .map(|p| p.id)
            .collect()
    };
    let mut app = test_app();
    // A pending decision is the only way all three panes coexist.
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    let panes: Vec<crate::wm::PaneId> = app.workspace.slots().iter().map(|s| s.id).collect();
    assert!(panes.len() >= 3, "premise: three panes: {panes:?}");
    for id in panes {
        app.focus_pane(id);
        app.sync_chat();
        assert_eq!(lit(&mut app), vec![id], "focus {:?}", app.focus());
    }
    // Two windows on one session, the other aimed at: the one you are in.
    let mut app = test_app();
    app.submit("look");
    app.feed_stream(StreamMsg::Chunk("A reply.".into()));
    app.feed_stream(StreamMsg::Complete);
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("vsplit");
    let focused = app.workspace.focused();
    assert_eq!(lit(&mut app), vec![focused], "the split, not its twin");
}

/// **Closing a viewer's own pane leaves no dead pane** — it goes with its
/// content, and focus lands where keys go (renamed, review stop 10, C 7: it
/// was "a viewer pane that survives close keeps its content", and asserts
/// the pane goes).
///
/// `hide` refuses to remove the LAST pane, but `close_viewer` retired the
/// content unconditionally — leaving a focused Graph pane with no view,
/// which drew nothing and whose handler returned before its own `q` arm.
/// Every key dead but Ctrl-C. (Reviewer E.)
#[test]
fn closing_a_viewers_own_pane_leaves_no_dead_pane() {
    let mut app = test_app();
    // **A pane the graph OWNS, not one it borrowed.** This used to open the
    // graph over the transcript, which sets `restore = Some(Output)` — so
    // `close_viewer` always took the restore path and `gone` was always true.
    // The branch under test was never entered: making the retire unconditional
    // left the test green. A split gives the graph a pane of its own
    // (`restore = None`), and `:only` then makes it the last one, which is the
    // state `close_focused` refuses to remove (Reviewer C, round 3).
    app.split_command("graph", SplitDir::Vertical);
    let gv = app.build_graph_view();
    app.set_graph_view_for_test(gv);
    // The graph's own pane, its inspector (`:only` keeps a companion with
    // its host, V5.3 (3e)) and the prompt bar; the inspector closed too, so
    // the graph's pane is the last.
    app.only_command(false);
    let here = app.workspace.focused();
    let inspector = app
        .companions_of(here)
        .next()
        .expect("premise: kept with it");
    app.workspace.close(inspector);
    app.reconcile_pane_state();
    assert_eq!(
        app.workspace.kind_of(app.workspace.focused()),
        Some(PaneKind::Graph),
        "premise: standing in the graph's own pane"
    );
    assert_eq!(
        app.workspace.pane_count(),
        2,
        "premise: the graph's pane, plus the prompt that cannot be closed away"
    );

    app.close_pane_command(Asked::Plain);
    // "Never a pane with nothing in it" holds by type since V5.3 (3d) — a
    // Graph pane IS its tree — so what is asked is where `:q` leaves you
    // (V5.3 verify, F6, measured): the graph's pane gone with its tree, and
    // the keys in the prompt, a pane that takes them.
    assert!(
        !app.workspace.has(PaneKind::Graph),
        "the graph's pane went, and its tree with it"
    );
    assert_eq!(
        app.workspace.kind_of(app.workspace.focused()),
        Some(PaneKind::Prompt),
        "focus where keys go"
    );
}

/// An unknown pane name is a **visible** refusal that lists the real ones —
/// the names are not guessable, and a silent no-op reads as a broken key.
#[test]
fn splitting_an_unknown_pane_name_says_so() {
    let mut app = test_app();
    let before = app.workspace.slots().len();
    app.split_command("nonesuch", SplitDir::Horizontal);
    assert_eq!(app.workspace.slots().len(), before, "nothing opened");
    let notice = app.notice().expect("a refusal the user can see");
    assert!(notice.contains("nonesuch"), "{notice}");
    assert!(
        notice.contains("graph"),
        "and lists what does work: {notice}"
    );
}

/// **No pane's title can reach the cell the pane number is drawn in.**
///
/// `elide_title` existed for exactly this and was wired to the two panes where
/// the overwrite had been reported. Six others went straight through, and two
/// of those carry the command being approved — so the decision pane rendered
/// `run this? — grep … 'a-very-long-needle-st2i`, with the pane number `2`
/// inside the string a human was authorising (Reviewer F, off the cell grid).
///
/// Driven over every kind that can hold a long title, through the real
/// renderer, because the defect was in the cells and every model-side value was
/// correct throughout.
#[tokio::test]
async fn a_pane_number_is_never_drawn_into_its_title() {
    let needle = "aaaaaaaaaabbbbbbbbbbccccccccccddddddddddeeeeeeeeeeffffffffffgggggggggg";
    let long = format!("grep -RIn '{needle}' /some/deeply/nested/path | sort -u | head -50");

    for open_it in [0u8, 1, 2] {
        let mut app = test_app();
        app.set_focus(PaneKind::Output);
        match open_it {
            0 => app.arrive_and_gate_for_test(None, "t1", &long),
            1 => app.open_analyze_command(&long),
            _ => app.open_help_command(None),
        }
        // A second pane, so the numbers are two digits wide somewhere.
        app.dispatch_command_for_test("sp");
        let screen = text_of(&mut app, 78, 30);

        // A title row is a border row: it starts with `┌`. The pane number sits
        // at its right-hand end, and nothing but border may touch it.
        for row in screen.lines().filter(|l| l.trim_start().starts_with('┌')) {
            let chars: Vec<char> = row.chars().collect();
            // Walk back from the closing corner over the number and its
            // padding; the cell before the title must be a border rule.
            let end = chars
                .iter()
                .rposition(|c| *c == '┐')
                .expect("a closed border");
            let tail: String = chars[end.saturating_sub(6)..end].iter().collect();
            assert!(
                tail.contains('─'),
                "a title ran into the pane-number cell — no border rule before \
                 the corner: {tail:?}\n{row}"
            );
        }
    }
}

/// **Every pane's row points at a page of the manual that opens** — the
/// `$help` the shipped formats name, held to `:help` itself, so a pointer
/// cannot name a section the manual does not have.
#[test]
fn every_pane_names_a_help_topic_that_opens() {
    for kind in PaneKind::all() {
        let topic = kind.help_topic();
        assert!(
            crate::help_view::anchor(topic).is_some(),
            "{kind:?}: `:help {topic}` opens nothing"
        );
    }
}
