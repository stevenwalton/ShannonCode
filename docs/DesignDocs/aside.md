# Actions aside — an experiment

STATUS: **PROPOSED 11-02, nothing built; experimental, off by default.** The
idea is Steven's (11-02, reading `file-tools.md`):

> given the graph structure we could perform any tool action without
> filling up the model's context … 1) We make the request to a subagent who
> does the file editing … 2) We could do a fork (in the background) where
> the model does the action and we merge back only with the result … we
> keep the context history when the editing is made. We just don't carry
> forward the extra information … something similar to Anthropic's
> microcompaction, but in a continuous manner.

Tags: *verified* (read in Shannon's code or public docs this session),
*inferred*, *unknown*.

**What it is for: context management, not money** (Steven, 11-02: "the
idea seems more about context management rather than saving of money …
if it helps or harms is something we'll have to experiment with and we're
not going to get the full results anytime soon … there could be
situations where it is useful and other times when it isn't"). §3's
prices are a side effect. The question the experiment answers is which
steps, if any, a model does as well or better without their material in
its window.

## 1. What carries forward, and what it is worth

What a tool action leaves in the context: the call (an Edit's `old_string`
and `new_string`, the model's own output) and its result (a Read's file,
a test log). Which of that is still worth its tokens later depends on what
it is *for*. This is my read (*inferred*: a model cannot measure what a
token in its context did; the experiment can):

- **Material for the next exact act is worth a lot**, until that act is
  done. An Edit needs `old_string` byte for byte. Without the Read in
  context the model re-reads (the tokens again) or reconstructs from
  memory (whitespace, names), and the second is how edits fail.
- **Facts that constrain later design are worth a lot, and are the
  hardest to keep.** A signature, an invariant, a comment saying why, a
  test helper seen in passing. A summary written at the time keeps what
  its writer thought would matter. What turns out to matter later is the
  thing it missed. After a compaction I re-read files to cite them,
  because the summary had the conclusion but not the line.
- **Material consumed by a conclusion is worth little afterwards**: a
  search's hits once the place is found, a listing, a passing test's log,
  a failed attempt (BSD `sed`'s error), a file read to answer one
  question.
- **Noise has a cost beyond tokens.** Anthropic's compaction page gives
  this as the reason for compacting: it "keeps the active context small,
  because response quality degrades as a conversation grows" (*verified*,
  11-02). The long-context literature says the same of irrelevant material
  (*recalled*). So leaving it behind may help the answer as well as the
  bill.

So an aside fits a step whose output is a **conclusion**: answer this
about the code, find where X is, run the tests and say what failed, make
this mechanical change. It does not fit material the parent is about to
reason over, or a region it will edit again. The decision belongs where
the plan is, with the model, at the moment of the action.

A wrong decision should cost one re-read, not a wrong answer. The graph
keeps the aside's branch, so a later `recall` (the model's) or `:b` (the
user's) brings back what was left. That makes the knob safe to turn
("verbosity is a knob": folded, never absent).

## 2. Prior art (verified 11-02, from Anthropic's public docs)

- **Anthropic's context editing** (beta, `context-management-2025-06-27`;
  the context-editing page):
  - `clear_tool_uses_20250919` clears the oldest tool results past a
    `trigger` (default 100,000 input tokens), keeping the last `keep`
    (default 3).
  - The calls stay visible unless `clear_tool_inputs` is set. Some tools
    can be spared with `exclude_tools`.
  - `clear_at_least` exists because each clearing **invalidates the
    cache**: it makes a clearing worth the re-write.
  - It is applied server-side: "Your client application maintains the
    full, unmodified conversation history". That is a projection, as an
    aside's is, but drawn after the fact.
  - On 5.1+ models a server-side edit never invalidates thinking, while a
    client-side edit of earlier turns can.
  - `count_tokens` takes `context_management` and returns the count before
    and after, so a clearing can be priced without a run.
- **Compaction** (`compact-2026-09-04`; on demand, at a threshold, or in
  the background, optionally keeping recent turns word for word): the
  docs' primary strategy. It summarises the whole history; an aside
  summarises one action.
- **Programmatic tool calling**, the nearest relative: Claude writes code
  that calls your tools inside Anthropic's code-execution container, and
  "intermediate results are not loaded into Claude's context window".
  - The docs report +11% on agentic-search benchmarks with 24% fewer
    input tokens.
  - Each client tool call still pauses the code and comes back as a
    `tool_use` the client answers, so Shannon's gate would still see every
    call.
  - The code runs in Anthropic's container: not ZDR (zero data
    retention), data kept up to 30 days, and an idle container is
    reclaimed after about 5 minutes. A pending call times out after about
    4 minutes, which is shorter than a human takes over an approval pane.
    That rules it out as Shannon's mechanism, but it is evidence that
    keeping intermediate results out of the context pays.
- **The memory tool** (`memory_20250818`) is the same just-in-time idea for
  what must outlive the context: a file read back on demand.

**How an aside differs.** Those clear *after the fact*, by threshold, and
need the cache rewritten or a private API to avoid it. An aside is decided
*at the action*, and it is append-only: the parent's prefix never changes,
so its cache stays valid on the public API, and so does a thinking check
that binds thinking to its prefix (N1c). It is also lossless in the graph,
since nothing is cleared, only not carried.

## 3. What it costs (the model to measure against)

Let C be the context before the action, M the action's material (reads,
edit text, logs, round-trip overhead), R the result brought back, and K
the requests the session makes afterwards, before it ends or compacts.

- **The cache read price is the model's**, as a share of the input
  price (prompt-caching page, 11-02): 0.1× generally; 0.05× on Opus 5.5 and
  Sonnet 5.5; 0.025× on Fable 5.1. A cache write is 1.25× (2× at a 1-hour
  TTL, time-to-live). Call the read share ρ.
- **Inline:** every later request carries M as cache reads, so ρ·K·M,
  plus M written to cache once.
- **Option 2, a fork:** its requests carry the parent's prefix, which is
  already cached, so the action costs about what it costs inline, plus a
  small overhead: the call, and R written by the fork. Every later
  request carries R instead of M. **Saved ≈ ρ·K·(M − R) input-price
  tokens.** On Opus 5.5, ρ = 0.05: a 20k-token detour followed by 40
  requests saves about 40k input-price tokens, roughly the cost of sending
  40k fresh once. That is modest. The larger effects are likely the window
  filling later (pushing back compaction, the lossy and expensive event)
  and quality (§1). (*Inferred* from the pricing shape. The experiment
  measures it.)
  - Whether a fork's first request reads the parent's cached prefix is
    *unverified*. The docs say a hit needs a 100% identical prefix and do
    not discuss branches, so the experiment checks the fork's first
    `cache_read_input_tokens`.
- **Option 1, a fresh subagent:** its prefix is small, so the action
  itself is cheaper. But the parent must write the task out, and the
  worker knows only what that says. Its edits are likely worse (*a
  guess*), measured in the same runs.
- **Never saved:** an Edit's own `new_string`. Someone writes that code
  either way, and an Edit delegated alone moves its text rather than
  saving it. The saving is in what the parent *reads*.

## 4. The shape

- **One tool, not a parameter on each.** The material worth leaving is
  a sequence (read, decide, edit, test), and the parent's call to a lone
  Edit already holds the text. So: `aside { task, bring_back? }` (the name
  is open). The file tools keep Claude Code's required arguments (S21-Q25)
  and can still take optional ones, for a narrower experiment such as a
  Read that answers a question.
- **A fork child at the call.** It inherits the session's context and has
  the session's tools without `aside` (depth 1 for the experiment). It is
  a spawn edge, so `[agents]`' `max_depth`, `max_fanout` and `on_limit`
  apply. Its calls go to the gate as the parent's would. Its last message
  is the result, cut to `bring_back` if given.
- **The record** reads: `aside: <task> — 3 reads, 1 edit, 40 s`, folded,
  its branch openable. Nothing moves focus (no teleports).
- **Read-first composes** (`file-tools.md` §3.6): stamps are read from
  the context sent, and the parent's context does not hold the aside's
  Reads. So the parent must Read before its own Edit. That is correct:
  it never saw the file.
- **Off by default**, behind an experimental switch, probably
  `[agents] aside`. It is read when a session starts, never mid-session:
  the tool list is the first thing in the cache prefix, and changing it
  invalidates every cached level (prompt-caching page).

## 5. The experiment

- **Measured per request:** Shannon's `Usage` already keeps input, cache
  write, cache read and output apart (`stream.rs:84`). Per task: tokens
  weighted by price, requests, the parent's peak context, wall time.
- **Tasks:** a fixed set on a scratch repo: a rename across files, a
  failing test fixed, a function added with its test, a module explained
  then changed.
- **Four arms, each run several times:** inline; inline under the API's
  own clearing (`clear_tool_uses_20250919`), the baseline an aside must
  beat; a fork aside; a fresh aside. `count_tokens` is free and has its own
  rate limit, so it can price a prompt before any spend.
- **Outcome as well as cost:** do the tests pass, did the parent re-read
  what an aside had read, how many turns. A cheaper run that fails costs
  more.
- **Spend:** live API, the key by path. Estimated before running and
  asked of Steven before spending.

## 6. Open

- The tool's name (`aside`, `fork`, `detour`); whether option 1 runs in
  the same experiment as a `fresh` flag; whether the model may `recall`
  an aside's branch, or only the user may.
