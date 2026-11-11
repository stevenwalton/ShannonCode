//! The graph viewer: what its tree shows and how it follows the store.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **Every composing key in `:graph` says why when nothing is selected.**
///
/// `d` `c` `m` `r` returned silently — six members of a family, one (`s`)
/// explaining itself and the rest reading as dead keys. This is the class
/// `App::refuse`'s own doc says was closed: "the rule had been applied to the
/// instance that was reported rather than to the class" (Reviewer D).
///
/// `m` left this family when merge became `:merge` and `m` became netrw's mark
/// prefix. A prefix that arms and waits is not a dead key — it is a key with
/// nothing to say *yet* — so it is deliberately not asserted here. `c` and the
/// old `r` left it as keys and stay in it as commands (`:clone`, `:return`),
/// beside `r`'s rename and `:rename`: the rule is the verb's, however spelled.
#[test]
fn the_composing_keys_say_why_with_nothing_selected() {
    let presses: [(&str, Press); 7] = [
        ("d", |app| {
            app.graph_key_for_test(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
        }),
        ("r", |app| {
            app.graph_key_for_test(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
        }),
        (":clone", |app| {
            app.dispatch_command_for_test("clone");
        }),
        (":return", |app| {
            app.dispatch_command_for_test("return");
        }),
        (":rename x", |app| {
            app.dispatch_command_for_test("rename x");
        }),
        (":merge", |app| {
            app.dispatch_command_for_test("merge");
        }),
        (":description x", |app| {
            app.dispatch_command_for_test("description x");
        }),
    ];
    for (key, press) in presses {
        let mut app = test_app();
        app.open_graph_command();
        app.workspace.focus_kind(PaneKind::Graph);
        // A graph view with no selection at all.
        app.set_graph_view_for_test(GraphView::new(shannon_core::Graph::new()));
        app.notices.clear();

        press(&mut app);
        let said = app.notice().unwrap_or_default();
        assert!(
            said.contains("nothing is selected"),
            "`{key}` must say why it did nothing, got {said:?}"
        );
        assert_eq!(
            app.notice_kind(),
            Some(crate::app::NoticeKind::Refusal),
            "…and it is a refusal, not a report — nothing happened"
        );
    }
}

/// **A marked row means exactly the turns drawn under it.**
///
/// Two functions decided "what is under this row" and they differed by one
/// term: the tree filtered `shows_full || owner == id`, the mark expansion
/// filtered `owner == id`. They agree for roots and branches. A **clone** or a
/// **merge** shows a full history it does not own — so marking its row and
/// pressing `s` refused with "owns no turns" about a row with turns visibly
/// drawn beneath it (Reviewer D), while `marked_turn_range`'s own comment
/// claimed both used the same function.
///
/// Safe because ownership is a concern for `d`, which destroys. Squash and
/// merge compose a NEW session and leave the originals untouched, so the row
/// can mean what it shows (Steven's call).
#[test]
fn a_clones_marked_row_expands_to_the_turns_it_draws() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    let source = app.conversation().episode().id;

    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(source);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.dispatch_command_for_test("clone");
    let clone = app.conversation().episode().id;
    assert_ne!(clone, source, "premise: a clone switched in");

    let graph = app.current_graph();
    let drawn = crate::graph_view::episode_children(&graph, clone);
    assert!(
        !drawn.is_empty(),
        "premise: the tree DRAWS turns under the clone's row"
    );
    assert!(
        drawn.iter().all(|d| d.shared),
        "premise: and it owns none of them — the case that used to refuse"
    );

    let marked: Vec<Node> = vec![Node::Episode(clone)];
    let (episode, start, end) = crate::app::session_ops::marked_turn_range(&marked, &graph)
        .expect("the row expands to what it shows");
    assert_eq!(episode, clone);
    assert_eq!(
        end - start,
        drawn.len(),
        "and to exactly as many turns as are on screen"
    );
}

#[test]
fn graph_view_renders_the_conversation_as_a_tree() {
    // A committed turn gives the live episode content to project.
    let mut app = test_app();
    app.conversation_mut().begin_turn("what is a monad");
    app.conversation_mut()
        .push_chunk("A monoid in the category of endofunctors.");
    app.conversation_mut().commit_turn();

    // Open the viewer over the current snapshot (what `:graph` does).
    open_graph(&mut app);

    // Wide enough that the tree | inspector split leaves room for both.
    let screen = text_of(&mut app, 80, 24);

    assert!(screen.contains("graph"), "pane title missing:\n{screen}");
    // The root episode and the turn (recognized by its first words) show.
    assert!(screen.contains("root"), "episode origin missing:\n{screen}");
    assert!(
        screen.contains("what is a monad"),
        "turn snippet missing:\n{screen}"
    );
    // The inspector pane shows the selected node's internals. The default
    // selection is the session's LAST TURN — where you are in the conversation
    // is the last thing in it, so that is what `:graph` opens onto rather than
    // making you press `G` every time (Steven, dogfooding round 1).
    assert!(
        screen.contains("Turn"),
        "inspector heading missing:\n{screen}"
    );
}

#[test]
fn graph_viewer_navigates_and_closes() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    app.conversation_mut().begin_turn("a question");
    app.conversation_mut().push_chunk("an answer");
    app.conversation_mut().commit_turn();
    open_graph(&mut app); // episode + 1 turn = 2 rows
    assert_eq!(
        app.graph_view().unwrap().selected(),
        1,
        "opens on the session's last turn, not the top of the tree"
    );

    app.graph_key_for_test(k(KeyCode::Char('k')));
    assert_eq!(
        app.graph_view().unwrap().selected(),
        0,
        "k moves the selection up"
    );

    app.dispatch_command_for_test("q");
    assert!(app.graph_view().is_none(), "`:q` closes the viewer");
}

/// **An armed prefix owns the next key: it completes the chord or dies with it.**
///
/// Two bugs in one shape. The prefix was a `bool` that eight motion handlers
/// cleared for themselves, which left every *other* way through the dispatcher
/// free to forget — and they did: `<Space>`, `Enter`, `:`, and all six
/// composing keys returned without touching it. A `g` typed before any of them
/// stayed armed, so a later `g` completed a chord abandoned several keystrokes
/// earlier and the tree jumped to the top for no visible reason.
///
/// And the key that ended the chord then *also* ran on its own. That is not
/// vim, and with `mf` in the vocabulary it is dangerous: `f` and `d` are
/// neighbours, so `md` — one slipped finger — reached the delete confirmation.
/// A dead chord now spends both halves and says so.
#[test]
fn an_armed_graph_prefix_owns_the_next_key() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    for q in ["first question", "second question"] {
        app.conversation_mut().begin_turn(q);
        app.conversation_mut().push_chunk("an answer");
        app.conversation_mut().commit_turn();
    }
    open_graph(&mut app); // session + 2 turns = 3 rows, opening on the last
    assert_eq!(
        app.graph_view().unwrap().selected(),
        2,
        "three rows, selection on the last"
    );

    // `<Space>` alone marks — the netrw-free shorthand for `mf`.
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    assert_eq!(
        app.graph_view().unwrap().marked_nodes().len(),
        1,
        "premise: `<Space>` marks when no chord is armed"
    );
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    assert_eq!(
        app.graph_view().unwrap().marked_nodes().len(),
        0,
        "premise: and toggles back off"
    );

    // **The dead chord.** `g<Space>` is not a command, so the `<Space>` is
    // spent on the failed `g` — it must NOT also mark, though `<Space>` is a
    // graph verb: the `g` is the keymap's now, and a keymap prefix owns the
    // next key before any pane's verb can claim it (`prefix_in_flight`). Dead
    // NAVIGATION chords are silent in every pane, as in Neovim
    // (`belloff=all`); the graph's own `m` chord still says why, below.
    app.graph_key_for_test(k(KeyCode::Char('g')));
    app.graph_key_for_test(k(KeyCode::Char(' ')));
    assert_eq!(
        app.graph_view().unwrap().marked_nodes().len(),
        0,
        "the key that killed the chord must not also run on its own"
    );
    // The chord is gone, so a later `g` starts fresh rather than completing it.
    app.graph_key_for_test(k(KeyCode::Char('g')));
    assert_eq!(
        app.graph_view().unwrap().selected(),
        2,
        "`g<Space>g` must not jump to the top"
    );
    // ...and that fresh prefix completes normally.
    app.graph_key_for_test(k(KeyCode::Char('g')));
    assert_eq!(
        app.graph_view().unwrap().selected(),
        0,
        "`gg` still jumps to the top"
    );

    // **`md` is a typo for `mf`, not a delete.** The whole reason a dead chord
    // swallows: `d` sits next to `f`, and reaching the delete confirmation
    // from a slipped finger is the failure this prevents.
    app.graph_key_for_test(k(KeyCode::Char('m')));
    app.graph_key_for_test(k(KeyCode::Char('d')));
    assert!(
        app.pending_confirm.is_empty(),
        "`md` must not reach the delete confirmation"
    );
    assert_eq!(
        app.graph_view().unwrap().marked_nodes().len(),
        0,
        "…and marks nothing either"
    );
    assert_eq!(
        app.notice_kind(),
        Some(crate::app::NoticeKind::Refusal),
        "…and a swallowed key says why, or it reads as a broken key"
    );

    // `me` is dead too — `e` used to be matched before the chord, so it
    // aborted the `m` and opened `$EDITOR` (the one exception). The seam
    // panics if `graph_key_opens_editor` — the predicate `handle_graph_key`
    // branches on — says `$EDITOR`, so reaching the chord's refusal is the pin.
    app.graph_key_for_test(k(KeyCode::Char('m')));
    app.graph_key_for_test(k(KeyCode::Char('e')));
    assert_eq!(
        app.notice_kind(),
        Some(crate::app::NoticeKind::Refusal),
        "`me` is a dead chord, not an edit"
    );

    // `<Esc>` cancels a half-typed command in vim and means nothing else. It
    // does not close the pane either — a viewer is a window, closed with `:q`
    // — so it is inert both with a chord armed and without one.
    app.graph_key_for_test(k(KeyCode::Char('m')));
    app.graph_key_for_test(k(KeyCode::Esc));
    assert!(
        app.graph_view().is_some(),
        "`<Esc>` on an armed prefix cancels the chord, not the pane"
    );
    app.graph_key_for_test(k(KeyCode::Esc));
    assert!(
        app.graph_view().is_some(),
        "and a bare `<Esc>` does not close a window either"
    );
    // The chord really was cancelled, not left armed: `f` alone marks nothing.
    app.graph_key_for_test(k(KeyCode::Char('f')));
    assert_eq!(
        app.graph_view().unwrap().marked_nodes().len(),
        0,
        "`<Esc>` disarmed the prefix, so a later `f` is not `mf`"
    );
}

/// **A verb that could not act refuses; it does not report.**
///
/// `App::refuse`'s own doc says this rule "had been applied to the instance
/// that was reported rather than to the class" — and then four composing paths
/// announced their failures with `notify`, which renders as a REPORT: same row,
/// styled as though something had happened. Merging a root session (there is no
/// base to merge into) said `merge: …` in the voice of success. The four were
/// merge, squash, and both halves of edit; this pins the class through the one
/// that has a terminal-free path.
#[test]
fn a_composing_verb_that_cannot_act_refuses_rather_than_reports() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("a question");
    app.conversation_mut().push_chunk("an answer");
    app.conversation_mut().commit_turn();
    // Premise: a root session, so there is genuinely nothing to merge into —
    // otherwise this measures a success ([[feedback_assert_the_subject]]).
    assert!(
        app.conversation().episode().origin.is_none(),
        "premise: the session must be a root, not a branch"
    );
    app.notices.clear();

    app.dispatch_command_for_test("merge");

    let said = app.notice().unwrap_or_default();
    assert!(said.contains("merge:"), "it names what failed: {said:?}");
    assert_eq!(
        app.notice_kind(),
        Some(crate::app::NoticeKind::Refusal),
        "…and it reads as a refusal, not as a report that it worked"
    );
}

#[test]
fn graph_viewer_drills_into_the_hierarchy() {
    use ratatui::{Terminal, backend::TestBackend};
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    set_graph_style(&mut app, shannon_core::GraphStyle::Tree); // tree-style rows
    app.conversation_mut().begin_turn("explain monads");
    app.conversation_mut().push_chunk("forty two");
    app.conversation_mut().commit_turn();
    open_graph(&mut app);

    // Collapsed by default: the turn shows, but its messages do not.
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    draw_frame(&mut app, &mut terminal);
    let before = rendered_text(terminal.backend().buffer());
    assert!(before.contains("explain monads"), "turn row:\n{before}");
    assert!(
        !before.contains("forty two"),
        "messages hidden until expanded:\n{before}"
    );

    // Select the turn (j) and expand it (l) → its messages appear.
    app.graph_key_for_test(k(KeyCode::Char('j')));
    app.graph_key_for_test(k(KeyCode::Char('l')));
    draw_frame(&mut app, &mut terminal);
    let after = rendered_text(terminal.backend().buffer());
    assert!(
        after.contains("reply: \"forty two\""),
        "message row appears, saying who spoke:\n{after}"
    );
}

/// **A glyph that is not a mark is said where it is set** (review stop 2,
/// B 5): `:set tui.graph_glyphs.child=🙂` echoed the value and the graph kept
/// the default, with the reason in the log alone. And one that is a mark says
/// nothing of the kind.
#[test]
fn a_glyph_that_is_not_a_mark_is_said_where_it_is_set() {
    let mut app = test_app();
    app.notices.clear();
    app.dispatch_command_for_test("set tui.graph_glyphs.child=🙂");
    let said = app
        .notices
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(said.contains("child = \"🙂\"` takes 2 cells"), "{said}");
    assert_eq!(
        app.glyph_notes().len(),
        1,
        "and at launch: {:?}",
        app.glyph_notes()
    );

    app.notices.clear();
    app.dispatch_command_for_test("set tui.graph_glyphs.child=◇");
    let said = app
        .notices
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(!said.contains("cells"), "{said}");
    assert!(app.glyph_notes().is_empty());
}

#[test]
fn sibling_root_sessions_stay_column_aligned_with_the_active_marker() {
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    set_graph_style(&mut app, shannon_core::GraphStyle::Tree); // ● is episode-only in tree
    // Session one (persisted so it survives the new session) ...
    app.conversation_mut().begin_turn("hi");
    app.conversation_mut().push_chunk("there");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    // ... then a fresh, empty session two — a SECOND root, marked active.
    app.reset_foreground();
    let mut gv = GraphView::new(app.current_graph());
    gv.set_active(Some(app.conversation().episode().id));
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);

    let screen = text_of(&mut app, 80, 24);

    // Both root episodes draw their ● at the same column: the active ▶ lives
    // in the gutter, so the live session is a sibling root, not nested.
    let cols: Vec<usize> = screen
        .lines()
        .filter_map(|l| l.chars().position(|c| c == '●'))
        .collect();
    assert_eq!(cols.len(), 2, "two sibling root episodes:\n{screen}");
    assert_eq!(
        cols[0], cols[1],
        "roots align despite the ▶ marker:\n{screen}"
    );
}

#[test]
fn enter_in_graph_loads_the_selected_session() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let mut app = test_app_with_store(
        test_backend(),
        Arc::new(Config::default()),
        Box::new(JsonlStore::new(dir.path()).unwrap()),
    );
    // Session one: persisted, then archived behind a fresh session two.
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("reply one");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    let first_id = app.conversation().episode().id;
    app.reset_foreground();
    assert_ne!(
        app.conversation().episode().id,
        first_id,
        "now on session two"
    );

    // Open :graph, select a node of the first session, press Enter.
    let mut gv = GraphView::new(app.current_graph());
    gv.set_active(Some(app.conversation().episode().id));
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    for _ in 0..6 {
        if app.graph_view().unwrap().selected_episode() == Some(first_id) {
            break;
        }
        app.graph_key_for_test(k(KeyCode::Char('j')));
    }
    assert_eq!(
        app.graph_view().unwrap().selected_episode(),
        Some(first_id),
        "navigated to the first session"
    );

    app.graph_key_for_test(k(KeyCode::Enter));
    assert!(app.graph_view().is_none(), "the viewer closes on open");
    assert_eq!(
        app.conversation().episode().id,
        first_id,
        "the first session is now active"
    );
    assert!(
        app.output().lines().iter().any(|l| l.contains("reply one")),
        "its transcript was repopulated"
    );
    let store = JsonlStore::new(dir.path()).unwrap();
    assert_eq!(
        store.get_ref("HEAD").unwrap(),
        Some(first_id),
        "HEAD follows the loaded session"
    );
}

#[test]
fn colon_in_the_graph_viewer_opens_the_command_line() {
    // Regression: while :graph was open, keys went to the viewer, so `:` was
    // eaten and you couldn't run :new/:q from it.
    //
    // It used to have to CLOSE the viewer first, because a viewer covered
    // the frame and a command run "underneath" it would have been invisible.
    // As a pane it just stays open — which is strictly better, since `:open
    // <hash>` and friends are exactly the commands you want to run *while
    // looking at the graph*.
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    open_graph(&mut app);
    app.graph_key_for_test(k(KeyCode::Char(':')));
    assert!(
        app.graph_view().is_some(),
        "the viewer stays open — it is a pane, not a takeover"
    );
    assert_eq!(app.cmdline.line(), Some(""), "the command line is open");
}

#[test]
fn git_style_decorates_head_root_and_oneline() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2); // Hello 0, Hello 1
    set_graph_style(&mut app, shannon_core::GraphStyle::Git);
    let active = app.conversation().episode().id;
    let mut gv = GraphView::with_sort(app.current_graph(), shannon_core::GraphSort::Recency);
    gv.set_active(Some(active));
    gv.reveal_and_select_episode(active);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    let text = text_of(&mut app, 82, 24);
    assert!(text.contains("● "), "git style draws commit dots");
    assert!(
        text.contains("(HEAD"),
        "the active session is decorated (HEAD …)"
    );
    assert!(
        text.contains("root)"),
        "a root episode's origin shows as root"
    );
    assert!(
        text.contains("Hello 0"),
        "turns carry a one-line content summary"
    );
}

#[test]
fn git_style_ascii_glyphs_fall_back_to_plain_characters() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let mut cfg = (*app.config).clone();
    cfg.tui.graph_style = shannon_core::GraphStyle::Git;
    cfg.tui.graph_chars = shannon_core::GraphChars::Ascii;
    app.config = std::sync::Arc::new(cfg);
    let active = app.conversation().episode().id;
    let mut gv = GraphView::with_sort(app.current_graph(), shannon_core::GraphSort::Recency);
    gv.set_active(Some(active));
    gv.reveal_and_select_episode(active);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    let text = text_of(&mut app, 82, 24);
    assert!(text.contains("* "), "ascii draws commit dots as *");
    // No Unicode lane glyphs leak through (pane borders still use box-drawing,
    // so we check the graph glyphs specifically, not the whole screen).
    assert!(!text.contains('●'), "no Unicode commit dots in ascii mode");
    assert!(!text.contains('╰'), "no Unicode corners in ascii mode");
}

#[test]
fn graph_draws_tree_connectors() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_two_turns(dir.path(), Config::default());
    set_graph_style(&mut app, shannon_core::GraphStyle::Tree); // ▹ leaf markers
    let active = app.conversation().episode().id;
    let mut gv = GraphView::new(app.current_graph());
    gv.set_active(Some(active));
    // Expand the first turn so a message subtree shows connectors too.
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    app.graph_key_for_test(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

    let screen = text_of(&mut app, 70, 24);
    assert!(
        screen.contains('├') && screen.contains('└') && screen.contains('│'),
        "tree connectors are drawn:\n{screen}"
    );
    assert!(
        screen.contains('▹'),
        "leaf rows are marked with ▹:\n{screen}"
    );
}
