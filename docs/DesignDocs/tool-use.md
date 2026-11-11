*tool-use.txt*   Wiring the permission gate into real Claude tool calls

                          SHANNON CODE — TOOL USE

This is the payoff milestone: Claude proposes a shell command, the permission
gate we built fires, a human decides (deny / allow-once / allow-always / edit),
and — only then — it runs, with the output fed back to the model. Everything up
to now (`:analyze`, the effect model, provenance colouring) has been the gate
with no door behind it. This builds the door.

STATUS: **S1-S4 BUILT** (2026-07-24/26); S5 close-out open. Written as a draft
for review 2026-07-24 and then built, so read §4's stages as a record, not a
plan: content-block plumbing, the gate with its four actions, execution with
timeout / output cap / process-group kill, and the loop closing back to the
model are all in. The `Not started` line that stood here until 2026-07-27 was
left over from the draft. §5's deferrals still stand — in particular **there is
still no sandbox**, which is the next milestone, not a solved problem.

==============================================================================
1. WHY, AND WHAT WE ARE (NOT) DOING                              *tool-why*

The thesis in one line: **make verification so cheap it stops being fatiguing**
("trust but verify", not "hand trust to the agent"). Tool use is where that
thesis meets a real agent loop instead of a debug viewer.

Scope THIS milestone — the tool-calling loop + the human decision:
  - Claude can call a `bash` tool.
  - Every proposed command runs through `RuleSet::decide` and is shown with the
    annotated-source diagnostic (the popup, in-app).
  - The human can DENY, ALLOW-ONCE, ALLOW-ALWAYS (scoped, §2), or EDIT.
  - An allowed command actually executes; its output returns to the model; the
    turn continues.

Explicitly NOT this milestone (tracked, §5) — deferred by Steven 2026-07-24:
  - **No full sandbox.** We run commands as the user, in the user's shell/cwd.
    We are honest that this is the trust model for now; hardening is its own
    milestone we are deliberately headed toward, not pretending to have.
  - No config-loadable policy rules yet (mechanism exists, §5).
  - No configurable colour scheme yet (§5).

Current state (verified in Shannon's source, so the plan is grounded, not
guessed):
  - `message.rs`  — `Message { role, content: String }` is **text only**. The
    doc-comment already says structured content "joins the type when a real use
    case lands." Tool use is that use case → this is the one real type change.
  - `stream.rs`   — `StopReason::ToolUse` is **already parsed** from the wire
    (`"tool_use" => Self::ToolUse`). The plumbing anticipated us.
  - `backend/request.rs` — `SendRequest` has **no `tools` field** yet.
  - `backend/anthropic.rs` — the SSE parser handles text `ContentBlockDelta`
    only; it does not yet assemble a `tool_use` block (`content_block_start`
    with `type:"tool_use"` + `input_json_delta`).
  - `permission/mod.rs` — `RuleSet::decide` + the decision axes are done and
    tested. `RuleSet::extend_from_lines` / `Rule::parse` already turn text into
    policy (the seam allow-always needs).
  - `permission_view.rs` — the styled producer is done; the live popup reuses
    it (it was built for `wm::OverlayStack`).

So the gate is finished. This milestone is plumbing + the loop + the door.

==============================================================================
2. THE FOUR ACTIONS — the heart, and the one open decision      *tool-actions*

On an ASK (and the equivalent on a modeled DENY), the human chooses:

  DENY         Refuse this invocation. Return a `tool_result` with
               `is_error: true` and a short reason. The model sees the refusal
               and re-plans. Never executes.

  ALLOW-ONCE   Run THIS exact invocation, once. Return its output. Standing
               policy is unchanged. This is the default focus.

  EDIT         Open the command for editing (inline, or `$EDITOR` via the
               existing suspend/resume seam). The edited command is RE-RUN
               through the gate from scratch — editing is not an approval.

  ALLOW-ALWAYS *** SECURITY DECISION — needs Steven's explicit sign-off. ***
               NOT a blind "always allow `rm`" button. On 2026-07-21 we ruled
               that out: a one-time yes must never silently widen standing
               policy (that launders a safe invocation into a broad grant and
               trains habituation into danger). "Policy is authored, never
               accreted." The allow/deny asymmetry: remembering a broad DENY is
               fail-closed (safe); remembering a broad ALLOW is fail-open
               (dangerous).

               PROPOSAL that keeps both the convenience and that rule:
               allow-always opens the RULE SURFACE pre-filled with the
               NARROWEST rule that covers this exact invocation (e.g. the exact
               `git status`, or `allow git:status` — a selector, not a path
               wildcard), which the human REVIEWS and may widen DELIBERATELY
               before it persists. Invariants:
                 (a) default scope = the exact invocation, never broader;
                 (b) the rule text is shown and requires confirm — no silent
                     persist;
                 (c) `deny-always` may be offered freely (fail-closed);
                     `allow-always` is always the guarded, reviewed path.
               Net: the human gets "stop asking me about *this*" without the
               engine ever inventing a broad allow.

               Open question for Steven: is exact-invocation the right default
               scope, or program-with-reviewed-args? Either fits the invariants;
               it changes the pre-filled rule text.

In `:analyze` (the debug viewer, which never executes) the action line stays
DESCRIPTIVE — it never offers to escalate. The live popup is where the actions
become real.

(A 5th INFORMATION action, EXPLAIN — an advisory LLM narration that never
decides — is designed [[project_shannon_analyze_diagnostics]] but is deferred
until after the loop works; it is not on this milestone's critical path.)

==============================================================================
3. ARCHITECTURE                                                 *tool-arch*

3.1 Content blocks (the one real type change)                   *tool-blocks*

`Message.content: String` becomes a list of typed blocks — the minimal set:

>
    enum Block {
        Text(String),
        ToolUse { id: String, name: String, input: serde_json::Value },
        ToolResult { tool_use_id: String, content: String, is_error: bool },
    }
<
Keep `Message::user(text)` / `assistant(text)` as text-only constructors so the
277 existing `.content` sites migrate mechanically (a text message is one
`Block::Text`). `to_editable`/`from_editable` (the JSONL/`$EDITOR` seam) extend
to render blocks — the persisted store already IS the Messages-API shape, so
this stays "the real thing, not an invented syntax." Provider mapping stays in
`backend/anthropic.rs`; core stays neutral (the module doc's stated contract).

3.2 The tool schema                                             *tool-schema*

One tool this milestone: `bash`, `{ command: string }`, described plainly. Lives
core-side as data (a neutral `ToolDef`) so a second frontend/model could reuse
it; `SendRequest` gains `tools: Vec<ToolDef>`; `anthropic.rs` serialises it to
the Messages-API `tools` array.

  As of 11-03 the set is `bash`, `Read`, `Edit`, `Write`, `mark` (and
  `skill` where one is found), one set per session in that order, the cache
  prefix's (`ToolDef::offered`). The file tools are `file-tools.md`'s: judged
  as the shell line of their effect, so this document's gate is theirs too,
  and documented in `:help shannon-files`.

3.3 The turn loop with tool use                                 *tool-loop*

Today: user msg → assistant streams text → `StopReason::EndTurn` → done.

With tools, one "turn" (one user prompt) may span several request/response
round-trips:

    user prompt
      → request (with tools)
      → assistant streams text + a tool_use block
      → StopReason::ToolUse   ← the assistant PAUSES here
      → [GATE: decide → popup → human action]
      → run it (or not) → build a tool_result
      → request again (history + tool_result as a new user message)
      → assistant streams … → StopReason::EndTurn
      → done

This is a loop the Conversation drives, gated in the middle. It reuses the
single-backend-slot discipline ([[feedback_single_backend_slot]]) — a tool call
occupies the one slot, exactly like a generation. Decomposition doctrine holds:
each round-trip is messages; the whole thing is one Episode turn.

3.4 The gate popup                                              *tool-popup*

The pending tool_use with a `bash` command feeds `parse_line` → `decide` →
`permission_view` (the SAME styled producer `:analyze` uses), rendered into a
`wm::OverlayStack` float. The decision stays PENDING while the float is up; the
four actions resolve it. This is the HITL modal M5's viewer was built for.

==============================================================================
4. STAGES (each green-committed; test point named)              *tool-stages*

S1 — Content-block plumbing.                                    *tool-s1*
  `Block` enum; `Message` holds `Vec<Block>`; text constructors preserved;
  `SendRequest.tools`; `anthropic.rs` serialises tools + assembles a `tool_use`
  block from the stream (`content_block_start`/`input_json_delta`); `StreamMsg`
  gains a tool-use variant; `to_editable` round-trips blocks.
  TEST (stub): a canned `tool_use` response parses into a `Block::ToolUse`;
  existing text conversations are byte-for-byte unchanged (regression surface =
  "did adding blocks break the 277 text sites").

S2 — The gate + the four actions (no execution yet).            *tool-s2*
  On a pending `bash` tool_use: run `decide`, open the popup via the SAME
  producer, wire DENY / ALLOW-ONCE / EDIT / ALLOW-ALWAYS (§2). Execution is
  STUBBED (allow → a fake "(ran)" result) so the decision flow is testable
  without running anything. allow-always drives the rule surface + confirm.
  TEST (stub): a stubbed tool_use for `ls` → popup → allow-once → fake result
  committed; for `rm -rf /` → popup shows red/DENY, deny → error tool_result;
  edit re-runs the gate.

S3 — Execution (no sandbox).                                    *tool-s3*
  Replace the stub with a real run: `std::process::Command` (or tokio) in the
  user's cwd, capture stdout+stderr+exit, timeout, and truncate large output for
  the tool_result. Honest trust model documented at the call site.
  TEST: allow-once on a real benign command (`echo hi`) returns real output;
  a slow command hits the timeout; big output truncates.

S4 — The loop closes.                                           *tool-s4*
  After a tool_result, issue the next request and continue the assistant turn;
  handle a model that calls SEVERAL tools; fold tool-call usage into totals.
  TEST (real API, Steven-driven): "list the files here and tell me what this is"
  → Claude calls bash → gate → allow → output returns → Claude summarises. The
  whole thing visible in the transcript, gated in the middle.

S5 — Close-out.                                                 *tool-s5*
  Milestone doc + TODO sync + memory; review pass (this is security-critical);
  confirm the deferred items (§5) are tracked, not forgotten.

==============================================================================
5. DELIBERATELY DEFERRED — tracked, not dropped                 *tool-deferred*

Steven 2026-07-24: fine to defer, as long as tracked. Mirrored in
`docs/Milestones/TODO.md`.

  - **Full sandbox / exec hardening.** The headed-toward end state. This
    milestone runs as the user. Isolation (namespaces/seccomp/container, cwd
    jail, env scrubbing, network policy) is its own milestone.
  - **User-editable policy lists.** "People are tripped up by different things —
    we want a way for them to inform us" (Steven). The mechanism exists
    (`RuleSet::extend_from_lines` / `Rule::parse`); wiring = a `[permissions]`
    Config section + an in-app rule editor. Small, high-value; do after the loop
    so there is a real loop to edit rules against.
  - **Configurable colour scheme.** Colours are hardcoded (`Tone::color`,
    `decision_color`, the carrier palette). M4 theme work; `graph_glyphs` is the
    seed. Purely additive.
  - **`program.rs` simplification.** Steven: "I suspect that `program.rs` can be
    significantly simplified — a lot of repeated patterns." End-of-milestone
    quality pass: the `Arg { canonical, spellings, takes_value, elevates }`
    tables repeat heavily; look for a macro / table-driven shape that keeps the
    per-flag rationale comments. Quality-only (`/shannon-review` territory), after
    the feature works.

==============================================================================
6. OPEN DECISIONS FOR STEVEN                                     *tool-open*

  1. ALLOW-ALWAYS scope (§2): exact-invocation (default proposed) vs
     program-with-reviewed-args. Changes the pre-filled rule text only.
  2. EDIT surface: inline mini-editor vs `$EDITOR` suspend/resume (we have the
     seam). Inline is faster in-flow; `$EDITOR` is more powerful.
  3. Tool result on DENY: a terse "denied by user" vs a structured reason the
     model can act on (e.g. "denied: destructive; propose a scoped alternative").
  4. Stage cut for a first end-to-end demo: is S1–S4 the right first slice, or
     do we want the `[permissions]` wiring pulled in early so you can dogfood
     custom rules against the real loop from day one?

vim:tw=78:ft=help:
