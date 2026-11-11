*shannon-buffers.txt*   Buffers and windows: what a pane actually shows

                    SHANNON CODE — THE BUFFER MODEL

STATUS: P1–P5 BUILT, plus per-window scroll and the generation-ownership work
in §6 (P4 corrected 2026-08-06; P5 and the viewport move 2026-08-07). The model
is complete: a buffer is content, a window owns cursor AND scroll, a prompt
aims at a buffer, and marks send one message to several. Threaded conversations
(`ideas.md` §4) sit on top of this and are the remaining consumer.
2026-10-20: §10 proposes the same split for every viewer (before tabs).
2026-10-21: §9 (one turn per session) BUILT through phase 6; §10 V1–V4a
built, V4b/V5 asked. §1–§8 corrected against the code — the 10-15 review's
corrections (`docs/Reviews/2026-10-15-buffers-md/`), each re-checked after
phases 1–6 rather than pasted, since several had been made true or false
again by then.
2026-10-24: V4 identity built and verified. 2026-10-25: V4b's questions
answered (vim's way throughout, §10.4); V4b next, then V5.

==============================================================================
1. THE PROBLEM, STATED PRECISELY                            *buffers-problem*

*(Historical — the state before P1, kept as the problem statement. None of
the three fields below exists any more.)*

`wm.rs` has claimed since it was written that "a pane is a `PaneId`, a buffer is
a `BufferId`, and the id→content mapping lives in the app… everything is a
buffer, viewports are pure view." `BufferId` exists in that file. **Nothing has
ever used it.**

What the app actually has is one of everything:

    App.conversation : Conversation   the foreground session
    App.output       : Output         its rendered transcript
    App.chat         : Buffer         the cursor over that transcript

and `PaneKind::Output` means "render `app.output`". So every transcript pane
shows the same session, at the same scroll, with the same cursor. That single
fact is behind four things we have already run into:

- **A prompt cannot be tied to a chat**, because there is nothing to tie it to.
  The prompt submits to `App.conversation`, full stop.
- **Two transcript panes cannot be independent.** Steven, splitting one:
  "I noticed that it changed both panes… I think we should leave them
  independent."
- **`:Send 1,3` has no referent.** Pane numbers exist now; the sessions behind
  them do not.
- **`:hide` has nothing to hide** — vim's `:hide` closes a window and keeps the
  buffer, which presumes a buffer that outlives the window.

Note what is *not* the problem: there is no shortage of `Conversation`s.
Background child sessions each own one (`Job.conversation`). They simply have
no viewport, which is the same gap seen from the other side.

==============================================================================
2. THE MODEL                                                  *buffers-model*

Vim's, because it is right and because it is the reference:

    BUFFER   the content.  A session: its `Conversation` and the `Output`
             rendering derived from it.
    WINDOW   a viewport onto a buffer, with ITS OWN cursor and scroll.

**The split between them is the whole design**, and the tell is what happens
when two windows show one buffer. In vim you get the same text and two
independent cursors — which is exactly Steven's instinct, arrived at from the
other direction. So:

    a buffer owns   what everyone must agree on   (the turns, the text)
    a window owns   where YOU are looking at it   (cursor, scroll, selection)

Concretely, `App.chat` — the transcript cursor — is in the wrong place today.
It is per-*window* state living as a global, which is precisely why two panes
could not disagree about where the cursor was. It moves to the pane.

    ┌─ buffers ──────────────┐        ┌─ panes ───────────────────────┐
    │ B0  Conversation+Output│◀───────│ P1  buffer B0, cursor, scroll │
    │ B1  Conversation+Output│◀──┐    │ P2  buffer B0, cursor, scroll │
    └────────────────────────┘   └────│ P3  buffer B1, cursor, scroll │
                                      │ P4  prompt → targets B0       │
                                      └───────────────────────────────┘

A prompt pane is a window too, and what it points at is a **target buffer** —
the session its `<Enter>` commits to. That is the tie, and it is drawn on two
channels: the target transcript wears an accent border (when unfocused —
focus wins the border), and the prompt's title names the target's short id.
One predicate gates both (`aim_needs_saying`); a rendered-frame test pins
both (`both_aim_channels_are_drawn`, §9 phase 1).

2.1 Identity                                                *buffers-identity*

`BufferId` is an **occurrence**, not content. It is not `EpisodeId`, and the
distinction is one this project has been bitten by before: a content hash names
*what* something is, never *which one*. Two buffers may be open on the same
episode (a session and a checkout of it), and closing one must not close the
other. `BufferId` is a plain counter, like `PaneId`.

2.2 What a buffer is NOT                                     *buffers-scope*

**SUPERSEDED 2026-10-20 by §10** (its consumers arrived; Steven: "are we
doing the 'everything is a buffer' yet?"). As written then:

Not every pane kind has one. The graph, the inspector, `:analyze` and `:help`
are *views over app state*, not sessions, and giving them buffer ids would be
generality with no consumer. `Slot.buffer` is therefore `Option<BufferId>`:
`Some` for `Output` and `Prompt`, `None` for the rest. If a second graph pane
ever needs its own selection, that is the moment to widen it — not before.

*(Stale in both halves by 10-15, review A: a prompt never carries a binding
— the aim is `App::prompt_aim`, pinned by
`a_prompt_pane_never_carries_a_session_binding` — while a pane lent to a
viewer keeps its session's, and a viewer split off a bound transcript
inherits one. Since §10 V3 the manual, `:analyze` and `:messages` are page
buffers (`App::pages`), numbered from the same counter.)*

==============================================================================
3. PHASES                                                    *buffers-phases*

Each is green and committed on its own. Ordered so the risky part (moving
per-window state out of globals) happens while behaviour is still pinned by the
existing tests.

**P1 — the registry, with one buffer.** Introduce `SessionBuffer { conversation,
output }` and `App.buffers: Vec<SessionBuffer>` holding exactly one. Route the
37 `self.conversation` sites through accessors (`conversation()`,
`conversation_mut()`) that resolve the *current* buffer. No behaviour change,
and the existing suite is the proof.

**P2 — the cursor moves to the pane. DONE.** `App.pane_views:
HashMap<PaneId, PaneView>` holds the per-window cursor; two transcript panes
are independent, and a closed pane's view goes with it through
`reconcile_pane_state`. The `Slot.buffer` binding did NOT land here — with one
buffer it would have been a field every pane sets to the same value, and P3 is
where it earns itself.

  *Learned while building it:* a pane's cursor buffer only mirrors the
  transcript **while that pane is focused**, so an unfocused pane holds a live
  cursor over stale lines. That is fine (an unfocused pane renders from
  `Output`, not from the cursor) and it is why the test drives focus rather
  than poking cursors directly — but it means the per-pane mirror is not a
  place to put anything that must be correct while unfocused.

**P3 — a pane can show another session. DONE.** `:open <hash>` (and `<CR>` in
`:graph`) point the focused pane at that session's buffer, creating one if
needed, and move the foreground with it. A child job's conversation lives in a
buffer from the moment it spawns, so `:open <child>` shows a running child's
reply *arriving* — inspectable rather than merely reported, which is "agents as
channels" (`ideas.md` §4) turning up early because the model was right.

  *Learned while building it:*

  - **An unbound pane follows the foreground**, which is the only sensible
    default and exactly wrong the instant the foreground moves — every sibling
    follows too. `pin_session_panes` makes every implicit binding explicit
    immediately before anything can move under it, on the one door that moves
    it. Without it, "it changed both panes" comes back wearing a buffer.
    *(Superseded 10-25, V5.3 (2): a transcript pane is born on its session,
    so nothing is unbound and the pin went.)*
  - **A pane lent to a viewer still owns its session**, in both directions: it
    must be pinned (or closing `:graph` restores it onto whatever is in front
    by then), and `:open` must be able to reclaim it (or `:open` typed while
    `:graph` has borrowed the only transcript pane refuses for want of a pane —
    in the layout the app starts in). One predicate, `session_panes`.
  - **Where a session opens** is the focused pane, the rule every other view
    command follows. From the prompt bar with one transcript pane there is
    nothing to guess; with several there is, and the thing we would guess from
    is creation order — nowhere on the screen. It refuses and says how to
    resolve it. P4 retires that branch.
  - **Two things were written for n=1 and quietly covered a fraction of n>1**:
    the orphan guard asked only the foreground for a live unpersisted clone,
    and `:delete` had a special case for "the active session" rather than a
    rule about every holder. Both are now stated over all buffers.
  - **The prompt can now be aimed at a running child**, so "somebody else's
    turn" and "my session" stopped being disjoint. `turn_in_flight` gained a
    job-on-the-foreground-buffer clause, and job resolution drains the queue —
    queuing with nothing to drain it is a prompt that silently never sends.
  - **A door only removes a rule if every caller walks through it** (found by
    dogfooding, 2026-08-03: `:new` from a `:graph` pane did nothing at all).
    P3 gave `:open` the three moves — reclaim the viewport, show the buffer,
    move the foreground — and left `:new` and `checkout_turn` with their own
    endings, so `:new` reset a session *behind* a view that kept showing the
    tree, and `:open <turn-id>` skipped the reclaim that `:open <session-id>`
    did. That last one is the sharp version: **one command with two
    behaviours, chosen by which id space the user's eight hex characters
    happened to name** — not something they can see, and not something they
    would think to ask about. The three moves are now `App::enter_session`
    and all three callers end there. (Since §9 phase 4 a composition or a
    checkout no longer rewrites its target in place: it lands BESIDE its
    sources in a buffer of its own, or goes on in its buffer when it
    replaces it — `App::land`; nothing but a stop ends work in flight.)
    `:new` also mints a buffer rather than
    emptying the one the pane held, because a buffer is an *occurrence*: a
    second pane parked on the old session keeps showing it.

*(The binding described in this paragraph was superseded on 2026-08-06 — see
"Corrected by dogfooding" below: the aim is `App::prompt_aim`.)*

**P4 — the prompt targets a buffer, and it is visible. DONE.** A prompt pane's
`Slot.buffer` is its **target**; `<Enter>` commits there. `None` means "follow
the foreground", the same unbound-follows rule transcript panes use — a prompt
is a window like any other, and what it points at is a binding, not a special
case.

  *The gesture:* **standing in a transcript pane aims the prompt at it.** No new
  grammar, and it is the project's own rule — act on the thing you are pointing
  at ([[feedback_predictable_from_screen]]). A pane lent to a viewer is not one:
  you went there to read, not to talk. `:open`/`:new` still move both, so the
  common case is unchanged.

  *The safety mechanism is that it is DRAWN*, and that is not decoration. Aiming
  by focus is only safe if you can see the aim before you commit; otherwise
  walking through panes to read something silently re-addresses your next
  sentence. Two channels, deliberately: an accent border on the target
  transcript, and the target's short id in the prompt's title. Colour is the one channel a
  colourblind user may not have, and "which conversation am I about to send to"
  is not a question to answer only in hue.

  *Learned while building it:* four functions moved focus and each would have
  needed the re-aim rule. `App::focus_moved` is the one door
  ([[feedback_one_door_per_invariant]]) — the lesson from `enter_session`, applied
  before the bug rather than after it. (Review 2026-10-15 A F4 found five
  writers past it — a split focusing its newcomer, a close reconciling onto
  the first leaf, a viewer opening in another pane; it became structural with
  `settle_focus` at the event's end, stop 5, re-measured in §9 phase 4.) And `turn_in_flight` had to start asking
  about the **target** rather than the foreground: "is the slot free" is a
  question about the session the turn would go to.

  *Corrected by dogfooding, 2026-08-06.* Three things, and the first two are the
  same mistake:

  - **The aim was stored on the prompt pane's `Slot.buffer`**, on the argument
    quoted above — "a prompt is a window like any other". The symmetry is
    appealing and it is false: `Workspace::prompt_pane` answered with the
    *first* prompt slot, so `:vsp` from the prompt produced a focused second
    window whose binding was never read and never written, drawing a title for
    an aim it did not own. **A window's binding cannot be a buffer's property.**
    It lives on `App::prompt_aim` now — one field, not a rule keeping N slots in
    sync — and `prompt_pane()` is *deleted* rather than fixed, because there is
    no correct "which one" for it to answer. *(Superseded 10-26: Steven asked
    for each prompt window's own draft and aim, so the aim is the WINDOW's —
    `PromptView.aim`, not a buffer's binding, which this lesson still holds —
    and `Workspace::prompt_pane` came back with a "which one" to answer: the
    focused prompt, else the one noted last. `tabs.md` has the model.)*
  - **The two channels drifted apart.** `prompt_title` suppressed itself with a
    single transcript pane and compared *resolved* buffers; `is_prompt_target`
    did neither, comparing the raw `Slot.buffer` — `None` for a pane that was
    nevertheless showing the target. So the accent stayed dark on the pane it
    described, then lit up on *both* panes the moment something pinned them.
    `aim_needs_saying()` is the one predicate both consult
    ([[project_shannon_two_predicates]]).
  - **`:new` in a split did nothing you could see.** `blank_buffer` reuses a
    session with no committed turns so that leaning on `:new` does not litter
    `:graph` with empty episodes — correct, and written when one pane made reuse
    *indistinguishable* from a fresh start. With a second pane on that session
    it is a dead command, and it then pinned both panes to the one buffer, so
    every later reply appeared in both transcripts and they read as welded
    together. Since `:vsp` + `:new` is the first thing anyone tries and the only
    route to the headline workflow, this was the whole feature failing from the
    front door. The rule now stops exactly where the user could tell the
    difference: reuse only while no other pane shows it.

  *And the round that followed, which found the deeper version of the same
  thing.* Three reviewers over the aim/buffer surface:

  - **A command must land where it looked.** The previous round moved every
    session command's *reads* onto the aim (`target_conversation`) and left
    every *write* on `App::current`. Seven synchronous compositions — clone,
    pick, merge, result-inject, the summary commit, checkout, the edit branch —
    therefore wrote into the foreground while reading from the pane you were
    standing in, so `:clone` from a non-foreground pane put the clone into the
    OTHER pane's session. **The welded-panes symptom, re-entered through the
    compose family.** Destination follows source now, and `prompt_target()` is
    documented as the single accessor for both. (A checkout was the exception
    — it landed in the focused pane's buffer, so from a lent `:graph` the two
    landed in different panes, review 2026-10-15 A F6; one rule since §9
    phase 4: a checkout by `host_pane`, a composition in the aim's window.)
  - **The CRITICAL, and the sharpest instance of it:** `:continue` resolved its
    whole request through the aim, then called `reveal_foreground()` — which
    goes through `enter_session` and therefore *re-aims the prompt* — one line
    before `submit()` read the aim again. The instruction, carrying 200
    characters of the aimed session's partial reply, was sent into whichever
    session happened to be foreground: a billable cross-session write that
    splices one conversation into another's context. `reveal_foreground()` is
    now `reveal(buffer)`; a function that reveals "the current thing" is a
    function whose callers cannot say what they meant. **Re-entered
    2026-10-15 (CRITICAL, review A F1):** the `instruct` branch submitted
    through `submit`, whose marks-win branch sends to the marked panes — so
    with a mark on another session, `:continue` quoted the aimed session's
    partial into that one. It commits to its captured target now
    (`submit_to`, never the marks; §9 phase 0, `b48a839`).
  - **`Workspace::buffer_of` answers *boundness*, not "which session does this
    pane show".** An unbound pane is showing the foreground, so `None` there is
    not "nothing". Three sites read it the other way — the accent, the automatic
    re-aim, and `:open` on the session you are already on. The name invited it;
    the doc now refuses the reading.

  *Test lesson, again and worse:* all three reviewers independently reported
  that the raw→resolved fixes **had no test** — reverting them left the suite
  green. Writing those tests, **three of my first drafts were vacuous**, because
  an unbound pane resolves to the foreground and I had not arranged for the aim
  to differ from it. The premise line (`assert_ne!(aim, current)`) is what makes
  these tests mean anything, and it is now written explicitly in each
  ([[feedback_mutation_check_guards]]) — and a premise can still be undercut by
  placement: the re-aim test's unbound pane was first in ring order, where the
  fallback picks it anyway, so reverting that fix passed (review 2026-10-15 A
  F2; replaced by M12 in §9 phase 1).

**P5 — marks and `:Send`. DONE 2026-08-07.** `<C-w><Space>` marks a transcript
pane and a marked prompt goes to all of them; `:Send 1,3` names panes by the
numbers already on their borders. Every send is a **job**, including the one to
the aimed session — the single foreground slot stays free, and making the aimed
one special would be two code paths for one gesture. `Job` already carried a
`BufferId` rather than owning a `Conversation`, so a turn on an *existing*
session needed no new machinery: `enqueue_job` is the half that was never about
children, a child spawn is that plus "make an episode first", and `:Send` is
that plus nothing.

**Per-window scroll. DONE 2026-08-07.** `Viewport` moved from `Output` (per
buffer) onto `PaneView`, beside the cursor. §2 said a window owns "cursor,
**scroll**, selection" from the start and P2 moved only the cursor, so two
windows on one session had exactly one degree of freedom — paging one paged
both, and `j`/`k` in one dragged its twin through `follow_cursor`. That, not
the border accent, is what made a split of one session read as *welded*
(Reviewer F). The wrap cache stays on `Output`, correctly: how text wraps at a
width is a property of the content. A tell that the seam was real —
`scroll_up`/`scroll_down`/`follow_cursor` became `&self`, because scrolling
never was a mutation of the transcript.

Threads (`ideas.md` §4) sit on top of P3 and are not scheduled here.

==============================================================================
4. THINGS THAT WILL BITE                                    *buffers-hazards*

Written down before they are discovered.

**One backend slot.** CHANGED 10-21 (§9 phase 6a): one turn PER SESSION, no
limit across sessions — see §9. What follows is the rule as it stood, kept
for its reasoning (the double-booking hazard is now per session).

Shannon ran one foreground turn at a time by design, and
every slot-holding state has to be in every guard or the slot is double-booked.
P4 lets a prompt aim at a different session; it must NOT quietly become "two
foreground turns at once". Sending to a buffer that is not the foreground one
is a *job*, which is machinery that already exists — the multi-send in P5 is
N jobs, not N foreground turns.

**The wrap cache is width-keyed. FIXED (§9 phase 3, 10-17).** `Output` cached
wrapped rows for one width, and rebuilt their styling (markdown, syntect) with
them, so two panes of different widths on one buffer re-wrapped and
re-highlighted every block every frame — measured 10-15 at 1.6 s a frame
(review B F1). A reply's styling is cached apart from its rows now, once per
change to its lines, and rows sit in two width slots: 0.13 ms (release, B's
fixture), same-width frames 0.07 ms as before.

**The transcript mirror was a measured cost. FIXED (§9 phase 3).** `sync_chat`
rebuilt the focused pane's cursor buffer from the whole transcript once per
dirty frame (B F5: 2.7 ms a streamed batch at 18.4k lines). Every block's
content is numbered now, and the mirror copies only the blocks whose number
moved (`Output::lines_since`): 23 µs. Each pane that has been focused still
holds its own copy; the borrow through `Output::line_at` was not needed.

**Teardown has one door** for panes: every close verb ends in
`reconcile_pane_state` (companions and the decision pane retire themselves, by
design). Buffers never close, so they have nothing to reconcile yet. Closing
the last pane on a buffer mid-turn KEEPS the turn — it streams on and commits
into its buffer — and says where it went (`settle_sight`, §9 phase 4).

**A pane showing a deleted session. HANDLED (P3).** `:delete` can remove an
episode a pane is looking at. Every buffer holding it starts fresh in place: the
pane neither renders something that is gone nor vanishes.

==============================================================================
5. STILL OPEN                                                  *buffers-open*

Named here so they are docketed rather than discovered.

**Applied only to content, at first.** The model converted every "what is this
pane showing?" query to a `PaneId` and left every "how big is it?" query keyed
on `PaneKind` via `first_of` — which *is* the at-most-one-pane-per-kind
assumption the model exists to delete. So the app sized one pane and drew
another: two `:help` panes paged against one another's width, the wheel over an
unfocused transcript scrolled the foreground session, and `follow_chat_cursor`
read one pane's cursor, sized against a second and scrolled a third. Fixed
when sizing moved to `PaneId` (pre-recovery; no hash survives);
`FrameLayout::first_of` is now test-only, which is the tell that the
addressing mode is gone rather than merely avoided.

**The same half-application, three more times. CLOSED (§7).** A `Job` got a
`BufferId` and `PendingSummary`/`PendingContinuation` did not — they still
composed into whichever buffer was foreground when they landed, so switching
sessions inside a distillation window overwrote the wrong one. A child's tokens
were billed to the foreground buffer. And `snapshot_graph` overlaid only the
foreground conversation. The lesson is not "check these three": it is that **a
value moving from `App` onto a per-pane or per-buffer home leaves one consumer
per site**, and finding them is a question about the surface, not about the
diff. Four independent discoveries of one missing field is what finally made
the case for one `Generation` type — §7. Not the last of them: `:Edit` opened
`current`'s transcript rather than the aimed one, and the status row in a
pane that shows no session described `current` (review 10-15, B F3/F4; fixed
in §9 phase 2 — the row reads the aim wherever you stand, and since phase 6d
draws `no agent` in red if it ever has none).

**Buffers are never closed.** No `:bd`; every `:open` and child spawn adds one
for the life of the process: ~170 KB for a short child, ~14 MB for a 20k-line
session once drawn (text plus its wrap cache), plus each focused pane's mirror
(measured 10-15, review B). Page buffers (§10 V3) likewise, one per kind.
When closing exists, three readers turn a dangling id into something worse
than `None`: `buffer()` panics, `pane_buffer` silently shows the foreground,
and `prompt_target` silently re-aims `<Enter>` at it. Those are the sites to
change with `:bd`; the status row already says `no agent` rather than panic.

**Foreground stream messages carry no buffer tag. FIXED (stage two).** A job's
carried their episode; the foreground's never needed one while there was a
single session, so moving the foreground had to *end* the in-flight turn. The
foreground carries a `buffer` and its stream is tagged with a `GenerationId` —
see §6, and §7 for the tag becoming the only convention.

**Where a child's tokens are billed. SETTLED.** A job bills its own buffer
(`seal_job`; pinned by `a_child_spawns_off_the_head_turn_and_runs_in_the_background`,
review 10-15 B). The row's `tok` is the shown session's committed total plus
its in-flight work, so a child's or a `:Send`'s spend appears only when you
stand on that session; there is no app-wide total. (It was: a child folded
its usage into whichever buffer was foreground when it resolved.)

==============================================================================
6. THE FOREGROUND GENERATION                              *buffers-generation*

Stage two of "every in-flight generation has an owner", and the counterpart of
`Job`. A child's stream always knew which buffer it wrote into; the
foreground's did not, because while there was one session "the foreground" and
"the current buffer" were the same thing said twice.

    Foreground { id: GenerationId, buffer: BufferId, stage: Stage, usage }
    Stage       ::= Streaming(JoinHandle) | AwaitingDecision

(The shape as it landed. It is now `Work::Turn { stage }` in one `Generation`
— §7 — with the `JoinHandle`s on the generation, where the other three kinds
already kept theirs; and `AwaitingDecision` is `Stage::ToolRound`, renamed
because it also covers commands already allowed and running.)

**What the buffer buys.** A turn writes where it was born. `:open` may move
`current` mid-turn without disturbing it — and since §9 phase 4 so may a
checkout or a composition, which land beside rather than over it (Steven,
10-20: "A prompt in flight should stay in flight unless someone `<C-c>` or
`:q!`/`:qa!`s a conversation") — so switching away is an ordinary
thing to do rather than something that has to end your turn first. "Is there a
turn I own on this buffer" is a field comparison — both seal predicates written
on 2026-08-03 were guessing at it from outside, first from "was a stream in
flight" (false while a human answers a tool prompt) and then from "does the
conversation have a turn in progress" (true of a *child's* turn, so sealing
deleted a running child's briefing and reported success over an empty episode).

**What the id buys.** `abort()` is neither synchronous nor does it empty the
channel, so a cancelled task's queued messages still arrive. They used to be
disowned by throwing the whole channel away and building a new one — which
worked, and hid the question behind a plumbing trick that had to be remembered
at every site that ends a turn. Now the message says whose it is.

**What `ToolRound` (then `AwaitingDecision`) buys.** The turn is alive with no task to hold. That
state is why `turn_in_flight()` needed seven terms and why the app *looked*
idle while a human decided: the only evidence was a `JoinHandle` that
`Complete` had already cleared. It is deliberately unreachable from
`start_generation` — a generation that has only just been sent must never
wear it, or `resume_after_tools` fires the moment a `tool_use` block is seen
and answers the model before its own reply has finished arriving.

**The queue came with it.** `pending` carries `(BufferId, String)`. A queued
prompt used to be bare text drained into whatever was in front, so the queue
had to be *thrown away* when the foreground moved — losing the user's words to
avoid delivering them to a stranger. Both halves were the same missing field.

**Deviation from the plan, noted:** the docket said `turn_usage` onto
`SessionBuffer`. It went onto the *generation* instead, mirroring
`Job::turn_usage` — a tally belongs to the turn that earns it, and a field on
every buffer that is only meaningful while one is running is a field that can
outlive its meaning, which is exactly the bug being fixed.

**Fell out of it:** `conversation_mut` and `persist_committed_turn` (with
`buffer_mut` and `output_mut`) have no production callers left — every write
now names the buffer it lands in. They are `#[cfg(test)]`, which is the tell
that the addressing mode is gone rather than merely avoided.
`flush_stream_tail` survived in the other form: it takes the `Generation` and
writes into `generation.buffer` (this said it too was test-only; review
10-15 A).

==============================================================================
7. ONE COLLECTION FOR EVERY GENERATION                      *buffers-work*

Stage three, and the last of "every in-flight generation has an owner": there
is one kind of thing now, not four.

    Generation { id, buffer, label, usage, billed, request, forwarder,
                 stream_tail, stop, provider_session, work }
    Work        ::= Turn { stage } | Job(JobWork)
                  | Summary(Box<SummaryWork>) | Continuation(Box<ContinuationWork>)
    Stage       ::= Streaming | ToolRound
    App         { generations: Vec<Generation>, … }

(As of 10-21. `Scope ::= Global | Session` sat beside `Work` until §9 phase
6a deleted it: `conflict(buffer)` is work on that buffer, whatever its kind.)

**What was wrong.** Four kinds of work outlive a keystroke, and they lived in
four fields of four types — `Option<Foreground>`, `Vec<Job>`,
`Option<PendingSummary>`, `Option<PendingContinuation>` — each with its own
channel, its own select arm, its own teardown, its own billing. Reviewer D
counted the damage: **six functions that must know about all four, thirty
cells, no two columns holding the same set.** Every fix to that point had
filled one cell. `end_work_on` (the teardown door) and `busy_reason` (the
query) were the first two to be filled *as tables*; this is the type change
that makes the remaining columns impossible to write wrong.

**The evidence it was one type.** Every field above `work` was discovered
separately on each of the old holders, usually as a bug: `buffer` (four times —
§5), `usage` (a tally on `App` outliving its turn and billed to the next
session), and `request` **and** `forwarder` (aborting only the forwarder
*detaches* the request rather than cancelling it, so an abandoned turn kept
streaming and billing). Four independent discoveries of the same three facts is
what a missing type looks like.

**One channel, one tag.** There were three conventions for "is this message
still wanted?": the foreground tagged by `GenerationId`, jobs by `EpisodeId`,
and the summary and continuation by nothing at all — they relied on their slot
being empty. Only the first can answer after a cancel, because `abort()` is
neither synchronous nor does it drain the channel. Now every kind bridges
through the same forwarder onto one channel, and the select loop has one arm.

**`Scope` is the asymmetry, made explicit.** *(SUPERSEDED 10-21 by §9: the
second clause is gone with the enum, and sessions run apart — kept for why
the rule was one.)* `busy_reason` was five terms, four
global and one per-buffer, with nothing to say which was which or why. The rule
is two clauses: same session always conflicts (one conversation has one open
turn, whatever opened it), and global work excludes global work anywhere. That
second clause is `buffers.md` §4's decision — Shannon runs **one foreground
turn**, so `:Send` to five sessions is five jobs and not five foregrounds — and
it is why `:Send` exists as a separate gesture at all. Making it a value is
what let `start_job_on` (which asks the *session* question) share the predicate
instead of hand-writing two of its five terms, the same shape that had
`llm_squash` omitting one.

**What it cost.** The `Option<_>` fields made "two foreground turns" and "two
distillations" *unrepresentable*; a `Vec` plus a predicate only makes them
*unreachable*. That is a real trade and it is taken deliberately: the predicate
was already load-bearing for the fourth invariant (one job per session), which
no `Option` could express, and the exhaustive `Work` match is what a fifth kind
of work now cannot escape. The immediate dividend was finding that
"one foreground turn app-wide" had **never been tested** — flipping every kind
to `Scope::Session` left all 807 tests green. It was pinned then by
`only_one_foreground_turn_runs_at_a_time_across_sessions`; re-measured 10-15
at 62a6dac, the same flip failed it and
`deleting_a_session_starts_the_prompt_that_was_queued_behind_it` (1752/1754),
while flipping only the Summary and Continuation halves left all 1754 green
(review C). §9 phase 6a made the flip on purpose: the first test inverted
(`a_turn_runs_on_each_session_at_once`), the second restated on a
same-session holder.

**What it unblocked.** Tools for jobs (docket 81), since landed:
`Work::offers_tools` and `resumable` both admit `Work::Job`. A tool round is a property of
a generation, not of "the foreground": the gate, the decision queue and the
resume path all key off a `GenerationId` already, and what stopped a job from
having one was that a job was a different type living in a different field.

==============================================================================
8. ONE TABLE FOR EVERY EXIT                                 *buffers-outcome*

Stage four, and the counterpart of §7: `Generation` collapsed the **holder**
axis; this collapses the **exit** axis.

    Outcome     ::= Finished | Failed(String) | Stopped | Superseded | Deleted
    App::resolve(Generation, Outcome) -> Ended
    Ended       ::= Turn{sealed} | Job{kept,saved,ending}
                  | Summary(Option<draft>) | Continuation

**What was wrong.** Thirteen sites removed a generation and three consulted a
shared exit; the other ten each re-derived the same three facts — what streamed
was billed so it seals, the slot released must be refilled, the handles must be
aborted — whose rationale was written out in **fourteen** separate paragraphs of
`app.rs`. Reviewer D counted both numbers.

**The tell that `Keep` was the wrong axis: every call site passed a constant.**
`Seal`, `Discard`, `Seal`, `Seal` — never a value computed from state. A
parameter that is a per-call-site constant is not a parameter; it is the
caller's identity laundered through a bool. That is also *why* both its arms
were pinned by nothing: there was nothing to vary.

**Naming the cause is what makes the table possible.** `(Work, Outcome)` is a
4×5 match with no wildcard on either axis, so a fifth kind of work *or* a fifth
reason work can end is a compile error. (False for a while: the non-`Finished`
arms ended in `_`, and so did `seals()`, review 10-15 C F7; every arm named
and `seals()` an exhaustive match since §9 phase 1.) And it lets the phrasing differ where it
genuinely should: "stopped", "the turn you left was interrupted" and "turn
interrupted ({err})" are the same mechanical seal and are not interchangeable
sentences — `seal_interrupted_turn` keys them on the `Outcome` rather than
having callers overwrite each other's notices.

**Two rows deliberately do not collapse**, and saying so is the point of
distinguishing a designed difference from an accretion:

- `Finished` means something per-kind. A turn commits; a job commits *and*
  persists; a distillation goes to `$EDITOR`; a continuation composes over the
  turn it was extending. `Ended::Summary` carries the draft back out because the
  terminal lives with the event loop, not with `resolve`.
- **Draining the prompt queue stays with the callers.** Releasing a slot and
  refilling it are two steps, and a loop over generations must finish the first
  for all of them before starting the second — `stop_work` (then `stop_turn`)
  learned that when per-arm draining started a queued prompt the next arm then
  sealed, consuming and billing the user's text without ever mentioning it.
  Two more callers drained at the wrong moment, found 10-15 (review C F1, F5):
  a batch delete looped over `reset_buffer`, which drained per iteration, so it
  started a prompt queued for a session it then deleted; and an unsaved
  distillation drained before its "apply it?" was answered. Since review stop
  1 a queued prompt starts in ONE place — `App::settle`, when the event ends —
  and no inner door refills (`8054c7d`).

**What it cost, honestly.** One property became over-determined and therefore
unpinnable: `Outcome::seals()` for a `Work::Turn` on the **delete** path has no
observable effect at all (the conversation is discarded a line later; the append
to an already-dropped episode fails and is handled; the files are
byte-identical). Two guards, either sufficient, neither individually mutable.
`seals()` was said to be pinned through `Work::Job` — FALSE by 10-15 (review C
F2: `seals() -> true` left all 1754 green). `OnDisk` had made the job row
over-determined too: a child is `Own(0)` from spawn, so a seal appends, the
append to a deleted episode fails, and it stays deleted either way. The flag
still decides the outcome for a `:Send` into a session never written
(`OnDisk::Absent`), where a seal would take the register branch and recreate
what was deleted — and that is where the pin went
(`deleting_a_never_written_session_with_a_send_in_flight_does_not_resurrect_it`,
§9 phase 1). The test that
covers the Turn side says plainly that it covers the *sentence*, not the seal.

Three cells were unpinned when the table landed and are pinned now: a finished
job must not be recorded as interrupted, a superseded turn must offer
`:continue`, and a deleted one must not.

==============================================================================
9. ONE TURN PER SESSION                                    *buffers-parallel*

PROPOSED 2026-10-15; APPROVED to build 10-15 (Steven: "go forward with the
plan in `buffers.md` with the fixes from the reviewers … several stops to do
reviews along the way"), in the order under **Build order** below. Replaces
§4's "one backend slot" and §7's second `Scope` clause. **BUILT 10-21**,
phases 0–6 (each marked below with its commits); phase 7 is this section's
and the manual's as-built pass (`:help shannon-sessions-apart`).

**Why.** The `:vfork` trial (10-15) gave two agents two tasks and ran them one
after the other: the fork's turn held the app's only slot, so MAIN's prompt
queued behind it ("a turn is in flight"). Doing tasks in parallel is the point
of forking — and of merging after: compact one fork, bring it into the other,
as a branch is merged in git. Nothing technical stopped it: two `claude -p`
processes or two streams run side by side. The app-wide rule is pinned only by
the test §7 wrote when it found the rule untested
(`only_one_foreground_turn_runs_at_a_time_across_sessions`), which this change
inverts; flipping `Work::Turn` to `Scope::Session` fails it and
`deleting_a_session_starts_the_prompt_that_was_queued_behind_it`, which uses
the rule for its premise (re-measured 10-15 at 62a6dac). The "807 green" §7
reports was the day the gap was found.

**The rule.** One open turn per SESSION, as today; no limit across sessions.
`Work::Turn`, `Work::Summary` and `Work::Continuation` become `Scope::Session`,
and with no `Scope::Global` work left the enum goes — `conflict(buffer)` is
"anything on this buffer". A `:Send` job stays what it is; it just stops being
the only way to run beside a turn. (As built, 6a: `Scope` deleted outright
rather than flipped kind by kind.)

**What the user sees — decided:**

- **Ctrl-C stops the turn of the session the prompt is aimed at** (Steven:
  "reduce surprise"). Not every turn; not the focused pane's if the prompt
  aims elsewhere. With nothing running there, it says so and names where
  something is. **As built (6b, 10-21, corrected by the full review):** the
  aimed session AND every marked one (`App::ctrl_c_stop`), so one press
  stops what one multi-send started. First built as the marks INSTEAD of
  the aim — the sessions `<Enter>` sends to — which left the aim's own
  reply running while other panes were marked, calling it "elsewhere"; all
  three reviewers of 10-21 independently proposed the union (A 4, B, C 2).
  The marks half is my extension, asked of Steven: call 3 decided the aim
  and did not speak of marks. Steven, 10-21: "That seems natural to me."
  **Not from the decision pane** (Steven, 10-21: "In the decision pane I
  don't think it should stop the sessions work. Do that from the prompt
  pane"): aimed at A, inside B's decision, the press stopped A's reply and
  left B's call standing (full review A 13). There it says where the stop
  is and that `d` denies; the row there drops `^C:stop`, and a busy
  refusal says "CTRL-C at the prompt" (`App::ctrl_c_stops_here`,
  `StopKey`; `ctrl_c_in_the_decision_pane_stops_no_sessions_work`).
- **The status row counts**: `N queued · M sessions running` when M > 1, so a
  turn in a pane off-screen is never invisible. M = 1 reads as today. **As
  built (6c):** `$sessions` counts sessions with a turn, a distillation or a
  continuation (jobs keep `$children`/`$sends`), said whenever one of them is
  not the aim — at M = 1 too, since the aim's own segments now say only the
  aim's (asked of Steven). `$queued` stays app-wide.
- **The prompt's aim is unchanged**: the prompt pane names its target, and the
  aimed transcript is highlighted (Steven: two indicators, already there —
  to verify, not to build). Verified 10-15 (review C F3): both exist,
  `prompt_title` and `is_prompt_target` → `ui.border.aimed`, and neither was
  pinned where drawn; pinned since phase 1 (`both_aim_channels_are_drawn`).
- **A decision belongs to its session, said two ways**: the pane opens beside
  that session's transcript (position) and names the session (text). Steven
  believes both exist; to verify in code, and to build whichever is missing.
  Questions from two sessions shared the one queue, in arrival order, until
  10-22: since then each session's are its own, in its own decision pane
  (`decision-panes-and-alerts.md` §3, A1.3).
  Verified 10-15: position existed and was pinned
  (`a_decision_opens_under_the_session_that_asked`); text was missing —
  built in phase 5 (`ToolPrompt::whose`: the title names the session that
  asked, and the next call's session by name when it is another's).

**What changes in the code** (the sites, counted 10-15 by `rg 'foreground'`
— the 13 lines that called `foreground()`; the class was wider, review C §3:
first-of-kind lookups, the global decision hold, `Scope::Global` users,
`anything_running`'s promise, the ending notices):

- `handle_turn_msg` finds its turn BY ID. `foreground().filter(id)` takes the
  first turn: a second session's chunks still landed (`dispatch_stream_msg`
  routes them by id), but its `ToolUse`, `Complete` and `Error` were dropped,
  so that turn never ended. **Done, phase 5.**
- `set_stage` takes the generation it moves — it moved the first session's
  stage for the second's tool round, which then never resumed. **Done,
  phase 5.**
- `streaming()`, `awaiting_decision()`, `answering()` ask about a SESSION
  (the aim's, for the row); an app-wide "any turn" is a separate question.
  **As built:** `answering` the aim's (6c); `streaming`/`awaiting_decision`
  are test-only, which is where "any turn" is still asked.
- `drain_next` examines every queued prompt, starting each whose session is
  free, each session's prompts in order. Front-only would let a busy session
  starve every other session's queue. **Done, phase 2** (C F6), then one
  door: `settle`.
- `stop_turn` (Ctrl-C) takes the aimed buffer. Built as `stop_work(Stop)`:
  `ctrl_c_stop()` (the aim and the marks) for Ctrl-C, `Stop::Everything`
  for leaving. `ctrl_c_reach` is the one statement of the key's reach —
  what a press stops (`ctrl_c_stop`, which the row's `^C:stop` and the
  "again" clause ask) and where a busy refusal says to press it
  (`stop_key_for`); they had each built it themselves, three changeable
  with every test green (full review 10-21, C 1).
- `foreground()` itself goes; what remains are by-id and by-buffer lookups,
  so "the turn" cannot be asked for again.

**Tests that must exist before it lands** (the property, then a mutation of
it): two sessions stream at once and each reply lands in its own transcript;
Ctrl-C stops only the aimed one; a queued prompt for a free session starts
while another session runs; two sessions' decisions each answer the right
call; a turn and a `:compact` on DIFFERENT sessions run together, on the SAME
session still refuse. **As built**, each in `app/tests/parallel.rs`:
`the_second_turns_ending_is_its_own`, `ctrl_c_stops_the_aimed_sessions_turn_only`,
`each_sessions_queue_drains_when_its_own_turn_ends` (this named
`a_prompt_for_another_session_starts_while_a_decision_waits`, a prompt that
never queued — full review 10-21, C),
`two_sessions_decisions_each_answer_their_own_call`,
`a_compact_runs_beside_another_sessions_turn_but_not_its_own` (the last two
written in phase 7, when this list was checked against the file and found
two short).

**Hazards.** Two agents bill at once. The row's `tok` stays the aimed
session's, as everything on the row is (Steven: "the status line is always
associated with the session we're aiming at"); no summed spend. Two Claude
Code turns are two `claude` processes,
each with its own hook socket (per turn today) and the shared `Pending` map,
keyed by `tool_use_id`, which ids from two processes do not collide in. The
decision pane's Visual-mode hazard (TODO: `va…` allows the call) gets twice the
traffic; it should be fixed before or with this.

*As built.* Honest ids do not collide, but the key was whatever a socket
client sent, and every `claude`'s Bash children inherit the socket's path
(review 10-15, B F2). Since `684d0f6` each question has a serial (`AskId`):
the map is keyed by it, an answer reaches the connection that asked, a
withdrawal or a turn's end removes only its own, and two questions claiming
one call are both refused — a held child's left open (review stop 7, E 2–3).
B's property 4, honouring a question only for an id the turn's own stream
announced, is not done (`claude-code-backend.md`). `va…` in the decision pane
was fixed 10-18 (B F7).

**Build order** (10-15/16, after the review of this doc —
`docs/Reviews/2026-10-15-buffers-md/`; A/B/C below name its reviewers and their
findings). Each step green and committed on its own; each phase ends at a
review stop, and the next phase starts from what it found. The review showed
the site list above is incomplete (C §3 has the full list: `set_stage`'s
caller wedges a second session's tool round as surely as `handle_turn_msg`
drops its ending), so the list to work from is the review's, not this
section's.

- **Phase 0 — the CRITICAL. DONE `b48a839`.** `:continue` commits to its own
  session through `submit_to`, never the marks (A F1).
- **Phase 1 — pin what is unpinned; no behaviour changes. DONE `57363f9`.**
  A's five tests
  (M5, M6, M12 in place of the vacuous re-aim test, M14, M17); a rendered-frame
  test for both aim channels (C F3); `seals()` pinned by a `:Send` into a
  never-written session (C F2); every `Outcome` arm named in `resolve` and
  `seals()` an exhaustive match, so §8's claim becomes true (C F7). Every
  guard mutation-checked.
- **Phase 2 — live bugs, today's rule. DONE `d1a96ca`…`151f067`** (what it
  found on the way: the TODO's "Found during phase 2"). An unsaved summary
  drains the queue
  only after "apply it?" is answered (C F1). `<CR>` in a split `:graph` lands
  where the graph was, not ring-first (A F3). A batch `:delete` drains once,
  after (C F5). `drain_next` is per-session FIFO, every entry examined, and a
  new prompt never overtakes one queued for its session (C F6 — §9's own
  bullet, needed now). An ending off the aim says "`:open` it, then
  `:continue`" (C F4). Reads through `current` follow the aim: the row in a
  pane that shows no session, `:Edit`, `:graph`'s landing, a typed `:tool`'s
  owner (B F3, B F4, A F7). Superseded jobs and continuations say so (C F8).
  **Review stop 1 — DONE 10-16** (`docs/Reviews/2026-10-16-phase2/`; fixes
  `7a36afe`…`069f249`). Its two HIGH, decided by Steven 10-16: a delete
  refuses while work runs on a session it would rebuild (`5d3758f`), and a
  queued prompt starts in one place — `App::settle`, when the event ends —
  with no inner door refilling (`8054c7d`). Two smaller calls too
  (`4505ee5`). Then phase 3.
- **Phase 3 — the wrap cache and the mirror, measured. DONE (10-17).** A
  reply's styling cached apart from its rows, once per change to its lines;
  rows in two width slots, latest first (B F1: 1.6 s/frame at widths one
  apart → 0.13 ms, release, B's fixture of 800 fenced turns; same-width
  frames 0.07 ms as before). Every block's content numbered (a process-wide
  version, new at creation and every change), and `sync_chat` copies only
  the blocks whose number changed since its copy, `Output::lines_since` (B
  F5: 2.7 ms → 23 µs a streamed batch at 18.4k lines); a mirror that does
  not hold what its record says is copied whole. The borrow through
  `Output::line_at` was not needed after that. **Review stop 2** (timings
  re-run: the numbers above, by throwaway release tests as B's).
- **Phase 4 — one door for focus, one rule for landing.** Every focus change
  goes through `focus_moved` (A F4); checkout and the compose family land by
  one rule (A F6); `adopt_session` in place or a new buffer (A F5 — call 1
  below); closing the last pane on a running turn says where it went (B F6).
  **Review stop 3.** **DONE 10-21**, re-measured first: A F4 was already
  structural (`settle_focus`, stop 5). A F5 by call 1: `App::land` and
  `Lands` — a session BESIDE its sources (clone, pick, merge, return, an
  edit's branch, a branch's or gathered summary, an import) takes a buffer
  of its own in the aim's window unless the aim is blank; one that
  REPLACES (`:continue`, `:compact`, a run of turns squashed in place by
  `s` or `S`) goes on in its buffer; derived from `adopt_composed`'s
  `carries` — and since the full review, the buffer derived from it too:
  `S` on A's turns while aimed at B landed over B, and `:squash` composed
  over the aim and ended its reply (D 1; `squash_home` is where a squash
  lives, and its busy check). Unreviewed, a result beside its sources
  arrives apart, in no pane (`Lands::Apart`, A 8). `Outcome::Superseded` is now the defensive
  ending only (`supersede_for_test`). A F6: the checkout lands by
  `host_pane` (it leaves `:graph`, as `<CR>` does) and a composition in the
  aim's window (the tree stays) — the workbench rule, designed, now said
  in `:help shannon-graph-ops`. B F6: `settle_sight`, at the event's end.
  The review stop folds into the full review after the buffers work.
- **Phase 5 — §9's prerequisites, the rule still app-wide.** **DONE
  10-21** (`15bac51`, `46089a0`, and the gate's serials): `handle_turn_msg`
  and `set_stage` by id, `foreground()` deleted (`turns()` where every turn
  is meant), the by-id harness (`feed_turn`, `fake_stream_on`; tests in
  `app/tests/parallel.rs`, pinned ahead of the flip); what is said names
  its session (`said_in`: refusals, `:qa`, a compaction) and so does the
  decision's title (`ToolPrompt::whose`, the next call's session by name);
  the gate's questions answered by serial (`AskId` — B F2's properties 1–3
  and 5; 4 not done, see `claude-code-backend.md`). Moved to phase 6, as
  they change behaviour only once sessions run apart: `resume_after_tools`
  per generation, the row's `answering…`/distilling/continuing for the aim,
  `Esc`'s cancel by session. `va…` (B F7) was already done (10-18).
  As planned: A by-id test
  harness FIRST (C 13: today's seams can only drive the first turn). Then
  `handle_turn_msg` by id; `set_stage` by id; `resume_after_tools` per
  generation; `foreground()` deleted for by-id and by-buffer lookups; the
  first-of-kind lookups by buffer (`cancel_summary`, `distilling`,
  `continuing`, `ProviderCompacted`); every ending notice, busy phrase and
  `:qa` line names its session. The `Pending` map's five properties and an
  adversarial test (B F2). The decision pane names its session (rendered
  title). `va…` cannot answer a decision (B F7). **Review stop 4**, with the
  security reviewer — held 10-21 as the project's **stop 7** (this plan's
  numbers predate the stops of 10-16…10-19, and 10-17's stop 4 had a
  Reviewer E of its own): E, a consistency review of the gate's serials,
  `docs/Reviews/2026-10-21-stop7/`. Its HIGH: a withdrawn question waiting
  in the queue came back with no record, and the lookups that default on a
  missing one ran it in Shannon's shell; the forget now clears the queue,
  and a call with no record is never decided or run.
- **Phase 6 — the flip.** `Scope` deleted; `conflict(buffer)`. Ctrl-C and
  `:qa!` (call 3); the row's `N queued · M sessions running`; the decision
  gate (call 2). `only_one_foreground_turn…` inverted, `deleting_a_session_
  starts…` rewritten on a same-session holder. The tests above plus C's
  list (1–14), each with its mutation. **Review stop 5**, every reviewer;
  the smoke run — now the full review of buffers (Steven, 10-20).
  **6a DONE 10-21** (`d71c69c`): `Scope` deleted, `conflict(buffer)` is
  work on that buffer only; `decision_on(buffer)` (now `decision_holds`,
  A2.4) holds only its session
  (`busy_reason_and_recovery`, `resume_after_tools`); a delete's sentence
  counts what it ended (`Lost`: turns, distillations, continuations,
  prompts); eight tests restated to the new rule, four added
  (`app/tests/parallel.rs`). **6b DONE 10-21:** Ctrl-C stops the aim's
  work and the marked sessions' (above), `:qa!` every session's; `^C:stop`, the
  "CTRL-C again stops it" clause and a busy refusal's "stop it (CTRL-C)"
  read the same predicate (elsewhere: "CTRL-C in its session") — as first
  built the refusal recomputed its own, true since the full review's fix
  (C 1: `ctrl_c_reach`), and from the decision pane none stops (A 13); with
  nothing to stop here, the press names what runs elsewhere; a `:Send`'s
  stop is named (`:child --stop <send>` and a held send's recovery); the
  report keeps every turn it ended (stop 7, E 5). **6c DONE 10-21:** the
  row's `answering…`, `distilling` and `continuing` are the aim's
  (`App::aims_work`, which `Esc`'s `cancel_summary` takes by too); a new
  `$sessions` in `$activity` says `M sessions running` — sessions with a
  turn, a distillation or a continuation (jobs have `$children`/`$sends`)
  — whenever one of them is not the aim. That widens "when M > 1" above:
  at M = 1 off the aim, the aim's segments would show nothing.
  `$queued` stays app-wide. `turns()` is test-only now. **6d DONE
  10-21:** a row with no session draws `no agent` in red
  (`App::aimed_session`, the row's one read) — unreachable (the aim falls
  back to `current`), replacing an `expect` in the draw. As first built
  the row said it while the rest of the frame panicked first, so it was
  never seen (full review 10-21, B 5, C 5): `view::draw` now asks once
  and draws the row alone, and the test draws the whole frame.
- **Phase 7 — the doc.** The three reports' corrections to §1–§9, §9 as
  built, `:help`, the docket. **DONE 10-21**, with §10 V6: the corrections
  re-checked against the code after phases 1–6 rather than pasted (several
  had been made true by then — §8's "no wildcard", the `seals()` pin — and
  others false, `Scope`); `:help shannon-sessions-apart` and
  `*shannon-submit*`; `docs/Testing/feature-checklist.md` (three stale lines,
  two sections to drive by hand); §9's own test list checked against
  `parallel.rs` — two missing, written. The full review of buffers follows.

**Calls before the phases that need them** (asked 10-16; the recommendation
first):

1. *(Phase 4)* `:open <turn>`, `<CR>` on a turn, `:reply` and the compose
   verbs rewrite the buffer every pane shows, and end its work in flight.
   Recommended: land in place only while no other pane shows that session and
   nothing runs on it — `blank_buffer`'s rule, generalised — else open a new
   buffer.
   **Steven, 10-20: "A prompt in flight should stay in flight unless someone
   `<C-c>` or `:q!`/`:qa!`s a conversation."** — the recommendation: nothing
   but a stop ends work in flight.
2. *(Phase 6)* Does a decision waiting in one session block sending to every
   session (pinned today, `a_decision_that_does_not_fit_still_blocks_every_
   session`)? Recommended: only its own.
   **Steven, 10-20: "No. Sessions are independent. `:vfork`/`:fork` don't
   work if that's true. We can't run multiple agents if that's true. So each
   agent is running independently."** — only its own.
3. *(Phase 6)* Ctrl-C stops the aimed session's turn (decided). Recommended:
   and everything else running ON that session (a job, a distillation, a
   continuation); `:qa!` stops and seals every session.
   **Steven, 10-20: "Yes."**
4. *(Phase 2)* The row in a pane that shows no session: the aimed session's —
   taken as decided by Steven's "the status line is always associated with
   the session we're aiming at".
   **Steven, 10-20: "Status line should always be associated with a
   session. If for some reason it isn't then show in red and say … 'no
   agent' (literal text)."**

**Then, merging — decided: `:compact`, then `:return`.** Steven's "compact one
fork and then bring it into the other session": `:compact` in the fork, then
`:return` brings its result back into the original as one turn. `:merge` keeps
refusing a clone. After the parallel turns land.

==============================================================================
10. EVERY PANE ON A BUFFER (proposed 2026-10-20)            *buffers-viewers*

Steven, 10-20: "are we doing the 'everything is a buffer' yet? Because that
should make tabs fairly easy… I agree with doing the refactor", the viewers
left to my judgement ("there's probably elegance in that work"). Before tabs
(`tabs.md` §7).

10.1 Why now: §2.2's consumers arrived                     *buffers-why-now*

§2.2 waited for "a second graph pane [that] needs its own selection". What
the code has instead (mapped 10-20, the sites read):

- **`:hide` on a viewer destroys it.** A session's window closes and its
  buffer lives on (`h` in `:ls`); a viewer's content is retired with its
  pane (`reconcile_pane_state`'s `pane_views.retain`, `release_companion`,
  `enter_session`). `:help` comes back at the top, `:graph` without its
  folds. vim's `:hide` keeps every buffer, help included.
- **Two doors build a graph, and they differ.** `:graph`
  (`open_graph_command`) adds the Claude Code section; `:sg`, `:split
  graph` and `<C-w>s` (`ensure_pane_content`) build without it, and the
  refresh carries only a section already there — a split graph never lists
  Claude Code's conversations. Each pane holds its own copy of one tree.
- **A per-pane buffer list already exists, half-built.** `PaneView.under`
  (a stack of views set aside in one pane) and `Slot.lent` (a viewer
  borrowing a transcript's pane) are each a pane remembering what it showed
  before. The walks over `pane_views` see the top view only: a graph set
  aside under `:help` misses `spend_graph_marks` and chord cancels.
- **Content and window are fused in every viewer.** `TextView` holds the
  lines AND the scroll, cursor and pending line; `GraphView` holds the tree
  AND the selection, top, Visual range and folds. So a viewer cannot be in
  two panes — or two tabs — as itself.
- **One thing built per pane kind, not once.** Folds, marks and `:set nu`
  are per-buffer or per-window in vim; today each would be built for each
  of fourteen kinds.

10.2 The model                                           *buffers-viewer-model*

§2's split, for every kind: a buffer owns what every window on it must agree
on; a window owns where YOU are in it.

    kind         buffer (content)                 window (yours)
    session      Conversation + Output            cursor, scroll, Visual
    graph        the tree, the Claude Code        selection, top, Visual,
                 section, the marks (V4b.4)       folds
    help         the topic's page                 cursor, scroll
    analyze      the input and its report         cursor, scroll
    messages     the log                          cursor, scroll
    listings     `:ls` `:alias` `:rules`          cursor, scroll
                 `:scriptnames` `:themes`,
                 derived from app state

Folds are the window's; marks are the TREE's (V4b.4, 10-25, netrw's: a
listing's marked list is its buffer's). The tree is ONE buffer per store, as
netrw's listing of a directory is one buffer (Steven, 10-26: "we only have
to handle the one 'file' but just in different views"), so every window on
it — `:sp`, `:sg`, a second `:graph` — shares its marks, both ways
(`App::settle_tree_marks`), and a verb spends them in all. A bare split
copies the folds, as vim's `:split` copies a window. (10-25 to 10-26 each
`:graph` shown together was a tree of its own, on a record of netrw that
did not reproduce — V4b below.)

**Not buffers — instruments, said as the exceptions they are:** the prompt
(it aims AT a buffer, §2), the decisions (each session's, a pane deciding
for its session — `decision-panes-and-alerts.md` §3; one app-wide call
until 10-22),
and the two companions, the inspector and the theme preview — each reads
another WINDOW's cursor (a graph's selection, a theme list's), not content
of its own. So a companion is its host's window: a view or a session asked
for in one shows in the host's window, over both while the companion sits
beside it (a nested split gives its space to the sibling there), focus with
it, and `:q` gives the host back with a companion beside it — a view's
`:q`; a session's window closes on `:q`, as a transcript's does, and
CTRL-^ is the way back (asked, 10-26) (Steven, 10-26, review stop 10 F 1's (a):
"treat `:themes` and the like as 'full screen'. So they can sit on top of
both"; `App::existing_host`). It took the companion's own pane, with
nothing behind it.

**Identity and listing.** One minter for every `BufferId` (`open_buffer`'s
counter). Sessions are LISTED; viewers are UNLISTED, as vim's help is: `:ls`
stays as it is, `:ls!` adds the viewers, `:b N` shows any of them in this
window. A viewer kind has ONE buffer, reused as vim's help buffer is:
`:help rules` moves the help buffer to `rules`, `:graph` shows the graph
buffer, `:sg` shows the same buffer in a new window (the split bug gone by
construction). Consequence to accept or not: viewers take numbers from the
same counter, so session numbers skip (`b1`, `b2`, `b4`) — vim's way.

**Lifetime.** A viewer buffer lives for the run, as a session's does; a
window closing hides it. `:q` on a viewer opened over a session's pane
gives the session back, as today — not vim's rule, Shannon's (`lent`).

**`under` and `lent` become one thing: the window's history** — the
buffers it showed before, most recent first (vim's alternate buffer `#`,
generalised). Closing a viewer window that has a history shows the previous
buffer; with none, the window closes. `<C-^>` / `:b#` toggles, as vim's.

10.3 The window mirrors its page                          *buffers-text-split*

**Revised 10-21, before building (was: split `TextView` in two).** A
`TextView`'s cursor is the transcript's own `prompt::Buffer`, which holds the
page's TEXT to move through — so every window on a page carries the text
anyway, and a page/window split would split nothing. Instead a viewer's
window does what a transcript window already does (`PaneView.chat`, synced
from its session's `Output` by version, `sync_chat`): the BUFFER holds the
page and a version; each window holds a `TextView` — its cursor, scroll and
selection over a copy — and takes the page again (`set_lines`, which keeps
the reader's place) when the version moved. One pattern for both kinds of
window. The decision's report (`ToolPrompt`) stays as it is: one call, one
pane, app-wide.

10.4 Phases (each green, mutation-checked; smoke where the user touches it)

- V1 — the false docs fixed (`BufferId` claims viewers today;
  `panes_showing` says four callers, has five; `PaneView`'s doubled
  paragraph). No behaviour change. **Revised 10-21:** the `Buffer { id,
  listed, content }` type moves to V3, with its first consumer — an enum of
  one variant is generality with no consumer (AGENTS.md's earned use).
- V2 — folded into V3: no text split (§10.3, revised).
- V3 — `Buffer { id, listed, content }` with `Content::{Session, Page}`;
  help, analyze and messages as unlisted buffers, their windows mirroring
  the page by version; `:hide` keeps them; `:ls!`; `:b N` on one.
  **Revised 10-21, building:** two collections and one minter, not one
  enum — `App::buffers` (sessions, listed) and `App::pages` (unlisted),
  both numbered by `App::mint_buffer`. With a `Content` enum every session
  accessor would answer "that id is a page" as well; listed is which
  collection holds a buffer. **V3a DONE (help):** one help buffer, made on
  the first `:help` (a refused topic makes none) and kept for the run;
  every `:help` window a `PageWindow` on it; where you were is the
  buffer's (`left_at`, vim's `'"`), kept at the event's end by
  `settle_pages` — every window, the focused one last, a jump not yet
  drawn where it lands — so no closing door has to remember it; `:b N`
  opens a window there, the line in the middle (vim restores the cursor,
  not the scroll); `:ls!` interleaves pages by number, flagged `u`; a
  session verb given a page's number says what it is. The manual is
  composed once per process. **V3b DONE:** `:analyze` and `:messages`
  are one page buffer each (`App::page_for`, the one maker), windows
  mirroring them by version (`PageBuffer::sync`, once per frame in
  `before_frame`). Two kinds of change, because they are two things a
  reader expects: a new report is a different document (`begin` —
  every window starts over at the top, titled by the new command, and
  where you were in the old one is forgotten); the history read again is
  the same document, newer (`renew` — every window takes the newer
  lines and keeps its place). `:sp` of a report is a window on it, not a
  copy picked from the first pane in ring order (`latest_analyze` gone).
  `PaneView.analyze`/`.messages` gone into `.page`; `AnalyzeView` is the
  report tests' view of `report_lines`.
- V4 — the graph as a buffer: ~~marks on it~~ marks on the window (§10.5
  #3), the rest on the window too; `:sg` and `:graph` one buffer; `under`
  and `lent` → the window's history. **V4a DONE (10-21):** one door
  builds every tree (`ensure_pane_content`; `:graph` calls it, as
  `:messages` does), so `:sg`, `:split graph` and `<C-w>s` list Claude
  Code's conversations and ask to read them as `:graph` does; a verb
  spends the marks of the tree it was pressed in only — `d` carries that
  tree through its question (`ConfirmAction::Delete`'s `from`), since
  focus can move while it is up. **Scope answered 10-21** (Steven: "I
  don't think we need to show _all_ claude code conversations. Only
  what's also in the same path. Let's keep it simple"): `a` (every
  directory's) is retired — it says so when pressed — and the section
  lists this directory's only, so no window has a scope to hold. The two
  design reviews
  (`docs/Reviews/2026-10-21-full/design-claude-section-{A,B}.md`)
  disagreed only on where that scope lived. Proposed, from A,
  for when V4 is built: the listing the buffer's, keyed by path, with
  `ClaudeReads` the one store of readings; what is open in it (folds,
  the cursor) the window's, as marks are (§10.5 #3); `:graph` re-lists; a
  split copies the window's state; a marked secret re-scrubs through the
  one door. Graph identity in `:ls!`/`:b N` no longer waits on it. **The
  last, built at review stop 10 (E 2)** with the listing still each
  window's: a section records the secrets it was listed under, and every
  tree rebuilt (`restore_tree`: a refresh, one brought back) lists a stale
  one again, what was open asked for again (`App::current_section`); the
  rules' door relists the shown ones at the mark.
  **V4 identity, building (10-24)**, scoped to Steven's word for it ("V4
  identity"): the tree's number, its `:ls!` row and `:b N`, and two window
  faults; the content/window split itself (one snapshot shared by every
  tree) is docketed until profiled, since `hydrate` keeps the bodies it
  loads inside `Graph` and a shared snapshot would be cloned per load.
  **(a) DONE:** a refresh keeps the window as it was. Every background
  event rebuilt each tree fresh (top 0, never drawn, no chord), so the
  window scrolled until the cursor sat on its bottom margin and the `m` of
  an `mf` was lost. `Held` now carries the window's top by identity (by
  how far up `above` it stands), the height it was settled at (a refresh
  is not a resize, so a `zt` past the end stays) and the chord; every
  rebuild puts them back, in place or onto a fresh view. The rule is
  vim's, measured in Neovim 0.12 with another window changing the buffer:
  the top keeps its line, and a deleted top becomes the line above what
  went. Vim also shows a line inserted just above the top — measured
  10-25: only rows inserted AT the top's edge show, n of them becoming the
  top and the cursor n screen rows lower, from another window or through
  the API in the window itself, and before line 1 likewise; a row two
  above the top stays hidden. I had left that out (an arrival signals
  rather than moves); **Steven, 10-25: vim's.** Built: the rows between
  the top row and the nearest row above the window still there show, so
  long as none of them was above the window (a row there re-sorting over
  them arrived nowhere near the edge). The case that decided it: a window
  at the list's start, under the default sort, stayed a row down when a
  session sorted to the top, and nothing on the screen said so. The limit:
  an arrival that would push the cursor off the window is not shown (the
  top by identity is), where Neovim would scroll to the cursor. The top
  holds by identity while it still shows the cursor; a re-sort (the default
  sort moves a session with a new turn to the top) keeps the cursor's row on
  the screen instead (verify L1). And a
  turn is held by its rank (the n-th copy of its content in its session),
  not its place: a turn deleted above renumbered every place after it, so a
  mark kept for a `p`, the cursor, an open fold and the window's top were
  lost in every tree. The limit: two copies of one content (a composition)
  cannot be told apart across a deletion of the first; that needs the
  deleting door to adjust what is held, as vim's `mark_adjust` does.
  **(b) DONE:** a split of a tree copies the window it was split from —
  what vim's `:split` copies (measured): the cursor, the folds, the top,
  which the new height settles. A split built a fresh tree, back on your
  session with every fold at its default. And the marks, since 10-25: I
  had left them out (a selection assembled for a verb there), and Steven
  overturned it — "When splitting the same conversation we're effectively
  treating it as the same file. So marks will be taken too." They are the
  window's, as its folds are (§10.2), and vim's `:split` copies a window's
  state whole; from the split on each window's are its own. Whether they
  should instead be the tree's, shared by every window on it as netrw's
  marked list is its buffer's (`netrw.txt` *local_markfilelist*), asked
  10-25. Visual and a chord end with the focus the split moves, in every
  pane.
  The copy shows the section it was split from, with what was read of it
  (verify M1: over a fresh one, a cursor on a Claude Code turn stood on
  the next conversation); a Claude Code turn with no row stands in at its
  conversation (`GraphView::stand_in`, as a Shannon turn at its session).
  A transcript's split likewise keeps the window's cursor and scroll (a
  fresh one followed the tail, so a split made while reading back showed
  the end). With the page's (A 12), every split of a window on a buffer
  is a second window where you were; the listings' wait for V5.
  **(c) DONE:** the tree is a buffer (`App::graph_buffer`, a
  `GraphBuffer`): numbered by `mint_buffer` when the first tree is built
  (`ensure_pane_content`, the one door), listed by `:ls!` as a page is
  (`u`, `a`/`h`), named by a session verb given its number
  (`App::unlisted_described`, which `:b N` and `session_named` both read).
  Where you were is the focused tree's cursor by identity (a `Spot`), kept
  at every event's end (`settle_graph`, as `settle_pages`); `:b N` opens a
  tree there, its row mid-window, while `:graph` still opens on your
  session (as `:help` opens at its start). In a pane already showing a
  tree, `:b N` does nothing, as vim's `:b N` of the window's own buffer —
  the pane it would use (`App::existing_host`, `host_pane`'s question
  without its making one: from the prompt bar, a tree lent over the aim;
  verify M2). `:graph` there still builds a fresh tree, which is how it
  lists Claude Code's conversations again.
  **Verified 10-24** (`docs/Reviews/2026-10-24-v4-identity/verify.md`):
  2 MEDIUM, 5 LOW, every one inside these fixes; all fixed.
  Left for V4b: a tree set aside in the pane (`PaneView::under`) is not
  what `:b N` brings back — a fresh tree is built there, at the row, as
  `:graph` builds one; the window's own history is V4b's.
  **V4b measured 10-21, not built — asked.** One history needs a session
  entry carrying its transcript window (`chat`, `synced`, `viewport`), and
  then every "which pane belongs to this session" answer moves from the
  workspace (`Slot.lent` + `Slot.buffer`) into the app: `session_panes`
  (13 callers), `pane_buffer`/`about_the_aim`, `aim_window` and the row's
  borrowed viewport, the decision pane's placement, `forget_cursors_on`,
  a split's inherited binding, `enter_session`'s deliberate clearing
  ("the session is replaced, not hidden"). What it buys: `<C-^>`/`:b#`
  and one mechanism instead of two. What it is not: a tab prerequisite —
  `tabs.md` §2, "a `Workspace` is already exactly a tab page", `lent`
  included; T1 is done. V5 likewise (and `:ls` as a buffer of itself,
  with `:ls!` a window flag, is awkward). Proposed: §9 phases 5–6 next
  (independent of both), V4b/V5 after or docketed — Steven's call.
  **Ordered 10-23** (Steven: V4 identity, V4b, V5, then the full review).
  **Plan, 10-24 — asked before building, since it changes what three verbs
  do.** Mapped (scratchpad map, sites read): `lent` has three writers, all
  in `Workspace` (`show_in`, `show_transcript`, `restore_or_close`) and four
  readers (`session_panes`, `restore_or_close`, `about_the_aim`,
  `focused_pane_gives_back`); `under` one setter (`App::show_in`), one
  giver (`reclaim_viewport`), and `enter_session` clears both.
  The shape: one `history` per window, most recent last, a buffer in it
  once — each entry a buffer with THIS window's place in it (a transcript's
  `chat` + `viewport`; a view's content, as `under` holds now). The window
  shows its last entry. `Slot.lent` and `PaneView::under` go; "which
  session is under this viewer" is the history's latest session entry,
  answered by the app (`session_panes` and its callers move off the
  workspace — the cost measured above).
  Measured in Neovim 0.12 (10-24): `:b A` makes the buffer left the
  alternate and puts this window back where it was in A; `:b#` (=
  `<C-^>`) returns likewise; a window that never showed A takes A's last
  place. With no alternate (re-measured 10-25, Neovim 0.12.6; 10-24 had
  both erring): `<C-^>` says `E23: No alternate file`, and `:b#` stays on
  the buffer, saying nothing; an alternate since wiped, `:b#` says `E86:
  Buffer N does not exist`, `<C-^>` `E23`.
  Phases: V4b.1 `under` → the history's views (no behaviour change);
  V4b.2 the transcript entry replaces `lent`, and the pane → session
  answers move to the app (no behaviour change; the bulk); V4b.3 the
  verbs below, as answered; V4b.4 the manual.
  **Answered, Steven 10-25** (asked 10-24, my recommendation first):
  1. `:open X` / `:b N` / `<CR>` from a viewer, or from another session:
     today the viewer — and a transcript it was lent over — are dropped
     ("the session is replaced, not hidden"). Vim's way: what you left
     becomes the window's alternate, and `<C-^>` takes you back to it,
     marks and folds and all. **Vim's** — "Definitely vim's way. That's
     kinda the whole point here." So a window's history holds any number
     of sessions, not the one `lent` holds.
  2. Going back to a session this window showed (`:b N`, `<C-^>`): today
     the window starts over, following the end (`show_buffer_in`'s
     `forget_cursor`). Vim's way: where you were in it, in this window; a
     window that never showed it follows the end, as now. **Vim's**, for
     both verbs — one rule, whatever the spelling. (The last half is NOT
     vim's — vim puts a window new to a buffer at its `'"`, as pages and
     the tree are here — review stop 10, D 5. **Steven, 10-26: "keep it at
     the end for now… We may want to change this one or allow markers to
     specify where."** Shannon's rule, a default: a transcript is live and
     its end is where the news lands. One answer, the session buffer's
     (`SessionBuffer::entry`), which every door asks, so a mark the
     session keeps changes it in one place.)
  3. `:q`: unchanged — over a view with something beneath, it shows that;
     otherwise the window closes (a transcript's `:q` closes, as vim's).
     **Unchanged.**
  4. `<C-^>` with nothing to go back to: **refused**, "no buffer to go
     back to in this window" (vim's E23, in our words — Steven: "If
     there's no buffer back, then no buffer to go back to… error seems
     correct"). `:b#` refuses alike, though vim's says nothing there (a
     dead key here; my call — Steven: "Vim's `:b#` to nothing could be
     improved by informing the user. So I'm good with your call").
     Normal-mode `#` stays the backward word search; `:b#` and `<C-^>`
     are the verbs.
  My call, for V4b.3, agreed 10-25: a session deleted leaves every
  window's history, so `<C-^>` then goes to the one before it, or refuses
  if none (vim keeps one alternate, and errs: `E86`/`E23`).
  **Steven's frame, 10-25: "same conversation ≈ same file"** for carrying
  vim's behaviours over. Its first consequence beyond V4b: vim's position
  marks (`ma`, `'a`, `` `a ``), which Shannon has none of — his use is a
  pane on a turn's summary and another on the code it refers to, "mark
  something while streaming, split, and jump back to that mark". In vim
  a lowercase mark is the buffer's, so every window on the session has
  it. Already on the roadmap: after tabs (Steven, 10-18: "folds is the
  minimum and we can roll out marks during the week… But tabs first";
  `tabs.md` Q4) — I asked again on 10-25 without checking.
  The tree's marks are netrw's (`mf`), not these. Steven, 10-25: "vim
  treats netrw as independent 'files'… we should treat the graph the same
  way". Measured then (Neovim 0.12.6, netrw v184): `:split` of a netrw
  window is the same buffer, its marks shared both ways; the directory
  opened again (`:new`, `:e dir`) is a new buffer with none of them.
  Asked 10-25 to confirm what that makes of a tree; **Steven, 10-25:
  "Let's go with neovim's way."** So `:sp` of a tree is the same tree,
  its marks shared both ways; `:graph` a tree of its own, its own number;
  `:b N` that tree; cursor, folds and scroll each window's. Measured
  further (netrw v184): a listing no window shows is unlisted but kept
  (`bufhidden=delete`), and opening the directory again takes it back —
  a new one only while it is shown elsewhere. So `:graph` takes a tree no
  window shows, else makes one: `:ls!` cannot fill with trees, and `:b N`
  after a `:q` still finds where you were (V4 (c)).
  **Measured again at review stop 10 (F 4), and it does NOT reproduce**
  (Neovim 0.12.6, netrw v184, the same; scripts in the session scratchpad,
  `netrw_{sexplore,cases,spelling,more}.lua`): the directory opened again
  — `:new | e dir`, `:new | e .`, `:new | Explore`, `:vnew`, `:tabnew`,
  `:Sexplore` from the listing or from a file's window, `:Vexplore`, the
  `/tmp` and `/private/tmp` spellings, a trailing slash — is the SAME
  buffer every time, shown elsewhere or hidden, its local marked list
  (`netrwmarkfilelist_<bufnr>`) with it. Neovim's way may then be one tree
  per directory, every window on it; the build keeps the record's rule
  until Steven answers (asked at stop 10's close).
  **Answered 10-26 — Steven: "(a) Follow netrw: one tree per store, and
  every :graph window shares its marks… Then we only have to handle the
  one 'file' but just in different views. Seems like it has the more
  elegant solution."** Built: `App::tree_buffer`, the one tree made on
  first need (as `page_for` makes one page of each kind), replacing
  `tree_for_graph`'s three answers (the one here, one no window shows, a
  new one). What went with plural trees: the `App::windows` walk for "no
  window shows it"; `companions_follow`, which kept an inspector across
  a tree for a tree in one pane — unreachable now, every change of a
  pane's content being to another buffer and so another kind; and three
  tests that set up two trees. Restated: `trees_are_netrws_listings`
  (one tree throughout, `:ls!` lists it once),
  `sg_from_a_tree_is_a_fresh_window_on_the_same_tree`, and the delete's
  spend (every window's, from the window asked in). V4b.4's limit, "a
  window's history keeps one tree", is gone with the second tree.
  **The plan, re-phased 10-25** with these answers, each phase committed
  green: V4b.1 one history (`PaneView::history`): the views set aside and
  `Away::Beneath`, where the window's session lies under them — `lent`
  and `under` were its two halves in two homes, and could not say in
  which order a transcript and the views around it came; "which panes own
  a session" (`App::session_panes`) and the aim's question move to the
  app. V4b.2 Q1 + Q2: `:open`/`:b N`/`<CR>` keep what they replace (a
  session as `Away::Session`, with this window's place), and coming back
  restores it. V4b.3 `<C-^>`/`:b#` and the refusal. V4b.4 the tree as
  netrw's: marks the tree's, trees plural, `:b N` to a view this window
  showed brings back that window. V4b.5 a deleted session leaves every
  history. V4b.6 the manual.
  Measured 10-25 for the split: vim's `:split` copies the window's
  alternate (`<C-^>` in the new window goes to it). A split here starts
  with no history: copied, `:q` on a split viewer would give back the
  source's transcript into the copy instead of closing it, and `:q` is
  answered unchanged (Q3). My call, flagged.
  **V4b.1 DONE (10-25):** no behaviour change — the whole suite as it
  was. `Slot.lent`, `Workspace::show_transcript` and `restore_or_close`
  are gone (`show_in` sets a kind, transcript included; `hide` closes,
  its one caller a companion's); `session_panes` is `App`'s, its 11
  readers moved, and marking asks `App::is_session_pane` first — a guard
  no test held in either home (`only_a_sessions_pane_takes_a_mark`).
  **V4b.2 DONE (10-25):** Q1 and Q2. `Away::Session(buffer, Place)` — a
  session the window left, with its cursor and scroll (not its lines:
  copied in again when it comes back, `App::put_place`, since a cursor
  placed in an empty copy is clamped). `show_buffer_in` is the one door a
  window's session changes by — `enter_session`'s and a composition
  landing in the window it shows (`place`) — and keeps the one it leaves
  where it lay (beneath the views, else last), putting this one back where
  it was. `enter_session` sets the viewer aside instead of retiring it,
  and a transcript opening supersedes its `Beneath`. `forget_cursors_on`
  resets the places kept for a session whose rows were rewritten. `<CR>`
  in a tree no longer closes it: it opens in the tree's own window (the
  `host_pane` answer), and so does an import from a Claude Code row
  (`Lands::Into`; `Import::into` went with V4b's verify fixes, `7fe6316`) —
  two ends for one key, chosen by
  whether a session already continues the conversation, would be one
  verb, two behaviours. `close_graph_landing_where_it_was` and
  `Workspace::sibling_of` went with the close. One predicate for "gives
  back" (`App::gives_back`), so `:q` on a transcript closes, history or
  none (Q3).
  The behaviour change to flag: `<CR>` in a tree in a pane of its own
  (`:sg`) shows the session IN that pane, as netrw's `<CR>` does — it
  closed the tree and showed it where its space went back.
  **V4b.3 DONE (10-25):** `<C-^>` and `:b#`, `WindowCmd::Alternate` —
  the window's last history entry back as it was: a session left, or the
  one beneath, through `enter_session`; a view through
  `show_from_history`, what it showed set aside in its place. Both ways
  back (`:q`'s `reclaim_viewport` and this) bring a view back by one door,
  `bring_back` (its companion, a tree rebuilt). From the prompt bar, the
  aimed window's (`existing_host`), as `:b N` there. Nothing to go back
  to: `NO_ALTERNATE`, for both spellings. The key: a terminal sends
  CTRL-^ as 0x1E, which the reader takes as CTRL with `6`; a terminal
  reporting keys in full may send `^` — both bound, in Normal, the first
  window command bound without `<C-w>`, so `execute_atom` answers a
  `Window` atom now instead of `Ignored` (it assumed every one came after
  the prefix). Smoke §15 drives the byte.
  **V4b.4 DONE (10-25):** the tree as netrw's listing (measured further:
  a hidden listing's marks are kept, and come back with it). `App::trees`,
  plural; `GraphView::tree` says which a window is on, carried in `Held`
  through a refresh and a split. `:graph` (`tree_for_graph`): the tree the
  pane shows, listed again; else one no window shows; else a new one.
  `:sg` from a tree: netrw's Sexplore, a tree of its own — no longer a
  copy of the window; as built, by `:graph`'s rule (one no window shows,
  its marks with it, else a new one — "fresh" here was true only with none
  hidden, review stop 10, F 4); a bare split's copy is built on the same
  tree (`open_tree_in` then, folded into `build_tree` since). The marks are the
  tree's (`GraphBuffer::marked`), each window's copy made equal at the
  event's end (`settle_tree_marks`): two windows on one tree share them,
  two `:graph`s do not — Steven's 10-20 "independent" stands for those.
  `:b N` to a view this window held (a tree, a page) brings it back as it
  was (`bring_back_held`, `PaneView::shows_buffer`); a tree in a window
  that never showed it opens at the tree's last place, as before. `:ls!`
  lists every tree. Limits, said: a window's history keeps one tree, one
  of each kind, so a second tree shown in a window drops the first's
  window there (its marks stay its tree's); trees are never dropped, but
  never outnumber the most once shown together.
  **V4b.5 DONE (10-25), and the plan corrected:** "a deleted session
  leaves every history" assumed a delete removes its buffer. It does not:
  a fresh session takes the deleted one's buffer (`reset_buffer`), as the
  panes showing it already showed, so the history keeps the buffer, and
  what leaves is the PLACE — `hold` → `forget_cursors_on`, which resets
  every place kept for it (V4b.2) — so going back finds the fresh session
  at its end. Test `a_deleted_session_leaves_no_place_behind`. Two arms
  are therefore not reached while buffers outlive their sessions, and are
  said so rather than tested through states the program cannot make:
  `give_back`'s `Back::Session` (a window that owned a session still does,
  beneath its views, with a left session before it), and the `Beneath`
  that `show_buffer_in` pushed for a viewer with no session, removed.
  **V4b.6, the manual:** written with each phase (`*shannon-window-where*`,
  `*shannon-alternate*`, `*shannon-buffers*`'s trees,
  `*shannon-graph-marks*`: the tree's).
- V5 — the listings as derived buffers (regenerated from app state as
  today, on the buffer instead of the pane).
  **Plan, 10-25** (read: `refresh_listing`, `listing_lines`,
  `PaneView::{listing, listing_all, themes}`, `ensure_pane_content`'s
  listing arms, `:ls`'s `listing_all = bang`). Today each pane holds its
  own copy of a listing's lines and re-derives it every frame, so two
  `:ls` panes derive it twice; a split of one starts at its top; `:b N`
  cannot name one; and a window's history keeps one of each KIND because
  a listing has nothing else to be known by. The shape:
  1. Each listing a buffer — `:ls`, `:ls!`, `:alias`, `:rules`,
     `:scriptnames`, `:themes` — numbered from the one counter when first
     opened (session numbers skip, §10.5 #1), listed `u` in `:ls!` as a
     page is, shown by `:b N` (this window's held view, else where you
     were, as a page).
  2. Its lines the BUFFER's, derived once a frame for every listing a
     window shows; each window on it takes them when they differ, keeping
     its place (`TextView::set_lines`). A window holds its cursor and
     scroll.
  3. A split of a listing keeps the window's place, as a page's does.
  4. With every view known by a buffer, the history's "one of each kind"
     becomes "a buffer in it once" — so a window can hold two trees.
  My calls, to flag (low stakes, each with a precedent): `:ls` and `:ls!`
  two buffers — two listings, as vim's are two outputs (the bang was a
  window flag, `listing_all`, which a buffer cannot carry); `:themes` reads
  its places again each time it is typed, as `:messages` reads its history
  again, and `:b N` shows it as it was.
  **Corrected 10-25** (the design study,
  `docs/Research/buffers-and-windows-prior-art.md` §5b): a buffer CAN carry
  it — Emacs's `*Buffer List*` is one buffer with its files-only flag on it
  (measured) — so one buffer or two is a choice, asked of Steven with both
  sides (§5b A1/A2), not a constraint; and whether its rows carry ids, with
  `<CR>` on a row (§5g), asked with it.
  **Answered, Steven 10-25:** Emacs's "everything is a buffer" is the
  framework — "giving us more flexibility in the future (e.g. … balloons/
  overlays) … Extensibility. Giving the user creative freedom." Q1 **one
  `:ls` buffer, the `!` a setting on it** (A1): "I do think of `:ls!` as
  'the same list, showing more'." Q2 **rows carry ids, `<CR>` on a row**:
  "However big it needs to be. We want an abstraction that can handle
  windows, panes, tabs, and all that. So elegance is key."
  **The plan, re-made 10-25** (read: `PaneView` `app.rs:1311-1442`, its
  `exchange` table `:1490`, `Away` `:1447`, `page.rs` whole, `page_for`
  `:5293`, `ensure_pane_content` `:13221`, `buffer_command` `:17718`,
  `buffers_listing` `:18053`, `theme_picker.rs` whole). What the reading
  showed: pages (V3) are already the mechanism — `page_for` allows any
  kind, `:analyze` is one buffer each command re-begins (A1's policy,
  §10.5 #2) — and `ThemeList` already carries a row per line, privately
  (`rows: Vec<Option<usize>>`), as `:ls` builds `(BufferId, String)` rows
  and throws the ids away. The window's content is still fused at the type:
  `PaneView` is an `Option` per kind, a history entry a whole `PaneView` with
  one filled, `exchange` the table moving them.
  - **V5.1 — a listing is a page.** `PageBuffer` carries its RECIPE
    (`Source`: the manual, `:analyze`'s report, the notice history, or a
    live listing — `:ls` with its `all`, `:alias`, `:rules`,
    `:scriptnames`, `:themes` with its list), its kind derived from it. One
    buffer per listing kind, minted by `page_for` (numbers skip, §10.5 #1),
    listed `u` in `:ls!`, shown by `:b N` where you were. A live listing is
    derived ONCE a frame per buffer some window shows (not per window), and
    renewed only when it differs; `sync_pages` hands it on, each window
    keeping its place. `:ls!` sets the one buffer's `all`, `:ls` clears it
    (Emacs's `C-u C-x C-b`, measured); the title says `ls!` while it is set,
    so a scope changed from another pane shows (G4). `:themes` reads its
    places again each time it is typed. `PaneView::{listing, listing_all,
    themes}` go, and the listing arms of `exchange`, `ensure_pane_content`,
    `listing_lines`, `holds`, `text_pane_mut` and the renderer with them.
    **DONE 10-25.** `page::Recipe` (`Manual`, `Report`, `Messages`,
    `Listing(Listing)`, `Themes(Option<ThemeList>)` — `:themes` a recipe of
    its own, its page made from a list it read and keeps, not from app
    state) and `PageBuffer::of(id, recipe)` saying each page's character
    once; `App::page_for(recipe)` find-or-mint, `set_recipe` for the
    commands that change one (`:ls!`/`:ls`, `:themes`), `open_listing_in`
    the one door a listing window opens by, `renew_listings` once a frame
    per shown buffer after sight (`before_frame`'s order kept), `refresh`
    renewing only on a change, `page_title` reading the buffer's
    `label`. Tests: `ls_bang_is_a_setting_of_the_one_listing` (rendered
    titles), `a_listing_is_derived_once_a_frame_however_many_windows_show_it`,
    `a_listing_is_numbered_listed_and_shown_by_b_n`,
    `ls_typed_again_in_its_window_keeps_your_place`,
    `a_split_of_a_listing_keeps_your_place_in_it`,
    `themes_typed_again_reads_its_places_again`; mutation 9/9 RED. The five
    hand-lists of "the page kinds" (`exchange`, `text_pane{,_mut}`,
    `focused_viewer`, the renderer) gained the listings — exhaustive, and
    V5.3's to remove (the window's one view answers them).
  - **V5.2 — rows carry ids; `<CR>` acts on a row.** A page's lines, each
    with what its row names (`Row`: a buffer, a theme, an alias, a script,
    a rule), built by one type that pushes a line and its row together.
    A renewed listing keeps each window's cursor and top on THEIR ROWS
    (V4 (a)'s rule for trees, vim's topline), by index only when the row
    went. `<CR>` on a row does what the listing's recipe says: `:ls` shows
    that buffer here, by `:b N`'s door, so `:ls` is the alternate and
    `<C-^>` comes back (netrw's `<CR>`, Emacs's `RET`); `:themes` wears it
    (its `<CR>` folded in, `ThemeList`'s private rows gone). A line naming
    nothing says so. A listing with no row action claims no `<CR>`.
    **DONE 10-25.** `page::Row` (`Buffer`, `Leftover`, `Alias`, `Rule` —
    its section's number and the rule as listed — `Script`, `Theme`) and
    `page::Lines` (a line and its row pushed together); the page and each
    window hold rows beside their lines (none for a page naming nothing);
    `PageBuffer::sync` maps old lines to new by row (`moves`: the k-th line
    of a row to the k-th, since a row may take several) and
    `TextView::set_lines_moved` keeps the cursor and the top on their lines
    (the selection's other end stays by line — the read-only buffer sets
    an anchor only at its cursor). `<CR>`: `focused_rows_act` (`:ls`,
    `:themes`) is the one question the claim, the dispatch and the test seam
    ask; `act_on_row` does the recipe's act; `show_buffer(id)` is the one
    ending `:b N` comes to once it has read its number and `<CR>` comes to
    with the row's buffer (`show_page_buffer`, `show_graph_buffer`,
    `show_session` — `switch_session` split at "the session is found").
    Found on the way, changed: `:b N` of the page a window shows now leaves
    it where it is (it put a fresh window at where you LAST were — the top
    before a settle), as a tree's always did. `:ls`'s header says `<CR>`.
  - **V5.3 — a window shows ONE thing.** `PaneView`'s per-kind `Option`s
    become one value — a session where the window is in it, a page window,
    a tree view, or an instrument (the decision, the companions) — and its
    history a list of those, a buffer in it once (so a window can hold two
    trees). `Away`, `Back` and `exchange` go. Designed in full when V5.1–2
    are in, and that design reviewed before it is built (it moves where a
    covered window's session place lives — `aim_viewport`, a landing's
    window, `session_panes` read it there).
    **Design, 10-25** (read: `PaneView` and its impl `app.rs:1311-1640`;
    `Slot` `workspace.rs:388`, its binding's three sites `:944`, `:972`,
    `:1065`; `pane_buffer`/`about_the_aim`/`session_panes`/`aim_viewport`
    `app.rs:5519-5612`; `pin_session_panes` `:5770`; `show_buffer_in`
    `:6918`; `reclaim_viewport` `:14316`; every `Away::` and `pane_views`
    site). **Today one window has two homes**: the workspace's `Slot` holds
    its kind, its session binding (kept under a view, so "beneath" works),
    its size and its `:Send` mark; `App::pane_views` holds the rest, made on
    demand (`entry(pane).or_default()`, four sites) and pruned by hand
    (`retain` in `reconcile_pane_state`, two `remove`s). Inside, a state no
    window can be in is representable — a page and a tree at once; a
    history entry a whole `PaneView` with one field filled — and kind and
    content are set by different calls (`Workspace::show_in` the kind, a
    field the content), the class behind "`:open` restored the kind and
    left the help page sitting there". Five hand-lists of "the page kinds"
    (V5.1) and `holds`/`focused_viewer`/`text_pane{,_mut}`'s per-kind
    tables exist only to ask which field a kind's content is in.
    **The shape:**
    - `View` — what a window shows, one value: `Session { buffer:
      Option<BufferId>, chat, synced, viewport }` (`None`: never bound,
      following the foreground — the state the app starts in, which
      `pin_session_panes` ends), `Page(PageWindow)`, `Tree(GraphView)`,
      `Inspector { row, text, painted }`, `Preview(Option<Preview>)`,
      `Decision { decides, shows }`, `Prompt`. `View::kind()` IS the kind;
      `View::buffer()` the buffer a session, a page or a tree is a window
      on.
    - `Window { view, history: Vec<View>, drawn }` — the history newest
      last, a BUFFER in it once (Emacs's `window-prev-buffers`, an ordered
      set; V5 item 4), never an instrument (a companion is re-derived from
      its host; the decision and the prompt are never set aside). Its
      session — what `about_the_aim`, `session_panes`, `aim_window`, a
      landing and the status row read — is the `Session` it shows, else the
      newest `Session` in its history: `Away::Beneath` and `Away::Session`
      are one entry, as today's order already makes them (a session left
      always lies before the one beneath).
    - **V5.3a** — `PaneView` becomes `Window` (the enum, the history, the
      binding in `View::Session`; `Slot.buffer`, `Away`, `Back`,
      `exchange`, `take`, `retire` gone). The compiler lists every reader
      of a covered window's place: each says whether it means the view
      shown or the window's session.
    - **V5.3b** — the window moves into its pane: `Workspace<W>` with
      `Slot { id, extent, marked, window: W }`, `kind_of` asking the window;
      `App::pane_views`, its on-demand creation and its pruning gone. A tab
      (T2, `tabs.md` §2: "a `Workspace` is already exactly a tab page")
      then carries its windows whole, and a float (a "balloon") is a window
      a second layer places.
    - Removed with them: the five page-kind hand-lists, `holds`'s and
      `text_pane{,_mut}`'s tables, `PaneView::default` standing in for a
      window that has no view yet.
    **For the design reviewer, before it is built** (the questions I cannot
    settle from my own reading): is a decision or the prompt better as a
    `View` or as a window of its own kind outside `View`; `Option` for an
    unbound session or bind at birth (the accent tests drive the unbound
    start); does any flow need one buffer twice in one window's history;
    every reader of a covered window's place, and which meaning each wants;
    `Workspace<W>` generic or a trait the layout asks `kind` of; what reads
    a `PaneView` that exists for a pane with nothing in it.
    **Reviewed 10-25: build with changes** (`docs/Reviews/2026-10-25-v53-
    design/review.md`, 2 HIGH, 4 MEDIUM, 3 LOW, all taken). What changed:
    - **A window may be DEDICATED** (Emacs's `window-dedicated-p`): the
      decision and the prompt, which host no view. A split prompt is a
      flexible pane, so `existing_host` let `:help` take it, set the prompt
      aside, and a transcript over it later counted as a session pane while
      `pane_buffer` answered the aim (P1, P5). My call, flagged to Steven:
      a prompt window hosts nothing — the prompt keeps nothing per window.
    - **The history holds PLACES, not views** — a buffer and where the
      window was in it, as Emacs's `window-prev-buffers`: a session's cursor
      and scroll, a page's topic and line, a tree's held rows. What a view
      set aside held was rebuilt when it came back anyway; kept, a session
      left was a whole transcript copy, a tree a whole graph.
    - **Bound at birth**: a window shows a buffer from the moment it is
      made, never "the foreground until told" — `pin_session_panes` and
      `pane_buffer`'s fallback go; a pane made for a session is born on it
      (P2: one made across the top recorded a session it never showed).
    - **V5.3b carries the deny**: a decision window removed by `:q`,
      `:only` or `:hide` is denied through one door, from the windows the
      workspace gives back — today a closed pane's view outlives it for
      `reconcile_pane_state` to read. `App::windows()` is the one walk of
      every window (each tab's, later a float's); a companion names its
      host rather than being paired by ring order.
    **The order, each step green and committed alone:** (1) the review's
    live findings first — the dedicated prompt, and `:reply` gated on a
    transcript you can see (it read a covered window's cursor); (2) bound
    at birth; (3) the `Window`/`View` enum with a history of places
    (`Away`, `Back`, `exchange`, `Slot.buffer` gone); (4) the window into its
    slot (`Workspace<W>`; `pane_views` gone); (5) `window.rs`, a module of
    its own. **(4) before (3)**, reordered while building (10-25): as an
    enum in today's map, a window is still made on demand
    (`entry(pane).or_default()`), so (3) would need a default VIEW for a
    pane with none — the lie the type exists to remove. In its slot, every
    pane is born holding its window; (4) passes today's `PaneView::default()`
    at each birth, and (3) makes each pass the view it shows, found by the
    compiler.
    **(4) DONE 10-25.** `Slot<W>` holds its window: the app's is a
    `Workspace<PaneView>`, the layout's tests a `Workspace<()>` (the
    geometry reads no window). Every birth takes one (`new`, `open`,
    `open_at`, `open_decision`, `open_across_top`, `split_pane` given the
    kind, `ensure_prompt`); every removal goes through `remove_slots` (the
    one write that shrinks `slots`, counted), which keeps the window in the
    workspace's closed list. The app's own closes claim theirs
    (`claim_closed`: a decision pane by placement, an answer or `:hide`; a
    companion with its host); `reconcile_pane_state` drains the rest
    (`take_closed`) — the deny door reads a decision window there, so a
    close that forgets to claim fails closed, denied. `pane_views`, its
    on-demand `entry().or_default()`, its `retain` and its two `remove`s
    are gone; `view_of{,_mut}` answer `Option`. **`App::windows{,_mut}`**
    is the walk of every window (review finding 3): 8 walkers through it
    (`:unread`, `relist_claude_sections`, a reading landing,
    `forget_cursors_on`, `refresh_graph_views`, `tree_for_graph`,
    `cancel_chords`, `end_visual_everywhere`); 2 walk the arrangement and
    say why (`sync_pages`, `settle_tree_marks`: the windows drawn — T2's
    to decide for a background tab). I first committed it NOT added,
    reasoning that T2 would remove the `workspace` field and the compiler
    would list the walkers; `tabs.md` §2 keeps a `workspace` accessor for
    the current tab, under which every walker narrows in silence — the
    reviewer's case. Reversed the same day.
    **(3), staged** (10-25): (3a) the history holds places; (3b) a
    window's content one value; (3c) its session in it, `Beneath` one
    with the sessions left, the binding off the slot; (3d) the kind read
    from the view, births passing views, companions naming their host.
    **(3a) DONE 10-25** — `Away::View(Viewed)`: a page as its window
    (`PageWindow`, re-synced when it comes back, as every window on a page
    is each frame), a tree as a place (`TreePlace`: what the window held,
    and its Claude Code section with what was read of it) built again on
    the latest state by the rebuild a shown tree takes on every background
    event (`App::restore_tree`, `refresh_graph_views`' body). A whole
    `PaneView` per entry, a graph clone per tree set aside, goes; so do
    `exchange`/`take` (one table, `PaneView::take_away`), `Back`, the
    history half of `trees_mut` (`tree_places_mut`: a reading landing
    reaches a place's section, `ClaudeSection::take_reading` shared with a
    shown tree), `tree_at`, `shows_buffer`, `PaneKind::is_companion` (the
    exhaustive table keeps nothing of a companion), `GraphView::add_reading`.
    `cancel_chords` walks shown trees only (a tree is set aside only by a
    key or `:` it took, which ends its chord; measured equivalent 10-21,
    and a place dropping its chord equivalent again, so not written). My
    call: `:unread` lets a set-aside tree's section go
    (`TreePlace::forget_claude`), listed again as `:graph` lists one when
    it comes back — less kept of what was read, not more. Mutation: 8 RED,
    1 equivalent (the chord, not written); the one GREEN found
    (`bring_back_held` by any view) now has its test
    (`b_n_brings_back_the_held_view_it_names`).
    **(3b) DONE 10-25** — `PaneView::view: View`, one of `None` (a
    transcript, the prompt, until 3c/3d), `Tree`, `Page`, `Inspector`
    (`InspectorView`: its row, its text, the painted message),
    `Preview`, `Decision` (`DecisionView`: whose — no longer an `Option`,
    a decision pane is born knowing — what it shows, how it was drawn).
    Nine `Option`s any two of which could be filled at once became one
    value; each read is a typed accessor (`tree()`, `page_mut()`, …), and
    every write of a shown window's content goes through `App::put_view`
    (counted: `open_listing_in` and `split_command` wrote the field
    directly, now through it). The companions are born holding their
    empty view (`ensure_pane_content`, which `pair_companion` now asks as
    a split always did), and a refresh fills it in place, never replacing
    what the window shows — the preview's put a view on each new theme,
    so its birth-time view was equivalent until it did as the
    inspector's.
    **(3c) DONE 10-25** — a transcript is `View::Session(SessionView)`:
    its session, cursor mirror, sync record and scroll, from birth
    (`PaneView::on`). `Away::Beneath` is gone: a view opening over a
    transcript sets its session aside as an `Away::Session` place, and the
    NEWEST session in a window's history is its own while a view shows
    (`PaneView::session`) — the review's rule, held by construction (Q5)
    and walked again by hand: views over views, a session left before one
    beneath, `:q` and `<C-^>` from each, a landing beside a covered
    session. So a covered transcript keeps a place, not its lines (copied
    in again when it comes back, as a session left always was). The slot's
    binding is gone (`Slot.buffer`, `bind`, `buffer_of`, `new`'s `first`,
    `open_across_top`'s `session`, `split_pane`'s copy): which session a
    pane shows is its window's, born so (`split_command`'s birth: a copy
    of the split window's session, cursor and scroll, else the aim; the
    fork's on the session it forks). Two doors, each one way: a window
    comes to SHOW a session by `PaneView::enter` (`enter_session` — every
    `:b N`, `:open`, `<CR>`, `:new`, checkout, `:q`/`<C-^>` back to one),
    and a landing repoints the session it shows or covers
    (`show_buffer_in`: `enter`, or `PaneView::repoint_beneath`, inserted
    after the newest session so `:q` still gives back the views above it
    first — finding 6). `about_the_aim`, `is_session_pane`, `pane_buffer`,
    `live_session` and `pane_viewport` each ask the window one question.
    Mutation 10 RED of 11 written; the 11th (`repoint_beneath` taking back
    a landed buffer's place) unreachable — a landing is a buffer of its
    own — so removed and its premise asserted (`debug_assert`). Two GREEN
    found and tested: finding 6's case had no test
    (`a_composition_under_two_views_lands_beneath_both`), nor a fork's
    window keeping its source as the alternate
    (`a_forks_window_keeps_its_source_as_the_alternate`).
    **(3d) DONE 10-25** — **a pane's kind is its view's** (`View::kind`,
    asked by the layout through `workspace::Pane`, which `PaneKind` also
    implements so the layout's own tests and proptest hold a bare kind).
    `Slot.kind` and `Workspace::show_in` are gone, and every birth passes
    the window that says what it shows (`new`, `open`, `open_decision`,
    `open_across_top`, `split_pane`, `ensure_prompt`; `View::Prompt` in
    place of `View::None`, no `Default` on `View` or `PaneView`). Content
    is built BEFORE it is shown: `content_for(here, kind)` (the table
    `ensure_pane_content` was, a tree by `build_tree`, a listing by
    `listing_view`), `open_viewer(view)`, `App::show_in(pane, view)`
    (`set_aside` swaps the new view in — no placeholder between), and
    `split_command` builds the newcomer's window first, so `:sp analyze`
    with no report refuses before any split instead of making a pane and
    taking it back. **A regression found reading, fixed here:**
    `split_command` opened its companion by a door of its own, so from
    (3b) a `:sg` tree's inspector was born with no view and drew nothing
    — every companion is now born by `open_companion`
    (`a_split_trees_inspector_shows_what_it_inspects`). Under (3d) that
    state cannot be written: an inspector pane IS its inspector view.
    **(3e) DONE 10-25** — **a companion names its host** (review finding
    7): `InspectorView::host`, `View::Preview { host, .. }`, born by
    `companion_view(host, kind)` through the one door, `pair_companion`
    (a split asks it now, its check exact). `host_of` (was `nearest_host`,
    ring proximity) reads the name, holding only while that pane shows the
    host's kind; `companions_of` (was `companion_of`, plural: a bare `:sp`
    of an inspector is a second on the tree, and `release_companion` closes
    both); the reconcile closes a companion whose host no longer holds —
    it hid one only once no host of its kind was left, and with two trees
    an orphan adopted the other's. `PaneKind::host_kind` is the pairing read
    backwards, derived from `companion`; `Workspace::hide` went with its
    last caller. My call, flagged: an inspector now keeps describing the
    tree it was opened beside when a move puts another nearer
    (`an_inspector_inspects_the_tree_it_names_not_the_nearest`) — what the
    screen showed it beside, not what is nearest now. **Found with it, and
    a second call of mine:** `:only` in an inspector closed the tree around
    it and left the inspector alone, inspecting nothing (the layout keeps a
    last pane, so neither the old rule nor the new could close it). `:only`
    now keeps a companion with its host both ways, as it keeps a decision
    pane with its window (Steven, 10-22): in the tree, its inspector; in
    the inspector, its tree (`only_in_an_inspector_leaves_no_orphan`). The
    reconcile's orphan rule is the backstop for a door that forgets to
    release (`a_companion_whose_host_closed_goes_at_the_reconcile`).
    Mutation 8 RED of 8.
    **(5) DONE 10-25** — **`app/window.rs`, a module of its own** (667
    lines): `Window` (was `PaneView`; the name is vim's, and what a slot
    holds), `View`, `SessionView`, `InspectorView`, `DecisionView`, `Away`,
    `Viewed`, `Place`, and their methods. `App::view_of{,_mut}` →
    `window_of{,_mut}` (95 calls). A move and a rename only: no behaviour
    changed, no test added. What stays in `app.rs` is what needs the app —
    the doors that build a view (`content_for`, `companion_view`,
    `build_tree`), the one write (`put_view`), and the walks.
    **Verified 10-25** (`docs/Reviews/2026-10-25-v53-verify/verify.md`, Opus,
    32 mutants: 29 red, 3 green; 2 HIGH by the unheld-guard rule, 2 MEDIUM,
    3 LOW). Dispositions:
    - **F1 (the history by kind, not by buffer)** — confirmed by running it
      (`:b <tree B>` then `:b#` lands on the transcript, not tree A). Not
      deliberate: this section's design said a buffer once, and V4b.4 had
      planned the same. Fixed by V5.4 (2) (the window half), with the
      review's probes Q2a/Q2b; the two docs stating the kind rule change
      there.
    - **F2 (HIGH: `:q`'s release held a pairing by order)** — fixed:
      `pair_companion` asks for a companion of ITS kind
      (`a_companion_of_another_kind_does_not_answer_for_this_one`,
      `q_from_a_list_over_a_tree_gives_the_tree_its_inspector`). `:q` still
      releases first, so the new companion opens in the space the old one
      leaves; without it the reconcile closes the old one after the new
      has split the host.
    - **F3 (HIGH: `host_of`'s kind half unheld)** — pinned
      (`a_companion_whose_host_shows_another_kind_holds_none`); `holds`,
      whose content check could not fail since (3d), is gone into it.
    - **F4 (four walkers re-look ids up in the current workspace)** —
      `App::windows`' doc counts ten walkers at eleven sites and names the
      four; `tabs.md` §3's row says what T2 changes at them.
    - **F5 (counts and claims)** — `release_companion`'s four doors;
      `show_in` is the door a view OPENS by, not "the one door";
      `focused_viewer`'s tree and page arms no longer ask for content they
      have by type.
    - **F6 (restated tests)** — the prompt test does the second `:sp` its
      doc named (born on the aim); the viewer-close test asks where `:q`
      leaves you (measured: the tree's pane gone, focus in the prompt); the
      close test's by-type half dropped. `history_of`'s buffer numbers move
      to V5.4 (2), where the two-tree tests need them.
    - **F7 (an equivalent reconcile inside `reclaim_viewport`)** — moved to
      `close_pane_command`, which ends both its branches in it, said
      there; `reclaim_viewport` is a give-back only.
  - **V5.4 — one buffer list.** Sessions, pages and trees behind one
    registry: "what is buffer N" answered once (`:b N`'s three lookups,
    `buffer_command` `:17741-17757`; `:ls!`'s three loops), the buffers in a
    module of their own out of `app.rs` (22k lines; the window types left in
    V5.3 (5)).
    **Design, 10-25** (read at `9aa46b8`: the three stores `app.rs:1764-1788`
    and `mint_buffer` `:19928`; `show_buffer` `:17787`; `buffer_command`
    `:17644`; `page_named` / `unlisted_described` / `tree_named`
    `:17825-17853`; `session_named` / `session_readings` `:20298-20359`;
    `buffers_listing`'s rows `:18063-18168`; `panes_showing` /
    `panes_on_page` / `panes_on_tree` `:19206-19230`; `pane_shows` `:14084`;
    `show_graph_buffer` `:17880`; `show_in` `:13918`; `Window::set_aside`
    `app/window.rs:514`). **V5.4 is the buffer half of "everything is a
    buffer"**: what a buffer is, which buffer a window shows, and a window's
    history keyed by buffer. What the reading showed:
    - (a) **"What is buffer N" has four readers, each probing three stores
      in its own order.** `show_buffer`: page, tree, session. `buffer_command`:
      page and tree twice (`unlisted_described`, then `page_named` /
      `tree_named`), then a session BY EPISODE (`session_readings` →
      `switch_session` → `buffer_for_episode`, the first buffer on it —
      right only while episodes and buffers are one to one; TODO's V5.4
      probe found no path to two, by reading). `session_named`'s refusal
      asks `unlisted_described`. `:ls` loops three times and sorts the rows
      back into number order.
    - (b) **"Which buffer does this window show" is asked five ways**:
      `pane_shows` (sessions), `panes_on_page`, `panes_on_tree`,
      `show_graph_buffer`'s `shows`, `show_page_buffer`'s check. V5.3's
      design named a `View::buffer()` (above, "`View::buffer()` the buffer a
      session, a page or a tree is a window on"); it was not built.
    - (c) **A gap V5.3 left, found here.** Its design keeps "a BUFFER in it
      once" (Emacs's `window-prev-buffers`); the build keeps one view per
      KIND — `set_aside` retains by `Away::kind`, `show_in` replaces a view
      of the kind shown. One rule for pages, which are one buffer per kind;
      not for trees, which are plural since V4b.4: `:b <tree B>` in a
      window on tree A drops A, so `<C-^>` cannot go back to it (traced,
      not yet run — step 1's first test).
    - (d) **`:ls`'s pane column names a session under a view** ("pane 1
      under help" — review stop 9, F1: "it is where `:b N` brings it
      back"), **and not a page or tree set aside**, which `:b N` brings
      back too (`bring_back_held`, V4b Q2).
    **The shape:**
    1. **`View::buffer()`, `Away::buffer()`, `Window::holds(id)`** — the
       buffer a session, a page or a tree is a window on; `None` for an
       instrument (the prompt, a decision, a companion). Every "does this
       window show X" asks `buffer()`; `holds(id) -> Option<Holding>`
       (`Shown`, `Under(PaneKind)`) is the one "does this window hold X" —
       `:ls`'s column for every kind, and `panes_holding(id)` under
       `panes_showing` / `panes_on_page` / `panes_on_tree`. The history
       keeps a buffer once (`set_aside` retains by buffer), and `show_in`
       replaces only the buffer shown, so `:b <tree B>` over tree A sets A
       aside (vim's alternate). Closes (b), (c), (d).
    2. **`Buffers`, the one list** (`app/buffers.rs`): a `Vec<Buffer>` in
       number order — `enum Buffer { Session(SessionBuffer),
       Page(PageBuffer), Tree(GraphBuffer) }` — and the counter, `mint`
       private to it, so "the one place a `BufferId` is minted" is the
       type's, not a comment's. `get(id) -> Option<&Buffer>`, `iter()`,
       and typed `session{,_mut}(id)`, `sessions{,_mut}()`, `page(id)`,
       `page_of_kind`, `pages()`, `tree{,_mut}(id)`, `trees{,_mut}()`. A kind
       added is a variant whose every `match` the compiler lists — Emacs's
       one buffer list, each buffer with its mode. The comment it answers
       (`app.rs:1777`: one collection would make "every session accessor
       answer 'that id is a page' as well") is met by the typed accessors,
       which return `None` for a page's id as they do today for any id not
       a session's. `App::buffer_of{,_mut}` → `session_of{,_mut}` (180
       calls): the old name meant "a buffer is a session", and under one
       list that meaning would still compile.
    3. **"What is buffer N", once.** `show_buffer(id)` is one `match` on
       `Buffer`. `:b N` reads `bN` as a buffer of any kind and shows it by
       its number, never through its episode; an id, a name or `%` still
       finds an episode (one may have no buffer open), as today, and the
       refusal for `b1234` read both ways stays. `unlisted_described`,
       `page_named` and `tree_named` become `Buffer::described()` and one
       lookup; `session_named` reads `bN` as the list says.
    4. **`:ls` in one loop** — `iter()` filtered by `all` or listed, each
       buffer's row from its variant; the sort goes, the order being the
       list's.
    5. **Modules**: `SessionBuffer` with `OnDisk`, `Listed`, `Unwritten`,
       `Billing` (re-exported: `view.rs` reads `crate::app::Billing`) and
       `Buffers` into `app/buffers.rs`.
    **Order**, each green, mutation-checked and committed alone: (1); (2)
    with (3), since a list with no single reader is a fourth place to ask;
    (4); (5). (1) waits on the V5.3 verify review, which reads the same
    code.
    **My calls, flagged:** (c)'s fix changes what `<C-^>` does after `:b`
    to another tree (vim's alternate, as the design said); (d)'s adds
    "pane N under X" to `:ls!` for a page or tree set aside (the session
    rule's reason, applied to every kind).
    **A question for Steven, not blocking:** instruments as buffers — the
    prompt, a decision, a companion (Emacs makes even the minibuffer one).
    V5.4 leaves them views: not numbered, not in `:ls!`, not `:b N`. Each
    is derived from another window's state (an inspector from its tree's
    cursor, a decision from a session's waiting call), so "the buffer" of
    one is a question about that state's lifetime. Recommended: docketed
    with balloons and overlays, after T2; nothing here precludes it.
    **Reviewed 10-25: build with changes** (`docs/Reviews/
    2026-10-25-v54-design/review.md`: 1 HIGH, 4 MEDIUM, 6 LOW; gap (c)
    confirmed by running it, probe P1). Taken, and what changed:
    - **The session half of the `b1234` refusal is pinned first** (HIGH 1):
      `session_readings` collects the buffer `b1234` AND every id beginning
      `b1234`; reading `bN` alone passed all 2322 tests. Its test (the
      review's P3, `:b`, `:Send`, `:decide`) lands before step (3).
    - **`Holding { Shown, Beneath, Aside }`** (M2): `panes_showing`'s 13
      callers mean the window's SESSION (shown or beneath), the page and
      tree askers mean shown; `Under(kind)` could not tell beneath from
      aside (counting any held session turned 14 tests red). One
      `Window::take(buffer) -> Option<Away>` replaces `take_view(at)`,
      `take_session` and `bring_back_held`'s search, and takes a session's
      place out BEFORE the swap (L6: a retain by buffer after it wiped the
      place, 3 tests red).
    - **One reader of a buffer's name** (M3): `buffer_named(token)` — `bN`
      from the list, `%` the aim, an id or a name through its episode,
      every reading collected — read by `:b`, `:Send`, `:decide` and
      `:resume`; `session_named` is it mapped to the episode, for the verbs
      that act on episodes (`:delete`, `:clone`, `:merge`, `:rename`,
      `:return`, `:child --stop`).
    - **A companion stays while its kind does** (M4): with (c) fixed, `:b#`
      between two trees closed the inspector and opened another (the user's
      sizing lost). One rule at the three doors (`show_in`,
      `show_from_history`, `enter_session`): release iff the companion
      kind changes. The review's P2 lands with step 1.
    - **Eleven sites ask what a window shows, not five** (M5): `View::buffer`
      is wired at all eleven (the six I missed: `listing_view`,
      `split_command`, `tree_for_graph`, `settle_tree_marks`,
      `renew_listings`, `enter_session`), and its doc names them with the
      count.
    - **No `Buffers::len`** (L7): `delete_flow.rs:1052` would count pages;
      `sessions().count()` says what it counts. The notice's "every buffer
      is on a fresh empty session" says "session".
    - **Minting is the list's** (L8): `Buffers::new(first)` and one
      `add(make: FnOnce(BufferId) -> Buffer) -> BufferId`, so a number is
      minted and pushed in one step at `page_for`, a tree's birth and
      `hold`. `BufferId`'s field private — decided at the build by the
      count of construction sites.
    - **`panes_holding` walks `App::windows()`** (L10), ordered by ring
      position, so T2 widens it in one place and a float counts.
    - Doc comments made false by (1)–(3) rewritten in the same commits
      (L11, sites listed in the review).
    - **Order** (Q6): (0) pins (P3); (1) the module move first, move only,
      so `Buffers` is written in `app/buffers.rs`; (2) the window half
      (design (1), with P1 inverted and P2); (3) `Buffers` with "what is
      buffer N" once, `buffer_named`, and `:ls` in one loop (design (2)–(4)).
    **Dropped:** item (d) (L9). Vim's `:ls` and Emacs's buffer list have no
    window column; extending "under" to pages and trees gives the column two
    meanings, and to every entry makes it a dump of each window's history.
    Docketed instead: vim's `#` flag (the focused window's alternate) —
    which needs the listing re-derived on a focus change.
    **Declined:** the buffer's id as its index (the review's lean) — no
    measured need at a handful of buffers, and it ties a number to a
    position before `:bw` exists to want tombstones.
    **My call, flagged** (the review's question): a tree held only in a
    window's history is hidden, and `:graph` takes it back — netrw's rule
    (a hidden listing is reopened), and two windows on it then share its
    marks as a split's do. Said if Steven would rather a held tree count
    as taken.
    **(0) DONE 10-25** — the `b1234` refusal's session half pinned
    (`b_on_a_session_number_that_begins_another_id_refuses`, `:b`, `:Send`,
    `:decide`); reading `bN` alone: RED.
    **(1) DONE 10-25** — `app/buffers.rs` (276 lines): `SessionBuffer`,
    `OnDisk`, `Contents`, `Listed`, `Unwritten`, `Billing`, and the test
    counter of `prompts`' scans; re-exported from `app` where other modules
    read them (`crate::app::Billing` in `view.rs`). Move only.
    **(2) DONE 10-25 — the window half.** `View::buffer` / `Window::buffer`
    (a session's, a page's, a tree's; `None` for an instrument);
    `Window::holds(id) -> Option<Holding { Shown, Beneath, Aside }>`;
    `Window::take(buffer)` (was `take_view(at)`, `take_session`, and
    `bring_back_held`'s search; `<C-^>` takes the newest by `give_back`);
    **the history a buffer once** (`set_aside` retains by `one_buffer`: the
    buffer, or for a test's bufferless tree its kind), `enter` taking the
    place out BEFORE the swap; `App::show_in` replaces only what the pane
    already shows (`Window::shows_as`), setting another tree aside — V5.3
    verify F1 fixed. **One companion rule**, `companions_follow(pane, to)`:
    released iff the companion kind changes, at the three doors with no
    reconcile after them (`show_in`, `show_from_history`,
    `enter_session`); `:q`, given back or closed, leaves them to the
    reconcile ending it — the rule's other half, the orphan rule. Measured
    on the way: a release before the give-back too left the same panes in
    the same places (`a_tree_given_back_puts_its_inspector_where_it_was`
    held without it), so the release-first reason the V5.3 verify left
    unmeasured was false, and the reconcile there is now the one mechanism
    rather than an equivalent second (F7, settled).
    `panes_holding(id, held)` walks `App::windows` in ring order;
    `panes_showing` = Shown | Beneath, `panes_on` (was `panes_on_page` and
    `panes_on_tree`) = Shown. Asked of `buffer()` now: `pane_shows`,
    `enter_session`, `show_page_buffer`, `show_graph_buffer`,
    `listing_view`, `tree_for_graph`'s shown set, `renew_listings`,
    `split_command`'s page copy and the walk above; `settle_tree_marks` and
    `tree_for_graph`'s own-pane check stay typed (they need the view, not
    only its buffer). Tests: the reviews' probes landed —
    `b_n_of_a_second_tree_keeps_the_first_as_the_alternate`,
    `b_n_of_a_second_tree_over_a_view_keeps_the_first_held`,
    `a_trees_inspector_stays_across_trees_in_its_pane`,
    `a_mark_spent_elsewhere_stays_spent_when_the_tree_comes_back` — and
    `a_tree_given_back_puts_its_inspector_where_it_was`,
    `a_page_set_aside_is_listed_hidden` (`panes_on` counting what is aside
    passed every test); `held_buffers` beside `history_of`, which names
    kinds (F6).
    **(3) DONE 10-25 — one buffer list, one reader of a buffer's name.**
    `app/buffers.rs`: `enum Buffer { Session, Page, Tree }` (`id`, `listed`,
    `described`) and `Buffers` — one `Vec<Buffer>` in number order, minted
    and pushed in one step (`add(make)`, the counter private; `new(first)`
    returns `b1`'s number, so `App::new` writes no `BufferId(0)`), typed
    `session/page/tree{,_mut}` and their walks, no `len` (L7), a test-only
    `skip_to`. `App::buffers` replaces `buffers`, `pages`, `trees` and
    `next_buffer_id`; `mint_buffer`, `buffer_numbered`, `page_named`,
    `tree_named` and `unlisted_described` go. `buffer_of{,_mut}` →
    `session_of{,_mut}` (180 calls). **"What is buffer N" once**:
    `show_buffer` one `match`; `:ls` one loop over the list, the sort gone.
    **One reader of a token** — `App::named(token) -> Named { Buffer(id),
    Session(episode) }`, every reading collected (`readings`): `bN` by its
    number, of any kind; `%` the aim's buffer; ids and names as stored
    sessions. `:b` acts on it; `session_buffer_named` (a page's or the tree's
    number refused as what it is) for `:Send` (`send_target`, then
    `open_named`), `:decide` (found, never opened) and `:resume`;
    `session_named` maps it to an episode for the episode verbs;
    `session_readings` (`:open`, `:rename`) is its sessions as episodes.
    **Changed, my call (flagged):** a token that numbers a page AND begins
    a session's id is refused at every verb, as `:b` refused it — `:delete
    b1234` took the id's session, past the page `:ls!` lists as b1234.
    The `:delete all` notice says "every open session is now a fresh empty
    one" (it said "every buffer", false once pages are buffers, L7). Tests:
    `b_n_names_a_buffer_by_its_number_not_its_episode` (two buffers on one
    episode, set up: through the episode `:b 3` showed b1),
    the `b1234` refusals at `:delete` too, a page's number refused alike by
    `:resume` and `:decide`, `b_on_a_session_number_its_own_id_begins_shows_it`
    (one session read two ways is one reading) and
    `delete_all_counts_sessions_not_pages`. Mutation 7 of 7 RED (two GREEN
    on the first run answered by those last two tests). The manual
    (`*shannon-sessions*` neighbourhood) says a page's number and a
    session's id is refused like two sessions.
  - Not precluded, for later: a float (a "balloon") is a window the
    layout places off the tiling — V5.3's window is independent of where it
    is drawn; overlays (decorations on a buffer's ranges) — content and
    decoration kept apart where V5 touches them.
- V6 — `:help shannon-buffers` and the manual; `feature-checklist.md`.
  **DONE 10-21** with §9 phase 7: `*shannon-buffers*` already said pages
  are buffers (V3); the manual gained `*shannon-sessions-apart*`, and
  `feature-checklist.md` a hand-driven section for each (pages are
  buffers; sessions run apart).
- Then tabs T2 (`tabs.md` §6), on windows that show buffers.

Not in it: the prompt as a buffer (one prompt; a second is its own design);
saving viewer buffers across runs; `:bd`/`:bw` (later, with `:ls`'s vim
shape — Steven, 10-20, image 21).

10.5 Decided — Steven, 10-20                              *buffers-viewer-open*

1. Viewers take numbers from the one counter, so session numbers skip
   (`b1`, `b2`, `b4`) — vim's way. "That sounds good to me."
2. `:analyze` is one buffer each command replaces, as `:help`'s. "Yes."
3. Graph marks are NOT shared: "No, treat each pane that shows a `:graph`
   as independent" — marks on the window (§10.2). **Refined 10-25**
   (Steven: "treat the graph the same way" vim treats netrw; "go with
   neovim's way"): each `:graph` is a tree of its own, so two `:graph`s
   stay independent; but the marks are the TREE's, so two windows on ONE
   tree (`:sp` of it) share them, as netrw's local marked list is its
   buffer's (§10.4 V4b.4). **Refined again 10-26**, netrw measured again
   (one buffer per directory, every spelling): ONE tree per store, every
   window on it sharing its marks — "we only have to handle the one 'file'
   but just in different views". The 10-20 "independent" now holds of the
   windows (cursor, folds, scroll), not the marks.

10.6 Order with §9                                        *buffers-order*

§9's phases 4–7 (one turn per session) were not yet built when this was
written (10-20; built 10-21 — §9's as-built notes); their calls are
answered above (10-20). Proposed, to Steven: V1, V2 (no behaviour change);
§9 phase 4 (one focus door, one landing rule — V4's window history builds
on it); V3–V5; §9 phases 5–6 (by-id turns, then the flip: sessions
independent, a decision blocking only its own); V6 and §9's phase 7 (the
docs); then a FULL review of all of it (Steven: "You should certainly have
a full review after buffers are fully implemented"); then tabs T2.
**Approved 10-21** ("2) Approved … keep going"); V2 since folded into V3.
