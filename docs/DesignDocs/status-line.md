# The status line

Steven, 2026-10-14: "let's simplify what's on the status line … We'll also
want to make this configurable for people. So there should be macros
associated with these things. I think starship has a helpful syntax that we
can follow … I'm open to suggestions here if you think you know a better way."

## What ships

One format per kind of pane, in `[tui.status]` (`shannon-core`'s
`StatusConfig`, read by `shannon-tui`'s `status_line`):

| key | shipped row (idle) |
|---|---|
| `prompt` | `[NORMAL] prompt · sonnet · ctx 40% · tok ↑1.2k (85% cached) ↓300` |
| `output` | `[NORMAL] output · sonnet · ctx 40% · tok ↑1.2k ↓300` |
| `decision` | `[NORMAL] decision · help `:help decision`` |
| `graph` | `[NORMAL] graph · 3/5 · help `:help graph-keys`` |
| `page` | `[NORMAL] rules · 12/40 · help `:help rules`` |

`(read-only)` means vim's `'nomodifiable'`: no text can be typed in, though
keys may act — the graph's `d` deletes. The word is borrowed: vim shows
`'nomodifiable'` as `[-]`, and its `[RO]` is `'readonly'`, under which
typing is allowed and only a write fails (review stop 13, C 9; the word
kept, Steven's call). `$readonly` is true in every pane but the prompt.

**Where the row says it.** Steven, 10-27 (review stop 13, S13-Q1), with a
screenshot of vim's `E21: Cannot make changes, 'modifiable' is off` on `i`
in a help file: "That's the only time vim tells me that it is read only. I
think we can do the same with `:graph`." So the graph and the decision
pane say it when a key would type (the graph's `i`: "this pane is
read-only"; a decision's: its legend, the keys that answer), not on the
row. Q12 had put it on the graph's row the same day, to agree with that
`i`; the agreement now lives in the predicate, `$readonly`, which any
format can carry. The `output` and `page` rows dropped it too on 10-30
(S14-Q1, Steven: "Yes"), so no shipped row says it; every pane but the
prompt says it when a key would type.

`page` serves every read-only page (`:help`, `:rules`, `:ls`, `:messages`,
`:alias`, `:scriptnames`, `:themes` and its preview, `:analyze`, the
inspector); `$pane` and `$help`
say which. Gone from the row: `^L ^C` (`^C:stop` stays while it stops
something), `i:insert Esc:normal ^W:focus`, `j/k move …`, the terminal's
size (`$size` if you want it) and `out N ↧` (`$lines`, `$follow`).

## The syntax, and why it is not quite starship's

Starship's grammar (`src/formatter/spec.pest`) reserves `[ ] ( ) $ \`. A
bare `[NORMAL]` is a parse error that replaces the whole prompt with `>`,
and a variable-free `(read-only)` is a conditional group that never shows —
probed with starship 1.26. Those are exactly the characters status lines are
made of (vim's `[+]`, helix's `[readonly]`, tmux's `[#S]`), and Steven's
default row has both. So:

- `$name` / `${name}`: starship's.
- `[text](style)`: starship's, but a `[` not closed by `](` is text, as in
  markdown. Style words are starship's (`bold`, `fg:#hex`, `bg:red`, a bare
  colour), colours as a theme writes them (`theme::colour_word`).
- `{ … }` is the group (starship's `( … )`): shown when a value in it is,
  and **always when it holds none** — vim's `%( %)` rule, not starship's.
- `\` before `$ [ ] { } ( ) \`.
- **Unknown names are said at load and drawn as written.** Starship, vim and
  tmux all drop them silently; that is the one thing this project does not
  do.

## Width: position is priority

Surveyed: tmux and vim cut by position (`%<`, region order); Powerline,
feline, heirline and zjstatus rank segments by number — and disagree on
which way the number runs (Powerline drops the larger first, feline keeps
it). So no number: **the rightmost group goes first**, then the next, then
what is left is cut with `…`. The order on screen is already a ranking the
user can see.

**One exception, by content not position**: a group showing an alert
(`$alerts`, `$decide`, `$queued`, `$running`, `$interrupt`) is never
dropped, only cut. A row that clips is honest; one that silently omits a
pending decision is not — and `DECIDE:` exists for a terminal too small to
draw the decision pane, exactly when the row is too narrow. This was the
old `Priority::Essential`, the reason the row ranked at all.

## Values

`status_line::Var`, declared once by a macro so `Var::ALL` is complete by
construction; the manual's list (`*shannon-statusline*`) is held to it by
`the_manuals_status_values_are_the_vars`. Each is computed at the draw by
`view::status_value`, an exhaustive match. `$alerts` and `$activity` are
joins of their parts, so a short format still says everything that waits.

## Token counts

Steven, 2026-10-15: "add an option to make these numbers human readable
and do that by default" — his row read `tok ↑10415096 (95% cached) ↓39996`.
`[tui.status] tokens = "short"` (default) or `"exact"`, read at every draw
(`view::token_values` → `status_line::token_count`).

- **One decimal, to the nearest, `.0` dropped, SI `k M G T P E`**: `999`,
  `1.2k`, `40k`, `10.4M`. The GitHub/Claude Code `12.3k` style. Not `du -h`
  or `numfmt --to=si`: both round UP (right for disk, where promising room
  you lack is the failure) and `du` keeps a decimal only below ten, so
  `10415096` reads `11M`, a 6% overstatement of a bill.
- **Never `1000k`**: a count that rounds to a thousand of a unit is written
  in the next (`999950` → `1M`).
- **`tokens`, not `numbers`**: it governs token counts only. `ctx` is a
  percentage, `$position` and `$lines` are counts nobody reads as a size.
- **Not yet wired** (each its own rule today): the model picker's limits
  (`model_picker::tokens`, truncating: `4096` → `4k`), the inspector's
  `usage`/`tokens` (`view::usage_str`, exact on purpose), its `max tok` and
  per-message `~N tok`, and `app::unaccounted_output`'s notice.

## Not done

- A per-page key (`[tui.status] rules = …`) beside `page`: earned when
  someone wants one page's row different.
- Theme roles in a style (`[$model](ui.statusline.model)`): with the
  theme's `ui.*` roles.
- A style that follows a value (`ctx`'s gauge colours the number, not the
  `ctx ` before it).
