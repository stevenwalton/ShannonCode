//! Which session a command acts on and lands in — the aim, wherever you stand.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

// ---------------------------------------------------------------------------
// P4 — the prompt targets a buffer, and it is VISIBLE.
// ---------------------------------------------------------------------------

/// Standing in a transcript pane aims the prompt at it, and `<Enter>` commits
/// there.
///
/// The feature the buffer model exists for. Before it, the prompt committed to
/// `current`, which only `:open`/`:new` moved — so with two transcript panes
/// open there was no gesture for "talk to that one" short of retyping its hash.
#[tokio::test]
async fn standing_in_a_transcript_pane_aims_the_prompt_at_it() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    app.set_focus(PaneKind::Output); // the prompt is focused at launch
    app.open_by_hash(&a.to_hex()[..8]);
    let a_pane = app.workspace.focused();
    let a_buffer = app.current;

    // A second transcript pane, pointed at B.
    app.split_command("output", SplitDir::Vertical);
    let b_pane = app.workspace.focused();
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.current;
    assert_ne!(a_buffer, b_buffer, "two sessions, two buffers");

    // Walk back to A's pane and type: it goes to A.
    app.focus_pane(a_pane);
    assert_eq!(
        app.prompt_target(),
        a_buffer,
        "aimed where you are standing"
    );
    app.submit("for A");
    let a_shown = app.session_of(a_buffer).unwrap().output.lines().join("\n");
    assert!(a_shown.contains("for A"), "A got it: {a_shown}");
    let b_shown = app.session_of(b_buffer).unwrap().output.lines().join("\n");
    assert!(!b_shown.contains("for A"), "B did not: {b_shown}");

    // And back again.
    app.feed_stream(StreamMsg::Complete);
    app.focus_pane(b_pane);
    assert_eq!(app.prompt_target(), b_buffer);
}

/// The aim is **drawn**, on both halves of the pair.
///
/// This is what makes aiming-by-focus safe rather than a way to send a message
/// to the wrong conversation: you see where it is going before you commit.
/// Asserted on the border accent AND on the title, because colour alone is the
/// one channel a colourblind user may not have.
#[tokio::test]
async fn the_prompts_target_is_visible_on_both_halves_of_the_pair() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    app.set_focus(PaneKind::Output); // the prompt is focused at launch
    app.open_by_hash(&a.to_hex()[..8]);
    let a_pane = app.workspace.focused();

    // One transcript pane: nothing to disambiguate, so no id in the title.
    assert_eq!(prompt_title(&app), " prompt ", "no noise when there is one");

    app.split_command("output", SplitDir::Vertical);
    let b_pane = app.workspace.focused();
    app.open_by_hash(&b.to_hex()[..8]);

    assert!(
        app.is_prompt_target(b_pane) && !app.is_prompt_target(a_pane),
        "the accent is on the pane the prompt commits to"
    );
    assert!(
        prompt_title(&app).contains(&b.short_hex()),
        "and the prompt names it: {}",
        prompt_title(&app)
    );

    app.focus_pane(a_pane);
    assert!(
        app.is_prompt_target(a_pane) && !app.is_prompt_target(b_pane),
        "the accent follows the aim"
    );
    assert!(
        prompt_title(&app).contains(&a.short_hex()),
        "so does the title: {}",
        prompt_title(&app)
    );
}

/// **Reading is not aiming.** A pane lent to `:graph` or `:help` does not
/// re-address the prompt when you step into it.
///
/// Without this, walking through panes to consult the manual would silently
/// point your next sentence at whatever session that pane happened to hold —
/// and the manual has no session, so the aim would be nonsense either way.
#[tokio::test]
async fn stepping_into_a_viewer_does_not_re_aim_the_prompt() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    let a = app.conversation().episode().id;
    let b = add_persisted_session(&mut app, "B0");
    app.set_focus(PaneKind::Output); // the prompt is focused at launch
    app.open_by_hash(&a.to_hex()[..8]);
    let a_pane = app.workspace.focused();
    let a_buffer = app.current;

    // A second pane on B, lent to the manual.
    app.split_command("output", SplitDir::Vertical);
    let b_pane = app.workspace.focused();
    app.open_by_hash(&b.to_hex()[..8]);
    app.open_help_command(None);
    assert_eq!(
        app.workspace.kind_of(b_pane),
        Some(PaneKind::Page(PageKind::Help))
    );

    app.focus_pane(a_pane);
    assert_eq!(app.prompt_target(), a_buffer, "aimed at A");
    app.focus_pane(b_pane); // step into the manual to read it

    assert_eq!(
        app.prompt_target(),
        a_buffer,
        "still aimed at A — you went there to read, not to talk"
    );
}

/// A target whose buffer is gone falls back rather than swallowing the turn.
#[tokio::test]
async fn a_prompt_aimed_at_a_closed_buffer_falls_back_to_the_foreground() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 1);
    // Aim at a buffer id that was never opened — the shape a closed buffer
    // leaves behind once `:bd` exists. Set on the aim itself: routing this
    // through the prompt pane's slot made the test vacuous the moment the aim
    // stopped living there, since an unread binding leaves the target at
    // `current` and the assertion passes for the wrong reason.
    app.aim_prompt_at(BufferId(9999));

    assert_eq!(
        app.prompt_target(),
        app.current,
        "falls back to the foreground rather than aiming at nothing"
    );
    app.submit("still sends");
    assert!(
        app.output().lines().join("\n").contains("still sends"),
        "and the turn is not swallowed"
    );
}

/// `:new` mints a **buffer**; it does not empty the one the pane held.
///
/// A buffer is an occurrence of a session, so a second pane parked on the old
/// one keeps showing it. Emptying in place would drag every pane bound to that
/// buffer along — the "it changed both panes" bug, wearing a session.
#[test]
fn new_leaves_a_second_pane_on_the_old_session_alone() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    let old = app.conversation().episode().id;
    // Two transcript panes on the one session — each born on it (V5.3 (2));
    // this bound the first by hand, through a landing's door into a window
    // already on the session, a state `place` never makes (review stop 10).
    app.set_focus(PaneKind::Output);
    let other = app.workspace.focused();
    app.split_command("output", SplitDir::Vertical);
    let here = app.workspace.focused();
    assert_ne!(here, other, "two distinct transcript panes");
    assert_eq!(
        app.live_session(other),
        Some(app.current),
        "premise: both on it"
    );

    app.new_session_command();

    assert_ne!(
        app.pane_session(here).conversation.episode().id,
        old,
        "the pane you ran it from moved to the fresh session"
    );
    assert_eq!(
        app.pane_session(other).conversation.episode().id,
        old,
        "and the pane parked on the old one still shows it"
    );
}

// ---- two sessions, side by side -------------------------------------------
// The workflow the buffer model exists for, and the one Steven's 2026-08-06
// dogfooding pass could not reach: `:vsp` then `:new` is the first thing anyone
// tries, and it used to do nothing you could see.

/// **A click inverts the geometry of the pane you clicked in.**
///
/// `place_cursor_at_mouse` read `self.output()` — the FOREGROUND buffer's
/// transcript — and wrote the result into the FOCUSED pane's cursor. With two
/// transcript panes on different sessions those are different objects, so a
/// click in the second pane was inverted against the first session's text: on a
/// long neighbour and a short local one, clicking the top row put the cursor on
/// the last line instead. Every sibling had already been converted to per-pane
/// addressing (`follow_chat_cursor`, `sync_chat`, `scroll_pane`,
/// `draw_transcript`); this one still compiled with the old meaning, which is
/// the dangerous shape ([[feedback_relocating_state_fans_out]], Reviewer A).
///
/// Mouse input had **no test at all** before this one.
#[test]
fn a_click_inverts_the_focused_panes_transcript_not_the_foregrounds() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    // A long transcript in the first pane…
    for i in 0..40 {
        app.output_mut().begin_turn(&format!("neighbour line {i}"));
        app.output_mut().end_turn();
    }
    app.split_command("", SplitDir::Vertical);
    let here = app.workspace.focused();
    app.new_session_command(); // …and a short one in the second
    app.output_mut().begin_turn("mine 0");
    app.output_mut().end_turn();
    app.output_mut().begin_turn("mine 1");
    app.output_mut().end_turn();

    let neighbour = app
        .session_panes(Over::ThisTab)
        .find(|&p| p != here)
        .expect("two transcript panes");
    // **Stand in the pane that is NOT the foreground.** That is the whole
    // point: `:new` leaves the pane you are in as both, so a click there
    // cannot tell `output()` from `pane_session(focused).output`. Step across
    // to the long neighbour and the two disagree.
    app.workspace.focus_pane(neighbour);
    assert_ne!(
        app.pane_session(neighbour).id,
        app.current,
        "premise: the focused pane is not the foreground"
    );
    let foreground = app.buffer().output.lines().len();
    let theirs = app.pane_session(neighbour).output.lines().len();
    assert!(
        theirs > foreground + 20,
        "premise: the pane I am in is much longer than the foreground \
         ({theirs} vs {foreground}), so inverting against the wrong one lands \
         somewhere this one never could"
    );

    let area = Rect::new(0, 0, 80, 24);
    let layout = app.workspace.frame_layout(area);
    let rect = layout.pane(neighbour).expect("on screen").rect;
    // The first content row of the pane the user is standing in. It is
    // scrolled to the tail, so that row is deep into the document.
    // The cursor indexes the pane's mirror of the transcript, so it has to
    // exist — `draw` does this via `sync_chat_if_dirty`.
    app.sync_chat();
    app.place_cursor_at_mouse_for_test(area, Position::new(rect.x + 2, rect.y + 1));

    assert!(
        app.pane_chat(neighbour).cursor().0 > foreground,
        "clicking in a long transcript must land inside IT, not at a line the \
         short foreground clamped it to (line {})",
        app.pane_chat(neighbour).cursor().0
    );
}

/// `:new` in a split gives that pane its **own** session.
///
/// `:vsp` inherits the pane's session (vim: a split is a second window on one
/// buffer), so both panes start on the same blank session. `:new` then had to
/// choose between the anti-litter rule ("a blank session *is* a new one, stay
/// put") and doing something visible. Staying put is only invisible while ONE
/// pane shows the session; with two it is a dead command, and it left both panes
/// pinned to one buffer so every later reply appeared in both.
#[test]
fn new_in_a_split_gives_that_pane_its_own_session() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    let fresh = app.workspace.focused();
    let sibling = app
        .session_panes(Over::ThisTab)
        .find(|&p| p != fresh)
        .expect("two transcript panes");
    assert_eq!(
        app.pane_session(fresh).id,
        app.pane_session(sibling).id,
        "the split starts on the session it split (vim)"
    );

    app.new_session_command();

    assert_ne!(
        app.pane_session(fresh).conversation.episode().id,
        app.pane_session(sibling).conversation.episode().id,
        "`:new` gave the focused pane a session of its own"
    );
    // And the aim is legible: exactly one pane wears the accent.
    assert!(app.is_prompt_target(fresh), "the prompt follows `:new`");
    assert!(
        !app.is_prompt_target(sibling),
        "the sibling is not where the next message goes"
    );
}

/// …and the anti-litter rule survives where it was actually aimed: one pane on a
/// blank session, `:new` stays put rather than minting an empty episode per
/// keypress.
#[test]
fn new_on_a_blank_session_alone_stays_put() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let before = app.conversation().episode().id;
    let buffers = app.buffers.sessions().count();

    app.new_session_command();

    assert_eq!(
        app.conversation().episode().id,
        before,
        "still the same blank session"
    );
    assert_eq!(
        app.buffers.sessions().count(),
        buffers,
        "and no buffer was minted"
    );
}

// `every_prompt_window_agrees_about_the_aim` stood here: every prompt window
// one composition, so one aim, after a `prompt_pane()` that answered with the
// FIRST slot drew a focused second window's title for a binding it did not
// own. Each prompt window owns its draft and its aim since 10-26 (Steven:
// "Both change when I change my aim where only the last should"), so the
// rule it held is the bug; `tests/prompts.rs` holds the one that replaced it.

/// With a single transcript pane the aim has nothing to distinguish, so **both**
/// channels stay quiet. They are two statements of one fact and used to disagree
/// — the title suppressed itself and the accent did not.
#[test]
fn the_aim_is_silent_when_there_is_nothing_to_distinguish() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let only = app.workspace.focused();
    app.new_session_command(); // binds the pane, which used to light it up

    assert_eq!(prompt_title(&app), " prompt ", "the title says nothing");
    assert!(
        !app.is_prompt_target(only),
        "and neither does the accent — one pane, nothing to choose between"
    );
}

// ---- a transcript pane is born on its session (V5.3 (2)) --------------------
// These were "an UNBOUND pane is showing the foreground" (Reviewer B,
// 2026-08-06): a fresh `:vs` was unbound, read as the foreground, and the
// raw→resolved fixes had no test. Panes are born bound now, so the unbound
// state is unreachable; each guards what it was for, in a state production
// makes (V5.3 design review, finding 5: two forced `bind(…, None)`).

/// **A pane made across the top is born on the session it was made for** —
/// `:b N` from the prompt bar with the aim in no pane (V5.3 design review,
/// P2). Born unbound, it read as the foreground, and `show_buffer_in` filed
/// that as a session the window had left: `<C-^>` there showed a session it
/// had never shown.
#[test]
fn a_pane_made_for_a_session_is_born_on_it() {
    let mut app = test_app();
    app.conversation_mut().begin_turn("first");
    app.conversation_mut().push_chunk("a reply");
    app.conversation_mut().commit_turn();
    let first = app.current;
    app.set_focus(PaneKind::Output);
    app.new_session_command();
    let second = app.current;
    assert_ne!(first, second, "premise: two sessions");
    // Aimed at the first, which no pane shows: `:b N` from the prompt bar
    // has no window to use, and makes one across the top.
    app.set_focus(PaneKind::Prompt);
    app.aim_prompt_at(first);
    assert!(app.aim_window().is_none(), "premise: the aim is in no pane");
    let before = app.workspace.pane_ids();
    app.dispatch_command_for_test(&format!("b{}", first.number()));
    let made: Vec<_> = app
        .workspace
        .pane_ids()
        .difference(&before)
        .copied()
        .collect();
    let [pane] = made[..] else {
        panic!("one pane made: {made:?}");
    };
    assert_eq!(app.pane_session(pane).id, first, "it shows the session");
    assert_eq!(
        app.window_of(pane).map_or(0, |v| v.history.len()),
        0,
        "and holds no session it never showed"
    );
}

/// **`:new` with no pane to stand in stays on a blank aim** — the
/// anti-litter rule (`blank_buffer`) where no window holds the session: from
/// the prompt bar with the only transcript closed, the blank session is the
/// one you are on, and a pane is made for it rather than a second blank.
#[test]
fn new_with_no_window_reuses_the_blank_aim() {
    let mut app = test_app();
    let blank = app.current;
    app.set_focus(PaneKind::Output);
    app.dispatch_command_for_test("close");
    assert!(
        !app.workspace.has(PaneKind::Output),
        "premise: no transcript"
    );
    app.set_focus(PaneKind::Prompt);
    let buffers = app.buffers.sessions().count();
    app.new_session_command();
    assert_eq!(
        app.buffers.sessions().count(),
        buffers,
        "no second blank session"
    );
    let pane = app
        .workspace
        .panes_of(PaneKind::Output)
        .next()
        .expect("a pane made");
    assert_eq!(app.pane_session(pane).id, blank, "on the blank one");
}

/// **Both halves of a split transcript wear the accent** — born on the
/// session it was split from, which is what the prompt commits to.
#[test]
fn a_split_transcript_is_born_on_its_session_and_wears_the_accent() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    assert_eq!(panes.len(), 2);
    for &p in &panes {
        assert_eq!(
            app.live_session(p),
            Some(app.current),
            "born on the session"
        );
        assert!(
            app.is_prompt_target(p),
            "both show what the prompt commits to"
        );
    }
}

/// **Focusing a pane aims the prompt at what it shows** — a third pane split
/// from the foreground's, born on it. The aim must start somewhere OTHER
/// than the foreground, or this asserts nothing (its first version passed
/// under mutation so).
#[test]
fn focusing_a_pane_aims_at_what_it_shows() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let first = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    app.new_session_command(); // this pane gets its own session; the foreground moves
    let foreground = app.current;
    // Split from the foreground's pane: born on the foreground.
    app.split_command("", SplitDir::Vertical);
    let third = app.workspace.focused();
    app.focus_pane(first);
    assert_ne!(
        app.prompt_target(),
        foreground,
        "premise: the aim is off the foreground"
    );
    assert_eq!(app.live_session(third), Some(foreground), "premise");
    app.focus_pane(third);

    assert_eq!(
        app.prompt_target(),
        foreground,
        "we aimed at the session the pane is showing"
    );
}

/// The automatic re-aim must not fire while a pane shows the target.
///
/// **And that pane must not come first in ring order.** It did, in this
/// test's first shape (a split puts the new pane before), and the fallback a
/// broken guard reaches picks the first transcript pane — the same pane, the
/// same target — so reverting the fix passed (review 2026-10-15, A F2, M12).
/// The premise line on ring order is what makes this mean anything. (The
/// target was shown by an UNBOUND pane, which no pane is now.)
#[test]
fn no_spurious_reaim_while_a_later_pane_shows_the_target() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let right = app.workspace.focused();
    app.split_command("", SplitDir::Vertical);
    let left = app.workspace.focused();
    app.new_session_command();
    let target = app.current;
    let other = app.pane_session(right).id;
    assert_ne!(target, other, "premise: two sessions");

    // The first pane shows the OTHER session; the target is shown only by a
    // later pane — each window's session pointed so, as `:b N` would.
    app.show_buffer_in(left, other);
    app.show_buffer_in(right, target);
    let order = app.workspace.ring_order();
    let at = |p| order.iter().position(|&q| q == p);
    assert!(
        at(left) < at(right),
        "premise: the pane showing another session comes first"
    );
    app.aim_prompt_at(target);

    // As if the pane it went to had closed: the re-aim's question is then
    // whether a pane still shows it.
    app.reaim_prompts_whose_pane_went(&[target]);

    assert_eq!(
        app.prompt_target(),
        target,
        "a session on screen is not gone just because nothing bound its pane"
    );
    assert!(
        app.notice().is_none(),
        "and nothing was announced: {:?}",
        app.notice()
    );
}

/// **`:open` the foreground from a pane showing another session aims the
/// prompt there** — `set_foreground` aims before its "already in front"
/// return, so the pane you typed in and the aim agree. Deleting that order
/// passed the suite (review 2026-10-15, A F2, M14).
#[test]
fn open_the_foreground_from_another_pane_aims_there() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, _b, sibling, _focused) = app_with_two_sessions(dir.path());
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    app.focus_pane(sibling);
    assert_ne!(
        app.prompt_target(),
        a_buffer,
        "premise: aimed off the foreground"
    );
    app.open_by_hash(&a.to_hex()[..8]);
    assert_eq!(
        app.pane_session(sibling).id,
        a_buffer,
        "the pane now shows A"
    );
    assert_eq!(app.prompt_target(), a_buffer, "and the prompt aims at it");
}

/// **The title names the aim when a viewer has borrowed the only
/// transcript** — `aim_needs_saying`'s second clause: no pane shows the aim,
/// so the title is the one channel left. Deletable with the suite green until
/// now (review 2026-10-15, A F2, M5).
#[test]
fn the_title_names_the_aim_when_a_viewer_borrowed_the_only_transcript() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.open_graph_command();
    assert_eq!(app.session_panes(Over::ThisTab).count(), 1, "premise");
    assert!(
        app.aim_pane().is_none(),
        "premise: nothing visibly shows the aim"
    );
    assert!(prompt_title(&app).contains('→'), "{:?}", prompt_title(&app));
}

/// **Two prompt windows over one transcript each name their aim** —
/// `aim_needs_saying`'s third clause: with two prompts, which goes where is
/// the question, each its own aim since 10-26 (`tests/prompts.rs`).
/// Deletable with the suite green until now (review 2026-10-15, A F2, M6).
#[test]
fn two_prompt_windows_name_their_aims() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    app.split_command("", SplitDir::Vertical);
    assert_eq!(
        app.workspace.panes_of(PaneKind::Prompt).count(),
        2,
        "premise"
    );
    assert_eq!(app.session_panes(Over::ThisTab).count(), 1, "premise");
    let transcript = app.session_panes(Over::ThisTab).next().unwrap();
    assert!(prompt_title(&app).contains('→'), "{:?}", prompt_title(&app));
    assert!(app.is_prompt_target(transcript), "and the accent agrees");
}

/// **A viewer lent over a transcript still names that transcript's session**
/// — `pane_session`'s lent case (`about_the_aim`'s one exception, before
/// review stop 10 folded it in). Nothing held it: answering "about the
/// aim" for every viewer left the whole suite green (mutation, fixing B F3).
/// `:Send <pane>` of the borrowed pane is REFUSED since review stop 10 (A 2)
/// — it shows the viewer, and sending through it read what the screen does
/// not show — but the refusal names A's buffer, the session under it, which
/// is this exception's answer: without it the sentence named the aimed B.
#[test]
fn a_viewer_lent_over_a_transcript_still_names_its_session() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (left, right) = (panes[0], panes[1]);
    app.focus_pane(left);
    app.open_by_hash(&a.to_hex()[..8]);
    app.focus_pane(right);
    app.open_by_hash(&b.to_hex()[..8]);
    app.focus_pane(left);
    app.dispatch_command_for_test("graph");
    assert!(
        app.window_of(left).is_some_and(Window::has_beneath),
        "premise: A's pane is lent to the graph"
    );
    app.focus_pane(right);
    assert_eq!(
        app.prompt_target(),
        app.pane_session(right).id,
        "premise: aimed at B"
    );

    let n = 1 + app
        .workspace
        .ring_order()
        .iter()
        .position(|&p| p == left)
        .expect("in the ring");
    assert_eq!(
        app.pane_session(left).conversation.episode().id,
        a,
        "the borrowed pane names A's session"
    );
    let a_buffer = app.pane_session(left).id;
    let refused = app
        .send_target(&n.to_string())
        .expect_err("a covered pane is refused");
    assert!(
        refused.contains(&format!("`:Send b{}`", a_buffer.number())),
        "and the refusal names the session under it, A's: {refused}"
    );
}

/// **`:Send` of a pane a viewer covers is refused in both spellings, and so
/// is its mark** — the pane shows the viewer; the refusal names the session
/// under it. `:Send 2` refused it as "nothing marked", `:Send 2,b2` sent to
/// the session under the viewer, and `<C-w><Space>` set a mark that was
/// never drawn (review stop 10, A 2: one verb, two behaviours, chosen by the
/// other token).
#[tokio::test]
async fn a_covered_pane_is_refused_by_every_spelling_of_send() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let b2 = app.pane_session(app.workspace.focused()).id;
    let lent = (app
        .session_panes(Over::ThisTab)
        .collect::<Vec<_>>()
        .into_iter())
    .find(|&p| app.pane_session(p).id != b2)
    .expect("the other pane");
    let under = app.pane_session(lent).id;
    app.focus_pane(lent);
    app.dispatch_command_for_test("help");
    assert!(
        app.window_of(lent).is_some_and(Window::has_beneath),
        "premise: lent"
    );
    let n = 1
        + (app.workspace.ring_order().iter())
            .position(|&p| p == lent)
            .unwrap();
    let says_under = format!("`:Send b{}`", under.number());

    for spelling in [n.to_string(), format!("{n},b{}", b2.number())] {
        app.prompt_mut().insert_pasted("hi");
        app.send_command(&spelling);
        let notice = app.notice().unwrap_or_default();
        assert!(notice.contains(&says_under), ":Send {spelling}: {notice:?}");
        assert!(
            app.job_episodes().is_empty(),
            ":Send {spelling} sent nothing"
        );
    }

    app.focus_pane(lent);
    app.window_command_for_test(crate::keymap::WindowCmd::ToggleMark);
    assert!(!app.workspace.is_marked(lent), "not marked");
    assert!(
        app.notice().is_some_and(|n| n.contains(&says_under)),
        "and said why: {:?}",
        app.notice()
    );
}

/// **`<C-w><Space>` marks only a pane that owns a session** — a mark on a
/// tree in its own pane, or on the prompt, names something `:Send` has no
/// way to send to, and is refused, said. The workspace asked this itself
/// until V4b moved "which panes own a session" to the app's window history
/// (10-25), and no test held it in either home.
#[test]
fn only_a_sessions_pane_takes_a_mark() {
    use crate::keymap::WindowCmd;
    let mut app = test_app();
    let prompt = app.workspace.focused();
    assert_eq!(app.focus(), PaneKind::Prompt, "premise: in the prompt");
    app.window_command_for_test(WindowCmd::ToggleMark);
    assert!(!app.workspace.is_marked(prompt), "no mark on the prompt");
    let transcript = app
        .session_panes(Over::ThisTab)
        .next()
        .expect("a transcript");
    app.focus_pane(transcript);
    app.window_command_for_test(WindowCmd::ToggleMark);
    assert!(app.workspace.is_marked(transcript), "a transcript marks");
    app.window_command_for_test(WindowCmd::ToggleMark);
    assert!(
        !app.workspace.is_marked(transcript),
        "premise: unmarked again"
    );

    app.dispatch_command_for_test("sgraph");
    let tree = app.workspace.focused();
    assert_eq!(app.focus(), PaneKind::Graph, "premise: a tree of its own");
    app.window_command_for_test(WindowCmd::ToggleMark);
    assert!(!app.workspace.is_marked(tree), "no mark on the tree");
    assert!(
        (app.notice()).is_some_and(|n| n.contains("only a transcript pane can be marked")),
        "said: {:?}",
        app.notice()
    );
}

/// **A transcript split from a lent viewer shows the aim too** (as the row
/// does, `standing_in_a_lent_viewer_the_row_describes_the_aim`, in
/// `status_row.rs`) — `split_command`
/// binds the newcomer to "the session that pane's row describes", and asked
/// `about_the_aim`, whose lent exception left it unbound: it showed
/// `current`, neither the session beneath nor the aim. `current` is put on
/// the session beneath here, so the old answer cannot pass for the new one.
#[test]
fn a_transcript_split_from_a_lent_viewer_shows_the_aim() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (left, right) = (panes[0], panes[1]);
    app.focus_pane(right);
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.prompt_target();
    app.focus_pane(left);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.prompt_target();
    app.dispatch_command_for_test("graph");
    app.focus_pane(right);
    app.focus_pane(left);
    assert_eq!(app.focus(), PaneKind::Graph, "premise: in the lent graph");
    assert_eq!(app.prompt_target(), b_buffer, "premise: aimed at B");
    assert_eq!(app.current, a_buffer, "premise: A, beneath, is in front");

    app.split_command("output", SplitDir::Horizontal);
    let newcomer = app.workspace.focused();
    assert_eq!(app.focus(), PaneKind::Output, "premise: a transcript");
    assert_eq!(
        app.pane_session(newcomer).id,
        b_buffer,
        "the aim, as the row says"
    );
}

/// **…and `:w` there saves the aim** — "save this session now"
/// (|write|) is a deictic, and a session command's deictic is the
/// aim (`target_conversation`; `:delete`, `:clone`, `:rename` already read
/// it). `:write` read `focused_buffer`, which a lent viewer keeps on the
/// session beneath it: standing in `:graph` over A, aimed at B, the row said
/// B and `:w` saved A.
#[test]
fn write_in_a_lent_viewer_saves_the_aim() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (left, right) = (panes[0], panes[1]);
    app.focus_pane(left);
    app.open_by_hash(&a.to_hex()[..8]);
    app.focus_pane(right);
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.prompt_target();
    app.focus_pane(left);
    app.dispatch_command_for_test("graph");
    app.focus_pane(right);
    app.focus_pane(left);
    assert_eq!(app.focus(), PaneKind::Graph, "premise: in the lent graph");
    assert_eq!(app.prompt_target(), b_buffer, "premise: aimed at B");
    let (a_tag, b_tag) = (app.session_tag(a), app.session_tag(b));
    assert!(
        !a_tag.contains(&b_tag) && !b_tag.contains(&a_tag),
        "premise: the tags tell A from B"
    );

    app.dispatch_command_for_test("w");
    let said = app.notice().unwrap_or_default();
    assert!(said.contains(&b_tag), "B saved: {said:?}");
    assert!(!said.contains(&a_tag), "not A: {said:?}");
}

/// **`:open <turn>` changes what you are aimed at, as vim's `:e` does**
/// (Steven, 10-16, D3) — the checkout in a buffer of its own, landing where
/// `:open <session>` lands, and what was shown stays a buffer. Standing in
/// `:graph` lent over A while aimed at B, it ADOPTED the checkout into A's
/// buffer: the session beneath the viewer, displaced, while every other
/// session verb acted on B (review stop 2, A F5).
#[test]
fn open_turn_from_a_lent_viewer_takes_its_own_buffer() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (left, right) = (panes[0], panes[1]);
    app.focus_pane(left);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.prompt_target();
    app.focus_pane(right);
    app.open_by_hash(&b.to_hex()[..8]);
    let b_buffer = app.prompt_target();
    app.focus_pane(left);
    app.dispatch_command_for_test("graph");
    app.focus_pane(right);
    app.focus_pane(left);
    assert_eq!(app.focus(), PaneKind::Graph, "premise: in the lent graph");
    assert_eq!(app.prompt_target(), b_buffer, "premise: aimed at B");

    let first = app.current_graph().full_turns(&a)[0];
    app.dispatch_command_for_test(&format!("open {}", &first.to_hex()[..8]));
    let holds = |app: &App, buffer| app.session_of(buffer).map(|b| b.conversation.episode().id);
    assert_eq!(
        holds(&app, a_buffer),
        Some(a),
        "A, beneath the viewer, is not displaced"
    );
    assert_eq!(holds(&app, b_buffer), Some(b), "nor B");
    let aimed = app.prompt_target();
    assert!(
        aimed != a_buffer && aimed != b_buffer,
        "the checkout is aimed at"
    );
    assert_eq!(
        app.aimed_buffer()
            .conversation
            .episode()
            .origin
            .map(|o| o.turn),
        Some(first),
        "a checkout of that turn"
    );
}

/// **Session commands read the aim, not `current`** (review 2026-10-15,
/// Reviewer A F7 and B F4 — their probes 5, 6 and 11).
///
/// `target_conversation`'s rule: a session command means the session on
/// screen, and `current` keeps only which episode `HEAD` names. Three readers
/// kept `current`: `:Edit` opened B's transcript, `:graph` landed on B's tip,
/// and a typed `:tool` was filed under B — standing in A's pane, aimed at A,
/// with B in front. The aim is set apart from `current` first, or every
/// assertion here passes by accident.
#[test]
fn session_commands_read_the_aim_not_current() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _, _) = app_with_two_sessions(dir.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (here, there) = (panes[0], panes[1]);
    app.focus_pane(here);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    app.focus_pane(there);
    app.open_by_hash(&b.to_hex()[..8]);
    app.focus_pane(here);
    assert_eq!(app.prompt_target(), a_buffer, "premise: aimed at A");
    assert_eq!(app.conversation().episode().id, b, "premise: B is in front");

    match app.dispatch_command("Edit") {
        CmdEffect::View { text, .. } => {
            assert!(
                text.contains("Hello 0") && !text.contains("B0"),
                ":Edit: {text}"
            );
        }
        _ => panic!(":Edit opened no viewer"),
    }

    app.open_graph_command();
    let view = app.graph_view().expect("a graph");
    assert_eq!(
        view.selected_episode(),
        Some(a),
        ":graph lands where you are"
    );
    assert_eq!(view.active(), Some(b), "and still flags HEAD's session");
    app.close_pane_command(Asked::Plain);

    app.dispatch_command("tool frobnicate --now");
    let call = app.tool_calls.last().expect("a typed :tool is filed");
    assert_eq!(call.buffer, a_buffer, "under the session you are aimed at");
}

/// **`:open <turn>` for a turn several sessions hold forks from the aimed
/// one** (review 2026-10-15, Reviewer A F7). Its tie-break preferred
/// `current`; with a clone of A in front, `:open <A's first turn>` typed in
/// A's pane forked from the clone — the same words, under the wrong parent in
/// `:graph`.
#[test]
fn open_turn_prefers_the_aimed_session_among_those_holding_it() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, _, _, _) = app_with_two_sessions(dir.path());
    let panes: Vec<_> = app.session_panes(Over::ThisTab).collect();
    let (here, there) = (panes[0], panes[1]);
    app.focus_pane(here);
    app.open_by_hash(&a.to_hex()[..8]);
    let a_buffer = app.current;
    let turn = app.conversation().episode().own_turns()[0];
    app.focus_pane(there);
    app.clone_by_hash(&a.to_hex()[..8]);
    let clone = app.pane_session(there).conversation.episode().id;
    assert_ne!(clone, a, "premise: a clone of A, beside it");
    app.open_by_hash(&clone.to_hex()[..8]);
    assert_eq!(
        app.conversation().episode().id,
        clone,
        "premise: the clone is in front"
    );
    // A turn of its own, so it is written and no longer a PURE clone: a pure
    // one anchors any checkout to its source (`persisted_anchor`), which made
    // the tie-break invisible here (equivalent mutant, found by running it).
    app.conversation_mut().begin_turn("C0");
    app.conversation_mut().push_chunk("its own reply");
    app.conversation_mut().commit_turn();
    app.persist_committed_turn();
    app.focus_pane(here);
    assert_eq!(app.prompt_target(), a_buffer, "premise: aimed at A");

    app.open_by_hash(&turn.to_hex()[..8]);

    let origin = app.pane_session(here).conversation.episode().origin;
    assert_eq!(
        origin.map(|o| o.episode),
        Some(a),
        "forked from the session you are aimed at"
    );
}

/// **Both aim channels are DRAWN** — the unfocused transcript the prompt
/// commits to wears `ui.border.aimed` while its sibling wears `ui.border`,
/// and the prompt's top border names the target. Read off the cells: the
/// predicates were tested and the drawing was not, so painting the aimed
/// border plain or drawing a constant ` prompt ` title left every test green
/// (review 2026-10-15, C F3). The aim is P4's safety mechanism, and §9 doubles
/// the traffic on it.
#[test]
fn both_aim_channels_are_drawn() {
    use crate::style::Ink;
    let dir = TempDir::new().unwrap();
    let (mut app, a, _b, first, second) = app_with_two_sessions(dir.path());
    app.open_by_hash(&a.to_hex()[..8]); // the second pane shows A
    app.focus_pane(first); // standing in the first aims the prompt at it
    let aimed = app.pane_session(first).id;
    assert_ne!(aimed, app.pane_session(second).id, "premise: two sessions");
    app.set_focus(PaneKind::Prompt);
    assert_eq!(
        app.prompt_target(),
        aimed,
        "premise: aimed at the first pane, which is not focused"
    );

    let (w, h) = (120, 30);
    let buf = frame_of(&mut app, w, h);
    let layout = app.workspace.frame_layout(Rect::new(0, 0, w, h));
    let rect = |pane| layout.pane(pane).expect("laid out").rect;
    let corner = |pane| buf[(rect(pane).x, rect(pane).y)].clone();
    let theme = app.theme();
    assert_ne!(
        theme.ui(Ink::BorderAimed).fg,
        theme.ui(Ink::Border).fg,
        "premise: the two borders differ"
    );
    assert_eq!(corner(first).symbol(), "┌", "premise: the corner");
    assert_eq!(
        corner(first).style().fg,
        theme.ui(Ink::BorderAimed).fg,
        "the aimed transcript wears the accent"
    );
    assert_eq!(
        corner(second).style().fg,
        theme.ui(Ink::Border).fg,
        "its sibling does not"
    );

    let prompt = app
        .workspace
        .panes_of(PaneKind::Prompt)
        .next()
        .expect("a prompt pane");
    let r = rect(prompt);
    let top: String = (r.x..r.x + r.width)
        .map(|x| buf[(x, r.y)].symbol())
        .collect();
    let handle = app
        .session_of(aimed)
        .expect("the aim is open")
        .conversation
        .episode()
        .handle();
    assert!(
        top.contains(&format!("→ {handle}")),
        "the prompt names its target: {top}"
    );
}

/// **`:open` on the session a pane shows is a no-op there** — no buffer
/// opened, and nothing filed in the window's history. An outcome, not a
/// path: since V4b `show_buffer_in` skips a window already on the buffer,
/// so `switch_session`'s early return and the entry path agree here. (It
/// was "even from an unbound pane", where they differed — the start's
/// state, which no pane is now: V5.3 (2).)
#[test]
fn reopening_the_session_a_pane_shows_is_a_noop() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 2);
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    assert_eq!(app.live_session(pane), Some(app.current), "premise");
    let id = app.conversation().episode().id;
    let buffers = app.buffers.sessions().count();

    app.switch_session(id);

    assert_eq!(
        app.buffers.sessions().count(),
        buffers,
        "no buffer was opened"
    );
    assert_eq!(
        app.window_of(pane).map_or(0, |v| v.history.len()),
        0,
        "and nothing filed as a session left"
    );
}

// ---- a command acts on, and lands in, the session you are pointing at ------
// Reviewer A, 2026-08-06. The previous round moved every session command's
// READS onto the aim (`target_conversation`) and left every WRITE on
// `self.current`, so with the two diverged a command quoted one session into
// another. Both halves have to name the same buffer.

/// `:continue` sends the follow-up to the session it read the partial from.
///
/// The sharp one: `reveal_foreground()` ran between resolving the request and
/// `submit()`, and revealing goes through `enter_session`, which RE-AIMS the
/// prompt. So the instruction — carrying 200 chars of the aimed session's
/// partial assistant reply — was sent into whichever session was foreground:
/// a billable cross-session write that also leaks one conversation into
/// another's context.
#[tokio::test]
async fn continue_resumes_the_session_you_are_aimed_at() {
    use shannon_core::StopReason;
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let a_pane = app.workspace.focused();

    // Session A: a truncated turn worth resuming.
    app.output_mut().begin_turn("write a poem");
    app.conversation_mut().begin_turn("write a poem");
    app.feed_stream(StreamMsg::Chunk("Roses are".into()));
    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);
    let a = app.pane_session(a_pane).id;

    // A second pane on its own session, which becomes the FOREGROUND. Then
    // walk back into A, so the aim is A while `current` is B.
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let b = app.current;
    assert_ne!(a, b, "premise: two sessions");
    app.focus_pane(a_pane);
    assert_eq!(
        app.prompt_target(),
        a,
        "premise: aimed at A, foreground is B"
    );

    app.continue_last_turn();

    let into_b = app.session_of(b).unwrap().conversation.messages().len();
    assert_eq!(into_b, 0, "B was not written to at all");
    let last = app
        .session_of(a)
        .unwrap()
        .conversation
        .messages()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last.role, shannon_core::Role::User);
    assert!(
        last.content.contains("Roses are"),
        "A's own partial, quoted back into A"
    );
    assert_eq!(app.prompt_target(), a, "and the aim did not move under us");
}

/// **`:continue` goes to the session it continues, marks or no marks** —
/// review 2026-10-15, Reviewer A F1, the CRITICAL. Marks stay on after a
/// `:Send 1,2`; `:continue` went through `submit`, whose marks win, so A's
/// cut-off reply was quoted into every MARKED session as a billed job, and A
/// got nothing unless it was marked itself. Marks are for your `<Enter>`.
#[tokio::test]
async fn continue_goes_to_its_own_session_past_the_marks() {
    use shannon_core::StopReason;
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let a_pane = app.workspace.focused();
    app.output_mut().begin_turn("write a poem");
    app.conversation_mut().begin_turn("write a poem");
    app.feed_stream(StreamMsg::Chunk("Roses are".into()));
    app.feed_stream(StreamMsg::Stopped(StopReason::MaxTokens));
    app.feed_stream(StreamMsg::Complete);
    let a = app.pane_session(a_pane).id;

    // B in its own pane, MARKED — as a `:Send 1,2` leaves it.
    app.split_command("", SplitDir::Vertical);
    app.new_session_command();
    let b_pane = app.workspace.focused();
    let b = app.pane_session(b_pane).id;
    assert_ne!(a, b, "premise: two sessions");
    assert!(app.workspace.set_mark(b_pane, true), "premise: B marked");
    app.focus_pane(a_pane);
    assert_eq!(app.prompt_target(), a, "premise: aimed at A");
    assert!(!app.workspace.is_marked(a_pane), "premise: A not marked");

    app.continue_last_turn();

    assert!(
        !app.generations
            .iter()
            .any(|g| matches!(g.work, Work::Job(_))),
        "no job went to the marked session"
    );
    let into_b = app.session_of(b).unwrap().conversation.messages().len();
    assert_eq!(into_b, 0, "B was not written to at all");
    let last = app
        .session_of(a)
        .unwrap()
        .conversation
        .messages()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last.role, shannon_core::Role::User);
    assert!(
        last.content.contains("Roses are"),
        "A's own partial, quoted back into A"
    );
    assert!(
        app.workspace.is_marked(b_pane),
        "the mark stays, for the next `<Enter>`"
    );
}

/// A composition lands in the pane you ran it from, and leaves its sibling be.
///
/// `:clone` read through the aim and wrote into `current`, so cloning from a
/// pane that was not the foreground put the clone into the OTHER pane's session
/// — the "welded together" symptom, re-entered through the compose family.
/// The aim and `current` must genuinely differ here or this asserts nothing.
#[tokio::test]
async fn a_clone_lands_in_the_pane_you_ran_it_from() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());
    app.open_by_hash(&a.to_hex()[..8]); // `focused` shows A, and A is the foreground
    app.focus_pane(sibling); // aim = B, foreground still A
    assert_ne!(
        app.prompt_target(),
        app.current,
        "premise: the aim is off the foreground"
    );

    app.clone_by_hash(&a.to_hex()[..8]);

    let here = app.pane_session(sibling).conversation.episode().id;
    assert_ne!(here, b, "the pane we ran it from now shows the clone");
    assert_ne!(here, a, "which is a fork, not A itself");
    assert_eq!(
        app.pane_session(focused).conversation.episode().id,
        a,
        "and the other pane still shows what it did"
    );
}

/// Same rule for `:open <turn-id>`, which is a different function reached by a
/// different id space — the pair `enter_session` was created to keep in step.
#[tokio::test]
async fn a_checkout_lands_in_the_pane_you_ran_it_from() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, sibling, focused) = app_with_two_sessions(dir.path());
    app.open_by_hash(&a.to_hex()[..8]); // `focused` shows A, A is the foreground
    app.focus_pane(sibling); // aim = B
    assert_ne!(
        app.prompt_target(),
        app.current,
        "premise: the aim is off the foreground"
    );
    let turn = app.current_graph().full_turns(&a)[0];

    app.open_by_hash(&turn.to_hex()[..8]);

    assert_ne!(
        app.pane_session(sibling).conversation.episode().id,
        b,
        "the checkout landed in the pane we ran it from"
    );
    assert_eq!(
        app.pane_session(focused).conversation.episode().id,
        a,
        "and the other pane still shows what it did"
    );
}

/// The status line beside the prompt describes the session the prompt is aimed
/// at — not whatever is merely the foreground. (Reviewer C: what the status
/// line reads had no test at all; it asserted `focused_buffer` until the row
/// read the aim itself, 10-16, and is drawn now.)
#[tokio::test]
async fn the_status_line_by_the_prompt_describes_the_aim() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, _b, sibling, _) = app_with_two_sessions(dir.path());
    app.open_by_hash(&a.to_hex()[..8]); // `focused` shows A, A is the foreground
    app.focus_pane(sibling); // aim moves to B; `current` stays A
    let aim = app.prompt_target();
    assert_ne!(aim, app.current, "premise: the aim is off the foreground");
    let lines = |buffer| app.session_of(buffer).unwrap().output.line_count();
    let (aim_lines, front) = (lines(aim), lines(app.current));
    assert_ne!(aim_lines, front, "premise: the counts tell them apart");

    app.set_focus(PaneKind::Prompt);

    let row = status_row(&mut app, "prompt", r"<$lines>");
    assert!(
        row.contains(&format!("<{aim_lines}>")),
        "standing in the prompt, the status line is about where the text goes: {row:?}"
    );
}

/// A prompt pane never inherits a session, so a later `:sp output` from it
/// cannot carry a stale one onto a real transcript. (Reviewer A2 showed the
/// workspace's filter was unguarded and *is* reachable through a second
/// split; since V5.3 (3c) the window holds its session, born with it or
/// without — a prompt's, without.) Restated (V5.3 verify, F6): the prompt's
/// having none is true by type now, so what is asked is the second split
/// its doc names — the transcript split from it is born on the aim.
#[test]
fn a_prompt_pane_never_carries_a_session_binding() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, _, sibling, _) = app_with_two_sessions(dir.path());
    // The aim off the foreground, so "the aim" and "whatever was current"
    // are two answers (as `the_status_line_by_the_prompt_describes_the_aim`).
    app.open_by_hash(&a.to_hex()[..8]);
    app.focus_pane(sibling);
    let aim = app.prompt_target();
    assert_ne!(aim, app.current, "premise: the aim is not the foreground");
    app.split_command("prompt", SplitDir::Vertical);
    let prompt = app.workspace.focused();
    assert_eq!(
        app.workspace.kind_of(prompt),
        Some(PaneKind::Prompt),
        "premise"
    );
    assert_eq!(app.live_session(prompt), None, "premise: a prompt has none");

    app.split_command("output", SplitDir::Vertical);
    let born = app.workspace.focused();
    assert_eq!(
        app.workspace.kind_of(born),
        Some(PaneKind::Output),
        "premise"
    );
    assert_eq!(
        app.live_session(born),
        Some(aim),
        "born on the session the prompt talks to — the prompt had none to give it"
    );
}

/// `session_panes` yields in RING order — the order the corner numbers show —
/// not creation order. Its doc claimed ring order from the day it was written
/// and iterated the slot vector; nothing noticed until `transcript_target`
/// started answering "the first pane showing the aim" with `.next()`, and
/// `:new` from the prompt landed in the pane that happened to be made first.
#[test]
fn session_panes_are_in_ring_order_not_creation_order() {
    use crate::workspace::NewPane;
    let mut app = test_app();
    let first_made = app
        .session_panes(Over::ThisTab)
        .next()
        .expect("a transcript pane");
    let session = app.live_session(first_made).expect("its session");
    // `Before` puts the newcomer ahead of it on screen, so the two orders differ.
    // Born on the session, as `:sp` is (`split_command`).
    let newer = (app.workspace)
        .split_pane(
            first_made,
            SplitDir::Vertical,
            NewPane::Before,
            Window::on(session),
        )
        .expect("split");

    let ring: Vec<_> = app.session_panes(Over::ThisTab).collect();

    assert_eq!(
        ring,
        vec![newer, first_made],
        "the pane drawn first comes first, whatever order they were created in"
    );
}

/// **`:reply` forks the session you are READING, into the pane you are reading
/// it in** — not into wherever the prompt happens to be aimed.
///
/// Two transcript panes on two sessions, prompt aimed at B, cursor in A's pane.
/// `:reply` reads A's cursor and A's episode, and `checkout_turn` adopted the
/// result into `prompt_target()`. `reply_command` computed the right buffer and
/// threw it away (`let buffer = …; let _ = buffer;`), which was the tell.
///
/// One rule, the visible one: everything acts on the pane you are in
/// (`[[feedback_predictable_from_screen]]`).
#[tokio::test]
async fn reply_forks_into_the_pane_you_are_standing_in() {
    let dir = TempDir::new().unwrap();
    let mut app = app_with_n_turns(dir.path(), 3);
    // The fixture leaves focus on the PROMPT, so reach for the transcript
    // explicitly — standing in the prompt, `:reply` refuses by design, and a
    // test that never noticed would be asserting about a refusal.
    // Same setup the other `:reply` tests use — it is what gives the pane an
    // `Output` whose lines map to turns.
    let trunk = app.conversation().episode().id;
    app.reset_foreground();
    app.set_focus(PaneKind::Output);
    let reading = app.workspace.focused();
    app.open_by_hash(&trunk.to_hex()[..8]);
    let session_a = app.pane_session(reading).id;

    // A second transcript pane, on a second session.
    app.split_command("", SplitDir::Horizontal);
    app.new_session_command();
    let session_b = app.current;
    assert_ne!(session_a, session_b, "premise: two different sessions");

    // Stand back in A's pane, aim the prompt at B, put the cursor on an early
    // turn, and reply.
    app.workspace.focus_pane(reading);
    app.aim_prompt_at(session_b);
    assert_eq!(
        app.pane_session(reading).id,
        session_a,
        "premise: the pane we are standing in still shows A"
    );
    // The chat mirror only tracks the transcript while its pane is focused, so
    // it has to be re-synced after moving back — otherwise the cursor indexes
    // stale lines and `:reply` refuses with "not part of a turn", which is a
    // refusal, not the behaviour under test.
    app.sync_chat();
    app.pane_chat_mut(reading).unwrap().place_cursor(0, 0);
    app.reply_command();
    assert!(
        app.notice().is_some_and(|n| n.contains("replying from")),
        "premise: the reply was accepted, not refused: {:?}",
        app.notice()
    );

    // In the pane being read, in a buffer of its own — as `:e` (D3): A stays
    // a buffer, B is untouched, and the fork reads from A.
    let fork = app.pane_session(reading);
    assert!(
        fork.id != session_a && fork.id != session_b,
        "the fork landed in the pane being read, not in the prompt's target"
    );
    assert_eq!(
        fork.conversation.episode().origin.map(|o| o.episode),
        Some(trunk),
        "a fork of A"
    );
    assert_eq!(
        app.session_of(session_a)
            .map(|b| b.conversation.episode().id),
        Some(trunk),
        "A is still a buffer, on A"
    );
    assert!(
        app.session_of(session_b).is_some(),
        "and the aimed session is untouched"
    );
}

/// **With the tree open, `:delete` deletes what `d` would delete.**
///
/// Reported from a drive with a screenshot: marks set, cursor on a row, and
/// `:delete` offered to remove a THIRD session — neither marked nor under the
/// cursor. It read the prompt's own session and never asked the graph, so one
/// destructive verb had two doors that answered differently, and the only place
/// the disagreement showed was the confirmation line.
#[test]
fn delete_with_the_tree_open_targets_the_graph_not_the_prompt() {
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _sibling, _focused) = app_with_two_sessions(dir.path());
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(a);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);
    let aimed = app.aimed_buffer().conversation.episode().id;
    assert_ne!(
        aimed, a,
        "premise: the prompt is aimed somewhere OTHER than the graph row, or \
         the two doors cannot be told apart"
    );

    app.dispatch_command_for_test("delete");

    match app.pending_confirm.front() {
        Some(ConfirmAction::Delete {
            episodes, turns, ..
        }) => {
            assert_eq!(
                episodes.as_slice(),
                &[a],
                "`:delete` must name the row the tree points at, not the prompt's {b:?}"
            );
            assert!(turns.is_empty(), "`:delete` names sessions, never turns");
        }
        other => panic!("expected a delete confirmation, got {other:?}"),
    }
}

/// **`:graph` takes over a pane from the prompt too, instead of splitting.**
///
/// Reported from a drive, with the repro: type into the prompt, then `:graph`,
/// and instead of the documented "look at the graph HERE" you get a three-way
/// stack — graph, transcript, prompt. From a transcript pane the same command
/// takes the pane over. One verb, two behaviours, chosen by which pane the
/// cursor happened to be in.
///
/// The cause is that `hosting_pane` asked for the FOCUSED pane and the prompt
/// bar can never host a viewer, so it fell straight through to making a new
/// one — silently doing what `:sg` exists to do, which is the other half of a
/// deliberately two-verb family.
///
/// Standing in the prompt is still pointing at something, and it is drawn: the
/// title bar reads `prompt → <id>`. So the viewer takes over the pane showing
/// what the prompt points at ([[feedback_predictable_from_screen]]).
#[test]
fn graph_from_the_prompt_takes_over_the_aimed_pane_rather_than_splitting() {
    let mut app = test_app();
    let transcript = app
        .workspace
        .first_of(PaneKind::Output)
        .expect("premise: a transcript pane to aim at");
    app.workspace.focus_kind(PaneKind::Prompt);
    assert_eq!(
        app.workspace.kind_of(app.workspace.focused()),
        Some(PaneKind::Prompt),
        "premise: standing in the prompt, which is where the repro starts"
    );

    app.open_graph_command();

    // Asserted on the pane's KIND, not on the pane count: `:graph` also opens
    // an inspector companion, so the count rises by one whether it borrowed or
    // split. Counting would have passed on the broken behaviour too.
    assert_eq!(
        app.workspace.kind_of(transcript),
        Some(PaneKind::Graph),
        "`:graph` must take over the pane the prompt is aimed at, not split"
    );
    assert!(
        app.workspace.first_of(PaneKind::Output).is_none(),
        "…so no transcript is left sitting beside a manufactured graph pane"
    );
    // …and `:q` gives it back, which is the half that makes borrowing safe.
    app.dispatch_command_for_test("q");
    assert_eq!(
        app.workspace.kind_of(transcript),
        Some(PaneKind::Output),
        "`:q` hands the transcript back"
    );
}

/// **No verb ignores a mark that is drawn on the screen.**
///
/// `GraphView::acts_on` calls itself "the one answer to 'what does this verb act
/// on?'" and names the six-site defect it closed. It had ONE consumer. `c`, `r`
/// and `e` read `selected_node`, so marking three sessions and pressing `c`
/// cloned the cursor row and silently ignored three highlighted rows. Four
/// reviewers found this independently, which is what a mark set that most verbs
/// ignore looks like from four angles ([[feedback_class_not_instance]]).
///
/// The refusal is the conservative half of a decision still open: netrw's `mc`
/// copies every marked file, so a marked `c` could reasonably clone them all.
/// What it must not do is what it did — draw three marks and quietly act on
/// one.
#[test]
fn a_single_session_verb_refuses_rather_than_ignoring_the_marks() {
    let k = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let dir = TempDir::new().unwrap();
    let (mut app, a, b, _sibling, _focused) = app_with_two_sessions(dir.path());
    let mut gv = GraphView::new(app.current_graph());
    gv.reveal_and_select_episode(a);
    open_graph_pane(&mut app);
    app.set_graph_view_for_test(gv);

    // Mark two distinct sessions with netrw's `mf`.
    app.graph_key_for_test(k(KeyCode::Char('m')));
    app.graph_key_for_test(k(KeyCode::Char('f')));
    let before = app.conversation().episode().id;
    app.graph_view_mut()
        .expect("the viewer is open")
        .reveal_and_select_episode(b);
    app.graph_key_for_test(k(KeyCode::Char('m')));
    app.graph_key_for_test(k(KeyCode::Char('f')));
    assert_eq!(
        app.graph_view().unwrap().marked_nodes().len(),
        2,
        "premise: two rows are marked and drawn as marked"
    );

    // `c` and the old `r` are commands now (`:clone`, `:return`) and `r`
    // renames; all four take one session, and all four are asked.
    let presses: [(&str, &str, Press); 6] = [
        (":clone", "clone", |app| {
            app.dispatch_command_for_test("clone");
        }),
        (":return", "return", |app| {
            app.dispatch_command_for_test("return");
        }),
        ("r", "rename", |app| {
            app.graph_key_for_test(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
        }),
        (":rename x", "rename", |app| {
            app.dispatch_command_for_test("rename x");
        }),
        (":merge", "merge", |app| {
            app.dispatch_command_for_test("merge");
        }),
        (":description x", "description", |app| {
            app.dispatch_command_for_test("description x");
        }),
    ];
    for (key, verb, press) in presses {
        app.notices.clear();
        press(&mut app);
        let said = app.notice().unwrap_or_default();
        assert!(
            said.contains(verb) && said.contains("marked"),
            "`{key}` must say it cannot act on 2 marks, got {said:?}"
        );
        assert_eq!(
            app.notice_kind(),
            Some(crate::app::NoticeKind::Refusal),
            "…as a refusal, because nothing happened"
        );
        assert_eq!(
            app.conversation().episode().id,
            before,
            "`{key}` must not quietly act on one of them"
        );
    }
}
