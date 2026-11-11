# File tools for Shannon's own backend

STATUS: **F1, F2a, F2b BUILT 11-02; F2c, F2d, F3, S21-Q27 11-03** — the
three tools offered and run on Shannon's own backend, judged where a link
lands them, driven offline by the stub, held to read-first, spending a
secret as `bash` does; documented (F4: `:help shannon-files`). Claude
Code's read-first half waits on a live probe (TODO). (S21-Q24.)
Steven, 11-02: "I think we should have our own. Can you present your design
before implementing? Let's make sure we are doing things efficiently." My
calls are marked as calls. Tags: *verified* (read in Shannon's code or a
public document, or measured this session), *inferred*, *unknown*. The experiment it led to, an action done in
a fork whose result alone comes back, is `aside.md`.

## 1. The problem

Shannon's API backend offers `bash` and `mark`, and `skill` where one is
found (`request.rs:122`). On it a file is read with `cat`, written with a
heredoc or `tee`, edited with `sed`. That costs three things:

- **Tokens and turns.** A `cat` has no window; a failed edit is a round
  trip. In images 44–52 the first `sed -i` failed on macOS (BSD `sed`
  wants a suffix argument), and the model retried.
- **Verification.** The decision pane shows a heredoc as a wall of
  command, never as a change. The Edit and Write records, and the diffs
  (D1–D4), fire only for Claude Code's Edit and Write: on this backend
  they have nothing to show.
- **Policy.** Read-before-write (S21-Q20) has nothing to check: a shell
  write reads nothing.

## 2. What exists to reuse (verified 11-02)

- **The gate already judges file tools.** It judges Claude Code's `Read`
  as `cat <path>`, and its `Write`/`Edit`/`MultiEdit` as `cat > <path>`
  (`claude_code.rs:553–555`, `reading`). So Q7's inside/outside defaults,
  `[secrets] paths` and your rules all apply, and `:analyze` agrees. A
  Shannon file tool judged through the same function is judged the way
  Claude Code's are, by construction.
- **The pane shows a write as its change.** `Change::Content` or
  `Change::Edits` (`claude_code.rs:620–649`); `e` edits the new text
  (`edit_text`, `edited_input`). Records say `Read a.rs, from line 40,
  60 lines` or `Edit a.rs` (`tool_record.rs:404–405, 572`). All of these
  are keyed by Claude Code's tool names and input keys.
- **`exec::Executor`** (`exec.rs:469`) is the seam tests replace, so that
  `cargo test` cannot cause a real effect. `ExecOutcome::report` carries
  the redaction note to the model, and `secrets::redact` scrubs output
  (`exec.rs:958`).
- **D1 reads a `Change`** through `Record::change`. S21-Q19 keeps an
  edit's place per call, as positions only, never file text (`diffs.md`
  §7).

## 3. The proposal

### 3.1 Three tools: Claude Code's shapes, Shannon's execution and policy

    Read   { file_path, offset?, limit? }
    Edit   { file_path, old_string, new_string, replace_all? }
    Write  { file_path, content }

The names and the **required** keys are Claude Code's (S21-Q25, Steven:
"make our own but it would take the same required arguments as Claude
Code's … if we want to add other parameters to be able to experiment … we
should be able to"). Optional keys are ours to add, as an experiment needs
them: a model that knows `Edit(a, b, c)` has nothing to learn from `Edit(a,
b, c, d = 1)`. Shannon defines them as ordinary `ToolDef`s, judges them, runs
them in-process and records them. Why that spelling:

- **One vocabulary for both backends.** Records, the pane's change, the
  diffs and read-before-write are written once. Who ran a call is already
  `Executor::{Shannon, Provider}` on the call (`app.rs:2621`). A new
  spelling would be a second vocabulary for every one of those readers to
  learn, and "the second spelling" is a class this codebase keeps paying
  for.
- **Familiarity** (*inferred, unverifiable*): Claude models are used
  heavily through Claude Code, so these schemas are likely the file
  schemas they have seen most. Anthropic documents no such thing. What it
  does document is that the text editor tool's schema is Anthropic-defined
  (§5).
- **Provider-neutral.** A plain `{name, description, input_schema}`
  needs no Anthropic-only `type`, so a second provider takes the same
  tools as they are.

Paths may be absolute, or relative to the session's start directory.
Claude Code requires absolute; Shannon accepts both and says so in the
description.

### 3.2 Read

- **Output** is `N<tab>line`, as Claude Code's Read results show it
  (observed 11-08): the cheapest numbering that `offset` can still be
  aimed with.
- **Window:** from `offset` (1-based), `limit` lines. The default is the
  whole file up to a cap of 2,000 lines and 256 KiB — Shannon's starting
  point, and a setting (Claude Code's tools reference documents only its
  own Read refusing a whole file over 256 KB, read in pages). Past the cap
  the result says so: `[lines
  1–2000 of 5412; read on with offset 2001]`. A line over 2,000 chars is
  cut and marked.
- **UTF-8 text only.** A file that isn't, or a directory, is an error
  result that names the alternative (`ls`, `bash`). Images, PDFs and
  notebooks are not in v1 (docketed).
- **Redacted** as `bash` output is: the same function, and the same note
  to the model.
- **An unchanged re-read** is answered with a one-line note, not the text (§3.6).

### 3.3 Edit

- **`old_string` must occur once**, or every time with `replace_all`.
  Otherwise the result is an error, worded as Claude Code words it ("not
  found"; "found 3 matches … provide more context" — observed 11-08),
  wording the model already knows.
- **Exact match.** A match that takes curly quotes as straight is
  docketed, not v1.
- **Line endings:** a CRLF file is matched as LF and written back CRLF.
  Otherwise the `\r` that Read never shows would make every match fail.
- **An Edit reads before it writes, so it is judged as both:** `cat
  <path>` and `cat > <path>`, the stricter winning. Its errors say
  something about the file ("found 3 matches"), and today's reading of
  Claude Code's Edit as a write only misses that. That gap exists for
  Claude Code's backend today; this fixes it for both.
- **Written in place:** open, write over, cut to length. That is what `cat > path`
  does, and `cat > path` is what the gate judged. Temp-and-rename would
  replace a symlink with a file, drop hard links and reset mode and owner:
  an act the gate never saw. A new file takes the umask, like a shell
  redirect. The owner-only rule is for Shannon's own files, not the
  user's project (my call).
  - *Since review stop 16 (E 3, 5, 6, 7, 10; Steven, 11-04):*
    - **A file with more than one name asks.** Written in place, a hard
      link changes under every name, and the others can be outside the
      project: a package store's file under `.venv`, a dotfile linked in.
      The line says the reach, `cat > $(find / -samefile 'P')`, a word no
      rule can place (`files::reach`, both backends). The pane says the
      file has N names. A Read of one is judged as before.
    - **Opened as judged.** Never through a link at the last step
      (`O_NOFOLLOW`): one planted since the judgment is refused. A device, a
      pipe or a socket is refused at the judgment, before any open.
    - **Written over, then cut.** A write that fails puts the old text back
      where it can, and says whether it could. It was cut first, so a full
      disk left the file empty.
    - **A new file a held secret is written into is made 0600.**
- **The result is one sentence:** `Updated a.rs: lines 40–44 (were
  40–42).` A note follows when the user's `e` changed the text, as Claude
  Code's result says the user changed it. There is no snippet; the model
  wrote the text.

### 3.4 Write

- **Creates or overwrites.** Over an existing file it is held to
  read-first (§3.6), as Edit is.
- **The result:** `Created a.rs (120 lines).` or `Wrote a.rs (120 lines,
  was 98).`

### 3.5 The gate, the pane, the record

- **One reading.** `reading` moves out of `claude_code.rs` into a neutral
  module, and its doc names its consumers: the Claude Code hook, Shannon's
  own calls, `:analyze`. The `Change` and `edit_text` helpers move with
  it.
- **Symlinks** (*verified 11-02*): the engine places a path by its
  spelling. "Purely lexical — we do not touch the filesystem, so symlinks
  are not followed", a documented limitation (`pipeline.rs:1222`), and
  `in_tree` is a prefix test (`line.rs:238`). So a link in the project
  pointing at `~/.ssh/config` is an inside read today, for `bash`'s `cat`
  and Claude Code's Read alike. A file tool knows exactly which file it
  touches, so it can do better: judge the given path and its resolved
  target, the stricter winning. `reading` already takes a file-system probe
  (`is_file`, for Grep), so a resolver fits the same shape, and Claude
  Code's calls gain it too.
  - *Built 11-03 (F2c).* `files::lands` follows every link (a link to
    nothing too: a Write makes its target), and the reading is two stages,
    `cat 'P'; cat 'T'`, so a landing can only make a call stricter.
    Shannon's own call is judged again at the run and refused if its
    reading changed (a link re-pointed beside it, or while the pane
    waited), then opened at the landing judged; the pane's title, the
    record and the result name it.
  - *One spelling at the gate (Steven, 11-09; review stop 17, E2 2, V H1,
    H2).* The gate matches by text, and macOS's `/tmp`, `/var` and `/etc`
    are links, so `Read /private/etc/hosts` asked under `deny * /etc/**`,
    and fixing one direction opened its mirror image. Now
    `shell::pipeline::normalize_path`, the door every rule, operand, `cd`
    and start already passes through, takes a link in `/` to where it
    leads. Toward the target because a link has one target where a target
    can have several links (`/bin` and `/sbin` on a merged Linux), and it
    is the spelling `getcwd` gives. As the kernel walks: at the root when
    the walk reaches it (`/tmp/..` is `/private`), without case on macOS
    (`/ETC/hosts`). Read once by `readlink`, no `stat`: `/home` leads to
    an automount. Only root makes an entry in `/`, so none is planted.
    What a person reads keeps their words: a rule is kept as written, a
    reason shows the operand as typed, and `A` offers it as typed unless
    a `cd` moved it. Two things show the gate's spelling: a landing, and
    `:rules`' scratch grant, which is one line now, not one per spelling.
    A glob at the root cannot be respelled, so a rule meets it through
    the links (`permission::line::across_root_links`), and a `re:` is
    shown every spelling of the path (`pattern::spellings`).
- **What runs unreviewed** (*measured 11-02*, with the start at the
  project): a redirect into a file inside is Allow (`cat > x`, `printf hi
  > x`, and `cat x > x`, an Edit's reading); `tee x` asks (a writer with
  no rule); outside asks; `.env` asks. `bash`'s description said every
  such write is "shown to the user first"; fixed in `d7535df`.
- **Unattended** (a `:child`, a `:Send` reply): a Read inside the
  project runs, as a reading command does. Edit and Write stop the job,
  as a writing command does, and their unattended descriptions say so,
  as `bash`'s does (`request.rs:148`).
- **The pane** shows the change, `e` edits the new text, and a deny
  returns an error result. That is today's flow for Claude Code's calls,
  here under `Executor::Shannon`.
- **Execution goes through `exec::Executor`.** The trait gains file
  operations (`FileOp`), so the test recorder sees them too and `cargo
  test` cannot write a file through this path. The sandbox, later,
  implements both kinds.
- **The record** keeps today's subjects. Shannon computes the edit's
  place from the file it edited, so no `tool_use_result` is needed, and
  keeps it as S21-Q19 does: positions only.
- **Secret handles in `content` or `new_string`:** S21-Q27 (built 11-03,
  §7).

### 3.6 What the model has read: stamps, read from the context itself

This is S21-Q20's mechanism, and the part I'm most rooting for.

- **A stamp per call.** Every Read, Edit and Write Shannon runs leaves a
  stamp beside its call, keyed by `tool_use_id`. It holds the resolved
  path, the lines covered, the file's line count, and a hash of the whole
  file as it was then. Never text.
- **"What the model has read" is computed from the messages about to be
  sent**: the stamps of the calls whose results are still in them. It is
  not a side cache. A fork, an edit of history, a squash or a compaction
  changes what was read because each changes the context; a call no
  longer in the context counts for nothing. An id is an occurrence, not a content hash, so two reads of one file
  stay two stamps.
- **The stamps are the running Shannon's** (review stop 16, C 7; traced):
  they live in the process (`App::file_stamps`), not in the session's
  store. After a restart (`shannon --resume`), a session's earlier reads
  count for nothing, and its first Edit of a file is told to Read it
  again. Persisting them by `tool_use_id` is docketed.
- **A stamp counts while its hash is the file's hash now.** A change by
  you, a formatter or a `bash` command makes the file unread: `a.rs
  changed since you read it; read it again`. That is an error result with
  no pane.
- **Edits carry coverage forward.** An Edit's stamp covers what the
  stamp before it covered, shifted by the edit, plus the new lines. A
  Write covers the whole file. So a model that read all of a file and
  then edited it still counts as having read all of it.
- **`[files] read_first = "any" | "whole" | "off"`.**
  - `"any"` is the default, as Steven set it: a partial read passes,
    because of tokens.
  - `"whole"`: the stamps must cover every line.
  - `"off"`: no check, though `old_string` must still match.
  - The dial later is `"near:N"`: the edit's lines and N lines around
    them must have been read.

  It applies to Claude Code's backend too, from the hook, once its Read's
  line counts are confirmed on the stream (*unknown*: a live probe).
- **Efficiency from the same stamps.** A Read of a range whose stamp is
  still in the context, with the same hash, is answered `a.rs is
  unchanged since your read above`, which saves a whole re-read.
- *Built 11-03 (F3).* Each file outcome's stamp is kept by its
  `tool_use_id` with a hash of the very result the model was handed
  (`App::file_stamps`); a call's context is the stamps whose results, word
  for word, are in the session's messages (`stamps_handed`,
  `App::file_context`) — a round's earlier results count, a call beside it
  in its round does not, another session's never. Named by the file the
  call opened (`Job::file`: its landing, or its path), not
  `fs::canonicalize`, which spelled `/tmp` as `/private/tmp` where the gate
  does not. Checked twice: an Edit with no read in the context is told
  where it arrives, before a pane asks you about a change that could not
  run (`Context::unread`, no disk touched on the UI thread); the rest at
  the run (`Context::seen`): the file's hash now, a Write over a text file
  that exists, `"whole"`'s every line. A Write over a file that is not text
  asks no read (none could show it). The unchanged re-read points back
  only to lines a Read showed (`Stamp::shown`), not ones an Edit or Write
  wrote, since a re-read after a change is how the model checks it. The
  descriptions say the shipped rule; they do not read `[files]` (under
  `"off"` they over-ask, which costs a read). Claude Code's half waits on
  a live probe of its Read's counts on the stream.

### 3.7 Steering

`bash`'s description, or the system notes, gains one sentence: read files
with Read and change them with Edit and Write, not with `cat`, `sed` or
heredocs. That way the record and the diff exist, and BSD `sed` is out of
the way.

## 4. Efficiency, counted

- **Definitions:** three tools ride in the cached prefix (`tools` →
  `system` → `messages`), so after the first request they cost a cache
  read. Their size is *unmeasured*. Measure each with the count-tokens
  endpoint before the descriptions are fixed, with a target of `bash`'s
  description or less each.
- **Per call:** an Edit sends only the change, both ways. Results are one
  sentence. Reads are windowed and numbered `N<tab>line`. An unchanged
  re-read costs a sentence.
- **Per turn:** no `sed` dialect failures, and no heredoc for the gate to
  read.
- **What it costs:** `read_first = "whole"` costs a full read. That is
  why it is not the default.

From Anthropic's tool-use and caching pages (read 11-02), for F2's
definitions:

- **Descriptions are worth their tokens.** The docs call detail "by far
  the most important factor in tool performance", and say at least 3–4
  sentences a tool. They are cached after the first request, at 0.05× on
  Opus 5.5.
- **Results "only high-signal information"**, and errors that say what to
  try next. A model given an invalid-input error retries 2–3 times. Both
  are what §3.2–3.4 return.
- **`strict: true`** guarantees inputs match the schema. It pairs with
  refusing unknown keys (`additionalProperties: false`). To be measured
  against the descriptions' size.
- **One tool set per session.** A change to `tools` invalidates every
  cached level (tools, system, messages), so the three are offered always
  or never, not per turn. That holds for `bash` and `skill` today too;
  TODO has the finding.
- **Never force a tool.** `tool_choice` `any` or `tool` returns a 400 on
  Opus 5.5, Sonnet 5.5 and Fable 5.1.
- **Tool search pays from about 10 tools or 10k tokens** of definitions.
  Six tools stay plain.

## 5. Alternatives (verified against Anthropic's docs, 11-02)

**Anthropic's text editor tool**: `type: "text_editor_20250728"`, `name:
"str_replace_based_edit_tool"`, optional `max_characters`, listed for every
current model. Its commands are `view` (a file with `view_range`, or a
directory), `str_replace`, `create` and `insert` (after line N).

- For: an Anthropic-defined schema, and one tool.
- Against:
  - An Anthropic-only wire form: `ToolDef` has no `type` and is
    provider-neutral by contract (`request.rs:96`).
  - A second vocabulary beside Claude Code's, so records, the pane, the
    diffs and Q20 would each be written twice or translated.
  - `insert` by line number trusts a numbered view to still be current.
  - `create` overwrites, with no read in its contract.
  - `view` lists directories, a second listing tool beside `ls`.
- Its token overhead is not on the tool reference page (*unknown*).

**Novel shapes of our own**, for example an `edits` array in one call, or
edits anchored by line ids instead of `old_string`.

- For: free to design, and possibly fewer tokens per edit.
- Against: a schema no model has seen, and a third vocabulary. Several
  edits already cost no extra round trip, since a model may issue several
  calls in one response, and no extra pane inside the project, where Q7
  lets a write run unasked (`decide.rs:1391`, `path_default`).
- An experiment worth running later, measured against these.

## 6. Phases (each green-committed, mutation-checked)

- **F1 — core.** A `files` module: inputs parsed and refused with
  reasons; Read, Edit and Write run over bytes as pure functions; stamps;
  `exec::Executor` gains `FileOp`. Proptest that an edit never panics
  (char boundaries, CRLF, empty strings).
- **F2 — gate, pane, record.** `reading` moved and its consumers named
  (Edit as read plus write); the symlink measurement; the tools offered,
  with foreground and unattended descriptions; `Executor::Shannon` calls
  in the pane with their change and `e`; records with positions; the
  steering sentence. Smoke cannot reach a model's tool call today (the
  stub streams text; `:tool` takes a shell line), so the stub learns to
  call a tool, or `:tool` learns `Read`/`Edit`/`Write`. That choice goes
  in F2's commit.
  - *Built 11-03 (F2d): the stub learns it* — my call, made for Steven's
    tutor draft as much as for smoke ("Do we have a way to force the tool
    call with the stub? … a useful debugging tool and works slightly
    different than `:analyze`"). A fenced block whose info string is
    `tool <name>` is a call: `bash`'s body the command as it stands, any
    other tool's a JSON object (one unreadable refuses them all, said).
    `:tool` stays a shell line. Answered, the stub says what each call
    returned as the model was handed it (`backend/stub.rs`,
    `:help shannon-stub`). Every tool the model can name goes through it,
    the gate's real path end to end; `:tool` would have been a second
    door into the file seam.
- **F3 — read-first.** Stamps kept beside the call and read from the
  messages sent; `[files] read_first`; the unchanged re-read's note; Claude Code's
  half if its stream carries Read's counts.
- **F4 — docs.** `:help shannon-files`, `tool-use.md`,
  `claude-code-backend.md`, the options; TODO and milestone entries.

After F2, D1 has a `Change` from both backends.

## 7. Questions — answered 11-02

- **S21-Q25 — the schema: a middle ground.** Our own tools, taking
  Claude Code's required arguments, with optional ones of ours (§3.1).
- **S21-Q26 — read-first: as proposed.** Steven asked whether it belongs
  under `[files]` or `[editing]`. There is no `[editing]` section, and
  "editing" here already means *your* editing (`e` on a call, `$EDITOR`,
  `compact_review`). The section also holds Read's caps, which edit
  nothing. Sections name a subsystem (`[exec]` runs commands, `[secrets]`,
  `[merge]`), so: `[files]` (my call).
- **S21-Q27 — a secret handle in written text: as `bash` does** (Steven:
  "The agent doesn't necessarily know that that's the secret"). A hard
  ask, then the value is substituted as it is written; a later Read comes
  back as the handle (`redact.rs:174`).
  - *False until review stop 16 (E 1, HIGH):* a Read numbered and cut its
    lines before the scrub. So a value over two lines, or one the
    2,000-character cut fell inside, was no longer one substring, and
    reached the model in clear. Now each line is scrubbed whole before it
    is numbered or cut. Where a value could run over lines (one held or
    configured with a newline, a key block, a URL's userinfo broken by one:
    `secrets::crosses_lines`), the file is scrubbed whole first, its lines
    kept (`secrets::redact_lines`): the handle stands where the value
    starts, and each later line it reaches reads `[… continued]`, so a
    number is still the file's. Anywhere else, the lines shown are
    scrubbed one by one, at the window's cost: scrubbing a 16 MiB file
    whole took 2.1 s. The redactions reported are those in the lines shown.
    `docs/Security/SECURITY-2026-11-03-a-read-splits-a-held-secret.md`.
  - *Built 11-03.* The spend is said in the line the gate judges:
    `cat > 'P'; printf '%s' '{{db.…}}' > 'P'` (`Call::judged`, a stage
    for each handle a Write's content or an Edit's `new_string` holds,
    into the file it opens), so the gate's scan makes it the hard ask a
    command's spend is, and the pane, a job's unattended check (it holds)
    and a re-gate read one line. The values go in at the run
    (`exec::run_file` → `Call::spent` → `Aliases::put_in_file`): the
    secret pass alone, an unknown handle or one kept off the wire refusing
    as for a command, but none of the shell's rules — `~` is text in a
    file, and a value's quote or newline changes no shell line. An Edit's
    `old_string` is put in too, or it could not find what a Read showed by
    its handle; that is not spent. An `e` that adds or takes out a handle
    is judged again from the edited text (`App::apply_write_edit`; it said
    the line could not change), so the run's check holds the call to the
    line approved. Claude Code's Write takes a handle as text — its model
    sees tool output unredacted, so it meets one only if your prompt held
    it; docketed.
- **S21-Q28 — order: mine** (Steven: "I'll defer to you"). F1–F4, then
  `aside.md`'s experiment, then S21-Q10's phases 3–4, then D1–D4, then N2.
  - The file tools first: they are what you are using and testing next,
    and D1, read-first and the aside experiment all build on them.
  - The experiment next, while the tools are fresh. It needs a spend
    estimate before any live run.
  - S21-Q10's last two phases are internal. Nothing above depends on
    them, and D3 waits on them, so they come before the diffs.
