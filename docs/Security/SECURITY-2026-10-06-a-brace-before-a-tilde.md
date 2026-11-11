# SECURITY — a brace before a tilde wrote `~-`

**Status: FIXED 2026-10-06 (the commit that adds this file). On `main`
since at least `803bf76` (verified there); the `~-`/`~+` handling it slips
past dates from SECURITY-2026-09-30 R2-3 (inferred, not bisected).**

**Found:** 2026-10-06, review round 27, Reviewer E2 (security), attacking
the brace fix of `SECURITY-2026-10-06-a-brace-closed-at-the-first.md`.
Reproduced through `RuleSet::decide_line` under `RuleSet::defaults()` on
`803bf76` and `785779e`, and with bash by `printf` only.

## HIGH — `find . -name ~{x,-}` ran as `find . -name ~x -delete`

Bash expands braces BEFORE tildes, so a brace can write a tilde-prefix:

    $ bash -c 'OLDPWD=-delete; PWD=-x; printf "<%s>" -name ~{x,-} ~{x,+}'
    <-name><~x><-delete><~x><-x>

`~-` is `$OLDPWD` and `~+` is `$PWD` — variables the line itself may set.
`could_become_a_flag` and `splits_unquoted` knew `~-`/`~+` only as
written: with a brace between, the word's literal prefix was `~`, neither
empty nor dash-led, so both said "cannot become a flag":

    shipped rules only                        803bf76   785779e   fixed
    OLDPWD=-delete; find . -name ~{x,-}       ALLOW     ALLOW     Ask
    find . -name ~{x,+}                       ALLOW     ALLOW     Ask

A `find` that only reads, as far as the gate could see, deleted. `~{x,}-`
and `~{1,x}` (a directory-stack entry) are the same shape.

**Reach:** the default policy; the model must write the shape and set the
variable — on the same line, as above.

## The fix

`shell::tilde_by_brace`: a word that begins with `~` and holds a brace that
opens anywhere in the tilde's prefix (before its first `/`) can become a
flag — read wide, the safe side. Consulted by `could_become_a_flag`; the
same rule in `splits_unquoted`'s brace arm (a literal prefix that is a
tilde with no `/` yet). A brace after the slash (`~/{a,-b}`) is a path's.

Tests: `a_placeholder_brace_is_no_hidden_flag` — the lines above, `~{x,}-`,
and the operand form, under `RuleSet::defaults()`, and the predicate's own
answer (an operand's other ask hid it: a mutant without the check passed
the engine lines until the assertion was added). Three mutants, all RED.

## To validate

    cargo test -p shannon-core a_placeholder_brace_is_no_hidden_flag

## Lesson

A prefix-based "can this become a flag" must model every expansion that
runs AFTER the one it reads and rewrites the prefix. Brace expansion was
modelled; the tilde expansion that follows it was not. Ask what each later
shell step does to the literal text, not only what the brace does.
