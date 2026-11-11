# Shannon Code — Design Doc

*First iteration. Sibling to `vision.md` — vision says "where and why," this
says "what and how." Both will evolve.*

Markers used in this doc:

- **DECIDED** — committed; argue if you disagree but the burden is on the
  challenger
- **OPEN** — real question, no commit yet; pick a direction in discussion
- **DEFERRED** — explicitly punted to a later section/iteration

---

## §0 Audience, Scope, Non-Goals

### 0.1 Audience

Two overlapping primary audiences. Calling them out explicitly because they pull
on different defaults and the rest of the doc has to reconcile that tension.

**A. Terminal-native daily-driver engineers.** People who live in the terminal,
run their editor (`vim`/`emacs`/$EDITOR) inside it, and want a coding agent that
integrates with that workflow rather than replacing it. They want sane defaults,
ergonomics that respect keyboard-first workflows, and the ability to use this
all day without friction.

**B. Researchers and powerusers building on top.** People doing model-comparison
work, agent-orchestration research, custom workflows, or building
company-internal orchestrators on this as a platform. They want first-class
access to the conversation graph, programmatic control over orchestration, and
primitives they can compose.

**Explicit non-audiences:**

- GUI-first users. (No web UI in v1; no Electron-style desktop wrapper.)
- IDE-integration-first users. (Use your editor inside the terminal; we don't
  ship a VS Code plugin.)
- Beginners learning to code. (Sane defaults, but power-user ergonomics win
  where they conflict.)
- Mobile users.

**Why two audiences and not one:** the orchestration features that researchers
care about (conversation forking, model variance, plugin extensibility) directly
improve daily-driver ergonomics (resume any past turn, swap models per task,
customize hooks). The audiences overlap in tooling, even if their headline use
cases differ. Trying to serve only A would leave the orchestration story
underbuilt; trying to serve only B would yield a research tool no one uses
daily.

**Importantly**, these types of users are a secondary concern. We are building
the baseline where they *could* build GUIs and IDE integrations *on top of* our
system, but we are putting **professionals first**.

### 0.2 Scope (what we're building, v1)

1. **A terminal agent harness.** Default TUI is simple (panes that scroll
   independently, `$EDITOR` integration, `tmux`-friendly[^0]). Fancy mode
   (sixel/kitty graphics, animations) is optional and behind a config flag.
2. **A conversation/history graph as a first-class object.** Episodes, turns,
   branches, fork/clone/squash-merge operations. Specified in §1.
3. **Multi-provider model access.** Anthropic-first, but the orchestrator does
   not assume a single provider. Provider abstraction is part of the
   architecture, not an afterthought. [DEFERRED — own section]
4. **Plugin / extension system.** Users can add commands, agents, hooks, custom
   merge strategies, custom compaction strategies. [DEFERRED — own section]
5. **Live-reload configuration.** Like `vim`'s `:so ~/.vimrc`. No
   restart-the-CLI-to-pick-up-a-config-change. Applies to permissions, MCPs,
   skills, tools, plugins, etc. [DEFERRED — own section]
6. **Editable permission suggestions.** When the agent asks "approve `gh pr
   *`?", the user can edit the suggestion inline ("no, `gh pr view *`") before
   accepting. [DEFERRED — own section]
7. **Pluggable tool restriction via `$PATH`-style mechanism.** Unix-philosophy:
   if a program isn't in the agent's path, it can't run it. Not a security
   boundary; an ergonomic. [DEFERRED — own section]
8. **State propagation rules for episodes.** What travels with a fork
   (CLAUDE.md, skills, MCPs, worktree state). See §1.4.

[^0]: We are as friendly to `tmux` as possible but `tmux` has limitations and we
    will not fight them. Our primary target is a modern terminal, not `tmux`.

### 0.3 Non-goals (v1)

- **No sandbox security boundary.** Tool restriction is ergonomic, not
  adversarial. Real isolation (separate uid, namespaces) is a future direction
  acknowledged in the vision doc footnote, not v1 scope.
- **No backwards compatibility with Claude Code's session files.** Could be
  added later as an import path. Not a blocker.
- **No reuse of Claude Code's OAuth `client_id`.** [DECIDED] That path is an
  impersonation gray area. If we ever do OAuth, it will be against our own
  registered client. M1 ships **API-key-only**; subscription/OAuth auth is
  **not foreclosed** for v1 but is [DEFERRED] until we understand the auth
  surface (token lifecycle, refresh, multi-credential UX) well enough to
  implement it cleanly.
- **No Anthropic-only assumptions in the architecture.** We're building for the
  API surface in the beginning, not for Anthropic's specific product.
  (Practically: Anthropic-first, but the abstraction supports OpenAI, Gemini,
  local models via OpenRouter or similar.)
- **No "auto-mode" classifier baked in.** Claude Code's auto mode classifier
  is an interesting feature but not a v1 requirement; we will add task specific
  models later and leverage the flexible expansion system to enable these
  advancements.
- **No remote control / mobile push notifications in v1.** Claude Code has
  them (Remote Control); we don't replicate them.
- **No process-isolation between sub-agents in v1.** Sub-agents run in the same
  process with logical isolation (separate message histories, separate context).
  Process isolation can come later if needed.

### 0.4 Hard constraints (foundational decisions)

These shape everything downstream and aren't up for casual revision.

- **Language: Rust.** [DECIDED] Drives off the supply-chain concerns in
  vision.md (no JavaScript) plus three positive criteria: (a) the TUI ecosystem
  we want lives here (ratatui + crossterm), (b) single-binary distribution
  without a runtime, (c) the type system and `no-unwrap-by-default` posture
  match the "elegant, adaptable" goal in AGENTS.md. Cost paid: slower
  iteration than Python; we accept that. Python and Go were the runner-up
  candidates; both were stronger on iteration speed but weaker on the
  distribution and TUI-ecosystem axes.
- **Config location: `$XDG_CONFIG_HOME` (defaulting to
  `$HOME/.config/shannon/`).** Not dotfiles. Cleaner, follows the modern
  convention, plays nicely with config-management tools. State and cache go to
  `$XDG_STATE_HOME` and `$XDG_CACHE_HOME` respectively. Config can be aware of
  other LLM locations, such as `~/.claude` and use these settings if the user
  wants. [DECIDED]
- **Auth: provider-pluggable; API-key default; M1 is API-key-only.**
  [DECIDED for M1, OPEN for v1.] M1 requires nothing more than a user-provided
  API key per provider, stored in `$XDG_CONFIG_HOME/shannon/credentials`
  chmod 600 (or platform keyring if available). The auth layer must be written
  behind a small enough interface that adding subscription/OAuth later does
  not require refactoring callers — we want this path *open*, not *built*.
  No reuse of Claude Code's `client_id` regardless of when we revisit (see
  §0.3).
- **Persistence: human-readable + grep-able.** Episodes persisted in a format
  you can `cat` and `grep`. JSONL is the default candidate; SQLite is on the
  table if querying becomes load-bearing. [OPEN — but bias toward JSONL until
  proven inadequate]
- **Single-binary distribution where possible.** Users should not need to
  install a runtime to use this. (If we pick Python, this means `pyinstaller` or
  `uv tool install`; if Rust/Go, native.)
- **Tooling for the project itself:** the design doc, vision doc, and supporting
  artifacts live in the project repo and evolve with it.

### 0.5 What this doc explicitly defers

These are real and need their own sections; not in this iteration:

- §2: TUI architecture (panes, scrolling, `$EDITOR` integration, simple vs.
  fancy mode)
- §3: Implementation language + concrete tech stack
- §4: Provider abstraction (multi-model)
- §5: Plugin / extension system
- §6: Permissions and live-reload config (including editable-suggestion UX)
- §7: Tool surface (file ops, bash, web, MCP)
- §8: Hook system (what we preserve from CC's design, what changes)
- §9: Memory / skills / persistent knowledge
- §10: Testing, observability, telemetry

---

## §1 The Conversation/History Model

The architectural center of the system. Specified first because everything else
(persistence, plugins, sub-agents, multi-model orchestration) rests on it.

The vision doc establishes the git mental model. This section makes it concrete:
types, operations, invariants, and the places the analogy breaks.

**Design stance — we are building an environment, not a rigid product.** This is
where researchers and power users will poke, fork, and extend, and we cannot
predict what they will want to do with it. So the driver for this whole section is
flexibility/extensibility: wherever a policy could differ — how branches merge, how
a range is summarized, how tool calls replay, where data is stored — it is a
**pluggable strategy with a simple default and an open interface** (a small trait),
not a hardcoded law. We build the simple version first, but as one instance of a
general mechanism, so a fork or plugin can do something that doesn't even make sense
to us. This is the same discipline as the elegance bar: a small, right abstraction
is both the flexible one and the efficient one ("a good programmer is lazy"). Our
own objectives will move as we experiment — rigidity is the expensive mistake.

### 1.1 Core types

```
Message     := { role, content, metadata }
Turn        := one user-side input + one assistant-side output (with tool-call cycles inside)
Episode     := an ordered sequence of Turns + the config that produced them
EpisodeRef  := stable identifier for an episode (analogous to a git ref)
Graph       := the set of all Episodes and the parent-pointers between them
```

**Turn — atomic unit.** A Turn is one round-trip: the user provides input (a
prompt, or a synthetic input from a sub-agent), the assistant produces output
(possibly with intermediate tool calls and results). Multiple internal tool-use
cycles within a single user prompt collapse into one Turn. This matches what a
researcher would treat as one observation.

- A Turn is **content-addressable** — its hash is derived from its messages +
  the config used (model, temperature, system prompt, tool definitions). Two
  Turns with identical inputs and config that happen to produce the same output
  have the same hash. This is what enables deduplication and lets us reason
  about "the same conversation" precisely.
- A Turn carries `provenance`: which model produced it, what temperature/params
  were active, what tools were available, what system prompt was in effect, what
  time it ran.
- **Content and occurrence are two things [DECIDED 2026-10-02].** Identity is
  content, so two productions of the same turn share one id — and NOT their
  facts: when each ran, what each cost, why each stopped, what a result turn
  distilled. The graph therefore keeps a turn's content once, by id (messages
  + model, exactly what the id hashes), and its provenance on the
  **occurrence**: the `Own` entry of the episode that produced it. A slice
  shares the occurrence, so borrowed history keeps the facts of where it was
  made. Keyed by id, a second production overwrote the first's facts; the type
  split makes that unrepresentable (`Graph::admit`, `Graph::placed`).
- **Provenance is what the request CARRIED [DECIDED 2026-10-04].** A turn's
  `TurnConfig` — model, reply ceiling, effort — is fixed when it begins, and
  every request it makes is built FROM it (`SendRequest::under`): its first,
  each tool round, a prefill continuation of it (re-filtered for what answers
  now). Settings that change mid-turn take effect on the next turn. The
  episode's own config is the session's preference — what `:model` and
  `:effort` set, kept in its header; `[turn] effort` is the default for a
  session that never chose — and is never sent as-is. **The two never stand
  in for each other:** a session composed from another (`S`, a prefill
  `:continue`, a squash) takes its source's preference as it is when it
  lands, never the config a request was sent under; and a turn no request
  produced (typed, returned, imported) seals as authored
  (`TurnConfig::authored`: its model, no ceiling, no effort), never under a
  preference.

**Episode — a conversation thread.** An Episode is an ordered sequence of Turns
plus the **active config** at the head (which can change turn-by-turn — e.g.,
the user swaps from Sonnet to Opus mid-conversation).

- An Episode has a single `parent` pointer to either another Episode (branch
  point) or `null` (root). The branch point includes which Turn it forked from.
- An Episode has a `head` — the most recent Turn.
- An Episode has a `state-snapshot` reference (see §1.4).

**Episode representation — a path with structural sharing. [DECIDED 2026-07-02;
IMPLEMENTED 2026-07-02, `6553a6a`]** "Ordered sequence of Turns" is the *logical*
view. **Physically an Episode stores only its fork point + the Turns it added past
that fork** ("divergent turns" / `own_turns`); the full sequence is *derived* by
walking the `parent` chain (`Graph::full_turns`). A session is therefore a **path**
through the shared Turn set, not a materialized copy — so branching/cloning a long
history is **O(1) time, O(0) space** (a new episode pointing at a fork; the prefix
is never copied). `root`, `head`, and "which route it took at each junction" all
fall out of the parent chain. The one thing each parent link stores beyond the fork
turn is a **`fork_index`** — the shared-prefix *length* — because the cut point
can't be inferred from the fork turn alone (a *checkout* branch keeps its fork turn,
an *edit* branch replaces it) and *searching* for a content-addressed turn's
position is an occurrence-vs-content error we refuse to make. A non-forking session
is just `(parent=None, own_turns=[all its turns])`.

- This is **structural sharing** (git's commit graph; a persistent/immutable
  cons-list sharing its tail). "Where two sessions share history / where they
  diverge" is a **lowest-common-ancestor (LCA)** query on the episode tree —
  cheap (a walk toward the root; binary-lifting → `O(log n)` only if ever
  profiled). It is **not** subgraph isomorphism (a different, NP-hard problem).
- **Content-addressing is what makes this work.** A Turn's hash is its content
  *only* (above), so a Turn is **position-independent** — the same Turn keeps the
  same id under any prefix. Editing a past Turn just moves the divergence point
  earlier (new content → new id → the suffix from there becomes the episode's own;
  the unedited prefix stays shared). So "clone" and "fork" are the *same* sharing
  mechanism differing only in intent, and "editing history" materializes only the
  changed suffix. We deliberately do **not** put the parent in the Turn hash (the
  "pure-git" variant): it would rewrite every downstream id on any edit/re-root
  and kill content-dedup, for no real gain.
- **Symmetric structural sharing (piece table).** Both *prefixes* and *suffixes*
  are shared structurally, by reference — no id-list is copied in any direction. An
  episode is a `Vec<Segment>` (a piece table over the shared Turn set): a branch is
  `[Slice(parent, 0..k), Own(..)]`, a re-root is `[Own(new_root), Slice(source,
  k..n)]`, a squash is `[Slice, Own, Slice]`. Only `Own` turns are new data, so every
  op costs exactly its changed turns. (This supersedes the earlier *prefix-biased
  asymmetry*, where a re-root copied its suffix as an id-list because an episode had a
  single parent pointer; the piece table's per-segment references removed that
  trade-off — `Graph::project` even resolves nested slices down to the true data
  owners, so the sharing stays flat.)

**EpisodeRef.** Episodes are referred to by stable IDs. Human-friendly names
(`main`, `experiment-haiku-routing`) are mutable pointers to Episodes (like git
branch names).

**Graph.** The full set of Episodes plus parent-pointers forms a DAG. This is
what we persist and what we navigate. The graph is queryable: "show me all
episodes that branched from this Turn," "show me all episodes that used Opus on
Turn 5."

**Fractal structure.** The model is recursive: `prompt | response ⊆ Turn ⊆ Episode
⊆ Graph`, and a tool-call inside a Turn can spawn a sub-agent, which is itself an
Episode (§1.x) — so it nests with potentially infinite depth and width. An **agent
is, mostly, its context/history**, which is exactly what an Episode is. We want
*one* uniform node abstraction so traversal, queries, and the viewer handle every
level the same way rather than special-casing each.

**Tool calls inside a Turn.** A Turn's internal tool-use cycles are part of its
record — we store each **call and its result**, so they are inspectable and so
re-deriving a Turn need not re-run side effects. On branch/clone, replay is
**replay-from-record by default** (use the stored result — deterministic, durable,
offline-safe); **re-execution** is the opt-in strategy for when you actually want
the tool to run again. (Side-effecting tools make these genuinely different — hence
a strategy, not a law.) A Turn may later be broken into multiple durable sections
(à la Temporal's workflow steps) for resumable long/tool-heavy operations; we keep
the record shape open to it without building it yet.

### 1.2 Operations

All operations on the graph are explicit and named. No magic.

**Two graphs, not one. [DECIDED, 2026-07-11 — the P6 deep review]** The structure holds
two *distinct* relations that must never be conflated (they already are separate in data):

- the **ancestry tree** — `Origin` edges (branch/clone/merge/tool-call): *spawn
  relations*. Depth, fan-out, and every agent guardrail are properties of THIS tree
  (`Graph::depth`, `Graph::spawn_children`). An `Origin` pins its attach point by
  `(episode, turn, occurrence index)` — the index is load-bearing, because
  content-addressed ids recur ([[content-addressed-ids]]).
- the **content-sharing DAG** — `Slice` references: *copy-on-write structure*. Dependents,
  GC safety (`has_dependents`), and storage cost are properties of THIS graph.

**An episode's whole life is three compositions.** Birth = `compose(spawn pieces)` + an
`Origin` edge — clone `[Span(p,0..len)]`, fork `[Span(p,0..k)]`, fresh-with-briefing
`[New(ctx)]`, briefed fork `[Span…,New(brief)]`; fan-out is N spawns. Life = a turn loop
appending `Own` turns. Return = compose into the parent (merge, squash, or result-inject
`[Span(parent), New(result)]` with `distilled_from` naming the child's turns). Child
sessions (P6) introduce **no new graph machinery** — only the *Job* (a live turn loop per
episode) and the guardrails over the ancestry tree.

**Read-side:**

- `episodes()` — list all episodes
- `turns(episode)` — list turns in an episode
- `view(episode, range?)` — render an episode (or a range of turns) for display
- `provenance(turn)` — what model/config produced this turn — asked of an
  **occurrence** (`Graph::placed`), never of a content id: identical turns
  share content, not provenance

**Write-side (graph-shape changes):**

- `start(config)` — create a new root episode with the given config
- `append(episode, user_input)` — run a turn on the given episode, mutating its
  head
- `branch(episode, at_turn=head, new_config?)` — create a new episode forking
  from `at_turn`, optionally with overridden config (e.g., switch models)
- `clone(episode, at_turn=head)` — special case of `branch` with the same
  config; useful for re-running the same prompt for variance
- `clone_n(episode, at_turn=head, n)` — clone N ways; the model-variance
  researcher workflow falls out of this
- `squash(episode, range, summary_strategy)` — collapse a range of turns into a
  single summary turn, producing a new episode. This is what "merge a side
  branch" means in our model. (See §1.3 on why "merge" isn't its own primitive.)
- `re-root(episode, new_root)` — construct a **new root** (a fresh starting
  history/context) and develop `episode`'s turns on top of it — git's
  `rebase --root` / graft. The **mirror of `branch`**: `branch` shares the
  *prefix* and diverges the *suffix* (anchored at the LCA); `re-root` diverges the
  *prefix* (new root) and shares the *suffix* (anchored at the lowest common
  *descendant*). Cheap here because Turns are position-independent (§1.1), unlike
  git rebase which rewrites every downstream hash; non-destructive (the original
  root persists). Verbatim graft vs. regenerate-the-suffix (the grafted replies
  were produced under the old root's context) is a **mode, not a gate** — the
  environment enables it and the inspector *surfaces* the seam; we do not judge the
  use (the environment stance, §1.2 below). *[direction — 2026-07-02; exact landing shape —
  new root in the same forest vs. a 3-way merge against an existing branch — is
  §1.7 open.]*
- `delete(episode)` — remove an episode from the graph. **Never orphans, never
  refuses for sharing** [DECIDED, Steven 2026-10-04]: every episode holding a
  `Slice` into it is first *detached* (`Graph::detach` — the turns it borrowed
  from the doomed episode become its own occurrences, provenance kept), as a
  git branch keeps commits another branch's deletion would otherwise take. The
  same for removing turns another episode reads past. A batch deletes readers
  before what they read, so a fork deleted alongside its source copies nothing.

**Merge is a pluggable strategy family, not a single primitive. [DECIDED,
revised 2026-06-27]** Conversation history is not commutative — you cannot blindly
zipper two branches together — so there is no one canonical `merge`. Instead, merge
is a **`MergeStrategy` trait** with a simple default and an open seam: `squash`
(collapse a branch into a summary and `append` it to the target) is the baseline
strategy, `result-injection` is the single-turn sub-agent case (§1.x), and users
can register their own. We expect several merge strategies to be wanted and cannot
predict them all (the environment stance above), so the *mechanism* is first-class
even though the v1 *defaults* are deliberately simple. (Earlier drafts said "no
merge primitive"; the intent was always to keep alternative strategies open — this
revision makes the strategy mechanism explicit rather than implying merge is
forbidden.)

**The one engine + the base triad. [DECIDED, 2026-07-06]** Every strategy lowers to
a `Vec<Piece>` (`Piece = Span{episode,start,end} | New(Vec<Turn>)`) run through
`Graph::compose` — a `Span` is projected (shared *by reference*, copy-on-write), a
`New` run is the only new data. On top of that one engine, **three base primitives
span the whole manipulation space** (any rearrangement a user could want composes from
them):

- **`pick`** — assemble an ordered list of spans cherry-picked from one or more
  episodes. The universal read-side composition: prune (keep the ranges around a
  dropped turn), sub-select, and the N-way **gather** (collect the best span from each
  of `clone_n`'s variants — the model-variance researcher's payoff, §1.2) are all
  `pick`s. "Keep everything" is `pick` of the whole range.
- **`merge-in`** — splice a source range *into* a target at a point (the branch-at-fork
  case is `MergeStraightIn`).
- **`re-root`** — replace an episode's leading turns (old root/context) with a freshly
  authored root, keeping the suffix: `[New(root), Span(episode, drop..)]`. The mirror of
  `branch`; `prepend` is the `drop == 0` case.

**Squash is `pick` + a `New` summary in place of a picked range** — and *who authors that
summary* is a separable **content-generation** layer, not part of the (pure, sync,
testable) `plan`: the `New(Vec<Turn>)` is an *input*, supplied by the user (`$EDITOR`) or
**generated by the model** (summarize this range). So LLM-squash drops in without touching
the engine, and "squash everything" vs. "squash a select few" is the *same* abstraction —
both are `pick` a range and replace it with a generated summary; the range is all that
differs. Everything is **non-destructive** (sources are only ever read, so the original
episode is the undo history) and **COW** (a pointer is cheaper than the data; `delete`
GCs branches/turns that become orphaned).

**The generation layer. [DECIDED, built 2026-07-10 — P5.4]** `S` in `:graph` asks the
backend to distill the squash sources — the selected branch's divergent turns, or a
`<Space>`-marked contiguous range (a single marked episode row = the whole session) — into
a draft summary. The request is the source conversation plus a configurable closing
instruction (`[merge] squash_instruction`); the reply streams on its **own channel**
(never into the transcript; the status row shows "distilling…", `Esc` cancels, its token
cost joins the session totals). The finished draft is seeded as the honest exchange — the
instruction sent as the user message, the model's reply as the assistant message — and
composes through the *same* tail as a hand-authored `s`: the pure strategy never knows who
wrote the summary. **Whether it passes through `$EDITOR` first is `[merge] distill_review`**
(default off; `:distill!` the other way once), as `:compact`'s is `compact_review` — it
always did until 10-21, and so an arriving draft took the terminal whatever you were
typing (full review A 8; Steven: "Distilling is like compaction so they should act
similarly"). Unreviewed, it composes on arrival and moves no pane: a range squashed in
place replaces its session, and a result beside its sources lands in a buffer of its own. Two provenance
facts make a squash inspectable afterwards: the summary turn(s) carry
`Provenance.distilled_from` (the content-addresses of the turns they replace —
machine-readable "what does this stand in for", shown by the inspector), and the sources
stay resolvable because squash preserves the originals. The marked-range mode is the
first **compaction** consumer: `s`/`S` on marked turns collapses them in place. A gapped
or cross-session mark set is *refused*, not widened — silently squashing unmarked turns
is the data-loss shape the model forbids.

**Editing the JSON is the universal manipulation, and the diff is self-checking. [DECIDED,
built 2026-07-06]** The editable view is the true Messages-API JSON (§1.1). When it's saved,
`Graph::reconstruct(target)` diffs the edited turn sequence against **every existing node**
by content-address and emits the minimal `Vec<Piece>` that reproduces it: any turn that
already exists *anywhere in the graph* is shared by reference (a `Span`, `project`ed to its
true owner — even across branches), and only genuinely-new turns are owned. Branch (edit the
middle), re-root (prepend), prune (delete), append, and cross-branch reuse all **fall out of
where the known/new boundaries land** — the diff decides the op. Because turns are
content-addressed, this *is* the consistency check: an unchanged turn is recognized as its
node, and `compose(reconstruct(t)) == t` by construction. *Deferred to a 3-way discussion
(each a "more complex decision"): a **convergent re-root / 3-way merge** that reconnects with
an existing branch — needs the suffix-LCA (mirror of `common_prefix_len`) as its base and a
pluggable **conflict resolver** (take-ours / take-theirs / interleave / model-reconcile),
since conversation content isn't line-mergeable; and a warn-not-block **validity** pass (role
alternation, tool-use/result pairing) once content blocks make an invalid composition
possible — with turn-granular `[user, assistant]` nodes the reconstruction is well-formed by
construction, so it's not earned yet.*

### 1.3 Mutability of history

**[DECIDED] — Option β: immutable on disk, mutable working view, with
explicit branch indicators.**

Turns are append-only on disk; the UI exposes a "working view" that can be
edited until commit. The two failure modes of β — silent branching and the
"why do I have 12 branches?" footgun — are addressed by making **every
implicit branch visible**. When the user edits a past turn, the UI surfaces a
banner identifying the parent turn and the new ref that will be created. No
mutation happens behind the user's back.

Editing the last user prompt before the assistant responds is a true no-op
(the turn hasn't been written yet). Editing a committed past turn always
produces a new branch on commit — never an in-place rewrite of history.

**Evidence.** The pattern of "immutable base + mutable-feeling overlay" is
well-studied. L-Store (Sadoghi et al., 2017) builds it for OLTP/OLAP at
database scale via base records + append-only tail records with explicit
lineage pointers; Datomic builds it as time-as-first-class facts with
"as-of" queries; Irmin and Dolt build content-addressed Merkle DAGs for
similar reasons. Our needs are tiny compared to any of these (one user,
thousands of turns, not millions of rows), but the architectural pattern
transfers directly: history is the truth, the working view is a projection.

This decision drives §1.3.1 (storage model) and forecloses any UX where the
user expects to mutate history without producing a branch.

### 1.3.1 Storage model: immutable log + edit overlay [DECIDED]

The conversation/history graph is defined in §1.1 in terms of **logical**
operations (`append`, `branch`, `view`). Storage is a **physical** concern,
and is hidden behind a `Store` trait (Rust). The trait surface is
deliberately small — no SQL, no Datomic-specific datalog. We want **the
backend to be swappable** for two reasons:

1. The "right" backend will change as scale and needs change (today's JSONL,
   tomorrow's SQLite, possibly a Datomic-flavored fact store later).
2. Anyone forking the project should be able to swap in their own storage
   layer (research workflows that want raw Postgres, embedded users who want
   in-memory) without touching the graph semantics.

**Trait surface (sketch, names will firm up in §3):**

```
trait Store {
    fn put_turn(&mut self, turn: Turn) -> Result<TurnId>;
    fn get_turn(&self, id: TurnId) -> Result<Turn>;
    fn put_episode(&mut self, ep: Episode) -> Result<EpisodeId>;
    fn get_episode(&self, id: EpisodeId) -> Result<Episode>;
    fn list_episodes(&self) -> Result<Vec<EpisodeRef>>;
    fn put_ref(&mut self, name: &str, id: EpisodeId) -> Result<()>;
    fn get_ref(&self, name: &str) -> Result<EpisodeId>;
    fn graph(&self) -> Result<Graph>;  // may be cached
    // ... compaction hooks and overlay reads, see below
}
```

Anything that reads or writes the conversation graph goes through this
trait. **The CLI, the TUI, the orchestrator, and plugins all consume
`&dyn Store` rather than knowing about JSONL.** That's the elegance bet: if
the model is right, swapping the backend is a single new `impl Store` and
nothing else changes.

**Default backend (M-something, not M1):** an append-only JSONL log per
episode plus an **edit overlay** that records "turn X has been superseded
by turn X' in this branch." The overlay is the architectural analog of
L-Store's *tail records* with indirection columns — the base log stays
intact, edits get appended elsewhere, the working view is materialized by
walking the overlay over the base.

Concretely:

```
$XDG_STATE_HOME/shannon/projects/<project-id>/
  episodes/
    <episode-id>.jsonl       # base log: append-only, one turn per line
    <episode-id>.overlay     # edit overlay: turn-supersession records
  refs/<ref-name>             # ref → episode-id
  graph.json                  # cached DAG, rebuildable
```

A read for `view(episode)` joins base + overlay. A compaction pass (the
analog of L-Store's "contention-free merge") can periodically rewrite
base + overlay into a fresh base log, in the background, without blocking
foreground work.

**What this preserves vs. CC:**
- `cat`, `grep`, `tail -f` still work on the base log.
- The overlay file is also JSONL — equally inspectable.
- The compaction process is observable: its input and output both live on
  disk, so a researcher can rerun it with a different policy.

**What we're betting on:**
- Linear JSONL scans stay cheap enough for episodes of realistic length
  (let's call it 10k turns). If they don't, the trait lets us swap in
  SQLite or a content-addressed object store with the same semantics.
- The overlay format stays simple. If we find ourselves piling up overlay
  types, that's a design smell — push richer semantics into a different
  backend rather than overload JSONL.

[OPEN] in this section, to revisit when we actually implement:
- Exact overlay record format (just `{supersedes: turn-id, with: new-turn-id}`?
  Or richer — partial edits, attribute-level overlays à la Datomic EAV?).
- Whether `graph.json` should be present at rest or always rebuilt at
  startup.
- When (if ever) to flip the default backend to SQLite for performance.
  PROPOSED 2026-10-01 with measurements: `storage.md` (packed store, lazy
  turn bodies, compression per turn, encryption-ready).

### 1.4 What travels with an episode (state propagation)

When you `branch(episode, at_turn, config)`, what carries over to the new
episode and what doesn't?

**Always carries (DECIDED):**
- Message history up to `at_turn`
- The active config at `at_turn` (model, params, system prompt, tools available)
  — unless explicitly overridden by `config` param
- The episode's `provenance` chain

**Carries by default, overridable per-branch (DECIDED):**
- Project-level state: `CLAUDE.md`-equivalent (call it `SHANNON.md` or
  `.shannon/context/`)
- Skills / a memory directory
- MCP server connections
- Tool definitions / restrictions
- Permission rules

**Does NOT carry; explicit setup required per episode:**
- Worktree / filesystem state. If you branch and want filesystem isolation, you
  create a worktree explicitly. Episodes are *logical*; worktrees are
  *physical*. They're bound by ID but not coupled. This preserves CC's worktree
  primitive (which is the right one) without making it implicit.
- Running background tasks.

**[OPEN]:** in-flight task graphs (TodoWrite-equivalent). My instinct says these
*should* carry — if I branch mid-task to try a different approach, I probably
want my plan intact. But it's worth arguing.

This is the answer to my earlier "what about state/memory?" critique of the
vision doc: episodes carry a *state-snapshot reference* at branch time. State
changes after the branch diverge per-episode. Conceptually similar to
copy-on-write filesystem snapshots.

### 1.5 Persistence

**Storage location:** `$XDG_STATE_HOME/shannon/projects/<project-id>/`. Project
ID is derived from the project's root path (hash) plus a human-readable name
when available.

**Layout (first proposal, OPEN):**

```
$XDG_STATE_HOME/shannon/
  projects/
    <project-id>/
      meta.json              # project name, root path, etc.
      episodes/
        <episode-id>.jsonl   # one episode per file; turns are JSONL records
      refs/
        main                 # text file containing episode-id of "main"
        <other-ref>          # additional named refs
      state-snapshots/
        <snapshot-id>/       # state captured at branch points
          SHANNON.md
          skills/
          ...
      graph.json             # parent-pointer DAG (rebuildable from episodes/)
```

JSONL for episodes because:
- `cat`, `grep`, `tail -f` all work
- Easy to repair manually if something goes wrong
- No format dependency beyond JSON
- Append-only matches the immutable-turn model

`graph.json` is a cache; rebuildable by scanning `episodes/*.jsonl` if it ever desyncs.

**[OPEN]:** how to handle very long episodes (thousands of turns). JSONL parses
linearly; a slow startup is a real risk. Mitigations: lazy-load (only parse
turns near `head` for display), or fall back to SQLite when episode size crosses
a threshold.

### 1.6 Relationship to provider API messages

The provider's `messages` array (the thing we POST to Anthropic / OpenAI / etc.)
is a **derived view** of an episode at a specific turn. It is not the source of
truth.

When we make an API call:
1. Take the episode up to its current head.
2. Apply any active compaction strategy (default: none; see §9 for compaction
   policies).
3. Project to the provider's message format (Anthropic messages, OpenAI chat
   completions, etc. — handled by provider abstraction, §4).
4. Send.

The response gets parsed back into a Turn and appended to the episode.

This separation means:
- Compaction is a render-time transformation, not a state mutation. The original
  turns remain on disk. (You can always re-render an episode without its
  compaction layer.)
- Switching providers mid-episode is a normal operation, not a special case.
- Multi-provider experimentation falls out naturally: `clone(episode,
  with_provider=openai)` is just `clone` plus a config override.

This is a significant divergence from CC, which mutates the messages array
directly under compaction. Our model preserves the history; theirs preserves the
in-context view. [DECIDED]

### 1.7 Open questions parked here

- **Task-graph propagation on branch** (§1.4): carry-by-default or explicit?
- **Persistence layout** (§1.5): JSONL-only, or hybrid with SQLite past a size
  threshold?
- **Convergent `re-root` / 3-way merge** (§1.2): the *base* `re-root` (new root +
  kept suffix as a **new root in the same forest**) is **[DECIDED, built]**. The open
  variant is the **literal 3-way merge against an existing branch** — a re-rooted (or
  edited) history that must *reconnect* with a branch it diverged from. Needs the
  **suffix-LCA** (`common_suffix_len`, the mirror of `common_prefix_len` — earned here,
  not before) and a pluggable **conflict resolver**, since conversation content isn't
  line-mergeable. Revisit as its own planning pass.
- **How to expose the graph to the user.** The model lets you do beautiful
  things; the UX for that is §2 work. But a sketch: a `:graph` view in the TUI?
  A side pane showing branch structure? Worth thinking about now so §2 doesn't
  accidentally close off options.
- **Sub-agent representation in the graph.** A sub-agent's conversation is its
  own episode with a special parent pointer (a "tool call" edge from the
  parent's turn). This generalizes CC's Agent tool but is worth pinning down in
  §1.8 or a §1.x extension.
- **Naming. [DECIDED 2026-06-27]** "Episode" is the canonical name — it leverages
  natural/existing terminology and carries the RL-trajectory connotation (a full
  rollout you can replay/branch). `Conversation` (the current type) stays only as a
  transient migration façade and is retired once `Episode` is the model.

---

## §1.x Sub-agents as episodes (sketch, not full spec)

This belongs in §1 because it falls directly out of the graph model, but I'm
leaving it as a sketch to avoid over-committing in iteration 1.

A sub-agent invocation creates a **child episode** with:
- A `parent` pointer of kind `tool-call` (vs. the regular `branch` kind)
- Its own `config` (can differ from parent — different model, different tools)
- Its own `state-snapshot` (typically inherited from parent at invocation time)

When the sub-agent finishes, its result is injected as a **tool result** in the
parent episode's current turn. The parent doesn't see the sub-agent's full
history — just the final result. The sub-agent's episode is still on disk and
the user can inspect it.

This generalizes the vision doc's "subagent merge strategy" picture: the trivial
merge (single-turn subagent) is `result-injection`, and a multi-turn subagent
that needs to report back richly uses `squash` on its own episode and injects
the summary.

Result: sub-agents are first-class graph citizens, multi-turn by default,
inspectable, and the user can `branch` from any of their turns to debug. This is
one of the strongest differentiators from CC, where sub-agent history is
ephemeral.

**Our own sub-agents, recursively — with guardrails and inspectability.** Beyond
calling Anthropic's Agent tool, we want users to compose their *own* sub-agents to
arbitrary **depth and width** (a sub-agent that spawns sub-agents, fanning out).
People will experiment here, so two things are non-negotiable:

- **Recursion guardrails.** Explicit caps — max depth, max fan-out, and a budget
  (turn/token/time) — that refuse to spawn past the limit and surface *why*, so a
  runaway tree cannot recurse infinitely. The caps are themselves configurable (the
  environment stance) with safe defaults.
- **Inspectability.** What an agent *can* do is part of what it *is*: the tools,
  MCPs, manifests, harnesses, and capabilities in scope for each episode are
  recorded in provenance and surfaced in the viewer, so power users and researchers
  can see exactly what was available at every node (to the extent the upstream
  provider allows).

---

## Open meta-question

The doc is currently written like an architectural memo. It could also be
written more like an RFC ("Proposal: …, Alternatives considered: …, Rejected
because: …"). Daily-driver engineers might prefer the memo style; researchers
and would-be plugin authors might prefer the RFC style. Pick before §2.

---

## Earned-abstraction roadmap (2026-06-18 elegance review)

Three Opus reviewers studied the whole system (editing spine / render+viewport /
whole-system seams) for abstractions that *simplify and optimize at once*. The
strong convergence: **the system is already well-factored — build each
abstraction *with* the feature that earns it, not before.** The recurring tell
of an *under*-built abstraction is "resolved data trapped inside a mutating
producer, or bolted on as a parallel array the renderer re-derives every frame."

**Bias caveat (recorded for honesty).** These reviews ran from prompts *we*
wrote, seeded with the same forward roadmap and the "simplify + optimize"
framing — so the apparent convergence is partly our prompt design echoing back,
not three independent minds. Trust the findings grounded in *checkable in-tree
evidence* over the seeded ones: the backend's dropped `content_block_*`/usage/
tool-use events (the `=> None` arms in `anthropic.rs`), the prompt's `selection`
parallel array re-split per frame in `selected_line`, and the absence of any
read-without-mutate path for yank. The build-order / earned-use *conclusions* are
more our-hypothesis-shaped — treat them as well-stress-tested hypotheses, and
re-derive each against the code when its feature actually arrives, not now.

Map of which abstraction lands with which upcoming feature:

| Upcoming feature | Earned abstraction | Why (simplify + optimize) |
|---|---|---|
| **yank `y`/`p`** (next) | **`RangeShape` + `resolve`/`read`/`drain`** split on `Prompt` | Yank must *read* the exact span delete would *drain*, without mutating. Today that geometry is trapped in the mutating `operate_*` producers. `resolve(target)->RangeShape` (pure; reuses `motion_target`'s probe), then `read` (yank/`selected_text()`/future add-to-context) vs `drain` (today's 3 delete bodies, verbatim). Avoids a parallel yank resolver. Confirm yank scope is the full grammar (`yw`/`yy`/`yi"`) — it is — which is what makes it earned vs. visual-only. |
| **visual block `Ctrl-V`** | `RangeShape::Block` variant (+ block-aware read/drain) | One new variant + one read arm + one drain arm, not a fourth fused producer. This is the feature that finally earns the resolved-shape value. |
| **mouse** | **`LineMap`** invertible wrap map in `wrap.rs` (`locate` forward, `hit` backward) | The forward char→(row,cell) map is fused into `Prompt::visible_window`; the backward inverse doesn't exist but mouse *and* `Ctrl-V` both need it. Pull into one tested transform with `row_starts` as a field; `hit` is then ~free. Also makes the prompt's per-frame geometry cacheable (O(viewport), matching Output). |
| **syntect highlighting / bubbles** | **`StyledRow { text, spans }`** as the row currency (replaces `Vec<String>`) | The prompt's `selection: Vec<Option<(usize,usize)>>` parallel array + per-frame `selected_line` re-split is the canary. syntect produces styled spans with nowhere to live in the `Block` cache; on `Vec<String>` they'd be re-styled per frame (O(viewport·spans)) or stapled on as more parallel arrays. One boundary-clipping routine (the `lo.max(rstart)..hi.min(rend)` dance already written for selection) then serves syntax + selection + bubbles. **Must land with syntect, not after** (smaller rework now: one styled overlay to migrate). |
| **prompt vertical overflow** | `Viewport` **`Anchor`** enum (`FollowTail`/`FollowPoint`/`Frozen`) | Share the scroll *state machine* (already in `viewport.rs`, used only by Output); keep `visible_window` per-pane. Wiring Prompt's cursor-follow `top` through it gives prompt scrollback for free. (Don't merge the content windowing — that would over-couple a cursor editor with a scrollback log.) |
| **usage display** | `StreamMsg::Usage` variant (part of ↓) | The variant *is* the feature; the view just reads it. |
| **tool-use / OAuth (M3)** — *the one exception: do BEFORE its forcing feature* | **Typed `StreamMsg` + `Message::content: Vec<ContentBlock>`** | The only seam whose retrofit cost is *super-linear* if deferred: it's the contract shared by every future `Backend` at once. The Anthropic backend already *drops* `content_block_start`/`message_delta`(usage)/tool-use/thinking because `StreamMsg` has nowhere to put them. Widen while there's one real backend + a stub. Unblocks usage, tool-use, thinking, and a *real* content hash for §1. |
| **conversation graph (M3)** | **`Turn { user, assistant, provenance }`** as `Conversation`'s unit (NOT the graph) | Cheap move that avoids a retrofit: today a turn is implied by adjacency in `Vec<Message>` and `fail_turn` is a positional `pop()`. A `Turn` makes role-alternation structural, gives `provenance` a home, and makes content-addressing hash a `Turn` not a guessed pair. Do **after** the protocol widening (else `assistant` bakes in the lossy `String`). Leave Episodes/branches/`Store` deferred (§1.5 is OPEN). |

**Explicitly leave alone (forward doors already open, abstracting now = premature):**
- `KeyOutcome`/`EditAction`/`OpTarget` — correctly 3-layered (prompt primitive / operator noun / pane verb); collapsing would re-couple.
- `motion_target` save/probe/restore — the right "one definition of where a motion lands"; build `resolve` on it.
- `App` god-struct / `Pane`/`Workspace` — trigger is the 3rd pane or design.md §2 being written (currently empty). The `Focus{Prompt,Chat}` binary branched in 3 spots is the watch-point; the retrofit stays local because `classify`/`Box<dyn Keymap>`/pure-`draw` kept topology out.
- `Arc<Config>` — already shaped for live-reload (parse → swap the `Arc`); per-pane config grows a table when §2 lands.
- Full `Pending` enum into Visual — share only the motion-accumulator (count+prefix→`resolve_motion`) when Visual grows counts; the operator half is Normal-shaped.

**Sequencing when these land:** protocol-widening → `Turn`; `LineMap` before/with `StyledRow`; `RangeShape` as the *first step* of yank.
