*shannon-split-panes.txt*   Decisions and prompts in split panes

                SHANNON CODE — ONE ASK PER CONVERSATION

STATUS: PROPOSAL, for discussion with Steven (written 2026-09-30). Only stage
0 (§5) is built: the decision opens under the session that asked. Nothing
else below is built. Steven's design notes are in `TODO.md` § "Decisions and
prompts in split panes". The site list came from a read-only mapping pass
over `app.rs`, `workspace.rs` and `view.rs`. The claims this proposal leans
on were checked by hand: the anchor, the global arm, `Scope::Global`, and
the manual's send section.

UPDATE 2026-10-26: §2 is the 09-30 map, kept as it was. Its "PROMPT AIM IS
ONE VALUE" ended 10-26: each prompt window owns its draft and its aim
(`View::Prompt(PromptView { draft, aim })`), §4 Q4 and Q5 answered below;
the model as built is in `tabs.md` (the "Confirmed (10-26)" prompt-model
entry).

==============================================================================
1. WHAT STEVEN ASKED FOR                                  *split-panes-ask*

- The ask appears WITHIN the chat pane it belongs to, as a sub-pane sized
  with the prompt's primitives (a height; static, or dynamic with min/max).
- Each chat's ask is independent: one session's pending question never
  blocks another's turn. A session in no pane holds, as child jobs do.
- Prompt aim is a setting, `auto` (follows the last chat pane visited) or
  `manual`, and it can always be overridden per prompt pane. Vim's
  `:setlocal` is the precedent. In Steven's walk (bottom-left → top-left →
  bottom-left → up → right (new chat) → down (new prompt)), one shared aim
  moved BOTH prompt panes.

==============================================================================
2. WHAT IS THERE NOW                                      *split-panes-now*

ONE DECISION FOR THE WHOLE APP. It lives in two App fields:
- `tool_prompt: Option<ToolPrompt>`, the decision on screen;
- `tool_queue: VecDeque<(id, command)>`, the decisions waiting behind it.

`ToolPrompt` records no owner. The asking session is re-derived each time:
`tool_buffer(id)` → `ToolCall.owner` → `Generation.buffer`, falling back to
`current` for a typed `:tool`. At most one `Tool` pane exists, because
`Workspace::show` resizes the one it finds.

IT BLOCKS EVERY SESSION. `busy_reason_and_recovery` has a global arm: while
either field is non-empty, a prompt typed to ANY session queues, with "a
command is awaiting your decision". The test
`a_decision_that_does_not_fit_still_blocks_every_session` pins this. Two
things still run past it:
- Marked sends still START, because `start_job_on` asks only the session
  question.
- Running jobs keep streaming.

But two attended sends that both ask queue their QUESTIONS behind each
other. Until tonight the manual (`*shannon-window-send*`) said "nothing
queues behind anything"; it now says what happens.

BEHIND IT, A SECOND GLOBAL RULE. `Work::Turn` is `Scope::Global`: there is
one foreground turn app-wide. `buffers.md` §4 states the rule: "sending to a
buffer that is not the foreground one is a job". A turn paused on a decision
(`Stage::ToolRound`) still holds that slot. **So removing the decision's
global arm alone changes nothing the user sees.** Another session's prompt
would still queue, behind the paused turn instead of behind the decision.
`buffers.md` §7 records that the rule was never tested: flipping every kind
of work to `Scope::Session` left 807 tests green.

IT LANDED IN THE WRONG PLACE. `fit_tool_pane` anchored under the FIRST
session pane in ring order, not the asking session's. Stage 0 fixes that.

PROMPT AIM IS ONE VALUE. `prompt_aim: Option<BufferId>` lives on the App.
`aim_prompt_at` writes it, called from:
- `set_foreground`;
- `focus_moved` (standing in a transcript pane);
- `reaim_if_target_is_gone`.

Every prompt pane shares one composition (one `Prompt`, one keymap) and
that one aim. `split_pane` deliberately drops a prompt pane's buffer
binding. There is no per-pane option storage: `:set` writes one global
layer, and `setlocal` does not exist.

SIZING. `Extent` is `Flexible`, `Bar` or `Transient { rows }`.
- The decision is the only `Transient`: content rows, capped by
  `tui.approval_max_height`, floor 1, and it yields under pressure.
- The prompt is a static `Bar`.
- A prompt pane made by a split is `Flexible`, so `set_prompt_height` never
  reaches it.

FOUND WHILE MAPPING, FIXED (647112c). A prompt queued behind a typed
`:tool`'s decision never started once the decision was answered. Only a
turn's end drained the queue, and that decision had no turn.

==============================================================================
3. THE PROPOSAL                                         *split-panes-design*

A. A DECISION BELONGS TO A SESSION.
   Replace the two globals with one per-session queue:

       decisions: HashMap<BufferId, VecDeque<ToolPrompt>>   (front = shown)

   `ToolPrompt` carries its `BufferId`, set once in `decide_tool_call`, so
   nothing re-derives the owner. A typed `:tool` belongs to the session it
   was typed at. Every "is a decision pending" question becomes "is one
   pending FOR THIS SESSION": `busy_reason` loses its global arm and gains a
   per-session one.

B. KEEP ONE FOREGROUND TURN; SEND THE REST AS JOBS.        [decision 1]
   `buffers.md` §4 already says what a prompt to a non-foreground session
   is: a job. Today `submit` QUEUES instead when another session holds the
   foreground. Proposed: in that case the prompt starts as an ATTENDED job,
   exactly a one-pane `:Send`, and it asks through its own session's
   decisions. Nothing new is needed: `start_job_on` + `JobKind::Send`.

   The alternative is to drop `Scope::Global` for turns. §7 of `buffers.md`
   shows that no test depends on it. That is the risk, not the reassurance:
   it means nothing checks what else leans on "one foreground".

   Recommendation: jobs. This keeps the invariant, reuses the machinery,
   and the user sees no difference: the reply streams into its pane either
   way. My bias, stated: I prefer the smaller change. If the "foreground"
   distinction is itself what you want gone, B is the wrong half-step.

C. THE ASK IS A SUB-PANE OF ITS SESSION'S PANE.
   `fit_tool_pane` becomes per session. For each session with a front
   decision, a `Transient` pane is split under a pane showing that session,
   by stage 0's rule.
   - `Workspace::show` / `hide` / `focus_kind` need pane-addressed versions.
     Today they act on "the first pane of a kind" or "every pane of a kind".
   - Keys route by the focused pane: a decision pane answers ITS session's
     front decision. `route_key`, `handle_tool_key`, `text_pane_mut`,
     `scroll_pane` and the status line's `DECIDE:` segment read the focused
     one, not a global.
   - `last_tool_width` becomes per pane.
   - The decision's title names its session when more than one session is
     on screen (the rule `aim_needs_saying` already states for the prompt).

D. A SESSION IN NO PANE HOLDS.                             [decision 3]
   Its turn or job parks as `Held::Decision`, beside `hold_job`'s existing
   `Held::Command`, and says so: "3f2a… is waiting on a decision — `:open`
   it to answer". Opening the session shows the ask again. Nothing is denied
   behind the user's back. Today `reconcile_pane_state`'s "pane gone → deny"
   would become "session gone from the screen → hold". One exception is
   open: a decision pane the USER closes still denies (the manual promises
   "closing is a deny"), while one that disappears because its session left
   the screen holds. The two are distinguishable: one is a `:close` of the
   decision pane, the other a change to the pane it hung from.

E. SIZING WITH THE PROMPT'S PRIMITIVES.
   The decision keeps `Transient`: sized to its content, and it yields.
   Steven's flexible prompt (min/max rows) is the same extent with bounds.
   One variant, `Extent::Fit { min, max }`, could serve both:
   - the prompt as `Fit { 1..=N }`, growing with its text;
   - the decision as `Fit { 1..=approval_max_height }`.

   Stage it after C. It is a nicety once placement is right.

F. PROMPT AIM PER PROMPT PANE.                             [decision 4, 5]
   `Slot.buffer` already exists for transcript panes. Proposed:
   - a prompt pane may carry an aim (`Slot.buffer`); none means it follows;
   - `tui.prompt.aim = "auto" | "manual"`. Under `auto`, a prompt pane is
     re-aimed when you stand in a transcript pane; under `manual`, only an
     explicit command moves it;
   - `:aim {n}` points THIS prompt pane at pane n's session. `:setlocal`
     needs an option-scope layer that does not exist yet; `:aim` gets the
     behaviour first and can become its sugar later.

   Open question: with one SHARED composition, two prompt panes aimed at
   different sessions would show the same draft bound for two places. That
   reads as one text with two destinations. Proposed: a prompt pane gets its
   own composition when its aim differs.

==============================================================================
4. DECISIONS FOR STEVEN                             *split-panes-decisions*

1. One foreground plus jobs (B, recommended), or drop `Scope::Global`?
   UPDATE 2026-09-30: Steven's answers ("everything is a pane"; prompt a
   chat, replace the view, the first keeps running; `:ls` with what needs
   you) point at a session model with no special foreground. My lean has
   moved to dropping `Scope::Global` for turns, with a careful test pass.
   Not yet confirmed. Per-session decisions (stages 1–4) wait on it.
2. With several panes on one session, which hosts its ask? Stage 0 uses the
   one you stand in, else the first. Is that right, or the most recently
   focused?
3. A session leaves the screen with an ask pending: hold (proposed), with
   a user's `:close` of the decision pane still a deny?
4. Aim: under `auto`, does standing in a transcript re-aim EVERY following
   prompt pane, or only the one nearest it? And `:aim` or `:setlocal`?
   ANSWERED 2026-10-26: "make sure that the reaim is on the last prompt
   accessed", with "a way to manually change the prompt's aim as a
   fallback" — built as `:aim [N]`.
5. Prompt composition: shared, or per pane once aims differ?
   ANSWERED 2026-10-26: "each prompt pane has its own draft and its own
   aim" — per pane always, not only once aims differ.
6. The status line with N sessions deciding: `DECIDE(2):` plus a key to
   jump to the next ask?

==============================================================================
5. STAGING                                               *split-panes-stages*

0. DONE 2026-09-30: the ask opens under a pane showing the asking session
   (the one you stand in, else the first). No state moved. The manual says
   where it opens and that it is one at a time.
   ALSO DONE: `:ls` lists sessions in no pane with `needs you`, `:buffer N`
   shows one, and `:Send b2` / `:Send <id>` reach one (Steven's buffer
   requests, `*shannon-buffers*`). This replaces §3D's "hold" with something
   better: a hidden session's ask waits visibly in `:ls`.
1. `ToolPrompt` gains its `BufferId`; the `decisions` map replaces
   `tool_prompt` / `tool_queue`, still SHOWING one at a time. This is a pure
   data move; the suite should not change.
2. The per-session busy arm, and a prompt to another session goes as a job
   (B). This is the first visible change: another session's prompt no
   longer waits.
3. One sub-pane per session (C); keys by the focused pane; the status
   segment.
4. Hold for a session in no pane (D).
5. Per-pane aim (F); then `Extent::Fit` (E).

Tests that assert today's global behaviour, and will change ON PURPOSE in
stage 2, each to be rewritten rather than deleted:
- `a_decision_that_does_not_fit_still_blocks_every_session`;
- the queued-decision tests around `tool_queue`.

 vim:tw=78:ts=8:ft=help:norl:
