//! **`:graph`'s keys and their commands** (Steven, 10-14: "`c` goes",
//! "renaming in `:graph`", "make sure we have commands for the relevant
//! actions"): `c` retired with a pointer, `r` renames, every verb has a
//! command, and a command typed in the tree acts on what a key would.

use super::*;

/// Two sessions on disk, the tree open and focused on the FIRST, the second
/// in front: `(app, first, second)`.
fn tree_on_the_first(dir: &std::path::Path) -> (App, EpisodeId, EpisodeId) {
    let mut app = app_with_n_turns(dir, 2);
    let first = app.conversation().episode().id;
    let second = add_persisted_session(&mut app, "Second");
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(first);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    assert_eq!(app.conversation().episode().id, second, "premise: in front");
    (app, first, second)
}

/// **A find in the tree is refused, says why, and eats no key** — `f` `F`
/// `t` `T` `;` `,` look along a line, and the tree's rows have none. Armed,
/// `f` ate the next key (the tree's `m` among them); `;` and `,` after a
/// find elsewhere were dropped silently (review stop 16, A L4).
#[test]
fn a_find_in_the_tree_is_refused_and_eats_no_key() {
    let dir = TempDir::new().unwrap();
    let (mut app, _, _) = tree_on_the_first(dir.path());
    type_keys(&mut app, "gg");
    for key in ["f", "F", "t", "T", ";", ","] {
        let row = app.graph_view().unwrap().selected();
        type_keys(&mut app, key);
        assert!(
            app.notice().is_some_and(|n| n.contains("rows have none")),
            "`{key}`: {:?}",
            app.notice()
        );
        type_keys(&mut app, "j");
        assert_eq!(
            app.graph_view().unwrap().selected(),
            row + 1,
            "`{key}` then `j`"
        );
        type_keys(&mut app, "k");
    }
}

/// **`shannon --resume` opens the tree at launch, and the first keys are
/// the tree's** — `gg` first lands on the top row. It opens in no event
/// (`main` calls `resume_command`), and the first key's end took the focus
/// it moved for its own and dropped the `g` that began the chord: the class
/// `App::open_tutor` fixed for the tutor. Nothing settles before the keys,
/// as launch settles nothing.
#[test]
fn a_resume_at_launch_takes_gg_as_the_first_keys() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    add_persisted_session(&mut app, "Second");
    app.resume_command("");
    assert_eq!(app.focus(), PaneKind::Graph, "premise: the tree has it");
    let landed = app.graph_view().unwrap().selected();
    assert!(landed > 0, "premise: it lands below the top ({landed})");
    type_keys(&mut app, "gg");
    assert_eq!(
        app.graph_view().unwrap().selected(),
        0,
        "`gg` as the first keys"
    );
}

/// **`c` is retired, and says where its verb went** — a hand that learned it
/// reaches for it, and silence would read as a broken key; it clones
/// nothing and leaves a Visual range standing.
#[test]
fn c_says_clone_is_a_command_now_and_does_nothing() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, second) = tree_on_the_first(dir.path());
    let sessions = app.current_graph().episode_ids().len();
    assert!(
        app.graph_view_mut()
            .expect("the viewer is open")
            .anchor_visual_at(Node::Episode(first)),
        "premise: a range"
    );
    type_keys(&mut app, "c");
    assert!(
        app.graph_view().is_some_and(GraphView::in_visual),
        "a key that did nothing spends no range"
    );
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("`:clone`"), "{said}");
    assert_eq!(app.notice_kind(), Some(crate::app::NoticeKind::Refusal));
    assert_eq!(
        app.conversation().episode().id,
        second,
        "nothing switched in"
    );
    assert_eq!(
        app.current_graph().episode_ids().len(),
        sessions,
        "nothing cloned"
    );
}

/// **`r` renames the row** — the `:` line opens holding `:rename <id> `, the
/// session's name after it when it has one, and Enter names the SELECTED
/// session, not the one in front: the session `r` checked is written on the
/// line, so Enter cannot act on another.
#[test]
fn r_opens_rename_on_the_row_and_enter_names_it() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, second) = tree_on_the_first(dir.path());
    let id = first.short_hex();
    type_keys(&mut app, "r");
    let opened = format!("rename {id} ");
    assert_eq!(
        app.cmdline.line(),
        Some(opened.as_str()),
        "opened on the row"
    );
    let mut submitted = None;
    for c in "picked".chars() {
        app.command_input(key(c));
    }
    if let Some(line) = app.command_input(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) {
        submitted = Some(line.clone());
        app.dispatch_command_for_test(&line);
    }
    assert_eq!(submitted, Some(format!("rename {id} picked")));
    assert_eq!(app.label_of(first).name.as_deref(), Some("picked"));
    assert_eq!(app.label_of(second).name, None, "not the one in front");

    type_keys(&mut app, "r");
    assert_eq!(
        app.cmdline.line(),
        Some(format!("rename {id} picked").as_str()),
        "its name, to edit, as netrw's `R` offers the old one"
    );
}

/// **`r` on a Visual range names the range's session, whatever is marked
/// elsewhere** — it checked the range and spent it, and Enter re-derived the
/// target from the marks, naming a session the line never showed (review of
/// 34033e8, Reviewer A, reproduced).
#[test]
fn r_on_a_range_names_the_range_not_the_marks() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, second) = tree_on_the_first(dir.path());
    let view = app.graph_view_mut().expect("the viewer is open");
    view.reveal_and_select_episode(second);
    type_keys(&mut app, "m");
    type_keys(&mut app, "f");
    let view = app.graph_view_mut().expect("the viewer is open");
    view.reveal_and_select_episode(first);
    assert!(
        view.anchor_visual_at(Node::Episode(first)),
        "premise: a range"
    );
    type_keys(&mut app, "r");
    let line = app.cmdline.line().unwrap_or_default().to_string();
    assert!(
        line.starts_with(&format!("rename {} ", first.short_hex())),
        "{line}"
    );
    app.dispatch_command_for_test(&format!("{line}ranged"));
    assert_eq!(app.label_of(first).name.as_deref(), Some("ranged"));
    assert_eq!(app.label_of(second).name, None, "not the marked one");
}

/// **A command typed in the tree reads its marks**, as its key did: with a
/// session marked and the cursor on another, `:clone` clones the marked one.
/// It read the cursor alone, and cloned a row nobody marked.
#[test]
fn clone_typed_in_the_tree_honours_its_marks() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, second) = tree_on_the_first(dir.path());
    type_keys(&mut app, "m");
    type_keys(&mut app, "f");
    app.graph_view_mut()
        .expect("the viewer is open")
        .reveal_and_select_episode(second);
    assert_eq!(
        app.graph_view().map(|v| v.marked_nodes().len()),
        Some(1),
        "premise: one marked, the cursor elsewhere"
    );
    app.dispatch_command_for_test("clone");
    let origin = app.conversation().episode().origin.expect("a clone");
    assert_eq!(
        origin.episode, first,
        "the marked session, not the cursor's"
    );
}

/// **`:pick` away from the tree says what it needs** — it reaches from
/// anywhere, `p` only from the tree, and it returned in silence (review of
/// 34033e8, Reviewer A).
#[test]
fn pick_without_a_tree_says_so() {
    let mut app = test_app();
    app.dispatch_command_for_test("pick");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("`:graph`"), "{said}");
}

/// **The key page lists every key the tree answers** — counted from the
/// handler, `graph_verb`, over every key a terminal sends plainly, never
/// from the page: a list cannot check that it lists everything. A key whose
/// verb moved to a command is the one exception, and it must NOT be listed —
/// it does nothing but say where the verb went.
#[test]
fn the_graph_keys_page_lists_every_verb() {
    let dir = TempDir::new().unwrap();
    let (app, ..) = tree_on_the_first(dir.path());
    let listed: std::collections::HashSet<&str> = crate::help::graph_key_rows()
        .into_iter()
        .flat_map(|row| row.keys)
        .collect();
    let mut codes: Vec<KeyCode> = (' '..='~').map(KeyCode::Char).collect();
    codes.extend([
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Insert,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
    ]);
    codes.extend((1..=12).map(KeyCode::F));
    let verb = |code: KeyCode| app.graph_verb(&KeyEvent::new(code, KeyModifiers::NONE));
    let mut answered = 0;
    for code in codes {
        let Some(found) = verb(code) else { continue };
        answered += 1;
        let spelled = match code {
            KeyCode::Char(' ') => "<Space>".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Tab => "<Tab>".to_string(),
            KeyCode::BackTab => "<S-Tab>".to_string(),
            KeyCode::Enter => "<CR>".to_string(),
            // The arrows are `h` and `l`, which the page lists ("the arrows
            // too") — so they must BE `h` and `l`.
            KeyCode::Left | KeyCode::Right => {
                let letter = if code == KeyCode::Left { 'h' } else { 'l' };
                assert_eq!(Some(found), verb(KeyCode::Char(letter)), "{code:?}");
                letter.to_string()
            }
            other => panic!("the page has no spelling for {other:?} — add one to it"),
        };
        match found {
            // A key that does nothing here, said: no verb to list.
            GraphVerb::Retired(_) | GraphVerb::NotHere(_) => assert!(
                !listed.contains(spelled.as_str()),
                "`{spelled}` is no verb and must not be listed"
            ),
            // A prefix is listed as its chords.
            GraphVerb::MarkPrefix => {
                assert!(listed.contains("mf") && listed.contains("mu"), "{listed:?}");
            }
            _ => assert!(
                listed.contains(spelled.as_str()),
                "the tree answers `{spelled}` ({found:?}) and its key page does not \
                 list it: {listed:?}"
            ),
        }
    }
    assert!(
        answered > 15,
        "premise: the handler answered keys: {answered}"
    );
    // `<Esc>` is a verb only while a summary is being drawn, so the loop
    // above (no summary) cannot reach it.
    assert!(listed.contains("<Esc>"), "{listed:?}");
}

/// **`:help graph-keys` typed in the tree opens in the tree's pane, and `:q`
/// gives the tree back as it was** — the pointer the tree's status row gives
/// (Steven, 10-14). It opened across the top, because a view replaced was
/// retired — the tree's marks, its folds, its inspector; set aside now, they
/// come back, and a view opens in the pane you are in from every pane
/// (Steven, 10-16: "The chat pane version is the correct version").
#[test]
fn help_typed_in_the_tree_takes_its_pane_and_q_gives_the_tree_back() {
    let dir = TempDir::new().unwrap();
    let (mut app, ..) = tree_on_the_first(dir.path());
    assert!(
        (app.graph_view()).is_some_and(|v| app.buffers.tree(v.tree()).is_some()),
        "premise: a window on a real tree"
    );
    // Settled after each key, as the event loop does: the mark is the
    // tree's at the event's end (`settle_tree_marks`), and it held unsettled
    // only on a view with no tree (review stop 10, C 3).
    type_keys(&mut app, "m");
    type_keys(&mut app, "f");
    let tree = app.workspace.focused();
    let stood = app.graph_view().and_then(|v| v.selected_node());
    let panes = app.workspace.pane_count();
    let inspectors = |app: &App| {
        app.workspace
            .pane_ids()
            .into_iter()
            .filter(|&p| app.workspace.kind_of(p) == Some(PaneKind::Inspector))
            .count()
    };
    assert_eq!(inspectors(&app), 1, "premise: the tree has its inspector");
    assert!(stood.is_some(), "premise: the cursor stands on a node");
    app.dispatch_command_for_test("help graph-keys");
    assert_eq!(app.workspace.focused(), tree, "in the tree's own pane");
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Help),
        "the page has focus"
    );
    assert_eq!(
        app.workspace.pane_count(),
        panes - 1,
        "nothing opened, and the inspector went with the tree"
    );
    let title = app.pane_page(tree).expect("the manual").title();
    assert!(title.contains("graph-keys"), "{title}");
    let frame = Rect::new(0, 0, 120, 40);
    assert_eq!(
        app.pane_content_size(tree, frame).0,
        118,
        "the frame's width, for the table"
    );
    app.dispatch_command_for_test("q");
    assert_eq!(app.workspace.focused(), tree, "`:q` gives the tree back");
    assert_eq!(app.focus(), PaneKind::Graph);
    assert_eq!(inspectors(&app), 1, "its inspector beside it");
    assert_eq!(
        app.graph_view().map(|v| v.marked_nodes().len()),
        Some(1),
        "with its mark"
    );
    assert_eq!(
        app.graph_view().and_then(|v| v.selected_node()),
        stood,
        "where it stood"
    );
}

/// **Where you were in a tree is the window you read in**, as a page's is
/// (`where_you_were_is_the_window_you_read_in`, `PageWindow::moved`) — two
/// windows on one tree (`:sp`), read in one, focus passing through the
/// other without moving it, then the prompt. The focused tree was recorded
/// every event, so the other's row was kept (review stop 10, D 3, probe P2).
/// Read in the window FIRST in the ring (`:vs` puts the copy left): a walk
/// recording every window, moved or not, ends on the other one — the probe
/// as D wrote it read in the second, and that mutant passed it.
#[test]
fn where_you_were_in_a_tree_is_the_window_you_read_in() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let _second = add_persisted_session(&mut app, "Second");
    open_graph_pane(&mut app);
    let other = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    app.settle();
    let read = app.workspace.focused();
    assert_ne!(read, other, "premise: two windows");
    let ring = app.workspace.ring_order();
    let at = |p| ring.iter().position(|&q| q == p);
    assert!(
        at(read) < at(other),
        "premise: the one read in is first in the ring"
    );
    let tree = app
        .pane_graph(read)
        .map(GraphView::tree)
        .expect("premise: a tree");
    assert_eq!(
        app.pane_graph(other).map(GraphView::tree),
        Some(tree),
        "premise: one tree"
    );
    app.workspace.focus_pane(read);
    app.settle();
    let start = app.pane_graph(read).and_then(GraphView::spot);
    type_keys(&mut app, "jj");
    if app.pane_graph(read).and_then(GraphView::spot) == start {
        type_keys(&mut app, "kk");
    }
    let read_spot = app.pane_graph(read).and_then(GraphView::spot);
    assert_ne!(read_spot, start, "premise: read in the first");
    app.workspace.focus_pane(other);
    app.settle();
    assert_ne!(
        app.pane_graph(other).and_then(GraphView::spot),
        read_spot,
        "premise: the other is elsewhere"
    );
    // A background event's refresh builds every tree window fresh: no move
    // (`Held` carries where each was settled), or the window last in the
    // walk would be "where you were".
    app.refresh_graph_views();
    app.settle();
    app.set_focus(PaneKind::Prompt);
    app.settle();
    let kept = app.buffers.tree(tree).and_then(|t| t.left_at().cloned());
    assert_eq!(
        kept, read_spot,
        "the window read in, not the one last stood in"
    );
}

/// **The graph's status row is the graph's**: where you are and where its
/// keys are listed — not the session's model and cost, nor `^L ^C ^W`,
/// keys that do here what they do everywhere (Steven, 10-14: "We don't need
/// to show the model, ctx, tokens … Most of this scrolls off"). Not
/// `(read-only)`: its `i` says that, as vim's E21 does, and the row need not
/// (Steven, 10-27, review stop 13, S13-Q1 — Q12 had put it there).
#[test]
fn the_graph_status_row_is_its_position_and_help() {
    let dir = TempDir::new().unwrap();
    let (mut app, ..) = tree_on_the_first(dir.path());
    let row = |app: &mut App| {
        app.notices.clear();
        screen(app, 120)
            .lines()
            .last()
            .unwrap_or_default()
            .to_string()
    };
    let graph = row(&mut app);
    assert_eq!(
        graph.trim_end(),
        " [NORMAL] graph · 3/5 · help `:help graph-keys`",
        "nothing running, so nothing about `^C`"
    );
    // The graph took the transcript's pane; the prompt is the session's.
    app.set_focus(PaneKind::Prompt);
    let prompt = row(&mut app);
    assert!(
        prompt.contains("stub"),
        "premise: the session's row names its model: {prompt}"
    );
}

/// Type `/pattern<CR>` into the focused tree, through the real routing.
fn search(app: &mut App, pattern: &str) {
    search_by(app, '/', pattern);
}

/// Type `{opener}pattern<CR>` — `/` or `?` — into the focused tree.
fn search_by(app: &mut App, opener: char, pattern: &str) {
    type_keys(app, &format!("{opener}{pattern}<CR>"));
}

/// **`/` `n` `N` move through the tree by what its rows say** (Steven,
/// 10-14: "Searching is helpful for moving, not _just_ finding … search
/// what we see on the screen"). It refused, "rows are sessions, not text".
#[test]
fn slash_moves_to_the_row_that_says_it() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, second) = tree_on_the_first(dir.path());
    let at = |app: &App| app.graph_view().and_then(GraphView::selected_node);
    assert_eq!(at(&app), Some(Node::Episode(first)), "premise");
    let turns = app.current_graph().full_turns(&first);

    search(&mut app, "Hello");
    assert!(
        matches!(at(&app), Some(Node::Turn { turn, .. }) if turn == turns[0]),
        "to the first row below that says it: {:?}",
        at(&app)
    );
    type_keys(&mut app, "n");
    assert!(
        matches!(at(&app), Some(Node::Turn { turn, .. }) if turn == turns[1]),
        "`n`, the next: {:?}",
        at(&app)
    );
    type_keys(&mut app, "N");
    assert!(
        matches!(at(&app), Some(Node::Turn { turn, .. }) if turn == turns[0]),
        "`N`, back: {:?}",
        at(&app)
    );

    // Round the end, and said.
    search(&mut app, "Second");
    assert!(
        matches!(at(&app), Some(Node::Turn { episode, .. }) if episode == second),
        "{:?}",
        at(&app)
    );
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("continuing at TOP"), "{said}");

    // Found nowhere: said, and nothing moves.
    let before = at(&app);
    search(&mut app, "nowhere-at-all");
    assert_eq!(at(&app), before);
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("pattern not found"), "{said}");
}

/// **`?` searches the tree backward, as it does everywhere** (Steven, 10-14:
/// "Good point about `?` being backwards search"). For a day it opened the
/// keys page instead, and `N` after a `/` was the only way back.
#[test]
fn question_mark_searches_the_tree_backward() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, _) = tree_on_the_first(dir.path());
    let at = |app: &App| app.graph_view().and_then(GraphView::selected_node);
    let turns = app.current_graph().full_turns(&first);
    search(&mut app, "Hello");
    type_keys(&mut app, "n");
    assert!(
        matches!(at(&app), Some(Node::Turn { turn, .. }) if turn == turns[1]),
        "premise: on the second turn: {:?}",
        at(&app)
    );
    search_by(&mut app, '?', "Hello");
    assert!(
        matches!(at(&app), Some(Node::Turn { turn, .. }) if turn == turns[0]),
        "`?`, the one before: {:?}",
        at(&app)
    );
    assert_eq!(app.focus(), PaneKind::Graph, "and no page opened");
    // `n` keeps the way `?` went: back from the first match is round the
    // top — forward, it would have stepped to the second with nothing said.
    type_keys(&mut app, "n");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("continuing at BOTTOM"), "{said}");
}

/// **A row's words are searched as it draws them** — a named session's id is
/// not, once `graph_ids` hides it; shown again, it is found. A FOLDED row's
/// are, and the match opens the fold, as vim's search does (Steven, 10-15:
/// a session folded under a turn "appears it is gone"). It was "only what
/// is drawn", and a fold's rows are not drawn.
#[test]
fn slash_searches_what_the_rows_say_and_opens_a_fold() {
    let dir = TempDir::new().unwrap();
    let (mut app, first, _) = tree_on_the_first(dir.path());
    app.dispatch_command_for_test(&format!("rename {} auth-fix", first.short_hex()));
    let at = |app: &App| app.graph_view().and_then(GraphView::selected_node);

    app.dispatch_command_for_test("set tui.graph_ids=hidden");
    search(&mut app, &first.short_hex());
    let said = app.notice().unwrap_or_default();
    assert!(said.contains("pattern not found"), "hidden: {said}");

    app.dispatch_command_for_test("set tui.graph_ids=after");
    search(&mut app, &first.short_hex());
    assert_eq!(at(&app), Some(Node::Episode(first)), "shown, found");

    // Folded away, the turns' words are found, and the fold opens.
    type_keys(&mut app, "h");
    let rows = |app: &App| app.graph_view().map_or(0, GraphView::visible_len);
    let folded = rows(&app);
    search(&mut app, "Hello");
    assert!(
        matches!(at(&app), Some(Node::Turn { episode, .. }) if episode == first),
        "{:?}",
        at(&app)
    );
    assert!(rows(&app) > folded, "the fold opened");
}

/// `said` with every 8-hex run masked — a composed session's id is minted
/// fresh each run, and is the one thing two identical runs may differ in.
fn without_ids(said: Option<String>) -> Option<String> {
    let said = said?;
    let mut out = String::new();
    let mut run = String::new();
    for c in said.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_hexdigit() {
            run.push(c);
            continue;
        }
        out.push_str(if run.len() == 8 { "<id>" } else { &run });
        run.clear();
        out.push(c);
    }
    out.pop();
    Some(out)
}

/// **A key and its command do the same thing** — run on identical states,
/// each leaves the same tree, question, notice (but the fresh id) and line.
/// Async because `S` starts a request.
#[tokio::test]
async fn a_key_and_its_command_agree() {
    // With a TURN row marked: `d` asks to delete the turn, and bare `:delete`
    // must too (it refused turn rows after `d` learned to take them).
    let pairs: [(char, &str); 3] = [('p', "pick"), ('S', "distill"), ('d', "delete")];
    for (k, command) in pairs {
        let run = |by_key: bool| {
            let dir = TempDir::new().unwrap();
            let (mut app, ..) = tree_on_the_first(dir.path());
            // One mark, so `p` has a set and `S` a range to read.
            type_keys(&mut app, "j");
            type_keys(&mut app, " ");
            if by_key {
                type_keys(&mut app, &k.to_string());
            } else {
                app.dispatch_command_for_test(command);
            }
            let probe = graph_probe(&app);
            (
                probe.view,
                probe.confirms,
                without_ids(probe.notice),
                probe.distilling,
                probe.command_line,
            )
        };
        let (pressed, typed) = (run(true), run(false));
        assert_eq!(pressed, typed, "`{k}` and `:{command}`");
        assert_ne!(
            pressed,
            {
                let dir = TempDir::new().unwrap();
                let (mut app, ..) = tree_on_the_first(dir.path());
                type_keys(&mut app, "j");
                type_keys(&mut app, " ");
                app.notices.clear();
                let probe = graph_probe(&app);
                (
                    probe.view,
                    probe.confirms,
                    without_ids(probe.notice),
                    probe.distilling,
                    probe.command_line,
                )
            },
            "premise: `{k}` did something to compare"
        );
    }
}

/// **`:graph` is a workbench**: every composing key leaves the tree open and
/// rebuilt, so you can operate several times without reopening it.
///
/// The policy was a coin-flip per key until it was settled deliberately
/// (2026-08-03): `c`/`m`/`r` stayed, `p`/`s`/`e` closed the viewer. Both
/// halves are asserted here because the failure mode is "someone adds a key and
/// picks one at random" — the same accretion that produced the split.
/// `<CR>` is the key that leaves, and it has its own test.
#[test]
fn every_composing_key_leaves_the_graph_open_and_refreshed() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    for (key, label) in [('c', "clone"), ('p', "pick"), ('m', "merge")] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 3); // base A→B→C
        let base = app.conversation().episode().id;
        // A real branch off turn 1, so `m` has something to merge — without one
        // it refuses, the tree stays for the WRONG reason, and this test passes
        // while measuring nothing.
        let t1 = app.conversation().episode().own_turns()[1];
        app.checkout_turn(base, t1, 1);
        app.conversation_mut().begin_turn("branch prompt");
        app.conversation_mut().push_chunk("branch reply");
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();
        let branch = app.conversation().episode().id;

        let target = if key == 'm' { branch } else { base };
        let mut gv = GraphView::new(app.current_graph());
        gv.reveal_and_select_episode(target);
        open_graph_pane(&mut app);
        let pane = app.workspace.focused();
        app.set_graph_view_for_test(gv);
        // `p` needs something marked; the rest act on the selection.
        if key == 'p' {
            app.graph_key_for_test(k(KeyCode::Char('j')));
            app.graph_key_for_test(k(KeyCode::Char(' ')));
        }
        let before = app.conversation().episode().id;

        // Merge is `:merge` now, not a key — `m` is netrw's mark prefix — and
        // clone is `:clone` (`c` retired, Steven 10-14). Both are still in this
        // family: the rule under test is "a composing verb leaves the tree
        // open and refreshed", and that is a property of the verb, not of how
        // it is spelled. Driven through the resolver so the table row is in
        // the loop ([[feedback_wiring_is_untested]]).
        match key {
            'm' => app.dispatch_command_for_test("merge"),
            'c' => app.dispatch_command_for_test("clone"),
            _ => {
                app.graph_key_for_test(k(KeyCode::Char(key)));
                false
            }
        };

        assert_ne!(
            app.conversation().episode().id,
            before,
            "`{key}` ({label}) actually composed something — otherwise this test \
             measures a refusal"
        );
        assert_eq!(
            app.workspace.kind_of(pane),
            Some(PaneKind::Graph),
            "`{key}` ({label}) kept the tree"
        );
        assert!(
            app.graph_view().is_some(),
            "`{key}` ({label}) left a rebuilt tree, not an empty pane"
        );
    }
}

/// `:return` (it was `r`) obeys the same workbench rule. Separate because a
/// return needs a finished child to return, which needs an async runtime to
/// spawn.
#[tokio::test]
async fn return_also_leaves_the_graph_open() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let parent = app.conversation().episode().id;
    let child = app.spawn_child_job("compute the answer", None).unwrap();
    app.feed_job(child, StreamMsg::Chunk("42".into()));
    app.feed_job(child, StreamMsg::Complete);

    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(child);
    open_graph_pane(&mut app);
    let pane = app.workspace.focused();
    app.set_graph_view_for_test(gv);

    app.dispatch_command_for_test("return");

    assert_ne!(
        app.conversation().episode().id,
        parent,
        "`:return` composed the result into the parent"
    );
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Graph),
        "`:return` kept the tree"
    );
    assert!(app.graph_view().is_some(), "and rebuilt it");
}

/// **`<CR>` in `:graph` opens in the tree's own window, the tree kept** —
/// netrw's `<CR>` (Steven, 10-25, V4b Q1) — never the first pane in ring
/// order (review 2026-10-15, Reviewer A F3, its probe 10).
///
/// B on the left, A on the right, standing on the right: `:sg`, then `<CR>`
/// on A. The graph closed FIRST, `Focus::reconcile` put focus ring-first —
/// the left pane, nothing the user chose — and the landing took that pane: A
/// in both, B off screen. Then it closed and landed where its space went
/// back, the tree dropped. The lent form (`:graph` over the transcript) is
/// the rule's other branch: there the tree's window is the transcript's.
#[test]
fn enter_in_a_graph_opens_in_its_window_and_keeps_it() {
    for split in [true, false] {
        let dir = TempDir::new().unwrap();
        let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
        // Named by ring order, not by the fixture: a vertical split puts the
        // new pane on the left, first in the ring, as vim does by default.
        let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
        let (left, right) = (panes[0], panes[1]);
        assert_eq!(
            app.workspace.ring_order().first(),
            Some(&left),
            "premise: the left pane is first in ring order"
        );
        app.workspace.focus_pane(left);
        app.focus_moved();
        app.open_by_hash(&b.to_hex()[..8]);
        app.workspace.focus_pane(right);
        app.focus_moved();
        app.open_by_hash(&a.to_hex()[..8]);

        app.dispatch_command_for_test(if split { "sgraph" } else { "graph" });
        assert_eq!(app.focus(), PaneKind::Graph, "split={split}: in the graph");
        let tree = app.workspace.focused();
        assert_eq!(tree == right, !split, "premise: lent, or a pane of its own");
        if let Some(view) = app.graph_view_mut() {
            view.reveal_and_select_episode(a);
        }
        app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.workspace.focused(), tree, "split={split}: its window");
        assert_eq!(app.workspace.kind_of(tree), Some(PaneKind::Output));
        assert_eq!(
            app.pane_session(tree).conversation.episode().id,
            a,
            "split={split}: A there"
        );
        assert!(keeps_a_tree(&app, tree), "split={split}: the tree kept");
        assert_eq!(
            app.pane_session(right).conversation.episode().id,
            a,
            "split={split}: the right pane still A"
        );
        assert_eq!(
            app.pane_session(left).conversation.episode().id,
            b,
            "split={split}: the left pane is untouched"
        );
    }
}

/// **A graph split from the PROMPT opens the session in its own window**
/// too, and touches neither pane beside it (review 2026-10-16, Reviewer B
/// F3, its probe 5). It closed first: its sibling was the prompt bar, which
/// cannot host a session, the landing dropped it, focus fell ring-first,
/// and `<CR>` on an off-screen session replaced the LEFT pane's — A F3's
/// own symptom. A split's pane is a peer, flexible, so it hosts (V4b Q1).
#[test]
fn enter_in_a_graph_split_from_the_prompt_opens_in_its_window() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    // A third session, in no pane: the one `<CR>` will open.
    let c = add_persisted_session(&mut app, "C0");
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (left, right) = (panes[0], panes[1]);
    app.focus_pane(left);
    app.open_by_hash(&b.to_hex()[..8]);
    app.focus_pane(right);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    app.set_focus(PaneKind::Prompt);
    assert_eq!(
        app.prompt_target(),
        a_buffer,
        "premise: aimed at A, on the right"
    );
    assert_eq!(
        app.workspace.ring_order().first(),
        Some(&left),
        "premise: ring-first"
    );
    assert!(
        panes
            .iter()
            .all(|&p| app.pane_session(p).conversation.episode().id != c),
        "premise: C is in no pane"
    );

    app.dispatch_command_for_test("sgraph");
    assert_eq!(app.focus(), PaneKind::Graph, "premise: in the graph");
    let tree = app.workspace.focused();
    if let Some(view) = app.graph_view_mut() {
        view.reveal_and_select_episode(c);
    }
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(
        app.pane_session(tree).conversation.episode().id,
        c,
        "opened in the tree's own window"
    );
    assert!(keeps_a_tree(&app, tree), "the tree kept");
    assert_eq!(
        app.pane_session(right).conversation.episode().id,
        a,
        "the aimed pane is untouched"
    );
    assert_eq!(
        app.pane_session(left).conversation.episode().id,
        b,
        "the ring-first pane is untouched"
    );
}

/// **…and over a view it set aside, where the graph stood** — `:graph`
/// typed in a `:messages` pane sets the messages aside (Steven, 10-16);
/// `<CR>` opens the session in that pane, the tree and the messages kept
/// in its history (V4b Q1) — not in the pane beside it, the messages left
/// showing where the graph was.
#[test]
fn enter_in_a_graph_over_a_view_lands_where_the_graph_stood() {
    let dir = TempDir::new().unwrap();
    let (mut app, ..) = app_with_two_sessions(dir.path());
    let c = add_persisted_session(&mut app, "C0");
    app.split_command("messages", SplitDir::Horizontal);
    let pane = app.workspace.focused();
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Messages)),
        "premise"
    );
    app.dispatch_command_for_test("graph");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Graph),
        "premise: the graph over the messages"
    );
    if let Some(view) = app.graph_view_mut() {
        view.reveal_and_select_episode(c);
    }
    app.graph_key_for_test(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.workspace.focused(), pane, "where the graph stood");
    assert_eq!(app.workspace.kind_of(pane), Some(PaneKind::Output));
    assert_eq!(app.pane_session(pane).conversation.episode().id, c);
    assert!(
        !app.workspace.has(PaneKind::Page(PageKind::Messages)),
        "the messages under it not shown"
    );
    assert!(keeps_a_tree(&app, pane), "the tree kept");
}

/// **A delete spends the tree's marks, every window's, from the window it
/// was asked in** — the marks are the one tree's, netrw's local marked list
/// (Steven, 10-26: one tree), and focus moving off every tree while the
/// question is up does not lose the spend (`ConfirmAction::Delete`'s
/// `from`: asked of the focused pane, the prompt has no tree to spend in).
/// Refused by the store, so the row stays and its marks can be seen. It was
/// "only the tree it was asked from", while each `:graph` was a tree of its
/// own.
#[test]
fn a_delete_spends_the_trees_marks_from_the_window_it_was_asked_in() {
    let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let (mut app, refused, a, _, _, _) = two_buffers_on_a_store_refusing_deletes();
    refused.lock().unwrap().push(a);
    app.open_graph_command();
    let asked = app.workspace.focused();
    app.split_command("graph", SplitDir::Vertical);
    app.settle();
    let other = app.workspace.focused();
    assert_ne!(asked, other, "premise: two windows");
    app.graph_view_mut().unwrap().reveal_and_select_episode(a);
    app.graph_key_for_test(k(' '));
    let marks = |app: &App, pane| app.pane_graph(pane).unwrap().marked_nodes();
    assert_eq!(
        marks(&app, asked),
        vec![Node::Episode(a)],
        "premise: marked in the other window, the tree's"
    );
    app.focus_pane(asked);
    app.graph_key_for_test(k('d'));
    assert!(app.confirm_prompt().is_some(), "premise: asked");
    app.set_focus(PaneKind::Prompt);
    type_keys(&mut app, "y");
    assert!(
        app.current_graph().episode_ids().contains(&a),
        "premise: refused"
    );
    assert!(marks(&app, asked).is_empty(), "spent where it was asked");
    assert!(
        marks(&app, other).is_empty(),
        "and so in every window on the tree"
    );
}

/// **A split of a tree is a second window on the same tree** (V4, V4b.4) —
/// it copies what vim's `:split` copies (measured in Neovim 0.12): the
/// cursor, the folds, the top; and the marks are the TREE's, shared both
/// ways, as netrw's are across `:split` (Steven, 10-25: "go with neovim's
/// way", measured). A split built a fresh tree, back on your session with
/// every fold at its default, as `:sp` of a page once did (docketed 10-21);
/// then it copied all but the marks; then the marks were each window's,
/// copied at the split.
#[test]
fn a_split_tree_is_a_second_window_on_it_its_marks_shared() {
    let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let (mut app, _, a, _, b, _) = two_buffers_on_a_store_refusing_deletes();
    assert_eq!(app.conversation().episode().id, b, "premise: in b");
    app.open_graph_command();
    let from = app.workspace.focused();
    app.graph_view_mut().unwrap().reveal_and_select_episode(a);
    app.graph_key_for_test(k(' '));
    // Each key's event ends as the loop ends it: the tree takes the mark.
    app.settle();
    let view = app.graph_view_mut().unwrap();
    view.reveal_and_select_episode(a);
    view.toggle();
    let at_a = |app: &App, pane| {
        let tree = app.pane_graph(pane).unwrap();
        let row = (tree.window(20).into_iter())
            .find(|r| r.selected)
            .map(|r| r.fold);
        (tree.selected_entry(), row)
    };
    let a_closed = (
        Some(crate::graph_view::Entry::Node(Node::Episode(a))),
        Some(Some(false)),
    );
    assert_eq!(at_a(&app, from), a_closed, "premise: on a, its fold closed");

    app.split_command("", SplitDir::Horizontal);
    app.settle();
    let copy = app.workspace.focused();
    assert_ne!(copy, from, "premise: split");
    assert_eq!(
        at_a(&app, copy),
        a_closed,
        "the cursor and the fold, copied"
    );
    let marks = |app: &App, pane| app.pane_graph(pane).unwrap().marked_nodes();
    assert_eq!(
        marks(&app, copy),
        vec![Node::Episode(a)],
        "the marks, copied"
    );
    assert_eq!(marks(&app, from), vec![Node::Episode(a)], "and kept");
    let tree = |app: &App, pane| app.pane_graph(pane).map(GraphView::tree);
    assert!(tree(&app, copy).is_some(), "premise: on a tree");
    assert_eq!(tree(&app, copy), tree(&app, from), "the same tree");

    // Unmarked in the copy: unmarked in the tree, so in the source too, at
    // the event's end as the loop settles it.
    app.graph_key_for_test(k(' '));
    assert!(
        marks(&app, copy).is_empty(),
        "premise: unmarked in the copy"
    );
    app.settle();
    assert!(
        marks(&app, from).is_empty(),
        "the tree's marks: the source's too"
    );
    // And marked in the source, marked in the copy.
    app.focus_pane(from);
    app.graph_view_mut().unwrap().reveal_and_select_episode(b);
    app.graph_key_for_test(k(' '));
    assert_eq!(marks(&app, copy), vec![Node::Episode(b)], "both ways");
}

/// **A tree set aside neither loses nor keeps back a mark** (V4b verify,
/// H1, its probe). The same tree shown in one window and set aside in
/// another: a row made since is in the shown one only, since a tree set
/// aside is not rebuilt; marked there, the mark is the tree's — and it was
/// gone at the next key, the set-aside window (no row for it) giving the
/// tree its stale marks. Brought back, that window shows the tree's marks.
#[test]
fn a_tree_set_aside_neither_loses_nor_keeps_back_a_mark() {
    let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let (mut app, _, _, here, other) = app_with_two_sessions(dir.path());
    let tree = |app: &App, pane| app.pane_graph(pane).map(GraphView::tree);
    app.focus_pane(here);
    app.dispatch_command_for_test("graph");
    let set_aside = tree(&app, here);
    app.dispatch_command_for_test("help");
    app.focus_pane(other);
    app.dispatch_command_for_test("graph");
    assert_eq!(
        tree(&app, other),
        set_aside,
        "premise: the tree no window shows"
    );
    let c = add_persisted_session(&mut app, "C0");
    app.refresh_graph_views();
    app.settle();
    app.graph_view_mut().unwrap().reveal_and_select_episode(c);
    app.graph_key_for_test(k(' '));
    let marks = |app: &App| app.graph_view().unwrap().marked_nodes();
    assert_eq!(marks(&app), vec![Node::Episode(c)], "premise: marked");

    app.graph_key_for_test(k('j'));
    assert_eq!(
        marks(&app),
        vec![Node::Episode(c)],
        "still marked at the next key"
    );

    app.focus_pane(here);
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Graph),
        "premise: back"
    );
    assert_eq!(
        app.pane_graph(here).unwrap().marked_nodes(),
        vec![Node::Episode(c)],
        "brought back, it shows the tree's marks"
    );
}

/// **The session tree is one buffer, as a netrw directory is** (measured at
/// review stop 10, Neovim 0.12.6, netrw v184: opened again by any spelling,
/// shown elsewhere or hidden, the same buffer, its local marked list with
/// it; Steven, 10-26: "we only have to handle the one 'file' but just in
/// different views"). `:sg` from a tree is a new window on it, built fresh
/// — netrw's `:Sexplore` — its marks the tree's, both ways; `:graph` in the
/// pane showing it lists it again (as `:e` of the file a window shows reads
/// it again), its marks kept; no window showing it, `:ls!` lists it once and
/// the next `:graph` shows it again, its marks with it. Each `:graph` shown
/// together was a tree of its own, on a record of netrw that did not
/// reproduce.
#[test]
fn trees_are_netrws_listings() {
    let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let (mut app, _, a, _, b, _) = two_buffers_on_a_store_refusing_deletes();
    let tree = |app: &App, pane| app.pane_graph(pane).map(GraphView::tree);
    let marks = |app: &App, pane| app.pane_graph(pane).unwrap().marked_nodes();
    app.open_graph_command();
    let first = app.workspace.focused();
    let the_tree = tree(&app, first);
    assert!(the_tree.is_some(), "premise: a tree");
    app.graph_view_mut().unwrap().reveal_and_select_episode(a);
    app.graph_key_for_test(k(' '));

    app.dispatch_command_for_test("sgraph");
    let second = app.workspace.focused();
    assert_ne!(second, first, "premise: split");
    assert_eq!(tree(&app, second), the_tree, ":sg — the same tree");
    assert_eq!(
        marks(&app, second),
        vec![Node::Episode(a)],
        "its marks, the tree's"
    );
    app.graph_view_mut().unwrap().reveal_and_select_episode(b);
    app.graph_key_for_test(k(' '));
    let first_marks = marks(&app, first);
    assert!(
        first_marks.len() == 2 && first_marks.contains(&Node::Episode(b)),
        "marked in the second, marked in the first: {first_marks:?}"
    );

    app.focus_pane(first);
    app.dispatch_command_for_test("graph");
    assert_eq!(
        tree(&app, first),
        the_tree,
        ":graph here: the same tree, listed again"
    );
    assert_eq!(marks(&app, first).len(), 2, "its marks kept");

    // Hidden — no window shows it — and shown again, its marks with it.
    app.dispatch_command_for_test("q");
    app.focus_pane(second);
    app.dispatch_command_for_test("q");
    let listed = app.buffers_listing(true);
    let rows = (listed.lines())
        .filter(|l| l.trim_start().starts_with('b') && l.contains("the session tree"))
        .count();
    assert_eq!(rows, 1, "listed once while hidden:\n{listed}");
    app.dispatch_command_for_test("graph");
    let pane = app.workspace.focused();
    assert_eq!(tree(&app, pane), the_tree, "the tree, shown again");
    assert_eq!(marks(&app, pane).len(), 2, "its marks with it");
    assert_eq!(app.buffers.trees().count(), 1, "one tree throughout");
}

/// **Every key the graph's key page lists does something** — the keys read
/// from the page a person reads (`help::graph_key_rows`), since the status
/// row's legend became a pointer to it (Steven, 10-14). The legend was a literal at
/// the render site once, advertising `q close` while the handler took `<Esc>`
/// alone — a claim about keys stated where no test could see it (Reviewer A).
///
/// "Does something" is deliberately generous: an action OR a **visible
/// refusal** passes, because a key that explains why it cannot act is working
/// as designed ([[feedback_visible_refusals]]). What fails is silence.
///
/// Two keys are not pressed, each named rather than hidden: `e` takes
/// `$EDITOR` through the terminal (`handle_graph_key`, which needs a real
/// `ShannonTerminal`), and `<Esc>` acts only while a summary is being drawn,
/// which this harness does not stream — `the_graph_keys_page_lists_every_verb`
/// holds both to the handler. Every command on the page resolves.
#[test]
fn every_key_the_graph_keys_page_lists_does_something() {
    let rows = crate::help::graph_key_rows();
    assert!(rows.len() > 15, "premise: the page parsed: {rows:?}");
    let mut advertised: Vec<(&str, Vec<KeyCode>)> = Vec::new();
    for row in &rows {
        // A row says what it does, in its column — one out of line would
        // read here as keys with nothing to say, or words as keys.
        assert!(
            !row.does.is_empty() && !row.keys.iter().any(|k| k.len() > 10),
            "a row out of its columns (6, 18, 32): {row:?}"
        );
        if let Some(command) = row.command {
            assert!(
                !matches!(
                    crate::command::resolve(command.trim_start_matches(':')),
                    crate::command::Resolved::Unknown
                ),
                "the page lists {command}, which is no command"
            );
        }
        for &key in &row.keys {
            let presses = match key {
                "e" | "<Esc>" => continue,
                "<CR>" => vec![KeyCode::Enter],
                "<Tab>" => vec![KeyCode::Tab],
                "<S-Tab>" => vec![KeyCode::BackTab],
                "<Space>" => vec![KeyCode::Char(' ')],
                // `mu` only has something to undo once something is marked,
                // so mark first: a gate whose premise is unmet fails exactly
                // like the bug.
                "mu" => "mfmu".chars().map(KeyCode::Char).collect(),
                // netrw's chords, driven as the two keys they are — which is
                // also what proves the `m` prefix reaches them.
                other => other.chars().map(KeyCode::Char).collect(),
            };
            advertised.push((key, presses));
        }
    }

    for (token, keys) in &advertised {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 3);
        // A second session, so `Tab` has somewhere to go.
        app.reset_foreground();
        app.conversation_mut().begin_turn("second".to_string());
        app.conversation_mut().push_chunk("ok");
        app.conversation_mut().commit_turn();
        app.persist_committed_turn();

        let mut gv = GraphView::new(app.current_graph());
        gv.select_top();
        open_graph_pane(&mut app);
        app.set_graph_view_for_test(gv);
        // Stand on the first TURN row: the row where every one of these keys
        // has a meaning. `l` expands it, `h` leaves it, `r` renames its
        // session, `d` asks about it.
        app.graph_key_for_test(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));

        // Premises, asserted rather than assumed — each one has its own way of
        // making a live key look dead ([[feedback_assert_the_subject]]).
        let view = app.graph_view().expect("premise: the viewer is open");
        assert!(
            view.graph().episode_ids().len() >= 2,
            "premise: `Tab` needs a second session to reach"
        );
        assert!(
            matches!(view.selected_node(), Some(Node::Turn { .. })),
            "premise: standing on a turn row, not the episode row"
        );
        // **And a session BELOW it.** `Tab` searches forward and is a
        // deliberate no-op past the last one, so run from the newest session
        // (sorted to the top) and the older one is the row it reaches. Started
        // from the bottom session this gate reported `Tab` dead — a premise
        // failing exactly like the bug it hunts.
        assert_eq!(
            view.selected(),
            1,
            "premise: row 0 is the newest session, row 1 its first turn"
        );

        let unbound = KeyEvent::new(KeyCode::F(12), KeyModifiers::NONE);
        app.graph_key_for_test(unbound);
        let before = graph_probe(&app);
        app.graph_key_for_test(unbound);
        assert_eq!(
            graph_probe(&app),
            before,
            "premise: an unbound key must change nothing, or this gate cannot \
             tell a live key from ambient settling"
        );

        for code in keys {
            app.graph_key_for_test(KeyEvent::new(*code, KeyModifiers::NONE));
        }
        assert_ne!(
            graph_probe(&app),
            before,
            "the key page lists {token:?} and nothing observable happened"
        );
    }
}

/// **`d` acts on the marked set, and the confirmation counts it.**
///
/// `<Space>`/`mf` marks were honoured by `p` and `S` and ignored by `d`, which
/// read `selected_node` alone. Steven marked two sessions, pressed `d`, and got
/// "delete session 7e1f00d1 permanently?" — one name for two marks. He answered
/// `n`, so which one it meant stayed unknown until now: it deleted one, and the
/// marks were the lie. Six verbs each answered "what do I act on?" for
/// themselves (`[[feedback_one_door_per_invariant]]`).
#[test]
fn d_in_graph_deletes_every_marked_session() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    add_persisted_session(&mut app, "C0"); // active, deliberately unmarked

    open_graph(&mut app);
    for id in [a, b] {
        app.graph_view_mut().unwrap().reveal_and_select_episode(id);
        app.graph_key_for_test(k(KeyCode::Char(' ')));
    }

    app.graph_key_for_test(k(KeyCode::Char('d')));
    // The prompt reads its count off the action, so it cannot name one while
    // deleting another.
    let prompt = app.confirm_prompt().expect("a question is up").text();
    assert!(
        prompt.contains("2 sessions"),
        "the confirmation must count the marks: {prompt}"
    );

    let action = app.pending_confirm.pop_front().unwrap();
    app.run_confirmed(action);
    let left = app.current_graph().episode_ids();
    assert!(!left.contains(&a) && !left.contains(&b), "both marked went");
}

/// **A turn row deletes the TURN — never its session.**
///
/// `node_episode` mapped Turn/Message to its Episode, so `d` on a turn removed
/// the whole session — delete on a *file* removing the *directory*. The
/// confirmation named the session, so the target was visible; what was hidden
/// is that the verb had **changed** target
/// ([[project_shannon_no_silent_fallback]]).
///
/// The first fix refused any selection containing a turn, which stopped the
/// escalation and also stopped the operation — a generalisation nobody decided,
/// and one that blocked deleting a marked range because a single row in it was
/// a turn (Steven, drive round 4). So the invariant this test guards is
/// unchanged and its expectation is not: pressing `d` on a turn stages a delete
/// of **that turn**, and the session stays.
#[test]
fn d_in_graph_deletes_the_turn_and_never_its_session() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let episode = app.conversation().episode().id;

    open_graph(&mut app);
    app.graph_view_mut()
        .unwrap()
        .reveal_and_select_episode(episode);
    // Into the session's turns: expand, then step onto the first turn row.
    app.graph_key_for_test(k(KeyCode::Char('l')));
    app.graph_key_for_test(k(KeyCode::Char('j')));
    let Some(Node::Turn { index, .. }) = app.graph_view().and_then(GraphView::selected_node) else {
        panic!("premise: the cursor is on a turn row");
    };

    app.graph_key_for_test(k(KeyCode::Char('d')));
    match app.pending_confirm.front() {
        Some(ConfirmAction::Delete {
            episodes,
            turns,
            leaves_gap,
            ..
        }) => {
            assert!(
                episodes.is_empty(),
                "the session must not be swept along with the row: {episodes:?}"
            );
            assert_eq!(turns.len(), 1, "exactly the row under the cursor");
            assert_eq!(turns[0].episode, episode);
            assert_eq!(turns[0].index, index, "the OCCURRENCE, not a lookup by id");
            assert!(
                *leaves_gap,
                "turn {index} of two is not trailing, so something still \
                 answers it and the user is told"
            );
        }
        other => panic!("expected a turn delete, got {other:?}"),
    }

    let action = app.pending_confirm.pop_front().unwrap();
    app.run_confirmed(action);
    assert!(
        app.current_graph().episode_ids().contains(&episode),
        "the session survives its turn"
    );
    assert_eq!(
        app.current_graph().full_turns(&episode).len(),
        1,
        "and is one turn shorter"
    );
}

// --- stage B: `H` `M` `L` and `z` in `:graph` and the decision pane ---------

/// **`H` `M` `L` select the top, middle and bottom rows of `:graph`'s window**,
/// through the real keys. The rule is pinned against Neovim in `viewport`;
/// this pins that the graph reaches it. They did nothing here: the graph
/// hand-matched its own keys and never learned them.
#[test]
fn h_m_l_select_the_top_middle_and_bottom_graph_rows() {
    let (mut app, h) = graph_of(60, 0);
    type_keys(&mut app, "30G");
    let top = graph_top(&app, h);
    assert!(top > 0, "premise: scrolled into the tree");
    type_keys(&mut app, "H");
    assert_eq!(graph_sel(&app), top, "H");
    type_keys(&mut app, "3H");
    assert_eq!(graph_sel(&app), top + 2, "3H");
    type_keys(&mut app, "L");
    assert_eq!(graph_sel(&app), top + h - 1, "L");
    type_keys(&mut app, "M");
    assert_eq!(graph_sel(&app), top + (h - 1) / 2, "M");
    assert_eq!(graph_top(&app, h), top, "none of them moved the window");

    // No frame between `G` and `H`: the window `H` reads is the one `50G`
    // leaves behind, not the one last drawn — `H` settles it first. The
    // expected top is worked out, not read back: reading the window AFTER `H`
    // settles it around wherever `H` landed, and agrees with a wrong answer.
    type_keys(&mut app, "gg");
    type_keys(&mut app, "50GH");
    assert_eq!(
        graph_sel(&app),
        49 - (h - 1),
        "H after a jump, before any frame: row 50 is the window's last"
    );
}

/// **Counts reach `:graph`** (`5j`, `{n}G`) now that its navigation is the
/// keymap's — and a verb DROPS a count rather than leaving it on the keymap to
/// multiply the next motion.
#[test]
fn counts_move_the_graph_selection_and_a_verb_drops_its_count() {
    let (mut app, _) = graph_of(20, 0);
    type_keys(&mut app, "gg5j");
    assert_eq!(graph_sel(&app), 5, "5j");
    type_keys(&mut app, "2k");
    assert_eq!(graph_sel(&app), 3, "2k");
    type_keys(&mut app, "12G");
    assert_eq!(graph_sel(&app), 11, "12G is the twelfth row");
    type_keys(&mut app, "3 ");
    assert_eq!(
        app.graph_view().unwrap().marked_nodes().len(),
        1,
        "3<Space> marks the row, once"
    );
    type_keys(&mut app, "j");
    assert_eq!(
        graph_sel(&app),
        12,
        "the 3 did not survive the verb to make this `3j`"
    );
}

/// **`zt` `zz` `zb` place `:graph`'s selection, and `zt` near the end scrolls
/// past it** — a text page's rules, with `~` below in `NonText`, asserted on
/// the rendered cells.
#[test]
fn z_places_the_graph_selection_and_zt_scrolls_past_the_end() {
    let (mut app, h) = graph_of(60, 0);
    type_keys(&mut app, "30Gzt");
    assert_eq!(graph_top(&app, h), 29, "zt");
    type_keys(&mut app, "zb");
    assert_eq!(graph_top(&app, h), 30 - h, "zb");
    type_keys(&mut app, "zz");
    assert_eq!(graph_top(&app, h), 29 - (h - 1) / 2, "zz");
    // `:h zt`: a count is the LINE to put there.
    type_keys(&mut app, "20zt");
    assert_eq!((graph_sel(&app), graph_top(&app, h)), (19, 19), "20zt");

    type_keys(&mut app, "Gzt");
    assert_eq!(
        graph_top(&app, h),
        60,
        "the last row at the top — past the end"
    );
    type_keys(&mut app, "k");
    assert_eq!(
        graph_top(&app, h),
        59,
        "k moves the window only as far as the cursor needs, as in vim"
    );

    let frame = frame_of(&mut app, 80, 24);
    let pane = app.workspace.focused();
    let rect = app
        .workspace
        .frame_layout(Rect::new(0, 0, 80, 24))
        .pane(pane)
        .expect("the graph is drawn")
        .rect;
    let buf = &frame;
    let tildes = (rect.y..rect.y + rect.height)
        .filter(|&y| {
            let cell = &buf[(rect.x + 1, y)];
            cell.symbol() == "~" && cell.style().fg == Some(crate::palette::NON_TEXT)
        })
        .count();
    assert_eq!(
        tildes,
        h - 2,
        "two rows drawn, `~` below:\n{}",
        rendered_text(buf)
    );
}

/// **A `z` typed on the keymap owns the next key, even a graph verb.** `zd` is
/// a dead `z` chord, never a delete — the rule the graph's own `md` keeps, now
/// for a prefix it does not own. While `3d` IS a delete: a count is not a
/// prefix, and must not turn the verb into the keymap's read-only refusal.
#[test]
fn a_z_prefix_owns_the_next_key_even_a_graph_verb() {
    let (mut app, _) = graph_of(3, 0);
    type_keys(&mut app, "zd");
    assert!(
        app.pending_confirm.is_empty(),
        "`zd` must not reach the delete confirmation"
    );
    type_keys(&mut app, "3d");
    assert!(
        !app.pending_confirm.is_empty(),
        "`3d` deletes: a count is not a prefix"
    );
}
