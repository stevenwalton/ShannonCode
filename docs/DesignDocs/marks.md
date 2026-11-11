# Position marks — `m{a-z}`, `'`, `` ` ``, `''`

STATUS (2026-10-26): **SUPERSEDED by `navigation.md`** (round 2, after
Steven's account of the goal: blocks, motions, folds, marks — yours on `'`,
the agent's on `` ` `` — and the gutter). Kept for what round 2 relies on:
the vim measurements (§2), the names already taken (§3), and the measured
tail-only transcript (§4). Its §5–§8 are round 1's; where they differ,
`navigation.md` holds. Built 10-27: `m{a-z}` and `'{a-z}` in a transcript
(`navigation.md` §7), the place not §4's `(block, line, col)` with a shift
rule but a block's own id (`output::Anchor`), so the insert needs no rule
and a truncation none either.

## 1. The ask

- Steven, 10-18: folds are the minimum for the soft release, "marks during
  the week… But tabs first".
- Steven, 10-25, his use: two panes on one conversation — "mark something
  while streaming, split, and jump back to that mark". In vim a lowercase
  mark is the buffer's ("same conversation ≈ same file"), so every window
  on the session has it.

## 2. vim, measured

Neovim 0.12, headless, `-u NONE`, 10-26 (`startofline` set, vim's default):

| What | Result |
|---|---|
| `ma` at 5,4 in one window; `'a` in a second window on the same buffer | line 5, its first non-blank (col 3) — the mark is the buffer's |
| `` `a `` there | 5,4 — the exact place |
| `''` after a jump in window 2 | back where window 2 was before it |
| `''` in window 1, which never jumped | line 1 — the context mark is the WINDOW's |
| two lines inserted above the mark | the mark moves with its line (7,4) |
| the mark's line deleted | the mark is erased; a jump to it is E20 "Mark not set" |
| `'A` from a window on another buffer | that window shows A's buffer, at its line |
| `mb` in a `nomodifiable`, `readonly` buffer | set (2,1) — read-only text takes marks |
| `'a` in a buffer that has no `a` | E20 |
| `:marks a` | `mark line  col file/text` / ` a      5    3 line 5` (col 0-based) |

## 3. "Mark" is taken twice already

- **A marked pane** — `<C-w><Space>` toggles a pane into `:Send`'s set,
  `<C-w>u` clears it; titles wear the glyph (`bindings.rs`).
- **The tree's marks** — netrw's: `mf` marks a row, `mu` unmarks all, shared
  by every window on the tree (`buffers.md` V4b). The tree owns `m` as a
  prefix, so `m{a-z}` cannot mean a position there without taking netrw's
  keys from it (netrw does the same to vim's `m`).

Position marks would be the third. vim lives with the same overload (netrw's
marked files beside `m{a-z}`) because the keys differ. Proposal: the
manual says "a marked pane" for the first, "a marked row" for the second,
and "mark `a`" (always with its letter) for a position.

## 4. Where a position is

A transcript window's cursor is `(line, col)` over the transcript's LOGICAL
lines — its `prompt::Buffer` mirror of `Output`'s blocks; wrapping is a
cache on the content (`Output`), never the position. So a mark survives a
resize, and a fold (when folds come) hides its line without moving it.

**Measured, 10-26: a session's transcript changes only at its tail.**
`Output`'s mutators: `told` and `begin_turn` push; `append_shown` grows the
last block (its last line, then new lines); `push_tool_record` seals the
open block and pushes, or inserts the record just before an EMPTY open
block; `end_turn` pops an empty open block after a record, or adds a blank
line to the last; `abandon_turn` truncates the last turn's blocks. Nothing
rewrites an earlier block, and a checkout is a new session (a branch), not
the old one rewritten (`checkout_turn`). So a mark on any line but the
last block's is stable without adjustment, and the tail has three cases:

1. **Lines appended** — to the line marked, or after it: it stays. This is
   "mark while streaming".
2. **A record inserted before an empty open block** — a mark on that
   block's one blank line would now name the record's first line. The mark
   is kept as `(block, line in block, col)`, not a flat line, so the insert
   moves it with its block: one rule, at the one insert.
3. **Blocks truncated or popped** (`abandon_turn`, `end_turn`) — marks in
   them are erased, as vim erases a mark whose line is deleted.

A jump to an erased or unset mark is refused, said ("mark `a` is not set"),
as E20 is.

## 5. Where each mark lives (vim's split)

| Mark | Whose | Home |
|---|---|---|
| `a`–`z` | the buffer's: every window on the session, every tab | the session buffer (`app/buffers.rs` `SessionBuffer`), beside its `output` |
| `A`–`Z` | global: a session and a place in it | `App` |
| `''`, `` `` `` (the context mark) | the window's | the window's cursor buffer — per window already (`SessionView.chat`, a `prompt::Buffer`; a draft's `prompt::Prompt`) |

**`''` is set by every jump, in one place.** vim's jump motions
(`:h jump-motions`) that exist here: `gg`, `G`, `{N}G`, `H`/`M`/`L`,
`]]`/`[[`/`][`/`[]` — all `CursorMotion`s, all through
`prompt::apply_motion`, so a `CursorMotion::is_jump` there is the door —
and search (`/`, `?`, `n`, `N`, `*`, `#`), which moves the cursor on its
own path (`App::search_again`) and is the second door, named in the
predicate's doc and pinned by a test per motion. A mark jump sets it too.
The jumplist (`CTRL-O`/`CTRL-I`) is later; `''` is its first entry.

## 6. Keys and commands

- `m{a-z}` sets, in a transcript pane (normal mode); `'{a-z}` goes to the
  line's first non-blank, `` `{a-z} `` to the exact place; `''` and
  `` `` `` swap with the context mark. `'` and `` ` `` are free in normal
  mode (only the text objects `i'`, `` i` `` use them, after an operator).
- `:marks [{marks}]` — **a page buffer**, as `:ls` and `:tabs` are: one row
  per mark (`mark line col text`, vim's columns), live, `<CR>` on a row
  going there. vim's is a message; ours is a buffer because everything
  else that lists is.
- `:delmarks {marks}` / `:delmarks!` (vim's `:delm`), refused naming the
  letters that were not set.
- A mark set is said ("mark `a` — line 412"), a jump is not (the cursor
  moving is the account).

## 7. Build order (each green-committed, mutation-checked)

- **M1 — `Marks`**: a pure type (set, get, erase-in-range, the insert
  shift), proptest: no mark ever names a line past the end.
- **M2 — the keys** in a transcript: `m{a-z}`, `'`, `` ` ``, refusals said;
  two windows on one session share them (Steven's case, as a test:
  mark, split, jump from the split).
- **M3 — `''`**: `is_jump` at `apply_motion`, search's door, a mark
  jump's; a test per jump motion (a list built from the enum, not beside
  it).
- **M4 — `:marks`, `:delmarks`**, the manual (`*shannon-marks*`), smoke.
- **M5 — `A`–`Z`** (after Steven's Q2).

## 8. Questions for Steven (my leans first)

- **Q1 — scope of the first cut.** Lean: transcripts only. The manual and
  other pages are static text and could take marks cheaply later; the
  prompt's draft is short; the tree keeps netrw's `m`.
- **Q2 — uppercase marks.** Lean: in this milestone, after M4 — your use
  needs only lowercase, and `'A` showing another session in this window is
  `:b`'s door, so they are cheap once the rest exists.
- **Q3 — persistence.** vim keeps lowercase marks across runs (shada).
  Lean: not in the first cut — live for the run, as windows are; docketed
  with the session store (a mark would be saved with its session).
- **Q4 — `:marks` as a live page** with `<CR>` jumping (lean), or vim's
  printed message?
- **Q5 — the name.** Lean: keep "marked pane" and "marked row" as they
  are, and always say a position mark with its letter (§3). A rename of
  the pane marks would be a bigger change than this one.
- **Q6 — visible?** vim shows nothing (a signs plugin does). Lean: nothing
  in the first cut; `:marks` lists them.
