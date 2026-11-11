//! **Visual over rows** — `v`/`V` in `:graph` and on the text pages, through
//! the keys a user presses (`crate::rows`).
//!
//! Steven, 2026-10-04: "visual mode no longer works here. So it made it hard
//! to batch select." It never had: `v` in `:graph` was refused.

use super::*;
use crate::rows::RowSelect;
use ratatui::style::Modifier;

/// `n` sessions in `:graph`, all folded, newest first — the oldest with two
/// turns, each later one with one (`spare 1`, `spare 2`, …) — and the cursor
/// on the top row.
fn folded_sessions(dir: &std::path::Path, n: usize) -> (App, Vec<EpisodeId>) {
    let mut app = app_with_n_turns(dir, 2);
    let mut ids = vec![app.conversation().episode().id];
    for i in 1..n {
        ids.insert(0, add_persisted_session(&mut app, &format!("spare {i}")));
    }
    let mut gv = GraphView::new(app.current_graph());
    for &id in &ids {
        gv.reveal_and_select_episode(id);
        gv.collapse();
    }
    gv.select_row(0);
    assert_eq!(gv.visible_len(), n, "premise: {n} folded rows");
    assert_eq!(gv.selected_episode(), Some(ids[0]), "premise: newest first");
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    (app, ids)
}

/// [`folded_sessions`] with two: `(app, newer, older)`.
fn two_folded_sessions(dir: &std::path::Path) -> (App, EpisodeId, EpisodeId) {
    let (app, ids) = folded_sessions(dir, 2);
    (app, ids[0], ids[1])
}

fn graph_selection(app: &App) -> Option<std::ops::RangeInclusive<usize>> {
    app.graph_view().and_then(RowSelect::selection)
}

/// **`V`, `j`, `d`: the range is what is deleted** — the batch select Steven
/// reached for. And the verb ends Visual: no range, no V-LINE, afterwards.
#[test]
fn a_range_in_the_graph_is_what_a_verb_acts_on() {
    let dir = TempDir::new().unwrap();
    let (mut app, newer, older) = two_folded_sessions(dir.path());
    type_keys(&mut app, "Vj");
    assert_eq!(app.focused_mode(), crate::keymap::Mode::VisualLine);
    assert_eq!(graph_selection(&app), Some(0..=1));
    assert_eq!(
        app.graph_view().map(GraphView::acts_on),
        Some(vec![Node::Episode(newer), Node::Episode(older)])
    );
    type_keys(&mut app, "d");
    assert_eq!(
        app.focused_mode(),
        crate::keymap::Mode::Normal,
        "Visual ended"
    );
    assert_eq!(graph_selection(&app), None, "and the range with it");
    let names = [app.session_label(newer), app.session_label(older)];
    type_keys(&mut app, "y");
    let said = app.notice().unwrap_or_default();
    assert!(said.starts_with("deleted "), "{said}");
    for name in &names {
        assert!(
            said.contains(name.as_str()),
            "the range, by name — {name}: {said}"
        );
    }
}

/// A session with three turns and a branch forked at its second (not the
/// tip: continuing the tip extends the session itself) — the branch hanging
/// under that turn's row, which is folded, so the branch is out of sight in
/// a fresh `:graph`. `(app, session, branch)`, the graph open with the cursor
/// on the top row.
fn session_with_hidden_branch(dir: &std::path::Path) -> (App, EpisodeId, EpisodeId) {
    let mut app = app_with_n_turns(dir, 3);
    let session = app.conversation().episode().id;
    let t1 = app.conversation().episode().own_turns()[1];
    app.checkout_turn(session, t1, 1);
    app.conversation_mut().begin_turn("on the branch");
    app.conversation_mut().push_chunk("r");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let branch = app.conversation().episode().id;
    assert_ne!(branch, session, "premise: a fork, not the tip extended");
    let mut gv = GraphView::new(app.current_graph());
    assert!(
        !visible_nodes(&gv).contains(&Node::Episode(branch)),
        "premise: the branch is folded out of sight"
    );
    gv.select_row(0);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    (app, session, branch)
}

/// Every row `view` shows, top to bottom.
fn visible_nodes(view: &GraphView) -> Vec<Node> {
    let mut v = view.clone();
    (0..v.visible_len())
        .filter_map(|row| {
            v.select_row(row);
            v.selected_node()
        })
        .collect()
}

/// **Select everything and delete it: it goes** — and a fork folded out of
/// sight keeps its history instead of blocking it (Steven, 10-04: 67
/// deleted, ten refused for forks he could not see, four times over).
#[test]
fn deleting_everything_in_sight_keeps_a_hidden_fork_whole() {
    let dir = TempDir::new().unwrap();
    let (mut app, session, branch) = session_with_hidden_branch(dir.path());
    let reads = app.current_graph().full_turns(&branch);
    let name = app.session_label(session);
    type_keys(&mut app, "ggVGd");
    type_keys(&mut app, "y");
    let said = app.notice().unwrap_or_default();
    assert!(said.starts_with(&format!("deleted {name}")), "{said}");
    assert!(said.contains("keeps its own copy"), "{said}");
    let graph = app.current_graph();
    assert!(
        !graph.episode_ids().contains(&session),
        "what was selected went"
    );
    assert_eq!(
        graph.full_turns(&branch),
        reads,
        "the fork reads what it read"
    );
    assert!(
        visible_nodes(&GraphView::new(graph)).contains(&Node::Episode(branch)),
        "and is a row of its own now, in sight"
    );
}

/// **An unfolded turn's message rows ride with the range** — refusing them
/// refused the whole delete (Steven's screenshot, 10-04). A message row on
/// its own still says why not.
#[test]
fn a_range_over_an_unfolded_turn_deletes_and_a_lone_message_says_why() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let session = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(session);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    type_keys(&mut app, "jl"); // the first turn, unfolded: its messages show
    type_keys(&mut app, "jd"); // a message row alone
    assert!(
        app.notice()
            .is_some_and(|n| n.contains("not single messages")),
        "{:?}",
        app.notice()
    );
    assert!(app.pending_confirm.is_empty(), "nothing asked");
    type_keys(&mut app, "ggVGd");
    assert!(
        app.confirm_prompt().is_some(),
        "asked: the range is deletable"
    );
    type_keys(&mut app, "y");
    assert!(!app.current_graph().episode_ids().contains(&session));
}

/// **A verb spends the marks of what it acted on, and only those**: a range
/// deleted, the mark beside it kept — netrw spends marks only through what
/// used them.
#[test]
fn a_range_verb_leaves_the_other_marks() {
    let dir = TempDir::new().unwrap();
    let (mut app, ids) = folded_sessions(dir.path(), 3);
    type_keys(&mut app, "jj k");
    assert_eq!(marked(&app), Some(1), "premise: the oldest marked");
    type_keys(&mut app, "kVjd");
    let names = [app.session_label(ids[0]), app.session_label(ids[1])];
    type_keys(&mut app, "y");
    let said = app.notice().unwrap_or_default();
    assert!(said.starts_with("deleted "), "{said}");
    for name in &names {
        assert!(
            said.contains(name.as_str()),
            "the range, by name — {name}: {said}"
        );
    }
    assert_eq!(marked(&app), Some(1), "the mark outside the range stayed");
    let graph = app.current_graph();
    assert!(
        graph.episode_ids().contains(&ids[2]),
        "and so did its session"
    );
}

/// **`p` spends the tree's marks, in every window on it** — the tree is one
/// buffer, a split another window on it, its marks shared both ways
/// (Steven, 10-26: "we only have to handle the one 'file' but just in
/// different views"; `trees_are_netrws_listings`).
///
/// It was "`p` spends the marks of its own tree only — the same session
/// marked in a second tree stays marked there" (10-20), on two trees the
/// app no longer makes: every split of a tree is on the same one. It held
/// because its keys never ended their events, so `settle_tree_marks` never
/// ran between them (review stop 12, Tier 3).
#[test]
fn pick_spends_the_trees_marks_in_every_window() {
    let dir = TempDir::new().unwrap();
    let (mut app, ids) = folded_sessions(dir.path(), 2);
    let first = app.workspace.focused();
    app.split_command("graph", SplitDir::Vertical);
    let second = app.workspace.focused();
    assert_ne!(first, second, "premise: two windows");
    let tree = |app: &App, pane| app.pane_graph(pane).map(GraphView::tree);
    assert_eq!(
        tree(&app, first),
        tree(&app, second),
        "premise: on one tree"
    );
    let marks = |app: &App, pane| app.pane_graph(pane).unwrap().marked_nodes();

    app.graph_view_mut()
        .unwrap()
        .reveal_and_select_episode(ids[1]);
    type_keys(&mut app, " ");
    assert_eq!(
        marks(&app, first),
        vec![Node::Episode(ids[1])],
        "marked in both"
    );
    app.focus_pane(first);
    type_keys(&mut app, "p");
    assert!(marks(&app, first).is_empty(), "spent where it picked");
    assert!(marks(&app, second).is_empty(), "and in the other window");
}

/// **`p` and `S` take a range too** — they read what was picked out, not
/// the marks alone, or a range on screen is silently ignored.
#[tokio::test]
async fn pick_and_squash_take_a_range() {
    let dir = TempDir::new().unwrap();
    let (mut app, _, _) = two_folded_sessions(dir.path());
    type_keys(&mut app, "Vjp");
    assert_eq!(
        branch_msgs(&app),
        vec![
            "spare 1", "reply", "Hello 0", "reply 0", "Hello 1", "reply 1"
        ],
        "both sessions, in display order"
    );

    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(base);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    type_keys(&mut app, "jVjS");
    let label = app.distilling().unwrap_or_default().to_string();
    assert!(
        label.starts_with("2 turns of"),
        "the range's two turns: {label}"
    );
}

/// **A range outranks the marks**: it is the gesture just made.
#[test]
fn a_range_outranks_the_marks() {
    let dir = TempDir::new().unwrap();
    let (mut app, newer, older) = two_folded_sessions(dir.path());
    type_keys(&mut app, "j ");
    assert_eq!(marked(&app), Some(1), "premise: the older row marked");
    type_keys(&mut app, "kV");
    assert_eq!(
        app.graph_view().map(GraphView::acts_on),
        Some(vec![Node::Episode(newer)]),
        "the range, not the mark on {older:?}"
    );
}

/// **`<Space>` and `mf` in Visual mark the range** — so ranges build a set —
/// and end Visual. Both keys, since they are two doors to one action.
#[test]
fn marking_in_visual_marks_the_range() {
    for mark in [" ", "mf"] {
        let dir = TempDir::new().unwrap();
        let (mut app, _, _) = two_folded_sessions(dir.path());
        type_keys(&mut app, "Vj");
        type_keys(&mut app, mark);
        assert_eq!(marked(&app), Some(2), "{mark:?} marks both rows");
        assert_eq!(graph_selection(&app), None, "{mark:?} ends the range");
        assert_eq!(app.focused_mode(), crate::keymap::Mode::Normal, "{mark:?}");
    }
}

/// `Esc` leaves Visual; `o` swaps the ends, so the range grows the other way.
#[test]
fn esc_and_o_act_on_the_range() {
    let dir = TempDir::new().unwrap();
    let (mut app, _, _) = two_folded_sessions(dir.path());
    type_keys(&mut app, "jVk");
    assert_eq!(graph_selection(&app), Some(0..=1));
    assert_eq!(app.graph_view().map(GraphView::selected), Some(0));
    type_keys(&mut app, "o");
    assert_eq!(
        app.graph_view().map(GraphView::selected),
        Some(1),
        "o: cursor to the anchor"
    );
    assert_eq!(graph_selection(&app), Some(0..=1), "the same rows");
    app.graph_key_for_test(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(graph_selection(&app), None);
    assert_eq!(app.focused_mode(), crate::keymap::Mode::Normal);
}

/// **A rebuild keeps the range.** A background turn landing rebuilds every
/// graph view, and the keymap would still say V-LINE over nothing.
#[test]
fn a_rebuild_keeps_the_range() {
    let dir = TempDir::new().unwrap();
    let (mut app, _, _) = two_folded_sessions(dir.path());
    type_keys(&mut app, "Vj");
    app.refresh_graph_views();
    assert!(graph_selection(&app).is_some(), "the range survived");
    assert_eq!(app.focused_mode(), crate::keymap::Mode::VisualLine);
}

/// **Visual ends when focus leaves** — here a decision taking focus, the
/// asynchronous way it leaves. One keymap serves every read-only pane, so a
/// mode left standing would follow focus into a pane with no range.
#[tokio::test]
async fn focus_leaving_ends_visual_and_keeps_insert() {
    let dir = TempDir::new().unwrap();
    let (mut app, _, _) = two_folded_sessions(dir.path());
    type_keys(&mut app, "Vj");
    let graph = app.workspace.focused();
    // The prompt typing, under the graph's Visual: Insert must survive.
    app.prompt_keymap.translate(key('i'));
    assert_eq!(app.prompt_keymap.mode(), crate::keymap::Mode::Insert);
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    // A decision arriving moves nothing (review stop 5, E2): the range stays.
    assert_ne!(app.focus(), PaneKind::Tool, "it did not take focus");
    assert!(
        app.pane_graph(graph)
            .is_some_and(|v| v.visual_anchor().is_some()),
        "nor ended the range"
    );
    // The user going to it is focus leaving.
    to_decision(&mut app);
    assert_eq!(
        app.chat_keymap.mode(),
        crate::keymap::Mode::Normal,
        "Visual ended"
    );
    assert!(
        app.pane_graph(graph)
            .is_some_and(|v| v.visual_anchor().is_none()),
        "and the range in the pane left"
    );
    assert_eq!(
        app.prompt_keymap.mode(),
        crate::keymap::Mode::Insert,
        "only Visual ends: a sentence half typed stays in Insert"
    );
}

/// **And a page's range** — a page selects in the transcript's buffer now,
/// not as rows, so it is cleared through its own door.
#[tokio::test]
async fn focus_leaving_a_page_ends_its_range() {
    let (mut app, _) = help_page();
    let help = app.workspace.focused();
    type_keys(&mut app, "vw");
    assert!(
        app.text_pane_mut_for_test(help).unwrap().selecting(),
        "premise: a range"
    );
    model_call(&mut app, "tu_1", "mkdir /tmp/shannon-test-absent");
    assert!(
        app.text_pane_mut_for_test(help).unwrap().selecting(),
        "a decision arriving moves nothing (review stop 5, E2)"
    );
    to_decision(&mut app);
    assert!(
        !app.text_pane_mut_for_test(help).unwrap().selecting(),
        "the range in the page left"
    );
}

/// **`<CR>` from a range ends Visual where it started, and only there.** The
/// tree's window shows the session now (the tree kept, V4b Q1), so focus is
/// on no tree; the prompt's keymap may be typing.
#[test]
fn opening_from_a_range_leaves_a_typing_prompt_typing() {
    let dir = TempDir::new().unwrap();
    let (mut app, _, _) = two_folded_sessions(dir.path());
    // Opened from the prompt, so closing it hands focus back there.
    let gv = app.graph_view().cloned().expect("premise: a graph");
    app.close_pane_command(Asked::Plain);
    app.set_focus(PaneKind::Prompt);
    app.prompt_keymap.translate(key('i'));
    app.open_kind(PaneKind::Graph);
    app.set_graph_view_for_test(gv);
    type_keys(&mut app, "Vj");
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_ne!(
        app.focus(),
        PaneKind::Graph,
        "premise: the session in its window"
    );
    assert_eq!(app.chat_keymap.mode(), crate::keymap::Mode::Normal);
    assert_eq!(
        app.prompt_keymap.mode(),
        crate::keymap::Mode::Insert,
        "focus is {:?}",
        app.focus()
    );
}

/// **`Esc` leaves Visual before it cancels a squash** — a second `Esc` does
/// that. One press must not discard a summary on the way out of a range.
#[tokio::test]
async fn esc_leaves_visual_before_cancelling_a_squash() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let base = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(base);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    type_keys(&mut app, "j j ");
    type_keys(&mut app, "S");
    assert!(app.distilling().is_some(), "premise: distilling");
    type_keys(&mut app, "V");
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    app.graph_key_for_test(esc);
    assert_eq!(graph_selection(&app), None, "the first Esc left Visual");
    assert!(app.distilling().is_some(), "and cancelled nothing");
    app.graph_key_for_test(esc);
    assert!(app.distilling().is_none(), "the second cancels");
}

/// **`e` edits one row**, and says so over a range or marks — editing the
/// cursor row while two are highlighted reads as having edited both.
#[test]
fn edit_refuses_more_than_one_row() {
    let dir = TempDir::new().unwrap();
    let (mut app, _, _) = two_folded_sessions(dir.path());
    type_keys(&mut app, "Vj");
    let why = app.edit_plan().err().unwrap_or_default();
    assert!(why.contains("2 rows"), "{why}");
}

/// **`y` in the graph says why** rather than doing nothing — and the range
/// goes with the Visual the keymap has already left.
#[test]
fn yank_in_the_graph_refuses_and_ends_the_range() {
    let dir = TempDir::new().unwrap();
    let (mut app, _, _) = two_folded_sessions(dir.path());
    type_keys(&mut app, "Vjy");
    assert!(
        app.notice().is_some_and(|n| n.contains("not text")),
        "{:?}",
        app.notice()
    );
    assert_eq!(graph_selection(&app), None);
}

/// **The range renders reversed, and the cursor inside it shows** — the
/// terminal's, on the cursor row's first cell, which is drawn plain so it
/// is not lost in the range (`view::place`).
#[test]
fn the_graph_draws_its_range_and_the_cursor_in_it() {
    let dir = TempDir::new().unwrap();
    let (mut app, _, _) = two_folded_sessions(dir.path());
    type_keys(&mut app, "Vj");
    let (buf, at) = cursor_of(&mut app, 100, 30);
    let pane = app.workspace.focused();
    let layout = app.workspace.frame_layout(Rect::new(0, 0, 100, 30));
    let rect = layout.pane(pane).expect("graph on screen").rect;
    let reversed = |x: u16, y: u16| buf[(x, y)].modifier.contains(Modifier::REVERSED);
    let (x, top) = (rect.x + 1, rect.y + 1);
    assert!(reversed(x + 1, top), "the anchor row is in the range");
    assert!(reversed(x + 1, top + 1), "so is the cursor row");
    assert_eq!(at, Some(Position::new(x, top + 1)), "the cursor");
    assert!(!reversed(x, top + 1), "its cell drawn plain");
    assert!(reversed(x, top), "the anchor row has no cursor");
}

// --- text pages ----------------------------------------------------------

/// `:help`, focused and drawn once, with a clipboard to watch.
fn help_page() -> (App, MemoryClipboard) {
    let mut app = test_app();
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    // The `:help` event ends there, as every event does in the loop: focus
    // moved, and `settle_focus` says so now, not at a later event's end.
    open_pane_of(&mut app, PaneKind::Page(PageKind::Help));
    (app, clip)
}

/// Lines `range` of the focused page, as a linewise yank holds them.
fn page_lines(app: &mut App, range: std::ops::RangeInclusive<usize>) -> String {
    let pane = app.workspace.focused();
    let text = app.text_pane_mut_for_test(pane).expect("a page");
    text.lines()[range]
        .iter()
        .map(|l| l.text() + "\n")
        .collect()
}

/// **`V`, `j`, `j`, `y` on a page copies those lines** — to the clipboard
/// and to the register `p` pastes in the prompt — and ends Visual.
#[test]
fn a_page_yanks_its_selected_lines() {
    let (mut app, clip) = help_page();
    type_keys(&mut app, "Vjjy");
    let want = page_lines(&mut app, 0..=2);
    assert_eq!(clip.last().as_deref(), Some(want.as_str()));
    assert_eq!(app.prompt().register_text().as_deref(), Some(want.as_str()));
    assert_eq!(app.focused_mode(), crate::keymap::Mode::Normal);
    let pane = app.workspace.focused();
    let text = app.text_pane_mut_for_test(pane).unwrap();
    assert!(!text.selecting(), "the range is spent");
}

/// `yy` and `yj` yank lines; `yiW` takes part of one — a page has columns
/// now, the transcript's (Steven, 10-07).
#[test]
fn a_page_yanks_lines_and_words_by_motion() {
    let (mut app, clip) = help_page();
    type_keys(&mut app, "yy");
    assert_eq!(clip.last(), Some(page_lines(&mut app, 0..=0)));
    type_keys(&mut app, "yj");
    assert_eq!(clip.last(), Some(page_lines(&mut app, 0..=1)));
    type_keys(&mut app, "yiW");
    assert_eq!(clip.last().as_deref(), Some("*help*"));
    assert_eq!(app.prompt().register_text().as_deref(), Some("*help*"));
}

/// **The mouse selects in a page** — a press puts the cursor on the char
/// under it, a drag (Visual from the press) takes it to the char under the
/// pointer, and the release copies what is selected (Steven, 10-07: "even
/// with the mouse, except for shift+drag. The mouse selection here is wonky
/// too"). The Drag arm's `begin_visual` is `v`'s anchor; `handle_mouse`
/// takes a live terminal, so its two halves are driven here.
#[test]
fn the_mouse_selects_in_a_page_and_a_release_copies_it() {
    let mut on = Config::default();
    on.tui.copy_on_select = true;
    let mut app = test_app_with(on);
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    // Opened as an event ends: unsettled, the next key's settle saw the
    // focus change and abandoned the Visual that key began.
    open_pane_of(&mut app, PaneKind::Page(PageKind::Help));
    app.before_frame(80, 24);
    let area = Rect::new(0, 0, 80, 24);
    let pane = app.workspace.focused();
    let rect = app
        .workspace
        .frame_layout(area)
        .pane(pane)
        .expect("on screen")
        .rect;
    // Line 0 is `*help*   Shannon Code help …`: press on its `h`.
    app.place_cursor_at_mouse_for_test(area, Position::new(rect.x + 2, rect.y + 1));
    assert_eq!(app.text_pane_mut_for_test(pane).unwrap().position(), (0, 1));
    type_keys(&mut app, "v");
    // Drag to the `p` of `help`, cell 4.
    app.place_cursor_at_mouse_for_test(area, Position::new(rect.x + 5, rect.y + 1));
    app.mouse_dragged = true;
    app.release_left();
    assert_eq!(clip.last().as_deref(), Some("help"));
}

/// **The decision's report is a page to the mouse too** (review stop 5, E2
/// #4, probed): a drag there entered Visual, the cursor stayed put, and the
/// release copied nothing, saying nothing — two of five sites spelled "a
/// text page or the decision" and these two did not (`PaneKind::is_page`).
/// And nothing is answered by it.
#[tokio::test]
async fn the_mouse_selects_in_the_decision_and_answers_nothing() {
    let mut on = Config::default();
    on.tui.copy_on_select = true;
    let mut app = test_app_with(on);
    let clip = MemoryClipboard::default();
    app.osc = Box::new(clip.clone());
    model_call(&mut app, "tu_1", "rm -rf /tmp/shannon-test-absent");
    app.set_focus(PaneKind::Tool);
    app.before_frame(80, 24);
    let area = Rect::new(0, 0, 80, 24);
    let pane = app.workspace.focused();
    let rect = app
        .workspace
        .frame_layout(area)
        .pane(pane)
        .expect("on screen")
        .rect;
    app.place_cursor_at_mouse_for_test(area, Position::new(rect.x + 1, rect.y + 1));
    assert_eq!(app.text_pane_mut_for_test(pane).unwrap().position(), (0, 0));
    type_keys(&mut app, "v");
    app.place_cursor_at_mouse_for_test(area, Position::new(rect.x + 4, rect.y + 1));
    assert_eq!(
        app.text_pane_mut_for_test(pane).unwrap().position(),
        (0, 3),
        "the cursor follows the drag"
    );
    app.mouse_dragged = true;
    app.release_left();
    assert!(
        clip.last().is_some_and(|c| c.chars().count() == 4),
        "the release copies what was dragged over: {:?}",
        clip.last()
    );
    assert!(app.shown_call().is_some(), "and answers nothing");
}

/// **A click in the prompt's insert ends its count** — `3iq`, a click past
/// the line's end, `z<Esc>` reads `qhelloz`, as an arrow ends it in vim;
/// the click replayed the whole run, `q` and all (review stop 16, A M7).
#[test]
fn a_click_in_the_prompts_insert_ends_its_count() {
    let mut app = test_app();
    type_keys(&mut app, "<Esc>ihello<Esc>0");
    app.before_frame(100, 30);
    let area = Rect::new(0, 0, 100, 30);
    let prompt = app.workspace.prompt_pane().expect("a prompt");
    let rect = app
        .workspace
        .frame_layout(area)
        .pane(prompt)
        .expect("on screen")
        .rect;
    type_keys(&mut app, "3iq");
    app.place_cursor_at_mouse_for_test(area, Position::new(rect.x + 40, rect.y + 1));
    type_keys(&mut app, "z<Esc>");
    assert_eq!(app.prompt().text(), "qhelloz");
}

/// **The cursor is not lost in a Visual range** — the prompt's and a
/// transcript's, which reverse the cell under it with the rest of the
/// range. On a grounded theme the cursor is the ground's text colour, and a
/// reversed plain cell's background is that colour too: the cursor at the
/// range's end was the selection's own colour (Reviewer D, F1 — `ui.cursor`'s
/// derivation over a selection nothing un-reversed). `view::place` draws the
/// cell under it plain, as a page's painted cursor did.
#[tokio::test]
async fn the_cursor_is_not_lost_in_a_visual_range() {
    let mut app = test_app();
    app.dispatch_command_for_test("set tui.theme=catppuccin-latte");
    assert!(app.theme().cursor().is_some(), "premise: a grounded theme");
    type_keys(&mut app, "<Esc>ihello world<Esc>vhh");
    let (buf, at) = cursor_of(&mut app, 100, 30);
    let at = at.expect("the prompt's cursor");
    let reversed = |x: u16| buf[(x, at.y)].modifier.contains(Modifier::REVERSED);
    assert_eq!(
        buf[(at.x, at.y)].symbol(),
        "r",
        "premise: on `r` of `world`"
    );
    assert!(
        reversed(at.x + 1) && reversed(at.x + 2),
        "premise: `ld` selected"
    );
    assert!(!reversed(at.x), "the prompt's cursor drawn plain");

    let mut app = test_app();
    app.dispatch_command_for_test("set tui.theme=catppuccin-latte");
    open_pane_of(&mut app, PaneKind::Output);
    // Off the rule above it, onto `> hello`.
    type_keys(&mut app, "jvl");
    let (buf, at) = cursor_of(&mut app, 100, 30);
    let at = at.expect("the transcript's cursor");
    let reversed = |x: u16| buf[(x, at.y)].modifier.contains(Modifier::REVERSED);
    assert!(reversed(at.x - 1), "premise: the anchor's cell selected");
    assert!(!reversed(at.x), "the transcript's cursor drawn plain");
}

/// **A page's own frame** at 100 × 30: its cells, where the terminal's
/// cursor is, and the content's top-left.
fn page_frame(app: &mut App) -> (ratatui::buffer::Buffer, Option<Position>, Position) {
    let (buf, at) = cursor_of(app, 100, 30);
    let pane = app.workspace.focused();
    let layout = app.workspace.frame_layout(Rect::new(0, 0, 100, 30));
    let rect = layout.pane(pane).expect("the page on screen").rect;
    (buf, at, Position::new(rect.x + 1, rect.y + 1))
}

/// **The cursor is where `w` put it** — the terminal's, on the column, and
/// no cell of the row painted for it.
#[test]
fn a_page_draws_its_cursor_where_w_put_it() {
    let (mut app, _) = help_page();
    type_keys(&mut app, "w");
    let pane = app.workspace.focused();
    let text = app.text_pane_mut_for_test(pane).unwrap().clone();
    assert_eq!(text.position(), (0, 1), "premise: past the `*`");
    let (buf, at, top) = page_frame(&mut app);
    assert_eq!(at, Some(Position::new(top.x + 1, top.y)), "on the `h`");
    assert_eq!(buf[(top.x + 1, top.y)].symbol(), "h");
    let reversed =
        (top.x..top.x + 40).any(|x| buf[(x, top.y)].modifier.contains(Modifier::REVERSED));
    assert!(!reversed, "no cell painted as the cursor");
}

/// **`w`, then `viw`, then `y` on a page copies that word** — what Steven
/// asked for: highlight text in `:help`, yank it, paste it into a prompt.
/// Through the keys, so the dispatch is the production one.
#[test]
fn a_page_selects_a_word_and_yanks_it() {
    let (mut app, clip) = help_page();
    type_keys(&mut app, "wviwy");
    assert_eq!(clip.last().as_deref(), Some("help"));
    assert_eq!(app.focused_mode(), crate::keymap::Mode::Normal);
}

/// **A page draws its range reversed, the cursor's cell plain in it**, and
/// nothing past the range.
#[test]
fn a_page_draws_its_range_and_the_cursor_in_it() {
    let (mut app, _) = help_page();
    type_keys(&mut app, "Vj");
    // Line 1 is empty and holds the cursor: its one cell is the cursor's,
    // drawn plain inside the range.
    let (buf, at, top) = page_frame(&mut app);
    let reversed = |x: u16, y: u16| buf[(x, y)].modifier.contains(Modifier::REVERSED);
    assert!(reversed(top.x, top.y), "line 0");
    assert!(!reversed(top.x, top.y + 2), "and no further");
    assert_eq!(at, Some(Position::new(top.x, top.y + 1)), "the cursor");
    assert!(!reversed(top.x, top.y + 1), "its cell drawn plain");
    // Past it, the empty line is one reversed cell, as vim draws it.
    type_keys(&mut app, "j");
    let (buf, _, _) = page_frame(&mut app);
    assert!(
        buf[(top.x, top.y + 1)]
            .modifier
            .contains(Modifier::REVERSED),
        "an empty line inside the range"
    );
}
