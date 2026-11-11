//! A scrollable buffer of pre-styled text — the ONE scroll mechanism behind
//! every static page: `:help` (`help_view`), `:analyze` and the decision
//! pane's report (`permission_view`), `:messages`, the listing (`:ls`,
//! `:alias`, `:rules`) and the `:graph` inspector (the app's `InspectorView`). Six
//! holders; `App::text_pane_mut` is the one door to each. (`:memories` and
//! `:skills` were listings; they are a setup screen now, `crate::shelf`.)
//!
//! # Why this exists
//!
//! The scroll rule was written four separate times before this: `Viewport` for
//! the transcript, `GraphView` for the tree, and two byte-identical copies
//! inside `permission_view` (one for `AnalyzeView`, one for `ToolPrompt`). The
//! copies had already diverged — `AnalyzeView::scroll_down` carried a comment
//! reading *"the same bug `ToolPrompt` was fixed for and this copy never
//! received"*, which is copy-paste debt describing itself.
//!
//! This module was introduced with the help pane and, for one commit, was used
//! by the help pane **alone** — while its own documentation claimed `:analyze`
//! shared it. Extracting a shared thing and wiring one consumer leaves the
//! duplication exactly where it was and adds a false claim on top; the two
//! `permission_view` copies are now really gone. `Viewport` and `GraphView`
//! keep their own, and honestly so: they scroll a live model with a cursor to
//! follow, not a static page.
//!
//! # Scrolling counts WRAPPED rows, not lines
//!
//! The copies all clamped against `lines.len()` while their panes rendered with
//! ratatui's `Wrap { trim: false }`, whose `scroll` offset counts *rendered*
//! rows. Those are the same number only while nothing wraps. When a 78-column
//! manual is read in a 40-column split, every long line becomes two rows, the
//! clamp stops the offset at roughly half the document, and **the tail is
//! simply unreachable** — the reader has no way to know the text continues.
//!
//! This mattered little when reports were full-width takeovers, which is when
//! that code was written. Panes made narrow viewers ordinary, so the bug moved
//! from theoretical to routine, and a `:help` you cannot scroll to the end of
//! is not a help system.
//!
//! Wrapping depends on the width, which is a *render*-time fact, so the height
//! and width are passed in at each scroll rather than stored — the view never
//! holds a stale idea of its own geometry.
//!
//! # One ruler: the page wraps itself
//!
//! It counted rows by hard cell wrap and let ratatui draw with its word
//! wrapper, which needs more rows — so the measure was always short, a
//! `:help` at 38 columns had 99 rows no key reached, and the decision pane
//! was sized by it, with a model-written command choosing the wrap ratio
//! (TODO.md's CRITICAL "the ruler we scroll by is not the ruler we render
//! by"). Now one function cuts a line into rows (`PageLine::wrap`): the
//! geometry counts by it, `visible_rows` draws by it, and the renderer asks
//! ratatui to wrap nothing.

use crate::keymap::{CursorMotion, EditAction, OpTarget, SelectionKind};
use crate::paint::PageLine;

/// A page's lines — roles, painted when drawn (`crate::paint`) — plus a
/// scroll offset, clamped so content is always visible.
#[derive(Debug, Clone, Default)]
pub struct TextView {
    lines: Vec<PageLine>,
    /// **The text at the window's top** — a source line and the char its top
    /// row starts at; the rendered row it stands on is derived at whatever
    /// width the page is drawn ([`top_row`](Self::top_row)), where
    /// [`visible_rows`](Self::visible_rows) starts. It was that row, kept:
    /// rows are a width's, and `:help` under a `:vsplit` came back six lines
    /// up (review stop 15, D M3, measured: line 100 → 94 — Neovim keeps
    /// 100), the class S21-Q10 removed from the transcript first.
    top: (usize, usize),
    /// A **source line** we have been asked to show but cannot place yet.
    ///
    /// The two units differ: callers know "line 412 of the manual", `scroll`
    /// counts wrapped rows, and the conversion needs the pane's width — which
    /// nobody knows until the frame is laid out. So the request is *held* and
    /// resolved on the next [`clamp`](Self::clamp), which the renderer already
    /// calls with the real geometry. Converting eagerly against a guessed width
    /// would land the jump a few rows off on any wrapped line, which for a
    /// section header is the difference between arriving and nearly arriving.
    ///
    /// Where in the window the cursor's line goes — at the top for a tag
    /// jump, in the middle for a page returned to (`App::show_page`, vim's
    /// `'"`). Only the place waits: the cursor is put on its line at once
    /// ([`reveal`](Self::reveal)), since a key the loop drains before the
    /// frame edits where the cursor is — `:b N` back to line 50 of the tutor,
    /// then `x`, changed line 1 (review stop 16, A L6).
    pending_place: Option<crate::viewport::Place>,
    /// **Where you are in the page, and what is selected** — the
    /// transcript's own read-only `prompt::Buffer` over the page's text, so
    /// a page takes every motion, `v` `V` `<C-v>`, `o`, a text object and
    /// `y` from the code the transcript takes them from
    /// (`Buffer::apply_read_only`). Steven, 10-07: "what if I want to
    /// visually highlight some text from the help file, yank it, and paste
    /// it into a prompt … We don't want to force that condition on the
    /// user." A page's cursor was a line of its own, and its selection whole
    /// rows: `w` had no column to move in.
    ///
    /// Before that, a text pane had no cursor at all: `j`/`k` moved the
    /// *view*, so `:help`, `:analyze` and `:messages` navigated differently
    /// from every other pane in the app. In vim, `:help` is a buffer in a
    /// window with a cursor and `'modifiable'` off — the read-only part is a
    /// property, not a different navigation system (docket 67).
    ///
    /// The window's top and the cursor are both **source lines** (`j` moves
    /// by line; a wrapped line is one press, as in vim without `gj`), and the
    /// window moves in **rendered rows**; `rows_before` is the conversion,
    /// exactly as `Output::logical_to_visual` is for the transcript. One is
    /// where the user is, the other where the window is, and
    /// `viewport::follow` relates them.
    cursor: crate::prompt::Buffer,
    /// **The geometry this page was last drawn at**, remembered so a motion
    /// does not have to be told.
    ///
    /// `clamp` already receives the real `(visible, width)` before every draw —
    /// it is the one place that knows — so a cursor motion can follow inside
    /// the window the user is actually looking at instead of asking the
    /// terminal for a size and computing the pane's share of it again. That
    /// also takes the terminal off the motion path entirely, which is what let
    /// the motions be tested at all (the same reason a decision pane keeps the
    /// width it was drawn at, `DecisionView::drawn_width`).
    ///
    /// `None` until the first frame: the cursor still moves, the window simply
    /// has no geometry to follow within yet.
    last_size: Option<(usize, u16)>,
    /// **The page's geometry at one width**: the row each line starts at,
    /// and the total last — prefix sums of each line's
    /// [`PageLine::rows`]. Every measure here is a lookup in it
    /// (`rows_before`, `rows_at`, `line_rows`), and it is counted by the wrap
    /// the renderer draws ([`visible_rows`](Self::visible_rows)).
    ///
    /// A memo whose **invalidation points are the two writers of `lines`**,
    /// [`set_lines`](Self::set_lines) and [`restyle`](Self::restyle), each
    /// clearing it. A third writer must do the same, or every geometry
    /// answer is computed against a document that is gone. It was two memos, the last `rows_at` and the
    /// last `rows_before`, the second walking the page for each new line;
    /// built once per width, the page is wrapped once and every answer after
    /// is O(1). `Arc` so an answer outlives the borrow that found it.
    geometry: std::cell::RefCell<Option<(u16, std::sync::Arc<[usize]>)>>,
}

impl TextView {
    /// A page from its lines — `PageLine`s, or ratatui `Line`s in the app's
    /// own colours, which convert as they are (`From<Line>`).
    pub fn new<L: Into<PageLine>>(lines: impl IntoIterator<Item = L>) -> TextView {
        let lines: Vec<PageLine> = lines.into_iter().map(Into::into).collect();
        TextView {
            cursor: crate::prompt::Buffer::from_lines(texts(&lines)),
            lines,
            last_size: None,
            top: (0, 0),
            pending_place: None,
            geometry: std::cell::RefCell::new(None),
        }
    }

    /// The row each line starts at, at `width`, and the total last — built
    /// once per width ([`geometry`](Self::geometry)'s memo).
    fn starts(&self, width: u16) -> std::sync::Arc<[usize]> {
        if let Some((w, starts)) = self.geometry.borrow().as_ref()
            && *w == width
        {
            return starts.clone();
        }
        let cells = width.max(1) as usize;
        let mut starts = Vec::with_capacity(self.lines.len() + 1);
        let mut at = 0;
        starts.push(at);
        for line in &self.lines {
            at += line.rows(cells);
            starts.push(at);
        }
        let starts: std::sync::Arc<[usize]> = starts.into();
        *self.geometry.borrow_mut() = Some((width, starts.clone()));
        starts
    }

    /// **The rendered row at the window's top**, at `width` — the row
    /// holding the text kept there (the `top` field): its line's first
    /// row, and as many more as its char is rows into the line. A line past
    /// the end (the page shrank) is the end's row, which the next
    /// [`clamp`](Self::clamp) takes back.
    pub fn top_row(&self, width: u16) -> usize {
        let starts = self.starts(width);
        let (line, at) = self.top;
        let into = match self.lines.get(line) {
            None => return starts[self.lines.len()],
            Some(_) if at == 0 => 0,
            Some(page_line) => (page_line.row_ranges(width.max(1) as usize).iter())
                .rposition(|&(start, _)| start <= at)
                .unwrap_or(0),
        };
        starts[line] + into
    }

    /// The row at the window's top at the width it was last drawn at, `0`
    /// before its first frame, saturated into a `u16` — what a test reads.
    /// Drawing asks [`top_row`](Self::top_row) at the width it draws.
    #[cfg(test)]
    pub fn scroll(&self) -> u16 {
        let row = self.last_size.map_or(0, |(_, width)| self.top_row(width));
        row.min(u16::MAX as usize) as u16
    }

    /// **The window's top put on rendered row `row`**, at `width` — the text
    /// there kept: its line, and the char its row starts at. The one writing
    /// of a row; [`move_top`](Self::move_top) is how a motion writes one.
    fn put_top_row(&mut self, row: usize, width: u16) {
        let starts = self.starts(width);
        let last = self.lines.len().saturating_sub(1);
        let line = (starts
            .partition_point(|&start| start <= row)
            .saturating_sub(1))
        .min(last);
        let into = row.saturating_sub(starts[line]);
        let at = match self.lines.get(line) {
            Some(page_line) if into > 0 => {
                let ranges = page_line.row_ranges(width.max(1) as usize);
                let nth = into.min(ranges.len().saturating_sub(1));
                ranges.get(nth).map_or(0, |&(start, _)| start)
            }
            _ => 0,
        };
        self.top = (line, at);
    }

    /// **The window's top moved to rendered row `row`** — and a top that did
    /// not move keeps its text: found at another width, its char may sit
    /// part-way into a row here, and rewritten from the row it slides to the
    /// row's start — a page moved between two widths, a `j` at each, crept up
    /// a row a round trip (`Output::move_top`'s rule, for the transcript).
    fn move_top(&mut self, row: usize, width: u16) {
        if row != self.top_row(width) {
            self.put_top_row(row, width);
        }
    }

    /// **The rows on screen** — at most `height` of them from the scroll, at
    /// `width`: each with its source line, the char of that line it starts
    /// at, and the row itself ([`PageRow`]). Cut by
    /// [`PageLine::row_ranges`], the function the page's geometry counts by
    /// (`starts`), so what is drawn is what was measured. Only the lines it
    /// crosses are wrapped, and of a line only the rows on screen are cut
    /// ([`PageLine::cut`]): linear in what is shown and in the length of the
    /// lines it crosses.
    pub fn visible_rows(&self, width: u16, height: usize) -> Vec<PageRow> {
        let starts = self.starts(width);
        let cells = width.max(1) as usize;
        let scroll = self.top_row(width);
        // The line holding the scroll's row: the last that starts at or
        // before it (every line has a row, so the starts strictly rise).
        let mut line = starts
            .partition_point(|&start| start <= scroll)
            .saturating_sub(1);
        let mut out = Vec::with_capacity(height);
        while out.len() < height && line < self.lines.len() {
            let ranges = self.lines[line].row_ranges(cells);
            let skip = scroll.saturating_sub(starts[line]).min(ranges.len());
            let end = (skip + height - out.len()).min(ranges.len());
            let rows = self.lines[line].cut(&ranges[skip..end]);
            out.extend(
                rows.into_iter()
                    .zip(&ranges[skip..end])
                    .map(|(row, &(start, _))| PageRow { line, start, row }),
            );
            line += 1;
        }
        out
    }

    /// **Replace the content, keeping the reader where they are.**
    ///
    /// The distinction from [`new`](Self::new) is the whole point, and it is a
    /// distinction only the caller can draw: a page whose CONTENT changed under
    /// a reader who did not move must not throw away their position, while a
    /// page that has become a DIFFERENT document should start at the top.
    ///
    /// The case that forced it: the `:graph` inspector's body carries a child
    /// session's *activity*, so a background job finishing rewrites the page
    /// with no selection change at all. Rebuilding the view there sent a reader
    /// scrolled into a long message body back to line 0, asynchronously
    /// (Reviewer A).
    ///
    /// The memos are content-derived and must go. `last_size` is not — it is
    /// the window this page is being read in, which did not change.
    pub fn set_lines<L: Into<PageLine>>(&mut self, lines: impl IntoIterator<Item = L>) {
        self.lines = lines.into_iter().map(Into::into).collect();
        *self.geometry.get_mut() = None;
        // Content can SHRINK, leaving the cursor past the end. The scroll is
        // left to the `clamp` that runs before every draw, which is the one
        // place that knows the geometry; the cursor cannot wait, because a
        // caller may read it first. `Buffer::set_lines` keeps the cursor and
        // the selection's other end, clamped into what is left.
        self.cursor.set_lines(texts(&self.lines));
        self.cursor.clamp_cursor_to_char();
    }

    /// **Replace the content, keeping the reader on THEIR LINES** — where
    /// `moved` says each old line went: the cursor stays on the line it was
    /// on, at its column, and the window's top on the line that was at the
    /// top, at the same char, wherever lines above them came or went (vim
    /// keeps `topline` so; `:graph` keeps its top by row, V4 (a)). A line
    /// `moved` places nowhere — it names nothing, or its row went — keeps its
    /// index, as [`set_lines`](Self::set_lines) keeps it. Drawn or not: the
    /// top is text, so it moves by its line (it was a row, readable as one
    /// only in a window drawn before). The selection's other end stays by
    /// line, as `set_lines` keeps it (the read-only buffer sets an anchor
    /// only at its cursor).
    pub fn set_lines_moved<L: Into<PageLine>>(
        &mut self,
        lines: impl IntoIterator<Item = L>,
        moved: impl Fn(usize) -> Option<usize>,
    ) {
        let (line, col) = self.cursor.cursor();
        let (top, at) = self.top;
        self.set_lines(lines);
        if let Some(to) = moved(line) {
            self.cursor.place_cursor(to, col);
            self.cursor.clamp_cursor_to_char();
        }
        if let Some(to) = moved(top) {
            self.top = (to, at);
        }
    }

    /// Put the cursor on source line `line` now, and its line at `place` in
    /// the window (`zt`/`zz`/`zb`'s rule, [`viewport::place_top`]) once the
    /// geometry is known — the conversion needs a width, which only the
    /// renderer knows. The cursor's line at that frame, not `line`: the
    /// cursor follows its text through an edit or a `set_lines` before it.
    ///
    /// [`viewport::place_top`]: crate::viewport::place_top
    pub fn reveal(&mut self, line: usize, place: crate::viewport::Place) {
        self.reveal_at((line, 0), place);
    }

    /// [`reveal`](Self::reveal), at a `(line, char)` — a place come back to,
    /// so a link's own column (`<C-t>`, then `<C-]>` follows the same one).
    pub fn reveal_at(&mut self, (line, char): (usize, usize), place: crate::viewport::Place) {
        self.cursor.place_cursor(line, char);
        self.pending_place = Some(place);
    }

    /// Rendered rows occupied by the first `line` source lines — the
    /// source-line → wrapped-row conversion.
    ///
    /// Its consumers: resolving a held [`reveal`](Self::reveal); the
    /// cursor-following clamp and the motions that place the cursor's row;
    /// [`shows`](Self::shows) and `max_scroll`; `:help`'s anchors
    /// (`help_view`); and asking afterwards *where a jump landed*, which is
    /// the only way to test an anchor without a second copy of the wrap
    /// formula living in the test.
    pub(crate) fn rows_before(&self, line: usize, width: u16) -> usize {
        // A lookup, per frame and per motion alike: `clamp_text_panes` runs
        // before every draw, and the manual is some 2700 lines and 150 KB
        // beside a session streaming per chunk (Reviewer D; measured again
        // by Reviewer B, round 28).
        let starts = self.starts(width);
        starts[line.min(self.lines.len())]
    }

    /// The page's lines, for the renderer to paint (`crate::paint::Painter`).
    pub fn lines(&self) -> &[PageLine] {
        &self.lines
    }

    /// How many rendered rows this content occupies at `width` — word-wrapped
    /// in display **cells** ([`PageLine::rows`]), an empty line one row: the
    /// rows [`visible_rows`](Self::visible_rows) draws.
    pub fn rows_at(&self, width: u16) -> usize {
        let starts = self.starts(width);
        starts[self.lines.len()]
    }

    /// **Whether the source lines `lines` are wholly on screen** in a page
    /// `visible` rows tall and `width` wide, at the current scroll: every row
    /// they wrap to. Stricter than `viewport::lines_on_screen`, which counts
    /// a line taller than the window as shown — right for `H` and `L`, wrong
    /// for "is the text you are approving all there".
    ///
    /// The count IS the drawing now (`visible_rows` cuts by the wrap
    /// `line_rows` counts), so a wrapped line is shown when its rows are on
    /// screen. It was never said to be, while rows were counted by cells and
    /// drawn by ratatui's word wrapper, which can need more (review of
    /// 716f4f9, Reviewer A).
    pub fn shows(&self, lines: std::ops::Range<usize>, visible: u16, width: u16) -> bool {
        let top = self.rows_before(lines.start, width);
        let end = self.rows_before(lines.end, width);
        let scroll = self.top_row(width);
        scroll <= top && end <= scroll + visible as usize
    }

    /// The furthest offset that still shows content — zero when it all fits.
    ///
    /// **The last LINE at the top, not the last full page** — vim's limit, so
    /// `zt` near the end can scroll past it (Steven uses that to bring text to
    /// eye level). Nothing but a placement goes there: cursor motions keep the
    /// last page full (`follow`). And the pane can never render blank, which is
    /// what this clamp exists to prevent after a shrink.
    fn max_scroll(&self, width: u16) -> usize {
        self.rows_before(self.lines.len().saturating_sub(1), width)
    }

    /// Re-clamp after the viewport changed (a resize, a relocate). Without
    /// this, shrinking a pane leaves the offset past the end and the pane
    /// renders blank.
    pub fn clamp(&mut self, visible: usize, width: u16, scrolloff: usize) {
        let resized = self.last_size != Some((visible, width));
        self.last_size = Some((visible, width));
        // A held jump resolves here, where the width is finally known.
        if let Some(place) = self.pending_place.take() {
            let line = self.cursor();
            let (at, rows) = (self.rows_before(line, width), self.line_rows(line, width));
            let top = crate::viewport::place_top(place, at, rows, visible.max(1), scrolloff);
            self.put_top_row(top, width);
        }
        // **Past the end survives a frame, not a resize** — the rule `:graph`
        // keeps too, so it lives in `viewport`.
        let ceiling = crate::viewport::top_ceiling(
            self.rows_at(width),
            self.max_scroll(width),
            visible,
            resized,
        );
        if self.top_row(width) > ceiling {
            self.put_top_row(ceiling, width);
        }
        // **And follow.** Clamping alone keeps the SCROLL legal and says
        // nothing about the cursor, so shrinking the terminal with `:help` open
        // left the reversed row off screen and the next `j` snapped the page
        // back to it — the same symptom `<C-f>` had before P3, arriving through
        // the resize door instead (Reviewer A). This is the one place that
        // knows the new geometry, so it is the place that owes the follow.
        self.follow(scrolloff);
    }

    // --- the cursor -------------------------------------------------------

    /// Which source line the cursor is on.
    pub fn cursor(&self) -> usize {
        self.cursor.cursor().0
    }

    /// The cursor as `(source line, char)` — where the renderer draws it.
    pub fn position(&self) -> (usize, usize) {
        self.cursor.cursor()
    }

    /// **One action from the keymap**: `H` `M` `L` by this page's window,
    /// everything else by the read-only buffer the transcript uses
    /// (`prompt::Buffer::apply_read_only`), the window following. `false`
    /// when it takes nothing — an edit, which a page never applies.
    pub fn apply(&mut self, action: EditAction, count: usize, scrolloff: usize) -> bool {
        if let EditAction::Move(CursorMotion::Screen(which)) = action {
            self.screen_line(which, scrolloff);
            return true;
        }
        let taken = self.cursor.apply_read_only(action, count);
        self.follow(scrolloff);
        taken
    }

    /// **An edit, by `editor`, of the text this window holds** — its buffer
    /// lent for `f` ([`Prompt::lent`](crate::prompt::Prompt::lent)), the
    /// window following after, as it follows a motion. A modifiable page's
    /// (`page::PageBuffer::edit`), which takes the text it leaves and paints
    /// it ([`restyle`](Self::restyle)).
    pub(crate) fn edit<R>(
        &mut self,
        editor: &mut crate::prompt::Prompt,
        scrolloff: usize,
        f: impl FnOnce(&mut crate::prompt::Prompt) -> R,
    ) -> R {
        let out = editor.lent(&mut self.cursor, f);
        self.follow(scrolloff);
        out
    }

    /// The text the cursor moves through, lines joined by `'\n'` — what an
    /// [`edit`](Self::edit) left.
    pub(crate) fn text(&self) -> String {
        self.cursor.lines().collect::<Vec<_>>().join("\n")
    }

    /// **Its lines painted anew, their text unchanged** — after an
    /// [`edit`](Self::edit), whose text the cursor's buffer already holds.
    /// Not [`set_lines`](Self::set_lines): that clamps the cursor onto a
    /// char, and an Insert's rests past a line's last one.
    pub(crate) fn restyle(&mut self, lines: Vec<PageLine>) {
        debug_assert_eq!(
            texts(&lines),
            self.cursor.lines().collect::<Vec<_>>(),
            "restyled, not rewritten"
        );
        self.lines = lines;
        *self.geometry.get_mut() = None;
    }

    /// The selected chars of source line `line` (`len` chars long), `[lo,
    /// hi)` — `None` outside the selection; by the buffer's own projection,
    /// so what is drawn is what `y` takes, in each of `v` `V` `<C-v>`.
    pub fn selected_char_range(&self, line: usize, len: usize) -> Option<(usize, usize)> {
        self.cursor.selected_char_range(line, len)
    }

    /// The source lines a Visual selection spans — an empty one in it too,
    /// which has no chars to select; `None` without one.
    pub fn selected_lines(&self) -> Option<std::ops::RangeInclusive<usize>> {
        self.cursor.selected_lines()
    }

    /// Is a Visual selection up?
    pub fn selecting(&self) -> bool {
        self.cursor.anchor().is_some()
    }

    /// Drop the Visual selection.
    pub fn clear_selection(&mut self) {
        self.cursor.clear_anchor();
    }

    /// **`y`** — the text `target` spans (a selection, `yy`, `yw`, `yiw`),
    /// and how it was shaped, for the register `p` pastes in the prompt; the
    /// transcript's yank (`prompt::Buffer::yank`). `None` when it spans
    /// nothing.
    pub fn yank(
        &mut self,
        target: OpTarget,
        count: usize,
    ) -> Option<(crate::clipboard::Copied, SelectionKind)> {
        self.cursor.yank(target, count)
    }

    /// **`/` `?` `n` `N` on a page** — the transcript's search
    /// (`prompt::Buffer::search`), the cursor on a char and the window
    /// following. `Some(wrapped)`, or `None` when nothing matches.
    pub fn search(
        &mut self,
        re: &regex::Regex,
        forward: bool,
        count: usize,
        scrolloff: usize,
    ) -> Option<bool> {
        let hit = self.cursor.search(re, forward, count);
        self.cursor.clamp_cursor_to_char();
        self.follow(scrolloff);
        hit
    }

    /// What `*` and `#` search for on this page, the cursor on its start
    /// (`prompt::Buffer::cursor_to_word`).
    pub fn cursor_to_word(&mut self) -> Option<(String, bool)> {
        self.cursor.cursor_to_word()
    }

    /// **Where a click lands** — the char drawn at `cell` of screen row
    /// `row` (both from the content's top-left) in a page drawn `width`
    /// wide: the cursor goes there, by the rows `visible_rows` draws, so a
    /// press and a drag land where they point (Steven, 10-07: the mouse
    /// selected across panes, or nothing). Past a row's end, its last char;
    /// below the page's end, nowhere.
    ///
    /// `typing`: in Insert or Replace (a page you may change), where a
    /// cursor may rest past a line's last char, as the prompt's click lets
    /// it: clamped, the next char went in before the last (review stop 16,
    /// A L5).
    pub fn click(&mut self, row: usize, cell: usize, width: u16, typing: bool) {
        let Some(PageRow {
            line,
            start,
            row: drawn,
        }) = self.visible_rows(width, row + 1).into_iter().nth(row)
        else {
            return;
        };
        let (mut used, mut col) = (0, start);
        for c in drawn.text().chars() {
            let w = crate::wrap::char_cells(c);
            if used + w > cell {
                break;
            }
            used += w;
            col += 1;
        }
        self.cursor.place_cursor(line, col);
        if !typing {
            self.cursor.clamp_cursor_to_char();
        }
    }

    /// **Where the cursor is drawn** in a page `width` × `height`: its row
    /// from the window's top and its cell — `click`'s inverse, by the rows
    /// `visible_rows` draws. The transcript's map
    /// ([`cursor_in_wrapped_word`](crate::wrap::cursor_in_wrapped_word)), so
    /// a char a wrap drops — the space at a break — is where the transcript
    /// puts it: the end of its row, or the next row's start when that row is
    /// full. A page measured it by hand and drew it past a full row's edge,
    /// clipped away (Reviewer D, F4). `None` off the window.
    pub fn cursor_cell(&self, width: u16, height: usize) -> Option<(usize, usize)> {
        let (line, col) = self.position();
        let chars: Vec<char> = self.lines.get(line)?.text().chars().collect();
        let cells = width.max(1) as usize;
        let rows = crate::wrap::wrap_cells_word(&chars, cells);
        let (into, cell) =
            crate::wrap::cursor_in_wrapped_word(&chars, &rows, cells, col.min(chars.len()));
        let row = (self.rows_before(line, width) + into).checked_sub(self.top_row(width))?;
        (row < height).then_some((row, cell))
    }

    /// What the Visual selection covers — what a drag's release copies.
    pub fn selected(&self) -> Option<crate::clipboard::Copied> {
        self.cursor.selected()
    }

    /// Put the cursor on source line `line` (clamped), keeping its column
    /// where the line allows — as the buffer's own `j`/`k` do.
    fn put(&mut self, line: usize) {
        let col = self.cursor.cursor().1;
        self.cursor.place_cursor(line, col);
        self.cursor.clamp_cursor_to_char();
    }

    /// Move the cursor `delta` **source lines**, clamped at both ends, and let
    /// the window follow.
    ///
    /// The cursor pushes the view; the view never drags the cursor. That is the
    /// same rule the transcript and the graph obey, through the same function —
    /// see [`viewport::follow`](crate::viewport::follow).
    pub fn move_cursor(&mut self, delta: isize, scrolloff: usize) {
        let last = self.lines.len().saturating_sub(1);
        self.put(self.cursor().saturating_add_signed(delta).min(last));
        self.follow(scrolloff);
    }

    /// Move the cursor by `rows` **rendered rows** — a page, a half page, a
    /// wheel notch.
    ///
    /// The conversion belongs here because this type owns `rows_before`, and
    /// leaving it to the caller got it wrong: `scroll_delta` returns rendered
    /// rows and it was handed straight to `move_cursor`, which counts source
    /// lines. On the manual (written to 78 columns) in a 38-column pane, one
    /// `<C-f>` moved 19 source lines — 25 rendered rows through a 20-row
    /// window, skipping five rows unread per press, and worse the narrower the
    /// pane (Reviewer A).
    ///
    /// Walks line by line rather than inverting `rows_before`, because a line's
    /// height depends on the width and there is no closed form; the walk is
    /// bounded by the page size, not the document.
    pub fn page(&mut self, rows: isize, scrolloff: usize) {
        let Some((_, width)) = self.last_size else {
            // Never drawn: no geometry to convert against. Fall back to lines,
            // which is at least the right direction and the right order of
            // magnitude, and the next clamp settles the window.
            return self.move_cursor(rows, scrolloff);
        };
        // A line taller than the page is paged THROUGH by moving the window,
        // since there is no next line to move to that would not skip its tail.
        let visible = self.last_size.map_or(1, |(v, _)| v.max(1));
        let cursor = self.cursor();
        if self.line_rows(cursor, width) >= visible {
            let max_top = self.rows_at(width).saturating_sub(visible);
            let at = self.rows_before(cursor, width);
            let lo = at.min(max_top);
            let hi = (at + self.line_rows(cursor, width) - visible).min(max_top);
            let want = self.top_row(width).saturating_add_signed(rows);
            self.move_top(want.clamp(lo, hi), width);
            // Past either end of this line, hand off to the ordinary motion so
            // the cursor keeps travelling.
            if want > hi || (rows < 0 && want < lo) {
                return self.move_cursor(rows.signum(), scrolloff);
            }
            return;
        }
        let step = |i: usize| self.line_rows(i, width);
        let last = self.lines.len().saturating_sub(1);
        let mut budget = rows.unsigned_abs();
        let mut at = cursor;
        while budget > 0 {
            let next = if rows > 0 {
                if at >= last {
                    break;
                }
                at + 1
            } else {
                if at == 0 {
                    break;
                }
                at - 1
            };
            // Charge the line we are LEAVING when moving down, and the one we
            // are entering when moving up — the rows crossed between `c` and
            // `c+k` are `step(c..c+k-1)`, not `step(c+1..c+k)`. Off by one line
            // per page otherwise, so `<C-f>` then `<C-b>` did not round-trip
            // (Reviewer A).
            budget = budget.saturating_sub(if rows > 0 { step(at) } else { step(next) });
            at = next;
        }
        self.goto_line(at, scrolloff);
    }

    /// Put the cursor on `line` (clamped), its column kept where the line
    /// allows — where a page or a screen line lands.
    pub fn goto_line(&mut self, line: usize, scrolloff: usize) {
        self.put(line);
        self.follow(scrolloff);
    }

    /// Move the window the minimum needed to keep the cursor inside the margin,
    /// in the geometry this page was last drawn at.
    fn follow(&mut self, scrolloff: usize) {
        let Some((visible, width)) = self.last_size else {
            return; // never drawn yet; the next clamp settles it
        };
        let visible = visible.max(1);
        let at = self.rows_before(self.cursor(), width);
        let tall = self.line_rows(self.cursor(), width);
        // **A source line taller than the window is scrolled THROUGH, not
        // snapped to.**
        //
        // The cursor is line-granular, so positioning the window purely from
        // `rows_before(cursor)` makes at most one scroll offset reachable per
        // source line — and every rendered row past `top + visible` inside the
        // cursor's own line becomes unreachable by any key. On the decision
        // pane, whose report puts a whole command on ONE source line, that hid
        // the tail of a long `rm -rf …` with no way to reveal it, on the one
        // pane whose hidden content is a safety question (Reviewer A, round 2).
        //
        // It is also the failure this module's header says it exists to
        // prevent — "the tail is simply unreachable" — arriving through the
        // cursor door instead of the scroll one, and a regression from the
        // round-1 switch of `j`/`k` to `move_cursor`.
        //
        // So: while the window shows ANY part of the cursor's line, leave it
        // where the user put it, clamped to that line's band. Only when the
        // cursor's line is entirely off screen does the ordinary rule apply.
        let rows = self.rows_at(width);
        let scroll = self.top_row(width);
        // The last full page — or, if a `zt` already put the window past it,
        // wherever the window is: a cursor never pushes past the end, and never
        // pulls a window back from it either (`viewport::follow`, same rule).
        let max_top = rows.saturating_sub(visible).max(scroll);
        if tall >= visible {
            let lo = at;
            let hi = at + tall - visible;
            self.move_top(scroll.clamp(lo.min(max_top), hi.min(max_top)), width);
            return;
        }
        // **A line that FITS is shown whole**, not just its first row.
        //
        // The band above was written for a line taller than the window, and
        // stopped one case short: a wrapped line at the END of a document is
        // shorter than the window, so it took the ordinary rule below — which
        // positions from `rows_before(cursor)`, the line's FIRST row. `G` on a
        // four-row last line in a ten-row pane left three of its rows below the
        // window and no key could reach them, because the cursor cannot go
        // past the last line and the window only ever follows the cursor.
        //
        // Found by rewriting `scrolling_counts_wrapped_rows_so_the_tail_is_
        // reachable` onto the cursor API: the old view-only `scroll_bottom`
        // reached row 30 and the cursor reached 27. The test was named for the
        // property; the property had quietly stopped holding for the last line
        // (`[[feedback_fix_neighbourhood]]` — the second visit to this
        // function, one case narrower).
        //
        // This is vim's rule too: `G` on a wrapped final line shows the whole
        // line rather than parking the window on its first row.
        let top = crate::viewport::follow(at, scroll, rows, visible, scrolloff);
        self.move_top(
            top.max((at + tall).saturating_sub(visible)).min(max_top),
            width,
        );
    }

    /// How many rendered rows one source line occupies at `width` — one past
    /// the end, as an empty page draws one blank row.
    fn line_rows(&self, line: usize, width: u16) -> usize {
        if line >= self.lines.len() {
            return 1;
        }
        let starts = self.starts(width);
        starts[line + 1] - starts[line]
    }

    // --- relative to the screen: `H` `M` `L`, `zt` `zz` `zb` ------------------

    /// **The source lines on screen**, first and last, in the geometry this
    /// page was last drawn at — `None` before the first frame. The rule (a
    /// line cut at an edge is not on screen) is
    /// [`viewport::lines_on_screen`](crate::viewport::lines_on_screen); an
    /// empty page is one blank line, as it is drawn.
    fn lines_on_screen(&self) -> Option<(usize, usize)> {
        let (visible, width) = self.last_size?;
        let heights = (0..self.lines.len().max(1)).map(|line| self.line_rows(line, width));
        crate::viewport::lines_on_screen(heights, self.top_row(width), visible)
    }

    /// `H` / `M` / `L` — to a line by its place on the screen. The rule is
    /// [`viewport::screen_line`](crate::viewport::screen_line), shared with
    /// every pane; this only says which lines are on screen.
    pub fn screen_line(&mut self, which: crate::viewport::ScreenLine, scrolloff: usize) {
        let Some((first, last)) = self.lines_on_screen() else {
            return; // never drawn: there is no screen to be relative to
        };
        let target =
            crate::viewport::screen_line(which, first, last, self.lines.len(), scrolloff, false);
        self.goto_line(target, scrolloff);
    }

    /// `zt` / `zz` / `zb` — put the cursor's line at the top, middle or bottom
    /// of the window; with `line` (`20zt`), go to that 1-based line first. The
    /// rule is [`viewport::place_top`](crate::viewport::place_top).
    pub fn place(&mut self, at: crate::viewport::Place, line: Option<usize>, scrolloff: usize) {
        if let Some(line) = line {
            self.put(line.saturating_sub(1));
        }
        let Some((visible, width)) = self.last_size else {
            return; // never drawn: the next clamp shows the cursor
        };
        let row = self.rows_before(self.cursor(), width);
        let rows = self.line_rows(self.cursor(), width);
        let top = crate::viewport::place_top(at, row, rows, visible.max(1), scrolloff);
        self.put_top_row(top, width);
        self.follow(scrolloff);
    }
}

/// **One row a page draws** ([`TextView::visible_rows`]): its source line,
/// the char of that line it starts at — the cursor and a selection are in
/// line chars, so this is where a row finds its part of them — and the row.
#[derive(Debug, Clone, PartialEq)]
pub struct PageRow {
    pub line: usize,
    pub start: usize,
    pub row: PageLine,
}

impl PageRow {
    /// Is this its line's first row?
    pub fn first(&self) -> bool {
        self.start == 0
    }
}

/// Each line's text, for the buffer the cursor moves in.
fn texts(lines: &[PageLine]) -> Vec<String> {
    lines.iter().map(PageLine::text).collect()
}

#[cfg(test)]
impl TextView {
    /// `G` — the last line (a test's way there).
    pub fn cursor_bottom(&mut self, scrolloff: usize) {
        self.goto_line(usize::MAX, scrolloff);
    }

    /// **The page as a pane of `kind` paints it**, by the shipped theme —
    /// through the production painter (`view::painter`), so a test reading
    /// a colour reads the one the app chose. The tests' view of `lines`.
    pub(crate) fn painted_as(
        &self,
        kind: crate::workspace::PaneKind,
    ) -> Vec<ratatui::text::Line<'static>> {
        let theme = crate::theme::Theme::terminal();
        let paint = crate::view::painter(kind, &theme);
        self.lines.iter().map(|line| paint.line(line)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    fn view(lines: &[&str]) -> TextView {
        TextView::new(
            lines
                .iter()
                .map(|s| Line::from(s.to_string()))
                .collect::<Vec<_>>(),
        )
    }

    /// **`set_lines` invalidates the geometry memos.**
    ///
    /// They were memos with nothing to invalidate, because `lines` was written
    /// once by `new`. `set_lines` made them cacheable-and-stale, which is a
    /// different kind of object: every clamp, every `max_scroll` and every
    /// cursor-follow would be computed against a document that no longer
    /// exists, and none of them would look wrong (Reviewer B named this as the
    /// trap in the fix).
    ///
    /// Asserts on the ANSWER, not on the memo field: a test that reads the
    /// `Cell` is testing that a cache was cleared, and what matters is that the
    /// measurement is right.
    /// **A cursor on the space a full row drops is still on screen** — the
    /// transcript's convention, the next row's start. A page measured it by
    /// hand, at the full row's end, a cell past the window: the cursor drawn
    /// there was clipped away (Reviewer D, F4). Measured from the window's
    /// top, and `None` off it.
    #[test]
    fn a_cursor_on_a_dropped_break_space_is_on_screen() {
        let mut v = view(&["top", "abcd efgh"]);
        v.goto_line(1, 0);
        for _ in 0..4 {
            v.apply(EditAction::Move(crate::keymap::CursorMotion::Right), 1, 0);
        }
        assert_eq!(v.position(), (1, 4), "premise: on the space");
        assert_eq!(v.cursor_cell(4, 10), Some((2, 0)), "the next row's start");
        assert_eq!(
            v.cursor_cell(20, 10),
            Some((1, 4)),
            "unwrapped, its own cell"
        );
        assert_eq!(v.cursor_cell(4, 2), None, "below a window two rows tall");
        // Scrolled: counted from the window's top, not the page's.
        let mut v = view(&["a", "b", "c", "d", "e"]);
        v.goto_line(4, 0);
        v.clamp(2, 20, 0);
        assert_eq!(
            v.cursor_cell(20, 2),
            Some((1, 0)),
            "the window's second row"
        );
    }

    #[test]
    fn replacing_the_content_replaces_its_measurements() {
        let mut v = view(&["a", "b", "c"]);
        assert_eq!(v.rows_at(20), 3, "premise — the memo is populated by this");
        assert_eq!(v.rows_before(2, 20), 2, "premise — and so is its sibling");

        v.set_lines(
            (0..10)
                .map(|i| Line::from(format!("line {i}")))
                .collect::<Vec<_>>(),
        );

        assert_eq!(v.rows_at(20), 10, "a stale row count strands the tail");
        assert_eq!(
            v.rows_before(5, 20),
            5,
            "a stale offset lands the cursor wrong"
        );
    }

    /// A shrinking replacement cannot leave the cursor past the end.
    #[test]
    fn replacing_the_content_bounds_the_cursor() {
        let mut v = view(&["a", "b", "c", "d", "e"]);
        v.goto_line(4, 0);
        assert_eq!(v.cursor(), 4, "premise — the cursor is at the last line");
        v.set_lines(vec![Line::from("only")]);
        assert_eq!(
            v.cursor(),
            0,
            "the cursor must land inside the new document"
        );
        // …and an empty document is a real case (a graph pane with nothing
        // selected renders one placeholder line, but a future caller may not).
        v.set_lines(Vec::<Line>::new());
        assert_eq!(v.cursor(), 0);
    }

    /// **And the selection's other end**: a range past the end of a shrunk
    /// page is bounded like the cursor, not dropped — the keymap is still in
    /// Visual. `o` swaps the two; `y` takes the lines.
    #[test]
    fn replacing_the_content_bounds_the_selection() {
        let mut v = view(&["a", "b", "c", "d", "e"]);
        v.goto_line(4, 0);
        assert!(v.apply(EditAction::SetAnchorLine, 1, 0));
        v.goto_line(0, 0);
        assert_eq!(
            v.selected_char_range(4, 1),
            Some((0, 1)),
            "premise: the ANCHOR is the far end"
        );
        v.set_lines(vec![Line::from("x"), Line::from("y")]);
        for line in 0..2 {
            assert_eq!(v.selected_char_range(line, 1), Some((0, 1)), "line {line}");
        }
        assert!(v.apply(EditAction::SwapSelectionEnds, 1, 0));
        assert_eq!(v.cursor(), 1, "`o`: the cursor to the anchor");
        let (copied, kind) = v.yank(OpTarget::Selection, 1).expect("lines");
        assert_eq!(copied.text(), "x\ny\n");
        assert_eq!(kind, SelectionKind::Line);
        assert!(!v.selecting(), "`y` spends the range");
    }

    /// **A page takes the transcript's motions and Visual** — `w`, `e`,
    /// `iw` and a charwise `y` (Steven, 10-07: "visually highlight some
    /// text from the help file, yank it, and paste it into a prompt"), and
    /// refuses an edit.
    #[test]
    fn a_page_moves_by_words_and_yanks_part_of_a_line() {
        use crate::keymap::{TextObject, TextObjectScope};
        let mut v = view(&["see `:help` for more", "next"]);
        assert!(v.apply(EditAction::Move(CursorMotion::WordForward), 1, 0));
        assert_eq!(v.position(), (0, 4), "`w` stops at the backtick");
        v.apply(EditAction::Move(CursorMotion::WordForward), 1, 0);
        assert_eq!(v.position(), (0, 6), "then `help`: `` `: `` is one word");
        assert!(v.apply(
            EditAction::SelectTextObject {
                scope: TextObjectScope::Around,
                object: TextObject::BigWord,
                count: 1,
            },
            1,
            0
        ));
        let (copied, kind) = v.yank(OpTarget::Selection, 1).expect("a word");
        assert_eq!(copied.text(), "`:help` ");
        assert_eq!(kind, SelectionKind::Char);
        assert!(!v.apply(EditAction::Undo, 1, 0), "a page applies no edit");
    }

    #[test]
    fn content_that_fits_does_not_scroll() {
        let mut v = view(&["a", "b", "c"]);
        v.clamp(10, 80, 0);
        v.page(1, 0);
        assert_eq!(v.scroll(), 0, "nothing to scroll to");
        v.cursor_bottom(0);
        assert_eq!(v.scroll(), 0);
    }

    /// The bug the four copies shared: clamping against the LINE count while
    /// the pane renders wrapped rows. Ten 100-cell lines are ten rows at width
    /// 100 but forty at width 25 — and at width 25 a line-count clamp strands
    /// three quarters of the document.
    #[test]
    fn scrolling_counts_wrapped_rows_so_the_tail_is_reachable() {
        let mut v = view(&["x".repeat(100).as_str(); 10]);
        assert_eq!(v.rows_at(100), 10);
        assert_eq!(v.rows_at(25), 40);

        v.clamp(10, 25, 0);
        v.cursor_bottom(0);
        assert_eq!(
            v.scroll(),
            30,
            "40 wrapped rows in a 10-row pane: the last screenful starts at 30"
        );
        // The line-count clamp would have stopped here, hiding rows 10..40.
        assert!(v.scroll() > 10 - 1);
    }

    #[test]
    fn an_empty_line_still_occupies_a_row() {
        let v = view(&["a", "", "b"]);
        assert_eq!(v.rows_at(80), 3);
    }

    #[test]
    fn wide_characters_wrap_by_display_cells() {
        // Six CJK chars = 12 cells: two rows at width 6, not one.
        let v = view(&["世界世界世界"]);
        assert_eq!(v.rows_at(6), 2);
    }

    #[test]
    fn scrolling_clamps_at_both_ends() {
        let mut v = view(&["line"; 100]);
        v.clamp(10, 80, 0);
        v.page(-5, 0);
        assert_eq!(v.scroll(), 0, "cannot scroll above the top");
        v.page(1_000, 0);
        assert_eq!(v.scroll(), 90);
        v.page(1, 0);
        assert_eq!(v.scroll(), 90, "cannot scroll past the last screenful");
    }

    /// Shrinking the pane must re-clamp, or the view renders blank: the offset
    /// is still pointing past the end of a now-narrower, taller document.
    #[test]
    fn clamp_recovers_after_the_viewport_shrinks() {
        let mut v = view(&["line"; 100]);
        v.clamp(50, 80, 0);
        v.cursor_bottom(0);
        assert_eq!(v.scroll(), 50);
        v.clamp(100, 80, 0);
        assert_eq!(v.scroll(), 0, "everything fits now");
    }

    /// **Every rendered row can be brought on screen by some key.**
    ///
    /// The cursor is line-granular, so positioning the window purely from
    /// `rows_before(cursor)` makes one scroll offset reachable per SOURCE line
    /// — and a source line taller than the window hides its own tail from every
    /// key. The decision pane puts a whole command on one source line, so a
    /// long `rm -rf …` was truncated with no way to reveal the rest, on the one
    /// pane whose hidden content is a safety question (Reviewer A, round 2).
    ///
    /// Swept rather than sampled: drive the motions a user has and collect the
    /// set of visible rows. A test asserting "it scrolled" would pass with nine
    /// of nineteen rows unreachable, which is what the round-1 tests did.
    ///
    /// **Over several document SHAPES**, because where the wrapped line sits
    /// changes which rule reaches it. With the long line in the middle and a
    /// one-row `tail` after it, this passed while a wrapped line at the END of
    /// the document still hid its own tail — that line is shorter than the
    /// window, so it missed the tall-line band, and the cursor cannot move past
    /// it to drag the window down. One shape, one rule, and the case that hid
    /// was the one nobody wrote a line for.
    /// **What this cannot see, named rather than left for a reader to
    /// discover.** `total` comes from [`rows_at`](TextView::rows_at), so this
    /// proves every row *we measure* is reachable — not every row the renderer
    /// draws. Those are the same number only while our ruler is right. Until
    /// S4 it was not: `rows_at` hard-wrapped by cells while `draw_text_pane`
    /// rendered with ratatui's word wrapper, and the real manual at 38
    /// columns measured 2450 rows and drew 2549 (Reviewer B). That is closed
    /// by one ruler, and held by `the_rows_counted_are_the_rows_drawn` — the
    /// test that compares the count with the DRAWING, which this one cannot.
    ///
    /// The fixtures hid the gap a second way: an unbroken run of `x` has no
    /// break opportunity, so word wrap and hard wrap agree on it exactly. A
    /// word-breakable fixture is below, and it passed while the gap was live,
    /// because the mismeasurement was in the RULER both sides of this test
    /// share (`[[feedback_fixture_omits_the_axis]]`).
    #[test]
    fn every_row_we_measure_is_reachable_by_some_key() {
        let long = "x".repeat(600);
        // A wrapped line that FITS the window, at the end — four rows in ten.
        let medium = "z".repeat(140);
        let words = "alpha bravo charlie delta echo foxtrot golf hotel ".repeat(12);
        for doc in [
            vec!["short", &long, "tail"],
            vec!["short", &long, &medium],
            // No tall line at all — just enough short ones to overflow, with
            // the wrapped one last.
            vec!["a", "b", "c", "d", "e", "f", "g", &medium],
            // Word-BREAKABLE, which the two above are not: a fixture with no
            // break opportunity cannot catch anything about word wrap at all.
            // That the rows counted here are the rows drawn is
            // `the_rows_counted_are_the_rows_drawn`.
            vec!["short", &words, "tail"],
        ] {
            every_row_reachable_in(&doc);
        }
    }

    /// The sweep above, for one document.
    fn every_row_reachable_in(lines: &[&str]) {
        let mut v = view(lines);
        let (visible, width) = (10usize, 38u16);
        v.clamp(visible, width, 0);
        let total = v.rows_at(width);
        assert!(total > visible, "premise: it overflows ({total} rows)");

        let mut seen = std::collections::BTreeSet::new();
        let mut note = |v: &TextView| {
            for r in v.scroll() as usize..(v.scroll() as usize + visible).min(total) {
                seen.insert(r);
            }
        };
        note(&v);
        for _ in 0..40 {
            v.move_cursor(1, 0);
            note(&v);
        }
        for _ in 0..10 {
            v.page(visible as isize, 0);
            note(&v);
        }
        for _ in 0..40 {
            v.move_cursor(-1, 0);
            note(&v);
        }
        for _ in 0..10 {
            v.page(-(visible as isize), 0);
            note(&v);
        }
        let missing: Vec<usize> = (0..total).filter(|r| !seen.contains(r)).collect();
        assert!(missing.is_empty(), "rows no key can reach: {missing:?}");
    }

    /// **The rows counted are the rows drawn** — one ruler (TODO.md's
    /// CRITICAL: rows were counted by hard cell wrap and drawn by ratatui's
    /// word wrap, and the manual at 38 columns counted 2450 rows and drew
    /// 2549 — 99 no key reached, and the decision pane sized short). For the
    /// shipped manual and word-breaking lines, at several widths: the page
    /// drawn whole is exactly `rows_at` rows, none wider than the page, each
    /// line's first row marked, and each line's rows spell it, less the
    /// spaces a break drops.
    #[test]
    fn the_rows_counted_are_the_rows_drawn() {
        let manual = TextView::new(crate::help_view::manual());
        let words = view(&[
            "aaaaaa aaaaaa aaaaaa",
            "",
            "  indented words that run past the edge of a narrow page",
            "averyveryverylongwordwithnobreakatall then words",
            "中文字符 wide 字 and narrow",
        ]);
        for page in [manual, words] {
            for width in [10u16, 38, 40, 79] {
                let total = page.rows_at(width);
                let drawn = page.visible_rows(width, total + 5);
                assert_eq!(drawn.len(), total, "width {width}");
                let mut lines: Vec<String> = vec![String::new(); page.lines().len()];
                for PageRow { line, start, row } in &drawn {
                    assert!(row.cells() <= width as usize, "{:?} at {width}", row.text());
                    assert_eq!(
                        *start == 0,
                        lines[*line].is_empty(),
                        "a line's first row is its first"
                    );
                    lines[*line].push_str(&row.text());
                }
                for (i, line) in page.lines().iter().enumerate() {
                    let squeeze = |s: &str| s.replace(' ', "");
                    assert_eq!(
                        squeeze(&lines[i]),
                        squeeze(&line.text()),
                        "line {i} at {width}"
                    );
                }
            }
        }
        // Wrapped whole and cut window by window, the same rows.
        let wide = view(&["a b c d e f g h i j k l m n o p q r s t u v w x y z"]);
        let all = wide.lines()[0].wrap(5);
        assert_eq!(
            wide.visible_rows(5, 100)
                .into_iter()
                .map(|r| r.row)
                .collect::<Vec<_>>(),
            all
        );
        // The case the reviewers measured: three words in ten cells are three
        // rows, counted and drawn (hard wrap counted two).
        let three = view(&["aaaaaa aaaaaa aaaaaa"]);
        assert_eq!(three.rows_at(10), 3);
        assert_eq!(three.visible_rows(10, 10).len(), 3);
    }

    /// **Drawing a long line is linear in it** — a model-written command
    /// with one unbroken word is the input. Each row was sliced from its
    /// run's start, so 4× the line cost ~16× (64K characters: half a second
    /// a frame in debug; review 28, Reviewer C). Measured as a RATIO of the
    /// best of several runs, so a busy machine slows both sides alike: linear
    /// is ~4, quadratic ~16; the bound is 8.
    #[test]
    fn drawing_a_long_line_is_linear() {
        let best = |n: usize| {
            let page = view(&[&"x".repeat(n)]);
            let _ = page.rows_at(80);
            (0..5)
                .map(|_| {
                    let started = std::time::Instant::now();
                    let rows = page.visible_rows(80, 30);
                    let took = started.elapsed();
                    assert_eq!(rows.len(), 30);
                    took
                })
                .min()
                .unwrap()
        };
        let (small, large) = (best(16_000), best(64_000));
        let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
        eprintln!("16K: {small:?}, 64K: {large:?}, ratio {ratio:.1}");
        assert!(
            ratio < 8.0,
            "4x the line cost {ratio:.1}x: {small:?} → {large:?}"
        );
    }

    /// **A held jump past the end lands on the last line** — `reveal`
    /// waits for a width, and the page can shrink before it gets one
    /// (`set_lines` clamps the cursor, not the held line). `rows_before`'s
    /// clamp is what kept that from indexing past its table; no caller
    /// reached it, so its deletion passed (Reviewer B, round 28).
    #[test]
    fn a_held_jump_past_the_end_lands_on_the_last_line() {
        let mut v = TextView::new((0..10).map(|i| Line::from(format!("line {i}"))));
        v.reveal(9, crate::viewport::Place::Top);
        v.set_lines(["a", "b", "c"].map(Line::from));
        v.clamp(5, 40, 0);
        assert_eq!(v.cursor(), 2);
    }

    /// A resize must leave the cursor ON SCREEN, not merely leave the scroll
    /// legal. `clamp`'s `follow` call was a guard whose deletion passed all 911
    /// tests: `clamp_recovers_after_the_viewport_shrinks` exercises only the
    /// `.min(max_scroll)` half (Reviewer A, round 2).
    #[test]
    fn a_resize_keeps_the_cursor_on_screen() {
        let mut v = view(&["line"; 60]);
        v.clamp(40, 80, 0);
        v.cursor_bottom(0);
        v.clamp(8, 80, 0);
        let at = v.rows_before(v.cursor(), 80);
        let top = v.scroll() as usize;
        assert!(
            (top..top + 8).contains(&at),
            "cursor row {at} is outside the window [{top}, {})",
            top + 8
        );
    }

    /// `page` moves at most ONE screenful. Asserting only that it moved is what
    /// let the row/line unit mismatch survive: `<C-f>` moved 34 rendered rows
    /// through a 20-row window, skipping 14 unread (Reviewer A, round 2).
    #[test]
    fn a_page_moves_at_most_one_screenful() {
        let wrapped: Vec<&str> = vec!["y".repeat(200).leak(); 40];
        let mut v = TextView::new(wrapped.iter().map(|s| Line::from(s.to_string())));
        let (visible, width) = (20usize, 38u16);
        v.clamp(visible, width, 0);
        let before = v.scroll() as usize;
        v.page(visible as isize, 0);
        let moved = v.scroll() as usize - before;
        assert!(
            moved <= visible,
            "one page moved {moved} rendered rows through a {visible}-row window"
        );
        assert!(moved > 0, "and it did move");
    }

    /// **A frame must not undo a page taken inside a tall line.**
    ///
    /// `clamp_text_panes` runs before every draw and `clamp` follows, so if
    /// `follow` snaps the window to the cursor line's FIRST row, paging into
    /// the middle of a long line is reverted on the next frame and the tail
    /// stays unreachable. The reachability sweep alone does not catch that —
    /// `page` positions the window itself, so it masks `follow` (found by
    /// mutation: snapping to `lo` left the sweep green).
    #[test]
    fn a_frame_does_not_undo_a_page_inside_a_tall_line() {
        let long = "x".repeat(600);
        let mut v = view(&["short", &long, "tail"]);
        let (visible, width) = (10usize, 38u16);
        v.clamp(visible, width, 0);
        v.move_cursor(1, 0); // onto the tall line
        // A partial page, so the window stays INSIDE that line's band — a full
        // page overshoots it and hands off to the next line, which is correct
        // behaviour and not what this test is about. (My first version paged a
        // whole screen and so never exercised the branch at all.)
        v.page(3, 0);
        let paged = v.scroll();
        assert!(paged > 0, "premise: the page moved the window");
        assert_eq!(v.cursor(), 1, "premise: and stayed on the tall line");

        v.clamp(visible, width, 0); // the next frame
        assert_eq!(
            v.scroll(),
            paged,
            "the frame dragged the window back to the line's first row"
        );
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;
    use ratatui::text::Line;

    /// Build a document from a list of line lengths.
    fn doc(lens: &[usize]) -> TextView {
        TextView::new(
            lens.iter()
                .map(|n| Line::from("w".repeat(*n)))
                .collect::<Vec<_>>(),
        )
    }

    proptest! {
        /// **The cursor's line is always at least partly on screen** — and when
        /// it FITS, all of it is.
        ///
        /// Stated as a property because the two rules were found one at a time,
        /// each by a different concrete failure: a line taller than the window
        /// hid its own tail (round 2), and then a wrapped line at the END of a
        /// document hid its own tail for the opposite reason (round 3). Two
        /// instances of one property is where enumerating cases stops being the
        /// cheaper move (`[[project_shannon_n_equals_two]]`).
        #[test]
        fn follow_keeps_the_cursor_line_visible(
            lens in proptest::collection::vec(0usize..300, 1..12),
            width in 4u16..60,
            visible in 1usize..40,
            scrolloff in 0usize..6,
            cursor in 0usize..12,
            paged in -3i32..4,
        ) {
            let mut v = doc(&lens);
            v.clamp(visible, width, scrolloff);
            v.goto_line(cursor, scrolloff);
            v.page(paged as isize, scrolloff);
            v.clamp(visible, width, scrolloff);

            let at = v.rows_before(v.cursor(), width);
            let tall = v.line_rows(v.cursor(), width);
            let top = v.scroll() as usize;
            let rows = v.rows_at(width);

            // Some part of the cursor's line is inside the window.
            prop_assert!(
                top < at + tall && top + visible > at,
                "cursor line rows [{}, {}) outside window [{}, {})",
                at,
                at + tall,
                top,
                top + visible
            );
            // The scroll never points past the last screenful.
            prop_assert!(top <= rows.saturating_sub(visible));
            // When the line fits and the document overflows, the WHOLE line is
            // on screen: the failure that hid the tail of a final wrapped line
            // was showing only its first row.
            if tall <= visible && rows >= visible {
                prop_assert!(
                    top <= at && at + tall <= top + visible,
                    "a line that fits was shown in part: rows [{}, {}) in window [{}, {})",
                    at,
                    at + tall,
                    top,
                    top + visible
                );
            }
        }

        /// `clamp`, `page` and the cursor jumps never panic, at any geometry —
        /// including the degenerate ones a resize really produces.
        #[test]
        fn a_text_view_never_panics_on_any_geometry(
            lens in proptest::collection::vec(0usize..80, 0..6),
            width in 0u16..8,
            visible in 0usize..4,
            scrolloff in 0usize..9,
        ) {
            let mut v = doc(&lens);
            v.clamp(visible, width, scrolloff);
            v.cursor_bottom(scrolloff);
            v.page(1, scrolloff);
            v.page(-1, scrolloff);
            v.move_cursor(-9, scrolloff);
            v.clamp(visible, width, scrolloff);
            prop_assert!(v.scroll() as usize <= v.rows_at(width));
        }
    }
}

#[cfg(test)]
mod on_screen_tests {
    use super::*;
    use ratatui::text::Line;

    /// Ten lines of 15 cells: at width 10 each wraps to TWO rows.
    fn wrapped() -> TextView {
        TextView::new(
            (0..10)
                .map(|i| Line::from(format!("{i}{}", "x".repeat(14))))
                .collect::<Vec<_>>(),
        )
    }

    /// **A line cut off at either edge is not "on screen"** — vim never shows
    /// one, and `H`/`L` go to lines you can see. Six rows from row 3 show the
    /// second half of line 1, lines 2 and 3 whole, and the first half of line
    /// 4: on screen are 2..=3. (Six, not five: five rows end exactly on a line
    /// boundary, so line 4 is not shown at all and the bottom cut is never
    /// exercised — which a surviving mutation showed.)
    #[test]
    fn a_line_cut_at_an_edge_does_not_count_as_shown() {
        let mut v = wrapped();
        v.clamp(6, 10, 0);
        v.put_top_row(3, 10);
        assert_eq!(v.lines_on_screen(), Some((2, 3)));
        v.screen_line(crate::viewport::ScreenLine::Top(1), 0);
        assert_eq!(v.cursor(), 2, "H skips the half-shown line 1");
        v.screen_line(crate::viewport::ScreenLine::Bottom(1), 0);
        assert_eq!(v.cursor(), 3, "L skips the half-shown line 4");
    }

    /// Unless it is the only line there is: a line taller than the window is
    /// on screen by itself, cut at both edges.
    #[test]
    fn a_line_taller_than_the_window_is_on_screen() {
        let mut v = TextView::new(vec![Line::from("x".repeat(60)), Line::from("tail")]);
        v.clamp(2, 10, 0);
        v.put_top_row(2, 10);
        assert_eq!(v.lines_on_screen(), Some((0, 0)));
    }

    /// Before the first frame there is no screen to be relative to.
    #[test]
    fn nothing_is_on_screen_before_the_first_frame() {
        assert_eq!(wrapped().lines_on_screen(), None);
    }

    /// The source line of the row at the top of `v`, drawn `w` wide.
    fn line_at_top(v: &TextView, w: u16) -> usize {
        v.visible_rows(w, 1)[0].line
    }

    /// **A page keeps its top line at another width** (review stop 15, D M3,
    /// measured: 200 lines that wrap twice as often at half the width, line
    /// 100 at the top, 120 → 60 columns — it came back with line 94 there;
    /// Neovim keeps 100). Its top was a row, and a row is a width's.
    #[test]
    fn a_page_keeps_its_top_line_at_another_width() {
        let mut v = TextView::new(
            (0..200)
                .map(|i| Line::from(format!("{i:03} {}", "word ".repeat(22))))
                .collect::<Vec<_>>(),
        );
        v.clamp(20, 120, 0);
        v.place(crate::viewport::Place::Top, Some(101), 0);
        assert_eq!(line_at_top(&v, 120), 100, "premise");
        assert_eq!(v.line_rows(100, 60), 2, "premise: two rows at half width");
        v.clamp(20, 60, 0);
        assert_eq!(line_at_top(&v, 60), 100, "Neovim keeps 100");
        assert_eq!(v.cursor(), 100);
        v.clamp(20, 120, 0);
        assert_eq!(line_at_top(&v, 120), 100, "and back");
    }

    /// **A page's top that did not move keeps its text** (`move_top`): its
    /// top on a long line's second row at 17 columns, drawn at 80 — where that
    /// char is part-way into the line's one row — moves nothing, and back at
    /// 17 the same row is at the top. Rewritten from the row, it crept up a
    /// row each round trip.
    #[test]
    fn a_page_top_that_does_not_move_keeps_its_text() {
        let mut lines: Vec<Line> = (0..60).map(|i| Line::from(format!("line {i}"))).collect();
        lines[30] = Line::from("a long line that wraps at seventeen cells");
        let mut v = TextView::new(lines);
        v.clamp(5, 17, 0);
        v.goto_line(31, 0);
        let second = v.rows_before(30, 17) + 1;
        v.put_top_row(second, 17);
        let held = v.top;
        assert!(held.0 == 30 && held.1 > 0, "premise: part-way in, {held:?}");
        v.clamp(5, 80, 0);
        assert_eq!(v.top, held, "nothing moved, nothing rewritten");
        v.clamp(5, 17, 0);
        assert_eq!(v.top_row(17), second);
    }

    /// **New lines above move a page's top with its line, drawn or not** — the
    /// top is text; as a row it could be read as a line only in a window drawn
    /// before, so a page not yet drawn kept row 0 while a drawn one moved.
    #[test]
    fn new_lines_above_move_the_top_drawn_or_not() {
        let more = || {
            (0..13)
                .map(|i| Line::from(format!("{i}")))
                .collect::<Vec<_>>()
        };
        let mut never = wrapped();
        never.top = (4, 0);
        never.set_lines_moved(more(), |line| Some(line + 3));
        assert_eq!(never.top, (7, 0));
        let mut drawn = wrapped();
        drawn.clamp(4, 10, 0);
        drawn.put_top_row(8, 10);
        assert_eq!(drawn.top, (4, 0), "premise: line 4, at two rows a line");
        drawn.set_lines_moved(more(), |line| Some(line + 3));
        assert_eq!(drawn.top, (7, 0));
    }
}
