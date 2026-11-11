//! Where a viewer opens, its companions, and what it declares.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// --- where a viewer opens -------------------------------------------------
//
// Dogfooding, 2026-07-27: with six panes open, `:graph` from a small corner
// pane put the graph across the screen in a pane the user was not looking at.
// The rule was "split whichever pane can afford the room", resolved as the
// FIRST slot in creation order — invisible state, so the result could not be
// predicted from anything on screen. Steven: "we need something predictable
// that makes sense", and then named it: open it in the pane I ran it from.

/// A viewer takes over the pane you ran the command from — whichever of the
/// many panes that is, and without disturbing any of the others.
#[test]
fn a_viewer_opens_in_the_pane_you_ran_it_from() {
    let frame = Rect::new(0, 0, 120, 40);
    let mut app = test_app();
    // Build a few panes so "the focused one" and "the first one" differ.
    app.split_command("output", SplitDir::Vertical);
    app.split_command("output", SplitDir::Horizontal);
    let target = app.workspace.focused();
    let before: Vec<_> = app
        .workspace
        .frame_layout(frame)
        .panes
        .iter()
        .map(|p| (p.id, p.rect))
        .collect();
    assert!(before.len() >= 4, "several panes, not the launch pair");

    app.open_graph_command();

    let layout = app.workspace.frame_layout(frame);
    assert_eq!(
        layout.pane(target).map(|p| p.kind),
        Some(PaneKind::Graph),
        "the graph took over the pane the command was run from"
    );
    // Every other pane keeps its geometry: showing a different thing in one
    // viewport is not a rearrangement of the workspace.
    for (id, rect) in before {
        if id == target {
            continue;
        }
        // The inspector splits the target, so only panes outside it are pinned.
        if let Some(now) = layout.pane(id) {
            assert_eq!(now.rect, rect, "pane {id:?} moved when it should not have");
        }
    }
}

/// Closing the viewer gives the pane back to what it was showing, rather than
/// destroying the viewport. A borrowed pane is returned, not consumed.
#[test]
fn closing_a_viewer_restores_what_the_pane_was_showing() {
    let mut app = test_app();
    app.split_command("output", SplitDir::Vertical);
    let target = app.workspace.focused();
    let panes_before = app.workspace.slots().len();

    app.open_graph_command();
    assert_eq!(app.workspace.kind_of(target), Some(PaneKind::Graph));

    app.close_pane_command(Asked::Plain);
    assert_eq!(
        app.workspace.kind_of(target),
        Some(PaneKind::Output),
        "the pane went back to the transcript"
    );
    assert_eq!(
        app.workspace.slots().len(),
        panes_before,
        "and the viewport itself survived — closing a view is not closing a pane"
    );
}

/// **`:q` steps back through what a pane showed, each buffer once** —
/// `:help` then `:messages` then `:q` gives the manual back, and the next
/// `:q` the transcript the chain started from. It went straight to the
/// transcript while a view replaced was retired; set aside now, the view you
/// came from is one `:q` away (Steven, 10-16 — the tree's case,
/// `help_typed_in_the_tree_takes_its_pane_and_q_gives_the_tree_back`). And
/// opening a buffer again supersedes the place underneath, so going back
/// and forth never builds a stack longer than the buffers it showed.
#[test]
fn q_steps_back_through_what_a_pane_showed_each_buffer_once() {
    let mut app = test_app();
    app.split_command("output", SplitDir::Vertical);
    let target = app.workspace.focused();

    for _ in 0..2 {
        app.open_help_command(None);
        app.dispatch_command_for_test("messages");
    }
    assert_eq!(
        app.workspace.kind_of(target),
        Some(PaneKind::Page(PageKind::Messages))
    );

    app.close_pane_command(Asked::Plain);
    assert_eq!(
        app.workspace.kind_of(target),
        Some(PaneKind::Page(PageKind::Help)),
        "the manual it replaced"
    );
    app.close_pane_command(Asked::Plain);
    assert_eq!(
        app.workspace.kind_of(target),
        Some(PaneKind::Output),
        "then where the chain started — each buffer once"
    );
}

/// **The same buffer again replaces what is shown, never sets it aside** —
/// `:help`, then `:help windows` in the same pane: the manual at the topic
/// takes the window's place, and `:q` gives back what lay beneath the
/// manual, not an older window on it (V5.3 (3d): `App::show_in` with the
/// buffer shown puts the view in its place, `Window::shows_as`; a set-aside
/// there would put the buffer shown under itself, which the history's rule
/// forbids). By kind until V5.4 (2) — the same for a page, one per kind;
/// another TREE is set aside (`b_n_of_a_second_tree_keeps_the_first_as_the_alternate`).
#[test]
fn the_same_buffer_again_replaces_what_is_shown() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let here = app.workspace.focused();
    app.open_help_command(None);
    app.open_help_command(Some("windows"));
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Page(PageKind::Help)),
        "premise"
    );
    assert!(
        !history_of(&app, here)
            .iter()
            .any(|entry| entry == "view help"),
        "the manual shown is not under itself: {:?}",
        history_of(&app, here)
    );
    app.close_pane_command(Asked::Plain);
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Output),
        "one `:q` back to what lay beneath"
    );
}

/// **A view typed in the inspector shows over the tree and it, and `:q`
/// gives both back** (Steven, 10-26: "(a) Open it in the companion's host
/// window instead, so :q brings both back… treat `:themes` and the like as
/// 'full screen'. So they can sit on top of both"). The manual opens in the
/// tree's window, the tree set aside there, and the inspector leaves with it,
/// its space the manual's; `:q` gives the tree back with an inspector beside
/// it. It took the inspector's own pane with nothing behind it, so `:q`
/// closed that and the tree was left with none — while the inspector's
/// status row offers `:help graph-inspector` (review stop 10, F 1, probed).
/// Before that, set aside like any view, it came back beside a tree that had
/// brought its own: two inspectors on one tree (review stop 3, A F4).
#[test]
fn a_view_typed_in_the_inspector_shows_over_both_and_q_gives_both_back() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let tree = app.workspace.focused();
    app.open_graph_command();
    app.settle();
    let inspectors = |app: &App| app.workspace.panes_of(PaneKind::Inspector).count();
    let inspector = (app.workspace.panes_of(PaneKind::Inspector).next()).expect("premise: one");
    let body = ratatui::layout::Rect::new(0, 0, 120, 40);
    let rect = |app: &App, pane| app.workspace.frame_layout(body).pane(pane).map(|p| p.rect);
    let both = rect(&app, tree)
        .zip(rect(&app, inspector))
        .map(|(t, i)| t.union(i));

    app.focus_pane(inspector);
    app.dispatch_command_for_test("help graph-inspector");
    assert_eq!(
        app.workspace.kind_of(tree),
        Some(PaneKind::Page(PageKind::Help)),
        "in the tree's window"
    );
    assert_eq!(app.workspace.focused(), tree, "focused there");
    assert_eq!(inspectors(&app), 0, "the inspector left with the tree");
    assert_eq!(rect(&app, tree), both, "over both");
    assert!(
        history_of(&app, tree).contains(&"view graph".to_string()),
        "the tree set aside: {:?}",
        history_of(&app, tree)
    );

    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(tree),
        Some(PaneKind::Graph),
        "`:q`: the tree"
    );
    assert_eq!(inspectors(&app), 1, "and one inspector beside it");
}

/// **A session asked for in the inspector shows in the tree's window too,
/// and you are there** — `<C-^>` from it brings the tree back with its
/// inspector; `:b N` asks the pane a view would use (`existing_host`). It
/// turned the inspector into a transcript with nothing behind it, so `:b#`
/// had nothing to go back to (review stop 10, F 1). Then its close left
/// focus on the first pane — here the left transcript, not the tree's
/// window — and `<C-^>` went nowhere (review of the 10-26 answers, H1: in
/// the default layout the first pane happened to be the tree's).
#[test]
fn a_session_asked_for_in_the_inspector_shows_in_the_trees_window() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let tree = app.workspace.focused();
    let session = app.prompt_target();
    app.split_command("", SplitDir::Vertical);
    let left = app.workspace.focused();
    assert_ne!(left, tree, "premise: a window to the left");
    app.focus_pane(tree);
    app.open_graph_command();
    app.settle();
    assert_eq!(
        app.workspace.kind_of(tree),
        Some(PaneKind::Graph),
        "premise: the tree, right"
    );
    let inspector = (app.workspace.panes_of(PaneKind::Inspector).next()).expect("premise: one");
    app.focus_pane(inspector);
    app.dispatch_command_for_test(&format!("b{}", session.number()));
    assert_eq!(
        app.workspace.kind_of(tree),
        Some(PaneKind::Output),
        "in the tree's window"
    );
    assert_eq!(
        app.workspace.focused(),
        tree,
        "and focused there, not the left"
    );
    assert_eq!(
        app.workspace.panes_of(PaneKind::Inspector).count(),
        0,
        "premise: gone with it"
    );
    app.dispatch_command_for_test("b#");
    assert_eq!(
        app.workspace.kind_of(tree),
        Some(PaneKind::Graph),
        "`<C-^>`: the tree"
    );
    assert_eq!(
        app.workspace.panes_of(PaneKind::Inspector).count(),
        1,
        "with its inspector"
    );
}

/// **A composition typed in the inspector leaves the tree up, as typed in
/// the tree** — the tree is a workbench you stay in, and a companion is
/// its host's window (`show_the_result`). From the inspector, `:clone` put
/// the clone in the tree's window and closed the inspector (review of the
/// 10-26 answers, M2).
#[test]
fn a_composition_typed_in_the_inspector_leaves_the_tree_up() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    app.set_focus(PaneKind::Output);
    let tree = app.workspace.focused();
    app.open_graph_command();
    app.settle();
    let inspector = (app.workspace.panes_of(PaneKind::Inspector).next()).expect("premise: one");
    app.focus_pane(inspector);
    let before = app.buffers.sessions().count();
    app.dispatch_command_for_test("clone");
    assert_eq!(
        app.buffers.sessions().count(),
        before + 1,
        "premise: cloned"
    );
    assert_eq!(
        app.workspace.kind_of(tree),
        Some(PaneKind::Graph),
        "the tree stays up"
    );
    assert_eq!(
        app.workspace.panes_of(PaneKind::Inspector).count(),
        1,
        "its inspector with it"
    );
}

/// **`:graph` in a tree's pane keeps its inspector** — the same buffer
/// listed again is put in place (`show_in`'s `shows_as`), its companion
/// where you put it; released there, it came back at its first width and
/// the top of its page on every re-list (review of the 10-26 answers, M5:
/// what `companions_follow` was written for, then held by no test).
#[test]
fn graph_in_a_trees_pane_keeps_its_inspector() {
    let mut app = test_app();
    open_graph_pane(&mut app);
    let inspector = (app.workspace.panes_of(PaneKind::Inspector).next()).expect("premise: one");
    app.dispatch_command_for_test("graph");
    let after: Vec<_> = app.workspace.panes_of(PaneKind::Inspector).collect();
    assert_eq!(after, [inspector], "the same inspector pane");
    app.focus_pane(inspector);
    app.dispatch_command_for_test("graph");
    let after: Vec<_> = app.workspace.panes_of(PaneKind::Inspector).collect();
    assert_eq!(after, [inspector], "typed in the inspector too");
}

/// **`<C-^>` in a companion with nothing to go back to names the window it
/// asked** — its host's, not "the aimed window" (the prompt bar's words).
#[test]
fn ctrl_hat_in_a_companion_with_no_alternate_names_its_hosts_window() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    // A tree in a pane of its own, with nothing behind it.
    app.dispatch_command_for_test("sgraph");
    let tree = app.workspace.focused();
    assert_eq!(
        app.workspace.kind_of(tree),
        Some(PaneKind::Graph),
        "premise: a tree"
    );
    let n = app.pane_number(tree).expect("premise: numbered");
    let inspector = (app.workspace.panes_of(PaneKind::Inspector).next()).expect("premise: one");
    app.focus_pane(inspector);
    app.dispatch_command_for_test("b#");
    assert_eq!(
        app.notice().as_deref(),
        Some(
            format!("no buffer to go back to in pane {n}, the window this one describes").as_str()
        ),
    );
}

/// **`:messages` typed in the tree takes the tree's pane** (Steven, 10-16,
/// images 48–51): it split the frame across the top beside the tree, where
/// from the transcript it replaced the transcript. Nothing opens; the tree's
/// inspector goes with it and comes back with it.
#[test]
fn messages_typed_in_the_tree_takes_its_pane() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let here = app.workspace.focused();
    app.open_graph_command();
    let panes = app.workspace.pane_count();
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Graph),
        "premise"
    );
    assert!(
        app.workspace.has(PaneKind::Inspector),
        "premise: its inspector"
    );

    app.dispatch_command_for_test("messages");
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Page(PageKind::Messages))
    );
    assert_eq!(app.workspace.focused(), here, "the pane you were in");
    assert_eq!(
        app.workspace.pane_count(),
        panes - 1,
        "no split; the inspector went with the tree"
    );

    app.dispatch_command_for_test("q");
    assert_eq!(app.workspace.kind_of(here), Some(PaneKind::Graph));
    assert!(
        app.workspace.has(PaneKind::Inspector),
        "and came back with it"
    );
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Output),
        "the transcript under both"
    );
}

/// **A tree given back is current** — set aside, it is not rebuilt as the
/// open ones are while turns land; given back, it is, so a session that
/// arrived meanwhile is in it.
#[test]
fn a_tree_given_back_holds_what_landed_meanwhile() {
    use shannon_core::graph::Node;
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    app.dispatch_command_for_test("messages");
    let c = add_persisted_session(&mut app, "C0");
    assert_eq!(
        app.focus(),
        PaneKind::Page(PageKind::Messages),
        "premise: still over the tree"
    );

    app.dispatch_command_for_test("q");
    assert_eq!(app.focus(), PaneKind::Graph, "premise: the tree back");
    let view = app.graph_view_mut().expect("the tree");
    view.reveal_and_select_episode(c);
    assert_eq!(
        view.selected_node(),
        Some(Node::Episode(c)),
        "with the session that landed while it was aside"
    );
}

/// **A session opened over set-aside views keeps them, and its `:q` closes**
/// — `:new` in a pane showing `:messages` over the manual: the session is
/// what the pane shows now, the views kept in its history for `<C-^>` (V4b
/// Q1; they were dropped), and `:q` there closes the pane rather than giving
/// back a manual from before it (Q3, `gives_back`).
#[test]
fn a_session_over_set_aside_views_keeps_them_and_q_closes() {
    let mut app = test_app();
    app.split_command("output", SplitDir::Vertical);
    let target = app.workspace.focused();
    app.open_help_command(None);
    app.dispatch_command_for_test("messages");
    assert!(app.gives_back(target), "premise: something under it");

    app.new_session_command();
    assert_eq!(app.workspace.kind_of(target), Some(PaneKind::Output));
    let kept = history_of(&app, target);
    assert!(
        ["view help", "view messages"]
            .iter()
            .all(|v| kept.iter().any(|k| k == v)),
        "the views kept: {kept:?}"
    );
    assert!(
        !app.gives_back(target),
        "nothing given back by a session opened there"
    );
}

/// A viewer opens **in the focused pane**, and `Esc` gives the pane back.
#[test]
fn a_viewer_takes_the_focused_pane_and_gives_it_back() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let here = app.workspace.focused();

    app.open_graph_command();
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Graph),
        "the pane the command was run from shows the graph"
    );

    app.close_pane_command(Asked::Plain);
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Output),
        "and Esc/:q hands the transcript back"
    );
}

/// `:new` from inside a viewer puts the fresh session **where you are
/// standing**, like every other command that answers "what does this pane
/// show?".
///
/// Reported by dogfooding: `:new` typed in a `:graph` pane did nothing you
/// could see. It reset a buffer *behind* a pane that went on showing the tree,
/// because it was the one command in the family that never asked which pane it
/// was run from.
#[test]
fn new_from_inside_a_viewer_shows_the_fresh_session_in_that_pane() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let before = app.conversation().episode().id;
    open_graph_pane(&mut app);
    let here = app.workspace.focused();
    assert_eq!(app.workspace.kind_of(here), Some(PaneKind::Graph));

    app.new_session_command();

    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Output),
        "the pane you ran it from shows the new session's transcript"
    );
    assert_ne!(
        app.conversation().episode().id,
        before,
        "and it is a different session from the one you were on"
    );
    assert!(
        app.conversation().episode().own_is_empty(),
        "a fresh one, with nothing in it"
    );
}

/// A composing op run from a viewer that CANNOT show the result hands its pane
/// back — the other half of the workbench rule.
///
/// `:graph` earns staying open because it rebuilds with the composed session in
/// it. `:help` shows the manual, which has nothing to do with what you just
/// did, so a `:clone` typed there would otherwise be invisible — the `:new` bug
/// in a second costume.
#[test]
fn composing_from_a_viewer_that_cannot_show_the_result_gives_the_pane_back() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let original = app.conversation().episode().id;
    app.set_focus(PaneKind::Output);
    let here = app.workspace.focused();
    app.open_help_command(None);
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Page(PageKind::Help))
    );

    app.clone_session();

    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Output),
        "the manual gave the pane back so the clone is visible"
    );
    assert_ne!(
        app.conversation().episode().id,
        original,
        "and we really are on the clone"
    );
}

/// …unless the focused pane **cannot host one**, in which case the view opens
/// across the top of the frame and the pane you were in is left alone.
///
/// The prompt bar is six rows. Showing the manual in it left about three lines
/// of text and no way to type — a trap Steven walked into and could only leave
/// through `:q`. This is not a rule about the prompt: it is a rule about panes
/// whose size is a declaration rather than a share, which is also the decision
/// pane, and it puts the view where vim puts `:help`.
#[test]
fn a_viewer_asked_for_from_a_bar_opens_across_the_top() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    let prompt = app.workspace.focused();
    let before = app.workspace.pane_count();

    app.open_graph_command();

    assert_eq!(
        app.workspace.kind_of(prompt),
        Some(PaneKind::Prompt),
        "the prompt is still a prompt — you can still type"
    );
    let landed = app.workspace.focused();
    assert_ne!(landed, prompt, "the view took a pane of its own");
    assert_eq!(app.workspace.kind_of(landed), Some(PaneKind::Graph));
    assert!(app.workspace.pane_count() > before, "a pane was added");
    assert_eq!(
        app.workspace.ring_order().first(),
        Some(&landed),
        "across the TOP — first in ring order, whatever the arrangement was"
    );

    // It was made for the view, so closing it closes the pane rather than
    // leaving an empty one.
    app.close_pane_command(Asked::Plain);
    assert_eq!(app.workspace.pane_count(), before, "and it goes away again");
}

/// `:help` is a pane now, not an `$EDITOR` shell-out: it opens, it scrolls,
/// and `q` puts the transcript back.
#[test]
fn help_opens_as_a_pane_and_scrolls() {
    let mut app = test_app();
    app.split_command("output", SplitDir::Vertical);
    let target = app.workspace.focused();

    app.open_help_command(None);
    assert_eq!(
        app.workspace.kind_of(target),
        Some(PaneKind::Page(PageKind::Help))
    );
    assert!(app.page_view().is_some(), "with the manual built");

    let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    // **`j` moves the CURSOR, and the window follows only when it has to.**
    // This used to assert `scroll() > 0` after one `j` — true when `j` moved
    // the view directly, and wrong: in vim the cursor travels through the
    // window and the page stays still until the cursor presses into the
    // scrolloff margin. A page that jumps on the first keypress is the
    // "the view is dragging you" complaint, one pane over (docket 67).
    app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('j'), (60, 10));
    assert_eq!(
        app.page_view().unwrap().text().cursor(),
        1,
        "j moves the cursor"
    );
    assert_eq!(
        app.page_view().unwrap().text().scroll(),
        0,
        "and the page has not moved — there is a whole window below"
    );
    // Held down, it eventually pushes the window.
    for _ in 0..40 {
        app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('j'), (60, 10));
    }
    assert!(
        app.page_view().unwrap().text().scroll() > 0,
        "the cursor reached the margin and pushed the page"
    );
    // `gg`, as in vim — a single `g` is a PREFIX now, not a command. The
    // pane's hand-written match used to treat one `g` as go-to-top.
    app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('g'), (60, 10));
    app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('g'), (60, 10));
    assert_eq!(app.page_view().unwrap().text().scroll(), 0, "gg is top");
    assert_eq!(
        app.page_view().unwrap().text().cursor(),
        0,
        "and so is the cursor"
    );

    // `G` must reach the real end. The manual is written to 78 columns, so in a
    // 60-column pane it wraps — a scroll clamped to the LINE count would strand
    // the tail here, which is the bug this pane's scrolling was built to avoid.
    app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('G'), (60, 10));
    let narrow = app.page_view().unwrap().text().scroll();
    app.handle_text_pane_key_for_test(PaneKind::Page(PageKind::Help), k('G'), (200, 10));
    let wide = app.page_view().unwrap().text().scroll();
    assert!(
        narrow > wide,
        "a narrower pane wraps more, so the end is further down: {narrow} vs {wide}"
    );

    // `:q` gives the lent viewport back. `<Esc>` used to do this; it is an
    // ordinary key now, and asserted so first — the manual's promise that
    // closing RESTORES what was there is the part that must survive.
    app.handle_text_pane_key_for_test(
        PaneKind::Page(PageKind::Help),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        (60, 10),
    );
    assert_eq!(
        app.workspace.kind_of(target),
        Some(PaneKind::Page(PageKind::Help)),
        "`<Esc>` must not close a window"
    );
    app.dispatch_command_for_test("q");
    assert_eq!(app.workspace.kind_of(target), Some(PaneKind::Output));
}

/// **Every pane's `help` points at a section the manual has, by the name it
/// has now** (review stop 17, F L3): the `shannon-` prefix went on 11-08 and
/// `:help` still reads the old spelling, so the tutor's status row went on
/// showing one the manual no longer uses, with nothing to say so.
#[test]
fn every_panes_help_topic_is_in_the_manual() {
    for kind in PaneKind::all() {
        let topic = kind.help_topic();
        assert!(
            crate::help_view::opening(Some(topic)).is_ok() && !topic.starts_with("shannon-"),
            "{kind:?}: `:help {topic}`"
        );
    }
}

/// **`:help` takes its whole argument** (review stop 17, F L5): `CTRL-W j` is
/// one key, and its first word alone opened the chord's first row.
#[test]
fn help_takes_a_chord_written_with_a_space() {
    let mut app = test_app();
    app.dispatch_command_for_test("help CTRL-W j");
    let title = app.page_view().expect("it opens").title().to_string();
    assert!(title.contains("CTRL-W j"), "{title:?}");
}

/// `:help <topic>` opens that chapter; an unknown one refuses visibly and
/// opens nothing.
#[test]
fn help_topic_opens_a_chapter_and_an_unknown_one_refuses() {
    let mut app = test_app();
    app.open_help_command(Some("windows"));
    assert!(app.workspace.has(PaneKind::Page(PageKind::Help)));
    assert!(app.page_view().unwrap().title().contains("windows"));

    let mut app = test_app();
    app.open_help_command(Some("nonsense"));
    assert!(
        !app.workspace.has(PaneKind::Page(PageKind::Help)),
        "nothing opened"
    );
    let notice = app.notice().expect("a visible refusal");
    assert!(notice.contains("nonsense"), "naming what failed: {notice}");
}

/// **`<C-w>j` from a viewer pane must move focus on the FIRST press.**
///
/// Steven, dogfooding 2026-07-27: "when pressing `<C-w>j` I had to do it twice
/// to get out. Same with `k`. But interestingly the movement seems to be tied
/// to `<C-w>` rather than with the direction."
///
/// The sequence: `<C-w>` is not plain, so it fell past the viewer arms to the
/// keymap and armed `Pending::Window`. The `j` after it IS plain, so the graph
/// pane claimed it as "move selection down" and the prefix stayed armed. The
/// next `<C-w>` then resolved in the `AfterWindow` context as `<C-w><C-w>` —
/// cycle. Hence two presses, and hence movement in ring order rather than the
/// direction asked for.
///
/// The rule ("a pane claiming plain keys must not eat half a chord") existed;
/// it had been written into the decision pane only, while three other panes
/// repeated the surrounding logic without it.
#[test]
fn a_viewer_does_not_swallow_the_second_key_of_a_window_chord() {
    let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
    let j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);

    // Every viewer, from the type — the graph and the text pages, as
    // `focused_viewer` reads them: this named three of twelve (review stop
    // 12, T3 14).
    let kinds = PaneKind::all().into_iter();
    for kind in kinds.filter(|k| k.is_text_page() || *k == PaneKind::Graph) {
        let mut app = test_app();
        open_pane_of(&mut app, kind);
        assert_eq!(app.focus(), kind, "{kind:?} is focused");

        // With nothing pending, `j` reaches SOMETHING that moves — either the
        // viewer's own handler (the graph, whose rows are its own vocabulary)
        // or the shared keymap (a text page, which navigates like the
        // transcript). Both are fine; what must not happen is the chord case
        // below. Asserted as "one of the two" rather than "the viewer claims
        // it", which was true of all three only while text panes had their own
        // hand-written key match (docket 67).
        assert!(
            app.viewer_claims_key(&j)
                || matches!(app.chat_keymap.translate(j), KeyOutcome::Edit { .. }),
            "{kind:?}: a plain `j` reaches neither the viewer nor the keymap"
        );
        // `<C-w>` is not plain, so it belongs to the window manager either way.
        assert!(
            !app.viewer_claims_key(&ctrl_w),
            "{kind:?} must never claim the chord prefix itself"
        );

        // Arm the chord the way the real key path does. The second key is `l`,
        // not `j`: `j` is navigation, which the graph no longer claims at all,
        // so it would pass this with the prefix check deleted. `l` is also the
        // graph's own expand verb, so only the check keeps `<C-w>l` whole.
        // Asked of the route, where the rule now lives (`route_key`, review
        // stop 16, D 7), not of one claimant.
        app.chat_keymap.translate(ctrl_w);
        assert!(app.prefix_in_flight(), "the prefix is armed");
        let l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);
        assert!(
            matches!(app.route_key(&l), KeyRoute::Keymap),
            "{kind:?} ate the second key of `<C-w>l`, so the move needed two presses"
        );
    }
}

/// `:open` while `:graph` has borrowed the only transcript pane lands *there*,
/// giving the viewport back — rather than refusing for want of a pane.
///
/// A lent pane is still its session's window. Asking `kind == Output` would
/// have said "no transcript pane to open it in" in the single-pane layout the
/// whole app starts in, which is the most common one there is.
#[test]
fn opening_a_session_reclaims_a_pane_lent_to_a_viewer() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let a = app.conversation().episode().id;
    add_persisted_session(&mut app, "B0");

    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.open_graph_command();
    assert!(app.graph_view().is_some(), "the graph borrowed the pane");

    app.open_by_hash(&a.to_hex()[..8]);

    assert!(app.notice().is_none(), "no refusal: {:?}", app.notice());
    assert!(app.graph_view().is_none(), "the viewer gave the pane back");
    assert_eq!(
        app.pane_session(pane).conversation.episode().id,
        a,
        "the session opened in the reclaimed pane"
    );
}

// --- every pane owns what it shows -------------------------------------

/// **Two `:help` panes are two readers.** Steven: "I did `:on` on the help
/// file, then `:sp` (which spawned a new `:help`) and I found that when I
/// scrolled one, it scrolled the other."
///
/// They were one reader because the manual was a single `Option` on the app, so
/// "this pane's help" and "the help" were the same object — the P2 cursor bug
/// one layer up. Reading two chapters side by side is ordinary; we do it
/// ourselves.
#[test]
fn two_help_panes_scroll_independently() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.open_help_command(None);
    let first = app.workspace.focused();
    app.split_command("", SplitDir::Horizontal);
    let second = app.workspace.focused();
    assert_ne!(first, second, "two panes");
    assert!(app.pane_page(second).is_some(), "the split shows help too");

    // Move the focused one. `j` now moves a cursor and the window follows, so
    // the independence to check is of the CURSOR — the scroll is downstream of
    // it and would not move at all for a few presses.
    for _ in 0..5 {
        app.handle_text_pane_key_for_test(
            PaneKind::Page(PageKind::Help),
            k(KeyCode::Char('j')),
            (80, 20),
        );
    }
    assert_eq!(
        app.pane_page(second).unwrap().text().cursor(),
        5,
        "the focused pane's cursor moved"
    );
    assert_eq!(
        app.pane_page(first).unwrap().text().cursor(),
        0,
        "its sibling stayed where it was"
    );
    // And the window follows independently too: drive one to the end.
    app.handle_text_pane_key_for_test(
        PaneKind::Page(PageKind::Help),
        k(KeyCode::Char('G')),
        (80, 20),
    );
    assert!(
        app.pane_page(second).unwrap().text().scroll() > 0,
        "the focused pane's window followed its cursor"
    );
    assert_eq!(
        app.pane_page(first).unwrap().text().scroll(),
        0,
        "and its sibling's did not"
    );
}

/// Running `:help` again opens **another** help pane rather than teleporting
/// focus to the one that exists.
///
/// The old shortcut read as vim and was not: it made "show me this here" mean
/// "take me over there", so `:help` from the transcript moved the cursor to a
/// pane across the screen — which is how Steven ended up in the prompt.
#[test]
fn a_second_help_opens_here_rather_than_focusing_the_first() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("output", SplitDir::Horizontal);
    let here = app.workspace.focused();
    let there = app
        .workspace
        .ring_order()
        .into_iter()
        .find(|&p| p != here && app.workspace.kind_of(p) == Some(PaneKind::Output))
        .expect("a second transcript pane");

    app.workspace.focus_pane(there);
    app.open_help_command(None);
    app.workspace.focus_pane(here);
    app.open_help_command(None);

    assert_eq!(app.workspace.focused(), here, "focus did not teleport");
    assert!(app.pane_page(here).is_some(), "help opened where I am");
    assert!(
        app.pane_page(there).is_some(),
        "and the other one is still up"
    );
}

/// Closing one viewer closes **that** one. `hide(kind)` acted on the first pane
/// of the kind, so `<Esc>` in the second help pane closed the first.
#[test]
fn closing_one_viewer_leaves_its_twin_alone() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.open_help_command(None);
    let first = app.workspace.focused();
    app.split_command("", SplitDir::Horizontal);
    let second = app.workspace.focused();

    app.close_pane_command(Asked::Plain);
    assert!(
        app.window_of(second).is_none_or(|v| v.page().is_none()),
        "the pane I closed is not showing help"
    );
    assert!(app.pane_page(first).is_some(), "the other one still is");
}

/// `:help delete` used to be refused. Every `*tag*` in the manual — and every
/// `:command` — is a place a reader might mean, and only the thirteen chapter
/// names resolved: a tag namespace that existed for the author alone.
#[test]
fn help_resolves_tags_and_commands_not_just_chapters() {
    let manual = crate::help::compose(&crate::command::commands_chapter());

    // A command, with or without its colon — Steven's actual `:help delete`.
    let delete = crate::help::anchor_line(&manual, "delete").expect("`delete` resolves");
    assert_eq!(
        crate::help::anchor_line(&manual, ":delete"),
        Some(delete),
        "the colon is optional"
    );
    assert!(
        manual.lines().nth(delete).unwrap().contains(":delete"),
        "lands on the :delete row: {:?}",
        manual.lines().nth(delete).unwrap()
    );

    // A tag, with or without the `shannon-` prefix.
    let graph = crate::help::anchor_line(&manual, "shannon-window-session").expect("full tag");
    assert_eq!(
        crate::help::anchor_line(&manual, "window-session"),
        Some(graph),
        "the prefix is optional"
    );

    // And a name that is neither still refuses, naming both namespaces.
    assert_eq!(
        crate::help::anchor_line(&manual, "definitely-not-a-tag"),
        None
    );
    let refusal = crate::help::no_such_topic("nonsense");
    assert!(
        refusal.contains("tag"),
        "the refusal teaches tags: {refusal}"
    );
}

/// `:help <tag>` opens the whole manual *at* the tag, so the surrounding
/// chapter and the next `|link|` are still reachable — vim's `:help {tag}`.
#[test]
fn help_on_a_tag_opens_the_manual_at_that_place() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.open_help_command(Some("delete"));

    let pane = app.workspace.focused();
    assert!(app.notice().is_none(), "no refusal: {:?}", app.notice());
    assert!(app.pane_page(pane).is_some(), "help opened");
    // The jump is HELD until the width is known, and settled by the same call
    // the event loop makes before every draw — the real one, not a mirror of
    // it, because the wiring is the part that was missing.
    app.clamp_text_panes(90, 40);
    assert!(
        app.pane_page(pane).unwrap().text().scroll() > 0,
        "the manual opened somewhere other than the top"
    );
}

/// REGRESSION: `:analyze <command>` must show the report it just built — and
/// must not close the pane you ran it from.
///
/// Moving view content onto the pane inverted an ordering: the handler built
/// the report into a local and *then* called `open_viewer`, which asked
/// `ensure_pane_content` "does some other pane already hold a report?". On a
/// fresh app that is always no, so the command refused, threw away the report
/// it was holding, and — because `close_if_new` read `restore.is_none()`, which
/// is also true of an ordinary transcript pane — closed the transcript.
/// First `:analyze` always failed, so there was never a "latest" one to find.
#[test]
fn analyze_shows_the_report_it_just_built() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let here = app.workspace.focused();
    let panes = app.workspace.pane_count();

    app.open_analyze_command("ls -la");

    assert_eq!(
        app.workspace.pane_count(),
        panes,
        "the pane I ran it from is still here"
    );
    assert_eq!(
        app.workspace.kind_of(here),
        Some(PaneKind::Page(PageKind::Analyze))
    );
    assert!(
        app.pane_page(here).is_some(),
        "the report is on screen: notice was {:?}",
        app.notice()
    );
}

// --- geometry is asked of a pane, like content --------------------------
//
// The buffer model converted every "what is this pane showing" query to a
// `PaneId` and left every "how big is it" query keyed on `PaneKind`, resolved
// through `first_of`. `first_of` IS the at-most-one-pane-per-kind assumption
// the model exists to delete, so the app sized one pane and drew another.

/// `:graph` twice must not stack up inspectors. Deleting the old
/// "already open? focus it" shortcut was right, but it left the companion open
/// on the re-entrant path — and `Workspace::open` always makes a NEW pane.
#[test]
fn opening_the_graph_twice_does_not_stack_up_inspectors() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);

    app.open_graph_command();
    let after_one = app.workspace.pane_count();
    app.open_graph_command();
    app.open_graph_command();

    assert_eq!(
        app.workspace.pane_count(),
        after_one,
        "one graph, one inspector, however many times you ask"
    );
    let inspectors = app
        .workspace
        .ring_order()
        .into_iter()
        .filter(|&p| app.workspace.kind_of(p) == Some(PaneKind::Inspector))
        .count();
    assert_eq!(inspectors, 1);
}

/// **Every documented way out of `:help` gives your transcript back.**
///
/// Steven: "if I type `:help` into the chat pane then I don't know a way to get
/// back to a normal chat pane." `<Esc>` worked; `:q` and `:close` — the ones
/// the manual names — destroyed the pane instead, so the documented key was the
/// one that failed, and with a single transcript pane there was nothing left to
/// go back to. A trap rather than a wall.
#[test]
fn every_way_out_of_help_gives_the_transcript_back() {
    /// A way out, as the user would spell it.
    type Exit = (&'static str, fn(&mut App));
    // The window keys through the window command, the ex commands through
    // dispatch. A row labelled `<Esc>` called `close_viewer` directly — but
    // `<Esc>` stopped closing a text page when closing moved to `:q`/`<C-w>q`
    // (a viewer is a window; vim closes none with a bare key), so the row
    // asserted a key that does nothing, by calling a function instead.
    let ways: [Exit; 5] = [
        ("<C-w>q", |a| {
            let _ = a.window_command_for_test(crate::keymap::WindowCmd::Quit);
        }),
        ("<C-w>c", |a| {
            let _ = a.window_command_for_test(crate::keymap::WindowCmd::Close);
        }),
        (":q", |a| {
            a.dispatch_command_for_test("q");
        }),
        (":close", |a| {
            a.dispatch_command_for_test("close");
        }),
        (":hide", |a| {
            a.dispatch_command_for_test("hide");
        }),
    ];
    for (label, leave) in ways {
        let mut app = test_app();
        app.set_focus(PaneKind::Output);
        let pane = app.workspace.focused();
        app.open_help_command(None);
        assert_eq!(
            app.workspace.kind_of(pane),
            Some(PaneKind::Page(PageKind::Help)),
            "{label}"
        );

        leave(&mut app);

        assert_eq!(
            app.workspace.kind_of(pane),
            Some(PaneKind::Output),
            "{label} must hand the transcript back, not destroy the pane"
        );
        assert!(app.pane_page(pane).is_none(), "{label} retired the manual");
    }
}

/// …including on the last WINDOW, where `:q` would otherwise take the quit
/// path rather than answer "close this view". Two `:q`s leave: give the pane
/// back, then quit from the transcript — the last window beside the prompt
/// bar (`App::last_window`; it was three, the transcript closed first).
///
/// The premise used to be `only_command()` leaving exactly one pane. That state
/// no longer exists — the prompt bar cannot be closed away while anything else
/// is on screen — so the test now builds the smallest workspace that IS
/// reachable and counts the keys from there.
#[test]
fn quit_on_a_lent_transcript_restores_before_it_quits() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.only_command(false);
    assert_eq!(
        app.workspace.pane_count(),
        2,
        "premise: the transcript, and the prompt that cannot be closed away"
    );
    let pane = app.workspace.focused();
    app.open_help_command(None);

    assert!(
        !app.window_command_for_test(crate::keymap::WindowCmd::Quit),
        "the first :q gives the pane back rather than quitting"
    );
    assert_eq!(app.workspace.kind_of(pane), Some(PaneKind::Output));
    assert_eq!(app.workspace.pane_count(), 2, "nothing closed");
    assert!(
        app.window_command_for_test(crate::keymap::WindowCmd::Quit),
        "and the second leaves: the transcript is the last window"
    );
}

/// **`:only` from a transcript leaves you able to type.**
///
/// Steven, dogfooding round 1: `:on` standing in a chat pane closed the prompt,
/// `:new` gave another chat pane, `:prompt` and `:chat` were not commands, and
/// `:help` had no recovery — the process had to be killed. The guard that was
/// supposed to prevent this counted PANES, and two panes is not the same claim
/// as one prompt.
#[test]
fn only_never_leaves_a_workspace_with_nowhere_to_type() {
    for stand_in in [PaneKind::Output, PaneKind::Prompt] {
        let mut app = test_app();
        app.split_command("graph", SplitDir::Vertical);
        app.set_focus(stand_in);
        app.only_command(false);
        assert!(
            app.workspace.has(PaneKind::Prompt),
            "standing in {stand_in:?}: :only must leave a prompt"
        );
        assert!(
            app.workspace.has(stand_in),
            "standing in {stand_in:?}: …and the pane you kept"
        );
    }
}

/// And the prompt cannot be closed one pane at a time either — same invariant,
/// different door. Beside ONE window that window is the last, so the bar's
/// `:q` leaves, as the window's does (`App::last_window`; Steven, 11-09:
/// `shannon` then `:q` said "the only prompt"). Beside two it refuses, and the
/// refusal SAYS which of the two it is, because "the last pane, :q quits" on a
/// workspace that plainly has three panes is a sentence that teaches the
/// wrong thing ([[feedback_visible_refusals]]) — and how to leave instead.
#[test]
fn closing_the_only_prompt_is_refused_with_its_own_reason() {
    for stand in [PaneKind::Prompt, PaneKind::Output] {
        let mut app = test_app();
        app.set_focus(stand);
        assert_eq!(
            app.workspace.pane_count(),
            2,
            "premise: the launch's layout"
        );
        assert!(
            app.window_command_for_test(crate::keymap::WindowCmd::Quit),
            "{stand:?}: beside one window, :q leaves"
        );
    }
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.set_focus(PaneKind::Prompt);
    assert_eq!(
        app.workspace.pane_count(),
        3,
        "premise: two windows and the bar"
    );
    assert!(
        !app.window_command_for_test(crate::keymap::WindowCmd::Quit),
        "beside two windows, :q on the prompt does not quit"
    );
    assert!(app.workspace.has(PaneKind::Prompt), "and it is still there");
    let notice = app.notice().unwrap_or_default().to_string();
    assert!(
        notice.contains("nowhere to type") && notice.contains("`:qa` quits"),
        "the refusal names its own reason, and the way out: {notice:?}"
    );
    // Two bars and no window: the one you stand in closes, never the app —
    // each is a window `:q` closes while the other is left.
    for _ in 0..2 {
        app.set_focus(PaneKind::Output);
        app.dispatch_command_for_test("close");
    }
    app.set_focus(PaneKind::Prompt);
    app.split_command("", SplitDir::Vertical);
    app.settle();
    assert_eq!(
        app.workspace.panes_of(PaneKind::Prompt).count(),
        app.workspace.pane_count(),
        "premise: only bars"
    );
    assert_eq!(app.workspace.pane_count(), 2, "premise: two of them");
    assert!(
        !app.window_command_for_test(crate::keymap::WindowCmd::Quit),
        "one bar of two closes"
    );
    assert_eq!(app.workspace.pane_count(), 1, "it closed");
}

/// **Each viewer names what it shows, so `$EDITOR` gets the right filetype.**
/// Every editor session used to be a `.md` tempfile, so `:config dump` showed
/// TOML as markdown and every `#` comment rendered as a heading (Steven,
/// 2026-09-24). Asserted at the dispatch, which is data, rather than in the
/// editor, which needs a terminal.
#[test]
fn each_viewer_declares_its_filetype() {
    use crate::editor::Filetype;
    let mut app = test_app();
    for (cmd, want) in [
        ("config dump", Filetype::Toml),
        ("keymap", Filetype::Text),
        ("Edit", Filetype::Markdown),
    ] {
        match app.dispatch_command(cmd) {
            CmdEffect::View { kind, .. } => assert_eq!(kind, want, ":{cmd}"),
            _ => panic!(":{cmd} did not open a viewer"),
        }
    }
}

/// **A session opens in the pane you stand in, whatever it shows** — vim's
/// `:b N` in the current window.
///
/// Standing in `:ls` beside `:help`, with no transcript on screen, `:open`
/// split a new pane across the whole top instead of trading the `:ls` pane
/// for the chat (Steven, 2026-10-01). Viewers and sessions each had a
/// placement rule, and a pane made for a viewer could host any viewer and no
/// session.
#[test]
fn a_session_opens_in_the_viewer_pane_you_stand_in() {
    for how in ["b 1", "buffer 1", "open"] {
        let mut app = test_app();
        let (ls, help) = viewers_only(&mut app);
        let panes = app.workspace.pane_count();
        let session = app.current;
        let cmd = if how == "open" {
            format!("open {}", app.conversation().episode().id.short_hex())
        } else {
            how.to_string()
        };
        app.dispatch_command_for_test(&cmd);
        assert_eq!(
            app.workspace.kind_of(ls),
            Some(PaneKind::Output),
            "`:{cmd}` trades the `:ls` pane for the chat"
        );
        assert!(
            app.pane_shows(ls, session),
            "`:{cmd}`: showing that session"
        );
        assert_eq!(app.workspace.pane_count(), panes, "`:{cmd}` split nothing");
        assert_eq!(
            app.workspace.kind_of(help),
            Some(PaneKind::Page(PageKind::Help)),
            "`:{cmd}`"
        );
        assert!(
            app.pane_listing(ls).is_none(),
            "`:{cmd}`: the listing retired"
        );
    }
}

/// **"Already here" means the pane SHOWS it**, not that it resolves to it.
/// A `:help` pane resolves to the aim (`pane_session`; the aim is the
/// foreground here, and it was the foreground outright when this was
/// written), so `:b 1` for the foreground from a manual split off the
/// transcript read as "already here, in this pane" and did nothing.
#[test]
fn opening_the_foreground_from_a_help_pane_replaces_the_manual() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let transcript = app.workspace.focused();
    app.split_command("help", SplitDir::Horizontal);
    let help = app.workspace.focused();
    assert!(
        app.pane_shows(transcript, app.current),
        "premise: shown beside"
    );
    assert_eq!(
        app.pane_session(help).id,
        app.current,
        "premise: resolves to it (the aim, which is the foreground here)"
    );

    app.dispatch_command_for_test("b 1");

    assert_eq!(app.workspace.kind_of(help), Some(PaneKind::Output));
    assert!(app.pane_shows(help, app.current));
}

/// **`:open <turn>` places like `:open <session>`** from a viewer pane too —
/// the round-3 rule, from the starting position the unified rule added. The
/// checkout used `reveal`, which keeps a session where it is already
/// visible, so from a `:help` split the session id replaced the manual and
/// the turn id did not.
#[test]
fn open_turn_and_open_session_land_in_the_same_pane_from_a_viewer() {
    for turn in [false, true] {
        let dir = TempDir::new().unwrap();
        let mut app = app_with_n_turns(dir.path(), 3);
        let episode = app.conversation().episode().id;
        let turn0 = app.conversation().episode().own_turns()[0];
        app.set_focus(PaneKind::Output);
        app.split_command("help", SplitDir::Horizontal);
        let help = app.workspace.focused();
        let hash = if turn {
            turn0.to_hex()[..8].to_string()
        } else {
            episode.to_hex()[..8].to_string()
        };
        app.open_command(&hash);
        assert_eq!(
            app.workspace.kind_of(help),
            Some(PaneKind::Output),
            "turn={turn}: the pane you stood in shows it"
        );
    }
}

/// **A composition you can already see does not take over the manual.**
/// `reveal` keeps a session where it is visible; only `:open`/`:b` put one in
/// the current window regardless. `:clone` from a `:help` split beside the
/// transcript lands in the transcript, and the manual stays.
#[test]
fn a_composition_already_on_screen_leaves_the_manual_alone() {
    // On disk: `:clone` refuses a session that is not.
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    app.set_focus(PaneKind::Output);
    let transcript = app.workspace.focused();
    app.split_command("help", SplitDir::Horizontal);
    let help = app.workspace.focused();
    let before = app.conversation().episode().id;

    app.dispatch_command_for_test("clone");

    assert_ne!(
        app.pane_session(transcript).conversation.episode().id,
        before,
        "premise: the clone landed, in the transcript"
    );
    assert_eq!(
        app.workspace.kind_of(help),
        Some(PaneKind::Page(PageKind::Help)),
        "the manual stays"
    );
}

/// **A view given back is never blank.** Made for `:graph`, then `:help` in
/// it, then `:q`: it came back a graph pane with no graph in it — blank, its
/// keys falling through to the chat — because `restore` named the graph
/// whose content `:help` had already retired. A view comes back kind AND
/// content now (the window's history), or not at all: the manual with its page,
/// then, with nothing under it, the pane made for it closes.
#[test]
fn a_view_given_back_is_never_blank() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let panes = app.workspace.pane_count();
    app.split_command("help", SplitDir::Horizontal);
    let pane = app.workspace.focused();
    assert!(app.pane_page(pane).is_some(), "premise: the manual");
    app.dispatch_command_for_test("messages");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Messages)),
        "premise"
    );

    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Help))
    );
    assert!(
        app.workspace
            .panes_of(PaneKind::Page(PageKind::Help))
            .all(|p| app.pane_page(p).is_some()),
        "no help pane without a manual"
    );

    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.pane_count(),
        panes,
        "the pane made for it went"
    );
}

/// **An inspector inspects the tree it names, not the nearest** — a second
/// tree split off the first's inspector lands between them in the ring
/// (`[A, B, B's, A's]`), nearer A's inspector than A is. Paired by ring
/// proximity, A's inspector took B's tree — two on one, none on A's;
/// named (V5.3 (3e), review finding 7), it keeps A's.
#[test]
fn an_inspector_inspects_the_tree_it_names_not_the_nearest() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("graph");
    let a = app.workspace.focused();
    let a_inspector = app.companions_of(a).next().expect("premise: A's inspector");
    app.workspace.focus_pane(a_inspector);
    app.dispatch_command_for_test("vsgraph");
    let b = app.workspace.focused();
    assert_eq!(
        app.workspace.kind_of(b),
        Some(PaneKind::Graph),
        "premise: B"
    );
    let ring = app.workspace.ring_order();
    let at = |p| ring.iter().position(|&q| q == p).expect("in the ring");
    assert!(
        at(b).abs_diff(at(a_inspector)) < at(a).abs_diff(at(a_inspector)),
        "premise: B nearer A's inspector than A is: {ring:?}"
    );
    assert_eq!(
        app.host_of(a_inspector),
        Some(a),
        "A's inspector inspects A"
    );
    let b_inspector = app.companions_of(b).next().expect("B brought its own");
    assert_ne!(b_inspector, a_inspector, "one each");
}

/// **`:only` keeps a companion with its host, both ways** — in a tree's
/// inspector it keeps the tree, in the tree it keeps the inspector, as it
/// keeps a decision pane with its window (V5.3 (3e)). From the inspector it
/// closed the tree around it and left the inspector alone, inspecting
/// nothing: the layout keeps a last pane, so not even the reconcile could
/// close the orphan (this test, written for the orphan rule, found it).
#[test]
fn only_in_an_inspector_leaves_no_orphan() {
    for stand_in_tree in [false, true] {
        let mut app = test_app();
        app.set_focus(PaneKind::Output);
        app.dispatch_command_for_test("graph");
        let tree = app.workspace.focused();
        let inspector = app
            .companions_of(tree)
            .next()
            .expect("premise: its inspector");
        app.split_command("output", SplitDir::Horizontal);
        let other = app.workspace.focused();
        app.workspace
            .focus_pane(if stand_in_tree { tree } else { inspector });
        app.dispatch_command_for_test("only");
        assert!(app.window_of(other).is_none(), "premise: the rest closed");
        assert_eq!(
            app.host_of(inspector),
            Some(tree),
            "the pair kept, inspecting"
        );
    }
}

/// **A companion whose host is closed by any door goes at the reconcile** —
/// the host closed by the workspace's own `close`, as a door that forgot
/// `release_companion` would: the reconcile closes what names it (V5.3
/// (3e)), the backstop "every resource bound to a pane reconciles here"
/// (`reconcile_pane_state`). Hidden only once no host of its kind was left,
/// with a second tree open the orphan adopted that one.
#[test]
fn a_companion_whose_host_closed_goes_at_the_reconcile() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("graph");
    let tree = app.workspace.focused();
    let inspector = app
        .companions_of(tree)
        .next()
        .expect("premise: its inspector");
    app.split_command("graph", SplitDir::Horizontal);
    assert_eq!(
        app.workspace.panes_of(PaneKind::Graph).count(),
        2,
        "premise: a second tree"
    );
    assert!(
        app.workspace.close(tree),
        "premise: the host closed by the generic door"
    );
    app.reconcile_pane_state();
    assert!(
        app.window_of(inspector).is_none(),
        "the orphan went with it"
    );
}

/// **The same, when the host pane is still there and shows another kind**
/// — the state a door that forgot `release_companion` would leave (V5.3
/// verify, F3): `host_of` holds only while the host pane shows the host's
/// kind, and the reconcile closes the companion. Asked of a closed host
/// alone, "the host merely exists" passed every test.
#[test]
fn a_companion_whose_host_shows_another_kind_holds_none() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("graph");
    let tree = app.workspace.focused();
    let inspector = app
        .companions_of(tree)
        .next()
        .expect("premise: its inspector");
    assert_eq!(app.host_of(inspector), Some(tree), "premise: paired");
    let help = app
        .content_for(Some(tree), PaneKind::Page(PageKind::Help))
        .expect("premise: the manual");
    // Past every door: `put_view` releases nothing.
    app.put_view(tree, help);
    assert_eq!(
        app.workspace.kind_of(tree),
        Some(PaneKind::Page(PageKind::Help)),
        "premise: the host pane shows another kind"
    );
    assert_eq!(app.host_of(inspector), None, "it holds no host");
    app.reconcile_pane_state();
    assert!(
        app.window_of(inspector).is_none(),
        "and goes at the reconcile"
    );
}

/// **A companion of another kind does not answer for this one** — asked of
/// `pair_companion` alone, past every door: a pane on a tree whose only
/// companion is a theme preview still naming it gets its inspector (V5.3
/// verify, F2). The `:q` below holds this only with its release; this holds
/// it without one.
#[test]
fn a_companion_of_another_kind_does_not_answer_for_this_one() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.dispatch_command_for_test("themes");
    let kinds = |app: &App| -> Vec<Option<PaneKind>> {
        let ids: Vec<_> = app.companions_of(pane).collect();
        ids.into_iter().map(|c| app.workspace.kind_of(c)).collect()
    };
    assert_eq!(
        kinds(&app),
        [Some(PaneKind::ThemePreview)],
        "premise: the list's preview"
    );
    let tree = app
        .content_for(Some(pane), PaneKind::Graph)
        .expect("premise: a tree");
    app.put_view(pane, tree);
    app.pair_companion(pane, PaneKind::Graph);
    assert!(
        kinds(&app).contains(&Some(PaneKind::Inspector)),
        "the tree's inspector beside it: {:?}",
        kinds(&app)
    );
}

/// **A tree given back over `:themes` comes back with ITS companion** —
/// `:graph`, `:themes` in the same pane, `:q`: the inspector beside the
/// tree, the list's preview gone (V5.3 verify, F2). `pair_companion` asked
/// whether ANY companion named the pane, so a preview still naming it read
/// as the tree's own; it held only because `:q` released the preview
/// first, and deleting that release passed every test.
#[test]
fn q_from_a_list_over_a_tree_gives_the_tree_its_inspector() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.dispatch_command_for_test("graph");
    app.dispatch_command_for_test("themes");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Themes)),
        "premise"
    );
    let kinds = |app: &App| -> Vec<Option<PaneKind>> {
        let ids: Vec<_> = app.companions_of(pane).collect();
        ids.into_iter().map(|c| app.workspace.kind_of(c)).collect()
    };
    assert_eq!(
        kinds(&app),
        [Some(PaneKind::ThemePreview)],
        "premise: its preview"
    );
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Graph),
        "premise: the tree back"
    );
    assert_eq!(
        kinds(&app),
        [Some(PaneKind::Inspector)],
        "the tree's own companion beside it"
    );
}

/// **Every companion naming a host leaves with it** — a bare `:sp` of an
/// inspector is a second on the same tree; `:help` over the tree takes
/// both, not the first and an orphan inspecting nothing until some later
/// close reconciled it.
#[test]
fn every_inspector_of_a_tree_leaves_with_it() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("graph");
    let tree = app.workspace.focused();
    let first = app
        .companions_of(tree)
        .next()
        .expect("premise: its inspector");
    app.workspace.focus_pane(first);
    app.dispatch_command_for_test("split");
    assert_eq!(
        app.companions_of(tree).count(),
        2,
        "premise: two on one tree"
    );
    app.workspace.focus_pane(tree);
    app.dispatch_command_for_test("help");
    assert_eq!(
        app.workspace.panes_of(PaneKind::Inspector).count(),
        0,
        "both left with it"
    );
}

/// **A tree split off brings an inspector that inspects** — `:sg`, and a
/// bare `:sp` of a tree: the inspector beside the new tree draws its
/// selection, as `:graph`'s does. The split opened its companion by a door
/// of its own, born with no view since V5.3 (3b), so it drew nothing until
/// (3d) gave every companion one door (`open_companion`).
#[test]
fn a_split_trees_inspector_shows_what_it_inspects() {
    for (split, from_tree) in [("sgraph", false), ("split", true)] {
        let mut app = test_app();
        app.conversation_mut().begin_turn("a question");
        app.conversation_mut().push_chunk("an answer");
        app.conversation_mut().commit_turn();
        app.set_focus(PaneKind::Output);
        if from_tree {
            app.dispatch_command_for_test("graph");
        }
        let before: Vec<_> = app.workspace.panes_of(PaneKind::Inspector).collect();
        app.dispatch_command_for_test(split);
        app.before_frame(100, 40);
        let new: Vec<_> = (app.workspace.panes_of(PaneKind::Inspector))
            .filter(|p| !before.contains(p))
            .collect();
        assert_eq!(
            new.len(),
            1,
            "`:{split}`: premise: an inspector beside the new tree"
        );
        assert!(
            app.pane_inspector(new[0]).is_some(),
            "`:{split}`: it shows the tree's selection"
        );
    }
}

/// **A graph's inspector leaves with the graph**, by every door: replaced by
/// a viewer, replaced by a session, closed. With one graph the orphan
/// inspected nothing; with a second graph open it adopted THAT one — two
/// inspectors on one tree. **And its window leaves with it** (V5.3 (4)):
/// claimed where it is closed (`release_companion`), not left in the
/// workspace's closed list for the next close of yours to drain — out of
/// every walk's reach till then, `:unread`'s too, which lets go of what the
/// windows painted.
#[test]
fn a_graphs_inspector_leaves_with_it() {
    for door in ["help", "b 1", "q"] {
        for two_graphs in [false, true] {
            let mut app = test_app();
            app.set_focus(PaneKind::Output);
            if two_graphs {
                app.split_command("graph", SplitDir::Horizontal);
                app.set_focus(PaneKind::Output);
            }
            app.split_command("graph", SplitDir::Horizontal);
            let graphs = app.workspace.panes_of(PaneKind::Graph).count();
            let inspectors = || app.workspace.panes_of(PaneKind::Inspector).count();
            assert_eq!(inspectors(), graphs, "premise: one each");

            app.dispatch_command_for_test(door);

            let graphs = app.workspace.panes_of(PaneKind::Graph).count();
            let inspectors: Vec<_> = app.workspace.panes_of(PaneKind::Inspector).collect();
            assert_eq!(
                inspectors.len(),
                graphs,
                "`:{door}`, two_graphs={two_graphs}: one inspector per graph"
            );
            assert!(
                inspectors.iter().all(|&p| app.inspected_graph(p).is_some()),
                "`:{door}`, two_graphs={two_graphs}: none inspecting nothing"
            );
            assert!(
                app.workspace.take_closed().is_empty(),
                "`:{door}`, two_graphs={two_graphs}: no closed window waits"
            );
        }
    }
}
