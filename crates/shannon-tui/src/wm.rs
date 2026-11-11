//! The window-manager core: how viewports are arranged, focused, and overlaid.
//!
//! ratatui is immediate-mode — it hands us a `Frame` and `Rect`s and the `Clear`
//! widget, and *no* window system (see `docs/Milestones/Milestone 5.md`). So the
//! model is ours to build, and this module is the pure, content-agnostic heart of
//! it: a **layout tree** of tiled panes, a **focus** ring over them, and an
//! **overlay stack** for floats/modals. It knows nothing about *what* a pane
//! shows — a pane is a [`PaneId`], a buffer is a [`BufferId`], and the
//! id→content mapping lives in the app. That separation is the whole point:
//! "everything is a buffer, viewports are pure view."
//!
//! Everything here is pure and testable with no rendering and no event types;
//! the app layer binds ids to buffers, walks [`Layout::compute`] to draw, and
//! routes input per [`OverlayStack::captures_input`].

use ratatui::layout::Rect;

/// A viewport slot in the layout tree. Stable across redraws; the app maps it to
/// whatever [`BufferId`] it currently shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u64);

impl PaneId {
    /// **A pane id never minted before in this process** — every workspace's
    /// panes from one count, so anything keyed by pane (a companion's host, a
    /// decision's window) cannot mistake one tab's pane for another's. Each
    /// `Workspace` counted from 0, and a second would have reused 0, 1, 2…
    /// (`docs/DesignDocs/tabs.md` §1). A pane the layout refused leaves a
    /// gap, which nothing reads.
    pub fn fresh() -> PaneId {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        PaneId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// A piece of content one or more viewports show — the emacs property we build
/// on. **Sessions, pages and session trees** alike, numbered from one count
/// by `App`'s buffer list (`Buffers::add`, the one place a number is minted
/// in production, by count: the field is public for the tests' fixtures), so
/// session numbers skip, vim's way (`docs/DesignDocs/buffers.md` §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BufferId(pub u64);

impl BufferId {
    /// **The number `:ls` gives it** — 1-based, as vim numbers buffers: `b1`
    /// is the first. One statement for every place a number is shown or read
    /// ([`numbered`](Self::numbered) is its inverse).
    pub fn number(self) -> u64 {
        self.0 + 1
    }

    /// The buffer `:ls` numbers `n`, if `n` can number one (`b0` cannot).
    pub fn numbered(n: u64) -> Option<BufferId> {
        n.checked_sub(1).map(BufferId)
    }
}

/// How a split divides its area — named to match vim's `:split` / `:vsplit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// `:split` — stack the two children **top over bottom** (a horizontal
    /// dividing line).
    Horizontal,
    /// `:vsplit` — place the two children **side by side** (a vertical dividing
    /// line).
    Vertical,
}

/// A binary tree of tiled panes. A leaf is one pane; a node splits its area
/// between two subtrees. This is the retained layout state; rects are derived
/// from it each frame by [`compute`](Layout::compute).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    /// A single pane fills the area.
    Leaf(PaneId),
    /// Two subtrees share the area along `dir`, divided by `sizing` (`first` =
    /// top or left).
    Split {
        dir: SplitDir,
        sizing: AxisSizing,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

/// A split's size **remembered per axis**.
///
/// Reorienting a split (vim `<C-w>K` ⇄ `<C-w>L`) changes which dimension the
/// divider runs along, and "12" means something different in each: 12 rows is a
/// usable prompt, 12 columns is a sliver. Holding one value per axis means
/// flipping and flipping back restores the size you had, instead of
/// reinterpreting rows as columns.
///
/// The workspace used to carry exactly this, as a `prompt_rows`/`prompt_cols`
/// pair — memory for *one* split, in a struct that could only ever describe
/// one. Here it belongs to **every** split, which is the same idea with the
/// cardinality removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisSizing {
    /// The size when stacked top/bottom (`:split`) — a row count.
    pub horizontal: Sizing,
    /// The size when side by side (`:vsplit`) — a column count.
    pub vertical: Sizing,
}

impl AxisSizing {
    /// The same sizing on both axes — the right seed for a ratio (which is
    /// axis-independent) and a sane one for cells until each axis is set.
    pub fn uniform(sizing: Sizing) -> AxisSizing {
        AxisSizing {
            horizontal: sizing,
            vertical: sizing,
        }
    }

    /// The sizing that applies along `dir`.
    pub fn along(self, dir: SplitDir) -> Sizing {
        match dir {
            SplitDir::Horizontal => self.horizontal,
            SplitDir::Vertical => self.vertical,
        }
    }

    /// Set the sizing for `dir`, leaving the other axis's memory intact.
    pub fn set_along(&mut self, dir: SplitDir, sizing: Sizing) {
        match dir {
            SplitDir::Horizontal => self.horizontal = sizing,
            SplitDir::Vertical => self.vertical = sizing,
        }
    }
}

/// How a split divides its axis. Absolute modes exist because a bar-like pane
/// (the prompt) is a **fixed** number of rows/cols — a percentage would grow it
/// as the terminal grows, which is wrong. This mirrors ratatui's `Constraint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sizing {
    /// `first` takes this percent of the axis (clamped 1..=99); `second` the rest.
    Ratio(u16),
    /// `first` takes exactly this many cells; `second` takes the rest.
    FirstCells(u16),
    /// `second` takes exactly this many cells; `first` takes the rest.
    SecondCells(u16),
}

impl Sizing {
    /// The same division seen from the other end of the divider — what a
    /// [`swap_with_sibling`](Layout::swap_with_sibling) needs so each pane keeps
    /// **its own** size rather than inheriting its sibling's.
    pub fn mirrored(self) -> Sizing {
        match self {
            Sizing::Ratio(pct) => Sizing::Ratio(100u16.saturating_sub(pct.clamp(1, 99))),
            Sizing::FirstCells(n) => Sizing::SecondCells(n),
            Sizing::SecondCells(n) => Sizing::FirstCells(n),
        }
    }
}

impl Layout {
    /// A fresh single-pane layout.
    pub fn single(pane: PaneId) -> Layout {
        Layout::Leaf(pane)
    }

    /// The panes, left-to-right / top-to-bottom (the focus-ring order).
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Layout::Leaf(p) => out.push(*p),
            Layout::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    /// Whether a pane exists in this tree.
    pub fn contains(&self, pane: PaneId) -> bool {
        self.leaves().contains(&pane)
    }

    /// Assign a screen rect to every pane. Integer geometry: `first` gets the
    /// floor of its share along the split axis, `second` gets the remainder, so
    /// the two always exactly tile `area` with no gap or overlap.
    pub fn compute(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.compute_into(area, &mut out);
        out
    }

    fn compute_into(&self, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Layout::Leaf(p) => out.push((*p, area)),
            Layout::Split {
                dir,
                sizing,
                first,
                second,
            } => {
                let (a, b) = split_rect(area, *dir, sizing.along(*dir));
                first.compute_into(a, out);
                second.compute_into(b, out);
            }
        }
    }

    /// Split `target` in two: it keeps its place as `first`, and `new_pane`
    /// becomes `second`, divided by `sizing` on both axes. Returns `true` if
    /// `target` was found. Use [`split_sized`](Layout::split_sized) to seed the
    /// two axes differently.
    pub fn split(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        dir: SplitDir,
        sizing: Sizing,
    ) -> bool {
        self.split_sized(target, new_pane, dir, AxisSizing::uniform(sizing))
    }

    /// [`split`](Layout::split) with per-axis sizes — the general form.
    pub fn split_sized(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        dir: SplitDir,
        sizing: AxisSizing,
    ) -> bool {
        match self {
            Layout::Leaf(p) if *p == target => {
                *self = Layout::Split {
                    dir,
                    sizing,
                    first: Box::new(Layout::Leaf(target)),
                    second: Box::new(Layout::Leaf(new_pane)),
                };
                true
            }
            Layout::Leaf(_) => false,
            Layout::Split { first, second, .. } => {
                first.split_sized(target, new_pane, dir, sizing)
                    || second.split_sized(target, new_pane, dir, sizing)
            }
        }
    }

    /// Put `new_pane` across the **whole frame's** edge, wrapping everything
    /// that is there now — vim's `:help` placement (a full-width window at the
    /// top, above whatever arrangement you had).
    ///
    /// Distinct from [`split_sized`](Layout::split_sized), which divides one
    /// leaf: this divides the *root*, so the newcomer's size comes from the
    /// frame rather than from one pane's share of it. That is the difference
    /// between a usable window and half of a six-row bar, and it is why this
    /// exists — splitting a pane that is sized to something other than the
    /// space available cannot produce a pane sized to the space available.
    ///
    /// `first` puts the newcomer before (top/left) rather than after.
    pub fn split_root(&mut self, new_pane: PaneId, dir: SplitDir, sizing: AxisSizing, first: bool) {
        let existing = std::mem::replace(self, Layout::Leaf(new_pane));
        let leaf = Box::new(Layout::Leaf(new_pane));
        let existing = Box::new(existing);
        *self = if first {
            Layout::Split {
                dir,
                sizing,
                first: leaf,
                second: existing,
            }
        } else {
            Layout::Split {
                dir,
                sizing,
                first: existing,
                second: leaf,
            }
        };
    }

    /// Remove `pane`; its sibling takes over the parent's area. Returns `true` if
    /// removed. Removing the **last** pane is refused (a layout must keep at least
    /// one) — returns `false`, tree unchanged.
    pub fn remove(&mut self, pane: PaneId) -> bool {
        match self {
            // A lone leaf can't remove itself — there must always be one pane.
            Layout::Leaf(_) => false,
            Layout::Split { first, second, .. } => {
                // Direct child is the target → replace self with the sibling.
                if matches!(first.as_ref(), Layout::Leaf(p) if *p == pane) {
                    *self = std::mem::replace(second, Layout::Leaf(pane));
                    return true;
                }
                if matches!(second.as_ref(), Layout::Leaf(p) if *p == pane) {
                    *self = std::mem::replace(first, Layout::Leaf(pane));
                    return true;
                }
                first.remove(pane) || second.remove(pane)
            }
        }
    }

    /// Whether this node is the split directly containing `pane` as a child leaf.
    fn is_direct_parent(&self, pane: PaneId) -> bool {
        matches!(
            self,
            Layout::Split { first, second, .. }
                if matches!(first.as_ref(), Layout::Leaf(p) if *p == pane)
                    || matches!(second.as_ref(), Layout::Leaf(p) if *p == pane)
        )
    }

    /// The `Split` node directly containing `pane` — the one the `<C-w>` window
    /// commands act on (resize / reorient / swap all target the split *around*
    /// the focused pane). `None` for a lone root pane (no parent to adjust).
    fn parent_of(&mut self, pane: PaneId) -> Option<&mut Layout> {
        if self.is_direct_parent(pane) {
            return Some(self);
        }
        match self {
            Layout::Leaf(_) => None,
            Layout::Split { first, second, .. } => {
                if let Some(x) = first.parent_of(pane) {
                    Some(x)
                } else {
                    second.parent_of(pane)
                }
            }
        }
    }

    /// Set the [`Sizing`] of the split *around* `pane`, **on its current axis**
    /// — the other axis keeps the size it remembers. No-op for a lone root
    /// pane. Returns `true` if a split was adjusted.
    pub fn set_sizing_around(&mut self, pane: PaneId, sizing: Sizing) -> bool {
        if let Some(Layout::Split { dir, sizing: s, .. }) = self.parent_of(pane) {
            let dir = *dir;
            s.set_along(dir, sizing);
            true
        } else {
            false
        }
    }

    /// The [`Sizing`] of the split around `pane` on its current axis, if it has
    /// a parent split.
    pub fn sizing_around(&mut self, pane: PaneId) -> Option<Sizing> {
        match self.parent_of(pane) {
            Some(Layout::Split { dir, sizing, .. }) => Some(sizing.along(*dir)),
            _ => None,
        }
    }

    /// Reorient the split around `pane` (vim `<C-w>H/J/K/L` change stacked ⇄
    /// side-by-side). No-op for a lone root pane.
    pub fn reorient_around(&mut self, pane: PaneId, dir: SplitDir) -> bool {
        if let Some(Layout::Split { dir: d, .. }) = self.parent_of(pane) {
            *d = dir;
            true
        } else {
            false
        }
    }

    /// The orientation of the split around `pane`, if any.
    pub fn dir_around(&mut self, pane: PaneId) -> Option<SplitDir> {
        match self.parent_of(pane) {
            Some(Layout::Split { dir, .. }) => Some(*dir),
            _ => None,
        }
    }

    /// Replace both axes of the sizing around `pane` — used to restore a bar's
    /// declared size on `<C-w>=` without forgetting the other axis.
    pub fn set_axis_sizing_around(&mut self, pane: PaneId, sizing: AxisSizing) -> bool {
        if let Some(Layout::Split { sizing: s, .. }) = self.parent_of(pane) {
            *s = sizing;
            true
        } else {
            false
        }
    }

    /// Give `pane` exactly `cells` **along its split's current axis**, whichever
    /// side of the divider it happens to be on.
    ///
    /// [`Sizing`] is written from one end of a divider and interpreted along
    /// one axis, so "make this pane N rows tall" is only expressible here,
    /// where both facts are known. Callers that built a `Sizing` themselves had
    /// to assume both — and every one of them assumed the arrangement it
    /// shipped in:
    ///
    /// - `Extent::Transient { rows }` became `SecondCells(rows + border)`. When
    ///   [`remove`](Layout::remove) promoted the pane into a *vertical*
    ///   grandparent split, or `Workspace::relocate` reoriented the split it
    ///   shares with its sibling, that row count was applied as **columns** — a
    ///   decision pane 110 columns wide beside an 8-column transcript, on a
    ///   120-column terminal.
    /// - `SecondCells` sized the *sibling* whenever the pane had become a first
    ///   child.
    ///
    /// Returns `false` for a lone root pane (no divider to move).
    pub fn set_cells_around(&mut self, pane: PaneId, cells: u16) -> bool {
        let second = self.is_second_child(pane);
        if let Some(Layout::Split { sizing, dir, .. }) = self.parent_of(pane) {
            let dir = *dir;
            sizing.set_along(
                dir,
                if second {
                    Sizing::SecondCells(cells)
                } else {
                    Sizing::FirstCells(cells)
                },
            );
            true
        } else {
            false
        }
    }

    /// The pane sharing a split with `pane`, when the sibling is a single leaf.
    ///
    /// `None` for a lone root pane, and for a sibling that is itself a split —
    /// callers use this to ask "may I disturb whoever is on the other side of
    /// this divider", and a subtree is not one pane to ask about.
    pub fn sibling_of(&mut self, pane: PaneId) -> Option<PaneId> {
        match self.parent_of(pane)? {
            Layout::Split { first, second, .. } => {
                let (a, b) = (first.as_ref(), second.as_ref());
                match (a, b) {
                    (Layout::Leaf(p), Layout::Leaf(q)) if *p == pane => Some(*q),
                    (Layout::Leaf(p), Layout::Leaf(q)) if *q == pane => Some(*p),
                    (Layout::Leaf(p), _) if *p == pane => None,
                    (_, Layout::Leaf(q)) if *q == pane => None,
                    _ => None,
                }
            }
            Layout::Leaf(_) => None,
        }
    }

    /// Whether `pane` is the **second** (bottom / right) child of its split.
    ///
    /// [`Sizing`] is written from one side of a divider, so anything moving a
    /// divider has to know which side the pane it is acting for sits on. A lone
    /// root pane has no split and answers `false`.
    pub fn is_second_child(&mut self, pane: PaneId) -> bool {
        matches!(
            self.parent_of(pane),
            Some(Layout::Split { second, .. })
                if matches!(second.as_ref(), Layout::Leaf(p) if *p == pane)
        )
    }

    /// How many panes are in this subtree — the weight `<C-w>=` divides by.
    pub fn leaf_count(&self) -> u16 {
        match self {
            Layout::Leaf(_) => 1,
            Layout::Split { first, second, .. } => {
                first.leaf_count().saturating_add(second.leaf_count())
            }
        }
    }

    /// Set every **proportional** split so the panes come out equally sized,
    /// leaving absolute (bar) splits alone — vim's `<C-w>=`: windows even out,
    /// bars stay bars.
    ///
    /// **Weighted by leaf count, not 50/50.** This used to set every ratio to
    /// 50, under a doc claiming it was vim's behaviour "in the general case" —
    /// and that is only the same thing when both subtrees hold one pane. Three
    /// transcripts from two `:vs` nest as `((A|B)|C)`, so 50-at-every-split
    /// gives **25/25/50**: press the key that means "make these even" and two
    /// panes stay half the width of the third. Exactly what Steven hit
    /// dogfooding, and the `n = 2` shortcut once more — a rule that agrees with
    /// the right one at two panes and diverges at three
    /// (`[[project_shannon_n_equals_two]]`).
    ///
    /// `first` takes `leaves(first) / (leaves(first) + leaves(second))`, so
    /// `((A|B)|C)` gives the left subtree 2/3 and C 1/3, and the inner split
    /// halves the left — a third each.
    pub fn equalize_ratios(&mut self) {
        if let Layout::Split {
            sizing,
            first,
            second,
            ..
        } = self
        {
            let (a, b) = (
                u32::from(first.leaf_count()),
                u32::from(second.leaf_count()),
            );
            // `checked_div` rather than a hand-written zero test: the guard and
            // the division are then one expression and cannot drift apart.
            let pct = (a * 100)
                .checked_div(a + b)
                .map_or(50, |p| p.clamp(1, 99) as u16);
            // Only proportional splits move. A bar (the prompt) carries an
            // absolute `Sizing`, and evening it out with the transcripts is
            // exactly what `<C-w>=` must not do.
            if matches!(sizing.horizontal, Sizing::Ratio(_)) {
                sizing.horizontal = Sizing::Ratio(pct);
            }
            if matches!(sizing.vertical, Sizing::Ratio(_)) {
                sizing.vertical = Sizing::Ratio(pct);
            }
            first.equalize_ratios();
            second.equalize_ratios();
        }
    }

    /// The route from the root to `pane`: for each split on the way, whether we
    /// descended into `second`, and that split's axis.
    fn route_to(&self, pane: PaneId) -> Option<Vec<(bool, SplitDir)>> {
        match self {
            Layout::Leaf(p) if *p == pane => Some(Vec::new()),
            Layout::Leaf(_) => None,
            Layout::Split {
                dir, first, second, ..
            } => {
                if let Some(mut r) = first.route_to(pane) {
                    r.insert(0, (false, *dir));
                    return Some(r);
                }
                let mut r = second.route_to(pane)?;
                r.insert(0, (true, *dir));
                Some(r)
            }
        }
    }

    /// The **nearest ancestor split running along `dir`** above `pane`, as a
    /// route to it plus which side `pane` lies on.
    ///
    /// This is what makes `<C-w>+` mean what it means in vim. Resizing used to
    /// consult only the pane's *immediate* parent, so with three transcripts
    /// side by side (a vertical parent) `<C-w>+` — a height change — matched
    /// nothing and did nothing, silently. Vim walks up: the height of "this
    /// window" is set by the closest horizontal divider above it, which here is
    /// the one between the transcripts and the prompt.
    fn route_along(&self, pane: PaneId, dir: SplitDir) -> Option<(Vec<bool>, bool)> {
        let route = self.route_to(pane)?;
        let at = route.iter().rposition(|(_, d)| *d == dir)?;
        Some((route[..at].iter().map(|(s, _)| *s).collect(), route[at].0))
    }

    /// The sizing of `route_along`'s split, on `dir`.
    pub fn sizing_along(&self, pane: PaneId, dir: SplitDir) -> Option<Sizing> {
        let (route, _) = self.route_along(pane, dir)?;
        match self.follow(&route)? {
            Layout::Split { sizing, .. } => Some(sizing.along(dir)),
            Layout::Leaf(_) => None,
        }
    }

    /// Whether `pane` lies in the `second` (bottom/right) child of
    /// `route_along`'s split.
    pub fn is_second_along(&self, pane: PaneId, dir: SplitDir) -> bool {
        self.route_along(pane, dir).is_some_and(|(_, side)| side)
    }

    /// Replace the sizing of `route_along`'s split, on `dir`.
    pub fn set_sizing_along(&mut self, pane: PaneId, dir: SplitDir, sizing: Sizing) -> bool {
        let Some((route, _)) = self.route_along(pane, dir) else {
            return false;
        };
        let mut node = self;
        for step in route {
            match node {
                Layout::Split { first, second, .. } => {
                    node = if step { second } else { first };
                }
                Layout::Leaf(_) => return false,
            }
        }
        match node {
            Layout::Split { sizing: s, .. } => {
                s.set_along(dir, sizing);
                true
            }
            Layout::Leaf(_) => false,
        }
    }

    /// Walk `route` from here, `true` meaning "into `second`".
    fn follow(&self, route: &[bool]) -> Option<&Layout> {
        let mut node = self;
        for step in route {
            match node {
                Layout::Split { first, second, .. } => {
                    node = if *step { second } else { first };
                }
                Layout::Leaf(_) => return None,
            }
        }
        Some(node)
    }

    /// Swap `pane` with its sibling in their parent split (vim `<C-w>x` /
    /// `<C-w>r` rotate, for the two-pane case). No-op for a lone root pane.
    ///
    /// **The sizing is mirrored with them.** A [`Sizing`] is written from one
    /// end of the divider (`FirstCells` / `SecondCells` / a ratio of the first
    /// share), so swapping the children without mirroring it hands each pane
    /// *the other one's size* — a 10-row prompt bar moved to the top became a
    /// 10-row transcript with a 35-row prompt. Both axes are mirrored, not just
    /// the current one, or the size only lies after the next reorient.
    pub fn swap_with_sibling(&mut self, pane: PaneId) -> bool {
        if let Some(Layout::Split {
            first,
            second,
            sizing,
            ..
        }) = self.parent_of(pane)
        {
            std::mem::swap(first, second);
            sizing.horizontal = sizing.horizontal.mirrored();
            sizing.vertical = sizing.vertical.mirrored();
            true
        } else {
            false
        }
    }
}

/// Divide `area` into (`first`, `second`) along `dir` per `sizing`. The two
/// always exactly tile `area` (no gap, no overlap); `first`'s share is clamped
/// to the available extent so `second` never underflows.
fn split_rect(area: Rect, dir: SplitDir, sizing: Sizing) -> (Rect, Rect) {
    // The total extent along the split axis.
    let total = match dir {
        SplitDir::Horizontal => area.height,
        SplitDir::Vertical => area.width,
    };
    // `first`'s extent, resolved from the sizing and clamped into `0..=total`.
    let first_extent = match sizing {
        Sizing::Ratio(pct) => (u32::from(total) * u32::from(pct.clamp(1, 99)) / 100) as u16,
        Sizing::FirstCells(n) => n.min(total),
        Sizing::SecondCells(n) => total.saturating_sub(n),
    };
    match dir {
        SplitDir::Horizontal => {
            let top = Rect {
                height: first_extent,
                ..area
            };
            let bottom = Rect {
                y: area.y.saturating_add(first_extent),
                height: total.saturating_sub(first_extent),
                ..area
            };
            (top, bottom)
        }
        SplitDir::Vertical => {
            let left = Rect {
                width: first_extent,
                ..area
            };
            let right = Rect {
                x: area.x.saturating_add(first_extent),
                width: total.saturating_sub(first_extent),
                ..area
            };
            (left, right)
        }
    }
}

/// The focused pane, plus ring and directional movement over a [`Layout`].
///
/// Kept separate from the tree so focus is one small piece of state (the routing
/// consensus for immediate-mode TUIs: one `focused` value, not per-widget flags).
///
/// **The rule that keeps one value from becoming several:** a pane decides
/// whether it is focused by **comparing itself to this value**, never by
/// negating another pane's answer, and every "which pane is at/under/for X"
/// question is answered by **walking the layout**, never by a hand-written list.
/// Both were violated while there were only two panes — where `!prompt` really
/// does mean "the chat" and a two-arm `if` really is exhaustive — and both broke
/// the moment a third pane arrived: the transcript and a pending decision drew
/// the focused border simultaneously, the decision pane was unreachable by
/// mouse, and `<C-w>+` aimed at it resized the transcript. The `n = 2` shortcut
/// is not a smaller version of the general rule; it is a different rule that
/// happens to agree at `n = 2`. (`[[feedback_one_door_per_invariant]]`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Focus {
    pane: PaneId,
    /// The pane focus came FROM — vim's `<C-w>p` target ("last accessed").
    ///
    /// Not the same question as ring order, and the difference is the `n = 2`
    /// shortcut one more time: with two panes "the one I came from" and "the
    /// previous one in the ring" are always the same pane, so `<C-w>p` was
    /// wired to `cycle(back)` and looked correct. With three, `<C-w>w`,
    /// `<C-w>w`, `<C-w>p` should return you to where you just were, and ring
    /// order sends you somewhere else entirely.
    ///
    /// `None` until focus has moved at least once, and cleared whenever the
    /// remembered pane stops existing — a stale `PaneId` here would send focus
    /// to a pane that closed.
    prev: Option<PaneId>,
}

impl Focus {
    pub fn new(pane: PaneId) -> Focus {
        Focus { pane, prev: None }
    }

    pub fn pane(self) -> PaneId {
        self.pane
    }

    /// The one door through which focus moves.
    ///
    /// Every `self.pane = …` goes through here, so `prev` cannot drift out of
    /// step with it: the invariant ("`prev` is the pane we came from") has one
    /// place that can violate it rather than one per motion command.
    /// Re-focusing the pane you are already on is not a move and must not
    /// overwrite `prev` — otherwise a redundant `<C-w>w` on a single pane, or a
    /// click on the pane you are in, would silently make `<C-w>p` a no-op.
    fn go_to(&mut self, pane: PaneId) {
        if pane != self.pane {
            self.prev = Some(self.pane);
            self.pane = pane;
        }
    }

    /// Point focus at `pane` if it exists in `layout`.
    pub fn set(&mut self, pane: PaneId, layout: &Layout) {
        if layout.contains(pane) {
            self.go_to(pane);
        }
    }

    /// If the focused pane vanished (after a `remove`), land on the first
    /// remaining pane so focus is never dangling — and drop a remembered
    /// `prev` that vanished with it, so `<C-w>p` can never aim at a closed pane.
    ///
    /// The landing is deliberately NOT recorded as a move: the pane we came
    /// from no longer exists, so remembering it would hand `<C-w>p` a corpse.
    pub fn reconcile(&mut self, layout: &Layout) {
        if self.prev.is_some_and(|p| !layout.contains(p)) {
            self.prev = None;
        }
        if !layout.contains(self.pane)
            && let Some(first) = layout.leaves().first()
        {
            self.pane = *first;
            if self.prev == Some(*first) {
                self.prev = None;
            }
        }
    }

    /// Move focus to the next / previous pane in ring order (wraps).
    pub fn cycle(&mut self, layout: &Layout, forward: bool) {
        let leaves = layout.leaves();
        if let Some(i) = leaves.iter().position(|p| *p == self.pane) {
            let n = leaves.len();
            let j = if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            };
            self.go_to(leaves[j]);
        }
    }

    /// vim `<C-w>p` — jump back to the pane focus came from, and leave *this*
    /// pane as the new `prev` so pressing it again returns (a toggle, as vim's
    /// is). Returns `false` when there is nowhere to go back to: no move has
    /// happened yet, or the remembered pane has closed. The caller reports that
    /// rather than silently doing nothing — a dead key reads as broken
    /// (`[[feedback_visible_refusals]]`).
    pub fn alternate(&mut self, layout: &Layout) -> bool {
        match self.prev {
            Some(prev) if layout.contains(prev) => {
                self.go_to(prev);
                true
            }
            _ => false,
        }
    }

    /// Move focus spatially (vim `<C-w>h/j/k/l`): the **neighbour** in that
    /// direction. No-op when there is none (the edge of the layout), as in vim.
    ///
    /// A candidate must clear two bars, and both matter:
    ///
    /// 1. **Beyond the current pane's edge** on the axis of travel — `Right`
    ///    means starting at or after this pane's right edge, not merely
    ///    "further right on average".
    /// 2. **Overlapping on the perpendicular axis** — a pane to the right must
    ///    share rows with this one, or it is not beside it, it is diagonal.
    ///
    /// The version this replaces scored candidates by distance between
    /// *centres* with the cross axis weighted ×4. That is fine for two panes
    /// and wrong for four: with `[graph | inspector]` above a full-width prompt,
    /// `<C-w>l` from the graph scored the prompt (25² + 13²·4 = 1301) below the
    /// inspector (50² + 0 = 2500) and moved focus **down**. A heuristic over
    /// centres cannot express "beside"; overlap can, at any pane count.
    ///
    /// Ties (two panes stacked in the column you are moving into) go to the
    /// nearest edge first, then top-left — vim resolves them with the cursor's
    /// position, which we do not track per pane yet.
    pub fn shift(&mut self, layout: &Layout, area: Rect, dir: Direction) {
        let rects = layout.compute(area);
        let Some((_, cur)) = rects.iter().find(|(p, _)| *p == self.pane) else {
            return;
        };
        let cur = *cur;
        let best = rects
            .iter()
            .filter(|(p, _)| *p != self.pane)
            .filter_map(|(p, r)| {
                // How far past our edge it starts (0 = flush against us), and
                // whether it shares any extent on the other axis.
                let (gap, overlaps) = match dir {
                    Direction::Right => (
                        i64::from(r.x) - i64::from(cur.x + cur.width),
                        spans_overlap(cur.y, cur.height, r.y, r.height),
                    ),
                    Direction::Left => (
                        i64::from(cur.x) - i64::from(r.x + r.width),
                        spans_overlap(cur.y, cur.height, r.y, r.height),
                    ),
                    Direction::Down => (
                        i64::from(r.y) - i64::from(cur.y + cur.height),
                        spans_overlap(cur.x, cur.width, r.x, r.width),
                    ),
                    Direction::Up => (
                        i64::from(cur.y) - i64::from(r.y + r.height),
                        spans_overlap(cur.x, cur.width, r.x, r.width),
                    ),
                };
                (gap >= 0 && overlaps).then_some((gap, r.y, r.x, *p))
            })
            .min_by_key(|(gap, y, x, _)| (*gap, *y, *x));
        if let Some((.., p)) = best {
            self.go_to(p);
        }
    }
}

/// A spatial focus direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Do `[a, a+alen)` and `[b, b+blen)` share any cell? Zero-length spans never
/// overlap — a pane squeezed to nothing is not beside anything.
fn spans_overlap(a: u16, alen: u16, b: u16, blen: u16) -> bool {
    alen > 0 && blen > 0 && a < b.saturating_add(blen) && b < a.saturating_add(alen)
}

/// A floating viewport laid over the tiled layout: a glance (`Float`) or a
/// blocking decision (`Modal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overlay {
    /// The buffer it shows.
    pub buffer: BufferId,
    /// Where it sits (already positioned by the caller).
    pub area: Rect,
    pub kind: OverlayKind,
}

/// Whether an overlay traps input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKind {
    /// A blocking decision (the HITL approval): captures all input until popped.
    Modal,
    /// A non-blocking glance (hover/peek): drawn on top, but input still goes to
    /// the focused pane (or the float joins the focus ring — the app decides).
    Float,
}

/// The floats/modals on top of the tiled layout, in z-order (last = topmost).
///
/// Input routing is **overlay-stack-first → focused pane → global keys**
/// (`docs/Milestone 5.md`): [`captures_input`](OverlayStack::captures_input)
/// answers the first hop.
#[derive(Debug, Clone, Default)]
pub struct OverlayStack {
    stack: Vec<Overlay>,
}

impl OverlayStack {
    pub fn new() -> OverlayStack {
        OverlayStack { stack: Vec::new() }
    }

    /// Push an overlay on top.
    pub fn push(&mut self, overlay: Overlay) {
        self.stack.push(overlay);
    }

    /// Pop the topmost overlay (dismiss the modal/float), returning it.
    pub fn pop(&mut self) -> Option<Overlay> {
        self.stack.pop()
    }

    /// The topmost overlay, if any.
    pub fn top(&self) -> Option<&Overlay> {
        self.stack.last()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Overlays bottom-to-top, for rendering (draw in this order so the topmost
    /// lands last, over everything, via `Clear`).
    pub fn iter(&self) -> impl Iterator<Item = &Overlay> {
        self.stack.iter()
    }

    /// The buffer that should receive input *instead of* the focused pane —
    /// `Some` exactly when the topmost overlay is a [`Modal`](OverlayKind::Modal).
    /// A non-modal float does not steal input (it participates in focus, which is
    /// the app's concern), so this returns `None` for it.
    pub fn captures_input(&self) -> Option<BufferId> {
        match self.top() {
            Some(o) if o.kind == OverlayKind::Modal => Some(o.buffer),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
