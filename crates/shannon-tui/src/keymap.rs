//! Keymaps: turn raw key events into modal outcomes.
//!
//! Separating "which key" from "what to do" gives two payoffs:
//! - The prompt exposes editing *primitives* (`EditAction`) and one
//!   `apply` entry point; it has no opinion about which key triggers what.
//! - A different style (emacs, helix, plain readline) is a new
//!   `impl Keymap` plus a config switch — no prompt change.
//!
//! A keymap is **stateful** — `translate(&mut self, …)` — because the
//! model is modal: the same key means different things in Normal vs
//! Insert. The same `&mut self` will carry chord-pending state when the
//! `$EDITOR` chord (`Ctrl-X Ctrl-E`) lands; until then there is no
//! `Pending` outcome to emit (earned-use: the variant arrives with its
//! first producer).
//!
//! The keymap's *output* is deliberately narrow (`KeyOutcome`): it
//! describes what the *focused pane* should do — edit, submit, scroll,
//! or a mode change. App-level concerns (switching focus, opening a
//! command line) are resolved one level up, in `shortcuts::classify`,
//! so the keymap never has to know panes exist.

use crossterm::event::{KeyEvent, KeyModifiers};

/// A **plain** (unmodified or merely shifted) key.
///
/// The seam between "this pane's own keys" and "the window manager's". Every
/// pane-owning handler must apply it: a handler that matches on `key.code`
/// alone makes `<C-x>` an alias for `x`, which in the graph pane turned `<C-d>`
/// — vim's half-page scroll — into a session delete, and swallowed `<C-w>` so
/// the pane could not be left from the keyboard at all. A decision's answers
/// are asked it by the decision itself (`ToolPrompt::resolve`), so the pane's
/// no-room guard and its answer path cannot disagree on which keys answer.
pub(crate) fn is_plain(key: &KeyEvent) -> bool {
    // **`SUPER` too.** It was screened in the viewer-close predicate (since
    // deleted, when closing became `:q`) — the one handler where the
    // consequence was benign — and not here, where a `SUPER`-modified
    // key counted as plain and reached `handle_graph_nav_key`, which matches on
    // bare `key.code`: `<Super-d>` would raise a delete-session confirmation
    // and `<Super-c>` would clone. Unreachable today (crossterm only reports
    // `SUPER` from kitty CSI-u, and nothing here pushes the enhancement flags),
    // which is exactly how a screen ends up in the wrong place (Reviewer A).
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

/// One primitive the prompt applies — a buffer edit, a cursor motion
/// (`Move`), or a selection-state change (`SetAnchor`/`ClearAnchor`).
/// These are the *interface* the prompt exposes; `prompt::Prompt::apply`
/// interprets them. (Submit is not here — it's a `KeyOutcome`, not an edit:
/// it consumes the whole buffer rather than mutating it.)
///
/// `Copy` because every variant is a unit, `Insert(char)`, or
/// `Move(CursorMotion)` — all `Copy`: a counted command applies the same
/// action `count` times, and passing it by value through that loop should
/// not require a clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    /// Insert a printable character at the cursor.
    Insert(char),
    /// Insert a newline at the cursor; cursor moves to start of new line.
    Newline,
    /// Delete one character to the left of the cursor (Backspace).
    DeleteBack,
    /// Delete one character to the right of the cursor (Delete).
    DeleteForward,
    /// Delete the word to the left of the cursor (insert-mode Ctrl-W).
    DeleteWordBack,
    /// Delete from the cursor to the start of the line (insert-mode
    /// Ctrl-U). Reclaimed now that Normal mode owns half-page scrolling,
    /// so Ctrl-U no longer has to be the global scroll key.
    DeleteToLineStart,
    /// Move the cursor. One variant for the whole motion vocabulary
    /// ([`CursorMotion`]); the prompt's `apply_motion` is the single place
    /// a motion maps to cursor arithmetic, shared with operator-range
    /// resolution so the two can never diverge.
    Move(CursorMotion),
    /// Leaving Insert nudges the caret one column left — onto the last
    /// typed char — without crossing to the previous line (vim's `<Esc>`
    /// rule). It bridges Insert's "cursor between chars, may sit at `len`"
    /// with Normal's "cursor on a char". At column 0 it stays put. A pure
    /// cursor move (no undo checkpoint) — *except* it also finalizes a
    /// pending block-insert (see [`BeginBlockInsert`](Self::BeginBlockInsert)).
    ClampCursorAfterInsert,
    /// Enter Insert at the left (`I`) or right (`A`) edge of the active
    /// block selection (vim `Ctrl-V`…`I`/`A`). Text typed on the first row
    /// is replicated to that column on every row of the block when Insert is
    /// left (`ClampCursorAfterInsert`). The prompt reads the rectangle from
    /// its own selection state, so there's no geometry payload.
    BeginBlockInsert(BlockEdge),
    /// Enter Insert from Normal (`i`/`a`/`A`/`I`/`o`/`O`): the prompt
    /// repositions (or opens a line) per [`InsertEntry`] and, when `count >
    /// 1`, arms a repeat — on `<Esc>` the typed text is replayed `count`
    /// times (`3ix` → "xxx"; `3o` opens three lines). The keymap switches to
    /// Insert alongside emitting this.
    EnterInsert { entry: InsertEntry, count: usize },
    /// Open a fresh empty line below the cursor and move onto it (vim
    /// `o`). A standalone editing primitive (used in tests); the keymap's
    /// `o` goes through [`EnterInsert`](Self::EnterInsert).
    OpenLineBelow,
    /// Open a fresh empty line above the cursor and move onto it (vim `O`).
    OpenLineAbove,
    /// Undo the last edit group (vim `u`).
    Undo,
    /// Redo the last undone group (vim `Ctrl-R`).
    Redo,
    /// Start a charwise Visual selection anchored at the cursor (vim `v`).
    /// Not a buffer mutation — it sets selection state the renderer
    /// highlights.
    SetAnchor,
    /// Start a linewise Visual selection anchored at the cursor (vim `V`):
    /// same anchor, but the selection covers whole rows.
    SetAnchorLine,
    /// Start a block (rectangular) Visual selection anchored at the cursor
    /// (vim `Ctrl-V`): the selection is the rectangle anchor↔cursor.
    SetAnchorBlock,
    /// Switch the active selection's geometry *without* re-anchoring —
    /// `v`↔`V`↔`Ctrl-V` mid-selection keeps both endpoints.
    SetSelectionKind(SelectionKind),
    /// Select a text object as the Visual selection (`viw`/`vi"`/`va(`/`vap`):
    /// set the anchor at the object's start and the cursor on its last char
    /// (charwise; a paragraph, linewise). A no-op if the object resolves to
    /// nothing (e.g. `vi(` with no pair). `count` objects (`v2ap`) — carried
    /// here, not as the outcome's count, which would select one object again
    /// from the end of the last; only a paragraph counts so far.
    SelectTextObject {
        scope: TextObjectScope,
        object: TextObject,
        count: usize,
    },
    /// Drop the Visual selection (leaving Visual without operating — `Esc`).
    ClearAnchor,
    /// Swap the cursor and the selection anchor (vim Visual `o`): extend the
    /// selection from the other end. A no-op without an active selection.
    SwapSelectionEnds,
    /// Paste the register *after* the cursor (vim `p`): after the cursor
    /// char for a charwise register, on a new line below for a linewise one.
    /// The text is the prompt's own register (filled by `y`), so there's no
    /// payload — the prompt reads its register when it applies this.
    Paste,
    /// Paste the register *before* the cursor (vim `P`): at the cursor for a
    /// charwise register, on a new line above for a linewise one.
    PasteBefore,
    /// `{count}~` — toggle the case of `count` chars from the cursor (stopping
    /// at the line's end) and land after them, on the line's last char at
    /// most. Carries its own count: applied `count` times, a toggle-and-step
    /// would toggle the last char again once it could step no further.
    ToggleCase { count: usize },
    /// `{count}r{char}` — replace `count` chars from the cursor with `with`,
    /// all or nothing: fewer than `count` left on the line changes nothing
    /// (vim). `with == '\n'` replaces them with ONE line break. The cursor
    /// lands on the last replaced char (the new line's start for a break).
    ReplaceChars { with: char, count: usize },
    /// `{count}R` — start Replace mode at the cursor. The keymap switches mode;
    /// on leaving, the text typed is replayed `count - 1` more times, each
    /// overwriting (vim `2Rab` over `abcdef` → `ababef`).
    EnterReplace { count: usize },
    /// A key typed in Replace mode: overwrite the char under the cursor, or
    /// append at the line's end; `'\n'` inserts a line break and overwrites
    /// nothing. What it covered is remembered so `<BS>` can put it back.
    ReplaceType(char),
    /// `<BS>` in Replace mode: undo the last char typed this Replace session —
    /// restoring what it overwrote, removing what it appended or the break it
    /// inserted. Before the session's start it only moves left.
    ReplaceBack,
    /// `<Esc>` from Replace: replay a counted session, then step onto the
    /// last typed char, as leaving Insert does.
    LeaveReplace,
}

/// What a case operator does to each char — Visual `~`, `u`, `U`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseOp {
    /// `~` — swap case.
    Toggle,
    /// `u` — to lower case.
    Lower,
    /// `U` — to upper case.
    Upper,
}

impl CaseOp {
    /// `c` in this case — **one char for one char**, so a column stays a
    /// column. Where Unicode's mapping is longer (`ß` upper-cases to `SS`) it
    /// takes the single-char form nvim does (`ẞ`), or leaves `c` as it is.
    pub fn apply(self, c: char) -> char {
        fn one(mut it: impl Iterator<Item = char>) -> Option<char> {
            match (it.next(), it.next()) {
                (Some(x), None) => Some(x),
                _ => None,
            }
        }
        let upper = || {
            one(c.to_uppercase())
                .or((c == 'ß').then_some('ẞ'))
                .unwrap_or(c)
        };
        let lower = || one(c.to_lowercase()).unwrap_or(c);
        match self {
            CaseOp::Upper => upper(),
            CaseOp::Lower => lower(),
            CaseOp::Toggle if c.is_lowercase() => upper(),
            CaseOp::Toggle if c.is_uppercase() => lower(),
            CaseOp::Toggle => c,
        }
    }
}

/// A vim operator — it acts on the text range a following motion (or a
/// doubled operator, or `$` via `D`/`C`) describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `d` — delete the range.
    Delete,
    /// `c` — delete the range and enter Insert (the keymap switches mode).
    Change,
    /// `y` — copy the range into the register, leaving the buffer untouched
    /// (so it never checkpoints undo). `p`/`P` paste it back.
    Yank,
    /// Visual `~`/`u`/`U` — change the case of every char in the range, in
    /// place. Never touches the register.
    Case(CaseOp),
    /// Visual `r{char}` — replace every char in the range with `char`, in
    /// place; line breaks stay. Never touches the register.
    Replace(char),
}

/// The single cursor-motion vocabulary — the noun every motion key
/// resolves to, whether it drives the cursor directly (`3j`) or bounds an
/// operator (`d3w`). Both paths run through `prompt::apply_motion`, so
/// there is exactly one definition of where each motion lands.
///
/// Under an operator each is charwise or linewise, as vim's
/// (`vim_keymap::is_charwise`, an exhaustive `match`): the cross-row and
/// screen members (`j` `k` `gg` `G` `{n}G` `H` `M` `L`) are linewise
/// ([`OpTarget::LineMotion`]), the rest charwise ([`OpTarget::Motion`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMotion {
    Left,
    Right,
    /// `k` — up one line (linewise as an operator target: `dk`).
    Up,
    /// `j` — down one line (linewise as an operator target: `dj`).
    Down,
    /// `0` / Home — to the start of the line (exclusive as an op target).
    LineStart,
    /// `$` / End — to the end of the line (inclusive as an op target).
    LineEnd,
    /// `w` — to the start of the next word (exclusive).
    WordForward,
    /// `b` — to the start of the previous word (exclusive).
    WordBackward,
    /// `e` — to the end of the current/next word (inclusive).
    WordEnd,
    /// `W` `B` `E` — `w` `b` `e` over WORDs, any run of non-blanks
    /// (`crate::word`): a path is one WORD and many words.
    BigWordForward,
    BigWordBackward,
    BigWordEnd,
    /// `gg` — to the first line of the buffer (column clamped).
    BufferStart,
    /// `G` — to the last line of the buffer (column clamped).
    BufferEnd,
    /// `{count}G` / `{count}gg` — to an absolute 1-indexed line (clamped to
    /// the buffer). The count is the *target line*, not a repeat — this is
    /// what `gg`/`G` resolve to when a count precedes them (`3G`, `10gg`).
    /// Linewise as an operator target (`d3G`).
    GotoLine(usize),
    /// `H` / `M` / `L` — a line picked by its place on the SCREEN. The count
    /// is a distance from the edge (`3H`), carried in the variant like
    /// `GotoLine`'s, not a repeat. Linewise as an operator target (`dH`), and
    /// then not pulled in by `scrolloff` (`:h H`). Where it lands is
    /// [`viewport::screen_line`](crate::viewport::screen_line), the one rule
    /// every pane asks.
    Screen(crate::viewport::ScreenLine),
    /// `]]` / `[[` — `count` SECTIONS forward / back, onto the line's first
    /// non-blank (`:h ]]`). What a section is belongs to the buffer
    /// ([`Sections`](crate::prompt::Sections)): a turn in a transcript, as
    /// neovim makes each shell prompt one in a terminal; vim's `{` in the
    /// first column elsewhere. The count rides in the variant, as `3H`'s
    /// does, because it is not a repeat: a count the sections run out
    /// before fails the whole motion (`2]]` in the last section). An
    /// operator over it is exclusive, with vim's linewise rule (`y]]` from a
    /// turn's first line takes that turn).
    ///
    /// `end`: `][` / `[]` — to a section's END instead (`:h ][`): vim's `}`
    /// in the first column; a turn's last line of text in a transcript.
    Section {
        forward: bool,
        end: bool,
        count: usize,
    },
    /// `'{a-z}` — to the place your mark `a` names, its column too (Steven,
    /// 10-26: with `` ` `` given to the agent's marks, `'` keeps vim's
    /// exact place), linewise as an operator target as vim's `'` is (`y'a`);
    /// `` `{a-z} `` — to the agent's, charwise as vim's `` ` `` is. Where a
    /// mark is belongs to the buffer — a transcript's are the session's,
    /// resolved through its blocks (`output::Anchor`); a buffer with none
    /// has no mark to go to, and the app says so before the motion is
    /// applied.
    Mark(Whose, char),
    /// `f{char}` `F{char}` `t{char}` `T{char}` — to the `count`-th `{char}`
    /// along this line (`:h f`); and what `;` `,` become ([`Find`]).
    FindChar(Find),
    /// `;` / `,` — the last `f` `F` `t` `T` again, the same way or the
    /// other (`reverse`), `count` times. Never taken by a buffer: the app
    /// makes it the find it repeats first (`App::concrete`), the last find
    /// being the editor's, not a pane's, as vim's is.
    RepeatFind {
        reverse: bool,
        count: usize,
    },
    /// `^` — the line's first non-blank (exclusive).
    FirstNonBlank,
    /// `}` / `{` — `count` paragraphs forward / back (`:h }`): to the next
    /// empty line past a paragraph, or the buffer's edge. Exclusive; at the
    /// last line's end, inclusive, as vim's (`findpar`).
    Paragraph {
        forward: bool,
        count: usize,
    },
    /// `ge` / `gE` — back to the end of the previous word / WORD (`:h ge`),
    /// inclusive.
    WordEndBackward {
        big: bool,
    },
    /// `%` — to the bracket matching the first `(` `)` `[` `]` `{` `}` at or
    /// after the cursor on its line (`:h %`), inclusive.
    MatchPair,
    /// `{count}%` — to the line `count` percent down the buffer (`:h N%`),
    /// linewise, as `{count}G` is.
    Percent(usize),
}

/// **A find along the line** — `f` `t` forward, `F` `T` back (`till`: one
/// short of the char), to the `count`-th `ch`. `again`: made by `;` or `,`,
/// where a `t` standing right before its char looks past it (vim's default,
/// `'cpoptions'` without `;`) — or `;` after `t` would never move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Find {
    pub till: bool,
    pub forward: bool,
    pub ch: char,
    pub count: usize,
    pub again: bool,
}

impl CursorMotion {
    /// **Does landing in a shut fold by this motion open it** — vim's
    /// `'foldopen'` default (`block,hor,mark,percent,quickfix,search,tag,
    /// undo`), measured in Neovim 0.12: a motion along the line (`hor`: `l`
    /// `$` `w`…), a mark, `]]` (`block`) open it; `j` `k` `G` `gg` `{n}G`
    /// and `H` `M` `L` do not — they step onto a fold as one line. A search
    /// opens one too, by its own door (`App::apply_search`).
    pub fn opens_folds(self) -> bool {
        match self {
            CursorMotion::Left
            | CursorMotion::Right
            | CursorMotion::LineStart
            | CursorMotion::LineEnd
            | CursorMotion::WordForward
            | CursorMotion::WordBackward
            | CursorMotion::WordEnd
            | CursorMotion::BigWordForward
            | CursorMotion::BigWordBackward
            | CursorMotion::BigWordEnd
            | CursorMotion::Section { .. }
            | CursorMotion::Mark(..)
            // `hor` (`f` `t` `;` `^`), `block` (`{` `}`), `percent` (`%`
            // and `N%` alike).
            | CursorMotion::FindChar(_)
            | CursorMotion::RepeatFind { .. }
            | CursorMotion::FirstNonBlank
            | CursorMotion::Paragraph { .. }
            | CursorMotion::MatchPair
            | CursorMotion::Percent(_) => true,
            // `ge` steps onto a fold as one line (measured, stop 16 A H2).
            CursorMotion::Up
            | CursorMotion::Down
            | CursorMotion::BufferStart
            | CursorMotion::BufferEnd
            | CursorMotion::GotoLine(_)
            | CursorMotion::WordEndBackward { .. }
            | CursorMotion::Screen(_) => false,
        }
    }
}

/// **Whose a mark is** — yours or the agent's: the session's own vocabulary
/// (`shannon_core::Whose`), so a motion, a session's marks and what its
/// header keeps name the owner alike.
pub use shannon_core::Whose;

/// Whether a text object covers just its content (`i`) or the
/// surrounding delimiters/whitespace too (`a`). `ciw` vs `caw`,
/// `ci"` vs `ca"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObjectScope {
    /// `i` — inner: the content only.
    Inner,
    /// `a` — around: content plus its delimiters (a quote/bracket pair) or
    /// trailing whitespace (a word).
    Around,
}

/// A text object — the noun in `c{i,a}{object}`. Resolved in `prompt.rs`
/// (`object_span`): a paragraph is whole lines; every other object is
/// chars of the current line. Bracket/quote matching is single-line for
/// now (a prompt is rarely a deep multi-line literal); multi-line is a
/// later refinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObject {
    /// `w` — a word (a run of one class, `crate::word`), or a run of blanks.
    Word,
    /// `W` — a WORD (a run of non-blanks), or a run of blanks.
    BigWord,
    /// `"` / `'` / `` ` `` — the text between a matching pair of this quote.
    Quote(char),
    /// `(` `{` `[` `<` (and aliases `b`/`B`) — the text between a matching
    /// bracket pair, stored as `(open, close)`.
    Pair(char, char),
    /// `p` — a paragraph: a run of lines that are not blank (or of blank
    /// ones), linewise, as vim's `ip`/`ap`. It did not exist, so `vap` —
    /// which the manual and a doc comment promised — selected nothing in any
    /// pane (Steven, 10-19).
    Paragraph,
}

/// What an operator applies over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpTarget {
    /// A charwise motion range measured from the cursor.
    Motion(CursorMotion),
    /// Linewise: the current line plus `count - 1` below it — the doubled
    /// operator forms `dd` / `cc`.
    Line,
    /// Linewise via a vertical, buffer or screen motion — `dj` / `dk` / `dG`
    /// / `dgg` / `d3G` / `dH`. The prompt resolves the whole-row span from
    /// the cursor to where the motion lands (`count` rows for `j`/`k`; to
    /// the buffer edge for `gg`/`G`; to the line for `{n}G`; to the screen
    /// line for `H`/`M`/`L`). Which motions reach here is
    /// `vim_keymap::is_charwise`'s `false` arm.
    LineMotion(CursorMotion),
    /// The `D`/`C` shape: from the cursor to the end of the line `n - 1` rows
    /// below (the payload is the line count `n`, ≥ 1). Charwise — `1` is the
    /// current line's tail (`D` = `d$`); `n > 1` spans rows (vim `3D` deletes
    /// to EOL plus the two lines below). The prompt resolves it to a `Chars`
    /// span, so it shares the multi-row drain.
    LineTail(usize),
    /// A text object around the cursor — `ciw`, `ci"`, `ca(`, …
    TextObject {
        scope: TextObjectScope,
        object: TextObject,
    },
    /// The active Visual selection (anchor → cursor). The range is the
    /// prompt's own selection state, so there's no payload here; `d`/`c`/`y`
    /// in Visual mode operate over it. Charwise or linewise is the prompt's
    /// `selection_linewise` flag (set by the mode), so one target serves
    /// both `v` and `V` — the prompt resolves the geometry.
    Selection,
}

/// Which edge of a block selection a block-insert acts on — `I` (left) or
/// `A` (right).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockEdge {
    /// `I` — the left edge (the block's lower column).
    Left,
    /// `A` — the right edge (just past the block's higher column).
    Right,
}

/// How a Normal-mode insert-entry repositions before Insert starts — the
/// six vim entries. The prompt owns the geometry (a reposition or a
/// line-open); the keymap just names the kind plus the count, so a counted
/// entry (`3i`, `3o`) can replay the typed text on `<Esc>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertEntry {
    /// `i` — before the cursor (no reposition).
    Before,
    /// `a` — after the cursor.
    After,
    /// `I` — at the start of the line.
    LineStart,
    /// `A` — at the end of the line.
    LineEnd,
    /// `o` — on a fresh line below.
    OpenBelow,
    /// `O` — on a fresh line above.
    OpenAbove,
}

/// The geometry of a Visual selection — charwise (`v`), linewise (`V`), or
/// block/rectangular (`Ctrl-V`). The prompt resolves a `Selection` operator
/// and draws the highlight against this, so one `OpTarget::Selection` and
/// one keymap `visual` handler serve all three modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    Char,
    Line,
    Block,
}

/// Editor modes. The three Visual modes (`v`/`V`/`Ctrl-V`) share the
/// keymap's `visual` handler and differ only in the prompt's
/// `selection_kind` ([`SelectionKind`]) — selection geometry + highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    /// `R` — typed chars overwrite; `<BS>` restores what they covered.
    Replace,
    /// Charwise Visual (`v`) — the selection is a char range.
    Visual,
    /// Linewise Visual (`V`) — the selection is whole rows.
    VisualLine,
    /// Block Visual (`Ctrl-V`) — the selection is a rectangle.
    VisualBlock,
}

impl Mode {
    /// Status-line label, always shown so the modal state is never a
    /// mystery (the Kakoune lesson — a blank screen that silently eats
    /// keys is the papercut we're avoiding).
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Replace => "REPLACE",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
            Mode::VisualBlock => "V-BLOCK",
        }
    }

    /// Whether this is one of the Visual family (a selection is active).
    pub fn is_visual(self) -> bool {
        matches!(self, Mode::Visual | Mode::VisualLine | Mode::VisualBlock)
    }

    /// Whether keys type text here — Insert or Replace — so the caret may
    /// rest past the line's last char, where the next char goes.
    pub fn is_typing(self) -> bool {
        matches!(self, Mode::Insert | Mode::Replace)
    }
}

/// How far one scroll command moves the viewport. The concrete row count
/// is resolved against the focused pane's height in `app.rs`, keeping the
/// key mapping geometry-independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollMagnitude {
    /// One visual row.
    Line,
    /// Half the visible pane (Ctrl-U / Ctrl-D).
    HalfPage,
    /// One visible pane minus a row of orientation (PgUp / PgDn, Ctrl-F/B).
    Page,
}

/// **A search key** — vim's `/` `?` `n` `N` `*` `#` (Steven, 10-07: "We
/// should prioritize searching").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchCmd {
    /// `/` (`forward`) or `?`: open the search line.
    Open { forward: bool },
    /// `n` (`reverse: false`) or `N`: the last search again, its way or the
    /// other.
    Next { reverse: bool },
    /// `*` (`forward`) or `#`: the word under the cursor, whole.
    Word { forward: bool },
}

/// A scroll request applied to whichever pane has focus — a viewport move by a
/// magnitude (the chat's `gg`/`G`/`j`/`k` are *cursor* motions now, not scrolls,
/// so the absolute jumps are gone; `Ctrl-U`/`D`/`F`/`B` and PgUp/Dn still scroll
/// directly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollCmd {
    Up(ScrollMagnitude),
    Down(ScrollMagnitude),
    /// `zt` `zz` `zb` (and `z<CR>` `z.` `z-`, which also move to the first
    /// non-blank) — put the cursor's line at the top, middle or bottom of the
    /// window. `line` is a typed count: vim's `[count]zt` goes to that line
    /// first. A viewport move like its siblings, relative to the cursor rather
    /// than by a distance; where the window lands is
    /// [`viewport::place_top`](crate::viewport::place_top).
    Place {
        at: crate::viewport::Place,
        line: Option<usize>,
        first_non_blank: bool,
    },
}

impl ScrollCmd {
    /// How far, ignoring which way — so a pane that scrolls by repeated
    /// single-row steps can still honour a page/half-page request. `None` for
    /// [`Place`](ScrollCmd::Place), which is not a distance.
    pub fn magnitude(self) -> Option<ScrollMagnitude> {
        match self {
            ScrollCmd::Up(m) | ScrollCmd::Down(m) => Some(m),
            ScrollCmd::Place { .. } => None,
        }
    }
}

/// A direction, for the directional window commands (`<C-w>h/j/k/l` focus,
/// `<C-w>H/J/K/L` relocate). Names mirror vim's `hjkl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Down,
    Up,
    Right,
}

/// A window-management command — **app topology**, not buffer editing. The
/// `<C-w>` prefix resolves to one of these; the keymap emits it as
/// [`KeyOutcome::Window`] and the *app* applies it to the pane layout (the
/// keymap never touches focus/size itself — same narrow-output rule as
/// [`Scroll`](KeyOutcome::Scroll)). Resize/relocate variants land with their
/// phases (B2/B3); focus is first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCmd {
    /// `<C-w>h/j/k/l` — move focus to the window in that direction (a no-op
    /// when there is no window that way, as in vim).
    Focus(Direction),
    /// `<C-w>w` / `<C-w><C-w>` — cycle focus to the next window (wraps).
    FocusCycle,
    /// `<C-w>W` — cycle focus to the previous window.
    FocusCycleBack,
    /// `<C-w>p` — focus the previously-focused window.
    FocusPrev,
    /// `<C-w><Space>` — mark or unmark this transcript pane, so `:Send` and a
    /// submitted prompt reach it. The `:graph` mark idiom, reused: the same
    /// gesture selects a set to act on, one level up.
    ToggleMark,
    /// `<C-w>u` — unmark every pane.
    ClearMarks,
    /// `{count}<C-w>+` — grow the focused pane by `count` rows (height; only
    /// in a stacked split, a no-op side-by-side — vim).
    ResizeGrow,
    /// `{count}<C-w>-` — shrink the focused pane by `count` rows.
    ResizeShrink,
    /// `{count}<C-w>>` — widen the focused pane by `count` cols (width; only
    /// in a side-by-side split, a no-op stacked — vim).
    ResizeWider,
    /// `{count}<C-w><` — narrow the focused pane by `count` cols.
    ResizeNarrower,
    /// `<C-w>=` — reset the split to the configured default size. On the
    /// two-pane, output-dominant layout this is a reset to the sane default,
    /// not a literal 50/50 equalize (which would waste the prompt); a true
    /// equal-split waits for real splits.
    Equalize,
    /// `<C-w>H/J/K/L` — relocate the focused pane to that edge (vim's window
    /// *move*). `K`/`J` (up/down) reorder the vertical stack; `H`/`L`
    /// (left/right) reorient to a side-by-side split.
    Relocate(Direction),
    /// `<C-w>s` / `:split` — split the focused pane, stacked.
    Split,
    /// `<C-w>v` / `:vsplit` — split the focused pane, side by side.
    VSplit,
    /// `<C-w>c` / `:close` — close the focused pane (never the last one).
    Close,
    /// `<C-w>o` / `:only` — close every pane but the focused one.
    Only,
    /// `<C-^>` / `:b#` — **the window's alternate**: what it showed last
    /// before what it shows, back as it was (vim's CTRL-^). Not a `<C-w>`
    /// chord, as in vim, but a window's: it acts on the window's history.
    Alternate,
    /// `<C-]>` (and `<CR>` on a link) — **follow the `|link|`** under the
    /// cursor or next on its line (vim's CTRL-]), in the manual or the tutor.
    FollowLink,
    /// `<C-t>` — **back from a link**, or `{count}` of them (vim's CTRL-T):
    /// the window's tag stack. A window's, as `<C-^>` is.
    LinkBack,
    /// `gt` / `:tabnext` — the next tab, or with a count tab N (vim's).
    TabNext,
    /// `gT` / `:tabprevious` — the previous tab, or with a count N back.
    TabPrevious,
    /// `<C-w>T` — move this window to a new tab after this one (vim's).
    MoveToTab,
    /// `<C-w>q` / `:quit` — close the focused pane, and **quit** when it is the
    /// last one. The one window command that can end the program, which is why
    /// it is distinct from [`Close`](Self::Close) rather than an alias for it.
    Quit,
}

/// What feeding one key to a keymap produced — scoped to the focused pane.
/// App-topology outcomes (`Window`) are *intents* the app executes, not the
/// keymap reaching into pane state; focus-switch as a single global key is
/// gone (it's `<C-w>w` now).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    /// Edit the prompt buffer, applying `action` `count` times as a single
    /// undo group (only meaningful when the prompt is focused and in a mode
    /// that edits). `count >= 1`. Insert-mode edits and uncounted Normal
    /// commands carry `count == 1`; a Normal-mode count prefix (`3j`, `5x`)
    /// puts the digits here. Construct the common count-1 case with
    /// [`KeyOutcome::edit`].
    Edit { action: EditAction, count: usize },
    /// Apply an operator (`d`/`c`) over the range its target describes,
    /// `count` times, as a single undo group. The keymap has already
    /// switched to Insert for a `Change`; the app just applies the buffer
    /// edit. (`count` is the resolved repeat — e.g. `3dd` arrives as
    /// `{Delete, Line, 3}`, `d2w` as `{Delete, Motion(WordForward), 2}`.)
    Operator {
        op: Operator,
        target: OpTarget,
        count: usize,
    },
    /// Submit the prompt buffer as a turn.
    Submit,
    /// Scroll the focused pane.
    Scroll(ScrollCmd),
    /// `zo` `zc` `za` `zR` `zM` — the fold at the cursor, or every fold, in
    /// the focused pane (N5a). The app applies it: folds are its window's.
    Fold(crate::fold::FoldVerb),
    /// A window-management command (focus, resize, relocate) over the pane
    /// layout. The app applies it — the keymap doesn't own topology.
    ///
    /// `None` means **no count was typed**, which is not the same as `Some(1)`:
    /// vim's bare `<C-w>w` cycles to the next window while `1<C-w>w` jumps to
    /// window 1. This used to be `usize` collapsed through `.max(1)`, and the
    /// doc comment admitted the consequence — "focus ignores it" — which is a
    /// dropped count wearing a disclaimer.
    Window(WindowCmd, Option<usize>),
    /// Open the prompt buffer in `$EDITOR` (`Ctrl-X Ctrl-E`). The app suspends
    /// the TUI, runs the editor, and replaces the prompt with the result —
    /// only when the prompt is focused (the chat has nothing to edit).
    OpenEditor,
    /// Open the `:` command line (vim's `:`). The app takes over key input for
    /// a one-line command (`:q`, `:Edit`, `:config dump`).
    EnterCommand,
    /// A search key (`/` `?` `n` `N` `*` `#`), `count` times for those that
    /// move. The app holds the last pattern (vim's `/` register) and searches
    /// the focused pane's text.
    Search { cmd: SearchCmd, count: usize },
    /// `m{a-z}` — set mark `a` where the cursor is. The app keeps it where
    /// the pane's text comes from (a transcript's session), not in the
    /// keymap or the window: every window on the session has it, as vim's
    /// lowercase marks are the buffer's.
    SetMark(char),
    /// The key resolved to a real action that is **not available in this
    /// pane**, carrying the sentence the user should read.
    ///
    /// Distinct from [`Ignored`](Self::Ignored), and the distinction is the
    /// whole point: `Ignored` means "no binding", which is a key the user never
    /// expected to work. This means "there IS a binding and you cannot have it
    /// here" — a dead key unless somebody says so
    /// (`[[feedback_visible_refusals]]`). The keymap is shared by every
    /// read-only pane and does not know which one it is driving, so it hands
    /// the app words rather than a pane-specific message.
    Refused(&'static str),
    /// Nothing matched / nothing observable happened.
    Ignored,
}

impl KeyOutcome {
    /// The common case: edit the buffer once. Keeps call sites that don't
    /// deal in counts (insert-mode edits, single Normal commands) terse.
    pub fn edit(action: EditAction) -> Self {
        KeyOutcome::Edit { action, count: 1 }
    }
}

/// A keymap translates raw key events into modal outcomes. Stateful
/// (modes, future chords) via `&mut self`.
pub trait Keymap {
    fn translate(&mut self, key: KeyEvent) -> KeyOutcome;

    /// The keymap's current mode, for the status indicator and for the
    /// renderer's "show the cursor?" decision.
    fn mode(&self) -> Mode;

    /// Is a multi-key chord in flight (a count, a prefix, an operator)?
    ///
    /// A handler that intercepts keys *ahead* of a keymap must ask this: eating
    /// the second key of a chord strands the prefix, and it then fires on
    /// whatever plain key reaches the keymap next — in a different pane,
    /// minutes later.
    fn pending_chord(&self) -> bool;

    /// Is a chord in flight that **owns the next key** — a prefix or an
    /// operator, not merely a count?
    ///
    /// The difference matters to a pane with verbs of its own (`:graph`, the
    /// decision pane). After `z`, `g` or `<C-w>` the next key completes that
    /// chord or kills it, whatever it means alone: `zd` must never reach a
    /// delete. A bare count owns nothing — `3d` in `:graph` is a delete with a
    /// count it cannot use, not a key for the keymap to refuse as a writer.
    /// Default: any chord, for a keymap that does not tell them apart.
    fn pending_prefix(&self) -> bool {
        self.pending_chord()
    }

    /// **The mode the cursor's shape is chosen by** (`tui.guicursor`): the
    /// keymap's own, or what waits in it — neovim's `o` while an operator
    /// waits for its motion, its `r` while `r` waits for a character.
    /// Default: the mode, for a keymap with no chords.
    fn cursor_mode(&self) -> crate::guicursor::CursorMode {
        self.mode().into()
    }

    /// **Abandon a half-typed chord.** The other half of
    /// [`pending_chord`](Self::pending_chord): a handler that eats a key ahead
    /// of the keymap must not leave the prefix armed, and asking was never
    /// enough on its own — there was nothing to *do* about a `true` answer.
    /// Default: no-op, for a keymap with no chords.
    fn cancel_chord(&mut self) {}

    /// **A `c` that found nothing to change goes back to Normal.** The keymap
    /// enters Insert as it emits a change, before the buffer resolves it; vim
    /// cancels a `c` whose motion or object fails (`ci(` with no pair, `cj` on
    /// the last line) and stays in Normal, so the next key is a command, not
    /// text (review stop 12, T6 5). Default: no-op, for a keymap with no
    /// Insert mode.
    fn abandon_change(&mut self) {}

    /// Begin a characterwise Visual selection programmatically — the mouse-drag
    /// entry point. Returns the anchor action to apply (the selection starts at
    /// the cursor). Default: no-op, for a keymap with no Visual mode.
    fn begin_visual(&mut self) -> KeyOutcome {
        KeyOutcome::Ignored
    }

    /// Leave Visual back to Normal programmatically — a plain mouse click that
    /// isn't a drag. Returns the action to clear the selection. Default: no-op.
    fn leave_visual(&mut self) -> KeyOutcome {
        KeyOutcome::Ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewport::ScreenLine;

    /// **Which motions open a fold they land in is vim's 'foldopen'**
    /// default (`block,hor,mark,percent,quickfix,search,tag,undo`; `:h
    /// 'foldopen'`), measured in Neovim 0.12 at review stop 15 (A): `l` `h`
    /// `0` `$` and the word motions (`hor`), a mark, `]]` (`block`) open;
    /// `j` `k` `G` `gg` `{n}G` `H` `M` `L` step onto a fold as one line.
    /// Stated here from vim's side, exhaustively, so a new motion is
    /// classified twice and an arm flipped in `opens_folds` fails: ten of
    /// its fourteen arms could flip with every test green (C M6).
    #[test]
    fn opens_folds_is_vims_foldopen() {
        fn vims(motion: CursorMotion) -> bool {
            match motion {
                // `hor`
                CursorMotion::Left
                | CursorMotion::Right
                | CursorMotion::LineStart
                | CursorMotion::LineEnd
                | CursorMotion::WordForward
                | CursorMotion::WordBackward
                | CursorMotion::WordEnd
                | CursorMotion::BigWordForward
                | CursorMotion::BigWordBackward
                | CursorMotion::BigWordEnd => true,
                // `hor` again: "fx", "^" and the rest along the line.
                CursorMotion::FindChar(_)
                | CursorMotion::RepeatFind { .. }
                | CursorMotion::FirstNonBlank => true,
                // `block` (`{` `}` too), `mark`, `percent` — `%` and `N%`
                // alike (`nv_percent` asks `FDO_PERCENT` of both; measured
                // at review stop 16, A H2).
                CursorMotion::Section { .. }
                | CursorMotion::Paragraph { .. }
                | CursorMotion::Mark(..)
                | CursorMotion::MatchPair
                | CursorMotion::Percent(_) => true,
                // Vertical, and the jumps 'foldopen' leaves out — `ge` too,
                // which steps onto a fold as one line (measured, A H2).
                CursorMotion::Up
                | CursorMotion::Down
                | CursorMotion::BufferStart
                | CursorMotion::BufferEnd
                | CursorMotion::GotoLine(_)
                | CursorMotion::WordEndBackward { .. }
                | CursorMotion::Screen(_) => false,
            }
        }
        // **Every variant has a slot**, by an exhaustive `match`: a motion
        // added to the type does not compile here until it has one, and a
        // slot `every` leaves unfilled fails below. `every` was a hand list,
        // and the seven motions of review stop 16 were in `vims` and in no
        // list — every one of their arms could flip, green (A H2).
        fn slot(motion: CursorMotion) -> usize {
            match motion {
                CursorMotion::Left => 0,
                CursorMotion::Right => 1,
                CursorMotion::Up => 2,
                CursorMotion::Down => 3,
                CursorMotion::LineStart => 4,
                CursorMotion::LineEnd => 5,
                CursorMotion::WordForward => 6,
                CursorMotion::WordBackward => 7,
                CursorMotion::WordEnd => 8,
                CursorMotion::BigWordForward => 9,
                CursorMotion::BigWordBackward => 10,
                CursorMotion::BigWordEnd => 11,
                CursorMotion::BufferStart => 12,
                CursorMotion::BufferEnd => 13,
                CursorMotion::GotoLine(_) => 14,
                CursorMotion::Screen(_) => 15,
                CursorMotion::Section { .. } => 16,
                CursorMotion::Mark(..) => 17,
                CursorMotion::FindChar(_) => 18,
                CursorMotion::RepeatFind { .. } => 19,
                CursorMotion::FirstNonBlank => 20,
                CursorMotion::Paragraph { .. } => 21,
                CursorMotion::WordEndBackward { .. } => 22,
                CursorMotion::MatchPair => 23,
                CursorMotion::Percent(_) => 24,
            }
        }
        const SLOTS: usize = 25;
        let section = |forward, end| CursorMotion::Section {
            forward,
            end,
            count: 1,
        };
        let every = [
            CursorMotion::Left,
            CursorMotion::Right,
            CursorMotion::Up,
            CursorMotion::Down,
            CursorMotion::LineStart,
            CursorMotion::LineEnd,
            CursorMotion::WordForward,
            CursorMotion::WordBackward,
            CursorMotion::WordEnd,
            CursorMotion::BigWordForward,
            CursorMotion::BigWordBackward,
            CursorMotion::BigWordEnd,
            CursorMotion::BufferStart,
            CursorMotion::BufferEnd,
            CursorMotion::GotoLine(3),
            CursorMotion::Screen(ScreenLine::Top(1)),
            CursorMotion::Screen(ScreenLine::Middle),
            CursorMotion::Screen(ScreenLine::Bottom(1)),
            section(true, false),
            section(false, true),
            CursorMotion::Mark(Whose::Yours, 'a'),
            CursorMotion::Mark(Whose::Agents, 'a'),
            CursorMotion::FindChar(Find {
                till: false,
                forward: true,
                ch: 'x',
                count: 1,
                again: false,
            }),
            CursorMotion::RepeatFind {
                reverse: false,
                count: 1,
            },
            CursorMotion::FirstNonBlank,
            CursorMotion::Paragraph {
                forward: true,
                count: 1,
            },
            CursorMotion::WordEndBackward { big: false },
            CursorMotion::MatchPair,
            CursorMotion::Percent(50),
        ];
        let filled: std::collections::HashSet<usize> = every.iter().map(|&m| slot(m)).collect();
        assert_eq!(filled.len(), SLOTS, "a motion `every` does not list");
        for motion in every {
            assert_eq!(motion.opens_folds(), vims(motion), "{motion:?}");
        }
    }
}
