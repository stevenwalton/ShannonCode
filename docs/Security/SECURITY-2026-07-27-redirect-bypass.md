# SECURITY — an `allow` grant did not contain the files a redirect touches

**Severity: MEDIUM** (revised down from an initial CRITICAL — see "What the
first report got wrong"). Found 2026-07-27 by a whole-surface sweep of
`shannon-core`; diagnosis corrected by Steven; **FIXED the same day.**

## The issue in one line

An `allow` rule is *containment* — "this program, touching only these paths" —
and the containment check graded `command.operands`, which never includes the
file a redirect touches, in either direction.

```
rules: allow cat /tmp/*
cat /tmp/a > ~/.zshrc      →  Allow   (before)   →  Ask   (after)
```

`Allow` is the unattended path: the gate never opens.

## What the first report got wrong

Two things, both worth recording because both were overstatements that a
reader would have acted on.

**1. The headline example needed root.** The original report led with
`cat /tmp/a > /etc/passwd`. `/etc/passwd` is `root:wheel 0644`; that command
dies with `EACCES` before writing anything, so it was never the risk. Steven:
*"Isn't that already handled by user permissions? … So requires privilege
escalation."* Correct. The real blast radius is **anything the user can write**
— `~/.zshrc`, `~/.bashrc`, project sources — which for an agent running as the
user is the whole interesting set, but is not a root compromise. `> ~/.zshrc`
is the sharp one: arbitrary code on the next shell.

**2. It was never exploitable under the shipped default policy.** With
`RuleSet::defaults()` and no user rules, every redirect above returns **Ask**,
with cause `DefaultDanger(Mutating)`. The bypass requires the user to have
written an `allow` rule — which is exactly the documented way to reduce prompt
fatigue, so it is a real hazard on the rung we encourage, but "out of the box"
was safe the whole time. Calling it CRITICAL was wrong.

## What Steven's reframing found that the reviewer's did not

The reviewer (and I) diagnosed this as "the rule engine cannot see redirect
targets, because `matched_by` takes `&Command` and redirects live on `Stage`".
True, but it invited the wrong fix. Steven:

> *"I'm not convinced that `cat` is the problem. It is the redirect that is
> allowing the write operation. Blaming `cat` seems to be pointing to the wrong
> culprit. The redirect allows many other commands to be problems, not just
> `cat`. `>` is a write command isn't it?"*

Chasing that turned up something both of us had missed: **the effect model was
already right.** `Stage::effect()` (`shell/pipeline.rs:131`) joins
`Effect::Mutating` whenever a Write/Append redirect is present, it is tested
(`redirection_is_an_effect_and_keeps_its_target`), and the module doc has said
so all along. My "the effect is ReadOnly" probe read `command.effect` — the
*program's* effect — not the stage's. So there was never anything to fix there,
and a fix aimed at the effect lattice would have been wasted work.

The actual gap is narrow and follows from a deliberate design choice: an
explicit `allow` is honoured *over* effect (`permission/mod.rs:177` — "the
user's call — honor it", with disturbed bindings the sole exception). Once the
user has granted, **containment is the only thing left**, and containment was
looking at the wrong set of paths.

## The fix

One concept: *every path the stage touches* is what a rule reasons about.

- `permission/rule.rs` — new `contained_paths(stage)`: operands **plus every
  redirect target, in both directions**. A grant says which files a command may
  touch, and a redirect names a file whichever way the arrow points: `> b` takes
  the write, `< b` gives the read.

  **The first version of this fix filtered to write/append**, and I wrote a test
  asserting that `cat < /etc/hosts` under `allow cat /tmp/*` was correct,
  justified as "a read is the program's own effect to answer for". Steven caught
  it in review — *"Did you fix both directions of the effect? Read and writes?"*
  — and the justification is the trap documented above: an explicit allow is
  honoured **over** effect, so nothing answers for it. Reading is also the
  exfiltration direction, which is why the credential override exists at all.

  Honest scope of the read half: for a `ReadOnly` program the command still runs
  via `DefaultRead` (reads running unattended is shipped doctrine, not a hole
  here). What changes is that the rule no longer *claims* to cover it — so
  `:analyze` stops attributing it to a grant — plus the deny/ask direction
  (`deny * ~/.ssh/**` now fires on `cat < ~/.ssh/id_rsa`) and any program whose
  own effect is not `ReadOnly`.

  Flag values remain excluded, unlike `stage_paths`: a flag value is often not a
  path (`git commit -m "msg"`) and requiring containment of a message would
  refuse grants users reasonably expect. That leaves a gap for flags that DO
  name files (`curl --netrc-file`), docketed rather than silently accepted.
- `Matcher::matched_by` and `operand_match` now take `&Stage`, the widest
  object they reason about (AGENTS.md §"Rules That Stay Fixed").
- **Both quantifiers read the same set.** The first pass fixed only the `allow`
  (containment) branch; the new `deny` test caught that the `Ask`/`Deny`
  (contact) branch still graded `command.operands`, so `deny * ~/.ssh/**` did
  not fire on `echo k > ~/.ssh/authorized_keys`. Fixing one and not the other
  would have been this sweep's own bug, one layer in.

## Tests

`permission/tests.rs`:
- `an_allow_grant_does_not_cover_the_file_a_redirect_writes` — in-scope
  redirect still allowed; out-of-scope truncate, append, and relative target
  all refused.
- `a_deny_fires_on_the_file_a_redirect_writes` — the contact half.
- `a_grant_does_not_cover_the_file_an_input_redirect_reads` — the read half,
  asserted on the *cause* (no longer a `Rule` match), which is the honest claim.
- `a_deny_fires_on_the_file_an_input_redirect_reads` — the contact direction,
  where the read half changes the outcome.

Mutation-checked: dropping the redirect chain from `contained_paths` fails the
allow test *and* the deny test.

## Still open, from the same sweep — and they are the SAME FORM as this

> **Since closed (re-checked 2026-11-05, on today's engine, the lines below
> decided again):** `cat /tmp/../etc/hosts` and `cat /tmp/a > /tmp/../…/.zshrc`
> under `allow cat /tmp/**` now ASK — a `..` is resolved before a path is
> placed, and tests hold `..` escapes as asks (`permission/tests.rs`); `read
> CDPATH < evil; cd ssh && cat x` under `allow cat *` asks. `find .
> -newdangerousaction rm` under `allow find *` still allows, by the design
> that replaced this ledger (a user's `*` covers what no model names; the
> dangerous flags are shipped carve-outs — `-exec`, `-execdir`, `-delete`,
> `-fprint` all ask under `allow find *`, checked the same day). What
> follows is the record as written on 07-27.

Tracing this bug produced a reusable question: **an explicit `allow` skips the
whole `default_decision` path, so for each signal that lives there, is it
re-checked, or does something else cover it, or is it simply gone?** Nobody had
written that ledger down. It is the answer to "why did four review passes miss
this": every pass asked *is rule X applied at all its sites* — and
`capability_unknown` **is**; its site list is complete. The defect is that the
outcome path does not consult it, which is only visible tracing the outcome
*backwards* through the override.

| signal in `default_decision` | skipped by `allow`? | still covered by |
|---|---|---|
| effect (incl. redirect -> `Mutating`) | yes, by design | containment — **fixed here** |
| `DisturbedBindings` | yes | re-checked inline ✓ |
| `UnknownFlag` / `UnknownProgram` | yes | **nothing** |
| `PathEscape` (`..`) | yes | **nothing** |
| unresolved operand | yes | the rest-wildcard rule (documented) |
| sensitive path | no — runs after the match ✓ | ✓ |
| secret handle | no — runs after the match ✓ | ✓ |

Verified live:

```
allow find *        ->  find . -newdangerousaction rm            Allow
allow cat /tmp/**   ->  cat /tmp/../etc/hosts                    Allow
allow cat /tmp/**   ->  cat /tmp/a > /tmp/../home/sam/.zshrc     Allow
```

The third one **partially defeats this fix**: containment is a glob match, and
`/tmp/**` matches that string textually, so `..` walks out of the grant and
writes. `PathEscape` is exactly the missing cover.

Recommended: hoist `UnknownFlag` and `PathEscape` next to the sensitive-path
and secret-handle overrides — after the winner match, so they cover rule-allow
and default-allow alike, which is where the two *working* overrides already
live. `UnknownProgram` is a judgement call for Steven: `allow frobnicate *`
names a program we have no model for, which is arguably opting in knowingly.

Also still open: `sets_cdpath` knows 4 var-writers where `writes_hazard_var`
knows 10, so `read CDPATH < evil` poisons neither the bindings nor the cwd axis.

All docketed in `TODO.md`.
