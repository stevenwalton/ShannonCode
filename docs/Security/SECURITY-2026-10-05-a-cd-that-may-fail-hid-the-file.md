# SECURITY — a `cd` that may have failed hid the file a command reads

**Status: FIXED 2026-10-05 (the commit that adds this file), by the change
Steven asked for — "We should be tracking the cwd" — not as a patch of its
own. It was live on `main` (pushed) until then.**

**Found:** 2026-10-05, by the parent, while writing the tests for that
change: the credential row was meant to show the gate still met a file
under the new model, and a mutation putting the OLD model back left it
GREEN only because the row's `cd` was itself a credential directory. Moved
to a `cd` that is not, the old model failed it. Reproduced through
`RuleSet::decide_line`, and the row is now a test
(`a_credential_past_a_cd_that_may_have_failed_is_met`).

## HIGH (FIXED) — `cd /home/me; cat .docker/config.json` was not met

Past a `cd` joined by `;`, a newline or `||`, the next command may run where
the line began (the `cd` failed) or where it went, so the pipeline gave it
`Cwd::Unknown` (review 20, Reviewer E2). Sound for a path RULE, which asks
under `Unknown` (`path_indeterminate`). Not for the credential gate, which
reads a token as written under `Unknown` (`stage_spellings`'s doc says so:
"a directory entry … does not" meet it), and a relative token names no
credential as written.

    [claude_code] path_policy = "deny" (the default), start ~/proj

    line                                          was     now
    cd /home/me && cat .docker/config.json        Deny    Deny
    cd /home/me; cat .docker/config.json          ALLOW   Deny
    cd /home/me
    cat .docker/config.json                       ALLOW   Deny
    cd /home/me && true; cat .docker/config.json  ALLOW   Deny

ALLOW is `DefaultRead`: it ran, and on Claude Code its model reads the
output unredacted. With Shannon's own model the output is redacted
(`path_policy = "ask"` asks), so the exposure is Claude Code's.

**Not reached** by the `cd`s a model types most: `cd ~`, a bare `cd`, and
`cd ~/.docker` are each denied themselves (a directory holding a
credential). Home spelt as a path is not, and a model working in a project
spells paths absolutely.

## The fix

`Cwd::OneOf`: past such a `cd`, the places the line may be in, every one
known (`reach` in `parse_line`). Every reader quantifies over them
(`Cwd::places`): the credential gate and a deny's contact meet a file at
ANY (`files_named`, flattened in `token_spellings`); an allow's containment
must cover it at EVERY place, an operand's or a flag's file; `inside_start`
holds only if every place is inside; `A` writes a path only where every
place names the same file.

## HIGH (FIXED in review) — `! cd X && cmd` ran `cmd` where `cd` FAILED

Found by Reviewer A in the review of the fix above, reproduced through
`decide_line`; live on `main` (the old model also took `&&` to mean "the
`cd` succeeded"). `peel_control_flow` dropped `!` as "negates a command's
status without changing what it does" — true of what it does, false of what
runs next: past `&&` after a `!`, the next command runs only if the `cd`
FAILED, i.e. where the line began.

    start ~/proj, path_policy = "deny"

    line                                                  was     now
    ! cd /nonexistent && cat ../.docker/config.json       ALLOW   Deny
    allow rm *, deny rm important/*:
      ! cd /nonexistent && rm important/x                 ALLOW   Deny
    allow rm build/*:  ! cd build && rm x                 ALLOW   Ask

The fix: a negated stage (`Finished::negated`) never narrows the next
command to its success; past it, the line is in any place it may be.
Validate: `cargo test -p shannon-core a_credential_past_a_cd`.

## Bounded

Each relative `cd` that may fail doubles the places (`cd d1; cd d2; …` is
2^k), and the readers work per place on the UI thread: ten `cd dN; make; cd
..;` took 8.6 s to decide in debug (Reviewer C). Past `MOST_PLACES` (8) the
line is `Unknown`, which asks.

## Residual

`cd $D; cat config.json` (a `cd` that cannot be followed) is still read as
written, and allowed: the documented `$X` residual (`stage_spellings`).

## Validate

    cargo test -p shannon-core a_credential_past_a_cd_that_may_have_failed_is_met

and, to see the old model fail it, replace `_ => reach.clone()` in
`parse_line` with `_ => Cwd::Unknown` (copy the file aside first).
