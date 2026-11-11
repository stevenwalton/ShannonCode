//! Provenance — tracing an unresolved value back to the command that produced it.
//!
//! A destructive command over an *unresolved* operand (`rm -r "$FOO"`) is
//! dangerous in a way its effect alone doesn't capture: **what it acts on is
//! decided at runtime by whatever produced `$FOO`.** To let a human verify it,
//! we trace `$FOO` backward to its source — `find . -type f` in
//!
//! ```text
//! while read -r FOO; do rm -r "$FOO"; done < <(find . -type f)
//! ```
//!
//! — so the view can say "read-only, BUT its output selects what `rm -r`
//! deletes." See `docs/DesignDocs/provenance-coloring.md` for the full rationale.
//!
//! # Backward, demand-driven, idiom-scoped, fail-open
//!
//! We do NOT taint every value forward. We start only from the dangerous+
//! indeterminate **sinks** the effect analysis already found, and walk *backward*
//! to the producer. That is cheaper and self-sanitising: a substitution feeding a
//! harmless command (`diff <(sort a) <(sort b)`) is never walked, so it raises
//! nothing — the blunt "flag every `<()`" alternative cries wolf there. We
//! recognise a small set of BINDING IDIOMS (`X=$(cmd)`, `while read V … < <(cmd)`,
//! `for V in $(cmd)`, plus copy chains `B=$A` resolved transitively and
//! cycle-guarded); anything else yields no link, and
//! a missed link degrades gracefully — the sink is still classified dangerous and
//! still gated. We NEVER invent a link we cannot trace (over-tainting is the
//! documented failure mode of every taint tool, and it is the alarm fatigue we
//! exist to prevent).
//!
//! # The swap seam
//!
//! [`Flow`] is the OUTPUT CONTRACT and [`analyze`] the single entry point. A
//! different provenance engine (a fuller taint pass, an LLM-assisted one) would
//! produce the same `Vec<Flow>`; the renderer never learns how they were found.
//! Internally the engine is composed of self-contained parts — sink finding, a
//! *list* of idiom recognisers, and the backward walk — so a part can be replaced
//! or an idiom added without disturbing the rest.

use std::ops::Range;

use super::Effect;
use super::lex::{Lexeme, Operator, Spanned, lex};
use super::pipeline::{Pipeline, subst_inner_span};

/// Depth cap for the backward copy-chain walk in [`resolve`]. A real command's
/// copy chains are a handful of links; anything past this is adversarial. The cap
/// keeps a crafted chain (`A0=$A1; A1=$A2; …`) from recursing deep enough to
/// overflow the stack and ABORT the gate — the same failure the substitution cap
/// (`pipeline::MAX_SUBST_DEPTH`) prevents, since a stack overflow can't be caught.
const MAX_RESOLVE_DEPTH: usize = 256;

/// A traced data-flow: a command's output (`source`) flows into a dangerous
/// command (`sink`), through a carrier variable or spliced straight in.
///
/// This is the engine's OUTPUT CONTRACT — the renderer colours `source` as a
/// warning, underlines a carrier at its binding and use sites to make the flow
/// legible, and leaves `sink` to the danger colouring. All ranges are byte ranges
/// into the original input line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flow {
    /// The producing COMMAND's span (`find …`) — the inner command of a
    /// `<(…)`/`$(…)` construct, or the stage piped into a `read`. Implicated by
    /// what it feeds, whatever its own effect.
    pub source: Range<usize>,
    /// The variable that holds the value on its way to the sink, or `None` when
    /// the producer's output is spliced into the sink's own words (`rm $(find …)`)
    /// and nothing holds it.
    pub carrier: Option<Carrier>,
    /// The dangerous command's span.
    pub sink: Range<usize>,
}

/// The variable a [`Flow`] travels through (`FOO` in `X=$(find); rm "$FOO"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carrier {
    /// The variable's name (`FOO`).
    pub variable: String,
    /// Where the variable is introduced (the `FOO` in `read -r FOO`, the `X=` in
    /// an assignment, the `f` in `for f in …`).
    pub binding_site: Range<usize>,
    /// Every occurrence of the variable in the line (`"$FOO"` in `rm -r "$FOO"`),
    /// not only the sink's — see `var_uses_in` for why the whole line.
    pub use_sites: Vec<Range<usize>>,
}

/// Trace provenance flows in `input`, already decomposed into `pipeline`.
///
/// Re-lexes `input` (cheap, and this is an approval-time query, not a render hot
/// path) because the source→binder→sink wiring survives in the lexeme stream but
/// is flattened out of the `Pipeline`. Returns one [`Flow`] per (sink, carrier)
/// pair we can trace to a command producer, and one per producer spliced into a
/// sink's words; untraceable unknowns yield nothing.
pub fn analyze(input: &str, pipeline: &Pipeline) -> Vec<Flow> {
    let Ok(lexemes) = lex(input) else {
        return Vec::new();
    };
    let binds = bindings(&lexemes, pipeline);
    let mut flows = Vec::new();
    // Point the warning at the producing COMMAND (`find …`), not the `$(…)`/`<(…)`
    // plumbing that captures it — the thing to scrutinise is what the command
    // emits. (`subst_inner_span` leaves a piped producer's plain span alone.)
    let producer = |construct: &Range<usize>| subst_inner_span(input, construct);
    for sink in sinks(pipeline) {
        for var in &sink.unknowns {
            // Walk the carrier back to a command producer, resolving "as of" the
            // sink's position so a later reassignment can't mis-attribute it
            // (following copies, cycle-guarded). No producer -> no link (fail open).
            let mut seen = Vec::new();
            let Some((source, binding_site)) = resolve(var, sink.span.start, &binds, &mut seen)
            else {
                continue;
            };
            flows.push(Flow {
                source: producer(&source),
                carrier: Some(Carrier {
                    variable: var.clone(),
                    binding_site,
                    use_sites: var_uses_in(input, &lexemes, var),
                }),
                sink: sink.span.clone(),
            });
        }
        flows.extend(
            spliced_into(input, &lexemes, &sink.span, &binds).map(|construct| Flow {
                source: producer(&construct),
                carrier: None,
                sink: sink.span.clone(),
            }),
        );
    }
    flows
}

// --- sinks -------------------------------------------------------------------

/// A dangerous command with the unresolved variables it consumes.
struct Sink {
    span: Range<usize>,
    /// Each variable a `$VAR` operand of the sink reads — the thing whose origin
    /// decides what the command acts on.
    unknowns: Vec<String>,
}

/// The stages worth explaining: those whose effect is Destructive or Executor.
/// Only what they cannot show is traced — a `$VAR` operand here, a spliced
/// substitution in [`spliced_into`] — so a destructive command over a *known*
/// path (`rm -r /tmp/x`) yields no flow: it is already red and needs no trace.
fn sinks(pipeline: &Pipeline) -> Vec<Sink> {
    pipeline
        .stages
        .iter()
        // A stage counts as dangerous if it resolves to something dangerous OR if
        // its own command intrinsically is. The second half matters when an
        // earlier stage disturbed the bindings: `effect()` then collapses to
        // `Unknown`, which would silently drop the trace for a command that still
        // *reads* as `rm -r "$FOO"`. Losing the explanation exactly when we
        // understand the line least is the wrong direction — uncertainty may add
        // alarm, never remove it. (Provenance stays advisory either way.)
        .filter(|s| {
            let dangerous = |e| matches!(e, Effect::Destructive | Effect::Executor);
            dangerous(s.effect()) || dangerous(s.command.effect)
        })
        .map(|s| Sink {
            span: s.span.clone(),
            unknowns: s
                .command
                .operands
                .iter()
                .filter_map(|op| var_ref(op))
                .collect(),
        })
        .collect()
}

/// The command substitutions spliced into a sink's own words — `rm -f $(find …)`,
/// `sh -c $(curl …)` — each the construct span of a producer whose output becomes
/// part of what the sink is told to act on, with no variable between.
///
/// Excluded, because the output lands somewhere other than the sink's words: a
/// process substitution (`<(…)` hands over a pipe's path, not the output), a
/// redirect target (`> $(…)` names where output goes), and a value already
/// recognised as a binding (`X=$(…) rm …` puts it in the environment). Only the
/// lexer's top-level constructs are seen, so a substitution nested inside another
/// (`rm $(cat $(find))`) links through the outer one alone.
fn spliced_into<'a>(
    input: &'a str,
    lex: &'a [Spanned<Lexeme>],
    sink: &'a Range<usize>,
    binds: &'a [Binding],
) -> impl Iterator<Item = Range<usize>> + 'a {
    let bound = |span: &Range<usize>| {
        binds
            .iter()
            .any(|b| matches!(&b.source, Source::Produced(p) if p == span))
    };
    lex.iter().enumerate().filter_map(move |(i, l)| {
        let Lexeme::Subst(_) = l.node else {
            return None;
        };
        let inside = sink.start <= l.span.start && l.span.end <= sink.end;
        let command_subst = input
            .get(l.span.clone())
            .is_some_and(|t| t.starts_with("$(") || t.starts_with('`'));
        let redirected = i.checked_sub(1).and_then(|p| lex.get(p)).is_some_and(|p| {
            matches!(
                p.node,
                Lexeme::Op(Operator::Write | Operator::Append | Operator::Read)
            )
        });
        (inside && command_subst && !redirected && !bound(&l.span)).then(|| l.span.clone())
    })
}

// --- bindings (the idiom recognisers) ----------------------------------------

/// Where a bound variable's value comes from.
enum Source {
    /// The output of a command producer, at this span (`$(find …)` / `<(find …)`).
    Produced(Range<usize>),
    /// A copy of another variable (`B=$A`) — resolved transitively by the walk.
    Copied(String),
}

/// One recognised binding: a variable, where it was introduced, and its source.
struct Binding {
    var: String,
    site: Range<usize>,
    source: Source,
}

/// Recognise every binding idiom over the lexeme stream and collect the edges.
///
/// Each idiom is a self-contained recogniser appended here; adding a new idiom
/// (idioms 4–5 in the design doc) is one more push, not a rewrite.
fn bindings(lex: &[Spanned<Lexeme>], pipeline: &Pipeline) -> Vec<Binding> {
    let mut out = Vec::new();
    recognise_for(lex, &mut out);
    recognise_read(lex, &mut out);
    recognise_piped_read(lex, pipeline, &mut out);
    recognise_assign(lex, &mut out);
    out
}

/// `cmd | while read V` — a loop fed by a **pipe** rather than by a `< <(cmd)`
/// redirect.
///
/// The same idiom wearing different clothes, and by far the more common
/// spelling: `find … | while read f; do rm "$f"; done`. Recognising only the
/// redirect form meant the pipe form got no trace at all — the producer stayed
/// green and the carrier unmarked, which is precisely the case where the user
/// most needs to see what `rm` will act on.
///
/// This one reads the **pipeline** rather than the lexeme stream: a `read`
/// stage's `connector` already says it was piped into, and the stage before it
/// is the producer. The structure we need is computed; it just wasn't consulted.
fn recognise_piped_read(lex: &[Spanned<Lexeme>], pipeline: &Pipeline, out: &mut Vec<Binding>) {
    for (i, stage) in pipeline.stages.iter().enumerate() {
        if stage.command.program() != "read" || stage.connector != Some(Operator::Pipe) {
            continue;
        }
        let Some(prev) = i.checked_sub(1).and_then(|p| pipeline.stages.get(p)) else {
            continue;
        };
        // `read`'s flags are modelled, so its operands are exactly the variables
        // it binds (a flag's value never lands here).
        for name in stage.command.operands.iter().filter(|o| is_name(o)) {
            let Some(site) = token_span(lex, &stage.span, name) else {
                continue;
            };
            out.push(Binding {
                var: name.clone(),
                site,
                source: Source::Produced(prev.span.clone()),
            });
        }
    }
}

/// The span of the word `name` inside `within` — used to underline a variable at
/// the place it is bound, rather than highlighting the whole `read` stage.
fn token_span(lex: &[Spanned<Lexeme>], within: &Range<usize>, name: &str) -> Option<Range<usize>> {
    lex.iter()
        .find(|l| {
            l.span.start >= within.start
                && l.span.end <= within.end
                && matches!(&l.node, Lexeme::Word(w) if w == name)
        })
        .map(|l| l.span.clone())
}

/// `for V in $(cmd)` / backticks — V is bound from the following substitution.
fn recognise_for(lex: &[Spanned<Lexeme>], out: &mut Vec<Binding>) {
    for w in lex.windows(4) {
        if let (Lexeme::Word(kw), Lexeme::Word(var), Lexeme::Word(in_kw), Lexeme::Subst(_)) =
            (&w[0].node, &w[1].node, &w[2].node, &w[3].node)
            && kw == "for"
            && in_kw == "in"
            && is_name(var)
        {
            out.push(Binding {
                var: var.clone(),
                site: w[1].span.clone(),
                source: Source::Produced(w[3].span.clone()),
            });
        }
    }
}

/// `read`'s value-taking flags — their next token is a VALUE, not a variable, so
/// the carrier scan must skip it (`read -p Continue F` binds `F`, not `Continue`).
/// Mirrors the flag model in `program.rs`'s `read` entry.
const READ_VALUE_FLAGS: &[&str] = &["-p", "-a", "-d", "-i", "-n", "-N", "-t", "-u"];

/// `while read [-flags] V …; do … done < <(cmd)` — bind each `read`'s variables
/// to the stdin producer of the loop that LEXICALLY CONTAINS it.
///
/// A stack of loop frames tracks nesting: a loop opener (`while`/`until`/`for`)
/// pushes a frame that collects the `read`-vars inside it, and the matching
/// `done` binds them to that loop's `< <(cmd)` stdin (if any). This scopes each
/// `read` to its own loop — binding *every* `read` to the line's first producer
/// cross-attributes a second loop's variable to the first loop's source, and
/// mis-binds a `read` that sits before any loop (Reviewer A).
fn recognise_read(lex: &[Spanned<Lexeme>], out: &mut Vec<Binding>) {
    let mut stack: Vec<Vec<(String, Range<usize>)>> = Vec::new();
    let mut i = 0;
    while i < lex.len() {
        match &lex[i].node {
            Lexeme::Word(w) if w == "while" || w == "until" || w == "for" => {
                stack.push(Vec::new());
            }
            Lexeme::Word(w) if w == "read" => {
                if let Some(frame) = stack.last_mut() {
                    let mut j = i + 1;
                    while let Some(Spanned {
                        node: Lexeme::Word(t),
                        span,
                    }) = lex.get(j)
                    {
                        if READ_VALUE_FLAGS.contains(&t.as_str()) {
                            j += 2; // skip the flag AND its value
                            continue;
                        }
                        if !t.starts_with('-') && is_name(t) {
                            frame.push((t.clone(), span.clone()));
                        }
                        j += 1;
                    }
                }
            }
            Lexeme::Word(w) if w == "done" => {
                // Close the innermost loop; bind its reads to its stdin producer.
                if let Some(reads) = stack.pop()
                    && let Some(producer) = done_stdin_producer(lex, i)
                {
                    for (var, site) in reads {
                        out.push(Binding {
                            var,
                            site,
                            source: Source::Produced(producer.clone()),
                        });
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// The producer feeding a loop's stdin: the `<(cmd)` in a `done < <(cmd)` at
/// lexeme index `done_idx`. `None` when the loop reads from elsewhere.
fn done_stdin_producer(lex: &[Spanned<Lexeme>], done_idx: usize) -> Option<Range<usize>> {
    match (lex.get(done_idx + 1), lex.get(done_idx + 2)) {
        (Some(op), Some(sub))
            if matches!(op.node, Lexeme::Op(Operator::Read))
                && matches!(sub.node, Lexeme::Subst(_)) =>
        {
            Some(sub.span.clone())
        }
        _ => None,
    }
}

/// Assignments: `X=$(cmd)` / `X+=$(cmd)` (bound from a producer — lexes as a
/// `NAME=`/`NAME+=` word then a `Subst`) and `B=$A` (a copy — a single
/// `NAME=$REF` word).
fn recognise_assign(lex: &[Spanned<Lexeme>], out: &mut Vec<Binding>) {
    for (i, sp) in lex.iter().enumerate() {
        let Lexeme::Word(w) = &sp.node else { continue };
        // `X=$(cmd)` / `X+=$(cmd)`: a `NAME=`/`NAME+=` word then a substitution.
        if let Some(lhs) = w.strip_suffix('=')
            && let Some(name) = assign_name(lhs)
            && let Some(Spanned {
                node: Lexeme::Subst(_),
                span,
            }) = lex.get(i + 1)
        {
            out.push(Binding {
                var: name.to_string(),
                site: sp.span.clone(),
                source: Source::Produced(span.clone()),
            });
            continue;
        }
        // `B=$A` / `B+=$A` / `B=${A}`: the RHS is a bare variable reference — the
        // WHOLE RHS (`X=$A=b` is `$A` concatenated with `=b`, not a copy).
        if let Some((lhs, rhs)) = w.split_once('=')
            && let Some(name) = assign_name(lhs)
            && let Some(referent) = bare_var_ref(rhs)
        {
            out.push(Binding {
                var: name.to_string(),
                site: sp.span.clone(),
                source: Source::Copied(referent),
            });
        }
    }
}

/// The variable an assignment left-hand side names, accepting the append form
/// (`NAME+=` — the same `+=` grammar `normalize::assignment` honours; keeping
/// them in sync so a value the effect engine treats as an assignment isn't
/// invisible to provenance). `None` if the lhs isn't a shell name.
fn assign_name(lhs: &str) -> Option<&str> {
    let name = lhs.strip_suffix('+').unwrap_or(lhs);
    is_name(name).then_some(name)
}

/// The variable named by a token that is *entirely* a bare reference — `$A` or
/// `${A}` and nothing more. Unlike [`var_ref`] (which accepts a leading ref in a
/// larger token, e.g. `$A/sub`), a copy assignment requires the reference to be
/// the whole value, or it's a concatenation, not a copy.
fn bare_var_ref(token: &str) -> Option<String> {
    let name = var_ref(token)?;
    (token == format!("${name}") || token == format!("${{{name}}}")).then_some(name)
}

// --- the backward walk -------------------------------------------------------

/// Resolve `var`, as of byte position `before`, to the command producer that
/// ultimately fills it — following copies transitively, cycle-guarded. Returns
/// the producer span and the *carrier's own* binding site (so the underline lands
/// on the variable the sink reads, not a copied ancestor's).
///
/// **Position-aware**: we take the last binding whose site *precedes* `before`,
/// modelling "the binding that already ran when the value was used" — shell's
/// last-assignment-wins, correctly. A flat "last in the list" (recogniser order)
/// or "last in the file" (ignoring the use position) points at the WRONG source
/// on shadowed names (`F=$(cat); for F in $(find); do rm "$F"`) and on a
/// reassignment *after* the use (`X=$(find a); rm "$X"; X=$(find b)`) — inventing
/// a confidently-wrong link, worse than none. (Reviewers A/C/D.)
fn resolve(
    var: &str,
    before: usize,
    binds: &[Binding],
    seen: &mut Vec<String>,
) -> Option<(Range<usize>, Range<usize>)> {
    // Bound the walk DEPTH, not just cycles: an acyclic copy chain still recurses
    // one frame per link, so a crafted chain would overflow the stack and abort
    // the gate before a verdict. Past the cap we drop the LINK only (fail-open —
    // provenance is advisory); the sink is still coloured and gated by its own
    // effect, so nothing dangerous is hidden.
    if seen.len() >= MAX_RESOLVE_DEPTH {
        return None;
    }
    if seen.iter().any(|v| v == var) {
        return None; // cycle — bail (fail open)
    }
    seen.push(var.to_string());
    // The last binding of `var` that had already executed at `before`.
    let b = binds
        .iter()
        .filter(|b| b.var == var && b.site.start < before)
        .max_by_key(|b| b.site.start)?;
    match &b.source {
        Source::Produced(span) => Some((span.clone(), b.site.clone())),
        // A copy captures the referent's value at the moment the COPY ran — so
        // chase the referent as of the copy's own site, keeping this variable's
        // binding site for the underline.
        Source::Copied(other) => {
            resolve(other, b.site.start, binds, seen).map(|(src, _)| (src, b.site.clone()))
        }
    }
}

// --- helpers -----------------------------------------------------------------

/// Every occurrence of `$var` in the line — the tokens carrying the value, to
/// underline. Matches the resolved-word form (`"$FOO"` lexes to `$FOO`).
///
/// Deliberately NOT restricted to the sink. The underline is an *identity*
/// channel ("this token is the carrier"), separate from the danger fill, so
/// marking every occurrence lets the eye follow the value through the whole
/// command — `find` → `FOO` → the `echo` that shows it → the `rm` that acts on
/// it. Restricting it to the sink hid the middle of the very flow it exists to
/// make legible.
fn var_uses_in(input: &str, lex: &[Spanned<Lexeme>], var: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    for l in lex.iter() {
        if !matches!(l.node, Lexeme::Word(_)) {
            continue;
        }
        let Some(text) = input.get(l.span.clone()) else {
            continue;
        };
        // A bare reference (`$FOO`, `"$FOO"`) underlines the whole token; a
        // reference EMBEDDED in a larger word (`"FOO is: $FOO"`) underlines just
        // the reference, so the trace stays legible through interpolation
        // instead of breaking at the first quoted string.
        if var_ref_of_word(&l.node).as_deref() == Some(var) {
            out.push(l.span.clone());
            continue;
        }
        out.extend(
            embedded_refs(text, var).map(|r| (l.span.start + r.start)..(l.span.start + r.end)),
        );
    }
    out
}

/// The `var_ref` of a `Word` lexeme, or `None` for any other lexeme.
fn var_ref_of_word(node: &Lexeme) -> Option<String> {
    match node {
        Lexeme::Word(w) => var_ref(w),
        _ => None,
    }
}

/// Byte ranges of every `$var` / `${var}` inside `text`, as an iterator.
///
/// Boundary-checked so `$FOOBAR` is not read as a use of `FOO`. ASCII-only
/// patterns, so every offset lands on a char boundary even in multibyte text.
fn embedded_refs<'a>(text: &'a str, var: &'a str) -> impl Iterator<Item = Range<usize>> + 'a {
    let plain = format!("${var}");
    let braced = format!("${{{var}}}");
    let mut found: Vec<Range<usize>> = Vec::new();
    for (pat, boundary) in [(braced, false), (plain, true)] {
        let mut from = 0usize;
        while let Some(i) = text[from..].find(&pat) {
            let start = from + i;
            let end = start + pat.len();
            // `$FOO` must not be the prefix of a longer name (`$FOOBAR`);
            // `${FOO}` is already delimited.
            let ok = !boundary
                || text[end..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
            // Skip a `$FOO` already covered by a `${FOO}` match.
            if ok && !found.iter().any(|r| r.start <= start && end <= r.end) {
                found.push(start..end);
            }
            from = end;
        }
    }
    found.into_iter()
}

/// Is `w` a shell variable name (`[A-Za-z_][A-Za-z0-9_]*`)? ASCII-only, per the
/// POSIX grammar — matching `normalize::assignment` (bash rejects non-ASCII
/// identifiers, so a Unicode-permissive check here would only ever over-match on
/// input that can't run).
fn is_name(w: &str) -> bool {
    let mut cs = w.chars();
    cs.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The variable a `$VAR` / `${VAR}` reference names, if the token is (or begins
/// with) a bare parameter expansion. `$FOO` -> `FOO`, `${FOO}` -> `FOO`,
/// `"$FOO"` (already quote-resolved to `$FOO`) -> `FOO`. A literal or a complex
/// expansion (`${FOO:-x}`) yields `None`.
fn var_ref(token: &str) -> Option<String> {
    let rest = token.strip_prefix('$')?;
    let (rest, braced) = match rest.strip_prefix('{') {
        Some(r) => (r, true),
        None => (rest, false),
    };
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if !is_name(&name) {
        return None;
    }
    // A braced ref must close cleanly with no modifier (`${FOO}` yes, `${FOO:-x}`
    // no) — a modifier means the value isn't a plain copy of the variable.
    if braced && rest.get(name.len()..) != Some("}") {
        return None;
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::parse_line;

    fn flows(input: &str) -> Vec<Flow> {
        let pipe = parse_line(input).expect("analyzable");
        analyze(input, &pipe)
    }

    /// Re-slice a span to its source text, so assertions read against the input.
    fn at<'a>(input: &'a str, r: &Range<usize>) -> &'a str {
        &input[r.clone()]
    }

    /// The carrier of a flow the test expects to travel through a variable.
    fn carried(f: &Flow) -> &Carrier {
        f.carrier.as_ref().expect("a carried flow")
    }

    /// `cmd | while read V` — the *common* spelling of the loop idiom, and the
    /// one that had no trace at all: the producer stayed green and the carrier
    /// unmarked, exactly where the user most needs to see what `rm` acts on.
    /// (Steven's report.)
    #[test]
    fn a_piped_read_links_to_the_command_feeding_it() {
        let input =
            r#"find delete_this_dir -name "*.txt" -type f | while read file; do rm "$file"; done"#;
        let fs = flows(input);
        assert_eq!(fs.len(), 1, "one flow: find -> file -> rm\n{fs:#?}");
        assert_eq!(
            at(input, &fs[0].source),
            r#"find delete_this_dir -name "*.txt" -type f"#
        );
        assert_eq!(carried(&fs[0]).variable, "file");
        assert_eq!(
            at(input, &carried(&fs[0]).binding_site),
            "file",
            "the carrier, not the whole read"
        );
        assert_eq!(
            carried(&fs[0])
                .use_sites
                .iter()
                .map(|u| at(input, u))
                .collect::<Vec<_>>(),
            vec![r#""$file""#]
        );
    }

    /// The last command in a pipe chain is the producer — `read` is fed by what
    /// immediately precedes it, not by the head of the line.
    #[test]
    fn a_piped_read_takes_its_immediate_producer() {
        let input = r#"cat a | find . -name x | while read f; do rm "$f"; done"#;
        let fs = flows(input);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert_eq!(at(input, &fs[0].source), "find . -name x");
    }

    /// A `read` that is NOT piped into keeps its previous behaviour — no
    /// producer is invented from whatever happens to sit before it.
    #[test]
    fn a_sequential_read_is_not_linked_to_the_previous_command() {
        let input = r#"find . -name x; while read f; do rm "$f"; done"#;
        assert!(flows(input).is_empty(), "`;` is not a data flow");
    }

    #[test]
    fn while_read_from_process_sub_links() {
        let input = r#"while read -r FOO; do rm -r "$FOO"; done < <(find . -type f)"#;
        let fs = flows(input);
        assert_eq!(fs.len(), 1, "one flow: find -> FOO -> rm\n{fs:#?}");
        let f = &fs[0];
        assert_eq!(at(input, &f.source), "find . -type f");
        assert_eq!(carried(f).variable, "FOO");
        assert_eq!(at(input, &carried(f).binding_site), "FOO");
        assert_eq!(
            carried(f)
                .use_sites
                .iter()
                .map(|u| at(input, u))
                .collect::<Vec<_>>(),
            vec![r#""$FOO""#]
        );
        assert_eq!(at(input, &f.sink), r#"rm -r "$FOO""#);
    }

    #[test]
    fn for_in_command_sub_links() {
        let input = r#"for f in $(find . -type f); do rm -r "$f"; done"#;
        let fs = flows(input);
        assert_eq!(fs.len(), 1);
        assert_eq!(at(input, &fs[0].source), "find . -type f");
        assert_eq!(carried(&fs[0]).variable, "f");
    }

    #[test]
    fn assignment_from_command_sub_links() {
        let input = r#"TARGET=$(find . -type f); rm -r "$TARGET""#;
        let fs = flows(input);
        assert_eq!(fs.len(), 1);
        assert_eq!(at(input, &fs[0].source), "find . -type f");
        assert_eq!(carried(&fs[0]).variable, "TARGET");
        assert_eq!(at(input, &carried(&fs[0]).binding_site), "TARGET=");
    }

    /// One-hop copy: the walk chases `B` <- `$A` <- `$(find)`, but keeps B's own
    /// binding site (the name the sink reads).
    #[test]
    fn one_hop_copy_is_followed() {
        let input = r#"A=$(find . -type f); B=$A; rm -r "$B""#;
        let fs = flows(input);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert_eq!(at(input, &fs[0].source), "find . -type f");
        assert_eq!(carried(&fs[0]).variable, "B");
    }

    /// The self-sanitising negative: a substitution feeding a READ-ONLY command is
    /// never walked (diff is not a sink), so nothing is flagged — the property
    /// that stops the false alarms a blunt "flag every `<()`" would raise.
    #[test]
    fn substitution_feeding_a_harmless_sink_is_silent() {
        assert!(flows("diff <(sort a) <(sort b)").is_empty());
    }

    /// A producer spliced straight into a sink's words has no variable between —
    /// `rm -f $(find …)` is the plainest spelling of "find decides what rm
    /// deletes", and it had no trace at all.
    #[test]
    fn a_spliced_substitution_links_without_a_carrier() {
        let input = "rm -f $(find . -name '*.o')";
        let fs = flows(input);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert_eq!(at(input, &fs[0].source), "find . -name '*.o'");
        assert_eq!(fs[0].carrier, None, "nothing holds the value");
        assert_eq!(at(input, &fs[0].sink), input);
        // Backticks are the same construct.
        let ticks = "rm -f `find . -name x`";
        assert_eq!(flows(ticks).len(), 1, "{:#?}", flows(ticks));
    }

    /// Where the output does NOT land in the sink's words, there is no splice: a
    /// redirect target names a file, and a prefix assignment puts the value in the
    /// environment (`$X` here still expands to its OLD value).
    #[test]
    fn a_substitution_outside_the_sinks_words_is_not_spliced() {
        // Mid-command, so the target sits inside `rm`'s span.
        assert!(flows("rm > $(mktemp) -f x").is_empty(), "a redirect target");
        assert!(
            flows("rm -f <(find .)").is_empty(),
            "a pipe's path, not output"
        );
        assert!(flows("X=$(find .) rm -f y").is_empty(), "a prefix binding");
        // Outside the sink's span entirely: the `echo` stage, not `rm`'s words.
        assert!(flows("echo $(find .); rm -f y").is_empty());
    }

    /// A destructive command over a KNOWN path has no unknown to trace.
    #[test]
    fn destructive_over_a_known_path_is_silent() {
        assert!(flows("rm -r /tmp/known").is_empty());
    }

    /// An untraceable origin (the variable comes from the environment, not a
    /// recognised binding) yields no link — the sink is still gated elsewhere, we
    /// simply don't invent a source.
    #[test]
    fn untraceable_variable_yields_no_link() {
        assert!(flows(r#"rm -r "$HOME_GROWN""#).is_empty());
    }

    /// A binding cycle must not loop forever; it bails (no link).
    #[test]
    fn copy_cycle_terminates() {
        // A<-B, B<-A, then a sink on $A — no producer, cycle-guarded.
        let input = r#"A=$B; B=$A; rm -r "$A""#;
        assert!(flows(input).is_empty());
    }

    /// Multiple carriers into one sink each get their own flow.
    #[test]
    fn multiple_variables_each_flow() {
        let input = r#"A=$(find . -name x); B=$(find . -name y); rm -r "$A" "$B""#;
        let mut fs = flows(input);
        fs.sort_by(|a, b| carried(a).variable.cmp(&carried(b).variable));
        assert_eq!(fs.len(), 2);
        assert_eq!(carried(&fs[0]).variable, "A");
        assert_eq!(carried(&fs[1]).variable, "B");
    }

    #[test]
    fn var_ref_parses_plain_and_braced_only() {
        assert_eq!(var_ref("$FOO").as_deref(), Some("FOO"));
        assert_eq!(var_ref("${FOO}").as_deref(), Some("FOO"));
        assert_eq!(var_ref("${FOO:-x}"), None); // a modifier, not a plain copy
        assert_eq!(var_ref("/tmp/x"), None);
        assert_eq!(var_ref("$1"), None); // positional, not a name
    }

    /// Position-aware resolve: a reassignment AFTER the use must not mis-attribute
    /// the sink to the later source (the `rm` ran on `find a`'s output). (A/C/D.)
    #[test]
    fn reassignment_after_use_traces_the_binding_that_ran() {
        let input = r#"X=$(find a); rm -r "$X"; X=$(find b)"#;
        let fs = flows(input);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert_eq!(
            at(input, &fs[0].source),
            "find a",
            "the binding before the use"
        );
    }

    /// Shadowing across idioms: the loop's per-iteration `F` (from `find`), not
    /// the earlier assignment `F=$(cat)`, drives the `rm` inside the loop.
    #[test]
    fn a_loop_var_shadows_an_earlier_assignment() {
        let input = r#"F=$(cat other); for F in $(find x); do rm -r "$F"; done"#;
        let fs = flows(input);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert_eq!(at(input, &fs[0].source), "find x");
    }

    /// Each `read` is scoped to its own loop — two loops don't cross-wire, and a
    /// `read` before any loop binds to nothing. (Reviewer A's M1.)
    #[test]
    fn reads_are_scoped_to_their_own_loop() {
        let input = "while read -r A; do rm -r \"$A\"; done < <(find a); \
                     while read -r B; do rm -r \"$B\"; done < <(find b)";
        let mut fs = flows(input);
        fs.sort_by(|x, y| carried(x).variable.cmp(&carried(y).variable));
        assert_eq!(fs.len(), 2, "{fs:#?}");
        assert_eq!(at(input, &fs[0].source), "find a"); // A ← loop a
        assert_eq!(at(input, &fs[1].source), "find b"); // B ← loop b, not a

        // A `read` before any loop has no producer — not the loop's.
        let before =
            r#"read -r CFG; rm -r "$CFG"; while read -r F; do rm -r "$F"; done < <(find .)"#;
        let fbefore = flows(before);
        assert_eq!(fbefore.len(), 1, "only F is traced\n{fbefore:#?}");
        assert_eq!(carried(&fbefore[0]).variable, "F");
    }

    /// The append form `X+=$(cmd)` is an assignment the effect engine handles;
    /// provenance must trace it too (Reviewer D — a silent gap otherwise).
    #[test]
    fn append_assignment_is_traced() {
        let input = r#"X+=$(find . -type f); rm -r "$X""#;
        let fs = flows(input);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert_eq!(at(input, &fs[0].source), "find . -type f");
        assert_eq!(carried(&fs[0]).variable, "X");
    }

    /// `X=$A=b` is `$A` concatenated with `=b`, NOT a copy of `A` — so tracing must
    /// not follow it to `A`'s source. (Reviewer A's L4.)
    #[test]
    fn concatenation_is_not_a_copy() {
        // A is a real producer; X=$A=b is a concat, so X must NOT inherit A's source.
        let input = r#"A=$(find a); X=$A=b; rm -r "$X""#;
        assert!(flows(input).is_empty(), "X is not a copy of A");
    }

    /// A value-taking `read` flag's bareword value is not a carrier variable
    /// (`read -p Continue F` binds `F`, not `Continue`). (Reviewer A's L1.)
    #[test]
    fn read_flag_value_is_not_a_carrier() {
        let input = r#"while read -p Continue F; do rm -r "$Continue"; done < <(find .)"#;
        // `$Continue` has no binding (Continue was the -p prompt), so no flow.
        assert!(flows(input).is_empty(), "the -p value is not a variable");
    }

    /// Defense-in-depth: a copy chain deeper than any real command must not recurse
    /// the resolver into a stack overflow that ABORTS the gate before a verdict —
    /// the same failure mode the substitution cap prevents. A modest chain still
    /// resolves; past the cap the deep link is dropped (advisory, fail-open), never
    /// walked without bound. (Reviewer E.)
    #[test]
    fn a_copy_chain_past_the_cap_bails_without_overflowing() {
        // A modest chain resolves end-to-end.
        let ok = r#"A0=$(find .); A1=$A0; A2=$A1; rm -r "$A2""#;
        let fs = flows(ok);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert_eq!(at(ok, &fs[0].source), "find .");

        // A chain deeper than the cap terminates safely — the deep link is dropped,
        // not walked, so no flow is produced and (the point) nothing overflows.
        let depth = MAX_RESOLVE_DEPTH + 50;
        let mut src = String::from("A0=$(find .); ");
        for i in 1..=depth {
            src.push_str(&format!("A{i}=$A{}; ", i - 1));
        }
        src.push_str(&format!(r#"rm -r "$A{depth}""#));
        assert!(flows(&src).is_empty(), "the over-cap link is dropped");
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use crate::shell::parse_line;
    use proptest::prelude::*;

    /// Command-shaped noise: shell metacharacters, quotes, substitutions,
    /// keywords, variables, and multibyte text — the operators are what make a
    /// generator find parser edges rather than random letters.
    fn arb_command() -> impl Strategy<Value = String> {
        let atom = prop_oneof![
            Just("rm".to_string()),
            Just("-rf".to_string()),
            Just("find".to_string()),
            Just("echo".to_string()),
            Just("read".to_string()),
            Just("IFS=".to_string()),
            Just("$FOO".to_string()),
            Just("\"$FOO\"".to_string()),
            Just("${FOO}".to_string()),
            Just("X=$(find .)".to_string()),
            Just("$(".to_string()),
            Just(")".to_string()),
            Just("<(".to_string()),
            Just("`".to_string()),
            Just("\"".to_string()),
            Just("'".to_string()),
            Just("\\".to_string()),
            Just("|".to_string()),
            Just("&&".to_string()),
            Just(";".to_string()),
            Just(">".to_string()),
            Just("<".to_string()),
            Just("while".to_string()),
            Just("do".to_string()),
            Just("done".to_string()),
            Just("for".to_string()),
            Just("in".to_string()),
            Just("if".to_string()),
            Just("fi".to_string()),
            Just("é€🙂".to_string()),
            Just("\u{202e}".to_string()), // bidi override
            "[a-zA-Z0-9_./-]{0,8}",
        ];
        prop::collection::vec(atom, 0..24).prop_map(|v| v.join(" "))
    }

    proptest! {
        /// The analyzer is a **gate**: it must return a verdict or a refusal for
        /// any input, never panic. A panic here is not a crash in a viewer — it
        /// is the safety check dying before it can say no.
        #[test]
        fn analysis_never_panics(cmd in arb_command()) {
            if let Ok(pipeline) = parse_line(&cmd) {
                let flows = analyze(&cmd, &pipeline);
                // Every span a flow reports must be a valid slice of the input,
                // or the renderer would slice out of bounds / mid-character.
                for f in flows {
                    prop_assert!(cmd.get(f.source.clone()).is_some());
                    prop_assert!(cmd.get(f.sink.clone()).is_some());
                    if let Some(c) = &f.carrier {
                        prop_assert!(cmd.get(c.binding_site.clone()).is_some());
                        for u in &c.use_sites {
                            prop_assert!(cmd.get(u.clone()).is_some());
                        }
                    }
                }
            }
        }

        /// Provenance is ADVISORY: it may explain a dangerous sink, but it must
        /// never invent one. Every flow's sink has to be a stage that really is
        /// dangerous — otherwise a yellow "feeds" note would point at nothing.
        #[test]
        fn every_flow_points_at_a_real_sink(cmd in arb_command()) {
            if let Ok(pipeline) = parse_line(&cmd) {
                for f in analyze(&cmd, &pipeline) {
                    let dangerous = |e| matches!(e, Effect::Destructive | Effect::Executor);
                    prop_assert!(
                        pipeline.stages.iter().any(|s| s.span == f.sink
                            && (dangerous(s.effect()) || dangerous(s.command.effect))),
                        "a flow must name a stage that is actually dangerous"
                    );
                }
            }
        }
    }
}
