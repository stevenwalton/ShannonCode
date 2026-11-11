# SECURITY — a brace group was closed at its first `}`

**Status: FIXED 2026-10-06 (the commit that adds this file). Live on `main`
(pushed): the flag-as-operand shapes since `ff4315a` (2026-10-06, "a broad
allow of yours answers the shipped asks"); the flag-in-a-value shape since
at least `803bf76` (verified there), likely since `splits_unquoted` was
written for SECURITY-2026-09-30 R2-3 (inferred, not bisected).**

**Found:** 2026-10-06, review round 27, Reviewer B (robustness), probing
`ff4315a`'s `shell::opens_brace` with bash 3.2. Reproduced through
`RuleSet::decide_line` under `RuleSet::defaults()` — no rule of the user's —
on `1d2d638` and, for the older shape, on `803bf76`.

## CRITICAL — `find . {x{}y,-delete}` ran unasked

Bash closes a brace group at its MATCHING `}` — counted by depth, past a
quoted or escaped one — and expands it if the group holds a `,` or `..`:

    $ printf '<%s>' {x{}y,W} {{},W} {x'}'y,W} {x\}y,W}
    <x{}y><W><{}><W><x}y><W><x}y><W>

`opens_brace` (`ff4315a`, written so `find -exec rm {} \;`'s `{}` is no
expansion) took the group to the FIRST `}` instead, so each of those words
read as one literal operand. A literal operand cannot be a hidden flag, so:

    shipped rules only                       803bf76   1d2d638   fixed
    find . {x{}y,-delete}                    Ask       ALLOW     Ask
    sort {x{}y,--compress-program=./evil} f  Ask       ALLOW     Ask
    rg {x{}y,--pre=./evil.sh} pat            Ask       ALLOW     Ask
    cat {x{}y,/etc/passwd}                   Ask       ALLOW     Ask
    find . -name {x{}y,-delete}              ALLOW     ALLOW     Ask
    find . -name {x'}'y,-delete}             ALLOW     ALLOW     Ask

`find`, `sort` and `rg` are read tools, allowed unasked inside the start;
the brace hid the flag that makes each delete, run a program, or (for
`cat`) read outside it — and `~/.ssh/id_rsa` the same way (Reviewer B).

**Reach:** the default policy. The model must write the shape; an eager
optimiser that has met a refusal for `-delete` has every reason to.

## The older half — a flag inside a value

`splits_unquoted` asks whether a word can split into several, one a flag
(`find . -name {x,-delete}` is `find . -name x -delete`). It read a brace
group with `take_while(|c| c != '}')` — the same first-`}` reading — before
`opens_brace` existed, so `-name {x{}y,-delete}` was a literal name, ALLOW
on `803bf76`. `ff4315a` folded that reading into `opens_brace` and gave it
to the three other predicates, which had read every `{` as an expansion.

## The fix — the one reading, read wide

`opens_brace(text, at)` now opens on a `,` or `..` ANYWHERE after the `{` in
the word. Every group bash expands holds one before its end, which is before
the word's, so the reading is a superset of bash's: it can ask more, never
less. A reading by depth would be exact on the source, but two of the four
callers (`expands`, `could_become_a_flag`) see quote-removed text, where
`{x'}'y,W}` is `{x}y,W}` and depth closes it early — so one wide reading
serves all four. What it costs: a literal `{}` with a list after it in the
same word (`{}{x,y}`) may be a flag where bash says not; an extra ask.
`find -exec rm {} \;` and `fd`'s `{.}`, `{/}`, `{//}` stay literal.

Tests: `a_brace_expands_only_with_a_list_or_a_range` (the four shapes, in
all four predicates) and `a_placeholder_brace_is_no_hidden_flag` (the six
lines above and the other three shapes, each `Ask` under
`RuleSet::defaults()`). The old reading, restored as a mutant, turns both
RED. Every cell of the table was run, not inferred: `803bf76` and
`1d2d638` through a probe test in a scratch worktree, the fixed column by
the test.

## To validate

    cargo test -p shannon-core a_placeholder_brace_is_no_hidden_flag

or, against any build: `RuleSet::defaults().decide_line("find . {x{}y,-delete}")`
→ not `Allow`.

## Lesson

The `{}` fix was a precision change to a soundness boundary, and it was
tested from the precise side only: every new test asked "is `{}` literal
now", none "is everything bash expands still an expansion". A scanner that
narrows what counts as an expansion needs the adversarial half — the
shapes that make bash's parse differ from ours (nesting, quotes, escapes) —
in the same commit. Recorded in memory as a gate failure class.
