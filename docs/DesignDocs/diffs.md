# Diffs — how a change to a file is drawn, and who chooses

STATUS (2026-11-02): DESIGN, asked (§7). Built: S21-Q16 (a) only — an
edit's removed text drawn at full contrast, not faint. Not built: the rest.
Written after Steven's images 30–37 (11-02), his answers S21-Q16 (a) and
S21-Q18 (run the differ as a program the user names, as git does), and
measurements of difft 0.72.0 and Neovim 0.12.6 (§4).

## 1. The problem, in Steven's words

- 11-01: "I'd like an option to have the diff method appear like
  difftastic … It is much more readable. I'd like that to be optional but
  default on. Difftastic's utility is that it understands syntax … It would
  be a great example of a plugin to integrate."
- 11-02, on the demo of today's record: "while I don't think the text is
  too dim I do think it is hard to read. The highlighting of green and red
  helps."
- "Study these carefully. Neither git's nor difftastics is 'perfect'. But
  look carefully to see why difftastic is so popular. How it helps users
  actually read and understand what is being changed. How the space on the
  screen is being used to give that message."
- "People will have different opinions and a goal of ours is to allow
  people to use what works for them. So options are important here. Some
  people will want your plain style, some git's, and some difftastic, or
  maybe even another. The most important part is that we're able to change
  the way we do things … Our job isn't to dictate preference, it is to
  allow the user to be able to express theirs."
- S21-Q18: "We'd just need to let the user be able to modify what
  differencer shannon is running. Much in the same way git can do that."

## 2. Four drawings of a change (images 30–37)

Claude Code drawing an edit of `help.rs` (30); Shannon's record today (31,
the S21-Q16 demo); `git show` of 8b0c350 through Steven's pager,
diff-so-fancy (32–34 — `core.pager` is `diff-so-fancy | bat …`, so these
are its drawing, not git's own); and `git -c diff.external=difft show` of
the same commit (35–37).

| | Claude Code | Shannon today | diff-so-fancy | difftastic |
|---|---|---|---|---|
| unit of change | the line | the whole edit | the line, its changed segments marked | the syntax token; the word inside a comment or text |
| old vs new told by | marker and background | marker | colour alone (it strips `+`/`-`) | side, and colour |
| coloured | each changed line, its whole width, over syntax colours | the marker; the text in syntax colours | changed lines' text; changed segments on a background | the changed tokens only; unchanged code nearly plain |
| compared | down: old rows, then new | down: all the old text, then all the new | down | across: old and new on one row, aligned, blank filler opposite an insertion |
| a reflow | removed and added whole (30: all three hunks are rustfmt reflows — `+18 -5`, no token changed) | — | removed and added whole | unchanged (§4) |
| where in the file | a line number, one gutter; `(+18 -5)` | nowhere | `path:line` and the enclosing scope, `@ … @ impl App {` | both numbers; a changed line's red/green and bold, the rest dim |
| context | 3 lines | none | 3 lines | 3 lines |
| width | full | full | full | halved — but a hunk changed on one side only is one column (36, `help.rs` 2/7: `.. 257`) |
| a long line | continues at the block's edge, led by its marker: `-very"),` reads as code | continues at column 0 | continues at column 0 (34) | wraps inside its half, `..` in the gutter (37) |
| when it cannot | — | — | — | says so in the header — `Text (1.2 MiB exceeded DFT_BYTE_LIMIT)` (35) — and diffs words within lines |

## 3. Why difftastic reads well

The reader's question is *what changed?*, and the answer is usually a few
tokens. Each of difftastic's choices protects that answer from everything
else on the screen:

1. **Colour is spent on change.** Unchanged code gets almost none: strings
   magenta, comments italic blue, keywords and types bold, identifiers plain
   (`src/display/style.rs`, `color_positions`). Red and green are the only
   loud colours on screen and they land only where something changed. The
   others spend colour on syntax first and whole lines second, and the
   change competes with both.
2. **Compare across, not down.** Old and new sit on one row, aligned, with a
   blank opposite an insertion: the eye moves sideways between two lines a
   few cells apart. The other three ask the reader to hold an old line in
   memory while finding its new one rows below. Shannon's today is the
   extreme case: the whole old text, then the whole new.
3. **Structure decides what changed, so the change is small.** A reflow, a
   re-indent, a list renumbered (`/// 4.` → `/// 5.`, image 36: only the
   digit) changes no token. A line differ reports each as lines removed and
   added — image 30 is nothing else — and that noise is what keeps the line
   differs from doing 1 and 2.
4. **The layout fits the hunk.** A hunk changed on one side only is drawn
   in one column with both numbers: halving the width would buy no
   comparison there, so the width is not spent (`side_by_side.rs:630-699`,
   decided per hunk).
5. **The gutter indexes the change.** A changed line's number is red or
   green and bold, an unchanged one dim (`apply_line_number_color`), so the
   gutter alone finds the changes; a wrapped line's continuation says `..`
   there, never in the code.
6. **It degrades out loud.** Past its size limit it names the limit in the
   header, and still marks words within lines.

Where it falls short — Steven: neither is perfect:

- Halving costs prose: image 37's markdown table rows wrap three and four
  rows a side.
- No enclosing scope: diff-so-fancy's `@ app.rs:18398 @ impl App {` says
  where you are; difftastic's header says `1/7`.
- Its JSON — what Shannon would read — flattens its kinds of change (§4).
- Whole files are slow: 0.67 s on `help.rs` (§4).

What the others do better: diff-so-fancy's scope line and full-width prose;
Claude Code's count in the header; and Shannon's today shows exactly the
strings the model sent — the call, which is what the approval pane is
approving.

**A rule from the comparison: old and new need a channel besides colour.**
difftastic's is the side, Claude Code's and Shannon's the marker.
diff-so-fancy strips the marker, so to a red–green colour-blind reader, or
in monochrome, its old and new lines are one colour. (S21-Q11 gave focus a
second channel for the same reason.)

## 4. Measured (11-02)

- difft 0.72.0 (`/opt/homebrew/bin/difft`); its source, a local build
  (00e2173). Neovim 0.12.6.
- `DFT_UNSTABLE=yes difft --display json OLD NEW` prints `{aligned_lines,
  chunks, language, path, status}`. `aligned_lines` pairs every line,
  `[lhs | null, rhs | null]`, 0-based. A chunk is a hunk's changed lines,
  each `{lhs, rhs}: {line_number, changes: [{start, end, content,
  highlight}]}`, `start`/`end` byte offsets in the line, `highlight` one of
  `delimiter normal string type comment keyword tree_sitter_error`. Only
  novel tokens are listed (`json.rs:335`); a created or deleted file has no
  chunks (`json.rs:85-97`); the format is unstable by its own flag.
- **The flattening.** `changes` holds every match that `is_novel()` —
  `Novel`, `NovelWord` and `UnchangedPartOfNovelItem` alike
  (`syntax.rs:717`). Probe (`scratchpad/difft_kinds`): a doc comment edited
  from "…, and every `:command`." to "…, every `:command`, and every key."
  lists every word of the comment on both sides, so the JSON cannot say
  which words are new — the ones difft's own display underlines (image 36).
- **The reflow.** The same probe's `head.eq_ignore_ascii_case(prefix)
  .then(…)` split over two lines comes back as aligned context (`[null,
  3]`) and no chunk.
- **Time**, best of 5, the process included (`scratchpad/caret/
  difft_time.py`): a 156-byte edit 27 ms; `help.rs` whole (341 KB) 669 ms,
  7 chunks; `app.rs` (1.2 MiB) 58 ms — over `DFT_BYTE_LIMIT`, a text diff.
- **Neovim** keeps the two halves apart: `'diffexpr'` names a program that
  writes an "ed-style or unified-style" diff, and is "Disallowed in
  |modeline|"; `'diffopt'` (default `internal,filler,closeoff,
  indent-heuristic,inline:char,linematch:40`) is vim's drawing of it —
  `inline:{none,simple,char,word}`, `context:{n}`, `filler`, `vertical`,
  `algorithm:{myers,minimal,patience,histogram}`, `iwhite`, `linematch:{n}`.
- **Shannon** draws a `Change` through one door in two places: the
  transcript's record (`tool_record.rs`, `Record::change`) and the approval
  pane (`permission_view.rs`, `change_lines`, which calls it). The record
  holds the strings only (`Change::Content`, `Change::Edits`): no file, so
  no line numbers and no context beyond what the model put in the edit.
  Its wrap (`wrap.rs`) has no hanging indent: a long `  + …` line continues
  at column 0.

## 5. Design: what changed, then how it is drawn

Two programs answer two questions. A **differ** says what changed; a
**layout** draws it. Vim keeps them apart: `'diffexpr'` is any program, and
the windows, filler and inline marks are vim's own. Git lets one program
do both: `diff.external` prints its own drawing. Shannon takes vim's split
as the base, because then the drawing wraps to the pane, takes the theme,
folds by hunk, and is text `/` can search; and git's as the escape hatch
(D4). The differ is the first seam of its kind — a program the user names,
read in a format Shannon knows — for the plugin system `design.md` §5
defers.

### 5.1 The value between them

```
Diff { rows: Vec<Row>, hunks: Vec<Range<usize>>, note: Option<String> }
Row  { old: Option<Side>, new: Option<Side> }   // aligned; None opposite an insertion
Side { line: usize, spans: Vec<(Range<usize>, Novel)> }
enum Novel { Token, Word, Rest }   // difftastic's Novel, NovelWord, UnchangedPartOfNovelItem
```

A line is an index into the old or new text the record holds; the file's
numbers are those plus where the edit starts, when that is known
(S21-Q19). The value knows no width and no colour, so a record computes it
once and keeps it.

### 5.2 Differs — vim's `'diffexpr'`

- **built-in** — a line diff, then within each pair of changed lines a word
  diff (vim's `linematch` pairs them, `inline:word` marks them). In-process,
  so it is always there: the floor every other differ falls back to. A crate
  or our own, measured before D1 and justified in that commit (AGENTS.md:
  dependencies need it).
- **difft** — its JSON into rows. The words of a changed comment or string
  are recovered by the built-in word diff, which the JSON cannot say (§4,
  the flattening). On when `difft` is installed — reading Steven's "default
  on" as "used when present" (§7).
- **a program writing a unified diff** — `diff -u`, `git diff --no-index
  --histogram`, anything that speaks the lingua franca, as vim's
  `'diffexpr'` does. Parsed into rows, the built-in word pass on top.
- (D4) **a program that draws** — git's `diff.external`: delta,
  diff-so-fancy, difft's own display. Its output shown as it drew it, kept
  to SGR (§5.5), run again when the width changes. Shannon loses the
  layout, the theme and the folds there: the price of a program that draws
  (S21-Q21).

A differ runs off the frame, once per record: 27 ms is many frames at the
rate Shannon aims for. A reopened session diffs its records in the
background, nearest the view first. The answers are cached by (differ,
version, old, new), owner-only: derived from text the transcript already
holds, so nothing is stored that was not stored before. How a live record
waits for its diff without moving the view (`feedback_no_teleports`) is
D2's to measure.

A differ that fails — missing, a non-zero exit, output that does not parse,
past its time or size bound — falls back to the built-in, and says so once:
`difft: <why>; drawn with the built-in diff`. A default that finds no
`difft` installed is not a failure, and says nothing.

### 5.3 Layouts — vim's `'diffopt'`

- **unified** — one column: `-` and `+` rows, changed words marked
  (`inline`), `context` lines around each hunk, the gutter's numbers when
  known. It does not depend on the width, so the view's wrap draws it — with
  a hanging indent under the code (vim's `breakindent`, `showbreak`), never
  a marker where code is read.
- **split** — difftastic's: old left, new right, aligned, filler, each half
  wrapped in its own column with `..` in the gutter. It depends on the
  width: a row is one line whose two halves wrap apart, which the view
  cannot draw yet. After S21-Q10 makes a window's top a line and column (D3).
- **auto** — difftastic's per-hunk rule: split where both sides changed and
  each half has room, else unified. The default once D3 lands; unified
  until then.
- **as-sent** — today's: the edit's old text, then its new, the arguments
  exactly as the model sent them — the call rather than the change.

Settings, named after vim's where vim has the thing:

```toml
[tui.diff]
program = "difft"        # "builtin", or a command: ["git", "diff", "--no-index", "--histogram"]
format  = "difft-json"   # "unified"; ("ansi", D4)
layout  = "auto"         # "unified" | "split" | "as-sent"
inline  = "word"         # vim's inline: "none" | "char" | "word"
context = 3              # vim's context:{n}
number  = true           # the gutter, where the edit's place is known
```

### 5.4 Colour

§3's first point, as theme roles, so each theme's author decides how loud:
`diff.removed` and `diff.added` (a changed line's marker and number),
`diff.removed.word` and `diff.added.word` (a changed token: red or green and
bold by default, difftastic's), `diff.gutter` (an unchanged line's number,
from `ui.text.faint`). A layout draws syntax colours on unchanged tokens
and a role on changed ones. The approval pane already keeps a diff's syntax
off the verdict's hues (`change_lines`' doc): the same idea, one step on.

### 5.5 Who may set the program

A differ is a program Shannon runs, so it is the user's alone: `[tui.diff]
program` and `format` join `YOURS_ONLY`, and a project's
`.shannon/config.toml` cannot set them — vim's "Disallowed in |modeline|".
`program = "difft"` resolves on the user's `PATH` once, at load, to the
path it names, and `:set tui.diff.program?` says which. The program gets
two temp files — `O_EXCL`, 0600, in Shannon's scratch dir — holding the
strings the model sent and nothing more. Its stdout is read to a bound; a
drawing program's (D4) is kept to SGR, every other escape dropped
(`feedback_sink_floor_is_its_own`). No test runs a real one
(`feedback_sandbox_before_seam`).

## 6. Phases

- **D1** — `Diff`, the built-in differ, the unified layout with words
  marked, the as-sent layout, the theme roles, the hanging indent.
  `Record::change` lays it out, so the approval pane follows. Nothing
  depends on the width: no view change.
- **D2** — differs as programs: `difft`'s JSON, a unified diff; off the
  frame, the cache, the fallback's note, `YOURS_ONLY`.
- **D3** — split and auto, after S21-Q10.
- **D4** — a program that draws (S21-Q21: build it).
- The edit's place in its file (S21-Q19 (a)): the hunks' starts from Claude
  Code's `tool_use_result`, as the record closes; numbers in its gutter.
  The window's `'number'` and `'relativenumber'` are N6's.

Order (Claude's call, 11-01, kept): S21-Q10's four phases, then D1–D3, then
N2 on Steven's go. D1–D2 do not need S21-Q10; D3 does.

## 7. Questions (11-02) — answered 11-02 night

- **S21-Q19 — line numbers: (a)**, and relative numbers too (Steven: "a
  very popular feature in vim because people use it to calculate how far
  they need to do something like `<N>j`"). Two numbers, two jobs: the
  FILE's, in the record's own gutter (where the line is in its file), and
  the WINDOW's — vim's `'number'` and `'relativenumber'`, counting the
  transcript's lines, which is what `<N>j` moves — in the window's gutter
  (`navigation.md` §8, N6). vim shows the second beside any text, a diff's
  numbers included; so does this.

  **The unknown, resolved by the Agent SDK's reference:** the stream
  carries each tool's result object — a user message's `tool_use_result`
  is the tool's structured output, not the text the model was sent;
  Shannon's own fixture has one for a Bash (`claude_code.rs`,
  `{"stdout":"hi",…}`), and reads none. An Edit's and a Write's output
  hold `structuredPatch` — hunks `{oldStart, oldLines, newStart, newLines,
  lines}` — and `originalFile`, the WHOLE file before. So the
  file's numbers come with the result, no read of Shannon's, for every
  Edit and Write there is: only Claude Code's backend has those tools
  (Shannon's own offers `bash`). Kept: the hunks' starts — positions —
  never `lines` or `originalFile`, which hold file text the model may not
  have read (`project_shannon_secrets`). The numbers arrive when the call
  ends, so the record gains them as it closes in place — the second
  consumer S21-Q10 phase 2 waits for. The pane, before the call runs, has
  none (the gate's own read, (a) as asked, if they are wanted there).
  Unverified: that the `claude` installed here sends it (a live probe).
- **S21-Q20 — a Write over an existing file: (a) now, (c) later** (Steven:
  "I'm with your lean"). He asked how a write happens without a read. In
  Claude Code, whether a Write over an existing file needs it read first
  depends on the model (its tools reference, checked 11-05: older models
  always; newer ones not, where the read would need no prompt; a file read
  only in part, under a `PARTIAL view` notice, always) — and a write by
  shell reads nothing: `cat > f <<'EOF'`, `tee`,
  `sed -i`, `>>` — images 44 and 49, where the model wrote and edited
  `test.py` with Bash and the Write and Edit records never ran. Append is
  one case of it, not the only one. `originalFile` (above) is the whole old
  file, live, for a Write — what (b) wanted; kept for the frame, never
  stored, it would make live and reopened read differently again.
- **S21-Q21 — a program that draws (D4): build it** (Steven: "We'll test it
  out and iterate if we need to"), last of the four.
- **difft by default** — fine as long as it is documented, including where
  the setting is (Steven): `:help` says difft is used when installed, and
  that `[tui.diff] program` in `config.toml` (`:config edit`) chooses
  another or `"builtin"`; `:set tui.diff.program?` says which ran.
