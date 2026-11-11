//! Tests for the workspace.
//!
//! Two halves. The first preserves the behaviour the old fixed-arrangement
//! version had, expressed against **observable geometry** rather than the
//! fields that used to hold it — a pane's position is where it lands, not a
//! `prompt_first: bool`. The second asserts the properties that make the model
//! general: they are written over *whatever panes exist*, and several run at
//! several different pane counts, because "correct at n = 2" is exactly the
//! failure mode this module was rebuilt to remove.

use super::*;
use ratatui::layout::Rect;

fn ws() -> Workspace {
    // 10-row prompt, 80-wide frame — the M1 defaults. Each slot's window is
    // its kind alone (`impl Pane for PaneKind`): the geometry asks a window
    // nothing else (which session a transcript shows is the app's window's).
    Workspace::new(10, PaneKind::Output, PaneKind::Prompt)
}

const FRAME: Rect = Rect {
    x: 0,
    y: 0,
    width: 80,
    height: 50,
};

/// The rect of the first pane showing `kind`.
fn rect_of(w: &Workspace, kind: PaneKind) -> Rect {
    w.frame_layout(FRAME)
        .first_of(kind)
        .unwrap_or_else(|| panic!("no {kind:?} pane"))
        .rect
}

/// A pane's content rows / cols.
fn rows_of(w: &Workspace, kind: PaneKind) -> u16 {
    rect_of(w, kind).height.saturating_sub(BORDER_SHAPE.0)
}
fn cols_of(w: &Workspace, kind: PaneKind) -> u16 {
    rect_of(w, kind).width.saturating_sub(BORDER_SHAPE.1)
}

/// Open a decision-pane-shaped transient pane of `rows`, splitting the output.
fn open_tool(w: &mut Workspace, rows: u16) {
    let near = w.first_of(PaneKind::Output).unwrap();
    decision_under(w, near, rows);
}

/// A decision pane of `rows` under `near` — the one open fitted, else one
/// opened — as the app fits one (`App::fit_tool_pane`).
fn decision_under(w: &mut Workspace, near: PaneId, rows: u16) {
    match w.first_of(PaneKind::Tool) {
        Some(id) => w.fit_decision(id, rows),
        None => {
            w.open_decision(near, rows, PaneKind::Tool);
        }
    }
}

/// Close every pane showing `kind`, each as the app closes one (`close`) —
/// the decision panes, as their answers close them.
fn close_every(w: &mut Workspace, kind: PaneKind) {
    let doomed: Vec<PaneId> = w.panes_of(kind).collect();
    for id in doomed {
        w.close(id);
    }
}

// --- geometry: the panes tile the body, a bar stays a bar -------------------

#[test]
fn default_layout_is_output_over_a_fixed_prompt_bar() {
    let w = ws();
    let fl = w.frame_layout(FRAME);
    assert_eq!(fl.status, Rect::new(0, 49, 80, 1), "status is the last row");
    assert_eq!(rect_of(&w, PaneKind::Prompt).height, 10 + BORDER_SHAPE.0);
    assert_eq!(
        fl.panes.iter().map(|p| p.rect.height).sum::<u16>(),
        49,
        "the panes tile the body"
    );
    assert!(
        rect_of(&w, PaneKind::Output).y < rect_of(&w, PaneKind::Prompt).y,
        "output above the prompt"
    );
}

#[test]
fn prompt_bar_stays_fixed_as_the_frame_grows() {
    let w = ws();
    let at = |h: u16| {
        w.frame_layout(Rect::new(0, 0, 80, h))
            .first_of(PaneKind::Prompt)
            .unwrap()
            .rect
            .height
    };
    assert_eq!(at(20), at(100), "a bar, not a percentage");
}

/// **The panes always exactly tile the body — at every pane count.** The old
/// version could only be asked about two; this walks whatever is there.
#[test]
fn panes_tile_the_body_exactly_at_every_count() {
    let mut w = ws();
    for extra in 0..4u16 {
        if extra > 0 {
            open_tool(&mut w, 3);
        }
        for h in [12u16, 24, 50, 100] {
            let frame = Rect::new(0, 0, 80, h);
            let fl = w.frame_layout(frame);
            let covered: u32 = fl
                .panes
                .iter()
                .map(|p| u32::from(p.rect.width) * u32::from(p.rect.height))
                .sum();
            let body = u32::from(frame.width) * u32::from(frame.height.saturating_sub(1));
            assert_eq!(
                covered,
                body,
                "no gap or overlap with {} panes at h={h}",
                fl.panes.len()
            );
        }
    }
}

// --- focus -----------------------------------------------------------------

#[test]
fn directional_focus_targets() {
    let mut w = ws();
    let body = Rect::new(0, 0, 80, 39);
    // Stacked, output on top: from the prompt, Up reaches the output.
    w.focus_kind(PaneKind::Prompt);
    w.focus_dir(Direction::Up, body);
    assert_eq!(w.focused_kind(), PaneKind::Output);
    w.focus_dir(Direction::Down, body);
    assert_eq!(w.focused_kind(), PaneKind::Prompt);
    // Cross-axis has no neighbour while stacked; edges stay put, never wrap.
    w.focus_kind(PaneKind::Output);
    w.focus_dir(Direction::Left, body);
    assert_eq!(w.focused_kind(), PaneKind::Output);
    w.focus_dir(Direction::Up, body);
    assert_eq!(w.focused_kind(), PaneKind::Output);
}

/// **Every pane is reachable by cycling, at every pane count** — and the ring
/// returns. The original bug was leaving a pane and being unable to get back,
/// because focus was a two-pane special case instead of movement over the tree.
#[test]
fn cycling_reaches_every_pane_that_exists() {
    let mut w = ws();
    let body = Rect::new(0, 0, 80, 39);
    for _ in 0..3 {
        let n = w.slots().len();
        let mut seen = Vec::new();
        for _ in 0..n {
            w.focus_cycle(true, body);
            seen.push(w.focused());
        }
        seen.sort();
        seen.dedup();
        assert_eq!(
            seen.len(),
            n,
            "a full cycle visits all {n} panes exactly once"
        );
        open_tool(&mut w, 3); // and again with one more pane
    }
}

/// A transient pane joins the ring and is reachable **by direction**, in both
/// directions, wherever it sits.
#[test]
fn a_transient_pane_is_reachable_and_leavable_by_direction() {
    let mut w = ws();
    open_tool(&mut w, 8);
    let body = Rect::new(0, 0, 80, 39);

    w.focus_kind(PaneKind::Output);
    w.focus_dir(Direction::Down, body);
    assert_eq!(
        w.focused_kind(),
        PaneKind::Tool,
        "down from the transcript reaches the decision"
    );
    w.focus_dir(Direction::Up, body);
    assert_eq!(w.focused_kind(), PaneKind::Output, "and up gets back out");
    w.focus_kind(PaneKind::Tool);
    w.focus_dir(Direction::Down, body);
    assert_eq!(
        w.focused_kind(),
        PaneKind::Prompt,
        "down continues to the prompt"
    );
}

/// Focus never dangles: closing the focused pane lands on a surviving one, and
/// closing a pane that did **not** hold focus leaves focus alone.
#[test]
fn focus_survives_a_pane_closing_and_is_not_stolen() {
    let mut w = ws();
    open_tool(&mut w, 5);

    w.focus_kind(PaneKind::Tool);
    close_every(&mut w, PaneKind::Tool);
    assert!(
        w.slot(w.focused()).is_some(),
        "focus landed on a pane that exists"
    );

    open_tool(&mut w, 5);
    w.focus_kind(PaneKind::Output);
    let held = w.focused();
    close_every(&mut w, PaneKind::Tool);
    assert_eq!(w.focused(), held, "a vanishing pane takes nobody's focus");
}

// --- resize ----------------------------------------------------------------

#[test]
fn resize_moves_the_divider_for_whichever_pane_is_focused() {
    let mut w = ws();
    let base = rows_of(&w, PaneKind::Prompt);

    w.focus_kind(PaneKind::Prompt);
    w.resize(SplitDir::Horizontal, 3, 80, 50);
    assert_eq!(rows_of(&w, PaneKind::Prompt), base + 3);
    w.resize(SplitDir::Horizontal, -5, 80, 50);
    assert_eq!(rows_of(&w, PaneKind::Prompt), base - 2);

    w.equalize(10, 80, 50);
    assert_eq!(
        rows_of(&w, PaneKind::Prompt),
        10,
        "back to its declared size"
    );

    // The other side of the same divider: growing the chat shrinks the prompt.
    w.focus_kind(PaneKind::Output);
    w.resize(SplitDir::Horizontal, 2, 80, 50);
    assert_eq!(rows_of(&w, PaneKind::Prompt), 8);

    // Off-axis is a no-op, as in vim.
    let h = rows_of(&w, PaneKind::Prompt);
    w.focus_kind(PaneKind::Prompt);
    w.resize(SplitDir::Vertical, 3, 80, 50);
    assert_eq!(rows_of(&w, PaneKind::Prompt), h);
}

#[test]
fn resize_clamps_to_floor_and_frame() {
    let mut w = ws();
    w.focus_kind(PaneKind::Prompt);
    w.resize(SplitDir::Horizontal, 9999, 80, 50);
    let big = rows_of(&w, PaneKind::Prompt);
    assert!((1..50).contains(&big), "clamped into the frame: {big}");
    w.resize(SplitDir::Horizontal, -9999, 80, 50);
    assert_eq!(rows_of(&w, PaneKind::Prompt), 1, "floors at 1");
}

/// **The decision pane is the user's to size and move** (Steven, 10-04: "We
/// shouldn't restrict this" — it refused, and so did the conversation beside
/// it). Content-fit until touched; `<C-w>+` grows it from the conversation
/// beside it, never from the prompt; `<C-w>L` stands it beside the
/// conversation at half the width — never its row count read as columns.
#[test]
fn a_decision_pane_is_the_users_to_size_and_move() {
    let mut w = ws();
    open_tool(&mut w, 5);
    assert_eq!(rows_of(&w, PaneKind::Tool), 5, "content-fit until touched");
    let (output, prompt) = (rows_of(&w, PaneKind::Output), rows_of(&w, PaneKind::Prompt));
    w.focus_kind(PaneKind::Tool);
    w.resize(SplitDir::Horizontal, 4, 80, 50);
    assert_eq!(rows_of(&w, PaneKind::Tool), 9);
    assert_eq!(rows_of(&w, PaneKind::Output), output - 4, "from beside it");
    assert_eq!(rows_of(&w, PaneKind::Prompt), prompt, "not from the prompt");
    w.relocate(Direction::Right, 80, 50);
    let (tool, out) = (rect_of(&w, PaneKind::Tool), rect_of(&w, PaneKind::Output));
    assert!(
        tool.x > out.x && tool.y == out.y,
        "beside it: {tool:?} {out:?}"
    );
    assert!((30..=50).contains(&tool.width), "half the width: {tool:?}");
}

/// **Where the user left it, the next decision opens** — side and size — and
/// a re-fit to new content leaves that size alone; `<C-w>=` gives it back to
/// its content, a band below.
#[test]
fn a_decision_pane_opens_where_it_was_left_until_equalized() {
    let mut w = ws();
    open_tool(&mut w, 5);
    w.focus_kind(PaneKind::Tool);
    w.relocate(Direction::Up, 80, 50);
    w.resize(SplitDir::Horizontal, 3, 80, 50);
    let placed = rect_of(&w, PaneKind::Tool);
    assert!(placed.y < rect_of(&w, PaneKind::Output).y, "premise: above");
    open_tool(&mut w, 2); // an edit re-fits the report
    assert_eq!(
        rect_of(&w, PaneKind::Tool),
        placed,
        "the user's size stands"
    );
    close_every(&mut w, PaneKind::Tool);
    open_tool(&mut w, 5); // the next decision
    assert_eq!(
        rect_of(&w, PaneKind::Tool),
        placed,
        "opens where it was left"
    );
    w.equalize(10, 80, 50);
    open_tool(&mut w, 5); // the app re-fits it to its content
    assert_eq!(rows_of(&w, PaneKind::Tool), 5);
    assert!(
        rect_of(&w, PaneKind::Tool).y > rect_of(&w, PaneKind::Output).y,
        "a band below again"
    );
    // …and where it was left is forgotten: the next one is a band too.
    close_every(&mut w, PaneKind::Tool);
    open_tool(&mut w, 4);
    assert_eq!(rows_of(&w, PaneKind::Tool), 4);
    assert!(rect_of(&w, PaneKind::Tool).y > rect_of(&w, PaneKind::Output).y);
}

/// **The first pane of a kind is the first on screen** — ring order, as the
/// corner numbers show, not the order panes were made: a split put BEFORE
/// the transcript is its kind's first (review stop 10, C 6).
#[test]
fn the_first_of_a_kind_is_the_first_on_screen() {
    let mut w = ws();
    let made_first = w.first_of(PaneKind::Output).unwrap();
    w.focus_pane(made_first);
    let before = w
        .split_pane(
            made_first,
            SplitDir::Vertical,
            NewPane::Before,
            PaneKind::Output,
        )
        .expect("split");
    assert_eq!(
        w.ring_order().iter().position(|&p| p == before),
        Some(0),
        "premise: the split is first in the ring"
    );
    assert_eq!(w.first_of(PaneKind::Output), Some(before));
}

/// **A content-sized pane promoted onto a column is never given its rows as
/// columns.** Closing the transcript above a decision, beside another one,
/// leaves the decision in the side-by-side split: re-fitting it there, or
/// fitting the frame, wrote its row count as a WIDTH.
#[test]
fn a_decision_promoted_beside_a_pane_keeps_a_column_width() {
    let mut w = ws();
    let left = w.first_of(PaneKind::Output).unwrap();
    w.focus_pane(left);
    let right = w
        .split_pane(left, SplitDir::Vertical, NewPane::After, PaneKind::Output)
        .expect("split");
    decision_under(&mut w, right, 3);
    assert!(w.close(right), "premise: the transcript above it closes");
    open_tool(&mut w, 3); // the app re-fits
    let tool = w
        .frame_layout(FRAME)
        .panes
        .iter()
        .find(|p| p.kind == PaneKind::Tool)
        .map(|p| p.rect)
        .expect("the decision is on screen");
    assert!(
        tool.width > 3 + BORDER_SHAPE.1,
        "a column, not 3 rows: {tool:?}"
    );
    // And in a frame too short for everything, where `fit` gives rows back:
    // narrowing a column frees no rows, so it must not narrow it at all.
    let short = Rect {
        height: 12,
        ..FRAME
    };
    let tool = w
        .frame_layout(short)
        .panes
        .iter()
        .find(|p| p.kind == PaneKind::Tool)
        .map(|p| p.rect)
        .expect("still on screen in a short frame");
    assert!(
        tool.width > 3 + BORDER_SHAPE.1,
        "short, still a column: {tool:?}"
    );
}

/// **In a short terminal the prompt gives way before the conversation
/// vanishes.** Bars shrank only to save a decision pane, so with none open a
/// ten-row prompt kept its rows and the transcript was squeezed to its
/// border (Steven's screenshot, 10-04).
#[test]
fn a_short_frame_shrinks_the_prompt_before_the_conversation() {
    let w = ws();
    let short = Rect {
        height: 12,
        ..FRAME
    };
    let fl = w.frame_layout(short);
    let out = fl.first_of(PaneKind::Output).unwrap().rect;
    assert!(
        out.height.saturating_sub(BORDER_SHAPE.0) >= MIN_FLEXIBLE_ROWS,
        "the conversation keeps its floor: {out:?}"
    );
    let prompt = fl.first_of(PaneKind::Prompt).unwrap().rect;
    assert!(prompt.height > BORDER_SHAPE.0, "still a prompt: {prompt:?}");
    assert_eq!(
        rows_of(&w, PaneKind::Prompt),
        10,
        "and the full prompt comes back in a frame with room"
    );
}

// --- relocate / reorient ----------------------------------------------------

#[test]
fn relocate_reorients_and_reorders() {
    let mut w = ws();
    w.focus_kind(PaneKind::Prompt);

    // <C-w>H: side-by-side, focused (prompt) on the left.
    w.relocate(Direction::Left, 80, 50);
    assert_ne!(
        rect_of(&w, PaneKind::Prompt).x,
        rect_of(&w, PaneKind::Output).x,
        "side by side"
    );
    assert!(
        rect_of(&w, PaneKind::Prompt).x < rect_of(&w, PaneKind::Output).x,
        "prompt on the left"
    );

    // Now width resize works.
    let base = cols_of(&w, PaneKind::Prompt);
    w.resize(SplitDir::Vertical, 4, 80, 50);
    assert_eq!(cols_of(&w, PaneKind::Prompt), base + 4);

    // <C-w>K: back to stacked, prompt on top.
    w.relocate(Direction::Up, 80, 50);
    assert_eq!(
        rect_of(&w, PaneKind::Prompt).x,
        rect_of(&w, PaneKind::Output).x,
        "stacked"
    );
    assert!(rect_of(&w, PaneKind::Prompt).y < rect_of(&w, PaneKind::Output).y);
    assert_eq!(
        rows_of(&w, PaneKind::Prompt),
        10,
        "the stacked span survived the round-trip"
    );
}

/// Per-axis size memory, which used to be a `prompt_rows`/`prompt_cols` pair on
/// the workspace and now belongs to the split — so it holds for **any** pane
/// that gets reoriented, not just the one that had fields named after it.
#[test]
fn per_axis_span_is_remembered_across_orientation_flips() {
    let mut w = ws();
    w.focus_kind(PaneKind::Prompt);
    w.resize(SplitDir::Horizontal, 5, 80, 50); // rows = 15
    w.relocate(Direction::Left, 80, 50); // → side by side
    w.resize(SplitDir::Vertical, 3, 80, 50); // cols = 43
    let cols = cols_of(&w, PaneKind::Prompt);
    w.relocate(Direction::Up, 80, 50); // → stacked
    assert_eq!(
        rows_of(&w, PaneKind::Prompt),
        15,
        "rows survived the flip to cols and back"
    );
    w.relocate(Direction::Left, 80, 50); // → side by side again
    assert_eq!(
        cols_of(&w, PaneKind::Prompt),
        cols,
        "and the cols survived the flip back to rows"
    );
}

// --- the yield policy -------------------------------------------------------

/// A transient pane **yields** rather than squeezing a flexible pane to
/// nothing, and disappears entirely when there is nothing to spare. Asserted as
/// a property over frame heights and over *several* transient panes, not as the
/// one arithmetic case the old `effective_tool_rows` covered.
#[test]
fn transient_panes_yield_to_keep_flexible_panes_readable() {
    for transients in 1..=3u16 {
        let mut w = ws();
        for _ in 0..transients {
            // Distinct kinds aren't needed: `show` updates in place, so this
            // exercises resize-in-place too.
            open_tool(&mut w, 8);
        }
        for h in 6..60u16 {
            let fl = w.frame_layout(Rect::new(0, 0, 80, h));
            // Whenever the transcript is on screen at all, it is readable...
            if let Some(out) = fl.first_of(PaneKind::Output) {
                let rows = out.content_size().1;
                assert!(
                    rows >= MIN_FLEXIBLE_ROWS || fl.first_of(PaneKind::Tool).is_none(),
                    "h={h}: transcript squeezed to {rows} with a decision still drawn"
                );
            }
            // ...and the panes still tile exactly.
            let covered: u32 = fl
                .panes
                .iter()
                .map(|p| u32::from(p.rect.width) * u32::from(p.rect.height))
                .sum();
            assert_eq!(covered, 80 * u32::from(h.saturating_sub(1)), "h={h}");
        }
    }
}

// --- hit testing ------------------------------------------------------------

/// Every pane is hit-testable, at every count, and the status row is not a pane.
#[test]
fn hit_testing_reaches_every_pane() {
    let mut w = ws();
    for _ in 0..2 {
        let fl = w.frame_layout(FRAME);
        for pane in &fl.panes {
            let centre = Position {
                x: pane.rect.x + pane.rect.width / 2,
                y: pane.rect.y + pane.rect.height / 2,
            };
            assert_eq!(
                w.pane_at(FRAME, centre),
                Some(pane.id),
                "{:?} is not clickable",
                pane.kind
            );
        }
        assert_eq!(
            w.pane_at(FRAME, Position { x: 40, y: 49 }),
            None,
            "the status row is not a pane"
        );
        assert_eq!(w.pane_at(FRAME, Position { x: 200, y: 200 }), None);
        open_tool(&mut w, 5);
    }
}

/// At most one pane is focused, at every pane count — the invariant the
/// renderer reads, asserted where it is produced.
#[test]
fn exactly_one_pane_is_marked_focused() {
    let mut w = ws();
    for _ in 0..3 {
        for id in w.slots().iter().map(|s| s.id).collect::<Vec<_>>() {
            w.focus_pane(id);
            let fl = w.frame_layout(FRAME);
            let focused: Vec<_> = fl.panes.iter().filter(|p| p.focused).collect();
            // A pane dropped by the fit pass may be the focused one; then no
            // pane is marked, which is the honest answer (and what the input
            // router keys "not visible, so not answerable" off).
            assert!(focused.len() <= 1, "{} panes marked focused", focused.len());
            if let Some(p) = fl.pane(id) {
                assert!(p.focused, "the focused pane must be the one marked");
            }
        }
        open_tool(&mut w, 4);
    }
}

// --- arbitrary number, arbitrary order --------------------------------------
//
// The tests above are still shaped by the arrangement we happen to ship. These
// are not: they build workspaces of random size and random shape and assert the
// invariants over whatever comes out. The point is not extra coverage — it is
// that the invariants can be *stated* without naming a pane or a count, which
// is the property the rewrite was for. If any of them needed to know how many
// panes there were, the model would still be wrong.

mod props {
    use super::*;
    use proptest::prelude::*;

    /// A scripted edit to a workspace — the operations a user has.
    #[derive(Debug, Clone)]
    enum Op {
        /// Split the `i`-th existing pane, adding a pane of the `k`-th kind.
        Open {
            at: usize,
            kind: usize,
            vertical: bool,
            extent: usize,
        },
        Close {
            kind: usize,
        },
        Focus {
            at: usize,
        },
        Cycle {
            forward: bool,
        },
        Move {
            dir: usize,
        },
        Resize {
            dir: bool,
            delta: i32,
        },
        Equalize,
        /// The destructive doors the op set never opened. `only` removes MANY
        /// panes in one pass and reconciles focus once — the shape most likely
        /// to strand focus or untile the tree, and the five invariants below
        /// had never once seen it.
        CloseFocused,
        Only,
        SplitFocused {
            vertical: bool,
        },
        ShowIn {
            at: usize,
            kind: usize,
        },
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0usize..8, 0usize..3, any::<bool>(), 0usize..3).prop_map(
                |(at, kind, vertical, extent)| Op::Open {
                    at,
                    kind,
                    vertical,
                    extent
                }
            ),
            (0usize..3).prop_map(|kind| Op::Close { kind }),
            (0usize..8).prop_map(|at| Op::Focus { at }),
            any::<bool>().prop_map(|forward| Op::Cycle { forward }),
            (0usize..4).prop_map(|dir| Op::Move { dir }),
            (any::<bool>(), -20i32..20).prop_map(|(dir, delta)| Op::Resize { dir, delta }),
            Just(Op::Equalize),
            Just(Op::CloseFocused),
            Just(Op::Only),
            any::<bool>().prop_map(|vertical| Op::SplitFocused { vertical }),
            (0usize..8, 0usize..3).prop_map(|(at, kind)| Op::ShowIn { at, kind }),
        ]
    }

    const KINDS: [PaneKind; 3] = [PaneKind::Output, PaneKind::Prompt, PaneKind::Tool];
    const DIRS: [Direction; 4] = [
        Direction::Up,
        Direction::Down,
        Direction::Left,
        Direction::Right,
    ];

    fn apply(w: &mut Workspace, op: &Op, frame: Rect) {
        let ids: Vec<PaneId> = w.slots().iter().map(|s| s.id).collect();
        match *op {
            Op::Open {
                at,
                kind,
                vertical,
                extent,
            } => {
                let near = ids[at % ids.len()];
                let extent = match extent {
                    0 => Extent::Flexible,
                    1 => Extent::Bar {
                        preferred: AxisSizing::uniform(Sizing::SecondCells(6)),
                    },
                    _ => Extent::Transient { rows: 5 },
                };
                let dir = if vertical {
                    SplitDir::Vertical
                } else {
                    SplitDir::Horizontal
                };
                w.open(extent, near, dir, KINDS[kind]);
            }
            Op::Close { kind } => close_every(w, KINDS[kind]),
            Op::Focus { at } => w.focus_pane(ids[at % ids.len()]),
            Op::Cycle { forward } => w.focus_cycle(forward, frame),
            Op::Move { dir } => w.relocate(DIRS[dir], frame.width, frame.height),
            Op::Resize { dir, delta } => w.resize(
                if dir {
                    SplitDir::Vertical
                } else {
                    SplitDir::Horizontal
                },
                delta,
                frame.width,
                frame.height,
            ),
            Op::Equalize => w.equalize(10, frame.width, frame.height),
            Op::CloseFocused => {
                w.close_focused();
            }
            Op::Only => {
                w.only(&[w.focused()]);
            }
            Op::SplitFocused { vertical } => {
                let dir = if vertical {
                    SplitDir::Vertical
                } else {
                    SplitDir::Horizontal
                };
                let focused = w.focused();
                let kind = w.kind_of(focused).unwrap_or(PaneKind::Output);
                let _ = w.split_pane(focused, dir, NewPane::Before, kind);
            }
            // What a pane shows is its window's: changed by giving it another.
            Op::ShowIn { at, kind } => {
                if let Some(window) = w.window_mut(ids[at % ids.len()]) {
                    *window = KINDS[kind];
                }
            }
        }
    }

    proptest! {
        /// **The invariants hold at any pane count, in any arrangement.**
        ///
        /// Every one of these is stated over "the panes" — none of them can be
        /// written down in a form that mentions how many there are, which is
        /// precisely why the old two-pane shortcuts could not express them.
        #[test]
        fn the_invariants_survive_arbitrary_edits(
            ops in proptest::collection::vec(op(), 0..24),
            w_frame in 10u16..120,
            h_frame in 4u16..60,
        ) {
            let frame = Rect::new(0, 0, w_frame, h_frame);
            let mut w = ws();
            for o in &ops {
                apply(&mut w, o, frame);

                let fl = w.frame_layout(frame);

                // 1. The panes tile the body EXACTLY — no gap, no overlap.
                let covered: u32 = fl.panes.iter()
                    .map(|p| u32::from(p.rect.width) * u32::from(p.rect.height))
                    .sum();
                prop_assert_eq!(
                    covered,
                    u32::from(frame.width) * u32::from(frame.height.saturating_sub(1)),
                    "tiling broke with {} panes", fl.panes.len()
                );

                // 2. AT MOST ONE pane is marked focused. (Zero when the focused
                //    pane was dropped by the fit pass — "not on screen" is an
                //    honest answer, and the one the input router keys off.)
                prop_assert!(
                    fl.panes.iter().filter(|p| p.focused).count() <= 1,
                    "more than one focused pane"
                );

                // 3. Focus always points at a pane that EXISTS.
                prop_assert!(
                    w.slot(w.focused()).is_some(),
                    "focus dangles after {:?}", o
                );

                // 4. Every pane on screen is hit-testable at its own centre, and
                //    the hit resolves to ITSELF — never to a neighbour.
                for pane in &fl.panes {
                    if pane.rect.width == 0 || pane.rect.height == 0 {
                        continue; // squeezed out; nothing to click
                    }
                    let centre = Position {
                        x: pane.rect.x + pane.rect.width / 2,
                        y: pane.rect.y + pane.rect.height / 2,
                    };
                    prop_assert_eq!(
                        w.pane_at(frame, centre), Some(pane.id),
                        "{:?} at {:?} is not clickable", pane.kind, pane.rect
                    );
                }

                // 5. There is always at least one pane: a layout cannot empty.
                prop_assert!(!w.slots().is_empty());
            }
        }

        /// Cycling is a **permutation of the panes**: `n` steps visits each
        /// exactly once and returns to where it started, at any `n` and after
        /// any rearrangement. This is the property "you can always get back to
        /// the pane you left" — the original bug — stated without a count.
        #[test]
        fn cycling_is_a_permutation(
            ops in proptest::collection::vec(op(), 0..12),
            h_frame in 20u16..60,
        ) {
            let frame = Rect::new(0, 0, 80, h_frame);
            let mut w = ws();
            for o in &ops {
                apply(&mut w, o, frame);
            }
            // Only panes the fit pass actually keeps take part in the ring.
            let on_screen: Vec<PaneId> =
                w.frame_layout(frame).panes.iter().map(|p| p.id).collect();
            prop_assume!(!on_screen.is_empty());
            w.focus_pane(on_screen[0]);

            let start = w.focused();
            let mut seen = Vec::new();
            for _ in 0..on_screen.len() {
                w.focus_cycle(true, w.body(frame));
                seen.push(w.focused());
            }
            prop_assert_eq!(w.focused(), start, "a full cycle returns");
            let mut uniq = seen.clone();
            uniq.sort();
            uniq.dedup();
            prop_assert_eq!(
                uniq.len(), on_screen.len(),
                "a full cycle visits every pane exactly once: {:?}", seen
            );
        }
    }
}

/// A transient pane's size is **cells along its own split's axis, from its own
/// side** — not `SecondCells(rows)`, which assumed both.
///
/// Reviewers B, C and D found this independently; the repro is the cheapest
/// statement of it. `remove` promotes a pane into its grandparent's split
/// (different axis, possibly the other side), and `relocate` reorients the
/// split a pane shares with its neighbour. Either turned the decision pane's
/// ROW count into a COLUMN count: a 110-column decision beside an 8-column
/// transcript, on a 120-column terminal.
#[test]
fn a_transient_pane_keeps_its_size_when_the_tree_moves_it() {
    let frame = Rect::new(0, 0, 120, 40);
    let mut w = ws();
    w.focus_kind(PaneKind::Output);
    w.split_pane(
        w.focused(),
        SplitDir::Vertical,
        NewPane::Before,
        PaneKind::Output,
    );
    let near = w.first_of(PaneKind::Output).unwrap();
    decision_under(&mut w, near, 8);
    // Close the pane it was split off — `remove` promotes it into the
    // *vertical* split above.
    w.focus_pane(near);
    w.close_focused();
    // It re-fits itself (what an edit-and-re-gate does).
    let near2 = w.first_of(PaneKind::Output).unwrap();
    decision_under(&mut w, near2, 8);

    let fl = w.frame_layout(frame);
    let tool = fl.first_of(PaneKind::Tool).expect("still open");
    let out = fl.first_of(PaneKind::Output).expect("still open");
    assert!(
        out.content_size().0 >= 20,
        "the transcript was squeezed to {} columns by a ROW count applied as columns",
        out.content_size().0
    );
    assert!(
        tool.rect.width < 120,
        "the decision pane took the whole frame: {:?}",
        tool.rect
    );
}

/// `<C-w>=` must restore a bar to ITS size, whichever side it now sits on.
///
/// `preferred` is stored in the `SecondCells` convention; `<C-w>K` swaps the
/// prompt to the first side, mirroring the tree's sizing but not the slot's.
/// Writing it back verbatim handed the bar's size to the *sibling* — a 35-row
/// prompt over a 10-row transcript. (Reviewer A, reproduced.)
#[test]
fn equalize_restores_a_relocated_bar_to_its_own_size() {
    let mut w = ws();
    w.focus_kind(PaneKind::Prompt);
    w.relocate(Direction::Up, 80, 50); // prompt to the top — now a FIRST child
    w.equalize(10, 80, 50);
    assert_eq!(
        rows_of(&w, PaneKind::Prompt),
        10,
        "the prompt keeps its declared height after being relocated"
    );
    assert!(
        rows_of(&w, PaneKind::Output) > 10,
        "and the transcript keeps the rest, not the bar's size"
    );
}

/// **A move aimed at the conversation takes the decision pane along as a
/// pane** — `<C-w>H` on the transcript reorients the split it shares with
/// the decision, which once read the decision's row count as a COLUMN count.
/// The decision becomes a bar at its drawn size first, its other axis
/// proportional. And a decision pane is never split, sized or not: each
/// session's decision has one pane.
#[test]
fn a_move_beside_the_decision_never_reads_its_rows_as_columns() {
    let mut w = ws();
    let near = w.first_of(PaneKind::Output).unwrap();
    decision_under(&mut w, near, 6);
    w.focus_pane(near);
    w.relocate(Direction::Left, 80, 50);
    let (out, tool) = (rect_of(&w, PaneKind::Output), rect_of(&w, PaneKind::Tool));
    assert!(
        out.x < tool.x,
        "the conversation on the left: {out:?} {tool:?}"
    );
    assert!(
        tool.width > 6 + BORDER_SHAPE.1,
        "not six rows read as columns: {tool:?}"
    );
    w.focus_kind(PaneKind::Tool);
    assert!(
        w.split_pane(
            w.focused(),
            SplitDir::Horizontal,
            NewPane::Before,
            PaneKind::Tool
        )
        .is_none()
    );
}

/// `PaneKind::all()` must really be all of them.
///
/// **This test used to prove nothing.** It iterated `PaneKind::ALL` — a
/// hand-written array — matched each element against an exhaustive arm, and
/// then compared `ALL.len()` to itself. Deleting a variant from `ALL` left
/// every assertion passing: the loop simply never saw it, and the length
/// agreed with the length. Reviewers mutation-verified that, and it had been
/// written the same day, in the commit about this exact class of bug.
///
/// The list is now `every_variant!`'s, whose exhaustive match IS the list —
/// so **the compiler**, not this test, is what makes it complete; the dead-end
/// chain this test was written against cannot be written (review stop 12,
/// Tier 3). What is left here: the kinds reachable by name, and the
/// openable set, are in it.
#[test]
fn pane_kind_all_is_complete() {
    let all = PaneKind::all();
    // Every name the command table offers (`PANE_NAMES`), not five typed here
    // (review stop 12, T7).
    for &name in crate::command::PANE_NAMES {
        let kind = PaneKind::from_name(name).expect("a name the command table accepts");
        assert!(
            all.contains(&kind),
            "{kind:?} can be opened by name but is not in the chain"
        );
    }
    assert!(
        all.contains(&PaneKind::Tool) && all.contains(&PaneKind::Inspector),
        "the two kinds opened for you rather than by you are in the chain too"
    );
    for kind in PaneKind::openable_kinds() {
        assert!(
            all.contains(&kind),
            "{kind:?} is openable but missing from the chain"
        );
    }
}

/// **Openable-by-name and openable-by-policy must be the same set.**
///
/// Deriving the list from `PaneKind::all()` made it complete *against the
/// enum* — a new variant is now a compile error. It did **not** pin the
/// answers: Reviewer D's original mutation (dropping `Messages`) still passed
/// after that fix, because "should this kind be openable?" is a policy the
/// compiler cannot check. A subset assertion never could catch it either;
/// that is what let the hand-written array lose a member in the first place.
///
/// So: a round trip in both directions against `from_name`, which is the door
/// the user actually comes through. Modelled on
/// `command::model_names_match_the_alias_table`, the one member of this family
/// that already got it right.
#[test]
fn every_openable_kind_has_a_name_and_every_name_is_openable() {
    use crate::command::PANE_NAMES;

    let openable = PaneKind::openable_kinds();

    for kind in &openable {
        let reachable = PANE_NAMES
            .iter()
            .filter_map(|n| PaneKind::from_name(n))
            .any(|k| k == *kind);
        assert!(
            reachable,
            "{kind:?} is openable but no name in PANE_NAMES resolves to it — \
             `:split {}` would be advertised and then refused",
            kind.title()
        );
    }

    for name in PANE_NAMES {
        let kind = PaneKind::from_name(name).unwrap_or_else(|| {
            panic!("`{name}` is offered for completion but resolves to nothing")
        });
        assert!(
            openable.contains(&kind),
            "`{name}` resolves to {kind:?}, which policy says is not openable"
        );
    }
}

/// **Two workspaces never share a pane id** — a tab each
/// (`docs/DesignDocs/tabs.md` §1): every map keyed by pane, a viewer's state
/// above all, would hold two tabs' panes under one key. Each counted from 0.
#[test]
fn two_workspaces_never_share_a_pane_id() {
    let (mut a, mut b) = (ws(), ws());
    for ws in [&mut a, &mut b] {
        let near = ws.focused();
        ws.open(
            Extent::Flexible,
            near,
            SplitDir::Vertical,
            PaneKind::Page(PageKind::Help),
        );
        assert_eq!(ws.pane_count(), 3, "premise: one opened");
    }
    assert!(
        a.pane_ids().is_disjoint(&b.pane_ids()),
        "{:?} and {:?}",
        a.pane_ids(),
        b.pane_ids()
    );
}

/// **Two decision panes are each their own** (`decision-panes-and-alerts.md`
/// §3.3; design review DR-A #2) — opened under two windows and fitted by id,
/// each to its own rows; and what the next one remembers is the pane you
/// resized last, not the first one made. Both are resized, the first first,
/// so a memory taken from the first decision pane (as it was) would differ.
#[test]
fn two_decision_panes_are_fitted_and_remembered_each_its_own() {
    let mut w = ws();
    let left = w.first_of(PaneKind::Output).unwrap();
    w.focus_pane(left);
    let right = w
        .split_pane(left, SplitDir::Vertical, NewPane::After, PaneKind::Output)
        .expect("split");
    let a = w.open_decision(left, 4, PaneKind::Tool);
    let b = w.open_decision(right, 7, PaneKind::Tool);
    let rows = |w: &Workspace, id: PaneId| {
        (w.frame_layout(FRAME).panes.iter())
            .find(|p| p.id == id)
            .map(|p| p.rect.height.saturating_sub(BORDER_SHAPE.0))
            .expect("drawn")
    };
    w.fit_decision(a, 5);
    assert_eq!((rows(&w, a), rows(&w, b)), (5, 7), "each fitted by its id");

    w.focus_pane(a);
    w.resize(SplitDir::Horizontal, 1, FRAME.width, FRAME.height);
    w.focus_pane(b);
    w.resize(SplitDir::Horizontal, 4, FRAME.width, FRAME.height);
    let (first, second) = (rows(&w, a), rows(&w, b));
    assert_ne!(first, second, "premise: two sizes");
    assert!(w.close(a) && w.close(b), "premise: both close");
    let next = w.open_decision(left, 3, PaneKind::Tool);
    assert_eq!(rows(&w, next), second, "the size of the pane resized last");
}
