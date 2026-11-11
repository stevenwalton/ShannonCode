//! **The decision over acts** (design §8.1): a stage is read as acts on
//! targets ([`acts`](super::acts)), each act is decided by the rules that
//! apply to it, and the stage is as strict as its strictest act.
//!
//! Per act: the rules that apply are gathered — a program rule by its form
//! (the chain a prefix of the act's, every flag it names present, its words
//! found), a path rule by its operation and pattern, a host rule by its
//! direction and host — and resolved by TRUST then SPECIFICITY. Trust: the
//! user's own tiers ([`Tier::User`], [`Tier::Session`]) are one group, the
//! project's below it, the shipped rules below both; the highest group with
//! a rule decides, and a lower group only NARROWS it — its asks and denies
//! stay (a GUARD: the credential paths under `allow :read ~`) unless a
//! higher rule is at least as specific (`allow :read ~/.ssh`), and its
//! allows count only where no higher group speaks. Specificity, within a
//! group: a rule strictly within another (its chain longer, its flags
//! more, its path below) displaces it, so `allow /mnt/foo` is the
//! exception under `deny /mnt` and `deny find -delete` holds under `allow
//! find *`; incomparable rules, and equal ones, fall to the strictest — as
//! do the rules that stand across groups. A path rule met only softly (from
//! above, or by a glob's contact) never displaces one naming the act, and
//! `DIR/**` is within `DIR` only for a delete.
//!
//! An allow on a program is read with CONTAINMENT: it covers the act only
//! if every node the act touched is covered — by the rule's own form,
//! another allow, a trailing `*`, or the model: a benign flag rides, but
//! only on a call that only reads (`allow find -name` covers `find . -name
//! x -L`; `allow rm` does not cover `rm -rf`, which `allow rm *` or `allow
//! rm -r -f` does — Steven, 10-10). A flag that raises the program past a
//! read is never benign, so the shipped carvings of the old engine (`find
//! -exec` asking beneath `allow find src`) are a property of the model
//! here, not rules: a blanket lifts them, a form that names the flag lifts
//! it, nothing else does.
//!
//! With no rule, the defaults (Q7): a path act inside where Shannon
//! started allows at read and write and asks at delete and exec; outside
//! it asks; a modelled program at a read node allows, at any other node
//! asks until a rule — "a writer with no rule" — unless every write it
//! names is granted by a path rule and it carries no flag a program rule
//! would have to approve (`rm build` under `allow :delete build` runs, `rm
//! -rf build` asks); an unmodelled program asks; a host asks;
//! a wrapper allows (what it runs is judged on its own). HARD, lifted by
//! nothing: a secret handle in the line, a privilege wrapper (a user deny
//! makes it a deny), a stage the gate cannot read soundly, a word it cannot
//! place, code it does not read.

use std::collections::{BTreeMap, HashMap};

use super::acts::{Act, Context, Dir, PathAct, ProgramAct, acts_of};
pub use super::acts::{Call, Written};
use super::line::{Body, HostPat, Parsed, PathPat, Resolved, Tier, glob_match, in_tree};
pub use super::model::ModelRef;
use super::model::{Effect, Trust};
use super::{Cited, Decision};
use crate::shell::{Stage, Unanalyzable, parse_line};

/// One body of one rule line, in force.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub action: Decision,
    pub tier: Tier,
    pub body: Body,
    /// The rule as a decision cites it.
    pub cited: Cited,
}

/// The rules in force: every body of every line, with its tier. Order is
/// irrelevant to the outcome.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    pub lines: Vec<Line>,
}

impl Policy {
    /// Every body of a parsed line, at a tier.
    pub fn push(&mut self, parsed: Parsed, tier: Tier, cited: Cited) {
        for body in parsed.bodies {
            self.lines.push(Line {
                action: parsed.action,
                tier,
                body,
                cited: cited.clone(),
            });
        }
    }
}

/// What a decision needs besides the policy: the reader's context, and
/// whether a secret handle is one Shannon minted.
pub struct Gate<'a> {
    pub policy: &'a Policy,
    pub cx: &'a Context<'a>,
    pub knows: &'a dyn Fn(&str) -> bool,
}

/// The act an [`ActVerdict`] is about, for the pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActKind {
    Program {
        name: String,
        chain: Vec<String>,
        effect: Effect,
        /// The API call it makes, when its model knows one (`gh api user`).
        call: Option<Call>,
        /// A wrapper the normalizer peeled (`timeout 5 CMD`): what it runs
        /// is judged, and shown, as its own act.
        wrapper: bool,
        /// The words no model knows — an unknown flag, an operand at a
        /// position the model says nothing about — whatever a rule made of
        /// them: what they do is not known, so the pane cannot call the
        /// call safe.
        unknown: Vec<String>,
    },
    Path {
        op: Option<Effect>,
        paths: Vec<String>,
        word: String,
        /// A redirection's target or a wrapper's argument — the shell's or
        /// the wrapper's, not a file the program names: a proposal writes
        /// it as a path rule, as it does every place.
        indirect: bool,
    },
    Host {
        dir: Dir,
        host: String,
    },
    Code {
        program: String,
        what: String,
    },
    Privilege(String),
    Unsound(String),
    Unplaced(String),
    Secret {
        handle: String,
        known: bool,
    },
    /// **The line names a variable a secret was lent from** (`[secrets]
    /// env`) — `$GITHUB_TOKEN`, `printenv GITHUB_TOKEN`, `ENVIRON["…"]`:
    /// the value is spent through `handle`, and a line that reaches for the
    /// variable faces a human (Steven, 10-31, S18-Q1). No command is handed
    /// it (`Aliases::lent_vars`), but a shell that sets it again — Claude
    /// Code's Bash replays your shell's setup — would print it.
    LentVar {
        name: String,
        handle: String,
    },
}

/// Why an act with no rule got its decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoRule {
    /// A modelled program at a read node.
    ModelledRead,
    /// A wrapper: what it runs is judged on its own.
    Wrapper,
    /// A path inside where Shannon started, at this operation.
    Inside(Option<Effect>),
    /// A path outside.
    Outside,
    /// Shannon does not model the program.
    Unmodelled,
    /// A modelled program past a read ("a writer with no rule").
    Effect(Effect),
    /// Words the model does not know.
    Unknown(Vec<String>),
    /// Flags the model guards on purpose (`grep -r`, `rg --hidden`), each
    /// with its reason: a rule naming the flag, or a blanket, runs them.
    Guarded(Vec<String>),
    /// What only a rule naming this exact chain lifts.
    NeedsNamed(Vec<String>),
    /// A host no rule names.
    Host,
    /// The program's model is a completion import (`trust =
    /// "completion"`): it says what the words are, never that a call is
    /// safe, so with no rule it asks.
    Completion,
}

/// What no rule lifts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hard {
    /// A secret handle in the line: the executor expands it, so the line
    /// carries the secret wherever it goes. Or the variable one was lent
    /// from, named ([`ActKind::LentVar`]).
    Secret,
    Privilege,
    /// The stage cannot be read soundly.
    Unsound,
    /// A word the gate could not place: it never guesses.
    Unplaced,
    /// Code the model does not read, or a program named by configuration.
    Code,
}

/// What decided an act.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum By {
    Rules(Vec<Cited>),
    Default(NoRule),
    Hard(Hard),
}

/// The decision for one act, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActVerdict {
    pub act: ActKind,
    pub decision: Decision,
    pub by: By,
    /// What an allow did not cover, when that is why it asks.
    pub uncovered: Vec<String>,
    pub model: Option<ModelRef>,
    /// Where the act is written in the line ([`Written`]), set by
    /// [`decide_stage`] from the act it judged: the pane marks these words,
    /// not the whole command.
    pub at: Written,
    /// The rules a path act meets softly — from above, or by a glob's
    /// contact — that ask as strictly as `by` does, when `by` is the place's
    /// own account (`rm -r /`: outside where Shannon started, and holding
    /// `~/.ssh`). Said after it: no grant of the place lifts them. Empty
    /// where what it meets is `by` itself — the place would be allowed
    /// without it (`rg foo .` above Shannon's state) — and under a deny.
    pub reaches: Vec<Cited>,
}

/// The decision for one stage: its strictest act.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageVerdict {
    pub decision: Decision,
    pub acts: Vec<ActVerdict>,
}

// ---- resolution --------------------------------------------------------------------

/// The rules among `xs` that no other is strictly within.
fn minimal<T: Clone>(xs: &[T], within: &dyn Fn(&T, &T) -> bool) -> Vec<T> {
    xs.iter()
        .filter(|a| !xs.iter().any(|b| within(b, a) && !within(a, b)))
        .cloned()
        .collect()
}

/// **Trust, then specificity: what stands.** Each group's most specific
/// rules (a narrower rule takes a broader one's place within a group);
/// then the highest group with a rule, beside the asks and denies of lower
/// groups no higher rule is at least as specific as — their allows count
/// only where no higher group spoke.
///
/// The decision is the strictest rule that stands, and its reason the
/// rules that stand at that strictness ([`at`]). A lower group only
/// narrows: taking the standing rules' most specific across groups as well
/// let the shipped `~/.ssh` guard, an ask, stand in place of a user's
/// `deny :* ~`, so `cat ~/.ssh/config` asked (10-13). And a step that took
/// a broader rule out beside a narrower, stricter one decided nothing the
/// strictest did not, and was held by no test (review of c9c40fe, A and C).
fn resolve<T: Clone>(
    cands: &[T],
    action: &dyn Fn(&T) -> Decision,
    tier: &dyn Fn(&T) -> Tier,
    within: &dyn Fn(&T, &T) -> bool,
) -> Vec<T> {
    let mut pool: Vec<T> = Vec::new();
    for group in (0..=2).rev() {
        let own: Vec<T> = cands
            .iter()
            .filter(|c| tier(c).group() == group)
            .cloned()
            .collect();
        // A group decides as a policy of its own first, so an exception it
        // carries (the `paths_except` grant under the `~/.claude` guard)
        // narrows what it passes down.
        let own = minimal(&own, within);
        if pool.is_empty() {
            pool = own;
            continue;
        }
        for c in own {
            if action(&c) != Decision::Allow && !pool.iter().any(|p| within(p, &c)) {
                pool.push(c);
            }
        }
    }
    pool
}

/// The rules among `standing` that decide: those at its strictest (the
/// decision), each a reason for it; a weaker one beside them is none — a
/// user's `allow :read ~` was cited as a place a guard keeps, and a
/// contact's ask as the way out of a deny (review of c9c40fe, A and C).
fn at<'s, T>(standing: &'s [T], action: &dyn Fn(&T) -> Decision) -> Option<(Decision, Vec<&'s T>)> {
    let decision = standing.iter().map(action).max()?;
    let deciding = standing.iter().filter(|c| action(c) == decision).collect();
    Some((decision, deciding))
}

// ---- program acts ------------------------------------------------------------------

/// A program rule as it applies to one act: direct, or derived for an inner
/// argv when the rule's chain runs past the outer act's chain into it.
#[derive(Clone)]
struct Applied<'a> {
    line: &'a Line,
    chain: Vec<String>,
    flags: Vec<(String, Option<String>)>,
    /// Chain words and flags as written, in order: an unmodelled program's
    /// prefix.
    words: Vec<String>,
    /// Words (not flags) the model does not know, matched as written.
    literal_words: Vec<String>,
    blanket: bool,
    /// A whole-line `re:` — matched against the stage's text, covering the
    /// program act's every node and none of its paths.
    regex: bool,
}

fn applied<'a>(
    line: &'a Line,
    act: &ProgramAct,
    texts: &[String],
    main: bool,
) -> Option<Applied<'a>> {
    match &line.body {
        Body::Regex(re) => {
            // The advanced tier reads text: the stage's own command (the one
            // its wrappers run) as the line spells it, and an inner argv as
            // its own program and words — so `deny re:^rm` meets `find -exec
            // rm` as `deny rm` does (review 2026-10-10, E1).
            let own = format!("{} {}", act.name, act.prefix_words.join(" "));
            let hit = if main {
                texts.iter().any(|t| re.find(t).is_some())
            } else {
                re.find(own.trim_end()).is_some()
            };
            hit.then(|| Applied {
                line,
                chain: Vec::new(),
                flags: Vec::new(),
                words: Vec::new(),
                literal_words: Vec::new(),
                blanket: line.action == Decision::Allow,
                regex: true,
            })
        }
        Body::Program(form) => {
            // Words the command must carry as written: the ones the model
            // does not know, and the operands it reads as data.
            let literal_words: Vec<String> = form
                .literal
                .iter()
                .filter(|w| !w.starts_with('-'))
                .chain(form.positionals.iter())
                .cloned()
                .collect();
            if form.program == act.name || form.program == "*" {
                return Some(Applied {
                    line,
                    chain: form.chain.clone(),
                    flags: form.flags.clone(),
                    words: form.words.clone(),
                    literal_words,
                    blanket: form.blanket,
                    regex: false,
                });
            }
            derived(line, act)
        }
        _ => None,
    }
}

/// A program rule **derived** for an inner argv: the act is the inner argv
/// of (outer, outer_chain), and the rule is on the outer program with a
/// chain running past it into this program (`allow uv run pytest *` names
/// pytest).
fn derived<'a>(line: &'a Line, act: &ProgramAct) -> Option<Applied<'a>> {
    let Body::Program(form) = &line.body else {
        return None;
    };
    let (outer, outer_chain) = act.via.as_ref()?;
    let names_it = form.program == *outer
        && form.chain.len() > outer_chain.len()
        && form.chain[..outer_chain.len()] == outer_chain[..]
        && form.chain[outer_chain.len()] == act.name;
    if !names_it {
        return None;
    }
    let literal_words: Vec<String> = form
        .literal
        .iter()
        .filter(|w| !w.starts_with('-'))
        .chain(form.positionals.iter())
        .cloned()
        .collect();
    let chain = form.chain[outer_chain.len() + 1..].to_vec();
    // Only the flags written after the inner program are its: those before
    // are the outer program's (`allow sudo -u=pg psql *` gives psql no `-u`).
    let outer = form.flags_before(outer_chain.len());
    let mut words = chain.clone();
    words.extend(
        form.words
            .iter()
            .filter(|w| w.starts_with('-'))
            .skip(outer)
            .cloned(),
    );
    Some(Applied {
        line,
        chain,
        flags: form.flags[outer.min(form.flags.len())..].to_vec(),
        words,
        literal_words,
        blanket: form.blanket,
        regex: false,
    })
}

/// The canonical name a rule's spelling names on this act: the touch that
/// lists the spelling, else the spelling itself (already canonical, or an
/// unmodelled program's).
fn canon_of(act: &ProgramAct, spelling: &str) -> String {
    act.touches
        .iter()
        .find(|t| t.names.iter().any(|s| s == spelling))
        .map(|t| t.name.clone())
        .unwrap_or_else(|| spelling.to_string())
}

/// A rule's flag value against the act's: none written matches any; written,
/// a glob over the value (`--file=/etc/*`).
fn value_matches(rule: &Option<String>, fact: &Option<String>) -> bool {
    match (rule, fact) {
        (None, _) => true,
        (Some(g), Some(v)) => glob_match(g, v, false),
        (Some(_), None) => false,
    }
}

/// Which words of an unmodelled argv are a flag's value: the word after a
/// flag that some rule on the program declares valued (`allow bat -l=`),
/// when the flag carries none attached and the word is no flag itself. Per
/// word, the flag it belongs to. A rule's own words are read the same way,
/// so `allow make -C sub build` and `make -C x build` agree on what the
/// arguments are.
fn values_of<'w>(words: &'w [String], valued: &[String]) -> Vec<Option<&'w str>> {
    let mut out = vec![None; words.len()];
    for k in 1..words.len() {
        let flag = &words[k - 1];
        if is_flag_word(flag) && valued.contains(flag) && !is_flag_word(&words[k]) {
            out[k] = Some(flag.as_str());
        }
    }
    out
}

/// The arguments of an unmodelled argv, or of a form read the same way:
/// its words that are neither a flag nor a declared flag's value.
fn arguments<'w>(words: &'w [String], valued: &[String]) -> Vec<&'w String> {
    words
        .iter()
        .zip(values_of(words, valued))
        .filter(|(w, of)| !is_flag_word(w) && of.is_none())
        .map(|(w, _)| w)
        .collect()
}

/// **An unmodelled argv's words as an allow must name them** — each read as
/// the decision reads it beside `policy`'s declarations: a declared flag
/// with its value (`-l=rust`), an argument as written; and whether any other
/// flag is there (for a `*`). `A` proposed `allow mytool rust build *` for
/// `mytool -l rust build` under `ask mytool -l=go`, a rule the gate read as
/// arguments `build` and never met (review 2026-10-11, A).
pub(super) fn unmodelled_words(
    policy: &Policy,
    program: &str,
    argv: &[String],
) -> (Vec<String>, bool) {
    let valued: Vec<String> = policy
        .lines
        .iter()
        .filter_map(|l| match &l.body {
            Body::Program(f) if f.program == program || f.program == "*" => Some(f),
            _ => None,
        })
        .flat_map(|f| f.flags.iter().filter(|(_, v)| v.is_some()))
        .map(|(n, _)| n.clone())
        .collect();
    let values = values_of(argv, &valued);
    let (mut words, mut flags) = (Vec::new(), false);
    for (k, w) in argv.iter().enumerate() {
        if values[k].is_some() {
            continue;
        }
        if !is_flag_word(w) {
            words.push(w.clone());
        } else if let Some(Some(_)) = values.get(k + 1) {
            words.push(format!("{w}={}", argv[k + 1]));
        } else {
            flags = true;
        }
    }
    (words, flags)
}

/// Does the applied rule match the act? May truncate a rule whose chain
/// runs into an inner argv to the part it shares with this act — when the
/// rule names the inner argv this act runs (`names_inner`,
/// [`names_its_inner`]). `valued`: the flags a rule on the program declares
/// to take a value.
fn matches_program(
    a: &mut Applied,
    act: &ProgramAct,
    valued: &[String],
    names_inner: &dyn Fn(&Line) -> bool,
) -> bool {
    if a.regex {
        return true;
    }
    if act.modelled {
        if a.chain.len() > act.chain.len() {
            // Only an ALLOW is cut down to the shared chain: `deny uv run
            // rm` says nothing about `uv run pytest` — it meets the inner
            // `rm` act through the derived rule (review 2026-10-10, A). And
            // only for the inner argv it names: `allow uv run pytest *` is
            // not `uv run ls pytest`.
            if !act.takes_argv
                || a.line.action != Decision::Allow
                || a.chain[..act.chain.len()] != act.chain[..]
                || !names_inner(a.line)
            {
                return false;
            }
            // The rest names the inner program: on this act only the shared
            // chain applies, with the flags written before it (named, so
            // present, as any allow's) and not the blanket.
            let outer = match &a.line.body {
                Body::Program(f) => f.flags_before(act.chain.len()),
                _ => 0,
            };
            a.chain.truncate(act.chain.len());
            a.flags.truncate(outer);
            a.blanket = false;
            a.literal_words.clear();
        } else if act.chain[..a.chain.len()] != a.chain[..] {
            return false;
        }
        let flags_hold = a.flags.iter().all(|(name, value)| {
            let canon = canon_of(act, name);
            // A flag the model does not know has no arity: its value is
            // read as an unmodelled program's would be. (A fact the model
            // implied — `gh api -f` ⇒ `method=POST` — has no touch, and is
            // read as the model's.)
            let unknown = act
                .touches
                .iter()
                .any(|t| t.unknown && t.names.iter().any(|s| s == name));
            if unknown {
                unmodelled_flag_holds(act, name, value, a.line.action != Decision::Allow)
            } else {
                // A deny or an ask meets any occurrence; an allow holds only
                // where EVERY one carries a value it names, as an unmodelled
                // flag's grant does — `allow gh api -X=GET` covered `gh api
                // -X GET -X DELETE` (review 2026-10-11, E1).
                let mut given = act
                    .facts
                    .iter()
                    .filter(|(n, _)| n == name || *n == canon)
                    .peekable();
                given.peek().is_some()
                    && if a.line.action == Decision::Allow {
                        given.all(|(_, v)| value_matches(value, v))
                    } else {
                        given.any(|(_, v)| value_matches(value, v))
                    }
            }
        });
        let words_hold = a
            .literal_words
            .iter()
            .all(|w| act.prefix_words.iter().any(|p| p == w));
        flags_hold && words_hold
    } else {
        // Unmodelled: an allow's bare words are a prefix of the argv's bare
        // words, in order (`allow uv run pytest *` is not `uv run --with
        // requests pytest`), and its flags are present anywhere — as a
        // person reads the rule, since without a model a flag's position
        // says nothing the gate can use (Steven, 10-10). A deny or an ask
        // is met by its words anywhere (contact). A flag written with a
        // value holds only for that value (`--out=a` is not `--out=b`). A
        // declared flag's value is the flag's, wherever the flag is, and no
        // part of the prefix; contact still meets it as a word.
        let contact = a.line.action != Decision::Allow;
        let flags_hold = a
            .flags
            .iter()
            .all(|(name, value)| unmodelled_flag_holds(act, name, value, contact));
        let words_hold = if contact {
            // Met anywhere: as a word, or as a value a dashed word may carry
            // (`deny mytool --lang rust` meets `--lang=rust` and `-lrust`;
            // review 2026-10-11, E1) — the reading that places them.
            let bare: Vec<String> = act
                .prefix_words
                .iter()
                .flat_map(|w| {
                    if is_flag_word(w) {
                        super::acts::cluster_values(w).unwrap_or_default()
                    } else {
                        vec![w.clone()]
                    }
                })
                .collect();
            a.words
                .iter()
                .filter(|w| !is_flag_word(w))
                .all(|w| bare.contains(w))
        } else {
            let args = arguments(&act.prefix_words, valued);
            let rule_args = arguments(&a.words, valued);
            // A word the rule wrote after a declared flag is the value it
            // names: `allow mytool -C sub build` is `-C=sub`, not any `-C`.
            let values_hold = a
                .words
                .iter()
                .zip(values_of(&a.words, valued))
                .all(|(w, of)| {
                    of.is_none_or(|flag| unmodelled_flag_holds(act, flag, &Some(w.clone()), false))
                });
            values_hold && args.len() >= rule_args.len() && args[..rule_args.len()] == rule_args[..]
        };
        flags_hold && words_hold
    }
}

/// Does the rule's flag `name` (with `value`, when it names one) hold on an
/// argv read without a model of the flag? Each occurrence is taken with its
/// value: attached, or — when the rule names one — the next word
/// (`--context prod`), the shape a person means by `--context=prod*`, as
/// no model says which. For contact (deny, ask) one occurrence suffices,
/// and a short cluster holding the letter is one; a grant holds only when
/// EVERY occurrence carries a value it names, spelled as written.
fn unmodelled_flag_holds(
    act: &ProgramAct,
    name: &str,
    value: &Option<String>,
    contact: bool,
) -> bool {
    let mut occurrences: Vec<Option<String>> = Vec::new();
    for (k, p) in act.prefix_words.iter().enumerate() {
        let (spelling, attached) = split_value(p);
        let next = || {
            value
                .is_some()
                .then(|| act.prefix_words.get(k + 1))
                .flatten()
                .filter(|n| !is_flag_word(n))
                .cloned()
        };
        if spelling == name {
            occurrences.push(attached.or_else(next));
        } else if contact {
            // In a cluster the letter takes what follows it, if anything
            // does (`-lrust` is `-l=rust`), else the attached or next value
            // (review 2026-10-11, E1: `deny -l=rust` missed `-lrust`).
            for rest in cluster_rests(spelling, name) {
                occurrences.push(if rest.is_empty() {
                    attached.clone().or_else(next)
                } else {
                    Some(rest.to_string())
                });
            }
        }
    }
    // For contact a rule on every value (`deny bat -l=*`) meets the flag
    // with none read too (`bat -l` last, or before another flag): without a
    // model, a missing value is the reader's guess, not the command's.
    let any_value = value.as_deref() == Some("*");
    !occurrences.is_empty()
        && if contact {
            occurrences
                .iter()
                .any(|v| value_matches(value, v) || (any_value && v.is_none()))
        } else {
            occurrences.iter().all(|v| value_matches(value, v))
        }
}

/// An argv word of an unmodelled program as a flag and its attached value
/// (`--out=a` is `--out` with `a`; `-x` is `-x` with none).
fn split_value(word: &str) -> (&str, Option<String>) {
    match word.split_once('=') {
        Some((flag, value)) if flag.len() > 1 && flag.starts_with('-') => {
            (flag, Some(value.to_string()))
        }
        _ => (word, None),
    }
}

/// Where the argv word `w`, read as a short cluster (`-sk`, `-oout.txt`),
/// holds the rule's one-letter flag `f` (`-k`): what follows each place the
/// letter stands among the cluster's leading letters (empty when it is the
/// last). Without a model `-sk` may be one flag, so this is contact only: a
/// deny or an ask fires on it, a grant never rests on it.
fn cluster_rests<'w>(w: &'w str, f: &str) -> Vec<&'w str> {
    let mut letter = f.strip_prefix('-').unwrap_or("").chars();
    let (Some(letter), None) = (letter.next(), letter.next()) else {
        return Vec::new();
    };
    // A cluster is a dashed word (`-sk`), never a bare one (`make check`
    // holds no `-k`), read by chars: a word opening with a multibyte
    // character is not sliced at byte one (review 2026-10-10, A and B).
    // Its letters run to the first character no option letter is — a
    // value may begin there (`-o./out`), so it is not a reason to stop
    // reading the word as a cluster (review 2026-10-11, E1).
    let Some(body) = w.strip_prefix('-').filter(|b| !b.starts_with('-')) else {
        return Vec::new();
    };
    if body.chars().count() < 2 {
        return Vec::new();
    }
    body.char_indices()
        .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '#')
        .take(super::acts::MAX_CLUSTER)
        .filter(|(_, c)| *c == letter)
        .map(|(at, c)| &body[at + c.len_utf8()..])
        .collect()
}

/// A dashed word of an unmodelled program's argv (`-` alone is stdin).
fn is_flag_word(w: &str) -> bool {
    w.starts_with('-') && w != "-"
}

/// Is the argv word `p` the rule's word `w`: the same, or `w` with a value
/// attached (`--jobs=4` is `--jobs`)?
fn flag_word_is(p: &str, w: &str) -> bool {
    p == w || p.strip_prefix(w).is_some_and(|r| r.starts_with('='))
}

/// `a ⊑ b` on forms: b's chain is a prefix of a's, b's flags and words are
/// among a's. Two regexes only when the same; a regex and a form never.
fn form_within(a: &Applied, b: &Applied) -> bool {
    if a.regex || b.regex {
        return a.regex && b.regex && a.line.cited.source == b.line.cited.source;
    }
    a.chain.len() >= b.chain.len()
        && a.chain[..b.chain.len()] == b.chain[..]
        && b.flags.iter().all(|(n, v)| {
            a.flags
                .iter()
                .any(|(an, av)| an == n && (v.is_none() || av == v))
        })
        // A rule naming an argument — a place among them — is more specific
        // than one naming none: `allow find -delete .` lifts `ask find
        // -delete`.
        && b.literal_words.iter().all(|w| a.literal_words.contains(w))
}

/// The rules on `act`'s program, each as it applies to it: direct, or
/// derived for an inner argv ([`applied`]).
fn applying<'g>(
    gate: &'g Gate,
    act: &ProgramAct,
    texts: &[String],
    main: bool,
) -> Vec<Applied<'g>> {
    gate.policy
        .lines
        .iter()
        .filter_map(|line| applied(line, act, texts, main))
        .collect()
}

/// Any rule on the program that gives a flag a value declares the flag
/// valued, for reading every rule and the command (§10, 10-10: a rule
/// naming it without `=` then matches every use).
fn valued_flags(applying: &[Applied]) -> Vec<String> {
    applying
        .iter()
        .flat_map(|a| &a.flags)
        .filter(|(_, v)| v.is_some())
        .map(|(n, _)| n.clone())
        .collect()
}

/// **Does `line`, cut down to `outer`'s chain, name the inner argv `outer`
/// runs?** — every inner act of `outer` (its `via`) met by the same rule,
/// DERIVED ([`derived`]), as that act's own decision meets it: the inner
/// program, then its words in their order. `allow uv run pytest *` cut down
/// to `uv run` covered the outer act whatever ran inside it, so `uv run ls
/// pytest` allowed — uv's sync included — with `ls` allowed on its own as a
/// read (review stop 12, T8; Steven, 10-27: "order of arguments matter").
///
/// - **Derived**: the rule read as the outer program's, naming the inner
///   argv — in `uv run uv run pytest` the outer one runs `uv`, which
///   `allow uv run pytest *` does not name. (Read directly, the same rule on
///   an inner act of the outer's own program could match only by cutting
///   down again, which one level never does: the two readings agree, an
///   equivalent mutant. Derived is the one that says what is meant.)
/// - **Every, not any**: two outer acts of one name and chain in a stage
///   share their inner acts' `via`, and an extra ask is the safe side of not
///   telling them apart.
/// - **One level**: an inner act that would itself be cut down is not
///   named. A rule written further out never covered the innermost act
///   (its `via` names the middle program, not the rule's), so such a line
///   asked already.
fn names_its_inner(
    gate: &Gate,
    line: &Line,
    outer: &ProgramAct,
    programs: &[&ProgramAct],
    texts: &[String],
) -> bool {
    let mut inner = programs.iter().copied().filter(|i| {
        !std::ptr::eq(*i, outer)
            && i.via
                .as_ref()
                .is_some_and(|(name, chain)| *name == outer.name && *chain == outer.chain)
    });
    let mut any = false;
    let every = inner.all(|i| {
        any = true;
        let valued = valued_flags(&applying(gate, i, texts, false));
        derived(line, i).is_some_and(|mut a| matches_program(&mut a, i, &valued, &|_| false))
    });
    any && every
}

fn decide_program(
    gate: &Gate,
    act: &ProgramAct,
    programs: &[&ProgramAct],
    texts: &[String],
    main: bool,
    writes_granted: bool,
    exec_granted: bool,
) -> ActVerdict {
    let kind = ActKind::Program {
        name: act.name.clone(),
        chain: act.chain.clone(),
        effect: act.effect,
        call: act.call.clone(),
        wrapper: act.wrapper,
        unknown: act
            .touches
            .iter()
            .filter(|t| t.unknown)
            .map(|t| t.name.clone())
            .chain(act.unknown_positionals.iter().cloned())
            .collect(),
    };
    let model = act.model.clone();
    // What a form must name, each once: a flag covers every time the call
    // gives it, and the lists below are read per occurrence — `A` offered
    // `allow sed -i -e -e -e -e` for four scripts (Steven, 11-02).
    let verdict = |decision, by, uncovered: Vec<String>| ActVerdict {
        act: kind.clone(),
        decision,
        by,
        uncovered: uncovered.into_iter().fold(Vec::new(), |mut once, item| {
            if !once.contains(&item) {
                once.push(item);
            }
            once
        }),
        model: model.clone(),
        at: act.written.clone(),
        reaches: Vec::new(),
    };
    // An exec grant on the path a program was run by stands in for an
    // UNMODELLED program (a script: running it is all it does); a modelled
    // one is decided by its model, as named bare — the grant covers the exec
    // act, never the flags (Steven, 10-11; review E1: `/usr/bin/gh auth
    // token` and `/bin/rm -rf` ran under a grant of the directory).
    let exec_granted = exec_granted && !act.modelled;
    // A deny or an ask with a path word constrains the PATH (`deny rm
    // /etc/*`: an rm acting there), never the program alone — its paths are
    // read as path rules scoped to the program (`path_cands`), and the form
    // says nothing here.
    let applying = applying(gate, act, texts, main);
    let valued = valued_flags(&applying);
    let names_inner = |line: &Line| names_its_inner(gate, line, act, programs, texts);
    let cands: Vec<Applied> = applying
        .into_iter()
        .filter(|a| {
            a.line.action == Decision::Allow
                || !matches!(&a.line.body, Body::Program(f) if !f.fences.is_empty())
        })
        .filter_map(|mut a| matches_program(&mut a, act, &valued, &names_inner).then_some(a))
        .collect();
    let pool = resolve(
        &cands,
        &|a: &Applied| a.line.action,
        &|a: &Applied| a.line.tier,
        &|a: &Applied, b: &Applied| form_within(a, b),
    );
    // A flag no rule names rides only where it changes nothing a rule could
    // be asked about: harmless (the model's mark, raised by any switch it
    // triggers) on a call that only reads — as it would run with no rule
    // at all, so an allow never makes `ls -la` stricter than no rule does.
    // Anywhere else a rule says which flags it approved (Steven, 10-10:
    // `allow rm` is not `rm -rf`; `allow rm *` is).
    // (A completion model's "harmless" is a guess, and backs nothing.)
    let completion = act
        .model
        .as_ref()
        .is_some_and(|m| m.trust == Trust::Completion);
    let rides = |t: &super::acts::Touch| {
        t.benign && !t.unknown && act.effect <= Effect::Read && !completion
    };
    let Some((decision, deciding)) = at(&pool, &|a: &Applied| a.line.action) else {
        // No rule: the default.
        let unknown: Vec<String> = act
            .touches
            .iter()
            .filter(|t| t.unknown)
            .map(|t| t.name.clone())
            .chain(act.unknown_positionals.iter().cloned())
            .collect();
        let (decision, why) = if exec_granted {
            (Decision::Allow, NoRule::Inside(Some(Effect::Exec)))
        } else if act.wrapper {
            (Decision::Allow, NoRule::Wrapper)
        } else if !act.modelled {
            (Decision::Ask, NoRule::Unmodelled)
        } else if completion {
            (Decision::Ask, NoRule::Completion)
        } else if !act.needs_named.is_empty() {
            (Decision::Ask, NoRule::NeedsNamed(act.needs_named.clone()))
        } else if !unknown.is_empty() {
            (Decision::Ask, NoRule::Unknown(unknown))
        } else if act
            .touches
            .iter()
            .any(|t| !t.benign && !t.unknown && t.why.is_some())
        {
            let whys = act
                .touches
                .iter()
                .filter(|t| !t.benign && !t.unknown)
                .filter_map(|t| t.why.clone())
                .collect();
            (Decision::Ask, NoRule::Guarded(whys))
        } else {
            // A writer whose every write a path rule grants runs — with no
            // flag a rule would have to name: the path rules grant the
            // places, and only a program rule approves its flags. Else
            // `allow :delete build` alone ran `rm -rf build`, which adding
            // `allow rm` then made ask (Steven, 10-10).
            let flagless = act.touches.iter().all(|t| t.unknown || rides(t));
            match act.effect {
                Effect::Read => (Decision::Allow, NoRule::ModelledRead),
                Effect::Write | Effect::Delete if writes_granted && flagless => {
                    (Decision::Allow, NoRule::Effect(act.effect))
                }
                e => (Decision::Ask, NoRule::Effect(e)),
            }
        };
        // Asking with no rule, the flags no form would carry unnamed — what
        // `A` names, read by this reader (a cluster is its letters: `tar
        // -xf a` names `-x`), never re-read from the words.
        let must_name = if decision == Decision::Allow {
            Vec::new()
        } else {
            act.touches
                .iter()
                .filter(|t| !t.unknown && !rides(t))
                .map(|t| format!("flag {}", t.names.first().unwrap_or(&t.name)))
                .collect()
        };
        return verdict(decision, By::Default(why), must_name);
    };
    let cited: Vec<Cited> = deciding.iter().map(|a| a.line.cited.clone()).collect();
    if decision != Decision::Allow {
        // What decided it is written where the rule's flags are: `deny rm
        // --no-preserve-root` is about that flag, not only about `rm`.
        let named = |t: &super::acts::Touch| {
            deciding.iter().any(|a| {
                a.flags
                    .iter()
                    .any(|(f, _)| *f == t.name || canon_of(act, f) == t.name || t.names.contains(f))
            })
        };
        let mut v = verdict(decision, By::Rules(cited), Vec::new());
        v.at.cause.extend(
            act.touches
                .iter()
                .filter(|t| named(t))
                .flat_map(|t| t.written.iter().cloned()),
        );
        v.at.cause.sort_by_key(|r| (r.start, r.end));
        v.at.cause.dedup();
        return v;
    }
    // Containment: every node the act touched is covered, or it asks.
    let allows: Vec<&Applied> = pool
        .iter()
        .filter(|a| a.line.action == Decision::Allow)
        .collect();
    let blanket = exec_granted || allows.iter().any(|a| a.blanket);
    // Named by the rule's canonical name, by its spelling, or — unmodelled
    // — as the word with a value attached (`--jobs=4` names `--jobs`).
    let names = |n: &str| {
        allows.iter().any(|a| {
            a.flags
                .iter()
                .any(|(f, _)| f == n || canon_of(act, f) == n || flag_word_is(n, f))
        })
    };
    // A word named as one the rule carries (its place, if it is one, is the
    // path rules' to judge).
    let named_word = |w: &String| allows.iter().any(|a| a.literal_words.contains(w));
    let mut uncovered: Vec<String> = Vec::new();
    if !blanket && !act.modelled {
        // Unmodelled, in argv order: a flag is covered by an allow naming
        // it; a declared flag's value goes with its flag (named with it
        // when the flag is not, so `A` names this value: `-l=rust`); the
        // arguments are the form's prefix, and past it are covered only by
        // a blanket.
        let longest_prefix = allows
            .iter()
            .map(|a| arguments(&a.words, &valued).len())
            .max()
            .unwrap_or(0);
        let values = values_of(&act.prefix_words, &valued);
        let mut arg = 0;
        for (k, w) in act.prefix_words.iter().enumerate() {
            if is_flag_word(w) {
                if !names(w) {
                    uncovered.push(match values.get(k + 1).copied().flatten() {
                        Some(_) => format!("flag {w}={}", act.prefix_words[k + 1]),
                        None => format!("flag {w}"),
                    });
                }
            } else if values[k].is_none() {
                if arg >= longest_prefix && !named_word(w) {
                    uncovered.push(format!("word {w}"));
                }
                arg += 1;
            }
        }
    } else if !blanket {
        for t in &act.touches {
            if !rides(t) && !names(&t.name) {
                uncovered.push(format!("flag {}", t.names.first().unwrap_or(&t.name)));
            }
        }
        for w in &act.unknown_positionals {
            if !named_word(w) {
                uncovered.push(format!("word {w}"));
            }
        }
        let longest_chain = allows.iter().map(|a| a.chain.len()).max().unwrap_or(0);
        for (k, node) in act.chain.iter().enumerate().skip(longest_chain) {
            // A read node rides; any other must be named — and under a
            // completion model none rides, its effects being a guess (review
            // 2026-10-11, E1: `allow bat` covered `bat cache`).
            if !completion
                && act
                    .chain_effects
                    .get(k + 1)
                    .is_some_and(|e| *e <= Effect::Read)
            {
                continue;
            }
            uncovered.push(format!("subcommand {node}"));
        }
    }
    if !allows.iter().any(|a| a.chain == act.chain) {
        uncovered.extend(act.needs_named.iter().cloned());
    }
    if uncovered.is_empty() {
        verdict(Decision::Allow, By::Rules(cited), Vec::new())
    } else {
        verdict(Decision::Ask, By::Rules(cited), uncovered)
    }
}

// ---- path acts ---------------------------------------------------------------------

#[derive(Clone)]
struct PathCand<'a> {
    line: &'a Line,
    pat: Resolved,
    /// Met only as a directory ABOVE the pattern (`rm -rf /` under `deny rm
    /// /etc/**`), or only by CONTACT — a glob that could pick what the
    /// pattern names (`cat *` under a `*.pem` guard): a fence there asks
    /// rather than refuses — whether the command reaches what the rule
    /// names is not in its words (Steven, 10-06 and 10-10: a "may name"
    /// asks, never denies).
    soft: bool,
}

/// The path rules that apply to the act: path rules at an operation that
/// covers it, scoped to its program or to none, and the fences of deny and
/// ask rules on its program whose form the stage's command meets (`active`:
/// `deny docker pull ubuntu:*` fences nothing off a `docker run`) — each
/// naming every place the act may be at.
/// No carve-out place is looked for deeper than this — far below any real
/// one (Claude Code's memory, `/Users/<you>/.claude/projects/<p>/memory`, is
/// six down) — nor longer: a word past either bound is not carved, and is
/// held (it fails closed). Bounded so the work is: a model-written
/// `~/.claude/a/a/…/x` of 16 KB took seconds (Reviewer B, round 28).
const DEEPEST_PLACE: usize = 32;
const LONGEST_PLACE: usize = 4096;

/// **Is `path` carved out of the guard `guard`** — inside a `[secrets]
/// paths_except` place the guard holds WHOLE: the shortest leading part of
/// the path an exception names, which the guard matches. `~/.claude` holds
/// Claude Code's memory place, so a memory there is free; `*.pem` does not
/// hold the place, so a `key.pem` inside it is still a credential. A place
/// is carved out of what contains it, never out of what it contains
/// (Steven, 10-06). Every exception lifted every guard, so a `.pem`, an
/// `id_rsa` or a `.env` in a carved place read unasked (review stop 12,
/// found porting the old matcher's tests to the gate).
fn carved(guard: &super::line::Resolved, except: &[super::line::Resolved], path: &str) -> bool {
    if except.is_empty() {
        return false;
    }
    path.match_indices('/')
        .map(|(at, _)| at)
        .filter(|&at| at > 0)
        .chain([path.len()])
        .take_while(|&end| end <= LONGEST_PLACE)
        .take(DEEPEST_PLACE)
        .map(|end| &path[..end])
        .find(|place| except.iter().any(|e| e.matches(place)))
        .is_some_and(|place| guard.matches(place))
}

fn path_cands<'a>(gate: &'a Gate, act: &PathAct, active: &[(&Line, String)]) -> Vec<PathCand<'a>> {
    let (start, home) = (gate.cx.start, gate.cx.home);
    // An allow covers the act only at EVERY place it may be at (past a `cd`
    // that may have failed); a deny or an ask meets it at any — or, failing
    // that, at a directory above what it names (`above`).
    // A grant of a delete (or of a word at no operation, which may be one)
    // written `DIR/**` stops at the directory: `allow rm build/**` does not
    // cover `rm -rf build` (Steven, 10-10); a read of the directory is a
    // read of what is below it, and is covered.
    let strict = matches!(act.op, Some(Effect::Delete) | None);
    let names = |pat: &PathPat, paths: &[&String], every: bool| -> Option<(Resolved, bool)> {
        let r = pat.resolve(start, home);
        if every {
            let granted = |p: &&String| {
                if strict {
                    r.grants_delete(p)
                } else {
                    r.matches(p)
                }
            };
            return paths.iter().all(granted).then_some((r, false));
        }
        if paths.iter().any(|p| r.matches(p)) {
            return Some((r, false));
        }
        // Met by contact or from above: the rule applies, softly. A listing
        // is never above what it lists: names reach the output, not what the
        // files below hold (Steven, 10-11: `git status` and `ls` asked for a
        // key file under the start).
        paths
            .iter()
            .any(|p| r.reaches(p) || (!act.listing && r.above(p)))
            .then_some((r, true))
    };
    let all_paths: Vec<&String> = act.paths.iter().collect();
    let mut out = Vec::new();
    for line in &gate.policy.lines {
        match &line.body {
            Body::Path {
                op,
                pat,
                scope,
                except,
            } => {
                let scoped = scope.as_ref().is_none_or(|s| *s == act.program);
                // The guard applies to the places NOT carved out: past a `cd`
                // that may have failed the file is in one of several places,
                // and only the ones outside an exception are the guard's
                // (review 2026-10-10, E1: it over-asked).
                let guard = pat.resolve(start, home);
                let except: Vec<_> = except.iter().map(|e| e.resolve(start, home)).collect();
                let open: Vec<&String> = act
                    .paths
                    .iter()
                    .filter(|p| !carved(&guard, &except, p))
                    .collect();
                // A shipped guard found anywhere (`**/.claude`) names what a
                // project holds — so inside where Shannon started it is read
                // below the start: a `.claude` that CONTAINS the start (a
                // project in `.claude/worktrees/x`) is not the project's,
                // and guarded every path in it (Steven, 10-11). Outside the
                // start, and at or below it, the guard is as ever.
                let below: Vec<String>;
                let open = if line.tier == Tier::Shipped
                    && matches!(pat, PathPat::Path { anywhere: true, .. })
                {
                    below = open
                        .iter()
                        .filter_map(|p| match p.strip_prefix(start) {
                            Some(rest) if start != "/" && rest.starts_with('/') => {
                                Some(rest.to_string())
                            }
                            Some(_) if p.as_str() == start => None,
                            _ => Some(p.to_string()),
                        })
                        .collect();
                    below.iter().collect()
                } else {
                    open
                };
                if scoped
                    && !open.is_empty()
                    && op.covers(act.op, line.action != Decision::Allow)
                    && let Some((pat, soft)) = names(pat, &open, line.action == Decision::Allow)
                {
                    out.push(PathCand { line, pat, soft });
                }
            }
            // A deny's or an ask's path words fence the place off from the
            // program — the files it names, and a redirection's target or a
            // wrapper's argument too: the stage touches the place on its
            // behalf, and a deny never silently stops protecting it (Steven,
            // 10-11: `deny cat /srv/keep/**` meets `cat a > /srv/keep/x`).
            // For the program the rule's form applied to: `deny uv run rm
            // /srv/keep` fences the inner `rm` (E1). (An allow's path words
            // grant nothing: they are arguments, and a place is granted by a
            // path rule — Steven, 10-10.)
            Body::Program(form)
                if active
                    .iter()
                    .any(|(l, name)| std::ptr::eq(*l, line) && *name == act.program) =>
            {
                for g in &form.fences {
                    if let Some((pat, soft)) = names(g, &all_paths, line.action == Decision::Allow)
                    {
                        out.push(PathCand { line, pat, soft });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The deny and ask rules with fences whose form (chain, flags, words) the
/// stage's program acts meet: the ones whose path words apply to the
/// stage's path acts.
fn active_program_rules<'a>(
    gate: &'a Gate,
    acts: &[Act],
    texts: &[String],
) -> Vec<(&'a Line, String)> {
    let mut out = Vec::new();
    for line in &gate.policy.lines {
        if !matches!(&line.body, Body::Program(f) if !f.fences.is_empty()) {
            continue;
        }
        // Each program act the rule's form applies to — the outer program,
        // or an inner one the chain runs into — names the program its path
        // words fence. (Only a deny or an ask has fences, and contact reads
        // no declared value: its words are met anywhere.)
        for a in acts {
            // (No allow is here, so none is cut down: `names_inner` is moot.)
            if let Act::Program(p) = a
                && applied(line, p, texts, true)
                    .is_some_and(|mut applied| matches_program(&mut applied, p, &[], &|_| false))
            {
                out.push((line, p.name.clone()));
            }
        }
    }
    out
}

/// Decide a path act under every program rule: for a credential lookup
/// (`RuleSet::credential_path`), where no command is being read.
pub(super) fn decide_path_alone(gate: &Gate, act: &PathAct) -> ActVerdict {
    let active: Vec<(&Line, String)> = gate
        .policy
        .lines
        .iter()
        .filter(|line| matches!(&line.body, Body::Program(_)))
        .map(|line| (line, act.program.clone()))
        .collect();
    decide_path(gate, act, &active)
}

fn decide_path(gate: &Gate, act: &PathAct, active: &[(&Line, String)]) -> ActVerdict {
    let kind = ActKind::Path {
        op: act.op,
        paths: act.paths.clone(),
        word: act.word.clone(),
        indirect: act.redirect,
    };
    let verdict = |decision, by, reaches| ActVerdict {
        act: kind.clone(),
        decision,
        by,
        uncovered: Vec::new(),
        model: None,
        at: path_written(act),
        reaches,
    };
    // `DIR/**` names less than `DIR` only for what may be a delete.
    let delete = matches!(act.op, Some(Effect::Delete) | None);
    let cands = path_cands(gate, act, active);
    // The reason: the rules that stand at the decision's strictness ([`at`]).
    let cite = |standing: &[PathCand]| {
        let deciding = at(standing, &|c: &PathCand| c.action()).map_or(Vec::new(), |(_, d)| d);
        By::Rules(deciding.into_iter().map(PathCand::cited).collect())
    };
    // The place's own account: the rules naming it, else the default.
    let account = |named: &[PathCand]| match ruled(named, delete) {
        Some((own, standing)) => (own, cite(&standing)),
        None => {
            let (own, why) = path_default(gate, act);
            (own, By::Default(why))
        }
    };
    let Some((decision, standing)) = ruled(&cands, delete) else {
        let (decision, by) = account(&[]);
        return verdict(decision, by, Vec::new());
    };
    if !cands.iter().any(|c| c.soft) {
        return verdict(decision, cite(&standing), Vec::new());
    }
    // The place's own account is said first where it decides as strictly:
    // what it is met by from above then says what no grant of the place
    // lifts. `rm -r /` is outside where Shannon started, and holding
    // Shannon's state is the second thing (Steven, 10-13). Where the place
    // would be allowed (`rg foo .`), what it is met by is the reason. So
    // `by` cites rules that name the place or rules met softly, never both:
    // a named one as strict as the decision makes the account decide.
    let named: Vec<PathCand> = cands.iter().filter(|c| !c.soft).cloned().collect();
    let (own, by) = account(&named);
    if own == decision {
        let met = standing
            .iter()
            .filter(|c| c.soft && c.action() == decision)
            .map(PathCand::cited)
            .collect();
        return verdict(decision, by, met);
    }
    verdict(decision, cite(&standing), Vec::new())
}

impl PathCand<'_> {
    /// What the rule says of the act. Met only softly — from above, or by a
    /// glob's contact — a deny asks: the words do not say whether the
    /// command reaches what the rule names.
    fn action(&self) -> Decision {
        match self.soft {
            true => self.line.action.min(Decision::Ask),
            false => self.line.action,
        }
    }

    /// The rule, said as it was met: by name, or from above — "`.` holds
    /// Shannon's state", never "`.` matches" it.
    fn cited(&self) -> Cited {
        Cited {
            reached: self.soft,
            ..self.line.cited.clone()
        }
    }
}

/// The decision the path rules among `cands` give, and the rules that give
/// it: the most specific by their paths (for an act that may be a
/// `delete`, `DIR/**` is narrower than `DIR`) — save that a rule met only
/// softly never takes the place of one naming the act. It says what lies
/// below the place, so it only adds strictness: the user's own `ask :*
/// ~/.ssh` under their `deny :delete ~` made `rm -r ~` ask (10-13). (Of
/// equal patterns, neither is narrower: both stand, and the stricter
/// decides.)
fn ruled<'a>(cands: &[PathCand<'a>], delete: bool) -> Option<(Decision, Vec<PathCand<'a>>)> {
    let within = |a: &PathCand, b: &PathCand| {
        a.pat.within_as(&b.pat, delete) && !(a.soft && !b.soft && !b.pat.within_as(&a.pat, delete))
    };
    let standing = resolve(
        cands,
        &|c: &PathCand| c.action(),
        &|c: &PathCand| c.line.tier,
        &within,
    );
    let decision = standing.iter().map(PathCand::action).max()?;
    Some((decision, standing))
}

/// A place no rule speaks of: inside where Shannon started a read, a write
/// or a word at no operation is allowed and a delete or an exec asks;
/// outside, it asks.
fn path_default(gate: &Gate, act: &PathAct) -> (Decision, NoRule) {
    let inside = act.paths.iter().all(|p| in_tree(p, gate.cx.start));
    match (inside, act.op) {
        (false, _) => (Decision::Ask, NoRule::Outside),
        (true, Some(Effect::Read) | Some(Effect::Write) | None) => {
            (Decision::Allow, NoRule::Inside(act.op))
        }
        (true, op) => (Decision::Ask, NoRule::Inside(op)),
    }
}

/// A path act is written at its word: it has no verb of its own.
fn path_written(act: &PathAct) -> Written {
    Written {
        verb: Vec::new(),
        cause: act.at.clone(),
    }
}

// ---- host acts ---------------------------------------------------------------------

#[derive(Clone)]
struct HostCand<'a> {
    line: &'a Line,
    pat: &'a HostPat,
}

fn decide_host(gate: &Gate, dir: Dir, host: &str) -> ActVerdict {
    let kind = ActKind::Host {
        dir,
        host: host.to_string(),
    };
    let cands: Vec<HostCand> = gate
        .policy
        .lines
        .iter()
        .filter_map(|line| match &line.body {
            Body::Host { dir: d, host: pat } if d.is_none_or(|d| d == dir) && pat.matches(host) => {
                Some(HostCand { line, pat })
            }
            _ => None,
        })
        .collect();
    let standing = resolve(
        &cands,
        &|c: &HostCand| c.line.action,
        &|c: &HostCand| c.line.tier,
        &|a: &HostCand, b: &HostCand| a.pat.within(b.pat),
    );
    match at(&standing, &|c: &HostCand| c.line.action) {
        Some((decision, deciding)) => ActVerdict {
            act: kind,
            decision,
            by: By::Rules(deciding.iter().map(|c| c.line.cited.clone()).collect()),
            uncovered: Vec::new(),
            model: None,
            at: Written::default(),
            reaches: Vec::new(),
        },
        None => ActVerdict {
            act: kind,
            decision: Decision::Ask,
            by: By::Default(NoRule::Host),
            uncovered: Vec::new(),
            model: None,
            at: Written::default(),
            reaches: Vec::new(),
        },
    }
}

// ---- the stage ---------------------------------------------------------------------

fn hard(act: ActKind, decision: Decision, why: Hard) -> ActVerdict {
    ActVerdict {
        act,
        decision,
        by: By::Hard(why),
        uncovered: Vec::new(),
        model: None,
        at: Written::default(),
        reaches: Vec::new(),
    }
}

/// `v`, written at `cause` — an act whose word the decision does not see
/// (a host, code, an unplaced word, a privilege wrapper).
fn written(v: ActVerdict, cause: &[std::ops::Range<usize>]) -> ActVerdict {
    ActVerdict {
        at: Written {
            verb: Vec::new(),
            cause: cause.to_vec(),
        },
        ..v
    }
}

/// **The allows of the user's that lift a privilege wrapper's ask** (Steven,
/// 10-11): on the wrapper, naming the program it runs as the chain's first
/// word, and among the flags written before it every flag given the wrapper
/// with its value (`allow sudo -u=postgres psql *` lifts `sudo -u postgres
/// psql -l`, not `sudo -E psql`). `allow sudo *` names no program and lifts
/// nothing; a shipped rule never does. What it runs is judged on its own
/// acts — its program act by the same rule, derived; its files by the path
/// rules.
fn privilege_lift(gate: &Gate, pv: &super::acts::Privileged) -> Option<Vec<Cited>> {
    let own = pv.own.as_ref()?;
    let runs = pv.runs.as_deref()?;
    let lifts: Vec<Cited> = gate
        .policy
        .lines
        .iter()
        .filter(|l| l.action == Decision::Allow && l.tier != Tier::Shipped)
        .filter(|l| {
            let Body::Program(f) = &l.body else {
                return false;
            };
            let named = &f.flags[..f.flags_before(0).min(f.flags.len())];
            let is = |t: &super::acts::Touch, (n, v): &(String, Option<String>)| {
                (*n == t.name || t.names.contains(n)) && value_matches(v, &t.value)
            };
            // Both ways: every flag given is named, and every flag named is
            // given — `allow sudo -u=postgres psql *` is not `sudo psql`,
            // which runs it as root.
            f.program.eq_ignore_ascii_case(&pv.wrapper)
                && f.chain.first().is_some_and(|c| c == runs)
                && own.iter().all(|t| named.iter().any(|n| is(t, n)))
                && named.iter().all(|n| own.iter().any(|t| is(t, n)))
        })
        .map(|l| l.cited.clone())
        .collect();
    (!lifts.is_empty()).then_some(lifts)
}

/// The stage as a `re:` rule reads it: its words joined by single spaces,
/// and with a program named by a path also by its name.
fn stage_texts(stage: &Stage, home: &str) -> Vec<String> {
    let words = &stage.command.words;
    let mut texts = vec![words.join(" ")];
    if let Some((_, name)) = words.first().and_then(|w| w.rsplit_once('/'))
        && !name.is_empty()
    {
        let mut named = vec![name.to_string()];
        named.extend(words[1..].iter().cloned());
        texts.push(named.join(" "));
    }
    // And with every `~` the shell expands spelled as the home it names
    // (`dd if=~/x`: bash expands that one too), so a regex naming home's
    // full path meets the line.
    let expanded: Vec<String> = words
        .iter()
        .map(|w| {
            if let Some(rest) = w.strip_prefix("~/") {
                format!("{home}/{rest}")
            } else if let Some((k, v)) = w.split_once("=~/") {
                format!("{k}={home}/{v}")
            } else {
                w.clone()
            }
        })
        .collect();
    if expanded != *words {
        texts.push(expanded.join(" "));
    }
    texts
}

/// Decide one stage: its acts, each by the rules that apply; the hard
/// asks; the strictest of them.
pub fn decide_stage(gate: &Gate, stage: &Stage) -> StageVerdict {
    let acts = acts_of(stage, gate.cx);
    let texts = stage_texts(stage, gate.cx.home);
    let mut out: Vec<ActVerdict> = Vec::new();
    // Paths and hosts first: a writer's default depends on its grants —
    // PER PROGRAM, from the files that program itself names, never from a
    // redirection: a grant of `> /tmp/scratch/log` said nothing about what
    // `git reset --hard` writes, and once let it run (review 2026-10-10,
    // A). A program whose write names no file (the node's own effect) is
    // granted nothing here: no file, no grant.
    let active = active_program_rules(gate, &acts, &texts);
    // program → (write/delete acts it named, every one of them granted by rule)
    let mut writes: HashMap<&str, (usize, bool)> = HashMap::new();
    // The exec path acts a rule granted, by their word.
    let mut exec_granted_words: Vec<&str> = Vec::new();
    for act in &acts {
        match act {
            Act::Path(p) => {
                let v = decide_path(gate, p, &active);
                let by_rule = matches!(v.by, By::Rules(_)) && v.decision == Decision::Allow;
                if !p.redirect && matches!(p.op, Some(Effect::Write) | Some(Effect::Delete)) {
                    let e = writes.entry(p.program.as_str()).or_insert((0, true));
                    e.0 += 1;
                    e.1 &= by_rule;
                }
                if p.op == Some(Effect::Exec) && by_rule {
                    exec_granted_words.push(p.word.as_str());
                }
                out.push(v);
            }
            Act::Host { dir, host, at, .. } => out.push(written(decide_host(gate, *dir, host), at)),
            Act::Code { program, what, at } => out.push(written(
                hard(
                    ActKind::Code {
                        program: program.clone(),
                        what: what.clone(),
                    },
                    Decision::Ask,
                    Hard::Code,
                ),
                at,
            )),
            Act::Privilege(pv) => {
                // A hard ask. A deny of the wrapper by the user (any tier of
                // theirs, or the project's) makes it a refusal; an allow of
                // theirs naming what it runs lifts it (`privilege_lift`).
                let w = &pv.wrapper;
                let denies: Vec<Cited> = gate
                    .policy
                    .lines
                    .iter()
                    .filter(|l| {
                        l.action == Decision::Deny
                            && l.tier != Tier::Shipped
                            && matches!(&l.body, Body::Program(f) if f.program.eq_ignore_ascii_case(w) && f.chain.is_empty() && f.flags.is_empty())
                    })
                    .map(|l| l.cited.clone())
                    .collect();
                let (decision, by) = if !denies.is_empty() {
                    (Decision::Deny, By::Rules(denies))
                } else if let Some(lifts) = privilege_lift(gate, pv) {
                    (Decision::Allow, By::Rules(lifts))
                } else {
                    (Decision::Ask, By::Hard(Hard::Privilege))
                };
                out.push(ActVerdict {
                    act: ActKind::Privilege(w.clone()),
                    decision,
                    by,
                    uncovered: Vec::new(),
                    model: None,
                    at: Written {
                        verb: Vec::new(),
                        cause: pv.at.clone(),
                    },
                    reaches: Vec::new(),
                });
            }
            // Of the whole stage: it cannot be read soundly anywhere.
            Act::Unsound(why) => out.push(written(
                hard(ActKind::Unsound(why.clone()), Decision::Ask, Hard::Unsound),
                std::slice::from_ref(&stage.span),
            )),
            Act::Unplaced(w, at) => out.push(written(
                hard(ActKind::Unplaced(w.clone()), Decision::Ask, Hard::Unplaced),
                at,
            )),
            Act::Program(_) => {}
        }
    }
    // The stage's own command: run plainly, or via a peeled wrapper — what
    // a `re:` rule reads. An inner argv (`find -exec rm`) is not it.
    let wrappers: Vec<&str> = acts
        .iter()
        .filter_map(|a| match a {
            Act::Program(p) if p.wrapper => Some(p.name.as_str()),
            _ => None,
        })
        .collect();
    let programs: Vec<&ProgramAct> = acts
        .iter()
        .filter_map(|a| match a {
            Act::Program(p) => Some(&**p),
            _ => None,
        })
        .collect();
    for act in &acts {
        if let Act::Program(p) = act {
            // By identity, not by name: an inner `timeout` read from a `find
            // -exec` is not the peeled one, and what it runs is not the
            // stage's command (review 2026-10-10, E1).
            let main = !p.wrapper
                && !p.inner
                && p.via.as_ref().is_none_or(|(outer, chain)| {
                    chain.is_empty() && wrappers.contains(&outer.as_str())
                });
            // The exec grant covers the one act run by that path (`./x.sh`),
            // never an inner argv it runs (review 2026-10-10, A).
            let exec_granted = p
                .exec_of
                .as_deref()
                .is_some_and(|w| exec_granted_words.contains(&w));
            let writes_granted = writes
                .get(p.name.as_str())
                .is_some_and(|(n, all)| *n > 0 && *all);
            out.push(decide_program(
                gate,
                p,
                &programs,
                &texts,
                main,
                writes_granted,
                exec_granted,
            ));
        }
    }
    // A stage that spends a secret always faces a human, whatever a rule
    // allows: the line that runs is not the line the rule described. An
    // unminted handle is included on purpose — it is the shape of an
    // injection, and must reach a human rather than be dropped.
    if let Some(handle) = super::stage_tokens(stage).find_map(|t| {
        crate::secrets::secret_handles_in(t)
            .first()
            .map(|h| h.to_string())
    }) {
        let known = (gate.knows)(&handle);
        // Written in the word that holds it.
        let at: Vec<std::ops::Range<usize>> = stage
            .command
            .expansion
            .iter()
            .position(|(w, _)| w.contains(&handle))
            .and_then(|j| stage.word_spans.get(j).cloned())
            .into_iter()
            .collect();
        out.push(written(
            hard(
                ActKind::Secret { handle, known },
                Decision::Ask,
                Hard::Secret,
            ),
            &at,
        ));
    }
    let decision = out
        .iter()
        .map(|a| a.decision)
        .max()
        .unwrap_or(Decision::Allow);
    StageVerdict {
        decision,
        acts: out,
    }
}

/// Decide a line: every stage, or why it could not be read at all.
pub fn decide_line(gate: &Gate, input: &str) -> Result<Vec<StageVerdict>, Unanalyzable> {
    let pipeline = parse_line(input)?;
    Ok(pipeline
        .stages
        .iter()
        .map(|s| decide_stage(gate, s))
        .collect())
}

/// The line's decision: its strictest stage; nothing parsed is not
/// permission.
pub fn decision_of(stages: &[StageVerdict]) -> Decision {
    stages
        .iter()
        .map(|s| s.decision)
        .max()
        .unwrap_or(Decision::Ask)
}

/// The executor's environment as the reader's table. What is withheld is
/// taken out by the rule set's own reading (`RuleSet::with_exec_env`).
pub fn env_table(configured: &[(String, String)]) -> BTreeMap<String, String> {
    crate::exec::child_env(configured, &[])
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::line;
    use crate::permission::model::Models;
    use crate::permission::{Origin, defaults};

    const START: &str = "/work/me/proj";

    /// The home the SHELL expands `~` with at parse time (`cd ~/notes`
    /// arrives resolved), so the reader must resolve a rule's `~` the same.
    fn home() -> &'static str {
        crate::secrets::home_dir().expect("tests run with a HOME")
    }

    /// A policy as `RuleSet::policy` assembles one: the fixed shipped rules
    /// and the credential guards, then the rules given at their tiers.
    fn policy(shipped: bool, rules: &[(&str, Tier)]) -> Policy {
        let mut p = Policy::default();
        if shipped {
            let (guards, notes) =
                defaults::credential_bodies(&crate::SecretsConfig::default(), &[], &[]);
            assert!(notes.is_empty(), "{notes:?}");
            let fixed = defaults::fixed().into_iter().map(|r| {
                let parsed = line::parse(&r.source, Models::shipped()).unwrap();
                (r.source, parsed)
            });
            let guards = guards.into_iter().map(|(t, p, g)| (t, p, Some(g)));
            for (text, parsed, guard) in fixed.map(|(t, p)| (t, p, None)).chain(guards) {
                p.push(
                    parsed.clone(),
                    Tier::Shipped,
                    Cited {
                        action: parsed.action,
                        source: text.clone(),
                        origin: Origin::Shipped,
                        guard,
                        reached: false,
                    },
                );
            }
        }
        for (text, tier) in rules {
            let parsed =
                line::parse(text, Models::shipped()).unwrap_or_else(|e| panic!("{text}: {e}"));
            let origin = match tier {
                Tier::Session => Origin::Session,
                _ => Origin::Config,
            };
            p.push(
                parsed.clone(),
                *tier,
                Cited {
                    // As `RuleSet::policy` cites it: a deny cited as an
                    // allow is a state the engine never makes.
                    action: parsed.action,
                    source: text.to_string(),
                    origin,
                    guard: None,
                    reached: false,
                },
            );
        }
        p
    }

    fn decide(policy: &Policy, start: &str, input: &str) -> Decision {
        let env = env_table(&[]);
        let cx = Context {
            start,
            home: home(),
            models: Models::shipped(),
            env: &env,
        };
        let knows = |_: &str| false;
        let gate = Gate {
            policy,
            cx: &cx,
            knows: &knows,
        };
        match decide_line(&gate, input) {
            Ok(stages) => decision_of(&stages),
            Err(_) => Decision::Ask,
        }
    }

    /// The acts of the first stage, for a test that reads why.
    fn decide_acts(policy: &Policy, start: &str, input: &str) -> Vec<ActVerdict> {
        let env = env_table(&[]);
        let cx = Context {
            start,
            home: home(),
            models: Models::shipped(),
            env: &env,
        };
        let knows = |_: &str| false;
        let gate = Gate {
            policy,
            cx: &cx,
            knows: &knows,
        };
        decide_line(&gate, input).unwrap().remove(0).acts
    }

    fn user(rules: &[&str]) -> Policy {
        let rules: Vec<(&str, Tier)> = rules.iter().map(|r| (*r, Tier::User)).collect();
        policy(true, &rules)
    }

    /// **Trust, then specificity.** The user's config files and session are
    /// one group (the project's file among them, Steven 10-13); the shipped
    /// rules narrow beneath them.
    #[test]
    fn a_lower_tier_only_narrows() {
        // A shipped allow cannot widen the user's deny.
        let p = policy(
            true,
            &[("allow rm *", Tier::Shipped), ("deny rm", Tier::User)],
        );
        assert_eq!(decide(&p, START, "rm build/x"), Decision::Deny);
        // A shipped deny stays under a broad user allow …
        let p = policy(
            true,
            &[
                ("deny :* /mnt", Tier::Shipped),
                ("allow :read /", Tier::User),
            ],
        );
        assert_eq!(decide(&p, START, "cat /mnt/x"), Decision::Deny);
        // … and is lifted by a user allow at least as specific.
        let p = policy(
            true,
            &[
                ("deny :* /mnt", Tier::Shipped),
                ("allow :read /mnt/x", Tier::User),
            ],
        );
        assert_eq!(decide(&p, START, "cat /mnt/x"), Decision::Allow);
        // With no user rule, the shipped decide, with their exceptions.
        let p = policy(
            true,
            &[
                ("deny :* /mnt", Tier::Shipped),
                ("allow :read /mnt/pub", Tier::Shipped),
            ],
        );
        assert_eq!(decide(&p, START, "cat /mnt/pub/x"), Decision::Allow);
        assert_eq!(decide(&p, START, "cat /mnt/x"), Decision::Deny);
        // Config and session are one group: the more specific decides, and
        // among equals the strictest.
        let p = policy(
            true,
            &[
                ("allow :read ~", Tier::Session),
                ("deny :* ~/private", Tier::User),
            ],
        );
        assert_eq!(decide(&p, START, "cat ~/private/x"), Decision::Deny);
        let p = policy(
            true,
            &[
                ("allow gh pr create", Tier::Session),
                ("deny gh pr create", Tier::User),
            ],
        );
        assert_eq!(decide(&p, START, "gh pr create --title x"), Decision::Deny);
    }

    /// **The shipped guards**: the credential paths ask beneath any allow
    /// that does not name them, the `paths_except` places are carved out,
    /// a write into a harness surface asks, and `rm --no-preserve-root` is
    /// refused under a blanket.
    #[test]
    fn the_shipped_guards_stand_until_named() {
        let p = user(&["allow :read ~", "allow cat *"]);
        assert_eq!(decide(&p, START, "cat ~/.ssh/config"), Decision::Ask);
        assert_eq!(decide(&p, START, "cat ~/.aws/credentials"), Decision::Ask);
        assert_eq!(
            decide(&p, START, "cat ~/.claude/projects/p/memory/notes.md"),
            Decision::Allow,
            "carved out"
        );
        assert_eq!(
            decide(&p, START, "cat ~/.claude/settings.json"),
            Decision::Ask
        );
        assert_eq!(decide(&p, START, "cat ~/notes/a.md"), Decision::Allow);
        // Carved out is not granted: with no rule of the user's the memory
        // place is a read outside the project, which asks as any other.
        let bare = user(&[]);
        assert_eq!(
            decide(&bare, START, "cat ~/.claude/projects/p/memory/notes.md"),
            Decision::Ask
        );
        assert!(matches!(
            decide_acts(&bare, START, "cat ~/.claude/projects/p/memory/notes.md")[0].by,
            By::Default(NoRule::Outside)
        ));
        let p = user(&["allow :read ~/.ssh"]);
        assert_eq!(
            decide(&p, START, "cat ~/.ssh/config"),
            Decision::Allow,
            "named: lifted"
        );
        let p = user(&["allow :write ."]);
        assert_eq!(
            decide(&p, START, "echo x > .claude/settings.json"),
            Decision::Ask
        );
        assert_eq!(decide(&p, START, "echo x > notes.md"), Decision::Allow);
        let p = user(&["allow :write .claude"]);
        assert_eq!(
            decide(&p, START, "echo x > .claude/settings.json"),
            Decision::Allow
        );
        let p = user(&["allow rm *", "allow :delete /"]);
        assert_eq!(
            decide(&p, START, "rm -rf --no-preserve-root /"),
            Decision::Deny
        );
        // A credential file in the project asks too — the guard is a name
        // anywhere, case folded.
        let p = user(&["allow cat *"]);
        assert_eq!(decide(&p, START, "cat .ENV"), Decision::Ask);
        assert_eq!(decide(&p, START, "cat keys/server.pem"), Decision::Ask);
    }

    /// **The hard asks** nothing lifts: a secret handle, a privilege wrapper
    /// (which a user deny refuses, and only an allow naming what it runs
    /// lifts — `a_privilege_wrapper_is_lifted_only_by_naming_what_it_runs`),
    /// an unsound stage, an unplaced word.
    #[test]
    fn hard_asks_are_lifted_by_nothing() {
        let p = user(&["allow curl *", "allow :fetch api.example.com", "allow ls *"]);
        assert_eq!(
            decide(
                &p,
                START,
                "curl -H 'Authorization: {{a.1234}}' https://api.example.com/x"
            ),
            Decision::Ask
        );
        assert_eq!(decide(&p, START, "sudo ls"), Decision::Ask);
        // An alias of a name the rules allow: `ll` asked as unmodelled
        // whatever the unsound stage did (review stop 12, T8 M9). `ls src`
        // alone allows, so only the disturbed names can ask here.
        assert_eq!(decide(&p, START, "ls src"), Decision::Allow, "premise");
        assert_eq!(
            decide(&p, START, "alias ls='rm -rf'; ls src"),
            Decision::Ask
        );
        assert_eq!(decide(&p, START, "cat $(ls src)"), Decision::Ask);
        let p = user(&["deny sudo", "allow ls *"]);
        assert_eq!(decide(&p, START, "sudo ls"), Decision::Deny);
        // The wrapper itself, run plainly, is allowed: what it runs is judged.
        let p = user(&[]);
        assert_eq!(decide(&p, START, "timeout 5 ls src"), Decision::Allow);
        assert_eq!(decide(&p, START, "timeout 5 rm src/x"), Decision::Ask);
    }

    /// **A privilege wrapper is lifted only by an allow naming what it runs**
    /// and exactly the flags given the wrapper (Steven, 10-11: "granting sudo
    /// doesn't grant everything, only everything user defines"); a flag that
    /// hands the command to a shell, and `su`, are lifted by nothing; the
    /// files are the path rules' as ever.
    #[test]
    fn a_privilege_wrapper_is_lifted_only_by_naming_what_it_runs() {
        let p = user(&["allow sudo ls *"]);
        assert_eq!(decide(&p, START, "sudo ls -la src"), Decision::Allow);
        assert_eq!(
            decide(&p, START, "sudo cat src/a"),
            Decision::Ask,
            "another program"
        );
        assert_eq!(
            decide(&p, START, "sudo ls /srv/x"),
            Decision::Ask,
            "a place outside"
        );
        assert_eq!(
            decide(&p, START, "sudo -E ls src"),
            Decision::Ask,
            "an unnamed flag"
        );
        assert_eq!(
            decide(&p, START, "timeout -s KILL 5 sudo ls src"),
            Decision::Allow,
            "another wrapper's flags are its own, not sudo's"
        );
        assert_eq!(
            decide(&p, START, "find src -exec sudo ls {} ;"),
            Decision::Ask,
            "an inner sudo, under no allow of find"
        );
        let p = user(&["allow find *", "allow sudo ls *"]);
        assert_eq!(
            decide(&p, START, "find src -exec sudo ls {} ;"),
            Decision::Allow,
            "an inner sudo is lifted by the same rule"
        );
        for blanket in ["allow sudo *", "allow sudo", "allow * *"] {
            let p = user(&[blanket, "allow ls *"]);
            assert_eq!(decide(&p, START, "sudo ls src"), Decision::Ask, "{blanket}");
        }
        let p = user(&["allow sudo -u=postgres psql *"]);
        assert_eq!(
            decide(&p, START, "sudo -u postgres psql -l"),
            Decision::Allow
        );
        assert_eq!(decide(&p, START, "sudo -u root psql -l"), Decision::Ask);
        assert_eq!(
            decide(&p, START, "sudo psql -l"),
            Decision::Ask,
            "a named flag must be given: without it psql runs as root"
        );
        let p = user(&[
            "allow sudo -s ls *",
            "allow sudo -i ls *",
            "allow sudo -E -s ls *",
        ]);
        for shell in ["sudo -s ls src", "sudo -i ls src", "sudo -Es ls src"] {
            assert_eq!(decide(&p, START, shell), Decision::Ask, "{shell}");
        }
        let p = user(&["allow doas -u=www ls *", "allow su ls *", "allow su -c=* *"]);
        assert_eq!(decide(&p, START, "doas -u www ls src"), Decision::Allow);
        assert_eq!(decide(&p, START, "su -c 'ls src'"), Decision::Ask);
        let p = user(&["allow sudo ls *", "deny sudo"]);
        assert_eq!(
            decide(&p, START, "sudo ls src"),
            Decision::Deny,
            "a deny still refuses"
        );
        let p = user(&["allow sudo ls *"]);
        assert_eq!(
            decide(&p, START, "Sudo ls src"),
            Decision::Ask,
            "another spelling of the wrapper is still one"
        );
        let p = user(&["allow run0 *", "allow ls *"]);
        assert_eq!(
            decide(&p, START, "run0 ls src"),
            Decision::Ask,
            "run0 is a wrapper too"
        );
    }

    /// **A call that sends is a write, by any method it is given** (review
    /// 2026-10-11, E1): `gh api`'s node is a read and its host no act, so a
    /// write method or a body ran with no rule, unattended; a program may
    /// take the last of two methods, so none is the gate's to pick; and an
    /// allow naming a method holds only where every one given is it.
    #[test]
    fn a_call_that_sends_is_a_write_by_any_method() {
        let p = user(&[]);
        assert_eq!(decide(&p, START, "gh api user"), Decision::Allow, "premise");
        for line in [
            "gh api -X PATCH repos/o/r",
            "gh api -f title=x repos/o/r/issues",
            "gh api --input body.json repos/o/r/issues",
        ] {
            assert_eq!(decide(&p, START, line), Decision::Ask, "{line}");
        }
        let p = user(&["allow gh api *"]);
        assert_eq!(
            decide(&p, START, "gh api -X PATCH repos/o/r"),
            Decision::Allow,
            "yours"
        );
        let p = user(&["allow gh api -X=GET"]);
        assert_eq!(decide(&p, START, "gh api -X GET user"), Decision::Allow);
        assert_eq!(
            decide(&p, START, "gh api -X GET -X DELETE user"),
            Decision::Ask
        );
        let p = user(&["allow curl *", "allow :fetch api.example.com"]);
        let url = "https://api.example.com/x";
        assert_eq!(
            decide(&p, START, &format!("curl -X GET {url}")),
            Decision::Allow
        );
        assert_eq!(
            decide(&p, START, &format!("curl -X GET -X DELETE {url}")),
            Decision::Ask
        );
    }

    /// **A config key runs something unless it is known not to** (review
    /// 2026-10-11, E1): git's list of keys that name a program missed
    /// `include.path`, `…uploadpack`, `…program`; the list is of the
    /// harmless now (`kv_safe`), and every other key asks.
    #[test]
    fn a_config_key_runs_something_unless_known_not_to() {
        let p = user(&[]);
        assert_eq!(decide(&p, START, "git status"), Decision::Allow, "premise");
        for line in [
            "git -c include.path=/work/me/proj/x.cfg status",
            "git -c remote.origin.uploadpack=x status",
            "git -c gpg.ssh.program=x status",
            "git -c core.pager=less log",
        ] {
            assert_eq!(decide(&p, START, line), Decision::Ask, "{line}");
        }
        for line in ["git -c color.ui=always status", "git -c User.Name=x status"] {
            assert_eq!(decide(&p, START, line), Decision::Allow, "{line}");
        }
    }

    /// **Where implementations differ on a flag's value, both readings
    /// stand** (review 2026-10-11, E2): BSD sed's `-i SUFFIX` read `sed -i
    /// s/x/y/ f` with `f` as the script, and GNU sed edits `f`; GNU's
    /// reading alone would miss BSD's script (`w FILE` in it).
    #[test]
    fn both_readings_of_an_in_place_flag_stand() {
        let p = user(&["allow sed *", "deny :write /srv/keep"]);
        for line in [
            "sed -i '' s/x/y/ /srv/keep/a",
            "sed -i s/x/y/ /srv/keep/a",
            "sed -i '' 's/a/b/w /srv/keep/x' notes.txt",
        ] {
            assert_eq!(decide(&p, START, line), Decision::Deny, "{line}");
        }
        assert_eq!(
            decide(&p, START, "sed -i '' s/x/y/ notes.txt"),
            Decision::Allow,
            "inside"
        );
        assert_eq!(
            decide(&user(&[]), START, "sed -n 1,5p notes.txt"),
            Decision::Allow
        );
    }

    /// **A shipped guard found anywhere is the project's, not its home's**
    /// (Steven, 10-11): a `.claude` that contains where Shannon started (a
    /// project in `.claude/worktrees/x`) guarded every path in it, so every
    /// write and every unmodelled word asked; one inside the project, and
    /// the harness's own outside it, are guarded as ever.
    #[test]
    fn a_guard_found_anywhere_reads_below_the_start() {
        let start = "/work/me/.claude/worktrees/w";
        let p = user(&["allow mytool *"]);
        assert_eq!(
            decide(&p, START, "echo x > notes.txt"),
            Decision::Allow,
            "premise"
        );
        for line in ["echo x > notes.txt", "mytool build", "echo x > src/a.rs"] {
            assert_eq!(decide(&p, start, line), Decision::Allow, "{line}");
        }
        for line in [
            "echo x > .claude/settings.json",
            "echo x > sub/.claude/settings.json",
            "echo x > /work/me/.claude/settings.json",
            "echo x > ../../settings.json",
        ] {
            assert_eq!(decide(&p, start, line), Decision::Ask, "{line}");
        }
    }

    /// **A deny meets every spelling of a value** (review 2026-10-11, E1):
    /// without a model no reading of a dashed word is the gate's to pick, so
    /// contact takes them all — `=`, attached, after any letter of a cluster,
    /// the next word — and a deny's plain word meets a value as a word. Each
    /// row was an Allow under `allow mytool *`.
    #[test]
    fn a_deny_meets_every_spelling_of_a_value() {
        for (deny, lines) in [
            (
                "deny mytool -l=rust",
                &["mytool -lrust x", "mytool -vlrust x"][..],
            ),
            (
                "deny mytool -l rust",
                &[
                    "mytool -l=rust x",
                    "mytool -lrust x",
                    "mytool -vlrust",
                    "mytool -l=rust",
                ][..],
            ),
            (
                "deny mytool -o=*",
                &["mytool -oout.txt", "mytool -o./out.txt", "mytool -oc++"][..],
            ),
            ("deny mytool --lang rust", &["mytool --lang=rust x"][..]),
            ("deny mytool rust", &["mytool -l=rust", "mytool -lrust"][..]),
        ] {
            let p = user(&["allow mytool *", deny]);
            for line in lines {
                assert_eq!(decide(&p, START, line), Decision::Deny, "{deny}: {line}");
            }
        }
        let p = user(&["allow mytool -C=*", "deny mytool -C sub"]);
        assert_eq!(decide(&p, START, "mytool -C=sub"), Decision::Deny);
        // Contact reads no declaration: a declared value is still a word.
        let p = user(&["allow mytool *", "ask mytool -l=go", "deny mytool rust"]);
        assert_eq!(decide(&p, START, "mytool -l rust"), Decision::Deny);
        // And meets only what is there.
        let p = user(&["allow mytool *", "deny mytool -l=rust"]);
        for line in ["mytool -l go x", "mytool -vgo x", "mytool --lang=rust"] {
            assert_eq!(decide(&p, START, line), Decision::Allow, "{line}");
        }
    }

    /// **A value is a place, with or without a `/`** (review 2026-10-11, E1):
    /// a slash-free value walked past every path rule and the credential
    /// guard, attached or after `=`, while the same word apart asked.
    #[test]
    fn a_value_is_a_place_with_or_without_a_slash() {
        let p = user(&["allow mytool *", "allow cat *"]);
        assert_eq!(
            decide(&p, START, "mytool --config .env"),
            Decision::Ask,
            "premise"
        );
        for line in [
            "mytool --config=.env",
            "mytool -c.env",
            "cat --frob=.env",
            "mytool -C..",
            "mytool -C=..",
        ] {
            assert_eq!(decide(&p, START, line), Decision::Ask, "{line}");
        }
        assert_eq!(
            decide(&p, START, "mytool -C=out"),
            Decision::Allow,
            "inside"
        );
        let long = format!(
            "mytool -{}",
            "v".repeat(super::super::acts::MAX_CLUSTER + 1)
        );
        assert_eq!(
            decide(&p, START, &long),
            Decision::Ask,
            "a cluster past reading"
        );
        let short = format!("mytool -{}", "v".repeat(super::super::acts::MAX_CLUSTER));
        assert_eq!(decide(&p, START, &short), Decision::Allow, "at the bound");
    }

    /// **A program's fence meets a redirection** (Steven, 10-11): the shell
    /// writes there for the program, and a deny never silently stops
    /// protecting its place.
    #[test]
    fn a_programs_fence_meets_a_redirection() {
        let p = user(&[
            "allow cat *",
            "allow :write /srv/**",
            "deny cat /srv/keep/**",
        ]);
        assert_eq!(
            decide(&p, START, "cat a.txt > /srv/out/x"),
            Decision::Allow,
            "premise"
        );
        assert_eq!(decide(&p, START, "cat a.txt > /srv/keep/x"), Decision::Deny);
        assert_eq!(
            decide(&p, START, "cat a.txt >> /srv/keep/x"),
            Decision::Deny
        );
    }

    /// **An exec grant covers the exec act, not the model** (Steven, 10-11):
    /// a program run by its path is decided by its model as when named bare
    /// — the grant stands in for an unmodelled one, a script, only. Before,
    /// a grant of the directory ran whatever its model asks about.
    #[test]
    fn an_exec_grant_covers_the_exec_act_not_the_model() {
        let p = user(&["allow :exec /usr/bin"]);
        for (bare, by_path) in [
            ("find . -delete", "/usr/bin/find . -delete"),
            ("gh auth token", "/usr/bin/gh auth token"),
        ] {
            assert_eq!(decide(&p, START, bare), Decision::Ask, "premise: {bare}");
            assert_eq!(decide(&p, START, by_path), Decision::Ask, "{by_path}");
        }
        assert_eq!(
            decide(&p, START, "/usr/bin/ls src"),
            Decision::Allow,
            "a read"
        );
        let p = user(&["allow :* /**"]);
        assert_eq!(
            decide(&p, START, "rm -rf /srv/out/x"),
            Decision::Ask,
            "premise"
        );
        assert_eq!(decide(&p, START, "/bin/rm -rf /srv/out/x"), Decision::Ask);
        let p = user(&["allow :exec scripts"]);
        assert_eq!(
            decide(&p, START, "./scripts/build.sh --fast"),
            Decision::Allow,
            "a script: running it is all it does"
        );
    }

    /// **A rule through an inner program gives each program its own flags**:
    /// those written before the inner program are the outer's (named, so
    /// present), those after the inner's. Before, the outer act's were
    /// cleared and the inner's rule carried both, so `allow uv --quiet run
    /// pytest *` covered neither `uv --quiet run pytest` nor anything else.
    #[test]
    fn a_rule_through_an_inner_program_gives_each_its_own_flags() {
        let p = user(&["allow uv --quiet run pytest *"]);
        assert_eq!(
            decide(&p, START, "uv --quiet run pytest -x"),
            Decision::Allow
        );
        assert_eq!(
            decide(&p, START, "uv -q run pytest -x"),
            Decision::Allow,
            "a spelling"
        );
        assert_eq!(
            decide(&p, START, "uv run pytest -x"),
            Decision::Ask,
            "a named flag is present, on the program it was written for"
        );
        assert_eq!(decide(&p, START, "uv --quiet run make"), Decision::Ask);
    }

    /// **Shannon's privilege wrappers are its models' marks**: the table
    /// that decides what lifts each, and the `privilege` its model carries,
    /// name the same programs — built from both sides.
    #[test]
    fn every_privilege_wrapper_is_marked_in_its_model() {
        let marked: Vec<&str> = Models::shipped()
            .names()
            .filter(|n| Models::shipped().get(n).is_some_and(|m| m.privilege))
            .collect();
        for (w, _) in super::super::acts::PRIVILEGE_WRAPPERS {
            assert!(
                marked.contains(w),
                "{w} is in the table, not marked in its model"
            );
        }
        for m in marked {
            assert!(
                super::super::acts::PRIVILEGE_WRAPPERS
                    .iter()
                    .any(|(w, _)| *w == m),
                "{m} is marked in its model, not in the table"
            );
        }
    }

    /// **An unmodelled allow is a literal prefix of the argv**, in order;
    /// its deny or ask is met by its words anywhere.
    #[test]
    fn an_unmodelled_allow_is_a_prefix() {
        let p = user(&["allow make test *"]);
        assert_eq!(decide(&p, START, "make test -j4"), Decision::Allow);
        assert_eq!(
            decide(&p, START, "make clean test"),
            Decision::Ask,
            "the same words in another order are another command"
        );
        let p = user(&["allow make test"]);
        assert_eq!(decide(&p, START, "make test"), Decision::Allow);
        assert_eq!(
            decide(&p, START, "make test CC=cc"),
            Decision::Ask,
            "past the prefix"
        );
        let p = user(&["allow make *", "deny make install"]);
        assert_eq!(
            decide(&p, START, "make -j4 install"),
            Decision::Deny,
            "contact"
        );
    }

    /// **`-f=` declares that an unmodelled program's flag takes a value**
    /// (Steven, 10-10): the word after it is the flag's, wherever the flag
    /// is, and still a place to the path rules.
    #[test]
    fn a_declared_value_goes_with_its_flag() {
        // Premise: a program no model may ever name, so the reader never
        // knows `-l`'s arity, and without a declaration `rust` is an
        // argument the rule left out.
        assert!(Models::shipped().get("mytool").is_none());
        let p = user(&["allow mytool -l"]);
        assert_eq!(decide(&p, START, "mytool -l rust"), Decision::Ask);
        let program = |p: &Policy, input: &str| {
            decide_acts(p, START, input)
                .into_iter()
                .find(|a| matches!(a.act, ActKind::Program { .. }))
                .unwrap()
        };
        assert_eq!(program(&p, "mytool -l rust").uncovered, ["word rust"]);

        let p = user(&["allow mytool -l=|--language="]);
        assert_eq!(decide(&p, START, "mytool -l rust"), Decision::Allow);
        assert_eq!(decide(&p, START, "mytool --language rust"), Decision::Allow);
        assert_eq!(decide(&p, START, "mytool -l=rust"), Decision::Allow);
        // The value only: another word is an argument, and a value-less
        // flag is not the declared one.
        assert_eq!(decide(&p, START, "mytool -l rust a.txt"), Decision::Ask);
        assert_eq!(decide(&p, START, "mytool a.txt -l rust"), Decision::Ask);
        assert_eq!(decide(&p, START, "mytool -l"), Decision::Ask);
        // Still a place: outside the project it asks until a path rule.
        assert_eq!(decide(&p, START, "mytool -l /srv/x/a"), Decision::Ask);
        let p = user(&["allow mytool -l=", "allow :* /srv/x"]);
        assert_eq!(decide(&p, START, "mytool -l /srv/x/a"), Decision::Allow);

        // Any rule declaring it makes the flag valued: one naming it
        // without `=` then matches every use; a value it names decides.
        let p = user(&["allow mytool -l", "ask mytool -l=go"]);
        assert_eq!(decide(&p, START, "mytool -l rust"), Decision::Allow);
        assert_eq!(decide(&p, START, "mytool -l go"), Decision::Ask);
        // A deny on every value meets the flag with none read, too.
        let p = user(&["allow mytool *", "deny mytool -l=*"]);
        assert_eq!(decide(&p, START, "mytool -l rust"), Decision::Deny);
        assert_eq!(decide(&p, START, "mytool -l"), Decision::Deny);
        assert_eq!(decide(&p, START, "mytool -l -p"), Decision::Deny);
        assert_eq!(decide(&p, START, "mytool -p"), Decision::Allow);

        // The prefix is the arguments: a declared value is no part of it,
        // in the command or in the rule.
        let p = user(&["allow mytool -C= build"]);
        assert_eq!(decide(&p, START, "mytool -C sub build"), Decision::Allow);
        assert_eq!(decide(&p, START, "mytool build -C sub"), Decision::Allow);
        assert_eq!(decide(&p, START, "mytool -C sub other"), Decision::Ask);
        // A declared flag no allow names takes its value with it into what
        // was not covered, so `A` names this value.
        let p = user(&["allow mytool -p", "ask mytool -l=go"]);
        assert_eq!(program(&p, "mytool -p -l rust").uncovered, ["flag -l=rust"]);
        // A word a rule wrote after a flag another rule declares is the
        // value it names, not any value.
        let p = user(&["allow mytool -C sub build", "ask mytool -C=/"]);
        assert_eq!(decide(&p, START, "mytool -C sub build"), Decision::Allow);
        assert_eq!(decide(&p, START, "mytool build -C sub"), Decision::Allow);
        assert_eq!(decide(&p, START, "mytool -C other build"), Decision::Ask);
    }

    /// **A `re:` allow covers the command and none of its paths**; a `re:`
    /// deny holds whatever else allows.
    #[test]
    fn a_regex_rule_reads_the_line() {
        let p = user(&[]);
        assert_eq!(
            decide(&p, START, "sed -n 1,5p Cargo.toml"),
            Decision::Allow,
            "the shipped allow"
        );
        assert_eq!(
            decide(&p, START, "sed -n 1,5p /etc/hosts"),
            Decision::Ask,
            "the path is outside"
        );
        let p = user(&["allow git *", r"deny re:^git push( |$)"]);
        assert_eq!(decide(&p, START, "git push origin main"), Decision::Deny);
        assert_eq!(
            decide(&p, START, "/usr/bin/git push origin main"),
            Decision::Deny,
            "by its name too"
        );
        assert_eq!(decide(&p, START, "git status"), Decision::Allow);
    }

    /// **Each act says why.** The pane's material: a rule cited, a default
    /// named, a hard ask, the nodes an allow did not cover.
    #[test]
    fn verdicts_carry_their_reasons() {
        let p = user(&["allow cargo test"]);
        let env = env_table(&[]);
        let cx = Context {
            start: START,
            home: home(),
            models: Models::shipped(),
            env: &env,
        };
        let knows = |_: &str| false;
        let gate = Gate {
            policy: &p,
            cx: &cx,
            knows: &knows,
        };
        let stages = decide_line(&gate, "cargo test --frobnicate").unwrap();
        let program = stages[0]
            .acts
            .iter()
            .find(|a| matches!(a.act, ActKind::Program { .. }))
            .unwrap();
        assert_eq!(program.decision, Decision::Ask);
        assert!(matches!(&program.by, By::Rules(c) if c[0].source == "allow cargo test"));
        assert_eq!(program.uncovered, ["flag --frobnicate"]);
        assert!(program.model.is_some(), "the model that read it");
        // Past a read, a harmless flag the form does not name is named in
        // what it did not cover — what `A` would add (Steven, 10-10).
        let stages = decide_line(&gate, "cargo test --workspace").unwrap();
        let program = stages[0]
            .acts
            .iter()
            .find(|a| matches!(a.act, ActKind::Program { .. }))
            .unwrap();
        assert_eq!(program.decision, Decision::Ask);
        assert_eq!(program.uncovered, ["flag --workspace"]);
        let stages = decide_line(&gate, "cat src/main.rs").unwrap();
        assert!(
            stages[0]
                .acts
                .iter()
                .any(|a| matches!(a.by, By::Default(NoRule::Inside(Some(Effect::Read)))))
        );
        assert!(
            stages[0]
                .acts
                .iter()
                .any(|a| matches!(a.by, By::Default(NoRule::ModelledRead)))
        );
        let stages = decide_line(&gate, "sudo ls").unwrap();
        assert!(
            stages[0]
                .acts
                .iter()
                .any(|a| matches!(a.by, By::Hard(Hard::Privilege)))
        );
    }

    /// **A narrower rule never loosens a deny** (10-13). Across tiers, the
    /// shipped `~/.ssh` guard — an ask, narrower than the user's `deny :*
    /// ~` — took its place, and `cat ~/.ssh/config` asked. Within one, the
    /// user's own `ask :* /srv/keep`, met from above by `rm -r /srv`, took
    /// the place of their `deny :delete /srv`. Each still narrows an allow.
    #[test]
    fn a_narrower_rule_never_loosens_a_deny() {
        let p = user(&["deny :* ~", "deny :delete /srv", "ask :* /srv/keep"]);
        for line in ["cat ~/.ssh/config", "rm -r ~", "rm -r /srv"] {
            assert_eq!(decide(&p, START, line), Decision::Deny, "{line}");
        }
        let p = user(&["allow :read ~", "allow :delete /srv", "ask :* /srv/keep"]);
        for line in ["cat ~/.ssh/config", "rm -r /srv"] {
            assert_eq!(decide(&p, START, line), Decision::Ask, "{line}");
        }
        assert_eq!(decide(&p, START, "cat ~/notes"), Decision::Allow, "premise");
    }

    /// **A contact from above is the reason only where it raised the
    /// decision** (Steven, 10-13: "`/` holds Shannon's own state" was said of
    /// `rm -r /`, where he expected "outside"). Where the place asks on its
    /// own account — outside, a delete inside, a rule of the user's naming
    /// it — that account is `by`, and the contact is `reaches`; where it
    /// would be allowed, the contact is `by`.
    #[test]
    fn a_contact_is_the_reason_only_where_it_raised_the_decision() {
        let place = |p: &Policy, line: &str| {
            decide_acts(p, START, line)
                .into_iter()
                .find(|a| matches!(a.act, ActKind::Path { .. }))
                .expect("a path act")
        };
        let ssh = |cited: &[Cited]| cited.iter().any(|c| c.source == "ask :* \\c~/.ssh");
        let p = user(&["ask :* keys"]);
        let root = place(&p, "rm -r /");
        assert_eq!(root.by, By::Default(NoRule::Outside), "{root:?}");
        assert!(ssh(&root.reaches), "{root:?}");
        let here = place(&p, "rm -r .");
        assert_eq!(here.decision, Decision::Ask);
        assert_eq!(here.by, By::Default(NoRule::Inside(Some(Effect::Delete))));
        assert!(
            here.reaches.iter().any(|c| c.source == "ask :* keys"),
            "{here:?}"
        );
        // Read inside, `.` would run unasked: what it holds is why it asks.
        let read = place(&p, "rg foo .");
        assert_eq!(read.decision, Decision::Ask);
        assert!(read.reaches.is_empty(), "{read:?}");
        assert!(
            matches!(&read.by, By::Rules(c) if c.iter().all(|c| c.reached)),
            "{read:?}"
        );
        // A rule of the user's naming the place is its account.
        let p = user(&["ask :read ~"]);
        let home = place(&p, "rg foo ~");
        assert!(
            matches!(&home.by, By::Rules(c) if c.iter().all(|c| c.source == "ask :read ~")),
            "{home:?}"
        );
        assert!(ssh(&home.reaches), "{home:?}");
        // Only what is met softly is what a place holds; a rule naming it is
        // its account, never echoed back (review of c9c40fe, C).
        assert!(home.reaches.iter().all(|c| c.reached), "{home:?}");
        let named = place(&user(&[]), "cat ~/.ssh/config");
        assert!(named.reaches.is_empty(), "{named:?}");
        // Under a deny, what is met softly asks, and is no part of it.
        let denied = place(&user(&["deny :read ~"]), "rg foo ~");
        assert_eq!(denied.decision, Decision::Deny);
        assert!(denied.reaches.is_empty(), "{denied:?}");
    }

    /// **`DIR/**` names less than `DIR` only for a delete** (review of
    /// e10b6d5, A, B and C). For any other act the two name the same paths,
    /// so neither is the more specific and the stricter decides: a user's
    /// `allow :read /srv/notes/**` took the place of their `deny :read
    /// /srv/notes`. For a delete, `DIR/**` is what is below the directory,
    /// and an exception to a deny of it.
    #[test]
    fn a_trailing_double_star_is_narrower_only_for_a_delete() {
        let p = user(&["deny :read /srv/notes", "allow :read /srv/notes/**"]);
        assert_eq!(decide(&p, START, "cat /srv/notes/a"), Decision::Deny);
        let p = user(&["deny :read /srv/notes/**", "allow :read /srv/notes"]);
        assert_eq!(decide(&p, START, "cat /srv/notes/a"), Decision::Deny);
        let p = user(&["deny :delete build", "allow :delete build/**"]);
        assert_eq!(decide(&p, START, "rm build/a"), Decision::Allow);
        assert_eq!(decide(&p, START, "rm -rf build"), Decision::Deny);
    }

    /// **What a verdict cites is what decided it** (review of c9c40fe, A and
    /// C: no test held the citations, so the rules a reason names could
    /// drift). Under a user's `allow :read ~`, `cat ~/.ssh/config` asks by
    /// the guard alone; the allow it narrows is no reason.
    #[test]
    fn a_verdict_cites_what_decided_it() {
        let acts = decide_acts(&user(&["allow :read ~"]), START, "cat ~/.ssh/config");
        let path = acts
            .iter()
            .find(|a| matches!(a.act, ActKind::Path { .. }))
            .expect("a path act");
        assert_eq!(path.decision, Decision::Ask);
        let By::Rules(cited) = &path.by else {
            panic!("{path:?}")
        };
        let sources: Vec<&str> = cited.iter().map(|c| c.source.as_str()).collect();
        assert_eq!(sources, ["ask :* \\c~/.ssh"], "{path:?}");
        // Met from above, the same: what `~` holds, never the allow of it —
        // a reason cites rules naming the place or rules met softly, never
        // both, and the pane reads it so.
        let acts = decide_acts(&user(&["allow :read ~"]), START, "rg foo ~");
        let home = acts
            .iter()
            .find(|a| matches!(a.act, ActKind::Path { .. }))
            .expect("a path act");
        let By::Rules(cited) = &home.by else {
            panic!("{home:?}")
        };
        assert!(cited.iter().all(|c| c.reached), "{home:?}");
    }
}
