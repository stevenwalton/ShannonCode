# SECURITY — a relative rule reached everywhere; a subtree deny misses its root

**Status: #1 FIXED 2026-10-05 (the commit that adds this file). #2 FIXED
the same day by option (a), asking (Steven: "These all look good to me").**

**Found:** 2026-10-04/05, by the parent, while studying Steven's logs for
allow-rule proposals (#1), and while checking #1's fix for a deny it might
narrow (#2). Every row was reproduced through `RuleSet::decide_line` from a
start directory (an out-of-tree harness), and #1's rows are now tests.

## 1. HIGH (FIXED) — `./**` normalised to `**`, which reaches anywhere

`glob::matches` normalises both sides first (`//`, `/./`, `..`, a trailing
`/`). `normalize_path` drops a `.` segment, so `./**` became `**`, and a
leading `**` is deliberately unanchored ("`**/*.pem` is anywhere"). The `./`
the user wrote to say "from here" was gone before any wildcard was asked.

    rule               line                 got     want
    allow rm ./**      rm -rf /             Allow   Ask
    allow rm ./**      rm ~/.zshrc          Allow   Ask
    allow rm ./**      rm ../other/x        Allow   Ask

**Its neighbourhood, the same class — a wildcard reaching a path the rule
never named:**

    allow rm ./*       rm -rf ..            Allow   Ask   (the parent)
    allow rm ./*       rm -rf .             Allow   Ask   (the project)
    allow rm ?*        rm -rf ..            Allow   Ask
    allow rm .*        rm -rf ..            Allow   Ask
    allow rm */x       rm /x                Allow   Ask   (`*` matched nothing)

A shell's `*` never expands to `.` or `..` (bash 5.2's `globskipdots`; zsh
always), and normalised, `.` is the empty path — which `*` matched.

**The fix** (`glob::reaches`, asked before the wildcards): an absolute
pattern, and a leading `**` not written after `./`, reach anywhere, as
before. Any other relative pattern names files from where Shannon started:
never an absolute spelling (a file inside the start is still reached by its
relative spelling, which `file_spellings` always yields), never above it
unless its `..`s are written, and never the start itself unless the pattern
is empty. `allow rm ../sibling/*` still reaches `../sibling/x`; `**/x` still
reaches `../other/x`.

Pinned by `a_relative_pattern_stays_where_it_was_written` (`glob.rs`) and
`a_grant_written_from_here_covers_only_what_is_here` (`permission/tests.rs`).
Mutation-checked: 7 mutants, all RED.

**Its cost, stated — and understated at first.** `reaches` sits inside
`glob::matches`, so it narrows a DENY's patterns as well as an allow's.
This said only `deny rm ./**` vs `rm -rf .`; the review (Reviewer C) found
`deny rm ./*` vs `rm -rf .` went Deny → Allow, and `deny rm */x` vs `rm /x`
Deny → Allow (correctly: `/x` is not a relative pattern's file). #2's fix
asks of the first again; so "pre-existing, independent of #1" below was
false for patterns rooted at `.` until #2 landed.

**Review follow-up.** Normalising could also MAKE a leading `**` from what
was not written as one: `a/../**` is `**`, and `allow rm a/../**` reached
anywhere (Reviewer C). Only a WRITTEN leading `**` reaches anywhere now
(`reaches`' `anywhere`, read before normalising); `./`'s special case is
gone with it. A deny on a string flag value that normalises to nothing
(`--flag=.` under `deny foo --flag=*`) no longer fires (Reviewer A, LOW —
degenerate, docketed).

## 2. HIGH (FIXED, option (a)) — a deny on a subtree did not fire on the subtree's root

A deny or an ask is CONTACT: it fires when a path the stage touches matches.
`rm -rf /etc` touches `/etc`, which `/etc/**` does not match, so the deny
never fires — and the manual teaches carving a deny out of a broad allow.

    rules                              line            got
    allow rm * + deny rm /etc/**       rm -rf /etc/x   Deny
    allow rm * + deny rm /etc/**       rm -rf /etc     ALLOW
    allow rm * + deny rm /etc/**       rm -rf /        ALLOW
    allow rm * + deny rm build/**      rm -rf build    ALLOW
    allow rm * + deny rm build/**      rm -rf .        ALLOW
    deny :write /etc/**  (defaults)    rm -rf /etc     Ask (rm's default)

Pre-existing (independent of #1). With the shipped rules alone it asks,
because `rm` is destructive; it RUNS under a broad allow.

**The credential gate already answers this question, the other way:** its
`aimed_alike` compares literal prefixes both ways, so `ls -la
~/.local/state` is denied for holding Shannon's log. Two graders, one
question ("does this touch X"), two answers.

**Why it is not fixed here: a path's text does not say whether it is a
directory, nor whether the program recurses.** `rm build` without `-r`
cannot remove a directory; `rm -r build`, `mv build x`, `chmod -R` can.

Options:

- **(a) Root-or-above asks.** A deny/ask path pattern with a literal root
  (`/etc/**`, `build/**`, `~/.ssh/**`) fires as `Unseen` — it asks — when a
  touched path is at or above that root. Precise for those patterns, cheap,
  and fail-closed. A leading-`**` pattern (`deny rm **/*.pem`) has no root:
  every path could hold a match, so it stays a named residual.
- **(b) Model recursion.** Program models say which flags recurse (`rm -r`,
  `chmod -R`, `chown -R`, `cp -r`), plus what moves a tree (`mv`); only those
  meet a pattern at or above its root. Precise for `**` patterns too; more
  work, and an unmodelled program asks anyway.
- **(c) Deny or ask.** Under (a) or (b), should reaching a deny's root DENY,
  as the rule says, or ASK, as a guess about a directory should?

Recommendation: (a), asking, now — it closes the rows above with no new
model — and (b) when the program models next grow.

**The fix — (a), asking.** A deny's or an ask's contact
(`rule.rs::operand_match`), finding no path the pattern matches, asks
(`Unseen(Unaccounted::HoldsWhatItNames)`, shown as the rule that may hold)
when a touched path is at or above what the pattern can name:

- a glob: `glob::encloses` — its ROOT is the literal text up to the first
  wildcard cut back to a directory (all of it with no wildcard), joined to
  the start when relative, compared with the path's absolute spelling;
- a regex: `PathRe::encloses` — no root to read, so asked of what lies under
  the path (`could_name("<path>/**")`), after first asking of a directory
  nobody has: a pattern that could name something THERE has no root.

All of the table's rows now ask. Not noise: a path beside the root (`/et`,
`/tmp`, a file) is unchanged, and what is inside still denies outright.

**Named residuals**, pinned in `a_subtree_deny_asks_of_its_root_and_above`:
a pattern that reaches anywhere has no root (`**/*.pem`, `re:/\.ssh/`), so
`rm -rf build` under `deny rm **/*.pem` runs under `allow rm *`; a relative
regex has no spelling of the start itself (`rm -rf .` under `deny rm
re:^build/`). Option (b) closes the first.

Mutation-checked: 9 mutants, all RED.

**Rewritten the same day: segment by segment, not by literal root.** The
root — the literal text before the first wildcard — missed every directory
BETWEEN it and a wildcard segment: `rm -rf build` under `deny rm */x`
(`build/x`), `rm -rf /srv/prod` under `deny rm /srv/*/data/**`. Found
re-checking the review's F6 rows. `glob::encloses` now asks whether the
path's segments match the pattern's first segments one by one (a wildcard
segment matches any name; below a `**` segment anything may), which holds
the root and its ancestors as before and these too. Rows added to the
test; mutation-checked (a `**` segment, a wildcard segment): RED.
