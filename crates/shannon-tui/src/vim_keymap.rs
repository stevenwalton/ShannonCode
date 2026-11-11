//! Vim-flavored modal keymap.
//!
//! Three modes are wired here — **Normal**, **Insert**, and **Visual**
//! (charwise; `v` enters, motions extend the selection, `d`/`c`/`x`
//! operate, `Esc`/`v` leave). One `VimKeymap` instance drives one pane:
//! - The **prompt** pane allows Insert (`allow_insert = true`) and can
//!   submit.
//! - The **chat** pane does not (`allow_insert = false`): it's a viewer,
//!   so Normal mode there only scrolls.
//!
//! Submit semantics (a confirmed UX decision): in **Insert**, Enter is a
//! newline (vim-consistent) and **Alt-Enter submits** without leaving
//! Insert; in **Normal**, Enter submits. So an accidental Enter while
//! typing a multi-line prompt never fires an API call.
//!
//! Normal mode in the prompt has the **motion baseline** plus the
//! insert-entry family: `i`/`a`/`A`/`I`/`o`/`O` to enter Insert (each
//! repositioning the cursor as in vim), Enter to submit, and cursor
//! motions — `hjkl` + arrows, `w`/`b`/`e`, `0`/`$`, `G`/`gg`. The chat
//! viewer's Normal mode scrolls instead (`j`/`k`/`Ctrl-U`/`Ctrl-D`).
//!
//! **The pending/chord engine.** Normal-mode commands can be multi-key.
//! `Pending` is a small state machine carried on the keymap between
//! keystrokes: `Root` (no operator — a count and/or a motion prefix may be
//! open) or `Operator` (a captured `d`/`c` plus what it's still `after`).
//! The chord nests, so a motion prefix (`g`) is completed by the *same*
//! `resolve_motion` table whether it's a cursor `gg` or an operator
//! `dgg` — one motion machine, no duplication. `x`/`X` delete a char.
//!
//! **Operators.** `d` (delete), `c` (change), and `y` (yank) capture and
//! await a target. Charwise motions give a within-line range —
//! `dw`/`db`/`de`/`dh`/`dl`/`d0`/`d$` (and the `c`/`y` equivalents), counts
//! multiplying (`2d3w` → six words). The vertical/buffer motions are
//! **linewise**: `dj`/`dk`/`dG`/`dgg` (and `c…`/`y…`) act on whole rows. A
//! doubled operator is linewise too (`dd`, `cc`, `yy`); `D`/`C`/`Y` are the
//! `d$`/`c$`/`yy` shorthands. `c` switches to Insert. The range itself is
//! resolved in `prompt.rs` (only it knows where a motion lands); the
//! keymap just names operator + target + count.
//!
//! **Text objects.** After an operator, `i`/`a` opens a text-object chord
//! whose third key is the object: `ciw`/`caw` (word), `ci"`/`ca'`
//! (quotes), `ci(`/`ca{`/`dib` (brackets, with `b`/`B` aliases). `i` is
//! the content, `a` includes the delimiters/trailing space. These too
//! resolve their range in `prompt.rs` (single-line for now).
//!
//! **Yank & paste.** `y` is the third operator (`yw`/`yy`/`yi"`/`yj`…),
//! plus `Y` (= `yy`) and Visual `y`; it copies into the prompt's register
//! without mutating. `p`/`P` paste it back (after / before the cursor;
//! below / above for a linewise register). The system-clipboard bridge
//! (OSC 52) and bracketed paste are the next step.
//!
//! **Case and replace.** `~` toggles case over a count; `r` arms
//! `Pending::Replace`, and the next key replaces — `count` chars in Normal,
//! the selection in Visual (where `~`/`u`/`U` are in-place operators too);
//! `R` enters `Mode::Replace`, whose keys overwrite and whose `<BS>` puts
//! back what they covered.
//!
//! `Ctrl-U` now means kill-to-line-start in Insert and half-page-up in
//! Normal — the M1 conflict (global scroll stole the insert binding) is
//! resolved by Normal mode owning the scroll.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::bindings::{Atom, Bindings, Chord, Context, DEFAULTS, PrefixId, chord_of};
use crate::keymap::{
    CursorMotion, EditAction, InsertEntry, KeyOutcome, Keymap, Mode, OpTarget, Operator, ScrollCmd,
    SelectionKind, TextObjectScope, Whose, WindowCmd,
};

/// No Ctrl/Alt held — Shift is fine, since Shift is what produced a
/// shifted glyph (`G`, `$`, `A`) or a capital letter in the first place.
/// The predicate behind every "command / printable key, not a control
/// chord" check.
fn plain_or_shifted(mods: KeyModifiers) -> bool {
    mods.is_empty() || mods == KeyModifiers::SHIFT
}

/// **The digit a key adds to a count**, or `None` — vim's fixed rule rather
/// than a binding (counts are not remappable in vim either): a plain `1`–`9`
/// always, and `0` only once a count is in progress, since a leading `0` is
/// the line-start motion.
///
/// One rule for every place a count accumulates — before a motion, after
/// `<C-w>`, inside an operator — and for the decision pane, which forwards a
/// count to the keymap rather than answering it with its legend. It was
/// written out at each of the three keymap sites.
pub(crate) fn count_digit(key: &KeyEvent, counting: bool) -> Option<usize> {
    let KeyCode::Char(c @ '0'..='9') = key.code else {
        return None;
    };
    (plain_or_shifted(key.modifiers) && (c != '0' || counting)).then(|| c as usize - '0' as usize)
}

/// **A count as typed: none, or 1 up to [`MAX_COUNT`]** — vim's `[count]`.
///
/// "None typed" was `0` on a bare `usize` through every state of the chord,
/// and a fold that needed it was one `max(1)` from erasing it: `dG` became
/// `d1G` that way (review stop 16, A M6), and the doc that called one
/// function "the single place a count's meaning is decided" named three,
/// with a fourth since (D 3). None is a state here, not a value, so each
/// reading says which it is — [`typed`](Self::typed) for a count that is a
/// place or a size (`3G`, `50%`, `2gt`, `20zt`), [`repeat`](Self::repeat)
/// for one that repeats (`3j`, at least once), [`times`](Self::times) for an
/// operator's by its target's (`2d3w`; none by none still none) — and a
/// `max(1)` on one does not typecheck.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Count(Option<std::num::NonZeroUsize>);

impl Count {
    /// None typed.
    const NONE: Count = Count(None);

    /// `n` typed, capped; `0` is none.
    fn of(n: usize) -> Count {
        Count(std::num::NonZeroUsize::new(n.min(MAX_COUNT)))
    }

    /// The count with one more digit typed — the arithmetic half of
    /// [`count_digit`], for every place a count accumulates.
    fn push(self, digit: usize) -> Count {
        let so_far = self.typed().unwrap_or(0);
        Count::of(so_far.saturating_mul(10).saturating_add(digit))
    }

    /// Whether one is being typed — a `0` then is a digit, not the line's
    /// start ([`count_digit`]).
    fn counting(self) -> bool {
        self.0.is_some()
    }

    /// The number typed, if one was: a place or a size, whose absence means
    /// something of its own (`G`: the last line).
    fn typed(self) -> Option<usize> {
        self.0.map(std::num::NonZeroUsize::get)
    }

    /// How many times to do it: what was typed, or once.
    fn repeat(self) -> usize {
        self.typed().unwrap_or(1)
    }

    /// This count by `other` — an operator's by its target's, as vim's
    /// `opcount * count0`: none only when neither was typed.
    fn times(self, other: Count) -> Count {
        match (self.typed(), other.typed()) {
            (None, None) => Count::NONE,
            (a, b) => Count::of(a.unwrap_or(1).saturating_mul(b.unwrap_or(1))),
        }
    }
}

/// Upper bound on a Normal-mode count prefix. A typo like `99999999x`
/// should not turn into a multi-second delete loop; vim caps counts too.
/// 100k is far beyond any deliberate count and keeps the apply loop
/// bounded.
const MAX_COUNT: usize = 100_000;

/// A motion prefix key pressed mid-chord, awaiting its completion key:
/// `g` (→ `gg`), `[` (→ `[[`), `]` (→ `]]`). It lives *inside* the chord
/// state so the one motion resolver completes it whether or not an operator
/// is pending — the same machine drives a cursor `gg` and an operator `dgg`,
/// a `]]` and a `y]]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MotionPrefix {
    /// `g` — currently completed only by a second `g` (`gg`).
    G,
    /// `[` — completed by `[` (`[[`).
    OpenBracket,
    /// `]` — completed by `]` (`]]`).
    CloseBracket,
    /// `'` / `` ` `` — completed by a mark's name (`'a`, `` `a ``), which is
    /// no binding: yours, or the agent's.
    Mark(Whose),
    /// `f` `F` `t` `T` — completed by the char to find, any char: no
    /// binding either.
    Find { till: bool, forward: bool },
}

impl MotionPrefix {
    /// The prefix a bound [`PrefixId`] opens on the motion path, or `None`
    /// for a command prefix (`<C-w>`, `z`).
    fn of(id: PrefixId) -> Option<MotionPrefix> {
        match id {
            PrefixId::G => Some(MotionPrefix::G),
            PrefixId::OpenBracket => Some(MotionPrefix::OpenBracket),
            PrefixId::CloseBracket => Some(MotionPrefix::CloseBracket),
            PrefixId::Mark => Some(MotionPrefix::Mark(Whose::Yours)),
            PrefixId::AgentMark => Some(MotionPrefix::Mark(Whose::Agents)),
            PrefixId::Find { till, forward } => Some(MotionPrefix::Find { till, forward }),
            PrefixId::Window | PrefixId::Z => None,
        }
    }

    /// The context its completion key is looked up in — `None` for a mark,
    /// whose name is any letter, and a find, whose char is any char
    /// ([`resolve_motion`]).
    fn context(self) -> Option<Context> {
        match self {
            MotionPrefix::G => Some(Context::AfterG),
            MotionPrefix::OpenBracket => Some(Context::AfterOpenBracket),
            MotionPrefix::CloseBracket => Some(Context::AfterCloseBracket),
            MotionPrefix::Mark(_) | MotionPrefix::Find { .. } => None,
        }
    }
}

/// What an operator (`d`/`c`) is still waiting for once it's captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterOp {
    /// Awaiting the motion target. `count` is the motion's *own* count
    /// (multiplies the operator's, e.g. `2d3w`); `prefix` is an open
    /// motion prefix — the `g` of `dgg`.
    Target {
        count: Count,
        prefix: Option<MotionPrefix>,
    },
    /// `i`/`a` opened a text-object chord; awaiting the object key. `count` is
    /// the object's own count, typed before the `i`/`a` (`d2ap`).
    TextObject {
        scope: TextObjectScope,
        count: Count,
    },
}

/// In-progress Normal-mode chord. A count and a motion prefix can be open
/// at the **root** (a cursor command like `3j` / `gg`) or **inside** an
/// operator (`2d3w` / `dgg`). Nesting the operator means the single motion
/// resolver handles a prefix at either level — that is what makes `dgg`
/// compose without a second `g` state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// No operator captured yet. `count` accumulates a leading count;
    /// `prefix` is an open motion prefix (a bare `g`).
    Root {
        count: Count,
        prefix: Option<MotionPrefix>,
    },
    /// `d`/`c` captured, carrying `op_count`, the count that preceded it.
    Operator {
        op: Operator,
        op_count: Count,
        after: AfterOp,
    },
    /// After a COMMAND prefix — `<C-w>` or `z` — awaiting the command key.
    /// `count` is the count typed before it (`5<C-w>+`, `20zt`); for `<C-w>`
    /// digits typed after the prefix accumulate too (`<C-w>5+`). Distinct
    /// from the motion `g` prefix, which resolves a motion rather than a
    /// command.
    ///
    /// This was `Window { count }`, and its doc said the command prefixes
    /// would unify when a second one landed; `z` is the second, so the kind
    /// became a field rather than a new state with its own copy of the
    /// continuation logic.
    Command { prefix: PrefixId, count: Count },
    /// After `r`, awaiting the char to replace with. In Normal it replaces
    /// `count` chars from the cursor — a repeat, resolved; in Visual, the
    /// selection (count 1). The mode decides which — one state, so `<Esc>`,
    /// `cancel_chord` and `pending_chord` cover both.
    Replace { count: usize },
    /// After `m`, awaiting the mark's name. A state of the chord, so the key
    /// after `m` is the name wherever keys are answered first: `ma` in a
    /// decision pane names a mark and does not allow (`pending_chord`).
    SetMark,
}

impl Default for Pending {
    fn default() -> Self {
        Pending::Root {
            count: Count::NONE,
            prefix: None,
        }
    }
}

/// The result of feeding one key to the shared motion resolver.
enum MotionResolution {
    /// A complete motion resolved this keystroke.
    Complete(CursorMotion),
    /// A prefix that needs one more key (`g`).
    Prefix(MotionPrefix),
    /// Not a motion key.
    None,
}

/// The result of one step of the shared cursor-motion chord accumulator
/// ([`VimKeymap::step_motion`]) — the machine that turns `3`, `g`, `j`, `$`,
/// `gg`, `3w` into a counted cursor motion. Normal-root commands and *all*
/// Visual modes run the same accumulator, so a motion behaves identically in
/// each (there is one cursor-motion machine, not a per-mode reimplementation
/// — the divergence that let `G` work but `gg` not in Visual).
enum MotionChord {
    /// A digit or motion prefix was absorbed (or a prefix completion failed
    /// and ate the key) — the chord continues / cancels; emit `Ignored`.
    Consumed,
    /// A motion resolved with this repeat (≥ 1).
    Resolved(CursorMotion, usize),
    /// Not a motion — the caller handles the key as a command, with the
    /// count typed (each command reads it as it means it, and a command
    /// prefix keeps "none" for `<C-w>5+`).
    Command(Count),
    /// **A motion prefix completed into a window command** — `gt`, `gT`,
    /// the tab pages (vim's `CTRL-W gt` is `gt`) — with the count typed
    /// before it (`2gt`), as `<C-w>`'s chords carry theirs. Every caller
    /// emits it as the window command it is.
    Window(WindowCmd, Option<usize>),
}

/// **The** key → motion resolver — now backed by the `bindings` atom table
/// (`Context::Motion` for the root keys, a prefix's own context to complete
/// it). Still the single source of truth shared by cursor motions and
/// operator targets, so the two can never drift. When `prefix` is set the
/// key completes that prefix instead. Motions are plain/shifted chords
/// (`cmd`); a control chord is never a motion.
fn resolve_motion(
    bindings: &Bindings,
    prefix: Option<MotionPrefix>,
    code: KeyCode,
    cmd: bool,
) -> MotionResolution {
    if !cmd {
        return MotionResolution::None;
    }
    // **A mark's name is not a binding** — any lowercase letter, vim's
    // fixed rule (`:h mark-motions`), as a count's digits are
    // ([`count_digit`]). Uppercase and the special marks (`''`) come later.
    if let Some(MotionPrefix::Mark(whose)) = prefix {
        return match code {
            KeyCode::Char(name @ 'a'..='z') => {
                MotionResolution::Complete(CursorMotion::Mark(whose, name))
            }
            _ => MotionResolution::None,
        };
    }
    // **A find's char is any char** (`fx`, `t)`, `f<Tab>`), as a mark's
    // name is no binding.
    if let Some(MotionPrefix::Find { till, forward }) = prefix {
        let ch = match code {
            KeyCode::Char(ch) => ch,
            KeyCode::Tab => '\t',
            _ => return MotionResolution::None,
        };
        return MotionResolution::Complete(CursorMotion::FindChar(crate::keymap::Find {
            till,
            forward,
            ch,
            count: 1,
            again: false,
        }));
    }
    let ctx = prefix
        .and_then(MotionPrefix::context)
        .unwrap_or(Context::Motion);
    match bindings.get(ctx, Chord { code, ctrl: false }) {
        Some(Atom::Motion(m)) => MotionResolution::Complete(m),
        Some(Atom::Prefix(id)) => {
            MotionPrefix::of(id).map_or(MotionResolution::None, MotionResolution::Prefix)
        }
        _ => MotionResolution::None,
    }
}

/// **A motion's count, as the motion reads it** — on the cursor path and the
/// operator's (`d3G`, the operator's count multiplied in first), so the two
/// cannot diverge. Returns `(motion, repeat ≥ 1)`. A command's count is read
/// where the command is ([`execute_atom`](VimKeymap::execute_atom),
/// [`command_key`](VimKeymap::command_key)); [`Count`] is what keeps each
/// reading saying which it is.
///
/// The motions' readings: **repeat** (`3j`), the default; **absolute line**
/// (`{count}G`/`{count}gg` → `GotoLine`; none typed, the last/first line);
/// **distance from the screen edge** (`3H`, `3L` — the count lands IN the
/// variant, repeat 1, because "the third line from the top" three times is
/// still the third line); **one motion of n** (`3]]`, `2tx`, `3}`: it fails
/// whole where vim's does); a **percentage** (`50%`; none, the matching
/// pair); and **ignored** (`M` and a mark take none, `:h M`).
fn apply_count_to_motion(m: CursorMotion, count: Count) -> (CursorMotion, usize) {
    match (m, count.typed()) {
        (CursorMotion::BufferStart | CursorMotion::BufferEnd, Some(line)) => {
            (CursorMotion::GotoLine(line), 1)
        }
        // `50%` is a line, half way down (`:h N%`); bare, the matching pair.
        // Past 100 it fails where it lands (`Buffer::landing`), as vim's
        // beeps.
        (CursorMotion::MatchPair, Some(percent)) => (CursorMotion::Percent(percent), 1),
        _ => count_in(m, count.repeat()),
    }
}

/// [`apply_count_to_motion`] for a motion whose count is a number of times,
/// `n` (≥ 1): in the motion, or as the repeat.
fn count_in(m: CursorMotion, n: usize) -> (CursorMotion, usize) {
    use crate::viewport::ScreenLine;
    match m {
        CursorMotion::Screen(ScreenLine::Top(_)) => (CursorMotion::Screen(ScreenLine::Top(n)), 1),
        CursorMotion::Screen(ScreenLine::Bottom(_)) => {
            (CursorMotion::Screen(ScreenLine::Bottom(n)), 1)
        }
        CursorMotion::Screen(ScreenLine::Middle) => (m, 1),
        // A place is one place: `3'a` is `'a` (vim takes no count there).
        CursorMotion::Mark(..) => (m, 1),
        // `3]]`: three sections in one motion, which fails whole if they run
        // out (`:h ]]`) — three repeats would stop at the edge instead.
        CursorMotion::Section { forward, end, .. } => (
            CursorMotion::Section {
                forward,
                end,
                count: n,
            },
            1,
        ),
        // `2tx` is "till before the SECOND x": two repeats of `tx` stand
        // still before the first. A paragraph's and a `;`'s count likewise
        // fail whole, as vim's do (`findpar`, `searchc`).
        CursorMotion::FindChar(find) => (
            CursorMotion::FindChar(crate::keymap::Find { count: n, ..find }),
            1,
        ),
        CursorMotion::RepeatFind { reverse, .. } => {
            (CursorMotion::RepeatFind { reverse, count: n }, 1)
        }
        CursorMotion::Paragraph { forward, .. } => {
            (CursorMotion::Paragraph { forward, count: n }, 1)
        }
        _ => (m, n),
    }
}

/// Whether a motion is charwise as an operator target — those resolve to a
/// `Motion` span: the ones within a line, and `]]` `[[`, exclusive across
/// lines, whose span the prompt shapes by vim's exclusive rules (they turn
/// linewise only when `:h exclusive-linewise` says so). The vertical/buffer
/// and screen motions (`j`/`k`/`gg`/`G`/`{n}G`/`H`/`M`/`L`) are linewise as
/// operator targets and are routed to `OpTarget::LineMotion` instead.
///
/// A `match`, not a `matches!`: a new motion is classed here before it
/// compiles. It was a `matches!`, so one silently became linewise, and
/// dropping `0` and `$` from it — `d$` deleting the whole line — passed
/// every test (review stop 12, T6 3).
fn is_charwise(m: CursorMotion) -> bool {
    match m {
        CursorMotion::Left
        | CursorMotion::Right
        | CursorMotion::WordForward
        | CursorMotion::WordBackward
        | CursorMotion::WordEnd
        | CursorMotion::BigWordForward
        | CursorMotion::BigWordBackward
        | CursorMotion::BigWordEnd
        | CursorMotion::LineStart
        | CursorMotion::LineEnd
        | CursorMotion::Section { .. }
        // `` ` `` is vim's exact place, exclusive; `'` its line (below).
        | CursorMotion::Mark(Whose::Agents, _)
        // Along the line (`:h f`, `:h ^`, `:h ge`), and `}` `%` across
        // lines, charwise as vim's are (`:h }`, `:h %`).
        | CursorMotion::FindChar(_)
        | CursorMotion::RepeatFind { .. }
        | CursorMotion::FirstNonBlank
        | CursorMotion::WordEndBackward { .. }
        | CursorMotion::Paragraph { .. }
        | CursorMotion::MatchPair => true,
        CursorMotion::Up
        | CursorMotion::Down
        | CursorMotion::BufferStart
        | CursorMotion::BufferEnd
        | CursorMotion::GotoLine(_)
        | CursorMotion::Percent(_)
        | CursorMotion::Screen(_)
        | CursorMotion::Mark(Whose::Yours, _) => false,
    }
}

/// A doubled operator (`dd`/`cc`/`yy`) — the operator key pressed again,
/// which means the linewise whole-line form.
fn doubled_operator(op: Operator, code: KeyCode) -> bool {
    matches!(
        (op, code),
        (Operator::Delete, KeyCode::Char('d'))
            | (Operator::Change, KeyCode::Char('c'))
            | (Operator::Yank, KeyCode::Char('y'))
    )
}

#[derive(Debug, Clone, Copy)]
pub struct VimKeymap {
    mode: Mode,
    /// Whether this pane has an Insert mode and can submit. The prompt
    /// does; the chat viewer does not.
    allow_insert: bool,
    /// Normal-mode count/prefix chord in progress (prompt editor only;
    /// the chat viewer never accumulates one).
    pending: Pending,
    /// The atom table this keymap resolves keys against — injected so a config
    /// remap (or a test) can supply its own. Defaults to the canonical
    /// [`DEFAULTS`]; the app installs the config-merged table via
    /// [`using`](Self::using).
    bindings: &'static Bindings,
    /// Mid-`Ctrl-X` of the `Ctrl-X Ctrl-E` `$EDITOR` chord (readline's binding).
    /// Set by a `Ctrl-X`; the next key either completes it (`Ctrl-E` → open the
    /// editor) or cancels it and is handled normally. Mode-independent.
    editor_chord: bool,
    /// Mid-`i`/`a` of a Visual text-object chord (`viw`/`vi"`/`va(`): the scope
    /// is armed by the `i`/`a`, with the count typed before it (`v2ap`), and
    /// the next key resolves the object. A Visual-only mini-state, separate
    /// from the Normal/operator `Pending` chord.
    visual_object: Option<(TextObjectScope, Count)>,
}

impl VimKeymap {
    /// Keymap for the prompt pane, starting in `start_mode`.
    pub fn prompt(start_mode: Mode) -> Self {
        Self {
            mode: start_mode,
            allow_insert: true,
            pending: Pending::default(),
            bindings: &DEFAULTS,
            editor_chord: false,
            visual_object: None,
        }
    }

    /// Keymap for the chat pane — a Normal-only viewer (no Insert,
    /// no submit).
    pub fn chat() -> Self {
        Self {
            mode: Mode::Normal,
            allow_insert: false,
            pending: Pending::default(),
            bindings: &DEFAULTS,
            editor_chord: false,
            visual_object: None,
        }
    }

    /// Resolve keys against `bindings` instead of [`DEFAULTS`] — the app passes
    /// the config-merged table ([`bindings::active`](crate::bindings::active));
    /// a test passes its own.
    pub fn using(mut self, bindings: &'static Bindings) -> Self {
        self.bindings = bindings;
        self
    }

    fn normal(&mut self, key: KeyEvent) -> KeyOutcome {
        // The editable prompt and the chat viewer share Normal mode but
        // diverge on what the navigation keys do: in the prompt they move
        // the *cursor*; in the viewer they scroll the *view*.
        if self.allow_insert {
            self.normal_editor(key)
        } else {
            self.normal_viewer(key)
        }
    }

    /// Normal mode in the editable prompt. The chord state decides how to
    /// read this key: a fresh command at the root (`3j`, `gg`, `dw`, `x`,
    /// insert-entry, …) or the continuation of a pending operator. `Pending`
    /// is `Copy`, so we read it out and let the handlers reset/advance it.
    fn normal_editor(&mut self, key: KeyEvent) -> KeyOutcome {
        match self.pending {
            Pending::Root { count, prefix } => self.root_key(key, count, prefix),
            Pending::Operator {
                op,
                op_count,
                after,
            } => self.operator_key(key, op, op_count, after),
            Pending::Command { prefix, count } => self.command_key(key, prefix, count),
            Pending::Replace { count } => self.replace_char_key(key, count),
            Pending::SetMark => self.set_mark_key(key),
        }
    }

    /// The key after `m`: the mark's name, a lowercase letter; anything else
    /// cancels, as vim's beep does. Uppercase marks wait for after the soft
    /// launch (`marks.md` Q2).
    fn set_mark_key(&mut self, key: KeyEvent) -> KeyOutcome {
        self.pending = Pending::default();
        match key.code {
            KeyCode::Char(name @ 'a'..='z') if plain_or_shifted(key.modifiers) => {
                KeyOutcome::SetMark(name)
            }
            _ => KeyOutcome::Ignored,
        }
    }

    /// **The name an `m` waits for, ahead of every other reading of the key**
    /// — in the viewer's Normal and in both Visuals, whose own readings come
    /// after it: in Visual the `a` of `ma` arms a text object
    /// (`[[feedback_chord_owns_its_keys]]`). `None` with no `m` waiting.
    fn waiting_mark(&mut self, key: KeyEvent) -> Option<KeyOutcome> {
        matches!(self.pending, Pending::SetMark).then(|| self.set_mark_key(key))
    }

    /// The key after `r`: the char to replace with. A printable key, or
    /// `Enter` for a line break; anything else cancels, as in vim. In Visual
    /// it replaces the selection and leaves Visual — except `r<CR>`, where vim
    /// and nvim put a literal carriage return (`^M`) in place of every
    /// selected char; that cancels here instead (Steven, 10-02: okay not to
    /// copy it) — and SAYS so, since a deliberate gap that answers with
    /// silence reads as a broken key.
    fn replace_char_key(&mut self, key: KeyEvent, count: usize) -> KeyOutcome {
        self.pending = Pending::default();
        let with = match key.code {
            KeyCode::Char(c) if plain_or_shifted(key.modifiers) => c,
            KeyCode::Enter if plain_or_shifted(key.modifiers) => '\n',
            _ => return KeyOutcome::Ignored,
        };
        if self.mode.is_visual() {
            if with == '\n' {
                return KeyOutcome::Refused(
                    "r<Enter> over a selection is not supported (vim fills it with ^M) — `c` then Enter breaks it",
                );
            }
            self.mode = Mode::Normal;
            return self.emit_operator(Operator::Replace(with), OpTarget::Selection, 1);
        }
        KeyOutcome::edit(EditAction::ReplaceChars { with, count })
    }

    /// A key while a command prefix is pending — `<C-w>` or `z`. The command
    /// key is looked up in the prefix's own context (`AfterWindow`, `AfterZ`)
    /// and emitted for the app to apply. Anything unrecognized cancels
    /// (vim-like); the chord ends either way.
    ///
    /// **Digits after the prefix mean different things per prefix**, so they
    /// are accumulated only for `<C-w>`, where `<C-w>5+` is the same count as
    /// `5<C-w>+`. After `z` a number is vim's `z{height}<CR>` — a window
    /// HEIGHT, which we do not support — so it cancels rather than being read
    /// as a line and landing somewhere else.
    fn command_key(&mut self, key: KeyEvent, prefix: PrefixId, count: Count) -> KeyOutcome {
        if prefix == PrefixId::Window
            && let Some(digit) = count_digit(&key, count.counting())
        {
            self.pending = Pending::Command {
                prefix,
                count: count.push(digit),
            };
            return KeyOutcome::Ignored;
        }
        self.pending = Pending::default();
        let context = match prefix {
            PrefixId::Window => Context::AfterWindow,
            PrefixId::Z => Context::AfterZ,
            PrefixId::G => Context::AfterG,
            PrefixId::OpenBracket => Context::AfterOpenBracket,
            PrefixId::CloseBracket => Context::AfterCloseBracket,
            // A motion prefix, never a command's; and its completion is no
            // table's ([`resolve_motion`]).
            PrefixId::Mark | PrefixId::AgentMark | PrefixId::Find { .. } => {
                return KeyOutcome::Ignored;
            }
        };
        let typed = count.typed();
        match chord_of(key).and_then(|c| self.bindings.get(context, c)) {
            Some(Atom::Window(cmd)) => KeyOutcome::Window(cmd, typed),
            Some(Atom::Fold(verb)) => KeyOutcome::Fold(verb),
            // `20zt`: the count is the LINE to put there (`:h zt`).
            Some(Atom::Scroll(ScrollCmd::Place {
                at,
                first_non_blank,
                ..
            })) => KeyOutcome::Scroll(ScrollCmd::Place {
                at,
                line: typed,
                first_non_blank,
            }),
            _ => KeyOutcome::Ignored,
        }
    }

    /// Feed one key to the shared cursor-motion chord accumulator, advancing
    /// `self.pending`. The single place counts, the `g` prefix, and a
    /// resolved motion are accumulated for the *cursor* (Normal root + every
    /// Visual mode); the operator chord ([`operator_key`](Self::operator_key))
    /// is its own machine because a resolved motion there becomes an operator
    /// target, with text-object and doubled-operator states interleaved.
    fn step_motion(
        &mut self,
        key: KeyEvent,
        count: Count,
        prefix: Option<MotionPrefix>,
    ) -> MotionChord {
        let cmd = plain_or_shifted(key.modifiers);

        // Completing a bare motion prefix (`g` → `gg`); anything else cancels
        // and is eaten. Either way the chord ends. A completion bound to a
        // window command (`gt`, `gT`) is that command, its count with it —
        // here alone: inside an operator (`dgt`) it has nothing to act on and
        // cancels, as `operator_key`'s completion does with no motion. In a
        // read-only pane no operator begins — its `d` is refused and ends the
        // chord at once — so the `gt` after it is a `gt` (vim swallows the
        // whole `d{target}` and refuses it; docketed, r12-F L4).
        if let Some(p) = prefix {
            self.pending = Pending::default();
            if let Some(Atom::Window(window)) =
                (p.context().zip(chord_of(key))).and_then(|(ctx, c)| self.bindings.get(ctx, c))
            {
                return MotionChord::Window(window, count.typed());
            }
            return match resolve_motion(self.bindings, Some(p), key.code, cmd) {
                MotionResolution::Complete(m) => {
                    let (m, n) = apply_count_to_motion(m, count);
                    MotionChord::Resolved(m, n)
                }
                _ => MotionChord::Consumed, // `gx` cancels
            };
        }

        // Count digits accumulate. `0` is a digit only once a count is in
        // progress; a leading `0` is the line-start motion (below).
        if let Some(digit) = count_digit(&key, count.counting()) {
            self.pending = Pending::Root {
                count: count.push(digit),
                prefix: None,
            };
            return MotionChord::Consumed;
        }

        // A motion (which may open a prefix) via the shared resolver.
        match resolve_motion(self.bindings, None, key.code, cmd) {
            MotionResolution::Complete(m) => {
                self.pending = Pending::default();
                let (m, n) = apply_count_to_motion(m, count);
                MotionChord::Resolved(m, n)
            }
            MotionResolution::Prefix(p) => {
                self.pending = Pending::Root {
                    count,
                    prefix: Some(p),
                };
                MotionChord::Consumed
            }
            MotionResolution::None => {
                self.pending = Pending::default();
                // The count as typed; the command reads it as it means it. A
                // command prefix like `<C-w>` carries "none", so a digit typed
                // *after* it (`<C-w>5+`) starts the count, not a defaulted 1.
                MotionChord::Command(count)
            }
        }
    }

    /// A key with no operator pending: a counted cursor motion (via the
    /// shared accumulator) or a fresh Normal command.
    fn root_key(
        &mut self,
        key: KeyEvent,
        count: Count,
        prefix: Option<MotionPrefix>,
    ) -> KeyOutcome {
        match self.step_motion(key, count, prefix) {
            MotionChord::Consumed => KeyOutcome::Ignored,
            MotionChord::Resolved(m, n) => KeyOutcome::Edit {
                action: EditAction::Move(m),
                count: n,
            },
            MotionChord::Command(n) => self.root_command(key, n),
            MotionChord::Window(cmd, n) => KeyOutcome::Window(cmd, n),
        }
    }

    /// A non-motion Normal command — the chord has ended and the motion
    /// resolver declined the key, so look it up in `Context::Normal` and
    /// `execute_atom` it, with the count typed. Submit is now a table atom too
    /// (`Enter` and `Ctrl+Enter` → `Submit`, rebindable — criterion 11).
    fn root_command(&mut self, key: KeyEvent, n: Count) -> KeyOutcome {
        match chord_of(key).and_then(|c| self.bindings.get(Context::Normal, c)) {
            Some(atom) => self.execute_atom(atom, n),
            None => KeyOutcome::Ignored,
        }
    }

    /// Turn a resolved command/visual [`Atom`] into a [`KeyOutcome`], applying
    /// its mode/pending side effect — the engine half of the engine/atoms
    /// split. `count` is the chord's count as typed, in every mode; each atom
    /// reads it as it means it — a repeat (`n`), a size or place
    /// (`typed()`), or the operator's own to multiply. The Visual operators
    /// apply once, whatever was typed. `Motion`/`Prefix(G)`/`Object` never
    /// arrive here.
    fn execute_atom(&mut self, atom: Atom, count: Count) -> KeyOutcome {
        use Atom::*;
        // The repeat for atoms that repeat (≥ 1).
        let n = count.repeat();
        match atom {
            Edit(action) => KeyOutcome::Edit { action, count: n },
            Enter(entry) => self.enter_insert(entry, n),
            EnterVisual(kind) => self.enter_visual(kind),
            // As typed: `G` and `%` read "none typed" as their own meaning,
            // so folded here `dG` was `d1G` (review stop 16, A M6).
            BeginOperator(op) => self.begin_operator(op, count),
            // `x`/`X` are vim's `dl`/`dh` — routed through the operator path so
            // they *cut* into the register (`xp` transposes) and share its EOL
            // handling (no spurious line-merge past end-of-line).
            DeleteCharAfter => {
                self.emit_operator(Operator::Delete, OpTarget::Motion(CursorMotion::Right), n)
            }
            DeleteCharBefore => {
                self.emit_operator(Operator::Delete, OpTarget::Motion(CursorMotion::Left), n)
            }
            YankLine => self.emit_operator(Operator::Yank, OpTarget::Line, n),
            // `D`/`C` = `d$`/`c$`, count-aware: the count rides in `LineTail`
            // (so the operator's own count stays 1; `3D` deletes to EOL plus
            // the two rows below). `C` enters Insert via `emit_operator`.
            DeleteToEol => self.emit_operator(Operator::Delete, OpTarget::LineTail(n), 1),
            ChangeToEol => self.emit_operator(Operator::Change, OpTarget::LineTail(n), 1),
            // The count rides in the action: each is ONE edit over `n` chars,
            // not `n` edits (see `EditAction::ToggleCase`).
            ToggleCase => KeyOutcome::edit(EditAction::ToggleCase { count: n }),
            // Awaits the char; `replace_char_key` reads the mode to decide
            // between `n` chars and the selection.
            ReplaceChar => {
                self.pending = Pending::Replace { count: n };
                KeyOutcome::Ignored
            }
            // Awaits the name; `set_mark_key`. A count means nothing to it.
            SetMark => {
                self.pending = Pending::SetMark;
                KeyOutcome::Ignored
            }
            EnterReplace => {
                self.mode = Mode::Replace;
                KeyOutcome::edit(EditAction::EnterReplace { count: n })
            }
            Scroll(cmd) => KeyOutcome::Scroll(cmd),
            Fold(verb) => KeyOutcome::Fold(verb),
            EnterCommand => KeyOutcome::EnterCommand,
            Search(cmd) => KeyOutcome::Search { cmd, count: n },
            Submit => KeyOutcome::Submit,
            LeaveVisual => {
                self.mode = Mode::Normal;
                KeyOutcome::edit(EditAction::ClearAnchor)
            }
            SwitchVisual(kind) => {
                let target = match kind {
                    SelectionKind::Char => Mode::Visual,
                    SelectionKind::Line => Mode::VisualLine,
                    SelectionKind::Block => Mode::VisualBlock,
                };
                self.switch_visual(target, kind)
            }
            // Operators act on the selection immediately (the prompt resolves
            // char/line/block geometry from its `selection_kind`). `d`/`y`
            // leave Visual; a `Change` stays — `emit_operator` enters Insert.
            VisualOperator(op) => {
                if op != Operator::Change {
                    self.mode = Mode::Normal;
                }
                // A selection operator applies once, whatever was typed — the
                // count belongs to the motions that built the selection, not
                // the operation over it. (A search after a count in Visual
                // takes it: `v3n` is the third match, as in vim — D 3.)
                self.emit_operator(op, OpTarget::Selection, 1)
            }
            // Block-insert is only meaningful in block Visual; charwise/
            // linewise `I`/`A` fall through (Ignored), as before.
            BlockInsert(edge) => {
                if self.mode == Mode::VisualBlock {
                    self.mode = Mode::Insert;
                    KeyOutcome::edit(EditAction::BeginBlockInsert(edge))
                } else {
                    KeyOutcome::Ignored
                }
            }
            // `o` in Visual — swap the selection's ends; stays in Visual.
            SwapSelectionEnds => KeyOutcome::edit(EditAction::SwapSelectionEnds),
            // `<C-w>` and `z` open a command prefix; the leading count rides
            // along (`5<C-w>+`, `20zt`). Continuation is `command_key`.
            Prefix(prefix @ (PrefixId::Window | PrefixId::Z)) => {
                self.pending = Pending::Command { prefix, count };
                KeyOutcome::Ignored
            }
            // A window command bound without `<C-w>` before it — `<C-^>`,
            // the window's alternate (V4b) — as `command_key` answers one
            // after it, the count it was typed with riding along.
            Window(cmd) => KeyOutcome::Window(cmd, count.typed()),
            // These never reach the command path: motions and the motion
            // prefixes `g` `[` `]` (motion path), text objects
            // (`resolve_text_object`).
            Motion(_)
            | Prefix(
                PrefixId::G
                | PrefixId::OpenBracket
                | PrefixId::CloseBracket
                | PrefixId::Mark
                | PrefixId::AgentMark
                | PrefixId::Find { .. },
            )
            | Object(_) => KeyOutcome::Ignored,
        }
    }

    /// Switch to Insert and emit the entry, carrying its repeat `count` so a
    /// counted entry (`3ix`, `3o`) replays the typed text on `<Esc>`.
    fn enter_insert(&mut self, entry: InsertEntry, count: usize) -> KeyOutcome {
        self.mode = Mode::Insert;
        KeyOutcome::edit(EditAction::EnterInsert { entry, count })
    }

    /// Capture a `d`/`c` and wait for its target. `op_count` is the count
    /// typed before it; the target's own count multiplies it.
    fn begin_operator(&mut self, op: Operator, op_count: Count) -> KeyOutcome {
        self.pending = Pending::Operator {
            op,
            op_count,
            after: AfterOp::Target {
                count: Count::NONE,
                prefix: None,
            },
        };
        KeyOutcome::Ignored
    }

    /// A key while an operator is pending: its motion target, a digit, the
    /// `i`/`a` of a text object, a doubled operator (`dd`/`cc`), or a motion
    /// prefix (`dg` → `dgg`). Anything unrecognized cancels (vim-like).
    fn operator_key(
        &mut self,
        key: KeyEvent,
        op: Operator,
        op_count: Count,
        after: AfterOp,
    ) -> KeyOutcome {
        let cmd = plain_or_shifted(key.modifiers);
        let (count, prefix) = match after {
            // Third key of a text-object chord (`ci"` → the `"`).
            AfterOp::TextObject { scope, count } => {
                self.pending = Pending::default();
                let count = op_count.times(count).repeat();
                return self.resolve_text_object(op, scope, count, key.code, cmd);
            }
            AfterOp::Target { count, prefix } => (count, prefix),
        };

        // Completing a motion prefix inside the operator (`dgg`).
        if let Some(p) = prefix {
            self.pending = Pending::default();
            return match resolve_motion(self.bindings, Some(p), key.code, cmd) {
                MotionResolution::Complete(m) => self.operate_with_motion(op, op_count, count, m),
                _ => KeyOutcome::Ignored,
            };
        }

        // The motion's own count.
        if let Some(digit) = count_digit(&key, count.counting()) {
            self.pending = Pending::Operator {
                op,
                op_count,
                after: AfterOp::Target {
                    count: count.push(digit),
                    prefix: None,
                },
            };
            return KeyOutcome::Ignored;
        }

        // `i`/`a` opens a text-object chord.
        let object_scope = match key.code {
            KeyCode::Char('i') if cmd => Some(TextObjectScope::Inner),
            KeyCode::Char('a') if cmd => Some(TextObjectScope::Around),
            _ => None,
        };
        if let Some(scope) = object_scope {
            self.pending = Pending::Operator {
                op,
                op_count,
                after: AfterOp::TextObject { scope, count },
            };
            return KeyOutcome::Ignored;
        }

        // Doubled operator → linewise (`dd`/`cc`).
        if cmd && doubled_operator(op, key.code) {
            self.pending = Pending::default();
            let count = op_count.times(count).repeat();
            return self.emit_operator(op, OpTarget::Line, count);
        }

        // Otherwise a motion target (or a prefix that opens, or cancel).
        self.pending = Pending::default();
        match resolve_motion(self.bindings, None, key.code, cmd) {
            MotionResolution::Complete(m) => self.operate_with_motion(op, op_count, count, m),
            MotionResolution::Prefix(p) => {
                self.pending = Pending::Operator {
                    op,
                    op_count,
                    after: AfterOp::Target {
                        count,
                        prefix: Some(p),
                    },
                };
                KeyOutcome::Ignored
            }
            MotionResolution::None => KeyOutcome::Ignored, // cancel
        }
    }

    /// Turn a resolved operator motion into an outcome. Charwise motions
    /// become a charwise `Motion` range; the vertical/buffer motions
    /// (`dj`/`dk`/`dG`/`dgg`/`d3G`) are linewise — routed to `LineMotion`,
    /// which the prompt resolves to a whole-row span. The two counts are
    /// multiplied FIRST, as vim's `opcount * count0`, and the product folded
    /// through the same `apply_count_to_motion` the cursor path uses: so
    /// `d3G` deletes *to line 3* (an absolute `GotoLine`), not three `G`s,
    /// and `2dG` is `d2G` — the operator's count, taken as a repeat, ran
    /// `G` twice and `tx` twice, standing still (review stop 16, A M6).
    fn operate_with_motion(
        &mut self,
        op: Operator,
        op_count: Count,
        motion_count: Count,
        m: CursorMotion,
    ) -> KeyOutcome {
        // None by none stays none, which `G` and `%` read as their own.
        let (m, count) = apply_count_to_motion(m, op_count.times(motion_count));
        let target = if is_charwise(m) {
            OpTarget::Motion(m)
        } else {
            OpTarget::LineMotion(m)
        };
        self.emit_operator(op, target, count)
    }

    /// Build an `Operator` outcome, applying the one invariant every
    /// operator path shares: a `Change` enters Insert. Centralizing it
    /// keeps the "did you remember the mode switch?" rule in one place — the
    /// non-Insert operators (`d`/`y`) simply don't match the branch.
    fn emit_operator(&mut self, op: Operator, target: OpTarget, count: usize) -> KeyOutcome {
        if op == Operator::Change {
            self.mode = Mode::Insert;
        }
        KeyOutcome::Operator { op, target, count }
    }

    /// Resolve a text object (`{i,a}{w,",',`,(,…}`) into an operator outcome —
    /// the object key looked up in `Context::TextObject` — over `count` of
    /// them, the operator's count times the object's (`2dap`, `d2ap`): it was
    /// always one, so a paragraph's count was unreachable (review stop 6,
    /// C 3). `c` enters Insert via `emit_operator`. An unrecognized object
    /// key cancels.
    fn resolve_text_object(
        &mut self,
        op: Operator,
        scope: TextObjectScope,
        count: usize,
        code: KeyCode,
        cmd: bool,
    ) -> KeyOutcome {
        if !cmd {
            return KeyOutcome::Ignored;
        }
        match self
            .bindings
            .get(Context::TextObject, Chord { code, ctrl: false })
        {
            Some(Atom::Object(object)) => {
                self.emit_operator(op, OpTarget::TextObject { scope, object }, count)
            }
            _ => KeyOutcome::Ignored,
        }
    }

    /// Normal mode in the chat viewer — a read-only **cursor surface** (the
    /// same pane framework as the editor, editing gated off). It reuses the
    /// shared motion machine ([`step_motion`]), so every vim motion + count
    /// resolves exactly as in the editor and moves the chat cursor (`Move`);
    /// the app applies it to the chat's [`Buffer`](crate::prompt::Buffer) and
    /// the viewport follows on render. `Ctrl-U`/`D` half-page and `Ctrl-F`/`B`
    /// page still scroll the viewport directly (vim pager keys); the `<C-w>`
    /// window prefix works (focus must return to the prompt). Visual + yank
    /// land next.
    fn normal_viewer(&mut self, key: KeyEvent) -> KeyOutcome {
        // Operator-pending continuation: `y{motion}` / `yy` / `yi"`. The chat's
        // *only* operator is Yank (read-only); `d`/`c` never begin one, so this
        // machine only ever resolves a Yank target.
        if let Pending::Operator {
            op,
            op_count,
            after,
        } = self.pending
        {
            return self.operator_key(key, op, op_count, after);
        }
        if let Pending::Command { prefix, count } = self.pending {
            return self.command_key(key, prefix, count);
        }
        if let Some(outcome) = self.waiting_mark(key) {
            return outcome;
        }
        let (count, prefix) = match self.pending {
            Pending::Root { count, prefix } => (count, prefix),
            _ => (Count::NONE, None),
        };
        // Every Normal command that writes nothing works here too — `v`/`V`,
        // `y`, `Y`, and (the ones this used to swallow) `:` and submit.
        //
        // This was a hand-written list of three keys, so `:` did nothing in the
        // transcript and `:graph` could only be typed from the prompt — which is
        // backwards, since looking at history is exactly when you want it. The
        // table decides now: a read-only pane is Normal mode minus the writers
        // ([`Atom::in_viewer`]), so an atom added later is covered or is a
        // compile error, never silently missing.
        //
        // That claim used to be false for every CONTROL chord: a hand-written
        // `match key.code` for `<C-w>/<C-u>/<C-d>/<C-f>/<C-b>/<C-v>` returned
        // above this, so the count leaked to the next key (`3<C-u>` then `j`
        // moved three lines) and a `[keymap.normal]` remap was honoured in the
        // prompt and ignored here. The block is gone; `<C-f>`/`<C-b>` are
        // atoms now, which also makes them bindable for the first time
        // (Reviewer D).
        //
        // **Not while a motion prefix waits** (`g`, `[`, `]`): the key
        // completes or cancels it, as in the prompt. Looked up first, `gx`
        // and `[x` ran `x` — refused as a writer — `g:` opened the command
        // line, and a completion key that is also a Normal command would be
        // shadowed outright (docketed by Reviewer A; `[` and `]` made it
        // three prefixes).
        if prefix.is_none()
            && let Some(atom) = chord_of(key).and_then(|c| self.bindings.get(Context::Normal, c))
        {
            // The chord ends here **either way**. The editor path clears
            // `pending` inside `step_motion` before dispatching; this lookup
            // returns first, so it must clear it itself — otherwise `3:` left a
            // count of 3 armed and the next `j` moved three lines, and `g:`
            // swallowed the key after it. Atoms that open their own chord
            // (`BeginOperator`, `Prefix`) set `pending` again below. A REFUSED
            // atom must clear it too: `3i` in the transcript ended the count as
            // surely as `3:` does, and leaving it armed would make the next `j`
            // move three lines for a key the user was told did nothing.
            self.pending = Pending::default();
            return match atom.viewer_fit().refusal() {
                None => self.execute_atom(atom, count),
                // **The writers are the reason this arm exists.** They used to
                // fall through to `step_motion`, which answers `Ignored` for a
                // non-motion — so `i`, `a`, `o`, `p`, `x`, `D` and `Enter` were
                // silently dead while `h`/`l`/`e` moved, i.e. the pane was alive
                // and broken at the same time (Reviewer F).
                Some(reason) => KeyOutcome::Refused(reason),
            };
        }
        // Motions move the cursor (the app applies `Move` to the chat buffer;
        // the viewport follows on render).
        match self.step_motion(key, count, prefix) {
            MotionChord::Consumed => KeyOutcome::Ignored,
            MotionChord::Resolved(m, n) => KeyOutcome::Edit {
                action: EditAction::Move(m),
                count: n,
            },
            MotionChord::Command(_) => KeyOutcome::Ignored,
            MotionChord::Window(cmd, n) => KeyOutcome::Window(cmd, n),
        }
    }

    /// Visual mode in the chat viewer — read-only: motions extend the
    /// selection, `y` yanks it (to the clipboard, the copy-to-context seam),
    /// `Esc` leaves, and `v`/`V`/`Ctrl-V` switch geometry. No `d`/`c`/`x`/`s`/
    /// `p` (those would mutate); the shared editor `visual` keeps those.
    fn visual_viewer(&mut self, key: KeyEvent) -> KeyOutcome {
        let cmd = plain_or_shifted(key.modifiers);
        if let Some(outcome) = self.waiting_mark(key) {
            return outcome;
        }
        // **The table decides, exactly as it does in the viewer's Normal.**
        //
        // This was a hand-written list of five keys and everything else fell
        // into `step_motion`, which answers `Ignored` for a non-motion. So in
        // the transcript, `v` then `o` could not flip the selection's end and
        // `vi"` selected nothing — both of them read-only atoms the table
        // already marks `in_viewer()`, both of them working one pane away in
        // the prompt's Visual. `Atom::in_viewer` is named in AGENTS.md as the
        // predicate with two callers; it had one (Reviewer A, round 3).
        //
        // Mirrors [`visual`](Self::visual) arm for arm, filtered by
        // `in_viewer()` — its fallbacks too (`<C-w>`, then `:`, `/` and the
        // scrolls; the second was missing until review stop 5, E2 #5). The
        // five keys it used to list are all `in_viewer()` atoms in
        // `Context::Visual`, so nothing that worked stops working.
        if let Some(outcome) = self.visual_object_chord(key, cmd) {
            return outcome;
        }
        // Motions extend the selection (same accumulator as Normal/the editor).
        let (count, prefix) = match self.pending {
            Pending::Root { count, prefix } => (count, prefix),
            _ => (Count::NONE, None),
        };
        match self.step_motion(key, count, prefix) {
            MotionChord::Consumed => KeyOutcome::Ignored,
            MotionChord::Resolved(m, n) => KeyOutcome::Edit {
                action: EditAction::Move(m),
                count: n,
            },
            MotionChord::Command(n) => self.visual_viewer_command(key, n),
            // `gt` from Visual: vim's ends the selection and goes (measured,
            // Neovim 0.12.6); here the focus moving ends it, in every pane
            // (`App::focus_moved`).
            MotionChord::Window(cmd, n) => KeyOutcome::Window(cmd, n),
        }
    }

    /// [`visual_command`](Self::visual_command), minus the writers.
    ///
    /// The one line that makes a read-only Visual a *filter* of the editor's
    /// Visual rather than a second, smaller keymap that has to be kept in step.
    fn visual_viewer_command(&mut self, key: KeyEvent, count: Count) -> KeyOutcome {
        match chord_of(key).and_then(|c| self.bindings.get(Context::Visual, c)) {
            // Same filter, same voice as the viewer's Normal: a bound key that
            // this pane cannot honour says so rather than doing nothing. `d` on
            // a transcript selection is the common one — the selection is real,
            // the highlight is real, and only the delete is impossible.
            Some(atom) => match atom.viewer_fit().refusal() {
                None => self.execute_atom(atom, count),
                Some(reason) => KeyOutcome::Refused(reason),
            },
            // Same fallbacks as the editor's Visual, and they matter more
            // here: this is the pane whose refusal message tells you to press
            // `<C-w>`, and whose page you came to search. `:` and `/` were
            // missing, so both did nothing, silently, in every viewer's Visual
            // (review stop 5, E2 #5); neither reads nor writes the selection.
            None => self
                .window_prefix_from_visual(key, count)
                .or_else(|| self.normal_atom_that_survives_visual(key, count))
                .unwrap_or(KeyOutcome::Ignored),
        }
    }

    /// Visual mode — shared by charwise (`v`), linewise (`V`), and block
    /// (`Ctrl-V`); they differ only in the prompt's `selection_kind`, which
    /// decides the selection geometry and highlight. A counted motion (`l`,
    /// `3j`, `$`, `gg`, …) extends the selection via the **same**
    /// [`step_motion`](Self::step_motion) accumulator Normal uses — so `gg`
    /// and counts work here exactly as there. Non-motion keys go to
    /// [`visual_command`](Self::visual_command).
    fn visual(&mut self, key: KeyEvent) -> KeyOutcome {
        let cmd = plain_or_shifted(key.modifiers);
        // `r` armed: this key is the char to replace the selection with.
        if let Pending::Replace { count } = self.pending {
            return self.replace_char_key(key, count);
        }
        if let Some(outcome) = self.waiting_mark(key) {
            return outcome;
        }
        // A text-object chord: its object key, or its `i`/`a` (capital `I`/`A`
        // are block-insert, resolved later via the command table).
        if let Some(outcome) = self.visual_object_chord(key, cmd) {
            return outcome;
        }
        // Visual carries the same Root count+prefix chord as Normal.
        // (Operator-pending and the window prefix can't occur in Visual; treat
        // them as a fresh chord.)
        let (count, prefix) = match self.pending {
            Pending::Root { count, prefix } => (count, prefix),
            Pending::Operator { .. }
            | Pending::Command { .. }
            | Pending::Replace { .. }
            | Pending::SetMark => (Count::NONE, None),
        };
        match self.step_motion(key, count, prefix) {
            MotionChord::Consumed => KeyOutcome::Ignored,
            // A motion extends the selection; the keymap stays in Visual.
            MotionChord::Resolved(m, n) => KeyOutcome::Edit {
                action: EditAction::Move(m),
                count: n,
            },
            MotionChord::Command(n) => self.visual_command(key, n),
            MotionChord::Window(cmd, n) => KeyOutcome::Window(cmd, n),
        }
    }

    /// **`<C-w>` from Visual ends the selection and opens the window prefix** —
    /// vim's `nv_window()` rule.
    ///
    /// `Context::Visual` binds no control chord but `<C-v>`, so `<C-w>` was not
    /// merely dead there: it was *swallowed*, and its second key then executed
    /// as a Visual command. `v` `<C-w>` `c` deleted your selection and entered
    /// Insert; `v` `<C-w>` `s` did the same. A window chord that destroys text
    /// is the worst shape a dead key can take, because the user's model says
    /// they never touched the buffer.
    ///
    /// Reachable by following our own advice, which is what raised it: a
    /// writer key refused in a read-only pane now says *"`<C-w>w` to the
    /// prompt"*, and from a Visual selection in the transcript that instruction
    /// did nothing while `w` extended the selection by a word (Reviewer A,
    /// probe-verified).
    ///
    /// The outcome is the anchor clear, not the prefix: the prefix is keymap
    /// state (`Pending::Command`), while the app owns the selection and has to
    /// be *told* to drop it, or the highlight outlives the mode that explains
    /// it.
    ///
    /// **Scoped to `<C-w>` deliberately.** `:`, `<C-d>`, `<C-u>`, `<C-f>` and
    /// `<C-b>` are dead in Visual too, and each needs a decision rather than a
    /// guess: vim's `:` prefills a `'<,'>` range we do not have, and vim's
    /// scroll keys in Visual move the cursor and EXTEND the selection, which is
    /// a different action from the viewport scroll `KeyOutcome::Scroll`
    /// performs. Docketed, not smuggled in here.
    fn window_prefix_from_visual(&mut self, key: KeyEvent, count: Count) -> Option<KeyOutcome> {
        let atom = chord_of(key).and_then(|c| self.bindings.get(Context::Normal, c))?;
        if !matches!(atom, Atom::Prefix(PrefixId::Window)) {
            return None;
        }
        self.mode = Mode::Normal;
        self.visual_object = None;
        // Arms `Pending::Command`, the count typed riding along as Normal's
        // does (`3<C-w>+`); its `KeyOutcome::Ignored` is discarded in favour
        // of the anchor clear, which is what the app must act on.
        let _ = self.execute_atom(atom, count);
        Some(KeyOutcome::edit(EditAction::ClearAnchor))
    }

    /// A non-motion Visual command — looked up in `Context::Visual` and
    /// executed with the count typed before it. The operators apply once
    /// (`execute_atom`): the count belongs to the motions that *built* the
    /// selection, not the operation over it. A search takes it (`v3n`).
    fn visual_command(&mut self, key: KeyEvent, count: Count) -> KeyOutcome {
        match chord_of(key).and_then(|c| self.bindings.get(Context::Visual, c)) {
            Some(atom) => self.execute_atom(atom, count),
            // Visual has no entry for this chord. Two things can still claim
            // it, and both are keys vim keeps live while a selection is up:
            //
            // - `<C-w>`, the window prefix, which means the same in every mode;
            // - `:` and the scroll commands, which are not *about* the
            //   selection — they page the window or start an ex-command. They
            //   were dead here, so `<C-d>` in Visual did nothing and `:` could
            //   not be reached without leaving the selection first (Steven,
            //   2026-08-25; "yes, follow vim").
            //
            // Consulting `Context::Normal` and filtering on `works_in_visual`
            // rather than falling through wholesale: a blanket fallthrough
            // would hand `x` and `p` their Normal meanings on top of the Visual
            // ones they already have.
            None => self
                .window_prefix_from_visual(key, count)
                .or_else(|| self.normal_atom_that_survives_visual(key, count))
                .unwrap_or(KeyOutcome::Ignored),
        }
    }

    /// A Normal-table atom that vim keeps live in Visual — the command line,
    /// the scroll commands, a search, a mark — with the count typed before
    /// it: `v3n` is the third match, as in vim (measured, nvim: still Visual,
    /// the anchor kept; review stop 16, D 3 — it was always 1). `None` for
    /// everything else, including a chord that is bound to nothing.
    fn normal_atom_that_survives_visual(
        &mut self,
        key: KeyEvent,
        count: Count,
    ) -> Option<KeyOutcome> {
        let atom = chord_of(key).and_then(|c| self.bindings.get(Context::Normal, c))?;
        atom.works_in_visual()
            .then(|| self.execute_atom(atom, count))
    }

    /// **The Visual text-object chord, both Visuals'** (the editor's and a
    /// viewer's, which each wrote it out): a pending `i`/`a` takes this key as
    /// its object; an `i`/`a` arms one, taking the count typed before it
    /// (`v2ap`) — which used to stay pending, so the object ignored it and
    /// the next `j` moved twice (review stop 6, A 2, C 3). `None`: not part of
    /// a chord.
    fn visual_object_chord(&mut self, key: KeyEvent, cmd: bool) -> Option<KeyOutcome> {
        if let Some((scope, count)) = self.visual_object.take() {
            return Some(self.resolve_visual_text_object(scope, count, key.code, cmd));
        }
        // **A waiting motion prefix owns the key**: the `a` of `'a` names a
        // mark and opens no object. Asked first, `v'a` selected nothing and
        // armed `va` — the first prefix completed by `i` or `a`.
        if matches!(
            self.pending,
            Pending::Root {
                prefix: Some(_),
                ..
            }
        ) {
            return None;
        }
        let scope = match key.code {
            KeyCode::Char('i') if cmd => TextObjectScope::Inner,
            KeyCode::Char('a') if cmd => TextObjectScope::Around,
            _ => return None,
        };
        let count = match self.pending {
            Pending::Root { count, .. } => count,
            _ => Count::NONE,
        };
        self.pending = Pending::default();
        self.visual_object = Some((scope, count));
        Some(KeyOutcome::Ignored)
    }

    /// Complete a Visual text-object chord (`vi`/`va` + object key): look the
    /// object up in `Context::TextObject` (the same table operator-pending
    /// `ciw` uses) and select `count` of it. An unrecognized object key
    /// cancels, leaving the selection unchanged.
    fn resolve_visual_text_object(
        &mut self,
        scope: TextObjectScope,
        count: Count,
        code: KeyCode,
        cmd: bool,
    ) -> KeyOutcome {
        if !cmd {
            return KeyOutcome::Ignored;
        }
        match self
            .bindings
            .get(Context::TextObject, Chord { code, ctrl: false })
        {
            Some(Atom::Object(object)) => KeyOutcome::edit(EditAction::SelectTextObject {
                scope,
                object,
                count: count.repeat(),
            }),
            _ => KeyOutcome::Ignored,
        }
    }

    /// Enter Visual from Normal (`v`/`V`/`Ctrl-V`): switch to the matching mode
    /// and anchor a selection of that geometry at the cursor. Shared by the
    /// editor (`execute_atom`) and the chat viewer (`normal_viewer`).
    fn enter_visual(&mut self, kind: SelectionKind) -> KeyOutcome {
        let (mode, anchor) = match kind {
            SelectionKind::Char => (Mode::Visual, EditAction::SetAnchor),
            SelectionKind::Line => (Mode::VisualLine, EditAction::SetAnchorLine),
            SelectionKind::Block => (Mode::VisualBlock, EditAction::SetAnchorBlock),
        };
        self.mode = mode;
        self.visual_object = None; // a fresh Visual gesture: no object chord pending
        // Discard any leading count/prefix: `v` is a fresh gesture, so `3v`
        // selects from the cursor (the first Visual *motion* takes its own
        // count). The keys clear `pending` before they get here — the
        // editor's `step_motion`, the viewer's own — so this is the mouse's:
        // a drag (`begin_visual`) after `3` extended the first motion ×3
        // without it (`chat_viewer_count_does_not_leak_into_visual`).
        self.pending = Pending::default();
        KeyOutcome::edit(anchor)
    }

    /// Route a Visual toggle key (`v`/`V`/`Ctrl-V`): if already in `target`,
    /// leave Visual (drop the selection); otherwise switch to `target`,
    /// flipping the geometry to `kind` while keeping both endpoints (not a
    /// re-anchor).
    fn switch_visual(&mut self, target: Mode, kind: SelectionKind) -> KeyOutcome {
        if self.mode == target {
            self.mode = Mode::Normal;
            KeyOutcome::edit(EditAction::ClearAnchor)
        } else {
            self.mode = target;
            KeyOutcome::edit(EditAction::SetSelectionKind(kind))
        }
    }

    fn insert(&mut self, key: KeyEvent) -> KeyOutcome {
        use EditAction::*;
        let m = key.modifiers;
        let ctrl = m.contains(KeyModifiers::CONTROL);
        // Rebindable Insert *control* chords come from the table first — today
        // just the direct-submit key (`Ctrl+Enter` → Submit). Printable typing
        // never produces a bound chord here, so this only fires for the few
        // bound control keys; everything else falls through to the edit match.
        if let Some(atom) = chord_of(key).and_then(|c| self.bindings.get(Context::Insert, c)) {
            return self.execute_atom(atom, Count::NONE);
        }
        match (key.code, m) {
            // Leaving Insert: switch to Normal *and* nudge the caret back
            // onto the last typed char (vim's `<Esc>` rule). The mode change
            // is the `self.mode` write; the outcome carries the cursor
            // clamp (the renderer reads `mode()` each frame regardless).
            (KeyCode::Esc, _) => {
                self.mode = Mode::Normal;
                KeyOutcome::edit(ClampCursorAfterInsert)
            }
            // Alt-Enter also submits, and `:help submit` says so: it is the
            // Insert submit that gets through tmux, which hands Ctrl+Enter
            // over as a bare CR (probed 11-10), and through terminals that
            // send Shift-Enter as Alt-Enter. Not rebindable: the `Chord` type
            // has no Alt, so it can't live in the table. `Ctrl+Enter` above
            // is the canonical, rebindable direct-submit key.
            (KeyCode::Enter, mods) if mods == KeyModifiers::ALT => KeyOutcome::Submit,
            // Plain or Shift-Enter inserts a newline.
            (KeyCode::Enter, mods) if plain_or_shifted(mods) => KeyOutcome::edit(Newline),
            (KeyCode::Char('w'), _) if ctrl => KeyOutcome::edit(DeleteWordBack),
            (KeyCode::Char('u'), _) if ctrl => KeyOutcome::edit(DeleteToLineStart),
            // Printable character.
            (KeyCode::Char(c), mods) if plain_or_shifted(mods) => KeyOutcome::edit(Insert(c)),
            (KeyCode::Backspace, _) => KeyOutcome::edit(DeleteBack),
            (KeyCode::Delete, _) => KeyOutcome::edit(DeleteForward),
            (KeyCode::Left, _) => KeyOutcome::edit(Move(CursorMotion::Left)),
            (KeyCode::Right, _) => KeyOutcome::edit(Move(CursorMotion::Right)),
            (KeyCode::Up, _) => KeyOutcome::edit(Move(CursorMotion::Up)),
            (KeyCode::Down, _) => KeyOutcome::edit(Move(CursorMotion::Down)),
            (KeyCode::Home, _) => KeyOutcome::edit(Move(CursorMotion::LineStart)),
            (KeyCode::End, _) => KeyOutcome::edit(Move(CursorMotion::LineEnd)),
            _ => KeyOutcome::Ignored,
        }
    }

    /// Replace mode (`R`): a printable key overwrites the char under the
    /// cursor, `Enter` breaks the line without overwriting, `<BS>` puts back
    /// what this session covered, `<Esc>` leaves. The submit chords are
    /// Insert's, from the same table. Not here (yet): Insert's `<C-w>`/`<C-u>`
    /// — they would have to restore, not delete, and are left inert.
    fn replace(&mut self, key: KeyEvent) -> KeyOutcome {
        use EditAction::*;
        if let Some(atom) = chord_of(key).and_then(|c| self.bindings.get(Context::Insert, c)) {
            return self.execute_atom(atom, Count::NONE);
        }
        let m = key.modifiers;
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                KeyOutcome::edit(LeaveReplace)
            }
            KeyCode::Enter if m == KeyModifiers::ALT => KeyOutcome::Submit,
            KeyCode::Enter if plain_or_shifted(m) => KeyOutcome::edit(ReplaceType('\n')),
            KeyCode::Char(c) if plain_or_shifted(m) => KeyOutcome::edit(ReplaceType(c)),
            KeyCode::Backspace => KeyOutcome::edit(ReplaceBack),
            KeyCode::Left => KeyOutcome::edit(Move(CursorMotion::Left)),
            KeyCode::Right => KeyOutcome::edit(Move(CursorMotion::Right)),
            KeyCode::Up => KeyOutcome::edit(Move(CursorMotion::Up)),
            KeyCode::Down => KeyOutcome::edit(Move(CursorMotion::Down)),
            KeyCode::Home => KeyOutcome::edit(Move(CursorMotion::LineStart)),
            KeyCode::End => KeyOutcome::edit(Move(CursorMotion::LineEnd)),
            _ => KeyOutcome::Ignored,
        }
    }
}

impl Keymap for VimKeymap {
    /// **Abandon a half-typed chord.** `<C-w>`, an operator, a count, a `g`
    /// prefix, `<C-x>` — anything waiting for its next key.
    ///
    /// Exists because a chord could survive a key that was never delivered to
    /// the keymap at all. `handle_event` answers `Ctrl-C`, `Ctrl-L`, a scroll
    /// and a `y`/`n` confirmation *above* the keymap and returns, so the key
    /// the chord was waiting for was eaten while the prefix stayed armed:
    /// pressing `<C-w>`, changing your mind, hitting `Ctrl-C` — which is the
    /// universal cancel, and which vim's `CTRL-C` does cancel a pending command
    /// with — and then typing `q` **closed a pane**, because `q` resolved in
    /// `Context::AfterWindow`. On the last pane that is a quit. Confirmed
    /// against the running binary through `tools/drive` (Reviewer D).
    fn cancel_chord(&mut self) {
        self.pending = Pending::default();
        self.editor_chord = false;
    }

    fn pending_chord(&self) -> bool {
        !matches!(
            self.pending,
            Pending::Root {
                count: Count::NONE,
                prefix: None
            }
        ) || self.editor_chord
    }

    fn pending_prefix(&self) -> bool {
        !matches!(self.pending, Pending::Root { prefix: None, .. }) || self.editor_chord
    }

    /// An operator waiting is `o` (neovim's `finish_op`), and `r` waiting
    /// is Replace (neovim pretends Replace there, in Visual too) — a `g`,
    /// `z` or `<C-w>` prefix, a count or `m` change nothing.
    fn cursor_mode(&self) -> crate::guicursor::CursorMode {
        use crate::guicursor::CursorMode;
        match self.pending {
            Pending::Operator { .. } => CursorMode::Operator,
            Pending::Replace { .. } => CursorMode::Replace,
            _ => self.mode.into(),
        }
    }

    /// Undoes `emit_operator`'s one step for a change: Insert back to Normal.
    fn abandon_change(&mut self) {
        if self.mode == Mode::Insert {
            self.mode = Mode::Normal;
        }
    }

    fn translate(&mut self, key: KeyEvent) -> KeyOutcome {
        // Kitty-protocol terminals deliver Release/Repeat; act on Press
        // (and the legacy `Repeat`-less default) only.
        if key.kind == KeyEventKind::Release {
            return KeyOutcome::Ignored;
        }
        // The `Ctrl-X Ctrl-E` `$EDITOR` chord, resolved above the mode handlers
        // (readline's binding; mode-independent). `Ctrl-X` arms it; the next key
        // either completes (`Ctrl-E`) or cancels and is handled normally.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if std::mem::take(&mut self.editor_chord) {
            if ctrl && key.code == KeyCode::Char('e') {
                return KeyOutcome::OpenEditor;
            }
            // not the completion — fall through to handle this key normally
        } else if ctrl && key.code == KeyCode::Char('x') {
            self.editor_chord = true;
            return KeyOutcome::Ignored;
        }
        // `Esc` aborts the **whole** partially-typed command, not one level of
        // it. Vim discards the pending chord entirely (`vgi<Esc>` leaves you in
        // Visual with nothing pending, not part-way through `gi`), so unwinding
        // one level per press would make deeply-nested chords need three, four,
        // … presses — a wedge in everything but name. With this, `Esc` from any
        // state reaches Normal in at most two: one to drop the chord, one to
        // leave Visual. (Found by the `esc_always_reaches_normal_no_wedge`
        // property, which shrank to `v g i`.)
        // Both chord states count — the Normal/operator `Pending` AND the Visual
        // text-object mini-state (`vi"`), which is where the property's `v g i`
        // case wedged: two independent pending levels, each swallowing one `Esc`.
        if key.code == KeyCode::Esc
            && (self.pending != Pending::default() || self.visual_object.is_some())
        {
            self.pending = Pending::default();
            self.visual_object = None;
            return KeyOutcome::Ignored;
        }
        match self.mode {
            Mode::Insert => self.insert(key),
            Mode::Replace => self.replace(key),
            // The editable prompt's Visual operates (`d`/`c`/`y`); the chat
            // viewer's is read-only (motions + `y`), so they take different
            // handlers — same split as Normal's editor/viewer.
            Mode::Visual | Mode::VisualLine | Mode::VisualBlock => {
                if self.allow_insert {
                    self.visual(key)
                } else {
                    self.visual_viewer(key)
                }
            }
            Mode::Normal => self.normal(key),
        }
    }

    fn mode(&self) -> Mode {
        self.mode
    }

    fn begin_visual(&mut self) -> KeyOutcome {
        // Reuse the `v` gesture: enter characterwise Visual + anchor at the
        // cursor (also clears any pending count, like a real `v`).
        self.enter_visual(SelectionKind::Char)
    }

    fn leave_visual(&mut self) -> KeyOutcome {
        self.mode = Mode::Normal;
        self.pending = Pending::default();
        KeyOutcome::edit(EditAction::ClearAnchor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::ViewerFit;

    /// The two viewer refusals, **read from the source of truth** rather than
    /// copied. A test that hardcodes the sentence keeps passing when the
    /// sentence is changed to something wrong, which makes it a test of the
    /// author's memory rather than of the program.
    const WRITES: &str = match ViewerFit::Writes.refusal() {
        Some(s) => s,
        None => unreachable!(),
    };
    const SENDS: &str = match ViewerFit::Sends.refusal() {
        Some(s) => s,
        None => unreachable!(),
    };
    use crate::keymap::{
        BlockEdge, Direction, ScrollCmd,
        ScrollMagnitude::{HalfPage, Page},
        TextObject, WindowCmd,
    };

    fn press(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    /// **The keys vim keeps live while a selection is up.**
    ///
    /// `:` and `CTRL-D`/`CTRL-U`/`CTRL-F`/`CTRL-B` are not *about* the
    /// selection — one starts an ex-command, the others page the window — and
    /// vim leaves them working in Visual. Here they were `Ignored`, so `<C-d>`
    /// did nothing and the command line could not be reached without dropping
    /// the selection first (Steven, 2026-08-25: "yes, follow vim").
    ///
    /// The negative half is the load-bearing one: this must NOT be a
    /// fallthrough to the Normal table, or `x` and `p` would pick up their
    /// Normal meanings on top of the Visual ones they already have.
    #[test]
    fn visual_keeps_the_keys_vim_keeps() {
        let survives = [
            (
                KeyCode::Char('d'),
                KeyModifiers::CONTROL,
                KeyOutcome::Scroll(ScrollCmd::Down(HalfPage)),
            ),
            (
                KeyCode::Char('u'),
                KeyModifiers::CONTROL,
                KeyOutcome::Scroll(ScrollCmd::Up(HalfPage)),
            ),
            (
                KeyCode::Char('f'),
                KeyModifiers::CONTROL,
                KeyOutcome::Scroll(ScrollCmd::Down(crate::keymap::ScrollMagnitude::Page)),
            ),
            (
                KeyCode::Char('b'),
                KeyModifiers::CONTROL,
                KeyOutcome::Scroll(ScrollCmd::Up(crate::keymap::ScrollMagnitude::Page)),
            ),
            (
                KeyCode::Char(':'),
                KeyModifiers::NONE,
                KeyOutcome::EnterCommand,
            ),
        ];
        for (code, mods, want) in survives {
            let mut vk = VimKeymap::prompt(Mode::Normal);
            vk.translate(press(KeyCode::Char('v'), KeyModifiers::NONE));
            assert_eq!(vk.mode(), Mode::Visual, "premise: we are in Visual");
            let got = vk.translate(press(code, mods));
            assert_eq!(got, want, "{code:?} must still work in Visual");
        }

        // **The negative half, and it has to use a key the Visual table does
        // NOT bind.** `x` proves nothing here: Visual binds it to
        // `visual-delete`, so the lookup succeeds before the filter is ever
        // consulted, and a mutation making `works_in_visual` say yes to
        // everything passed. `p` is bound in Normal and absent from Visual,
        // which is exactly where a blanket fallthrough would show up — as a
        // paste in the middle of a selection.
        let mut vk = VimKeymap::prompt(Mode::Normal);
        vk.translate(press(KeyCode::Char('v'), KeyModifiers::NONE));
        assert_eq!(
            vk.translate(press(KeyCode::Char('p'), KeyModifiers::NONE)),
            KeyOutcome::Ignored,
            "`p` is Normal-only: falling through wholesale would paste here"
        );
        assert_eq!(vk.mode(), Mode::Visual, "and the selection is still up");
    }

    fn prompt_insert() -> VimKeymap {
        VimKeymap::prompt(Mode::Insert)
    }

    fn ctrl(c: char) -> KeyEvent {
        press(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn plain(c: char) -> KeyEvent {
        press(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn ctrl_w_w_cycles_focus() {
        // `<C-w>` opens the window prefix (no outcome); `w` cycles focus.
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(k.translate(ctrl('w')), KeyOutcome::Ignored);
        assert_eq!(
            k.translate(plain('w')),
            KeyOutcome::Window(WindowCmd::FocusCycle, None),
        );
    }

    /// vim distinguishes a bare `<C-w>w` (cycle) from `{count}<C-w>w` (jump to
    /// window {count}), so the count must survive as `None` vs `Some(n)`.
    /// Collapsing absent-to-one with `.max(1)` made pane numbering
    /// unexpressible, and the outcome's doc comment said so out loud.
    #[test]
    fn a_window_count_survives_as_absent_or_present() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(k.translate(ctrl('w')), KeyOutcome::Ignored);
        assert_eq!(
            k.translate(plain('w')),
            KeyOutcome::Window(WindowCmd::FocusCycle, None),
            "no count typed"
        );

        // `2<C-w>w` — the count may be typed before the chord.
        assert_eq!(k.translate(plain('2')), KeyOutcome::Ignored);
        assert_eq!(k.translate(ctrl('w')), KeyOutcome::Ignored);
        assert_eq!(
            k.translate(plain('w')),
            KeyOutcome::Window(WindowCmd::FocusCycle, Some(2)),
            "a typed count reaches the command"
        );

        // …and `<C-w>3w`, with the count inside the chord.
        assert_eq!(k.translate(ctrl('w')), KeyOutcome::Ignored);
        assert_eq!(k.translate(plain('3')), KeyOutcome::Ignored);
        assert_eq!(
            k.translate(plain('w')),
            KeyOutcome::Window(WindowCmd::FocusCycle, Some(3)),
        );

        // An explicit `1` is NOT the same as no count: it is a jump to pane 1.
        assert_eq!(k.translate(plain('1')), KeyOutcome::Ignored);
        assert_eq!(k.translate(ctrl('w')), KeyOutcome::Ignored);
        assert_eq!(
            k.translate(plain('w')),
            KeyOutcome::Window(WindowCmd::FocusCycle, Some(1)),
            "`1<C-w>w` jumps; it does not cycle"
        );
    }

    #[test]
    fn ctrl_w_ctrl_w_also_cycles_focus() {
        // The quick toggle: `<C-w><C-w>`.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(ctrl('w')),
            KeyOutcome::Window(WindowCmd::FocusCycle, None),
        );
    }

    #[test]
    fn ctrl_w_directional_focus() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(plain('j')),
            KeyOutcome::Window(WindowCmd::Focus(Direction::Down), None),
        );
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(plain('k')),
            KeyOutcome::Window(WindowCmd::Focus(Direction::Up), None),
        );
    }

    #[test]
    fn window_prefix_works_from_the_chat_viewer() {
        // Focus must be switchable back from the chat pane (a Normal-only
        // viewer), so the window prefix works there too.
        let mut k = VimKeymap::chat();
        assert_eq!(k.translate(ctrl('w')), KeyOutcome::Ignored);
        assert_eq!(
            k.translate(plain('w')),
            KeyOutcome::Window(WindowCmd::FocusCycle, None),
        );
    }

    #[test]
    fn window_prefix_cancels_on_unknown_key_then_resumes_normal() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(ctrl('w'));
        // `z` isn't a window command → cancel, no outcome.
        assert_eq!(k.translate(plain('z')), KeyOutcome::Ignored);
        // ...and the chord is over: the next key is a fresh Normal command.
        assert_eq!(
            k.translate(plain('j')),
            KeyOutcome::edit(EditAction::Move(CursorMotion::Down)),
        );
    }

    #[test]
    fn ctrl_w_resize_with_count_straddle() {
        // `5<C-w>+` → grow by 5: the count typed *before* the prefix reaches
        // the command. `<C-w>-` / `<C-w>=` resolve too.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(plain('5'));
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(press(KeyCode::Char('+'), KeyModifiers::SHIFT)),
            KeyOutcome::Window(WindowCmd::ResizeGrow, Some(5)),
        );
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(plain('-')),
            KeyOutcome::Window(WindowCmd::ResizeShrink, None),
        );
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(plain('=')),
            KeyOutcome::Window(WindowCmd::Equalize, None),
        );
    }

    #[test]
    fn ctrl_w_resize_count_after_prefix() {
        // `<C-w>5+` also works — the count accumulates after the prefix.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(ctrl('w'));
        k.translate(plain('5'));
        assert_eq!(
            k.translate(press(KeyCode::Char('+'), KeyModifiers::SHIFT)),
            KeyOutcome::Window(WindowCmd::ResizeGrow, Some(5)),
        );
    }

    #[test]
    fn ctrl_w_relocate_up_down() {
        // `<C-w>K`/`J` relocate the focused pane top/bottom (capital → SHIFT).
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(press(KeyCode::Char('K'), KeyModifiers::SHIFT)),
            KeyOutcome::Window(WindowCmd::Relocate(Direction::Up), None),
        );
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(press(KeyCode::Char('J'), KeyModifiers::SHIFT)),
            KeyOutcome::Window(WindowCmd::Relocate(Direction::Down), None),
        );
    }

    #[test]
    fn ctrl_w_relocate_sides_and_width_resize() {
        // `<C-w>H`/`L` reorient side-by-side; `<C-w>>`/`<` resize width.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(press(KeyCode::Char('H'), KeyModifiers::SHIFT)),
            KeyOutcome::Window(WindowCmd::Relocate(Direction::Left), None),
        );
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(press(KeyCode::Char('L'), KeyModifiers::SHIFT)),
            KeyOutcome::Window(WindowCmd::Relocate(Direction::Right), None),
        );
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(press(KeyCode::Char('>'), KeyModifiers::SHIFT)),
            KeyOutcome::Window(WindowCmd::ResizeWider, None),
        );
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(press(KeyCode::Char('<'), KeyModifiers::SHIFT)),
            KeyOutcome::Window(WindowCmd::ResizeNarrower, None),
        );
    }

    #[test]
    fn config_override_is_honored() {
        use crate::bindings::Bindings;
        use std::collections::BTreeMap;
        // A config that remaps `j` (a motion) to word-forward.
        let mut motion = BTreeMap::new();
        motion.insert("j".to_string(), "word-forward".to_string());
        let mut raw = BTreeMap::new();
        raw.insert("motion".to_string(), motion);
        let (merged, _warnings) = Bindings::with_overrides(&raw);
        // Leak to `&'static` (process-lived, fine in a test) and build a keymap
        // on it via `using` — no global mutation, so other tests are unaffected.
        let bindings: &'static Bindings = Box::leak(Box::new(merged));
        let mut k = VimKeymap::prompt(Mode::Normal).using(bindings);
        // `j` now moves word-forward; `k` is untouched.
        assert_eq!(
            k.translate(plain('j')),
            KeyOutcome::edit(EditAction::Move(CursorMotion::WordForward)),
        );
        assert_eq!(
            k.translate(plain('k')),
            KeyOutcome::edit(EditAction::Move(CursorMotion::Up)),
        );
    }

    #[test]
    fn insert_printable_is_edit_insert() {
        let mut k = prompt_insert();
        assert_eq!(
            k.translate(press(KeyCode::Char('a'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::Insert('a')),
        );
        assert_eq!(
            k.translate(press(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::Insert('A')),
        );
    }

    #[test]
    fn insert_enter_is_newline_alt_enter_submits() {
        let mut k = prompt_insert();
        assert_eq!(
            k.translate(press(KeyCode::Enter, KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::Newline),
        );
        assert_eq!(
            k.translate(press(KeyCode::Enter, KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::Newline),
        );
        assert_eq!(
            k.translate(press(KeyCode::Enter, KeyModifiers::ALT)),
            KeyOutcome::Submit,
        );
    }

    #[test]
    fn replace_enter_overwrites_alt_enter_submits() {
        // `:help submit` promises ALT-<Enter> from Replace as from Insert: the
        // submit that gets through tmux, where CTRL-<Enter> is a bare CR.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(plain('R'));
        assert_eq!(k.mode(), Mode::Replace, "the premise: `R` entered Replace");
        assert_eq!(
            k.translate(press(KeyCode::Enter, KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::ReplaceType('\n')),
        );
        assert_eq!(
            k.translate(press(KeyCode::Enter, KeyModifiers::ALT)),
            KeyOutcome::Submit,
        );
    }

    #[test]
    fn submit_keys_normal_and_ctrl_enter() {
        // Normal: plain Enter submits (vim-consistent), and so does Ctrl+Enter.
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            k.translate(press(KeyCode::Enter, KeyModifiers::NONE)),
            KeyOutcome::Submit,
        );
        assert_eq!(
            k.translate(press(KeyCode::Enter, KeyModifiers::CONTROL)),
            KeyOutcome::Submit,
        );
    }

    #[test]
    fn insert_ctrl_enter_is_the_direct_submit() {
        // The direct-submit key works mid-compose without leaving Insert, while
        // plain Enter (tested above) stays a newline.
        let mut k = prompt_insert();
        assert_eq!(
            k.translate(press(KeyCode::Enter, KeyModifiers::CONTROL)),
            KeyOutcome::Submit,
        );
    }

    #[test]
    fn insert_ctrl_w_and_ctrl_u_edit() {
        let mut k = prompt_insert();
        assert_eq!(
            k.translate(press(KeyCode::Char('w'), KeyModifiers::CONTROL)),
            KeyOutcome::edit(EditAction::DeleteWordBack),
        );
        assert_eq!(
            k.translate(press(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            KeyOutcome::edit(EditAction::DeleteToLineStart),
        );
    }

    #[test]
    fn esc_switches_to_normal_and_back() {
        let mut k = prompt_insert();
        assert_eq!(k.mode(), Mode::Insert);
        // Esc leaves Insert and nudges the caret back onto the last char.
        assert_eq!(
            k.translate(press(KeyCode::Esc, KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::ClampCursorAfterInsert),
        );
        assert_eq!(k.mode(), Mode::Normal);
        // `i` returns to Insert (entry kind `Before`, uncounted).
        assert_eq!(
            k.translate(press(KeyCode::Char('i'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::EnterInsert {
                entry: InsertEntry::Before,
                count: 1,
            }),
        );
        assert_eq!(k.mode(), Mode::Insert);
    }

    #[test]
    fn normal_editor_submits_and_moves_cursor() {
        use EditAction::*;
        let mut k = VimKeymap::prompt(Mode::Normal);
        let cases = [
            (KeyCode::Enter, KeyModifiers::NONE, KeyOutcome::Submit),
            (
                KeyCode::Char('h'),
                KeyModifiers::NONE,
                KeyOutcome::edit(Move(CursorMotion::Left)),
            ),
            (
                KeyCode::Char('l'),
                KeyModifiers::NONE,
                KeyOutcome::edit(Move(CursorMotion::Right)),
            ),
            (
                KeyCode::Char('j'),
                KeyModifiers::NONE,
                KeyOutcome::edit(Move(CursorMotion::Down)),
            ),
            (
                KeyCode::Char('k'),
                KeyModifiers::NONE,
                KeyOutcome::edit(Move(CursorMotion::Up)),
            ),
            (
                KeyCode::Char('w'),
                KeyModifiers::NONE,
                KeyOutcome::edit(Move(CursorMotion::WordForward)),
            ),
            (
                KeyCode::Char('b'),
                KeyModifiers::NONE,
                KeyOutcome::edit(Move(CursorMotion::WordBackward)),
            ),
            (
                KeyCode::Char('e'),
                KeyModifiers::NONE,
                KeyOutcome::edit(Move(CursorMotion::WordEnd)),
            ),
            (
                KeyCode::Char('0'),
                KeyModifiers::NONE,
                KeyOutcome::edit(Move(CursorMotion::LineStart)),
            ),
            // `$` and `G` arrive with Shift held (the shifted glyph).
            (
                KeyCode::Char('$'),
                KeyModifiers::SHIFT,
                KeyOutcome::edit(Move(CursorMotion::LineEnd)),
            ),
            (
                KeyCode::Char('G'),
                KeyModifiers::SHIFT,
                KeyOutcome::edit(Move(CursorMotion::BufferEnd)),
            ),
            // Ctrl-U/D still emit scroll (binding preserved for the prompt).
            (
                KeyCode::Char('u'),
                KeyModifiers::CONTROL,
                KeyOutcome::Scroll(ScrollCmd::Up(HalfPage)),
            ),
            (
                KeyCode::Char('d'),
                KeyModifiers::CONTROL,
                KeyOutcome::Scroll(ScrollCmd::Down(HalfPage)),
            ),
        ];
        for (code, mods, expected) in cases {
            assert_eq!(
                k.translate(press(code, mods)),
                expected,
                "{code:?} {mods:?}"
            );
        }
    }

    #[test]
    fn insert_entry_commands_switch_mode_and_name_their_entry() {
        // (key, shift?, entry kind). Each emits an uncounted `EnterInsert`
        // (the prompt does the reposition) and leaves the keymap in Insert.
        let cases = [
            (KeyCode::Char('i'), KeyModifiers::NONE, InsertEntry::Before),
            (KeyCode::Char('a'), KeyModifiers::NONE, InsertEntry::After),
            (
                KeyCode::Char('A'),
                KeyModifiers::SHIFT,
                InsertEntry::LineEnd,
            ),
            (
                KeyCode::Char('I'),
                KeyModifiers::SHIFT,
                InsertEntry::LineStart,
            ),
            (
                KeyCode::Char('o'),
                KeyModifiers::NONE,
                InsertEntry::OpenBelow,
            ),
            (
                KeyCode::Char('O'),
                KeyModifiers::SHIFT,
                InsertEntry::OpenAbove,
            ),
        ];
        for (code, mods, entry) in cases {
            let mut k = VimKeymap::prompt(Mode::Normal);
            assert_eq!(
                k.translate(press(code, mods)),
                KeyOutcome::edit(EditAction::EnterInsert { entry, count: 1 }),
                "{code:?}",
            );
            assert_eq!(k.mode(), Mode::Insert, "{code:?} must enter Insert");
        }
    }

    #[test]
    fn undo_redo_bindings_distinct_from_ctrl_u_scroll() {
        use EditAction::*;
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            k.translate(press(KeyCode::Char('u'), KeyModifiers::NONE)),
            KeyOutcome::edit(Undo),
        );
        assert_eq!(
            k.translate(press(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            KeyOutcome::edit(Redo),
        );
        // Ctrl-U remains half-page scroll, not undo.
        assert_eq!(
            k.translate(press(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            KeyOutcome::Scroll(ScrollCmd::Up(HalfPage)),
        );
    }

    #[test]
    fn count_prefix_repeats_a_motion() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        // Digits accumulate silently...
        assert_eq!(
            k.translate(press(KeyCode::Char('3'), KeyModifiers::NONE)),
            KeyOutcome::Ignored,
        );
        // ...then the motion carries the count.
        assert_eq!(
            k.translate(press(KeyCode::Char('j'), KeyModifiers::NONE)),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Down),
                count: 3,
            },
        );
        // Count is consumed: the next bare motion is count 1.
        assert_eq!(
            k.translate(press(KeyCode::Char('j'), KeyModifiers::NONE)),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Down),
                count: 1,
            },
        );
    }

    #[test]
    fn multi_digit_count_accumulates() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        for d in ['1', '2'] {
            assert_eq!(
                k.translate(press(KeyCode::Char(d), KeyModifiers::NONE)),
                KeyOutcome::Ignored,
            );
        }
        assert_eq!(
            k.translate(press(KeyCode::Char('l'), KeyModifiers::NONE)),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Right),
                count: 12,
            },
        );
    }

    #[test]
    fn zero_is_line_start_without_count_but_a_digit_within_one() {
        // Bare `0` is the line-start motion.
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            k.translate(press(KeyCode::Char('0'), KeyModifiers::NONE)),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::LineStart),
                count: 1,
            },
        );
        // After a leading `1`, the `0` extends the count to 10.
        k.translate(press(KeyCode::Char('1'), KeyModifiers::NONE));
        k.translate(press(KeyCode::Char('0'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('l'), KeyModifiers::NONE)),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Right),
                count: 10,
            },
        );
    }

    #[test]
    fn x_and_capital_x_delete_chars_with_count() {
        // `x`/`X` are vim's `dl`/`dh` — operator outcomes (so they cut into
        // the register), not bare DeleteForward/DeleteBack edits.
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            k.translate(press(KeyCode::Char('x'), KeyModifiers::NONE)),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::Motion(CursorMotion::Right),
                count: 1,
            },
        );
        k.translate(press(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('X'), KeyModifiers::SHIFT)),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::Motion(CursorMotion::Left),
                count: 2,
            },
        );
    }

    #[test]
    fn gg_chord_goes_to_buffer_start() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        // First `g` is silent (prefix pending).
        assert_eq!(
            k.translate(press(KeyCode::Char('g'), KeyModifiers::NONE)),
            KeyOutcome::Ignored,
        );
        assert_eq!(
            k.translate(press(KeyCode::Char('g'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::Move(CursorMotion::BufferStart)),
        );
    }

    #[test]
    fn g_prefix_then_unmapped_key_cancels() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('g'), KeyModifiers::NONE));
        // `gz` is not a command: the chord cancels, nothing applied.
        assert_eq!(
            k.translate(press(KeyCode::Char('z'), KeyModifiers::NONE)),
            KeyOutcome::Ignored,
        );
        // And the keymap is back to a clean Normal state — `l` moves once.
        assert_eq!(
            k.translate(press(KeyCode::Char('l'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::Move(CursorMotion::Right)),
        );
    }

    /// **`]]` `[[` `][` `[]` are motions through the one machine**: the
    /// prompt's and the transcript's keymaps alike, the first key the
    /// direction and a doubled one a start (`][` forward to an end), the
    /// count IN the motion (`3]]` is one motion that fails whole, not three),
    /// an operator's target charwise for the buffer to shape by vim's
    /// exclusive rules, its own count kept apart for the buffer to multiply
    /// (`2y3]]`); `[x` and `gx` cancel and are eaten in both — the transcript
    /// ran `x` as a refused writer, and `g:` opened the command line, until a
    /// waiting prefix came first there.
    #[test]
    fn brackets_resolve_to_section_motions() {
        let section = |forward, end, count| CursorMotion::Section {
            forward,
            end,
            count,
        };
        for mut k in [VimKeymap::prompt(Mode::Normal), VimKeymap::chat()] {
            for (keys, forward, end, count) in [
                ("]]", true, false, 1),
                ("[[", false, false, 1),
                ("][", true, true, 1),
                ("[]", false, true, 1),
                ("3]]", true, false, 3),
                ("2][", true, true, 2),
            ] {
                assert_eq!(
                    feed(&mut k, keys),
                    KeyOutcome::edit(EditAction::Move(section(forward, end, count))),
                    "{keys}"
                );
            }
            // The counts multiply into the motion, `y6]]`, which fails whole
            // as `6]]` does (`:h operator`; review stop 16, A M6).
            assert_eq!(
                feed(&mut k, "2y3]]"),
                KeyOutcome::Operator {
                    op: Operator::Yank,
                    target: OpTarget::Motion(section(true, false, 6)),
                    count: 1,
                },
            );
            for keys in ["[x", "]x", "gx", "g:"] {
                assert_eq!(feed(&mut k, keys), KeyOutcome::Ignored, "`{keys}` cancels");
                assert_eq!(
                    feed(&mut k, "l"),
                    KeyOutcome::edit(EditAction::Move(CursorMotion::Right)),
                    "and nothing is left armed after `{keys}`"
                );
            }
        }
    }

    /// **`m{a-z}` names a mark and `'{a-z}` goes to one, `` `{a-z} `` to the
    /// agent's, through the one machine** — in the prompt's keymap and the
    /// transcript's alike (the app decides where marks are kept). Each is a
    /// motion: a count means nothing to it, Visual extends to it, and under
    /// an operator `'` is linewise and `` ` `` charwise, as vim's are. The
    /// name is any lowercase letter, no binding; anything else cancels and
    /// leaves nothing armed — so the key after `m` is never a command of its
    /// own (`ma` in a decision pane names `a`, never allows).
    #[test]
    fn m_and_quote_name_and_go_to_a_mark() {
        let to = |name| KeyOutcome::edit(EditAction::Move(CursorMotion::Mark(Whose::Yours, name)));
        let agents = |name| CursorMotion::Mark(Whose::Agents, name);
        for mut k in [VimKeymap::prompt(Mode::Normal), VimKeymap::chat()] {
            assert_eq!(feed(&mut k, "ma"), KeyOutcome::SetMark('a'));
            assert_eq!(feed(&mut k, "mz"), KeyOutcome::SetMark('z'));
            assert_eq!(feed(&mut k, "'a"), to('a'));
            assert_eq!(feed(&mut k, "3'q"), to('q'), "no count");
            assert_eq!(
                feed(&mut k, "y'a"),
                KeyOutcome::Operator {
                    op: Operator::Yank,
                    target: OpTarget::LineMotion(CursorMotion::Mark(Whose::Yours, 'a')),
                    count: 1,
                },
            );
            assert_eq!(
                feed(&mut k, "`a"),
                KeyOutcome::edit(EditAction::Move(agents('a'))),
                "the agent's"
            );
            assert_eq!(
                feed(&mut k, "y`a"),
                KeyOutcome::Operator {
                    op: Operator::Yank,
                    target: OpTarget::Motion(agents('a')),
                    count: 1,
                },
                "charwise, as vim's `"
            );
            assert_eq!(feed(&mut k, "m"), KeyOutcome::Ignored);
            assert!(k.pending_chord(), "`m` waits for the name");
            k.cancel_chord();
            for keys in ["m1", "mA", "m'", "'1", "'A", "''", "`1", "``"] {
                assert_eq!(feed(&mut k, keys), KeyOutcome::Ignored, "`{keys}` cancels");
                assert_eq!(
                    feed(&mut k, "l"),
                    KeyOutcome::edit(EditAction::Move(CursorMotion::Right)),
                    "and nothing is left armed after `{keys}`"
                );
            }
        }
        let mut visual = VimKeymap::chat();
        feed(&mut visual, "v");
        assert_eq!(feed(&mut visual, "'b"), to('b'), "Visual extends to it");
    }

    #[test]
    fn count_is_carried_into_the_insert_entry() {
        // `3a` carries the count into `EnterInsert` so the prompt can replay
        // the typed text three times on `<Esc>` (vim `3ia` → "aaa"). The
        // outcome's own `count` stays 1 — the repeat lives in the action.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('a'), KeyModifiers::NONE)),
            KeyOutcome::Edit {
                action: EditAction::EnterInsert {
                    entry: InsertEntry::After,
                    count: 3,
                },
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Insert);
    }

    #[test]
    fn count_is_cleared_when_entering_insert() {
        // A dangling count must not leak into the next Normal command
        // after a round-trip through Insert.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('5'), KeyModifiers::NONE));
        k.translate(press(KeyCode::Char('i'), KeyModifiers::NONE)); // enter Insert, drops count
        k.translate(press(KeyCode::Esc, KeyModifiers::NONE)); // back to Normal
        assert_eq!(
            k.translate(press(KeyCode::Char('l'), KeyModifiers::NONE)),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Right),
                count: 1,
            },
        );
    }

    /// **A count is none, or what was typed, and each reading says which**
    /// (review stop 16, D 3): a repeat is at least once, a product is none
    /// only when neither was typed, digits accumulate capped.
    #[test]
    fn a_count_is_none_or_what_was_typed() {
        let (none, two, three) = (Count::NONE, Count::of(2), Count::of(3));
        assert_eq!((none.typed(), none.repeat()), (None, 1));
        assert_eq!((three.typed(), three.repeat()), (Some(3), 3));
        assert_eq!(none.times(none), none, "none by none is none: `dG`");
        assert_eq!(none.times(three), three);
        assert_eq!(two.times(none), two);
        assert_eq!(two.times(three), Count::of(6));
        assert_eq!(Count::of(0), none);
        assert_eq!(none.push(4).push(0), Count::of(40));
        assert_eq!(Count::of(MAX_COUNT).push(9), Count::of(MAX_COUNT));
        assert!(!none.counting() && two.counting());
    }

    /// **A count typed in Visual reaches the search after it** (review stop
    /// 16, D 3, measured against nvim: `v3n` lands on the third match,
    /// still Visual; here it was the first) — in the prompt's Visual and a
    /// viewer's alike, as in Normal; an operator over the selection still
    /// applies once.
    #[test]
    fn a_count_in_visual_reaches_the_search_after_it() {
        let normal = feed(&mut VimKeymap::prompt(Mode::Normal), "3n");
        assert!(
            matches!(normal, KeyOutcome::Search { count: 3, .. }),
            "{normal:?}"
        );
        for mut k in [VimKeymap::prompt(Mode::Normal), VimKeymap::chat()] {
            feed(&mut k, "v");
            let searched = feed(&mut k, "3n");
            assert!(
                matches!(searched, KeyOutcome::Search { count: 3, .. }),
                "{searched:?}"
            );
            assert!(k.mode().is_visual(), "still Visual");
        }
        let mut k = VimKeymap::prompt(Mode::Normal);
        feed(&mut k, "v");
        assert_eq!(
            feed(&mut k, "3d"),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::Selection,
                count: 1,
            }
        );
    }

    /// Feed a sequence of plain (no-modifier) char keys, returning the
    /// outcome of the final keypress (the earlier ones are chord-builders
    /// expected to be `Ignored`).
    fn feed(k: &mut VimKeymap, keys: &str) -> KeyOutcome {
        let mut out = KeyOutcome::Ignored;
        for c in keys.chars() {
            let mods = if c.is_ascii_uppercase() || c == '$' {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            out = k.translate(press(KeyCode::Char(c), mods));
        }
        out
    }

    /// **What waits in the keymap picks the cursor's shape** (neovim's
    /// `cursor_get_mode_idx`): an operator waiting for its motion is `o`,
    /// counted or with a `g` begun; `r` waiting is Replace, in Normal and
    /// in Visual; a prefix, a count or `m` waiting is the mode itself.
    #[test]
    fn what_waits_picks_the_cursor_mode() {
        use crate::guicursor::CursorMode::{self, *};
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(k.cursor_mode(), Normal);
        let waits: [(&str, CursorMode); 8] = [
            ("d", Operator),
            ("2c", Operator),
            ("y", Operator),
            ("dg", Operator),
            ("r", Replace),
            ("g", Normal),
            ("3", Normal),
            ("m", Normal),
        ];
        for (keys, want) in waits {
            feed(&mut k, keys);
            assert_eq!(k.cursor_mode(), want, "after `{keys}`");
            k.cancel_chord();
        }
        k.translate(press(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(k.cursor_mode(), Normal, "after `<C-w>`");
        k.cancel_chord();
        feed(&mut k, "v");
        assert_eq!(k.cursor_mode(), Visual);
        feed(&mut k, "r");
        assert_eq!(k.cursor_mode(), Replace, "`r` in Visual");
        k.cancel_chord();
        k.translate(press(KeyCode::Esc, KeyModifiers::NONE));
        feed(&mut k, "R");
        assert_eq!(k.cursor_mode(), Replace);
        k.translate(press(KeyCode::Esc, KeyModifiers::NONE));
        feed(&mut k, "i");
        assert_eq!(k.cursor_mode(), Insert);
    }

    #[test]
    fn operator_then_motion_emits_operator_outcome() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        // `d` alone is a pending chord (nothing applied yet).
        assert_eq!(
            k.translate(press(KeyCode::Char('d'), KeyModifiers::NONE)),
            KeyOutcome::Ignored,
        );
        assert_eq!(
            k.translate(press(KeyCode::Char('w'), KeyModifiers::NONE)),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::Motion(CursorMotion::WordForward),
                count: 1,
            },
        );
        // Chord cleared: a bare `w` afterwards is a plain motion again.
        assert_eq!(
            k.translate(press(KeyCode::Char('w'), KeyModifiers::NONE)),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::WordForward),
                count: 1,
            },
        );
    }

    /// Every motion the table binds, with the keys that reach it: each
    /// `Motion` atom in `Context::Motion`, and each behind a motion prefix
    /// (`g`, `[`, `]`) in that prefix's context — walked from the table, so a
    /// binding added later is in it. Behind `'` and `` ` ``, whose
    /// completions are no table's, the first and last names (`'a`, `'z`).
    fn every_bound_motion() -> Vec<(Vec<Chord>, CursorMotion)> {
        let mut out = Vec::new();
        for first in DEFAULTS.chords_in(Context::Motion) {
            match DEFAULTS.get(Context::Motion, first) {
                Some(Atom::Motion(m)) => out.push((vec![first], m)),
                Some(Atom::Prefix(id)) => {
                    let Some(prefix) = MotionPrefix::of(id) else {
                        continue;
                    };
                    let Some(context) = prefix.context() else {
                        // Behind `f` `F` `t` `T`, a char: any, so a letter
                        // and one a binding would claim elsewhere (`;`).
                        if let MotionPrefix::Find { till, forward } = prefix {
                            for ch in ['x', ';'] {
                                let second = Chord {
                                    code: KeyCode::Char(ch),
                                    ctrl: false,
                                };
                                let find = crate::keymap::Find {
                                    till,
                                    forward,
                                    ch,
                                    count: 1,
                                    again: false,
                                };
                                out.push((vec![first, second], CursorMotion::FindChar(find)));
                            }
                            continue;
                        }
                        let MotionPrefix::Mark(whose) = prefix else {
                            panic!(
                                "a motion prefix with no table is a mark's or a find's: {prefix:?}"
                            );
                        };
                        for name in ['a', 'z'] {
                            let second = Chord {
                                code: KeyCode::Char(name),
                                ctrl: false,
                            };
                            out.push((vec![first, second], CursorMotion::Mark(whose, name)));
                        }
                        continue;
                    };
                    for second in DEFAULTS.chords_in(context) {
                        match DEFAULTS.get(context, second) {
                            Some(Atom::Motion(m)) => out.push((vec![first, second], m)),
                            Some(Atom::Prefix(_)) => panic!("a prefix behind {first:?}: walk it"),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Vim's class of a motion as an operator's target — `:h linewise`
    /// names `j` `k` `gg` `G` `H` `M` `L`, and `'` (`:h '`); the rest here
    /// are charwise (`:h exclusive`, `:h inclusive`), `` ` `` too (`:h `a`).
    /// Written apart from `is_charwise`, by an exhaustive `match`, so a new
    /// motion is classed here too.
    fn vim_linewise(m: CursorMotion) -> bool {
        use CursorMotion::*;
        match m {
            Up | Down | BufferStart | BufferEnd | GotoLine(_) | Screen(_) => true,
            Mark(Whose::Yours, _) => true,
            // `:h N%`: "linewise".
            Percent(_) => true,
            Mark(Whose::Agents, _)
            | Left
            | Right
            | LineStart
            | LineEnd
            | WordForward
            | WordBackward
            | WordEnd
            | BigWordForward
            | BigWordBackward
            | BigWordEnd
            | Section { .. }
            // `:h f` `:h ;` `:h ^` `:h ge` inclusive or exclusive, `:h }`
            // exclusive, `:h %` inclusive: charwise, every one.
            | FindChar(_)
            | RepeatFind { .. }
            | FirstNonBlank
            | WordEndBackward { .. }
            | Paragraph { .. }
            | MatchPair => false,
        }
    }

    /// **Every bound motion moves, and targets an operator as vim's does** —
    /// in the prompt's Normal and Visual and the transcript's, and under the
    /// prompt's `d` and the transcript's `y`. Hand lists named a few
    /// (`W` `B` `E`, `j` `k`, `G` `gg`); dropping `0` and `$` from
    /// `is_charwise`, so `d$` deleted the whole line, passed every test
    /// (review stop 12, T6 3).
    #[test]
    fn every_bound_motion_moves_and_targets_as_vim_does() {
        let keys = |k: &mut VimKeymap, chords: &[Chord]| {
            let mut out = KeyOutcome::Ignored;
            for c in chords {
                let mods = match c.ctrl {
                    true => KeyModifiers::CONTROL,
                    false => KeyModifiers::NONE,
                };
                out = k.translate(press(c.code, mods));
            }
            out
        };
        let motions = every_bound_motion();
        // Premise: the walk reaches each family, the prefixed ones too.
        for family in [
            |m: &CursorMotion| *m == CursorMotion::LineEnd,
            |m: &CursorMotion| *m == CursorMotion::Down,
            |m: &CursorMotion| *m == CursorMotion::BufferStart,
            |m: &CursorMotion| matches!(m, CursorMotion::Screen(_)),
            |m: &CursorMotion| matches!(m, CursorMotion::Section { .. }),
            |m: &CursorMotion| matches!(m, CursorMotion::Mark(Whose::Yours, _)),
            |m: &CursorMotion| matches!(m, CursorMotion::Mark(Whose::Agents, _)),
        ] {
            assert!(
                motions.iter().any(|(_, m)| family(m)),
                "premise: {motions:?}"
            );
        }
        for (chords, m) in &motions {
            let moved = KeyOutcome::Edit {
                action: EditAction::Move(*m),
                count: 1,
            };
            let normal = [VimKeymap::prompt(Mode::Normal), VimKeymap::chat()];
            for (mut k, pane) in normal.into_iter().zip(["prompt", "transcript"]) {
                assert_eq!(keys(&mut k, chords), moved, "{chords:?} in the {pane}");
            }
            let visual = [VimKeymap::prompt(Mode::Normal), VimKeymap::chat()];
            for (mut k, pane) in visual.into_iter().zip(["prompt", "transcript"]) {
                k.translate(plain('v'));
                assert_eq!(
                    keys(&mut k, chords),
                    moved,
                    "{chords:?} in the {pane}'s Visual"
                );
            }
            let target = match vim_linewise(*m) {
                true => OpTarget::LineMotion(*m),
                false => OpTarget::Motion(*m),
            };
            for (mut k, op, key) in [
                (VimKeymap::prompt(Mode::Normal), Operator::Delete, 'd'),
                (VimKeymap::chat(), Operator::Yank, 'y'),
            ] {
                k.translate(plain(key));
                let want = KeyOutcome::Operator {
                    op,
                    target,
                    count: 1,
                };
                assert_eq!(keys(&mut k, chords), want, "`{key}` then {chords:?}");
            }
        }
    }

    #[test]
    fn doubled_operator_is_linewise() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            feed(&mut k, "dd"),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::Line,
                count: 1,
            },
        );
        assert_eq!(
            feed(&mut k, "cc"),
            KeyOutcome::Operator {
                op: Operator::Change,
                target: OpTarget::Line,
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Insert, "cc enters Insert");
    }

    #[test]
    fn counts_multiply_around_the_operator() {
        // `2d3w` deletes six words.
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            feed(&mut k, "2d3w"),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::Motion(CursorMotion::WordForward),
                count: 6,
            },
        );
    }

    #[test]
    fn capital_d_and_c_are_to_line_end() {
        // `D`/`C` are the `LineTail` shape with line count 1 (= `d$`/`c$`).
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            k.translate(press(KeyCode::Char('D'), KeyModifiers::SHIFT)),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::LineTail(1),
                count: 1,
            },
        );
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            k.translate(press(KeyCode::Char('C'), KeyModifiers::SHIFT)),
            KeyOutcome::Operator {
                op: Operator::Change,
                target: OpTarget::LineTail(1),
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Insert, "C enters Insert");
    }

    #[test]
    fn counted_capital_d_carries_the_line_count() {
        // `3D` → delete to EOL plus the two rows below (`LineTail(3)`), not a
        // no-op repeat of `D`.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('D'), KeyModifiers::SHIFT)),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::LineTail(3),
                count: 1,
            },
        );
    }

    #[test]
    fn change_operator_enters_insert() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            feed(&mut k, "cw"),
            KeyOutcome::Operator {
                op: Operator::Change,
                target: OpTarget::Motion(CursorMotion::WordForward),
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Insert);
    }

    #[test]
    fn operator_then_unmapped_key_cancels() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('d'), KeyModifiers::NONE));
        // `dz` is not a command: cancel.
        assert_eq!(
            k.translate(press(KeyCode::Char('z'), KeyModifiers::NONE)),
            KeyOutcome::Ignored,
        );
        // Back to clean Normal: `l` is a plain motion, not eaten by a
        // stale operator.
        assert_eq!(
            k.translate(press(KeyCode::Char('l'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::Move(CursorMotion::Right)),
        );
    }

    #[test]
    fn counted_cross_row_operator_multiplies() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            feed(&mut k, "2dj"),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::LineMotion(CursorMotion::Down),
                count: 2,
            },
        );
    }

    #[test]
    fn counted_g_and_gg_go_to_an_absolute_line() {
        // `{count}G`/`{count}gg` resolve to `GotoLine(count)` with repeat 1 —
        // the count is the target line, not a repeat of `G`.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::Move(CursorMotion::GotoLine(3))),
        );
        // Multi-digit: `10G` → line 10.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('1'), KeyModifiers::NONE));
        k.translate(press(KeyCode::Char('0'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::Move(CursorMotion::GotoLine(10))),
        );
        // `3gg` → line 3 too (same fold through the `g` prefix).
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(
            feed(&mut k, "gg"),
            KeyOutcome::edit(EditAction::Move(CursorMotion::GotoLine(3))),
        );
    }

    #[test]
    fn counted_operator_to_g_targets_an_absolute_line() {
        // `d3G` deletes linewise *to line 3* — a `GotoLine` LineMotion, not
        // three `G`s (which would collapse to the last line).
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('d'), KeyModifiers::NONE));
        k.translate(press(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::LineMotion(CursorMotion::GotoLine(3)),
                count: 1,
            },
        );
    }

    #[test]
    fn v_enters_visual_and_anchors() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::SetAnchor),
        );
        assert_eq!(k.mode(), Mode::Visual);
    }

    #[test]
    fn visual_motion_extends_selection() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('l'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::Move(CursorMotion::Right)),
        );
        assert_eq!(k.mode(), Mode::Visual, "motion keeps selecting");
        assert_eq!(
            k.translate(press(KeyCode::Char('w'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::Move(CursorMotion::WordForward)),
        );
    }

    #[test]
    fn visual_delete_operates_on_selection_and_exits() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('d'), KeyModifiers::NONE)),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::Selection,
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn visual_change_enters_insert() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('c'), KeyModifiers::NONE)),
            KeyOutcome::Operator {
                op: Operator::Change,
                target: OpTarget::Selection,
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Insert);
    }

    #[test]
    fn visual_esc_and_v_clear_and_exit() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Esc, KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::ClearAnchor),
        );
        assert_eq!(k.mode(), Mode::Normal);
        // `v` while already Visual toggles back out.
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::ClearAnchor),
        );
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn yank_operator_then_motion_stays_normal() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            feed(&mut k, "yw"),
            KeyOutcome::Operator {
                op: Operator::Yank,
                target: OpTarget::Motion(CursorMotion::WordForward),
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Normal, "yank never enters Insert");
    }

    #[test]
    fn yy_and_capital_y_are_linewise_yank() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            feed(&mut k, "yy"),
            KeyOutcome::Operator {
                op: Operator::Yank,
                target: OpTarget::Line,
                count: 1,
            },
        );
        // `Y` (shifted) is the `yy` shorthand, with a count.
        k.translate(press(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('Y'), KeyModifiers::SHIFT)),
            KeyOutcome::Operator {
                op: Operator::Yank,
                target: OpTarget::Line,
                count: 2,
            },
        );
    }

    #[test]
    fn yank_cross_row_motion_is_linewise() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            feed(&mut k, "yj"),
            KeyOutcome::Operator {
                op: Operator::Yank,
                target: OpTarget::LineMotion(CursorMotion::Down),
                count: 1,
            },
        );
    }

    #[test]
    fn yank_text_object() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        // yi" — feed `y`, `i`, then `"` (shifted).
        k.translate(press(KeyCode::Char('y'), KeyModifiers::NONE));
        k.translate(press(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('"'), KeyModifiers::SHIFT)),
            KeyOutcome::Operator {
                op: Operator::Yank,
                target: OpTarget::TextObject {
                    scope: TextObjectScope::Inner,
                    object: TextObject::Quote('"'),
                },
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn paste_keys_emit_paste_edits_with_count() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            k.translate(press(KeyCode::Char('p'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::Paste),
        );
        assert_eq!(
            k.translate(press(KeyCode::Char('P'), KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::PasteBefore),
        );
        // `3p` carries the count.
        k.translate(press(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('p'), KeyModifiers::NONE)),
            KeyOutcome::Edit {
                action: EditAction::Paste,
                count: 3,
            },
        );
    }

    #[test]
    fn visual_yank_yanks_selection_and_exits() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('y'), KeyModifiers::NONE)),
            KeyOutcome::Operator {
                op: Operator::Yank,
                target: OpTarget::Selection,
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Normal, "visual yank returns to Normal");
    }

    #[test]
    fn capital_v_enters_linewise_visual() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            k.translate(press(KeyCode::Char('V'), KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::SetAnchorLine),
        );
        assert_eq!(k.mode(), Mode::VisualLine);
    }

    #[test]
    fn linewise_visual_operates_over_the_selection() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('V'), KeyModifiers::SHIFT));
        // The same `OpTarget::Selection` — the prompt resolves it linewise.
        assert_eq!(
            k.translate(press(KeyCode::Char('d'), KeyModifiers::NONE)),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::Selection,
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn v_and_capital_v_toggle_selection_kind_keeping_the_anchor() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE)); // charwise
        // `V` switches to linewise (a kind flip, not a re-anchor or exit).
        assert_eq!(
            k.translate(press(KeyCode::Char('V'), KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::SetSelectionKind(SelectionKind::Line)),
        );
        assert_eq!(k.mode(), Mode::VisualLine);
        // `v` switches back to charwise.
        assert_eq!(
            k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::SetSelectionKind(SelectionKind::Char)),
        );
        assert_eq!(k.mode(), Mode::Visual);
        // `v` again exits (already charwise).
        assert_eq!(
            k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::ClearAnchor),
        );
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn capital_v_exits_linewise_visual() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('V'), KeyModifiers::SHIFT));
        assert_eq!(
            k.translate(press(KeyCode::Char('V'), KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::ClearAnchor),
        );
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn ctrl_v_enters_block_visual() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            k.translate(press(KeyCode::Char('v'), KeyModifiers::CONTROL)),
            KeyOutcome::edit(EditAction::SetAnchorBlock),
        );
        assert_eq!(k.mode(), Mode::VisualBlock);
    }

    #[test]
    fn block_visual_operates_over_the_selection() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert_eq!(
            k.translate(press(KeyCode::Char('d'), KeyModifiers::NONE)),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::Selection,
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn ctrl_v_switches_to_block_then_exits() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE)); // charwise
        // Ctrl-V switches charwise → block (kind flip, anchor kept).
        assert_eq!(
            k.translate(press(KeyCode::Char('v'), KeyModifiers::CONTROL)),
            KeyOutcome::edit(EditAction::SetSelectionKind(SelectionKind::Block)),
        );
        assert_eq!(k.mode(), Mode::VisualBlock);
        // Ctrl-V again exits.
        assert_eq!(
            k.translate(press(KeyCode::Char('v'), KeyModifiers::CONTROL)),
            KeyOutcome::edit(EditAction::ClearAnchor),
        );
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn block_visual_capital_i_and_a_begin_block_insert() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::CONTROL)); // block
        assert_eq!(
            k.translate(press(KeyCode::Char('I'), KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::BeginBlockInsert(BlockEdge::Left)),
        );
        assert_eq!(k.mode(), Mode::Insert);

        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert_eq!(
            k.translate(press(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::BeginBlockInsert(BlockEdge::Right)),
        );
        assert_eq!(k.mode(), Mode::Insert);
    }

    #[test]
    fn visual_gg_and_counts_extend_like_normal() {
        // Regression: Visual ran its own motion handling that dropped the
        // `g` prefix (and counts), so `G` worked but `gg` did not. Now Visual
        // shares Normal's accumulator.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE)); // charwise visual
        // `gg`: first `g` pends, second resolves (previously this was Ignored).
        assert_eq!(
            k.translate(press(KeyCode::Char('g'), KeyModifiers::NONE)),
            KeyOutcome::Ignored,
        );
        assert_eq!(
            k.translate(press(KeyCode::Char('g'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::Move(CursorMotion::BufferStart)),
        );
        assert_eq!(k.mode(), Mode::Visual, "motion keeps selecting");
        // Counts work too: `3j` extends three rows.
        k.translate(press(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('j'), KeyModifiers::NONE)),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Down),
                count: 3,
            },
        );
    }

    #[test]
    fn block_visual_gg_works_like_capital_g() {
        // The exact reported divergence: `G` worked in block Visual, `gg` not.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::CONTROL)); // block visual
        assert_eq!(
            k.translate(press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::Move(CursorMotion::BufferEnd)),
        );
        assert_eq!(k.mode(), Mode::VisualBlock);
        k.translate(press(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('g'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::Move(CursorMotion::BufferStart)),
        );
        assert_eq!(k.mode(), Mode::VisualBlock, "still selecting after gg");
    }

    #[test]
    fn capital_i_in_charwise_visual_is_not_block_insert() {
        // `I`/`A` are block-insert only in block mode; charwise Visual leaves
        // them alone (no spurious mode change).
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE)); // charwise
        assert_eq!(
            k.translate(press(KeyCode::Char('I'), KeyModifiers::SHIFT)),
            KeyOutcome::Ignored,
        );
        assert_eq!(k.mode(), Mode::Visual);
    }

    /// **A text object takes its count** — `v2ap`, `d2ap`, `2dap`, `2d3ap` —
    /// and leaves none pending: `v2ap` selected one paragraph and the next `j`
    /// moved twice, and an operator's object always counted one (review stop
    /// 6, A 2, C 3). The editor's Visual and a viewer's alike.
    #[test]
    fn a_text_object_takes_its_count() {
        let typed = |k: &mut VimKeymap, keys: &str| -> Vec<KeyOutcome> {
            keys.chars()
                .map(|c| k.translate(press(KeyCode::Char(c), KeyModifiers::NONE)))
                .collect()
        };
        let paragraphs = |count| {
            KeyOutcome::edit(EditAction::SelectTextObject {
                scope: TextObjectScope::Around,
                object: TextObject::Paragraph,
                count,
            })
        };
        for mut k in [VimKeymap::prompt(Mode::Normal), VimKeymap::chat()] {
            let said = typed(&mut k, "v2ap");
            assert_eq!(said.last(), Some(&paragraphs(2)), "v2ap");
            assert_eq!(
                typed(&mut k, "j").last(),
                Some(&KeyOutcome::Edit {
                    action: EditAction::Move(CursorMotion::Down),
                    count: 1
                }),
                "no count left pending"
            );
        }
        for (keys, count) in [("d2ap", 2), ("2dap", 2), ("2d3ap", 6), ("dap", 1)] {
            let mut k = VimKeymap::prompt(Mode::Normal);
            let said = typed(&mut k, keys);
            assert_eq!(
                said.last(),
                Some(&KeyOutcome::Operator {
                    op: Operator::Delete,
                    target: OpTarget::TextObject {
                        scope: TextObjectScope::Around,
                        object: TextObject::Paragraph,
                    },
                    count,
                }),
                "{keys}"
            );
        }
    }

    #[test]
    fn visual_text_object_viw_selects_inner_word() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE)); // enter Visual
        assert_eq!(k.mode(), Mode::Visual);
        // `i` arms the chord (silent); `w` completes it into a selection.
        assert_eq!(
            k.translate(press(KeyCode::Char('i'), KeyModifiers::NONE)),
            KeyOutcome::Ignored,
        );
        assert_eq!(
            k.translate(press(KeyCode::Char('w'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::SelectTextObject {
                scope: TextObjectScope::Inner,
                object: TextObject::Word,
                count: 1,
            }),
        );
        assert_eq!(k.mode(), Mode::Visual, "stays in Visual after selecting");
    }

    #[test]
    fn visual_text_object_va_quote_selects_around_quotes() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE));
        k.translate(press(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('"'), KeyModifiers::SHIFT)),
            KeyOutcome::edit(EditAction::SelectTextObject {
                scope: TextObjectScope::Around,
                object: TextObject::Quote('"'),
                count: 1,
            }),
        );
    }

    #[test]
    fn visual_o_swaps_selection_ends() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('v'), KeyModifiers::NONE)); // enter Visual
        assert_eq!(
            k.translate(press(KeyCode::Char('o'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::SwapSelectionEnds),
        );
        assert_eq!(k.mode(), Mode::Visual, "o stays in Visual");
    }

    #[test]
    fn text_object_chord_inner_word() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        // `c` and `i` are silent chord-builders; `w` completes it.
        assert_eq!(feed(&mut k, "ciw"), {
            KeyOutcome::Operator {
                op: Operator::Change,
                target: OpTarget::TextObject {
                    scope: TextObjectScope::Inner,
                    object: TextObject::Word,
                },
                count: 1,
            }
        });
        assert_eq!(k.mode(), Mode::Insert, "ciw enters Insert");
    }

    #[test]
    fn text_object_chord_around_quote() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        // da" — feed `d`, `a`, then `"` (Shift).
        k.translate(press(KeyCode::Char('d'), KeyModifiers::NONE));
        k.translate(press(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('"'), KeyModifiers::SHIFT)),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::TextObject {
                    scope: TextObjectScope::Around,
                    object: TextObject::Quote('"'),
                },
                count: 1,
            },
        );
    }

    #[test]
    fn text_object_bracket_aliases() {
        // `b` is an alias for `(` as a text object.
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            feed(&mut k, "dib"),
            KeyOutcome::Operator {
                op: Operator::Delete,
                target: OpTarget::TextObject {
                    scope: TextObjectScope::Inner,
                    object: TextObject::Pair('(', ')'),
                },
                count: 1,
            },
        );
    }

    #[test]
    fn text_object_unmapped_object_cancels() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(press(KeyCode::Char('c'), KeyModifiers::NONE));
        k.translate(press(KeyCode::Char('i'), KeyModifiers::NONE));
        // `ciz` — `z` is not a text object: cancel, stay Normal.
        assert_eq!(
            k.translate(press(KeyCode::Char('z'), KeyModifiers::NONE)),
            KeyOutcome::Ignored,
        );
        assert_eq!(
            k.mode(),
            Mode::Normal,
            "cancelled change does not enter Insert"
        );
        // Chord fully cleared.
        assert_eq!(
            k.translate(press(KeyCode::Char('l'), KeyModifiers::NONE)),
            KeyOutcome::edit(EditAction::Move(CursorMotion::Right)),
        );
    }

    /// **A digit after `z` cancels** (`command_key`): vim's `z{height}<CR>`
    /// is a window height, so `z5t` is not `5zt` — read as a count it would
    /// put line 5 at the top. Held by nothing: digits accumulating after `z`
    /// as after `<C-w>` passed the suite (review stop 12, T6 8).
    #[test]
    fn a_digit_after_z_cancels_the_chord() {
        for mut k in [VimKeymap::chat(), VimKeymap::prompt(Mode::Normal)] {
            let mut key = |c| k.translate(plain(c));
            assert_eq!(key('z'), KeyOutcome::Ignored, "premise: `z` pending");
            assert_eq!(key('5'), KeyOutcome::Ignored);
            let t = key('t');
            assert!(
                !matches!(t, KeyOutcome::Scroll(ScrollCmd::Place { .. })),
                "`z5t` placed a line: {t:?}"
            );
            // `t` begins a find now (`tj`), so the count's witness is the
            // find's: one, not five.
            assert_eq!(
                key('j'),
                KeyOutcome::Edit {
                    action: EditAction::Move(CursorMotion::FindChar(crate::keymap::Find {
                        till: true,
                        forward: true,
                        ch: 'j',
                        count: 1,
                        again: false,
                    })),
                    count: 1,
                },
                "and the count did not carry"
            );
        }
    }

    #[test]
    fn normal_editor_arrows_move_cursor() {
        use EditAction::*;
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(
            k.translate(press(KeyCode::Left, KeyModifiers::NONE)),
            KeyOutcome::edit(Move(CursorMotion::Left)),
        );
        assert_eq!(
            k.translate(press(KeyCode::Down, KeyModifiers::NONE)),
            KeyOutcome::edit(Move(CursorMotion::Down)),
        );
    }

    /// `i` and `Enter` are refused in a viewer — and **for different reasons**,
    /// which the user is told.
    ///
    /// Both used to answer `Ignored`, i.e. exactly what an unbound key answers,
    /// so a dead `i` was indistinguishable from a broken keyboard. They are also
    /// not the same refusal: `i` has no buffer to write, while `Enter` would
    /// send a buffer you cannot see from here — so asserting they differ is what
    /// stops the two collapsing into one unhelpful sentence.
    #[test]
    fn chat_keymap_has_no_insert_or_submit() {
        let mut k = VimKeymap::chat();
        assert_eq!(k.mode(), Mode::Normal);
        // `i` does not enter Insert (chat is a viewer) — and says why.
        let insert = k.translate(press(KeyCode::Char('i'), KeyModifiers::NONE));
        assert_eq!(insert, KeyOutcome::Refused(WRITES));
        assert_eq!(k.mode(), Mode::Normal);
        // Enter does not submit from the chat pane — and says a different why.
        let submit = k.translate(press(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(submit, KeyOutcome::Refused(SENDS));
        assert_ne!(
            insert, submit,
            "two different impossibilities must not read as one message"
        );
        // `j` moves the cursor (the chat is a read-only cursor surface).
        assert_eq!(
            k.translate(press(KeyCode::Char('j'), KeyModifiers::NONE)),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Down),
                count: 1,
            },
        );
    }

    /// **A read-only pane is Normal mode minus the writers** — asked of the
    /// table, not of a hand-written list of keys.
    ///
    /// The hand-list was three keys long, so `:` did nothing in the transcript
    /// and `:graph` could only be typed from the prompt: backwards, since
    /// looking at history is exactly when you want it. Submit stays out
    /// deliberately (it sends a buffer you cannot see from here).
    #[test]
    fn chat_viewer_takes_every_non_writing_normal_command() {
        let mut k = VimKeymap::chat();
        assert_eq!(
            k.translate(press(KeyCode::Char(':'), KeyModifiers::NONE)),
            KeyOutcome::EnterCommand,
            "`:` must work from a viewer",
        );
        // `Y` copies out — reading, so it belongs here.
        assert!(matches!(
            VimKeymap::chat().translate(press(KeyCode::Char('Y'), KeyModifiers::NONE)),
            KeyOutcome::Operator { .. }
        ));
        // …and every writer is still refused — *visibly*. `Ignored` would mean
        // "no such binding", which is the one thing that is not true of these.
        for key in ['i', 'a', 'o', 'p', 'u', 'x', 'D', 'C'] {
            assert_eq!(
                VimKeymap::chat().translate(press(KeyCode::Char(key), KeyModifiers::NONE)),
                KeyOutcome::Refused(WRITES),
                "`{key}` writes; a viewer must refuse it and say so",
            );
        }
    }

    /// **`v` `<C-w>` `c` must close a window, not delete your selection.**
    ///
    /// `Context::Visual` binds no control chord but `<C-v>`, so `<C-w>` was
    /// swallowed and its SECOND key executed as a Visual command: `c` is
    /// `visual-change`, which deletes the selection and enters Insert. The user
    /// typed a window chord and lost text.
    ///
    /// Both halves are asserted. Leaving Visual is not enough on its own — the
    /// app owns the selection, so without the `ClearAnchor` the highlight
    /// outlives the mode. And arming the prefix is not enough either, or `c`
    /// still reaches the Visual table.
    #[test]
    fn a_window_chord_from_visual_closes_a_window_and_never_edits() {
        for mut k in [VimKeymap::prompt(Mode::Normal), VimKeymap::chat()] {
            k.translate(plain('v'));
            assert!(
                matches!(k.mode(), Mode::Visual),
                "premise — `v` must enter Visual, or this tests nothing"
            );

            assert_eq!(
                k.translate(ctrl('w')),
                KeyOutcome::edit(EditAction::ClearAnchor),
                "`<C-w>` must drop the selection the app is drawing"
            );
            assert_eq!(k.mode(), Mode::Normal, "vim's nv_window ends Visual");

            // The dangerous key, second: `c` must be the WINDOW command, not
            // `visual-change`.
            assert_eq!(
                k.translate(plain('c')),
                KeyOutcome::Window(WindowCmd::Close, None),
                "the chord's second key must not reach the Visual table"
            );
        }
    }

    /// The refusal a read-only pane shows names `<C-w>w`, and it works from the
    /// state that produced it.
    ///
    /// The message was reachable from a Visual selection in the transcript,
    /// where `<C-w>` did nothing and `w` then extended the selection by a word
    /// — advice that silently did the wrong thing (Reviewer A).
    #[test]
    fn the_way_out_named_by_a_refusal_works_from_where_it_is_shown() {
        let mut k = VimKeymap::chat();
        k.translate(plain('v'));
        assert_eq!(
            k.translate(plain('d')),
            KeyOutcome::Refused(WRITES),
            "premise — this is the message under test"
        );
        assert!(
            WRITES.contains("<C-w>w"),
            "premise — the message names this key: {WRITES}"
        );
        k.translate(ctrl('w'));
        assert_eq!(
            k.translate(plain('w')),
            KeyOutcome::Window(WindowCmd::FocusCycle, None),
            "`<C-w>w` must cycle focus, not extend the selection"
        );
    }

    /// **A refused key ends the chord it was typed in.**
    ///
    /// `3i` in the transcript is a count followed by a key that cannot act
    /// here. The count is spent either way — vim ends the chord on any command
    /// key, not only on ones that succeed — so leaving `pending` armed would
    /// make the *next* `j` move three lines, for a key the user was just told
    /// did nothing. The success path already cleared it (`3:` and `g:` both had
    /// this bug); the refusal path is new and inherits the same rule.
    #[test]
    fn a_refused_key_ends_the_chord_it_was_typed_in() {
        let mut k = VimKeymap::chat();
        k.translate(plain('3'));
        assert_eq!(
            k.translate(plain('i')),
            KeyOutcome::Refused(WRITES),
            "premise — `i` must be the refused key, or this tests nothing"
        );
        assert_eq!(
            k.translate(plain('j')),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Down),
                count: 1,
            },
            "the count died with the refused key; `j` moves ONE line"
        );
    }

    #[test]
    fn chat_viewer_has_full_vim_navigation() {
        let mut k = VimKeymap::chat();
        let mv = |action, count| KeyOutcome::Edit { action, count };
        // The chat is a cursor surface now: motions move the cursor (the app
        // applies them to the chat buffer; the viewport follows).
        // `gg` → buffer start, `G` → buffer end.
        k.translate(press(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('g'), KeyModifiers::NONE)),
            mv(EditAction::Move(CursorMotion::BufferStart), 1),
        );
        assert_eq!(
            k.translate(press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            mv(EditAction::Move(CursorMotion::BufferEnd), 1),
        );
        // `{count}j` moves the cursor that many rows.
        k.translate(press(KeyCode::Char('5'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('j'), KeyModifiers::NONE)),
            mv(EditAction::Move(CursorMotion::Down), 5),
        );
        // `{count}G` → absolute line.
        k.translate(press(KeyCode::Char('1'), KeyModifiers::NONE));
        k.translate(press(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(
            k.translate(press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            mv(EditAction::Move(CursorMotion::GotoLine(12)), 1),
        );
        // Within-line motions now work too (it's a cursor, not a scroll).
        assert_eq!(
            k.translate(press(KeyCode::Char('w'), KeyModifiers::NONE)),
            mv(EditAction::Move(CursorMotion::WordForward), 1),
        );
        // `Ctrl-F`/`Ctrl-B` still page the viewport directly.
        assert_eq!(
            k.translate(press(KeyCode::Char('f'), KeyModifiers::CONTROL)),
            KeyOutcome::Scroll(ScrollCmd::Down(Page)),
        );
    }

    #[test]
    fn chat_viewer_visual_and_yank() {
        let mut k = VimKeymap::chat();
        // `v` enters charwise Visual (anchors a selection).
        assert_eq!(
            k.translate(plain('v')),
            KeyOutcome::edit(EditAction::SetAnchor)
        );
        assert_eq!(k.mode(), Mode::Visual);
        // A motion extends the selection (a `Move`, not a scroll).
        assert_eq!(
            k.translate(plain('l')),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Right),
                count: 1,
            },
        );
        // `y` yanks the selection and leaves Visual.
        assert_eq!(
            k.translate(plain('y')),
            KeyOutcome::Operator {
                op: Operator::Yank,
                target: OpTarget::Selection,
                count: 1,
            },
        );
        assert_eq!(k.mode(), Mode::Normal);
        // Mutating ops are gated — no `d`/`c` in the read-only chat, and the
        // gate says so: the selection is on screen, so a silent `d` is the one
        // refusal a user has visible evidence should have worked.
        k.translate(plain('v'));
        assert_eq!(k.translate(plain('d')), KeyOutcome::Refused(WRITES));
        assert_eq!(k.translate(plain('c')), KeyOutcome::Refused(WRITES));
        assert_eq!(k.mode(), Mode::Visual, "a gated key doesn't leave Visual");
        // The case and replace family too, in Visual and in Normal — and a
        // refused `r` arms nothing, so the next key is not eaten as its char.
        for key in ['~', 'u', 'U', 'r'] {
            assert_eq!(
                k.translate(plain(key)),
                KeyOutcome::Refused(WRITES),
                "{key}"
            );
        }
        k.translate(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        for key in ['~', 'r', 'R'] {
            assert_eq!(
                k.translate(plain(key)),
                KeyOutcome::Refused(WRITES),
                "{key}"
            );
        }
        assert!(!k.pending_chord(), "nothing armed");
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn chat_viewer_count_does_not_leak_into_visual() {
        // `3v` enters Visual; the leading count must NOT carry into the first
        // selection motion (vim: `3v` selects from the cursor, the motion takes
        // its own count). Reviewer-found: `enter_visual` left `pending` set.
        let mut k = VimKeymap::chat();
        k.translate(plain('3'));
        k.translate(plain('v'));
        assert_eq!(k.mode(), Mode::Visual);
        assert_eq!(
            k.translate(plain('l')),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Right),
                count: 1,
            },
        );
        // **And a drag after a count** (review stop 12, T6 7): both key
        // paths clear the count before `enter_visual`, so its own clear was
        // held by nothing — but a mouse drag (`begin_visual`) goes past
        // both, and `3` then a drag extended the first motion ×3.
        for mut k in [VimKeymap::chat(), VimKeymap::prompt(Mode::Normal)] {
            k.translate(plain('3'));
            k.begin_visual();
            assert_eq!(k.mode(), Mode::Visual);
            assert_eq!(
                k.translate(plain('l')),
                KeyOutcome::Edit {
                    action: EditAction::Move(CursorMotion::Right),
                    count: 1,
                },
                "a drag after `3`"
            );
        }
    }

    /// **A Ctrl chord is a chord too.** The viewer answered `<C-w>/<C-u>/
    /// <C-d>/<C-f>/<C-b>/<C-v>` from a hand-written `match key.code` that
    /// returned *before* the table lookup — so the comment directly below it,
    /// claiming "the table decides now… an atom added later is covered or is a
    /// compile error", was true for plain keys and false for every Ctrl chord
    /// (Reviewer D).
    ///
    /// Two consequences, both of them the bugs
    /// `chat_viewer_a_command_ends_the_chord_it_was_typed_in` exists for, one
    /// modifier over: the count leaked to the next key, and a
    /// `[keymap.normal]` remap of `<C-u>` was honoured in the prompt and
    /// ignored in the viewer.
    #[test]
    fn chat_viewer_a_ctrl_chord_ends_the_chord_it_was_typed_in() {
        let mut k = VimKeymap::chat();
        k.translate(plain('3'));
        assert_eq!(
            k.translate(ctrl('u')),
            KeyOutcome::Scroll(ScrollCmd::Up(HalfPage))
        );
        assert_eq!(
            k.translate(plain('j')),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Down),
                count: 1,
            },
            "the count went with the <C-u>, not on to the next motion"
        );
    }

    /// A non-writing Normal command ends the chord it was typed in.
    ///
    /// The viewer's `in_viewer` lookup returns **before** `step_motion`, which
    /// is where the editor path clears `pending` — so it has to clear it
    /// itself. Both halves of that were real dogfooding bugs, and the guard
    /// that fixed them had **no test**: reviewers mutation-verified that
    /// deleting the line passed the whole suite. This is the test that was
    /// owed (`[[feedback_mutation_check_guards]]`).
    #[test]
    fn chat_viewer_a_command_ends_the_chord_it_was_typed_in() {
        // `3:` — the count belongs to the `:` that consumed it, not to the
        // next key. It used to stay armed, so `j` afterwards moved THREE lines.
        let mut k = VimKeymap::chat();
        k.translate(plain('3'));
        assert_eq!(k.translate(plain(':')), KeyOutcome::EnterCommand);
        assert_eq!(
            k.translate(plain('j')),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Down),
                count: 1,
            },
            "the count went with the `:`, not on to the next motion"
        );

        // `g:` — a half-consumed `g` prefix used to swallow the key after the
        // command, so the next `j` did nothing at all. The `:` is the `g`'s
        // to complete, as in the prompt (vim has no `g:`): it cancels the
        // chord, and the key after it is not eaten.
        let mut k = VimKeymap::chat();
        k.translate(plain('g'));
        assert_eq!(k.translate(plain(':')), KeyOutcome::Ignored);
        assert_eq!(
            k.translate(plain('j')),
            KeyOutcome::Edit {
                action: EditAction::Move(CursorMotion::Down),
                count: 1,
            },
            "the `g` went with the `:` — the next key is not eaten"
        );
    }

    #[test]
    fn chat_viewer_operator_yank() {
        let mut k = VimKeymap::chat();
        // `yw` — operator-pending yank over a motion (Yank is the chat's only op).
        k.translate(plain('y'));
        assert_eq!(
            k.translate(plain('w')),
            KeyOutcome::Operator {
                op: Operator::Yank,
                target: OpTarget::Motion(CursorMotion::WordForward),
                count: 1,
            },
        );
        // `yy` — doubled operator, linewise.
        k.translate(plain('y'));
        assert_eq!(
            k.translate(plain('y')),
            KeyOutcome::Operator {
                op: Operator::Yank,
                target: OpTarget::Line,
                count: 1,
            },
        );
        // `d`/`c` never begin an operator in the read-only chat, and refuse out
        // loud rather than looking unbound.
        assert_eq!(k.translate(plain('d')), KeyOutcome::Refused(WRITES));
        assert_eq!(k.translate(plain('c')), KeyOutcome::Refused(WRITES));
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn ctrl_x_ctrl_e_opens_the_editor() {
        // The `$EDITOR` chord works mode-independently (readline's binding).
        let mut k = VimKeymap::prompt(Mode::Insert);
        assert_eq!(k.translate(ctrl('x')), KeyOutcome::Ignored, "Ctrl-X arms");
        assert_eq!(k.translate(ctrl('e')), KeyOutcome::OpenEditor);
        // From Normal too.
        let mut k = VimKeymap::prompt(Mode::Normal);
        k.translate(ctrl('x'));
        assert_eq!(k.translate(ctrl('e')), KeyOutcome::OpenEditor);
        // `Ctrl-X` then a non-`Ctrl-E` cancels the chord; the key is handled
        // normally (here, typed in Insert).
        let mut k = VimKeymap::prompt(Mode::Insert);
        k.translate(ctrl('x'));
        assert_eq!(
            k.translate(plain('a')),
            KeyOutcome::edit(EditAction::Insert('a')),
        );
    }

    #[test]
    fn programmatic_visual_enter_and_leave() {
        // The mouse drives Visual through these (drag enters, click leaves).
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(k.begin_visual(), KeyOutcome::edit(EditAction::SetAnchor));
        assert_eq!(k.mode(), Mode::Visual);
        assert_eq!(k.leave_visual(), KeyOutcome::edit(EditAction::ClearAnchor));
        assert_eq!(k.mode(), Mode::Normal);
    }

    #[test]
    fn colon_opens_the_command_line() {
        let mut k = VimKeymap::prompt(Mode::Normal);
        assert_eq!(k.translate(plain(':')), KeyOutcome::EnterCommand);
        // It's a Normal command — `:` typed in Insert is just a char.
        let mut k = prompt_insert();
        assert_eq!(
            k.translate(plain(':')),
            KeyOutcome::edit(EditAction::Insert(':')),
        );
    }

    #[test]
    fn release_events_ignored() {
        let mut k = prompt_insert();
        let mut release = press(KeyCode::Char('a'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(k.translate(release), KeyOutcome::Ignored);
    }

    // --- property tests (criterion 13: count idempotence + reachability) ----
    mod props {
        use super::*;
        use proptest::prelude::*;

        /// An arbitrary keystroke drawn from the grammar's alphabet, across the
        /// three modifier classes the keymap distinguishes.
        fn arb_key() -> impl Strategy<Value = KeyEvent> {
            let codes = prop::sample::select(vec![
                'h', 'j', 'k', 'l', 'w', 'b', 'e', '0', '$', 'G', 'g', 'i', 'a', 'A', 'I', 'o',
                'O', 'x', 'X', 'v', 'V', 'd', 'c', 'y', 'Y', 'p', 'P', 'D', 'C', 'u', 'r', 's',
                '"', '(', ')', '{', '[', '<', '>', '=', '+', '-', '1', '2', '3', 'H', 'J', 'K',
                'L', 'R', '~', 'U',
            ])
            .prop_map(KeyCode::Char);
            let specials = prop_oneof![
                Just(KeyCode::Esc),
                Just(KeyCode::Enter),
                Just(KeyCode::Backspace),
            ];
            let code = prop_oneof![9 => codes, 1 => specials];
            let mods = prop_oneof![
                Just(KeyModifiers::NONE),
                Just(KeyModifiers::SHIFT),
                Just(KeyModifiers::CONTROL),
            ];
            (code, mods).prop_map(|(code, m)| KeyEvent::new(code, m))
        }

        /// **The viewer's Visual is the editor's Visual minus the writers** —
        /// a filter, not a second, smaller keymap.
        ///
        /// It was a hand-list of five keys with everything else falling into
        /// `step_motion`, which answers `Ignored` for a non-motion. So in the
        /// transcript `v` then `o` could not flip the selection's end and
        /// `vi"` selected nothing — both read-only atoms the table already
        /// marks `in_viewer()`, both working one pane away in the prompt.
        /// `Atom::in_viewer` is named in AGENTS.md as the predicate with two
        /// callers; it had one (Reviewer A, round 3).
        ///
        /// Enumerated **from the table**, so a key added to `Context::Visual`
        /// is covered on the day it is added rather than when someone
        /// remembers to extend a list here.
        #[test]
        fn the_viewers_visual_is_the_editors_visual_minus_writers() {
            let chords = DEFAULTS.chords_in(Context::Visual);
            assert!(
                chords.len() >= 8,
                "premise: the Visual table is populated ({} chords)",
                chords.len()
            );
            let mut checked_both_ways = (false, false);
            for chord in chords {
                let key = press(
                    chord.code,
                    if chord.ctrl {
                        KeyModifiers::CONTROL
                    } else {
                        KeyModifiers::NONE
                    },
                );
                let mut viewer = VimKeymap::chat();
                let mut editor = VimKeymap::prompt(Mode::Normal);
                let v = press(KeyCode::Char('v'), KeyModifiers::NONE);
                viewer.translate(v);
                editor.translate(v);
                assert_eq!(viewer.mode(), Mode::Visual, "premise: `v` enters Visual");
                assert_eq!(editor.mode(), Mode::Visual, "premise: `v` enters Visual");

                let atom = DEFAULTS.get(Context::Visual, chord).expect("just listed");
                let editor_said = format!("{:?}", editor.translate(key));
                let viewer_said = format!("{:?}", viewer.translate(key));
                if atom.in_viewer() {
                    checked_both_ways.0 = true;
                    assert_eq!(
                        viewer_said, editor_said,
                        "{chord:?} ({atom:?}) is read-only, so the viewer must \
                         do what the editor does"
                    );
                } else {
                    checked_both_ways.1 = true;
                    // **The atom's own reason**, not a constant repeated here:
                    // a new refusal kind is covered the day it is added, and a
                    // reason that drifts from the verdict fails.
                    let expected = format!(
                        "{:?}",
                        KeyOutcome::Refused(
                            atom.viewer_fit()
                                .refusal()
                                .expect("not in_viewer ⇒ has a reason")
                        )
                    );
                    assert_eq!(
                        viewer_said, expected,
                        "{chord:?} ({atom:?}) writes, so a read-only pane must \
                         refuse it *and say why*"
                    );
                }
            }
            assert_eq!(
                checked_both_ways,
                (true, true),
                "premise: the table has both read-only and writing atoms, or \
                 one half of this test never ran"
            );
        }

        /// **What Normal keeps live in Visual, a viewer's Visual keeps too** —
        /// `:`, `/`, the scrolls (`Atom::works_in_visual`). They come from the
        /// Normal table by the editor's fallback, which the viewer's Visual
        /// lacked: `:` and `/` did nothing, silently, in every viewer's
        /// selection (review stop 5, E2 #5). Enumerated from the Normal table,
        /// as its sibling above is from the Visual one.
        #[test]
        fn what_survives_visual_survives_a_viewers_visual() {
            let mut checked = 0;
            for chord in DEFAULTS.chords_in(Context::Normal) {
                let atom = DEFAULTS.get(Context::Normal, chord).expect("just listed");
                if !atom.works_in_visual() || DEFAULTS.get(Context::Visual, chord).is_some() {
                    continue;
                }
                let mods = match chord.ctrl {
                    true => KeyModifiers::CONTROL,
                    false => KeyModifiers::NONE,
                };
                let key = press(chord.code, mods);
                let mut viewer = VimKeymap::chat();
                let mut editor = VimKeymap::prompt(Mode::Normal);
                let v = press(KeyCode::Char('v'), KeyModifiers::NONE);
                viewer.translate(v);
                editor.translate(v);
                let (mut editor_said, mut viewer_said) =
                    (editor.translate(key), viewer.translate(key));
                // One that waits for a name (`m`): the name is what acts, in
                // both — the `a` of `ma` armed a text object (review stop 14,
                // A 8).
                if matches!(atom, Atom::SetMark) {
                    let a = press(KeyCode::Char('a'), KeyModifiers::NONE);
                    (editor_said, viewer_said) = (editor.translate(a), viewer.translate(a));
                    assert_eq!(editor_said, KeyOutcome::SetMark('a'), "{chord:?}");
                }
                assert_ne!(editor_said, KeyOutcome::Ignored, "premise: {chord:?} acts");
                assert_eq!(viewer_said, editor_said, "{chord:?} ({atom:?})");
                checked += 1;
            }
            assert!(checked >= 2, "premise: `:` and `/` at least ({checked})");
        }

        fn esc() -> KeyEvent {
            press(KeyCode::Esc, KeyModifiers::NONE)
        }

        proptest! {
            /// Mode-transition reachability + no-panic: any key sequence runs
            /// without panicking, and pressing `Esc` a bounded number of times
            /// always reaches Normal — no input can *wedge* a mode. Two Escs
            /// suffice for every reachable state: the deepest is Visual with a
            /// pending motion-prefix (`vg`), where the first `Esc` cancels the
            /// prefix and the second leaves Visual. This **matches real vim**
            /// (`vg<Esc><Esc>`, confirmed with the user) — a pending prefix
            /// swallows the first `Esc` to cancel itself; it is not a wedge.
            #[test]
            fn esc_always_reaches_normal_no_wedge(
                keys in prop::collection::vec(arb_key(), 0..100),
            ) {
                let mut k = VimKeymap::prompt(Mode::Normal);
                for key in keys {
                    let _ = k.translate(key);
                }
                k.translate(esc());
                if k.mode() != Mode::Normal {
                    k.translate(esc());
                }
                prop_assert_eq!(k.mode(), Mode::Normal, "two Escs failed to reach Normal");
            }

            /// The chat viewer never enters a **typing** mode — Insert or
            /// Replace. It's read-only, so no input can put it in an editing
            /// mode (Normal and Visual, for selection→yank, are the only
            /// reachable modes).
            #[test]
            fn viewer_never_enters_a_typing_mode(keys in prop::collection::vec(arb_key(), 0..100)) {
                let mut k = VimKeymap::chat();
                for key in keys {
                    let _ = k.translate(key);
                    prop_assert!(!k.mode().is_typing(), "{:?}", k.mode());
                }
            }

            /// Count idempotence: a typed count prefix on a repeating motion
            /// (`{n}l`) resolves to exactly that repeat, clamped at `MAX_COUNT`.
            /// Exercises digit accumulation + the per-motion count fold + the
            /// saturating clamp in one shot.
            #[test]
            fn count_prefix_repeats_motion(n in 1usize..=250_000) {
                let mut k = VimKeymap::prompt(Mode::Normal);
                for d in n.to_string().chars() {
                    k.translate(press(KeyCode::Char(d), KeyModifiers::NONE));
                }
                let out = k.translate(press(KeyCode::Char('l'), KeyModifiers::NONE));
                prop_assert_eq!(
                    out,
                    KeyOutcome::Edit {
                        action: EditAction::Move(CursorMotion::Right),
                        count: n.min(MAX_COUNT),
                    },
                );
            }
        }
    }
}
