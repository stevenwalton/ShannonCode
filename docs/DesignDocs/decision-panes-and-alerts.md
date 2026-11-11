# Decision panes and alerts

DECIDED 2026-10-22. Steven's answers are in §6, and the design review in
§9. The build order is §7. A1 (decisions per session, A1.0–A1.5) and A2
(reviews as decisions, §4.1) are built; the review stop is next. §2
describes the code as it was before A1.

## 0. Where this comes from

Two answers to the full review of 10-21, and a third remark.

**D 4.** A reviewed summary opens `$EDITOR` the moment it arrives, taking
the terminal mid-typing. Steven:

> "I think what might work best is using a subpane prompting for review.
> Similar to how we use the tool ask pane. That also gives us a shared
> primitive, the child pane. When the user navigates to it they may accept
> the review and be brought to $EDITOR. This is then an informed and
> expected event. This gives us a generalized principle."

**F 4.** A result that lands in no pane is said only by a notice, which
the next key clears. Steven:

> "I think this is right. We should ensure that this is modifiable though
> because people will want to evolve what the alerts can do. We should be
> able to define triggers like this and have a visual display outcome."

**And, 10-22:**

> "The decision pane should also be associated with panes it is making
> decisions for."

The philosophy behind all three is the same: an environment people shape
to their own use, built from small parts that compose (Unix), and kept
elegant.

## 1. The principle

**Something that arrives and needs you gets a pane attached to its
session. You go to it; it never comes to you.** The decision pane already
does this for a tool call (12a07f9, 10-19):

- it opens beside the session that asked, and focus stays where it was;
- unfocused, its title says how to reach it;
- its keys wait `tui.approval_delay_ms`, so a keystroke already on its way
  answers nothing;
- answered from inside, it stays for its session's next call.

Under this rule the `$EDITOR` of a review is a key pressed in the pane,
which is synchronous: you asked for it. That keeps invariant 8 ("takes no
terminal") without a special case.

## 2. What exists, and what it costs

**One decision pane for the whole app.**
- `PaneKind::Tool` is titled "decision". It shows `App::tool_prompt`, one
  call at a time.
- Every other session's calls wait in one `tool_queue`.
- The rest is app-wide too: `decision_waits_for` (held for a session's
  next call), `answers_from` (the arming), `last_tool_width` (the width
  the report is measured at).

The counts, outside comments (DR-A):

| state | sites |
|---|---|
| `tool_prompt` | 45 (+1 in `setup_flow.rs`) |
| `tool_queue` | 17 |
| `decision_waits_for` | 13 |
| `answers_from` | 8 |
| `last_tool_width` | 5 |
| `PaneKind::Tool` | 47 |

Much of that is cross-session bookkeeping, there only because one pane
serves every session:
- the title's ", next another session's";
- `decide_tool_call` closing a pane held for another session;
- `settle_decision_wait`'s check that the next call is "ours";
- `cycle_call` handing the pane to another session;
- tabs.md's open Q1.

It is the "parallel mechanism means a fused abstraction" smell: the
decision belongs to a session, and the code keeps re-deriving which one.

**Two ways a summary waits on you today:**
- the event loop's stream arm hands a reviewed distillation straight to
  `$EDITOR` (`review_and_commit_summary`), which is the takeover;
- a review quit without a save asks `ConfirmAction::ApplySummary`, a y/N
  question that takes every key (`route_key`) and holds its session.

## 3. Decisions belong to sessions

### 3.1 The model

A session's decisions are part of the session (its buffer); a decision
pane is a window onto them. Geometry belongs to the pane, not to the
decisions.

    SessionBuffer.decisions: Decisions {
        shown:   Option<Decision>,       // the one in its pane
        waiting: VecDeque<Decision>,     // calls first, then reviews
        answers_from: Instant,           // the arming, per session
        kept_open: bool,                 // held for the next call
        left: bool,                      // a review's pane closed by you
    }
    enum Decision { Call(ToolPrompt), Review(Review) }
    PaneView { decides: Option<(BufferId, PaneId /* host */)>,
               drawn_width: u16, .. }

**As built (A1.1–A2.2).** The state lives in `app/decisions.rs`, one map
on the `App` rather than a field on each `SessionBuffer`: `Decisions {
sessions: BTreeMap<BufferId, Session> }`, its fields private to the
module, a session with nothing having no entry. `Session { shown:
Option<ToolPrompt>, waiting: VecDeque<Waiting>, review: Option<Review>,
kept_open, hidden, answers_from: Option<Instant> }`: a review is a slot
beside the calls, not a `Decision` variant in one queue (§4.1), and
`left` became `hidden`, set by `:hide` (§3.6). What a pane shows is
`OnPane::{Call, Review}`; its answer, `Answer::{Call, Review}`. The pane's
view records what it shows by identity, `shows: Option<Shown>` (`Shown::
{Call(id), Review}`; A2.4, review stop 8 D 5).

### 3.2 Every door names its session

No function resolves "the one call". These all take the call or the
session as an argument (DR-A #4):
- `resolve_open_call(id, …)`, `close_decision_pane(b)` and
  `ToolKeyEffect::Edit(b)`;
- `tool_call_done`, which reads the session before the record is
  forgotten;
- the edit doors (`tool_edit_seed`, `apply_write_edit`, `apply_tool_edit`),
  `propose_rule` and `cycle_call`;
- withdrawals (`withdraw_decisions`, `take_provider_question`).

**"Held" is checked by pane identity:** the focused pane is this
session's decision pane. A policy change re-judges every session's shown
call, not one (`revisit_open_decision`, DR-A #1). The one iterator over
every **call** awaiting a decision, every session's — shown or waiting — is
`open_decisions()`, and its readers are `work_said`, `quitting_ends` and
`leaving`. One session's: `Decisions::calls_of(b)`, and **two questions,
each named** (review stop 8, D 1): `holds(b)` — a call shown or waiting,
its record standing, or its review — is what gates read (the busy hold,
a turn's resume, `:ls`'s "needs you"); `on_pane(b)` — what its pane shows
— is what you owe now, and every sentence telling you to act reads it.
They differ while an allowed command runs and the next call waits unasked:
held, nothing to answer yet. Reviews come from `Decisions::every_review`;
what a pane shows, from `every_on_pane`. Setup's refusal and its bar read
`decision_in_a_pane` (stop 8, C 5).

### 3.3 Placement is diff-based and sticky (DR-A #5)

One rule runs at the event's end: `settle_decisions`, run BEFORE
`settle_focus` (DR-A #6). It does two things.

**It opens a decision pane** when a session has something shown or kept
open, has no pane yet, and a window shows it. That's its own window, or
one a viewer has borrowed (`panes_with_session`), in any tab, which is tabs.md
Q1 (a).
- The host is the focused window if that shows the session, else the
  first.
- The size is the one you last gave a decision pane: `decision_place`,
  recorded from the pane you actually resized or moved (DR-A #2).
- An opened pane re-arms (DR-A #7).

**It closes a pane** only when its host stops showing its session. Then
it reopens under another window that shows the session, if one does.

Otherwise it never moves, resizes or reopens a pane. The workspace gets
id-keyed doors: `open_decision(near, rows) -> PaneId`,
`fit_decision(id, rows)` and `close(id)`. The kind-keyed
`show`/`hide`/`first_of` are no longer used for decisions. Each pane is
fitted at its own drawn width.

### 3.4 Keys, arming, the legend and the row

- **Keys.** `handle_tool_key` reads the focused pane's `decides`, then
  that session's `shown`.
- **Arming** is per session. `answer_keys(pane)` reports Selecting and
  Chord only for the focused pane, and Waiting from that pane's session,
  so an unfocused legend never shows another pane's state (DR-A #13). The
  event loop wakes at the earliest arming.
- **The row describes the aim, wherever you stand**, as recorded in
  `buffers.md` §9 ("the status line is always associated with the session
  we're aiming at"). That stays: standing in another session's decision
  pane, F 7's prefix names whose activity it is (DR-A #10).
- **Every rule that holds today holds per pane:**
  - visible means answerable;
  - a chord or a selection owns the next key;
  - `Esc` answers nothing;
  - plain keys only.

### 3.5 Room, and a session in no window

**`$decide` is per session.** `decision_shown` becomes a map from session
to what was drawn. `$decide` names each session whose pane wasn't drawn,
or lost its keys: `DECIDE: no room — b3 · :decide 3` (DR-A #3). When
several are waiting it counts them: `3 waiting — :decide 3`.

**A session in no window keeps its decision waiting.** It is announced:
- on the row (`$hidden`, below);
- on its `:ls` row, which already says "needs you";
- by its tab's bell, later.

**As built (A1.5): `$hidden`, at the row's right.** Steven, 10-22: "We can
show hidden pending decisions in the status line (as a variable we allow
for the user defining their status line). I think by default it should go
in the bottom on the right side." So a decision with no pane — in no
window, or `:hide`n — is `$hidden`'s (`decision hidden · :decide 3`, or `2
decisions hidden · :decide`), and `$decide` keeps to a pane that cannot
show its keys. Two pieces of grammar made it a default rather than a
special case: `$fill` (starship's, vim's `%=`) is the room the rest leaves,
so `{ $fill$hidden}` ends every shipped row; and a join (`$alerts`,
`$activity`) lists only what the format does not write itself
(`Format::places`), so `$hidden` placed at the right is not said again
beside the pane's name, and a row of yours with `$alerts` alone still says
it. That second rule is B1's too: `${alert.x}` written in a format leaves
`$alerts`.

**`:decide [N]`** shows session N, or the aim, or else the first one
waiting. It shows the session where `host_pane` puts it, focuses the
session's decision pane by id, and re-arms. You asked, so focus moving is
right here.

This replaces "a decision must always get a pane", whose need (a turn
never waits with no visible way to answer it) is met by the row and
`:decide`. Two callers put calls into sessions in no window:
- `:Send bN` and `:Send ball`: these wait and are counted;
- `:tool`, which you just typed: its session is shown first.

Claude Code's hook deadline (59 minutes) runs while a call waits. The
`:ls` row says the time left (DR-A #16).

### 3.6 Closing, door by door

| Door | A call | A review |
|---|---|---|
| `q` in the pane | deny | refused, naming `:hide` and `d` |
| `:q`, `:close`, `<C-w>c`/`<C-w>q` on the pane | deny | refused, naming `:hide` and `d` (A2, below) |
| `:hide` on the pane | hidden: waits, said by `$hidden`, until `:decide` | hidden, the same |
| `:q`, `:close`, `<C-w>c`/`<C-w>q` on the chat it sits under | refused, naming `bN` and `:hide` | refused, the same |
| … with a second window here on the same session | closes; the pane goes under the other (stop 11, F 6) | the same |
| `:only` from such a second window | keeps the pane, under the window kept | the same |
| `:hide`, `:q!` on that chat | the chat and its pane close; waits, hidden | the same |
| `:only`/`<C-w>o` elsewhere | refused, naming `bN` and `:only!` | refused, the same |
| `:only!` elsewhere | closes; waits, hidden | the same |
| `:tabclose`, `:tabonly` over a tab holding it | refused, naming the tab, `bN` and the `!` | refused, the same |
| `:tabclose!`, `:tabonly!` | closes; waits, hidden | the same |
| its session's window shows another (`:b`, `:open`) | waits, its pane gone with its session | waits |
| `:qa` | refused, named in the quit's ends | named, the same |
| `:qa!` | cancelled with the quit, audited so | dropped with it |
| `:q!` on the pane | hidden, as `:hide` (Steven, 10-26: "close as unanswered"; it denied) | dropped, said |
| `:wq!`, `:x!` on the pane | hidden, as `:q!`, once written | refused, as `:q` (review stop 8, A 5) |
| `:q` etc. on a call's pane, a review waiting behind it | deny the call; the review comes up next, as a waiting call would | — |
| `:q`, `:hide` on a pane kept for the next call | closes; nothing hidden, the next call opens a pane | — |

**The rule (Steven, 10-26: "It is important that we don't answer the
request, the user must do that"):** a close answers nothing but the one
made in a call's own pane — `:q`, `:close`, `<C-w>c`, `<C-w>q` there deny,
as `q` does, and claim the pane they answered (`close_pane_command`); its
`:q!` does not, closing it unanswered as `:hide` does (Steven, 10-26:
"`:q!` denying seems non-obvious to me. It reads to be as a (force) quit
function"). Any
other close that would take a request's pane is refused, naming whose
(`held::HeldWork::why`, remedy first since stop 11) — the chat it sits under included, since
the chat is what the request is decided by ("let's make sure to not close
the associated chat too"). Forced — the `!`, or `:hide` on the chat — it
closes, "not a deny, a close": `reconcile_pane_state` reads the decision
window given back unclaimed and leaves its request waiting, hidden, as
`:hide` in its pane does; `:decide N` brings it back. Hidden wherever its
session is shown, or `:only!` would have the pane open again at once over
the room it made. A pane kept open for a next call holds no request, so
it closes with anything and nothing is hidden. Closes made by placement
answer nothing (§3.3).

*Before 10-26* the reconcile denied a decision window closed while its
session was shown, so `:only` here denied a call whose session a parked
tab's window kept "shown" (r12-F M1), and a forgotten claim answered.
Now a door that forgets to claim fails safe: waiting, not answered.

**A close answers what the pane was on** (review stop 8, A 8 / C 10; mine,
for Steven to overturn): a review is hidden only when its pane showed it
(`PaneView::shows`), so a call's pane closed over a review denies the
call and the review comes up as a waiting call would. It was hidden, where
`q` on the same call brought it back — two denies, two ends.

**As built (A1.4): a decision pane goes with the window it sits under.**
`:only` keeps the focused window and its decision panes — and, typed in a
decision pane, keeps its host (`only_command`). This replaced "`:only` from
inside the pane closes its host, so its session leaves the screen and the
call waits": typed for room, `:only` made the question vanish, a Norman
door. It is Steven's "associated with panes it is making decisions for",
and the child-pane primitive of D 4: a child goes with its parent. The
no-room advice is "`:only` from its conversation, or a taller window" (DR-A
#8); a key refused in the undrawn pane itself says "`:only` for room",
which now holds there too. The manual's table in `help.rs` followed in the
same commit.

**Also as built:** the closed pane is known by its view (`PaneView::decides`)
outliving it at `reconcile_pane_state`; every close of ours
(`close_decision_pane_by_id`, placement or an answer) and your deny in the
pane claim the view with the pane, so only a forced close is read as one,
and said ("the request waits, hidden: `:decide N`"). A call waiting for
want of a window gets no notice of its own: the row says it, as for
placement.

**`:hide` keeps it (A1.5; Steven, 10-22: "`:q` removes from the buffer
while hide doesn't").** Vim's two verbs: `:q` abandons a buffer, `:hide`
keeps it hidden. So `:q`/`:close` deny a call, and `:hide` closes the pane
by id and marks the session's decisions hidden (`Decisions::hide`): no pane
opens for them, not when the conversation is shown again — you hid the
decision, not the session — until `:decide`, or a `:tool` typed in that
session, whose call waits behind it. Hidden is forgotten once the session
has nothing left, at the event's end (`forget_idle`): within an event its
last waiting call is taken off to be shown, and the session holds nothing
for that moment, so a door's tidy cannot be where it ends.

**A review's `:q`, decided by me for A2, for Steven to overturn:** a review
is paid work, the modified buffer of vim's E37 — `:q` there refuses,
naming `:hide` (keep it) and `d` (drop it), rather than dropping a draft
or keeping it against the verb's meaning. §4's table follows.

### 3.7 What this deletes

- `tool_prompt`, `tool_queue`, `decision_waits_for`, `answers_from` and
  `last_tool_width` as app-wide fields;
- ", next another session's", and `ToolPrompt::next_elsewhere`;
- `settle_decision_wait`'s cross-session close;
- `cycle_call`'s hand-off: `n`/`N` walk this session's own calls;
- `fit_tool_pane`'s "any session pane, then the pane you stand in";
- the kind-keyed decision placement in the workspace.

## 4. Reviews are decisions

**On arrival** the review is pushed onto its session's decisions inside
the summary's `Complete` arm, in the same event and before `settle`
(DR-B #1). It's queued behind that session's calls. Nothing opens
`$EDITOR`. Its pane follows §3. The title says `{n}^Ww to review`.

**A review holds its session from the moment it arrives.**
`Decision::holds_slot()` (as built: `App::review_holding`, §4.1), with a
phrase and a recovery for each kind, is consulted by:
- `busy_reason_and_recovery` (first), so a prompt queued behind it says
  "b3's summary review waits — `:decide 3`";
- `start_job_on`, so a `:Send` waits too;
- `quitting_ends`, so `:qa` names the draft rather than dropping it.

The wait is now unbounded: a review you leave keeps its session's prompts
queued, and the queue says why.

**Its other readers move with it** (DR-B #2):
- `:delete` of the session drops the review and counts it ("deleted b3,
  with a summary draft");
- CTRL-C answers nothing in its pane, and its hint has a review form;
- setup's refusal names `:decide`.

`ApplySummary` and its `holds_session` arm go.

**In the pane** it shows the draft, rendered and with secrets masked
(`row_secrets`), not the editor's JSON. The pane is capped in height. It
also shows what the draft was made from ("2 turns of 5da64e5a"). Its keys:

| key            | does                                                     |
|----------------|----------------------------------------------------------|
| `<CR>` / `e`   | review it in `$EDITOR`, the event you asked for; `:wq` applies |
| `a`            | apply it as drafted, unreviewed                          |
| `d`            | drop it; the notice says what was dropped                |
| `:hide`        | leave the pane; it keeps waiting (`$hidden`, `:decide`)  |
| `:q`           | refused, naming `:hide` and `d` — vim's E37 (§3.6)       |
| `/` `n` `N`    | search the draft (a review has no calls to walk)         |

- **Keys resolve from the whole `KeyEvent`.** `Decision::resolve(&KeyEvent)`
  is the one answer that both the answer path and the no-room guard read,
  since `<CR>` is not a `Char` (DR-B #8).
- **The keys arm** like a call's.
- **The legend, the "not a decision key" text and the arming text** are
  read off the decision. **As built (A2.4):** each kind builds one
  `decision_page::KeyTable` from what it is now (a call's from its
  verdict, proposal, editability and queue; a review's from what `$EDITOR`
  last did), and `resolve`, the legend, the status row's short form and
  the refusal are its projections — so they cannot disagree (review stop 8,
  C 8 / D 4: a denied call's short form offered `a allow`). `A` with no
  rule is the one row that resolves unlisted. The page also carries its
  title and its subject (`DecisionPage::title`, `subject`; C 13), so the
  view draws either kind through one call.

**After the editor:**
- `:wq` composes through `land_summary_review`, as `Landing::Asked`. The
  landing reads the pane's host window as where you stand, so answering
  from the pane never retires a `:graph` you were working in (DR-B #6).
- Quitting without a save keeps the review in the pane, marked "closed
  unsaved". That's a flag on the review, not a third kind (DR-B #10).
- A failed `$EDITOR`, or `:cq`, also returns to the review with the
  reason. Paid work is dropped only by `d` (DR-B #9).

**The words on screen that change** (DR-B #16):
- "it opens for review when it is written" at the press;
- the `$distilling` comment;
- `help.rs`'s review paragraphs.

When the session isn't shown at the press, the notice says where the
review will wait (`:decide N`).

### 4.1 As built (A2.2, 10-22)

- **A session's review is a slot beside its calls**
  (`decisions::Session::review`), not a variant of one queue. At most one:
  the review holds its session from arrival, so no second distillation
  starts there. **Calls first, exactly:** the pane shows the review only
  when the session has no call shown and none waiting (`Decisions::on_pane`)
  — an allowed call running with another waiting keeps the pane for the
  calls. A call you type while the review is up takes the pane (one typed
  is the only kind: no turn runs in a held session), its keys armed as any
  new call's; answered, the review comes back to the same pane.
- **One page, two kinds** (A2.2a, `decision_page::DecisionPage`): the
  review is drawn, fitted and folded by the call's rules; `OnPane` is what
  a pane shows, and `OnPane::resolve` the one answer the keys and the
  no-room guard read (a review's `<CR>` is no character).
- **Whatever comes onto a pane is armed and fitted at the event's end,
  once** (`settle_decisions`, by what the pane's view records it shows,
  `PaneView::shows`; A2.4), not at the doors it arrives by: a call shown,
  the pane opened or come back, and a review — after an allow it comes up
  while the command runs, after a deny at once, and arriving at a pane
  kept open. (A burst of keys settles per event, so a key on its way
  answers nothing.)
  `settle_decision_wait` counts a review as the session's next, so the pane
  you answered calls in is the one it takes.
- **Keys:** `<CR>`/`e` editor, `a` apply as drafted, `d` drop, `q` refused
  naming `:hide` and `d`; `j k / n N` the page's. `:q`, `:close`, `<C-w>c`,
  `<C-w>q` refused (E37); `:q!` drops; `:hide` hides; `:only` from another
  window on the session leaves it hidden, said.
- **Applied from its pane, it composes standing in its host window**
  (DR-B #6 as built): the decision pane hosts nothing, so the landing's
  reveal went to the aim's window — the `:graph` borrowing your only
  transcript.
- **The editor round trip** (A2.3, folded in: without it the review could
  only be applied as drafted or dropped): saved composes, unsaved marks it
  `closed unsaved` and keeps it, a failed editor or `:cq` keeps it and says
  why.
- **Its readers:** `busy_reason_and_recovery` first, `start_job_on`,
  `quitting_ends` (`review_holding`); `:delete` drops it, counted in `Lost`
  ("with a summary draft"); `:qa!` takes it; setup refuses over one in a
  pane; CTRL-C's hint has its form; `$hidden`, `$decide`, `:decide`, `:ls`
  count it. `ConfirmAction::ApplySummary`, `holds_session` and
  `question_holding` are gone; so is the event loop's terminal hand-off
  (`dispatch_stream_msg` returns nothing).
- **Decided by me, for Steven to overturn:** `:q!` on a review drops it
  (vim's abandon), where the design said only `d` does; `q` refuses rather
  than drops; a review in a hidden session stays hidden until `:decide`.
- **Review stop 8's fixes** (`docs/Reviews/2026-10-22-stop8/`):
  - a compose that refuses — a save that does not parse, a session changed,
    a write that fails — keeps the review: taken only once it composed
    (A 1 / B 1); a refused save is kept on it, `<CR>` opening it again
    (`your edit not applied`); a save emptied is `closed unsaved`;
  - the review holds the session it replaces as well as its own
    (`SquashTarget::replaces`), and a turn cut from a held session is
    refused (A 4) — mine, for Steven to overturn;
  - an empty reply composes nothing and puts nothing up (B 3);
  - the call's and the review's keys are one path (`OnPane::resolve`, C 4);
    "calls first" is stated once (`Session::front`, A 2 / C 1), and
    `wants_a_pane` reads it (D 1); every refused key restarts the wait (D 2);
  - held with nothing to answer yet — an allowed command running, the
    session's next call behind it — is said as that, not "answer it" or
    `:decide N` (D 1; `App::review_reach` for a review behind calls);
  - deleting the session a review would replace drops the review, counted
    (D 3) — mine, for Steven to overturn (the other option: refuse it);
  - a teardown answers nothing by proxy: deleting a session cancels every
    call shown or waiting there before its work stops, as `:qa!` does
    (E1 1); a call coming into view by room waits again, and a key before
    any frame drew it is refused (E1 2); a chord owns its key ahead of the
    room guards (E1 5); an answered call keeps its session's pane however
    it was answered (E1 6, mine);
  - every call a policy change answers is named, with its session (E1 4,
    E2 3); an allow Claude Code did not get, or one at its deadline, is
    said and audited as a withdrawal (E2 6); a marked secret is drawn as
    one on the call's pane (E2 5).

## 5. Alerts are rules

Today `$alerts` is a fixed set: `$decide $queued $running $interrupt`.
Each is a `Var` with its own text, and `Var::is_alert` hand-lists them for
the width rule.

### 5.1 The shape

It follows the house precedent for shipped items you can remove, as
`[permissions] remove` and `[skills] disabled` do (DR-B #11):

    [tui.alerts]
    glyph    = "🔔"                # the bell `:ls` and the tabline wear
    disabled = ["queued"]          # stacked over the shipped list

    [tui.alert.ctx]                # one table per alert
    when   = "ctx_percent >= 80"
    say    = "[ctx $ctx](bold fg:red)"
    border = "ui.caution"
    mark   = true
    bell   = false
    notify = false

- Shipped alerts sit in `Config::default()`, so `:set` reaches them and
  `:config dump` shows them. A table you write deep-merges over a shipped
  one of the same name.
- Names are `[a-z0-9_]`. Both structs deny unknown fields, so a misspelt
  key is said at load.
- **`$alerts` lists every live alert**: shipped order, then yours. Another
  order is written in the format itself, with `{ · ${alert.decide}}{ ·
  ${alert.ctx}}`, so position is still priority.
- `${alert.name}` needs the format grammar to take a `.` in a name (DR-B
  #12).

### 5.2 Facts and triggers

**Facts** are numbers. They aren't `Var`s: they have their own name
table, a `facts!` macro checked by the same list test (DR-B #18). Each
fact has a scope (DR-B #5).

| scope | facts |
|---|---|
| per session | `decision`, `review`, `unshown`, `ctx_percent`, `queued`, `running`, `answering` |
| app-wide | `unshown_count`, `unshown_first`, `sessions`, `children`, `sends` |

- Facts are worked out in `before_frame` (the one mutable hook before a
  draw), and only the facts some live `when` names.
- The cost is O(sessions × facts named) per frame, and never O(history).

**A trigger** is a fact, or a fact compared with an integer (`>` `>=`
`==` `<` `<=`).
- Errors say what to write instead: `ctx >= 80` gets "the fact is
  `ctx_percent`", and `and`/`or` gets "write two alerts".
- A broken `when` shows as `[name: when?]` in `$alerts`, never silently
  dead.

A per-session alert holds for each session where its fact does.

### 5.3 Outcomes, all optional

**`say`**: a status format. `$alerts` and `${alert.x}` are pieces the
renderer expands inline, so styles survive. Either one inside a `say` is
refused at load, since that would recurse (DR-B #4).

**`border`**: on each session window where the alert holds. Colour stays
focus > aim > alert, and the alert also sets a channel neither of the
others uses: a thick border or a title badge. The aim's accent is a
safety channel (DR-B #13). "Its window" means visibly shown.

**`mark`**: the glyph, in its own `:ls` column, padded by cells, not
chars; later on the tab that shows the session.

**`bell`** and **`notify`** fire on an edge keyed by (alert, session).
- The edge state is seeded silently on the first frame and after
  `:reload`, so nothing rings at launch. Edges append to an outbox that
  the loop writes through the backend after `terminal.draw`, one BEL a
  frame at most (DR-B #15).
- **`notify`'s payload has one door** (DR-B #3, security): its words —
  a template of `$alert` and `$session`, never the `say`, so no model
  text leaves the terminal (§7, B2.3) — scrubbed, filtered (every
  control, C1 included, and what `shannon_core::text::misleads`: the bidi
  embeddings, overrides and isolates, the line and paragraph separators,
  tags — the OS's notification centre runs the bidi algorithm and breaks
  lines where a terminal cell does neither; review stop 9, E2), scrubbed
  again, then cut. Every language's letters, marks and joiners pass.
  Any other order leaks (§7). A test feeds it an `ESC ] 52` payload, and
  an override in a name.
- The encoding is a pure function: OSC 9 by default, and the tmux
  passthrough wrap when asked. Tmux needs `allow-passthrough`, so
  `:help` says it.
- The shipped `unshown_session` rings and notifies; `[tui.alerts] bell`
  and `notify` turn every alert's off at once. This said "off" until
  Steven's 10-23 answers (§7, B2.3): users decide, from on.

**Role words** (`ui.caution`) in `say` and `border` stay symbolic and
resolve at draw, so a theme change shows on the next frame. Every shipped
theme has them.

### 5.4 F 4's `unshown`

`unshown` is a buffer whose revision changed since a pane last showed it,
by something that arrived (below).
It needs a revision counter on the session, bumped at its doors, which get
counted (DR-B #14). It is derived from the outcome, as `settle_sight` is.
It also covers a `:Send` reply or a turn ending in a session no pane
shows, which a landing-only mark would miss. "Shown" means drawn, not only
present in the layout: what a frame draws is asked once (`App::frame_draws`),
by the draw and by sight alike. A viewer lent over a transcript, a screen
over the whole frame, and a float (`:memories`, `:skills`: 90%×80% of the
frame, the transcript's tail under it) show it not; `:effort`'s panel
leaves it readable, so it is shown (Steven, 10-23: "it isn't seen, so
therefore it is hidden"). **And drawn is not seen while you are not
looking** (Steven, 10-24: "the window not being in focus is not considered
seen … the ring is about if the user is 'looking' when the event
happens"): a turn can last over an hour while you work in another window,
and its end is what the bell is for. Shannon asks the terminal for focus
reports (`?1004`; tmux needs `focus-events on`); a lost focus makes every
pane draw nothing seen (`App::looked`, read by `settle_seen`), a regained
focus or any key, click or paste ends it, and a terminal that never
reports is taken as looked at. Focus reports are off while `$EDITOR`
runs: an editor that did not ask for them reads `ESC [ O` as keys.

**What is news is said by whoever makes the change** (Steven, 10-23: "your
own actions are not news. You explicitly did it"). It was inferred after
the fact, from "changed and idle", so each door decided by accident: your
`:delete` or turn delete of a session no pane drew rang "news in" the
session left in its place, a `<C-c>` on a hidden `:Send` rang for your own
echo, and one arrived summary was news over a hidden buffer and silent
apart (review stop 9: A F1, D1, A F6). As built (`app/news.rs`):

- **One door gives a buffer a session**, whole (`App::hold`), with a
  `Landing`: `Asked` (launch, `:new`, `:open`, `:fork`, a checkout,
  `:delete`, a turn delete, a landing you asked for) is taken as it is at
  the next frame, without a word; `Arrived` (a landing nobody just asked
  for) has been seen empty, so anything in it is news, and its alerts held
  nothing before it. The same `Landing` decides whether the result takes a
  viewport, so the two rules cannot disagree.
- **A change you make is yours** (`App::yours`): your echo, and what an
  ending you asked for does (a stop, a delete; `Outcome::landing`) — its
  tool round torn down (`⚙ cancelled`) and its seal (`App::ending`). What
  you had seen before it you have seen after it, and what it moved, of the
  session's alerts and the app's, is taken as it left things
  (`Edges::absorb`), so it rings nothing, while what arrived since the last
  frame, or arrives after it, still rises. For an ending, "before" is the
  moment before you ended it: the work's generation is already taken when
  the ending runs. So a `<C-c>` before a word of the reply marks nothing
  and rings nothing; after part of one it marks (unread) and rings nothing
  (review stop 9's verification, G 1 and G 4).
- **A failure is said where it happened**, a `:Send`'s and a `:child`'s
  as a turn's (`[error: …]` in the transcript), so the news it rings is
  there to read (G 3).
- **What lands over a buffer keeps that buffer's edges**: `queued`,
  `decision` and `ctx_percent` are asked of the buffer, so one holding
  throughout does not ring again; nothing-held-before is a new buffer's
  (G 5).
- **What arrives is news wherever it lands**, your own prompt's reply
  included: under a viewer lent over its pane it is unshown, and rings
  (Steven, 10-23, of the ring when a turn ends: "that's a useful signal").

## 6. Steven's answers, 10-22

1. **The name:** keep "decision pane".
2. **Dismissal:** "Let's use `:q` to quit the decision pane." On a review,
   ~~`:q` leaves and the review waits; bare `q` is refused, naming `:q`~~ —
   superseded by his later answer, "`:q` removes from the buffer while
   hide doesn't" (A1.5): `:hide` is what leaves a review waiting, and as
   built (A2.2, §4.1) `:q` on a review refuses as vim's E37 does for a
   modified buffer, `:q!` drops it, and `q` names `:hide` and `d` — mine,
   for Steven to overturn. A call's dismissal still denies (§3.6).
3. **Order:** tool calls first.
4. **Keys and arming:** "Sounds good". So `a` stays, and reviews arm.
5. **Triggers:** "Yes", facts and comparisons for now.
6. **Outcomes:** "All (options), and adding a bell icon to the tab if it
   is in a tab or a bell icon when listing in `:ls`. We should be able to
   change the character if a user wants." That's `[tui.alerts] glyph`.
7. **When:** "Yes", A then B, before tabs T2. Steven: "We should be
   getting the buffers done and then moving to tabs."

**Decided by me in review, for Steven to overturn:**
- the row keeps describing the aim, by his rule (`buffers.md` §9);
- ~~`:hide` on a call denies, since every dismissal of a call does~~ —
  overturned 10-22: "`:q` removes from the buffer while hide doesn't";
  `:hide` keeps it, hidden (§3.6, A1.5);
- a decision opens under a window showing its session, in whatever tab
  (Q1 (a));
- a failed editor, or `:cq`, keeps the draft;
- shipped alerts don't ring or notify;
- alerts are removed by `disabled`, not by a `show` list.

**Decided by me building A1.4, for Steven to overturn:**
- `:only` typed in a decision pane keeps its host (§3.6, "as built"),
  where this doc said it closes the host and the call waits;
- `:decide` with no N picks the aim's, else the first in NO window, else
  the first on screen — the one nothing else points to before one already
  drawn;
- `$decide` counts the rest beside the name, `DECIDE in X (+1 more): …`:
  trailing the command, the count went off a 150-column row with it;
- ~~`:setup` is refused only over a decision in a pane (DR-A #11);~~
  10-26 (Steven: "I'd open it up so these don't block", on condition that
  what a screen changes is reliable under it): `:setup`, `:models`,
  `:effort` and the shelf open over any decision, the legend or bar
  saying one waits; a turn waiting keeps the config it began with
  (`in_flight_config`), and a model change for its session is refused
  until it ends (`a_screen_over_a_waiting_call_changes_only_what_comes_
  after`). A y/N still holds them (`refuse_over_a_confirm`);
- ~~the `:ls` row's time left on Claude Code's 59-minute deadline (§3.5,
  DR-A #16) is not built: the TUI does not record when the hook asked.
  Docketed; it fits B1's facts.~~ Built in A1.5: the question says what is
  left of the hook's deadline (`ToolQuestion::answer_by`, an instant since
  review stop 8, A 6), and the call's
  record keeps it on the TUI's clock (`claude-code-backend.md`).

Steven, 10-22, on that list: "They look good to me, so no change." And:
"The `:ls` timer is also good. We should implement it. Also let's make
sure that in setup we mention the timeout limitation" — built in A1.5.

## 7. Build order

Each step is a green commit, mutation-checked; smoked where you'd see it.
Docs go in the same commit as the code.

**A1, decisions per session**, in five commits (DR-A):
- **A1.0, test seams.**
  - `shown_call()` is the one shown call, and panics if two sessions have
    one; `shown_call_in(b)`, `to_decision_of(b)` and `tool_key_in(b, c)`
    focus by pane id.
  - The ~250 test reads move to these, and the multi-session tests to the
    explicit forms.
  - No production change.
- **A1.1, one door for decision state.** The API takes identity
  (`decision_of(b)`, `decision_in(pane)`, `open_decisions()`, …) while
  storage is still app-wide, and §3.2's doors get their arguments. The
  commit carries the count: no reads of the old fields outside the door.
  No behaviour change.
- **A1.2, decision panes by id in the workspace.** `PaneView.decides`, a
  width per pane, and the place recorded from the pane resized. No
  behaviour change.
- **A1.3, storage on the session, N panes.** `settle_decisions`, re-arming
  on appearance, per-session `$decide`, every-session revisits, and the
  deletions in §3.7. A session in no window still gets today's fallback
  pane.
- **A1.4, a session in no window waits.** `:decide`, the close table
  (§3.6), the no-room advice, and `:tool`/`:Send`. Smoked. Built: see
  §3.6 "as built" and §6's A1.4 list.
- **A1.5, Steven's answers of 10-22**, in two commits:
  - `:hide` keeps a decision, hidden (§3.6); `$hidden` at the row's
    right, with `$fill` and a join leaving out what the format places
    (§3.5);
  - Claude Code's deadline: the question carries how long the hook will
    wait, `:ls` says the time left, a withdrawal past it says why, and
    setup says the limit.

**A2, reviews are decisions**, in three commits (DR-B):
- **A2.1:** `Decision::resolve(&KeyEvent)`, the edit effect dispatched by
  kind, and an editor seam for tests. **Built:** `ToolPrompt::resolve`
  takes the whole key, plain only (`keymap::is_plain`, moved there from
  `app.rs`), so the no-room guard and the answer path ask one question —
  `<C-a>` was refused as an answer by the guard, asked of its character;
  `editor::Editor` (the terminal's is `Suspended`) is what `e` on a call
  (`edit_tool_command`) and a summary's review open, so a test drives the
  round trip. The dispatch by kind lands with the second kind, in A2.2: a
  match of one arm is generality with no consumer.
- **A2.2:** a review as a decision. Arrival, `holds_slot`, the readers,
  `:delete`, CTRL-C, the keys, the close table's review column. The modal
  goes. **Built** (§4.1), in two commits: A2.2a the decision page
  extracted from the call, no behaviour change; A2.2 the review, with A2.3
  folded in.
- **A2.3:** `<CR>`/`e` to the editor and back, the "closed unsaved" flag,
  a failed editor keeping the draft, and the host window as where you
  stand. Smoked. **Built with A2.2.**

**Review stop:** reviewers A–F, at full strength, since this is the
consent surface.

**B1, alerts as rules:**
- **B1.1:** `$alerts` as expanded pieces, with today's four as shipped
  rules reproducing today's row byte for byte; `is_alert` narrowed.
  **Built** (10-23): five shipped alerts (A1.5 added `hidden`), each a
  `say` of its value, in `status_line`'s `Alerts`. `$alerts` and
  `${alert.<name>}` are format pieces (`Piece::Alerts`, `Piece::Alert`),
  and `is_alert` is gone: only an alert piece keeps its group, so `$hidden`
  written bare is a value like any other. That needed `${alert.x}` now,
  not in B1.4, because the shipped rows end in `{ $fill${alert.hidden}}`.
  A `.` is a name's only between braces, so `$model.` still ends a
  sentence. A `say` holding an alert piece is refused when read (§5.3).
  4160 rows measured identical before and after.
- **B1.2:** facts with scopes, and the trigger grammar with its errors.
  **Built** (10-23), with B1.4's config half so it has a reader:
  `[tui.alert.<name>] when/say` and `[tui.alerts] disabled`. A fact's
  scope is its type (`SessionFact`, `AppFact`), so a session's fact can't
  be asked without its session; the row asks the aim's. Facts are asked at
  draw, only for an alert being drawn. §5.2's `before_frame` waits for
  B2.3, the first reader needing a frame's facts kept until the next. Your
  alerts follow the shipped ones by name. Shipped alerts sit in
  `Config::default()`, and a file's table lays over them key by key: the
  config is folded from the files alone, so a map they name would
  otherwise be theirs wholesale. `glyph`, `border`, `mark`, `bell` and
  `notify` come with their outcomes in B2. `:help shannon-alerts`.
- **B1.3:** the revision counter and `unshown`. **Built** (10-23). The
  revision is the transcript's (`Output::revision`), drawn from the
  counter the blocks' versions use. Six doors move it, and a test names
  each. `seen` is settled in `before_frame` from the layout the frame
  draws, so "shown" means drawn with room, never a viewer over the pane.
  A buffer not yet seen is seeded silently. Facts `unshown` and
  `unshown_count`; `unshown_first` became `$unshown`'s `:b N`, since a
  buffer's number is nothing to compare. No shipped alert says it yet.
- **B1.4:** `[tui.alerts]` and `[tui.alert.*]` in the config (`${alert.x}`
  built in B1.1; the tables, `:set` and `:reload` in B1.2). **Built**
  (10-23), the rest: role words in every format, kept as the role and
  asked of the theme at draw (`ui.caution` alone is all its style,
  `fg:ui.caution` its colour). No `ui.alert` role was added: the existing
  roles serve, and every theme has each. Rendered-cell tests.

**B2, outcomes:**
- **B2.1:** `border`, as a fourth input to the pane's block. **Built**
  (10-23), on both transcript paths. The colour is focus's, then the
  aim's, then the alert's, and the heavy line is the alert's alone (a title
  badge was the alternative; the line needs no room). A border on an alert
  that holds for no session is said at load.
- **B2.2:** `mark`, as the `:ls` column. **Built** (10-23). The column
  appears only while some alert marks. The glyph is sanitized and capped at
  four cells, since a project's config can set it and it reaches cells.
- **B2.3:** `bell` and `notify`: the outbox, the payload door, the pure
  encoding, and the write after the draw. Smoked, capturing the pty's
  bytes. **Steven's answers, 10-23**, which change §5.3:
  - Ship `unshown` and `unshown_session`. `unshown` holds only once its
    session is idle, so a bell can't ring on your own `:Send`.
  - Bell and notify are ON by default for `unshown_session`, with master
    switches in `[tui.alerts]`.
  - `notify` is a template (`$alert`, `$session`), not the `say`: no model
    text leaves the terminal.
  - tmux is chosen by hand (`notify_via`), not by reading `$TMUX`.
  - Bell and notify need a `when`.
  - A session is seeded silently the first frame it is seen.

  **Built** (10-23). Pure parts live in `crate::ring`. The edges are
  `App::settle_rings`, re-seeded when the alert table changes. The bytes
  are written in `event_loop` after the draw, and smoke §12 captures them.
  The door sanitises, then scrubs, then cuts, and each other order leaks:
  a control character hides a secret from the scrub, and a cut leaves its
  prefix. Words that do not begin with a letter get a lead, so they never
  become an OSC 9 sub-command (`9;4;…` is a progress bar); "a digit" first
  let a space, a sign or a full-width digit through (review stop 9, E3).

**Then the rest of the buffers** (`buffers.md` §10, each re-measured
first): V4's graph identity in `:ls!`/`:b N`, V4b (one window history),
V5 (listings as buffers). **Then tabs T2**, whose tabline wears the glyph.

## 8. Verification, when built

- A decision arriving while you type leaves focus and the prompt's text
  untouched (12a07f9's test, per pane).
- **Two sessions never cross:**
  - an answer key in A's pane resolves A's call while the aim is at B, so
    a mutant reading the aim goes red;
  - B's withdrawal leaves A's pane id, scroll and focus alone;
  - a `:source` that denies B's call leaves A's untouched.
- **Placement is sticky:** `<C-w>j` between A's two windows and into the
  pane leaves its id and rect unchanged, and a resize survives ten events.
- Two panes at two widths are each fitted at their own.
- With room for one pane, the other session is named on the row.
- `$EDITOR` opens only from a key in the pane, after arming.
- A queued prompt stays held across a review's arrival.
- `:qa` names the draft; `:only` doesn't reopen a left review.
- Alerts: a configured alert shows, is cut but not dropped at 40 columns,
  and a bad `when` is said at load. Borders are checked by rendered-cell
  tests. The `ESC ] 52` payload comes out clean. `unshown` clears on the
  first frame a pane draws the buffer, and not before.
- Smoke: a decision in a split, `:decide`, `:Send b2`, the
  distil-with-review path.

## 9. The design review, 10-22

Two Opus reviewers checked this against the code before anything was
built:
- `docs/Reviews/2026-10-22-design-decisions/DR-A-decisions-per-session.md`;
- `docs/Reviews/2026-10-22-design-decisions/DR-B-reviews-and-alerts.md`.

Every finding is folded in above, with its number. Their verdicts: the
model is right, the inventory was incomplete, and what it missed lands on
the consent surface. The worst of what it missed:
- one revisit for N calls;
- the no-room fallback holding one slot;
- a review's hold.
