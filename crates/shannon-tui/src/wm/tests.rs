//! Pure tests for the window-manager core: geometry, focus, overlays. No
//! rendering, no event types — the whole point of keeping this layer content-
//! agnostic is that it's exhaustively testable in isolation.

use super::*;

fn area(w: u16, h: u16) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: w,
        height: h,
    }
}

fn p(n: u64) -> PaneId {
    PaneId(n)
}

// --- Layout geometry ------------------------------------------------------

#[test]
fn single_pane_fills_the_area() {
    let l = Layout::single(p(1));
    assert_eq!(l.compute(area(80, 24)), vec![(p(1), area(80, 24))]);
}

#[test]
fn splits_tile_exactly_with_no_gap_or_overlap() {
    // A vsplit 50/50 of an even width divides cleanly; the two rects abut.
    let mut l = Layout::single(p(1));
    assert!(l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50)));
    let rects = l.compute(area(80, 24));
    let left = rects.iter().find(|(id, _)| *id == p(1)).unwrap().1;
    let right = rects.iter().find(|(id, _)| *id == p(2)).unwrap().1;
    assert_eq!(
        left,
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 24
        }
    );
    assert_eq!(
        right,
        Rect {
            x: 40,
            y: 0,
            width: 40,
            height: 24
        }
    );
    // No gap, no overlap: right starts exactly where left ends.
    assert_eq!(left.x + left.width, right.x);
}

#[test]
fn odd_split_gives_the_remainder_to_second() {
    // 81 wide, 50% → first floor(40.5)=40, second gets the leftover 41.
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50));
    let rects = l.compute(area(81, 10));
    let a = rects[0].1;
    let b = rects[1].1;
    assert_eq!(a.width + b.width, 81, "exactly tiles, no lost column");
    assert_eq!(a.width, 40);
    assert_eq!(b.width, 41);
}

#[test]
fn horizontal_split_stacks_top_over_bottom() {
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Horizontal, Sizing::Ratio(25));
    let rects = l.compute(area(80, 24));
    let top = rects[0].1;
    let bottom = rects[1].1;
    assert_eq!(top.height, 6); // 25% of 24
    assert_eq!(bottom.height, 18);
    assert_eq!(top.y + top.height, bottom.y);
}

#[test]
fn ratio_is_clamped_into_range() {
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(250)); // absurd → clamped to 99
    let rects = l.compute(area(100, 10));
    assert_eq!(rects[0].1.width, 99);
    assert_eq!(rects[1].1.width, 1);
}

#[test]
fn nested_splits_compute_and_enumerate_in_order() {
    // Split into left|right, then split the right into top/bottom.
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50));
    l.split(p(2), p(3), SplitDir::Horizontal, Sizing::Ratio(50));
    assert_eq!(l.leaves(), vec![p(1), p(2), p(3)]);
    let rects = l.compute(area(80, 20));
    assert_eq!(rects.len(), 3);
    // Every rect stays within the frame and the areas sum to the whole.
    let total: u32 = rects
        .iter()
        .map(|(_, r)| u32::from(r.width) * u32::from(r.height))
        .sum();
    assert_eq!(total, 80 * 20);
}

// --- split / remove -------------------------------------------------------

#[test]
fn split_targets_the_named_pane_only() {
    let mut l = Layout::single(p(1));
    assert!(
        !l.split(p(99), p(2), SplitDir::Vertical, Sizing::Ratio(50)),
        "unknown target"
    );
    assert!(l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50)));
    assert_eq!(l.leaves(), vec![p(1), p(2)]);
}

#[test]
fn remove_promotes_the_sibling() {
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50));
    l.split(p(2), p(3), SplitDir::Horizontal, Sizing::Ratio(50));
    assert!(l.remove(p(2)));
    assert_eq!(l.leaves(), vec![p(1), p(3)], "3 takes 2's place");
    // The sibling now owns the whole right column.
    let rects = l.compute(area(80, 20));
    let three = rects.iter().find(|(id, _)| *id == p(3)).unwrap().1;
    assert_eq!(three.width, 40);
}

#[test]
fn cannot_remove_the_last_pane() {
    let mut l = Layout::single(p(1));
    assert!(!l.remove(p(1)), "a layout must keep at least one pane");
    assert_eq!(l.leaves(), vec![p(1)]);
}

// --- pane-relative ops (the <C-w> window commands) ------------------------

#[test]
fn resize_reorient_swap_target_the_split_around_a_pane() {
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50));

    // Resize: set the sizing around either pane (they share the one split).
    assert!(l.set_sizing_around(p(1), Sizing::Ratio(70)));
    assert_eq!(
        l.sizing_around(p(2)),
        Some(Sizing::Ratio(70)),
        "same split, seen from either pane"
    );
    let rects = l.compute(area(100, 10));
    assert_eq!(rects[0].1.width, 70);

    // Reorient: side-by-side → stacked.
    assert_eq!(l.dir_around(p(1)), Some(SplitDir::Vertical));
    assert!(l.reorient_around(p(1), SplitDir::Horizontal));
    assert_eq!(l.dir_around(p(1)), Some(SplitDir::Horizontal));

    // Swap: the two panes trade places (order flips), geometry preserved.
    assert!(l.swap_with_sibling(p(1)));
    assert_eq!(l.leaves(), vec![p(2), p(1)]);
}

#[test]
fn pane_relative_ops_are_noops_on_a_lone_pane() {
    let mut l = Layout::single(p(1));
    assert!(
        !l.set_sizing_around(p(1), Sizing::Ratio(50)),
        "no parent split to resize"
    );
    assert!(!l.reorient_around(p(1), SplitDir::Horizontal));
    assert!(!l.swap_with_sibling(p(1)));
    assert_eq!(l.sizing_around(p(1)), None);
    assert_eq!(l.leaves(), vec![p(1)]);
}

#[test]
fn absolute_sizing_is_a_fixed_bar_that_does_not_grow_with_the_area() {
    // The prompt case: a fixed-height bar as `second`, output takes the rest.
    // The bar stays N rows whether the frame is short or tall — the whole reason
    // Sizing exists (a percent would grow it).
    let mut l = Layout::single(p(1)); // output
    l.split(p(1), p(2), SplitDir::Horizontal, Sizing::SecondCells(6)); // prompt = 6 rows
    for h in [12u16, 40, 100] {
        let rects = l.compute(area(80, h));
        let output = rects.iter().find(|(id, _)| *id == p(1)).unwrap().1;
        let prompt = rects.iter().find(|(id, _)| *id == p(2)).unwrap().1;
        assert_eq!(prompt.height, 6, "bar is fixed at 6 rows (frame h={h})");
        assert_eq!(output.height, h - 6, "output takes the rest");
        assert_eq!(output.height + prompt.height, h, "exact tiling");
    }
    // FirstCells mirrors it (bar as the first pane).
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Horizontal, Sizing::FirstCells(6));
    let rects = l.compute(area(80, 20));
    assert_eq!(rects[0].1.height, 6);
    assert_eq!(rects[1].1.height, 14);
}

#[test]
fn absolute_sizing_clamps_and_never_underflows() {
    // A bar larger than the frame: it takes all of it, the other pane gets zero
    // (not a panic / underflow).
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Horizontal, Sizing::SecondCells(100));
    let rects = l.compute(area(80, 10));
    assert_eq!(
        rects[0].1.height, 0,
        "output squeezed to nothing, no underflow"
    );
    assert_eq!(rects[1].1.height, 10, "bar clamped to the frame");
}

// --- Focus ----------------------------------------------------------------

#[test]
fn cycle_wraps_in_ring_order() {
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50));
    l.split(p(2), p(3), SplitDir::Vertical, Sizing::Ratio(50));
    let mut f = Focus::new(p(1));
    f.cycle(&l, true);
    assert_eq!(f.pane(), p(2));
    f.cycle(&l, true);
    assert_eq!(f.pane(), p(3));
    f.cycle(&l, true);
    assert_eq!(f.pane(), p(1), "wraps");
    f.cycle(&l, false);
    assert_eq!(f.pane(), p(3), "backward wraps too");
}

#[test]
fn reconcile_rescues_focus_after_removal() {
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50));
    let mut f = Focus::new(p(2));
    l.remove(p(2));
    f.reconcile(&l);
    assert_eq!(f.pane(), p(1), "focus follows to a surviving pane");
}

#[test]
fn shift_moves_focus_spatially() {
    // left | right, focus starts left; shift right lands on the right pane.
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50));
    let a = area(80, 24);
    let mut f = Focus::new(p(1));
    f.shift(&l, a, Direction::Right);
    assert_eq!(f.pane(), p(2));
    f.shift(&l, a, Direction::Right);
    assert_eq!(f.pane(), p(2), "no pane further right — stays put");
    f.shift(&l, a, Direction::Left);
    assert_eq!(f.pane(), p(1));
    f.shift(&l, a, Direction::Up);
    assert_eq!(f.pane(), p(1), "nothing above in a side-by-side split");
}

/// vim `<C-w>p` is "the pane I came FROM", which is a different question from
/// "the previous pane in the ring". They give the same answer at two panes —
/// which is exactly why `FocusPrev` was wired to `cycle(back)` and looked
/// right. Three panes is the smallest case that can tell them apart.
#[test]
fn alternate_returns_to_where_you_came_from_not_around_the_ring() {
    // Ring order is p1, p2, p3.
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Horizontal, Sizing::Ratio(33));
    l.split(p(2), p(3), SplitDir::Horizontal, Sizing::Ratio(50));
    assert_eq!(l.leaves(), vec![p(1), p(2), p(3)]);

    let mut f = Focus::new(p(1));
    f.cycle(&l, true); // p1 -> p2
    f.cycle(&l, true); // p2 -> p3

    assert!(f.alternate(&l));
    assert_eq!(f.pane(), p(2), "back to where we came from");
    // Ring-order-back from p3 is also p2, so assert the half that differs:
    // alternating again must RETURN to p3, where cycling back would go to p1.
    assert!(f.alternate(&l));
    assert_eq!(
        f.pane(),
        p(3),
        "alternate toggles; it does not walk the ring"
    );
}

/// Nothing to go back to is a refusal the caller must be able to report, not a
/// silent no-op — and a remembered pane that has closed must never be the
/// target (it would focus a pane that no longer exists).
#[test]
fn alternate_refuses_with_no_history_or_a_closed_pane() {
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Horizontal, Sizing::Ratio(50));
    let mut f = Focus::new(p(1));
    assert!(!f.alternate(&l), "no move has happened yet");

    f.cycle(&l, true);
    assert_eq!(f.pane(), p(2));
    // p1 — the pane we came from — closes.
    l.remove(p(1));
    f.reconcile(&l);
    assert!(!f.alternate(&l), "the pane we came from is gone");
    assert_eq!(f.pane(), p(2));
}

/// Re-focusing the pane you are already on is not a move. If it overwrote the
/// memory, a redundant click (or `<C-w>w` on a lone pane) would quietly turn
/// `<C-w>p` into a no-op.
#[test]
fn refocusing_the_current_pane_does_not_erase_the_way_back() {
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Horizontal, Sizing::Ratio(50));
    let mut f = Focus::new(p(1));
    f.cycle(&l, true);
    f.set(p(2), &l); // click on the pane you are already in
    f.set(p(2), &l);
    assert!(f.alternate(&l));
    assert_eq!(f.pane(), p(1));
}

// --- Overlays -------------------------------------------------------------

#[test]
fn modal_captures_input_float_does_not() {
    let mut s = OverlayStack::new();
    assert_eq!(s.captures_input(), None);

    s.push(Overlay {
        buffer: BufferId(7),
        area: area(20, 5),
        kind: OverlayKind::Float,
    });
    assert_eq!(s.captures_input(), None, "a float does not steal input");

    s.push(Overlay {
        buffer: BufferId(9),
        area: area(30, 8),
        kind: OverlayKind::Modal,
    });
    assert_eq!(
        s.captures_input(),
        Some(BufferId(9)),
        "the topmost modal captures"
    );

    s.pop();
    assert_eq!(
        s.captures_input(),
        None,
        "back to the float; input flows to panes"
    );
}

#[test]
fn overlays_iterate_bottom_to_top_for_rendering() {
    let mut s = OverlayStack::new();
    s.push(Overlay {
        buffer: BufferId(1),
        area: area(1, 1),
        kind: OverlayKind::Float,
    });
    s.push(Overlay {
        buffer: BufferId(2),
        area: area(1, 1),
        kind: OverlayKind::Modal,
    });
    let order: Vec<u64> = s.iter().map(|o| o.buffer.0).collect();
    assert_eq!(order, vec![1, 2], "draw bottom first so the top lands last");
}

/// Directional focus must mean **the neighbour in that direction**, not "the
/// nearest centre that happens to lie that way".
///
/// The centre-distance heuristic this replaced weighted the cross axis and fell
/// apart as soon as a third row existed: with the graph and its inspector side
/// by side above a full-width prompt, `<C-w>l` from the graph scored the prompt
/// (25² + 13²·4 = 1301) below the inspector (50² = 2500) and focused the pane
/// *below*. Overlap is the rule vim uses and the only one that stays right.
#[test]
fn directional_focus_picks_the_neighbour_not_the_nearest_centre() {
    // output over [graph | inspector] over prompt — the real `:graph` shape.
    let (out, graph, inspect, prompt) = (p(1), p(2), p(3), p(4));
    let mut l = Layout::single(out);
    l.split(out, graph, SplitDir::Horizontal, Sizing::Ratio(33));
    l.split(graph, prompt, SplitDir::Horizontal, Sizing::Ratio(60));
    l.split(graph, inspect, SplitDir::Vertical, Sizing::Ratio(50));
    let a = area(100, 39);

    let go = |from: PaneId, dir: Direction| {
        let mut f = Focus::new(from);
        f.shift(&l, a, dir);
        f.pane()
    };

    // The bug, exactly: right of the graph is the inspector, never the prompt.
    assert_eq!(
        go(graph, Direction::Right),
        inspect,
        "right = beside, not below"
    );
    assert_eq!(go(inspect, Direction::Left), graph);
    // Vertical movement crosses whole rows.
    assert_eq!(go(out, Direction::Down), graph, "down from the top row");
    assert_eq!(go(graph, Direction::Down), prompt);
    assert_eq!(
        go(inspect, Direction::Down),
        prompt,
        "from the right column too"
    );
    assert_eq!(
        go(prompt, Direction::Up),
        graph,
        "up lands in the left column"
    );
    // Edges stay put rather than wrapping or sliding diagonally.
    assert_eq!(go(out, Direction::Up), out);
    assert_eq!(
        go(out, Direction::Left),
        out,
        "a full-width pane has no left"
    );
    assert_eq!(go(prompt, Direction::Right), prompt);
}

/// The case the **overlap** rule decides, as opposed to the edge test.
///
/// Two columns split at different heights:
///
///     +--------+-------+
///     |   1    |   2   |     1: y 0..10    2: y 0..5
///     |        +-------+
///     +--------+       |
///     |   3    |   4   |     3: y10..20    4: y 5..20
///     +--------+-------+
///
/// From 3, everything in the right column starts at the same x, so "is it past
/// my edge" cannot choose between 2 and 4 — and a tie-break on position picks
/// 2, which does not share a single row with 3. Only overlap gets this right.
#[test]
fn directional_focus_needs_overlap_not_just_an_edge() {
    let (p1, p2, p3, p4) = (p(1), p(2), p(3), p(4));
    let mut l = Layout::single(p1);
    l.split(p1, p2, SplitDir::Vertical, Sizing::Ratio(50)); // 1 | 2
    l.split(p1, p3, SplitDir::Horizontal, Sizing::Ratio(50)); // left: 1 over 3
    l.split(p2, p4, SplitDir::Horizontal, Sizing::Ratio(25)); // right: 2 over 4
    let a = area(100, 20);

    let go = |from: PaneId, dir: Direction| {
        let mut f = Focus::new(from);
        f.shift(&l, a, dir);
        f.pane()
    };

    assert_eq!(
        go(p3, Direction::Right),
        p4,
        "right of the lower-left pane is the pane it shares rows with"
    );
    assert_eq!(go(p1, Direction::Right), p2, "and the upper-left reaches 2");
    assert_eq!(go(p2, Direction::Left), p1, "2 shares rows only with 1");
    // Going back left from 4 is genuinely ambiguous — it spans rows 5..20, so
    // it touches BOTH 1 (0..10) and 3 (10..20). vim resolves this with the
    // cursor's row; panes do not carry their own cursor yet, so the documented
    // tie-break is topmost. Asserted so the choice is deliberate, not incidental.
    assert_eq!(go(p4, Direction::Left), p1, "ties go to the topmost");
}

/// `Sizing::mirrored` is the same division seen from the other end of a
/// divider — and nothing observably exercised it.
///
/// Every test that reached a `swap_with_sibling` did so while the sizing was
/// `Ratio(50)`, whose mirror is itself: deleting the mirroring entirely was
/// invisible. The doc says both axes must mirror "or the size only lies after
/// the next reorient", and that claim had no test. Stated as the property that
/// matters — a mirrored sizing gives `second` exactly what the original gave
/// `first` — rather than as a table of pairs.
#[test]
fn a_mirrored_sizing_swaps_the_two_sides() {
    let area = area(100, 40);
    let width = |r: Rect| r.width;
    let height = |r: Rect| r.height;
    let cases: [(SplitDir, &dyn Fn(Rect) -> u16); 2] = [
        (SplitDir::Vertical, &width),
        (SplitDir::Horizontal, &height),
    ];
    for (dir, extent) in cases {
        for sizing in [
            Sizing::Ratio(25),
            Sizing::Ratio(70),
            Sizing::FirstCells(12),
            Sizing::SecondCells(7),
        ] {
            let mut a = Layout::single(p(1));
            a.split(p(1), p(2), dir, sizing);
            let mut b = Layout::single(p(1));
            b.split(p(1), p(2), dir, sizing.mirrored());

            let (first_a, second_b) = (extent(a.compute(area)[0].1), extent(b.compute(area)[1].1));
            assert_eq!(
                first_a, second_b,
                "{sizing:?} on {dir:?}: the mirror must give `second` what the original gave `first`"
            );
        }
    }
    // And it is an involution, so flipping twice is where you started.
    for s in [
        Sizing::Ratio(30),
        Sizing::FirstCells(9),
        Sizing::SecondCells(4),
    ] {
        assert_eq!(s.mirrored().mirrored(), s, "{s:?} round-trips");
    }
}

// --- `<C-w>=` and `<C-w>+`: what vim means, at three panes ----------------

/// **Three panes come out in thirds, not 25/25/50.**
///
/// Two `:vs` from one pane nest as `((A|B)|C)`, and `equalize_ratios` set every
/// split to 50 — so the left subtree took half and split it, leaving A and B at
/// a quarter each while C kept half. Steven hit exactly this dogfooding:
/// pressing the key that means "make these even" left two panes half the width
/// of the third.
///
/// The `n = 2` shortcut again: 50-at-every-split agrees with "equal windows"
/// when both subtrees hold one pane, and diverges the moment one holds two.
#[test]
fn equalize_makes_three_panes_equal_not_a_quarter_each() {
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50));
    l.split(p(2), p(3), SplitDir::Vertical, Sizing::Ratio(50));
    assert_eq!(l.leaf_count(), 3, "premise: three panes");

    l.equalize_ratios();

    let rects = l.compute(area(90, 10));
    let widths: Vec<u16> = rects.iter().map(|(_, r)| r.width).collect();
    let (lo, hi) = (*widths.iter().min().unwrap(), *widths.iter().max().unwrap());
    // Two cells of slack: 90 into thirds is 30 each, and integer ratios at two
    // levels round independently. The OLD rule gave 22/22/45 here — a spread of
    // 23 — so this discriminates by a wide margin without pretending the
    // arithmetic is exact.
    assert!(
        hi - lo <= 2,
        "every pane gets a third, give or take rounding: {widths:?}"
    );
    assert!(
        hi < lo * 2,
        "and no pane is twice another, which is what 25/25/50 looked like: \
         {widths:?}"
    );
}

/// Two panes still come out halves — the case the old rule got right, kept so
/// the fix is a generalisation rather than a swap.
#[test]
fn equalize_still_halves_two_panes() {
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(80));

    l.equalize_ratios();

    let widths: Vec<u16> = l
        .compute(area(80, 10))
        .iter()
        .map(|(_, r)| r.width)
        .collect();
    assert_eq!(widths, vec![40, 40], "halves");
}

/// **`<C-w>+` finds the nearest horizontal divider ABOVE the pane**, rather
/// than only its immediate parent.
///
/// With three transcripts side by side over a prompt — `((A|B)|C)` above `P` —
/// a transcript's immediate parent is a *vertical* split, so a height change
/// matched nothing and did nothing, silently. Vim walks up to the closest
/// horizontal divider, which is the one between the transcripts and the prompt.
#[test]
fn a_height_change_walks_up_to_the_nearest_horizontal_divider() {
    // The prompt goes under the ROOT first, then the transcripts split inside
    // it — otherwise the divider lands under one column instead of all three,
    // which is a different layout and a test of nothing.
    let mut l = Layout::single(p(1));
    l.split(p(1), p(9), SplitDir::Horizontal, Sizing::SecondCells(3));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50));
    l.split(p(2), p(3), SplitDir::Vertical, Sizing::Ratio(50));

    // The immediate parent of pane 1 is vertical, so the old question said no.
    assert_eq!(l.dir_around(p(1)), Some(SplitDir::Vertical));
    // The new one finds the horizontal split above it.
    assert_eq!(
        l.sizing_along(p(1), SplitDir::Horizontal),
        Some(Sizing::SecondCells(3)),
        "the transcript/prompt divider is what sets a transcript's height"
    );
    assert!(
        !l.is_second_along(p(1), SplitDir::Horizontal),
        "and the transcripts are on the FIRST side of it"
    );

    assert!(l.set_sizing_along(p(1), SplitDir::Horizontal, Sizing::SecondCells(6)));
    assert_eq!(
        l.sizing_along(p(2), SplitDir::Horizontal),
        Some(Sizing::SecondCells(6)),
        "and it is genuinely one divider — its sibling sees the same change"
    );
}

/// With no divider on that axis at all there is nothing to move, and saying so
/// would be noise: vim is silent here too.
#[test]
fn a_height_change_with_no_horizontal_divider_finds_nothing() {
    let mut l = Layout::single(p(1));
    l.split(p(1), p(2), SplitDir::Vertical, Sizing::Ratio(50));
    assert_eq!(l.sizing_along(p(1), SplitDir::Horizontal), None);
    assert!(!l.set_sizing_along(p(1), SplitDir::Horizontal, Sizing::FirstCells(5)));
}
