//! Output pane buffer + viewport.
//!
//! Stores the chat transcript as a sequence of **blocks** — one per
//! user echo or assistant response — each tagged with a role. A block is
//! a *dumb render unit*: it holds raw logical lines and a cached wrap of
//! them. It deliberately does **not** re-enforce turn invariants
//! (role-alternation, commit-on-complete, drop-on-error); that lives in
//! `shannon-core::conversation::Conversation`, the single authority on the
//! transcript's shape. `Output` is the *view* of that transcript, fed in
//! tandem, and the role tag exists for rendering (and, later, the M3
//! graph view) — not as a second source of truth.
//!
//! ## The wrap cache (the perf contract)
//!
//! Wrapping logical lines to the terminal width is the render hot path.
//! M1 re-wrapped the *entire* buffer on every frame, so per-frame cost
//! grew O(history) (see `benches/output_wrap.rs`). Here each block caches
//! its wrapped rows at the two widths last asked for — two panes on one
//! session at widths one column apart both hit — and only recomputes when
//! **either** a third width is asked for (a resize) **or** the block's own
//! content is appended to (only the in-flight block, while streaming). A
//! reply's styling is cached apart from the rows, once per change to its
//! lines, whatever the width: redone inside every re-wrap, it made two
//! such panes cost 1.6 s a frame at 20.8k lines; 0.13 ms since (buffers
//! review B F1, measured 10-17). Sealed blocks are frozen: a steady-state
//! redraw re-wraps nothing and just slices the cache. The caches live
//! behind `RefCell` so `visible_window` stays `&self` and the renderer
//! (`view::draw`, which holds `&App`) does not have to become mutable —
//! interior mutability is the right tool for a memoization cache that is
//! logically pure.
//!
//! Scrolling state lives in a `Viewport` field (see `viewport.rs`),
//! shared with the (future) prompt pane so there is one scroll
//! implementation, not two.
//!
//! Wrapping goes through the shared [`crate::wrap`] primitive, which breaks by
//! terminal display cells (CJK/emoji wrap correctly) and is **word-aware**
//! ([`wrap_cells_word`](crate::wrap::wrap_cells_word)) for readability. The
//! cached rows carry each row's slice of its line's **styling** (markdown
//! structure, syntax highlighting once sealed), parallel to the rows, so the
//! renderer never re-styles per frame.

use std::cell::{Cell, Ref, RefCell};

use crate::style::CellStyle;
use crate::viewport::Viewport;

/// Who produced a block. Drives styling once chat bubbles land (M2
/// "pretty defaults"); today it only distinguishes the user echo from
/// the assistant stream, but it's intrinsic to what a block *is*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockRole {
    User,
    Assistant,
    /// A tool call and what came of it. Its own role because it is neither
    /// party talking: it is machinery acting on the conversation's behalf, and
    /// it must be **visible in the transcript** rather than announced in a
    /// status line that scrolls away — "what did it just run, and what happened"
    /// is exactly the question the transcript should answer later.
    Tool,
    /// **A rule above or below your message** (Steven, 10-15: the user's side
    /// "easier to notice when scrolling through"): one empty line, drawn
    /// across the pane in `ui.border.user`. A line rather than a decoration
    /// so the cursor, a click and the wrap count it as they count every row;
    /// empty, so a yank or a search finds nothing in it.
    Rule,
    /// **What the model thought** (N1b, `docs/DesignDocs/navigation.md` §4) —
    /// a reasoning summary or a note between calls, as the API sent its
    /// text: a block of its own, where it was written in the reply
    /// ([`Output::append_thought`]), drawn quieter than the reply
    /// (`ui.text.quiet.thinking`, italic and dim). Shipped shut, its first
    /// line showing (N5a, NQ10: `FoldRule::SHIPPED`).
    Thinking,
}

impl BlockRole {
    /// Every role — the list built by the compiler (`every_variant!`).
    pub const ALL: &'static [BlockRole] =
        every_variant!(BlockRole: User, Assistant, Tool, Rule, Thinking);

    /// **Its `[tui.fold]` rule** ([`KindRule`]); `None` for a rule line,
    /// one row, which never folds. One match, so how a block starts
    /// (`starts_shut`), whether it has a fold (`Folds::has_fold`), a changed
    /// rule ([`ruled_otherwise`]) and a `:set` of one ([`ruled_by_keys`])
    /// agree on which kinds a rule has.
    pub fn fold_rule(self) -> Option<KindRule> {
        let rule = |start: fn(FoldRule) -> FoldStart, key| Some(KindRule { start, key });
        match self {
            BlockRole::Thinking => rule(|rule| rule.thinking, "tui.fold.thinking"),
            BlockRole::Tool => rule(|rule| rule.tool, "tui.fold.tool"),
            BlockRole::Assistant => rule(|rule| rule.reply, "tui.fold.reply"),
            BlockRole::User => rule(|rule| rule.message, "tui.fold.message"),
            BlockRole::Rule => None,
        }
    }

    /// **Does `rule` give this kind a fold** before one is made by hand —
    /// what `zM` and `zR` shut and open of blocks still to come.
    fn ruled_a_fold(self, rule: FoldRule) -> bool {
        self.fold_rule()
            .is_some_and(|kind| (kind.start)(rule).has_a_fold())
    }

    /// **Its name in a saved mark** ([`Position`], `shannon_core::Mark`) —
    /// written to disk, so a renamed variant keeps its word.
    pub fn name(self) -> &'static str {
        match self {
            BlockRole::User => "you",
            BlockRole::Assistant => "agent",
            BlockRole::Tool => "tool",
            BlockRole::Rule => "rule",
            BlockRole::Thinking => "thinking",
        }
    }

    /// The role a saved mark's word names ([`name`](Self::name)), if any —
    /// over every role, the list built by the compiler (`every_variant!`).
    pub fn from_name(name: &str) -> Option<BlockRole> {
        (BlockRole::ALL.iter().copied()).find(|role| role.name() == name)
    }

    /// **The model wrote it** — its reply and its thinking: styled from its
    /// markdown, and a held value in it is one the model put there unless
    /// it was put back from a handle ([`Output::shown_at`]).
    pub fn the_models(self) -> bool {
        matches!(self, BlockRole::Assistant | BlockRole::Thinking)
    }
}

/// One transcript block: a role, its raw logical lines (no embedded
/// `'\n'`, sanitized at ingest), a `sealed` flag (an open block is the
/// in-flight assistant response still being streamed into), and a
/// width-keyed wrap cache.
#[derive(Debug, Clone)]
struct Block {
    /// Who produced this block — surfaced per row by `visible_window` and read
    /// by the renderer for the per-role "bubble" treatment (and, later, the M3
    /// graph view). No longer the single source of turn truth (that's
    /// `Conversation`); a rendering tag.
    role: BlockRole,
    /// **Which turn this block belongs to** — its index in the session's
    /// rendered history. What makes "reply to the turn under the cursor"
    /// answerable from the transcript instead of only from `:graph`: a cursor
    /// knows its line, a line knows its block, and now a block knows its turn.
    ///
    /// `None` for anything appended outside a turn (a replayed header, a
    /// notice, a turn the conversation dropped: [`Output::abandon_turn`]),
    /// so a caller must handle "this line is not part of a turn".
    turn: Option<usize>,
    /// **Drawn only while live** — a call's `⚙ running:` line
    /// (`tool_record::Record::live_only`), or a thought the turn ended
    /// before its block closed, which the turn does not keep
    /// (`Output::close_thought`): a reopened turn draws neither, so neither
    /// is counted in a kept place ([`Output::position_of`]).
    live_only: bool,
    /// **Of a turn the conversation dropped** and the transcript kept — it
    /// ran something, or says why it failed ([`Output::abandon_turn`]):
    /// painted apart, so nobody reads it as said to the model.
    dropped: bool,
    /// **A record of the agent's `mark`** (`tool_record::Record::of_a_mark`):
    /// never where a quote lands ([`Output::find_quote`]).
    of_a_mark: bool,
    /// **What it says shut, when not its first line** — a command record's
    /// `⚙ verb: the command` (`tool_record::Record::title`, vim's
    /// `'foldtext'`): its text and one style per character.
    title: Option<(String, Vec<CellStyle>)>,
    /// **The call it records** (`tool_record::Record::call_id`) — a running
    /// line's, which the record closing the call replaces
    /// ([`Output::push_tool_record`]).
    call: Option<String>,
    sealed: bool,
    lines: Vec<String>,
    /// **Styles given with the lines**, parallel to them: a tool record's,
    /// chosen where the call was read (`tool_record`), since what a line IS
    /// — a command, its output, a removed line — is known there and nowhere
    /// after. Empty for a block styled from its text (a reply's markdown)
    /// or not at all (the user's echo).
    given: Vec<Vec<CellStyle>>,
    /// **A reply's styling, per logical line — once per change to its
    /// lines, whatever the width**: markdown structure, and syntax
    /// highlighting once sealed. Computed inside the width-keyed wrap, it
    /// was redone at every re-wrap, so two panes one column apart
    /// re-highlighted every fenced block on every frame — 1.6 s a frame at
    /// 20.8k lines (buffers review, B F1). `None` until asked, and after
    /// the lines change.
    styled: RefCell<Option<Vec<Vec<CellStyle>>>>,
    /// **Where a held secret was put back from its handle**, as `(line,
    /// from, to)` chars — an assistant block's, given with its text
    /// ([`Output::append_shown`]). A held value anywhere else in what the
    /// model wrote is one it wrote itself: it had the value (review stop 6,
    /// F 5; [`Output::shown_at`]).
    shown: Vec<(usize, usize, usize)>,
    /// The wrapped rows at the two widths last asked for, the latest first:
    /// two panes at two widths both hit (B F1); a third width wraps over the
    /// older.
    cache: RefCell<[WrapCache; 2]>,
    /// **What was found under its fold, and of what** — `(version, key,
    /// found)` ([`Output::fold_holds`]): asked again only when its lines
    /// change or the asker's key does, so a frame asks each fold once.
    held: Cell<Option<(u64, u64, bool)>>,
    /// **Which content this is** — a number no other block's content, in any
    /// transcript, ever had: given at creation and at every change to
    /// `lines` ([`invalidate`](Self::invalidate)), so a copy of the lines can
    /// tell what it still has right ([`Output::lines_since`]). Process-wide,
    /// not per transcript: a transcript cloned and then changed apart from
    /// its clone would number two contents alike.
    version: u64,
    /// **Which block this is**, for its whole life ([`BlockId`]): unlike
    /// `version`, [`invalidate`](Self::invalidate) keeps it.
    id: BlockId,
}

/// The last block version given ([`Block::version`]).
static VERSIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A block version no content has had.
fn next_version() -> u64 {
    VERSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// **Which block this is, for its whole life** — given when it is made and
/// kept through every change to its lines: what a place in the conversation
/// names ([`Anchor`]). Not its index, which a record inserted before an
/// empty open block moves (`push_tool_record`); not a turn's index, which
/// the next turn reuses after `abandon_turn`; not its version, which every
/// streamed chunk moves. Process-wide, as versions are, so a block that is
/// gone — popped, or truncated with its turn — is never named again: an
/// anchor into it is not set, as vim's mark on a deleted line (E20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(u64);

/// The last block id given ([`BlockId`]).
static BLOCK_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl BlockId {
    /// An id no block has had.
    fn next() -> Self {
        Self(BLOCK_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1)
    }
}

/// **A copy of a transcript's lines, as of when it was taken** — each
/// block's version and line count, for [`Output::lines_since`] to say what
/// the copy still has right. Empty: a copy of nothing yet.
#[derive(Debug, Clone, Default)]
pub struct Synced {
    blocks: Vec<(u64, usize)>,
}

impl Synced {
    /// How many lines the copy holds.
    pub fn lines(&self) -> usize {
        self.blocks.iter().map(|&(_, n)| n).sum()
    }
}

/// **What the caches have had to recompute** — instrumentation, read by the
/// tests that prove the caches are reused: a steady redraw bumps neither.
#[derive(Debug, Clone, Default)]
struct Ops {
    /// Wraps of a block to a width it had no rows for.
    wraps: Cell<usize>,
    /// Stylings of a reply's lines.
    styles: Cell<usize>,
}

/// A block's wrapped rows at a particular width, in both its shapes. `width ==
/// 0` is the "invalid / not yet computed" sentinel (a real content width is
/// never 0), used to force a recompute after the block's lines change.
#[derive(Debug, Clone, Default)]
struct WrapCache {
    width: u16,
    /// The block open: every row of every line.
    open: Rows,
    /// **The block shut to one row** (N5a) — `None` where it cannot shut:
    /// its content takes one row or none at this width (vim's
    /// `foldminlines`). Wrapped with the open rows, so asking costs nothing
    /// more per frame; which shape a window draws is its own
    /// ([`Shut`]).
    shut: Option<Rows>,
}

/// **A shut block's row** (N5a) — what it stands for, which the view draws
/// around its text: `+ `, the text, `…` where it was cut, and how many lines
/// ([`suffix`](Self::suffix)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldRow {
    /// The lines it stands for: its block's content, to the last line with
    /// text — the blank line after, a separator, stays drawn.
    pub lines: usize,
    /// Whether its text is cut short to fit the row.
    pub cut: bool,
    /// **Whether its text is its block's title, not its first line** — a
    /// command record's (vim's `'foldtext'`): no line's characters are
    /// under its cells, so what marks a line's characters (a selection, a
    /// secret) is not laid onto it char by char.
    pub titled: bool,
    /// **A thought the turn ended before it did** (`Output::close_thought`)
    /// — its last line says so, and that line is under the fold: the row
    /// says it (S21-Q13; review stop 15, E M3).
    pub cut_off: bool,
}

impl FoldRow {
    /// **What follows the row's text when a value the model wrote itself
    /// lies under it** — the view's to say, which holds the secret table
    /// (`view::hides_a_leak`), in `ui.secret.leaked` as the value is drawn
    /// open (S21-Q13; E M3: shut, nothing said a thought held one).
    pub const LEAKED: &str = " · leaked value";

    /// What follows the row's text: how many lines it holds, and that it
    /// was cut off.
    pub fn suffix(self) -> String {
        let lines = match self.lines {
            1 => " · 1 line".to_string(),
            n => format!(" · {n} lines"),
        };
        match self.cut_off {
            true => format!("{lines} · cut off"),
            false => lines,
        }
    }

    /// What is drawn before its text: the shut fold's glyph (`fold::glyph`,
    /// vim's `foldclose`, `+`) and a space.
    pub fn lead() -> String {
        format!("{} ", crate::fold::glyph(false))
    }
}

/// **A window's folds** (N5a, `docs/DesignDocs/navigation.md` §6) — the
/// blocks it set open or shut, over the rule every other block follows. The
/// window's, as vim's folds are: two windows on one session fold apart.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folds {
    /// A block set open (`true`) or shut — `zo` `zc` `za` — and its kind,
    /// so a change to that kind's rule can put it back
    /// ([`to_rule`](Self::to_rule)). One here has a fold, whatever its
    /// kind's rule: a reply you shut by hand ([`has_fold`](Self::has_fold)).
    set: std::collections::HashMap<BlockId, (BlockRole, bool)>,
    /// `zR` (open) or `zM` (shut), by kind (`role as usize`): every block
    /// of it not set since. `None` for a kind with no fold by rule.
    all: [Option<bool>; BlockRole::ALL.len()],
}

impl Folds {
    /// Set `block`, of `role`, open or shut.
    pub fn set(&mut self, block: BlockId, role: BlockRole, open: bool) {
        self.set.insert(block, (role, open));
    }

    /// **`zR` (`open`) or `zM`: every fold there is open or shut** — a block
    /// folded by hand, of any kind, and every block of a kind `rule` gives a
    /// fold, those to come too, until one is set. A reply nobody folded has
    /// no fold and stays drawn, as vim's `zM` leaves lines no fold holds
    /// (Steven, 11-03: "`zM` should fold the folds we set"); it shut every
    /// block of every kind.
    pub fn set_all(&mut self, open: bool, rule: FoldRule) {
        for (_, was) in self.set.values_mut() {
            *was = open;
        }
        for &role in BlockRole::ALL {
            self.all[role as usize] = role.ruled_a_fold(rule).then_some(open);
        }
    }

    /// **Does `block`, of `role`, have a fold** — one made by hand
    /// ([`set`](Self::set)), or its kind's by `rule`: what `zo` finds
    /// there, or says is not (vim's E490).
    pub fn has_fold(&self, block: BlockId, role: BlockRole, rule: FoldRule) -> bool {
        self.set.contains_key(&block) || role.ruled_a_fold(rule)
    }

    /// **Every block of `kinds` back to its kind's rule** — what was set on
    /// one, and `zR`/`zM`'s for its kind, forgotten; every other kind's
    /// kept, since its rule did not change.
    pub fn to_rule(&mut self, kinds: &[BlockRole]) {
        self.set.retain(|_, (role, _)| !kinds.contains(role));
        for &role in kinds {
            self.all[role as usize] = None;
        }
    }

    /// The window's folds, asked by the geometry, every block not set
    /// following `rule`.
    pub fn shut(&self, rule: FoldRule) -> Shut<'_> {
        Shut {
            folds: Some(self),
            rule,
        }
    }
}

/// **Which blocks start shut, by kind** — `[tui.fold]` itself (N5a, NQ3:
/// "Tools and thoughts are default folded. But they can be unfolded
/// depending on settings"): `FoldRule::SHIPPED` shuts thinking and tool
/// records, `FoldRule::OPEN` nothing. The config's type, so no copy of it
/// here can disagree.
pub type FoldRule = shannon_core::config::FoldConfig;

/// How one kind of block starts under a [`FoldRule`].
pub type FoldStart = shannon_core::config::FoldStart;

/// **One kind's part of `[tui.fold]`** ([`BlockRole::fold_rule`]).
#[derive(Debug, Clone, Copy)]
pub struct KindRule {
    /// How the kind starts, read out of a whole rule.
    pub start: fn(FoldRule) -> FoldStart,
    /// The key a `:set` names it by.
    pub key: &'static str,
}

/// Does a block of `role`, holding `lines` lines ([`FoldRow::lines`]),
/// start shut under `rule`? Your message and a reply do only if set to.
fn starts_shut(rule: FoldRule, role: BlockRole, lines: usize) -> bool {
    role.fold_rule()
        .is_some_and(|kind| (kind.start)(rule).shuts(lines))
}

/// **The kinds whose rule is not the same in `now` as in `was`** — whose
/// folds a changed `[tui.fold]` puts back to it, and only theirs: a
/// `:set tui.fold.thinking=open` after `zM` opened every reply too
/// (Steven, 11-01).
pub fn ruled_otherwise(was: FoldRule, now: FoldRule) -> Vec<BlockRole> {
    let start = |rule, role: BlockRole| role.fold_rule().map(|kind| (kind.start)(rule));
    (BlockRole::ALL.iter().copied())
        .filter(|&role| start(was, role) != start(now, role))
        .collect()
}

/// **The kinds whose rule a `:set` of `keys` names** — `tui.fold.thinking`,
/// or a key inside it, names thinking; `tui.fold` every kind a rule has.
pub fn ruled_by_keys(keys: &[String]) -> Vec<BlockRole> {
    use shannon_core::config::key_is_under;
    let names = |kind: &str| (keys.iter()).any(|k| key_is_under(k, kind) || key_is_under(kind, k));
    (BlockRole::ALL.iter().copied())
        .filter(|role| role.fold_rule().is_some_and(|kind| names(kind.key)))
        .collect()
}

/// **The shapes a window draws its blocks in** — asked of every question
/// about rows ([`Output::visible_window`], [`Output::focused_window`], the
/// scroll and click geometry), so what is drawn and where a key lands are
/// one answer. A block it holds shut draws its shut rows where it has them.
#[derive(Debug, Clone, Copy)]
pub struct Shut<'a> {
    folds: Option<&'a Folds>,
    rule: FoldRule,
}

impl Shut<'_> {
    /// A window that folds nothing: every block open.
    pub const NONE: Shut<'static> = Shut {
        folds: None,
        rule: FoldRule::OPEN,
    };

    /// Does this window hold `block` shut — what it set, else `zR`/`zM`'s,
    /// else the rule for its kind and length?
    fn holds(&self, block: &Block) -> bool {
        let set = (self.folds.and_then(|f| f.set.get(&block.id))).map(|&(_, open)| open);
        let all = self.folds.and_then(|f| f.all[block.role as usize]);
        let shuts = || starts_shut(self.rule, block.role, block.content());
        set.or(all).map_or_else(shuts, |open| !open)
    }
}

/// A block's rows in one shape — open, or shut ([`WrapCache`]). `styles` is
/// parallel to `rows`: `styles[r]` has one [`CellStyle`] per *visible* char
/// of `rows[r]` (markdown styling, computed once here and cached). A plain
/// row is all-`PLAIN` and renders exactly as before.
#[derive(Debug, Clone, Default)]
struct Rows {
    rows: Vec<String>,
    styles: Vec<Vec<CellStyle>>,
    /// Visual rows produced per logical line, parallel to the block's `lines`
    /// (sums to `rows.len()`). Lets a caller map a logical line to its visual-row
    /// offset within the block without re-wrapping the whole block — the
    /// geometry the chat cursor overlay (P3) projects onto.
    line_rows: Vec<usize>,
    /// Logical char offset (within its line) at which each visual row's text
    /// begins — parallel to `rows`. Lets the focused-chat window map a logical
    /// selection range onto each visual row's cells without re-wrapping.
    row_offset: Vec<usize>,
    /// The block-local logical line each visual row belongs to — parallel to
    /// `rows`, so a window reports a row's origin in O(1) rather than walking
    /// the block from its first row.
    row_line: Vec<usize>,
    /// **Shut**: what its first row stands for; `None` open. The shut shape's
    /// first row is the fold's: one row high, on its first line
    /// (`line_rows[0] == 1`, each other content line none), showing its
    /// first line with text (`row_line[0]`), cut to the row.
    fold: Option<FoldRow>,
}

impl Block {
    /// Nothing but empty lines — an assistant block that was opened and never
    /// streamed into.
    fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    /// **How many of its lines a fold of it holds** — to its last line with
    /// text; the blank ones after are its separator, which stays drawn. 0
    /// when it has no text.
    fn content(&self) -> usize {
        (self.lines.iter())
            .rposition(|l| !l.trim().is_empty())
            .map_or(0, |last| last + 1)
    }

    /// **`call`'s running line** — only that is a place a record closing the
    /// call takes; a record already closed takes nothing.
    fn runs(&self, call: &str) -> bool {
        self.live_only && self.call.as_deref() == Some(call)
    }

    /// Tag this block with the turn it belongs to.
    fn of_turn(mut self, turn: usize) -> Self {
        self.turn = Some(turn);
        self
    }

    fn new(role: BlockRole, sealed: bool, lines: Vec<String>) -> Self {
        Self {
            role,
            turn: None,
            live_only: false,
            dropped: false,
            of_a_mark: false,
            title: None,
            call: None,
            sealed,
            lines,
            given: Vec::new(),
            shown: Vec::new(),
            styled: RefCell::new(None),
            cache: RefCell::new(Default::default()),
            held: Cell::new(None),
            version: next_version(),
            id: BlockId::next(),
        }
    }

    /// **Its content changed**: a new version, and the caches stale so the
    /// next read re-styles and re-wraps. Called whenever `lines` changes, or
    /// `sealed`, which styling reads.
    fn invalidate(&mut self) {
        self.version = next_version();
        *self.styled.borrow_mut() = None;
        for slot in self.cache.borrow_mut().iter_mut() {
            slot.width = 0;
        }
    }

    /// **Append streamed text** — the reply's ([`Output::append_shown`]) or
    /// a thought's ([`Output::append_thought`]): embedded `'\n'` splits it,
    /// the part before the newline joining the last line, each part after a
    /// fresh line, each sanitized; `shown`'s ranges kept where they land.
    fn append(&mut self, chunk: &str, shown: &[(usize, usize)]) {
        if !shown.is_empty() {
            // Where each char of `chunk` lands, as the split and
            // `sanitize_line` put it: `None` for a newline or a dropped char.
            let mut line = self.lines.len().saturating_sub(1);
            let mut col = self.lines.last().map_or(0, |l| l.chars().count());
            let lands: Vec<Option<(usize, usize)>> = chunk
                .chars()
                .map(|c| match c {
                    '\n' => {
                        (line, col) = (line + 1, 0);
                        None
                    }
                    c if c.is_control() && c != '\t' => None,
                    _ => {
                        col += 1;
                        Some((line, col - 1))
                    }
                })
                .collect();
            for &(lo, hi) in shown {
                let mut run: Option<(usize, usize, usize)> = None;
                for &(at_line, at) in lands.iter().take(hi).skip(lo).flatten() {
                    run = match run {
                        Some((l, from, _)) if l == at_line => Some((l, from, at + 1)),
                        Some(done) => {
                            self.shown.push(done);
                            Some((at_line, at, at + 1))
                        }
                        None => Some((at_line, at, at + 1)),
                    };
                }
                self.shown.extend(run);
            }
        }

        let mut parts = chunk.split('\n');
        // `split` always yields at least one element, even for "".
        let first = sanitize_line(parts.next().unwrap_or(""));
        match self.lines.last_mut() {
            Some(last) => last.push_str(&first),
            None => self.lines.push(first),
        }
        self.lines.extend(parts.map(sanitize_line));
        self.invalidate();
    }

    /// The block's wrapped rows at `width`, recomputing them only if no
    /// slot holds them, then borrowing them. The one place the "ensure
    /// freshness, then read" pattern lives — callers slice the returned rows
    /// directly.
    fn cached_rows(&self, width: u16, ops: &Ops) -> Ref<'_, WrapCache> {
        self.ensure(width, ops);
        Ref::map(self.cache.borrow(), |slots| &slots[0])
    }

    /// **Its rows at `width` in the shape `shut` asks** — shut where it can
    /// shut, else open.
    fn shape(&self, width: u16, ops: &Ops, shut: &Shut<'_>) -> Ref<'_, Rows> {
        let held = shut.holds(self);
        Ref::map(self.cached_rows(width, ops), |cache| {
            match (held, &cache.shut) {
                (true, Some(rows)) => rows,
                _ => &cache.open,
            }
        })
    }

    /// **Whether this block can shut at `width`** — its content two rows or
    /// more there ([`shut_rows`](Self::shut_rows): vim's `foldminlines`,
    /// counted in screen lines, measured in Neovim). The one answer the
    /// drawing ([`shape`](Self::shape), by the shut rows it holds), the `z`
    /// verbs ([`Output::fold_at`]) and the keys' copy
    /// ([`Output::shut_lines`]) ask: the copy counted LINES, so a one-line
    /// thought wrapped in a narrow window drew shut and the keys walked
    /// into it (review stop 15, A L9 / C M4 / D M7).
    fn can_shut(&self, width: u16, ops: &Ops) -> bool {
        width > 0 && self.cached_rows(width, ops).shut.is_some()
    }

    /// Number of visual rows this block occupies at `width` in the shape
    /// `shut` asks, recomputing the wrap only if no slot holds it. `ops`
    /// counts the recomputes so tests can prove the caches are actually
    /// reused.
    fn row_count(&self, width: u16, ops: &Ops, shut: &Shut<'_>) -> usize {
        self.shape(width, ops, shut).rows.len()
    }

    /// Make the first slot the rows at `width`: the slot that holds them,
    /// else a fresh wrap over the older slot.
    fn ensure(&self, width: u16, ops: &Ops) {
        let mut slots = self.cache.borrow_mut();
        if slots[0].width == width {
            return;
        }
        if slots[1].width != width {
            slots[1] = self.wrap(width, ops);
            ops.wraps.set(ops.wraps.get() + 1);
        }
        slots.swap(0, 1);
    }

    /// **A reply's styling, per logical line** — styled once per change to
    /// its lines ([`styled`](Self::styled)'s field doc says why).
    fn styled(&self, ops: &Ops) -> Ref<'_, [Vec<CellStyle>]> {
        if self.styled.borrow().is_none() {
            // **No syntect while the block is still arriving.** Every chunk
            // changes its lines, so an open block re-styles its whole
            // length on the next frame; driving syntect through that is what
            // made a streaming reply cost 1.7s of CPU at 4k chars and 25s at
            // 16k. Flat `CODE` while it streams, the real pass once at seal —
            // which is what `highlight.rs` has always claimed happens.
            let highlight = match self.sealed {
                true => crate::markdown::Highlight::Syntax,
                false => crate::markdown::Highlight::Flat,
            };
            let maps = crate::markdown::style_lines_with(&self.lines, highlight);
            *self.styled.borrow_mut() = Some(maps);
            ops.styles.set(ops.styles.get() + 1);
        }
        Ref::map(self.styled.borrow(), |styled| {
            styled.as_deref().unwrap_or_default()
        })
    }

    /// The rows at `width`, each with the slice of its line's style map that
    /// covers its *visible* chars — so wrap and style cannot disagree.
    fn wrap(&self, width: u16, ops: &Ops) -> WrapCache {
        // Only ASSISTANT rows are rendered through the per-char style map
        // (`view::styled_line`); user echoes and tool output take one flat
        // per-role style. Building a map they never read means markdown-parsing
        // — and, on a fenced block, syntax-highlighting — up to
        // `exec.max_output_bytes` of arbitrary command output, then discarding
        // all of it. `cat README.md` is not an exotic thing for an agent to
        // run. (Reviewer D.) A tool record's styles were given with it
        // (`tool_record`), computed once where the call was read. Thinking is
        // the model's markdown too (a heading, a `code` span).
        let styled = self.role.the_models().then(|| self.styled(ops));
        let maps: &[Vec<CellStyle>] = match (&styled, self.role) {
            (Some(styled), _) => styled,
            (None, BlockRole::Tool) => &self.given,
            (None, _) => &[],
        };
        let mut rows = Vec::new();
        let mut styles = Vec::new();
        let mut line_rows = Vec::with_capacity(self.lines.len());
        let mut row_offset = Vec::new();
        let mut row_line = Vec::new();
        for (li, line) in self.lines.iter().enumerate() {
            let smap = maps.get(li).map_or(&[][..], Vec::as_slice);
            let chars: Vec<char> = line.chars().collect();
            let mut offset = 0usize; // logical char offset of the current row
            let before = rows.len();
            for row in crate::wrap::wrap_cells_word(&chars, width as usize) {
                // The row's visible chars are logical `[offset, offset+visible)`;
                // any dropped word-wrap break space sits past `visible` within
                // `char_len`, so the style slice covers exactly the visible run.
                let visible = row.text.chars().count();
                let lo = offset.min(smap.len());
                let hi = (offset + visible).min(smap.len());
                styles.push(smap[lo..hi].to_vec());
                row_offset.push(offset);
                row_line.push(li);
                rows.push(row.text);
                offset += row.char_len;
            }
            line_rows.push(rows.len() - before);
        }
        let open = Rows {
            rows,
            styles,
            line_rows,
            row_offset,
            row_line,
            fold: None,
        };
        let shut = self.shut_rows(&open, maps, width);
        WrapCache { width, open, shut }
    }

    /// **This block shut to one row at `width`** (N5a), from its `open` rows
    /// and its lines' styles — `None` when its content (to its last line
    /// with text) takes fewer than two rows, so shutting it hides nothing.
    /// The row is its title where it has one ([`FoldRow::titled`]), else its
    /// first line with text, cut to leave the row room for what the view
    /// draws around it (`FoldRow::lead`, `…`, the suffix); the blank lines
    /// after the content keep their rows.
    fn shut_rows(&self, open: &Rows, maps: &[Vec<CellStyle>], width: u16) -> Option<Rows> {
        let content = self.content();
        let content_rows: usize = open.line_rows[..content].iter().sum();
        if content_rows < 2 {
            return None;
        }
        let first = (self.lines[..content].iter())
            .position(|l| !l.trim().is_empty())
            .unwrap_or(0);
        let (text, smap) = match &self.title {
            Some((title, styles)) => (title.as_str(), styles.as_slice()),
            None => (
                self.lines[first].as_str(),
                maps.get(first).map_or(&[][..], Vec::as_slice),
            ),
        };
        let mut fold = FoldRow {
            lines: content,
            cut: false,
            titled: self.title.is_some(),
            // Drawn this run only, a thought is the one the turn cut off.
            cut_off: self.role == BlockRole::Thinking && self.live_only,
        };
        let room = (width as usize)
            .saturating_sub(crate::wrap::str_cells(&FoldRow::lead()))
            .saturating_sub(crate::wrap::str_cells(&fold.suffix()));
        let chars: Vec<char> = text.chars().collect();
        let kept = match crate::wrap::cells_width(&chars) <= room {
            true => chars.len(),
            // One cell for the `…` that says it was cut; the cut the one a
            // label takes (`wrap::leading`), not a copy of it (review stop
            // 15, B L4).
            false => {
                fold.cut = true;
                crate::wrap::leading(text, room.saturating_sub(1))
                    .chars()
                    .count()
            }
        };
        let mut line_rows = vec![0; self.lines.len()];
        line_rows[0] = 1;
        line_rows[content..].copy_from_slice(&open.line_rows[content..]);
        let mut rows = Rows {
            rows: vec![chars[..kept].iter().collect()],
            styles: vec![smap[..kept.min(smap.len())].to_vec()],
            line_rows,
            row_offset: vec![0],
            row_line: vec![first],
            fold: Some(fold),
        };
        // The separator's rows, as they are open.
        for r in content_rows..open.rows.len() {
            rows.rows.push(open.rows[r].clone());
            rows.styles.push(open.styles[r].clone());
            rows.row_offset.push(open.row_offset[r]);
            rows.row_line.push(open.row_line[r]);
        }
        Some(rows)
    }
}

/// **A transcript's blocks, and which content they hold** — apart, so no
/// change to them can leave the revision where it was: the list is this
/// module's alone, a read is the slice ([`Deref`](std::ops::Deref)), and
/// every writer asks [`edit`](blocks::Blocks::edit), which moves the
/// revision first. It held because each of six writers remembered to call
/// `changed()`, and the test listing them could not see a seventh (review
/// stop 9, C 11).
mod blocks {
    use super::{Block, next_version};

    #[derive(Debug, Clone, Default)]
    pub(super) struct Blocks {
        list: Vec<Block>,
        /// From the counter the blocks' versions come from, so "has it
        /// changed since you saw it" is one comparison, not a walk of its
        /// history (`unshown`, B1.3). `Output::EMPTY` until the first edit.
        revision: u64,
    }

    impl Blocks {
        /// Which content they hold: equal twice only if nothing changed
        /// between.
        pub(super) fn revision(&self) -> u64 {
            self.revision
        }

        /// **The blocks, to change** — given a revision no content has had
        /// first, so whatever the caller does to them is a change seen.
        pub(super) fn edit(&mut self) -> &mut Vec<Block> {
            self.revision = next_version();
            &mut self.list
        }
    }

    impl std::ops::Deref for Blocks {
        type Target = [Block];

        fn deref(&self) -> &[Block] {
            &self.list
        }
    }

    impl<'a> IntoIterator for &'a Blocks {
        type Item = &'a Block;
        type IntoIter = std::slice::Iter<'a, Block>;

        fn into_iter(self) -> Self::IntoIter {
            self.list.iter()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Output {
    /// The transcript, in order. Each entry is a user echo or an
    /// assistant response (the latter possibly still streaming, if it's
    /// the last block and `!sealed`). Changed only through
    /// [`edit`](blocks::Blocks::edit), which moves the revision.
    blocks: blocks::Blocks,
    /// What the blocks' caches recomputed — pure instrumentation (a
    /// steady-state redraw must bump nothing); read by the cache-reuse
    /// tests.
    ops: Ops,
    /// How many turns have been begun — the index the next one gets.
    turns: usize,
    /// **A thought the stream is in that has drawn nothing yet** — `Some` of
    /// the whitespace it streamed before its first text, waiting to land in
    /// front of it ([`append_thought`](Self::append_thought)): a thought
    /// opens with its first text. `Some("")` too, for one whose first chunk
    /// was only a handle's start, held back. The stream is thinking either
    /// way ([`thinking`](Self::thinking)): asked of the block alone, a held
    /// tail was joined to the reply's first words (the close verifier at
    /// review stop 15, LOW 3, measured: `{{secrHi`).
    thought_ws: Option<String>,
    /// **Records of calls you typed, waiting for the reply's block to end**
    /// ([`push_typed_record`](Self::push_typed_record)) — in the order they
    /// came, each its turn's; put in by [`put_in_order`](Self::put_in_order)
    /// ahead of what it places, and once nothing is being written
    /// ([`place_waiting`](Self::place_waiting)).
    waiting: Vec<Block>,
}

/// The window of visible visual rows produced by `Output::visible_window`,
/// with the bookkeeping a caller needs to scroll relative to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VisibleWindow {
    /// The visible rows, top-to-bottom, already wrapped to width.
    /// Length is `<= height`.
    pub rows: Vec<String>,
    /// Per-row styling, parallel to `rows`: `styles[r]` has one [`CellStyle`]
    /// per visible char of `rows[r]`. The renderer groups equal-style runs into
    /// spans; an all-`PLAIN` row renders as plain text.
    pub styles: Vec<Vec<CellStyle>>,
    /// Which block produced each row, parallel to `rows` — the renderer's hook
    /// for per-role "bubble" treatment (the user echo vs the assistant reply).
    pub roles: Vec<BlockRole>,
    /// Whether each row is of a turn not in the conversation, parallel to
    /// `rows` ([`Span::dropped`]) — painted over its role.
    pub dropped: Vec<bool>,
    /// **The fold each row stands for**, parallel to `rows` — `Some` on a
    /// shut block's one row, whose text is its first line's start: the view
    /// draws [`FoldRow::lead`] before it and the rest after it.
    pub folds: Vec<Option<FoldRow>>,
    /// Total visual rows in the buffer at the requested width.
    pub total_rows: usize,
    /// Index of the topmost visible row within the wrapped sequence.
    pub top: usize,
    /// Where each row begins — `(logical line, char offset)`, parallel to
    /// `rows` — so the renderer matches a secret on its LOGICAL line and slices
    /// it to the row ([`Output::line_at`] gives the line).
    pub origins: Vec<(LineAt, usize)>,
}

/// Where a transcript line lives: `(block, line within it)` — what a window's
/// `origins` carry, so the renderer reads a row's logical line in O(1)
/// ([`Output::line_at`]). It was a flat index across every block, which the
/// lookup walked the history to resolve — once per visible line, every frame
/// a secret was held: 1.1 ms a frame over 20k blocks, against 0.2 ms for the
/// window itself (Reviewer B, measured).
pub type LineAt = (usize, usize);

/// **A place in the conversation, not on the screen** — a block and a place
/// in its lines: what a mark keeps (`navigation.md` §7; NQ6, Steven 10-27:
/// "we need to be able to have the coordinates" — session, turn, block,
/// offset). The session is the buffer this transcript is; the turn is the
/// block's ([`Span::turn`]). Resolved through the blocks as they are now
/// ([`Output::locate`]), so a reply streaming below it, a record inserted
/// ahead of it, a fold or a resize cannot move it.
///
/// **Kept past the run as a [`Position`]**: the store keeps turns, not
/// blocks, so a session saves its marks as a turn, the block's role and
/// which of that role, and the place in it ([`Output::position_of`]), and
/// opens them through a replay that draws the turn as it was drawn live
/// ([`Output::anchor_of`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    /// The block it is in.
    pub block: BlockId,
    /// The line, within the block.
    pub line: usize,
    /// The column, in chars into the line.
    pub col: usize,
}

/// **A place in terms that outlive a run** — a turn, the block of that turn
/// by its role and which of that role, and a line and column in it: what a
/// session keeps of a mark (`shannon_core::Mark`), where an [`Anchor`]'s
/// block id lasts only the run. A replay draws a turn's blocks as the live
/// turn drew them (`App::replay_turn`) — all but a call's `⚙ running:`
/// line, drawn live only and counted by neither side — so the same
/// position names the same text: held by
/// `a_reopened_turn_draws_the_blocks_it_drew_live`, whose turn runs a
/// command. Never the line's text: a transcript shows a held secret's value.
///
/// **Its turn is the store's** only while the transcript numbers its turns
/// as the conversation does: a turn the conversation drops, the transcript
/// drops or takes out of every turn ([`Output::abandon_turn`]), or every
/// mark after it is kept one turn late (review stop 14, A 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub turn: usize,
    pub role: BlockRole,
    pub nth: usize,
    pub line: usize,
    pub col: usize,
}

/// **A window's place as positions** (S21-Q4, Steven 11-01: "Carry over
/// whole place by position") — its cursor, the line at its top, and the
/// blocks it opened or shut, each a [`Position`] already in the numbering
/// of the session made from its own ([`Output::keep_view`]); found again in
/// that session's transcript ([`Output::find_view`]), as vim's
/// `winsaveview` keeps a window's cursor and top line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeptView {
    cursor: Option<Position>,
    top: Option<Position>,
    folds: Vec<(Position, bool)>,
    all: [Option<bool>; BlockRole::ALL.len()],
}

/// **A [`KeptView`], found in a transcript** — the cursor's line and column
/// and the top's, where it has them, and the folds on its blocks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoundView {
    pub cursor: Option<(usize, usize)>,
    pub top: Option<(usize, usize)>,
    pub folds: Folds,
}

/// **Why a place is not kept past the run** ([`Output::position_of`]) — said
/// when a mark is set there, or leaves what is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unkept {
    /// Its block is gone (popped, or dropped with its turn).
    Gone,
    /// Its block is in no turn: a notice.
    NoTurn,
    /// Its block is of a turn the conversation dropped, kept on screen
    /// ([`Output::abandon_turn`]).
    NotInConversation,
    /// Its block is drawn only this run: a call's `⚙ running:` line, or a
    /// thought cut off with its turn — a reopened turn draws neither.
    LiveOnly,
}

impl Unkept {
    /// Why, in words that follow "mark `a` …, not saved: " (`App::set_mark`,
    /// `App::keep_moved_marks`).
    pub fn why(self) -> &'static str {
        match self {
            Unkept::Gone => "its line is gone",
            Unkept::NoTurn => "its line is in no turn",
            Unkept::NotInConversation => "its turn is not in the conversation",
            Unkept::LiveOnly => "its line is drawn only this run, not when the session reopens",
        }
    }
}

/// **A place, found** ([`Output::find_each`]): its logical line of
/// [`Output::lines`], its column, its block's turn, and its line's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found<'a> {
    pub line: usize,
    pub col: usize,
    pub turn: Option<usize>,
    pub text: &'a str,
}

/// **One block, where it is now** — what [`Output::spans`] yields, the walk
/// from blocks to lines (`navigation.md` §4, "one query per session").
/// Read by [`sections`](Output::sections),
/// [`turn_of_line`](Output::turn_of_line) and [`anchor_at`](Output::anchor_at)
/// (`span_of_line`), [`locate`](Output::locate),
/// [`find_each`](Output::find_each), [`position_of`](Output::position_of),
/// [`anchor_of`](Output::anchor_of), and in the app `set_mark`,
/// `answer_mark_call` and `found_in` (counted 10-31, review stop 14, A 9: the
/// list named five). **Not the one walk yet**: `find_quote` walks the blocks
/// for a flag a span does not carry (`of_a_mark`); the two windows
/// (`visible_window`, `focused_window`) walk them for their wrap caches;
/// `lines`, `line_count` count them; and `shut_lines` walks them for each
/// one's content, as the windows draw it shut. [`fold_at`](Output::fold_at)
/// reads it (N5a). Not yet wired: the block motions and the gutter
/// (navigation N2, N6), which must agree with the windows.
#[derive(Debug, Clone, Copy)]
pub struct Span<'a> {
    /// Which block ([`BlockId`]).
    pub id: BlockId,
    /// The turn it belongs to; `None` for a defensive block no turn opened
    /// (`end_turn` or a chunk with no open block).
    pub turn: Option<usize>,
    /// Who produced it.
    pub role: BlockRole,
    /// Drawn only while live: a reopened turn draws no block in its place
    /// ([`Unkept::LiveOnly`]).
    pub live_only: bool,
    /// Of a turn not in the conversation, kept on screen
    /// ([`Output::abandon_turn`], [`Unkept::NotInConversation`]).
    pub dropped: bool,
    /// Its first line, as an index into [`Output::lines`].
    pub first: usize,
    /// Its lines.
    pub lines: &'a [String],
}

/// The visible window for the **focused chat**: the same styled, role-tagged,
/// word-wrapped rows [`VisibleWindow`] carries, plus a per-row selection
/// highlight and the cursor — so focusing the chat looks identical to the
/// streaming view but gains a navigable caret. Produced by
/// [`Output::focused_window`], which frames around the chat cursor (not the
/// scroll viewport), so chat navigation is purely cursor-driven (as it is today).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FocusedWindow {
    /// Visible rows, top-to-bottom (`len <= height`).
    pub rows: Vec<String>,
    /// Per-row styling, parallel to `rows` (markdown/syntax, from the cache).
    pub styles: Vec<Vec<CellStyle>>,
    /// Which block produced each row, parallel to `rows`.
    pub roles: Vec<BlockRole>,
    /// Whether each row is of a turn not in the conversation, parallel to
    /// `rows`; see [`VisibleWindow::dropped`].
    pub dropped: Vec<bool>,
    /// The fold each row stands for; see [`VisibleWindow::folds`].
    pub folds: Vec<Option<FoldRow>>,
    /// Per-row selected **char** span `[lo, hi)` (Visual mode), as offsets into
    /// the row's visible text, parallel to `rows`; `None` where the row carries
    /// no selection. (Char offsets, like the prompt's selection — the renderer
    /// slices chars to reverse-video them.)
    pub selection: Vec<Option<(usize, usize)>>,
    /// The cursor's `(row_in_window, cell)`, or `None` if it fell outside the
    /// window (shouldn't happen — the window frames around it).
    pub cursor: Option<(usize, usize)>,
    /// Where each row begins — `(logical line, char offset)`, parallel to
    /// `rows`; see [`VisibleWindow::origins`].
    pub origins: Vec<(LineAt, usize)>,
}

impl Output {
    pub fn new() -> Self {
        Self::default()
    }

    /// **Which content it holds**: equal twice only if nothing in it changed
    /// between — and [`EMPTY`](Self::EMPTY) only while nothing was ever
    /// written to it.
    pub fn revision(&self) -> u64 {
        self.blocks.revision()
    }

    /// **The revision of a transcript nothing was ever written to**
    /// (`Output::new`) — no content's: every change takes one past it
    /// (`next_version` counts from 1). What a session that arrived has been
    /// seen as, so anything in it is news (`app::news`).
    pub const EMPTY: u64 = 0;

    /// **What the coming turn tells the model beyond its prompt**, as one dim
    /// line ahead of its echo and part of that turn
    /// (`tool_record::context_line`). Nothing when it tells nothing. Every
    /// turn starter and the replay call it, so a reopened session shows what
    /// a live one did.
    pub fn told(&mut self, blocks: &[shannon_core::Block]) {
        if let Some(line) = crate::tool_record::context_line(blocks) {
            let turn = self.turns;
            self.open_your_side(turn);
            (self.blocks.edit())
                .push(Block::new(BlockRole::User, true, sanitize_lines(&line)).of_turn(turn));
        }
    }

    /// Begin a turn: push your side of it, then open a fresh empty assistant
    /// block to receive the streamed response. Pair with `end_turn` once the
    /// response finishes.
    ///
    /// Your side is framed: a rule, what the turn was told (`told`), the
    /// echo, a rule, then one blank line before the reply.
    ///
    /// The chat-transcript grammar (user-echo prefix, the frame, streaming
    /// target, inter-turn separator) is owned here rather than reconstructed
    /// at every event-loop callsite.
    pub fn begin_turn(&mut self, user_text: &str) {
        let echo = sanitize_lines(&format!("> {user_text}"));
        let turn = self.turns;
        self.turns += 1;
        self.open_your_side(turn);
        let blocks = self.blocks.edit();
        blocks.push(Block::new(BlockRole::User, true, echo).of_turn(turn));
        blocks.push(Block::new(BlockRole::Rule, true, vec![String::new()]).of_turn(turn));
        // The blank line after the frame is your side's, not the reply's: a
        // tool record that arrives before any text goes in front of the open
        // block (`push_tool_record`), and so lands below it, not on the rule.
        blocks.push(Block::new(BlockRole::User, true, vec![String::new()]).of_turn(turn));
        // The open assistant block starts with one empty line — the
        // streaming target that `append_chunk` builds onto.
        blocks.push(Block::new(BlockRole::Assistant, false, vec![String::new()]).of_turn(turn));
    }

    /// The rule that opens `turn`'s side — once: `told` opens it when the
    /// turn carries a context line, which then sits inside the frame, and
    /// `begin_turn` finds it open.
    fn open_your_side(&mut self, turn: usize) {
        if self.blocks.last().is_some_and(|b| b.turn == Some(turn)) {
            return;
        }
        (self.blocks.edit())
            .push(Block::new(BlockRole::Rule, true, vec![String::new()]).of_turn(turn));
    }

    /// Which turn the logical line at `line` belongs to, if any.
    ///
    /// The transcript cursor indexes [`lines`](Self::lines), so this is the
    /// bridge from "where the cursor is" to "which turn that is" — and
    /// therefore from a keypress in the chat pane to a node in the graph.
    pub fn turn_of_line(&self, line: usize) -> Option<usize> {
        self.span_of_line(line)?.turn
    }

    /// **Every block, where it is now**, in order — the one walk from blocks
    /// to lines ([`Span`]). O(blocks), as each of its readers was before it.
    pub fn spans(&self) -> impl Iterator<Item = Span<'_>> {
        let mut first = 0;
        self.blocks.iter().map(move |block| {
            let span = Span {
                id: block.id,
                turn: block.turn,
                role: block.role,
                live_only: block.live_only,
                dropped: block.dropped,
                first,
                lines: &block.lines,
            };
            first += block.lines.len();
            span
        })
    }

    /// The block holding logical line `line`, if any.
    fn span_of_line(&self, line: usize) -> Option<Span<'_>> {
        self.spans()
            .find(|span| line < span.first + span.lines.len())
    }

    /// **The place at logical line `line`, column `col`**, in the
    /// conversation's coordinates — `None` past the last line.
    pub fn anchor_at(&self, line: usize, col: usize) -> Option<Anchor> {
        let span = self.span_of_line(line)?;
        Some(Anchor {
            block: span.id,
            line: line - span.first,
            col,
        })
    }

    /// **The latest turn's index** — the one being answered, or the last
    /// answered — or `None` before any.
    pub fn last_turn(&self) -> Option<usize> {
        self.turns.checked_sub(1)
    }

    /// **The first place in `turn` where `quote` stands on one line**, or,
    /// with no turn, in the latest turn that has it, searching back — what
    /// the agent's `mark` tool means by `at` (`ToolDef::mark`). Exact and
    /// within a line: a model copies a phrase well, and a match that spans
    /// a line break, or fuzzes, would be a guess at which place was meant.
    /// The column is in chars. `None` for an empty quote. One pass: the
    /// first match of the latest turn that has one.
    ///
    /// **Each block is searched as the model was sent it** (`as_sent`), never
    /// as drawn: the transcript draws a held value where the model wrote its
    /// handle, and an answer of found or not over the drawn text told the
    /// model one bit a call about a value it was never sent — read a prefix
    /// at a time (review stop 14, B F1). The BLOCK, not each line: a value
    /// across lines (a PEM key) is no line's, and a line at a time sent its
    /// middle line in clear (Reviewer E F2). A line `as_sent` changes lands
    /// at its start, its drawn columns not the ones the model quoted; a
    /// block whose lines a value joined lands at the block's start.
    ///
    /// **Never a call's running line** ([`Unkept::LiveOnly`]): the record
    /// that closes the call holds the same command, and a mark there is kept.
    ///
    /// **Nor what asked, nor a mark** (review stop 14, A 6): searching back
    /// with no turn named, this turn's own message is left out — "mark where
    /// we chose BTreeMap" found "BTreeMap" in your request, the latest line
    /// holding it — and so is every record of a `mark`, whose result repeats
    /// its quote, so a second mark at a phrase landed on the first's record.
    /// Named by `turns_ago`, this turn's message is searched as any line.
    ///
    /// **Nor its thinking** (N1b): what is drawn is the API's summary of the
    /// model's reasoning, not words the model wrote, so a quote found there
    /// matched a paraphrase by chance — and found first, as thinking stands
    /// before the reply, it took the mark from the line meant.
    pub fn find_quote(
        &self,
        quote: &str,
        turn: Option<usize>,
        as_sent: impl Fn(&str) -> String,
    ) -> Option<Anchor> {
        if quote.is_empty() {
            return None;
        }
        let asking = |block: &Block| {
            turn.is_none() && block.role == BlockRole::User && block.turn == self.last_turn()
        };
        let mut best: Option<(usize, Anchor)> = None;
        let quotable = |block: &&Block| {
            !block.live_only
                && !block.of_a_mark
                && block.role != BlockRole::Thinking
                && !asking(block)
        };
        for block in self.blocks.iter().filter(quotable) {
            let Some(at_turn) = block.turn else { continue };
            let wanted = turn.is_none_or(|turn| turn == at_turn);
            if !wanted || best.is_some_and(|(seen, _)| seen >= at_turn) {
                continue;
            }
            let sent = as_sent(&block.lines.join("\n"));
            let sent: Vec<&str> = sent.split('\n').collect();
            // A handle has no line break, so a value across lines leaves
            // fewer: the lines no longer match one to one.
            let aligned = sent.len() == block.lines.len();
            let found = sent.iter().enumerate().find_map(|(line, text)| {
                let at = text.find(quote)?;
                Some(match aligned {
                    true => Anchor {
                        block: block.id,
                        line,
                        col: match *text == block.lines[line] {
                            true => text[..at].chars().count(),
                            false => 0,
                        },
                    },
                    false => Anchor {
                        block: block.id,
                        line: 0,
                        col: 0,
                    },
                })
            });
            if let Some(anchor) = found {
                best = Some((at_turn, anchor));
            }
        }
        best.map(|(_, anchor)| anchor)
    }

    /// **`anchor` as a [`Position`]**, to keep past the run — or why it
    /// cannot be. Which of its role it is counts only the blocks a reopened
    /// turn draws, as [`anchor_of`](Self::anchor_of) does.
    pub fn position_of(&self, anchor: Anchor) -> Result<Position, Unkept> {
        let spans: Vec<Span<'_>> = self.spans().collect();
        let at = (spans.iter())
            .position(|span| span.id == anchor.block)
            .ok_or(Unkept::Gone)?;
        // Not in the conversation first: true of every line of its turn, a
        // running line among them, and the larger fact.
        if spans[at].dropped {
            return Err(Unkept::NotInConversation);
        }
        if spans[at].live_only {
            return Err(Unkept::LiveOnly);
        }
        let (turn, role) = (spans[at].turn.ok_or(Unkept::NoTurn)?, spans[at].role);
        let nth = (spans[..at].iter())
            .filter(|span| span.turn == Some(turn) && span.role == role && !span.live_only)
            .count();
        Ok(Position {
            turn,
            role,
            nth,
            line: anchor.line,
            col: anchor.col,
        })
    }

    /// **The [`Anchor`] a kept [`Position`] names here**, or `None` when this
    /// transcript has no such block or line — a session reopened by a
    /// build that draws it differently.
    pub fn anchor_of(&self, position: Position) -> Option<Anchor> {
        let span = (self.spans())
            .filter(|span| span.turn == Some(position.turn) && span.role == position.role)
            .filter(|span| !span.live_only)
            .nth(position.nth)?;
        (position.line < span.lines.len()).then_some(Anchor {
            block: span.id,
            line: position.line,
            col: position.col,
        })
    }

    /// **A window's place here, as positions** ([`KeptView`]) — its
    /// `cursor` and its `top` (each a line and column; no top while it
    /// follows the end), and its `folds` — each turn through `map`, the
    /// session made from this one's numbering of it. A place on a turn `map`
    /// drops, or on a block a rebuilt transcript does not draw
    /// ([`position_of`](Self::position_of)), is left out.
    pub fn keep_view(
        &self,
        cursor: (usize, usize),
        top: Option<(usize, usize)>,
        folds: &Folds,
        map: &dyn Fn(usize) -> Option<usize>,
    ) -> KeptView {
        let kept = |anchor: Option<Anchor>| {
            let position = self.position_of(anchor?).ok()?;
            let turn = map(position.turn)?;
            Some(Position { turn, ..position })
        };
        let block = |block| Anchor {
            block,
            line: 0,
            col: 0,
        };
        KeptView {
            cursor: kept(self.anchor_at(cursor.0, cursor.1)),
            top: top.and_then(|(line, col)| kept(self.anchor_at(line, col))),
            folds: (folds.set.iter())
                .filter_map(|(&id, &(_, open))| Some((kept(Some(block(id)))?, open)))
                .collect(),
            all: folds.all,
        }
    }

    /// **`kept`, found here** ([`FoundView`]) — what this transcript does
    /// not have stays `None`, or out of the folds.
    pub fn find_view(&self, kept: &KeptView) -> FoundView {
        let here = |position: &Position| self.locate(self.anchor_of(*position)?);
        let set = (kept.folds.iter())
            .filter_map(|(at, open)| Some((self.anchor_of(*at)?.block, (at.role, *open))))
            .collect();
        FoundView {
            cursor: kept.cursor.as_ref().and_then(here),
            top: kept.top.as_ref().and_then(here),
            folds: Folds { set, all: kept.all },
        }
    }

    /// **Where each of `anchors` is now, in one walk** — its line and column
    /// (as [`locate`](Self::locate)), its turn, and its line's text; `None`
    /// for one whose block is gone. What is asked of many places at once, and
    /// often: a window's copy is told its marks at every sync, `:marks` is
    /// derived every frame it is shown — a `locate` each walked the blocks
    /// once a place.
    pub fn find_each(&self, anchors: &[Anchor]) -> Vec<Option<Found<'_>>> {
        let mut wanted: std::collections::HashMap<BlockId, Vec<usize>> =
            std::collections::HashMap::new();
        for (at, anchor) in anchors.iter().enumerate() {
            wanted.entry(anchor.block).or_default().push(at);
        }
        let mut found = vec![None; anchors.len()];
        for span in self.spans() {
            let Some(ats) = wanted.get(&span.id) else {
                continue;
            };
            for &at in ats {
                let anchor = anchors[at];
                found[at] = span.lines.get(anchor.line).map(|text| Found {
                    line: span.first + anchor.line,
                    col: anchor.col,
                    turn: span.turn,
                    text: text.as_str(),
                });
            }
        }
        found
    }

    /// **Where `anchor` is now** — its logical line of
    /// [`lines`](Self::lines) and its column; `None` once its block is gone (popped by
    /// `end_turn`, truncated with an abandoned turn), as vim erases a mark
    /// with its line. A block's lines grow at its end (every writer pushes,
    /// appends to the last line, or truncates whole blocks), so the line and
    /// the column still name the text they did — but one: a call's running
    /// line, replaced whole by the record closing it (`push_tool_record`);
    /// its line 0 is the record's first, the same call's.
    pub fn locate(&self, anchor: Anchor) -> Option<(usize, usize)> {
        let span = self.spans().find(|span| span.id == anchor.block)?;
        (anchor.line < span.lines.len()).then(|| (span.first + anchor.line, anchor.col))
    }

    /// **Where each turn begins and ends**, as logical lines of
    /// [`lines`](Self::lines), each list ascending: the transcript's SECTIONS
    /// (`prompt::Sections::At`) — `]]` `[[` move between the starts, `][`
    /// `[]` to the ends — as neovim makes each shell prompt one in a terminal
    /// (`:h terminal_]]`). A turn begins at its first block, the rule above
    /// your message, so `y]]` from there takes the whole turn; it ends at its
    /// last line of text — the reply's, once there is one — as a `{` section
    /// ends at its `}`.
    pub fn sections(&self) -> (Vec<usize>, Vec<usize>) {
        let (mut starts, mut ends) = (Vec::new(), Vec::new());
        let mut previous = None;
        // The turn's last line of text so far.
        let mut text = None;
        for span in self.spans() {
            if span.turn != previous {
                ends.extend(text.take());
                if span.turn.is_some() {
                    starts.push(span.first);
                }
            }
            previous = span.turn;
            if span.turn.is_some()
                && let Some(at) = span.lines.iter().rposition(|l| !l.trim().is_empty())
            {
                text = Some(span.first + at);
            }
        }
        ends.extend(text);
        (starts, ends)
    }

    /// **The lines a window's shut folds hold**, each `[start, end)` of
    /// [`lines`](Self::lines), in order — what its navigation copy is told
    /// (`Buffer::set_folds`), so `j` `k`, `w` `e` and an operator take as
    /// one line what it draws as one row: each block it draws shut at
    /// `width` (`Block::can_shut`) — a long line wrapped is one, folding
    /// that one line. `None` for a window not drawn yet: no width, and the
    /// rule with no limit to the width — two lines of content or more.
    pub fn shut_lines(&self, shut: &Shut<'_>, width: Option<u16>) -> Vec<(usize, usize)> {
        let mut first = 0;
        let mut folds = Vec::new();
        for block in self.blocks.iter() {
            let content = block.content();
            let shuts = match width {
                Some(width) => block.can_shut(width, &self.ops),
                None => content >= 2,
            };
            if shuts && shut.holds(block) {
                folds.push((first, first + content));
            }
            first += block.lines.len();
        }
        folds
    }

    /// **The fold at logical line `line`**, at `width` — the block holding it
    /// when the line is in its content and the block can shut there (its
    /// content two rows or more) — as its id and kind, and whether `shut`
    /// (the window's) holds it shut; `None` where there is none: what `zo`
    /// `zc` `za` act on, and say they cannot (vim's E490), and what a search
    /// or a mark landing in opens.
    pub fn fold_at(
        &self,
        line: usize,
        width: u16,
        shut: &Shut<'_>,
    ) -> Option<(BlockId, BlockRole, bool)> {
        let span = self.span_of_line(line)?;
        let block = self.blocks.iter().find(|b| b.id == span.id)?;
        let inside = line - span.first < block.content();
        let shuts = block.can_shut(width, &self.ops);
        (inside && shuts).then(|| (block.id, block.role, shut.holds(block)))
    }

    /// Seal the in-progress assistant block. Appends a trailing blank
    /// line *to that block* so consecutive turns render with a separating
    /// row — the separator is content of the just-finished turn, which is
    /// why blocks can simply concatenate with no spacer logic.
    pub fn end_turn(&mut self) {
        // A thought still open is one whose block never came: the turn ended
        // first, and keeps none of it.
        self.cut_thought();
        let blocks = self.blocks.edit();
        // After a tool record or a thought, each carrying its own separator,
        // an open block that received nothing is the next round that never
        // came: dropped, or the turn ended in three blank rows (review 20,
        // Reviewer A).
        if let [.., record, open] = blocks.as_slice()
            && matches!(record.role, BlockRole::Tool | BlockRole::Thinking)
            && record.sealed
            && !open.sealed
            && open.is_empty()
        {
            blocks.pop();
        } else {
            match blocks.last_mut().filter(|block| !block.sealed) {
                Some(block) => {
                    block.lines.push(String::new());
                    block.sealed = true;
                    block.invalidate();
                }
                // Defensive: `end_turn` without an open turn shouldn't happen
                // in the app flow, but if it does, emit a bare separator block
                // rather than silently dropping the call.
                None => blocks.push(Block::new(BlockRole::Assistant, true, vec![String::new()])),
            }
        }
        // A typed record that waited for the reply's block goes in after it.
        self.place_waiting();
    }

    /// Append `chunk` to the open (streaming) assistant block. Embedded
    /// `'\n'` splits across lines: the part before the newline joins the
    /// block's last line, the parts after start fresh lines. Each segment
    /// is sanitized of control characters — this is the streamed
    /// model-output path, the one most likely to carry an injected escape
    /// sequence. If no block is open, one is created (a safety net;
    /// `begin_turn` normally makes the target first).
    pub fn append_chunk(&mut self, chunk: &str) {
        self.append_shown(chunk, &[]);
    }

    /// [`append_chunk`](Self::append_chunk), with the char ranges of `chunk`
    /// where a held secret was put back from its handle
    /// (`Aliases::unmask_shown`), kept as where they land in the block's
    /// lines — past each newline, and past each control char the sanitizing
    /// drops — for [`shown_at`](Self::shown_at).
    ///
    /// Text after a thought ends it: the reply moved on.
    pub fn append_shown(&mut self, chunk: &str, shown: &[(usize, usize)]) {
        self.close_thought(false);
        let blocks = self.blocks.edit();
        if blocks.last().is_none_or(|block| block.sealed) {
            blocks.push(Block::new(BlockRole::Assistant, false, vec![String::new()]));
        }
        (blocks.last_mut())
            .filter(|block| !block.sealed)
            .expect("just ensured an open block exists")
            .append(chunk, shown);
    }

    /// **The model's thinking, as it streams** (N1b) — onto the thinking
    /// block open, or a new one placed where it was written, as a tool
    /// record is (`put_in_order`): after the reply's text so far, the
    /// reply's next words streaming below it. `shown` as
    /// [`append_shown`](Self::append_shown)'s. A reopened turn draws its
    /// kept blocks through here too (`App::replay_turn`), a block's whole
    /// text as one chunk, so live and reopened draw alike.
    pub fn append_thought(&mut self, chunk: &str, shown: &[(usize, usize)]) {
        let at = match self.open_thought() {
            Some(at) => at,
            // **A thought opens with its first text** — whitespace before it
            // waits ([`thought_ws`](Self::thought_ws)) and lands in front of
            // it, so what is drawn is the block as kept. One of whitespace
            // alone never opens: kept as blank rows mid-turn, no fold could
            // shut it (`zc`: "no fold here"), and it split the reply in two
            // around it (review stop 15, B L3).
            None if chunk.trim().is_empty() => {
                self.thought_ws.get_or_insert_default().push_str(chunk);
                return;
            }
            None => {
                let turn = self.turns.saturating_sub(1);
                let open = Block::new(BlockRole::Thinking, false, vec![String::new()]);
                self.put_in_order(open.of_turn(turn))
            }
        };
        let waiting = self.thought_ws.take().unwrap_or_default();
        let thought = &mut self.blocks.edit()[at];
        thought.append(&waiting, &[]);
        thought.append(chunk, shown);
    }

    /// **The thinking block's own end** — its block arrived whole
    /// (`StreamMsg::Thought`): sealed, with the separator a record carries.
    pub fn end_thought(&mut self) {
        self.close_thought(false);
    }

    /// **The open thought's end, without its block** — its work ended first
    /// (`App::resolve`, before a failure's line is written; `end_turn` too):
    /// said cut, and drawn this run only.
    pub fn cut_thought(&mut self) {
        self.close_thought(true);
    }

    /// Is the stream in a thought — its block open, or one not yet drawn
    /// (whitespace or a held tail waiting, `thought_ws`) — its end not come?
    pub fn thinking(&self) -> bool {
        self.open_thought().is_some() || self.thought_ws.is_some()
    }

    /// **What the stream held back** (`App::unmask_stream`'s tail, never a
    /// whole handle), landed where the stream was: the open thought, else
    /// the reply.
    pub fn append_held(&mut self, tail: &str) {
        match (tail.is_empty(), self.thinking()) {
            (true, _) => {}
            (false, true) => self.append_thought(tail, &[]),
            (false, false) => self.append_chunk(tail),
        }
    }

    /// The open thinking block, if any: last, or just before the reply's
    /// open block ([`put_in_order`](Self::put_in_order) puts it there).
    fn open_thought(&self) -> Option<usize> {
        let len = self.blocks.len();
        (len.saturating_sub(2)..len)
            .rev()
            .find(|&at| self.blocks[at].role == BlockRole::Thinking && !self.blocks[at].sealed)
    }

    /// **Close the open thought**, if any — `cut` when the turn ended before
    /// its block came: said so on its last line, and drawn this run only
    /// ([`Unkept::LiveOnly`]), since the turn keeps no block it never got.
    /// One that drew nothing goes.
    fn close_thought(&mut self, cut: bool) {
        self.end_open_thought(cut);
        // A typed record that waited for the thought to end goes in.
        self.place_waiting();
    }

    /// [`close_thought`](Self::close_thought)'s thought.
    fn end_open_thought(&mut self, cut: bool) {
        /// The line a cut thought ends on.
        const CUT: &str =
            "\n[cut off — the turn ended before this thought did, and does not keep it]";
        // Whitespace that never met text was never a thought drawn, and the
        // stream is out of it.
        self.thought_ws = None;
        let Some(at) = self.open_thought() else {
            return;
        };
        let blocks = self.blocks.edit();
        if blocks[at].is_empty() {
            blocks.remove(at);
            return;
        }
        let thought = &mut blocks[at];
        // **Its trailing blank lines are not drawn**: the API ends a
        // thought's text with two newlines (measured, 11-01: every thinking
        // block of a session), and drawn with its separator they stood as
        // three blank rows before the reply, where one stands between any
        // two blocks — shut too, since a fold keeps the blanks after its
        // text drawn (`content`). What is kept is the API's, untouched.
        let text = (thought.lines.iter())
            .rposition(|l| !l.trim().is_empty())
            .map_or(1, |last| last + 1);
        thought.lines.truncate(text);
        if cut {
            thought.append(CUT, &[]);
            thought.live_only = true;
        }
        thought.lines.push(String::new());
        thought.sealed = true;
        thought.invalidate();
    }

    /// Record a tool call in the transcript: what was proposed, and how it was
    /// resolved. Sealed immediately (it is a fact, not a stream) and kept out of
    /// the open assistant block so streaming that resumes afterwards is not
    /// interleaved into it.
    ///
    /// `detail` lines are indented under the header so a multi-line result reads
    /// as belonging to its call. A plain record: [`push_tool_record`]
    /// (Self::push_tool_record) takes one built from the call.
    #[cfg(test)]
    pub fn push_tool_activity(&mut self, header: &str, detail: &str) {
        let mut record = crate::tool_record::Record::verbatim(header);
        record.output(detail);
        self.push_tool_record(record);
    }

    /// Record a tool call in the transcript, its lines styled as
    /// [`tool_record`](crate::tool_record) built them.
    ///
    /// **The record closing a call takes its running line's place** (review
    /// stop 15, F H2): put in order below it, the `⚙ running:` line stayed
    /// above the record that said the call had ended — shut, the two read
    /// as one command still running, and another said nothing. In place, the
    /// live transcript draws what a reopened one does, block for block, and
    /// what streamed after it while it ran stays where it was, an open
    /// thought too. Its block keeps its id, so a mark or a fold set on the
    /// running line stays on the call's record; a window's place below it
    /// moves with its lines (`App::anchors_on`).
    pub(crate) fn push_tool_record(&mut self, record: crate::tool_record::Record) {
        self.place_record(record, false);
    }

    /// **The record of a call you typed** (`:tool`) — not the turn's: placed
    /// as [`push_tool_record`](Self::push_tool_record) places one, but while
    /// the reply is writing a block (its text, or a thought) it waits for
    /// that block to end ([`waiting`](Self::waiting)). Placed at once, it cut
    /// the reply where the stream stood, mid-word, and only live: a reopened
    /// turn keeps no typed call, and read whole (Steven, 11-01: "the old
    /// brass h", the record, "and lamp"). The model's own record never comes
    /// mid-block: its text ends before its call. Returns whether it waits,
    /// newly, for the caller to say so.
    pub(crate) fn push_typed_record(&mut self, record: crate::tool_record::Record) -> bool {
        self.place_record(record, true)
    }

    /// [`push_tool_record`](Self::push_tool_record) and
    /// [`push_typed_record`](Self::push_typed_record): `typed` waits out a
    /// block being written. Whether it waits, newly.
    fn place_record(&mut self, record: crate::tool_record::Record, typed: bool) -> bool {
        let (live_only, of_a_mark) = (record.live_only(), record.of_a_mark());
        let title = record.title().cloned();
        let call = record.call_id().map(str::to_string);
        let (mut lines, mut given) = record.into_parts();
        lines.push(String::new()); // separator, as blocks simply concatenate
        given.push(Vec::new());
        if let Some(running) = call.as_deref().and_then(|call| self.running_mut(call)) {
            running.lines = lines;
            running.given = given;
            running.title = title;
            running.live_only = live_only;
            running.of_a_mark = of_a_mark;
            running.invalidate();
            return false;
        }
        // Tagged with the turn that ran it, so `:reply` on a `$ ls` line
        // answers instead of saying "that line is not part of a turn" about a
        // line that plainly is. The field's doc claimed `None` meant only "a
        // replayed header, a notice" — a narrower scope than the code had
        // (Reviewer A).
        let mut record = Block::new(BlockRole::Tool, true, lines);
        record.given = given;
        record.live_only = live_only;
        record.of_a_mark = of_a_mark;
        record.title = title;
        record.call = call;
        let record = record.of_turn(self.turns.saturating_sub(1));
        if typed && self.writing() {
            self.waiting.push(record);
            return true;
        }
        // A call after a thought ends it, as text does.
        self.close_thought(false);
        self.put_in_order(record);
        false
    }

    /// **The reply is writing a block** — a thought open, or its text's
    /// block holding words and not ended: what a typed record waits out.
    fn writing(&self) -> bool {
        self.open_thought().is_some()
            || (self.blocks.last()).is_some_and(|block| !block.sealed && !block.is_empty())
    }

    /// **The typed records waiting, put in** once nothing is being written —
    /// a thought ended, the turn ended — in order, where
    /// [`put_in_order`](Self::put_in_order) places a block: it puts what
    /// waits ahead of the block it places, so placing the last places all.
    fn place_waiting(&mut self) {
        if self.waiting.is_empty() || self.writing() {
            return;
        }
        let last = self.waiting.pop().expect("just checked it is not empty");
        self.put_in_order(last);
    }

    /// **Where `call`'s running line stands**, while it does — the block
    /// the record closing the call takes ([`push_tool_record`](Self::push_tool_record)).
    pub(crate) fn running_at(&self, call: &str) -> Option<usize> {
        (self.blocks.iter()).rposition(|b| b.runs(call))
    }

    /// `call`'s running line, drawn ([`running_at`](Self::running_at)) or
    /// waiting to go in ([`waiting`](Self::waiting)) — the block its closing
    /// record takes.
    fn running_mut(&mut self, call: &str) -> Option<&mut Block> {
        match self.running_at(call) {
            Some(at) => Some(&mut self.blocks.edit()[at]),
            None => (self.waiting.iter_mut()).rfind(|b| b.runs(call)),
        }
    }

    /// **A block of the reply's, placed in the order it happened** — a tool
    /// record, a thought — and its index. What the model wrote BEFORE it
    /// stays above it, and the reply's next words stream into a fresh block
    /// BELOW it (Steven, 2026-10-03): the record used to go in front of the
    /// open block, so a denial sat above the sentence that led to it, and
    /// the next round's text ran onto the last line of the one before: "in
    /// sequence.I'm unable to run that". Nothing written yet: it goes in
    /// front of the open block, the reply streaming below.
    ///
    /// **A typed record waiting goes in first** ([`waiting`](Self::waiting)):
    /// it came while the block this one ends was written, so before this.
    fn put_in_order(&mut self, block: Block) -> usize {
        let mut placed = std::mem::take(&mut self.waiting);
        placed.push(block);
        let last = placed.len() - 1;
        let blocks = self.blocks.edit();
        let len = blocks.len();
        match blocks.last_mut() {
            Some(open) if !open.sealed && !open.is_empty() => {
                let turn = open.turn;
                open.lines.push(String::new());
                open.sealed = true;
                open.invalidate();
                blocks.extend(placed);
                let mut next = Block::new(BlockRole::Assistant, false, vec![String::new()]);
                next.turn = turn;
                blocks.push(next);
                len + last
            }
            Some(open) if !open.sealed => {
                blocks.splice(len - 1..len - 1, placed);
                len - 1 + last
            }
            _ => {
                blocks.extend(placed);
                len + last
            }
        }
    }

    /// **Undo the most recent [`begin_turn`](Self::begin_turn)** — the
    /// transcript half of `Conversation::fail_turn`.
    ///
    /// A turn that never received a chunk and never finished a tool round is
    /// not sealed: `interrupt_turn` drops it, truncating the user's message
    /// back out of the conversation. The echo stayed on screen regardless, so
    /// the transcript showed `> write me a poem about ducks` with nothing under
    /// it, forever, while the model had never been told — and the *next* turn
    /// went to a model that had never seen it. Screen and record disagreeing
    /// about what was said is the worst version of the ambiguous-record trap,
    /// because the screen is the only copy the user can read (Reviewer F,
    /// reproduced).
    ///
    /// **Drops every trailing block belonging to that turn**, not "the last
    /// two". `begin_turn` pushes two (echo + open assistant) and
    /// `push_tool_record` can insert more — all
    /// of them tagged with the same turn number, which is exactly what makes
    /// the rule expressible. Counting instead would be a proxy, and a proxy
    /// that is right until a command runs.
    ///
    /// **Keeps the turn when it touched the machine, or says why it
    /// failed.** A `Tool` block is an account of a command that really ran —
    /// the single most important thing this transcript carries, and often
    /// the *only* surviving record of it, because a turn killed mid-command
    /// has no finished round and so `interrupt_turn` keeps nothing either.
    /// Two correct-sounding rules collide there ("screen must match the
    /// record" against "a machine action is always recorded") and the second
    /// one wins: the echo describes something the model never received,
    /// while the `⚙ cancelled: …` line describes something that happened to
    /// the user's computer. A failure's `[error: …]` (`App::say_failure`)
    /// keeps it too: the line says the request failed, under the words that
    /// asked, which the input does not hold again.
    ///
    /// **Kept, it is no turn's.** The conversation has no such turn, and a
    /// transcript numbering one the store does not put every mark set after
    /// it one turn late on reopen, `:reply` on the turn before it, and
    /// `turns_ago` one off what the model saw (review stop 14, A 2). So
    /// either way the counter goes back, and a kept turn is ended as a turn
    /// ends (`end_turn`), its lines left as they were.
    ///
    /// **And said so, painted apart** (Steven, 10-31: "making it clear in the
    /// conversation that this message was not sent and not included"): its
    /// last line is `[not in the conversation: {why}]`, and every block of
    /// it is [`dropped`](Span::dropped), drawn in `ui.danger` — your words,
    /// a record of what ran, and the error under them, which stays for you
    /// to diagnose. `why` is the ending's, said by the caller.
    ///
    /// So this removes for the case it was written for — `CTRL-C` before the
    /// first chunk, nothing run — and keeps the rest, numbered as nothing.
    /// Found by an existing test, which is the argument for having written it.
    /// A turn already ended (`end_turn`) is the conversation's, and is left
    /// as it is.
    ///
    /// Returns whether its blocks were removed, so a caller cannot report an
    /// undo that did not happen.
    pub fn abandon_turn(&mut self, why: &str) -> bool {
        let Some(turn) = self.turns.checked_sub(1) else {
            return false; // nothing has ever begun
        };
        let mine = |b: &Block| b.turn == Some(turn);
        // The counter goes back either way, or the next turn is numbered as
        // though this one had happened — and `turn_of_line` would then map
        // rows to a turn the conversation does not have. A typed record
        // waiting (`waiting`) is a command that ran too: kept, it goes in as
        // the turn ends; truncated, it was left waiting for a turn that is
        // gone, never drawn.
        if !self.waiting.is_empty()
            || (self.blocks.iter()).filter(|b| mine(b)).any(|b| {
                b.role == BlockRole::Tool || (b.role == BlockRole::Assistant && !b.is_empty())
            })
        {
            // A turn that ended (`end_turn`) is the conversation's: nothing
            // to undo. One the conversation dropped is still open — its
            // reply's block, or the round's after a record.
            let Some(open) = self.blocks.last().filter(|b| mine(b) && !b.sealed) else {
                return false;
            };
            // On the open block's next line, or its first when it is empty
            // (the round after a record that never came).
            let gap = if open.is_empty() { "" } else { "\n" };
            self.append_chunk(&format!("{gap}[not in the conversation: {why}]"));
            self.end_turn();
            for block in self.blocks.edit().iter_mut().filter(|b| mine(b)) {
                block.turn = None;
                block.dropped = true;
            }
            self.turns = turn;
            return false;
        }
        let keep = self
            .blocks
            .iter()
            .rposition(|b| !mine(b))
            .map_or(0, |i| i + 1);
        if keep == self.blocks.len() {
            return false; // the last turn left no blocks; nothing to undo
        }
        self.blocks.edit().truncate(keep);
        self.turns = turn;
        true
    }

    /// Whether the transcript is empty (nothing rendered yet).
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Total logical-line count across all blocks. Used by the status
    /// line as a rough size indicator; not a hot path.
    pub fn line_count(&self) -> usize {
        self.blocks.iter().map(|b| b.lines.len()).sum()
    }

    /// Flattened logical lines across all blocks, in render order.
    /// Allocates; intended for tests and diagnostics, not the hot path.
    pub fn lines(&self) -> Vec<String> {
        self.blocks
            .iter()
            .flat_map(|b| b.lines.iter().cloned())
            .collect()
    }

    /// **What a copy of these lines, taken at `synced`, lacks** — how many
    /// of its first lines are still right (each block's, from the first, that
    /// has not changed since: the same version is the same content), and the
    /// lines after them; `synced` then describes the copy brought up to date.
    /// Compares versions, O(blocks), and copies only what changed: while a
    /// reply streams that is its own block. The transcript's mirror was
    /// copied whole on every streamed batch — 4.2 ms at 20.8k lines, the
    /// frame rate capped near 240 by the copy alone (buffers review, B F5).
    pub fn lines_since(&self, synced: &mut Synced) -> (usize, Vec<String>) {
        let same = self
            .blocks
            .iter()
            .zip(&synced.blocks)
            .take_while(|(block, (version, _))| block.version == *version)
            .count();
        synced.blocks.truncate(same);
        let keep = synced.lines();
        let changed = &self.blocks[same..];
        synced
            .blocks
            .extend(changed.iter().map(|b| (b.version, b.lines.len())));
        let tail = changed
            .iter()
            .flat_map(|b| b.lines.iter().cloned())
            .collect();
        (keep, tail)
    }

    /// The logical line a window's `origins` name, borrowed — O(1), because a
    /// renderer asks once per distinct visible line, every frame.
    pub fn line_at(&self, (block, line): LineAt) -> Option<&str> {
        self.blocks.get(block)?.lines.get(line).map(String::as_str)
    }

    /// **Where in the logical line at `at` a held secret was put back from
    /// its handle** — char ranges — when the model wrote that line (its
    /// reply, its thinking: [`BlockRole::the_models`]); `None` when it did
    /// not (your echo, a tool's record), where a held value says nothing
    /// about what the model had (review stop 6, F 5).
    pub fn shown_at(&self, (block, line): LineAt) -> Option<Vec<(usize, usize)>> {
        let block = self.blocks.get(block)?;
        block.role.the_models().then(|| {
            block
                .shown
                .iter()
                .filter(|&&(l, _, _)| l == line)
                .map(|&(_, from, to)| (from, to))
                .collect()
        })
    }

    /// **The title block `block` draws shut**, when it has one — a command
    /// record's (`tool_record::Record::title`), whose row is a prefix of it
    /// ([`FoldRow::titled`]).
    pub fn title_of(&self, block: usize) -> Option<&str> {
        let (title, _) = self.blocks.get(block)?.title.as_ref()?;
        Some(title)
    }

    /// **Whether a line the fold of `block` stands for holds what `holds`
    /// finds** — `holds(text, put_back)`, `put_back` as
    /// [`shown_at`](Self::shown_at) gives it; only the model's text is
    /// asked, as only it has a `put_back`. Asked of a shut row by the view
    /// (`view::hides_a_leak`), every frame: remembered per change to the
    /// block's lines and to `key`, what `holds` reads (the secret table's
    /// `fingerprint`), and walked once, the put-backs read by line — per
    /// line through `shown_at`, a fold cost its lines times its put-backs.
    pub fn fold_holds(
        &self,
        block: usize,
        key: u64,
        holds: impl Fn(&str, &[(usize, usize)]) -> bool,
    ) -> bool {
        let Some(block) = self.blocks.get(block).filter(|b| b.role.the_models()) else {
            return false;
        };
        if let Some((version, asked, found)) = block.held.get()
            && (version, asked) == (block.version, key)
        {
            return found;
        }
        let lines = block.content();
        let mut put_back = vec![Vec::new(); lines];
        for &(line, from, to) in &block.shown {
            if let Some(at) = put_back.get_mut(line) {
                at.push((from, to));
            }
        }
        let found = (0..lines).any(|line| holds(&block.lines[line], &put_back[line]));
        block.held.set(Some((block.version, key, found)));
        found
    }

    /// Logical lines from the LAST one back, borrowed — for a search that
    /// wants what was said most recently first and can stop early
    /// (`:secret`'s completion), without copying the transcript.
    pub fn lines_from_end(&self) -> impl Iterator<Item = &str> {
        self.blocks
            .iter()
            .rev()
            .flat_map(|b| b.lines.iter().rev().map(String::as_str))
    }

    pub fn is_following_tail(vp: &Viewport) -> bool {
        vp.is_following_tail()
    }

    /// **The row at a window's top, and the rows there are** — the one
    /// reading of a [`Viewport`]'s top: the last page while it follows the
    /// end, else the row holding its text ([`logical_to_visual`](Self::logical_to_visual))
    /// at this width and in these folds, clamped to the last page — where a
    /// top whose lines are gone lands, and one a wider window has no room
    /// for. A line inside a shut fold is on the fold's row.
    pub fn top_row(
        &self,
        vp: &Viewport,
        shut: &Shut<'_>,
        width: u16,
        height: usize,
    ) -> (usize, usize) {
        let total = self.total_visual_rows(shut, width);
        let last_page = total.saturating_sub(height);
        let top = match vp.top() {
            None => last_page,
            Some((line, col)) => self.logical_to_visual(shut, line, col, width).0,
        };
        (top.min(last_page), total)
    }

    /// **A window's top moved from row `from` to row `to`** — the one door
    /// for placing a view by row. At or past the last page it follows the
    /// end: being at the end and following it are one thing by any route
    /// (`G` once froze there, and new replies were not shown —
    /// `[[feedback_one_door_per_invariant]]`). Short of it, frozen on the
    /// text at `to`: its line and the column its row starts at.
    ///
    /// **A top that did not move keeps its text.** Its column may sit
    /// part-way into a row at this width (it was found at another), and
    /// read back from the row it would slide to the row's start: a window
    /// moved between two widths, a `j` at each, crept up a row each time.
    fn move_top(
        &self,
        vp: &mut Viewport,
        (shut, width): (&Shut<'_>, u16),
        (from, to): (usize, usize),
        last_page: usize,
    ) {
        if to >= last_page {
            vp.snap_to_tail();
        } else if to != from {
            let (line, col) = self.visual_to_logical(shut, to, 0, width);
            vp.freeze_at(line, col);
        }
    }

    /// Move the viewport the *minimum* needed to keep the cursor inside a soft
    /// margin — vim's `scrolloff`.
    ///
    /// The cursor pushes the view rather than the view dragging the cursor: near
    /// the top, moving up scrolls up; near the bottom, moving down scrolls down;
    /// in the middle, nothing moves. `scrolloff` is the soft ceiling/floor, and
    /// is clamped to half the window so a large value degrades to keep-centered
    /// (vim's `so=999` idiom) instead of fighting itself.
    #[allow(clippy::too_many_arguments)] // the window's two halves, then a place
    pub fn follow_cursor(
        &self,
        vp: &mut Viewport,
        shut: &Shut<'_>,
        width: u16,
        height: u16,
        cursor_line: usize,
        cursor_col: usize,
        scrolloff: usize,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let h = height as usize;
        let (cursor_abs, _) = self.logical_to_visual(shut, cursor_line, cursor_col, width);
        let (top, total) = self.top_row(vp, shut, width, h);
        // The rule itself lives in `viewport::follow` — it was written here and
        // again in `GraphView::window_top`, identically and independently, and
        // a third reader (`TextView`) had none at all.
        let to = crate::viewport::follow(cursor_abs, top, total, h, scrolloff);
        self.move_top(vp, (shut, width), (top, to), total.saturating_sub(h));
    }

    /// **Where the cursor at `cursor` goes now that the window moved** — the
    /// other direction from [`follow_cursor`](Self::follow_cursor), and the
    /// same band ([`viewport::drag`](crate::viewport::drag)): `None` when it
    /// is still inside, else the logical position on the nearest row that is,
    /// in the cursor's own cell (vim keeps the column it wants).
    ///
    /// In ROWS, as `follow_cursor` is: a reply is one logical line over many
    /// rows, so a rule in lines would leave a cursor on a tall line's first
    /// row off screen while its last rows were on it.
    pub fn dragged(
        &self,
        vp: &Viewport,
        shut: &Shut<'_>,
        width: u16,
        height: u16,
        cursor: (usize, usize),
        scrolloff: usize,
    ) -> Option<(usize, usize)> {
        if width == 0 || height == 0 {
            return None;
        }
        let h = height as usize;
        let (top, total) = self.top_row(vp, shut, width, h);
        let (row, cell) = self.logical_to_visual(shut, cursor.0, cursor.1, width);
        let to = crate::viewport::drag(row, top, top + h - 1, total, scrolloff);
        (to != row).then(|| self.visual_to_logical(shut, to, cell, width))
    }

    /// `H` / `M` / `L` — **the logical line** at that place on screen, by the
    /// rules every pane shares: which lines are on screen
    /// ([`viewport::lines_on_screen`](crate::viewport::lines_on_screen)) and
    /// which of them is meant
    /// ([`viewport::screen_line`](crate::viewport::screen_line)). `None` at
    /// zero size.
    pub fn screen_line(
        &self,
        vp: &Viewport,
        shut: &Shut<'_>,
        width: u16,
        height: u16,
        which: crate::viewport::ScreenLine,
        scrolloff: usize,
    ) -> Option<usize> {
        if width == 0 || height == 0 {
            return None;
        }
        // **A shut fold is one line on screen**, as vim counts a closed fold
        // (Neovim 0.12: `M` on the line after a 31-line fold): the lines it
        // hides have no rows, and counted as lines `M` landed among them,
        // inside it (review stop 15, B M1). Counted over the lines with rows,
        // then mapped back.
        let all = self.line_heights(shut, width);
        let shown: Vec<usize> = (0..all.len()).filter(|&l| all[l] > 0).collect();
        let heights: Vec<usize> = shown.iter().map(|&l| all[l]).collect();
        let h = height as usize;
        let total: usize = heights.iter().sum();
        let (top, _) = self.top_row(vp, shut, width, h);
        let (first, last) = crate::viewport::lines_on_screen(heights.iter().copied(), top, h)?;
        let line =
            crate::viewport::screen_line(which, first, last, heights.len(), scrolloff, false);
        // **`H`/`M`/`L` never move the window** (vim's do not), so the line's
        // first ROW must be in the band a scroll keeps the cursor in
        // (`viewport::drag`). `screen_line` counts `scrolloff` in LINES, and
        // with replies many rows tall it chose a line whose first row was
        // inside the margin, which following then scrolled — `H` moved the
        // window up to four rows (review of c816165, Reviewer C). So take the
        // nearest line on screen, toward the band, whose first row is in it;
        // a line taller than the band has none, and keeps the line's choice.
        let starts: Vec<usize> = heights
            .iter()
            .scan(0, |row, rows| {
                let start = *row;
                *row += rows;
                Some(start)
            })
            .collect();
        let held = |l: usize| crate::viewport::drag(starts[l], top, top + h - 1, total, scrolloff);
        if held(line) == starts[line] {
            return Some(shown[line]);
        }
        let nearest = if held(line) > starts[line] {
            (line + 1..=last).find(|&l| held(l) == starts[l])
        } else {
            (first..line).rev().find(|&l| held(l) == starts[l])
        };
        Some(shown[nearest.unwrap_or(line)])
    }

    /// `zt` / `zz` / `zb` — put logical line `line` at the top, middle or
    /// bottom of the window, by [`viewport::place_top`](crate::viewport::place_top).
    /// Past the last page it shows the last page: this viewport clamps its top
    /// there on every read, where a text page may hold a `zt` past it.
    ///
    /// A line inside a shut fold is placed as the fold's row: no row is its
    /// own (its height is none), so it goes back to the line that has one,
    /// the fold's first — summed as it was, `zt` there put the row after the
    /// fold at the top and hid the fold.
    #[allow(clippy::too_many_arguments)] // the window's two halves, then a place
    pub fn place(
        &self,
        vp: &mut Viewport,
        shut: &Shut<'_>,
        width: u16,
        height: u16,
        line: usize,
        at: crate::viewport::Place,
        scrolloff: usize,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let heights = self.line_heights(shut, width);
        let line = line.min(heights.len().saturating_sub(1));
        let line = (0..=line).rev().find(|&l| heights[l] > 0).unwrap_or(line);
        let row: usize = heights[..line].iter().sum();
        let rows = heights.get(line).copied().unwrap_or(1);
        let h = height as usize;
        let to = crate::viewport::place_top(at, row, rows, h, scrolloff);
        let (from, total) = self.top_row(vp, shut, width, h);
        self.move_top(vp, (shut, width), (from, to), total.saturating_sub(h));
    }

    /// The rows each logical line takes at `width`, in [`lines`](Self::lines)
    /// order — from the per-block wrap caches, so a keypress pays a sum, not a
    /// re-wrap. In a shut block, its first line one and the rest of its
    /// content none: every scroll and follow reads these.
    fn line_heights(&self, shut: &Shut<'_>, width: u16) -> Vec<usize> {
        self.blocks
            .iter()
            .flat_map(|b| b.shape(width, &self.ops, shut).line_rows.clone())
            .collect()
    }

    /// Scroll up (toward older content) by `delta` visual rows. It stops
    /// following the end **only if the view moves** — PgUp on content that
    /// fits, or at the top, must not flip the tail marker (`move_top`: a top
    /// that did not move is left as it was).
    pub fn scroll_up(
        &self,
        vp: &mut Viewport,
        shut: &Shut<'_>,
        delta: usize,
        viewport_width: u16,
        viewport_height: u16,
    ) {
        let h = viewport_height as usize;
        let (top, total) = self.top_row(vp, shut, viewport_width, h);
        let to = top.saturating_sub(delta);
        self.move_top(
            vp,
            (shut, viewport_width),
            (top, to),
            total.saturating_sub(h),
        );
    }

    /// Scroll down (toward newer content) by `delta` visual rows.
    /// Re-engages follow-tail on reaching the bottom.
    pub fn scroll_down(
        &self,
        vp: &mut Viewport,
        shut: &Shut<'_>,
        delta: usize,
        viewport_width: u16,
        viewport_height: u16,
    ) {
        let h = viewport_height as usize;
        let (top, total) = self.top_row(vp, shut, viewport_width, h);
        let last_page = total.saturating_sub(h);
        let to = top.saturating_add(delta).min(last_page);
        self.move_top(vp, (shut, viewport_width), (top, to), last_page);
    }

    /// Total visual rows at `width` in a window's shapes, summed from the
    /// (cached) per-block wrap. Reuses each block's cache, so this is cheap
    /// after the first wrap at a given width.
    pub fn total_visual_rows(&self, shut: &Shut<'_>, width: u16) -> usize {
        if width == 0 {
            return 0;
        }
        self.blocks
            .iter()
            .map(|b| b.row_count(width, &self.ops, shut))
            .sum()
    }

    /// Compute the visible window for the given viewport size. Slices the
    /// per-block wrap caches; re-wraps only blocks whose width changed or
    /// whose content was appended since the last call. Skips whole blocks
    /// that fall entirely above the viewport, so the work is
    /// O(blocks + height), not O(total rows).
    pub fn visible_window(
        &self,
        vp: &Viewport,
        shut: &Shut<'_>,
        width: u16,
        height: u16,
    ) -> VisibleWindow {
        if width == 0 || height == 0 {
            return VisibleWindow::default();
        }
        let height = height as usize;

        let (top, total_rows) = self.top_row(vp, shut, width, height);

        let mut rows: Vec<String> = Vec::with_capacity(height);
        let mut styles: Vec<Vec<CellStyle>> = Vec::with_capacity(height);
        let mut roles: Vec<BlockRole> = Vec::with_capacity(height);
        let mut dropped: Vec<bool> = Vec::with_capacity(height);
        let mut folds: Vec<Option<FoldRow>> = Vec::with_capacity(height);
        let mut origins: Vec<(LineAt, usize)> = Vec::with_capacity(height);
        let mut row_index = 0usize; // running visual-row index across blocks
        for (b, block) in self.blocks.iter().enumerate() {
            let cache = block.shape(width, &self.ops, shut);
            let block_len = cache.rows.len();

            // Skip blocks entirely above the visible window.
            if row_index + block_len <= top {
                row_index += block_len;
                continue;
            }
            // Jump straight to the first visible row *within* this block
            // (`top - row_index` for the first partially-visible block; 0
            // for every block after it). Slicing rather than scanning from
            // row 0 keeps the whole operation O(viewport), not O(history)
            // — the difference between flat and history-linear framing
            // when one block holds a long response.
            let start = top.saturating_sub(row_index);
            for i in start..block_len {
                rows.push(cache.rows[i].clone());
                styles.push(cache.styles[i].clone());
                roles.push(block.role);
                dropped.push(block.dropped);
                folds.push(cache.fold.filter(|_| i == 0));
                origins.push(((b, cache.row_line[i]), cache.row_offset[i]));
                if rows.len() == height {
                    break;
                }
            }
            if rows.len() == height {
                break;
            }
            row_index += block_len;
        }
        VisibleWindow {
            rows,
            styles,
            roles,
            dropped,
            folds,
            total_rows,
            top,
            origins,
        }
    }

    /// Map a logical position — `flat_line` indexing [`lines`](Self::lines),
    /// `col` a char index into it — to its **absolute** visual `(row, cell)` over
    /// the word-wrapped transcript at `width`. The chat overlay (P3) uses this so
    /// its caret lands on the very rows Output renders — one geometry source, no
    /// second wrap model to drift against. Reuses the per-block + per-line cache
    /// and re-wraps only the cursor's own line.
    ///
    /// `cell` is always on a char boundary. `row` can equal `total_rows` (the
    /// phantom row past a line that exactly fills `width`, i.e. the caret rests at
    /// the next line's start); the caller clamps into its window. Out-of-range
    /// `flat_line` clamps to the last row.
    ///
    /// A place inside a shut fold is on its one row, at its start — where a
    /// window draws the cursor on a fold, as vim's does.
    pub fn logical_to_visual(
        &self,
        shut: &Shut<'_>,
        flat_line: usize,
        col: usize,
        width: u16,
    ) -> (usize, usize) {
        if width == 0 {
            return (0, 0);
        }
        let mut line_cursor = 0usize; // running flat logical-line index
        let mut row_cursor = 0usize; // running absolute visual row
        for block in &self.blocks {
            let cache = block.shape(width, &self.ops, shut);
            let n = block.lines.len();
            if flat_line < line_cursor + n {
                let local = flat_line - line_cursor;
                if cache.fold.is_some_and(|fold| local < fold.lines) {
                    return (row_cursor, 0);
                }
                let before: usize = cache.line_rows[..local].iter().sum();
                let chars: Vec<char> = block.lines[local].chars().collect();
                let rows = crate::wrap::wrap_cells_word(&chars, width as usize);
                let (r, cell) = crate::wrap::cursor_in_wrapped_word(
                    &chars,
                    &rows,
                    width as usize,
                    col.min(chars.len()),
                );
                return (row_cursor + before + r, cell);
            }
            line_cursor += n;
            row_cursor += cache.rows.len();
        }
        (row_cursor.saturating_sub(1), 0)
    }

    /// Inverse of [`logical_to_visual`](Self::logical_to_visual): an absolute visual `(row, cell)` back to
    /// a logical `(flat_line, col)`. Drives mouse hit-testing in the focused
    /// chat. Clamps past-the-end to the last line's end.
    /// A shut fold's row is its first line's start.
    pub fn visual_to_logical(
        &self,
        shut: &Shut<'_>,
        row: usize,
        cell: usize,
        width: u16,
    ) -> (usize, usize) {
        if width == 0 {
            return (0, 0);
        }
        let mut line_cursor = 0usize;
        let mut row_cursor = 0usize;
        for block in &self.blocks {
            let cache = block.shape(width, &self.ops, shut);
            let block_rows = cache.rows.len();
            if row == row_cursor && cache.fold.is_some() {
                return (line_cursor, 0);
            }
            if row < row_cursor + block_rows {
                let mut local_visual = row - row_cursor;
                for (local_line, &lr) in cache.line_rows.iter().enumerate() {
                    if local_visual < lr {
                        let chars: Vec<char> = block.lines[local_line].chars().collect();
                        let rows = crate::wrap::wrap_cells_word(&chars, width as usize);
                        let col = crate::wrap::col_from_visual_word(&rows, local_visual, cell);
                        return (line_cursor + local_line, col);
                    }
                    local_visual -= lr;
                }
                break; // unreachable: line_rows sums to block_rows
            }
            line_cursor += block.lines.len();
            row_cursor += block_rows;
        }
        // Past the last row: clamp to the end of the last logical line.
        let last_col = self
            .blocks
            .last()
            .and_then(|b| b.lines.last())
            .map(|l| l.chars().count())
            .unwrap_or(0);
        (line_cursor.saturating_sub(1), last_col)
    }

    /// The visible window for the focused chat: styled word-wrapped rows (from the
    /// cache — no re-styling), a per-row selection highlight, and the cursor.
    /// Frames around the cursor (cursor-follow), so navigation scrolls purely by
    /// moving the cursor — matching the chat's existing behavior — and reuses the
    /// O(viewport) per-block cache (the chat's own render was O(history)).
    ///
    /// `selected(line, len)` returns the selected char range on a logical line
    /// (the chat `Buffer` owns the Visual-mode shape — char/line/block — so this
    /// projection stays kind-agnostic). Each row's range is clipped to the row, as
    /// char offsets into that row's visible text.
    #[allow(clippy::too_many_arguments)] // the window's two halves, then a place
    pub fn focused_window(
        &self,
        vp: &Viewport,
        shut: &Shut<'_>,
        width: u16,
        height: u16,
        cursor_line: usize,
        cursor_col: usize,
        selected: impl Fn(usize, usize) -> Option<(usize, usize)>,
    ) -> FocusedWindow {
        if width == 0 || height == 0 {
            return FocusedWindow::default();
        }
        let h = height as usize;
        let (cursor_abs, cursor_cell) =
            self.logical_to_visual(shut, cursor_line, cursor_col, width);
        // The viewport is RETAINED, not derived from the cursor. Deriving it
        // (`cursor_abs - (h-1)`) pinned the cursor to the last row forever: you
        // could scroll the text but never move off the bottom line. The cursor
        // moves inside a stable window and *pushes* it only at the soft edges —
        // see `follow_cursor`, which the app calls after a motion.
        let (top, _) = self.top_row(vp, shut, width, h);

        let mut rows = Vec::with_capacity(h);
        let mut styles = Vec::with_capacity(h);
        let mut roles = Vec::with_capacity(h);
        let mut dropped = Vec::with_capacity(h);
        let mut folds = Vec::with_capacity(h);
        let mut selection = Vec::with_capacity(h);
        let mut origins = Vec::with_capacity(h);

        let mut abs = 0usize; // running absolute visual row
        let mut flat_line = 0usize; // running flat logical-line index
        'blocks: for (b, block) in self.blocks.iter().enumerate() {
            let cache = block.shape(width, &self.ops, shut);
            let block_rows = cache.rows.len();
            if abs + block_rows <= top {
                abs += block_rows; // whole block above the window
                flat_line += block.lines.len();
                continue;
            }
            // From the first VISIBLE row, each row's line read off the wrap
            // cache — not walked from the block's first row, which made the
            // focused chat over a long reply O(rows above it) per frame
            // (Reviewer C). And each line's length counted once, not once per
            // row: one long line fills the window, and counting it per row was
            // 17 ms a frame on a 3 MB line (Reviewer B, measured).
            let mut line_len: Option<(usize, usize)> = None;
            for i in top.saturating_sub(abs)..block_rows {
                let local_line = cache.row_line[i];
                let row = &cache.rows[i];
                let cstart = cache.row_offset[i];
                let vis_len = row.chars().count();
                let len = match line_len {
                    Some((at, len)) if at == local_line => len,
                    _ => {
                        let len = block.lines[local_line].chars().count();
                        line_len = Some((local_line, len));
                        len
                    }
                };
                let fold = cache.fold.filter(|_| i == 0);
                let sel = match fold.filter(|fold| fold.titled) {
                    // A title is no line's characters: its row is selected
                    // whole when its first line is — what an operator then
                    // takes whole, `V` on it the whole record — and not at
                    // all when part of it is (vim lights a closed fold whole
                    // or not at all, `fold_line`).
                    Some(_) => (selected(flat_line + local_line, len) == Some((0, len)))
                        .then_some((0, vis_len)),
                    None => selected(flat_line + local_line, len).and_then(|(slo, shi)| {
                        // Clip the line's selected char range to this row, as
                        // char offsets within the row's visible text (the
                        // renderer slices chars to reverse-video them, like
                        // `selected_line`).
                        let lo = slo.max(cstart);
                        let hi = shi.min(cstart + vis_len);
                        // `then` (lazy), not `then_some`: a row past the
                        // selection has `hi <= lo`, and `lo - cstart` is only
                        // safe to build when `lo < hi` (where `lo >= cstart`).
                        (lo < hi).then(|| (lo - cstart, hi - cstart))
                    }),
                };
                rows.push(row.clone());
                styles.push(cache.styles[i].clone());
                roles.push(block.role);
                dropped.push(block.dropped);
                folds.push(fold);
                selection.push(sel);
                origins.push(((b, local_line), cstart));
                if rows.len() == h {
                    break 'blocks;
                }
            }
            abs += block_rows;
            flat_line += block.lines.len();
        }

        let cursor =
            (cursor_abs >= top && cursor_abs < top + h).then(|| (cursor_abs - top, cursor_cell));
        FocusedWindow {
            rows,
            styles,
            roles,
            dropped,
            folds,
            selection,
            cursor,
            origins,
        }
    }

    /// Map a click at window-relative `(vrow, vcol)` in the focused chat back to
    /// a logical `(flat_line, col)`. Drives mouse positioning and drag-select.
    ///
    /// **Takes the `Viewport`, and asks it the same question the renderer asks.**
    /// This computed `top` as `cursor_abs - (height - 1)` under a doc comment
    /// claiming it "uses the same cursor-derived top as `focused_window`" —
    /// which `focused_window` had stopped doing, in a change whose own comment
    /// explains why deriving the top is wrong (it pins the cursor to the last
    /// row). So the moment a pane was scrolled back, a click landed on a
    /// different line than the one under the pointer: 16 lines away in the
    /// measured case, and a drag then reverse-videos a span you did not select
    /// and `y` copies it (Reviewer A, round 3).
    ///
    /// One rule, one caller of it. `Buffer::cursor_from_visual` and
    /// `Buffer::visible_window` already share their top this way; the chat pair
    /// was the one that did not.
    /// `size` is the pane's content `(width, height)`, `cursor` the pane's
    /// logical cursor (returned unchanged for a zero-sized pane) and `at` the
    /// window-relative `(row, col)` clicked. Grouped as pairs because eight
    /// loose arguments is where they stop being readable — and every one of
    /// them is half of something.
    pub fn chat_click(
        &self,
        vp: &crate::viewport::Viewport,
        shut: &Shut<'_>,
        size: (u16, u16),
        cursor: (usize, usize),
        at: (usize, usize),
    ) -> (usize, usize) {
        let (width, height) = size;
        if width == 0 || height == 0 {
            return cursor;
        }
        let (top, _) = self.top_row(vp, shut, width, height as usize);
        self.visual_to_logical(shut, top + at.0, at.1, width)
    }

    /// Number of actual wrap recomputations performed so far. Cache
    /// instrumentation: a redraw at an unchanged width must not increase
    /// it. Exercised by the cache-reuse tests.
    #[cfg(test)]
    fn wrap_ops(&self) -> usize {
        self.ops.wraps.get()
    }

    /// Number of reply stylings so far — markdown and syntax highlighting,
    /// which no width change may redo.
    #[cfg(test)]
    fn style_ops(&self) -> usize {
        self.ops.styles.get()
    }
}

/// Strip control characters from one line before it enters the buffer.
/// Output text — especially streamed model output, which can be
/// prompt-injected — would otherwise reach the terminal verbatim through
/// ratatui's cells: an embedded `ESC[…` / OSC sequence could move the
/// cursor, repaint the screen, or set the window title (terminal
/// injection). We render to a TUI, so we own this boundary.
///
/// `'\n'` never reaches here (callers split on it first). Tabs map to a
/// single space so they don't misalign against the terminal's tab stops;
/// every other C0/C1 control and DEL is dropped. Printable text,
/// including multi-byte UTF-8, passes through. Runs once at ingest, not
/// per frame — here, for a leftover's last line in `:ls`, and for a
/// session's first prompt there (`SessionBuffer::prompts`, memoised).
/// (`:alias` runs it per frame on each value: a handful of short strings.)
pub(crate) fn sanitize_line(line: &str) -> String {
    line.chars()
        .filter_map(|c| match c {
            '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

/// Sanitize a possibly-multi-line string into per-line entries. Uses
/// `str::lines`, which already gives exactly the semantics the user echo
/// wants — no phantom trailing blank, and `\r\n` collapsed to one break
/// (any stray `\r` would be dropped by `sanitize_line` anyway). An empty
/// input still yields one (empty) line so it renders as a blank row.
pub(crate) fn sanitize_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(sanitize_line).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Build a sealed assistant block of `n` short lines, for scroll /
    /// wrap tests that just need bulk content.
    fn filled(n: usize) -> Output {
        let mut o = Output::new();
        o.begin_turn("seed");
        for i in 0..n {
            o.append_chunk(&format!("line {i}\n"));
        }
        o.end_turn();
        o
    }

    /// **Every change moves the revision, and a read moves nothing** — each
    /// public door, and the reads a frame makes. That every writer moves it
    /// is the type's (`blocks::Blocks`: its list is that module's, and
    /// `edit` moves it first); this walks the doors and checks the counter
    /// moves at all. A change that did not move it would let a reply land in
    /// a session no pane shows with nothing saying so (`unshown`). And none
    /// moves it to `EMPTY`, a new transcript's alone: a session that arrived
    /// is seen as it, so a change landing there would read as nothing to
    /// read.
    #[test]
    fn every_change_moves_the_revision() {
        let mut o = Output::new();
        assert_eq!(o.revision(), Output::EMPTY, "nothing written: EMPTY");
        let mut last = o.revision();
        let mut moved = |o: &Output, door: &str| {
            assert_ne!(o.revision(), last, "{door} moved nothing");
            assert_ne!(o.revision(), Output::EMPTY, "{door} moved it to EMPTY");
            last = o.revision();
        };
        o.told(&[shannon_core::Block::Context {
            kind: shannon_core::ContextKind::Instructions,
            source: "/p/AGENTS.md".into(),
            text: "be brief".into(),
        }]);
        moved(&o, "told");
        o.begin_turn("hello");
        moved(&o, "begin_turn");
        o.append_chunk("a reply");
        moved(&o, "append_chunk");
        o.push_tool_activity("$ ls", "a.txt");
        moved(&o, "push_tool_record");
        o.end_turn();
        moved(&o, "end_turn");
        o.begin_turn("again");
        moved(&o, "begin_turn");
        assert!(
            o.abandon_turn("you stopped it"),
            "premise: an empty turn undone"
        );
        moved(&o, "abandon_turn");
        // What a frame reads leaves it.
        let before = o.revision();
        let _ = (o.lines(), o.line_count(), o.sections(), o.turn_of_line(0));
        let _ = (o.spans().count(), o.anchor_at(0, 0).map(|a| o.locate(a)));
        assert_eq!(o.revision(), before, "a read is no change");
        // And nothing undone is no change.
        assert!(
            !o.abandon_turn("you stopped it"),
            "premise: nothing to undo"
        );
        assert_eq!(o.revision(), before, "an undo of nothing");
    }

    #[test]
    fn new_output_is_empty_and_follows_tail() {
        let vp = Viewport::default();
        let o = Output::new();
        assert!(Output::is_following_tail(&vp));
        assert!(o.is_empty());
        assert_eq!(o.line_count(), 0);
    }

    /// **The line a turn's reply begins on**, when the turn is the first and
    /// its echo one row: your side takes a rule, the echo, a rule and a blank
    /// line (`begin_turn`). What a geometry test that seeds its content with
    /// a turn counts from.
    const REPLY: usize = 4;

    /// Each block's role, in order — the transcript's grammar, read.
    fn roles(o: &Output) -> Vec<BlockRole> {
        o.blocks.iter().map(|b| b.role).collect()
    }

    /// **Your message is framed** (Steven, 10-15: "a boundary around the user
    /// message, above and below", and "one newline after"): a rule, the
    /// echo, a rule, a blank line, then the reply streams below.
    #[test]
    fn begin_turn_echoes_user_and_opens_streaming_target() {
        let mut o = Output::new();
        o.begin_turn("hello");
        assert_eq!(o.lines(), ["", "> hello", "", "", ""]);
        use BlockRole::{Assistant, Rule, User};
        assert_eq!(roles(&o), [Rule, User, Rule, User, Assistant]);
        o.append_chunk("A");
        o.append_chunk("ssistant");
        assert_eq!(o.lines(), ["", "> hello", "", "", "Assistant"]);
        assert_eq!(o.lines()[REPLY], "Assistant");
    }

    /// **What a turn was told sits inside its frame**, above the echo — one
    /// rule opens the side, not one per block.
    #[test]
    fn a_turns_context_line_is_inside_its_frame() {
        let mut o = Output::new();
        o.told(&[shannon_core::Block::Context {
            kind: shannon_core::ContextKind::Memory,
            source: "/p/memory/MEMORY.md".to_string(),
            text: "remember this".to_string(),
        }]);
        o.begin_turn("hello");
        use BlockRole::{Assistant, Rule, User};
        assert_eq!(roles(&o), [Rule, User, User, Rule, User, Assistant]);
        assert_eq!(o.lines()[2], "> hello");
        assert!(o.blocks.iter().all(|b| b.turn == Some(0)), "one turn's");
    }

    /// **Each turn is a section from its first line** — the rule above your
    /// message, or above what the turn was told — so `]]` lands on it and
    /// `y]]` from it takes the whole turn (`Buffer`'s `Sections::At`); it
    /// ends at its last line of text, where `][` lands.
    #[test]
    fn each_turn_begins_a_section_at_its_rule() {
        let mut o = Output::new();
        o.begin_turn("one");
        o.append_chunk("reply");
        o.end_turn();
        o.told(&[shannon_core::Block::Context {
            kind: shannon_core::ContextKind::Memory,
            source: "/p/memory/MEMORY.md".to_string(),
            text: "remember this".to_string(),
        }]);
        o.begin_turn("two");
        let (starts, ends) = o.sections();
        assert_eq!(starts, [0, REPLY + 2], "after `reply` and its blank line");
        let lines = o.lines();
        assert_eq!(lines[starts[1]], "", "the rule");
        assert_eq!(lines[starts[1] + 2], "> two", "below the context line");
        assert_eq!(o.turn_of_line(starts[1]), Some(1), "the rule is the turn's");
        assert_eq!(
            ends,
            [REPLY, starts[1] + 2],
            "the reply; the echo, unanswered"
        );
        assert_eq!(lines[ends[0]], "reply");
    }

    #[test]
    fn end_turn_appends_separator_and_next_turn_lands_clean() {
        let mut o = Output::new();
        o.begin_turn("hi");
        o.append_chunk("reply");
        o.end_turn();
        assert_eq!(o.lines(), ["", "> hi", "", "", "reply", ""]);
        o.begin_turn("again");
        assert_eq!(
            o.lines(),
            ["", "> hi", "", "", "reply", "", "", "> again", "", "", ""],
        );
        assert_eq!(o.blocks[5].role, BlockRole::Rule, "the next turn's rule");
    }

    /// **A turn that ends on a tool record ends with ONE blank row**, as any
    /// turn does: the record carries its own separator, and the block opened
    /// below it for a next round that never came added a second (and
    /// `end_turn` a third) — review 20, Reviewer A.
    #[test]
    fn a_turn_ending_on_a_tool_record_ends_with_one_blank_row() {
        let mut o = Output::new();
        o.begin_turn("hi");
        o.append_chunk("running it");
        o.push_tool_activity("$ ls", "a.txt");
        o.end_turn();
        assert_eq!(
            o.lines(),
            [
                "",
                "> hi",
                "",
                "",
                "running it",
                "",
                "$ ls",
                "  ```output",
                "  a.txt",
                "  ```",
                "",
            ],
        );
        o.begin_turn("again");
        assert_eq!(
            o.lines()[11..13],
            ["", "> again"],
            "the next turn lands clean"
        );
    }

    #[test]
    fn append_chunk_handles_embedded_and_trailing_newlines() {
        let mut o = Output::new();
        o.begin_turn("x");
        o.append_chunk("He");
        o.append_chunk("llo\nwor");
        o.append_chunk("ld\n");
        assert_eq!(o.lines()[REPLY..], ["Hello", "world", ""]);
    }

    /// **A turn reads in the order it happened** (Steven's demos,
    /// 2026-10-03): the sentence before a tool call, the call's record, then
    /// the next round's text on a line of its own. The record went in FRONT
    /// of the open block, so a denial sat above the sentence that asked for
    /// it, and two rounds ran together: "in sequence.I'm unable to run that".
    /// A record before any text still goes first, with the reply below it;
    /// and every block stays the turn's, so an undo still finds them all.
    #[test]
    fn a_turn_reads_in_the_order_it_happened() {
        let mut o = Output::new();
        o.begin_turn("run it");
        o.append_chunk("I'll run it.");
        o.push_tool_activity("⚙ DENIED — nothing ran: gh api user", "");
        o.append_chunk("I can't run that.");
        o.push_tool_activity("⚙ output of: ls", "a.txt");
        o.append_chunk("Done.");
        o.end_turn();
        assert_eq!(
            o.lines(),
            [
                "",
                "> run it",
                "",
                "",
                "I'll run it.",
                "",
                "⚙ DENIED — nothing ran: gh api user",
                "",
                "I can't run that.",
                "",
                "⚙ output of: ls",
                "  ```output",
                "  a.txt",
                "  ```",
                "",
                "Done.",
                "",
            ]
        );
        assert!(
            o.blocks.iter().all(|b| b.turn == Some(0)),
            "one turn's blocks"
        );

        let mut o = Output::new();
        o.begin_turn("run it");
        o.push_tool_activity("⚙ output of: ls", "");
        o.append_chunk("Here it is.");
        o.end_turn();
        // The record lands below the frame's blank line, never on the rule.
        assert_eq!(
            o.lines(),
            [
                "",
                "> run it",
                "",
                "",
                "⚙ output of: ls",
                "",
                "Here it is.",
                ""
            ]
        );
    }

    #[test]
    fn append_chunk_strips_control_characters_and_maps_tabs() {
        let mut o = Output::new();
        o.begin_turn("x");
        o.append_chunk("a\tb\x1b[31mc\r");
        // Tab -> space; ESC and CR dropped; printable brackets stay inert.
        assert_eq!(o.lines().last().unwrap(), "a b[31mc");
    }

    /// **A put-back secret is kept where it lands** — after a control char
    /// the sanitizing drops, split at a newline — and asked of the model's
    /// lines alone (review stop 6, F 5).
    #[test]
    fn shown_spans_land_where_the_text_does() {
        let mut o = Output::new();
        o.begin_turn("hi");
        let reply = o.blocks.len() - 1;
        // "ab\x1bcd" shows as "abcd": "cd", chars 3..5 given, lands at 2..4.
        o.append_shown("ab\x1bcd", &[(3, 5)]);
        // " xy\nzw": the range over "y\nz" is two, one each side of the break.
        o.append_shown(" xy\nzw", &[(2, 5)]);
        assert_eq!(o.line_at((reply, 0)), Some("abcd xy"));
        assert_eq!(o.shown_at((reply, 0)), Some(vec![(2, 4), (6, 7)]));
        assert_eq!(o.shown_at((reply, 1)), Some(vec![(0, 1)]));
        o.append_chunk("\nplain");
        assert_eq!(
            o.shown_at((reply, 2)),
            Some(vec![]),
            "the model's, none put back"
        );
        assert_eq!(o.shown_at((1, 0)), None, "your echo is not the model's");
    }

    #[test]
    fn begin_turn_sanitizes_user_echo() {
        let mut o = Output::new();
        o.begin_turn("hi\x1b]0;pwned\x07 there");
        assert_eq!(o.lines()[1], "> hi]0;pwned there");
    }

    #[test]
    fn visible_window_wraps_to_width() {
        let vp = Viewport::default();
        let mut o = Output::new();
        o.begin_turn("x");
        o.append_chunk("abcdefghij"); // 10 chars
        let w = o.visible_window(&vp, &Shut::NONE, 4, 10);
        // Your side, then "abcd","efgh","ij"
        assert_eq!(w.total_rows, REPLY + 3);
        assert_eq!(w.rows[REPLY], "abcd");
        assert_eq!(w.rows[REPLY + 2], "ij");
    }

    #[test]
    fn follow_tail_keeps_bottom_visible() {
        let vp = Viewport::default();
        let o = filled(30);
        let w = o.visible_window(&vp, &Shut::NONE, 80, 5);
        assert!(Output::is_following_tail(&vp));
        assert_eq!(w.rows.len(), 5);
        assert_eq!(w.top + w.rows.len(), w.total_rows);
    }

    #[test]
    fn scroll_up_then_append_does_not_snap_to_tail() {
        let mut vp = Viewport::default();
        // The headline UX invariant: scrolled-back must stay put when
        // new content arrives.
        let mut o = filled(30);
        o.scroll_up(&mut vp, &Shut::NONE, 3, 80, 5);
        let top_before = o.visible_window(&vp, &Shut::NONE, 80, 5).top;
        o.begin_turn("more");
        for i in 0..30 {
            o.append_chunk(&format!("x{i}\n"));
        }
        o.end_turn();
        let top_after = o.visible_window(&vp, &Shut::NONE, 80, 5).top;
        assert_eq!(top_before, top_after, "scrolled viewport must not move");
        assert!(!Output::is_following_tail(&vp));
    }

    #[test]
    fn scroll_down_to_bottom_re_engages_follow_tail() {
        let mut vp = Viewport::default();
        let o = filled(30);
        o.scroll_up(&mut vp, &Shut::NONE, 10, 80, 5);
        assert!(!Output::is_following_tail(&vp));
        o.scroll_down(&mut vp, &Shut::NONE, usize::MAX, 80, 5);
        assert!(Output::is_following_tail(&vp));
    }

    #[test]
    fn zero_dim_viewport_returns_empty_window() {
        let vp = Viewport::default();
        let mut o = Output::new();
        o.begin_turn("anything");
        assert!(o.visible_window(&vp, &Shut::NONE, 0, 10).rows.is_empty());
        assert!(o.visible_window(&vp, &Shut::NONE, 10, 0).rows.is_empty());
    }

    #[test]
    fn multibyte_chars_dont_break_wrapping() {
        let vp = Viewport::default();
        let mut o = Output::new();
        o.begin_turn("x");
        o.append_chunk("αβγδεζηθικ"); // 10 Greek chars (1 cell each)
        let w = o.visible_window(&vp, &Shut::NONE, 4, 10);
        // Your side + 3 wrapped rows of the Greek line
        assert_eq!(w.total_rows, REPLY + 3);
        assert_eq!(w.rows[REPLY].chars().count(), 4);
        assert_eq!(w.rows[REPLY + 2].chars().count(), 2);
    }

    #[test]
    fn wide_chars_wrap_by_display_cells() {
        let vp = Viewport::default();
        let mut o = Output::new();
        o.begin_turn("x");
        o.append_chunk("中文字漢"); // 4 CJK chars, 2 cells each
        let w = o.visible_window(&vp, &Shut::NONE, 4, 10);
        // "> x" (3 cells, 1 row) + "中文" (4 cells) + "字漢" (4 cells).
        assert_eq!(w.rows, ["", "> x", "", "", "中文", "字漢"]);
    }

    #[test]
    fn cache_is_reused_across_redraws_at_same_width() {
        let vp = Viewport::default();
        let o = filled(50);
        // First window wraps every block once.
        let _ = o.visible_window(&vp, &Shut::NONE, 40, 10);
        let after_first = o.wrap_ops();
        assert!(after_first > 0, "first render must wrap");
        // A second redraw at the same width must re-wrap nothing.
        let _ = o.visible_window(&vp, &Shut::NONE, 40, 10);
        assert_eq!(o.wrap_ops(), after_first, "redraw must reuse the cache");
        // A different width forces a re-wrap...
        let _ = o.visible_window(&vp, &Shut::NONE, 20, 10);
        assert!(o.wrap_ops() > after_first, "width change must re-wrap");
    }

    /// **A copy brought up to date is the transcript, every time — copying
    /// only what changed** (buffers review, B F5: the mirror was copied whole
    /// per streamed batch). Through every way a transcript changes: a turn
    /// begun, a chunk streamed, a tool record put before the open reply and
    /// after it, a turn ended and one abandoned; and a clone changed apart
    /// from the copy's transcript, which shares its first blocks and not its
    /// last.
    #[test]
    fn a_copy_brought_up_to_date_is_the_transcript() {
        let mut o = filled(50);
        let (mut copy, mut synced) = (Vec::new(), Synced::default());
        let mut sync = |o: &Output, copy: &mut Vec<String>| {
            let (keep, tail) = o.lines_since(&mut synced);
            assert!(keep <= copy.len(), "keeps only what it has");
            copy.truncate(keep);
            let copied = tail.len();
            copy.extend(tail);
            assert_eq!(*copy, o.lines(), "the transcript");
            copied
        };
        assert_eq!(sync(&o, &mut copy), o.line_count(), "first, all of it");
        assert_eq!(sync(&o, &mut copy), 0, "unchanged, nothing");
        o.begin_turn("a question");
        sync(&o, &mut copy);
        o.push_tool_activity("$ ls", "a\nb");
        sync(&o, &mut copy);
        o.append_chunk("streaming, first part");
        sync(&o, &mut copy);
        o.append_chunk(" and more\nand a line");
        assert_eq!(sync(&o, &mut copy), 2, "only the open reply's lines");
        o.push_tool_activity("$ pwd", "/");
        sync(&o, &mut copy);
        o.end_turn();
        sync(&o, &mut copy);
        o.begin_turn("never answered");
        sync(&o, &mut copy);
        assert!(o.abandon_turn("you stopped it"), "premise: undone");
        sync(&o, &mut copy);
        let mut apart = o.clone();
        o.begin_turn("here");
        o.append_chunk("one way");
        sync(&o, &mut copy);
        apart.begin_turn("there");
        apart.append_chunk("another way");
        sync(&apart, &mut copy);
    }

    /// **Two panes at two widths both hit the cache** — one slot per width,
    /// two kept: an odd terminal split in two gives halves one column apart,
    /// and a single slot re-wrapped every block of a shared session twice a
    /// frame (buffers review, B F1: 1.6 s a frame at 20.8k lines).
    #[test]
    fn two_widths_both_hit_the_cache() {
        let vp = Viewport::default();
        let o = filled(50);
        let _ = o.visible_window(&vp, &Shut::NONE, 105, 10);
        let _ = o.visible_window(&vp, &Shut::NONE, 104, 10);
        let warm = o.wrap_ops();
        assert!(warm > 0, "premise: both widths wrapped once");
        for width in [105, 104, 105, 104] {
            let window = o.visible_window(&vp, &Shut::NONE, width, 10);
            let fresh = filled(50).visible_window(&vp, &Shut::NONE, width, 10);
            assert_eq!(window.rows, fresh.rows, "the rows at {width}");
        }
        assert_eq!(o.wrap_ops(), warm, "neither width wraps again");
        let _ = o.visible_window(&vp, &Shut::NONE, 60, 10);
        assert!(o.wrap_ops() > warm, "a third width wraps, over the older");
    }

    /// **A change empties both slots, not the latest** — the older width's
    /// pane showed the reply frozen as it was before the chunk, while the
    /// other pane went on (review stop 4, A2 1: resetting one slot passed all
    /// 1882 tests).
    #[test]
    fn a_change_reaches_the_older_width_too() {
        let vp = Viewport::default();
        let reply = |o: &mut Output, text: &str| {
            o.begin_turn("q");
            o.append_chunk(text);
        };
        let mut o = Output::new();
        reply(&mut o, "one two three four five six seven eight nine ten");
        let _ = o.visible_window(&vp, &Shut::NONE, 80, 40);
        let _ = o.visible_window(&vp, &Shut::NONE, 79, 40);
        o.append_chunk(" eleven twelve\nand a new line");
        let mut fresh = Output::new();
        reply(
            &mut fresh,
            "one two three four five six seven eight nine ten eleven twelve\nand a new line",
        );
        for width in [80, 79] {
            assert_eq!(
                o.visible_window(&vp, &Shut::NONE, width, 40).rows,
                fresh.visible_window(&vp, &Shut::NONE, width, 40).rows,
                "at {width}"
            );
        }
    }

    /// **A width change re-wraps, never re-styles** — styling (markdown,
    /// syntax highlighting once sealed) does not depend on the width, and
    /// redone inside every re-wrap it was what made a re-wrap cost 1.6 s a
    /// frame where prose alone cost 22 ms (B F1). A change to the lines
    /// styles again.
    #[test]
    fn a_width_change_does_not_restyle() {
        let vp = Viewport::default();
        let mut o = Output::new();
        o.begin_turn("show me");
        o.append_chunk("Here:\n```rust\nfn main() {}\n```");
        o.end_turn();
        let _ = o.visible_window(&vp, &Shut::NONE, 40, 10);
        let styled = o.style_ops();
        assert!(styled > 0, "premise: the reply styled");
        for width in [30, 20, 41] {
            let _ = o.visible_window(&vp, &Shut::NONE, width, 10);
        }
        assert_eq!(o.style_ops(), styled, "re-wrapped, not re-styled");
        o.begin_turn("more");
        o.append_chunk("and more");
        let _ = o.visible_window(&vp, &Shut::NONE, 41, 10);
        assert!(o.style_ops() > styled, "new lines are styled");
    }

    #[test]
    fn append_only_rewraps_the_touched_block() {
        let vp = Viewport::default();
        let mut o = filled(50);
        let _ = o.visible_window(&vp, &Shut::NONE, 40, 10); // prime the cache
        let primed = o.wrap_ops();
        let before = o.blocks.len();
        // Streaming one more chunk invalidates only the open block.
        o.begin_turn("q");
        o.append_chunk("partial");
        let _ = o.visible_window(&vp, &Shut::NONE, 40, 10);
        // The new blocks (your side and the open reply) wrap; the 50 older
        // lines' block(s) do not.
        let new = o.blocks.len() - before;
        assert_eq!(new, 5, "premise: your side's four and the reply");
        let delta = o.wrap_ops() - primed;
        assert!(delta <= new, "only the new blocks should wrap, got {delta}");
    }

    #[test]
    fn logical_to_visual_locates_a_wrapped_line() {
        // Your side (one row each) then "abcdefghij" wrapping at width 4 →
        // rows abcd/efgh/ij.
        let mut o = Output::new();
        o.begin_turn("hi");
        o.append_chunk("abcdefghij");
        // Flattened logical lines: [1]="> hi", [REPLY]="abcdefghij".
        assert_eq!(o.logical_to_visual(&Shut::NONE, 1, 0, 4), (1, 0));
        // col 0 of the long line is the row right after your side.
        assert_eq!(o.logical_to_visual(&Shut::NONE, REPLY, 0, 4), (REPLY, 0));
        // col 5 ('f') is in the 2nd wrapped row of that line, cell 1.
        assert_eq!(
            o.logical_to_visual(&Shut::NONE, REPLY, 5, 4),
            (REPLY + 1, 1)
        );
        // col 8 ('i') is in the 3rd wrapped row, cell 0.
        assert_eq!(
            o.logical_to_visual(&Shut::NONE, REPLY, 8, 4),
            (REPLY + 2, 0)
        );
    }

    #[test]
    fn logical_and_visual_projections_round_trip() {
        let vp = Viewport::default();
        let mut o = Output::new();
        o.begin_turn("hello world this is a longish first line");
        o.append_chunk("a reply with several words to wrap\nand a second line");
        o.end_turn();
        let width = 7u16;
        let total = o
            .visible_window(&vp, &Shut::NONE, width, u16::MAX)
            .total_rows;
        let lines = o.lines();
        for (li, line) in lines.iter().enumerate() {
            for col in 0..=line.chars().count() {
                let (r, c) = o.logical_to_visual(&Shut::NONE, li, col, width);
                if r >= total {
                    continue; // phantom row past the end — caller clamps
                }
                // Inverting then re-projecting is stable (the maps agree).
                let (li2, col2) = o.visual_to_logical(&Shut::NONE, r, c, width);
                assert_eq!(
                    o.logical_to_visual(&Shut::NONE, li2, col2, width),
                    (r, c),
                    "unstable at line {li} col {col}"
                );
            }
        }
    }

    #[test]
    fn focused_window_shows_styled_rows_and_the_cursor() {
        let vp = Viewport::default();
        let mut o = Output::new();
        o.begin_turn("hi"); // "> hi"
        o.append_chunk("abcdefghij"); // wraps at width 4 → abcd/efgh/ij
        o.end_turn();
        // Cursor on the reply ("abcdefghij"), col 5 ('f') → its 2nd row, cell 1.
        let fw = o.focused_window(&vp, &Shut::NONE, 4, 10, REPLY, 5, |_, _| None);
        let (crow, ccell) = fw.cursor.expect("cursor visible");
        assert_eq!(fw.rows[crow], "efgh");
        assert_eq!(ccell, 1);
        assert!(fw.selection.iter().all(Option::is_none), "no selection");
        assert_eq!(fw.roles.len(), fw.rows.len());
    }

    #[test]
    fn focused_window_projects_selection_across_wrapped_rows() {
        let vp = Viewport::default();
        let mut o = Output::new();
        o.begin_turn("x"); // "> x"
        o.append_chunk("abcdefghij"); // the reply, wraps abcd/efgh/ij at width 4
        o.end_turn();
        // Select logical chars [5,9) of the reply — "fghi", spanning the efgh and ij rows.
        let fw = o.focused_window(&vp, &Shut::NONE, 4, 10, REPLY, 0, |line, _len| {
            (line == REPLY).then_some((5, 9))
        });
        let efgh = fw.rows.iter().position(|r| r == "efgh").unwrap();
        let ij = fw.rows.iter().position(|r| r == "ij").unwrap();
        assert_eq!(fw.selection[efgh], Some((1, 4)), "f,g,h of efgh");
        assert_eq!(fw.selection[ij], Some((0, 1)), "i of ij");
        // Rows outside the selection carry none.
        let echo = fw.rows.iter().position(|r| r == "> x").unwrap();
        assert_eq!(fw.selection[echo], None);
    }

    /// Only **assistant** rows are markdown-parsed; tool output and user
    /// echoes are not.
    ///
    /// The renderer feeds the per-char style map to `view::styled_line` for
    /// assistant rows only — everything else takes one flat per-role style and
    /// never reads the map. Building it anyway means markdown-parsing (and, on
    /// a fenced block, syntax-highlighting) up to `exec.max_output_bytes` of
    /// arbitrary command output **on every re-wrap**, then throwing all of it
    /// away. `cat README.md` is not an exotic thing for an agent to run.
    ///
    /// Reviewer D found the cost and the gate went in without a test, so
    /// deleting the match arm passed the suite. This is that test — and it
    /// asserts the *contract* (who gets styled) rather than a duration, so it
    /// stays honest if the parser gets faster.
    #[test]
    fn only_assistant_rows_are_markdown_parsed() {
        let vp = Viewport::default();
        let markdown = "# heading with `code` and **bold**";

        let mut assistant = Output::new();
        assistant.begin_turn("x");
        assistant.append_chunk(markdown);
        assistant.end_turn();
        let fw = assistant.focused_window(&vp, &Shut::NONE, 80, 10, 0, 0, |_, _| None);
        let row = fw.rows.iter().position(|r| r.contains("heading")).unwrap();
        assert!(
            !fw.styles[row].is_empty(),
            "an assistant row carries its per-char markdown styling"
        );

        // The same text as TOOL output — the `cat README.md` case.
        let mut tool = Output::new();
        tool.begin_turn("x");
        tool.push_tool_activity("$ cat README.md", markdown);
        tool.end_turn();
        let fw = tool.focused_window(&vp, &Shut::NONE, 80, 10, 0, 0, |_, _| None);
        let row = fw.rows.iter().position(|r| r.contains("heading")).unwrap();
        assert_eq!(
            fw.styles[row][2..],
            vec![crate::tool_record::OUTPUT; fw.rows[row].chars().count() - 2],
            "tool output is NOT markdown-parsed — it wears the style its record \
             gave it, flat, and no heading of its own"
        );
    }

    /// A selection that ends **before** a later wrapped row begins must not
    /// underflow — the guard on the live focused render path.
    ///
    /// The existing wrapped-selection test covers a range that *spans* rows,
    /// where every subtraction is safe by construction. This is the other side:
    /// on rows past the selection, `hi` clips to `shi`, which is **less than
    /// the row's start offset**, so `hi - cstart` underflows. `then` (lazy)
    /// is what stops it; `then_some` would evaluate the tuple unconditionally
    /// and panic (`[[feedback_then_some_eager]]`).
    ///
    /// Reviewers mutation-verified that this guard had no test, and said its
    /// twin in the prompt (`Buffer::visible_window`) had one; it did not
    /// until review stop 12 (T6 4,
    /// `visible_window_a_selection_before_a_wrapped_row_leaves_it_unselected`).
    /// This one is on the path that runs every frame a transcript pane is
    /// focused.
    #[test]
    fn a_selection_ending_before_a_wrapped_row_leaves_that_row_unselected() {
        let vp = Viewport::default();
        let mut o = Output::new();
        o.begin_turn("x"); // "> x"
        o.append_chunk("abcdefghij"); // the reply, wraps abcd/efgh/ij at width 4
        o.end_turn();
        // Select chars [0,2) of the reply — "ab", entirely inside its FIRST row.
        // Rows "efgh" (starts at char 4) and "ij" (char 8) are both past it.
        let fw = o.focused_window(&vp, &Shut::NONE, 4, 10, REPLY, 0, |line, _len| {
            (line == REPLY).then_some((0, 2))
        });
        let abcd = fw.rows.iter().position(|r| r == "abcd").unwrap();
        let efgh = fw.rows.iter().position(|r| r == "efgh").unwrap();
        let ij = fw.rows.iter().position(|r| r == "ij").unwrap();
        assert_eq!(fw.selection[abcd], Some((0, 2)), "a,b of abcd");
        assert_eq!(fw.selection[efgh], None, "past the selection, not inverted");
        assert_eq!(fw.selection[ij], None);
    }

    #[test]
    fn chat_click_inverts_the_cursor_projection() {
        let vp = Viewport::default();
        let mut o = Output::new();
        o.begin_turn("hi");
        o.append_chunk("abcdefghij");
        o.end_turn();
        let (w, h) = (4u16, 10u16);
        let fw = o.focused_window(&vp, &Shut::NONE, w, h, REPLY, 5, |_, _| None);
        let (crow, ccell) = fw.cursor.unwrap();
        // Clicking the cell the cursor renders at recovers the logical position.
        assert_eq!(
            o.chat_click(&vp, &Shut::NONE, (w, h), (REPLY, 5), (crow, ccell)),
            (REPLY, 5)
        );
    }

    /// **…and it still does when the pane is SCROLLED BACK.**
    ///
    /// The test above builds four rows in a ten-row window, where the retained
    /// top and the old `cursor_abs - (h - 1)` formula are both zero — a harness
    /// that makes the two rules agree, so it could not see them disagree. With
    /// a document taller than the window and the viewport parked away from the
    /// cursor, the old formula landed sixteen lines from the pointer, and a
    /// drag reverse-videoed a span the user never selected (Reviewer A,
    /// round 3).
    #[test]
    fn chat_click_reads_the_retained_viewport_not_the_cursor() {
        let mut o = Output::new();
        for i in 0..40 {
            o.begin_turn(&format!("line{i}"));
            o.end_turn();
        }
        let (w, h) = (40u16, 10u16);
        let total = o.total_visual_rows(&Shut::NONE, w);
        assert!(
            total > h as usize,
            "premise: the document overflows the pane"
        );

        // Parked in the middle, and the cursor far below it — the state the
        // two rules disagree in.
        let mut vp = Viewport::default();
        let (line, col) = o.visual_to_logical(&Shut::NONE, 5, 0, w);
        vp.freeze_at(line, col);
        let (cursor_line, cursor_col) = (30usize, 0usize);
        assert_ne!(
            o.top_row(&vp, &Shut::NONE, w, h as usize).0,
            o.logical_to_visual(&Shut::NONE, cursor_line, cursor_col, w)
                .0
                .saturating_sub(h as usize - 1),
            "premise: retained top and cursor-derived top differ here, or this \
             test cannot tell them apart"
        );

        // The top visible row is the row the renderer draws first.
        let fw = o.focused_window(&vp, &Shut::NONE, w, h, cursor_line, cursor_col, |_, _| None);
        let (line, _) = o.chat_click(&vp, &Shut::NONE, (w, h), (cursor_line, cursor_col), (0, 0));
        assert_eq!(
            o.lines()[line].trim_start_matches("> "),
            fw.rows[0].trim_start_matches("> "),
            "clicking the top row lands on the text drawn there"
        );
    }

    #[test]
    fn focused_chat_renders_the_same_content_as_the_streaming_view() {
        let vp = Viewport::default();
        // The core P3 guarantee, machine-checked: focusing the chat shows the
        // IDENTICAL word-wrapped + styled + role-tagged rows as the unfocused /
        // streaming view — it adds a cursor and nothing else (no reflow, no style
        // loss). Compared at the same (tail) scroll position. Markdown in the
        // content makes `styles` non-trivial, so the styles assertion proves
        // highlighting is preserved, not just that two plain rows match.
        let mut o = Output::new();
        o.begin_turn("a question with **emphasis** and several words that wrap");
        o.append_chunk("a reply with `code` and enough words to wrap across rows");
        o.end_turn();
        let (w, h) = (16u16, 6u16);

        let streaming = o.visible_window(&vp, &Shut::NONE, w, h); // unfocused: follow-tail
        // Focused: place the cursor at the tail so the window frames the same rows.
        let last_line = o.lines().len() - 1;
        let focused = o.focused_window(&vp, &Shut::NONE, w, h, last_line, 0, |_, _| None);

        assert_eq!(
            focused.rows, streaming.rows,
            "same wrapped text — no reflow"
        );
        assert_eq!(
            focused.styles, streaming.styles,
            "same styling — no style loss"
        );
        assert_eq!(focused.roles, streaming.roles, "same role bubbles");
        assert!(focused.cursor.is_some(), "focused view adds a cursor");
    }

    #[test]
    fn focused_window_caps_rows_to_height() {
        let vp = Viewport::default();
        let o = filled(40);
        let fw = o.focused_window(&vp, &Shut::NONE, 20, 6, 0, 0, |_, _| None);
        assert!(fw.rows.len() <= 6);
        assert_eq!(fw.styles.len(), fw.rows.len());
        assert_eq!(fw.selection.len(), fw.rows.len());
    }

    #[test]
    fn visual_to_logical_clamps_past_the_end() {
        let mut o = Output::new();
        o.begin_turn("hi");
        o.append_chunk("yo");
        o.end_turn();
        // A row far past the content clamps to the last line's end.
        let (li, col) = o.visual_to_logical(&Shut::NONE, 9999, 0, 8);
        let lines = o.lines();
        assert_eq!(li, lines.len() - 1);
        assert_eq!(col, lines[li].chars().count());
    }

    /// Build an `Output` from a list of streamed chunks (one turn).
    fn from_chunks(chunks: &[String]) -> Output {
        let mut o = Output::new();
        o.begin_turn("p");
        for c in chunks {
            o.append_chunk(c);
        }
        o.end_turn();
        o
    }

    proptest! {
        /// Across arbitrary content and viewports, the window never
        /// reports more rows than the viewport can hold, and its slice
        /// stays within the total.
        #[test]
        fn visible_rows_never_exceed_height(
            chunks in prop::collection::vec("[a-zA-Z0-9 \n]{0,40}", 0..30),
            width in 1u16..120,
            height in 1u16..50,
        ) {
            let vp = Viewport::default();
                let o = from_chunks(&chunks);
            let w = o.visible_window(&vp, &Shut::NONE,width, height);
            prop_assert!(w.rows.len() <= height as usize);
            prop_assert!(w.top + w.rows.len() <= w.total_rows);
        }

        /// Following tail (the default), the window's bottom aligns with
        /// the buffer's last visual row.
        #[test]
        fn follow_tail_anchors_to_bottom(
            chunks in prop::collection::vec("[a-zA-Z0-9 ]{1,40}", 1..30),
            width in 1u16..120,
            height in 1u16..50,
        ) {
            let vp = Viewport::default();
                let o = from_chunks(&chunks);
            prop_assert!(Output::is_following_tail(&vp), "fresh Output should follow tail");
            let w = o.visible_window(&vp, &Shut::NONE,width, height);
            prop_assume!(w.total_rows > 0);
            prop_assert_eq!(w.top + w.rows.len(), w.total_rows);
        }

        /// Total visual rows is monotonic in appended content at a fixed
        /// width.
        #[test]
        fn append_monotonically_grows_total_rows(
            chunks in prop::collection::vec("[a-zA-Z0-9 ]{1,40}", 1..30),
            width in 1u16..120,
        ) {
            let vp = Viewport::default();
                let mut o = Output::new();
            o.begin_turn("p");
            let mut prev = o.visible_window(&vp, &Shut::NONE,width, 1).total_rows;
            for c in &chunks {
                o.append_chunk(c);
                let total = o.visible_window(&vp, &Shut::NONE,width, 1).total_rows;
                prop_assert!(total >= prev);
                prev = total;
            }
        }

        /// A top on a line past the end clamps to the last viable position —
        /// never panics, never over-runs.
        #[test]
        fn a_top_past_the_end_clamps_gracefully(
            chunks in prop::collection::vec("[a-zA-Z0-9 ]{1,40}", 1..30),
            width in 1u16..120,
            height in 1u16..50,
        ) {
            let mut vp = Viewport::default();
                let o = from_chunks(&chunks);
            vp.freeze_at(usize::MAX, usize::MAX);
            let w = o.visible_window(&vp, &Shut::NONE,width, height);
            prop_assert!(w.top + w.rows.len() <= w.total_rows);
        }
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;

    fn tall(n: usize) -> Output {
        let mut o = Output::new();
        o.begin_turn("q");
        for i in 0..n {
            o.append_chunk(&format!("line{i}\n"));
        }
        o.end_turn();
        o
    }

    /// A window frozen with row `row` at its top, at width `w`, nothing shut
    /// — the text at that row, as `move_top` freezes it.
    fn at_row(o: &Output, row: usize, w: u16) -> Viewport {
        let mut vp = Viewport::default();
        let (line, col) = o.visual_to_logical(&Shut::NONE, row, 0, w);
        vp.freeze_at(line, col);
        vp
    }

    /// The row at `vp`'s top, `w` by `h`, nothing shut.
    fn top(o: &Output, vp: &Viewport, w: u16, h: u16) -> usize {
        o.top_row(vp, &Shut::NONE, w, usize::from(h)).0
    }

    /// A transcript with every kind of row: replies that wrap at word
    /// boundaries, blank lines, wide characters, and a thought the shipped
    /// rule shuts to one row.
    fn mixed() -> Output {
        let mut o = Output::new();
        for turn in 0..6 {
            o.begin_turn(&format!("question {turn}"));
            o.append_thought(
                "a thought that runs on past any narrow width\nsecond\nthird",
                &[],
            );
            o.end_thought();
            o.append_chunk("a reply line long enough to wrap at every width tried, word by word\n");
            o.append_chunk("\n漢字の行も幅が二つずつ数えられる行\nshort\n");
            o.end_turn();
        }
        o
    }

    /// The line where `needle`'s `nth` occurrence begins in `o`.
    fn line_of(o: &Output, needle: &str, nth: usize) -> usize {
        (o.lines().iter().enumerate())
            .filter(|(_, l)| l.starts_with(needle))
            .nth(nth)
            .expect("the fixture has it")
            .0
    }

    /// **A row put at the top reads back as itself** — every row short of
    /// the last page, at four widths, open and in the shipped rule's folds:
    /// the round trip a top kept as text rests on (S21-Q10).
    #[test]
    fn a_row_put_at_the_top_reads_back_as_itself() {
        let o = mixed();
        let shipped = Folds::default();
        let shipped = shipped.shut(FoldRule::SHIPPED);
        let h = 5;
        for shut in [&Shut::NONE, &shipped] {
            for w in [9u16, 17, 40, 80] {
                let last_page = o.total_visual_rows(shut, w) - h;
                for row in 0..last_page {
                    let mut vp = Viewport::following_tail();
                    o.move_top(&mut vp, (shut, w), (last_page, row), last_page);
                    assert_eq!(o.top_row(&vp, shut, w, h).0, row, "row {row}, width {w}");
                }
            }
        }
    }

    /// **A top keeps its text across a width, a fold and a pane with no
    /// room** — the classes S21-Q10 removes by construction, each once found
    /// live: a `:vsplit` read a window laid at the old width (A H1), `zM`
    /// and `zR` moved the top (A M5), a frame at width 0 put it at the start
    /// for good (A M6). Reading never writes: the same view, read anywhere,
    /// is read back unchanged.
    #[test]
    fn a_top_keeps_its_text_across_widths_and_folds() {
        let o = mixed();
        let shipped = Folds::default();
        let shipped = shipped.shut(FoldRule::SHIPPED);
        let h = 5;
        let reply = line_of(&o, "a reply line", 3);
        let mut vp = Viewport::default();
        vp.freeze_at(reply, 0);
        let line_at_top = |vp: &Viewport, shut: &Shut<'_>, w: u16| {
            let row = o.top_row(vp, shut, w, h).0;
            o.visual_to_logical(shut, row, 0, w)
        };
        for w in [80u16, 17, 9, 80] {
            assert_eq!(line_at_top(&vp, &Shut::NONE, w), (reply, 0), "width {w}");
            assert_eq!(line_at_top(&vp, &shipped, w), (reply, 0), "width {w}, shut");
        }
        assert_eq!(o.top_row(&vp, &Shut::NONE, 0, h).0, 0, "no room: no row");
        assert_eq!(vp.top(), Some((reply, 0)), "and nothing written");

        // A line inside a shut thought: on the fold's row while it is shut,
        // its own once it opens.
        let inside = line_of(&o, "second", 3);
        vp.freeze_at(inside, 0);
        let fold_row = o.logical_to_visual(&shipped, inside - 1, 0, 40).0;
        assert_eq!(o.top_row(&vp, &shipped, 40, h).0, fold_row);
        assert_eq!(line_at_top(&vp, &Shut::NONE, 40), (inside, 0));
    }

    /// **A top that did not move keeps its text** (`move_top`). Frozen on a
    /// reply's second row at width 17, a `j` at width 80 — where that column
    /// is part-way into the line's one row — moves nothing, and back at 17 the
    /// same row is at the top. Rewritten from the row each time, it was the
    /// row's start at 80, the line's first row at 17: a window crept up a row
    /// a round trip.
    #[test]
    fn a_top_that_does_not_move_keeps_its_text() {
        let o = mixed();
        let reply = line_of(&o, "a reply line", 3);
        let second_row = o.logical_to_visual(&Shut::NONE, reply, 0, 17).0 + 1;
        let held = o.visual_to_logical(&Shut::NONE, second_row, 0, 17);
        assert_eq!(held.0, reply, "premise: the reply wraps at 17");
        let mut vp = Viewport::default();
        vp.freeze_at(held.0, held.1);
        let at_80 = o.top_row(&vp, &Shut::NONE, 80, 5).0;
        assert_ne!(
            o.visual_to_logical(&Shut::NONE, at_80, 0, 80),
            held,
            "premise: at 80 its column is part-way into a row"
        );
        o.follow_cursor(&mut vp, &Shut::NONE, 80, 5, reply, 0, 0);
        assert_eq!(vp.top(), Some(held), "nothing moved, nothing rewritten");
        assert_eq!(o.top_row(&vp, &Shut::NONE, 17, 5).0, second_row);
    }

    /// Following, the top is the last page — or the start, when it all fits.
    #[test]
    fn a_top_while_following_is_the_last_page() {
        let o = tall(30);
        let total = o.total_visual_rows(&Shut::NONE, 40);
        let vp = Viewport::following_tail();
        assert_eq!(top(&o, &vp, 40, 5), total - 5);
        assert_eq!(top(&o, &vp, 40, 200), 0, "content that fits");
        assert_eq!(top(&Output::new(), &vp, 40, 5), 0, "no content");
    }

    /// Frozen, the top is its text's row — clamped to the last page, where a
    /// top past the end, or a window taller than its content, lands.
    #[test]
    fn a_frozen_top_is_its_rows_clamped_to_the_last_page() {
        let o = tall(30);
        let total = o.total_visual_rows(&Shut::NONE, 40);
        assert_eq!(top(&o, &at_row(&o, 3, 40), 40, 5), 3);
        let mut past = Viewport::default();
        past.freeze_at(usize::MAX, 0);
        assert_eq!(top(&o, &past, 40, 5), total - 5);
        assert_eq!(top(&o, &at_row(&o, 3, 40), 40, 200), 0);
    }

    /// From the tail, a scroll up freezes the window `delta` rows above it.
    #[test]
    fn scroll_up_from_the_tail_freezes_above_it() {
        let o = tall(30);
        let last_page = o.total_visual_rows(&Shut::NONE, 40) - 5;
        let mut vp = Viewport::following_tail();
        o.scroll_up(&mut vp, &Shut::NONE, 3, 40, 5);
        assert!(!vp.is_following_tail());
        assert_eq!(top(&o, &vp, 40, 5), last_page - 3);
    }

    /// At the top, PgUp again moves nothing and changes nothing — the text it
    /// holds included.
    #[test]
    fn scroll_up_at_the_top_is_a_noop_and_keeps_its_state() {
        let o = tall(30);
        let mut vp = Viewport::following_tail();
        o.scroll_up(&mut vp, &Shut::NONE, usize::MAX, 40, 5);
        assert!(!vp.is_following_tail());
        assert_eq!(top(&o, &vp, 40, 5), 0);
        let held = vp;
        o.scroll_up(&mut vp, &Shut::NONE, 5, 40, 5);
        assert_eq!(vp, held);
    }

    /// Content that fits: a scroll up does nothing, and does NOT stop
    /// following — the tail marker must not flip on a key that moved nothing.
    #[test]
    fn scroll_up_on_content_that_fits_is_a_noop() {
        let o = tall(3);
        let mut vp = Viewport::following_tail();
        o.scroll_up(&mut vp, &Shut::NONE, 2, 40, 200);
        assert!(vp.is_following_tail());
    }

    /// A scroll down that reaches the end follows it; one short of it stays
    /// frozen where it stopped; at the tail, it is idempotent.
    #[test]
    fn scroll_down_follows_the_end_only_on_reaching_it() {
        let o = tall(30);
        let mut vp = Viewport::following_tail();
        o.scroll_up(&mut vp, &Shut::NONE, usize::MAX, 40, 5);
        o.scroll_down(&mut vp, &Shut::NONE, 3, 40, 5);
        assert!(!vp.is_following_tail());
        assert_eq!(top(&o, &vp, 40, 5), 3);
        o.scroll_down(&mut vp, &Shut::NONE, usize::MAX, 40, 5);
        assert!(vp.is_following_tail(), "the end reached: following it");
        let held = vp;
        o.scroll_down(&mut vp, &Shut::NONE, 5, 40, 5);
        assert_eq!(vp, held);
    }

    /// The bug Steven hit twice: with the viewport DERIVED from the cursor, the
    /// cursor was pinned to the last row — you could scroll the text but never
    /// move off the bottom line. The viewport is retained, so a cursor in the
    /// middle of the window leaves it alone.
    #[test]
    fn the_cursor_is_not_pinned_to_the_bottom_row() {
        let o = tall(40);
        let (w, h) = (40u16, 10u16);
        // Viewport parked near the end; cursor a few rows above the bottom.
        let vp = at_row(&o, 20, w);
        let win = o.focused_window(&vp, &Shut::NONE, w, h, 24, 0, |_, _| None);
        let row = win.cursor.expect("cursor is visible").0;
        assert!(
            row < (h as usize) - 1,
            "the cursor sits inside the window, not welded to the last row (got {row})"
        );
    }

    /// Moving UP near the ceiling pulls the view up; moving DOWN near the floor
    /// pushes it down; in the middle nothing moves. The soft margin is
    /// `scrolloff`.
    #[test]
    fn the_cursor_pushes_the_view_at_the_soft_edges() {
        let o = tall(60);
        let (w, h, so) = (40u16, 10u16, 3usize);
        let mut vp = at_row(&o, 20, w);

        // Middle of the window: no movement.
        o.follow_cursor(&mut vp, &Shut::NONE, w, h, 25, 0, so);
        let mid = top(&o, &vp, w, h);
        assert_eq!(mid, 20, "a centred cursor moves nothing");

        // Within `scrolloff` of the top: the view follows up, to leave `so`
        // rows above the cursor — the rule's number, not just a direction
        // (review stop 12, T6 13).
        o.follow_cursor(&mut vp, &Shut::NONE, w, h, 21, 0, so);
        let up = top(&o, &vp, w, h);
        assert_eq!(up, 21 - so, "approaching the ceiling scrolls up");

        // Within `scrolloff` of the bottom: the view follows down, to leave
        // `so` rows below it.
        let mut vp = at_row(&o, 20, w);
        o.follow_cursor(&mut vp, &Shut::NONE, w, h, 28, 0, so);
        let down = top(&o, &vp, w, h);
        assert_eq!(
            down,
            28 + so + 1 - h as usize,
            "approaching the floor scrolls down"
        );
    }

    /// **A line past the end places the last one** rather than slicing past
    /// the wrap cache — the chat cursor that names the line is a mirror,
    /// re-synced once a frame, and a panic is not an answer to a stale one.
    #[test]
    fn a_line_past_the_end_is_placed_as_the_last() {
        let o = tall(60);
        let (w, h) = (40u16, 10u16);
        let mut vp = at_row(&o, 0, w);
        o.place(
            &mut vp,
            &Shut::NONE,
            w,
            h,
            10_000,
            crate::viewport::Place::Top,
            0,
        );
        assert!(vp.is_following_tail(), "the last page: the last line's");
    }

    /// A margin larger than the window degrades to keep-centered rather than
    /// fighting itself (vim's `so=999`).
    #[test]
    fn an_oversized_margin_does_not_thrash() {
        let o = tall(60);
        // From a window the cursor is off, so it must move: the margin clamps
        // to half the window, `(10 - 1) / 2` rows, so the cursor ends in the
        // middle band — that many rows on either side — and "does not
        // thrash" is a fixed point: following again moves nothing. It
        // asserted `top <= 25`, which 0 satisfied (review stop 12, T6 13).
        let mut vp = at_row(&o, 0, 40);
        o.follow_cursor(&mut vp, &Shut::NONE, 40, 10, 25, 0, 999);
        let at = top(&o, &vp, 40, 10);
        let margin = (10 - 1) / 2;
        assert!(
            (margin..=10 - 1 - margin).contains(&(25 - at)),
            "keep-centred: the cursor at row {} of the window",
            25 - at
        );
        o.follow_cursor(&mut vp, &Shut::NONE, 40, 10, 25, 0, 999);
        assert_eq!(top(&o, &vp, 40, 10), at, "and stays");
    }
}

#[cfg(test)]
mod streaming_cost_tests {
    use super::*;

    /// **The open block is styled flat; sealing re-styles it in colour.**
    ///
    /// `markdown` can be told which to do, and `Block::ensure` has to actually
    /// ask — a `Highlight` parameter nothing passes correctly is a knob, not a
    /// fix. Asserted through `Output`, at the seam where the streaming
    /// invalidate-then-restyle loop actually lives.
    #[test]
    fn ensure_asks_the_block_whether_it_is_still_arriving() {
        let mut out = Output::new();
        out.begin_turn("write me some rust");
        for chunk in ["```rust\n", "fn main() { let x = 1; }\n", "```\n"] {
            out.append_chunk(chunk);
        }

        // The code line's styles, found by the row's text.
        let code_row = |out: &Output| -> Vec<CellStyle> {
            let vp = Viewport::default();
            let w = out.visible_window(&vp, &Shut::NONE, 80, 40);
            let i = w
                .rows
                .iter()
                .position(|r| r.contains("fn main"))
                .expect("the code line is on screen");
            w.styles[i].clone()
        };

        let open = code_row(&out);
        assert!(
            !open.is_empty() && open.iter().all(|c| *c == crate::markdown::CODE_STYLE),
            "a streaming fence must not pay for syntect: {open:?}"
        );

        out.end_turn();
        let sealed = code_row(&out);
        assert!(
            sealed.iter().any(|c| *c != crate::markdown::CODE_STYLE),
            "sealing must run the real pass: {sealed:?}"
        );
    }
}

/// **A place in the conversation stays where it was set** ([`Anchor`]) —
/// through the three things that happen at a transcript's tail (`marks.md`
/// §4, measured): lines appended, a record inserted ahead of an empty open
/// block, blocks truncated or popped.
#[cfg(test)]
mod anchor_tests {
    use super::*;
    use proptest::prelude::*;

    /// Two turns: one told something, with a reply and a record; one open.
    fn two_turns() -> Output {
        let mut o = Output::new();
        o.told(&[shannon_core::Block::Context {
            kind: shannon_core::ContextKind::Memory,
            source: "/p/memory/MEMORY.md".to_string(),
            text: "remember this".to_string(),
        }]);
        o.begin_turn("one");
        o.append_chunk("a reply\nof two lines");
        o.push_tool_activity("$ ls", "a.txt\nb.txt");
        o.append_chunk("after it");
        o.end_turn();
        o.begin_turn("two");
        o.append_chunk("stream");
        o
    }

    /// **Every line and column, there and back**, and each anchor's turn the
    /// line's: `anchor_at` and `locate` are one coordinate system.
    #[test]
    fn every_place_anchors_and_locates_back_to_itself() {
        let o = two_turns();
        let lines = o.lines();
        assert!(lines.len() > 10, "premise: something to walk: {lines:?}");
        for (line, text) in lines.iter().enumerate() {
            for col in [0, text.chars().count()] {
                let anchor = o.anchor_at(line, col).expect("a line has a place");
                assert_eq!(o.locate(anchor), Some((line, col)), "line {line}: {text:?}");
                let span = (o.spans()).find(|s| s.id == anchor.block).unwrap();
                assert_eq!(span.turn, o.turn_of_line(line), "line {line}");
            }
        }
        assert_eq!(o.anchor_at(lines.len(), 0), None, "past the end");
    }

    /// **A record inserted ahead keeps a place on what it was set on**: an
    /// empty open block takes the record in front of it (`push_tool_record`),
    /// so the block's index moves, and a place by index would name the
    /// record's first line.
    #[test]
    fn a_record_inserted_ahead_moves_the_line_not_the_place() {
        let mut o = Output::new();
        o.begin_turn("run it");
        let open = o.line_count() - 1;
        let at = o.anchor_at(open, 0).unwrap();
        let echo = o.anchor_at(1, 2).unwrap();
        let blocks = o.blocks.len();
        o.push_tool_activity("$ ls", "a.txt");
        assert_eq!(
            o.blocks.len(),
            blocks + 1,
            "premise: inserted, not pushed after"
        );
        let (line, _) = o.locate(at).expect("the open block is still there");
        assert!(line > open, "premise: the line moved: {line} from {open}");
        assert_eq!(
            o.lines()[open],
            "$ ls",
            "premise: by index it names the record"
        );
        o.append_chunk("the reply");
        assert_eq!(o.locate(at), Some((line, 0)), "streamed into, still there");
        assert_eq!(o.lines()[line], "the reply", "the place is the reply's");
        assert_eq!(o.locate(echo), Some((1, 2)), "a place above did not move");
    }

    /// **A place in a block that is gone is not set** — vim erases a mark
    /// with its line (E20). `abandon_turn` gives the next turn the same
    /// index and lines, so a place by turn and line would name the new echo;
    /// `end_turn` pops an empty open block after a record.
    #[test]
    fn a_place_in_a_block_that_is_gone_is_not_set() {
        let mut o = Output::new();
        o.begin_turn("never sent");
        let echo = o.anchor_at(1, 0).unwrap();
        assert!(
            o.abandon_turn("you stopped it"),
            "premise: nothing ran, so it is undone"
        );
        o.begin_turn("sent");
        assert_eq!(o.turn_of_line(1), Some(0), "premise: the index is reused");
        assert_eq!(o.locate(echo), None, "the abandoned echo is not this one");

        o.push_tool_activity("$ ls", "a.txt");
        let open = o.anchor_at(o.line_count() - 1, 0).unwrap();
        let blocks = o.blocks.len();
        o.end_turn();
        assert_eq!(o.blocks.len(), blocks - 1, "premise: the open block popped");
        assert_eq!(o.locate(open), None, "popped with it");
    }

    /// What a transcript's tail can do, in any order.
    #[derive(Debug, Clone)]
    enum Op {
        Begin,
        Chunk(String),
        Record,
        End,
        Abandon,
        /// Set a place: a line (as a seed, modulo the line count) and a
        /// column.
        Place(usize, usize),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            Just(Op::Begin),
            "[a-z \n]{0,12}".prop_map(Op::Chunk),
            Just(Op::Record),
            Just(Op::End),
            Just(Op::Abandon),
            (any::<usize>(), 0usize..20).prop_map(|(seed, col)| Op::Place(seed, col)),
        ]
    }

    proptest! {
        /// **A place names the text it was set on wherever it resolves** —
        /// that text, or it grown at its end — at its column; **and it
        /// resolves while nothing could have removed its block.**
        ///
        /// Only `end_turn` (its pop) and `abandon_turn` remove blocks, so a
        /// place with neither after it must resolve. Without that half a
        /// `None` passed for any place, and an id re-made at every chunk
        /// would have lost every place green. (A count of lines is no
        /// oracle for the other half: a record inserted ahead of the open
        /// block moves it down before `end_turn` pops it.)
        #[test]
        fn a_place_names_the_text_it_was_set_on(ops in prop::collection::vec(op(), 0..60)) {
            let mut o = Output::new();
            // Each place, the text it was set on, and whether a remover has
            // run since.
            let mut places: Vec<(Anchor, String, bool)> = Vec::new();
            for op in ops {
                let removes = matches!(op, Op::End | Op::Abandon);
                match op {
                    Op::Begin => o.begin_turn("p"),
                    Op::Chunk(text) => o.append_chunk(&text),
                    Op::Record => o.push_tool_activity("$ ls", "a.txt"),
                    Op::End => o.end_turn(),
                    Op::Abandon => {
                        o.abandon_turn("you stopped it");
                    }
                    Op::Place(seed, col) => {
                        let n = o.line_count();
                        if n > 0 {
                            let line = seed % n;
                            let anchor = o.anchor_at(line, col).expect("a line has a place");
                            places.push((anchor, o.lines()[line].clone(), false));
                        }
                    }
                }
                for (.., removed) in &mut places {
                    *removed |= removes;
                }
            }
            let lines = o.lines();
            for (anchor, was, removed) in places {
                prop_assert!(removed || o.locate(anchor).is_some(), "{:?} on {:?} lost", anchor, was);
                if let Some((line, col)) = o.locate(anchor) {
                    prop_assert!(lines[line].starts_with(&was), "{was:?} became {:?}", lines[line]);
                    prop_assert_eq!(col, anchor.col);
                }
            }
        }
    }

    /// **Many places found in one walk say what `locate` says of each** —
    /// and their turn and their line's text; a place whose block is gone is
    /// `None`, in its own slot, the rest unmoved.
    #[test]
    fn many_places_are_found_as_each_is_located() {
        let mut o = two_turns();
        let n = o.line_count();
        let mut places: Vec<Anchor> = (0..n).map(|l| o.anchor_at(l, 1).unwrap()).collect();
        o.begin_turn("gone soon");
        let gone = o.anchor_at(o.line_count() - 1, 0).unwrap();
        places.insert(3, gone);
        assert!(o.abandon_turn("you stopped it"), "premise: undone");
        let found = o.find_each(&places);
        assert_eq!(found.len(), places.len());
        assert_eq!(found[3], None, "gone");
        let lines = o.lines();
        for (anchor, found) in places.iter().zip(&found).filter(|(a, _)| **a != gone) {
            let found = found.expect("still there");
            assert_eq!(Some((found.line, found.col)), o.locate(*anchor));
            assert_eq!(found.text, lines[found.line]);
            assert_eq!(found.turn, o.turn_of_line(found.line));
        }
    }

    /// **A quoted phrase is found where the agent means it** (`find_quote`,
    /// the `mark` tool's `at`): in the latest turn that has it, searching
    /// back, or in the turn named; on the first line of that turn holding
    /// it; at a column in chars, past any wide char before it. Absent, or
    /// empty, it is nowhere.
    #[test]
    fn a_quote_is_found_in_the_latest_turn_that_has_it() {
        let mut o = Output::new();
        for (ask, reply) in [
            ("one", "é the plan, once"),
            ("two", "the plan\nthe plan, two"),
        ] {
            o.begin_turn(ask);
            o.append_chunk(reply);
            o.end_turn();
        }
        let at = |o: &Output, a: Anchor| {
            let (line, col) = o.locate(a).unwrap();
            (o.lines()[line].clone(), col)
        };
        // Nothing held: each line was sent as it is drawn.
        let sent = |line: &str| line.to_string();
        let latest = o.find_quote("the plan", None, sent).unwrap();
        assert_eq!(
            at(&o, latest),
            ("the plan".into(), 0),
            "turn two, its first"
        );
        let first = o.find_quote("the plan", Some(0), sent).unwrap();
        assert_eq!(at(&o, first), ("é the plan, once".into(), 2), "in chars");
        assert_eq!(o.last_turn(), Some(1));
        let echo = o.find_quote("one", None, sent).unwrap();
        assert_eq!(at(&o, echo).0, "> one", "your words are the turn's too");
        // The latest turn's own request is no place, searching back
        // (review stop 14, A 6); named, it is the turn's first.
        let two = o.find_quote("two", None, sent).unwrap();
        assert_eq!(at(&o, two).0, "the plan, two", "not what asked");
        let named = o.find_quote("two", Some(1), sent).unwrap();
        assert_eq!(
            at(&o, named).0,
            "> two",
            "the turn's first, not its last block's"
        );
        assert_eq!(o.find_quote("never said", None, sent), None);
        assert_eq!(o.find_quote("two", Some(0), sent), None, "not in that turn");
        assert_eq!(o.find_quote("", None, sent), None);
    }

    /// **A quote never lands in a thought** (N1b): what is drawn there is
    /// the API's summary, not the model's words, and it stands before the
    /// reply — found first, it took the mark from the reply's line.
    #[test]
    fn a_quote_never_lands_in_a_thought() {
        let mut o = Output::new();
        o.begin_turn("ask");
        o.append_thought("chose BTreeMap for order", &[]);
        o.end_thought();
        o.append_chunk("Use a BTreeMap here.");
        o.end_turn();
        let at = o
            .find_quote("BTreeMap", None, str::to_string)
            .expect("found");
        let (line, _) = o.locate(at).unwrap();
        assert_eq!(o.lines()[line], "Use a BTreeMap here.");
        assert_eq!(o.find_quote("for order", None, str::to_string), None);
    }

    /// **A thought is the model's writing**: a held value put back in it is
    /// where `shown_at` says, as in a reply (review stop 6, F 5), so one it
    /// was not put back is drawn as the model's own.
    #[test]
    fn a_thought_is_the_models_writing() {
        let mut o = Output::new();
        o.begin_turn("ask");
        o.append_thought("the key hunter2 here", &[(8, 15)]);
        o.end_thought();
        let block = (o.spans().position(|s| s.role == BlockRole::Thinking)).expect("a thought");
        assert_eq!(o.shown_at((block, 0)), Some(vec![(8, 15)]));
    }

    /// **Every role's saved word reads back as that role** — over the type
    /// (`every_variant!`), so a role added without a word fails here.
    #[test]
    fn every_roles_word_reads_back() {
        for role in every_variant!(BlockRole: User, Assistant, Tool, Rule, Thinking) {
            assert_eq!(BlockRole::from_name(role.name()), Some(*role), "{role:?}");
        }
    }

    /// **A thought is put in order, as a record is**: after the reply's text
    /// (sealed, its blank line kept), or before an open block nothing was
    /// written into — and the turn ending after it leaves no empty block.
    #[test]
    fn a_thought_is_put_in_order_and_the_turn_ends_clean() {
        let mut o = Output::new();
        o.begin_turn("ask");
        o.append_thought("first", &[]);
        assert!(o.thinking(), "open while it streams");
        o.end_thought();
        assert!(!o.thinking());
        o.append_chunk("said");
        o.append_thought("then", &[]);
        o.end_thought();
        o.end_turn();
        let tail: Vec<(BlockRole, Vec<String>)> = (o.spans())
            .filter(|s| s.role.the_models())
            .map(|s| (s.role, s.lines.to_vec()))
            .collect();
        let lines = |of: &[&str]| -> Vec<String> { of.iter().map(|l| l.to_string()).collect() };
        assert_eq!(
            tail,
            [
                (BlockRole::Thinking, lines(&["first", ""])),
                (BlockRole::Assistant, lines(&["said", ""])),
                (BlockRole::Thinking, lines(&["then", ""])),
            ],
            "no empty block after the last thought"
        );
    }

    /// **A call arriving while a thought is open ends it first** — sealed
    /// with its separator, above the record, as text after a thought does.
    #[test]
    fn a_call_after_an_open_thought_ends_it() {
        let mut o = Output::new();
        o.begin_turn("ask");
        o.append_thought("hm", &[]);
        o.push_tool_activity("⚙ ran: ls", "a");
        assert!(!o.thinking(), "ended");
        let kinds: Vec<(BlockRole, Vec<String>)> = (o.spans())
            .filter(|s| matches!(s.role, BlockRole::Thinking | BlockRole::Tool))
            .map(|s| (s.role, s.lines.to_vec()))
            .collect();
        assert_eq!(
            kinds[0],
            (BlockRole::Thinking, vec!["hm".into(), String::new()])
        );
        assert_eq!(kinds[1].0, BlockRole::Tool, "{kinds:?}");
    }

    /// A transcript whose turn thought three lines, then replied.
    fn three_line_thought() -> (Output, usize) {
        let mut o = Output::new();
        o.begin_turn("ask");
        o.append_thought("first thought line, quite long indeed\nsecond\nthird", &[]);
        o.end_thought();
        o.append_chunk("reply");
        o.end_turn();
        let first = (o.lines().iter())
            .position(|l| l.starts_with("first thought"))
            .unwrap();
        (o, first)
    }

    /// **A shut block is one row** (N5a): its first line with text, cut to
    /// leave room for what the view draws around it; its separator kept; one
    /// row high on its first line and none on the rest, so every scroll
    /// reads it — a place inside it on its row, a click on its row its first
    /// line, and its lines what the navigation copy folds.
    #[test]
    fn a_shut_block_is_one_row() {
        let (o, first) = three_line_thought();
        let folds = Folds::default();
        let shut = folds.shut(FoldRule::SHIPPED);
        let (width, vp) = (30, Viewport::default());
        let open = o.visible_window(&vp, &Shut::NONE, width, 40);
        let window = o.visible_window(&vp, &shut, width, 40);
        let at = (window.folds.iter())
            .position(Option::is_some)
            .expect("a fold row");
        let fold = window.folds[at].unwrap();
        assert_eq!(
            fold,
            FoldRow {
                lines: 3,
                cut: true,
                titled: false,
                cut_off: false
            }
        );
        let text = &window.rows[at];
        assert!("first thought line, quite long indeed".starts_with(text.as_str()));
        let around =
            crate::wrap::str_cells(&FoldRow::lead()) + 1 + crate::wrap::str_cells(&fold.suffix());
        assert!(
            crate::wrap::str_cells(text) + around <= width as usize,
            "{text:?}"
        );
        assert_eq!(window.rows[at + 1], "", "its separator kept");
        assert_eq!(window.rows[at + 2], "reply");
        let hidden = (open.rows.iter().skip(at).take_while(|r| !r.is_empty())).count();
        assert_eq!(open.total_rows - window.total_rows, hidden - 1);
        assert_eq!(o.logical_to_visual(&shut, first + 2, 3, width), (at, 0));
        assert_eq!(o.visual_to_logical(&shut, at, 5, width), (first, 0));
        assert_eq!(o.shut_lines(&shut, Some(width)), vec![(first, first + 3)]);
        assert_eq!(o.shut_lines(&shut, None), vec![(first, first + 3)]);
        assert_eq!(
            o.shut_lines(&Shut::NONE, Some(width)),
            Vec::new(),
            "open, none"
        );
    }

    /// **`zt` on a line inside a shut fold puts the fold's row at the top** —
    /// the line has no row of its own, so it is placed as the fold's first.
    #[test]
    fn zt_inside_a_shut_fold_puts_its_row_at_the_top() {
        let (o, first) = three_line_thought();
        let folds = Folds::default();
        let shut = folds.shut(FoldRule::SHIPPED);
        let mut vp = Viewport::default();
        let fold_row = o.logical_to_visual(&shut, first, 0, 30).0;
        assert!(fold_row > 0, "premise: rows above it to scroll past");
        for line in [first, first + 1, first + 2] {
            let top = crate::viewport::Place::Top;
            o.place(&mut vp, &shut, 30, 3, line, top, 0);
            let window = o.visible_window(&vp, &shut, 30, 3);
            assert_eq!(window.top, fold_row, "line {line}");
            assert!(window.folds[0].is_some(), "the fold's row first");
        }
    }

    /// **A block whose content is one row cannot shut** — shutting it would
    /// hide nothing (vim's `foldminlines`, counted in screen lines, measured
    /// in Neovim 0.12) — and **one long line, wrapped, is drawn shut and
    /// folded, that one line**: the keys take it as the screen shows it, `w`
    /// past it rather than into it (review stop 15, A L9: the keys' copy
    /// counted lines and folded none). Not drawn yet, a window has no width,
    /// and the rule with none: two lines or more.
    #[test]
    fn what_takes_one_row_does_not_shut_and_one_long_line_does() {
        let mut o = Output::new();
        o.begin_turn("ask");
        o.append_thought("short", &[]);
        o.end_thought();
        o.append_thought(&"long ".repeat(20), &[]);
        o.end_thought();
        o.end_turn();
        let folds = Folds::default();
        let shut = folds.shut(FoldRule::SHIPPED);
        let window = o.visible_window(&Viewport::default(), &shut, 30, 40);
        let shut_rows: Vec<&String> = (window.rows.iter().zip(&window.folds))
            .filter_map(|(row, fold)| fold.map(|_| row))
            .collect();
        assert_eq!(shut_rows.len(), 1, "the long one only: {:?}", window.rows);
        assert!(shut_rows[0].starts_with("long long"));
        assert!(window.rows.iter().any(|r| r == "short"), "drawn open");
        let long = (o.lines().iter())
            .position(|l| l.starts_with("long"))
            .expect("premise");
        assert_eq!(o.shut_lines(&shut, Some(30)), vec![(long, long + 1)]);
        assert_eq!(o.shut_lines(&shut, Some(200)), Vec::new(), "one row wide");
        assert_eq!(o.shut_lines(&shut, None), Vec::new());
    }

    /// **A window's folds** — set per block over the rule, `zR`/`zM` over
    /// every fold there is: a kind's by rule, those to come too, and one
    /// made by hand on a reply, which the shipped rule gives none (Steven,
    /// 11-03) — and `NONE` folds nothing.
    #[test]
    fn a_windows_folds_set_over_the_rule() {
        use shannon_core::config::Folded;
        let (o, _) = three_line_thought();
        let thought = (o.spans().find(|s| s.role == BlockRole::Thinking)).unwrap();
        let reply = (o.spans().find(|s| s.role == BlockRole::Assistant)).unwrap();
        let mut folds = Folds::default();
        let block = |span: &Span<'_>| o.blocks.iter().find(|b| b.id == span.id).unwrap();
        let rule = FoldRule::SHIPPED;
        let held = |folds: &Folds, span: &Span<'_>| folds.shut(rule).holds(block(span));
        assert!(held(&folds, &thought), "the rule: thinking shut");
        assert!(!held(&folds, &reply), "the rule: a reply open");
        assert!(!folds.has_fold(reply.id, reply.role, rule), "and no fold");
        folds.set(thought.id, thought.role, true);
        assert!(!held(&folds, &thought), "set open");
        folds.set_all(false, rule);
        assert!(held(&folds, &thought), "zM: the thought shut");
        assert!(
            !held(&folds, &reply),
            "zM: a reply with no fold stays drawn"
        );
        folds.set(reply.id, reply.role, false);
        assert!(held(&folds, &reply), "zc made its fold, shut");
        assert!(folds.has_fold(reply.id, reply.role, rule));
        folds.set_all(true, rule);
        assert!(!held(&folds, &thought) && !held(&folds, &reply), "zR");
        folds.set_all(false, rule);
        assert!(held(&folds, &reply), "zM again: the fold made is kept");
        assert!(!Shut::NONE.holds(block(&thought)));

        let replies = FoldRule {
            reply: FoldStart::Fixed(Folded::Open),
            ..rule
        };
        let mut ruled = Folds::default();
        assert!(!ruled.shut(replies).holds(block(&reply)), "starts open");
        ruled.set_all(false, replies);
        assert!(
            ruled.shut(replies).holds(block(&reply)),
            "reply = \"open\" is a fold: zM shuts it"
        );
    }

    /// **A kind put back to its rule is that kind alone** — its `zM` and
    /// what was set on a block of it forgotten, every other kind's kept
    /// (Steven, 11-01: `:set tui.fold.thinking=open` after `zM` opened the
    /// replies too).
    #[test]
    fn a_kind_put_back_to_its_rule_leaves_the_others() {
        let (o, _) = three_line_thought();
        let thought = (o.spans().find(|s| s.role == BlockRole::Thinking)).unwrap();
        let reply = (o.spans().find(|s| s.role == BlockRole::Assistant)).unwrap();
        let block = |span: &Span<'_>| o.blocks.iter().find(|b| b.id == span.id).unwrap();
        let open = FoldRule {
            thinking: FoldStart::Fixed(shannon_core::config::Folded::Open),
            ..FoldRule::SHIPPED
        };
        let held = |folds: &Folds, span: &Span<'_>| folds.shut(open).holds(block(span));
        let mut folds = Folds::default();
        folds.set(reply.id, reply.role, true);
        folds.set_all(false, open);
        folds.to_rule(&[BlockRole::Thinking]);
        assert!(!held(&folds, &thought), "zM's, forgotten: the rule");
        assert!(held(&folds, &reply), "a reply keeps zM's");
        folds.set(thought.id, thought.role, false);
        folds.set(reply.id, reply.role, false);
        folds.to_rule(&[BlockRole::Thinking]);
        assert!(!held(&folds, &thought), "set by hand, forgotten: the rule");
        assert!(held(&folds, &reply), "a reply keeps what was set");
    }

    /// **Which kinds a change hands back** — those whose rule differs, or
    /// whose key a `:set` names (a key inside it, `tui.fold` whole); and the
    /// keys [`BlockRole::fold_rule`] names are the `[tui.fold]` table's own,
    /// every one named once, so a `:set` of one is never missed.
    #[test]
    fn the_kinds_a_rule_change_names() {
        use shannon_core::config::{Folded, ShutOver};
        let thinking_open = FoldRule {
            thinking: FoldStart::Fixed(Folded::Open),
            ..FoldRule::SHIPPED
        };
        let tool_over = FoldRule {
            tool: FoldStart::Over(ShutOver { shut_over: 3 }),
            ..thinking_open
        };
        let same = ruled_otherwise(FoldRule::SHIPPED, FoldRule::SHIPPED);
        assert_eq!(same, Vec::new());
        let thinking = ruled_otherwise(FoldRule::SHIPPED, thinking_open);
        assert_eq!(thinking, vec![BlockRole::Thinking]);
        assert_eq!(
            ruled_otherwise(thinking_open, tool_over),
            vec![BlockRole::Tool]
        );
        let keys =
            |keys: &[&str]| ruled_by_keys(&keys.iter().map(|k| k.to_string()).collect::<Vec<_>>());
        assert_eq!(keys(&["tui.fold.thinking"]), vec![BlockRole::Thinking]);
        let over = keys(&["tui.fold.tool.shut_over"]);
        assert_eq!(over, vec![BlockRole::Tool]);
        let whole = keys(&["tui.fold"]);
        let ruled = [BlockRole::User, BlockRole::Assistant, BlockRole::Tool];
        assert_eq!(whole, [&ruled[..], &[BlockRole::Thinking]].concat());
        assert_eq!(keys(&["tui.fold.reply"]), vec![BlockRole::Assistant]);
        assert_eq!(keys(&["tui.foldx", "tui.scrolloff"]), Vec::new());

        let table = shannon_core::config::Config::default().as_table();
        let fold = &table["tui"]["fold"];
        let mut theirs: Vec<String> = (fold.as_table().unwrap().keys())
            .map(|key| format!("tui.fold.{key}"))
            .collect();
        let mut ours: Vec<String> = (BlockRole::ALL.iter())
            .filter_map(|role| role.fold_rule().map(|kind| kind.key.to_string()))
            .collect();
        theirs.sort();
        ours.sort();
        assert_eq!(ours, theirs, "one role per key of [tui.fold]");
    }

    /// **`{ shut_over = N }` shuts what holds more than N lines** — the
    /// count its shut row would show; `"open"` shuts nothing of its kind,
    /// and a kind's rule is that kind's alone.
    #[test]
    fn a_kind_shut_over_n_shuts_only_what_holds_more() {
        use shannon_core::config::{FoldStart, Folded, ShutOver};
        let (mut o, _) = three_line_thought();
        o.begin_turn("run");
        o.push_tool_record(running("ls", "c1"));
        o.end_turn();
        let thought = (o.blocks.iter().find(|b| b.role == BlockRole::Thinking)).unwrap();
        let record = (o.blocks.iter().find(|b| b.role == BlockRole::Tool)).unwrap();
        assert_eq!(thought.content(), 3, "premise: three lines");
        let folds = Folds::default();
        let held = |thinking: FoldStart| {
            let rule = FoldRule {
                thinking,
                ..FoldRule::SHIPPED
            };
            folds.shut(rule).holds(thought)
        };
        let over = |shut_over| FoldStart::Over(ShutOver { shut_over });
        assert!(!held(over(3)), "three lines, not more than three: open");
        assert!(held(over(2)), "more than two: shut");
        assert!(!held(FoldStart::Fixed(Folded::Open)), "open");
        let tools_open = FoldRule {
            tool: FoldStart::Fixed(Folded::Open),
            ..FoldRule::SHIPPED
        };
        assert!(
            folds.shut(tools_open).holds(thought),
            "the tool's rule is not thinking's"
        );
        assert!(
            !folds.shut(tools_open).holds(record),
            "the tool's is the record's"
        );
        let thinking_open = FoldRule {
            thinking: FoldStart::Fixed(Folded::Open),
            ..FoldRule::SHIPPED
        };
        assert!(
            folds.shut(thinking_open).holds(record),
            "and thinking's not it"
        );
    }

    /// **A window's place is kept by position and found again** in a
    /// transcript rebuilt from the turns a map keeps (S21-Q4) — the cursor
    /// on its text at its column, a fold on its block, `zR`/`zM`'s state;
    /// what is on a turn the map drops is not found.
    #[test]
    fn a_view_is_kept_by_position_and_found_again() {
        // Turns: 0 "zero"; 1 a thought, then "one"; 2 "two".
        let turns = |o: &mut Output, from: usize| {
            for turn in from..3 {
                o.begin_turn(&format!("ask {turn}"));
                if turn == 1 {
                    o.append_thought("weigh\nit\nup", &[]);
                    o.end_thought();
                }
                o.append_chunk(["zero", "one", "two"][turn]);
                o.end_turn();
            }
        };
        let mut was = Output::new();
        turns(&mut was, 0);
        let line = |o: &Output, text: &str| o.lines().iter().position(|l| l == text).unwrap();
        let thought = |o: &Output| {
            (o.spans().find(|s| s.role == BlockRole::Thinking))
                .unwrap()
                .id
        };
        // `zM`, thinking put back to its rule, then the thought opened.
        let mut folds = Folds::default();
        folds.set_all(false, FoldRule::SHIPPED);
        folds.to_rule(&[BlockRole::Thinking]);
        folds.set(thought(&was), BlockRole::Thinking, true);
        let cursor = (line(&was, "one"), 2);
        let top = Some((line(&was, "zero"), 0));
        // The new session starts at the old turn 1.
        let kept = was.keep_view(cursor, top, &folds, &|turn| turn.checked_sub(1));

        let mut now = Output::new();
        turns(&mut now, 1);
        let found = now.find_view(&kept);
        assert_eq!(
            found.cursor,
            Some((line(&now, "one"), 2)),
            "{:?}",
            now.lines()
        );
        assert_ne!(line(&now, "one"), cursor.0, "premise: it moved");
        assert_eq!(found.top, None, "turn 0 is not kept");
        let set = found.folds.set.get(&thought(&now));
        assert_eq!(set, Some(&(BlockRole::Thinking, true)));
        assert_eq!(found.folds.all, folds.all, "zM's, kind by kind");
    }

    /// **One blank row after a thought, as after any block** — the two
    /// newlines the API ends a thought's text with are not drawn (Steven's
    /// screens, 11-01: three blank rows before every reply); a cut note
    /// stands right under its text.
    #[test]
    fn a_thought_ends_on_its_text() {
        let mut o = Output::new();
        o.begin_turn("ask");
        o.append_thought("weigh it\n\n", &[]);
        o.end_thought();
        o.append_chunk("so");
        let lines = o.lines();
        let at = |text: &str| lines.iter().position(|l| l == text).unwrap();
        assert_eq!(at("so") - at("weigh it"), 2, "one blank row: {lines:#?}");

        let mut o = Output::new();
        o.begin_turn("ask");
        o.append_thought("weigh it\n\n", &[]);
        o.cut_thought();
        let lines = o.lines();
        let at = |text: &str| lines.iter().position(|l| l.starts_with(text)).unwrap();
        assert_eq!(at("[cut off") - at("weigh it"), 1, "{lines:#?}");
    }

    /// **A turn ending on a thought still open cuts it** — said, and drawn
    /// this run only — and a thought that drew nothing but blank lines goes.
    #[test]
    fn a_turn_ending_on_an_open_thought_cuts_it() {
        let mut o = Output::new();
        o.begin_turn("ask");
        o.append_thought("\n", &[]);
        o.end_thought();
        assert!(
            o.spans().all(|s| s.role != BlockRole::Thinking),
            "a blank thought goes"
        );
        o.append_chunk("said");
        o.append_thought("halfway", &[]);
        o.end_turn();
        let cut = (o.spans().find(|s| s.role == BlockRole::Thinking)).expect("kept on screen");
        assert_eq!(cut.lines[0], "halfway");
        assert!(cut.lines[1].starts_with("[cut off"), "{:?}", cut.lines);
        assert!(cut.live_only, "drawn this run only");
    }

    /// **A quote is matched against the line as the model was sent it**
    /// (review stop 14, B F1): a held value drawn where the model wrote its
    /// handle is no text to it — its prefix is found nowhere, as text never
    /// written; the handle is found, on its line's start, since the drawn
    /// columns are not the ones it quoted.
    #[test]
    fn a_quote_is_matched_as_the_line_was_sent() {
        let mut o = Output::new();
        o.begin_turn("go");
        o.append_chunk("export TOKEN=hunter2\nplain line");
        o.end_turn();
        let sent = |line: &str| line.replace("hunter2", "{{secret.t}}");
        assert_eq!(o.find_quote("TOKEN=hun", None, sent), None, "a prefix");
        assert_eq!(o.find_quote("hunter2", None, sent), None, "the value");
        let at = o.find_quote("TOKEN={{secret.t}}", None, sent).unwrap();
        let (line, col) = o.locate(at).unwrap();
        assert_eq!((o.lines()[line].as_str(), col), ("export TOKEN=hunter2", 0));
        let plain = o.find_quote("line", None, sent).unwrap();
        assert_eq!(
            o.locate(plain).unwrap().1,
            6,
            "a line with none: its column"
        );
    }

    /// **A place on a line a transcript does not have is not there** — one
    /// copied before the line arrived (`Output` is `Clone`) holds the block,
    /// shorter: the line past its end would name the next block's.
    #[test]
    fn a_place_past_its_blocks_end_is_not_there() {
        let mut o = Output::new();
        o.begin_turn("p");
        o.append_chunk("one");
        let before = o.clone();
        o.append_chunk("\ntwo");
        o.push_tool_activity("$ ls", "a.txt");
        let two = o.anchor_at(o.lines().iter().position(|l| l == "two").unwrap(), 0);
        let two = two.unwrap();
        assert!(
            before.spans().any(|s| s.id == two.block),
            "premise: the copy holds the block"
        );
        assert_eq!(before.locate(two), None);
    }

    /// A call's `⚙ running:` line, as `App::record_started` writes it.
    fn running(command: &str, call: &str) -> crate::tool_record::Record {
        let input = serde_json::json!({ "command": command });
        crate::tool_record::Record::started("running", shannon_core::BASH_TOOL, &input)
            .of_call(call)
    }

    /// The record closing `call`, as `App::record_tool_command` writes it.
    fn ran(command: &str, call: &str, output: &str) -> crate::tool_record::Record {
        let mut record = crate::tool_record::Record::command("ran", command, None).of_call(call);
        record.output(output);
        record
    }

    /// A turn that started two commands and heard from the second, as the
    /// live view draws it: the first's running line, then the second's
    /// record in its own running line's place.
    fn ran_two() -> Output {
        let mut o = Output::new();
        o.begin_turn("run both");
        o.append_chunk("first");
        o.push_tool_record(running("ls one", "c1"));
        o.push_tool_record(running("ls two", "c2"));
        o.push_tool_record(ran("ls two", "c2", "b.txt"));
        o.append_chunk("done");
        o.end_turn();
        o
    }

    /// **A kept place names its block again** — every place in a turn that
    /// ran commands, there and back through a [`Position`], counting only
    /// the blocks a reopened turn draws (review stop 14, A 1): counted, a
    /// running line put the place one record on, onto the next command's.
    /// A running line itself is not kept, and says why; once its call
    /// closes, its record stands where it stood, each record the one a
    /// reopen draws (review stop 15, F H2).
    #[test]
    fn a_kept_place_names_its_block_again() {
        let mut o = ran_two();
        let lines = o.lines();
        let running: Vec<BlockId> = (o.spans()).filter(|s| s.live_only).map(|s| s.id).collect();
        assert_eq!(running.len(), 1, "premise: the first still running");
        for (line, text) in lines.iter().enumerate() {
            let anchor = o.anchor_at(line, 1).unwrap();
            match o.position_of(anchor) {
                Ok(position) => {
                    assert_eq!(o.anchor_of(position), Some(anchor), "line {line}: {text:?}");
                }
                Err(why) => {
                    assert!(running.contains(&anchor.block), "{line}: {text:?}");
                    assert_eq!(why, Unkept::LiveOnly);
                }
            }
        }
        o.push_tool_record(ran("ls one", "c1", "a.txt"));
        assert!(!(o.spans()).any(|s| s.live_only), "both closed");
        let lines = o.lines();
        let at = |text: &str| lines.iter().position(|l| l == text).unwrap();
        assert!(at("  a.txt") < at("  b.txt"), "in the order they started");
        let p = o
            .position_of(o.anchor_at(at("  b.txt"), 0).unwrap())
            .unwrap();
        assert_eq!((p.role, p.nth), (BlockRole::Tool, 1), "the second record");
    }

    /// **The record closing a call takes its running line's place** (review
    /// stop 15, F H2) — its own call's, whichever closes first; the block
    /// kept (its id, its turn), so what was set on the line stays on the
    /// call; and what streamed below it meanwhile left as it was, a thought
    /// still open. A record of a call that never ran is put in order.
    #[test]
    fn a_record_closing_a_call_takes_its_running_lines_place() {
        let mut o = Output::new();
        o.begin_turn("run both");
        o.push_tool_record(running("ls one", "c1"));
        o.push_tool_record(running("ls two", "c2"));
        o.append_thought("still thinking", &[]);
        let ids = |o: &Output| (o.spans()).map(|s| (s.id, s.turn)).collect::<Vec<_>>();
        let before = ids(&o);
        o.push_tool_record(ran("ls two", "c2", "b.txt"));
        assert_eq!(ids(&o), before, "no block made or moved");
        let lines = o.lines();
        let at = |text: &str| lines.iter().position(|l| l.starts_with(text));
        assert!(at("⚙ running: ls one").is_some(), "the other runs on");
        assert_eq!(at("⚙ running: ls two"), None, "its own line, replaced");
        assert!(at("⚙ running: ls one") < at("⚙ ran:"), "{lines:#?}");
        assert!(o.open_thought().is_some(), "the thought below runs on");
        // A second record of a call already closed takes nothing: only a
        // running line is a place to take.
        o.push_tool_record(ran("ls two", "c2", "again"));
        assert_eq!(o.spans().count(), before.len() + 1, "one record more");
        assert!(o.lines().iter().any(|l| l == "  b.txt"), "the first kept");
        assert!(
            o.open_thought().is_none(),
            "put in order, it ends the thought"
        );
        // Refused before it ran: no running line to take, so in order.
        o.push_tool_record(
            crate::tool_record::Record::command("DENIED", "rm x", None).of_call("c3"),
        );
        assert_eq!(o.spans().count(), before.len() + 2, "and one more");
    }

    /// **A typed record waits for the block the reply is writing** (Steven,
    /// 11-01: "the old brass h", the record, "and lamp") — its running line
    /// and the record closing it never drawn mid-block, the reply's words
    /// whole; in below them when the turn ends. Where nothing is being
    /// written, it goes in at once.
    #[test]
    fn a_typed_record_waits_for_the_reply_block_to_end() {
        let mut o = Output::new();
        o.begin_turn("a story");
        o.append_chunk("the old brass h");
        assert!(o.push_typed_record(running("sleep 3", "t1")), "it waits");
        o.append_chunk("and lamp");
        let closing = ran("sleep 3", "t1", "slept");
        assert!(!o.push_typed_record(closing), "its line's place, waiting");
        let lines = o.lines();
        assert!(
            lines.contains(&"the old brass hand lamp".to_string()),
            "{lines:#?}"
        );
        assert!(!lines.iter().any(|l| l.contains("sleep 3")), "not in yet");
        o.end_turn();
        let lines = o.lines();
        let at = |text: &str| lines.iter().position(|l| l.contains(text));
        assert!(at("hand lamp") < at("⚙ ran:"), "{lines:#?}");
        assert_eq!(at("⚙ running:"), None, "its running line, replaced");
        assert_eq!(o.spans().filter(|s| s.role == BlockRole::Tool).count(), 1);

        // Nothing being written: in at once.
        let mut o = Output::new();
        o.begin_turn("run");
        assert!(!o.push_typed_record(running("sleep 3", "t1")), "no wait");
        assert!(
            o.lines()
                .iter()
                .any(|l| l.starts_with("⚙ running: sleep 3"))
        );
    }

    /// **What waits goes in ahead of the model's next block** — it came
    /// while the block that block ends was written; and a thought being
    /// written is waited out, the thought one block, the record in when it
    /// ends.
    #[test]
    fn a_typed_record_goes_in_before_the_models_next_block() {
        let mut o = Output::new();
        o.begin_turn("run");
        o.append_chunk("let me check");
        assert!(o.push_typed_record(running("sleep 3", "t1")));
        o.push_tool_record(running("ls", "c1"));
        let lines = o.lines();
        let at = |text: &str| lines.iter().position(|l| l.starts_with(text));
        assert!(at("let me check") < at("⚙ running: sleep 3"));
        assert!(at("⚙ running: sleep 3") < at("⚙ running: ls"), "{lines:#?}");

        let mut o = Output::new();
        o.begin_turn("think");
        o.append_thought("weigh", &[]);
        assert!(o.push_typed_record(running("sleep 3", "t1")), "it waits");
        o.append_thought("ing it", &[]);
        assert!(
            o.lines().contains(&"weighing it".to_string()),
            "one thought"
        );
        o.append_chunk("so");
        let lines = o.lines();
        let at = |text: &str| lines.iter().position(|l| l.starts_with(text));
        assert!(at("weighing it") < at("⚙ running: sleep 3"), "{lines:#?}");
        assert!(at("⚙ running: sleep 3") < at("so"), "{lines:#?}");
    }

    /// **A turn dropped while a typed record waits keeps it** — a command
    /// that ran: put in as the turn ends, drawn with the turn as dropped.
    /// Truncated with the turn, it was left waiting for one that is gone,
    /// never drawn.
    #[test]
    fn a_dropped_turn_keeps_the_typed_record_waiting() {
        let mut o = Output::new();
        o.begin_turn("think");
        o.append_thought("weigh", &[]);
        assert!(o.push_typed_record(ran("rm x", "t1", "removed")));
        assert!(!o.abandon_turn("stopped"), "kept, not removed");
        let record = o.spans().find(|s| s.role == BlockRole::Tool);
        assert!(record.is_some_and(|s| s.dropped), "in, as dropped");
    }

    /// **A quote is found where it is kept**, never on a running line: the
    /// record closing the call holds the same command, in its place.
    #[test]
    fn a_quote_is_never_found_on_a_running_line() {
        let o = ran_two();
        let at = o.find_quote("ls two", None, str::to_string).expect("found");
        let span = o.spans().find(|s| s.id == at.block).unwrap();
        assert!(!span.live_only, "on {:?}", span.lines);
        assert_eq!(span.lines[at.line], "  ls two");
        assert_eq!(
            o.find_quote("ls one", None, str::to_string),
            None,
            "only on a line still running"
        );
    }

    /// **A fold is asked what it holds once per change** (`fold_holds`) —
    /// again only when its lines change or the key does; a record, not the
    /// model's text, is never asked. Counted, not timed: deleting the
    /// memo leaves every answer right, and every frame asking every line.
    #[test]
    fn a_fold_is_asked_what_it_holds_once_per_change() {
        let mut o = Output::new();
        o.begin_turn("look");
        o.append_thought("one\ntwo\nthree", &[]);
        let at = (o.spans())
            .position(|s| s.role == BlockRole::Thinking)
            .unwrap();
        let asked = Cell::new(0);
        let ask = |o: &Output, key| {
            o.fold_holds(at, key, |_, _| {
                asked.set(asked.get() + 1);
                false
            })
        };
        assert!(!ask(&o, 1));
        assert_eq!(asked.get(), 3, "premise: each line asked");
        ask(&o, 1);
        assert_eq!(asked.get(), 3, "the same lines and key: remembered");
        ask(&o, 2);
        assert_eq!(asked.get(), 6, "another key: asked again");
        o.append_thought("\nfour", &[]);
        ask(&o, 2);
        assert_eq!(asked.get(), 10, "its lines changed: asked again");
        o.push_tool_record(ran("ls", "c1", "a.txt"));
        let record = (o.spans()).position(|s| s.role == BlockRole::Tool).unwrap();
        assert!(
            !o.fold_holds(record, 2, |_, _| true),
            "not the model's text"
        );
    }

    /// **A turn dropped after it ran something is kept, as no turn's** —
    /// ended as a turn ends, nothing of it numbered, and the next turn
    /// numbered as the dropped one was (review stop 14, A 2): the
    /// conversation has no such turn. One that only said it failed, too.
    #[test]
    fn a_dropped_turn_that_ran_or_failed_is_kept_as_no_turns() {
        for kept in ["record", "failure"] {
            let mut o = Output::new();
            o.begin_turn("one");
            o.append_chunk("reply");
            o.end_turn();
            o.begin_turn("two");
            match kept {
                "record" => o.push_tool_activity("⚙ cancelled: ls", ""),
                _ => o.append_chunk("\n[error: rate limited]"),
            }
            let why = match kept {
                "record" => "you stopped it before its round finished",
                _ => "the request failed",
            };
            // Where the turn begins: the rule above your words.
            let first = *o.sections().0.last().expect("two turns");
            assert!(!o.abandon_turn(why), "{kept}: kept, not removed");
            assert!(
                (first..o.line_count()).all(|l| o.turn_of_line(l).is_none()),
                "{kept}: none of it numbered"
            );
            assert!(o.blocks.iter().all(|b| b.sealed), "{kept}: ended");
            // Said, and why — its last line of text — and all of it painted
            // apart, a place in it said not kept for that reason.
            let lines = o.lines();
            let said = lines.iter().rev().find(|l| !l.is_empty()).unwrap();
            assert_eq!(*said, format!("[not in the conversation: {why}]"), "{kept}");
            let spans: Vec<Span<'_>> = o.spans().filter(|s| s.first >= first).collect();
            assert!(spans.iter().all(|s| s.dropped), "{kept}: all of it apart");
            assert!(
                o.spans().filter(|s| s.first < first).all(|s| !s.dropped),
                "{kept}: and nothing before it"
            );
            let echo = o.anchor_at(lines.iter().position(|l| l == "> two").unwrap(), 0);
            assert_eq!(
                o.position_of(echo.unwrap()),
                Err(Unkept::NotInConversation),
                "{kept}"
            );
            assert_eq!(o.last_turn(), Some(0), "{kept}");
            o.begin_turn("three");
            let three = o.lines().iter().position(|l| l == "> three").unwrap();
            assert_eq!(o.turn_of_line(three), Some(1), "{kept}: the next is turn 1");
        }
    }
}
