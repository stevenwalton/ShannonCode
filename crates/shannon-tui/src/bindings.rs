//! The keymap **atom table** — the data layer of the modal keymap.
//!
//! The engine (`vim_keymap.rs`) owns the *grammar* — how `[count][operator]
//! [count][motion|text-object]` composes, the chord state machine, which
//! atom kinds are legal in which context. This module owns the *atoms*:
//! **which key is which action**. That split is the M2 "customization as
//! substrate" thesis — vim's grammar can't be a flat table (operator-pending
//! is recursive), but the leaf "key `w` means word-forward" *is* data, and
//! making it data is what lets a user remap a key (Phase C) without touching
//! the engine.
//!
//! The defaults here are **canonical vim** (the vim-fidelity bar): the table
//! makes them *remappable*, it does not make them a compromise. A user's
//! `config.toml` will later be a sparse diff over [`DEFAULTS`].
//!
//! Phase A routed the engine's pane leaves through this table (the motion
//! resolver, the Normal/Visual commands, the text-object key); Phase B added
//! the `<C-w>` window-command contexts. Phase C (the config layer) adds the
//! stable atom *names* (`ATOM_NAMES`), the chord/context string parsers, and
//! [`Bindings::with_overrides`], which merges a user's sparse `[keymap.*]` diff
//! onto [`DEFAULTS`] into the process-global [`active`] table. (See
//! `DesignDocs/keymap-atom-table.md`.)

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, OnceLock};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::keymap::{
    BlockEdge, CaseOp, CursorMotion, Direction, EditAction, InsertEntry, Operator, ScrollCmd,
    ScrollMagnitude::{HalfPage, Page},
    SearchCmd, SelectionKind, TextObject, WindowCmd,
};

/// A multi-key **prefix** namespace — a key that opens a sub-context needing
/// one more key to resolve: the motion prefixes `g` (`gg`), `[` and `]`
/// (`[[` `]]`), and the command prefixes `<C-w>` and `z`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixId {
    /// `g` — the goto/`gg` prefix (motion-resolving).
    G,
    /// `[` — vim's backward family (`[[`), motion-resolving: its first
    /// key is the direction (`:h ]]`).
    OpenBracket,
    /// `]` — the forward family (`]]`).
    CloseBracket,
    /// `<C-w>` — the window-command prefix (command-resolving).
    Window,
    /// `z` — the scroll-relative-to-cursor prefix (`zt` `zz` `zb`),
    /// command-resolving like `Window`. The fold commands (`zo`, `zc`, …) will
    /// live here too; Steven is planning them separately.
    Z,
    /// `'` — your mark's place (`'a`), motion-resolving. Completed by the
    /// mark's name, a letter: vim's fixed rule, not a binding, as a count's
    /// digits are (`vim_keymap::resolve_motion`).
    Mark,
    /// `` ` `` — the agent's mark's place (`` `a ``), as `'` is yours.
    AgentMark,
    /// `f` `F` `t` `T` — a find along the line, motion-resolving: completed
    /// by the char to find, any char (`vim_keymap::resolve_motion`), as a
    /// mark is by its name.
    Find { till: bool, forward: bool },
}

/// A resolved binding target. The engine knows how each kind *composes* and
/// where it's legal; the table only says key → atom. The grammar-composing
/// kinds (`Motion`, `Prefix`) are looked up in the motion contexts; the rest
/// are terminal commands the engine's `execute_atom` turns into a
/// [`KeyOutcome`](crate::keymap::KeyOutcome) plus its mode/pending side effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Atom {
    // --- the motion path (Context::Motion / AfterG) ---
    /// A cursor motion / operator target (`w`, `$`, `gg`, …).
    Motion(CursorMotion),
    /// A prefix key that needs a completion (`g`).
    Prefix(PrefixId),

    // --- Normal commands (Context::Normal) ---
    /// A buffer edit applied `count` times (`p`/`P`/`u`/`Ctrl-R`).
    Edit(EditAction),
    /// Enter Insert via one of the six entries (`i`/`a`/`A`/`I`/`o`/`O`).
    Enter(InsertEntry),
    /// Enter Visual, anchoring a selection of this geometry (`v`/`V`/`Ctrl-V`).
    EnterVisual(SelectionKind),
    /// Open operator-pending (`d`/`c`/`y`).
    BeginOperator(Operator),
    /// `x` — delete the char under the cursor (`dl`, count-repeatable).
    DeleteCharAfter,
    /// `X` — delete the char before the cursor (`dh`).
    DeleteCharBefore,
    /// `Y` — linewise yank of the line (`yy`).
    YankLine,
    /// `D` — delete to end-of-line plus `count - 1` rows below (`d$`).
    DeleteToEol,
    /// `C` — change to end-of-line plus `count - 1` rows below (`c$`).
    ChangeToEol,
    /// `~` — toggle the case of `count` chars and step past them.
    ToggleCase,
    /// `r` — replace chars with the next key typed: `count` of them from the
    /// cursor in Normal, the whole selection in Visual.
    ReplaceChar,
    /// `R` — enter Replace mode.
    EnterReplace,
    /// `m` — set a mark where the cursor is, named by the next key, a letter
    /// (`ma`). Reads nothing and writes no text: vim sets one in a
    /// `nomodifiable` buffer (measured, `marks.md` §2).
    SetMark,
    /// Scroll the focused pane (`Ctrl-U`/`Ctrl-D`).
    Scroll(ScrollCmd),
    /// `zo` `zc` `za` `zR` `zM` — open, shut or flip the fold at the cursor,
    /// or every fold (`fold::FoldVerb`, N5a).
    Fold(crate::fold::FoldVerb),
    /// `:` — open the command line (`:q`, `:Edit`, `:config dump`).
    EnterCommand,
    /// `/` `?` `n` `N` `*` `#` — search the focused pane's text.
    Search(SearchCmd),
    /// Submit the prompt as a turn (`Enter` in Normal; `Ctrl+Enter` anywhere).
    Submit,

    // --- Visual commands (Context::Visual) ---
    /// `Esc` — leave Visual, dropping the selection.
    LeaveVisual,
    /// `v`/`V`/`Ctrl-V` in Visual — flip the selection geometry, or toggle
    /// Visual off if already this kind.
    SwitchVisual(SelectionKind),
    /// `d`/`x`/`c`/`s`/`y` — operate on the selection.
    VisualOperator(Operator),
    /// `I`/`A` — block-insert at the left/right edge (VisualBlock only).
    BlockInsert(BlockEdge),
    /// `o` — swap the selection's ends (cursor ↔ anchor), to extend from
    /// the other side.
    SwapSelectionEnds,

    // --- window commands (Context::AfterWindow) ---
    /// A window-management command after `<C-w>` (`<C-w>w`, `<C-w>j`, …). The
    /// engine emits it as `KeyOutcome::Window`; the app applies it.
    Window(WindowCmd),

    // --- text objects (Context::TextObject) ---
    /// The object key after `{operator}{i,a}` (`w`, `"`, `(`, …).
    Object(TextObject),
}

/// The engine state a key is looked up *in* — what's legal and what a key
/// means both depend on it. (The engine owns the transitions between
/// contexts; the table owns key → atom *within* one.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    /// Root cursor-motion keys — shared by direct cursor moves (`3j`) and
    /// operator targets (`d3w`), so the two can never diverge.
    Motion,
    /// After a bare `g`: completes the `g` prefix (`gg`).
    AfterG,
    /// After a bare `[`: completes it (`[[`).
    AfterOpenBracket,
    /// After a bare `]`: completes it (`]]`).
    AfterCloseBracket,
    /// Normal-mode non-motion commands (reached once the motion resolver has
    /// declined the key).
    Normal,
    /// Visual-mode non-motion commands (shared by `v`/`V`/`Ctrl-V`; the active
    /// geometry is the keymap's mode).
    Visual,
    /// Insert-mode *control* chords (not printable text): the rebindable submit
    /// key (`Ctrl+Enter`) lives here. Printable keys never reach the table —
    /// they're typed — so this context holds only the few non-text actions.
    Insert,
    /// After `<C-w>`: the window-command sub-context (`<C-w>w`, `<C-w>j`, …).
    AfterWindow,
    /// After `z`: where the cursor's line goes in the window (`zt`, `zz`, …).
    AfterZ,
    /// The object key of a text-object chord (`{op}{i,a}` → here).
    TextObject,
}

/// A canonical key chord the table is keyed by. `ctrl` distinguishes a
/// `CONTROL` chord (`<C-v>`) from a plain/shifted one — Shift only produces
/// the glyph (`$`, `G`), so it folds into the plain class (matching the
/// engine's `plain_or_shifted` predicate). Alt/Super combos canonicalize to
/// no chord (no binding uses them in these tables).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub code: KeyCode,
    pub ctrl: bool,
}

/// Canonicalize a raw key event to the chord the table is keyed by, or `None`
/// for a modifier combo no binding uses (Alt/Super). A control chord folds in
/// any Shift; a plain/shifted chord is the "command key, not a control chord"
/// class.
pub fn chord_of(key: KeyEvent) -> Option<Chord> {
    let m = key.modifiers;
    if m.contains(KeyModifiers::CONTROL) {
        Some(Chord {
            code: key.code,
            ctrl: true,
        })
    } else if m.is_empty() || m == KeyModifiers::SHIFT {
        Some(Chord {
            code: key.code,
            ctrl: false,
        })
    } else {
        None
    }
}

/// A plain/shifted (non-control) chord — the common case for vim command and
/// motion keys.
fn cmd(code: KeyCode) -> Chord {
    Chord { code, ctrl: false }
}

/// A `CONTROL` chord (`<C-v>`, `<C-u>`, …).
fn ctrl(code: KeyCode) -> Chord {
    Chord { code, ctrl: true }
}

/// The key → atom table, per context. A runtime `HashMap` (not a `match`)
/// precisely so a user override can be merged into it later; the per-key
/// lookup is off the render hot path (once per keystroke), so a hash lookup
/// is free.
#[derive(Debug)]
pub struct Bindings {
    map: HashMap<(Context, Chord), Atom>,
}

impl Bindings {
    /// The atom bound to `chord` in `ctx`, if any.
    pub fn get(&self, ctx: Context, chord: Chord) -> Option<Atom> {
        self.map.get(&(ctx, chord)).copied()
    }

    /// Every chord bound in `ctx` — for anything that must consider all of
    /// them, rather than a list written beside the table.
    ///
    /// A hand-written list of the keys a context binds is the same shape as the
    /// hand-written list of keys the viewer's Visual mode used to accept, which
    /// is what this accessor's first consumer was written to replace. Its
    /// second is [`listing`], and that is the reason it is no longer
    /// test-only: what `:keymap` prints has to be the table, not a description
    /// of it kept alongside.
    pub(crate) fn chords_in(&self, ctx: Context) -> Vec<Chord> {
        let mut out: Vec<Chord> = self
            .map
            .keys()
            .filter(|(c, _)| *c == ctx)
            .map(|(_, chord)| *chord)
            .collect();
        // A HashMap iterates in an unspecified order; a failing test should
        // name the same key every run.
        out.sort_by_key(|c| (format!("{:?}", c.code), c.ctrl));
        out
    }

    fn from_entries(entries: &[(Context, Chord, Atom)]) -> Self {
        let mut map = HashMap::with_capacity(entries.len());
        for &(ctx, chord, atom) in entries {
            let prev = map.insert((ctx, chord), atom);
            // The canonical defaults must not bind one (context, chord) twice
            // — a duplicate would silently let the last entry win, masking a
            // copy-paste slip. (A user *override* will go through a separate
            // merge path where overwriting is intended.)
            debug_assert!(
                prev.is_none(),
                "duplicate default binding for {ctx:?} {chord:?}"
            );
        }
        Bindings { map }
    }
}

/// The canonical vim default bindings — the data that replaces the engine's
/// hardcoded `match` leaves. Built once, lazily. A user's config diffs
/// against this in Phase C.
pub static DEFAULTS: LazyLock<Bindings> = LazyLock::new(|| {
    use crate::viewport::{Place, ScreenLine};
    use Atom::*;
    use CursorMotion::{
        BigWordBackward, BigWordEnd, BigWordForward, BufferEnd, BufferStart, Down, FirstNonBlank,
        Left, LineEnd, LineStart, MatchPair, Right, Screen, Section, Up, WordBackward, WordEnd,
        WordEndBackward, WordForward,
    };
    use KeyCode::Char;
    // A `z` command before its count is known; the engine fills `line`.
    let place = |at, first_non_blank| ScrollCmd::Place {
        at,
        line: None,
        first_non_blank,
    };
    // A find's prefix, and `;` `,` and `}` `{` before their counts are known
    // (`vim_keymap::apply_count_to_motion` fills them).
    let find = |till, forward| PrefixId::Find { till, forward };
    let again = |reverse| CursorMotion::RepeatFind { reverse, count: 1 };
    let paragraph = |forward| CursorMotion::Paragraph { forward, count: 1 };
    let listed = [
        // --- Context::Motion — cursor moves and operator targets ---
        (Context::Motion, cmd(Char('h')), Motion(Left)),
        (Context::Motion, cmd(KeyCode::Left), Motion(Left)),
        (Context::Motion, cmd(Char('l')), Motion(Right)),
        (Context::Motion, cmd(KeyCode::Right), Motion(Right)),
        (Context::Motion, cmd(Char('k')), Motion(Up)),
        (Context::Motion, cmd(KeyCode::Up), Motion(Up)),
        (Context::Motion, cmd(Char('j')), Motion(Down)),
        (Context::Motion, cmd(KeyCode::Down), Motion(Down)),
        (Context::Motion, cmd(Char('w')), Motion(WordForward)),
        (Context::Motion, cmd(Char('b')), Motion(WordBackward)),
        (Context::Motion, cmd(Char('e')), Motion(WordEnd)),
        (Context::Motion, cmd(Char('W')), Motion(BigWordForward)),
        (Context::Motion, cmd(Char('B')), Motion(BigWordBackward)),
        (Context::Motion, cmd(Char('E')), Motion(BigWordEnd)),
        // `0`/Home → line start. (A `0` *digit* is consumed as a count by the
        // engine before it reaches the table, so this only fires for a
        // leading `0` — vim's line-start.)
        (Context::Motion, cmd(Char('0')), Motion(LineStart)),
        (Context::Motion, cmd(KeyCode::Home), Motion(LineStart)),
        (Context::Motion, cmd(Char('$')), Motion(LineEnd)),
        (Context::Motion, cmd(KeyCode::End), Motion(LineEnd)),
        (Context::Motion, cmd(Char('G')), Motion(BufferEnd)),
        // `H` `M` `L` — by place on the SCREEN. The `1` is the default
        // distance from the edge; a typed count replaces it (`3H`), in
        // `vim_keymap::apply_count_to_motion`.
        (
            Context::Motion,
            cmd(Char('H')),
            Motion(Screen(ScreenLine::Top(1))),
        ),
        (
            Context::Motion,
            cmd(Char('M')),
            Motion(Screen(ScreenLine::Middle)),
        ),
        (
            Context::Motion,
            cmd(Char('L')),
            Motion(Screen(ScreenLine::Bottom(1))),
        ),
        (Context::Motion, cmd(Char('g')), Prefix(PrefixId::G)),
        (
            Context::Motion,
            cmd(Char('[')),
            Prefix(PrefixId::OpenBracket),
        ),
        (
            Context::Motion,
            cmd(Char(']')),
            Prefix(PrefixId::CloseBracket),
        ),
        (Context::Motion, cmd(Char('\'')), Prefix(PrefixId::Mark)),
        (Context::Motion, cmd(Char('`')), Prefix(PrefixId::AgentMark)),
        // `f` `F` `t` `T` `;` `,` — finds along the line (`:h f`); `;` `,`
        // the last again, which the app makes concrete (`App::concrete`).
        (Context::Motion, cmd(Char('f')), Prefix(find(false, true))),
        (Context::Motion, cmd(Char('F')), Prefix(find(false, false))),
        (Context::Motion, cmd(Char('t')), Prefix(find(true, true))),
        (Context::Motion, cmd(Char('T')), Prefix(find(true, false))),
        (Context::Motion, cmd(Char(';')), Motion(again(false))),
        (Context::Motion, cmd(Char(',')), Motion(again(true))),
        // `^` the first non-blank; `}` `{` paragraphs; `%` the matching pair.
        (Context::Motion, cmd(Char('^')), Motion(FirstNonBlank)),
        (Context::Motion, cmd(Char('}')), Motion(paragraph(true))),
        (Context::Motion, cmd(Char('{')), Motion(paragraph(false))),
        (Context::Motion, cmd(Char('%')), Motion(MatchPair)),
        // --- Context::AfterG — complete the `g` prefix ---
        (Context::AfterG, cmd(Char('g')), Motion(BufferStart)),
        // `ge` `gE` — back to a word's (WORD's) end.
        (
            Context::AfterG,
            cmd(Char('e')),
            Motion(WordEndBackward { big: false }),
        ),
        (
            Context::AfterG,
            cmd(Char('E')),
            Motion(WordEndBackward { big: true }),
        ),
        // `gt` / `gT` — the tab pages (vim's; `tabs.md` §4). Window commands,
        // as vim's `CTRL-W gt` is `gt`: the count (`2gt`) rides them as it
        // rides `<C-w>`'s, and an operator before one has nothing to act on.
        (Context::AfterG, cmd(Char('t')), Window(WindowCmd::TabNext)),
        (
            Context::AfterG,
            cmd(Char('T')),
            Window(WindowCmd::TabPrevious),
        ),
        // --- `[[` `]]` — a section back / forward, `[]` `][` a section's end;
        // the FIRST key is the direction (`:h ]]`). The `1` is the default
        // count, which a typed one replaces (`apply_count_to_motion`) ---
        (
            Context::AfterOpenBracket,
            cmd(Char('[')),
            Motion(Section {
                forward: false,
                end: false,
                count: 1,
            }),
        ),
        (
            Context::AfterOpenBracket,
            cmd(Char(']')),
            Motion(Section {
                forward: false,
                end: true,
                count: 1,
            }),
        ),
        (
            Context::AfterCloseBracket,
            cmd(Char(']')),
            Motion(Section {
                forward: true,
                end: false,
                count: 1,
            }),
        ),
        (
            Context::AfterCloseBracket,
            cmd(Char('[')),
            Motion(Section {
                forward: true,
                end: true,
                count: 1,
            }),
        ),
        // --- Context::Normal — non-motion commands ---
        (Context::Normal, cmd(Char('i')), Enter(InsertEntry::Before)),
        (Context::Normal, cmd(Char('a')), Enter(InsertEntry::After)),
        (Context::Normal, cmd(Char('A')), Enter(InsertEntry::LineEnd)),
        (
            Context::Normal,
            cmd(Char('I')),
            Enter(InsertEntry::LineStart),
        ),
        (
            Context::Normal,
            cmd(Char('o')),
            Enter(InsertEntry::OpenBelow),
        ),
        (
            Context::Normal,
            cmd(Char('O')),
            Enter(InsertEntry::OpenAbove),
        ),
        (Context::Normal, cmd(Char('x')), DeleteCharAfter),
        (Context::Normal, cmd(Char('X')), DeleteCharBefore),
        (Context::Normal, cmd(Char('~')), ToggleCase),
        (Context::Normal, cmd(Char('r')), ReplaceChar),
        (Context::Normal, cmd(Char('R')), EnterReplace),
        (Context::Normal, cmd(Char('m')), SetMark),
        (
            Context::Normal,
            ctrl(Char('v')),
            EnterVisual(SelectionKind::Block),
        ),
        (
            Context::Normal,
            cmd(Char('v')),
            EnterVisual(SelectionKind::Char),
        ),
        (
            Context::Normal,
            cmd(Char('V')),
            EnterVisual(SelectionKind::Line),
        ),
        (
            Context::Normal,
            cmd(Char('d')),
            BeginOperator(Operator::Delete),
        ),
        (
            Context::Normal,
            cmd(Char('c')),
            BeginOperator(Operator::Change),
        ),
        (
            Context::Normal,
            cmd(Char('y')),
            BeginOperator(Operator::Yank),
        ),
        (Context::Normal, cmd(Char('Y')), YankLine),
        (Context::Normal, cmd(Char('p')), Edit(EditAction::Paste)),
        (
            Context::Normal,
            cmd(Char('P')),
            Edit(EditAction::PasteBefore),
        ),
        (Context::Normal, cmd(Char('D')), DeleteToEol),
        (Context::Normal, cmd(Char('C')), ChangeToEol),
        (Context::Normal, cmd(Char('u')), Edit(EditAction::Undo)),
        (Context::Normal, ctrl(Char('r')), Edit(EditAction::Redo)),
        (
            Context::Normal,
            ctrl(Char('u')),
            Scroll(ScrollCmd::Up(HalfPage)),
        ),
        (
            Context::Normal,
            ctrl(Char('d')),
            Scroll(ScrollCmd::Down(HalfPage)),
        ),
        // Full-page paging. These lived ONLY in the chat viewer's hand-written
        // Ctrl block, so they were unbindable and absent from Normal — the
        // block is gone and the table owns every chord now (Reviewer D).
        (
            Context::Normal,
            ctrl(Char('f')),
            Scroll(ScrollCmd::Down(Page)),
        ),
        (
            Context::Normal,
            ctrl(Char('b')),
            Scroll(ScrollCmd::Up(Page)),
        ),
        // `:` opens the command line (`:q`, `:Edit`, `:config dump`).
        (Context::Normal, cmd(Char(':')), EnterCommand),
        // Search, vim's keys: `/` and `?` open the search line, `n` and `N`
        // repeat it, `*` and `#` search the word under the cursor.
        (
            Context::Normal,
            cmd(Char('/')),
            Search(SearchCmd::Open { forward: true }),
        ),
        (
            Context::Normal,
            cmd(Char('?')),
            Search(SearchCmd::Open { forward: false }),
        ),
        (
            Context::Normal,
            cmd(Char('n')),
            Search(SearchCmd::Next { reverse: false }),
        ),
        (
            Context::Normal,
            cmd(Char('N')),
            Search(SearchCmd::Next { reverse: true }),
        ),
        (
            Context::Normal,
            cmd(Char('*')),
            Search(SearchCmd::Word { forward: true }),
        ),
        (
            Context::Normal,
            cmd(Char('#')),
            Search(SearchCmd::Word { forward: false }),
        ),
        // Submit (criterion 11, all rebindable): in Normal, `Enter` submits
        // (vim-consistent — Enter has no edit meaning there); `Ctrl+Enter` is
        // the direct-submit key that also works mid-compose in Insert (below).
        // Plain `Enter` in Insert stays a newline, so an accidental Enter while
        // composing never fires a turn.
        (Context::Normal, cmd(KeyCode::Enter), Submit),
        (Context::Normal, ctrl(KeyCode::Enter), Submit),
        (Context::Insert, ctrl(KeyCode::Enter), Submit),
        // `<C-w>` opens the window-command prefix (focus now; resize/relocate
        // in later phases).
        (Context::Normal, ctrl(Char('w')), Prefix(PrefixId::Window)),
        // `<C-^>`, the window's alternate (vim's CTRL-^, V4b). A terminal
        // sends it as the byte 0x1E, which reads as CTRL with `6` (Ctrl-6,
        // Ctrl-Shift-6 alike); a terminal reporting keys in full may send
        // the `^` itself.
        (
            Context::Normal,
            ctrl(Char('6')),
            Window(WindowCmd::Alternate),
        ),
        (
            Context::Normal,
            ctrl(Char('^')),
            Window(WindowCmd::Alternate),
        ),
        // `<C-]>` follows a `|link|`, `<C-t>` comes back (vim's tag keys).
        // A terminal sends `<C-]>` as the byte 0x1D, which reads as CTRL
        // with `5`, as `<C-^>` does with `6`; one reporting keys in full
        // may send the `]` itself.
        (
            Context::Normal,
            ctrl(Char('5')),
            Window(WindowCmd::FollowLink),
        ),
        (
            Context::Normal,
            ctrl(Char(']')),
            Window(WindowCmd::FollowLink),
        ),
        (
            Context::Normal,
            ctrl(Char('t')),
            Window(WindowCmd::LinkBack),
        ),
        // `z` opens the scroll-relative-to-cursor prefix.
        (Context::Normal, cmd(Char('z')), Prefix(PrefixId::Z)),
        // --- Context::AfterZ — `zt` `zz` `zb`, and `z<CR>` `z.` `z-`, which
        // land the same way and also go to the first non-blank (`:h z<CR>`).
        // The count (`20zt`) is filled in by the engine, like `<C-w>`'s.
        (
            Context::AfterZ,
            cmd(Char('t')),
            Scroll(place(Place::Top, false)),
        ),
        (
            Context::AfterZ,
            cmd(Char('z')),
            Scroll(place(Place::Middle, false)),
        ),
        (
            Context::AfterZ,
            cmd(Char('b')),
            Scroll(place(Place::Bottom, false)),
        ),
        (
            Context::AfterZ,
            cmd(KeyCode::Enter),
            Scroll(place(Place::Top, true)),
        ),
        (
            Context::AfterZ,
            cmd(Char('.')),
            Scroll(place(Place::Middle, true)),
        ),
        (
            Context::AfterZ,
            cmd(Char('-')),
            Scroll(place(Place::Bottom, true)),
        ),
        // (And vim's fold verbs, `zo` `zc` `za` `zR` `zM` …: `fold::Z_KEYS`,
        // chained on below.)
        // --- Context::AfterWindow — the `<C-w>` window sub-context ---
        (
            Context::AfterWindow,
            cmd(Char('h')),
            Window(WindowCmd::Focus(Direction::Left)),
        ),
        (
            Context::AfterWindow,
            cmd(Char('j')),
            Window(WindowCmd::Focus(Direction::Down)),
        ),
        (
            Context::AfterWindow,
            cmd(Char('k')),
            Window(WindowCmd::Focus(Direction::Up)),
        ),
        (
            Context::AfterWindow,
            cmd(Char('l')),
            Window(WindowCmd::Focus(Direction::Right)),
        ),
        (
            Context::AfterWindow,
            cmd(Char('w')),
            Window(WindowCmd::FocusCycle),
        ),
        // The mark family: `<C-w><Space>` toggles this pane into the send set,
        // `<C-w>u` clears it. `<Space>` because it is the key `:graph` already
        // uses to mark a node — one gesture for "choose a set to act on".
        (
            Context::AfterWindow,
            cmd(Char(' ')),
            Window(WindowCmd::ToggleMark),
        ),
        (
            Context::AfterWindow,
            cmd(Char('u')),
            Window(WindowCmd::ClearMarks),
        ),
        // `<C-w><C-w>` also cycles — the quick focus toggle on two panes.
        (
            Context::AfterWindow,
            ctrl(Char('w')),
            Window(WindowCmd::FocusCycle),
        ),
        (
            Context::AfterWindow,
            cmd(Char('W')),
            Window(WindowCmd::FocusCycleBack),
        ),
        (
            Context::AfterWindow,
            cmd(Char('p')),
            Window(WindowCmd::FocusPrev),
        ),
        (
            Context::AfterWindow,
            cmd(Char('+')),
            Window(WindowCmd::ResizeGrow),
        ),
        (
            Context::AfterWindow,
            cmd(Char('-')),
            Window(WindowCmd::ResizeShrink),
        ),
        (
            Context::AfterWindow,
            cmd(Char('>')),
            Window(WindowCmd::ResizeWider),
        ),
        (
            Context::AfterWindow,
            cmd(Char('<')),
            Window(WindowCmd::ResizeNarrower),
        ),
        (
            Context::AfterWindow,
            cmd(Char('=')),
            Window(WindowCmd::Equalize),
        ),
        // `<C-w>H/J/K/L` relocate the focused pane to an edge (vim window
        // move): `K`/`J` reorder the vertical stack, `H`/`L` reorient to a
        // side-by-side split.
        (
            Context::AfterWindow,
            cmd(Char('K')),
            Window(WindowCmd::Relocate(Direction::Up)),
        ),
        (
            Context::AfterWindow,
            cmd(Char('J')),
            Window(WindowCmd::Relocate(Direction::Down)),
        ),
        (
            Context::AfterWindow,
            cmd(Char('H')),
            Window(WindowCmd::Relocate(Direction::Left)),
        ),
        (
            Context::AfterWindow,
            cmd(Char('L')),
            Window(WindowCmd::Relocate(Direction::Right)),
        ),
        // Splitting and closing, vim's letters exactly: `s`/`S` stack, `v`
        // side-by-side, `c` close, `o` keep only this one.
        (
            Context::AfterWindow,
            cmd(Char('s')),
            Window(WindowCmd::Split),
        ),
        (
            Context::AfterWindow,
            cmd(Char('S')),
            Window(WindowCmd::Split),
        ),
        (
            Context::AfterWindow,
            cmd(Char('v')),
            Window(WindowCmd::VSplit),
        ),
        (
            Context::AfterWindow,
            cmd(Char('c')),
            Window(WindowCmd::Close),
        ),
        // vim's `<C-w>q`: close this window, and leave the editor if it was the
        // last one. `<C-w>c` never quits; that is the whole difference.
        (
            Context::AfterWindow,
            cmd(Char('q')),
            Window(WindowCmd::Quit),
        ),
        (
            Context::AfterWindow,
            cmd(Char('o')),
            Window(WindowCmd::Only),
        ),
        // vim's `<C-w>T`: this window to a new tab of its own.
        (
            Context::AfterWindow,
            cmd(Char('T')),
            Window(WindowCmd::MoveToTab),
        ),
        // --- Context::Visual — non-motion commands ---
        (Context::Visual, cmd(KeyCode::Esc), LeaveVisual),
        (
            Context::Visual,
            ctrl(Char('v')),
            SwitchVisual(SelectionKind::Block),
        ),
        (
            Context::Visual,
            cmd(Char('v')),
            SwitchVisual(SelectionKind::Char),
        ),
        (
            Context::Visual,
            cmd(Char('V')),
            SwitchVisual(SelectionKind::Line),
        ),
        (
            Context::Visual,
            cmd(Char('d')),
            VisualOperator(Operator::Delete),
        ),
        (
            Context::Visual,
            cmd(Char('x')),
            VisualOperator(Operator::Delete),
        ),
        (
            Context::Visual,
            cmd(Char('c')),
            VisualOperator(Operator::Change),
        ),
        (
            Context::Visual,
            cmd(Char('s')),
            VisualOperator(Operator::Change),
        ),
        (
            Context::Visual,
            cmd(Char('y')),
            VisualOperator(Operator::Yank),
        ),
        (
            Context::Visual,
            cmd(Char('I')),
            BlockInsert(BlockEdge::Left),
        ),
        (
            Context::Visual,
            cmd(Char('A')),
            BlockInsert(BlockEdge::Right),
        ),
        (Context::Visual, cmd(Char('o')), SwapSelectionEnds),
        (
            Context::Visual,
            cmd(Char('~')),
            VisualOperator(Operator::Case(CaseOp::Toggle)),
        ),
        (
            Context::Visual,
            cmd(Char('u')),
            VisualOperator(Operator::Case(CaseOp::Lower)),
        ),
        (
            Context::Visual,
            cmd(Char('U')),
            VisualOperator(Operator::Case(CaseOp::Upper)),
        ),
        (Context::Visual, cmd(Char('r')), ReplaceChar),
        // --- Context::TextObject — the object key of `{op}{i,a}{object}` ---
        (
            Context::TextObject,
            cmd(Char('w')),
            Object(TextObject::Word),
        ),
        (
            Context::TextObject,
            cmd(Char('W')),
            Object(TextObject::BigWord),
        ),
        (
            Context::TextObject,
            cmd(Char('"')),
            Object(TextObject::Quote('"')),
        ),
        (
            Context::TextObject,
            cmd(Char('\'')),
            Object(TextObject::Quote('\'')),
        ),
        (
            Context::TextObject,
            cmd(Char('`')),
            Object(TextObject::Quote('`')),
        ),
        (
            Context::TextObject,
            cmd(Char('(')),
            Object(TextObject::Pair('(', ')')),
        ),
        (
            Context::TextObject,
            cmd(Char(')')),
            Object(TextObject::Pair('(', ')')),
        ),
        (
            Context::TextObject,
            cmd(Char('b')),
            Object(TextObject::Pair('(', ')')),
        ),
        (
            Context::TextObject,
            cmd(Char('{')),
            Object(TextObject::Pair('{', '}')),
        ),
        (
            Context::TextObject,
            cmd(Char('}')),
            Object(TextObject::Pair('{', '}')),
        ),
        (
            Context::TextObject,
            cmd(Char('B')),
            Object(TextObject::Pair('{', '}')),
        ),
        (
            Context::TextObject,
            cmd(Char('[')),
            Object(TextObject::Pair('[', ']')),
        ),
        (
            Context::TextObject,
            cmd(Char(']')),
            Object(TextObject::Pair('[', ']')),
        ),
        (
            Context::TextObject,
            cmd(Char('<')),
            Object(TextObject::Pair('<', '>')),
        ),
        (
            Context::TextObject,
            cmd(Char('>')),
            Object(TextObject::Pair('<', '>')),
        ),
        (
            Context::TextObject,
            cmd(Char('p')),
            Object(TextObject::Paragraph),
        ),
    ];
    // vim's fold verbs after `z`, from the one list that names them
    // (`fold::Z_KEYS`), which `FoldVerb::after_z` reads too.
    let folds = (crate::fold::Z_KEYS.iter())
        .map(|&(key, verb)| (Context::AfterZ, cmd(Char(key)), Fold(verb)));
    let entries: Vec<(Context, Chord, Atom)> = listed.into_iter().chain(folds).collect();
    Bindings::from_entries(&entries)
});

/// The keymap the engine actually consults — `DEFAULTS` merged with the user's
/// config overrides, installed once at startup. Falls back to `DEFAULTS`
/// before (or without) a config, so the keymap tests see the canonical table.
static ACTIVE: OnceLock<Bindings> = OnceLock::new();

/// Install the merged bindings (call once, at startup). A second call is a
/// no-op — the table is process-global and immutable after the first set.
pub fn set_active(bindings: Bindings) {
    let _ = ACTIVE.set(bindings);
}

/// The active bindings: the config-merged table if installed, else the
/// canonical [`DEFAULTS`].
pub fn active() -> &'static Bindings {
    ACTIVE.get().unwrap_or(&DEFAULTS)
}

/// The stable config-name ↔ [`Atom`] table — the single source of truth for
/// what a user writes in `[keymap.*]`. Every atom the defaults bind has a name
/// here (guarded by a completeness test), so the whole keymap is remappable
/// and round-trips for `:keymap dump`. Searched linearly at config-load time
/// (off any hot path).
static ATOM_NAMES: LazyLock<Vec<(&'static str, Atom)>> = LazyLock::new(|| {
    use crate::viewport::{Place, ScreenLine};
    use Atom::*;
    use CursorMotion as M;
    use Direction as D;
    use EditAction as EA;
    use InsertEntry as IE;
    use Operator as Op;
    use ScrollCmd as SC;
    use SelectionKind as SK;
    use TextObject as TO;
    use WindowCmd as W;
    let place = |at, first_non_blank| SC::Place {
        at,
        line: None,
        first_non_blank,
    };
    vec![
        // motions
        ("left", Motion(M::Left)),
        ("right", Motion(M::Right)),
        ("up", Motion(M::Up)),
        ("down", Motion(M::Down)),
        ("word-forward", Motion(M::WordForward)),
        ("word-backward", Motion(M::WordBackward)),
        ("word-end", Motion(M::WordEnd)),
        ("bigword-forward", Motion(M::BigWordForward)),
        ("bigword-backward", Motion(M::BigWordBackward)),
        ("bigword-end", Motion(M::BigWordEnd)),
        ("line-start", Motion(M::LineStart)),
        ("line-end", Motion(M::LineEnd)),
        ("buffer-start", Motion(M::BufferStart)),
        ("buffer-end", Motion(M::BufferEnd)),
        ("screen-top", Motion(M::Screen(ScreenLine::Top(1)))),
        ("screen-middle", Motion(M::Screen(ScreenLine::Middle))),
        ("screen-bottom", Motion(M::Screen(ScreenLine::Bottom(1)))),
        (
            "section-forward",
            Motion(M::Section {
                forward: true,
                end: false,
                count: 1,
            }),
        ),
        (
            "section-backward",
            Motion(M::Section {
                forward: false,
                end: false,
                count: 1,
            }),
        ),
        (
            "section-end-forward",
            Motion(M::Section {
                forward: true,
                end: true,
                count: 1,
            }),
        ),
        (
            "section-end-backward",
            Motion(M::Section {
                forward: false,
                end: true,
                count: 1,
            }),
        ),
        ("first-non-blank", Motion(M::FirstNonBlank)),
        (
            "paragraph-forward",
            Motion(M::Paragraph {
                forward: true,
                count: 1,
            }),
        ),
        (
            "paragraph-backward",
            Motion(M::Paragraph {
                forward: false,
                count: 1,
            }),
        ),
        ("match-pair", Motion(M::MatchPair)),
        (
            "word-end-backward",
            Motion(M::WordEndBackward { big: false }),
        ),
        (
            "bigword-end-backward",
            Motion(M::WordEndBackward { big: true }),
        ),
        (
            "find-again",
            Motion(M::RepeatFind {
                reverse: false,
                count: 1,
            }),
        ),
        (
            "find-again-reverse",
            Motion(M::RepeatFind {
                reverse: true,
                count: 1,
            }),
        ),
        // prefixes
        (
            "prefix:find",
            Prefix(PrefixId::Find {
                till: false,
                forward: true,
            }),
        ),
        (
            "prefix:find-backward",
            Prefix(PrefixId::Find {
                till: false,
                forward: false,
            }),
        ),
        (
            "prefix:till",
            Prefix(PrefixId::Find {
                till: true,
                forward: true,
            }),
        ),
        (
            "prefix:till-backward",
            Prefix(PrefixId::Find {
                till: true,
                forward: false,
            }),
        ),
        ("prefix:goto", Prefix(PrefixId::G)),
        ("prefix:open-bracket", Prefix(PrefixId::OpenBracket)),
        ("prefix:close-bracket", Prefix(PrefixId::CloseBracket)),
        ("prefix:window", Prefix(PrefixId::Window)),
        ("prefix:z", Prefix(PrefixId::Z)),
        ("prefix:mark", Prefix(PrefixId::Mark)),
        ("prefix:agent-mark", Prefix(PrefixId::AgentMark)),
        // `z` commands — vim's "scroll-cursor" family (`:h scroll-cursor`)
        ("scroll-cursor-top", Scroll(place(Place::Top, false))),
        ("scroll-cursor-middle", Scroll(place(Place::Middle, false))),
        ("scroll-cursor-bottom", Scroll(place(Place::Bottom, false))),
        (
            "scroll-cursor-top-first-nonblank",
            Scroll(place(Place::Top, true)),
        ),
        (
            "scroll-cursor-middle-first-nonblank",
            Scroll(place(Place::Middle, true)),
        ),
        (
            "scroll-cursor-bottom-first-nonblank",
            Scroll(place(Place::Bottom, true)),
        ),
        // the fold verbs (`:h fold-commands`)
        ("fold-open", Fold(crate::fold::FoldVerb::Open)),
        ("fold-close", Fold(crate::fold::FoldVerb::Close)),
        ("fold-toggle", Fold(crate::fold::FoldVerb::Toggle)),
        ("fold-open-all", Fold(crate::fold::FoldVerb::OpenAll)),
        ("fold-close-all", Fold(crate::fold::FoldVerb::CloseAll)),
        // edits (count-repeatable)
        ("paste-after", Edit(EA::Paste)),
        ("paste-before", Edit(EA::PasteBefore)),
        ("undo", Edit(EA::Undo)),
        ("redo", Edit(EA::Redo)),
        // enter Insert
        ("insert", Enter(IE::Before)),
        ("append", Enter(IE::After)),
        ("insert-line-start", Enter(IE::LineStart)),
        ("append-line-end", Enter(IE::LineEnd)),
        ("open-below", Enter(IE::OpenBelow)),
        ("open-above", Enter(IE::OpenAbove)),
        // enter Visual
        ("visual", EnterVisual(SK::Char)),
        ("visual-line", EnterVisual(SK::Line)),
        ("visual-block", EnterVisual(SK::Block)),
        // operators
        ("delete", BeginOperator(Op::Delete)),
        ("change", BeginOperator(Op::Change)),
        ("yank", BeginOperator(Op::Yank)),
        // single-key edits
        ("delete-char", DeleteCharAfter),
        ("delete-char-before", DeleteCharBefore),
        ("yank-line", YankLine),
        ("delete-to-eol", DeleteToEol),
        ("change-to-eol", ChangeToEol),
        ("toggle-case", ToggleCase),
        ("replace-char", ReplaceChar),
        ("replace-mode", EnterReplace),
        ("set-mark", SetMark),
        ("command-line", EnterCommand),
        ("search-forward", Search(SearchCmd::Open { forward: true })),
        (
            "search-backward",
            Search(SearchCmd::Open { forward: false }),
        ),
        ("search-next", Search(SearchCmd::Next { reverse: false })),
        ("search-previous", Search(SearchCmd::Next { reverse: true })),
        ("search-word", Search(SearchCmd::Word { forward: true })),
        (
            "search-word-backward",
            Search(SearchCmd::Word { forward: false }),
        ),
        ("submit", Submit),
        // scroll
        ("scroll-half-up", Scroll(SC::Up(HalfPage))),
        ("scroll-half-down", Scroll(SC::Down(HalfPage))),
        ("scroll-page-up", Scroll(SC::Up(Page))),
        ("scroll-page-down", Scroll(SC::Down(Page))),
        // visual-mode commands
        ("leave-visual", LeaveVisual),
        ("switch-visual", SwitchVisual(SK::Char)),
        ("switch-visual-line", SwitchVisual(SK::Line)),
        ("switch-visual-block", SwitchVisual(SK::Block)),
        ("visual-delete", VisualOperator(Op::Delete)),
        ("visual-change", VisualOperator(Op::Change)),
        ("visual-yank", VisualOperator(Op::Yank)),
        (
            "visual-toggle-case",
            VisualOperator(Op::Case(CaseOp::Toggle)),
        ),
        ("visual-lowercase", VisualOperator(Op::Case(CaseOp::Lower))),
        ("visual-uppercase", VisualOperator(Op::Case(CaseOp::Upper))),
        ("block-insert", BlockInsert(BlockEdge::Left)),
        ("block-append", BlockInsert(BlockEdge::Right)),
        ("swap-selection-ends", SwapSelectionEnds),
        // window commands
        ("window-focus-left", Window(W::Focus(D::Left))),
        ("window-focus-down", Window(W::Focus(D::Down))),
        ("window-focus-up", Window(W::Focus(D::Up))),
        ("window-focus-right", Window(W::Focus(D::Right))),
        ("window-cycle", Window(W::FocusCycle)),
        ("window-cycle-back", Window(W::FocusCycleBack)),
        ("window-prev", Window(W::FocusPrev)),
        ("window-grow", Window(W::ResizeGrow)),
        ("window-shrink", Window(W::ResizeShrink)),
        ("window-wider", Window(W::ResizeWider)),
        ("window-narrower", Window(W::ResizeNarrower)),
        ("window-equalize", Window(W::Equalize)),
        ("window-move-left", Window(W::Relocate(D::Left))),
        ("window-move-down", Window(W::Relocate(D::Down))),
        ("window-move-up", Window(W::Relocate(D::Up))),
        ("window-move-right", Window(W::Relocate(D::Right))),
        ("window-split", Window(W::Split)),
        ("window-vsplit", Window(W::VSplit)),
        ("window-close", Window(W::Close)),
        ("window-quit", Window(W::Quit)),
        ("window-toggle-mark", Window(W::ToggleMark)),
        ("window-clear-marks", Window(W::ClearMarks)),
        ("window-only", Window(W::Only)),
        ("window-alternate", Window(W::Alternate)),
        ("follow-link", Window(W::FollowLink)),
        ("link-back", Window(W::LinkBack)),
        ("tab-next", Window(W::TabNext)),
        ("tab-previous", Window(W::TabPrevious)),
        ("window-move-to-tab", Window(W::MoveToTab)),
        // text objects (after `{operator}{i,a}`)
        ("object-word", Object(TO::Word)),
        ("object-bigword", Object(TO::BigWord)),
        ("object-quote-double", Object(TO::Quote('"'))),
        ("object-quote-single", Object(TO::Quote('\''))),
        ("object-quote-backtick", Object(TO::Quote('`'))),
        ("object-paren", Object(TO::Pair('(', ')'))),
        ("object-brace", Object(TO::Pair('{', '}'))),
        ("object-bracket", Object(TO::Pair('[', ']'))),
        ("object-angle", Object(TO::Pair('<', '>'))),
        ("object-paragraph", Object(TO::Paragraph)),
    ]
});

/// What an atom does in a **read-only pane** — work, or be refused, and why.
///
/// One answer rather than two, because the refusal the user reads and the
/// decision that refused have to be the same fact. The predicate was a `bool`
/// for months and the reason existed nowhere: every writer key in the
/// transcript returned `Ignored`, so `i` was indistinguishable from a broken
/// keyboard while the status row advertised `i:insert`. A second table mapping
/// atom → sentence would have re-opened the same gap one indirection later
/// (`[[feedback_enumeration_means_unmodeled]]`), so the reason is a *variant of
/// the verdict*: a new kind of refusal cannot be added without words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerFit {
    /// Available here.
    Available,
    /// Refused — it writes, and a read-only pane has no buffer to write through.
    Writes,
    /// Refused — it sends the **prompt's** text, which is not the buffer you
    /// are looking at.
    Sends,
}

impl ViewerFit {
    /// The sentence the user reads when this atom is refused, or `None` when it
    /// is available.
    ///
    /// Each names the remedy, because "read-only" alone tells a user what they
    /// cannot do and not what they can. `<C-w>w` is the way back to the prompt
    /// from every read-only pane.
    pub const fn refusal(self) -> Option<&'static str> {
        match self {
            ViewerFit::Available => None,
            // **No `y copies from here` clause.** It was there, and it was true
            // in exactly one of the five panes this fires in: `y` is
            // `Available`, so the keymap emits an operator, and every pane but
            // the transcript drops it on a `{}` arm. So the sentence advertised
            // a key that is silently dead — the defect this whole refusal
            // exists to remove, moved one key over and then promoted. Worse,
            // four of those panes ALSO say "nothing to select here" on `v`, so
            // the two messages contradicted each other (Reviewer F).
            //
            // A remedy that is only sometimes true has to be a function of the
            // pane, not a constant; making it one is docketed. Saying less is
            // the honest half.
            // **`<C-w>w` CYCLES; it does not go to the prompt.** With three
            // panes open, one press lands wherever the ring goes next, and the
            // sentence promised otherwise — a remedy that is only sometimes
            // true is the defect this refusal exists to remove, moved one key
            // over (Steven, dogfooding 2026-08-25). Name the destination and
            // let `<C-w>w` be what it is.
            ViewerFit::Writes => {
                Some("this pane is read-only — type in a prompt pane (<C-w>w cycles)")
            }
            ViewerFit::Sends => {
                Some("the prompt is what sends — go to a prompt pane (<C-w>w cycles), then Enter")
            }
        }
    }
}

impl Atom {
    /// Is this atom available in a **read-only pane** (the transcript, a
    /// viewer)?
    ///
    /// Consults [`viewer_fit`](Self::viewer_fit) rather than re-deciding, so
    /// there is one membership rule and the refusing path cannot disagree with
    /// the accepting one.
    pub fn in_viewer(self) -> bool {
        self.viewer_fit() == ViewerFit::Available
    }

    /// Whether this atom works in a read-only pane, and the reason when it does
    /// not.
    ///
    /// Such a pane has the prompt's Normal mode minus anything that would write
    /// through it. Asking the table is what keeps that honest: the viewer used
    /// to hand-list the three keys it accepted, so `:` was silently swallowed
    /// and `:graph` could only be typed from the prompt — backwards, since
    /// looking at history is exactly when you want it. A hand-list must also be
    /// remembered every time an atom is added; this cannot be forgotten,
    /// because a new variant is a compile error here.
    pub fn viewer_fit(self) -> ViewerFit {
        use Atom::*;
        use ViewerFit::*;
        match self {
            // Navigation, scrolling, selection, window commands, `:`, and
            // search, which only moves the cursor. A fold is a view, as a
            // scroll is: it shows the text otherwise, and writes none.
            Motion(_) | Prefix(_) | Scroll(_) | EnterCommand | Search(_) => Available,
            Fold(_) => Available,
            EnterVisual(_) | LeaveVisual | SwitchVisual(_) | SwapSelectionEnds => Available,
            Window(_) | Object(_) => Available,
            // A mark is a place, not text: vim sets one in a `nomodifiable`
            // buffer, and the transcript is where marks are first wanted.
            SetMark => Available,
            // Copying out is reading, so `Y` and a Visual `y` belong here; every
            // other operator writes.
            YankLine => Available,
            BeginOperator(op) | VisualOperator(op) => {
                if matches!(op, Operator::Yank) {
                    Available
                } else {
                    Writes
                }
            }
            // **Submit is excluded, and not because it writes a buffer.** It
            // sends the *prompt's* text — a buffer you can neither see nor edit
            // from here — so an `Enter` while reading would fire a billable turn
            // you did not compose. Same reasoning that keeps plain `Enter` a
            // newline in Insert. `<C-w>w` back to the prompt to send.
            Submit => Sends,
            // The writers.
            Edit(_) | Enter(_) | BlockInsert(_) => Writes,
            DeleteCharAfter | DeleteCharBefore | DeleteToEol | ChangeToEol => Writes,
            ToggleCase | ReplaceChar | EnterReplace => Writes,
        }
    }

    /// **Does this key still mean itself while a selection is up?**
    ///
    /// Vim keeps the command line and the scroll commands live in Visual —
    /// `:` starts an ex-command, `CTRL-D`/`CTRL-U`/`CTRL-F`/`CTRL-B` page the
    /// window — because neither is *about* the selection. Everything else in
    /// Visual belongs to the Visual table, which is why this is a small `true`
    /// list and a large `false` one rather than a fallthrough to Normal:
    /// falling through would give `x` and `p` their Normal meanings on top of
    /// their Visual ones.
    ///
    /// A property of the atom, exhaustively matched, so a new atom has to
    /// answer the question rather than inherit a default
    /// (`[[feedback_enumeration_means_unmodeled]]`).
    pub fn works_in_visual(self) -> bool {
        match self {
            // Neither reads nor writes the selection — a search moves its
            // end, as a motion does (vim's `v/foo`); a mark is set where its
            // end is, and the selection stays (vim's `vjma`, nvim checked
            // 10-31: still Visual, `'a` at the cursor; review stop 14, A 8).
            Atom::Scroll(_) | Atom::EnterCommand | Atom::Search(_) | Atom::SetMark => true,
            // Motions already reach Visual through the motion table; the rest
            // are Normal-mode edits, whose Visual meanings are their own.
            Atom::Motion(_)
            | Atom::Prefix(_)
            | Atom::Edit(_)
            | Atom::Enter(_)
            | Atom::EnterVisual(_)
            | Atom::BeginOperator(_)
            | Atom::DeleteCharAfter
            | Atom::DeleteCharBefore
            | Atom::YankLine
            | Atom::DeleteToEol
            | Atom::ChangeToEol
            | Atom::ToggleCase
            | Atom::ReplaceChar
            | Atom::EnterReplace
            | Atom::Submit
            | Atom::LeaveVisual
            | Atom::SwitchVisual(_)
            | Atom::VisualOperator(_)
            | Atom::BlockInsert(_)
            | Atom::SwapSelectionEnds => false,
            // None is ever looked up here: a window command arrives through
            // the `<C-w>` prefix (`window_prefix_from_visual` ends Visual and
            // arms it), a fold verb through the `z` prefix, and an object
            // through `Context::TextObject` after `vi`/`va`. `false` says
            // "not via this door", not "not in Visual".
            Atom::Window(_) | Atom::Fold(_) | Atom::Object(_) => false,
        }
    }

    /// The [`Atom`] a config name refers to, or `None` if the name is unknown.
    pub fn from_name(name: &str) -> Option<Atom> {
        ATOM_NAMES.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
    }

    /// The stable config name for this atom, or `None` if it isn't a bindable
    /// (named) atom.
    pub fn name(self) -> Option<&'static str> {
        ATOM_NAMES.iter().find(|(_, a)| *a == self).map(|(n, _)| *n)
    }
}

impl Context {
    /// The `[keymap.<name>]` table this context is spelled as in a config.
    ///
    /// Exhaustive, so a context added later has to be *given* a name rather
    /// than silently becoming un-remappable — the config would go on accepting
    /// its chords nowhere.
    pub fn config_name(self) -> &'static str {
        match self {
            Context::Motion => "motion",
            Context::AfterG => "goto",
            Context::AfterOpenBracket => "open-bracket",
            Context::AfterCloseBracket => "close-bracket",
            Context::Normal => "normal",
            Context::Visual => "visual",
            Context::Insert => "insert",
            Context::AfterWindow => "window",
            Context::AfterZ => "z",
            Context::TextObject => "text-object",
        }
    }

    /// A one-line account of what this context IS, for `:keymap`.
    ///
    /// A user reading `[keymap.motion]` cannot tell from the name that it is
    /// shared by direct moves and operator targets — which is the one fact that
    /// explains why remapping `j` there also changes `dj`.
    pub fn describe(self) -> &'static str {
        match self {
            Context::Motion => {
                "cursor motions — shared by `3j` and by `d3j`, so a remap changes both"
            }
            Context::AfterG => "after a bare `g` (`gg`)",
            Context::AfterOpenBracket => {
                "after a bare `[` (`[[` a section back — a turn, in a transcript; `[]` an end)"
            }
            Context::AfterCloseBracket => "after a bare `]` (`]]` a section forward; `][` an end)",
            Context::Normal => "Normal-mode commands the motion resolver declined",
            Context::Visual => "Visual-mode commands (motions come from `motion`)",
            Context::Insert => "Insert mode",
            Context::AfterWindow => "after `<C-w>` — the window commands",
            Context::AfterZ => {
                "after `z` — put the cursor's line at the top/middle/bottom, or open and shut folds"
            }
            Context::TextObject => "the object key after `{operator}{i,a}` (`diw`)",
        }
    }
}

/// Every [`Context`], in `:keymap` display order — complete by the compiler
/// (`every_variant!`: the list is an exhaustive match, so a context left out
/// does not compile). It was a `next()` chain, whose match gave each context
/// a successor and never checked that anything led to it (review stop 12,
/// T6 10; `[[feedback_enumeration_means_unmodeled]]`).
pub const CONTEXTS: &[Context] = every_variant!(
    Context: Motion,
    AfterG,
    AfterOpenBracket,
    AfterCloseBracket,
    Normal,
    Visual,
    Insert,
    AfterWindow,
    AfterZ,
    TextObject,
);

/// Parse a config context name (`[keymap.<name>]`) to an engine [`Context`].
///
/// **Searches [`Context::config_name`]** rather than re-listing the names. The
/// two directions were a pair of hand-written matches, which is the shape that
/// lets a config accept a name that `:keymap` cannot print (or the reverse) —
/// and neither would fail to compile.
fn parse_context(name: &str) -> Option<Context> {
    CONTEXTS.iter().copied().find(|c| c.config_name() == name)
}

/// The keys a chord string may name inside `<…>`, and the codes they mean.
///
/// **One table, read in both directions**: [`parse_keyname`] searches it by
/// name, [`key_name`] by code. So a key that can be typed into a config can
/// also be printed by `:keymap`, and a key added to one direction cannot be
/// missing from the other.
///
/// `CR` is a second spelling of `Enter` and comes after it, because the
/// by-code search takes the first hit and `Enter` is the one to show
/// (`[[feedback_second_spelling]]`).
const KEY_NAMES: &[(&str, KeyCode)] = &[
    ("Esc", KeyCode::Esc),
    ("Enter", KeyCode::Enter),
    ("CR", KeyCode::Enter),
    ("Space", KeyCode::Char(' ')),
    ("Tab", KeyCode::Tab),
    ("Up", KeyCode::Up),
    ("Down", KeyCode::Down),
    ("Left", KeyCode::Left),
    ("Right", KeyCode::Right),
    ("Home", KeyCode::Home),
    ("End", KeyCode::End),
];

/// The `<…>` name for a key code, or `None` when the code is an ordinary glyph
/// that spells itself.
fn key_name(code: KeyCode) -> Option<&'static str> {
    KEY_NAMES.iter().find(|(_, c)| *c == code).map(|(n, _)| *n)
}

/// How a chord is **written in a config** — the inverse of `parse_chord`.
///
/// Round-tripping is the property that matters and it is tested against the
/// whole default table: what `:keymap` prints must be something a user can
/// paste into `config.toml` and have parse back to the same chord. A listing
/// that shows an unpasteable spelling is worse than no listing, because it
/// looks authoritative.
pub fn chord_name(chord: Chord) -> String {
    let glyph = match key_name(chord.code) {
        Some(name) => name.to_string(),
        None => match chord.code {
            KeyCode::Char(c) => c.to_string(),
            // Nothing else is bindable today; printed as Debug so an
            // unrepresentable key is visibly odd rather than silently wrong.
            other => format!("{other:?}"),
        },
    };
    let bracketed = key_name(chord.code).is_some() || chord.ctrl;
    match (chord.ctrl, bracketed) {
        (true, _) => format!("<C-{glyph}>"),
        (false, true) => format!("<{glyph}>"),
        (false, false) => glyph,
    }
}

/// **The atom a key names in Normal mode with nothing pending**, through the
/// *active* table — so a remap carries its meaning with the key rather than
/// leaving it attached to a letter. `None` when the key is bound to nothing
/// (counts included: a digit is vim's fixed rule, not a binding —
/// `vim_keymap::count_digit`).
///
/// **`Motion` first, then `Normal`**, the order the keymap resolves a root key
/// in. Asking `Normal` alone missed every motion — `j` is not in it — which
/// is how the decision pane's first cut answered `j` with "not a decision
/// key".
///
/// For a pane with verbs of its own that must decide which of the remaining
/// keys the keymap should have (the decision pane: those
/// [`Atom::in_viewer`] admits).
pub fn normal_atom(key: KeyEvent) -> Option<Atom> {
    let chord = chord_of(key)?;
    let table = active();
    table
        .get(Context::Motion, chord)
        .or_else(|| table.get(Context::Normal, chord))
}

/// **The active keymap, as text** — what `:keymap` shows.
///
/// `only` names one context (`:keymap normal`); `None` lists them all. The
/// error is the user-facing sentence for an unknown name, so the caller can
/// hand it straight to a refusal.
///
/// It exists because **no action name was discoverable inside the product**.
/// The shipped config template said "run `:keymap names`", which was not a
/// command and resolved to *not a command*; `:help`'s keymap chapter listed the
/// seven contexts and zero actions and pointed at `:config dump`, which by
/// design carries no `[keymap]` section. So `scroll-page-up` was reachable only
/// by reading this file — an extension point whose vocabulary was a secret
/// (Reviewer F).
///
/// Read out of [`active`] rather than [`DEFAULTS`]: a user who remapped a key
/// wants to see what they have, and a listing that shows the shipped table to
/// somebody running a modified one is a lie in the one place they came for the
/// truth.
pub fn listing(only: Option<&str>) -> Result<String, String> {
    let contexts: Vec<Context> = match only {
        None => CONTEXTS.to_vec(),
        Some(name) => match parse_context(name.trim()) {
            Some(ctx) => vec![ctx],
            None => {
                let names: Vec<&str> = CONTEXTS.iter().map(|c| c.config_name()).collect();
                return Err(format!(
                    "no keymap context `{}` — try one of: {}",
                    name.trim(),
                    names.join(", ")
                ));
            }
        },
    };
    let table = active();
    let mut out = String::new();
    out.push_str(
        "KEYMAP — the keys in force, and the action names a config binds them to\n\n\
         A chord is what you press; an action is the stable name for what it does.\n\
         Remap in your config (`:config edit`) — it is a sparse diff over the\n\
         defaults, so anything you do not name keeps working:\n\n    \
         [keymap.normal]\n    \
         \"<C-d>\" = \"submit\"\n\n",
    );
    if only.is_none() {
        out.push_str("`:keymap <context>` shows one context on its own.\n\n");
    }
    let mut bound: Vec<Atom> = Vec::new();
    for ctx in &contexts {
        out.push_str(&format!(
            "--- [keymap.{}] {}\n",
            ctx.config_name(),
            ctx.describe()
        ));
        let mut rows: Vec<(String, &'static str)> = Vec::new();
        for chord in table.chords_in(*ctx) {
            let atom = match table.get(*ctx, chord) {
                Some(a) => a,
                None => continue,
            };
            bound.push(atom);
            // An atom with no name is bound but not *bindable* — the user
            // cannot name it in a config. Say which, rather than omitting the
            // row: a key missing from the listing reads as a key that does
            // nothing.
            rows.push((chord_name(chord), atom.name().unwrap_or("(unnameable)")));
        }
        rows.sort_by(|a, b| {
            a.0.to_lowercase()
                .cmp(&b.0.to_lowercase())
                .then(a.0.cmp(&b.0))
        });
        for (chord, action) in rows {
            out.push_str(&format!("    {chord:<10}  {action}\n"));
        }
        out.push('\n');
    }
    // Only meaningful for the whole listing: an action absent from ONE context
    // is not unbound, it is bound somewhere else.
    if only.is_none() {
        let unbound: Vec<&str> = atom_names()
            .filter(|(_, atom)| !bound.contains(atom))
            .map(|(name, _)| name)
            .collect();
        // **Printed even when empty.** "Every action has a key" is worth
        // knowing, and a section that only sometimes appears makes the
        // manual's sentence about it only sometimes true — which is the
        // class of defect this command was written to remove.
        out.push_str("--- bindable, but nothing presses them by default\n    ");
        let joined = unbound.join(", ");
        out.push_str(if joined.is_empty() {
            "(none — every action is on a key)"
        } else {
            &joined
        });
        out.push_str("\n\n");
    }
    Ok(out)
}

/// Every `(config name, atom)` pair — the vocabulary a `[keymap.*]` table may
/// use.
///
/// The table itself stays private: it is searched in both directions by
/// [`Atom::from_name`] and [`Atom::name`], and a second way to reach it would
/// be a second way for it to be read inconsistently.
pub fn atom_names() -> impl Iterator<Item = (&'static str, Atom)> {
    ATOM_NAMES.iter().copied()
}

/// Parse a key name inside `<…>` (`Esc`, `Enter`, `Space`, `Tab`, an arrow, or
/// a single char). `whole` is the full chord, for the error message.
fn parse_keyname(name: &str, whole: &str) -> Result<KeyCode, String> {
    if let Some((_, code)) = KEY_NAMES.iter().find(|(n, _)| *n == name) {
        return Ok(*code);
    }
    let mut cs = name.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) => Ok(KeyCode::Char(c)),
        _ => Err(format!("unknown key name `{name}` in `{whole}`")),
    }
}

/// Parse a config chord string to a [`Chord`]: a bare single char (`j`, `$`,
/// `K`) or a bracketed form (`<C-w>`, `<Esc>`, `<S-l>`). Modifiers are `C`
/// (control) and `S` (shift); shift folds into the glyph to match
/// [`chord_of`].
fn parse_chord(s: &str) -> Result<Chord, String> {
    if let Some(inner) = s.strip_prefix('<').and_then(|x| x.strip_suffix('>')) {
        let segs: Vec<&str> = inner.split('-').collect();
        let (mods, key) = segs.split_at(segs.len().saturating_sub(1));
        let key = key.first().copied().unwrap_or("");
        let mut ctrl = false;
        let mut shift = false;
        for m in mods {
            match *m {
                "C" => ctrl = true,
                "S" => shift = true,
                other => return Err(format!("unknown modifier `{other}` in `{s}`")),
            }
        }
        let mut code = parse_keyname(key, s)?;
        if shift && let KeyCode::Char(c) = code {
            code = KeyCode::Char(c.to_ascii_uppercase());
        }
        Ok(Chord { code, ctrl })
    } else {
        let mut cs = s.chars();
        match (cs.next(), cs.next()) {
            (Some(c), None) => Ok(Chord {
                code: KeyCode::Char(c),
                ctrl: false,
            }),
            _ => Err(format!(
                "not a key chord: `{s}` (use a single char or `<…>`)"
            )),
        }
    }
}

impl Bindings {
    /// Build the active table: the canonical [`DEFAULTS`] with the user's
    /// config overrides applied on top. Returns the merged table and any
    /// warnings — an unknown context/action or a malformed chord is *skipped
    /// with a warning*, never a hard error (forward-compat over strictness, the
    /// config doctrine). Context names: `motion` (cursor/operator-target keys),
    /// `goto` (after `g`), `normal`, `visual`, `window` (after `<C-w>`),
    /// `text-object`.
    pub fn with_overrides(
        raw: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> (Bindings, Vec<String>) {
        let mut map = DEFAULTS.map.clone();
        let mut warnings = Vec::new();
        for (ctx_name, chords) in raw {
            let Some(ctx) = parse_context(ctx_name) else {
                warnings.push(format!("keymap: unknown context `{ctx_name}`"));
                continue;
            };
            for (chord_str, atom_name) in chords {
                let chord = match parse_chord(chord_str) {
                    Ok(c) => c,
                    Err(e) => {
                        warnings.push(format!("keymap [{ctx_name}] `{chord_str}`: {e}"));
                        continue;
                    }
                };
                match Atom::from_name(atom_name) {
                    Some(atom) => {
                        map.insert((ctx, chord), atom);
                    }
                    None => warnings.push(format!(
                        "keymap [{ctx_name}] `{chord_str}`: unknown action `{atom_name}`"
                    )),
                }
            }
        }
        (Bindings { map }, warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every `z` key a fold reads is bound after `z`, to its verb** — the
    /// keymap built from `fold::Z_KEYS`, the list `FoldVerb::after_z` reads,
    /// so a key added there reaches the transcript and `:models` alike
    /// (review stop 15, D L9: the two were listed apart).
    #[test]
    fn every_fold_key_is_bound_to_its_verb() {
        for &(key, verb) in &crate::fold::Z_KEYS {
            let bound = DEFAULTS.get(Context::AfterZ, cmd(KeyCode::Char(key)));
            assert_eq!(bound, Some(Atom::Fold(verb)), "z{key}");
            assert_eq!(crate::fold::FoldVerb::after_z(key), Some(verb), "z{key}");
        }
    }

    /// **What `:keymap` prints can be pasted back into a config.**
    ///
    /// The listing exists so a user can find an action name and bind it, which
    /// means every chord in it is a promise about a spelling the parser
    /// accepts. `chord_name` is the inverse of `parse_chord` and inverses are
    /// exactly where a pair of hand-written matches drift — so this drives the
    /// whole default table through both directions rather than sampling a few.
    #[test]
    fn every_printed_chord_parses_back_to_itself() {
        let mut checked = 0;
        for (_, chord) in DEFAULTS.map.keys() {
            let printed = chord_name(*chord);
            assert_eq!(
                parse_chord(&printed),
                Ok(*chord),
                "`{printed}` is what :keymap shows for {chord:?}, and it does not parse back"
            );
            // **Round-tripping is not enough to be readable.** A bare space
            // parses back perfectly and draws as an empty column, so dropping
            // `Space` from `KEY_NAMES` would leave `window-toggle-mark` next to
            // nothing at all — a key the listing appears to say does not exist.
            assert!(
                !printed.trim().is_empty() && !printed.contains(' '),
                "{chord:?} prints as {printed:?}, which is not something a user can read or type"
            );
            checked += 1;
        }
        assert!(
            checked > 50,
            "premise: the table is populated ({checked} chords)"
        );
        // The named keys are the interesting half — a bare glyph round-trips
        // trivially, so a gate that only saw glyphs would be measuring nothing.
        let named = DEFAULTS
            .map
            .keys()
            .filter(|(_, c)| key_name(c.code).is_some() || c.ctrl)
            .count();
        assert!(
            named > 5,
            "premise: the table binds named/ctrl chords ({named})"
        );
    }

    /// **Every context can be written in a config and printed back** — and
    /// the manual names each, and counts them right.
    ///
    /// `parse_context` searches `config_name`, so they cannot disagree by
    /// construction; `CONTEXTS` is complete by the compiler. What was not
    /// held: the manual. A pinned `len() == 10` failed a context added
    /// properly and said nothing of one the manual leaves out (review stop
    /// 12, T6 10); its block is read instead — every name in it, and its
    /// count word the list's length.
    #[test]
    fn every_context_round_trips_through_its_config_name() {
        let page = crate::help::compose(&crate::command::commands_chapter());
        let words = [
            "zero", "one", "two", "three", "four", "five", "six", "seven", "eight",
        ];
        let words = [&words[..], &["nine", "ten", "eleven", "twelve", "thirteen"]].concat();
        let counted = format!("The {} contexts:", words[CONTEXTS.len()]);
        let at = page
            .find(&counted)
            .unwrap_or_else(|| panic!("the manual says {counted:?} — it counts the contexts"));
        let block: Vec<&str> = (page[at + counted.len()..].lines())
            .skip_while(|l| l.trim().is_empty())
            .take_while(|l| !l.trim().is_empty())
            .flat_map(str::split_whitespace)
            .collect();
        for ctx in CONTEXTS.iter() {
            assert!(
                block.contains(&ctx.config_name()),
                "the manual names `{}`: {block:?}",
                ctx.config_name()
            );
        }
        for ctx in CONTEXTS.iter() {
            assert_eq!(parse_context(ctx.config_name()), Some(*ctx));
            assert!(
                !ctx.describe().is_empty(),
                "{ctx:?} has no description for `:keymap`"
            );
        }
    }

    /// **The listing shows the keys that are actually bound.**
    ///
    /// Built from the table rather than compared against a copy: for every
    /// chord in a context, its printed spelling and its action name must appear.
    #[test]
    fn the_listing_shows_every_bound_key_and_its_action_name() {
        let text = listing(None).expect("no filter is always valid");
        for ctx in CONTEXTS.iter() {
            assert!(
                text.contains(&format!("[keymap.{}]", ctx.config_name())),
                "no section for {ctx:?}"
            );
        }
        for ((ctx, chord), atom) in DEFAULTS.map.iter() {
            let name = atom.name().expect("every default atom is nameable");
            assert!(
                text.contains(name),
                "{ctx:?} binds {chord:?} to `{name}` and the listing never says so"
            );
        }
        // The reason the command exists: a name reachable only by reading this
        // file. `scroll-page-up` is the one the review named.
        assert!(text.contains("scroll-page-up"), "{text}");
    }

    /// One context on its own, and a wrong name that says what the right ones
    /// are.
    #[test]
    fn a_bad_keymap_context_names_the_good_ones() {
        let one = listing(Some("window")).expect("`window` is a context");
        assert!(one.contains("[keymap.window]"));
        assert!(
            !one.contains("[keymap.insert]"),
            "`:keymap window` must show ONE context"
        );
        let err = listing(Some("wndow")).expect_err("`wndow` is not a context");
        for ctx in CONTEXTS.iter() {
            assert!(
                err.contains(ctx.config_name()),
                "a refusal that does not list the alternatives is a riddle: {err}"
            );
        }
    }

    #[test]
    fn every_default_atom_has_a_round_trip_name() {
        // Every atom the defaults bind must be nameable (so it's remappable and
        // dumpable), and the name must round-trip. Catches a missing or
        // mistyped entry in `ATOM_NAMES`.
        for (key, atom) in DEFAULTS.map.iter() {
            let name = atom.name().unwrap_or_else(|| {
                panic!("default-bound atom {atom:?} (at {key:?}) has no config name")
            });
            assert_eq!(
                Atom::from_name(name),
                Some(*atom),
                "name `{name}` does not round-trip to its atom",
            );
        }
    }

    #[test]
    fn atom_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (name, _) in ATOM_NAMES.iter() {
            assert!(seen.insert(*name), "duplicate atom name `{name}`");
        }
    }

    #[test]
    fn parse_chord_handles_plain_ctrl_shift_named() {
        let plain = |c| Chord {
            code: KeyCode::Char(c),
            ctrl: false,
        };
        assert_eq!(parse_chord("j"), Ok(plain('j')));
        assert_eq!(parse_chord("$"), Ok(plain('$')));
        assert_eq!(
            parse_chord("<C-w>"),
            Ok(Chord {
                code: KeyCode::Char('w'),
                ctrl: true
            })
        );
        assert_eq!(
            parse_chord("<Esc>"),
            Ok(Chord {
                code: KeyCode::Esc,
                ctrl: false
            })
        );
        // Shift folds into the glyph (matches `chord_of`): `<S-l>` == `L`.
        assert_eq!(parse_chord("<S-l>"), Ok(plain('L')));
        assert!(parse_chord("foo").is_err());
        assert!(parse_chord("<C-Bogus>").is_err());
    }

    #[test]
    fn overrides_remap_and_warn() {
        let map = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>()
        };
        let mut raw = BTreeMap::new();
        raw.insert("motion".to_string(), map(&[("j", "word-forward")]));
        raw.insert("normal".to_string(), map(&[("q", "no-such-action")]));
        raw.insert("bogus".to_string(), BTreeMap::new());
        let (bindings, warnings) = Bindings::with_overrides(&raw);
        // `j` (a motion) now resolves to word-forward; `k` is untouched.
        let plain = |c| Chord {
            code: KeyCode::Char(c),
            ctrl: false,
        };
        assert_eq!(
            bindings.get(Context::Motion, plain('j')),
            Some(Atom::Motion(CursorMotion::WordForward))
        );
        assert_eq!(
            bindings.get(Context::Motion, plain('k')),
            Some(Atom::Motion(CursorMotion::Up))
        );
        // The unknown action and unknown context each warned.
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn insert_context_override_binds_submit() {
        // The rebind story end-to-end: a `[keymap.insert]` table parses (the
        // `insert` context name) and `<C-d>` → submit resolves in Insert.
        let mut raw = BTreeMap::new();
        let mut t = BTreeMap::new();
        t.insert("<C-d>".to_string(), "submit".to_string());
        raw.insert("insert".to_string(), t);
        let (bindings, warnings) = Bindings::with_overrides(&raw);
        assert!(warnings.is_empty(), "clean override should not warn");
        let cd = Chord {
            code: KeyCode::Char('d'),
            ctrl: true,
        };
        assert_eq!(bindings.get(Context::Insert, cd), Some(Atom::Submit));
        // The default Insert submit (Ctrl+Enter) is still there.
        let center = Chord {
            code: KeyCode::Enter,
            ctrl: true,
        };
        assert_eq!(bindings.get(Context::Insert, center), Some(Atom::Submit));
    }

    /// **`<C-]>` arrives as a terminal sends it**: the byte 0x1D reads as
    /// CTRL with `5`, and a terminal reporting keys in full sends the `]`.
    /// Both are one key, and each of the link keys can be remapped.
    #[test]
    fn the_link_keys_arrive_however_a_terminal_sends_them() {
        let chord = |c| Chord {
            code: KeyCode::Char(c),
            ctrl: true,
        };
        let (bindings, _) = Bindings::with_overrides(&BTreeMap::new());
        let follow = Some(Atom::Window(WindowCmd::FollowLink));
        assert_eq!(bindings.get(Context::Normal, chord('5')), follow);
        assert_eq!(bindings.get(Context::Normal, chord(']')), follow);
        assert_eq!(
            bindings.get(Context::Normal, chord('t')),
            Some(Atom::Window(WindowCmd::LinkBack))
        );
        assert_eq!(Atom::from_name("follow-link"), follow);
        assert_eq!(
            Atom::from_name("link-back"),
            Some(Atom::Window(WindowCmd::LinkBack))
        );
    }
}
