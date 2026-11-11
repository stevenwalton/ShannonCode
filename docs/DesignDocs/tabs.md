# Tabs — proposal (10-18), Steven's answers in §7

**Decided (Steven, 10-18 late):** Q1 (a) — a decision for a session in
another tab opens in THAT tab; the tabline marks it and the status row says
which tab; "We should *not* jump the user to the tab because that'll give
them a lot of surprise ... just warn them that it is there and they need to
go address it." Q2 per tab — "Aiming at something invisible is advanced
behavior and should take a command" (`:Send` addresses buffers, tabs
included). Q3 the focused pane's session name, overridable — "Let's not
just allow the session renaming but also tab renaming. This will help
people organize." (`:tabrename`.) Q4 folds after tabs.

Steven, 10-18: "But tabs first" (of tabs, folds and marks for the soft
release), and "Tabs should be structurally cheap as long as we have the
buffers done correctly. So want to double check on that."

What a tab IS was decided in `Milestone 5.md` (§"The three viewport
kinds"): **a named layout of panes**, vim's meaning — not a file and not an
agent ("the vim 'tab' naming trap"). And step 4 there: "Tabs + the weechat
hotlist — multi-agent, activity-tiered, announce-don't-seize." This
proposal builds that; the questions at the end are Steven's.

## 1. Are the buffers done correctly? (verified 10-18)

**Sessions, yes.** A `BufferId` is an occurrence of a session
(`SessionBuffer`, app.rs ~933), minted app-wide and only ever increasing
(`next_buffer_id`, minted only in `open_buffer`), held in one
`App::buffers` that nothing ever shrinks. Closing a pane keeps its buffer
(`:hide` says so: "sessions live in `buffers` for the life of the run";
`:ls` flags it `h`). So a session shown in tab 1 and tab 2 is one buffer
in two panes — "one buffer, many viewports" holds across tabs for free.

**Viewers, no — and tabs do not need them to be.** A viewer's state
(`:graph`'s tree, `:help`'s page, the inspector, `:analyze`) lives on its
PANE (its slot's `Window`, `app/window.rs`, since V5.3 (4)–(5); then
`App::pane_views: HashMap<PaneId, PaneView>`), not on a buffer (TODO
"Every pane on a buffer?", 10-15). A pane belongs to one
tab, so its view simply waits while its tab is hidden. That refactor
matters for one viewer in two panes, or a `:hide`n viewer coming back —
not for tabs.

**The real catch: pane ids are per workspace.** `Workspace::new` hard-codes
`PaneId(0)` (transcript) and `PaneId(1)` (prompt) with `next_id: 2`
(workspace.rs:495-527), and mints at `open`, `open_across_top`,
`ensure_prompt`. A second `Workspace` mints the same ids, and every map
keyed by pane id — `pane_views` above all — would collide. Fix first: one
app-wide minter. **Done since** (T1): `PaneId::fresh` (`wm.rs:30`) is one
app-wide counter; and since V5.3 (4), 10-25, no map is keyed by pane id —
each window lives in its pane's slot.

## 2. The model

- `App` holds `tabs: Vec<Workspace>` and `tab: usize`. A `Workspace` is
  already exactly a tab page: its layout tree, its slots (what each pane
  shows, `marked`), its focus (with `prev` for `<C-w>p`), and where
  the user put the decision pane (`decision_place`). (A slot's `lent` went
  into each window's history, on the app's `Window` beside its cursor,
  in V4b, 10-25 — a tab's windows carry theirs as they carry their cursors.
  And the windows themselves are in the slots since V5.3 (4), 10-25:
  `Workspace<Window>`, so a tab carries its windows whole.)
- **The `workspace` accessor narrows in silence**: every walk written
  `self.workspace.windows()` would mean "this tab's" the day it names the
  current tab. So the walks that mean every window go through ONE door,
  `App::windows{,_mut}` (V5.3 design review, finding 3), which T2 changes
  to walk every tab; the two that mean the windows drawn (`sync_pages`,
  `settle_tree_marks`) walk the workspace and say why — row in §3.
- `self.workspace` (158 uses, `rg --count-matches`; every one a method
  call on the field — none is borrowed whole, assigned, or `mem::take`n)
  becomes `self.tab()` / `self.tab_mut()` — mechanical. The tests reach
  the field directly 525 times (`app.workspace.…`); a `workspace` accessor
  of the same name keeps them as written. (**Superseded by §6b**, 10-26:
  175 production sites and 758 test uses by then; the field stays, meaning
  the tab you are in, the others parked beside it.)
- Buffers, the prompt's text, the keymaps: app-wide, unchanged. (The one
  foreground turn went with §9 of `buffers.md`, sessions run apart. The
  decision queue went 10-22: each session's decisions are its own, in a
  decision pane under a window showing it — so a tab holds the decision
  panes of the sessions it shows, and Q1 below is answered by placement;
  `decision-panes-and-alerts.md` §3.)

## 3. What "every pane" means, site by site

Most sites mean "this tab's panes" and become so by the accessor. These
mean something else and must be decided one by one (the bug class:
`[[feedback_relocating_state_fans_out]]` — every reader still compiles and
keeps the old meaning). Proposed answers (10-18; **decided as built in
§6b** — this table is the proposal, kept: row 90 named `tree_for_graph`,
gone with one tree, and widened `session_panes`, whose three readers mean
this tab, as the site list found):

| Site | Today | Proposed |
|---|---|---|
| ~~`reconcile_pane_state` (`pane_views.retain` by `workspace.pane_ids()`)~~ | gone 10-25: a window lives in its slot and leaves with it (`buffers.md` V5.3 (4)) | — nothing to prune, in any tab |
| `reconcile_pane_state`: a decision whose pane is gone | a decision window the workspace gives back unclaimed (`take_closed`; V5.3 (4)) while its session is still shown; with its session off screen too, the call waits (since 10-22, A1.4) | drain EVERY tab's closed list — **as built 10-26: never a deny**; unclaimed means forced, the request waits hidden (`decision-panes-and-alerts.md` §3.6) |
| `App::windows{,_mut}` — `:unread`, `relist_claude_sections`, a reading landing, `forget_cursors_on`'s history, `refresh_graph_views`, `tree_for_graph`, `cancel_chords`, `end_visual_everywhere`, `companions_of`, the reconcile's orphan rule, `panes_holding`, `holdings`, `session_panes` (13 walkers, 14 sites, recounted at review stop 10's close: `session_panes` walked the ring by its own filter until D 6) | this workspace's windows (V5.3 (4)) | every tab's in the door — **and** at the four walkers that look each id up again through the current workspace (`end_visual_everywhere`, `refresh_graph_views`, `relist_claude_sections`' second walk, the orphan rule and its `workspace.close`): those lookups tab-wide, or the walker acts on the window the walk hands it (V5.3 verify, F4; this said "changed in the door alone") |
| `sync_pages`, `settle_tree_marks`, `settle_pages`, `renew_listings` (`self.workspace.windows*` / `ring_order`, said at each; the last two added at review stop 10) | the windows drawn | decide: is a background tab's window drawn? (a page is synced when it comes back; a tree set aside neither speaks nor listens) |
| `window_on` → `aim_window` / `existing_host` / `aim_viewport`, a landing's window (`place`) | a window showing the aim (`panes_on`), else one with it beneath a view (`panes_with_session`) — both every window since stop 10 (`pane_showing` went; A 3's "two scopes" was the ring and every window) | this tab's, both halves: the aim's window in THIS tab, else none — or `:graph` from the prompt opens in another tab's window (Q1 (a)) |
| `show_if_unseen`, `review_reach` / `review_when_written`, `unnoticed_if_reused`, `blank_buffer`'s "no window" arm (review stop 10, A 3) | this workspace | decide each: "in no window" means in no tab's (a decision shown, a review reachable, a session reused), so every tab's |
| `fit_decision_pane` / `settle_decisions` (where a decision opens) | under a window showing its session (since 10-22, per session; one in no window waits, `:decide`) | windows in every tab — the session's tab, Q1 (a) |
| `panes_with_session` (was `panes_showing`, renamed at review stop 10) / `shown_elsewhere` / `forget_cursors_on` | this workspace | every tab |
| `:ls` pane column, `a`/`h` flags | this workspace | every tab; `a` = shown in some tab, the column `t2:3` (built at T3) |
| ~~`pin_session_panes` (unbound panes follow the foreground)~~ | gone 10-25: every transcript pane is born on its session (`buffers.md` V5.3 (2)) | — nothing follows the foreground, in any tab |
| `reaim_if_target_is_gone`, `aim_pane`, `aim_needs_saying` | this workspace | this tab (Q2) |
| marks (`submit`'s override, `:Send` marks, `ring_order` numbers) | this workspace | this tab: marks are drawn, so the override is visible; hidden marks must not send |
| `refresh_graph_views` (iterates `pane_views`) | every view | every tab's — through `App::windows`, row above |

## 4. Keys and commands (vim's, with Shannon's `:new` meaning)

- `gt` / `gT` / `{n}gt` — next / previous / nth tab. `g` is a MOTION prefix
  today (bindings.rs:316; `gx` cancels), so `gt` needs the planned
  `Tab(TabCmd)` atom (keymap-atom-table.md:141, "parsed now, no-op until
  tabs" — not in the code) and the operator path must reject `dgt`.
- `<C-w>T` — move this pane to a new tab (unbound today).
- `:tabnew [<id>]` — a new tab with a fresh session, or session `<id>`
  (Shannon's `:new` is "a fresh session in this pane", so `:tabnew` is the
  same in a new tab). `:tab <cmd>` — run a pane-opening command in a new
  tab (`:tab graph`, `:tab help rules`), vim's modifier.
- `:tabclose[!]`, `:tabonly[!]`, `:tabnext` / `:tabprevious`, `:tabmove
  [N]`, `:tabs` (list: each tab, its panes, what each shows).
- `:q` / `<C-w>c` on a tab's last pane closes the tab (vim), never the app
  while another tab is open; `:qa` as now.

## 5. The tabline and the hotlist

- One row at the top when there is more than one tab (vim's
  `showtabline=1`), drawn with the existing `ui.*` roles. A tab's label:
  its number and its focused pane's session name (or viewer kind), Q3.
- Per tab, the weechat tier of its busiest session (`Milestone 5.md`):
  needs-you (a decision waiting) ≫ error/stopped ≫ new output ≫ idle —
  colour for the state plus a glyph for liveness, as the graph's row marks
  do. Announce, never seize: nothing moves you to another tab.
- Geometry: `body_of` (a free function of `workspace.rs` since V5.3 (4))
  is "the frame minus its last row", and
  `clamp` reserves that one status row SEPARATELY (workspace.rs:1491,
  1803) — both take the tabline row, or one of them lies. Mouse: a click on
  a label is `{n}gt` (`handle_mouse` → `pane_at` → `rects(body_of(…))`).

## 6. Build order (each a green commit, mutation-checked, smoked)

- T1 — app-wide pane ids; `pane_views` pruned against every workspace.
  No UI. Test: two workspaces never share an id; a hidden tab's graph view
  survives a reconcile. (The ids done; the pruning gone with `pane_views`
  in V5.3 (4) — a hidden tab's windows are in its own slots.)
- T2 — `tabs` + the accessor; `:tabnew`, `:tabclose`, `gt`/`gT`/`{n}gt`;
  the tabline. Smoke: open a tab, switch, close.
- T3 — the §3 sites; the hotlist tier on the tabline; Q1's decision rule;
  the status row naming a decision waiting in another tab.
- T4 — `<C-w>T`, `:tab <cmd>`, `:tabmove`, `:tabs`, `:tabonly`; mouse;
  `:help shannon-tabs` (vim style), `feature-checklist.md`.

Not in it: saving tabs across runs; vertical/tree tab strips (presentation,
later — `Milestone 5.md`); per-tab options. The "every pane on a buffer"
refactor (§1) comes BEFORE T2 (Steven, 10-20): `buffers.md` §10.

## 6b. T2 — the build plan (10-26), after the buffer work

Written before the code, from the code as it stands at `8d3bb89` (one
tree per store, a companion is its host's window, a session's entry).
Phases, each a green commit, mutation-checked; smoke at the end of T2.

**The model — my call, flagged.** `App::workspace` stays the field it is,
and means **the tab you are in**; the other tabs are parked beside it,
`App::tabs: Tabs` (`Vec<Option<Tab>>` in tab order, `None` where the
current one is, and its index). A tab is `Tab { workspace }` — it was
`Tab { workspace, aim }`, its prompt's aim (Q2, per tab) swapped beside the
workspace with `App::prompt_aim`, until 10-26 put each prompt's draft and
aim on its window (`PromptView`), parked with the workspace. Switching
swaps it in one door (`App::show_tab`). Why not `tabs: Vec<Workspace>` + `self.tab()`: the 166
production uses and 758 test uses of `workspace` mean "the arrangement on
screen" today and still do; an accessor rename is 924 edits that change no
meaning, while the sites whose meaning DOES change are the ones below, and
they are the ones the build must name.

**Every walk says how far it reaches.** The one hazard (§2: "the
`workspace` accessor narrows in silence", and the reverse, a walk widened
in silence): `panes_holding`, and the walks over it — `panes_on`,
`panes_with_session`, `session_panes`, `holdings` — take an
`Over { ThisTab, EveryTab }` (planned as `Reach { Here, Everywhere }`;
renamed at review stop 10, `review::Reach` having the name), so the
compiler lists every caller and each says which it means. Ring order is
the current tab's ring, then each parked tab's in tab order. Decided per
caller (`rg`, 10-26), **as built at T4** — and since the r12 reviews
(`docs/Reviews/2026-10-26-tabs/{A,F}.md`: eight of these flipped and
passed the suite), each row is held by a named test in `app/tests/tabs.rs`:

| Caller | Over | Why |
|---|---|---|
| `aim_pane` (so `aim_needs_saying`, `reaim_if_target_is_gone`, `fork_command`), `window_on` (so `aim_window`, `place`), `session_panes` in `aim_needs_saying`, `reaim_if_target_is_gone`, `:Send all` | ThisTab | the aim is drawn (Q2, `[[feedback_predictable_from_screen]]`); marks are this tab's |
| `settle_sight` | EveryTab (planned ThisTab) | its notice says "in no pane", false of a parked tab's window at every `gt` (the site list) |
| `reveal` (a landing's pane), `show_if_unseen` (`:tool`, and `:decide` once in the right tab) | ThisTab | vim's `:b`: in this window, though another tab shows it |
| `fit_decision_pane` | ThisTab first, else EveryTab (T3) | under a window here, else in the session's own tab (Q1 (a)); equivalent first-pane either way today, the ring walking this tab first |
| `settle_decisions` | EveryTab (T3) | each tab's pane kept while its host shows its session |
| `:decide` | the pane's tab, else the session's (r12-F H2) | you asked to go to it; with no pane it showed the session in THIS tab's window, putting that window's session in none |
| review sentences (`where_shown`, T3) | ThisTab, else which tab | "beside it" is never said of a pane you cannot see |
| `reconcile_pane_state` (a decision window given back unclaimed) | EveryTab | **answered 10-26**: no close answers — refused without `!`, forced it waits hidden, wherever its session is shown (r12-F M1 closed) |
| `held_in_tab` (`:tabclose`, `:tabonly` refusals; `requests_in_tab` until a166fd2) | the tab named | a request or a draft in another tab is that tab's, named with its `Ngt` |
| `close_pane_command`'s `twinned` (a request's chat closes freely beside a twin; stop 11, F 6) | ThisTab | the twin keeps the chat in view, and the pane goes under it — a window in another tab keeps neither; `a_twin_in_another_tab_does_not_free_the_chat` (review stop 12, T5 F3: `EveryTab` passed the suite) |
| `forget_cursors_on`, `shown_elsewhere` / `blank_buffer`, `unnoticed_if_reused` | EveryTab | "in no window" means in no tab's |
| `landed_notice` | ThisTab | a landing is placed in the event that asked, in this tab (`Lands::Into` is synchronous — the site list verified it), so another tab's arm could not be reached |
| `holdings` (`:ls`) | EveryTab | `a` = shown in some tab; the pane column `t2:3` (T3, `App::pane_labels`) |

**As built, beyond the plan** (the independent site list,
`docs/Reviews/2026-10-26-tabs/sites.md`): `decision_pane_of` finds a
pane in any tab (one decision, one pane — a second here would answer it
twice), and `decision_in_a_pane` asks this tab's (setup covers what is on
screen); `$decide` leaves one parked elsewhere to `$hidden`, which says
`DECIDE in tab N · Ngt`; `:decide` goes to its tab (you asked); a tab
coming back re-arms its decision panes (finding 5: parked, the delay ran
out unseen) and is clamped to the frame; and the aim parked with a tab is
pinned to where its prompt went (`App::pin_aim` — `None`, "follow the
foreground", came back as another tab's session, the foreground being the
app's). `gt` needed the motion path to emit a window command
(`MotionChord::Window`), and the key seam to apply one (`key_for_test`).

**Lookups by pane id are app-wide.** Ids are minted app-wide (T1), so
`App::window_of{,_mut}` and a new `App::kind_of` find a pane in any tab,
and `App::close_window` closes it in the tab that has it (the orphan rule,
`release_companion`, a decision's pane). The stored ids — a companion's
`host` (same tab as its companion), a decision's `decides` (the window it
sits under, same tab), `ConfirmAction::Delete.from`, an import's
`Lands::Into` (asynchronous: lands in its pane wherever it is, moving
nothing — `[[feedback_no_teleports]]`) — then resolve wherever they are.
`self.workspace.kind_of` (21 sites) becomes `App::kind_of` — one is left,
`gives_back`, on the focused pane, where it is correct (r12-A LOW-6).

**`App::windows` walks every tab** (11 `windows{,_mut}` sites and 3
`windows_in`, recounted by r12-A, 10-26; planned as 12 walkers — every tab, as its doc
has said since V5.3); `self.workspace.windows*` / `ring_order` stay for
the windows DRAWN — `sync_pages`, `settle_tree_marks`, `settle_pages`,
`settle_graph`, `renew_listings`: a parked tab's windows are not drawn and
are brought up to date when it comes back, as a view set aside is.

**A tab coming back** (`App::show_tab`, the one door): its workspace and
aim swapped in; each tree window in it takes the tree's marks (a parked
window, like one set aside, missed every mark since, and asked first it
would give the tree its stale ones — V4b verify H1); the next frame syncs
its pages and listings. Focus moving is the settle's (`settle_focus`).

**Commands and keys (T2):** `:tabnew [N|id]` (a new tab after this one,
on a fresh session or the one named — Shannon's `:new`), `:tabclose`
(refused on the last tab, vim's E784; refused over a review's draft, as
`:q` is), `:tabnext [N]` / `:tabprevious [N]` with `gt` / `gT` / `{n}gt`
/ `{n}gT` (`Tab` atom under the `g` prefix; `dgt` refused by the
operator path), and `:q` on a tab's last pane closes the tab while
another is open (vim). Measured in Neovim 0.12.6 (scratchpad
`nvim_tabs.lua`, 10-26): `:tabnew` opens AFTER the current tab;
`:tabclose` and `:q` on a tab's last window go to the tab on the RIGHT,
or the left one if it was the last; the only tab refuses (E784); `gt` and
`gT` wrap; `{n}gt` goes to tab n, and with no tab n does nothing, silently;
`{n}gT` goes back n, wrapping; `:tabnext N` goes to tab N (E475 with no
tab N); `:tabprevious N` goes back N, wrapping.

**The tabline (T2):** one row at the top while there is more than one tab
(vim's `showtabline=1`): each tab's number and its focused pane's name —
the session's handle, or the view's kind — the current one picked out.
**One geometry door**: the frame a tab tiles is `App::tab_frame(frame)`
(the frame less the tabline row), asked by the draw, the clamp, the mouse
and `pane_content_size` alike (§5: "both take the tabline row, or one of
them lies"). **As built, a better door** (10-26): the row is the
WORKSPACE's — each reserves it above its panes (`Workspace::reserve_top`,
set on every tab when the count crosses one, `App::sync_tabline`), and its
one frame-to-body mapping (`Workspace::body`) and the clamp's reserve read
it, so every caller kept passing the frame it had and none could forget
the row; `App::body_rect`, the third statement of "the frame minus the
status row", asks `Workspace::body` (an equivalent under mutation: focus
moves compare panes relatively). Roles `ui.tabline` and
`ui.tabline.selected` (vim's TabLine/TabLineSel), drawn by
`view::draw_tabline`, numbers kept and names cut when narrow.

**Left to T3/T4, said:** a decision placed in its session's tab and the
status row naming it (Q1 (a)); the hotlist tiers; `<C-w>T`, `:tab
<cmd>`, `:tabmove`, `:tabs`, `:tabonly`, `:tabrename` (Q3), the mouse on
the tabline; `:help shannon-tabs`.

**`:tabs` as built (10-26), the last of T4.** A ninth page kind
(`PageKind::Tabs`, after D2 made the kinds one variant), a live listing as
`:ls` is (`Listing::Tabs`, derived each frame a window shows it): vim's
layout — `Tab page N` and the name you gave it, then the tab's windows in
ring order, each numbered as its corner is in its tab, `>` the window you
are in (the tab you are in only, vim's), `+` a prompt holding a draft
(vim's "modified"), a decision pane by whose. Every line names its tab
(`Row::Tab`), so `<CR>` anywhere in a block goes to it; on the tab you are
in, said. `:split tabs` opens one too.

**T3 as built (10-26).** Q1 (a): `fit_decision_pane` opens a decision
under a window showing its session in THIS tab (the focused, else the
first), else in the session's own tab (the first in tab order) — in that
tab's workspace (`App::workspace_of_mut`), nothing moved here; it is armed
there at its opening and again when its tab is shown (`tab_shown`).
`settle_decisions` keeps every tab's decision panes, each while its host
shows its session (sticky, DR-A #5). Said: `$hidden`'s `DECIDE in tab N ·
Ngt` (T2), `:ls`'s `needs you — in tab N, Ngt`, its pane column `t2:3`
(`App::pane_labels`), and a review's sentences beside a session shown
only in another tab (`App::where_shown`). The hotlist on the tab line
(`Milestone 5.md`'s tiers, announce-don't-seize): another tab with a
decision waiting in `ui.caution` (the hue of an ask), one holding a
session an alert marks wearing the alerts' glyph — the user's to define,
as `:ls`'s mark column is (`[tui.alert.*] mark`; news by default). Not
built: error/stopped as a tier of its own — no alert says it today, and
one that marks would show here as news does.

**T4 as built (10-26), the commands.** Measured first in Neovim 0.12.6
(scratchpad `nvim_tabs{4,5}.lua`): `:tabmove` bare or `$` moves the tab to
the end; `0` to the front; `N` after tab N, so N itself or the tab before
it is no move, and N = the count is the end; `+N` / `-N` (bare `+` / `-`:
one) are relative, and past either end — `+1` from the last too — is E475,
never wrapped as `gt` wraps; `:tabonly` keeps the current tab, `:tabonly N`
tab N, and with one tab says "Already only one tab page". Built as
measured: `tabs::move_to` (the argument, a pure function, its table the
measurement), `Tabs::move_current`, `Tabs::close_others` (the windows to
the reconcile, as `close_current`'s). `:tabonly` is refused over another
tab's review, naming the tab, and `:tabclose` over this one's — one rule,
`App::tabs_with_review`. **A tab's name is its place's** (Q3,
`:tabrename`): `Tabs.pages` is a `Vec<Page { parked, name }>`, so the name
stays with its tab while another is shown and moves with it, and
`Tabs::switch` swaps only what is shown. `:tabrename`'s grammar is
`:rename`'s (bare says it, `!` takes it off); the name is checked by
`Label::plain` at a session name's length, and may hold spaces: a tab's
name is read, never typed back as a token (my call; if `:tabnext <name>` is
ever wanted, it tightens to one word).

**One tab argument** (measured, `nvim_tabs{6,7}.lua`): `:tabnext`,
`:tabonly` and `:tabclose` take N (from 1), `+N` / `-N` (relative; bare
`+` / `-` one; never wrapped) and `$` (the last); `0` and past either end
are E475; `:tabprevious` takes a count only (its `+1` and `$` are E475);
`:tabclose N` closes tab N and leaves you where you are. Built as one
reader, `tabs::tab_named`, which `:tabmove`'s relative and `$` forms and
`{N}gt`'s refusal also ask (`digits` refuses a sign, which `usize`'s parse
takes). **Two T2 bugs it closed**: `:tabnext +2` read as tab 2 (the parse
took the sign), and `:tabclose 3` closed the tab you were in (its argument
was taken and dropped). `Tabs::close(at)` closes a parked tab, the shown
one staying shown.

**A click on the tab line** (`tui.mouse`) goes to the tab under it:
`App::click_tab_line`, split from `handle_mouse` (which takes a live
terminal) as `release_left` is, and hit-tested by `view::tab_at` over
`view::tabline_texts` — the one layout the line is drawn with, so a label
and the click that finds it cannot part. Counted in cells: the alerts'
default mark (🔔) is four bytes and two cells.

**One door for a window born; `:tab {cmd}` and `<C-w>T` on it.** A window
was born at four sites, each its own workspace call: the split door
(`:split`, `:sg`), `host_pane`'s and `session_host`'s third step (across
the top), and `:fork`'s split. They are `App::new_window(window, Born)`
now, and `:tab {cmd}` is a flag that door takes once (vim's
`cmdmod.tab`): the first window `cmd` would open is born in a new tab
after this one, through `App::open_tab_holding`, which `:tabnew` and
`<C-w>T` make their tabs by too (`Workspace::new` seeds any window over
the prompt). Under the flag `host_pane` and `session_host` take no pane,
so what `:help`, `:graph` or `:b N` would show where you stand opens in
the tab instead — measured shape, `:tab help` from tab 1 of 3 is tab 2 of
4 holding the help (and its prompt). **One divergence, my call**: vim's
`:tab b 2` opens no tab (its `:b` opens no window); here `:b` and `:help`
come by one door and `:tab` sends both, since `:tab help` is what `:tab`
is for. A command that opens no window opens no tab; the flag is cleared
around the command, so nothing is left armed. `:tab fork` says "in a new
tab", not "beside it".

`<C-w>T` (measured: the window leaves its tab and is the new one's;
with one window vim says "Already only one window" — said here too,
pointing at `:tab sp`, the copy; r13-F L1 measured it) moves
the window itself, claimed and not closed, so no decision on it is
answered. Standing in a companion moves its host (F1); the host's
companions are made again beside it there (`pair_companion`). A decision
on its session follows by the rule already written: `settle_decisions`
keeps a pane only under its host, which has left, and `fit_decision_pane`
opens it again under the window in the new tab — armed, new under your
cursor. The moved window gets a fresh pane id, which is what makes that
so: keeping the id would have stranded the pane in the old tab, its host
"still there". The tab left, if its prompt went to the window moved, is
re-aimed before it is parked (`App::aim_fallback`, the re-aim's answer)
and that is said of it — "tab 1's prompt now goes to …" — since the
re-aim's own sentence would read as the new tab's. Refused, said: the
prompt, a decision pane, a tab's only window.

**Steven's tab test pass (10-26, smoke 101/101).** What he hit, measured
in Neovim first (`nvim_tabs8.lua`), then built:
- **vim's ranges before the tab commands** — `:+tabmove`, `:-tabmove`,
  `:$tabnext`, `:.tabclose`, `:+tabonly`, `:-tabnew` (just before this
  one), `:$tabnew`: the command line's leading count reads `+N` `-N` `$`
  `.` now (`command::parts`), handed as the argument to the one tab reader
  (`tab_named`, which gained `.`); `:tabprevious` and `:buffer` take a
  number only (vim's E16). `:tabnew`'s range is read before a session is
  made.
- **A `:tabmove` that moves nothing is said** — vim counts the tabs before
  the move, so from tab 2 `:tabm1` and `:tabm2` are no move, silently;
  "`:tabm1` doesn't appear to work".
- **`:ball` and `:tab ball`** — a window for each session `:ls` lists that
  no window here shows, split below in `:ls` order, at most N (the one you
  are in first, vim's measured count); under `:tab`, a tab for each
  session no tab shows, after this one, ending in the last. **Nothing is
  closed** (my call; vim's `:ball` closes what it does not need, and a
  window here carries its session's decision). With other tabs open,
  vim's order is its own ([b][a][x][c] measured); ours is "after this
  one, in order".
- **`:tab` with no such command says so as `:tab`'s** — "not a command:
  `:ball`" named no `:tab`.
- **The theme preview draws the tab line** (yours, one with news, one
  deciding) — the roles `ui.tabline` and `ui.tabline.selected`, which a
  theme file sets like any other.

**The r13 reviews of T3/T4** (`docs/Reviews/2026-10-26-t3t4/{A,F}.md`, at
`f2d5d42`), triaged at once — both found the one crash:
- **`:tab sp` from the prompt panicked** (F C1, A 1): the split door built
  a second prompt window and the new tab's constructor was handed it.
  Under `:tab` the window split is now the one the command is about
  (`existing_host`: the prompt's aim, a companion's host — which also
  fixes F M1 / A 3, a tab of an inspector with no tree); a prompt or a
  companion with nothing behind it is refused, said; and `new_window`
  refuses one itself, the floor that makes `open_tab_holding`'s `expect`
  hold by construction.
- `:tab new` makes a fresh session (A 2: it reused the blank one here and
  said "already on a fresh session"); `:tab b N` of a page opens a tab as
  of a session (A 6); what `:tab {cmd}` leaves to the terminal (`:squash`,
  `:child`) runs under the flag (`CmdEffect::InTab`, A 7).
- `<C-w>T`: `{N}<C-w>T` before tab N (F L2), its `:Send` mark kept (A 8),
  its companions released after the move cannot be refused (A 3), the
  sentence naming the tab left as it is numbered after, the hint `:tab sp`
  and vim's "Already only one window" (F L1, measured).
- `:{N}tabnew` (F L4); `:tabmove` with one tab said (L3); another tab's
  close said (L9); a cut tab's decision colours the `<` / `>` that hides it
  (F M2; a cut tab's alert glyph is not drawn — said in the manual).
- **Found writing the tests**: the re-aim fell back to the foreground,
  which is app-wide — `:tabnew` moves it — so a tab's prompt could be
  re-aimed at a session only another tab showed, announced as a fix.
  `App::aim_fallback` takes the foreground only if this tab holds it.
- Tests for every guard the two found unheld: `close_tab`'s `tab_shown`
  (F H1), `<C-w>T`'s only-window count beside a decision and beside a
  companion (F H2), `9gT`'s inner `% count` and `:tabprevious 0` (F H3),
  `tab_shown`'s `cancel_chords` (A 4, F L8) and its decision-pane
  exception (A 5 — today's rule, which changes with Steven's answer), the
  sentences that name a tab (F L6), the tab you are in drawn as yours
  though it decides (F L7); A's parser sweep and 600-step random walk
  kept as tests.

**The r12 reviews of T2** (`docs/Reviews/2026-10-26-tabs/{A,F}.md`, at
`2565cb5`), triaged at T4. Already closed by then: F H2's placement (T3),
F L7's `$` / `+N` (one tab argument), F L10 (the click), A LOW-3 (T3's
`workspace_of_mut`), A M-3's flags (T3's `pane_labels`, now held by a
test). Fixed:
- **The aim on a tab's return** (A M-1): a switch read as a focus move,
  and `focus_moved` aimed the prompt at the focused transcript over the aim
  the tab came back with. `tab_shown` ends what a focus move ends and
  records the focus as seen — except a decision pane, which `settle_focus`
  still leaves for the prompt (Steven's call, below).
- **`pin_aim` held** at both doors (A H-1, F H1), from launch at the prompt.
- **`:decide` goes to the session's tab** for a call with no pane (F H2's
  remaining half): it showed the session in this tab's window.
- **The prompt's height** reaches every tab (A M-2, `App::workspaces_mut`,
  which `sync_tabline` asks too).
- **The last transcript closed** says where its session still is, and
  offers `:q` only when `:q` would do it (F M5).
- One tab: `gt` says so (F L1); `:tabclose`'s advice `:qa` (F L2); a
  narrow line keeps the tab you are in, `<` / `>` where cut, and names
  whole or not at all below six cells (F L3); a companion's tab named as
  its host (F L6); `:[count]tabnext` and its kin (F L7; `App::counted`,
  which `:buffer` asks too).
- Tests for every reach the reviews flipped GREEN (A H-2, F M4), the tab
  line's names and order (F M3), `gt` from the prompt's Normal and every
  Visual (A M-4, F L11), a call withdrawn while its pane is parked (A
  LOW-1); the Q2 test that held nothing, rewritten (A H-1).
Left, said: A LOW-4 (a panel or float draws over the tab line — an overlay
over both, as `:themes` is, Steven 10-26), A LOW-5 (harmless), F L9 / A
LOW-2 (`open_tab`'s `put_place` equivalent while a window is born at the
end, as the launch's). Docketed: F L4 (in a read-only pane `d` is refused
at once, so `dgt` then switches tab; vim swallows the whole `d{target}` —
`dw` there moves by a word here, a gap wider than tabs). **Asked of
Steven**: F M1 (a close's deny reaching across tabs), F M2 (the draft
follows you across tabs, the aim does not), F L5 / A M-1 (coming back to a
tab you left standing in its decision pane).

## 7. Steven's decisions

- **Q1 — a decision for a session in another tab.** (a) `Milestone 5.md`'s
  rule, "modal = focused agent only": it opens in the asking session's
  tab; here the tabline marks that tab needs-you and the status row says
  `DECIDE in tab N — {n}gt`, and every other turn waits (one foreground
  turn) until you go there. (b) Today's behaviour: it opens in the tab you
  are in, beside your first session, naming the session. My lean: (a) — it
  is the rule already written for this, and (b) is "approving agent N's
  command thinking it's agent M's". Ask: should a jump key go straight to
  the tab that needs you (weechat's `Alt-A`)?
- **Q2 — the prompt's aim, per tab or app-wide?** The aim must be on screen
  (`[[feedback_predictable_from_screen]]`). Per tab: each tab remembers what
  its prompt sends to, and switching back restores it. App-wide: switching
  tabs re-aims at this tab's focused session. Lean: per tab.
- **Q3 — a tab's name.** vim: the active window's buffer name. Or a name
  you give (`:tabrename`, or `:tabnew` taking one) — `Milestone 5.md` says
  "a *named* layout". Lean: the focused pane's, overridable by a name.
- **Q4 — folds and marks after tabs** (Steven, 10-18: folds the minimum for
  the soft release, marks during the week). Folds wait for Steven's design
  (TODO §"Folding — in the graph AND in the conversation": "a placeholder
  for his design, not mine"); `:graph`'s Claude Code section already folds
  one level at a time (362f40d).

**Steven, 10-26** — the five questions after T4 (verbatim where quoted):

- **D2, `PaneKind::Page(PageKind)`**: "Approved" — before `:tabs`.
- **`:q` after a session opened from a companion**: "Option (a)" — keep
  V4b Q3 (a transcript's `:q` closes); CTRL-^ is the way back.
- **A close that would decide a request**: "we should prevent the close
  except in a `:qa!` like setting … In a `:qa` case we should refuse to
  close that conversation without the bang … when a force is applied we
  close the program, not answer a request. It is important that we don't
  answer the request, the user must do that. So I think we don't close and
  say something like 'Pending request in b#'." Read as (confirming with
  him): no close ever answers a request — only the user, in its pane; a
  close that would take a waiting request's pane is refused, naming it;
  `:qa` refuses while one waits, `:qa!` quits without answering it.
- **The draft across tabs**: "(a) A draft per tab … rather than vim's
  'editing a file' we're 'having a conversation'. So yeah, I think the
  prompt pane should be unique to the tab." And a bug, two prompt panes
  sharing one aim: "Both change when I change my aim where only the last
  should" — fix now. Docketed, low priority: a setting under which
  splitting a chat or a prompt pane makes both, a chat over its own prompt.
- **Back into a decision pane on return**: not understood — asked again,
  explained. Answered: "back to the last pane we were in. i.e. the vim
  way. This fits my mental model of a tab being 'a working space'. So
  let's not change the state when moving away (unless there's an explicit
  action like `:Send` involved)."
- **Confirmed (10-26)**, the close rule: a close never answers a request —
  only you, in its pane (`q` / `:q` there denies); any other close that
  would take a waiting request's pane is refused, "and also the request
  is tied to a chat, so let's make sure to not close the associated chat
  too"; `:qa` refuses while one waits, `:qa!` quits without answering;
  `:tabclose!` and `:only!` force the close — "It is an explicit command.
  So not a deny, a close" — the request left waiting. **Built 10-26**
  (`decision-panes-and-alerts.md` §3.6): `:tabonly!` too; on the chat a
  request's pane sits under, `:q`, `:close`, `<C-w>c`, `<C-w>q` refuse,
  and `:hide` or `:q!` close it — my call, as vim's `:hide` keeps what it
  closes. Forced, the request waits HIDDEN (`:decide N`), so `:only!` does
  not have it open again over the room it made. Not refused: `:b`/`:open`
  in that chat — its pane goes with its session, waiting, and comes back
  with it (CTRL-^); vim's own `:b` from a modified buffer refuses, so say
  if you want that here.
- **Confirmed (10-26)**, the prompt model: each prompt pane its own draft
  and aim; focusing a chat re-aims the prompt accessed last; a tab's
  prompt its own. And "a way to manually change the prompt's aim as a
  fallback since we've run into this bug a few times." **Built 10-26**:
  `View::Prompt(PromptView { draft, aim })`, one per prompt window, so a
  tab's drafts and aims park with its workspace (`Tab.aim` and `pin_aim`
  went: every prompt is born aimed — where the one split from goes, a new
  tab's at the tab you left's). The one you write in is
  `Workspace::prompt_pane` — the focused prompt, else the one noted last
  at `focus_moved`, else the first; standing in a transcript re-aims that
  one alone. The unnamed register is shared by every draft
  (`prompt::Registers`), vim's. `:only` keeps the prompt you write in
  (`Workspace::only`), which it closed and the reconcile put back, harmless
  while the draft was the app's. Each prompt pane draws and titles its own.
  **And a draft holds its window**, as a request does (`app/held.rs`): a
  prompt with a draft is not closed by `:q`, `:close`, `<C-w>c`, `<C-w>q`
  or `:hide` (a draft has nowhere to wait), nor `:only`, `:tabclose`,
  `:tabonly` over one, nor a tab by its last pane; `!` closes, the draft
  dropped, said — vim's E37 for a modified buffer. **And `:aim N`**, the
  fallback: the prompt you write in aimed at session N, shown or not (the
  command Q2 said aiming somewhere invisible should take); the reconcile's
  re-aim fires only when a window here whose session was the aim closed,
  so the next close of anything does not take an `:aim` back. **Review
  stop 11 (10-26)**: that close re-aims EVERY prompt here whose aim's
  window went (`reaim_prompts_whose_pane_went`), each said — a pane
  closing is not "the prompt accessed last"; while a tab has two prompts,
  the one you write in wears `%` in its title and in `:tabs`, and
  `:Send`/`:aim` name its pane; and a draft holds the APP open as running
  work does (`quitting_ends`): `:q` on the last pane, `<C-w>q`, `:wq` and
  `:qa` refuse, `:qa!` quits over it — my call, vim's `:qa` over a
  modified buffer.

**Steven, 10-20** — on §1 and the model:

- "When I said 'Are buffers done correctly' I mean, are we doing the
  'everything is a buffer' yet? Because that should make tabs fairly
  easy." Answered: not yet — sessions are buffers, viewers are panes (§1).
  **Decided: the refactor comes first** ("I agree with doing the
  refactor"), viewers included at my judgement ("I'm okay if `:graph`,
  `:help`, etc get modified… there's probably elegance in that work").
  vim's `:ls` lists neither help nor the file explorer, so a viewer can be
  an unlisted buffer.
- **A tab is a stored window** (pane layout), confirmed with his vim tabs
  (images 18–20): "They're kinda like your working environment. I
  frequently use them like I would different monitors or different
  workspaces. But people use them differently, maybe one file (or chat)
  per tab. We don't care how users organize, just that we are giving them
  the tools to organize" — as marks (extended from vim) and folding are.
  Tabs are not listed by `:ls` (vim's `:tabs` lists them).
- `:ls` as today for now; later toward vim's (image 21: number, flags,
  name, line).
- No weechat bindings ("I've never used weechat… the vim bindings are
  fine"); user-written keybindings, later; a jump to the asks waiting,
  soon (deferred, docketed).
