# `shannon tutor` — a design, and notes on Steven's draft

STATUS: **BUILT 11-03** on the leans (Steven, 11-03: "give it your best
go and we'll iterate on it. We can take your leans") — §6 says what was
built and every call made beyond T1–T5. The companion to Steven's draft
(`shannon-tutor.md`, his, in his voice); §4 holds my notes on the draft's
text, §5 the two pieces it asked me to draw. The text ships as
`crates/shannon-tui/src/tutor.txt`. Tags as elsewhere: *verified* (read
in Shannon's source, or measured this session), *inferred*, *my call*.

## 1. What the draft asks

- `shannon tutor`: Shannon as it always looks, but a **tutor pane** where
  the chat would be and the prompt below it, aimed at nothing.
- **The stub**, so no tokens and no risk: "They can experience Shannon for
  free and at no risk to their systems (e.g. the agent doing something
  unintended)." A `:model` away from a real one.
- **Editable like vimtutor**: "I want the user to be able to edit the
  tutor text, but they will not be able to overwrite the files."
- The startup screen names `shannon tutor` and `shannon --resume`.
  (`--resume` BUILT 11-03, `1992538`; the tutor's line comes with it.)
- Lessons 1–3: moving, panes, quitting, modes; talking to the agent,
  layout, splits, tool calls, buffers/tabs, the graph; advanced motions,
  marks, folds, the config, secrets, graph surgery.

## 2. The design

- **Invocation.** `shannon tutor` — `main.rs`'s `Invocation` gains
  `Tutor`, its usage line and `--help` too (*verified*: `Invocation` is an
  enum read by one parser, `main.rs:93`).
- **Its text** ships in the binary (`include_str!`, as `help.rs` does the
  manual), `{VERSION}` filled at launch. One file, so the tutor is edited
  as text and reviewed as text.
- **The stub answers** every session the tutor makes (`:model` changes
  it, as anywhere). Its tool block is how Lesson 2.3 shows a call without
  a model: the stub asks for the call a prompt writes out (F2d, `c3fcd50`,
  `:help shannon-stub`).
- **Where it runs.** A fresh directory of its own (`$TMPDIR/shannon-tutor-…`),
  seeded with a few files the lessons use, as vimtutor works on a copy.
  An approved call then runs THERE: a lesson's `Read`/`Edit`/`Write` and
  its `ls` touch nothing of yours. (T3.)
- **Its sessions** are kept in memory, not in your store, and said so at
  `:qa` — the tutor is a sandpit, and `shannon --resume` should not list
  "Hello World" forever. (T2.)
- **Its prompt is short** (3 rows, not 10): at a 24-row terminal the
  usual layout leaves the tutor pane 11 rows, and the draft wants the
  `h j k l` diagram on the first screen. (T4.)

## 3. Questions (Steven's)

- **T1 — the tutor pane: a page, or an editable buffer?** The draft
  wants vimtutor's "edit the text, never overwrite the file". Today a
  page (`:help`'s kind) is read-only, and the only editor is the prompt
  (`PromptView.draft`, a `prompt::Prompt`, one per prompt window —
  *verified*, `app/window.rs:172`).
  - (a) **A page**, as `:help`. Practice happens in the prompt: "yank
    this line (`yy`), `<C-w>j`, `p`, fix it". Smallest; no new view kind.
    Not vimtutor's.
  - (b) **An editable buffer** — a new view kind holding the same
    editor the prompt uses, on a tutor buffer that is never sent and
    never saved (`:w` says it is a copy). vimtutor's, and the first
    "scratch buffer" Shannon has (a notes pane later, `:enew`). Every
    site that matches on `View` learns it (render, keys, focus, split,
    tabs, `:q`, `:ls`): the larger build, maybe two days with tests.
  - (c) **A prompt window holding the tutor text**, aimed nowhere, its
    send refused. Cheapest editable one, but a prompt that may not send
    is the confusion the window model removed ("each prompt pane has its
    own draft and its own aim").
  - Lean **(b)**, rooting for it: "everything is a buffer" is your
    direction, and vimtutor's edit-in-place is the lesson. (a) if the
    soft release cannot wait two days.
- **T2 — sessions the tutor makes: kept or dropped?** Lean dropped, said
  at exit (§2). Against: someone who switches to a real model in the
  tutor and has a good conversation loses it.
- **T3 — where it runs: a fresh directory, or where you typed it?** Lean
  fresh (§2) — "no risk to their systems". Against: Lesson 3.4's local
  config (`.shannon/`) then lands in a temporary directory.
- **T4 — the tutor's prompt 3 rows?** Lean yes (§2). The prompt bar is
  a fixed height (`[tui.prompt] height`, `workspace.rs:703`, *verified*):
  it does not grow as you type, and `<C-w>+` / `<C-w>-` resize it, which
  a lesson could teach.
- **T5 — the `h j k l` diagram first, or after the intro?** The draft
  asks ("Alternatively, we could put that at the very top. What do you
  think?"). Lean: the welcome banner, ONE sentence and the `:q` line,
  then the diagram — a stuck reader needs to move and to leave before
  anything else; the rest of the intro after Lesson 1.0.

## 4. Notes on the draft

Every concrete claim was checked against the code 11-03 (a read-only
fact-check: about 100 claims, most TRUE; the key corrections re-read in
source by me). What is wrong, by lesson — the fix in the draft's own
terms, its voice left to you:

- **Intro — "leave it at any time by typing `:q`".** In the prompt, `:q`
  is refused while other panes are open ("this is the only prompt —
  closing it would leave nowhere to type", `app.rs:14503`); the splash
  says `:qa` for exactly this reason. Lesson 1.3 likewise: `:qa`, and
  `:qa!` past a warning (`:q!` takes `:q`'s refusal there).
- **1.0 — `:set tui.mouse = false`.** `:set` splits on spaces, so it is
  refused; `:set notui.mouse` is the spelling. Mouse capture is read at
  launch, though: off for good is `mouse = false` under `[tui]` in the
  config, then relaunch.
- **1.4 — `<C-[>` is the same as `<Esc>`.** TRUE where it reached you
  (Steven, 11-03: works in ghostty + tmux; it is one byte in the legacy
  encoding, as he said). Where the terminal speaks the kitty protocol
  Shannon asks for, `Ctrl-[` arrived as its own key and did nothing; now
  read as `<Esc>` wherever it comes from (`shortcuts::as_vim_reads`,
  11-03). The lesson can promise it. "on unix systems" can go: it is the
  terminal's, on any OS.
- **2.0 — `<C-CR>` sends.** Only where the terminal speaks the kitty
  protocol; elsewhere Ctrl-Enter is a plain Enter, a newline in Insert.
  tmux does not pass it (measured 11-10, tmux 3.8, `send-keys C-Enter`):
  its `extended-keys` encodes Ctrl-Enter only for a program that asks for
  modifyOtherKeys, and Shannon asks for the kitty protocol alone. The
  Normal-mode `<CR>` always works: lead with it. `<M-CR>` submits from
  Insert through tmux; it is in `:help submit`, not the lesson (Steven,
  11-10: the manual is enough).
- **2.1 — the title `prompt -> <id-1>`.** It is `prompt → <id>`, shown
  only when the tab has two transcripts (or the aim is off screen).
- **2.2 — `:vsp` then `:new`.** The new window opens on the LEFT and takes
  focus (vim's `splitright` off), showing what the split pane showed; so
  the chat ends up left of the tutor, not right as the diagram draws it.
- **2.3 — "While you can use auto-mode in Shannon".** There is no auto
  mode. The nearest are rules: `A` in the pane, `:allow`, `[permissions]
  rules`. "turn all themes off": there is no such switch; the shipped
  `terminal` theme wears your terminal's own colours. And an approved call
  really runs (no sandbox): the lesson's command should be harmless, or
  the tutor should run in its own directory (T3).
- **2.4 — `:ls` shows two buffers.** `:ls` lists sessions; a page (the
  tutor, under T1 (a)) shows only under `:ls!`. "run `:new` a few times":
  `:new` on a blank session that one pane shows stays on it — send
  something in each first. `:tabnew ball` is `:tab ball`.
- **2.5 — "(this breaks caching)".** Unsettled: a checkout keeps the
  history before it, so a cache entry still warm can serve it; an edit
  does break it from the edited turn on. I would drop the parenthesis or
  say "may". `:resume <name>` opens that session; bare, the graph.
- **3.1 — next/previous tool call, thinking block (left blank).** Not
  built: N2's `]e`/`[e` and `]t`/`[t` (`navigation.md`). Either the
  release takes N2, or those two rows go. "`h e l o A I i m a 0 y r u ?`
  we got them all": true in the prompt; the transcript refuses the
  editing ones.
- **3.2 — 52 marks, a–z and A–Z.** Only `a`–`z` (uppercase after the soft
  launch, your call 10-26). A mark is a place (turn, block, column), not
  a word, and only in a transcript. The agent marks only on Shannon's own
  backend (Claude Code sessions have no `mark` tool).
- **3.3 — `zf/zF`, `zd/zD`, `zE`.** None exist. What does: `zo`/`zO`,
  `zc`/`zC` (on a reply or your message `zc` makes the fold, 11-03),
  `za`/`zA`, `zR`, `zM`. "Claude, please put those instructions here":

      [tui.fold]
      thinking = "shut"     # "shut", "open", "manual", or { shut_over = N }
      tool = "shut"         #   shut when it holds more than N lines
      reply = "manual"      # "manual": no fold until zc makes one
      message = "manual"

  and `:set tui.fold.tool=open` shows at once (`:help shannon-folds`).
- **3.4 — the config.** The user file is
  `$XDG_CONFIG_HOME/shannon/config.toml` (`~/.config/shannon/config.toml`),
  not `$XDG_CONFIG_DIR/…/shannon.toml`; the local one is
  `.shannon/config.toml` where Shannon started, read only once you trust
  the directory (`:trust .`). `:config save` writes the PROJECT's file
  (refused untrusted, `:config! save` forces); nothing saves a session's
  settings into your user file, and `:config save local` does not exist.
  `:theme` does not exist (`:themes`, `tui.theme`). `:config edit` checks
  the file on save but applies nothing: `:reload` does.
- **3.5 — secrets.** `:unsecret` is `:nosecret <handle|value>`. `:alias
  <text>` makes nothing: `:alias` and `:secrets` both open the table; a
  plain alias is a `[[alias]]` in the config (`{{a:repo}}`). A handle is
  `{{<kind>.<4 hex>}}` — `{{secret.1a2b}}`, or `{{github-token.…}}` for a
  kind it recognises.
- **3.6 — graphs.** "a Write-Ahead-Log (WAL)": each session is one
  append-only JSONL file; "WAL" is only the proposed SQLite store's word.
  No rebase. `:fork` clones the session into a new pane (a sibling sharing
  all its history), not a branch at a past turn — `<CR>` on a turn in
  `:graph` is that. `:merge` splices a branch back into the session it
  forked from. `:squash` takes no id (the marks or the selection say
  what). `:distill!` is not "do it yourself": its `!` flips the review
  of the model's draft; writing it yourself is `:squash`. "for full
  functionality you need to be using the API": the only API-only piece
  found is the agent's `mark`.

Your two inline questions: **the year** — vim has had split windows since
Vim 3.0, August 1994 (stacked; side by side from Vim 6.0, 2001, from
memory) and tab pages since Vim 7.0, May 2006 (vim's own `version7.txt`
names `new-tab-pages`). **Forcing a tool call with the stub** — yes, F2d,
built for this: a ```` ```tool bash ```` block (§2, §5).

## 5. The two pieces it asked for

Both drawn by the program itself (the test harness's frame, 100 columns,
11-03), so they are what Shannon shows, not what I think it shows. Colour
is lost here: in the pane each program, flag, path and redirect has its
role's hue, and the verdict line its own.

**`<INSERT OUR VARIANT>`** (Lesson 2.3) — the draft's command, its paths
shortened to `/tmp/scratchpad` as asked. Drawing it found a defect, now
fixed (`99dd02a`): the `awk`'s `> file` had no break to take and wrapped
to the pane's first column.

```
┌ run this? · 7548fe39 ──────────────────────────────────────────────────────────────────────────2─┐
│  ASK   you'll be prompted first                                                                  │
│                                                                                                  │
│  mkdir -p /tmp/scratchpad/permtest \                                                             │
│    && awk 'BEGIN { for (i = 1; i <= 5; i++) printf "%d,%d,item-%02d\n", i, i*i, i }' \           │
│           > /tmp/scratchpad/permtest/data.csv \                                                  │
│    && sort --field-separator=, --key=2 --numeric-sort \                                          │
│           --reverse /tmp/scratchpad/permtest/data.csv \                                          │
│      | awk -F, '{ sum += $2; printf "%-8s %4d running=%d\n", $3, $2, sum }' \                    │
│      | tee /tmp/scratchpad/permtest/report.txt \                                                 │
│      | wc --lines \                                                                              │
│    && echo "---" \                                                                               │
│    && sed 's/item-/ITEM_/' /tmp/scratchpad/permtest/report.txt \                                 │
│      | tr '[:lower:]' '[:upper:]' \                                                              │
│      | head --lines=3 \                                                                          │
│    && date '+done at %Y-%m-%dT%H:%M:%S'                                                          │
│                                                                                                  │
│                                                                                        ↓ 16 more │
│  answer: d/q deny · a allow once · e edit · j/k scroll · A always, this session: allow mkdir /tmp│
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Below the fold (`↓ 16 more`) the report says, per stage, why it asks.
In the tutor the reader would get here by sending the stub a block
(F2d), so the lesson can say: copy this into the prompt and send it.

    ```tool bash
    mkdir -p /tmp/scratchpad/permtest && awk ... | head --lines=3
    ```

**The graph, with the inspector** (Lesson 2.5): two turns, then `:fork`
and a turn on the branch; `:graph`, then `l` `j` `l` to unfold a turn.
The fork shares the root's first turn (`clone ← 39f303f1`).

```
┌ graph ───────────────────────────────────────1─┐┌ inspect ─────────────────────────────────────2─┐
│  ● tour e946fb37  (root)                       ││Message                                         │
│  ╰● 39f303f1  What does main.rs do?            ││  session  5904e40f                             │
│   │ you: What does main.rs do?                 ││     turn  2/3                                  │
│   │ reply: It reads the flags and starts the lo││     role  user                                 │
│▶  ├● 5904e40f  (HEAD, clone ← 39f303f1)        ││    index  0                                    │
│   │╰● 39f303f1  What does main.rs do?  ⋯       ││   length  13 chars · ~3 tok                    │
│   │ ● 678954fc  And the loop?                  ││                                                │
│   │ │ you: And the loop?                       ││And the loop?                                   │
│   │ │ reply: It reads keys and draws frames.   ││~                                               │
│   │ ● fed571cd  Explain the loop again, briefly││~                                               │
│   ● 678954fc  And the loop?  ⋯                 ││~                                               │
└────────────────────────────────────────────────┘└────────────────────────────────────────────────┘
```

The Claude Code pane in Lesson 2.3 needs your screenshot ("Remind me and
I'll drop you a screenshot") — reminding.

## 6. As built (11-03)

- **T1 (b), as vim's `'modifiable'`.** The tutor is a page
  (`PageKind::Tutor`, `Recipe::Tutor`) whose text you may change — so a
  split copies it, `:new` sets it aside and `:b N` brings it back (`:ls`
  lists it, `b1`, since 11-09; below), as Lesson 2.4 teaches. Its text and its undo are the
  BUFFER's (one `Prompt` editor on the `PageBuffer`), its cursor each
  window's: an edit lends the
  window's buffer to the editor (`Prompt::lent`), and every other window
  takes the text at its next sync. It takes the prompt's keymap (Insert,
  operators, `u`); `<CR>` there is refused ("the tutor is never sent");
  a yank there is a `p` in a prompt. Nothing writes it.
- **T2 dropped**: a `MemoryStore`, said at launch.
- **T3 fresh**: `$TMPDIR/shannon-tutor-<pid>-<time>`, 0700, seeded with
  `notes.txt`; entered before anything reads where Shannon started.
  Left behind at exit for the OS to clear (deleting a directory on exit
  is a destructive act the tutor does not need).
- **T4**: a 3-row prompt. **T5**: the banner, one line on moving and
  leaving, the diagram, then the intro.
- *My call*: **the tutor reads none of your config files** — vimtutor's
  `vim -u NONE`, so the lessons describe the Shannon you get; and because
  `:reload` lays your file OVER every layer, a `[tui] model` of yours
  would have put a real model behind the lessons' "send this". Its own
  settings are a layer of their own (`Origin::Tutor`): over the defaults,
  under a `:set`, never written by `:config save`. `:reload` and `:config
  edit` there say why they have nothing. Cost: your theme and keymap are
  not the tutor's.
- *My call*: **no `:split tutor`** — the tutor is opened by `shannon
  tutor` only, where the stub answers.
- *My call*, on the text: a practice lesson added (1.4 DELETING AND
  UNDOING: `x` `dw` `dd` `u` `<C-r>` `p`; `.` out, unbuilt), since the
  page now takes edits; the lessons renumbered 1.0–1.4 (the draft skipped
  1.1); 3.1's tool-call and thinking-block rows out until N2; 2.3's
  command made portable (`wc -l`, `head -n 3`: BSD's have no
  `--lines`) and relative (it runs in the tutor's directory); every §4
  correction applied in the draft's voice; a closing "THAT'S THE TOUR".
- **Found building it**: the tutor opens at launch, outside any event, so
  the first key's end took the focus move for its own and dropped a chord
  that key began (`gg` as your first keys did nothing). `open_tutor`
  says its move (`focus_moved`). `--resume` opens `:graph` the same way
  and has the same latent drop — docketed (TODO.md, soft release).
- **Review stop 16** (A–C, 11-03; `docs/Reviews/2026-11-03-stop16/`):
  - *Isolation, held.* What launch chooses is one value, `tutor::Launch`:
    the directory, the config layer, a `MemoryStore`, `NoVault`, a trust
    list loaded from nothing, Claude Code sessions kept in memory, no
    memory or skills read from your config directory or home, and no
    scratch directory. `main` takes it whole (`Chosen`, one function with
    an arm per launch). D 1 found the last three undecided: every tutor
    run made `projects/<its directory>/memory` in your config tree. It is
    tested beside smoke step 23. A practice `:secret` or `:trust .` keeps
    nothing of yours, and a command runs in the tutor's directory. The
    directory is never one that exists (`make`, `seed`). Shannon's own
    paths stay guarded, as in any launch.
  - *No project above it* (E 4, measured): the walk for memory, skills and
    `AGENTS.md` climbed from the tutor's directory to the nearest ancestor
    holding a `.git`. On Linux that is under `/tmp`, where any account can
    make one, and each place found was a read allowed unasked. The tutor's
    settings mark no project root (`[memory] project_root = []`), so the
    walk is its directory alone.
  - *Open, for F (E 9):* `:model` to a real model in the tutor runs
    without your `[permissions]` and `[secrets]` rules (the tutor reads no
    config of yours), and nothing says so. Your rules don't apply, and the
    defaults still ask for anything outside.
  - *An edit's window keeps the version it made*, so `Ais line` types in
    order. Every other window follows its text through the edit (the
    page's `Shift`s), not by row.
  - *vim's rules, measured in nvim:*
    - a click in Insert lands past a line's end;
    - an arrow or a click ends an insert's count;
    - `:b N` back puts the cursor where you were at once
      (`TextView::reveal`), so a key before the frame edits there.
  - *Said, not silent:* `<C-x><C-e>` (it opens a prompt's draft) and an
    operator over `H` `M` `L`.
  - *Undo is bounded by bytes* (32 MiB) as well as by count: a pasted page
    held 500 copies of itself.
- **Its pictures** (review stop 17): the decision pane is drawn by the pane
  itself, at a `{DECISION}` marker under the command it asks
  (`tutor::Pictures`, Steven: "We should be rendering just as shannon's
  would"). The drawing of `:graph` beside its inspector is painted as
  `:graph` paints, approximately, by the shapes it draws: the panes'
  borders, the tree's glyphs in lane colours two columns a lane, a
  session's id and decoration dim, a message dim, the `HEAD` row bold, the
  inspector's heading bold and its keys dim (`tutor::graph_drawing`;
  Steven, 11-09: "An approximation is fine. We don't need to load a real
  graph"). Every other drawing is plain, and a row you change is painted
  as far as it goes.
- **`b1`, and no chat until called** (Steven's run, 11-09: "`:ls` then
  `:b1` I go to a new conversation rather than back to the tutor ... Show
  the tutor in `:ls` and I don't think we should open a chat until one is
  actually called"). The tutor is `b1`, as vimtutor's file is buffer 1: a
  launch says what it opens on (`Opens::Tutor`), and its number is held
  for the page (`Buffers::add_as`). `:ls` lists a page you edit, as vim
  lists a file and not its help (`Buffer::listed`), and every session.
  The tutor's window is its own, over nothing, and the tutor's `:q` is the
  last window's, which leaves, as vimtutor's does (`App::last_window`,
  S11).
- **No session until one is called** (11-11). The launch built a session
  anyway, `b2`, and `:ls` kept it out (`App::called`: blank, unnamed, in no
  window) — Steven: "That is a secret buffer. It is hidden." So the launch
  makes NONE (`Buffers::new`): `current` and the prompt name the lessons,
  `b1`, and nothing is aimed at — the case the code had as error-catching
  only (the status row's red "no agent", Steven, 10-20). That case is
  handled now, not guarded: every read of the aim's session is an
  `Option` (`aimed_session`, `session_for_pane`, `foreground_session`), a
  session command refuses with one wording (`no_agent`), a prompt is not
  sent and stays (Steven: "it can just not accept prompts"), `:models`,
  `:model` and `:effort` refuse ("refuse"), and the panes draw — the row
  says "no agent" in red. `:new`, `:vnew` or `:snew` makes the first
  session. `called` went with the hidden session: no session is kept out
  of `:ls`. A test runs every command in the table in the tutor with no
  session, then draws a frame (`no_command_panics_in_the_tutor_before_a_
  session`).
