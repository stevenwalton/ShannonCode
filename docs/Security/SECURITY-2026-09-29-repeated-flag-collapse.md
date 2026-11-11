# SECURITY — a repeated flag keeps only its last value, so the gate reads the wrong one

**Status: FIXED 2026-09-30** (found 2026-09-29 by the demo-replay reviewer,
reproduced independently the same day). See "Fix" below.
**Severity: MEDIUM** — revised UP from "LOW today" on 2026-09-30, when a wider
probe showed the class already reached a key file's CONTENTS and a user's deny
rules, not only `find` listing names. It needs a broad grant the user wrote
(`allow tar:*`), which is exactly the rung the credential override exists to
hold under:

    allow tar:*                    tar -x --file=~/.ssh/id_rsa --file=a.tar  -> Allow
    allow tar:*                    tar -x --file=~/.ssh/id_rsa               -> Ask
    allow tar:* + deny * /etc/**   tar -c --file=/etc/x --file=a.tar y      -> Allow
    allow tar:* + deny tar --file=/etc/*   (same line)                      -> Allow

The original estimate held for the DEFAULT policy (no user grant): there the
only reachable case was `find` listing names.

## The issue

`Command.flags` is a `BTreeMap` (`shell/mod.rs:216`), filled at
`shell/normalize.rs:259`. When a command repeats a flag, the insert
**replaces** the earlier value: only the LAST occurrence survives. Everything
that reasons about flag VALUES reads that map, so it sees one value where the
command has several:

- the sensitive-path gate and `path_indeterminate`, through `stage_paths`
  (`permission/mod.rs:504`);
- the secret-handle gate (a handle in an earlier occurrence is not seen, yet
  it is still substituted at execution);
- rule value predicates (`permission/rule.rs:100`).

## Verified (probe, 2026-09-29)

`RuleSet::defaults().decide_line(...)`:

    find . -name '*.pem'                 -> Ask     (can reach credential files)
    find . -name '*.pem' -name '*'       -> Allow   (the SAME files: -name ANDs)
    find . -name '*' -name '*.pem'       -> Ask     (order-dependent)

`find . -name '*.pem' -o -name x` is caught today only by an unrelated false
alarm (`-o` is reported as an unrecognised flag). Fixing that false alarm
first would open this.

## Scope

- **Trigger:** any modelled command with a value-taking flag given twice.
- **Today:** `find` with repeated `-name`/`-path` — lists names, not contents.
- **Latent:** any future model whose flag values are paths or secrets
  (`curl -H`, `--data`, `-o`), and any user rule matching a flag value.

## Fix

`Command.flags` is a `Vec<Flag>` — every occurrence, in the order written
(`shell/mod.rs`). Order-freeness moved from the storage to the questions:
`has_flag`, `flag_values` (ALL values) and `flag_names` (the set). The
last-wins accessor `flag_value` is gone, so no check can pick one value again.

Every reader, by consumer (`rg '\.flags\b|flag_values|has_flag'`, 5 sites):

- `stage_paths` (`permission/mod.rs`) — the credential gate, the handle gate
  (`stage_tokens`), `path_indeterminate` and deny-side path contact all read
  it, so they see every value together;
- `has_unresolved_operand` (`shell/mod.rs`) — every value;
- `elevated_effect` (`normalize.rs`) and `writes_hazard_var` (`pipeline.rs`)
  — every flag;
- rule value predicates (`permission/rule.rs`, `flag_value_match`) — now a
  QUANTIFIER, and the two verbs need opposite ones, as operand patterns
  already do: `deny`/`ask` fire on ANY value, `allow` covers the command only
  if EVERY value matches (and at least one was given). Steven's "a single
  instance matches" is the deny side; applying it to `allow` would be the
  fail-open direction (`allow tar --file=*.tar` covering a second
  `--file=~/.ssh/id_rsa`).

Landed before `find`'s `-o`/`-a`/`-not` are modelled (class C), as required.

## Same class, found by the same probe — fixed in the follow-up commit

"A flag the command carries is not seen by the rule that names it" had
four more instances, all pre-existing:

- A rule naming a flag of an **unmodelled** program (`deny curl -k`) could
  not fire: an unmodelled program's words are all `operands`, and a literal
  flag was looked for only in `unknown`. `allow curl:*` + `deny curl -k` ran
  `curl -k` — "deny wins" was false for every program without a model.
- A literal flag's VALUE was never read (`deny kubectl --context=prod*`,
  and `glob.rs`'s own example `deny curl --url=*://*.*.*.*`, parsed and were
  inert), and a literal flag was missed as `--frob=1`.
- A modeled program refuses to split a cluster holding a letter it does not
  know (`rm -rfz`), and that hid the letters it DOES know from every rule.
- A `*` rule names no program, so its flag stayed literal and `deny * -r`
  never met `rm -r`.

Fix: `Command.unmodeled_flags` — every dashed word no model named, by its
spelling, with the value it may carry (inline, else the next word, as
getopt reads a value-taking flag). The normalizer fills it for modelled and
unmodelled programs alike, because only it still has argv adjacency; it adds
no word the other fields lack, so containment grades the same words. Rules
read literal flags through the same quantifier code as canonical ones. For
`deny`/`ask` only, an unsplit short cluster also counts as the shorts it
would bundle (`-sk` → `-k`; resolved through the model for a refused
cluster) — whether an unmodelled program bundles is unknowable, which makes
it the safe reading for contact and the unsafe one for `allow`. A rule's
literal flag is read through each command's model at match time (`read_by`,
over `program::canonical_of` — the resolver parse time uses too), so a `*`
rule meets every spelling a program's model gives its flag.

Still open (TODO.md): a refused cluster's known letters do not raise the
command's effect (latent: no modelled bundling program has an elevating
short flag today).

## How to validate

`cargo test -p shannon-core repeated_flag` and `find_action` — the regression
tests put the value that matters FIRST (the order that was hidden) and in its
mirror position:

- `a_repeated_flag_keeps_every_value_in_order` (normalizer)
- `a_repeated_flag_hides_no_value_from_the_credential_gate`
- `a_repeated_flag_cannot_walk_past_a_deny`
- `an_allow_on_a_flag_value_covers_every_value`
- `a_find_action_is_denied_wherever_and_however_often_it_appears`
- follow-up: `a_deny_on_an_unmodeled_programs_flag_fires`,
  `a_literal_flag_is_seen_in_its_value_spelling`,
  `a_literal_flag_value_rule_reads_every_value`,
  `a_known_letter_in_a_refused_cluster_meets_the_deny`,
  `a_star_rules_flag_is_read_by_each_commands_model`,
  `an_allow_requiring_a_literal_flag_takes_it_only_as_written`,
  `unmodeled_flags_keep_their_spelling_and_the_word_after`

The three original probe lines through `RuleSet::defaults().decide_line` are
all `Ask`.
