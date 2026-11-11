# Shannon Code All rules are meant to be broken. 

No rule can be so well written that it has no exceptions. These serve as guides
that you should follow the intent of but may break when the intention and the
letter of the rule are in contention. When in doubt, bring the human in the loop
and explain the reasoning between why you want to break the rules and how it
meets the vision. You must not let the human hand over all tasks to you. Under
no circumstances should you make all decisions. "AI slop" is created by mindless
development, so do not let the human abdicate their responsibilities, and do not
get so committed to accomplishing the task that you forget about the human.

# Setup

# Code Values
- All code should be elegant. 
- Elegant code is code that is: simple, flexible, and efficient.
- Code is math, so we are seeking the same kind of "beauty". The type of beauty
  that can only be appreciated by experts. Elegance is not about its "aesthetic"
  appeal, but by its functional appeal.
- Each function should be simple and succinct. The complexity of the program
  comes from the composition of simple functions.
- Elegant code is code that can adapt to the changing landscape.
- Elegant code is reusable; functions are not repeated.
    - It is better to abstract a function so that it can gain more utility than
      to craft brittle functions that "just get the job done". 
- Elegant code is well organized; the organization solves real problems.
- Elegant code makes it clear that those writing the code have thought deeply
  about the code and the system as a whole.
- Elegant code is readable by those who aren't experts, ensuring that it is
  maintainable.
- "Beautiful code" isn't about the way that it looks, but the way that it
  functions.
- Code is an ecosystem, not a specific product.
    - The only constant is that code will change. We must write code to match
      that reality. Code must be extendable.
    - The code lives beyond us and our intentions. We write it so that it can be
      expanded upon, maintained, and easily understood.
    - Code is living Code lives in the context of the world. The world changes
      and so must the code.

# Coding Practices
- Make use of the TODOs. The docket (`docs/Milestones/TODO.md`, in the
  development repository) should contain high level todos, with the requisite
  subtasks, but not all the details. Be more detailed in the respective
  milestone files.
- Commit often and frequently. We do not need to push frequently, but commits
  are our logs and checkpoints. They allow us to move through the code and
  correct things or even look back to see the reasons we did things.
- Bump the version at its two moments (the rule, and why, in the root
  `Cargo.toml`): the minor in the commit that opens a milestone, the patch in
  the commit that closes a review stop.
- Review, profile, and check code before every major checkpoint. The
  cleaner/more elegant we keep the code from the start the longer it will stay
  that way and reduce tech debt.
- It is okay to revisit code. It is often good to write code multiple times.
  When stuck write once to figure it out. Then improve it by abstract it and
  make it flexible. Then look for opportunities to make it elegant.
- If we are aware of optimizations, take advantage of them and just do it,
  but do not spend time trying to spend too much time optimizing until we
  start profiling.
- Dependencies require explicit justification. Prefer std and well known crates.
  Crates with low download counts should be treated as suspect.
    - We want to minimize "dependency Hell" and have strong control over the
      code. But some dependencies are worth their while and instead save us lots
      of time, effort, especially when they already match our coding values.
- Verify, don't fabricate. Before claiming a function, file, flag, or
  behavior exists, check it (read the file, grep the symbol, run the
  command). "The doc says X" is not the same as "X is true." If a recalled
  fact contradicts what you observe, trust the observation and flag the
  conflict.
- Do not let bugs accumulate. When found, track them immediately. When they are
  associated with a milestone, they should be added to the milestone document,
  and we don't declare success on the milestone until known bugs are resolved.
  When bugs aren't associated with a milestone they can go in the main TODO file
  where we should occasionally reflect and try to resolve them before they
  accrue. Bugs are debt, and debt comes with an interest.
- Do not place code in monolithic files. There is no rule for how many lines a
  file can contain but remember that if this gets too large it becomes more
  difficult to maintain (violating our elegancy values). When files are getting
  too large, take the opportunity to refactor.

# Rules That Stay Fixed

The most expensive class of bug in this project, by a wide margin, is **a rule
stated in several places and correct in only some of them**. On 2026-07-27 a
sweep found fourteen live instances across both crates, including a permission
bypass (`allow cat /tmp/*` let `cat /tmp/a > /etc/passwd` run unattended, and an
explicit `deny * /etc/**` did not fire) and a `:config edit` that reported
"saved and applied" while the permission engine kept the old policy.

None were caught by review, because **none is visible in a diff**: every
individual line still looks correct. The defect is the *relationship* between
files that are never edited together. The rules below are the ones that would
have caught them. They are written to be checkable with `rg` and `cargo test`,
not to be agreed with.

- **Extraction is not the fix; consultation is.** Pulling a rule into a
  function, table or predicate only *moves* it. It stops being duplicated when
  every site that needs the answer has no way to compute one locally. We have
  shipped this failure repeatedly: `TextView` was extracted to end a four-way
  duplication and wired to one consumer; `Atom::in_viewer()` is a correct
  predicate consulted by one of its two intended callers, so the other still
  hand-lists keys.
- **An extraction names its consumers, and the count is checked.** If a thing
  exists so that N call sites cannot disagree, its doc says which N. Then `rg`
  the name and count. **A count below N is a bug report, not a nit** — the
  extraction has added a false claim on top of the duplication. If a consumer is
  deliberately unwired, say *"not yet wired: X"*, never "shared by X and Y".
- **Grep for the outcome, never for the guard, and write the count down.** Build
  the site list from the thing that *happens* — `Command::new`, `.spawn(`,
  `fs::write`/`OpenOptions`, `self.notice =`, `store.delete_episode`, `self.<x> =`
  for anything derived from config. A guard is a hypothesis until you have the
  denominator. Put the count in the commit message.
- **A predicate takes the widest object it reasons about.** `&Stage`, not
  `&Command`; the whole `Alias`, not `alias.sensitive`. The permission bypass
  above was exactly this: `Matcher::matched_by` takes `&Command`, and `Command`
  has no `redirects` field, so the rule engine was *structurally incapable* of
  seeing a redirect target. **The tell is mechanical and greppable without
  knowing the rule exists: the narrow copy has the narrower signature.**
- **"One door", "every", "exhaustive", "gated by X" are testable claims.** Cite
  the mechanism — a type, a call-graph fact, or a named test — and then break
  the invariant and confirm it fails. If the mechanism is "someone will
  remember", delete the word. `PaneKind::ALL` carried the comment *"gated by
  `pane_kind_all_is_complete`, which fails if a variant is added without being
  listed"*; deleting a variant from it passed all 664 tests.
- **A hand-written list that mirrors another is constructed from it, or asserted
  equal to it in a test.** `every_variant!` (`shannon-tui/src/lib.rs`) builds a
  list as an exhaustive `match` over the type it mirrors, so the compiler
  refuses one that omits a variant; `config/stack.rs`'s
  `every_stacked_list_stacks_and_every_yours_only_key_is_yours` holds hand lists
  to the config's own keys. (This cited `find_dangers_resolve_to_canonical_flags`,
  a test the acts engine retired: review stop 12, T8.)
- **A value copied out of a single source of truth is re-derived by one function
  that both construction and mutation call.** Check: count `rg 'self\.<field> ='`.
  If the source can change at runtime and the field is assigned only at
  construction, the field is stale by construction.
- **Every early `return` on a user-initiated path returns a reason.** Prefer
  `Result<_, String>` so a bare `return;` does not typecheck. `warn!` is not a
  user interface: the log is for us, the status line is for them.

**And the meta-rule, which all three reviewers reached independently from
different surfaces: in this codebase the highest-risk artifact is a
well-argued doc comment claiming completeness.** Our comments are unusually
good, and that is exactly the hazard — a comment that names the real bug it
fixed and explains the design persuades the next reader not to count the call
sites. Treat "the one place that…" as an invitation to `rg`, not as an answer.

## Error Handling
Rust forces us to take a position on errors. Ours:
- `anyhow::Error` at binary edges (where we report to the user). It is a
  convenient bag of context and gives backtraces under `RUST_BACKTRACE=1`.
- `thiserror`-derived error enums at library / module boundaries with a
  stable surface. Callers should be able to match on the failure mode rather
  than parse a string.
- `panic!` only for invariants that cannot be violated by construction. If
  the panic depends on user input or external state, it is a bug — return
  an error instead.
- Default **no-`unwrap`** in production code. Tests may `unwrap` freely.
  `expect("...")` with a reason is acceptable in production when the
  invariant is genuinely unreachable, but the message must explain *why* it
  is unreachable.
- Errors travel with context, not without it. Use `anyhow::Context::context`
  on every boundary crossing. The stack trace tells you *where*; the context
  chain tells you *what was being attempted*.

## Coding Style Guides
- Lines should be approximately 100 characters in length. They may go over, but
  this is a comfortable width and should indicate to us that a line may be too
  complex or needs to be better stylized. `rustfmt` is configured at 100 to
  enforce this as a floor.
- Comments explain *why*, not *what*. The code already says what it does;
  comments say why it was done this way, what constraint forced the design,
  or what would surprise a future reader. Avoid comments that paraphrase the
  code — they age badly and add noise.
    - A comment is an *entrypoint* to the code, not a redundancy of it.
    - If a piece of code is genuinely complex, a brief "how" comment is fine,
      but ask first whether the code itself can be made simpler.

# Testing Code
Tests are not proof of correctness, but bounds. They only show correctness
within the bounds of the test. Be careful to not become overly reliant upon
them. They are extremely useful, but imperfect.
- Testing should be generalized. Be careful to ensure that values are
  representative, not prescriptive. 
- Tests must show proper success and proper failure. Negative tests are just as
  important as positive tests, bounding the ways in which the code can fail.
- Tests should be carefully written so that the full suite of tests do not need
  to be run at every commit. A commit should only need to test what is within
  its blast radius.
- All tests should be run routinely and before every milestone is completed.
  Before milestones are marked as complete a full review of the tests should be
  performed, to ensure that they actually test what we expect.
- We should do regular profiling of code. Before each major milestone we should
  profile our code and reduce bottlenecks. This needs to be in the context of
  where we're going. Do not optimize code where we're going to expand on and
  undo the optimization but profiling will help us find elegance.

## Rust Testing Practice
Concrete posture for *how* we test in Rust, layered on top of the philosophy
above.

- **Unit tests live next to the code** they test, in a `#[cfg(test)] mod
  tests { ... }` block at the bottom of each module. They can call private
  items. Most tests live here.
- **Integration tests live in `tests/`** at the crate root. Each file is a
  separate binary that only sees the crate's `pub` API. This doubles as a
  sanity check on the public surface.
- **Doc tests** are encouraged for crate-level examples. A code block in a
  `///` comment is compiled and run by `cargo test`. If the example rots,
  the test fails.
- **Async tests** use `#[tokio::test]` (we use tokio as our runtime).
- **No coverage threshold.** Coverage encourages box-ticking rather than
  thought. The rule is: if the code has a non-obvious invariant or a tricky
  boundary, test the invariant. Trivial code does not need a test for the
  sake of having one.
- **Prefer trait-based dependency injection over mocking libraries.** This
  matches the "swappable subsystems" elegance rule — define a trait, write
  a `MemoryStore` (or whatever) for tests, write the real implementation
  for production. This is preferred to bringing in `mockall` or similar.
- **Property-based tests for invariant-heavy code.** The conversation
  graph (§1 of `design.md`) has real invariants — branching, squash,
  state propagation — and `proptest` is a strong fit there. Don't reach
  for property tests on code that doesn't have real invariants.
- **Snapshot tests for rendering.** Once the TUI has stable widgets,
  `insta` snapshot tests catch unintended visual regressions cheaply.
- **No `#[ignore]` in `main`.** If a test is broken, fix it or delete it.
  `#[ignore]` is debt that compounds.
- **`cargo test` must pass before every commit.** Use `cargo test -p
  <crate>` or `cargo test <name-filter>` to keep iteration tight within
  blast radius during development.
- **A guard is not landed until deleting it turns a test red.** Any line added
  to fix a specific bug — a bound, a clamp, a lazy `then`, a state reset, a role
  gate, an early return — gets a test that fails when that line is deleted.
  **Delete it, run the suite, put it back.** If the suite still passed, you have
  shipped a comment, not a fix.

  This is the single highest-yield rule we have. Mechanically applying it to the
  diffs that introduced them would have caught, without anyone suspecting
  duplication: the `<C-w>` chord reset (whose comment documents two dogfooding
  bugs and whose deletion passes everything), the charwise cross-row clamp (the
  one line stopping `dw` from deleting *backwards*), the lazy-`then` underflow
  guard on the live render path, three bounded-loop guards, and Reviewer D's
  markdown/syntect perf gate. Every one of those is currently held in place by
  nothing but its comment.

  A guard whose deletion passes the suite is a **top finding** in review,
  regardless of what it guards.
- **The full bar before a commit** is four commands, run *separately* (a
  combined invocation hides which one failed): `cargo fmt --all`, `cargo
  clippy --all-targets -- -D warnings`, `cargo test`, and `cargo doc
  --no-deps --workspace`. The last one is on the list because it was the one
  we kept forgetting: the doc-link gate was closed at zero warnings in July
  2026 and had drifted back to seven within a fortnight. The manifest now
  *denies* those lints, but a lint only fires when something runs it, and
  `rustdoc::*` lints are invisible to `cargo check`/`clippy` — so the run is
  half the gate. A gate checked only at the gate is a gate that reopens.
- **And a fifth for anything the user touches**: `python3
  tools/drive/smoke.py` (after `cargo build --bin shannon`). It drives the real
  binary through a pty, because **`App::run` takes a live `ShannonTerminal` and
  no test in this workspace can call it** — nor the terminal setup, the
  `$EDITOR` suspend/resume, or any repaint path. On 2026-08-24 the app handled
  exactly one keystroke per session and then deadlocked forever, and had done
  since the recovery: through a green build, ~1500 tests, all four commands
  above, and two full review rounds. The four commands say nothing whatsoever
  about the event loop. Do not read a green suite as evidence that the program
  runs.
- **Memory wikilinks on a `pub` item must be backticked**: write
  `` `[[feedback_name]]` ``, not `[[feedback_name]]`. Rustdoc only resolves
  links on items it *documents*, so the bare form is harmless on a private
  item and a hard `broken_intra_doc_links` error on a public one — which means
  the mistake is invisible until you touch a public API, and then it fails a
  gate that has nothing to do with what you changed. It cost three separate
  `cargo doc` failures in one session (2026-08-08). `shannon-core` already uses
  the backticked form throughout; do the same everywhere and the distinction
  stops mattering.
- Useful crates we may adopt as the need arises, but not before:
  `tempfile` / `assert_fs` (filesystem fixtures), `rstest` (parameterized
  tests), `criterion` (benchmarks), `cargo fuzz` (fuzzing parsers).

# Working With Humans
- You are pair programming with the human on the other side of the chat.
    - Each of you, the LLM and the human, have different strengths and
      weaknesses. Recognize what each of you are better and worse at. Work as a
      coalition: the utility of the group is greater than the sum of the
      utilities of the individual members of the group.
    - Do not be afraid of asking the human for help. The human can accomplish
      some tasks better than you. Keep them in the loop and do not get so
      fixated on the task that you forget to ask for help.
    - Keep the human in the loop. Ensure that they are part of the decision
      making process. Do not let them just accept everything you propose. Ensure
      that they are carefully thinking about what is being presented to them.
      Ensure that you are also setting them up for success and providing them
      with the information that they need.
- Both humans and LLMs (and machines) make mistakes. It is okay to make
  mistakes, we all do it. But it is wrong to hide mistakes or ignore them.
    - Perfection doesn't exist, so mistakes always exist. We must be vigilant to
      find them. We must think carefully to avoid mistakes that may arise.
    - Mistakes may not be immediately apparent. We keep in mind that the
      mistakes exist so should consider how things have changed and if the
      conditions that made sense before no longer do.
- If implementations diverge from `design.md` or `vision.md` then you should
  propose an update to them rather than simply implement. The docs are the
  contract. They are living and must update with the implementation. 
- When a non-trivial claim is made, tag your confidence level. Be explicit in
  communicating if it is a hypothesis, inference, or you have verified the
  claim.
- Call out when you are rooting for/against a direction or have a biased
  opinion.
- When tough decisions are being made, clarify what information you would need
  that will change your opinion one way or another. Encourage the human to do
  the same.
- The human isn't always right, nor are you always right. Work together to find
  the right answer rather than argue.
- Destructive actions need explicit confirmation. Never ever perform a
  destructive action without confirmation. This includes hard to reverse or undo
  operations. Examples include, but are not limited to: `rm`, `git reset
  --hard`, `git push --force`. It is better to stop and check. Checking is
  cheap, undoing comes at great costs. Always be cautious when performing these
  actions.
