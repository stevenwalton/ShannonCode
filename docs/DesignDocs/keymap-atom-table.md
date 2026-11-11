# Keymap atom-table substrate — design draft

Status: **BUILT** — this design shipped as `bindings.rs` (the atom table) plus
`vim_keymap.rs` (the engine over it), and `[keymap.<context>]` remapping is
live. Read it as the rationale behind that code, not as pending work. Written
2026-06-21 and marked "draft for review" until 2026-07-27; the paragraph below
describes the pre-build state and is kept because the problem statement is
still the clearest explanation of *why* the table exists.

> The config-driven remappable keymap is the M2 thesis ("customization as
> substrate") and the one load-bearing piece of it still unbuilt: the editing
> grammar is done and deep-reviewed, but *which key is which action* is still
> hardcoded `match` arms in five places (`resolve_motion`, `root_command`,
> `visual_command`, `resolve_text_object`, `shortcuts::classify`). This draft
> designs the data model that makes those leaves remappable, and pressure-tests
> it against the hard cases — `2d3w`, `dgg`, `ci"`, **`5<C-w>+`**, **`<C-w>L`**,
> `gt`/`gT` — *on paper* before any code moves.

**One known gap against this design** (docketed in `TODO.md`): the `:graph`
viewer's keys are still hardcoded `KeyCode` matches in `app.rs`, outside the
table — the one surface that has not yet been brought under it.

It is deliberately additive: it **keeps the reviewed engine** (the `Pending`
state machine + operator-pending composition) and only turns its *leaves*
into data. No rewrite.

---

## 0. Principles (inherited, not re-litigated)

- **Vim-fidelity is the bar.** vim is the *reference model*, not loose
  inspiration. We won't build all of vim, but every subset we ship must feel
  native to a true vim user — the complete, faithful version with the subtle
  behaviors right, not the token "vim-like" most TUIs settle for. Default
  bindings are **canonical vim**; the table makes them *remappable*, it does
  not make the defaults a compromise. Verify exact behavior against real vim
  before encoding (the recurring dropped-count class is what "close enough"
  guessing costs). See the vim-fidelity memory.
- **Engine is code; atoms are data.** vim's grammar is `[count][op][count]
  [motion|textobj]` — recursive, not flat; a lookup table can't model
  operator-pending. So the *composition* stays code. Only the *atoms* —
  "key `w` means word-forward", "key `d` means the delete operator" — are a
  config-driven table. (design.md §, Milestone 2 "Full vim grammar".)
- **Sane defaults you can mold.** Defaults live in code; the user's
  `config.toml` is a *sparse diff* against them. Missing file ≠ error; the
  user's file is never written. (Mirrors the existing `#[serde(default)]`
  config doctrine.)
- **Narrow keymap output; the app executes topology.** The keymap emits a
  typed *intent* (`KeyOutcome`); it never itself switches focus or resizes a
  pane. This is why a future emacs/graph-pane keymap doesn't re-implement
  topology — it binds the *same atoms* to different keys.
- **Un-shadowable globals.** `Ctrl-C` (quit) / `Ctrl-L` (redraw) resolve
  *before* the table and cannot be rebound.
- **Bind to atoms, not key-sequences.** A binding maps a chord to a *named
  action*, never to "the keys `dd`". This sidesteps vim's `map` vs `noremap`
  recursion footgun entirely — there is nothing to recursively expand.

---

## 1. Scope for this milestone (refined with the user, 2026-06-21)

The M2 doc currently lists "Tabs / resizable splits … not [in M2]". We are
**refining** that, not overturning it:

- **IN (new, this milestone):** window **focus / move / resize / equalize**
  over the *existing two panes* — `<C-w>h/j/k/l`, `<C-w>H/J/K/L`,
  `{count}<C-w>+/-`, `<C-w></>`, `<C-w>=`, `<C-w>_`/`|`. "We effectively
  already have splits" — the prompt/chat layout *is* a split; these
  commands act on it and are useful now.
- **OUT (still deferred), but the data model must accommodate:** arbitrary
  *new* splits (`<C-w>v`/`s` that spawn panes), **tabs** (`gt`/`gT`,
  `{count}gt`), the command-line grammar (`:`), leader keys, which-key
  popups. These get *seams and parsing*, not execution. The reason
  spawning panes/tabs waits: they're motivated by the features that need
  *multiple views* — **talking to several Claudes at once, the git-like
  conversation graph, observing subagent conversations** — and are best
  designed alongside those, not speculatively now.

> **M2 doc amendment needed:** one line under "What is NOT in Milestone 2"
> to move pane focus/move/resize *in* and keep new-splits/tabs *out*. Flagged
> so the scope change is explicit, not silent.

---

## 2. The three resolution layers (where a key goes)

A key resolves through a fixed priority chain:

1. **`classify_global(key) -> Option<Global>`** — only `Ctrl-C` and
   `Ctrl-L`. Checked first; not in the table; not rebindable. *Everything
   else currently in `classify` (focus toggle, page scroll) becomes an atom.*
2. **The focused pane's keymap engine** — the counted chord resolver over
   the atom table. It resolves editing keys **and** chrome-prefixed
   sequences (`<C-w>…`, `g…`), emitting a `KeyOutcome`.
3. *(gone)* — the separate stateless chrome pre-filter (`classify`'s
   focus/scroll half) **dissolves** into (1) + the table.

### Why chrome merges into the engine — the `5<C-w>+` argument

`5<C-w>+` (grow the focused pane by 5 rows) proves the count **straddles**
the `<C-w>` prefix: the `5` is typed *before* `<C-w>`, but the `+` *after*
consumes it. A stateless `classify` that only claims `<C-w>` would strand
the `5` (the pane engine accumulated it). The count and the `<C-w>` must be
seen by the **same accumulator**. The pane engine already *is* that
accumulator (`Pending::Root { count }`). So `<C-w>` becomes a **prefix
inside the engine**, carrying the count into a window sub-context, and the
resolved `window-resize-grow` atom is emitted as a `KeyOutcome::Window(…,
count)` the **app** executes. The keymap still "doesn't know panes exist"
— it emits an intent, exactly like the existing `KeyOutcome::Scroll`/
`Submit`. `classify` shrinks to the two un-shadowable globals.

This also **dissolves the `Ctrl-W`-is-mode-aware special case**: `Ctrl-W`
is simply bound to `prefix:window` in the Normal table and to `delete-word`
in the Insert table — two ordinary rows, no bespoke mode check in
`classify`.

---

## 3. The atom — the stable vocabulary

`Atom` is the internal resolved form; config strings parse into it. It is a
thin tagging over the *existing* enums plus the new chrome intents:

```rust
enum Atom {
    // --- pane-editor atoms: the engine composes these ---
    Motion(CursorMotion),            // w/b/e/0/$/j/k/gg/G…  (role-agnostic)
    Operator(Operator),              // d/c/y — opens operator-pending
    TextObject { scope, object },    // iw/a"/i( — legal only in op-pending/visual
    Enter(InsertEntry),              // i/a/A/I/o/O
    Mode(ModeChange),                // ->Normal / ->Visual{,Line,Block}
    Edit(EditAction),                // paste/undo/redo/x/X… (counted)
    Scroll(ScrollCmd),
    Submit,

    // --- prefixes: open a sub-context; resolve to a motion OR a command ---
    Prefix(PrefixId),                // G, Window, (later) CtrlX, Leader

    // --- app-topology intents: emitted up to the app, not executed here ---
    Window(WindowCmd),               // focus/move/resize/equalize/max/(split/close later)
    Tab(TabCmd),                     // next/prev/goto/new/close   (parsed now, no-op until tabs)

    Nop,                             // explicit unbind ("nop" in config)
}
```

Config names them with **stable strings** — `"word-forward"`, `"delete"`,
`"inner-word"`, `"paste-after"`, `"prefix:window"`, `"window-resize-grow"`,
`"tab-next"`, `"submit"`, `"nop"`. A `FromStr`/`Display` pair gives
parse + `:keymap dump` + did-you-mean.

**The role-agnostic-motion property is preserved:** one `Motion(WordForward)`
atom is a cursor move at root *and* an operator target in op-pending — the
engine contextualizes, the table never duplicates it. (This is today's
"`resolve_motion` shared by cursor and operator so they can't drift",
unchanged.)

---

## 4. Contexts & the table shape

A binding lives in a **context** — the engine's state when the key arrives:

| Context | Entered by | Legal atom kinds |
|---|---|---|
| `Normal` | default | all |
| `Insert` | `i`/`a`/… | Edit, Mode(->Normal), Prefix(CtrlX) |
| `Visual` | `v`/`V`/`Ctrl-V` (geometry is mode state) | Motion, Operator→Selection, TextObject, Mode |
| `OperatorPending` | `d`/`c`/`y` | Motion, TextObject-opener, doubled-op, count |
| `Prefix(G)` | `g` | Motion (`gg`) **or** Command (`gt`/`gT`) |
| `Prefix(Window)` | `<C-w>` | Window(*), count |
| *(later)* `Prefix(CtrlX)` | `Ctrl-X` | Edit(launch-editor) |
| *(later)* `CommandLine` | `:` | its own mini-grammar |

```rust
struct Bindings {
    // few, fixed contexts → a per-context chord map.
    by_ctx: EnumMap<Context, HashMap<Chord, Atom>>,
}
struct Chord { code: KeyCode, mods: Mods }   // canonicalized (see §6)
```

The **engine owns context transitions** (the reviewed `Pending` machine);
the **table owns key→atom within a context**. Only the leaf lookups change
from `match` arms to `bindings.get(ctx, chord)`.

---

## 5. Engine changes (small, additive)

Keep `Pending`. Generalize exactly two things:

**(a) `MotionPrefix::G` → a prefix that resolves to motion *or* command.**
```rust
enum PrefixResolution { Motion(CursorMotion), Command(Atom), None }
```
`g` then `g` → `Motion(BufferStart)` (composes: `dgg` still works); `g` then
`t` → `Command(Tab(Next))` (terminal). The legality rule is the engine's,
not the data's: **in `OperatorPending`, only a `Motion` resolution from a
prefix is valid** — so `dgt` is rejected for free (a `Command` atom in
op-pending = invalid chord = `Ignored`), while `dgg` composes. The data only
says key→atom; the engine says which atom *kinds* a context accepts. This is
what keeps the table honest without the table modeling the grammar.

**(b) `<C-w>` as `Prefix(Window)` with a straddling count.** From `Normal`,
`<C-w>` enters `Prefix(Window)` carrying the accumulated count. In that
context: `h/j/k/l`→`Window(Focus{Left..})`, `H/J/K/L`→`Window(Move{..})`,
`+`/`-`→`Window(Resize{Grow,Shrink})` *[count = rows]*, `<`/`>`→
`Window(Resize{Narrower,Wider})` *[count = cols]*, `=`→`Window(Equalize)`,
`_`/`|`→`Window(Maximize{H,V})`, `w`/`W`/`<C-w>`→`Window(Cycle{Fwd,Back})`
(so `<C-w><C-w>` toggles on two panes), `p`→`Window(Prev)`,
`v`/`s`→`Window(Split{V,H})` *(parsed; exec deferred)*,
`c`/`q`→`Window(Close)` *(deferred)*. **`H/J/K/L` are vim's *relocate*** —
on two panes they reorder the stack and can flip the split orientation
(stacked ↔ side-by-side); pin the exact reorientation against real vim in
Phase B rather than approximating (§0). The **load-bearing** requirement is
that the count typed *before* `<C-w>` reaches the command (`5<C-w>+` → +5);
whether we *also* accept a count *after* the prefix (`<C-w>5+`) is an
optional vim-parity nicety — **verify against real vim before committing to
it**, don't assume (dropped-count discipline). The context-carried count
makes the before-case work either way.

**Count semantics per atom — the dropped-count discipline at the substrate.**
A single fold `apply_count(atom, count) -> (Atom, usize)` decides, *in one
place*, how each atom consumes the count: **repeat** (`5j`), **absolute**
(`{count}G`→line, `{count}gt`→tab N), **magnitude** (`5<C-w>+`→+5 rows),
or **ignore**. No atom silently drops its count. (Generalizes the existing
`apply_count_to_motion`; this is the `3ix`/`3G`/`3D` bug class designed out
at the root — see the dropped-count memory.)

---

## 6. Config / TOML schema

```toml
[keymap.normal]
"w"       = "word-forward"
"d"       = "delete"          # operator → operator-pending
"g"       = "prefix:g"
"<C-w>"   = "prefix:window"
"<Enter>" = "submit"
"x"       = "delete-char"

[keymap.window]               # the <C-w> sub-context
"L" = "window-move-right"
"+" = "window-resize-grow"
"=" = "window-equalize"
"w" = "window-cycle"          # <C-w>w / <C-w><C-w> = focus toggle on two panes

[keymap.insert]
"<C-w>" = "delete-word"       # no clash with Normal's window-prefix: different context
```

- **Chord syntax:** `"<C-w>"`, `"<S-l>"`≡`"L"`, `"<Esc>"`, `"<Enter>"`,
  `"<Space>"`, `"g"`. A small parser ↔ crossterm `KeyCode`/modifiers,
  **canonicalized** (shifted letter ≡ uppercase; one spelling per chord).
- **Atom syntax:** the stable name; `prefix:<id>` for prefixes; `"nop"` to
  unbind a default.
- **Merge:** `DEFAULT_BINDINGS` (code) is the base; each `[keymap.<ctx>]`
  table is a **sparse diff** — present chords override, absent keep the
  default. (Same doctrine as `#[serde(default)]` config.)
- **Validation (mirrors the config doctrine):**
  - unknown **atom name** → *warn* + did-you-mean, don't error (forward-compat);
  - malformed **chord** → error naming the offending string;
  - atom **illegal in that context** (e.g. `delete` under `[keymap.window]`)
    → warn-and-skip;
  - rebinding `Ctrl-C`/`Ctrl-L` → warn + ignore (un-shadowable).
- **Where it lives:** `shannon-core::config` stores the **raw string table**
  (`Map<String, Map<String,String>>`) as plain serde data — core never
  learns what a motion is (it stays ratatui/atom-free, per the existing
  "TuiConfig is plain data; tui converts at the edge" rule). `shannon-tui`
  owns the `Atom` enum and does the parse/validate/merge into the runtime
  `Bindings` at startup.
- **`:keymap dump`** prints the *effective* table as commented TOML (never
  to disk); a **completeness guard test** asserts every atom and every
  default binding round-trips (same pattern as `:config dump`).

---

## 7. `KeyOutcome` / app dispatch deltas

Add to `KeyOutcome`:
```rust
Window(WindowCmd, usize),     // count = rows/cols/repeat per the atom
Tab(TabCmd, usize),
Pending(PendingHint),         // surface the in-progress chord for the indicator
```
- The app's outcome handler grows arms: `Window → window_mgr.apply(cmd,n)`,
  `Tab → tabs.apply(…)`, `Pending → render the hint`.
- **`Pending` is wanted independently:** the M2 "indicator shows the pending
  prefix" criterion is *not wired today* (the keymap holds `Pending` state
  but never surfaces it). Surfacing it here also gives the which-key seam for
  free.
- **A tiny window-manager model** appears in the app: for two fixed panes a
  layout value `{ panes: [PaneId; 2], order, orientation: Stacked |
  SideBySide, focus, ratio }`. `h/j/k/l` move focus; `H/J/K/L`
  reorder/reorient; `+/-`/`<`/`>` nudge `ratio`; `=` resets it to even;
  `_`/`|` maximize; the layout reads it. This *formalizes* the "we already
  have splits" 2-pane layout into the smallest structure the window atoms can
  act on faithfully — **not** a general split tree (that waits for `<C-w>v`
  spawning panes, which lands alongside multi-Claude / the graph / subagent
  views — §1).

### Note: bare `<C-w>` stops being focus-toggle

Making `<C-w>` a prefix means bare `<C-w>` now *waits*. vim's idiom restores
the quick toggle: **`<C-w><C-w>`** (= "cycle window", which on two panes is a
toggle) and the directional `<C-w>h`/`<C-w>l`. The existing
`focus_starts_on_prompt_and_toggles` test updates to drive `<C-w><C-w>`.
(If we want a single-key toggle too, it's just another default binding — but
defaulting to the vim idiom keeps muscle memory.)

---

## 8. Migration plan (each phase behavior-preserving + green)

- **Phase A — table, no behavior change.** Introduce `Atom`, `Bindings`,
  the chord/atom string parsers, and `DEFAULT_BINDINGS` that reproduce
  *today's* hardcoded bindings exactly. Swap the five match-function leaves
  for `bindings.get(ctx, chord)`. The `Pending` machine and composition are
  untouched; the whole existing test suite stays green (the proof it's
  behavior-preserving).
- **Phase B — window commands.** Generalize `MotionPrefix`→`Prefix
  {Motion|Command}`; add `Prefix(Window)` + `WindowCmd` atoms +
  `KeyOutcome::Window` + the app's 2-pane window-manager + dispatch.
  `classify`→`classify_global`. Ships `<C-w>` focus/move/resize/equalize.
- **Phase C — config wiring.** Raw string table in core; tui
  merge/validate/`:keymap dump`; the "override is honored" acceptance test
  (`"j" = "word-forward"` ⇒ `j` does `w`, nothing else changes). Add
  `gt`/`gT` defaults (Tab atoms) as the **command-prefix proof** (parses,
  dispatches, no-op until tabs).
- **Phase D — later/M3.** Tabs feature + real split tree; command-line
  context; leader; which-key popup (consumes the `Pending` hint).

---

## 9. Acceptance demos (what "done" looks like)

- `5<C-w>+` grows the focused pane 5 rows; `<C-w>L` relocates it; `<C-w>=`
  equalizes; `<C-w><C-w>` toggles focus. *(new, in-milestone)*
- `[keymap.normal] "j" = "word-forward"` makes `j` do `w`; every other key
  stays default. *(criterion 3/4: remap honored)*
- `gt`/`gT` parse to `Tab` atoms and dispatch (no-op until tabs) — the
  command-prefix path is wired and tested. *(accommodation)*
- `2d3w`, `dgg`, `ci"`, `3o`, `{count}G` behave **identically** to today
  (engine untouched — Phase A regression proof).
- `:keymap dump` round-trips to defaults; completeness guard green.

---

## 10. "Is there more?" — fold-in-now vs. seam-only

**Fold in now (cheap, earns its place this milestone):**
- **Submit as an atom** (criterion 11) — already `KeyOutcome::Submit`; the
  open "submit default binding" question simply becomes a default table row.
- **The pending-chord hint → indicator** (`KeyOutcome::Pending`) — wire it;
  it closes a standing M2 criterion *and* is the which-key seam.
- **Insert-mode atoms** — `Ctrl-W` (word-del), `Ctrl-U` (kill-to-start),
  and the `Ctrl-X Ctrl-E` `$EDITOR` chord as `Prefix(CtrlX)`. Doing Insert
  as a context is what *dissolves* the `Ctrl-W` mode-aware special case.

**Seam only (don't build — earned-use):** leader keys, which-key popup,
command-line grammar, user macros / `map`-to-sequence, a real split tree,
named registers. The prefix machinery + atom namespace already admit them;
adding one is "a new context + atoms", not a redesign.

**A latent trap this design removes:** today `MotionPrefix` conflates
"prefix" with "motion prefix". Generalizing to `Prefix → {Motion|Command}`
*now* (even though only `g`/`gg` exists today) is what makes `gt` a table
row instead of a special case later — the single highest-leverage forward
move in this draft, and the reason to design the prefix type before the
tabs feature forces it.

---

## 11. Decisions (resolved 2026-06-21)

1. **`<C-w>` becomes a real prefix — full vim window grammar.** Bare
   `<C-w>` waits; the quick toggle is `<C-w><C-w>` *and* `<C-w>w` (both
   cycle). The whole canonical set is in scope: `<C-w>` + `h/j/k/l` (focus),
   `H/J/K/L` (move/relocate the pane), `w`/`W` (cycle fwd/back), `p`
   (previous), `+`/`-` `<`/`>` (resize), `=` (equalize), `_`/`|` (maximize).
   No dedicated single-key toggle — the vim idiom is the default. *(Be
   genuinely vim-faithful here, not "vim-like" — §0.)*
2. **Scope: pane focus/move/resize is IN M2; new panes/tabs are OUT.** The
   *reason* tabs/new-splits wait (user): they're better served alongside the
   features that motivate multiple views — **talking to multiple Claudes,
   the git-like conversation graph, observing subagent conversations**. M2
   acts on the *existing* two panes; spawning panes/tabs lands with those
   later features. M2 doc "What is NOT in Milestone 2" amended accordingly.
3. **Stay vim-like throughout** — defaults are canonical vim (§0); atom
   names are descriptive flat-kebab + `prefix:` (an internal config string,
   independent of the keys, which are pure vim).
4. **Window-manager: minimal now.** A two-pane layout value carrying
   `{ orientation, order, ratio }` (enough for `H/J/K/L` reorientation +
   resize); a general split *tree* waits for pane-spawning. No tree up front.
