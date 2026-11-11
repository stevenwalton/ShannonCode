//! **Visual over rows** — what `v`, `V` and `<C-v>` mean in a pane whose
//! cursor is a row: a `:graph` row ([`GraphView`]).
//!
//! # Why this exists
//!
//! Visual existed in the two panes with a `Buffer` — the prompt and the
//! transcript — and was refused everywhere else, so `:graph`, a list you
//! delete from, had no way to select a range (Steven, 2026-10-04: "visual
//! mode no longer works here… sign of bad sharing/primitives for the
//! panes"). Each row pane already had its own cursor; what was missing was
//! the other end. One trait, so the app answers "start / drop / swap a
//! selection" through one door for every row pane, rather than once per kind.
//!
//! A row has no columns, so all three Visual keys select whole rows. The
//! keymap's mode label still says which key was pressed; the geometry here
//! is linewise either way.
//!
//! The text pages were row panes too, until they took the transcript's
//! `Buffer` (Steven, 10-07: select part of a line in `:help`, yank it,
//! paste it into a prompt) — a page is text, so it has columns.
//!
//! Not a row pane, deliberately: the decision pane, whose `o` and `d` are
//! answers.
//!
//! [`GraphView`]: crate::graph_view::GraphView

use std::ops::RangeInclusive;

/// A pane whose cursor is a row and whose Visual selection is whole rows.
///
/// Positions are the pane's own cursor positions — visible rows in the
/// graph — so [`selection`](Self::selection) is what the renderer highlights
/// and what a verb acts on, in the same units.
pub(crate) trait RowSelect {
    /// Anchor a selection at the cursor (`v`, `V`, `<C-v>`).
    fn start_selection(&mut self);
    /// Drop the selection (`Esc`, a verb that took it, focus leaving).
    fn clear_selection(&mut self);
    /// Swap the cursor and the anchor (`o`): extend from the other end.
    /// Nothing without a selection.
    fn swap_selection_ends(&mut self);
    /// The selected rows, both ends included; `None` without a selection.
    fn selection(&self) -> Option<RangeInclusive<usize>>;
}

/// The rows between two ends, in either order, both included.
pub(crate) fn between(a: usize, b: usize) -> RangeInclusive<usize> {
    a.min(b)..=a.max(b)
}
