//! Layout constants, and the shape a frame is divided into.
//!
//! **This module used to own the split.** `split_frame` computed a fixed
//! output/prompt/status division and was the single source both `view.rs` and
//! `app.rs` consulted. The pane/window rewrite removed that job: panes are now
//! arbitrary in number and order, so the division is a walk over
//! [`wm::Layout`](crate::wm) and it lives with the tree that knows the
//! arrangement — [`Workspace::frame_layout`](crate::workspace::Workspace).
//!
//! What is left here is the *shape* of the answer plus the border arithmetic —
//! the part every pane renderer and every scroll calculation must agree on. The
//! producer moved; the vocabulary did not, because a renderer should not have to
//! know how the tree reached its conclusion.
//!
//! Rebuilt 2026-08-15 during the recovery: no copy of the post-rewrite file
//! survived, so it is derived from its consumers (`view.rs`, `workspace.rs`,
//! `app.rs`, `app/tests.rs`), which are complete.

use ratatui::layout::Rect;

use crate::wm::PaneId;
use crate::workspace::PaneKind;

/// Border contribution to a pane's outer dimensions, in
/// `(height, width)` order. `BORDER_SHAPE.0` is the vertical
/// contribution (1 row top + 1 row bottom), `BORDER_SHAPE.1` is the
/// horizontal (1 col left + 1 col right). Tuple instead of two named
/// consts so the call sites stay terse but the intent (separate
/// dimensions) is encoded in the type. If we ever go asymmetric or
/// borderless this is the single source.
pub const BORDER_SHAPE: (u16, u16) = (2, 2);

/// One pane, placed: where it is, what it shows, and how the user addresses it.
///
/// Produced by [`Workspace::rects`](crate::workspace::Workspace::rects), which
/// derives every field from the same walk — in particular [`number`](Self::number)
/// is this list's position rather than a separately-maintained counter, so the
/// number the border shows and the number `{count}<C-w>w` takes cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneRect {
    /// Which pane this is. The identity every command and every lookup keys on.
    pub id: PaneId,
    /// What it is currently showing.
    pub kind: PaneKind,
    /// Its outer rectangle, borders included.
    pub rect: Rect,
    /// Whether it holds focus. Exactly one pane in a frame has this set.
    pub focused: bool,
    /// 1-based ring position — what the border shows and `{count}<C-w>w` takes.
    pub number: usize,
}

impl PaneRect {
    /// Content area inside the border, as `(width, height)`.
    ///
    /// The renderers and the scroll math must agree on what "content" means, so
    /// neither subtracts the border itself.
    pub fn content_size(&self) -> (u16, u16) {
        content_size(self.rect)
    }
}

/// A whole frame, divided: every pane the tree placed, plus the status row.
///
/// Not `Copy` — it carries the pane list, whose length is the point.
#[derive(Debug, Clone)]
pub struct FrameLayout {
    /// Every pane, in ring order. Ring order is what `<C-w>w` walks and what
    /// [`PaneRect::number`] indexes, so iteration order is meaningful and must
    /// not be re-sorted by a consumer.
    pub panes: Vec<PaneRect>,
    /// The tab line along the top — no rows with one tab (vim's
    /// `showtabline=1`), one with more. Never a pane.
    pub tabline: Rect,
    /// The bottom row. Always present, always one row, never a pane — it
    /// belongs to no buffer and cannot take focus.
    pub status: Rect,
}

impl FrameLayout {
    /// Where a given pane landed this frame, if it is on screen at all.
    ///
    /// `Option` rather than a panic because a `PaneId` can outlive its pane: a
    /// teardown can close the pane a queued action still names, and the honest
    /// answer is "it is not here" rather than an index into a stale list
    /// (`[[feedback_teardown_doors]]`).
    pub fn pane(&self, id: PaneId) -> Option<&PaneRect> {
        self.panes.iter().find(|p| p.id == id)
    }

    /// The first pane showing `kind`, in ring order.
    ///
    /// Test-only: production callers ask the workspace, which owns the slot
    /// list; this exists so a geometry assertion can start from what was
    /// actually drawn.
    ///
    /// "First" is well defined because [`panes`](Self::panes) is in ring order,
    /// the same order `<C-w>w` walks — so the pane this returns is the one a
    /// user would call the first one. A `kind` can legitimately be on screen
    /// more than once (two transcripts, say); callers that care about all of
    /// them ask the workspace instead.
    #[cfg(test)]
    pub fn first_of(&self, kind: PaneKind) -> Option<&PaneRect> {
        self.panes.iter().find(|p| p.kind == kind)
    }
}

/// Content area (inside the border) of a pane `Rect`, as `(width,
/// height)`. The single place the border subtraction lives, so every
/// pane renderer and the scroll math agree on what "content" means.
pub fn content_size(area: Rect) -> (u16, u16) {
    (
        area.width.saturating_sub(BORDER_SHAPE.1),
        area.height.saturating_sub(BORDER_SHAPE.0),
    )
}
