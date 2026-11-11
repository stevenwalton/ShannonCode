//! Whole-line analysis: decomposing `cd foo && cargo test | tee log` into
//! stages we can each reason about — **with the state that changes what later
//! stages mean.**
//!
//! # Why decomposition alone is not enough
//!
//! The obvious approach ("split on `&&`, check each part") is unsound, because
//! earlier stages change the meaning of later ones:
//!
//! ```text
//! cd /             && rm -rf *      # each part looks tame; together, catastrophic
//! cd /tmp/scratch  && rm -rf *      # identical shape, entirely reasonable
//! ```
//!
//! So this layer is a tiny **abstract interpretation**: it walks the stages in
//! order, tracks the working directory through `cd`, and records the directory
//! each stage would actually run in ([`Cwd`]). A `cd` we cannot resolve
//! (`cd "$DIR"`, `cd -`, a glob, or bare `cd` to `$HOME`) makes the directory
//! [`Cwd::Unknown`] for every later stage — and Unknown is sticky. That is the
//! fail-closed direction: we would rather say "I don't know where this runs"
//! than assume it runs somewhere harmless.
//!
//! # Why this matters for prompts
//!
//! Refusing to analyze compounds is safe but *expensive*: it is why a harness
//! asks about `cd api && npm test` even when every part is already permitted —
//! the "700+ prompts" complaint. With sound decomposition, "every stage is
//! already permitted" becomes a conclusion we can actually reach, so the volume
//! of asks drops without lowering the bar. Reducing ask-volume is a security
//! intervention, not a convenience (habituation is the enemy).
//!
//! # Redirections are effects, not noise
//!
//! `>` and `>>` **write to a file**, so they raise the stage's effect to at
//! least [`Effect::Mutating`] and the target is recorded for path policy. (A
//! harness that strips redirections before matching lets `python x.py > prod.db`
//! match a rule written for `python`.) We keep them.

use std::ops::Range;

use super::lex::{Lexeme, Operator, Spanned, lex};
use super::normalize::normalize_as_written;
use super::{Command, Effect, Unanalyzable};

/// Where a stage runs, tracked through preceding `cd`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cwd {
    /// The directory the line started in — no `cd` has happened yet.
    Start,
    /// A directory we resolved. Absolute if it begins with `/`, otherwise
    /// relative to wherever the line started.
    Resolved(String),
    /// **One of several places, each known** — two or more, distinct, each a
    /// [`Start`](Cwd::Start) or a [`Resolved`](Cwd::Resolved). Past a `cd`
    /// that may have failed: `cd build; rm -rf out` runs in `build` or where
    /// the line began, and both are places we can name (Steven, 10-05: "We
    /// should be tracking the cwd"). It was [`Unknown`](Cwd::Unknown), which
    /// asked of every such line — 39 of a day's stages. A reader answers for
    /// every place ([`places`](Cwd::places)): a stage is inside only if each
    /// is, a deny meets a file at any, an allow covers it at all.
    OneOf(Vec<Cwd>),
    /// A `cd` we could not resolve (variable, glob, `~`, `-`, or no argument).
    /// Sticky: every later stage is Unknown too.
    Unknown,
}

impl Cwd {
    /// **Every place a stage may run in** — one, or several past a `cd` that
    /// may have failed; `None` where a `cd` could not be followed.
    pub fn places(&self) -> Option<&[Cwd]> {
        match self {
            Cwd::Start | Cwd::Resolved(_) => Some(std::slice::from_ref(self)),
            Cwd::OneOf(places) => Some(places),
            Cwd::Unknown => None,
        }
    }

    /// The places of both, each once — [`Unknown`](Cwd::Unknown) if either
    /// is, or past [`MOST_PLACES`]; a single place as itself.
    fn union(&self, other: &Cwd) -> Cwd {
        let (Some(a), Some(b)) = (self.places(), other.places()) else {
            return Cwd::Unknown;
        };
        let mut places: Vec<Cwd> = Vec::new();
        for place in a.iter().chain(b) {
            if !places.contains(place) {
                if places.len() == MOST_PLACES {
                    return Cwd::Unknown;
                }
                places.push(place.clone());
            }
        }
        match places.len() {
            1 => places.pop().unwrap_or(Cwd::Start),
            _ => Cwd::OneOf(places),
        }
    }
}

/// **The most places a line is followed into** before it is
/// [`Unknown`](Cwd::Unknown) — which asks, so the bound costs prompts, never
/// soundness. Each relative `cd` that may fail doubles the places (`cd d1;
/// cd d2; …` is 2^k), and every reader works per place on the UI thread:
/// ten `cd dN; make; cd ..;` took 8.6 s to decide (review of 0b04a29,
/// Reviewer C — the class of `[[feedback_bound_the_work]]`).
pub const MOST_PLACES: usize = 8;

/// Whether a stage's program names still mean what we think they mean.
///
/// A command's *name* does not determine the *program*: shell aliases, shell
/// functions, and `$PATH` all sit between the two. If an earlier stage disturbs
/// any of them, then `rm` in a later stage may not be `/bin/rm`, and every
/// conclusion we drew from the name is void — so we say so rather than pretend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bindings {
    /// Names resolve the way we analyzed them.
    Trusted,
    /// A preceding stage redefined how names resolve (`alias`, `unalias`,
    /// `eval`, `source`/`.`, a function definition, `hash`, or a `PATH` change).
    /// Sticky, like [`Cwd::Unknown`].
    Disturbed,
}

/// What a redirection does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectKind {
    /// `>` — truncate and write.
    Write,
    /// `>>` — append.
    Append,
    /// `<` — read from.
    Read,
}

/// One redirection attached to a stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub kind: RedirectKind,
    /// The file it reads from or writes to, as written.
    pub target: String,
    /// Byte range in the original line of the operator and its target
    /// (`> notes.txt`), so a view can mark the write where it is written.
    pub span: Range<usize>,
}

impl Redirect {
    /// **Is the target exactly `/dev/null`** — the file that keeps nothing
    /// written to it and gives nothing to a read? `2>/dev/null` made every
    /// stage that silences its errors "mutating", and so asked (the 2026-09-25
    /// demo). EXACTLY that spelling and nothing else under `/dev`:
    /// `/dev/tcp/host/port` is bash opening a network connection, and a
    /// device, a `/dev/fd/N` or another spelling of the same file is not
    /// worth the guess — they still count as the files they may be.
    pub fn is_null(&self) -> bool {
        self.target == "/dev/null"
    }

    /// Does this redirection put bytes in a file — a write or an append, to
    /// anything but [`/dev/null`](Self::is_null)?
    pub fn writes(&self) -> bool {
        matches!(self.kind, RedirectKind::Write | RedirectKind::Append) && !self.is_null()
    }

    /// What this redirection does to its target: writes it, or reads it.
    pub fn effect(&self) -> Effect {
        if self.writes() {
            Effect::Mutating
        } else {
            Effect::ReadOnly
        }
    }
}

/// One simple command in a line, plus everything that changes what it means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// The normalized command.
    pub command: Command,
    /// Redirections attached to it.
    pub redirects: Vec<Redirect>,
    /// The directory this stage runs in, per the `cd`s before it.
    pub cwd: Cwd,
    /// Whether program names still resolve as analyzed, per the `alias`/`source`
    /// /`eval`/`PATH` changes before it.
    pub bindings: Bindings,
    /// How it joined to the previous stage (`None` for the first).
    pub connector: Option<Operator>,
    /// Byte range in the **original input line** of the command this stage runs —
    /// the part after control-flow scaffolding is peeled (`do rm "$i"` → the span
    /// of `rm "$i"`), so a renderer can highlight the command in situ. For a stage
    /// pulled from a substitution (`<(brew list)`), this is the span of the whole
    /// substitution construct in the outer line (precise inner mapping is a later
    /// refinement).
    pub span: Range<usize>,
    /// Byte range in the original line of each word as written —
    /// assignments, wrappers and the command, index for index with
    /// [`Command::expansion`] — so an act can say which words it stands on
    /// ([`written_index`](Self::written_index)).
    pub word_spans: Vec<Range<usize>>,
    /// **Heredoc bodies this line feeds to stdin** — data, not arguments or
    /// paths, so no path or flag check reads them. The one check that must:
    /// a SECRET HANDLE is expanded wherever it is written, bodies included
    /// (`expand_at_exec` scans the raw line), so the handle gate reads this
    /// too — while heredocs were refused that was implicit, and skipping a
    /// body as "data" would have spent a handle in it unasked. Held by the
    /// last stage of the line: the verdict is the line's worst stage, so any
    /// stage asking is the line asking.
    pub data: Vec<String>,
    /// **An earlier stage of the line set a shell variable** — `export X=…`,
    /// `read X`, a bare `X=…`, any writer (`sets_a_variable`) — so this
    /// command may run with an environment its own words do not show. Sticky,
    /// like [`Cwd::Unknown`]. A regex allow reads only the words, so it does
    /// not cover such a command: `export RIPGREP_CONFIG_PATH=./f; rg foo`
    /// handed rg its `--pre` from a file the model wrote, under an allow that
    /// read `rg foo` (review of c09c1ba, Reviewer B).
    pub env_set: bool,
    /// **An earlier stage of the line set or unset `HOME`** — so a `~` here is
    /// not the home every spelling assumes: `export HOME=/etc; rm ~/passwd`
    /// removes `/etc/passwd`. Sticky, like [`env_set`](Self::env_set). A
    /// prefix assignment does not count: `HOME=/etc rm ~/x` expands `~` before
    /// the assignment applies (review of f50fda5, Reviewer B).
    pub home_set: bool,
}

impl Stage {
    /// The index among the words as written ([`word_spans`](Self::word_spans))
    /// of the command's word `i` (`command.words[i]`). The peel only takes
    /// words off the front — assignments, wrappers and their own words —
    /// and keeps the rest as written, so the command's words are the last
    /// of them.
    pub fn written_index(&self, i: usize) -> usize {
        self.word_spans
            .len()
            .saturating_sub(self.command.words.len())
            + i
    }

    /// The stage's effect: its command's, raised to at least
    /// [`Effect::Mutating`] if it writes to a file via redirection.
    ///
    /// If the name bindings were [`Disturbed`](Bindings::Disturbed) by an
    /// earlier stage, the effect is [`Effect::Unknown`] regardless of what the
    /// command *looks* like — we cannot classify a program we can no longer
    /// identify. This is what closes the `alias rm=... && rm ...` bypass: the
    /// second stage stops being "an `rm` we understand" and becomes "something
    /// we can't vouch for."
    pub fn effect(&self) -> Effect {
        let own = self.command_effect();
        if self.redirects.iter().any(Redirect::writes) {
            own.join(Effect::Mutating)
        } else {
            own
        }
    }

    /// **What the command does, apart from its redirections** — what the
    /// words it names are acted on at; a redirection's target is acted on at
    /// [`Redirect::effect`]. A granted read with `> out.txt` read as a write
    /// of the file it reads (review of f50fda5, Reviewers A and C).
    pub fn command_effect(&self) -> Effect {
        if self.bindings == Bindings::Disturbed {
            Effect::Unknown
        } else {
            self.command.effect
        }
    }
}

/// A decomposed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub stages: Vec<Stage>,
}

impl Pipeline {
    /// The most dangerous effect anywhere in the line — a line is only as safe
    /// as its worst stage. [`Effect::Unknown`] dominates (fail closed).
    pub fn effect(&self) -> Effect {
        self.stages
            .iter()
            .map(Stage::effect)
            .fold(Effect::ReadOnly, Effect::join)
    }

    /// Did we track the working directory for every stage? `false` means some
    /// `cd` was unresolvable, so path-scoped decisions can't be trusted.
    pub fn cwd_fully_known(&self) -> bool {
        !self.stages.iter().any(|s| s.cwd == Cwd::Unknown)
    }

    /// Did every stage's program name resolve as analyzed? `false` means an
    /// `alias`/`source`/`eval`/`PATH` change happened mid-line, so name-based
    /// conclusions about later stages are void.
    pub fn bindings_trusted(&self) -> bool {
        !self
            .stages
            .iter()
            .any(|s| s.bindings == Bindings::Disturbed)
    }
}

/// Decompose a command line into [`Stage`]s, tracking `cd` as it goes.
/// Fails closed on anything [`lex`] refuses, on an empty segment (`a &&`), and
/// on a redirection with no target.
pub fn parse_line(input: &str) -> Result<Pipeline, Unanalyzable> {
    parse_line_at(input, 0)
}

/// Cap on `$(…)`/`` `…` ``/`<(…)` nesting. Each level recurses through
/// [`collect_subst`] → `parse_line_at`; without a cap a deeply-nested
/// substitution overflows the stack and *aborts the process* (which a
/// `catch_unwind` cannot catch) — turning the gate that is meant to REFUSE a
/// dangerous line into one that crashes before rendering any verdict. A genuine
/// one-liner never approaches this; past it we fail closed with a refusal.
const MAX_SUBST_DEPTH: usize = 32;

fn parse_line_at(input: &str, subst_depth: usize) -> Result<Pipeline, Unanalyzable> {
    if subst_depth > MAX_SUBST_DEPTH {
        return Err(Unanalyzable::ShellStructure {
            found: "substitution nested too deep",
        });
    }
    let lexemes = lex(input)?;
    if lexemes.is_empty() {
        return Err(Unanalyzable::Empty);
    }

    let mut stages: Vec<Stage> = Vec::new();
    // Words carry their source span so `finish_stage` can record where the
    // command sits in the original line (spans survive control-flow peeling).
    let mut words: Vec<Spanned<String>> = Vec::new();
    let mut redirects: Vec<Redirect> = Vec::new();
    let mut connector: Option<Operator> = None;
    let mut cwd = Cwd::Start;
    let mut bindings = Bindings::Trusted;
    // Once CDPATH is set, `cd foo` may resolve somewhere else entirely
    // (BashPitfalls #48), so our directory arithmetic stops being sound.
    let mut cdpath_set = false;
    // And once HOME is, so does a bare `cd` or `cd ~`.
    let mut home_set = false;
    // And once any variable is, a later command's environment is not its
    // words' (`Stage::env_set`).
    let mut env_set = false;
    // **Every place the line may be in by now**, whichever of its commands
    // ran, failed or succeeded: where a command runs once the one before it
    // may not have (`;`, a newline, `||`).
    let mut reach = Cwd::Start;
    // How many control-flow constructs we are currently inside. Anything > 0
    // means a command here may run repeatedly (a loop body) or conditionally (a
    // branch), which is exactly when directory arithmetic stops being sound.
    let mut control_depth: i32 = 0;
    // Commands pulled out of `$(…)` / `` `…` `` / `<(…)` / `>(…)` substitutions.
    // They run (in their own subshell), so they are real stages a rule must gate
    // — but their `cd`/bindings don't reach the outer line, so we keep them
    // separate and append them, rather than threading them through the outer
    // directory arithmetic.
    let mut subst_stages: Vec<Stage> = Vec::new();
    // Heredoc bodies, which arrive after their line has ended (`Stage::data`).
    let mut here_bodies: Vec<String> = Vec::new();

    let mut it = lexemes.into_iter().peekable();
    while let Some(Spanned { node, span }) = it.next() {
        match node {
            Lexeme::Word(w) => {
                let mut word = Spanned { node: w, span };
                glue_pieces(input, &mut it, &mut word, subst_depth, &mut subst_stages)?;
                words.push(word);
            }
            Lexeme::HereDoc(body) => here_bodies.push(body),
            // A substitution's inner command is analyzed as its own pipeline and
            // its stages are collected (each spanning its own command, offset into
            // the outer line — see `collect_subst`).
            // The value it splices into the outer command is unknowable, so the
            // outer command gains an unresolved operand — carrying the construct's
            // span so the outer command's own span covers it. Inner refusal
            // propagates (fail closed).
            Lexeme::Subst(inner) => {
                collect_subst(input, &inner, span.clone(), subst_depth, &mut subst_stages)?;
                let mut word = Spanned {
                    node: SUBST_VALUE.to_string(),
                    span,
                };
                glue_pieces(input, &mut it, &mut word, subst_depth, &mut subst_stages)?;
                words.push(word);
            }
            Lexeme::Op(op) if op.is_connector() => {
                // `;` and newline may separate nothing at all (a leading,
                // trailing, or repeated one). `&&`/`||`/`|` may not.
                if words.is_empty() && redirects.is_empty() {
                    if matches!(op, Operator::Seq | Operator::Newline) {
                        connector = Some(op);
                        continue;
                    }
                    return Err(Unanalyzable::ShellStructure {
                        found: "empty command segment",
                    });
                }
                let finished = finish_stage(
                    input,
                    &mut words,
                    &mut redirects,
                    &cwd,
                    bindings,
                    env_set,
                    home_set,
                    connector,
                )?;
                control_depth += finished.depth;
                // A close (`done`/`fi`) with no matching opener drives depth
                // negative — we misread the structure (a stray `done` cancelling
                // an unclosed `for` nets to zero and would otherwise pass). Refuse.
                if control_depth < 0 {
                    return Err(Unanalyzable::ShellStructure {
                        found: "unbalanced control flow",
                    });
                }
                // A `for`'s variable is written as a writer's names are, before
                // the body that runs with it (review 20, Reviewer E2).
                if let Some(name) = &finished.loop_var {
                    cdpath_set |= name == "CDPATH";
                    home_set |= name == "HOME";
                    if super::program::env_hazard(name).is_some() {
                        bindings = Bindings::Disturbed;
                    }
                }
                if let Some(stage) = finished.stage {
                    // Every command in a pipeline runs in its own SUBSHELL, so a
                    // `cd` there never reaches the parent (SubShell / BashFAQ 24).
                    // Propagating it would place later stages in a directory they
                    // are not in — unsound in the dangerous direction. We cannot
                    // model subshell scoping exactly, so it becomes unknowable.
                    let piped = op == Operator::Pipe || stage.connector == Some(Operator::Pipe);
                    // Where the line is if this command ran and succeeded.
                    let after = if (piped || control_depth > 0) && changes_cwd(&stage.command) {
                        Cwd::Unknown
                    } else {
                        next_cwd(&cwd, &stage.command, cdpath_set, home_set)
                    };
                    reach = reach.union(&after);
                    // **A `cd` may fail.** Past `&&` the next command runs only
                    // if this one succeeded — when this one surely ran, which
                    // past an `||` it may not have (`a || cd x && b`), and when
                    // no `!` inverted it (`! cd x && b` runs `b` where `cd x`
                    // FAILED); a pipe's next command runs beside it. Past `;`, a newline or `||`
                    // it runs either way (or only if this failed), so in any
                    // place the line may be in: `cd build/zzz; rm -rf src`
                    // deletes `<start>/src` when there is no `build/zzz`
                    // (review 20, Reviewer E2) — `build/zzz/src` when there is.
                    // That was Unknown: both are places we can name.
                    cwd = match op {
                        Operator::Pipe => after,
                        Operator::And
                            if stage.connector != Some(Operator::Or) && !finished.negated =>
                        {
                            after
                        }
                        _ => reach.clone(),
                    };
                    cdpath_set = cdpath_set || sets_cdpath(&stage.command);
                    home_set = home_set || sets_variable(&stage.command, "HOME");
                    env_set = env_set || sets_a_variable(&stage.command);
                    bindings = next_bindings(bindings, &stage.command);
                    stages.push(stage);
                }
                connector = Some(op);
            }
            Lexeme::Op(op) => {
                // A redirection: the very next lexeme must be its target word.
                let kind = match op {
                    Operator::Write => RedirectKind::Write,
                    Operator::Append => RedirectKind::Append,
                    Operator::Read => RedirectKind::Read,
                    // `2>&1`, `>&-`: a descriptor copied or closed. It names no
                    // file, so there is nothing to record and no target to take.
                    // A heredoc names none either: its body is a later lexeme.
                    Operator::Dup | Operator::HereDoc => continue,
                    _ => unreachable!("connectors handled above"),
                };
                let op_at = span.start;
                match it.next().map(|s| (s.node, s.span)) {
                    // Glued like any word: `>/etc/pass$(echo wd)` writes
                    // `/etc/passwd`, never `/etc/pass`.
                    Some((Lexeme::Word(target), span)) => {
                        let mut target = Spanned { node: target, span };
                        glue_pieces(input, &mut it, &mut target, subst_depth, &mut subst_stages)?;
                        redirects.push(Redirect {
                            kind,
                            target: target.node,
                            span: op_at..target.span.end,
                        });
                    }
                    // A substitution in target position is one of two things,
                    // and the lexer emits the same token for both.
                    //
                    // `< <(cmd)` / `> >(cmd)` is a PROCESS substitution: the
                    // inner command runs, but there is no file to record —
                    // reading a FIFO is not a write.
                    //
                    // `> $(cmd)` / `` > `cmd` `` is a COMMAND substitution, and
                    // it names a file we simply cannot know yet. Treating it
                    // like the first dropped the redirect entirely, so the write
                    // disappeared from `Stage::effect`, `stage_paths`,
                    // `file_words` and `path_indeterminate` at once, and
                    // `echo x > $(cat f)` graded `ReadOnly` and
                    // `safe_unattended` (Reviewer A, HIGH). Record it with the
                    // unresolvable target: the `$` makes `path_indeterminate`
                    // fire for free, and the effect lattice sees the write.
                    // This module already states the rule in the sibling case —
                    // "rather than DROP a write (fail open), refuse".
                    Some((Lexeme::Subst(inner), span)) => {
                        let names_a_file = input
                            .get(span.clone())
                            .is_some_and(|t| t.starts_with("$(") || t.starts_with('`'));
                        collect_subst(input, &inner, span.clone(), subst_depth, &mut subst_stages)?;
                        let mut target = Spanned {
                            node: SUBST_VALUE.to_string(),
                            span,
                        };
                        let alone = target.span.end;
                        glue_pieces(input, &mut it, &mut target, subst_depth, &mut subst_stages)?;
                        // Anything glued on makes it a name, whatever it began as.
                        if names_a_file || target.span.end != alone {
                            redirects.push(Redirect {
                                kind,
                                target: target.node,
                                span: op_at..target.span.end,
                            });
                        }
                    }
                    _ => {
                        return Err(Unanalyzable::ShellStructure {
                            found: "redirect without a target",
                        });
                    }
                }
            }
        }
    }

    if !words.is_empty() || !redirects.is_empty() {
        // The last segment: a `for` it opens assigns for nothing after it.
        let Finished { stage, depth, .. } = finish_stage(
            input,
            &mut words,
            &mut redirects,
            &cwd,
            bindings,
            env_set,
            home_set,
            connector,
        )?;
        control_depth += depth;
        // No negative check here: nothing runs between this point and the
        // `control_depth != 0` refusal below, which catches every negative this
        // could. Deleting it left the suite green — it read as a safety check
        // and was not one, unlike its in-loop twin, which is load-bearing
        // (Reviewer B/A, mutation-checked both ways).
        if let Some(stage) = stage {
            stages.push(stage);
        }
    } else {
        // Nothing after the final separator: fine for `;`/newline, not for `&&`.
        if let Some(op) = connector
            && !matches!(op, Operator::Seq | Operator::Newline)
        {
            return Err(Unanalyzable::ShellStructure {
                found: "empty command segment",
            });
        }
    }
    // An unbalanced construct (`for i in 1; do rm x` with no `done`, or a stray
    // `done`) means we did not read the structure the way the shell will. Our
    // decomposition is only sound if the scaffolding actually balances, so
    // refuse rather than analyze a shape we misread.
    if control_depth != 0 {
        return Err(Unanalyzable::ShellStructure {
            found: "unbalanced control flow",
        });
    }
    // The heredoc bodies go to the line's last stage (`Stage::data` says why
    // any stage will do); a line that is only a substitution has none, and
    // then the substitution's stage holds them.
    if let Some(last) = stages.last_mut().or(subst_stages.last_mut()) {
        last.data.append(&mut here_bodies);
    }
    // The substitutions ran too — fold their stages in. A line that is *only* a
    // substitution (`$(rm x)`) still gates the inner command this way.
    stages.extend(subst_stages);
    if stages.is_empty() {
        return Err(Unanalyzable::Empty);
    }
    Ok(Pipeline { stages })
}

/// The placeholder operand a substitution splices into the outer command. It
/// carries a `$` so [`Command::has_unresolved_operand`](super::Command) treats
/// the outer command's value as unknown — because it is.
const SUBST_VALUE: &str = "$()";

/// The span of a substitution construct's inner content: everything between the
/// `$(`/`` ` ``/`<(`/`>(` opener and the closing delimiter, with surrounding
/// whitespace trimmed. Points a highlight at the producing COMMAND (`find …`)
/// rather than the shell plumbing (`<(…)`) that captures its output. `construct`
/// is a whole-substitution span as the lexer records it (`capture_*` copies the
/// inner verbatim, so these byte offsets land on real char boundaries).
pub fn subst_inner_span(input: &str, construct: &Range<usize>) -> Range<usize> {
    // Only a real construct has an opener to strip. A producer span can also be
    // an ordinary command (`find … | while read f`), and blindly peeling two
    // bytes off that would eat into the command itself — `find . -name x`
    // became `nd . -name`. Guard rather than trust the caller.
    let is_construct = input.get(construct.clone()).is_some_and(|t| {
        t.starts_with("$(") || t.starts_with('`') || t.starts_with("<(") || t.starts_with(">(")
    });
    if !is_construct {
        return construct.clone();
    }
    // Opener is one byte for backticks, two for the paren forms (`$(`/`<(`/`>(`);
    // the closer (`)` or `` ` ``) is always one byte.
    let opener = if input.as_bytes().get(construct.start) == Some(&b'`') {
        1
    } else {
        2
    };
    let start = (construct.start + opener).min(construct.end);
    let end = construct.end.saturating_sub(1).max(start);
    let content = &input[start..end];
    let inner_start = start + (content.len() - content.trim_start().len());
    let inner_end = end - (content.len() - content.trim_end().len());
    inner_start..inner_end.max(inner_start)
}

/// Analyze a substitution's inner command text into stages, appending them to
/// `into`. Empty inner text (`$()`) contributes nothing; anything the inner
/// [`parse_line`] refuses propagates (fail closed) — a dangerous command hidden
/// in a substitution must never slip through as "unanalyzed but allowed."
fn collect_subst(
    input: &str,
    inner: &str,
    construct_span: Range<usize>,
    subst_depth: usize,
    into: &mut Vec<Stage>,
) -> Result<(), Unanalyzable> {
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    // The inner pipeline's spans are relative to `inner`; offset them back into the
    // OUTER line so a renderer highlights the producing COMMAND (`find …`) where it
    // sits — not the `<(…)` plumbing around it. `capture_*` copied the inner
    // verbatim, so `trimmed` aligns byte-for-byte with the outer input at `base`.
    // `depth + 1` bounds the recursion.
    let base = subst_inner_span(input, &construct_span).start;
    let mut inner_stages = parse_line_at(trimmed, subst_depth + 1)?.stages;
    let shift = |r: &Range<usize>| (base + r.start)..(base + r.end);
    for stage in &mut inner_stages {
        stage.span = shift(&stage.span);
        stage.word_spans = stage.word_spans.iter().map(shift).collect();
        for r in &mut stage.redirects {
            r.span = shift(&r.span);
        }
    }
    into.extend(inner_stages);
    Ok(())
}

/// Glue onto `word` every piece that starts where it ends.
///
/// The lexer ends a word at a substitution; the shell does not. `a$(b)c`,
/// `X=$(cmd)` and `--file=$(cmd)` are each ONE word, and `>/etc/pass$(echo
/// wd)` one target. Read as pieces, `X=$(find .) rm -f y` ran a program
/// called `$()` with `rm` as its argument, and that redirect wrote
/// `/etc/pass`. A substitution joins as the unknowable [`SUBST_VALUE`], its
/// command still collected as a stage.
fn glue_pieces(
    input: &str,
    it: &mut std::iter::Peekable<std::vec::IntoIter<Spanned<Lexeme>>>,
    word: &mut Spanned<String>,
    subst_depth: usize,
    subst_stages: &mut Vec<Stage>,
) -> Result<(), Unanalyzable> {
    while let Some(piece) = it.next_if(|n| {
        n.span.start == word.span.end && matches!(n.node, Lexeme::Word(_) | Lexeme::Subst(_))
    }) {
        match piece.node {
            Lexeme::Word(w) => word.node.push_str(&w),
            Lexeme::Subst(inner) => {
                collect_subst(input, &inner, piece.span.clone(), subst_depth, subst_stages)?;
                word.node.push_str(SUBST_VALUE);
            }
            // `next_if` took only the two above.
            Lexeme::Op(_) | Lexeme::HereDoc(_) => {}
        }
        word.span.end = piece.span.end;
    }
    Ok(())
}

/// What one segment turns out to be once control-flow scaffolding is peeled off.
struct Segment {
    /// The words to analyze as a simple command (each with its source span), or
    /// `None` when the segment was pure structure and executes nothing (a `for`
    /// iteration list, a bare `done`).
    words: Option<Vec<Spanned<String>>>,
    /// Net change to control-flow nesting depth (`for`/`while`/`if` open,
    /// `done`/`fi` close).
    depth: i32,
    /// The variable a `for` assigns (`for PATH in .` writes `PATH`), which
    /// stays set after the loop as during it. No stage runs to carry it, so
    /// it is handed to the caller to apply as a writer's names are (review
    /// 20, Reviewer E2).
    loop_var: Option<String>,
    /// Behind a `!`, which inverts its status: an `&&` after it runs when
    /// the command FAILED.
    negated: bool,
}

/// What [`finish_stage`] hands back: the stage the segment runs, if any; its
/// control-flow depth change; the variable a `for` it opens assigns; and
/// whether a `!` inverted its status.
struct Finished {
    stage: Option<Stage>,
    depth: i32,
    loop_var: Option<String>,
    negated: bool,
}

/// Peel control-flow keywords off a segment to reach the command inside it.
///
/// # Why we see through control flow instead of refusing it
///
/// We used to refuse any line containing `for`/`while`/`if`, on the stated
/// grounds that "a loop body runs an unknown number of times over values we
/// cannot know." That reasoning was wrong, and it made us give up on ordinary,
/// completely determinate commands — `for i in {1..9}; do echo "$i"; done` is
/// not mysterious.
///
/// The correction: **for the permission question, iteration values don't
/// matter.** Control flow decides *whether* and *how often* a command runs, not
/// *what that command can do*. So the effect of a compound is the join of the
/// effects of the simple commands inside it, and every one of those is something
/// we already analyze. `for f in *; do rm -rf "$f"; done` is
/// `Destructive` because of the `rm`, and we can say so precisely — naming the
/// real reason instead of shrugging at the loop.
///
/// # The one thing that must not go wrong
///
/// Some positions hold **words** (the `for` iteration list) and others hold
/// **commands** (a `while` condition, any body). Mistaking a command for a word
/// would silently skip it — the only failure direction that is unsafe — so the
/// classification below is explicit per keyword, and anything not enumerated is
/// **refused**, not guessed.
fn peel_control_flow(words: &[Spanned<String>]) -> Result<Segment, Unanalyzable> {
    let mut depth = 0;
    let mut negated = false;
    let mut rest = words;
    loop {
        let Some(first) = rest.first() else {
            // Nothing left: the segment was pure structure (`done`, `then`).
            return Ok(Segment {
                words: None,
                depth,
                loop_var: None,
                negated,
            });
        };
        let Some(kw) = super::program::keyword(&first.node) else {
            // A real command.
            return Ok(Segment {
                words: Some(rest.to_vec()),
                depth,
                loop_var: None,
                negated,
            });
        };
        match kw {
            // Opens a construct whose remainder is a WORD LIST, not commands:
            // nothing in `for i in a b c` executes. (A `$(…)` hiding there is
            // already refused by `lex`, so a command cannot lurk in this list.)
            // But its first word is a variable it WRITES.
            "for" => {
                return Ok(Segment {
                    words: None,
                    depth: depth + 1,
                    loop_var: rest.get(1).map(|w| w.node.clone()),
                    negated,
                });
            }
            // Opens a construct whose remainder IS a command — the condition of
            // a `while`/`until`/`if` runs like any other.
            "while" | "until" | "if" => {
                depth += 1;
                rest = &rest[1..];
            }
            // Closes a construct.
            "done" | "fi" => {
                depth -= 1;
                rest = &rest[1..];
            }
            // Internal markers; whatever follows is a command (or nothing).
            "do" | "then" | "else" | "elif" => rest = &rest[1..],
            // `!` inverts a command's status without changing what it does —
            // and so which of `&&`/`||` runs next (review of 0b04a29,
            // Reviewer A: `! cd X && cmd` runs `cmd` where `cd` FAILED).
            "!" => {
                negated = true;
                rest = &rest[1..];
            }
            // Not modeled yet (`case`/`esac`, `select`, `function`, `coproc`,
            // `{ }` grouping, `[[ ]]`). Refuse honestly rather than guess at
            // which positions hold commands.
            other => return Err(Unanalyzable::ControlFlow { keyword: other }),
        }
    }
}

/// Build one stage from the accumulated words/redirects, clearing them.
/// No stage for a segment that executes nothing (pure control-flow
/// structure); with it, that segment's control-flow depth change and any
/// variable a `for` it opens assigns ([`Finished`]).
#[allow(clippy::too_many_arguments)]
fn finish_stage(
    input: &str,
    words: &mut Vec<Spanned<String>>,
    redirects: &mut Vec<Redirect>,
    cwd: &Cwd,
    bindings: Bindings,
    env_set: bool,
    home_set: bool,
    connector: Option<Operator>,
) -> Result<Finished, Unanalyzable> {
    if words.is_empty() && redirects.is_empty() {
        // `a && `, `| b` — malformed for our purposes; don't guess.
        return Err(Unanalyzable::ShellStructure {
            found: "empty command segment",
        });
    }
    let taken = std::mem::take(words);
    let segment = peel_control_flow(&taken)?;
    let taken_redirects = std::mem::take(redirects);
    let Some(command_words) = segment.words else {
        // Pure structure. A redirect attached to it (`done > out.log`) applies
        // to the whole construct, not to any one command, and we have no stage
        // to hang it on — so rather than DROP a write (fail open), refuse.
        if !taken_redirects.is_empty() {
            return Err(Unanalyzable::ShellStructure {
                found: "redirect on a control-flow construct",
            });
        }
        return Ok(Finished {
            stage: None,
            depth: segment.depth,
            loop_var: segment.loop_var,
            negated: segment.negated,
        });
    };
    if command_words.is_empty() {
        return Err(Unanalyzable::ShellStructure {
            found: "empty command segment",
        });
    }
    // The command's source span: from the first surviving word to the last (the
    // scaffolding peeled above is excluded, so `do rm "$i"` spans just `rm "$i"`).
    // `command_words` is non-empty here, so both ends exist.
    let span = command_words[0].span.start..command_words[command_words.len() - 1].span.end;
    // Read from the words AS WRITTEN, the one place their quoting survives.
    let splits: Vec<bool> = command_words
        .iter()
        .map(|w| {
            input
                .get(w.span.clone())
                .is_some_and(super::splits_unquoted)
        })
        .collect();
    // And whether each still expands: `'*.rs'` reaches the program as text.
    let live: Vec<bool> = command_words
        .iter()
        .map(|w| {
            input
                .get(w.span.clone())
                .is_none_or(super::expands_as_written)
        })
        .collect();
    let word_spans: Vec<Range<usize>> = command_words.iter().map(|s| s.span.clone()).collect();
    let node_words: Vec<String> = command_words.into_iter().map(|s| s.node).collect();
    let mut command = normalize_as_written(&node_words, &splits);
    command.expansion = node_words.into_iter().zip(live).collect();
    Ok(Finished {
        stage: Some(Stage {
            command,
            redirects: taken_redirects,
            cwd: cwd.clone(),
            bindings,
            connector,
            span,
            word_spans,
            data: Vec::new(),
            env_set,
            home_set,
        }),
        depth: segment.depth,
        loop_var: segment.loop_var,
        negated: segment.negated,
    })
}

/// Does this command move the working directory? Inside a loop or a conditional
/// that makes the location unknowable — a loop body's `cd` compounds every
/// iteration, and a branch's `cd` may or may not have happened at all — so the
/// caller degrades [`Cwd`] to [`Cwd::Unknown`] rather than doing arithmetic on a
/// directory the command may not be in.
///
/// `eval` and `source`/`.` count: they may run a `cd` we never see, so a deny
/// on `/etc/*` was lost to `eval cd /etc && rm passwd` (review 20, Reviewer
/// E2); [`next_cwd`] makes the place unknown after them.
fn changes_cwd(command: &Command) -> bool {
    matches!(
        command.program(),
        "cd" | "pushd" | "popd" | "eval" | "source" | "."
    )
}

/// Whether program names still resolve as analyzed *after* running `command`.
///
/// The commands that break the name→program mapping: `alias`/`unalias` (rebind a
/// name), `eval` (runs constructed text), `source`/`.` (may define anything),
/// a shell function definition, `hash` (rebinds the lookup cache), and any
/// change to `PATH` (a different `rm` earlier in the path wins). Once disturbed,
/// it stays disturbed — we have no way to prove the mapping was restored.
fn next_bindings(current: Bindings, command: &Command) -> Bindings {
    if current == Bindings::Disturbed {
        return Bindings::Disturbed; // sticky
    }
    let program = command.program();
    // Always rebinding.
    if matches!(
        program,
        "alias" | "unalias" | "eval" | "source" | "." | "function" | "hash"
    ) {
        return Bindings::Disturbed;
    }
    // A BARE assignment (`PATH=/tmp` with no command) persists in the shell, so
    // a hazardous variable poisons every later stage. The prefixed form
    // (`PATH=/tmp ls`) is scoped to that one command and is handled by its
    // effect instead — the distinction matters, or we would over-fire on the
    // extremely common `FOO=1 cmd`.
    if program.is_empty()
        && command
            .env
            .keys()
            .any(|k| super::program::env_hazard(k).is_some())
    {
        return Bindings::Disturbed;
    }
    // A builtin that REBINDS a shell variable poisons later name resolution when
    // it targets a HAZARDOUS one — `read PATH < evil; ls` and
    // `export LD_PRELOAD=/e.so && ls` both make `ls` run attacker code while the
    // writer itself touches no files (a pure BINDINGS-axis danger). We route the
    // whole var-writer family through ONE hazard oracle (`writes_hazard_var`), so
    // we fix the CLASS, not the symptom: the old `"PATH"` substring test silently
    // missed `LD_PRELOAD`/`IFS`/`DYLD_*`/… — a false, un-prompted ALLOW.
    if writes_hazard_var(command) {
        return Bindings::Disturbed;
    }
    Bindings::Trusted
}

/// Does this builtin REBIND a shell variable whose name is a hazard
/// ([`env_hazard`](super::program::env_hazard) — `PATH`/`IFS`/`LD_*`/…)? Such a
/// rebind changes how a LATER stage's name resolves to a program even though the
/// builtin itself touches no files, so the danger lives purely on the BINDINGS
/// axis. Routing the entire family through ONE oracle means a newly modeled
/// builtin can't silently reopen the `read PATH < evil; ls` RCE — the exact
/// regression that bit us when `read` was first modeled ReadOnly (a program
/// modeled ReadOnly is a *safety assertion*, and dropping its fail-closed
/// `Unknown` can unmask a hole unless the bindings axis is also considered).
///
/// How each writer names its target (all captured by scanning operands, splitting
/// `NAME=value` to the bare `NAME`):
/// - `export`/`declare`/`typeset`/`set` name it as `NAME=value` or a bare `NAME`;
/// - `read`/`getopts` take the bare `NAME`; `let` takes `NAME=expr`;
/// - `printf -v NAME` names it, gated on the `-v` flag so plain `printf '%s' PATH`
///   (data, no variable written) is left alone;
/// - `mapfile`/`readarray` take the `NAME`.
///
/// **A flag's value can BE the target.** This used to say that a flag's value
/// never reaches here, because "modeled flags consume it" — true of
/// `read -d PATH VAR`, where `PATH` is the delimiter, and false of
/// `read -a PATH`, where the array NAME is precisely the variable written. `-a`
/// is modelled `takes_value: true`, so the name was consumed as the flag's
/// value and never scanned, and bindings stayed Trusted while `PATH` was
/// redirected (Reviewer A). For a writer program we now scan flag values too.
///
/// Scanning *every* flag value would over-read: `read -d PATH VAR` uses `PATH`
/// as a delimiter, and a precision test pins that it must NOT disturb. So the
/// question "does this flag name a variable" is asked of the program model —
/// [`names_a_variable`](super::program::names_a_variable) — rather than
/// answered at this call site, which is where the same kind of knowledge
/// already lives for hazards, wrappers and credential shapes.
/// `CDPATH` is deliberately absent — it is a CWD hazard, handled by
/// [`sets_cdpath`], not a program-resolution one.
fn writes_hazard_var(command: &Command) -> bool {
    variables_written(command)
        .into_iter()
        .any(|name| super::program::env_hazard(name).is_some())
}

/// **The shell variables a builtin writes** — set, assigned, read into, or
/// unset — by name. The one list of writers, for both questions asked of it:
/// does the line move a name the program search reads ([`writes_hazard_var`]),
/// and does it move `HOME` or `CDPATH` ([`sets_variable`]).
///
/// There were two lists, and they disagreed both ways (review 20, Reviewer
/// A): the hazard check knew `read`/`getopts`/`let`/`mapfile`/`printf -v`
/// and not `readonly`/`unset`; the `cd` check the reverse — so `read HOME <
/// f; cd` went to the real home, and `unset PATH` left the names trusted.
///
/// A name is a word's LEADING IDENTIFIER — `PATH` in `PATH=x`, `PATH+=x`,
/// `PATH[0]`, `PATH++` — and every identifier of a `let`'s expressions
/// (`let x=PATH=1`). And any word of any command writes one with a default
/// (`${CDPATH:=/etc}`). Each spelling left the names trusted, and a `for`
/// loop's variable too (`Segment::loop_var`; review 20, Reviewer E2).
/// Over-reading is the safe direction: `getopts`'s option string and `unset
/// -f`'s function name are read as names too, and match nothing that matters.
fn variables_written(command: &Command) -> Vec<&str> {
    let program = command.program();
    let is_writer = matches!(
        program,
        "export"
            | "declare"
            | "typeset"
            | "set"
            | "readonly"
            | "unset"
            | "read"
            | "getopts"
            | "let"
            | "mapfile"
            | "readarray"
            | "wait"
    ) || (program == "printf"
        && command.unknown.iter().any(|f| f.starts_with("-v")));
    let mut names: Vec<&str> = Vec::new();
    if is_writer {
        let words = command
            .operands
            .iter()
            .map(String::as_str)
            .chain(command.unknown.iter().map(String::as_str))
            // `printf -vPATH`: the name attached to `-v` (SECURITY-2026-09-30
            // R2-11 — only the separate `-v PATH` was seen).
            .chain(
                command
                    .unknown
                    .iter()
                    .filter(|_| program == "printf")
                    .filter_map(|f| f.strip_prefix("-v")),
            )
            .chain(
                command
                    .flags
                    .iter()
                    .filter(|f| super::program::names_a_variable(program, &f.name))
                    .filter_map(|f| f.value.as_deref()),
            );
        for word in words {
            if program == "let" {
                names.extend(identifiers(word));
            } else {
                names.extend(leading_identifier(word));
            }
        }
    }
    for word in command.words.iter().chain(command.env.values()) {
        names.extend(defaulted(word));
    }
    names
}

/// The identifier a word begins with, if it begins with one.
fn leading_identifier(word: &str) -> Option<&str> {
    let end = word
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(word.len());
    let name = &word[..end];
    (!name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit())).then_some(name)
}

/// Every identifier in a word — for `let`, whose expressions name variables
/// anywhere (`let x=PATH=1`, `let 'PATH = 5'`).
fn identifiers(word: &str) -> impl Iterator<Item = &str> {
    word.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter_map(leading_identifier)
}

/// The variables a word assigns by default: `${NAME:=…}`, `${NAME=…}`, an
/// element's too — `param::assigned`, the one parse of a `${…}`.
fn defaulted(word: &str) -> impl Iterator<Item = &str> {
    super::param::assigned(word)
}

/// Does this command set (or unset) the shell variable `name` — `CDPATH`,
/// which makes a later `cd foo` unpredictable, or `HOME`, which moves where
/// a bare `cd` and `cd ~` go? An assignment (`HOME=/x`, as a prefix or a
/// command of its own) or a builtin that writes one ([`variables_written`]);
/// `unset` counts, since what the variable then holds is not what we assumed
/// either. By the exact name: `HOMEBREW_PREFIX=x` is not `HOME`.
fn sets_variable(command: &Command, name: &str) -> bool {
    command.env.contains_key(name)
        || variables_written(command)
            .into_iter()
            .any(|written| written == name)
}

/// Does this command set `CDPATH`, making later `cd`s unpredictable?
fn sets_cdpath(command: &Command) -> bool {
    sets_variable(command, "CDPATH")
}

/// **Does this command set a shell variable a LATER command may run with?**
/// A bare assignment (`X=1` with no command) or any writer
/// ([`variables_written`]). Not a prefixed one (`X=1 make`): that is scoped
/// to its own command, and every later one is clean of it.
fn sets_a_variable(command: &Command) -> bool {
    (command.program().is_empty() && !command.env.is_empty())
        || !variables_written(command).is_empty()
}

/// `~` or `~/…` with nothing else that expands — the home directory and the
/// rest of the path, which bash resolves without asking anything we cannot
/// know. `~user` (another user's home) and `~/$X` are not.
fn home_relative(arg: &str) -> Option<&str> {
    let rest = arg.strip_prefix('~')?;
    ((rest.is_empty() || rest.starts_with('/')) && !super::expands(rest)).then_some(rest)
}

/// The working directory *after* running `command`, given the current one.
///
/// `home_set`: an earlier stage set or unset `HOME`, so a bare `cd` and `cd
/// ~` go somewhere we cannot say.
fn next_cwd(current: &Cwd, command: &Command, cdpath_set: bool, home_set: bool) -> Cwd {
    // `pushd`/`popd` change the working directory exactly like `cd`, but we
    // don't model the directory stack — so treat them as making the location
    // unknowable rather than passing the cwd through unchanged (which would let
    // `pushd / && rm -rf *` look like it runs where the line started).
    // (Reviewer A.) And `eval`/`source`/`.`, which may run a `cd` we never
    // see (review 20, Reviewer E2).
    if command.program() != "cd" && changes_cwd(command) {
        return Cwd::Unknown;
    }
    if command.program() != "cd" {
        return current.clone();
    }
    // With CDPATH set — by an earlier stage OR by this cd's own prefix
    // (`CDPATH=/x cd foo`, scoped to exactly this cd) — the target is
    // unpredictable from the path alone (BashPitfalls #48). (Reviewer E.)
    if cdpath_set || command.env.contains_key("CDPATH") {
        return Cwd::Unknown;
    }
    let Some(places) = current.places() else {
        return Cwd::Unknown; // sticky
    };
    // **A bare `cd` is `cd ~`** (Steven, 2026-10-03), and `~` is a place we
    // know: the home directory, absolute, so a later `..` climbs from it
    // rather than from where the line started. Unknown when this line moved
    // `HOME` (`HOME=/x cd`, an earlier `export HOME=…`), or there is none.
    // `cd a b` is still not something we resolve.
    let arg = match command.operands.as_slice() {
        [] => "~",
        [arg] => arg.as_str(),
        _ => return Cwd::Unknown,
    };
    if let Some(rest) = home_relative(arg) {
        if home_set || command.env.contains_key("HOME") {
            return Cwd::Unknown;
        }
        return crate::secrets::home_dir().map_or(Cwd::Unknown, |home| {
            Cwd::Resolved(normalize_path(&format!("{home}{rest}")))
        });
    }
    // `cd -`, or anything still holding an expansion/glob (`~user $ * ? [ {`):
    // we don't know the target. Uses the shared `expands` predicate so this
    // can't drift from `has_unresolved_operand` (it had been missing `[` and
    // `{`). (Reviewers D, E.)
    if arg == "-" || super::expands(arg) {
        return Cwd::Unknown;
    }
    if arg.starts_with('/') {
        return Cwd::Resolved(normalize_path(arg));
    }
    // Relative: from each place the line may be in, so from several it may
    // land in several (`cd a; cd b` is `a/b` or `b`).
    places
        .iter()
        .map(|place| relative_cd(place, arg))
        .fold(None, |all: Option<Cwd>, here| {
            Some(all.map_or(here.clone(), |all| all.union(&here)))
        })
        .unwrap_or(Cwd::Unknown)
}

/// Where `cd <arg>`, `arg` relative, goes from `place` (a `Start` or a
/// `Resolved`).
fn relative_cd(place: &Cwd, arg: &str) -> Cwd {
    let joined = match place {
        Cwd::Resolved(base) if !base.is_empty() => format!("{base}/{arg}"),
        _ => arg.to_string(),
    };
    // **Back where the line started is `Start`**, never `Resolved("")`: `cd
    // .`, `cd ""` and `cd src && cd ..` normalise to the empty path, and a
    // reader joining a token to it built `/tmp/x` out of `tmp/x` — so `allow
    // rm /tmp/**` covered `cd . && rm -rf tmp/x`, the project's own file
    // (review 20, Reviewers A and B).
    match normalize_path(&joined) {
        here if here.is_empty() => Cwd::Start,
        here => Cwd::Resolved(here),
    }
}

/// **The one spelling a path is compared in.** Collapses `//`, `/./`, `..` and
/// a trailing `/`, textually, and takes the system's own links at the root
/// to where they lead (`/tmp/x` is `/private/tmp/x` on macOS).
///
/// Lexical but for that one table ([`root_links`], read once): we do not
/// touch the filesystem per path, so a link further down is not followed (a
/// documented limitation: a `cd` through a symlink can land somewhere else
/// than this path suggests).
///
/// **The root links are part of the spelling**, because the two forms name
/// one file and a rule is matched by text: under `deny * /etc/**`, `Read
/// /private/etc/hosts` asked, and under `deny :* vault/**` in a project at
/// `/private/tmp/p`, `cat /tmp/p/vault/x` did not deny (review stop 17, E2 2
/// and V H1, H2). Always toward the target: a link has one target, while one
/// target can have several links (`/bin` and `/sbin` on a merged Linux), and
/// it is the spelling `getcwd` and `canonicalize` give.
///
/// **Every predicate that answers "which file is this token" must call this,
/// on BOTH the rule and the operand.** It was private and wired to `cd`
/// targets alone, so nothing normalised the operands that rules match:
/// `cat //srv/prod/x` was allowed where `cat /srv/prod/x` was denied, and
/// `~/.docker//config.json` read back marked safe-to-run-unattended. A
/// one-character edit after a refusal turned a deny into an auto-run.
///
/// The trap worth naming: the gate already had a name for one member of this
/// class — `..` was named (`PathEscape`, in the old engine) — and naming it is what
/// convinced everyone the class was closed. `.` and `//` are the same class
/// and were never enumerated
/// ([[project_shannon_gate_normalisation]], [[feedback_second_spelling]]).
pub(crate) fn normalize_path(p: &str) -> String {
    normalize_among(p, root_links())
}

/// [`normalize_path`] over a table of root links, so a test can name its own.
pub(crate) fn normalize_among(p: &str, links: &[RootLink]) -> String {
    let absolute = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for comp in p.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !absolute {
                    // Above the starting directory; keep it explicit.
                    parts.push("..");
                }
                // At an absolute root, `..` stays at the root.
            }
            // As the kernel walks it, a name at the root at the moment it
            // is reached: `/tmp/..` is `/private`, and `/x/../tmp` is a link.
            c if absolute && parts.is_empty() => match root_link(c, links) {
                Some(to) => parts.extend(to.split('/').filter(|s| !s.is_empty())),
                None => parts.push(c),
            },
            c => parts.push(c),
        }
    }
    if absolute {
        format!("/{}", parts.join("/"))
    } else {
        parts.join("/")
    }
}

/// A link in `/`: its name, and where it leads, spelled through any other.
pub(crate) type RootLink = (String, String);

/// Where the root link named `name` leads. Without case on macOS, whose
/// file system folds it: `/ETC/hosts` opens `/private/etc/hosts`, and read
/// as a name of its own it was under no rule.
fn root_link<'a>(name: &str, links: &'a [RootLink]) -> Option<&'a str> {
    let same = |link: &str| match cfg!(target_os = "macos") {
        true => link.eq_ignore_ascii_case(name),
        false => link == name,
    };
    (links.iter())
        .find(|(link, _)| same(link))
        .map(|(_, to)| to.as_str())
}

/// **The system's own links at the root.** Read once, by `readlink` alone:
/// a root link can lead to an automounted directory (`/home` on macOS),
/// which a `stat` would wait on. Only root makes an entry in `/`, so none
/// is a link a model or a repository planted.
pub(crate) fn root_links() -> &'static [RootLink] {
    static LINKS: std::sync::OnceLock<Vec<RootLink>> = std::sync::OnceLock::new();
    LINKS.get_or_init(|| {
        let root = std::path::Path::new("/");
        let Ok(entries) = std::fs::read_dir(root) else {
            return Vec::new();
        };
        settled(
            (entries.flatten())
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_symlink()))
                .filter_map(|entry| {
                    let to = root.join(std::fs::read_link(entry.path()).ok()?);
                    Some((entry.file_name().to_str()?.to_string(), to.to_str()?.into()))
                })
                .collect(),
        )
    })
}

/// Each link's target spelled through the others, so a path takes one step
/// however the links chain (`/a` → `/tmp/x`, `/tmp` → `/private/tmp`). A
/// pass per link is the longest chain; a cycle stops there, and names
/// nothing a file could be opened by.
fn settled(mut links: Vec<RootLink>) -> Vec<RootLink> {
    for _ in 0..links.len() {
        links = (links.iter())
            .map(|(name, to)| (name.clone(), normalize_among(to, &links)))
            .collect();
    }
    links
}

/// **Every other spelling of `full` through a root link** (`/tmp/x` for
/// `/private/tmp/x`; `/bin/ls` and `/sbin/ls` for `/usr/bin/ls`): for a
/// regex, which is text no door can respell, so it is shown the path each
/// way one may name it.
pub(crate) fn link_spellings(full: &str) -> impl Iterator<Item = String> + '_ {
    root_links().iter().filter_map(move |(name, to)| {
        let rest = full.strip_prefix(to.as_str())?;
        (rest.is_empty() || rest.starts_with('/')).then(|| format!("/{name}{rest}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(s: &str) -> Pipeline {
        parse_line(s).expect("analyzable")
    }

    /// A stage's span re-slices the ORIGINAL line to the command it runs, with
    /// control-flow scaffolding excluded — the property the annotated-source
    /// renderer needs to highlight `rm "$i"` in place, not the whole loop.
    #[test]
    fn stage_span_reslices_to_the_command_without_scaffolding() {
        let input = r#"for i in {1..9}; do rm "$i"; done"#;
        let p = line(input);
        assert_eq!(
            p.stages.len(),
            1,
            "only `rm` executes; for/do/done are structure"
        );
        assert_eq!(&input[p.stages[0].span.clone()], r#"rm "$i""#);
    }

    /// The lexer ends a word at a substitution; the shell does not. Pieces with
    /// no gap between them are ONE word — an assignment, an operand, a flag's
    /// value, a redirect target — and every substitution among them still
    /// runs as a stage of its own.
    #[test]
    fn a_word_glued_to_a_substitution_is_one_word() {
        let p = line("X=$(find .) rm -f y");
        let rm = &p.stages[0].command;
        assert_eq!(rm.program(), "rm", "not a program called `$()`");
        assert!(rm.env.contains_key("X"));
        assert_eq!(rm.operands, vec!["y"]);

        let p = line("rm -f a$(echo b)c");
        assert_eq!(p.stages[0].command.operands, vec!["a$()c"]);
        assert!(p.stages.iter().any(|s| s.command.program() == "echo"));

        let p = line("tar -x --file=$(echo a.tar)");
        let values: Vec<&str> = p.stages[0].command.flag_values("file").collect();
        assert_eq!(values, vec!["$()"]);

        let p = line("echo x >/etc/pass$(echo wd)");
        assert_eq!(p.stages[0].redirects[0].target, "/etc/pass$()");
        assert!(p.stages[0].command.operands.iter().all(|o| o == "x"));
        // A process substitution glued to more is a name, not a pipe.
        let p = line("cat < <(ls)x");
        assert_eq!(
            p.stages[0].redirects.len(),
            1,
            "{:?}",
            p.stages[0].redirects
        );
        // A gap is still a gap.
        let p = line("rm -f a $(echo b)");
        assert_eq!(p.stages[0].command.operands, vec!["a", "$()"]);
    }

    /// A pipeline highlights each stage where it sits.
    #[test]
    fn each_pipe_stage_spans_its_own_command() {
        let input = "cat file | grep foo | wc -l";
        let p = line(input);
        let spans: Vec<&str> = p.stages.iter().map(|s| &input[s.span.clone()]).collect();
        assert_eq!(spans, vec!["cat file", "grep foo", "wc -l"]);
    }

    /// A command pulled from a process substitution carries the span of the inner
    /// COMMAND in the OUTER line (not the `<(…)` plumbing around it), so a renderer
    /// highlights the producer where it appears without painting the delimiters.
    #[test]
    fn subst_stage_spans_the_inner_command_in_the_outer_line() {
        let input = "sort < <(find . -type f)";
        let p = line(input);
        // Two stages: the outer `sort`, and `find` from the substitution.
        let find = p
            .stages
            .iter()
            .find(|s| s.command.program() == "find")
            .expect("find stage");
        // The `find …` command, not the `<(…)` construct.
        assert_eq!(&input[find.span.clone()], "find . -type f");

        // Leading whitespace inside the construct is trimmed away too.
        let padded = line("sort < <(  find .  )");
        let find = padded
            .stages
            .iter()
            .find(|s| s.command.program() == "find")
            .expect("find stage");
        assert_eq!(&"sort < <(  find .  )"[find.span.clone()], "find .");
    }

    /// The case that makes decomposition-without-state unsound: same shape,
    /// opposite meaning. We must be able to tell these apart.
    #[test]
    fn cd_changes_what_a_later_stage_means() {
        let dangerous = line("cd / && rm -rf x");
        let fine = line("cd /tmp/scratch && rm -rf x");
        assert_eq!(dangerous.stages[1].cwd, Cwd::Resolved("/".into()));
        assert_eq!(
            fine.stages[1].cwd,
            Cwd::Resolved(normalize_path("/tmp/scratch"))
        );
        // Both are destructive; it is the *directory* that distinguishes them,
        // which is exactly what a path-scoped rule needs.
        assert_eq!(dangerous.effect(), Effect::Destructive);
        assert_eq!(fine.effect(), Effect::Destructive);
    }

    /// The everyday case that should stop prompting: every stage analyzable.
    #[test]
    fn the_cd_do_work_cd_back_pattern_decomposes() {
        let p = line("cd api && ls -l && cd ..");
        assert_eq!(p.stages.len(), 3);
        assert_eq!(p.stages[0].cwd, Cwd::Start);
        assert_eq!(p.stages[1].cwd, Cwd::Resolved("api".into()));
        assert_eq!(p.stages[2].cwd, Cwd::Resolved("api".into()));
        assert!(p.cwd_fully_known());
        // `ls` is read-only and `cd` is harmless → the whole line is read-only.
        assert_eq!(p.effect(), Effect::ReadOnly);
    }

    #[test]
    fn relative_cds_compose_and_dotdot_pops() {
        let p = line("cd a && cd b && cd .. && pwd");
        assert_eq!(p.stages[1].cwd, Cwd::Resolved("a".into()));
        assert_eq!(p.stages[2].cwd, Cwd::Resolved("a/b".into()));
        assert_eq!(p.stages[3].cwd, Cwd::Resolved("a".into()));
    }

    /// **Back where the line started is `Start`** — the one writer that can
    /// produce an empty directory, and the only guard against one: no
    /// reader re-checks (review 20, Reviewers A and B).
    #[test]
    fn a_cd_back_to_the_start_is_the_start() {
        for text in [
            "cd . && pwd",
            "cd ./ && pwd",
            "cd '' && pwd",
            "cd src/.. && pwd",
            "cd src && cd .. && pwd",
        ] {
            let p = line(text);
            assert_eq!(p.stages.last().unwrap().cwd, Cwd::Start, "{text}");
        }
    }

    #[test]
    fn an_unresolvable_cd_makes_later_stages_unknown_and_is_sticky() {
        let p = line("cd $DIR && rm -rf x && cd sub");
        assert_eq!(p.stages[0].cwd, Cwd::Start);
        assert_eq!(p.stages[1].cwd, Cwd::Unknown);
        assert_eq!(p.stages[2].cwd, Cwd::Unknown, "Unknown must be sticky");
        assert!(!p.cwd_fully_known());
    }

    /// **A bare `cd` is `cd ~`, and `~` is a place we know** (Steven,
    /// 2026-10-03; it was Unknown) — the child inherits our `HOME`
    /// (`exec.rs`'s allowlist), so it lands where `home_dir` says. Absolute,
    /// so `..` climbs from home and not from where the line started. Unknown
    /// once the line moves `HOME`, and for anyone else's home.
    #[test]
    fn bare_cd_goes_home_unless_the_line_moved_it() {
        let home = crate::secrets::home_dir().expect("tests run with a HOME");
        for (text, want) in [
            ("cd && rm -rf x", home.to_string()),
            ("cd ~ && rm -rf x", home.to_string()),
            ("cd ~/notes && rm -rf x", format!("{home}/notes")),
            (
                "cd ~ && cd .. && rm -rf x",
                normalize_path(&format!("{home}/..")),
            ),
        ] {
            let p = line(text);
            assert_eq!(p.stages.last().unwrap().cwd, Cwd::Resolved(want), "{text}");
        }
        for text in [
            "HOME=/tmp cd && rm -rf x",
            "export HOME=/tmp && cd && rm -rf x",
            "HOME=/tmp; cd ~ && rm -rf x",
            "unset HOME; cd && rm -rf x",
            "readonly HOME=/tmp; cd && rm -rf x",
            "cd ~bob && rm -rf x",
            "cd ~/$X && rm -rf x",
            // Every writer of the one list moves it (review 20, Reviewer A).
            "read HOME < f; cd && rm -rf x",
            "printf -v HOME /tmp; cd && rm -rf x",
            "declare HOME+=/x; cd && rm -rf x",
        ] {
            let p = line(text);
            assert_eq!(p.stages.last().unwrap().cwd, Cwd::Unknown, "{text}");
        }
        // A name that only BEGINS like `HOME` is another variable.
        let p = line("export HOMEBREW_PREFIX=/opt; cd && rm -rf x");
        assert_eq!(
            p.stages.last().unwrap().cwd,
            Cwd::Resolved(home.to_string())
        );
    }

    /// **One list of writers, for both questions** — the names a program
    /// search reads are moved by the same builtins that move `HOME`.
    /// `unset` and `readonly` were absent from the hazard list.
    #[test]
    fn every_writer_disturbs_the_names_it_moves() {
        for text in [
            "unset PATH; ls",
            "readonly PATH=/tmp; ls",
            "read PATH < f; ls",
        ] {
            let p = line(text);
            assert_eq!(
                p.stages.last().unwrap().bindings,
                Bindings::Disturbed,
                "{text}"
            );
        }
        let p = line("unset FOO; ls");
        assert_ne!(p.stages.last().unwrap().bindings, Bindings::Disturbed);
    }

    /// **A name is written however it is spelled** — a subscript, an
    /// operator, a chained `let`, a `${NAME:=…}` default in any word — and
    /// a `for` loop's variable is written too (review 20, Reviewer E2: each
    /// left the names trusted, and `for PATH in .; do ls; done` ran a
    /// project's `./ls` unattended).
    #[test]
    fn every_spelling_of_a_write_is_one() {
        for text in [
            "read 'PATH[0]' < f; ls",
            "let PATH++; ls",
            "let 'PATH = 5'; ls",
            "let x=PATH=1; ls",
            "echo ${PATH:=/tmp} > /dev/null; ls",
            // An element is the variable (review, Reviewer C: the default's
            // own parse knew no subscript).
            "echo ${PATH[0]:=/tmp} > /dev/null; ls",
            "echo ${X:-${PATH=/tmp}} > /dev/null; ls",
            "for PATH in .; do ls; done",
        ] {
            let p = line(text);
            assert_eq!(
                p.stages.last().unwrap().bindings,
                Bindings::Disturbed,
                "{text}"
            );
        }
        let p = line("for i in a b; do ls; done");
        assert_eq!(
            p.stages.last().unwrap().bindings,
            Bindings::Trusted,
            "premise"
        );
        for text in [
            "for HOME in /etc; do true; done; cd && rm -rf x",
            "for CDPATH in /etc; do true; done; cd ssh && cat x",
            "echo ${CDPATH:=/etc} > /dev/null; cd ssh && cat x",
            "echo ${CDPATH[0]:=/etc} > /dev/null; cd ssh && cat x",
        ] {
            let p = line(text);
            assert_eq!(p.stages.last().unwrap().cwd, Cwd::Unknown, "{text}");
        }
    }

    /// **A `cd` that may have failed leaves the line in one of two places** —
    /// after `;`, a newline or `||`, the next command runs whether the `cd`
    /// succeeded or not (or ONLY if it failed), so it may run where the line
    /// began: `cd build/zzz; rm -rf src` deletes `<start>/src` when there is
    /// no `build/zzz` (review 20, Reviewer E2), `build/zzz/src` when there
    /// is. Both are places we can name, and it was `Unknown`, which asked of
    /// every such line (Steven, 10-05: "We should be tracking the cwd"). After
    /// `&&` it ran only if the `cd` did.
    #[test]
    fn a_cd_that_may_have_failed_leaves_the_line_in_one_of_two_places() {
        let either = |dir: &str| Cwd::OneOf(vec![Cwd::Start, Cwd::Resolved(dir.into())]);
        for (text, want) in [
            ("cd build/zzz; rm -rf src", either("build/zzz")),
            ("cd build/zzz || rm -rf src", either("build/zzz")),
            ("cd build/zzz\nrm -rf src", either("build/zzz")),
            ("cd build && ls; rm -rf src", either("build")),
            ("cd build && ls || rm -rf src", either("build")),
            // Past an `||`, an `&&`'s left side may not have run at all.
            ("ls || cd x && rm y", either("x")),
            // A `!` inverts it: past `&&`, the `cd` FAILED (review of
            // 0b04a29, Reviewer A).
            ("! cd x && rm y", either("x")),
            // Every place each `cd` may have left it.
            (
                "cd a; cd b; ls",
                Cwd::OneOf(vec![
                    Cwd::Start,
                    Cwd::Resolved("a".into()),
                    Cwd::Resolved("b".into()),
                    Cwd::Resolved("a/b".into()),
                ]),
            ),
            // An absolute `cd` lands in one place from any.
            ("cd a; cd /tmp && ls", Cwd::Resolved(normalize_path("/tmp"))),
            // One we cannot follow is still sticky.
            ("cd $D; cd a; ls", Cwd::Unknown),
            ("cd a; cd $D; ls", Cwd::Unknown),
        ] {
            let p = line(text);
            assert_eq!(p.stages.last().unwrap().cwd, want, "{text:?}");
        }
        // **Bounded**: each relative `cd` that may fail doubles the places,
        // so past `MOST_PLACES` the line is Unknown — and it is decided in
        // bounded time (review of 0b04a29, Reviewer C: 2^k places, seconds on
        // the UI thread). Three `cd`s are 8 places; four would be 16.
        let three = line("cd a; cd b; cd c; ls");
        assert_eq!(
            three.stages.last().unwrap().cwd.places().map(<[_]>::len),
            Some(MOST_PLACES)
        );
        assert_eq!(
            line("cd a; cd b; cd c; cd d; ls")
                .stages
                .last()
                .unwrap()
                .cwd,
            Cwd::Unknown
        );
        // Twelve, so without the bound this fails in seconds rather than hangs.
        let long: String = (0..12).map(|n| format!("cd d{n}; make; cd ..; ")).collect();
        let started = std::time::Instant::now();
        let p = line(&format!("{long}ls"));
        let _ = crate::permission::RuleSet::defaults()
            .with_start("/home/me/proj")
            .decide(&p);
        assert_eq!(p.stages.last().unwrap().cwd, Cwd::Unknown);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        for (text, want) in [
            ("cd build && rm -rf src", "build"),
            ("cd /tmp && cd a && ls", "/tmp/a"),
            ("ls; cd build && rm x", "build"),
        ] {
            let p = line(text);
            assert_eq!(
                p.stages.last().unwrap().cwd,
                Cwd::Resolved(normalize_path(want)),
                "{text}"
            );
        }
    }

    /// **A root link is taken where the walk reaches it**, as the kernel
    /// does: at the root, whatever came before, and a `..` after it climbs
    /// the target. Only a name at the root, and only that whole name.
    #[test]
    fn a_root_link_is_taken_where_the_walk_reaches_it() {
        let links: Vec<RootLink> = vec![("tmp".into(), "/private/tmp".into())];
        let n = |p: &str| normalize_among(p, &links);
        for (given, want) in [
            ("/tmp/x", "/private/tmp/x"),
            ("//tmp//x/", "/private/tmp/x"),
            ("/tmp", "/private/tmp"),
            ("/tmp/..", "/private"),
            ("/tmp/../etc/x", "/private/etc/x"),
            ("/a/../tmp/x", "/private/tmp/x"),
            ("/a/tmp/x", "/a/tmp/x"),
            ("/tmpx/y", "/tmpx/y"),
            ("tmp/x", "tmp/x"),
            ("/private/tmp/x", "/private/tmp/x"),
        ] {
            assert_eq!(n(given), want, "{given}");
            assert_eq!(n(want), want, "{want} reads as itself");
        }
    }

    /// **The links settle through each other**, and several may lead to one
    /// place (`/bin` and `/sbin` on a merged Linux): the direction that has
    /// one answer. A cycle ends.
    #[test]
    fn root_links_settle_and_several_may_lead_to_one_place() {
        let links = settled(vec![
            ("a".into(), "/tmp/x".into()),
            ("tmp".into(), "/private/tmp".into()),
        ]);
        assert_eq!(normalize_among("/a/y", &links), "/private/tmp/x/y");
        let links: Vec<RootLink> = vec![
            ("bin".into(), "/usr/bin".into()),
            ("sbin".into(), "/usr/bin".into()),
        ];
        assert_eq!(normalize_among("/bin/ls", &links), "/usr/bin/ls");
        assert_eq!(normalize_among("/sbin/ls", &links), "/usr/bin/ls");
        let looped = settled(vec![("a".into(), "/b".into()), ("b".into(), "/a".into())]);
        assert_eq!(looped.len(), 2);
    }

    /// **macOS folds the case of a name at the root**, so a link's other
    /// casing opens the same place (`/ETC/hosts`): read as a name of its
    /// own it was under no rule.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_root_link_folds_case_on_macos() {
        let links: Vec<RootLink> = vec![("etc".into(), "/private/etc".into())];
        assert_eq!(normalize_among("/ETC/hosts", &links), "/private/etc/hosts");
    }

    /// **A regex is shown each way to name the path**: it is text, which the
    /// one spelling cannot reach.
    #[test]
    fn a_path_has_a_spelling_through_each_of_its_links() {
        for (name, to) in root_links() {
            let full = format!("{to}/x");
            assert!(
                link_spellings(&full).any(|s| s == format!("/{name}/x")),
                "{full}"
            );
            let beside = format!("{to}x/y");
            assert!(link_spellings(&beside).all(|s| s != format!("/{name}x/y")));
            let re = crate::pattern::PathRe::parse(&format!("re:^/{name}/x$"))
                .unwrap()
                .unwrap();
            assert!(re.matches(&format!("/{name}/x")), "{name}");
            assert!(re.matches(&full), "{full}");
        }
    }

    /// **A `cd` run some other way still moves the line**: `builtin cd` is a
    /// `cd`, and `eval`/`source` may run one (review 20, Reviewer E2 — a deny
    /// on `/etc/*` was lost to `builtin cd /etc && rm passwd`).
    #[test]
    fn a_cd_run_another_way_still_moves_the_line() {
        let p = line("builtin cd /etc && rm passwd");
        assert_eq!(
            p.stages.last().unwrap().cwd,
            Cwd::Resolved(normalize_path("/etc"))
        );
        for text in [
            "eval cd /etc && rm passwd",
            "source x && rm passwd",
            ". x && rm passwd",
        ] {
            let p = line(text);
            assert_eq!(p.stages.last().unwrap().cwd, Cwd::Unknown, "{text}");
        }
    }

    /// **Each word knows where it is written**, so an act can mark the
    /// words it stands on: past a peel (the command's words are the last of
    /// those written), and inside a substitution (shifted into the outer
    /// line, as the stage's own span is).
    #[test]
    fn a_word_keeps_where_it_is_written() {
        let text = "X=1 timeout 5 rm -f a; echo $(ls -a b) > c";
        let p = line(text);
        let at = |s: &Stage, i: usize| &text[s.word_spans[s.written_index(i)].clone()];
        let rm = &p.stages[0];
        assert_eq!(rm.command.words, ["rm", "-f", "a"]);
        assert_eq!(
            (0..3).map(|i| at(rm, i)).collect::<Vec<_>>(),
            ["rm", "-f", "a"]
        );
        let ls = p
            .stages
            .iter()
            .find(|s| s.command.program == "ls")
            .expect("the substitution's stage");
        assert_eq!(at(ls, 1), "-a");
        assert_eq!(at(ls, 2), "b");
        let echo = p
            .stages
            .iter()
            .find(|s| s.command.program == "echo")
            .expect("echo");
        assert_eq!(&text[echo.redirects[0].span.clone()], "> c");
    }

    #[test]
    fn redirection_is_an_effect_and_keeps_its_target() {
        let p = line("echo hi > out.txt");
        assert_eq!(p.stages.len(), 1);
        assert_eq!(
            p.stages[0].redirects,
            vec![Redirect {
                kind: RedirectKind::Write,
                target: "out.txt".into(),
                span: 8..17,
            }]
        );
        // `echo` alone is read-only; the redirect makes the stage mutating.
        assert_eq!(p.stages[0].command.effect, Effect::ReadOnly);
        assert_eq!(p.stages[0].effect(), Effect::Mutating);
        assert_eq!(line("echo hi >> log").stages[0].effect(), Effect::Mutating);
    }

    #[test]
    fn pipes_are_stages_too() {
        let p = line("cat f | wc -l");
        assert_eq!(p.stages.len(), 2);
        assert_eq!(p.stages[1].connector, Some(Operator::Pipe));
        assert_eq!(p.stages[1].command.program(), "wc");
    }

    #[test]
    fn a_line_is_only_as_safe_as_its_worst_stage() {
        // ls is ReadOnly, but the find -delete makes the line destructive.
        assert_eq!(line("ls && find . -delete").effect(), Effect::Destructive);
        // An unmodeled program dominates: we can't prove the line safe.
        assert_eq!(line("ls && frobnicate").effect(), Effect::Unknown);
    }

    #[test]
    fn malformed_and_unmodelable_lines_fail_closed() {
        assert!(parse_line("ls &&").is_err(), "trailing connector");
        assert!(parse_line("| wc").is_err(), "leading pipe");
        assert!(parse_line("echo >").is_err(), "redirect without target");
        assert!(parse_line("echo $(id").is_err(), "unbalanced substitution");
        assert!(parse_line("(cd /)").is_err(), "subshell");
        assert!(parse_line("").is_err(), "empty");
    }

    #[test]
    fn substitutions_are_decomposed_and_gate_the_inner_command() {
        // The inner command becomes its own stage — a danger hidden in a
        // substitution is caught, not laundered past the gate.
        let p = parse_line("cat $(rm -rf /)").expect("decomposed");
        let progs: Vec<&str> = p.stages.iter().map(|s| s.command.program()).collect();
        assert_eq!(progs, vec!["cat", "rm"]);
        assert_eq!(p.effect(), Effect::Destructive, "the hidden rm is seen");

        // The user's process-substitution example: the loop body AND the `find`
        // feeding it are both visible.
        let p =
            parse_line(r#"while IFS= read -r F; do rm -rf "$F"; done < <(find . -name "*.pdf")"#)
                .expect("decomposed");
        let progs: Vec<&str> = p.stages.iter().map(|s| s.command.program()).collect();
        assert!(progs.contains(&"rm"), "the loop body is a stage");
        assert!(
            progs.contains(&"find"),
            "the process substitution is a stage"
        );

        // The outer command's spliced-in value is unknown.
        let p = parse_line("echo $(id)").expect("decomposed");
        assert!(p.stages[0].command.has_unresolved_operand());
    }

    /// The eager-optimizer bypass: rebind a name, then use it. The second stage
    /// must stop being "an `rm` we understand" — otherwise a rule about `rm`
    /// silently stops applying to the thing actually running.
    #[test]
    fn an_alias_voids_name_based_conclusions_for_later_stages() {
        let p = line("alias rm=echo && rm -rf /");
        assert_eq!(
            p.stages[0].bindings,
            Bindings::Trusted,
            "the alias itself is fine"
        );
        assert_eq!(p.stages[1].bindings, Bindings::Disturbed);
        // Despite *looking* like a destructive rm, we can't identify it at all.
        assert_eq!(p.stages[1].command.program(), "rm");
        assert_eq!(p.stages[1].effect(), Effect::Unknown);
        assert!(!p.bindings_trusted());
    }

    #[test]
    fn binding_disturbance_is_sticky_and_covers_the_family() {
        for line_src in [
            "alias x=y && ls",
            "unalias rm && ls",
            "eval something && ls",
            "source setup.sh && ls",
            ". setup.sh && ls",
            "hash -r && ls",
            "export PATH=/tmp:/usr/bin && ls",
        ] {
            let p = line(line_src);
            let last = p.stages.last().expect("stages");
            assert_eq!(
                last.bindings,
                Bindings::Disturbed,
                "{line_src} should disturb name resolution"
            );
            assert_eq!(last.effect(), Effect::Unknown, "for {line_src}");
        }
    }

    /// Deeply-nested substitution must REFUSE, not overflow the stack and abort
    /// the process — the gate failing must mean "refuse to analyze", never "crash
    /// before a verdict". (Reviewer B: `catch_unwind` can't catch a stack overflow.)
    #[test]
    fn deeply_nested_substitution_refuses_rather_than_crashing() {
        let bomb = format!("echo {}rm -rf /{}", "$(".repeat(500), ")".repeat(500));
        match parse_line(&bomb) {
            Err(Unanalyzable::ShellStructure { found }) => {
                assert!(found.contains("too deep"), "got {found}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        // A realistic shallow nesting still analyzes fine.
        assert!(parse_line("echo $(basename $(pwd))").is_ok());
    }

    /// A stray `done`/`fi` drives control depth negative; it must be refused, not
    /// cancelled against a later unclosed opener to a passing net-zero.
    #[test]
    fn a_stray_close_keyword_is_refused() {
        assert!(parse_line("done; for i in 1; do rm -rf /tmp/x").is_err());
        assert!(parse_line("fi; rm x").is_err());
        // The balanced forms still analyze.
        assert!(parse_line("for i in 1; do echo $i; done").is_ok());
        assert!(parse_line("if true; then echo hi; fi").is_ok());
    }

    /// A builtin that binds a shell variable from DATA (`read`/`getopts`/`let`)
    /// poisons name resolution when it targets a hazardous variable — `read PATH <
    /// evil; ls` makes `ls` resolve via attacker-controlled `PATH`. These are
    /// ReadOnly in effect, so the guard is on the BINDINGS axis; without it,
    /// modeling them read-only turns this RCE from ASK into a false ALLOW.
    #[test]
    fn a_var_binding_builtin_targeting_path_disturbs_later_stages() {
        for line_src in [
            "read PATH; ls",
            "read -r PATH; ls",
            "getopts o PATH; ls",
            "let PATH=5; ls",
            "read IFS; ls",
        ] {
            let p = line(line_src);
            let last = p.stages.last().expect("stages");
            assert_eq!(
                last.bindings,
                Bindings::Disturbed,
                "{line_src} should disturb name resolution"
            );
            assert_eq!(last.effect(), Effect::Unknown, "for {line_src}");
        }
    }

    /// The precision counterpart: reading into an ORDINARY variable is not a
    /// disturbance (over-firing here would ask on every `while read line` loop),
    /// and a hazardous name as a flag VALUE (`read -d PATH VAR`) is the delimiter,
    /// not the target — the modeled flags keep them apart.
    #[test]
    fn reading_an_ordinary_variable_does_not_disturb() {
        for line_src in ["read FOO; ls", "read -r line; ls", "read -d PATH VAR; ls"] {
            let p = line(line_src);
            let last = p.stages.last().expect("stages");
            assert_eq!(last.bindings, Bindings::Trusted, "{line_src}");
            assert_eq!(last.effect(), Effect::ReadOnly, "{line_src}");
        }
    }

    /// The var-writer disturbance keys on the HAZARD ORACLE, not a `"PATH"`
    /// substring: `export LD_PRELOAD=/e.so && ls` is a SILENT RCE — `ls` runs
    /// attacker code, yet `LD_PRELOAD` has no "PATH" in it. Every hazardous
    /// var-writer must disturb later resolution, across the whole family. (Reviewer
    /// E — same class as the `read PATH` fix, but this one reached a false ALLOW
    /// with no prompt at all.)
    #[test]
    fn hazardous_var_writers_disturb_whatever_the_variable_name() {
        for line_src in [
            "export LD_PRELOAD=/e.so && ls",
            "export IFS=x && ls",
            "declare LD_AUDIT=/e && ls",
            "typeset BASH_ENV=/e && ls",
            "printf -v PATH /tmp/evil && ls",
            "printf -v LD_PRELOAD x && ls",
            "mapfile PATH; ls",
            "readarray IFS; ls",
        ] {
            let p = line(line_src);
            let last = p.stages.last().expect("stages");
            assert_eq!(
                last.bindings,
                Bindings::Disturbed,
                "{line_src} must disturb name resolution"
            );
            assert_eq!(last.effect(), Effect::Unknown, "for {line_src}");
        }
    }

    /// Precision counterpart: `printf` WITHOUT `-v` writes no variable, so a
    /// literal `PATH` operand is data, not a target — no disturbance (a spurious
    /// ask here would train dismissal).
    #[test]
    fn printf_without_dash_v_does_not_disturb() {
        let p = line("printf '%s' PATH; ls");
        let last = p.stages.last().expect("stages");
        assert_eq!(last.bindings, Bindings::Trusted);
    }

    /// Over-triggering would be its own failure — noise causes habituation. An
    /// environment variable that isn't PATH leaves name resolution alone.
    #[test]
    fn an_unrelated_export_does_not_disturb_bindings() {
        let p = line("export RUST_LOG=debug && ls -l");
        let last = p.stages.last().expect("stages");
        assert_eq!(last.bindings, Bindings::Trusted);
        assert_eq!(last.effect(), Effect::ReadOnly);
    }

    /// A BARE hazardous assignment persists in the shell, so it poisons later
    /// stages — unlike the prefixed form, which is scoped to one command.
    #[test]
    fn a_bare_hazardous_assignment_disturbs_later_stages() {
        let p = line("PATH=/tmp && ls -l");
        assert_eq!(
            p.stages[0].command.program(),
            "",
            "a bare assignment runs nothing"
        );
        assert_eq!(p.stages[1].bindings, Bindings::Disturbed);
        assert_eq!(p.stages[1].effect(), Effect::Unknown);

        let q = line("LD_PRELOAD=/evil.so && ls -l");
        assert_eq!(q.stages[1].effect(), Effect::Unknown);
    }

    /// ...but an ordinary variable must not, or we would fire on the very common
    /// `FOO=1 cmd` shape and drown the user in noise.
    #[test]
    fn a_bare_harmless_assignment_does_not_disturb() {
        let p = line("RUST_LOG=debug && ls -l");
        assert_eq!(p.stages[1].bindings, Bindings::Trusted);
        assert_eq!(p.stages[1].effect(), Effect::ReadOnly);
    }

    /// The prefixed form is scoped to its own stage, so the *next* stage is fine.
    #[test]
    fn an_env_prefix_is_scoped_to_its_own_stage() {
        let p = line("LD_PRELOAD=/x ls && ls -l");
        assert_eq!(p.stages[0].effect(), Effect::Executor, "the prefixed stage");
        assert_eq!(
            p.stages[1].bindings,
            Bindings::Trusted,
            "but not the next one"
        );
        assert_eq!(p.stages[1].effect(), Effect::ReadOnly);
    }

    #[test]
    fn eval_and_source_are_executors_by_identity() {
        assert_eq!(line("eval whatever").stages[0].effect(), Effect::Executor);
        assert_eq!(line("source setup.sh").stages[0].effect(), Effect::Executor);
    }

    /// A newline separates commands. Treating it as whitespace merged two
    /// commands into one and hid the second inside the first's operands —
    /// `ls\nrm -rf /` classified as a read-only `ls`. Multi-line input is the
    /// normal shape of a script, so this must never regress.
    #[test]
    fn a_newline_separates_commands() {
        let p = line("ls\nrm -rf /");
        assert_eq!(
            p.stages.len(),
            2,
            "a newline is a separator, not whitespace"
        );
        assert_eq!(p.stages[1].command.program(), "rm");
        assert_eq!(p.effect(), Effect::Destructive);
    }

    #[test]
    fn blank_statements_are_tolerated_but_dangling_connectors_are_not() {
        assert_eq!(line("ls\n\nrm -rf /").stages.len(), 2, "repeated newlines");
        assert_eq!(
            line("ls;").stages.len(),
            1,
            "trailing semicolon is valid shell"
        );
        assert_eq!(line("\nls\n").stages.len(), 1, "leading/trailing newline");
        assert!(
            parse_line("ls &&").is_err(),
            "a dangling && is still malformed"
        );
    }

    /// A wrapper's arguments *are* another command: `sudo rm -rf /` is an `rm`.
    /// Without peeling, every wrapper is a one-token bypass of every name-based
    /// rule — the same defect as an env prefix hiding the program.
    #[test]
    fn wrappers_are_peeled_to_reach_the_real_command() {
        for (src, wrappers) in [
            ("sudo rm -rf /", vec!["sudo"]),
            ("nice rm -rf /", vec!["nice"]),
            ("sudo -u root rm -rf /", vec!["sudo"]),
            ("timeout 5 rm -rf /", vec!["timeout"]),
            ("nohup rm -rf /", vec!["nohup"]),
            ("env FOO=1 rm -rf /", vec!["env"]),
            ("sudo nice rm -rf /", vec!["sudo", "nice"]),
        ] {
            let cmd = &line(src).stages[0].command;
            assert_eq!(cmd.program(), "rm", "for {src}");
            assert_eq!(cmd.wrappers, wrappers, "for {src}");
            assert!(
                cmd.has_flag("recursive") && cmd.has_flag("force"),
                "for {src}"
            );
            assert_eq!(cmd.effect, Effect::Destructive, "for {src}");
        }
        // `env FOO=1 rm` still captures the assignment while peeling.
        let cmd = &line("env FOO=1 rm -rf /").stages[0].command;
        assert_eq!(cmd.env.get("FOO").map(String::as_str), Some("1"));
    }

    #[test]
    fn a_wrapper_with_no_inner_command_stays_the_command() {
        let cmd = &line("sudo").stages[0].command;
        assert_eq!(cmd.program(), "sudo");
        assert!(cmd.wrappers.is_empty());
        assert_eq!(
            cmd.effect,
            Effect::Executor,
            "a bare wrapper still runs things"
        );
    }

    /// We do not model control flow, so we refuse rather than analyze `for` as
    /// though it were a program — which would produce confident nonsense.
    #[test]
    fn control_flow_is_decomposed_into_the_commands_it_runs() {
        // We see THROUGH the loop to the command inside it. The scaffolding
        // executes nothing, so it produces no stage; the body does.
        let p = parse_line("for f in *; do rm $f; done").expect("decomposed");
        assert_eq!(p.stages.len(), 1, "only the body is a command");
        assert_eq!(p.stages[0].command.program(), "rm");
        assert_eq!(p.effect(), Effect::Destructive, "the rm, not the loop");

        // A `while` CONDITION is a command too — it must not be skipped.
        let p = parse_line("while rm -f lock; do ls; done").expect("decomposed");
        let programs: Vec<&str> = p.stages.iter().map(|s| s.command.program()).collect();
        assert_eq!(programs, vec!["rm", "ls"], "condition AND body analyzed");

        // Every branch of an `if` is analyzed — either may run.
        let p = parse_line("if test -f x; then rm x; else ls; fi").expect("decomposed");
        let programs: Vec<&str> = p.stages.iter().map(|s| s.command.program()).collect();
        assert_eq!(programs, vec!["test", "rm", "ls"]);
    }

    #[test]
    fn a_determinate_loop_is_read_only() {
        // The case that exposed the old over-refusal: nothing here is mysterious.
        let p = parse_line(r#"for i in {1..9}; do echo "$i"; done"#).expect("decomposed");
        assert_eq!(p.stages.len(), 1);
        assert_eq!(p.stages[0].command.program(), "echo");
        assert_eq!(p.effect(), Effect::ReadOnly);
    }

    #[test]
    fn cd_inside_control_flow_makes_the_directory_unknown() {
        // A loop body's `cd` compounds each iteration and a branch's `cd` may not
        // have happened at all, so later stages must not assume a location.
        let p = parse_line("for d in a b; do cd $d; done; rm -rf x").expect("decomposed");
        let last = p.stages.last().expect("a stage");
        assert_eq!(last.cwd, Cwd::Unknown);
        assert!(!p.cwd_fully_known());
    }

    #[test]
    fn unmodeled_constructs_are_still_refused_honestly() {
        // `select`'s positions (word list vs. commands) aren't enumerated yet,
        // so we refuse rather than guess — and say that's what we're doing.
        match parse_line("select x in a b; do ls; done") {
            Err(Unanalyzable::ControlFlow { keyword }) => assert_eq!(keyword, "select"),
            other => panic!("select should still be refused, got {other:?}"),
        }
        // `case` is refused even earlier — `lex` stops on the `)` — but the
        // point that matters is that it does not slip through.
        assert!(parse_line("case $x in a) rm y ;; esac").is_err());
        assert!(
            Unanalyzable::ControlFlow { keyword: "case" }
                .reason()
                .contains("don't decompose yet")
        );
    }

    #[test]
    fn unbalanced_control_flow_is_refused() {
        // If the scaffolding doesn't balance we misread the structure, and a
        // misread structure is not a safe thing to draw conclusions from.
        for src in ["for i in 1; do rm x", "done", "if true; then rm x"] {
            assert!(
                parse_line(src).is_err(),
                "{src} should be refused as unbalanced"
            );
        }
    }

    #[test]
    fn a_redirect_on_a_construct_is_not_silently_dropped() {
        // `done > out` writes; there is no single stage to attach it to, so we
        // refuse rather than lose the write (the only unsafe failure direction).
        assert!(parse_line("for i in 1 2; do echo $i; done > out.log").is_err());
    }

    /// BashPitfalls #48: with `CDPATH` set, `cd foo` can land somewhere the path
    /// alone does not predict, so our directory arithmetic stops being sound.
    #[test]
    fn cdpath_makes_later_cds_unknowable() {
        let p = line("export CDPATH=/elsewhere && cd foo && rm -rf x");
        assert_eq!(p.stages[2].cwd, Cwd::Unknown, "cd is no longer predictable");
        assert!(!p.cwd_fully_known());

        // The bare-assignment form too.
        let q = line("CDPATH=/elsewhere && cd foo && ls");
        assert_eq!(q.stages[2].cwd, Cwd::Unknown);

        // ...but an ordinary variable leaves `cd` alone.
        let r = line("export RUST_LOG=debug && cd foo && ls");
        assert_eq!(r.stages[2].cwd, Cwd::Resolved("foo".into()));
    }

    /// BashPitfalls #56: `xargs` builds its argument list from stdin, so the
    /// operands we can see are not the ones that get acted on. A path-scoped
    /// rule must not treat the visible list as complete.
    #[test]
    fn xargs_operands_come_from_stdin() {
        let cmd = &line("xargs rm -rf").stages[0].command;
        assert_eq!(cmd.program(), "rm", "still peeled to the real command");
        assert_eq!(cmd.effect, Effect::Destructive);
        assert!(
            cmd.has_unresolved_operand(),
            "the real targets arrive on stdin, so paths are not knowable"
        );
    }

    /// Every command in a pipeline runs in its own SUBSHELL, so a `cd` there
    /// never reaches the parent. Propagating it would place later stages in a
    /// directory they are not in — unsound in the dangerous direction, because
    /// a rule scoped to that directory would wrongly permit them.
    #[test]
    fn a_cd_inside_a_pipeline_does_not_propagate() {
        let p = line("cd /tmp | cat && rm -rf x");
        assert_eq!(p.stages[2].command.program(), "rm");
        assert_eq!(
            p.stages[2].cwd,
            Cwd::Unknown,
            "the cd ran in a subshell; we must not claim rm runs in /tmp"
        );
        assert!(!p.cwd_fully_known());

        // Outside a pipeline it still propagates normally.
        let q = line("cd /tmp && rm -rf x");
        assert_eq!(q.stages[1].cwd, Cwd::Resolved(normalize_path("/tmp")));
    }

    /// ANSI-C quoting decodes escapes, so `$'\x2f'` IS `/`. Showing the literal
    /// text while the shell runs something else is precisely the
    /// text-is-not-behavior failure this module exists to prevent.
    #[test]
    fn ansi_c_quoting_is_refused_rather_than_misread() {
        assert!(parse_line("rm -rf $'\\x2f'").is_err());
        assert!(parse_line("echo $\"hello\"").is_err());
        // Ordinary quoting is unaffected.
        assert!(parse_line("echo 'plain'").is_ok());
        assert!(parse_line("echo \"plain\"").is_ok());
    }

    /// `pushd`/`popd` change the cwd like `cd`; we don't model the stack, so
    /// they make the location Unknown (fail closed) rather than passing it
    /// through — else `pushd / && rm -rf *` looks like it runs where the line
    /// started. (Reviewer A.)
    #[test]
    fn pushd_popd_are_tracked_as_unknown_cwd() {
        assert_eq!(line("pushd / && rm -rf x").stages[1].cwd, Cwd::Unknown);
        assert_eq!(line("popd && rm -rf x").stages[1].cwd, Cwd::Unknown);
        assert!(!line("pushd / && rm -rf x").cwd_fully_known());
    }

    /// A `cd` argument holding a glob or brace is not a resolved directory. The
    /// predicate had been missing `[` and `{`. (Reviewers D, E.)
    #[test]
    fn a_cd_with_a_glob_or_brace_is_unknown() {
        assert_eq!(line("cd build[0] && rm -rf x").stages[1].cwd, Cwd::Unknown);
        assert_eq!(line("cd a{b,c} && rm -rf x").stages[1].cwd, Cwd::Unknown);
    }

    /// `CDPATH=/x cd foo` scopes CDPATH to that very cd, so it is the cd that
    /// becomes unpredictable — and it was the one case still resolved.
    /// (Reviewer E.)
    #[test]
    fn a_prefixed_cdpath_makes_its_own_cd_unknown() {
        assert_eq!(
            line("CDPATH=/elsewhere cd foo && rm -rf x").stages[1].cwd,
            Cwd::Unknown
        );
    }

    #[test]
    fn path_normalization_is_lexical() {
        assert_eq!(normalize_path("/a/./b/../c"), "/a/c");
        assert_eq!(normalize_path("a/b/../.."), "");
        assert_eq!(normalize_path("../x"), "../x");
        assert_eq!(normalize_path("/.."), "/");
    }
}

#[cfg(test)]
mod hazard_flag_tests {
    use super::*;

    fn trusted(line: &str) -> bool {
        parse_line(line).expect("parses").bindings_trusted()
    }

    /// **A variable-writing program can name its target in a flag's VALUE.**
    ///
    /// `writes_hazard_var` scanned operands and `unknown` only, on the stated
    /// grounds that "a flag's VALUE never reaches here: modeled flags consume
    /// it". That is true of `read -d PATH VAR`, where `PATH` is the delimiter —
    /// and false of `read -a PATH`, where the array NAME is exactly the
    /// variable being written. `-a` is modelled `takes_value: true`, so the
    /// name was consumed as the flag's value and never scanned, and bindings
    /// stayed Trusted while `PATH` was redirected (Reviewer A, HIGH).
    ///
    /// The premise rows matter: without them this test could pass because
    /// everything is Disturbed.
    #[test]
    fn a_hazard_var_named_in_a_flag_value_still_disturbs_bindings() {
        assert!(!trusted("read PATH; ls"), "premise: the documented shape");
        assert!(
            trusted("read -a FOO; ls"),
            "premise: a harmless name is fine"
        );
        assert!(trusted("ls; ls"), "premise: no writer at all");

        assert!(
            !trusted("read -a PATH; ls"),
            "the array NAME is the variable being written"
        );
        assert!(
            !trusted("read -a LD_PRELOAD; ls"),
            "and the same for any other resolution hazard"
        );
    }
}
