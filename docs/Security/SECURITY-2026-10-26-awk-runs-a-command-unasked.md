# SECURITY — awk ran a command unasked when `system` was spelled with a space

**Status: FIXED 2026-10-26 (the commit that adds this file). On `main` and
`origin/main` from `410f0cb` (2026-10-09, permission grammar P1: the awk
language reader) until then — pushed, so released builds of that span
carry it.**

**Found:** 2026-10-26, review stop 12 (the test-suite review), Reviewer T8,
probed through `RuleSet::defaults().with_start(…)` and against macOS's BWK
awk 20200816. Reproduced independently the same day by the regression test
below, on `6d80f43`.

## CRITICAL — any command, no rule, no prompt, safe unattended

awk's program is code. The gate reads it with the awk language reader
(`permission::model::lang::awk::scan`), which looked for the TEXT
`system(`. awk accepts whitespace between a builtin's name and its
parenthesis, so:

    awk 'BEGIN { system ("id") }'          Allow, safe_unattended
    awk 'BEGIN { system\t("id") }'         Allow, safe_unattended
    awk 'BEGIN { f="system"; @f("id") }'   Allow (gawk's indirect call)
    awk 'BEGIN { system("id") }'           Ask, Hard(Code) — the one spelling found

With the shipped defaults and no rule at all. In the foreground an Allow
runs without asking, on either backend (the API, or Claude Code's Bash
tool through the PreToolUse hook); for a `:child` or `:Send` job,
`safe_unattended` let it run with nobody watching. `id` stands for any
command: the program runs whatever the string names.

The tests certified the hole closed: `awk::tests::code_is_found` and the
acceptance cases for awk's `system()` all used the one spelling the
scanner matched — a spelling hypothesis, tested on the spelling it was
written from (`[[feedback_second_spelling]]`).

## Scope

- Every Shannon build since `410f0cb` (2026-10-09). Before it, awk was
  judged by the older engine, which treated it as an executor.
- Any session where the model writes an awk program: an eager model whose
  `bash -c` or `sh` asks, or a prompt injection in a file it read.
- Not reached: `awk -f prog.awk` (the program from a file is `exec_file`,
  asks since 10-13); gawk's long options (unknown to the model, so they
  ask).

## The fix

Fail closed on the language, not on a spelling (`awk::scan`, `has_word`):
the word `system` anywhere in the program is code, whatever follows it; any
`@` before a name is code (gawk's `@load`, `@include`, and `@f()`, an
indirect call that reaches `system` through a string). A program that only
mentions `system`, or `@name`, in a string asks — the safe side, as the
reader's own doc already accepted for `system(` in a string. The manual's
awk line says so.

## To validate

    cargo test -p shannon-core awk_running_a_command_asks_however_it_is_spelled
    cargo test -p shannon-core code_is_found_however_the_call_is_spelled
    cargo test -p shannon-core the_acceptance_suite_stands_where_it_says

The first is RED on `6d80f43` (`awk 'BEGIN { system ("id") }'` decided
Allow); its `@f` row builds the name from pieces, so only the `@` rule
answers it. The acceptance case `awk-system-spelled-with-a-space-asks` has
no rule. Mutants, each RED: the literal `system(` again; the `@` rule off;
the word boundary off (a `subsystem` would ask).

## The class

A danger list of spellings stands in for a property nobody computed
("can this program run a command") — the same shape as
`SECURITY-2026-09-29-repeated-flag-collapse.md` and
`SECURITY-2026-09-30-flags-the-rules-cannot-see.md`. sed's reader
(`permission/model/lang/sed.rs`) is not that shape: it parses the script
command by command (addresses, then the letter, then what the letter
takes) and calls any letter it does not know code. Read for this class on
2026-10-26, no spelling of `e`, `s///e`, `w` or `r` found that it passes —
by reading, not by a probe table (review stop 12 recommends one there
too).
