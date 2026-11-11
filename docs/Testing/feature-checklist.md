# Shannon Code — feature & behavior checklist

A flat, runnable acceptance list for **manual end-to-end testing** (distinct
from `Milestones/Milestone 2.md`, which tracks *work*; this tracks *observable
behavior*, vim-faithfulness included). Run top-to-bottom in a real terminal
near a milestone close. Status keys:

- `[x]` implemented + has automated test coverage
- `[~]` implemented but a **known minor divergence** from vim (noted inline)
- `[ ]` **not yet implemented** (expected to be reached for; flagged so a
  tester knows it's a gap, not a bug)
- `[!]` **known issue / watch item** (don't sign off until re-tested)

Last synced to code: **2026-08-02**. Sections up to "Submit / panes /
rendering" are the M1/M2 editor surface (previously synced 2026-06-21). The
sections after it cover M3 (the graph, sessions, children, lifecycle), the
2026-07-26/27 pane-and-window rewrite, and M4 (tool use, the permission gate,
secrets) — all of which shipped after that date and were missing from this
list, which meant a test drive run from it would have exercised roughly the
editor and nothing else.

**These new sections are `[ ]` on purpose.** Their automated tests are green,
but *green tests are not a test drive* — nothing below the editor line has been
driven by hand end to end. Tick them as you go; a `[!]` on anything that
surprises you is more valuable than a `[x]`.

---

## Modes & transitions
- [x] Normal ↔ Insert via `i` `a` `A` `I` `o` `O`, each repositioning the caret
- [x] `Esc` leaves Insert **and nudges the caret back onto the last char**
- [x] `v` enters/exits charwise Visual; `Esc` leaves dropping the selection
- [x] `start_mode` config (Normal/Insert) honored on launch
- [x] **remappable keymap**: `[keymap.<context>]` in `config.toml` overrides any binding (`"j" = "word-forward"`); contexts are `motion`/`goto`/`normal`/`visual`/`window`/`text-object`; an unknown action/context or bad chord warns and is skipped (the rest still apply); defaults stay canonical-vim
- [x] Status line always shows the focused pane's mode (no silent modes)

## Cursor (Normal) — caret faithfulness
- [x] `h j k l` + arrows; `w b e` (`e` crosses lines / blank lines to the next word-end); `0 $`; `gg G`
- [~] `w`/`b`/`e` + `iw`/`aw` use **whitespace-only** word boundaries (= vim's `W`/`B`/`E` class); punctuation-aware lowercase motions are a tracked fidelity item (divergence 6)
- [x] counts repeat motions (`3j`, `12l`); `{count}G`/`{count}gg` go to that absolute line (`3G`, `10G`); `d3G` deletes to it
- [x] count semantics are per-motion (one `apply_count_to_motion` fold — repeat vs. absolute line)
- [x] caret sits **on** a char: `$` lands on the last char, `l` can't pass it
- [x] `h`/`l`/arrows do **not** cross line boundaries (vim default whichwrap)
- [x] `j`/`k` clamp the column to the target line; no sticky desired-column (by design, M1)

## Insert editing
- [x] printable chars; `Enter` = newline; **`Ctrl+Enter` submits** (the rebindable direct-submit key); Shift-Enter = newline; Alt-Enter submits, in Insert and Replace (`:help submit`: the submit that gets through tmux, where Ctrl+Enter arrives as a plain Enter; also terminals that map Shift-Enter→Alt-Enter)
- [x] `Backspace`, `Delete`, `Ctrl-W` (word-back), `Ctrl-U` (kill-to-start)
- [x] arrows in Insert reach end-of-line (append) and don't cross lines
- [x] **counted insert-entry**: `3ix`→"xxx", `3a`/`3A`/`3I` repeat the typed text, `3o`/`3O` open N lines (even with nothing typed) — one undo group

## Operators `d` / `c` / `y`
- [x] charwise: `dw db de dh dl d0 d$` (+ `c…`/`y…`); `cw` acts like `ce` (keeps trailing space)
- [x] linewise doubled: `dd cc yy`; shorthands `D C Y` (= `d$ c$ yy`)
- [x] `{count}D`/`{count}C`: to EOL plus the rows below (`3D` deletes to EOL + 2 lines, vim) — count-faithful, not collapsed to `D`
- [x] linewise cross-row: `dj dk dG dgg` (+ `c…`/`y…`) — whole-row spans
- [x] counts multiply around the operator (`2d3w` = 6 words)
- [x] text objects: `iw aw`, `i" a' i\``, `i( a{ i[ i< ib aB`, nesting-aware brackets
- [x] `c` enters Insert; `d`/`y` stay Normal; `change + typing` = one undo
- [x] no-op operator (`di(` no pair, `dj` last line, `de` at EOL) makes no undo step / no redo wipe

## Yank / paste / register
- [x] `y`/`Y`/visual `y` fill the register; **delete cuts** too (`d`/`c`/`x`)
- [x] `ddp` swaps lines · `xp` transposes · `dwp` / `ci(`+`p` round-trip
- [x] `p`/`P` charwise (after / at cursor); cursor lands on last pasted char (vim `p`)
- [x] `p`/`P` linewise (line below / above); `yyp` duplicates the line
- [x] `dggp` / `dGp` / `d0p` match vim (verified by hand 2026-06-19)
- [x] `3p` repeats contiguously and undoes as **one** group
- [x] register survives submit (yank in one prompt → paste in the next)
- [x] linewise paste lands the caret on the **first non-blank** char of the pasted line (vim); column 0 only if the line is all-blank
- [~] **Visual-mode `p` (paste over the selection)** — no-op in Visual; the `vex`+`p` (cut-then-paste) two-step covers the workflow, so deferred (verified by hand 2026-06-19).

## Undo / redo
- [x] `u` / `Ctrl-R`; typing run coalesces; a motion breaks the run
- [x] a whole Insert session is one undo (vim): every insert-mode edit coalesces — typing, `<CR>`, `<BS>`/`Ctrl-W`/`Ctrl-U`/`Delete` — so `ifoo<BS>x<Esc>` / `ifoo<CR>bar<Esc>` each undo in one step
- [x] counted insert replays the **whole** typed text incl. newlines (`3ifoo<CR>bar<Esc>`)
- [x] `change + typing` = one undo; `delete + typing` = two undos
- [x] an operator (and a counted command) is a single undo group
- [x] redo stack dropped after a fresh edit
- [x] undo depth bounded (`UNDO_LIMIT`)
- [x] **property-tested**: arbitrary edit runs never break the buffer invariants (≥1 line, cursor in bounds) and `Undo`×k/`Redo`×k round-trips the buffer; arbitrary key runs never panic and `Esc` always reaches Normal (no wedge); a count prefix on a repeating motion resolves exactly (clamped at `MAX_COUNT`)

## Clipboard (OSC 52) & external paste
- [x] yank mirrors to the **system clipboard** (OSC 52); `d`/`x`/`c` do **not**
- [x] bracketed paste = one undo group, never key-by-key (no stray submits / pasted commands)
- [x] paste sanitization: CRLF/CR → LF; control chars incl. bare `ESC` stripped
- [x] **OSC 52 under tmux** — confirmed 2026-06-27 working direct (ghostty), over SSH, **and through tmux**. tmux swallows a plain OSC 52, so when `$TMUX` is set we wrap it in tmux's DCS passthrough (`clipboard.rs`); needs `allow-passthrough on`/`all` (tmux 3.3+).
- [ ] OSC 52 read-back (paste *from* OS clipboard via `p`) — intentionally not done; bracketed paste covers inbound

## Visual mode
- [x] `v` + motions extend the (charwise) selection; reverse-video highlight; multi-line
- [x] `d` `x` `c` `y` operate on the selection
- [x] linewise Visual `V` (whole-row selection + highlight; linewise d/c/y/x)
- [x] block Visual `Ctrl-V` (rectangular select + highlight; block `d`/`x`/`y`; blockwise `p`/`P` rectangle paste with short-row padding)
- [x] `v`↔`V`↔`Ctrl-V` switch mid-selection (keeps both endpoints)
- [x] block-insert `I`/`A` (type on first row → replicated to every row on `Esc`, one undo group)
- [x] block `$` (`Ctrl-V`…`$`): right edge tracks each row's own end — `A`/`d`/`y` act raggedly, no padding
- [x] counts + `gg` in Visual (shares Normal's motion accumulator — `3j`, `gg` all work)
- [x] block-`c` replicate typed text to every row on `Esc` (cut the rectangle, type on the first row, replicate to all — reuses the block-insert machinery; one undo group)
- [x] visual text objects: `viw`/`vi"`/`va(` … select the object as a charwise selection (reuses the operator-pending `Context::TextObject` resolvers)
- [x] visual `o` (swap selection ends) — swaps cursor ↔ anchor (rebindable `swap-selection-ends` atom); prompt pane

## Submit / panes / rendering
- [x] submit is a **rebindable atom** (`submit`): default `Enter` + `Ctrl+Enter` in Normal, `Ctrl+Enter` in Insert (direct-submit mid-compose); config `[keymap.normal]`/`[keymap.insert]` can remap it; empty/whitespace prompt ignored
- [x] window focus: `<C-w>w`/`<C-w><C-w>` cycle, `<C-w>h/j/k/l` directional (works from either pane); bare `<C-w>` is now a prefix, not a single-key toggle; `Ctrl-C` interrupts and never quits (see "Sessions run apart"); `Ctrl-L` forces redraw
- [x] window resize: `{count}<C-w>+`/`-` grow/shrink the focused pane's height (count straddles the prefix: `5<C-w>+` ≡ `<C-w>5+`), `<C-w>=` resets to the configured default; clamped to fit the frame
- [x] window relocate: `<C-w>K`/`J` reorder the vertical stack (prompt above/below chat); `<C-w>H`/`L` reorient to a side-by-side split (chat │ prompt)
- [x] window resize is axis-aware (vim): `<C-w>+`/`-` height (stacked only), `<C-w>>`/`<` width (side-by-side only); the off-axis command is a no-op; the prompt's span on each axis is remembered across reorientation
- [x] chat pane is a **read-only cursor surface** (the same `Buffer` framework as the prompt, editing gated): when focused it shows a cursor and *every* vim motion moves it (`h`/`j`/`k`/`l`, `w`/`b`/`e`, `0`/`$`, `gg`/`G`, `{count}G`, counts), the viewport following; when unfocused it renders the streaming transcript. `Ctrl-U`/`D`/`F`/`B` + PgUp/PgDn still scroll directly.
- [x] chat pane **yank** (its only operator — read-only): `yw`/`yy`/`yi"` (operator-pending) and `v`/`V`/`Ctrl-V`…`y` (Visual) copy transcript spans **to the prompt's register** (so `p` in the prompt pastes — vim's global register) **and** the system clipboard (OSC 52) — the M3 "select a memory/history span → copy" seam; mutating ops (`d`/`c`/`x`/`p`/Insert) are gated. (Minor: focusing reflows long lines hard-cell vs word-aware.)
- [x] soft-wrap with correct cursor placement; wide/CJK chars wrap by cells
- [x] status line shows per-session token usage (`tok ↑in ↓out`), summed across turns; hidden until a turn reports any (the stub emits synthetic usage so it shows offline)
- [x] `$EDITOR` escape hatch: `Ctrl-X Ctrl-E` (readline binding, mode-independent) opens the prompt in `$VISUAL`/`$EDITOR`/`vi` (`.md` tempfile); save-and-quit replaces the prompt (one undo group, no auto-submit), errors are non-fatal (terminal restored via the suspend guard even on editor crash)
- [x] `:` command line (status row): `:q` quits, `:Edit` opens the transcript read-only in `$EDITOR`, `:config dump` opens the commented default config; `:` is the rebindable `command-line` atom; Esc / backspace-past-`:` cancel
- [x] **pretty defaults**: markdown styling (headings, fenced + inline code) and **syntect** syntax highlighting for languaged code fences, computed once at commit and cached (per-row styles parallel to the wrapped rows); per-role bubbles (the user echo dimmed so the assistant reply stands out, and framed by two rules in `ui.border.user` with a blank line before the reply); zero re-wrap/re-style on a same-width redraw (cache-reuse probe) and O(viewport)-not-O(history) framing (guarded by the append-during-stream `criterion` bench)
- [~] configurable per-role bubble **padding/alignment** — role *distinction* is in (dim echo); the config knobs for padding/alignment are a small remaining refinement
- [x] mouse capture is `tui.mouse`-gated (default on); **click** focuses the pane under the pointer; **wheel** scrolls the output transcript; **Shift-drag** falls through to the terminal's native selection (the copy escape hatch)
- [x] mouse **click-to-position cursor + drag-select** (drag → Visual), both panes: the reverse screen→buffer map (`cursor_from_visual`) mirrors the soft-wrap (wide/CJK chars, scroll `top`, phantom EOL rows); a click positions the cursor (keeps Insert; drops a prior selection from Visual), a drag enters Visual + anchors at the press point and extends as you move, re-anchoring each fresh drag

---

## Panes & windows (the 2026-07-26/27 rewrite — NOT yet hand-driven)

The old model was a fixed two-pane pair with focus derived by negation. It is
now a tree of arbitrarily many panes in any order. Nearly every item here is
new surface.

- [ ] `:sp`/`:split`, `:vs`/`:vsplit` divide THE FOCUSED PANE (vim); `:sg`/`:vsg`
      open the graph in a split; `<C-w>s`/`v` are the key forms
- [ ] `:clo`/`:close` closes the focused pane; the LAST pane refuses (`:q` quits)
- [ ] `:on`/`:only` closes every other pane; with one pane open it says "this is
      the only pane" rather than "closed 0 panes"
- [ ] `<C-w>w`/`<C-w><C-w>` cycle forward, `<C-w>W` back — **with 3+ panes open**,
      not just two
- [ ] `<C-w>p` returns to the pane you came FROM, and pressing it again returns;
      with 3 panes this must differ from `<C-w>W`. Says so when there is no
      history yet
- [ ] `<C-w>h/j/k/l` reach the pane that is actually THERE: past this pane's edge
      on the axis of travel AND overlapping perpendicular. A diagonal pane is
      never reached in one press. **Try an L-shaped 3-pane layout** — this is
      where a centre-distance heuristic used to send `l` downward
- [ ] `{count}<C-w>+`/`-` height, `>`/`<` width; the off-axis one is a no-op
- [ ] `<C-w>=` restores declared sizes — **including after `<C-w>K`** (a
      relocated bar used to come back inverted)
- [ ] `<C-w>H/J/K/L` relocate; flipping stacked↔side-by-side and back restores
      the size you had on EACH axis
- [ ] a click focuses the pane under the pointer; the wheel scrolls the pane
      under the pointer
- [ ] every pane takes `:` (run `:sg` from the transcript); `<Enter>` deliberately
      does not submit from another pane
- [ ] a pane with no content still draws its border and title (try
      `:split analyze` before running `:analyze`) — never a black hole
- [ ] the status line names the focused pane and its mode, whatever else is open

## Windows, round two (2026-07-27 → 08-02 — from Steven's own dogfooding)

Every item here is a bug he hit or a rule that changed underneath him, so these
are the ones most worth re-driving.

- [ ] `:q` closes the focused pane and quits only on the LAST one (vim);
      `<C-w>q` is the key form; `:qa`/`:qall` quits outright; `:close` never
      quits
- [ ] a split ALWAYS divides the pane you are standing in — including the
      prompt. Compare `:sp` from the prompt against `:sp` from a transcript:
      they must behave identically (they did not: the prompt redirected)
- [ ] the new pane appears ABOVE (`:sp`) / LEFT (`:vs`) and takes the cursor;
      `[tui] splitbelow = true` / `splitright = true` flip each axis
- [ ] `:graph`, `:help`, `:analyze` open IN the focused pane, never elsewhere;
      `<Esc>`/`:q` hands the pane back to what it was showing
- [ ] `q` no longer closes a viewer (it is vim's record-macro key)
- [ ] every pane shows its NUMBER in the top-right; `3<C-w>w` goes to pane 3;
      a number that does not exist says so
- [ ] `<C-w>w` with NO count still cycles (it is not `1<C-w>w`)
- [ ] `<C-w>j`/`k` leave a viewer pane on the FIRST press, in the direction
      asked for (they used to need two, and moved in ring order)
- [ ] `:on` from a prompt-only or chat-only state is recoverable —
      `:sp output` / `:sp prompt` bring the other back
- [ ] TWO transcript panes are independent: each keeps its own cursor and
      selection while showing the same conversation
- [ ] `:sp prompt` gives a second usable prompt; typing in it works

## One pane, one session (buffer model P3 — 2026-08-02, NOT hand-driven)

The headline is two sessions on screen at once. Everything here is new in
`e77fbe2` + `f10d9e7`; nothing below has been driven by a human yet.

- [ ] every transcript pane's TITLE names the session it shows (short id), so
      two panes on two sessions are told apart without opening `:graph`
- [ ] a branch adds `— branch ← <turn>` after the id; a root session does not
- [ ] `:vs` then `<C-w>w` then `:open <hash>` → that pane shows the other
      session and **the sibling does not move**. This is the one to try first;
      it is the bug class ("it changed both panes") in its newest form
- [ ] the PROMPT follows the pane you opened in — type and the turn lands in
      the session you are looking at
- [ ] `:sp` on a pane showing session B gives a second window on B, not on the
      foreground; the two keep separate cursors and scroll
- [ ] `:open` from the PROMPT with one transcript pane just works; with several
      it refuses and says to focus one first (`<C-w>w`) — no silent guess
- [ ] `:open` while `:graph` has taken over the only transcript pane reclaims
      that pane rather than refusing
- [ ] `<CR>` on a session in `:graph` opens it in the pane the graph was in
- [ ] switch away from a session mid-stream: the turn KEEPS STREAMING into its
      own session (Steven, 10-20: "A prompt in flight should stay in flight"),
      and its chunks do not leak into the session you switched to
- [ ] the status line (lines, tail marker, token total) describes the session
      your prompt goes to (Steven, 10-16) — move the aim between two panes and
      watch it change

### Watching a child (P3b)

- [ ] `:child <briefing>`, then `:graph` → `<CR>` on the child (or `:open
      <child-hash>`) **while it runs** → the reply streams into that pane live
- [ ] the child's pane title says `· running`, and stops when it finishes
- [ ] typing into a session a child is mid-turn on QUEUES (the notice names the
      child) and sends when the child finishes
- [ ] a child running on ANOTHER session does not block your prompt at all
- [ ] `:child --stop <id>` on a child you are watching: the partial stays

### Deleting, with more than one session open

- [ ] `:delete <hash>` of a session another pane is showing → that pane falls
      back to a fresh empty session (it neither vanishes nor shows a ghost)
- [ ] `:clone`, leave the clone in one pane, `:open` the original in another,
      then `:delete` the original → REFUSED (it would orphan shared turns)

## Sessions run apart (`buffers.md` §9, 10-21 — NOT hand-driven)

Each session answers on its own (Steven, 10-20: "Sessions are independent").
Tested by `app/tests/parallel.rs`; none of it by a human yet.

- [ ] `:vfork`, ask a question in each pane: both answer AT ONCE, each into its
      own pane — the second is not queued behind the first
- [ ] a decision waiting in one session: a prompt for the other starts, and
      the other's own allowed calls run and come back
- [ ] `Ctrl-C` aimed at one stops ITS turn; the other streams on. Aimed at an
      idle session, `Ctrl-C` says "nothing here CTRL-C stops … running
      elsewhere: …" naming the busy one
- [ ] mark two panes, one prompt (a multi-send), one `Ctrl-C`: both replies
      stop
- [ ] the row: `answering…` for the aimed session only, `2 sessions running`
      while another runs (and `1 session running` when only the other does);
      `^C:stop` only while something runs where your prompt goes
- [ ] `S` in `:graph`, then aim elsewhere: `<Esc>` does not cancel it; aimed
      back, it does
- [ ] `:qa` names every session's work; `:qa!` stops all of it
- [ ] a refusal about another session's busy slot says "CTRL-C in its
      session"; `:child --stop <a send's id>` names the session to press it in

## Pages are buffers (`buffers.md` §10 V3, 10-21 — NOT hand-driven)

- [ ] `:help`, move, `:q`, then `:ls!` → the manual listed `u`, with its line;
      `:b N` → back where you were, that line mid-window
- [ ] two `:help` windows read one manual; each keeps its own place
- [ ] `:analyze <cmd>` in two panes, then `:analyze <other>` → both show the
      new report, from the top, titled by the new command
- [ ] `:messages` open while notices arrive → it takes them without losing
      your place; session numbers skip pages' (`b1`, `b3` with the manual `b2`)
- [ ] `:sg` and `<C-w>s` on `:graph` list Claude Code's conversations as
      `:graph` does; `d` in a `:sg` tree spends that tree's marks only (its
      own tree); in a `<C-w>s` copy, the tree's, so both windows'

## The tree as a buffer (`buffers.md` §10.4 V4 identity, 10-24 — NOT hand-driven)

- [ ] scroll a long `:graph` down, let a child job end → the window stays put;
      `m` then a background event then `f` → the row is marked
- [ ] `<C-w>s` on a tree moved off your session, a fold closed, a row
      marked → the copy is on the same row, the fold closed, the row marked;
      `mf` in the copy marks it in the other too (one tree, netrw's);
      `:sp` in a transcript scrolled back → the copy is at the same place,
      not the tail
- [ ] `:sg` from a tree → a tree of its own (`:ls!` lists two), no marks;
      `:q` both, `:graph` → the first taken back, its marks with it
- [ ] `:graph` at the list's start (most recent first): a background session
      takes a turn → it shows on the top row, the cursor a row lower;
      scrolled mid-list, a turn arriving just above the window's top shows
      on top; one landing two rows above stays hidden
- [ ] `:graph`, move, `:q`, `:ls!` → the tree listed `u`; `:b N` → back on
      that row, mid-window; `:graph` → on your session; `:b N` inside a tree
      keeps its marks

## One window history (`buffers.md` §10.4 V4b, 10-25 — NOT hand-driven)

- [ ] in a transcript scrolled back, `:b` another session, then `:b` back →
      where you were (cursor and scroll), not the end; a session the window
      never showed opens at its end
- [ ] `:graph` over a transcript, mark a row, `<CR>` on a session → it opens
      in that pane; the neighbouring panes unchanged; `<C-^>` → the tree as
      you left it (the row, the mark, its inspector); `<C-^>` → the session
- [ ] `<C-^>` in a fresh window → "no buffer to go back to in this window";
      `:b#` the same; from the prompt bar, `:b#` acts on the aimed window
      (smoke §15 drives the 0x1E byte)
- [ ] `:help` over a transcript, `:b N` → that session; `:q` there closes the
      pane (a transcript gives nothing back), history or none

## Every view a buffer (`buffers.md` §10.4 V5, 10-25 — NOT hand-driven)

- [ ] `:ls!` in one pane, `:vs`, `:ls` in the other → both list the sessions
      alone, both titled `ls`; `:ls!` again → both list the pages and trees,
      both titled `ls!` (one buffer, its `!` a setting — Steven's "the same
      list, showing more")
- [ ] `:alias`, `:q`, `:ls!` → `the aliases` listed `u`; `:b N` → it again;
      the session opened next skips its number
- [ ] `:ls`, `G`, `:ls` again → the cursor stays; `:sp` → the copy is on
      the same line
- [ ] drop a theme file in `themes/` beside your config while `:themes` is
      open → not listed; `:sp` → still not; `:themes` → listed, in both
- [ ] `:ls`, `<CR>` on another session's line → it, in this pane; `<C-^>` →
      `:ls`, on that line; `<CR>` on the heading → "no buffer on this line";
      in `:ls!`, `<CR>` on the manual's line → the manual
- [ ] `:rules` scrolled down onto a shipped rule, then `:allow` twice → the
      cursor and the window's top stay on their lines (by row, not by index)

## The graph viewer & history operations (M3 — NOT yet hand-driven)

- [ ] `:graph`/`:g` opens in a SPLIT beside the conversation, not over it;
      `q`/`<Esc>` close just that pane
- [ ] `j`/`k`/`gg`/`G` move, `<Tab>`/`<S-Tab>` step sessions, `l`/`h`/`<CR>`
      unfold/fold
- [ ] `<CR>` on a session opens it; on a turn checks it out (transcript shows
      history to that point; continuing forks a branch)
- [ ] the inspector tracks the selection: ids, origin, model, per-turn usage,
      stop reason, `distills:`, child depth/count, subtree usage
- [ ] `<Space>` marks; the MARK SHAPE picks the semantics for `s`/`S`/`p`
- [ ] `e` edits a message / turn / session's turns as Messages-API JSON;
      malformed JSON cancels and changes nothing
- [ ] `c` clone, `m` merge, `p` pick, `r` return
- [ ] `s` squash — all three mark shapes: none (branch→base), one contiguous run
      (in place), scattered (gathered into a new session)
- [ ] `S` LLM-squash streams off-transcript, `<Esc>` cancels mid-generation, the
      draft opens in `$EDITOR` for approval
- [ ] `d` deletes a session (y/N); REFUSES with the reason when another session
      shares its turns
- [ ] every operation composes a NEW session and leaves the originals reachable
- [ ] **the tree is a workbench**: `c`, `m`, `p`, `s`, `S`, `e`, `r`, `d` all
      leave `:graph` open and rebuilt with what you just made — only `<CR>`
      leaves. Chain several without reopening the viewer
- [ ] the same op run as a `:` command from a pane showing the MANUAL hands
      that pane back instead, since it cannot show you the result

## Sessions, children, lifecycle (M3 — NOT yet hand-driven)

- [ ] `:new` (stays put when already blank), `:clone`, `:open <hash>`,
      `:delete` (current / `all` / by id), ids `<Tab>`-complete
- [ ] **the prompt's target (P4)**: with two transcript panes, `<C-w>w` into
      one and type — it goes THERE. The border accent and the `prompt → <id>`
      title both say which before you press Enter
- [ ] stepping into a `:help`/`:graph` pane does NOT re-aim the prompt
- [ ] start a turn, `:open` another session mid-stream: the first keeps
      streaming into its own pane, and its tokens are billed to it
- [ ] a prompt queued behind a turn goes to the session it was TYPED for, even
      if you moved away while it waited
- [ ] `:graph` marks survive a background child streaming
- [ ] **from inside a viewer**: `:new` and `:open <hash>` typed in a `:graph`
      or `:help` pane land the session in THAT pane — including `:open` on a
      turn id, which checks out a point (the two id spaces must behave alike)
- [ ] `:new` while a second pane is parked on the old session leaves that pane
      alone — it keeps showing the session you left
- [ ] a branch's title shows `<id> — branch ← <turn>` (per pane — see "One
      pane, one session")
- [ ] sessions persist as you go; relaunch with `[tui] resume = "last"` returns
      to HEAD; files are `0600` in `0700` dirs
- [ ] `:child <prompt>` and `/child <prompt>` (typed in Insert) both spawn;
      no prompt opens `$EDITOR`
- [ ] `-m <model>` runs the child on another model; `--stop <id>` cancels one
      and KEEPS what it had produced, sealed and marked
- [ ] the status line counts running children; `:graph` shows `◆` and `▶ running`
- [ ] `[agents]` guardrails: depth and fan-out, with `on_limit` =
      confirm / block / advisory
- [ ] a turn's stop reason is recorded and shown; `:continue` resumes a
      `max_tokens`/`interrupted` turn and REFUSES a whole one, with the reason
- [ ] `[continue] strategy`: `instruct` (default, any model) and `prefill`
      (exact splice; 4.6+/5-family models reject it — expect the note)

## The permission gate & secrets (M4 — NOT yet hand-driven)

**No sandbox exists.** An allowed command runs as you, in your shell and cwd.
Test with commands you would be happy to run by hand.

- [ ] a proposed command stops at the DECISION pane and nothing runs until you
      answer; the pane never times out into a yes
- [ ] `d`/`n`/`q` all deny; `<Esc>` answers nothing — a reflexive key
      decides no call
- [ ] `a` allows once; `A` allows this exact command for the session
- [ ] `A` is NOT offered on a command whose text is unstable (a glob, a
      variable, a substitution) — those get `a` only
- [ ] a DENY verdict offers no allow key at all, and says why when you reach for
      one
- [ ] `e` edits the command and re-gates the result from scratch
- [ ] `j`/`k` scroll a long report; the action line stays visible
- [ ] closing the decision pane (`:close`, `:q`, `<C-w>c`, `<C-w>q`) DENIES
      rather than leaving the turn wedged
- [ ] `:hide` on the decision pane KEEPS the call: the pane goes, the row's
      right says `decision hidden · :decide N`, `:b` away and back brings no
      pane, `:decide` brings it back and you answer it there (10-22: "`:q`
      removes from the buffer while hide doesn't")
- [ ] **decisions per session** (`decision-panes-and-alerts.md` §3, 10-22):
      two sessions side by side, each with a call — two panes, each under its
      conversation; `n`/`N` walk only that session's calls
- [ ] `:only` in a conversation keeps its decision pane, and in the pane keeps
      its conversation; from the prompt the call WAITS, and the row's right
      says `decision hidden · :decide N`
- [ ] a session in no window (`:new` over it, `:Send bN`): its call waits,
      said on the row; `:decide N` shows it and stands you in its pane, the
      keys waiting a moment; `:tool` typed for an unseen session shows it first
- [ ] **a summary's review is a decision** (`:compact!`, `:distill!`, §4): the
      draft waits in a pane beside its session, the cursor where it was;
      `<CR>` opens it in `$EDITOR` (`:wq` composes, a quit without saving
      leaves it `closed unsaved`), `a` applies it as drafted, `d` drops it,
      `:hide` keeps it; `:q` and `q` refuse, `:q!` drops; a prompt for the
      session waits until it is answered; applied from a `:graph` over your
      transcript, the graph stays
- [ ] through Claude Code, `:ls` counts a decision's deadline down (`needs you ·
      58m left`, live while `:ls` is open); `:setup`'s Claude Code page says a
      call left 59 minutes is refused
- [ ] on a terminal too short to draw the pane, no key can answer it — and it
      says why instead of trapping input
- [ ] `:analyze <cmd>` previews without running and without spending; the report
      is identical to the live pane's
- [ ] `:tool <cmd>` routes through the gate INCLUDING running it if allowed
- [ ] colour: green read-only, yellow mutating-or-unknown, red destructive;
      an unresolved value flowing into a dangerous command underlines the
      carrier at both ends
- [ ] `:secret <text>` — it leaves as a handle; the SCREEN still shows it as
      itself, styled
- [ ] `:secret --no-spend <text>` additionally refuses commands that expand it
- [ ] `:secrets`/`:alias` lists handles and sources, NEVER values
- [ ] a credential pasted into the prompt is offered for marking; ordinary prose
      is not (a false offer teaches you to dismiss)
- [ ] `[exec]` timeout kills a pipeline by process GROUP (try `sleep 60 | cat`)

## Docs & help (NOT yet hand-driven)

- [ ] `:help` opens the manual; `:help <topic>` opens one chapter;
      `<Tab>` completes topic names and aliases (`subagent`, `security`)
- [ ] every `|shannon-…|` you follow exists (gated by a test as of 2026-07-27)
- [ ] `:config edit` opens your file, applies on save, and SAYS what happened —
      including "saved but does not parse, running config unchanged"
- [ ] the config file it creates is `0600`
- [ ] `:config dump` shows every field, commented

---

### Open divergences/gaps to resolve before M2 sign-off
1. `[~]` Visual-mode `p` (replace selection) — `vex`+`p` covers it; revisit if it grates.
2. `[x]` block-`c` replicate-on-Esc — **done**: cuts the rectangle and replicates the typed text to every row (reuses the block-insert path; one undo group).
3. `[x]` linewise paste caret → first non-blank — **done** (col 0 only if all-blank).
4. `[~]` block selection on ragged lines without `$` is bounded by each line's length (no virtual columns); `$` (end-of-line) is supported.
5. `[x]` OSC 52 under tmux/SSH — **resolved**: direct + SSH + tmux all confirmed (tmux via the DCS-passthrough wrap, `allow-passthrough on`).
6. `[ ]` punctuation-aware `w`/`b`/`e` + `iw`/`aw` — currently whitespace-only (= vim `W`/`B`/`E`); true lowercase motions also break on punctuation (`foo.bar` → `foo`,`.`,`bar`). Deferred to a dedicated fidelity pass (motions + operators + text objects together).
