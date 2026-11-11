//! Window commands: split, close, only, and pane focus.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// `:split` / `:vsplit` / `:close` / `:only` over the real command path —
/// every step typed. It called `split_command`, `close_pane_command` and
/// `workspace.only` itself, so it held none of the dispatch it named
/// (review stop 12, T1 2).
#[test]
fn splits_open_and_close_panes() {
    let mut app = test_app();
    let panes = |a: &App| a.workspace.slots().len();
    let start = panes(&app);

    // A bare `:split` duplicates the focused pane's content, and focus
    // follows the new one (vim).
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("split");
    assert_eq!(panes(&app), start + 1);
    assert_eq!(app.focus(), PaneKind::Output, "same content, new pane");

    // `:vsplit graph` opens a named pane — and the graph brings its
    // inspector, so this is +2.
    app.dispatch_command_for_test("vsplit graph");
    assert!(app.workspace.has(PaneKind::Graph));
    assert!(app.workspace.has(PaneKind::Inspector));
    assert!(app.graph_view().is_some(), "and its content was built");

    // `:close` on the graph retires the view state with the last pane.
    app.set_focus(PaneKind::Graph);
    app.dispatch_command_for_test("close");
    assert!(!app.workspace.has(PaneKind::Graph));
    assert!(
        app.graph_view().is_none(),
        "the last pane of a kind takes its state with it"
    );

    // `:only` collapses to the focused pane, whatever is open — and the
    // prompt you type into, its draft its window's own (10-26).
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("only");
    assert_eq!(panes(&app), 2, "the transcript, and the prompt kept");
    // …the transcript closes, and the last pane refuses to, with a reason.
    app.dispatch_command_for_test("close");
    assert_eq!(panes(&app), 1, "the prompt alone");
    app.notices.clear();
    app.dispatch_command_for_test("close");
    assert_eq!(panes(&app), 1, "never zero panes");
    assert!(app.notice().is_some(), "and it says why");
}

/// `:sg` and `:vsg` — vim's "split and open X" family — differ in the axis,
/// and both split **the pane you are in**, whichever pane that is.
///
/// This used to carry a prompt exception, and the exception was the bug: from
/// the prompt, `:sp` silently split the TRANSCRIPT, so the new pane appeared
/// above the prompt while the same key from the transcript put it below. Same
/// key, same words, two different results depending on where you stood.
///
/// **Typed, every spelling of every split** — `:split`, `:vsplit` and
/// their aliases with `graph`, `:sg` and `:vsg` with theirs, from the
/// command table. This mapped each command to an axis itself and called
/// `split_command` with it, so swapping the dispatch's `Split`/`VSplit` arms
/// or its `SGraph`/`VSGraph` ones passed the crate (review stop 12, T1 2).
#[test]
fn split_open_commands_split_the_focused_pane_on_the_named_axis() {
    let frame = Rect::new(0, 0, 100, 40);
    use crate::command::{Command, TABLE};
    let splits = TABLE.iter().filter_map(|spec| {
        let (side_by_side, arg) = match spec.cmd {
            Command::Split => (false, " graph"),
            Command::VSplit => (true, " graph"),
            Command::SGraph => (false, ""),
            Command::VSGraph => (true, ""),
            _ => return None,
        };
        let spellings = std::iter::once(spec.name).chain(spec.aliases.iter().copied());
        Some(spellings.map(move |s| (format!("{s}{arg}"), side_by_side)))
    });
    let lines: Vec<(String, bool)> = splits.flatten().collect();
    assert_eq!(lines.len(), 10, "premise, every spelling: {lines:?}");
    for (cmd, want_side_by_side) in lines {
        for from in [PaneKind::Prompt, PaneKind::Output] {
            let mut app = test_app();
            app.set_focus(from);
            let split_me = app.workspace.focused();
            let before = pane_rect(&app.workspace.frame_layout(frame), from);

            app.dispatch_command_for_test(&cmd);

            let layout = app.workspace.frame_layout(frame);
            let graph = pane_rect(&layout, PaneKind::Graph);
            let host = layout.pane(split_me).expect("the split pane survives").rect;
            assert_eq!(
                graph.x != host.x,
                want_side_by_side,
                "{cmd:?} from {from:?} used the wrong axis: graph {graph:?} host {host:?}"
            );
            // The new pane came out of the FOCUSED pane's area, not some other
            // pane's: together they occupy exactly what that pane had.
            // The room came out of the FOCUSED pane, not some other pane's:
            // the graph lands inside the area that pane used to occupy. (It
            // shares that area with its inspector, so this is containment
            // rather than an exact sum.)
            let inside = graph.x >= before.x
                && graph.y >= before.y
                && graph.x + graph.width <= before.x + before.width
                && graph.y + graph.height <= before.y + before.height;
            assert!(
                inside,
                "{cmd:?} from {from:?} took room from another pane: \
                 graph {graph:?} is not inside {before:?}"
            );
        }
    }
}

/// **`:vnew` and `:snew`: a fresh session in a new pane beside the window
/// you are about** (Steven, 11-10: it took `:vsp` then `:new`) — typed,
/// every spelling, on its axis; from the transcript, and from the prompt,
/// which splits the window it aims at and never itself (`:vs` there makes a
/// second prompt). The window split keeps its session; the new one's is its
/// own, focused, and the prompt aims there.
#[test]
fn vnew_and_snew_show_a_fresh_session_beside_the_window_you_are_about() {
    let frame = Rect::new(0, 0, 100, 40);
    for (cmd, side_by_side) in [("vnew", true), ("vne", true), ("snew", false)] {
        for from in [PaneKind::Output, PaneKind::Prompt] {
            let mut app = test_app();
            app.set_focus(PaneKind::Output);
            let host = app.workspace.focused();
            let was = app.pane_session(host).id;
            let room = (app.workspace.frame_layout(frame).pane(host))
                .expect("the host")
                .rect;
            app.set_focus(from);

            app.dispatch_command_for_test(cmd);

            let new = app.workspace.focused();
            let at = format!("`:{cmd}` from {from:?} ({:?})", app.notice());
            assert_ne!(new, host, "{at}: a new pane, focused");
            assert_eq!(app.kind_of(new), Some(PaneKind::Output), "{at}");
            let layout = app.workspace.frame_layout(frame);
            let old = layout.pane(host).expect("the host survives").rect;
            let made = layout.pane(new).expect("the new pane").rect;
            assert_eq!(old.x != made.x, side_by_side, "{at}: the axis");
            // Out of the host's room: the prompt's is untouched.
            let inside = |r: Rect| {
                r.x >= room.x
                    && r.y >= room.y
                    && r.right() <= room.right()
                    && r.bottom() <= room.bottom()
            };
            assert!(
                inside(made) && inside(old),
                "{at}: {made:?} and {old:?} in {room:?}"
            );
            assert_eq!(
                app.pane_session(host).id,
                was,
                "{at}: the host keeps its own"
            );
            let fresh = app.pane_session(new).id;
            assert_ne!(fresh, was, "{at}: a session of its own");
            assert_eq!(app.prompt_target(), fresh, "{at}: the prompt aims there");
            // Born on the session split from, its alternate: `:b#` goes back.
            app.dispatch_command_for_test("b#");
            assert_eq!(
                app.pane_session(new).id,
                was,
                "{at}: `:b#` to the one split from"
            );
        }
    }
}

/// **`:vnew` makes a new session always, and says nothing false** — from a
/// page in a window of its own, the aim's blank session in no window, the new
/// pane gets a new one: not `:new`'s stay, which said "already on a fresh
/// session" under the pane just made for it (that mutant passed a test built
/// over a LENT page, whose window still counts as the session's). The blank
/// one is not hidden: `:ls` lists it, as vim lists a hidden buffer — `:ls`
/// once kept an uncalled session out, and `:vnew` stepped around it (Steven,
/// 11-11: "That is a secret buffer"). From the prompt alone, a window made
/// where `:new` makes one, on a new session too.
#[test]
fn vnew_makes_a_new_session_from_a_page_and_from_a_prompt_alone() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let transcript = app.workspace.focused();
    let blank = app.pane_session(transcript).id;
    app.dispatch_command_for_test("vsplit help");
    let manual = app.workspace.focused();
    app.workspace.focus_pane(transcript);
    app.dispatch_command_for_test("close");
    assert!(
        matches!(app.kind_of(manual), Some(PaneKind::Page(_))),
        "premise: the manual, in its own window"
    );
    assert_eq!(
        app.prompt_target(),
        blank,
        "premise: aimed at the blank one"
    );
    assert!(
        app.panes_with_session(blank, Over::EveryTab)
            .next()
            .is_none(),
        "premise: in no window"
    );
    app.workspace.focus_pane(manual);
    app.notices.clear();
    let sessions = app.buffers.sessions().count();

    app.dispatch_command_for_test("vnew");

    let new = app.workspace.focused();
    assert_ne!(new, manual, "a new pane: {:?}", app.notice());
    assert_ne!(app.pane_session(new).id, blank, "a new session");
    assert_eq!(app.buffers.sessions().count(), sessions + 1, "one made");
    assert!(
        !app.notice().unwrap_or_default().contains("already on"),
        "{:?}",
        app.notice()
    );
    assert!(
        matches!(app.kind_of(manual), Some(PaneKind::Page(_))),
        "the manual stays where it was"
    );
    assert!(
        (app.buffers_listing(false).lines())
            .any(|l| l.trim_start().starts_with(&format!("b{} ", blank.number()))),
        "the blank one listed, not hidden"
    );

    // From the prompt alone: a window made for it, on a new session.
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let blank = app.pane_session(app.workspace.focused()).id;
    app.dispatch_command_for_test("close");
    assert_eq!(app.workspace.slots().len(), 1, "premise: the prompt alone");
    assert_eq!(app.prompt_target(), blank, "premise: aimed at it");
    app.dispatch_command_for_test("vnew");
    assert_eq!(
        app.workspace.slots().len(),
        2,
        "a window: {:?}",
        app.notice()
    );
    let made = app.workspace.panes_of(PaneKind::Output).next();
    let shown = made.map(|pane| app.pane_session(pane).id);
    assert!(
        shown.is_some_and(|shown| shown != blank),
        "a new session: {shown:?} against {blank:?}"
    );
}

#[test]
fn focus_starts_on_prompt_and_toggles() {
    let mut app = test_app();
    assert_eq!(app.focus(), PaneKind::Prompt);
    app.toggle_focus();
    assert_eq!(app.focus(), PaneKind::Output);
    app.toggle_focus();
    assert_eq!(app.focus(), PaneKind::Prompt);
}

#[test]
fn window_focus_commands_move_focus() {
    let mut app = test_app();
    assert_eq!(app.focus(), PaneKind::Prompt);
    // Stacked (output top, prompt below): up → chat, down → prompt.
    assert!(
        !app.apply_window(WindowCmd::Focus(Direction::Up), None, 80, 50),
        "not a quit"
    );
    assert_eq!(app.focus(), PaneKind::Output);
    assert!(
        !app.apply_window(WindowCmd::Focus(Direction::Down), None, 80, 50),
        "not a quit"
    );
    assert_eq!(app.focus(), PaneKind::Prompt);
    // No horizontal neighbor on a vertical stack → left/right are no-ops.
    assert!(
        !app.apply_window(WindowCmd::Focus(Direction::Left), None, 80, 50),
        "not a quit"
    );
    assert_eq!(app.focus(), PaneKind::Prompt);
    // Cycle toggles between the two.
    assert!(
        !app.apply_window(WindowCmd::FocusCycle, None, 80, 50),
        "not a quit"
    );
    assert_eq!(app.focus(), PaneKind::Output);
}

#[test]
fn window_resize_changes_the_prompt_height() {
    let mut app = test_app();
    let base = prompt_content_rows(&app);
    // Prompt focused: grow the focused pane = taller prompt; shrink = shorter.
    assert!(
        !app.apply_window(WindowCmd::ResizeGrow, Some(3), 80, 50),
        "not a quit"
    );
    assert_eq!(prompt_content_rows(&app), base + 3);
    assert!(
        !app.apply_window(WindowCmd::ResizeShrink, Some(5), 80, 50),
        "not a quit"
    );
    assert_eq!(prompt_content_rows(&app), base - 2);
    // `=` resets to the configured default.
    assert!(
        !app.apply_window(WindowCmd::Equalize, None, 80, 50),
        "not a quit"
    );
    assert_eq!(prompt_content_rows(&app), app.config.tui.prompt.height);
    // Chat focused: growing the chat shrinks the prompt.
    app.set_focus(PaneKind::Output);
    assert!(
        !app.apply_window(WindowCmd::ResizeGrow, Some(2), 80, 50),
        "not a quit"
    );
    assert_eq!(prompt_content_rows(&app), base - 2);
    // Width resize is a no-op while stacked (vim: `</>` need a horizontal split).
    let h = prompt_content_rows(&app);
    assert!(
        !app.apply_window(WindowCmd::ResizeWider, Some(3), 80, 50),
        "not a quit"
    );
    assert_eq!(prompt_content_rows(&app), h);
}

#[test]
fn window_resize_clamps_to_floor_and_frame() {
    let mut app = test_app();
    // Can't shrink below one row.
    assert!(
        !app.apply_window(WindowCmd::ResizeShrink, Some(9999), 80, 50),
        "not a quit"
    );
    assert_eq!(prompt_content_rows(&app), 1);
    // Can't grow past what the frame holds (status + min output + borders
    // reserved): on a height-50 frame the prompt stays under 50.
    assert!(
        !app.apply_window(WindowCmd::ResizeGrow, Some(9999), 80, 50),
        "not a quit"
    );
    assert!(prompt_content_rows(&app) < 50 && prompt_content_rows(&app) >= 1);
    // A tiny frame floors the prompt at 1.
    assert!(
        !app.apply_window(WindowCmd::ResizeGrow, Some(9999), 80, 4),
        "not a quit"
    );
    assert_eq!(prompt_content_rows(&app), 1);
}

#[test]
fn window_relocate_reorders_the_stack() {
    let mut app = test_app();
    assert!(!is_before(&app, PaneKind::Prompt, PaneKind::Output)); // default: output on top, prompt below
    // Focused (prompt) to the top → prompt becomes the first pane.
    assert!(
        !app.apply_window(WindowCmd::Relocate(Direction::Up), None, 80, 50),
        "not a quit"
    );
    assert!(is_before(&app, PaneKind::Prompt, PaneKind::Output));
    // ...then to the bottom → prompt no longer first.
    assert!(
        !app.apply_window(WindowCmd::Relocate(Direction::Down), None, 80, 50),
        "not a quit"
    );
    assert!(!is_before(&app, PaneKind::Prompt, PaneKind::Output));
    // From the chat pane: chat to the top → prompt is the bottom pane.
    app.set_focus(PaneKind::Output);
    assert!(
        !app.apply_window(WindowCmd::Relocate(Direction::Up), None, 80, 50),
        "not a quit"
    );
    assert!(!is_before(&app, PaneKind::Prompt, PaneKind::Output));
    // ...chat to the bottom → prompt is the top pane.
    assert!(
        !app.apply_window(WindowCmd::Relocate(Direction::Down), None, 80, 50),
        "not a quit"
    );
    assert!(is_before(&app, PaneKind::Prompt, PaneKind::Output));
}

#[test]
fn window_relocate_side_by_side_and_width_resize() {
    let mut app = test_app();
    // `<C-w>L` (prompt focused) → side-by-side, prompt on the right.
    assert!(
        !app.apply_window(WindowCmd::Relocate(Direction::Right), None, 80, 50),
        "not a quit"
    );
    assert!(side_by_side(&app, PaneKind::Prompt, PaneKind::Output));
    assert!(!is_before(&app, PaneKind::Prompt, PaneKind::Output)); // prompt is the right (second) pane
    // Entry splits the frame EVENLY. Changed 2026-07-26 with the
    // arbitrary-pane rewrite: the old code seeded `prompt_cols` to
    // `frame_w / 2` *content* columns, so the prompt got 40 and the
    // transcript 36 — "half" that wasn't. An axis nobody has sized is now
    // proportional, so both panes get 38 and no reseed hook has to fire.
    assert_eq!(prompt_content_cols(&app), 38);
    // `</>` resize the width now (prompt focused: wider = wider prompt).
    assert!(
        !app.apply_window(WindowCmd::ResizeWider, Some(5), 80, 50),
        "not a quit"
    );
    assert_eq!(prompt_content_cols(&app), 43);
    assert!(
        !app.apply_window(WindowCmd::ResizeNarrower, Some(2), 80, 50),
        "not a quit"
    );
    assert_eq!(prompt_content_cols(&app), 41);
    // `+/-` (height) is a no-op while side-by-side.
    let w = prompt_content_cols(&app);
    assert!(
        !app.apply_window(WindowCmd::ResizeGrow, Some(3), 80, 50),
        "not a quit"
    );
    assert_eq!(prompt_content_cols(&app), w);
    // `<C-w>H` (prompt focused) → prompt on the left, still side-by-side;
    // the width is preserved (re-issuing only flips the side).
    assert!(
        !app.apply_window(WindowCmd::Relocate(Direction::Left), None, 80, 50),
        "not a quit"
    );
    assert!(is_before(&app, PaneKind::Prompt, PaneKind::Output));
    assert_eq!(prompt_content_cols(&app), w);
    // `<C-w>K` returns to a vertical stack.
    assert!(
        !app.apply_window(WindowCmd::Relocate(Direction::Up), None, 80, 50),
        "not a quit"
    );
    assert!(!side_by_side(&app, PaneKind::Prompt, PaneKind::Output));
}

#[test]
fn window_focus_directional_side_by_side() {
    let mut app = test_app();
    // Side-by-side with the prompt on the right (chat left, prompt right).
    assert!(
        !app.apply_window(WindowCmd::Relocate(Direction::Right), None, 80, 50),
        "not a quit"
    );
    assert!(!is_before(&app, PaneKind::Prompt, PaneKind::Output));
    // Left → first (chat), Right → second (prompt).
    assert!(
        !app.apply_window(WindowCmd::Focus(Direction::Left), None, 80, 50),
        "not a quit"
    );
    assert_eq!(app.focus(), PaneKind::Output);
    assert!(
        !app.apply_window(WindowCmd::Focus(Direction::Right), None, 80, 50),
        "not a quit"
    );
    assert_eq!(app.focus(), PaneKind::Prompt);
    // Up/Down have no neighbor side-by-side → no-op.
    assert!(
        !app.apply_window(WindowCmd::Focus(Direction::Up), None, 80, 50),
        "not a quit"
    );
    assert_eq!(app.focus(), PaneKind::Prompt);
}

/// **Every guard on a user-initiated action says why, on screen.**
///
/// Swept 2026-07-27 after Steven's "we've certainly missed similar bugs":
/// 28 of 43 `warn!` sites in this file had no on-screen path, and many were
/// reachable by a keypress. The worst was `d` on a session another branch
/// shares — you confirm the delete, the guard refuses to orphan the turns, and
/// *nothing appears*. The manual promises the opposite in two places ("the
/// reason appears on the status line — never only in a log"), and that promise
/// was true of `s` (squash), which is where the rule was first applied. Same
/// class as everything else this week: applied to the reported instance, not
/// to the class.
///
/// The table calls the real handlers rather than a stand-in dispatcher — a
/// second copy of command routing would be the very bug this sweep is about.
#[test]
fn a_refused_command_always_says_why() {
    // (a name for the failure, what the user did, what the notice must mention)
    type Case = (&'static str, fn(&mut App), &'static str);
    let cases: &[Case] = &[
        (":open with no id", |a| a.open_command(""), "needs an id"),
        (
            ":open <unknown>",
            |a| a.open_command("ffffffff"),
            "no session or turn",
        ),
        (
            ":clone <unknown>",
            |a| a.clone_by_hash("ffffffff"),
            "no session matches",
        ),
        (
            ":delete <unknown>",
            |a| a.delete_by_hash("ffffffff"),
            "no session matches",
        ),
        (
            ":help <unknown topic>",
            |a| a.open_help_command(Some("nonsense")),
            "no help for",
        ),
        (
            ":split <unknown pane>",
            |a| a.split_command("nowhere", SplitDir::Horizontal),
            "no pane called",
        ),
    ];
    for (what, run, want) in cases {
        let mut app = test_app();
        app.notices.clear();
        run(&mut app);
        let notice = app
            .notice()
            .unwrap_or_else(|| panic!("{what} refused silently — that reads as a dead key"));
        assert!(
            notice.to_lowercase().contains(&want.to_lowercase()),
            "{what} said {notice:?}, which does not mention {want:?}"
        );
    }
}

/// **`:q` closes the window; only the last one quits.**
///
/// Steven, dogfooding: "How do I close a window? Doing `:q` quits everything."
/// It is the right question and the right key — vim's `:q` closes the current
/// window and leaves the editor only when there is nothing left to close. Ours
/// exited unconditionally, which made "close this pane" a verb you had to
/// already know the name of (`:close`), and made the manual's advice ("the last
/// pane will not close; `:q` quits") actively misleading.
#[test]
fn quit_closes_the_pane_and_only_exits_on_the_last_one() {
    let mut app = test_app();
    app.split_command("output", SplitDir::Vertical);
    let before = app.workspace.pane_count();
    assert!(before > 1, "several panes open");

    // Not the last pane: `:q` closes it and stays running.
    let quit = app.window_command_for_test(crate::keymap::WindowCmd::Quit);
    assert!(!quit, "`:q` with panes left must not exit");
    assert_eq!(app.workspace.pane_count(), before - 1, "it closed one");

    // Close down to the last window — the one beside the prompt bar — then
    // `:q` really does quit.
    while app.workspace.pane_count() > 2 {
        assert!(!app.window_command_for_test(crate::keymap::WindowCmd::Quit));
    }
    assert!(
        app.window_command_for_test(crate::keymap::WindowCmd::Quit),
        "the last window's `:q` exits"
    );
}

/// `[tui] splitbelow` / `splitright` flip where the new pane lands — vim's
/// options, with vim's defaults (above / left).
#[test]
fn split_placement_follows_the_config() {
    let frame = Rect::new(0, 0, 100, 40);
    for (below, right) in [(false, false), (true, true)] {
        let mut cfg = Config::default();
        cfg.tui.splitbelow = below;
        cfg.tui.splitright = right;
        for dir in [SplitDir::Horizontal, SplitDir::Vertical] {
            let mut app = test_app_with(cfg.clone());
            app.set_focus(PaneKind::Output);
            let host = app.workspace.focused();
            let before = app.workspace.frame_layout(frame).pane(host).unwrap().rect;

            app.split_command("", dir);
            let layout = app.workspace.frame_layout(frame);
            let new = layout.pane(app.workspace.focused()).unwrap().rect;

            let after_side = match dir {
                SplitDir::Horizontal => new.y > before.y,
                SplitDir::Vertical => new.x > before.x,
            };
            let want_after = match dir {
                SplitDir::Horizontal => below,
                SplitDir::Vertical => right,
            };
            assert_eq!(
                after_side, want_after,
                "splitbelow={below} splitright={right} {dir:?}: new pane on the wrong side"
            );
        }
    }
}

/// vim's `{count}<C-w>w` jumps to window {count}; a bare `<C-w>w` cycles.
///
/// The two used to be indistinguishable: the keymap collapsed "no count" into
/// `1` with `.max(1)`, and the outcome's own doc comment admitted the result —
/// "focus ignores it". A dropped count with a disclaimer is still a dropped
/// count, and it made pane numbering unexpressible.
#[test]
fn a_counted_window_cycle_jumps_to_that_pane_number() {
    let frame = Rect::new(0, 0, 120, 40);
    let mut app = test_app();
    // Four panes, so a jump and a cycle cannot coincide.
    app.split_command("output", SplitDir::Vertical);
    app.split_command("output", SplitDir::Horizontal);
    let numbered: Vec<(usize, PaneKind)> = app
        .workspace
        .frame_layout(frame)
        .panes
        .iter()
        .map(|p| (p.number, p.kind))
        .collect();
    assert!(numbered.len() >= 4, "several panes: {numbered:?}");

    // Every number on screen is reachable by typing it.
    for (n, _) in &numbered {
        assert!(!app.apply_window(WindowCmd::FocusCycle, Some(*n), 120, 40));
        let now = app.workspace.frame_layout(frame);
        let focused = now.panes.iter().find(|p| p.focused).expect("one focused");
        assert_eq!(
            focused.number, *n,
            "`{n}<C-w>w` landed on {}",
            focused.number
        );
    }

    // A bare `<C-w>w` still cycles rather than jumping to pane 1.
    assert!(!app.apply_window(WindowCmd::FocusCycle, Some(1), 120, 40));
    assert!(!app.apply_window(WindowCmd::FocusCycle, None, 120, 40));
    let after = app.workspace.frame_layout(frame);
    let focused = after.panes.iter().find(|p| p.focused).unwrap();
    assert_eq!(focused.number, 2, "cycle moved on from pane 1");

    // A number nobody can see is refused out loud, not silently ignored.
    app.notices.clear();
    assert!(!app.apply_window(WindowCmd::FocusCycle, Some(99), 120, 40));
    assert!(app.notice().is_some(), "no such pane says so");
}

/// Repointing a pane forgets where it was looking, not just where its cursor
/// was — a stale `follow_tail == false` means a streaming reply never appears.
#[test]
fn repointing_a_pane_resets_its_scroll() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 12);
    let long = app.conversation().episode().id;
    app.reset_foreground();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    app.open_by_hash(&long.to_hex()[..8]);
    app.sync_chat();
    app.pane_chat_mut(pane).unwrap().place_cursor(0, 0);
    app.follow_pane_cursor(pane, (80, 10));
    assert!(
        !app.pane_viewport(pane).is_following_tail(),
        "premise: scrolled back"
    );

    app.new_session_command();

    assert!(
        app.pane_viewport(pane).is_following_tail(),
        "a fresh session in this pane starts at the live end"
    );
}

/// **`:new` makes a transcript pane when there is none.**
///
/// It refused — "no transcript pane to open it in" — which is a dead end the
/// user kept reaching: `:only` on a viewer or on the prompt leaves no
/// transcript, and then the one command whose job is "give me a new session"
/// cannot run. Recovery was four steps (`:sp output`, `<C-w>k`, `:hide`,
/// `<C-w>=`) and the refusal named only the first.
#[test]
fn new_creates_a_transcript_pane_when_there_is_none() {
    let mut app = test_app();
    app.open_graph_command();
    app.only_command(false); // a graph and a prompt, no transcript
    assert!(
        app.workspace.first_of(PaneKind::Output).is_none(),
        "premise: nowhere to put a session, which is where `:new` gave up"
    );

    app.dispatch_command_for_test("new");

    assert!(
        app.workspace.first_of(PaneKind::Output).is_some(),
        "`:new` must make somewhere to put it rather than refusing"
    );
}
