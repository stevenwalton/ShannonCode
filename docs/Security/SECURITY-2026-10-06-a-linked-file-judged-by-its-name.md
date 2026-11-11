# SECURITY — a linked instruction file was judged by its name

**Status: FIXED 2026-10-06 (the commit that adds this file). Never on
`main`: it lived only in the unpushed memory-and-skills commits
`7c3e32c..1d2d638`.**

**Found:** 2026-10-06, review round 27 — by all three of Reviewers A, B
and C, independently. Reproduced by each with a probe in a scratch
worktree; A's used the shipped `RuleSet::defaults().credential_path`.

## HIGH — a repository's `AGENTS.md` could be your key

Memory and skills read files the user did not write: a cloned repository's
`AGENTS.md`, `SHANNON.md`, a `.shannon/skills/*/SKILL.md`. Each was put to
the gate's credential judge (`[secrets]`) — by its OWN path. The judge is
lexical, and the read follows links:

    AGENTS.md -> ../../.ssh/id_ed25519      judged "AGENTS.md": Loaded
    .shannon/skills/x/SKILL.md -> ../../../.env.local
                                           judged by its name: read

`Adder::add` had canonicalized the path — only to drop duplicates — and
judged the link's spelling. So the key was read whole on the first turn of
every session and sent to the provider (the API, or `claude -p`), with no
tool call and no prompt; it was stored in the session too. The transcript
said only `⚙ told the model: AGENTS.md`. The `skill` tool, ungated by
design, returned a linked `.env` the same way.

Worse than the gate's known lexical limit (TODO, "symlinks"): there the
MODEL makes the link and runs a command the gate sees, and the command's
output is redacted. Here Shannon itself did the read, unasked, with the
canonical path in hand — and the text skipped redaction entirely: told text
and skill text were masked for held secrets only, so even an unlinked
`password: …` line in an `AGENTS.md` left in clear while the same line from
`cat AGENTS.md` arrived `[redacted: named secret]`.

## The fix

1. **Judged as named AND as it resolves** — `knowledge::credential(path,
   real, judge)`, the one predicate both readers of a found file ask
   (`Adder::add` and `knowledge::skill_text`).
2. **Redacted as command output is** — `App::outbound`
   (`secrets::redact` with `[secrets]` and the aliases), the one door
   `App::told` and `App::skill_text` pass. A file whose text was scrubbed
   is said so in the transcript line (`AGENTS.md (redacted)`).
3. The source path is masked too (Reviewer A: a path names your home,
   which `alias_home` promises never travels).

## To validate

    cargo test -p shannon-core a_link_to_a_credential_path_is_never_read

and the app-level tests in `crates/shannon-tui/src/app/tests/knowledge.rs`
(a file under `[secrets] paths` is NOT READ and absent from the request;
redaction of told text). Each guard was mutation-checked RED.

## The other half — where a link leads, not only whether to a key

Found the same day by Reviewer E1 (security), attacking the fix above, and
confirmed by test: the resolved path was judged against `[secrets]` only.
A repository's `AGENTS.md` linked to `~/.bash_history` — no credential —
was still read and told every session, unasked; and a repository's
`.shannon/skills` linked out of it put an `allow :read <link>/**` in force,
approving reads wherever it led. Reads stay inside approved places
(Steven, 10-05), and a repository chose this place.

Fixed: a file or place a RELATIVE location found (the repository's own)
must resolve inside the repository (`locate::leaves_the_repository`), or it
is not read — `:memories`/`:skills` list it, "leads out of the
repository" — and no rule is made for it (said at launch). A placed
location (`~`, `/`, `{config}`) is yours, and follows its links. Test:
`a_repository_link_out_of_the_repository_is_not_read`; both checks
mutation-checked RED.

Also from E1, fixed: a skill's arguments are placed piece by piece and stop
at the cap (the old replace-then-cut built 600 MB first — in ~60 ms here, so
the bound is structural, not timed); `<shannon-context` is defused in any
case and as an opening tag too; a place is read for at most 1000 entries,
and says so. Probed and held: `ln -s` or `cp -r ~/.ssh` into the memory
write location asks (`the_memory_write_place_plants_no_link_unasked`) — the
write through a link already there is the docketed lexical residual.

## Residual

A link swapped between the judge and the read (a race with another process
on the machine) is not closed: the read opens by path. Out of the threat
model here — the adversary is the repository's content and the model, not
a concurrent local process.
