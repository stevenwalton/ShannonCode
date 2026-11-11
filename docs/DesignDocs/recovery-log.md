# The recovery log

*Status: proposal. Written 2026-08-14, three days after we lost a month of work
and got most of it back by accident.*

## The one-sentence version

An agent that can run a shell can destroy the workspace, and the thing it
destroys usually includes the backup — so Shannon should keep a **byte-exact,
content-addressed, out-of-tree snapshot of every file it touches**, and the
hard part is not storing them, it is *addressing* them.

## Why this document exists

On 2026-08-11 a `git commit -m` whose message contained a backticked
`` `find . -delete` `` ran that command instead of recording it. The working
tree went, and `.git` went with it, because `.git` lives *inside* the thing that
was deleted. Thirty unpushed commits, twenty-seven days.

We recovered nearly all of it. That is the interesting part, because **we
recovered it from four systems, none of which was designed to be a backup**:

1. `~/.claude/file-history/` — Claude Code snapshots files it edits, keyed by
   content hash, in a directory outside the project. 1,727 files, 115 MB,
   byte-exact, with real timestamps. This was the single best source and we did
   not know it existed until the fourth day.
2. **Reviewer transcripts.** Our own review skill tells whole-surface reviewers
   to *read whole files* — because a diff cannot show a rule stated in four
   places and correct in three. 204 review sessions therefore contain complete
   file bodies. This is what returned every `shell/*` module.
3. `.bak` copies left by mutation-checking — files copied aside before being
   deliberately broken to prove a test catches it.
4. Session transcripts, which record `Write` contents and `Read` results.

Every one of those is a side effect of doing something else. The lesson is not
"we got lucky". It is that **the information was already being written down four
times over, and what was missing was an index.** We spent days doing archaeology
on data we already had.

## What the accidental system got right

Worth stating plainly, because these are the properties to keep:

- **Out of the tree.** `find . -delete` took `.git` because `.git` was inside
  the blast radius. `~/.claude/file-history/` survived for one reason: it was
  somewhere else. A recovery store that lives under the workspace root is a
  recovery store that shares the workspace's fate.
- **Byte-exact copies, not deltas.** The transcripts contain ~3,458 `Edit`
  records and we could use **none** of them, because an unknown number are
  mutation checks — code deliberately broken and reverted seconds later — and
  they are indistinguishable from real edits. Replaying deltas would have
  faithfully reintroduced every bug we had proved fixed. A snapshot carries no
  ambiguity about intent.
- **Content addressing.** Identical content stores once. That is what makes
  "snapshot everything, always" affordable rather than absurd.
- **Snapshot on read, not only on write.** The reviewer corpus is the proof: a
  file the agent merely *looked at* is a file we can restore. It is a
  surprisingly complete net for nearly no cost, and it covered exactly the
  modules that had no `.bak` — because nobody was editing them.

## What it got wrong, which is the actual design requirement

**`file-history` tracks tool operations, not the filesystem.**

It hooks `Write` and `Edit`. From 2026-08-08 onward most of the editing in that
session was done with Bash heredocs (`python3 - <<'PY' … PY`), and it saw none
of it. So the store went stale on the *hottest* files at exactly the moment they
were changing fastest — `app.rs`, `app/tests.rs`, `conversation.rs` and twenty
others. Its last snapshot of `app.rs` is 29 KB behind the truth.

That is the requirement, stated as a property rather than a list:

> **The trigger is "a file under the workspace changed", not "a particular tool
> was called."**

An agent with a shell can write a file by a hundred routes: `>`, `>>`, `tee`,
`sed -i`, `python`, `patch`, `mv`, an editor it spawned, a build script, a
formatter. Enumerating them is the mistake `[[feedback_enumeration_means_unmodeled]]`
names — a curated list of the ways files change is a proxy for a property nobody
computed. We already know how this ends, because we already shipped it once:
`extract.py`'s partial-read guard tested a JSON key that never existed, so every
truncated read was indexed as complete, and `store/failing.rs` went into the
tree as 80 lines of a 129-line file. It compiled. It passed. The only trace was
an unused-import warning.

## Design

### Where it lives

Under the XDG state dir, beside the session store, **never under the workspace
root**. Owner-only, files `0600` and dirs `0700`, matching the posture the log
dir and tempfiles already hold (`[[feedback_persisted_data_perms]]`).

    $XDG_STATE_HOME/shannon/recovery/<workspace-hash>/
        objects/<blake3-prefix>/<blake3>        content, stored once
        index.jsonl                             append-only observations

### What an entry is

An append-only record, one per observation:

    {"path": "crates/…/app.rs", "hash": "…", "at": "…", "size": 574671,
     "how": "read" | "write" | "scan", "turn": "<TurnId>"}

`objects/` is content-addressed with the same blake3 we already use for
`TurnId`, so the machinery exists. `index.jsonl` is the part that matters: it is
the thing whose absence cost us three days. **The index is the product; the
blobs are the easy half.**

The `turn` field is what makes this Shannon's rather than a generic backup: an
entry is tied to the turn that produced it, so the graph can answer *"what did
the tree look like when this turn ran"* — and `:graph` is already a viewer.

### How a snapshot is triggered

Three ways, in increasing cost:

1. **On touch.** Any file the agent reads or writes through a tool. Free, and it
   is the net that saved the `shell/*` modules.
2. **On turn boundary, by scan.** Walk the workspace (respecting `.gitignore`
   plus an explicit exclude list), hash each file, record only what changed.
   This is the one that closes the heredoc blind spot, because it asks the
   filesystem rather than the tool layer. Cost is a `stat` per file and a hash
   per *changed* file; on a repo this size that is milliseconds.
3. **On demand** — `:snapshot`, and before anything the permission engine rates
   `Destructive`.

Defaults per `[[feedback_sane_defaults_rings]]`: (1) always, (2) on by default
with a size ceiling, (3) always available. A user with a 40 GB checkout turns
(2) off and keeps (1); that is the ring structure, not a special case.

### What it must never do

**This is a black-box recorder, not version control.** It never creates commits,
never touches the user's git history, never adds a remote, and never requires
management. Aider's auto-commit is the cautionary tale
(`[[feedback_no_externalized_costs]]`): it solved its own problem by moving the
cost onto the user's history, where every collaborator pays it forever. If the
user never has to think about this system, it is working.

It is also **not** a replacement for pushing. It buys back the window between
commits, which is exactly the window that killed us.

### Reading it back

`:recover` — list observations for a path, newest first, with timestamps and
sizes; `:recover <path> <at>` writes that version to a scratch buffer, not over
the working file. Restoring is the user's decision and must be inspectable
before it happens; the viewing-first rule applies here more than anywhere,
because the failure mode of a recovery tool is silently restoring the wrong
thing (`[[feedback_build_environment]]`).

## Open questions

- **Retention.** Content addressing makes this cheap but not free. A ceiling by
  age or by total bytes, with the newest-per-path always kept? I would rather
  ship "no eviction" first and measure, since the whole value is having the
  thing you did not think you would need.
- **Excludes.** `.gitignore` is the obvious default and it is *wrong* in at
  least one direction: a key file is gitignored and must never be
  snapshotted, while build outputs are ignored and need not be. Credential-shaped
  paths must be excluded on the `secrets::sensitive_path` axis, not the git one.
  This is worth getting right before the first line is written — a recovery log
  that snapshots a credential has created a new copy of it in a new place, which
  is precisely the harm the secrets subsystem exists to prevent. (A real
  instance: an editor's undo file of a key file.)
- **Whether the agent should see it.** My instinct is no by default: an agent
  that knows it has undo is an agent with less reason to be careful, and
  `[[project_shannon_security_posture]]` says the adversary is an eager
  optimizer. The user can ask on its behalf.

## The test this design has to pass

Not a unit test — a rehearsal. **Delete the working tree, and restore it.** If
that is not a thing we have actually done, we do not have a recovery system, we
have a directory of files that we believe would help. Every one of the four
accidental sources above was believed to be adequate right up until someone had
to use it, and three of the four turned out to have a silent hole:

- transcript `Read` results are truncated for large files, and the truncation is
  invisible;
- `Edit` deltas are unreplayable for reasons that only became clear afterwards;
- `file-history` is blind to shell-mediated writes.

A restore rehearsal in CI, on a scratch copy, is the only thing that finds the
fourth hole — the one we have not thought of yet.
