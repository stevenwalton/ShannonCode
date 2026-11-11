//! Multi-line prompt editor.
//!
//! Buffer is stored as `Vec<Vec<char>>` so cursor arithmetic is O(1)
//! per edit regardless of multi-byte content. We convert to `String`
//! only at render and submit time; in M1 the buffer is small by
//! construction (capped at the prompt-pane's editable area in normal
//! use), so the conversion cost is irrelevant.
//!
//! Cursor columns are stored as *char* indices (O(1) edits). The
//! *visual* cursor position the renderer needs is computed in terminal
//! display cells via the shared [`crate::wrap`] primitive, so wide
//! characters (CJK, emoji) position correctly — the M1 "wide chars
//! misposition the cursor" bug is closed. (Grapheme-cluster-aware cursor
//! *motion* — treating a base+combining sequence as one stop — is a
//! separate, later concern; `unicode-segmentation` when it's wanted.)
//!
//! Key handling lives in `keymap.rs` and `vim_keymap.rs`. This module
//! exposes only editing primitives plus a single `apply(EditAction)`
//! entry point — the prompt has no opinion about which physical key
//! triggers what.

use std::cell::RefCell;
use std::rc::Rc;

use crate::clipboard::Copied;
use crate::keymap::{
    BlockEdge, CaseOp, CursorMotion, EditAction, Find, InsertEntry, OpTarget, Operator,
    SelectionKind, TextObject, TextObjectScope,
};
use crate::word;
use crate::wrap::{cells_width, cursor_in_wrapped, wrap_cells};

/// Soft-wrapped window of the prompt buffer. Shape mirrors
/// `output::VisibleWindow` so a future shared `Viewport`
/// abstraction can absorb both panes' scroll math; the only extra
/// here is `cursor_visual`, since `Output` has no cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptWindow {
    /// Visible visual rows top-to-bottom, already wrapped to width.
    /// `rows.len() <= height`.
    pub rows: Vec<String>,
    /// Cursor position *within* the window, in `(row, col)` visual
    /// coordinates. Translates directly into a terminal cursor
    /// offset once the pane's `(x, y)` origin is added.
    pub cursor_visual: (usize, usize),
    /// Total visual rows in the wrapped buffer (incl. any phantom
    /// continuation row the cursor created). Useful for status
    /// counters and bound-checking.
    pub total_rows: usize,
    /// Index of the topmost visible visual row.
    pub top: usize,
    /// Per visible row, the selected `[lo, hi)` *char* range within that
    /// row's string (the Visual highlight), or `None` if nothing on the
    /// row is selected. Char offsets (not cells) so the renderer can split
    /// the string directly and let the terminal handle wide-char widths.
    /// Aligned 1:1 with `rows`.
    pub selection: Vec<Option<(usize, usize)>>,
    /// Where each row begins in the buffer — `(logical line, char offset)`,
    /// aligned 1:1 with `rows` — so a renderer can match on the LOGICAL line
    /// and slice to the row: a marked secret long enough to wrap is still one
    /// secret (`view::row_secrets`).
    pub origins: Vec<(usize, usize)>,
}

/// One visual row of a wrapped [`Buffer`]: the logical `line` it shows, the
/// char it `start`s at, and how many chars (`len`) it holds — `Buffer::layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowSpan {
    line: usize,
    start: usize,
    len: usize,
}

/// **The first row of a `height`-row window asked to start at `top`** that
/// shows the cursor's row: `top` itself when the cursor is inside it, else
/// the nearest top that shows it — `viewport::follow` with no margin, so a
/// window drawn before it was settled still has its cursor on screen.
fn showing(top: usize, cursor: usize, rows: usize, height: usize) -> usize {
    crate::viewport::follow(cursor, top, rows, height, 0)
}

/// The **navigation + selection core** shared by the prompt editor and the
/// chat viewer: a line buffer, a cursor, and the Visual-selection state. The
/// cursor motions, the selection geometry, and the non-mutating yank
/// (`read`/`resolve`) are defined over it; `Prompt` adds the editing layer
/// (insert/delete/undo/register/paste) on top, and the chat pane will use a
/// `Buffer` read-only — the same pane framework, with mutation gated off.
/// Fields are private to this module, so `Prompt`'s editing methods reach
/// `self.buf.lines` directly.
#[derive(Debug, Clone)]
pub struct Buffer {
    /// One entry per logical line. Within a line, chars are in the order
    /// the user typed them.
    lines: Vec<Vec<char>>,
    /// Row of the cursor in `lines`.
    cursor_row: usize,
    /// Column of the cursor within `lines[cursor_row]`, in *chars*.
    /// Invariant: `cursor_col <= lines[cursor_row].len()`.
    cursor_col: usize,
    /// Visual-mode selection anchor `(row, col)`, set when entering Visual
    /// (`v`/`V`). The selection spans from here to the cursor; `None`
    /// outside Visual mode. The renderer reads it to highlight; operators
    /// consume it. Cleared on submit, on a selection operator, and on `Esc`.
    selection_anchor: Option<(usize, usize)>,
    /// The active selection's geometry — charwise (`v`), linewise (`V`), or
    /// block (`Ctrl-V`) — set by the mode the keymap is in. Decides how the
    /// selection resolves for both the operator (`resolve`) and the
    /// highlight (`visible_window`). Meaningful only while `selection_anchor`
    /// is `Some`.
    selection_kind: SelectionKind,
    /// Whether the selection's right edge tracks **end-of-line** (vim's `$`
    /// in Visual): set by a `$` motion while selecting, cleared by any other
    /// within-line horizontal motion, preserved across vertical ones. Only a
    /// *block* selection uses it — the right edge becomes each row's own
    /// length (`Ctrl-V`…`$`…`A`/`d` act raggedly), not a fixed column.
    selection_to_eol: bool,
    /// What `]]` and `[[` move between — vim's `{` lines unless the owner
    /// says otherwise ([`set_sections`](Self::set_sections)).
    sections: Sections,
    /// **Where each mark is, as this copy's lines** — `'a`'s and `` `a ``'s
    /// targets, set beside the lines by an owner that keeps marks
    /// ([`set_marks`](Self::set_marks): a transcript's, resolved through its
    /// blocks). Empty: none here.
    marks: Vec<MarkAt>,
    /// **The folds shut here**, as this copy's lines — each `[start, end)`,
    /// ascending and apart, set beside the lines by an owner that folds
    /// ([`set_folds`](Self::set_folds): a transcript's window). A shut fold
    /// is one line to `j` `k` and a word motion, and whole to an operator
    /// (vim's `fold-behavior`, measured in Neovim 0.12). Empty: none.
    folds: Vec<(usize, usize)>,
    /// **The column a vertical motion aims for** — vim's `curswant` (`:h
    /// curswant`): `j` `k` through a shorter line land on its end and come
    /// back to the column on a longer one. A SCREEN column, in cells
    /// (`wrap::char_cells`, what the pane draws), or a line's end after `$`.
    /// `None`: where the cursor is. Nearly every action resets it, an edit
    /// that leaves the cursor in place too; a vertical motion, a motion that
    /// failed and a Visual anchor keep it ([`settle_want`](Self::settle_want);
    /// measured in Neovim 0.12, review stop 12, Q5: "Match vim").
    ///
    /// Tagged with the cursor it was set at, so a move that reaches no reset
    /// (a click, a search, a re-sync's clamp) aims from the cursor's own
    /// column, never somewhere stale.
    want: Aim,
}

/// A remembered column and the cursor it was set at ([`Buffer::want`]).
type Aim = Option<(Want, (usize, usize))>;

/// **A mark, and where it is in a buffer's lines**: whose, its name, and
/// `(line, column)` ([`Buffer::set_marks`]).
pub type MarkAt = ((crate::keymap::Whose, char), (usize, usize));

/// Where a vertical motion aims ([`Buffer::want`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// A screen column, in cells.
    Cells(usize),
    /// A line's end, whatever its length — `$` (vim's `MAXCOL`).
    End,
}

/// **What a section is in a buffer** — what `]]` `[[` move between, and
/// `][` `[]` to the ends of (`:h section`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Sections {
    /// Vim's own: one begins at a line that begins with `{` or a form feed,
    /// and ends at one that begins with `}` or a form feed. Its nroff macros
    /// (`.SH`, `.NH`, … — `'sections'`) are not read.
    #[default]
    Braces,
    /// Sections begin and end at these lines, each list ascending: a
    /// transcript's turns (`Output::sections`), set with the lines they index.
    /// A line past the buffer's end is never reached, so a stale list cannot
    /// panic.
    At {
        starts: Vec<usize>,
        ends: Vec<usize>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Prompt {
    /// The navigation + selection core (lines, cursor, Visual state).
    buf: Buffer,
    /// Undo / redo history of buffer snapshots. A snapshot is pushed
    /// *before* a mutating action; `undo` restores the most recent.
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Whether the previous action continues an insert "run" (a char
    /// insert, or opening a line) so consecutive typing coalesces into a
    /// single undo step rather than one-per-keystroke.
    coalescing: bool,
    /// The unnamed register: the text of the last yank *or* delete/change
    /// (`y`/`d`/`c`/`x`) — delete cuts, vim-style, so `ddp`/`xp` work — that
    /// `p`/`P` paste back. Empty until the first such op. Shared with every
    /// draft made [`sharing`](Self::sharing) it ([`Registers`]); mirrored to
    /// the system clipboard (OSC 52) by the app. Survives `take()`
    /// deliberately, so a yank in one prompt can be pasted into the next,
    /// matching vim's register persistence.
    register: Registers,
    /// An in-flight block-insert (vim `Ctrl-V`…`I`/`A`): the rectangle's row
    /// span + insert column, set when Insert is entered from block Visual.
    /// On leaving Insert, the text typed on the first row is replicated to
    /// every row of the block. `None` outside a block insert.
    pending_block_insert: Option<BlockInsert>,
    /// An in-flight counted insert (vim `3i`/`3a`/`3o`…): the repeat count
    /// and where typing began. On leaving Insert, the typed text is replayed
    /// `count - 1` more times. `None` for an uncounted insert.
    pending_insert_repeat: Option<InsertRepeat>,
    /// An in-flight Replace session (vim `R`). `None` outside one.
    replacing: Option<Replacing>,
    /// **The window's first visual row**, kept across frames — the prompt
    /// pane's own scroll, so the cursor PUSHES the window at `scrolloff` from
    /// an edge rather than dragging it (Steven, 10-15: "the cursor is just
    /// sticky to the bottom line"). Moved by [`settle`](Self::settle) once a
    /// frame and by [`place`](Self::place) (`zt` `zz` `zb`). It was re-derived
    /// from the cursor every frame since M1, so nothing could hold it.
    top: usize,
    /// `(width, height)` the window was last settled at — a new geometry
    /// refills the last page, as vim's does (`viewport::top_ceiling`).
    settled: Option<(usize, usize)>,
    /// **The aim a motion just reset, and where the motion started**, until
    /// the Normal clamp says whether it moved at all
    /// ([`clamp_cursor_to_char`](Self::clamp_cursor_to_char)): the clamp is
    /// the app's, after the motion, so a motion it undid looked like one
    /// that moved. `None` after anything but a motion.
    unsettled: Option<(Aim, (usize, usize))>,
}

/// A Replace session (vim `R`): what each typed char covered, so `<BS>` can
/// put it back, and what was typed, for a counted `R`'s replay on `<Esc>`.
/// The two grow and shrink together, one entry per typed char.
#[derive(Debug, Clone, Default)]
struct Replacing {
    /// Total repeats (`2R` → 2; 1 when uncounted).
    count: usize,
    /// Per typed char: `Some(c)` it overwrote `c`; `None` it was appended at
    /// the line's end or was a line break — so `<BS>` removes it.
    covered: Vec<Option<char>>,
    /// The chars typed, in order (`'\n'` for a break).
    typed: Vec<char>,
    /// Where the session's last keystroke left things — the cursor, and the
    /// line it was on. `None` before the first. What makes the stacks above
    /// mean anything: see [`Prompt::replace_session`].
    tip: Option<(usize, usize, Vec<char>)>,
}

impl Replacing {
    /// A session with nothing behind it: `<BS>` only steps left, `<Esc>`
    /// replays nothing.
    fn fresh() -> Self {
        Replacing {
            count: 1,
            ..Replacing::default()
        }
    }
}

/// A pending counted insert (vim `3i`/`3a`/`3o`…). On `<Esc>` the text typed
/// since `begin` is replayed `count - 1` more times — inline for
/// `i`/`a`/`A`/`I`, on a fresh line each for `o`/`O`.
#[derive(Debug, Clone, Copy)]
struct InsertRepeat {
    /// Total repeats (≥ 2; an uncounted insert arms nothing).
    count: usize,
    /// Where the typed text starts (the cursor right after the entry's
    /// reposition / line-open).
    begin: (usize, usize),
    /// The entry kind — only its line-vs-inline replay distinction matters.
    entry: InsertEntry,
}

/// A pending block-insert (vim `Ctrl-V`…`I`/`A`). Records where to replicate
/// the typed text when Insert is left.
#[derive(Debug, Clone, Copy)]
struct BlockInsert {
    /// First and last rows of the block (inclusive). The user types on
    /// `first`; `first+1..=last` receive the replica.
    first: usize,
    last: usize,
    /// The block's nominal insert column (left edge for `I`, right edge for
    /// `A`). Each row inserts at `min(col, row_len)` — a row shorter than the
    /// column takes the text at *its own end* (vim's ragged `$`-block
    /// append), never space-padded to vertically align.
    col: usize,
    /// The column the cursor started typing at on `first` (`min(col,
    /// first_len)`) — needed to slice out the typed text once the row grows.
    start_col: usize,
    /// How many lines the buffer had when it began: a join or a split while
    /// typing (`<Del>` at the row's end) leaves rows that are not the
    /// block's, and the replica is abandoned (review stop 12, T6 1).
    lines: usize,
}

/// A yanked span plus its geometry ([`SelectionKind`]). Paste honors it
/// exactly as vim does: a *linewise* register opens a fresh line (`p` below,
/// `P` above); a *charwise* one inserts inline (`p` after the cursor, `P` at
/// it); a *block* one inserts a rectangle (one segment per successive row).
#[derive(Debug, Clone)]
struct Register {
    /// The text. A linewise register ends each line (including the last)
    /// with `'\n'`; charwise/block multi-row registers join rows with `'\n'`
    /// and have none trailing. Always `from.text()`, kept rendered because a
    /// paste reads it.
    text: String,
    kind: SelectionKind,
    /// What it was taken from — for the clipboard, which judges a secret by
    /// position (`App::copy_to_clipboard`).
    from: Copied,
}

/// **The unnamed register, one for every draft that shares it** — vim's is
/// the editor's, not a buffer's: a yank in one prompt window pastes in
/// another, and a yank in the chat in whichever prompt you are in. Each
/// prompt window owns its draft (Steven, 10-26: "each prompt pane has its own
/// draft"), so the register cannot be a draft's field: every window's draft
/// holds a handle on the one cell (`App::registers`, given at the window's
/// birth through [`Prompt::sharing`]). A `Prompt` made alone has its own.
/// One thread reads it, a borrow at a time (`paste`, `set_register`).
#[derive(Debug, Clone, Default)]
pub struct Registers(Rc<RefCell<Option<Register>>>);

/// The resolved geometry an operator acts over — computed once (pure,
/// reusing the motion probe), then either `read` (yank, without mutating)
/// or `drain` (delete/change). Splitting resolution from mutation is what
/// lets yank copy *exactly* the span delete would remove, with no parallel
/// resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeShape {
    /// Charwise: `start` (inclusive) to `end` (exclusive), in `(row, col)`.
    /// Single-row when `start.0 == end.0` (a plain `[lo, hi)` drain);
    /// otherwise the head of `start.0` from `start.1` and the tail of
    /// `end.0` before `end.1` join, dropping the rows between.
    Chars {
        start: (usize, usize),
        end: (usize, usize),
    },
    /// Linewise: the whole rows `[first, last]` inclusive.
    Lines { first: usize, last: usize },
    /// Block (rectangular): char columns `[clo, chi)` on every row in
    /// `[rlo, rhi]` inclusive. Rows shorter than `chi` contribute (and
    /// receive) less — the rectangle is clamped per row.
    Block {
        rows: (usize, usize),
        cols: (usize, usize),
    },
}

impl RangeShape {
    /// The `[lo, hi)` char span this shape covers on logical row `li`,
    /// clamped to `row_len` — or `None` if the row lies outside it. The one
    /// place selection geometry projects to a per-row span: the selection
    /// highlight, every `read` (through `copied`) and the block `drain` all go
    /// through it, so what the user sees highlighted is the *same* computation
    /// an operator takes (the see-vs-cut invariant the resolve/read/drain split
    /// exists to keep).
    ///
    /// Charwise spans the first row from its start column and the last row up
    /// to its (exclusive) end, with full rows between; linewise is the whole
    /// row; block is the column rectangle clamped to each row's own length.
    fn row_span(self, li: usize, row_len: usize) -> Option<(usize, usize)> {
        let (lo, hi) = match self {
            RangeShape::Chars {
                start: (sr, sc),
                end: (er, ec),
            } => {
                if li < sr || li > er {
                    return None;
                }
                let lo = if li == sr { sc } else { 0 };
                let hi = if li == er { ec.min(row_len) } else { row_len };
                (lo, hi)
            }
            RangeShape::Lines { first, last } => {
                if li < first || li > last {
                    return None;
                }
                (0, row_len)
            }
            RangeShape::Block {
                rows: (rlo, rhi),
                cols: (clo, chi),
            } => {
                if li < rlo || li > rhi {
                    return None;
                }
                (clo.min(row_len), chi.min(row_len))
            }
        };
        (lo < hi).then_some((lo, hi))
    }

    /// The logical rows the shape touches, first to last.
    fn rows(self) -> std::ops::RangeInclusive<usize> {
        match self {
            RangeShape::Chars {
                start: (sr, _),
                end: (er, _),
            } => sr..=er,
            RangeShape::Lines { first, last } => first..=last,
            RangeShape::Block { rows: (lo, hi), .. } => lo..=hi,
        }
    }

    /// The register geometry a shape yanks as — so a paste re-opens lines,
    /// inserts inline, or lays a rectangle.
    fn kind(self) -> SelectionKind {
        match self {
            RangeShape::Chars { .. } => SelectionKind::Char,
            RangeShape::Lines { .. } => SelectionKind::Line,
            RangeShape::Block { .. } => SelectionKind::Block,
        }
    }
}

/// A point-in-time buffer + cursor, for undo/redo. The prompt is small by
/// construction, so cloning the whole buffer per group is cheap.
#[derive(Debug, Clone)]
struct Snapshot {
    lines: Vec<Vec<char>>,
    cursor_row: usize,
    cursor_col: usize,
    /// What it holds, in bytes, counted once when taken — for
    /// [`UNDO_BYTES`].
    bytes: usize,
}

/// Cap on undo depth — bounds memory against a pathological paste-and-edit
/// loop. The oldest entry is dropped past this.
const UNDO_LIMIT: usize = 500;

/// **And on its size** — a snapshot is the whole text, and "the prompt is
/// small by construction" stopped being true when a page became editable:
/// the tutor with 10k lines pasted grew 210 MB in 140 edits, 1.4 GB at the
/// 500 the count allows (review stop 16, B 5, measured). The oldest go
/// first, the newest always kept.
const UNDO_BYTES: usize = 32 * 1024 * 1024;

impl Default for Buffer {
    fn default() -> Self {
        Self {
            lines: vec![Vec::new()],
            cursor_row: 0,
            cursor_col: 0,
            selection_anchor: None,
            selection_kind: SelectionKind::Char,
            selection_to_eol: false,
            sections: Sections::Braces,
            marks: Vec::new(),
            folds: Vec::new(),
            want: None,
        }
    }
}

impl Buffer {
    /// A read-only buffer over `lines` — the chat pane's navigable view of the
    /// transcript, cursor at the origin. Editing is gated by the caller (the
    /// chat keymap emits only motions / selection / yank).
    pub fn from_lines(lines: Vec<String>) -> Self {
        let mut b = Buffer::default();
        b.set_lines(lines);
        b
    }

    /// Re-sync the buffer's text from the (possibly grown) transcript, or a
    /// page's new content, keeping the cursor where it was, clamped into the
    /// new content — and a selection's other end, clamped the same way.
    pub fn set_lines(&mut self, lines: Vec<String>) {
        self.set_lines_from(0, lines);
    }

    /// **[`set_lines`](Self::set_lines) from line `keep` on** — the lines
    /// before it stay as they are, the rest become `tail` — for an owner that
    /// knows how much of its copy is still right (the transcript's mirror,
    /// `App::sync_chat`: a streaming reply changes only its own lines).
    pub fn set_lines_from(&mut self, keep: usize, tail: Vec<String>) {
        self.lines.truncate(keep);
        self.lines
            .extend(tail.into_iter().map(|l| l.chars().collect::<Vec<char>>()));
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);
        self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
        // Re-clamp the selection anchor too, into the new content. Downstream
        // (`row_span`/`read`) clamps defensively so a stale column can't panic,
        // but an unclamped anchor would briefly highlight a column past the
        // line's end. CLAMPED, not dropped, when its row is gone: the keymap
        // is still in Visual, and a mode over no range made the next `y` do
        // nothing (a page's content shrinks under a reader — the inspector —
        // and kept its range bounded; `:graph` re-anchors for the same
        // reason).
        if let Some((r, c)) = self.selection_anchor {
            let r = r.min(self.lines.len() - 1);
            self.selection_anchor = Some((r, c.min(self.lines[r].len())));
        }
    }

    /// Say what a section is here — set beside [`set_lines`](Self::set_lines)
    /// by an owner whose sections are not vim's `{` lines (the transcript's
    /// turns, `App::sync_chat`).
    pub fn set_sections(&mut self, sections: Sections) {
        self.sections = sections;
    }

    /// Say where each mark is, as this copy's lines ([`MarkAt`]) — set
    /// beside [`set_lines`](Self::set_lines) by an owner that keeps marks
    /// (the transcript's session, `App::sync_chat`).
    pub fn set_marks(&mut self, marks: Vec<MarkAt>) {
        self.marks = marks;
    }

    /// Say which folds are shut, as this copy's lines (`[start, end)`, in
    /// order) — set beside [`set_lines`](Self::set_lines) by an owner that
    /// folds (a transcript's window, `App::sync_chat`). The cursor stays on
    /// its line, as vim's does under `zc`: it is drawn on the fold's row.
    ///
    /// Put in order here, overlapping ones joined and empty ones dropped, so
    /// [`fold_at`](Self::fold_at)'s search is over a list it can search
    /// whatever an owner hands in.
    pub fn set_folds(&mut self, mut folds: Vec<(usize, usize)>) {
        folds.retain(|&(start, end)| start < end);
        folds.sort_unstable();
        self.folds = Vec::with_capacity(folds.len());
        for (start, end) in folds {
            match self.folds.last_mut() {
                // Overlapping, joined; two that only touch stay two, as vim's.
                Some(last) if start < last.1 => last.1 = last.1.max(end),
                _ => self.folds.push((start, end)),
            }
        }
    }

    /// **The shut fold holding `row`**, as `[start, end)` — `None` when it is
    /// in none. A fold past the lines is cut to them, so a stale list cannot
    /// send a motion off the end.
    pub fn fold_at(&self, row: usize) -> Option<(usize, usize)> {
        let at = self.folds.partition_point(|&(_, end)| end <= row);
        let &(start, end) = self.folds.get(at)?;
        (start <= row && row < self.lines.len()).then(|| (start, end.min(self.lines.len())))
    }

    /// `row`, or the first line of the shut fold holding it.
    fn fold_start(&self, row: usize) -> usize {
        self.fold_at(row).map_or(row, |(start, _)| start)
    }

    /// `row`, or the last line of the shut fold holding it.
    fn fold_end(&self, row: usize) -> usize {
        self.fold_at(row).map_or(row, |(_, end)| end - 1)
    }

    /// **Where a line's text begins** — its first non-blank char, or its last
    /// when it is all blank (0 when empty): vim's `beginline(BL_WHITE |
    /// BL_FIX)`, where `]]` `[[`, a linewise `p`, and with Vim's
    /// `'startofline'` `H` `M` `L` put the cursor (measured, nvim 0.12.6).
    /// One rule for all of them: the prompt's `H`, the transcript's and `p`
    /// each had a copy, and all three went to 0 on an all-blank line.
    pub fn first_non_blank(&self, row: usize) -> usize {
        let line = self.lines.get(row).map_or(&[][..], Vec::as_slice);
        line.iter()
            .position(|c| !c.is_whitespace())
            .unwrap_or(line.len().saturating_sub(1))
    }

    /// Apply a cursor motion `count` times (the chat pane's navigation),
    /// stopping early at a boundary fixed point so a pathological count is
    /// O(distance), not O(count).
    pub fn move_cursor(&mut self, motion: CursorMotion, count: usize) {
        for _ in 0..count.max(1) {
            let before = (self.cursor_row, self.cursor_col);
            self.apply_motion(motion);
            if (self.cursor_row, self.cursor_col) == before {
                break;
            }
        }
    }

    /// Place the cursor at a logical `(row, col)`, clamped into the buffer (row
    /// to the line count, col to that line's length). A pure cursor move — no
    /// edit, no undo, the selection anchor untouched (the caller manages it).
    /// The mouse positions the cursor through this.
    pub fn place_cursor(&mut self, row: usize, col: usize) {
        let row = row.min(self.lines.len().saturating_sub(1));
        self.cursor_row = row;
        self.cursor_col = col.min(self.lines[row].len());
        self.want = None;
    }

    /// All lines as `String`s, in order.
    pub fn lines(&self) -> impl Iterator<Item = String> + '_ {
        self.lines.iter().map(|l| l.iter().collect())
    }

    /// `(row, col)` of the cursor in logical buffer coordinates.
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The Visual selection anchor, if any.
    pub fn anchor(&self) -> Option<(usize, usize)> {
        self.selection_anchor
    }

    /// The selected char range `[lo, hi)` on logical line `line` (clamped to
    /// `len`), or `None` if the line is outside the selection. Goes through the
    /// same `row_span` projection the in-pane highlight uses, so an external
    /// renderer (the focused chat, drawn from `Output`) highlights exactly what
    /// an operator would cut — kind-agnostic across char/line/block.
    pub fn selected_char_range(&self, line: usize, len: usize) -> Option<(usize, usize)> {
        self.selection_shape().and_then(|s| s.row_span(line, len))
    }

    /// The lines a Visual selection spans, whole or in part; `None` without
    /// one. An empty line inside it has no chars for
    /// [`selected_char_range`](Self::selected_char_range) to give and is
    /// still in it — what a renderer asks to draw it selected.
    pub fn selected_lines(&self) -> Option<std::ops::RangeInclusive<usize>> {
        self.selection_shape().map(|s| s.rows())
    }

    /// Anchor a selection at the cursor with the given geometry (vim
    /// `v`/`V`/`Ctrl-V`). A pure selection-state change.
    pub fn set_anchor(&mut self, kind: SelectionKind) {
        self.selection_anchor = Some((self.cursor_row, self.cursor_col));
        self.selection_kind = kind;
        self.selection_to_eol = false; // a fresh selection starts at a column
    }

    /// Switch the active selection's geometry without re-anchoring (vim
    /// `v`↔`V`↔`Ctrl-V` mid-selection keeps both endpoints).
    pub fn set_selection_kind(&mut self, kind: SelectionKind) {
        self.selection_kind = kind;
    }

    /// Drop the Visual selection (leaving Visual — `Esc`).
    pub fn clear_anchor(&mut self) {
        self.selection_anchor = None;
    }

    /// **The Visual selection's anchor moved to `(row, col)`**, its kind
    /// kept — the lines above it changed, not the selection (a record closing
    /// in place, `App::put_anchored`). Clamped as a re-sync clamps it; a
    /// no-op without a selection.
    pub fn move_anchor(&mut self, row: usize, col: usize) {
        if self.selection_anchor.is_some() && row < self.lines.len() {
            self.selection_anchor = Some((row, col.min(self.lines[row].len())));
        }
    }

    /// Swap the cursor and the selection anchor (vim Visual `o`): the anchor
    /// becomes the cursor and vice-versa, so further motions extend the
    /// selection from the other end. A no-op without an active selection.
    pub fn swap_selection_ends(&mut self) {
        if let Some(anchor) = self.selection_anchor {
            self.selection_anchor = Some((self.cursor_row, self.cursor_col));
            self.cursor_row = anchor.0;
            self.cursor_col = anchor.1;
        }
    }

    /// Select a text object as a Visual selection (`viw`/`vi"`/`va(`/`vap`):
    /// charwise from the object's start to the cursor on its last char, or —
    /// a paragraph — linewise over its lines, the cursor on the last. A
    /// no-op if the object resolves to nothing (e.g. `vi(` with no
    /// surrounding pair) — the current selection is left as-is. Asks the
    /// resolver operator-pending `ciw` asks (`object_span`, `Buffer::resolve`).
    pub fn select_text_object(&mut self, scope: TextObjectScope, object: TextObject, count: usize) {
        let at = (self.cursor_row, self.cursor_col);
        let (anchor, kind, cursor) = match object_span(&self.lines, at, scope, object, count) {
            // The cursor on the span's last char: the one before its end, or
            // — an end at a line's start — the line before's end.
            Some(ObjectSpan::Chars { start, end }) if end > start => {
                let last = match end {
                    (row, 0) => (row - 1, self.lines[row - 1].len().saturating_sub(1)),
                    (row, col) => (row, col - 1),
                };
                (start, SelectionKind::Char, last)
            }
            Some(ObjectSpan::Lines { top, bottom }) => ((top, 0), SelectionKind::Line, (bottom, 0)),
            _ => return,
        };
        self.selection_anchor = Some(anchor);
        self.selection_kind = kind;
        self.selection_to_eol = false;
        (self.cursor_row, self.cursor_col) = cursor;
    }

    /// **One action on a READ-ONLY buffer** — the transcript's, and every
    /// page's (`TextView`): a motion, `v` `V` `<C-v>`, a switch between them,
    /// `Esc`, `o`, a text object. `false` for what it does not take: an edit,
    /// which a read-only surface never applies, and `H` `M` `L`, which need
    /// the window the caller keeps. The window follows by the caller, who
    /// knows it.
    ///
    /// The one door for both surfaces: the transcript took motions and the
    /// anchors through its own arm and dropped `o` and `viw` on the floor,
    /// while a page had a line cursor of its own and no columns at all.
    pub fn apply_read_only(&mut self, action: EditAction, count: usize) -> bool {
        let before = self.cursor();
        let taken = self.apply_read_only_once(action, count);
        if taken {
            self.settle_want(action, before);
        }
        taken
    }

    fn apply_read_only_once(&mut self, action: EditAction, count: usize) -> bool {
        match action {
            EditAction::Move(CursorMotion::Screen(_)) => return false,
            EditAction::Move(motion) => {
                self.move_cursor(motion, count);
                self.clamp_cursor_to_char();
            }
            EditAction::SetAnchor => self.set_anchor(SelectionKind::Char),
            EditAction::SetAnchorLine => self.set_anchor(SelectionKind::Line),
            EditAction::SetAnchorBlock => self.set_anchor(SelectionKind::Block),
            EditAction::SetSelectionKind(kind) => self.set_selection_kind(kind),
            EditAction::ClearAnchor => self.clear_anchor(),
            EditAction::SwapSelectionEnds => self.swap_selection_ends(),
            EditAction::SelectTextObject {
                scope,
                object,
                count,
            } => self.select_text_object(scope, object, count),
            _ => return false,
        }
        true
    }

    /// **`/` `?` `n` `N`** — the cursor to the start of the `count`th match
    /// of `re` past it, forward or back, wrapping past an end as vim's
    /// `'wrapscan'` does. `Some(wrapped)` where it landed — `wrapped` when it
    /// went past an end to get there; `None` when nothing matches anywhere,
    /// the cursor where it was. One search for every pane with text: the
    /// prompt, the transcript and the pages (`TextView`).
    pub fn search(&mut self, re: &regex::Regex, forward: bool, count: usize) -> Option<bool> {
        let mut wrapped = false;
        for _ in 0..count.max(1) {
            let (row, col, past) = self.next_match(re, forward)?;
            wrapped |= past;
            self.cursor_row = row;
            self.cursor_col = col;
            self.want = None;
        }
        Some(wrapped)
    }

    /// The next match's start from the cursor, `(row, col, past an end)`:
    /// the rest of the cursor's line, each line after (or before), then
    /// round past the end to the cursor's line again — every line once.
    fn next_match(&self, re: &regex::Regex, forward: bool) -> Option<(usize, usize, bool)> {
        let n = self.lines.len();
        let (row, col) = (self.cursor_row, self.cursor_col);
        for step in 0..=n {
            let (at, past) = match forward {
                true => ((row + step) % n, row + step >= n),
                false => ((row + n - step) % n, step > row),
            };
            let starts = match_starts(&self.lines[at], re);
            let hit = match (forward, step) {
                (true, 0) => starts.iter().copied().find(|&c| c > col),
                (true, s) if s == n => starts.iter().copied().find(|&c| c <= col),
                (true, _) => starts.first().copied(),
                (false, 0) => starts.iter().copied().rfind(|&c| c < col),
                (false, s) if s == n => starts.iter().copied().rfind(|&c| c >= col),
                (false, _) => starts.last().copied(),
            };
            if let Some(c) = hit {
                return Some((at, c, past));
            }
        }
        None
    }

    /// **The word `*` and `#` search for**, vim's: the keyword under or
    /// after the cursor on its line, else the run of non-blanks there —
    /// with whether it is a keyword; `None` when only blanks are left. The
    /// cursor goes to its start, as vim's `*` puts it, so the search that
    /// follows skips the word itself whichever way it goes.
    pub fn cursor_to_word(&mut self) -> Option<(String, bool)> {
        use crate::word::{Class, class};
        let line = &self.lines[self.cursor_row];
        let from = self.cursor_col.min(line.len());
        let of = |i: usize| class(line[i], false);
        // The run holding `i` whose chars `keep`, both ways.
        let run = |i: usize, keep: &dyn Fn(usize) -> bool| {
            let start = (0..i).rev().take_while(|&j| keep(j)).last().unwrap_or(i);
            let end = (i..line.len()).find(|&j| !keep(j)).unwrap_or(line.len());
            (start, line[start..end].iter().collect::<String>())
        };
        let keyword = |j: usize| of(j) == Class::Keyword;
        let found = match (from..line.len()).find(|&i| keyword(i)) {
            Some(i) => (run(i, &keyword), true),
            None => {
                let i = (from..line.len()).find(|&i| of(i) != Class::Blank)?;
                (run(i, &|j| of(j) != Class::Blank), false)
            }
        };
        let ((start, word), is_keyword) = found;
        self.cursor_col = start;
        self.want = None;
        Some((word, is_keyword))
    }

    /// Yank `target`'s text without mutating (the chat pane's read-only `y` —
    /// `yw`/`yy`/`yi"` via an operator target, or `vwy` via `OpTarget::Selection`).
    /// Returns what it took — a [`Copied`], whose `text()` is the yank — *and*
    /// its geometry kind (so a paste re-opens lines vs. inserts inline), parks
    /// the cursor at the range start (vim's yank rule), and drops a Visual
    /// selection. `None` if the range is empty.
    pub fn yank(&mut self, target: OpTarget, count: usize) -> Option<(Copied, SelectionKind)> {
        let shape = self.resolve(Operator::Yank, target, count.max(1))?;
        let copied = self.copied(shape);
        if matches!(target, OpTarget::Selection) {
            self.selection_anchor = None;
        }
        if copied.text().is_empty() {
            return None;
        }
        self.move_cursor_to_range_start(shape);
        self.want = None;
        Some((copied, shape.kind()))
    }

    /// What the Visual selection covers, read without touching the buffer —
    /// what a mouse release copies (vim's `clipboard=autoselect`): the
    /// selection stays up for `y`, `d` or `:`. `None` with no selection or an
    /// empty one.
    pub fn selected(&self) -> Option<Copied> {
        let copied = self.copied(self.selection_shape()?);
        (!copied.text().is_empty()).then_some(copied)
    }

    /// The selection as an ordered `(start, end)` pair of `(row, col)`
    /// points, both inclusive — vim's Visual covers the char under the
    /// cursor. `None` when no anchor is set.
    fn selection_bounds(&self) -> Option<((usize, usize), (usize, usize))> {
        let a = self.selection_anchor?;
        let c = (self.cursor_row, self.cursor_col);
        Some(if a <= c { (a, c) } else { (c, a) })
    }

    /// Move the cursor by one motion. **The** single place a motion maps to
    /// cursor arithmetic — driven both by `dispatch` (a plain `3j`) and by
    /// `motion_target` (an operator's range), so the two can never diverge.
    fn apply_motion(&mut self, motion: CursorMotion) {
        match motion {
            CursorMotion::Left => self.move_left(),
            CursorMotion::Right => self.move_right(),
            CursorMotion::Up => self.move_up(),
            CursorMotion::Down => self.move_down(),
            CursorMotion::LineStart => self.move_home(),
            CursorMotion::LineEnd => self.move_end(),
            CursorMotion::WordForward => self.move_word_forward(false),
            CursorMotion::WordBackward => self.move_word_backward(false),
            CursorMotion::WordEnd => self.move_word_end(false),
            CursorMotion::BigWordForward => self.move_word_forward(true),
            CursorMotion::BigWordBackward => self.move_word_backward(true),
            CursorMotion::BigWordEnd => self.move_word_end(true),
            CursorMotion::BufferStart => self.move_buffer_start(),
            CursorMotion::BufferEnd => self.move_buffer_end(),
            CursorMotion::GotoLine(line) => self.move_to_line(line),
            // `H` `M` `L` are relative to the WINDOW, which this buffer does
            // not know. Not reached: the app moves them through the window
            // that shows the text (`Prompt::screen_line`,
            // `App::chat_screen_line`, a page's `TextView`) before any
            // buffer's door, and refuses an operator over them (`yH`) out
            // loud (`apply_visible_refusal`), being handed no window there.
            CursorMotion::Screen(_) => {}
            // A failed count does not move at all, as vim's beeps.
            CursorMotion::Section {
                forward,
                end,
                count,
            } => {
                if let Some(((row, _), _)) = self.find_section(forward, end, count, false) {
                    self.cursor_row = row;
                    self.cursor_col = self.first_non_blank(row);
                }
            }
            // The exact place, as vim's `` ` `` (Steven, 10-26). A mark not
            // here does not move, as vim's E20; the app says so first. Clamped:
            // a table set beside other lines cannot point past these.
            CursorMotion::Mark(whose, name) => {
                if let Some(&(_, (row, col))) = self.marks.iter().find(|(m, _)| *m == (whose, name))
                {
                    self.cursor_row = row.min(self.lines.len() - 1);
                    self.cursor_col = col.min(self.lines[self.cursor_row].len());
                }
            }
            // Where each lands is said once, by `landing`, which an
            // operator's range asks too. One that fails does not move, as
            // vim's beeps.
            CursorMotion::FindChar(_)
            | CursorMotion::RepeatFind { .. }
            | CursorMotion::FirstNonBlank
            | CursorMotion::Paragraph { .. }
            | CursorMotion::WordEndBackward { .. }
            | CursorMotion::MatchPair
            | CursorMotion::Percent(_) => {
                if let Some(((row, col), _)) = self.landing(motion, 1) {
                    self.cursor_row = row;
                    self.cursor_col = col;
                }
            }
        }
        // Track vim's `$`-to-end-of-line stickiness for a live selection.
        if self.selection_anchor.is_some() {
            self.selection_to_eol = match motion {
                CursorMotion::LineEnd => true,
                CursorMotion::Up
                | CursorMotion::Down
                | CursorMotion::BufferStart
                | CursorMotion::BufferEnd
                | CursorMotion::GotoLine(_) => self.selection_to_eol,
                _ => false,
            };
        }
    }

    /// **Where `motion` lands from the cursor, `count` of it, and whether
    /// an operator takes the char there** — `None` when it fails (vim
    /// beeps): a cursor stays, an operator is cancelled. The finds, `^`,
    /// paragraphs, `ge` and `%` are said here, once, for a cursor
    /// ([`apply_motion`](Self::apply_motion), `count` 1) and an operator
    /// ([`charwise_shape`](Self::charwise_shape)) alike; a count they carry
    /// is multiplied by an operator's (`2d3fx` is `d6fx`). Every other
    /// motion by its steps ([`motion_target`](Self::motion_target)).
    fn landing(&mut self, motion: CursorMotion, count: usize) -> Option<(word::Pos, bool)> {
        let at = self.cursor();
        let count = count.max(1);
        match motion {
            CursorMotion::FindChar(find) => {
                let find = Find {
                    count: find.count.saturating_mul(count),
                    ..find
                };
                let col = find_in_line(&self.lines[at.0], at.1, find)?;
                // `f` `t` take their char, `F` `T` stop short (`:h f`).
                Some(((at.0, col), find.forward))
            }
            // The app's to make the find it repeats (`App::concrete`).
            CursorMotion::RepeatFind { .. } => None,
            CursorMotion::FirstNonBlank => Some(((at.0, self.first_non_blank(at.0)), false)),
            CursorMotion::Paragraph { forward, count: n } => {
                self.find_paragraph(forward, n.saturating_mul(count))
            }
            CursorMotion::WordEndBackward { big } => {
                match word::prev_end(&self.lines, at, count, big) {
                    (to, true) => Some((to, true)),
                    (_, false) => None,
                }
            }
            CursorMotion::MatchPair => self.match_pair().map(|to| (to, true)),
            // `:h N%`: `(N * lines + 99) / 100`, landed as `{n}G` lands —
            // its column kept, vim's `nostartofline`. Past 100 it fails, as
            // vim's beeps: `d200%` cut the whole buffer (review stop 16, A L1
            // and L2, C 5, measured).
            CursorMotion::Percent(n) if n > 100 => None,
            CursorMotion::Percent(n) => {
                let line = (n * self.lines.len()).div_ceil(100).max(1);
                let to = self.motion_target(CursorMotion::GotoLine(line), 1);
                Some((to, false))
            }
            _ => Some((self.motion_target(motion, count), motion_inclusive(motion))),
        }
    }

    /// **Where `count` paragraphs forward or back land** — vim's `findpar`
    /// (`:h }`): each walks past a non-empty line to the next empty one;
    /// only the LAST may stop at the buffer's edge instead, so a count that
    /// runs out first fails (`None`). Forward onto the last line, its last
    /// char, inclusive (`d}` there takes it); elsewhere column 0, exclusive.
    fn find_paragraph(&self, forward: bool, count: usize) -> Option<(word::Pos, bool)> {
        let last = self.lines.len() - 1;
        let mut row = self.cursor_row;
        for left in (0..count.max(1)).rev() {
            let (mut passed, mut first) = (false, true);
            loop {
                passed |= !self.lines[row].is_empty();
                // A shut fold each count starts in is stepped past whole, as
                // vim's `findpar` skips one: from inside a shut thought, `}`
                // stopped on a blank line it hid, and opened it (review stop
                // 16, A M4).
                if first && let Some((start, past)) = self.fold_at(row) {
                    row = if forward { past - 1 } else { start };
                }
                if !first && passed && self.lines[row].is_empty() {
                    break;
                }
                first = false;
                let next = match forward {
                    true => (row < last).then(|| row + 1),
                    false => row.checked_sub(1),
                };
                match next {
                    Some(next) => row = next,
                    None if left > 0 => return None,
                    None => break,
                }
            }
        }
        let len = self.lines[row].len();
        Some(match forward && row == last && len > 0 {
            true => ((row, len - 1), true),
            false => ((row, 0), false),
        })
    }

    /// **The bracket matching the first one at or after the cursor on its
    /// line** — vim's `%` over `(` `)` `[` `]` `{` `}` (`:h %`; not its C
    /// comments or `#if`s): forward from an opener, back from a closer,
    /// nesting counted, across lines. `None` when the line holds none from
    /// the cursor on, or it has no match.
    ///
    /// **Quotes, as vim's default reads them** (`'cpoptions'` without `%`,
    /// measured): from outside quotes, a bracket inside a double-quoted span
    /// is passed over; from inside one, only that span is searched. It
    /// counted all of them: `f("(", x)` matched nothing (review stop 16,
    /// A M5, C 5).
    ///
    /// **And escapes, as vim pairs them** (measured): an escaped bracket
    /// pairs only with an escaped one — `\(` with `\)`, `(` with `)` — and a
    /// character literal's (`'('`) is passed over. Either may be the one
    /// `%` starts from: `x \( (b)` from its start stays, as vim's does.
    fn match_pair(&self) -> Option<word::Pos> {
        let (row, col) = self.cursor();
        let line = &self.lines[row];
        let (start, (this, that, forward)) =
            (col..line.len()).find_map(|c| pair_of(line[c]).map(|pair| (c, pair)))?;
        let escapes = escaped(line, start);
        let inside = quoted(line)[start];
        let mut spans = (row, quoted(line));
        // The start is the first of the nesting, whatever it is written as.
        let mut depth = 1usize;
        let mut at = (row, start);
        loop {
            at = self.step_over(at, forward)?;
            if spans.0 != at.0 {
                spans = (at.0, quoted(&self.lines[at.0]));
            }
            let here = spans.1.get(at.1).copied().flatten();
            if inside.is_some() && (at.0 != row || here != inside) {
                return None;
            }
            let line = &self.lines[at.0];
            let Some(&c) = line.get(at.1) else { continue };
            if here != inside || literal(line, at.1) || escaped(line, at.1) != escapes {
                continue;
            }
            if c == this {
                depth += 1;
            } else if c == that {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
        }
    }

    /// The next char's place (the last one's, back), across lines — an
    /// empty line's `(row, 0)` holds none; `None` past the buffer's edge.
    fn step_over(&self, (row, col): word::Pos, forward: bool) -> Option<word::Pos> {
        match forward {
            true if col + 1 < self.lines[row].len() => Some((row, col + 1)),
            true => (row + 1 < self.lines.len()).then(|| (row + 1, 0)),
            false if col > 0 => Some((row, col - 1)),
            false => {
                let up = row.checked_sub(1)?;
                Some((up, self.lines[up].len().saturating_sub(1)))
            }
        }
    }

    /// The `(row, col)` a motion lands on after `count` steps from the
    /// cursor, *without* moving it. Reuses the real motion methods (a
    /// save/apply/restore round-trip) so operator ranges and cursor
    /// motions can never diverge.
    fn motion_target(&mut self, motion: CursorMotion, count: usize) -> (usize, usize) {
        let saved = (self.cursor_row, self.cursor_col);
        for _ in 0..count.max(1) {
            let before = (self.cursor_row, self.cursor_col);
            self.apply_motion(motion);
            if (self.cursor_row, self.cursor_col) == before {
                break;
            }
        }
        let target = (self.cursor_row, self.cursor_col);
        self.cursor_row = saved.0;
        self.cursor_col = saved.1;
        target
    }

    /// **Where `count` sections forward or back land** — vim's `findpar` for
    /// `]]` `[[`, and with `end` for `][` `[]` (nvim 0.12.6 measured against
    /// it): each step walks to the next section's first line (its end line);
    /// only the LAST step may stop at the buffer's edge instead, so `2]]` in
    /// the last section and `9[[` from the first fail (`None`, nothing moves).
    /// The column is 0 — but `]]` onto the last line is that line's last
    /// char, and inclusive, so `y]]` in the last section takes all of it (not
    /// `][`, whose `y][` stops short of it, as vim's does).
    ///
    /// `after_op` is vim's Vi quirk for `{` sections, kept as vim keeps it:
    /// an operator's `]]` also stops at a `}` line and takes it (`d]]` inside
    /// a C function deletes to its end). Turns have no `}`.
    fn find_section(
        &self,
        forward: bool,
        end: bool,
        count: usize,
        after_op: bool,
    ) -> Option<((usize, usize), bool)> {
        let last = self.lines.len() - 1;
        let quirk = after_op && forward && !end && self.sections == Sections::Braces;
        let stops = |row: usize| match &self.sections {
            Sections::Braces => {
                let first = self.lines[row].first().copied();
                let mark = if end { '}' } else { '{' };
                first == Some(mark) || first == Some('\x0c') || (quirk && first == Some('}'))
            }
            Sections::At { starts, ends } => match end {
                true => ends.binary_search(&row).is_ok(),
                false => starts.binary_search(&row).is_ok(),
            },
        };
        let mut row = self.cursor_row;
        // `left` is the steps still to take after this one.
        for left in (0..count.max(1)).rev() {
            loop {
                let next = if forward {
                    (row < last).then_some(row + 1)
                } else {
                    row.checked_sub(1)
                };
                let Some(next) = next else {
                    if left > 0 {
                        return None;
                    }
                    break;
                };
                row = next;
                if stops(row) {
                    break;
                }
            }
        }
        // The quirk takes the `}` line itself: the stop is the line after it,
        // which past the end is the last line, not inclusive (vim tests the
        // last line before it clamps).
        let below = row + usize::from(quirk && self.lines[row].first() == Some(&'}'));
        let inclusive = forward && !end && below == last;
        let row = below.min(last);
        let len = self.lines[row].len();
        Some(match inclusive && len > 0 {
            true => ((row, len - 1), true),
            false => ((row, 0), false),
        })
    }

    /// **The span an operator over `]]` / `[[` covers** — exclusive, shaped
    /// by vim's rules for that (`:h exclusive`, `:h exclusive-linewise`): an
    /// end at a line's start moves to the end of the line before, and the
    /// span is linewise when it starts in its line's indent. So `y]]` from a
    /// turn's first line takes that turn and not the next one's first line;
    /// from mid-line, the chars up to the end of the line before.
    fn section_shape(&self, forward: bool, end: bool, count: usize) -> Option<RangeShape> {
        let ((row, col), inclusive) = self.find_section(forward, end, count, true)?;
        let here = (self.cursor_row, self.cursor_col);
        let (start, mut stop) = match forward {
            true => (here, (row, col + usize::from(inclusive))),
            false => ((row, col), here),
        };
        if start >= stop {
            return None;
        }
        if !inclusive && stop.1 == 0 && stop.0 > start.0 {
            let before = stop.0 - 1;
            let indent = self.lines[start.0]
                .iter()
                .take_while(|c| c.is_whitespace())
                .count();
            if start.1 <= indent {
                return Some(RangeShape::Lines {
                    first: start.0,
                    last: before,
                });
            }
            stop = (before, self.lines[before].len());
        }
        Some(RangeShape::Chars { start, end: stop })
    }

    fn move_left(&mut self) {
        self.cursor_col = self.cursor_col.saturating_sub(1);
    }

    fn move_right(&mut self) {
        if self.cursor_col < self.lines[self.cursor_row].len() {
            self.cursor_col += 1;
        }
    }

    /// Clamp the cursor onto a character — the Normal/Visual invariant that
    /// the caret sits *on* a char, never past the last one (vim). The aim
    /// survives it (`want`): `j` onto a shorter line in Normal lands on its
    /// last char, and comes back to the column.
    pub fn clamp_cursor_to_char(&mut self) {
        let before = self.cursor();
        let last_char = self.lines[self.cursor_row].len().saturating_sub(1);
        self.cursor_col = self.cursor_col.min(last_char);
        let now = self.cursor();
        if let Some((_, at)) = &mut self.want
            && *at == before
        {
            *at = now;
        }
    }

    /// Where a vertical motion from the cursor aims: the remembered
    /// [`want`](Self::want) while the cursor is where it was set, else the
    /// cursor's own screen column.
    fn aim(&self) -> Want {
        match self.want {
            Some((want, at)) if at == self.cursor() => want,
            _ => Want::Cells(cells_width(&self.lines[self.cursor_row][..self.cursor_col])),
        }
    }

    /// The column on `row` that aiming at `want` lands on: the char whose
    /// cells hold that screen column — a wide char from either of its two —
    /// else the line's end. A zero-width char holds none, so the cursor never
    /// lands on a combining mark.
    fn col_aimed(&self, row: usize, want: Want) -> usize {
        let line = &self.lines[row];
        let Want::Cells(want) = want else {
            return line.len();
        };
        let mut at = 0;
        for (i, &ch) in line.iter().enumerate() {
            let cells = crate::wrap::char_cells(ch);
            if cells > 0 && want < at + cells {
                return i;
            }
            at += cells;
        }
        line.len()
    }

    /// **Onto `row` by a vertical motion**, at the column it aims for, kept
    /// for the next one. The one door `j` `k` `gg` `G` `{n}G` take (Shannon's
    /// `G` and `gg` keep the column, as vim's do under `nostartofline`).
    fn move_vertically(&mut self, row: usize) {
        let want = self.aim();
        self.cursor_row = row;
        self.cursor_col = self.col_aimed(row, want);
        self.want = Some((want, self.cursor()));
    }

    /// **What an action leaves of the aim** ([`want`](Self::want)), from the
    /// cursor it started at: a vertical motion and `$` set it themselves, a
    /// motion that failed and a Visual anchor (`v` `V` `<C-v>`) keep it, and
    /// every other action resets it to where the cursor is — an edit that
    /// leaves the cursor in place too (`x`, `r`, `u`, `yy`: each measured).
    /// Both doors call it: [`apply_read_only`](Self::apply_read_only) and
    /// `Prompt::apply_counted`.
    fn settle_want(&mut self, action: EditAction, before: (usize, usize)) {
        use CursorMotion::{BufferEnd, BufferStart, Down, GotoLine, LineEnd, Up};
        let keeps = match action {
            EditAction::Move(Up | Down | BufferStart | BufferEnd | GotoLine(_) | LineEnd) => true,
            EditAction::Move(_) => self.cursor() == before,
            EditAction::SetAnchor
            | EditAction::SetAnchorLine
            | EditAction::SetAnchorBlock
            | EditAction::SetSelectionKind(_) => true,
            _ => false,
        };
        if !keeps {
            self.want = None;
        }
    }

    /// `k` — a shut fold is one line: from inside one, to the line above it;
    /// onto one from below, to its first line, as from above.
    fn move_up(&mut self) {
        if let Some(above) = self.fold_start(self.cursor_row).checked_sub(1) {
            self.move_vertically(self.fold_start(above));
        }
    }

    /// `j` — from inside a shut fold, to the line below it.
    fn move_down(&mut self) {
        let below = self.fold_end(self.cursor_row) + 1;
        if below < self.lines.len() {
            self.move_vertically(below);
        }
    }

    /// **Where a word motion starts from** — the cursor, or, in a shut fold,
    /// its edge the motion leaves by: the last char of its last line going
    /// forward, its first line's start going back. Measured: `w` and `e` on
    /// a fold over lines 2–4 land on line 5, `b` on line 1, the fold shut.
    fn word_from(&self, forward: bool) -> word::Pos {
        match (self.fold_at(self.cursor_row), forward) {
            (None, _) => self.cursor(),
            (Some((start, _)), false) => (start, 0),
            (Some((_, end)), true) => (end - 1, self.lines[end - 1].len().saturating_sub(1)),
        }
    }

    fn move_home(&mut self) {
        self.cursor_col = 0;
    }

    /// `$`: to the line's end, and aiming at every line's end after it.
    fn move_end(&mut self) {
        self.cursor_col = self.lines[self.cursor_row].len();
        self.want = Some((Want::End, self.cursor()));
    }

    /// Vim `w` (`W`, `big`): to the start of the next word, across lines —
    /// an indent is no word, an empty line is one ([`word::next_start`]).
    fn move_word_forward(&mut self, big: bool) {
        let to = word::next_start(&self.lines, self.word_from(true), 1, big, false).0;
        self.land(to);
    }

    /// Vim `b` (`B`, `big`): to the start of the word before, across lines
    /// ([`word::prev_start`]).
    fn move_word_backward(&mut self, big: bool) {
        (self.cursor_row, self.cursor_col) =
            word::prev_start(&self.lines, self.word_from(false), 1, big).0;
    }

    /// Vim `e` (`E`, `big`): to the end of this word or the next, across lines
    /// and past empty ones ([`word::next_end`]). With no word ahead it lands on
    /// the furthest cell it reached — a trailing space, or a trailing empty
    /// line — and on the buffer's last char it stays put.
    fn move_word_end(&mut self, big: bool) {
        let to = word::next_end(&self.lines, self.word_from(true), 1, big, false).0;
        self.land(to);
    }

    /// Onto `to`, a forward word motion's landing — moved back off a line's
    /// end it moved onto, as vim's `adjust_cursor` leaves a `w` or `e` that
    /// ran out of buffer.
    fn land(&mut self, (row, col): word::Pos) {
        let moved = (row, col) > self.cursor();
        let past_end = col > 0 && col >= self.lines[row].len();
        self.cursor_row = row;
        self.cursor_col = if moved && past_end { col - 1 } else { col };
    }

    /// Vim `G`: to the last line of the buffer, at the column aimed for.
    fn move_buffer_end(&mut self) {
        self.move_vertically(self.lines.len().saturating_sub(1));
    }

    /// Vim `gg`: to the first line of the buffer, at the column aimed for.
    fn move_buffer_start(&mut self) {
        self.move_vertically(0);
    }

    /// Vim `{count}G` / `{count}gg`: to a 1-indexed `line`, clamped, at the
    /// column aimed for.
    fn move_to_line(&mut self, line: usize) {
        self.move_vertically(
            line.saturating_sub(1)
                .min(self.lines.len().saturating_sub(1)),
        );
    }
}

impl Buffer {
    /// **The buffer wrapped to `width`** — every visual row as the span of
    /// its logical line it shows, and the cursor's `(row, cell)` among them,
    /// with the phantom continuation row the cursor needs at a full row's
    /// end.
    ///
    /// The one map behind the render ([`visible_window`](Self::visible_window)),
    /// the click ([`cursor_from_visual`](Self::cursor_from_visual)) and the
    /// prompt's window (`Prompt::settle`, `zt`, `H` `M` `L`). The render and
    /// the click each built their own, kept in step by a comment — and each
    /// re-derived the window top from the cursor, the rule that pinned the
    /// prompt's cursor to its bottom row.
    fn layout(&self, width: usize) -> (Vec<RowSpan>, (usize, usize)) {
        let mut spans = Vec::new();
        let mut cursor = (0, 0);
        for (li, line) in self.lines.iter().enumerate() {
            let wrapped = wrap_cells(line, width);
            let mut starts = Vec::with_capacity(wrapped.len());
            let mut acc = 0usize;
            for row in &wrapped {
                starts.push(acc);
                acc += row.char_len;
            }
            if li == self.cursor_row {
                let (dr, cell) = cursor_in_wrapped(line, &starts, width, self.cursor_col);
                cursor = (spans.len() + dr, cell);
            }
            for (row, start) in wrapped.iter().zip(starts) {
                spans.push(RowSpan {
                    line: li,
                    start,
                    len: row.char_len,
                });
            }
        }
        while cursor.0 >= spans.len() {
            // The phantom continuation row: empty, at the cursor line's end.
            let end = self.lines.get(self.cursor_row).map_or(0, Vec::len);
            spans.push(RowSpan {
                line: self.cursor_row,
                start: end,
                len: 0,
            });
        }
        (spans, cursor)
    }

    /// Render the buffer as a window of soft-wrapped visual rows from row
    /// `top` — the caller's window, which this only moves as far as it takes
    /// to show the cursor — with the cursor's visual position and the
    /// per-row selection highlight.
    pub fn visible_window(&self, top: usize, width: usize, height: usize) -> PromptWindow {
        if width == 0 || height == 0 {
            return PromptWindow {
                rows: Vec::new(),
                cursor_visual: (0, 0),
                total_rows: 0,
                top: 0,
                selection: Vec::new(),
                origins: Vec::new(),
            };
        }
        let (spans, cursor) = self.layout(width);
        let top = showing(top, cursor.0, spans.len(), height);
        let sel_shape = self.selection_shape();
        let shown = &spans[top..(top + height).min(spans.len())];
        let mut rows = Vec::with_capacity(shown.len());
        let mut selection = Vec::with_capacity(shown.len());
        let mut origins = Vec::with_capacity(shown.len());
        for span in shown {
            let line = &self.lines[span.line];
            let (rstart, rend) = (span.start, span.start + span.len);
            rows.push(line[rstart..rend].iter().collect());
            let sel = sel_shape
                .and_then(|s| s.row_span(span.line, line.len()))
                .and_then(|(slo, shi)| {
                    let lo = slo.max(rstart);
                    let hi = shi.min(rend);
                    // `then` (lazy), not `then_some` (eager): a visual row that
                    // starts *past* the selection's end (`shi < rstart`, common
                    // on a wrapped line) has `hi < rstart`, so computing
                    // `hi - rstart` eagerly underflows. The closure runs only
                    // when `lo < hi`, where `hi > lo >= rstart` makes it safe.
                    (lo < hi).then(|| (lo - rstart, hi - rstart))
                });
            selection.push(sel);
            origins.push((span.line, span.start));
        }
        PromptWindow {
            rows,
            cursor_visual: (cursor.0.saturating_sub(top), cursor.1),
            total_rows: spans.len(),
            top,
            selection,
            origins,
        }
    }

    /// Logical line `i`'s text, if there is one — what a renderer matches
    /// against when a row is only part of it (see `PromptWindow::origins`).
    pub fn line_text(&self, i: usize) -> Option<String> {
        self.lines.get(i).map(|l| l.iter().collect())
    }

    /// Reverse of [`visible_window`](Self::visible_window)'s wrap: map a click at
    /// window-relative visual coordinates — `vrow` a visible-row index, `vcol` a
    /// display-cell column — in the window from `top` back to a logical `(row,
    /// col)`. The same `Buffer::layout` and the same window as the
    /// render, so a click lands on the glyph it's over. Clamps into range.
    /// Drives mouse positioning and drag-select.
    pub fn cursor_from_visual(
        &self,
        top: usize,
        width: usize,
        height: usize,
        vrow: usize,
        vcol: usize,
    ) -> (usize, usize) {
        if width == 0 || height == 0 || self.lines.is_empty() {
            return (0, 0);
        }
        let (spans, cursor) = self.layout(width);
        let top = showing(top, cursor.0, spans.len(), height);
        let RowSpan {
            line: li,
            start,
            len,
        } = spans[(top + vrow).min(spans.len() - 1)];
        // Walk the row's chars by display width to land on the clicked cell;
        // past the row's end clamps to its last char boundary.
        let line = &self.lines[li];
        let mut col = start;
        let mut cells = 0usize;
        while col < start + len {
            let w = cells_width(&line[col..col + 1]);
            if cells + w > vcol {
                break;
            }
            cells += w;
            col += 1;
        }
        (li, col)
    }

    /// Resolve an operator target to the concrete [`RangeShape`] it covers,
    /// without mutating the buffer — a shut fold it touches whole
    /// ([`whole_folds`](Self::whole_folds)), but under a charwise or block
    /// selection, which Neovim leaves as it is (`vjy` into a fold takes to
    /// the cursor).
    fn resolve(&mut self, op: Operator, target: OpTarget, count: usize) -> Option<RangeShape> {
        let shape = self.resolve_unfolded(op, target, count)?;
        let selected =
            matches!(target, OpTarget::Selection) && !matches!(shape, RangeShape::Lines { .. });
        Some(if selected {
            shape
        } else {
            self.whole_folds(shape)
        })
    }

    /// **A shut fold, whole to an operator** — "a closed fold is included as
    /// a whole" (`:h fold-behavior`): a range starting in one starts at its
    /// first line's start, one ending in one at its last line's end, unless
    /// it ends at the fold's very start (the line break before it). Vim's
    /// `do_pending_operator`; measured in Neovim on a fold over lines 2–4:
    /// `yy` `2yy` `yj` `V` linewise, `yl` `yw` charwise, each the fold whole.
    fn whole_folds(&self, shape: RangeShape) -> RangeShape {
        if self.folds.is_empty() {
            return shape;
        }
        match shape {
            RangeShape::Lines { first, last } => RangeShape::Lines {
                first: self.fold_start(first),
                last: self.fold_end(last),
            },
            RangeShape::Chars { start, end } => {
                let start = self.fold_at(start.0).map_or(start, |(first, _)| (first, 0));
                let end = match self.fold_at(end.0) {
                    Some((first, past)) if end.1 > 0 || end.0 > first => {
                        (past - 1, self.lines[past - 1].len())
                    }
                    _ => end,
                };
                RangeShape::Chars { start, end }
            }
            block @ RangeShape::Block { .. } => block,
        }
    }

    /// The line `count - 1` lines below `row`, a shut fold counting one —
    /// `2yy` on a fold takes it and the line after (measured). Stops at the
    /// last line, so a count past the buffer costs its length, not itself.
    fn lines_below(&self, mut row: usize, count: usize) -> usize {
        let last = self.lines.len().saturating_sub(1);
        for _ in 1..count {
            if row >= last {
                break;
            }
            row = (self.fold_end(row) + 1).min(last);
        }
        row
    }

    /// [`resolve`](Self::resolve), before a shut fold is taken whole.
    fn resolve_unfolded(
        &mut self,
        op: Operator,
        target: OpTarget,
        count: usize,
    ) -> Option<RangeShape> {
        match target {
            OpTarget::Line => {
                let cr = self.cursor_row;
                Some(self.line_span_shape(cr, self.lines_below(cr, count)))
            }
            OpTarget::LineTail(lines) => {
                let cr = self.cursor_row;
                let er = (cr + lines.saturating_sub(1)).min(self.lines.len() - 1);
                Some(RangeShape::Chars {
                    start: (cr, self.cursor_col),
                    end: (er, self.lines[er].len()),
                })
            }
            OpTarget::LineMotion(motion) => {
                let cr = self.cursor_row;
                // By `landing`, so a motion that fails cancels the operator
                // as vim's beep does — `d200%` cut the cursor's line.
                let ((tr, _), _) = self.landing(motion, count)?;
                if tr == cr && matches!(motion, CursorMotion::Up | CursorMotion::Down) {
                    return None;
                }
                Some(self.line_span_shape(cr.min(tr), cr.max(tr)))
            }
            OpTarget::Selection => self.selection_shape(),
            OpTarget::TextObject { scope, object } => {
                match object_span(&self.lines, self.cursor(), scope, object, count)? {
                    ObjectSpan::Chars { start, end } => Some(self.operated(op, start, end)),
                    ObjectSpan::Lines { top, bottom } => Some(self.line_span_shape(top, bottom)),
                }
            }
            // `2y3]]` is `y6]]`: the operator's count multiplies the one the
            // motion carries.
            OpTarget::Motion(CursorMotion::Section {
                forward,
                end,
                count: steps,
            }) => self.section_shape(forward, end, steps.saturating_mul(count)),
            OpTarget::Motion(motion) => self.charwise_shape(op, motion, count),
        }
    }

    /// **What a charwise motion spans under `op`** (`dw` `de` `db` `d$` …),
    /// across lines as vim's does: a word motion lands by the walker with an
    /// operator's specials ([`word_target`](Self::word_target)), any other by
    /// [`motion_target`](Self::motion_target); then the rules every charwise
    /// range takes ([`operated`](Self::operated)). It clamped every motion to
    /// the cursor's line, so `db` at a line's start did nothing and `de` from
    /// a line's last word stopped at its end (review stop 12, T6 5). `None`:
    /// the motion failed, and vim's beep cancels the operator.
    ///
    /// vim moves a forward motion's end back off a line's end, included
    /// (`adjust_cursor`). Here that is no step: an exclusive end at a line's
    /// end already takes its last char, and an inclusive one past it is cut
    /// to the line where a range is read — by [`RangeShape::row_span`] (the
    /// register, `~`, `r`) and by `drain`'s own clamp. The step was written,
    /// and mutated: no row moved.
    fn charwise_shape(
        &mut self,
        op: Operator,
        motion: CursorMotion,
        count: usize,
    ) -> Option<RangeShape> {
        let from = self.cursor();
        let (to, inclusive) = match word_motion(motion) {
            Some(big) => self.word_target(op, motion, big, count)?,
            None => self.landing(motion, count)?,
        };
        let (start, last) = if from <= to { (from, to) } else { (to, from) };
        let end = if inclusive {
            (last.0, last.1 + 1)
        } else {
            last
        };
        Some(self.operated(op, start, end))
    }

    /// **Where a word motion under `op` lands, and whether that char is in
    /// the range** — vim's `nv_wordcmd` and `nv_bck_word`: `cw` on a non-blank
    /// is an `e` that stops at its own word's end (`:h cw`), so from a word's
    /// last char it changes that char; `w` stops at its line's end on its last
    /// count (`:h w`); a `b` whose count starts at the buffer's start cancels
    /// (`None`).
    fn word_target(
        &self,
        op: Operator,
        motion: CursorMotion,
        big: bool,
        count: usize,
    ) -> Option<(word::Pos, bool)> {
        use CursorMotion as M;
        let at = self.cursor();
        let on_word = self.lines[at.0]
            .get(at.1)
            .is_some_and(|c| !c.is_whitespace());
        Some(match motion {
            M::WordForward | M::BigWordForward if op == Operator::Change && on_word => {
                (word::next_end(&self.lines, at, count, big, true).0, true)
            }
            M::WordForward | M::BigWordForward => {
                (word::next_start(&self.lines, at, count, big, true).0, false)
            }
            M::WordEnd | M::BigWordEnd => {
                (word::next_end(&self.lines, at, count, big, false).0, true)
            }
            _ => match word::prev_start(&self.lines, at, count, big) {
                (to, true) => (to, false),
                (_, false) => return None,
            },
        })
    }

    /// **The range `op` covers from `start` to `end` (exclusive)**, by vim's
    /// rules for a charwise motion or object (`:h exclusive`): an end in
    /// column 0 of a later line moves to the end of the line before — or,
    /// from at or before the start line's first non-blank, the range is those
    /// lines (`:h exclusive-linewise`); and a `d` over lines with only blanks
    /// before its start and after its end takes them whole (`:h d-special`).
    fn operated(&self, op: Operator, start: word::Pos, end: word::Pos) -> RangeShape {
        let in_indent = |(row, col): word::Pos| {
            (self.lines[row].iter())
                .take_while(|c| c.is_whitespace())
                .count()
                >= col
        };
        let mut end = end;
        if end.1 == 0 && end.0 > start.0 {
            let row = end.0 - 1;
            if in_indent(start) {
                return self.line_span_shape(start.0, row);
            }
            end = (row, self.lines[row].len());
        }
        let blank_after = (self.lines[end.0].iter().skip(end.1)).all(|c| c.is_whitespace());
        if op == Operator::Delete && end.0 > start.0 && in_indent(start) && blank_after {
            return self.line_span_shape(start.0, end.0);
        }
        RangeShape::Chars { start, end }
    }

    /// The whole-row span `[first, last]`, clamped to the buffer.
    fn line_span_shape(&self, first: usize, last: usize) -> RangeShape {
        let last = last.min(self.lines.len().saturating_sub(1));
        let first = first.min(last);
        RangeShape::Lines { first, last }
    }

    /// **The selection's anchor names a place the buffer has** — clamped by
    /// the two doors every edit passes (`Prompt::apply_counted`,
    /// `Prompt::apply_operator`). A join under an anchor on the row it
    /// removed left the anchor past the last line, for the next read of the
    /// selection to index (review stop 12: the fuzz widened for T6 1 found
    /// it). Clamped rather than cleared, so a Visual selection stays one the
    /// keymap's mode can stand on.
    fn keep_anchor_inside(&mut self) {
        if let Some((row, col)) = self.selection_anchor {
            let row = row.min(self.lines.len().saturating_sub(1));
            let len = self.lines.get(row).map_or(0, Vec::len);
            self.selection_anchor = Some((row, col.min(len)));
        }
    }

    /// The shape of the Visual selection (anchor → cursor), per its kind.
    fn selection_shape(&self) -> Option<RangeShape> {
        let ((sr, sc), (er, ec)) = self.selection_bounds()?;
        Some(match self.selection_kind {
            SelectionKind::Char => RangeShape::Chars {
                start: (sr, sc),
                end: (er, ec + 1),
            },
            SelectionKind::Line => self.line_span_shape(sr, er),
            SelectionKind::Block => RangeShape::Block {
                rows: (sr, er),
                cols: (sc.min(ec), self.block_right_edge(sc, ec)),
            },
        })
    }

    /// The exclusive right-edge column of a block selection.
    fn block_right_edge(&self, sc: usize, ec: usize) -> usize {
        if self.selection_to_eol {
            usize::MAX
        } else {
            sc.max(ec) + 1
        }
    }

    /// Copy the text a shape covers, without mutating — the yank half.
    fn read(&self, shape: RangeShape) -> String {
        self.copied(shape).text()
    }

    /// What a shape covers, with its rows for context — what `read` renders,
    /// and what the clipboard judges a secret by (`Copied`). Every row of the
    /// shape, each spanned by [`RangeShape::row_span`]; a row it spans none of
    /// (an empty one, or a block past its end) still joins, as `""`.
    fn copied(&self, shape: RangeShape) -> Copied {
        Copied::of(
            shape.rows().map(|r| {
                let row = self.lines[r].as_slice();
                let (lo, hi) = shape.row_span(r, row.len()).unwrap_or((0, 0));
                (row, lo, hi)
            }),
            shape.kind() == SelectionKind::Line,
        )
    }

    /// Park the cursor at the start of a resolved range (yank's cursor rule).
    fn move_cursor_to_range_start(&mut self, shape: RangeShape) {
        match shape {
            RangeShape::Chars { start, .. } => {
                self.cursor_row = start.0;
                self.cursor_col = start.1.min(self.lines[start.0].len());
            }
            RangeShape::Lines { first, .. } => {
                self.cursor_row = first;
                self.cursor_col = self.cursor_col.min(self.lines[first].len());
            }
            RangeShape::Block { rows, cols } => {
                self.cursor_row = rows.0;
                self.cursor_col = cols.0.min(self.lines[rows.0].len());
            }
        }
    }

    /// Rewrite every char a shape covers through `f`, in place — line breaks
    /// untouched, through the same [`RangeShape::row_span`] the highlight
    /// draws, so what changes is what was shown selected.
    fn map_chars(&mut self, shape: RangeShape, mut f: impl FnMut(char) -> char) {
        for r in shape.rows() {
            if let Some((lo, hi)) = shape.row_span(r, self.lines[r].len()) {
                for c in &mut self.lines[r][lo..hi] {
                    *c = f(*c);
                }
            }
        }
    }

    /// Where vim leaves the cursor after changing a range in place (Visual
    /// `~`/`u`/`U`/`r`): its first char — the first line's start for a
    /// linewise one, where yank keeps the column. Clamped onto a char.
    fn move_cursor_to_shape_start(&mut self, shape: RangeShape) {
        let (row, col) = match shape {
            RangeShape::Chars { start, .. } => start,
            RangeShape::Lines { first, .. } => (first, 0),
            RangeShape::Block { rows, cols } => (rows.0, cols.0),
        };
        self.cursor_row = row;
        self.cursor_col = col.min(self.lines[row].len().saturating_sub(1));
    }
}

impl Prompt {
    pub fn new() -> Self {
        Self::default()
    }

    /// **An empty draft sharing `registers`** — a prompt window's, so what
    /// one yanks the next pastes ([`Registers`]).
    pub fn sharing(registers: &Registers) -> Self {
        Prompt {
            register: registers.clone(),
            ..Self::default()
        }
    }

    /// **Drop the insert session in flight** — a counted insert's repeat, a
    /// block insert's rows, the undo run — for a draft the cursor has left
    /// for another window: vim drops the count when the cursor moves. The
    /// mode is one keymap's, every draft's, so its `<Esc>` would come later,
    /// in another insert begun here, and replay into it: `3iab`, a click
    /// away and back, `iz<Esc>` read `abzabzabz` (review stop 11, A 1).
    pub fn end_insert_session(&mut self) {
        self.pending_insert_repeat = None;
        self.pending_block_insert = None;
        self.coalescing = false;
    }

    /// **`f`, editing `buffer`** — lent for the call, and handed back with
    /// what `f` did to it: text, cursor, selection. What this editor keeps
    /// is everything else — its undo and redo, the register, an insert in
    /// flight — so a buffer some window holds (a modifiable page's,
    /// `page::PageBuffer::edit`) is edited by its page's one editor: vim's
    /// undo is a buffer's, its cursor a window's. An undo restores into
    /// whichever buffer is lent, since a snapshot is text and a cursor.
    pub fn lent<R>(&mut self, buffer: &mut Buffer, f: impl FnOnce(&mut Prompt) -> R) -> R {
        std::mem::swap(&mut self.buf, buffer);
        let out = f(self);
        std::mem::swap(&mut self.buf, buffer);
        out
    }

    /// All lines as `String`s, in order — a test's reading of the draft.
    /// The renderer uses `visible_lines`, so a buffer taller than the
    /// prompt pane still shows the cursor row. Test-only: the compiler
    /// cannot say so of a `pub` item, and an `allow(dead_code)` hid it.
    #[cfg(test)]
    pub fn lines(&self) -> impl Iterator<Item = String> + '_ {
        self.buf.lines()
    }

    /// Render the prompt buffer as a window of soft-wrapped visual
    /// rows. Logical lines are wrapped to `width` *display cells* (shared
    /// [`crate::wrap::wrap_cells`]), shown from the window's own top
    /// ([`settle`](Self::settle)) — moved only as far as it takes to keep the
    /// cursor on screen, should a frame be drawn unsettled.
    ///
    /// The cursor's visual `(row, col)` is computed in cells, mapped from
    /// its logical char index through the same wrap: find the visual row
    /// holding `cursor_col`, then sum the display widths of the chars
    /// before it on that row. At a wrap boundary the cursor lands at the
    /// start of the next row.
    ///
    /// "Phantom row" handling: when the cursor sits past the right edge of
    /// a full row at end-of-line (e.g. a row exactly `width` cells wide,
    /// cursor just after it), a fresh empty row is appended for the cursor
    /// to occupy — it has no cell on the full row. This matches vim's
    /// `wrap` behavior.
    pub fn visible_window(&self, width: usize, height: usize) -> PromptWindow {
        self.buf.visible_window(self.top, width, height)
    }

    /// **Settle the window against the size it is about to be drawn at** —
    /// once a frame (`App::before_frame`), which is the one place every edit
    /// passes through: a motion, typing, a paste, `$EDITOR`, a history recall
    /// and a submit all change the cursor, and none has to remember to scroll.
    ///
    /// The cursor pushes the window `scrolloff` rows from an edge and does not
    /// move it in between (`viewport::follow`); a new size refills the last
    /// page, while an unchanged one lets a `zt` near the end stay past it.
    pub fn settle(&mut self, width: usize, height: usize, scrolloff: usize) {
        if width == 0 || height == 0 {
            return;
        }
        let (spans, cursor) = self.buf.layout(width);
        let resized = self.settled != Some((width, height));
        self.settled = Some((width, height));
        let last_line_at = spans
            .iter()
            .position(|s| s.line == spans[spans.len() - 1].line)
            .unwrap_or(0);
        let ceiling = crate::viewport::top_ceiling(spans.len(), last_line_at, height, resized);
        self.top = crate::viewport::follow(
            cursor.0,
            self.top.min(ceiling),
            spans.len(),
            height,
            scrolloff,
        );
    }

    /// `zt` `zz` `zb` (and `z<CR>` `z.` `z-`, which also go to the first
    /// non-blank): put the cursor's line at the top, middle or bottom of a
    /// `height`-row window — `line` (`20zt`, 1-based) first, as vim's count
    /// is the LINE (`:h zt`). The window is the caller's size, the rows
    /// [`settle`](Self::settle) will keep.
    pub fn place(
        &mut self,
        at: crate::viewport::Place,
        line: Option<usize>,
        first_non_blank: bool,
        (width, height): (usize, usize),
        scrolloff: usize,
    ) {
        if let Some(n) = line {
            self.buf
                .place_cursor(n.saturating_sub(1), self.buf.cursor_col);
        }
        if first_non_blank {
            self.cursor_to_first_non_blank();
        }
        self.buf.clamp_cursor_to_char();
        if width == 0 || height == 0 {
            return;
        }
        let (spans, _) = self.buf.layout(width);
        let row = spans
            .iter()
            .position(|s| s.line == self.buf.cursor_row)
            .unwrap_or(0);
        let rows = spans
            .iter()
            .filter(|s| s.line == self.buf.cursor_row)
            .count();
        self.top = crate::viewport::place_top(at, row, rows, height, scrolloff);
    }

    /// `H` `M` `L`: to the logical line at that place in the window, on its
    /// first non-blank char (`:h H`) — measured against what is ON screen,
    /// so a line cut off at an edge does not count (`viewport::lines_on_screen`).
    pub fn screen_line(
        &mut self,
        which: crate::viewport::ScreenLine,
        (width, height): (usize, usize),
        scrolloff: usize,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let (spans, cursor) = self.buf.layout(width);
        let top = showing(self.top, cursor.0, spans.len(), height);
        let mut heights = vec![0usize; self.buf.lines.len()];
        for span in &spans {
            heights[span.line] += 1;
        }
        let Some((first, last)) = crate::viewport::lines_on_screen(heights, top, height) else {
            return;
        };
        let line = crate::viewport::screen_line(
            which,
            first,
            last,
            self.buf.lines.len(),
            scrolloff,
            false,
        );
        self.buf.place_cursor(line, 0);
        self.cursor_to_first_non_blank();
        self.buf.clamp_cursor_to_char();
    }

    /// The cursor to where its line's text begins ([`Buffer::first_non_blank`]).
    fn cursor_to_first_non_blank(&mut self) {
        let row = self.buf.cursor_row;
        let col = self.buf.first_non_blank(row);
        self.buf.place_cursor(row, col);
    }

    /// Logical line `i`'s text; see [`Buffer::line_text`].
    pub fn line_text(&self, i: usize) -> Option<String> {
        self.buf.line_text(i)
    }

    /// Logical `(row, col)` for a click at window-relative visual `(vrow, vcol)`
    /// — the mouse-positioning reverse map (see [`Buffer::cursor_from_visual`]).
    pub fn cursor_from_visual(
        &self,
        width: usize,
        height: usize,
        vrow: usize,
        vcol: usize,
    ) -> (usize, usize) {
        self.buf
            .cursor_from_visual(self.top, width, height, vrow, vcol)
    }

    /// Place the cursor at a logical `(row, col)` (mouse positioning); see
    /// [`Buffer::place_cursor`]. Selection anchor untouched.
    pub fn place_cursor(&mut self, row: usize, col: usize) {
        self.buf.place_cursor(row, col);
    }

    /// `(row, col)` of the cursor in *logical* buffer coordinates — a
    /// test's reading, like [`lines`](Self::lines).
    #[cfg(test)]
    pub fn cursor(&self) -> (usize, usize) {
        self.buf.cursor()
    }

    /// `/` `?` `n` `N` in the prompt — [`Buffer::search`], the one search.
    pub fn search(&mut self, re: &regex::Regex, forward: bool, count: usize) -> Option<bool> {
        self.buf.search(re, forward, count)
    }

    /// What `*` and `#` search for here — [`Buffer::cursor_to_word`].
    pub fn cursor_to_word(&mut self) -> Option<(String, bool)> {
        self.buf.cursor_to_word()
    }

    pub fn line_count(&self) -> usize {
        self.buf.line_count()
    }

    /// Apply an edit action once. Editing never "produces" a value —
    /// submit is a separate gesture (`Prompt::take`) the app drives on
    /// `KeyOutcome::Submit`, not a buffer edit.
    pub fn apply(&mut self, action: EditAction) {
        self.apply_counted(action, 1);
    }

    /// Apply `action` `count` times (clamped to >= 1) as a single undo
    /// group — a counted Normal-mode command (`3p`, `3w`) is one gesture
    /// and one undo step, matching vim. (`3x` is not one: `x` is `dl`, an
    /// operator, `apply_operator`'s.)
    ///
    /// Undo bookkeeping happens here, once for the whole group: a mutation
    /// snapshots the buffer first, *coalescing* a run of typing (and a
    /// line-open that starts one) into a single undo step. Motions and
    /// undo/redo never checkpoint. The repetition itself is in `dispatch`,
    /// which carries no undo logic.
    pub fn apply_counted(&mut self, action: EditAction, count: usize) {
        use EditAction::*;
        let count = count.max(1);
        // A typing "run" coalesces into a single undo step: **every edit that
        // only fires during an Insert session** — char inserts, `Newline`, the
        // insert-mode deletes (`Backspace`/`Ctrl-W`/`Ctrl-U`/`Delete`), and a
        // line-open that starts one. vim makes a whole `i…<Esc>` one undo
        // whatever you do inside it (`ifoo<BS>x<Esc>` → one undo); lumping these
        // with Normal-mode mutations used to split the session. (Normal-mode
        // deletes/changes go through *operators*, not these actions, so they're
        // unaffected; `Paste` is a discrete Normal gesture and stays its own
        // undo.)
        //
        // A Replace session (`R…<Esc>`) is a run the same way: its typing and
        // its `<BS>` are one undo, and the first char typed opens it — so an
        // `R<Esc>` that typed nothing leaves no empty undo step behind.
        let is_run = matches!(
            action,
            Insert(_)
                | Newline
                | DeleteBack
                | DeleteForward
                | DeleteWordBack
                | DeleteToLineStart
                | OpenLineBelow
                | OpenLineAbove
                | ReplaceType(_)
                | ReplaceBack
        );
        let mutating = is_run || matches!(action, Paste | PasteBefore);
        if mutating && !(is_run && self.coalescing) {
            self.checkpoint();
        }
        self.coalescing = is_run;
        let started = self.buf.cursor();

        for _ in 0..count {
            let before = (
                self.buf.cursor_row,
                self.buf.cursor_col,
                self.buf.lines.len(),
                self.buf.lines[self.buf.cursor_row].len(),
            );
            self.dispatch(action);
            // Fixed point: a delete/motion that saturated at a boundary
            // won't do anything on the remaining repeats, so stop. Bounds
            // a pathological `99999x` on a long line. (Inserts always grow
            // the line, so this never short-circuits them.) (Reviewer B.)
            //
            // Guarded by `a_pathological_count_is_bounded_by_distance_not_by_the_count`,
            // which is wall-clock because this is a responsiveness property:
            // deleting this break makes that test take ~7s against a 1s ceiling.
            if before
                == (
                    self.buf.cursor_row,
                    self.buf.cursor_col,
                    self.buf.lines.len(),
                    self.buf.lines[self.buf.cursor_row].len(),
                )
            {
                break;
            }
        }
        self.buf.keep_anchor_inside();
        let prior = self.buf.want;
        self.buf.settle_want(action, started);
        self.unsettled = match action {
            Move(_) if self.buf.want != prior => Some((prior, started)),
            _ => None,
        };
    }

    /// Carry out one edit action against the buffer. No undo bookkeeping
    /// lives here — `apply_counted` owns that so a counted command stays a
    /// single group.
    fn dispatch(&mut self, action: EditAction) {
        use EditAction::*;
        match action {
            Insert(c) => self.insert_char(c),
            Newline => self.newline(),
            DeleteBack => self.delete_back(),
            DeleteForward => self.delete_forward(),
            DeleteWordBack => self.delete_word_back(),
            DeleteToLineStart => self.delete_to_line_start(),
            Move(m) => {
                // vim restarts the Replace session where an arrow key moved
                // you — even back to the same spot, which `replace_session`'s
                // own check would let continue.
                if let Some(r) = &mut self.replacing {
                    *r = Replacing::fresh();
                }
                // And ends the insert session once the cursor moves (vim's
                // `arrow_used`), as a click or a focus move does:
                // `3i<Down>x<Esc>` puts one `x`. The repeat replayed
                // everything from where typing began to the cursor (review
                // stop 16, A M7, measured). Its undo run `apply_counted`
                // has already ended.
                self.end_insert_session();
                self.buf.apply_motion(m)
            }
            ClampCursorAfterInsert => self.clamp_cursor_after_insert(),
            BeginBlockInsert(edge) => self.begin_block_insert(edge),
            EnterInsert { entry, count } => self.enter_insert(entry, count),
            OpenLineBelow => self.open_line_below(),
            OpenLineAbove => self.open_line_above(),
            Undo => self.undo_edit(),
            Redo => self.redo_edit(),
            SetAnchor => self.buf.set_anchor(SelectionKind::Char),
            SetAnchorLine => self.buf.set_anchor(SelectionKind::Line),
            SetAnchorBlock => self.buf.set_anchor(SelectionKind::Block),
            SetSelectionKind(kind) => self.buf.selection_kind = kind,
            SelectTextObject {
                scope,
                object,
                count,
            } => self.buf.select_text_object(scope, object, count),
            SwapSelectionEnds => self.buf.swap_selection_ends(),
            ClearAnchor => self.buf.selection_anchor = None,
            Paste => self.paste(false),
            PasteBefore => self.paste(true),
            ToggleCase { count } => self.toggle_case(count),
            ReplaceChars { with, count } => self.replace_chars(with, count),
            EnterReplace { count } => {
                self.replacing = Some(Replacing {
                    count: count.max(1),
                    ..Replacing::fresh()
                });
            }
            ReplaceType(c) => self.replace_type(c),
            ReplaceBack => self.replace_back(),
            LeaveReplace => self.leave_replace(),
        }
    }

    /// **One undo step for a command that ran — changed text or not.** vim's
    /// rule for `~`, `r` and the Visual case/replace operators: `~` over a
    /// digit, `rb` over a `b`, `vu` over lowercase each leave a step that `u`
    /// undoes, while a command that FAILED (`5ra` with three chars left, `~`
    /// on an empty line) leaves none — which is why the callers return early
    /// for those and call this only once the command has run (nvim, with an
    /// undo break between commands; Reviewer C, round 19).
    fn undo_step(&mut self, edit: impl FnOnce(&mut Self)) {
        let before = self.snapshot();
        edit(self);
        self.push_undo(before);
        self.coalescing = false;
    }

    /// `{count}~` — toggle the case of `count` chars from the cursor, stopping
    /// at the line's end, and land after them: on the last char at most. One
    /// undo step (`undo_step`); none on an empty line, where it fails.
    fn toggle_case(&mut self, count: usize) {
        let row = self.buf.cursor_row;
        let len = self.buf.lines[row].len();
        let Some(last) = len.checked_sub(1) else {
            return; // an empty line: nothing to toggle, nowhere to go
        };
        let from = self.buf.cursor_col.min(last);
        let to = from.saturating_add(count.max(1)).min(len);
        self.undo_step(|p| {
            for c in &mut p.buf.lines[row][from..to] {
                *c = CaseOp::Toggle.apply(*c);
            }
            p.buf.cursor_col = to.min(last);
        });
    }

    /// `{count}r{char}` — replace `count` chars from the cursor, all or
    /// nothing (fewer left on the line changes nothing, as in vim), landing on
    /// the last one replaced. A `'\n'` replaces them with ONE line break and
    /// lands at the new line's start. One undo step (`undo_step`); none when
    /// it fails.
    fn replace_chars(&mut self, with: char, count: usize) {
        let (row, col) = (self.buf.cursor_row, self.buf.cursor_col);
        let count = count.max(1);
        let end = match col.checked_add(count) {
            Some(end) if end <= self.buf.lines[row].len() => end,
            _ => return,
        };
        self.undo_step(|p| {
            if with == '\n' {
                let tail = p.buf.lines[row].split_off(end);
                p.buf.lines[row].truncate(col);
                p.buf.lines.insert(row + 1, tail);
                p.buf.cursor_row = row + 1;
                p.buf.cursor_col = 0;
            } else {
                p.buf.lines[row][col..end].fill(with);
                p.buf.cursor_col = end - 1;
            }
        });
    }

    /// **The Replace session, if it is still where it left off** — the cursor
    /// and the line it was typing exactly as its last keystroke left them.
    /// Anything else that moved the cursor or rewrote that line in between —
    /// a click, a paste, an `$EDITOR` result, an undo, a marked paste — means
    /// the stacks no longer describe the text: `<BS>` would put a remembered
    /// char wherever the cursor now is, and a count would replay there
    /// (Reviewers A, B, C, round 19: `Rxy`, click, `<BS>` turned `abcdef` into
    /// `xycdbf`). So the session restarts here, as vim restarts it wherever
    /// you moved to — one check every Replace action passes, rather than a
    /// reset at each door that can move the cursor, which the next door
    /// added would miss.
    fn replace_session(&mut self) -> Option<&mut Replacing> {
        let (row, col) = (self.buf.cursor_row, self.buf.cursor_col);
        let line = &self.buf.lines[row];
        let session = self.replacing.as_mut()?;
        let moved = session
            .tip
            .as_ref()
            .is_some_and(|(r, c, at)| (*r, *c) != (row, col) || at != line);
        if moved {
            *session = Replacing::fresh();
        }
        Some(session)
    }

    /// Record where a Replace keystroke left things — see
    /// [`replace_session`](Self::replace_session).
    fn mark_replace_tip(&mut self) {
        let (row, col) = (self.buf.cursor_row, self.buf.cursor_col);
        if let Some(session) = &mut self.replacing {
            session.tip = Some((row, col, self.buf.lines[row].clone()));
        }
    }

    /// One key typed in Replace mode: overwrite the char under the cursor, or
    /// append at the line's end; a `'\n'` breaks the line and overwrites
    /// nothing (vim). Recorded for `<BS>` and the replay.
    fn replace_type(&mut self, c: char) {
        self.replace_session();
        let (row, col) = (self.buf.cursor_row, self.buf.cursor_col);
        let covered = if c == '\n' {
            self.newline();
            None
        } else if col < self.buf.lines[row].len() {
            let was = std::mem::replace(&mut self.buf.lines[row][col], c);
            self.buf.cursor_col = col + 1;
            Some(was)
        } else {
            self.insert_char(c);
            None
        };
        if let Some(session) = &mut self.replacing {
            session.covered.push(covered);
            session.typed.push(c);
        }
        self.mark_replace_tip();
    }

    /// `<BS>` in Replace mode: take back the last char typed this session —
    /// put back what it overwrote, or remove what it appended or the break it
    /// made. With nothing typed it only steps left, on this line.
    fn replace_back(&mut self) {
        let undone = self.replace_session().and_then(|session| {
            session.typed.pop();
            session.covered.pop()
        });
        match undone {
            Some(Some(was)) => {
                // The session is where it left off, so the overwrite it
                // undoes is the char just behind the cursor.
                let Some(col) = self.buf.cursor_col.checked_sub(1) else {
                    return;
                };
                self.buf.cursor_col = col;
                self.buf.lines[self.buf.cursor_row][col] = was;
            }
            Some(None) => self.delete_back(),
            None => self.buf.cursor_col = self.buf.cursor_col.saturating_sub(1),
        }
        self.mark_replace_tip();
    }

    /// `<Esc>` from Replace: replay a counted session's typing `count - 1`
    /// more times, each overwriting as it goes, then step onto the last typed
    /// char as leaving Insert does. A session that no longer is where it left
    /// off replays nothing. The replay joins the session's undo group.
    fn leave_replace(&mut self) {
        self.replace_session();
        if let Some(session) = self.replacing.take() {
            for _ in 1..session.count {
                for &c in &session.typed {
                    self.replace_type(c);
                }
            }
        }
        self.buf.cursor_col = self.buf.cursor_col.saturating_sub(1);
    }

    /// Vim's `<Esc>`-from-Insert nudge: step the cursor one column left onto
    /// the last typed char, staying on the line (no wrap to the previous
    /// line's end the way a plain `h`/`move_left` would). At column 0 it
    /// stays put. Bridges Insert's "cursor between chars, may sit at `len`"
    /// with Normal's "cursor on a char". Also finalizes either pending insert
    /// before the nudge: a block-insert (`Ctrl-V`…`I`/`A`, replicate down the
    /// rectangle) or a counted insert (`3i`…, replay the typed text). At most
    /// one is ever armed at a time.
    fn clamp_cursor_after_insert(&mut self) {
        if let Some(bi) = self.pending_block_insert.take() {
            self.finish_block_insert(bi);
        }
        if let Some(rep) = self.pending_insert_repeat.take() {
            self.finish_insert_repeat(rep);
        }
        self.buf.cursor_col = self.buf.cursor_col.saturating_sub(1);
    }

    /// Enter Insert from Normal (`i`/`a`/`A`/`I`/`o`/`O`): reposition (or open
    /// a line), then arm a repeat when `count > 1` so the typed text replays
    /// on `<Esc>`. `o`/`O` open a line (a mutation, so they own an undo
    /// checkpoint and coalesce the run); the reposition entries don't touch
    /// the buffer, leaving the first typed char to start the undo group.
    fn enter_insert(&mut self, entry: InsertEntry, count: usize) {
        match entry {
            InsertEntry::Before => {}
            InsertEntry::After => self.buf.move_right(),
            InsertEntry::LineStart => self.buf.move_home(),
            InsertEntry::LineEnd => self.buf.move_end(),
            InsertEntry::OpenBelow => {
                self.checkpoint();
                self.open_line_below();
                self.coalescing = true;
            }
            InsertEntry::OpenAbove => {
                self.checkpoint();
                self.open_line_above();
                self.coalescing = true;
            }
        }
        if count > 1 {
            self.pending_insert_repeat = Some(InsertRepeat {
                count,
                begin: (self.buf.cursor_row, self.buf.cursor_col),
                entry,
            });
        }
    }

    /// Replay a counted insert's typed text `count - 1` more times. The text
    /// is everything typed from `begin` to the cursor — **including newlines**
    /// (vim `3ifoo<CR>bar<Esc>` repeats the whole `foo\nbar`, not just the last
    /// row). `i`/`a`/`A`/`I` repeat inline at the cursor; `o`/`O` open a fresh
    /// line each — and open all `count` lines even when no text was typed (vim
    /// `3o<Esc>` leaves three blank lines). No checkpoint — the replay joins
    /// the insert's undo group.
    fn finish_insert_repeat(&mut self, rep: InsertRepeat) {
        let cursor = (self.buf.cursor_row, self.buf.cursor_col);
        // Fail the replay safe only if the cursor ended up *before* where
        // typing began (a backward move / odd state). A multi-row insert is
        // replayed faithfully via the same `read`/`splice` the yank-paste path
        // uses, so newlines carry through.
        if cursor < rep.begin {
            return;
        }
        let text = self.buf.read(RangeShape::Chars {
            start: rep.begin,
            end: cursor,
        });
        let line_repeat = matches!(rep.entry, InsertEntry::OpenBelow | InsertEntry::OpenAbove);
        // An inline `i`/`a`/`A`/`I` with nothing typed has nothing to replay;
        // `o`/`O` still must open their remaining `count - 1` lines.
        if text.is_empty() && !line_repeat {
            return;
        }
        for _ in 1..rep.count {
            if line_repeat {
                self.open_line_below();
            }
            let (r, c) = (self.buf.cursor_row, self.buf.cursor_col);
            let (nr, nc) = self.splice_text_at(r, c, &text);
            self.buf.cursor_row = nr;
            self.buf.cursor_col = nc;
        }
    }

    /// Enter a block-insert (`Ctrl-V`…`I`/`A`): position the cursor at the
    /// block's chosen edge column on its first row and record the pending
    /// replication (consumed when Insert is left). No buffer mutation here —
    /// so the first typed char owns the undo checkpoint and the later
    /// replication joins that one group. The selection is consumed.
    fn begin_block_insert(&mut self, edge: BlockEdge) {
        if let Some(RangeShape::Block { rows, cols }) = self.buf.selection_shape() {
            let (first, last) = rows;
            let (clo, chi) = cols;
            let col = match edge {
                BlockEdge::Left => clo,
                BlockEdge::Right => chi,
            };
            let start_col = col.min(self.buf.lines[first].len());
            self.buf.cursor_row = first;
            self.buf.cursor_col = start_col;
            self.pending_block_insert = Some(BlockInsert {
                first,
                last,
                col,
                start_col,
                lines: self.buf.lines.len(),
            });
        }
        self.buf.selection_anchor = None;
    }

    /// Replicate the block-insert text to every other row of the block: the
    /// text typed on the first row (`start_col`→cursor) is inserted at
    /// `min(col, row_len)` on each — at the row's own end when it's shorter
    /// than the column (vim's ragged `$`-block append; never space-padded to
    /// align). A no-op if the cursor wandered off the first row or before the
    /// start (e.g. a typed newline or a move), or if lines were joined or
    /// split meanwhile — fail safe, not weird: a `<Del>` at the row's end
    /// joined the next row in, and the replica went onto a line that was
    /// never the block's, or past the last one, a panic (review stop 12, T6 1).
    fn finish_block_insert(&mut self, bi: BlockInsert) {
        if self.buf.cursor_row != bi.first
            || self.buf.cursor_col < bi.start_col
            || self.buf.lines.len() != bi.lines
        {
            return;
        }
        let text: Vec<char> = self.buf.lines[bi.first][bi.start_col..self.buf.cursor_col].to_vec();
        if text.is_empty() {
            return;
        }
        for r in (bi.first + 1)..=bi.last {
            let at = bi.col.min(self.buf.lines[r].len());
            self.buf.lines[r].splice(at..at, text.iter().copied());
        }
    }

    /// Apply an operator (`d`/`c`/`y`) over the range its target describes,
    /// `count` times. The keymap owns the mode switch for `Change`; here we
    /// `resolve` the target to a `RangeShape`, copy it into the register
    /// (every operator — delete *cuts*, exactly like vim, so `ddp`/`xp`
    /// work), then `read`-only (`y`) or `drain` it (`d`/`c`).
    ///
    /// Undo grouping is vim-faithful: snapshot *before* the edit and
    /// checkpoint only if the buffer actually changed — a no-op operator
    /// (`dl` at EOL, `di(` with no pair, `dj` on the last line) creates no
    /// undo step and doesn't wipe redo. And a `Change` *coalesces* into the
    /// text the user then types, so the whole change (delete + typing) is a
    /// single undo, exactly like vim — not a separate "delete" and "insert"
    /// step. A `Yank` never touches the buffer, so it never checkpoints.
    ///
    /// `false` when the target resolved to nothing (`ci(` with no pair, `cj`
    /// on the last line, `db` at the buffer's start) — where vim's beep
    /// cancels the operator, so a `c` stays in Normal (`App`'s to restore).
    ///
    /// One that ran resets the column a vertical motion aims for, as vim's
    /// operators do (`yy` too: measured); a cancelled one leaves it.
    pub fn apply_operator(&mut self, op: Operator, target: OpTarget, count: usize) -> bool {
        let ran = self.apply_operator_once(op, target, count);
        if ran {
            self.buf.want = None;
        }
        ran
    }

    fn apply_operator_once(&mut self, op: Operator, target: OpTarget, count: usize) -> bool {
        let count = count.max(1);
        let shape = self.buf.resolve(op, target, count);
        match op {
            Operator::Yank => {
                // Yank fills the register and parks the cursor at the start
                // of the range (vim: `yw`/`yy` don't move; `yb` moves to
                // start) — an empty range too, which empties the register
                // (`fill_register`). A target that resolved to nothing
                // leaves both.
                if let Some(shape) = shape {
                    self.fill_register(shape, op);
                    self.buf.move_cursor_to_range_start(shape);
                }
                self.coalescing = false; // a complete gesture; breaks a run
            }
            Operator::Delete | Operator::Change => {
                let before = self.snapshot();
                if let Some(shape) = shape {
                    // Delete/change *cut*: the removed text fills the
                    // register before we drain it (so `ddp`, `xp`, visual
                    // `dp` all paste it back).
                    self.fill_register(shape, op);
                    self.drain(shape, op);
                }
                let changed = self.buf.lines != before.lines;
                if changed {
                    self.push_undo(before);
                }
                // A `Change` continues into the inserted text, so let the
                // next typing run join this group (one undo for the whole
                // change). A `Delete` is a complete gesture and breaks a run.
                self.coalescing = changed && op == Operator::Change;
            }
            // In place, char for char — the register is not a cut's to fill.
            // One undo step whether or not a char changed (`vu` over
            // lowercase is a step in vim) — see `undo_step`.
            Operator::Case(_) | Operator::Replace(_) => {
                if let Some(shape) = shape {
                    self.undo_step(|p| {
                        p.buf.map_chars(shape, |c| match op {
                            Operator::Case(case) => case.apply(c),
                            Operator::Replace(with) => with,
                            Operator::Yank | Operator::Delete | Operator::Change => c,
                        });
                        p.buf.move_cursor_to_shape_start(shape);
                    });
                }
            }
        }
        // A selection operator (`d`/`c`/`y` in Visual) consumes the anchor.
        if matches!(target, OpTarget::Selection) {
            self.buf.selection_anchor = None;
        }
        self.buf.keep_anchor_inside();
        shape.is_some()
    }

    /// Copy a resolved shape's text into the register, tagging it linewise
    /// or charwise so paste re-opens lines vs. inserts inline. Shared by
    /// yank and the delete/change cut.
    ///
    /// **An empty range is vim's two ways** (measured, Neovim 0.12.6): a
    /// yank of one empties the register — `yi"` over `""`, `yl` on an empty
    /// line, `y0` at column 0 — and a cut of one leaves it, nothing having
    /// been cut (`di"`, `dl`, `d0` there). This kept it for the yank too,
    /// under "vim's rule", which holds for the cut alone (review stop 12,
    /// T6 6). A failed target (`yi(` with no pair) resolves to no shape and
    /// never comes here.
    fn fill_register(&mut self, shape: RangeShape, op: Operator) {
        let from = self.buf.copied(shape);
        if from.text().is_empty() && op != Operator::Yank {
            return;
        }
        self.set_register(from, shape.kind());
    }

    /// Remove the text a shape covers — the delete/change half of the
    /// split. Charwise drains `[lo, hi)` (single row) or joins the head of
    /// the first row to the tail of the last (cross-row); linewise removes
    /// the rows (`Delete`) or clears the first and keeps it (`Change`, which
    /// the keymap has entered Insert for). The buffer never goes empty — a
    /// fully-deleted buffer keeps one blank line. `op` matters only for the
    /// linewise Delete/Change distinction.
    fn drain(&mut self, shape: RangeShape, op: Operator) {
        // `drain` only ever runs for Delete/Change — yank reads, and the case
        // and replace operators rewrite in place, never removing (see
        // `apply_operator`). Pin it so a refactor that misroutes one here
        // trips in tests instead of silently deleting on a yank or a `~`.
        debug_assert!(
            matches!(op, Operator::Delete | Operator::Change),
            "drain() called with {op:?}"
        );
        match shape {
            RangeShape::Chars { start, end } => {
                let (sr, sc) = start;
                let (er, ec) = end;
                if sr == er {
                    let len = self.buf.lines[sr].len();
                    let lo = sc.min(len);
                    let hi = ec.min(len);
                    if lo < hi {
                        self.buf.lines[sr].drain(lo..hi);
                    }
                    self.buf.cursor_row = sr;
                    self.buf.cursor_col = lo;
                } else {
                    // Keep the head of the first row and the tail of the
                    // last, joined into one row; drop the rows between.
                    let tail: Vec<char> = {
                        let last = &self.buf.lines[er];
                        last[ec.min(last.len())..].to_vec()
                    };
                    let head = sc.min(self.buf.lines[sr].len());
                    self.buf.lines[sr].truncate(head);
                    self.buf.lines[sr].extend(tail);
                    self.buf.lines.drain(sr + 1..=er);
                    self.buf.cursor_row = sr;
                    self.buf.cursor_col = head;
                }
            }
            RangeShape::Lines { first, last } => match op {
                // Delete drops the whole row span. Yank and the in-place
                // operators never reach `drain` (asserted above); they're
                // folded in here only as a release fallback, not because one
                // ever drains lines.
                Operator::Delete | Operator::Yank | Operator::Case(_) | Operator::Replace(_) => {
                    self.buf.lines.drain(first..=last);
                    if self.buf.lines.is_empty() {
                        self.buf.lines.push(Vec::new());
                    }
                    // Onto the line that follows, at its first non-blank —
                    // vim's, under its default 'startofline' (Neovim's
                    // default keeps the column for `dd`). It went to column
                    // 0 (review stop 12, T6 5).
                    self.buf.cursor_row = first.min(self.buf.lines.len() - 1);
                    self.buf.cursor_col = self.buf.first_non_blank(self.buf.cursor_row);
                }
                Operator::Change => {
                    if last > first {
                        self.buf.lines.drain(first + 1..=last);
                    }
                    self.buf.lines[first].clear();
                    self.buf.cursor_row = first;
                    self.buf.cursor_col = 0;
                }
            },
            shape @ RangeShape::Block {
                rows: (rlo, rhi),
                cols: (clo, _),
            } => {
                // Remove each row's `[lo, hi)` (clamped per row by the shared
                // `row_span`, so the highlight and the cut agree). Same cut for
                // Delete and Change; the cursor parks at the top-left corner.
                for r in rlo..=rhi {
                    if let Some((lo, hi)) = shape.row_span(r, self.buf.lines[r].len()) {
                        self.buf.lines[r].drain(lo..hi);
                    }
                }
                self.buf.cursor_row = rlo;
                let start_col = clo.min(self.buf.lines[rlo].len());
                self.buf.cursor_col = start_col;
                // Block `c`: arm the replicate so the text typed on the parked
                // first row is inserted on every row of the block on `<Esc>`
                // (vim `Ctrl-V`…`c`), at the block's left column — reusing the
                // same `BlockInsert` machinery as block-`I`/`A`. Block `d` is a
                // complete gesture and does not replicate.
                if op == Operator::Change {
                    self.pending_block_insert = Some(BlockInsert {
                        first: rlo,
                        last: rhi,
                        col: clo,
                        start_col,
                        lines: self.buf.lines.len(),
                    });
                }
            }
        }
    }

    /// Paste the register at the cursor — `p` (`before == false`) pastes
    /// after, `P` (`before == true`) at/above — honoring the register's
    /// geometry exactly as vim does. No register → no-op.
    fn paste(&mut self, before: bool) {
        let Some(reg) = self.register.0.borrow().clone() else {
            return;
        };
        match reg.kind {
            SelectionKind::Line => self.paste_linewise(&reg.text, before),
            SelectionKind::Char => self.paste_charwise(&reg.text, before),
            SelectionKind::Block => self.paste_block(&reg.text, before),
        }
    }

    /// Linewise paste: each register line becomes a fresh row — below the
    /// cursor for `p`, above for `P`. The caret lands where the first pasted
    /// line's text begins ([`Buffer::first_non_blank`], as vim's does).
    fn paste_linewise(&mut self, text: &str, before: bool) {
        let new_rows: Vec<Vec<char>> = text.lines().map(|l| l.chars().collect()).collect();
        if new_rows.is_empty() {
            return;
        }
        let at = if before {
            self.buf.cursor_row
        } else {
            self.buf.cursor_row + 1
        };
        for (i, row) in new_rows.into_iter().enumerate() {
            self.buf.lines.insert(at + i, row);
        }
        self.buf.cursor_row = at;
        self.buf.cursor_col = self.buf.first_non_blank(at);
    }

    /// The column `p`/`P` insert at on the cursor row: `P` (`before`) at the
    /// cursor, `p` just past the cursor char, clamped to the line length.
    /// Shared by charwise and block paste so the two can't drift.
    fn paste_col(&self, before: bool) -> usize {
        if before {
            self.buf.cursor_col
        } else {
            (self.buf.cursor_col + 1).min(self.buf.lines[self.buf.cursor_row].len())
        }
    }

    /// Charwise paste: insert inline at the cursor (`P`) or just after the
    /// cursor char (`p`). The cursor lands on the last pasted char — or, for
    /// text over lines, on the first, as vim's (`:h p`; it landed on the last
    /// line: review stop 12, T6 5).
    fn paste_charwise(&mut self, text: &str, before: bool) {
        let (row, col) = (self.buf.cursor_row, self.paste_col(before));
        let (er, ec) = self.splice_text_at(row, col, text);
        (self.buf.cursor_row, self.buf.cursor_col) = match text.contains('\n') {
            true => (row, col),
            false => (er, ec.saturating_sub(1)),
        };
    }

    /// Block paste: each segment is inserted at the same column on the
    /// cursor row and successive rows below (creating rows past the buffer
    /// end, and space-padding rows shorter than the target column so the
    /// rectangle stays aligned). `p` inserts after the cursor column, `P`
    /// at it. Cursor parks at the top-left of the pasted block.
    fn paste_block(&mut self, text: &str, before: bool) {
        let col = self.paste_col(before);
        for (i, seg) in text.split('\n').enumerate() {
            let row = self.buf.cursor_row + i;
            if row == self.buf.lines.len() {
                self.buf.lines.push(Vec::new());
            }
            let line = &mut self.buf.lines[row];
            if line.len() < col {
                line.resize(col, ' '); // pad a short row up to the column
            }
            let chars: Vec<char> = seg.chars().collect();
            line.splice(col..col, chars);
        }
        self.buf.cursor_col = col;
    }

    /// The current register contents, if any — what `p` pastes.
    pub fn register_text(&self) -> Option<String> {
        (self.register.0.borrow().as_ref()).map(|r| r.text.clone())
    }

    /// What the register was filled from, with its rows for context — what the
    /// app mirrors to the system clipboard after a yank (`App::copy_to_clipboard`).
    pub fn register_copied(&self) -> Option<Copied> {
        (self.register.0.borrow().as_ref()).map(|r| r.from.clone())
    }

    /// What the Visual selection covers, without touching the buffer or the
    /// register — see [`Buffer::selected`].
    pub fn selected(&self) -> Option<Copied> {
        self.buf.selected()
    }

    /// Fill the unnamed register — from a yank or cut here, or from outside
    /// the prompt: a yank in the *chat* pane lands here so it can be pasted in
    /// the prompt with `p` (vim's global register: a yank in one buffer pastes
    /// in another). `kind` decides linewise vs charwise vs block paste.
    pub fn set_register(&mut self, from: Copied, kind: SelectionKind) {
        *self.register.0.borrow_mut() = Some(Register {
            text: from.text(),
            kind,
            from,
        });
    }

    /// Insert externally-pasted `text` at the cursor as a **single** undo
    /// group — bracketed paste: one gesture, never interpreted key-by-key,
    /// so a paste containing newlines splits lines but fires no commands or
    /// submits. The caller sanitizes the bytes; embedded `'\n'` splits the
    /// row. The cursor ends *after* the inserted text (you keep typing past
    /// it) — unlike vim `p`, which lands on the last char.
    pub fn insert_pasted(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.checkpoint();
        self.buf.want = None;
        self.coalescing = false; // a discrete gesture; the next typing is new
        let (row, col) = (self.buf.cursor_row, self.buf.cursor_col);
        let (er, ec) = self.splice_text_at(row, col, text);
        self.buf.cursor_row = er;
        self.buf.cursor_col = ec;
    }

    /// Splice `text` (which may contain `'\n'`) into `row` at char column
    /// `col`, splitting the row where newlines fall. Returns the position
    /// *just after* the last inserted char; the caller positions the cursor
    /// (vim `p` lands on the last char, bracketed paste after it). Pure
    /// insertion — no cursor or undo bookkeeping.
    fn splice_text_at(&mut self, row: usize, col: usize, text: &str) -> (usize, usize) {
        let mut segments = text.split('\n');
        let first: Vec<char> = segments.next().unwrap_or_default().chars().collect();
        let rest: Vec<Vec<char>> = segments.map(|s| s.chars().collect()).collect();
        let col = col.min(self.buf.lines[row].len());

        if rest.is_empty() {
            // Single-line: splice into the current row, O(line) not O(line²).
            let n = first.len();
            self.buf.lines[row].splice(col..col, first);
            return (row, col + n);
        }

        // Multi-line: the row splits at `col`. The first segment extends the
        // current row; the middle/last segments become new rows; the
        // original tail re-joins the final pasted row.
        let tail = self.buf.lines[row].split_off(col);
        self.buf.lines[row].extend(first);
        let mut insert_at = row + 1;
        for seg in rest {
            self.buf.lines.insert(insert_at, seg);
            insert_at += 1;
        }
        let last_row = insert_at - 1;
        let after = self.buf.lines[last_row].len();
        self.buf.lines[last_row].extend(tail);
        (last_row, after)
    }

    /// The Visual selection anchor, if any. The renderer reads it (with
    /// the cursor) to highlight the selected span.
    pub fn anchor(&self) -> Option<(usize, usize)> {
        self.buf.anchor()
    }

    fn snapshot(&self) -> Snapshot {
        let lines = self.buf.lines.clone();
        let bytes = (lines.iter())
            .map(|line| line.len() * std::mem::size_of::<char>() + std::mem::size_of::<Vec<char>>())
            .sum();
        Snapshot {
            lines,
            cursor_row: self.buf.cursor_row,
            cursor_col: self.buf.cursor_col,
            bytes,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.buf.lines = s.lines;
        self.buf.cursor_row = s.cursor_row;
        self.buf.cursor_col = s.cursor_col;
    }

    /// Push the current state onto the undo stack (before a mutation) and
    /// invalidate the redo stack. Bounded by `UNDO_LIMIT`.
    fn checkpoint(&mut self) {
        let snap = self.snapshot();
        self.push_undo(snap);
    }

    /// Commit a previously-taken snapshot as an undo checkpoint, dropping
    /// the redo stack. Split from `checkpoint` so an operator can snapshot
    /// *before* its edit and commit only if the edit actually changed the
    /// buffer (no undo step / redo wipe for a no-op command — vim's rule).
    fn push_undo(&mut self, snap: Snapshot) {
        self.undo.push(snap);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        let mut held: usize = self.undo.iter().map(|s| s.bytes).sum();
        while held > UNDO_BYTES && self.undo.len() > 1 {
            held -= self.undo.remove(0).bytes;
        }
        self.redo.clear();
    }

    fn undo_edit(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(prev);
        }
        self.coalescing = false;
    }

    fn redo_edit(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(next);
        }
        self.coalescing = false;
    }

    /// Take the buffer's text (lines joined by `'\n'`) and reset to an
    /// empty one-line buffer with the cursor at origin. This is the
    /// submit gesture's accessor.
    pub fn take(&mut self) -> String {
        let joined = self
            .buf
            .lines
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        self.buf.lines = vec![Vec::new()];
        self.buf.cursor_row = 0;
        self.buf.cursor_col = 0;
        // The submitted text's edit history doesn't belong to the next
        // prompt.
        self.undo.clear();
        self.redo.clear();
        self.coalescing = false;
        self.buf.selection_anchor = None;
        self.buf.selection_kind = SelectionKind::Char;
        self.buf.selection_to_eol = false;
        self.buf.want = None;
        self.pending_block_insert = None;
        self.pending_insert_repeat = None;
        // A submit from Replace mode keeps the mode (the keymap's), so the
        // session restarts rather than ending: typing on overwrites and `<BS>`
        // puts back as before, and nothing sent replays into the next prompt.
        if let Some(session) = &mut self.replacing {
            *session = Replacing::fresh();
        }
        joined
    }

    /// The buffer's text (lines joined by `'\n'`) **without** clearing it — the
    /// non-destructive read `take` is the destructive version of. Used to seed
    /// the `$EDITOR` tempfile.
    pub fn text(&self) -> String {
        self.buf
            .lines
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Replace the whole buffer with `text` (the `$EDITOR` result) as a single
    /// undo group, cursor parked at the end. `u` restores the pre-edit prompt.
    pub fn replace_with(&mut self, text: &str) {
        self.checkpoint();
        self.coalescing = false;
        self.buf.want = None;
        self.buf.lines = text.split('\n').map(|l| l.chars().collect()).collect();
        if self.buf.lines.is_empty() {
            self.buf.lines.push(Vec::new());
        }
        self.buf.selection_anchor = None;
        self.buf.cursor_row = self.buf.lines.len() - 1;
        self.buf.cursor_col = self.buf.lines[self.buf.cursor_row].len();
    }

    fn insert_char(&mut self, c: char) {
        self.buf.lines[self.buf.cursor_row].insert(self.buf.cursor_col, c);
        self.buf.cursor_col += 1;
    }

    fn newline(&mut self) {
        let tail = self.buf.lines[self.buf.cursor_row].split_off(self.buf.cursor_col);
        self.buf.lines.insert(self.buf.cursor_row + 1, tail);
        self.buf.cursor_row += 1;
        self.buf.cursor_col = 0;
    }

    fn delete_back(&mut self) {
        if self.buf.cursor_col > 0 {
            self.buf.cursor_col -= 1;
            self.buf.lines[self.buf.cursor_row].remove(self.buf.cursor_col);
        } else if self.buf.cursor_row > 0 {
            // At the start of a non-first line: merge it into the line
            // above, parking the cursor where the join happened.
            let cur = self.buf.lines.remove(self.buf.cursor_row);
            self.buf.cursor_row -= 1;
            self.buf.cursor_col = self.buf.lines[self.buf.cursor_row].len();
            self.buf.lines[self.buf.cursor_row].extend(cur);
        }
    }

    fn delete_forward(&mut self) {
        let line_len = self.buf.lines[self.buf.cursor_row].len();
        if self.buf.cursor_col < line_len {
            self.buf.lines[self.buf.cursor_row].remove(self.buf.cursor_col);
        } else if self.buf.cursor_row + 1 < self.buf.lines.len() {
            // At the end of a non-last line: pull the next line up onto
            // the current one. Cursor stays put.
            let next = self.buf.lines.remove(self.buf.cursor_row + 1);
            self.buf.lines[self.buf.cursor_row].extend(next);
        }
    }

    /// Vim's word-backward delete (Ctrl-W in insert mode): the blanks to
    /// the left of the cursor and the run of one class before them
    /// (`crate::word::start_before`). At the start of a line, falls through
    /// to `delete_back` so the line merge stays consistent.
    fn delete_word_back(&mut self) {
        if self.buf.cursor_col == 0 {
            self.delete_back();
            return;
        }
        // Over the blanks to the left, then one run of a class, as vim's
        // `i_CTRL-W` (`crate::word`): from `cd /usr/lib|`, `lib` goes.
        let new_col =
            crate::word::start_before(&self.buf.lines[self.buf.cursor_row], self.buf.cursor_col);
        self.buf.lines[self.buf.cursor_row].drain(new_col..self.buf.cursor_col);
        self.buf.cursor_col = new_col;
    }

    /// Clamp the cursor onto a character (Normal/Visual). Delegates to the
    /// buffer; the app applies it after a Normal/Visual edit.
    ///
    /// **A motion the clamp undid did not move**: `l` on a line's last char
    /// steps past it and is clamped back — vim's fails, and keeps the column
    /// a vertical motion aims for (`unsettled`).
    pub fn clamp_cursor_to_char(&mut self) {
        self.buf.clamp_cursor_to_char();
        if let Some((prior, started)) = self.unsettled.take()
            && self.buf.cursor() == started
        {
            self.buf.want = prior;
        }
    }

    /// Vim `o`: open an empty line below the cursor's line and move onto
    /// it. (The keymap switches to Insert alongside this.)
    fn open_line_below(&mut self) {
        self.buf.lines.insert(self.buf.cursor_row + 1, Vec::new());
        self.buf.cursor_row += 1;
        self.buf.cursor_col = 0;
    }

    /// Vim `O`: open an empty line above the cursor's line and move onto
    /// it. The new line takes the cursor's row index; `cursor_row` is
    /// unchanged because the old line shifts down.
    fn open_line_above(&mut self) {
        self.buf.lines.insert(self.buf.cursor_row, Vec::new());
        self.buf.cursor_col = 0;
    }

    /// Delete from the cursor back to the start of the line (insert-mode
    /// Ctrl-U). At column 0 it's a no-op — vim's Ctrl-U does not merge
    /// into the previous line.
    fn delete_to_line_start(&mut self) {
        self.buf.lines[self.buf.cursor_row].drain(0..self.buf.cursor_col);
        self.buf.cursor_col = 0;
    }
}

/// Where `re` matches in `line`, as CHAR indices, in order. Counted along
/// the matches rather than from the line's start for each — a transcript
/// line can be a megabyte of minified JSON, and a search must stay linear in
/// it.
fn match_starts(line: &[char], re: &regex::Regex) -> Vec<usize> {
    let text: String = line.iter().collect();
    let (mut byte, mut char) = (0, 0);
    re.find_iter(&text)
        .map(|m| {
            char += text[byte..m.start()].chars().count();
            byte = m.start();
            char
        })
        .collect()
}

/// Whether an operator motion *includes* the char it lands on. `e` lands
/// on the last char of the range and must cover it (`de` deletes through
/// the word's end); the rest stop just short of the target. `$` is vim's
/// inclusive too (`:h $`), but its target here is the line's LENGTH, past
/// the last char, so the inclusive end is one past that and cut back to the
/// line wherever a range is read (`RangeShape::row_span`) — the entry
/// changes no range, an equivalence (review stop 12, T6 12: it said `$`
/// lands on the last char; its mutant stayed green).
fn motion_inclusive(motion: CursorMotion) -> bool {
    matches!(
        motion,
        CursorMotion::WordEnd | CursorMotion::BigWordEnd | CursorMotion::LineEnd
    )
}

/// **The column `find` lands on in `line`, from `col`** — the `count`-th
/// `ch` after (before) it, a `t` (`T`) one short of it; `None` when the line
/// holds fewer. `again` (a `;` or `,`) with a count of one: a `t` standing
/// right before its char looks past it, or `;` would never move (vim's
/// default, `'cpoptions'` without `;`). With more, that char is the first
/// counted, as vim's `searchc` counts it (`t,2;` measured, 11-03).
fn find_in_line(line: &[char], col: usize, find: Find) -> Option<usize> {
    let beside = |at: Option<usize>| {
        find.till
            && find.again
            && find.count == 1
            && at.and_then(|at| line.get(at)) == Some(&find.ch)
    };
    let nth = find.count.max(1) - 1;
    match find.forward {
        true => {
            let from = col + 1 + usize::from(beside(Some(col + 1)));
            let hit = (from..line.len())
                .filter(|&c| line[c] == find.ch)
                .nth(nth)?;
            Some(if find.till { hit - 1 } else { hit })
        }
        false => {
            let to = col.min(line.len()) - usize::from(beside(col.checked_sub(1)));
            let hit = (0..to).rev().filter(|&c| line[c] == find.ch).nth(nth)?;
            Some(if find.till { hit + 1 } else { hit })
        }
    }
}

/// **Which double-quoted span each char of `line` is in**, numbered, `None`
/// outside every one — for `%` ([`Buffer::match_pair`]). A `"` that is
/// escaped or a character literal (`'"'`) opens and closes nothing; a line
/// with an odd number of the rest has no spans, as vim treats one.
fn quoted(line: &[char]) -> Vec<Option<usize>> {
    let quotes: Vec<usize> = (0..line.len())
        .filter(|&c| line[c] == '"' && !escaped(line, c) && !literal(line, c))
        .collect();
    let mut spans = vec![None; line.len()];
    if quotes.len() % 2 == 1 {
        return spans;
    }
    for (n, pair) in quotes.chunks(2).enumerate() {
        for span in &mut spans[pair[0]..=pair[1]] {
            *span = Some(n);
        }
    }
    spans
}

/// **The char at `c` is escaped** — an odd run of `\` before it. `%` pairs
/// an escaped bracket only with an escaped one ([`Buffer::match_pair`]),
/// and an escaped `"` opens no quote ([`quoted`]).
fn escaped(line: &[char], c: usize) -> bool {
    line[..c].iter().rev().take_while(|&&ch| ch == '\\').count() % 2 == 1
}

/// **The char at `c` is a character literal's** — `'x'` or `'\x'`: `%`
/// passes over a bracket so written, and [`quoted`] over a quote. The
/// second is escaped as well, which [`escaped`] alone does not skip: in
/// `\( '\)' \)` only this passes over the middle one.
fn literal(line: &[char], c: usize) -> bool {
    let at = |i: Option<usize>| i.and_then(|i| line.get(i)).copied();
    let closed = at(Some(c + 1)) == Some('\'');
    let plain = at(c.checked_sub(1)) == Some('\'');
    let escaped = at(c.checked_sub(1)) == Some('\\') && at(c.checked_sub(2)) == Some('\'');
    closed && (plain || escaped)
}

/// A bracket `%` matches: itself, its partner, and whether the partner is
/// after it.
fn pair_of(c: char) -> Option<(char, char, bool)> {
    Some(match c {
        '(' => ('(', ')', true),
        '[' => ('[', ']', true),
        '{' => ('{', '}', true),
        ')' => (')', '(', false),
        ']' => (']', '[', false),
        '}' => ('}', '{', false),
        _ => return None,
    })
}

/// `Some(big)` for a word motion — `w` `b` `e` (`false`), `W` `B` `E`
/// (`true`) — the motions that cross lines by the walker (`crate::word`).
fn word_motion(motion: CursorMotion) -> Option<bool> {
    use CursorMotion as M;
    match motion {
        M::WordForward | M::WordBackward | M::WordEnd => Some(false),
        M::BigWordForward | M::BigWordBackward | M::BigWordEnd => Some(true),
        _ => None,
    }
}

/// What a text object spans: chars from `start` to `end` (exclusive) — a
/// word object can reach the next line — or whole lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectSpan {
    Chars { start: word::Pos, end: word::Pos },
    Lines { top: usize, bottom: usize },
}

/// **What a text object spans** from `(row, col)`, `count` of them where
/// the object counts (a paragraph) — the one resolver Visual's
/// `v{i,a}{obj}` and an operator's `{op}{i,a}{obj}` both ask.
fn object_span(
    lines: &[Vec<char>],
    (row, col): (usize, usize),
    scope: TextObjectScope,
    object: TextObject,
    count: usize,
) -> Option<ObjectSpan> {
    let line = &lines[row];
    let chars = match object {
        TextObject::Word | TextObject::BigWord => {
            let around = scope == TextObjectScope::Around;
            let big = object == TextObject::BigWord;
            let (start, end) = word::object(lines, (row, col), count, around, big)?;
            return Some(ObjectSpan::Chars { start, end });
        }
        TextObject::Quote(q) => quote_object_range(line, col, q, scope),
        TextObject::Pair(open, close) => pair_object_range(line, col, open, close, scope),
        TextObject::Paragraph => {
            let (top, bottom) = paragraph_lines(lines, row, scope, count)?;
            return Some(ObjectSpan::Lines { top, bottom });
        }
    };
    chars.map(|(lo, hi)| ObjectSpan::Chars {
        start: (row, lo),
        end: (row, hi),
    })
}

/// **`ip` / `ap`: the lines a paragraph spans from `row`**, vim's rule
/// (`:h ip`, checked against nvim): a paragraph is a run of lines that are
/// not blank, and a blank line is empty or white space only. `ip` is the
/// run `row` is in, blank or not — `count` runs. `ap` from text is the
/// paragraph and the blank lines after it, or, where none follow, the blank
/// lines before it; from a blank line, those blank lines and the paragraph
/// after. `None` where there are not `count` of them (`v9ap`), or no
/// paragraph follows a blank `ap` — vim selects nothing then too.
fn paragraph_lines(
    lines: &[Vec<char>],
    row: usize,
    scope: TextObjectScope,
    count: usize,
) -> Option<(usize, usize)> {
    let blank = |r: usize| lines[r].iter().all(|c| c.is_whitespace());
    let last = lines.len().checked_sub(1)?;
    // The end of the run of lines like `r`, from `r` down.
    let run_end = |r: usize| {
        let kind = blank(r);
        (r..=last)
            .take_while(|&e| blank(e) == kind)
            .last()
            .unwrap_or(r)
    };
    // The start of the run of lines like `r`, from `r` up.
    let run_start = |r: usize| {
        let kind = blank(r);
        (0..=r)
            .rev()
            .take_while(|&s| blank(s) == kind)
            .last()
            .unwrap_or(r)
    };
    // The line after `r`, if there is one.
    let next = |r: usize| (r < last).then_some(r + 1);
    let mut top = run_start(row);
    let mut bottom = row;
    for unit in 0..count.max(1) {
        let from = match unit {
            0 => row,
            _ => next(bottom)?,
        };
        bottom = run_end(from);
        if scope == TextObjectScope::Inner {
            continue;
        }
        match (blank(from), next(bottom)) {
            // Blank lines and the paragraph after them.
            (true, Some(text)) => bottom = run_end(text),
            (true, None) => return None,
            // A paragraph and the blank lines after it.
            (false, Some(white)) => bottom = run_end(white),
            // None after the last: the blank lines before the first.
            (false, None) if unit + 1 == count.max(1) && top > 0 => {
                top = run_start(top - 1);
            }
            (false, None) if unit + 1 == count.max(1) => {}
            (false, None) => return None,
        }
    }
    Some((top, bottom))
}

/// `i"` / `a"` (and `'`, `` ` ``): the range between the first matching
/// pair of `q` at or after the cursor. `Inner` excludes the quotes;
/// `Around` includes them and the white space after the closing quote, or —
/// with none there — before the opening one (`:h aquote`; it took neither,
/// so `da"` left two spaces where vim leaves one: review stop 12, T6 5).
/// Single-line, as vim's.
fn quote_object_range(
    line: &[char],
    col: usize,
    q: char,
    scope: TextObjectScope,
) -> Option<(usize, usize)> {
    let quotes: Vec<usize> = line
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == q)
        .map(|(i, _)| i)
        .collect();
    // Pair them (0,1), (2,3), …; the cursor's pair is the first whose
    // close is at or after it (covers cursor before or inside the pair).
    let mut i = 0;
    while i + 1 < quotes.len() {
        let (open, close) = (quotes[i], quotes[i + 1]);
        if col <= close {
            if scope == TextObjectScope::Inner {
                return Some((open + 1, close));
            }
            let blank = |c: &&char| c.is_whitespace();
            let after = line[close + 1..].iter().take_while(blank).count();
            let before = match after {
                0 => line[..open].iter().rev().take_while(blank).count(),
                _ => 0,
            };
            return Some((open - before, close + 1 + after));
        }
        i += 2;
    }
    None
}

/// `i(` / `a(` (and the other brackets): the range between the bracket
/// pair enclosing the cursor, respecting nesting. `Inner` excludes the
/// brackets, `Around` includes them. Single-line.
fn pair_object_range(
    line: &[char],
    col: usize,
    open: char,
    close: char,
    scope: TextObjectScope,
) -> Option<(usize, usize)> {
    if line.is_empty() {
        return None;
    }
    let col = col.min(line.len() - 1);

    // Walk left for the enclosing open bracket, tracking nesting via the
    // closes seen on the way. A close *at* the cursor is the pair we want,
    // so it doesn't count toward depth.
    let mut depth = 0i32;
    let mut open_idx = None;
    for i in (0..=col).rev() {
        let c = line[i];
        if c == close && i != col {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                open_idx = Some(i);
                break;
            }
            depth -= 1;
        }
    }
    let open_idx = open_idx?;

    // Walk right from just past the open for its matching close.
    let mut depth = 0i32;
    let mut close_idx = None;
    for (j, &c) in line.iter().enumerate().skip(open_idx + 1) {
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                close_idx = Some(j);
                break;
            }
            depth -= 1;
        }
    }
    let close_idx = close_idx?;

    Some(match scope {
        TextObjectScope::Inner => (open_idx + 1, close_idx),
        TextObjectScope::Around => (open_idx, close_idx + 1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Undo is bounded by size as well as by count** — the oldest go
    /// first, the newest is always kept (review stop 16, B 5: an editable
    /// page made a snapshot the whole page).
    #[test]
    fn undo_is_bounded_by_size_and_keeps_the_newest() {
        let mut p = Prompt::new();
        p.replace_with(&"x".repeat(600_000));
        let one = p.snapshot().bytes;
        assert!(one >= 2_400_000, "premise: a large snapshot ({one})");
        for _ in 0..40 {
            let snap = p.snapshot();
            p.push_undo(snap);
        }
        let held: usize = p.undo.iter().map(|s| s.bytes).sum();
        assert!(held <= UNDO_BYTES, "{held} held");
        assert_eq!(p.undo.len(), UNDO_BYTES / one, "as many as fit");
        // One larger than the cap alone is still kept: the newest is.
        let mut big = Prompt::new();
        big.replace_with(&"y".repeat(UNDO_BYTES / 4 + 1));
        let snap = big.snapshot();
        big.push_undo(snap);
        assert_eq!(big.undo.len(), 1, "the newest kept");
    }

    #[test]
    fn cursor_from_visual_no_wrap() {
        // A tall window (top == 0): visual rows map straight to logical lines.
        let b = Buffer::from_lines(vec!["hello".to_string(), "world".to_string()]);
        assert_eq!(b.cursor_from_visual(0, 80, 100, 0, 0), (0, 0));
        assert_eq!(b.cursor_from_visual(0, 80, 100, 0, 3), (0, 3));
        // Past the end of the line clamps to its end (EOL), not the next line.
        assert_eq!(b.cursor_from_visual(0, 80, 100, 0, 99), (0, 5));
        assert_eq!(b.cursor_from_visual(0, 80, 100, 1, 2), (1, 2));
    }

    #[test]
    fn cursor_from_visual_wrapped_line() {
        // "abcdefg" at width 4 → rows "abcd" (offset 0) and "efg" (offset 4).
        let b = Buffer::from_lines(vec!["abcdefg".to_string()]);
        assert_eq!(b.cursor_from_visual(0, 4, 100, 0, 2), (0, 2)); // 'c'
        assert_eq!(b.cursor_from_visual(0, 4, 100, 1, 1), (0, 5)); // 'f' (offset 4 + 1)
    }

    #[test]
    fn cursor_from_visual_wide_chars() {
        // Cells: a(1) 中(2) b(1). A click in the right half of a wide glyph lands
        // on that glyph's char.
        let b = Buffer::from_lines(vec!["a中b".to_string()]);
        assert_eq!(b.cursor_from_visual(0, 80, 100, 0, 0), (0, 0)); // 'a'
        assert_eq!(b.cursor_from_visual(0, 80, 100, 0, 1), (0, 1)); // 中 (left cell)
        assert_eq!(b.cursor_from_visual(0, 80, 100, 0, 2), (0, 1)); // 中 (right cell)
        assert_eq!(b.cursor_from_visual(0, 80, 100, 0, 3), (0, 2)); // 'b'
    }

    #[test]
    fn cursor_from_visual_respects_scroll_top() {
        // 10 lines in a 3-row window with the cursor at the bottom → top == 7.
        let lines: Vec<String> = (0..10).map(|i| format!("line{i}")).collect();
        let mut b = Buffer::from_lines(lines);
        b.place_cursor(9, 0);
        assert_eq!(b.cursor_from_visual(0, 80, 3, 0, 0), (7, 0)); // first visible row
        assert_eq!(b.cursor_from_visual(0, 80, 3, 2, 0), (9, 0)); // last visible row
    }

    #[test]
    fn cursor_from_visual_maps_an_upper_wrapped_line_when_scrolled() {
        // Wrapped at width 4: "one"(1 row) / "abcdefgh"(2 rows) / "two"(1) /
        // "end"(1). With the cursor at the end of "end" and a 3-row window,
        // top==2, so the visible rows are the *second* half of the wrapping
        // non-cursor line "abcdefgh", then "two", then "end". A click on that
        // upper wrapped row must resolve into line 1 — the case where a
        // non-cursor line wraps AND the view is scrolled at the same time.
        let mut b = Buffer::from_lines(vec![
            "one".to_string(),
            "abcdefgh".to_string(),
            "two".to_string(),
            "end".to_string(),
        ]);
        b.place_cursor(3, 3);
        assert_eq!(b.cursor_from_visual(0, 4, 3, 0, 1), (1, 5)); // "efgh" row → 'f'
        assert_eq!(b.cursor_from_visual(0, 4, 3, 1, 0), (2, 0)); // "two"
        assert_eq!(b.cursor_from_visual(0, 4, 3, 2, 2), (3, 2)); // "end" → 'd'
    }

    #[test]
    fn cursor_from_visual_phantom_row_maps_to_end_of_line() {
        // A line that exactly fills the width has a phantom continuation row at
        // the cursor's end; a click there maps to the line's end, not past it.
        let mut b = Buffer::from_lines(vec!["abcdefgh".to_string()]);
        b.place_cursor(0, 8); // end of a line wrapping to "abcd"/"efgh" at width 4
        assert_eq!(b.cursor_from_visual(0, 4, 5, 2, 0), (0, 8)); // the phantom row → EOL
    }

    #[test]
    fn cursor_from_visual_empty_buffer_is_origin() {
        // A single empty line (an empty prompt): any click maps to (0, 0).
        let b = Buffer::from_lines(vec![String::new()]);
        assert_eq!(b.cursor_from_visual(0, 80, 100, 0, 0), (0, 0));
        assert_eq!(b.cursor_from_visual(0, 80, 100, 5, 9), (0, 0)); // out of range → clamped
    }

    /// **A `zt` past the end survives a frame, not a resize** — vim's rule
    /// (`viewport::top_ceiling`): an unchanged size keeps the window where
    /// `zt` put it, `~` below; a new size refills the last page.
    #[test]
    fn prompt_window_past_the_end_survives_a_frame_not_a_resize() {
        use crate::viewport::Place;
        let mut p = Prompt::new();
        let text: Vec<String> = (0..60).map(|i| format!("line {i}")).collect();
        p.insert_pasted(&text.join("\n"));
        p.settle(80, 10, 0);
        p.place(Place::Top, Some(58), false, (80, 10), 0);
        assert_eq!(
            p.visible_window(80, 10).top,
            57,
            "premise: past the last page (50)"
        );
        p.settle(80, 10, 0);
        assert_eq!(p.visible_window(80, 10).top, 57, "the same size keeps it");
        p.settle(80, 12, 0);
        assert_eq!(
            p.visible_window(80, 12).top,
            48,
            "a new size refills the last page"
        );
    }

    #[test]
    fn buffer_yank_reads_target_text_and_kind() {
        let mut b = Buffer::from_lines(vec!["hello world".to_string()]);
        // `yw` yanks the word + its trailing space (charwise), parking the
        // cursor at the range start.
        let (copied, kind) = b
            .yank(OpTarget::Motion(CursorMotion::WordForward), 1)
            .unwrap();
        assert_eq!(copied.text(), "hello ");
        assert_eq!(kind, SelectionKind::Char);
        assert_eq!(b.cursor(), (0, 0));
        // `yy` (whole line) is linewise — newline-terminated.
        let (copied, kind) = b.yank(OpTarget::Line, 1).unwrap();
        assert_eq!(copied.text(), "hello world\n");
        assert_eq!(kind, SelectionKind::Line);
    }

    /// Six lines, the fold over the 2nd to 4th shut — the shape every
    /// fold test here was measured on in Neovim 0.12 (`2,4fold`, `zM`).
    fn folded() -> Buffer {
        let lines = ["one", "two two", "three", "four", "five", "six"];
        let mut b = Buffer::from_lines(lines.iter().map(|l| l.to_string()).collect());
        b.set_folds(vec![(1, 4)]);
        b
    }

    /// **A shut fold is one line to `j` and `k`** (`:h fold-behavior`):
    /// onto it from either side lands on its first line, the column kept;
    /// from anywhere in it, past it. `{count}G` keeps the line it names.
    #[test]
    fn a_shut_fold_is_one_line_to_j_and_k() {
        let mut b = folded();
        b.move_cursor(CursorMotion::Down, 1);
        assert_eq!(b.cursor(), (1, 0), "j onto it: its first line");
        b.move_cursor(CursorMotion::Down, 1);
        assert_eq!(b.cursor(), (4, 0), "j again: past it");
        b.move_cursor(CursorMotion::Up, 1);
        assert_eq!(b.cursor(), (1, 0), "k from below: its first line");
        b.place_cursor(0, 2);
        b.move_cursor(CursorMotion::Down, 1);
        assert_eq!(b.cursor(), (1, 2), "the column kept");
        b.place_cursor(5, 0);
        b.move_cursor(CursorMotion::GotoLine(3), 1);
        assert_eq!(b.cursor(), (2, 0), "3G: its own line, inside");
        b.move_cursor(CursorMotion::Up, 1);
        assert_eq!(b.cursor(), (0, 0), "k from inside: above it");
        b.place_cursor(2, 0);
        b.move_cursor(CursorMotion::Down, 1);
        assert_eq!(b.cursor(), (4, 0), "j from inside: below it");
    }

    /// **A word motion leaves a shut fold by its edge** — forward from its
    /// last char, back from its first line (measured: `w` `W` `e` to line
    /// 5, `b` to line 1) — from its first line or from inside it.
    #[test]
    fn a_word_motion_leaves_a_shut_fold_by_its_edge() {
        for at in [(1, 0), (2, 3)] {
            for (motion, to) in [
                (CursorMotion::WordForward, (4, 0)),
                (CursorMotion::BigWordForward, (4, 0)),
                (CursorMotion::WordEnd, (4, 3)),
                (CursorMotion::WordBackward, (0, 0)),
            ] {
                let mut b = folded();
                b.place_cursor(at.0, at.1);
                b.move_cursor(motion, 1);
                assert_eq!(b.cursor(), to, "{motion:?} from {at:?}");
            }
        }
    }

    /// **Folds are kept in order, joined where they overlap** — two that
    /// only touch stay two, as vim's do — whatever order an owner gives.
    #[test]
    fn folds_are_kept_in_order_and_joined_where_they_overlap() {
        let mut b = folded();
        b.set_folds(vec![(4, 6), (2, 4), (1, 3)]);
        assert_eq!(b.fold_at(3), Some((1, 4)), "the two overlapping, one");
        assert_eq!(b.fold_at(4), Some((4, 6)), "touching, apart");
        b.place_cursor(0, 0);
        b.move_cursor(CursorMotion::Down, 2);
        assert_eq!(b.cursor(), (4, 0), "two lines: a fold, then the next");
        // One that holds nothing, or ends before it starts, is no fold — and
        // left in, it would hide the fold before it from the search.
        b.set_folds(vec![(4, 2), (1, 3), (5, 5)]);
        assert_eq!(b.fold_at(2), Some((1, 3)));
        assert_eq!((b.fold_at(4), b.fold_at(5)), (None, None));
    }

    /// **An operator takes a shut fold whole** — linewise and charwise, from
    /// its first line or inside it, a count counting it one line (each as
    /// measured in Neovim). A charwise selection into it is not widened.
    #[test]
    fn an_operator_takes_a_shut_fold_whole() {
        let fold = "two two\nthree\nfour";
        let yank = |at: (usize, usize), target, count| {
            let mut b = folded();
            b.place_cursor(at.0, at.1);
            let (copied, kind) = b.yank(target, count).expect("something");
            (copied.text(), kind)
        };
        use SelectionKind::{Char, Line};
        assert_eq!(yank((1, 0), OpTarget::Line, 1), (format!("{fold}\n"), Line));
        assert_eq!(yank((2, 0), OpTarget::Line, 1), (format!("{fold}\n"), Line));
        assert_eq!(
            yank((1, 0), OpTarget::Line, 2),
            (format!("{fold}\nfive\n"), Line),
            "2yy: the fold and the line after"
        );
        let right = OpTarget::Motion(CursorMotion::Right);
        assert_eq!(yank((1, 0), right, 1), (fold.to_string(), Char));
        assert_eq!(
            yank((2, 2), right, 1),
            (fold.to_string(), Char),
            "from inside it: from its start"
        );
        let word = OpTarget::Motion(CursorMotion::WordForward);
        assert_eq!(yank((1, 0), word, 1), (fold.to_string(), Char));
        let down = OpTarget::LineMotion(CursorMotion::Down);
        assert_eq!(yank((0, 0), down, 1), (format!("one\n{fold}\n"), Line));
        let mut b = folded();
        b.place_cursor(1, 0);
        b.set_anchor(SelectionKind::Line);
        let (copied, _) = b.yank(OpTarget::Selection, 1).unwrap();
        assert_eq!(copied.text(), format!("{fold}\n"), "V: whole");
        let mut b = folded();
        b.place_cursor(0, 1);
        b.set_anchor(SelectionKind::Char);
        b.move_cursor(CursorMotion::Down, 1);
        let (copied, _) = b.yank(OpTarget::Selection, 1).unwrap();
        assert_eq!(copied.text(), "ne\ntw", "v: to the cursor (measured `vjy`)");
        assert_eq!(
            yank((5, 0), OpTarget::Line, usize::MAX),
            ("six\n".to_string(), Line),
            "a count past the end: the last line, at once"
        );
    }

    fn section(forward: bool, count: usize) -> CursorMotion {
        CursorMotion::Section {
            forward,
            end: false,
            count,
        }
    }

    /// `][` / `[]`.
    fn section_end(forward: bool, count: usize) -> CursorMotion {
        CursorMotion::Section {
            forward,
            end: true,
            count,
        }
    }

    fn buffer_at(lines: &[&str], (row, col): (usize, usize)) -> Buffer {
        let mut b = Buffer::from_lines(lines.iter().map(|l| l.to_string()).collect());
        b.place_cursor(row, col);
        b
    }

    /// **The read-only door keeps the column too** — the transcript's and
    /// every page's cursor (`apply_read_only`), as the prompt's does (the
    /// table in `app/tests/vim_fidelity.rs`): `j` through a short line and
    /// back, a failed `l` keeping it, `$` aiming at every end, `v` keeping it
    /// and `Esc` resetting it (Q5).
    #[test]
    fn a_read_only_cursor_comes_back_to_its_column() {
        use CursorMotion::{Down, LineEnd, Right, Up};
        let down = |b: &mut Buffer| b.apply_read_only(EditAction::Move(Down), 1);
        let mut b = buffer_at(&["abcdefghij", "abc", "abcdefghij"], (0, 7));
        down(&mut b);
        assert_eq!(b.cursor(), (1, 2), "the short line's last char");
        b.apply_read_only(EditAction::Move(Right), 1);
        assert_eq!(b.cursor(), (1, 2), "premise: `l` fails there");
        down(&mut b);
        assert_eq!(b.cursor(), (2, 7), "back, past the failed `l`");
        b.apply_read_only(EditAction::Move(Up), 1);
        b.apply_read_only(EditAction::SetAnchor, 1);
        down(&mut b);
        assert_eq!(b.cursor(), (2, 7), "`v` keeps it");
        b.apply_read_only(EditAction::Move(Up), 1);
        b.apply_read_only(EditAction::ClearAnchor, 1);
        down(&mut b);
        assert_eq!(b.cursor(), (2, 2), "`Esc` set it where it was");

        let mut b = buffer_at(&["abcdef", "ab", "abcdefghij"], (0, 0));
        b.apply_read_only(EditAction::Move(LineEnd), 1);
        down(&mut b);
        down(&mut b);
        assert_eq!(b.cursor(), (2, 9), "`$`: every line's end");

        // A move that reaches no reset — here a re-sync shrinking the line
        // under the cursor — leaves no stale aim: from where the cursor is.
        let mut b = buffer_at(&["abcdefghij", "abc", "abcdefghij"], (0, 7));
        down(&mut b);
        b.set_lines(["abcdefghij", "a", "abcdefghij"].map(String::from).to_vec());
        b.clamp_cursor_to_char();
        down(&mut b);
        assert_eq!(b.cursor(), (2, 0), "aimed from the cursor, not from 7");
    }

    /// Sections at lines 2 and 5 (`{` in the first column).
    const SECTIONS: &[&str] = &["a1", "b2", "{c", "d4", "  e5", "{f", "g7"];
    /// A C function, for vim's Vi quirk: an operator's `]]` also stops at `}`.
    const C: &[&str] = &["int f()", "{", "  x;", "}", "", "int g()", "{", "}"];

    /// One measured motion: the lines, the cursor, forward, the count, where
    /// it lands.
    type MotionCase = (
        &'static [&'static str],
        (usize, usize),
        bool,
        usize,
        (usize, usize),
    );
    /// One measured operator: the lines, the cursor, forward, the count, the
    /// span yanked and its kind — `None` when nothing is.
    type SpanCase = (
        &'static [&'static str],
        (usize, usize),
        bool,
        usize,
        Option<(&'static str, SelectionKind)>,
    );

    /// **`]]` and `[[` land where vim's do** — every row measured against
    /// nvim 0.12.6 headless (`normal! ]]` from the cursor given), 0-based:
    /// onto the first non-blank (an all-blank line's last char); the buffer's
    /// edge when no section is left; a count the sections run out before
    /// moving nothing at all.
    #[test]
    fn sections_move_as_vims_do() {
        #[rustfmt::skip]
        let cases: &[MotionCase] = &[
            (SECTIONS, (0, 0), true, 1, (2, 0)),
            (SECTIONS, (0, 1), true, 1, (2, 0)),
            (SECTIONS, (0, 0), true, 2, (5, 0)),
            (SECTIONS, (0, 0), true, 3, (6, 0)), // the last step may stop at the end
            (SECTIONS, (5, 0), true, 1, (6, 0)),
            (SECTIONS, (6, 1), true, 1, (6, 0)), // on the last line: its start
            (SECTIONS, (6, 1), false, 1, (5, 0)),
            (SECTIONS, (2, 0), false, 1, (0, 0)),
            (SECTIONS, (3, 1), false, 1, (2, 0)),
            (SECTIONS, (6, 1), false, 2, (2, 0)),
            (SECTIONS, (0, 1), false, 1, (0, 0)),
            (SECTIONS, (5, 0), true, 2, (5, 0)), // runs out: nothing moves
            (SECTIONS, (2, 0), false, 2, (2, 0)),
            (SECTIONS, (0, 0), true, 9, (0, 0)),
            (SECTIONS, (6, 1), false, 9, (6, 1)),
            (&["one", "two", "   three"], (0, 1), true, 1, (2, 3)),
            (&["   one", "two"], (1, 1), false, 1, (0, 3)),
            (&["abc", "def", "    "], (0, 1), true, 1, (2, 3)), // all blank: its last
            (&["x", "", "{", "", "y"], (0, 0), true, 1, (2, 0)),
            (C, (2, 0), true, 1, (6, 0)), // no quirk without an operator
        ];
        for &(lines, from, forward, count, to) in cases {
            let mut b = buffer_at(lines, from);
            b.move_cursor(section(forward, count), 1);
            let keys = format!("{count}{}", if forward { "]]" } else { "[[" });
            assert_eq!(b.cursor(), to, "{keys} from {from:?} in {lines:?}");
        }
    }

    /// **An operator over a section is exclusive, as vim's is** — the spans
    /// nvim 0.12.6 yanked from the same cursors (`normal! y]]`): linewise
    /// when it starts in the line's indent and ends at a line's start
    /// (`:h exclusive-linewise`), else charwise to the end of the line
    /// before; the last section through the end of the buffer; vim's quirk
    /// for `}`; nothing when the count runs out.
    #[test]
    fn an_operator_over_a_section_is_exclusive_as_vims_is() {
        use SelectionKind::{Char, Line};
        #[rustfmt::skip]
        let cases: &[SpanCase] = &[
            (SECTIONS, (0, 0), true, 1, Some(("a1\nb2\n", Line))),
            (SECTIONS, (0, 1), true, 1, Some(("1\nb2", Char))),
            (SECTIONS, (5, 0), true, 1, Some(("{f\ng7", Char))),
            (SECTIONS, (6, 0), false, 1, Some(("{f\n", Line))),
            (SECTIONS, (6, 1), false, 1, Some(("{f\ng", Char))),
            (SECTIONS, (3, 1), false, 1, Some(("{c\nd", Char))),
            (SECTIONS, (0, 0), true, 9, None),
            (&["one", "two", "   three"], (0, 0), true, 1, Some(("one\ntwo\n   three", Char))),
            (&["one", "two", "   three"], (1, 1), true, 1, Some(("wo\n   three", Char))),
            (&["   one", "two"], (1, 1), false, 1, Some(("   one\nt", Char))),
            (&["x", "", "{", "", "y"], (0, 0), true, 1, Some(("x\n\n", Line))),
            (&["x", "", "{", "", "y"], (3, 0), false, 1, Some(("{\n", Line))),
            (C, (2, 0), true, 1, Some(("  x;\n}\n", Line))), // to the `}`, and it
            (C, (6, 0), true, 1, Some(("{\n", Line))), // a `}` on the last line clamps
            (C, (2, 0), false, 1, Some(("{\n", Line))),
        ];
        for &(lines, from, forward, count, want) in cases {
            let mut b = buffer_at(lines, from);
            let got = b
                .yank(OpTarget::Motion(section(forward, count)), 1)
                .map(|(copied, kind)| (copied.text().to_string(), kind));
            let keys = format!("y{count}{}", if forward { "]]" } else { "[[" });
            assert_eq!(
                got,
                want.map(|(text, kind)| (text.to_string(), kind)),
                "{keys} from {from:?} in {lines:?}"
            );
        }
        // `2y]]` is `y2]]`: the operator's count multiplies the motion's.
        let starts: Vec<String> = (0..6).map(|i| format!("{{{i}")).collect();
        let starts: Vec<&str> = starts.iter().map(String::as_str).collect();
        let mut b = buffer_at(&starts, (0, 0));
        let (copied, _) = b.yank(OpTarget::Motion(section(true, 1)), 2).unwrap();
        assert_eq!(copied.text(), "{0\n{1\n");
    }

    /// **`][` and `[]` land where vim's do** — the same oracle, `}` in the
    /// first column at lines 2 and 5: as `]]`, but never the last line's last
    /// char (`y][` stops short of it), and with no `}` quirk.
    #[test]
    fn section_ends_move_as_vims_do() {
        const ENDS: &[&str] = &["a1", "b2", "}c", "d4", "  e5", "}f", "g7"];
        #[rustfmt::skip]
        let moves: &[MotionCase] = &[
            (ENDS, (0, 0), true, 1, (2, 0)),
            (ENDS, (0, 1), true, 1, (2, 0)),
            (ENDS, (0, 0), true, 2, (5, 0)),
            (ENDS, (0, 0), true, 3, (6, 0)),
            (ENDS, (0, 0), true, 4, (0, 0)), // runs out: nothing moves
            (ENDS, (5, 0), true, 1, (6, 0)),
            (ENDS, (6, 1), true, 1, (6, 0)),
            (ENDS, (6, 1), false, 1, (5, 0)),
            (ENDS, (2, 0), false, 1, (0, 0)),
            (ENDS, (3, 1), false, 2, (0, 0)),
            (ENDS, (3, 1), false, 3, (3, 1)),
            (ENDS, (0, 1), false, 1, (0, 0)),
            (&["x", "{", "    ", "y"], (0, 0), true, 1, (3, 0)), // `{` is no end
            (&["x", "{", "}", "y"], (0, 0), true, 1, (2, 0)),
        ];
        for &(lines, from, forward, count, to) in moves {
            let mut b = buffer_at(lines, from);
            b.move_cursor(section_end(forward, count), 1);
            let keys = format!("{count}{}", if forward { "][" } else { "[]" });
            assert_eq!(b.cursor(), to, "{keys} from {from:?} in {lines:?}");
        }
        use SelectionKind::{Char, Line};
        #[rustfmt::skip]
        let spans: &[SpanCase] = &[
            (ENDS, (0, 0), true, 1, Some(("a1\nb2\n", Line))),
            (ENDS, (0, 1), true, 1, Some(("1\nb2", Char))),
            (ENDS, (5, 0), true, 1, Some(("}f\n", Line))), // not the last line
            (ENDS, (6, 0), false, 1, Some(("}f\n", Line))),
            (ENDS, (6, 1), false, 1, Some(("}f\ng", Char))),
        ];
        for &(lines, from, forward, count, want) in spans {
            let mut b = buffer_at(lines, from);
            let got = b
                .yank(OpTarget::Motion(section_end(forward, count)), 1)
                .map(|(copied, kind)| (copied.text().to_string(), kind));
            let keys = format!("y{count}{}", if forward { "][" } else { "[]" });
            assert_eq!(
                got,
                want.map(|(text, kind)| (text.to_string(), kind)),
                "{keys} from {from:?} in {lines:?}"
            );
        }
    }

    /// **A buffer told its sections moves between those** — a transcript's
    /// turns, their starts and their ends — and a stale list past the end is
    /// never reached.
    #[test]
    fn a_buffer_told_its_sections_moves_between_them() {
        let lines = [
            "",
            "> one",
            "reply",
            "{ not a section }",
            "",
            "> two",
            "more",
        ];
        let mut b = buffer_at(&lines, (0, 0));
        b.set_sections(Sections::At {
            starts: vec![0, 4, 99],
            ends: vec![2, 6, 99],
        });
        b.move_cursor(section(true, 1), 1);
        assert_eq!(b.cursor(), (4, 0), "the `{{` line is not one here");
        b.move_cursor(section(true, 1), 1);
        assert_eq!(b.cursor(), (6, 0), "none left: the last line");
        b.move_cursor(section(false, 1), 1);
        assert_eq!(b.cursor(), (4, 0));
        b.move_cursor(section_end(false, 1), 1);
        assert_eq!(b.cursor(), (2, 0), "back to the first turn's end");
        b.move_cursor(section_end(true, 1), 1);
        assert_eq!(b.cursor(), (6, 0), "on to the next");
    }

    #[test]
    fn text_and_replace_with_roundtrip() {
        let mut p = Prompt::new();
        type_str(&mut p, "hello");
        p.apply(EditAction::Newline);
        type_str(&mut p, "world");
        assert_eq!(p.text(), "hello\nworld"); // non-destructive read
        // `$EDITOR` returns new content → replace as one undo group, cursor at end.
        p.replace_with("replaced\ntext\nhere");
        assert_eq!(p.text(), "replaced\ntext\nhere");
        assert_eq!(p.cursor(), (2, 4));
        p.apply(EditAction::Undo);
        assert_eq!(p.text(), "hello\nworld");
    }

    #[test]
    fn set_lines_clamps_selection_anchor_into_shrunk_content() {
        let mut b = Buffer::from_lines(vec!["hello world".to_string()]);
        b.move_cursor(CursorMotion::LineEnd, 1);
        b.set_anchor(SelectionKind::Char); // anchor near the end
        // Re-sync to shorter content: the anchor's column must clamp into the
        // new (shorter) row, not point past its end.
        b.set_lines(vec!["hi".to_string()]);
        let (r, c) = b.anchor().expect("anchor survives a surviving row");
        assert_eq!(r, 0);
        assert!(c <= 2, "anchor column {c} must clamp into the 2-char row");
        // An anchor whose row is gone is CLAMPED onto the last row — the
        // keymap is still in Visual, so the range must stay.
        let mut b = Buffer::from_lines(vec!["a".into(), "b".into(), "cde".into()]);
        b.move_cursor(CursorMotion::BufferEnd, 1);
        b.move_cursor(CursorMotion::LineEnd, 1);
        b.set_anchor(SelectionKind::Char);
        b.set_lines(vec!["x".into()]);
        assert_eq!(b.anchor(), Some((0, 1)));
        b.set_lines(vec![]); // → one empty line
        assert_eq!(b.anchor(), Some((0, 0)));
    }

    /// **`/` and `?` over a buffer**: forward past the cursor, back before
    /// it, round an end (said), a count, and a lone match under the cursor
    /// found again only by going round — vim's `'wrapscan'`. Columns are
    /// chars, past a multibyte one.
    #[test]
    fn search_walks_matches_and_wraps_round_the_ends() {
        let re = regex::Regex::new("ab").unwrap();
        let mut b = Buffer::from_lines(vec!["ab x ab".into(), "—— ab".into(), "none".into()]);
        assert_eq!(b.search(&re, true, 1), Some(false));
        assert_eq!(b.cursor(), (0, 5), "past the cursor's own match");
        b.search(&re, true, 1);
        assert_eq!(b.cursor(), (1, 3), "chars, past the dashes");
        assert_eq!(b.search(&re, true, 1), Some(true), "round the bottom");
        assert_eq!(b.cursor(), (0, 0));
        assert_eq!(b.search(&re, false, 1), Some(true), "round the top");
        assert_eq!(b.cursor(), (1, 3));
        assert_eq!(b.search(&re, false, 2), Some(false));
        assert_eq!(b.cursor(), (0, 0), "a count");
        let lone = regex::Regex::new("x").unwrap();
        b.place_cursor(0, 3);
        assert_eq!(b.search(&lone, true, 1), Some(true), "itself, round");
        assert_eq!(b.cursor(), (0, 3));
        let missing = regex::Regex::new("zz").unwrap();
        assert_eq!(b.search(&missing, false, 1), None);
        assert_eq!(b.cursor(), (0, 3), "a miss moves nothing");
    }

    /// **`*`'s word**: the keyword under or after the cursor, the cursor put
    /// on its start; else a run of non-blanks; none in blanks.
    #[test]
    fn the_word_a_star_takes() {
        let mut b = Buffer::from_lines(vec!["  --flag a_b  ".into()]);
        b.place_cursor(0, 9);
        assert_eq!(b.cursor_to_word(), Some(("a_b".into(), true)));
        assert_eq!(b.cursor(), (0, 9));
        b.place_cursor(0, 0);
        assert_eq!(b.cursor_to_word(), Some(("flag".into(), true)), "after it");
        assert_eq!(b.cursor(), (0, 4));
        let mut dashes = Buffer::from_lines(vec!["x -- ".into()]);
        dashes.place_cursor(0, 2);
        assert_eq!(dashes.cursor_to_word(), Some(("--".into(), false)));
        dashes.place_cursor(0, 4);
        assert_eq!(dashes.cursor_to_word(), None);
    }

    #[test]
    fn chat_buffer_select_and_render_does_not_panic() {
        // Mimics the chat flow: build from transcript lines, anchor a
        // selection, extend it, render at various sizes.
        for content in [
            vec![],
            vec![String::new()],
            vec![
                "> hi".to_string(),
                String::new(),
                "Hello! I'm Claude.".to_string(),
            ],
            vec!["中文 test".to_string()],
        ] {
            let mut b = Buffer::from_lines(content);
            b.clamp_cursor_to_char();
            b.set_anchor(SelectionKind::Char);
            b.move_cursor(CursorMotion::WordForward, 1);
            b.move_cursor(CursorMotion::Down, 1);
            for (w, h) in [(20usize, 5usize), (3, 2), (1, 1), (0, 0), (80, 1)] {
                let _ = b.visible_window(0, w, h);
            }
        }
    }

    fn type_str(p: &mut Prompt, s: &str) {
        for c in s.chars() {
            p.apply(EditAction::Insert(c));
        }
    }

    #[test]
    fn new_prompt_starts_empty_with_cursor_at_origin() {
        let p = Prompt::new();
        assert_eq!(p.cursor(), (0, 0));
        assert_eq!(p.line_count(), 1);
        assert_eq!(p.lines().collect::<Vec<_>>(), vec![String::new()]);
    }

    #[test]
    fn take_returns_buffer_and_clears() {
        let mut p = Prompt::new();
        type_str(&mut p, "hello");
        assert_eq!(p.take(), "hello".to_string());
        assert_eq!(p.cursor(), (0, 0));
        assert_eq!(p.line_count(), 1);
        assert!(p.lines().next().unwrap().is_empty());
    }

    #[test]
    fn newline_action_splits_line() {
        let mut p = Prompt::new();
        type_str(&mut p, "ab");
        p.apply(EditAction::Newline);
        type_str(&mut p, "cd");
        assert_eq!(p.cursor(), (1, 2));
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["ab".to_string(), "cd".to_string()]
        );
    }

    #[test]
    fn take_joins_lines_with_newlines() {
        let mut p = Prompt::new();
        type_str(&mut p, "one");
        p.apply(EditAction::Newline);
        type_str(&mut p, "two");
        assert_eq!(p.take(), "one\ntwo".to_string());
    }

    #[test]
    fn delete_to_line_start_keeps_text_right_of_cursor() {
        let mut p = Prompt::new();
        type_str(&mut p, "abcdef");
        p.apply(EditAction::Move(CursorMotion::Left));
        p.apply(EditAction::Move(CursorMotion::Left)); // cursor before 'e' (col 4)
        p.apply(EditAction::DeleteToLineStart);
        assert_eq!(p.lines().next().unwrap(), "ef");
        assert_eq!(p.cursor(), (0, 0));
    }

    #[test]
    fn word_motions_navigate_within_line() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo bar baz");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::Move(CursorMotion::WordForward));
        assert_eq!(p.cursor(), (0, 4), "w -> start of 'bar'");
        p.apply(EditAction::Move(CursorMotion::WordForward));
        assert_eq!(p.cursor(), (0, 8), "w -> start of 'baz'");
        p.apply(EditAction::Move(CursorMotion::WordEnd));
        assert_eq!(p.cursor(), (0, 10), "e -> last char of 'baz'");
        p.apply(EditAction::Move(CursorMotion::WordBackward));
        assert_eq!(p.cursor(), (0, 8), "b -> back to start of 'baz'");
    }

    /// **A word is vim's, a WORD too** — every column here is headless
    /// nvim's answer on the same line (Steven, 10-07: `w` crossed a whole
    /// path; there was no `W`): `w` stops at each `/`, `-`, `.` and `_`-
    /// joined name; `W` crosses the path whole; `iw`/`iW`, `cw`/`cW` too.
    #[test]
    fn a_word_and_a_word_are_vims() {
        let line = "ls /usr/lib-x/a_b.rs  foo.bar";
        let visits = |motion: CursorMotion, from: usize| {
            let mut b = Buffer::from_lines(vec![line.to_string()]);
            b.place_cursor(0, from);
            (0..9)
                .map(|_| {
                    b.move_cursor(motion, 1);
                    b.cursor().1.min(28)
                })
                .collect::<Vec<_>>()
        };
        use CursorMotion::*;
        assert_eq!(visits(WordForward, 0), [3, 4, 7, 8, 11, 12, 13, 14, 17]);
        assert_eq!(
            visits(BigWordForward, 0),
            [3, 22, 28, 28, 28, 28, 28, 28, 28]
        );
        assert_eq!(visits(WordEnd, 0), [1, 3, 6, 7, 10, 11, 12, 13, 16]);
        assert_eq!(visits(BigWordEnd, 0), [1, 19, 28, 28, 28, 28, 28, 28, 28]);
        assert_eq!(
            visits(WordBackward, 28),
            [26, 25, 22, 18, 17, 14, 13, 12, 11]
        );
        assert_eq!(visits(BigWordBackward, 28), [22, 3, 0, 0, 0, 0, 0, 0, 0]);

        let chars: Vec<Vec<char>> = vec![line.chars().collect()];
        let inner = TextObjectScope::Inner;
        let chars_of = |row, lo, hi| {
            Some(ObjectSpan::Chars {
                start: (row, lo),
                end: (row, hi),
            })
        };
        assert_eq!(
            object_span(&chars, (0, 12), inner, TextObject::Word, 1),
            chars_of(0, 12, 13)
        );
        assert_eq!(
            object_span(&chars, (0, 12), inner, TextObject::BigWord, 1),
            chars_of(0, 3, 20)
        );
        let change = |motion: CursorMotion| {
            let mut b = Buffer::from_lines(vec![line.to_string()]);
            b.place_cursor(0, 8);
            match b.resolve(Operator::Change, OpTarget::Motion(motion), 1) {
                Some(RangeShape::Chars { start, end }) => (start.1, end.1),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(change(WordForward), (8, 11), "cw: `lib`");
        assert_eq!(change(BigWordForward), (8, 20), "cW: `lib-x/a_b.rs`");
        let mut b = Buffer::from_lines(vec![line.to_string()]);
        b.place_cursor(0, 8);
        match b.resolve(Operator::Delete, OpTarget::Motion(BigWordEnd), 1) {
            Some(RangeShape::Chars { start, end }) => {
                assert_eq!((start.1, end.1), (8, 20), "dE takes the WORD's last char")
            }
            other => panic!("{other:?}"),
        }
    }

    /// **A forward word motion that runs out of buffer lands on its last
    /// char, not past it** — vim's `adjust_cursor`, in the buffer itself, so
    /// a surface the Normal clamp does not reach draws the cursor where
    /// vim's is.
    #[test]
    fn a_word_motion_that_runs_out_lands_on_the_last_char() {
        let landing = |lines: &[&str], from: (usize, usize), motion| {
            let mut b = buffer_at(lines, from);
            b.move_cursor(motion, 1);
            b.cursor()
        };
        use CursorMotion::{WordEnd, WordForward};
        assert_eq!(landing(&["foo bar"], (0, 4), WordForward), (0, 6));
        assert_eq!(
            landing(&["foo "], (0, 2), WordEnd),
            (0, 3),
            "a trailing space"
        );
        assert_eq!(
            landing(&["foo"], (0, 2), WordEnd),
            (0, 2),
            "on the last char: stays"
        );
        assert_eq!(
            landing(&["foo", ""], (0, 2), WordEnd),
            (1, 0),
            "an empty last line"
        );
    }

    #[test]
    fn word_end_crosses_lines_and_skips_blanks() {
        // `e` from the end of a word with nothing left on the line jumps to the
        // end of the next line's word (skipping the newline).
        let mut b = Buffer::from_lines(vec!["foo".to_string(), "bar".to_string()]);
        b.place_cursor(0, 2);
        b.move_cursor(CursorMotion::WordEnd, 1);
        assert_eq!(b.cursor(), (1, 2), "e crosses to end of next line's word");

        // A blank line *between* words is skipped (oracle-confirmed).
        let mut b = Buffer::from_lines(vec!["foo".to_string(), String::new(), "bar".to_string()]);
        b.place_cursor(0, 2);
        b.move_cursor(CursorMotion::WordEnd, 1);
        assert_eq!(b.cursor(), (2, 2), "e skips a blank line between words");
    }

    #[test]
    fn word_end_dead_ends_on_the_furthest_reachable_cell() {
        // No word ahead, only a trailing blank line → land on it at column 0
        // (oracle-confirmed: the cursor jumps to the new line).
        let mut b = Buffer::from_lines(vec!["foo".to_string(), String::new()]);
        b.place_cursor(0, 2);
        b.move_cursor(CursorMotion::WordEnd, 1);
        assert_eq!(b.cursor(), (1, 0), "e lands on a trailing blank line");

        // No word ahead, only trailing spaces → land on the last space
        // (oracle-confirmed: goes to last space).
        let mut b = Buffer::from_lines(vec!["foo  ".to_string()]);
        b.place_cursor(0, 2);
        b.move_cursor(CursorMotion::WordEnd, 1);
        assert_eq!(b.cursor(), (0, 4), "e lands on the last trailing space");

        // Already on the buffer's last char → no-op.
        let mut b = Buffer::from_lines(vec!["foo".to_string()]);
        b.place_cursor(0, 2);
        b.move_cursor(CursorMotion::WordEnd, 1);
        assert_eq!(b.cursor(), (0, 2), "e is a no-op at the buffer's last char");
    }

    #[test]
    fn apply_counted_repeats_motion() {
        let mut p = Prompt::new();
        type_str(&mut p, "0123456789");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_counted(EditAction::Move(CursorMotion::Right), 4);
        assert_eq!(p.cursor(), (0, 4), "3l-style repeat moves 4 cells");
    }

    #[test]
    fn apply_counted_delete_is_one_undo_group() {
        let mut p = Prompt::new();
        type_str(&mut p, "abcdef");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        // The loop's grouping, with an edit that repeats: no key sends a
        // counted `DeleteForward` (`3x` is `d3l`, an operator), but `3p`
        // reaches this same group (review stop 12, T6 12: it said `3x`).
        p.apply_counted(EditAction::DeleteForward, 3);
        assert_eq!(p.lines().next().unwrap(), "def");
        // A single undo restores all three, not one at a time.
        p.apply(EditAction::Undo);
        assert_eq!(
            p.lines().next().unwrap(),
            "abcdef",
            "counted delete undoes as one step"
        );
    }

    #[test]
    fn apply_counted_overshoot_matches_exact_count() {
        // The fixed-point early-break must not change results: a count
        // past the boundary lands exactly where "just enough" would.
        let mut over = Prompt::new();
        type_str(&mut over, "abc");
        over.apply(EditAction::Move(CursorMotion::LineStart));
        over.apply_counted(EditAction::Move(CursorMotion::Right), 999); // overshoot
        assert_eq!(over.cursor(), (0, 3), "stops at end of line, not beyond");

        let mut del = Prompt::new();
        type_str(&mut del, "abc");
        del.apply(EditAction::Move(CursorMotion::LineStart));
        del.apply_counted(EditAction::DeleteForward, 999); // overshoot
        assert_eq!(
            del.lines().next().unwrap(),
            "",
            "clears the line, then stops"
        );
    }

    #[test]
    fn operator_overshoot_count_stops_at_boundary() {
        // Same guarantee through the operator/motion_target path.
        let mut p = Prompt::new();
        type_str(&mut p, "one two three");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_operator(
            Operator::Delete,
            OpTarget::Motion(CursorMotion::WordForward),
            999,
        );
        assert_eq!(
            p.lines().next().unwrap(),
            "",
            "dw past the last word clears to EOL"
        );
    }

    /// A pathological count is **O(distance), not O(count)** — in all three
    /// loops that take one.
    ///
    /// The two tests above already prove the fixed-point break does not change
    /// *results*; both use a count of 999, which runs fine either way. What
    /// nothing tested was the thing the guard is actually for: that a count you
    /// can type in four keystrokes cannot freeze the UI. Reviewers
    /// mutation-verified that deleting all three breaks passed the whole suite.
    ///
    /// This is a **responsiveness** guard, so the assertion is wall-clock. The
    /// numbers are measured, not guessed: with the breaks every case below
    /// saturates on its FIRST step, so all three calls together are O(1) and
    /// the test runs in 0.00s. Without them one loop alone costs ~16ns per
    /// iteration in debug — measured at ~4s for this count, four times the
    /// ceiling. (The first draft of this test used 30M and a 2s ceiling and
    /// **passed under mutation at 0.48s**: a timing test whose margin is
    /// assumed rather than measured is just a slow way to assert nothing.)
    #[test]
    fn a_pathological_count_is_bounded_by_distance_not_by_the_count() {
        // Big enough that an unguarded loop is unmistakable, and irrelevant to
        // a guarded one: every case below saturates on its FIRST step.
        const ABSURD: usize = 500_000_000;
        let ceiling = std::time::Duration::from_secs(1);

        // 1. `move_cursor` — the chat pane's navigation.
        let start = std::time::Instant::now();
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.buf.move_cursor(CursorMotion::Left, ABSURD); // already at column 0
        assert_eq!(p.cursor(), (0, 0));

        // 2. `apply_counted` — the counted-edit loop (`99999p`, `99999w`; an
        //    edit that saturates at once here, which no key sends counted).
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        p.apply(EditAction::Move(CursorMotion::LineEnd));
        p.apply_counted(EditAction::DeleteForward, ABSURD); // nothing after the cursor

        // 3. `motion_target` — the operator-range probe, which runs the same
        //    motion loop without moving the cursor.
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_operator(
            Operator::Delete,
            OpTarget::Motion(CursorMotion::Left),
            ABSURD,
        );
        assert_eq!(p.lines().next().unwrap(), "abc", "nothing to the left");

        let took = start.elapsed();
        assert!(
            took < ceiling,
            "{ABSURD} repeats took {took:?} — a counted command is looping per-count \
             instead of stopping at the boundary fixed point"
        );
    }

    #[test]
    fn apply_counted_clamps_zero_to_one() {
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_counted(EditAction::Move(CursorMotion::Right), 0);
        assert_eq!(p.cursor(), (0, 1), "count 0 behaves as 1");
    }

    #[test]
    fn word_end_at_or_past_line_end_does_not_move_backward() {
        // Regression (Reviewer A+B): with the cursor past the last char
        // (Insert's caret; Normal clamps it back, but the motion must not
        // assume it did), `e` must not jump backward.
        let mut p = Prompt::new();
        type_str(&mut p, "x"); // cursor at col 1 == len (past end)
        p.apply(EditAction::Move(CursorMotion::WordEnd));
        assert_eq!(p.cursor(), (0, 1), "e stays; no backward jump");
    }

    #[test]
    fn de_at_end_of_line_is_a_noop() {
        // The operator path for the same bug: `de` with no word ahead must
        // delete nothing, not the final char via a backward range.
        let mut p = Prompt::new();
        type_str(&mut p, "foo"); // cursor at col 3 == len
        p.apply_operator(Operator::Delete, OpTarget::Motion(CursorMotion::WordEnd), 1);
        assert_eq!(
            p.lines().next().unwrap(),
            "foo",
            "de at EOL changes nothing"
        );
    }

    #[test]
    fn move_buffer_start_jumps_to_first_line() {
        let mut p = Prompt::new();
        type_str(&mut p, "aa");
        p.apply(EditAction::Newline);
        type_str(&mut p, "bbb"); // cursor at (1, 3)
        p.apply(EditAction::Move(CursorMotion::BufferStart));
        assert_eq!(p.cursor().0, 0, "gg -> first line");
        assert_eq!(p.cursor().1, 2, "column clamped to first line length");
    }

    #[test]
    fn move_buffer_end_jumps_to_last_line() {
        let mut p = Prompt::new();
        type_str(&mut p, "a");
        p.apply(EditAction::Newline);
        type_str(&mut p, "bb");
        p.apply(EditAction::Newline);
        type_str(&mut p, "ccc");
        p.apply(EditAction::Move(CursorMotion::Up));
        p.apply(EditAction::Move(CursorMotion::Up));
        assert_eq!(p.cursor().0, 0);
        p.apply(EditAction::Move(CursorMotion::BufferEnd));
        assert_eq!(p.cursor().0, 2, "G -> last line");
    }

    #[test]
    fn goto_line_jumps_to_the_absolute_line() {
        let mut p = Prompt::new();
        for (i, s) in ["a", "b", "c", "d"].iter().enumerate() {
            type_str(&mut p, s);
            if i < 3 {
                p.apply(EditAction::Newline);
            }
        }
        p.apply(EditAction::Move(CursorMotion::GotoLine(2)));
        assert_eq!(p.cursor().0, 1, "line 2 → row 1 (1-indexed)");
        p.apply(EditAction::Move(CursorMotion::GotoLine(99)));
        assert_eq!(p.cursor().0, 3, "past the end clamps to the last line");
        p.apply(EditAction::Move(CursorMotion::GotoLine(1)));
        assert_eq!(p.cursor().0, 0, "line 1 → row 0");
    }

    #[test]
    fn operator_to_goto_line_is_linewise() {
        // `d3G` from the first line deletes rows 1-3 (a/b/c), leaving "d".
        let mut p = four_lines(0);
        p.apply_operator(
            Operator::Delete,
            OpTarget::LineMotion(CursorMotion::GotoLine(3)),
            1,
        );
        assert_eq!(p.lines().collect::<Vec<_>>(), vec!["d".to_string()]);
        assert_eq!(p.cursor(), (0, 0));
    }

    #[test]
    fn open_line_below_and_above_insert_empty_lines() {
        let mut p = Prompt::new();
        type_str(&mut p, "one");
        p.apply(EditAction::OpenLineBelow);
        assert_eq!(p.cursor(), (1, 0));
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["one".to_string(), String::new()]
        );
        type_str(&mut p, "two"); // cursor (1, 3)
        p.apply(EditAction::OpenLineAbove);
        // New empty line takes row 1; "two" pushed to row 2; cursor on it.
        assert_eq!(p.cursor(), (1, 0));
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["one".to_string(), String::new(), "two".to_string()]
        );
    }

    #[test]
    fn undo_and_redo_restore_buffer() {
        let mut p = Prompt::new();
        type_str(&mut p, "hello"); // one coalesced typing run
        assert_eq!(p.lines().next().unwrap(), "hello");
        p.apply(EditAction::Undo);
        assert_eq!(p.lines().next().unwrap(), "", "undo removes the typing run");
        p.apply(EditAction::Redo);
        assert_eq!(p.lines().next().unwrap(), "hello", "redo re-applies it");
    }

    #[test]
    fn undo_separates_runs_broken_by_a_motion() {
        let mut p = Prompt::new();
        type_str(&mut p, "ab");
        p.apply(EditAction::Move(CursorMotion::Left)); // breaks the typing run
        type_str(&mut p, "X"); // new group; inserts before 'b' -> "aXb"
        assert_eq!(p.lines().next().unwrap(), "aXb");
        p.apply(EditAction::Undo);
        assert_eq!(p.lines().next().unwrap(), "ab", "undo the 'X' group");
        p.apply(EditAction::Undo);
        assert_eq!(p.lines().next().unwrap(), "", "undo the 'ab' run");
    }

    #[test]
    fn redo_is_dropped_after_a_new_edit() {
        let mut p = Prompt::new();
        type_str(&mut p, "a");
        p.apply(EditAction::Undo); // buffer "", redo has "a"
        type_str(&mut p, "b"); // a new edit invalidates redo
        p.apply(EditAction::Redo); // nothing to redo
        assert_eq!(p.lines().next().unwrap(), "b");
    }

    #[test]
    fn operator_dw_deletes_word_and_trailing_space() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo bar");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_operator(
            Operator::Delete,
            OpTarget::Motion(CursorMotion::WordForward),
            1,
        );
        assert_eq!(p.lines().next().unwrap(), "bar");
        assert_eq!(p.cursor(), (0, 0));
    }

    #[test]
    fn operator_de_is_inclusive_of_word_end() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo bar");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_operator(Operator::Delete, OpTarget::Motion(CursorMotion::WordEnd), 1);
        // `de` deletes through the last char of "foo", leaving " bar".
        assert_eq!(p.lines().next().unwrap(), " bar");
    }

    #[test]
    fn operator_d_dollar_deletes_to_line_end() {
        let mut p = Prompt::new();
        type_str(&mut p, "hello world");
        for _ in 0..5 {
            p.apply(EditAction::Move(CursorMotion::Left)); // cursor at col 6 (start of "world")
        }
        p.apply_operator(Operator::Delete, OpTarget::Motion(CursorMotion::LineEnd), 1);
        assert_eq!(p.lines().next().unwrap(), "hello ");
    }

    #[test]
    fn capital_d_deletes_to_line_end() {
        // `D` = `LineTail(1)` = `d$`.
        let mut p = Prompt::new();
        type_str(&mut p, "hello world");
        for _ in 0..5 {
            p.apply(EditAction::Move(CursorMotion::Left)); // col 6
        }
        p.apply_operator(Operator::Delete, OpTarget::LineTail(1), 1);
        assert_eq!(p.lines().next().unwrap(), "hello ");
    }

    #[test]
    fn counted_capital_d_deletes_to_eol_plus_lines_below() {
        // `3D` from col 3 of line 1: delete to EOL + the two rows below,
        // leaving the head joined with line 4 (vim).
        let mut p = Prompt::new();
        type_str(&mut p, "abcDEF");
        p.apply(EditAction::Newline);
        type_str(&mut p, "line2");
        p.apply(EditAction::Newline);
        type_str(&mut p, "line3");
        p.apply(EditAction::Newline);
        type_str(&mut p, "line4");
        p.apply(EditAction::Move(CursorMotion::BufferStart)); // row 0
        p.apply(EditAction::Move(CursorMotion::LineStart));
        for _ in 0..3 {
            p.apply(EditAction::Move(CursorMotion::Right)); // col 3 (on 'D')
        }
        p.apply_operator(Operator::Delete, OpTarget::LineTail(3), 1);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["abc".to_string(), "line4".to_string()]
        );
        assert_eq!(p.cursor(), (0, 3));
        // `3D` cuts, so it pastes back.
        assert_eq!(p.register_text().as_deref(), Some("DEF\nline2\nline3"));
    }

    #[test]
    fn counted_capital_c_changes_to_eol_plus_lines_below() {
        // `2C` from col 1: change to EOL + the row below, then type.
        let mut p = Prompt::new();
        type_str(&mut p, "axx");
        p.apply(EditAction::Newline);
        type_str(&mut p, "bbb");
        p.apply(EditAction::Newline);
        type_str(&mut p, "keep");
        p.apply(EditAction::Move(CursorMotion::BufferStart));
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::Move(CursorMotion::Right)); // col 1 (on 'x')
        p.apply_operator(Operator::Change, OpTarget::LineTail(2), 1);
        type_str(&mut p, "Z");
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["aZ".to_string(), "keep".to_string()]
        );
    }

    #[test]
    fn counted_capital_d_clamps_past_the_buffer_end() {
        // `9D` near the end just deletes to the buffer end, no panic.
        let mut p = Prompt::new();
        type_str(&mut p, "one");
        p.apply(EditAction::Newline);
        type_str(&mut p, "two");
        p.apply(EditAction::Move(CursorMotion::BufferStart));
        p.apply(EditAction::Move(CursorMotion::LineStart)); // row 0, col 0
        p.apply_operator(Operator::Delete, OpTarget::LineTail(9), 1);
        assert_eq!(p.lines().collect::<Vec<_>>(), vec!["".to_string()]);
    }

    #[test]
    fn operator_db_deletes_backward() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo bar"); // cursor at end (col 7)
        p.apply(EditAction::Move(CursorMotion::WordBackward)); // col 4 (start of "bar")
        p.apply_operator(
            Operator::Delete,
            OpTarget::Motion(CursorMotion::WordBackward),
            1,
        );
        // Deletes "foo " (the range [0,4)), leaving "bar".
        assert_eq!(p.lines().next().unwrap(), "bar");
        assert_eq!(p.cursor(), (0, 0));
    }

    #[test]
    fn operator_counted_motion_multiplies() {
        let mut p = Prompt::new();
        type_str(&mut p, "a b c d");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_operator(
            Operator::Delete,
            OpTarget::Motion(CursorMotion::WordForward),
            2,
        );
        // Two words deleted: "a " and "b ", leaving "c d".
        assert_eq!(p.lines().next().unwrap(), "c d");
    }

    #[test]
    fn operator_cw_acts_like_ce_keeping_trailing_space() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo bar");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        // Change-word should not swallow the space (cw == ce).
        p.apply_operator(
            Operator::Change,
            OpTarget::Motion(CursorMotion::WordForward),
            1,
        );
        assert_eq!(p.lines().next().unwrap(), " bar");
    }

    #[test]
    fn operator_dd_removes_whole_lines() {
        let mut p = Prompt::new();
        type_str(&mut p, "one");
        p.apply(EditAction::Newline);
        type_str(&mut p, "two");
        p.apply(EditAction::Newline);
        type_str(&mut p, "three");
        p.apply(EditAction::Move(CursorMotion::Up)); // onto "two"
        p.apply_operator(Operator::Delete, OpTarget::Line, 1);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["one".to_string(), "three".to_string()]
        );
        assert_eq!(p.cursor(), (1, 0));
    }

    #[test]
    fn operator_dd_count_removes_multiple_lines() {
        let mut p = Prompt::new();
        for s in ["a", "b", "c", "d"] {
            type_str(&mut p, s);
            p.apply(EditAction::Newline);
        }
        p.apply(EditAction::Move(CursorMotion::BufferStart)); // onto "a"
        p.apply_operator(Operator::Delete, OpTarget::Line, 3);
        // Removed a, b, c — leaving "d" and the trailing empty line.
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["d".to_string(), String::new()]
        );
    }

    #[test]
    fn operator_dd_last_line_leaves_one_blank() {
        let mut p = Prompt::new();
        type_str(&mut p, "only");
        p.apply_operator(Operator::Delete, OpTarget::Line, 1);
        assert_eq!(p.line_count(), 1);
        assert_eq!(p.lines().next().unwrap(), "");
    }

    #[test]
    fn operator_cc_clears_line_but_keeps_it() {
        let mut p = Prompt::new();
        type_str(&mut p, "one");
        p.apply(EditAction::Newline);
        type_str(&mut p, "two"); // cursor on "two"
        p.apply_operator(Operator::Change, OpTarget::Line, 1);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["one".to_string(), String::new()]
        );
        assert_eq!(p.cursor(), (1, 0));
    }

    #[test]
    fn change_then_typing_is_one_undo_like_vim() {
        // The "extra undo" fix: `cw` then typing a replacement is a SINGLE
        // undo (delete + inserted text), not delete-then-insert as two.
        let mut p = Prompt::new();
        type_str(&mut p, "word baz");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_operator(
            Operator::Change,
            OpTarget::Motion(CursorMotion::WordForward),
            1,
        );
        type_str(&mut p, "new"); // the change's inserted text
        assert_eq!(p.lines().next().unwrap(), "new baz");
        p.apply(EditAction::Undo);
        assert_eq!(
            p.lines().next().unwrap(),
            "word baz",
            "one undo reverts the whole change, not just the typing"
        );
    }

    #[test]
    fn delete_then_typing_stays_two_undos() {
        // A `Delete` is a complete gesture — it must NOT coalesce with text
        // typed afterward (that's a separate edit, two undos, like vim).
        let mut p = Prompt::new();
        type_str(&mut p, "word baz");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_operator(
            Operator::Delete,
            OpTarget::Motion(CursorMotion::WordForward),
            1,
        );
        type_str(&mut p, "x");
        p.apply(EditAction::Undo);
        assert_eq!(
            p.lines().next().unwrap(),
            "baz",
            "first undo drops the typed text"
        );
        p.apply(EditAction::Undo);
        assert_eq!(
            p.lines().next().unwrap(),
            "word baz",
            "second undo restores the delete"
        );
    }

    #[test]
    fn noop_operator_does_not_wipe_redo() {
        // A no-op operator must not create an undo step or clear redo.
        let mut p = Prompt::new();
        type_str(&mut p, "hello");
        p.apply(EditAction::Undo); // buffer "", redo holds "hello"
        // `di(` with no parens on the line changes nothing.
        p.apply_operator(
            Operator::Delete,
            OpTarget::TextObject {
                scope: TextObjectScope::Inner,
                object: TextObject::Pair('(', ')'),
            },
            1,
        );
        p.apply(EditAction::Redo);
        assert_eq!(
            p.lines().next().unwrap(),
            "hello",
            "redo survived the no-op operator"
        );
    }

    #[test]
    fn operator_is_a_single_undo_group() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo bar baz");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_operator(
            Operator::Delete,
            OpTarget::Motion(CursorMotion::WordForward),
            2,
        );
        assert_eq!(p.lines().next().unwrap(), "baz");
        p.apply(EditAction::Undo);
        assert_eq!(
            p.lines().next().unwrap(),
            "foo bar baz",
            "the whole operator undoes in one step"
        );
    }

    // --- cross-row linewise operator motions (dj/dk/dG/dgg) -----------

    /// Build a 4-line buffer "a/b/c/d" with the cursor on row `row`.
    fn four_lines(row: usize) -> Prompt {
        let mut p = Prompt::new();
        for (i, s) in ["a", "b", "c", "d"].iter().enumerate() {
            type_str(&mut p, s);
            if i < 3 {
                p.apply(EditAction::Newline);
            }
        }
        p.apply(EditAction::Move(CursorMotion::BufferStart));
        for _ in 0..row {
            p.apply(EditAction::Move(CursorMotion::Down));
        }
        p
    }

    #[test]
    fn dj_deletes_current_and_next_line() {
        let mut p = four_lines(0); // on "a"
        p.apply_operator(
            Operator::Delete,
            OpTarget::LineMotion(CursorMotion::Down),
            1,
        );
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["c".to_string(), "d".to_string()]
        );
        assert_eq!(p.cursor(), (0, 0));
    }

    #[test]
    fn dk_deletes_current_and_previous_line() {
        let mut p = four_lines(2); // on "c"
        p.apply_operator(Operator::Delete, OpTarget::LineMotion(CursorMotion::Up), 1);
        // Removes "b" and "c", leaving "a" and "d"; cursor on the row that
        // was "b" (now "d").
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["a".to_string(), "d".to_string()]
        );
        assert_eq!(p.cursor(), (1, 0));
    }

    #[test]
    fn counted_dj_spans_more_rows() {
        let mut p = four_lines(0); // on "a"
        p.apply_operator(
            Operator::Delete,
            OpTarget::LineMotion(CursorMotion::Down),
            2,
        );
        // [0..=2] removed (a, b, c), leaving "d".
        assert_eq!(p.lines().collect::<Vec<_>>(), vec!["d".to_string()]);
    }

    #[test]
    fn dgg_deletes_up_to_first_line() {
        let mut p = four_lines(2); // on "c"
        p.apply_operator(
            Operator::Delete,
            OpTarget::LineMotion(CursorMotion::BufferStart),
            1,
        );
        // Rows 0..=2 (a, b, c) removed, leaving "d".
        assert_eq!(p.lines().collect::<Vec<_>>(), vec!["d".to_string()]);
        assert_eq!(p.cursor(), (0, 0));
    }

    #[test]
    fn d_capital_g_deletes_to_last_line() {
        let mut p = four_lines(1); // on "b"
        p.apply_operator(
            Operator::Delete,
            OpTarget::LineMotion(CursorMotion::BufferEnd),
            1,
        );
        // Rows 1..=3 (b, c, d) removed, leaving "a".
        assert_eq!(p.lines().collect::<Vec<_>>(), vec!["a".to_string()]);
        assert_eq!(p.cursor(), (0, 0));
    }

    #[test]
    fn cj_changes_two_lines_to_one_blank() {
        let mut p = four_lines(0); // on "a"
        p.apply_operator(
            Operator::Change,
            OpTarget::LineMotion(CursorMotion::Down),
            1,
        );
        // Change keeps a single (cleared) line where the span was.
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![String::new(), "c".to_string(), "d".to_string()]
        );
        assert_eq!(p.cursor(), (0, 0));
    }

    #[test]
    fn dgg_deleting_whole_buffer_keeps_one_blank() {
        let mut p = four_lines(3); // on "d"
        // From the last line, gg reaches row 0 → the whole buffer.
        p.apply_operator(
            Operator::Delete,
            OpTarget::LineMotion(CursorMotion::BufferStart),
            1,
        );
        assert_eq!(p.line_count(), 1);
        assert_eq!(p.lines().next().unwrap(), "");
    }

    #[test]
    fn dj_on_last_line_is_a_noop() {
        // Relative motion can't advance past the last line → vim no-op
        // (not "delete the current line").
        let mut p = four_lines(3); // on "d"
        p.apply_operator(
            Operator::Delete,
            OpTarget::LineMotion(CursorMotion::Down),
            1,
        );
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string()
            ]
        );
    }

    #[test]
    fn dk_on_first_line_is_a_noop() {
        let mut p = four_lines(0); // on "a"
        p.apply_operator(Operator::Delete, OpTarget::LineMotion(CursorMotion::Up), 1);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string()
            ]
        );
    }

    // --- visual selection (phase 1: model + delete) -------------------

    #[test]
    fn set_anchor_records_cursor_and_clear_drops_it() {
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::Move(CursorMotion::Right)); // (0, 1)
        p.apply(EditAction::SetAnchor);
        assert_eq!(p.anchor(), Some((0, 1)));
        p.apply(EditAction::ClearAnchor);
        assert_eq!(p.anchor(), None);
    }

    #[test]
    fn select_text_object_inner_word_anchors_start_cursor_on_last_char() {
        // `viw`: anchor at the word's start, cursor on its last char.
        let mut p = Prompt::new();
        type_str(&mut p, "foo bar baz");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::Move(CursorMotion::WordForward)); // (0, 4), start of "bar"
        p.apply(EditAction::SelectTextObject {
            scope: TextObjectScope::Inner,
            object: TextObject::Word,
            count: 1,
        });
        assert_eq!(p.anchor(), Some((0, 4)), "anchor at the start of the word");
        assert_eq!(p.cursor(), (0, 6), "cursor on the word's last char");
    }

    #[test]
    fn select_text_object_inner_quote() {
        // `vi"` selects the text between the quotes.
        let mut p = Prompt::new();
        type_str(&mut p, "say \"hi\" now");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        for _ in 0..6 {
            p.apply(EditAction::Move(CursorMotion::Right)); // (0, 6), on the 'i'
        }
        p.apply(EditAction::SelectTextObject {
            scope: TextObjectScope::Inner,
            object: TextObject::Quote('"'),
            count: 1,
        });
        assert_eq!(
            p.anchor(),
            Some((0, 5)),
            "anchor just inside the open quote"
        );
        assert_eq!(p.cursor(), (0, 6), "cursor on the inner text's last char");
    }

    #[test]
    fn swap_selection_ends_swaps_cursor_and_anchor() {
        // Visual `o`: cursor jumps to the anchor and vice-versa; a second swap
        // restores. Lets you extend a selection from the other end.
        let mut b = Buffer::from_lines(vec!["hello world".to_string()]);
        b.place_cursor(0, 2);
        b.set_anchor(SelectionKind::Char); // anchor at (0, 2)
        b.place_cursor(0, 7); // cursor extends to (0, 7)
        b.swap_selection_ends();
        assert_eq!(b.cursor(), (0, 2), "cursor jumps to the old anchor");
        assert_eq!(b.anchor(), Some((0, 7)), "anchor becomes the old cursor");
        b.swap_selection_ends();
        assert_eq!(b.cursor(), (0, 7), "swapping again restores");
        assert_eq!(b.anchor(), Some((0, 2)));
    }

    #[test]
    fn swap_selection_ends_without_selection_is_a_noop() {
        let mut b = Buffer::from_lines(vec!["abc".to_string()]);
        b.place_cursor(0, 1);
        b.swap_selection_ends();
        assert_eq!(b.cursor(), (0, 1));
        assert_eq!(b.anchor(), None);
    }

    #[test]
    fn select_text_object_no_pair_leaves_selection_untouched() {
        // `vi(` with no surrounding pair is a no-op (selection unchanged).
        let mut p = Prompt::new();
        type_str(&mut p, "no parens here");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::SelectTextObject {
            scope: TextObjectScope::Inner,
            object: TextObject::Pair('(', ')'),
            count: 1,
        });
        assert_eq!(p.anchor(), None, "no surrounding pair → no selection");
    }

    /// The buffer nvim was asked about (10-19): two paragraphs, two blank
    /// lines, one, a line of white space only — blank too — and one.
    const PARAGRAPHS: [&str; 8] = ["a1", "a2", "", "", "b1", "   ", "c1", "c2"];

    /// **`ip` / `ap` select what vim selects** — case for case as headless
    /// nvim answered (`v{count}{i,a}p`, then the `'<` `'>` lines), rows from
    /// 0 here. `None` is where nvim selected nothing.
    #[test]
    fn paragraph_objects_select_what_vim_selects() {
        use TextObjectScope::{Around as A, Inner as I};
        type Case = (&'static [&'static str], usize, TextObjectScope, usize);
        let cases: &[(Case, Option<(usize, usize)>)] = &[
            ((&PARAGRAPHS, 0, I, 1), Some((0, 1))),
            ((&PARAGRAPHS, 0, A, 1), Some((0, 3))),
            ((&PARAGRAPHS, 1, A, 1), Some((0, 3))),
            ((&PARAGRAPHS, 2, I, 1), Some((2, 3))),
            ((&PARAGRAPHS, 2, A, 1), Some((2, 4))),
            ((&PARAGRAPHS, 3, A, 1), Some((2, 4))),
            ((&PARAGRAPHS, 4, I, 1), Some((4, 4))),
            ((&PARAGRAPHS, 4, A, 1), Some((4, 5))),
            ((&PARAGRAPHS, 5, I, 1), Some((5, 5))),
            ((&PARAGRAPHS, 5, A, 1), Some((5, 7))),
            ((&PARAGRAPHS, 6, I, 1), Some((6, 7))),
            ((&PARAGRAPHS, 6, A, 1), Some((5, 7))),
            ((&PARAGRAPHS, 7, A, 1), Some((5, 7))),
            ((&PARAGRAPHS, 0, A, 2), Some((0, 5))),
            ((&PARAGRAPHS, 0, I, 2), Some((0, 3))),
            ((&PARAGRAPHS, 0, A, 9), None),
            ((&["x", "y"], 0, A, 1), Some((0, 1))),
            ((&["x", "y"], 1, I, 1), Some((0, 1))),
            ((&["", "", ""], 1, A, 1), None),
            ((&["", "", ""], 1, I, 1), Some((0, 2))),
            ((&["", "x", ""], 0, A, 1), Some((0, 1))),
            ((&["x", "", ""], 2, A, 1), None),
        ];
        for &((lines, row, scope, count), want) in cases {
            let chars: Vec<Vec<char>> = lines.iter().map(|l| l.chars().collect()).collect();
            assert_eq!(
                paragraph_lines(&chars, row, scope, count),
                want,
                "{lines:?} from {row}, {scope:?} ×{count}"
            );
        }
    }

    /// **A paragraph is lines, through both doors** — Visual's `vap` (then
    /// `y`) and the operator's `yap` take the same lines, linewise, as nvim's
    /// register (`V`, `a1 a2 _ _`); `dap` and `dip` leave what nvim's leave.
    #[test]
    fn a_paragraph_is_selected_yanked_and_deleted_as_lines() {
        let around = TextObject::Paragraph;
        let object = |scope| OpTarget::TextObject {
            scope,
            object: around,
        };
        let mut b = Buffer::from_lines(PARAGRAPHS.iter().map(|l| l.to_string()).collect());
        b.place_cursor(1, 1);
        assert!(b.apply_read_only(
            EditAction::SelectTextObject {
                scope: TextObjectScope::Around,
                object: around,
                count: 1,
            },
            1
        ));
        assert_eq!(b.selected_lines(), Some(0..=3), "`vap` from a2");
        let mut two = Buffer::from_lines(PARAGRAPHS.iter().map(|l| l.to_string()).collect());
        two.place_cursor(0, 0);
        two.apply_read_only(
            EditAction::SelectTextObject {
                scope: TextObjectScope::Around,
                object: around,
                count: 2,
            },
            1,
        );
        assert_eq!(two.selected_lines(), Some(0..=5), "`v2ap`, as nvim's");
        let lines = ("a1\na2\n\n\n".to_string(), SelectionKind::Line);
        let (copied, kind) = b.yank(OpTarget::Selection, 1).expect("lines");
        assert_eq!((copied.text(), kind), lines);
        b.place_cursor(0, 0);
        let (copied, kind) = b.yank(object(TextObjectScope::Around), 1).expect("lines");
        assert_eq!((copied.text(), kind), lines);

        let typed = || {
            let mut p = Prompt::new();
            for (i, line) in PARAGRAPHS.iter().enumerate() {
                if i > 0 {
                    p.apply(EditAction::Newline);
                }
                type_str(&mut p, line);
            }
            p
        };
        let mut p = typed();
        p.place_cursor(0, 0);
        p.apply_operator(Operator::Delete, object(TextObjectScope::Around), 1);
        assert_eq!(p.lines().collect::<Vec<_>>(), ["b1", "   ", "c1", "c2"]);
        let mut p = typed();
        p.place_cursor(6, 0);
        p.apply_operator(Operator::Delete, object(TextObjectScope::Inner), 1);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            ["a1", "a2", "", "", "b1", "   "]
        );
    }

    #[test]
    fn delete_selection_single_line_is_inclusive() {
        let mut p = Prompt::new();
        type_str(&mut p, "hello world");
        p.apply(EditAction::Move(CursorMotion::LineStart)); // (0, 0), on 'h'
        p.apply(EditAction::SetAnchor);
        for _ in 0..4 {
            p.apply(EditAction::Move(CursorMotion::Right)); // (0, 4), on 'o'
        }
        p.apply_operator(Operator::Delete, OpTarget::Selection, 1);
        assert_eq!(
            p.lines().next().unwrap(),
            " world",
            "deletes 'hello' inclusive"
        );
        assert_eq!(p.cursor(), (0, 0));
        assert_eq!(p.anchor(), None, "selection consumed");
    }

    #[test]
    fn empty_selection_deletes_the_char_under_cursor() {
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        p.apply(EditAction::Move(CursorMotion::LineStart)); // (0, 0)
        p.apply(EditAction::SetAnchor); // anchor == cursor
        p.apply_operator(Operator::Delete, OpTarget::Selection, 1);
        assert_eq!(p.lines().next().unwrap(), "bc");
    }

    #[test]
    fn delete_selection_across_lines_joins_head_and_tail() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo");
        p.apply(EditAction::Newline);
        type_str(&mut p, "bar");
        p.apply(EditAction::Newline);
        type_str(&mut p, "baz");
        // Anchor at (0, 1), select down to (2, 1).
        p.apply(EditAction::Move(CursorMotion::BufferStart)); // (0, 3) col preserved
        p.apply(EditAction::Move(CursorMotion::LineStart)); // (0, 0)
        p.apply(EditAction::Move(CursorMotion::Right)); // (0, 1)
        p.apply(EditAction::SetAnchor);
        p.apply(EditAction::Move(CursorMotion::Down)); // (1, 1)
        p.apply(EditAction::Move(CursorMotion::Down)); // (2, 1)
        p.apply_operator(Operator::Delete, OpTarget::Selection, 1);
        // Keeps "f" (head of row 0) + "z" (tail of row 2 after col 1).
        assert_eq!(p.lines().collect::<Vec<_>>(), vec!["fz".to_string()]);
        assert_eq!(p.cursor(), (0, 1));
    }

    #[test]
    fn change_selection_coalesces_with_typing_one_undo() {
        let mut p = Prompt::new();
        type_str(&mut p, "hello");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::SetAnchor);
        for _ in 0..4 {
            p.apply(EditAction::Move(CursorMotion::Right)); // select "hello"
        }
        p.apply_operator(Operator::Change, OpTarget::Selection, 1);
        type_str(&mut p, "hi"); // the change's inserted text
        assert_eq!(p.lines().next().unwrap(), "hi");
        p.apply(EditAction::Undo);
        assert_eq!(
            p.lines().next().unwrap(),
            "hello",
            "visual change + typing is one undo"
        );
    }

    #[test]
    fn visible_window_no_selection_when_no_anchor() {
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        let w = p.visible_window(20, 5);
        assert_eq!(w.selection.len(), w.rows.len());
        assert!(w.selection.iter().all(Option::is_none));
    }

    #[test]
    fn visible_window_marks_single_line_selection() {
        let mut p = Prompt::new();
        type_str(&mut p, "hello world");
        p.apply(EditAction::Move(CursorMotion::LineStart)); // (0, 0)
        p.apply(EditAction::SetAnchor);
        for _ in 0..4 {
            p.apply(EditAction::Move(CursorMotion::Right)); // (0, 4)
        }
        let w = p.visible_window(20, 5);
        // Inclusive of the cursor char → chars [0, 5) ("hello").
        assert_eq!(w.selection[0], Some((0, 5)));
    }

    #[test]
    fn visible_window_marks_multi_line_selection_per_row() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo");
        p.apply(EditAction::Newline);
        type_str(&mut p, "bar");
        p.apply(EditAction::Move(CursorMotion::BufferStart)); // (0, 3)
        p.apply(EditAction::Move(CursorMotion::LineStart)); // (0, 0)
        p.apply(EditAction::Move(CursorMotion::Right)); // (0, 1)
        p.apply(EditAction::SetAnchor);
        p.apply(EditAction::Move(CursorMotion::Down)); // (1, 1)
        p.apply(EditAction::Move(CursorMotion::Right)); // (1, 2)
        let w = p.visible_window(20, 5);
        assert_eq!(w.selection[0], Some((1, 3)), "first line: col 1 to end");
        assert_eq!(
            w.selection[1],
            Some((0, 3)),
            "last line: start through cursor"
        );
    }

    #[test]
    fn visible_window_selection_slices_across_a_wrap() {
        let mut p = Prompt::new();
        type_str(&mut p, "0123456789"); // 10 chars
        p.apply(EditAction::Move(CursorMotion::LineStart)); // (0, 0)
        p.apply(EditAction::Move(CursorMotion::Right));
        p.apply(EditAction::Move(CursorMotion::Right)); // (0, 2)
        p.apply(EditAction::SetAnchor);
        for _ in 0..5 {
            p.apply(EditAction::Move(CursorMotion::Right)); // (0, 7)
        }
        // width 5 → rows "01234" / "56789"; selection chars [2, 8).
        let w = p.visible_window(5, 5);
        assert_eq!(w.selection[0], Some((2, 5)), "tail of the first visual row");
        assert_eq!(
            w.selection[1],
            Some((0, 3)),
            "head of the second visual row"
        );
    }

    /// **A selection ending before a wrapped row begins leaves that row
    /// unselected, and does not underflow** — the lazy `then` in
    /// `Buffer::visible_window`. The test above spans both rows, where every
    /// subtraction is safe; on a row past the selection `hi < rstart`, and
    /// `then_some` computes `hi - rstart` anyway. Two docs said this guard
    /// had a test, and `then` → `then_some` passed the suite (review stop 12,
    /// T6 4).
    #[test]
    fn visible_window_a_selection_before_a_wrapped_row_leaves_it_unselected() {
        let mut p = Prompt::new();
        type_str(&mut p, "0123456789");
        p.apply(EditAction::Move(CursorMotion::LineStart)); // (0, 0)
        p.apply(EditAction::SetAnchor);
        p.apply(EditAction::Move(CursorMotion::Right)); // (0, 1)
        // width 5 → rows "01234" / "56789"; selection chars [0, 2), inside
        // the first. The second starts at 5, past it.
        let w = p.visible_window(5, 5);
        assert_eq!(w.rows, ["01234", "56789"], "premise: one line, two rows");
        assert_eq!(w.selection[0], Some((0, 2)));
        assert_eq!(w.selection[1], None, "past the selection, not inverted");
    }

    #[test]
    fn cleared_selection_makes_the_operator_a_noop() {
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        p.apply(EditAction::SetAnchor);
        p.apply(EditAction::ClearAnchor);
        p.apply_operator(Operator::Delete, OpTarget::Selection, 1);
        assert_eq!(p.lines().next().unwrap(), "abc", "nothing selected → no-op");
    }

    // --- linewise Visual (V) ------------------------------------------

    #[test]
    fn linewise_visual_delete_removes_whole_rows() {
        let mut p = Prompt::new();
        type_str(&mut p, "one");
        p.apply(EditAction::Newline);
        type_str(&mut p, "two");
        p.apply(EditAction::Newline);
        type_str(&mut p, "three");
        p.apply(EditAction::Move(CursorMotion::BufferStart)); // onto row 0
        p.apply(EditAction::SetAnchorLine); // linewise anchor
        p.apply(EditAction::Move(CursorMotion::Down)); // extend to row 1
        p.apply_operator(Operator::Delete, OpTarget::Selection, 1);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["three".to_string()],
            "both whole rows removed"
        );
        assert_eq!(p.anchor(), None, "selection consumed");
    }

    #[test]
    fn linewise_visual_yank_is_a_linewise_register() {
        let mut p = Prompt::new();
        type_str(&mut p, "aa");
        p.apply(EditAction::Newline);
        type_str(&mut p, "bb");
        p.apply(EditAction::Move(CursorMotion::BufferStart)); // row 0
        p.apply(EditAction::SetAnchorLine); // just row 0 (anchor == cursor row)
        p.apply_operator(Operator::Yank, OpTarget::Selection, 1);
        assert_eq!(
            p.register_text().as_deref(),
            Some("aa\n"),
            "linewise register keeps the trailing newline"
        );
        // ...so pasting duplicates a whole line below.
        p.apply(EditAction::Paste);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["aa".to_string(), "aa".to_string(), "bb".to_string()]
        );
    }

    #[test]
    fn set_selection_linewise_flips_kind_without_re_anchoring() {
        // The `v`↔`V` switch: same anchor, geometry changes char→line.
        let mut p = Prompt::new();
        type_str(&mut p, "ab");
        p.apply(EditAction::Newline);
        type_str(&mut p, "cd");
        p.apply(EditAction::Move(CursorMotion::BufferStart)); // (0,?)
        p.apply(EditAction::SetAnchor); // charwise anchor at row 0
        p.apply(EditAction::Move(CursorMotion::Down)); // cursor row 1
        // Switch to linewise (anchor preserved), then delete: whole rows.
        p.apply(EditAction::SetSelectionKind(SelectionKind::Line));
        p.apply_operator(Operator::Delete, OpTarget::Selection, 1);
        assert_eq!(
            p.line_count(),
            1,
            "both rows gone (linewise), not a char span"
        );
        assert_eq!(p.lines().next().unwrap(), "");
    }

    #[test]
    fn visible_window_linewise_selection_highlights_whole_rows() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo");
        p.apply(EditAction::Newline);
        type_str(&mut p, "barbar"); // longer row, to prove it's not a char range
        p.apply(EditAction::Move(CursorMotion::BufferStart)); // row 0
        p.apply(EditAction::SetAnchorLine);
        p.apply(EditAction::Move(CursorMotion::Down)); // extend to row 1
        let w = p.visible_window(20, 5);
        assert_eq!(w.selection[0], Some((0, 3)), "whole first row");
        assert_eq!(w.selection[1], Some((0, 6)), "whole second row");
    }

    // --- block Visual (Ctrl-V) ----------------------------------------

    /// 3-row grid "abcd"/"efgh"/"ijkl" with the cursor moved to `(row, col)`.
    fn grid_at(row: usize, col: usize) -> Prompt {
        let mut p = Prompt::new();
        for (i, s) in ["abcd", "efgh", "ijkl"].iter().enumerate() {
            type_str(&mut p, s);
            if i < 2 {
                p.apply(EditAction::Newline);
            }
        }
        p.apply(EditAction::Move(CursorMotion::BufferStart));
        p.apply(EditAction::Move(CursorMotion::LineStart));
        for _ in 0..row {
            p.apply(EditAction::Move(CursorMotion::Down));
        }
        for _ in 0..col {
            p.apply(EditAction::Move(CursorMotion::Right));
        }
        p
    }

    #[test]
    fn row_span_projects_each_shape_to_per_row_spans() {
        // The single per-row projection shared by the selection highlight and
        // the block read/drain — so what's highlighted matches what's cut.

        // Charwise: first row from its start col; last row to the (exclusive)
        // end; full rows between; rows outside the span are None.
        let chars = RangeShape::Chars {
            start: (1, 2),
            end: (3, 4),
        };
        assert_eq!(chars.row_span(0, 10), None);
        assert_eq!(chars.row_span(1, 10), Some((2, 10))); // start: [sc, len)
        assert_eq!(chars.row_span(2, 10), Some((0, 10))); // interior: full
        assert_eq!(chars.row_span(3, 10), Some((0, 4))); // end: [0, ec)
        assert_eq!(chars.row_span(4, 10), None);

        // Linewise: every row in [first, last] is the whole row; an empty row
        // (lo == hi) collapses to None.
        let lines = RangeShape::Lines { first: 1, last: 2 };
        assert_eq!(lines.row_span(0, 5), None);
        assert_eq!(lines.row_span(1, 5), Some((0, 5)));
        assert_eq!(lines.row_span(2, 0), None);
        assert_eq!(lines.row_span(3, 5), None);

        // Block: the column rectangle, clamped to each row's own length.
        let block = RangeShape::Block {
            rows: (0, 2),
            cols: (1, 4),
        };
        assert_eq!(block.row_span(0, 10), Some((1, 4))); // full width
        assert_eq!(block.row_span(1, 2), Some((1, 2))); // short row: partial
        assert_eq!(block.row_span(2, 1), None); // row shorter than left edge
        assert_eq!(block.row_span(3, 10), None); // outside the row span

        // `$`-block: the right edge is usize::MAX, clamped to each row's end.
        let eol = RangeShape::Block {
            rows: (0, 1),
            cols: (1, usize::MAX),
        };
        assert_eq!(eol.row_span(0, 5), Some((1, 5)));
        assert_eq!(eol.row_span(1, 3), Some((1, 3)));
    }

    #[test]
    fn block_visual_delete_removes_a_column_rectangle() {
        let mut p = grid_at(0, 1); // anchor corner at (0,1)
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Right)); // cursor (2,2)
        p.apply_operator(Operator::Delete, OpTarget::Selection, 1);
        // Columns [1,3) (the 'b','c'/'f','g'/'j','k' rectangle) removed.
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["ad".to_string(), "eh".to_string(), "il".to_string()]
        );
        assert_eq!(p.cursor(), (0, 1), "cursor parks at the top-left");
    }

    #[test]
    fn block_visual_change_replicates_typed_text_to_every_row() {
        // `Ctrl-V` a column, `c`, type, `Esc` → the rectangle is cut and the
        // text typed on the first row is replicated onto every row (vim block
        // change), not just the first row.
        let mut p = grid_at(0, 1); // "abcd"/"efgh"/"ijkl", anchor corner at (0,1)
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Right)); // block cols [1,3)
        p.apply_operator(Operator::Change, OpTarget::Selection, 1); // cut + begin change
        assert_eq!(
            p.cursor(),
            (0, 1),
            "change parks at the top-left, ready to type"
        );
        type_str(&mut p, "XY");
        p.apply(EditAction::ClampCursorAfterInsert); // Esc finalizes + replicates
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["aXYd".to_string(), "eXYh".to_string(), "iXYl".to_string()],
            "typed text is replicated to every row of the block"
        );
    }

    #[test]
    fn block_visual_change_is_a_single_undo_group() {
        // The cut + type + replicate undo as one step (like block insert).
        let mut p = grid_at(0, 1);
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Right));
        p.apply_operator(Operator::Change, OpTarget::Selection, 1);
        type_str(&mut p, "XY");
        p.apply(EditAction::ClampCursorAfterInsert);
        p.apply(EditAction::Undo);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["abcd".to_string(), "efgh".to_string(), "ijkl".to_string()],
            "one undo restores the whole block change"
        );
    }

    #[test]
    fn block_visual_yank_and_paste_round_trips_a_rectangle() {
        let mut p = grid_at(0, 1);
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Right)); // (2,2)
        p.apply_operator(Operator::Yank, OpTarget::Selection, 1);
        assert_eq!(
            p.register_text().as_deref(),
            Some("bc\nfg\njk"),
            "blockwise register"
        );
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["abcd".to_string(), "efgh".to_string(), "ijkl".to_string()],
            "yank leaves the buffer"
        );
        // Cursor parked at (0,1); `P` re-inserts the rectangle at column 1.
        p.apply(EditAction::PasteBefore);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "abcbcd".to_string(),
                "efgfgh".to_string(),
                "ijkjkl".to_string()
            ]
        );
    }

    #[test]
    fn block_paste_pads_short_rows_and_extends_the_buffer() {
        let mut p = Prompt::new();
        type_str(&mut p, "AB");
        p.apply(EditAction::Newline);
        type_str(&mut p, "CD");
        p.apply(EditAction::Newline);
        type_str(&mut p, "x"); // a short trailing row
        // Block-yank the "AB"/"CD" rectangle (rows 0-1, cols [0,2)).
        p.apply(EditAction::Move(CursorMotion::BufferStart));
        p.apply(EditAction::Move(CursorMotion::LineStart)); // (0,0)
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Right)); // (1,1)
        p.apply_operator(Operator::Yank, OpTarget::Selection, 1); // "AB\nCD"
        // Paste at the end of the short row "x": row 2 takes it inline, and
        // a fresh row 3 is created + space-padded up to the column.
        p.apply(EditAction::Move(CursorMotion::BufferEnd));
        p.apply(EditAction::Move(CursorMotion::LineEnd)); // (2,1)
        p.apply(EditAction::PasteBefore); // col 1
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "AB".to_string(),
                "CD".to_string(),
                "xAB".to_string(),
                " CD".to_string()
            ]
        );
    }

    #[test]
    fn visible_window_block_selection_highlights_a_rectangle() {
        let mut p = grid_at(0, 1);
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Right)); // (2,2)
        let w = p.visible_window(20, 5);
        // Columns [1,3) highlighted on every spanned row — not a char run.
        assert_eq!(w.selection[0], Some((1, 3)));
        assert_eq!(w.selection[1], Some((1, 3)));
        assert_eq!(w.selection[2], Some((1, 3)));
    }

    // --- counted insert (3i / 3a / 3o …) ------------------------------

    /// Drive a counted insert: enter, type, leave (the `<Esc>` finalize).
    fn counted_insert(p: &mut Prompt, entry: InsertEntry, count: usize, typed: &str) {
        p.apply(EditAction::EnterInsert { entry, count });
        type_str(p, typed);
        p.apply(EditAction::ClampCursorAfterInsert);
    }

    #[test]
    fn counted_insert_replays_the_typed_text() {
        // vim `3ix<Esc>` → "xxx" (the reported bug).
        let mut p = Prompt::new();
        counted_insert(&mut p, InsertEntry::Before, 3, "x");
        assert_eq!(p.lines().next().unwrap(), "xxx");
        // The whole typed string repeats, not just one char: `2iab` → "abab".
        let mut p = Prompt::new();
        counted_insert(&mut p, InsertEntry::Before, 2, "ab");
        assert_eq!(p.lines().next().unwrap(), "abab");
    }

    #[test]
    fn counted_append_repeats_after_the_cursor() {
        let mut p = Prompt::new();
        type_str(&mut p, "Z");
        p.apply(EditAction::Move(CursorMotion::LineStart)); // on 'Z'
        counted_insert(&mut p, InsertEntry::After, 3, "x"); // `3a`
        assert_eq!(p.lines().next().unwrap(), "Zxxx");
    }

    #[test]
    fn counted_insert_replays_multiline_text() {
        // vim `3ifoo<CR>bar<Esc>` repeats the whole `foo\nbar`, newlines and
        // all — not just the last row (the reported bug).
        let mut p = Prompt::new();
        p.apply(EditAction::EnterInsert {
            entry: InsertEntry::Before,
            count: 3,
        });
        type_str(&mut p, "foo");
        p.apply(EditAction::Newline);
        type_str(&mut p, "bar");
        p.apply(EditAction::ClampCursorAfterInsert);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "foo".to_string(),
                "barfoo".to_string(),
                "barfoo".to_string(),
                "bar".to_string(),
            ],
        );
        // The whole counted insert (and its replay) is a single undo group.
        p.apply(EditAction::Undo);
        assert_eq!(p.lines().collect::<Vec<_>>(), vec![String::new()]);
    }

    #[test]
    fn insert_session_with_backspace_is_one_undo() {
        // vim: a whole `i…<Esc>` is one undo whatever you do inside it —
        // `ifoo<BS>x<Esc>` → "fox", and one `u` reverts the whole session.
        let mut p = Prompt::new();
        p.apply(EditAction::EnterInsert {
            entry: InsertEntry::Before,
            count: 1,
        });
        type_str(&mut p, "foo");
        p.apply(EditAction::DeleteBack); // <BS> → "fo"
        type_str(&mut p, "x"); // → "fox"
        p.apply(EditAction::ClampCursorAfterInsert);
        assert_eq!(p.lines().next().unwrap(), "fox");
        p.apply(EditAction::Undo);
        assert_eq!(p.lines().collect::<Vec<_>>(), vec![String::new()]);
    }

    #[test]
    fn counted_open_below_makes_a_line_per_count() {
        // `3ox<Esc>` → three new lines below, each "x".
        let mut p = Prompt::new();
        type_str(&mut p, "top");
        counted_insert(&mut p, InsertEntry::OpenBelow, 3, "x");
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "top".to_string(),
                "x".to_string(),
                "x".to_string(),
                "x".to_string()
            ]
        );
        // ...and the whole thing undoes in one step.
        p.apply(EditAction::Undo);
        assert_eq!(p.lines().collect::<Vec<_>>(), vec!["top".to_string()]);
    }

    #[test]
    fn counted_open_above_makes_a_line_per_count() {
        // `3Ox<Esc>` → three new lines above, each "x".
        let mut p = Prompt::new();
        type_str(&mut p, "bot");
        counted_insert(&mut p, InsertEntry::OpenAbove, 3, "x");
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "x".to_string(),
                "x".to_string(),
                "x".to_string(),
                "bot".to_string()
            ]
        );
    }

    #[test]
    fn counted_insert_is_a_single_undo_group() {
        let mut p = Prompt::new();
        counted_insert(&mut p, InsertEntry::Before, 3, "x");
        assert_eq!(p.lines().next().unwrap(), "xxx");
        p.apply(EditAction::Undo);
        assert_eq!(
            p.lines().next().unwrap(),
            "",
            "one undo reverts all three copies"
        );
    }

    #[test]
    fn counted_insert_with_no_typing_changes_nothing() {
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        counted_insert(&mut p, InsertEntry::Before, 3, ""); // `3i<Esc>`
        assert_eq!(p.lines().next().unwrap(), "abc");
    }

    #[test]
    fn an_arrow_inside_a_counted_insert_drops_the_count() {
        // nvim, on ["ab","cd"] from (0,0): `3i<Down>x<Esc>` → ["ab","xcd"].
        // The move ends the count; what was typed before it stays, once
        // (review stop 16, A M7).
        for (before, want_top) in [("", "ab"), ("q", "qab")] {
            let mut p = Prompt::new();
            type_str(&mut p, "ab");
            p.apply(EditAction::Newline);
            type_str(&mut p, "cd");
            p.apply(EditAction::Move(CursorMotion::Up));
            p.apply(EditAction::Move(CursorMotion::LineStart));
            p.apply(EditAction::EnterInsert {
                entry: InsertEntry::Before,
                count: 3,
            });
            type_str(&mut p, before);
            p.apply(EditAction::Move(CursorMotion::Down));
            type_str(&mut p, "z");
            p.apply(EditAction::ClampCursorAfterInsert);
            let lines: Vec<String> = p.lines().collect();
            assert_eq!(lines[0], want_top, "{before:?}");
            assert_eq!(lines.concat().matches('z').count(), 1, "{lines:?}");
        }
    }

    #[test]
    fn an_arrow_inside_an_insert_breaks_its_undo() {
        // nvim: `ifoo<Left>bar<Esc>u` leaves `foo` — the arrow began a new
        // undo step — where `ifoobar<Esc>u` leaves nothing.
        let mut p = Prompt::new();
        p.apply(EditAction::EnterInsert {
            entry: InsertEntry::Before,
            count: 1,
        });
        type_str(&mut p, "foo");
        p.apply(EditAction::Move(CursorMotion::Left));
        type_str(&mut p, "bar");
        p.apply(EditAction::ClampCursorAfterInsert);
        assert_eq!(p.lines().next().unwrap(), "fobaro");
        p.apply(EditAction::Undo);
        assert_eq!(p.lines().next().unwrap(), "foo");
    }

    #[test]
    fn counted_open_below_with_no_typing_still_opens_count_lines() {
        // vim `3o<Esc>` with nothing typed → three blank lines below, not one
        // (the empty-text replay guard must not skip the line-opening).
        let mut p = Prompt::new();
        type_str(&mut p, "top");
        counted_insert(&mut p, InsertEntry::OpenBelow, 3, "");
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "top".to_string(),
                String::new(),
                String::new(),
                String::new(),
            ]
        );
        // ...and the three opened lines undo in one step.
        p.apply(EditAction::Undo);
        assert_eq!(p.lines().collect::<Vec<_>>(), vec!["top".to_string()]);
    }

    #[test]
    fn counted_open_above_with_no_typing_still_opens_count_lines() {
        // vim `3O<Esc>` with nothing typed → three blank lines above.
        let mut p = Prompt::new();
        type_str(&mut p, "bot");
        counted_insert(&mut p, InsertEntry::OpenAbove, 3, "");
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                String::new(),
                String::new(),
                String::new(),
                "bot".to_string(),
            ]
        );
    }

    // --- block insert (Ctrl-V I / A) ----------------------------------

    /// **A block insert whose rows were joined replicates nothing** (review
    /// stop 12, T6 1) — `<C-v> j I X <End> <Del> <Esc>`: `<Del>` at the
    /// row's end joins the next row into it, and the replica went onto the
    /// row after, never the block's (here `ijkl`) — or, on a two-line
    /// buffer, past the last row: a panic from the keyboard. The block's
    /// rows are not the rows it had, so the replica is abandoned, as for a
    /// cursor that left the row.
    #[test]
    fn a_block_insert_over_joined_rows_replicates_nothing() {
        let mut p = grid_at(0, 0); // "abcd"/"efgh"/"ijkl"
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::BeginBlockInsert(BlockEdge::Left));
        type_str(&mut p, "X");
        p.apply(EditAction::Move(CursorMotion::LineEnd));
        p.apply(EditAction::DeleteForward);
        p.apply(EditAction::ClampCursorAfterInsert);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["Xabcdefgh".to_string(), "ijkl".to_string()],
            "joined, typed once, no replica"
        );
    }

    /// **A selection's anchor stays inside the buffer through a join** — the
    /// sequence the widened fuzz shrank to (review stop 12): `a`, `<CR>`,
    /// an anchor on the new row, `<BS>` joining it away. The anchor kept the
    /// removed row, for the next read of the selection to index.
    #[test]
    fn an_anchor_on_a_joined_row_stays_inside_the_buffer() {
        let mut p = Prompt::new();
        type_str(&mut p, "a");
        p.apply(EditAction::Newline);
        p.apply(EditAction::SetAnchor);
        assert_eq!(p.buf.selection_anchor, Some((1, 0)), "premise: on row 1");
        p.apply(EditAction::DeleteBack);
        assert_eq!(p.lines().count(), 1, "premise: joined");
        assert_eq!(
            p.buf.selection_anchor,
            Some((0, 0)),
            "the row clamped to the buffer, the column kept where it fits"
        );
    }

    #[test]
    fn block_insert_left_prepends_to_every_row() {
        // The killer case: `Ctrl-V` a column, `I`, type, `Esc` → every line
        // gets the text prepended (e.g. commenting out a block).
        let mut p = grid_at(0, 0); // "abcd"/"efgh"/"ijkl", cursor (0,0)
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Down)); // span rows 0..2, col 0
        p.apply(EditAction::BeginBlockInsert(BlockEdge::Left));
        assert_eq!(p.cursor(), (0, 0), "Insert starts at the left edge");
        type_str(&mut p, "// ");
        p.apply(EditAction::ClampCursorAfterInsert); // Esc finalizes
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "// abcd".to_string(),
                "// efgh".to_string(),
                "// ijkl".to_string()
            ]
        );
    }

    #[test]
    fn block_insert_right_appends_at_the_column_on_every_row() {
        let mut p = grid_at(0, 1);
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Right)); // block cols [1,3)
        p.apply(EditAction::BeginBlockInsert(BlockEdge::Right));
        assert_eq!(
            p.cursor(),
            (0, 3),
            "Insert starts past the right edge (col 3)"
        );
        type_str(&mut p, "X");
        p.apply(EditAction::ClampCursorAfterInsert);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "abcXd".to_string(),
                "efgXh".to_string(),
                "ijkXl".to_string()
            ]
        );
    }

    #[test]
    fn block_insert_is_a_single_undo_group() {
        let mut p = grid_at(0, 0);
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::BeginBlockInsert(BlockEdge::Left));
        type_str(&mut p, "X");
        p.apply(EditAction::ClampCursorAfterInsert);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "Xabcd".to_string(),
                "Xefgh".to_string(),
                "Xijkl".to_string()
            ]
        );
        // One undo reverts the typing *and* the replication.
        p.apply(EditAction::Undo);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["abcd".to_string(), "efgh".to_string(), "ijkl".to_string()],
            "block insert undoes as one group"
        );
    }

    #[test]
    fn dollar_block_insert_appends_at_each_rows_own_end() {
        // `Ctrl-V <down> $ A`: append at the END of each line — ragged, no
        // space-padding to vertically align (the reported divergence).
        let mut p = Prompt::new();
        type_str(&mut p, "abcdef"); // row 0, len 6
        p.apply(EditAction::Newline);
        type_str(&mut p, "xy"); // row 1, len 2
        p.apply(EditAction::Newline);
        type_str(&mut p, "ghij"); // row 2, len 4
        p.apply(EditAction::Move(CursorMotion::BufferStart));
        p.apply(EditAction::Move(CursorMotion::LineStart)); // (0,0)
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::Move(CursorMotion::Down)); // (2,0)
        p.apply(EditAction::Move(CursorMotion::LineEnd)); // `$` → to-eol
        p.apply(EditAction::BeginBlockInsert(BlockEdge::Right)); // `A`
        type_str(&mut p, "!");
        p.apply(EditAction::ClampCursorAfterInsert); // `Esc`
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec![
                "abcdef!".to_string(),
                "xy!".to_string(),
                "ghij!".to_string()
            ],
            "each line gets it at its own end, no padding"
        );
    }

    #[test]
    fn dollar_block_delete_removes_to_each_rows_end() {
        // `Ctrl-V <down> $ d`: delete from the column to the end of each line.
        let mut p = Prompt::new();
        type_str(&mut p, "keepXXXX");
        p.apply(EditAction::Newline);
        type_str(&mut p, "keepYY");
        p.apply(EditAction::Move(CursorMotion::BufferStart));
        p.apply(EditAction::Move(CursorMotion::LineStart));
        for _ in 0..4 {
            p.apply(EditAction::Move(CursorMotion::Right)); // (0,4), after "keep"
        }
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down)); // (1,4)
        p.apply(EditAction::Move(CursorMotion::LineEnd)); // `$`
        p.apply_operator(Operator::Delete, OpTarget::Selection, 1);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["keep".to_string(), "keep".to_string()]
        );
    }

    #[test]
    fn block_dollar_then_explicit_motion_clears_the_eol_flag() {
        // `$` then a horizontal motion picks a fixed column again (the flag
        // is sticky only until an explicit column motion).
        let mut p = Prompt::new();
        type_str(&mut p, "abcdef");
        p.apply(EditAction::Newline);
        type_str(&mut p, "xy");
        p.apply(EditAction::Move(CursorMotion::BufferStart));
        p.apply(EditAction::Move(CursorMotion::LineStart)); // (0,0)
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down)); // (1,0)
        p.apply(EditAction::Move(CursorMotion::LineEnd)); // `$` → to-eol on "xy" (col 2)
        p.apply(EditAction::Move(CursorMotion::LineStart)); // `0` → fixed column 0, clears flag
        p.apply_operator(Operator::Delete, OpTarget::Selection, 1);
        // Block is now just column 0 (cols [0,1)) on both rows — one char each.
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["bcdef".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn block_insert_empty_typing_changes_nothing() {
        // `Ctrl-V` `I` then immediately `Esc` — no text, no change.
        let mut p = grid_at(0, 0);
        p.apply(EditAction::SetAnchorBlock);
        p.apply(EditAction::Move(CursorMotion::Down));
        p.apply(EditAction::BeginBlockInsert(BlockEdge::Left));
        p.apply(EditAction::ClampCursorAfterInsert);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["abcd".to_string(), "efgh".to_string(), "ijkl".to_string()]
        );
    }

    // --- yank / paste (register) --------------------------------------

    #[test]
    fn yank_word_then_paste_inserts_after_cursor() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo bar");
        p.apply(EditAction::Move(CursorMotion::LineStart)); // on 'f'
        // `yw` yanks "foo " (charwise), buffer unchanged, cursor at start.
        p.apply_operator(
            Operator::Yank,
            OpTarget::Motion(CursorMotion::WordForward),
            1,
        );
        assert_eq!(p.lines().next().unwrap(), "foo bar", "yank does not mutate");
        assert_eq!(p.cursor(), (0, 0), "cursor parks at range start");
        // `p` pastes after the cursor char → "ffoo oo bar"? No: insert after
        // 'f' (col 1). Register "foo " → "f" + "foo " + "oo bar".
        p.apply(EditAction::Paste);
        assert_eq!(p.lines().next().unwrap(), "ffoo oo bar");
    }

    #[test]
    fn yank_does_not_create_an_undo_step() {
        let mut p = Prompt::new();
        type_str(&mut p, "hello"); // one typing run
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_operator(Operator::Yank, OpTarget::Motion(CursorMotion::LineEnd), 1);
        // The only undoable thing is the typing run; yank added nothing.
        p.apply(EditAction::Undo);
        assert_eq!(
            p.lines().next().unwrap(),
            "",
            "undo removes the typing, not a yank step"
        );
    }

    #[test]
    fn linewise_yank_paste_duplicates_line_below() {
        let mut p = Prompt::new();
        type_str(&mut p, "one");
        p.apply(EditAction::Newline);
        type_str(&mut p, "two"); // cursor on "two"
        // `yy` then `p` duplicates the line below (the vim reflex).
        p.apply_operator(Operator::Yank, OpTarget::Line, 1);
        p.apply(EditAction::Paste);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["one".to_string(), "two".to_string(), "two".to_string()]
        );
        assert_eq!(p.cursor(), (2, 0), "cursor on the pasted line");
    }

    #[test]
    fn linewise_paste_before_opens_line_above() {
        let mut p = Prompt::new();
        type_str(&mut p, "a");
        p.apply(EditAction::Newline);
        type_str(&mut p, "b"); // on "b" (row 1)
        p.apply_operator(Operator::Yank, OpTarget::Line, 1); // yank "b"
        p.apply(EditAction::PasteBefore); // P → above row 1
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string(), "b".to_string()]
        );
        assert_eq!(p.cursor(), (1, 0), "cursor on the line pasted above");
    }

    #[test]
    fn linewise_paste_lands_caret_on_first_non_blank() {
        // vim: the caret lands on the first non-blank char of the pasted line,
        // not column 0 — matters for indented lines.
        let mut p = Prompt::new();
        type_str(&mut p, "    foo"); // 4-space indent
        p.apply_operator(Operator::Yank, OpTarget::Line, 1);
        p.apply(EditAction::Paste);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["    foo".to_string(), "    foo".to_string()]
        );
        assert_eq!(
            p.cursor(),
            (1, 4),
            "caret on 'f' (first non-blank), not col 0"
        );
    }

    /// An all-blank pasted line has no non-blank char: vim puts the caret on
    /// its LAST char (`beginline(BL_WHITE | BL_FIX)`; nvim 0.12.6, a
    /// linewise `p` of four spaces → col 3). This said col 0 until the rule
    /// became one (`Buffer::first_non_blank`).
    #[test]
    fn linewise_paste_of_an_all_blank_line_lands_on_its_last_char() {
        let mut p = Prompt::new();
        type_str(&mut p, "    ");
        p.apply_operator(Operator::Yank, OpTarget::Line, 1);
        p.apply(EditAction::Paste);
        assert_eq!(p.cursor(), (1, 3), "all-blank pasted line → its last char");
    }

    #[test]
    fn charwise_paste_before_inserts_at_cursor() {
        let mut p = Prompt::new();
        type_str(&mut p, "ab");
        p.apply(EditAction::Move(CursorMotion::LineStart)); // on 'a'
        // Yank just 'a' via a visual-style single-char selection.
        p.apply(EditAction::SetAnchor);
        p.apply_operator(Operator::Yank, OpTarget::Selection, 1); // register "a"
        // Cursor parked at (0,0); `P` inserts "a" at the cursor → "aab".
        p.apply(EditAction::PasteBefore);
        assert_eq!(p.lines().next().unwrap(), "aab");
    }

    #[test]
    fn counted_charwise_paste_repeats() {
        let mut p = Prompt::new();
        type_str(&mut p, "x");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::SetAnchor);
        p.apply_operator(Operator::Yank, OpTarget::Selection, 1); // register "x"
        // `3p` pastes three contiguous copies after the cursor.
        p.apply_counted(EditAction::Paste, 3);
        assert_eq!(p.lines().next().unwrap(), "xxxx");
        // ...and undoes as a single group (one gesture).
        p.apply(EditAction::Undo);
        assert_eq!(p.lines().next().unwrap(), "x", "3p is one undo step");
    }

    #[test]
    fn visual_yank_across_lines_round_trips_through_paste() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo");
        p.apply(EditAction::Newline);
        type_str(&mut p, "bar");
        // Select from (0,1) down to (1,1): "oo\nba".
        p.apply(EditAction::Move(CursorMotion::BufferStart));
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::Move(CursorMotion::Right)); // (0,1)
        p.apply(EditAction::SetAnchor);
        p.apply(EditAction::Move(CursorMotion::Down)); // (1,1)
        p.apply_operator(Operator::Yank, OpTarget::Selection, 1);
        // Buffer untouched; register holds the cross-row charwise text.
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["foo".to_string(), "bar".to_string()],
            "yank leaves the buffer"
        );
        assert_eq!(p.anchor(), None, "selection consumed");
        // Paste before at the parked cursor (0,1) re-inserts "oo\nba",
        // splitting the row: "f" + "oo" / "ba" + "oo".
        p.apply(EditAction::PasteBefore);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["foo".to_string(), "baoo".to_string(), "bar".to_string()]
        );
    }

    #[test]
    fn delete_cuts_into_register_so_ddp_swaps_lines() {
        let mut p = Prompt::new();
        type_str(&mut p, "one");
        p.apply(EditAction::Newline);
        type_str(&mut p, "two");
        p.apply(EditAction::Move(CursorMotion::BufferStart)); // onto "one"
        // `dd` cuts "one" (linewise) into the register, leaving "two".
        p.apply_operator(Operator::Delete, OpTarget::Line, 1);
        assert_eq!(p.lines().collect::<Vec<_>>(), vec!["two".to_string()]);
        // `p` pastes it below → the classic `ddp` line swap.
        p.apply(EditAction::Paste);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["two".to_string(), "one".to_string()]
        );
    }

    #[test]
    fn x_cuts_into_register_so_xp_transposes() {
        // `x` is `dl` at the prompt level: delete the char, register it.
        let mut p = Prompt::new();
        type_str(&mut p, "ab");
        p.apply(EditAction::Move(CursorMotion::LineStart)); // on 'a'
        p.apply_operator(Operator::Delete, OpTarget::Motion(CursorMotion::Right), 1);
        assert_eq!(p.lines().next().unwrap(), "b", "x deleted 'a'");
        // `p` pastes 'a' after the cursor char → "ba" (transpose).
        p.apply(EditAction::Paste);
        assert_eq!(p.lines().next().unwrap(), "ba");
    }

    /// `dw` on the LAST word of a line deletes forwards to the end of it — the
    /// one thing stopping it from deleting **backwards**.
    ///
    /// `w` from the last word lands on `(row+1, 0)`. Charwise operators work
    /// within one row, so the target column has to be clamped to this row's
    /// end; without the clamp the raw `0` reads as "target is left of the
    /// cursor", the span inverts to `(0, cursor)`, and `dw` eats the start of
    /// the line instead. Reviewers mutation-verified that the guard had no
    /// test — and said its twin (the lazy `then` in `visible_window`) had
    /// one, which it did not until review stop 12 (T6 4): exactly how a
    /// guard gets written and then forgotten
    /// (`[[feedback_mutation_check_guards]]`).
    #[test]
    fn dw_at_the_end_of_a_line_deletes_forwards_not_backwards() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo bar");
        p.apply(EditAction::Newline);
        type_str(&mut p, "next line");
        // Park on "bar" — the last word of row 0, with a row below it.
        p.apply(EditAction::Move(CursorMotion::Up));
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::Move(CursorMotion::WordForward)); // (0, 4) on "bar"

        p.apply_operator(
            Operator::Delete,
            OpTarget::Motion(CursorMotion::WordForward),
            1,
        );

        let rows: Vec<String> = p.lines().collect();
        assert_eq!(
            rows[0], "foo ",
            "deleted `bar` — forwards to the line end. Backwards would leave `bar`"
        );
        assert_eq!(rows[1], "next line", "and the row below is untouched");
    }

    #[test]
    fn delete_word_cuts_and_pastes_back() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo bar");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        // `dw` cuts "foo " (charwise) into the register, leaving "bar".
        p.apply_operator(
            Operator::Delete,
            OpTarget::Motion(CursorMotion::WordForward),
            1,
        );
        assert_eq!(p.lines().next().unwrap(), "bar");
        // Park at end, paste it back → "barfoo " (cursor was at 0; p after).
        p.apply(EditAction::Move(CursorMotion::LineEnd));
        p.apply(EditAction::Paste);
        assert_eq!(p.lines().next().unwrap(), "barfoo ");
    }

    #[test]
    fn visual_delete_cuts_into_register() {
        let mut p = Prompt::new();
        type_str(&mut p, "hello");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::SetAnchor);
        for _ in 0..4 {
            p.apply(EditAction::Move(CursorMotion::Right)); // select "hello"
        }
        p.apply_operator(Operator::Delete, OpTarget::Selection, 1);
        assert_eq!(p.lines().next().unwrap(), "", "selection deleted");
        p.apply(EditAction::Paste);
        assert_eq!(
            p.lines().next().unwrap(),
            "hello",
            "visual d then p restores it"
        );
    }

    #[test]
    fn paste_with_no_register_is_a_noop() {
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        p.apply(EditAction::Paste);
        assert_eq!(
            p.lines().next().unwrap(),
            "abc",
            "nothing yanked → nothing pasted"
        );
    }

    #[test]
    fn empty_yank_keeps_the_prior_register() {
        let mut p = Prompt::new();
        type_str(&mut p, "kept");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply_operator(Operator::Yank, OpTarget::Motion(CursorMotion::LineEnd), 1); // "kept"
        // A FAILED yank (`yi(` with no parens: no shape) must not wipe it.
        // An empty one does, as vim's does —
        // `an_empty_yank_empties_the_register_an_empty_cut_does_not`.
        p.apply_operator(
            Operator::Yank,
            OpTarget::TextObject {
                scope: TextObjectScope::Inner,
                object: TextObject::Pair('(', ')'),
            },
            1,
        );
        p.apply(EditAction::Move(CursorMotion::LineEnd));
        p.apply(EditAction::Paste);
        assert_eq!(
            p.lines().next().unwrap(),
            "keptkept",
            "register survived the empty yank"
        );
    }

    #[test]
    fn register_text_exposes_the_yanked_text() {
        let mut p = Prompt::new();
        type_str(&mut p, "hello");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        assert_eq!(p.register_text().as_deref(), None, "empty until a yank/cut");
        p.apply_operator(Operator::Yank, OpTarget::Motion(CursorMotion::LineEnd), 1);
        assert_eq!(p.register_text().as_deref(), Some("hello"));
    }

    #[test]
    fn insert_pasted_single_line_lands_cursor_after_text() {
        let mut p = Prompt::new();
        type_str(&mut p, "ad");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::Move(CursorMotion::Right)); // between 'a' and 'd'
        p.insert_pasted("bc");
        assert_eq!(p.lines().next().unwrap(), "abcd");
        assert_eq!(
            p.cursor(),
            (0, 3),
            "cursor rests after the paste (insert, not vim p)"
        );
    }

    #[test]
    fn insert_pasted_multiline_splits_and_is_one_undo() {
        let mut p = Prompt::new();
        type_str(&mut p, "ad");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::Move(CursorMotion::Right)); // col 1
        p.insert_pasted("X\nY");
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["aX".to_string(), "Yd".to_string()]
        );
        assert_eq!(p.cursor(), (1, 1), "cursor after the last pasted char");
        // The whole multi-line paste is a single undo group.
        p.apply(EditAction::Undo);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            vec!["ad".to_string()],
            "bracketed paste undoes in one step"
        );
    }

    #[test]
    fn insert_pasted_empty_is_a_noop() {
        let mut p = Prompt::new();
        type_str(&mut p, "ab");
        p.insert_pasted("");
        assert_eq!(p.lines().next().unwrap(), "ab");
        // No undo step was created.
        p.apply(EditAction::Undo);
        assert_eq!(p.lines().next().unwrap(), "", "only the typing run undoes");
    }

    // --- text objects -------------------------------------------------

    /// Apply an operator + text object, returning the resulting first
    /// line. Cursor is placed at `col` first.
    fn obj_line(text: &str, col: usize, scope: TextObjectScope, object: TextObject) -> String {
        let mut p = Prompt::new();
        type_str(&mut p, text);
        p.apply(EditAction::Move(CursorMotion::LineStart));
        for _ in 0..col {
            p.apply(EditAction::Move(CursorMotion::Right));
        }
        p.apply_operator(Operator::Delete, OpTarget::TextObject { scope, object }, 1);
        p.lines().next().unwrap()
    }

    #[test]
    fn text_object_inner_word() {
        // diw from inside "bar" removes just the word.
        assert_eq!(
            obj_line("foo bar baz", 5, TextObjectScope::Inner, TextObject::Word),
            "foo  baz"
        );
    }

    #[test]
    fn text_object_a_word_takes_trailing_space() {
        // daw removes the word and its trailing whitespace.
        assert_eq!(
            obj_line("foo bar baz", 4, TextObjectScope::Around, TextObject::Word),
            "foo baz"
        );
    }

    #[test]
    fn text_object_inner_and_around_quotes() {
        // ci" / di" on `say "hi" now` — inner leaves the quotes.
        assert_eq!(
            obj_line(
                "say \"hi\" now",
                5,
                TextObjectScope::Inner,
                TextObject::Quote('"')
            ),
            "say \"\" now"
        );
        // around removes the quotes too, and the space after — vim's
        // `say now`. This asserted `say  now`, the code's, not vim's (review
        // stop 12, T6 5); the white-space cases are measured in
        // `app::tests::vim_fidelity`.
        assert_eq!(
            obj_line(
                "say \"hi\" now",
                5,
                TextObjectScope::Around,
                TextObject::Quote('"')
            ),
            "say now"
        );
    }

    #[test]
    fn text_object_quotes_from_before_the_pair() {
        // Cursor before the opening quote still finds the pair.
        assert_eq!(
            obj_line(
                "a \"x\" b",
                0,
                TextObjectScope::Inner,
                TextObject::Quote('"')
            ),
            "a \"\" b"
        );
    }

    #[test]
    fn text_object_inner_and_around_parens() {
        // ci( / ca( on `f(a, b)`.
        assert_eq!(
            obj_line(
                "f(a, b)",
                3,
                TextObjectScope::Inner,
                TextObject::Pair('(', ')')
            ),
            "f()"
        );
        assert_eq!(
            obj_line(
                "f(a, b)",
                3,
                TextObjectScope::Around,
                TextObject::Pair('(', ')')
            ),
            "f"
        );
    }

    #[test]
    fn text_object_parens_respect_nesting() {
        // Cursor in the inner pair operates on the innermost enclosing one.
        assert_eq!(
            obj_line(
                "(a (b) c)",
                4,
                TextObjectScope::Inner,
                TextObject::Pair('(', ')')
            ),
            "(a () c)"
        );
        // Cursor in the outer region (not inside the inner pair) takes the
        // outer pair.
        assert_eq!(
            obj_line(
                "(a (b) c)",
                1,
                TextObjectScope::Inner,
                TextObject::Pair('(', ')')
            ),
            "()"
        );
    }

    #[test]
    fn text_object_no_surrounding_pair_is_noop() {
        // di( with no parens on the line changes nothing.
        assert_eq!(
            obj_line(
                "no parens here",
                3,
                TextObjectScope::Inner,
                TextObject::Pair('(', ')')
            ),
            "no parens here"
        );
    }

    #[test]
    fn delete_back_at_origin_is_noop() {
        let mut p = Prompt::new();
        p.apply(EditAction::DeleteBack);
        assert_eq!(p.cursor(), (0, 0));
        assert_eq!(p.line_count(), 1);
    }

    #[test]
    fn delete_back_at_line_start_merges_with_previous() {
        let mut p = Prompt::new();
        type_str(&mut p, "ab");
        p.apply(EditAction::Newline);
        type_str(&mut p, "cd");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        p.apply(EditAction::DeleteBack);
        assert_eq!(p.line_count(), 1);
        assert_eq!(p.cursor(), (0, 2));
        assert_eq!(p.lines().collect::<Vec<_>>(), vec!["abcd".to_string()]);
    }

    #[test]
    fn delete_forward_at_line_end_merges_next_line_up() {
        let mut p = Prompt::new();
        type_str(&mut p, "ab");
        p.apply(EditAction::Newline);
        type_str(&mut p, "cd");
        p.apply(EditAction::Move(CursorMotion::Up));
        p.apply(EditAction::Move(CursorMotion::LineEnd));
        p.apply(EditAction::DeleteForward);
        assert_eq!(p.line_count(), 1);
        assert_eq!(p.lines().collect::<Vec<_>>(), vec!["abcd".to_string()]);
    }

    #[test]
    fn clamp_after_insert_steps_onto_last_typed_char() {
        // Vim `<Esc>`: typing "abcd" leaves the caret at col 4 (past 'd');
        // leaving Insert nudges it back onto 'd' (col 3).
        let mut p = Prompt::new();
        type_str(&mut p, "abcd");
        assert_eq!(p.cursor(), (0, 4), "Insert caret sits after the last char");
        p.apply(EditAction::ClampCursorAfterInsert);
        assert_eq!(p.cursor(), (0, 3), "nudged back onto 'd'");
    }

    #[test]
    fn clamp_after_insert_at_column_zero_stays_put() {
        // At col 0 the nudge is a no-op — and never wraps to the line above.
        let mut p = Prompt::new();
        type_str(&mut p, "ab");
        p.apply(EditAction::Newline); // cursor at (1, 0)
        assert_eq!(p.cursor(), (1, 0));
        p.apply(EditAction::ClampCursorAfterInsert);
        assert_eq!(
            p.cursor(),
            (1, 0),
            "stays at col 0, no wrap to the line above"
        );
    }

    #[test]
    fn arrow_actions_navigate_within_line() {
        let mut p = Prompt::new();
        type_str(&mut p, "hi");
        assert_eq!(p.cursor(), (0, 2));
        p.apply(EditAction::Move(CursorMotion::Left));
        assert_eq!(p.cursor(), (0, 1));
        p.apply(EditAction::Move(CursorMotion::Right));
        assert_eq!(p.cursor(), (0, 2));
    }

    /// Up and Down clamp to a shorter line — and come back to the column on
    /// a longer one: vim's sticky column (`curswant`). M1 pinned the clamp
    /// alone ("no sticky-column semantics"); Steven, 10-27, Q5: "Match vim".
    #[test]
    fn up_and_down_clamp_and_come_back_to_the_column() {
        let mut p = Prompt::new();
        // Short line first so that moving Up from a longer line below
        // actually has to clamp.
        type_str(&mut p, "hi");
        p.apply(EditAction::Newline);
        type_str(&mut p, "long_line"); // cursor at (1, 9)
        p.apply(EditAction::Move(CursorMotion::Up));
        assert_eq!(p.cursor(), (0, 2));
        p.apply(EditAction::Move(CursorMotion::Down));
        assert_eq!(p.cursor(), (1, 9), "back to the column it came from");
    }

    #[test]
    fn home_and_end_jump_within_line() {
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        p.apply(EditAction::Move(CursorMotion::LineStart));
        assert_eq!(p.cursor(), (0, 0));
        p.apply(EditAction::Move(CursorMotion::LineEnd));
        assert_eq!(p.cursor(), (0, 3));
    }

    #[test]
    fn delete_word_back_removes_one_word_and_trailing_space() {
        let mut p = Prompt::new();
        type_str(&mut p, "hello world");
        p.apply(EditAction::DeleteWordBack);
        assert_eq!(p.lines().next().unwrap(), "hello ");
        assert_eq!(p.cursor(), (0, 6));
    }

    #[test]
    fn delete_word_back_skips_multiple_trailing_spaces() {
        let mut p = Prompt::new();
        type_str(&mut p, "foo   bar");
        // After "foo   bar", we expect Ctrl-W to delete "bar" only;
        // the spaces between are part of an earlier word's run.
        p.apply(EditAction::DeleteWordBack);
        assert_eq!(p.lines().next().unwrap(), "foo   ");

        // Pressing again deletes the run of spaces *and* "foo".
        p.apply(EditAction::DeleteWordBack);
        assert_eq!(p.lines().next().unwrap(), "");
        assert_eq!(p.cursor(), (0, 0));
    }

    #[test]
    fn visible_window_empty_buffer_renders_one_row() {
        let p = Prompt::new();
        let w = p.visible_window(20, 10);
        assert_eq!(w.rows, vec![String::new()]);
        assert_eq!(w.cursor_visual, (0, 0));
        assert_eq!(w.total_rows, 1);
        assert_eq!(w.top, 0);
    }

    #[test]
    fn visible_window_short_line_fits_without_wrap() {
        let mut p = Prompt::new();
        type_str(&mut p, "abc");
        let w = p.visible_window(20, 10);
        assert_eq!(w.rows, vec!["abc"]);
        assert_eq!(w.cursor_visual, (0, 3));
    }

    #[test]
    fn visible_window_wraps_long_line_to_multiple_rows() {
        let mut p = Prompt::new();
        type_str(&mut p, "0123456789abcdefghij"); // 20 chars
        let w = p.visible_window(10, 10);
        // Wraps to 2 rows of 10. Cursor at col 20 lands on a phantom
        // continuation row.
        assert_eq!(w.rows, vec!["0123456789", "abcdefghij", ""]);
        assert_eq!(w.cursor_visual, (2, 0));
        assert_eq!(w.total_rows, 3);
    }

    #[test]
    fn visible_window_cursor_mid_wrap_has_no_phantom() {
        let mut p = Prompt::new();
        type_str(&mut p, "0123456789abcde"); // 15 chars
        // Cursor at col 15, width 10 -> wraps to 2 rows; cursor at
        // (1, 5), no phantom row needed.
        let w = p.visible_window(10, 10);
        assert_eq!(w.rows, vec!["0123456789", "abcde"]);
        assert_eq!(w.cursor_visual, (1, 5));
        assert_eq!(w.total_rows, 2);
    }

    #[test]
    fn full_width_interior_line_does_not_splice_phantom_row() {
        // Regression: a logical line that exactly fills the width, with the
        // cursor at its end and another line below, must NOT inject a blank
        // continuation row between them (the phantom is only for the end of
        // the whole buffer).
        let mut p = Prompt::new();
        type_str(&mut p, "0123"); // exactly width 4
        p.apply(EditAction::Newline);
        type_str(&mut p, "xy");
        p.apply(EditAction::Move(CursorMotion::Up));
        p.apply(EditAction::Move(CursorMotion::LineEnd)); // cursor at (0, 4)
        let w = p.visible_window(4, 10);
        assert_eq!(w.rows, vec!["0123".to_string(), "xy".to_string()]);
        assert_eq!(w.total_rows, 2, "no spurious blank row");
        assert_eq!(w.cursor_visual, (1, 0));
    }

    #[test]
    fn visible_window_slides_to_keep_cursor_visible() {
        let mut p = Prompt::new();
        for _ in 0..15 {
            type_str(&mut p, "x");
            p.apply(EditAction::Newline);
        }
        // 15 single-char lines + empty trailing line = 16 logical lines.
        // Each wraps to 1 visual row. Cursor at (15, 0) -> visual (15, 0).
        let w = p.visible_window(20, 10);
        assert_eq!(w.total_rows, 16);
        assert_eq!(w.top, 6);
        assert_eq!(w.rows.len(), 10);
        assert_eq!(w.cursor_visual, (9, 0)); // bottom of the window
        assert_eq!(w.rows.last().unwrap(), ""); // cursor's row
        assert_eq!(w.rows.first().unwrap(), "x"); // line 6
    }

    #[test]
    fn visible_window_wrapped_line_can_overflow_height_alone() {
        let mut p = Prompt::new();
        // Single 50-char line at width 10 wraps to 5 visual rows;
        // height 3 must slide so cursor (at end) stays visible.
        for _ in 0..50 {
            p.apply(EditAction::Insert('x'));
        }
        let w = p.visible_window(10, 3);
        // Cursor at col 50 -> visual (5, 0) (phantom row).
        assert_eq!(w.total_rows, 6);
        assert_eq!(w.top, 3);
        assert_eq!(w.cursor_visual, (2, 0));
        assert_eq!(w.rows.len(), 3);
        assert_eq!(w.rows.last().unwrap(), ""); // phantom continuation
    }

    #[test]
    fn visible_window_zero_dim_returns_empty() {
        let mut p = Prompt::new();
        type_str(&mut p, "anything");
        let w0 = p.visible_window(0, 10);
        assert!(w0.rows.is_empty());
        assert_eq!(w0.total_rows, 0);
        let h0 = p.visible_window(10, 0);
        assert!(h0.rows.is_empty());
    }

    #[test]
    fn visible_window_multibyte_chars_do_not_break_wrap() {
        let mut p = Prompt::new();
        // 10 Greek chars (each 1 cell); width 4 -> 3 rows of 4/4/2 chars.
        type_str(&mut p, "αβγδεζηθικ");
        let w = p.visible_window(4, 10);
        assert_eq!(w.rows.len(), 3);
        assert_eq!(w.rows[0].chars().count(), 4);
        assert_eq!(w.rows[2].chars().count(), 2);
    }

    #[test]
    fn visible_window_wide_chars_wrap_by_cells_and_place_cursor() {
        let mut p = Prompt::new();
        // 3 CJK chars, 2 cells each (6 cells). Cursor at char col 3 (end).
        type_str(&mut p, "中文字");
        let w = p.visible_window(4, 10);
        // Width 4 cells: "中文" (4 cells) then "字" (2 cells).
        assert_eq!(w.rows, vec!["中文", "字"]);
        // Cursor after the 3rd char: row 1, cell column = width("字") = 2.
        assert_eq!(w.cursor_visual, (1, 2));
    }

    #[test]
    fn visible_window_cursor_cell_column_counts_wide_chars() {
        let mut p = Prompt::new();
        // Cells: a=1, 中=2, b=1. Cursor at char col 3 (end of line).
        type_str(&mut p, "a中b");
        let w = p.visible_window(20, 10);
        assert_eq!(w.rows, vec!["a中b"]);
        // Cursor cell column = 1 + 2 + 1 = 4 (not the char count 3).
        assert_eq!(w.cursor_visual, (0, 4));
    }

    #[test]
    fn delete_word_back_at_line_start_merges_with_previous() {
        let mut p = Prompt::new();
        type_str(&mut p, "first");
        p.apply(EditAction::Newline);
        // At (1, 0): no word on this line, so the call should fall
        // through to merging with the previous line.
        p.apply(EditAction::DeleteWordBack);
        assert_eq!(p.line_count(), 1);
        assert_eq!(p.cursor(), (0, 5));
        assert_eq!(p.lines().next().unwrap(), "first");
    }

    /// Visual `~`/`r` leave the cursor at the range's start, ON a char — a
    /// block whose first row is shorter than its left edge starts past that
    /// row's end, and the buffer clamps it itself rather than leaning on the
    /// app's later clamp (Reviewer A, round 19: deleting it passed).
    #[test]
    fn a_shape_start_is_clamped_onto_a_char() {
        let mut b = Buffer::from_lines(vec!["ab".into(), "abcdef".into()]);
        b.move_cursor_to_shape_start(RangeShape::Block {
            rows: (0, 1),
            cols: (4, 5),
        });
        assert_eq!(b.cursor(), (0, 1));
        b.move_cursor_to_shape_start(RangeShape::Lines { first: 1, last: 1 });
        assert_eq!(b.cursor(), (1, 0), "linewise: the line's start, as vim");
    }

    /// A prompt holding `text` with `R` begun at `col` on its first line and
    /// `typed` replaced in — the start of every stale-session case below.
    fn replacing(text: &str, col: usize, count: usize, typed: &str) -> Prompt {
        let mut p = Prompt::new();
        p.replace_with(text);
        p.place_cursor(0, col);
        p.apply(EditAction::EnterReplace { count });
        for c in typed.chars() {
            p.apply(EditAction::ReplaceType(c));
        }
        p
    }

    /// **A Replace session that something else moved restarts — whatever
    /// moved it.** `<BS>` put a remembered char wherever the cursor had gone:
    /// `Rxy`, a click, `<BS>` turned `abcdef` into `xycdbf` (Reviewers A, B,
    /// C, round 19). Each door below moves the cursor or rewrites the line
    /// without being a Replace keystroke; after each, `<BS>` only steps left,
    /// as vim's does after a move, and `<Esc>` replays nothing.
    #[test]
    fn a_replace_session_restarts_after_anything_else_moves_it() {
        let text = |p: &Prompt| p.lines().collect::<Vec<_>>().join("\n");

        // A click.
        let mut p = replacing("abcdef", 0, 1, "xy");
        p.place_cursor(0, 5);
        p.apply(EditAction::ReplaceBack);
        assert_eq!((text(&p).as_str(), p.cursor()), ("xycdef", (0, 4)), "click");

        // A click to column 0 — the step-left has nowhere to go, and nothing
        // underflows (B's mutant panicked here).
        let mut p = replacing("abcdef", 0, 1, "x");
        p.place_cursor(0, 0);
        p.apply(EditAction::ReplaceBack);
        assert_eq!((text(&p).as_str(), p.cursor()), ("xbcdef", (0, 0)), "col 0");

        // A paste.
        let mut p = replacing("abcdef", 0, 1, "xy");
        p.insert_pasted("123");
        p.apply(EditAction::ReplaceBack);
        assert_eq!(text(&p), "xy123cdef", "paste");

        // An `$EDITOR` result that leaves the cursor exactly where the session
        // did — the line it was typing is what tells them apart.
        let mut p = replacing("abc", 3, 1, "xy");
        assert_eq!(p.cursor(), (0, 5), "premise: appended to the end");
        p.replace_with("hello");
        assert_eq!(p.cursor(), (0, 5), "premise: the same cursor");
        p.apply(EditAction::ReplaceBack);
        assert_eq!(text(&p), "hello", "editor");

        // An undo.
        let mut p = replacing("abcdef", 2, 1, "xy");
        p.apply(EditAction::Undo);
        p.apply(EditAction::ReplaceBack);
        assert_eq!(text(&p), "abcdef", "undo");

        // A counted session moved elsewhere replays nothing there.
        let mut p = replacing("abcdef", 0, 2, "xy");
        p.place_cursor(0, 4);
        p.apply(EditAction::LeaveReplace);
        assert_eq!(text(&p), "xycdef", "no replay at the click");

        // And one left alone still puts back what it covered.
        let mut p = replacing("abcdef", 0, 1, "xy");
        p.apply(EditAction::ReplaceBack);
        assert_eq!(text(&p), "xbcdef", "premise: the session works");
    }

    /// **A submit from Replace mode restarts the session** — the mode stays
    /// (it is the keymap's), and what was sent must not replay into the next
    /// prompt: `3R`, `xy`, submit, `hi<Esc>` gave `hixyhixyhi` with the reset
    /// deleted (Reviewer B, round 19). Typing on still overwrites and `<BS>`
    /// still puts back.
    #[test]
    fn a_submit_restarts_the_replace_session() {
        let mut p = replacing("abcdef", 0, 3, "xy");
        assert_eq!(p.take(), "xycdef");
        for c in "hi".chars() {
            p.apply(EditAction::ReplaceType(c));
        }
        p.apply(EditAction::LeaveReplace);
        assert_eq!(p.lines().collect::<Vec<_>>(), ["hi"]);

        let mut p = replacing("abc", 0, 1, "x");
        p.take();
        p.replace_with("pqr");
        p.place_cursor(0, 0);
        p.apply(EditAction::ReplaceType('z'));
        p.apply(EditAction::ReplaceBack);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            ["pqr"],
            "puts back after a submit"
        );

        // The one shape `replace_session`'s own check cannot tell from a
        // submit: a session whose last key left an EMPTY line at the origin —
        // just what `take` leaves. Only the restart in `take` drops its count.
        let mut p = replacing("", 0, 3, "x");
        p.apply(EditAction::ReplaceBack);
        assert_eq!(p.take(), "");
        p.apply(EditAction::ReplaceType('h'));
        p.apply(EditAction::LeaveReplace);
        assert_eq!(
            p.lines().collect::<Vec<_>>(),
            ["h"],
            "no count after a submit"
        );
    }

    // --- property tests (criterion 13: invariants, not just examples) -------
    mod props {
        use super::*;
        use proptest::prelude::*;

        /// A printable char — mostly ASCII, with a wide CJK char and an emoji
        /// mixed in to keep the display-cell / char-index distinction honest.
        fn arb_char() -> impl Strategy<Value = char> {
            prop_oneof![
                8 => proptest::char::range('a', 'z'),
                2 => proptest::char::range('A', 'Z'),
                1 => Just('世'),
                1 => Just('🦀'),
                1 => Just(' '),
            ]
        }

        /// Every motion — the WORD, section and screen ones too (review stop
        /// 12, T6 1: the generator held eleven, and the crash it missed
        /// was in what it left out).
        fn arb_motion() -> impl Strategy<Value = CursorMotion> {
            use crate::viewport::ScreenLine;
            prop_oneof![
                Just(CursorMotion::Left),
                Just(CursorMotion::Right),
                Just(CursorMotion::Up),
                Just(CursorMotion::Down),
                Just(CursorMotion::LineStart),
                Just(CursorMotion::LineEnd),
                Just(CursorMotion::WordForward),
                Just(CursorMotion::WordBackward),
                Just(CursorMotion::WordEnd),
                Just(CursorMotion::BigWordForward),
                Just(CursorMotion::BigWordBackward),
                Just(CursorMotion::BigWordEnd),
                Just(CursorMotion::BufferStart),
                Just(CursorMotion::BufferEnd),
                (1usize..20).prop_map(CursorMotion::GotoLine),
                (any::<bool>(), any::<bool>(), 1usize..4).prop_map(|(forward, end, count)| {
                    CursorMotion::Section {
                        forward,
                        end,
                        count,
                    }
                }),
                prop_oneof![
                    (0usize..3).prop_map(ScreenLine::Top),
                    Just(ScreenLine::Middle),
                    (0usize..3).prop_map(ScreenLine::Bottom),
                ]
                .prop_map(CursorMotion::Screen),
            ]
        }

        fn arb_object() -> impl Strategy<Value = (TextObjectScope, TextObject)> {
            let scope = prop_oneof![Just(TextObjectScope::Inner), Just(TextObjectScope::Around)];
            let object = prop_oneof![
                Just(TextObject::Word),
                Just(TextObject::BigWord),
                Just(TextObject::Quote('"')),
                Just(TextObject::Quote('\'')),
                Just(TextObject::Pair('(', ')')),
                Just(TextObject::Pair('{', '}')),
                Just(TextObject::Paragraph),
            ];
            (scope, object)
        }

        /// What an operator acts over — every `OpTarget`.
        fn arb_target() -> impl Strategy<Value = OpTarget> {
            prop_oneof![
                arb_motion().prop_map(OpTarget::Motion),
                Just(OpTarget::Line),
                arb_motion().prop_map(OpTarget::LineMotion),
                (1usize..4).prop_map(OpTarget::LineTail),
                arb_object().prop_map(|(scope, object)| OpTarget::TextObject { scope, object }),
                Just(OpTarget::Selection),
            ]
        }

        fn arb_operator() -> impl Strategy<Value = Operator> {
            prop_oneof![
                Just(Operator::Delete),
                Just(Operator::Change),
                Just(Operator::Yank),
                Just(Operator::Case(CaseOp::Toggle)),
                Just(Operator::Case(CaseOp::Lower)),
                Just(Operator::Case(CaseOp::Upper)),
                arb_char().prop_map(Operator::Replace),
            ]
        }

        /// One step of the fuzz: an edit, or an operator over a target —
        /// `apply_operator` is a door of its own, which the fuzz never
        /// walked.
        #[derive(Debug, Clone)]
        enum Step {
            Edit(EditAction),
            Op(Operator, OpTarget, usize),
        }

        fn arb_step() -> impl Strategy<Value = Step> {
            let entry = prop_oneof![
                Just(InsertEntry::Before),
                Just(InsertEntry::After),
                Just(InsertEntry::LineStart),
                Just(InsertEntry::LineEnd),
                Just(InsertEntry::OpenBelow),
                Just(InsertEntry::OpenAbove),
            ];
            let kind = prop_oneof![
                Just(SelectionKind::Char),
                Just(SelectionKind::Line),
                Just(SelectionKind::Block),
            ];
            prop_oneof![
                10 => arb_action().prop_map(Step::Edit),
                2 => (arb_operator(), arb_target(), 1usize..4)
                    .prop_map(|(op, target, count)| Step::Op(op, target, count)),
                1 => prop_oneof![Just(BlockEdge::Left), Just(BlockEdge::Right)]
                    .prop_map(|e| Step::Edit(EditAction::BeginBlockInsert(e))),
                1 => Just(Step::Edit(EditAction::ClampCursorAfterInsert)),
                1 => (entry, 1usize..3)
                    .prop_map(|(entry, count)| Step::Edit(EditAction::EnterInsert { entry, count })),
                1 => (arb_object(), 1usize..3).prop_map(|((scope, object), count)| {
                    Step::Edit(EditAction::SelectTextObject { scope, object, count })
                }),
                1 => kind.prop_map(|k| Step::Edit(EditAction::SetSelectionKind(k))),
                1 => Just(Step::Edit(EditAction::SwapSelectionEnds)),
            ]
        }

        /// A buffer-affecting edit — *excluding* Undo/Redo, so the undo-stack
        /// symmetry property starts from an empty redo stack.
        fn arb_edit() -> impl Strategy<Value = EditAction> {
            prop_oneof![
                5 => arb_char().prop_map(EditAction::Insert),
                2 => Just(EditAction::Newline),
                2 => Just(EditAction::DeleteBack),
                1 => Just(EditAction::DeleteForward),
                1 => Just(EditAction::DeleteWordBack),
                1 => Just(EditAction::DeleteToLineStart),
                3 => arb_motion().prop_map(EditAction::Move),
                1 => Just(EditAction::OpenLineBelow),
                1 => Just(EditAction::OpenLineAbove),
                1 => Just(EditAction::SetAnchor),
                1 => Just(EditAction::SetAnchorLine),
                1 => Just(EditAction::SetAnchorBlock),
                1 => Just(EditAction::ClearAnchor),
                1 => Just(EditAction::Paste),
                1 => Just(EditAction::PasteBefore),
                1 => (1usize..6).prop_map(|count| EditAction::ToggleCase { count }),
                1 => (prop_oneof![arb_char(), Just('\n')], 1usize..5)
                    .prop_map(|(with, count)| EditAction::ReplaceChars { with, count }),
                // The Replace session's pieces, in any order — including ones
                // the keymap never emits (a `<BS>` with no session, typing
                // after a line-open), which must still not panic.
                1 => (1usize..4).prop_map(|count| EditAction::EnterReplace { count }),
                2 => prop_oneof![arb_char(), Just('\n')].prop_map(EditAction::ReplaceType),
                1 => Just(EditAction::ReplaceBack),
                1 => Just(EditAction::LeaveReplace),
            ]
        }

        /// Any edit, including Undo/Redo — for the general no-panic fuzz.
        fn arb_action() -> impl Strategy<Value = EditAction> {
            prop_oneof![
                8 => arb_edit(),
                1 => Just(EditAction::Undo),
                1 => Just(EditAction::Redo),
            ]
        }

        /// The buffer invariants every edit must preserve, mode-agnostic: at
        /// least one line, the cursor row in range, the cursor column no past
        /// its line's end (the prompt allows the one-past-end Insert column;
        /// the app clamps to a char in Normal).
        fn invariants_hold(p: &Prompt) -> bool {
            let lines: Vec<String> = p.lines().collect();
            if lines.is_empty() {
                return false;
            }
            let (r, c) = p.cursor();
            // And a selection's anchor names a row the buffer has: a
            // selection left past a deleted row is the next panic.
            let anchored = p
                .buf
                .selection_anchor
                .is_none_or(|(ar, _)| ar < lines.len());
            r < lines.len() && c <= lines[r].chars().count() && anchored
        }

        proptest! {
            /// No edit sequence — however adversarial — panics or violates the
            /// buffer invariants. The Unicode chars stress the char-index math.
            /// Every edit the keymap can send, and every operator over every
            /// kind of target (review stop 12, T6 1: a block insert over
            /// joined rows panicked, and the fuzz generated neither).
            #[test]
            fn arbitrary_edits_preserve_invariants(
                steps in prop::collection::vec(arb_step(), 0..80),
            ) {
                let mut p = Prompt::new();
                for step in steps {
                    match step.clone() {
                        Step::Edit(a) => p.apply(a),
                        Step::Op(op, target, count) => {
                            p.apply_operator(op, target, count);
                        }
                    }
                    prop_assert!(invariants_hold(&p), "invariant broke after {step:?}");
                }
            }

            /// Undo/redo stack symmetry: after any edit run (no undo/redo in
            /// it, so the redo stack starts empty), `Undo`×k then `Redo`×k
            /// restores the buffer for *any* k — over-undoing past the history
            /// floor and re-redoing is a faithful round-trip.
            #[test]
            fn balanced_undo_redo_restores_buffer(
                actions in prop::collection::vec(arb_edit(), 0..60),
                k in 0usize..70,
            ) {
                let mut p = Prompt::new();
                for a in actions {
                    p.apply(a);
                }
                let before: Vec<String> = p.lines().collect();
                for _ in 0..k {
                    p.apply(EditAction::Undo);
                }
                for _ in 0..k {
                    p.apply(EditAction::Redo);
                }
                let after: Vec<String> = p.lines().collect();
                prop_assert_eq!(before, after);
            }

            /// **Folds of any shape never send a read-only buffer astray** —
            /// out of order, overlapping, empty, past the lines: every motion
            /// and every yank leaves the cursor on a line, within it (N5a).
            #[test]
            fn arbitrary_folds_keep_the_cursor_inside(
                lines in prop::collection::vec(
                    prop::collection::vec(arb_char(), 0..6)
                        .prop_map(|cs| cs.into_iter().collect::<String>()),
                    1..8,
                ),
                folds in prop::collection::vec((0usize..10, 0usize..10), 0..4),
                moves in prop::collection::vec((arb_motion(), 1usize..4), 0..20),
                targets in prop::collection::vec((arb_target(), 1usize..4), 0..6),
            ) {
                let mut b = Buffer::from_lines(lines.clone());
                b.set_folds(folds);
                let inside = |b: &Buffer| {
                    let (r, c) = b.cursor();
                    r < lines.len() && c <= lines[r].chars().count()
                };
                for (motion, count) in moves {
                    b.move_cursor(motion, count);
                    prop_assert!(inside(&b), "after {motion:?}");
                }
                for (target, count) in targets {
                    let _ = b.yank(target, count);
                    prop_assert!(inside(&b), "after yanking {target:?}");
                }
            }
        }
    }
}
