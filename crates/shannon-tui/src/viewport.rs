//! Reusable scroll/viewport state.
//!
//! Two modes:
//! - **follow tail**: viewport tracks the bottom of the content
//!   automatically; appending new content keeps it visible.
//! - **frozen**: user has scrolled back; viewport stays at the chosen
//!   top regardless of new appends, until they snap back to tail.
//!
//! The modes are mutually exclusive. A frozen top is TEXT — a line and a
//! column — and the row it stands on is derived where it is read
//! (`Output::top_row`), from the content, the width, the window's folds and
//! the rule, which is what makes a row.
//!
//! The `Viewport` struct is `Output`'s. The free functions are every
//! window's: the transcript (`Output`), a text page (`TextView`), `:graph`
//! (`GraphView`), and the prompt (`Prompt::settle`, `place`,
//! `screen_line`) — the last joined 10-15, when its cursor-derived top was
//! replaced by a stored one (it had been promised here since M1). The point
//! of pulling this out is that no window re-types a scroll rule.

/// **Where a transcript window looks** — following the end, or frozen on
/// the text at its top.
///
/// **The top is a line, not a row** (S21-Q10, review stop 15, D's HIGH 1).
/// A row is derived from four inputs — the content, the width, the
/// window's folds and the rule — and kept as state it went stale whenever
/// one changed with no one to re-anchor it: a `:vsplit` read a window laid
/// at the old width (A H1), `zR`/`zM` and `:set tui.fold` moved the top
/// (A M5), a pane with no room put it at the start for good (A M6). Kept as
/// the text, the row is read off whatever the inputs are now, as vim keeps
/// `topline`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    /// The logical line at the top and the column of its top row's first
    /// character — a row part-way down a wrapped line is the row holding
    /// that column at any width. Only consulted when `follow_tail ==
    /// false`; clamped to the last page at read time, so it outlives the
    /// lines it named.
    top: (usize, usize),
    follow_tail: bool,
}

/// **Where the window sits so that `position` stays comfortably inside it** —
/// vim's `scrolloff`, as one function.
///
/// Returns the new first-visible index, given where the window is now. The
/// cursor pushes the view rather than the view dragging the cursor: near the
/// top, moving up scrolls up; near the bottom, moving down scrolls down; in the
/// middle nothing moves at all, which is the part that makes reading feel
/// still.
///
/// `scrolloff` is a soft margin, clamped to half the window so an oversized
/// value degrades to keep-centred (vim's `so=999` idiom) instead of fighting
/// itself. Everything is in **rows of whatever the caller is counting** —
/// wrapped visual rows for a transcript, tree rows for the graph — because the
/// rule does not care, and pretending it does is what produced two of it.
///
/// **It was written twice**, in `Output::follow_cursor` and
/// `GraphView::window_top`, independently and identically: same margin clamp,
/// same band, same stay-put-inside-it, same last-page clamp, different
/// spellings. Two correct copies of a non-trivial rule is the shape that rots —
/// one gets a fix and the other does not, and nothing fails
/// (`[[feedback_rules_rot_by_retyping]]`). A third caller (`TextView`, which
/// had no follow rule at all) is what made the duplication worth removing
/// rather than noting.
pub fn follow(
    position: usize,
    current_top: usize,
    len: usize,
    height: usize,
    scrolloff: usize,
) -> usize {
    if height == 0 {
        return 0;
    }
    let margin = scrolloff.min((height - 1) / 2);
    // The band of tops that keeps `position` outside both margins. The clamp on
    // `margin` guarantees `lo <= hi`.
    //
    // Both ends subtract FROM `position` rather than adding to it. The obvious
    // spelling of `lo` is `(position + margin + 1).saturating_sub(height)`,
    // which is the same number and overflows on the way there — plain addition,
    // and `usize` addition panics in dev/test where overflow checks are on. The
    // comment beside it used to say "saturation included", which was true of the
    // result and not of how it was reached (Reviewer B).
    //
    // `height - margin - 1` cannot underflow: `height >= 1` after the early
    // return, and `margin <= (height - 1) / 2 <= height - 1`.
    let lo = position.saturating_sub(height - margin - 1);
    let hi = position.saturating_sub(margin);
    // Stay put while already inside the band — the viewport must not creep when
    // the cursor moves through its middle — then keep the last page full.
    //
    // **A cursor never pushes the window past the end, but a window already
    // past it stays.** `zt` on the last lines scrolls past the end (vim does;
    // Steven uses it to bring text to eye level), and the next `k` must not
    // snap it back to a full last page — vim leaves it until the cursor
    // actually needs the window to move. So the ceiling is the last full page
    // OR wherever the window already is, whichever is further. For every window
    // that is not past the end this is exactly the old `min(len - height)`.
    let full_page = len.saturating_sub(height);
    current_top.clamp(lo, hi).min(full_page.max(current_top))
}

/// A line picked by its place on the SCREEN rather than in the buffer — vim's
/// `H`, `M` and `L`. The count is a distance from the edge (`3H` is the third
/// line from the top), not a repeat; `1` is the edge itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenLine {
    Top(usize),
    Middle,
    Bottom(usize),
}

/// **Which line `H`/`M`/`L` land on**, given the lines `first..=last` currently
/// on screen, the buffer's `len` lines, and `scrolloff`.
///
/// Measured against Neovim (`--clean`, a 22-row window showing lines 30–51 of
/// 100): `H`→30, `3H`→32, `99H`→51, `M`→40, `L`→51, `3L`→49; with
/// `scrolloff=5` and lines 25–46 on screen, `H`→30 and `L`→41; at the top of the
/// buffer `H` stays on line 1; on a 7-line buffer `M`→4 and `L`→7 — the middle
/// of the TEXT shown, not of the window (`:h H`, `:h M`, `:h L`).
///
/// `scrolloff` pulls the target in from an edge unless that edge is the
/// buffer's own start or end, and **not at all when an operator is pending**
/// ("`yH` yanks from the first visible line") — `operator` says which. The
/// margin is clamped to half the lines shown, as `follow`'s is, so the two
/// rules cannot disagree about where the band is.
///
/// All indexes are the caller's LINE indexes, 0-based. Like `follow`, it does
/// not care what a line is — a source line, a tree row.
pub fn screen_line(
    which: ScreenLine,
    first: usize,
    last: usize,
    len: usize,
    scrolloff: usize,
    operator: bool,
) -> usize {
    let last = last.min(len.saturating_sub(1)).max(first);
    let (lo, hi) = band(first, last, len, scrolloff, operator);
    let target = match which {
        ScreenLine::Top(n) => first.saturating_add(n.saturating_sub(1)),
        ScreenLine::Middle => first + (last - first) / 2,
        ScreenLine::Bottom(n) => last.saturating_sub(n.saturating_sub(1)).max(first),
    };
    target.clamp(lo, hi)
}

/// **Where a cursor at `position` goes when the window `first..=last` moved
/// under it** — vim's rule for `<C-e>`, `<C-y>` and the wheel: the window
/// scrolls and the cursor stays where it was, unless that is outside the
/// `scrolloff` band, and then it is dragged to the band's nearer edge.
///
/// The band is [`screen_line`]'s, so `H` and a scroll cannot disagree about
/// where a cursor may rest; and it is the band [`follow`] keeps the cursor in,
/// so a dragged cursor never moves the window back (`follow` of the result is
/// the window it was dragged into — asserted in this module's tests).
pub fn drag(position: usize, first: usize, last: usize, len: usize, scrolloff: usize) -> usize {
    let last = last.min(len.saturating_sub(1)).max(first);
    let (lo, hi) = band(first, last, len, scrolloff, false);
    position.clamp(lo, hi)
}

/// The lines `first..=last` that a cursor may rest on, `scrolloff` in from
/// each edge that is not the buffer's own — `lo <= hi` always. `last` is
/// already clamped into the buffer by the caller.
fn band(first: usize, last: usize, len: usize, scrolloff: usize, operator: bool) -> (usize, usize) {
    let end = len.saturating_sub(1);
    let shown = last - first + 1;
    let margin = if operator {
        0
    } else {
        scrolloff.min((shown - 1) / 2)
    };
    let lo = if first == 0 { first } else { first + margin };
    let hi = if last >= end { last } else { last - margin };
    (lo.min(hi), hi)
}

/// **The lines on screen**, first and last, for lines `heights` rows tall
/// shown `visible` rows at a time from row `top` — `None` with no lines.
///
/// The window counts ROWS and may start part-way down a wrapped line; vim
/// never shows a line cut at an edge, and `H`/`L` go to lines that are on
/// screen. So a line cut off at either edge does not count as shown, unless
/// it is the only one (a line taller than the window).
///
/// Its three consumers count lines differently and rows the same: a text
/// page (`TextView`, source lines), a transcript (`Output`, logical lines)
/// and the prompt (`Prompt`, its draft's lines). It was `TextView`'s alone
/// until the transcript took `H`/`M`/`L` (10-04); this said "two" after the
/// prompt took them (review stop 12, T6 12).
pub fn lines_on_screen(
    heights: impl IntoIterator<Item = usize>,
    top: usize,
    visible: usize,
) -> Option<(usize, usize)> {
    let bottom = top + visible.max(1);
    // (line, the row it starts on) of the first line reaching into the window,
    // and (line, the row it ends before) of the last.
    let (mut first, mut last) = (None, (0, 0));
    let mut row = 0;
    for (line, rows) in heights.into_iter().enumerate() {
        if row >= bottom {
            break;
        }
        let end = row + rows;
        if end > top {
            first.get_or_insert((line, row));
            last = (line, end);
        }
        row = end;
    }
    let (mut first, starts) = first?;
    let (mut last, ends) = last;
    if first < last && starts < top {
        first += 1;
    }
    if last > first && ends > bottom {
        last -= 1;
    }
    Some((first, last))
}

/// Where the cursor's line goes in the window — vim's `zt`, `zz`, `zb` (and
/// `z<CR>`, `z.`, `z-`, which land the same way).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Top,
    Middle,
    Bottom,
}

/// **The window top that puts a line at `place`**, for a line occupying rows
/// `at..at + rows`, shown `height` rows at a time.
///
/// Measured against Neovim (22-row window, the cursor on line 50): `zt`→top
/// 50, `zz`→40, `zb`→29; with `scrolloff=5`, `zt`→45 and `zb`→34 while `zz` is
/// unmoved; at line 3, `zb` and `zz` stop at the top of the buffer; and `zt`
/// on line 95 of 100 puts it at the top with `~` below (`:h scroll-cursor`).
///
/// **Past the end is allowed**, as in vim — Steven uses `zt`/`z.` on the last
/// lines to bring text to eye level. No clamp is needed for it to stay
/// sensible: every placement keeps the window at or above the cursor's own
/// line (`top <= at`), so the line is always on screen and the pane is never
/// blank. `follow` then leaves such a window where it is.
///
/// In ROWS of whatever the caller counts, like `follow`: a wrapped line is
/// several.
pub fn place_top(place: Place, at: usize, rows: usize, height: usize, scrolloff: usize) -> usize {
    if height == 0 {
        return 0;
    }
    let margin = scrolloff.min((height - 1) / 2);
    match place {
        Place::Top => at.saturating_sub(margin),
        Place::Middle => (at + rows / 2).saturating_sub((height - 1) / 2),
        Place::Bottom => (at + rows.max(1) + margin).saturating_sub(height),
    }
}

/// **The furthest a window's top may sit**, for content `rows` tall whose last
/// line starts at row `last_line_at`, shown `height` rows at a time.
///
/// Past the end survives a frame but not a resize — measured against Neovim:
/// `zt` on line 95 of 100 stays put through every redraw and every cursor move
/// inside the window, and growing the window until the page fits snaps it back
/// to line 1. So a NEW geometry refills the last page, and an unchanged one
/// allows anything up to the last line at the top, which is where a `zt` may
/// have put it ([`place_top`]).
///
/// Its own function because three windows keep a top across frames — a
/// text page (`TextView`, in wrapped rows), `:graph` (in tree rows) and the
/// prompt (`Prompt`, in its spans) — and the rule was measured once (this
/// said "two": review stop 12, T6 12).
pub fn top_ceiling(rows: usize, last_line_at: usize, height: usize, resized: bool) -> usize {
    if resized {
        rows.saturating_sub(height.max(1))
    } else {
        last_line_at
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Self::following_tail()
    }
}

impl Viewport {
    pub fn following_tail() -> Self {
        Self {
            top: (0, 0),
            follow_tail: true,
        }
    }

    pub fn is_following_tail(&self) -> bool {
        self.follow_tail
    }

    /// Re-anchor to the latest content. Idempotent. Reached through
    /// `Output::put_top_row`, the one door for placing the view by row.
    pub fn snap_to_tail(&mut self) {
        self.follow_tail = true;
    }

    /// **The text at its top** — a logical line and a column; `None` while
    /// it follows the end, where the end decides.
    pub fn top(&self) -> Option<(usize, usize)> {
        (!self.follow_tail).then_some(self.top)
    }

    /// Stop following; the top on `line`, at the row holding `col`.
    ///
    /// Not the door for "put the view at this row" — that is
    /// `Output::put_top_row`, which owns the rule that arriving at the end
    /// is following it (`G` once stopped new replies being shown by
    /// freezing there: `[[feedback_one_door_per_invariant]]`). This is for
    /// a place found as text: a carried window's top, a test's.
    pub fn freeze_at(&mut self, line: usize, col: usize) {
        self.top = (line, col);
        self.follow_tail = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_follows_tail() {
        let v = Viewport::default();
        assert!(v.is_following_tail());
    }

    /// Frozen, it has a top — the text it was frozen on — and following, none:
    /// the end decides. (The row rules — clamped to the last page, a scroll
    /// that re-engages the end — are `Output`'s, where a row can be derived:
    /// its `a_top_*` and `scroll_*` tests.)
    #[test]
    fn a_frozen_view_holds_its_text_and_a_following_one_none() {
        let mut v = Viewport::following_tail();
        assert_eq!(v.top(), None);
        v.freeze_at(7, 40);
        assert!(!v.is_following_tail());
        assert_eq!(v.top(), Some((7, 40)));
        v.snap_to_tail();
        assert!(v.is_following_tail());
        assert_eq!(v.top(), None);
    }

    /// **The two hand-written copies and the extracted rule agree everywhere.**
    ///
    /// This is what makes the extraction safe to make, and it is deliberately a
    /// *differential* test rather than a set of examples: an extraction is only
    /// correct if the new function agrees with the old code on every input, and
    /// examples check the inputs whoever wrote them thought of.
    ///
    /// The two originals are transcribed here rather than called, because the
    /// point of the change is that they stop existing — a test that called them
    /// would have to keep them alive to keep passing, which is the extraction
    /// that never finishes ([[feedback_fix_neighbourhood]]: an extraction is
    /// done when the old copies are gone, not when the abstraction exists).
    fn original_output_rule(pos: usize, top: usize, len: usize, h: usize, so: usize) -> usize {
        if h == 0 {
            return 0;
        }
        let max_top = len.saturating_sub(h);
        let margin = so.min(h.saturating_sub(1) / 2);
        let mut top = top.min(max_top);
        if pos < top + margin {
            top = pos.saturating_sub(margin);
        }
        let last_visible = top + h.saturating_sub(1);
        if pos + margin > last_visible {
            top = (pos + margin + 1).saturating_sub(h);
        }
        top.min(max_top)
    }

    fn original_graph_rule(pos: usize, top: usize, len: usize, h: usize, so: usize) -> usize {
        if h == 0 || len <= h {
            return 0;
        }
        let off = so.min((h - 1) / 2);
        let lo = (pos + off + 1).saturating_sub(h);
        let hi = pos.saturating_sub(off);
        top.clamp(lo, hi).min(len - h)
    }

    proptest::proptest! {
        #[test]
        fn the_extracted_follow_rule_matches_both_originals(
            pos in 0usize..200,
            top in 0usize..200,
            len in 0usize..200,
            h in 0usize..40,
            so in 0usize..30,
        ) {
            // Only meaningful where the position is actually in the document;
            // both originals are called with an in-range cursor by construction.
            let pos = if len == 0 { 0 } else { pos.min(len - 1) };
            // And a window that is not already past the end. The originals
            // snapped ANY top back to a full last page; `follow` now leaves a
            // window a `zt` put past the end where it is (vim's rule, measured —
            // `follow_keeps_a_past_end_window_and_never_makes_one`), so the two
            // are only the same rule where no window is past the end.
            let top = top.min(len.saturating_sub(h));
            let want_graph = original_graph_rule(pos, top, len, h, so);
            let want_output = original_output_rule(pos, top, len, h, so);
            let got = follow(pos, top, len, h, so);
            proptest::prop_assert_eq!(got, want_graph, "graph rule disagrees");
            proptest::prop_assert_eq!(got, want_output, "output rule disagrees");
        }

        /// **`follow` does not panic anywhere in its domain.**
        ///
        /// Separate from the differential test above, and it has to be. That
        /// one clamps `pos` into the document (`pos.min(len - 1)`), because
        /// both transcribed originals are only *defined* for an in-range
        /// cursor — so widening its generators reaches nothing, which is a
        /// property test that has quietly become a large example-based one
        /// ([[feedback_assert_the_subject]]: I widened the range first and it
        /// measured nothing).
        ///
        /// This one asserts the weaker thing over the WHOLE domain, which is
        /// what catches arithmetic. `follow` is documented as a shared
        /// primitive for further reuse; its callers' current bounds are not
        /// part of its contract. Reviewer B found `usize::MAX` overflowing an
        /// intermediate `position + margin + 1` — and `usize` addition panics
        /// in dev/test, where overflow checks are on.
        #[test]
        fn follow_never_panics_on_any_input(
            pos in proptest::prop_oneof![0usize..300, (usize::MAX - 300)..=usize::MAX],
            top in proptest::prop_oneof![0usize..300, (usize::MAX - 300)..=usize::MAX],
            len in proptest::prop_oneof![0usize..300, (usize::MAX - 300)..=usize::MAX],
            h in proptest::prop_oneof![0usize..60, (usize::MAX - 60)..=usize::MAX],
            so in proptest::prop_oneof![0usize..60, (usize::MAX - 60)..=usize::MAX],
        ) {
            let top_out = follow(pos, top, len, h, so);
            // And it never pushes a window past the last full page: past it
            // only if it already was, and never further (a `zt` may leave it
            // there; a cursor move must not take it further).
            proptest::prop_assert!(top_out <= len.saturating_sub(h).max(top));
        }
    }
}

/// Every expectation below is a number read off real Neovim (`--clean`,
/// headless, a 22-row window — `reference_nvim_oracle`), converted to 0-based.
#[cfg(test)]
mod screen_relative_tests {
    use super::*;

    const LEN: usize = 100;

    /// Lines 30–51 on screen (0-based 29..=50), `scrolloff=0`.
    #[test]
    fn h_m_l_land_where_vim_lands() {
        let at = |w| screen_line(w, 29, 50, LEN, 0, false) + 1;
        assert_eq!(at(ScreenLine::Top(1)), 30, "H");
        assert_eq!(at(ScreenLine::Top(3)), 32, "3H");
        assert_eq!(at(ScreenLine::Top(99)), 51, "99H clamps to the window");
        assert_eq!(at(ScreenLine::Middle), 40, "M");
        assert_eq!(at(ScreenLine::Bottom(1)), 51, "L");
        assert_eq!(at(ScreenLine::Bottom(3)), 49, "3L");
    }

    /// `scrolloff=5`, lines 25–46 on screen: pulled five in from each edge —
    /// but not past the buffer's own start, and not with an operator pending.
    #[test]
    fn scrolloff_pulls_h_and_l_in_except_at_the_buffer_edge() {
        assert_eq!(
            screen_line(ScreenLine::Top(1), 24, 45, LEN, 5, false) + 1,
            30,
            "H"
        );
        assert_eq!(
            screen_line(ScreenLine::Bottom(1), 24, 45, LEN, 5, false) + 1,
            41,
            "L"
        );
        assert_eq!(
            screen_line(ScreenLine::Top(1), 0, 21, LEN, 5, false) + 1,
            1,
            "H at line 1"
        );
        assert_eq!(
            screen_line(ScreenLine::Top(1), 24, 45, LEN, 5, true) + 1,
            25,
            "yH: an operator reaches the first visible line"
        );
    }

    /// A 7-line buffer in a 22-row window: the middle of the text SHOWN.
    #[test]
    fn m_is_the_middle_of_the_text_not_of_the_window() {
        assert_eq!(
            screen_line(ScreenLine::Middle, 0, 21, 7, 0, false) + 1,
            4,
            "M"
        );
        assert_eq!(
            screen_line(ScreenLine::Bottom(1), 0, 21, 7, 0, false) + 1,
            7,
            "L"
        );
    }

    /// The cursor on line 50 (row 49), one row a line, 22 rows high.
    #[test]
    fn zt_zz_zb_place_the_line_where_vim_does() {
        let top = |p, so| place_top(p, 49, 1, 22, so) + 1;
        assert_eq!(top(Place::Top, 0), 50, "zt");
        assert_eq!(top(Place::Middle, 0), 40, "zz");
        assert_eq!(top(Place::Bottom, 0), 29, "zb");
        assert_eq!(top(Place::Top, 5), 45, "zt, scrolloff=5");
        assert_eq!(top(Place::Middle, 5), 40, "zz ignores scrolloff");
        assert_eq!(top(Place::Bottom, 5), 34, "zb, scrolloff=5");
        assert_eq!(place_top(Place::Bottom, 2, 1, 22, 0) + 1, 1, "zb at line 3");
        assert_eq!(place_top(Place::Middle, 2, 1, 22, 0) + 1, 1, "zz at line 3");
    }

    /// **Past the end, as vim does** (Steven: he uses it to bring text to eye
    /// level): `zt` on line 95 of 100 puts it at the top, `~` below; `z.` on
    /// the last line puts it mid-window.
    #[test]
    fn zt_near_the_end_scrolls_past_it() {
        assert_eq!(place_top(Place::Top, 94, 1, 22, 0) + 1, 95, "vim's 95");
        assert_eq!(
            place_top(Place::Middle, 99, 1, 22, 0) + 1,
            90,
            "z. on the last line"
        );
    }

    /// **A window already past the end stays** when the cursor moves inside it
    /// — vim does not snap back to a full last page — and a cursor never pushes
    /// a window past the end on its own. 100 lines, 22 rows, top at row 94.
    #[test]
    fn follow_keeps_a_past_end_window_and_never_makes_one() {
        assert_eq!(
            follow(93, 94, LEN, 22, 0),
            93,
            "k above the top: scroll up by one"
        );
        assert_eq!(
            follow(99, 94, LEN, 22, 0),
            94,
            "moving inside it: stays past the end"
        );
        assert_eq!(
            follow(99, 70, LEN, 22, 0),
            78,
            "G from above: a full last page, not past"
        );
    }

    /// A wrapped line is several rows; `zb` puts its LAST row at the bottom.
    #[test]
    fn a_wrapped_line_is_placed_by_its_rows() {
        assert_eq!(
            place_top(Place::Bottom, 40, 3, 22, 0),
            21,
            "rows 40..43 end on row 21's bottom"
        );
        assert_eq!(place_top(Place::Top, 40, 3, 22, 0), 40);
    }

    /// **The window moved; the cursor stays unless it fell out of the band**
    /// — `scrolloff` in from each edge that is not the buffer's own. (Lines
    /// 30–51 of 100 on screen, the same window as `H`'s measurements.)
    #[test]
    fn a_scroll_drags_the_cursor_only_out_of_the_band() {
        assert_eq!(drag(40, 30, 51, 100, 5), 40, "inside: it stays");
        assert_eq!(drag(31, 30, 51, 100, 5), 35, "above: to the band's top");
        assert_eq!(drag(12, 30, 51, 100, 5), 35, "far above too");
        assert_eq!(drag(50, 30, 51, 100, 5), 46, "below: to the band's bottom");
        assert_eq!(
            drag(2, 0, 21, 100, 5),
            2,
            "the buffer's start has no margin"
        );
        assert_eq!(drag(99, 78, 99, 100, 5), 99, "nor its end");
        assert_eq!(drag(40, 30, 51, 100, 0), 40, "no scrolloff");
        assert_eq!(drag(60, 30, 51, 100, 0), 51);
    }

    /// **A dragged cursor never pushes the window back** — `drag` and
    /// [`follow`] are one band, read in two directions. Exhaustive over every
    /// valid window of a small buffer, every window height, every margin up to
    /// and past half the window, and every position: if they disagreed
    /// anywhere, a wheel notch would scroll and the next `j` would scroll it
    /// back.
    ///
    /// **The SAME band, not one inside the other**: a cursor `follow` leaves
    /// the window still for is exactly one `drag` leaves where it is. It
    /// checked only that the dragged cursor was inside, at one height (10),
    /// and a band one row too narrow passed it and every other test (review,
    /// Reviewer C).
    #[test]
    fn a_dragged_cursor_is_a_fixed_point_of_follow() {
        for len in [1usize, 7, 10, 23, 40] {
            for h in 1..=12usize {
                for top in 0..=len.saturating_sub(h) {
                    for scrolloff in 0..=12 {
                        let last = top + h - 1;
                        for position in 0..len + 3 {
                            let at = drag(position, top, last, len, scrolloff);
                            let case = format!(
                                "len {len}, h {h}, top {top}, so {scrolloff}: {position} → {at}"
                            );
                            assert_eq!(follow(at, top, len, h, scrolloff), top, "{case}");
                            if position < len {
                                let still = follow(position, top, len, h, scrolloff) == top;
                                assert_eq!(still, at == position, "one band: {case}");
                            }
                        }
                    }
                }
            }
        }
    }

    /// **Which lines are on screen**: a line cut off at an edge is not,
    /// unless it is the only one — rows 3..13 of lines 4, 4, 4, 4 rows tall.
    #[test]
    fn a_line_cut_at_an_edge_is_not_on_screen() {
        let four = || [4, 4, 4, 4].into_iter();
        // Rows 3..13: line 0 (rows 0..4) cut at the top, line 3 (12..16) at
        // the bottom — lines 1 and 2 are the whole ones.
        assert_eq!(lines_on_screen(four(), 3, 10), Some((1, 2)));
        assert_eq!(lines_on_screen(four(), 0, 8), Some((0, 1)), "edges exact");
        assert_eq!(lines_on_screen(four(), 0, 16), Some((0, 3)));
        // One line taller than the window: it is what is on screen.
        assert_eq!(lines_on_screen([30].into_iter(), 5, 10), Some((0, 0)));
        // Two lines, both cut: the top one is dropped, and the bottom one is
        // then the only one left, so it stays (`TextView`'s rule, unchanged).
        assert_eq!(lines_on_screen([10, 10].into_iter(), 5, 10), Some((1, 1)));
        assert_eq!(lines_on_screen(std::iter::empty(), 0, 10), None);
    }
}
