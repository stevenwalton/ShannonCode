PROVENANCE-AWARE COMMAND COLOURING                          provenance-coloring
================================================================================

Design doc for context-sensitive danger colouring in `:analyze` (and the live
permission popup it will feed). Grounds the [analyze diagnostics] work's next
step. Status: DESIGN — not yet built. Written 2026-07-21 after a research spike.


1. THE PROBLEM                                              *prov-problem*
--------------------------------------------------------------------------------

Today `:analyze` colours each command by its effect *in isolation*. Given

>       while read -r FOO; do rm -r "$FOO"; done < <(find . -type f)

it paints `find . -type f` GREEN (read-only, safe). But `find` is not safe
*here*: it is the SOURCE of the value bound to FOO, which flows into the
destructive `rm -r "$FOO"`. `find` DECIDES WHAT GETS DELETED. To know what the
`rm` will do, you must look at the `find`. Green tells the user "don't bother
looking" — the opposite of the truth.

The same `find` standalone (`find . -type f` on its own) genuinely IS green. So
the colour must be CONTEXT-SENSITIVE: the same command, a different colour by its
ROLE in the line. v1 is context-free (each stage classified alone) and so cannot
express this. Closing that gap is the crux of the whole permission thesis:
precise, honest communication is what lets a human verify without fatigue — the
alternative to "just trust the agent."


2. THREE AXES (v1 collapses all three to the first)        *prov-axes*
--------------------------------------------------------------------------------

    1. EFFECT       what a command does in isolation.  find=read, rm=destroy.
                    v1 colours on this alone.
    2. DETERMINACY  is the command's SCOPE known?  `rm -r "$FOO"` is
                    destructive AND indeterminate — we don't know WHAT it
                    deletes. Orthogonal to effect (`cat "$X"` is read-only and
                    indeterminate). Detected today by `has_unresolved_operand`.
    3. PROVENANCE   for each unresolved value, WHERE does it come from?
                    `$FOO` <- read <- FIFO <- find.  The axis that LINKS
                    stages; entirely absent today.

The rule we want: a benign command that is the SOURCE of an unresolved value
flowing into a dangerous, indeterminate consumer is raised to a WARNING (yellow)
with a note tracing the link. `rm` = primary danger (red); `find` = implicated
source (yellow). Never red for `find` — it is genuinely read-only; the warning
is about its ROLE, not its effect.


3. WHAT THE RESEARCH FOUND                                 *prov-priorart*
--------------------------------------------------------------------------------

A focused prior-art spike (ShellCheck, ABASH, Semgrep/CodeQL, HotOS'25, and the
agent harnesses) returned four load-bearing findings:

ABASH (Mazurak & Zdancewic, PLAS'07) IS THIS FEATURE, 19 years ago.
    Abstract interpretation of bash parameterised by per-command SIGNATURES that
    mark which argument positions are dangerous sinks. A variable's label is a
    product lattice: integrity (tainted/untainted) x EXPANSION ({0,1,2+} words).
    Command substitution and `read` both yield "unknown, tainted" — exactly our
    "source of an unknown." Crucially, ABASH makes the SAME precision cut we
    proposed — explicit-only forward taint, fail-open on the rest, accept false
    negatives to kill false positives — and ARGUES IT IS DESIRABLE, not merely
    expedient. Peer-reviewed existence proof + endorsement of our cut.
    (cis.upenn.edu/~stevez/papers/MZ07.pdf — §3.3 is our spec sheet.)

ShellCheck proves IDIOM-SHAPE MATCHING is a legitimate production strategy.
    It ships low-noise checks that recognise our exact binding idioms —
    SC2044 (`for x in $(find …)`), SC2013 (`for … in $(cat)` -> use `while
    read`), SC2045 (`ls`). Its DFA tracks value CHARACTERISTICS (space/numeric
    status), NOT provenance — no edge from a `$()` source to a use site. So the
    idiom recogniser is borrowable; the provenance link is ours to add.

NO AGENT HARNESS DOES THIS. (Claude Code, Cursor, Codex, Aider, Warp.)
    All classify the command STRING atomically — name/glob allowlists, regex
    denylists. Claude Code's "command injection detection" bumps a line
    containing `$(...)` to manual approval, but that is a PRESENCE heuristic, not
    a data-flow trace: it never connects the substitution's output to a
    downstream sink or explains the link. Published bypasses (Formal.ai on
    Claude Code; Backslash on Cursor) exist precisely because atomic matching is
    trivially defeated. => Our feature is genuinely NOVEL in the agent space. A
    differentiator, and the absence is a design gap, not a bad idea.

BACKWARD, DEMAND-DRIVEN taint is the right fit for our polarity.
    Classic taint runs FORWARD from a large source set. We already COMPUTE our
    sink set (the destructive/executor stages). So run it BACKWARD: only when a
    stage is dangerous AND indeterminate do we ask "where did this operand come
    from?", walk back through the binding idiom, and — if it lands on a
    substitution/`read`/`for` producer — annotate that producer. This is cheaper
    and, decisively, SELF-SANITISING: a substitution that feeds a harmless sink
    is never walked, so `diff <(sort a) <(sort b)` raises nothing for free (diff
    is not a destructive sink). The blunt "colour all `<()` yellow" alternative
    cries wolf on exactly that case; backward-from-sinks does not.


4. THE DESIGN                                              *prov-design*
--------------------------------------------------------------------------------

Backward, demand-driven, idiom-scoped taint, as an ADDITIVE pass that never
touches the (security-critical, sound) effect/decision analysis.

    a. SINKS.  Reuse the existing per-stage effect. A sink = a stage whose
       effect is Destructive or Executor AND which has an unresolved operand
       (`has_unresolved_operand`). A destructive command over a KNOWN path
       (`rm -r /tmp/x`) is already red and needs no provenance — we only explain
       what we cannot see.

    b. BINDINGS.  Recognise the small set of idioms that bind a variable (or
       splice a value) from a command's output, and record source-span ->
       carrier edges:
         1. `X=$(cmd)` / `X=`cmd`` then `$X` an operand      (simplest, commonest)
         2. `while read -r V; do … done < <(cmd)` and
            `cmd | while read -r V; do …`                    (our canonical case)
         3. `for V in $(cmd)` / backticks                    (SC2044 shape)
         4. `rm -r "$(find …)"` — substitution IS the operand (walk terminates
            immediately at the substitution). DONE 2026-09-30 for the unquoted
            spelling (`provenance::spliced_into`; a `Flow` with no `carrier`);
            the quoted one waits on the lexer, which refuses `"$(…)"` whole.
         5. `find … | xargs rm` — classify the PIPELINE: upstream producer feeds
            a destructive downstream. (Later; needs pipe-adjacency.)

    c. BACKWARD WALK.  For each sink's unresolved operand: if it is a variable
       `$V`, look up V's binder among (b); if the binder's value came from a
       command producer, that producer is the SOURCE. One hop of copy-
       propagation (`A=$(f); B=$A; rm "$B"`) — bounded, NOT general alias
       analysis. If the operand is a substitution directly, the source is it.

    d. IMPLICATION.  A resolved source is coloured YELLOW (warn) and gets a LINK
       NOTE: `find . -type f produces the paths bound to FOO, which rm -r
       deletes`. If the walk fails (idiom unrecognised, eval, dynamic name), we
       emit NOTHING extra: the source stays its intrinsic colour (green) and the
       sink stays red + ASK. We never FALSELY imply a link.

    e. FAIL OPEN, SAFELY.  A missed link degrades to v1: the danger is still red,
       the line still ASKs (effect/decision fail closed INDEPENDENTLY of
       provenance). So we can be precise-where-confident and silent-elsewhere —
       the posture ABASH defends. This is the ONE place we are deliberately
       asymmetric: prefer MISSING a subtle link over INVENTING one, because a
       false link is the alarm-fatigue we exist to prevent.

    f. EXPANSION NUANCE (steal from ABASH, maybe later).  `rm -r $FOO` (unquoted,
       word-splits into many operands) is a STRICTLY WORSE hazard than
       `rm -r "$FOO"` (one operand). Worth surfacing even without full taint. A
       candidate second signal, deferred from the first cut.


5. THE ARCHITECTURE QUESTION                               *prov-arch*
--------------------------------------------------------------------------------

CODE FACT (verified): `peel_control_flow` flattens the line, so the flattened
`Pipeline` holds the sink (`rm`, unresolved operand), the source (`find` subst
stage), and the binder (`read` stage / the `for` iteration list) as DISCONNECTED
stages. The edges the backward walk needs are gone.

Two ways to recover them:

    A. SEPARATE IDIOM PASS — pattern-match the shapes (`while read V … < <(cmd)`,
       `for V in $(cmd)`) on a less-flattened view and emit (source, sink, var)
       links. A lookup table of shapes. It does NOT compose: a copied value
       (`A=$(f); B=$A; rm $B`), a var used in two sinks, two vars from one
       `read`, a concatenated value — each misses the pattern, or needs a new
       hand-written one.

    B. ENRICH THE PIPELINE into a small data-flow graph — each stage records its
       variable READS, WRITES, and the SOURCE of each write (command-output /
       literal / unknown), plus the thin structural wiring (which `< <(cmd)` FIFO
       feeds which loop's `read`). Provenance is then a GRAPH WALK: from a sink's
       unknown operand, follow write->source edges backward (ABASH's two-pass:
       record assignments, then propagate). Chains, multiple variables,
       reassignment, transitive flows all FALL OUT of the walk — record
       primitives, not shapes.

DECISION: B, built core-first.  It is the more robust and it is what the goal
actually is — "trace the unknowns, walking back through the call graph" is a
graph walk, i.e. B; A only fakes it. Three reasons seal it:

  - COMPOSABILITY. B handles chains/multi-var/reassignment by construction; A
    needs a new pattern per case and still can't do transitive flow cleanly.
  - THE RENDERING NEEDS IT. Highlighting the carrier variable at its binding
    site, per-variable colours, multi-var tracing (§6) all require knowing
    variable IDENTITY and every occurrence SPAN — exactly the read/write sets B
    records and A lacks. The rendering goal and the robust architecture point the
    same way.
  - THE COST IS SMALLER THAN IT SOUNDS. The enrichment is ADDITIVE DATA (new
    fields). The effect/decision logic does not read them and stays byte-for-byte
    identical; only the new provenance pass reads them. Regression surface = "did
    adding fields break parsing," caught by the existing tests — not a rewrite of
    the safety logic.

B splits into a general CORE (per-stage var read/write/source sets — does the
chains and multi-var work) plus a THIN structural layer for the one genuinely-
structural link (loop-stdin / pipe wiring). Build the core first; robustness
where it matters, idiom-specificity only where the wiring is inherently
structural. P0 spike confirms feasibility of tying a sink's `$VAR` back to its
producer before committing.


6. RENDERING                                               *prov-render*
--------------------------------------------------------------------------------

Follow CodeQL/rustc: emphasise the endpoints, one label per hop, a CHAIN not a
dump. Primary attention on the SOURCE (the surprising part we're flagging),
sink already red, a hanging note tracing source -> $VAR -> sink.

>       ASK — you'll be prompted first
>
>         while read -r FOO; do
>             rm -r "$FOO"              (red — destructive, scope unknown)
>         done < <(find . -type f)      (yellow — this is what fills FOO)
>
>         [ask]  rm  · destructive
>          |_ deletes recursively; WHAT it deletes is decided at runtime by FOO
>         [warn] find · feeds FOO
>          |_ `find . -type f` produces the paths bound to FOO — so `rm -r`
>             removes whatever find returns. Check this to know what's deleted.

HIGHLIGHT THE CARRIER VARIABLE.  Mark the traced variable at BOTH its binding
site (`FOO` in `while read -r FOO`) and its use site (`$FOO` in the sink), so the
eye follows the flow `find -> FOO -> $FOO -> rm`. It is the intermediate hop in
the trace; in a multi-line loop it is what makes the chain legible (CodeQL/rustc
"show the hops"). Minor in a 2-liner, load-bearing at scale.

VARIABLE IDENTITY RIDES A SEPARATE CHANNEL.  Danger already owns fg/bg
(green-fg / yellow-fill / red-fill); variable identity must NOT reuse them or it
fights the danger signal. Use UNDERLINE with a muted per-variable palette
(cyan/magenta/blue — deliberately not green/yellow/red) as the underline colour.
Then `$FOO` inside a red `rm` fill is red-bg + cyan-underline, and `FOO` at the
`read` site is plain + cyan-underline — same underline = same variable,
COMPOSING with the danger fill instead of competing. Staged: (1) underline the
traced variable(s) linking binding<->use; (2) per-variable distinct underline
colour for the multi-variable case. CAVEAT: coloured underlines (SGR 58) have
uneven terminal support — confirm ratatui/terminal support in P0; fallback is a
plain single underline (identity by name), which degrades gracefully.

THE POLARITY GOTCHA (UX).  `find` is genuinely read-only; we warn about it
because of a DOWNSTREAM sink, which is counter-intuitive read left-to-right. The
note MUST make the RELATIONSHIP its subject ("read-only, BUT its output selects
what `rm -r` deletes"), or we look like we mis-classified `find`. Vocabulary
from [analyze diagnostics]: a proven chain gets an EFFECT-note; an untraceable
origin gets an INDETERMINATE-note — never a false EFFECT-note.


7. PITFALLS (from the research)                            *prov-pitfalls*
--------------------------------------------------------------------------------

- OVER-TAINTING is THE documented failure mode of every taint tool: false
  positives -> the human tunes them out. Weight the design toward missing a
  subtle link over inventing one. (Our whole reason to exist.)
- eval / `source $VAR` / dynamic command names are WALLS. Everyone gives up
  there (ABASH warns unconditionally; ShellCheck refuses; HotOS'25 defers to
  runtime). Our fail-open stance turns each wall into an "unknown" effect, not a
  false "safe."
- QUOTING is expansion semantics and load-bearing (see 4f); don't collapse
  quoted vs unquoted.
- Bound the backward walk (one copy-prop hop); do NOT attempt general aliasing.


8. PHASED PLAN                                             *prov-plan*
--------------------------------------------------------------------------------

    P0  DONE (2026-07-21, throwaway spike). Validated the B-core end to end: the
        backward walk produced the correct producer->var->sink link for ALL THREE
        idioms (`while read … < <(find)`, `for f in $(find)`, `X=$(find); rm $X`),
        and correctly produced NO link for the negatives (`diff <(sort a) <(sort
        b)` — read-only sink, never walked, self-sanitising; `rm -r /tmp/known` —
        no `$VAR`). ratatui 0.30 HAS `Style::underline_color` + `Modifier::
        UNDERLINED` (terminal SGR-58 support degrades to a plain underline).
        Findings that shape P1:
          - The wiring is recoverable from the LEXEME STREAM, not the flattened
            `Pipeline` (there `find` is a floating stage, conn=None, the `< <(…)`
            link gone). So the provenance pass reads lexemes (or enriches during
            parse), NOT the post-flatten stages.
          - Recovery primitives that worked:
              SINK   = a `Pipeline` stage with Destructive/Executor effect AND a
                       `$VAR` operand (`var_ref` parses `$FOO`/`${FOO}`).
              BIND   = a var->producer map from lexeme-level idiom recognition:
                       `done <Read> <Subst>` window = loop-stdin producer;
                       `read [-flags] V…` = non-flag names bound from loop stdin;
                       `for V in <Subst>` = V from the Subst;
                       `NAME= <Subst>` = NAME from the Subst.
              USE    = the lexeme inside the sink's span whose `var_ref` == var.
          - Subtleties: an unmodeled `read` does NOT parse flags (`-r` arrives as
            an operand — so "non-flag operands after read" is the rule); `X=`
            lexes as its own `Word` immediately before the `Subst`.
    P1  B-core: enrich stages with var read/write/source sets (additive fields),
        + the thin loop-stdin/pipe wiring; the backward-walk provenance query
        emitting (source-span, sink-span, var-occurrences) links for idioms 1–3
        (assignment, while-read, for-in). Pure, unit-tested; effect/decision
        untouched (regression-tested).
    P2  Renderer: implicated-source yellow + the link note (polarity-correct
        wording) + carrier-variable underline linking binding<->use. Extends
        `permission_view`.
    P3  Idioms 4–5 (direct-substitution operand, `find | xargs`); per-variable
        underline palette for multi-var; expansion nuance (4f) if it earns it.
        Idiom 4 DONE (2026-09-30).
    P4  Security/soundness review pass (this is analyzer code): confirm fail-open
        holds, no false EFFECT-notes, no regression to effect/decision.

Each phase green-committed; P1 is testable in isolation (links, no UI); P2 is the
first VISIBLE change (`find` goes yellow with a note + the carrier underline).


9. OPEN DECISIONS (for review)                             *prov-open*
--------------------------------------------------------------------------------

- ARCHITECTURE — DECIDED: B, core-first (§5). P0 spike confirms feasibility.
- FIRST-CUT SCOPE — idioms 1–3 in P1, defer 4–5 + expansion nuance? (recommend
  yes: cover the common cases, keep the surface small.)
- CARRIER-VARIABLE HIGHLIGHT — DECIDED: yes, on a separate underline channel
  (§6), staged (single underline first, per-variable palette later).
- `read` ITSELF — today `read -r F` shows yellow (unmodeled builtin). Under this
  model `read` is benign machinery (binds a var); model it as such so the
  interesting yellow lands on `find`, not scattered across the binder? (worth a
  small program-model entry.)


================================================================================
See also: [analyze diagnostics] (the rendering model + action grammar),
[permission failure-mode lens] (false-positive posture). vim: `:h` this file.
