# SECURITY — a key file named through a link was guarded by its name alone

**Status: FIXED 2026-10-27 (the commit that adds this file), not pushed
then (`origin` was private).
The gap is on `origin/main` (14d0f13): the name-only registration
predates 09-30, and became an asymmetry on 10-11, when own state and
Claude Code's home began registering both names (`with_real_path`, review
2026-10-11, E2).**

**Found:** 2026-10-27, review stop 12, while fixing T6 2 (launch's key-file
registration held by no test). Reproduced by test before the fix.

## LOW–MEDIUM — the key Shannon authenticates with, read unasked

Launch registers every key file with the gate as Shannon's own state, so
the model cannot read the credential this process holds. The gate reads a
path as written. Own state and Claude Code's home were registered by the
name given AND by where it really is; the key files only by the name given.

So a `SHANNON_API_KEY_FILE` named through a link that leads inside where
Shannon started:

    SHANNON_API_KEY_FILE=~/keys/k     ~/keys -> ~/proj/secrets
    (started in ~/proj)
    cat secrets/k                     a modelled read inside: Allow, unattended

The guard named `~/keys/k`; the model's spelling, the real one, met no
guard, and a read inside the start allows by default. The output then
passes the output redaction, which may mask a key-shaped string — not
verified for this case, and not the promise: the promise was that the read
asks.

**Scope.** Only a named key file reached through a link into the start (or
into a place a user rule allows). The saved key (`shannon setup`'s) was
registered by both names already, by `register_own_state`. Outside the
start a read asks by default, so the default policy alone did not expose a
named key outside the project.

## The fix

`keys::protected_key_files` — the list launch hands the gate, now in the
library so it is tested (it was a loop in `main::app_config`) — registers
each key file through `with_real_path`, as the other two surfaces are.
`with_real_path`'s doc names all three.

## To validate

    cargo test -p shannon-tui --lib keys::tests::

`a_key_file_named_through_a_link_is_guarded_where_it_really_is` builds the
link into a start and asks the gate (`RuleSet::with_own_state`, the door
`App::protect_paths` feeds): RED before the fix (Allow), and RED under the
fix's mutant (`with_real_path` taken out). Launch's one call into it is
checked end to end by `tools/drive/smoke.py` §19.

## Residual

The gate's own lexical limit stands: a link the MODEL makes to the key,
then reads, is judged by the link's name — `docs/Milestones/TODO.md`'s
"Residual (a lexical gate's, not the scratch's)", which the sandbox layer
is what closes. Out of this fix's scope.
