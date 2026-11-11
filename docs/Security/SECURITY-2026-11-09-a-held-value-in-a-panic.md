# SECURITY — a held secret behind a multibyte character crashes Shannon, and the crash prints it

**Status: FIXED 2026-11-09, in review stop 17 (`fbffc8b`). Opened 2026-11-09:
introduced in `b8b36c4` (2026-11-08), present on `origin/main` (private) until
that fix is pushed.**

**Found:** 2026-11-09, review stop 17, Reviewer B, finding F1 — measured at
2ddb5d0 with probe tests (`docs/Reviews/2026-11-08-stop17/B-robustness.md`).
Traced again by Claude in the main tree.

## HIGH — the process dies, and the value you marked secret goes to the log and the screen

`credential_words` (`permission/mod.rs`) asks `Aliases::secret_ranges` where a
held secret sits in a command's word, and sliced the word with the answer:

    aliases.secret_ranges(token).iter().any(|&(from, to)| !token[from..to].starts_with("{{"))

`secret_ranges` documents and returns **character** offsets. As bytes, any
multibyte character ahead of the value lands inside a character, and the slice
panics. Its message holds the word:

    byte index 1 is not a char boundary; it is inside 'é' (bytes 0..2) of `éhunter2beornot2be`

**What is lost.** The panic hook scrubs with the default policy and no vault,
on purpose (it must take no lock the panicking thread could hold), so a held
value with no credential's shape is written in clear to the log and, through
the default hook, to stderr over the screen. Shannon exits.

**Reach.** With a secret held:
- a typed rule, with the value or its handle: `:allow rm é{{name.xxxx}}`;
- any asking command holding the raw value behind a wide character, since each
  decision's report builds `A`'s proposal through this code. A model given a
  handle stops earlier, at the hard ask; a raw value in a call is what the
  Claude Code provider can produce (inferred from `DecisionPage::leaked`, not
  run).

Two wide characters ahead of the value land on a boundary and read the wrong
text instead: a handle taken for a value (inferred, from the same arithmetic).

## The fix

The offsets are read as characters, and no byte index is taken:

    .any(|&(from, _)| !token.chars().skip(from).take(2).eq("{{".chars()))

Regressions: `a_held_value_behind_a_multibyte_character_is_found` (core, the
definition and `A`'s proposal) and the wide-character case in
`a_typed_rule_holding_a_credential_is_refused` (the app). Reverting to the byte
slice turns the first red.

## Validate

    cargo test -p shannon-core --lib a_held_value_behind
    cargo test -p shannon-tui --lib a_typed_rule_holding_a_credential

## To remember

An offset's unit is part of its type, and a `usize` does not say it. Every
other reader of `secret_ranges` (`view.rs`, two sites) used characters, which
is why this one read as right. Counted: 3 production readers, all correct now.
The hook's limit stands as designed; the defence is not panicking on what a
model or a user can type, which the proptest over the tutor's marks now also
asks of that reader.
