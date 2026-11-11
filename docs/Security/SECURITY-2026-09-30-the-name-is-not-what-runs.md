# SECURITY — the name a rule or model trusts is not the thing that runs

**Status: #1–#3 FIXED 2026-09-30 (the commit that adds this file); #4
docketed.** One predicate, `name_not_what_runs` (`permission/mod.rs`), is
consulted by both doors a stage can be allowed through — the allow arm and
`capability_unknown` for the default — and `ENV_HAZARDS` gained git's
family. Pinned by `a_program_named_by_a_path_never_inherits_the_names_trust`
(every modeled program, `./` and `/tmp/`, by default and under an allow) and
`a_variable_that_changes_what_runs_overrides_any_allow` (every hazard,
prefixed and through `env`); eight mutations, all red. **Steven decided
(2026-09-30):** an explicit path never keeps a program's trust, system
directories included — `/usr/bin/ls` asks ("would make it too complicated");
judging a hook variable's VALUE as a program (`GIT_PAGER=cat git log`,
which asks under `allow git log *`) is docketed for later — "a very good
case", after what is most used. Not done here: folding every post-rule override into one
ordered list (Reviewer D MED-1) — the class is one predicate now, the
family of overrides is still five arms.
**Found:** in the round-16 review (`b2c4db7..0bf33fd`): #2 by Reviewer D;
#1, #3 and #4 by Reviewer E tracing #2's class. Every decision below was
reproduced through `RuleSet::decide_line` by the reviewer named (temporary
probe tests, since removed); what the parent verified is marked.

**The class.** A permission decision keys on the program NAME and its argv.
Anything that makes the name run a different file, or load different code,
while the name and argv stay the same, walks past it. The engine already
has one member of this family handled — disturbed bindings (`alias ls=rm;
ls`, `export PATH=.; ls`) override any allow to Ask — and the others were
never put beside it.

**Introduced vs pre-existing.** #1 and #3's list gap predate this round
and are live on a stock install. #2 is new: before `a3fa1a9` no
user-written allow existed (the shipped defaults contain no `allow` rule,
and the old exact-text grants could not match a prefixed line). This round
made all four worse: `A` now proposes allow rules (`58431c9`), one
keystroke and Enter away.

## 1. HIGH (pre-existing, no rule needed) — `./ls docs` runs whatever `./ls` is, unprompted

The program word is looked up by BASENAME: `shell/program.rs:103-106`
(`lookup`), `shell/mod.rs:465-467` (`is_modeled`), `permission/rule.rs:416-428`
(`Matcher::targets`). So `./ls`, `/tmp/ls` and `/bin/ls` all get `ls`'s
model, which is ReadOnly — but the file at `./ls` or `/tmp/ls` is whatever
is on disk there.

    policy              line             got     want
    defaults (no rule)  ./ls docs        Allow   Ask
    defaults (no rule)  /tmp/ls docs     Allow   Ask
    allow ls docs *     ./ls docs        Allow   Ask
    allow ls docs *     /tmp/ls docs     Allow   Ask

The default lands at `permission/mod.rs:519-522` (`ReadOnly → Allow`)
because `capability_unknown` (`:719-738`) sees a modeled basename; under a
rule, at the allow arm `:307-324`. `safe_unattended` (`:963-1002`) is TRUE,
so a background `:child` or `:Send` job runs it with no pane and no key.
Parent-verified: `lookup` is byte-identical at `b2c4db7^`.

**Preconditions:** a file named like a modeled read-only program (`ls cat
head tail grep find wc …`) in the repo, `/tmp` or anywhere the session
wrote; the model names it by path.

**Fix:** a program word containing `/` does not inherit a modeled
program's trust for a GRANT: capability-unknown on the default path, and an
override to Ask after any allow. Keep basename matching for `deny`/`ask` —
`deny rm` SHOULD catch `/bin/rm` and `./rm`. Basename is safe for refusal
and unsafe for grant. **Decision for Steven:** whether a trusted system
directory (`/bin/ls`, `/usr/bin/ls`) is exempt; failing closed (ask) until
then. Same decision as SECURITY-2026-09-30-flags #10 ("a rule names a
program by path"); resolve them together.

## 2. HIGH (introduced `a3fa1a9`) — `PATH=. ls docs` runs `./ls` under `allow ls docs *`

The allow arm (`permission/mod.rs:307-324`) overrides an allow only for
disturbed bindings and `refusal_unruled`; it never reads the stage's own
environment. A prefixed stage's bindings stay `Trusted`
(`shell/pipeline.rs:1308-1316`): the prefix is "handled by its effect"
(`pipeline.rs:705-709`), and an allow rule outranks the effect. The `env`
wrapper folds its `NAME=VALUE` into the same `command.env`.

    policy             line                                     got     want
    allow ls docs *    PATH=. ls docs                           Allow   Ask
    allow ls docs *    env PATH=. ls docs                       Allow   Ask
    allow ls docs *    LD_PRELOAD=/x.so ls docs                 Allow   Ask
    allow cat *        BASH_ENV=./x.sh cat README               Allow   Ask
    allow cat *        DYLD_INSERT_LIBRARIES=./x.dylib cat R    Allow   Ask
    defaults (no rule) any of the above                         Ask     Ask

Parent-verified that the prefix changes what runs: in a directory holding
an executable `./ls` stand-in that only prints a marker, `bash -c 'PATH=.
ls docs'` and `bash -c 'env PATH=. ls docs'` both ran the stand-in.
Settled by E as NOT holes: `export PATH=.; ls docs` (Ask: disturbed
bindings), `alias ls=rm; ls docs` (Ask), `builtin ls` (Ask), `env -S …`
(Ask), `env -i ls`, `command ls`, `exec ls` (Allow, but they run the real
`ls`).

`A` on `PATH=/tmp ls docs` proposes `ls docs *` — the prefix is not in
`command.operands` — so the user is invited to write the rule while looking
at the prefix.

**Fix:** an `env_hazard` name (`shell/program.rs:1507`) in the stage's own
`command.env` means the allow does not describe what runs: Ask.

## 3. MEDIUM/HIGH (list gap, pre-existing) — `env_hazard` misses `GIT_CONFIG*`

`ENV_HAZARDS` (`shell/program.rs:1517-1683`) lists `GIT_SSH_COMMAND`,
`GIT_EXTERNAL_DIFF`, `GIT_PAGER`, `GIT_EDITOR` — it knows git runs code
from its environment — but not `GIT_CONFIG`, `GIT_CONFIG_GLOBAL`,
`GIT_CONFIG_SYSTEM`, or `GIT_CONFIG_COUNT`/`_KEY_n`/`_VALUE_n`, which set
all of those (and `core.fsmonitor`, `alias.*`, diff/filter drivers) from a
file or the environment. Parent-verified by reading the table.

    policy        line                                           got     want
    allow git *   export GIT_CONFIG_GLOBAL=./e; git status       Allow   Ask
    allow git *   GIT_CONFIG_GLOBAL=./e git status               Allow   Ask

The first is live today (bindings are disturbed only by listed names,
`pipeline.rs:710-717`, `:765-806`); the second would survive #2's fix,
which consults the same list. **Fix:** add the `GIT_CONFIG*` family (a
prefix entry covers the `_COUNT`/`_KEY_n`/`_VALUE_n` trio), effect
Executor. The list being curated is the deeper problem
(`[[feedback_enumeration_means_unmodeled]]`); a family, once one member is
known, is enumerated whole.

## 4. LOW/MEDIUM (docket) — a broad allow on a config-reading tool grants config-as-code

`allow git *` plus a checked-out repo whose `.git/config` sets
`core.pager`, `core.fsmonitor`, `alias.*` or a diff/filter driver: `git
log`/`status`/`diff` runs that code with no prompt. The engine cannot read
the repo's config. Not fixable here; surface it where the rule is written
(`:rules`, the `A` legend, the manual's teaching chapter) and docket it.

## The fix, as one change

Reviewer D (MED-1) found the post-rule overrides spread over separate arms
of `decide_stage` with repeated rationale (DisturbedBindings `mod.rs:314`,
`refusal_unruled` `:319`/`:329`, SensitivePath `:350`, Privileged `:402`,
SecretHandle `:425`; `Verdict::may_be_remembered` re-lists three). #1 and
#2 are two more members. One ordered `overrides(stage)` applied after the
rule/default decision carries all of them, so the next member is one
entry, not one more arm. `proposed_allow` runs after it: it proposes only
a rule that, added to the policy, decides the stage Allow.

**Regression tests:** every `env_hazard` name × {prefix, `env` wrapper} ×
{`allow X *`, `allow X <operand>`} is never Allow; `./X`, `/tmp/X` for
each modeled ReadOnly program is never Allow by default or under `allow X
*`, while `deny X` still refuses `./X`; the two `GIT_CONFIG` lines Ask.
Mutation-check by deleting each override entry.

**How to validate:** `RuleSet::decide_line` on each table above.
