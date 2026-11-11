# SECURITY — a variable's braces hid the path they write

**Status: FIXED 2026-10-05 (the commit that adds this file).**
`shell::param::spellings` reads what a word's `${…}` write;
`token_spellings` (`permission/mod.rs`) yields those spellings beside the
word, so the credential gate's literal check, its glob/regex check and a
deny's or an ask's contact read them together. An allow's containment does
not: it reads `file_spellings`, because a default is a file the word MAY
name, and an allow must cover the file it DOES. Pinned by
`a_word_written_inside_an_expansion_is_read`,
`a_word_past_the_spelling_bound_asks` and
`a_default_meets_a_deny_and_never_satisfies_an_allow`
(`permission/tests.rs`), and `shell::param`'s own tests.

**Found:** 2026-10-04, by the parent, while reading Steven's Shannon logs
for allow-rule proposals. Every row below was reproduced through
`RuleSet::decide_line`, with the shipped defaults plus Shannon's own state
as `register_own_state` registers it (an out-of-tree harness).

**Found in the wild, not constructed.** A Claude Code session
(claude-code/opus) ran

    ls -la ${XDG_STATE_HOME:-$HOME/.local/state}/shannon/log/ 2>&1 | tail -5

and its `ls` stage was `Allow DefaultRead`. A minute later the same model's
`fd … $HOME/.local/state` was a hard deny, because THAT spelling reached
Shannon's log directory. The XDG fallback is how a model writes a state
path, unprompted.

## The class

A parameter expansion can put text the COMMAND writes into the value:
`${X:-w}` is `w` whenever `X` is unset. The credential gate compared a
word's literal prefix (up to the first `$`) and literal suffix (after the
last `}`) — `aimed_alike` — and for `${X:-~/.ssh/id_rsa}` both are empty,
so the comparison never ran. That is not the residual `cat $X` keeps on
purpose (the VALUE of `X` is unknowable): the default is right there.

`:=` alone was caught, and by accident: `path_spellings` splits a token at
each `=` for `--file=x`, so `${X:=~/.ssh/k}` yielded `~/.ssh/k}`. With its
trailing brace that met a directory entry and no suffix entry —
`${X:=key.api}` read as `key.api}` and was allowed.

## Severity: HIGH

- It defeats `[claude_code] path_policy = "deny"`, the default, which
  exists because Claude Code's model sees a command's output before
  Shannon's redaction can.
- Shannon's own state is directory entries (the log, the state directory
  holding every project's conversations, `config.toml`, `api_key`), which
  this never met.
- No rule is needed: the shipped defaults.
- Foreground only: background work already held each word
  (`path_indeterminate` → `UnresolvedOperand`).

## Rows (`path_policy = "deny"`)

    line                                                     before  after
    cat ${X:-~/.ssh/id_rsa}                                  Allow   Deny
    cat ${X-~/.aws/credentials}    (also :+  +  :=  =)       Allow   Deny
    cat "${X:-$HOME/.ssh/id_rsa}"                            Allow   Deny
    cat ${X:-~}/.ssh/config                                  Allow   Deny
    cat ${NOPE:-.test_key.api}                               Allow   Deny
    cat ${NOPE:=x.api}                                       Allow   Deny
    head -c 4000 ${X:-~/.config/shannon/api_key}             Allow   Deny
    cat ${XDG_CONFIG_HOME:-$HOME/.config}/shannon/config.toml Allow  Deny
    ls -la ${XDG_STATE_HOME:-$HOME/.local/state}/shannon/log/ Allow  Deny
    cat ${X:-${Y:-~/.ssh/id_rsa}}                            Allow   Deny
    cat ${HOME%/}/.ssh/config  (also ${HOME#} :0 ^^ /x/x)    Allow   Deny
    cat ${HOME/%//.ssh/id_rsa}                               Allow   Deny
    cat ${X/*/~/.ssh/config}                                 Allow   Deny
    deny rm /etc/** :  rm ${X:-/etc/passwd}                  Ask     Deny

Unchanged, as they should be: `cat ${X:-README.md}`, `cat $X`, `cat ${X}`
(Allow); `cat ${X:?~/.ssh/id_rsa}` (Allow — `:?`'s word is an error
message, never the value); `allow rm build/**` does not cover `rm
${X:-build/a}`.

## The fix

What each `${…}` may read as, from what it writes
(`shell/param.rs`):

- `${N:-w}` `${N-w}` `${N:=w}` `${N=w}` `${N:+w}` `${N+w}` → `w`;
- `${N/pat/rep}` (and `//`, `/#`, `/%`) → `rep`, `${N}rep`, `rep${N}`;
- every operator → `${N}`, a value made from `N`'s — which the gate
  already reads as home for `HOME` (`literal_prefix`).

One expansion at a time, the others as written: an expansion left as
written reads as "anything" to the glob check, which compares prefixes and
suffixes both ways, so the count stays linear. A word written inside an
expansion is spelled in turn (nesting). Bounded at 16 spellings or `${`s
— the words are the model's and are matched on the UI thread — and a word
past it is undecided: the gate asks (`CredentialUndecided`), never reads
it as harmless.

**Mutation-checked:** 11 mutants, all RED — the spellings dropped from
`token_spellings`, the undecided check, the `${N}` alternative, the
substitution arm, nesting, the budget, an unclosed brace, `:?` read as a
value, `${#N}` read as a variable, and the upfront `${` count (a stack
overflow on 100 000 nested braces). A twelfth found dead code: stripping
quotes inside a default — the lexer already removes them inside braces as
outside, which a test row now pins (`${X:-~/.s'sh'/config}`).

## Review follow-up (Reviewers A and C, the same day)

The fix was incomplete in three ways, each a bypass of the boundary it
drew, under `path_policy = "deny"`:

- **"One expansion at a time" was false.** Its argument was that an
  expansion left as written reads as "anything" to the glob check; but
  `aimed_alike` needs a literal on the side a PATTERN is anchored, so an
  unread `${A…}` leading the word aimed it at nothing: `cat
  ${A:-}${B:-~/.ssh/config}` was allowed (A). Every combination of
  readings is spelled now, bounded at 16 (four defaults in one word; a
  fifth asks).
- **`#` and `!` are parameters as well as prefixes**: `${#:+$HOME/.ssh/
  id_rsa}`, `${#+…}`, `${#/*/…}`, `${!:-…}` swallowed their word (A).
  `name_len` named every special parameter; two shortcuts had taken two
  back.
- **The glob check's prefix was never normalised**: `$HOME//.ssh/config`,
  `${HOME}/./.ssh/config`, and this report's own idiom with a trailing `/`
  in its default (`${XDG_STATE_HOME:-$HOME/.local/state/}/shannon/log/`)
  were allowed while `~//.ssh/config` was denied (C). `literal_prefix`
  normalises its whole directories — not the start of a name: `.` in `cat
  .*` is `.env`'s first character. And `${HOME[0]}` reads as `${HOME}`.

Rows for each are in `a_word_written_inside_an_expansion_is_read`;
mutation-checked (combinations, `#`, `!`, subscript, normalising, the name's
start): RED.

**Open, Steven's call (Reviewer C, F5):** a default the command only MAY
use denies and names the entry — `:+`'s word is used only when the variable
is SET — while a `re:` entry's "could" only asks, and the manual says the
gate "never denies on a guess". Options: route `:+`/`+` and replacement
spellings to "could" (ask), all default spellings to "could", or say in the
manual that a written default is not a guess.

## Residuals

- `$X` and `${!X}`: the value is unknowable here. Pinned in
  `the_named_residuals_of_the_credential_aim_check`.
- A read after a `cd` that cannot be followed, `cd $D && cat config.json`
  (TODO, review 20 (E)): unchanged, still Steven's call. A WRITTEN default
  in the `cd` (`cd ${X:-~/.ssh}`) is denied at the `cd` itself.
