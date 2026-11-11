# Navigating a long conversation — blocks, motions, folds, marks, the gutter

STATUS (2026-11-01): DESIGN round 2, BUILDING. Built: blocks and anchors
(10-27), your marks N3 and the agent's N4 (10-27/28), `:marks`, marks kept
with the session (10-28) and hardened by review stop 14 (10-31), N1a (the
thinking wire, verified live 10-31), N5's first primitive (`fold.rs`,
`:models`), N1b (thinking shown — Steven, 11-01, S21-Q1 (a); §4 item 4),
N5a (folds' core: blocks shut, thinking and records shipped shut, the `z`
verbs; §6). Not built: N2 motions, N5's rest, N6 the
gutter. Written 2026-10-27 after Steven's account of the goal
(quoted in §1). Supersedes `marks.md` (round 1), whose vim
measurements and anchor analysis this relies on. The questions (§10) are
open: NQ1–NQ9 asked 10-27 (reports say NQ, apart from a review's Q
numbers — the two series collided once), NQ10–NQ11 added 10-27 evening
after reading what the API asks of thinking (§4), which also found a gap
that is live today: the agent's notes between tool calls are dropped.

## 1. The problem, in Steven's words (10-26)

- "We have a core problem with conversations is that agents generate A LOT
  of text. We added the command to jump through the user messages but there
  are also other things people want to jump through … So we'll want jumps
  to our unique blocks … We're missing thinking output (we should have
  this). And of course tool calls are atomic blocks. So we should have
  movements for those, complementing our existing conversation jump. This
  is where our movement grammar can diverge from vim. But this should also
  motivate our folding design."
- Folding: "Finding these 'atomic' blocks and allowing a user configuration
  for automatically folding (thinking will automatically be folded by
  default)", and in the prompt too — "We don't have to fold anything
  automatically, but … Claude Code 'folds' pasted text by default. It lets
  you unfold but not refold, so that's annoying... and `<C-g>` unfolds that
  paste too" (a fold is a view: the editor takes the text whole).
- Marks: "every time I answer you my view jumps to the latest message.
  Luckily Shannon doesn't do this (and we always have `G` to go to the
  bottom trivially), but it is useful to mark those and come back to them
  … the purpose of marks is directly in line with a major problem we're
  trying to resolve: navigating through long and complex conversations."
- Agent marks (med/high, "I'd lean into doing this sooner"): "have the
  agent generate its own marks and in a turn summary it can tell the user
  to jump to that … a huge sell and fits our overall goal of making
  Shannon a tool for promoting that 'work *with* an agent' mentality. …
  It can be a shipped tool and/or skill, but we'll need to also remember
  to update our system prompt to tell the agent that it has these and how
  it should use them." `'` for your marks, `` ` `` for the agent's — vim
  makes the two interchangeable; `"` was considered and left to vim's
  registers; `` ` `` is harder to hit and already "machine-oriented
  notation" (markdown).
- Notes beside marks (low, after vim's annotated-bookmarks plugin):
  deferred until a quickfix-like list or bubbles.
- "I am less concerned if we're marking an exact position in a
  conversation insomuch as the atomic block. Exact position is better …
  I'd still encourage you to do exact position."
- `:marks` (round 1 Q4): "show not just marks but a bit of what they are
  marking so users can intelligently navigate to the correct item, not
  needing to memorize the mapping"; the quickfix-like menu may replace it
  later. Uppercase marks: not needed for the soft launch, though "a good
  thing that will make us look more polished". Persistence: "save along
  with the session … carry the metadata in our database attached to the
  conversation", not in M1.
- The gutter: a fold column as vim's `foldcolumn`, holding marks too (a
  mark with a note a different glyph); default width 1 ("normally this
  will just look like some extra padding"), 0 allowed, wider shows fold
  depth (his `usr_28.txt` screenshots at `foldcolumn=4`), glyphs
  configurable, good-looking defaults with an ASCII fallback, Nerd Fonts
  for those who want them; "our folds can be more aesthetically pleasing".
- Splits (10-25): a conversation is a file — `:vsp` on one session is two
  views of it, so marks are shared; he reads "the turn summary" in one
  pane and an earlier explanation in the other, marking while it streams.
- Pace: "No need to rush … I'd rather be slow and right than fast and
  wrong."

## 2. My reading, restated (for Steven to correct)

**One structure, four operations on it.**

- **The structure.** A transcript is a sequence of ATOMIC BLOCKS, each of a
  kind — your message, the agent's text, its thinking, a tool call and what
  came of it — and a turn groups the blocks one message of yours set off.
  A block is the unit of meaning; finer units inside the agent's text (a
  code block, a heading) are a second tier, by the same mechanism, later.
- **Motions** move between blocks of a kind (`]]` already walks turns).
- **Folds** shut a block to one line that says what it holds, and keep it
  reachable. **Rules** decide what starts folded (thinking does). This IS
  the verbosity knob the docket already named — "quiet" means folded and
  reachable, never absent (`TODO.md` §"Folding").
- **Marks** remember a place so you can return to it: yours (`'`), and the
  agent's (`` ` ``), which it hands you by name ("the open question is at
  `` `c ``"). The block is what matters; the exact place is a bonus.
- **The gutter** shows folds and marks at a glance, beside the text.

The four read ONE answer to "what are the blocks here" — so a motion, a
fold, a mark's preview and a gutter glyph cannot disagree about where a
tool call starts.

## 3. What exists (measured 10-26)

- **Blocks.** `output::BlockRole` is `User`, `Assistant`, `Tool`, `Rule`,
  each block tagged with its turn. The agent's text is ALREADY split at each
  tool call (`push_tool_record` seals the text before the record and opens
  a block after it), so "agent text" and "tool call" are blocks today.
  `Rule` is decoration (the line framing your message), not a kind.
- **Turn motions.** `]]` `[[` `][` `[]` (`Output::sections`).
- **Thinking: dropped, everywhere** (as of 10-27; kept since N1a, shown
  since N1b — §4). The API backend: "Thinking delta —
  silently dropped in M1" (`backend/anthropic.rs:1028`). Claude Code:
  thinking "means nothing here" (`backend/claude_code.rs:107`). The Claude
  Code importer counts and skips it (`claude_import.rs:1314`). Not stored,
  not shown.
- **Folds.** The graph's tree folds (`GraphView`'s `expanded`); the
  transcript has none. And a Write's diff is CUT at 400 lines ("… N more
  lines", `tool_record.rs:31` `MOST_SHOWN`) — absent, not folded.
- **The `z` prefix.** `zt` `zz` `zb` `z<CR>` `z.` `z-` are taken (scroll);
  `zo` `zc` `za` `zv` `zx` `zR` `zM` `zr` `zm` `zj` `zk` are free.
- **Brackets.** `]]` family taken (turns); every other `]x` free in a
  transcript.
- **A session's transcript changes only at its tail** (`marks.md` §4), so a
  place in it is stable within a run.
- **Marks, measured in Neovim** (`marks.md` §2): a lowercase mark is the
  buffer's, `''` the window's; erased with its line (E20).

## 4. Blocks — the one structure

**Kinds**: turn ⊃ { your message, thinking, agent text, tool call }. One
query per session — the blocks of a kind as line spans — read by motions,
folds, `:marks` and the gutter.

**Thinking** needs three things:
1. parsed, from both backends, into a block of its own;
2. stored with its turn — it is part of what the model produced (Q1: is it
   part of the turn's content-addressed identity?);
3. shown as a block, folded by default.

**Known unknown, to read before building:** what the API asks of thinking
across a tool round — whether a request continuing a turn that used tools
must send the thinking blocks back (with their signatures). Today nothing
is kept, so nothing is sent; storing it may change what a round must send.
Read the API reference (the `claude-api` skill) first, not memory.

**Read 10-27** (the skill's `shared/model-migration.md`, the Sonnet 5.5,
Opus 5.5 and Fable 5.1 sections; facts of the docs, not measured live —
the binary never runs against the API here):

- **What Shannon gets today.** It sends no `thinking` field
  (`anthropic.rs`'s request struct) to its default `claude-sonnet-5-5`, so
  thinking is adaptive (on), and `display` defaults to `"omitted"`: each
  thinking block arrives with an empty text and a `signature`. The
  parser drops thinking deltas and blocks.
- **The agent's notes between tool calls are thinking blocks.** On Sonnet
  5.5, a note longer than a sentence or two between tool calls comes back
  as a *progress-update* `thinking` block (Opus 5.5 and Fable 5.1 alike),
  empty under `"omitted"`. So **a long tool-using turn goes quiet in
  Shannon today** — inferred from the docs and the parser, not seen.
- **Display.** `"omitted"` (default); `"updates"` — progress updates as
  text, reasoning hidden (beta `thinking-display-updates-2026-08-18`;
  under it, any non-empty thinking block IS an update); `"summarized"` —
  reasoning summaries and updates, mixed (not told apart by type). An
  update streams as `thinking_delta` before the `tool_use` it introduces;
  zero or more per response; a response cut short can end on one whose
  text is exactly `This part of the response was interrupted before it
  finished.` — continued by sending the turn back unchanged.
- **Sent back, unchanged — every block, every request.** "A block sent
  back gives the model the full note it wrote." Stripping them all is
  what Shannon does — on every request, mid tool round, on its default
  model — and the guide does NOT call that a steady state (review stop
  13, C 7): strip-all is "a one-time recovery, not a steady-state
  pattern"; "dropping or editing them breaks the turn" (the same model);
  an assistant turn whose `tool_use` awaits its result "should go back
  with its thinking intact"; Sonnet 5.5's section: "don't strip them
  yourself". Whether today's requests succeed regardless is not measured
  here (no live API) — one more reason N1 comes first.
- **Preserved thinking: a block is bound to its prefix.** Its signature
  records the top-level `system`, the set of `tools`, and every message
  before it, and chains to the previous thinking block. Change any of
  them and every later block is invalid: a 400 on accounts created on or
  after 2026-08-31 (older ones opt in), unless the request sets
  `thinking.block_binding.prefix_mismatch_behavior: "drop_block"` (beta
  `thinking-binding-controls-2026-08-01`), when the API drops the first
  invalid block and every one after it and lists them in the response's
  `input_transformations`. Removing a LEADING run of blocks is fine;
  removing one from the middle is an edit.
- **Bound to its model too.** An Opus 5.5 block is read only by Fable 5.1
  and Mythos 5.1; a `:model` switch to anything else drops it, unbilled.
- **Thinking counts toward `max_tokens`** even when its text is omitted.

**What that means for the graph.** Shannon composes `messages` itself, so
it owns the prefix check. Append-only continuation keeps every block
valid; a branch at turn N keeps the blocks up to N (same prefix). An `e`
edit, a squash, a merge, a distill or `:compact` (keep-tail compaction
breaks the kept turns' blocks; replacing the whole history with a summary
does not), a changed system prompt or tool list between requests
(docketed: neither is sealed per turn), and a `:model` switch each make
later blocks unsendable — still worth SHOWING, as what the model thought
then. NQ1 and two new questions (§10) turn on this.

**Per model — it is not one request shape** (Steven, 10-27: "support a
few of the <5.0 models (like Opus 4.8)"; from the docs' thinking,
extended-thinking and effort pages, read 10-27):

| Models | Thinking | `display` default | Earlier turns' thinking |
|---|---|---|---|
| Opus 5.5, Opus 5, Sonnet 5.5, Sonnet 5, Fable 5.1/5, Haiku 5.5 | on by default; `{type: "adaptive"}` (Opus 5.5, Fable 5.1 and Fable 5 accept nothing else; Sonnet 5.5's lowest is `between_tools`) | `"omitted"` | kept |
| Opus 4.8, 4.7, 4.6, Sonnet 4.6 | off until `{type: "adaptive"}`; `"enabled"` + `budget_tokens` a 400 from 4.7 on | `"omitted"` (4.7, 4.8); `"summarized"` (4.6) | kept |
| Haiku 4.5 and earlier | only `{type: "enabled", budget_tokens: N}`; `"adaptive"` a 400; no interleaving between tool calls (Haiku 4.5) | summarized | last turn only (the API strips older ones itself) |

Sources, read 10-27: the API's pages, not only the skill —
`platform.claude.com/docs/en/build-with-claude/thinking` ("Thinking block
preservation by model": keep all prior turns for Opus 4.5 and later Opus,
Sonnet 4.6 and later, Haiku 5.5, Fable 5.1/5; the last turn only for
earlier Opus and Sonnet and every Haiku through 4.5) and
`…/extended-thinking` ("Claude Haiku 4.5 does not support interleaved
thinking"; `type: "enabled"` rejected from 4.7). The rest is the skill's
`shared/model-migration.md`.

**N1 as it is built (10-30; my calls marked, for Steven to overturn).**
Read again from the skill (`shared/model-migration.md`,
`shared/preserved-thinking-migration.md`, `python/claude-api/streaming.md`):
a `thinking` block is `{thinking, signature}` and streams as
`content_block_start`, `thinking_delta`s and a `signature_delta`; some
models send `redacted_thinking {data}`; every one goes back unchanged,
empty ones too; with the beta header `thinking-binding-controls-2026-08-01`
the response's `input_transformations` (on `message_start`'s `message`)
lists each block the API dropped, `{type: "thinking_dropped", path,
reason}`.

1. **The order a reply was written in is kept** (my call). A message's
   text lives in `content` and its other blocks beside it, the text sent
   first — a reply written `[thinking, text, thinking, tool_use]` (a
   reasoning block, a remark, a progress update, a call) went back
   reordered, which is an edit of the prefix every later block is bound
   to. A thinking block now records how much of the reply's text came
   before it (`at`, in memory only: derived from the array on read, from
   the text so far as it streams), and the message is written in that
   order. A `tool_use` keeps today's rule — after the text before it —
   so text the model wrote between two calls of one reply would still go
   first: not seen in the API's shapes, and `drop_block` (3) turns it into
   a dropped block said, never a 400.
2. **Parsed, both backends.** The API's block assembler follows the open
   block, a tool call or a thinking block; thinking text streams for the
   screen, and the whole block, signature and all, is recorded when it
   closes, as a tool call is. Claude Code's stream carries the same blocks
   (shown; Claude Code keeps its own history, so nothing is sent back).
3. **Asked for, by category** (§11, second round): a model of 4.6 or later
   is sent `{type: "adaptive", display}` with `block_binding:
   {prefix_mismatch_behavior: "drop_block"}` and the beta header (NQ11's
   defence, (b)); 4.5 and earlier, `{type: "enabled", budget_tokens}` only
   when `[thinking] budget` is set, nothing otherwise — today's request
   (my call: thinking on a budget is billed thinking you did not ask
   for). The generation is read off the id (`claude-opus-4-8` → 4.8), so
   a model typed in full works without a row; the stub and an id with no
   version are sent nothing. `[thinking] display = "summarized"` (NQ10).
   A dropped block is said: "the API dropped N thinking blocks — the
   conversation before them changed".
   **Verified live 10-31** (`examples/live_probe.rs`, Steven's approval to
   automate): every row of the alias table, 4.6 to 5.5 adaptive with the
   binding header and Haiku 4.5 on a 2048 budget, answered at its own reply
   ceiling; on Sonnet 5.5 a block written before a call went back on the
   next round and was kept (nothing dropped — (1)'s order is the one the
   binding expects), and after the first message was edited each block
   sent was dropped `prefix_binding_mismatch`. One of Steven's sessions saw
   a model switch drop one as `model_binding_mismatch`.
4. **Shown** (N1b): a thinking block of its own in the transcript, shut,
   its first line on the fold (NQ10, NQ3), live and on a reopened turn.
   **Built 11-01, open** (S21-Q1: "Do a first" — open now, shut once N5's
   folds exist), **and shut the same day by N5a** (§6: one row, its first
   line, ` · N lines`; `zo` opens it): `BlockRole::Thinking` (saved word
   `thinking`), placed in
   the reply where it was written as a tool record is
   (`Output::append_thought`, `put_in_order`), drawn in
   `ui.text.quiet.thinking` (unset as shipped, so each theme's quiet text;
   italic and dim, 11-01: "Italic helps but is not enough"; its markdown
   kept), live from the stream's
   thinking chunks and reopened from the kept blocks through the same
   doors (`replay_turn` splits the text at each block's `at`), so a mark
   in a thought is kept and lands. My calls, for Steven to overturn: an
   empty block (`"omitted"`, Claude Code's default) draws nothing — a row
   per round saying nothing; an encrypted one draws a line saying so; a
   thought cut off by a stop or failure says so and is drawn this run
   only (`Unkept::LiveOnly`: the turn keeps no block that never closed);
   the agent's `mark` never quotes into thinking (the summary is not its
   words); thinking a turn showed counts against `unaccounted_output`'s
   bill; a turn with only thinking streamed, stopped by CTRL-C, is still
   removed whole (S20-Q1). Docketed: a backend sending a thought's block
   after the reply's next words (not seen) would store it lower than it
   streamed, so a reopen would draw it there.
5. **Shannon's own check** (NQ11 (a), N1c — NOT BUILT, docketed at review
   stop 15, E L10): each turn's provenance keeps the hash of the prefix it
   was produced under; a request whose prefix hashes otherwise strips that
   turn's thinking and every later turn's before sending, and every graph
   surgery says what thinking it leaves unsendable (NQ1's limits).

So the request's `thinking` is a per-model capability in `models.rs`,
beside the effort levels it already lists — not a constant. **Effort
too:** an effort change sent per message (a `role: "system"` message with
`output_config`, beta `mid-conversation-output-config-2026-07-01`) keeps
the prompt cache on Fable 5.1, Mythos 5.1, Opus 5.5, Opus 5, Sonnet 5.5
and Haiku 5.5; on Sonnet 5, Fable 5 and every 4.x only the top-level
value exists, and changing it starts the cache over (what `:effort`
says today, for every model). Docketed with N1.

## 5. Motions — where the grammar leaves vim

vim's bracket pairs are the convention for "next / previous thing"
(unimpaired's `]q` `]b`; Neovim's own `]d` `]q`), so blocks take them.

| Keys | Moves to | Note |
|---|---|---|
| `]]` `[[` | a turn's start — your message | exists; a turn begins at its first block, your message (`Output::sections`), so a `]u` would be this twice (Steven, 10-27) |
| `]a` `[a` | the agent's text | Neovim maps `]a` to `:next` (the argument list), which Shannon has none of |
| `]t` `[t` | thinking | Steven's pick (10-27); Neovim's `]t` is `:tnext` (tags), none here either |
| `]e` `[e` | a tool call | "execution" (Steven, 10-27, over `]c`, vim's diff-mode "next change"); free in vim and Neovim |
| `['` `]'` `` [` `` `` ]` `` | the previous / next line with a mark of yours (`'`) or the agent's (`` ` ``) | vim's own (`motion.txt`); silent today (review stop 14, A 8). With them `g'` / `` g` ``: vim's `'` / `` ` `` without touching the jumplist, so the same as `'` / `` ` `` until `''` has one — needs a prefix after a prefix in the motion table |
| `]d` `[d` | a turn not in the conversation — a failure, a command stopped part-way (`Span::dropped`, drawn in `ui.danger`) | Neovim's diagnostic jump (`]d` `[d`); vim's own `[d` shows a `#define`, which Shannon has none of. Steven, 10-31 (S19-Q2): "some powerusers will make good use of this", both directions |
| `zj` `zk` | the START of the next block / the END of the previous one, of ANY kind | vim's fold motions, open folds or shut (Steven tried it): `zj` to the next fold's start, `zk` to the previous fold's END (`fold.txt`) — kept so, not both to a start (review stop 13, C 8) |
| `[z` `]z` | the start / end of the block you are in | vim's "start/end of the open fold" |

Counts (`3]t`), operators over them (`y]t`), and each a jump (it sets
`''`, so `''` comes back). **Checked 10-27 against Neovim 0.12's
`index.txt` and its default mappings (`vim_diff.txt`):** vim's own
bracket commands are `[#` `['` `[(` `[*` `` [` `` `[/` `[D` `[I` `[P` `[[`
`[]` `[c` `[d` `[f` `[i` `[m` `[p` `[s` `[z` `[{` and their `]` twins;
Neovim remaps vim's `[d` `]d` `[D` `]D` (show and list `#define`s) to
diagnostics, and adds `]q` `]l` (quickfix, location list), `]t`
(tags), `]a` (arguments), `]b` (buffers), `]<Space>`. None of `a` `t` `u`
`h` is vim's own. Two follow from it: **`]b` `[b` should be `:bnext`
`:bprevious`** — Shannon has buffers, and a Neovim hand will press them
(N2); and `]q` `[q` wait for a quickfix list (`:marks` becoming one).
`a}` is a text object, after an operator — a different sequence from
`]a`, so the two complement (`d]a` would be an operator over the motion).
`(` `)` and `{` `}` (sentences, paragraphs) move inside text, a level
below blocks — no clash; Shannon has `ip`/`ap` but not those motions yet
(measured by grep), a vim gap of its own.

## 6. Folds

- **Units and levels.** Level 1 a turn; level 2 its blocks; level 3, later,
  a code block in the agent's text. `zM` shuts everything: the
  conversation's OUTLINE, one line per turn. `zR` opens it all; `zr` `zm` a
  level at a time; `zo` `zc` `za` `zv` `zx` as vim's.
- **The window's**, as vim's folds are: two windows on one session fold
  apart (beside the cursor, in `SessionView`).
- **A folded block says what it hides** — vim's `+--  4 lines: …····`, ours
  plainer to read: `▸ thinking · 42 lines`, `▸ $ cargo test · 812 lines ·
  exit 0`, `▸ turn 12 · "should we keep the twin rule…" · 3 tool calls`.
- **Rules** (`[tui.fold]`, Q3 for the shape): per kind, open or shut by
  default; **thinking and tool calls ship shut, whatever their length**
  (Steven, 10-27: "Tools and thoughts are default folded. But they can be
  unfolded depending on settings"); `{ shut_over = N }` is a setting, not
  the default. Folding retires the 400-line cut: the lines are there,
  folded. A fold you set is kept; one you never touched follows its rule
  (the graph's rule today, `default_open`).
- **What opens a fold for you**: a search landing in it, a mark or a motion
  going into it (vim's `foldopen`).
- **The prompt folds too.** A paste is a block of the draft (bracketed
  paste says where it landed), open by default (Steven: "We don't have to
  fold anything automatically"), and `zo`/`zc` work both ways — where
  Claude Code's opens once and never shuts. A fold is a view: `$EDITOR`, a
  yank and `:Send` take the text whole.

**N5a — the core, for the transcript's blocks (designed 11-01, building;
Steven: "continue right into N5").** Level 2 first (a block); turns
(level 1, `zM` as the outline), the prompt's pastes and the gutter after.

- **Whose.** The window's (`SessionView`), as above: a split copies them,
  as vim's `:split` does; a session made from another carries them on the
  turns it keeps, by place as marks are (S16-Q2), then they diverge.
- **What folds.** A block whose content takes two or more ROWS at the
  window's width — vim's `foldminlines`, which counts screen lines
  (measured in Neovim 0.12, review stop 15): a one-line thought wrapped in
  a narrow window folds, the same thought one row wide does not. Its
  trailing blank line, the separator, stays outside, so a shut block keeps
  its spacing. One rule for the drawing, the `z` verbs and the keys
  (`Block::can_shut`); the keys' copy once counted LINES, and walked into
  a wrapped line drawn shut (A L9). Its state: what you set (`zo` `zc`
  `za`; `zR` `zM` set every fold there is), else its kind's rule —
  thinking and tool records shut, the rest open (NQ3).
- **Drawn.** One row: `+`, the block's first line in its own colours cut
  to the row, then ` · N lines` in `ui.text.faint` — so a thought reads
  as its first line (NQ10), a record as its `⚙` line. A command's record
  draws its TITLE there instead (vim's `'foldtext'`): `⚙ ran: cargo test`
  or `⚙ failed: …`, the line its call stood as while it ran, since its
  `⚙` line alone said `⚙ output of:` of every command, a failed one's as a
  clean one's (review stop 15, E H2). After the count, what the fold hides
  that you would want to see (S21-Q13, E M3): `· cut off` for a thought
  the turn ended before, `· leaked value` in `ui.secret.leaked` for a
  value the model wrote itself under it — the view's to find, which holds
  the secret table, the row's text cut again to leave the word room. A
  shut fold's first line is one row high and the rest none: every scroll
  and follow reads heights, so they need nothing else.
- **A record closes in place.** The record that closes a call takes its
  `⚙ running:` line's place (F H2): below it, the running line stayed
  above the record, and shut the two read as a command still running. In
  place, live draws what a reopen draws, block for block; a window reading
  below it keeps its line and top by its blocks, as vim's `mark_adjust`
  keeps a window past an edit above it.
- **Moved.** `j` `k` step over a shut fold as one line, onto its first;
  an operator takes it whole (`yy` on it yanks every line — vim's "a
  closed fold is included as a whole"); what lands in one by search, a
  mark, `]]` or a motion along the line opens it (vim's `foldopen`
  default), `G` `gg` `j` `k` do not.
- **Not kept past the run** (as vim's, without `:mkview`): reopened, every
  block follows its rule again (S21-Q2, Steven 11-01: "Only folds that stay
  are due to settings").

**N5a as built (11-01; 1b242e5, 483fa48, and the keys' commit).** Every
rule above, each measured first in Neovim 0.12 (manual fold over lines
2–4 of six, keys fed as typed, since `:normal!` is a mapping and
'foldopen' does not apply to one):

- `Buffer::set_folds` — the navigation copy's shut lines (sorted,
  overlapping joined, touching kept apart, empty dropped). `j` `k` step
  over one; `{n}G` keeps its line inside one; `zc` keeps the cursor's line;
  `w` `W` `e` leave by its last char, `b` by its first line; an operator
  takes it whole (`yy` `2yy` `yj` `V` linewise, `yl` `yw` charwise). A
  charwise selection into one is not widened (Neovim's `vjy` is not).
- `output::Folds` on `SessionView`, `output::Shut` (`App::pane_shut`)
  asked by every question about rows — a parameter, so the compiler found
  all nine callers. Each block's wrap cache holds both shapes; a shut row
  is a prefix of its first line, the view drawing `+ `, `…`, ` · N lines`
  around it in `ui.text.faint` — or of its title (`FoldRow::titled`, a
  command's record, review stop 15), whose cells are no line's: a secret
  on it is read on the row itself, and a selection lights it whole (its
  first line wholly selected, what an operator then takes whole) or not
  at all, as vim lights a closed fold.
- `zo` `zO` `zc` `zC` `za` `zA` `zR` `zM` (`bindings`' `AfterZ`, names
  `fold-open` … `fold-close-all`); no fold at the cursor says so (vim's
  E490), and so does a pane that folds nothing yet. 'foldopen':
  `CursorMotion::opens_folds` (`l` `$` `w`…, a mark, `]]`) and a search
  open the fold landed in; `j` `k` `G` `gg` `{n}G` `H` `M` `L` do not.

My calls, for Steven to overturn: `zR`/`zM` hold for blocks still to come
(a thought arriving after `zR` streams open) until one is set, or its
kind's rule changes — a changed rule, or a `:set` of one even to the value
it had, puts that kind's folds in every window back to it, hand-set ones
too, as vim's `:set foldlevel` does (measured, Neovim 0.12; review stop 15,
E L5: `zM` outranked every later `:set`); other kinds keep theirs, since
each kind has its own rule where vim has one level (Steven, 11-01: a
`:set tui.fold.thinking=open` after `zM` opened every reply); a reply,
your message and a record all fold with `zc`, only thinking and records
start shut; the 400-line cut of a Write's diff stays until the records
are re-measured shut; a block one row long at the window's width has no
fold (vim's `foldminlines`, in screen lines).

**N5a.4 (11-01).** Measured first in Neovim 0.12.6 (`fold-oracle4`):
`:split` copies a window's manual folds, opened and shut, and `zR`'s
`foldlevel`; the two then fold apart; a window that shows another buffer
(`:enew`) and comes back (`:b 1`) finds its folds as it left them.

- `window::Place` holds the window's `Folds` beside its cursor and
  scroll, so the split (`split_command`'s `Away::Session`) and every
  return (`put_place`, from `<C-^>`, `:b N`, a viewer given back) carry
  them; a reshape of the session (`forget_cursors_on`) puts each window
  at the session's entry, folds and all.
- `[tui.fold]` (`config::FoldConfig`, which `output::FoldRule` now IS):
  `thinking`, `tool` — `"shut"` (shipped), `"open"`, or `{ shut_over = N
  }`, shut when it holds more than N lines, the count its row shows. Read
  at every question (`App::fold_rule`), so a `:set` shows on the next
  frame. A misspelt key in the table is refused; a value it cannot read
  says the three it can.
- `Output::fold_at` takes the window's `Shut` and answers whether it
  holds the block shut: both its callers asked that next, and the rule
  needs the block's length (`Shut::holds_block` is gone).

**A window's place carried by position (S21-Q4, Steven 11-01: "Carry
over whole place by position").** A session made from another starts
where the window reading the source was: its cursor, the line at its top
(vim's `winsaveview` keeps `topline`) and its folds, each an
`output::Position` — (turn, kind, nth, line, col), skipping blocks drawn
only this run — through the door's `TurnMap`, the one its marks go
through. What the map drops is the new session's entry's. `Output::
keep_view` / `find_view`; `app/carry.rs`, two shapes:

- **Over** (a compaction, a squash carrying its source: `Lands::Over`):
  every window on the session, showing it or holding it set aside, is
  kept before the landing (`views_on`) and found after (`put_views`) —
  `forget_cursors_on` had put them all at the entry.
- **Into** (`:clone`, an edit, `:fork`/`:vfork`): the window that shows
  the result, which set the source aside entering it, starts where it was
  reading it (`carry_into`, `carry_into_shown`). A fork's new split now
  starts at the window it splits from, as `:split` copies one — the
  session it forks, its alternate, there too (it was the entry).
- **Not a checkout** (S21-Q5, Steven 11-01: "it is correct to land on the
  turn you are in"): `<CR>` on a turn in `:graph` lands at the checkout's
  end, the turn picked. Carried, it landed two ways, by whether the
  graph's pane had been reading the session (a bare `:graph`) or not
  (`:sg`).

The top is a line and column, carried as it is — drawn or not, at any
width (it was read at the size the window's rows were laid out at, and a
window never drawn carried its rows raw, until S21-Q10 below). A view
placed by its top keeps its cursor on screen and no more (`H`'s margin) —
a rebuilt transcript can push the cursor below it. Frozen it was, frozen
it stays, on the last page too: following the end is `G`'s to take up.

**A window keeps its top line when its width changes** (Steven, 11-01,
images 7–9: after `:vfork` "both scroll and in different ways", and
CTRL-^ scrolled). A transcript's scroll was a ROW, and rows are a width's:
the `:vfork` halved the original, the same row stood earlier in the text,
and it jumped back; the fork's top was read at the new width from the old
rows; CTRL-^ put the source back by its old rows. vim keeps `topline`, a
line. First fixed by re-anchoring each frame (`keep_transcript_tops`,
ed22a97), which review stop 15 found still lost the line wherever an input
of a row changed with no re-anchor after it — a burst of keys before a
frame, `zR`/`zM`, `:set tui.fold`, a pane with no room (A H1, M5, M6).
**S21-Q10, phase 1 (11-02): the top is TEXT** — `Viewport` holds a line
and column, and its row is derived where it is read (`Output::top_row`),
at whatever width, folds and rule the window has; a move writes the text
at the row it moved to (`Output::move_top`), and a top that did not move
keeps its own. What a new width still owes is the cursor on screen (vim's
own on a resize) and the keys' copy a sync (`App::note_transcript_sizes`).
Phase 1b: a page's top too (`TextView::top_row` — `:help`, `:messages`,
`:analyze`, the listings, the decision pane; review stop 15, D M3: `:help`
under a `:vsplit` came back six lines up, Neovim keeps the line).

**`:graph` opens on the turn you were reading** (image 11: the cursor on
turn 2, the graph on turn 3): the turn under the focused transcript's
cursor (`App::turn_read`, `GraphView::reveal_and_select_turn`), on the
row that draws it — a turn a branch shares is drawn under its owner
(`Graph::placed`), image 15. It first asked only a window scrolled back,
reading "following its end" as "its cursor never moved"; but a window
follows its end whenever its top is the last screenful's, so a
transcript that fits its window opened on turn 3 with the cursor on turn
2 (review stop 15, A H3 / C H3). Instead a window born at its end has its
cursor there (`SessionBuffer::entry`, the last line), as a checkout's
does (`App::at_its_end`), so the cursor always says where you are — the
last turn where it never moved, as Steven asked ("when entering `:graph`
we should land on the current turn too").

**A reply has no fold until you make one (Steven, 11-03).** `zM` shut
every block, replies and your messages too — "The surprise was that I
didn't define a fold for the conversation text. Default should be that we
manually fold that text, but not that there is a fold there already. In
the fold settings a user can set that, but that's not the default." So
vim's model: `zM`/`zR` shut and open every fold there is
(`Folds::set_all`), and lines no fold holds stay drawn. `[tui.fold]`
gains `reply` and `message`, and a fourth value, `"manual"` — no fold
until `zc` (or `za`) makes one — shipped for both; `"open"` gives every
block of the kind a fold that starts open, which `zM` shuts. A fold made
by hand outlives `zM`/`zR` (they cleared what was set); `zo` on a block
with no fold says so (vim's E490) and makes none, or the next `zM` would
shut a reply never folded.

Not yet: turn folds (level 1), `zr` `zm` `zv` `zx` `zj` `zk`, `zd` `zE`
(a fold made by hand is undone only by a `:set` of its kind), the
prompt's pastes, the gutter.

## 7. Marks

**Yours**: `m{a-z}` sets, `'{a-z}` goes. With `` ` `` given to the agent,
vim's exact-place jump is gone, so `'a` lands at the exact place marked
(its column kept) — still linewise as an operator target (`y'a`), as vim's
`'` is. `''` returns from any jump.

**Built 10-27 (N3, transcripts first):** `m{a-z}` and `'{a-z}` — `'` a
motion through the buffer's one machine (`CursorMotion::Mark`), so `v'a`
grows to it and `y'a` is linewise; the mark kept on the session
(`SessionBuffer::marks`) and told to each window's copy beside its lines
(`App::sync_chat_in`); setting one says its turn; one not set, or whose
block is gone, refused as E20; `m`/`'` refused, said, in any pane but a
transcript. Not yet: `''`, `:delmarks` (`:marks` and persistence came
after, below).

**Built 10-27 (N4, the agent's, on the API):** the `mark` tool
(`ToolDef::mark`, offered beside `bash`, foreground and unattended),
answered at once through `App::answer_own_call` — the one door for
Shannon's own tools, which `skill` now walks too. Input `{note, at?:
{quote, turns_ago?}}` — `turns_ago`, not a turn number: the model never
sees Shannon's numbering, so the quote is searched from this turn back
(`Output::find_quote`), and `turns_ago` narrows it. Here is the call's own
record (`⚙ marked: mark "note"`), so `` `a `` lands on the line saying the
agent marked it. Letters: the next free, the oldest's past 26 (NQ5); the
result names it ("mark `a` set here.", and for a quote where it landed,
review stop 14, A 6); asking the reply to name it is the description's
since c983067. The when and the naming are in the
tool's description — sent with every request, the system prompt's part
the API renders — not in Shannon's notes. `` ` `` charwise under an
operator, as vim's.

**Kept with the session, 10-28** (Steven: "We should store marks along
with the conversation, so agent marks shouldn't be destroyed when we
exit"). In the episode's header (`Episode::marks`, `shannon_core::Mark`),
re-put through the door a rename takes (`write_preferences`) at every
change, and with the first header for a session not yet on disk. A header
field, not a record kind: an older build ignores an unknown key, where an
unknown record on any line but the last fails the whole session; absent
keeps the last header's (as `inherited`), an empty list clears. **By
place, never by text** — turn, block role and which of that role, line,
column (`output::Position`) — because a transcript draws a held secret's
value, which the header must never hold. A reopened turn must draw the
blocks it drew live for a place to name the same text, and did not: the
replay trimmed a reply's trailing newlines (a row fewer before a record)
and called every own tool's record `output of` — both fixed, held by
`a_reopened_turn_draws_the_blocks_it_drew_live`. A saved mark a reopen
does not draw is kept unplaced, listed, and saved again. Docketed: a
surgery removing turns before a mark leaves it naming the turn after.

**Review stop 14 found three more ways a saved place named other text
(A 1–3), fixed 10-31.** A call's `⚙ running:` line is drawn live and not
reopened (the record closing the call is), so it is LIVE-ONLY: no place
counts it, a quote never lands on it, and a mark on it is said "not
saved". A turn the conversation drops but the transcript must keep (it
ran something, or says why it failed) is kept as NO TURN'S, so the
transcript numbers its turns as the store does (`Output::abandon_turn`).
And at every turn's or job's ending the marks are kept again if what
they stand on moved (`App::keep_moved_marks`), a mark leaving what is
kept said with why — kept only at the next save, one in a stopped turn
reopened on the next turn, which took its index. The live story the
parity test reopens now runs a command.

Steven saw "agent marks converted to user marks" after a restart (10-28):
nothing converts — the two maps have one writer each (`m`, the tool), and
a new process started them empty, since nothing was saved; the `'a` `'b`
he saw were set by `m` in the new run (asked to confirm).

**`:marks` built 10-27**: a live listing (`Listing::Marks`), every
session's — vim lists the current buffer's; here one page buffer serves
every window, so it lists each session with a mark under its `:ls`
number. Yours, then the agent's (Steven: "proper groupings"), each in the
order they stand in the conversation, with the turn, the agent's note
first, and the line. `<CR>` shows the session here as `:b N` does and
lands through the one `Mark` motion. One walk of a session's blocks for
all its marks (`Output::find_each`), since a listing is derived every
frame it is shown; the copy's table at sync uses the same walk.

**The agent's**: `` `{a-z} `` goes. How the agent makes one (Q4):

- **(a) a tool**, `mark`, with a required note. It marks HERE — where the
  call falls in the transcript, after the text before it — or an earlier
  place, given as a turn and a short quoted phrase that Shannon finds
  there (models quote well; a phrase not found is refused, said).
- **(b) an inline marker** the agent writes in its text. Works on any
  backend, no round trip; but a syntax in prose can collide, and a marker
  the agent repeats from a file it read would mark too.
- **(c) a skill** — the instructions, over (a) or (b).

Backend fact: on the API, Shannon defines the tools. Through Claude Code,
a Shannon tool needs an MCP server Shannon serves to `claude -p` — not
built. Lean: (a), on the API first.

**Its name (Q5).** Lean: Shannon assigns the next unused letter (`a`, `b`,
…) and the tool's result says which, so the summary names a mark that
exists; past 26 the oldest is reused, and `:marks` shows each with its note,
so a stale `` `c `` in an old summary is recoverable. The alternative:
numbers that never repeat (`` `1 ``…`` `9 ``, then a count).

**Every agent mark has a note** — that is how you know what it marks. The
notes feature is deferred for YOUR marks only (Steven: low).

**The system prompt** says what marks are, when to make one (a question
left open for you; a result you will want again; where a plan was agreed),
and to name the mark in the turn's closing words.

**`:marks`** — a live page (Steven: yes): each mark, whose, its turn, its
note, and a preview of what it marks (the line, or the block's folded
line); `<CR>` goes there. It is a quickfix list in all but name — vim's
quickfix window (`:copen`) and location list (`:lopen`, a window's own),
walked by `:cnext` `:cprev`. Our page buffers already have that shape; a
general quickfix list later is a page kind whose rows are places.

**The anchor (Q6).** A mark should point into the CONVERSATION — a turn, a
block of it, an offset in it — and be resolved to a line through the
rendering, so a fold, a rule, a resize or a reload cannot move it.
Persistence wants that anyway (Steven: "save along with the session"). Lean:
that anchor from the first cut, if the transcript can say which turn and
which block a line comes from — to measure before choosing.

**Built 10-27** (`output.rs`): the session is the buffer; a block is a
`BlockId`, minted when the block is made and kept through every change to
its lines; the offset is a line and column in it (`Anchor`); the turn is
the block's, the index `:reply` maps into `full_turns`. Not a block's
index — a record inserted ahead of an empty open block moves it — nor a
turn's — `abandon_turn` gives the next turn the same one. A block that is
gone (popped, truncated) takes its places with it: not set, as vim's E20.
`Output::spans` is the walk from blocks to lines the marks' readers go
through — not yet the only one: `find_quote`, the two windows and the line
counts walk the blocks themselves (`Span`'s doc lists them, review stop
14, A 9); folds and the gutter must agree with the windows. A place is
persisted as the turn, the block's role and place among that turn's
blocks of the role, and the line and column (10-28, below) — the store
keeps turns, not blocks.

**Later**: uppercase marks (Steven: after the soft launch is fine); your
notes (with a quickfix list or bubbles). Persistence was built 10-28.

## 8. The gutter

vim's pieces: `foldcolumn` (depth: `-` open, `+` shut, `|` inside),
`signcolumn` (where marks show as signs), Neovim's `statuscolumn` (one
format for both), `fillchars` (`foldopen`, `foldclose`, `foldsep`).

Ours: one column, default width 1 (padding when it has nothing to say), 0
hides it, wider shows fold depth. Glyphs configurable: Unicode defaults
with an ASCII fallback (`▾` `▸` `│` / `-` `+` `|`), Nerd Font glyphs by
config; a mark glyph per kind — yours, the agent's, one with a note.
Colours by theme roles, each with a rendered-cell test. At width 1 a fold
and a mark can want the same cell (Q7). After the marks milestone (M5),
as Steven placed it.

**Numbers** (Steven, 11-02, S21-Q19: "we'll want relative numbers too …
people use it to calculate how far they need to do something like
`<N>j`"): vim's `'number'` and `'relativenumber'`, as window options —
the window's lines, which is what a count moves, one number a wrapped line
however many rows it takes; with both, the cursor's line absolute and the
rest their distance from it, as vim draws them (`'numberwidth'`, the
`LineNr` and `CursorLineNr` roles). A diff's numbers are its FILE's, in
the record's own text (`diffs.md` §7); these are beside any text, a diff
included.

## 9. Build order (proposal)

- **N1 — Blocks**: the one block query; thinking parsed, stored and shown
  (after reading the API's rules for thinking across a tool round).
- **N2 — Motions**: `]x` `[x` per kind, `zj` `zk`, each a jump.
- **N3 — Your marks**: `marks.md`'s M1–M4, on the conversation anchor.
- **N4 — The agent's marks**: the tool, the system prompt, notes on agent
  marks, `:marks` showing whose.
- **N5 — Folds**: the `z` commands, rules, folded lines, the 400-line cut
  retired, the prompt's folds. **Begun 10-31 (Steven: "these could in fact
  share the same primitives"):** `crates/shannon-tui/src/fold.rs` holds what
  every fold shares — vim's `z` verbs (`zo` `zc` `za` `zR` `zM`) and what
  each does to a fold, and the line's glyph (`+` shut, `-` open, vim's
  `fillchars` defaults). Its first consumer is `:models`' older models;
  the transcript's folds came next (N5a, 11-01) — the WINDOW's, as vim's
  are, and not kept past the run (S21-Q2), so not "per session as marks
  do", as this said; whether a fork carries a window's place is S21-Q4.
- **N6 — The gutter.**
- **Later**: uppercase marks; your notes (persistence built 10-28).

Blocks first, though marks were next: `:marks`' preview, the anchor, the
gutter and folds all read the block structure, and N1 also closes a real
gap (thinking never shown).

## 10. Questions for Steven (my leans first)

- **Q1 — thinking's identity.** Is a turn's thinking part of its
  content-addressed identity (its hash)? Lean: yes — it is what the model
  produced, and a turn without it is a different record — but it changes
  every turn id that has thinking, so it is a store decision. *Restated
  10-27 with §4's facts:* no stored turn has thinking yet, so hashing it
  only when present moves no existing id. What a hash cannot carry is
  whether a block may be SENT: that belongs to the occurrence (its
  prefix), not the content ([[feedback_content_addressed_ids]]). So the
  lean holds with a second fact beside it: each block stored with the
  hash of the prefix it was produced under, and sent only while the
  request's prefix hashes the same. Against: a squashed or merged copy of
  a turn keeps thinking it can never send again (shown, not sent).
- **Q10 — what to ask the API for.** `display: "summarized"` (reasoning
  summaries and the progress updates, mixed, not told apart by type) or
  `"updates"` (the updates alone; reasoning never seen; beta header), as
  a setting either way. Lean: a setting, default `"summarized"` — you
  asked to see thinking — with the fold rule by size (§6: shut past N
  lines), which leaves an update (a sentence or two) open and a
  reasoning summary shut. Unknown: whether a short summary is common
  enough to make that rule leak.
- **Q11 — thinking an edit has made unsendable.** (a) Shannon strips it
  by its own prefix check — it composed the request, so it knows the
  first changed message; (b) send `drop_block` with the beta header and
  let the API drop it, saying so when `input_transformations` lists any;
  (c) both, (a) as the rule and (b) as the defence. Lean: (c); a `:model`
  switch is the exception — the API drops what the new model cannot
  read, unbilled, and the docs say not to strip it yourself.
- **Q2 — the letters.** `]u` `]a` `]t` `]h`, or others?
- **Q3 — rules' shape.** Per kind with a size threshold (`tool = { shut_over
  = 40 }`), or a small expression language like the alerts' (`when = …`)?
  Lean: per kind first; the alert grammar if a second need appears.
- **Q4 — how the agent marks.** Lean (a), the tool, on the API first.
- **Q5 — an agent mark's name.** Lean: assigned letters, oldest reused.
- **Q6 — the anchor.** Lean: into the conversation, from the first cut.
- **Q7 — one cell, two things.** At width 1, a shut fold and a mark on the
  same line: which shows? Lean: the mark (a fold says itself on its line).
- **Q8 — order.** Lean: blocks first (§9).
- **Q9 — "bubbles".** In the docs a bubble is a message's per-role framing;
  did you mean that, or a floating note beside the text (Neovim's virtual
  text, an editor's hover)?

## 11. Answers (Steven, 10-27 evening)

- **NQ1** — not a blocker; he asked what a content hash is and why the
  exact earlier conversation matters (answered in the turn summary). To
  SURFACE when the graph manipulates: an edit, squash or merge says what
  thinking it leaves unsendable.
- **NQ2** — `zj`/`zk` yes (vim's fold motions work on open folds too); `]u`
  dropped (it is `]]`); `]a` kept; `]t` is thinking, his pick; the tool
  call's letter re-asked as **NQ2b** (`]c` proposed, §5). Also `[z`/`]z`,
  and `]b`/`[b` as Neovim's buffer motions (§5).
- **NQ3** — "not sure what you mean": re-explained (turn summary); open.
- **NQ4** — wants the implementation in detail before agreeing: given in
  the turn summary; open.
- **NQ5** — not answered; lean stands.
- **NQ6** — yes, a point in the conversation: "we need to be able to have
  the coordinates" — `(session, turn, block, offset)`.
- **NQ7** — yes, the mark shows. **NQ8** — yes, blocks first.
- **NQ9** — "balloons", not bubbles: vim's balloon / Neovim's floating
  window (`K` over a symbol; a diagnostic beside the text) — his call to
  me, implement or defer, undocumented or experimental allowed. **My
  call: defer** to when a note has a place to appear (your marks' notes,
  which it would carry, are deferred too). The foundation exists unwired:
  `wm::OverlayStack` (floating viewports, `Float` and `Modal`), used by no
  pane yet — a balloon is a `Float` anchored at a conversation coordinate
  (NQ6), dismissed when the cursor leaves it.
- **NQ10** — his lean accepted, "address the unknown". The docs answer
  it: under `"summarized"` a progress update is "Summary text, not
  distinguishable from a reasoning block" (the thinking page's table); no
  summary length is documented. So the fold rule by size is dropped for
  something that needs no telling apart: **every thinking block starts
  shut, and its folded line shows its first line of text** — an update,
  a sentence or two, reads whole folded where it fits the row (else up to
  the row's end); a reasoning summary shows where it starts. Setting: `display`, default `"summarized"`, per model as §4's
  table allows.
- **NQ11** — the lean accepted for now; "let's come back to this".
- Questions go in the turn summary, always, so they cannot be missed.

### Second round (Steven, 10-27 night)

- **Tool calls are `]e` `[e`** ("execution" — his vote, over `]c`); with
  tools there, **`]t` `[t` stay thinking**. `[e`/`]e` are in neither vim's
  `index.txt` nor Neovim 0.12's defaults; vim-unimpaired's "exchange line"
  is an edit, meaningless in a read-only transcript.
- **NQ1 — the lean, with its limits written down** (§4 and here): thinking
  joins a turn's hash when present (no existing id moves); whether a block
  may be SENT is the occurrence's, by a hash of the prefix it was produced
  under. Limits: a turn recomposed by an edit, squash or merge keeps its
  thinking as a record, shown and never sent; every graph surgery says
  what thinking it leaves unsendable; a `:model` switch drops what the new
  model cannot read (the API, unbilled). **Not built** (review stop 15,
  E L10): every kept thought is SENT today (`anthropic.rs` only counts
  them), and the API drops what no longer matches (`drop_block`, said on
  the status line) — the "never sent" half is §4 item 5's check, N1c,
  docketed.
- **Thinking is a CATEGORY, not a per-model property** ("there's a
  category here and we should leverage that"): models that think
  adaptively (4.6 and later: send `{type: "adaptive", display:
  "summarized"}` — on 5.x it is already on, and asking is harmless) and
  models that think only on a budget (4.5 and earlier: `{type: "enabled",
  budget_tokens}`, or no thinking). Two kinds, read off the model's
  generation, not a flag per row of `models.rs`.
- **NQ10's source**, since `:help summarized` is vim's: the API's thinking
  page, `https://platform.claude.com/docs/en/build-with-claude/thinking`,
  section "Progress updates between tool calls" — its table's row for
  `"summarized"` reads "Summary text, not distinguishable from a reasoning
  block"; its "Summarized thinking" section gives no length.
- **NQ3** — the design stands, and **shut regardless of length is
  accepted and is the default**: `thinking = "shut"`, `tool = "shut"`;
  `{ shut_over = N }` the option, not the default. (Read as: tool calls
  start shut whatever their size — confirmed in the third round.)
- **NQ4 — approved; build it**, and "make sure to do proper groupings" in
  `:marks` (yours apart from the agent's, each in turn order). Steven
  will test whether the model marks conversations reliably.
- **NQ5** — approved: assigned letters, the oldest reused past 26.
- **NQ9** — deferred, confirmed.

### Third round (Steven, 10-27, late)

- **NQ3, confirmed**: "Tools and thoughts are default folded. But they can
  be unfolded depending on settings." §6 says so.
- **Read-only is said when you try to type**, as vim's E21 is, not on the
  row (review stop 13, S13-Q1; `status-line.md`). The transcript a reader
  folds and marks in is read-only too; its row is asked about (S14-Q1).

### Fourth round (Steven, 10-30, after review stop 14)

- **S14-Q1** — yes: no shipped row says `(read-only)` (`26d556b`).
- **S16-Q2 — a fork carries its marks, then they diverge** (review stop
  14, A 5): "let's carry them over in forks. But if a user starts deleting
  them in one fork then the removals are not shared. The same will be true
  about folding … Think of it like cloning a person. They would share all
  the same history and memories *up to the point of cloning*. But at that
  point in time they diverge." So a fork, `:clone`, `:vfork` and a branch
  copy the marks (and later the folds) on the turns they keep, as their own;
  from there each session's are its own. `:continue` keeps them (its turns
  are the same). **Built 10-31** (`App::marks_carried`, a turn map per
  maker): `clone_of` (`:clone`, `:fork`, `:vfork`) the stored turns, a
  checkout those up to its turn, an edit's branch its unchanged prefix and
  its suffix moved by the slice's change in length, `adopt_composed`'s
  `carries` — `:continue` all, a squash of a session's own turns those
  outside the range, moved — the left ones said. Unread entries go as
  they were. Not carried: a merge, a pick, a return (beside their sources,
  not made from one).
- **S16-Q1** — the `:marks` colours stay as they are (B F10).
- **S15-Q3** — closed untraced: Steven set `'a` and `'b` himself; why two
  sets showed is not reproduced.

### Fifth round (Steven, 11-01, around review stop 15)

Gathered here at review stop 15 (C L-i); each is quoted where it was
built, too.
- **S21-Q1** — thinking shown now, "Do a first": open until N5, shut since
  N5a (§4 item 4).
- **S21-Q2** — folds are not kept past the run: "Only folds that stay are
  due to settings" (§6).
- **S21-Q3** — a reply's markdown markers stay drawn: "Go ahead with the
  lean for now, I think we will stick to that decision" (TODO, Markdown's
  rest).
- **S21-Q4** — a session made from the one a window reads carries its
  place: "Carry over whole place by position" (§6, `app/carry.rs`).
- **S21-Q5** — a checkout lands at the turn picked: "it is correct to land
  on the turn you are in".
- **S21-Q6** — forks follow `tui.splitright` / `tui.splitbelow`, no
  setting of their own.
- **S21-Q7** — the aim accent on a second window of one session: "leave it
  for now" (his reading of `<C-^>` was right: it shows the window's
  alternate there).
- **S21-Q8** — a checkout reads `at <turn>` until it owns one: "I think
  this works. Still a little confusing but I think people will acclimate".
- **S21-Q9** — `/` keeps Rust regex unless `less` and the like read `^`
  `$` as vim does (measured: `grep` `sed` and vim literal mid-pattern,
  `grep -E` `awk` `rg` anchors; `less` unmeasured here). `:help` key tags,
  a separate half, asked again (S21-Q14).
- **S21-Q10** — the topline class fix before N2: "Approved".
- **S21-Q11** — focus on a second channel, the focused title in reverse
  video: "let's build it. Put it in its own commit".
- **S21-Q12** — a look at a checkout is no loss on quit: agreed.
- **S21-Q13** — a shut fold says what it hides: "Having that extra
  visibility will be a bit nice".

### Sixth round (Steven, 11-01, after review stop 15)

- **S21-Q14** — `:help` finds keys, and `/` names a misplaced anchor:
  "Yes. This is one of the easiest ways for users to get help. They will
  often know the keys but not the name of things… I might do something
  like `:help zM`", both halves. Built: a key table's row answers a bare
  key, before a same-named command (Neovim 0.12.6, measured: `:help w` the
  motion, `:help :w` the command; `ctrl-w` `^w` read as `CTRL-W`; `GG`
  opens `gg`; `<C-w>` it does not read, Shannon does); a search that found
  nothing says what a mid-pattern `^` or `$` matches, and its escape.
- **S21-Q15** — a `:model` or `:effort` set on a look is not something of
  its own: "This kind of action doesn't count until there's a follow-up,
  like a sent message" (as built).
- **S21-Q16** — removed lines' contrast: leans (a), not dimming removed
  text, but asked for a demonstration first; and a syntax-aware diff,
  difftastic's way, optional and on by default — timing Claude's call,
  "we should integrate soon".
- **S21-Q17** — no notice when `<C-^>` hides a fork: "the problem was
  really just me not understanding <C-^>" (F M6 closed, unbuilt).

### Seventh round (Steven, 11-02, after the demonstration)

- **S21-Q16** — after the demonstration in four themes: "I'm aligned with
  you: a" (built: removed text at full contrast), and "while I don't think
  the text is too dim I do think it is hard to read" — the diff itself,
  studied in `diffs.md` from his images 30–37.
- **S21-Q18** — run the differ as a program, not linked: "We'd just need to
  let the user be able to modify what differencer shannon is running. Much
  in the same way git can do that." Design: `diffs.md` §5; its questions
  are S21-Q19..Q21.
