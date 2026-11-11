# SECURITY — a backtick substitution was read as quoted text

**Status: FIXED 2026-10-05 on `worktree-param-default-bypass` (the commit
that adds this file). Live on `main` (pushed) since `5b1d8b6` (2026-10-05,
"a quoted glob is text the program gets, not the shell's"), and made
worse — never on `main` — by `716f4f9` on the branch.**

**Found:** 2026-10-05, review round 25, Reviewer A (correctness), tracing
`716f4f9`'s `hidden_flag` change back to the quoting table it trusts.
Reproduced through `RuleSet::decide_line` against `main`'s engine (a harness
built from the `main` checkout) and against the branch.

## HIGH — a path-scoped allow covered `` rm build/`echo ../../x` ``

`5b1d8b6` taught the lexer which words the shell still expands
(`expands_as_written`, read from each word's source slice), so a quoted
`'*.rs'` reaches the program as text. The scanner knew `$`, globs and
braces — and not a backtick. A backtick substitution's word is spliced as
`$()` (`SUBST_VALUE`), but its source holds no `$`, so `Command::expands`
called it literal. An allow's containment then graded the TEXT `$()` as a
file:

    allow rm build/*                       main      fixed
    rm build/x                             Allow     Allow
    rm build/$(echo x)                     Ask       Ask
    rm build/`echo x`                      ALLOW     Ask
    rm build/`echo ../../../etc/passwd`    ALLOW     Ask

`build/$()` matches `build/*`, so whatever the backticks print is deleted —
`build/../../../etc/passwd` included. The cause cited was the user's own
rule, with the trigger `build/$()`. Any path-scoped grant for a program that
acts on its operands is the same shape (`allow cp …`, `allow tar …`,
`allow :write build/**`).

**Reach:** needs a path-scoped allow the user wrote — no shipped rule
allows a destructive program — and a model that writes backticks, which a
model rarely chooses but can.

## CRITICAL on the branch only — `` find . `echo -delete` `` ran

`716f4f9` made `Command::hidden_flag` skip a word the shell does not expand,
so a quoted `fd '*.toml'` stops asking. Trusting the same table, a backtick
word was "not expanded", and `` find . `echo -delete` `` — `find . -delete`
to bash — was a plain read: **Allow** under the default policy. On `main`
it asks (`hidden_flag` there reads text, and `$()` can become a flag). Also
on the branch: `` cat `echo /etc/passwd` `` placed as the relative file
`$()`, inside the start, ran.

## The fix — the class, in both scanners

A backtick is `$(…)` written the old way: it expands outside single quotes
(`expands_as_written`) and its output word-splits when unquoted
(`splits_unquoted`). Both scanners now say so; every consumer of the table
(`Command::expands` → `hidden_flag`, `placed`, containment's `literal`;
`words_may_split` → `flags_are_final`) is right by the same edit. Tests:
`a_backtick_substitution_expands` (the hidden flag as an operand and inside
a flag's value, `rm build/`…`` under `allow rm build/*`, a single-quoted
backtick as text) and the read rows in
`a_read_runs_unasked_only_inside_where_shannon_started`. Both scanner edits
mutation-checked RED.

## To validate

    cargo test -p shannon-core a_backtick_substitution_expands

or, against any build, `RuleSet::configured` with `allow rm build/*` and
`decide_line("rm build/`echo x`")` → `Ask`.

## Residual

Other expansion forms the scanners might miss were checked by hand: `$'…'`
and `$"…"` begin with `$`; `<(…)` and `>(…)` are lexed as substitutions of
their own; history expansion (`!`) is off in a non-interactive shell. None
found; the class is "an expansion the quoting scan does not know", and a
new shell syntax is a new row.
