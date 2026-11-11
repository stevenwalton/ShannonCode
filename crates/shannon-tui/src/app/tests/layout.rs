//! Where panes go: the layout a split or a new window takes.
//!
//! Moved out of `tests.rs` verbatim at review stop 12's Tier 4 (10-27);
//! the harness stays there, reached by `use super::*`.

use super::*;

/// **A prompt window you split off hosts no view** (`PaneKind::dedicated`;
/// V5.3 design review, finding 1, probed) — a split prompt is a flexible
/// pane, and `:help` there took it, setting the prompt aside. The view
/// opens where it would from the prompt bar: in the aim's pane.
#[test]
fn a_split_prompt_window_hosts_no_view() {
    let mut app = test_app();
    app.set_focus(PaneKind::Prompt);
    app.split_command("", SplitDir::Vertical);
    let window = app.workspace.focused();
    assert_eq!(
        app.workspace.kind_of(window),
        Some(PaneKind::Prompt),
        "premise"
    );
    assert!(app.workspace.is_flexible(window), "premise: flexible");
    app.open_help_command(None);
    assert_eq!(app.workspace.kind_of(window), Some(PaneKind::Prompt));
    assert!(
        app.workspace.has(PaneKind::Page(PageKind::Help)),
        "the manual opened elsewhere"
    );
}

/// **`<C-w>+` works on a transcript in a side-by-side layout.**
///
/// The layout Steven built dogfooding: three transcripts from two `:vs`, prompt
/// below. A transcript's immediate parent is a *vertical* split, and `resize`
/// asked `dir_around(id) != Some(dir)` and returned — so `<C-w>+`, a height
/// change, matched nothing and did nothing, silently. Vim walks up to the
/// closest horizontal divider, which here is the one between the transcripts
/// and the prompt.
///
/// Asserted on the drawn geometry rather than on the sizing, because the sizing
/// is the mechanism and the height is the promise.
#[tokio::test]
async fn growing_a_transcript_works_when_its_parent_split_is_vertical() {
    let frame = Rect::new(0, 0, 90, 40);
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.split_command("", SplitDir::Vertical);
    let focused = app.workspace.focused();
    assert_eq!(
        app.workspace.frame_layout(frame).panes.len(),
        4,
        "premise: three transcripts and a prompt"
    );
    let height_of = |app: &App, pane| {
        app.workspace
            .frame_layout(frame)
            .panes
            .iter()
            .find(|p| p.id == pane)
            .map(|p| p.content_size().1)
            .expect("the pane is in the layout")
    };
    let before = height_of(&app, focused);

    let _ = app.apply_window(WindowCmd::ResizeGrow, Some(3), frame.width, frame.height);

    assert!(
        height_of(&app, focused) > before,
        "the transcript column grew: {before} -> {}",
        height_of(&app, focused)
    );
}

/// And `<C-w>=` evens three transcripts out rather than leaving two at a
/// quarter — the same layout, the other key Steven reached for.
#[tokio::test]
async fn equalize_evens_three_transcripts() {
    let frame = Rect::new(0, 0, 90, 40);
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    app.split_command("", SplitDir::Vertical);

    let _ = app.apply_window(WindowCmd::Equalize, None, frame.width, frame.height);

    let widths: Vec<u16> = app
        .workspace
        .frame_layout(frame)
        .panes
        .iter()
        .filter(|p| app.workspace.kind_of(p.id) == Some(PaneKind::Output))
        .map(|p| p.content_size().0)
        .collect();
    assert_eq!(widths.len(), 3, "premise: three transcripts");
    let (lo, hi) = (*widths.iter().min().unwrap(), *widths.iter().max().unwrap());
    assert!(
        hi < lo * 2,
        "no transcript is twice another — 25/25/50 was the bug: {widths:?}"
    );
}

/// **`:hide` gives a borrowed pane back rather than closing the window.**
///
/// Steven reached for `:hide` to close the manual, which is the right reflex —
/// vim's `:hide` means "stop showing this here". The literal vim action (close
/// the window) would be wrong for us: a viewer BORROWS a pane rather than
/// owning a window, so closing it would take the transcript with it.
#[tokio::test]
async fn hide_gives_a_borrowed_pane_back_to_its_transcript() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    let pane = app.workspace.focused();
    let panes = app.workspace.pane_count();
    app.open_help_command(None);
    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Page(PageKind::Help)),
        "premise: the manual borrowed the transcript's pane"
    );

    app.hide_command();

    assert_eq!(
        app.workspace.kind_of(pane),
        Some(PaneKind::Output),
        "the pane went back to the transcript it was showing"
    );
    assert_eq!(
        app.workspace.pane_count(),
        panes,
        "and the window is still there — closing it would have taken the \
         conversation with it"
    );
}

/// On a pane showing no viewer, `:hide` is the ordinary window close — as in
/// vim, where it always closes the current window.
#[tokio::test]
async fn hide_on_a_plain_pane_closes_it() {
    let mut app = test_app();
    app.set_focus(PaneKind::Output);
    app.split_command("", SplitDir::Vertical);
    let panes = app.workspace.pane_count();

    app.hide_command();

    assert_eq!(app.workspace.pane_count(), panes - 1, "the window closed");
}
