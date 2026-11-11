# SECURITY — a `paths_except` carve-out lifted every credential guard

**Status: FIXED 2026-10-27 (the commit that adds this file), not pushed
then (`origin` was private).
On `origin/main` since 0d2a89a (2026-10-10, the switch to the acts
engine): the old matcher held the rule, and the new encoding did not carry
it.**

**Found:** 2026-10-27, review stop 12, porting the deleted
`secrets::sensitive_path`'s tests to the gate (Q1). Reproduced by test
before the fix.

## LOW–MEDIUM — a key file in a carved place, read unasked

`[secrets] paths_except` carves a place out of a guard that holds it whole:
by default Claude Code's memory and skills places out of `~/.claude`, so its
memories read. The rule, as the old matcher and its doc had it: "`*.pem`
does not hold the place, so a `key.pem` inside it still is" — a place is
carved out of what contains it, never out of what it contains (Steven,
10-06).

The gate's encoding (`defaults::credential_bodies`) attached every
carve-out to every guard, and the path decision (`decide::path_cands`)
dropped any path inside one. So inside a carved place EVERY credential
guard was lifted:

    cat ~/.claude/projects/<p>/memory/deploy.pem     Allow (unattended)
    … memory/id_rsa, memory/.env, memory/credentials, memory/x.key

and a user's own carve-out the same way (`paths = ["~/vault", "*.pem"]`,
`paths_except = ["~/vault/public"]`: `~/vault/public/k.pem` unguarded).
`credential_path` — what memory and skills ask before reading a file — went
through the same door, so a credential file in Claude Code's memory place
would also be read by Shannon and sent with the memories.

**Scope.** A credential file has to be inside a carved place. The model
cannot copy one there without a read the guard asks about; a link it makes
there is the gate's known lexical residual (below) and the knowledge reader
judges a file's real path as well. So the exposure is a credential the user
(or another program) put in a carved place.

## The fix

`decide::carved`: a path is carved out of a guard only inside a
`paths_except` place the guard holds whole — the shortest leading part of
the path an exception names, matched by the guard — bounded (32 places,
4096 bytes; past either, held: it fails closed).

## To validate

    cargo test -p shannon-core --lib permission::credential_tests

`a_carve_out_lifts_only_what_holds_the_whole_place` (RED before the fix:
`memory/deploy.pem` was not a credential path; commands `cat memory/id_rsa`
etc. asked of the gate too) and `the_carve_out_reads_a_bounded_part_of_a_word`.
Mutants RED: any carve-out lifting any guard; the bound removed.

## Residual

The gate is lexical: a link the model makes in an approved place and then
reads is judged by the link's name — `docs/Milestones/TODO.md`'s "Residual
(a lexical gate's, not the scratch's)", which the sandbox layer closes.
