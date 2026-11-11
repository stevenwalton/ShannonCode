# The `claude -p` backend — Claude Code as a provider, Shannon as its gate

Status: DESIGN, 2026-10-03. Decisions are Steven's (answers of 10-03, quoted);
the Claude Code facts are quoted from its docs pages, read directly
(`code.claude.com/docs/en/hooks`, `…/permissions`) — the docs agent had two hook
facts backwards earlier (`[[feedback_verify_doc_agent_claims]]`). What the docs
do not state is marked UNSTATED and is a probe (of the installed binary; the
probe is kept in the development repository).

## Decisions (Steven, 2026-10-03)

1. **Hook first.** No `claude -p` turn ships before Shannon's gate decides every
   tool call Claude Code makes — a PreToolUse hook.
2. **One process per turn**, continued with `--resume <session-id>`. Abort is
   killing the process group, as `exec.rs` does — after an interrupt (review
   21): Claude Code runs each Bash command in a group of its own, which a
   SIGKILL to `claude`'s group never reaches (observed: an aborted turn left
   its command running), and in `-p` SIGINT ends the turn where SIGTERM
   leaves it unfinished (its headless documentation). So `claude` alone gets
   SIGINT, then its group SIGKILL after 3 s (`claude_code::Claude`).
   UNVERIFIED on 2.1.288 that an ended turn ends its shells; background
   (`&`/`run_in_background`) shells are left to Claude Code's own shutdown.
3. **Model ids `claude-code/<model>`** (`claude-code/sonnet`). The code already
   routes a turn's backend BY ITS MODEL NAME (`App::backend_for`, `:model stub`,
   per session), and names this "the seam a second real provider lands on". No
   `:backend` command: `:model claude-code/sonnet` switches, per session.
4. **The user's CLAUDE.md rides along**, said in `shannon setup` and the manual;
   Shannon adds nothing to the system prompt.

Standing: users log in to `claude` themselves; Shannon never offers Claude.ai
login and never reads `claude`'s credentials (`[[project_shannon_auth_policy]]`).
A mid-conversation switch seeds ONE assistant line (the switch probe, 10-03: an
injected `assistant` line is remembered; a `user` line runs as a turn).

## The one structural difference

Shannon's `Backend` is one MODEL REQUEST per call: it streams text and
`StreamMsg::ToolUse`, and Shannon gates, runs, and replies. `claude -p` runs its
OWN tool loop: Claude Code executes Bash/Read/Edit… itself. So a `claude -p` turn
is a whole agent turn, and Shannon's part in a tool call is to DECIDE it (through
the hook) and to RECORD it (from the stream) — never to run it.

## Claude Code facts this rests on (quoted)

- "PreToolUse hooks run before the permission prompt, for every tool except
  `EndConversation`. The hook output can deny the tool call, force a prompt, or
  skip the prompt to let the call proceed." (permissions)
- "PreToolUse hook decisions don't bypass permission rules. Claude Code
  evaluates deny and ask rules regardless of what a PreToolUse hook returns: a
  matching deny rule blocks the call, and a matching ask rule still prompts even
  when the hook returned `"allow"` or `"ask"`." (permissions) — so **Shannon can
  never loosen the user's own Claude Code rules**; it can only add refusals.
- "A hook that exits with code 2 stops the tool call before permission rules are
  evaluated." "Exit 2 means a blocking error … even a JSON `permissionDecision`
  of `"allow"` can't override it." (permissions, hooks)
- `dontAsk`: "Auto-denies every call that would otherwise prompt." (permissions)
- Output: `{"hookSpecificOutput": {"hookEventName": "PreToolUse",
  "permissionDecision": "allow"|"deny"|"ask"|"defer",
  "permissionDecisionReason": "…"}}`; `updatedInput` "directly under
  `hookSpecificOutput` replaces a tool's arguments before it runs." (hooks)
- Input on stdin: `session_id`, `cwd`, `permission_mode`, `hook_event_name`,
  `tool_name`, `tool_input` (Bash: `command`, `description`, `timeout`,
  `run_in_background`), `tool_use_id`. (hooks)
- **FAIL-OPEN EDGES — the design's hard constraints:**
  - "A timed-out `command` … hook doesn't block the tool call. The call
    continues through the normal permission flow, so don't count on a stalled
    hook to act as a gate." (hooks)
  - "Any other exit code doesn't block on its own … it's a non-blocking error
    for most hook events: the action proceeds." (hooks) — a Rust panic exits
    101, so **a panicking hook fails open**.
  - `timeout`: seconds; "Defaults: 600 for `command`"; may be set higher.
- "Hook entries merge across settings levels rather than replacing each
  other" (hooks); a hook passed with `--settings` runs beside the user's own.
- "A hook process inherits the parent environment" (hooks).
- UNSTATED in the docs, MEASURED by the hook probe (Steven ran
  it, 10-03, Claude Code 2.1.288, haiku):
  - `--settings` takes a JSON STRING: every probe's hook fired from one.
  - Hook stdin top keys: `cwd`, `hook_event_name`, `permission_mode`,
    `prompt_id`, `session_id`, `tool_input`, `tool_name`, `tool_use_id`,
    `transcript_path`. `tool_input`: Bash `{command, description}`; Read
    `{file_path}`; Write `{file_path, content}`; Edit `{file_path,
    old_string, new_string, replace_all}`. (Grep and Glob were never called:
    the model used Bash `grep`/`ls`/`find` — their shapes stay unmeasured.)
  - **Exit 2**: the tool does not run; the model's tool_result reads
    `PreToolUse:Bash hook error: [<hook command>]: <stderr>` — it sees the
    reason — and the call is listed in `result.permission_denials`.
  - **`allow` under `dontAsk`, no `--allowedTools`**: the tool RAN (Write
    made its file) — the hook's allow lifts dontAsk's auto-deny.
  - **`ask` with no TTY**: a refusal — the tool did not run, its result is
    the `permissionDecisionReason`, it is a permission denial. Fail-closed.
  - **`allow` + `updatedInput`**: the EDITED command ran.
  - **Two hooks disagree**: deny wins — allow + exit 2, and allow + JSON
    deny, both blocked. So the user's own deny hooks still beat Shannon's
    allow, and Shannon's deny beats any allow hook of theirs.
  - Not run: whether a subagent's tool calls reach the hook (opt-in probe).

## Shape

    Shannon TUI ──spawn per turn──▶ claude -p --input-format stream-json
       ▲   │                           --output-format stream-json --verbose
       │   │                           --include-partial-messages
       │   │                           --permission-mode dontAsk
       │   │                           --model <m> [--resume <sid>]
       │   │                           --settings <hooks: PreToolUse "*" →
       │   │                                       `shannon hook`, timeout T>
       │   └── env SHANNON_HOOK_SOCKET=<private dir>/hook.sock
       │                                   │ for every tool call
       │                                   ▼
       └──── unix socket (0600, 0700 dir) ◀── `shannon hook` (stdin: the call)
              request: the call                 prints allow JSON, or EXITS 2
              reply: allow [+ edited input]     with the reason on stderr
                     | deny + reason

### `shannon hook` — fail closed, by construction
- A subcommand of the same binary, spawned by Claude Code. Reads the call from
  stdin, connects to `$SHANNON_HOOK_SOCKET`, waits for the answer.
- **Every path but an explicit allow exits 2**: no socket, a bad reply, an IO
  error, its own deadline, a panic (`hook::run` decides under
  `catch_unwind`), an allow it cannot write (`hook::emit`). Allow is the one
  JSON it ever prints. What the process cannot catch — an abort, a binary
  deleted or moved under a live session — the hook's shell command does:
  `'<shannon>' hook || exit 2` (review 21).
- Its own deadline sits below the configured `timeout`: it denies before Claude
  Code could time it out (a timed-out hook FAILS OPEN). `timeout` is set high
  (a human may take minutes; proposal: 3600 s, deadline 3540 s). The deadline
  bounds the WHOLE process — a watchdog thread exits 2 at it — not only the
  wait for an answer, whose clock started after stdin closed (review 21).
- **The deadline is said** (10-22, A1.5; Steven: "The `:ls` timer is also
  good. We should implement it"). `StreamMsg::ToolAsk` carries one
  `ToolQuestion`, its `answer_by` the instant `HOOK_DEADLINE` ends, counted
  from Shannon accepting the hook's connection (the watchdog started a
  moment before, reading stdin; the wait to be named counts against it).
  The TUI puts it on its own clock at arrival by what is LEFT of it then
  (`Executor::Provider`'s `answer_by`) — an instant, not a span, since the
  TUI's loop is blocked while `$EDITOR` runs and a span read late gave the
  hook minutes it did not have (review stop 8, A 6). `:ls` counts it down
  (`needs you · 52m left`), and a withdrawal within a second of it
  (`DEADLINE_SLACK`) says the deadline refused the call. Setup's Claude
  Code page states the limit.

### The gate side — one door, not a copy
A hook request becomes a tracked tool call like an API `ToolUse`, and walks the
same doors (`arrive_tool_call` → `decide_tool_call` → `tool_prompt` → resolve).
What differs is WHO RUNS an approved call: today `run_tool_call` executes it; for
a hook call the "run" is replying allow (and the edit `e` makes replies allow
with `updatedInput`), and `finish_tool_call(…, error)` replies deny. A call
records its executor — Shannon, or a hook reply channel — so the doors stay one.

As built (2026-10-03, `App::admit_provider_call`, `Executor::Provider`):
- **Not alias-resolved.** `gate_tool_call` resolves `{{a:…}}` before judging;
  a provider call skips it, because Claude Code runs the input as given and
  the judged line must be the run line. (Secret handles therefore do not
  resolve in a Claude Code turn — they run as literal text.)
- **Never pushed as a pending `tool_use`.** Claude Code runs the call; the
  round is recorded from `StreamMsg::ToolRecord`, so a question can never park
  a turn waiting on a result Shannon will not produce.
- **Answered by question, not by call id** (10-21; review of `buffers.md`,
  10-15, Reviewer B F2). The hook question's `tool_use_id` is the asker's to
  say, and any process holding the socket can ask (Claude Code's tool shells
  inherit its path). So each question gets a serial (`AskId`), the gate's
  map is keyed by it, `ToolAsk`/`ToolWithdrawn` carry it, and
  `Backend::answer_tool` takes it: an answer reaches exactly the connection
  that asked about that input, and a turn's end or a hook's disconnect
  removes only its own questions. The call's record keeps its serial
  (`Executor::Provider`'s `ask`); a withdrawal under another serial leaves
  the decision up. Two questions claiming one call are both refused, the
  open one taken off the pane — Claude Code asks once per call, and which is
  the impostor cannot be told — whichever asked second, a `:child`'s too;
  but the question that HOLDS a child stays open (an answer would let it
  carry on; review stop 7, E 2–3). One it asks after, on its own gate — a
  parallel call, a subagent's — is refused at once, said: a held job never
  resumes, so nothing could answer it, and left open it waited out the
  hook's own deadline (59 minutes) and was refused then anyway (Steven,
  10-27, review stop 12 Q4: "we can start with more restricted children
  that refuse calls"). The cross-generation checks are defence in depth:
  property 4 (below) keeps a turn from asking about another's call
  (review stop 12, T4 1). A failed claim answers nobody through the
  round: a finished call's result outlives its record (E 4). A turn that
  has ended takes no more questions (`TurnQuestions`, E 6), and a call with
  no record is never decided or run (E 1).
- **Only a call the turn's output named** (B's property 4; BUILT 10-22).
  The turn's loop names each call to the gate as its line is taken in
  (`take_line`: every `assistant` tool_use, a subagent's too, and a
  top-level `content_block_start`, small and written before the input), in
  an insert-only set per turn (a `watch`, `Named`). A question waits up to
  `NAMED_WITHIN` (3 s) for its id: nothing documents the order of a call's
  line and its hook, so the gate assumes none, and a line bigger than the
  pipe arrives in pieces while the loop's `select!` reads whichever is
  ready. Unnamed in time, it is refused with a
  reason Claude Code's model can act on, sent to the TUI as
  `StreamMsg::ToolUnnamed` and said, and never put to the pane.
  **A background agent's calls are refused unless named** (nothing
  documents that its calls reach the output) — Steven, 10-22: kept that way, with
  `[claude_code] background_tasks = false`
  (`CLAUDE_CODE_DISABLE_BACKGROUND_TASKS`, which takes Bash's
  `run_in_background` too) "to close the gap if they want. We can
  experiment with it too." It stops a made-up id, not a real one read from
  the transcript: the both-refused rule above is what bounds that.
- **"Who decides" is one answer, read in two places**: `Executor::judge` →
  `Judge::{Rules, Allow, Ask}` decides a call on arrival (`decide_tool_call`)
  and the unattended child's bar; the pane built for it carries the same
  answer as a `ProviderCall` (`ToolPrompt::for_provider`), which
  `revisit_open_decision` reads — so an `:allow WebFetch *` (the line
  `WebFetch …` parses as a command named `WebFetch`) cannot answer an
  always-ask call. Its report (`opaque_report`) shows the line as written,
  why it asks, and no `A` rule.
- **Edits map back for `Bash` only** (`claude_code::edited_input`). `e` on any
  other tool is refused before `$EDITOR`, with the reason on screen.
- **Jobs**: an attended `:Send` gets the pane; an unattended `:child` allows
  what is `safe_unattended` and otherwise HOLDS, leaving the hook's question
  open — Claude Code waits, stopped, until `:child --stop` (whose teardown
  refuses the question). Work offered no tools (a distillation, a
  continuation) is answered deny at once.
- **Records are scrubbed** (input and output) before the screen and the store
  — see the open question below about what Claude Code's own model saw. So is
  the line on every record of a decision (allowed, denied, withdrawn): Claude
  Code's model writes lines from output it read unredacted. The decision pane
  shows the line as it will run.
- **Calls run where Shannon started, or not at all** (review 21). The gate
  judges a line from Shannon's start, and Claude Code keeps a `cd` between
  Bash calls — after `cd src`, `cat prod_config.txt` read as a top-level file:
  a protected path, allowed. `claude` runs with
  `CLAUDE_BASH_MAINTAIN_PROJECT_WORKING_DIR=1`, and a call whose hook `cwd` is
  anywhere else (or unsaid) is refused, visibly (`app::runs_elsewhere`).
  Judging in the reported `cwd` instead would need the analyzer to split
  "where a line begins" from "where rules are anchored" at every parse site.
- **Shannon's key is withheld** from `claude` and so from its Bash
  (`claude_command`, `config::KEY_VARS`): Claude Code would bill an
  `ANTHROPIC_API_KEY` ahead of the login setup names, and `echo
  $ANTHROPIC_API_KEY` is a line the gate allows.
- **The gate is shown to be in the loop** (review 21). A `"disableAllHooks":
  true` in ANY of Claude Code's settings files — a cloned repo's
  `.claude/settings.json` included — turns off the `--settings` hook too
  (its hooks documentation: the key is read after settings precedence, and
  `--settings` overrides only the keys it names). A call's
  question reaches Shannon before its result can exist, so a SUCCESSFUL result
  for an id never asked stops the turn and says why (`Relayed::Ungated`).
  Errors are not counted (Claude Code answers some calls with one before any
  hook runs — observed 11-08: a call whose input fails Claude Code's schema
  is answered with an `InputValidationError` before a PreToolUse hook that
  would have refused it), so one ungated call that fails slips past. See
  open question 8.
- **`--resume` is keyed by the history**, not the episode: `claude_sessions`
  maps a digest of the transcript as a finished turn left it to that turn's
  session. Any other history — a branch, an edit, a turn another backend
  served — seeds a fresh session from the transcript. Only a FINISHED
  transcript turn files (a distillation's session saw text the transcript does
  not hold; a failed turn's is whatever Claude Code kept). Not persisted.
- **Routing is one rule**: `app::route` decides the backend for a model id and
  `app::model_answering` reports it, so with no API key a `claude-code/…` session is
  not shown as the stub (it was, for one build — two rules for one fact).

Mapping non-Bash tools onto the rules (the rules read shell lines) — as built
in `claude_code::reading`:
- `Bash` → its `command`, as is.
- `Read {file_path}` → `cat '<file_path>'`; `Grep {pattern, path}` of a FILE →
  `grep -e '<pattern>' '<path>'`, of a tree → `grep -r -e …` (10-04: Steven,
  "for now drop the `-r`" where the path is a file); `Glob {path}` → `find
  '<path or .>'` — reads, so the credential gate and path rules see them.
  `grep -r` is unmodelled ON PURPOSE (`GUARDED_MODES`: a recursive read prints
  files no argument names), so **a tree Grep asks** — conservative; see the
  open questions.
- `Write`/`NotebookEdit {file_path}` → a redirect write to the path (`cat >
  '<file_path>'`), so `:write`-level and path rules apply; `Edit`/`MultiEdit`
  → a read and a write (`cat '<p>' > '<p>'`, 11-02, F2a: an edit's errors say
  what the file holds), as Shannon's own file tools are read
  (`files::reading`).
- **Through a link** (11-03, F2c): a file tool's path, and a Grep's file, are
  judged where they land as well (`files::lands`) — `cat '<p>'; cat '<t>'`,
  the stricter winning — so a link in the project to `~/.ssh/config` asks as
  a read of it does. The system's root links (`/tmp` → `/private/tmp`) keep
  their spelling. Claude Code opens the file itself, so nothing re-checks the
  link at the run, as Shannon does for its own.
- `WebFetch`/`WebSearch`, `Task` (a subagent), MCP tools, anything unknown →
  ASK, naming the tool. CONFIRMED (Steven, 10-03): "ask is good … We will
  configure web, subagents, and MCP later … I do want to have a config as well
  as auto allow web. But default ask is a good default." So `[claude_code]
  web = "ask" | "allow"` (default `"ask"`) now; subagents and MCP ask until
  they are configured. (Allowing web lets the model send data in a URL — the
  manual says so where the knob is.)
- Harmless internal tools (`TodoWrite`, …) → allow.

### The stream side
Per turn, events → `StreamMsg`: `stream_event` text deltas → `Chunk`;
`assistant` tool_use + `user` tool_result → a new `StreamMsg::ToolRecord`
(display + store, never run); each top-level `message_start` →
`StreamMsg::NextRequest` + `Usage` (its prompt) and `message_delta` → `Usage`
(its output), so every tool round inside the turn is its own request and
the last one's prompt is the window (10-15: the `result`'s usage is the
total of them — and read as one request it put a 1M session at `ctx 100%`,
measured); `result` → `Stopped` + `Complete` (or `Error` on
`is_error`), and `Usage` only when the stream metered no request;
`system/compact_boundary` → `StreamMsg::ProviderCompacted` (said on the status
line: its model works from a summary from there; `[claude_code] auto_compact =
false` → `DISABLE_AUTO_COMPACT`);
`system/init` → `StreamMsg::ProviderSession`, filed by
history for `--resume` (above). Events with a `parent_tool_use_id` are a
subagent's — parsed with their parent, recorded flat for now (open question 4).

## Open questions (raised 2026-10-03, for Steven)
1. **The redaction boundary is not Shannon's here.** On the API path an
   approved command's output is redacted before the model sees it. On this
   path Claude Code runs the tool and its model sees the output AS IS — a
   PreToolUse hook decides whether a call runs, not what its output says.
   Shannon scrubs only its own copies (screen, store, later handoffs). So the
   gate is the only layer: an approved `cat .env` sends `.env` to the
   provider. Options: (a) document it at the knob and in the pane; (b) raise
   a credential-path ASK to DENY for provider calls by default; (c) probe
   whether a PostToolUse hook can rewrite a built-in tool's output. The
   model's reply text can repeat what it read, and that streams to the screen
   and store unscrubbed today.
   DECIDED (Steven, 10-04): (a) + (b). "Documentation is important
   regardless, we don't want surprises. But let's not scrub, for now. Docket
   the option." BUILT: `[claude_code] path_policy`, default `"deny"`, in
   place of `[secrets] path_policy` for these calls — every judging site
   asks `App::rules_for_call`; the pane says who runs the call and names the
   knob (`PROVIDER_NOTE`); `DEFAULT_TOML` and `:help shannon-config-tree`
   say it. DOCKETED: (c), and scrubbing Claude Code's reply text.
   CORRECTION (10-04): (c) is not an open question — Steven PROBED it on
   10-02 (`TODO.md`, the `claude -p` masking entries): a PostToolUse hook's
   `updatedToolOutput` in the tool's own OBJECT shape replaced the value for
   Bash and Read, and the model saw only the handle (the string form failed).
   Not yet probed: subagents, tools beyond Bash/Read, and a hook that crashes
   (fail-open, measured for the string form). So scrubbing on this path is a
   build, not research — still docketed, per Steven.
2. **Every Grep asks** (`grep -r` is guarded). Correct, and probably noisy.
   A cheaper safe reading needs to know whether `path` is a file (no `-r`) or
   whether the tree holds a credential path — a filesystem question the
   mapping does not ask yet. Claude Code's Grep skips gitignored files (its
   tools reference); whether it reaches hidden ones it does not say, so a
   tree Grep is read as reaching them. Steven asked why (10-04) — answered.
   DECIDED (10-04):
   a Grep of a FILE drops `-r` and is judged as the read it is; a tree keeps
   `-r` and asks. BUILT: `reading` takes an `is_file` check
   (`App::read_provider_call` asks the filesystem). STILL OPEN, docketed: a
   quieter tree Grep — constrain it (`updatedInput` excluding `[secrets]`
   paths, needs a probe) or teach the rules recursion (`ignore` crate or
   `rg --files`).
3. **The pane's `e` is offered for tools that refuse it** (any non-`Bash`
   call). DECIDED (Steven, 10-04): strike it through and keep refusing —
   "it could be available but currently isn't … encourages people to try the
   full featured version". BUILT: `ProviderCall` on the pane drives both the
   struck legend (`action_line`, `CROSSED_OUT`) and the key's refusal, which
   says an API model edits every call. Still open: a tool-aware pane that
   shows what a `Write` writes.
4. **A subagent's records are recorded flat**, as if the top-level agent made
   them (`parent` is parsed and dropped). Whether its calls reach the hook at
   all is the unrun probe above.
5. **`Bash`'s `dangerouslyDisableSandbox` input asks every time** (11-08;
   until then it passed unread, and a rule allowing the command let it leave
   the sandbox). Read as `Judge::Unsandboxed`: the pane analyses the command
   as ever, heads it "outside Claude Code's sandbox", and offers no rule; a
   rule added while it is open does not answer it; a deny still denies; a
   `:child` holds on it. Any value but `false` counts (fails closed).
6. **Switching back to the API** after a Claude Code turn sends `tool_use`
   blocks named `Bash`/`Read` that the API request never offered — unverified
   whether the API accepts them.
7. **Claude Code reads the repo before any tool runs** (found 10-04, when a
   first turn quoted `git status` with no tool in the record). Its context
   carries a git status snapshot — the current branch, the main branch,
   `git status` output and recent commits (its settings reference, on
   `includeGitInstructions`) — read by Claude Code itself at start: no tool
   call, so no hook, so Shannon's gate never sees it. Off by
   `CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS=1` or `includeGitInstructions:
   false` in the `--settings` Shannon passes — which also drops Claude
   Code's commit and PR instructions. Steven's call; the manual should say
   it either way.
8. **A repository can run code, or remove the gate, before Shannon sees a
   call** (review 21, Reviewer B). A `-p` session shows no workspace trust
   dialog and no per-server approval: a project's `.claude/settings.json`
   hooks run (`SessionStart` before the first prompt) and its `.mcp.json`
   servers connect, in a folder never trusted (Claude Code's headless
   documentation), and its `disableAllHooks` drops Shannon's hook. The
   tripwire (above) catches the dropped hook on the first call that works;
   it cannot catch a `SessionStart` hook or an MCP server starting. The
   structural fix is `--setting-sources user` plus `--strict-mcp-config` —
   at a price, inferred: the project's CLAUDE.md goes with the `project`
   source (as its documentation has `CLAUDE.local.md` go with `local`), so
   it would stop loading (Decision 4), with the project's skills, its
   Claude Code permission rules, and the user's own MCP servers. Steven's call. Options: (a) keep as is, documented —
   the same exposure as running `claude -p` in that directory oneself; (b)
   both flags, accepting the loss; (c) `--strict-mcp-config` alone (MCP only,
   CLAUDE.md kept); (d) (b) only when the directory is not one the user has
   trusted in Claude Code (needs reading Claude Code's trust state — a probe).
   DECIDED (Steven, 10-04): none of those — Shannon asks its OWN trust
   question per directory, as Claude Code does when run interactively: once,
   remembered; a trusted directory covers its subdirectories; asked for any
   directory. BUILT: `crate::trust` (kept in `trusted_dirs` in the state
   directory, 0600 — Shannon's own state, which the gate protects, so an
   agent cannot write itself a trust), `app::trust_flow`. The question is
   asked where a prompt is sent (`submit_prompt`, before the prompt is
   taken: `n` leaves it there); every other start — `:continue`, a job, a
   distillation, a queued prompt — is refused at the one door every request
   leaves through (`spawn_tagged`), saying how to trust. `:trust [dir]`,
   `:untrust [dir]`.

## Claude Code conversations in `:graph` (decided 2026-10-04, built)

Steven: "Any way we can include claude code conversations in `:graph`?" —
first use, recovering the very session that built this backend.

Facts (Claude Code's `.claude` directory reference, and its files measured
by structure, 10-04): each conversation is
`~/.claude/projects/<dir>/<session-id>.jsonl`, `<dir>` the start directory
with every character other than a letter or digit as `-` (documented), cut
past 200 characters with a hash appended (the Agent SDK's sessions docs; the
hash is not given). A path those rules do not settle — not ASCII, or past
the cut — Shannon names no directory for. Entries carry `uuid` and
`parentUuid` — a tree; compaction nulls `parentUuid` and keeps
`logicalParentUuid`; subagent runs are `isSidechain`; `summary` and
custom-title entries name it (measured). The format is Claude Code's
INTERNAL format, not an API: read what is known, skip and say what is not.

DECIDED (Steven, 10-04):
- **Import on open.** `:graph` lists this directory's Claude Code
  conversations (title, date, size — read without loading bodies). Enter copies the path Claude Code would resume into a
  Shannon session — user text, replies, tool rounds; thinking and system
  reminders skipped; tool output SCRUBBED on the way in (the transcripts
  hold it raw) — that records its Claude Code session id and directory.
  Everything Shannon does to sessions then works on it; it is a snapshot.
- **Fork by default.** `[claude_code] continue = "fork" | "resume"`;
  `:graph` can override per open (Enter forks; another key resumes in place,
  confirmed — in place shares the file with any live Claude Code). A turn
  then runs `claude --resume <id> [--fork-session]` in the conversation's
  own directory.
- **This directory, plus a path**: a command brings one in by id or file
  from elsewhere (this session started in the parent directory).
- Shannon reads `~/.claude/projects` only on the user's command, never
  Claude Code's credentials; the agent's own gate already protects
  `~/.claude` (shipped seed).
  CHANGED (review stop 5, L1, HIGH; Steven, 10-19): Claude Code's home is
  `$CLAUDE_CONFIG_DIR` when set, and every guard named `~/.claude` — so
  under the variable its credentials, `.claude.json`, transcripts and
  hook-running `settings.json` were an ordinary path. Launch resolves the
  home ONCE (`App::set_claude_home`), and the seed's `{claude-home}` entry
  guards it and its real path (`secrets::CLAUDE_HOME`), with its carve-outs
  placed the same way and a write asked even under `path_policy = "allow"`;
  the listing and the knowledge roots read the same value.

REVISED (Steven, 10-04, after the probe and review 21's H1): **continued as a
Shannon session**, not `--resume`. The gate judges a call from Shannon's own
start directory and refuses one Claude Code reports running elsewhere
(`runs_elsewhere`); a conversation from another directory (this one is the
parent directory's) resumed in place would refuse every call. Resuming in
Claude Code needs a per-session working directory — the gate's start, the
trust check and `claude`'s spawn all per session — DOCKETED. And **the
resume path by default, the rest on ask** (`:import --all`).

Measured (a structure-only probe, 2.1.288, one session's file:
269 MB, 100k lines, longest 3 MB): chain entries are `user`, `assistant`,
`attachment`, `system`; the rest is metadata (`ai-title`, `last-prompt`
with `leafUuid`, `custom-title`, modes, file-history). Each assistant
content block is its own entry (10,434 `message.id`s shared); 45
compactions, each a `system/compact_boundary` with no `parentUuid` and a
`logicalParentUuid`; no parent points outside the file; no sidechain entry
in this file (2.1.288 writes subagents elsewhere).

Measured again (10-17, six of Steven's session files, structure only: keys,
line numbers, booleans — `tools`-less scratch scripts, nothing kept). In
newer files a compaction KEEPS its last messages: the boundary carries
`compactMetadata.preservedSegment{headUuid, anchorUuid, tailUuid}` and
`preservedMessages{anchorUuid, uuids, allUuids}`; the kept messages stay
where they were, before the boundary; and `logicalParentUuid` names the
TAIL — the entry after the summary — not the last entry before. Followed,
that loops back onto the walk: one file read as one compaction of 29,
another as one of 47. The history before a compaction ends where the file
stood when it compacted (its newest non-subagent entry); so walked, every
compaction is crossed (46/47, 29/29, 10/10), to the first prompt or to the
compaction a resume copied in. `claude --resume` writes a new file that
begins with the source's latest compaction copied (uuids kept) and nothing
before it; the SOURCE gains `{"type": "continued-in", "continuedInSessionId",
"sessionId", "timestamp"}` (one file measured holds six: five files, one
never written). Nothing in the new file points back.

Measured once more (10-17, the same look, ids/line numbers/booleans): the
first message a compaction keeps (`preservedSegment.headUuid`) is an
ASSISTANT message in every boundary seen — the summary is the user's turn,
so the two alternate — before the boundary in the file it compacted; its
tail is either before it too (older boundaries) or an attachment right after
the summary. The file a resume writes holds them in the order the model
gets them: boundary, summary, the kept messages, the tail, on one parent
chain. So `Scope::Resume` splices the kept stretch after the summary
(Steven, 10-17: "The cost is worth it") and reads the original as the
resume's file reads; the question after a summary then opens its own turn
where `Lineage` joins it to the summary's (`ImportedTurn::joined`, which
`:graph`'s placement consults).

BUILT (10-04): `shannon_core::claude_import` — `list` (titles from a file's
tail: custom > generated > summary > last prompt), `read(path, Scope)`: two
streamed passes (the chain without bodies, then the entries on it by
offset), the walk back from the newest non-subagent entry, parallel calls'
results recovered off the walk's one branch, blocks merged
by `message.id`, prompts merged until a reply, every call answered (a
missing result says so), results cut at the exec output cap, and what was
not brought in counted (`Skipped`). `:import [--all] <id|file>`
(`app::import_flow`): replayed through the live-turn calls, scrubbed, each
turn sealed as `claude-code/<model>`, adopted as a lazy session like a
clone; the notice says what was left out. Turns carry the import's time.

BUILT (10-04, after Steven: "I don't see our session in `:graph`… make
sure we don't let `shannon` delete `claude code` sessions. Only shannon
sessions"; decided: own section, Enter imports; this directory's, every
directory's on a key): `GraphView`'s Claude Code section — rows AFTER the
tree, in the cursor's and a Visual range's reach, carrying no `Node`, so no
verb acting on nodes can reach a Claude Code file by construction. `<Enter>`
imports; `d` refuses with why; a range over them deletes Shannon's rows and
the question says the rest stay; they cannot be marked; `a` toggles every
directory's (netrw's `a`). Listed on opening `:graph` and on `a` only — a
rebuild carries the listing, since each file's tail is read for its title.
The inspector describes a row. Shannon writes nothing under
`~/.claude/projects`: the import reads, the listing reads.

BUILT (10-18, Steven 10-17: ASK before reading them — "So they don't think
we're doing anything sneaky here. I think a side benefit is that the user
will expect that action to take time"; the shape agreed 10-18): Shannon
reads Claude Code's conversations only where the user said it may, asked
once a directory — `y` this run, `a` always, `n` not (then the heading says
`not read — l asks`, and `l` on it asks again; the next `:graph` does not).
`a` (every directory's) is its own question; leave for Claude Code's
projects directory holds each project's under it — and ONLY that one: any
other directory's leave is its own (an import asks about its file's
directory, whichever, and `a` to one above the projects directory held
every project for good — review stop 5, A 1). Every directory recorded or
asked about is folded by name first (`claude_sessions::normal`: absolute,
no `.` or `..`), and an import reads the path it asked about (A 2, B 6,
C 1). The record is appended one line per write (B 8). `always` is one line in
Shannon's Claude Code record (`claude_sessions`, `{read: dir}`, owner-only).
Three doors read, and each checks (`App::may_read_claude`): the listing
(`App::claude_list`, every caller of `claude_import::list`) and a reading
(`App::claude_reading`, the lineage, off the UI thread) refuse; an import
(`App::import_conversation`, `:import` and `<CR>` alike — Steven, 10-18:
"I'd rather have a gate for reading from Claude Code sessions") asks: of
this directory's Claude Code folder, leave for it (`y` or `a` imports); of
a file anywhere else, that file alone (`ConfirmAction::ImportOne`, `y/N`,
nothing recorded, nothing else read — Steven, 10-19: "Let's only import
named imports when they are outside the working directory"). Before any of them,
only names are looked at: whether this directory has a Claude Code
directory (`project_dir`, which lists names for a long one), and
`:import <id>`'s search by file name — so the section,
the match for older imports and every reading under them read nothing
until then. A listing refused is not "none there": the match for older
imports returns rather than check every candidate unfound. `:unread [all]`
takes leave back (Steven, 10-18: "I think we should add a command"): as
`n` for this run, `always` taken back for good with one more line
(`{unread: dir}`, the record only appended to), what was read there let
go (a reading landing after is dropped; an inspector's painted page too),
and every `:graph` relisted (`relist_claude_sections`, the one helper `y`
uses too). Leave for every directory's still holds this one's: `:unread`
says so, `all` takes it. It re-reads the record first, so an `always`
another running Shannon said is taken back too (review stop 5, E1 F1);
the other way round, an `:unread` elsewhere leaves this run reading until
it ends.

BUILT (10-18, Steven 10-17: Claude Code's own resumes as lineage — "Seeing
*where* we had picked up in Shannon compared to where that conversation
currently is"; 10-18: no new eager reads): a `claude --resume` leaves a
`continued-in` record (`continuedInSessionId`) in the file it moved on
from; the new file names nothing back. The listing reads them from the
same tail as the title (`Found::resumed_in`; ids only, compared with
listed ones, never made a path; one written further back than the 256 KiB
tail is not seen). The new file copies the source's chain from the
compaction latest then with the entries' ids kept, so `:graph` hangs it
under the source at the latest source turn it holds — placed by identity,
as a session is since stop 4 A 1 — the shared turns drawn once, as the
source's, and a session or branch at one of them hung at the source's
turn (`graph_view::Resumes`, `Placed`). Unread (either), it hangs from the
source's row. One source each, the first listed to name it; a name that
would close a cycle is passed over. Each lineage is listed by its newest
conversation.

CHANGED (10-18, Steven: "I'm going to change a earlier decision though
because of an optimization issue" — the section before its readings came
"looks pretty nice … especially how the line connects the conversations",
after, 900 turns opened at once, "shows too much all at once"): a
conversation's TURNS are shown only once the user opens them. Until then
what hangs from any of its turns — sessions, resumes — hangs from its row,
in its turns' order (`Layout::conversation`), the row open to it by default
and ending in ⋯. `l` opens a level (what hangs, then the turns; one with
nothing hanging, its turns at once), `h` closes one (`GraphView::deeper`,
`show_turns`); which conversations are open is kept by id through every
rebuild and refresh (`Held::opened`). A reading landing moves no
conversation and no session — only a placed session's imported turns go to
the conversation, drawn once its turns are. Given up: `/` reaches a
conversation's turns only once they are open (read-but-closed ones were
searched before). Not chosen (offered): a "spine" — only the turns
something hangs at, the rest folded — which says where each parted but
moves rows when a reading lands.

## Build order (each step committed, tested, mutation-checked)
1. **Probe** (Steven runs the hook probe): the UNSTATED items.
2. `shannon hook` + the socket protocol, fail-closed — unit-tested with a fake
   socket server: every failure exits 2, a panic exits 2, the deadline exits 2.
3. The stream parser (`backend/claude_code.rs`) — pure, tested against the
   recorded fidelity events (`fidelity-events.jsonl`, 10-03).
4. The backend: spawn per turn, `--resume`, abort = kill the group, model
   routing `claude-code/…` in `backend_for`; tested with a fake `claude`.
5. The TUI: hook calls through the gate's doors (executor on the call), tool
   records displayed and stored, the seeding rule on a provider switch.
6. `shannon setup` + manual (`:help shannon-claude-code`): log in to `claude`;
   slower than the API; your CLAUDE.md rides along. BUILT (10-04), to
   Steven's spec: a choice screen (API "fully featured", Claude Code
   "limited — you get the TUI features, but you don't have as much control",
   demo "doesn't connect to an agent"), then what the choice needs — a key
   saved owner-only beside the config (`api_key`, read at launch after
   `ANTHROPIC_API_KEY` and `SHANNON_API_KEY_FILE`), or a `claude auth status`
   check (exit code + `loggedIn`/`authMethod` only; never Claude Code's
   credentials). Esc leaves any screen. Opens on `shannon setup`, `:setup`,
   a first launch (no key, no config file), and a `:model` switch to a model
   that is not set up — which then only makes that model work, while a menu
   choice is written to `[tui] model` (`toml_edit`, that key alone) and
   switches the session. A saved key answers at once (`App::make_api`).
7. Steven drives it (`smoke.py` cannot reach `claude`).
